//! コアデータ構造。`docs/data-model.md` と対応させること。

use serde::{Deserialize, Serialize};

/// 検証対象の言語。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Python,
    Go,
    JavaScript,
    TypeScript,
}

impl Language {
    /// 対応している拡張子（小文字）。
    pub const SUPPORTED_EXTENSIONS: [&'static str; 5] = ["rs", "py", "go", "js", "ts"];

    /// ファイル拡張子から言語を判定する。未知の拡張子は `None`。
    pub fn try_from_extension(ext: &str) -> Option<Language> {
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Some(Language::Rust),
            "py" => Some(Language::Python),
            "go" => Some(Language::Go),
            "js" => Some(Language::JavaScript),
            "ts" => Some(Language::TypeScript),
            _ => None,
        }
    }

    /// 互換用: 未知の拡張子は Rust 既定。新規コードは `try_from_extension` を使うこと。
    pub fn from_extension(ext: &str) -> Language {
        Self::try_from_extension(ext).unwrap_or(Language::Rust)
    }

    pub fn name(&self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Python => "python",
            Language::Go => "go",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
        }
    }
}

/// 1 つの AI 生成コード候補。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    /// 候補識別子（ファイル名やモデル名から導出）。
    pub id: String,
    /// ソースコード本体。
    pub source: String,
    pub language: Language,
}

/// 各検証ステージの結果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StageOutcome {
    /// 成功。付随する所要時間(ms)。
    Passed { duration_ms: u64 },
    /// 失敗。標準エラー等の要約。
    Failed { detail: String },
    /// タイムアウト。
    TimedOut { limit_ms: u64 },
    /// 未実行（前段が失敗した等）。
    Skipped,
}

impl StageOutcome {
    pub fn is_passed(&self) -> bool {
        matches!(self, StageOutcome::Passed { .. })
    }

    /// 所要時間(ms)。Passed のときのみ Some。
    pub fn duration_ms(&self) -> Option<u64> {
        match self {
            StageOutcome::Passed { duration_ms } => Some(*duration_ms),
            _ => None,
        }
    }
}

/// 評価軸ごとの獲得点。各軸の上限は `config::Rubric` の重み。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct AxisScores {
    pub correctness: f64,
    pub security: f64,
    pub performance: f64,
    pub maintainability: f64,
    pub resource_usage: f64,
    /// LLM セマンティックレビュー軸（phase14）。--llm-review 未指定時は 0。
    pub reasoning: f64,
}

impl AxisScores {
    /// 全軸の合計。
    pub fn total(&self) -> f64 {
        self.correctness
            + self.security
            + self.performance
            + self.maintainability
            + self.resource_usage
            + self.reasoning
    }
}

