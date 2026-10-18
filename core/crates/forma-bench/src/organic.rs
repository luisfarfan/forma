//! A variable-width stroker with controlled irregularity, to draw original
//! letters for the L and O cases. Pressure, tremor (edge noise), bulb
//! terminals and gaps are parameters, and the peak displacement injected is
//! known, so the benchmark can check that the engine does not homogenize it.

use kurbo::{BezPath, Point};

use crate::det::{ValueNoise, cos, sin};
use crate::shapes::{circle, oriented, polygon};

/// The distance between samples along a stroke, in pixels.
const STEP: f64 = 1.5;
/// Samples of an ellipse.
const ELLIPSE_SAMPLES: usize = 120;

/// How a stroke is drawn.
#[derive(Clone, Debug)]
pub struct Brush {
    /// Nominal width in pixels.
    pub width: f64,
    /// Pressure: relative width swing (0.3 is plus or minus 30 %).
    pub pressure: f64,
    /// Pressure cycles along a stroke.
    pub pressure_cycles: f64,
    /// Edge noise: peak displacement of each edge, in pixels.
    pub edge_amp: f64,
    /// Edge noise: distance between lattice points, in pixels.
    pub edge_wavelength: f64,
    /// Centerline wobble: peak displacement, in pixels (0 for none).
    pub wobble: f64,
    /// Bulb terminals: radius as a multiple of the half width, and the peak
    /// jitter of the radius in pixels.
    pub bulb: Option<(f64, f64)>,
    /// How far open strokes run past their ends, in pixels.
    pub extend: f64,
    /// The seed of this brush.
    pub seed: u64,
}

impl Brush {
    /// The largest displacement from the ideal outline this brush injects
    /// at any point, in pixels.
    #[must_use]
    pub fn irregularity(&self) -> f64 {
        let bulb = self.bulb.map_or(0.0, |(_, jitter)| jitter);
        self.edge_amp
            .max(self.wobble)
            .max(bulb)
            .max(self.width * self.pressure / 2.0)
    }
}

/// A centerline to stroke.
#[derive(Clone, Debug)]
pub struct Skeleton {
    /// Vertices of the centerline.
    pub points: Vec<Point>,
    /// Whether the line is a closed loop.
    pub closed: bool,
}

/// The centerline strokes of a letter in the box `(x, y, w, h)` (pixels, y
/// down). Letters: `H I L T E O`.
#[must_use]
pub fn letter(ch: char, x: f64, y: f64, w: f64, h: f64) -> Vec<Skeleton> {
    let p = |u: f64, v: f64| Point::new(x + u * w, y + v * h);
    let line = |a: (f64, f64), b: (f64, f64)| Skeleton {
        points: vec![p(a.0, a.1), p(b.0, b.1)],
        closed: false,
    };
    match ch {
        'H' => vec![
            line((0.0, 0.0), (0.0, 1.0)),
            line((1.0, 0.0), (1.0, 1.0)),
            line((0.0, 0.5), (1.0, 0.5)),
        ],
        'I' => vec![line((0.5, 0.0), (0.5, 1.0))],
        'L' => vec![line((0.0, 0.0), (0.0, 1.0)), line((0.0, 1.0), (0.9, 1.0))],
        'T' => vec![line((0.0, 0.0), (1.0, 0.0)), line((0.5, 0.0), (0.5, 1.0))],
        'E' => vec![
            line((0.0, 0.0), (0.0, 1.0)),
            line((0.0, 0.0), (1.0, 0.0)),
            line((0.0, 0.5), (0.8, 0.5)),
            line((0.0, 1.0), (1.0, 1.0)),
        ],
        'O' => vec![ellipse_skeleton(x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0)],
        _ => Vec::new(),
    }
}

/// A closed ellipse centerline.
#[must_use]
pub fn ellipse_skeleton(cx: f64, cy: f64, rx: f64, ry: f64) -> Skeleton {
    let points = (0..ELLIPSE_SAMPLES)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / ELLIPSE_SAMPLES as f64;
            Point::new(cx + rx * cos(a), cy + ry * sin(a))
        })
        .collect();
    Skeleton {
        points,
        closed: true,
    }
}

fn dist(a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    (dx * dx + dy * dy).sqrt()
}

/// The points of a polyline at `STEP` spacing (an open polyline includes both
/// ends; a closed one does not repeat the first point).
fn resample(points: &[Point], closed: bool) -> Vec<Point> {
    let mut pts: Vec<Point> = points.to_vec();
    if closed && let Some(&first) = points.first() {
        pts.push(first);
    }
    let mut out = Vec::new();
    for pair in pts.windows(2) {
        let n = (dist(pair[0], pair[1]) / STEP).ceil().max(1.0) as usize;
        for k in 0..n {
            let t = k as f64 / n as f64;
            out.push(Point::new(
                pair[0].x + (pair[1].x - pair[0].x) * t,
                pair[0].y + (pair[1].y - pair[0].y) * t,
            ));
        }
    }
    if !closed && let Some(&last) = pts.last() {
        out.push(last);
    }
    out
}

