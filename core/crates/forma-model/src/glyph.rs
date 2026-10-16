//! Glyphs, layers and outlines.
//!
//! Coordinates are `f64` in font units with the y axis pointing up. Contours
//! follow PostScript direction: outer contours counter-clockwise (positive
//! signed area), inner contours clockwise.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{GlyphId, MasterId, PathId};

/// The longest glyph name allowed, in characters.
pub const MAX_GLYPH_NAME_LEN: usize = 63;

/// The only glyph name that may start with a period.
const NOTDEF: &str = ".notdef";

/// Returns whether `name` is an acceptable glyph name.
///
/// A valid name is exactly `.notdef`, or 1 to 63 printable ASCII characters
/// (`!` through `~`, so no spaces) that does not start with a digit or a
/// period.
#[must_use]
pub fn is_valid_glyph_name(name: &str) -> bool {
    if name == NOTDEF {
        return true;
    }
    // Names are ASCII-only, so the byte length is the character count; any
    // multi-byte character is rejected by the character check below anyway.
    if name.is_empty() || name.len() > MAX_GLYPH_NAME_LEN {
        return false;
    }
    if !name.bytes().all(|b| matches!(b, 0x21..=0x7E)) {
        return false;
    }
    !name.starts_with(|c: char| c.is_ascii_digit() || c == '.')
}

/// The role of a node within a contour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum NodeKind {
    /// An on-curve point reached by a straight segment.
    Line,
    /// An on-curve point reached by a cubic Bezier segment.
    Curve,
    /// An on-curve point reached by a quadratic Bezier segment.
    QCurve,
    /// An off-curve control point.
    OffCurve,
}

/// A point of a contour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Node {
    /// Horizontal position in font units.
    pub x: f64,
    /// Vertical position in font units (y axis up).
    pub y: f64,
    /// The role of the node.
    pub kind: NodeKind,
    /// Whether the node joins its neighbouring segments smoothly.
    pub smooth: bool,
}

/// A contour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Path {
    /// Identifier of the contour, unique within its layer.
    pub id: PathId,
    /// Whether the contour is closed.
    pub closed: bool,
    /// The nodes of the contour, in drawing order.
    pub nodes: Vec<Node>,
}

/// The outline of a glyph in one master.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Layer {
    /// Horizontal advance in font units.
    pub advance_width: f64,
    /// The contours of the layer.
    pub paths: Vec<Path>,
}

/// A glyph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Glyph {
    /// Stable identifier of the glyph.
    pub id: GlyphId,
    /// The glyph name; see [`is_valid_glyph_name`].
    pub name: String,
    /// Unicode code points encoded by the glyph.
    pub unicodes: Vec<u32>,
    /// Whether the glyph is included when the font is exported.
    pub export: bool,
    /// The outline for each master the glyph has been drawn in.
    pub layers: BTreeMap<MasterId, Layer>,
}
