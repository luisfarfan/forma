//! Geometry checks and safe automatic fixes.
//!
//! [`check_paths`] reports problems in outlines; [`fix_paths`] repairs the
//! ones that have a safe, shape-preserving repair. Both are built on the same
//! detection code, so a fix only ever touches what a check reports.

use std::collections::BTreeMap;

use forma_model::{Node, NodeKind, Path, PathId};
use kurbo::{
    CubicBez, Line, ParamCurve, ParamCurveArclen, ParamCurveExtrema, PathSeg, Point, QuadBez,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::convert::{Outline, Segment, outline, point};
use crate::direction::{contour_depths, reversed_nodes, signed_area};

/// Segments shorter than this, in font units, are reported as tiny.
const TINY_SEGMENT_LENGTH: f64 = 1.0;

/// Accuracy, in font units, of the arc length used for the tiny-segment test.
const ARC_LENGTH_ACCURACY: f64 = 1e-6;

/// How far a coordinate may be from a multiple of the grid and still count as
/// on the grid.
const GRID_TOLERANCE: f64 = 1e-9;

/// Extrema at curve parameters outside `(MIN, MAX)` are too close to a segment
/// end to be worth a node of their own.
const EXTREMUM_T_MIN: f64 = 0.01;
const EXTREMUM_T_MAX: f64 = 0.99;

/// How far, in font units, a curve may stick out past the range spanned by its
/// end points before the extremum counts as missing.
const EXTREMUM_OVERSHOOT: f64 = 0.5;

/// Two extremum parameters closer than this are the same split point.
const SPLIT_T_EPSILON: f64 = 1e-9;

/// Splitting a segment can expose extrema that were within the allowance as
/// part of the whole segment; this many rounds always suffice (a cubic has at
/// most four interior extrema), the cap only guards against numeric surprises.
const MAX_SPLIT_ROUNDS: usize = 8;

/// The kind of problem found in a contour.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum IssueKind {
    /// The contour is not closed. Reported only; never fixed.
    OpenContour,
    /// A closed contour runs the wrong way for its nesting depth: even depth
    /// must be counter-clockwise, odd depth clockwise. Fixed by reversing it.
    WrongDirection,
    /// Two consecutive on-curve nodes (cyclically, in a closed contour) sit at
    /// the same position and the segment between them has no extent. Fixed by
    /// removing the redundant node and the off-curve nodes of that segment.
    DuplicatePoints,
    /// A segment is shorter than one font unit but not empty. Reported only.
    TinySegment,
    /// A coordinate is not a multiple of the grid. One issue per contour. Fixed
    /// by rounding every coordinate of the contour to the grid.
    OffGrid,
    /// A curve has an extreme point in x or y that sticks out past its end
    /// points but has no on-curve node. Fixed by splitting the curve there.
    MissingExtrema,
    /// The contour is malformed and cannot be converted to a Bezier path. The
    /// other checks skip it. Reported only.
    InvalidContour,
}

/// One problem found by [`check_paths`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GeometryIssue {
    /// The kind of problem.
    pub kind: IssueKind,
    /// The contour it was found in.
    pub path: PathId,
    /// Index of the offending node within the contour's `nodes`, when the
    /// problem is local to one; `None` when it concerns the whole contour.
    pub node: Option<usize>,
    /// A human-readable description.
    pub message: String,
    /// Whether [`fix_paths`] can repair this particular issue.
    pub fixable: bool,
}

/// How many times [`fix_paths`] repeats its pass at most. Every pass removes
/// nodes or rounds coordinates, so in practice two or three suffice.
const MAX_FIX_PASSES: usize = 6;

/// The order in which [`fix_paths`] applies the fixes.
const FIX_ORDER: [IssueKind; 4] = [
    IssueKind::DuplicatePoints,
    IssueKind::MissingExtrema,
    IssueKind::OffGrid,
    IssueKind::WrongDirection,
];

/// Every kind of issue, for running all the checks.
const ALL_KINDS: [IssueKind; 7] = [
    IssueKind::OpenContour,
    IssueKind::WrongDirection,
    IssueKind::DuplicatePoints,
    IssueKind::TinySegment,
    IssueKind::OffGrid,
    IssueKind::MissingExtrema,
    IssueKind::InvalidContour,
];

// ------------------------------------------------------------- detection

