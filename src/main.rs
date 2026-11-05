//! SafeCode Arena CLI — `safecode`
//!
//! コマンド:
//!   safecode evaluate <candidate.rs>... [--tests <dir>] [--prop-tests <dir>]
//!            [--timeout-secs N] [--out <file>] [--format markdown|json]
//!            [--config <safecode.toml>] [--db <path>] [--regression-threshold F]
//!   safecode generate <spec.md> [--candidates N] [--out <file>]
//!   safecode history --db <path>

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use safecode_arena::config::Config;
use safecode_arena::generator;
use safecode_arena::model::{Candidate, Language};
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
        /// Wasm 実行のメモリ上限（MB、1〜4096）。
        #[arg(long, default_value_t = 64, value_parser = safecode_arena::wasm::parse_wasm_memory_mb)]
        wasm_max_memory_mb: usize,
        /// ミューテーションテスト（cargo mutants）を実行してテスト品質を採点する。
        /// 実行時間が長いためデフォルト off。cargo-mutants が PATH にない場合は Skipped 扱い。
        #[arg(long)]
        mutation: bool,
        /// LLM セマンティックレビューを実行して reasoning 軸を採点する。
        /// SAFECODE_LLM_BACKEND=claude|ollama で切り替え（既定 claude）。
        #[arg(long)]
        llm_review: bool,
    },
    /// 自然言語 spec から N 候補を LLM で生成し、検証・採点する。
    Generate {
        /// spec ファイル（.md / .txt）。
        spec: String,
        /// 生成する候補数（1〜8）。
        #[arg(long, default_value_t = 3, value_parser = generator::parse_candidate_count)]
        candidates: usize,
        /// 評価ステージ（compile/test 等）のタイムアウト秒数。
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
        /// LLM 候補生成呼び出しのタイムアウト秒数（Claude 既定 120、Ollama 既定 180 の代わりに使用）。
        #[arg(long, default_value_t = 120)]
        llm_timeout_secs: u64,
        /// レポート出力先ファイル（省略時は stdout）。
        #[arg(long)]
        out: Option<String>,
        /// 出力形式。
        #[arg(long, value_enum, default_value_t = Format::Markdown)]
        format: Format,
        /// 採点ルーブリック設定ファイル。
        #[arg(long)]
        config: Option<String>,
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
            mutation,
            llm_review,
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
            mutation,
            llm_review,
        }),
        Command::Generate {
            spec,
            candidates,
            timeout_secs,
            llm_timeout_secs,
            out,
            format,
            config,
        } => run_generate(
            spec,
            candidates,
            timeout_secs,
            llm_timeout_secs,
            out,
            format,
            config,
        ),
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
    mutation: bool,
    llm_review: bool,
}

fn run_evaluate(args: EvaluateArgs) -> anyhow::Result<()> {
    if args.candidates.is_empty() {
        anyhow::bail!("候補ファイルを 1 つ以上指定してください");
    }
    let timeout = Duration::from_secs(args.timeout_secs);
    let config = Config::load(args.config.as_deref())?;
    let eval_opts = pipeline::EvalOptions {
        tests_dir: args.tests.as_deref().map(Path::new),
        prop_tests_dir: args.prop_tests.as_deref().map(Path::new),
        wasm_opts: pipeline::WasmOptions {
            entry: args.wasm_entry.as_deref(),
            fuel: args.wasm_fuel,
            max_memory_bytes: args.wasm_max_memory_mb * 1024 * 1024,
        },
        run_mutation: args.mutation,
        run_llm_review: args.llm_review,
    };

    // 評価前に全候補を読み込み、未対応拡張子・ID 重複を先に検出する。
    let candidates = pipeline::load_candidates(&args.candidates)?;
    let mut evals = Vec::with_capacity(candidates.len());
    let mut rubrics = Vec::with_capacity(candidates.len());
    let mut failed_ids: Vec<String> = Vec::new();
    for candidate in &candidates {
        eprintln!("評価中: {} ...", candidate.id);
        let rubric = config.rubric_for(candidate.language.name());
        if let Some(eval) = keep_evaluated(
            &candidate.id,
            pipeline::evaluate_candidate(candidate, timeout, &rubric, &eval_opts),
        ) {
            evals.push(eval);
            rubrics.push(rubric);
        } else {
            failed_ids.push(candidate.id.clone());
        }
    }
    if evals.is_empty() {
        anyhow::bail!("全候補の評価に失敗しました");
    }

    scoring::assign_performance_each(&mut evals, &rubrics);
    scoring::assign_reasoning_each(&mut evals, &rubrics);
    let ranked = scoring::rank(evals);

    render_and_write(&ranked, args.format, args.out.as_deref())?;

    // 永続化 + リグレッション検出（出力後に実行して PR コメント用 report が確実に書かれるようにする）
    if let Some(db_path) = args.db.as_deref() {
        let options = run_options_key(args.llm_review, args.mutation, args.wasm_entry.as_deref());
        let has_regression = persist_and_report_regressions(
            db_path,
            &ranked,
            &failed_ids,
            args.regression_threshold,
            &options,
        )?;
        if has_regression {
            std::process::exit(1);
        }
    }
    Ok(())
}

/// スコアに影響する実行オプションを正規化した比較キー（#191）。
/// reasoning（--llm-review）・correctness の再配分（--mutation）・resource_usage（--wasm-entry）が変わる。
fn run_options_key(llm_review: bool, mutation: bool, wasm_entry: Option<&str>) -> String {
    format!(
        "llm_review={llm_review};mutation={mutation};wasm_entry={}",
        wasm_entry.unwrap_or("")
    )
}

