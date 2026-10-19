//! Glyph candidates: connected components, merging, rows and ordering.

use image::RgbImage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::decode::MAX_IMAGE_PIXELS;
use forma_render::Mask;

use crate::mask::{Coverage, binary_mask, coverage, effective_upsample};
use crate::palette::{PaletteEntry, background, palette, pick_ink};
use crate::{MAX_CANDIDATES, TraceError};

/// Pieces below this fraction of the median piece area are micro-noise and
/// dropped at once.
const MICRO_FRACTION: f64 = 0.005;
/// Pieces from this fraction of the median piece area define the rows; final
/// candidates below this fraction of the median candidate area are noise.
const NOISE_FRACTION: f64 = 0.02;
/// Pieces in a row merge if they overlap horizontally by this much of the
/// narrower one.
const MERGE_OVERLAP: f64 = 0.5;
/// A small piece joins a row if its vertical centre is within the row band
/// expanded by this fraction of the row height on each side.
const BAND_MARGIN: f64 = 0.35;
/// Memory-safety cap on raw connected components, far above any real
/// alphabet.
const MAX_RAW_COMPONENTS: usize = 1_000_000;

/// Options of [`analyze`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TraceOptions {
    /// The ink color; `None` picks the most common non-background color.
    pub color: Option<[u8; 3]>,
    /// Color tolerance: CIE76 ΔE in Lab (D65).
    pub tolerance: f64,
    /// Upsampling factor of the mask used to find shapes.
    pub upsample: u32,
    /// How the edge of the ink is read.
    #[serde(default)]
    pub extractor: Extractor,
    /// How closely the outline follows it.
    #[serde(default)]
    pub fidelity: Fidelity,
    /// The coverage level of the edge, 0.3 to 0.7; 0.5 by default. A higher
    /// level opens slits that the image's blur half closed, a lower one
    /// keeps faint ink.
    #[serde(default = "default_isoline")]
    pub isoline: f64,
}

const fn default_isoline() -> f64 {
    0.5
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            color: None,
            tolerance: 25.0,
            upsample: 4,
            extractor: Extractor::default(),
            fidelity: Fidelity::default(),
            isoline: default_isoline(),
        }
    }
}

/// How the edge of the ink is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Extractor {
    /// The isoline of the ink's coverage, at sub-pixel precision (marching
    /// squares; see `crate::subpixel`).
    #[default]
    Subpixel,
    /// The pixel edges of the 4x upsampled mask (the Hito 2 tracer).
    EdgeWalk,
}

/// How closely the outline follows the edge of the ink.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Fidelity {
    /// Within half an image pixel, keeping irregularities the image holds
    /// steadily (they are part of the drawing).
    #[default]
    Faithful,
    /// Within one image pixel, smoothing irregularities smaller than that.
    Clean,
}

/// A pixel rectangle; `x1` and `y1` are exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PxRect {
    /// Left edge.
    pub x0: u32,
    /// Top edge.
    pub y0: u32,
    /// Right edge, exclusive.
    pub x1: u32,
    /// Bottom edge, exclusive.
    pub y1: u32,
}

/// A row of glyphs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Row {
    /// Row number, from the top.
    pub index: usize,
    /// Topmost pixel of any candidate in the row.
    pub top: f64,
    /// Typical top edge (median), in pixels: the cap line. Measured on the
    /// ink mask, so it has its sub-pixel precision.
    pub cap_px: f64,
    /// Typical bottom edge (median), in pixels: the baseline. Measured on the
    /// ink mask, so it has its sub-pixel precision.
    pub baseline_px: f64,
    /// Bottommost pixel of any candidate in the row.
    pub bottom: f64,
    /// How the cap line and baseline slope across the row (pixels down per
    /// pixel right; 0 for level rows): a photo taken a little askew tilts
    /// them. `cap_px` and `baseline_px` are their heights at `center_x`.
    #[serde(default)]
    pub slope: f64,
    /// Where `cap_px` and `baseline_px` are measured along the row.
    #[serde(default)]
    pub center_x: f64,
}

impl Row {
    /// The baseline at `x`, in pixels.
    #[must_use]
    pub fn baseline_at(&self, x: f64) -> f64 {
        self.baseline_px + self.slope * (x - self.center_x)
    }
}

