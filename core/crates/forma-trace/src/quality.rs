//! The quality of a traced glyph against its ink: how far the outline
//! strays from the ink's edge, whether it has the same pieces and holes,
//! and whether a fine feature of the ink (a slit, a thin stroke) was lost
//! or one was invented.
//!
//! The reference is the ink's isoline at the trace level (see
//! [`crate::subpixel`]), in original pixels. Both the reference and the
//! outline are rasterized at up to [`FINE`] times that resolution for the area
//! checks. The gates are conjunctive: a high overlap never makes up for a
//! lost counter or a closed slit (spec §4).

use forma_model::{NodeKind, Path, Transform};
use forma_render::{Mask, coverage_mask};
use kurbo::{BezPath, PathEl, Point, Shape};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::TraceError;
use crate::measure::distance_transform;

/// The raster checks run at up to this many samples per original pixel...
const FINE: f64 = 4.0;
/// ...and over at most this many samples (a large glyph is checked more
/// coarsely rather than with gigabytes of rasters).
const MAX_RASTER_SAMPLES: f64 = 4.0e6;
/// Outlines are flattened, and edges sampled, at this spacing (pixels).
const SAMPLE_PX: f64 = 0.25;
/// A region where the outline and the ink disagree is a lost or invented
/// feature when it is at least as wide as the thresholds say (see
/// [`QualityThresholds::feature_width_px`]) and at least this large
/// (square pixels). Narrower slivers along an edge are the fitting error
/// the Hausdorff gate bounds.
const FEATURE_AREA_PX2: f64 = 2.0;
/// Most features listed per glyph.
const MAX_LISTED: usize = 8;

/// The quality gates, versioned: every report and provenance records the
/// version it was judged with (spec §4; values are experimental until
/// calibrated, §8.5).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QualityThresholds {
    /// The version of these values.
    pub version: u32,
    /// Lowest overlap with the ink mask.
    pub iou_min: f64,
    /// Highest 95th percentile distance between outline and ink edge, in
    /// image pixels.
    pub hausdorff_p95_px: f64,
    /// Highest distance between outline and ink edge anywhere, in image
    /// pixels: a spike or a dent this far off is a defect however small its
    /// area.
    pub hausdorff_max_px: f64,
    /// Highest angle, in degrees, between the tangents at a smooth node.
    pub g1_degrees: f64,
    /// Narrowest disagreement between outline and ink that counts as a
    /// lost or invented feature, in image pixels, for anti-aliased edges...
    pub feature_width_px: f64,
    /// ...and for hard-edged ink, whose pixel steps are noise half a pixel
    /// high on each side (a feature narrower than this cannot be told from
    /// a step).
    pub feature_width_hard_px: f64,
    /// Highest 99th percentile distance between outline and ink edge, in
    /// image pixels, with `fidelity: faithful` (spec §4: the fit
    /// deviation)...
    pub fit_faithful_px: f64,
    /// ...with `fidelity: clean`, and for hard-edged ink (whose steps are
    /// noise half a pixel high).
    pub fit_clean_px: f64,
    /// Lowest overlap between a counter of the ink and its counter in the
    /// outline.
    pub hole_iou_min: f64,
}

/// The thresholds in force.
pub const THRESHOLDS: QualityThresholds = QualityThresholds {
    version: 1,
    iou_min: 0.97,
    hausdorff_p95_px: 0.75,
    hausdorff_max_px: 2.0,
    g1_degrees: 3.0,
    feature_width_px: 0.8,
    feature_width_hard_px: 1.5,
    fit_faithful_px: 0.5,
    fit_clean_px: 1.0,
    hole_iou_min: 0.5,
};

/// What a glyph is judged with besides the thresholds: the fit budget of
/// its fidelity, the narrowest feature that counts, and the part of a pixel
/// the font's unit grid cannot resolve (half a unit: coordinates are
/// rounded to whole units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Allowance {
    pub fit_px: f64,
    pub min_feature_px: f64,
    pub grid_px: f64,
}

/// A fine feature on which the outline and the ink disagree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FeatureIssue {
    /// `filled`: background of the ink (a slit, a notch, a counter) that
    /// the outline covers; `lost`: ink (a thin stroke, a spur) that the
    /// outline leaves out.
    pub kind: String,
    /// Its width, in image pixels.
    pub width_px: f64,
    /// Its area, in square image pixels.
    pub area_px2: f64,
    /// Where it is: its centre, in image pixels.
    pub at_px: [f64; 2],
}

