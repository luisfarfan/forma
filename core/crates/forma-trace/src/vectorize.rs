//! Vectorization: from the ink mask of a candidate to clean contours in font
//! units.
//!
//! The binary mask (at the upsampled resolution, see [`binary_mask`]) is cut
//! out around the candidate, `visioncortex` walks the pixel edges of each of
//! its pieces, each walk is reduced to its midline (the polygon through the
//! middle of its runs, see [`midline`]), and the midlines are mapped to font
//! units and cleaned up ([`crate::cleanup`]).
//!
//! Why the walk and not `visioncortex`'s own curves: its simplification
//! keeps the outer corners of the staircase's steps (outer with respect to
//! each contour, so counters grow as well as outer contours) and its
//! smoothing interpolates them, so its curves lie outside the mask's edge,
//! by about a tenth of an original pixel on average and up to a quarter at
//! the ends of a straight edge (measured on the specimen: +0.01 to +0.14 px
//! per letter, the least for letters with counters). The midline lies on
//! the mask's 50 % edge, with no bias.
//!
//! Coordinates: a mask pixel `(mx, my)` covers the original-pixel square
//! `[mx/u, (mx+1)/u) x [my/u, (my+1)/u)`, `u` being the upsampling factor
//! actually used. The walk runs along pixel edges, so its points are mask
//! pixel corners; dividing by `u` gives original pixels.
//!
//! `visioncortex` (0.9.3) returns, per 4-connected cluster, the outer
//! boundary first and then one walk per hole (holes that touch the cut-out's
//! border are ignored, hence the margin). Outer boundaries run clockwise on
//! screen (y down) and holes counter-clockwise; with y flipped to font units
//! that is the wrong way round, which [`crate::cleanup::finish`] fixes with
//! `normalize_direction`.

use std::panic::{AssertUnwindSafe, catch_unwind};

use forma_geometry::transform_paths;
use forma_model::{Path, Transform};
use forma_render::Mask;
use image::RgbImage;
use kurbo::Point;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use visioncortex::{BinaryImage, CompoundPathElement, PathSimplifyMode};

use crate::candidates::{Analysis, Extractor, Fidelity, PxRect, TraceOptions, analyze_parts};
use crate::cleanup::Fit;
use crate::mask::Coverage;
use crate::quality::{FeatureIssue, QualityReport, THRESHOLDS, ToPixels};
use crate::subpixel::{Field, edge_softness, isolines};
use crate::{TraceError, cleanup, fidelity, quality};

/// Mask pixels of padding around the cut-out, so that counters never touch
/// its border (`visioncortex` ignores holes that do).
const MARGIN: usize = 2;

/// Pieces of a candidate smaller than this fraction of its largest piece are
/// specks (the same fraction as the micro-noise of [`analyze`]).
const SPECK_FRACTION: f64 = 0.005;

/// The left and right side bearings of a traced glyph, as a fraction of the
/// em.
const SIDE_BEARING_EM: f64 = 0.05;

/// Where a traced glyph sits in the image.
///
/// A point `(x, y)` in font units corresponds to the original pixel
/// `(origin_px.0 + (x - lsb) / scale, origin_px.1 - y / scale)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Placement {
    /// Font units per original pixel: the cap height in units over the cap
    /// height of the candidate's row in pixels.
    pub scale: f64,
    /// The pixel that maps to `(lsb, 0)`: the left edge of the candidate's ink
    /// (its box's `x0`, refined to the mask resolution, moved by the whole
    /// units that put the traced outline's left edge exactly on `lsb`) and
    /// the baseline of its row.
    pub origin_px: (f64, f64),
    /// The left side bearing in font units: 5 % of the em, rounded.
    pub lsb: f64,
}

