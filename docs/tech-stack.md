# SafeCode Arena — 技術スタック

## 言語・バージョン

- **Rust** 2021 edition（rustc 1.94+）
  - 理由: プロセス制御・所有権による安全なリソース管理・将来の Wasmtime 組み込みとの親和性。検証ランナー自体が高速・堅牢である必要がある。

## ビルド・実行環境

- **Cargo**: ビルド/テスト/依存管理
- バイナリ名: `safecode`（`src/main.rs`）
- ライブラリ: `safecode_arena`（`src/lib.rs`）— コアドメインを CLI から分離

## 主要クレートと選定理由

| クレート                     | 用途                                             | 選定理由                                    |
| ---------------------------- | ------------------------------------------------ | ------------------------------------------- |
| `clap`                       | CLI 引数パース                                   | derive で宣言的、サブコマンドに強い         |
| `serde` / `serde_json`       | 候補・結果・レポートのシリアライズ               | デファクト。JSON 出力で他ツール連携が容易   |
| `toml`                       | 採点ルーブリック設定の読込                       | 設定ファイルを TOML で持たせる（Phase 2）   |
| `anyhow`                     | アプリ層のエラー集約                             | エラー伝播を簡潔に                          |
| `thiserror`                  | ドメイン層のエラー型定義                         | ライブラリ境界では型付きエラー              |
| `tempfile`                   | 候補のサンドボックス展開                         | 一時ディレクトリの安全な生成/自動削除       |
| `wait-timeout`               | 子プロセスのタイムアウト kill                    | 制限時間超過で確実に kill（暴走コード対策） |
| `chrono`                     | レポートのタイムスタンプ                         | 時刻整形                                    |
| `rusqlite` (bundled)         | run 履歴の永続化・リグレッション検出             | bundled で system sqlite 非依存             |
| `wasmtime` / `wasmtime-wasi` | Wasm サンドボックス実行（capability ベース隔離） | fuel/メモリ制限付き埋め込みランタイム       |

### dev-dependencies

| クレート     | 用途              |
| ------------ | ----------------- |
| `assert_cmd` | CLI の E2E テスト |
| `predicates` | 出力アサーション  |

## Phase 3 で利用（候補プロジェクト側のツール）

| ツール         | 用途                                        | 備考                                           |
| -------------- | ------------------------------------------- | ---------------------------------------------- |
| `cargo clippy` | security / maintainability の lint シグナル | Rust ツールチェーン同梱。clippy ステージで実行 |
| `proptest`     | property test（`--prop-tests` 指定時）      | 候補の一時プロジェクトに dev-dependency 追加   |

## 発展フェーズで追加予定（未使用）

| クレート      | 用途                                             | フェーズ |
| ------------- | ------------------------------------------------ | -------- |
| `tree-sitter` | AST 解析・コード類似度（精緻な maintainability） | Phase 5+ |
| `cargo-fuzz`  | Fuzzing 実行（外部ツール）                       | Phase 5+ |
| `criterion`   | 性能ベンチマーク（絶対性能）                     | Phase 5+ |
| `cargo-audit` | 依存の脆弱性スキャン                             | Phase 5+ |

## 開発ツール

- **rustfmt**: フォーマット固定（`cargo fmt`）
- **clippy**: lint（`cargo clippy -- -D warnings`）
- **cargo test**: 単体 + E2E テスト
- CI: GitHub Actions（fmt / clippy / test）— Phase 2 で追加

## 依存関係の構造

```text
src/main.rs (CLI: safecode)
   └── src/lib.rs (safecode_arena)
        ├── model    … Candidate / Evaluation / StageOutcome / Language
        ├── runner   … タイムアウト付きプロセス実行（→ tempfile, std::process）
        ├── scoring  … 採点ルーブリック（重みは spec.md と一致）
        └── report   … Markdown レポート生成（→ chrono）
```
