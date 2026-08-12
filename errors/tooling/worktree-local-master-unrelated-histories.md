---
title: "worktree 経由で PR を出し続けるとローカル master が unrelated histories になる"
tags: [git, worktree]
severity: medium
date: "2026-08-12"
---

## 症状

全 PR を worktree 経由で作成・マージした後、ローカル master に `git pull` すると:

```
fatal: refusing to merge unrelated histories
```

ローカル master は initial commit 1 本のみ、origin/master は 20+ コミット先行している。

## 原因

worktree は独立した HEAD を持ち、ローカル master ブランチには一切コミットされない。
PR を origin/master にマージしても、ローカル master は初期コミットのまま取り残される。
その後 `git pull` すると、origin/master と共通祖先がないため unrelated histories になる。

## 解決策

セッション終了時に以下を実行してローカルを同期する：

```bash
# staged changes があればまず stash
git stash

# ローカル master を origin に強制同期（staged changes は既に origin に入っている）
git fetch origin
git reset --hard origin/master

# stash は不要なら drop
git stash drop
```

または worktree での作業開始前に毎回 `git pull` しておく。

## 予防

- worktree で作業 → PR マージ → セッション終了前に必ずローカル master を sync する
- `git branch --show-current && git status` の確認をセッション開始時だけでなく終了時にも実行