/// One vectorized candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TracedGlyph {
    /// The (first) candidate's index in the analysis.
    pub candidate: usize,
    /// Every candidate traced into this glyph (several for a letter drawn
    /// in separate pieces).
    pub candidates: Vec<usize>,
    /// Clean contours in font units: outer contours counter-clockwise,
    /// counters clockwise, nodes at the extrema, on a 1-unit grid.
    pub paths: Vec<Path>,
    /// The width of the contours' bounding box.
    pub ink_width: f64,
    /// `ink_width` plus both side bearings: the outline spans `lsb` to
    /// `lsb + ink_width`, so both bearings are `lsb`.
    pub advance_width: f64,
    /// Intersection over union of the contours, rasterized at the image
    /// scale, with the candidate's mask (see [`crate::compare`]).
    pub iou: f64,
    /// How the contours map back to the image.
    pub placement: Placement,
    /// How faithful the contours are (spec §4).
    pub quality: QualityReport,
    /// The attempt that produced them (see [`trace_groups`]).
    pub attempt: String,
    /// Whether a person (or an agent) should look at the glyph: it fails a
    /// quality gate, or the image holds a feature too faint to be sure of.
    pub needs_review: bool,
    /// Why, in words; empty when nothing needs a look.
    pub reasons: Vec<String>,
    /// Features of the ink that only show at a stricter level (a slit the
    /// image's blur half closed, an anti-aliased corner); informative. A
    /// counter that only shows there makes the glyph `needs_review`.
    pub faint: Vec<FeatureIssue>,
}

/// The analysis of an image and its binary mask.
pub(crate) struct Prepared {
    pub analysis: Analysis,
    pub coverage: Coverage,
    pub mask: Mask,
    pub upsample: u32,
}

/// The ink of one candidate at the mask resolution, without the pieces of
/// other candidates that fall inside its box.
pub(crate) struct CandidateInk {
    /// The upsampling factor of the mask.
    pub upsample: u32,
    /// Mask-pixel position of `pixels[0]` (may be negative: the margin).
    pub x0: i64,
    /// See `x0`.
    pub y0: i64,
    /// Width of `pixels`, margin included.
    pub width: usize,
    /// Height of `pixels`, margin included.
    pub height: usize,
    /// Row-major ink flags.
    pub pixels: Vec<bool>,
    /// The leftmost ink, in original pixels.
    pub ink_left_px: f64,
    /// The candidate's box in original pixels.
    pub bbox: PxRect,
    /// The baseline of the candidate's row, in original pixels.
    pub baseline_px: f64,
    /// Font units per original pixel.
    pub scale: f64,
}

impl CandidateInk {
    /// Whether the mask pixel `(mx, my)` (absolute) is ink of the candidate.
    pub(crate) fn at(&self, mx: i64, my: i64) -> bool {
        let (lx, ly) = (mx - self.x0, my - self.y0);
        if lx < 0 || ly < 0 || lx >= self.width as i64 || ly >= self.height as i64 {
            return false;
        }
        self.pixels[ly as usize * self.width + lx as usize]
    }
}

pub(crate) fn check_cap_height(cap_height_units: f64) -> Result<(), TraceError> {
    if cap_height_units.is_finite() && cap_height_units > 0.0 {
        Ok(())
    } else {
        Err(TraceError::InvalidArgument(format!(
            "the cap height must be a positive number of units, got {cap_height_units}"
        )))
    }
}

/// Analyzes `image`, keeping its coverage and binary mask.
pub(crate) fn prepare(image: &RgbImage, opts: &TraceOptions) -> Result<Prepared, TraceError> {
    let parts = analyze_parts(image, opts)?;
    Ok(Prepared {
        analysis: parts.analysis,
        coverage: parts.coverage,
        mask: parts.mask,
        upsample: parts.upsample,
    })
}

pub(crate) fn check_index(analysis: &Analysis, index: usize) -> Result<(), TraceError> {
    let count = analysis.candidates.len();
    if index < count {
        Ok(())
    } else {
        Err(TraceError::CandidateOutOfRange { index, count })
    }
}

