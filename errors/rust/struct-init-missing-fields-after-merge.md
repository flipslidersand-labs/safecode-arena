---
title: "struct フィールド追加後に別ブランチ merge でビルドエラー"
tags: [rust, struct, merge]
severity: medium
date: "2026-08-13"
---

## 症状

```
error[E0063]: missing fields `reasoning_comment` and `reasoning_score` in initializer of `StageResults`
   --> src/pipeline.rs:822:19
```

phase13 (TypeScript パイプライン) と phase14 (LLM review + reasoning フィールド追加) を
それぞれ独立した worktree で開発し、両方 master にマージした後に発生。

## 原因

phase13 ブランチは phase14 より先に分岐していたため、`StageResults` に
`reasoning_score: Option<f64>` / `reasoning_comment: Option<String>` が追加された
phase14 の変更を含んでいなかった。
GitHub 上の squash merge は struct の定義側 (phase14) を取り込んだが、
初期化側 (phase13 の TypeScript pipeline 関数) は古いまま残った。

## 解決策

`run_typescript_stages` 内の 2 箇所の `StageResults { ... }` 初期化に
`reasoning_score: None, reasoning_comment: None` を追加。

```rust
Ok(StageResults {
    // ... 既存フィールド
    bench_ns: None,
    reasoning_score: None,   // 追加
    reasoning_comment: None, // 追加
})
```

## 予防

struct に新フィールドを追加するときは、`#[non_exhaustive]` を付けないかぎり
既存の全初期化箇所がコンパイルエラーになるため、追加したブランチ内で
`grep -rn "StageResults {" src/` を実行して全箇所を確認する。
worktree 並行開発時は merge 後に必ず `cargo build` を実行してから push する。
