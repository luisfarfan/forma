//! Fidelity: how well contours cover the ink of a candidate, as an IoU and
//! an overlay image.

use std::io::Cursor;

use forma_model::{Path, Transform};
use forma_render::{MAX_MASK_PIXELS, Mask, coverage_mask, mask_iou};
use image::{ImageFormat, Rgb, RgbImage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::TraceError;
use crate::candidates::TraceOptions;
use crate::vectorize::{CandidateInk, candidate_ink, check_cap_height, check_index, prepare};

/// Original pixels of margin around the candidate's box in the comparison.
const MARGIN: i64 = 2;
/// The coverage threshold of the IoU (half a pixel).
const THRESHOLD: u8 = 128;
/// The overlay is enlarged by an integer factor up to about this size.
const OVERLAY_TARGET: u32 = 512;
/// The largest enlargement of the overlay.
const OVERLAY_MAX_FACTOR: u32 = 8;
/// The comparison window may grow to hold the contours up to this many
/// times the candidate's box (plus [`MAX_GROWTH_SLACK`] pixels).
const MAX_GROWTH: i64 = 8;
const MAX_GROWTH_SLACK: i64 = 64;
/// The largest side of a mask.
const MAX_SIDE: i64 = u16::MAX as i64;

const BACKGROUND: Rgb<u8> = Rgb([0xFF, 0xFF, 0xFF]);
const MASK_GRAY: Rgb<u8> = Rgb([0x9A, 0x9A, 0x9A]);
const OUTLINE_ORANGE: Rgb<u8> = Rgb([0xFF, 0x5A, 0x1F]);
const DIFFERENCE_RED: Rgb<u8> = Rgb([0xE5, 0x48, 0x4D]);

/// The result of [`compare`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Comparison {
    /// Intersection over union of the rasterized contours and the
    /// candidate's mask, in original pixels (1.0 is a perfect match).
    pub iou: f64,
    /// PNG overlay: the mask in gray `#9A9A9A`, the contour's outline in
    /// orange `#FF5A1F` and the pixels where they differ in red `#E5484D`,
    /// on white, enlarged by an integer factor.
    pub overlay_png: Vec<u8>,
}

/// Which x in font units goes to the left edge of the candidate's ink.
#[derive(Clone, Copy)]
pub(crate) enum Anchor {
    /// The left edge of the contours' bounding box (any contours).
    InkLeft,
    /// This x: the left side bearing of a traced glyph, which was placed
    /// with it (the bounding box of the cleaned contours may be a fraction
    /// of a pixel off the mask's left edge).
    At(f64),
}

/// The comparison window, in original pixels: its top-left pixel and size.
struct Window {
    x: i64,
    y: i64,
    width: u32,
    height: u32,
}

/// The transform from font units to window pixels: the left edge of the
/// contours' bounding box goes to the left edge of the ink, y = 0 to the
/// baseline, `scale` units per pixel; `factor` enlarges the result.
fn to_window(ink: &CandidateInk, x_min: f64, window: &Window, factor: f64) -> Transform {
    let s = factor / ink.scale;
    Transform {
        xx: s,
        xy: 0.0,
        yx: 0.0,
        yy: -s,
        dx: (ink.ink_left_px - window.x as f64) * factor - x_min * s,
        dy: (ink.baseline_px - window.y as f64) * factor,
    }
}

/// The candidate's box plus a margin, grown to hold the contours.
fn window(ink: &CandidateInk, paths: &[Path], anchor: Anchor) -> Result<(Window, f64), TraceError> {
    let b = ink.bbox;
    let (mut x0, mut y0) = (i64::from(b.x0) - MARGIN, i64::from(b.y0) - MARGIN);
    let (mut x1, mut y1) = (i64::from(b.x1) + MARGIN, i64::from(b.y1) + MARGIN);
    let bounds = forma_geometry::bbox(paths);
    let x_min = match anchor {
        Anchor::At(x) => x,
        Anchor::InkLeft => bounds.map_or(0.0, |r| r.x0),
    };
    if let Some(r) = bounds {
        let px0 = ink.ink_left_px + (r.x0 - x_min) / ink.scale;
        let px1 = ink.ink_left_px + (r.x1 - x_min) / ink.scale;
        let py0 = ink.baseline_px - r.y1 / ink.scale;
        let py1 = ink.baseline_px - r.y0 / ink.scale;
        if ![px0, px1, py0, py1].iter().all(|v| v.is_finite()) {
            return Err(TraceError::Render("the contours are not finite".into()));
        }
        let clamp = |v: f64| v.clamp(-1e9, 1e9) as i64;
        x0 = x0.min(clamp(px0.floor()) - 1);
        y0 = y0.min(clamp(py0.floor()) - 1);
        x1 = x1.max(clamp(px1.ceil()) + 1);
        y1 = y1.max(clamp(py1.ceil()) + 1);
    }
    let (w, h) = (x1 - x0, y1 - y0);
    // Contours many times larger than the candidate are at the wrong scale;
    // comparing them would only cost time.
    let (bw, bh) = (
        i64::from(b.x1 - b.x0) + 2 * MARGIN,
        i64::from(b.y1 - b.y0) + 2 * MARGIN,
    );
    let too_large =
        w > MAX_GROWTH * bw + MAX_GROWTH_SLACK || h > MAX_GROWTH * bh + MAX_GROWTH_SLACK;
    if too_large || w > MAX_SIDE || h > MAX_SIDE || (w as u64) * (h as u64) > MAX_MASK_PIXELS {
        return Err(TraceError::Render(format!(
            "the contours are too large to compare: {w} x {h} pixels"
        )));
    }
    Ok((
        Window {
            x: x0,
            y: y0,
            width: w as u32,
            height: h as u32,
        },
        x_min,
    ))
}

