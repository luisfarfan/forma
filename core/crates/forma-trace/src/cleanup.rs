//! Cleanup of traced contours: from `visioncortex` cubics to clean font
//! contours.
//!
//! Traced splines are noisy in ways a designer's contour is not: straight
//! edges come as cubics with collinear handles; a smooth curve comes as many
//! short cubics with small kinks between them; the steps of hard-edged
//! pixels survive the 4x mask as wiggles a pixel high; sharp corners come
//! slightly rounded; and the start point is wherever the pixel walk began.
//! Each contour is therefore
//!
//! 1. sampled densely ([`SAMPLES_PER_PIXEL`] samples per original pixel);
//! 2. cut at its corners, found at the scale of a few pixels so that pixel
//!    steps are not corners and placed where the edges on both sides meet
//!    ([`corners`]);
//! 3. smoothed between corners without shrinking ([`smooth`]) and refitted
//!    with [`fit_cubics`] (a line where the stretch is straight), with a
//!    tolerance of [`SIMPLIFY_ERROR`] units that grows, up to
//!    [`MAX_FIT_PX`] pixels, with the noise the smoothing removed
//!    ([`tolerance`]): an anti-aliased edge is known to a fraction of a
//!    pixel, a hard-edged one only to half a pixel;
//! 4. split at its extrema, which become cut points (like corners), so the
//!    final simplification keeps nodes exactly there;
//! 5. started at its lowest corner or extremum, so the start node that
//!    [`simplify_path`] always keeps is never a redundant one;
//! 6. simplified with [`simplify_path`] at [`SIMPLIFY_ERROR`] units.
//!
//! [`finish`] then gives the contours their direction, fixes duplicates,
//! missing extrema and the grid, and marks the smooth nodes.

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
/// ...and still turns this fraction of that seen from half as far.
const CORNER_CONCENTRATION: f64 = 0.7;
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

/// A corner of the outline: the sample where it turns and where the edges
/// on both sides meet.
struct Corner {
    at: usize,
    point: Point,
}

/// The corners of the outline. A sample is a corner where the outline turns
/// by [`CORNER_TURN_DEGREES`] seen from `reach` ([`CORNER_SCALE_PX`]) on
/// each side, more than at any sample within `reach`, and the turn is
/// concentrated: seen from half as far it is still
/// [`CORNER_CONCENTRATION`] of that (a curve turns half as much over half
/// the arc; pixel steps do not turn at that scale at all). The corner is put
/// where the edges on both sides meet, since the vectorizer rounds sharp
/// corners slightly.
fn corners(samples: &Samples, reach: f64) -> Vec<Corner> {
    let n = samples.pts.len();
    let turn: Vec<f64> = (0..n).map(|i| samples.turn(i, reach)).collect();
    // How many samples either side the `reach` window spans (samples are
    // roughly evenly spaced).
    let spacing = samples.length / n as f64;
    let window = ((reach / spacing).ceil() as usize).clamp(1, n / 2);
    let mut out = Vec::new();
    for i in 0..n {
        if turn[i] < CORNER_TURN_DEGREES {
            continue;
        }
        let strongest = (1..=window).all(|k| {
            let (before, after) = (turn[(i + n - k) % n], turn[(i + k) % n]);
            before < turn[i] && after <= turn[i]
        });
        if !strongest || samples.turn(i, reach / 2.0) < CORNER_CONCENTRATION * turn[i] {
            continue;
        }
        let s = samples.arc[i];
        let here = samples.pts[i];
        let meet = |near: f64, far: f64| {
            intersect(
                samples.at(s - far),
                samples.at(s - near),
                samples.at(s + near),
                samples.at(s + far),
            )
            .filter(|p| p.distance(here) <= reach)
        };
        let point = meet(reach, 2.0 * reach)
            .or_else(|| meet(reach / 2.0, reach))
            .unwrap_or(here);
        out.push(Corner { at: i, point });
    }
    out
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

/// Fits the smoothed stretch: a line if it stays within `tolerance` of its
/// chord, else cubics; the segments come with their start's smooth flag.
fn fit_stretch(points: &[Point], tolerance: f64, closed: bool) -> Vec<(Seg, bool)> {
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
        return Vec::new();
    };
    if !closed && first.distance(last) > tolerance {
        let chord = Line::new(first, last);
        let straight = points
            .iter()
            .all(|&p| chord.nearest(p, 1e-9).distance_sq <= tolerance * tolerance);
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

/// The refit tolerance of a stretch: [`SIMPLIFY_ERROR`] units for a clean
/// edge (an anti-aliased image pins it down to a fraction of a pixel), more
/// where smoothing moved the samples, as it does on the steps of hard-edged
/// pixels ([`NOISE_FACTOR`] times the median displacement, which a few real
/// features do not sway), up to [`MAX_FIT_PX`] pixels.
fn tolerance(raw: &[Point], smoothed: &[Point], px: f64) -> f64 {
    let mut moved: Vec<f64> = raw
        .iter()
        .zip(smoothed)
        .map(|(a, b)| a.distance(*b))
        .collect();
    if moved.is_empty() {
        return SIMPLIFY_ERROR;
    }
    moved.sort_by(f64::total_cmp);
    let median = moved[moved.len() / 2];
    (NOISE_FACTOR * median)
        .min(MAX_FIT_PX * px)
        .max(SIMPLIFY_ERROR)
}

/// Replaces the traced cubics with a fit that ignores pixel steps: corners
/// are kept, the stretches between them are smoothed and refitted (see
/// [`corners`], [`smooth`], [`fit_stretch`]). `px` is the size of an
/// original pixel in font units.
fn refit(segs: &[Seg], px: f64) -> (Vec<Seg>, Vec<bool>) {
    let samples = sample(segs, px);
    if samples.pts.len() < 3 || samples.length <= EPSILON {
        return (Vec::new(), Vec::new());
    }
    let reach = (CORNER_SCALE_PX * px).min(samples.length / 8.0);
    let sigma = SMOOTH_PX * SAMPLES_PER_PIXEL;
    let corners = corners(&samples, reach);
    let n = samples.pts.len();
    let mut out: Vec<(Seg, bool)> = Vec::new();
    if corners.is_empty() {
        let smoothed = smooth(&samples.pts, sigma, true);
        let tolerance = tolerance(&samples.pts, &smoothed, px);
        out = fit_stretch(&smoothed, tolerance, true);
    } else {
        let m = corners.len();
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
                if walked >= reach && walked <= gap - reach {
                    stretch.push(samples.pts[i]);
                }
            }
            stretch.push(to.point);
            let smoothed = smooth(&stretch, sigma, false);
            let tolerance = tolerance(&stretch, &smoothed, px);
            out.extend(fit_stretch(&smoothed, tolerance, false));
        }
    }
    out.into_iter().unzip()
}

