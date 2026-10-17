//! The annotated preview: candidate boxes, their numbers and the row lines,
//! drawn with built-in 5x7 digits (no fonts).

use std::io::Cursor;

use image::{ImageFormat, Rgb, RgbImage};

use crate::{Analysis, TraceError};

/// 5x7 digit bitmaps, one `u8` per row, bit 4 = leftmost column.
const DIGITS: [[u8; 7]; 10] = [
    [
        0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
    ],
    [
        0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
    ],
    [
        0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
    ],
    [
        0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
    ],
    [
        0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
    ],
    [
        0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
    ],
    [
        0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
    ],
    [
        0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
    ],
    [
        0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
    ],
    [
        0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
    ],
];

const BOX: Rgb<u8> = Rgb([0, 255, 0]);
const TEXT: Rgb<u8> = Rgb([255, 255, 0]);
const BASELINE: Rgb<u8> = Rgb([0, 200, 255]);
const CAP: Rgb<u8> = Rgb([255, 0, 255]);

fn put(img: &mut RgbImage, x: i64, y: i64, c: Rgb<u8>) {
    if x >= 0
        && y >= 0
        && (x as u64) < u64::from(img.width())
        && (y as u64) < u64::from(img.height())
    {
        img.put_pixel(x as u32, y as u32, c);
    }
}

fn number(img: &mut RgbImage, n: usize, x: i64, y: i64, scale: i64) {
    let mut cx = x;
    for ch in n.to_string().bytes() {
        let glyph = &DIGITS[usize::from(ch - b'0')];
        for (ry, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits & (0b10000 >> col) != 0 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            put(img, cx + col * scale + sx, y + ry as i64 * scale + sy, TEXT);
                        }
                    }
                }
            }
        }
        cx += 6 * scale;
    }
}

/// PNG of `image` with each candidate's box and number plus the cap and
/// baseline lines of every row.
///
/// # Errors
///
/// [`TraceError::Render`] if the PNG cannot be encoded.
pub fn annotated_png(image: &RgbImage, analysis: &Analysis) -> Result<Vec<u8>, TraceError> {
    let mut img = image.clone();
    for row in &analysis.rows {
        for x in 0..i64::from(img.width()) {
            put(&mut img, x, row.baseline_px.round() as i64, BASELINE);
            put(&mut img, x, row.cap_px.round() as i64, CAP);
        }
    }
    for c in &analysis.candidates {
        let (x0, y0) = (i64::from(c.bbox.x0), i64::from(c.bbox.y0));
        let (x1, y1) = (i64::from(c.bbox.x1) - 1, i64::from(c.bbox.y1) - 1);
        for x in x0..=x1 {
            put(&mut img, x, y0, BOX);
            put(&mut img, x, y1, BOX);
        }
        for y in y0..=y1 {
            put(&mut img, x0, y, BOX);
            put(&mut img, x1, y, BOX);
        }
        let scale = 2;
        let ty = if y0 >= 7 * scale + 2 {
            y0 - 7 * scale - 1
        } else {
            y0 + 2
        };
        number(&mut img, c.index, x0 + 1, ty, scale);
    }
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)
        .map_err(|e| TraceError::Render(e.to_string()))?;
    Ok(out.into_inner())
}
