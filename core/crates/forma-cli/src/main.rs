//! `forma`: the FORMA command-line tool and MCP server.
//!
//! A thin facade over the `forma-engine` command registry: every subcommand
//! that touches a document is a sequence of [`forma_engine::Engine::call`]s
//! made as `cli`, and `forma mcp` serves the same registry over MCP. The only
//! logic here is the command line, the sandbox of the paths the human types,
//! and `--yes` for approvals.
//!
//! On `forma mcp`, stdout carries only the protocol; logs (`tracing`, filtered
//! by `FORMA_LOG`, `warn` by default) always go to stderr.

#![forbid(unsafe_code)]

mod cli;
mod commands;
mod docs;
mod output;
mod session;

use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    // Usage errors exit 2 and `--help`/`--version` exit 0, as clap does.
    let cli = cli::Cli::parse();
    init_logging();
    commands::run(cli.command)
}

/// Sends `tracing` events to stderr, filtered by `FORMA_LOG` (default `warn`).
fn init_logging() {
    let filter = EnvFilter::try_from_env("FORMA_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    // A second subscriber cannot be installed; the first one stays.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}