/// The slope of a row's lines: the median slope between pairs of its
/// pieces' bottoms (Theil–Sen: descenders and dots are outvoted); `0` with
/// fewer than [`SLOPE_MIN_PIECES`] pieces, beyond [`MAX_ROW_SLOPE`], or
/// when the drop across the row is within what overshoots explain.
fn row_slope(pieces: &[Piece]) -> f64 {
    // The largest pieces carry the lines; dust only costs time (the pairs
    // grow with the square of the pieces).
    let mut largest: Vec<&Piece> = pieces.iter().collect();
    largest.sort_by_key(|p| std::cmp::Reverse((p.area, p.x0, p.y0)));
    largest.truncate(SLOPE_PIECES);
    let points: Vec<(f64, f64)> = largest
        .iter()
        .map(|p| ((f64::from(p.x0) + f64::from(p.x1)) / 2.0, p.bottom))
        .collect();
    if points.len() < SLOPE_MIN_PIECES {
        return 0.0;
    }
    let mut slopes = Vec::new();
    for (i, a) in points.iter().enumerate() {
        for b in &points[i + 1..] {
            let dx = b.0 - a.0;
            if dx.abs() > 1.0 {
                slopes.push((b.1 - a.1) / dx);
            }
        }
    }
    let slope = median(&mut slopes);
    // Overshoots and descenders move bottoms by a few pixels: only a drop
    // across the row larger than that is a tilt.
    let (lo, hi) = points.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
        (lo.min(p.0), hi.max(p.0))
    });
    let mut heights: Vec<f64> = largest.iter().map(|p| p.bottom - p.top).collect();
    let cap = median(&mut heights);
    let drop = slope.abs() * (hi - lo);
    if slope.abs() <= MAX_ROW_SLOPE && drop >= TILT_MIN_CAPS * cap {
        slope
    } else {
        0.0
    }
}

/// One shape found in the image, in original pixel coordinates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Candidate {
    /// Reading-order index (row by row, left to right).
    pub index: usize,
    /// The row it belongs to.
    pub row: usize,
    /// Bounding box.
    pub bbox: PxRect,
    /// Ink area in original pixels.
    pub area: u64,
}

/// The result of [`analyze`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Analysis {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Background color.
    pub background: [u8; 3],
    /// Ink color.
    pub ink: [u8; 3],
    /// Color groups, most common first.
    pub palette: Vec<PaletteEntry>,
    /// Rows, top to bottom.
    pub rows: Vec<Row>,
    /// Candidates in reading order.
    pub candidates: Vec<Candidate>,
    /// A remark, e.g. why nothing was found.
    pub note: Option<String>,
}

/// A raw shape: bounding box in original pixels plus area in mask pixels.
#[derive(Clone, Copy)]
struct Piece {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    /// The top and bottom edges of its ink at the mask resolution, in
    /// original pixels (the box rounds them out to whole pixels).
    top: f64,
    bottom: f64,
    area: u64,
}

impl Piece {
    fn w(&self) -> f64 {
        f64::from(self.x1 - self.x0)
    }
    fn h(&self) -> f64 {
        f64::from(self.y1 - self.y0)
    }
}

fn components(mask: &forma_render::Mask, u: u32) -> Result<Vec<Piece>, TraceError> {
    let (w, h) = (mask.width as usize, mask.height as usize);
    let mut seen: Vec<bool> = mask.data.iter().map(|v| *v == 0).collect();
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut minx, mut miny, mut maxx, mut maxy, mut area) =
            (usize::MAX, usize::MAX, 0, 0, 0u64);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
            area += 1;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        let u = u as usize;
        if out.len() >= MAX_RAW_COMPONENTS {
            return Err(TraceError::TooManySpecks {
                limit: MAX_RAW_COMPONENTS,
            });
        }
        out.push(Piece {
            x0: (minx / u) as u32,
            y0: (miny / u) as u32,
            x1: (maxx / u + 1) as u32,
            y1: (maxy / u + 1) as u32,
            top: miny as f64 / u as f64,
            bottom: (maxy + 1) as f64 / u as f64,
            area,
        });
    }
    Ok(out)
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// The fewest pieces a row's slope is measured on.
const SLOPE_MIN_PIECES: usize = 5;
/// A row is tilted only if its baseline drops at least this share of the
/// cap height from end to end.
const TILT_MIN_CAPS: f64 = 0.06;
/// The most pieces a row's slope is measured on (the largest).
const SLOPE_PIECES: usize = 200;
/// The steepest slope a row's lines are taken to have (about 6°); steeper is
/// not a tilted photo but a layout this reading does not know.
const MAX_ROW_SLOPE: f64 = 0.1;
/// A row shorter than this share of the typical row is minor...
const MINOR_ROW: f64 = 0.75;
/// ...and joins a neighbour closer than this share of the typical row.
const MINOR_ROW_GAP: f64 = 0.15;
/// The most pieces of one row that are merged; the smallest beyond are dust.
const MAX_ROW_PIECES: usize = 2000;