/// Checks contours for common outline problems.
///
/// Issues come in contour order and, within a contour, in the order of
/// [`IssueKind`]. A contour that cannot be converted to a Bezier path (see
/// [`to_bezpath`](crate::to_bezpath)) yields a single
/// [`IssueKind::InvalidContour`] issue and is skipped by every other check.
///
/// `grid` is the grid, in font units, that coordinates should sit on; the
/// [`IssueKind::OffGrid`] check only runs when it is positive.
#[must_use]
pub fn check_paths(paths: &[Path], grid: f64) -> Vec<GeometryIssue> {
    collect(paths, grid, &ALL_KINDS)
        .into_iter()
        .map(|(_, issue)| issue)
        .collect()
}

/// Runs the checks for `kinds` and pairs every issue with the index of its
/// contour in `paths`.
fn collect(paths: &[Path], grid: f64, kinds: &[IssueKind]) -> Vec<(usize, GeometryIssue)> {
    let wants = |kind: IssueKind| kinds.contains(&kind);
    let depths = if wants(IssueKind::WrongDirection) {
        contour_depths(paths)
    } else {
        Vec::new()
    };
    let mut found = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let depth = depths.get(index).copied().unwrap_or(0);
        for issue in check_path(path, depth, grid, wants) {
            found.push((index, issue));
        }
    }
    found
}

fn check_path(
    path: &Path,
    depth: usize,
    grid: f64,
    wants: impl Fn(IssueKind) -> bool,
) -> Vec<GeometryIssue> {
    let mut issues = Vec::new();
    let mut report = |kind: IssueKind, node: Option<usize>, message: String, fixable: bool| {
        issues.push(GeometryIssue {
            kind,
            path: path.id,
            node,
            message,
            fixable,
        });
    };

    let outline = match outline(path) {
        Ok(outline) => outline,
        Err(error) => {
            if wants(IssueKind::InvalidContour) {
                report(IssueKind::InvalidContour, None, error.to_string(), false);
            }
            return issues;
        }
    };

    if wants(IssueKind::OpenContour) && !path.closed {
        report(
            IssueKind::OpenContour,
            None,
            "contour is open".into(),
            false,
        );
    }

    if wants(IssueKind::WrongDirection)
        && path.closed
        && let Some(message) = direction_problem(path, depth)
    {
        report(IssueKind::WrongDirection, None, message, true);
    }

    if wants(IssueKind::DuplicatePoints) {
        for (k, fixable) in duplicates(&outline) {
            let end = outline.segments[k].end_index;
            let mut message =
                format!("on-curve node {end} repeats the position of the previous on-curve node");
            if !fixable {
                message.push_str("; removing it would leave fewer than 2 on-curve nodes");
            }
            report(IssueKind::DuplicatePoints, Some(end), message, fixable);
        }
    }

    if wants(IssueKind::TinySegment) || wants(IssueKind::MissingExtrema) {
        for (k, segment) in outline.segments.iter().enumerate() {
            let start = start_point(&outline, k);
            if is_zero_length(start, segment) {
                continue; // reported as a duplicate, not as tiny
            }
            let Some(geometry) = segment_geometry(start, segment) else {
                continue;
            };
            let end = segment.end_index;
            if wants(IssueKind::TinySegment) {
                let length = geometry.arclen(ARC_LENGTH_ACCURACY);
                if length > 0.0 && length < TINY_SEGMENT_LENGTH {
                    report(
                        IssueKind::TinySegment,
                        Some(end),
                        format!(
                            "segment arriving at node {end} is only {length:.3} units long \
                             (minimum {TINY_SEGMENT_LENGTH})"
                        ),
                        false,
                    );
                }
            }
            if wants(IssueKind::MissingExtrema) {
                let missing = overshooting_extrema(&geometry).len();
                if missing > 0 {
                    report(
                        IssueKind::MissingExtrema,
                        Some(end),
                        format!(
                            "segment arriving at node {end} has {missing} extreme point(s) \
                             without an on-curve node"
                        ),
                        true,
                    );
                }
            }
        }
    }

    if wants(IssueKind::OffGrid)
        && grid > 0.0
        && grid.is_finite()
        && let Some((first, count)) = off_grid_nodes(path, grid)
    {
        report(
            IssueKind::OffGrid,
            Some(first),
            format!(
                "{count} node(s) have coordinates off the grid of {grid}; the first is node {first}"
            ),
            true,
        );
    }

    // Segment loops interleave kinds; report them grouped, nodes in order.
    issues.sort_by_key(|issue| issue.kind);
    issues
}

