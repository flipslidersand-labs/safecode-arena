//! Python 候補のステージ (py_compile / pytest / ruff / mutmut)。

use super::common::parse_mutation_json;
use super::*;

/// Python 候補: py_compile（構文チェック）→ pytest → ruff。
/// prop_test / wasm は非対応のため Skipped。
pub(crate) fn run_python_stages(
    root: &Path,
    candidate: &Candidate,
    ctx: &StageContext,
) -> Result<StageResults> {
    let StageContext {
        timeout,
        tests_dir,
        run_mutation,
        ..
    } = *ctx;
    fs::write(root.join("candidate.py"), &candidate.source).context("候補ソースの書込に失敗")?;
    if let Some(dir) = tests_dir {
        copy_ext_no_clobber(dir, root, "py")?;
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
    let test = run_test_stage("test", Language::Python, t, timeout, false);

    // lint: ruff（指摘があっても stage は成功扱い、件数のみ採点に使う）
    // ruff 未導入・実行失敗は「指摘 0 件」ではなく lint Skipped にする (#158)
    let (lint, lint_warnings) = if ruff_available() {
        let mut l = Command::new("sh");
        l.arg("-c")
            .arg("ruff check --ignore-noqa --output-format=concise candidate.py 1>&2; true")
            .current_dir(root);
        let (outcome, out, truncated) = runner::run_stage_capture_flagged("ruff", l, timeout);
        lint_outcome(
            outcome,
            &out,
            truncated,
            ruff_run_failed,
            count_ruff_findings,
        )
    } else {
        (StageOutcome::Skipped, 0)
    };

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
    let mutmut_available = tool_available("mutmut", &["--version"]);
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
    parse_mutation_json(stdout)
}

/// `ruff` が PATH に存在するか確認する。
fn ruff_available() -> bool {
    tool_available("ruff", &["--version"])
}

/// ruff の出力が「実行失敗」(コマンド不在・設定ファイル不正などの異常終了)を示すか判定する純関数 (#158)。
///
/// ruff は指摘ありでも異常終了でも終了コードが 1/2 で紛れるため、出力の語句で判定する。
/// 語句は実出力で確認済み: 不在=`sh: 1: ruff: not found`、設定不正=`ruff failed\n  Cause: ...`。
pub(super) fn ruff_run_failed(output: &str) -> bool {
    output.contains(": not found")
        || output.contains("command not found")
        || output.lines().any(|l| l.trim() == "ruff failed")
}

#[cfg(test)]
mod mutation_tests {
    use super::*;

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
mod lint_run_failed_tests {
    use super::ruff_run_failed;

    #[test]
    fn ruff_missing_binary_is_failure() {
        assert!(ruff_run_failed("sh: 1: ruff: not found"));
        assert!(ruff_run_failed("sh: ruff: command not found"));
    }

    #[test]
    fn ruff_internal_error_is_failure() {
        assert!(ruff_run_failed(
            "ruff failed\n  Cause: Failed to parse pyproject.toml\n"
        ));
    }

    #[test]
    fn ruff_findings_and_clean_are_not_failures() {
        assert!(!ruff_run_failed(
            "candidate.py:1:8: F401 [*] `os` imported but unused\nFound 1 error.\n"
        ));
        assert!(!ruff_run_failed("All checks passed!\n"));
        assert!(!ruff_run_failed(""));
    }
}
