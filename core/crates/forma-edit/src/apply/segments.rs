//! Segment bookkeeping for contours in the UFO point convention.
//!
//! The on-curve nodes of a contour split it into segments. Segment `k` of a
//! closed contour ends at the `k`-th on-curve node (in `nodes` order) and
//! starts at the previous one, wrapping around, so segment 0 starts at the
//! last on-curve node. Segment `k` of an open contour runs from the `k`-th to
//! the `(k + 1)`-th on-curve node, because the first on-curve node is the
//! starting point. The off-curve nodes of a segment are the ones between its
//! two on-curve nodes, which for a closed contour may sit at both the end and
//! the start of the list.

use forma_geometry::to_bezpath;
use forma_model::{Node, NodeKind, Path};

use crate::EditError;

pub(super) fn is_on_curve(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Line | NodeKind::Curve | NodeKind::QCurve)
}

/// One segment, as indices into the node list.
pub(super) struct Span {
    /// The on-curve node the segment starts at.
    pub start: usize,
    /// The on-curve node the segment ends at.
    pub end: usize,
    /// The off-curve nodes in between, in drawing order.
    pub controls: Vec<usize>,
}

/// The on-curve structure of a well-formed contour.
pub(super) struct Layout {
    closed: bool,
    node_count: usize,
    /// Indices of the on-curve nodes, in node order.
    pub on_curve: Vec<usize>,
}

impl Layout {
    /// Analyses a contour, rejecting one that does not convert to a Bezier
    /// path (which also guarantees the shape of every segment).
    pub fn of(path: &Path) -> Result<Layout, EditError> {
        to_bezpath(path).map_err(|error| EditError::InvalidGeometry(error.to_string()))?;
        Ok(Layout {
            closed: path.closed,
            node_count: path.nodes.len(),
            on_curve: path
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, node)| is_on_curve(node.kind))
                .map(|(index, _)| index)
                .collect(),
        })
    }

    /// How many segments the contour has.
    pub fn segment_count(&self) -> usize {
        if self.closed {
            self.on_curve.len()
        } else {
            self.on_curve.len().saturating_sub(1)
        }
    }

    /// Segment `k`, or `None` if the contour has no such segment.
    pub fn segment(&self, k: usize) -> Option<Span> {
        let m = self.on_curve.len();
        let (start, end) = if self.closed {
            (
                *self.on_curve.get((k + m.checked_sub(1)?) % m)?,
                *self.on_curve.get(k)?,
            )
        } else {
            (*self.on_curve.get(k)?, *self.on_curve.get(k + 1)?)
        };
        let mut controls = Vec::new();
        let mut index = (start + 1) % self.node_count;
        while index != end {
            controls.push(index);
            index = (index + 1) % self.node_count;
        }
        Some(Span {
            start,
            end,
            controls,
        })
    }
}

/// Rebuilds a node list without the nodes in `removed`, putting `replacement`
/// where index `at` was (`at` may be the length of the list to append).
///
/// For a closed contour that wraps, a replacement placed at the end of the
/// list stands between the same two neighbours as the nodes it replaces.
pub(super) fn splice(
    nodes: &[Node],
    removed: &[usize],
    at: usize,
    replacement: Vec<Node>,
) -> Vec<Node> {
    let mut out = Vec::with_capacity(nodes.len() + replacement.len());
    let mut pending = Some(replacement);
    for (index, node) in nodes.iter().enumerate() {
        if index == at {
            out.extend(pending.take().into_iter().flatten());
        }
        if !removed.contains(&index) {
            out.push(node.clone());
        }
    }
    out.extend(pending.into_iter().flatten());
    out
}

/// The nodes of `path` in reverse drawing order, describing the same shape.
///
/// A node's kind describes the segment arriving at it, so each kind moves to
/// the node the reversed segment now arrives at, and the off-curve nodes of
/// every segment are reversed with it. The first on-curve node stays first
/// (for an open contour the old last node comes first); the off-curve nodes
/// of a closed contour's closing segment end up at the end of the list.
pub(super) fn reversed_nodes(path: &Path) -> Result<Vec<Node>, EditError> {
    let layout = Layout::of(path)?;
    let nodes = &path.nodes;
    let spans: Vec<Span> = (0..layout.segment_count())
        .filter_map(|k| layout.segment(k))
        .collect();
    let on_curve = &layout.on_curve;
    let kind_of = |k: usize| {
        spans
            .get(k)
            .map_or(NodeKind::Line, |span| nodes[span.end].kind)
    };
    let reversed_controls = |k: usize| {
        spans
            .get(k)
            .into_iter()
            .flat_map(|span| span.controls.iter().rev())
            .map(|&index| nodes[index].clone())
            .collect::<Vec<Node>>()
    };
    let with_kind = |index: usize, kind: NodeKind| Node {
        kind,
        ..nodes[index].clone()
    };

    let mut out = Vec::with_capacity(nodes.len());
    let m = on_curve.len();
    if path.closed {
        // The reversed contour walks the on-curve nodes 0, m-1, ..., 1; the
        // segment arriving at node `i` is the old segment `i + 1`, reversed.
        out.push(with_kind(on_curve[0], kind_of(1)));
        for i in (1..m).rev() {
            let k = (i + 1) % m;
            out.extend(reversed_controls(k));
            out.push(with_kind(on_curve[i], kind_of(k)));
        }
        out.extend(reversed_controls(1));
    } else {
        // The old last node becomes the move-to; the segment arriving at node
        // `i` is the old segment `i`, reversed.
        out.push(with_kind(on_curve[m - 1], NodeKind::Line));
        for k in (0..m - 1).rev() {
            out.extend(reversed_controls(k));
            out.push(with_kind(on_curve[k], kind_of(k)));
        }
    }
    Ok(out)
}
