//! The command line, as `clap` sees it. Nothing here runs anything.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use forma_engine::{Policy, Toolset};

/// The FORMA font editor on the command line, and its MCP server.
///
/// Every subcommand is a thin facade over the same command registry the MCP
/// server and the editor use: `forma call <tool>` runs any of the 35 tools.
#[derive(Debug, Parser)]
#[command(
    name = "forma",
    version,
    propagate_version = true,
    arg_required_else_help = true,
    after_help = "\
Output: JSON on stdout (pretty, 2-space indent); logs on stderr (set FORMA_LOG, e.g. \
FORMA_LOG=debug).
Exit codes: 0 success; 1 a tool error (its {code, message, details} JSON is on stdout) or a \
failed validation; 2 a usage error, or an operation that needs approval (re-run with --yes).

Register the MCP server with an agent client:
  claude mcp add forma -- forma mcp --allow-dir ~/fonts"
)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a new, empty font document.
    New(NewArgs),
    /// Print a document's family, metrics, masters and glyph count as JSON.
    Info(PathArg),
    /// Export a document to a binary font (ttf, woff2) or a UFO.
    Export(ExportArgs),
    /// Validate a document; exit 0 if it has no errors, 1 otherwise.
    Validate(PathArg),
    /// Convert a document to another storage format (by extension).
    ///
    /// Typically `.forma` to `.formapackage`, the diff-friendly folder format.
    Pack(ConvertArgs),
    /// Convert a document to another storage format (by extension).
    ///
    /// Typically `.formapackage` to `.forma`. The same conversion as `pack`.
    Unpack(ConvertArgs),
    /// Run any registry tool (see the MCP tool list) with JSON arguments.
    Call(CallArgs),
    /// Serve MCP over stdio, for agent clients (Claude Code, Codex, ...).
    ///
    /// Nothing but the protocol is written to stdout; logs go to stderr.
    Mcp(McpArgs),
    /// Write the reference of every tool (docs/api/tools.md) from the registry.
    ///
    /// For the repository's maintainers: `forma gen-docs docs/api`.
    #[command(hide = true)]
    GenDocs(GenDocsArgs),
}

/// A document path.
#[derive(Debug, Args)]
pub struct PathArg {
    /// The `.forma` file or `.formapackage` folder.
    pub path: PathBuf,
}

/// Arguments of `forma new`.
#[derive(Debug, Args)]
pub struct NewArgs {
    /// Where to write the document: a `.forma` file or `.formapackage` folder
    /// that does not exist yet.
    pub path: PathBuf,
    /// The family name of the font.
    #[arg(long)]
    pub family: String,
    /// Units per em (16 to 16384).
    #[arg(long, default_value_t = 1000)]
    pub upm: u16,
}

/// An export format.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Format {
    /// OpenType with TrueType outlines.
    Ttf,
    /// The same font compressed for the web.
    Woff2,
    /// A UFO 3 folder.
    Ufo,
}

impl Format {
    /// The name the `export_font` tool takes.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ttf => "ttf",
            Self::Woff2 => "woff2",
            Self::Ufo => "ufo",
        }
    }
}

/// Arguments of `forma export`.
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// The `.forma` file or `.formapackage` folder to export.
    pub path: PathBuf,
    /// The format to write.
    #[arg(long, value_enum)]
    pub format: Format,
    /// The file (`.ttf`, `.woff2`) or folder (`.ufo`) to write.
    #[arg(short = 'o', long = "out")]
    pub out: PathBuf,
    /// Approve overwriting an existing target without asking.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

/// Arguments of `forma pack` and `forma unpack`.
#[derive(Debug, Args)]
pub struct ConvertArgs {
    /// The document to read.
    pub src: PathBuf,
    /// The document to write; its extension picks the format.
    pub dst: PathBuf,
    /// Approve overwriting an existing destination without asking.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

/// Arguments of `forma call`.
#[derive(Debug, Args)]
pub struct CallArgs {
    /// The tool name, such as `glyph_create` (see `forma call forma_capabilities`).
    pub tool: String,
    /// The tool's arguments, as a JSON object.
    #[arg(long, default_value = "{}")]
    pub args: String,
    /// Open this document first and inject its `doc` handle and current
    /// `base_rev` into the arguments, for the tools that take them (and only
    /// if `--args` does not give them).
    #[arg(long)]
    pub doc_path: Option<PathBuf>,
    /// After a successful call that changes the document, save it to its file.
    #[arg(long, requires = "doc_path")]
    pub save: bool,
    /// Where to write the PNG of a tool that returns an image.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Approve a destructive or massive operation without asking.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

/// How destructive or massive operations are approved.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum PolicyArg {
    /// A plan confirmed by the client (`plan_commit`) is enough.
    Prompted,
    /// Massive non-destructive operations apply directly.
    Auto,
    /// Plans need approval in the FORMA app (not available yet).
    Strict,
}

impl From<PolicyArg> for Policy {
    fn from(policy: PolicyArg) -> Self {
        match policy {
            PolicyArg::Prompted => Policy::Prompted,
            PolicyArg::Auto => Policy::Auto,
            PolicyArg::Strict => Policy::Strict,
        }
    }
}

/// Arguments of `forma gen-docs`.
#[derive(Debug, Args)]
pub struct GenDocsArgs {
    /// The folder to write `tools.md` into (created if missing).
    pub out_dir: PathBuf,
}

/// Arguments of `forma mcp`.
#[derive(Debug, Args)]
pub struct McpArgs {
    /// A directory documents may be read from and written to (repeatable);
    /// it must exist. Default: the current directory only.
    #[arg(long = "allow-dir", value_name = "DIR")]
    pub allow_dir: Vec<PathBuf>,
    /// How destructive or massive operations are approved.
    #[arg(long, value_enum, default_value_t = PolicyArg::Prompted)]
    pub policy: PolicyArg,
    /// Only serve these toolsets (comma-separated); `core` is always included.
    /// One or more of: core, glyphs, geometry, spacing, quality, export,
    /// preview, history. Default: all.
    #[arg(long, value_delimiter = ',', value_parser = parse_toolset, value_name = "A,B")]
    pub toolsets: Option<Vec<Toolset>>,
    /// Refuse every tool that changes anything.
    #[arg(long)]
    pub read_only: bool,
    /// Append one JSON line per tool call to this file.
    #[arg(long, value_name = "FILE")]
    pub audit_log: Option<PathBuf>,
    /// Keep the crash-recovery journals in this folder (created if missing).
    /// Default: the `recovery` folder of the OS data directory for FORMA.
    #[arg(long, value_name = "DIR", conflicts_with = "no_journal")]
    pub journal_dir: Option<PathBuf>,
    /// Do not keep a crash-recovery journal.
    #[arg(long)]
    pub no_journal: bool,
    /// Keep the document locks in this folder (created if missing). A
    /// document file has a single writer among the FORMA processes that use
    /// the same folder; the others open it read-only. Default:
    /// $FORMA_LOCK_DIR, else the `locks` folder of the OS data directory for
    /// FORMA (the default of every `forma` command).
    #[arg(long, value_name = "DIR")]
    pub lock_dir: Option<PathBuf>,
}

/// Reads a toolset name for `--toolsets`.
fn parse_toolset(text: &str) -> Result<Toolset, String> {
    Toolset::ALL
        .into_iter()
        .find(|toolset| toolset.as_str() == text)
        .ok_or_else(|| {
            let names: Vec<&str> = Toolset::ALL
                .iter()
                .map(|toolset| toolset.as_str())
                .collect();
            format!(
                "unknown toolset '{text}'; expected one of: {}",
                names.join(", ")
            )
        })
}
