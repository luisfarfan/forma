use forma_trace::{MAX_IMAGE_BYTES, TraceError, TraceOptions, analyze, annotated_png, decode};
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
    for (y0, y1) in [(20u32, 60u32), (110, 150)] {
        for k in 0..3u32 {
            let x0 = 15 + k * 60;
            // Different widths so left-to-right is observable.
            rect(&mut img, x0, y0, x0 + 20 + 5 * k, y1, ORANGE);
        }
    }
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    assert_eq!(a.candidates.len(), 6);
    assert_eq!(a.rows.len(), 2);
    for (i, c) in a.candidates.iter().enumerate() {
        assert_eq!(c.index, i);
        assert_eq!(c.row, i / 3);
        let (y0, y1) = if i < 3 { (20, 60) } else { (110, 150) };
        let x0 = 15 + (i as u32 % 3) * 60;
        let w = 20 + 5 * (i as u32 % 3);
        assert_eq!(
            (c.bbox.x0, c.bbox.y0, c.bbox.x1, c.bbox.y1),
            (x0, y0, x0 + w, y1)
        );
        assert_eq!(c.area, u64::from(w * (y1 - y0)));
    }
    assert!((a.rows[0].baseline_px - 60.0).abs() <= 1.0);
    assert!((a.rows[1].baseline_px - 150.0).abs() <= 1.0);
    assert!((a.rows[0].cap_px - 20.0).abs() <= 1.0);
    assert!((a.rows[1].cap_px - 110.0).abs() <= 1.0);
}

/// Three tall bars give the row context (the dot of an "i" overlaps the tall
/// bar's vertical band).
fn with_context(img: &mut RgbImage) {
    rect(img, 10, 20, 24, 85, ORANGE);
    rect(img, 150, 20, 164, 85, ORANGE);
    rect(img, 180, 20, 194, 85, ORANGE);
}

fn dims(img: &RgbImage) -> RgbImage {
    img.clone()
}

#[test]
fn pieces_merge_into_one_candidate() {
    let mut img = canvas(220, 100);
    with_context(&mut img);
    rect(&mut img, 80, 40, 94, 85, ORANGE);
    rect(&mut img, 80, 22, 94, 34, ORANGE);
    let a = analyze(&dims(&img), &TraceOptions::default()).unwrap();
    assert_eq!(a.candidates.len(), 4);
    let b = a.candidates[1].bbox;
    assert_eq!((b.x0, b.y0, b.x1, b.y1), (80, 22, 94, 85));
}

#[test]
fn colon_and_equals_are_one_candidate_each() {
    let mut img = canvas(220, 100);
    with_context(&mut img);
    rect(&mut img, 60, 30, 72, 42, ORANGE);
    rect(&mut img, 60, 60, 72, 72, ORANGE);
    rect(&mut img, 100, 40, 130, 47, ORANGE);
    rect(&mut img, 100, 58, 130, 65, ORANGE);
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    assert_eq!(a.candidates.len(), 5, "{:?}", a.candidates);
    let colon = a.candidates[1].bbox;
    assert_eq!((colon.x0, colon.y0, colon.x1, colon.y1), (60, 30, 72, 72));
    let equals = a.candidates[2].bbox;
    assert_eq!(
        (equals.x0, equals.y0, equals.x1, equals.y1),
        (100, 40, 130, 65)
    );
}

