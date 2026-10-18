//! The `detail` of tools with long lists, and the report files that hold
//! what a summary leaves out.
//!
//! A tool with `detail: "summary"` (the default) answers with totals and the
//! first few exceptions; `"full"` keeps the longer lists of earlier versions.
//! Either way the complete detail is written to a report file:
//!
//! ```text
//! <document folder>/.forma-reports/<font id>/r<rev>-<tool>-<UTC>-<n>.json
//! ```
//!
//! Reports are diagnostics: reproducible (the header names the document,
//! revision, tool, argument hash, engine version and quality thresholds
//! version) and disposable. Deleting them loses nothing, because everything
//! essential (provenance, review state and per-glyph measurements) lives in
//! the document. A report is created exclusively, so it never overwrites an
//! earlier one, and old ones are pruned: at most [`MAX_REPORTS_PER_TOOL`]
//! per document and tool, and [`MAX_REPORT_DIR_BYTES`] per document. A read-only server writes
//! none. Writing a report never fails the call: a problem is told in
//! `report_note`.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::engine::{DocHandle, Engine};
use crate::limits::{MAX_RESULT_ENTRIES, MAX_RESULT_LIST_BYTES};
use crate::plans::{canonical_json, rfc3339, sha256};

/// How much a tool lists in its answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    /// Totals and the first few exceptions; the rest is in the report file.
    #[default]
    Summary,
    /// Longer lists (up to 200 entries or 32 KB each).
    Full,
}

/// The folder of reports, next to the document.
pub const REPORTS_DIR: &str = ".forma-reports";

/// The most reports kept per document and tool.
pub const MAX_REPORTS_PER_TOOL: usize = 20;

/// The most bytes of reports kept per document.
pub const MAX_REPORT_DIR_BYTES: u64 = 20 * 1024 * 1024;

/// The largest report; a larger body is replaced by a note.
pub const MAX_REPORT_BYTES: usize = 2 * 1024 * 1024;

/// The most entries of a list in a summary answer.
pub const SUMMARY_ENTRIES: usize = 20;

/// The most bytes of JSON of a list in a summary answer.
pub const SUMMARY_LIST_BYTES: usize = 4 * 1024;

/// The version of the quality thresholds recorded in reports and
/// provenance.
pub const THRESHOLDS_VERSION: u32 = forma_trace::THRESHOLDS.version;

impl Detail {
    /// The most entries and bytes of JSON of a list at this detail.
    #[must_use]
    pub fn list_budget(self) -> (usize, usize) {
        match self {
            Detail::Summary => (SUMMARY_ENTRIES, SUMMARY_LIST_BYTES),
            Detail::Full => (MAX_RESULT_ENTRIES, MAX_RESULT_LIST_BYTES),
        }
    }
}

/// Where the complete detail of a call went, flattened into results.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReportRef {
    /// The report file with the complete detail of the call (a diagnostic:
    /// it can be deleted without losing anything of the document); absent
    /// when none was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_path: Option<String>,
    /// Why no report was written, when that is worth saying.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_note: Option<String>,
}

impl ReportRef {
    fn note(text: impl Into<String>) -> ReportRef {
        ReportRef {
            report_path: None,
            report_note: Some(text.into()),
        }
    }
}

/// Whether `dir`, canonicalized, lies inside one of the engine's (canonical)
/// allowed roots.
fn inside_roots(engine: &Engine, dir: &Path) -> Option<PathBuf> {
    let canonical = dunce::canonicalize(dir).ok()?;
    engine
        .roots
        .iter()
        .any(|root| canonical.starts_with(root))
        .then_some(canonical)
}

/// The folder that holds the reports of `doc`: next to its file when that
/// folder is inside an allowed root, else in the first allowed root.
fn reports_folder(engine: &Engine, doc: Option<&DocHandle>) -> Option<(PathBuf, String, u64)> {
    let (next_to_file, id, rev) = match doc.and_then(|handle| engine.docs.get(handle)) {
        Some(open) => (
            open.source_path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf),
            open.font().id.to_string(),
            open.rev().0,
        ),
        None => (None, "no-document".to_owned(), 0),
    };
    let base = next_to_file
        .and_then(|dir| inside_roots(engine, &dir))
        .or_else(|| engine.roots.first().cloned())?;
    Some((base.join(REPORTS_DIR).join(&id), id, rev))
}

/// Creates `folder` (and the reports folder above it, with a `.gitignore`
/// that keeps reports out of version control), refusing symbolic links: a
/// link could send reports, and their pruning, outside the allowed roots.
fn prepare_folder(folder: &Path) -> Result<(), String> {
    let reports = folder.parent().ok_or("no reports folder")?;
    for dir in [reports, folder] {
        match fs::symlink_metadata(dir) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(format!(
                    "'{}' is a link or not a folder; no report was written",
                    crate::engine::path_text(dir)
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(dir)
                    .map_err(|e| format!("cannot create the reports folder: {e}"))?;
                if dir == reports {
                    let _ = fs::write(dir.join(".gitignore"), "*\n");
                }
            }
            Err(error) => return Err(format!("cannot use the reports folder: {error}")),
        }
    }
    Ok(())
}

