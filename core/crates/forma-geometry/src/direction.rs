//! Signed area, nesting depth and contour direction.

use forma_model::{Node, NodeKind, Path};
use kurbo::{PathEl, Point};

use crate::convert::{outline, to_bezpath};
use crate::{FLATTEN_TOLERANCE, GeometryError};

/// The flattened polygon of a closed, valid contour; `None` for open or
/// unconvertible contours.
fn flattened(path: &Path) -> Option<Vec<Point>> {
    if !path.closed {
        return None;
    }
    let bez = to_bezpath(path).ok()?;
    let mut points = Vec::new();
    kurbo::flatten(bez.elements().iter().copied(), FLATTEN_TOLERANCE, |el| {
        if let PathEl::MoveTo(p) | PathEl::LineTo(p) = el {
            points.push(p);
        }
    });
    Some(points)
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
fn contains(polygon: &[Point], q: Point) -> bool {
    let mut inside = false;
    for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
        if (a.y > q.y) != (b.y > q.y) {
            let x_at_q = (b.x - a.x) * (q.y - a.y) / (b.y - a.y) + a.x;
            if q.x < x_at_q {
                inside = !inside;
            }
        }
    }
    inside
}

/// The nesting depth of each contour: how many *other* closed contours contain
/// its first on-curve node (even-odd rule on the flattened contours).
///
/// Open contours, and contours that cannot be converted, have depth `0` and
/// never contain anything.
#[must_use]
pub fn contour_depths(paths: &[Path]) -> Vec<usize> {
    let polygons: Vec<Option<Vec<Point>>> = paths.iter().map(flattened).collect();
    paths
        .iter()
        .enumerate()
        .map(|(i, path)| {
            if polygons[i].is_none() {
                return 0;
            }
            let Some(probe) = path
                .nodes
                .iter()
                .find(|n| n.kind != NodeKind::OffCurve)
                .map(|n| Point::new(n.x, n.y))
            else {
                return 0;
            };
            polygons
                .iter()
                .enumerate()
                .filter(|&(j, polygon)| {
                    j != i
                        && polygon
                            .as_deref()
                            .is_some_and(|polygon| contains(polygon, probe))
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
