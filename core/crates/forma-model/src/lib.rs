//! The FORMA typographic data model.
//!
//! Plain, serializable types shared by every other crate: stable identifiers,
//! revisions, actors, the font document with its masters, glyphs, components
//! and kerning, and the entity keys that revisions and conflicts are tracked
//! by.

#![forbid(unsafe_code)]

mod actor;
mod component;
mod font;
mod glyph;
mod ids;
mod kerning;
mod provenance;
mod rev;

pub use actor::Actor;
pub use component::{Component, MIN_TRANSFORM_DETERMINANT, Transform};
pub use font::{
    Font, FontInfo, MAX_UNITS_PER_EM, MIN_UNITS_PER_EM, Master, ModelError, VerticalMetrics,
};
pub use glyph::{Glyph, Layer, MAX_GLYPH_NAME_LEN, Node, NodeKind, Path, is_valid_glyph_name};
pub use ids::{EntityKey, FontId, GlyphId, KerningGroupId, MasterId, PathId};
pub use kerning::{KernRef, KernSide, Kerning, KerningGroup, KerningPair, MasterPairs};
pub use provenance::{Provenance, ProvenanceDetail, Review, Source};
pub use rev::Rev;
