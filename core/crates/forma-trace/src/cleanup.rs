//! Cleanup of traced contours: from the midline of a pixel walk (see
//! [`crate::vectorize`]) to clean font contours.
//!
//! Traced outlines are noisy in ways a designer's contour is not: a smooth
//! curve comes as a polygon with small kinks at the steps of the mask; the
//! steps of hard-edged pixels survive the 4x mask as wiggles a pixel high;
//! sharp corners come rounded by the anti-aliasing and the upsampling; and
//! the start point is wherever the pixel walk began.
//! Each contour is therefore
//!
//! 1. sampled densely ([`SAMPLES_PER_PIXEL`] samples per original pixel);
//! 2. cut at its corners, found at the scale of a few pixels so that pixel
//!    steps are not corners, and placed where the edges on both sides meet,
//!    read beyond the rounding the mask gives them (which grows as the
//!    angle gets acute) ([`corners`], [`place`]);
//! 3. smoothed between corners without shrinking ([`smooth`]) and refitted
//!    with [`fit_cubics`] (a line where the stretch is straight), with a
//!    tolerance that grows, up to [`MAX_FIT_PX`] pixels, with the noise the
//!    smoothing removed ([`tolerance`]): an anti-aliased edge is known to a
//!    fraction of a pixel, a hard-edged one only to half a pixel. It never
//!    goes below [`error_floor`]: [`SIMPLIFY_ERROR`] units, or one mask
//!    pixel at large scales. Between two straight edges the corner then
//!    moves to where their least-squares lines meet;
//! 4. split at its extrema, which become cut points (like corners), so the
//!    final simplification keeps nodes exactly there;
//! 5. started at its lowest corner or extremum, so the start node that
//!    [`simplify_path`] always keeps is never a redundant one;
//! 6. simplified with [`simplify_path`] at the same floor.
//!
//! [`finish`] then gives the contours their direction, fixes duplicates,
//! missing extrema and the grid, puts nodes at the leftmost and rightmost
//! points (so the side bearings can be exact) and marks the smooth nodes.

use forma_geometry::{
    IssueKind, fit_cubics, fix_paths, normalize_direction, signed_area, simplify_path,
};
use forma_model::{Node, NodeKind, Path, PathId};
use kurbo::{CubicBez, Line, ParamCurve, ParamCurveExtrema, ParamCurveNearest, Point, Vec2};

use crate::TraceError;

/// The simplification error, in font units (spec §6.1).
const SIMPLIFY_ERROR: f64 = 1.5;
/// The grid, in font units.
const GRID: f64 = 1.0;
/// Tangents that differ by at most this many degrees make a smooth node.
const SMOOTH_DEGREES: f64 = 3.0;
/// A cubic whose handles are within this distance, in font units, of its
/// chord (so the curve is within 0.75 of it) is drawn as a line.
const STRAIGHT_TOLERANCE: f64 = 1.0;
/// A vertex between two lines that is within this distance, in font units,
/// of the line joining its neighbours (with every vertex merged into them
/// before) is dropped.
const LINE_MERGE_TOLERANCE: f64 = 1.0;
/// Samples per original pixel along the traced outline.
const SAMPLES_PER_PIXEL: f64 = 4.0;
/// A corner turns the outline by at least this much...
const CORNER_TURN_DEGREES: f64 = 45.0;
/// ...seen from this many original pixels on each side...
const CORNER_SCALE_PX: f64 = 2.5;
/// ...and still turns this fraction of that seen from half as far, or at
/// most this many times that seen from twice as far.
const CORNER_CONCENTRATION: f64 = 0.7;
const CORNER_SATURATION: f64 = 1.3;
/// A stretch that sags from its chord by at most this many pixels, and at
/// most this fraction of the chord, is a line.
const STRAIGHT_PX: f64 = 0.5;
const STRAIGHT_SAG: f64 = 0.01;
/// Rounds of re-reading a corner's edges beyond its rounding.
const PLACE_ROUNDS: usize = 2;
/// The width (sigma) of the smoothing of the stretches between corners, in
/// original pixels: it flattens the steps of hard-edged pixels.
const SMOOTH_PX: f64 = 1.5;
/// The refit tolerance grows with the noise of the outline: this many times
/// the median displacement of the smoothing...
const NOISE_FACTOR: f64 = 5.0;
/// ...up to this many original pixels (the steps of hard-edged pixels leave
/// the edge uncertain by half a pixel).
const MAX_FIT_PX: f64 = 0.6;
/// An extremum is split off only if the curve goes this far, in font units,
/// past the range of its end points (the threshold of the missing-extrema
/// check).
const EXTREMUM_OVERSHOOT: f64 = 0.5;
/// Extrema this close to a segment end (in curve parameter) are not split.
const EXTREMUM_T_MIN: f64 = 0.01;
const EXTREMUM_T_MAX: f64 = 0.99;
/// How closely a contour is refitted: the profile of one attempt of the
/// trace (see `crate::vectorize`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fit {
    /// The width (sigma) of the smoothing between corners, in original
    /// pixels; 0 does not smooth.
    pub smooth_px: f64,
    /// The largest refit tolerance, in original pixels.
    pub max_fit_px: f64,
    /// How far, in original pixels, a corner may be moved from the traced
    /// edge to where its edges meet (to undo the rounding of anti-aliasing);
    /// 0 keeps corners on the edge. The traced edge is the evidence: a tip
    /// the image draws round (a tapering serif, a ball) is never stretched
    /// into a spike beyond it.
    pub corner_limit_px: f64,
}