/// Describes the direction problem of a closed contour, if it has one.
///
/// Contours without area have no direction and are never reported.
fn direction_problem(path: &Path, depth: usize) -> Option<String> {
    let area = signed_area(path);
    if area == 0.0 || area.is_nan() {
        return None;
    }
    let wants_counter_clockwise = depth.is_multiple_of(2);
    if (area > 0.0) == wants_counter_clockwise {
        return None;
    }
    let direction = |counter_clockwise: bool| {
        if counter_clockwise {
            "counter-clockwise"
        } else {
            "clockwise"
        }
    };
    Some(format!(
        "contour at nesting depth {depth} runs {} but must run {}",
        direction(!wants_counter_clockwise),
        direction(wants_counter_clockwise),
    ))
}

/// The start point of segment `k` of `outline`.
fn start_point(outline: &Outline, k: usize) -> Point {
    let node = match k.checked_sub(1) {
        Some(previous) => outline.segments[previous].end,
        None => outline.start,
    };
    point(node)
}

/// The Bezier segment from `start` through the controls of `segment`.
fn segment_geometry(start: Point, segment: &Segment) -> Option<PathSeg> {
    let end = point(segment.end);
    match (segment.kind, segment.controls.as_slice()) {
        (NodeKind::Line, []) => Some(PathSeg::Line(Line::new(start, end))),
        (NodeKind::QCurve, [c]) => Some(PathSeg::Quad(QuadBez::new(start, point(c), end))),
        (NodeKind::Curve, [c1, c2]) => Some(PathSeg::Cubic(CubicBez::new(
            start,
            point(c1),
            point(c2),
            end,
        ))),
        _ => None,
    }
}

/// Whether a segment is a single point: its end and all its off-curve nodes
/// sit exactly on its start.
fn is_zero_length(start: Point, segment: &Segment) -> bool {
    point(segment.end) == start && segment.controls.iter().all(|&c| point(c) == start)
}

/// The zero-length segments of a contour, by segment index, with whether
/// removing each is allowed.
///
/// A closed contour must keep at least two on-curve nodes: once only two are
/// left, the remaining duplicates are reported but cannot be removed.
fn duplicates(outline: &Outline) -> Vec<(usize, bool)> {
    let mut remaining = outline.segments.len() + usize::from(!outline.closed);
    let mut found = Vec::new();
    for (k, segment) in outline.segments.iter().enumerate() {
        if !is_zero_length(start_point(outline, k), segment) {
            continue;
        }
        let removable = !outline.closed || remaining > 2;
        if removable {
            remaining = remaining.saturating_sub(1);
        }
        found.push((k, removable));
    }
    found
}

fn on_grid(value: f64, grid: f64) -> bool {
    ((value / grid).round() * grid - value).abs() <= GRID_TOLERANCE
}

/// Rounds `value` to the nearest multiple of `grid`, never to `-0.0`.
fn snap(value: f64, grid: f64) -> f64 {
    let snapped = (value / grid).round() * grid;
    if snapped == 0.0 { 0.0 } else { snapped }
}

/// The first node with a coordinate off the grid and how many nodes there are.
fn off_grid_nodes(path: &Path, grid: f64) -> Option<(usize, usize)> {
    let mut off = path
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| !(on_grid(n.x, grid) && on_grid(n.y, grid)))
        .map(|(i, _)| i);
    let first = off.next()?;
    Some((first, 1 + off.count()))
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

impl Axis {
    fn of(self, p: Point) -> f64 {
        match self {
            Axis::X => p.x,
            Axis::Y => p.y,
        }
    }

    /// `p` with the other coordinate zeroed.
    fn project(self, p: Point) -> Point {
        match self {
            Axis::X => Point::new(p.x, 0.0),
            Axis::Y => Point::new(0.0, p.y),
        }
    }
}

/// The curve parameters, ascending, at which `segment` has an x or y extremum
/// that is missing a node: strictly inside `(0.01, 0.99)` and sticking out
/// past the range spanned by the segment's end points by more than 0.5 units.
fn overshooting_extrema(segment: &PathSeg) -> Vec<f64> {
    let mut params = Vec::new();
    for axis in [Axis::X, Axis::Y] {
        // The extrema of a curve whose other coordinate is flattened to zero
        // are exactly the extrema along this axis.
        let projected = match *segment {
            PathSeg::Line(_) => continue,
            PathSeg::Quad(q) => PathSeg::Quad(QuadBez::new(
                axis.project(q.p0),
                axis.project(q.p1),
                axis.project(q.p2),
            )),
            PathSeg::Cubic(c) => PathSeg::Cubic(CubicBez::new(
                axis.project(c.p0),
                axis.project(c.p1),
                axis.project(c.p2),
                axis.project(c.p3),
            )),
        };
        let (a, b) = (axis.of(segment.start()), axis.of(segment.end()));
        let (low, high) = (a.min(b), a.max(b));
        for t in projected.extrema() {
            if !(t > EXTREMUM_T_MIN && t < EXTREMUM_T_MAX) {
                continue;
            }
            let value = axis.of(segment.eval(t));
            if value > high + EXTREMUM_OVERSHOOT || value < low - EXTREMUM_OVERSHOOT {
                params.push(t);
            }
        }
    }
    params.sort_by(f64::total_cmp);
    params.dedup_by(|a, b| (*a - *b).abs() < SPLIT_T_EPSILON);
    params
}

