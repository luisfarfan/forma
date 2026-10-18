//! Integration tests for the preview renderer.
//!
//! Exact PNG bytes are deliberately never asserted (SIMD paths may round
//! differently across platforms): the tests check dimensions, coverage
//! fractions and differences between renders.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use forma_geometry::path_to_svg;
use forma_model::{
    Component, Font, Glyph, GlyphId, KernRef, Layer, MasterId, Node, NodeKind, Path, PathId,
    Transform,
};
use forma_render::{
    DEFAULT_SIZE, Mask, PreviewOptions, RenderError, compare_png, coverage_mask, glyph_png,
    glyph_svg, mask_iou, text_png,
};
use vello_cpu::Pixmap;

// ------------------------------------------------------------------ helpers

fn node(x: f64, y: f64, kind: NodeKind) -> Node {
    Node {
        x,
        y,
        kind,
        smooth: false,
    }
}

/// A closed rectangle of line nodes; counter-clockwise when `ccw`.
fn rect(id: u32, x0: f64, y0: f64, x1: f64, y1: f64, ccw: bool) -> Path {
    let corners = if ccw {
        [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    } else {
        [(x0, y0), (x0, y1), (x1, y1), (x1, y0)]
    };
    Path {
        id: PathId(id),
        closed: true,
        nodes: corners
            .into_iter()
            .map(|(x, y)| node(x, y, NodeKind::Line))
            .collect(),
    }
}

/// A closed contour with two on-curve line corners, one cubic segment (two
/// off-curve nodes) and a closing line: four on-curve and two off-curve nodes.
fn curved_contour(id: u32) -> Path {
    Path {
        id: PathId(id),
        closed: true,
        nodes: vec![
            node(100.0, 0.0, NodeKind::Line),
            node(500.0, 0.0, NodeKind::Line),
            node(600.0, 0.0, NodeKind::OffCurve),
            node(600.0, 100.0, NodeKind::OffCurve),
            node(600.0, 200.0, NodeKind::Curve),
            node(100.0, 200.0, NodeKind::Line),
        ],
    }
}

fn glyph(font: &Font, name: &str, unicodes: &[u32], advance: f64, paths: Vec<Path>) -> Glyph {
    let master = font.masters[0].id;
    let mut layers = BTreeMap::new();
    layers.insert(
        master,
        Layer {
            advance_width: advance,
            paths,
            components: Vec::new(),
        },
    );
    Glyph {
        id: GlyphId::new(),
        name: name.to_owned(),
        unicodes: unicodes.to_vec(),
        export: true,
        layers,
    }
}

fn add(font: &mut Font, glyph: Glyph) -> Arc<Glyph> {
    let glyph = Arc::new(glyph);
    Arc::make_mut(&mut font.glyphs).insert(glyph.id, Arc::clone(&glyph));
    font.glyph_order.push(glyph.id);
    glyph
}

fn master(font: &Font) -> MasterId {
    font.masters[0].id
}

fn opts(size: u32) -> PreviewOptions {
    PreviewOptions {
        size,
        ..PreviewOptions::default()
    }
}

/// The dimensions stored in the PNG header (IHDR chunk).
fn png_size(bytes: &[u8]) -> (u32, u32) {
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    assert_eq!(&bytes[12..16], b"IHDR");
    let be = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    (be(16), be(20))
}

fn decode(bytes: &[u8]) -> Pixmap {
    Pixmap::from_png(Cursor::new(bytes)).expect("valid PNG")
}

/// Every pixel as `(r, g, b, a)`.
fn pixels(bytes: &[u8]) -> Vec<(u8, u8, u8, u8)> {
    decode(bytes)
        .data()
        .iter()
        .map(|p| (p.r, p.g, p.b, p.a))
        .collect()
}

fn pixel(bytes: &[u8], x: u32, y: u32) -> (u8, u8, u8, u8) {
    let pixmap = decode(bytes);
    let p = pixmap.sample(
        u16::try_from(x).expect("x fits"),
        u16::try_from(y).expect("y fits"),
    );
    (p.r, p.g, p.b, p.a)
}

/// The fraction of pixels that are mostly dark.
fn dark_fraction(bytes: &[u8]) -> f64 {
    let all = pixels(bytes);
    let dark = all.iter().filter(|p| p.0 < 128 && p.1 < 128 && p.2 < 128);
    dark.count() as f64 / all.len() as f64
}

fn count(bytes: &[u8], predicate: impl Fn((u8, u8, u8, u8)) -> bool) -> usize {
    pixels(bytes).into_iter().filter(|&p| predicate(p)).count()
}

fn is_red(p: (u8, u8, u8, u8)) -> bool {
    p.0 > 200 && p.1 < 80 && p.2 < 80
}

/// A 60% black fill over white: a mid gray.
fn is_translucent_black(p: (u8, u8, u8, u8)) -> bool {
    p.0 == p.1 && p.1 == p.2 && (90..=115).contains(&p.0)
}

/// UPM 1000 (ascender 800, descender -200, x-height 500, cap height 700) with
/// a 500 x 500 square at x 100..600, advance 700.
fn square_font() -> (Font, Arc<Glyph>) {
    let mut font = Font::new("Test", 1000);
    let g = glyph(
        &font,
        "square",
        &[0x53],
        700.0,
        vec![rect(0, 100.0, 0.0, 600.0, 500.0, true)],
    );
    let g = add(&mut font, g);
    (font, g)
}

// -------------------------------------------------------------------- tests

#[test]
fn glyph_png_has_requested_size() {
    let (font, g) = square_font();
    for size in [16, 64, 512, 1000, 1024] {
        let png = glyph_png(&font, &g, master(&font), &opts(size)).expect("renders");
        assert_eq!(png_size(&png), (size, size), "size {size}");
    }
}

#[test]
fn default_options_are_512() {
    let defaults = PreviewOptions::default();
    assert_eq!(defaults.size, 512);
    assert_eq!(defaults.size, DEFAULT_SIZE);
    assert!(!defaults.show_nodes);
    assert!(!defaults.show_metrics);

    let (font, g) = square_font();
    let png = glyph_png(&font, &g, master(&font), &defaults).expect("renders");
    assert_eq!(png_size(&png), (512, 512));
}

#[test]
fn filled_square_covers_expected_fraction() {
    let (font, g) = square_font();
    let png = glyph_png(&font, &g, master(&font), &opts(512)).expect("renders");

    // The box is 700 wide and 1000 tall (ascender 800 .. descender -200); with
    // a 10% margin the square canvas spans 1200 font units, so a 500 x 500
    // square covers (500 / 1200)^2 of it.
    let expected = (500.0_f64 / 1200.0).powi(2);
    let actual = dark_fraction(&png);
    assert!(
        (actual - expected).abs() <= expected * 0.05,
        "dark fraction {actual}, expected {expected} +- 5%"
    );

    // Opaque white background, black fill.
    assert_eq!(pixel(&png, 0, 0), (255, 255, 255, 255));
    assert_eq!(pixel(&png, 256, 256), (0, 0, 0, 255));
}

#[test]
fn size_out_of_range_is_error() {
    let (font, g) = square_font();
    for size in [0, 15, 1025, 2048] {
        let err = glyph_png(&font, &g, master(&font), &opts(size)).expect_err("rejected");
        assert_eq!(err, RenderError::InvalidSize(size));
        let err = glyph_svg(&font, &g, master(&font), &opts(size)).expect_err("rejected");
        assert_eq!(err, RenderError::InvalidSize(size));
        let err =
            compare_png(&font, &font, "square", master(&font), &opts(size)).expect_err("rejected");
        assert_eq!(err, RenderError::InvalidSize(size));
    }
    for height in [0, 15, 1025, 2048] {
        let err = text_png(&font, "S", height).expect_err("rejected");
        assert_eq!(err, RenderError::InvalidSize(height));
    }
}

#[test]
fn text_png_width_grows_with_text() {
    let mut font = Font::new("Test", 1000);
    let a = glyph(
        &font,
        "a",
        &[0x61],
        500.0,
        vec![rect(0, 50.0, 0.0, 450.0, 500.0, true)],
    );
    add(&mut font, a);

    let one = text_png(&font, "a", 100).expect("renders");
    let four = text_png(&font, "aaaa", 100).expect("renders");
    let (w1, h1) = png_size(&one);
    let (w4, h4) = png_size(&four);
    assert_eq!((h1, h4), (100, 100));
    assert!(w4 > w1, "{w4} should exceed {w1}");

    // scale = 100 / (800 + 200) = 0.1, so each advance is 50 px, plus 10%
    // of the height as margin on both sides (20 px).
    assert_eq!(w1, 70);
    assert_eq!(w4, 220);

    // Something was drawn.
    assert!(dark_fraction(&four) > 0.05);
}

#[test]
fn text_png_width_is_capped_and_clipped() {
    let mut font = Font::new("Test", 1000);
    let a = glyph(
        &font,
        "a",
        &[0x61],
        500.0,
        vec![rect(0, 50.0, 0.0, 450.0, 500.0, true)],
    );
    add(&mut font, a);

    let text = "a".repeat(100);
    let png = text_png(&font, &text, 1024).expect("clipped, not an error");
    assert_eq!(png_size(&png), (2048, 1024));
}

#[test]
fn text_png_missing_char_uses_notdef_or_box() {
    // No .notdef in the font: a hollow box of width UPM / 2 is drawn.
    let mut font = Font::new("Test", 1000);
    let a = glyph(
        &font,
        "a",
        &[0x61],
        500.0,
        vec![rect(0, 50.0, 0.0, 450.0, 500.0, true)],
    );
    add(&mut font, a);
    let boxed = text_png(&font, "?", 100).expect("renders");
    // scale 0.1: advance 500 -> 50 px, margins 2 x 10 px.
    assert_eq!(png_size(&boxed), (70, 100));
    let box_dark = dark_fraction(&boxed);
    assert!(box_dark > 0.0, "the fallback box must be visible");
    // Outlined, not solid: much less ink than a solid 400 x 700 rectangle.
    assert!(box_dark < 0.2, "box should be hollow, got {box_dark}");

    // With a .notdef glyph in the model, that glyph is used instead.
    let notdef = glyph(
        &font,
        ".notdef",
        &[],
        800.0,
        vec![rect(0, 100.0, 0.0, 700.0, 700.0, true)],
    );
    add(&mut font, notdef);
    let with_notdef = text_png(&font, "?", 100).expect("renders");
    // advance 800 -> 80 px + 20 px margins.
    assert_eq!(png_size(&with_notdef), (100, 100));
    assert!(
        dark_fraction(&with_notdef) > box_dark,
        "the solid .notdef has more ink than the hollow box"
    );
    assert_ne!(with_notdef, boxed);
}

#[test]
fn empty_text_is_error() {
    let (font, _) = square_font();
    assert_eq!(text_png(&font, "", 128), Err(RenderError::InvalidText));
}

#[test]
fn compare_png_differs_from_glyph_png() {
    let (before, _) = square_font();
    let m = master(&before);

    // `after` shares the master and has a bigger "square".
    let mut after = Font {
        masters: before.masters.clone(),
        ..Font::new("Test", 1000)
    };
    let bigger = glyph(
        &after,
        "square",
        &[0x53],
        700.0,
        vec![rect(0, 100.0, 0.0, 650.0, 600.0, true)],
    );
    let bigger = add(&mut after, bigger);

    let compared = compare_png(&before, &after, "square", m, &opts(512)).expect("renders");
    let plain = glyph_png(&after, &bigger, m, &opts(512)).expect("renders");

    assert_eq!(png_size(&compared), (512, 512));
    assert_ne!(compared, plain);
    assert!(count(&compared, is_red) > 0, "before is drawn in red");
    assert!(
        count(&compared, is_translucent_black) > 0,
        "after is drawn at 60% black"
    );
}

#[test]
fn compare_with_glyph_only_in_after() {
    let (with_glyph, _) = square_font();
    let without = Font {
        masters: with_glyph.masters.clone(),
        ..Font::new("Test", 1000)
    };
    let m = master(&with_glyph);

    // Only in `after`: only the translucent fill is drawn.
    let only_after = compare_png(&without, &with_glyph, "square", m, &opts(256)).expect("renders");
    assert_eq!(count(&only_after, is_red), 0);
    assert!(count(&only_after, is_translucent_black) > 100);

    // Only in `before`: only the red outline is drawn.
    let only_before = compare_png(&with_glyph, &without, "square", m, &opts(256)).expect("renders");
    assert!(count(&only_before, is_red) > 100);
    assert_eq!(count(&only_before, is_translucent_black), 0);

    // In neither: an error naming the glyph.
    let err = compare_png(&without, &without, "square", m, &opts(256)).expect_err("missing");
    assert_eq!(err, RenderError::GlyphNotFound("square".to_owned()));
}

#[test]
fn svg_contains_path_data() {
    let (font, g) = square_font();
    let m = master(&font);
    let svg = glyph_svg(&font, &g, m, &opts(512)).expect("renders");

    let contour = &g.layers[&m].paths[0];
    let d = path_to_svg(contour);
    assert!(!d.is_empty());
    assert!(svg.contains(&format!("d=\"{d}\"")), "{svg}");
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
    assert!(svg.contains("fill=\"black\""));
    assert!(svg.contains("transform=\"scale(1,-1)\""));
    // The frame is 1200 font units square, centred on the 700 x 1000 box
    // (x 350, y 300); y is negated by the flip.
    assert!(svg.contains("viewBox=\"-250 -900 1200 1200\""), "{svg}");
    assert!(svg.contains("width=\"512\"") && svg.contains("height=\"512\""));
    assert!(svg.trim_end().ends_with("</svg>"));
}

#[test]
fn svg_has_a_single_path_for_all_contours() {
    let mut font = Font::new("Test", 1000);
    let g = glyph(
        &font,
        "ring",
        &[],
        700.0,
        vec![
            rect(0, 0.0, 0.0, 600.0, 600.0, true),
            rect(1, 200.0, 200.0, 400.0, 400.0, false),
        ],
    );
    let g = add(&mut font, g);
    let svg = glyph_svg(&font, &g, master(&font), &opts(256)).expect("renders");
    assert_eq!(svg.matches("<path").count(), 1, "{svg}");
    assert!(svg.contains("fill-rule=\"nonzero\""));
    assert!(!svg.contains("id=\"metrics\""));
    assert!(!svg.contains("id=\"nodes\""));

    let with_overlays = glyph_svg(
        &font,
        &g,
        master(&font),
        &PreviewOptions {
            size: 256,
            show_nodes: true,
            show_metrics: true,
        },
    )
    .expect("renders");
    assert_eq!(with_overlays.matches("<path").count(), 1);
    assert!(with_overlays.contains("id=\"metrics\""));
    assert!(with_overlays.contains("id=\"nodes\""));
}

#[test]
fn nodes_overlay_changes_pixels() {
    let mut font = Font::new("Test", 1000);
    let g = glyph(&font, "curvy", &[], 700.0, vec![curved_contour(0)]);
    let g = add(&mut font, g);
    let m = master(&font);

    let plain = glyph_png(&font, &g, m, &opts(512)).expect("renders");
    let with_nodes = glyph_png(
        &font,
        &g,
        m,
        &PreviewOptions {
            size: 512,
            show_nodes: true,
            show_metrics: false,
        },
    )
    .expect("renders");
    assert_ne!(plain, with_nodes);

    // Four on-curve nodes, each a 4 x 4 pixel square of #1f4fd1.
    let on_curve = count(&with_nodes, |p| p == (0x1f, 0x4f, 0xd1, 255));
    assert_eq!(on_curve, 4 * 16);
    assert_eq!(count(&plain, |p| p == (0x1f, 0x4f, 0xd1, 255)), 0);
    // Two off-curve nodes in light gray (and gray handle lines).
    let off_curve = count(&with_nodes, |p| p == (0xa0, 0xa0, 0xa0, 255));
    assert!(off_curve >= 2 * 16, "off-curve pixels: {off_curve}");
}

#[test]
fn metrics_overlay_changes_pixels() {
    let (font, g) = square_font();
    let m = master(&font);
    let plain = glyph_png(&font, &g, m, &opts(512)).expect("renders");
    let with_metrics = glyph_png(
        &font,
        &g,
        m,
        &PreviewOptions {
            size: 512,
            show_nodes: false,
            show_metrics: true,
        },
    )
    .expect("renders");
    assert_ne!(plain, with_metrics);

    // Five horizontal and two vertical guides across a 512 px canvas.
    let gray = count(&with_metrics, |p| p == (0xb0, 0xb0, 0xb0, 255));
    assert!(gray > 2000, "guide pixels: {gray}");
    assert_eq!(count(&plain, |p| p == (0xb0, 0xb0, 0xb0, 255)), 0);
    // The glyph itself is still drawn on top.
    assert_eq!(pixel(&with_metrics, 256, 256), (0, 0, 0, 255));
}

#[test]
fn wrong_direction_contour_still_renders_filled_nonzero() {
    let mut font = Font::new("Test", 1000);
    let ccw = glyph(
        &font,
        "ccw",
        &[],
        700.0,
        vec![rect(0, 100.0, 0.0, 600.0, 500.0, true)],
    );
    let cw = glyph(
        &font,
        "cw",
        &[],
        700.0,
        vec![rect(0, 100.0, 0.0, 600.0, 500.0, false)],
    );
    // A "ring" whose inner contour runs the same way as the outer one: under
    // the non-zero rule the winding number there is 2, so it stays filled
    // (an even-odd rule would punch a hole).
    let same_direction_ring = glyph(
        &font,
        "ring",
        &[],
        700.0,
        vec![
            rect(0, 0.0, 0.0, 600.0, 600.0, true),
            rect(1, 200.0, 200.0, 400.0, 400.0, true),
        ],
    );
    let ccw = add(&mut font, ccw);
    let cw = add(&mut font, cw);
    let ring = add(&mut font, same_direction_ring);
    let m = master(&font);

    let ccw_png = glyph_png(&font, &ccw, m, &opts(512)).expect("renders");
    let cw_png = glyph_png(&font, &cw, m, &opts(512)).expect("renders");
    let expected = (500.0_f64 / 1200.0).powi(2);
    let ccw_dark = dark_fraction(&ccw_png);
    let cw_dark = dark_fraction(&cw_png);
    assert!(
        (cw_dark - expected).abs() <= expected * 0.05,
        "cw {cw_dark}"
    );
    assert!((cw_dark - ccw_dark).abs() < 1e-3, "{cw_dark} vs {ccw_dark}");

    let ring_png = glyph_png(&font, &ring, m, &opts(512)).expect("renders");
    let solid = (600.0_f64 / 1200.0).powi(2);
    assert!((dark_fraction(&ring_png) - solid).abs() <= solid * 0.02);
    // The centre of the inner contour: font (300, 300) -> pixel (235, 256).
    assert_eq!(pixel(&ring_png, 235, 256), (0, 0, 0, 255));
}

#[test]
fn render_is_deterministic_same_process() {
    let mut font = Font::new("Test", 1000);
    let g = glyph(&font, "curvy", &[0x63], 700.0, vec![curved_contour(0)]);
    let g = add(&mut font, g);
    let m = master(&font);
    let all = PreviewOptions {
        size: 300,
        show_nodes: true,
        show_metrics: true,
    };

    assert_eq!(
        glyph_png(&font, &g, m, &all).expect("renders"),
        glyph_png(&font, &g, m, &all).expect("renders")
    );
    assert_eq!(
        text_png(&font, "ccc", 90).expect("renders"),
        text_png(&font, "ccc", 90).expect("renders")
    );
    assert_eq!(
        compare_png(&font, &font, "curvy", m, &all).expect("renders"),
        compare_png(&font, &font, "curvy", m, &all).expect("renders")
    );
    assert_eq!(
        glyph_svg(&font, &g, m, &all).expect("renders"),
        glyph_svg(&font, &g, m, &all).expect("renders")
    );
}

#[test]
fn glyph_without_layer_for_master_is_error() {
    let (font, g) = square_font();
    let other = MasterId::new();
    let err = glyph_png(&font, &g, other, &opts(64)).expect_err("no such layer");
    assert_eq!(
        err,
        RenderError::MissingLayer {
            glyph: "square".to_owned()
        }
    );
    let err = glyph_svg(&font, &g, other, &opts(64)).expect_err("no such layer");
    assert!(matches!(err, RenderError::MissingLayer { .. }));
    let err = compare_png(&font, &font, "square", other, &opts(64)).expect_err("no such layer");
    assert!(matches!(err, RenderError::MissingLayer { .. }));
}

#[test]
fn invalid_contour_is_a_geometry_error() {
    let mut font = Font::new("Test", 1000);
    let broken = Path {
        id: PathId(0),
        closed: true,
        nodes: vec![node(0.0, 0.0, NodeKind::Line)],
    };
    let g = glyph(&font, "broken", &[0x62], 500.0, vec![broken]);
    let g = add(&mut font, g);
    let err = glyph_png(&font, &g, master(&font), &opts(64)).expect_err("degenerate contour");
    assert!(matches!(err, RenderError::Geometry(_)), "{err:?}");
}

#[test]
fn zero_advance_glyph_is_centered_on_the_origin() {
    // advance 0: the box is as wide as it is tall (1000) and centred on x = 0,
    // so a 200 x 200 square centred on the origin is centred in the image.
    let mut font = Font::new("Test", 1000);
    let g = glyph(
        &font,
        "mark",
        &[0x301],
        0.0,
        vec![rect(0, -100.0, 100.0, 100.0, 300.0, true)],
    );
    let g = add(&mut font, g);
    let png = glyph_png(&font, &g, master(&font), &opts(600)).expect("renders");
    // Box 1000 x 1000 + 10% margin each side = 1200 -> 0.5 px per unit, the
    // vertical centre is y = 300, so the square's centre (0, 200) lands at
    // x = 300, y = 300 + 0.5 * 100 = 350.
    assert_eq!(pixel(&png, 300, 350), (0, 0, 0, 255));
    assert_eq!(pixel(&png, 300, 200), (255, 255, 255, 255));
}

// ------------------------------------------------- degenerate framing (fix 1)

/// A font whose default master has the given ascender and descender, with a
/// 200 x 200 square glyph "a" (U+0061) of the given advance.
fn framed_font(
    upm: u16,
    ascender: f64,
    descender: f64,
    advance: f64,
) -> (Font, Arc<Glyph>, MasterId) {
    let mut font = Font::new("Test", upm);
    font.masters[0].metrics.ascender = ascender;
    font.masters[0].metrics.descender = descender;
    let g = glyph(
        &font,
        "a",
        &[0x61],
        advance,
        vec![rect(0, 0.0, 0.0, 200.0, 200.0, true)],
    );
    let g = add(&mut font, g);
    let m = master(&font);
    (font, g, m)
}

fn assert_degenerate(result: Result<impl std::fmt::Debug, RenderError>, what: &str) {
    match result {
        Err(RenderError::Geometry(message)) => assert!(
            message.starts_with("degenerate framing: "),
            "{what}: unexpected message {message:?}"
        ),
        other => panic!("{what}: expected a degenerate framing error, got {other:?}"),
    }
}

/// All four previews of the glyph "a" of `font`.
fn assert_all_degenerate(font: &Font, g: &Glyph, m: MasterId, what: &str, text: bool) {
    let o = opts(128);
    assert_degenerate(glyph_png(font, g, m, &o), &format!("{what}: glyph_png"));
    assert_degenerate(glyph_svg(font, g, m, &o), &format!("{what}: glyph_svg"));
    assert_degenerate(
        compare_png(font, font, "a", m, &o),
        &format!("{what}: compare_png"),
    );
    if text {
        assert_degenerate(text_png(font, "a", 64), &format!("{what}: text_png"));
    }
}

#[test]
fn huge_advance_is_a_degenerate_framing_error() {
    for advance in [f64::MAX, 1e300, 1e12] {
        let (font, g, m) = framed_font(1000, 800.0, -200.0, advance);
        // text_png lays out and clips by advance, so it is not framed by it.
        assert_all_degenerate(&font, &g, m, &format!("advance {advance:e}"), false);
    }
}

#[test]
fn overflowing_metrics_are_a_degenerate_framing_error() {
    // Both are finite, but the extent between them overflows to infinity.
    let (font, g, m) = framed_font(1000, 1e308, -1e308, 600.0);
    assert_all_degenerate(&font, &g, m, "metrics +-1e308", true);

    // Finite extent, far too large to show anything.
    let (font, g, m) = framed_font(1000, 1e300, -1e300, 600.0);
    assert_all_degenerate(&font, &g, m, "metrics +-1e300", true);
}

#[test]
fn near_zero_extent_is_a_degenerate_framing_error() {
    // Subnormal extent: the scale would overflow to infinity.
    let (font, g, m) = framed_font(1000, 1e-310, 0.0, 0.0);
    assert_all_degenerate(&font, &g, m, "extent 1e-310, no advance", true);

    // Tiny but normal extent: the scale would be ~1e300, which shows nothing.
    let (font, g, m) = framed_font(1000, 1e-300, 0.0, 0.0);
    assert_all_degenerate(&font, &g, m, "extent 1e-300, no advance", true);

    // The vertical extent alone is enough, even with a sane advance.
    let (font, g, m) = framed_font(1000, 1e-300, 0.0, 600.0);
    assert_all_degenerate(&font, &g, m, "extent 1e-300, advance 600", true);
}

#[test]
fn zero_units_per_em_text_is_a_degenerate_framing_error() {
    // Font::new derives zero metrics from zero units per em.
    let (font, _, _) = framed_font(0, 0.0, 0.0, 600.0);
    assert_degenerate(text_png(&font, "a", 64), "upm 0, zero metrics");

    // Even with usable metrics, a missing-glyph box could not be sized.
    let (mut font, _, _) = framed_font(0, 800.0, -200.0, 600.0);
    assert_degenerate(text_png(&font, "b", 64), "upm 0, usable metrics");

    // Valid text of a valid font is unaffected by the check.
    font.units_per_em = 1000;
    assert!(text_png(&font, "b", 64).is_ok());
}

#[test]
fn unusable_metrics_fall_back_to_proportional_ones() {
    // The documented fallback: ascender not above the descender, or not
    // finite, means 80% / -20% of the units per em, i.e. the metrics
    // Font::new gives. The preview is identical to that of a regular font.
    let (reference, ref_glyph, ref_master) = framed_font(1000, 800.0, -200.0, 600.0);
    let expected = glyph_png(&reference, &ref_glyph, ref_master, &opts(128)).expect("renders");
    let expected_text = text_png(&reference, "a", 64).expect("renders");

    for (ascender, descender) in [
        (0.0, 0.0),
        (-100.0, 100.0),
        (f64::NAN, -200.0),
        (800.0, f64::NEG_INFINITY),
    ] {
        let (font, g, m) = framed_font(1000, ascender, descender, 600.0);
        let png = glyph_png(&font, &g, m, &opts(128)).expect("falls back, not an error");
        assert_eq!(png, expected, "metrics {ascender}/{descender}");
        let text = text_png(&font, "a", 64).expect("falls back, not an error");
        assert_eq!(text, expected_text, "text, metrics {ascender}/{descender}");
        glyph_svg(&font, &g, m, &opts(128)).expect("falls back, not an error");
    }
}

#[test]
fn normal_fonts_still_render() {
    // Units per em from the OpenType minimum to its maximum, with large and
    // small advances, at the smallest and largest sizes.
    for (upm, advance) in [(16, 8.0), (1000, 600.0), (2048, 1200.0), (16384, 30000.0)] {
        let u = f64::from(upm);
        let (mut font, g, m) = framed_font(upm, u * 0.8, -u * 0.2, advance);
        // A glyph that is a fifth of the em, so it is visible at any scale.
        let big = glyph(
            &font,
            "b",
            &[0x62],
            advance,
            vec![rect(0, 0.0, 0.0, u / 5.0, u / 5.0, true)],
        );
        add(&mut font, big);
        for size in [16, 512, 1024] {
            let png = glyph_png(&font, &g, m, &opts(size)).expect("renders");
            assert_eq!(png_size(&png), (size, size), "upm {upm}, size {size}");
            let svg = glyph_svg(&font, &g, m, &opts(size)).expect("renders");
            assert!(svg.contains("<path"), "upm {upm}");
            compare_png(&font, &font, "a", m, &opts(size)).expect("renders");
        }
        let png = glyph_png(&font, &g, m, &opts(256)).expect("renders");
        assert!(dark_fraction(&png) > 0.0, "upm {upm}: something is drawn");
        let text = text_png(&font, "ab", 64).expect("renders");
        assert_eq!(png_size(&text).1, 64);
        assert!(dark_fraction(&text) > 0.0, "upm {upm}: text is drawn");
    }
}

// ------------------------------------------------- components, kerning, masks

fn with_components(font: &Font, name: &str, advance: f64, components: Vec<Component>) -> Glyph {
    let mut g = glyph(font, name, &[], advance, Vec::new());
    let m = font.masters[0].id;
    if let Some(layer) = g.layers.get_mut(&m) {
        layer.components = components;
    }
    g
}

fn shift(dx: f64, dy: f64) -> Transform {
    Transform {
        dx,
        dy,
        ..Transform::IDENTITY
    }
}

#[test]
fn composite_glyph_renders_like_decomposed() {
    let mut font = Font::new("Test", 1000);
    let a = glyph(
        &font,
        "A",
        &[0x41],
        600.0,
        vec![rect(0, 50.0, 0.0, 550.0, 600.0, true)],
    );
    let a = add(&mut font, a);
    let acute = glyph(
        &font,
        "acute",
        &[],
        300.0,
        vec![rect(0, 0.0, 0.0, 100.0, 150.0, true)],
    );
    let acute = add(&mut font, acute);
    let composite = with_components(
        &font,
        "Aacute",
        600.0,
        vec![
            Component {
                base: a.id,
                transform: Transform::IDENTITY,
            },
            Component {
                base: acute.id,
                transform: shift(250.0, 700.0),
            },
        ],
    );
    let composite = add(&mut font, composite);
    let decomposed = glyph(
        &font,
        "Aacute.flat",
        &[],
        600.0,
        vec![
            rect(0, 50.0, 0.0, 550.0, 600.0, true),
            rect(1, 250.0, 700.0, 350.0, 850.0, true),
        ],
    );
    let decomposed = add(&mut font, decomposed);

    let m = master(&font);
    // Nodes are drawn for a glyph's own contours only, so only the plain
    // and metrics views are comparable.
    let options = PreviewOptions {
        size: 256,
        show_nodes: false,
        show_metrics: true,
    };
    let png = glyph_png(&font, &composite, m, &options).expect("renders");
    let flat = glyph_png(&font, &decomposed, m, &options).expect("renders");
    assert_eq!(pixels(&png), pixels(&flat));
    let png = glyph_png(&font, &composite, m, &opts(256)).expect("renders");
    let base_only = glyph_png(&font, &a, m, &opts(256)).expect("renders");
    assert_ne!(pixels(&png), pixels(&base_only), "the accent must be drawn");
    let svg = glyph_svg(&font, &composite, m, &opts(256)).expect("svg");
    assert_eq!(
        svg.matches('M').count(),
        2,
        "both contours in the SVG: {svg}"
    );
}

#[test]
fn text_applies_kerning() {
    let mut font = Font::new("Test", 1000);
    let a = glyph(
        &font,
        "A",
        &[0x41],
        600.0,
        vec![rect(0, 0.0, 0.0, 500.0, 700.0, true)],
    );
    let a = add(&mut font, a);
    let v = glyph(
        &font,
        "V",
        &[0x56],
        650.0,
        vec![rect(0, 0.0, 0.0, 500.0, 700.0, true)],
    );
    let v = add(&mut font, v);
    let m = master(&font);
    let height = 100u32;
    let plain = png_size(&text_png(&font, "AV", height).expect("renders")).0;

    font.kerning
        .set_pair(m, KernRef::Glyph(a.id), KernRef::Glyph(v.id), Some(-100.0));
    let kerned = png_size(&text_png(&font, "AV", height).expect("renders")).0;

    // Scale: height / (ascender - descender) = 100 / 1000 px per unit.
    let scale = 0.1;
    let expected = (600.0 + 650.0 - 100.0) * scale + 2.0 * 10.0;
    assert!(
        (f64::from(kerned) - expected).abs() <= 1.0,
        "{kerned} vs {expected}"
    );
    assert!((f64::from(plain) - f64::from(kerned) - 10.0).abs() <= 1.0);
}

fn identity_scale() -> Transform {
    // 1 pixel per font unit, y down: y' = -y.
    Transform {
        yy: -1.0,
        ..Transform::IDENTITY
    }
}

fn lit(mask: &Mask) -> usize {
    mask.data.iter().filter(|&&v| v >= 128).count()
}

#[test]
fn coverage_mask_of_square() {
    // A 100 x 100 square placed at y -100..0 so that it lands in 0..100 once y is flipped.
    let square = rect(0, 0.0, -100.0, 100.0, 0.0, true);
    let mask = coverage_mask(&[square], &identity_scale(), 150, 150).expect("mask");
    assert_eq!((mask.width, mask.height), (150, 150));
    assert_eq!(mask.data.len(), 150 * 150);
    assert!(lit(&mask).abs_diff(10_000) <= 200, "{}", lit(&mask));
    // Rows run top to bottom; the square occupies the top-left corner.
    assert_eq!(mask.data[50 * 150 + 50], 255);
    assert_eq!(mask.data[120 * 150 + 120], 0);
    // Same input, same coverage values.
    let again = coverage_mask(
        &[rect(0, 0.0, -100.0, 100.0, 0.0, true)],
        &identity_scale(),
        150,
        150,
    )
    .expect("mask");
    assert_eq!(mask, again);
}

#[test]
fn coverage_of_a_curve_adds_up_to_its_area() {
    // A disc of four cubics (radius 20 px, centre off the pixel grid): its
    // coverage must add up to its area. The rasterizer flattens curves to
    // within a quarter of a pixel, which left out 14 px² (1.1 %) of this
    // disc: chords inside the arcs, up to 0.26 of a pixel's coverage along
    // them.
    let (cx, cy, r) = (25.3, -25.7, 20.0);
    let k = 0.552_284_749_830_793_4 * r;
    let corners = [(r, 0.0), (0.0, r), (-r, 0.0), (0.0, -r)];
    let mut nodes = Vec::new();
    let mut cubics = Vec::new();
    for i in 0..4 {
        let (ax, ay) = corners[i];
        let (bx, by) = corners[(i + 1) % 4];
        // Handles along the tangents: perpendicular to the radius.
        let p0 = (cx + ax, cy + ay);
        let p1 = (cx + ax - ay / r * k, cy + ay + ax / r * k);
        let p2 = (cx + bx + by / r * k, cy + by - bx / r * k);
        let p3 = (cx + bx, cy + by);
        if i == 0 {
            nodes.push(node(p0.0, p0.1, NodeKind::Curve));
        }
        nodes.push(node(p1.0, p1.1, NodeKind::OffCurve));
        nodes.push(node(p2.0, p2.1, NodeKind::OffCurve));
        if i < 3 {
            nodes.push(node(p3.0, p3.1, NodeKind::Curve));
        }
        cubics.push([p0, p1, p2, p3]);
    }
    let disc = Path {
        id: PathId(0),
        closed: true,
        nodes,
    };
    // The exact area of the four cubics (shoelace over a fine sampling).
    let mut points = Vec::new();
    for [p0, p1, p2, p3] in cubics {
        for j in 0..2000 {
            let t = f64::from(j) / 2000.0;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            points.push((
                a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
                a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
            ));
        }
    }
    let n = points.len();
    let area = (0..n)
        .map(|i| {
            let ((x0, y0), (x1, y1)) = (points[i], points[(i + 1) % n]);
            x0 * y1 - x1 * y0
        })
        .sum::<f64>()
        .abs()
        / 2.0;
    let mask = coverage_mask(&[disc], &identity_scale(), 52, 52).expect("mask");
    let covered: f64 = mask.data.iter().map(|&v| f64::from(v) / 255.0).sum();
    // Within a hundredth of a pixel along the edge (the flattening's chords).
    let perimeter = std::f64::consts::TAU * r;
    assert!(
        (covered - area).abs() <= 0.01 * perimeter,
        "coverage {covered} vs area {area}"
    );
}

#[test]
fn coverage_mask_limits() {
    let t = identity_scale();
    assert!(matches!(
        coverage_mask(&[], &t, 0, 10),
        Err(RenderError::InvalidSize(_))
    ));
    assert!(matches!(
        coverage_mask(&[], &t, 10_000, 10_000),
        Err(RenderError::InvalidSize(_))
    ));
    let empty = coverage_mask(&[], &t, 8, 8).expect("mask");
    assert!(empty.data.iter().all(|&v| v == 0));
}

#[test]
fn iou_identical_is_one_and_disjoint_is_zero() {
    let t = identity_scale();
    let a = coverage_mask(&[rect(0, 0.0, -50.0, 50.0, 0.0, true)], &t, 200, 200).expect("a");
    let b = coverage_mask(&[rect(0, 100.0, -150.0, 150.0, -100.0, true)], &t, 200, 200).expect("b");
    assert_eq!(mask_iou(&a, &a, 128).expect("iou"), 1.0);
    assert_eq!(mask_iou(&a, &b, 128).expect("iou"), 0.0);
    let empty = coverage_mask(&[], &t, 200, 200).expect("e");
    assert_eq!(mask_iou(&empty, &empty, 128).expect("iou"), 1.0);
    let half = coverage_mask(&[rect(0, 0.0, -50.0, 25.0, 0.0, true)], &t, 200, 200).expect("h");
    assert!((mask_iou(&a, &half, 128).expect("iou") - 0.5).abs() < 0.01);
    let small = coverage_mask(&[], &t, 10, 10).expect("s");
    assert!(mask_iou(&a, &small, 128).is_err());
}
