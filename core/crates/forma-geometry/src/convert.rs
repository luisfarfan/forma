//! Conversion between model contours and kurbo Bezier paths.

use forma_model::{Node, NodeKind, Path, PathId};
use kurbo::{BezPath, ParamCurveExtrema, PathEl, Point, Rect};

use crate::GeometryError;

/// Two points closer than this on both axes count as the same point when an
/// imported contour closes onto its start.
const COINCIDENT_EPSILON: f64 = 1e-9;

/// One segment of a contour: the off-curve nodes it passes through and the
/// on-curve node it arrives at, whose kind says how the segment is drawn.
pub(crate) struct Segment<'a> {
    pub kind: NodeKind,
    pub controls: Vec<&'a Node>,
    pub end: &'a Node,
    /// Index of `end` in the node list of the contour.
    pub end_index: usize,
}

/// A validated contour split into its start point and its segments.
///
/// For a closed contour the last segment arrives back at `start`, and `start`
/// is the first on-curve node of the contour.
pub(crate) struct Outline<'a> {
    pub start: &'a Node,
    pub closed: bool,
    pub segments: Vec<Segment<'a>>,
}

pub(crate) fn point(node: &Node) -> Point {
    Point::new(node.x, node.y)
}

fn is_on_curve(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Line | NodeKind::Curve | NodeKind::QCurve)
}

/// Validates `path` and splits it into segments.
pub(crate) fn outline(path: &Path) -> Result<Outline<'_>, GeometryError> {
    let nodes = &path.nodes;
    if nodes.is_empty() {
        return Err(GeometryError::Degenerate("contour has no nodes".into()));
    }
    for (i, node) in nodes.iter().enumerate() {
        if !(node.x.is_finite() && node.y.is_finite()) {
            return Err(GeometryError::InvalidPath(format!(
                "node {i} has a non-finite coordinate"
            )));
        }
        if !is_on_curve(node.kind) && node.kind != NodeKind::OffCurve {
            return Err(GeometryError::InvalidPath(format!(
                "node {i} has an unsupported kind"
            )));
        }
    }

    if path.closed {
        let on_curve = nodes.iter().filter(|n| is_on_curve(n.kind)).count();
        if on_curve < 2 {
            return Err(GeometryError::Degenerate(format!(
                "closed contour needs at least 2 on-curve nodes, found {on_curve}"
            )));
        }
        // Rotate so that the walk starts at an on-curve node; off-curve nodes
        // in front of it lead into the closing segment.
        let first = nodes
            .iter()
            .position(|n| is_on_curve(n.kind))
            .unwrap_or_default();
        let order: Vec<usize> = (first..nodes.len()).chain(0..first).collect();
        let start = &nodes[order[0]];
        // Walk the rest of the contour, then the start node again to close it.
        let walk: Vec<usize> = order[1..].iter().copied().chain([order[0]]).collect();
        let segments = split_segments(nodes, &walk)?;
        Ok(Outline {
            start,
            closed: true,
            segments,
        })
    } else {
        let start = &nodes[0];
        if !is_on_curve(start.kind) {
            return Err(GeometryError::InvalidPath(
                "open contour must start with an on-curve node".into(),
            ));
        }
        let walk: Vec<usize> = (1..nodes.len()).collect();
        let segments = split_segments(nodes, &walk)?;
        Ok(Outline {
            start,
            closed: false,
            segments,
        })
    }
}

