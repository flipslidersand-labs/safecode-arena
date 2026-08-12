//! 検証パイプライン駆動。候補を一時 Cargo プロジェクトへ展開し、
//! compile → test → 採点 までを実行する。

use crate::analysis::{count_eslint_findings, count_go_vet_findings, count_lint_warnings, count_ruff_findings, count_staticcheck_findings, has_criterion_bench, SourceMetrics};
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

/// Criterion を dev-dependency に持つ Cargo.toml（ベンチ用）。
const CARGO_TOML_WITH_CRITERION: &str = r#"[package]
name = "candidate"
version = "0.0.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }

[[bench]]
name = "criterion"
harness = false
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
/// `lint` / `lint_warnings` は Rust では cargo clippy、Python では ruff を指す
///
struct StageResults {
    compile: StageOutcome,
    test: StageOutcome,
    lint: StageOutcome,
    lint_warnings: usize,
    prop_test: StageOutcome,
    wasm: StageOutcome,
    wasm_fuel_used: Option<u64>,
    mutation: StageOutcome,
    mutation_caught: usize,
    mutation_total: usize,
    audit_findings: usize,
    bench_ns: Option<u64>,
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
            mutation: StageOutcome::Skipped,
            mutation_caught: 0,
            mutation_total: 0,
            audit_findings: 0,
            bench_ns: None,
        }
    }
}

/// 1 候補を一時ディレクトリへ展開し、言語に応じた検証ステージを実行して採点する。
///
/// - compile が通らなかった場合、後続ステージはすべて `Skipped`。
/// - `tests_dir`: 統合テストを置いたディレクトリ（任意）。
/// - `prop_tests_dir`: proptest ファイルのディレクトリ（Rust のみ）。
/// - `benches_dir`: Criterion ベンチファイル（`*.rs`）を置いたディレクトリ（Rust のみ）。
/// - `wasm_opts`: Wasm サンドボックス実行のオプション（Rust のみ）。`entry` 指定時のみ実行。
/// - `run_mutation`: true のとき Rust 候補に対して `cargo mutants` を実行する（デフォルト off）。
pub fn evaluate_candidate(
    candidate: &Candidate,
    timeout: Duration,
    rubric: &Rubric,
    tests_dir: Option<&Path>,
    prop_tests_dir: Option<&Path>,
    benches_dir: Option<&Path>,
    wasm_opts: &WasmOptions,
    run_mutation: bool,
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
            benches_dir,
            wasm_opts,
            run_mutation,
        )?,
        Language::Python => run_python_stages(root, candidate, timeout, tests_dir, run_mutation)?,
        Language::Go => run_go_stages(root, candidate, timeout, tests_dir, run_mutation)?,
        Language::JavaScript => run_js_stages(root, candidate, timeout, tests_dir)?,
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
        r.mutation_caught,
        r.mutation_total,
        r.audit_findings,
    );
    let score = axes.total();
    Evaluation {
        candidate_id: candidate.id.clone(),
        compile: r.compile,
        test: r.test,
        lint: r.lint,
        lint_warnings: r.lint_warnings,
        prop_test: r.prop_test,
        wasm: r.wasm,
        wasm_fuel_used: r.wasm_fuel_used,
        mutation: r.mutation,
        mutation_caught: r.mutation_caught,
        mutation_total: r.mutation_total,
        audit_findings: r.audit_findings,
        bench_ns: r.bench_ns,
        axes,
        score,
    }
}

