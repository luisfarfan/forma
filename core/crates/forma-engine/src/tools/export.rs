//! Export: `export_font`.
//!
//! TTF and WOFF2 are compiled with `forma_compile` and written atomically
//! (a temporary file next to the target, flushed, then renamed over it, with
//! `forma_store::write_file_atomic`); a UFO folder is written by
//! `forma_interop`, which never replaces anything but a UFO. Writing over an
//! existing file or UFO is destructive and passes the approval gate as a
//! snapshot of the document (the plan pins the revision). Exporting never
//! changes the document.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forma_compile::OutputFormat;
use forma_model::{Actor, Rev};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::{DocHandle, Engine, path_text};
use crate::errors::{ErrorCode, ToolError, from_compile_error, from_interop_error};
use crate::registry::{CommandSpec, Effects, OutputKind, ToolOutput, Toolset, schema_of};
use crate::tools::json_output;
use crate::tools::limits::parse_checked_ref;
use crate::tools::plans::{Change, Gate, OrPlan, Proposal, approval_gate};

/// The file that marks a folder as a UFO (the rule `forma_interop` applies).
const UFO_MARKER: &str = "metainfo.plist";

/// `export_font`.
pub(crate) const EXPORT_FONT: CommandSpec = CommandSpec {
    name: "export_font",
    title: "Export font",
    description: "Export `doc` to `path` inside the allowed roots (relative paths resolve against \
                  the first one) as a TTF or WOFF2 font, or as a UFO folder; the extension must \
                  match `format` (.ttf, .woff2, .ufo). The default master is exported, only \
                  glyphs marked for export, and a .notdef and a space are added when missing \
                  (`report.added_glyphs`). Pass the latest `rev` you received as `base_rev`: a \
                  newer one is rejected, an older one is accepted and the current state is \
                  exported. The document is not changed. Overwriting an existing file or UFO is \
                  destructive and may require approval depending on the server policy \
                  (APPROVAL_REQUIRED with a plan to confirm, which is refused if the document \
                  changes before it is committed); with `dry_run: true` the call only returns \
                  the plan. A font that cannot be compiled fails with VALIDATION_FAILED (see \
                  validate) or COMPILE_FAILED.",
    toolset: Toolset::Export,
    effects: Effects::DESTRUCTIVE,
    output: OutputKind::Json,
    input_schema: schema_of::<ExportFontArgs>,
    output_schema: Some(schema_of::<OrPlan<ExportFontResult>>),
    handler: export_font,
};

/// The format to export to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// OpenType with TrueType outlines (`.ttf`).
    Ttf,
    /// The same font compressed for the web (`.woff2`).
    Woff2,
    /// A UFO 3 folder (`.ufo`), for other font editors.
    Ufo,
}

impl ExportFormat {
    /// The name of the format, which is also the extension of its files.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ttf => "ttf",
            Self::Woff2 => "woff2",
            Self::Ufo => "ufo",
        }
    }
}

/// Arguments of `export_font`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportFontArgs {
    /// The document handle.
    pub doc: DocHandle,
    /// The latest revision you received.
    pub base_rev: u64,
    /// The format to export to.
    pub format: ExportFormat,
    /// The file (`.ttf`, `.woff2`) or folder (`.ufo`) to write, inside the
    /// allowed roots.
    pub path: PathBuf,
    /// Only plan the call: change nothing and return `{rev, plan}`, the plan
    /// of what the call would do, with a `plan_token` that plan_commit can
    /// run once the user confirms.
    #[serde(default)]
    pub dry_run: bool,
}

/// Result of `export_font`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportFontResult {
    /// The current revision, which was exported (exporting changes nothing).
    pub rev: u64,
    /// The canonical path written.
    pub path: String,
    /// The format written.
    pub format: ExportFormat,
    /// The size of the file written, in bytes; `null` for a UFO folder.
    pub bytes: Option<u64>,
    /// What the export added or noticed.
    pub report: ExportReport,
}

/// What an export added to the font or noticed about it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportReport {
    /// The glyphs the compiler added because the font lacked them
    /// (`.notdef`, `space`), in the order they were added; empty for a UFO.
    pub added_glyphs: Vec<String>,
    /// Warnings about the compiled font.
    pub warnings: Vec<String>,
}

/// Refuses a `path` whose extension is not the one of `format` (ignoring
/// case).
fn check_extension(path: &Path, format: ExportFormat) -> Result<(), ToolError> {
    let wanted = format.as_str();
    let matches = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted));
    if matches {
        Ok(())
    } else {
        Err(ToolError::invalid_argument(format!(
            "`path` must end in .{wanted} to export as {wanted}, got '{}'",
            path.display()
        )))
    }
}