/// How faithful a traced outline is to its ink.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QualityReport {
    /// Overlap of the outline with the ink mask (1 is perfect).
    pub iou_mask: f64,
    /// 95th percentile distance between the outline and the ink's edge,
    /// both ways, in image pixels.
    pub hausdorff_p95_px: f64,
    /// 99th percentile of that distance: the fit deviation (spec §4).
    pub fit_deviation_px: f64,
    /// The fit deviation allowed for this glyph: the fidelity's budget plus
    /// what the font's unit grid cannot resolve.
    pub fit_budget_px: f64,
    /// What the font's unit grid cannot resolve, in image pixels (half a
    /// unit), added to the distance thresholds.
    pub grid_px: f64,
    /// Largest such distance.
    pub hausdorff_max_px: f64,
    /// Separate pieces of the ink and of the outline.
    pub components: [usize; 2],
    /// Holes (counters) of the ink and of the outline.
    pub holes: [usize; 2],
    /// Whether pieces and holes match, each counter of the ink with one of
    /// the outline.
    pub topology_ok: bool,
    /// Fine features filled or lost (at most the first 8).
    pub features: Vec<FeatureIssue>,
    /// Smooth nodes whose tangents differ by more than the threshold.
    pub g1_violations: usize,
    /// Places where the outline crosses itself or another of its contours
    /// (a loop a non-zero fill hides, but an editor shows).
    pub crossings: usize,
    /// On-curve nodes per 1000 units of outline.
    pub nodes_per_1000: f64,
}

impl QualityReport {
    /// The gates this report fails, as readable reasons; empty when it
    /// passes every one.
    #[must_use]
    pub fn failures(&self, t: &QualityThresholds) -> Vec<String> {
        let mut out = Vec::new();
        if !self.topology_ok {
            let [ink_holes, out_holes] = self.holes;
            let [ink_parts, out_parts] = self.components;
            if ink_holes != out_holes {
                out.push(format!(
                    "the ink has {ink_holes} counter(s), the outline {out_holes}"
                ));
            } else if ink_parts != out_parts {
                out.push(format!(
                    "the ink has {ink_parts} piece(s), the outline {out_parts}"
                ));
            } else {
                out.push("a counter of the ink has no match in the outline".to_owned());
            }
        }
        for f in &self.features {
            let what = if f.kind == "filled" {
                "slit or notch closed"
            } else {
                "thin stroke lost"
            };
            out.push(format!(
                "{what} ({:.1} px wide) at ({:.0}, {:.0})",
                f.width_px, f.at_px[0], f.at_px[1]
            ));
        }
        if self.hausdorff_max_px >= NO_EDGE {
            out.push("no edge of the ink at this level, or no outline".to_owned());
            return out;
        }
        let max_allowed = t.hausdorff_max_px + 2.0 * self.grid_px;
        if self.hausdorff_max_px > max_allowed {
            out.push(format!(
                "the outline strays {:.1} px from the ink at one spot (at most {:.1})",
                self.hausdorff_max_px, max_allowed
            ));
        }
        let p95_allowed = t.hausdorff_p95_px + self.grid_px;
        if self.hausdorff_p95_px > p95_allowed {
            out.push(format!(
                "the outline strays {:.2} px from the ink (95th percentile; at most {:.2})",
                self.hausdorff_p95_px, p95_allowed
            ));
        }
        if self.fit_deviation_px > self.fit_budget_px {
            out.push(format!(
                "the outline deviates {:.2} px from the ink (99th percentile; the fidelity \
                 allows {:.2})",
                self.fit_deviation_px, self.fit_budget_px
            ));
        }
        if self.iou_mask < t.iou_min {
            out.push(format!(
                "overlap {:.3} below {:.3}",
                self.iou_mask, t.iou_min
            ));
        }
        if self.crossings > 0 {
            out.push(format!(
                "the outline crosses itself {} time(s)",
                self.crossings
            ));
        }
        if self.g1_violations > 0 {
            out.push(format!("{} smooth node(s) with a kink", self.g1_violations));
        }
        out
    }
}

/// How font units map to image pixels: `px = (x - lsb) / scale + origin.x`,
/// `py = origin.y - y / scale`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ToPixels {
    pub scale: f64,
    pub lsb: f64,
    pub origin: (f64, f64),
}

impl ToPixels {
    fn point(&self, x: f64, y: f64) -> Point {
        Point::new(
            (x - self.lsb) / self.scale + self.origin.0,
            self.origin.1 - y / self.scale,
        )
    }
}

