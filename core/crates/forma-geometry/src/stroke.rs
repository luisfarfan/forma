//! Strokes: outlines drawn around skeletons.
//!
//! A skeleton is the centre line of a stroke. [`stroke`] samples it, offsets
//! the samples by half the stroke width on each side along the normal, fits
//! cubics through the offsets ([`fit_cubics`]), closes the ends with caps,
//! rounds the corners and unites everything with [`remove_overlap`].

use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::fmt::Display;

use forma_model::{Node, NodeKind, Path, PathId};
use kurbo::{BezPath, CubicBez, ParamCurve, ParamCurveDeriv, PathSeg, Point, Vec2};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::checks::segment_geometry;
use crate::convert::{outline, point};
use crate::{GeometryError, IssueKind, fit_cubics, fix_paths, from_bezpath, remove_overlap};

/// The largest distance, in font units, between two consecutive samples of a
/// skeleton.
const SAMPLE_STEP: f64 = 2.0;

/// How far, in font units, the fitted offset curves may stray from the
/// offset samples.
const FIT_ERROR: f64 = 0.5;

/// The most samples one call may take, over all its skeletons: two million
/// font units of skeleton. It keeps absurd coordinates from exhausting
/// memory.
const MAX_SAMPLES: usize = 1_000_000;

/// Where two segments meet, the offsets of their ends may be this far apart,
/// in font units, and still be fitted as one smooth curve; a wider gap is a
/// corner and gets a round join.
const SMOOTH_JOIN_GAP: f64 = 1.0;

/// The largest change of direction, in degrees, at a smooth meeting of two
/// segments, however thin the stroke.
const SMOOTH_JOIN_ANGLE: f64 = 10.0;

/// A corner inside a segment (see `split_at_cusps`) closer than this, in font
/// units, to a sample is put at the sample.
const CORNER_SNAP: f64 = 1e-3;

/// The fraction of the bound on a segment's speed under which the segment
/// nearly stops: its samples there are less than 0.02 units apart (instead
/// of up to [`SAMPLE_STEP`]), so a turn between two of them happens on the
/// spot (see `split_at_cusps`).
const CUSP_SPEED: f64 = 0.01;

/// How many steps the search for a cusp between two samples takes; each one
/// keeps two thirds of the interval, so the cusp is found to within about
/// 1e-7 of the samples' spacing in `t`.
const CUSP_SEARCH_STEPS: usize = 40;

/// A segment whose points all lie within this distance of each other has no
/// length and is skipped.
const ZERO_LENGTH: f64 = 1e-9;

/// How far, in curve parameter, to look on either side of a point where the
/// derivative vanishes to find the direction of travel.
const DIRECTION_NUDGE: f64 = 1e-3;

/// A fitted cubic whose handles lie within this distance, in font units, of
/// its chord (and between its ends) is drawn as a line.
const STRAIGHT_TOLERANCE: f64 = 1e-6;

/// A node of a skeleton: a point of the centre line of a stroke.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkeletonNode {
    /// Horizontal position in font units.
    pub x: f64,
    /// Vertical position in font units (y axis up).
    pub y: f64,
    /// The role of the node, as in a contour: `line`, `curve` or `qcurve` for
    /// an on-curve node (the kind of the segment that arrives at it; the first
    /// node of an open skeleton is its start), `offcurve` for a control point.
    pub kind: NodeKind,
    /// The stroke width at this node in font units; the style's width when
    /// omitted. It must be a positive number, and only on-curve nodes use it:
    /// between two on-curve nodes the width changes linearly with the
    /// distance travelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
}

/// The centre line of a stroke: an open or closed contour of skeleton nodes.
///
/// It follows the node convention of contours: a cubic segment is two
/// `offcurve` nodes followed by a `curve` node, a quadratic one is one
/// `offcurve` node followed by a `qcurve` node. It needs at least two
/// on-curve nodes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Skeleton {
    /// The nodes, in drawing order.
    pub nodes: Vec<SkeletonNode>,
    /// Whether the skeleton is a closed loop (no caps), like the centre line
    /// of an "o".
    #[serde(default)]
    pub closed: bool,
}