#[test]
fn stacked_rows_never_merge() {
    let mut img = canvas(100, 160);
    for y0 in [10u32, 60, 110] {
        rect(&mut img, 30, y0, 60, y0 + 30, ORANGE);
    }
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    assert_eq!(a.candidates.len(), 3);
    assert_eq!(a.rows.len(), 3);
    assert_eq!(
        a.candidates.iter().map(|c| c.row).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[test]
fn noise_speck_is_dropped_and_micro_speck_is_ignored() {
    let mut img = three_rects();
    // About 1.7 % of a rectangle: a speck between letters, dropped at the end.
    rect(&mut img, 45, 40, 50, 45, ORANGE);
    // A single pixel right above the first rectangle: micro-noise.
    rect(&mut img, 20, 15, 21, 16, ORANGE);
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    assert_eq!(a.candidates.len(), 3);
    let b = a.candidates[0].bbox;
    assert_eq!((b.x0, b.y0, b.x1, b.y1), (10, 20, 40, 70));
}

#[test]
fn explicit_color_selects_that_ink() {
    let mut img = three_rects();
    disc(&mut img, 170.0, 50.0, 12.0, BLUE);
    let opts = TraceOptions {
        color: Some(BLUE),
        ..TraceOptions::default()
    };
    let a = analyze(&img, &opts).unwrap();
    assert_eq!(a.ink, BLUE);
    assert_eq!(a.candidates.len(), 1);
}

#[test]
fn too_many_candidates() {
    let mut img = canvas(320, 220);
    for r in 0..20u32 {
        for c in 0..30u32 {
            rect(
                &mut img,
                5 + c * 10,
                5 + r * 10,
                10 + c * 10,
                10 + r * 10,
                ORANGE,
            );
        }
    }
    assert!(matches!(
        analyze(&img, &TraceOptions::default()),
        Err(TraceError::TooManyCandidates(600))
    ));
}

#[test]
fn decode_rejects_too_many_bytes() {
    let big = vec![0u8; MAX_IMAGE_BYTES as usize + 1];
    assert!(matches!(decode(&big), Err(TraceError::TooLarge { .. })));
}

fn encode(img: &image::DynamicImage, f: image::ImageFormat) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    img.write_to(&mut bytes, f).unwrap();
    bytes.into_inner()
}

#[test]
fn decode_jpeg() {
    let mut img = canvas(32, 32);
    rect(&mut img, 8, 8, 24, 24, ORANGE);
    let bytes = encode(
        &image::DynamicImage::ImageRgb8(img),
        image::ImageFormat::Jpeg,
    );
    let back = decode(&bytes).unwrap();
    assert_eq!(back.dimensions(), (32, 32));
    assert!(back.get_pixel(16, 16).0[0] > 200);
}

#[test]
fn decode_grayscale_png() {
    let g = image::GrayImage::from_pixel(4, 4, image::Luma([77]));
    let bytes = encode(&image::DynamicImage::ImageLuma8(g), image::ImageFormat::Png);
    assert_eq!(decode(&bytes).unwrap().get_pixel(1, 1).0, [77, 77, 77]);
}

#[test]
fn decode_other_formats_are_rejected() {
    let mut gif = b"GIF89a".to_vec();
    gif.extend([0u8; 64]);
    assert!(matches!(decode(&gif), Err(TraceError::Decode(_))));
}

#[test]
fn transparent_png_is_composited_over_white() {
    let mut rgba = image::RgbaImage::from_pixel(120, 60, image::Rgba([0, 0, 0, 0]));
    for (x0, x1) in [(10u32, 30u32), (50, 70), (90, 110)] {
        for y in 10..50 {
            for x in x0..x1 {
                rgba.put_pixel(x, y, image::Rgba([ORANGE[0], ORANGE[1], ORANGE[2], 255]));
            }
        }
    }
    let bytes = encode(
        &image::DynamicImage::ImageRgba8(rgba),
        image::ImageFormat::Png,
    );
    let img = decode(&bytes).unwrap();
    assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255]);
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    assert_eq!(a.background, [255, 255, 255]);
    for (got, want) in a.ink.iter().zip(ORANGE) {
        assert!(got.abs_diff(want) <= 12, "{:?}", a.ink);
    }
    assert_eq!(a.candidates.len(), 3);
}

#[test]
fn tiny_and_all_ink_images_do_not_panic() {
    let one = RgbImage::from_pixel(1, 1, Rgb(ORANGE));
    let a = analyze(&one, &TraceOptions::default()).unwrap();
    assert!(a.candidates.is_empty() && a.note.is_some());
    let all = RgbImage::from_pixel(40, 40, Rgb(ORANGE));
    let a = analyze(&all, &TraceOptions::default()).unwrap();
    assert!(a.candidates.is_empty() && a.note.is_some());
}

#[test]
fn annotated_png_draws_the_candidate_box() {
    let img = three_rects();
    let a = analyze(&img, &TraceOptions::default()).unwrap();
    let back = decode(&annotated_png(&img, &a).unwrap()).unwrap();
    // The top edge of the first box, away from the digits, is green.
    assert_eq!(back.get_pixel(38, 20).0, [0, 255, 0]);
    assert_eq!(img.get_pixel(38, 20).0, ORANGE);
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
