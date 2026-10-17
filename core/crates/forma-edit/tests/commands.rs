//! Glyph-level, metrics and font-level commands.

mod common;

use std::sync::Arc;

use common::*;
use forma_edit::{Command, EditError, apply};
use forma_geometry::signed_area;
use forma_model::{EntityKey, MasterId, PathId};

// ------------------------------------------------------------ CreateGlyph

#[test]
fn create_glyph_adds_glyph_with_a_layer_per_master() {
    let mut font = base_font();
    let bold = add_master(&mut font);
    let paths = vec![square(7, 0.0, 0.0, 100.0, 100.0, true)];
    let (next, _) = run(&font, create("C", &[0x43], 520.0, paths.clone()));

    let glyph = glyph(&next, "C");
    assert_eq!(glyph.unicodes, vec![0x43]);
    assert!(glyph.export);
    assert_eq!(glyph.layers.len(), 2);
    for master in [next.masters[0].id, bold] {
        let layer = &glyph.layers[&master];
        assert_eq!(layer.advance_width, 520.0);
        assert_eq!(layer.paths.len(), 1);
        assert_eq!(layer.paths[0].nodes, paths[0].nodes);
    }
    assert_eq!(next.glyph_order.last(), Some(&glyph.id));
    assert_eq!(next.glyph_order.len(), 3);
}

#[test]
fn create_glyph_rejects_duplicate_name() {
    let font = base_font();
    let err = fails(&font, create("A", &[], 500.0, vec![]));
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
}

#[test]
fn create_glyph_rejects_invalid_name() {
    let font = base_font();
    for name in ["", "has space", "1abc", ".hidden", "caf\u{e9}"] {
        let err = fails(&font, create(name, &[], 500.0, vec![]));
        assert!(
            matches!(err, EditError::InvalidArgument(_)),
            "{name:?}: {err:?}"
        );
    }
    // `.notdef` is the one name allowed to start with a period.
    run(&font, create(".notdef", &[], 500.0, vec![]));
}

#[test]
fn create_glyph_rejects_degenerate_path() {
    let font = base_font();
    let one_point = closed(0, vec![line(0.0, 0.0)]);
    let err = fails(&font, create("C", &[], 500.0, vec![one_point]));
    assert!(matches!(err, EditError::InvalidGeometry(_)), "{err:?}");

    let empty = closed(0, vec![]);
    let err = fails(&font, create("C", &[], 500.0, vec![empty]));
    assert!(matches!(err, EditError::InvalidGeometry(_)), "{err:?}");
}

#[test]
fn create_glyph_rejects_malformed_path() {
    let font = base_font();
    // A cubic needs two off-curve nodes.
    let bad = closed(
        0,
        vec![
            line(0.0, 0.0),
            off(10.0, 10.0),
            curve(100.0, 0.0),
            line(50.0, 90.0),
        ],
    );
    let err = fails(&font, create("C", &[], 500.0, vec![bad]));
    assert!(matches!(err, EditError::InvalidGeometry(_)), "{err:?}");
}

#[test]
fn create_glyph_rejects_non_finite_coordinates() {
    let font = base_font();
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let path = closed(0, vec![line(0.0, 0.0), line(bad, 0.0), line(100.0, 100.0)]);
        let err = fails(&font, create("C", &[], 500.0, vec![path]));
        assert!(matches!(err, EditError::InvalidGeometry(_)), "{err:?}");
    }
}

#[test]
fn create_glyph_preserves_direction() {
    let font = base_font();
    let clockwise = square(0, 0.0, 0.0, 100.0, 100.0, false);
    assert!(signed_area(&clockwise) < 0.0);
    let (next, _) = run(&font, create("C", &[], 500.0, vec![clockwise.clone()]));

    let stored = &layer(&next, "C").paths[0];
    assert!(
        signed_area(stored) < 0.0,
        "a clockwise outer contour stays clockwise"
    );
    assert_eq!(stored.nodes, clockwise.nodes);
}

#[test]
fn create_glyph_renumbers_path_ids() {
    let font = base_font();
    let paths = vec![
        square(40, 0.0, 0.0, 100.0, 100.0, true),
        square(3, 200.0, 0.0, 300.0, 100.0, true),
        square(40, 400.0, 0.0, 500.0, 100.0, true),
    ];
    let (next, _) = run(&font, create("C", &[], 600.0, paths));
    let ids: Vec<PathId> = layer(&next, "C").paths.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![PathId(0), PathId(1), PathId(2)]);
}

