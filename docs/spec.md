# SafeCode Arena — 仕様書

## 目的

AI が生成した複数のコード候補を、**安全な隔離環境で自動検証し、採点して採用候補を決める**検証ランナー。プロジェクトの主役はコード生成ではなく、「生成物を信用可能な状態へ持っていく検証処理系」である。

## 解決する問題

- AI 生成コードは「それらしく動く」が、正しさ・安全性・性能の保証がない。
- 候補が複数あるとき、人手でのレビュー・実行・比較はコストが高く再現性もない。
- 検証パイプライン（コンパイル → テスト → property test → fuzz → 性能 → 静的解析）を一貫した採点基準で回す仕組みが必要。

## パイプライン

```text
仕様
 ↓
複数コード候補
 ↓
コンパイル
 ↓
単体テスト
 ↓
Property-based Test
 ↓
Fuzzing
 ↓
性能測定
 ↓
静的解析
 ↓
採用候補決定
```

## 評価軸（採点ルーブリック）

| 軸              | 重み | 実装状況                                      |
| --------------- | ---- | --------------------------------------------- |
| correctness     | 50   | ✅ compile 40% + test 40% + property test 20% |
| security        | 20   | ✅ unsafe ヒューリスティック 50% + clippy 50% |
| performance     | 15   | ✅ 候補間 compile+test 時間の相対比較         |
| maintainability | 10   | ✅ 関数長ヒューリスティック 60% + clippy 40%  |
| resource_usage  | 5    | ⬜ 未計測（Phase 4 以降）                     |

## MVP の境界線

### やること（Phase 1〜2）

- Rust コード候補（`.rs` ファイル）の受け取り（複数可）
- 一時ディレクトリへの展開とコンパイル（`cargo build`）
- テスト実行（`cargo test`、テストディレクトリ指定可）
- ステージごとのタイムアウト
- 標準出力/標準エラーの取得
- correctness 軸での採点とランキング
- Markdown レポート生成

### やらないこと（MVP では対象外）

- Python / Go / JavaScript 対応（発展）
- Wasmtime/WASI による完全サンドボックス（MVP はプロセス隔離）
- Property-based Test / Fuzzing の自動実行（発展）
- Mutation / Differential Testing（発展）
- 複数 AI API 連携・コード生成（本プロジェクトは「検証」が責務）
- GitHub PR 連携（発展）
- 結果の永続化（SQLite）（Phase 4）

## 利用イメージ

```bash
# 単一候補
safecode evaluate candidate.rs --tests tests/

# 複数候補を比較
safecode evaluate cand_a.rs cand_b.rs cand_c.rs --tests tests/ --out report.md
```

出力（Markdown レポート）:

```text
# SafeCode Arena 評価レポート
| 順位 | 候補   | スコア | コンパイル | テスト |
| 1    | cand_b | 50.0   | ✅ 1200ms | ✅ 800ms |
| 2    | cand_a | 25.0   | ✅ 900ms  | ❌ failed |
**採用候補**: `cand_b`（50.0点）
```

## 成功条件

- **Phase 1 完了**: `safecode evaluate candidate.rs` で、コンパイル・テスト・採点・レポート生成が一気通貫で動く。
- **Phase 2 完了**: 本物のタイムアウト kill と複数候補比較が動く。
- **Phase 3 完了**: security/performance/maintainability の各軸が実測値で採点される。
