//! Cubic Bezier curve fitting and path simplification.
//!
//! [`fit_cubics`] is the curve fitting algorithm of Philip J. Schneider,
//! "An Algorithm for Automatically Fitting Digitized Curves" (Graphics Gems,
//! 1990), written from the published description: least-squares cubic through
//! the points with fixed end tangents, chord-length parameters improved by a
//! few Newton-Raphson steps, and a split at the point of largest error when
//! the fit is not good enough. [`simplify_path`] uses it to replace the
//! stretches of a contour between corners with fewer segments.

use forma_model::{Node, NodeKind, Path};
use kurbo::{
    BezPath, CubicBez, Line, ParamCurve, ParamCurveDeriv, ParamCurveNearest, PathEl, Point, Vec2,
};

use crate::GeometryError;
use crate::convert::{Segment, outline, point};

/// Points closer than this are the same point.
const DUPLICATE_EPSILON: f64 = 1e-9;

/// How many times the parameters of a stretch are refined (Newton-Raphson).
const MAX_REPARAMETERIZATIONS: usize = 4;

/// The tangent angle above which a node counts as a corner, in degrees.
const CORNER_ANGLE_DEGREES: f64 = 3.0;

/// The share of the error budget of [`simplify_path`] spent on flattening the
/// original curves into sample points; the fit gets the rest.
const FLATTEN_SHARE: f64 = 0.1;

// ------------------------------------------------------------------ fitting

fn unit(v: Vec2) -> Option<Vec2> {
    let length = v.hypot();
    (length.is_finite() && length > DUPLICATE_EPSILON).then(|| v / length)
}

/// A stretch of the points still to fit, and the tangents its curve must have.
#[derive(Clone, Copy)]
struct Job {
    first: usize,
    last: usize,
    /// The direction in which the curve leaves `first`.
    t1: Vec2,
    /// The direction from `last` back into the curve.
    t2: Vec2,
}

enum Outcome {
    Fitted(CubicBez),
    /// The fit is not good enough; split at this point index.
    Split(usize),
}

/// Parameters in 0..=1 proportional to the distance walked along the points.
fn chord_parameters(pts: &[Point]) -> Vec<f64> {
    let mut u = Vec::with_capacity(pts.len());
    let mut walked = 0.0;
    u.push(0.0);
    for pair in pts.windows(2) {
        walked += pair[0].distance(pair[1]);
        u.push(walked);
    }
    if walked > 0.0 {
        for value in &mut u {
            *value /= walked;
        }
    }
    if let Some(last) = u.last_mut() {
        *last = 1.0;
    }
    u
}

/// The cubic from `pts[0]` to the last point with the given end tangents whose
/// handle lengths minimise the squared distance to the points at parameters
/// `u`.
fn generate_bezier(pts: &[Point], u: &[f64], t1: Vec2, t2: Vec2) -> CubicBez {
    let p0 = pts[0];
    let p3 = pts[pts.len() - 1];

    let (mut c00, mut c01, mut c11) = (0.0, 0.0, 0.0);
    let (mut x0, mut x1) = (0.0, 0.0);
    for (&p, &t) in pts.iter().zip(u) {
        let s = 1.0 - t;
        let b0 = s * s * s;
        let b1 = 3.0 * s * s * t;
        let b2 = 3.0 * s * t * t;
        let b3 = t * t * t;
        let a1 = t1 * b1;
        let a2 = t2 * b2;
        c00 += a1.dot(a1);
        c01 += a1.dot(a2);
        c11 += a2.dot(a2);
        let rest = p.to_vec2() - (p0.to_vec2() * (b0 + b1) + p3.to_vec2() * (b2 + b3));
        x0 += a1.dot(rest);
        x1 += a2.dot(rest);
    }

    let det = c00 * c11 - c01 * c01;
    let chord = p0.distance(p3);
    // A closed loop starts and ends at the same point: fall back on the
    // length walked rather than on a zero chord.
    let reach = if chord > DUPLICATE_EPSILON {
        chord
    } else {
        pts.windows(2)
            .map(|pair| pair[0].distance(pair[1]))
            .sum::<f64>()
            / 2.0
    };
    let heuristic = reach / 3.0;

    let mut alpha_l = heuristic;
    let mut alpha_r = heuristic;
    if det.abs() > 1e-12 * (c00 * c11).max(f64::MIN_POSITIVE) {
        let l = (x0 * c11 - x1 * c01) / det;
        let r = (c00 * x1 - c01 * x0) / det;
        // Handles that vanish or point backwards give a degenerate curve:
        // keep the one-third heuristic instead.
        let floor = 1e-6 * reach;
        if l > floor && r > floor && l.is_finite() && r.is_finite() {
            alpha_l = l;
            alpha_r = r;
        }
    }
    CubicBez::new(p0, p0 + t1 * alpha_l, p3 + t2 * alpha_r, p3)
}

