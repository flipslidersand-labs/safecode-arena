//! E2E テスト。`safecode evaluate` を実バイナリで起動し、
//! 一時 Cargo プロジェクトでの compile/test/lint/採点/レポートを検証する。
//!
//! Phase 3 以降のスコア上限:
//!   prop_test なし → correctness = 80%（compile 40% + test 40%）
//!   安全(20) + 性能(15) + 保守(10) = 45、合計 = 40+45 = 85.0
//!
//! NOTE: 内部で `cargo build`/`cargo test` を起動するため実行は遅い。

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;

fn write_candidate(name: &str, source: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new()
        .prefix(name)
        .suffix(".rs")
        .tempfile()
        .unwrap();
    f.write_all(source.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

#[test]
fn evaluate_compiling_candidate_scores_above_zero() {
    let cand = write_candidate("ok", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        // compile+test+lint(0 warn)+性能(単独最速) 85.0
        // prop_test は Skipped なので correctness は 80% (40+40)
        .stdout(predicate::str::contains("✅"))
        .stdout(predicate::str::contains("85.0"))
        .stdout(predicate::str::contains("0 warn"))
        .stdout(predicate::str::contains("採用候補"));
}

#[test]
fn evaluate_broken_candidate_skips_test_and_scores_zero() {
    let cand = write_candidate("ng", "pub fn broken(\n");

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("❌"))
        .stdout(predicate::str::contains("skipped"))
        .stdout(predicate::str::contains("0.0"));
}

#[test]
#[ignore = "criterion の取得・ビルドが必要。CI は ignored-tests.yml が --ignored で実行する (#171)"]
fn evaluate_broken_criterion_bench_candidate_fails_compile() {
    // Criterion 候補は benches/bench.rs に置かれる。構文エラーでも compile/test が
    // Passed になっていた回帰を防ぐ (#171)。
    let cand = write_candidate(
        "broken_bench",
        "use criterion::{criterion_group, criterion_main, Criterion};\n\
         fn b(c: &mut Criterion) { c.bench_function(\"x\", |b| b.iter(|| 1 + 1)); }\n\
         criterion_group!(benches, b);\n\
         criterion_main!(benches);\n\
         fn broken(\n",
    );

    let out = Command::cargo_bin("safecode")
        .unwrap()
        .args(["evaluate", "--format", "json"])
        .arg(cand.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let e = &v[0];
    assert!(
        e["compile"].get("Failed").is_some(),
        "compile: {}",
        e["compile"]
    );
    assert_eq!(e["test"], "Skipped");
    assert_eq!(e["score"], 0.0);
}

#[test]
fn evaluate_without_candidates_fails() {
    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .assert()
        .failure()
        .stderr(predicate::str::contains("候補ファイルを"));
}

#[test]
fn evaluate_json_format_emits_structured_output() {
    let cand = write_candidate("ok", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");

    Command::cargo_bin("safecode")
        .unwrap()
        .args(["evaluate", "--format", "json"])
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("\"candidate_id\""))
        .stdout(predicate::str::contains("\"axes\""))
        .stdout(predicate::str::contains("\"maintainability\""))
        .stdout(predicate::str::contains("\"score\""))
        .stdout(predicate::str::contains("\"Passed\""));
}

#[test]
fn evaluate_with_external_tests_dir() {
    let cand = write_candidate("ok", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");

    // 外部統合テスト（候補クレート名は "candidate"）
    let tests_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        tests_dir.path().join("ext.rs"),
        "#[test]\nfn external_add_works() { assert_eq!(candidate::add(2, 3), 5); }\n",
    )
    .unwrap();

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .args(["--tests", tests_dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("85.0"))
        .stdout(predicate::str::contains("✅"))
        .stdout(predicate::str::contains("0 warn"));
}

#[test]
fn db_persists_runs_and_detects_regression() {
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("history.db");
    // 候補 ID を固定するため同名ファイルを使う。
    let subject = work.path().join("subject.rs");

    // run #1: lint 警告なしのクリーンな候補（安全 20）
    std::fs::write(&subject, "pub fn add(a: i32, b: i32) -> i32 { a + b }\n").unwrap();
    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(&subject)
        .args(["--db", db.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("run #1 を保存"));

    // run #2: 未使用変数 2 件で lint 警告 → スコア下落 → リグレッション
    // (警告 1 件の減点は 0.8 点でしきい値 1.0 未満のため 2 件にする。#248 でまとめ行を数えなくなった)
    std::fs::write(
        &subject,
        "pub fn add(a: i32, b: i32) -> i32 { let x = 1; let y = 2; a + b }\n",
    )
    .unwrap();
    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(&subject)
        .args(["--db", db.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("リグレッション検出"))
        .stderr(predicate::str::contains("subject"))
        .stderr(predicate::str::contains("保存しません"));

    // run #3: 同じ劣化状態で再実行しても、基準は #1 のままなので再び失敗する（#178）
    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(&subject)
        .args(["--db", db.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "リグレッション検出（直近 run #1 比）",
        ));

    // history: 回帰 run は保存されないので #1 のみ
    Command::cargo_bin("safecode")
        .unwrap()
        .args(["history", "--db", db.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("#1"))
        .stdout(predicate::str::contains("#2").not());
}

fn write_py(name: &str, source: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new()
        .prefix(name)
        .suffix(".py")
        .tempfile()
        .unwrap();
    f.write_all(source.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

#[test]
#[ignore = "pytest/ruff が必要。CI は ignored-tests.yml が --ignored で実行する (#108)"]
fn python_clean_candidate_scores() {
    // 構文 OK・ruff 指摘なし・テストなし(=成功扱い)。
    // correctness 40 + security 20 + maintainability 10 + performance 15 = 85.0
    let cand = write_py("ok", "def add(a, b):\n    return a + b\n");

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("85.0"))
        .stdout(predicate::str::contains("✅"));
}

#[test]
fn python_syntax_error_fails_compile() {
    // コロン欠落 → py_compile 失敗。
    let cand = write_py("ng", "def add(a, b)\n    return a + b\n");

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("❌"))
        .stdout(predicate::str::contains("0.0"));
}

#[test]
fn python_inline_test_failure_detected() {
    // candidate.py 内の test_ 関数が pytest に収集されて失敗することを確認。
    // 修正前: pytest が candidate.py をスキップ → exit 5 → 成功扱い → score 85.0
    // 修正後: pytest が test_wrong を実行 → 失敗 → test stage Failed → score 50.0
    // （test 失敗時は performance も 0 になるため 50 = compile20 + sec20 + maint10）
    let cand = write_py(
        "inline_fail",
        "def add(a, b):\n    return a + b\n\ndef test_wrong():\n    assert add(1, 2) == 99\n",
    );

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("50.0"))
        .stdout(predicate::str::contains("❌ failed"));
}

#[test]
fn rust_exit_zero_in_test_is_detected_as_failed() {
    // テスト中に exit(0) で打ち切ると cargo test は終了コード 0 だが結果行が出ない (#226)。
    let cand = write_candidate(
        "exit0",
        "pub fn f() -> i32 { std::process::exit(0) }\n\
         #[cfg(test)]\nmod t {\n    #[test]\n    fn a() { super::f(); }\n}\n",
    );

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("❌ failed"))
        .stdout(predicate::str::contains("85.0").not());
}

#[test]
fn python_os_exit_zero_in_candidate_is_detected_as_failed() {
    // os._exit(0) で pytest を打ち切っても終了コード 0 になるが、結果行が無いので失敗扱い (#226)。
    let cand = write_py(
        "exit0",
        "import os\n\ndef add(a, b):\n    return a + b\n\ndef test_x():\n    os._exit(0)\n",
    );

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("❌ failed"))
        .stdout(predicate::str::contains("85.0").not());
}

#[test]
#[ignore = "wasm32-wasip1 ターゲットが必要。CI は ignored-tests.yml が --ignored で実行する (#108)"]
fn wasm_entry_runs_sandbox_and_awards_resource_usage() {
    // wasm32-wasip1 にビルドして隔離実行できる候補。
    let cand = write_candidate("ok", "pub fn run() {}\n");

    Command::cargo_bin("safecode")
        .unwrap()
        .args(["evaluate", "--format", "json", "--wasm-entry", "run"])
        .arg(cand.path())
        .assert()
        .success()
        // Wasm ステージが Passed し、resource_usage が満点(5.0)になる。
        .stdout(predicate::str::contains("\"wasm\""))
        .stdout(predicate::str::contains("\"resource_usage\": 5.0"));
}

#[test]
#[ignore = "wasm32-wasip1 ターゲットが必要。CI は ignored-tests.yml が --ignored で実行する (#108)"]
fn db_skips_comparison_when_run_options_differ() {
    // #191: 同じコードでも --wasm-entry の有無で resource_usage が 5 点変わる。
    // 実行条件が違う run とは比較せず、回帰として exit 1 にしない。
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("history.db");
    let subject = work.path().join("subject.rs");
    std::fs::write(&subject, "pub fn run() {}\n").unwrap();

    Command::cargo_bin("safecode")
        .unwrap()
        .args(["evaluate", "--wasm-entry", "run"])
        .arg(&subject)
        .args(["--db", db.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("run #1 を保存"));

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(&subject)
        .args(["--db", db.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("実行条件が同じ過去 run がない"))
        .stderr(predicate::str::contains("リグレッション検出").not())
        .stderr(predicate::str::contains("run #2 を保存"));

    // 条件を戻すと #1 と比較され、同じコードなので回帰なし。
    Command::cargo_bin("safecode")
        .unwrap()
        .args(["evaluate", "--wasm-entry", "run"])
        .arg(&subject)
        .args(["--db", db.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "リグレッションなし（直近 run #1 と比較",
        ));
}

// ── Go ──────────────────────────────────────────────────────────────────────

fn write_go(name: &str, source: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new()
        .prefix(name)
        .suffix(".go")
        .tempfile()
        .unwrap();
    f.write_all(source.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

#[test]
fn go_clean_candidate_scores() {
    // 構文 OK・go vet 指摘なし・テストなし（go test "./..." は "[no test files]" で exit 0）。
    let cand = write_go(
        "ok",
        "package candidate\n\nfunc Add(a, b int) int {\n\treturn a + b\n}\n",
    );

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("85.0"))
        .stdout(predicate::str::contains("✅"));
}

#[test]
fn go_forged_no_tests_marker_with_exit_zero_is_failed() {
    // init が "[no tests to run]" 相当を出して exit(0) しても、Test 関数があれば合格しない (#226)。
    let cand = write_go(
        "forged",
        "package candidate\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n\n\
         func init() {\n\tfmt.Println(\"testing: warning: no tests to run\")\n\tos.Exit(0)\n}\n\n\
         func Add(a, b int) int {\n\treturn a + b\n}\n",
    );
    let tests_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        tests_dir.path().join("c_test.go"),
        "package candidate\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n\tt.Fatal(\"x\")\n}\n",
    )
    .unwrap();

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .args(["--tests", tests_dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("❌ failed"))
        .stdout(predicate::str::contains("85.0").not());
}

#[test]
fn go_syntax_error_fails_compile() {
    // 型構文エラー → go build 失敗。
    let cand = write_go(
        "ng",
        "package candidate\n\nfunc Add(a, b int int {\n\treturn a + b\n}\n",
    );

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("❌"))
        .stdout(predicate::str::contains("0.0"));
}

// ── JavaScript ───────────────────────────────────────────────────────────────

fn write_js(name: &str, source: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new()
        .prefix(name)
        .suffix(".js")
        .tempfile()
        .unwrap();
    f.write_all(source.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

#[test]
fn js_clean_candidate_scores() {
    // 構文 OK・テストファイルなし。candidate.js 自体は node --test に渡さない（#159）ので
    // test は Skipped となり、満点にはならない（compile のみで 50.0）。
    let cand = write_js(
        "ok",
        "function add(a, b) { return a + b; }\nmodule.exports = { add };\n",
    );

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("50.0"))
        .stdout(predicate::str::contains("✅"));
}

#[test]
fn js_syntax_error_fails_compile() {
    // 引数リスト構文エラー → node --check 失敗。
    let cand = write_js("ng", "function add(a b) { return a + b; }\n");

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("❌"))
        .stdout(predicate::str::contains("0.0"));
}

// ── TypeScript ────────────────────────────────────────────────────────────────

fn write_ts(name: &str, source: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new()
        .prefix(name)
        .suffix(".ts")
        .tempfile()
        .unwrap();
    f.write_all(source.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

#[test]
fn ts_clean_candidate_scores_or_skips() {
    // tsc が PATH になければ Skipped → スコア 0.0 かつ ⏭ を含む。
    // tsc があれば型検査 OK → 85.0 かつ ✅ を含む。
    let cand = write_ts(
        "ok",
        "function add(a: number, b: number): number { return a + b; }\n",
    );

    let output = Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let out = String::from_utf8_lossy(&output);
    // tsc の有無に関わらず safecode 自体は exit 0 でなければならない。
    assert!(
        out.contains("85.0") || out.contains("0.0"),
        "expected score 85.0 (tsc found) or 0.0 (tsc skipped), got: {out}"
    );
}

#[test]
fn ts_type_error_fails_compile() {
    // 型エラー → tsc --noEmit 失敗。tsc がなければ Skipped（スコア 0.0）。
    let cand = write_ts("ng", "const x: number = \"not a number\";\n");

    let output = Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let out = String::from_utf8_lossy(&output);
    // tsc あり → ❌ (type error)、tsc なし → 0.0 (skipped)
    assert!(
        out.contains("❌") || out.contains("0.0"),
        "expected ❌ or 0.0 for type error, got: {out}"
    );
}

#[test]
fn evaluate_all_candidates_erroring_warns_with_id_and_fails() {
    // TMPDIR を存在しないパスにすると evaluate_candidate が一時ディレクトリ生成で Err を返す(#168)。
    // 1 候補ごとに ID 付きで警告して続行し、全滅なら非 0 終了する。
    let work = tempfile::tempdir().unwrap();
    let a = work.path().join("cand_a.rs");
    let b = work.path().join("cand_b.rs");
    std::fs::write(&a, "pub fn f() {}\n").unwrap();
    std::fs::write(&b, "pub fn g() {}\n").unwrap();
    let missing_tmp = work.path().join("no-such-tmp");

    Command::cargo_bin("safecode")
        .unwrap()
        .env("TMPDIR", &missing_tmp)
        .arg("evaluate")
        .arg(&a)
        .arg(&b)
        .assert()
        .failure()
        .stderr(predicate::str::contains("[warn] cand_a の評価失敗"))
        .stderr(predicate::str::contains("[warn] cand_b の評価失敗"))
        .stderr(predicate::str::contains("全候補の評価に失敗しました"));
}

#[test]
fn db_previous_candidate_dropped_by_eval_error_fails() {
    // #181: 前回 run にあった候補が今回評価エラーで脱落しても、他が成功すれば
    // 従来は比較されず exit 0 だった。--db 指定時は脱落を回帰として扱う。
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("history.db");
    let keep = work.path().join("keep.rs");
    let flaky = work.path().join("flaky.py");
    std::fs::write(&keep, "pub fn add(a: i32, b: i32) -> i32 { a + b }\n").unwrap();
    std::fs::write(&flaky, "def f():\n    return 1\n").unwrap();

    // run #1: 両方成功して保存される。
    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(&keep)
        .arg(&flaky)
        .args(["--db", db.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("run #1 を保存"));

    // run #2: tests ディレクトリ内の "bad.py" がディレクトリなので、Python 候補だけコピーで Err になる。
    let tests = work.path().join("tests");
    std::fs::create_dir_all(tests.join("bad.py")).unwrap();
    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(&keep)
        .arg(&flaky)
        .args(["--tests", tests.to_str().unwrap()])
        .args(["--db", db.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("[warn] flaky の評価失敗"))
        .stderr(predicate::str::contains("評価失敗で脱落"))
        .stderr(predicate::str::contains("flaky"))
        .stderr(predicate::str::contains("保存しません"));

    // --db なしでは従来どおり exit 0（generate と揃えた #173 の仕様）。
    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(&keep)
        .arg(&flaky)
        .args(["--tests", tests.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn evaluate_html_format_writes_file_with_out() {
    let cand = write_candidate("ok", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("report.html");

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .args(["--format", "html", "--out", out.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("レポートを書き出しました"));

    let html = std::fs::read_to_string(&out).unwrap();
    assert!(html.contains("<!DOCTYPE html>"), "html: {html}");
}

#[test]
fn evaluate_out_to_missing_directory_fails() {
    let cand = write_candidate("ok", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
    let work = tempfile::tempdir().unwrap();
    let out = work.path().join("no-such-dir").join("report.md");

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .args(["--out", out.to_str().unwrap()])
        .assert()
        .failure();
    assert!(!out.exists());
}

#[test]
fn history_with_empty_db_reports_no_runs() {
    let work = tempfile::tempdir().unwrap();
    let db = work.path().join("empty.db");

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("history")
        .args(["--db", db.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("run 履歴はありません"));
}

#[test]
fn generate_with_missing_spec_fails_before_calling_llm() {
    Command::cargo_bin("safecode")
        .unwrap()
        .arg("generate")
        .arg("/no/such/spec.md")
        .assert()
        .failure()
        .stderr(predicate::str::contains("spec ファイルの読込に失敗"));
}

#[test]
fn regression_threshold_rejects_negative_nan_and_inf() {
    let cand = write_candidate("ok", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
    for v in ["-1", "nan", "inf"] {
        Command::cargo_bin("safecode")
            .unwrap()
            .arg("evaluate")
            .arg(cand.path())
            .arg(format!("--regression-threshold={v}"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("regression-threshold"));
    }
}

#[test]
fn nan_weight_in_config_fails_at_startup() {
    let cand = write_candidate("ok", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
    let mut cfg = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
    cfg.write_all(b"[weights]\ncorrectness = nan\n").unwrap();
    cfg.flush().unwrap();
    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .arg("--config")
        .arg(cfg.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("correctness"));
}

/// 親(safecode 本体)の環境にある API キー相当の値が、候補コードの実行(cargo test)に
/// 継承されず、失敗 detail 経由でレポートへ漏れないこと (#225)。
#[test]
fn evaluate_does_not_leak_parent_env_secret_to_candidate() {
    let cand = write_candidate(
        "envprobe",
        r#"pub fn probe() -> i32 { 1 }

#[cfg(test)]
mod tests {
    #[test]
    fn reads_env() {
        let v = std::env::var("ANTHROPIC_API_KEY").unwrap_or_else(|_| "ENVPROBE_UNSET".into());
        panic!("ENVPROBE_RESULT={v}");
    }
}
"#,
    );

    let out = Command::cargo_bin("safecode")
        .unwrap()
        .env("ANTHROPIC_API_KEY", "dummy-secret-for-test")
        .args(["evaluate", "--format", "json"])
        .arg(cand.path())
        .output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // 候補のテストが実際に走り、キーが見えなかったことを示す。
    assert!(all.contains("ENVPROBE_RESULT=ENVPROBE_UNSET"), "{all}");
    assert!(!all.contains("dummy-secret-for-test"), "{all}");
}

// --- lint 抑止コメントの回帰テスト (#237) ---
// 候補が先頭で lint を抑止しても警告件数が 0 に化けないこと。

#[test]
fn rust_inner_allow_attribute_does_not_hide_clippy_warnings() {
    // rustc の unused_variables / nonstandard_style / deprecated と clippy の len_zero を含む。
    // 抑止属性を付けても 0 warn に化けない(clippy 側は `v.len() == 0` のみ)。
    // 4 = bad_name + deprecated 利用 + 未使用変数 + len_zero(cargo のまとめ行は数えない。#248)。
    let cand = write_candidate(
        "sup",
        "#![allow(clippy::all, warnings)]\n\
         pub struct bad_name;\n\
         #[deprecated]\n\
         pub fn old() {}\n\
         pub fn use_old() { old() }\n\
         pub fn no_items(v: &[i32]) -> bool { v.len() == 0 }\n\
         pub fn add(a: i32, b: i32) -> i32 { let x = 1; a + b }\n",
    );

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("4 warn"));
}

#[test]
#[ignore = "pytest/ruff が必要。CI は ignored-tests.yml が --ignored で実行する (#108)"]
fn python_noqa_file_directive_does_not_hide_ruff_findings() {
    // `import os` は未使用 import (F401) 1 件。`# ruff: noqa` を付けても 0 warn に化けない。
    let cand = write_py(
        "sup",
        "# ruff: noqa\nimport os\n\n\ndef add(a, b):\n    return a + b\n",
    );

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(cand.path())
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"\b[1-9][0-9]* warn").unwrap());
}

#[test]
#[ignore = "eslint(v9+) と node が必要。実行環境依存のため ignored (#237)"]
fn js_eslint_disable_comment_does_not_hide_findings() {
    // CI(ignored-tests.yml)には eslint が無い。パイプラインと同じ解決(PATH 上の `eslint`)で
    // 実行できなければ、skipped 表示を誤検知せず明示してスキップする。
    let eslint_ok = std::process::Command::new("eslint")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !eslint_ok {
        eprintln!("eslint 未導入のためスキップ: js_eslint_disable_comment_does_not_hide_findings");
        return;
    }
    let mut f = tempfile::Builder::new()
        .prefix("sup")
        .suffix(".js")
        .tempfile()
        .unwrap();
    f.write_all(b"/* eslint-disable */\nconst unused = 1;\nmodule.exports = {};\n")
        .unwrap();
    f.flush().unwrap();

    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(f.path())
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"\b[1-9][0-9]* warn").unwrap());
}
