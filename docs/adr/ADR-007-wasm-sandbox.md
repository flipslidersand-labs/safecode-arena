# ADR-007: Wasm サンドボックスは埋め込み wasmtime + wasip1 で実装する

- **日付**: 2026-06-24
- **状態**: Accepted

## 背景

Phase 4 の目標のひとつは「信頼できる隔離」。Phase 1〜3 のプロセス隔離（一時ディレクトリ + タイムアウト kill、ADR-002）は同一ユーザ権限で動くため、ファイル/ネットワークアクセスを完全には封じられない。実装ガイドは Wasmtime + WASI による capability ベース隔離を予定していた。

環境調査の結果:

- `wasm32-wasip1` / `wasm32-wasip2` ターゲットはインストール済み。
- wasmtime CLI / cargo-wasi は未インストール。

## 決定

候補を `wasm32-wasip1` にビルドし、**埋め込み `wasmtime` クレート**（CLI 非依存）で実行する Wasm ステージを追加する。

- 一時プロジェクトに生成ハーネス `src/main.rs`（`fn main() { candidate::<entry>(); }`）を書き、`cargo build --target wasm32-wasip1 --release` で `.wasm` を得る。
- 実行ランタイムは capability ゼロ: `WasiCtxBuilder::new().build_p1()`（preopen なし・env なし・ネットワークなし）。
- 命令数上限は `Config::consume_fuel(true)` + `Store::set_fuel` の **fuel**、メモリ上限は `StoreLimitsBuilder`。
- `_start`（WASI command）を呼び、`I32Exit(0)` / 正常終了 → Passed、fuel 枯渇（`Trap::OutOfFuel`）→ TimedOut、その他 trap / 非ゼロ exit → Failed。
- ステージは **`--wasm-entry` 指定時のみ**実行（未指定は Skipped）。`resource_usage` 軸（従来 0）の採点に使う。

## 理由

- 埋め込みクレートなら外部バイナリのインストールを要求せず、決定的に隔離実行できる。
- fuel は wall-clock ではなく命令数ベースで**決定的**なため、CI でも再現性がある（ユニットテストを wat で書ける）。
- capability ゼロの WASI は「信頼できない第三者コードの実行」という本プロジェクトの主目的に直結する。
- エントリ gated にすることで、既存の評価フロー（Phase 1〜3）に回帰を出さない。

## トレードオフ

- `wasmtime` クレートはビルドが非常に重い（cranelift 等、初回コンパイル数分・高メモリ）。CI 時間とビルド資源に影響する。
- WASI preview1 で動かすには候補が `wasm32-wasip1` でビルドできる必要がある。スレッド/ネットワーク等 wasi 非対応の std 機能を使う候補はビルド失敗（Wasm = Failed）となる — これは「サンドボックス可搬性」のシグナルとして妥当だが、ネイティブでは動く候補を弾く場合がある。
- 任意エントリ関数（no-arg）を要求するため、引数や戻り値で検証したいコードはハーネス側の工夫が要る。
- `resource_usage` は当面 Wasm 実行の成否による binary 採点（満点/0）。候補間の fuel 相対比較（`scoring::assign_performance` と同じパターン）は将来拡張。