/// 1 候補に対する検証結果の集約。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evaluation {
    pub candidate_id: String,
    pub compile: StageOutcome,
    pub test: StageOutcome,
    /// linter（Rust: cargo clippy、Python: ruff）の実行結果。compile が Passed のときのみ実行。
    pub lint: StageOutcome,
    /// linter が報告した warning/finding 数。lint が Skipped/Failed のときは 0。
    pub lint_warnings: usize,
    /// property test の実行結果。--prop-tests 指定時のみ実行。
    pub prop_test: StageOutcome,
    /// Wasm サンドボックス実行の結果。--wasm-entry 指定時のみ実行。
    pub wasm: StageOutcome,
    /// Wasm 実行で消費した fuel（命令数）。実行しなかった場合は None。
    pub wasm_fuel_used: Option<u64>,
    /// ミューテーションテスト（cargo mutants）の実行結果。--mutation 指定時のみ実行。
    pub mutation: StageOutcome,
    /// ミューテーションテストで検出できたミュータント数。
    pub mutation_caught: usize,
    /// ミューテーションテストで生成されたミュータント総数。
    pub mutation_total: usize,
    /// cargo-audit が検出した RUSTSEC 脆弱性数。Rust 以外 / Cargo.lock なしは 0。
    pub audit_findings: usize,
    /// Criterion / #[bench] ベンチマークの中央値 (ns)。ベンチ未実行は None。
    pub bench_ns: Option<u64>,
    /// LLM セマンティックレビューの結果。--llm-review 未指定時は Skipped。
    pub reasoning: StageOutcome,
    /// LLM が返した 0.0–1.0 スコア。reasoning が Passed のときのみ意味を持つ。
    pub reasoning_score: f64,
    /// LLM コメント（最大 120 字）。
    pub reasoning_comment: Option<String>,
    /// 軸別の獲得点。
    pub axes: AxisScores,
    /// 0.0〜100.0 の総合スコア（= axes.total()）。
    pub score: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Language; 5] = [
        Language::Rust,
        Language::Python,
        Language::Go,
        Language::JavaScript,
        Language::TypeScript,
    ];

    #[test]
    fn supported_extensions_all_map_to_some() {
        for ext in Language::SUPPORTED_EXTENSIONS {
            assert!(Language::try_from_extension(ext).is_some(), "{ext}");
        }
    }

    #[test]
    fn supported_extensions_map_to_distinct_languages() {
        let langs: Vec<_> = Language::SUPPORTED_EXTENSIONS
            .iter()
            .map(|e| Language::try_from_extension(e).unwrap())
            .collect();
        assert_eq!(langs, ALL.to_vec());
    }

    #[test]
    fn try_from_extension_is_case_insensitive() {
        assert_eq!(Language::try_from_extension("RS"), Some(Language::Rust));
        assert_eq!(Language::try_from_extension("Py"), Some(Language::Python));
        assert_eq!(
            Language::try_from_extension("TS"),
            Some(Language::TypeScript)
        );
    }

    #[test]
    fn try_from_extension_rejects_unknown_and_malformed() {
        for ext in ["", "txt", "rs2", ".rs", "rs ", " rs", "r s", "\trs", "rs\n"] {
            assert_eq!(Language::try_from_extension(ext), None, "{ext:?}");
        }
    }

    #[test]
    fn from_extension_known_matches_try_from() {
        for ext in Language::SUPPORTED_EXTENSIONS {
            assert_eq!(
                Some(Language::from_extension(ext)),
                Language::try_from_extension(ext)
            );
        }
    }

    #[test]
    fn from_extension_falls_back_to_rust() {
        assert_eq!(Language::from_extension("unknown"), Language::Rust);
        assert_eq!(Language::from_extension(""), Language::Rust);
    }

    #[test]
    fn name_matches_serde_lowercase_rename() {
        for lang in ALL {
            let json = serde_json::to_string(&lang).unwrap();
            assert_eq!(json, format!("\"{}\"", lang.name()));
        }
    }

    #[test]
    fn name_round_trips_through_serde() {
        for lang in ALL {
            let json = format!("\"{}\"", lang.name());
            assert_eq!(serde_json::from_str::<Language>(&json).unwrap(), lang);
        }
    }

    #[test]
    fn duration_ms_is_some_only_for_passed() {
        assert_eq!(
            StageOutcome::Passed { duration_ms: 42 }.duration_ms(),
            Some(42)
        );
        assert_eq!(
            StageOutcome::Passed { duration_ms: 0 }.duration_ms(),
            Some(0)
        );
    }

    #[test]
    fn duration_ms_is_none_for_non_passed() {
        let others = [
            StageOutcome::Failed { detail: "x".into() },
            StageOutcome::TimedOut { limit_ms: 1000 },
            StageOutcome::Skipped,
        ];
        for o in others {
            assert_eq!(o.duration_ms(), None, "{o:?}");
        }
    }

    #[test]
    fn is_passed_only_for_passed() {
        assert!(StageOutcome::Passed { duration_ms: 1 }.is_passed());
        assert!(!StageOutcome::Skipped.is_passed());
        assert!(!StageOutcome::TimedOut { limit_ms: 1 }.is_passed());
    }

    #[test]
    fn axis_scores_total_sums_all_six_axes() {
        let a = AxisScores {
            correctness: 1.0,
            security: 2.0,
            performance: 4.0,
            maintainability: 8.0,
            resource_usage: 16.0,
            reasoning: 32.0,
        };
        // 2 の冪なので、どの軸が欠けても/重複しても合計が一致しない。
        assert_eq!(a.total(), 63.0);
    }

    #[test]
    fn axis_scores_default_total_is_zero() {
        assert_eq!(AxisScores::default().total(), 0.0);
    }
}
