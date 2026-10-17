//! Vectorization: from the ink mask of a candidate to clean contours in font
//! units.
//!
//! The binary mask (at the upsampled resolution, see [`binary_mask`]) is cut
//! out around the candidate, its pixels are handed to `visioncortex` (spline
//! mode), and the splines are mapped to font units and cleaned up
//! ([`crate::cleanup`]).
//!
//! Coordinates: a mask pixel `(mx, my)` covers the original-pixel square
//! `[mx/u, (mx+1)/u) x [my/u, (my+1)/u)`, `u` being the upsampling factor
//! actually used. `visioncortex` traces pixel edges, so its points are mask
//! pixel corners; dividing by `u` gives original pixels.
//!
//! `visioncortex` (0.9.3) returns, per 4-connected cluster, the outer
//! boundary first and then one spline per hole (holes that touch the cut-out's
//! border are ignored, hence the margin). Outer boundaries run clockwise on
//! screen (y down) and holes counter-clockwise; with y flipped to font units
//! that is the wrong way round, which [`crate::cleanup::finish`] fixes with
//! `normalize_direction`. Its angle parameters are in radians.

use std::panic::{AssertUnwindSafe, catch_unwind};

use forma_geometry::transform_paths;
use forma_model::{Path, Transform};
use forma_render::Mask;
use image::RgbImage;
use kurbo::Point;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use visioncortex::{BinaryImage, CompoundPathElement, PathSimplifyMode, PointF64};

use crate::candidates::{Analysis, PxRect, TraceOptions, analyze};
use crate::mask::{binary_mask, effective_upsample};
use crate::{TraceError, cleanup, fidelity};

/// Mask pixels of padding around the cut-out, so that counters never touch
/// its border (`visioncortex` ignores holes that do).
const MARGIN: usize = 2;

/// `visioncortex` spline parameters (spec §6.1): corner threshold, minimum
/// segment length in mask pixels, smoothing iterations and splice threshold.
const CORNER_DEGREES: f64 = 60.0;
const SEGMENT_LENGTH: f64 = 4.0;
const MAX_ITERATIONS: usize = 10;
const SPLICE_DEGREES: f64 = 45.0;

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
    /// The candidate's index in the analysis.
    pub candidate: usize,
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
}

/// The analysis of an image and its binary mask.
pub(crate) struct Prepared {
    pub analysis: Analysis,
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

/// Analyzes `image` and builds its binary mask.
pub(crate) fn prepare(image: &RgbImage, opts: &TraceOptions) -> Result<Prepared, TraceError> {
    let analysis = analyze(image, opts)?;
    let (w, h) = image.dimensions();
    let upsample = effective_upsample(w, h, opts.upsample);
    let mask = binary_mask(
        image,
        analysis.background,
        analysis.ink,
        opts.tolerance,
        upsample,
    );
    Ok(Prepared {
        analysis,
        mask,
        upsample,
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
    let analysis = &prepared.analysis;
    check_index(analysis, index)?;
    let candidate = &analysis.candidates[index];
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
    let b = candidate.bbox;
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

/// The `visioncortex` splines of the candidate's ink, as lists of cubic
/// control points (`1 + 3n` points, the last equal to the first) in mask
/// pixels relative to the cut-out.
fn splines(ink: &CandidateInk) -> Result<Vec<Vec<PointF64>>, TraceError> {
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
            let compound = cluster.to_compound_path(
                PathSimplifyMode::Spline,
                CORNER_DEGREES.to_radians(),
                SEGMENT_LENGTH,
                MAX_ITERATIONS,
                SPLICE_DEGREES.to_radians(),
            );
            for element in compound.iter() {
                match element {
                    CompoundPathElement::Spline(spline) => out.push(spline.points.clone()),
                    // Not produced in spline mode; drawn as straight cubics
                    // just in case.
                    CompoundPathElement::PathI32(path) => {
                        out.push(polygon_as_cubics(path.path.iter().map(|p| PointF64 {
                            x: f64::from(p.x),
                            y: f64::from(p.y),
                        })));
                    }
                    CompoundPathElement::PathF64(path) => {
                        out.push(polygon_as_cubics(path.path.iter().copied()));
                    }
                }
            }
        }
        out
    }));
    traced.map_err(|_| TraceError::Vectorize("the vectorizer failed on this shape".into()))
}

