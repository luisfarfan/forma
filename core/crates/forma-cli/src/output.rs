//! What the binary prints, and how a failure becomes an exit code.
//!
//! The conventions: success is pretty JSON (2-space indent) on stdout; a tool
//! error is its `{code, message, details}` JSON on stdout with exit 1; usage
//! problems and approvals that need `--yes` exit 2; everything else (an
//! unwritable stdout, an unreadable working directory) is a message on stderr
//! with exit 1.

use std::io::{self, ErrorKind, Write};

use forma_engine::ToolError;
use serde_json::{Value, json};

/// A tool error, a failed validation or an internal failure.
pub const EXIT_FAILURE: u8 = 1;
/// A usage error, or an operation that needs `--yes`.
pub const EXIT_USAGE: u8 = 2;

/// Why a subcommand did not succeed.
#[derive(Debug)]
pub enum Failure {
    /// A tool answered with an error: its JSON on stdout, exit 1.
    Tool(ToolError),
    /// The operation needs approval and `--yes` was not given: the plan on
    /// stdout, a hint on stderr, exit 2.
    NeedsApproval(Value),
    /// A usage mistake that argument parsing cannot see: a message on stderr,
    /// exit 2.
    Usage(String),
    /// Anything else: a message on stderr, exit 1.
    Other(anyhow::Error),
}

impl From<ToolError> for Failure {
    fn from(error: ToolError) -> Self {
        Self::Tool(error)
    }
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

/// How a subcommand ended: `Ok(code)` if it printed its result (code 0, or 1
/// for a failed validation), `Err` if it failed.
pub type Exit = Result<u8, Failure>;

/// Prints `value` as pretty JSON with a final newline. A closed pipe (`forma
/// info | head`) is not an error.
pub fn print_json(value: &Value) -> Result<(), Failure> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| Failure::Other(anyhow::Error::new(error).context("cannot encode JSON")))?;
    text.push('\n');
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Err(error) if error.kind() != ErrorKind::BrokenPipe => Err(Failure::Other(
            anyhow::Error::new(error).context("cannot write to stdout"),
        )),
        _ => Ok(()),
    }
}

/// The `{code, message, details}` JSON of a tool error.
pub fn error_json(error: &ToolError) -> Value {
    json!({
        "code": error.code.as_str(),
        "message": error.message,
        "details": error.details,
    })
}

/// Reports `failure` and returns the exit code.
pub fn report(failure: Failure) -> u8 {
    match failure {
        Failure::Tool(error) => {
            // If stdout is unusable there is nowhere better to say so.
            let _ = print_json(&error_json(&error));
            EXIT_FAILURE
        }
        Failure::NeedsApproval(plan) => {
            let _ = print_json(&plan);
            eprintln!("approval required: re-run with --yes to approve");
            EXIT_USAGE
        }
        Failure::Usage(message) => {
            eprintln!("error: {message}");
            EXIT_USAGE
        }
        Failure::Other(error) => {
            eprintln!("error: {error:#}");
            EXIT_FAILURE
        }
    }
}
