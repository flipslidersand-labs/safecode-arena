# SafeCode Arena — 実装ガイド

各 Phase は「動く状態」で区切る。前 Phase の完成条件を満たしてから次へ進む。

---

## Phase 1: 一気通貫のパイプライン（最初の完成条件）

**目標**: `safecode evaluate candidate.rs` で compile → test → 採点 → レポートが通る。

### ステップ

1. `Candidate` をファイルから読み込む（`id` はファイル名 stem）。
2. `tempfile::TempDir` に最小 Cargo プロジェクトを生成し、`source` を `src/lib.rs`（または指定）へ配置。
3. `runner::run_stage` で `cargo build` を実行 → `compile: StageOutcome`。
4. compile が Passed なら `cargo test`（`--tests` 指定があればコピー）→ `test: StageOutcome`。失敗なら test は `Skipped`。
5. `scoring::score` で採点 → `Evaluation`。
6. `report::render` で Markdown 生成。`--out` 指定時はファイル、なければ stdout。

### 完成条件（動作確認）

```bash
# 成功する候補
echo 'pub fn add(a:i32,b:i32)->i32{a+b}' > /tmp/ok.rs
safecode evaluate /tmp/ok.rs            # → スコア > 0、レポート出力

# コンパイルが通らない候補
echo 'pub fn broken(' > /tmp/ng.rs
safecode evaluate /tmp/ng.rs            # → compile ❌、test skipped、スコア 0
```

### 難所と対策

- **一時 Cargo プロジェクト生成**: `Cargo.toml` を文字列テンプレートで書き出す。依存なしの最小構成から。
- **テスト配置**: MVP は「候補内 `#[cfg(test)]`」を優先。外部 `--tests` ディレクトリは Phase 2。

---

## Phase 2: タイムアウト・複数候補・設定

**目標**: 暴走コードを確実に止め、複数候補をランキングする。

### ステップ

1. `wait_timeout` クレート導入で、`run_stage` を本物のタイムアウト kill に置換（現状は事後判定）。
2. 複数 `candidates` を受け取り、各々を評価 → `scoring::rank` でランキング。
3. 採点ルーブリックを `safecode.toml` から読み込み（重みの外部化）。
4. `--format json` 対応。

### 完成条件

```bash
echo 'pub fn f(){loop{}}' > /tmp/loop.rs
safecode evaluate /tmp/loop.rs --timeout-secs 3   # → 3秒で ⏱ timeout

safecode evaluate a.rs b.rs c.rs --out report.md   # → 3候補がスコア順に並ぶ
```

---

## Phase 3: 多軸採点（security / performance / maintainability）

**目標**: correctness 以外の軸を実測する。

### ステップ

1. **maintainability**: `tree-sitter` で AST を解析し、関数長・複雑度・命名を採点。
2. **performance**: `criterion` ベンチを候補に対して実行し、相対スコア化。
3. **security**: `cargo clippy` + `cargo audit` の結果を集計。`unsafe` 使用にペナルティ。
4. **property test / fuzzing**: `proptest` / `cargo-fuzz` を任意実行。

### 完成条件

各軸が 0 でない実測値で採点され、レポートに軸別内訳が出る。

---

## Phase 4: サンドボックス強化・永続化

**目標**: 信頼できる隔離と履歴管理。

### ステップ

1. `wasmtime` + WASI で候補を WebAssembly 化して実行（capability ベース）。
2. `rusqlite` で run/candidate/evaluation を永続化。
3. 過去 run との差分比較（リグレッション検出）。

---

## 発展（Phase 5+）

- Python / Go / JavaScript 対応
- Mutation Testing / Differential Testing
- 複数 AI API 連携（候補の自動生成）
- コード類似度分析
- GitHub Pull Request 連携

---

## 実装順序の根拠

- まず **correctness の一気通貫**を作ることで、パイプライン全体の I/O とレポート形を確定させる（最も価値が高く、他軸の土台になる）。
- タイムアウトは「危険なコードを安全に扱う」本質なので Phase 2 で確実に。
- 多軸採点は土台が固まってから足す（各軸は独立に追加可能な設計にしてある）。
- サンドボックス強化（Wasm）は重く、MVP の価値を出した後に回す。