/// The 8-connected ink pieces inside `region` of `mask` (`x0, y0, x1, y1`,
/// exclusive ends): a label per pixel (0 = no ink) and, per label, its area
/// and whether it continues outside the region (a piece of a neighbour).
fn pieces(mask: &Mask, region: (usize, usize, usize, usize)) -> (Vec<u32>, Vec<(u64, bool)>) {
    let (rx0, ry0, rx1, ry1) = region;
    let (rw, rh) = (rx1 - rx0, ry1 - ry0);
    let (mw, mh) = (mask.width as i64, mask.height as i64);
    let ink = |x: i64, y: i64| -> bool {
        x >= 0
            && y >= 0
            && x < mw
            && y < mh
            && mask.data[y as usize * mw as usize + x as usize] != 0
    };
    let mut labels = vec![0u32; rw * rh];
    let mut info: Vec<(u64, bool)> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..rw * rh {
        let (sx, sy) = ((rx0 + start % rw) as i64, (ry0 + start / rw) as i64);
        if labels[start] != 0 || !ink(sx, sy) {
            continue;
        }
        let label = info.len() as u32 + 1;
        labels[start] = label;
        stack.push(start);
        let (mut area, mut foreign) = (0u64, false);
        while let Some(i) = stack.pop() {
            area += 1;
            let (x, y) = ((rx0 + i % rw) as i64, (ry0 + i / rw) as i64);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if !ink(nx, ny) {
                        continue;
                    }
                    let inside =
                        nx >= rx0 as i64 && ny >= ry0 as i64 && nx < rx1 as i64 && ny < ry1 as i64;
                    if !inside {
                        foreign = true;
                        continue;
                    }
                    let j = (ny as usize - ry0) * rw + (nx as usize - rx0);
                    if labels[j] == 0 {
                        labels[j] = label;
                        stack.push(j);
                    }
                }
            }
        }
        info.push((area, foreign));
    }
    (labels, info)
}

/// The ink of candidate `index`: the pieces inside its box that do not
/// continue outside it, without specks.
pub(crate) fn candidate_ink(
    prepared: &Prepared,
    index: usize,
    cap_height_units: f64,
) -> Result<CandidateInk, TraceError> {
    group_ink(prepared, &[index], cap_height_units)
}

/// The ink of the candidates `indices` traced as one glyph: the pieces
/// inside the union of their boxes that do not continue outside it, without
/// specks. They must be of the same row, whose cap height sets the scale.
pub(crate) fn group_ink(
    prepared: &Prepared,
    indices: &[usize],
    cap_height_units: f64,
) -> Result<CandidateInk, TraceError> {
    let analysis = &prepared.analysis;
    let Some(&index) = indices.first() else {
        return Err(TraceError::InvalidArgument("no candidate to trace".into()));
    };
    for &i in indices {
        check_index(analysis, i)?;
    }
    let candidate = &analysis.candidates[index];
    if let Some(&other) = indices
        .iter()
        .find(|&&i| analysis.candidates[i].row != candidate.row)
    {
        return Err(TraceError::InvalidArgument(format!(
            "candidates {index} and {other} are in different rows; a glyph's pieces must be \
             in one row"
        )));
    }
    let union = indices.iter().fold(candidate.bbox, |b, &i| {
        let c = analysis.candidates[i].bbox;
        PxRect {
            x0: b.x0.min(c.x0),
            y0: b.y0.min(c.y0),
            x1: b.x1.max(c.x1),
            y1: b.y1.max(c.y1),
        }
    });
    let row = analysis
        .rows
        .get(candidate.row)
        .ok_or_else(|| TraceError::Vectorize(format!("candidate {index} has no row")))?;
    let row_height = row.baseline_px - row.cap_px;
    if !(row_height.is_finite() && row_height > 0.0) {
        return Err(TraceError::Vectorize(format!(
            "row {} has no cap height",
            row.index
        )));
    }
    let scale = cap_height_units / row_height;
    let u = prepared.upsample as usize;
    let mask = &prepared.mask;
    let b = union;
    let region = (
        (b.x0 as usize * u).min(mask.width as usize),
        (b.y0 as usize * u).min(mask.height as usize),
        (b.x1 as usize * u).min(mask.width as usize),
        (b.y1 as usize * u).min(mask.height as usize),
    );
    let (labels, info) = pieces(mask, region);
    let largest = info
        .iter()
        .filter(|(_, foreign)| !foreign)
        .map(|(area, _)| *area)
        .max()
        .unwrap_or(0);
    let floor = ((u * u) as f64).max(largest as f64 * SPECK_FRACTION);
    let keep: Vec<bool> = info
        .iter()
        .map(|(area, foreign)| !foreign && *area as f64 >= floor)
        .collect();
    let (rw, rh) = (region.2 - region.0, region.3 - region.1);
    let (width, height) = (rw + 2 * MARGIN, rh + 2 * MARGIN);
    let mut pixels = vec![false; width * height];
    let mut left = usize::MAX;
    for y in 0..rh {
        for x in 0..rw {
            let label = labels[y * rw + x];
            if label != 0 && keep[label as usize - 1] {
                pixels[(y + MARGIN) * width + x + MARGIN] = true;
                left = left.min(x);
            }
        }
    }
    if left == usize::MAX {
        return Err(TraceError::Vectorize(format!(
            "candidate {index} has no ink"
        )));
    }
    Ok(CandidateInk {
        upsample: prepared.upsample,
        x0: region.0 as i64 - MARGIN as i64,
        y0: region.1 as i64 - MARGIN as i64,
        width,
        height,
        pixels,
        ink_left_px: (region.0 + left) as f64 / u as f64,
        bbox: b,
        baseline_px: row.baseline_px,
        scale,
    })
}

