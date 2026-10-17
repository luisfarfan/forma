//! Vectorization, cleanup and fidelity of traced candidates.

use forma_geometry::{
    Cap, Skeleton, SkeletonNode, StrokeStyle, bbox, check_paths, signed_area, stroke,
};
use forma_model::{NodeKind, Path, Transform};
use forma_trace::{TraceError, TraceOptions, TracedGlyph, analyze, compare, trace};
use image::{Rgb, RgbImage};

const ORANGE: [u8; 3] = [0xFF, 0x5A, 0x1F];
const CAP: f64 = 700.0;
const UPM: u16 = 1000;

fn canvas(w: u32, h: u32) -> RgbImage {
    RgbImage::from_pixel(w, h, Rgb([0, 0, 0]))
}

fn rect(img: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32) {
    for y in y0..y1 {
        for x in x0..x1 {
            img.put_pixel(x, y, Rgb(ORANGE));
        }
    }
}

/// Supersampled coverage of the annulus between the two radii (0 to 1) at
/// pixel `(x, y)`.
fn ring_coverage(x: u32, y: u32, cx: f64, cy: f64, r_out: f64, r_in: f64, samples: u32) -> f64 {
    let mut inside = 0;
    for sy in 0..samples {
        for sx in 0..samples {
            let px = f64::from(x) + (f64::from(sx) + 0.5) / f64::from(samples);
            let py = f64::from(y) + (f64::from(sy) + 0.5) / f64::from(samples);
            let d2 = (px - cx).powi(2) + (py - cy).powi(2);
            if d2 <= r_out * r_out && d2 >= r_in * r_in {
                inside += 1;
            }
        }
    }
    f64::from(inside) / f64::from(samples * samples)
}

/// An anti-aliased ring (8 x 8 samples per pixel), like a rendered or
/// scanned drawing; `r_in = 0` gives a disc.
fn ring(img: &mut RgbImage, cx: f64, cy: f64, r_out: f64, r_in: f64) {
    paint_ring(img, cx, cy, r_out, r_in, 8);
}

/// A hard-edged ring: the pixels whose centre lies between the radii.
fn hard_ring(img: &mut RgbImage, cx: f64, cy: f64, r_out: f64, r_in: f64) {
    paint_ring(img, cx, cy, r_out, r_in, 1);
}

fn paint_ring(img: &mut RgbImage, cx: f64, cy: f64, r_out: f64, r_in: f64, samples: u32) {
    for y in 0..img.height() {
        for x in 0..img.width() {
            let c = ring_coverage(x, y, cx, cy, r_out, r_in, samples);
            if c > 0.0 {
                let blend = |v: u8| (f64::from(v) * c).round() as u8;
                img.put_pixel(x, y, Rgb(ORANGE.map(blend)));
            }
        }
    }
}

fn trace_one(img: &RgbImage) -> TracedGlyph {
    let opts = TraceOptions::default();
    let a = analyze(img, &opts).unwrap();
    assert_eq!(a.candidates.len(), 1, "{:?}", a.candidates);
    let mut traced = trace(img, &opts, &[0], CAP, UPM).unwrap();
    assert_eq!(traced.len(), 1);
    traced.remove(0)
}

fn on_curve(path: &Path) -> usize {
    path.nodes
        .iter()
        .filter(|n| n.kind != NodeKind::OffCurve)
        .count()
}

fn ring_image() -> RgbImage {
    let mut img = canvas(200, 200);
    ring(&mut img, 100.0, 100.0, 60.0, 30.0);
    img
}

fn rectangle_image() -> RgbImage {
    let mut img = canvas(200, 400);
    rect(&mut img, 40, 50, 160, 350);
    img
}

fn circle_image() -> RgbImage {
    let mut img = canvas(200, 200);
    ring(&mut img, 100.0, 100.0, 60.0, 0.0);
    img
}

