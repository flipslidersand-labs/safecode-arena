use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "safecode-arena", about = "Automated evaluation runner for AI-generated code")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Evaluate a single code candidate
    Eval {
        #[arg(long)]
        file: String,
        #[arg(long, default_value = "spec.yaml")]
        spec: String,
    },
    /// Compare multiple candidates in a directory
    Compare {
        #[arg(long)]
        dir: String,
        #[arg(long, default_value = "spec.yaml")]
        spec: String,
        #[arg(long, default_value = "report.md")]
        out: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    println!("safecode-arena — not yet implemented");
    Ok(())
}
