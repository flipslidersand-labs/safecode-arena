use safecode_arena::model::{AxisScores, Evaluation};
use safecode_arena::session::{check_and_persist, SaveStatus};
use safecode_arena::store::Store;

fn eval(id: &str, correctness: f64) -> Evaluation {
    Evaluation {
        axes: AxisScores {
            correctness,
            ..AxisScores::default()
        },
        score: correctness,
        ..Evaluation::for_test(id)
    }
}

/// `Store::open_in_memory` は `cfg(test)` 限定で統合テストから使えないため、tmp の SQLite ファイルを使う。
struct TmpStore {
    store: Store,
    path: std::path::PathBuf,
}

static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl TmpStore {
    fn new() -> Self {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("safecode-regression-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path).unwrap();
        TmpStore { store, path }
    }
}

impl std::ops::Deref for TmpStore {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.store
    }
}

impl std::ops::DerefMut for TmpStore {
    fn deref_mut(&mut self) -> &mut Store {
        &mut self.store
    }
}

impl Drop for TmpStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

const T: &str = "2026-01-01 00:00:00";

#[test]
fn first_run_has_no_baseline_and_is_saved() {
    let mut s = TmpStore::new();
    let out = check_and_persist(&mut s, &[eval("a", 80.0)], &[], 0.0, "k", T).unwrap();
    assert!(out.baseline_run_id.is_none());
    assert!(!out.skipped_incomparable);
    assert!(!out.has_regression());
    assert!(matches!(out.save, SaveStatus::Saved(_)));
    assert!(s.latest_run_id().unwrap().is_some());
}

#[test]
fn no_regression_saves_new_run() {
    let mut s = TmpStore::new();
    check_and_persist(&mut s, &[eval("a", 80.0)], &[], 0.0, "k", T).unwrap();
    let out = check_and_persist(&mut s, &[eval("a", 85.0)], &[], 0.0, "k", T).unwrap();
    assert!(out.baseline_run_id.is_some());
    assert!(!out.has_regression());
    assert!(matches!(out.save, SaveStatus::Saved(id) if Some(id) == s.latest_run_id().unwrap()));
}

#[test]
fn regression_is_reported_and_not_saved() {
    let mut s = TmpStore::new();
    check_and_persist(&mut s, &[eval("a", 80.0)], &[], 5.0, "k", T).unwrap();
    let base = s.latest_run_id().unwrap();
    let out = check_and_persist(&mut s, &[eval("a", 60.0)], &[], 5.0, "k", T).unwrap();
    assert_eq!(out.regressions.len(), 1);
    assert_eq!(out.regressions[0].candidate_id, "a");
    assert!(out.has_regression());
    assert!(matches!(out.save, SaveStatus::SkippedDueToRegression));
    assert_eq!(s.latest_run_id().unwrap(), base);
}

#[test]
fn dropped_candidate_counts_as_regression() {
    let mut s = TmpStore::new();
    check_and_persist(
        &mut s,
        &[eval("a", 80.0), eval("b", 70.0)],
        &[],
        0.0,
        "k",
        T,
    )
    .unwrap();
    let out =
        check_and_persist(&mut s, &[eval("a", 80.0)], &["b".to_string()], 0.0, "k", T).unwrap();
    assert_eq!(out.dropped, vec!["b".to_string()]);
    assert!(matches!(out.save, SaveStatus::SkippedDueToRegression));
}

#[test]
fn different_options_are_not_compared() {
    let mut s = TmpStore::new();
    check_and_persist(&mut s, &[eval("a", 80.0)], &[], 0.0, "k1", T).unwrap();
    let out = check_and_persist(&mut s, &[eval("a", 10.0)], &[], 0.0, "k2", T).unwrap();
    assert!(out.skipped_incomparable);
    assert!(out.baseline_run_id.is_none());
    assert!(!out.has_regression());
    assert!(matches!(out.save, SaveStatus::Saved(_)));
}
