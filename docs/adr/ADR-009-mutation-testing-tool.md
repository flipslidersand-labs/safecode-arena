# ADR-009: Mutation Testing ツール選定 — cargo mutants 採用

## ステータス

採用済み（2026-08-11）

## コンテキスト

Phase 8 でミューテーションテストを追加する。対象言語は Rust（Go/Python は将来拡張）。
以下の 3 ツールを検討した。

| ツール          | 言語   | 最終更新       | JSON出力    | パイプライン追加コスト |
| --------------- | ------ | -------------- | ----------- | ---------------------- |
| `cargo mutants` | Rust   | 活発（2024〜） | ✅ `--json` | 低                     |
| `mutmut`        | Python | 中程度         | △（DB経由） | 中                     |
| `go-mutesting`  | Go     | 低調（2020〜） | ❌          | 高                     |

## 決定

**`cargo mutants` を採用し、Rust 候補のみを対象とする。**

## 採用理由

1. **Cargo エコシステム完全統合**: `--manifest-path` で一時プロジェクトを指定するだけで動く。既存の一時 Cargo プロジェクト生成ロジックをそのまま再利用できる。
2. **機械可読な JSON 出力**: `--json` フラグで `total_mutants / caught / missed / timeout` を構造化出力。正規表現パースが不要。
3. **活発なメンテナンス**: 2024年時点で定期リリースが続いており、wasmtime のような突然の破壊的変更リスクが低い。
4. **自己検証可能**: safecode-arena 自体が Rust プロジェクトなので、ツールを自分自身に適用してデバッグできる。

## 却下理由

- **`mutmut`**: Python 候補への適用は有効だが、`setup.cfg` / `pyproject.toml` の設定注入が必要で一時プロジェクト構造への変更量が多い。Python は将来 Phase で対応。
- **`go-mutesting`**: メンテナンスが低調で JSON 出力がない。`gremlins` の方が有望だが実績が少ない。

## トレードオフ

- mutation testing は実行時間が長い（候補のコード量 × ミュータント数）。デフォルト off にして `--mutation` フラグで明示的に有効化することで対処する。
- `cargo mutants` が PATH にない場合は `Skipped` として減点しない（CI 環境でのインストール要件を任意にする）。

## スコアリングへの影響

mutation を有効化した場合、`correctness` 軸の内訳を再配分する（合計は変わらない）:

| ステージ  | mutation なし | mutation あり |
| --------- | ------------- | ------------- |
| compile   | 40%           | 30%           |
| test      | 40%           | 30%           |
| prop_test | 20%           | 15%           |
| mutation  | —             | 25%           |