/// The outline as polygons in image pixels, flattened.
fn outline_px(paths: &[Path], to: &ToPixels) -> Result<Vec<Vec<Point>>, TraceError> {
    let mut out = Vec::new();
    for path in paths {
        let bez: BezPath =
            forma_geometry::to_bezpath(path).map_err(|e| TraceError::Geometry(e.to_string()))?;
        let mut ring: Vec<Point> = Vec::new();
        kurbo::flatten(bez.iter(), SAMPLE_PX * to.scale * 0.25, |el| match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => ring.push(to.point(p.x, p.y)),
            PathEl::ClosePath => {
                if let Some(&first) = ring.first() {
                    ring.push(first);
                }
                if ring.len() >= 4 {
                    out.push(std::mem::take(&mut ring));
                } else {
                    ring.clear();
                }
            }
            _ => {}
        });
    }
    Ok(out)
}

/// Points along polygons every [`SAMPLE_PX`].
fn samples(rings: &[Vec<Point>]) -> Vec<Point> {
    let mut out = Vec::new();
    for ring in rings {
        for pair in ring.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let n = (a.distance(b) / SAMPLE_PX).ceil().max(1.0) as usize;
            for k in 0..n {
                out.push(a.lerp(b, k as f64 / n as f64));
            }
        }
    }
    out
}

/// Segments of polygons bucketed on a grid, for nearest-distance queries.
struct Segments {
    cell: f64,
    buckets: std::collections::HashMap<(i64, i64), Vec<(Point, Point)>>,
    /// The range of occupied cells: `(min_x, min_y, max_x, max_y)`.
    bounds: Option<(i64, i64, i64, i64)>,
}

impl Segments {
    fn new(rings: &[Vec<Point>]) -> Segments {
        let cell = 2.0;
        let mut buckets: std::collections::HashMap<(i64, i64), Vec<(Point, Point)>> =
            std::collections::HashMap::new();
        let mut bounds: Option<(i64, i64, i64, i64)> = None;
        for ring in rings {
            for pair in ring.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let (cx0, cx1) = (
                    (a.x.min(b.x) / cell).floor() as i64,
                    (a.x.max(b.x) / cell).floor() as i64,
                );
                let (cy0, cy1) = (
                    (a.y.min(b.y) / cell).floor() as i64,
                    (a.y.max(b.y) / cell).floor() as i64,
                );
                bounds = Some(match bounds {
                    None => (cx0, cy0, cx1, cy1),
                    Some((a0, b0, a1, b1)) => (a0.min(cx0), b0.min(cy0), a1.max(cx1), b1.max(cy1)),
                });
                for cx in cx0..=cx1 {
                    for cy in cy0..=cy1 {
                        buckets.entry((cx, cy)).or_default().push((a, b));
                    }
                }
            }
        }
        Segments {
            cell,
            buckets,
            bounds,
        }
    }

    /// The distance from `p` to the nearest segment, searched in growing
    /// rings of cells (only the cells on each ring), never beyond the
    /// occupied cells; `None` without segments.
    fn distance(&self, p: Point) -> Option<f64> {
        let (bx0, by0, bx1, by1) = self.bounds?;
        let (cx, cy) = (
            (p.x / self.cell).floor() as i64,
            (p.y / self.cell).floor() as i64,
        );
        let reach = (cx - bx0)
            .abs()
            .max((cx - bx1).abs())
            .max((cy - by0).abs())
            .max((cy - by1).abs());
        let mut best = f64::INFINITY;
        let visit = |x: i64, y: i64, best: &mut f64| {
            if let Some(segs) = self.buckets.get(&(x, y)) {
                for &(a, b) in segs {
                    *best = best.min(segment_distance(p, a, b));
                }
            }
        };
        for r in 0..=reach {
            if r == 0 {
                visit(cx, cy, &mut best);
            } else {
                for d in -r..=r {
                    visit(cx + d, cy - r, &mut best);
                    visit(cx + d, cy + r, &mut best);
                }
                for d in (-r + 1)..r {
                    visit(cx - r, cy + d, &mut best);
                    visit(cx + r, cy + d, &mut best);
                }
            }
            // Everything in further rings is at least r cells away.
            if best <= (r as f64) * self.cell {
                break;
            }
        }
        Some(best)
    }
}

fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    if len2 <= 1e-18 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// The distance reported when one side has no edge at all.
const NO_EDGE: f64 = 1.0e6;

