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
    score: f64,        // 0.0〜100.0
}
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

ステージ遷移ルール（MVP）:

```text
compile = Passed ──→ test を実行
compile = Failed/TimedOut ──→ test = Skipped
```

## 採点モデル（重み）

`docs/spec.md` の評価軸と一致させる。`src/scoring.rs` の定数が単一の真実源。

```text
correctness     50   ← MVP で実測（compile 25 + test 25）
security        20   ← Phase 3
performance     15   ← Phase 3
maintainability 10   ← Phase 3
resource_usage   5   ← Phase 3
合計            100
```

## 永続化（Phase 4）

MVP では永続化しない（結果は標準出力 or Markdown ファイル）。
Phase 4 で SQLite に以下を保存する想定:

| テーブル      | 主なカラム                                        |
| ------------- | ------------------------------------------------- |
| `runs`        | id, created_at, spec_hash                         |
| `candidates`  | id, run_id, source_hash, language                 |
| `evaluations` | candidate_id, run_id, compile, test, score (JSON) |

関係: `runs 1 — N candidates 1 — 1 evaluations`

## インターフェース（JSON 出力）

`--format json` 指定時（Phase 2）に出力する形。`Evaluation` を serde でそのままシリアライズ。

```json
{
  "candidate_id": "cand_b",
  "compile": { "Passed": { "duration_ms": 1200 } },
  "test": { "Passed": { "duration_ms": 800 } },
  "score": 50.0
}
```
