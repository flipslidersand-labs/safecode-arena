//! SafeCode Arena — AI生成コード検証ランナー
//!
//! 仕様 → 複数コード候補 → コンパイル → テスト → 採点 → レポート
//! というパイプラインを駆動するコアドメイン。
//!
//! MVP の責務:
//! - Rust コード候補を一時ディレクトリへ展開
//! - `cargo build` / `cargo test` をタイムアウト付きで実行
//! - 結果を採点ルーブリックに従ってスコア化
//! - Markdown レポートを生成

pub mod analysis;
pub mod config;
pub mod model;
pub mod pipeline;
pub mod report;
pub mod runner;
pub mod scoring;
pub mod store;

pub use model::{Candidate, Evaluation, Language, StageOutcome};