/// The largest squared distance between the points and the curve at their
/// parameters, over the interior points, and the index of that point. The
/// index is always interior, even if every distance is NaN.
fn max_error(pts: &[Point], u: &[f64], bez: &CubicBez) -> (f64, usize) {
    let mut worst = 0.0;
    let mut index = pts.len() / 2;
    for i in 1..pts.len() - 1 {
        let d = bez.eval(u[i]).distance_squared(pts[i]);
        if d.is_nan() {
            return (f64::INFINITY, i);
        }
        if d > worst {
            worst = d;
            index = i;
        }
    }
    (worst, index)
}

/// One Newton-Raphson step moving `u` towards the parameter of the point of
/// the curve nearest to `p`.
fn newton_step(bez: &CubicBez, p: Point, u: f64) -> f64 {
    let first = bez.deriv();
    let second = first.deriv();
    let diff = bez.eval(u) - p;
    let d1 = first.eval(u).to_vec2();
    let d2 = second.eval(u).to_vec2();
    let denominator = d1.dot(d1) + diff.dot(d2);
    if denominator.abs() < 1e-12 {
        return u;
    }
    let next = u - diff.dot(d1) / denominator;
    if next.is_finite() {
        next.clamp(0.0, 1.0)
    } else {
        u
    }
}

fn reparameterize(pts: &[Point], u: &[f64], bez: &CubicBez) -> Vec<f64> {
    let last = pts.len() - 1;
    pts.iter()
        .zip(u)
        .enumerate()
        .map(|(i, (&p, &t))| {
            // The end points stay where they are.
            if i == 0 || i == last {
                t
            } else {
                newton_step(bez, p, t)
            }
        })
        .collect()
}

fn fit_job(pts: &[Point], job: &Job, tol_sq: f64) -> Outcome {
    let seg = &pts[job.first..=job.last];
    if seg.len() == 2 {
        // Nothing to fit: a curve with handles a third of the way along the
        // tangents.
        let third = seg[0].distance(seg[1]) / 3.0;
        return Outcome::Fitted(CubicBez::new(
            seg[0],
            seg[0] + job.t1 * third,
            seg[1] + job.t2 * third,
            seg[1],
        ));
    }

    let mut u = chord_parameters(seg);
    let mut bez = generate_bezier(seg, &u, job.t1, job.t2);
    let (mut worst, mut split) = max_error(seg, &u, &bez);
    let mut fitted = (worst <= tol_sq).then_some(bez);
    for _ in 0..MAX_REPARAMETERIZATIONS {
        u = reparameterize(seg, &u, &bez);
        bez = generate_bezier(seg, &u, job.t1, job.t2);
        (worst, split) = max_error(seg, &u, &bez);
        if worst <= tol_sq {
            fitted = Some(bez);
        }
    }
    match fitted {
        Some(bez) => Outcome::Fitted(bez),
        None => Outcome::Split(job.first + split),
    }
}

/// The first of `walk` that is at least `reach` from `origin`, or the last one
/// if none is.
fn first_beyond(walk: impl Iterator<Item = Point>, origin: Point, reach: f64) -> Option<Point> {
    let mut last = None;
    for p in walk {
        if origin.distance(p) >= reach {
            return Some(p);
        }
        last = Some(p);
    }
    last
}