/// The 95th and 99th percentiles and the maximum of the distances between
/// two sets of polygons, both ways; [`NO_EDGE`] when one set is empty and
/// the other is not.
fn hausdorff(a: &[Vec<Point>], b: &[Vec<Point>]) -> (f64, f64, f64) {
    let (sa, sb) = (Segments::new(a), Segments::new(b));
    let (pa, pb) = (samples(a), samples(b));
    if pa.is_empty() && pb.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    if pa.is_empty() || pb.is_empty() {
        return (NO_EDGE, NO_EDGE, NO_EDGE);
    }
    let mut d: Vec<f64> = pa
        .into_iter()
        .map(|p| sb.distance(p).unwrap_or(NO_EDGE))
        .collect();
    d.extend(pb.into_iter().map(|p| sa.distance(p).unwrap_or(NO_EDGE)));
    d.sort_by(f64::total_cmp);
    let at = |q: f64| d[((d.len() - 1) as f64 * q).round() as usize];
    (at(0.95), at(0.99), d[d.len() - 1])
}

/// Polygons in image pixels as closed model paths of lines.
fn as_paths(rings: &[Vec<Point>]) -> Vec<Path> {
    rings
        .iter()
        .enumerate()
        .map(|(k, ring)| Path {
            id: forma_model::PathId(k as u32),
            closed: true,
            nodes: ring[..ring.len().saturating_sub(1)]
                .iter()
                .map(|p| forma_model::Node {
                    x: p.x,
                    y: p.y,
                    kind: NodeKind::Line,
                    smooth: false,
                })
                .collect(),
        })
        .collect()
}

/// The pixel window `[x0, y0, x1, y1)` that holds every polygon, one pixel
/// of margin around.
fn window(rings: &[&[Vec<Point>]]) -> (i64, i64, i64, i64) {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for set in rings {
        for ring in set.iter() {
            for p in ring {
                x0 = x0.min(p.x);
                y0 = y0.min(p.y);
                x1 = x1.max(p.x);
                y1 = y1.max(p.y);
            }
        }
    }
    if x0 > x1 {
        return (0, 0, 1, 1);
    }
    (
        x0.floor() as i64 - 1,
        y0.floor() as i64 - 1,
        x1.ceil() as i64 + 1,
        y1.ceil() as i64 + 1,
    )
}

/// The samples per pixel of the raster checks over `win`: [`FINE`], fewer
/// for a large window (at most [`MAX_RASTER_SAMPLES`] in all).
fn fineness(win: (i64, i64, i64, i64)) -> f64 {
    let area = ((win.2 - win.0) * (win.3 - win.1)).max(1) as f64;
    (MAX_RASTER_SAMPLES / area).sqrt().min(FINE)
}

/// The raster size of `win` at `f` samples per pixel.
fn raster_size(win: (i64, i64, i64, i64), f: f64) -> (usize, usize) {
    (
        ((win.2 - win.0) as f64 * f).ceil().max(1.0) as usize,
        ((win.3 - win.1) as f64 * f).ceil().max(1.0) as usize,
    )
}

/// Rasterizes polygons (image pixels, non-zero fill) in `win` at `f`
/// samples per pixel.
fn raster(
    rings: &[Vec<Point>],
    win: (i64, i64, i64, i64),
    f: f64,
) -> Result<Vec<bool>, TraceError> {
    let (x0, y0, _, _) = win;
    let (w, h) = raster_size(win, f);
    let too_large = || TraceError::Render("the quality raster is too large".into());
    let (w, h) = (
        u32::try_from(w).map_err(|_| too_large())?,
        u32::try_from(h).map_err(|_| too_large())?,
    );
    let t = Transform {
        xx: f,
        xy: 0.0,
        yx: 0.0,
        yy: f,
        dx: -(x0 as f64) * f,
        dy: -(y0 as f64) * f,
    };
    let mask: Mask = coverage_mask(&as_paths(rings), &t, w, h).map_err(|e| {
        TraceError::Render(format!(
            "the quality raster ({w} x {h} samples) failed: {e}"
        ))
    })?;
    Ok(mask.data.iter().map(|&v| v >= 128).collect())
}

