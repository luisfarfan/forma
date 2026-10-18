//! Deterministic degradations: JPEG at quality 70, a Gaussian blur with
//! 4 % noise, and a 1.5 degree rotation. All integer or IEEE-exact
//! arithmetic with fixed seeds, so the bytes are the same on every platform.

use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbImage};

use crate::corpus::Truth;
use crate::det::{Rng, exp_neg};
use crate::error::BenchError;
use crate::scene::Scene;

/// The JPEG quality of [`Degradation::Jpeg70`].
pub const JPEG_QUALITY: u8 = 70;
/// The blur of [`Degradation::BlurNoise`], as a standard deviation in pixels.
pub const BLUR_SIGMA: f64 = 1.5;
/// The noise of [`Degradation::BlurNoise`], as a fraction of the full range.
pub const NOISE_FRACTION: f64 = 0.04;
/// The rotation of [`Degradation::Rotate15`], in degrees.
pub const ROTATION_DEGREES: f64 = 1.5;
/// The seed of the noise.
const NOISE_SEED: u64 = 0x464F_524D_4101;

/// A degradation of the clean render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Degradation {
    /// The clean image.
    None,
    /// Compressed as JPEG, quality 70.
    Jpeg70,
    /// Gaussian blur (sigma 1.5) and 4 % noise.
    BlurNoise,
    /// Rotated 1.5 degrees.
    Rotate15,
}

impl Degradation {
    /// Every degradation, in the order of the case ids.
    pub const ALL: [Degradation; 4] = [
        Degradation::None,
        Degradation::Jpeg70,
        Degradation::BlurNoise,
        Degradation::Rotate15,
    ];

    /// The name used in case ids.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Degradation::None => "none",
            Degradation::Jpeg70 => "jpeg70",
            Degradation::BlurNoise => "blurnoise",
            Degradation::Rotate15 => "rot15",
        }
    }

    /// Parses the name used in case ids.
    #[must_use]
    pub fn parse(text: &str) -> Option<Degradation> {
        Self::ALL.into_iter().find(|d| d.name() == text)
    }
}

/// The encoded image of a scene under a degradation, with its extension and
/// its truth.
///
/// # Errors
///
/// Any error of rendering or encoding.
pub fn finish(
    scene: Scene,
    degradation: Degradation,
) -> Result<(Vec<u8>, &'static str, Truth), BenchError> {
    let scene = match degradation {
        Degradation::Rotate15 => scene.rotated(ROTATION_DEGREES),
        _ => scene,
    };
    let truth = scene.truth()?;
    let mut img = scene.render()?;
    match degradation {
        Degradation::None | Degradation::Rotate15 => Ok((encode_png(&img)?, "png", truth)),
        Degradation::Jpeg70 => Ok((encode_jpeg(&img, JPEG_QUALITY)?, "jpg", truth)),
        Degradation::BlurNoise => {
            img = blur_rgb(&img, BLUR_SIGMA);
            add_noise(&mut img, NOISE_FRACTION, NOISE_SEED);
            Ok((encode_png(&img)?, "png", truth))
        }
    }
}

/// PNG bytes of `img` (default encoder settings, no metadata).
///
/// # Errors
///
/// [`BenchError::Image`] if the encoder fails.
pub fn encode_png(img: &RgbImage) -> Result<Vec<u8>, BenchError> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgb8,
        )
        .map_err(|e| BenchError::Image(e.to_string()))?;
    Ok(bytes)
}

/// JPEG bytes of `img` at `quality`.
///
/// # Errors
///
/// [`BenchError::Image`] if the encoder fails.
pub fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>, BenchError> {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgb8,
        )
        .map_err(|e| BenchError::Image(e.to_string()))?;
    Ok(bytes)
}

/// The integer weights (summing to exactly 65536) of a Gaussian kernel.
fn kernel(sigma: f64) -> Vec<i64> {
    let radius = (3.0 * sigma).ceil() as i64;
    let raw: Vec<f64> = (-radius..=radius)
        .map(|i| exp_neg((i * i) as f64 / (2.0 * sigma * sigma)))
        .collect();
    let total: f64 = raw.iter().sum();
    let mut weights: Vec<i64> = raw
        .iter()
        .map(|w| (w / total * 65536.0).round() as i64)
        .collect();
    let sum: i64 = weights.iter().sum();
    weights[radius as usize] += 65536 - sum;
    weights
}

/// A separable Gaussian blur of one channel, with clamped edges.
#[must_use]
pub fn blur_gray(data: &[u8], width: usize, height: usize, sigma: f64) -> Vec<u8> {
    let weights = kernel(sigma);
    let radius = (weights.len() / 2) as i64;
    let mut tmp = vec![0u8; data.len()];
    for y in 0..height {
        for x in 0..width {
            let mut acc = 0i64;
            for (k, w) in weights.iter().enumerate() {
                let sx = (x as i64 + k as i64 - radius).clamp(0, width as i64 - 1) as usize;
                acc += w * i64::from(data[y * width + sx]);
            }
            tmp[y * width + x] = ((acc + 32768) >> 16) as u8;
        }
    }
    let mut out = vec![0u8; data.len()];
    for y in 0..height {
        for x in 0..width {
            let mut acc = 0i64;
            for (k, w) in weights.iter().enumerate() {
                let sy = (y as i64 + k as i64 - radius).clamp(0, height as i64 - 1) as usize;
                acc += w * i64::from(tmp[sy * width + x]);
            }
            out[y * width + x] = ((acc + 32768) >> 16) as u8;
        }
    }
    out
}

/// A Gaussian blur of an RGB image.
#[must_use]
pub fn blur_rgb(img: &RgbImage, sigma: f64) -> RgbImage {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let raw = img.as_raw();
    let channels: Vec<Vec<u8>> = (0..3)
        .map(|k| {
            let plane: Vec<u8> = raw.iter().skip(k).step_by(3).copied().collect();
            blur_gray(&plane, w, h, sigma)
        })
        .collect();
    let mut out = Vec::with_capacity(raw.len());
    for ((r, g), b) in channels[0].iter().zip(&channels[1]).zip(&channels[2]) {
        out.extend([*r, *g, *b]);
    }
    RgbImage::from_raw(img.width(), img.height(), out).unwrap_or_else(|| img.clone())
}

/// Adds uniform noise of `fraction` of the full range to every channel.
pub fn add_noise(img: &mut RgbImage, fraction: f64, seed: u64) {
    let amplitude = (fraction * 255.0).round() as i32;
    let mut rng = Rng::new(seed);
    for value in img.as_mut() {
        let delta = rng.below(2 * amplitude as u64 + 1) as i32 - amplitude;
        *value = (i32::from(*value) + delta).clamp(0, 255) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_sums_to_one() {
        assert_eq!(kernel(BLUR_SIGMA).iter().sum::<i64>(), 65536);
    }

    #[test]
    fn blur_keeps_flat_areas() {
        let flat = vec![200u8; 20 * 20];
        assert!(blur_gray(&flat, 20, 20, 1.5).iter().all(|&v| v == 200));
    }

    #[test]
    fn names_round_trip() {
        for d in Degradation::ALL {
            assert_eq!(Degradation::parse(d.name()), Some(d));
        }
    }
}