/// The direction of travel at the end point `inward` starts from, estimated
/// from the points that follow it.
///
/// Points closer than `reach` to the one before them say little about the
/// direction (noise dominates), so the estimate uses the first two points at
/// least `reach` apart: the derivative at the end of the parabola through the
/// three, exact for a curve that is locally a parabola. With a single such
/// point (or a parabola that disagrees with the chord) it falls back on the
/// direction of the chord.
fn tangent_into(inward: impl Iterator<Item = Point>, reach: f64) -> Option<Vec2> {
    let mut walk = inward;
    let origin = walk.next()?;
    let mut first: Option<Point> = None;
    let mut second: Option<Point> = None;
    let mut farthest = origin;
    for p in walk {
        farthest = p;
        match first {
            None if origin.distance(p) >= reach => first = Some(p),
            Some(a) if a.distance(p) >= reach => {
                second = Some(p);
                break;
            }
            _ => {}
        }
    }
    let a = first.unwrap_or(farthest);
    let chord = unit(a - origin);
    let (Some(b), Some(chord)) = (second, chord) else {
        return chord;
    };
    let h1 = origin.distance(a);
    let h2 = a.distance(b);
    let parabola = origin.to_vec2() * (-(2.0 * h1 + h2) / (h1 * (h1 + h2)))
        + a.to_vec2() * ((h1 + h2) / (h1 * h2))
        - b.to_vec2() * (h1 / (h2 * (h1 + h2)));
    unit(parabola)
        .filter(|d| d.dot(chord) > 0.0)
        .or(Some(chord))
}

/// The unit tangent at the interior point `k` of the stretch `first..=last`,
/// pointing back along the points.
fn center_tangent(pts: &[Point], first: usize, k: usize, last: usize, reach: f64) -> Vec2 {
    let before = first_beyond(pts[first..k].iter().rev().copied(), pts[k], reach);
    let after = first_beyond(pts[k + 1..=last].iter().copied(), pts[k], reach);
    before
        .zip(after)
        .and_then(|(b, a)| unit(b - a))
        .or_else(|| unit(pts[k - 1] - pts[k + 1]))
        .or_else(|| unit(pts[k - 1] - pts[k]))
        .unwrap_or(Vec2::new(-1.0, 0.0))
}

