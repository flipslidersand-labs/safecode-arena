# Implementation Guide — SafeCode Arena

## Phase 1: コンパイル実行（1週）

### 実装内容

- `src/compiler.rs` — `rustc <file> --edition 2021 -o /tmp/out` をサブプロセスで実行
- `tempfile::TempDir` で一時ディレクトリを作成、Drop 時に自動削除
- `tokio::time::timeout` でコンパイルタイムアウトを検出

### 完成条件

```bash
safecode-arena eval --file examples/hello.rs
# compile_ok: true, duration_ms: 1234
safecode-arena eval --file examples/broken.rs
# compile_ok: false, stderr: "error[E0308]: ..."
```

---

## Phase 2: テスト実行（1週）

### 実装内容

- `src/tester.rs` — `cargo test` を `tokio::process::Command` で実行
- stdout から `test result: ok. N passed; M failed` をパース
- タイムアウト時はプロセスグループごと `SIGKILL`

### 完成条件

```bash
safecode-arena eval --file examples/with_tests.rs
# test_passed: 3, test_failed: 0, timed_out: false
```

---

## Phase 3: スコアリング + SQLite 保存（3日）

### 実装内容

- `src/scorer.rs` — `ScoreWeights` × 各指標の生スコアで加重平均を計算
- `src/store.rs` — rusqlite で `submissions` / `eval_results` テーブルに保存

### 完成条件

```bash
safecode-arena eval --file candidate.rs --spec spec.yaml
# score: 72.5 / 100
# saved to results.db
```

---

## Phase 4: Markdown レポート生成（3日）

### 実装内容

- `src/reporter.rs` — 評価結果を Markdown 表形式で `report.md` に出力
- `safecode-arena compare --dir ./candidates/` で複数候補を比較

### 完成条件

```markdown
## SafeCode Arena Report

| Candidate      | Score | Correctness | Security | Performance |
| -------------- | ----- | ----------- | -------- | ----------- |
| candidate_a.rs | 85.0  | 50.0        | 18.0     | 12.0        |
| candidate_b.rs | 72.5  | 45.0        | 15.0     | 10.0        |
```

---

## Phase 5: Wasmtime サンドボックス（1〜2週）

### 実装内容

- `src/sandbox.rs` — コードを Wasm にコンパイルし Wasmtime + WASI で実行
- WASI の `preopened_dirs` を空にしてファイルアクセスを禁止
- ネットワーク接続は WASI Preview 2 の Capability でブロック

### 完成条件

```bash
# ファイルシステムにアクセスしようとするコードを検証
safecode-arena eval --file malicious.rs --sandbox
# sandbox violation: file access denied
```

---

## Phase 6: Proptest 統合（1週）

### 実装内容

- 候補コードに `proptest` のテストが含まれているか検出
- `cargo test --features proptest` で Property-based test を実行
- フェイル時のシュリンク済み入力を `TestResult` に記録

---

## 実装順序の根拠

コンパイル→テスト→スコア→レポートの順は「評価パイプライン」の自然な流れ。
Wasm サンドボックスはアーキテクチャを大きく変える変更なので Phase 5 に後回しにし、
まず「正しく動く評価器」を Phase 4 で完成させてから安全性を強化する。
