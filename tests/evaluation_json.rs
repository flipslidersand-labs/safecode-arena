//! `Evaluation` の JSON 形式（キー名と順序）を固定する。
//! フィールドをサブ構造へ移す際（`#[serde(flatten)]` 等）のキー重複・rename 漏れ検出用。

use safecode_arena::model::{Evaluation, StageOutcome};

#[test]
fn evaluation_json_keys_and_order_are_stable() {
    let json = serde_json::to_string(&Evaluation::for_test("c1")).unwrap();
    let expected = concat!(
        r#"{"candidate_id":"c1","#,
        r#""compile":{"Passed":{"duration_ms":1}},"#,
        r#""test":{"Passed":{"duration_ms":1}},"#,
        r#""lint":"Skipped","lint_warnings":0,"#,
        r#""prop_test":"Skipped","#,
        r#""wasm":"Skipped","wasm_fuel_used":null,"#,
        r#""mutation":"Skipped","mutation_caught":0,"mutation_total":0,"#,
        r#""audit_findings":0,"bench_ns":null,"#,
        r#""reasoning":"Skipped","reasoning_score":0.0,"reasoning_comment":null,"#,
        r#""axes":{"correctness":0.0,"security":0.0,"performance":0.0,"maintainability":0.0,"resource_usage":0.0,"reasoning":0.0},"#,
        r#""score":0.0}"#,
    );
    assert_eq!(json, expected);
}

#[test]
fn evaluation_json_roundtrips() {
    let e = Evaluation {
        compile: StageOutcome::Skipped,
        score: 12.5,
        ..Evaluation::for_test("c2")
    };
    let back: Evaluation = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
    assert_eq!(back.candidate_id, "c2");
    assert_eq!(back.score, 12.5);
}
