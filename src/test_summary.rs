//! テスト出力の結果行パーサ (#226 PR1)。
//!
//! 終了コードだけでは `exit(0)` / `os._exit(0)` による合格偽装を防げないため、
//! テストランナー自身が出す結果行を解析して判定材料にする。
//! 合格の定義は「exit 0 かつ結果行あり かつ失敗 0」。0 件実行(結果行はあるが 0 passed)は許可する。
//! 本モジュールは純関数のみで、pipeline への配線は PR2 で行う。
//!
//! `output` には stdout と stderr を連結したものを渡すこと
//! (cargo は `Running` を stderr、`test result:` を stdout に出す。node --test は pipeline で 1>&2 される)。
//!
//! `Ok { passed }` の意味は言語ごとに異なる: Rust/Python/JS/TS はテスト件数、Go はテストが走った
//! パッケージ数(`go test` の `ok` 行数。`-v` なしでは件数が出ないため)。

use crate::model::Language;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestVerdict {
    /// 結果行があり失敗 0。
    Ok { passed: u32 },
    /// 結果行から失敗(または異常)を検出した。
    Failed { reason: String },
    /// 期待する結果行が見つからない(途中終了・偽装の疑い)。
    NoSummary,
}

/// 言語に応じてテスト出力を解析する。
pub fn parse_test_summary(lang: Language, output: &str) -> TestVerdict {
    let text = strip_ansi(output);
    match lang {
        Language::Rust => parse_cargo(&text),
        Language::Python => parse_pytest(&text),
        Language::Go => parse_go(&text),
        // `node --test` は Node のバージョン・TTY 有無で tap (`# pass N`) と
        // spec (`ℹ pass N`) の 2 形式になるため両方を受け付ける。
        // TS も `node --test dist/*.test.js` なので同じ形式。
        Language::JavaScript | Language::TypeScript => parse_node_test(&text),
    }
}

/// ANSI エスケープシーケンス (CSI) を除去する。
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for n in chars.by_ref() {
                if ('\x40'..='\x7e').contains(&n) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `... N word ...` の N を返す(区切りは空白・`;`・`,`)。
fn count_of(line: &str, word: &str) -> Option<u32> {
    let toks: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || c == ';' || c == ',')
        .filter(|t| !t.is_empty())
        .collect();
    toks.windows(2)
        .find(|w| w[1] == word)
        .and_then(|w| w[0].parse().ok())
}

fn parse_cargo(text: &str) -> TestVerdict {
    // テストバイナリ起動 (Running) と doctest 起動 (Doc-tests) の数 = 結果行の数であるべき。
    let mut launched = 0usize;
    let mut results = 0usize;
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut bad_status = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("Running ") || t.starts_with("Doc-tests ") {
            launched += 1;
        } else if let Some(rest) = t.strip_prefix("test result:") {
            results += 1;
            if rest.split_whitespace().next() != Some("ok.") {
                bad_status = true;
            }
            passed += count_of(rest, "passed").unwrap_or(0);
            failed += count_of(rest, "failed").unwrap_or(0);
        }
    }
    if failed > 0 || bad_status {
        return TestVerdict::Failed {
            reason: format!("cargo test: {failed} 件失敗"),
        };
    }
    if results == 0 || results != launched {
        return TestVerdict::NoSummary;
    }
    TestVerdict::Ok { passed }
}