/// Labels the 8-connected (`eight`) or 4-connected regions where `pick`
/// holds; returns the labels (0 = none) and each region's pixel count and
/// whether it touches the border.
fn regions(
    pick: &dyn Fn(usize) -> bool,
    w: usize,
    h: usize,
    eight: bool,
) -> (Vec<u32>, Vec<(usize, bool)>) {
    let mut labels = vec![0u32; w * h];
    let mut info = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if labels[start] != 0 || !pick(start) {
            continue;
        }
        let label = info.len() as u32 + 1;
        labels[start] = label;
        stack.push(start);
        let (mut count, mut border) = (0usize, false);
        while let Some(i) = stack.pop() {
            count += 1;
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            if x == 0 || y == 0 || x as usize == w - 1 || y as usize == h - 1 {
                border = true;
            }
            for (dx, dy) in [
                (-1, 0),
                (1, 0),
                (0, -1),
                (0, 1),
                (-1, -1),
                (1, -1),
                (-1, 1),
                (1, 1),
            ]
            .into_iter()
            .take(if eight { 8 } else { 4 })
            {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if labels[j] == 0 && pick(j) {
                    labels[j] = label;
                    stack.push(j);
                }
            }
        }
        info.push((count, border));
    }
    (labels, info)
}

/// Pieces (8-connected ink) and holes (4-connected background not reaching
/// the border) of a raster, with the hole labels.
fn topology(ink: &[bool], w: usize, h: usize) -> (usize, Vec<u32>, Vec<(usize, bool)>) {
    let (_, pieces) = regions(&|i| ink[i], w, h, true);
    let (labels, bg) = regions(&|i| !ink[i], w, h, false);
    (pieces.len(), labels, bg)
}

/// The disagreement regions of `pick` wide and large enough to be a
/// feature, as issues of `kind`.
fn features(
    pick: &dyn Fn(usize) -> bool,
    (w, h): (usize, usize),
    origin: (i64, i64),
    f: f64,
    min_width_px: f64,
    kind: &str,
) -> Vec<FeatureIssue> {
    let (x0, y0) = origin;
    let inside: Vec<bool> = (0..w * h).map(pick).collect();
    if !inside.iter().any(|&v| v) {
        return Vec::new();
    }
    let edt = distance_transform(&inside, w, h);
    let (labels, info) = regions(&|i| inside[i], w, h, true);
    let mut widest = vec![0.0f64; info.len()];
    let mut centre = vec![(0.0f64, 0.0f64); info.len()];
    for (i, &label) in labels.iter().enumerate() {
        if label == 0 {
            continue;
        }
        let k = label as usize - 1;
        widest[k] = widest[k].max(edt[i].sqrt());
        centre[k].0 += (i % w) as f64;
        centre[k].1 += (i / w) as f64;
    }
    info.iter()
        .enumerate()
        .filter_map(|(k, &(count, _))| {
            let width_px = 2.0 * widest[k] / f;
            let area_px2 = count as f64 / (f * f);
            (width_px >= min_width_px && area_px2 >= FEATURE_AREA_PX2).then(|| FeatureIssue {
                kind: kind.to_owned(),
                width_px: (width_px * 100.0).round() / 100.0,
                area_px2: (area_px2 * 10.0).round() / 10.0,
                at_px: [
                    x0 as f64 + centre[k].0 / count as f64 / f,
                    y0 as f64 + centre[k].1 / count as f64 / f,
                ],
            })
        })
        .collect()
}

/// Whether segments `a0-a1` and `b0-b1` cross properly (each strictly
/// through the other).
fn cross(a0: Point, a1: Point, b0: Point, b1: Point) -> bool {
    let side = |p: Point, q: Point, r: Point| (q - p).cross(r - p);
    let (d1, d2) = (side(b0, b1, a0), side(b0, b1, a1));
    let (d3, d4) = (side(a0, a1, b0), side(a0, a1, b1));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// The proper crossings between the segments of the flattened outline
/// (within a contour, non-adjacent segments; between contours, any), found
/// with a grid of buckets.
fn crossings(rings: &[Vec<Point>]) -> usize {
    let cell = 2.0;
    let mut segs: Vec<(usize, usize, Point, Point)> = Vec::new();
    for (r, ring) in rings.iter().enumerate() {
        for (k, pair) in ring.windows(2).enumerate() {
            if pair[0].distance(pair[1]) > 1e-12 {
                segs.push((r, k, pair[0], pair[1]));
            }
        }
    }
    let mut buckets: std::collections::HashMap<(i64, i64), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, &(_, _, a, b)) in segs.iter().enumerate() {
        for cx in (a.x.min(b.x) / cell).floor() as i64..=(a.x.max(b.x) / cell).floor() as i64 {
            for cy in (a.y.min(b.y) / cell).floor() as i64..=(a.y.max(b.y) / cell).floor() as i64 {
                buckets.entry((cx, cy)).or_default().push(i);
            }
        }
    }
    let mut found = std::collections::BTreeSet::new();
    for list in buckets.values() {
        for (x, &i) in list.iter().enumerate() {
            for &j in &list[x + 1..] {
                let (ri, ki, a0, a1) = segs[i];
                let (rj, kj, b0, b1) = segs[j];
                if ri == rj {
                    let n = rings[ri].len() - 1;
                    let adjacent = ki.abs_diff(kj) <= 1 || ki.abs_diff(kj) == n - 1;
                    if adjacent {
                        continue;
                    }
                }
                if cross(a0, a1, b0, b1) {
                    found.insert((i.min(j), i.max(j)));
                }
            }
        }
    }
    found.len()
}

