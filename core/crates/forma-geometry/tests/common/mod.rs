//! Helpers shared by the integration tests of this crate.
#![allow(dead_code)]

use forma_geometry::to_bezpath;
use forma_model::{Node, NodeKind, Path, PathId};
use kurbo::{Line, ParamCurve, ParamCurveArclen, PathEl, Point, Rect, Shape};
use proptest::test_runner::{Config, RngSeed};

// ------------------------------------------------------- property tests

/// The seed every property test of this crate starts from ("FORMA").
const PROPTEST_SEED: u64 = 0x46_4f_52_4d_41;

/// The configuration of a property test: `cases` cases generated from a
/// fixed seed, so every run (CI included) tests the same cases. Random
/// strokes and unions can hit rare failures of the overlap-removal engine
/// that are known and out of our hands (see `union_keeps_the_whole_stroke`
/// in `stroke.rs`); a fresh seed on every run would turn CI red at random.
///
/// Set `FORMA_PROPTEST_RANDOM=1` to explore with a fresh random seed
/// instead, for example with many more cases:
/// `FORMA_PROPTEST_RANDOM=1 cargo test -p forma-geometry --release`.
pub fn proptest_config(cases: u32) -> Config {
    let random = std::env::var_os("FORMA_PROPTEST_RANDOM").is_some_and(|value| value == "1");
    Config {
        cases,
        rng_seed: if random {
            RngSeed::Random
        } else {
            RngSeed::Fixed(PROPTEST_SEED)
        },
        ..Config::default()
    }
}

// ------------------------------------------------------------------ nodes

pub fn node(x: f64, y: f64, kind: NodeKind) -> Node {
    Node {
        x,
        y,
        kind,
        smooth: false,
    }
}

pub fn line(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::Line)
}

pub fn off(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::OffCurve)
}

pub fn curve(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::Curve)
}

pub fn closed(nodes: Vec<Node>) -> Path {
    Path {
        id: PathId(0),
        closed: true,
        nodes,
    }
}

// ----------------------------------------------------------------- shapes

/// Handle length of a cubic quarter circle of radius 1.
const KAPPA: f64 = 0.552_284_749_830_793_4;

/// A circle of four cubic quarter arcs, counter-clockwise or clockwise.
pub fn circle(cx: f64, cy: f64, r: f64, counter_clockwise: bool) -> Path {
    let k = KAPPA * r;
    // Counter-clockwise from the rightmost point; the closing segment's
    // handles are the last two nodes.
    let mut nodes = vec![
        curve(cx + r, cy),
        off(cx + r, cy + k),
        off(cx + k, cy + r),
        curve(cx, cy + r),
        off(cx - k, cy + r),
        off(cx - r, cy + k),
        curve(cx - r, cy),
        off(cx - r, cy - k),
        off(cx - k, cy - r),
        curve(cx, cy - r),
        off(cx + k, cy - r),
        off(cx + r, cy - k),
    ];
    if !counter_clockwise {
        // Mirroring about the horizontal axis through the centre keeps the
        // circle and reverses its direction.
        for n in &mut nodes {
            n.y = 2.0 * cy - n.y;
        }
    }
    closed(nodes)
}

/// An axis-aligned rectangle, counter-clockwise or clockwise.
pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64, counter_clockwise: bool) -> Path {
    let mut nodes = vec![line(x0, y0), line(x1, y0), line(x1, y1), line(x0, y1)];
    if !counter_clockwise {
        nodes.reverse();
    }
    closed(nodes)
}

// ------------------------------------------------------------- measuring

/// The exact signed area of a closed contour (positive counter-clockwise).
pub fn area(path: &Path) -> f64 {
    to_bezpath(path).expect("valid contour").area()
}

/// The signed area of every contour.
pub fn areas(paths: &[Path]) -> Vec<f64> {
    paths.iter().map(area).collect()
}

/// The total signed area of the contours.
pub fn total_area(paths: &[Path]) -> f64 {
    paths.iter().map(area).sum()
}

/// The non-zero winding number of `p` with respect to all the contours,
/// counted on the contours flattened to within 0.001 units (kurbo's own
/// `winding` can be off within about a unit of a curve).
pub fn winding(paths: &[Path], p: Point) -> i32 {
    polygon_winding(&polygons(paths, 0.001), p)
}

/// The non-zero winding number of `p` with respect to closed polygons (see
/// [`polygons`]); flatten once with a tolerance of 0.001 to probe many points.
pub fn polygon_winding(polygons: &[Vec<Point>], p: Point) -> i32 {
    let mut total = 0;
    for polygon in polygons {
        for (i, &a) in polygon.iter().enumerate() {
            let b = polygon[(i + 1) % polygon.len()];
            let side = (b - a).cross(p - a);
            if a.y <= p.y && b.y > p.y && side > 0.0 {
                total += 1;
            } else if a.y > p.y && b.y <= p.y && side < 0.0 {
                total -= 1;
            }
        }
    }
    total
}

