//! UFO export: what `write_ufo` puts on disk (read back with norad itself) and
//! how it treats an existing destination.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forma_interop::{InteropError, write_ufo};
use forma_model::{
    Font, Glyph, GlyphId, Layer, Master, MasterId, Node, NodeKind, Path as Contour, PathId,
    VerticalMetrics,
};
use norad::PointType;
use tempfile::TempDir;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn gid(n: u128) -> GlyphId {
    GlyphId(Uuid::from_u128(n))
}

fn node(x: f64, y: f64, kind: NodeKind, smooth: bool) -> Node {
    Node { x, y, kind, smooth }
}

fn line(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::Line, false)
}

fn contour(id: u32, closed: bool, nodes: Vec<Node>) -> Contour {
    Contour {
        id: PathId(id),
        closed,
        nodes,
    }
}

fn square(id: u32) -> Contour {
    contour(
        id,
        true,
        vec![
            line(0.0, 0.0),
            line(500.0, 0.0),
            line(500.0, 700.0),
            line(0.0, 700.0),
        ],
    )
}

/// An exportable glyph with the given outline in the default master of `font`.
fn glyph(font: &Font, n: u128, name: &str, advance_width: f64, paths: Vec<Contour>) -> Glyph {
    Glyph {
        id: gid(n),
        name: name.to_owned(),
        unicodes: Vec::new(),
        export: true,
        layers: [(
            font.default_master().id,
            Layer {
                advance_width,
                paths,
            },
        )]
        .into(),
    }
}

/// Adds `glyph` to `font`, at the end of the glyph order.
fn push(font: &mut Font, glyph: Glyph) {
    font.glyph_order.push(glyph.id);
    Arc::make_mut(&mut font.glyphs).insert(glyph.id, Arc::new(glyph));
}

/// A font with a few glyphs: an outline-only `.notdef`, an `A` with a straight
/// and a curved contour and two code points, and an outline-less `space`.
fn sample_font() -> Font {
    let mut font = Font::new("Forma Test", 1000);
    font.info.style_name = "Book".to_owned();
    font.info.version_major = 2;
    font.info.version_minor = 7;
    font.info.copyright = Some("(c) FORMA contributors".to_owned());
    font.info.designer = Some("Ada Lovelace".to_owned());

    let notdef = glyph(&font, 1, ".notdef", 500.0, vec![square(1)]);
    push(&mut font, notdef);

    let mut a = glyph(
        &font,
        2,
        "A",
        612.5,
        vec![
            contour(
                1,
                true,
                vec![line(0.0, 0.0), line(600.0, 0.0), line(300.0, 700.0)],
            ),
            contour(
                2,
                true,
                vec![
                    node(100.0, 100.0, NodeKind::OffCurve, false),
                    node(200.0, 150.0, NodeKind::OffCurve, false),
                    node(250.0, 250.0, NodeKind::Curve, true),
                    line(100.0, 250.0),
                ],
            ),
        ],
    );
    // Deliberately not ascending, to catch any sorting.
    a.unicodes = vec![0x391, 0x41];
    push(&mut font, a);

    let mut space = glyph(&font, 3, "space", 250.0, Vec::new());
    space.unicodes = vec![0x20];
    push(&mut font, space);

    font
}

/// Where the UFO of a test goes: a not-yet-existing folder in a fresh temp dir.
fn destination() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let dir = tmp.path().join("Out.ufo");
    (tmp, dir)
}

fn export(font: &Font, dir: &Path) {
    write_ufo(font, font.default_master().id, dir).expect("export the font");
}

fn load(dir: &Path) -> norad::Font {
    norad::Font::load(dir).expect("norad loads the exported UFO")
}

/// The UFO point type that the export must give node `index` of `path`.
fn expected_type(path: &Contour, index: usize) -> PointType {
    if !path.closed && index == 0 {
        return PointType::Move;
    }
    match path.nodes[index].kind {
        NodeKind::Line => PointType::Line,
        NodeKind::Curve => PointType::Curve,
        NodeKind::QCurve => PointType::QCurve,
        NodeKind::OffCurve => PointType::OffCurve,
        _ => unreachable!("every node kind is mapped"),
    }
}

/// The names of the entries of `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read dir")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// The plist tag (`integer`, `real`, ...) that holds the value of `key`.
fn plist_value_tag(plist: &str, key: &str) -> String {
    let marker = format!("<key>{key}</key>");
    let start = plist
        .find(&marker)
        .unwrap_or_else(|| panic!("no key {key} in\n{plist}"));
    let rest = plist[start + marker.len()..].trim_start();
    let rest = rest.strip_prefix('<').expect("a tag follows the key");
    rest.chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect()
}