#[test]
fn traced_ring_has_counter() {
    let g = trace_one(&ring_image());
    assert_eq!(g.paths.len(), 2, "{:?}", g.paths);
    let areas: Vec<f64> = g.paths.iter().map(signed_area).collect();
    let (outer, inner) = if areas[0].abs() > areas[1].abs() {
        (areas[0], areas[1])
    } else {
        (areas[1], areas[0])
    };
    assert!(outer > 0.0, "outer contour counter-clockwise: {areas:?}");
    assert!(inner < 0.0, "inner contour clockwise: {areas:?}");
    assert!(g.iou >= 0.99, "iou {}", g.iou);
    // An "O" must not be over-noded.
    for p in &g.paths {
        assert!(on_curve(p) <= 12, "{} on-curve nodes", on_curve(p));
    }
}

#[test]
fn traced_rectangle_is_four_lines() {
    let g = trace_one(&rectangle_image());
    assert_eq!(g.paths.len(), 1, "{:?}", g.paths);
    let nodes = &g.paths[0].nodes;
    assert_eq!(nodes.len(), 4, "{nodes:?}");
    assert!(nodes.iter().all(|n| n.kind == NodeKind::Line), "{nodes:?}");
    assert!(g.iou >= 0.99, "iou {}", g.iou);
}

#[test]
fn traced_circle_iou() {
    let g = trace_one(&circle_image());
    assert_eq!(g.paths.len(), 1);
    assert!(g.iou >= 0.99, "iou {}", g.iou);
    let n = on_curve(&g.paths[0]);
    assert!(n <= 12, "{n} on-curve nodes: {:?}", g.paths[0].nodes);
}

#[test]
fn hard_edged_pixels_are_not_traced_as_steps() {
    // Without anti-aliasing the edge is only known to half a pixel: the
    // steps must not become nodes. (Even the true circle scores about 0.996
    // against such a disc, and the true ring about 0.993.)
    let mut img = canvas(200, 200);
    hard_ring(&mut img, 100.0, 100.0, 60.0, 0.0);
    let g = trace_one(&img);
    assert_eq!(g.paths.len(), 1);
    assert!(g.iou >= 0.99, "iou {}", g.iou);
    assert!(on_curve(&g.paths[0]) <= 12, "{:?}", g.paths[0].nodes);
    assert!(check_paths(&g.paths, 1.0).is_empty());

    let mut img = canvas(200, 200);
    hard_ring(&mut img, 100.0, 100.0, 60.0, 30.0);
    let g = trace_one(&img);
    assert_eq!(g.paths.len(), 2);
    assert!(g.iou >= 0.98, "iou {}", g.iou);
    for p in &g.paths {
        assert!(on_curve(p) <= 12, "{} on-curve nodes", on_curve(p));
    }
    assert!(check_paths(&g.paths, 1.0).is_empty());
}

#[test]
fn geometry_is_clean_after_trace() {
    for img in [ring_image(), rectangle_image(), circle_image()] {
        let g = trace_one(&img);
        let issues = check_paths(&g.paths, 1.0);
        assert!(issues.is_empty(), "{issues:?}");
    }
}

#[test]
fn scale_follows_cap_height() {
    let mut img = canvas(300, 200);
    rect(&mut img, 50, 40, 110, 140);
    let g = trace_one(&img);
    let b = bbox(&g.paths).unwrap();
    assert!((b.height() - 700.0).abs() <= 7.0, "height {}", b.height());
    assert!((b.y0).abs() <= 1.0, "sits on the baseline: {}", b.y0);
    assert!((g.placement.scale - 7.0).abs() < 1e-9);
    // The ink starts at the left side bearing; the advance adds both bearings.
    let lsb = 50.0;
    assert_eq!(g.placement.lsb, lsb);
    assert!((b.x0 - lsb).abs() <= 1.0, "x0 {}", b.x0);
    assert!((g.ink_width - b.width()).abs() < 1e-9);
    assert!((g.advance_width - (g.ink_width + 2.0 * lsb)).abs() < 1e-9);
    assert_eq!(g.placement.origin_px, (50.0, 140.0));
}

