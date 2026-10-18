//! Tool errors: the stable error codes every facade reports, and the single
//! place where errors of the lower crates are mapped onto them.

use std::fmt;

use forma_compile::CompileError;
use forma_edit::{ConflictInfo, Document, EditError};
use forma_geometry::GeometryError;
use forma_interop::InteropError;
use forma_model::{EntityKey, Font, GlyphId, KernRef, KerningGroupId, Rev};
use forma_render::RenderError;
use forma_spacing::SpacingError;
use forma_store::StoreError;
use forma_trace::TraceError;
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
/// [`conflict_details`]; [`from_edit_error_in`] also names the glyphs.
#[must_use]
pub fn from_edit_error(error: EditError) -> ToolError {
    match error {
        EditError::NotFound(_) => ToolError::not_found(error.to_string()),
        EditError::InvalidArgument(message) | EditError::NotAllowed(message) => {
            ToolError::invalid_argument(message)
        }
        EditError::InvalidGeometry(message) => ToolError::new(ErrorCode::InvalidGeometry, message),
        EditError::LimitExceeded {
            message,
            limit,
            actual,
        } => ToolError::new(ErrorCode::LimitExceeded, message)
            .with_details(json!({ "limit": limit, "actual": actual })),
        EditError::Conflict(ref info) => {
            let details = conflict_details(info);
            ToolError::new(ErrorCode::RevisionConflict, error.to_string()).with_details(details)
        }
    }
}

/// Maps a [`GeometryError`] of `stroke` onto a [`ToolError`]: a skeleton or
/// style that cannot be stroked (`InvalidPath`) is `INVALID_ARGUMENT`; strokes
/// beyond the work budget of overlap removal (`TooComplex`) are
/// `LIMIT_EXCEEDED` with `details = { limit, actual }`; strokes that cannot be
/// united (`Degenerate`, or anything else) are `INVALID_GEOMETRY`.
#[must_use]
pub fn from_stroke_error(error: GeometryError) -> ToolError {
    match error {
        GeometryError::InvalidPath(_) => ToolError::invalid_argument(error.to_string()),
        GeometryError::TooComplex { limit, actual, .. } => {
            ToolError::new(ErrorCode::LimitExceeded, error.to_string())
                .with_details(json!({ "limit": limit, "actual": actual }))
        }
        _ => ToolError::new(ErrorCode::InvalidGeometry, error.to_string()),
    }
}

/// Maps a [`TraceError`] of the image tools onto a [`ToolError`]:
///
/// | `TraceError`                                      | code               |
/// |---------------------------------------------------|--------------------|
/// | `TooLarge`, `TooManyCandidates`                   | `LIMIT_EXCEEDED`   |
/// | `Decode`, `CandidateOutOfRange`, `Vectorize`,     | `INVALID_ARGUMENT` |
/// | `InvalidArgument`                                 |                    |
/// | `Render`, `Geometry`                              | `INVALID_GEOMETRY` |
#[must_use]
pub fn from_trace_error(error: TraceError) -> ToolError {
    match error {
        TraceError::TooLarge { bytes, pixels } => ToolError::new(
            ErrorCode::LimitExceeded,
            format!(
                "the image is too large ({bytes} bytes, {pixels} pixels); the limits are {} \
                 bytes and {} pixels: scale it down",
                forma_trace::MAX_IMAGE_BYTES,
                forma_trace::MAX_IMAGE_PIXELS
            ),
        ),
        TraceError::TooManyCandidates(count) => limit_error(
            format!(
                "the image has {count} candidates; the limit is {}: use a cleaner image or \
                 a stricter `tolerance`",
                forma_trace::MAX_CANDIDATES
            ),
            forma_trace::MAX_CANDIDATES,
            count,
        ),
        TraceError::Render(_) | TraceError::Geometry(_) => {
            ToolError::new(ErrorCode::InvalidGeometry, error.to_string())
        }
        _ => ToolError::invalid_argument(error.to_string()),
    }
}

