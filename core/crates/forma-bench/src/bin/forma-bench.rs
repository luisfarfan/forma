//! `forma-bench`: the benchmark runner (development tool).
//!
//! `forma-bench run --mode c --level s|m|m-hand|l|o --out <dir>` runs the
//! pipeline without an agent over a corpus level and writes `results.json`
//! per case and `summary.json`.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use forma_bench::{Level, run_level};

#[derive(Parser)]
#[command(name = "forma-bench", about = "FORMA benchmark runner")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Runs a mode over a corpus level.
    Run {
        /// The mode: only `c` (no agent) is implemented here.
        #[arg(long, default_value = "c")]
        mode: String,
        /// The level: s, m, m-hand, l or o.
        #[arg(long)]
        level: String,
        /// The output folder.
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Command::Run { mode, level, out } = cli.command;
    if !mode.eq_ignore_ascii_case("c") {
        eprintln!("only mode c is implemented by forma-bench; modes A and B run with agents");
        return ExitCode::from(2);
    }
    let Some(level) = Level::parse(&level) else {
        eprintln!("unknown level {level:?}: use s, m, m-hand, l or o");
        return ExitCode::from(2);
    };
    match run_level(level, &out) {
        Ok(summary) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&summary).unwrap_or_default()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("forma-bench: {error}");
            ExitCode::FAILURE
        }
    }
}