impl Fit {
    /// The profile of the Hito 2 tracer: for the stepped edges of the
    /// upsampled mask.
    pub(crate) const STEPPED: Fit = Fit {
        smooth_px: SMOOTH_PX,
        max_fit_px: MAX_FIT_PX,
        corner_limit_px: 2.5,
    };
}

/// Contours enclosing less than this many square font units are noise.
const MIN_AREA: f64 = 1.0;
/// Points closer than this are the same point.
const EPSILON: f64 = 1e-9;

#[derive(Clone, Copy, Debug)]
enum Seg {
    Line(Point, Point),
    Cubic(CubicBez),
}

impl Seg {
    fn start(&self) -> Point {
        match self {
            Seg::Line(a, _) => *a,
            Seg::Cubic(c) => c.p0,
        }
    }

    fn end(&self) -> Point {
        match self {
            Seg::Line(_, b) => *b,
            Seg::Cubic(c) => c.p3,
        }
    }
}

fn unit(v: Vec2) -> Option<Vec2> {
    let length = v.hypot();
    (length.is_finite() && length > EPSILON).then(|| v / length)
}

fn angle_degrees(a: Vec2, b: Vec2) -> f64 {
    a.dot(b).clamp(-1.0, 1.0).acos().to_degrees()
}

/// A line if the cubic's handles lie on its chord, between its ends.
fn straighten(c: CubicBez) -> Seg {
    let chord = Line::new(c.p0, c.p3);
    if chord.length() <= EPSILON {
        return Seg::Cubic(c);
    }
    let on_chord = |p: Point| {
        let nearest = chord.nearest(p, 1e-9);
        nearest.distance_sq.sqrt() <= STRAIGHT_TOLERANCE && nearest.t > 0.0 && nearest.t < 1.0
            || p.distance(c.p0) <= EPSILON
            || p.distance(c.p3) <= EPSILON
    };
    if on_chord(c.p1) && on_chord(c.p2) {
        Seg::Line(c.p0, c.p3)
    } else {
        Seg::Cubic(c)
    }
}

/// Merges consecutive lines whose shared vertex (and every vertex merged
/// into them before) stays within [`LINE_MERGE_TOLERANCE`] of the merged
/// line. `smooth` holds the flag of each segment's start; the merged
/// vertices lose theirs.
fn merge_lines(segs: &mut Vec<Seg>, mut smooth: Vec<bool>) -> Vec<bool> {
    // The original vertices each line stands for, besides its ends.
    let mut covered: Vec<Vec<Point>> = vec![Vec::new(); segs.len()];
    let mut changed = true;
    while changed && segs.len() > 2 {
        changed = false;
        let mut i = 0;
        while i < segs.len() && segs.len() > 2 {
            let j = (i + 1) % segs.len();
            if let (Seg::Line(a, v), Seg::Line(_, b)) = (segs[i], segs[j]) {
                let merged = Line::new(a, b);
                let mut points: Vec<Point> = covered[i].clone();
                points.push(v);
                points.extend(covered[j].iter().copied());
                let fits = merged.length() > EPSILON
                    && points.iter().all(|&p| {
                        let nearest = merged.nearest(p, 1e-9);
                        nearest.distance_sq.sqrt() <= LINE_MERGE_TOLERANCE
                            && nearest.t > 0.0
                            && nearest.t < 1.0
                    });
                if fits {
                    segs[i] = Seg::Line(a, b);
                    covered[i] = points;
                    segs.remove(j);
                    covered.remove(j);
                    smooth.remove(j);
                    changed = true;
                    continue;
                }
            }
            i += 1;
        }
    }
    smooth
}

/// Splits the cubics at the extrema that stick out past their end points;
/// the new vertices are cut points (not smooth).
fn split_extrema(segs: Vec<Seg>, smooth: Vec<bool>) -> (Vec<Seg>, Vec<bool>) {
    let mut out_segs = Vec::with_capacity(segs.len());
    let mut out_smooth = Vec::with_capacity(segs.len());
    for (seg, flag) in segs.into_iter().zip(smooth) {
        let Seg::Cubic(c) = seg else {
            out_segs.push(seg);
            out_smooth.push(flag);
            continue;
        };
        let (lo, hi) = (
            Point::new(c.p0.x.min(c.p3.x), c.p0.y.min(c.p3.y)),
            Point::new(c.p0.x.max(c.p3.x), c.p0.y.max(c.p3.y)),
        );
        let mut cuts: Vec<f64> = c
            .extrema()
            .into_iter()
            .filter(|&t| t > EXTREMUM_T_MIN && t < EXTREMUM_T_MAX)
            .filter(|&t| {
                let p = c.eval(t);
                p.x < lo.x - EXTREMUM_OVERSHOOT
                    || p.x > hi.x + EXTREMUM_OVERSHOOT
                    || p.y < lo.y - EXTREMUM_OVERSHOOT
                    || p.y > hi.y + EXTREMUM_OVERSHOOT
            })
            .collect();
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        // The first piece starts at the segment's own vertex; the others at
        // a new extremum vertex, which is a cut point.
        let mut from = 0.0;
        for t in cuts.into_iter().chain([1.0]) {
            out_segs.push(Seg::Cubic(c.subsegment(from..t)));
            out_smooth.push(if from == 0.0 { flag } else { false });
            from = t;
        }
    }
    (out_segs, out_smooth)
}

