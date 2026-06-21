# SafeCode Arena — データモデル

ソースの `src/model.rs` と本ドキュメントを常に一致させること。

## コアデータ構造

### `Language`

検証対象の言語。MVP は `Rust` のみ。

```rust
enum Language { Rust /* 発展: Python, Go, JavaScript */ }
```

### `Candidate`

1 つの AI 生成コード候補。

```rust
struct Candidate {
    id: String,        // ファイル名やモデル名から導出
    source: String,    // ソースコード本体
    language: Language,
}
```

### `Evaluation`

1 候補に対する検証結果の集約。

```rust
struct Evaluation {
    candidate_id: String,
    compile: StageOutcome,
    test: StageOutcome,
    clippy: StageOutcome,      // compile 成功時のみ実行
    clippy_warnings: usize,    // clippy が報告した warning 数
    prop_test: StageOutcome,   // --prop-tests 指定時のみ実行
    axes: AxisScores,          // 軸別の獲得点
    score: f64,                // = axes.total()（0.0〜100.0）
}
```

### `AxisScores`

各評価軸の獲得点。上限は `config::Rubric` の重み。

```rust
struct AxisScores {
    correctness: f64,
    security: f64,
    performance: f64,
    maintainability: f64,
    resource_usage: f64,
}
// total() = 全軸の合計
```

## 状態遷移（StageOutcome）

各検証ステージの結果を表す enum。

```rust
enum StageOutcome {
    Passed { duration_ms: u64 },  // 成功 + 所要時間
    Failed { detail: String },    // 失敗 + stderr 要約
    TimedOut { limit_ms: u64 },   // タイムアウト
    Skipped,                      // 前段失敗等で未実行
}
```

ステージ遷移ルール:

```text
compile = Passed ──→ test / clippy を実行（prop_test は --prop-tests 指定時）
compile = Failed/TimedOut ──→ test / clippy / prop_test = Skipped
```

## 採点モデル（重み）

重みは `config::Rubric`（既定値 = `docs/spec.md` の評価軸）。`safecode.toml` で上書き可。
各軸の達成率は `src/scoring.rs` / `src/analysis.rs` が算出する。

```text
correctness     50   ← compile 40% + test 40% + prop_test 20%
security        20   ← unsafe ヒューリスティック 50% + clippy 50%
performance     15   ← 候補間 compile+test 時間の相対比較（最速=満点）
maintainability 10   ← 関数長ヒューリスティック 60% + clippy 40%
resource_usage   5   ← 未計測（0）。Phase 4 以降
合計            100
```

- security / maintainability は **compile 成功時のみ加点**。
- clippy warning: security は 1 件 -0.1、maintainability は 3 件ごと -0.1。
- prop_test 失敗（反例検出）は correctness の prop 分（20%）が入らない。

## 永続化（Phase 4）

MVP では永続化しない（結果は標準出力 or Markdown ファイル）。
Phase 4 で SQLite に以下を保存する想定:

| テーブル      | 主なカラム                                              |
| ------------- | ------------------------------------------------------- |
| `runs`        | id, created_at, spec_hash                               |
| `candidates`  | id, run_id, source_hash, language                       |
| `evaluations` | candidate_id, run_id, compile, test, axes, score (JSON) |

関係: `runs 1 — N candidates 1 — 1 evaluations`

## インターフェース（JSON 出力）

`--format json` 指定時に出力する形。`Evaluation` を serde でそのままシリアライズ。

```json
{
  "candidate_id": "cand_b",
  "compile": { "Passed": { "duration_ms": 1200 } },
  "test": { "Passed": { "duration_ms": 800 } },
  "clippy": { "Passed": { "duration_ms": 300 } },
  "clippy_warnings": 0,
  "prop_test": "Skipped",
  "axes": {
    "correctness": 40.0,
    "security": 20.0,
    "performance": 15.0,
    "maintainability": 10.0,
    "resource_usage": 0.0
  },
  "score": 85.0
}
```
