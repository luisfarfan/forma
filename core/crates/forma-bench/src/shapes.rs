//! Small vector primitives for the generated cases. Every shape is built
//! from IEEE-exact operations only (no platform trigonometry), and every
//! solid has the same orientation while every hole has the opposite one, so
//! the non-zero fill rule of the rasterizer gives the union of the solids
//! minus the holes.

use kurbo::{Affine, BezPath, Point, Shape};

/// The control-point distance of a quarter circle, as a fraction of the
/// radius.
const KAPPA: f64 = 0.552_284_749_830_793_4;

/// `path` with the orientation asked for: solids positive, holes negative.
#[must_use]
pub fn oriented(path: BezPath, solid: bool) -> BezPath {
    if (path.area() > 0.0) == solid {
        path
    } else {
        BezPath::from_vec(path.reverse_subpaths().elements().to_vec())
    }
}

/// A closed polygon through `points`.
#[must_use]
pub fn polygon(points: &[Point], solid: bool) -> BezPath {
    let mut path = BezPath::new();
    if let Some((&first, rest)) = points.split_first() {
        path.move_to(first);
        for &p in rest {
            path.line_to(p);
        }
        path.close_path();
    }
    oriented(path, solid)
}

/// An axis-aligned rectangle.
#[must_use]
pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64, solid: bool) -> BezPath {
    polygon(
        &[
            Point::new(x0, y0),
            Point::new(x1, y0),
            Point::new(x1, y1),
            Point::new(x0, y1),
        ],
        solid,
    )
}

/// An ellipse made of four cubic arcs.
#[must_use]
pub fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64, solid: bool) -> BezPath {
    let (kx, ky) = (KAPPA * rx, KAPPA * ry);
    let mut p = BezPath::new();
    p.move_to((cx + rx, cy));
    p.curve_to((cx + rx, cy + ky), (cx + kx, cy + ry), (cx, cy + ry));
    p.curve_to((cx - kx, cy + ry), (cx - rx, cy + ky), (cx - rx, cy));
    p.curve_to((cx - rx, cy - ky), (cx - kx, cy - ry), (cx, cy - ry));
    p.curve_to((cx + kx, cy - ry), (cx + rx, cy - ky), (cx + rx, cy));
    p.close_path();
    oriented(p, solid)
}

/// A circle.
#[must_use]
pub fn circle(cx: f64, cy: f64, r: f64, solid: bool) -> BezPath {
    ellipse(cx, cy, r, r, solid)
}

/// A parallelogram whose top edge is shifted by `skew` to the right of the
/// bottom edge (slanted stems).
#[must_use]
pub fn slanted_bar(x0: f64, y0: f64, x1: f64, y1: f64, skew: f64) -> BezPath {
    polygon(
        &[
            Point::new(x0 + skew, y0),
            Point::new(x1 + skew, y0),
            Point::new(x1, y1),
            Point::new(x0, y1),
        ],
        true,
    )
}

/// `shapes` moved by `(dx, dy)`.
#[must_use]
pub fn moved(shapes: &[BezPath], dx: f64, dy: f64) -> Vec<BezPath> {
    let t = Affine::translate((dx, dy));
    shapes.iter().map(|s| t * s.clone()).collect()
}
