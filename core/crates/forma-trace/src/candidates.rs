//! Glyph candidates: connected components, merging, rows and ordering.

use image::RgbImage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::decode::MAX_IMAGE_PIXELS;
use crate::mask::{binary_mask, effective_upsample};
use crate::palette::{PaletteEntry, background, palette, pick_ink};
use crate::{MAX_CANDIDATES, TraceError};

/// Pieces smaller than this fraction of the median area are noise.
const NOISE_FRACTION: f64 = 0.02;
/// Pieces merge if they overlap horizontally by this much of the narrower.
const MERGE_OVERLAP: f64 = 0.5;
/// ... and their vertical gap is at most this fraction of the taller one.
const MERGE_GAP: f64 = 0.35;
/// The most raw components examined before giving up.
const MAX_RAW_COMPONENTS: usize = 10_000;

/// Options of [`analyze`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TraceOptions {
    /// The ink color; `None` picks the most common non-background color.
    pub color: Option<[u8; 3]>,
    /// Color tolerance, in RGB distance.
    pub tolerance: f64,
    /// Upsampling factor of the mask used to find shapes.
    pub upsample: u32,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            color: None,
            tolerance: 25.0,
            upsample: 4,
        }
    }
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
    /// Typical top edge (median), in pixels: the cap line.
    pub cap_px: f64,
    /// Typical bottom edge (median), in pixels: the baseline.
    pub baseline_px: f64,
    /// Bottommost pixel of any candidate in the row.
    pub bottom: f64,
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

fn components(mask: &forma_render::Mask, u: u32) -> Vec<Piece> {
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
        // Below one original pixel of ink: always noise.
        if area < (u * u) as u64 {
            continue;
        }
        out.push(Piece {
            x0: (minx / u) as u32,
            y0: (miny / u) as u32,
            x1: (maxx / u + 1) as u32,
            y1: (maxy / u + 1) as u32,
            area,
        });
    }
    out
}

fn should_merge(a: &Piece, b: &Piece) -> bool {
    let ov = f64::from(a.x1.min(b.x1)) - f64::from(a.x0.max(b.x0));
    if ov <= 0.0 || ov < MERGE_OVERLAP * a.w().min(b.w()) {
        return false;
    }
    let gap = (f64::from(a.y0.max(b.y0)) - f64::from(a.y1.min(b.y1))).max(0.0);
    gap <= MERGE_GAP * a.h().max(b.h())
}

fn merge_pieces(mut pieces: Vec<Piece>) -> Vec<Piece> {
    loop {
        let mut merged = false;
        'scan: for i in 0..pieces.len() {
            for j in (i + 1)..pieces.len() {
                if should_merge(&pieces[i], &pieces[j]) {
                    let b = pieces.remove(j);
                    let a = &mut pieces[i];
                    a.x0 = a.x0.min(b.x0);
                    a.y0 = a.y0.min(b.y0);
                    a.x1 = a.x1.max(b.x1);
                    a.y1 = a.y1.max(b.y1);
                    a.area += b.area;
                    merged = true;
                    break 'scan;
                }
            }
        }
        if !merged {
            return pieces;
        }
    }
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

fn group_rows(pieces: &[Piece]) -> Vec<Vec<Piece>> {
    let mut sorted: Vec<Piece> = pieces.to_vec();
    sorted.sort_by_key(|p| (p.y0, p.x0, p.y1, p.x1));
    // (ymin, ymax, members)
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
    rows.into_iter()
        .map(|(_, _, mut m)| {
            m.sort_by_key(|p| (p.x0, p.y0, p.x1, p.y1, p.area));
            m
        })
        .collect()
}

/// Finds the glyph candidates of `image`. Deterministic.
///
/// Noise is removed after nearby pieces are merged (so the dot of an "i" is
/// not mistaken for noise).
///
/// # Errors
///
/// [`TraceError::TooLarge`] if the image exceeds the pixel limit and
/// [`TraceError::TooManyCandidates`] above [`MAX_CANDIDATES`].
pub fn analyze(image: &RgbImage, opts: &TraceOptions) -> Result<Analysis, TraceError> {
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
    if pixels == 0 {
        analysis.note = Some("no ink found: the image is empty".into());
        return Ok(analysis);
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
        return Ok(analysis);
    };
    analysis.ink = ink;
    let u = effective_upsample(width, height, opts.upsample);
    let mask = binary_mask(image, analysis.background, ink, opts.tolerance, u);
    let raw = components(&mask, u);
    drop(mask);
    if raw.len() > MAX_RAW_COMPONENTS {
        return Err(TraceError::TooManyCandidates(raw.len()));
    }
    let merged = merge_pieces(raw);
    let mut areas: Vec<f64> = merged.iter().map(|p| p.area as f64).collect();
    let floor = median(&mut areas) * NOISE_FRACTION;
    let kept: Vec<Piece> = merged
        .into_iter()
        .filter(|p| p.area as f64 >= floor)
        .collect();
    if kept.is_empty() {
        analysis.note = Some("no ink found: no shape has the ink color".into());
        return Ok(analysis);
    }
    if kept.len() > MAX_CANDIDATES {
        return Err(TraceError::TooManyCandidates(kept.len()));
    }
    let uu = u64::from(u) * u64::from(u);
    for (ri, row) in group_rows(&kept).into_iter().enumerate() {
        let mut tops: Vec<f64> = row.iter().map(|p| f64::from(p.y0)).collect();
        let mut bottoms: Vec<f64> = row.iter().map(|p| f64::from(p.y1)).collect();
        analysis.rows.push(Row {
            index: ri,
            top: tops.iter().copied().fold(f64::MAX, f64::min),
            cap_px: median(&mut tops),
            baseline_px: median(&mut bottoms),
            bottom: bottoms.iter().copied().fold(0.0, f64::max),
        });
        for p in row {
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
    Ok(analysis)
}
