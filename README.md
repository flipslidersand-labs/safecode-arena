# SafeCode Arena

AI が生成した複数のコード候補を、**安全な隔離環境で自動検証し、採点して採用候補を決める**検証ランナー。

> プロジェクトの主役はコード生成ではなく、「生成物を信用可能な状態へ持っていく検証処理系」。

## パイプライン

```text
仕様 → 複数コード候補 → コンパイル → テスト → (property/fuzz/性能/静的解析) → 採点 → 採用候補決定
```

## 使い方（MVP 目標）

```bash
# 単一候補
safecode evaluate candidate.rs --tests tests/

# 複数候補を比較してレポート出力
safecode evaluate cand_a.rs cand_b.rs cand_c.rs --tests tests/ --out report.md
```

## 採点ルーブリック

| 軸              | 重み |
| --------------- | ---- |
| correctness     | 50   |
| security        | 20   |
| performance     | 15   |
| maintainability | 10   |
| resource_usage  | 5    |

## 開発

```bash
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt
```

## ドキュメント

- [仕様](docs/spec.md)
- [技術スタック](docs/tech-stack.md)
- [データモデル](docs/data-model.md)
- [実装ガイド](docs/implementation-guide.md)
- [ADR](docs/adr/)

## ステータス

🚧 Phase 1 実装中（パイプライン骨格まで完了）。進捗は GitHub Issue #1「全体スケジュール」を参照。

## ライセンス

MIT
