//! 検証パイプライン駆動。候補を一時 Cargo プロジェクトへ展開し、
//! compile → test → 採点 までを実行する。

use crate::analysis::{count_clippy_warnings, count_ruff_findings, SourceMetrics};
use crate::config::Rubric;
use crate::model::{Candidate, Evaluation, Language, StageOutcome};
use crate::{runner, scoring, wasm};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Wasm ステージのターゲット。
const WASM_TARGET: &str = "wasm32-wasip1";

/// Wasm 実行のオプション。`entry` 指定時のみ Wasm ステージを実行する。
#[derive(Debug, Clone, Copy)]
pub struct WasmOptions<'a> {
    pub entry: Option<&'a str>,
    pub fuel: u64,
    pub max_memory_bytes: usize,
}

impl Default for WasmOptions<'_> {
    fn default() -> Self {
        WasmOptions {
            entry: None,
            fuel: 100_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

/// 一時プロジェクトの Cargo.toml。依存なしの最小構成。
const CARGO_TOML_TEMPLATE: &str = r#"[package]
name = "candidate"
version = "0.0.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#;

/// proptest を dev-dependency に持つ Cargo.toml。
const CARGO_TOML_WITH_PROPTEST: &str = r#"[package]
name = "candidate"
version = "0.0.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dev-dependencies]
proptest = "1"
"#;

/// 候補ファイルを読み込んで `Candidate` を生成する。
/// `id` はファイル名 stem（拡張子なし）。
pub fn load_candidate(path: &Path) -> Result<Candidate> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("候補ファイルの読込に失敗: {}", path.display()))?;
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("candidate")
        .to_string();
    let language = path
        .extension()
        .and_then(|e| e.to_str())
        .map(Language::from_extension)
        .unwrap_or(Language::Rust);
    Ok(Candidate {
        id,
        source,
        language,
    })
}

/// テストディレクトリ内の指定拡張子ファイルを `dest` へコピーする。
fn copy_ext(src_dir: &Path, dest: &Path, ext: &str) -> Result<()> {
    fs::create_dir_all(dest).context("コピー先ディレクトリの作成に失敗")?;
    for entry in fs::read_dir(src_dir)
        .with_context(|| format!("テストディレクトリの読込に失敗: {}", src_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            let name = path.file_name().unwrap();
            fs::copy(&path, dest.join(name))
                .with_context(|| format!("テストのコピーに失敗: {}", path.display()))?;
        }
    }
    Ok(())
}

/// エントリ名が Rust の単純な識別子か検証する（ハーネスへの注入防止）。
fn is_valid_entry(name: &str) -> bool {
    !name.is_empty()
        && name.chars().enumerate().all(|(i, c)| {
            c == '_'
                || if i == 0 {
                    c.is_alphabetic()
                } else {
                    c.is_alphanumeric()
                }
        })
}

/// Wasm ステージ: 候補を `wasm32-wasip1` にビルドし、生成ハーネスの
/// エントリ関数を fuel/メモリ制限付き wasmtime で実行する。
/// 返り値は `(StageOutcome, 消費 fuel)`。
fn run_wasm_stage(
    root: &Path,
    entry: &str,
    timeout: Duration,
    opts: &WasmOptions,
) -> (StageOutcome, Option<u64>) {
    if !is_valid_entry(entry) {
        return (
            StageOutcome::Failed {
                detail: format!("不正なエントリ名: {entry}"),
            },
            None,
        );
    }

    // 生成ハーネス（cargo が src/main.rs を bin "candidate" として検出）。
    let harness = format!("fn main() {{ candidate::{entry}(); }}\n");
    if let Err(e) = fs::write(root.join("src").join("main.rs"), harness) {
        return (
            StageOutcome::Failed {
                detail: format!("ハーネス書込失敗: {e}"),
            },
            None,
        );
    }

    // wasm ビルド
    let mut build = Command::new("cargo");
    build
        .args(["build", "--target", WASM_TARGET, "--release"])
        .current_dir(root);
    let build_outcome = runner::run_stage("wasm-build", build, timeout);
    if !build_outcome.is_passed() {
        return (build_outcome, None);
    }

    // wasm 実行
    let artifact = root
        .join("target")
        .join(WASM_TARGET)
        .join("release")
        .join("candidate.wasm");
    let wo = wasm::run_wasm(&artifact, opts.fuel, opts.max_memory_bytes);
    (wo.outcome, wo.fuel_used)
}

/// 各検証ステージの生結果。言語ごとのランナーが埋め、`assemble` が採点する。
/// `lint` / `lint_warnings` は Rust では clippy、Python では ruff を指す
/// （`Evaluation` 上は互換のため clippy フィールドに格納する）。
struct StageResults {
    compile: StageOutcome,
    test: StageOutcome,
    lint: StageOutcome,
    lint_warnings: usize,
    prop_test: StageOutcome,
    wasm: StageOutcome,
    wasm_fuel_used: Option<u64>,
}

impl StageResults {
    /// compile 失敗時の既定（後続ステージはすべて Skipped）。
    fn skipped_after_compile(compile: StageOutcome) -> Self {
        StageResults {
            compile,
            test: StageOutcome::Skipped,
            lint: StageOutcome::Skipped,
            lint_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
        }
    }
}

