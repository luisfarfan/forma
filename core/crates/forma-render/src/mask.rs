//! Coverage masks: rasterized outlines as numbers, for comparing glyphs with
//! images.

use forma_model::{Path, Transform};
use vello_cpu::kurbo::{Affine, BezPath, PathEl, Point};

use crate::RenderError;
use crate::draw::{Canvas, outline};

/// The most pixels a [`Mask`] may have.
pub const MAX_MASK_PIXELS: u64 = 50_000_000;

/// The widest or tallest mask, in pixels (the renderer addresses pixels with
/// 16 bits).
const MAX_MASK_SIDE: u32 = u16::MAX as u32;

/// Curves are flattened to within this many pixels before they are filled.
///
/// The rasterizer flattens curves itself to within a quarter of a pixel,
/// which is fine for a preview but not for a measurement: a chord a quarter
/// of a pixel inside an arc takes up to a quarter of the coverage of the
/// pixels along it (measured on round letters at 112 px: off by up to 0.26,
/// 0.1 on average, against 0.02 on straight edges).
const FLATTEN_TOLERANCE_PX: f64 = 0.01;

/// The most chords per curve (a curve thousands of pixels long).
const MAX_CHORDS: f64 = 65_536.0;

/// The coverage of an outline on a pixel grid.
///
/// `data` holds `width * height` values in rows from top to bottom, left to
/// right: 0 where the pixel is outside the outline, 255 where it is entirely
/// inside, and in between at the anti-aliased edges (the fraction of the
/// pixel's area covered, non-zero fill rule).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Coverage, 0 to 255, `width * height` values.
    pub data: Vec<u8>,
}

fn check_dimensions(width: u32, height: u32) -> Result<(u16, u16), RenderError> {
    let too_big = u64::from(width) * u64::from(height) > MAX_MASK_PIXELS;
    if width == 0 || height == 0 || too_big {
        return Err(RenderError::InvalidSize(width.max(height)));
    }
    let narrow = |side: u32| {
        if side > MAX_MASK_SIDE {
            return Err(RenderError::InvalidSize(side));
        }
        u16::try_from(side).map_err(|_| RenderError::InvalidSize(side))
    };
    Ok((narrow(width)?, narrow(height)?))
}

/// The length of `p` taken as a vector, with IEEE-exact operations only.
fn length(p: Point) -> f64 {
    (p.x * p.x + p.y * p.y).sqrt()
}

/// `shape` with its curves replaced by chords within `tolerance` of them.
///
/// Each curve is cut into `n` equal steps of its parameter: a curve whose
/// second derivative is at most `m` long strays at most `m / (8 n²)` from
/// such chords, so `n = ceil(sqrt(m / (8 tolerance)))`. Only IEEE-exact
/// operations are used (no `hypot` or other library maths, whose last bit
/// varies between platforms), so the chords, and the coverage, are the same
/// on every platform.
fn flatten(shape: &BezPath, tolerance: f64) -> BezPath {
    let chords = |m: f64| (m / (8.0 * tolerance)).sqrt().ceil().clamp(1.0, MAX_CHORDS) as usize;
    let mut flat = BezPath::new();
    let mut at = Point::ZERO;
    let mut start = Point::ZERO;
    for element in shape.elements() {
        match *element {
            PathEl::MoveTo(p) => {
                flat.move_to(p);
                (at, start) = (p, p);
            }
            PathEl::LineTo(p) => {
                flat.line_to(p);
                at = p;
            }
            PathEl::QuadTo(p1, p2) => {
                let p0 = at;
                let bend = (p0.to_vec2() - 2.0 * p1.to_vec2() + p2.to_vec2()).to_point();
                let n = chords(2.0 * length(bend));
                for k in 1..n {
                    let t = k as f64 / n as f64;
                    let u = 1.0 - t;
                    let (a, b, c) = (u * u, 2.0 * u * t, t * t);
                    flat.line_to(Point::new(
                        a * p0.x + b * p1.x + c * p2.x,
                        a * p0.y + b * p1.y + c * p2.y,
                    ));
                }
                flat.line_to(p2);
                at = p2;
            }
            PathEl::CurveTo(p1, p2, p3) => {
                let p0 = at;
                let bend0 = (p0.to_vec2() - 2.0 * p1.to_vec2() + p2.to_vec2()).to_point();
                let bend1 = (p1.to_vec2() - 2.0 * p2.to_vec2() + p3.to_vec2()).to_point();
                let n = chords(6.0 * length(bend0).max(length(bend1)));
                for k in 1..n {
                    let t = k as f64 / n as f64;
                    let u = 1.0 - t;
                    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    flat.line_to(Point::new(
                        a * p0.x + b * p1.x + c * p2.x + d * p3.x,
                        a * p0.y + b * p1.y + c * p2.y + d * p3.y,
                    ));
                }
                flat.line_to(p3);
                at = p3;
            }
            PathEl::ClosePath => {
                flat.close_path();
                at = start;
            }
        }
    }
    flat
}

