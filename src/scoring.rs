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
        reasoning: 0.0, // assign_reasoning() で後から付与
    }
}

/// LLM レビュースコアから reasoning 軸を付与する。
///
/// `llm_score`: 0.0–1.0。`rubric.reasoning` が 0 のときは何もしない。
/// 付与後、`score` を `axes.total()` で再計算する。
pub fn assign_reasoning(eval: &mut Evaluation, llm_score: f64, rubric: &Rubric) {
    if rubric.reasoning <= 0.0 {
        return;
    }
    eval.axes.reasoning = rubric.reasoning * llm_score.clamp(0.0, 1.0);
    eval.score = eval.axes.total();
}

/// 候補集合の compile+test 所要時間を相対比較し、performance 軸を付与する。
/// 最速候補に満点、それ以外は `min_time / own_time` で按分。
/// compile か test が通っていない候補は performance 0。
/// 付与後、各 `score` を `axes.total()` で再計算する。
pub fn assign_performance(evals: &mut [Evaluation], rubric: &Rubric) {
    let rubrics = vec![*rubric; evals.len()];
    assign_performance_each(evals, &rubrics);
}

/// `assign_performance` の候補ごと rubric 版。`rubrics[i]` が `evals[i]` の performance 配点。
/// 相対比較（最速との比）は全候補で行い、比率に候補ごとの `rubric.performance` を掛ける。
pub fn assign_performance_each(evals: &mut [Evaluation], rubrics: &[Rubric]) {
    assert_eq!(
        evals.len(),
        rubrics.len(),
        "evals と rubrics の長さが不一致"
    );
    // bench_ns が 1 件でもあれば全候補で bench_ns を優先する。
    // bench_ns がない候補は 0 扱い（最遅）ではなく None 扱い（採点対象外）。
    let has_any_bench = evals.iter().any(|e| e.bench_ns.is_some());
    let times: Vec<Option<u64>> = evals
        .iter()
        .map(|e| {
            if has_any_bench {
                // compile/test が通っていない候補は bench が速くても採点対象外
                if e.compile.is_passed() && e.test.is_passed() {
                    e.bench_ns
                } else {
                    None
                }
            } else {
                match (e.compile.duration_ms(), e.test.duration_ms()) {
                    (Some(c), Some(t)) => Some(c + t),
                    _ => None,
                }
            }
        })
        .collect();

    let fastest = times.iter().flatten().copied().min();

    for ((e, time), rubric) in evals.iter_mut().zip(times.iter()).zip(rubrics) {
        let ratio = match (time, fastest) {
            (Some(t), Some(min)) if *t > 0 => min as f64 / *t as f64,
            (Some(_), Some(_)) => 1.0, // 0ns/ms は満点扱い
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
            reasoning: StageOutcome::Skipped,
            reasoning_score: 0.0,
            reasoning_comment: None,
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
            reasoning: StageOutcome::Skipped,
            reasoning_score: 0.0,
            reasoning_comment: None,
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
            reasoning: StageOutcome::Skipped,
            reasoning_score: 0.0,
            reasoning_comment: None,
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

    #[test]
    fn bench_ns_preferred_over_compile_time() {
        let r = Rubric::default();
        let mk = |id: &str, bench: Option<u64>, c: u64, t: u64| {
            let mut e = Evaluation {
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
                bench_ns: bench,
                reasoning: StageOutcome::Skipped,
                reasoning_score: 0.0,
                reasoning_comment: None,
                axes: AxisScores::default(),
                score: 0.0,
            };
            e.axes = AxisScores::default();
            e
        };
        // fast: bench=100ns, slow compile; slow: bench=2000ns, fast compile
        let mut evals = vec![
            mk("fast", Some(100), 500, 500),
            mk("slow", Some(2000), 10, 10),
        ];
        assign_performance(&mut evals, &r);
        let fast = evals.iter().find(|e| e.candidate_id == "fast").unwrap();
        let slow = evals.iter().find(|e| e.candidate_id == "slow").unwrap();
        // bench_ns を使うので fast が高スコア
        assert!(fast.axes.performance > slow.axes.performance);
    }

    #[test]
    fn bench_path_ignores_candidates_that_failed_test() {
        let r = Rubric::default();
        let mk = |id: &str, bench: u64, test: StageOutcome| Evaluation {
            candidate_id: id.into(),
            compile: passed(1),
            test,
            lint: StageOutcome::Skipped,
            lint_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
            mutation: StageOutcome::Skipped,
            mutation_caught: 0,
            mutation_total: 0,
            audit_findings: 0,
            bench_ns: Some(bench),
            reasoning: StageOutcome::Skipped,
            reasoning_score: 0.0,
            reasoning_comment: None,
            axes: AxisScores::default(),
            score: 0.0,
        };
        // broken: テスト失敗だが bench は最速。ok: テスト成功。
        let mut evals = vec![mk("broken", 10, failed()), mk("ok", 1000, passed(1))];
        assign_performance(&mut evals, &r);
        assert_eq!(evals[0].axes.performance, 0.0);
        // ok が唯一の採点対象なので満点
        assert_eq!(evals[1].axes.performance, r.performance);
    }

    fn perf_eval(ms: u64) -> Evaluation {
        Evaluation {
            candidate_id: "c".into(),
            compile: passed(ms),
            test: passed(0),
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
            reasoning: StageOutcome::Skipped,
            reasoning_score: 0.0,
            reasoning_comment: None,
            axes: AxisScores::default(),
            score: 0.0,
        }
    }

    #[test]
    fn performance_each_uses_per_candidate_weight() {
        let mk = perf_eval;
        let mut evals = vec![mk(10), mk(20)];
        let a = Rubric {
            performance: 30.0,
            ..Rubric::default()
        };
        let b = Rubric {
            performance: 10.0,
            ..Rubric::default()
        };
        assign_performance_each(&mut evals, &[a, b]);
        assert_eq!(evals[0].axes.performance, 30.0);
        assert_eq!(evals[1].axes.performance, 5.0); // 10 * (10/20)
    }

    #[test]
    fn performance_single_rubric_matches_each() {
        let mk = perf_eval;
        let r = Rubric::default();
        let mut x = vec![mk(10), mk(40)];
        let mut y = vec![mk(10), mk(40)];
        assign_performance(&mut x, &r);
        assign_performance_each(&mut y, &[r, r]);
        assert_eq!(x[1].axes.performance, y[1].axes.performance);
        assert_eq!(x[1].score, y[1].score);
    }

    fn reasoning_rubric(w: f64) -> Rubric {
        Rubric {
            reasoning: w,
            ..Rubric::default()
        }
    }

    #[test]
    fn reasoning_zero_weight_leaves_eval_unchanged() {
        let mut e = perf_eval(1);
        e.axes.correctness = 10.0;
        e.axes.reasoning = 3.0;
        e.score = 99.0; // total() とは異なる値: 再計算されないことを確認
        assign_reasoning(&mut e, 1.0, &reasoning_rubric(0.0));
        assert_eq!(e.axes.reasoning, 3.0);
        assert_eq!(e.score, 99.0);
    }

    #[test]
    fn reasoning_scales_by_weight_and_recomputes_score() {
        let mut e = perf_eval(1);
        e.axes.correctness = 10.0;
        assign_reasoning(&mut e, 0.5, &reasoning_rubric(10.0));
        assert_eq!(e.axes.reasoning, 5.0);
        assert_eq!(e.score, e.axes.total());
        assert_eq!(e.score, 15.0);
    }

    #[test]
    fn reasoning_llm_score_is_clamped_to_unit_range() {
        let r = reasoning_rubric(10.0);
        let mut hi = perf_eval(1);
        assign_reasoning(&mut hi, 2.0, &r);
        assert_eq!(hi.axes.reasoning, 10.0);
        let mut lo = perf_eval(1);
        assign_reasoning(&mut lo, -1.0, &r);
        assert_eq!(lo.axes.reasoning, 0.0);
    }

    #[test]
    fn reasoning_nan_llm_score_propagates_nan() {
        // 現行仕様の固定: f64::clamp は NaN をそのまま返すため、NaN の
        // llm_score は reasoning と score を NaN にする（0 や上限には丸められない）。
        // 変更する場合は呼び出し側での NaN 除外とあわせて意図的に更新すること。
        let mut e = perf_eval(1);
        assign_reasoning(&mut e, f64::NAN, &reasoning_rubric(10.0));
        assert!(e.axes.reasoning.is_nan());
        assert!(e.score.is_nan());
    }
}