fn parse_pytest(text: &str) -> TestVerdict {
    // -q では "3 passed in 0.01s"、通常は "==== 3 passed in 0.01s ====" 形式。末尾側の最後のサマリ行を採用。
    let summary = text.lines().rev().find_map(|line| {
        let t = line.trim().trim_matches('=').trim();
        let (head, tail) = t.rsplit_once(" in ")?;
        let dur = tail.split_whitespace().next()?.strip_suffix('s')?;
        dur.parse::<f64>().ok()?;
        Some(head.to_string())
    });
    let Some(head) = summary else {
        return TestVerdict::NoSummary;
    };
    if head.contains("no tests ran") {
        return TestVerdict::Ok { passed: 0 };
    }
    let failed = count_of(&head, "failed").unwrap_or(0);
    let errors = count_of(&head, "error")
        .or_else(|| count_of(&head, "errors"))
        .unwrap_or(0);
    if failed > 0 || errors > 0 {
        return TestVerdict::Failed {
            reason: format!("pytest: {failed} 件失敗, {errors} 件エラー"),
        };
    }
    let passed = count_of(&head, "passed");
    let other = [
        "skipped",
        "deselected",
        "xfailed",
        "xpassed",
        "warnings",
        "warning",
    ]
    .iter()
    .any(|w| count_of(&head, w).is_some());
    match passed {
        Some(n) => TestVerdict::Ok { passed: n },
        None if other => TestVerdict::Ok { passed: 0 },
        None => TestVerdict::NoSummary,
    }
}

fn parse_go(text: &str) -> TestVerdict {
    let mut ok = 0u32;
    let mut no_files = 0u32;
    let mut failed = false;
    for line in text.lines() {
        let first = line.split_whitespace().next();
        if line.starts_with("--- FAIL") || first == Some("FAIL") {
            failed = true;
        } else if first == Some("ok") {
            ok += 1;
        } else if first == Some("?") && line.contains("[no test files]") {
            no_files += 1;
        }
    }
    if failed {
        TestVerdict::Failed {
            reason: "go test: FAIL を検出".to_string(),
        }
    } else if ok + no_files > 0 {
        TestVerdict::Ok { passed: ok }
    } else {
        TestVerdict::NoSummary
    }
}

