//! Coverage masks: rasterized outlines as numbers, for comparing glyphs with
//! images.

use forma_model::{Path, Transform};
use vello_cpu::kurbo::{Affine, BezPath};

use crate::RenderError;
use crate::draw::{Canvas, outline};

/// The most pixels a [`Mask`] may have.
pub const MAX_MASK_PIXELS: u64 = 50_000_000;

/// The widest or tallest mask, in pixels (the renderer addresses pixels with
/// 16 bits).
const MAX_MASK_SIDE: u32 = u16::MAX as u32;

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

/// Rasterizes `paths` (font units, y up) into a [`Mask`] of
/// `width` x `height` pixels.
///
/// `to_pixels` maps font units to pixels with y pointing down
/// (`x' = xx*x + yx*y + dx`, `y' = xy*x + yy*y + dy`). Pixel `(i, j)` is the
/// square `[i, i+1) x [j, j+1)` in the mapped coordinates, so its centre is
/// `(i + 0.5, j + 0.5)`; row 0 is the top. Contours are filled together with
/// the non-zero rule, using the same single-threaded, baseline-SIMD
/// `vello_cpu` rasterizer as the PNG previews, so equal inputs give equal
/// values on any call. Cubic curves are never converted to quadratics.
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
    let shape: BezPath = outline("mask", paths)?;
    let mut canvas = Canvas::new(w, h);
    canvas.fill(&shape, Affine::new(coeffs), crate::draw::BLACK);
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