/// Fits cubics through `points`; the engine of [`fit_cubics`].
///
/// `start_dir` and `end_dir` are the directions of travel at the first and
/// last point (estimated from the neighbouring points when `None`). For a
/// `closed` run that has no tangents given, they are the same estimate taken
/// across the join.
fn fit_run(
    points: &[Point],
    max_error: f64,
    start_dir: Option<Vec2>,
    end_dir: Option<Vec2>,
    closed: bool,
) -> Vec<CubicBez> {
    if points.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
        return Vec::new();
    }
    let mut pts: Vec<Point> = Vec::with_capacity(points.len() + 1);
    for &p in points {
        if pts
            .last()
            .is_none_or(|last| last.distance(p) > DUPLICATE_EPSILON)
        {
            pts.push(p);
        }
    }
    if closed {
        // A loop may repeat its first point at the end.
        while pts.len() > 1 && pts[0].distance(pts[pts.len() - 1]) <= DUPLICATE_EPSILON {
            pts.pop();
        }
    }
    if pts.len() < 2 {
        return Vec::new();
    }

    // Neighbours closer than the tolerance only add noise to a direction.
    let reach = if max_error.is_finite() && max_error > 0.0 {
        max_error
    } else {
        0.0
    };
    let mut start_dir = start_dir.and_then(unit);
    let mut end_dir = end_dir.and_then(unit);
    if closed && pts.len() >= 3 {
        // The same direction on both sides of the join: from the point before
        // it to the point after it.
        let after = first_beyond(pts[1..].iter().copied(), pts[0], reach);
        let before = first_beyond(pts[1..].iter().rev().copied(), pts[0], reach);
        let join = before
            .zip(after)
            .and_then(|(b, a)| unit(a - b))
            .or_else(|| unit(pts[1] - pts[0]));
        start_dir = start_dir.or(join);
        end_dir = end_dir.or(join);
        pts.push(pts[0]);
    }

    let t1 = start_dir
        .or_else(|| tangent_into(pts.iter().copied(), reach))
        .unwrap_or(Vec2::new(1.0, 0.0));
    let t2 = end_dir
        .map(|d| -d)
        .or_else(|| tangent_into(pts.iter().rev().copied(), reach))
        .unwrap_or(Vec2::new(-1.0, 0.0));
    let n = pts.len();

    // Squared distances are compared; a negative or NaN tolerance accepts
    // nothing, so the fit ends up through every point.
    let tol_sq = if max_error.is_nan() || max_error < 0.0 {
        -1.0
    } else {
        max_error * max_error
    };

    let mut out = Vec::new();
    // Stretches still to fit, the one to emit first on top. Splitting always
    // leaves two strictly shorter stretches, so this ends; an explicit stack
    // keeps a long run from overflowing the call stack.
    let mut work = vec![Job {
        first: 0,
        last: n - 1,
        t1,
        t2,
    }];
    while let Some(job) = work.pop() {
        match fit_job(&pts, &job, tol_sq) {
            Outcome::Fitted(bez) => out.push(bez),
            Outcome::Split(at) => {
                let center = center_tangent(&pts, job.first, at, job.last, reach);
                work.push(Job {
                    first: at,
                    last: job.last,
                    t1: -center,
                    t2: job.t2,
                });
                work.push(Job {
                    first: job.first,
                    last: at,
                    t1: job.t1,
                    t2: center,
                });
            }
        }
    }
    out
}

/// Fits a chain of cubic Beziers through `points`, none of them farther than
/// `max_error` from the curve.
///
/// This is Schneider's algorithm (Graphics Gems, 1990). The first cubic starts
/// at the first point and the last one ends at the last point; the chain is
/// continuous and, where the algorithm had to split, also tangent-continuous.
/// The end tangents come from the neighbouring points. Each stretch is
/// parameterised by chord length and refined four times by Newton-Raphson;
/// the most refined fit within `max_error` is kept, and the stretch is split
/// at the point of largest error if none is. (Stopping at the first fit
/// within a loose `max_error` would keep the chord-length fit, which lies
/// on the inside of an arc: a quarter circle fitted to within a hundredth of
/// its radius strays 0.6 % inward but only 0.25 % outward.)
///
/// With `closed`, the points are a loop: the last cubic ends where the first
/// begins, with the same tangent direction there. A repeated first point at the
/// end is ignored, and fewer than three distinct points are fitted as open.
///
/// Consecutive duplicate points are ignored. The result is empty when fewer than
/// two distinct points remain or a coordinate is not finite. The distance is
/// measured to each point at its fitted parameter, which is never less than
/// the distance to the curve; a `max_error` that is negative or not a number
/// gives a chain through every point.
#[must_use]
pub fn fit_cubics(points: &[Point], max_error: f64, closed: bool) -> Vec<CubicBez> {
    fit_run(points, max_error, None, None, closed)
}

// ------------------------------------------------------------- simplifying

/// The geometry of one segment of a contour.
#[derive(Clone, Copy)]
enum Shape {
    Line,
    Quad(Point),
    Cubic(Point, Point),
}

/// One segment of the contour being simplified.
struct Seg<'a> {
    start: Point,
    end: Point,
    shape: Shape,
    source: &'a Segment<'a>,
}

