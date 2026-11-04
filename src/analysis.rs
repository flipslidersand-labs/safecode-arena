//! 候補ソースの静的解析。各軸の「達成率」(0.0〜1.0) を算出する。
//!
//! Phase 3 は外部重量依存（Tree-sitter / cargo-audit / Criterion）を避け、
//! 決定的で軽量なヒューリスティックから始める。精度は後続フェーズで上げる
//! （ADR-006 参照）。

/// ソースから得る静的メトリクス。
#[derive(Debug, Clone, Copy)]
pub struct SourceMetrics {
    /// `unsafe` の出現数。
    pub unsafe_count: usize,
    /// 非空行数。
    pub loc: usize,
    /// `fn ` の出現数（関数定義の近似）。
    pub fn_count: usize,
    /// 最長行の文字数。
    pub max_line_len: usize,
}

impl SourceMetrics {
    pub fn analyze(source: &str) -> Self {
        let unsafe_count = source.matches("unsafe").count();
        // 関数定義の近似（言語横断: Rust "fn ", Python "def ", JS "function "）。
        let fn_count = source.matches("fn ").count()
            + source.matches("def ").count()
            + source.matches("function ").count();
        let loc = source.lines().filter(|l| !l.trim().is_empty()).count();
        let max_line_len = source.lines().map(|l| l.chars().count()).max().unwrap_or(0);
        SourceMetrics {
            unsafe_count,
            loc,
            fn_count,
            max_line_len,
        }
    }

    /// security 達成率: `unsafe` 1 つにつき 0.25 減点。
    pub fn security_ratio(&self) -> f64 {
        (1.0 - 0.25 * self.unsafe_count as f64).clamp(0.0, 1.0)
    }

    /// maintainability 達成率:
    /// - 関数あたり平均行数が 30 を超えた分だけ徐々に減点（超過 1 行につき 0.01、平均 130 行で 0）
    /// - 120 字を超える行があれば 0.1 減点
    pub fn maintainability_ratio(&self) -> f64 {
        let avg_fn_len = self.loc as f64 / self.fn_count.max(1) as f64;
        let len_penalty = ((avg_fn_len - 30.0).max(0.0)) / 100.0;
        let line_penalty = if self.max_line_len > 120 { 0.1 } else { 0.0 };
        (1.0 - len_penalty - line_penalty).clamp(0.0, 1.0)
    }
}

/// clippy の stderr 出力から lint warning 件数を数える（Rust: clippy output format）。
///
/// `cargo clippy` は各警告を `warning: ...` で始まる行として stderr に出力する。
/// ただし `warning: N warnings emitted` のサマリ行は重複なので除外する。
pub fn count_lint_warnings(stderr: &str) -> usize {
    stderr
        .lines()
        .filter(|l| {
            l.starts_with("warning:")
                && !l.contains("warnings emitted")
                && !l.contains("warning emitted")
        })
        .count()
}

/// ruff (`--output-format=concise`) の出力から指摘件数を数える。
///
/// 末尾の "Found N errors." サマリを優先し、無ければ `path:line:col:` 形式の
/// 診断行（`.py:` を含む行）を数える。
pub fn count_ruff_findings(output: &str) -> usize {
    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Found ") {
            if let Some(n) = rest.split_whitespace().next() {
                if let Ok(count) = n.parse::<usize>() {
                    return count;
                }
            }
        }
    }
    output
        .lines()
        .filter(|l| {
            l.contains(".py:")
                && l.split(':')
                    .nth(1)
                    .is_some_and(|s| s.trim().parse::<u32>().is_ok())
        })
        .count()
}

/// `go vet` の出力から指摘件数を数える。
///
/// go vet は問題を `path/to/file.go:line:col: message` 形式で stderr に出力する。
/// 末尾の `# module` や空行を除いた診断行数を返す。
pub fn count_go_vet_findings(stderr: &str) -> usize {
    stderr
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#') && t.contains(".go:")
        })
        .count()
}

/// `staticcheck` の出力から指摘件数を数える。
///
/// staticcheck は `file.go:line:col: message (SAxxxx)` 形式で stdout に出力する。
pub fn count_staticcheck_findings(stdout: &str) -> usize {
    stdout
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && t.contains(".go:")
        })
        .count()
}

/// ESLint の出力から指摘件数を数える。
///
/// 末尾のサマリ(`N problems` / `N problem`、または stylish の `✖ N problems (E errors, W warnings)`)を優先し、
/// 無ければ `.js:` を含む診断行(compact 形式)の数にフォールバックする。
pub fn count_eslint_findings(output: &str) -> usize {
    for line in output.lines() {
        // `✖ N problems (E errors, W warnings)` の括弧以降と先頭の `✖` を除いて判定する。
        let t = line.trim();
        let t = t
            .split(" (")
            .next()
            .unwrap_or(t)
            .trim_start_matches('✖')
            .trim();
        for suffix in [" problems", " problem"] {
            if let Some(n) = t
                .strip_suffix(suffix)
                .and_then(|r| r.trim().parse::<usize>().ok())
            {
                return n;
            }
        }
    }
    // fallback: count diagnostic lines (contain ".js:" and error/warning)
    output
        .lines()
        .filter(|l| {
            let t = l.trim();
            (t.contains("error") || t.contains("warning")) && t.contains(".js:")
        })
        .count()
}