/// G1 violations: smooth on-curve nodes whose incoming and outgoing
/// directions differ by more than `degrees`.
fn g1_violations(paths: &[Path], degrees: f64) -> usize {
    let mut count = 0;
    for path in paths {
        let nodes = &path.nodes;
        let n = nodes.len();
        for i in 0..n {
            if !nodes[i].smooth || nodes[i].kind == NodeKind::OffCurve {
                continue;
            }
            let here = Point::new(nodes[i].x, nodes[i].y);
            let dir = |j: usize| Point::new(nodes[j].x, nodes[j].y);
            let d_in = (1..n)
                .map(|k| dir((i + n - k) % n))
                .find(|p| p.distance(here) > 1e-9);
            let d_out = (1..n)
                .map(|k| dir((i + k) % n))
                .find(|p| p.distance(here) > 1e-9);
            if let (Some(a), Some(b)) = (d_in, d_out) {
                let (u, v) = (here - a, b - here);
                let cos = u.dot(v) / (u.hypot() * v.hypot());
                if cos.clamp(-1.0, 1.0).acos().to_degrees() > degrees {
                    count += 1;
                }
            }
        }
    }
    count
}

/// On-curve nodes per 1000 units of perimeter.
fn node_density(paths: &[Path]) -> f64 {
    let mut perimeter = 0.0;
    let mut nodes = 0usize;
    for path in paths {
        nodes += path
            .nodes
            .iter()
            .filter(|n| n.kind != NodeKind::OffCurve)
            .count();
        if let Ok(bez) = forma_geometry::to_bezpath(path) {
            perimeter += bez.perimeter(0.5);
        }
    }
    if perimeter <= 0.0 {
        0.0
    } else {
        nodes as f64 * 1000.0 / perimeter
    }
}

/// The quality of `paths` (font units, placed by `to`) against `reference`
/// (the ink's isoline polygons in image pixels, at the trace level). The
/// overlap is measured between the two rasterized, so that it follows the
/// level traced.
pub(crate) fn evaluate(
    reference: &[Vec<Point>],
    paths: &[Path],
    to: &ToPixels,
    thresholds: &QualityThresholds,
    allowance: Allowance,
) -> Result<QualityReport, TraceError> {
    let outline = outline_px(paths, to)?;
    let (p95, p99, max) = hausdorff(reference, &outline);
    let win = window(&[reference, &outline]);
    let f = fineness(win);
    let (w, h) = raster_size(win, f);
    let ink = raster(reference, win, f)?;
    let out = raster(&outline, win, f)?;
    let (both, either) = ink
        .iter()
        .zip(&out)
        .fold((0usize, 0usize), |(i, u), (&a, &b)| {
            (i + usize::from(a && b), u + usize::from(a || b))
        });
    let iou = if either == 0 {
        1.0
    } else {
        both as f64 / either as f64
    };
    let (ink_parts, ink_holes, ink_bg) = topology(&ink, w, h);
    let (out_parts, out_holes, out_bg) = topology(&out, w, h);
    let holes_of = |bg: &[(usize, bool)]| bg.iter().filter(|(_, border)| !border).count();
    let (n_ink_holes, n_out_holes) = (holes_of(&ink_bg), holes_of(&out_bg));
    // Each counter of the ink must overlap one counter of the outline with
    // an IoU of at least `hole_iou_min`.
    let matched = ink_bg.iter().enumerate().all(|(k, &(count, border))| {
        if border {
            return true;
        }
        let label = k as u32 + 1;
        let mut overlap: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        for (i, &l) in ink_holes.iter().enumerate() {
            if l == label && out_holes[i] != 0 && !out_bg[out_holes[i] as usize - 1].1 {
                *overlap.entry(out_holes[i]).or_default() += 1;
            }
        }
        overlap.iter().any(|(&o, &shared)| {
            let other = out_bg[o as usize - 1].0;
            shared as f64 / (count + other - shared).max(1) as f64 >= thresholds.hole_iou_min
        })
    });
    let topology_ok = ink_parts == out_parts && n_ink_holes == n_out_holes && matched;
    let origin = (win.0, win.1);
    let size = (w, h);
    let min = allowance.min_feature_px;
    let mut found = features(&|i| !ink[i] && out[i], size, origin, f, min, "filled");
    found.extend(features(
        &|i| ink[i] && !out[i],
        size,
        origin,
        f,
        min,
        "lost",
    ));
    found.sort_by(|a, b| b.area_px2.total_cmp(&a.area_px2));
    found.truncate(MAX_LISTED);
    let round = |v: f64| (v * 1000.0).round() / 1000.0;
    Ok(QualityReport {
        iou_mask: iou,
        hausdorff_p95_px: round(p95),
        fit_deviation_px: round(p99),
        fit_budget_px: round(allowance.fit_px + allowance.grid_px),
        grid_px: round(allowance.grid_px),
        hausdorff_max_px: round(max),
        components: [ink_parts, out_parts],
        holes: [n_ink_holes, n_out_holes],
        topology_ok,
        features: found,
        g1_violations: g1_violations(paths, thresholds.g1_degrees),
        crossings: crossings(&outline),
        nodes_per_1000: (node_density(paths) * 10.0).round() / 10.0,
    })
}

