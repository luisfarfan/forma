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
/// provenance; 0 until the thresholds are versioned.
pub const THRESHOLDS_VERSION: u32 = 0;

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

/// The folder that holds the reports of `doc`: next to its file, or in the
/// first allowed root for a document never saved.
fn reports_folder(engine: &Engine, doc: Option<&DocHandle>) -> Option<(PathBuf, String, u64)> {
    let (base, id, rev) = match doc.and_then(|handle| engine.docs.get(handle)) {
        Some(open) => {
            let base = open
                .source_path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .or_else(|| engine.roots.first().cloned())?;
            (base, open.font().id.to_string(), open.rev().0)
        }
        None => (engine.roots.first().cloned()?, "no-document".to_owned(), 0),
    };
    Some((base.join(REPORTS_DIR).join(&id), id, rev))
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
    if let Err(error) = fs::create_dir_all(&folder) {
        return ReportRef::note(format!("cannot create the reports folder: {error}"));
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

/// The sort key of a report file name `r<rev>-<tool>-<stamp>-<n>.json`: its
/// time stamp and sequence number, then the name.
fn age_key(name: &str) -> (String, u32, String) {
    let stem = name.trim_end_matches(".json");
    let mut parts = stem.rsplitn(3, '-');
    let n = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let stamp = parts.next().unwrap_or_default().to_owned();
    (stamp, n, name.to_owned())
}

/// Keeps the newest [`MAX_REPORTS_PER_TOOL`] reports of `tool` in `folder`
/// and at most [`MAX_REPORT_DIR_BYTES`] of reports in all, deleting the
/// oldest first.
fn prune(folder: &Path, tool: &str) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    let mut files: Vec<(String, u64)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let len = entry.metadata().ok()?.len();
            (name.starts_with('r') && name.ends_with(".json")).then_some((name, len))
        })
        .collect();
    files.sort_by_key(|(name, _)| age_key(name));
    let marker = format!("-{tool}-");
    let of_tool = files
        .iter()
        .filter(|(name, _)| name.contains(&marker))
        .count();
    let mut excess = of_tool.saturating_sub(MAX_REPORTS_PER_TOOL);
    let mut total: u64 = files.iter().map(|(_, len)| len).sum();
    for (name, len) in &files {
        let over_count = excess > 0 && name.contains(&marker);
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
    fn age_key_orders_by_stamp_then_sequence() {
        let mut names = vec![
            "r9-validate-20261018T101500Z-1.json",
            "r2-validate-20261018T101500Z-0.json",
            "r5-validate-20261018T091500Z-3.json",
        ];
        names.sort_by_key(|name| age_key(name));
        assert_eq!(
            names,
            [
                "r5-validate-20261018T091500Z-3.json",
                "r2-validate-20261018T101500Z-0.json",
                "r9-validate-20261018T101500Z-1.json",
            ]
        );
    }

    #[test]
    fn budgets_differ_by_detail() {
        assert_eq!(Detail::Summary.list_budget(), (20, 4096));
        assert_eq!(Detail::Full.list_budget(), (200, 32 * 1024));
        assert_eq!(Detail::default(), Detail::Summary);
    }
}
