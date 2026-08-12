---
title: "cargo mutants が missed mutants のとき exit 1 かつ valid JSON を出力する"
tags: [rust, mutation-testing, cargo-mutants]
severity: high
date: "2026-08-12"
---

## 症状

`cargo mutants --json` を実行すると、ミュータントが missed（テストで捕捉されなかった）場合に
exit code 1 を返すが、同時に valid な JSON 出力も stdout に書き出す。

## 原因

cargo mutants の仕様：テストが全ミュータントを捕捉した場合のみ exit 0。
1 件でも missed があれば exit 1 だが、集計結果の JSON は常に出力される。

これは「テスト品質が低い = 失敗」という設計判断。

## 解決策

`run_stage_capture` の outcome を JSON の有無に関わらず優先する。

```rust
// ❌ 誤り: JSON が取れれば Passed に書き換える
if !outcome.is_passed() {
    if let Some((caught, total)) = parse_mutants_json(&stdout) {
        return (StageOutcome::Passed { ... }, caught, total);
    }
}

// ✅ 正しい: outcome はそのまま返す。JSON があれば caught/total だけ取る
if !outcome.is_passed() {
    return (outcome, 0, 0);
}
match parse_mutants_json(&stdout) {
    Some((caught, total)) => (outcome, caught, total),
    None => (outcome, 0, 0),
}
```

missed mutants = テスト品質の問題 = scoring 上 Failed が正しい分類。

## 予防

外部ツールの exit code 意味論を必ず確認する。
「non-zero = 致命的エラー」ではなく「non-zero = 品質基準を満たさない」という設計もある。