// ---------------------------------------------------------------- fixing

/// Fixes the problems of the requested `kinds`, returning new contours; the
/// input is left unchanged and contour ids are preserved.
///
/// Within a pass the fixes run in this order, each on the result of the
/// previous one:
///
/// 1. [`IssueKind::DuplicatePoints`]: remove redundant nodes;
/// 2. [`IssueKind::MissingExtrema`]: split curves at their extrema;
/// 3. [`IssueKind::OffGrid`]: round every coordinate of the flagged contours
///    to `grid` (including the nodes added by the previous step);
/// 4. [`IssueKind::WrongDirection`]: reverse the flagged contours.
///
/// Only contours that [`check_paths`] flags for a kind are touched by that
/// kind's fix, and contours no fix applies to are returned as they were.
/// Kinds without a fix ([`IssueKind::OpenContour`], [`IssueKind::TinySegment`],
/// [`IssueKind::InvalidContour`]) are ignored, as are issues reported with
/// `fixable: false`.
///
/// Rounding to the grid can undo the work of the steps before it (two nodes
/// may land on the same point, a rounded curve may overshoot its end points),
/// so the pass is repeated until it changes nothing, up to a few times. The
/// result is then stable: fixing it again changes nothing.
#[must_use]
pub fn fix_paths(paths: &[Path], kinds: &[IssueKind], grid: f64) -> Vec<Path> {
    let mut fixed = paths.to_vec();
    for _ in 0..MAX_FIX_PASSES {
        let mut touched = false;
        for kind in FIX_ORDER {
            if kinds.contains(&kind) {
                touched |= apply_fix(kind, &mut fixed, grid);
            }
        }
        if !touched {
            break;
        }
    }
    fixed
}

/// Applies the fix for `kind` to every contour that `kind`'s check flags as
/// fixable; returns whether there was any.
fn apply_fix(kind: IssueKind, paths: &mut [Path], grid: f64) -> bool {
    let found = collect(paths, grid, &[kind]);
    let fixable = fixable_nodes(&found);
    match kind {
        IssueKind::DuplicatePoints => {
            for (&index, ends) in &fixable {
                remove_duplicates(&mut paths[index], ends);
            }
        }
        IssueKind::MissingExtrema => {
            for (&index, ends) in &fixable {
                add_extrema(&mut paths[index], ends);
            }
        }
        IssueKind::OffGrid => {
            for &index in fixable.keys() {
                for node in &mut paths[index].nodes {
                    node.x = snap(node.x, grid);
                    node.y = snap(node.y, grid);
                }
            }
        }
        IssueKind::WrongDirection => {
            for &index in fixable.keys() {
                if let Ok(nodes) = reversed_nodes(&paths[index]) {
                    paths[index].nodes = nodes;
                }
            }
        }
        IssueKind::OpenContour | IssueKind::TinySegment | IssueKind::InvalidContour => {}
    }
    !fixable.is_empty()
}

/// For each contour with fixable issues, the nodes its issues point at.
fn fixable_nodes(found: &[(usize, GeometryIssue)]) -> BTreeMap<usize, Vec<usize>> {
    let mut by_path: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, issue) in found.iter().filter(|(_, issue)| issue.fixable) {
        by_path.entry(*index).or_default().extend(issue.node);
    }
    by_path
}

/// A segment with owned nodes: its off-curve nodes and the on-curve node it
/// arrives at.
struct Piece {
    controls: Vec<Node>,
    end: Node,
}

impl Piece {
    fn keep(segment: &Segment) -> Self {
        Self {
            controls: segment.controls.iter().map(|&n| n.clone()).collect(),
            end: segment.end.clone(),
        }
    }
}