fn limit_error(message: String, limit: usize, actual: usize) -> ToolError {
    ToolError::new(ErrorCode::LimitExceeded, message)
        .with_details(json!({ "limit": limit, "actual": actual }))
}

/// Maps a [`SpacingError`] of the spacing and kerning functions onto a
/// [`ToolError`]:
///
/// | `SpacingError`     | code               | why                                         |
/// |--------------------|--------------------|---------------------------------------------|
/// | `InvalidParameter` | `INVALID_ARGUMENT` | an argument is out of range                 |
/// | `MissingReference` | `INVALID_ARGUMENT` | `H`/`O` is missing or has no ink: the call  |
/// |                    |                    | is valid, the font cannot be measured as is |
/// | `MissingLayer`     | `INVALID_ARGUMENT` | a listed glyph has no layer in the master   |
/// | `Geometry`         | `INVALID_GEOMETRY` | an outline cannot be expanded               |
///
/// The glyphs and masters of a call are resolved before the functions run,
/// so none of these is a missing handle (`NOT_FOUND`): they are preconditions
/// on the font, which the caller fixes by drawing the reference glyph or
/// choosing other glyphs.
#[must_use]
pub fn from_spacing_error(error: SpacingError) -> ToolError {
    match error {
        SpacingError::Geometry(_) => ToolError::new(ErrorCode::InvalidGeometry, error.to_string()),
        _ => ToolError::invalid_argument(error.to_string()),
    }
}

/// [`from_edit_error`] for the error of a change of `document` (which a
/// failed change leaves as it was): the details of a conflict also name each
/// glyph, kerning group and kerning pair, see [`conflict_details_in`].
#[must_use]
pub fn from_edit_error_in(error: EditError, document: &Document) -> ToolError {
    match error {
        EditError::Conflict(ref info) => {
            let details = conflict_details_in(info, document);
            ToolError::new(ErrorCode::RevisionConflict, error.to_string()).with_details(details)
        }
        other => from_edit_error(other),
    }
}

/// [`conflict_details`] plus, for each conflicting glyph, kerning group and
/// kerning pair, its `name`:
///
/// * a glyph: its name, such as `"a"`;
/// * a kerning group: `@` and its name, such as `"@O"`;
/// * a kerning pair: `"<first>/<second>"`, each side the name of its glyph
///   or `@` and the name of its group, such as `"A/V"` or `"@O/V"`.
///
/// Each glyph or group is named as it is in `document` now or, when the
/// conflicting change deleted it, as it was just before that change. An
/// entity with a side that cannot be named that way gets no `name`.
/// `{ "current_rev": n, "conflicts": [{ "entity": "glyph:<uuid>", "rev": n,
/// "actor": "user", "name": "a" }] }`.
#[must_use]
pub fn conflict_details_in(info: &ConflictInfo, document: &Document) -> Value {
    let mut details = conflict_details(info);
    if let Some(Value::Array(conflicts)) = details.get_mut("conflicts") {
        for (conflict, (entity, rev, _)) in conflicts.iter_mut().zip(&info.conflicts) {
            let name = match entity {
                EntityKey::Glyph(id) => name_in(document, *rev, |font| glyph_name(font, *id)),
                EntityKey::KerningGroup(id) => {
                    name_in(document, *rev, |font| group_name(font, *id))
                }
                EntityKey::KerningPair(_, first, second) => {
                    name_in(document, *rev, |font| kern_ref_name(font, *first)).and_then(|first| {
                        name_in(document, *rev, |font| kern_ref_name(font, *second))
                            .map(|second| format!("{first}/{second}"))
                    })
                }
                EntityKey::FontInfo | EntityKey::VerticalMetrics(_) | EntityKey::GlyphOrder => None,
            };
            if let (Some(name), Some(conflict)) = (name, conflict.as_object_mut()) {
                conflict.insert("name".into(), Value::String(name));
            }
        }
    }
    details
}