/// Counters that only show at a stricter level: background regions closed
/// off from the outside at `stricter` but not at `reference`, at least
/// [`FEATURE_AREA_PX2`] large (a counter that the image's blur or the ink's
/// bleed filled). They call for a look: the drawing may have one more
/// counter than the image shows at the trace level.
pub(crate) fn faint_counters(
    reference: &[Vec<Point>],
    stricter: &[Vec<Point>],
) -> Result<Vec<FeatureIssue>, TraceError> {
    let win = window(&[reference, stricter]);
    let f = fineness(win);
    let (w, h) = raster_size(win, f);
    let ink = raster(reference, win, f)?;
    let strict = raster(stricter, win, f)?;
    let (_, ink_labels, ink_bg) = topology(&ink, w, h);
    let (_, labels, bg) = topology(&strict, w, h);
    let mut found = Vec::new();
    for (k, &(count, border)) in bg.iter().enumerate() {
        let area_px2 = count as f64 / (f * f);
        if border || area_px2 < FEATURE_AREA_PX2 {
            continue;
        }
        let label = k as u32 + 1;
        // New: none of its pixels is in a counter of the reference.
        let (mut sx, mut sy, mut known) = (0.0, 0.0, false);
        for (i, &l) in labels.iter().enumerate() {
            if l != label {
                continue;
            }
            sx += (i % w) as f64;
            sy += (i / w) as f64;
            let r = ink_labels[i];
            if r != 0 && !ink_bg[r as usize - 1].1 {
                known = true;
            }
        }
        if !known {
            found.push(FeatureIssue {
                kind: "faint_counter".to_owned(),
                width_px: 0.0,
                area_px2: (area_px2 * 10.0).round() / 10.0,
                at_px: [
                    win.0 as f64 + sx / count as f64 / f,
                    win.1 as f64 + sy / count as f64 / f,
                ],
            });
        }
    }
    found.truncate(MAX_LISTED);
    Ok(found)
}

/// Fine features of the ink that only show at a stricter level: the regions
/// that are ink at `reference` but background at `stricter` (a slit that
/// the image's blur closed, ink too faint to be sure of), wide and large
/// enough to matter. Informative: anti-aliased corners also show here.
pub(crate) fn faint_features(
    reference: &[Vec<Point>],
    stricter: &[Vec<Point>],
    min_width_px: f64,
) -> Result<Vec<FeatureIssue>, TraceError> {
    let win = window(&[reference, stricter]);
    let f = fineness(win);
    let (w, h) = raster_size(win, f);
    let ink = raster(reference, win, f)?;
    let strict = raster(stricter, win, f)?;
    let mut found = features(
        &|i| ink[i] && !strict[i],
        (w, h),
        (win.0, win.1),
        f,
        min_width_px,
        "faint",
    );
    found.sort_by(|a, b| b.area_px2.total_cmp(&a.area_px2));
    found.truncate(MAX_LISTED);
    Ok(found)
}

