//! 検証パイプライン駆動。候補を一時 Cargo プロジェクトへ展開し、
//! compile → test → 採点 までを実行する。

use crate::analysis::{
    count_eslint_findings, count_go_vet_findings, count_lint_warnings, count_ruff_findings,
    count_staticcheck_findings, has_bench, SourceMetrics,
};
use crate::config::Rubric;
use crate::model::{Candidate, Evaluation, Language, StageOutcome};
use crate::{runner, scoring, wasm};
use anyhow::{Context, Result};
use std::collections::HashMap;
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

/// Criterion ベンチを持つ Cargo.toml（`benches/bench.rs` を参照）。
const CARGO_TOML_WITH_CRITERION: &str = r#"[package]
name = "candidate"
version = "0.0.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }

[[bench]]
name = "bench"
harness = false
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
/// 拡張子なしは Rust、未対応の拡張子はエラー。
pub fn load_candidate(path: &Path) -> Result<Candidate> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("候補ファイルの読込に失敗: {}", path.display()))?;
    let id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("candidate")
        .to_string();
    let language = match path.extension().and_then(|e| e.to_str()) {
        None => Language::Rust,
        Some(ext) => Language::try_from_extension(ext).ok_or_else(|| {
            anyhow::anyhow!(
                "未対応の拡張子 `.{ext}`: {}（対応: {}）",
                path.display(),
                Language::SUPPORTED_EXTENSIONS
                    .iter()
                    .map(|e| format!(".{e}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?,
    };
    Ok(Candidate {
        id,
        source,
        language,
    })
}

/// 複数の候補ファイルを読み込む。候補 ID（ファイル名 stem）が重複したらエラー。
pub fn load_candidates<P: AsRef<Path>>(paths: &[P]) -> Result<Vec<Candidate>> {
    let mut seen: HashMap<String, &Path> = HashMap::new();
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let p = p.as_ref();
        let c = load_candidate(p)?;
        if let Some(prev) = seen.insert(c.id.clone(), p) {
            anyhow::bail!(
                "候補 ID `{}` が重複: {} と {}（ファイル名 stem を変えてください）",
                c.id,
                prev.display(),
                p.display()
            );
        }
        out.push(c);
    }
    Ok(out)
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
    let wo = wasm::run_wasm(&artifact, opts.fuel, opts.max_memory_bytes, timeout);
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
    reasoning_score: Option<f64>,
    reasoning_comment: Option<String>,
}

impl Default for StageResults {
    fn default() -> Self {
        StageResults {
            compile: StageOutcome::Skipped,
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
            reasoning_score: None,
            reasoning_comment: None,
        }
    }
}

impl StageResults {
    /// compile 失敗時の既定（後続ステージはすべて Skipped）。
    fn skipped_after_compile(compile: StageOutcome) -> Self {
        StageResults {
            compile,
            ..Default::default()
        }
    }
}

/// `evaluate_candidate` に渡すオプション群。
#[derive(Default)]
pub struct EvalOptions<'a> {
    pub tests_dir: Option<&'a Path>,
    pub prop_tests_dir: Option<&'a Path>,
    pub wasm_opts: WasmOptions<'a>,
    pub run_mutation: bool,
    pub run_llm_review: bool,
}

/// 1 候補を一時ディレクトリへ展開し、言語に応じた検証ステージを実行して採点する。
///
/// compile が通らなかった場合、後続ステージはすべて `Skipped`。
pub fn evaluate_candidate(
    candidate: &Candidate,
    timeout: Duration,
    rubric: &Rubric,
    opts: &EvalOptions,
) -> Result<Evaluation> {
    let dir = tempfile::tempdir().context("一時ディレクトリの生成に失敗")?;
    let root = dir.path();

    let results = match candidate.language {
        Language::Rust => run_rust_stages(
            root,
            candidate,
            timeout,
            opts.tests_dir,
            opts.prop_tests_dir,
            &opts.wasm_opts,
            opts.run_mutation,
        )?,
        Language::Python => {
            run_python_stages(root, candidate, timeout, opts.tests_dir, opts.run_mutation)?
        }
        Language::Go => run_go_stages(root, candidate, timeout, opts.tests_dir, opts.run_mutation)?,
        Language::JavaScript => run_js_stages(root, candidate, timeout, opts.tests_dir)?,
        Language::TypeScript => run_ts_stages(root, candidate, timeout, opts.tests_dir)?,
    };

    // LLM セマンティックレビュー（オプション）
    let (reasoning_score, reasoning_comment) = if opts.run_llm_review {
        let test_passed = results.test.is_passed();
        match crate::llm::review(&candidate.source, test_passed) {
            Some(r) => (Some(r.score), Some(r.comment)),
            None => (None, None),
        }
    } else {
        (None, None)
    };

    let results = StageResults {
        reasoning_score,
        reasoning_comment,
        ..results
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
        reasoning: if r.reasoning_score.is_some() {
            StageOutcome::Passed { duration_ms: 0 }
        } else {
            StageOutcome::Skipped
        },
        reasoning_score: r.reasoning_score.unwrap_or(0.0),
        reasoning_comment: r.reasoning_comment,
        axes,
        score,
    }
}

/// Rust 候補: 一時 Cargo プロジェクトで compile → test → lint(clippy) → prop_test → wasm → mutation。
fn run_rust_stages(
    root: &Path,
    candidate: &Candidate,
    timeout: Duration,
    tests_dir: Option<&Path>,
    prop_tests_dir: Option<&Path>,
    wasm_opts: &WasmOptions,
    run_mutation: bool,
) -> Result<StageResults> {
    fs::create_dir_all(root.join("src")).context("src ディレクトリの作成に失敗")?;
    let is_bench = has_bench(&candidate.source);
    let toml = if is_bench {
        CARGO_TOML_WITH_CRITERION
    } else if prop_tests_dir.is_some() {
        CARGO_TOML_WITH_PROPTEST
    } else {
        CARGO_TOML_TEMPLATE
    };
    fs::write(root.join("Cargo.toml"), toml).context("Cargo.toml の書込に失敗")?;
    if is_bench {
        // Criterion ベンチ候補: benches/bench.rs に配置し、src/lib.rs は空にする。
        fs::create_dir_all(root.join("benches")).context("benches ディレクトリの作成に失敗")?;
        fs::write(root.join("benches").join("bench.rs"), &candidate.source)
            .context("ベンチソースの書込に失敗")?;
        fs::write(root.join("src").join("lib.rs"), "").context("空 lib.rs の書込に失敗")?;
    } else {
        fs::write(root.join("src").join("lib.rs"), &candidate.source)
            .context("候補ソースの書込に失敗")?;
    }
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
    // CARGO_TERM_COLOR=always(CI 既定)だと警告行に ANSI が付き件数を数えられないため色を切る。
    c.args(["clippy", "--color", "never", "--", "-W", "clippy::all"])
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
    let bench_ns = if has_bench(&candidate.source) {
        run_bench_stage(root, timeout)
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
        ..Default::default()
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
        mutation,
        mutation_caught,
        mutation_total,
        ..Default::default()
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

/// ESLint の出力が「実行失敗」(設定なし・未知オプション・フォーマッタ欠落)を示すか判定する純関数。
///
/// `eslint ... 2>&1; true` は終了コードを捨てるため、失敗が「指摘 0 件」に化けるのを防ぐ (#152)。
fn eslint_run_failed(output: &str) -> bool {
    const MARKERS: [&str; 4] = [
        "Oops! Something went wrong",
        "Invalid option '--",
        "formatter is no longer part of core ESLint",
        "couldn't find an eslint.config",
    ];
    MARKERS.iter().any(|m| output.contains(m))
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
        l.arg("-c").arg("go vet ./... 1>&2; true").current_dir(root);
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
        mutation,
        mutation_caught,
        mutation_total,
        ..Default::default()
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
        if eslint_run_failed(&out) {
            // 実行失敗を「指摘 0 件」にしない (#152)
            (StageOutcome::Skipped, 0)
        } else {
            (outcome, count_eslint_findings(&out))
        }
    } else {
        (StageOutcome::Skipped, 0)
    };

    Ok(StageResults {
        compile,
        test,
        lint,
        lint_warnings,
        ..Default::default()
    })
}

/// `tsc` が PATH に存在するか確認する。
fn tsc_available() -> bool {
    std::process::Command::new("tsc")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// TypeScript 候補: tsc --noEmit（型検査）→ tsc + node --test（テスト）→ ESLint（あれば）。
/// prop_test / wasm / mutation は非対応のため Skipped。
/// `tsc` が PATH にない場合はコンパイル Skipped（減点なし）。
fn run_ts_stages(
    root: &Path,
    candidate: &Candidate,
    timeout: Duration,
    tests_dir: Option<&Path>,
) -> Result<StageResults> {
    // 最小 tsconfig（strict + CommonJS 出力）
    let tsconfig = r#"{"compilerOptions":{"target":"ES2022","module":"CommonJS","strict":true,"outDir":"dist"},"include":["*.ts"]}"#;
    fs::write(root.join("tsconfig.json"), tsconfig).context("tsconfig.json の書込に失敗")?;
    fs::write(root.join("candidate.ts"), &candidate.source).context("候補ソースの書込に失敗")?;
    if let Some(dir) = tests_dir {
        copy_ext(dir, root, "ts")?;
    }

    if !tsc_available() {
        return Ok(StageResults::default());
    }

    // compile: 型検査のみ（JS 出力なし）
    let mut c = Command::new("tsc");
    c.args(["--noEmit"]).current_dir(root);
    let compile = runner::run_stage("compile", c, timeout);
    if !compile.is_passed() {
        return Ok(StageResults::skipped_after_compile(compile));
    }

    // test: JS にトランスパイルしてから node --test でテストファイルのみ実行
    // candidate.js は渡さない（node --test はテストファイルとして実行するため副作用が起きる）
    let test_files_exist = tests_dir.is_some()
        && tests_dir
            .map(|d| {
                d.read_dir()
                    .ok()
                    .map(|mut e| e.next().is_some())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
    let test = if test_files_exist {
        let mut t = Command::new("sh");
        t.arg("-c")
            .arg(r#"tsc && node --test $(ls dist/*.test.js 2>/dev/null) 1>&2"#)
            .current_dir(root);
        runner::run_stage("test", t, timeout)
    } else {
        StageOutcome::Skipped
    };

    // lint: ESLint があれば TypeScript 向けルールで実行
    let (lint, lint_warnings) = if eslint_available() {
        let mut l = Command::new("sh");
        l.arg("-c")
            .arg(r#"eslint --no-eslintrc --rule 'no-undef: error' --rule 'no-unused-vars: warn' --format compact candidate.ts 2>&1; true"#)
            .current_dir(root);
        let (outcome, out) = runner::run_stage_capture("eslint", l, timeout);
        if eslint_run_failed(&out) {
            // 実行失敗を「指摘 0 件」にしない (#152)
            (StageOutcome::Skipped, 0)
        } else {
            (outcome, count_eslint_findings(&out))
        }
    } else {
        (StageOutcome::Skipped, 0)
    };

    Ok(StageResults {
        compile,
        test,
        lint,
        lint_warnings,
        ..Default::default()
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

/// Criterion ベンチを `benches/bench.rs` として実行し、中央値 (ns) を返す。
///
/// `benches/` ディレクトリが存在し Cargo.toml が criterion 対応の場合のみ実行。
/// Criterion の出力形式 `bench_name  time:   [lo mid hi]` の中央値を抽出する。
/// 複数ベンチが存在する場合は最小中央値（最速）を採用する。
fn run_bench_stage(root: &Path, timeout: Duration) -> Option<u64> {
    if !root.join("benches").exists() {
        return None;
    }
    let mut cmd = Command::new("cargo");
    cmd.arg("bench").current_dir(root);
    let (_outcome, output) = runner::run_stage_capture("bench", cmd, timeout);
    parse_criterion_ns(&output)
}

/// Criterion の出力から中央値 (ns) を抽出する。
///
/// 出力形式: `bench_name  time:   [lo_val lo_unit mid_val mid_unit hi_val hi_unit]`
/// 複数ベンチがある場合は最小中央値を返す。
fn parse_criterion_ns(output: &str) -> Option<u64> {
    let mut min_ns: Option<f64> = None;
    for line in output.lines() {
        let Some(after_time) = line.split("time:").nth(1) else {
            continue;
        };
        let inner = after_time.trim().trim_start_matches('[');
        let values: Vec<&str> = inner.split_whitespace().collect();
        if values.len() < 4 {
            continue;
        }
        let mid_val: f64 = match values[2].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let mid_unit = values[3].trim_end_matches(']');
        let Some(ns) = to_ns(mid_val, mid_unit) else {
            continue;
        };
        min_ns = Some(match min_ns {
            Some(prev) => prev.min(ns),
            None => ns,
        });
    }
    min_ns.map(|v| v.round() as u64)
}

fn to_ns(val: f64, unit: &str) -> Option<f64> {
    match unit {
        "ns" => Some(val),
        "µs" | "us" => Some(val * 1_000.0),
        "ms" => Some(val * 1_000_000.0),
        "s" => Some(val * 1_000_000_000.0),
        _ => None,
    }
}

#[cfg(test)]
mod bench_tests {
    use super::*;

    #[test]
    fn parse_criterion_ns_parses_ns_unit() {
        let output = "my_bench   time:   [123.00 ns 456.00 ns 789.00 ns]";
        assert_eq!(parse_criterion_ns(output), Some(456));
    }

    #[test]
    fn parse_criterion_ns_parses_us_unit() {
        let output = "my_bench   time:   [1.0 µs 2.5 µs 3.0 µs]";
        assert_eq!(parse_criterion_ns(output), Some(2500));
    }

    #[test]
    fn parse_criterion_ns_picks_min_across_benches() {
        let output = "fast_bench  time:   [10.0 ns 20.0 ns 30.0 ns]\nslow_bench  time:   [100.0 ns 200.0 ns 300.0 ns]";
        assert_eq!(parse_criterion_ns(output), Some(20));
    }

    #[test]
    fn parse_criterion_ns_returns_none_on_empty() {
        assert_eq!(parse_criterion_ns(""), None);
        assert_eq!(parse_criterion_ns("no bench output here"), None);
    }

    #[test]
    fn parse_criterion_ns_parses_ms_and_s_units() {
        assert_eq!(
            parse_criterion_ns("b  time:   [1.0 ms 1.5 ms 2.0 ms]"),
            Some(1_500_000)
        );
        assert_eq!(
            parse_criterion_ns("b  time:   [1.0 s 2.0 s 3.0 s]"),
            Some(2_000_000_000)
        );
    }

    #[test]
    fn parse_criterion_ns_accepts_us_ascii_unit() {
        assert_eq!(
            parse_criterion_ns("b  time:   [1.0 us 2.5 us 3.0 us]"),
            Some(2500)
        );
    }

    #[test]
    fn parse_criterion_ns_compares_min_across_units() {
        // 単位が違っても ns 換算で最小を採る（900 µs < 1 ms）
        let output = "a  time:   [1.0 ms 1.0 ms 1.0 ms]\nb  time:   [1.0 us 900.0 us 1.0 ms]";
        assert_eq!(parse_criterion_ns(output), Some(900_000));
    }

    #[test]
    fn parse_criterion_ns_unknown_unit_is_skipped() {
        assert_eq!(
            parse_criterion_ns("b  time:   [1.0 ps 2.0 ps 3.0 ps]"),
            None
        );
        // 未知単位の行は無視し、有効な行だけで最小を決める
        let output = "x  time:   [1.0 ps 2.0 ps 3.0 ps]\ny  time:   [1.0 ns 7.0 ns 9.0 ns]";
        assert_eq!(parse_criterion_ns(output), Some(7));
    }

    #[test]
    fn parse_criterion_ns_incomplete_line_is_skipped() {
        // values.len() < 4（中央値の単位がない）
        assert_eq!(parse_criterion_ns("b  time:   [1.0 ns 2.0"), None);
        assert_eq!(parse_criterion_ns("b  time:   "), None);
        let output = "x  time:   [1.0 ns 2.0]\ny  time:   [1.0 ns 7.0 ns 9.0 ns]";
        assert_eq!(parse_criterion_ns(output), Some(7));
    }

    #[test]
    fn parse_criterion_ns_non_numeric_median_is_skipped() {
        assert_eq!(
            parse_criterion_ns("b  time:   [1.0 ns abc ns 3.0 ns]"),
            None
        );
        let output = "x  time:   [1.0 ns abc ns 3.0 ns]\ny  time:   [1.0 ns 7.0 ns 9.0 ns]";
        assert_eq!(parse_criterion_ns(output), Some(7));
    }

    #[test]
    fn parse_criterion_ns_trims_closing_bracket_on_unit() {
        // 4 トークンのみで中央値の単位に ']' が付く形
        assert_eq!(
            parse_criterion_ns("b  time:   [1.0 ns 5.0 ms]"),
            Some(5_000_000)
        );
        assert_eq!(parse_criterion_ns("b  time:   [1.0 ns 5.0 ps]"), None);
    }

    #[test]
    fn parse_criterion_ns_rounds_to_nearest() {
        assert_eq!(
            parse_criterion_ns("b  time:   [1.0 ns 2.5 ns 3.0 ns]"),
            Some(3)
        );
        assert_eq!(
            parse_criterion_ns("b  time:   [1.0 ns 2.4 ns 3.0 ns]"),
            Some(2)
        );
    }

    #[test]
    fn has_bench_detects_criterion() {
        use crate::analysis::has_bench;
        let src = r#"use criterion::{criterion_group, criterion_main, Criterion};
fn bench(c: &mut Criterion) { c.bench_function("f", |b| b.iter(|| 1 + 1)); }
criterion_group!(benches, bench);
criterion_main!(benches);"#;
        assert!(has_bench(src));
        assert!(!has_bench("fn regular() {}"));
    }
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    #[test]
    fn parse_audit_json_typical_count() {
        let out = r#"{"database":{"advisory-count":1},"vulnerabilities":{"found":true,"count":2,"list":[]}}"#;
        assert_eq!(parse_audit_json(out), Some(2));
    }

    #[test]
    fn parse_audit_json_zero_vulnerabilities() {
        let out = r#"{"vulnerabilities":{"found":false,"count":0,"list":[]}}"#;
        assert_eq!(parse_audit_json(out), Some(0));
    }

    #[test]
    fn parse_audit_json_invalid_returns_none() {
        assert_eq!(parse_audit_json(""), None);
        assert_eq!(parse_audit_json("not json"), None);
        assert_eq!(parse_audit_json("{broken"), None);
        // vulnerabilities / count が欠けている
        assert_eq!(parse_audit_json(r#"{"foo":1}"#), None);
        assert_eq!(parse_audit_json(r#"{"vulnerabilities":{}}"#), None);
        // count が数値でない／負数
        assert_eq!(
            parse_audit_json(r#"{"vulnerabilities":{"count":"2"}}"#),
            None
        );
        assert_eq!(
            parse_audit_json(r#"{"vulnerabilities":{"count":-1}}"#),
            None
        );
    }

    #[test]
    fn parse_audit_json_skips_progress_lines() {
        let out = "    Fetching advisory database\n    Loaded 500 advisories\n{\"vulnerabilities\":{\"count\":3}}\n";
        assert_eq!(parse_audit_json(out), Some(3));
    }

    #[test]
    fn parse_audit_json_uses_first_valid_line() {
        let out = "{\"vulnerabilities\":{\"count\":1}}\n{\"vulnerabilities\":{\"count\":9}}";
        assert_eq!(parse_audit_json(out), Some(1));
    }

    #[test]
    fn parse_audit_json_skips_invalid_json_line_before_valid() {
        let out = "{not valid\n{\"other\":1}\n  {\"vulnerabilities\":{\"count\":4}}  ";
        assert_eq!(parse_audit_json(out), Some(4));
    }
}

#[cfg(test)]
mod mutation_tests {
    use super::*;

    #[test]
    fn parse_gremlins_json_typical() {
        let json = r#"{"mutants_killed":8,"mutants_total":10,"elapsed_ns":12345}"#;
        assert_eq!(parse_gremlins_json(json), Some((8, 10)));
    }

    #[test]
    fn parse_gremlins_json_all_killed() {
        let json = r#"{"mutants_killed":5,"mutants_total":5}"#;
        assert_eq!(parse_gremlins_json(json), Some((5, 5)));
    }

    #[test]
    fn parse_gremlins_json_zero_mutants() {
        let json = r#"{"mutants_killed":0,"mutants_total":0}"#;
        assert_eq!(parse_gremlins_json(json), Some((0, 0)));
    }

    #[test]
    fn parse_gremlins_json_invalid_returns_none() {
        assert_eq!(parse_gremlins_json(""), None);
        assert_eq!(parse_gremlins_json("not json"), None);
        assert_eq!(parse_gremlins_json(r#"{"other":"field"}"#), None);
    }

    #[test]
    fn parse_mutmut_json_typical() {
        let output = "some log\n{\"total_mutants\":6,\"caught\":4}\n";
        assert_eq!(parse_mutmut_json(output), Some((4, 6)));
    }

    #[test]
    fn parse_mutmut_json_picks_last_json_line() {
        // mutmut が途中に別の JSON を出しても末尾（最新）を使う。
        let output = "{\"total_mutants\":3,\"caught\":1}\n{\"total_mutants\":6,\"caught\":4}\n";
        assert_eq!(parse_mutmut_json(output), Some((4, 6)));
    }

    #[test]
    fn parse_mutmut_json_no_json_returns_none() {
        assert_eq!(parse_mutmut_json(""), None);
        assert_eq!(parse_mutmut_json("just plain text\n"), None);
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn unknown_extension_is_error_listing_supported() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "foo.java", "class A {}");
        let err = load_candidate(&p).unwrap_err().to_string();
        assert!(err.contains(".java"), "{err}");
        for ext in ["rs", "py", "go", "js", "ts"] {
            assert!(err.contains(ext), "{err}");
        }
        assert!(load_candidate(&write(d.path(), "foo.txt", "x")).is_err());
    }

    #[test]
    fn no_extension_defaults_to_rust() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "foo", "pub fn f() {}");
        assert_eq!(load_candidate(&p).unwrap().language, Language::Rust);
    }

    #[test]
    fn known_extensions_unchanged() {
        let d = tempfile::tempdir().unwrap();
        for (f, l) in [
            ("a.rs", Language::Rust),
            ("a.py", Language::Python),
            ("a.go", Language::Go),
            ("a.js", Language::JavaScript),
            ("A.TS", Language::TypeScript),
        ] {
            let c = load_candidate(&write(d.path(), f, "x")).unwrap();
            assert_eq!(c.language, l);
        }
    }

    #[test]
    fn duplicate_candidate_id_is_error_with_paths() {
        let d = tempfile::tempdir().unwrap();
        let a = write(d.path(), "a/foo.rs", "pub fn f() {}");
        let b = write(d.path(), "b/foo.rs", "pub fn g() {}");
        let err = load_candidates(&[a.clone(), b.clone()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("foo"), "{err}");
        assert!(err.contains(&a.display().to_string()), "{err}");
        assert!(err.contains(&b.display().to_string()), "{err}");
    }

    #[test]
    fn distinct_ids_load_in_order() {
        let d = tempfile::tempdir().unwrap();
        let a = write(d.path(), "a.rs", "pub fn f() {}");
        let b = write(d.path(), "b.py", "x = 1");
        let cs = load_candidates(&[a, b]).unwrap();
        assert_eq!(
            cs.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }
}

#[cfg(test)]
mod entry_tests {
    use super::*;

    #[test]
    fn is_valid_entry_table() {
        let cases: &[(&str, bool)] = &[
            // 正常系
            ("run", true),
            ("_x", true),
            ("f1", true),
            ("_", true),
            // 拒否系（コード注入・不正識別子）
            ("", false),
            ("1a", false),
            ("a b", false),
            ("a;b", false),
            ("a::b", false),
            ("a()", false),
            ("a();evil", false),
            ("a\nb", false),
            ("a\n", false),
            ("a-b", false),
            // 非 ASCII: 現状仕様を固定（is_alphabetic/is_alphanumeric のため許可される）
            ("日本語", true),
            ("関数1", true),
            ("1関数", false),
        ];
        for (name, expected) in cases {
            assert_eq!(is_valid_entry(name), *expected, "name={name:?}");
        }
    }

    #[test]
    fn run_wasm_stage_rejects_invalid_entry_without_side_effects() {
        let d = tempfile::tempdir().unwrap();
        let opts = WasmOptions::default();
        for bad in ["", "a();evil", "a::b", "a\nb"] {
            let (outcome, fuel) = run_wasm_stage(d.path(), bad, Duration::from_secs(1), &opts);
            match outcome {
                StageOutcome::Failed { detail } => {
                    assert!(detail.contains("不正なエントリ名"), "{detail}")
                }
                other => panic!("Failed を期待: {other:?}"),
            }
            assert_eq!(fuel, None);
        }
        // ハーネス書込にも cargo 実行にも到達していない（src/main.rs 未生成）
        assert!(!d.path().join("src").exists());
    }
}

#[cfg(test)]
mod copy_assemble_tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        p
    }

    fn cand(source: &str) -> Candidate {
        Candidate {
            id: "c1".into(),
            source: source.into(),
            language: Language::Rust,
        }
    }

    fn passed() -> StageOutcome {
        StageOutcome::Passed { duration_ms: 7 }
    }

    // ---- load_candidate / load_candidates ----

    #[test]
    fn load_candidate_missing_file_is_error_with_path() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nope.rs");
        let err = format!("{:#}", load_candidate(&p).unwrap_err());
        assert!(err.contains("候補ファイルの読込に失敗"), "{err}");
        assert!(err.contains(&p.display().to_string()), "{err}");
    }

    #[test]
    fn load_candidate_directory_is_error() {
        let d = tempfile::tempdir().unwrap();
        assert!(load_candidate(d.path()).is_err());
    }

    #[test]
    fn load_candidate_id_is_stem_and_source_is_preserved() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "x.test.py", "print(1)\n");
        let c = load_candidate(&p).unwrap();
        assert_eq!(c.id, "x.test");
        assert_eq!(c.source, "print(1)\n");
        assert_eq!(c.language, Language::Python);
    }

    #[test]
    fn load_candidate_extension_is_case_insensitive() {
        let d = tempfile::tempdir().unwrap();
        for (f, l) in [
            ("a.RS", Language::Rust),
            ("a.Py", Language::Python),
            ("a.GO", Language::Go),
            ("a.JS", Language::JavaScript),
        ] {
            assert_eq!(
                load_candidate(&write(d.path(), f, "x")).unwrap().language,
                l
            );
        }
    }

    #[test]
    fn load_candidates_empty_is_ok_and_missing_path_propagates() {
        let none: [&Path; 0] = [];
        assert!(load_candidates(&none).unwrap().is_empty());
        let d = tempfile::tempdir().unwrap();
        let ok = write(d.path(), "a.rs", "x");
        let missing = d.path().join("missing.rs");
        assert!(load_candidates(&[ok, missing]).is_err());
    }

    // ---- copy_ext ----

    #[test]
    fn copy_ext_copies_only_matching_extension() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        write(&src, "a.rs", "A");
        write(&src, "b.rs", "B");
        write(&src, "c.txt", "C");
        write(&src, "noext", "N");
        write(&src, "D.RS", "D");
        let dest = d.path().join("dest");
        copy_ext(&src, &dest, "rs").unwrap();
        let mut names: Vec<_> = fs::read_dir(&dest)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        // 拡張子比較は大文字小文字を区別する（D.RS はコピーされない）
        assert_eq!(names, ["a.rs", "b.rs"]);
        assert_eq!(fs::read_to_string(dest.join("a.rs")).unwrap(), "A");
        assert_eq!(fs::read_to_string(dest.join("b.rs")).unwrap(), "B");
    }

    #[test]
    fn copy_ext_creates_nested_dest_and_handles_no_match() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        write(&src, "c.txt", "C");
        let dest = d.path().join("x/y/z");
        copy_ext(&src, &dest, "rs").unwrap();
        assert!(dest.is_dir());
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    }

    #[test]
    fn copy_ext_does_not_recurse_and_overwrites_existing() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        write(&src, "sub/inner.rs", "I");
        write(&src, "a.rs", "new");
        let dest = d.path().join("dest");
        write(&dest, "a.rs", "old");
        copy_ext(&src, &dest, "rs").unwrap();
        assert!(!dest.join("inner.rs").exists());
        assert!(!dest.join("sub").exists());
        assert_eq!(fs::read_to_string(dest.join("a.rs")).unwrap(), "new");
    }

    #[test]
    fn copy_ext_missing_src_is_error_but_dest_already_created() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("absent");
        let dest = d.path().join("dest");
        let err = format!("{:#}", copy_ext(&src, &dest, "rs").unwrap_err());
        assert!(err.contains("テストディレクトリの読込に失敗"), "{err}");
        assert!(err.contains(&src.display().to_string()), "{err}");
        // 現状仕様: create_dir_all が read_dir より先に走る
        assert!(dest.is_dir());
    }

    #[test]
    fn copy_ext_directory_named_with_ext_is_copy_error() {
        // 現状仕様を固定: 拡張子一致のディレクトリは is_file 判定がなく fs::copy が失敗する
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        fs::create_dir_all(src.join("weird.rs")).unwrap();
        let err = format!(
            "{:#}",
            copy_ext(&src, &d.path().join("dest"), "rs").unwrap_err()
        );
        assert!(err.contains("テストのコピーに失敗"), "{err}");
    }

    // ---- assemble ----

    #[test]
    fn assemble_without_reasoning_is_skipped_and_score_equals_axes_total() {
        let c = cand("pub fn f() {}");
        let r = StageResults {
            compile: passed(),
            test: passed(),
            ..Default::default()
        };
        let e = assemble(&c, r, &Rubric::default());
        assert_eq!(e.candidate_id, "c1");
        assert!(matches!(e.reasoning, StageOutcome::Skipped));
        assert_eq!(e.reasoning_score, 0.0);
        assert_eq!(e.reasoning_comment, None);
        assert_eq!(e.score, e.axes.total());
        // compile+test のみ: correctness = 50 * 0.8
        assert!((e.axes.correctness - 40.0).abs() < 1e-9, "{:?}", e.axes);
        assert_eq!(e.axes.performance, 0.0);
        assert_eq!(e.axes.resource_usage, 0.0);
    }

    #[test]
    fn assemble_with_reasoning_score_marks_passed_but_axis_is_not_assigned_here() {
        let c = cand("pub fn f() {}");
        let rubric = Rubric {
            reasoning: 10.0,
            ..Rubric::default()
        };
        let r = StageResults {
            compile: passed(),
            test: passed(),
            reasoning_score: Some(0.8),
            reasoning_comment: Some("ok".into()),
            ..Default::default()
        };
        let e = assemble(&c, r, &rubric);
        assert!(matches!(
            e.reasoning,
            StageOutcome::Passed { duration_ms: 0 }
        ));
        assert_eq!(e.reasoning_score, 0.8);
        assert_eq!(e.reasoning_comment.as_deref(), Some("ok"));
        // 現状仕様: axes.reasoning は assign_reasoning() で後から付与されるため 0 のまま
        assert_eq!(e.axes.reasoning, 0.0);
        assert_eq!(e.score, e.axes.total());
    }

    #[test]
    fn assemble_some_zero_reasoning_is_passed_not_skipped() {
        let r = StageResults {
            reasoning_score: Some(0.0),
            ..Default::default()
        };
        let e = assemble(&cand("x"), r, &Rubric::default());
        assert!(matches!(
            e.reasoning,
            StageOutcome::Passed { duration_ms: 0 }
        ));
        assert_eq!(e.reasoning_score, 0.0);
    }

    #[test]
    fn assemble_compile_failure_zeroes_axes_and_passes_fields_through() {
        let r = StageResults {
            compile: StageOutcome::Failed {
                detail: "boom".into(),
            },
            lint_warnings: 3,
            mutation_caught: 2,
            mutation_total: 5,
            audit_findings: 1,
            bench_ns: Some(42),
            wasm_fuel_used: Some(9),
            ..Default::default()
        };
        let e = assemble(&cand("pub fn f() {}"), r, &Rubric::default());
        assert!(matches!(e.compile, StageOutcome::Failed { .. }));
        assert_eq!(e.axes.security, 0.0);
        assert_eq!(e.axes.maintainability, 0.0);
        assert_eq!(e.score, e.axes.total());
        assert_eq!(
            (e.lint_warnings, e.mutation_caught, e.mutation_total),
            (3, 2, 5)
        );
        assert_eq!(
            (e.audit_findings, e.bench_ns, e.wasm_fuel_used),
            (1, Some(42), Some(9))
        );
    }
}

#[cfg(test)]
mod eslint_run_failed_tests {
    use super::eslint_run_failed;

    #[test]
    fn detects_invalid_option() {
        assert!(eslint_run_failed(
            "Invalid option '--eslintrc' - perhaps you meant '--ext'?"
        ));
    }

    #[test]
    fn detects_missing_config_oops() {
        assert!(eslint_run_failed(
            "\nOops! Something went wrong! :(\n\nESLint: 10.2.0\n\nESLint couldn't find an eslint.config.(js|mjs|cjs) file."
        ));
    }

    #[test]
    fn detects_missing_compact_formatter() {
        assert!(eslint_run_failed(
            "The compact formatter is no longer part of core ESLint. Install it manually with `npm install -D eslint-formatter-compact`"
        ));
    }

    #[test]
    fn normal_outputs_are_not_failures() {
        assert!(!eslint_run_failed(""));
        assert!(!eslint_run_failed("candidate.js: line 1, col 7, Warning - 'a' is assigned a value but never used. (no-unused-vars)\n\n1 problem"));
        assert!(!eslint_run_failed(
            "\u{2716} 2 problems (1 error, 1 warning)"
        ));
    }
}