/// The part of a skeleton between the fractions `from` and `to` of its length,
/// as an open skeleton (used to cut stencil gaps).
#[must_use]
pub fn slice(skeleton: &Skeleton, from: f64, to: f64) -> Skeleton {
    let mut pts = resample(&skeleton.points, skeleton.closed);
    if skeleton.closed
        && let Some(&first) = pts.first()
    {
        pts.push(first);
    }
    let n = pts.len();
    let a = ((n - 1) as f64 * from).round() as usize;
    let b = (((n - 1) as f64 * to).round() as usize)
        .max(a + 1)
        .min(n - 1);
    Skeleton {
        points: pts[a..=b].to_vec(),
        closed: false,
    }
}

fn normals(pts: &[Point], closed: bool) -> Vec<(f64, f64)> {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = if closed {
                (pts[(i + n - 1) % n], pts[(i + 1) % n])
            } else {
                (pts[i.saturating_sub(1)], pts[(i + 1).min(n - 1)])
            };
            let (tx, ty) = (b.x - a.x, b.y - a.y);
            let len = (tx * tx + ty * ty).sqrt().max(1e-9);
            (-ty / len, tx / len)
        })
        .collect()
}

fn area(points: &[Point]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        / 2.0
}

/// Strokes `skeleton` with `brush`. `salt` makes the noise of each stroke
/// independent. The result is solid pieces (and, for a closed loop, a hole).
#[must_use]
pub fn stroke(skeleton: &Skeleton, brush: &Brush, salt: u64) -> Vec<BezPath> {
    let mut points = skeleton.points.clone();
    if !skeleton.closed && brush.extend > 0.0 && points.len() >= 2 {
        let n = points.len();
        let (a, b) = (points[0], points[1]);
        let l = dist(a, b).max(1e-9);
        points[0] = Point::new(
            a.x - (b.x - a.x) / l * brush.extend,
            a.y - (b.y - a.y) / l * brush.extend,
        );
        let (c, d) = (points[n - 2], points[n - 1]);
        let l = dist(c, d).max(1e-9);
        points[n - 1] = Point::new(
            d.x + (d.x - c.x) / l * brush.extend,
            d.y + (d.y - c.y) / l * brush.extend,
        );
    }
    let pts = resample(&points, skeleton.closed);
    if pts.len() < 2 {
        return Vec::new();
    }
    let mut s = vec![0.0; pts.len()];
    for i in 1..pts.len() {
        s[i] = s[i - 1] + dist(pts[i - 1], pts[i]);
    }
    let total = s[pts.len() - 1].max(1.0);
    let seed = brush.seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let left = ValueNoise::new(seed, brush.edge_amp, brush.edge_wavelength, total);
    let right = ValueNoise::new(seed ^ 0xA5A5, brush.edge_amp, brush.edge_wavelength, total);
    let wob = ValueNoise::new(seed ^ 0x5A5A, brush.wobble, 14.0, total);
    let norm = normals(&pts, skeleton.closed);
    // The wobble moves the centerline along its normal.
    let center: Vec<Point> = pts
        .iter()
        .zip(&norm)
        .zip(&s)
        .map(|((p, n), &si)| {
            let d = if brush.wobble > 0.0 { wob.at(si) } else { 0.0 };
            Point::new(p.x + n.0 * d, p.y + n.1 * d)
        })
        .collect();
    let side = |sign: f64, noise: &ValueNoise| -> Vec<Point> {
        center
            .iter()
            .zip(&norm)
            .zip(&s)
            .map(|((p, n), &si)| {
                let t = si / total;
                let width = brush.width
                    * (1.0
                        + brush.pressure * sin(std::f64::consts::TAU * brush.pressure_cycles * t));
                let d = sign * (width / 2.0 + noise.at(si));
                Point::new(p.x + n.0 * d, p.y + n.1 * d)
            })
            .collect()
    };
    let a = side(1.0, &left);
    let b = side(-1.0, &right);
    let mut out = Vec::new();
    if skeleton.closed {
        let (outer, inner) = if area(&a).abs() >= area(&b).abs() {
            (a, b)
        } else {
            (b, a)
        };
        out.push(polygon(&outer, true));
        out.push(polygon(&inner, false));
    } else {
        let mut ring = a;
        ring.extend(b.into_iter().rev());
        out.push(oriented(polygon(&ring, true), true));
        if let Some((mult, jitter)) = brush.bulb {
            let j = ValueNoise::new(seed ^ 0xB0B0, jitter, 1.0, 4.0);
            let radius = brush.width / 2.0 * mult;
            for (k, end) in [pts[0], pts[pts.len() - 1]].into_iter().enumerate() {
                out.push(circle(end.x, end.y, radius + j.at(k as f64 + 0.5), true));
            }
        }
    }
    out
}
