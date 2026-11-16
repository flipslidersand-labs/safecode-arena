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

| 軸              | 重み | 実装状況                                                                                                                                                                                    |
| --------------- | ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| correctness     | 50   | ✅ compile 40% + test 40% + prop_test 20%（mutation 有効時: 30+30+15+mutation 25%）<br>mutation: Rust=`cargo-mutants`、Python=`mutmut`、Go=`gremlins`（各ツールが PATH になければ Skipped） |
| security        | 20   | ✅ unsafe ヒューリスティック 40% + lint 40% + `cargo audit` 20%（audit が結果を返せないときは audit 項を除き、残りを 50% ずつに再配分）                                                     |
| performance     | 15   | ✅ 候補間の相対比較。compile と test が通った候補が全員 Criterion の計測値を持つときはその値、それ以外は compile+test 時間                                                                  |
| maintainability | 10   | ✅ 関数長ヒューリスティック 60% + clippy 40%                                                                                                                                                |
| resource_usage  | 5    | ✅ Wasm サンドボックス実行の成否                                                                                                                                                            |

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

- Go / JavaScript 対応（発展。Python は対応済み）
- Wasmtime/WASI による完全サンドボックス（MVP はプロセス隔離）
- Property-based Test / Fuzzing の自動実行（発展）
- Mutation / Differential Testing（発展）
- 複数 AI API 連携・コード生成（本プロジェクトは「検証」が責務）
- GitHub PR 連携（発展）

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

## CLI リファレンス

`safecode --help` と各サブコマンドの `--help` の出力（版 0.1.0）、および `src/main.rs` の定義に基づく。

### `safecode evaluate [OPTIONS] [CANDIDATES]...`

1 つ以上の候補ファイルを検証・採点する。言語は拡張子（`.rs` `.py` `.go` `.js` `.ts`）で決まる。

| オプション | 既定値 | 内容 |
| --- | --- | --- |
| `--tests <DIR>` | なし | テストファイルを置いたディレクトリ |
| `--prop-tests <DIR>` | なし | proptest を使った property test を置いたディレクトリ |
| `--timeout-secs <N>` | 60 | 各ステージのタイムアウト秒数 |
| `--out <FILE>` | 標準出力 | レポートの出力先 |
| `--format <FORMAT>` | `markdown` | `markdown` / `json` / `html` |
| `--config <FILE>` | `./safecode.toml` があれば使用 | 採点ルーブリックの設定ファイル |
| `--db <FILE>` | なし | 評価結果を保存する SQLite DB。指定すると過去 run との回帰を検出する |
| `--regression-threshold <X>` | 1 | 回帰と判定する点数の下落幅（0 以上の有限値） |
| `--wasm-entry <NAME>` | なし | Wasm サンドボックスで実行するエントリ関数名（引数なしの `pub fn`）。指定すると `wasm32-wasip1` にビルドして隔離実行する |
| `--wasm-fuel <N>` | 100000000 | Wasm 実行の fuel 上限（命令数） |
| `--wasm-max-memory-mb <N>` | 64 | Wasm 実行のメモリ上限（MB、1〜4096） |
| `--mutation` | off | ミューテーションテストを実行する。ツールが PATH に無ければ Skipped |
| `--llm-review` | off | LLM セマンティックレビューを実行し reasoning 軸を採点する |

### `safecode generate [OPTIONS] <SPEC>`

自然言語の spec ファイル（`.md` / `.txt`）から候補を LLM で生成し、検証・採点する。生成される候補は Rust。

| オプション | 既定値 | 内容 |
| --- | --- | --- |
| `--candidates <N>` | 3 | 生成する候補数（1〜8） |
| `--timeout-secs <N>` | 30 | 評価ステージのタイムアウト秒数 |
| `--llm-timeout-secs <N>` | 120 | LLM 呼び出しのタイムアウト秒数 |
| `--out <FILE>` | 標準出力 | レポートの出力先 |
| `--format <FORMAT>` | `markdown` | `markdown` / `json` / `html` |
| `--config <FILE>` | `./safecode.toml` があれば使用 | 採点ルーブリックの設定ファイル |

### `safecode history --db <FILE>`

保存済みの run 履歴を一覧表示する。`--db` は必須。

### 環境変数

`--llm-review` と `generate` でだけ参照する（`src/llm_client.rs`）。

| 変数 | 既定値 | 内容 |
| --- | --- | --- |
| `SAFECODE_LLM_BACKEND` | `claude` | `ollama` を指定すると Ollama、それ以外は Claude |
| `ANTHROPIC_API_KEY` | なし | Claude を使うときに必須。未設定なら LLM 呼び出しは結果なしになる |
| `ANTHROPIC_MODEL` | `claude-haiku-4-5-20251001` | Claude のモデル |
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama の API エンドポイント |
| `OLLAMA_MODEL` | `qwen2.5-coder:7b` | Ollama のモデル |

### 設定ファイル（`safecode.toml`）

`[weights]` に 5 軸（`correctness` `security` `performance` `maintainability` `resource_usage`）と任意の `reasoning` を書く。`[weights.rust]` `[weights.python]` `[weights.go]` `[weights.javascript]` `[weights.typescript]` で言語別に上書きできる。未知のキー、有限でない値、負の値はエラーになる（`src/config.rs`）。

### 終了コード

| コード | 条件 |
| --- | --- |
| 0 | 正常終了（候補のコンパイルやテストが失敗していても、評価自体が完了すれば 0） |
| 1 | 実行エラー（候補ファイルの指定なし、ファイルが無い、未対応の拡張子など）。または `--db` 指定時に回帰を検出した |
| 2 | 引数の誤り（未知のオプションなど） |

## 成功条件

- **Phase 1 完了**: `safecode evaluate candidate.rs` で、コンパイル・テスト・採点・レポート生成が一気通貫で動く。
- **Phase 2 完了**: 本物のタイムアウト kill と複数候補比較が動く。
- **Phase 3 完了**: security/performance/maintainability の各軸が実測値で採点される。