/// The boundaries of the candidate's ink, as closed polygons (the last point
/// equal to the first) in mask pixels relative to the cut-out: the
/// `visioncortex` walk of each 4-connected cluster's pixel edges (outer
/// boundary, then its holes), reduced to its midline (see [`midline`]).
fn boundaries(ink: &CandidateInk) -> Result<Vec<Vec<Point>>, TraceError> {
    let mut image = BinaryImage::new_w_h(ink.width, ink.height);
    for y in 0..ink.height {
        for x in 0..ink.width {
            if ink.pixels[y * ink.width + x] {
                image.set_pixel(x, y, true);
            }
        }
    }
    let speck = (ink.upsample * ink.upsample) as usize;
    // `visioncortex` asserts on inputs it cannot handle; a panic there must
    // not take the caller down.
    let traced = catch_unwind(AssertUnwindSafe(|| {
        let mut out = Vec::new();
        for cluster in image.to_clusters(false).iter() {
            if cluster.size() < speck {
                continue;
            }
            // The walk alone: the curve parameters only apply to the
            // simplifying modes.
            let compound = cluster.to_compound_path(PathSimplifyMode::None, 0.0, 0.0, 1, 0.0);
            for element in compound.iter() {
                let walk: Vec<Point> = match element {
                    CompoundPathElement::PathI32(path) => path
                        .path
                        .iter()
                        .map(|p| Point::new(f64::from(p.x), f64::from(p.y)))
                        .collect(),
                    CompoundPathElement::PathF64(path) => {
                        path.path.iter().map(|p| Point::new(p.x, p.y)).collect()
                    }
                    // Not produced without simplification.
                    CompoundPathElement::Spline(spline) => {
                        spline.points.iter().map(|p| Point::new(p.x, p.y)).collect()
                    }
                };
                let line = midline(&walk);
                if line.len() >= 4 {
                    out.push(line);
                }
            }
        }
        out
    }));
    traced.map_err(|_| TraceError::Vectorize("the vectorizer failed on this shape".into()))
}

/// The midline of a pixel-edge walk (a closed polygon of axis-aligned runs,
/// the last point equal to the first): the closed polygon through the
/// middle of each run, and through the vertex between two runs longer than
/// one mask pixel (a corner of the ink rather than a step of the staircase).
///
/// The ink's true edge crosses the steps of the mask's staircase halfway, so
/// the middles of the runs lie on it: a straight edge at any slope comes out
/// straight, at its true position. The walk itself, and a polygon through
/// the outer corners of its steps (which is what `visioncortex`'s own
/// simplification keeps before smoothing), lie outside the ink by up to a
/// mask pixel. Corners come out cut by a fraction of a mask pixel;
/// [`crate::cleanup`] puts them back where the edges meet.
fn midline(walk: &[Point]) -> Vec<Point> {
    let runs: Vec<(Point, Point)> = walk
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .filter(|(a, b)| a.distance(*b) > 0.0)
        .collect();
    let n = runs.len();
    let mut out = Vec::with_capacity(2 * n + 1);
    for k in 0..n {
        let (a, b) = runs[k];
        let (pa, pb) = runs[(k + n - 1) % n];
        if a.distance(b) > 1.0 && pa.distance(pb) > 1.0 {
            out.push(a);
        }
        out.push(a.midpoint(b));
    }
    if let Some(&first) = out.first() {
        out.push(first);
    }
    out
}