/// The shape of the open ends of a stroke.
///
/// In JSON: `{"type": "round"}` or `{"type": "bulb", "ratio": 1.4}`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Cap {
    /// A half circle as wide as the stroke at that end.
    Round,
    /// A round cap plus a circle of radius `ratio × width / 2` centred on the
    /// end: a ball terminal.
    Bulb {
        /// The diameter of the ball relative to the stroke width at the end.
        /// It must be a positive number; up to 1 the ball fits inside the
        /// round cap and changes nothing.
        ratio: f64,
    },
}

/// How skeletons are stroked. Corners are always joined round.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StrokeStyle {
    /// The stroke width in font units, for the on-curve nodes that do not give
    /// their own. It must be a positive number.
    pub width: f64,
    /// The shape of the open ends.
    pub cap: Cap,
}

/// Draws the outline of strokes along `skeletons`.
///
/// Each skeleton is sampled at most 2 units apart; at every sample the width
/// is interpolated linearly, by distance, between the widths of the on-curve
/// nodes around it, and the sample is offset by half the width to each side
/// along the normal. Cubics are fitted through the offsets of each side with
/// an error of at most 0.5 units. The open ends of a skeleton get the `cap`
/// of the style, as wide as the stroke there; corners (where the direction
/// changes abruptly) get a round join. Finally everything is united with
/// [`remove_overlap`], so strokes that cross themselves or each other come
/// out as clean outlines: outer contours counter-clockwise, counters
/// clockwise, nodes at the extrema.
///
/// Where the skeleton bends more tightly than half the stroke width, the
/// offset on the inside of the bend stops at the centre of curvature instead
/// of folding back over itself (which would cut a false counter there); the
/// stroke fills the bend. At a cusp inside a segment, where the curve turns
/// on the spot (typically back on itself), the segment is cut and the pieces
/// are joined round, like a corner; smooth bends are never cut.
///
/// # Errors
///
/// * [`GeometryError::InvalidPath`] if the style's width, a node's width or
///   the bulb ratio is not a positive number, or a skeleton has fewer than
///   two on-curve nodes, no length, a coordinate that is not finite or nodes
///   that break the contour convention, or the skeletons are too long
///   (more than a million samples). The message names the skeleton.
/// * [`GeometryError::Degenerate`] if removing the overlaps fails.
pub fn stroke(skeletons: &[Skeleton], style: &StrokeStyle) -> Result<Vec<Path>, GeometryError> {
    check_positive(style.width, "the stroke width")?;
    if let Cap::Bulb { ratio } = style.cap {
        check_positive(ratio, "the bulb ratio")?;
    }
    let mut budget = MAX_SAMPLES;
    let mut paths: Vec<Path> = Vec::new();
    for (index, skeleton) in skeletons.iter().enumerate() {
        let shape = shape(skeleton, style.width, &mut budget).map_err(|error| match error {
            GeometryError::InvalidPath(message) | GeometryError::Degenerate(message) => {
                GeometryError::InvalidPath(format!("skeleton {index}: {message}"))
            }
        })?;
        for contour in draw(&shape, style.cap) {
            let id = u32::try_from(paths.len())
                .map_err(|_| GeometryError::InvalidPath("too many contours".into()))?;
            match from_bezpath(&contour, id) {
                Ok(converted) => paths.extend(converted),
                // A band edge that shrank to a point (the inside of a loop
                // tighter than the stroke) encloses nothing.
                Err(GeometryError::Degenerate(_)) => {}
                Err(error) => return Err(error),
            }
        }
    }
    // The union comes out in PostScript direction. Contours that need no
    // union come back as drawn, which linesweeper only does when they already
    // run that way; either way `normalize_direction` is not needed. Contours
    // that come back as drawn still lack extrema nodes.
    let united = remove_overlap(&paths)?;
    Ok(fix_paths(
        &united,
        &[IssueKind::MissingExtrema, IssueKind::DuplicatePoints],
        0.0,
    ))
}

fn check_positive(value: f64, what: impl Display) -> Result<(), GeometryError> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(GeometryError::InvalidPath(format!(
            "{what} must be a positive number, got {value}"
        )))
    }
}

// --------------------------------------------------------------- sampling

