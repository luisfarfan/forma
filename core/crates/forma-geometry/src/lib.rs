//! Outline geometry for FORMA.
//!
//! Converts model contours to and from [`kurbo`] Bezier paths, imports SVG
//! path data, and computes signed area, bounding boxes, contour nesting and
//! contour direction.
//!
//! # Contour convention
//!
//! Contours follow the UFO point convention used by [`forma_model::Path`]:
//!
//! * On-curve nodes are [`NodeKind::Line`], [`NodeKind::Curve`] (cubic) and
//!   [`NodeKind::QCurve`] (quadratic); the kind describes the segment that
//!   *arrives* at the node.
//! * [`NodeKind::OffCurve`] nodes precede the on-curve node they lead into. A
//!   cubic segment is exactly two off-curve nodes followed by a `Curve`; a
//!   quadratic segment is one off-curve node followed by a `QCurve`.
//! * In a closed contour the segment that arrives at the first on-curve node
//!   starts at the last on-curve node, so its off-curve nodes may sit at the
//!   end of the list (or at the start, before the first on-curve node).
//! * An open contour starts with an on-curve node, which is its move-to; its
//!   kind is treated as `Line`.
//!
//! Coordinates are `f64` in font units with the y axis pointing up. Contours
//! follow PostScript direction: outer contours counter-clockwise (positive
//! signed area), inner contours clockwise.
//!
//! [`NodeKind::Line`]: forma_model::NodeKind::Line
//! [`NodeKind::Curve`]: forma_model::NodeKind::Curve
//! [`NodeKind::QCurve`]: forma_model::NodeKind::QCurve
//! [`NodeKind::OffCurve`]: forma_model::NodeKind::OffCurve

#![forbid(unsafe_code)]

mod convert;
mod direction;
mod svg;

pub use convert::{bbox, from_bezpath, to_bezpath};
pub use direction::{contour_depths, normalize_direction, signed_area};
pub use svg::{CoordinateSystem, path_to_svg, paths_from_svg};

/// Tolerance, in font units, used when flattening curves for area and
/// containment computations.
pub(crate) const FLATTEN_TOLERANCE: f64 = 0.25;

/// An error raised while converting or measuring contours.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GeometryError {
    /// The contour or the SVG data is malformed: an off-curve sequence that is
    /// not terminated by a suitable on-curve node, a segment with the wrong
    /// number of off-curve nodes, a non-finite coordinate, or unparsable SVG.
    #[error("invalid path: {0}")]
    InvalidPath(String),
    /// The contour is well formed but has too few points to enclose anything.
    #[error("degenerate path: {0}")]
    Degenerate(String),
}
