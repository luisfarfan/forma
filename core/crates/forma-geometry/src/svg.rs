//! SVG path data import and export.

use forma_model::Path;
use kurbo::{BezPath, PathEl, Point};

use crate::GeometryError;
use crate::convert::{from_bezpath, to_bezpath};

/// Where the y axis of imported SVG data points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CoordinateSystem {
    /// Font coordinates: the y axis points up; coordinates are used as given.
    Font,
    /// SVG coordinates: the y axis points down. Each y becomes
    /// `ascender - y`, so the top of the em box (y = 0 in SVG) lands on the
    /// ascender.
    SvgYDown {
        /// The font-space y of the SVG origin.
        ascender: f64,
    },
}

/// Parses SVG path data (the `d` attribute) into model contours.
///
/// All commands are supported: relative commands, `H`/`V`, smooth curves and
/// arcs, which arrive as cubic segments. Each subpath becomes one contour with
/// ids `0, 1, 2, ...` in order; callers that need other ids reassign them.
/// A closed subpath whose last segment returns to its start does not repeat
/// the start node. Empty data yields no contours.
///
/// The direction of every contour is preserved as written: this function never
/// re-orients geometry. Note that [`CoordinateSystem::SvgYDown`] mirrors the
/// orientation (a contour that is counter-clockwise as written ends up
/// clockwise in font coordinates). Use
/// [`normalize_direction`](crate::normalize_direction) to apply PostScript
/// direction explicitly.
///
/// # Errors
///
/// [`GeometryError::InvalidPath`] if the data cannot be parsed or yields a
/// non-finite coordinate; [`GeometryError::Degenerate`] if a closed subpath has
/// fewer than two distinct on-curve points.
pub fn paths_from_svg(d: &str, cs: CoordinateSystem) -> Result<Vec<Path>, GeometryError> {
    let bez = BezPath::from_svg(d)
        .map_err(|e| GeometryError::InvalidPath(format!("malformed SVG path data: {e}")))?;
    let bez = match cs {
        CoordinateSystem::Font => bez,
        CoordinateSystem::SvgYDown { ascender } => flip_y(&bez, ascender),
    };
    from_bezpath(&bez, 0)
}

fn flip_y(bez: &BezPath, ascender: f64) -> BezPath {
    let flip = |p: Point| Point::new(p.x, ascender - p.y);
    BezPath::from_vec(
        bez.elements()
            .iter()
            .map(|el| match *el {
                PathEl::MoveTo(p) => PathEl::MoveTo(flip(p)),
                PathEl::LineTo(p) => PathEl::LineTo(flip(p)),
                PathEl::QuadTo(c, p) => PathEl::QuadTo(flip(c), flip(p)),
                PathEl::CurveTo(c1, c2, p) => PathEl::CurveTo(flip(c1), flip(c2), flip(p)),
                PathEl::ClosePath => PathEl::ClosePath,
            })
            .collect(),
    )
}

/// Formats a number without a trailing `.0` and without negative zero.
fn number(v: f64) -> String {
    if v == 0.0 {
        "0".to_owned()
    } else {
        v.to_string()
    }
}

fn coords(points: &[Point]) -> String {
    points
        .iter()
        .map(|p| format!("{} {}", number(p.x), number(p.y)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Renders a contour as SVG path data in font coordinates (no y flip), using
/// the commands `M`, `L`, `C`, `Q` and `Z`.
///
/// Numbers are written without trailing zeros (`100`, `12.5`). A contour that
/// cannot be represented (see [`to_bezpath`] for the conditions) yields an
/// empty string.
#[must_use]
pub fn path_to_svg(path: &Path) -> String {
    let Ok(bez) = to_bezpath(path) else {
        return String::new();
    };
    bez.elements()
        .iter()
        .map(|el| match *el {
            PathEl::MoveTo(p) => format!("M{}", coords(&[p])),
            PathEl::LineTo(p) => format!("L{}", coords(&[p])),
            PathEl::QuadTo(c, p) => format!("Q{}", coords(&[c, p])),
            PathEl::CurveTo(c1, c2, p) => format!("C{}", coords(&[c1, c2, p])),
            PathEl::ClosePath => "Z".to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}
