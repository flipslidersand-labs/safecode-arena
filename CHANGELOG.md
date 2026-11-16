# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

No version has been tagged or released yet (`Cargo.toml` declares `0.1.0`), so everything is listed under `Unreleased`.
Entries are the titles of merged `feat` / `fix` pull requests, oldest first. Refactors, tests, docs, CI and dependency updates are not listed.
The first features were committed directly, before pull requests were used; those entries are commit subjects and carry no link.

## [Unreleased]

### Added

- Phase 1 evaluation pipeline (compile, test, scoring, Markdown report)
- Phase 2: timeout kill, rubric config, JSON output, external tests
- Phase 3: clippy and property test stages, multi-axis scoring
- Phase 4-a: SQLite persistence, regression detection, `history`
- Phase 4-b: Wasm sandbox; enhanced comparison report
- Python support (multi-language evaluation)
- Go / JavaScript 対応 ([#8](https://github.com/flipslidersand-labs/safecode-arena/pull/8))
- Phase 7 — staticcheck + ESLint + language-specific rubric ([#9](https://github.com/flipslidersand-labs/safecode-arena/pull/9))
- **phase8**: mutation testing — cargo mutants で correctness を強化 ([#44](https://github.com/flipslidersand-labs/safecode-arena/pull/44))
- **phase8b**: Python mutation testing — mutmut で correctness を強化 ([#48](https://github.com/flipslidersand-labs/safecode-arena/pull/48))
- add sample candidate files to examples/ ([#60](https://github.com/flipslidersand-labs/safecode-arena/pull/60))
- **phase11**: GitHub Actions CI — PR 評価自動化 + リグレッション検出 ([#68](https://github.com/flipslidersand-labs/safecode-arena/pull/68))
- **phase10**: Criterion ベンチマーク統合 — performance スコア実測化 ([#70](https://github.com/flipslidersand-labs/safecode-arena/pull/70))
- **phase14**: LLM セマンティックレビュー軸 — Claude/Ollama で reasoning スコア ([#79](https://github.com/flipslidersand-labs/safecode-arena/pull/79))
- **phase13**: TypeScript サポート — tsc --noEmit + node --test ([#80](https://github.com/flipslidersand-labs/safecode-arena/pull/80))
- **phase15**: generate サブコマンド — 自然言語 spec から候補を LLM 自動生成 ([#81](https://github.com/flipslidersand-labs/safecode-arena/pull/81))
- PRテンプレートにtest-generator呼び出しチェック項目を追加 ([#103](https://github.com/flipslidersand-labs/safecode-arena/pull/103))
- **test-summary**: テスト出力の結果行パーサを追加 (#226 PR1) ([#236](https://github.com/flipslidersand-labs/safecode-arena/pull/236))

### Fixed

- **python**: candidate.py 内の test_ 関数を pytest で収集する ([#7](https://github.com/flipslidersand-labs/safecode-arena/pull/7))
- **mutation**: don't misclassify failed stages when JSON is parseable ([#50](https://github.com/flipslidersand-labs/safecode-arena/pull/50))
- assign_reasoning() 呼出追加 + OLLAMA_HOST 規約統一 ([#89](https://github.com/flipslidersand-labs/safecode-arena/pull/89))
- generate --llm-timeout-secs 追加 + TypeScript テスト candidate.js 除外 ([#90](https://github.com/flipslidersand-labs/safecode-arena/pull/90))
- CIでテスト実行可能に、DB/sandbox依存は誤りと判明 ([#105](https://github.com/flipslidersand-labs/safecode-arena/pull/105))
- **runner**: stdout も含めて全文キャプチャし audit/ruff/bench の結果が捨てられる問題を修正 ([#113](https://github.com/flipslidersand-labs/safecode-arena/pull/113))
- **scoring**: bench 経路でも compile/test 成功候補のみ performance 採点 ([#114](https://github.com/flipslidersand-labs/safecode-arena/pull/114))
- **pipeline**: 未知拡張子をエラーにし候補 ID の重複を検出 ([#115](https://github.com/flipslidersand-labs/safecode-arena/pull/115))
- **config**: 言語別ルーブリックを評価に反映し部分上書きに対応 ([#129](https://github.com/flipslidersand-labs/safecode-arena/pull/129))
- **generate**: max_tokens を候補数に比例させ --candidates を検証 (#118) ([#133](https://github.com/flipslidersand-labs/safecode-arena/pull/133))
- **store**: 回帰検出から相対評価の performance 軸を除外 (#128) ([#134](https://github.com/flipslidersand-labs/safecode-arena/pull/134))
- **analysis**: ESLint 複数形サマリ行を認識する ([#151](https://github.com/flipslidersand-labs/safecode-arena/pull/151))
- **generator**: ```rustc タグがコード先頭に混入する (#142) ([#154](https://github.com/flipslidersand-labs/safecode-arena/pull/154))
- **wasm**: proc_exit テスト fixture の memory export 不足と trap 原因の欠落 (#148) ([#153](https://github.com/flipslidersand-labs/safecode-arena/pull/153))
- **config**: reject unknown keys in safecode.toml ([#155](https://github.com/flipslidersand-labs/safecode-arena/pull/155))
- **pipeline**: ESLint 実行失敗を指摘 0 件にしない (#152) ([#156](https://github.com/flipslidersand-labs/safecode-arena/pull/156))
- **pipeline**: JS テストで candidate.js を node --test に渡さない (#159) ([#163](https://github.com/flipslidersand-labs/safecode-arena/pull/163))
- **runner**: タイムアウト時にプロセスグループごと kill する (#162) ([#164](https://github.com/flipslidersand-labs/safecode-arena/pull/164))
- **generator**: doc コメント内の ``` で候補を切断しない (#160) ([#165](https://github.com/flipslidersand-labs/safecode-arena/pull/165))
- **wasm**: 実行に壁時計のタイムアウトを追加する (#161) ([#166](https://github.com/flipslidersand-labs/safecode-arena/pull/166))
- **pipeline**: ruff/go vet の未導入・実行失敗を lint Skipped にする (#158) ([#167](https://github.com/flipslidersand-labs/safecode-arena/pull/167))
- **main**: evaluate で 1 候補のエラーが全体を中断しない (#168) ([#173](https://github.com/flipslidersand-labs/safecode-arena/pull/173))
- **pipeline**: ベンチ候補の compile で benches もビルドする (#171) ([#174](https://github.com/flipslidersand-labs/safecode-arena/pull/174))
- cargo audit が結果を返せない場合は audit 項を採点から除外 ([#183](https://github.com/flipslidersand-labs/safecode-arena/pull/183))
- 回帰検出時は run を DB に保存しない ([#184](https://github.com/flipslidersand-labs/safecode-arena/pull/184))
- **cli**: --wasm-max-memory-mb を 1..=4096 に検証 ([#185](https://github.com/flipslidersand-labs/safecode-arena/pull/185))
- --db 指定時、評価失敗で脱落した前回候補を回帰として exit 1 (#181) ([#186](https://github.com/flipslidersand-labs/safecode-arena/pull/186))
- --prop-tests が同名の --tests ファイルを上書きしないようにする ([#187](https://github.com/flipslidersand-labs/safecode-arena/pull/187))
- **llm**: build_prompt で source 内の {TEST_RESULT} を書き換えない ([#192](https://github.com/flipslidersand-labs/safecode-arena/pull/192))
- **llm**: parse_review が前置き・後置き付き応答を読めるようにする ([#193](https://github.com/flipslidersand-labs/safecode-arena/pull/193))
- レビュー未取得の候補を reasoning 0点扱いにしない ([#194](https://github.com/flipslidersand-labs/safecode-arena/pull/194))
- **store**: 実行オプションが同じ run とだけ回帰比較する ([#195](https://github.com/flipslidersand-labs/safecode-arena/pull/195))
- **pipeline**: ESLint 9+ で JS の lint を実際に動かす (#152) ([#201](https://github.com/flipslidersand-labs/safecode-arena/pull/201))
- **runner**: 子プロセスの環境変数を許可リスト方式にする ([#232](https://github.com/flipslidersand-labs/safecode-arena/pull/232))
- **store**: 回帰比較から reasoning 軸を除く ([#233](https://github.com/flipslidersand-labs/safecode-arena/pull/233))
- **cli**: --regression-threshold と重みに有限値・範囲の検証を追加 ([#235](https://github.com/flipslidersand-labs/safecode-arena/pull/235))
- **scoring**: bench 付きと無しが混在するとき bench 無しの候補を 0 点にしない (#240) ([#245](https://github.com/flipslidersand-labs/safecode-arena/pull/245))
- **pipeline**: テスト結果行を確認し exit(0) による合格偽装を検出する (#226 PR2) ([#246](https://github.com/flipslidersand-labs/safecode-arena/pull/246))
- **pipeline**: lint 抑止コメントで警告が 0 件になる抜け穴を塞ぐ (#237) ([#247](https://github.com/flipslidersand-labs/safecode-arena/pull/247))
- **report**: 候補由来の文字列によるコードフェンス脱出を防ぐ (#230) ([#249](https://github.com/flipslidersand-labs/safecode-arena/pull/249))
- **runner**: 子プロセス出力の取り込みと出力ファイルに上限を設ける (#227) ([#250](https://github.com/flipslidersand-labs/safecode-arena/pull/250))
- **pipeline**: bench 時間を stdout でなく Criterion の estimates.json から読む (#239) ([#254](https://github.com/flipslidersand-labs/safecode-arena/pull/254))
- **main**: 回帰比較キーに tests/prop-tests/重みを含める (#243) ([#255](https://github.com/flipslidersand-labs/safecode-arena/pull/255))
- **llm**: 応答中の最後の有効な採点 JSON を採用する ([#258](https://github.com/flipslidersand-labs/safecode-arena/pull/258))
- **analysis**: cargo のクレート単位まとめ行を lint 警告に数えない (#248) ([#259](https://github.com/flipslidersand-labs/safecode-arena/pull/259))
- **report**: candidate_id のインラインコードをバッククォート安全にする ([#261](https://github.com/flipslidersand-labs/safecode-arena/pull/261))
- **ci**: reusable workflow の固定 SHA を qa-workflows main 上の同内容コミットへ差し替える ([#267](https://github.com/flipslidersand-labs/safecode-arena/pull/267))
- **report**: Markdown 表・見出しの candidate_id をエスケープ ([#279](https://github.com/flipslidersand-labs/safecode-arena/pull/279))
- **llm**: 異なる採点 JSON が複数ある応答と長大応答を拒否 ([#280](https://github.com/flipslidersand-labs/safecode-arena/pull/280))
- **pipeline**: テストの同名ファイルによる候補上書きを衝突エラーにする ([#282](https://github.com/flipslidersand-labs/safecode-arena/pull/282))
- 回帰比較キーの穴を塞ぐ (wasm 上限・NULL 旧 run・統合テスト) ([#281](https://github.com/flipslidersand-labs/safecode-arena/pull/281))
- 出力切り詰め時は lint 件数を警告過多として最低点にする ([#284](https://github.com/flipslidersand-labs/safecode-arena/pull/284))
- 0.5ns 未満の bench median が 0ns に丸められる問題 ([#287](https://github.com/flipslidersand-labs/safecode-arena/pull/287))
- **pipeline**: ベンチ候補の test/clippy に --benches、Criterion toml に proptest (#231) ([#288](https://github.com/flipslidersand-labs/safecode-arena/pull/288))
