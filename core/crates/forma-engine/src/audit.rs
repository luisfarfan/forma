//! The audit log: one JSON line per [`Engine::call`], when
//! [`EngineConfig::audit_log`](crate::EngineConfig::audit_log) is set.
//!
//! Each line is a compact JSON object:
//!
//! ```json
//! {"ts":"2027-01-15T08:05:00Z","actor":"agent:claude","tool":"glyph_create",
//!  "doc":"doc_…","rev_before":3,"rev_after":4,"ok":true,"error_code":null,
//!  "args_sha256":"…"}
//! ```
//!
//! * `ts`: when the call arrived, RFC 3339 UTC, from the engine's clock;
//! * `doc`: the open document the call is about (its `doc` argument; for
//!   `document_create` and `document_open` the document they return; for
//!   `plan_commit` the document of the plan), or `null`. A handle that names
//!   no open document is not recorded;
//! * `rev_before`, `rev_after`: the revision of that document before and
//!   after the call, `null` when it was not open then;
//! * `error_code`: the code of a failed call, `null` on success;
//! * `args_sha256`: the SHA-256 (lowercase hex) of the arguments as canonical
//!   JSON (keys sorted, compact). The arguments themselves (outlines, names,
//!   text) are never written;
//! * `report_path`: present when the call wrote a report file with its
//!   complete detail (see `tools::report`).
//!
//! The file is opened in append mode for every line and created if missing;
//! its folder must exist. A line that cannot be written is reported with
//! `tracing::warn!` and never fails the call.

use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;

use forma_model::Actor;
use serde::Serialize;
use serde_json::Value;

use crate::engine::{DocHandle, Engine};
use crate::errors::{ErrorCode, ToolError};
use crate::plans::{canonical_json, rfc3339, sha256};
use crate::registry::ToolOutput;

/// The longest tool name written; an unknown tool's name is cut to this many
/// bytes (registered names are at most 40 characters).
const MAX_TOOL_NAME: usize = 64;

/// One line of the audit log.
#[derive(Serialize)]
struct AuditLine<'a> {
    ts: &'a str,
    actor: &'a str,
    tool: &'a str,
    doc: Option<&'a str>,
    rev_before: Option<u64>,
    rev_after: Option<u64>,
    ok: bool,
    error_code: Option<ErrorCode>,
    args_sha256: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    report_path: Option<&'a str>,
}

/// What is known of a call when it arrives, until it is done.
#[derive(Debug)]
pub(crate) struct AuditEntry {
    ts: String,
    actor: String,
    tool: String,
    doc: Option<DocHandle>,
    rev_before: Option<u64>,
    args_sha256: String,
}

impl AuditEntry {
    /// Records the call of `tool` with `args` by `actor` as it arrives.
    pub(crate) fn begin(engine: &Engine, actor: &Actor, tool: &str, args: &Value) -> AuditEntry {
        let doc = match args.get("doc").and_then(Value::as_str) {
            Some(handle) if engine.docs.contains_key(handle) => Some(DocHandle::from(handle)),
            Some(_) => None,
            None if tool == "plan_commit" => args
                .get("plan_token")
                .and_then(Value::as_str)
                .and_then(|token| engine.plans.doc_of(token, actor)),
            None => None,
        };
        let rev_before = doc.as_ref().and_then(|doc| rev_of(engine, doc.as_str()));
        AuditEntry {
            ts: rfc3339(engine.plans.now_unix()),
            actor: actor.to_string(),
            tool: cut(tool, MAX_TOOL_NAME).to_owned(),
            doc,
            rev_before,
            args_sha256: hex(&sha256(canonical_json(args).as_bytes())),
        }
    }

    /// Completes the record with the outcome of the call and appends it to
    /// the audit log.
    pub(crate) fn finish(self, engine: &Engine, result: &Result<ToolOutput, ToolError>) {
        let Some(path) = engine.cfg.audit_log.as_deref() else {
            return;
        };
        let opened = match result {
            Ok(ToolOutput::Json(value)) => value
                .get("doc")
                .and_then(Value::as_str)
                .filter(|handle| engine.docs.contains_key(*handle)),
            _ => None,
        };
        // The document the call was about when it arrived, or else the one
        // it created or opened.
        let doc = self.doc.as_ref().map(DocHandle::as_str).or(opened);
        let rev_after = doc.and_then(|handle| rev_of(engine, handle));
        let line = AuditLine {
            ts: &self.ts,
            actor: &self.actor,
            tool: &self.tool,
            doc,
            rev_before: self.rev_before,
            rev_after,
            ok: result.is_ok(),
            error_code: result.as_ref().err().map(|error| error.code),
            args_sha256: &self.args_sha256,
            report_path: match result {
                Ok(ToolOutput::Json(value)) => value.get("report_path").and_then(Value::as_str),
                _ => None,
            },
        };
        if let Err(error) = append(path, &line) {
            tracing::warn!(
                path = %path.display(),
                %error,
                "cannot write to the audit log"
            );
        }
    }
}

/// The current revision of the open document `doc`.
fn rev_of(engine: &Engine, doc: &str) -> Option<u64> {
    engine.docs.get(doc).map(|open| open.rev().0)
}

/// Appends `line` and a newline to the file at `path` in one write.
fn append(path: &Path, line: &AuditLine<'_>) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(line)?;
    bytes.push(b'\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(&bytes)
}

/// `text` cut to at most `max` bytes, at a character boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `bytes` as lowercase hexadecimal.
fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_names_are_cut_at_a_character_boundary() {
        assert_eq!(cut("glyph_get", 64), "glyph_get");
        let long = "é".repeat(40); // 80 bytes
        let short = cut(&long, 63);
        assert_eq!(short.len(), 62);
        assert!(long.starts_with(short));
    }

    #[test]
    fn digests_are_lowercase_hex() {
        assert_eq!(hex(&[0x00, 0xab, 0x7f]), "00ab7f");
        assert_eq!(hex(&sha256(b"")).len(), 64);
    }
}
