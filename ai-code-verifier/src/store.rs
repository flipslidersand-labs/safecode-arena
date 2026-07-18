//! 評価結果の永続化（SQLite）と run 間のリグレッション検出。
//!
//! - 各 `evaluate` 実行を 1 つの run として保存する。
//! - 同じ候補 ID のスコアが過去 run より下がっていれば「リグレッション」として検出する。

use crate::model::Evaluation;
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

/// 過去スコアと現在の評価を突き合わせ、スコアが下がった候補を返す。
/// `threshold` 以上下落したものだけを対象にする（誤差・揺らぎ除け）。
pub fn find_regressions(
    previous: &HashMap<String, f64>,
    current: &[Evaluation],
    threshold: f64,
) -> Vec<Regression> {
    let mut out = Vec::new();
    for e in current {
        if let Some(&prev) = previous.get(&e.candidate_id) {
            if prev - e.score >= threshold {
                out.push(Regression {
                    candidate_id: e.candidate_id.clone(),
                    previous: prev,
                    current: e.score,
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

    fn eval(id: &str, score: f64) -> Evaluation {
        Evaluation {
            candidate_id: id.into(),
            compile: StageOutcome::Passed { duration_ms: 1 },
            test: StageOutcome::Passed { duration_ms: 1 },
            lint: StageOutcome::Skipped,
            lint_warnings: 0,
            prop_test: StageOutcome::Skipped,
            wasm: StageOutcome::Skipped,
            wasm_fuel_used: None,
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
}
