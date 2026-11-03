//! 評価結果の永続化（SQLite）と run 間のリグレッション検出。
//!
//! - 各 `evaluate` 実行を 1 つの run として保存する。
//! - 同じ候補 ID のスコアが過去 run より下がっていれば「リグレッション」として検出する。
//!   比較に使うのは performance 軸を除いた絶対評価軸の合計（`comparable_score`）。
//!   performance は run 内の最速候補に対する相対値で run 間比較できないため除外する。

use crate::model::{AxisScores, Evaluation};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;

/// run の要約（一覧表示用）。
#[derive(Debug, Clone)]
pub struct RunSummary {
    pub id: i64,
    pub created_at: String,
    pub candidate_count: usize,
    pub best_candidate: Option<String>,
    pub best_score: Option<f64>,
}

/// 候補スコアの後退。
#[derive(Debug, Clone)]
pub struct Regression {
    pub candidate_id: String,
    pub previous: f64,
    pub current: f64,
}

impl Regression {
    pub fn delta(&self) -> f64 {
        self.current - self.previous
    }
}

/// run 間比較に使うスコア。相対評価の performance 軸を除いた軸の合計。
pub fn comparable_score(axes: &AxisScores) -> f64 {
    axes.total() - axes.performance
}

/// 保存済み detail_json から比較用スコアを計算する。
/// `axes` を持たない古い行は計算不能として None（比較対象外）。
fn comparable_from_detail(detail_json: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(detail_json).ok()?;
    let axes = v.get("axes")?.as_object()?;
    let get = |k: &str| axes.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    Some(
        get("correctness")
            + get("security")
            + get("maintainability")
            + get("resource_usage")
            + get("reasoning"),
    )
}

/// SQLite を裏に持つ評価ストア。
pub struct Store {
    conn: Connection,
}

impl Store {
    /// DB を開く（なければ作成）。スキーマを初期化する。
    pub fn open(path: &Path) -> Result<Store> {
        let conn = Connection::open(path)
            .with_context(|| format!("DB を開けません: {}", path.display()))?;
        Self::from_conn(conn)
    }

    /// テスト用のインメモリ DB。
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Store> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Store> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS runs (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 created_at  TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS evaluations (
                 id            INTEGER PRIMARY KEY AUTOINCREMENT,
                 run_id        INTEGER NOT NULL REFERENCES runs(id),
                 candidate_id  TEXT NOT NULL,
                 score         REAL NOT NULL,
                 detail_json   TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_eval_run ON evaluations(run_id);",
        )
        .context("スキーマ初期化に失敗")?;
        Ok(Store { conn })
    }

    /// 1 つの run を保存し、その run_id を返す。
    pub fn save_run(&mut self, created_at: &str, evals: &[Evaluation]) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute("INSERT INTO runs (created_at) VALUES (?1)", [created_at])?;
        let run_id = tx.last_insert_rowid();
        for e in evals {
            let detail = serde_json::to_string(e)?;
            tx.execute(
                "INSERT INTO evaluations (run_id, candidate_id, score, detail_json)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![run_id, e.candidate_id, e.score, detail],
            )?;
        }
        tx.commit()?;
        Ok(run_id)
    }

    /// 直近（最新）の run_id。run が無ければ None。
    pub fn latest_run_id(&self) -> Result<Option<i64>> {
        let id = self.conn.query_row("SELECT MAX(id) FROM runs", [], |r| {
            r.get::<_, Option<i64>>(0)
        })?;
        Ok(id)
    }

    /// 指定 run の候補 ID → スコアの対応。
    pub fn run_scores(&self, run_id: i64) -> Result<HashMap<String, f64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT candidate_id, score FROM evaluations WHERE run_id = ?1")?;
        let rows = stmt.query_map([run_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, score) = row?;
            map.insert(id, score);
        }
        Ok(map)
    }

    /// 指定 run の候補 ID → 比較用スコア（performance 除く）。
    /// detail_json に axes が無く計算できない行は含めない（比較対象外）。
    pub fn run_comparable_scores(&self, run_id: i64) -> Result<HashMap<String, f64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT candidate_id, detail_json FROM evaluations WHERE run_id = ?1")?;
        let rows = stmt.query_map([run_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, detail) = row?;
            if let Some(s) = comparable_from_detail(&detail) {
                map.insert(id, s);
            }
        }
        Ok(map)
    }

    /// 全 run の要約（新しい順）。
    pub fn list_runs(&self) -> Result<Vec<RunSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.created_at,
                    COUNT(e.id),
                    (SELECT candidate_id FROM evaluations WHERE run_id = r.id
                       ORDER BY score DESC LIMIT 1),
                    MAX(e.score)
             FROM runs r LEFT JOIN evaluations e ON e.run_id = r.id
             GROUP BY r.id ORDER BY r.id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(RunSummary {
                id: r.get(0)?,
                created_at: r.get(1)?,
                candidate_count: r.get::<_, i64>(2)? as usize,
                best_candidate: r.get(3)?,
                best_score: r.get(4)?,
            })
        })?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }
}

