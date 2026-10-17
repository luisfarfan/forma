//! Image decoding with size limits checked before any pixel is decoded.

use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits, RgbImage};

use crate::TraceError;

/// The largest encoded image accepted, in bytes (64 MiB).
pub const MAX_IMAGE_BYTES: u64 = 67_108_864;
/// The most pixels an image may declare.
pub const MAX_IMAGE_PIXELS: u64 = 50_000_000;

/// Decodes a PNG or JPEG into RGB. Transparent pixels are composited over
/// white.
///
/// The declared dimensions are read from the header and checked against
/// [`MAX_IMAGE_PIXELS`] before any pixel memory is reserved.
///
/// # Errors
///
/// [`TraceError::TooLarge`] for oversized input, [`TraceError::Decode`] for
/// anything that is not a valid PNG or JPEG.
pub fn decode(bytes: &[u8]) -> Result<RgbImage, TraceError> {
    let len = bytes.len() as u64;
    if len > MAX_IMAGE_BYTES {
        return Err(TraceError::TooLarge {
            bytes: len,
            pixels: 0,
        });
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| TraceError::Decode(e.to_string()))?;
    match reader.format() {
        Some(ImageFormat::Png | ImageFormat::Jpeg) => {}
        Some(other) => {
            return Err(TraceError::Decode(format!(
                "unsupported format {other:?}: only PNG and JPEG are accepted"
            )));
        }
        None => return Err(TraceError::Decode("unrecognized image format".into())),
    }
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| TraceError::Decode(e.to_string()))?;
    let pixels = u64::from(w) * u64::from(h);
    if pixels > MAX_IMAGE_PIXELS {
        return Err(TraceError::TooLarge { bytes: len, pixels });
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| TraceError::Decode(e.to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(w);
    limits.max_image_height = Some(h);
    limits.max_alloc = Some(MAX_IMAGE_PIXELS * 8);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| TraceError::Decode(e.to_string()))?;
    let rgba = img.to_rgba8();
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (dst, src) in out.pixels_mut().zip(rgba.pixels()) {
        let a = u32::from(src.0[3]);
        for i in 0..3 {
            let c = u32::from(src.0[i]);
            dst.0[i] = ((c * a + 255 * (255 - a) + 127) / 255) as u8;
        }
    }
    Ok(out)
}
