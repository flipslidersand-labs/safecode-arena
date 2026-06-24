//! 採点ルーブリック。重みは `config::Rubric`（既定値は `docs/spec.md` と一致）。
//!
//! - correctness:     コンパイル + テスト + property test 通過で実測
//! - security:        `unsafe` 使用 + clippy warning 数で実測（Phase 3）
//! - maintainability: 関数長ヒューリスティック + clippy warning 数（Phase 3）
//! - performance:     候補間の compile+test 時間の相対比較
//! - resource_usage:  未計測（0）。Phase 4 以降。

use crate::analysis::SourceMetrics;
use crate::config::Rubric;
use crate::model::{AxisScores, Evaluation, StageOutcome};

/// clippy warning 数から security 達成率を算出する。
///
/// 0 件 = 1.0、1 件ごとに 0.1 減点。clippy 自体が失敗なら 0.0 扱い。
fn clippy_security_ratio(clippy: &StageOutcome, warnings: usize) -> f64 {
    match clippy {
        StageOutcome::Passed { .. } => (1.0 - 0.1 * warnings as f64).clamp(0.0, 1.0),
        StageOutcome::Failed { .. } => 0.0,
        _ => 1.0, // Skipped / TimedOut は減点なし（clippy を実行しなかった場合）
    }
}

/// clippy warning 数から maintainability 補正係数を返す。
///
/// 0 件 = 1.0、3 件ごとに 0.1 減点。
fn clippy_maintainability_ratio(warnings: usize) -> f64 {
    (1.0 - 0.1 * (warnings / 3) as f64).clamp(0.0, 1.0)
}

/// performance を除く軸を採点する。performance は候補集合が揃ってから
/// [`assign_performance`] で相対的に付与する。
///
/// Phase 3 追加パラメータ:
/// - `prop_test`: property test ステージの結果（correctness に反映）
/// - `clippy`: clippy ステージの結果（security / maintainability に反映）
/// - `clippy_warnings`: clippy が報告した warning 数
///
/// Phase 4 追加パラメータ:
/// - `wasm`: Wasm サンドボックス実行の結果（resource_usage に反映）
#[allow(clippy::too_many_arguments)]
pub fn axes_without_performance(
    compile: &StageOutcome,
    test: &StageOutcome,
    prop_test: &StageOutcome,
    clippy: &StageOutcome,
    clippy_warnings: usize,
    wasm: &StageOutcome,
    metrics: &SourceMetrics,
    rubric: &Rubric,
) -> AxisScores {
    // correctness: compile(40%) + test(40%) + prop_test(20%)
    // prop_test が Skipped の場合は compile+test のみ（上限 80%）
    let mut correctness = 0.0;
    if compile.is_passed() {
        correctness += rubric.correctness * 0.4;
    }
    if test.is_passed() {
        correctness += rubric.correctness * 0.4;
    }
    if prop_test.is_passed() {
        correctness += rubric.correctness * 0.2;
    }

    // ビルドできないコードは security / maintainability を採点しない
    let (security, maintainability) = if compile.is_passed() {
        let unsafe_ratio = metrics.security_ratio();
        let clippy_sec = clippy_security_ratio(clippy, clippy_warnings);
        // security = unsafe ヒューリスティック(50%) + clippy(50%)
        let sec = rubric.security * (unsafe_ratio * 0.5 + clippy_sec * 0.5);

        let heuristic_maint = metrics.maintainability_ratio();
        let clippy_maint = clippy_maintainability_ratio(clippy_warnings);
        // maintainability = 関数長ヒューリスティック(60%) + clippy(40%)
        let maint = rubric.maintainability * (heuristic_maint * 0.6 + clippy_maint * 0.4);
        (sec, maint)
    } else {
        (0.0, 0.0)
    };

    // resource_usage: Wasm サンドボックスで正常実行できたら満点、失敗/タイムアウトは 0。
    // 未実行（--wasm-entry なし or compile 失敗）は Skipped → 0。
    let resource_usage = if wasm.is_passed() {
        rubric.resource_usage
    } else {
        0.0
    };

    AxisScores {
        correctness,
        security,
        maintainability,
        performance: 0.0,
        resource_usage,
    }
}