/// A sample of a skeleton: where it is, which way it goes, how wide the
/// stroke is there and how far its band reaches on each side.
#[derive(Clone, Copy, Debug)]
struct Station {
    at: Point,
    /// The unit direction of travel.
    dir: Vec2,
    width: f64,
    /// How far the band reaches to the left: half the width, or less where
    /// the skeleton bends to the left more tightly than that (see `reach`).
    left_reach: f64,
    /// How far the band reaches to the right.
    right_reach: f64,
}

impl Station {
    fn new(at: Point, dir: Vec2, width: f64, curvature: f64) -> Self {
        Self {
            at,
            dir,
            width,
            left_reach: reach(width, curvature),
            right_reach: reach(width, -curvature),
        }
    }

    /// The edge of the band on one side (see `reach`).
    fn offset(&self, side: Side) -> Point {
        match side {
            Side::Left => self.at + self.dir.turn_90() * self.left_reach,
            Side::Right => self.at - self.dir.turn_90() * self.right_reach,
        }
    }

    /// The point half the width away on one side, where caps and outer joins
    /// start and end.
    fn full(&self, side: Side) -> Point {
        let half = self.dir.turn_90() * (self.width / 2.0);
        match side {
            Side::Left => self.at + half,
            Side::Right => self.at - half,
        }
    }
}