/// A closed polyline sampled densely along the traced outline, with the arc
/// length walked to each sample.
struct Samples {
    pts: Vec<Point>,
    arc: Vec<f64>,
    length: f64,
}

impl Samples {
    /// The point at arc length `s` (taken around the loop).
    fn at(&self, s: f64) -> Point {
        let n = self.pts.len();
        let s = s.rem_euclid(self.length);
        let i = self.arc.partition_point(|&a| a <= s).saturating_sub(1);
        let j = (i + 1) % n;
        let next = if j == 0 { self.length } else { self.arc[j] };
        let span = next - self.arc[i];
        if span <= EPSILON {
            return self.pts[i];
        }
        self.pts[i].lerp(self.pts[j], (s - self.arc[i]) / span)
    }

    /// The arc length from sample `i` forward to sample `j`.
    fn ahead(&self, i: usize, j: usize) -> f64 {
        (self.arc[j] - self.arc[i]).rem_euclid(self.length)
    }

    /// How much the outline turns at sample `i`, in degrees, seen from `reach`
    /// before and after it.
    fn turn(&self, i: usize, reach: f64) -> f64 {
        let here = self.pts[i];
        let before = self.at(self.arc[i] - reach);
        let after = self.at(self.arc[i] + reach);
        match (unit(here - before), unit(after - here)) {
            (Some(a), Some(b)) => angle_degrees(a, b),
            _ => 0.0,
        }
    }
}

/// Samples the segments about [`SAMPLES_PER_PIXEL`] times per pixel.
fn sample(segs: &[Seg], px: f64) -> Samples {
    let step = px / SAMPLES_PER_PIXEL;
    let mut pts = Vec::new();
    for seg in segs {
        // The control polygon is at least as long as the curve.
        let reach = match *seg {
            Seg::Line(a, b) => a.distance(b),
            Seg::Cubic(c) => c.p0.distance(c.p1) + c.p1.distance(c.p2) + c.p2.distance(c.p3),
        };
        let n = (reach / step).ceil().clamp(1.0, 1e6) as usize;
        pts.extend((0..n).map(|k| {
            let t = k as f64 / n as f64;
            match *seg {
                Seg::Line(a, b) => a.lerp(b, t),
                Seg::Cubic(c) => c.eval(t),
            }
        }));
    }
    let mut arc = Vec::with_capacity(pts.len());
    let mut walked = 0.0;
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            walked += pts[i - 1].distance(*p);
        }
        arc.push(walked);
    }
    let length = walked + pts.last().map_or(0.0, |l| l.distance(pts[0]));
    Samples { pts, arc, length }
}

/// A corner of the outline: the sample where it turns, where the edges on
/// both sides meet, and how far along the outline on each side its rounding
/// reaches.
struct Corner {
    at: usize,
    point: Point,
    reach: f64,
}

/// The corners of the outline.
///
/// A sample is a corner candidate where the outline turns by
/// [`CORNER_TURN_DEGREES`] seen from `reach` ([`CORNER_SCALE_PX`]) on each
/// side, more than at any sample within `reach`. It is a corner if the turn
/// is concentrated there, as a curve's is not:
///
/// * seen from half as far it still turns [`CORNER_CONCENTRATION`] of that
///   (a curve turns half as much over half the arc), or
/// * seen from twice as far it turns at most [`CORNER_SATURATION`] times
///   that (a curve turns twice as much over twice the arc; a corner, even
///   one the mask rounded, has already turned all it turns).
///
/// Pixel steps do not turn at that scale at all. See [`place`] for where
/// the corner goes.
fn corners(samples: &Samples, reach: f64, limit: f64) -> Vec<Corner> {
    let n = samples.pts.len();
    let turn: Vec<f64> = (0..n).map(|i| samples.turn(i, reach)).collect();
    // How many samples either side the `reach` window spans (samples are
    // roughly evenly spaced).
    let spacing = samples.length / n as f64;
    let window = ((reach / spacing).ceil() as usize).clamp(1, n / 2);
    let mut found = Vec::new();
    for i in 0..n {
        if turn[i] < CORNER_TURN_DEGREES {
            continue;
        }
        let strongest = (1..=window).all(|k| {
            let (before, after) = (turn[(i + n - k) % n], turn[(i + k) % n]);
            before < turn[i] && after <= turn[i]
        });
        if !strongest {
            continue;
        }
        let concentrated = samples.turn(i, reach / 2.0) >= CORNER_CONCENTRATION * turn[i];
        let saturated = samples.turn(i, 2.0 * reach) <= CORNER_SATURATION * turn[i];
        if concentrated || saturated {
            found.push(i);
        }
    }
    let m = found.len();
    (0..m)
        .map(|k| {
            // The rounding must not reach past a third of the way to the
            // neighbouring corners.
            let room = if m == 1 {
                samples.length / 3.0
            } else {
                let before = samples.ahead(found[(k + m - 1) % m], found[k]);
                let after = samples.ahead(found[k], found[(k + 1) % m]);
                before.min(after) / 3.0
            };
            place(samples, found[k], reach, room, limit)
        })
        .collect()
}