/// 候補集合の compile+test 所要時間を相対比較し、performance 軸を付与する。
/// 最速候補に満点、それ以外は `min_time / own_time` で按分。
/// compile か test が通っていない候補は performance 0。
/// 付与後、各 `score` を `axes.total()` で再計算する。
pub fn assign_performance(evals: &mut [Evaluation], rubric: &Rubric) {
    let times: Vec<Option<u64>> = evals
        .iter()
        .map(|e| match (e.compile.duration_ms(), e.test.duration_ms()) {
            (Some(c), Some(t)) => Some(c + t),
            _ => None,
        })
        .collect();

    let fastest = times.iter().flatten().copied().min();

    for (e, time) in evals.iter_mut().zip(times.iter()) {
        let ratio = match (time, fastest) {
            (Some(t), Some(min)) if *t > 0 => min as f64 / *t as f64,
            (Some(_), Some(_)) => 1.0, // 0ms は満点扱い
            _ => 0.0,
        };
        e.axes.performance = rubric.performance * ratio;
        e.score = e.axes.total();
    }
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

    fn passed(ms: u64) -> StageOutcome {
        StageOutcome::Passed { duration_ms: ms }
    }
    fn failed() -> StageOutcome {
        StageOutcome::Failed { detail: "x".into() }
    }
    fn metrics(src: &str) -> SourceMetrics {
        SourceMetrics::analyze(src)
    }
    fn axes(compile: &StageOutcome, test: &StageOutcome, src: &str) -> AxisScores {
        axes_without_performance(
            compile,
            test,
            &StageOutcome::Skipped,
            &StageOutcome::Skipped,
            0,
            &StageOutcome::Skipped,
            &metrics(src),
            &Rubric::default(),
        )
    }

    #[test]
    fn correctness_full_on_compile_and_test() {
        let r = Rubric::default();
        let a = axes(&passed(1), &passed(1), "fn f(){}");
        // compile(40%) + test(40%) = 80% of correctness (no prop_test)
        assert_eq!(a.correctness, r.correctness * 0.8);
    }

    #[test]
    fn correctness_with_prop_test_reaches_full() {
        let r = Rubric::default();
        let a = axes_without_performance(
            &passed(1),
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            0,
            &StageOutcome::Skipped,
            &metrics("fn f(){}"),
            &r,
        );
        assert_eq!(a.correctness, r.correctness);
    }

    #[test]
    fn correctness_half_on_compile_only() {
        let r = Rubric::default();
        let a = axes(&passed(1), &StageOutcome::Skipped, "fn f(){}");
        assert_eq!(a.correctness, r.correctness * 0.4);
    }

    #[test]
    fn unsafe_code_loses_security_points() {
        let clean = axes(&passed(1), &passed(1), "fn f(){}");
        let risky = axes(&passed(1), &passed(1), "fn f(){ unsafe {} }");
        assert!(clean.security > risky.security);
    }

    #[test]
    fn clippy_warnings_reduce_security() {
        let r = Rubric::default();
        let no_warn = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &passed(1),
            0,
            &StageOutcome::Skipped,
            &metrics("fn f(){}"),
            &r,
        );
        let with_warn = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &passed(1),
            5,
            &StageOutcome::Skipped,
            &metrics("fn f(){}"),
            &r,
        );
        assert!(no_warn.security > with_warn.security);
    }

    #[test]
    fn clippy_failure_reduces_security_to_half() {
        let clean = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &StageOutcome::Skipped, // clippy skipped → ratio 1.0
            0,
            &StageOutcome::Skipped,
            &metrics("fn f(){}"),
            &Rubric::default(),
        );
        let clippy_fail = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &failed(), // clippy failed → ratio 0.0
            0,
            &StageOutcome::Skipped,
            &metrics("fn f(){}"),
            &Rubric::default(),
        );
        // clippy failed → clippy_sec = 0, unsafe clean → unsafe_ratio = 1.0
        // security = rubric.security * (1.0*0.5 + 0.0*0.5) = rubric.security * 0.5
        assert!((clippy_fail.security - clean.security * 0.5).abs() < 1e-9);
    }

    #[test]
    fn wasm_pass_awards_resource_usage() {
        let r = Rubric::default();
        let with_wasm = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &StageOutcome::Skipped,
            0,
            &passed(1), // wasm passed
            &metrics("fn f(){}"),
            &r,
        );
        let no_wasm = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &StageOutcome::Skipped,
            0,
            &StageOutcome::Skipped, // wasm not run
            &metrics("fn f(){}"),
            &r,
        );
        assert_eq!(with_wasm.resource_usage, r.resource_usage);
        assert_eq!(no_wasm.resource_usage, 0.0);
    }

    #[test]
    fn performance_is_relative_fastest_wins() {
        let r = Rubric::default();
        let mk = |id: &str, c: u64, t: u64| Evaluation {
            candidate_id: id.into(),
            compile: passed(c),
            test: passed(t),
            clippy: StageOutcome::Skipped,
            clippy_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
            axes: AxisScores::default(),
            score: 0.0,
        };
        let mut evals = vec![mk("fast", 10, 10), mk("slow", 30, 30)];
        assign_performance(&mut evals, &r);
        assert_eq!(evals[0].axes.performance, r.performance);
        assert!(evals[1].axes.performance < r.performance);
        assert!((evals[1].axes.performance - r.performance / 3.0).abs() < 1e-9);
    }

    #[test]
    fn performance_zero_when_not_compiled() {
        let r = Rubric::default();
        let mut evals = vec![Evaluation {
            candidate_id: "ng".into(),
            compile: failed(),
            test: StageOutcome::Skipped,
            clippy: StageOutcome::Skipped,
            clippy_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
            axes: AxisScores::default(),
            score: 0.0,
        }];
        assign_performance(&mut evals, &r);
        assert_eq!(evals[0].axes.performance, 0.0);
    }

    #[test]
    fn rank_orders_by_score_desc() {
        let mk = |id: &str, s: f64| Evaluation {
            candidate_id: id.into(),
            compile: passed(1),
            test: passed(1),
            clippy: StageOutcome::Skipped,
            clippy_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
            axes: AxisScores::default(),
            score: s,
        };
        let ranked = rank(vec![mk("a", 25.0), mk("b", 50.0), mk("c", 0.0)]);
        let ids: Vec<_> = ranked.iter().map(|e| e.candidate_id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a", "c"]);
    }
}