/// 現在の評価を保存する前に、実行条件（`options`）が同じ直近 run と比較し、後退した候補を stderr に警告する。
/// 条件が同じ run が無ければ比較をスキップする（条件違いのスコア差を回帰と誤検出しないため）。
/// リグレッションがあれば run は保存しない（基準を劣化値に更新しないため）。
/// リグレッションが 1 件以上あれば `true` を返す。
fn persist_and_report_regressions(
    db_path: &str,
    ranked: &[Evaluation],
    failed_ids: &[String],
    threshold: f64,
    options: &str,
) -> anyhow::Result<bool> {
    let mut s = Store::open(Path::new(db_path))?;
    let mut has_regression = false;

    let baseline = s.latest_run_id_matching(options)?;
    if baseline.is_none() && s.latest_run_id()?.is_some() {
        eprintln!("⚠️  実行条件が同じ過去 run がないため比較をスキップ（今回: {options}）");
    }
    if let Some(prev_id) = baseline {
        let prev = s.run_comparable_scores(prev_id)?;
        let regs = store::find_regressions(&prev, ranked, threshold);
        let dropped = store::find_dropped(&prev, failed_ids);
        if !dropped.is_empty() {
            has_regression = true;
            eprintln!("⚠️  前回 run #{prev_id} にあった候補が評価失敗で脱落:");
            for id in &dropped {
                eprintln!("  - {id}");
            }
        }
        if regs.is_empty() {
            if dropped.is_empty() {
                eprintln!("リグレッションなし（直近 run #{prev_id} と比較、performance 軸を除く）");
            }
        } else {
            has_regression = true;
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

    if has_regression {
        // 劣化 run を保存すると次回の比較基準が下がり、再実行で回帰が見逃される。
        eprintln!("リグレッション検出のため run を保存しません（基準を維持）");
        return Ok(true);
    }

    let created_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let run_id = s.save_run(&created_at, options, ranked)?;
    eprintln!("run #{run_id} を保存しました: {db_path}");
    Ok(has_regression)
}

/// 1 候補の評価エラーで全体を止めない。Err は候補 ID 付きで警告して `None` を返す。
fn keep_evaluated<T>(id: &str, result: anyhow::Result<T>) -> Option<T> {
    match result {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("[warn] {id} の評価失敗: {e:#}");
            None
        }
    }
}

fn render_and_write(
    ranked: &[Evaluation],
    format: Format,
    out: Option<&str>,
) -> anyhow::Result<()> {
    let rendered = match format {
        Format::Markdown => report::render(ranked),
        Format::Json => serde_json::to_string_pretty(ranked)?,
        Format::Html => report::render_html(ranked),
    };
    match out {
        Some(path) => {
            std::fs::write(path, &rendered)?;
            eprintln!("レポートを書き出しました: {path}");
        }
        None => print!("{rendered}"),
    }
    Ok(())
}

fn run_generate(
    spec_path: String,
    n: usize,
    timeout_secs: u64,
    llm_timeout_secs: u64,
    out: Option<String>,
    format: Format,
    config: Option<String>,
) -> anyhow::Result<()> {
    let spec = std::fs::read_to_string(&spec_path)
        .with_context(|| format!("spec ファイルの読込に失敗: {spec_path}"))?;

    eprintln!("[generate] spec: {spec_path} ({} chars)", spec.len());
    eprintln!("[generate] {n} 候補を生成中 ...");

    let llm_timeout = Duration::from_secs(llm_timeout_secs);
    let sources = generator::generate(&spec, n, llm_timeout);
    if sources.is_empty() {
        anyhow::bail!("LLM からコード候補を取得できませんでした。SAFECODE_LLM_BACKEND / API キーを確認してください。");
    }
    eprintln!("[generate] {} 候補を取得", sources.len());
    if let Some(w) = generator::shortfall_warning(n, sources.len()) {
        eprintln!("[generate] 警告: {w}");
    }

    let timeout = Duration::from_secs(timeout_secs);
    // generate の候補は常に Rust
    let rubric = Config::load(config.as_deref())?.rubric_for(Language::Rust.name());

    let mut evals = Vec::with_capacity(sources.len());
    for (i, source) in sources.iter().enumerate() {
        let id = format!("candidate_{}", i + 1);
        eprintln!("[evaluate] {} ...", id);
        let candidate = Candidate {
            id,
            source: source.clone(),
            language: Language::Rust,
        };
        evals.extend(keep_evaluated(
            &candidate.id,
            pipeline::evaluate_candidate(
                &candidate,
                timeout,
                &rubric,
                &pipeline::EvalOptions::default(),
            ),
        ));
    }

    if evals.is_empty() {
        anyhow::bail!("全候補の評価に失敗しました");
    }

    scoring::assign_performance(&mut evals, &rubric);
    let rubrics = vec![rubric; evals.len()];
    scoring::assign_reasoning_each(&mut evals, &rubrics);
    let ranked = scoring::rank(evals);

    render_and_write(&ranked, format, out.as_deref())
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

#[cfg(test)]
mod tests {
    use super::keep_evaluated;

    #[test]
    fn keep_evaluated_passes_ok_and_drops_err() {
        assert_eq!(keep_evaluated("a", Ok(1)), Some(1));
        assert_eq!(keep_evaluated::<i32>("b", Err(anyhow::anyhow!("io"))), None);
    }
}
