//! Typed edit commands, documents, revisions and history for FORMA fonts.
//!
//! Every mutation of a font - from the editor, the CLI or an AI agent - is a
//! [`Command`]. [`apply`] runs one command against a [`Font`](forma_model::Font)
//! and returns the edited font together with the set of entities it touched;
//! the input font is never modified, and a rejected command reports an
//! [`EditError`] and changes nothing.
//!
//! A [`Document`] runs commands as transactions on top of [`apply`]: it keeps
//! the font of every revision, rejects a transaction that touches an entity
//! someone else changed after the transaction's base revision
//! ([`EditError::Conflict`]), and records every change in an attributed
//! history that supports undo, redo, revert and restore. [`diff`] compares two
//! states of a font.
//!
//! # Conventions
//!
//! * Contours follow the UFO point convention of [`forma_geometry`]: off-curve
//!   nodes precede the on-curve node they lead into, a cubic segment is two
//!   off-curve nodes plus a `Curve`, a quadratic one is one off-curve node plus
//!   a `QCurve`, and a closed contour wraps around.
//! * Every contour entering a font must convert to a Bezier path with finite
//!   coordinates, otherwise the command fails with
//!   [`EditError::InvalidGeometry`].
//! * Editing commands never change the direction of a contour; only the
//!   `Reverse` contour action and the `WrongDirection` geometry fix do.

#![forbid(unsafe_code)]

mod apply;
mod command;
mod diff;
mod document;
mod error;
mod history;

pub use apply::{affected_glyph_count, apply};
pub use command::{Command, ContourAction, GlyphRef, NodeAction};
pub use diff::{FontDiff, GlyphDiff, diff};
pub use document::Document;
pub use error::{ConflictInfo, EditError};
pub use history::{EntryKind, HistoryEntry, TxId, TxOutcome};