#[test]
fn create_glyph_rejects_unicode_collision() {
    let font = base_font();
    let err = fails(&font, create("C", &[0x43, 0x41], 500.0, vec![]));
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
}

#[test]
fn create_glyph_rejects_repeated_or_invalid_code_points() {
    let font = base_font();
    for unicodes in [&[0x43, 0x43][..], &[0xD800], &[0x11_0000]] {
        let err = fails(&font, create("C", unicodes, 500.0, vec![]));
        assert!(
            matches!(err, EditError::InvalidArgument(_)),
            "{unicodes:?}: {err:?}"
        );
    }
}

#[test]
fn create_glyph_rejects_invalid_advance_width() {
    let font = base_font();
    for advance in [-1.0, f64::NAN, f64::INFINITY] {
        let err = fails(&font, create("C", &[], advance, vec![]));
        assert!(
            matches!(err, EditError::InvalidArgument(_)),
            "{advance}: {err:?}"
        );
    }
    run(&font, create("C", &[], 0.0, vec![]));
}

// ------------------------------------------------------------ UpdateGlyph

fn update(
    glyph: &str,
    name: Option<&str>,
    unicodes: Option<Vec<u32>>,
    export: Option<bool>,
) -> Command {
    Command::UpdateGlyph {
        glyph: by_name(glyph),
        name: name.map(str::to_owned),
        unicodes,
        export,
    }
}

#[test]
fn update_glyph_changes_name_unicodes_and_export() {
    let font = base_font();
    let a = id_of(&font, "A");
    let (next, touched) = run(
        &font,
        update("A", Some("Aacute"), Some(vec![0xC1, 0x41]), Some(false)),
    );
    let glyph = &next.glyphs[&a];
    assert_eq!(glyph.name, "Aacute");
    assert_eq!(glyph.unicodes, vec![0xC1, 0x41]);
    assert!(!glyph.export);
    assert_eq!(touched, keys([EntityKey::Glyph(a)]));
    // The outline is untouched.
    assert_eq!(glyph.layers, font.glyphs[&a].layers);
}

#[test]
fn update_glyph_may_keep_its_own_name_and_unicodes() {
    let font = base_font();
    run(&font, update("A", Some("A"), Some(vec![0x41]), None));
}

#[test]
fn update_glyph_rejects_bad_names_and_collisions() {
    let font = base_font();
    for cmd in [
        update("A", Some("B"), None, None),
        update("A", Some("not valid"), None, None),
        update("A", None, Some(vec![0x42]), None),
        update("A", None, Some(vec![0x50, 0x50]), None),
    ] {
        let err = fails(&font, cmd.clone());
        assert!(
            matches!(err, EditError::InvalidArgument(_)),
            "{cmd:?}: {err:?}"
        );
    }
}

#[test]
fn update_glyph_without_changes_is_invalid() {
    let font = base_font();
    let err = fails(&font, update("A", None, None, None));
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
}

#[test]
fn update_glyph_unknown_glyph_is_not_found() {
    let font = base_font();
    let err = fails(&font, update("Z", Some("Y"), None, None));
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
}

// ----------------------------------------------------------- DeleteGlyphs

#[test]
fn delete_glyph_updates_order() {
    let mut font = base_font();
    let c = add_glyph(&mut font, "C", &[0x43], 500.0, vec![]);
    let a = id_of(&font, "A");
    let b = id_of(&font, "B");
    let (next, touched) = run(
        &font,
        Command::DeleteGlyphs {
            glyphs: vec![by_name("A"), c.into()],
        },
    );
    assert_eq!(next.glyph_order, vec![b]);
    assert_eq!(next.glyphs.len(), 1);
    assert!(next.glyphs.contains_key(&b));
    assert_eq!(
        touched,
        keys([
            EntityKey::Glyph(a),
            EntityKey::Glyph(c),
            EntityKey::GlyphOrder
        ])
    );
}

#[test]
fn delete_glyphs_rejects_empty_list() {
    let font = base_font();
    let err = fails(&font, Command::DeleteGlyphs { glyphs: vec![] });
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
}

#[test]
fn delete_glyphs_with_an_unknown_ref_deletes_nothing() {
    let font = base_font();
    let err = fails(
        &font,
        Command::DeleteGlyphs {
            glyphs: vec![by_name("A"), by_name("Nope")],
        },
    );
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
}