/// Writes the report of a call of `tool` with `args` about `doc`, holding
/// `body`, and prunes old reports. Never fails: problems become a note.
pub(crate) fn write_report(
    engine: &Engine,
    doc: Option<&DocHandle>,
    tool: &str,
    args: &Value,
    body: &impl Serialize,
) -> ReportRef {
    if engine.cfg.read_only {
        return ReportRef::note("the server is read-only: no report file was written");
    }
    let Some((folder, doc_id, rev)) = reports_folder(engine, doc) else {
        // Nowhere to write (no allowed root): nothing worth saying.
        return ReportRef::default();
    };
    let body = match serde_json::to_value(body) {
        Ok(value) => value,
        Err(error) => return ReportRef::note(format!("cannot encode the report: {error}")),
    };
    let created = rfc3339(engine.plans.now_unix());
    let header = json!({
        "doc_id": doc_id,
        "rev": rev,
        "tool": tool,
        "args_sha256": hex(&sha256(canonical_json(args).as_bytes())),
        "engine_version": env!("CARGO_PKG_VERSION"),
        "thresholds_version": THRESHOLDS_VERSION,
        "created_utc": created,
    });
    let mut text = canonical_json(&json!({ "header": header, "body": body }));
    if text.len() > MAX_REPORT_BYTES {
        text = canonical_json(&json!({
            "header": header,
            "truncated": true,
            "note": format!(
                "the detail took {} bytes, more than the {MAX_REPORT_BYTES} a report keeps; \
                 call the tool on fewer glyphs",
                text.len()
            ),
        }));
    }
    if let Err(note) = prepare_folder(&folder) {
        return ReportRef::note(note);
    }
    let stamp: String = created
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    for n in 0..1000 {
        let path = folder.join(format!("r{rev}-{tool}-{stamp}-{n}.json"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(text.as_bytes()) {
                    let _ = fs::remove_file(&path);
                    return ReportRef::note(format!("cannot write the report: {error}"));
                }
                prune(&folder, tool);
                return ReportRef {
                    report_path: Some(crate::engine::path_text(&path)),
                    report_note: None,
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return ReportRef::note(format!("cannot write the report: {error}")),
        }
    }
    ReportRef::note("too many reports in the same second")
}

/// The parts of a report file name `r<rev>-<tool>-<stamp>-<n>.json`
/// (`<stamp>` like `20261018T074217Z`, `<tool>` lowercase letters, digits
/// and underscores): its tool and its age (time stamp, sequence number);
/// `None` for any other name, which pruning never touches.
fn parse_name(name: &str) -> Option<(String, (String, u32))> {
    let stem = name.strip_suffix(".json")?.strip_prefix('r')?;
    let mut parts = stem.rsplitn(3, '-');
    let n: u32 = parts.next()?.parse().ok()?;
    let stamp = parts.next()?;
    let rest = parts.next()?;
    let (rev, tool) = rest.split_once('-')?;
    let stamp_ok = stamp.len() == 16
        && stamp.ends_with('Z')
        && stamp.as_bytes()[8] == b'T'
        && stamp
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 8 || i == 15 || b.is_ascii_digit());
    let tool_ok = !tool.is_empty()
        && tool
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    (stamp_ok && tool_ok && !rev.is_empty() && rev.bytes().all(|b| b.is_ascii_digit()))
        .then(|| (tool.to_owned(), (stamp.to_owned(), n)))
}

/// Keeps the newest [`MAX_REPORTS_PER_TOOL`] reports of `tool` in `folder`
/// and at most [`MAX_REPORT_DIR_BYTES`] of reports in all, deleting the
/// oldest first.
fn prune(folder: &Path, tool: &str) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    // Only regular files named like reports are ever counted or deleted.
    let mut files: Vec<(String, String, (String, u32), u64)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let (owner, age) = parse_name(&name)?;
            let len = entry.metadata().ok()?.len();
            Some((name, owner, age, len))
        })
        .collect();
    files.sort_by(|a, b| (&a.2, &a.0).cmp(&(&b.2, &b.0)));
    let of_tool = files.iter().filter(|f| f.1 == tool).count();
    let mut excess = of_tool.saturating_sub(MAX_REPORTS_PER_TOOL);
    let mut total: u64 = files.iter().map(|f| f.3).sum();
    for (name, owner, _, len) in &files {
        let over_count = excess > 0 && owner == tool;
        if !over_count && total <= MAX_REPORT_DIR_BYTES {
            continue;
        }
        if fs::remove_file(folder.join(name)).is_ok() {
            total = total.saturating_sub(*len);
            if over_count {
                excess -= 1;
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_parse_and_order_by_stamp_then_sequence() {
        let mut names = vec![
            "r9-validate-20261018T101500Z-1.json",
            "r2-validate-20261018T101500Z-0.json",
            "r5-image_trace-20261018T091500Z-3.json",
        ];
        names.sort_by_key(|name| parse_name(name).map(|(_, age)| age));
        assert_eq!(
            names,
            [
                "r5-image_trace-20261018T091500Z-3.json",
                "r2-validate-20261018T101500Z-0.json",
                "r9-validate-20261018T101500Z-1.json",
            ]
        );
        assert_eq!(
            parse_name("r5-image_trace-20261018T091500Z-3.json").map(|(t, _)| t),
            Some("image_trace".to_owned())
        );
        for other in [
            "results.json",
            "r1-validate-notastamp000000-0.json",
            "rx-validate-20261018T101500Z-0.json",
            "r1-Validate-20261018T101500Z-0.json",
            "r1-validate-20261018T101500Z-0.txt",
        ] {
            assert!(parse_name(other).is_none(), "{other}");
        }
    }

    #[test]
    fn budgets_differ_by_detail() {
        assert_eq!(Detail::Summary.list_budget(), (20, 4096));
        assert_eq!(Detail::Full.list_budget(), (200, 32 * 1024));
        assert_eq!(Detail::default(), Detail::Summary);
    }
}