/// Rust ソースに Criterion ベンチマークまたは組み込み `#[bench]` が含まれるか判定する。
pub fn has_bench(source: &str) -> bool {
    source.contains("use criterion")
        || source.contains("extern crate criterion")
        || source.contains("#[bench]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_reduces_security() {
        let clean = SourceMetrics::analyze("pub fn ok() {}\n");
        assert_eq!(clean.security_ratio(), 1.0);

        let risky = SourceMetrics::analyze("pub fn bad() { unsafe { } }\n");
        assert_eq!(risky.security_ratio(), 0.75);
    }

    #[test]
    fn small_function_is_fully_maintainable() {
        let m = SourceMetrics::analyze("pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
        assert_eq!(m.maintainability_ratio(), 1.0);
    }

    #[test]
    fn count_lint_warnings_ignores_summary() {
        let stderr =
            "warning: unused variable `x`\nwarning: unused import\nwarning: 2 warnings emitted\n";
        assert_eq!(count_lint_warnings(stderr), 2);
    }

    #[test]
    fn count_lint_warnings_zero_on_clean() {
        assert_eq!(count_lint_warnings(""), 0);
        assert_eq!(count_lint_warnings("error[E0001]: something\n"), 0);
    }

    #[test]
    fn very_long_function_loses_maintainability() {
        let mut src = String::from("pub fn big() {\n");
        for i in 0..120 {
            src.push_str(&format!("    let _x{i} = {i};\n"));
        }
        src.push_str("}\n");
        let m = SourceMetrics::analyze(&src);
        assert!(m.maintainability_ratio() < 1.0);
    }

    #[test]
    fn python_def_counts_as_function() {
        // Python の def も関数としてカウントされ、保守性が満点になる。
        let m = SourceMetrics::analyze("def add(a, b):\n    return a + b\n");
        assert_eq!(m.fn_count, 1);
        assert_eq!(m.maintainability_ratio(), 1.0);
    }

    #[test]
    fn count_ruff_findings_uses_summary() {
        let out = "candidate.py:1:8: F401 [*] `os` imported but unused\nFound 1 error.\n";
        assert_eq!(count_ruff_findings(out), 1);
    }

    #[test]
    fn count_ruff_findings_zero_when_clean() {
        assert_eq!(count_ruff_findings("All checks passed!\n"), 0);
        assert_eq!(count_ruff_findings(""), 0);
    }

    #[test]
    fn count_staticcheck_finds_diagnostics() {
        let out = "candidate.go:5:2: S1000 use plain channel send or receive (S1000)\n";
        assert_eq!(count_staticcheck_findings(out), 1);
    }

    #[test]
    fn count_staticcheck_zero_on_clean() {
        assert_eq!(count_staticcheck_findings(""), 0);
    }

    #[test]
    fn count_eslint_uses_summary_line() {
        let out = "candidate.js: line 3, col 7, warning - 'x' is defined but never used. (no-unused-vars)\n✖ 1 problem (0 errors, 1 warning)\n";
        assert_eq!(count_eslint_findings(out), 1);
    }

    #[test]
    fn count_eslint_zero_on_clean() {
        assert_eq!(count_eslint_findings(""), 0);
    }

    fn metrics(loc: usize, fn_count: usize, max_line_len: usize) -> SourceMetrics {
        SourceMetrics {
            unsafe_count: 0,
            loc,
            fn_count,
            max_line_len,
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn go_vet_counts_diagnostic_lines_only() {
        let out = "# candidate\n./candidate.go:5:2: unreachable code\n\n  \n./candidate.go:9:1: result of fmt.Sprintf call not used\n";
        assert_eq!(count_go_vet_findings(out), 2);
    }

    #[test]
    fn go_vet_ignores_header_blank_and_non_go_lines() {
        assert_eq!(count_go_vet_findings(""), 0);
        assert_eq!(count_go_vet_findings("# candidate\n"), 0);
        assert_eq!(
            count_go_vet_findings("vet: some failure\nexit status 1\n"),
            0
        );
        // `#` で始まる行は `.go:` を含んでいても除外される。
        assert_eq!(count_go_vet_findings("# candidate.go:1:1: x\n"), 0);
        // 前後の空白は trim されて数えられる。
        assert_eq!(count_go_vet_findings("  ./a.go:1:1: msg\n"), 1);
    }

    #[test]
    fn has_bench_detects_each_pattern() {
        assert!(has_bench("use criterion::Criterion;\n"));
        assert!(has_bench("extern crate criterion;\n"));
        assert!(has_bench("#[bench]\nfn b() {}\n"));
        assert!(!has_bench("fn main() {}\n"));
        assert!(!has_bench(""));
        // `use criterion` の部分一致のみで判定される（`criterion` 単独では不十分）。
        assert!(!has_bench("// criterion benchmark later\n"));
    }

    #[test]
    fn maintainability_avg_fn_len_boundary_at_30() {
        assert!(approx(metrics(30, 1, 0).maintainability_ratio(), 1.0));
        assert!(approx(metrics(31, 1, 0).maintainability_ratio(), 0.99));
        // 平均 = 60/2 = 30 → 減点なし。
        assert!(approx(metrics(60, 2, 0).maintainability_ratio(), 1.0));
    }

    #[test]
    fn maintainability_clamps_to_zero_at_130_lines() {
        // 30 行を超えた分を 100 行で打ち切るので、平均 130 行で 0。
        assert!(approx(metrics(100, 1, 0).maintainability_ratio(), 0.3));
        assert!(approx(metrics(130, 1, 0).maintainability_ratio(), 0.0));
        assert_eq!(metrics(10_000, 1, 200).maintainability_ratio(), 0.0);
    }

    #[test]
    fn maintainability_long_line_boundary_at_120() {
        assert!(approx(metrics(1, 1, 120).maintainability_ratio(), 1.0));
        assert!(approx(metrics(1, 1, 121).maintainability_ratio(), 0.9));
    }

    #[test]
    fn maintainability_fn_count_zero_treated_as_one() {
        assert!(approx(metrics(30, 0, 0).maintainability_ratio(), 1.0));
        assert!(approx(metrics(31, 0, 0).maintainability_ratio(), 0.99));
        // 実ソースでも fn が無ければ loc がそのまま平均になる。
        let m = SourceMetrics::analyze("let a = 1;\n");
        assert_eq!(m.fn_count, 0);
        assert!(approx(m.maintainability_ratio(), 1.0));
    }

    #[test]
    fn maintainability_penalties_combine() {
        assert!(approx(metrics(60, 1, 121).maintainability_ratio(), 0.6));
    }

    #[test]
    fn unsafe_counted_in_identifiers_and_comments() {
        // 現状の挙動: 単純な部分文字列一致なので誤検出する。
        assert_eq!(
            SourceMetrics::analyze("let not_unsafe = 1;\n").unsafe_count,
            1
        );
        assert_eq!(
            SourceMetrics::analyze("// no unsafe here\n").unsafe_count,
            1
        );
        assert_eq!(
            SourceMetrics::analyze("let s = \"unsafe\";\n").unsafe_count,
            1
        );
        assert_eq!(
            SourceMetrics::analyze("unsafe { unsafe { } }\n").unsafe_count,
            2
        );
    }

    #[test]
    fn unsafe_ratio_clamps_at_four() {
        let m = |n| SourceMetrics {
            unsafe_count: n,
            ..metrics(1, 1, 0)
        };
        assert!(approx(m(3).security_ratio(), 0.25));
        assert_eq!(m(4).security_ratio(), 0.0);
        assert_eq!(m(10).security_ratio(), 0.0);
    }

    #[test]
    fn analyze_loc_and_max_line_len() {
        let m = SourceMetrics::analyze("a\n\n   \nbbb\n");
        assert_eq!(m.loc, 2);
        assert_eq!(m.max_line_len, 3);
        let e = SourceMetrics::analyze("");
        assert_eq!((e.loc, e.max_line_len, e.fn_count), (0, 0, 0));
    }

    #[test]
    fn count_eslint_singular_and_plural_summary() {
        assert_eq!(count_eslint_findings("1 problem\n"), 1);
        assert_eq!(count_eslint_findings("3 problems\n"), 3);
        assert_eq!(count_eslint_findings("  12 problems  \n"), 12);
    }

    #[test]
    fn count_eslint_stylish_plural_summary() {
        let out = "\n/p/a.js\n  3:5  error  'x' is defined but never used  no-unused-vars\n\n✖ 5 problems (3 errors, 2 warnings)\n";
        assert_eq!(count_eslint_findings(out), 5);
        assert_eq!(
            count_eslint_findings("✖ 5 problems (3 errors, 2 warnings)\n"),
            5
        );
    }

    #[test]
    fn count_eslint_stylish_singular_summary() {
        assert_eq!(
            count_eslint_findings("✖ 1 problem (0 errors, 1 warning)\n"),
            1
        );
    }

    #[test]
    fn count_eslint_summary_wins_over_diagnostic_lines() {
        // サマリと `.js:` 診断行が併存しても二重計上しない。
        let out = "a.js: line 1, col 1, error - x\nb.js: line 2, col 1, warning - y\n✖ 5 problems (3 errors, 2 warnings)\n";
        assert_eq!(count_eslint_findings(out), 5);
    }

    #[test]
    fn count_eslint_without_summary_falls_back_to_line_scan() {
        let out = "a.js: line 1, col 1, error - x\nb.js: line 2, col 1, warning - y\n";
        assert_eq!(count_eslint_findings(out), 2);
        assert_eq!(count_eslint_findings(""), 0);
    }
}
