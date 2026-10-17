//! Signed area, nesting depth and contour direction.

use std::cmp::Ordering;

use forma_model::{Node, NodeKind, Path};
use kurbo::{ParamCurve, PathEl, PathSeg, Point, Rect};

use crate::convert::{outline, to_bezpath};
use crate::{FLATTEN_TOLERANCE, GeometryError};

/// The flattened polygon of a closed, valid contour; `None` for open or
/// unconvertible contours.
///
/// Each curve is flattened from whichever of its two ends gives the smaller
/// sequence of control points, so that a contour and its reversal have the
/// same polygon, in reverse order (flattening a curve backwards picks other
/// points). Reversing a contour to fix its direction must not move it: the
/// nesting test would then answer differently for contours that cross it.
fn flattened(path: &Path) -> Option<Vec<Point>> {
    if !path.closed {
        return None;
    }
    let bez = to_bezpath(path).ok()?;
    let mut points = Vec::new();
    for segment in bez.segments() {
        if points.is_empty() {
            points.push(segment.start());
        }
        if let PathSeg::Line(line) = segment {
            points.push(line.p1);
            continue;
        }
        let backwards = runs_backwards(&segment);
        let canonical = if backwards {
            segment.reverse()
        } else {
            segment
        };
        let mut piece = Vec::new();
        let elements = [PathEl::MoveTo(canonical.start()), canonical.as_path_el()];
        kurbo::flatten(elements, FLATTEN_TOLERANCE, |el| {
            if let PathEl::LineTo(p) = el {
                piece.push(p);
            }
        });
        if backwards {
            // From the segment's end back to its start: drop the start,
            // walk the rest the other way and end at the segment's end.
            piece.pop();
            piece.reverse();
            piece.push(segment.end());
        }
        points.extend(piece);
    }
    Some(points)
}

/// Whether a curve's control points, read from its end, come before the same
/// points read from its start (comparing x, then y): the curve is then
/// flattened from its end.
fn runs_backwards(segment: &PathSeg) -> bool {
    let (controls, count) = match *segment {
        PathSeg::Line(l) => ([l.p0, l.p1, Point::ZERO, Point::ZERO], 2),
        PathSeg::Quad(q) => ([q.p0, q.p1, q.p2, Point::ZERO], 3),
        PathSeg::Cubic(c) => ([c.p0, c.p1, c.p2, c.p3], 4),
    };
    let controls = &controls[..count];
    controls
        .iter()
        .zip(controls.iter().rev())
        .map(|(forward, backward)| {
            backward
                .x
                .total_cmp(&forward.x)
                .then(backward.y.total_cmp(&forward.y))
        })
        .find(|order| order.is_ne())
        .is_some_and(Ordering::is_lt)
}

/// Shoelace formula; positive for counter-clockwise polygons with y up.
fn polygon_area(points: &[Point]) -> f64 {
    let Some(&origin) = points.first() else {
        return 0.0;
    };
    // Measured from the first point to keep large offsets from costing
    // precision.
    let sum: f64 = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| {
            let (ax, ay) = (a.x - origin.x, a.y - origin.y);
            let (bx, by) = (b.x - origin.x, b.y - origin.y);
            ax * by - bx * ay
        })
        .sum();
    sum / 2.0
}

/// The signed area of a contour in font coordinates (y up): positive when the
/// contour runs counter-clockwise, negative when clockwise.
///
/// The area is that of the contour flattened to a polygon with a tolerance of
/// 0.25 font units, so curved contours are approximate; the sign is reliable
/// for any contour of meaningful size. Open contours, and contours that cannot
/// be converted (see [`to_bezpath`](crate::to_bezpath)), have area `0.0`.
#[must_use]
pub fn signed_area(path: &Path) -> f64 {
    flattened(path).map_or(0.0, |points| polygon_area(&points))
}

/// Even-odd ray test: whether `q` lies inside `polygon`.
///
/// Each edge is measured from its lower end, so that the answer does not
/// depend on which way the polygon runs.
fn contains(polygon: &[Point], q: Point) -> bool {
    let mut inside = false;
    for (a, b) in edges(polygon) {
        if (a.y > q.y) != (b.y > q.y) {
            let (a, b) = if a.y < b.y { (a, b) } else { (b, a) };
            let x_at_q = (b.x - a.x) * (q.y - a.y) / (b.y - a.y) + a.x;
            if q.x < x_at_q {
                inside = !inside;
            }
        }
    }
    inside
}

/// The edges of a closed polygon, the closing one included.
fn edges(polygon: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(&a, &b)| (a, b))
}