fn glyph_order(ufo: &norad::Font) -> Vec<String> {
    ufo.lib
        .get("public.glyphOrder")
        .and_then(|value| value.as_array())
        .expect("lib.plist has public.glyphOrder")
        .iter()
        .map(|name| {
            name.as_string()
                .expect("glyph names are strings")
                .to_owned()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Content
// ---------------------------------------------------------------------------

#[test]
fn ufo_roundtrip_via_norad() {
    let font = sample_font();
    let (_tmp, dir) = destination();
    export(&font, &dir);

    let ufo = load(&dir);
    assert_eq!(ufo.font_info.family_name.as_deref(), Some("Forma Test"));

    let layer = ufo.default_layer();
    assert_eq!(layer.len(), font.glyphs.len());
    let master = font.default_master().id;
    for id in &font.glyph_order {
        let expected = &font.glyphs[id];
        let written = layer
            .get_glyph(&expected.name)
            .unwrap_or_else(|| panic!("glyph {} was not written", expected.name));
        let expected_layer = &expected.layers[&master];

        assert_eq!(
            written.width, expected_layer.advance_width,
            "{}",
            expected.name
        );
        let codepoints: Vec<u32> = written.codepoints.iter().map(u32::from).collect();
        assert_eq!(codepoints, expected.unicodes, "{}", expected.name);

        assert_eq!(
            written.contours.len(),
            expected_layer.paths.len(),
            "{}",
            expected.name
        );
        for (written_path, path) in written.contours.iter().zip(&expected_layer.paths) {
            assert_eq!(
                written_path.points.len(),
                path.nodes.len(),
                "{}",
                expected.name
            );
            for (index, (point, node)) in written_path.points.iter().zip(&path.nodes).enumerate() {
                assert_eq!((point.x, point.y), (node.x, node.y), "{}", expected.name);
                assert_eq!(point.typ, expected_type(path, index), "{}", expected.name);
            }
        }
    }
}

#[test]
fn font_info_is_written() {
    let mut font = sample_font();
    font.masters[0].metrics = VerticalMetrics {
        ascender: 800.0,
        descender: -200.0,
        x_height: 500.5,
        cap_height: 700.0,
    };
    let (_tmp, dir) = destination();
    export(&font, &dir);

    let info = load(&dir).font_info;
    assert_eq!(info.family_name.as_deref(), Some("Forma Test"));
    assert_eq!(info.style_name.as_deref(), Some("Book"));
    assert_eq!(info.units_per_em.map(|upm| upm.as_f64()), Some(1000.0));
    assert_eq!(info.ascender, Some(800.0));
    assert_eq!(info.descender, Some(-200.0));
    assert_eq!(info.x_height, Some(500.5));
    assert_eq!(info.cap_height, Some(700.0));
    assert_eq!(info.version_major, Some(2));
    assert_eq!(info.version_minor, Some(7));
    assert_eq!(info.copyright.as_deref(), Some("(c) FORMA contributors"));
    assert_eq!(
        info.open_type_name_designer.as_deref(),
        Some("Ada Lovelace")
    );

    // Integral values are written as integers, the others as reals.
    let plist = fs::read_to_string(dir.join("fontinfo.plist")).expect("read fontinfo.plist");
    for key in [
        "unitsPerEm",
        "ascender",
        "descender",
        "capHeight",
        "versionMajor",
    ] {
        assert_eq!(plist_value_tag(&plist, key), "integer", "{key}");
    }
    assert_eq!(plist_value_tag(&plist, "xHeight"), "real");
}

#[test]
fn absent_copyright_and_designer_are_not_written() {
    let mut font = sample_font();
    font.info.copyright = None;
    font.info.designer = None;
    let (_tmp, dir) = destination();
    export(&font, &dir);

    let info = load(&dir).font_info;
    assert_eq!(info.copyright, None);
    assert_eq!(info.open_type_name_designer, None);
}

#[test]
fn glyph_order_preserved() {
    let mut font = sample_font();
    // The glyph map is ordered by id; make the glyph order differ from that.
    font.glyph_order = vec![gid(3), gid(1), gid(2)];
    let (_tmp, dir) = destination();
    export(&font, &dir);

    assert_eq!(glyph_order(&load(&dir)), ["space", ".notdef", "A"]);
}

#[test]
fn non_exported_glyphs_are_skipped() {
    let mut font = sample_font();
    let mut hidden = glyph(&font, 4, "hidden", 300.0, vec![square(1)]);
    hidden.export = false;
    hidden.unicodes = vec![0x58];
    push(&mut font, hidden);
    // The hidden glyph sits mid-order: [.notdef, hidden, space, A].
    font.glyph_order.swap(1, 3);
    let (_tmp, dir) = destination();
    export(&font, &dir);

    let ufo = load(&dir);
    assert!(ufo.default_layer().get_glyph("hidden").is_none());
    assert_eq!(ufo.default_layer().len(), 3);
    assert_eq!(glyph_order(&ufo), [".notdef", "space", "A"]);
}

#[test]
fn open_contour_starts_with_move() {
    let mut font = Font::new("Open", 1000);
    let open_path = glyph(
        &font,
        1,
        "stroke",
        400.0,
        vec![
            contour(
                1,
                false,
                vec![
                    line(0.0, 0.0),
                    node(50.0, 100.0, NodeKind::OffCurve, false),
                    node(100.0, 150.0, NodeKind::OffCurve, false),
                    node(200.0, 200.0, NodeKind::Curve, false),
                    line(300.0, 200.0),
                ],
            ),
            // Whatever the kind of its first node, an open contour starts with a move.
            contour(
                2,
                false,
                vec![node(0.0, 300.0, NodeKind::Curve, false), line(100.0, 300.0)],
            ),
            square(3),
        ],
    );
    push(&mut font, open_path);
    let (_tmp, dir) = destination();
    export(&font, &dir);

    let ufo = load(&dir);
    let written = ufo
        .default_layer()
        .get_glyph("stroke")
        .expect("glyph written");
    let types: Vec<Vec<PointType>> = written
        .contours
        .iter()
        .map(|c| c.points.iter().map(|p| p.typ).collect())
        .collect();
    assert_eq!(
        types[0],
        [
            PointType::Move,
            PointType::OffCurve,
            PointType::OffCurve,
            PointType::Curve,
            PointType::Line
        ]
    );
    assert_eq!(types[1], [PointType::Move, PointType::Line]);
    assert!(!written.contours[0].is_closed());
    assert!(!written.contours[1].is_closed());
    // A closed contour never starts with a move.
    assert_eq!(types[2], [PointType::Line; 4]);
    assert!(written.contours[2].is_closed());
}

#[test]
fn closed_contour_keeps_node_order_even_when_it_starts_off_curve() {
    let font = sample_font();
    let (_tmp, dir) = destination();
    export(&font, &dir);

    let ufo = load(&dir);
    let written = ufo.default_layer().get_glyph("A").expect("glyph written");
    let types: Vec<PointType> = written.contours[1].points.iter().map(|p| p.typ).collect();
    assert_eq!(
        types,
        [
            PointType::OffCurve,
            PointType::OffCurve,
            PointType::Curve,
            PointType::Line
        ]
    );
    assert!(written.contours[1].is_closed());
}

#[test]
fn smooth_flags_are_preserved() {
    let mut font = Font::new("Smooth", 1000);
    let round = glyph(
        &font,
        1,
        "o",
        600.0,
        vec![contour(
            1,
            true,
            vec![
                node(300.0, 0.0, NodeKind::Curve, true),
                node(500.0, 0.0, NodeKind::OffCurve, false),
                node(600.0, 200.0, NodeKind::OffCurve, false),
                node(600.0, 350.0, NodeKind::Curve, false),
                node(600.0, 500.0, NodeKind::QCurve, true),
                node(300.0, 700.0, NodeKind::Line, false),
                node(0.0, 350.0, NodeKind::Line, true),
            ],
        )],
    );
    let expected: Vec<bool> = round.layers[&font.default_master().id].paths[0]
        .nodes
        .iter()
        .map(|n| n.smooth)
        .collect();
    push(&mut font, round);
    let (_tmp, dir) = destination();
    export(&font, &dir);

    let ufo = load(&dir);
    let written = ufo.default_layer().get_glyph("o").expect("glyph written");
    let smooth: Vec<bool> = written.contours[0]
        .points
        .iter()
        .map(|p| p.smooth)
        .collect();
    assert_eq!(smooth, expected);
    assert!(smooth.iter().any(|&s| s) && smooth.iter().any(|&s| !s));
}

#[test]
fn smooth_is_never_written_on_off_curve_points() {
    let mut font = Font::new("Bad", 1000);
    let bad = glyph(
        &font,
        1,
        "bad",
        600.0,
        vec![contour(
            1,
            true,
            vec![
                node(0.0, 0.0, NodeKind::Curve, false),
                node(100.0, 0.0, NodeKind::OffCurve, true),
                node(200.0, 100.0, NodeKind::OffCurve, true),
                node(200.0, 200.0, NodeKind::Curve, true),
            ],
        )],
    );
    push(&mut font, bad);
    let (_tmp, dir) = destination();
    export(&font, &dir);

    let glifs: Vec<PathBuf> = fs::read_dir(dir.join("glyphs"))
        .expect("read glyphs dir")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "glif"))
        .collect();
    assert_eq!(glifs.len(), 1);
    let xml = fs::read_to_string(&glifs[0]).expect("read glif");
    // A glif leaves the type of an off-curve point out.
    let off_curves: Vec<&str> = xml
        .split("<point ")
        .skip(1)
        .filter(|point| !point.contains("type="))
        .collect();
    assert_eq!(off_curves.len(), 2);
    for point in off_curves {
        let element = point.split('>').next().unwrap_or_default();
        assert!(!element.contains("smooth"), "{element}");
    }
    assert!(
        xml.contains("smooth=\"yes\""),
        "the on-curve smooth flag is kept"
    );
}

#[test]
fn exports_the_requested_master() {
    let mut font = sample_font();
    let bold = Master {
        id: MasterId::new(),
        name: "Bold".to_owned(),
        metrics: VerticalMetrics {
            ascender: 900.0,
            descender: -250.0,
            x_height: 520.0,
            cap_height: 710.0,
        },
    };
    let bold_id = bold.id;
    font.masters.push(bold);
    // Give every glyph a bold layer with a distinctive advance.
    let glyphs = Arc::make_mut(&mut font.glyphs);
    for glyph in glyphs.values_mut() {
        let glyph = Arc::make_mut(glyph);
        glyph.layers.insert(
            bold_id,
            Layer {
                advance_width: 777.0,
                paths: Vec::new(),
            },
        );
    }
    let (_tmp, dir) = destination();
    write_ufo(&font, bold_id, &dir).expect("export the bold master");

    let ufo = load(&dir);
    assert_eq!(ufo.font_info.ascender, Some(900.0));
    assert_eq!(ufo.font_info.descender, Some(-250.0));
    assert_eq!(ufo.font_info.x_height, Some(520.0));
    assert_eq!(ufo.font_info.cap_height, Some(710.0));
    for glyph in ufo.default_layer().iter() {
        assert_eq!(glyph.width, 777.0, "{}", glyph.name());
        assert!(glyph.contours.is_empty(), "{}", glyph.name());
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[test]
fn missing_layer_is_an_error() {
    let mut font = sample_font();
    let mut lonely = glyph(&font, 4, "lonely", 300.0, Vec::new());
    lonely.layers.clear();
    push(&mut font, lonely);
    let (_tmp, dir) = destination();

    let error = write_ufo(&font, font.default_master().id, &dir).expect_err("must fail");
    match error {
        InteropError::MissingLayer { glyph, master } => {
            assert_eq!(glyph, "lonely");
            assert_eq!(master, font.default_master().id);
        }
        other => panic!("unexpected error {other:?}"),
    }
    assert!(!dir.exists(), "nothing is written when the export fails");
}

#[test]
fn unknown_master_is_an_error() {
    let font = sample_font();
    let (_tmp, dir) = destination();
    let stranger = MasterId::new();

    let error = write_ufo(&font, stranger, &dir).expect_err("must fail");
    assert!(
        matches!(error, InteropError::UnknownMaster(id) if id == stranger),
        "{error:?}"
    );
    assert!(!dir.exists());
}

#[test]
fn failed_export_leaves_an_existing_ufo_untouched() {
    let font = sample_font();
    let (tmp, dir) = destination();
    export(&font, &dir);
    let before = fs::read(dir.join("lib.plist")).expect("read lib.plist");

    let mut broken = sample_font();
    let mut lonely = glyph(&broken, 4, "lonely", 300.0, Vec::new());
    lonely.layers.clear();
    push(&mut broken, lonely);
    let error = write_ufo(&broken, broken.default_master().id, &dir).expect_err("must fail");
    assert!(
        matches!(error, InteropError::MissingLayer { .. }),
        "{error:?}"
    );

    assert_eq!(
        fs::read(dir.join("lib.plist")).expect("read lib.plist"),
        before
    );
    assert_eq!(entries(tmp.path()), ["Out.ufo"]);
}

// ---------------------------------------------------------------------------
// Destination handling
// ---------------------------------------------------------------------------

/// A failure while writing, after the font was converted, keeps the old UFO.
#[cfg(unix)]
#[test]
fn unwritable_parent_leaves_an_existing_ufo_untouched() {
    use std::os::unix::fs::PermissionsExt;

    /// Makes the folder writable again, even when the test panics.
    struct Unlock<'a>(&'a Path);
    impl Drop for Unlock<'_> {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o755));
        }
    }

    let font = sample_font();
    let (tmp, dir) = destination();
    export(&font, &dir);
    let before = fs::read(dir.join("lib.plist")).expect("read lib.plist");

    let _unlock = Unlock(tmp.path());
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o555)).expect("lock the folder");
    if fs::File::create(tmp.path().join("probe")).is_ok() {
        return; // Permissions do not apply (running as root), so there is nothing to test.
    }

    let error = write_ufo(&font, font.default_master().id, &dir).expect_err("must fail");
    // A file system failure inside the UFO library is an I/O error, described
    // with the library's message and the cause.
    match error {
        InteropError::Io(error) => {
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            let message = error.to_string();
            assert!(message.contains("Permission denied"), "{message}");
            assert!(message.contains("failed to"), "{message}");
        }
        other => panic!("unexpected error {other:?}"),
    }
    assert_eq!(
        fs::read(dir.join("lib.plist")).expect("read lib.plist"),
        before
    );
    assert_eq!(entries(tmp.path()), ["Out.ufo"]);
}

