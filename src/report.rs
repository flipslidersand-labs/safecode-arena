//! Markdown レポート生成。

use crate::model::{Evaluation, StageOutcome};

fn stage_cell(o: &StageOutcome) -> String {
    match o {
        StageOutcome::Passed { duration_ms } => format!("✅ {duration_ms}ms"),
        StageOutcome::Failed { .. } => "❌ failed".to_string(),
        StageOutcome::TimedOut { limit_ms } => format!("⏱ timeout({limit_ms}ms)"),
        StageOutcome::Skipped => "— skipped".to_string(),
    }
}

fn clippy_cell(o: &StageOutcome, warnings: usize) -> String {
    match o {
        StageOutcome::Passed { .. } if warnings == 0 => "✅ 0 warn".to_string(),
        StageOutcome::Passed { .. } => format!("⚠️ {warnings} warn"),
        StageOutcome::Failed { .. } => "❌ failed".to_string(),
        StageOutcome::TimedOut { limit_ms } => format!("⏱ timeout({limit_ms}ms)"),
        StageOutcome::Skipped => "— skipped".to_string(),
    }
}

/// ランク済みの評価から Markdown レポートを生成する。
pub fn render(evals: &[Evaluation]) -> String {
    let mut out = String::new();
    out.push_str("# SafeCode Arena 評価レポート\n\n");
    out.push_str(&format!(
        "生成日時: {}\n\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    out.push_str(
        "| 順位 | 候補 | 合計 | 正誤 | 安全 | 性能 | 保守 | コンパイル | テスト | Clippy | PropTest |\n",
    );
    out.push_str(
        "| ---- | ---- | ---- | ---- | ---- | ---- | ---- | ---------- | ------ | ------ | -------- |\n",
    );
    for (i, e) in evals.iter().enumerate() {
        out.push_str(&format!(
            "| {} | {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {} | {} | {} | {} |\n",
            i + 1,
            e.candidate_id,
            e.score,
            e.axes.correctness,
            e.axes.security,
            e.axes.performance,
            e.axes.maintainability,
            stage_cell(&e.compile),
            stage_cell(&e.test),
            clippy_cell(&e.clippy, e.clippy_warnings),
            stage_cell(&e.prop_test),
        ));
    }
    if let Some(best) = evals.first() {
        out.push_str(&format!(
            "\n**採用候補**: `{}`（{:.1}点）\n",
            best.candidate_id, best.score
        ));
    }
    out
}
