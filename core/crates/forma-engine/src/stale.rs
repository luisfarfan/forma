//! Stale references: a call based on revision `base_rev` that names a glyph
//! another actor renamed or deleted since gets `REVISION_CONFLICT` about
//! that glyph, like any other change it would overwrite, rather than a bare
//! `NOT_FOUND` that leaves the caller guessing.

use std::collections::BTreeSet;

use forma_edit::{EditError, GlyphRef};
use forma_model::{EntityKey, Rev};
use serde_json::Value;

use crate::engine::Engine;
use crate::errors::{ErrorCode, ToolError, conflict_details_in};

/// The glyph reference of a `NOT_FOUND` error about a glyph: the text of
/// `not found: glyph '<ref>'`, also when an `edit_batch` operation prefixed
/// it (`operation <n>: …`). Both messages are this crate's (and
/// forma-edit's) own.
fn missing_glyph(message: &str) -> Option<&str> {
    let message = match message.strip_prefix("operation ") {
        Some(rest) => {
            let (index, rest) = rest.split_once(": ")?;
            if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            rest
        }
        None => message,
    };
    message
        .strip_prefix("not found: glyph '")?
        .strip_suffix('\'')
}

impl Engine {
    /// Turns the `NOT_FOUND` `error` of a call on the document `doc` based
    /// on `base_rev` into `REVISION_CONFLICT` when the glyph it could not
    /// find existed at `base_rev` (the same reference finds it in that
    /// revision) and no longer answers to that reference (another change
    /// renamed or deleted it after `base_rev`). The details are those of any
    /// conflict, naming the glyph, plus the `op_index` of an `edit_batch`
    /// operation. Any other error is returned as it is.
    pub(crate) fn stale_reference(
        &self,
        doc: Option<&str>,
        base_rev: Option<u64>,
        error: ToolError,
    ) -> ToolError {
        if error.code != ErrorCode::NotFound {
            return error;
        }
        let (Some(doc), Some(base_rev)) = (doc, base_rev) else {
            return error;
        };
        let Some(text) = missing_glyph(&error.message) else {
            return error;
        };
        let Some(open) = self.docs.get(doc) else {
            return error;
        };
        let Some(then) = open.document.snapshot_at(Rev(base_rev)) else {
            return error;
        };
        let Ok(reference) = text.parse::<GlyphRef>();
        let Some(id) = reference.resolve(&then) else {
            return error;
        };
        // Still there under that reference: the failure is the call's own
        // (an earlier operation of a batch deleted it, say).
        if reference.resolve(open.font()).is_some() {
            return error;
        }
        let touched = BTreeSet::from([EntityKey::Glyph(id)]);
        let Err(EditError::Conflict(info)) = open.document.check_conflicts(&touched, Rev(base_rev))
        else {
            return error;
        };
        let mut details = conflict_details_in(&info, &open.document);
        if let (Some(index), Value::Object(details)) = (error.details.get("op_index"), &mut details)
        {
            details.insert("op_index".into(), index.clone());
        }
        ToolError::new(
            ErrorCode::RevisionConflict,
            format!(
                "glyph '{text}' was renamed or deleted after base_rev {base_rev} (see \
                 details.conflicts); re-read the document and retry"
            ),
        )
        .with_details(details)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_glyph_messages_are_recognized() {
        assert_eq!(missing_glyph("not found: glyph 'a'"), Some("a"));
        assert_eq!(missing_glyph("not found: glyph 'it's'"), Some("it's"));
        assert_eq!(
            missing_glyph("operation 12: not found: glyph 'b'"),
            Some("b")
        );
        assert_eq!(missing_glyph("not found: master 'x'"), None);
        assert_eq!(missing_glyph("not found: contour 1 of glyph 'a'"), None);
        assert_eq!(missing_glyph("operation x: not found: glyph 'b'"), None);
        assert_eq!(missing_glyph("unknown document handle"), None);
        // The messages of the lower crates keep this shape.
        let edit = crate::errors::from_edit_error(EditError::NotFound("glyph 'q'".into()));
        assert_eq!(missing_glyph(&edit.message), Some("q"));
    }
}
