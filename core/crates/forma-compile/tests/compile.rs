//! Compilation to TTF and WOFF2: what the preflight adds, and what fontc puts
//! in the binary (read back with read-fonts).

mod common;

use common::{
    DEMO_FAMILY, DEMO_GLYPHS, DEMO_UPM, add_glyph, advance_of, circle, circle_counter, closed,
    demo_font, edit_glyph, node, rect, svg,
};
use forma_compile::{CompileError, CompileReport, OutputFormat, compile, preflight};
use forma_geometry::{bbox, signed_area, to_bezpath};
use forma_model::{Font, MasterId, NodeKind, Path, VerticalMetrics};
use read_fonts::tables::glyf::{Glyph as TtGlyph, SimpleGlyph};
use read_fonts::types::{GlyphId, GlyphId16, NameId, Tag};
use read_fonts::{FontRef, TableProvider};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compile_ttf(font: &Font) -> (Vec<u8>, CompileReport) {
    compile(font, OutputFormat::Ttf).expect("the font compiles to TTF")
}

fn parse(bytes: &[u8]) -> FontRef<'_> {
    FontRef::new(bytes).expect("read-fonts parses the compiled font")
}

fn num_glyphs(font: &FontRef) -> u16 {
    font.maxp().expect("maxp").num_glyphs()
}

/// The glyph names of the binary, by glyph id, from the `post` table.
fn glyph_names(font: &FontRef) -> Vec<String> {
    let post = font.post().expect("post");
    (0..num_glyphs(font))
        .map(|gid| {
            post.glyph_name(GlyphId16::new(gid))
                .expect("every glyph has a name in post")
                .to_owned()
        })
        .collect()
}

/// The glyph id of the glyph called `name` in the binary.
fn gid_of(font: &FontRef, name: &str) -> GlyphId {
    let names = glyph_names(font);
    let index = names
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("no glyph named {name:?} in {names:?}"));
    GlyphId::new(u32::try_from(index).expect("small glyph id"))
}

fn cmap_lookup(font: &FontRef, c: char) -> Option<GlyphId> {
    font.cmap().expect("cmap").map_codepoint(c)
}

fn advance(font: &FontRef, gid: GlyphId) -> u16 {
    font.hmtx()
        .expect("hmtx")
        .advance(gid)
        .expect("the glyph has an advance")
}

fn name_strings(font: &FontRef, id: NameId) -> Vec<String> {
    let name = font.name().expect("name");
    name.name_record()
        .iter()
        .filter(|record| record.name_id() == id)
        .map(|record| {
            record
                .string(name.string_data())
                .expect("readable name string")
                .to_string()
        })
        .collect()
}

/// The outline of glyph `gid`, or `None` for an empty glyph.
fn simple_glyph<'a>(font: &FontRef<'a>, gid: GlyphId) -> Option<SimpleGlyph<'a>> {
    let loca = font.loca(None).expect("loca");
    let glyf = font.glyf().expect("glyf");
    match loca.get_glyf(gid, &glyf).expect("readable glyph") {
        None => None,
        Some(TtGlyph::Simple(glyph)) => Some(glyph),
        Some(TtGlyph::Composite(_)) => panic!("glyph {gid} is unexpectedly a composite"),
    }
}

/// The signed area of each contour of a TrueType glyph, taking every point
/// (on- and off-curve) as a polygon vertex: positive when counter-clockwise.
fn contour_areas(glyph: &SimpleGlyph) -> Vec<f64> {
    let points: Vec<_> = glyph.points().collect();
    let mut start = 0;
    glyph
        .end_pts_of_contours()
        .iter()
        .map(|end| {
            let end = usize::from(end.get());
            let contour = &points[start..=end];
            start = end + 1;
            let twice: f64 = contour
                .iter()
                .zip(contour.iter().cycle().skip(1))
                .map(|(a, b)| f64::from(a.x) * f64::from(b.y) - f64::from(b.x) * f64::from(a.y))
                .sum();
            twice / 2.0
        })
        .collect()
}

fn default_master(font: &Font) -> MasterId {
    font.default_master().id
}

fn table<'a>(font: &FontRef<'a>, tag: &[u8; 4]) -> &'a [u8] {
    font.table_data(Tag::new(tag))
        .unwrap_or_else(|| panic!("table {tag:?} is present"))
        .as_bytes()
}

