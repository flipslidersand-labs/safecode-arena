---
title: "wasmtime のエラーを再ラップすると trap/exit が downcast できず誤判定"
tags: [rust, wasmtime, error-handling, downcast]
severity: high
date: "2026-06-27"
---

## 症状

`safecode evaluate --wasm-entry` で、無限ループ候補が fuel 枯渇しても Wasm ステージが
`TimedOut` ではなく `Failed` になる。正常終了（`exit(0)`）の候補も `Passed` にならず Failed
になりうる。エラー詳細には `all fuel consumed by WebAssembly` と出ているのに分類できていない。

## 原因

`TypedFunc::call` が返したエラーを `classify_error(&anyhow::anyhow!("{e:#}"), ...)` のように
**文字列へ再ラップ**していた。これで元の型情報が失われ、`downcast_ref::<wasmtime::Trap>()` /
`downcast_ref::<wasmtime_wasi::I32Exit>()` が常に `None` を返していた。

加えて wasmtime 46 の `wasmtime::Error` は `anyhow::Error` とは**別型**で、`&e` をそのまま
`&anyhow::Error` を受ける関数へ渡すと型エラー（E0308）になる。

## 解決策

- 再ラップをやめ、元のエラーをそのまま分類関数へ渡す。
- 分類関数の引数型は `&wasmtime::Error` にする（wasmtime 独自型の `downcast_ref` を使う）。
- fuel 枯渇は downcast に加えてメッセージ文字列 `contains("fuel consumed")` でフォールバック判定。

```rust
// 呼び出し側
Err(e) => classify_error(&e, elapsed_ms),

// 分類側
fn classify_error(e: &wasmtime::Error, elapsed_ms: u64) -> StageOutcome {
    if let Some(exit) = e.downcast_ref::<I32Exit>() { /* 0=Passed, else Failed */ }
    let out_of_fuel = e.downcast_ref::<Trap>() == Some(&Trap::OutOfFuel)
        || format!("{e:#}").contains("fuel consumed");
    // ...
}
```

## 予防

- 他クレートが返すエラーは**型を保ったまま**扱う。`anyhow!("{e:#}")` 等で文字列化すると
  `downcast` が効かなくなる。
- `downcast_ref` は呼び出し対象のエラー型（ここでは `wasmtime::Error`）に対して行う。
  型エイリアスだと思い込まず、コンパイルエラーが出たら実際の型を確認する。
- trap 種別の判定は downcast を主とし、ラップ経路の不確実性に備えてメッセージ照合を保険にする。