/// Rust 候補: compile → test → lint(clippy) → prop_test → wasm → mutation → audit → bench。
fn run_rust_stages(
    root: &Path,
    candidate: &Candidate,
    timeout: Duration,
    tests_dir: Option<&Path>,
    prop_tests_dir: Option<&Path>,
    benches_dir: Option<&Path>,
    wasm_opts: &WasmOptions,
    run_mutation: bool,
) -> Result<StageResults> {
    fs::create_dir_all(root.join("src")).context("src ディレクトリの作成に失敗")?;
    let use_criterion = benches_dir.is_some() || has_criterion_bench(&candidate.source);
    let toml = if use_criterion {
        CARGO_TOML_WITH_CRITERION
    } else if prop_tests_dir.is_some() {
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
    if let Some(dir) = benches_dir {
        copy_ext(dir, &root.join("benches"), "rs")?;
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
    let lint_warnings = count_lint_warnings(&lint_stderr);

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

    let (mutation, mutation_caught, mutation_total) = if run_mutation && test.is_passed() {
        run_rust_mutation(root, timeout)
    } else {
        (StageOutcome::Skipped, 0, 0)
    };

    let audit_findings = run_cargo_audit(root, timeout);

    let bench_ns = if use_criterion {
        run_criterion_bench(root, timeout)
    } else {
        None
    };

    Ok(StageResults {
        compile,
        test,
        lint,
        lint_warnings,
        prop_test,
        wasm,
        wasm_fuel_used,
        mutation,
        mutation_caught,
        mutation_total,
        audit_findings,
        bench_ns,
    })
}

/// `cargo mutants` で Rust 候補のミューテーションテストを実行する。
///
/// - `cargo mutants` が PATH にない場合は `(Skipped, 0, 0)` を返す（減点なし）。
/// - タイムアウトは test ステージの 3 倍を目安として渡す（ミュータント数に比例するため）。
fn run_rust_mutation(root: &Path, timeout: Duration) -> (StageOutcome, usize, usize) {
    // cargo mutants が利用可能か確認
    let mutants_available = std::process::Command::new("cargo")
        .args(["mutants", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !mutants_available {
        return (StageOutcome::Skipped, 0, 0);
    }

    let out_dir = root.join("mutants-out");
    let mut cmd = Command::new("cargo");
    cmd.args([
        "mutants",
        "--manifest-path",
        root.join("Cargo.toml").to_str().unwrap_or("Cargo.toml"),
        "--output",
        out_dir.to_str().unwrap_or("mutants-out"),
        "--timeout",
        "30",
        "--jobs",
        "1",
        "--json",
    ]);

    // mutation testing は時間がかかるため timeout を 3 倍に緩める
    let mutation_timeout = timeout * 3;
    let (outcome, stdout) = runner::run_stage_capture("mutation", cmd, mutation_timeout);

    // cargo mutants がステージ自体に失敗した場合（timeout等）は outcome をそのまま返す
    if !outcome.is_passed() {
        return (outcome, 0, 0);
    }

    // outcome.is_passed() — JSON パース成功
    match parse_mutants_json(&stdout) {
        Some((caught, total)) => (outcome, caught, total),
        None => (outcome, 0, 0),
    }
}

/// `cargo mutants --json` の出力から (caught, total_mutants) を取り出す。
fn parse_mutants_json(stdout: &str) -> Option<(usize, usize)> {
    #[derive(serde::Deserialize)]
    struct MutantsOutput {
        total_mutants: usize,
        caught: usize,
    }
    // stdout の最後の JSON 行を探す（進捗ログが混在する場合があるため）
    for line in stdout.lines().rev() {
        let line = line.trim();
        if line.starts_with('{') {
            if let Ok(m) = serde_json::from_str::<MutantsOutput>(line) {
                return Some((m.caught, m.total_mutants));
            }
        }
    }
    None
}

/// Python 候補: py_compile（構文チェック）→ pytest → ruff。
/// prop_test / wasm は非対応のため Skipped。
fn run_python_stages(
    root: &Path,
    candidate: &Candidate,
    timeout: Duration,
    tests_dir: Option<&Path>,
    run_mutation: bool,
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
    // --override-ini=python_files=*.py で candidate.py を収集対象に含める。
    // デフォルト discovery は test_*.py のみのため、明示オーバーライドが必要。
    let mut t = Command::new("sh");
    t.arg("-c")
        .arg(r#"python3 -m pytest -q --override-ini="python_files=*.py" . ; c=$?; [ $c -eq 0 ] || [ $c -eq 5 ]"#)
        .current_dir(root);
    let test = runner::run_stage("test", t, timeout);

    // lint: ruff（指摘があっても stage は成功扱い、件数のみ採点に使う）
    let mut l = Command::new("sh");
    l.arg("-c")
        .arg("ruff check --output-format=concise candidate.py 1>&2; true")
        .current_dir(root);
    let (lint, lint_out) = runner::run_stage_capture("ruff", l, timeout);
    let lint_warnings = count_ruff_findings(&lint_out);

    let (mutation, mutation_caught, mutation_total) = if run_mutation && test.is_passed() {
        run_python_mutation(root, timeout)
    } else {
        (StageOutcome::Skipped, 0, 0)
    };

    Ok(StageResults {
        compile,
        test,
        lint,
        lint_warnings,
        prop_test: StageOutcome::Skipped,
        wasm: StageOutcome::Skipped,
        wasm_fuel_used: None,
        mutation,
        mutation_caught,
        mutation_total,
        audit_findings: 0,
        bench_ns: None,
    })
}

/// Python 候補に mutmut でミューテーションテストを実行する。
fn run_python_mutation(root: &Path, timeout: Duration) -> (StageOutcome, usize, usize) {
    let mutmut_available = std::process::Command::new("mutmut")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !mutmut_available {
        return (StageOutcome::Skipped, 0, 0);
    }

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg("mutmut run --tests-dir . --output mutmut.json --json-output 2>&1")
        .current_dir(root);

    let mutation_timeout = timeout * 3;
    let (outcome, stdout) = runner::run_stage_capture("mutation", cmd, mutation_timeout);

    match parse_mutmut_json(&stdout) {
        Some((caught, total)) => (outcome, caught, total),
        None => (outcome, 0, 0),
    }
}

/// mutmut JSON 出力から (caught, total_mutants) を取り出す。
fn parse_mutmut_json(stdout: &str) -> Option<(usize, usize)> {
    #[derive(serde::Deserialize)]
    struct MutmutOutput {
        total_mutants: usize,
        caught: usize,
    }
    for line in stdout.lines().rev() {
        let line = line.trim();
        if line.starts_with('{') {
            if let Ok(m) = serde_json::from_str::<MutmutOutput>(line) {
                return Some((m.caught, m.total_mutants));
            }
        }
    }
    None
}

/// `staticcheck` が PATH に存在するか確認する。
fn staticcheck_available() -> bool {
    std::process::Command::new("staticcheck")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `eslint` が PATH に存在するか確認する。
fn eslint_available() -> bool {
    std::process::Command::new("eslint")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Go 候補: 一時モジュールで go build → go test → staticcheck（なければ go vet）。
/// prop_test / wasm は非対応のため Skipped。
fn run_go_stages(
    root: &Path,
    candidate: &Candidate,
    timeout: Duration,
    tests_dir: Option<&Path>,
    run_mutation: bool,
) -> Result<StageResults> {
    // go.mod を生成（依存なしの最小モジュール）
    let go_mod = "module candidate\n\ngo 1.22\n";
    fs::write(root.join("go.mod"), go_mod).context("go.mod の書込に失敗")?;
    fs::write(root.join("candidate.go"), &candidate.source).context("候補ソースの書込に失敗")?;
    if let Some(dir) = tests_dir {
        copy_ext(dir, root, "go")?;
    }

    // compile: go build
    let mut c = Command::new("go");
    c.args(["build", "./..."]).current_dir(root);
    let compile = runner::run_stage("compile", c, timeout);
    if !compile.is_passed() {
        return Ok(StageResults::skipped_after_compile(compile));
    }

    // test: go test（テストファイルなし → exit 0 with "[no test files]", 成功扱い）
    let mut t = Command::new("go");
    t.args(["test", "./..."]).current_dir(root);
    let test = runner::run_stage("test", t, timeout);

    // lint: staticcheck を優先、なければ go vet にフォールバック
    let (lint, lint_warnings) = if staticcheck_available() {
        let mut l = Command::new("sh");
        l.arg("-c")
            .arg("staticcheck ./... 2>&1; true")
            .current_dir(root);
        let (outcome, out) = runner::run_stage_capture("staticcheck", l, timeout);
        (outcome, count_staticcheck_findings(&out))
    } else {
        let mut l = Command::new("sh");
        l.arg("-c")
            .arg("go vet ./... 1>&2; true")
            .current_dir(root);
        let (outcome, out) = runner::run_stage_capture("go-vet", l, timeout);
        (outcome, count_go_vet_findings(&out))
    };

    let (mutation, mutation_caught, mutation_total) = if run_mutation && test.is_passed() {
        run_go_mutation(root, timeout)
    } else {
        (StageOutcome::Skipped, 0, 0)
    };

    Ok(StageResults {
        compile,
        test,
        lint,
        lint_warnings,
        prop_test: StageOutcome::Skipped,
        wasm: StageOutcome::Skipped,
        wasm_fuel_used: None,
        mutation,
        mutation_caught,
        mutation_total,
        audit_findings: 0,
        bench_ns: None,
    })
}

/// `gremlins` で Go 候補のミューテーションテストを実行する。
///
/// - `gremlins` が PATH にない場合は `(Skipped, 0, 0)` を返す（減点なし）。
fn run_go_mutation(root: &Path, timeout: Duration) -> (StageOutcome, usize, usize) {
    let gremlins_available = std::process::Command::new("gremlins")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !gremlins_available {
        return (StageOutcome::Skipped, 0, 0);
    }

    let json_out = "gremlins.json";
    let mutation_timeout = timeout * 3;

    let mut cmd = Command::new("gremlins");
    cmd.args(["unleash", "--output", json_out, "./..."])
        .current_dir(root);
    let (outcome, _) = runner::run_stage_capture("mutation", cmd, mutation_timeout);

    if matches!(outcome, StageOutcome::TimedOut { .. }) {
        return (outcome, 0, 0);
    }

    match fs::read_to_string(root.join(json_out))
        .ok()
        .and_then(|s| parse_gremlins_json(&s))
    {
        Some((caught, total)) => {
            let duration_ms = outcome
                .duration_ms()
                .unwrap_or(mutation_timeout.as_millis() as u64);
            (StageOutcome::Passed { duration_ms }, caught, total)
        }
        None => (StageOutcome::Skipped, 0, 0),
    }
}

/// `gremlins unleash --output` の JSON から (caught, total) を取り出す。
///
/// gremlins JSON フォーマット: `{"mutants_killed": N, "mutants_total": N, ...}`
fn parse_gremlins_json(content: &str) -> Option<(usize, usize)> {
    #[derive(serde::Deserialize)]
    struct GremlinsOutput {
        mutants_killed: usize,
        mutants_total: usize,
    }
    serde_json::from_str::<GremlinsOutput>(content)
        .ok()
        .map(|g| (g.mutants_killed, g.mutants_total))
}

/// JavaScript 候補: node --check（構文検証）→ node --test → ESLint（あれば）。
/// prop_test / wasm は非対応のため Skipped。
fn run_js_stages(
    root: &Path,
    candidate: &Candidate,
    timeout: Duration,
    tests_dir: Option<&Path>,
) -> Result<StageResults> {
    fs::write(root.join("candidate.js"), &candidate.source).context("候補ソースの書込に失敗")?;
    if let Some(dir) = tests_dir {
        copy_ext(dir, root, "js")?;
    }

    // compile: 構文チェック
    let mut c = Command::new("node");
    c.args(["--check", "candidate.js"]).current_dir(root);
    let compile = runner::run_stage("compile", c, timeout);
    if !compile.is_passed() {
        return Ok(StageResults::skipped_after_compile(compile));
    }

    // test: Node 組込みランナー（18+）で candidate.js + *.test.js を実行。
    let mut t = Command::new("sh");
    t.arg("-c")
        .arg(r#"node --test candidate.js $(ls *.test.js 2>/dev/null) 1>&2"#)
        .current_dir(root);
    let test = runner::run_stage("test", t, timeout);

    // lint: ESLint があれば --no-eslintrc + 最小ルールで実行
    let (lint, lint_warnings) = if eslint_available() {
        let mut l = Command::new("sh");
        l.arg("-c")
            .arg(r#"eslint --no-eslintrc --rule 'no-undef: error' --rule 'no-unused-vars: warn' --format compact candidate.js 2>&1; true"#)
            .current_dir(root);
        let (outcome, out) = runner::run_stage_capture("eslint", l, timeout);
        (outcome, count_eslint_findings(&out))
    } else {
        (StageOutcome::Skipped, 0)
    };

    Ok(StageResults {
        compile,
        test,
        lint,
        lint_warnings,
        prop_test: StageOutcome::Skipped,
        wasm: StageOutcome::Skipped,
        wasm_fuel_used: None,
        mutation: StageOutcome::Skipped,
        mutation_caught: 0,
        mutation_total: 0,
        audit_findings: 0,
        bench_ns: None,
    })
}

/// `cargo audit --json` で Rust 候補の脆弱性をスキャンし、検出件数を返す。
///
/// - `cargo audit` が PATH にない場合や `Cargo.lock` が存在しない場合は 0 を返す（減点なし）。
fn run_cargo_audit(root: &Path, timeout: Duration) -> usize {
    let audit_available = std::process::Command::new("cargo")
        .args(["audit", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !audit_available {
        return 0;
    }
    if !root.join("Cargo.lock").exists() {
        return 0;
    }

    let mut cmd = Command::new("cargo");
    cmd.args(["audit", "--json"]).current_dir(root);
    let (_outcome, stdout) = runner::run_stage_capture("audit", cmd, timeout);
    parse_audit_json(&stdout).unwrap_or(0)
}

/// `cargo audit --json` の出力から脆弱性件数を取り出す。
fn parse_audit_json(stdout: &str) -> Option<usize> {
    #[derive(serde::Deserialize)]
    struct Vulnerabilities {
        count: usize,
    }
    #[derive(serde::Deserialize)]
    struct AuditOutput {
        vulnerabilities: Vulnerabilities,
    }
    for line in stdout.lines() {
        let line = line.trim();
        if line.starts_with('{') {
            if let Ok(a) = serde_json::from_str::<AuditOutput>(line) {
                return Some(a.vulnerabilities.count);
            }
        }
    }
    None
}

/// Criterion ベンチマークを実行し、全ベンチの平均実行時間 (ns) を返す。
///
/// - Criterion が設定されていない（ベンチファイルなし）場合は `None`。
/// - 実行は `cargo bench -- --output-format criterion` だが、標準 text 出力を解析する。
fn run_criterion_bench(root: &Path, timeout: Duration) -> Option<u64> {
    let bench_timeout = timeout * 2;
    let mut cmd = Command::new("cargo");
    cmd.args(["bench"]).current_dir(root);
    let (_outcome, output) = runner::run_stage_capture("bench", cmd, bench_timeout);
    parse_criterion_output(&output)
}

/// Criterion の text 出力から全ベンチの平均実行時間 (ns) を取り出す。
///
/// Criterion は以下の形式で出力する:
/// `<bench_name>  time:   [<low> <mean> <high>]`
/// 単位は ns / µs / ms / s。複数ベンチがある場合は算術平均を返す。
pub fn parse_criterion_output(output: &str) -> Option<u64> {
    let mut total_ns: f64 = 0.0;
    let mut count: usize = 0;

    for line in output.lines() {
        if let Some(rest) = line.split("time:").nth(1) {
            if let Some(ns) = parse_time_bracket(rest) {
                total_ns += ns;
                count += 1;
            }
        }
    }

    if count > 0 {
        Some((total_ns / count as f64) as u64)
    } else {
        None
    }
}

/// `[<low> <mean> <high>]` から mean (ns) を取り出す。
fn parse_time_bracket(s: &str) -> Option<f64> {
    let inner = s.trim().trim_start_matches('[').split(']').next()?.trim();
    let parts: Vec<&str> = inner.split_whitespace().collect();
    // parts = ["1.2345", "µs", "1.2350", "µs", "1.2360", "µs"]  (6 tokens)
    // mean = parts[2], unit = parts[3]
    if parts.len() >= 4 {
        let value: f64 = parts[2].parse().ok()?;
        let unit = parts[3];
        let ns = match unit {
            "ns" => value,
            "µs" | "us" => value * 1_000.0,
            "ms" => value * 1_000_000.0,
            "s" => value * 1_000_000_000.0,
            _ => return None,
        };
        Some(ns)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_criterion_ns_unit() {
        let output = "fib/10   time:   [123.00 ns 124.00 ns 125.00 ns]";
        assert_eq!(parse_criterion_output(output), Some(124));
    }

    #[test]
    fn parse_criterion_us_unit() {
        let output = "sort/1000  time:   [1.2000 µs 1.2500 µs 1.3000 µs]";
        let ns = parse_criterion_output(output).unwrap();
        assert!((ns as f64 - 1250.0).abs() < 1.0);
    }

    #[test]
    fn parse_criterion_multiple_benches_averages() {
        let output = "a  time:   [1000.0 ns 1000.0 ns 1001.0 ns]\nb  time:   [3000.0 ns 3000.0 ns 3001.0 ns]";
        assert_eq!(parse_criterion_output(output), Some(2000));
    }

    #[test]
    fn parse_criterion_no_match_returns_none() {
        assert_eq!(parse_criterion_output("no benchmarks here"), None);
    }

    #[test]
    fn has_criterion_bench_detects_use() {
        assert!(has_criterion_bench("use criterion::Criterion;"));
        assert!(has_criterion_bench("criterion_group!(benches, my_bench);"));
        assert!(!has_criterion_bench("fn main() {}"));
    }
}