/// Groups the nodes visited by `walk` into segments, checking that every
/// on-curve node is preceded by the number of off-curve nodes its kind needs.
fn split_segments<'a>(
    nodes: &'a [Node],
    walk: &[usize],
) -> Result<Vec<Segment<'a>>, GeometryError> {
    let mut segments = Vec::new();
    let mut pending: Vec<&Node> = Vec::new();
    for &index in walk {
        let node = &nodes[index];
        if node.kind == NodeKind::OffCurve {
            pending.push(node);
            continue;
        }
        let found = pending.len();
        let problem = match node.kind {
            NodeKind::Line if found != 0 => Some(format!(
                "line node {index} is preceded by {found} off-curve node(s); \
                 a line segment takes none"
            )),
            NodeKind::Curve if found != 2 => Some(format!(
                "curve node {index} must be preceded by exactly 2 off-curve nodes, found {found}"
            )),
            NodeKind::QCurve if found != 1 => Some(format!(
                "qcurve node {index} must be preceded by exactly 1 off-curve node, found {found} \
                 (implied on-curve points are not supported)"
            )),
            _ => None,
        };
        if let Some(message) = problem {
            return Err(GeometryError::InvalidPath(message));
        }
        segments.push(Segment {
            kind: node.kind,
            controls: std::mem::take(&mut pending),
            end: node,
            end_index: index,
        });
    }
    if !pending.is_empty() {
        return Err(GeometryError::InvalidPath(format!(
            "{} off-curve node(s) are not followed by an on-curve node",
            pending.len()
        )));
    }
    Ok(segments)
}

/// Converts a contour to a kurbo [`BezPath`].
///
/// The contour follows the UFO convention described in the crate docs: a cubic
/// segment is two `OffCurve` nodes followed by a `Curve`, a quadratic segment
/// is one `OffCurve` followed by a `QCurve`, and a `Line` has no off-curve
/// nodes before it. A closed contour may start with off-curve nodes; the
/// resulting path starts at the first on-curve node.
///
/// # Errors
///
/// * [`GeometryError::Degenerate`] if the contour has no nodes, or is closed
///   with fewer than two on-curve nodes.
/// * [`GeometryError::InvalidPath`] if a coordinate is not finite, an
///   off-curve sequence is not terminated by an on-curve node, a segment has
///   the wrong number of off-curve nodes, or an open contour does not start
///   with an on-curve node.
pub fn to_bezpath(path: &Path) -> Result<BezPath, GeometryError> {
    let outline = outline(path)?;
    let mut bez = BezPath::new();
    bez.move_to(point(outline.start));
    let last = outline.segments.len().saturating_sub(1);
    for (i, segment) in outline.segments.iter().enumerate() {
        let end = point(segment.end);
        match (segment.kind, segment.controls.as_slice()) {
            // The closing line of a closed contour is drawn by `ClosePath`.
            (NodeKind::Line, []) => {
                if !(outline.closed && i == last) {
                    bez.line_to(end);
                }
            }
            (NodeKind::QCurve, [c]) => bez.quad_to(point(c), end),
            (NodeKind::Curve, [c1, c2]) => bez.curve_to(point(c1), point(c2), end),
            _ => {
                return Err(GeometryError::InvalidPath(
                    "segment does not match the contour convention".into(),
                ));
            }
        }
    }
    if outline.closed {
        bez.close_path();
    }
    Ok(bez)
}

fn model_node(p: Point, kind: NodeKind) -> Node {
    Node {
        x: p.x,
        y: p.y,
        kind,
        smooth: false,
    }
}

fn coincident(a: &Node, b: &Node) -> bool {
    (a.x - b.x).abs() <= COINCIDENT_EPSILON && (a.y - b.y).abs() <= COINCIDENT_EPSILON
}

fn missing_move_to() -> GeometryError {
    GeometryError::InvalidPath("drawing command without a preceding move-to".into())
}

/// Turns the nodes collected for one subpath into a [`Path`].
fn finish_subpath(id: PathId, mut nodes: Vec<Node>, closed: bool) -> Result<Path, GeometryError> {
    if closed && nodes.len() >= 2 {
        // The closing segment already ends on the start point: its kind moves
        // to the start node and the duplicate node is dropped.
        let ends_on_start = nodes
            .first()
            .zip(nodes.last())
            .is_some_and(|(first, last)| coincident(first, last));
        if ends_on_start
            && let Some(last) = nodes.pop()
            && let Some(first) = nodes.first_mut()
        {
            first.kind = last.kind;
        }
    }
    if closed {
        let on_curve = nodes.iter().filter(|n| is_on_curve(n.kind)).count();
        if on_curve < 2 {
            return Err(GeometryError::Degenerate(format!(
                "closed contour needs at least 2 on-curve nodes, found {on_curve}"
            )));
        }
    }
    Ok(Path { id, closed, nodes })
}

