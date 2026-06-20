//! 候補を一時環境で実行する検証ランナー。
//!
//! MVP: プロセスベースの隔離（一時ディレクトリ + タイムアウト）。
//! 発展: Wasmtime/WASI による capability ベースのサンドボックス。

use crate::model::StageOutcome;
use std::time::{Duration, Instant};

/// プロセスをタイムアウト付きで実行し、ステージ結果へ変換する。
///
/// NOTE: MVP の素朴な実装。`wait_timeout` クレート導入で
///       本物のタイムアウト kill に置き換える（implementation-guide Phase 2）。
pub fn run_stage(label: &str, mut command: std::process::Command, limit: Duration) -> StageOutcome {
    let start = Instant::now();
    let output = match command.output() {
        Ok(o) => o,
        Err(e) => {
            return StageOutcome::Failed {
                detail: format!("{label}: 起動失敗: {e}"),
            }
        }
    };
    let elapsed = start.elapsed();

    if elapsed > limit {
        return StageOutcome::TimedOut {
            limit_ms: limit.as_millis() as u64,
        };
    }
    if output.status.success() {
        StageOutcome::Passed {
            duration_ms: elapsed.as_millis() as u64,
        }
    } else {
        StageOutcome::Failed {
            detail: String::from_utf8_lossy(&output.stderr)
                .lines()
                .take(20)
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}
