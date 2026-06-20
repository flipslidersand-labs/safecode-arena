# Data Model — SafeCode Arena

## 設定・入力

```rust
/// spec.yaml から読み込む評価仕様
#[derive(Deserialize)]
pub struct EvalSpec {
    pub name:        String,
    pub description: String,
    pub language:    Language,
    pub timeout_sec: u64,
    pub weights:     ScoreWeights,
    pub test_cases:  Vec<TestCase>,
}

#[derive(Deserialize)]
pub struct ScoreWeights {
    pub correctness:     u8,   // 合計 100 になること
    pub security:        u8,
    pub performance:     u8,
    pub maintainability: u8,
    pub resource_usage:  u8,
}

#[derive(Deserialize)]
pub enum Language { Rust, Python, Go }
```

## 評価結果

```rust
pub struct Submission {
    pub id:          String,   // UUID
    pub source_code: String,
    pub language:    Language,
}

pub struct CompileResult {
    pub ok:         bool,
    pub stderr:     String,
    pub duration_ms: u64,
}

pub struct TestResult {
    pub passed:   u32,
    pub failed:   u32,
    pub ignored:  u32,
    pub timed_out: bool,
    pub stdout:   String,
}

pub struct RunResult {
    pub stdout:      String,
    pub stderr:      String,
    pub exit_code:   i32,
    pub duration_ms: u64,
    pub timed_out:   bool,
    pub max_rss_kb:  u64,
}

pub struct EvalResult {
    pub submission_id:   String,
    pub compile:         CompileResult,
    pub test:            TestResult,
    pub score:           f64,        // 0.0〜100.0
    pub score_breakdown: ScoreBreakdown,
    pub evaluated_at:    chrono::DateTime<chrono::Utc>,
}

pub struct ScoreBreakdown {
    pub correctness:     f64,
    pub security:        f64,
    pub performance:     f64,
    pub maintainability: f64,
    pub resource_usage:  f64,
}
```

## SQLite スキーマ

```sql
CREATE TABLE submissions (
    id           TEXT PRIMARY KEY,
    source_code  TEXT NOT NULL,
    language     TEXT NOT NULL,
    submitted_at INTEGER NOT NULL
);

CREATE TABLE eval_results (
    id              TEXT PRIMARY KEY,
    submission_id   TEXT NOT NULL REFERENCES submissions(id),
    compile_ok      INTEGER NOT NULL,
    test_passed     INTEGER NOT NULL,
    test_failed     INTEGER NOT NULL,
    score           REAL NOT NULL,
    report_md       TEXT,
    evaluated_at    INTEGER NOT NULL
);
```
