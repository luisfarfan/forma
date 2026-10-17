use forma_trace::{TraceError, TraceOptions, analyze, annotated_png, decode};
use image::{Rgb, RgbImage};

const ORANGE: [u8; 3] = [0xFF, 0x5A, 0x1F];
const BLUE: [u8; 3] = [0x2A, 0x5B, 0xFF];

fn canvas(w: u32, h: u32) -> RgbImage {
    RgbImage::from_pixel(w, h, Rgb([0, 0, 0]))
}

fn rect(img: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 3]) {
    for y in y0..y1 {
        for x in x0..x1 {
            img.put_pixel(x, y, Rgb(c));
        }
    }
}

fn disc(img: &mut RgbImage, cx: f64, cy: f64, r: f64, c: [u8; 3]) {
    for y in 0..img.height() {
        for x in 0..img.width() {
            let (dx, dy) = (f64::from(x) + 0.5 - cx, f64::from(y) + 0.5 - cy);
            if dx * dx + dy * dy <= r * r {
                img.put_pixel(x, y, Rgb(c));
            }
        }
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn png_header(w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = b"IHDR".to_vec();
    chunk.extend(w.to_be_bytes());
    chunk.extend(h.to_be_bytes());
    chunk.extend([8, 2, 0, 0, 0]);
    out.extend(13u32.to_be_bytes());
    out.extend(&chunk);
    out.extend(crc32(&chunk).to_be_bytes());
    // The PNG reader only reports dimensions once it reaches the first IDAT.
    out.extend(0u32.to_be_bytes());
    out.extend(b"IDAT");
    out.extend(crc32(b"IDAT").to_be_bytes());
    out
}

#[test]
fn decode_rejects_oversized_dimensions() {
    let err = decode(&png_header(10_000, 10_000)).unwrap_err();
    assert!(matches!(err, TraceError::TooLarge { .. }), "{err:?}");
}

#[test]
fn decode_rejects_garbage() {
    let garbage: Vec<u8> = (0..300u32).map(|i| (i * 37 + 11) as u8).collect();
    assert!(matches!(decode(&garbage), Err(TraceError::Decode(_))));
}

#[test]
fn decode_round_trips_png() {
    let mut img = canvas(20, 10);
    rect(&mut img, 2, 2, 8, 8, ORANGE);
    let mut bytes = std::io::Cursor::new(Vec::new());
    img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    assert_eq!(decode(bytes.get_ref()).unwrap(), img);
}

fn three_rects() -> RgbImage {
    let mut img = canvas(200, 100);
    rect(&mut img, 10, 20, 40, 70, ORANGE);
    rect(&mut img, 60, 20, 90, 70, ORANGE);
    rect(&mut img, 110, 20, 140, 70, ORANGE);
    img
}

#[test]
fn palette_finds_background_and_ink() {
    let mut img = three_rects();
    disc(&mut img, 170.0, 50.0, 12.0, BLUE);
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    assert_eq!(a.background, [0, 0, 0]);
    for (got, want) in a.ink.iter().zip(ORANGE) {
        assert!(got.abs_diff(want) <= 12, "{:?}", a.ink);
    }
}

#[test]
fn blue_distractor_is_ignored() {
    let mut img = three_rects();
    disc(&mut img, 170.0, 50.0, 12.0, BLUE);
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    assert_eq!(a.candidates.len(), 3);
    assert!(a.note.is_none());
}

#[test]
fn rows_and_order() {
    let mut img = canvas(200, 200);
    for (r, (y0, y1)) in [(20u32, 60u32), (110, 150)].into_iter().enumerate() {
        for k in 0..3u32 {
            let x0 = 15 + k * 60;
            // Different widths so left-to-right is observable.
            rect(&mut img, x0, y0, x0 + 20 + 5 * k, y1, ORANGE);
        }
        let _ = r;
    }
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    assert_eq!(a.candidates.len(), 6);
    assert_eq!(a.rows.len(), 2);
    for (i, c) in a.candidates.iter().enumerate() {
        assert_eq!(c.index, i);
        assert_eq!(c.row, i / 3);
    }
    assert!(a.candidates[0].bbox.x0 < a.candidates[1].bbox.x0);
    assert!(a.candidates[1].bbox.x0 < a.candidates[2].bbox.x0);
    assert!((a.rows[0].baseline_px - 60.0).abs() <= 1.0);
    assert!((a.rows[1].baseline_px - 150.0).abs() <= 1.0);
    assert!((a.rows[0].cap_px - 20.0).abs() <= 1.0);
}

#[test]
fn pieces_merge_into_one_candidate() {
    let mut img = canvas(100, 100);
    rect(&mut img, 40, 40, 54, 85, ORANGE);
    rect(&mut img, 40, 22, 54, 34, ORANGE);
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    assert_eq!(a.candidates.len(), 1);
    let b = a.candidates[0].bbox;
    assert!(b.y0 <= 22 && b.y1 >= 85);
}

#[test]
fn touching_shapes_are_one_candidate() {
    let mut img = canvas(100, 60);
    rect(&mut img, 20, 15, 40, 35, ORANGE);
    rect(&mut img, 40, 15, 60, 35, ORANGE);
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    assert_eq!(a.candidates.len(), 1);
    assert_eq!(a.candidates[0].bbox.x1 - a.candidates[0].bbox.x0, 40);
}

#[test]
fn no_ink_gives_note() {
    let img = RgbImage::from_pixel(50, 50, Rgb([240, 240, 240]));
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    assert!(a.candidates.is_empty());
    assert!(a.note.is_some_and(|n| n.starts_with("no ink found")));
}

#[test]
fn analysis_is_deterministic() {
    let mut img = three_rects();
    disc(&mut img, 170.0, 50.0, 12.0, BLUE);
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    let b = analyze(&img, &TraceOptions::default()).unwrap();
    assert_eq!(a, b);
}

#[test]
fn annotated_png_is_a_png() {
    let img = three_rects();
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    let png = annotated_png(&img, &a).unwrap();
    let back = decode(&png).unwrap();
    assert_eq!(back.dimensions(), img.dimensions());
    assert_ne!(back, img);
}
