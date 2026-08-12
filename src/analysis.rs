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
    /// - 関数あたり平均行数が 30 を超えた分だけ徐々に減点（100 行で 0）
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
/// `eslint --format compact` は末尾に `N problems` または `N problem` を出力する。
pub fn count_eslint_findings(output: &str) -> usize {
    for line in output.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_suffix(" problems") {
            if let Ok(n) = rest.trim().parse::<usize>() {
                return n;
            }
        }
        if let Some(rest) = t.strip_suffix(" problem") {
            if let Ok(n) = rest.trim().parse::<usize>() {
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
}