/// What `name_of` finds in the current font of `document` or, failing that,
/// in its font just before revision `rev` (the conflicting change).
fn name_in(
    document: &Document,
    rev: Rev,
    name_of: impl Fn(&Font) -> Option<String>,
) -> Option<String> {
    name_of(document.font()).or_else(|| {
        let before = rev.0.checked_sub(1).map(Rev)?;
        let font = document.snapshot_at(before)?;
        name_of(&font)
    })
}

fn glyph_name(font: &Font, id: GlyphId) -> Option<String> {
    font.glyphs.get(&id).map(|glyph| glyph.name.clone())
}

fn group_name(font: &Font, id: KerningGroupId) -> Option<String> {
    font.kerning
        .groups
        .get(&id)
        .map(|group| format!("@{}", group.name))
}

fn kern_ref_name(font: &Font, reference: KernRef) -> Option<String> {
    match reference {
        KernRef::Glyph(id) => glyph_name(font, id),
        KernRef::Group(id) => group_name(font, id),
    }
}

/// The `details` of a `REVISION_CONFLICT`:
/// `{ "current_rev": n, "conflicts": [{ "entity": "glyph:<uuid>", "rev": n, "actor": "user" }] }`
/// (the engine adds each glyph's `name`, see [`conflict_details_in`]).
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

/// Maps a [`CompileError`] onto a [`ToolError`]; the message is the error
/// text.
///
/// | `CompileError` | code                |
/// |----------------|---------------------|
/// | `Preflight`    | `VALIDATION_FAILED` |
/// | `Fontc`        | `COMPILE_FAILED`    |
/// | `Woff2`        | `COMPILE_FAILED`    |
/// | `Io`           | `IO_ERROR`          |
///
/// A variant added later is `COMPILE_FAILED`.
#[must_use]
pub fn from_compile_error(error: CompileError) -> ToolError {
    let code = match &error {
        CompileError::Preflight(_) => ErrorCode::ValidationFailed,
        CompileError::Io(_) => ErrorCode::IoError,
        _ => ErrorCode::CompileFailed,
    };
    ToolError::new(code, error.to_string())
}

/// Maps an [`InteropError`] (a UFO export) onto a [`ToolError`]; the message
/// is the error text.
///
/// | `InteropError`                    | code               |
/// |-----------------------------------|--------------------|
/// | `Io` (also a write failure of the UFO library caused by the file system) | `IO_ERROR` |
/// | `DestinationExists`               | `INVALID_ARGUMENT` |
/// | `Norad`, `MissingLayer`, `UnknownMaster` (data that cannot be exported) | `COMPILE_FAILED` |
///
/// A variant added later is `COMPILE_FAILED`.
#[must_use]
pub fn from_interop_error(error: InteropError) -> ToolError {
    let code = match &error {
        InteropError::Io(_) => ErrorCode::IoError,
        InteropError::DestinationExists(_) => ErrorCode::InvalidArgument,
        _ => ErrorCode::CompileFailed,
    };
    ToolError::new(code, error.to_string())
}

/// The prefix of the [`RenderError::Geometry`] message of a preview whose
/// frame cannot be computed (an extent that is zero, overflows or scales
/// out of range): a problem of the requested framing, not of an outline.
const DEGENERATE_FRAMING: &str = "degenerate framing";