/// Whether exporting to `target` replaces something: a file for TTF and
/// WOFF2, a UFO for UFO. Anything else in the way is refused, before any
/// plan is made.
fn overwrites(target: &Path, format: ExportFormat) -> Result<bool, ToolError> {
    let metadata = match fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(ToolError::io(format!(
                "cannot inspect '{}': {error}",
                path_text(target)
            )));
        }
    };
    let replaceable = match format {
        ExportFormat::Ufo => metadata.is_dir() && target.join(UFO_MARKER).is_file(),
        ExportFormat::Ttf | ExportFormat::Woff2 => metadata.is_file(),
    };
    if replaceable {
        return Ok(true);
    }
    let what = match format {
        ExportFormat::Ufo => "a UFO",
        ExportFormat::Ttf | ExportFormat::Woff2 => "a file",
    };
    Err(ToolError::invalid_argument(format!(
        "'{}': destination exists and is not {what}; it was left untouched",
        path_text(target)
    )))
}

fn export_font(engine: &mut Engine, actor: &Actor, args: Value) -> Result<ToolOutput, ToolError> {
    let call: ExportFontArgs = parse_checked_ref(&args)?;
    let rev = engine.doc(&call.doc)?.rev();
    if call.base_rev > rev.0 {
        return Err(ToolError::invalid_argument(format!(
            "base_rev {} is newer than the current revision {}",
            call.base_rev, rev.0
        )));
    }
    check_extension(&call.path, call.format)?;
    let target = engine.check_allowed(&call.path, false)?;
    let overwrites = overwrites(&target, call.format)?;
    if overwrites {
        // A read-only handle exports to new files only.
        engine.ensure_writable(call.doc.as_str())?;
    }

    let label = if overwrites {
        format!("overwrites {}", path_text(&target))
    } else {
        format!("writes {}", path_text(&target))
    };
    let proposal = Proposal {
        actor,
        tool: EXPORT_FONT.name,
        args: &args,
        doc: &call.doc,
        base_rev: Rev(call.base_rev),
        dry_run: call.dry_run,
        destructive: overwrites,
        label,
        change: Change::Snapshot,
    };
    if let Gate::Answer(answer) = approval_gate(engine, &proposal)? {
        return Ok(answer);
    }

    let font = Arc::clone(engine.doc(&call.doc)?.font());
    let (bytes, report) = match call.format {
        ExportFormat::Ttf | ExportFormat::Woff2 => {
            let format = if call.format == ExportFormat::Ttf {
                OutputFormat::Ttf
            } else {
                OutputFormat::Woff2
            };
            let (binary, compiled) =
                forma_compile::compile(&font, format).map_err(from_compile_error)?;
            forma_store::write_file_atomic(&target, &binary).map_err(|error| {
                ToolError::io(format!("cannot write '{}': {error}", path_text(&target)))
            })?;
            let size = u64::try_from(binary.len()).unwrap_or(u64::MAX);
            let report = ExportReport {
                added_glyphs: compiled.added_glyphs,
                warnings: compiled.warnings,
            };
            (Some(size), report)
        }
        ExportFormat::Ufo => {
            let master = font
                .masters
                .first()
                .ok_or_else(|| ToolError::new(ErrorCode::CompileFailed, "the font has no masters"))?
                .id;
            forma_interop::write_ufo(&font, master, &target).map_err(from_interop_error)?;
            (None, ExportReport::default())
        }
    };
    json_output(&ExportFontResult {
        rev: rev.0,
        path: path_text(&target),
        format: call.format,
        bytes,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_match_their_format_in_any_case() {
        for (format, path) in [
            (ExportFormat::Ttf, "a.ttf"),
            (ExportFormat::Ttf, "a.TtF"),
            (ExportFormat::Woff2, "dir/a.WOFF2"),
            (ExportFormat::Ufo, "a.ufo"),
        ] {
            assert!(check_extension(Path::new(path), format).is_ok(), "{path}");
        }
        for (format, path) in [
            (ExportFormat::Ttf, "a.woff2"),
            (ExportFormat::Woff2, "a.woff"),
            (ExportFormat::Ufo, "ufo"),
            (ExportFormat::Ttf, ".ttf"),
        ] {
            let error = check_extension(Path::new(path), format).expect_err(path);
            assert_eq!(error.code, ErrorCode::InvalidArgument);
            assert!(error.message.contains(format.as_str()), "{}", error.message);
        }
    }
}
