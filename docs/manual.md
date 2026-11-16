# SafeCode Arena — マニュアル

使う人向けの手順。オプションの一覧は [spec.md](spec.md) の「CLI リファレンス」を参照。

「検証済み」と書いたコマンドは 2026-11-17 に Linux 上で実行して結果を確認した。それ以外は**未検証**（コードとヘルプ出力から書いた）。

## 導入

必要なもの: Rust（版は `rust-toolchain.toml` で固定。rustup が自動で合わせる）。評価したい言語のツール（Python / Go / Node.js など）は、その言語の候補を評価するときだけ要る。

```bash
git clone https://github.com/flipslidersand-labs/safecode-arena.git
cd safecode-arena
cargo build --release
./target/release/safecode --version
```

上の 4 行は未検証（CI は同じツールチェーンでビルドしている）。以降の例は `safecode` に PATH が通っている前提で書く。通していない場合は `./target/release/safecode` に読み替える。

## 日常操作

### 候補を 1 つ評価する（検証済み）

リポのルートで実行する。

```bash
safecode evaluate examples/good.rs
```

標準出力に Markdown のレポートが出る。実際の出力（抜粋）:

```text
| 順位 | 候補 | 合計 | 正誤 | 安全 | 性能 | 保守 | 資源 | コンパイル | テスト | Clippy | PropTest | Wasm |
| 1 🥇 | good | 85.0 | 40.0 | 20.0 | 15.0 | 10.0 | 0.0 | ✅ 75ms | ✅ 132ms | ✅ 0 warn | — skipped | — skipped |

**採用候補**: `good`（85.0点）
```

property test と Wasm を指定していないので、その分（correctness の 20%、resource_usage の 5 点）は入らない。

### 複数の候補を比べる

```bash
# JSON で出す（検証済み）
safecode evaluate examples/good.rs examples/buggy.rs --format json

# ファイルに書き出す（未検証）
safecode evaluate examples/good.rs examples/buggy.rs --out report.md
safecode evaluate examples/good.rs examples/buggy.rs --format html --out report.html
```

言語は拡張子で決まる。違う言語の候補を 1 回で比べられる。

### テストを足す（未検証）

```bash
safecode evaluate candidate.rs --tests tests/ --prop-tests prop/
```

### 履歴を残して回帰を検出する（未検証）

```bash
safecode evaluate candidate.rs --db history.db
safecode history --db history.db
```

同じ実行条件の前回 run より点数が `--regression-threshold`（既定 1）以上下がると、警告を出して終了コード 1 で終わる。その run は DB に保存されない。

### Wasm サンドボックスで実行する（未検証）

候補に引数なしの `pub fn` が必要。`wasm32-wasip1` ターゲットを入れておく。

```bash
rustup target add wasm32-wasip1
safecode evaluate candidate.rs --wasm-entry run
```

### LLM を使う（未検証）

```bash
# Claude（既定）
export ANTHROPIC_API_KEY=<your-key>
safecode evaluate candidate.rs --llm-review
safecode generate examples/specs/fibonacci.md --candidates 3

# Ollama
export SAFECODE_LLM_BACKEND=ollama
safecode evaluate candidate.rs --llm-review
```

`generate` が作る候補は Rust である。環境変数の一覧は [spec.md](spec.md) を参照。

### 重みを変える（未検証）

カレントディレクトリに `safecode.toml` を置くか、`--config` で指定する。

```toml
[weights]
correctness = 50
security = 20
performance = 15
maintainability = 10
resource_usage = 5

# 言語別の上書き（任意）
[weights.go]
correctness = 55
security = 15
performance = 15
maintainability = 10
resource_usage = 5
```

## トラブル時

| 症状 | 原因と対処 |
| --- | --- |
| `Error: 候補ファイルを 1 つ以上指定してください`（終了コード 1） | `evaluate` に候補ファイルを渡していない |
| `No such file or directory`（終了コード 1） | 候補のパスが違う。`examples/` の例はリポのルートから実行する |
| `未対応の拡張子` | 対応は `.rs` `.py` `.go` `.js` `.ts`。拡張子で言語を決めている |
| lint / mutation の列が `skipped` | そのツール（`ruff`、`staticcheck`、`eslint`、`cargo-mutants` など）が PATH に無い。入れると採点に入る |
| ステージが `timed out` | `--timeout-secs` を延ばす（`evaluate` の既定は 60 秒、`generate` は 30 秒） |
| `--llm-review` を付けても reasoning が `Skipped` | `ANTHROPIC_API_KEY` が未設定、または LLM から有効な応答が返らなかった。その候補は reasoning を採点しない |
| `--db` 付きの実行が終了コード 1 で終わる | 回帰の検出。標準エラーに下がった候補が出る。意図した変更なら別の DB ファイルで取り直す |
| `safecode.toml` の読み込みエラー | 未知のキーと、有限でない値・負の値は拒否する |
| `cargo test` が失敗する（開発時） | `cargo test --lib --tests` を使う。`examples/` は評価用の候補で、Cargo の example としてはビルドできない |

## 注意

候補のコードはホスト上の子プロセスとして動く（[design.md](design.md) の「主要な制約」）。出所の分からないコードを評価するときは、コンテナや使い捨ての VM の中で `safecode` を実行する。
