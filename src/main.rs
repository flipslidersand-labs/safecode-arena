//! SafeCode Arena CLI — `safecode`
//!
//! コマンド:
//!   safecode evaluate <candidate.rs>... [--tests <dir>] [--prop-tests <dir>]
//!            [--timeout-secs N] [--out <file>] [--format markdown|json]
//!            [--config <safecode.toml>] [--db <path>] [--regression-threshold F]
//!   safecode history --db <path>

use clap::{Parser, Subcommand, ValueEnum};
use safecode_arena::config::Rubric;
use safecode_arena::store::{self, Store};
use safecode_arena::{pipeline, report, scoring, Evaluation};
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
    Html,
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
        /// 評価結果を保存する SQLite DB（指定時、過去 run との後退を検出）。
        #[arg(long)]
        db: Option<String>,
        /// リグレッションと判定するスコア下落の下限。
        #[arg(long, default_value_t = 1.0)]
        regression_threshold: f64,
        /// Wasm サンドボックスで実行する候補のエントリ関数名（no-arg pub fn）。
        /// 指定すると wasm32-wasip1 にビルドして隔離実行する。
        #[arg(long)]
        wasm_entry: Option<String>,
        /// Wasm 実行の fuel 上限（命令数）。
        #[arg(long, default_value_t = 100_000_000)]
        wasm_fuel: u64,
        /// Wasm 実行のメモリ上限（MB）。
        #[arg(long, default_value_t = 64)]
        wasm_max_memory_mb: usize,
    },
    /// 保存済みの run 履歴を一覧表示する。
    History {
        /// 履歴 DB のパス。
        #[arg(long)]
        db: String,
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
            db,
            regression_threshold,
            wasm_entry,
            wasm_fuel,
            wasm_max_memory_mb,
        } => run_evaluate(EvaluateArgs {
            candidates,
            tests,
            prop_tests,
            timeout_secs,
            out,
            format,
            config,
            db,
            regression_threshold,
            wasm_entry,
            wasm_fuel,
            wasm_max_memory_mb,
        }),
        Command::History { db } => run_history(&db),
    }
}

/// `evaluate` サブコマンドの引数一式。
struct EvaluateArgs {
    candidates: Vec<String>,
    tests: Option<String>,
    prop_tests: Option<String>,
    timeout_secs: u64,
    out: Option<String>,
    format: Format,
    config: Option<String>,
    db: Option<String>,
    regression_threshold: f64,
    wasm_entry: Option<String>,
    wasm_fuel: u64,
    wasm_max_memory_mb: usize,
}

fn run_evaluate(args: EvaluateArgs) -> anyhow::Result<()> {
    if args.candidates.is_empty() {
        anyhow::bail!("候補ファイルを 1 つ以上指定してください");
    }
    let timeout = Duration::from_secs(args.timeout_secs);
    let rubric = Rubric::load(args.config.as_deref())?;
    let tests_dir = args.tests.as_deref().map(Path::new);
    let prop_tests_dir = args.prop_tests.as_deref().map(Path::new);
    let wasm_opts = pipeline::WasmOptions {
        entry: args.wasm_entry.as_deref(),
        fuel: args.wasm_fuel,
        max_memory_bytes: args.wasm_max_memory_mb * 1024 * 1024,
    };

    let mut evals = Vec::with_capacity(args.candidates.len());
    for path in &args.candidates {
        let candidate = pipeline::load_candidate(Path::new(path))?;
        eprintln!("評価中: {} ...", candidate.id);
        evals.push(pipeline::evaluate_candidate(
            &candidate,
            timeout,
            &rubric,
            tests_dir,
            prop_tests_dir,
            &wasm_opts,
        )?);
    }

    scoring::assign_performance(&mut evals, &rubric);
    let ranked = scoring::rank(evals);

    // 永続化 + リグレッション検出
    if let Some(db_path) = args.db.as_deref() {
        persist_and_report_regressions(db_path, &ranked, args.regression_threshold)?;
    }

    let rendered = match args.format {
        Format::Markdown => report::render(&ranked),
        Format::Json => serde_json::to_string_pretty(&ranked)?,
        Format::Html => report::render_html(&ranked),
    };
    match args.out {
        Some(path) => {
            std::fs::write(&path, rendered)?;
            eprintln!("レポートを書き出しました: {path}");
        }
        None => print!("{rendered}"),
    }
    Ok(())
}

/// 現在の評価を保存する前に直近 run と比較し、後退した候補を stderr に警告する。
fn persist_and_report_regressions(
    db_path: &str,
    ranked: &[Evaluation],
    threshold: f64,
) -> anyhow::Result<()> {
    let mut s = Store::open(Path::new(db_path))?;

    if let Some(prev_id) = s.latest_run_id()? {
        let prev = s.run_scores(prev_id)?;
        let regs = store::find_regressions(&prev, ranked, threshold);
        if regs.is_empty() {
            eprintln!("リグレッションなし（直近 run #{prev_id} と比較）");
        } else {
            eprintln!("⚠️  リグレッション検出（直近 run #{prev_id} 比）:");
            for r in &regs {
                eprintln!(
                    "  - {}: {:.1} → {:.1}（{:+.1}）",
                    r.candidate_id,
                    r.previous,
                    r.current,
                    r.delta()
                );
            }
        }
    }

    let created_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let run_id = s.save_run(&created_at, ranked)?;
    eprintln!("run #{run_id} を保存しました: {db_path}");
    Ok(())
}

fn run_history(db_path: &str) -> anyhow::Result<()> {
    let s = Store::open(Path::new(db_path))?;
    let runs = s.list_runs()?;
    if runs.is_empty() {
        println!("run 履歴はありません: {db_path}");
        return Ok(());
    }
    println!("# run 履歴（{db_path}）\n");
    println!("| run | 日時 | 候補数 | 最良候補 | 最良スコア |");
    println!("| --- | ---- | ------ | -------- | ---------- |");
    for r in runs {
        println!(
            "| #{} | {} | {} | {} | {} |",
            r.id,
            r.created_at,
            r.candidate_count,
            r.best_candidate.as_deref().unwrap_or("—"),
            r.best_score
                .map(|s| format!("{s:.1}"))
                .unwrap_or_else(|| "—".into()),
        );
    }
    Ok(())
}
