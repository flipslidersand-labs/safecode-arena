# Spec — SafeCode Arena

## プロジェクトの目的

AI が生成した複数のコード候補を安全な環境で自動コンパイル・テスト・ファジング・性能計測し、
重み付きスコアで採点して Markdown レポートを生成する評価ランナー。

## 解決する問題

| 問題                                          | SafeCode Arena での解決策                      |
| --------------------------------------------- | ---------------------------------------------- |
| AI 生成コードを信用せずに手動で確認するコスト | 自動化された評価パイプラインで定量スコアを出す |
| コード候補のどれが「最善」か判断しにくい      | 正確性・安全性・性能の多軸スコアで比較         |
| 悪意あるコードや無限ループのリスク            | タイムアウト + プロセス分離でホストを保護      |

## 利用イメージ

```bash
# 単一ファイルを評価
safecode-arena eval --file candidate.rs --spec spec.yaml

# ディレクトリ内の全候補を比較
safecode-arena compare --dir ./candidates/ --spec spec.yaml --out report.md

# STDIN からコードを受け取る
cat generated.rs | safecode-arena eval --lang rust --spec spec.yaml
```

## 評価軸と重み

```yaml
correctness: 50 # cargo test の合否
security: 20 # clippy deny(unsafe) + cargo-audit
performance: 15 # Criterion ベンチマーク (relative)
maintainability: 10 # ファイル行数・関数の複雑度
resource_usage: 5 # max RSS・コンパイル時間
```

## MVP の境界線

### やること (Phase 1〜4)

- Rust ソースのコンパイル（`rustc` をサブプロセスで呼ぶ）
- `cargo test` の実行・タイムアウト検出・stdout キャプチャ
- テスト結果から correctness スコアを計算
- Markdown レポート生成 (`report.md`)
- SQLite に評価結果を永続化

### やらないこと (Phase 1)

- Wasmtime サンドボックス（Phase 5 で追加）
- Python / Go / JS 対応
- Fuzzing・Mutation Testing
- GitHub PR 連携

## 成功条件

| Phase   | 完成条件                                                                           |
| ------- | ---------------------------------------------------------------------------------- |
| Phase 1 | `rustc candidate.rs` をサブプロセスで実行し compile_ok / stderr を記録             |
| Phase 2 | `cargo test` 結果から passed/failed/timeout を正しく取得                           |
| Phase 3 | 重み付きスコアを計算し SQLite に保存                                               |
| Phase 4 | `report.md` に全候補のスコア比較表が生成される                                     |
| Phase 5 | Wasmtime + WASI でコードをサンドボックス実行、ファイル・ネット無制限アクセスを防止 |
| Phase 6 | `proptest` による Property-based testing を評価パイプラインに組み込む              |
