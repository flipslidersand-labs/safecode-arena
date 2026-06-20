//! Phase 1 の E2E テスト。`safecode evaluate` を実バイナリで起動し、
//! 一時 Cargo プロジェクトでの compile/test/採点/レポートを検証する。
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
        // 採点表に ✅ が出て、スコア 0 ではない（コンパイル+テスト通過で 50.0）
        .stdout(predicate::str::contains("✅"))
        .stdout(predicate::str::contains("50.0"))
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