impl Seg<'_> {
    fn is_curved(&self) -> bool {
        !matches!(self.shape, Shape::Line)
    }

    /// The direction in which the segment leaves its start.
    fn direction_out(&self) -> Option<Vec2> {
        let toward = |p: Point| unit(p - self.start);
        match self.shape {
            Shape::Line => toward(self.end),
            Shape::Quad(c) => toward(c).or_else(|| toward(self.end)),
            Shape::Cubic(c1, c2) => toward(c1)
                .or_else(|| toward(c2))
                .or_else(|| toward(self.end)),
        }
    }

    /// The direction in which the segment arrives at its end.
    fn direction_in(&self) -> Option<Vec2> {
        let from = |p: Point| unit(self.end - p);
        match self.shape {
            Shape::Line => from(self.start),
            Shape::Quad(c) => from(c).or_else(|| from(self.start)),
            Shape::Cubic(c1, c2) => from(c2).or_else(|| from(c1)).or_else(|| from(self.start)),
        }
    }
}

/// A segment of the simplified contour: the off-curve nodes it passes through
/// and the on-curve node it arrives at.
struct OutSeg {
    controls: Vec<Node>,
    end: Node,
}

impl OutSeg {
    /// A segment of the original contour, unchanged.
    fn original(seg: &Seg) -> OutSeg {
        OutSeg {
            controls: seg.source.controls.iter().map(|&n| n.clone()).collect(),
            end: seg.source.end.clone(),
        }
    }
}

/// How many nodes the segments take.
fn node_cost(segs: &[Seg]) -> usize {
    segs.iter().map(|s| 1 + s.source.controls.len()).sum()
}

fn model_node(p: Point, kind: NodeKind, smooth: bool) -> Node {
    Node {
        x: p.x,
        y: p.y,
        kind,
        smooth,
    }
}

/// Turns fitted cubics into segments of the simplified contour; the last one
/// arrives at `end`, the original node of the run's last vertex.
fn cubics_to_segments(cubics: &[CubicBez], end: &Node) -> Vec<OutSeg> {
    let last = cubics.len().saturating_sub(1);
    cubics
        .iter()
        .enumerate()
        .map(|(i, c)| OutSeg {
            controls: vec![
                model_node(c.p1, NodeKind::OffCurve, false),
                model_node(c.p2, NodeKind::OffCurve, false),
            ],
            end: if i == last {
                Node {
                    kind: NodeKind::Curve,
                    ..end.clone()
                }
            } else {
                // The fit is tangent-continuous where it splits.
                model_node(c.p3, NodeKind::Curve, true)
            },
        })
        .collect()
}

/// The points of the run's curves, flattened closely enough that the polyline
/// stays within `tolerance` of them. They lie on the curves, and the run's
/// start and end points are the first and last of them.
fn flatten_run(run: &[Seg], tolerance: f64) -> Vec<Point> {
    let mut bez = BezPath::new();
    if let Some(first) = run.first() {
        bez.move_to(first.start);
    }
    for seg in run {
        match seg.shape {
            Shape::Line => bez.line_to(seg.end),
            Shape::Quad(c) => bez.quad_to(c, seg.end),
            Shape::Cubic(c1, c2) => bez.curve_to(c1, c2, seg.end),
        }
    }
    let mut points = Vec::new();
    kurbo::flatten(bez.elements().iter().copied(), tolerance, |el| {
        if let PathEl::MoveTo(p) | PathEl::LineTo(p) = el {
            points.push(p);
        }
    });
    points
}

/// What a run is allowed to know about the contour around it.
struct RunContext {
    /// The run is the whole closed contour: it ends where it starts.
    is_loop: bool,
    /// The run is a loop whose join is not a corner, so the fitted curve is
    /// tangent-continuous across it.
    smooth_join: bool,
    max_error: f64,
}