/// The distance from `p` to the nearest sample of the traced outline.
fn off_outline(samples: &Samples, p: Point) -> f64 {
    samples
        .pts
        .iter()
        .map(|q| q.distance(p))
        .fold(f64::INFINITY, f64::min)
}

/// Where the corner at sample `i` goes: where the edges on both sides meet,
/// since the mask rounds sharp corners (and blunts acute ones: the
/// rounding of an angle `θ` reaches about `r / tan(θ/2)` along the edges
/// and `r / sin(θ/2)` short of the apex). The edges are read beyond the
/// rounding, from `d` to `2d` away, with `d = reach / tan(θ/2)` (at least
/// `reach`, at most `room`), `θ` estimated from the edges themselves.
fn place(samples: &Samples, i: usize, reach: f64, room: f64, limit: f64) -> Corner {
    let s = samples.arc[i];
    let here = samples.pts[i];
    let edges = |d: f64| {
        (
            (samples.at(s - 2.0 * d), samples.at(s - d)),
            (samples.at(s + d), samples.at(s + 2.0 * d)),
        )
    };
    let span = |interior: f64| {
        let half = (interior / 2.0).to_radians();
        let d = if half > EPSILON {
            reach / half.tan()
        } else {
            room
        };
        d.max(reach).min(room.max(reach / 2.0))
    };
    // The interior angle between the edges read at `d`.
    let interior = |d: f64| {
        let ((a0, a1), (b0, b1)) = edges(d);
        match (unit(a1 - a0), unit(b1 - b0)) {
            (Some(a), Some(b)) => 180.0 - angle_degrees(a, b),
            _ => 90.0,
        }
    };
    let mut d = span(interior(reach));
    for _ in 0..PLACE_ROUNDS {
        d = span(interior(d));
    }
    let ((a0, a1), (b0, b1)) = edges(d);
    let point = intersect(a0, a1, b0, b1)
        .filter(|p| p.distance(here) <= 2.0 * d && off_outline(samples, *p) <= limit);
    match point {
        Some(point) => Corner {
            at: i,
            point,
            reach: d,
        },
        None => Corner {
            at: i,
            point: here,
            reach: reach.min(room.max(reach / 2.0)),
        },
    }
}

/// A refitted stretch between two corners: its segments (with the smooth
/// flag of each start) and, if it is one line, the line that fits its
/// samples best.
struct Stretch {
    segs: Vec<(Seg, bool)>,
    line: Option<(Point, Vec2)>,
}

/// The line that fits `points` best (least squares, perpendicular
/// distances): a point on it and its direction; `None` for fewer than two
/// distinct points.
fn best_line(points: &[Point]) -> Option<(Point, Vec2)> {
    if points.len() < 2 {
        return None;
    }
    let count = points.len() as f64;
    let centre = points.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2()) / count;
    let (mut xx, mut xy, mut yy) = (0.0, 0.0, 0.0);
    for p in points {
        let d = p.to_vec2() - centre;
        xx += d.x * d.x;
        xy += d.x * d.y;
        yy += d.y * d.y;
    }
    // The direction of largest spread: the principal axis.
    let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
    let direction = Vec2::new(angle.cos(), angle.sin());
    (xx + yy > EPSILON).then(|| (centre.to_point(), direction))
}

/// Where the line through `a0` and `a1` meets the line through `b0` and
/// `b1`.
fn intersect(a0: Point, a1: Point, b0: Point, b1: Point) -> Option<Point> {
    let (da, db) = (a1 - a0, b1 - b0);
    let det = da.cross(db);
    if det.abs() <= EPSILON * da.hypot() * db.hypot() {
        return None;
    }
    let t = (b0 - a0).cross(db) / det;
    Some(a0 + da * t)
}

/// Smooths a stretch of samples without shrinking it: `2 G(p) - G(G(p))`
/// ("twicing" a Gaussian `G`), which removes the pixel-scale wiggles like
/// `G` but cancels the pull of `G` towards the centre of curvature to second
/// order, so tight curves keep their size.
fn smooth(points: &[Point], sigma: f64, closed: bool) -> Vec<Point> {
    let once = gaussian(points, sigma, closed);
    let twice = gaussian(&once, sigma, closed);
    once.iter()
        .zip(&twice)
        .map(|(a, b)| (a.to_vec2() * 2.0 - b.to_vec2()).to_point())
        .collect()
}

