//! 採点ルーブリック。重みは `config::Rubric`（既定値は `docs/spec.md` と一致）。
//!
//! - correctness:     コンパイル + テスト + property test 通過で実測
//! - security:        `unsafe` 使用 + lint warning 数で実測（Phase 3）
//! - maintainability: 関数長ヒューリスティック + lint warning 数（Phase 3）
//! - performance:     候補間の compile+test 時間の相対比較
//! - resource_usage:  未計測（0）。Phase 4 以降。

use crate::analysis::SourceMetrics;
use crate::config::Rubric;
use crate::model::{AxisScores, Evaluation, StageOutcome};

/// lint warning 数から security 達成率を算出する。
///
/// 0 件 = 1.0、1 件ごとに 0.1 減点。lint 自体が失敗なら 0.0 扱い。
fn lint_security_ratio(lint: &StageOutcome, warnings: usize) -> f64 {
    match lint {
        StageOutcome::Passed { .. } => (1.0 - 0.1 * warnings as f64).clamp(0.0, 1.0),
        StageOutcome::Failed { .. } => 0.0,
        _ => 1.0, // Skipped / TimedOut は減点なし（lint を実行しなかった場合）
    }
}

/// lint warning 数から maintainability 補正係数を返す。
///
/// 0 件 = 1.0、3 件ごとに 0.1 減点。
fn lint_maintainability_ratio(warnings: usize) -> f64 {
    (1.0 - 0.1 * (warnings / 3) as f64).clamp(0.0, 1.0)
}

