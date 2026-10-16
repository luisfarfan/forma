//! Node-level edits of one contour.

use std::collections::BTreeSet;

use forma_model::{EntityKey, Font, MasterId, Node, NodeKind, Path, PathId};
use kurbo::{CubicBez, ParamCurve, Point, QuadBez};

use super::segments::{Layout, Span, is_on_curve, splice};
use super::{Touched, check_path, layer_mut, resolve_glyph, resolve_master};
use crate::{EditError, GlyphRef, NodeAction};

pub(super) fn nodes_edit(
    font: &mut Font,
    glyph: &GlyphRef,
    master: Option<MasterId>,
    path_id: PathId,
    action: &NodeAction,
) -> Result<Touched, EditError> {
    let id = resolve_glyph(font, glyph)?;
    let master = resolve_master(font, master)?;
    let layer = layer_mut(font, id, master)?;
    let path = layer
        .paths
        .iter_mut()
        .find(|path| path.id == path_id)
        .ok_or_else(|| EditError::NotFound(format!("contour {} of glyph '{glyph}'", path_id.0)))?;

    let nodes = match *action {
        NodeAction::Move {
            ref indices,
            dx,
            dy,
        } => move_nodes(path, indices, dx, dy)?,
        NodeAction::Insert { segment_index, t } => insert_node(path, segment_index, t)?,
        NodeAction::Delete { index } => delete_node(path, index)?,
        NodeAction::SetSmooth { index, smooth } => set_smooth(path, index, smooth)?,
    };
    let edited = Path {
        id: path.id,
        closed: path.closed,
        nodes,
    };
    check_path(&edited)?;
    *path = edited;
    Ok(BTreeSet::from([EntityKey::Glyph(id)]))
}

fn out_of_range(index: usize, len: usize) -> EditError {
    EditError::InvalidArgument(format!(
        "node index {index} is out of range: the contour has {len} nodes"
    ))
}

fn move_nodes(path: &Path, indices: &[usize], dx: f64, dy: f64) -> Result<Vec<Node>, EditError> {
    if indices.is_empty() {
        return Err(EditError::InvalidArgument("no node indices to move".into()));
    }
    if !(dx.is_finite() && dy.is_finite()) {
        return Err(EditError::InvalidArgument(format!(
            "the offset must be finite, got ({dx}, {dy})"
        )));
    }
    let mut nodes = path.nodes.clone();
    let targets: BTreeSet<usize> = indices.iter().copied().collect();
    for &index in &targets {
        let node = nodes
            .get_mut(index)
            .ok_or_else(|| out_of_range(index, path.nodes.len()))?;
        node.x += dx;
        node.y += dy;
    }
    Ok(nodes)
}

fn set_smooth(path: &Path, index: usize, smooth: bool) -> Result<Vec<Node>, EditError> {
    let mut nodes = path.nodes.clone();
    let node = nodes
        .get_mut(index)
        .ok_or_else(|| out_of_range(index, path.nodes.len()))?;
    if !is_on_curve(node.kind) {
        return Err(EditError::InvalidArgument(format!(
            "node {index} is an off-curve node; only on-curve nodes can be smooth"
        )));
    }
    node.smooth = smooth;
    Ok(nodes)
}

fn point(node: &Node) -> Point {
    Point::new(node.x, node.y)
}

fn node_at(point: Point, kind: NodeKind, smooth: bool) -> Node {
    Node {
        x: point.x,
        y: point.y,
        kind,
        smooth,
    }
}

fn malformed() -> EditError {
    EditError::InvalidGeometry("segment does not match the contour convention".into())
}