/// The contour of a `visioncortex` spline (`1 + 3n` control points, the last
/// equal to the first) in font units, refitted and simplified, or `None` if
/// it encloses nothing. `px` is the size of an original pixel in font units.
pub(crate) fn contour(points: &[Point], px: f64) -> Result<Option<Path>, TraceError> {
    let mut segs: Vec<Seg> = Vec::new();
    for k in 0..points.len().saturating_sub(1) / 3 {
        let c = CubicBez::new(
            points[3 * k],
            points[3 * k + 1],
            points[3 * k + 2],
            points[3 * k + 3],
        );
        let extent = [c.p1, c.p2, c.p3]
            .iter()
            .map(|p| p.distance(c.p0))
            .fold(0.0, f64::max);
        if extent > EPSILON {
            segs.push(straighten(c));
        }
    }
    let (Some(first), Some(last)) = (segs.first(), segs.last()) else {
        return Ok(None);
    };
    if last.end().distance(first.start()) > EPSILON {
        segs.push(Seg::Line(last.end(), first.start()));
    }
    let (mut segs, smooth) = refit(&segs, px);
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
    simplify_path(&path, SIMPLIFY_ERROR)
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
    for (i, path) in out.iter_mut().enumerate() {
        path.id = PathId(i as u32);
        mark_smooth(path);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The control points of a closed spline of straight cubics through
    /// `corners`, each edge cut in two, starting halfway along the first
    /// edge (as a pixel walk may).
    fn straight_spline(corners: &[Point]) -> Vec<Point> {
        let n = corners.len();
        let mut stops = Vec::new();
        for k in 0..n {
            let (a, b) = (corners[k], corners[(k + 1) % n]);
            stops.push(a.midpoint(b));
            stops.push(b);
        }
        // stops: mid 0, corner 1, mid 1, ..., corner 0; walk from mid 0 back
        // to it.
        let mut points = vec![stops[0]];
        for &p in stops[1..].iter().chain([&stops[0]]) {
            let a = points[points.len() - 1];
            points.extend([a.lerp(p, 1.0 / 3.0), a.lerp(p, 2.0 / 3.0), p]);
        }
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
        let points = straight_spline(&corners);
        assert_ne!(points[0], corners[0], "starts mid-edge");
        let path = contour(&points, 5.0).unwrap().unwrap();
        assert_eq!(path.nodes.len(), 4, "{:?}", path.nodes);
        assert!(path.nodes.iter().all(|n| n.kind == NodeKind::Line));
        // The start moved to the lowest, then leftmost, corner.
        assert_eq!((path.nodes[0].x, path.nodes[0].y), (0.0, 0.0));
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
