//! The FORMA typographic data model.
//!
//! Plain, serializable types shared by every other crate: stable identifiers,
//! revisions, actors, the font document with its masters and glyphs, and the
//! entity keys that revisions and conflicts are tracked by.

#![forbid(unsafe_code)]

mod actor;
mod font;
mod glyph;
mod ids;
mod rev;

pub use actor::Actor;
pub use font::{
    Font, FontInfo, MAX_UNITS_PER_EM, MIN_UNITS_PER_EM, Master, ModelError, VerticalMetrics,
};
pub use glyph::{Glyph, Layer, MAX_GLYPH_NAME_LEN, Node, NodeKind, Path, is_valid_glyph_name};
pub use ids::{EntityKey, FontId, GlyphId, MasterId, PathId};
pub use rev::Rev;
