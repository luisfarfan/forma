//! Tool errors: the stable error codes every facade reports, and the single
//! place where errors of the lower crates are mapped onto them.

use std::fmt;

use forma_edit::{ConflictInfo, EditError};
use forma_store::StoreError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

/// The stable code of a [`ToolError`], serialized in `SCREAMING_SNAKE_CASE`
/// (for example `REVISION_CONFLICT`). These eleven codes are part of the
/// public API contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// An entity the call touches changed after its `base_rev`.
    RevisionConflict,
    /// The operation is destructive or massive and needs an approved plan.
    ApprovalRequired,
    /// A document handle, glyph, revision or file does not exist.
    NotFound,
    /// An argument is missing, malformed or has a value that is not accepted.
    InvalidArgument,
    /// The operation would produce a malformed outline.
    InvalidGeometry,
    /// A per-call limit was exceeded.
    LimitExceeded,
    /// A path lies outside the allowed roots.
    PathNotAllowed,
    /// The font could not be compiled.
    CompileFailed,
    /// The font failed validation.
    ValidationFailed,
    /// The operation is not supported (yet, or in the current mode).
    Unsupported,
    /// A file system operation failed.
    IoError,
}

impl ErrorCode {
    /// Every code, in declaration order.
    pub const ALL: [ErrorCode; 11] = [
        Self::RevisionConflict,
        Self::ApprovalRequired,
        Self::NotFound,
        Self::InvalidArgument,
        Self::InvalidGeometry,
        Self::LimitExceeded,
        Self::PathNotAllowed,
        Self::CompileFailed,
        Self::ValidationFailed,
        Self::Unsupported,
        Self::IoError,
    ];

    /// The serialized form of the code, such as `"NOT_FOUND"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RevisionConflict => "REVISION_CONFLICT",
            Self::ApprovalRequired => "APPROVAL_REQUIRED",
            Self::NotFound => "NOT_FOUND",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::InvalidGeometry => "INVALID_GEOMETRY",
            Self::LimitExceeded => "LIMIT_EXCEEDED",
            Self::PathNotAllowed => "PATH_NOT_ALLOWED",
            Self::CompileFailed => "COMPILE_FAILED",
            Self::ValidationFailed => "VALIDATION_FAILED",
            Self::Unsupported => "UNSUPPORTED",
            Self::IoError => "IO_ERROR",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error of a tool call: a stable [`ErrorCode`], a human- and
/// agent-readable message, and structured `details` (`null` when there are
/// none).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Error)]
#[error("{code}: {message}")]
pub struct ToolError {
    /// The stable error code.
    pub code: ErrorCode,
    /// What went wrong, phrased so that the caller can correct the call.
    pub message: String,
    /// Structured details, such as the conflicts of a `REVISION_CONFLICT`.
    #[serde(default)]
    pub details: Value,
}

impl ToolError {
    /// An error with the given code and message and no details.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ToolError {
        ToolError {
            code,
            message: message.into(),
            details: Value::Null,
        }
    }

    /// The same error with `details` attached.
    #[must_use]
    pub fn with_details(mut self, details: Value) -> ToolError {
        self.details = details;
        self
    }

    /// An `INVALID_ARGUMENT` error.
    #[must_use]
    pub fn invalid_argument(message: impl Into<String>) -> ToolError {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    /// A `NOT_FOUND` error.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> ToolError {
        Self::new(ErrorCode::NotFound, message)
    }

    /// An `UNSUPPORTED` error.
    #[must_use]
    pub fn unsupported(message: impl Into<String>) -> ToolError {
        Self::new(ErrorCode::Unsupported, message)
    }

    /// A `PATH_NOT_ALLOWED` error.
    #[must_use]
    pub fn path_not_allowed(message: impl Into<String>) -> ToolError {
        Self::new(ErrorCode::PathNotAllowed, message)
    }

    /// An `IO_ERROR` error.
    #[must_use]
    pub fn io(message: impl Into<String>) -> ToolError {
        Self::new(ErrorCode::IoError, message)
    }
}