/// Rasterizes `paths` (font units, y up) into a [`Mask`] of
/// `width` x `height` pixels.
///
/// `to_pixels` maps font units to pixels with y pointing down
/// (`x' = xx*x + yx*y + dx`, `y' = xy*x + yy*y + dy`). Pixel `(i, j)` is the
/// square `[i, i+1) x [j, j+1)` in the mapped coordinates, so its centre is
/// `(i + 0.5, j + 0.5)`; row 0 is the top. Contours are filled together with
/// the non-zero rule, using the same single-threaded, baseline-SIMD
/// `vello_cpu` rasterizer as the PNG previews, so equal inputs give equal
/// values on any call. Curves are first flattened in pixel space to within
/// a hundredth of a pixel, so the coverage of a curved edge is as exact as
/// that of a straight one; they are never converted to quadratics.
///
/// # Errors
///
/// * [`RenderError::InvalidSize`] if a dimension is 0 or above 65 535, or
///   `width * height` exceeds 50 000 000;
/// * [`RenderError::Geometry`] if a contour cannot be converted to a curve or
///   `to_pixels` has a non-finite coefficient.
pub fn coverage_mask(
    paths: &[Path],
    to_pixels: &Transform,
    width: u32,
    height: u32,
) -> Result<Mask, RenderError> {
    let (w, h) = check_dimensions(width, height)?;
    let coeffs = [
        to_pixels.xx,
        to_pixels.xy,
        to_pixels.yx,
        to_pixels.yy,
        to_pixels.dx,
        to_pixels.dy,
    ];
    if !coeffs.iter().all(|c| c.is_finite()) {
        return Err(RenderError::Geometry(
            "the pixel transform is not finite".to_owned(),
        ));
    }
    let shape: BezPath = Affine::new(coeffs) * outline("mask", paths)?;
    let flat = flatten(&shape, FLATTEN_TOLERANCE_PX);
    let mut canvas = Canvas::new(w, h);
    canvas.fill(&flat, Affine::IDENTITY, crate::draw::BLACK);
    let data = canvas.finish_alpha();
    Ok(Mask {
        width,
        height,
        data,
    })
}

/// The intersection over union of two masks: the pixels with coverage
/// `>= threshold` in both, over those with it in either. 1.0 if both are
/// empty. 128 is the usual threshold.
///
/// # Errors
///
/// [`RenderError::Geometry`] if the dimensions differ.
pub fn mask_iou(a: &Mask, b: &Mask, threshold: u8) -> Result<f64, RenderError> {
    if (a.width, a.height) != (b.width, b.height) || a.data.len() != b.data.len() {
        return Err(RenderError::Geometry(format!(
            "mask sizes differ: {}x{} vs {}x{}",
            a.width, a.height, b.width, b.height
        )));
    }
    let (mut both, mut either) = (0u64, 0u64);
    for (&x, &y) in a.data.iter().zip(&b.data) {
        let (in_a, in_b) = (x >= threshold, y >= threshold);
        both += u64::from(in_a && in_b);
        either += u64::from(in_a || in_b);
    }
    if either == 0 {
        return Ok(1.0);
    }
    Ok(both as f64 / either as f64)
}