// ---------------------------------------------------------------------------
// The fixture itself
// ---------------------------------------------------------------------------

#[test]
fn demo_font_contours_follow_postscript_direction() {
    let font = demo_font();
    font.validate_invariants().expect("the demo font is valid");
    assert!(font.glyph_by_name(".notdef").is_none());
    assert!(font.glyph_by_name("space").is_none());
    for name in DEMO_GLYPHS {
        let glyph = font.glyph_by_name(name).expect("demo glyph");
        let layer = &glyph.layers[&default_master(&font)];
        let depths = forma_geometry::contour_depths(&layer.paths);
        for (path, depth) in layer.paths.iter().zip(depths) {
            let area = signed_area(path);
            if depth % 2 == 0 {
                assert!(area > 0.0, "{name}: outer contour {:?} is not CCW", path.id);
            } else {
                assert!(area < 0.0, "{name}: counter {:?} is not CW", path.id);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Compiling to TTF
// ---------------------------------------------------------------------------

#[test]
fn compiles_demo_font_to_ttf() {
    let font = demo_font();
    let (bytes, report) = compile_ttf(&font);
    let ttf = parse(&bytes);

    // `cmap` maps 'A' to the glyph called `A`, and so for every demo glyph.
    for name in DEMO_GLYPHS {
        let c = name.chars().next().expect("one-letter name");
        assert_eq!(cmap_lookup(&ttf, c), Some(gid_of(&ttf, name)), "cmap {c:?}");
    }

    // The advance of `A` is the model's, rounded (612.5 -> 613).
    let a = gid_of(&ttf, "A");
    assert_eq!(advance(&ttf, a), 613);
    for name in DEMO_GLYPHS {
        let expected = advance_of(&font, name).round();
        assert_eq!(
            f64::from(advance(&ttf, gid_of(&ttf, name))),
            expected,
            "{name}"
        );
    }

    // Name ID 1 is the family.
    let families = name_strings(&ttf, NameId::FAMILY_NAME);
    assert!(!families.is_empty(), "name ID 1 is present");
    assert!(families.iter().all(|f| f == DEMO_FAMILY), "{families:?}");

    // head.unitsPerEm is the font's.
    assert_eq!(ttf.head().expect("head").units_per_em(), DEMO_UPM);

    // The preflight added `.notdef` (glyph 0) and `space`; nothing else was
    // added or dropped.
    assert_eq!(report.added_glyphs, [".notdef", "space"]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let names = glyph_names(&ttf);
    assert_eq!(
        names,
        [".notdef", "A", "B", "H", "O", "n", "o", "space"],
        "glyph order"
    );
    assert_eq!(cmap_lookup(&ttf, ' '), Some(gid_of(&ttf, "space")));
}

#[test]
fn outer_contours_are_clockwise_in_truetype() {
    // The model uses PostScript direction (outer CCW); TrueType outlines are
    // conventionally the reverse, which is what fontc and fontmake produce.
    let (bytes, _) = compile_ttf(&demo_font());
    let ttf = parse(&bytes);

    let h = simple_glyph(&ttf, gid_of(&ttf, "H")).expect("H has an outline");
    let areas = contour_areas(&h);
    assert_eq!(areas.len(), 3);
    assert!(areas.iter().all(|&a| a < 0.0), "H stems and bar: {areas:?}");

    let o = simple_glyph(&ttf, gid_of(&ttf, "O")).expect("O has an outline");
    let areas = contour_areas(&o);
    assert_eq!(areas.len(), 2);
    let mut areas = areas;
    areas.sort_by(|a, b| b.abs().total_cmp(&a.abs()));
    let (outer, inner) = (areas[0], areas[1]);
    assert!(
        outer < 0.0 && inner > 0.0,
        "O outer CW, counter CCW: {areas:?}"
    );
}

#[test]
fn empty_font_compiles_with_notdef_and_space() {
    let font = Font::new("Empty", 1000);
    let (bytes, report) = compile_ttf(&font);
    let ttf = parse(&bytes);

    assert_eq!(num_glyphs(&ttf), 2);
    assert_eq!(glyph_names(&ttf), [".notdef", "space"]);
    assert_eq!(report.added_glyphs, [".notdef", "space"]);

    let notdef = GlyphId::new(0);
    let space = GlyphId::new(1);
    assert_eq!(advance(&ttf, notdef), 500);
    assert_eq!(advance(&ttf, space), 250);
    assert_eq!(cmap_lookup(&ttf, ' '), Some(space));
    assert!(
        simple_glyph(&ttf, notdef).is_some(),
        ".notdef has an outline"
    );
    assert!(simple_glyph(&ttf, space).is_none(), "space is empty");
}

#[test]
fn notdef_is_glyph_zero_even_if_listed_later() {
    let mut font = demo_font();
    // The font's own `.notdef`, last in the glyph order, with a distinctive
    // advance so it can be told apart from a generated one.
    add_glyph(
        &mut font,
        ".notdef",
        &[],
        777.0,
        svg(&[rect(50.0, 0.0, 727.0, 700.0)]),
    );
    assert_eq!(
        font.glyph_by_name(".notdef").map(|g| g.id),
        font.glyph_order.last().copied()
    );

    let (preflighted, report) = preflight(&font).expect("preflight");
    assert_eq!(report.added_glyphs, ["space"], "the font's .notdef is kept");
    let first = preflighted.glyph_order[0];
    assert_eq!(preflighted.glyphs[&first].name, ".notdef");
    preflighted
        .validate_invariants()
        .expect("valid preflighted font");

    let (bytes, report) = compile_ttf(&font);
    assert_eq!(report.added_glyphs, ["space"]);
    let ttf = parse(&bytes);
    assert_eq!(glyph_names(&ttf)[0], ".notdef");
    assert_eq!(advance(&ttf, GlyphId::new(0)), 777);
    assert_eq!(
        glyph_names(&ttf).iter().filter(|n| *n == ".notdef").count(),
        1
    );
}

#[test]
fn existing_space_not_duplicated() {
    // A glyph with another name already encodes U+0020.
    let mut font = demo_font();
    add_glyph(&mut font, "wordspace", &[0x20], 300.0, Vec::new());
    let (preflighted, report) = preflight(&font).expect("preflight");
    assert_eq!(report.added_glyphs, [".notdef"]);
    assert!(preflighted.glyph_by_name("space").is_none());

    let (bytes, _) = compile_ttf(&font);
    let ttf = parse(&bytes);
    let wordspace = gid_of(&ttf, "wordspace");
    assert_eq!(cmap_lookup(&ttf, ' '), Some(wordspace));
    assert_eq!(advance(&ttf, wordspace), 300);
    assert!(!glyph_names(&ttf).iter().any(|n| n == "space"));

    // A glyph called `space` exists but does not encode U+0020.
    let mut font = demo_font();
    add_glyph(&mut font, "space", &[], 280.0, Vec::new());
    let (preflighted, report) = preflight(&font).expect("preflight");
    assert_eq!(report.added_glyphs, [".notdef"]);
    let spaces = preflighted
        .glyphs
        .values()
        .filter(|g| g.name == "space")
        .count();
    assert_eq!(spaces, 1);

    let (bytes, _) = compile_ttf(&font);
    let ttf = parse(&bytes);
    let names = glyph_names(&ttf);
    assert_eq!(names.iter().filter(|n| *n == "space").count(), 1);
    assert_eq!(advance(&ttf, gid_of(&ttf, "space")), 280);
}

#[test]
fn preflight_does_not_mutate_input() {
    let font = demo_font();
    let before = font.clone();

    let (preflighted, report) = preflight(&font).expect("preflight");
    assert_eq!(font, before);
    assert_ne!(preflighted, font);
    assert_eq!(report.added_glyphs, [".notdef", "space"]);
    assert_eq!(preflighted.glyphs.len(), font.glyphs.len() + 2);
    // Every original glyph is carried over unchanged.
    for (id, glyph) in font.glyphs.iter() {
        assert_eq!(preflighted.glyphs.get(id), Some(glyph));
    }

    compile(&font, OutputFormat::Woff2).expect("compile");
    assert_eq!(font, before);
}

#[test]
fn non_exported_glyphs_absent_from_binary() {
    let mut font = demo_font();
    let z = add_glyph(
        &mut font,
        "Z",
        &['Z' as u32],
        600.0,
        svg(&[rect(50.0, 0.0, 550.0, 700.0)]),
    );
    edit_glyph(&mut font, z, |glyph| glyph.export = false);

    let (bytes, _) = compile_ttf(&font);
    let ttf = parse(&bytes);
    assert_eq!(cmap_lookup(&ttf, 'Z'), None);
    assert!(!glyph_names(&ttf).iter().any(|n| n == "Z"));
    // The demo glyphs plus `.notdef` and `space`.
    assert_eq!(usize::from(num_glyphs(&ttf)), DEMO_GLYPHS.len() + 2);
}

#[test]
fn quadratic_and_cubic_outlines_compile() {
    let mut font = Font::new("Curves", 1000);

    // Quadratic segments only: a square whose top and left sides bow inwards,
    // with every off-curve point inside the square.
    // The closing segment is the quadratic arriving at the first node, whose
    // off-curve point sits at the end of the list.
    let quad = closed(
        0,
        vec![
            node(0.0, 0.0, NodeKind::QCurve),
            node(400.0, 0.0, NodeKind::Line),
            node(400.0, 400.0, NodeKind::Line),
            node(200.0, 300.0, NodeKind::OffCurve),
            node(0.0, 400.0, NodeKind::QCurve),
            node(100.0, 200.0, NodeKind::OffCurve),
        ],
    );
    assert!(signed_area(&quad) > 0.0, "the quadratic contour is CCW");
    to_bezpath(&quad).expect("the quadratic contour is well formed");
    assert!(quad.nodes.iter().any(|n| n.kind == NodeKind::QCurve));
    add_glyph(&mut font, "quad", &['q' as u32], 500.0, vec![quad.clone()]);

    // Cubic segments only: a ring of arcs.
    let ring = svg(&[
        circle(300.0, 300.0, 250.0),
        circle_counter(300.0, 300.0, 150.0),
    ]);
    assert!(
        ring.iter()
            .flat_map(|p| &p.nodes)
            .any(|n| n.kind == NodeKind::Curve)
    );
    add_glyph(&mut font, "ring", &['c' as u32], 600.0, ring.clone());

    let (bytes, _) = compile_ttf(&font);
    let ttf = parse(&bytes);

    for (name, c, paths) in [("quad", 'q', vec![quad]), ("ring", 'c', ring)] {
        let gid = gid_of(&ttf, name);
        assert_eq!(cmap_lookup(&ttf, c), Some(gid));
        let glyph = simple_glyph(&ttf, gid).unwrap_or_else(|| panic!("{name} has an outline"));
        assert_eq!(
            glyph.end_pts_of_contours().len(),
            paths.len(),
            "{name} contours"
        );
        assert!(
            glyph.points().any(|p| !p.on_curve),
            "{name} keeps curves (off-curve points)"
        );
        let expected = bbox(&paths).expect("model bbox");
        let tolerance = 1.0;
        let close = |got: i16, want: f64| (f64::from(got) - want).abs() <= tolerance;
        assert!(
            close(glyph.x_min(), expected.x0)
                && close(glyph.y_min(), expected.y0)
                && close(glyph.x_max(), expected.x1)
                && close(glyph.y_max(), expected.y1),
            "{name}: binary bbox ({}, {}, {}, {}) vs model {expected:?}",
            glyph.x_min(),
            glyph.y_min(),
            glyph.x_max(),
            glyph.y_max()
        );
    }
}

// ---------------------------------------------------------------------------
// WOFF2
// ---------------------------------------------------------------------------

#[test]
fn woff2_decompresses_to_equivalent_ttf() {
    let font = demo_font();
    let (ttf_bytes, _) = compile_ttf(&font);
    let (woff2_bytes, report) = compile(&font, OutputFormat::Woff2).expect("compile to WOFF2");
    assert_eq!(report.added_glyphs, [".notdef", "space"]);

    assert_eq!(&woff2_bytes[..4], b"wOF2", "WOFF2 signature");
    assert!(woff2_bytes.len() < ttf_bytes.len(), "WOFF2 is smaller");

    let decompressed = woofwoof::decompress(&woff2_bytes).expect("woofwoof decompresses");
    let from_woff2 = parse(&decompressed);
    let ttf = parse(&ttf_bytes);

    assert_eq!(table(&from_woff2, b"cmap"), table(&ttf, b"cmap"));
    assert_eq!(table(&from_woff2, b"hmtx"), table(&ttf, b"hmtx"));
    assert_eq!(glyph_names(&from_woff2), glyph_names(&ttf));
    assert_eq!(
        cmap_lookup(&from_woff2, 'A'),
        Some(gid_of(&from_woff2, "A"))
    );
    assert_eq!(advance(&from_woff2, gid_of(&from_woff2, "A")), 613);
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

#[test]
fn invalid_font_fails_preflight() {
    // Two glyphs with the same name break the model invariants.
    let mut font = demo_font();
    add_glyph(&mut font, "A", &[], 500.0, Vec::new());
    assert!(font.validate_invariants().is_err());

    assert!(matches!(preflight(&font), Err(CompileError::Preflight(_))));
    assert!(matches!(
        compile(&font, OutputFormat::Ttf),
        Err(CompileError::Preflight(_))
    ));
    assert!(matches!(
        compile(&font, OutputFormat::Woff2),
        Err(CompileError::Preflight(_))
    ));
}

#[test]
fn broken_contour_fails_preflight_only_in_exported_glyphs() {
    // A cubic segment with a single off-curve point.
    let broken = closed(
        0,
        vec![
            node(0.0, 0.0, NodeKind::Line),
            node(100.0, 0.0, NodeKind::Line),
            node(100.0, 50.0, NodeKind::OffCurve),
            node(100.0, 100.0, NodeKind::Curve),
        ],
    );
    assert!(to_bezpath(&broken).is_err());

    let mut font = demo_font();
    let id = add_glyph(&mut font, "broken", &[], 500.0, vec![broken]);
    font.validate_invariants().expect("model invariants hold");
    let error = preflight(&font).expect_err("an exported broken contour is rejected");
    match error {
        CompileError::Preflight(message) => assert!(message.contains("broken"), "{message}"),
        other => panic!("expected a preflight error, got {other:?}"),
    }
    assert!(matches!(
        compile(&font, OutputFormat::Ttf),
        Err(CompileError::Preflight(_))
    ));

    // The same glyph is fine when it is not exported.
    edit_glyph(&mut font, id, |glyph| glyph.export = false);
    preflight(&font).expect("a non-exported broken glyph does not block compilation");
}

#[test]
fn glyph_data_that_cannot_be_compiled_fails_preflight() {
    type Breakage = fn(&mut forma_model::Glyph, MasterId);
    let cases: [(&str, Breakage); 6] = [
        ("missing default layer", |glyph, master| {
            glyph.layers.remove(&master);
        }),
        ("surrogate code point", |glyph, _| {
            glyph.unicodes = vec![0xD800]
        }),
        ("code point beyond Unicode", |glyph, _| {
            glyph.unicodes = vec![0x11_0000]
        }),
        ("negative advance", |glyph, master| {
            glyph.layers.get_mut(&master).expect("layer").advance_width = -1.0;
        }),
        ("non-finite advance", |glyph, master| {
            glyph.layers.get_mut(&master).expect("layer").advance_width = f64::NAN;
        }),
        ("coordinate beyond the TrueType range", |glyph, master| {
            glyph.layers.get_mut(&master).expect("layer").paths[0].nodes[1].x = 40_000.0;
        }),
    ];
    for (what, breakage) in cases {
        let mut font = demo_font();
        let master = default_master(&font);
        let id = font.glyph_by_name("B").expect("B").id;
        edit_glyph(&mut font, id, |glyph| breakage(glyph, master));
        match preflight(&font).map(|_| ()) {
            Err(CompileError::Preflight(message)) => {
                assert!(message.contains("\"B\""), "{what}: {message}");
            }
            other => panic!("{what}: expected a preflight error, got {other:?}"),
        }
    }
}

#[test]
fn vertical_metrics_that_cannot_be_stored_fail_preflight() {
    type Breakage = fn(&mut VerticalMetrics);
    let cases: [(&str, Breakage); 3] = [
        ("non-finite cap height", |m| m.cap_height = f64::NAN),
        ("infinite ascender", |m| m.ascender = f64::INFINITY),
        ("descender beyond 16 bits", |m| m.descender = -40_000.0),
    ];
    for (what, breakage) in cases {
        let mut font = demo_font();
        breakage(&mut font.masters[0].metrics);
        match compile(&font, OutputFormat::Ttf).map(|_| ()) {
            Err(CompileError::Preflight(message)) => {
                assert!(message.contains("\"Regular\""), "{what}: {message}");
            }
            other => panic!("{what}: expected a preflight error, got {other:?}"),
        }
    }
}

#[test]
fn notdef_uses_seven_tenths_of_the_em_when_the_cap_height_is_too_small() {
    // 1000 units per em: margin 50, so the frame needs a cap height above 100.
    for (cap_height, expected_height) in [
        (0.0, 700.0),
        (100.0, 700.0),
        (-300.0, 700.0),
        (101.0, 101.0),
    ] {
        let mut font = Font::new("Small caps", 1000);
        font.masters[0].metrics.cap_height = cap_height;
        let (preflighted, _) = preflight(&font).expect("preflight");
        let notdef = preflighted.glyph_by_name(".notdef").expect(".notdef");
        let layer = &notdef.layers[&default_master(&font)];
        assert_eq!(
            bounds(&layer.paths[0]),
            (50.0, 0.0, 450.0, expected_height),
            "cap height {cap_height}"
        );
        assert!(signed_area(&layer.paths[0]) > 0.0 && signed_area(&layer.paths[1]) < 0.0);
    }
}

#[test]
fn notdef_outline_follows_the_rules() {
    // 2048 units per em, with a second master: advance 1024, margin
    // round(102.4) = 102, height round(cap height 1433.6) = 1434.
    let mut font = Font::new("Scaled", 2048);
    let mut bold = font.masters[0].clone();
    bold.id = MasterId::new();
    bold.name = "Bold".to_owned();
    bold.metrics = VerticalMetrics {
        cap_height: 1500.0,
        ..bold.metrics
    };
    font.masters.push(bold);

    let (preflighted, report) = preflight(&font).expect("preflight");
    assert_eq!(report.added_glyphs, [".notdef", "space"]);
    preflighted
        .validate_invariants()
        .expect("valid preflighted font");

    let notdef = preflighted.glyph_by_name(".notdef").expect(".notdef");
    assert_eq!(preflighted.glyph_order[0], notdef.id);
    assert!(notdef.export);
    assert!(notdef.unicodes.is_empty());
    assert_eq!(notdef.layers.len(), 2, "one layer per master");

    let rect_nodes = |path: &Path| -> Vec<(f64, f64)> {
        assert!(path.closed);
        assert!(path.nodes.iter().all(|n| n.kind == NodeKind::Line));
        path.nodes.iter().map(|n| (n.x, n.y)).collect()
    };
    for master in &font.masters {
        let layer = &notdef.layers[&master.id];
        assert_eq!(layer.advance_width, 1024.0);
        assert_eq!(layer.paths.len(), 2);
        let (outer, inner) = (&layer.paths[0], &layer.paths[1]);
        assert!(signed_area(outer) > 0.0, "outer CCW");
        assert!(signed_area(inner) < 0.0, "inner CW");
        // Same outline in every master, from the default master's cap height.
        assert_eq!(bounds(outer), (102.0, 0.0, 922.0, 1434.0));
        assert_eq!(bounds(inner), (204.0, 102.0, 820.0, 1332.0));
        for (x, y) in rect_nodes(outer).into_iter().chain(rect_nodes(inner)) {
            assert_eq!((x.fract(), y.fract()), (0.0, 0.0), "integer coordinates");
        }
    }

    let space = preflighted.glyph_by_name("space").expect("space");
    assert_eq!(space.unicodes, [0x20]);
    assert!(space.export);
    assert_eq!(space.layers.len(), 2);
    for layer in space.layers.values() {
        assert_eq!(layer.advance_width, 512.0);
        assert!(layer.paths.is_empty());
    }
}

/// The bounding box of one contour as `(x0, y0, x1, y1)`.
fn bounds(path: &Path) -> (f64, f64, f64, f64) {
    let rect = bbox(std::slice::from_ref(path)).expect("bbox");
    (rect.x0, rect.y0, rect.x1, rect.y1)
}
