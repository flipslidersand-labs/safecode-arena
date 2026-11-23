//! Go 候補のステージ (go build / go test / staticcheck / gremlins)。

use super::*;

/// `staticcheck` が PATH に存在するか確認する。
fn staticcheck_available() -> bool {
    tool_available("staticcheck", &["--version"])
}

/// staticcheck / go vet の出力が「実行失敗」を示すか判定する純関数 (#158)。
///
/// 確認済み: コマンド不在 (`sh: 1: staticcheck: not found`)、go の起動エラー
/// (`go: parsing $GOFLAGS: ...`)。`go: downloading ...` 等の進捗行は失敗扱いにしない。
fn go_lint_run_failed(output: &str) -> bool {
    output.contains(": not found")
        || output.contains("command not found")
        || output.contains("go: parsing $GOFLAGS")
}

/// Go 候補: 一時モジュールで go build → go test → staticcheck（なければ go vet）。
/// prop_test / wasm は非対応のため Skipped。
pub(crate) fn run_go_stages(
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
    // go.mod を生成（依存なしの最小モジュール）
    let go_mod = "module candidate\n\ngo 1.22\n";
    fs::write(root.join("go.mod"), go_mod).context("go.mod の書込に失敗")?;
    fs::write(root.join("candidate.go"), &candidate.source).context("候補ソースの書込に失敗")?;
    if let Some(dir) = tests_dir {
        copy_ext_no_clobber(dir, root, "go")?;
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
    // -v: ok パッケージに `=== RUN` 行があることを run_test_stage が確認する (init の os.Exit(0) 対策)。
    t.args(["test", "-v", "./..."]).current_dir(root);
    let test = run_test_stage("test", Language::Go, t, timeout, go_has_test_funcs(root));

    // lint: staticcheck を優先、なければ go vet にフォールバック
    let (lint, lint_warnings) = if staticcheck_available() {
        let mut l = Command::new("sh");
        l.arg("-c")
            .arg("staticcheck ./... 2>&1; true")
            .current_dir(root);
        let (outcome, out, truncated) =
            runner::run_stage_capture_flagged("staticcheck", l, timeout);
        lint_outcome(
            outcome,
            &out,
            truncated,
            go_lint_run_failed,
            count_staticcheck_findings,
        )
    } else {
        let mut l = Command::new("sh");
        l.arg("-c").arg("go vet ./... 1>&2; true").current_dir(root);
        let (outcome, out, truncated) = runner::run_stage_capture_flagged("go-vet", l, timeout);
        lint_outcome(
            outcome,
            &out,
            truncated,
            go_lint_run_failed,
            count_go_vet_findings,
        )
    };

    let (mutation, mutation_caught, mutation_total) = if run_mutation && test.is_passed() {
        run_go_mutation(root, timeout)
    } else {
        (StageOutcome::Skipped, 0, 0)
    };

    Ok(StageResults {
        compile,
        test,
        lint: LintResult {
            outcome: lint,
            warnings: lint_warnings,
        },
        mutation: MutationResult {
            outcome: mutation,
            caught: mutation_caught,
            total: mutation_total,
        },
        ..Default::default()
    })
}

/// `gremlins` で Go 候補のミューテーションテストを実行する。
///
/// - `gremlins` が PATH にない場合は `(Skipped, 0, 0)` を返す（減点なし）。
fn run_go_mutation(root: &Path, timeout: Duration) -> (StageOutcome, usize, usize) {
    let gremlins_available = tool_available("gremlins", &["--version"]);
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

/// 候補ディレクトリ直下の `*_test.go` に実行対象のテスト関数 (`func Test` / `func Fuzz`) があるか。
/// ある場合、`[no tests to run]` 付きの ok 行でも `=== RUN` を必須にする
/// (候補の init が同じ文字列を出して exit(0) する偽装を防ぐ)。
/// Benchmark / Example のみのファイルは `go test` で RUN 行が出ないため対象外。
fn go_has_test_funcs(root: &Path) -> bool {
    root.read_dir()
        .map(|rd| {
            rd.flatten().any(|e| {
                e.file_name().to_string_lossy().ends_with("_test.go")
                    && fs::read_to_string(e.path())
                        .map(|c| c.contains("func Test") || c.contains("func Fuzz"))
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
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
}

#[cfg(test)]
mod lint_run_failed_tests {
    use super::go_lint_run_failed;

    #[test]
    fn go_missing_binary_is_failure() {
        assert!(go_lint_run_failed("sh: 1: go: not found"));
        assert!(go_lint_run_failed("sh: 1: staticcheck: not found"));
        assert!(go_lint_run_failed("sh: staticcheck: command not found"));
    }

    #[test]
    fn go_tool_errors_are_failures() {
        assert!(go_lint_run_failed(
            "go: parsing $GOFLAGS: unknown flag -bogus"
        ));
    }

    #[test]
    fn go_findings_and_clean_are_not_failures() {
        assert!(!go_lint_run_failed(
            "# candidate\n./candidate.go:3:14: fmt.Printf format %d has arg \"a\" of wrong type string\n"
        ));
        assert!(!go_lint_run_failed(
            "candidate.go:5:2: S1000 use plain channel send (S1000)\n"
        ));
        assert!(!go_lint_run_failed(
            "go: downloading example.com/x v1.0.0\n"
        ));
        assert!(!go_lint_run_failed(""));
    }
}

#[cfg(test)]
mod go_has_test_funcs_tests {
    use super::*;

    #[test]
    fn go_test_files_detection() {
        let d = tempfile::tempdir().unwrap();
        assert!(!go_has_test_funcs(d.path()));
        fs::write(
            d.path().join("b_test.go"),
            "func BenchmarkX(b *testing.B) {}\n",
        )
        .unwrap();
        assert!(!go_has_test_funcs(d.path()));
        fs::write(d.path().join("a_test.go"), "func TestA(t *testing.T) {}\n").unwrap();
        assert!(go_has_test_funcs(d.path()));
    }
}