/// How far the band of a stroke `width` wide reaches on the side towards
/// which the skeleton bends with `curvature` (positive: bending towards that
/// side).
///
/// Half the width, except where the radius of curvature is smaller: an offset
/// beyond the centre of curvature folds back over itself, its winding cancels
/// that of the part before it and the union would cut a false counter there.
/// Stopping at the centre keeps every part of the band winding the same way;
/// the area beyond it is covered by the band on either side of the bend, the
/// joins and the caps.
fn reach(width: f64, curvature: f64) -> f64 {
    let half = width / 2.0;
    if curvature > 0.0 {
        half.min(1.0 / curvature)
    } else {
        half
    }
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

/// A skeleton cut at its corners into runs of samples along which the
/// direction changes smoothly. Consecutive runs meet at a corner: the last
/// station of one is at the same point as the first station of the next.
enum Shape {
    /// An open skeleton: caps at both ends.
    Open(Vec<Vec<Station>>),
    /// A closed skeleton with corners; the last run meets the first one.
    Ring(Vec<Vec<Station>>),
    /// A closed skeleton without corners: one run that loops back to its
    /// first station (which is not repeated at the end).
    Loop(Vec<Station>),
}

fn unit(v: Vec2) -> Option<Vec2> {
    let length = v.hypot();
    (length.is_finite() && length > 1e-12).then(|| v / length)
}

/// A bound on the speed `|dB/dt|` of a segment: the samples taken at even
/// steps of `t` are never farther apart than this divided by the number of
/// steps.
fn speed_bound(segment: &PathSeg) -> f64 {
    match *segment {
        PathSeg::Line(l) => l.p0.distance(l.p1),
        PathSeg::Quad(q) => 2.0 * q.p0.distance(q.p1).max(q.p1.distance(q.p2)),
        PathSeg::Cubic(c) => {
            3.0 * c
                .p0
                .distance(c.p1)
                .max(c.p1.distance(c.p2))
                .max(c.p2.distance(c.p3))
        }
    }
}

fn derivative(segment: &PathSeg, t: f64) -> Vec2 {
    match *segment {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => q.deriv().eval(t).to_vec2(),
        PathSeg::Cubic(c) => c.deriv().eval(t).to_vec2(),
    }
}

/// The direction of travel at `t`. Where the derivative vanishes (a handle on
/// its node) it is taken from the points just around `t`.
fn direction_at(segment: &PathSeg, t: f64) -> Option<Vec2> {
    unit(derivative(segment, t)).or_else(|| {
        let before = segment.eval((t - DIRECTION_NUDGE).max(0.0));
        let after = segment.eval((t + DIRECTION_NUDGE).min(1.0));
        unit(after - before)
    })
}

fn second_derivative(segment: &PathSeg, t: f64) -> Vec2 {
    match *segment {
        PathSeg::Line(_) => Vec2::ZERO,
        PathSeg::Quad(q) => q.deriv().deriv().eval(t).to_vec2(),
        PathSeg::Cubic(c) => c.deriv().deriv().eval(t).to_vec2(),
    }
}

/// The signed curvature at `t`: positive where the segment bends to the left
/// (counter-clockwise), 0 where it has no defined direction.
fn curvature_at(segment: &PathSeg, t: f64) -> f64 {
    let d1 = derivative(segment, t);
    let speed = d1.hypot();
    if !(speed.is_finite() && speed > 1e-12) {
        return 0.0;
    }
    let curvature = d1.cross(second_derivative(segment, t)) / (speed * speed * speed);
    if curvature.is_finite() {
        curvature
    } else {
        0.0
    }
}

/// Samples a segment at even steps of `t`, at most [`SAMPLE_STEP`] apart,
/// with widths from `w0` at its start to `w1` at its end, interpolated by
/// distance, and cuts the samples into runs at its cusps (see
/// `split_at_cusps`). `None` for a segment without length.
fn sample_segment(
    segment: &PathSeg,
    w0: f64,
    w1: f64,
    budget: &mut usize,
) -> Result<Option<Vec<Vec<Station>>>, GeometryError> {
    let bound = speed_bound(segment);
    if bound <= ZERO_LENGTH {
        return Ok(None);
    }
    let steps = (bound / SAMPLE_STEP).ceil().max(1.0);
    if !steps.is_finite() || steps >= *budget as f64 {
        return Err(GeometryError::InvalidPath(format!(
            "the skeletons are too long to stroke (more than {MAX_SAMPLES} samples)"
        )));
    }
    let steps = steps as usize;
    *budget -= steps + 1;

    let params: Vec<f64> = (0..=steps).map(|k| k as f64 / steps as f64).collect();
    let points: Vec<Point> = params
        .iter()
        .enumerate()
        .map(|(k, &t)| match k {
            0 => segment.start(),
            _ if k == steps => segment.end(),
            _ => segment.eval(t),
        })
        .collect();
    let mut walked = Vec::with_capacity(points.len());
    let mut distance = 0.0;
    walked.push(0.0);
    for pair in points.windows(2) {
        distance += pair[0].distance(pair[1]);
        walked.push(distance);
    }

    let chord = unit(segment.end() - segment.start());
    let mut samples: Vec<(f64, Station)> = Vec::with_capacity(points.len());
    for (k, ((&t, &at), &along)) in params.iter().zip(&points).zip(&walked).enumerate() {
        let tangent = unit(derivative(segment, t));
        if tangent.is_none() && k != 0 && k != steps {
            // The segment stops dead inside: a cusp, where its direction is
            // not defined. `split_at_cusps` puts a corner there.
            continue;
        }
        let dir = tangent
            .or_else(|| direction_at(segment, t))
            .or_else(|| samples.last().map(|(_, s)| s.dir))
            .or(chord)
            .unwrap_or(Vec2::new(1.0, 0.0));
        let width = if distance > 0.0 {
            w0 + (w1 - w0) * (along / distance)
        } else {
            w0
        };
        samples.push((t, Station::new(at, dir, width, curvature_at(segment, t))));
    }
    Ok(Some(split_at_cusps(segment, bound, &samples)))
}

/// Whether the direction turns by at most [`SMOOTH_JOIN_ANGLE`] from `a` to
/// `b`.
fn turns_smoothly(a: &Station, b: &Station) -> bool {
    a.dir.dot(b.dir) >= SMOOTH_JOIN_ANGLE.to_radians().cos()
}

/// Cuts the samples of a segment into runs at its cusps, where it turns on
/// the spot: where the direction turns by more than 90 degrees from one
/// sample to the next, or by more than [`SMOOTH_JOIN_ANGLE`] where the
/// segment nearly stops (its speed drops below [`CUSP_SPEED`] of `bound`, the
/// bound on its speed, between the two samples; a sample where it stops dead
/// has been left out already). The offsets cannot follow such a turn (they
/// would leave a bite out of the stroke); consecutive runs meet at a corner
/// instead, drawn like the corners between segments, where the segment moves
/// slowest between the two samples (the cusp itself) unless that is within
/// [`CORNER_SNAP`] of one of them. Smooth bends, however tight, are not cut:
/// the offsets follow them (see `reach` for the inside).
///
/// Runs left with a single station (two corners at the same sample) are
/// dropped: the runs around them meet at that point.
fn split_at_cusps(segment: &PathSeg, bound: f64, samples: &[(f64, Station)]) -> Vec<Vec<Station>> {
    let mut runs: Vec<Vec<Station>> = Vec::new();
    let mut run: Vec<Station> = Vec::new();
    let mut previous: Option<&(f64, Station)> = None;
    for sample in samples {
        let (t, station) = *sample;
        if let Some(&(t0, before)) = previous
            && let Some(slowest) = cusp_between(segment, bound, (t0, &before), (t, &station))
        {
            let at = if slowest.distance(before.at) <= CORNER_SNAP {
                before.at
            } else if slowest.distance(station.at) <= CORNER_SNAP {
                station.at
            } else {
                slowest
            };
            if at != before.at {
                run.push(Station { at, ..before });
            }
            runs.push(std::mem::take(&mut run));
            if at != station.at {
                run.push(Station { at, ..station });
            }
        }
        run.push(station);
        previous = Some(sample);
    }
    runs.push(run);
    runs.retain(|run| run.len() >= 2);
    runs
}

/// Where `segment` has a cusp between two consecutive samples (see
/// `split_at_cusps`): the point where it moves slowest between them, or
/// `None` if it turns smoothly there.
fn cusp_between(
    segment: &PathSeg,
    bound: f64,
    (t0, a): (f64, &Station),
    (t1, b): (f64, &Station),
) -> Option<Point> {
    if turns_smoothly(a, b) {
        return None;
    }
    let speed = |t: f64| derivative(segment, t).hypot();
    // A ternary search for the slowest point.
    let (mut lo, mut hi) = (t0, t1);
    for _ in 0..CUSP_SEARCH_STEPS {
        let third = (hi - lo) / 3.0;
        if speed(lo + third) <= speed(hi - third) {
            hi -= third;
        } else {
            lo += third;
        }
    }
    let slowest = (lo + hi) / 2.0;
    let turns_back = a.dir.dot(b.dir) < 0.0;
    (turns_back || speed(slowest) <= CUSP_SPEED * bound).then(|| segment.eval(slowest))
}

/// Whether two stations at the same point continue each other smoothly: the
/// direction changes by at most [`SMOOTH_JOIN_ANGLE`] and the offsets on both
/// sides are at most [`SMOOTH_JOIN_GAP`] apart.
fn joins_smoothly(a: &Station, b: &Station) -> bool {
    turns_smoothly(a, b)
        && [Side::Left, Side::Right]
            .iter()
            .all(|&side| a.full(side).distance(b.full(side)) <= SMOOTH_JOIN_GAP)
}

/// Merges the station that ends a segment into `b`, the one that starts the
/// next at the same point: the mean of the two directions and the shorter
/// reach on each side.
fn blend(a: &Station, b: &mut Station) {
    b.dir = unit(a.dir + b.dir).unwrap_or(b.dir);
    b.left_reach = a.left_reach.min(b.left_reach);
    b.right_reach = a.right_reach.min(b.right_reach);
}

/// Validates and samples a skeleton, and cuts it into runs at its corners.
fn shape(
    skeleton: &Skeleton,
    default_width: f64,
    budget: &mut usize,
) -> Result<Shape, GeometryError> {
    let on_curve = skeleton
        .nodes
        .iter()
        .filter(|node| node.kind != NodeKind::OffCurve)
        .count();
    if on_curve < 2 {
        return Err(GeometryError::InvalidPath(format!(
            "a skeleton needs at least 2 on-curve nodes, found {on_curve}"
        )));
    }
    for (index, node) in skeleton.nodes.iter().enumerate() {
        if let Some(width) = node.width {
            check_positive(width, format_args!("the width of node {index}"))?;
        }
    }

    let path = Path {
        id: PathId(0),
        closed: skeleton.closed,
        nodes: skeleton
            .nodes
            .iter()
            .map(|node| Node {
                x: node.x,
                y: node.y,
                kind: node.kind,
                smooth: false,
            })
            .collect(),
    };
    let outline = outline(&path)?;
    let width_of = |index: usize| {
        skeleton
            .nodes
            .get(index)
            .and_then(|node| node.width)
            .unwrap_or(default_width)
    };

    // The node each segment starts from: the first on-curve node of a closed
    // skeleton (where its last segment ends), node 0 of an open one.
    let mut from_index = if skeleton.closed {
        outline
            .segments
            .last()
            .map_or(0, |segment| segment.end_index)
    } else {
        0
    };
    let mut from = point(outline.start);
    let mut runs: Vec<Vec<Station>> = Vec::new();
    for segment in &outline.segments {
        let geometry = segment_geometry(from, segment).ok_or_else(|| {
            GeometryError::InvalidPath("segment does not match the contour convention".into())
        })?;
        let sampled = sample_segment(
            &geometry,
            width_of(from_index),
            width_of(segment.end_index),
            budget,
        )?;
        from_index = segment.end_index;
        from = point(segment.end);
        let mut pieces = sampled.unwrap_or_default().into_iter();
        let Some(stations) = pieces.next() else {
            continue;
        };
        match runs.last_mut() {
            Some(run)
                if run
                    .last()
                    .is_some_and(|last| joins_smoothly(last, &stations[0])) =>
            {
                if let Some(last) = run.last_mut() {
                    blend(&stations[0], last);
                }
                run.extend_from_slice(&stations[1..]);
            }
            _ => runs.push(stations),
        }
        // The segment's own corners (cusps) start new runs.
        runs.extend(pieces);
    }
    if runs.is_empty() {
        return Err(GeometryError::InvalidPath(
            "the skeleton has no length".into(),
        ));
    }
    if !skeleton.closed {
        return Ok(Shape::Open(runs));
    }

    // The last station of a closed skeleton is back at its first one.
    let closing = match (runs.last().and_then(|run| run.last()), runs[0].first()) {
        (Some(last), Some(first)) => joins_smoothly(last, first),
        _ => false,
    };
    if !closing {
        return Ok(Shape::Ring(runs));
    }
    if runs.len() == 1 {
        let mut run = runs.remove(0);
        if let Some(last) = run.pop() {
            blend(&last, &mut run[0]);
        }
        return Ok(Shape::Loop(run));
    }
    // The first run continues the last one: append it, and the corners stay
    // between consecutive runs.
    let first = runs.remove(0);
    if let Some(run) = runs.last_mut() {
        if let Some(last) = run.last_mut() {
            blend(&first[0], last);
        }
        run.extend_from_slice(&first[1..]);
    }
    Ok(Shape::Ring(runs))
}

// ---------------------------------------------------------------- drawing

/// Fits cubics through the offsets of a run on one side.
fn fit_side(run: &[Station], side: Side, closed: bool) -> Vec<CubicBez> {
    let points: Vec<Point> = run.iter().map(|station| station.offset(side)).collect();
    fit_cubics(&points, FIT_ERROR, closed)
}

/// Whether a cubic is a straight line: its handles on its chord, between its
/// ends.
fn is_straight(c: &CubicBez) -> bool {
    let chord = c.p3 - c.p0;
    let length = chord.hypot();
    if length <= ZERO_LENGTH {
        return false;
    }
    [c.p1, c.p2].iter().all(|&handle| {
        let v = handle - c.p0;
        let along = chord.dot(v) / (length * length);
        chord.cross(v).abs() / length <= STRAIGHT_TOLERANCE && (0.0..=1.0).contains(&along)
    })
}

fn push_cubic(out: &mut BezPath, c: CubicBez) {
    if is_straight(&c) {
        out.line_to(c.p3);
    } else {
        out.curve_to(c.p1, c.p2, c.p3);
    }
}

/// Appends the cubics fitted along a side; `end` is where they end, drawn as
/// a line if the fit gave nothing.
fn push_forward(out: &mut BezPath, cubics: &[CubicBez], end: Point) {
    if cubics.is_empty() {
        out.line_to(end);
    }
    for &c in cubics {
        push_cubic(out, c);
    }
}

/// Appends the cubics fitted along a side, backwards; `start` is where they
/// start (where the backward walk ends).
fn push_backward(out: &mut BezPath, cubics: &[CubicBez], start: Point) {
    if cubics.is_empty() {
        out.line_to(start);
    }
    for c in cubics.iter().rev() {
        push_cubic(out, CubicBez::new(c.p3, c.p2, c.p1, c.p0));
    }
}

/// Appends an arc around `center` from the current point `from` to `to`,
/// turning by `sweep` radians (counter-clockwise when positive), with cubics
/// of at most a quarter turn each. The radius changes linearly from that of
/// `from` to that of `to`; `to` is reached exactly.
fn arc_to(out: &mut BezPath, center: Point, from: Point, to: Point, sweep: f64) {
    let (u, v) = (from - center, to - center);
    let (r0, r1) = (u.hypot(), v.hypot());
    let start_angle = u.atan2();
    // A half turn takes exactly two quarters.
    let pieces = (sweep.abs() / FRAC_PI_2 - 1e-9).ceil().max(1.0) as usize;
    let step = sweep / pieces as f64;
    let handle = 4.0 / 3.0 * (step / 4.0).tan();
    let mut previous = from;
    for i in 1..=pieces {
        let (s0, s1) = ((i - 1) as f64, i as f64);
        let n = pieces as f64;
        let (ra, rb) = (r0 + (r1 - r0) * s0 / n, r0 + (r1 - r0) * s1 / n);
        let (ta, tb) = (start_angle + step * s0, start_angle + step * s1);
        let end = if i == pieces {
            to
        } else {
            center + Vec2::from_angle(tb) * rb
        };
        let c1 = previous + Vec2::from_angle(ta).turn_90() * (handle * ra);
        let c2 = end - Vec2::from_angle(tb).turn_90() * (handle * rb);
        out.curve_to(c1, c2, end);
        previous = end;
    }
}

/// Goes from the current point (the edge of the band at `station`, on
/// `side`) out to the full half width, if the band stops short of it there.
fn out_to_full(out: &mut BezPath, station: &Station, side: Side) {
    let (edge, full) = (station.offset(side), station.full(side));
    if edge != full {
        out.line_to(full);
    }
}

/// Comes back from the full half width at `station` to the edge of its band
/// on `side`, if the band stops short of it there.
fn in_from_full(out: &mut BezPath, station: &Station, side: Side) {
    let (edge, full) = (station.offset(side), station.full(side));
    if edge != full {
        out.line_to(edge);
    }
}

/// The corner between the end station `a` of a run and the start station `b`
/// of the next one (at the same point), on one side. The right side is
/// walked forwards (from `a` to `b`), the left side backwards; either way the
/// stroke is on the left of the walk.
///
/// On the outside of the corner (a positive turn for the walk) the two band
/// edges are joined by an arc of the full half width around the corner
/// point. On the inside the edges overlap; going through the corner point
/// keeps the area between them covered, with the same winding, for the
/// union.
fn corner(out: &mut BezPath, a: &Station, b: &Station, side: Side) {
    let angle = a.dir.cross(b.dir).atan2(a.dir.dot(b.dir));
    let (from, to, turn) = match side {
        Side::Right => (a, b, angle),
        Side::Left => (b, a, -angle),
    };
    if turn > 0.0 {
        out_to_full(out, from, side);
        arc_to(out, a.at, from.full(side), to.full(side), turn);
        in_from_full(out, to, side);
    } else {
        out.line_to(a.at);
        out.line_to(to.offset(side));
    }
}

/// The cap at an open end: from the band edge on side `from` round to the
/// band edge on the other side, along a half circle of the full half width
/// (in front of the end for the right-to-left cap, behind it for the
/// left-to-right one).
fn cap(out: &mut BezPath, end: &Station, from: Side, to: Side) {
    out_to_full(out, end, from);
    arc_to(out, end.at, end.full(from), end.full(to), PI);
    in_from_full(out, end, to);
}

/// A closed circle around `center` through `start`, counter-clockwise.
fn circle(center: Point, start: Point) -> BezPath {
    let mut out = BezPath::new();
    out.move_to(start);
    arc_to(&mut out, center, start, start, TAU);
    out.close_path();
    out
}

/// The contours of one stroke. Each runs with the stroke on its left
/// (counter-clockwise), so that overlapping parts add up under the non-zero
/// rule instead of cancelling.
fn draw(shape: &Shape, cap: Cap) -> Vec<BezPath> {
    match shape {
        Shape::Open(runs) => draw_open(runs, cap),
        Shape::Ring(runs) => draw_ring(runs),
        Shape::Loop(run) => draw_loop(run),
    }
}

fn draw_open(runs: &[Vec<Station>], end_cap: Cap) -> Vec<BezPath> {
    let (Some(first), Some(last)) = (
        runs.first().and_then(|run| run.first()).copied(),
        runs.last().and_then(|run| run.last()).copied(),
    ) else {
        return Vec::new();
    };
    // Along the right side, round the end, back along the left side and
    // round the start.
    let mut out = BezPath::new();
    out.move_to(first.offset(Side::Right));
    for (i, run) in runs.iter().enumerate() {
        let end = run.last().copied().unwrap_or(last);
        let fitted = fit_side(run, Side::Right, false);
        push_forward(&mut out, &fitted, end.offset(Side::Right));
        if let Some(next) = runs.get(i + 1).and_then(|next| next.first()) {
            corner(&mut out, &end, next, Side::Right);
        }
    }
    cap(&mut out, &last, Side::Right, Side::Left);
    for (i, run) in runs.iter().enumerate().rev() {
        let start = run.first().copied().unwrap_or(first);
        let fitted = fit_side(run, Side::Left, false);
        push_backward(&mut out, &fitted, start.offset(Side::Left));
        if let Some(previous) = i.checked_sub(1).and_then(|j| runs[j].last()) {
            corner(&mut out, previous, &start, Side::Left);
        }
    }
    cap(&mut out, &first, Side::Left, Side::Right);
    out.close_path();

    let mut contours = vec![out];
    if let Cap::Bulb { ratio } = end_cap
        && ratio > 1.0
    {
        for end in [first, last] {
            let radius = ratio * end.width / 2.0;
            contours.push(circle(end.at, end.at - end.dir.turn_90() * radius));
        }
    }
    contours
}

fn draw_ring(runs: &[Vec<Station>]) -> Vec<BezPath> {
    let count = runs.len();
    if count == 0 {
        return Vec::new();
    }
    let station = |run: usize, last: bool| {
        let run = &runs[run % count];
        if last { run.last() } else { run.first() }
    };
    let (Some(start), Some(end)) = (station(0, false), station(count - 1, true)) else {
        return Vec::new();
    };
    // The right side forwards and the left side backwards, each a loop with
    // the corners between the runs.
    let mut right = BezPath::new();
    right.move_to(start.offset(Side::Right));
    for (i, run) in runs.iter().enumerate() {
        let (Some(last), Some(next)) = (station(i, true), station(i + 1, false)) else {
            continue;
        };
        let fitted = fit_side(run, Side::Right, false);
        push_forward(&mut right, &fitted, last.offset(Side::Right));
        corner(&mut right, last, next, Side::Right);
    }
    right.close_path();

    let mut left = BezPath::new();
    left.move_to(end.offset(Side::Left));
    for (i, run) in runs.iter().enumerate().rev() {
        let (Some(first), Some(previous)) = (station(i, false), station(i + count - 1, true))
        else {
            continue;
        };
        let fitted = fit_side(run, Side::Left, false);
        push_backward(&mut left, &fitted, first.offset(Side::Left));
        corner(&mut left, previous, first, Side::Left);
    }
    left.close_path();
    vec![right, left]
}

fn draw_loop(run: &[Station]) -> Vec<BezPath> {
    let Some(start) = run.first() else {
        return Vec::new();
    };
    let mut right = BezPath::new();
    right.move_to(start.offset(Side::Right));
    push_forward(
        &mut right,
        &fit_side(run, Side::Right, true),
        start.offset(Side::Right),
    );
    right.close_path();
    let mut left = BezPath::new();
    left.move_to(start.offset(Side::Left));
    push_backward(
        &mut left,
        &fit_side(run, Side::Left, true),
        start.offset(Side::Left),
    );
    left.close_path();
    vec![right, left]
}