fn parse_node_test(text: &str) -> TestVerdict {
    let (mut pass, mut fail, mut cancelled) = (None, None, 0u32);
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("# ").or_else(|| line.strip_prefix("ℹ ")) else {
            continue;
        };
        let mut it = rest.split_whitespace();
        let (Some(key), Some(val)) = (it.next(), it.next()) else {
            continue;
        };
        let Ok(n) = val.parse::<u32>() else { continue };
        match key {
            "pass" => pass = Some(n),
            "fail" => fail = Some(n),
            "cancelled" => cancelled = n,
            _ => {}
        }
    }
    match (pass, fail) {
        (Some(_), Some(f)) if f > 0 || cancelled > 0 => TestVerdict::Failed {
            reason: format!("node --test: {f} 件失敗, {cancelled} 件キャンセル"),
        },
        (Some(p), Some(_)) => TestVerdict::Ok { passed: p },
        _ => TestVerdict::NoSummary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(v: &TestVerdict) -> bool {
        matches!(v, TestVerdict::Failed { .. })
    }

    const RUST_OK: &str = "   Running unittests src/lib.rs (target/debug/deps/x-1)\n\
running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n\
     Running tests/it.rs (target/debug/deps/it-2)\n\
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n\
   Doc-tests x\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";

    #[test]
    fn rust_multi_binary_ok() {
        assert_eq!(
            parse_test_summary(Language::Rust, RUST_OK),
            TestVerdict::Ok { passed: 6 }
        );
    }

    #[test]
    fn rust_zero_tests_ok() {
        let o = "     Running unittests src/lib.rs (a)\n\
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
        assert_eq!(
            parse_test_summary(Language::Rust, o),
            TestVerdict::Ok { passed: 0 }
        );
    }

    #[test]
    fn rust_failed() {
        let o = "     Running unittests src/lib.rs (a)\n\
test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out\n";
        assert!(failed(&parse_test_summary(Language::Rust, o)));
    }

    #[test]
    fn rust_no_summary() {
        assert_eq!(
            parse_test_summary(Language::Rust, "   Compiling x\n"),
            TestVerdict::NoSummary
        );
    }

    #[test]
    fn rust_binary_exited_midway() {
        // 2 バイナリ起動したが結果行は 1 つ(2 つ目が exit(0) した)。
        let o = "     Running unittests src/lib.rs (a)\n\
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n\
     Running tests/it.rs (b)\nrunning 1 test\n";
        assert_eq!(
            parse_test_summary(Language::Rust, o),
            TestVerdict::NoSummary
        );
    }

    #[test]
    fn rust_ansi_colors() {
        let o = "\x1b[1m\x1b[92m     Running\x1b[0m unittests src/lib.rs (a)\n\
test result: \x1b[32mok\x1b[0m. 4 passed; 0 failed; 0 ignored\n";
        // 色が "ok" だけに付くので "ok." は ANSI 除去後に成立する。
        assert_eq!(
            parse_test_summary(Language::Rust, o),
            TestVerdict::Ok { passed: 4 }
        );
    }

    #[test]
    fn python_ok_quiet_and_banner() {
        assert_eq!(
            parse_test_summary(Language::Python, "...\n3 passed in 0.01s\n"),
            TestVerdict::Ok { passed: 3 }
        );
        assert_eq!(
            parse_test_summary(Language::Python, "==== 2 passed, 1 skipped in 0.12s ====\n"),
            TestVerdict::Ok { passed: 2 }
        );
    }

    #[test]
    fn python_zero_tests() {
        assert_eq!(
            parse_test_summary(Language::Python, "no tests ran in 0.01s\n"),
            TestVerdict::Ok { passed: 0 }
        );
    }

    #[test]
    fn python_failed_and_error() {
        let v = parse_test_summary(Language::Python, "=== 1 failed, 2 passed in 0.5s ===\n");
        assert!(failed(&v));
        let v = parse_test_summary(Language::Python, "=== 1 error in 0.5s ===\n");
        assert!(failed(&v));
    }

    #[test]
    fn python_no_summary() {
        assert_eq!(
            parse_test_summary(Language::Python, "....\n"),
            TestVerdict::NoSummary
        );
    }

    #[test]
    fn go_ok_and_no_test_files() {
        let o = "ok  \texample/a\t0.003s\n?   \texample/b\t[no test files]\n";
        assert_eq!(
            parse_test_summary(Language::Go, o),
            TestVerdict::Ok { passed: 1 }
        );
        let o = "?   \texample/b\t[no test files]\n";
        assert_eq!(
            parse_test_summary(Language::Go, o),
            TestVerdict::Ok { passed: 0 }
        );
    }

    #[test]
    fn go_failed() {
        let o = "--- FAIL: TestX (0.00s)\nFAIL\nFAIL\texample/a\t0.002s\nok  \texample/b\t0.1s\n";
        assert!(failed(&parse_test_summary(Language::Go, o)));
    }

    #[test]
    fn go_no_summary() {
        assert_eq!(parse_test_summary(Language::Go, ""), TestVerdict::NoSummary);
    }

    #[test]
    fn node_tap_and_spec() {
        let tap = "# tests 2\n# pass 2\n# fail 0\n# cancelled 0\n";
        let spec = "ℹ tests 2\nℹ pass 2\nℹ fail 0\nℹ cancelled 0\n";
        for lang in [Language::JavaScript, Language::TypeScript] {
            assert_eq!(parse_test_summary(lang, tap), TestVerdict::Ok { passed: 2 });
            assert_eq!(
                parse_test_summary(lang, spec),
                TestVerdict::Ok { passed: 2 }
            );
        }
    }

    #[test]
    fn node_zero_failed_and_missing() {
        let z = "ℹ pass 0\nℹ fail 0\n";
        assert_eq!(
            parse_test_summary(Language::JavaScript, z),
            TestVerdict::Ok { passed: 0 }
        );
        let f = "# pass 1\n# fail 1\n";
        assert!(failed(&parse_test_summary(Language::JavaScript, f)));
        let c = "# pass 1\n# fail 0\n# cancelled 1\n";
        assert!(failed(&parse_test_summary(Language::TypeScript, c)));
        assert_eq!(
            parse_test_summary(Language::TypeScript, "ok\n"),
            TestVerdict::NoSummary
        );
    }
}
