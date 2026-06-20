//! SafeCode Arena CLI — `safecode`
//!
//! コマンド:
//!   safecode evaluate <candidate.rs>... [--tests <dir>] [--prop-tests <dir>]
//!            [--timeout-secs N] [--out <file>] [--format markdown|json]
//!            [--config <safecode.toml>]

use clap::{Parser, Subcommand, ValueEnum};
use safecode_arena::config::Rubric;
use safecode_arena::{pipeline, report, scoring};
use std::path::Path;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "safecode", version, about = "AI生成コード検証ランナー")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// レポート出力形式。
#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Markdown,
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// 1 つ以上のコード候補を検証・採点する。
    Evaluate {
        /// 検証する Rust ソースファイル（複数指定可）。
        candidates: Vec<String>,
        /// 統合テストとして含める `.rs` を置いたディレクトリ（任意）。
        #[arg(long)]
        tests: Option<String>,
        /// proptest を使った property test ファイルを置いたディレクトリ（任意）。
        /// 指定すると proptest が dev-dependency に追加されて実行される。
        #[arg(long)]
        prop_tests: Option<String>,
        /// 各ステージのタイムアウト秒数。
        #[arg(long, default_value_t = 60)]
        timeout_secs: u64,
        /// レポート出力先（省略時は標準出力）。
        #[arg(long)]
        out: Option<String>,
        /// 出力形式。
        #[arg(long, value_enum, default_value_t = Format::Markdown)]
        format: Format,
        /// 採点ルーブリック設定ファイル（既定: ./safecode.toml があれば使用）。
        #[arg(long)]
        config: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Evaluate {
            candidates,
            tests,
            prop_tests,
            timeout_secs,
            out,
            format,
            config,
        } => {
            if candidates.is_empty() {
                anyhow::bail!("候補ファイルを 1 つ以上指定してください");
            }
            let timeout = Duration::from_secs(timeout_secs);
            let rubric = Rubric::load(config.as_deref())?;
            let tests_dir = tests.as_deref().map(Path::new);
            let prop_tests_dir = prop_tests.as_deref().map(Path::new);

            let mut evals = Vec::with_capacity(candidates.len());
            for path in &candidates {
                let candidate = pipeline::load_candidate(Path::new(path))?;
                eprintln!("評価中: {} ...", candidate.id);
                evals.push(pipeline::evaluate_candidate(
                    &candidate,
                    timeout,
                    &rubric,
                    tests_dir,
                    prop_tests_dir,
                )?);
            }

            scoring::assign_performance(&mut evals, &rubric);
            let ranked = scoring::rank(evals);
            let rendered = match format {
                Format::Markdown => report::render(&ranked),
                Format::Json => serde_json::to_string_pretty(&ranked)?,
            };

            match out {
                Some(path) => {
                    std::fs::write(&path, rendered)?;
                    eprintln!("レポートを書き出しました: {path}");
                }
                None => print!("{rendered}"),
            }
            Ok(())
        }
    }
}