/// 1 候補を一時ディレクトリへ展開し、言語に応じた検証ステージを実行して採点する。
///
/// - compile が通らなかった場合、後続ステージはすべて `Skipped`。
/// - `tests_dir`: 統合テストを置いたディレクトリ（任意）。
/// - `prop_tests_dir`: proptest ファイルのディレクトリ（Rust のみ）。
/// - `wasm`: Wasm サンドボックス実行のオプション（Rust のみ）。`entry` 指定時のみ実行。
pub fn evaluate_candidate(
    candidate: &Candidate,
    timeout: Duration,
    rubric: &Rubric,
    tests_dir: Option<&Path>,
    prop_tests_dir: Option<&Path>,
    wasm_opts: &WasmOptions,
) -> Result<Evaluation> {
    let dir = tempfile::tempdir().context("一時ディレクトリの生成に失敗")?;
    let root = dir.path();

    let results = match candidate.language {
        Language::Rust => run_rust_stages(
            root,
            candidate,
            timeout,
            tests_dir,
            prop_tests_dir,
            wasm_opts,
        )?,
        Language::Python => run_python_stages(root, candidate, timeout, tests_dir)?,
    };

    Ok(assemble(candidate, results, rubric))
}

/// ステージ結果を採点して `Evaluation` を組み立てる（言語非依存）。
fn assemble(candidate: &Candidate, r: StageResults, rubric: &Rubric) -> Evaluation {
    let metrics = SourceMetrics::analyze(&candidate.source);
    let axes = scoring::axes_without_performance(
        &r.compile,
        &r.test,
        &r.prop_test,
        &r.lint,
        r.lint_warnings,
        &r.wasm,
        &metrics,
        rubric,
    );
    let score = axes.total();
    Evaluation {
        candidate_id: candidate.id.clone(),
        compile: r.compile,
        test: r.test,
        clippy: r.lint,
        clippy_warnings: r.lint_warnings,
        prop_test: r.prop_test,
        wasm: r.wasm,
        wasm_fuel_used: r.wasm_fuel_used,
        axes,
        score,
    }
}

/// Rust 候補: 一時 Cargo プロジェクトで compile → test → clippy → prop_test → wasm。
fn run_rust_stages(
    root: &Path,
    candidate: &Candidate,
    timeout: Duration,
    tests_dir: Option<&Path>,
    prop_tests_dir: Option<&Path>,
    wasm_opts: &WasmOptions,
) -> Result<StageResults> {
    fs::create_dir_all(root.join("src")).context("src ディレクトリの作成に失敗")?;
    let toml = if prop_tests_dir.is_some() {
        CARGO_TOML_WITH_PROPTEST
    } else {
        CARGO_TOML_TEMPLATE
    };
    fs::write(root.join("Cargo.toml"), toml).context("Cargo.toml の書込に失敗")?;
    fs::write(root.join("src").join("lib.rs"), &candidate.source)
        .context("候補ソースの書込に失敗")?;
    if let Some(dir) = tests_dir {
        copy_ext(dir, &root.join("tests"), "rs")?;
    }

    let mut build = Command::new("cargo");
    build.arg("build").current_dir(root);
    let compile = runner::run_stage("compile", build, timeout);
    if !compile.is_passed() {
        return Ok(StageResults::skipped_after_compile(compile));
    }

    let mut t = Command::new("cargo");
    t.arg("test").current_dir(root);
    let test = runner::run_stage("test", t, timeout);

    let mut c = Command::new("cargo");
    c.args(["clippy", "--", "-W", "clippy::all"])
        .current_dir(root);
    let (lint, lint_stderr) = runner::run_stage_capture("clippy", c, timeout);
    let lint_warnings = count_clippy_warnings(&lint_stderr);

    let prop_test = if let Some(prop_dir) = prop_tests_dir {
        copy_ext(prop_dir, &root.join("tests"), "rs")?;
        let mut pt = Command::new("cargo");
        pt.arg("test").current_dir(root);
        runner::run_stage("prop_test", pt, timeout)
    } else {
        StageOutcome::Skipped
    };

    let (wasm, wasm_fuel_used) = match wasm_opts.entry {
        Some(entry) => run_wasm_stage(root, entry, timeout, wasm_opts),
        None => (StageOutcome::Skipped, None),
    };

    Ok(StageResults {
        compile,
        test,
        lint,
        lint_warnings,
        prop_test,
        wasm,
        wasm_fuel_used,
    })
}

/// Python 候補: py_compile（構文チェック）→ pytest → ruff。
/// prop_test / wasm は非対応のため Skipped。
fn run_python_stages(
    root: &Path,
    candidate: &Candidate,
    timeout: Duration,
    tests_dir: Option<&Path>,
) -> Result<StageResults> {
    fs::write(root.join("candidate.py"), &candidate.source).context("候補ソースの書込に失敗")?;
    if let Some(dir) = tests_dir {
        copy_ext(dir, root, "py")?;
    }

    // compile: 構文チェック
    let mut c = Command::new("python3");
    c.args(["-m", "py_compile", "candidate.py"])
        .current_dir(root);
    let compile = runner::run_stage("compile", c, timeout);
    if !compile.is_passed() {
        return Ok(StageResults::skipped_after_compile(compile));
    }

    // test: pytest（exit 5 = テスト未収集 → 成功扱い、Rust の 0 tests と同様）
    let mut t = Command::new("sh");
    t.arg("-c")
        .arg("python3 -m pytest -q . ; c=$?; [ $c -eq 0 ] || [ $c -eq 5 ]")
        .current_dir(root);
    let test = runner::run_stage("test", t, timeout);

    // lint: ruff（指摘があっても stage は成功扱い、件数のみ採点に使う）
    let mut l = Command::new("sh");
    l.arg("-c")
        .arg("ruff check --output-format=concise candidate.py 1>&2; true")
        .current_dir(root);
    let (lint, lint_out) = runner::run_stage_capture("ruff", l, timeout);
    let lint_warnings = count_ruff_findings(&lint_out);

    Ok(StageResults {
        compile,
        test,
        lint,
        lint_warnings,
        prop_test: StageOutcome::Skipped,
        wasm: StageOutcome::Skipped,
        wasm_fuel_used: None,
    })
}
