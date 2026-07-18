# ADR-008: 多言語対応は言語別ステージランナーで実装する

- **日付**: 2026-06-24
- **状態**: Accepted

## 背景

Phase 5（発展）として Rust 以外の言語（まず Python）を検証対象に加える。これまでパイプラインは Cargo/clippy/wasm を前提に Rust 専用で組まれていた。`model::Language` は `Rust` のみのスタブだった。

利用可能なツール: python3 3.12 / pytest 9.1 / ruff 0.15。

## 決定

ファイル拡張子から言語を判定し（`Language::from_extension`、`.py` → Python）、言語ごとのステージランナーに分岐する。

- 共通の中間表現 `StageResults`（compile / test / lint / lint_warnings / prop_test / wasm）を各ランナーが埋め、`assemble` が言語非依存に採点する。
- **Rust** (`run_rust_stages`): 既存どおり Cargo プロジェクト + clippy + proptest + wasm。
- **Python** (`run_python_stages`):
  - compile = `python3 -m py_compile`（構文チェック）
  - test = `pytest`（テスト未収集 exit 5 は成功扱い。Rust の 0 tests と同様）
  - lint = `ruff check`（指摘があってもステージは成功、件数のみ採点に使用）
  - prop_test / wasm は非対応 → Skipped
- `Evaluation` の `clippy` / `clippy_warnings` フィールドを「言語のリンター結果」として再利用する（Python では ruff）。
- 静的解析（`SourceMetrics`）の関数カウントを言語横断化（`fn ` / `def ` / `function `）。

## 理由

- `StageResults` + `assemble` により採点ロジックを 1 箇所に保ち、言語追加は「ステージを埋めるランナー」を足すだけにできる（Go/JS も同パターンで追加可能）。
- ruff は exit 1 を返すが、clippy 同様「通過＋警告数」のセマンティクスに寄せることで、既存の security/maintainability 配点（clippy 連動）をそのまま再利用できる。
- 性能軸は候補間の wall-clock 相対比較なので、Rust と Python を 1 回の run で横断比較できる（インタプリタ起動分 Python は不利になるが、相対指標として妥当）。

## トレードオフ

- `Evaluation.clippy` という名称が Python では ruff を指し、意味と名前がずれる（互換維持のため改名せず、ドキュメントで補足）。
- Python はインライン test を自動収集しない（pytest は `test_*.py` のみ収集）。候補自身のテストは外部 `--tests` で渡す必要がある（Rust の `#[cfg(test)]` と非対称）。
- ruff / pytest 未インストール環境ではステージが空振りする（lint は 0 件・成功扱いに退避）。
- 性能比較が言語をまたぐと、インタプリタ言語が構造的に不利。将来は言語内での相対比較に分けることを検討。