/// Merges the pieces of one row whose horizontal overlap is at least
/// [`MERGE_OVERLAP`] of the narrower one: sweep by x with union-find.
fn merge_row(mut pieces: Vec<Piece>) -> Vec<Piece> {
    pieces.sort_by_key(|p| (p.x0, p.y0, p.x1, p.y1, p.area));
    let n = pieces.len();
    let mut parent: Vec<usize> = (0..n).collect();
    let mut active: Vec<usize> = Vec::new();
    for j in 0..n {
        active.retain(|&i| pieces[i].x1 > pieces[j].x0);
        for &i in &active {
            let (a, b) = (&pieces[i], &pieces[j]);
            let ov = f64::from(a.x1.min(b.x1)) - f64::from(a.x0.max(b.x0));
            if ov > 0.0 && ov >= MERGE_OVERLAP * a.w().min(b.w()) {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[rj.max(ri)] = rj.min(ri);
                }
            }
        }
        active.push(j);
    }
    let mut merged: Vec<Option<Piece>> = vec![None; n];
    for (i, p) in pieces.iter().enumerate() {
        let r = find(&mut parent, i);
        merged[r] = Some(match merged[r] {
            None => *p,
            Some(m) => Piece {
                x0: m.x0.min(p.x0),
                y0: m.y0.min(p.y0),
                x1: m.x1.max(p.x1),
                y1: m.y1.max(p.y1),
                top: m.top.min(p.top),
                bottom: m.bottom.max(p.bottom),
                area: m.area + p.area,
            },
        });
    }
    merged.into_iter().flatten().collect()
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

/// Rows of `pieces` by vertical overlap: `(top, bottom, members)`, top to
/// bottom.
fn group_rows(pieces: &[Piece]) -> Vec<(u32, u32, Vec<Piece>)> {
    let mut sorted: Vec<Piece> = pieces.to_vec();
    sorted.sort_by_key(|p| (p.y0, p.x0, p.y1, p.x1));
    let mut rows: Vec<(u32, u32, Vec<Piece>)> = Vec::new();
    for p in sorted {
        let found = rows.iter().position(|(lo, hi, _)| {
            let ov = f64::from(p.y1.min(*hi)) - f64::from(p.y0.max(*lo));
            ov > 0.0 && ov * 2.0 >= p.h().min(f64::from(hi - lo))
        });
        match found {
            Some(i) => {
                let r = &mut rows[i];
                r.0 = r.0.min(p.y0);
                r.1 = r.1.max(p.y1);
                r.2.push(p);
            }
            None => rows.push((p.y0, p.y1, vec![p])),
        }
    }
    rows.sort_by_key(|(lo, hi, _)| (*lo, *hi));
    merge_minor_rows(rows)
}

