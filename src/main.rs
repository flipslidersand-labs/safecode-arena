//! SafeCode Arena CLI — `safecode`
//!
//! MVP コマンド:
//!   safecode evaluate <candidate.rs> [--tests <dir>] [--timeout-secs N] [--out report.md]

use clap::{Parser, Subcommand};
use safecode_arena::{pipeline, report, scoring};
use std::path::Path;
use std::time::Duration;

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
            // tests は Phase 2 で外部テストディレクトリ対応時に使用する。
            tests: _,
            timeout_secs,
            out,
        } => {
            if candidates.is_empty() {
                anyhow::bail!("候補ファイルを 1 つ以上指定してください");
            }
            let timeout = Duration::from_secs(timeout_secs);

            let mut evals = Vec::with_capacity(candidates.len());
            for path in &candidates {
                let candidate = pipeline::load_candidate(Path::new(path))?;
                eprintln!("評価中: {} ...", candidate.id);
                evals.push(pipeline::evaluate_candidate(&candidate, timeout)?);
            }

            let ranked = scoring::rank(evals);
            let markdown = report::render(&ranked);

            match out {
                Some(path) => {
                    std::fs::write(&path, markdown)?;
                    eprintln!("レポートを書き出しました: {path}");
                }
                None => print!("{markdown}"),
            }
            Ok(())
        }
    }
}
