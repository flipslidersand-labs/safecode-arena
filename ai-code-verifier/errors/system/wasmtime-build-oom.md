---
title: "wasmtime 依存のビルドがメモリ枯渇で kill される (exit 137)"
tags: [rust, wasmtime, build, oom, memory]
severity: medium
date: "2026-06-27"
---

## 症状

`wasmtime` / `wasmtime-wasi` を依存に追加後、`cargo build` が `exit code 137` で停止する。
コンパイルログは出力途中で途切れ、エラーメッセージは残らない。

## 原因

exit 137 = SIGKILL（128+9）。OOM killer による強制終了。wasmtime（cranelift 等）は
コンパイルが非常に重く、cargo がフル並列で多数の rustc を同時起動するとメモリを使い切る。

## 解決策

並列度を下げてビルドする。

```bash
CARGO_BUILD_JOBS=2 cargo build -j2
```

初回のみ重い（数分）。以降は増分ビルドで軽い。

## 予防

- wasmtime を含むリポは常に `-j2`（または環境メモリに応じた小さめ並列）でビルドする。
- CI でも並列度を絞る。`cargo test` も同様（テストバイナリのリンクで再度重くなる）。
- 出力途中で落ちて exit 137 を見たら、まず OOM を疑い並列度を下げる。
