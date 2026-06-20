# ADR-002: Phase 5 のサンドボックスに Wasmtime + WASI を使う

- **日付**: 2026-06-20
- **状態**: Accepted

## 背景

Phase 5 でコードを安全に実行するサンドボックスが必要。
選択肢: Docker コンテナ / seccomp / Wasmtime + WASI / gVisor。

## 決定

`wasmtime` + WASI Preview 2 を使う。

## 理由

- Docker はデーモン依存・起動オーバーヘッドが大きく、候補ごとのコンテナ起動はコストが高い
- seccomp は syscall 単位のフィルタで設定が煩雑、かつ Root で動くリスクが残る
- Wasmtime は Capability ベースで「この Wasm だけにこのディレクトリを許可」という細粒度制御が可能
- fluxion (X1) で Wasmtime を使う予定があるため、学習の共通化ができる

## トレードオフ

- `rustc --target wasm32-wasi` でコンパイルすると標準ライブラリの一部が使えない
- WASI Preview 2 の Capability は仕様策定中で API が変わる可能性がある
