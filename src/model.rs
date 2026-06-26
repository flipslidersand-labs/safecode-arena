//! コアデータ構造。`docs/data-model.md` と対応させること。

use serde::{Deserialize, Serialize};

/// 検証対象の言語。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Python,
    // 発展: Go, JavaScript
}

impl Language {
    /// ファイル拡張子から言語を判定する。未知の拡張子は Rust 既定。
    pub fn from_extension(ext: &str) -> Language {
        match ext.to_ascii_lowercase().as_str() {
            "py" => Language::Python,
            _ => Language::Rust,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Python => "python",
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
}

impl AxisScores {
    /// 全軸の合計。
    pub fn total(&self) -> f64 {
        self.correctness
            + self.security
            + self.performance
            + self.maintainability
            + self.resource_usage
    }
}

/// 1 候補に対する検証結果の集約。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evaluation {
    pub candidate_id: String,
    pub compile: StageOutcome,
    pub test: StageOutcome,
    /// cargo clippy の実行結果。compile が Passed のときのみ実行。
    pub clippy: StageOutcome,
    /// clippy が報告した warning 数。clippy が Skipped/Failed のときは 0。
    pub clippy_warnings: usize,
    /// property test の実行結果。--prop-tests 指定時のみ実行。
    pub prop_test: StageOutcome,
    /// Wasm サンドボックス実行の結果。--wasm-entry 指定時のみ実行。
    pub wasm: StageOutcome,
    /// Wasm 実行で消費した fuel（命令数）。実行しなかった場合は None。
    pub wasm_fuel_used: Option<u64>,
    /// 軸別の獲得点。
    pub axes: AxisScores,
    /// 0.0〜100.0 の総合スコア（= axes.total()）。
    pub score: f64,
}