/// The edge of the candidate's ink as closed polygons in original pixels:
/// the isoline of its own coverage at `level` ([`Extractor::Subpixel`]) or
/// the midlines of the mask's pixel-edge walks ([`Extractor::EdgeWalk`]).
fn edge_loops(
    prepared: &Prepared,
    ink: &CandidateInk,
    extractor: Extractor,
    level: f64,
) -> Result<Vec<Vec<Point>>, TraceError> {
    match extractor {
        Extractor::EdgeWalk => {
            let u = f64::from(ink.upsample);
            Ok(boundaries(ink)?
                .into_iter()
                .map(|ring| {
                    ring.into_iter()
                        .map(|p| Point::new((ink.x0 as f64 + p.x) / u, (ink.y0 as f64 + p.y) / u))
                        .collect()
                })
                .collect())
        }
        _ => Ok(isolines(&own_field(prepared, ink), level as f32)),
    }
}

/// The candidate's own coverage field (see [`Field::own`]).
fn own_field(prepared: &Prepared, ink: &CandidateInk) -> Field {
    let mask = &prepared.mask;
    let (mw, mh) = (i64::from(mask.width), i64::from(mask.height));
    let mask_ink = |mx: i64, my: i64| -> bool {
        mx >= 0 && my >= 0 && mx < mw && my < mh && mask.data[(my * mw + mx) as usize] != 0
    };
    Field::own(&prepared.coverage, &mask_ink, ink)
}

/// Vectorizes edge polygons (original pixels) into clean contours in font
/// units.
fn vectorize(
    ink: &CandidateInk,
    loops: &[Vec<Point>],
    lsb: f64,
    fit: Fit,
    upsample: u32,
) -> Result<Vec<Path>, TraceError> {
    let to_units = |p: &Point| -> Point {
        Point::new(
            (p.x - ink.ink_left_px) * ink.scale + lsb,
            (ink.baseline_px - p.y) * ink.scale,
        )
    };
    let mut contours = Vec::new();
    for ring in loops {
        let points: Vec<Point> = ring.iter().map(to_units).collect();
        if let Some(path) = cleanup::contour(&points, ink.scale, upsample, fit)? {
            contours.push(path);
        }
    }
    Ok(cleanup::finish(contours))
}

/// One way of tracing a candidate: how its edge is read and how closely the
/// outline follows it.
struct Attempt {
    name: &'static str,
    extractor: Extractor,
    fit: Fit,
}

/// Edges softer than this (see [`edge_softness`]) are anti-aliased; harder
/// ones are pixel steps, smoothed as noise.
const SOFT_EDGES: f64 = 0.25;

/// The attempts of [`trace_groups`], in order: the first that passes every
/// quality gate wins. Hard-edged ink (`hard`) starts by smoothing its pixel
/// steps away, which are noise half a pixel high, not drawing.
fn attempts(opts: &TraceOptions, hard: bool) -> Vec<Attempt> {
    let faithful = Attempt {
        name: "subpixel",
        extractor: Extractor::Subpixel,
        fit: Fit {
            smooth_px: 0.75,
            max_fit_px: 0.5,
            corner_limit_px: 2.0,
        },
    };
    let tight = Attempt {
        name: "subpixel-tight",
        extractor: Extractor::Subpixel,
        fit: Fit {
            smooth_px: 0.4,
            max_fit_px: 0.3,
            corner_limit_px: 1.0,
        },
    };
    let exact = Attempt {
        name: "subpixel-exact",
        extractor: Extractor::Subpixel,
        fit: Fit {
            smooth_px: 0.0,
            max_fit_px: 0.2,
            corner_limit_px: 0.0,
        },
    };
    let walk = Attempt {
        name: "edge-walk",
        extractor: Extractor::EdgeWalk,
        fit: Fit::STEPPED,
    };
    let clean = Attempt {
        name: "subpixel-clean",
        extractor: Extractor::Subpixel,
        fit: Fit {
            smooth_px: 1.5,
            max_fit_px: 1.0,
            corner_limit_px: 2.5,
        },
    };
    let stepped = Attempt {
        name: "subpixel-stepped",
        extractor: Extractor::Subpixel,
        fit: Fit {
            corner_limit_px: 2.0,
            ..Fit::STEPPED
        },
    };
    let mut list = match (opts.fidelity, hard) {
        (_, true) => vec![stepped, faithful, tight, exact],
        (Fidelity::Clean, false) => vec![clean, faithful, tight, exact],
        _ => vec![faithful, tight, exact],
    };
    if opts.extractor == Extractor::EdgeWalk {
        list.insert(0, walk);
    } else {
        list.push(walk);
    }
    list
}

