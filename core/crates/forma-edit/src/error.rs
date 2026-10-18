//! Errors raised while applying edit commands.

use forma_model::{Actor, EntityKey, Rev};
use thiserror::Error;

/// Details of a revision conflict: which entities changed after the revision
/// the caller based its edit on.
///
/// The document layer fills this in; the pure commands never raise it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictInfo {
    /// The current revision of the document.
    pub current_rev: Rev,
    /// Each conflicting entity with the revision it was last changed at and
    /// the actor who changed it.
    pub conflicts: Vec<(EntityKey, Rev, Actor)>,
}

/// Why an edit command was rejected.
///
/// A rejected command never changes the font it was applied to.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum EditError {
    /// A glyph, master or contour the command refers to does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// An argument has a value the command cannot accept.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The command would produce a malformed outline: a contour that cannot
    /// be converted to a Bezier path, or a non-finite coordinate.
    #[error("invalid geometry: {0}")]
    InvalidGeometry(String),
    /// Another actor changed an entity the command touches.
    #[error(
        "revision conflict: {} entities changed since the base revision (current revision {})",
        .0.conflicts.len(),
        .0.current_rev.0
    )]
    Conflict(ConflictInfo),
    /// The actor is not allowed to perform the operation.
    #[error("not allowed: {0}")]
    NotAllowed(String),
    /// The command is beyond a work budget (overlap removal of very complex
    /// contours): `actual` against `limit`.
    #[error("limit exceeded: {message}")]
    LimitExceeded {
        /// What was too large, and what to do about it.
        message: String,
        /// The budget.
        limit: u64,
        /// The amount the command needed.
        actual: u64,
    },
}
