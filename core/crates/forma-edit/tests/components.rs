//! Component commands (`set_components`, `decompose`), overlap removal and
//! how deleting, renaming and spacing glyphs treat components.

mod common;

use std::sync::Arc;

use common::*;
use forma_edit::{Command, ComponentInput, EditError, affected_glyph_count};
use forma_geometry::{bbox, flatten_layer, layer_bbox, signed_area};
use forma_model::{Component, EntityKey, Font, MasterId, Path, Transform};
use kurbo::Rect;

fn translate(dx: f64, dy: f64) -> Transform {
    Transform {
        dx,
        dy,
        ..Transform::IDENTITY
    }
}

fn input(base: &str, transform: Transform) -> ComponentInput {
    ComponentInput {
        base: by_name(base),
        transform,
    }
}

fn set_components(glyph: &str, components: Vec<ComponentInput>) -> Command {
    Command::SetComponents {
        glyph: by_name(glyph),
        master: None,
        components,
    }
}

fn decompose(glyphs: &[&str]) -> Command {
    Command::Decompose {
        glyphs: glyphs.iter().map(|name| by_name(name)).collect(),
        master: None,
    }
}

fn remove_overlap(glyphs: &[&str]) -> Command {
    Command::RemoveOverlap {
        glyphs: glyphs.iter().map(|name| by_name(name)).collect(),
        master: None,
    }
}

fn delete(names: &[&str]) -> Command {
    Command::DeleteGlyphs {
        glyphs: names.iter().map(|name| by_name(name)).collect(),
    }
}

fn metrics(glyph: &str, lsb: Option<f64>, rsb: Option<f64>) -> Command {
    Command::SetMetrics {
        glyph: by_name(glyph),
        master: None,
        advance_width: None,
        lsb,
        rsb,
    }
}

/// `base_font()` (`A` a square 100..500 × 0..700, `B` a circle around
/// (350, 350) of radius 300) plus the composites `Aacute` and `Agrave`, each
/// `A` and a small `B` above it, with no contours of their own.
fn composite_font() -> Font {
    let mut font = base_font();
    for name in ["Aacute", "Agrave"] {
        add_glyph(&mut font, name, &[], 600.0, Vec::new());
        font = run(
            &font,
            set_components(
                name,
                vec![
                    input("A", Transform::IDENTITY),
                    input(
                        "B",
                        Transform {
                            xx: 0.2,
                            yy: 0.2,
                            dx: 230.0,
                            dy: 750.0,
                            ..Transform::IDENTITY
                        },
                    ),
                ],
            ),
        )
        .0;
    }
    font
}

