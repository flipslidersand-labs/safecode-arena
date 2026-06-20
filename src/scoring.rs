//! 採点ルーブリック。重みは `config::Rubric`（既定値は `docs/spec.md` と一致）。
//!
//! MVP では correctness（コンパイル+テスト通過）のみを実測し、
//! 残りの軸は 0 点（未計測）として扱う。

use crate::config::Rubric;
use crate::model::{Evaluation, StageOutcome};

/// 採点: コンパイル通過で correctness の半分、テスト通過で残り半分を付与。
///
/// security / performance / maintainability / resource_usage は
/// Phase 3 以降で実装（現状は加点なし）。
pub fn score(compile: &StageOutcome, test: &StageOutcome, rubric: &Rubric) -> f64 {
    let mut s = 0.0;
    if compile.is_passed() {
        s += rubric.correctness * 0.5;
    }
    if test.is_passed() {
        s += rubric.correctness * 0.5;
    }
    s
}

/// 評価集合を総合スコア降順に並べ替えて採用候補を決める。
pub fn rank(mut evals: Vec<Evaluation>) -> Vec<Evaluation> {
    evals.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    evals
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Evaluation;

    fn passed() -> StageOutcome {
        StageOutcome::Passed { duration_ms: 1 }
    }
    fn failed() -> StageOutcome {
        StageOutcome::Failed { detail: "x".into() }
    }

    #[test]
    fn compile_and_test_pass_gives_full_correctness() {
        let r = Rubric::default();
        assert_eq!(score(&passed(), &passed(), &r), r.correctness);
    }

    #[test]
    fn compile_only_gives_half_correctness() {
        let r = Rubric::default();
        assert_eq!(
            score(&passed(), &StageOutcome::Skipped, &r),
            r.correctness * 0.5
        );
    }

    #[test]
    fn compile_fail_gives_zero() {
        let r = Rubric::default();
        assert_eq!(score(&failed(), &StageOutcome::Skipped, &r), 0.0);
    }

    #[test]
    fn custom_rubric_weight_is_used() {
        let r = Rubric {
            correctness: 80.0,
            ..Rubric::default()
        };
        assert_eq!(score(&passed(), &passed(), &r), 80.0);
    }

    #[test]
    fn rank_orders_by_score_desc() {
        let mk = |id: &str, s: f64| Evaluation {
            candidate_id: id.into(),
            compile: passed(),
            test: passed(),
            score: s,
        };
        let ranked = rank(vec![mk("a", 25.0), mk("b", 50.0), mk("c", 0.0)]);
        let ids: Vec<_> = ranked.iter().map(|e| e.candidate_id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a", "c"]);
    }
}