#[test]
fn missing_parent_folders_are_created() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let dir = tmp.path().join("a").join("b").join("Out.ufo");
    export(&sample_font(), &dir);

    assert!(dir.join("metainfo.plist").is_file());
    assert_eq!(entries(&tmp.path().join("a").join("b")), ["Out.ufo"]);
}

#[test]
fn existing_ufo_is_replaced() {
    let (tmp, dir) = destination();
    export(&sample_font(), &dir);
    // Something that only the old UFO has.
    fs::write(dir.join("stale.txt"), "old").expect("write stale file");

    let mut other = Font::new("Replacement", 1000);
    let only = glyph(&other, 9, "B", 420.0, vec![square(1)]);
    push(&mut other, only);
    export(&other, &dir);

    let ufo = load(&dir);
    assert_eq!(ufo.font_info.family_name.as_deref(), Some("Replacement"));
    assert_eq!(ufo.default_layer().len(), 1);
    assert!(ufo.default_layer().get_glyph("B").is_some());
    assert!(
        !dir.join("stale.txt").exists(),
        "the old UFO is replaced, not merged"
    );
    // No temporary or backup folder is left next to it.
    assert_eq!(entries(tmp.path()), ["Out.ufo"]);
}

#[test]
fn existing_non_ufo_dir_is_refused() {
    let (tmp, dir) = destination();
    fs::create_dir(&dir).expect("create dir");
    fs::write(dir.join("precious.txt"), "do not lose me").expect("write file");

    let font = sample_font();
    let error = write_ufo(&font, font.default_master().id, &dir).expect_err("must refuse");
    assert!(
        matches!(&error, InteropError::DestinationExists(path) if path == &dir),
        "{error:?}"
    );

    assert_eq!(entries(&dir), ["precious.txt"]);
    assert_eq!(
        fs::read_to_string(dir.join("precious.txt")).expect("read file"),
        "do not lose me"
    );
    assert_eq!(entries(tmp.path()), ["Out.ufo"]);
}

#[test]
fn existing_empty_dir_is_refused() {
    let (tmp, dir) = destination();
    fs::create_dir(&dir).expect("create dir");

    let font = sample_font();
    let error = write_ufo(&font, font.default_master().id, &dir).expect_err("must refuse");
    assert!(
        matches!(error, InteropError::DestinationExists(_)),
        "{error:?}"
    );
    assert!(entries(&dir).is_empty());
    assert_eq!(entries(tmp.path()), ["Out.ufo"]);
}

#[test]
fn existing_file_is_refused() {
    let (tmp, dir) = destination();
    fs::write(&dir, "just a file").expect("write file");

    let font = sample_font();
    let error = write_ufo(&font, font.default_master().id, &dir).expect_err("must refuse");
    assert!(
        matches!(&error, InteropError::DestinationExists(path) if path == &dir),
        "{error:?}"
    );
    assert_eq!(fs::read_to_string(&dir).expect("read file"), "just a file");
    assert_eq!(entries(tmp.path()), ["Out.ufo"]);
}