#[test]
fn delete_glyphs_counts_a_glyph_once() {
    let font = base_font();
    let a = id_of(&font, "A");
    let (next, touched) = run(
        &font,
        Command::DeleteGlyphs {
            glyphs: vec![by_name("A"), a.into(), by_name("A")],
        },
    );
    assert_eq!(next.glyphs.len(), 1);
    assert_eq!(touched, keys([EntityKey::Glyph(a), EntityKey::GlyphOrder]));
}

// -------------------------------------------------------------- SetOutline

#[test]
fn set_outline_replaces_paths_in_the_chosen_master() {
    let mut font = base_font();
    let bold = add_master(&mut font);
    let a = id_of(&font, "A");
    let paths = vec![square(9, 0.0, 0.0, 50.0, 50.0, true)];
    let (next, touched) = run(
        &font,
        Command::SetOutline {
            glyph: by_name("A"),
            master: Some(bold),
            paths: paths.clone(),
        },
    );
    assert_eq!(layer_of(&next, "A", bold).paths.len(), 1);
    assert_eq!(layer_of(&next, "A", bold).paths[0].nodes, paths[0].nodes);
    assert_eq!(layer_of(&next, "A", bold).advance_width, 600.0);
    // The other master is untouched.
    assert_eq!(layer(&next, "A"), layer(&font, "A"));
    assert_eq!(touched, keys([EntityKey::Glyph(a)]));
}

#[test]
fn set_outline_defaults_to_the_first_master_and_renumbers() {
    let font = base_font();
    let paths = vec![
        square(9, 0.0, 0.0, 50.0, 50.0, false),
        square(9, 100.0, 0.0, 150.0, 50.0, true),
    ];
    let (next, _) = run(
        &font,
        Command::SetOutline {
            glyph: by_name("A"),
            master: None,
            paths: paths.clone(),
        },
    );
    let stored = &layer(&next, "A").paths;
    let ids: Vec<PathId> = stored.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![PathId(0), PathId(1)]);
    assert_eq!(
        stored[0].nodes, paths[0].nodes,
        "direction is kept as given"
    );
    assert!(signed_area(&stored[0]) < 0.0);
}

#[test]
fn set_outline_can_clear_a_layer() {
    let font = base_font();
    let (next, _) = run(
        &font,
        Command::SetOutline {
            glyph: by_name("A"),
            master: None,
            paths: vec![],
        },
    );
    assert!(layer(&next, "A").paths.is_empty());
}

#[test]
fn set_outline_rejects_invalid_geometry() {
    let font = base_font();
    let err = fails(
        &font,
        Command::SetOutline {
            glyph: by_name("A"),
            master: None,
            paths: vec![closed(
                0,
                vec![line(0.0, 0.0), line(f64::NAN, 1.0), line(5.0, 5.0)],
            )],
        },
    );
    assert!(matches!(err, EditError::InvalidGeometry(_)), "{err:?}");
}

// -------------------------------------------------------------- SetMetrics

fn metrics(advance_width: Option<f64>, lsb: Option<f64>, rsb: Option<f64>) -> Command {
    Command::SetMetrics {
        glyph: by_name("A"),
        master: None,
        advance_width,
        lsb,
        rsb,
    }
}

fn x_extent(font: &forma_model::Font, name: &str) -> (f64, f64) {
    let rect = forma_geometry::bbox(&layer(font, name).paths).expect("glyph has contours");
    (rect.min_x(), rect.max_x())
}

#[test]
fn set_metrics_lsb_moves_contours() {
    // `A` spans x = 100..500 with advance 600: lsb 100, rsb 100.
    let font = base_font();
    let a = id_of(&font, "A");
    let (next, touched) = run(&font, metrics(None, Some(50.0), None));
    assert_eq!(x_extent(&next, "A"), (50.0, 450.0));
    assert_eq!(layer(&next, "A").advance_width, 550.0, "rsb is preserved");
    assert_eq!(touched, keys([EntityKey::Glyph(a)]));

    let (next, _) = run(&font, metrics(None, Some(130.0), None));
    assert_eq!(x_extent(&next, "A"), (130.0, 530.0));
    assert_eq!(layer(&next, "A").advance_width, 630.0);
}

#[test]
fn set_metrics_rsb_sets_the_advance_from_the_right_edge() {
    let font = base_font();
    let (next, _) = run(&font, metrics(None, None, Some(30.0)));
    assert_eq!(layer(&next, "A").advance_width, 530.0);
    assert_eq!(x_extent(&next, "A"), (100.0, 500.0), "contours do not move");
}

