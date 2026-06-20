//! コアデータ構造。`docs/data-model.md` と対応させること。

use serde::{Deserialize, Serialize};

/// 検証対象の言語。MVP では Rust のみ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    // 発展: Python, Go, JavaScript
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
}

/// 1 候補に対する検証結果の集約。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evaluation {
    pub candidate_id: String,
    pub compile: StageOutcome,
    pub test: StageOutcome,
    /// 0.0〜100.0 の総合スコア。
    pub score: f64,
}