/// Lays out the nodes of a contour from the pieces that replace each segment
/// of `outline`; `replace` gets the segment's start point and the segment.
///
/// A closed contour comes out with its first on-curve node first and the
/// off-curve nodes of the closing segment last, the layout
/// [`reversed_nodes`] produces. Returns `None` if no piece is left.
fn rebuild<'a>(
    outline: &Outline<'a>,
    mut replace: impl FnMut(Point, &Segment<'a>) -> Vec<Piece>,
) -> Option<Vec<Node>> {
    let mut pieces = Vec::new();
    let mut start = point(outline.start);
    for segment in &outline.segments {
        pieces.extend(replace(start, segment));
        start = point(segment.end);
    }

    let mut nodes = Vec::new();
    if outline.closed {
        // The last piece arrives at the node the contour starts with.
        let last = pieces.pop()?;
        nodes.push(last.end);
        for piece in pieces {
            nodes.extend(piece.controls);
            nodes.push(piece.end);
        }
        nodes.extend(last.controls);
    } else {
        nodes.push(outline.start.clone());
        for piece in pieces {
            nodes.extend(piece.controls);
            nodes.push(piece.end);
        }
    }
    Some(nodes)
}

/// Removes the segments arriving at `ends` (node indices): their on-curve
/// node and their off-curve nodes.
fn remove_duplicates(path: &mut Path, ends: &[usize]) {
    let nodes = outline(path).ok().and_then(|outline| {
        rebuild(&outline, |_, segment| {
            if ends.contains(&segment.end_index) {
                Vec::new()
            } else {
                vec![Piece::keep(segment)]
            }
        })
    });
    if let Some(nodes) = nodes {
        path.nodes = nodes;
    }
}

/// Splits the segments arriving at `ends` (node indices) at their missing
/// extrema.
fn add_extrema(path: &mut Path, ends: &[usize]) {
    let nodes = outline(path).ok().and_then(|outline| {
        rebuild(&outline, |start, segment| {
            if ends.contains(&segment.end_index) {
                split_at_extrema(start, segment)
            } else {
                vec![Piece::keep(segment)]
            }
        })
    });
    if let Some(nodes) = nodes {
        path.nodes = nodes;
    }
}

/// Subdivides a curve segment at its missing extrema, so that the shape is
/// unchanged. The new on-curve nodes have the segment's kind and are smooth;
/// the segment's own end node is kept as it is.
fn split_at_extrema(start: Point, segment: &Segment) -> Vec<Piece> {
    let Some(geometry) = segment_geometry(start, segment) else {
        return vec![Piece::keep(segment)];
    };

    // Splitting at the extrema of the whole segment can leave a piece whose
    // own extrema now overshoot its (closer) end points, so repeat until no
    // piece is flagged.
    let mut parts = vec![geometry];
    for _ in 0..MAX_SPLIT_ROUNDS {
        let mut split_any = false;
        parts = parts
            .into_iter()
            .flat_map(|part| {
                let params = overshooting_extrema(&part);
                if params.is_empty() {
                    return vec![part];
                }
                split_any = true;
                let mut bounds = vec![0.0];
                bounds.extend(params);
                bounds.push(1.0);
                bounds
                    .windows(2)
                    .map(|w| part.subsegment(w[0]..w[1]))
                    .collect()
            })
            .collect();
        if !split_any {
            break;
        }
    }

    let count = parts.len();
    parts
        .into_iter()
        .enumerate()
        .map(|(i, part)| {
            let (controls, end) = match part {
                PathSeg::Cubic(c) => (vec![c.p1, c.p2], c.p3),
                PathSeg::Quad(q) => (vec![q.p1], q.p2),
                PathSeg::Line(l) => (Vec::new(), l.p1),
            };
            let end = if i + 1 == count {
                segment.end.clone()
            } else {
                Node {
                    x: end.x,
                    y: end.y,
                    kind: segment.kind,
                    smooth: true,
                }
            };
            Piece {
                controls: controls
                    .into_iter()
                    .map(|p| Node {
                        x: p.x,
                        y: p.y,
                        kind: NodeKind::OffCurve,
                        smooth: false,
                    })
                    .collect(),
                end,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_never_returns_negative_zero() {
        assert!(snap(-0.2, 1.0).is_sign_positive());
        assert_eq!(snap(2.6, 0.5), 2.5);
    }

    #[test]
    fn on_grid_accepts_rounding_noise_only() {
        assert!(on_grid(0.1 + 0.2, 0.1));
        assert!(!on_grid(0.3 + 1e-8, 0.1));
    }
}
