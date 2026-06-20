//! 採点ルーブリック。重みは `docs/spec.md` の評価軸と一致させる。
//!
//! correctness: 50, security: 20, performance: 15,
//! maintainability: 10, resource_usage: 5
//!
//! MVP では correctness（コンパイル+テスト通過）のみを実測し、
//! 残りの軸は 0 点（未計測）として扱う。

use crate::model::{Evaluation, StageOutcome};

pub const W_CORRECTNESS: f64 = 50.0;
pub const W_SECURITY: f64 = 20.0;
pub const W_PERFORMANCE: f64 = 15.0;
pub const W_MAINTAINABILITY: f64 = 10.0;
pub const W_RESOURCE: f64 = 5.0;

/// MVP の採点: コンパイル通過で correctness の半分、
/// テスト通過で残り半分を付与。
pub fn score(compile: &StageOutcome, test: &StageOutcome) -> f64 {
    let mut s = 0.0;
    if compile.is_passed() {
        s += W_CORRECTNESS * 0.5;
    }
    if test.is_passed() {
        s += W_CORRECTNESS * 0.5;
    }
    // security / performance / maintainability / resource_usage は
    // Phase 3 以降で実装（現状は加点なし）。
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
        assert_eq!(score(&passed(), &passed()), W_CORRECTNESS);
    }

    #[test]
    fn compile_only_gives_half_correctness() {
        assert_eq!(
            score(&passed(), &StageOutcome::Skipped),
            W_CORRECTNESS * 0.5
        );
    }

    #[test]
    fn compile_fail_gives_zero() {
        assert_eq!(score(&failed(), &StageOutcome::Skipped), 0.0);
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
