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

# property test (proptest) も実行
safecode evaluate candidate.rs --prop-tests prop/

# JSON 出力 / 採点ルーブリックの上書き
safecode evaluate candidate.rs --format json --config safecode.toml
```

## 採点ルーブリック

| 軸              | 重み | 算出                                       |
| --------------- | ---- | ------------------------------------------ |
| correctness     | 50   | compile 40% + test 40% + property test 20% |
| security        | 20   | unsafe ヒューリスティック 50% + clippy 50% |
| performance     | 15   | 候補間 compile+test 時間の相対比較         |
| maintainability | 10   | 関数長ヒューリスティック 60% + clippy 40%  |
| resource_usage  | 5    | 未計測（Phase 4 以降）                     |

重みは `safecode.toml` の `[weights]` で上書きできる。

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

✅ Phase 3 完了（多軸採点: correctness/security/performance/maintainability を実測。clippy ステージ + property test 対応）。次は Phase 4（Wasm サンドボックス・SQLite 永続化）。進捗は GitHub Issue #1「全体スケジュール」を参照。

## ライセンス

MIT