#[cfg(test)]
mod tests {
    use forma_model::{Node, PathId};

    use super::*;

    fn square(x0: f64, y0: f64, size: f64, clockwise: bool) -> Vec<Point> {
        let mut p = vec![
            Point::new(x0, y0),
            Point::new(x0 + size, y0),
            Point::new(x0 + size, y0 + size),
            Point::new(x0, y0 + size),
        ];
        if clockwise {
            p.reverse();
        }
        p.push(p[0]);
        p
    }

    fn path(ring: &[Point]) -> Path {
        Path {
            id: PathId(0),
            closed: true,
            nodes: ring[..ring.len() - 1]
                .iter()
                .map(|p| Node {
                    x: p.x,
                    y: -p.y,
                    kind: NodeKind::Line,
                    smooth: false,
                })
                .collect(),
        }
    }

    const ALLOW: Allowance = Allowance {
        fit_px: 0.5,
        min_feature_px: 0.8,
        grid_px: 0.0,
    };

    const IDENTITY: ToPixels = ToPixels {
        scale: 1.0,
        lsb: 0.0,
        origin: (0.0, 0.0),
    };

    #[test]
    fn high_iou_missing_hole_fails() {
        // A 60 px square with a 4 px counter: losing the counter keeps the
        // overlap above 0.99, and must still fail.
        let reference = vec![
            square(10.0, 10.0, 60.0, false),
            square(38.0, 38.0, 4.0, true),
        ];
        let outline = vec![path(&square(10.0, 10.0, 60.0, false))];
        let q = evaluate(&reference, &outline, &IDENTITY, &THRESHOLDS, ALLOW).expect("q");
        assert!(!q.topology_ok, "{q:?}");
        assert_eq!(q.holes, [1, 0]);
        assert!(!q.failures(&THRESHOLDS).is_empty());
    }

    #[test]
    fn a_closed_slit_is_a_filled_feature() {
        // The reference has a 2 px slit 30 px deep; the outline is solid.
        let mut slitted = vec![
            Point::new(10.0, 10.0),
            Point::new(70.0, 10.0),
            Point::new(70.0, 39.0),
            Point::new(40.0, 39.0),
            Point::new(40.0, 41.0),
            Point::new(70.0, 41.0),
            Point::new(70.0, 70.0),
            Point::new(10.0, 70.0),
        ];
        slitted.push(slitted[0]);
        let reference = vec![slitted];
        let outline = vec![path(&square(10.0, 10.0, 60.0, false))];
        let q = evaluate(&reference, &outline, &IDENTITY, &THRESHOLDS, ALLOW).expect("q");
        assert!(q.features.iter().any(|f| f.kind == "filled"), "{q:?}");
    }

    #[test]
    fn a_faithful_outline_passes() {
        let reference = vec![square(10.0, 10.0, 60.0, false)];
        let outline = vec![path(&square(10.1, 10.1, 59.8, false))];
        let q = evaluate(&reference, &outline, &IDENTITY, &THRESHOLDS, ALLOW).expect("q");
        assert!(q.failures(&THRESHOLDS).is_empty(), "{q:?}");
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    #[test]
    fn an_empty_side_is_reported_at_once_not_searched() {
        let ring = vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0),
            Point::new(0.0, 0.0),
        ];
        let started = std::time::Instant::now();
        let (p95, _, max) = hausdorff(&[], &[ring]);
        assert!(p95 >= NO_EDGE && max >= NO_EDGE);
        assert!(started.elapsed().as_secs_f64() < 1.0);
    }

    #[test]
    fn large_windows_are_checked_more_coarsely() {
        assert!((fineness((0, 0, 100, 100)) - FINE).abs() < 1e-9);
        let f = fineness((0, 0, 4000, 4000));
        let (w, h) = raster_size((0, 0, 4000, 4000), f);
        assert!((w * h) as f64 <= MAX_RASTER_SAMPLES * 1.01, "{w} x {h}");
    }
}

#[cfg(test)]
mod crossing_tests {
    use super::*;

    #[test]
    fn a_loop_is_a_crossing_and_a_square_is_not() {
        let ring = |pts: &[(f64, f64)]| {
            let mut v: Vec<Point> = pts.iter().map(|&(x, y)| Point::new(x, y)).collect();
            v.push(v[0]);
            v
        };
        let square = ring(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        assert_eq!(crossings(&[square]), 0);
        let bow = ring(&[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)]);
        assert_eq!(crossings(&[bow]), 1);
    }
}
