//! Pure typed edit commands for FORMA fonts.
//!
//! Every mutation of a font - from the editor, the CLI or an AI agent - is a
//! [`Command`]. [`apply`] runs one command against a [`Font`](forma_model::Font)
//! and returns the edited font together with the set of entities it touched;
//! the input font is never modified, and a rejected command reports an
//! [`EditError`] and changes nothing. Documents, revisions and history are
//! layered on top of this crate.
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
mod error;

pub use apply::{affected_glyph_count, apply};
pub use command::{Command, ContourAction, GlyphRef, NodeAction};
pub use error::{ConflictInfo, EditError};