/// Merges each minor row (much shorter than the image's typical row: the
/// tilde above an Ñ, the half of a row of punctuation that rises above the
/// baseline or falls below it) into the neighbouring row it overlaps or
/// nearly touches. Full rows are separated by clear space and never merge.
fn merge_minor_rows(mut rows: Vec<(u32, u32, Vec<Piece>)>) -> Vec<(u32, u32, Vec<Piece>)> {
    loop {
        if rows.len() < 2 {
            return rows;
        }
        let mut heights: Vec<f64> = rows.iter().map(|(lo, hi, _)| f64::from(hi - lo)).collect();
        let typical = median(&mut heights);
        let gap = |a: &(u32, u32, Vec<Piece>), b: &(u32, u32, Vec<Piece>)| {
            f64::from(b.0) - f64::from(a.1)
        };
        // The minor row closest to a neighbour, and that neighbour.
        let mut best: Option<(f64, usize, usize)> = None;
        for i in 0..rows.len() {
            if f64::from(rows[i].1 - rows[i].0) >= MINOR_ROW * typical {
                continue;
            }
            let near = [
                i.checked_sub(1).map(|j| (gap(&rows[j], &rows[i]), j)),
                (i + 1 < rows.len()).then(|| (gap(&rows[i], &rows[i + 1]), i + 1)),
            ];
            for (g, j) in near.into_iter().flatten() {
                if g < MINOR_ROW_GAP * typical && best.is_none_or(|(b, _, _)| g < b) {
                    best = Some((g, i, j));
                }
            }
        }
        let Some((_, i, j)) = best else {
            return rows;
        };
        let (lo, hi) = (i.min(j), i.max(j));
        let (_, _, members) = rows.remove(hi);
        let row = &mut rows[lo];
        row.0 = row
            .0
            .min(members.iter().map(|p| p.y0).min().unwrap_or(row.0));
        row.1 = row
            .1
            .max(members.iter().map(|p| p.y1).max().unwrap_or(row.1));
        row.2.extend(members);
    }
}

/// Finds the glyph candidates of `image`. Deterministic.
///
/// Rows come from the large pieces; small pieces join the row whose band holds
/// them; pieces merge only within a row (so ":" and "i" stay whole and nothing
/// cascades across rows).
///
/// # Errors
///
/// [`TraceError::TooLarge`] if the image exceeds the pixel limit,
/// [`TraceError::TooManySpecks`] if it has more separate pieces of ink than
/// the memory-safety cap (1 000 000) before the noise filter, and
/// [`TraceError::TooManyCandidates`] above [`MAX_CANDIDATES`].
pub fn analyze(image: &RgbImage, opts: &TraceOptions) -> Result<Analysis, TraceError> {
    analyze_parts(image, opts).map(|parts| parts.analysis)
}

/// What [`analyze`] computes on the way: the analysis, the soft coverage
/// at the image's resolution and the binary mask at `upsample` times it.
pub(crate) struct Parts {
    pub analysis: Analysis,
    pub coverage: Coverage,
    pub mask: Mask,
    pub upsample: u32,
}

