# SafeCode Arena — 設計書

内側の構造をまとめる。外から見た振る舞い（CLI・設定・環境変数）は [spec.md](spec.md)、型の定義は [data-model.md](data-model.md)、決定の経緯は [adr/](adr/) を参照。

## 構成

バイナリ `safecode`（`src/main.rs`）は引数の解釈と入出力だけを持ち、検証・採点はライブラリ `safecode_arena`（`src/lib.rs`）に置く（[ADR-004](adr/ADR-004-cli-library-split.md)）。

```mermaid
flowchart TD
    main["main (CLI)"] --> config
    main --> generator
    main --> pipeline
    main --> scoring
    main --> report
    main --> store
    generator --> llm_client
    pipeline --> runner
    pipeline --> analysis
    pipeline --> test_summary
    pipeline --> wasm
    pipeline --> llm
    pipeline --> scoring
    llm --> llm_client
    scoring --> analysis
    scoring --> config
```

矢印は「呼び出す側 → 呼ばれる側」。`model` は全モジュールが共有する型なので図から省いた。

## モジュールの責務

| モジュール | 責務 |
| --- | --- |
| `main` | `evaluate` / `generate` / `history` の引数解釈、レポートの出力先、DB への保存と回帰時の終了コード |
| `config` | `safecode.toml` の読み込み。重み（`Rubric`）と言語別の上書き。未知キーと範囲外の値を拒否する |
| `model` | `Candidate` / `Language` / `StageOutcome` / `AxisScores` / `Evaluation` |
| `pipeline` | 候補を一時ディレクトリへ展開し、言語ごとに compile → test → lint →（任意）property test / mutation / bench / Wasm / LLM レビューを実行する |
| `runner` | 1 ステージ分の子プロセス実行。タイムアウト、プロセスグループ単位の kill、環境変数の許可リスト、出力の取り込み上限 |
| `analysis` | ソースの静的解析（`unsafe`・関数長）と、lint ツール出力からの指摘件数の集計 |
| `test_summary` | テスト出力の結果行を読み、終了コードだけに頼らず合否を判定する |
| `wasm` | wasmtime による `wasm32-wasip1` の隔離実行。fuel・メモリ・壁時計の上限 |
| `llm` | LLM セマンティックレビュー（reasoning 軸）。プロンプト生成と応答の解析 |
| `generator` | 自然言語 spec から N 候補を LLM で生成し、応答からコードブロックを取り出す |
| `llm_client` | Claude / Ollama への HTTP 呼び出し（`pub(crate)`）。バックエンドは環境変数で選ぶ |
| `scoring` | 各ステージの結果を重み付きで 5 軸の点数にする。performance は候補間の相対評価 |
| `report` | Markdown / HTML レポートの生成。候補由来の文字列のエスケープ |
| `store` | SQLite への保存と、過去 run との回帰検出 |

## データの流れ（`evaluate`）

1. `pipeline::load_candidates` がファイルを読み、拡張子から `Language` を決めて `Candidate` にする。
2. `config::Config::load` が重みを読み、言語ごとの `Rubric` を決める。
3. `pipeline::evaluate_candidate` が候補ごとに各ステージを実行する。各ステージは `runner` 経由の子プロセスで、結果は `StageOutcome`（Passed / Failed / TimedOut / Skipped）になる。
4. `scoring` が correctness / security / maintainability / resource_usage を候補単体で採点する。
5. 全候補がそろってから `scoring::assign_performance` が performance を相対評価で付け、`scoring::rank` が合計点の降順に並べる。
6. `report` が Markdown / HTML を生成する（JSON は `Evaluation` をそのまま出力）。
7. `--db` 指定時は `store` が同じ実行条件の過去 run と比べ、回帰があれば終了コード 1 で終わる。

`generate` は 1 の代わりに `generator::generate` が LLM から Rust の候補を受け取り、以降は同じ流れを通る。

## 外部依存

| 種類 | 内容 |
| --- | --- |
| 言語ツールチェーン | 評価する言語のコンパイラ・テストランナー・lint（README の「Supported languages」の表）。任意ツールが PATH に無い場合、そのステージは Skipped になる |
| Wasm | `wasmtime`（クレートとして埋め込み）と `wasm32-wasip1` ターゲット（[ADR-007](adr/ADR-007-wasm-sandbox.md)） |
| 永続化 | SQLite（`rusqlite`）。`--db` を指定したときだけ使う |
| LLM | Anthropic Messages API または Ollama。`--llm-review` と `generate` でだけ呼ぶ |

依存クレートの一覧と版は [tech-stack.md](tech-stack.md) と `Cargo.toml` を参照。

## 主要な制約

- **候補のコードはホスト上の子プロセスとして動く。** compile と test はプロセス隔離のみで、Wasm サンドボックスは `--wasm-entry` を指定した Rust 候補の追加ステージである（[ADR-002](adr/ADR-002-sandbox-strategy.md)）。信頼できない候補を評価するときは、ホスト側でコンテナ等の隔離を用意すること。
- **子プロセスに渡す環境変数は許可リスト方式。** `runner` の `ALLOWED_ENV` と `LC_*` 以外は渡さない。API キーは候補のプロセスに渡らない。
- **出力には上限がある。** 取り込みは 4 MiB、出力ファイルは 64 MiB で打ち切る（`src/runner.rs` の `MAX_CAPTURE_BYTES` / `MAX_OUTPUT_FILE_BYTES`）。
- **performance と reasoning は回帰比較に使わない。** performance は同じ run の候補間の相対値、reasoning は LLM の応答で揺れるため、`store::comparable_score` から除いている。
- **回帰比較は実行条件が同じ run どうしだけ。** オプションや重みが違う run とは比べない。