/// performance を除く軸を採点する。performance は候補集合が揃ってから
/// [`assign_performance`] で相対的に付与する。
///
/// Phase 3 追加パラメータ:
/// - `prop_test`: property test ステージの結果（correctness に反映）
/// - `lint`: lint ステージの結果（security / maintainability に反映）
/// - `lint_warnings`: linter が報告した warning 数
///
/// Phase 4 追加パラメータ:
/// - `wasm`: Wasm サンドボックス実行の結果（resource_usage に反映）
///
/// Phase 8 追加パラメータ:
/// - `mutation_caught` / `mutation_total`: cargo mutants の結果（correctness に反映）
///   mutation が実行された場合（total > 0）、correctness の重みを再配分する:
///   compile(30%) + test(30%) + prop_test(15%) + mutation(25%)
#[allow(clippy::too_many_arguments)]
pub fn axes_without_performance(
    compile: &StageOutcome,
    test: &StageOutcome,
    prop_test: &StageOutcome,
    lint: &StageOutcome,
    lint_warnings: usize,
    wasm: &StageOutcome,
    metrics: &SourceMetrics,
    rubric: &Rubric,
    mutation_caught: usize,
    mutation_total: usize,
    audit_findings: usize,
) -> AxisScores {
    // mutation が実行された場合は重みを再配分する。
    // Skipped（total == 0）の場合は従来通り compile(40%) + test(40%) + prop_test(20%)。
    let mut correctness = if mutation_total > 0 {
        let mutation_ratio = mutation_caught as f64 / mutation_total as f64;
        let mut c = 0.0;
        if compile.is_passed() {
            c += rubric.correctness * 0.30;
        }
        if test.is_passed() {
            c += rubric.correctness * 0.30;
        }
        if prop_test.is_passed() {
            c += rubric.correctness * 0.15;
        }
        c += rubric.correctness * 0.25 * mutation_ratio;
        c
    } else {
        // correctness: compile(40%) + test(40%) + prop_test(20%)
        // prop_test が Skipped の場合は compile+test のみ（上限 80%）
        let mut c = 0.0;
        if compile.is_passed() {
            c += rubric.correctness * 0.4;
        }
        if test.is_passed() {
            c += rubric.correctness * 0.4;
        }
        if prop_test.is_passed() {
            c += rubric.correctness * 0.2;
        }
        c
    };
    let _ = &mut correctness; // suppress unused_mut warning

    // ビルドできないコードは security / maintainability を採点しない
    let (security, maintainability) = if compile.is_passed() {
        let unsafe_ratio = metrics.security_ratio();
        let lint_sec = lint_security_ratio(lint, lint_warnings);
        // audit: 脆弱性 1 件ごとに 0.25 減点、4 件以上で 0
        let audit_sec = (1.0 - 0.25 * audit_findings as f64).clamp(0.0, 1.0);
        // security = unsafe(40%) + lint(40%) + audit(20%)
        let sec = rubric.security * (unsafe_ratio * 0.4 + lint_sec * 0.4 + audit_sec * 0.2);

        let heuristic_maint = metrics.maintainability_ratio();
        let lint_maint = lint_maintainability_ratio(lint_warnings);
        // maintainability = 関数長ヒューリスティック(60%) + lint(40%)
        let maint = rubric.maintainability * (heuristic_maint * 0.6 + lint_maint * 0.4);
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

/// 候補集合の performance 軸を付与する。
///
/// **優先**: compile した全候補が `bench_ns`（Criterion 実測値）を持つ場合は
/// それを相対比較する（小さいほど速い = 高スコア）。
///
/// **フォールバック**: bench_ns が揃わない場合は compile+test 所要時間で比較。
///
/// 付与後、各 `score` を `axes.total()` で再計算する。
pub fn assign_performance(evals: &mut [Evaluation], rubric: &Rubric) {
    let all_have_bench = !evals.is_empty()
        && evals
            .iter()
            .filter(|e| e.compile.is_passed())
            .all(|e| e.bench_ns.is_some());

    if all_have_bench {
        let min_ns = evals.iter().filter_map(|e| e.bench_ns).min();
        for e in evals.iter_mut() {
            let ratio = match (e.bench_ns, min_ns) {
                (Some(t), Some(min)) if t > 0 => min as f64 / t as f64,
                (Some(_), Some(_)) => 1.0,
                _ => 0.0,
            };
            e.axes.performance = rubric.performance * ratio;
            e.score = e.axes.total();
        }
    } else {
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
                (Some(_), Some(_)) => 1.0,
                _ => 0.0,
            };
            e.axes.performance = rubric.performance * ratio;
            e.score = e.axes.total();
        }
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
            0,
            0,
            0,
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
            0,
            0,
            0,
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
    fn lint_warnings_reduce_security() {
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
            0,
            0,
            0,
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
            0,
            0,
            0,
        );
        assert!(no_warn.security > with_warn.security);
    }

    #[test]
    fn lint_failure_reduces_security_to_half() {
        let clean = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &StageOutcome::Skipped, // lint skipped → ratio 1.0
            0,
            &StageOutcome::Skipped,
            &metrics("fn f(){}"),
            &Rubric::default(),
            0,
            0,
            0,
        );
        let lint_fail = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &failed(), // lint failed → ratio 0.0
            0,
            &StageOutcome::Skipped,
            &metrics("fn f(){}"),
            &Rubric::default(),
            0,
            0,
            0,
        );
        // lint failed → lint_sec = 0, unsafe clean → unsafe_ratio = 1.0, audit_sec = 1.0
        // security = rubric.security * (1.0*0.4 + 0.0*0.4 + 1.0*0.2) = rubric.security * 0.6
        assert!((lint_fail.security - clean.security * 0.6).abs() < 1e-9);
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
            0,
            0,
            0,
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
            0,
            0,
            0,
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
            lint: StageOutcome::Skipped,
            lint_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
            mutation: StageOutcome::Skipped,
            mutation_caught: 0,
            mutation_total: 0,
            audit_findings: 0,
            bench_ns: None,
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
            lint: StageOutcome::Skipped,
            lint_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
            mutation: StageOutcome::Skipped,
            mutation_caught: 0,
            mutation_total: 0,
            audit_findings: 0,
            bench_ns: None,
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
            lint: StageOutcome::Skipped,
            lint_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
            mutation: StageOutcome::Skipped,
            mutation_caught: 0,
            mutation_total: 0,
            audit_findings: 0,
            bench_ns: None,
            axes: AxisScores::default(),
            score: s,
        };
        let ranked = rank(vec![mk("a", 25.0), mk("b", 50.0), mk("c", 0.0)]);
        let ids: Vec<_> = ranked.iter().map(|e| e.candidate_id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a", "c"]);
    }

    // ── Phase 8: mutation testing ─────────────────────────────────────────────

    #[test]
    fn mutation_skipped_gives_same_as_before() {
        // mutation_total == 0 → 従来の重み (compile 40% + test 40%)
        let r = Rubric::default();
        let a = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &StageOutcome::Skipped,
            0,
            &StageOutcome::Skipped,
            &metrics("fn f(){}"),
            &r,
            0, // mutation_caught
            0, // mutation_total → Skipped
            0,
        );
        assert_eq!(a.correctness, r.correctness * 0.8);
    }

    #[test]
    fn mutation_full_score_boosts_correctness() {
        // mutation 全検出 (caught == total) → compile(30%) + test(30%) + mutation(25%) = 85%
        let r = Rubric::default();
        let a = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &StageOutcome::Skipped,
            0,
            &StageOutcome::Skipped,
            &metrics("fn f(){}"),
            &r,
            12, // mutation_caught
            12, // mutation_total
            0,
        );
        assert!((a.correctness - r.correctness * 0.85).abs() < 1e-9);
    }

    #[test]
    fn mutation_zero_score_reduces_correctness() {
        // mutation 0 件検出 → compile(30%) + test(30%) + mutation(0%) = 60%
        let r = Rubric::default();
        let a = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &StageOutcome::Skipped,
            0,
            &StageOutcome::Skipped,
            &metrics("fn f(){}"),
            &r,
            0,  // mutation_caught
            12, // mutation_total → 0%
            0,
        );
        assert!((a.correctness - r.correctness * 0.60).abs() < 1e-9);
    }

    #[test]
    fn mutation_partial_score_is_proportional() {
        // 9/12 = 75% → compile(30%) + test(30%) + mutation(25% * 0.75) = 78.75%
        let r = Rubric::default();
        let a = axes_without_performance(
            &passed(1),
            &passed(1),
            &StageOutcome::Skipped,
            &StageOutcome::Skipped,
            0,
            &StageOutcome::Skipped,
            &metrics("fn f(){}"),
            &r,
            9,  // mutation_caught
            12, // mutation_total
            0,
        );
        let expected = r.correctness * (0.30 + 0.30 + 0.25 * 0.75);
        assert!((a.correctness - expected).abs() < 1e-9);
    }

    // ── Phase 10: Criterion benchmark ────────────────────────────────────────

    #[test]
    fn bench_ns_used_for_performance_when_all_have_it() {
        let r = Rubric::default();
        let mk = |id: &str, ns: u64| Evaluation {
            candidate_id: id.into(),
            compile: passed(1),
            test: passed(1),
            lint: StageOutcome::Skipped,
            lint_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
            mutation: StageOutcome::Skipped,
            mutation_caught: 0,
            mutation_total: 0,
            audit_findings: 0,
            bench_ns: Some(ns),
            axes: AxisScores::default(),
            score: 0.0,
        };
        let mut evals = vec![mk("fast", 1_000), mk("slow", 4_000)];
        assign_performance(&mut evals, &r);
        assert_eq!(evals[0].axes.performance, r.performance);
        assert!((evals[1].axes.performance - r.performance * 0.25).abs() < 1e-9);
    }

    #[test]
    fn fallback_to_time_when_bench_ns_missing() {
        let r = Rubric::default();
        let mk = |id: &str, c: u64, t: u64| Evaluation {
            candidate_id: id.into(),
            compile: passed(c),
            test: passed(t),
            lint: StageOutcome::Skipped,
            lint_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
            mutation: StageOutcome::Skipped,
            mutation_caught: 0,
            mutation_total: 0,
            audit_findings: 0,
            bench_ns: None,
            axes: AxisScores::default(),
            score: 0.0,
        };
        let mut evals = vec![mk("fast", 10, 10), mk("slow", 30, 30)];
        assign_performance(&mut evals, &r);
        assert_eq!(evals[0].axes.performance, r.performance);
        assert!((evals[1].axes.performance - r.performance / 3.0).abs() < 1e-9);
    }
}