#[test]
fn set_metrics_advance_width_sets_it_directly() {
    let font = base_font();
    let (next, _) = run(&font, metrics(Some(640.0), None, None));
    assert_eq!(layer(&next, "A").advance_width, 640.0);
    assert_eq!(x_extent(&next, "A"), (100.0, 500.0));
}

#[test]
fn set_metrics_applies_lsb_before_advance_or_rsb() {
    let font = base_font();
    let (next, _) = run(&font, metrics(Some(700.0), Some(0.0), None));
    assert_eq!(x_extent(&next, "A"), (0.0, 400.0));
    assert_eq!(layer(&next, "A").advance_width, 700.0);

    let (next, _) = run(&font, metrics(None, Some(20.0), Some(60.0)));
    assert_eq!(x_extent(&next, "A"), (20.0, 420.0));
    assert_eq!(layer(&next, "A").advance_width, 480.0);
}

#[test]
fn set_metrics_rsb_and_advance_conflict() {
    let font = base_font();
    let err = fails(&font, metrics(Some(600.0), None, Some(10.0)));
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
}

#[test]
fn set_metrics_lsb_and_rsb_need_contours() {
    let mut font = base_font();
    add_glyph(&mut font, "space", &[0x20], 250.0, vec![]);
    for (lsb, rsb) in [(Some(10.0), None), (None, Some(10.0))] {
        let err = fails(
            &font,
            Command::SetMetrics {
                glyph: by_name("space"),
                master: None,
                advance_width: None,
                lsb,
                rsb,
            },
        );
        assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
    }
    // The advance width alone is fine without contours.
    let (next, _) = run(
        &font,
        Command::SetMetrics {
            glyph: by_name("space"),
            master: None,
            advance_width: Some(300.0),
            lsb: None,
            rsb: None,
        },
    );
    assert_eq!(layer(&next, "space").advance_width, 300.0);
}

#[test]
fn set_metrics_rejects_a_negative_or_non_finite_result() {
    let font = base_font();
    for cmd in [
        metrics(Some(-1.0), None, None),
        metrics(None, None, Some(-600.0)),
        // Moving the glyph 1100 to the left takes the advance below zero.
        metrics(None, Some(-1000.0), None),
        metrics(Some(f64::NAN), None, None),
        metrics(None, Some(f64::INFINITY), None),
        metrics(None, None, Some(f64::NAN)),
    ] {
        let err = fails(&font, cmd.clone());
        assert!(
            matches!(err, EditError::InvalidArgument(_)),
            "{cmd:?}: {err:?}"
        );
    }
}

#[test]
fn set_metrics_without_fields_is_invalid() {
    let font = base_font();
    let err = fails(&font, metrics(None, None, None));
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
}

#[test]
fn set_metrics_only_touches_the_chosen_master() {
    let mut font = base_font();
    let bold = add_master(&mut font);
    let (next, _) = run(
        &font,
        Command::SetMetrics {
            glyph: by_name("A"),
            master: Some(bold),
            advance_width: None,
            lsb: Some(0.0),
            rsb: None,
        },
    );
    assert_eq!(layer_of(&next, "A", bold).advance_width, 500.0);
    assert_eq!(layer(&next, "A"), layer(&font, "A"));
}

// ------------------------------------------------------------- SetFontInfo

fn font_info() -> Command {
    Command::SetFontInfo {
        family_name: None,
        style_name: None,
        version_major: None,
        version_minor: None,
        copyright: None,
        designer: None,
        units_per_em: None,
    }
}

fn with_info(f: impl FnOnce(&mut Command)) -> Command {
    let mut cmd = font_info();
    f(&mut cmd);
    cmd
}