/// Gaussian smoothing of a stretch of samples. An open stretch keeps its
/// ends (it is mirrored through them), a closed one wraps around.
fn gaussian(points: &[Point], sigma: f64, closed: bool) -> Vec<Point> {
    let n = points.len();
    if n < 3 || sigma <= 0.0 {
        return points.to_vec();
    }
    let radius = (3.0 * sigma).ceil() as i64;
    let weights: Vec<f64> = (-radius..=radius)
        .map(|k| (-(k * k) as f64 / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f64 = weights.iter().sum();
    let last = n as i64 - 1;
    let get = |j: i64| -> Point {
        if closed {
            return points[j.rem_euclid(n as i64) as usize];
        }
        if j < 0 {
            let mirror = points[(-j).min(last) as usize];
            return points[0] + (points[0] - mirror);
        }
        if j > last {
            let mirror = points[(2 * last - j).max(0) as usize];
            return points[n - 1] + (points[n - 1] - mirror);
        }
        points[j as usize]
    };
    (0..n as i64)
        .map(|i| {
            if !closed && (i == 0 || i == last) {
                return points[i as usize];
            }
            let mut acc = Vec2::ZERO;
            for (k, w) in (-radius..=radius).zip(&weights) {
                acc += get(i + k).to_vec2() * *w;
            }
            (acc / total).to_point()
        })
        .collect()
}

/// Fits the smoothed stretch: a line if it stays close to its chord (within
/// `tolerance`, or within [`STRAIGHT_PX`] pixels if that is less than
/// [`STRAIGHT_SAG`] of the chord's length: anti-aliased slanted edges
/// ripple by a fraction of a pixel, and a sag that small is not a curve),
/// else cubics; the segments come with their start's smooth flag.
fn fit_stretch(points: &[Point], tolerance: f64, px: f64, closed: bool) -> Vec<(Seg, bool)> {
    fit_checked(points, tolerance, px, closed, 0)
}

/// How many times a stretch is halved before its points become lines.
const MAX_SPLITS: usize = 8;

/// How far, in multiples of the tolerance (at least one original pixel),
/// a fitted curve may stray from the polyline of its points. A least-squares
/// fit is close to its points but may loop away between them (a cubic
/// fitted to a hairpin); such a fit is refused.
const STRAY_FACTOR: f64 = 3.0;

/// The largest distance from points along `segs` to the polyline `points`.
fn stray(segs: &[(Seg, bool)], points: &[Point]) -> f64 {
    let mut worst = 0.0f64;
    for (seg, _) in segs {
        let Seg::Cubic(c) = seg else { continue };
        for k in 1..16 {
            let p = c.eval(f64::from(k) / 16.0);
            let d = points
                .windows(2)
                .map(|w| {
                    let ab = w[1] - w[0];
                    let len2 = ab.hypot2();
                    if len2 <= EPSILON {
                        return p.distance(w[0]);
                    }
                    let t = ((p - w[0]).dot(ab) / len2).clamp(0.0, 1.0);
                    p.distance(w[0] + ab * t)
                })
                .fold(f64::INFINITY, f64::min);
            worst = worst.max(d);
        }
    }
    worst
}

/// [`fit_stretch`], refusing fits that stray from the points (see
/// [`STRAY_FACTOR`]): the stretch is halved and each half fitted, down to
/// lines through its points.
fn fit_checked(
    points: &[Point],
    tolerance: f64,
    px: f64,
    closed: bool,
    depth: usize,
) -> Vec<(Seg, bool)> {
    let segs = fit_plain(points, tolerance, px, closed);
    let allowed = (STRAY_FACTOR * tolerance).max(px);
    if points.len() < 4 || stray(&segs, points) <= allowed {
        return segs;
    }
    if depth >= MAX_SPLITS {
        // Lines through the points: never far from them.
        let mut lines: Vec<(Seg, bool)> = points
            .windows(2)
            .filter(|w| w[0].distance(w[1]) > EPSILON)
            .map(|w| (Seg::Line(w[0], w[1]), false))
            .collect();
        if closed
            && let (Some(&a), Some(&b)) = (points.last(), points.first())
            && a.distance(b) > EPSILON
        {
            lines.push((Seg::Line(a, b), false));
        }
        return lines;
    }
    // A closed loop is opened at its start; halves are open stretches.
    let mid = points.len() / 2;
    let mut first = fit_checked(&points[..=mid], tolerance, px, false, depth + 1);
    let mut second = fit_checked(&points[mid..], tolerance, px, false, depth + 1);
    if closed
        && let (Some(&a), Some(&b)) = (points.last(), points.first())
        && a.distance(b) > EPSILON
    {
        second.push((Seg::Line(a, b), false));
    }
    first.append(&mut second);
    first
}

/// The fit of [`fit_stretch`], unchecked.
fn fit_plain(points: &[Point], tolerance: f64, px: f64, closed: bool) -> Vec<(Seg, bool)> {
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
        return Vec::new();
    };
    let length = first.distance(last);
    if !closed && length > tolerance {
        let chord = Line::new(first, last);
        let allowed = tolerance.max((STRAIGHT_PX * px).min(STRAIGHT_SAG * length));
        let straight = points
            .iter()
            .all(|&p| chord.nearest(p, 1e-9).distance_sq <= allowed * allowed);
        if straight {
            return vec![(Seg::Line(first, last), false)];
        }
    }
    let cubics = fit_cubics(points, tolerance, closed);
    if cubics.is_empty() {
        return if !closed && first.distance(last) > EPSILON {
            vec![(Seg::Line(first, last), false)]
        } else {
            Vec::new()
        };
    }
    cubics
        .into_iter()
        .enumerate()
        .map(|(k, c)| (straighten(c), closed || k > 0))
        .collect()
}

/// The smallest fitting and simplification error: a quarter of an original
/// pixel, or one mask pixel (`1/upsample` of an original pixel) if that is
/// more, at most [`SIMPLIFY_ERROR`] units and at least a quarter unit.
/// Detail finer than that is not in the image; fitting to it only adds
/// nodes.
fn error_floor(px: f64, upsample: u32) -> f64 {
    // A quarter of an original pixel (or one mask pixel), never more than
    // SIMPLIFY_ERROR units: in a high-resolution image a fixed budget in
    // units would be pixels wide (spec §3.1: budgets are in source pixels).
    (px * 0.25)
        .max(px / f64::from(upsample.max(1)))
        .clamp(0.25, SIMPLIFY_ERROR)
}

/// The refit tolerance of a stretch: `floor` (see [`error_floor`]) for a clean
/// edge (an anti-aliased image pins it down to a fraction of a pixel), more
/// where smoothing moved the samples, as it does on the steps of hard-edged
/// pixels ([`NOISE_FACTOR`] times the median displacement, which a few real
/// features do not sway), up to [`MAX_FIT_PX`] pixels.
fn tolerance(raw: &[Point], smoothed: &[Point], px: f64, floor: f64, max_fit_px: f64) -> f64 {
    let mut moved: Vec<f64> = raw
        .iter()
        .zip(smoothed)
        .map(|(a, b)| a.distance(*b))
        .collect();
    if moved.is_empty() {
        return floor;
    }
    moved.sort_by(f64::total_cmp);
    let median = moved[moved.len() / 2];
    (NOISE_FACTOR * median).min(max_fit_px * px).max(floor)
}

/// Replaces the traced cubics with a fit that ignores pixel steps: corners
/// are kept, the stretches between them are smoothed and refitted (see
/// [`corners`], [`smooth`], [`fit_stretch`]). `px` is the size of an
/// original pixel in font units.
fn refit(segs: &[Seg], px: f64, floor: f64, fit: Fit) -> (Vec<Seg>, Vec<bool>) {
    let samples = sample(segs, px);
    if samples.pts.len() < 3 || samples.length <= EPSILON {
        return (Vec::new(), Vec::new());
    }
    let reach = (CORNER_SCALE_PX * px).min(samples.length / 8.0);
    let sigma = fit.smooth_px * SAMPLES_PER_PIXEL;
    let limit = fit.corner_limit_px * px;
    let corners = corners(&samples, reach, limit);
    let n = samples.pts.len();
    let out: Vec<(Seg, bool)> = if corners.is_empty() {
        let smoothed = smooth(&samples.pts, sigma, true);
        let tolerance = tolerance(&samples.pts, &smoothed, px, floor, fit.max_fit_px);
        fit_stretch(&smoothed, tolerance, px, true)
    } else {
        let m = corners.len();
        let mut fitted: Vec<Stretch> = Vec::with_capacity(m);
        for k in 0..m {
            let (from, to) = (&corners[k], &corners[(k + 1) % m]);
            let mut stretch = vec![from.point];
            // The samples next to a corner belong to its rounding.
            let gap = if m == 1 {
                samples.length
            } else {
                samples.ahead(from.at, to.at)
            };
            let mut i = from.at;
            loop {
                i = (i + 1) % n;
                if i == to.at {
                    break;
                }
                let walked = samples.ahead(from.at, i);
                if walked >= from.reach && walked <= gap - to.reach {
                    stretch.push(samples.pts[i]);
                }
            }
            stretch.push(to.point);
            let smoothed = smooth(&stretch, sigma, false);
            let tolerance = tolerance(&stretch, &smoothed, px, floor, fit.max_fit_px);
            let segs = fit_stretch(&smoothed, tolerance, px, false);
            let line = match segs.as_slice() {
                [(Seg::Line(..), _)] => best_line(&stretch[1..stretch.len() - 1]),
                _ => None,
            };
            fitted.push(Stretch { segs, line });
        }
        // Between two straight edges, the corner is where their best lines
        // meet: the edges then follow all their samples, not just the few
        // read next to each corner.
        if m > 1 {
            for k in 0..m {
                let before = (k + m - 1) % m;
                let (Some((p, u)), Some((q, v))) = (fitted[before].line, fitted[k].line) else {
                    continue;
                };
                let Some(meet) = intersect(p, p + u, q, q + v) else {
                    continue;
                };
                if meet.distance(corners[k].point) > 2.0 * corners[k].reach
                    || off_outline(&samples, meet) > limit
                {
                    continue;
                }
                if let [(Seg::Line(a, _), _)] = fitted[before].segs.as_slice() {
                    fitted[before].segs = vec![(Seg::Line(*a, meet), false)];
                }
                if let [(Seg::Line(_, b), _)] = fitted[k].segs.as_slice() {
                    fitted[k].segs = vec![(Seg::Line(meet, *b), false)];
                }
            }
        }
        fitted
            .into_iter()
            .flat_map(|stretch| stretch.segs)
            .collect()
    };
    out.into_iter().unzip()
}

/// The contour of a traced boundary (a closed polygon, see
/// [`crate::vectorize`]) in font units, refitted and simplified, or `None` if
/// it encloses nothing. `px` is the size of an original pixel in font units
/// and `upsample` the mask's upsampling factor.
pub(crate) fn contour(
    points: &[Point],
    px: f64,
    upsample: u32,
    fit: Fit,
) -> Result<Option<Path>, TraceError> {
    let floor = error_floor(px, upsample);
    let mut segs: Vec<Seg> = points
        .windows(2)
        .filter(|pair| pair[0].distance(pair[1]) > EPSILON)
        .map(|pair| Seg::Line(pair[0], pair[1]))
        .collect();
    let (Some(start), Some(end)) = (segs.first().map(Seg::start), segs.last().map(Seg::end)) else {
        return Ok(None);
    };
    if end.distance(start) > EPSILON {
        segs.push(Seg::Line(end, start));
    }
    let (mut segs, smooth) = refit(&segs, px, floor, fit);
    if segs.len() < 2 {
        return Ok(None);
    }
    let smooth = merge_lines(&mut segs, smooth);
    if segs.len() < 2 {
        return Ok(None);
    }
    let (segs, smooth) = split_extrema(segs, smooth);

    // Start at the lowest (then leftmost) cut point.
    let start = (0..segs.len())
        .filter(|&v| !smooth[v])
        .min_by(|&a, &b| {
            let (pa, pb) = (segs[a].start(), segs[b].start());
            pa.y.total_cmp(&pb.y).then(pa.x.total_cmp(&pb.x))
        })
        .unwrap_or(0);
    let n = segs.len();
    let order: Vec<usize> = (0..n).map(|k| (start + k) % n).collect();

    let node = |p: Point, kind: NodeKind, smooth: bool| Node {
        x: p.x,
        y: p.y,
        kind,
        smooth,
    };
    let kind_of = |seg: &Seg| match seg {
        Seg::Line(..) => NodeKind::Line,
        Seg::Cubic(_) => NodeKind::Curve,
    };
    // The start node first (it carries the kind of the closing segment),
    // then each segment's controls and end, and the closing segment's
    // controls last.
    let closing = &segs[order[n - 1]];
    let mut nodes = vec![node(segs[start].start(), kind_of(closing), smooth[start])];
    for (k, &i) in order.iter().enumerate() {
        if let Seg::Cubic(c) = segs[i] {
            nodes.push(node(c.p1, NodeKind::OffCurve, false));
            nodes.push(node(c.p2, NodeKind::OffCurve, false));
        }
        if k + 1 < n {
            let next = order[k + 1];
            nodes.push(node(segs[i].end(), kind_of(&segs[i]), smooth[next]));
        }
    }
    let path = Path {
        id: PathId(0),
        closed: true,
        nodes,
    };
    if signed_area(&path).abs() < MIN_AREA {
        return Ok(None);
    }
    simplify_path(&path, floor)
        .map(Some)
        .map_err(|e| TraceError::Geometry(e.to_string()))
}

/// Marks as smooth the on-curve nodes next to a curve whose tangents (the
/// directions to the neighbouring nodes) differ by at most
/// [`SMOOTH_DEGREES`]; every other node is not smooth.
fn mark_smooth(path: &mut Path) {
    let nodes = &path.nodes;
    let n = nodes.len();
    let p = |i: usize| Point::new(nodes[i].x, nodes[i].y);
    let flags: Vec<bool> = (0..n)
        .map(|i| {
            if nodes[i].kind == NodeKind::OffCurve {
                return false;
            }
            let here = p(i);
            let d_in = (1..n)
                .map(|k| (i + n - k) % n)
                .find_map(|j| unit(here - p(j)));
            let d_out = (1..n).map(|k| (i + k) % n).find_map(|j| unit(p(j) - here));
            let curved =
                nodes[i].kind != NodeKind::Line || nodes[(i + 1) % n].kind == NodeKind::OffCurve;
            match (d_in, d_out) {
                (Some(a), Some(b)) => curved && angle_degrees(a, b) <= SMOOTH_DEGREES,
                _ => false,
            }
        })
        .collect();
    for (node, flag) in path.nodes.iter_mut().zip(flags) {
        node.smooth = flag;
    }
}

/// Puts a node (on the grid) at every horizontal extremum of a curve that
/// reaches the glyph's left or right edge (`x_lo`, `x_hi`) past its end
/// points, however slightly (the missing-extrema fix leaves overshoots of
/// up to half a unit), so that the bounding box is set by nodes and the side
/// bearings can be exact.
fn pin_side_extremes(path: &mut Path, x_lo: f64, x_hi: f64) {
    let nodes = &path.nodes;
    let n = nodes.len();
    let Some(first) = nodes.iter().position(|n| n.kind != NodeKind::OffCurve) else {
        return;
    };
    let point = |node: &Node| Point::new(node.x, node.y);
    let snapped = |p: Point, kind: NodeKind, smooth: bool| Node {
        x: (p.x / GRID).round() * GRID,
        y: (p.y / GRID).round() * GRID,
        kind,
        smooth,
    };
    let mut out = vec![nodes[first].clone()];
    let mut from = point(&nodes[first]);
    let mut controls: Vec<Node> = Vec::new();
    for k in 1..=n {
        let node = &nodes[(first + k) % n];
        if node.kind == NodeKind::OffCurve {
            controls.push(node.clone());
            continue;
        }
        let closing = k == n;
        let mut split = None;
        if let (NodeKind::Curve, [c1, c2]) = (node.kind, controls.as_slice()) {
            let c = CubicBez::new(from, point(c1), point(c2), point(node));
            let (lo, hi) = (c.p0.x.min(c.p3.x), c.p0.x.max(c.p3.x));
            split = c
                .extrema()
                .into_iter()
                .filter(|&t| t > EXTREMUM_T_MIN && t < EXTREMUM_T_MAX)
                .find(|&t| {
                    let x = c.eval(t).x;
                    (x < lo - EPSILON && x < x_lo + EXTREMUM_OVERSHOOT)
                        || (x > hi + EPSILON && x > x_hi - EXTREMUM_OVERSHOOT)
                })
                .map(|t| (c.subsegment(0.0..t), c.subsegment(t..1.0)));
        }
        match split {
            Some((a, b)) => {
                out.push(snapped(a.p1, NodeKind::OffCurve, false));
                out.push(snapped(a.p2, NodeKind::OffCurve, false));
                out.push(snapped(a.p3, NodeKind::Curve, true));
                out.push(snapped(b.p1, NodeKind::OffCurve, false));
                out.push(snapped(b.p2, NodeKind::OffCurve, false));
            }
            None => out.append(&mut controls),
        }
        controls.clear();
        if !closing {
            out.push(node.clone());
        }
        from = point(node);
    }
    path.nodes = out;
}

/// Gives the contours their PostScript direction, fixes duplicate points,
/// missing extrema and off-grid coordinates, drops contours that enclose
/// nothing, marks the smooth nodes and numbers the contours from 0.
pub(crate) fn finish(paths: Vec<Path>) -> Vec<Path> {
    let mut paths: Vec<Path> = paths
        .into_iter()
        .filter(|p| signed_area(p).abs() >= MIN_AREA)
        .collect();
    normalize_direction(&mut paths);
    let fixed = fix_paths(
        &paths,
        &[
            IssueKind::DuplicatePoints,
            IssueKind::TinySegment,
            IssueKind::MissingExtrema,
            IssueKind::OffGrid,
            IssueKind::WrongDirection,
        ],
        GRID,
    );
    let mut out: Vec<Path> = fixed
        .into_iter()
        .filter(|p| signed_area(p).abs() >= MIN_AREA)
        .collect();
    if let Some(bounds) = forma_geometry::bbox(&out) {
        for path in &mut out {
            pin_side_extremes(path, bounds.x0, bounds.x1);
        }
    }
    for (i, path) in out.iter_mut().enumerate() {
        path.id = PathId(i as u32);
        mark_smooth(path);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed polygon through `corners`, each edge cut in two, starting
    /// halfway along the first edge (as a pixel walk may).
    fn mid_edge_polygon(corners: &[Point]) -> Vec<Point> {
        let n = corners.len();
        let mut points = Vec::new();
        for k in 0..n {
            let (a, b) = (corners[k], corners[(k + 1) % n]);
            points.push(a.midpoint(b));
            points.push(b);
        }
        points.push(points[0]);
        points
    }

    #[test]
    fn a_contour_starting_mid_edge_keeps_only_its_corners() {
        let corners = [
            Point::new(0.0, 0.0),
            Point::new(300.0, 0.0),
            Point::new(300.0, 700.0),
            Point::new(0.0, 700.0),
        ];
        let points = mid_edge_polygon(&corners);
        assert_ne!(points[0], corners[0], "starts mid-edge");
        let path = contour(&points, 5.0, 4, Fit::STEPPED).unwrap().unwrap();
        assert_eq!(path.nodes.len(), 4, "{:?}", path.nodes);
        assert!(path.nodes.iter().all(|n| n.kind == NodeKind::Line));
        // The start moved to the lowest, then leftmost, corner.
        assert!(path.nodes[0].x.abs() < 1e-9 && path.nodes[0].y.abs() < 1e-9);
    }

    #[test]
    fn smoothing_keeps_lines_and_ends() {
        let line: Vec<Point> = (0..50)
            .map(|k| Point::new(f64::from(k), 2.0 * f64::from(k)))
            .collect();
        let smoothed = smooth(&line, 6.0, false);
        for (a, b) in line.iter().zip(&smoothed) {
            assert!(a.distance(*b) < 1e-9);
        }
    }
}