fn master(font: &Font) -> MasterId {
    font.default_master().id
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

fn assert_rect_eq(a: Rect, b: Rect) {
    for (x, y) in [(a.x0, b.x0), (a.y0, b.y0), (a.x1, b.x1), (a.y1, b.y1)] {
        assert!(close(x, y), "{a:?} != {b:?}");
    }
}

fn area(paths: &[Path]) -> f64 {
    paths.iter().map(signed_area).sum()
}

#[test]
fn set_components_with_the_same_list_touches_nothing() {
    let mut font = base_font();
    add_glyph(&mut font, "Adot", &[], 600.0, Vec::new());
    let list = || vec![input("A", translate(10.0, 0.0))];
    let (next, _) = run(&font, set_components("Adot", list()));
    let (again, touched) = run(&next, set_components("Adot", list()));
    assert!(touched.is_empty(), "{touched:?}");
    assert_eq!(again, next);
    // Moving the component is a change.
    let (_, touched) = run(
        &next,
        set_components("Adot", vec![input("A", translate(20.0, 0.0))]),
    );
    assert_eq!(touched.len(), 1);
}

#[test]
fn set_components_and_render_bbox() {
    let mut font = base_font();
    add_glyph(&mut font, "Adot", &[], 600.0, Vec::new());
    let adot = id_of(&font, "Adot");
    let small = Transform {
        xx: 0.2,
        yy: 0.2,
        dx: 230.0,
        dy: 750.0,
        ..Transform::IDENTITY
    };
    let (next, touched) = run(
        &font,
        set_components(
            "Adot",
            vec![input("A", Transform::IDENTITY), input("B", small)],
        ),
    );
    assert_eq!(touched, keys([EntityKey::Glyph(adot)]));
    assert_eq!(
        layer(&next, "Adot").components,
        vec![
            Component {
                base: id_of(&next, "A"),
                transform: Transform::IDENTITY,
            },
            Component {
                base: id_of(&next, "B"),
                transform: small,
            },
        ]
    );
    // `A` spans 100..500 × 0..700; the small `B`, 240..360 × 760..880.
    let rect = layer_bbox(&next, adot, master(&next))
        .expect("flattens")
        .expect("an outline");
    assert_rect_eq(rect, Rect::new(100.0, 0.0, 500.0, 880.0));
    // The base glyphs are shared, untouched.
    assert!(Arc::ptr_eq(
        &font.glyphs[&id_of(&font, "A")],
        &next.glyphs[&id_of(&next, "A")]
    ));
    assert_eq!(
        affected_glyph_count(&font, &set_components("Adot", Vec::new())),
        1
    );

    // The transform defaults to the identity on the wire.
    let cmd: Command = serde_json::from_value(serde_json::json!({
        "op": "set_components", "glyph": "Adot", "components": [{"base": "A"}]
    }))
    .expect("a command");
    assert_eq!(
        cmd,
        set_components("Adot", vec![input("A", Transform::IDENTITY)])
    );

    // An empty list removes the components.
    let (cleared, _) = run(&next, set_components("Adot", Vec::new()));
    assert!(layer(&cleared, "Adot").components.is_empty());

    // Refused: an unknown base or master, a singular or non-finite
    // transform, the glyph itself, and a cycle.
    let error = fails(
        &font,
        set_components("Adot", vec![input("Z", Transform::IDENTITY)]),
    );
    assert!(matches!(error, EditError::NotFound(_)), "{error:?}");
    let error = fails(
        &font,
        Command::SetComponents {
            glyph: by_name("Adot"),
            master: Some(MasterId::new()),
            components: Vec::new(),
        },
    );
    assert!(matches!(error, EditError::NotFound(_)), "{error:?}");
    for transform in [
        Transform {
            yy: 0.0,
            ..Transform::IDENTITY
        },
        Transform {
            dx: f64::NAN,
            ..Transform::IDENTITY
        },
    ] {
        let error = fails(&font, set_components("Adot", vec![input("A", transform)]));
        assert!(matches!(error, EditError::InvalidArgument(_)), "{error:?}");
    }
    let error = fails(
        &font,
        set_components("Adot", vec![input("Adot", Transform::IDENTITY)]),
    );
    assert!(matches!(error, EditError::InvalidArgument(_)), "{error:?}");
    let error = fails(
        &next,
        set_components("A", vec![input("Adot", translate(0.0, 0.0))]),
    );
    assert!(
        matches!(&error, EditError::InvalidArgument(message) if message.contains("cycle")),
        "{error:?}"
    );
}

#[test]
fn decompose_preserves_outline() {
    let mut font = composite_font();
    // `Ring` has a contour of its own and nests `Aacute` (itself a
    // composite), mirrored and moved.
    add_glyph(
        &mut font,
        "Ring",
        &[],
        900.0,
        vec![square(7, 600.0, 0.0, 700.0, 100.0, true)],
    );
    let mirrored = Transform {
        xx: -1.0,
        dx: 1200.0,
        dy: 20.0,
        ..Transform::IDENTITY
    };
    font = run(
        &font,
        set_components(
            "Ring",
            vec![
                input("Aacute", mirrored),
                input("B", translate(0.0, -800.0)),
            ],
        ),
    )
    .0;
    let ring = id_of(&font, "Ring");
    let flat = flatten_layer(&font, ring, master(&font)).expect("flattens");

    let (next, touched) = run(&font, decompose(&["A", "Ring"]));
    // `A` has no components: it is left alone.
    assert_eq!(touched, keys([EntityKey::Glyph(ring)]));
    assert!(Arc::ptr_eq(
        &font.glyphs[&id_of(&font, "A")],
        &next.glyphs[&id_of(&next, "A")]
    ));
    let after = layer(&next, "Ring");
    assert!(after.components.is_empty());
    assert_eq!(after.advance_width, 900.0);
    assert_eq!(after.paths.len(), flat.len());
    assert!(close(area(&after.paths), area(&flat)), "area");
    assert_rect_eq(
        bbox(&after.paths).expect("an outline"),
        bbox(&flat).expect("an outline"),
    );
    // The glyph's own contour keeps its id; the new ones follow it.
    assert_eq!(after.paths[0], layer(&font, "Ring").paths[0]);
    let ids: Vec<u32> = after.paths.iter().map(|path| path.id.0).collect();
    assert_eq!(ids, (7..7 + 4).collect::<Vec<u32>>());
    // The components used are unchanged.
    assert_eq!(layer(&next, "Aacute"), layer(&font, "Aacute"));

    assert_eq!(
        affected_glyph_count(&font, &decompose(&["A", "Ring", "A"])),
        2
    );
    let error = fails(&font, decompose(&[]));
    assert!(matches!(error, EditError::InvalidArgument(_)), "{error:?}");
    let error = fails(&font, decompose(&["Z"]));
    assert!(matches!(error, EditError::NotFound(_)), "{error:?}");
}

#[test]
fn delete_used_base_is_refused() {
    let font = composite_font();
    let error = fails(&font, delete(&["A"]));
    assert_eq!(
        error,
        EditError::InvalidArgument(
            "glyph 'A' is used as a component by: Aacute, Agrave".to_owned()
        )
    );
    // Deleting one user is not enough.
    let error = fails(&font, delete(&["Agrave", "A"]));
    assert_eq!(
        error,
        EditError::InvalidArgument("glyph 'A' is used as a component by: Aacute".to_owned())
    );
    // Decomposing the users first frees the base.
    let (decomposed, _) = run(&font, decompose(&["Aacute", "Agrave"]));
    let (next, _) = run(&decomposed, delete(&["A"]));
    assert!(next.glyph_by_name("A").is_none());
}

#[test]
fn delete_base_and_users_together_is_allowed() {
    let font = composite_font();
    let ids = [
        id_of(&font, "A"),
        id_of(&font, "Aacute"),
        id_of(&font, "Agrave"),
    ];
    let (next, touched) = run(&font, delete(&["Aacute", "A", "Agrave"]));
    for name in ["A", "Aacute", "Agrave"] {
        assert!(next.glyph_by_name(name).is_none(), "{name}");
    }
    let mut expected = keys([EntityKey::GlyphOrder]);
    expected.extend(ids.map(EntityKey::Glyph));
    assert_eq!(touched, expected);
    // `B` is still used by both, which are deleted too: fine.
    let (next, _) = run(&font, delete(&["Aacute", "Agrave", "B"]));
    assert!(next.glyph_by_name("B").is_none());
}

#[test]
fn rename_base_keeps_components() {
    let font = composite_font();
    let a = id_of(&font, "A");
    let rename = Command::UpdateGlyph {
        glyph: by_name("A"),
        name: Some("A.ss01".to_owned()),
        unicodes: None,
        export: None,
    };
    let (next, touched) = run(&font, rename);
    assert_eq!(touched, keys([EntityKey::Glyph(a)]));
    assert_eq!(layer(&next, "Aacute"), layer(&font, "Aacute"));
    assert_eq!(layer(&next, "Aacute").components[0].base, a);
    let aacute = id_of(&next, "Aacute");
    assert_eq!(
        flatten_layer(&next, aacute, master(&next)).expect("flattens"),
        flatten_layer(&font, aacute, master(&font)).expect("flattens")
    );
    // The components can name the base by its new name.
    let (next, _) = run(
        &next,
        set_components("Agrave", vec![input("A.ss01", translate(10.0, 0.0))]),
    );
    assert_eq!(layer(&next, "Agrave").components[0].base, a);
}

#[test]
fn lsb_moves_components() {
    let mut font = composite_font();
    // `Adot`: a dot contour at 520..560 and `A` moved 50 to the right
    // (150..550), so the outline spans 150..560.
    add_glyph(
        &mut font,
        "Adot",
        &[],
        700.0,
        vec![square(0, 520.0, 800.0, 560.0, 840.0, true)],
    );
    font = run(
        &font,
        set_components("Adot", vec![input("A", translate(50.0, 0.0))]),
    )
    .0;
    let adot = id_of(&font, "Adot");

    let (next, _) = run(&font, metrics("Adot", Some(20.0), None));
    let shifted = layer(&next, "Adot");
    // Everything moved by 20 - 150 = -130, and the advance with it.
    assert_eq!(shifted.components[0].transform, translate(-80.0, 0.0));
    assert_eq!(shifted.paths[0].nodes[0].x, 390.0);
    assert_eq!(shifted.advance_width, 570.0);
    let rect = layer_bbox(&next, adot, master(&next))
        .expect("flattens")
        .expect("an outline");
    assert!(close(rect.min_x(), 20.0), "{rect:?}");

    // The right side bearing is measured on the whole outline too.
    let (next, _) = run(&font, metrics("Adot", None, Some(40.0)));
    assert_eq!(layer(&next, "Adot").advance_width, 600.0);

    // A glyph made only of components has side bearings.
    let (next, _) = run(&font, metrics("Aacute", Some(0.0), Some(0.0)));
    let aacute = layer(&next, "Aacute");
    assert_eq!(aacute.components[0].transform, translate(-100.0, 0.0));
    assert_eq!(aacute.components[1].transform.dx, 130.0);
    assert!(
        close(aacute.advance_width, 400.0),
        "{}",
        aacute.advance_width
    );
}

#[test]
fn remove_overlap_command_unions_and_noop_on_clean() {
    let mut font = base_font();
    add_glyph(
        &mut font,
        "H",
        &[],
        400.0,
        vec![
            square(0, 0.0, 0.0, 200.0, 200.0, true),
            square(1, 100.0, 0.0, 300.0, 200.0, true),
        ],
    );
    let h = id_of(&font, "H");

    let (next, touched) = run(&font, remove_overlap(&["A", "H", "B"]));
    // Only `H` had overlaps.
    assert_eq!(touched, keys([EntityKey::Glyph(h)]));
    for name in ["A", "B"] {
        assert!(
            Arc::ptr_eq(
                &font.glyphs[&id_of(&font, name)],
                &next.glyphs[&id_of(&next, name)]
            ),
            "{name}"
        );
    }
    // The union is exact up to the sweep's tolerance (about 1e-6 units).
    let united = &layer(&next, "H").paths;
    assert_eq!(united.len(), 1);
    assert!((area(united) - 60_000.0).abs() < 1e-2, "{}", area(united));
    let rect = bbox(united).expect("an outline");
    let expected = Rect::new(0.0, 0.0, 300.0, 200.0);
    for (x, y) in [
        (rect.x0, expected.x0),
        (rect.y0, expected.y0),
        (rect.x1, expected.x1),
        (rect.y1, expected.y1),
    ] {
        assert!((x - y).abs() < 1e-4, "{rect:?}");
    }

    // Nothing to do: nothing touched, nothing copied.
    let (again, touched) = run(&next, remove_overlap(&["A", "H"]));
    assert!(touched.is_empty());
    assert!(Arc::ptr_eq(&again.glyphs, &next.glyphs));

    assert_eq!(
        affected_glyph_count(&font, &remove_overlap(&["H", "A", "H"])),
        2
    );
    let error = fails(&font, remove_overlap(&[]));
    assert!(matches!(error, EditError::InvalidArgument(_)), "{error:?}");

    // A contour the union cannot read is reported with its glyph.
    let mut broken = base_font();
    add_glyph(
        &mut broken,
        "dot",
        &[],
        100.0,
        vec![closed(0, vec![line(10.0, 10.0)])],
    );
    let error = fails(&broken, remove_overlap(&["A", "dot"]));
    assert!(
        matches!(&error, EditError::InvalidGeometry(message) if message.contains("'dot'")),
        "{error:?}"
    );
}