fn polygon_as_cubics(points: impl Iterator<Item = PointF64>) -> Vec<PointF64> {
    let mut out: Vec<PointF64> = Vec::new();
    for p in points {
        if let Some(&last) = out.last() {
            out.push(last);
            out.push(p);
        }
        out.push(p);
    }
    out
}

/// Vectorizes the candidate's ink into clean contours in font units.
fn vectorize(ink: &CandidateInk, lsb: f64) -> Result<Vec<Path>, TraceError> {
    let u = f64::from(ink.upsample);
    let to_units = |p: &PointF64| -> Point {
        let px = (ink.x0 as f64 + p.x) / u;
        let py = (ink.y0 as f64 + p.y) / u;
        Point::new(
            (px - ink.ink_left_px) * ink.scale + lsb,
            (ink.baseline_px - py) * ink.scale,
        )
    };
    let mut contours = Vec::new();
    for spline in splines(ink)? {
        let points: Vec<Point> = spline.iter().map(to_units).collect();
        if let Some(path) = cleanup::contour(&points, ink.scale, ink.upsample)? {
            contours.push(path);
        }
    }
    Ok(cleanup::finish(contours))
}

/// Vectorizes the candidates `indices` of `image` into clean contours in font
/// units.
///
/// For each candidate: the scale is `cap_height_units` over the cap height of
/// its row in pixels; the binary mask is cut out around its box (keeping only
/// its own pieces) with a 2-pixel margin at the mask resolution; the pieces
/// are traced with `visioncortex` in spline mode (corner threshold 60°,
/// segment length 4, 10 iterations, splice threshold 45°) and mapped to font
/// units with y up: `x = (px - x0) * scale + lsb`, `y = (baseline_px - py) *
/// scale`, where `x0` is the left edge of the ink and
/// `lsb = round(0.05 * units_per_em)`. The contours are then cleaned up:
/// cut at their corners, smoothed and refitted between them so that pixel
/// steps do not become nodes, split at their extrema, simplified to within
/// 1.5 units, given the PostScript direction (outer contours
/// counter-clockwise, counters clockwise), with nodes at the extrema, on a
/// 1-unit grid, without duplicate points, and marked `smooth` where the
/// tangents differ by at most 3°. Finally the fidelity is measured: the IoU
/// of [`crate::compare`], with the contours where they were traced (lsb on
/// the ink's left edge) rather than aligned by their bounding box.
///
/// Deterministic: equal inputs give equal outputs.
///
/// # Errors
///
/// * [`TraceError::CandidateOutOfRange`] if an index is not a candidate of
///   the image's analysis;
/// * [`TraceError::InvalidArgument`] if `cap_height_units` is not a positive
///   number;
/// * the errors of [`analyze`], and [`TraceError::Vectorize`],
///   [`TraceError::Geometry`] or [`TraceError::Render`] if a step fails.
pub fn trace(
    image: &RgbImage,
    opts: &TraceOptions,
    indices: &[usize],
    cap_height_units: f64,
    units_per_em: u16,
) -> Result<Vec<TracedGlyph>, TraceError> {
    check_cap_height(cap_height_units)?;
    let prepared = prepare(image, opts)?;
    for &index in indices {
        check_index(&prepared.analysis, index)?;
    }
    let lsb = (SIDE_BEARING_EM * f64::from(units_per_em)).round();
    let mut out = Vec::with_capacity(indices.len());
    for &index in indices {
        let ink = candidate_ink(&prepared, index, cap_height_units)?;
        let traced = vectorize(&ink, lsb)?;
        // Cleanup moves the outline's left edge by a fraction of a pixel:
        // shift it (by whole units, keeping the grid) back onto the side
        // bearing, and the placement with it.
        // The bounding box of curves carries rounding noise (an extremum
        // node at 50 may come out as 49.999999999999993): snap what is that
        // close to the grid.
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
        let ink_width = bounds.map_or(0.0, |r| r.width());
        let iou = fidelity::evaluate(&ink, &paths, fidelity::Anchor::At(lsb + shift), false)?.0;
        out.push(TracedGlyph {
            candidate: index,
            paths,
            ink_width,
            advance_width: ink_width + 2.0 * lsb,
            iou,
            placement: Placement {
                scale: ink.scale,
                origin_px: (ink.ink_left_px - shift / ink.scale, ink.baseline_px),
                lsb,
            },
        });
    }
    Ok(out)
}