/// The distance from `p` to the segment `ab`, the same either way round.
fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (a, b) = if (a.x, a.y) <= (b.x, b.y) {
        (a, b)
    } else {
        (b, a)
    };
    let ab = b - a;
    let length2 = ab.hypot2();
    let t = if length2 > 0.0 {
        ((p - a).dot(ab) / length2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// The distance from `p` to the boundary of `polygon`.
fn boundary_distance(polygon: &[Point], p: Point) -> f64 {
    edges(polygon)
        .map(|(a, b)| segment_distance(p, a, b))
        .fold(f64::INFINITY, f64::min)
}

/// How many edges of a contour, the longest ones, propose a probe point (see
/// [`inside_points`]). It bounds the cost of a contour flattened into many
/// edges; one probe clear of the other contour is enough.
const PROBE_EDGES: usize = 32;

/// Points just inside a closed polygon, `offset` away from the middle of its
/// [`PROBE_EDGES`] longest edges (longest first) towards the interior: the
/// side given by the sign of the polygon's area. A point the even-odd test
/// does not put inside the polygon (on a lobe of a self-intersecting
/// contour) is left out.
///
/// The choice depends on the shape only, not on where the contour starts or
/// which way it runs: lengths are compared to within `offset`, and edges of
/// the same length are taken from left to right, then from bottom to top.
/// Reversing a contour (to fix its direction) must not change its depth.
fn inside_points(polygon: &[Point], offset: f64) -> Vec<Point> {
    let area = polygon_area(polygon);
    if !(area.is_finite() && area != 0.0 && offset > 0.0) {
        return Vec::new();
    }
    let mut longest: Vec<(f64, Point, Point)> = edges(polygon)
        .filter(|(a, b)| a != b)
        .map(|(a, b)| ((a.distance(b) / offset).round(), a, b))
        .collect();
    let middle = |a: Point, b: Point| a.midpoint(b);
    longest.sort_by(|(l1, a1, b1), (l2, a2, b2)| {
        let (m1, m2) = (middle(*a1, *b1), middle(*a2, *b2));
        l2.total_cmp(l1)
            .then(m1.x.total_cmp(&m2.x))
            .then(m1.y.total_cmp(&m2.y))
    });
    longest.truncate(PROBE_EDGES);
    longest
        .into_iter()
        .map(|(_, a, b)| {
            let inward = (b - a).turn_90() * (area.signum() / a.distance(b));
            a.midpoint(b) + inward * offset
        })
        .filter(|&p| contains(polygon, p))
        .collect()
}

/// The relative size, against the diagonal of a contour's bounding box,
/// under which a distance to the contour counts as zero: a point that close
/// lies on its boundary.
const BOUNDARY_TOLERANCE_RATIO: f64 = 1e-6;

/// The absolute distance under which a point lies on a contour's boundary.
const MIN_BOUNDARY_TOLERANCE: f64 = 1e-9;

/// A closed contour prepared for nesting tests.
struct Nesting {
    /// The contour flattened to within [`FLATTEN_TOLERANCE`].
    polygon: Vec<Point>,
    bounds: Rect,
    /// Closer than this to `polygon`, a point is on the contour's boundary.
    /// It is also how far inside the contour its probe points are.
    tolerance: f64,
    /// Points just inside the contour (see [`inside_points`]).
    probes: Vec<Point>,
    /// The first on-curve node: the probe of last resort.
    first: Point,
}

impl Nesting {
    /// `None` for open contours and contours that cannot be converted.
    fn new(path: &Path) -> Option<Self> {
        let polygon = flattened(path)?;
        let first = path
            .nodes
            .iter()
            .find(|n| n.kind != NodeKind::OffCurve)
            .map(|n| Point::new(n.x, n.y))?;
        let bounds = polygon
            .iter()
            .fold(Rect::from_points(first, first), |rect, &p| rect.union_pt(p));
        let tolerance = (BOUNDARY_TOLERANCE_RATIO * bounds.size().to_vec2().hypot())
            .max(MIN_BOUNDARY_TOLERANCE);
        let probes = inside_points(&polygon, tolerance);
        Some(Self {
            polygon,
            bounds,
            tolerance,
            probes,
            first,
        })
    }

    /// Whether this contour contains `other`, tested at one of `other`'s
    /// probe points.
    ///
    /// The probes lie a distance `d` (`other`'s tolerance) inside `other`. If
    /// the two contours do not cross, a probe farther than `d` from this
    /// contour's boundary gives the right answer: were this contour inside
    /// `other` (a counter touching its outer contour, say), the probe could
    /// only lie inside this one if this boundary passed between the probe and
    /// `other`'s boundary, within `d` of the probe. So the first probe
    /// farther than `d` plus the flattening tolerance (the test runs on the
    /// flattened contours; the curves may be that far from them) is used;
    /// failing that, the probe farthest from this boundary if it is clear of
    /// it at all; failing that (every probe is ambiguous), `other`'s first
    /// on-curve node.
    fn contains_contour(&self, other: &Nesting) -> bool {
        let clear = other.tolerance + self.tolerance;
        let safe = other.tolerance + FLATTEN_TOLERANCE.max(self.tolerance);
        let reach = self.bounds.inflate(safe, safe);
        let mut best: Option<(f64, Point)> = None;
        for &p in &other.probes {
            if !reach.contains(p) {
                return false; // far outside this contour
            }
            let distance = boundary_distance(&self.polygon, p);
            if distance > safe {
                return contains(&self.polygon, p);
            }
            if distance > clear && best.is_none_or(|(farthest, _)| distance > farthest) {
                best = Some((distance, p));
            }
        }
        contains(&self.polygon, best.map_or(other.first, |(_, p)| p))
    }
}

/// The nesting depth of each contour: how many *other* closed contours
/// contain it (even-odd rule on the flattened contours).
///
/// A contour is tested at a point just inside it (next to the middle of one
/// of its longest edges) that is clear of the boundary of the contour it is
/// tested against, so contours that touch each other, at a node or along an
/// edge, wherever they start, are nested correctly. Contours that cross each
/// other have no well-defined nesting; they get the answer of whichever
/// point is used. Only when every such point is ambiguous is the contour's
/// first on-curve node tested instead.
///
/// Open contours, and contours that cannot be converted, have depth `0` and
/// never contain anything.
#[must_use]
pub fn contour_depths(paths: &[Path]) -> Vec<usize> {
    let nestings: Vec<Option<Nesting>> = paths.iter().map(Nesting::new).collect();
    nestings
        .iter()
        .enumerate()
        .map(|(i, nesting)| {
            let Some(nesting) = nesting else {
                return 0;
            };
            nestings
                .iter()
                .enumerate()
                .filter(|&(j, other)| {
                    j != i
                        && other
                            .as_ref()
                            .is_some_and(|other| other.contains_contour(nesting))
                })
                .count()
        })
        .collect()
}

/// The nodes of `path` in reverse drawing order, describing exactly the same
/// shape.
///
/// A node's kind describes the segment arriving at it, so each kind moves to
/// the node the reversed segment now arrives at, and the off-curve nodes of
/// every segment are reversed with it. The first on-curve node stays first;
/// for a closed contour the off-curve nodes of the closing segment end up at
/// the end of the list.
pub(crate) fn reversed_nodes(path: &Path) -> Result<Vec<Node>, GeometryError> {
    let outline = outline(path)?;
    let segments = &outline.segments;
    let mut nodes = Vec::with_capacity(path.nodes.len());
    let reversed_controls = |index: usize| {
        segments[index]
            .controls
            .iter()
            .rev()
            .map(|&node| node.clone())
            .collect::<Vec<Node>>()
    };

    if outline.closed {
        // Segment `k` runs from on-curve node `k` to node `k + 1`; the last
        // one runs back to node 0. Walk them backwards from node 0.
        let mut start = outline.start.clone();
        start.kind = segments[0].kind;
        nodes.push(start);
        for k in (1..segments.len()).rev() {
            nodes.extend(reversed_controls(k));
            let mut node = segments[k - 1].end.clone();
            node.kind = segments[k].kind;
            nodes.push(node);
        }
        nodes.extend(reversed_controls(0));
    } else {
        // Segment `k` runs from on-curve node `k` to node `k + 1`; the new
        // first node is the old last one, a move-to.
        let mut first = segments
            .last()
            .map_or(outline.start, |segment| segment.end)
            .clone();
        first.kind = NodeKind::Line;
        nodes.push(first);
        for k in (0..segments.len()).rev() {
            nodes.extend(reversed_controls(k));
            let mut node = if k == 0 {
                outline.start.clone()
            } else {
                segments[k - 1].end.clone()
            };
            node.kind = segments[k].kind;
            nodes.push(node);
        }
    }
    Ok(nodes)
}

/// The fraction of its bounding box area under which a contour's signed
/// area is treated as noise: the contour has no direction.
const NOISE_AREA_RATIO: f64 = 1e-6;

/// The absolute area under which a contour has no direction.
const NOISE_AREA: f64 = 1e-9;

/// The signed area of `path` (see [`signed_area`]) when it is large enough to
/// give the contour a direction; `None` for a contour whose area is below
/// 1e-6 of its bounding box area, or below 1e-9 absolute (a degenerate sliver
/// or a self-overlapping contour whose lobes cancel), or not a number. The
/// sign of such an area is float noise: a reversal cannot be trusted to
/// change it, so reporting a direction for it would never be fixed.
#[must_use]
pub fn directed_area(path: &Path) -> Option<f64> {
    let area = signed_area(path);
    if area.is_nan() {
        return None;
    }
    let bbox = crate::bbox(std::slice::from_ref(path)).map_or(0.0, |rect| rect.area());
    let noise = (NOISE_AREA_RATIO * bbox).max(NOISE_AREA);
    (area.abs() >= noise).then_some(area)
}

/// Gives every closed contour its PostScript direction according to its
/// nesting depth (see [`contour_depths`]): even depth counter-clockwise
/// (positive area), odd depth clockwise.
///
/// Reversing a contour keeps its shape, its id and its smooth flags. Open
/// contours, contours that cannot be converted, and contours without a
/// direction (see [`directed_area`]) are left untouched.
pub fn normalize_direction(paths: &mut [Path]) {
    let depths = contour_depths(paths);
    for (path, depth) in paths.iter_mut().zip(depths) {
        let Some(area) = directed_area(path) else {
            continue;
        };
        let wants_counter_clockwise = depth % 2 == 0;
        if (area > 0.0) != wants_counter_clockwise
            && let Ok(nodes) = reversed_nodes(path)
        {
            path.nodes = nodes;
        }
    }
}

#[cfg(test)]
mod tests {
    use forma_model::PathId;

    use super::*;

    fn node(x: f64, y: f64, kind: NodeKind) -> Node {
        Node {
            x,
            y,
            kind,
            smooth: false,
        }
    }

    fn path(closed: bool, nodes: Vec<Node>) -> Path {
        Path {
            id: PathId(0),
            closed,
            nodes,
        }
    }

    #[test]
    fn reversing_twice_restores_a_canonical_closed_contour() {
        let original = path(
            true,
            vec![
                node(0.0, 0.0, NodeKind::Line),
                node(100.0, 0.0, NodeKind::Line),
                node(150.0, 20.0, NodeKind::OffCurve),
                node(150.0, 80.0, NodeKind::OffCurve),
                node(100.0, 100.0, NodeKind::Curve),
                node(50.0, 100.0, NodeKind::Line),
                node(0.0, 120.0, NodeKind::OffCurve),
                node(0.0, 50.0, NodeKind::QCurve),
            ],
        );
        let mut once = original.clone();
        once.nodes = reversed_nodes(&original).expect("valid");
        assert_ne!(once, original);
        let mut twice = once.clone();
        twice.nodes = reversed_nodes(&once).expect("valid");
        assert_eq!(twice, original);
    }

    #[test]
    fn reversing_a_closed_contour_with_a_closing_curve() {
        // The first node is a Curve; its handles are the last two nodes.
        let original = path(
            true,
            vec![
                node(0.0, 0.0, NodeKind::Curve),
                node(100.0, 0.0, NodeKind::Line),
                node(100.0, 60.0, NodeKind::OffCurve),
                node(60.0, 100.0, NodeKind::OffCurve),
            ],
        );
        // The cubic now arrives at (100, 0) and the closing segment is a line.
        let reversed = reversed_nodes(&original).expect("valid");
        assert_eq!(
            reversed,
            vec![
                node(0.0, 0.0, NodeKind::Line),
                node(60.0, 100.0, NodeKind::OffCurve),
                node(100.0, 60.0, NodeKind::OffCurve),
                node(100.0, 0.0, NodeKind::Curve),
            ]
        );
    }

    #[test]
    fn reversing_an_open_contour_moves_kinds_to_the_new_arrival_nodes() {
        let original = path(
            false,
            vec![
                node(0.0, 0.0, NodeKind::Line),
                node(2.0, 8.0, NodeKind::OffCurve),
                node(8.0, 8.0, NodeKind::OffCurve),
                node(10.0, 0.0, NodeKind::Curve),
                node(20.0, 0.0, NodeKind::Line),
            ],
        );
        let reversed = reversed_nodes(&original).expect("valid");
        assert_eq!(
            reversed,
            vec![
                node(20.0, 0.0, NodeKind::Line),
                node(10.0, 0.0, NodeKind::Line),
                node(8.0, 8.0, NodeKind::OffCurve),
                node(2.0, 8.0, NodeKind::OffCurve),
                node(0.0, 0.0, NodeKind::Curve),
            ]
        );
    }

    #[test]
    fn reversing_keeps_smooth_flags_on_their_nodes() {
        let mut original = path(
            true,
            vec![
                node(0.0, 0.0, NodeKind::Line),
                node(10.0, 0.0, NodeKind::Line),
                node(10.0, 10.0, NodeKind::Line),
            ],
        );
        original.nodes[1].smooth = true;
        let reversed = reversed_nodes(&original).expect("valid");
        let smooth: Vec<(f64, bool)> = reversed.iter().map(|n| (n.x, n.smooth)).collect();
        assert_eq!(smooth, vec![(0.0, false), (10.0, false), (10.0, true)]);
    }
}
