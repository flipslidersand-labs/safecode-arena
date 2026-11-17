# SafeCode Arena

AI が生成した複数のコード候補を、**安全な隔離環境で自動検証し、採点して採用候補を決める**検証ランナー。

> プロジェクトの主役はコード生成ではなく、「生成物を信用可能な状態へ持っていく検証処理系」。

## パイプライン

```text
仕様 → 複数コード候補 → コンパイル → テスト → (property/fuzz/性能/静的解析) → 採点 → 採用候補決定
```

## 使い方

```bash
# 単一候補
safecode evaluate candidate.rs --tests tests/

# 複数候補を比較してレポート出力
safecode evaluate cand_a.rs cand_b.rs cand_c.rs --tests tests/ --out report.md

# property test (proptest) も実行
safecode evaluate candidate.rs --prop-tests prop/

# JSON 出力 / 採点ルーブリックの上書き
safecode evaluate candidate.rs --format json --config safecode.toml

# 結果を DB に保存（過去 run と比較してリグレッション検出）
safecode evaluate candidate.rs --db history.db
safecode history --db history.db

# Wasm サンドボックスで隔離実行（候補に pub fn run() が必要）
safecode evaluate candidate.rs --wasm-entry run --wasm-fuel 100000000

# Python / Go / JavaScript 候補も評価可能（拡張子で自動判定）
safecode evaluate solution.py --tests py_tests/
safecode evaluate solution.go --tests tests/
safecode evaluate solution.js
safecode evaluate cand.rs cand.py cand.go    # 言語横断で 1 回比較

# LLM セマンティックレビュー（reasoning 軸）
safecode evaluate candidate.rs --llm-review

# spec ファイルから N 候補を生成し、評価して順位を付ける
safecode generate examples/specs/fibonacci.md --candidates 3
safecode generate myspec.md --candidates 5 --out report.md --format json
```

### 対応言語

| 言語       | compile        | test                  | lint                     | wasm             | mutation（`--mutation`）       |
| ---------- | -------------- | --------------------- | ------------------------ | ---------------- | ------------------------------ |
| Rust       | `cargo build`  | `cargo test`          | `clippy`                 | ✅ wasm32-wasip1 | `cargo-mutants`（PATH にあれば） |
| Python     | `py_compile`   | `pytest`              | `ruff`                   | —                | `mutmut`（PATH にあれば）        |
| Go         | `go build`     | `go test`             | `staticcheck` → `go vet` | —                | `gremlins`（PATH にあれば）      |
| JavaScript | `node --check` | `node --test`         | `eslint`（PATH にあれば）  | —                | —                              |
| TypeScript | `tsc --noEmit` | `tsc` + `node --test` | `eslint`（PATH にあれば）  | —                | —                              |

## 環境変数

| 変数 | 既定値 | 用途 |
| --- | --- | --- |
| `SAFECODE_LLM_BACKEND` | `claude` | `claude` か `ollama`。`--llm-review` と `generate` で使う LLM を選ぶ |
| `ANTHROPIC_API_KEY` | — | `SAFECODE_LLM_BACKEND=claude` のとき必須 |
| `ANTHROPIC_MODEL` | `claude-haiku-4-5-20251001` | 使用する Claude のモデル |
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama の API エンドポイント |
| `OLLAMA_MODEL` | `qwen2.5-coder:7b` | 生成・レビューに使う Ollama のモデル |

## 採点ルーブリック

| 軸              | 重み | 算出                                                                                       |
| --------------- | ---- | ------------------------------------------------------------------------------------------ |
| correctness     | 50   | compile 40% + test 40% + property test 20%（`--mutation` 時: 30% + 30% + 15% + mutation 25%） |
| security        | 20   | unsafe ヒューリスティック 40% + clippy/lint 40% + `cargo audit` 20%                        |
| performance     | 15   | `bench_ns`（Criterion）を優先。無ければ候補間 compile+test 時間の相対比較                  |
| maintainability | 10   | 関数長ヒューリスティック 60% + clippy 40%                                                  |
| resource_usage  | 5    | Wasm サンドボックス（wasm32-wasip1）実行の成否                                             |
| reasoning       | —    | LLM セマンティックレビューの点数（`--llm-review` を付け、`[weights]` の `reasoning` を正にしたとき。既定の重みは 0） |

重みは `safecode.toml` の `[weights]` で上書きできる。

### CI の必須設定

`runs-on: ${{ vars.GATE_RUNNER }}` を使う workflow（`eval.yml`・`ignored-tests.yml`）には既定の runner がありません。リポジトリまたは organization の変数 `GATE_RUNNER` に runner ラベルを設定してください。未設定の場合、これらのジョブは runner を割り当てられず完了しません（失敗するか、待機したままになります）。

## 開発

```bash
cargo build
cargo test --lib --tests      # 素の `cargo test` は失敗する（examples/ は評価用のサンプル候補で、Cargo の example ではない）
cargo clippy --lib --tests --bins -- -D warnings
cargo fmt
```

## ドキュメント

- [仕様・CLI リファレンス](docs/spec.md)
- [マニュアル](docs/manual.md)
- [設計書](docs/design.md)
- [技術スタック](docs/tech-stack.md)
- [データモデル](docs/data-model.md)
- [実装ガイド](docs/implementation-guide.md)
- [ADR](docs/adr/)
- [変更履歴](CHANGELOG.md)

## ステータス

現在の版: `0.1.0`（タグ・リリースは未作成）。直近の変更は[変更履歴](CHANGELOG.md)を参照。

✅ Phase 1〜15 完了。全採点軸を実測。SQLite 永続化 + リグレッション検出。Wasm サンドボックス。**多言語対応**: Rust / Python / Go / JavaScript / TypeScript を拡張子で自動判定。**Mutation Testing**: Rust（`cargo-mutants`）、Python（`mutmut`）、Go（`gremlins`）。**Criterion ベンチマーク**で performance 軸を実測。**LLM セマンティックレビュー**（Claude / Ollama）で reasoning 軸を採点。**`generate` コマンド**: 自然言語の spec から N 候補を生成して自動で順位付け。GitHub Actions CI でリグレッションを検出。

## ライセンス

MIT
