//! `forma-bench`: the benchmark runner (development tool).
//!
//! * `forma-bench run --mode c --level s|m|m-hand|l|o --out <dir>` runs the
//!   pipeline without an agent over a corpus level and writes `results.json`
//!   per case and `summary.json`.
//! * `forma-bench list --level <level>` prints the case ids.
//! * `forma-bench export-case --case <id> --out <dir>` writes the image and
//!   its layout for the runs with agents (`tools/bench/run_agent.sh`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use forma_bench::{Level, build_case, case_ids, run_level};

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
    /// Lists the case ids of a level.
    List {
        /// The level: s, m, m-hand, l or o.
        #[arg(long)]
        level: String,
    },
    /// Writes one case (`<id>.<ext>` and `<id>.layout.txt`, one row per line).
    ExportCase {
        /// The case id (see `list`).
        #[arg(long)]
        case: String,
        /// The output folder.
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Run { mode, level, out } => run(&mode, &level, &out),
        Command::List { level } => list(&level),
        Command::ExportCase { case, out } => export_case(&case, &out),
    }
}

fn parse_level(level: &str) -> Result<Level, ExitCode> {
    Level::parse(level).ok_or_else(|| {
        eprintln!("unknown level {level:?}: use s, m, m-hand, l or o");
        ExitCode::from(2)
    })
}

fn run(mode: &str, level: &str, out: &Path) -> ExitCode {
    if !mode.eq_ignore_ascii_case("c") {
        eprintln!("only mode c is implemented by forma-bench; modes A and B run with agents");
        return ExitCode::from(2);
    }
    let level = match parse_level(level) {
        Ok(level) => level,
        Err(code) => return code,
    };
    match run_level(level, out) {
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

fn list(level: &str) -> ExitCode {
    let level = match parse_level(level) {
        Ok(level) => level,
        Err(code) => return code,
    };
    for id in case_ids(level) {
        println!("{id}");
    }
    ExitCode::SUCCESS
}

fn export_case(case: &str, out: &Path) -> ExitCode {
    let built = build_case(case).map_err(|e| e.to_string()).and_then(|c| {
        std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
        let image = out.join(format!("{}.{}", c.id, c.extension));
        std::fs::write(&image, &c.image).map_err(|e| e.to_string())?;
        let layout = out.join(format!("{}.layout.txt", c.id));
        std::fs::write(&layout, c.layout.join("\n") + "\n").map_err(|e| e.to_string())?;
        Ok(image)
    });
    match built {
        Ok(image) => {
            println!("{}", image.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("forma-bench: {error}");
            ExitCode::FAILURE
        }
    }
}