#[test]
fn set_font_info_updates_the_given_fields_only() {
    let font = base_font();
    let (next, touched) = run(
        &font,
        Command::SetFontInfo {
            family_name: Some("Forma Sans".into()),
            style_name: None,
            version_major: Some(2),
            version_minor: Some(5),
            copyright: Some("(c) FORMA".into()),
            designer: Some("Ada".into()),
            units_per_em: None,
        },
    );
    assert_eq!(next.info.family_name, "Forma Sans");
    assert_eq!(next.info.style_name, font.info.style_name);
    assert_eq!((next.info.version_major, next.info.version_minor), (2, 5));
    assert_eq!(next.info.copyright.as_deref(), Some("(c) FORMA"));
    assert_eq!(next.info.designer.as_deref(), Some("Ada"));
    assert_eq!(touched, keys([EntityKey::FontInfo]));

    // An empty string removes an optional field.
    let (cleared, _) = run(
        &next,
        Command::SetFontInfo {
            family_name: None,
            style_name: None,
            version_major: None,
            version_minor: None,
            copyright: Some(String::new()),
            designer: None,
            units_per_em: None,
        },
    );
    assert_eq!(cleared.info.copyright, None);
    assert_eq!(cleared.info.designer.as_deref(), Some("Ada"));
}

#[test]
fn set_font_info_units_per_em_does_not_scale_outlines() {
    let font = base_font();
    let (next, touched) = run(
        &font,
        with_info(|cmd| {
            if let Command::SetFontInfo { units_per_em, .. } = cmd {
                *units_per_em = Some(2048);
            }
        }),
    );
    assert_eq!(next.units_per_em, 2048);
    assert_eq!(
        next.glyphs, font.glyphs,
        "outlines and metrics stay as they were"
    );
    assert_eq!(touched, keys([EntityKey::FontInfo]));
}

#[test]
fn set_font_info_validates_units_per_em_and_names() {
    let font = base_font();
    for upm in [0_u16, 15, 16385, u16::MAX] {
        let cmd = with_info(|cmd| {
            if let Command::SetFontInfo { units_per_em, .. } = cmd {
                *units_per_em = Some(upm);
            }
        });
        let err = fails(&font, cmd);
        assert!(
            matches!(err, EditError::InvalidArgument(_)),
            "{upm}: {err:?}"
        );
    }
    for upm in [16_u16, 16384] {
        run(
            &font,
            with_info(|cmd| {
                if let Command::SetFontInfo { units_per_em, .. } = cmd {
                    *units_per_em = Some(upm);
                }
            }),
        );
    }
    let empty_family = with_info(|cmd| {
        if let Command::SetFontInfo { family_name, .. } = cmd {
            *family_name = Some(String::new());
        }
    });
    assert!(matches!(
        fails(&font, empty_family),
        EditError::InvalidArgument(_)
    ));
    assert!(matches!(
        fails(&font, font_info()),
        EditError::InvalidArgument(_)
    ));
}

// ------------------------------------------------------ SetVerticalMetrics

#[test]
fn set_vertical_metrics_defaults_to_the_first_master() {
    let mut font = base_font();
    let bold = add_master(&mut font);
    let regular = font.masters[0].id;
    let (next, touched) = run(
        &font,
        Command::SetVerticalMetrics {
            master: None,
            ascender: Some(900.0),
            descender: None,
            x_height: Some(510.0),
            cap_height: None,
        },
    );
    let m = &next.masters[0].metrics;
    assert_eq!((m.ascender, m.x_height), (900.0, 510.0));
    assert_eq!(m.descender, font.masters[0].metrics.descender);
    assert_eq!(m.cap_height, font.masters[0].metrics.cap_height);
    assert_eq!(
        next.masters[1], font.masters[1],
        "the other master is untouched"
    );
    assert_eq!(touched, keys([EntityKey::VerticalMetrics(regular)]));

    let (next, touched) = run(
        &font,
        Command::SetVerticalMetrics {
            master: Some(bold),
            ascender: None,
            descender: Some(-250.0),
            x_height: None,
            cap_height: Some(720.0),
        },
    );
    assert_eq!(next.masters[1].metrics.descender, -250.0);
    assert_eq!(next.masters[1].metrics.cap_height, 720.0);
    assert_eq!(touched, keys([EntityKey::VerticalMetrics(bold)]));
}

#[test]
fn set_vertical_metrics_rejects_bad_input() {
    let font = base_font();
    let make = |master: Option<MasterId>, ascender: Option<f64>| Command::SetVerticalMetrics {
        master,
        ascender,
        descender: None,
        x_height: None,
        cap_height: None,
    };
    assert!(matches!(
        fails(&font, make(Some(MasterId::new()), Some(1.0))),
        EditError::NotFound(_)
    ));
    assert!(matches!(
        fails(&font, make(None, Some(f64::NAN))),
        EditError::InvalidArgument(_)
    ));
    assert!(matches!(
        fails(&font, make(None, None)),
        EditError::InvalidArgument(_)
    ));
}