/// 過去の比較用スコア（`run_comparable_scores`）と現在の評価を突き合わせ、下がった候補を返す。
/// 現在側も performance を除いた `comparable_score` で比べる。
/// `threshold` 以上下落したものだけを対象にする（誤差・揺らぎ除け）。
pub fn find_regressions(
    previous: &HashMap<String, f64>,
    current: &[Evaluation],
    threshold: f64,
) -> Vec<Regression> {
    let mut out = Vec::new();
    for e in current {
        if let Some(&prev) = previous.get(&e.candidate_id) {
            let cur = comparable_score(&e.axes);
            if prev - cur >= threshold {
                out.push(Regression {
                    candidate_id: e.candidate_id.clone(),
                    previous: prev,
                    current: cur,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AxisScores, Evaluation, StageOutcome};

    /// correctness 軸に score を置いた評価（performance は 0）。
    fn eval(id: &str, score: f64) -> Evaluation {
        let mut e = eval_raw(id, score);
        e.axes.correctness = score;
        e
    }

    fn eval_axes(id: &str, correctness: f64, performance: f64) -> Evaluation {
        let mut e = eval_raw(id, correctness + performance);
        e.axes.correctness = correctness;
        e.axes.performance = performance;
        e
    }

    fn eval_raw(id: &str, score: f64) -> Evaluation {
        Evaluation {
            candidate_id: id.into(),
            compile: StageOutcome::Passed { duration_ms: 1 },
            test: StageOutcome::Passed { duration_ms: 1 },
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
            score,
        }
    }

    #[test]
    fn save_and_read_back_run() {
        let mut s = Store::open_in_memory().unwrap();
        assert_eq!(s.latest_run_id().unwrap(), None);

        let run_id = s
            .save_run("2026-06-22 00:00:00", &[eval("a", 80.0), eval("b", 60.0)])
            .unwrap();
        assert_eq!(s.latest_run_id().unwrap(), Some(run_id));

        let scores = s.run_scores(run_id).unwrap();
        assert_eq!(scores.get("a"), Some(&80.0));
        assert_eq!(scores.get("b"), Some(&60.0));
    }

    #[test]
    fn list_runs_reports_best_candidate() {
        let mut s = Store::open_in_memory().unwrap();
        s.save_run("t1", &[eval("a", 80.0), eval("b", 90.0)])
            .unwrap();
        let runs = s.list_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].candidate_count, 2);
        assert_eq!(runs[0].best_candidate.as_deref(), Some("b"));
        assert_eq!(runs[0].best_score, Some(90.0));
    }

    #[test]
    fn detects_score_regression() {
        let mut prev = HashMap::new();
        prev.insert("a".to_string(), 85.0);
        prev.insert("b".to_string(), 50.0);

        // a は下落、b は上昇、c は新規 → a のみリグレッション
        let curr = vec![eval("a", 70.0), eval("b", 60.0), eval("c", 40.0)];
        let regs = find_regressions(&prev, &curr, 1.0);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].candidate_id, "a");
        assert_eq!(regs[0].delta(), -15.0);
    }

    #[test]
    fn ignores_regression_below_threshold() {
        let mut prev = HashMap::new();
        prev.insert("a".to_string(), 85.0);
        let curr = vec![eval("a", 84.9)];
        assert!(find_regressions(&prev, &curr, 1.0).is_empty());
    }

    #[test]
    fn performance_drop_alone_is_not_regression() {
        // 前 run: performance 20 / 今 run: 速い候補が増えて performance 5
        let mut s = Store::open_in_memory().unwrap();
        let run = s.save_run("t", &[eval_axes("a", 60.0, 20.0)]).unwrap();
        let prev = s.run_comparable_scores(run).unwrap();
        let curr = vec![eval_axes("a", 60.0, 5.0)];
        assert!(find_regressions(&prev, &curr, 1.0).is_empty());
    }

    #[test]
    fn correctness_drop_is_regression_even_if_performance_rises() {
        let mut s = Store::open_in_memory().unwrap();
        let run = s.save_run("t", &[eval_axes("a", 60.0, 5.0)]).unwrap();
        let prev = s.run_comparable_scores(run).unwrap();
        let curr = vec![eval_axes("a", 40.0, 20.0)];
        let regs = find_regressions(&prev, &curr, 1.0);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].previous, 60.0);
        assert_eq!(regs[0].current, 40.0);
    }

    #[test]
    fn threshold_boundary_is_inclusive() {
        let mut prev = HashMap::new();
        prev.insert("a".to_string(), 60.0);
        assert_eq!(
            find_regressions(&prev, &[eval_axes("a", 59.0, 9.0)], 1.0).len(),
            1
        );
        assert!(find_regressions(&prev, &[eval_axes("a", 59.5, 0.0)], 1.0).is_empty());
    }

    #[test]
    fn threshold_zero_flags_ties_as_regression() {
        // 現状の挙動を固定: threshold=0 だと同点（差 0）も `>=` により回帰として返る。
        let mut prev = HashMap::new();
        prev.insert("a".to_string(), 60.0);
        let regs = find_regressions(&prev, &[eval("a", 60.0)], 0.0);
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].delta(), 0.0);
        // 上昇は threshold=0 でも回帰ではない
        assert!(find_regressions(&prev, &[eval("a", 60.5)], 0.0).is_empty());
    }

    #[test]
    fn find_regressions_handles_empty_inputs() {
        let mut prev = HashMap::new();
        prev.insert("a".to_string(), 60.0);
        assert!(find_regressions(&HashMap::new(), &[eval("a", 0.0)], 1.0).is_empty());
        assert!(find_regressions(&prev, &[], 1.0).is_empty());
        assert!(find_regressions(&HashMap::new(), &[], 0.0).is_empty());
    }

    #[test]
    fn empty_run_lists_with_no_best_candidate() {
        let mut s = Store::open_in_memory().unwrap();
        let run_id = s.save_run("t-empty", &[]).unwrap();
        assert_eq!(s.latest_run_id().unwrap(), Some(run_id));
        assert!(s.run_scores(run_id).unwrap().is_empty());
        assert!(s.run_comparable_scores(run_id).unwrap().is_empty());

        let runs = s.list_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, run_id);
        assert_eq!(runs[0].created_at, "t-empty");
        assert_eq!(runs[0].candidate_count, 0);
        assert_eq!(runs[0].best_candidate, None);
        assert_eq!(runs[0].best_score, None);
    }

    #[test]
    fn multiple_runs_list_newest_first_and_latest_is_max() {
        let mut s = Store::open_in_memory().unwrap();
        let r1 = s.save_run("t1", &[eval("a", 10.0)]).unwrap();
        let r2 = s.save_run("t2", &[]).unwrap();
        let r3 = s
            .save_run("t3", &[eval("a", 30.0), eval("b", 20.0)])
            .unwrap();
        assert!(r1 < r2 && r2 < r3);
        assert_eq!(s.latest_run_id().unwrap(), Some(r3));

        let runs = s.list_runs().unwrap();
        let ids: Vec<i64> = runs.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![r3, r2, r1]);
        assert_eq!(runs[0].candidate_count, 2);
        assert_eq!(runs[1].candidate_count, 0);
        assert_eq!(runs[2].candidate_count, 1);
        // run ごとに独立して集計される
        assert_eq!(runs[0].best_candidate.as_deref(), Some("a"));
        assert_eq!(runs[2].best_score, Some(10.0));
        assert_eq!(s.run_scores(r1).unwrap().get("a"), Some(&10.0));
    }

    #[test]
    fn duplicate_candidate_id_in_run_keeps_last_row() {
        // 現状の挙動を固定: 同一 candidate_id が複数行あると HashMap で後勝ち（挿入順）。
        let mut s = Store::open_in_memory().unwrap();
        let run = s
            .save_run("t", &[eval("a", 10.0), eval("a", 70.0)])
            .unwrap();
        assert_eq!(s.run_scores(run).unwrap().get("a"), Some(&70.0));
        assert_eq!(s.run_comparable_scores(run).unwrap().get("a"), Some(&70.0));
        assert_eq!(s.list_runs().unwrap()[0].candidate_count, 2);
    }

    #[test]
    fn open_directory_path_fails() {
        let dir = std::env::temp_dir();
        assert!(Store::open(&dir).is_err());
    }

    #[test]
    fn rows_without_axes_are_excluded_from_comparison() {
        let s = Store::open_in_memory().unwrap();
        s.conn
            .execute("INSERT INTO runs (id, created_at) VALUES (1, 't')", [])
            .unwrap();
        s.conn
            .execute(
                "INSERT INTO evaluations (run_id, candidate_id, score, detail_json)
                 VALUES (1, 'old', 90.0, '{\"candidate_id\":\"old\",\"score\":90.0}'),
                        (1, 'new', 50.0, '{\"axes\":{\"correctness\":30.0,\"performance\":20.0}}')",
                [],
            )
            .unwrap();
        let m = s.run_comparable_scores(1).unwrap();
        assert!(!m.contains_key("old"));
        assert_eq!(m.get("new"), Some(&30.0));
        // 古い行は前回値なし扱い → 現在が低くても回帰にならない
        assert!(find_regressions(&m, &[eval("old", 0.0)], 1.0).is_empty());
    }
}