/// The candidate's ink in original pixels: a pixel is in (255) when at least
/// half of its mask pixels are.
fn reference(ink: &CandidateInk, window: &Window) -> Mask {
    let u = i64::from(ink.upsample);
    let need = (u * u + 1) / 2;
    let mut data = vec![0u8; window.width as usize * window.height as usize];
    for j in 0..i64::from(window.height) {
        for i in 0..i64::from(window.width) {
            let (ox, oy) = (window.x + i, window.y + j);
            let mut count = 0;
            for my in oy * u..(oy + 1) * u {
                for mx in ox * u..(ox + 1) * u {
                    count += i64::from(ink.at(mx, my));
                }
            }
            if count >= need {
                data[j as usize * window.width as usize + i as usize] = 255;
            }
        }
    }
    Mask {
        width: window.width,
        height: window.height,
        data,
    }
}

fn render(paths: &[Path], t: &Transform, width: u32, height: u32) -> Result<Mask, TraceError> {
    coverage_mask(paths, t, width, height).map_err(|e| TraceError::Render(e.to_string()))
}

/// The IoU of `paths` against the candidate's ink and, if asked, the overlay
/// PNG.
pub(crate) fn evaluate(
    ink: &CandidateInk,
    paths: &[Path],
    anchor: Anchor,
    with_overlay: bool,
) -> Result<(f64, Option<Vec<u8>>), TraceError> {
    let (window, x_min) = window(ink, paths, anchor)?;
    let reference = reference(ink, &window);
    let coverage = render(
        paths,
        &to_window(ink, x_min, &window, 1.0),
        window.width,
        window.height,
    )?;
    let iou = mask_iou(&reference, &coverage, THRESHOLD)
        .map_err(|e| TraceError::Render(e.to_string()))?;
    if !with_overlay {
        return Ok((iou, None));
    }
    let side = window.width.max(window.height);
    let mut factor = (OVERLAY_TARGET / side).clamp(1, OVERLAY_MAX_FACTOR);
    let fits = |f: u32| {
        let (w, h) = (u64::from(window.width * f), u64::from(window.height * f));
        w * h <= MAX_MASK_PIXELS && w <= MAX_SIDE as u64 && h <= MAX_SIDE as u64
    };
    while factor > 1 && !fits(factor) {
        factor -= 1;
    }
    let (fw, fh) = (window.width * factor, window.height * factor);
    let fine = render(
        paths,
        &to_window(ink, x_min, &window, f64::from(factor)),
        fw,
        fh,
    )?;
    let inside = |x: i64, y: i64| -> bool {
        x >= 0
            && y >= 0
            && x < i64::from(fw)
            && y < i64::from(fh)
            && fine.data[y as usize * fw as usize + x as usize] >= THRESHOLD
    };
    let mut img = RgbImage::from_pixel(fw, fh, BACKGROUND);
    for y in 0..fh {
        for x in 0..fw {
            let k = (y / factor) as usize * window.width as usize + (x / factor) as usize;
            let (in_ref, in_cov) = (reference.data[k] != 0, coverage.data[k] >= THRESHOLD);
            let (xi, yi) = (i64::from(x), i64::from(y));
            let outline = inside(xi, yi)
                && !(inside(xi - 1, yi)
                    && inside(xi + 1, yi)
                    && inside(xi, yi - 1)
                    && inside(xi, yi + 1));
            let color = if outline {
                OUTLINE_ORANGE
            } else if in_ref != in_cov {
                DIFFERENCE_RED
            } else if in_ref {
                MASK_GRAY
            } else {
                BACKGROUND
            };
            img.put_pixel(x, y, color);
        }
    }
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)
        .map_err(|e| TraceError::Render(e.to_string()))?;
    Ok((iou, Some(out.into_inner())))
}

/// Compares `paths` (font units, y up) with the ink of candidate `candidate`
/// of `image`.
///
/// The contours are rasterized with [`coverage_mask`] at the candidate's
/// scale (`cap_height_units` over the cap height of its row in pixels), with
/// the left edge of their bounding box on the left edge of the candidate's
/// ink (its box's `x0`, refined to the mask resolution) and y = 0 on the
/// row's baseline. The IoU is computed in original pixels against the
/// candidate's binary mask (a pixel is ink when at least half of its
/// upsampled mask is), over the candidate's box plus a 2-pixel margin, grown
/// to hold the contours. Pieces of other candidates inside the box are not
/// part of the mask. No contours give an IoU of 0 (1 if the candidate had no
/// ink, which cannot happen for a candidate of the analysis).
///
/// # Errors
///
/// * [`TraceError::CandidateOutOfRange`] if `candidate` is not a candidate of
///   the image's analysis;
/// * [`TraceError::InvalidArgument`] if `cap_height_units` is not a positive
///   number;
/// * [`TraceError::Render`] if a contour is malformed or the contours reach
///   farther than 8 times the candidate's box; the errors of
///   [`crate::analyze`].
pub fn compare(
    image: &RgbImage,
    opts: &TraceOptions,
    candidate: usize,
    paths: &[Path],
    cap_height_units: f64,
) -> Result<Comparison, TraceError> {
    check_cap_height(cap_height_units)?;
    let prepared = prepare(image, opts)?;
    check_index(&prepared.analysis, candidate)?;
    let ink = candidate_ink(&prepared, candidate, cap_height_units)?;
    let (iou, png) = evaluate(&ink, paths, Anchor::InkLeft, true)?;
    Ok(Comparison {
        iou,
        overlay_png: png.unwrap_or_default(),
    })
}