// ------------------------------------------------------------- masters

#[test]
fn an_unknown_master_is_not_found() {
    let font = base_font();
    let unknown = MasterId::new();
    for cmd in [
        Command::SetOutline {
            glyph: by_name("A"),
            master: Some(unknown),
            paths: vec![],
        },
        Command::SetMetrics {
            glyph: by_name("A"),
            master: Some(unknown),
            advance_width: Some(10.0),
            lsb: None,
            rsb: None,
        },
    ] {
        let err = fails(&font, cmd);
        assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
    }
}

// ----------------------------------------------------------- apply itself

#[test]
fn apply_reports_touched_entities() {
    let font = base_font();
    let (next, touched) = run(&font, create("C", &[0x43], 500.0, vec![]));
    let c = id_of(&next, "C");
    assert_eq!(touched, keys([EntityKey::Glyph(c), EntityKey::GlyphOrder]));

    let a = id_of(&font, "A");
    let (_, touched) = run(&font, update("A", None, None, Some(false)));
    assert_eq!(touched, keys([EntityKey::Glyph(a)]));
    let (_, touched) = run(&font, metrics(Some(1.0), None, None));
    assert_eq!(touched, keys([EntityKey::Glyph(a)]));
    let (_, touched) = run(
        &font,
        Command::SetOutline {
            glyph: by_name("A"),
            master: None,
            paths: vec![],
        },
    );
    assert_eq!(touched, keys([EntityKey::Glyph(a)]));
}

#[test]
fn apply_is_pure() {
    let font = base_font();
    let before = font.clone();
    for cmd in [
        create("C", &[0x43], 500.0, vec![]),
        update("A", Some("Q"), None, None),
        metrics(None, Some(0.0), None),
        Command::DeleteGlyphs {
            glyphs: vec![by_name("A")],
        },
        Command::SetFontInfo {
            family_name: Some("Other".into()),
            style_name: None,
            version_major: None,
            version_minor: None,
            copyright: None,
            designer: None,
            units_per_em: Some(2048),
        },
    ] {
        let _ = apply(&font, &cmd);
        assert_eq!(font, before, "{cmd:?} changed its input");
    }
}

#[test]
fn a_rejected_command_leaves_the_font_alone() {
    let font = base_font();
    let before = font.clone();
    let _ = fails(&font, create("A", &[], 500.0, vec![]));
    assert_eq!(font, before);
}

#[test]
fn apply_shares_untouched_glyphs_with_the_input() {
    let font = base_font();
    let b = id_of(&font, "B");
    let a = id_of(&font, "A");
    let (next, _) = run(&font, metrics(Some(1.0), None, None));
    assert!(Arc::ptr_eq(&next.glyphs[&b], &font.glyphs[&b]));
    assert!(!Arc::ptr_eq(&next.glyphs[&a], &font.glyphs[&a]));
}

#[test]
fn glyph_edits_leave_the_glyph_order_alone() {
    let font = base_font();
    let (next, _) = run(&font, update("A", Some("Q"), None, None));
    assert_eq!(next.glyph_order, font.glyph_order);
}

#[test]
fn a_command_for_a_missing_layer_is_not_found() {
    // Glyphs imported from a source may lack a layer for some master.
    let mut font = base_font();
    let bold = add_master(&mut font);
    let a = id_of(&font, "A");
    Arc::make_mut(
        Arc::make_mut(&mut font.glyphs)
            .get_mut(&a)
            .expect("glyph A"),
    )
    .layers
    .remove(&bold);
    let err = fails(
        &font,
        Command::SetMetrics {
            glyph: by_name("A"),
            master: Some(bold),
            advance_width: Some(1.0),
            lsb: None,
            rsb: None,
        },
    );
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
}

#[test]
fn set_metrics_advance_width_does_not_revalidate_untouched_contours() {
    // A malformed contour that is already in the font does not block a change
    // that does not move any contour.
    let mut font = base_font();
    add_glyph(
        &mut font,
        "odd",
        &[],
        500.0,
        vec![closed(0, vec![line(0.0, 0.0)])],
    );
    let (next, _) = run(
        &font,
        Command::SetMetrics {
            glyph: by_name("odd"),
            master: None,
            advance_width: Some(250.0),
            lsb: None,
            rsb: None,
        },
    );
    assert_eq!(layer(&next, "odd").advance_width, 250.0);
}
