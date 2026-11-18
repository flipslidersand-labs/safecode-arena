use safecode_arena::config::Config;
use safecode_arena::config::Rubric;
use safecode_arena::model::Candidate;
use safecode_arena::model::Language;
use safecode_arena::pipeline::{EvalOptions, WasmOptions};
use safecode_arena::session::{conditions_key, run_options_key, RunConditions};
use safecode_arena::store::Store;
use std::path::Path;

fn base() -> RunConditions<'static> {
    RunConditions {
        llm_review: false,
        mutation: false,
        wasm_entry: None,
        tests: true,
        prop_tests: false,
        wasm_fuel: 100_000_000,
        wasm_max_memory_mb: 64,
        rubrics: vec![("rust", Rubric::default())],
        languages: vec![Language::Rust],
    }
}

#[test]
fn key_changes_with_wasm_limits() {
    let k = conditions_key(&base());
    let mut c = base();
    c.wasm_fuel = 1;
    assert_ne!(conditions_key(&c), k);
    let mut c = base();
    c.wasm_max_memory_mb = 1;
    assert_ne!(conditions_key(&c), k);
}

#[test]
fn key_is_stable_for_same_conditions() {
    assert_eq!(conditions_key(&base()), conditions_key(&base()));
}

#[test]
fn key_changes_with_prop_tests_tests_and_weights() {
    let k = conditions_key(&base());
    let mut c = base();
    c.prop_tests = true;
    assert_ne!(conditions_key(&c), k);
    let mut c = base();
    c.tests = false;
    assert_ne!(conditions_key(&c), k);
    let mut c = base();
    c.rubrics = vec![(
        "rust",
        Rubric {
            correctness: 40.0,
            security: 30.0,
            ..Rubric::default()
        },
    )];
    assert_ne!(conditions_key(&c), k);
    let mut c = base();
    c.rubrics = vec![("go", Rubric::default())];
    assert_ne!(conditions_key(&c), k);
}

#[test]
fn key_ignores_candidate_order_and_duplicates() {
    let mut a = base();
    a.rubrics = vec![("rust", Rubric::default()), ("go", Rubric::default())];
    let mut b = base();
    b.rubrics = vec![
        ("go", Rubric::default()),
        ("rust", Rubric::default()),
        ("rust", Rubric::default()),
    ];
    assert_eq!(conditions_key(&a), conditions_key(&b));
}

#[test]
fn run_with_other_prop_tests_condition_is_not_a_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("t.db")).unwrap();
    let mut with_prop = base();
    with_prop.prop_tests = true;
    let k_prop = conditions_key(&with_prop);
    let k_plain = conditions_key(&base());
    let id = s.save_run("t1", &k_prop, &[]).unwrap();
    assert_eq!(s.latest_run_id_matching(&k_prop).unwrap(), Some(id));
    assert_eq!(s.latest_run_id_matching(&k_plain).unwrap(), None);
}

#[test]
fn rust_key_is_unchanged_format() {
    let mut c = base();
    c.mutation = true;
    c.wasm_entry = Some("run");
    c.prop_tests = true;
    assert_eq!(
            conditions_key(&c),
            format!(
                "llm_review=false;mutation=true;wasm_entry=run;tests=true;prop_tests=true;wasm_fuel=100000000;wasm_max_memory_mb=64;rubric=rust:{}",
                {
                    let r = Rubric::default();
                    format!(
                        "{},{},{},{},{},{}",
                        r.correctness, r.security, r.performance, r.maintainability, r.resource_usage, r.reasoning
                    )
                }
            )
        );
}

#[test]
fn key_ignores_options_unsupported_by_all_languages() {
    let mut plain = base();
    plain.languages = vec![Language::JavaScript];
    let mut loud = base();
    loud.languages = vec![Language::JavaScript];
    loud.mutation = true;
    loud.prop_tests = true;
    loud.wasm_entry = Some("run");
    loud.wasm_fuel = 1;
    loud.wasm_max_memory_mb = 1;
    assert_eq!(conditions_key(&plain), conditions_key(&loud));
}

#[test]
fn key_keeps_mutation_for_python_but_not_wasm() {
    let mut a = base();
    a.languages = vec![Language::Python];
    let k = conditions_key(&a);
    a.mutation = true;
    assert_ne!(conditions_key(&a), k);
    let mut b = base();
    b.languages = vec![Language::Python];
    b.wasm_entry = Some("run");
    assert_eq!(conditions_key(&b), k);
}

#[test]
fn mixed_run_with_rust_keeps_rust_options() {
    let mut a = base();
    a.languages = vec![Language::JavaScript, Language::Rust];
    let k = conditions_key(&a);
    a.wasm_entry = Some("run");
    assert_ne!(conditions_key(&a), k);
}

#[test]
fn derived_key_matches_manual_conditions() {
    let cfg = Config::default();
    let cands = vec![
        Candidate {
            id: "a".into(),
            source: String::new(),
            language: Language::Rust,
        },
        Candidate {
            id: "b".into(),
            source: String::new(),
            language: Language::Python,
        },
    ];
    let opts = EvalOptions {
        tests_dir: Some(Path::new("t")),
        prop_tests_dir: None,
        wasm_opts: WasmOptions {
            entry: Some("run"),
            fuel: 7,
            max_memory_bytes: 3 * 1024 * 1024,
        },
        run_mutation: true,
        run_llm_review: false,
        reviewer: None,
    };
    let manual = conditions_key(&RunConditions {
        llm_review: false,
        mutation: true,
        wasm_entry: Some("run"),
        tests: true,
        prop_tests: false,
        wasm_fuel: 7,
        wasm_max_memory_mb: 3,
        rubrics: cands
            .iter()
            .map(|c| (c.language.name(), cfg.rubric_for(c.language.name())))
            .collect(),
        languages: vec![Language::Rust, Language::Python],
    });
    assert_eq!(run_options_key(&opts, &cands, &cfg), manual);
}