#[test]
fn out_of_range_candidate_is_an_error() {
    let img = rectangle_image();
    let err = trace(&img, &TraceOptions::default(), &[0, 3], CAP, UPM).unwrap_err();
    assert_eq!(err, TraceError::CandidateOutOfRange { index: 3, count: 1 });
    let err = compare(&img, &TraceOptions::default(), 1, &[], CAP).unwrap_err();
    assert_eq!(err, TraceError::CandidateOutOfRange { index: 1, count: 1 });
}

#[test]
fn compare_agrees_with_trace_and_overlays() {
    let img = ring_image();
    let opts = TraceOptions::default();
    let g = trace_one(&img);
    let c = compare(&img, &opts, 0, &g.paths, CAP).unwrap();
    // `trace` places the contours exactly where it traced them; `compare`
    // can only align their left edge with the mask's, a fraction of a pixel
    // apart.
    assert!((c.iou - g.iou).abs() < 0.005, "{} vs {}", c.iou, g.iou);
    let overlay = image::load_from_memory(&c.overlay_png).unwrap().to_rgb8();
    let has = |rgb: [u8; 3]| overlay.pixels().any(|p| p.0 == rgb);
    assert!(has([0x9A, 0x9A, 0x9A]), "mask in gray");
    assert!(has([0xFF, 0x5A, 0x1F]), "outline in orange");

    // Without the counter, the hole shows up as a difference.
    let outer: Vec<Path> = g
        .paths
        .iter()
        .filter(|p| signed_area(p) > 0.0)
        .cloned()
        .collect();
    let c = compare(&img, &opts, 0, &outer, CAP).unwrap();
    assert!(c.iou < 0.9, "iou {}", c.iou);
    let overlay = image::load_from_memory(&c.overlay_png).unwrap().to_rgb8();
    assert!(overlay.pixels().any(|p| p.0 == [0xE5, 0x48, 0x4D]));

    // Nothing drawn: nothing matches.
    let c = compare(&img, &opts, 0, &[], CAP).unwrap();
    assert_eq!(c.iou, 0.0);

    // Contours at a wildly wrong scale are refused, not rasterized.
    let huge: Vec<Path> = g
        .paths
        .iter()
        .map(|p| {
            let mut p = p.clone();
            for n in &mut p.nodes {
                n.x *= 1000.0;
                n.y *= 1000.0;
            }
            p
        })
        .collect();
    assert!(matches!(
        compare(&img, &opts, 0, &huge, CAP),
        Err(TraceError::Render(_))
    ));
}

#[test]
fn neighbours_do_not_leak_into_a_candidate() {
    // A "Γ" whose box takes in a sliver of the bar next to it (they overlap
    // too little to merge): tracing the "Γ" must not pick the sliver up.
    let mut img = canvas(200, 200);
    rect(&mut img, 40, 40, 55, 140);
    rect(&mut img, 40, 40, 100, 50);
    rect(&mut img, 90, 60, 130, 140);
    let opts = TraceOptions::default();
    let a = analyze(&img, &opts).unwrap();
    assert_eq!(a.candidates.len(), 2, "{:?}", a.candidates);
    let traced = trace(&img, &opts, &[0, 1], CAP, UPM).unwrap();
    let counts: Vec<Vec<usize>> = traced
        .iter()
        .map(|g| g.paths.iter().map(|p| p.nodes.len()).collect())
        .collect();
    assert_eq!(counts, vec![vec![6], vec![4]]);
    for g in &traced {
        assert!(g.paths[0].nodes.iter().all(|n| n.kind == NodeKind::Line));
        assert!(g.iou >= 0.99, "iou {}", g.iou);
    }
}

#[test]
fn trace_is_deterministic() {
    let img = ring_image();
    let a = trace(&img, &TraceOptions::default(), &[0], CAP, UPM).unwrap();
    let b = trace(&img, &TraceOptions::default(), &[0], CAP, UPM).unwrap();
    assert_eq!(a, b);
}