/// Maps an [`EditError`] onto a [`ToolError`].
///
/// | `EditError`       | code                | message                         |
/// |-------------------|---------------------|---------------------------------|
/// | `NotFound`        | `NOT_FOUND`         | `"not found: <what>"`           |
/// | `InvalidArgument` | `INVALID_ARGUMENT`  | the inner message               |
/// | `InvalidGeometry` | `INVALID_GEOMETRY`  | the inner message               |
/// | `Conflict`        | `REVISION_CONFLICT` | the error text; details below   |
/// | `NotAllowed`      | `INVALID_ARGUMENT`  | the inner message, unchanged    |
///
/// The details of a conflict are
/// `{ "current_rev": n, "conflicts": [{ "entity", "rev", "actor" }] }`, see
/// [`conflict_details`].
#[must_use]
pub fn from_edit_error(error: EditError) -> ToolError {
    match error {
        EditError::NotFound(_) => ToolError::not_found(error.to_string()),
        EditError::InvalidArgument(message) | EditError::NotAllowed(message) => {
            ToolError::invalid_argument(message)
        }
        EditError::InvalidGeometry(message) => ToolError::new(ErrorCode::InvalidGeometry, message),
        EditError::Conflict(ref info) => {
            let details = conflict_details(info);
            ToolError::new(ErrorCode::RevisionConflict, error.to_string()).with_details(details)
        }
    }
}

/// The `details` of a `REVISION_CONFLICT`:
/// `{ "current_rev": n, "conflicts": [{ "entity": "glyph:<uuid>", "rev": n, "actor": "user" }] }`.
#[must_use]
pub fn conflict_details(info: &ConflictInfo) -> Value {
    let conflicts: Vec<Value> = info
        .conflicts
        .iter()
        .map(|(entity, rev, actor)| {
            json!({
                "entity": entity.to_string(),
                "rev": rev.0,
                "actor": actor.to_string(),
            })
        })
        .collect();
    json!({ "current_rev": info.current_rev.0, "conflicts": conflicts })
}

/// Maps a [`StoreError`] onto a [`ToolError`]; the message is the error text.
///
/// * A symbolic link inside a package ([`StoreError::SymlinkInPackage`]) is
///   `PATH_NOT_ALLOWED`: it could lead outside the allowed roots.
/// * Failures of the environment ([`StoreError::is_io_failure`]: file system
///   errors, and SQLite reporting a full disk, an I/O error, a file it cannot
///   open, a read-only, busy or locked database or a denied permission) are
///   `IO_ERROR`.
/// * Everything else (unknown format, not a FORMA document, unsupported
///   version, invalid or damaged content, other SQLite errors such as "not a
///   database") is `INVALID_ARGUMENT`.
#[must_use]
pub fn from_store_error(error: StoreError) -> ToolError {
    if matches!(error, StoreError::SymlinkInPackage(_)) {
        ToolError::path_not_allowed(error.to_string())
    } else if error.is_io_failure() {
        ToolError::io(error.to_string())
    } else {
        ToolError::invalid_argument(error.to_string())
    }
}

/// Maps a failure to deserialize the arguments of a tool onto
/// `INVALID_ARGUMENT`, keeping serde's message (which names the offending or
/// unknown field).
#[must_use]
pub fn from_args_error(error: &serde_json::Error) -> ToolError {
    ToolError::invalid_argument(format!("invalid arguments: {error}"))
}

impl From<EditError> for ToolError {
    fn from(error: EditError) -> Self {
        from_edit_error(error)
    }
}

impl From<StoreError> for ToolError {
    fn from(error: StoreError) -> Self {
        from_store_error(error)
    }
}

#[cfg(test)]
mod tests {
    use forma_model::{Actor, EntityKey, GlyphId, Rev};

    use super::*;

    #[test]
    fn codes_serialize_to_the_contract_strings() {
        let expected = [
            "REVISION_CONFLICT",
            "APPROVAL_REQUIRED",
            "NOT_FOUND",
            "INVALID_ARGUMENT",
            "INVALID_GEOMETRY",
            "LIMIT_EXCEEDED",
            "PATH_NOT_ALLOWED",
            "COMPILE_FAILED",
            "VALIDATION_FAILED",
            "UNSUPPORTED",
            "IO_ERROR",
        ];
        for (code, text) in ErrorCode::ALL.iter().zip(expected) {
            assert_eq!(serde_json::to_value(code).ok(), Some(json!(text)));
            assert_eq!(code.as_str(), text);
            assert_eq!(code.to_string(), text);
        }
    }

