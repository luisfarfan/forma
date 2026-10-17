//! The ink mask: soft membership, upsampling and a 50 % threshold.

use forma_render::{MAX_MASK_PIXELS, Mask};
use image::RgbImage;

use crate::palette::{dist2, lab};

/// The largest mask side accepted by the renderer.
const MAX_SIDE: u64 = 65_535;

/// The upsampling factor actually used: `requested` reduced until the mask
/// fits [`MAX_MASK_PIXELS`] and the side limit (never below 1).
pub(crate) fn effective_upsample(width: u32, height: u32, requested: u32) -> u32 {
    let mut u = requested.clamp(1, 16);
    while u > 1 {
        let (w, h) = (
            u64::from(width) * u64::from(u),
            u64::from(height) * u64::from(u),
        );
        if w * h <= MAX_MASK_PIXELS && w <= MAX_SIDE && h <= MAX_SIDE {
            break;
        }
        u -= 1;
    }
    u
}

/// How much of `ink` a pixel holds, 0 to 1: full inside the tolerance of the
/// ink; for other pixels near the background-to-ink segment (an anti-aliased
/// edge) the distance to the background over the sum of both distances; zero
/// for any other color.
fn soft(p: [f64; 3], bg: [f64; 3], ink: [f64; 3], span: f64, tolerance: f64) -> f32 {
    let d_ink = dist2(p, ink).sqrt();
    if d_ink <= tolerance {
        return 1.0;
    }
    let d_bg = dist2(p, bg).sqrt();
    if d_bg + d_ink > span + tolerance {
        return 0.0;
    }
    (d_bg / (d_bg + d_ink)) as f32
}

/// Binary ink mask (0 or 255) at `upsample` times the image size (reduced if
/// needed; see [`Mask`] dimensions). The soft membership is upsampled
/// bilinearly and thresholded at 50 %.
pub(crate) fn binary_mask(
    image: &RgbImage,
    background: [u8; 3],
    ink: [u8; 3],
    tolerance: f64,
    upsample: u32,
) -> Mask {
    let (w, h) = image.dimensions();
    let u = effective_upsample(w, h, upsample);
    let bg = lab(background);
    let ink_lab = lab(ink);
    let span = dist2(bg, ink_lab).sqrt();
    let (mw, mh) = (w * u, h * u);
    if span <= tolerance || w == 0 || h == 0 {
        return Mask {
            width: mw,
            height: mh,
            data: vec![0; mw as usize * mh as usize],
        };
    }
    let mut s = vec![0f32; w as usize * h as usize];
    for (i, px) in image.pixels().enumerate() {
        s[i] = soft(lab(px.0), bg, ink_lab, span, tolerance);
    }
    let at = |x: i64, y: i64| -> f32 {
        let xi = x.clamp(0, i64::from(w) - 1) as usize;
        let yi = y.clamp(0, i64::from(h) - 1) as usize;
        s[yi * w as usize + xi]
    };
    let uf = u as f32;
    let mut data = vec![0u8; mw as usize * mh as usize];
    for my in 0..mh {
        let sy = (my as f32 + 0.5) / uf - 0.5;
        let y0 = sy.floor();
        let fy = sy - y0;
        let y0 = y0 as i64;
        for mx in 0..mw {
            let sx = (mx as f32 + 0.5) / uf - 0.5;
            let x0 = sx.floor();
            let fx = sx - x0;
            let x0 = x0 as i64;
            let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
            let bot = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
            let v = top * (1.0 - fy) + bot * fy;
            if v >= 0.5 {
                data[my as usize * mw as usize + mx as usize] = 255;
            }
        }
    }
    Mask {
        width: mw,
        height: mh,
        data,
    }
}
