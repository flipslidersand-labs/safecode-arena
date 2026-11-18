//! Rust 候補のステージ (compile / test / clippy / Wasm / mutation / audit / bench)。

use super::*;

/// Wasm ステージのターゲット。
const WASM_TARGET: &str = "wasm32-wasip1";

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
proptest = "1"

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

/// rustc が既定で warn する lint のうち、`--force-warn` で強制するもの (#237)。
///
/// `#![allow(warnings)]` は `warnings` グループ全体を消すが、`--force-warn warnings` は
/// E0602 で使えないため個別に列挙する。`unused` グループは既定 allow の
/// `unused_macro_rules` / `unused_extern_crates` を含み通常候補の件数まで変えるので使わない。
/// 除外したもの: 標準の候補コードで出ない/評価対象外の lint (target 固有の asm・gpu・
/// aarch64、internal/incomplete features、diagnostic attribute 系、unknown-lints 等の
/// メタ lint、将来互換の ambiguous-*/uncovered-* 系)。
const RUSTC_FORCE_WARN_LINTS: &[&str] = &[
    "nonstandard_style",
    "deprecated",
    "dead_code",
    "unused_imports",
    "unused_variables",
    "unused_assignments",
    "unused_mut",
    "unused_must_use",
    "unused_parens",
    "unused_braces",
    "unused_comparisons",
    "unused_allocation",
    "unused_doc_comments",
    "unused_labels",
    "unused_unsafe",
    "unused_attributes",
    "unused_macros",
    "unused_visibilities",
    "unreachable_code",
    "unreachable_patterns",
    "unconditional_recursion",
    "path_statements",
    "redundant_semicolons",
    "map_unit_fn",
    "irrefutable_let_patterns",
    "overlapping_range_endpoints",
    "non_contiguous_range_endpoints",
    "bare_trait_objects",
    "array_into_iter",
    "for_loops_over_fallibles",
    "noop_method_call",
    "non_fmt_panics",
    "non_local_definitions",
    "static_mut_refs",
    "invalid_value",
    "type_alias_bounds",
    "trivial_bounds",
    "double_negations",
    "dropping_references",
    "forgetting_references",
    "dropping_copy_types",
    "forgetting_copy_types",
    "while_true",
    "mismatched_lifetime_syntaxes",
    "non_shorthand_field_patterns",
    "improper_ctypes",
    "improper_ctypes_definitions",
    "const_item_mutation",
    "function_item_references",
    "dangling_pointers_from_locals",
    "dangling_pointers_from_temporaries",
    "invalid_nan_comparisons",
    "invalid_from_utf8",
    "unnecessary_transmutes",
];

/// `cargo clippy` の引数。`--force-warn` は候補先頭の `#![allow(...)]` でも警告を出させる (#237)。
/// CARGO_TERM_COLOR=always(CI 既定)だと警告行に ANSI が付き件数を数えられないため色を切る。
pub(super) fn clippy_args(is_bench: bool) -> Vec<&'static str> {
    let mut args = vec!["clippy"];
    if is_bench {
        args.push("--benches");
    }
    args.extend(["--color", "never", "--", "--force-warn", "clippy::all"]);
    for lint in RUSTC_FORCE_WARN_LINTS {
        args.push("--force-warn");
        args.push(lint);
    }
    args
}

/// Rust 候補の test ステージで使う `cargo` 引数。ベンチ候補は `--benches` を付ける (#231)。
pub(super) fn rust_test_args(is_bench: bool) -> &'static [&'static str] {
    if is_bench {
        &["test", "--color", "never", "--benches"]
    } else {
        &["test", "--color", "never"]
    }
}

/// Rust 候補の compile ステージで使う `cargo` 引数。
///
/// ベンチ候補は `benches/bench.rs` に置かれ、既定の `cargo build`/`test`/`clippy` は
/// bench ターゲットをビルドしないため、`--benches` を付けて候補本体を検査する (#171)。
pub(super) fn rust_build_args(is_bench: bool) -> &'static [&'static str] {
    if is_bench {
        &["build", "--benches"]
    } else {
        &["build"]
    }
}

