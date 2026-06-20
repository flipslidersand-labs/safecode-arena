# ADR-006: Phase 3 の多軸採点は軽量な静的解析・相対比較から始める

- **日付**: 2026-06-21
- **状態**: Accepted

## 背景

Phase 3 は correctness 以外の軸（security / performance / maintainability）を実測する。実装ガイドの当初案は Tree-sitter（AST 解析）・Criterion（ベンチ）・cargo-audit（脆弱性）を用いるものだったが、これらは C ツールチェーン依存・外部バイナリ依存・ベンチ整備が必要で、導入コストと CI への影響が大きい。

## 決定

外部重量依存を即時導入せず、決定的で軽量な実装から始める。

- **security**: ソース中の `unsafe` 出現数で減点（1 つにつき 0.25）。
- **maintainability**: 関数あたり平均行数・最長行長によるヒューリスティック。
- **performance**: 候補間の `compile + test` 所要時間の相対比較（最速に満点、他は按分）。
- **resource_usage**: 当面未計測（0）。
- security / maintainability は **compile 成功時のみ加点**（ビルドできないコードの静的品質を採用判断に使う意味がないため）。

各軸は `model::AxisScores` として独立に保持し、レポート/JSON に内訳を出す。

## 理由

- 外部依存なしで決定的 → ユニットテストが容易（`analysis` / `scoring` を純関数で検証）。
- まず「動く多軸採点」を完成させ、各軸の精度は後続で差し替え可能にする（共通開発ルール「動く最小→段階的」）。
- `AxisScores` を中心に据えたことで、Tree-sitter 版 maintainability や Criterion 版 performance を後から軸単位で置換できる。

## トレードオフ

- ヒューリスティックは粗い。真の循環的複雑度・実際の脆弱性・絶対性能は捉えない。
- performance は候補集合内の相対値であり、単一候補では常に満点になる。
- `unsafe` カウントは文字列一致で、コメントや文字列リテラル中の語も数える（誤検知あり）。Tree-sitter 導入時に解消予定。
