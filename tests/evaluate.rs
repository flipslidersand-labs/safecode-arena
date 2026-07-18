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

    // run #2: 未使用変数で lint 警告 → スコア下落 → リグレッション
    std::fs::write(
        &subject,
        "pub fn add(a: i32, b: i32) -> i32 { let x = 1; a + b }\n",
    )
    .unwrap();
    Command::cargo_bin("safecode")
        .unwrap()
        .arg("evaluate")
        .arg(&subject)
        .args(["--db", db.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("リグレッション検出"))
        .stderr(predicate::str::contains("subject"));

    // history: 2 run が並ぶ
    Command::cargo_bin("safecode")
        .unwrap()
        .args(["history", "--db", db.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("#1"))
        .stdout(predicate::str::contains("#2"));
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
