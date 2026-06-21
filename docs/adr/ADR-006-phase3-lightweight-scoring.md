# ADR-006: Phase 3 の多軸採点は clippy + 軽量解析で実装する

- **日付**: 2026-06-21
- **状態**: Accepted

## 背景

Phase 3 は correctness 以外の軸（security / performance / maintainability）を実測する。実装ガイドの当初案は Tree-sitter（AST 解析）・Criterion（ベンチ）・cargo-audit（脆弱性 DB）を用いるものだったが、これらは C ツールチェーン依存・外部 DB・ベンチ整備が必要で導入コストが大きい。一方 `cargo clippy` は Rust ツールチェーンに同梱され、security/maintainability の両方に効くシグナルを安価に得られる。

## 決定

clippy と軽量な静的解析・相対比較を組み合わせて各軸を実測する。重量級依存（Tree-sitter / Criterion / cargo-audit）は後続フェーズへ繰り延べる。

- **correctness**: compile 40% + test 40% + property test 20%（proptest、`--prop-tests` 指定時のみ実行）。
- **security**: `unsafe` ヒューリスティック 50% + clippy 50%（warning 1 件 -0.1、clippy 自体の失敗で 0）。
- **maintainability**: 関数長ヒューリスティック 60% + clippy 40%（warning 3 件ごと -0.1）。
- **performance**: 候補間の `compile + test` 所要時間の相対比較（最速に満点、他は按分）。
- **resource_usage**: 当面未計測（0）。
- security / maintainability は **compile 成功時のみ加点**（ビルドできないコードの静的品質を採用判断に使う意味がないため）。

各軸は `model::AxisScores` として独立に保持し、レポート/JSON に内訳を出す。clippy は専用ステージとして実行し（`runner::run_stage_capture` で stderr を捕捉）、warning 数を `analysis::count_clippy_warnings` で数える。

## 理由

- clippy は追加バイナリ不要で、unused/不適切なパターン/`unsafe` などを一括検出でき、security と maintainability の両軸に再利用できる。
- ヒューリスティック部分（`analysis`）と配点ロジック（`scoring`）は純関数で、外部依存なしにユニットテストできる。
- `AxisScores` を中心に据えたことで、後から Tree-sitter 版 maintainability や Criterion 版 performance を軸単位で差し替えられる。
- property test は opt-in にすることで、通常の評価では proptest の取得・ビルドコストを発生させない。

## トレードオフ

- ヒューリスティックは粗い。真の循環的複雑度・実際の脆弱性 CVE・絶対性能は捉えない。
- clippy の警告数を security/maintainability 両方に使うため、両軸が相関する（独立な指標ではない）。
- performance は候補集合内の相対値であり、単一候補では常に満点になる。
- `unsafe` カウントは文字列一致で、コメントや文字列リテラル中の語も数える（誤検知あり）。Tree-sitter 導入時に解消予定。
- captured stderr は先頭 20 行に要約されるため、警告が非常に多い場合は warning 数を過小カウントしうる。