/// Rust 候補: 一時 Cargo プロジェクトで compile → test → lint(clippy) → prop_test → wasm → mutation。
pub(crate) fn run_rust_stages(
    root: &Path,
    candidate: &Candidate,
    ctx: &StageContext,
) -> Result<StageResults> {
    let StageContext {
        timeout,
        tests_dir,
        prop_tests_dir,
        wasm_opts,
        run_mutation,
    } = *ctx;
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
    build.args(rust_build_args(is_bench)).current_dir(root);
    let compile = runner::run_stage("compile", build, timeout);
    if !compile.is_passed() {
        return Ok(StageResults::skipped_after_compile(compile));
    }

    let mut t = Command::new("cargo");
    t.args(rust_test_args(is_bench)).current_dir(root);
    let test = run_test_stage("test", Language::Rust, t, timeout, false);

    let mut c = Command::new("cargo");
    c.args(clippy_args(is_bench)).current_dir(root);
    let (lint, lint_stderr, truncated) = runner::run_stage_capture_flagged("clippy", c, timeout);
    let lint_warnings = floor_if_truncated(count_lint_warnings(&lint_stderr), truncated);

    let prop_test = if let Some(prop_dir) = prop_tests_dir {
        copy_ext_keep_existing(prop_dir, &root.join("tests"), "rs", "prop_")?;
        let mut pt = Command::new("cargo");
        pt.args(["test", "--color", "never"]).current_dir(root);
        run_test_stage("prop_test", Language::Rust, pt, timeout, false)
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

/// エントリ名が Rust の単純な識別子か検証する（ハーネスへの注入防止）。
pub(super) fn is_valid_entry(name: &str) -> bool {
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
pub(super) fn run_wasm_stage(
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

/// `cargo mutants` で Rust 候補のミューテーションテストを実行する。
///
/// - `cargo mutants` が PATH にない場合は `(Skipped, 0, 0)` を返す（減点なし）。
/// - タイムアウトは test ステージの 3 倍を目安として渡す（ミュータント数に比例するため）。
pub(super) fn run_rust_mutation(root: &Path, timeout: Duration) -> (StageOutcome, usize, usize) {
    // cargo mutants が利用可能か確認
    let mutants_available = tool_available("cargo", &["mutants", "--version"]);
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
pub(super) fn parse_mutants_json(stdout: &str) -> Option<(usize, usize)> {
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

/// `cargo audit --json` で Rust 候補の脆弱性をスキャンし、検出件数を返す。
///
/// - `cargo audit` が PATH にない場合や `Cargo.lock` が存在しない場合は `Some(0)`（減点なし・従来どおり）。
/// - 実行したが JSON が得られない（タイムアウト・起動失敗・RustSec DB 取得エラー）場合は
///   `None`（audit 不能）。脆弱性検出時も exit 1 だが JSON は出るため exit code でなく JSON 可否で判定する。
pub(super) fn run_cargo_audit(root: &Path, timeout: Duration) -> Option<usize> {
    let audit_available = tool_available("cargo", &["audit", "--version"]);
    if !audit_available {
        return Some(0);
    }
    if !root.join("Cargo.lock").exists() {
        return Some(0);
    }

    let mut cmd = Command::new("cargo");
    cmd.args(["audit", "--json"]).current_dir(root);
    let (_outcome, stdout) = runner::run_stage_capture("audit", cmd, timeout);
    parse_audit_json(&stdout)
}

/// `cargo audit --json` の出力から脆弱性件数を取り出す。
pub(super) fn parse_audit_json(stdout: &str) -> Option<usize> {
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

/// Criterion ベンチを実行し、中央値 (ns) を返す。
///
/// 時間は stdout ではなく Criterion が書く `target/criterion/**/new/estimates.json`
/// の `median.point_estimate` から読む（候補が stdout に偽の `time:` 行を出しても影響しない, #239）。
/// 複数ベンチがある場合は最小中央値（最速）を採用する（従来の stdout 解析と同じ規則）。
/// estimates.json が無い・壊れている場合は `None`（計測失敗）で、stdout へはフォールバックしない。
///
/// 限界: 候補コードはベンチ実行中に `target/` へ書けるため estimates.json 自体の偽造は防げない。
/// 実行前に既存の `target/criterion` を削除し、持ち込まれた古い/偽のファイルは除く。
pub(super) fn run_bench_stage(root: &Path, timeout: Duration) -> Option<u64> {
    if !root.join("benches").exists() {
        return None;
    }
    let _ = std::fs::remove_dir_all(root.join("target/criterion"));
    let (_outcome, output) = runner::run_stage_capture("bench", bench_command(root), timeout);
    bench_ns_after_run(root, &output)
}

/// `cargo bench` のコマンドを組み立てる。出力先を `root/target` に固定し、
/// 運用側の `CARGO_TARGET_DIR`（許可リスト経由で入る）で結果の場所がずれないようにする。
fn bench_command(root: &Path) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.arg("bench")
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join("target"));
    cmd
}

/// ベンチ実行後に bench_ns を決める。`stdout` は候補が自由に出力できるため受け取って無視する
/// （偽の `time:` 行で操作されないことをテストで固定するための境界, #239）。
fn bench_ns_after_run(root: &Path, _stdout: &str) -> Option<u64> {
    read_criterion_estimates_ns(root)
}

const MAX_ESTIMATES_BYTES: u64 = 1 << 20;

const MAX_ESTIMATES_DEPTH: usize = 8;

/// `target/criterion` 配下の `new/estimates.json` を集め、最小の中央値 (ns) を返す。
fn read_criterion_estimates_ns(root: &Path) -> Option<u64> {
    fn walk(dir: &Path, depth: usize, min: &mut Option<f64>) {
        if depth > MAX_ESTIMATES_DEPTH {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            // symlink は辿らない
            if !ft.is_dir() {
                continue;
            }
            let path = entry.path();
            if entry.file_name() == "new" {
                let f = path.join("estimates.json");
                let ok = std::fs::symlink_metadata(&f)
                    .map(|m| m.is_file() && m.len() <= MAX_ESTIMATES_BYTES)
                    .unwrap_or(false);
                if ok {
                    if let Some(ns) = std::fs::read_to_string(&f)
                        .ok()
                        .and_then(|t| parse_estimates_median_ns(&t))
                    {
                        *min = Some(min.map_or(ns, |p| p.min(ns)));
                    }
                }
            } else {
                walk(&path, depth + 1, min);
            }
        }
    }
    let mut min = None;
    walk(&root.join("target/criterion"), 0, &mut min);
    // 0.5ns 未満が 0 に丸められると scoring で最速=0 扱いになり比率が歪むため、下限を 1ns にする (#285)
    min.map(|v| (v.round() as u64).max(1))
}

/// estimates.json の `median.point_estimate` (ns) を取り出す純関数。
/// slope でなく median を選ぶ理由: median は常に存在し、Flat サンプリングでは slope が欠けるため。
/// 従来の typical() (= slope) との差は実測で小さい（median 287.79 / slope 287.53 / mean 287.89 ns）。
/// 欠損・壊れた JSON・非有限値・0 以下は `None`。
fn parse_estimates_median_ns(json: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let x = v.get("median")?.get("point_estimate")?.as_f64()?;
    (x.is_finite() && x > 0.0).then_some(x)
}

/// Criterion の出力から中央値 (ns) を抽出する。
///
/// 出力形式: `bench_name  time:   [lo_val lo_unit mid_val mid_unit hi_val hi_unit]`
/// 複数ベンチがある場合は最小中央値を返す。
#[allow(dead_code)] // #239: 実行経路では未使用（stdout は信用しない）。既存テストと回帰テストのため残す
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

    #[test]
    fn rust_build_args_builds_benches_only_for_bench_candidates() {
        // ベンチ候補は benches/bench.rs に置かれ、既定の cargo build では検査されない (#171)。
        assert_eq!(rust_build_args(true), ["build", "--benches"]);
        // 通常の lib 候補のコマンドは変えない。
        assert_eq!(rust_build_args(false), ["build"]);
    }

    /// 回帰 (#231): test / clippy もベンチ候補では `--benches` を付ける。
    #[test]
    fn test_and_clippy_args_include_benches_only_for_bench_candidates() {
        assert_eq!(
            rust_test_args(true),
            ["test", "--color", "never", "--benches"]
        );
        assert_eq!(rust_test_args(false), ["test", "--color", "never"]);
        let b = clippy_args(true);
        assert_eq!(&b[..4], ["clippy", "--benches", "--color", "never"]);
        assert!(!clippy_args(false).contains(&"--benches"));
    }

    /// 回帰 (#231): Criterion 用 Cargo.toml にも proptest を dev-dependency として含める。
    /// (含めるだけ。prop_test ステージ自体はベンチ候補向けに変えていない。)
    #[test]
    fn criterion_cargo_toml_declares_proptest_dev_dependency() {
        assert!(CARGO_TOML_WITH_CRITERION.contains("proptest"));
        assert!(CARGO_TOML_WITH_CRITERION.contains("criterion"));
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
mod clippy_args_tests {
    use super::clippy_args;

    #[test]
    fn clippy_args_force_warn_clippy_and_rustc_lints() {
        // `-W` は候補の `#![allow]` に負けるため `--force-warn` を使う (#237)
        let a = clippy_args(false);
        assert!(!a.contains(&"-W"));
        for lint in [
            "clippy::all",
            "nonstandard_style",
            "deprecated",
            "unused_variables",
        ] {
            let i = a.iter().position(|x| *x == lint).unwrap();
            assert_eq!(a[i - 1], "--force-warn");
        }
        // 既定 allow の lint を含む `unused` グループは通常候補の件数を変えるので使わない
        assert!(!a.contains(&"unused"));
    }
}

#[cfg(test)]
mod bench_estimates_tests {
    use super::*;

    fn est(median: &str) -> String {
        format!(
            r#"{{"mean":{{"point_estimate":999.0}},"median":{{"point_estimate":{median},"standard_error":1.0}}}}"#
        )
    }

    fn write_est(root: &Path, rel: &str, body: &str) {
        let dir = root.join("target/criterion").join(rel).join("new");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("estimates.json"), body).unwrap();
    }

    #[test]
    fn parse_estimates_reads_median() {
        assert_eq!(parse_estimates_median_ns(&est("123.4")), Some(123.4));
    }

    #[test]
    fn parse_estimates_rejects_bad_input() {
        assert_eq!(parse_estimates_median_ns(""), None);
        assert_eq!(parse_estimates_median_ns("{broken"), None);
        assert_eq!(
            parse_estimates_median_ns(r#"{"mean":{"point_estimate":1.0}}"#),
            None
        );
        assert_eq!(parse_estimates_median_ns(&est("0.0")), None);
        assert_eq!(parse_estimates_median_ns(&est("-5.0")), None);
        assert_eq!(parse_estimates_median_ns(&est("null")), None);
        assert_eq!(parse_estimates_median_ns(&est("\"NaN\"")), None);
        assert_eq!(parse_estimates_median_ns(&est("1e999")), None);
    }

    #[test]
    fn read_estimates_picks_min_across_benches_including_nested() {
        let d = tempfile::tempdir().unwrap();
        write_est(d.path(), "a", &est("300.0"));
        write_est(d.path(), "grp/b", &est("20.4"));
        write_est(d.path(), "c", "{broken");
        assert_eq!(read_criterion_estimates_ns(d.path()), Some(20));
    }

    /// 回帰 (#285): 0.5ns 未満の median が 0ns に丸められて performance の比率が歪まない。
    #[test]
    fn read_estimates_sub_ns_is_not_rounded_to_zero() {
        let d = tempfile::tempdir().unwrap();
        write_est(d.path(), "fast", &est("0.3"));
        assert_eq!(read_criterion_estimates_ns(d.path()), Some(1));
        let d2 = tempfile::tempdir().unwrap();
        write_est(d2.path(), "slow", &est("2.0"));
        let fast = read_criterion_estimates_ns(d.path()).unwrap();
        let slow = read_criterion_estimates_ns(d2.path()).unwrap();
        assert!(fast > 0, "0ns になってはいけない");
        assert_eq!(fast as f64 / slow as f64, 0.5);
    }

    #[test]
    fn read_estimates_ignores_non_new_dirs() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("target/criterion/a/base");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("estimates.json"), est("1.0")).unwrap();
        assert_eq!(read_criterion_estimates_ns(d.path()), None);
    }

    #[test]
    fn read_estimates_missing_is_none() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read_criterion_estimates_ns(d.path()), None);
    }

    /// 回帰 (#239): run_bench_stage が使う bench_ns_after_run 経由で検証する。
    /// 偽の stdout 行 + estimates.json 無しなら None。
    #[test]
    fn forged_stdout_line_does_not_yield_bench_ns() {
        let forged = "x time: [0 ns 0 ns 0 ns]";
        assert_eq!(parse_criterion_ns(forged), Some(0)); // 旧経路なら 0 になる行
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("target/criterion")).unwrap();
        assert_eq!(bench_ns_after_run(d.path(), forged), None);
    }

    /// 偽の stdout 行があっても正しい estimates.json の値が採用される。
    #[test]
    fn forged_stdout_line_is_ignored_when_estimates_exist() {
        let d = tempfile::tempdir().unwrap();
        write_est(d.path(), "a", &est("250.0"));
        let forged = "x time: [0 ns 0 ns 0 ns]\ny time: [1 ns 1 ns 1 ns]";
        assert_eq!(bench_ns_after_run(d.path(), forged), Some(250));
    }

    #[test]
    fn bench_command_pins_cargo_target_dir() {
        let d = tempfile::tempdir().unwrap();
        let cmd = bench_command(d.path());
        let v = cmd
            .get_envs()
            .find(|(k, _)| *k == "CARGO_TARGET_DIR")
            .and_then(|(_, v)| v.map(|v| v.to_os_string()));
        assert_eq!(v, Some(d.path().join("target").into_os_string()));
    }
}

#[cfg(test)]
mod mutation_tests {
    use super::*;

    #[test]
    fn parse_mutants_json_typical() {
        let output = "{\"total_mutants\":6,\"caught\":4}\n";
        assert_eq!(parse_mutants_json(output), Some((4, 6)));
    }

    #[test]
    fn parse_mutants_json_skips_progress_lines() {
        // 進捗ログが前後に混在しても JSON 行を採る。
        let output = "Found 6 mutants\nrunning...\n{\"total_mutants\":6,\"caught\":4}\ndone\n";
        assert_eq!(parse_mutants_json(output), Some((4, 6)));
    }

    #[test]
    fn parse_mutants_json_picks_last_json_line() {
        // 複数の JSON 行があれば末尾（最新）を使う。
        let output = "{\"total_mutants\":3,\"caught\":1}\n{\"total_mutants\":6,\"caught\":4}\n";
        assert_eq!(parse_mutants_json(output), Some((4, 6)));
    }

    #[test]
    fn parse_mutants_json_invalid_returns_none() {
        // run_rust_mutation はここで None になると黙って (0, 0) にする。
        assert_eq!(parse_mutants_json(""), None);
        assert_eq!(parse_mutants_json("just plain text\n"), None);
        // 必須キー欠落
        assert_eq!(parse_mutants_json("{\"total_mutants\":6}"), None);
        assert_eq!(parse_mutants_json("{\"caught\":4}"), None);
        // 型不一致（文字列・小数・null）
        assert_eq!(
            parse_mutants_json("{\"total_mutants\":\"6\",\"caught\":4}"),
            None
        );
        assert_eq!(
            parse_mutants_json("{\"total_mutants\":6.5,\"caught\":4}"),
            None
        );
        assert_eq!(
            parse_mutants_json("{\"total_mutants\":null,\"caught\":4}"),
            None
        );
        // 負数（usize に入らない）
        assert_eq!(
            parse_mutants_json("{\"total_mutants\":6,\"caught\":-1}"),
            None
        );
    }

    #[test]
    fn parse_mutants_json_skips_broken_json_line() {
        // 末尾の壊れた `{` 行を飛ばし、手前の有効行を拾う。
        let output = "{\"total_mutants\":6,\"caught\":4}\n{\"total_mutants\":6,\"cau\n";
        assert_eq!(parse_mutants_json(output), Some((4, 6)));
    }

    #[test]
    fn parse_mutants_json_caught_exceeding_total_is_returned_as_is() {
        // 現状の挙動の固定であり、仕様ではない。
        // caught > total_mutants でもクランプせずそのまま返る。
        // 将来クランプ等を入れる場合はこのテストを更新してよい。
        let output = "{\"total_mutants\":3,\"caught\":5}\n";
        assert_eq!(parse_mutants_json(output), Some((5, 3)));
    }
}