/// Replaces a run of segments of one kind (all straight, or all curved) with
/// fewer nodes, or returns `None` if that is not possible within the error.
fn refit_run(run: &[Seg], context: &RunContext, end: &Node) -> Option<Vec<OutSeg>> {
    let (first, last) = (run.first()?, run.last()?);
    if run.len() < 2 {
        return None;
    }
    let fit_tolerance = context.max_error * (1.0 - FLATTEN_SHARE);

    let cubics = if first.is_curved() {
        let points = flatten_run(run, context.max_error * FLATTEN_SHARE);
        let out_dir = first.direction_out();
        let in_dir = last.direction_in();
        if context.smooth_join {
            let join = match (out_dir, in_dir) {
                (Some(a), Some(b)) => unit(a + b).or(Some(a)),
                (a, b) => a.or(b),
            };
            fit_run(&points, fit_tolerance, join, join, true)
        } else {
            fit_run(&points, fit_tolerance, out_dir, in_dir, false)
        }
    } else {
        let points: Vec<Point> = std::iter::once(first.start)
            .chain(run.iter().map(|seg| seg.end))
            .collect();
        if !context.is_loop {
            let chord = Line::new(first.start, last.end);
            let straight = points[1..points.len() - 1]
                .iter()
                .all(|&p| chord.nearest(p, 1e-9).distance_sq <= context.max_error.powi(2));
            if straight {
                return Some(vec![OutSeg {
                    controls: Vec::new(),
                    end: Node {
                        kind: NodeKind::Line,
                        ..end.clone()
                    },
                }]);
            }
        }
        if context.smooth_join {
            fit_run(&points, fit_tolerance, None, None, true)
        } else {
            fit_run(&points, fit_tolerance, None, None, false)
        }
    };

    // Only worth it if it takes fewer nodes than the original.
    if cubics.is_empty() || 3 * cubics.len() >= node_cost(run) {
        return None;
    }
    Some(cubics_to_segments(&cubics, end))
}

fn segment_shape(segment: &Segment) -> Result<Shape, GeometryError> {
    match (segment.kind, segment.controls.as_slice()) {
        (NodeKind::Line, []) => Ok(Shape::Line),
        (NodeKind::QCurve, [c]) => Ok(Shape::Quad(point(c))),
        (NodeKind::Curve, [c1, c2]) => Ok(Shape::Cubic(point(c1), point(c2))),
        _ => Err(GeometryError::InvalidPath(
            "segment does not match the contour convention".into(),
        )),
    }
}

/// Whether the node between `incoming` and `outgoing` is a corner: the
/// tangents differ by more than 3 degrees (or one is undefined), or the node
/// is not smooth and a curve meets it.
fn is_corner(incoming: &Seg, outgoing: &Seg, node: &Node) -> bool {
    let (Some(d_in), Some(d_out)) = (incoming.direction_in(), outgoing.direction_out()) else {
        return true;
    };
    let angle = d_in.dot(d_out).clamp(-1.0, 1.0).acos().to_degrees();
    if angle > CORNER_ANGLE_DEGREES {
        return true;
    }
    // A node the designer left unsmoothed next to a curve is kept; between two
    // straight segments that go on in the same direction it carries nothing.
    !node.smooth && (incoming.is_curved() || outgoing.is_curved())
}

