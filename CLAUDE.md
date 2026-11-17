# CLAUDE.md

判断軸と規約は [AGENTS.md](AGENTS.md) の指す先を読む。ここにはこのリポ固有の実コマンドと罠だけを書く。

## コマンド

```bash
cargo build
cargo test --lib --tests
cargo clippy --lib --tests --bins -- -D warnings
cargo fmt
```

ツールチェーンは `rust-toolchain.toml` で固定している。CI（`.github/workflows/ci.yml`）も同じ引数で test と clippy を実行する。

## 罠

- 素の `cargo test` / `cargo clippy` は使わない。`examples/` は評価対象のサンプル候補（わざと壊した `broken.rs` / `buggy.rs` を含む）で、`main` を持たないため Cargo の example としてはビルドできない。
- clippy は `-D warnings`。定数どうしの `assert!` は `assertions_on_constants` で落ちる。
- `src/model.rs` を変えたら `docs/data-model.md` を、既定の重みを変えたら `docs/spec.md` と README の採点表を合わせる。
- 採点式を変えたら README.md / README.ja.md / `docs/spec.md` / `docs/data-model.md` の 4 か所を同時に直す。
- 環境変数を書き換えるテストは並列実行で競合する。`src/llm_client.rs` のテストは同モジュール内の非公開ロック（`ENV_LOCK`）で直列化している。他のモジュールや `tests/` からは参照できないので、そこで環境変数を書き換えるテストは足さない（必要ならロックを共有できる形にしてから）。

## 触らない箇所

- `AGENTS.md`（人だけが更新する）。
- `.github/workflows/` の reusable workflow の固定 SHA と `runs-on`。変更するときは単独の PR にし、人がマージする。
- `examples/broken.rs` と `examples/buggy.rs` を「直す」こと。失敗する候補のサンプルで、`buggy.rs` は `.github/workflows/eval.yml` の評価対象になっている。
