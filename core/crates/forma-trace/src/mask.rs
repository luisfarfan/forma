//! The ink mask: soft membership (the ink coverage of each pixel),
//! upsampling and a 50 % threshold.

use forma_render::{MAX_MASK_PIXELS, Mask};
use image::RgbImage;

use crate::palette::{dist2, lab, to_f};

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

/// The background and ink colours a pixel is weighed against: encoded sRGB
/// (where anti-aliasing mixes them) and Lab (where the tolerance is measured).
struct Mix {
    bg: [f64; 3],
    ink: [f64; 3],
    bg_lab: [f64; 3],
    ink_lab: [f64; 3],
    /// The Lab distance from the background to the ink.
    span: f64,
    tolerance: f64,
}

impl Mix {
    /// How much of `ink` a pixel holds, 0 to 1.
    ///
    /// Which colours count is decided in Lab: the ink within the tolerance,
    /// and any colour near the background-to-ink segment (an anti-aliased
    /// edge); any other colour is not ink (0). How much ink a counted pixel
    /// holds is its coverage: anti-aliasing mixes the ink into the background
    /// linearly in the encoded sRGB values, so the coverage is where the
    /// pixel projects onto the background-to-ink segment there, clamped to
    /// 0..1. Any other measure is biased at the 50 % threshold: a ratio of
    /// Lab distances puts it at about 45 % coverage, and counting every
    /// pixel within the tolerance as full ink puts 72 % coverage at 100 %,
    /// both of which move edges outwards.
    fn soft(&self, p: [u8; 3]) -> f32 {
        let p_lab = lab(p);
        let d_ink = dist2(p_lab, self.ink_lab).sqrt();
        let d_bg = dist2(p_lab, self.bg_lab).sqrt();
        if d_ink > self.tolerance && d_bg + d_ink > self.span + self.tolerance {
            return 0.0;
        }
        let (mut along, mut length2) = (0.0, 0.0);
        for ((c, bg), ink) in to_f(p).iter().zip(self.bg).zip(self.ink) {
            let v = ink - bg;
            along += (c - bg) * v;
            length2 += v * v;
        }
        if length2 <= 0.0 {
            return 1.0;
        }
        (along / length2).clamp(0.0, 1.0) as f32
    }
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
    let mix = Mix {
        bg: to_f(background),
        ink: to_f(ink),
        bg_lab: lab(background),
        ink_lab: lab(ink),
        span: dist2(lab(background), lab(ink)).sqrt(),
        tolerance,
    };
    let (mw, mh) = (w * u, h * u);
    if mix.span <= tolerance || w == 0 || h == 0 {
        return Mask {
            width: mw,
            height: mh,
            data: vec![0; mw as usize * mh as usize],
        };
    }
    let mut s = vec![0f32; w as usize * h as usize];
    for (i, px) in image.pixels().enumerate() {
        s[i] = mix.soft(px.0);
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
