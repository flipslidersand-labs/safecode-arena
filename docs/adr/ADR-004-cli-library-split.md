# ADR-004: CLI（bin）とコアドメイン（lib）を分離する

- **日付**: 2026-06-20
- **状態**: Accepted

## 背景

検証ロジック（候補実行・採点・レポート）は、CLI だけでなく将来の MCP サーバや GitHub PR 連携からも呼び出したい。CLI に処理を直書きすると再利用とテストが困難になる。

## 決定

`src/lib.rs`（`safecode_arena`）にコアドメイン（model / runner / scoring / report）を置き、`src/main.rs`（`safecode`）は薄い CLI アダプタに限定する。

## 理由

- ライブラリは `assert_cmd` を介さず直接ユニットテストできる。
- Phase 5+ の MCP サーバ化・PR 連携で同じコアを再利用できる。
- 関心の分離により、各モジュール（runner / scoring）を独立に進化させられる。

## トレードオフ

- 小規模なうちは bin/lib 2 ファイル構成がややオーバーヘッドに見える。
- 公開 API（lib の pub 境界）を意識する必要があり、内部変更時の影響範囲管理が増える。