/// Converts a kurbo [`BezPath`] to model contours.
///
/// Every `MoveTo` starts a new contour and `ClosePath` closes it. Ids are
/// assigned sequentially from `first_id`. When the closing segment ends on the
/// start point, no duplicate node is created: the start node takes the kind of
/// that segment and its off-curve nodes stay at the end of the list. No node is
/// marked smooth.
///
/// # Errors
///
/// * [`GeometryError::InvalidPath`] if a coordinate is not finite, a drawing
///   command appears before any `MoveTo`, or a `ClosePath` has nothing to
///   close.
/// * [`GeometryError::Degenerate`] if a closed contour has fewer than two
///   on-curve nodes.
pub fn from_bezpath(bez: &BezPath, first_id: u32) -> Result<Vec<Path>, GeometryError> {
    if bez.elements().iter().any(|el| !el.is_finite()) {
        return Err(GeometryError::InvalidPath(
            "path has a non-finite coordinate".into(),
        ));
    }

    let mut paths: Vec<Path> = Vec::new();
    let mut current: Option<Vec<Node>> = None;

    let next_id = |paths: &[Path]| -> Result<PathId, GeometryError> {
        u32::try_from(paths.len())
            .ok()
            .and_then(|n| first_id.checked_add(n))
            .map(PathId)
            .ok_or_else(|| GeometryError::InvalidPath("path ids overflow".into()))
    };
    for el in bez.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                if let Some(nodes) = current.take() {
                    let id = next_id(&paths)?;
                    paths.push(finish_subpath(id, nodes, false)?);
                }
                current = Some(vec![model_node(p, NodeKind::Line)]);
            }
            PathEl::LineTo(p) => {
                let nodes = current.as_mut().ok_or_else(missing_move_to)?;
                nodes.push(model_node(p, NodeKind::Line));
            }
            PathEl::QuadTo(c, p) => {
                let nodes = current.as_mut().ok_or_else(missing_move_to)?;
                nodes.push(model_node(c, NodeKind::OffCurve));
                nodes.push(model_node(p, NodeKind::QCurve));
            }
            PathEl::CurveTo(c1, c2, p) => {
                let nodes = current.as_mut().ok_or_else(missing_move_to)?;
                nodes.push(model_node(c1, NodeKind::OffCurve));
                nodes.push(model_node(c2, NodeKind::OffCurve));
                nodes.push(model_node(p, NodeKind::Curve));
            }
            PathEl::ClosePath => {
                let nodes = current.take().ok_or_else(|| {
                    GeometryError::InvalidPath("close-path without an open subpath".into())
                })?;
                let id = next_id(&paths)?;
                paths.push(finish_subpath(id, nodes, true)?);
            }
        }
    }
    if let Some(nodes) = current.take() {
        let id = next_id(&paths)?;
        paths.push(finish_subpath(id, nodes, false)?);
    }
    Ok(paths)
}

fn bezpath_bbox(bez: &BezPath) -> Option<Rect> {
    let mut acc: Option<Rect> = None;
    let mut add = |r: Rect| acc = Some(acc.map_or(r, |a| a.union(r)));
    // A subpath without segments (a lone move-to) still occupies its point.
    for el in bez.elements() {
        if let PathEl::MoveTo(p) = *el {
            add(Rect::from_points(p, p));
        }
    }
    for segment in bez.segments() {
        add(segment.bounding_box());
    }
    acc
}

/// The exact bounding box of the contours, or `None` if there is nothing to
/// measure.
///
/// Curve extrema are taken into account, so off-curve nodes do not inflate the
/// box. Contours that cannot be converted (see [`to_bezpath`]) are ignored.
#[must_use]
pub fn bbox(paths: &[Path]) -> Option<Rect> {
    paths
        .iter()
        .filter_map(|path| to_bezpath(path).ok())
        .filter_map(|bez| bezpath_bbox(&bez))
        .reduce(|a, b| a.union(b))
}
