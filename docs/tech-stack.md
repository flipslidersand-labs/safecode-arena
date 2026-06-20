# Tech Stack — SafeCode Arena

## 言語・バージョン

- Rust 1.78+ (edition 2021)

## 主要クレートと選定理由

| クレート      | バージョン | 役割                              | 選定理由                                 |
| ------------- | ---------- | --------------------------------- | ---------------------------------------- |
| `wasmtime`    | 28         | Wasm サンドボックス実行 (Phase 5) | WASI 対応・Capability 制御が強力         |
| `rusqlite`    | 0.31       | 評価結果の永続化                  | 外部 DB 不要で bundled 可能              |
| `tempfile`    | 3          | 評価用一時ディレクトリ            | Drop 時に自動削除でクリーンアップ不要    |
| `serde`       | 1          | spec.yaml・結果の直列化           | derive マクロで簡潔                      |
| `serde_yaml`  | 0.9        | spec.yaml 読み込み                | 評価軸定義ファイルのパース               |
| `tokio`       | 1          | 非同期サブプロセス + タイムアウト | `tokio::process::Command` でプロセス管理 |
| `clap`        | 4          | CLI                               | derive マクロで簡潔                      |
| `anyhow`      | 1          | エラーハンドリング                | 雑多なサブプロセスエラーを集約           |
| `tree-sitter` | 0.22       | 静的解析 (行数・複雑度, Phase 6)  | 構文木を取得して maintainability 計算    |

## アーキテクチャ

```
CLI (clap)
  └── Runner
        ├── Compiler     rustc / cargo build を subprocess で実行
        ├── Tester       cargo test を subprocess で実行
        ├── Scorer       重み付きスコア計算
        ├── Reporter     Markdown レポート生成
        └── Store        SQLite への永続化

[Phase 5]
Runner
  └── WasmSandbox       Wasmtime + WASI でコードを隔離実行
```

## 評価フロー

```
spec.yaml
  + candidates/
       ↓
[Compiler]  → compile_ok, compile_time_ms, stderr
[Tester]    → passed, failed, timeout_count, stdout
[Scorer]    → weighted_score (0〜100)
[Reporter]  → report.md
[Store]     → results.db (SQLite)
```