/// Replaces the stretches of `path` between corners with fewer segments.
///
/// A corner is an on-curve node where the incoming and outgoing tangents
/// differ by more than 3 degrees, or a node that is not `smooth` and has a
/// curved segment next to it. Corners are kept exactly, and so are the start
/// node of the contour and its end points when it is open. Where a straight
/// segment meets a curved one the contour is also cut, so straight edges stay
/// straight.
///
/// Each stretch between two such nodes is then simplified. A stretch of
/// straight segments that stays within `max_error` of the line between its end
/// points becomes one `line`. Any other stretch is sampled, fitted with
/// [`fit_cubics`] using its own end tangents, and replaced by the fitted cubics
/// when that takes fewer nodes; otherwise it is left as it was. The nodes
/// where the fit splits are `smooth`. Quadratic segments that are replaced
/// become cubic.
///
/// The result stays within `max_error` of the original contour, give or take
/// the 10% of the budget spent on sampling the curves. The id and the closed
/// flag are kept.
///
/// # Errors
///
/// * [`GeometryError::InvalidPath`] if `max_error` is not a positive finite
///   number, or the contour is malformed (see [`to_bezpath`](crate::to_bezpath)).
/// * [`GeometryError::Degenerate`] if the contour has no nodes, or is closed
///   with fewer than two on-curve nodes.
pub fn simplify_path(path: &Path, max_error: f64) -> Result<Path, GeometryError> {
    if !(max_error.is_finite() && max_error > 0.0) {
        return Err(GeometryError::InvalidPath(format!(
            "max_error must be a positive number, got {max_error}"
        )));
    }
    let outline = outline(path)?;
    let segment_count = outline.segments.len();
    if segment_count == 0 {
        return Ok(path.clone());
    }
    // Vertex `i` is where segment `i - 1` ends and segment `i` starts. A
    // closed contour has one vertex per segment; the last segment returns to
    // vertex 0.
    let vertex_count = if outline.closed {
        segment_count
    } else {
        segment_count + 1
    };
    let vertices: Vec<&Node> = std::iter::once(outline.start)
        .chain(outline.segments.iter().map(|segment| segment.end))
        .take(vertex_count)
        .collect();

    let mut segs: Vec<Seg> = Vec::with_capacity(segment_count);
    for (k, source) in outline.segments.iter().enumerate() {
        segs.push(Seg {
            start: point(vertices[k]),
            end: point(vertices[(k + 1) % vertex_count]),
            shape: segment_shape(source)?,
            source,
        });
    }

    // The segments that meet at each vertex; the ends of an open contour have
    // only one.
    let meeting = |i: usize| -> (Option<usize>, Option<usize>) {
        let incoming = match i.checked_sub(1) {
            Some(k) => Some(k),
            None => outline.closed.then(|| segment_count - 1),
        };
        (incoming, (i < segment_count).then_some(i))
    };
    let corner: Vec<bool> = (0..vertex_count)
        .map(|i| match meeting(i) {
            (Some(a), Some(b)) => is_corner(&segs[a], &segs[b], vertices[i]),
            _ => true,
        })
        .collect();
    // Where the contour has to be cut: corners, changes between straight and
    // curved, the ends of an open contour and, to keep the start point where
    // it is, vertex 0.
    let cut: Vec<usize> = (0..vertex_count)
        .filter(|&i| match meeting(i) {
            (Some(a), Some(b)) => i == 0 || corner[i] || segs[a].is_curved() != segs[b].is_curved(),
            _ => true,
        })
        .collect();

    // The runs of segments between cuts, as ranges of segment indices.
    let mut ranges: Vec<(usize, usize)> = cut.windows(2).map(|w| (w[0], w[1])).collect();
    if outline.closed
        && let Some(&last_cut) = cut.last()
    {
        ranges.push((last_cut, segment_count));
    }

    // Only vertex 0 cuts the contour: one run goes all the way round.
    let is_loop = outline.closed && cut.len() == 1;
    let context = RunContext {
        is_loop,
        smooth_join: is_loop && !corner[0],
        max_error,
    };
    let mut simplified: Vec<OutSeg> = Vec::with_capacity(segment_count);
    let mut changed = false;
    for (from, to) in ranges {
        let run = &segs[from..to];
        let end_node = vertices[to % vertex_count];
        match refit_run(run, &context, end_node) {
            Some(replacement) => {
                changed = true;
                simplified.extend(replacement);
            }
            None => simplified.extend(run.iter().map(OutSeg::original)),
        }
    }
    if !changed {
        return Ok(path.clone());
    }

    let mut nodes: Vec<Node> = Vec::with_capacity(path.nodes.len());
    let mut start = outline.start.clone();
    if outline.closed {
        // The node that starts the contour carries the kind of the segment
        // that closes it, whose off-curve nodes come last.
        if let Some(closing) = simplified.pop() {
            start.kind = closing.end.kind;
            nodes.push(start);
            for seg in simplified {
                nodes.extend(seg.controls);
                nodes.push(seg.end);
            }
            nodes.extend(closing.controls);
        }
    } else {
        nodes.push(start);
        for seg in simplified {
            nodes.extend(seg.controls);
            nodes.push(seg.end);
        }
    }
    Ok(Path {
        id: path.id,
        closed: path.closed,
        nodes,
    })
}
