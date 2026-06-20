//! SafeCode Arena CLI — `safecode`
//!
//! MVP コマンド:
//!   safecode evaluate <candidate.rs> [--tests <dir>] [--timeout-secs N] [--out report.md]

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "safecode", version, about = "AI生成コード検証ランナー")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 1 つ以上のコード候補を検証・採点する。
    Evaluate {
        /// 検証する Rust ソースファイル（複数指定可）。
        candidates: Vec<String>,
        /// テストディレクトリ（任意）。
        #[arg(long)]
        tests: Option<String>,
        /// 各ステージのタイムアウト秒数。
        #[arg(long, default_value_t = 60)]
        timeout_secs: u64,
        /// レポート出力先（省略時は標準出力）。
        #[arg(long)]
        out: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Evaluate {
            candidates,
            tests,
            timeout_secs,
            out,
        } => {
            // TODO(Phase 1): 候補読み込み → runner で compile/test → scoring → report
            // 現状はパイプラインの骨格のみ。implementation-guide.md の Phase 1 で実装。
            eprintln!(
                "evaluate: {} 候補, tests={:?}, timeout={}s, out={:?}",
                candidates.len(),
                tests,
                timeout_secs,
                out
            );
            eprintln!("(Phase 1 未実装: パイプライン本体をここに実装する)");
            Ok(())
        }
    }
}