/// [`analyze`], keeping the coverage and the mask.
pub(crate) fn analyze_parts(image: &RgbImage, opts: &TraceOptions) -> Result<Parts, TraceError> {
    let (width, height) = image.dimensions();
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_IMAGE_PIXELS {
        return Err(TraceError::TooLarge { bytes: 0, pixels });
    }
    let mut analysis = Analysis {
        width,
        height,
        background: [0, 0, 0],
        ink: [0, 0, 0],
        palette: Vec::new(),
        rows: Vec::new(),
        candidates: Vec::new(),
        note: None,
    };
    let empty = |analysis: Analysis| Parts {
        analysis,
        coverage: Coverage {
            width: 0,
            height: 0,
            data: Vec::new(),
        },
        mask: Mask {
            width: 0,
            height: 0,
            data: Vec::new(),
        },
        upsample: 1,
    };
    if pixels == 0 {
        analysis.note = Some("no ink found: the image is empty".into());
        return Ok(empty(analysis));
    }
    analysis.background = background(image);
    analysis.palette = palette(image);
    let Some(ink) = pick_ink(
        &analysis.palette,
        analysis.background,
        opts.tolerance,
        opts.color,
    ) else {
        analysis.note = Some("no ink found: the image has a single color".into());
        return Ok(empty(analysis));
    };
    analysis.ink = ink;
    let u = effective_upsample(width, height, opts.upsample);
    let soft = coverage(image, analysis.background, ink, opts.tolerance);
    let mask = binary_mask(&soft, u);
    let raw = components(&mask, u)?;
    let mut areas: Vec<f64> = raw.iter().map(|p| p.area as f64).collect();
    let median_piece = median(&mut areas);
    let solid: Vec<Piece> = raw
        .into_iter()
        .filter(|p| p.area as f64 >= median_piece * MICRO_FRACTION)
        .collect();
    let (large, small): (Vec<Piece>, Vec<Piece>) = solid
        .into_iter()
        .partition(|p| p.area as f64 >= median_piece * NOISE_FRACTION);
    let mut rows = group_rows(&large);
    // Small pieces join the row whose expanded band holds their centre
    // (nearest band centre wins); the rest are noise.
    let large_per_row: Vec<Vec<Piece>> = rows.iter().map(|r| r.2.clone()).collect();
    for p in small {
        let cy = (f64::from(p.y0) + f64::from(p.y1)) / 2.0;
        let mut best: Option<(f64, usize)> = None;
        for (i, (lo, hi, _)) in rows.iter().enumerate() {
            let (lo, hi) = (f64::from(*lo), f64::from(*hi));
            let margin = (hi - lo) * BAND_MARGIN;
            if cy >= lo - margin && cy <= hi + margin {
                let dist = (cy - (lo + hi) / 2.0).abs();
                if best.is_none_or(|(d, _)| dist < d) {
                    best = Some((dist, i));
                }
            }
        }
        if let Some((_, i)) = best {
            rows[i].2.push(p);
        }
    }
    // A row of more pieces than any drawing has is dust: only its largest
    // pieces are merged (the merge compares pieces pairwise, and dust never
    // becomes a candidate anyway).
    let mut dust = 0usize;
    let merged: Vec<(usize, Vec<Piece>)> = rows
        .into_iter()
        .enumerate()
        .map(|(i, (_, _, mut members))| {
            if members.len() > MAX_ROW_PIECES {
                members.sort_by_key(|p| std::cmp::Reverse((p.area, p.x0, p.y0)));
                dust += members.len() - MAX_ROW_PIECES;
                members.truncate(MAX_ROW_PIECES);
            }
            (i, merge_row(members))
        })
        .collect();
    if dust > 0 {
        analysis.note = Some(format!(
            "{dust} specks of dust were ignored: rows with more than {MAX_ROW_PIECES} pieces keep \
             only their largest"
        ));
    }
    let mut areas: Vec<f64> = merged
        .iter()
        .flat_map(|(_, c)| c.iter().map(|p| p.area as f64))
        .collect();
    let floor = median(&mut areas) * NOISE_FRACTION;
    let kept: Vec<(usize, Vec<Piece>)> = merged
        .into_iter()
        .map(|(i, c)| {
            (
                i,
                c.into_iter()
                    .filter(|p| p.area as f64 >= floor)
                    .collect::<Vec<_>>(),
            )
        })
        .filter(|(_, c)| !c.is_empty())
        .collect();
    let total: usize = kept.iter().map(|(_, c)| c.len()).sum();
    if total == 0 {
        analysis.note = Some("no ink found: no shape has the ink color".into());
        return Ok(empty(analysis));
    }
    if total > MAX_CANDIDATES {
        return Err(TraceError::TooManyCandidates(total));
    }
    let uu = u64::from(u) * u64::from(u);
    for (ri, (orig, cands)) in kept.into_iter().enumerate() {
        // The cap line and baseline at the mask's resolution: whole-pixel
        // boxes would round every top up and every bottom down, making the
        // cap height up to two pixels too tall.
        // On a tilted row the lines are measured as they run, at the
        // row's centre.
        let large = &large_per_row[orig];
        let slope = row_slope(large);
        let mut xs: Vec<f64> = large
            .iter()
            .map(|p| (f64::from(p.x0) + f64::from(p.x1)) / 2.0)
            .collect();
        let center_x = median(&mut xs);
        let level =
            |p: &Piece, y: f64| y - slope * ((f64::from(p.x0) + f64::from(p.x1)) / 2.0 - center_x);
        let mut tops: Vec<f64> = large.iter().map(|p| level(p, p.top)).collect();
        let mut bottoms: Vec<f64> = large.iter().map(|p| level(p, p.bottom)).collect();
        analysis.rows.push(Row {
            slope,
            center_x,
            index: ri,
            top: cands
                .iter()
                .map(|p| f64::from(p.y0))
                .fold(f64::MAX, f64::min),
            cap_px: median(&mut tops),
            baseline_px: median(&mut bottoms),
            bottom: cands.iter().map(|p| f64::from(p.y1)).fold(0.0, f64::max),
        });
        for p in cands {
            analysis.candidates.push(Candidate {
                index: analysis.candidates.len(),
                row: ri,
                bbox: PxRect {
                    x0: p.x0,
                    y0: p.y0,
                    x1: p.x1,
                    y1: p.y1,
                },
                area: (p.area + uu / 2) / uu,
            });
        }
    }
    Ok(Parts {
        analysis,
        coverage: soft,
        mask,
        upsample: u,
    })
}