/// One traced attempt with its verdict.
struct Outcome {
    attempt: &'static str,
    paths: Vec<Path>,
    shift: f64,
    iou: f64,
    quality: QualityReport,
    failures: Vec<String>,
}

/// Vectorizes the candidates `indices` of `image`, each into its own glyph;
/// see [`trace_groups`].
///
/// # Errors
///
/// As [`trace_groups`].
pub fn trace(
    image: &RgbImage,
    opts: &TraceOptions,
    indices: &[usize],
    cap_height_units: f64,
    units_per_em: u16,
) -> Result<Vec<TracedGlyph>, TraceError> {
    let groups: Vec<Vec<usize>> = indices.iter().map(|&i| vec![i]).collect();
    trace_groups(image, opts, &groups, cap_height_units, units_per_em)
}

/// Vectorizes each group of candidates of `image` (one or several pieces of
/// the same row) into clean contours in font units, one glyph per group.
///
/// For each group: the scale is `cap_height_units` over the cap height of
/// its row in pixels; the reference edge is the isoline of the group's own
/// ink coverage at `opts.isoline` (see [`crate::subpixel`]). The outline is
/// traced by a series of attempts ([`Extractor::Subpixel`] at decreasing
/// smoothing, then [`Extractor::EdgeWalk`]); each is cleaned up (corners,
/// smoothing and refit, extrema, PostScript direction, 1-unit grid, smooth
/// nodes; see [`crate::cleanup`]), placed with its left edge on
/// `lsb = round(0.05 * units_per_em)`, and judged against the reference
/// ([`QualityReport`], [`THRESHOLDS`]). The first attempt that passes every
/// gate wins; if none does, the best one is kept and the glyph
/// `needs_review`, with the reasons. Features that only show at a stricter
/// level (`isoline + 0.15`) are reported in `faint` and also call for a
/// review.
///
/// Deterministic: equal inputs give equal outputs.
///
/// # Errors
///
/// * [`TraceError::CandidateOutOfRange`] if an index is not a candidate of
///   the image's analysis;
/// * [`TraceError::InvalidArgument`] if `cap_height_units` is not a positive
///   number, a group is empty or mixes rows;
/// * the errors of [`crate::analyze`], and [`TraceError::Vectorize`],
///   [`TraceError::Geometry`] or [`TraceError::Render`] if a step fails.
pub fn trace_groups(
    image: &RgbImage,
    opts: &TraceOptions,
    groups: &[Vec<usize>],
    cap_height_units: f64,
    units_per_em: u16,
) -> Result<Vec<TracedGlyph>, TraceError> {
    check_cap_height(cap_height_units)?;
    if !(0.3..=0.7).contains(&opts.isoline) {
        return Err(TraceError::InvalidArgument(format!(
            "the isoline must be within 0.3 to 0.7, got {}",
            opts.isoline
        )));
    }
    let prepared = prepare(image, opts)?;
    for group in groups {
        for &index in group {
            check_index(&prepared.analysis, index)?;
        }
    }
    let lsb = (SIDE_BEARING_EM * f64::from(units_per_em)).round();
    let mut out = Vec::with_capacity(groups.len());
    for group in groups {
        let ink = group_ink(&prepared, group, cap_height_units)?;
        let field = own_field(&prepared, &ink);
        let reference = isolines(&field, opts.isoline as f32);
        let stricter = isolines(&field, (opts.isoline + 0.15) as f32);
        let hard = edge_softness(&field, opts.isoline as f32) < SOFT_EDGES;
        let (faint, faint_counters) = if hard {
            // A hard edge has no blur to half close a slit.
            (Vec::new(), Vec::new())
        } else {
            (
                quality::faint_features(&reference, &stricter, THRESHOLDS.feature_width_px)?,
                quality::faint_counters(&reference, &stricter)?,
            )
        };
        let mut best: Option<Outcome> = None;
        for attempt in attempts(opts, hard) {
            let loops = if attempt.extractor == Extractor::Subpixel {
                reference.clone()
            } else {
                edge_loops(&prepared, &ink, attempt.extractor, opts.isoline)?
            };
            let upsample = if attempt.extractor == Extractor::EdgeWalk {
                ink.upsample
            } else {
                4
            };
            let traced = vectorize(&ink, &loops, lsb, attempt.fit, upsample)?;
            let outcome = judge(&ink, &reference, traced, lsb, attempt.name, hard)?;
            let passed = outcome.failures.is_empty();
            let better = best
                .as_ref()
                .is_none_or(|b| severity(&outcome) < severity(b));
            if better {
                best = Some(outcome);
            }
            if passed {
                break;
            }
        }
        let Some(chosen) = best else {
            return Err(TraceError::Vectorize(
                "no attempt traced the candidate".into(),
            ));
        };
        let bounds = forma_geometry::bbox(&chosen.paths);
        let ink_width = bounds.map_or(0.0, |r| r.width());
        let mut reasons = chosen.failures.clone();
        for f in &faint_counters {
            reasons.push(format!(
                "a counter of {:.0} square px shows at a stricter level at ({:.0}, {:.0}): the \
                 image may have filled it; trace with a higher isoline to open it",
                f.area_px2, f.at_px[0], f.at_px[1]
            ));
        }
        out.push(TracedGlyph {
            candidate: group[0],
            candidates: group.clone(),
            paths: chosen.paths,
            ink_width,
            advance_width: ink_width + 2.0 * lsb,
            iou: chosen.iou,
            placement: Placement {
                scale: ink.scale,
                origin_px: (ink.ink_left_px - chosen.shift / ink.scale, ink.baseline_px),
                lsb,
            },
            quality: chosen.quality,
            attempt: chosen.attempt.to_owned(),
            needs_review: !reasons.is_empty(),
            reasons,
            faint,
        });
    }
    Ok(out)
}

