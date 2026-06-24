//! 検証パイプライン駆動。候補を一時 Cargo プロジェクトへ展開し、
//! compile → test → 採点 までを実行する。

use crate::analysis::{count_clippy_warnings, SourceMetrics};
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
    Ok(Candidate {
        id,
        source,
        language: Language::Rust,
    })
}

/// 外部テストディレクトリの `.rs` ファイルを一時プロジェクトの `tests/` へコピーする。
fn copy_tests(src_dir: &Path, dest_tests: &Path) -> Result<()> {
    fs::create_dir_all(dest_tests).context("tests ディレクトリの作成に失敗")?;
    for entry in fs::read_dir(src_dir)
        .with_context(|| format!("テストディレクトリの読込に失敗: {}", src_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let name = path.file_name().unwrap();
            fs::copy(&path, dest_tests.join(name))
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

/// 1 候補を一時 Cargo プロジェクトへ展開し、compile → test → clippy → prop_test → wasm → 採点する。
///
/// - compile が通らなかった場合、test / clippy / prop_test / wasm はすべて `Skipped`。
/// - `tests_dir`: 統合テスト `.rs` を置いたディレクトリ（任意）。
/// - `prop_tests_dir`: proptest ファイルを置いたディレクトリ（任意）。指定時は
///   proptest を dev-dependency に追加して実行する。
/// - `wasm`: Wasm サンドボックス実行のオプション。`entry` 指定時のみ実行。
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
    fs::create_dir_all(root.join("src")).context("src ディレクトリの作成に失敗")?;

    // proptest を使う場合は専用 Cargo.toml を使う
    let toml = if prop_tests_dir.is_some() {
        CARGO_TOML_WITH_PROPTEST
    } else {
        CARGO_TOML_TEMPLATE
    };
    fs::write(root.join("Cargo.toml"), toml).context("Cargo.toml の書込に失敗")?;
    fs::write(root.join("src").join("lib.rs"), &candidate.source)
        .context("候補ソースの書込に失敗")?;

    if let Some(dir) = tests_dir {
        copy_tests(dir, &root.join("tests"))?;
    }

    // compile
    let mut build = Command::new("cargo");
    build.arg("build").current_dir(root);
    let compile = runner::run_stage("compile", build, timeout);

    // test（compile 成功時のみ）
    let test = if compile.is_passed() {
        let mut t = Command::new("cargo");
        t.arg("test").current_dir(root);
        runner::run_stage("test", t, timeout)
    } else {
        StageOutcome::Skipped
    };

    // clippy（compile 成功時のみ。stderr から warning 数を取得）
    let (clippy, clippy_warnings) = if compile.is_passed() {
        let mut c = Command::new("cargo");
        c.args(["clippy", "--", "-W", "clippy::all"])
            .current_dir(root);
        let (outcome, stderr) = runner::run_stage_capture("clippy", c, timeout);
        let warn_count = count_clippy_warnings(&stderr);
        (outcome, warn_count)
    } else {
        (StageOutcome::Skipped, 0)
    };

    // property test（--prop-tests 指定 + compile 成功時のみ）
    let prop_test = if compile.is_passed() {
        if let Some(prop_dir) = prop_tests_dir {
            copy_tests(prop_dir, &root.join("tests"))?;
            let mut pt = Command::new("cargo");
            pt.arg("test").current_dir(root);
            runner::run_stage("prop_test", pt, timeout)
        } else {
            StageOutcome::Skipped
        }
    } else {
        StageOutcome::Skipped
    };

    // Wasm サンドボックス（--wasm-entry 指定 + compile 成功時のみ）
    let (wasm_stage, wasm_fuel_used) = match (compile.is_passed(), wasm_opts.entry) {
        (true, Some(entry)) => run_wasm_stage(root, entry, timeout, wasm_opts),
        _ => (StageOutcome::Skipped, None),
    };

    let metrics = SourceMetrics::analyze(&candidate.source);
    let axes = scoring::axes_without_performance(
        &compile,
        &test,
        &prop_test,
        &clippy,
        clippy_warnings,
        &wasm_stage,
        &metrics,
        rubric,
    );
    let score = axes.total();
    Ok(Evaluation {
        candidate_id: candidate.id.clone(),
        compile,
        test,
        clippy,
        clippy_warnings,
        prop_test,
        wasm: wasm_stage,
        wasm_fuel_used,
        axes,
        score,
    })
}