    #[test]
    fn error_serializes_with_null_details_by_default() {
        let error = ToolError::not_found("unknown document handle");
        assert_eq!(
            serde_json::to_value(&error).ok(),
            Some(json!({
                "code": "NOT_FOUND",
                "message": "unknown document handle",
                "details": null
            }))
        );
    }

    #[test]
    fn edit_errors_map_to_codes() {
        let cases = [
            (EditError::NotFound("glyph 'a'".into()), ErrorCode::NotFound),
            (
                EditError::InvalidArgument("bad".into()),
                ErrorCode::InvalidArgument,
            ),
            (
                EditError::InvalidGeometry("open".into()),
                ErrorCode::InvalidGeometry,
            ),
            (
                EditError::NotAllowed("nothing to undo".into()),
                ErrorCode::InvalidArgument,
            ),
        ];
        for (error, code) in cases {
            assert_eq!(from_edit_error(error).code, code);
        }
        let not_allowed = from_edit_error(EditError::NotAllowed(
            "last transaction belongs to user; use history_revert".into(),
        ));
        assert_eq!(
            not_allowed.message,
            "last transaction belongs to user; use history_revert"
        );
    }

    #[test]
    fn conflict_maps_with_details() {
        let glyph = GlyphId::new();
        let error = from_edit_error(EditError::Conflict(ConflictInfo {
            current_rev: Rev(7),
            conflicts: vec![(EntityKey::Glyph(glyph), Rev(6), Actor::User)],
        }));
        assert_eq!(error.code, ErrorCode::RevisionConflict);
        assert_eq!(
            error.details,
            json!({
                "current_rev": 7,
                "conflicts": [{ "entity": format!("glyph:{glyph}"), "rev": 6, "actor": "user" }]
            })
        );
    }

    fn sqlite(code: std::os::raw::c_int) -> StoreError {
        StoreError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            Some("detail".into()),
        ))
    }

    #[test]
    fn store_errors_map_io_and_the_rest() {
        let io = from_store_error(StoreError::Io(std::io::Error::other("disk full")));
        assert_eq!(io.code, ErrorCode::IoError);
        let invalid = from_store_error(StoreError::Invalid("broken".into()));
        assert_eq!(invalid.code, ErrorCode::InvalidArgument);
        assert_eq!(invalid.message, "invalid document: broken");
        let link = from_store_error(StoreError::SymlinkInPackage("pkg/glyphs".into()));
        assert_eq!(link.code, ErrorCode::PathNotAllowed);
        assert!(link.message.contains("pkg/glyphs"), "{}", link.message);
    }

    #[test]
    fn sqlite_environment_failures_are_io_errors() {
        use rusqlite::ffi;
        for code in [
            ffi::SQLITE_FULL,
            ffi::SQLITE_IOERR,
            ffi::SQLITE_IOERR_READ,
            ffi::SQLITE_IOERR_WRITE,
            ffi::SQLITE_IOERR_FSYNC,
            ffi::SQLITE_CANTOPEN,
            ffi::SQLITE_READONLY,
            ffi::SQLITE_PERM,
            ffi::SQLITE_BUSY,
            ffi::SQLITE_LOCKED,
        ] {
            assert_eq!(
                from_store_error(sqlite(code)).code,
                ErrorCode::IoError,
                "code {code}"
            );
        }
        for code in [
            ffi::SQLITE_NOTADB,
            ffi::SQLITE_CORRUPT,
            ffi::SQLITE_CONSTRAINT,
        ] {
            assert_eq!(
                from_store_error(sqlite(code)).code,
                ErrorCode::InvalidArgument,
                "code {code}"
            );
        }
        assert_eq!(
            from_store_error(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows)).code,
            ErrorCode::InvalidArgument
        );
    }
}