/// How bad an attempt that fails is, to keep the least bad when none
/// passes: a spot far off the ink first (a spike is worse than anything
/// else), then the topology, then lost or invented features, then the
/// remaining gates, then the typical distance.
fn severity(outcome: &Outcome) -> (bool, bool, usize, usize, i64) {
    let q = &outcome.quality;
    (
        q.hausdorff_max_px > 2.0 * THRESHOLDS.hausdorff_max_px,
        !q.topology_ok,
        q.features.len(),
        outcome.failures.len(),
        (q.hausdorff_p95_px * 1000.0).round() as i64,
    )
}

/// Places traced contours with their left edge on `lsb` and judges them
/// against the reference edge.
fn judge(
    ink: &CandidateInk,
    reference: &[Vec<Point>],
    traced: Vec<Path>,
    lsb: f64,
    attempt: &'static str,
    hard: bool,
) -> Result<Outcome, TraceError> {
    // Cleanup moves the outline's left edge by a fraction of a pixel:
    // shift it (by whole units, keeping the grid) back onto the side
    // bearing, and the placement with it. The bounding box of curves
    // carries rounding noise (an extremum node at 50 may come out as
    // 49.999999999999993): snap what is that close to the grid.
    let bounds = forma_geometry::bbox(&traced).map(|r| {
        let snap = |v: f64| {
            if (v - v.round()).abs() < 1e-6 {
                v.round()
            } else {
                v
            }
        };
        kurbo::Rect::new(snap(r.x0), r.y0, snap(r.x1), r.y1)
    });
    let shift = bounds.map_or(0.0, |r| (lsb - r.x0).round());
    let paths = transform_paths(
        &traced,
        &Transform {
            dx: shift,
            ..Transform::IDENTITY
        },
    );
    let iou = fidelity::evaluate(ink, &paths, fidelity::Anchor::At(lsb + shift), false)?.0;
    let to = ToPixels {
        scale: ink.scale,
        lsb,
        origin: (ink.ink_left_px - shift / ink.scale, ink.baseline_px),
    };
    let quality = quality::evaluate(reference, &paths, &to, iou, &THRESHOLDS, hard)?;
    let failures = quality.failures(&THRESHOLDS);
    Ok(Outcome {
        attempt,
        paths,
        shift,
        iou,
        quality,
        failures,
    })
}