fn skeleton(nodes: &[(f64, f64, NodeKind)], closed: bool) -> Skeleton {
    Skeleton {
        nodes: nodes
            .iter()
            .map(|&(x, y, kind)| SkeletonNode {
                x,
                y,
                kind,
                width: None,
            })
            .collect(),
        closed,
    }
}

/// Renders `paths` anti-aliased in orange on black, `cap_px` pixels per 700
/// units, with a 20-pixel margin.
fn render(paths: &[Path], cap_px: f64) -> RgbImage {
    let b = bbox(paths).unwrap();
    let s = cap_px / 700.0;
    let (w, h) = ((b.width() * s) as u32 + 40, (b.height() * s) as u32 + 40);
    let t = Transform {
        xx: s,
        xy: 0.0,
        yx: 0.0,
        yy: -s,
        dx: 20.0 - b.x0 * s,
        dy: 20.0 + b.y1 * s,
    };
    let m = forma_render::coverage_mask(paths, &t, w, h).unwrap();
    let mut img = canvas(w, h);
    for (i, v) in m.data.iter().enumerate() {
        let c = f64::from(*v) / 255.0;
        let blend = |v: u8| (f64::from(v) * c).round() as u8;
        img.put_pixel(i as u32 % w, i as u32 / w, Rgb(ORANGE.map(blend)));
    }
    img
}

#[test]
fn stroked_letters_at_110_px_trace_faithfully() {
    // Letters about as large as the demo's (110 px), drawn with strokes:
    // curves, ball terminals, corners and counters.
    let (l, o, c) = (NodeKind::Line, NodeKind::OffCurve, NodeKind::Curve);
    let s_letter = stroke(
        &[skeleton(
            &[
                (450.0, 600.0, l),
                (350.0, 720.0, o),
                (80.0, 700.0, o),
                (100.0, 520.0, c),
                (120.0, 350.0, o),
                (480.0, 380.0, o),
                (480.0, 180.0, c),
                (480.0, -20.0, o),
                (150.0, 0.0, o),
                (60.0, 120.0, c),
            ],
            false,
        )],
        &StrokeStyle {
            width: 90.0,
            cap: Cap::Bulb { ratio: 1.4 },
        },
    )
    .unwrap();
    let a_letter = stroke(
        &[
            skeleton(&[(40.0, 0.0, l), (270.0, 680.0, l), (500.0, 0.0, l)], false),
            skeleton(&[(130.0, 250.0, l), (410.0, 250.0, l)], false),
        ],
        &StrokeStyle {
            width: 90.0,
            cap: Cap::Round,
        },
    )
    .unwrap();
    let o_letter = stroke(
        &[skeleton(
            &[
                (300.0, 0.0, c),
                (500.0, 0.0, o),
                (550.0, 150.0, o),
                (550.0, 350.0, c),
                (550.0, 700.0, o),
                (50.0, 700.0, o),
                (50.0, 350.0, c),
                (50.0, 0.0, o),
                (100.0, 0.0, o),
            ],
            true,
        )],
        &StrokeStyle {
            width: 90.0,
            cap: Cap::Round,
        },
    )
    .unwrap();
    for (name, paths) in [("S", s_letter), ("A", a_letter), ("O", o_letter)] {
        let g = trace_one(&render(&paths, 110.0));
        assert!(g.iou >= 0.97, "{name}: iou {}", g.iou);
        let issues = check_paths(&g.paths, 1.0);
        assert!(issues.is_empty(), "{name}: {issues:?}");
        assert_eq!(g.paths.len(), paths.len(), "{name}");
        let traced: usize = g.paths.iter().map(on_curve).sum();
        let drawn: usize = paths.iter().map(on_curve).sum();
        assert!(
            traced <= drawn + drawn / 2,
            "{name}: {traced} nodes for {drawn}"
        );
    }
}
