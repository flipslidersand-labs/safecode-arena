//! `evaluate_candidate` の reviewer 注入（reasoning 経路）の単体テスト。
//! compile 失敗する候補を使い、実バックエンド・環境変数に依存せず検証する。

use safecode_arena::config::Rubric;
use safecode_arena::llm::LlmReview;
use safecode_arena::model::{Candidate, Language, StageOutcome};
use safecode_arena::pipeline::{evaluate_candidate, EvalOptions};
use std::cell::Cell;
use std::time::Duration;

fn eval(opts: &EvalOptions) -> safecode_arena::model::Evaluation {
    let cand = Candidate {
        id: "c".into(),
        source: "pub fn broken(\n".into(),
        language: Language::Rust,
    };
    evaluate_candidate(&cand, Duration::from_secs(60), &Rubric::default(), opts).unwrap()
}

#[test]
fn review_success_sets_reasoning() {
    let r = |_: &str, _: bool| {
        Some(LlmReview {
            score: 0.75,
            comment: "ok".into(),
        })
    };
    let e = eval(&EvalOptions {
        run_llm_review: true,
        reviewer: Some(&r),
        ..Default::default()
    });
    assert!(matches!(e.reasoning, StageOutcome::Passed { .. }));
    assert_eq!(e.reasoning_score, 0.75);
    assert_eq!(e.reasoning_comment.as_deref(), Some("ok"));
}

#[test]
fn review_none_leaves_reasoning_skipped() {
    let r = |_: &str, _: bool| None;
    let e = eval(&EvalOptions {
        run_llm_review: true,
        reviewer: Some(&r),
        ..Default::default()
    });
    assert!(matches!(e.reasoning, StageOutcome::Skipped));
    assert_eq!(e.reasoning_score, 0.0);
    assert!(e.reasoning_comment.is_none());
}

#[test]
fn review_disabled_does_not_call_reviewer() {
    let called = Cell::new(false);
    let r = |_: &str, _: bool| {
        called.set(true);
        Some(LlmReview {
            score: 1.0,
            comment: "x".into(),
        })
    };
    let e = eval(&EvalOptions {
        run_llm_review: false,
        reviewer: Some(&r),
        ..Default::default()
    });
    assert!(!called.get());
    assert!(matches!(e.reasoning, StageOutcome::Skipped));
}