/// The horizontal extent of the outline at height `y`: the distance between
/// the leftmost and the rightmost crossing of the horizontal line.
pub fn width_at(paths: &[Path], y: f64) -> f64 {
    let probe = Line::new((-1.0e5, y), (1.0e5, y));
    let mut xs: Vec<f64> = Vec::new();
    for path in paths {
        let bez = to_bezpath(path).expect("valid contour");
        for segment in bez.segments() {
            for hit in segment.intersect_line(probe) {
                xs.push(probe.eval(hit.line_t).x);
            }
        }
    }
    let min = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let max = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    max - min
}

/// Each contour flattened to a closed polygon (the closing vertex is not
/// repeated).
pub fn polygons(paths: &[Path], tolerance: f64) -> Vec<Vec<Point>> {
    paths
        .iter()
        .map(|path| {
            let bez = to_bezpath(path).expect("valid contour");
            let mut points = Vec::new();
            kurbo::flatten(bez.elements().iter().copied(), tolerance, |el| {
                if let PathEl::MoveTo(p) | PathEl::LineTo(p) = el {
                    points.push(p);
                }
            });
            while points.len() > 1 && points.first() == points.last() {
                points.pop();
            }
            points
        })
        .collect()
}

/// Whether segments `ab` and `cd` cross at a point interior to both. Touching
/// (an end point on the other segment) and collinear overlap do not count.
fn properly_cross(a: Point, b: Point, c: Point, d: Point) -> bool {
    let eps = 1e-9 * (b - a).hypot() * (d - c).hypot();
    let side = |p: Point, q: Point, r: Point| {
        let o = (q - p).cross(r - p);
        if o > eps {
            1
        } else if o < -eps {
            -1
        } else {
            0
        }
    };
    let (d1, d2) = (side(c, d, a), side(c, d, b));
    let (d3, d4) = (side(a, b, c), side(a, b, d));
    d1 * d2 < 0 && d3 * d4 < 0
}

/// Edges whose end points come this close are touching within the tolerance
/// of the overlap removal (contours may touch), not crossing.
const TOUCH: f64 = 1e-3;

/// How many pairs of edges of the flattened contours cross each other, within
/// a contour (non-adjacent edges) or between contours. Edges that touch
/// within 0.001 units at their end points do not count.
pub fn crossing_count(paths: &[Path]) -> usize {
    struct Edge {
        contour: usize,
        index: usize,
        len: usize,
        a: Point,
        b: Point,
        bounds: Rect,
    }
    let mut edges = Vec::new();
    for (contour, polygon) in polygons(paths, 0.01).iter().enumerate() {
        let len = polygon.len();
        for index in 0..len {
            let a = polygon[index];
            let b = polygon[(index + 1) % len];
            edges.push(Edge {
                contour,
                index,
                len,
                a,
                b,
                bounds: Rect::from_points(a, b),
            });
        }
    }
    let mut count = 0;
    for i in 0..edges.len() {
        for j in i + 1..edges.len() {
            let (e, f) = (&edges[i], &edges[j]);
            if e.contour == f.contour {
                let gap = e.index.abs_diff(f.index);
                if gap <= 1 || gap == e.len - 1 {
                    continue;
                }
            }
            if e.bounds.x1 < f.bounds.x0
                || f.bounds.x1 < e.bounds.x0
                || e.bounds.y1 < f.bounds.y0
                || f.bounds.y1 < e.bounds.y0
            {
                continue;
            }
            let touching = [e.a, e.b]
                .iter()
                .any(|p| [f.a, f.b].iter().any(|q| p.distance(*q) < TOUCH));
            if !touching && properly_cross(e.a, e.b, f.a, f.b) {
                count += 1;
            }
        }
    }
    count
}

/// Checks the PostScript direction of a clean outline: every contour has the
/// filled area (winding 1) just to its left and nothing (winding 0) just to
/// its right, probed at the middle of its longest segment.
///
/// Unlike the nesting test of `check_paths`, this does not depend on where a
/// contour starts, so it is not fooled by contours that touch.
pub fn assert_filled_on_the_left(paths: &[Path]) {
    for (index, path) in paths.iter().enumerate() {
        let bez = to_bezpath(path).expect("valid contour");
        let segment = bez
            .segments()
            .max_by(|a, b| a.arclen(1e-6).total_cmp(&b.arclen(1e-6)))
            .expect("a contour with segments");
        let middle = segment.eval(0.5);
        let tangent = segment.eval(0.5 + 1e-4) - segment.eval(0.5 - 1e-4);
        let normal = tangent.normalize().turn_90() * 0.05;
        assert_eq!(
            (
                winding(paths, middle + normal),
                winding(paths, middle - normal)
            ),
            (1, 0),
            "contour {index} must have the filled side on its left: {path:?}"
        );
    }
}