/// Splits segment `segment_index` at `t` by De Casteljau subdivision.
fn insert_node(path: &Path, segment_index: usize, t: f64) -> Result<Vec<Node>, EditError> {
    if !(t > 0.0 && t < 1.0) {
        return Err(EditError::InvalidArgument(format!(
            "t must be strictly between 0 and 1, got {t}"
        )));
    }
    let layout = Layout::of(path)?;
    let span = layout.segment(segment_index).ok_or_else(|| {
        EditError::InvalidArgument(format!(
            "segment index {segment_index} is out of range: the contour has {} segments",
            layout.segment_count()
        ))
    })?;
    let nodes = &path.nodes;
    let start = point(&nodes[span.start]);
    let end = &nodes[span.end];
    let controls: Vec<Point> = span.controls.iter().map(|&i| point(&nodes[i])).collect();

    let replacement = match (end.kind, controls.as_slice()) {
        (NodeKind::Line, []) => vec![node_at(start.lerp(point(end), t), NodeKind::Line, false)],
        (NodeKind::QCurve, &[control]) => {
            let quad = QuadBez::new(start, control, point(end));
            let (before, after) = (quad.subsegment(0.0..t), quad.subsegment(t..1.0));
            vec![
                node_at(before.p1, NodeKind::OffCurve, false),
                node_at(before.p2, NodeKind::QCurve, true),
                node_at(after.p1, NodeKind::OffCurve, false),
            ]
        }
        (NodeKind::Curve, &[first, second]) => {
            let cubic = CubicBez::new(start, first, second, point(end));
            let (before, after) = (cubic.subsegment(0.0..t), cubic.subsegment(t..1.0));
            vec![
                node_at(before.p1, NodeKind::OffCurve, false),
                node_at(before.p2, NodeKind::OffCurve, false),
                node_at(before.p3, NodeKind::Curve, true),
                node_at(after.p1, NodeKind::OffCurve, false),
                node_at(after.p2, NodeKind::OffCurve, false),
            ]
        }
        _ => return Err(malformed()),
    };
    // The new nodes take the place of the segment's handles, right after its
    // start node.
    Ok(splice(nodes, &span.controls, span.start + 1, replacement))
}

/// Deletes an on-curve node, merging the segments on either side.
fn delete_node(path: &Path, index: usize) -> Result<Vec<Node>, EditError> {
    let layout = Layout::of(path)?;
    let nodes = &path.nodes;
    let node = nodes
        .get(index)
        .ok_or_else(|| out_of_range(index, nodes.len()))?;
    let position = layout
        .on_curve
        .iter()
        .position(|&i| i == index)
        .filter(|_| is_on_curve(node.kind))
        .ok_or_else(|| {
            EditError::InvalidArgument(format!(
                "node {index} is an off-curve node; only on-curve nodes can be deleted"
            ))
        })?;
    let count = layout.on_curve.len();

    if path.closed {
        if count < 3 {
            return Err(EditError::InvalidGeometry(
                "a closed contour needs at least 2 on-curve nodes".into(),
            ));
        }
        let incoming = layout.segment(position).ok_or_else(malformed)?;
        let outgoing = layout
            .segment((position + 1) % count)
            .ok_or_else(malformed)?;
        return merge(nodes, index, &incoming, &outgoing);
    }

    if count == 1 {
        // Nothing is left; the caller rejects the empty contour.
        return Ok(Vec::new());
    }
    if position == 0 {
        // The next on-curve node becomes the starting point.
        let outgoing = layout.segment(0).ok_or_else(malformed)?;
        let mut kept = nodes.clone();
        kept[outgoing.end].kind = NodeKind::Line;
        let removed: Vec<usize> = std::iter::once(index).chain(outgoing.controls).collect();
        Ok(splice(&kept, &removed, kept.len(), Vec::new()))
    } else if position == count - 1 {
        let incoming = layout.segment(position - 1).ok_or_else(malformed)?;
        let removed: Vec<usize> = std::iter::once(index).chain(incoming.controls).collect();
        Ok(splice(nodes, &removed, nodes.len(), Vec::new()))
    } else {
        let incoming = layout.segment(position - 1).ok_or_else(malformed)?;
        let outgoing = layout.segment(position).ok_or_else(malformed)?;
        merge(nodes, index, &incoming, &outgoing)
    }
}

/// Removes the node `deleted` between two segments and draws one segment from
/// the start of the incoming one to the end of the outgoing one: a cubic with
/// the incoming first handle and the outgoing last handle when both were
/// cubic, otherwise a line.
fn merge(
    nodes: &[Node],
    deleted: usize,
    incoming: &Span,
    outgoing: &Span,
) -> Result<Vec<Node>, EditError> {
    let mut kept = nodes.to_vec();
    let both_cubic =
        nodes[deleted].kind == NodeKind::Curve && nodes[outgoing.end].kind == NodeKind::Curve;
    let handles = if both_cubic {
        let (Some(&first), Some(&last)) = (incoming.controls.first(), outgoing.controls.get(1))
        else {
            return Err(malformed());
        };
        vec![nodes[first].clone(), nodes[last].clone()]
    } else {
        kept[outgoing.end].kind = NodeKind::Line;
        Vec::new()
    };
    let removed: Vec<usize> = incoming
        .controls
        .iter()
        .copied()
        .chain([deleted])
        .chain(outgoing.controls.iter().copied())
        .collect();
    Ok(splice(&kept, &removed, incoming.start + 1, handles))
}
