use safecode_arena::config::Rubric;
use safecode_arena::model::{AxisScores, Evaluation, StageOutcome};
use safecode_arena::scoring::{assign_performance, assign_performance_each};
use safecode_arena::session::{finalize, keep_evaluated};

fn passed(ms: u64) -> StageOutcome {
    StageOutcome::Passed { duration_ms: ms }
}

fn eval(id: &str, ms: u64, correctness: f64) -> Evaluation {
    let axes = AxisScores {
        correctness,
        ..AxisScores::default()
    };
    Evaluation {
        compile: passed(ms),
        axes,
        score: axes.total(),
        ..Evaluation::for_test(id)
    }
}

fn ids(v: &[Evaluation]) -> Vec<&str> {
    v.iter().map(|e| e.candidate_id.as_str()).collect()
}

#[test]
fn performance_same_rubric_matches_each() {
    let rubric = Rubric::default();
    let mk = || {
        vec![
            eval("a", 100, 10.0),
            eval("b", 200, 10.0),
            eval("c", 400, 10.0),
        ]
    };
    let mut single = mk();
    assign_performance(&mut single, &rubric);
    let mut each = mk();
    assign_performance_each(&mut each, &[rubric; 3]);
    for (s, e) in single.iter().zip(&each) {
        assert_eq!(s.axes.performance, e.axes.performance);
    }
    assert!(single[0].axes.performance > single[2].axes.performance);
}

#[test]
fn finalize_ranks_by_score_with_performance() {
    let rubric = Rubric::default();
    // 同じ正答点なら速い候補が上に来る。
    let out = finalize(
        vec![eval("slow", 400, 10.0), eval("fast", 100, 10.0)],
        &[rubric, rubric],
    );
    assert_eq!(ids(&out), ["fast", "slow"]);
    assert!(out[0].axes.performance > out[1].axes.performance);
    assert_eq!(out[0].reasoning_comment, None);
}

#[test]
fn finalize_assigns_reasoning_to_reviewed_only() {
    let rubric = Rubric {
        reasoning: 10.0,
        ..Rubric::default()
    };
    let mut reviewed = eval("reviewed", 100, 10.0);
    reviewed.reasoning = passed(1);
    reviewed.reasoning_score = 0.5;
    let plain = eval("plain", 100, 10.0);
    let out = finalize(vec![plain, reviewed], &[rubric, rubric]);
    let r = out.iter().find(|e| e.candidate_id == "reviewed").unwrap();
    assert_eq!(r.axes.reasoning, 5.0);
    let p = out.iter().find(|e| e.candidate_id == "plain").unwrap();
    assert_eq!(p.axes.reasoning, 0.0);
}

#[test]
#[should_panic]
fn finalize_rejects_length_mismatch() {
    finalize(vec![eval("a", 1, 1.0)], &[]);
}

#[test]
fn keep_evaluated_passes_ok_and_drops_err() {
    assert_eq!(keep_evaluated("a", Ok(1)), Some(1));
    assert_eq!(keep_evaluated::<i32>("b", Err(anyhow::anyhow!("io"))), None);
}