/// Maps a [`RenderError`] onto a [`ToolError`].
///
/// | `RenderError`                                   | code               |
/// |-------------------------------------------------|--------------------|
/// | `InvalidSize`, `InvalidText`                    | `INVALID_ARGUMENT` |
/// | `GlyphNotFound`, `MissingLayer`                 | `NOT_FOUND`        |
/// | `Geometry` starting with "degenerate framing"   | `INVALID_ARGUMENT` |
/// | any other `Geometry`                            | `INVALID_GEOMETRY` |
/// | `Encode`                                        | `IO_ERROR`         |
///
/// The message is the error text, except for a degenerate framing, whose
/// message is the framing problem itself (not "invalid outline: …").
#[must_use]
pub fn from_render_error(error: RenderError) -> ToolError {
    match error {
        RenderError::InvalidSize(_) | RenderError::InvalidText => {
            ToolError::invalid_argument(error.to_string())
        }
        RenderError::GlyphNotFound(_) | RenderError::MissingLayer { .. } => {
            ToolError::not_found(error.to_string())
        }
        RenderError::Geometry(message) if message.starts_with(DEGENERATE_FRAMING) => {
            ToolError::invalid_argument(message)
        }
        RenderError::Geometry(_) => ToolError::new(ErrorCode::InvalidGeometry, error.to_string()),
        RenderError::Encode(_) => ToolError::io(error.to_string()),
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
    fn spacing_errors_map_to_codes() {
        let code = |error| from_spacing_error(error).code;
        assert_eq!(
            code(SpacingError::InvalidParameter("x".into())),
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            code(SpacingError::MissingReference("x".into())),
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            code(SpacingError::MissingLayer("x".into())),
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            code(SpacingError::Geometry("x".into())),
            ErrorCode::InvalidGeometry
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

    #[test]
    fn compile_errors_map_to_codes() {
        let cases = [
            (
                CompileError::Preflight("no".into()),
                ErrorCode::ValidationFailed,
            ),
            (CompileError::Fontc("boom".into()), ErrorCode::CompileFailed),
            (CompileError::Woff2("bad".into()), ErrorCode::CompileFailed),
            (
                CompileError::Io(std::io::Error::other("disk")),
                ErrorCode::IoError,
            ),
        ];
        for (error, code) in cases {
            let text = error.to_string();
            let mapped = from_compile_error(error);
            assert_eq!(mapped.code, code, "{text}");
            assert_eq!(mapped.message, text);
        }
    }

    #[test]
    fn interop_errors_map_to_codes() {
        let master = forma_model::MasterId::new();
        let cases = [
            (
                InteropError::Io(std::io::Error::other("disk")),
                ErrorCode::IoError,
            ),
            (
                InteropError::DestinationExists("x.ufo".into()),
                ErrorCode::InvalidArgument,
            ),
            (
                InteropError::Norad("bad name".into()),
                ErrorCode::CompileFailed,
            ),
            (
                InteropError::MissingLayer {
                    glyph: "a".into(),
                    master,
                },
                ErrorCode::CompileFailed,
            ),
            (
                InteropError::UnknownMaster(master),
                ErrorCode::CompileFailed,
            ),
        ];
        for (error, code) in cases {
            let text = error.to_string();
            let mapped = from_interop_error(error);
            assert_eq!(mapped.code, code, "{text}");
            assert_eq!(mapped.message, text);
        }
    }

    #[test]
    fn render_errors_map_to_codes() {
        let cases = [
            (RenderError::InvalidSize(4), ErrorCode::InvalidArgument),
            (RenderError::InvalidText, ErrorCode::InvalidArgument),
            (RenderError::GlyphNotFound("a".into()), ErrorCode::NotFound),
            (
                RenderError::MissingLayer { glyph: "a".into() },
                ErrorCode::NotFound,
            ),
            (
                RenderError::Geometry("degenerate framing: the extent is 0".into()),
                ErrorCode::InvalidArgument,
            ),
            (
                RenderError::Geometry("contour 2 of 'a' has 1 node".into()),
                ErrorCode::InvalidGeometry,
            ),
            (RenderError::Encode("png".into()), ErrorCode::IoError),
        ];
        for (error, code) in cases {
            let text = error.to_string();
            assert_eq!(from_render_error(error).code, code, "{text}");
        }
        let framing = from_render_error(RenderError::Geometry(
            "degenerate framing: the extent is 0".into(),
        ));
        assert_eq!(framing.message, "degenerate framing: the extent is 0");
        // Only a message that starts with it is a framing problem.
        let outline = from_render_error(RenderError::Geometry(
            "glyph 'degenerate framing' is open".into(),
        ));
        assert_eq!(outline.code, ErrorCode::InvalidGeometry);
        assert!(outline.message.starts_with("invalid outline: "));
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
