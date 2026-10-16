//! Glyph references, command serialization and `affected_glyph_count`.

mod common;

use common::*;
use forma_edit::{Command, ContourAction, EditError, GlyphRef, NodeAction, affected_glyph_count};
use forma_geometry::IssueKind;
use forma_model::{EntityKey, Font, GlyphId, PathId};
use serde_json::json;

// ------------------------------------------------------------- GlyphRef

#[test]
fn glyph_ref_deserializes_uuid_text_as_an_id_and_other_text_as_a_name() {
    let id = GlyphId::new();
    let from_uuid: GlyphRef = serde_json::from_value(json!(id.to_string())).expect("a string");
    assert_eq!(from_uuid, GlyphRef::Id(id));

    for name in ["A", "Aacute", "uni0041", "a.alt", "not-a-uuid-0000"] {
        let parsed: GlyphRef = serde_json::from_value(json!(name)).expect("a string");
        assert_eq!(parsed, GlyphRef::Name(name.to_owned()));
    }
    assert!(serde_json::from_value::<GlyphRef>(json!(5)).is_err());

    assert_eq!(
        serde_json::to_value(GlyphRef::Id(id)).expect("ser"),
        json!(id.to_string())
    );
    assert_eq!(serde_json::to_value(by_name("A")).expect("ser"), json!("A"));
    assert_eq!("A".parse::<GlyphRef>(), Ok(GlyphRef::Name("A".into())));
    assert_eq!(id.to_string().parse::<GlyphRef>(), Ok(GlyphRef::Id(id)));
}

fn set_advance(glyph: GlyphRef) -> Command {
    Command::SetMetrics {
        glyph,
        master: None,
        advance_width: Some(111.0),
        lsb: None,
        rsb: None,
    }
}

#[test]
fn glyph_ref_resolves_by_id_and_by_name() {
    let font = base_font();
    let b = id_of(&font, "B");
    for glyph in [GlyphRef::Id(b), by_name("B")] {
        let (next, touched) = run(&font, set_advance(glyph));
        assert_eq!(layer(&next, "B").advance_width, 111.0);
        assert_eq!(layer(&next, "A").advance_width, 600.0);
        assert_eq!(touched, keys([EntityKey::Glyph(b)]));
    }
}

#[test]
fn glyph_ref_uuid_falls_back_to_name() {
    // A glyph may be named like a UUID. A reference that parses as a UUID is
    // looked up by id first and by name when no glyph has that id.
    let mut font = base_font();
    let uuid_text = "a1b2c3d4-0000-4000-8000-000000000001";
    let odd = add_glyph(&mut font, uuid_text, &[], 300.0, vec![]);
    let reference: GlyphRef = serde_json::from_value(json!(uuid_text)).expect("a string");
    assert!(matches!(reference, GlyphRef::Id(_)));
    let (next, touched) = run(&font, set_advance(reference));
    assert_eq!(layer(&next, uuid_text).advance_width, 111.0);
    assert_eq!(touched, keys([EntityKey::Glyph(odd)]));
}

#[test]
fn glyph_ref_id_lookup_wins_over_a_name_that_looks_like_the_id() {
    // Glyph ids are random, so pick one whose text is a legal glyph name for
    // the second glyph to carry.
    let mut font = Font::new("Test", 1000);
    let a: GlyphId = "a1b2c3d4-0000-4000-8000-0000000000a1"
        .parse()
        .expect("a uuid");
    add_glyph_with_id(&mut font, a, "A", &[], 600.0, vec![]);
    add_glyph(&mut font, &a.to_string(), &[], 300.0, vec![]);
    let (next, _) = run(&font, set_advance(GlyphRef::Id(a)));
    assert_eq!(layer(&next, "A").advance_width, 111.0);
    assert_eq!(layer(&next, &a.to_string()).advance_width, 300.0);
}

#[test]
fn glyph_ref_name_never_matches_an_id() {
    let font = base_font();
    let a = id_of(&font, "A");
    let err = fails(&font, set_advance(GlyphRef::Name(a.to_string())));
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
}

#[test]
fn unknown_glyph_is_not_found_and_names_the_reference() {
    let font = base_font();
    let err = fails(&font, set_advance(by_name("Zed")));
    assert_eq!(err, EditError::NotFound("glyph 'Zed'".into()));

    let ghost = GlyphId::new();
    let err = fails(&font, set_advance(GlyphRef::Id(ghost)));
    assert_eq!(err, EditError::NotFound(format!("glyph '{ghost}'")));
}

// -------------------------------------------------------------- serde

fn all_commands() -> Vec<(&'static str, Command)> {
    let id = GlyphId::new();
    let master = forma_model::MasterId::new();
    vec![
        (
            "create_glyph",
            create(
                "A",
                &[0x41],
                600.0,
                vec![square(0, 0.0, 0.0, 10.0, 10.0, true)],
            ),
        ),
        (
            "update_glyph",
            Command::UpdateGlyph {
                glyph: by_name("A"),
                name: Some("B".into()),
                unicodes: Some(vec![0x42]),
                export: Some(false),
            },
        ),
        (
            "delete_glyphs",
            Command::DeleteGlyphs {
                glyphs: vec![by_name("A"), id.into()],
            },
        ),
        (
            "set_outline",
            Command::SetOutline {
                glyph: id.into(),
                master: Some(master),
                paths: vec![circle(0, 0.0, 0.0, 10.0)],
            },
        ),
        (
            "contour_edit",
            Command::ContourEdit {
                glyph: by_name("A"),
                master: None,
                action: ContourAction::Transform {
                    path_id: PathId(2),
                    affine: [1.0, 0.0, 0.0, 1.0, 5.0, 6.0],
                },
            },
        ),
        (
            "nodes_edit",
            Command::NodesEdit {
                glyph: by_name("A"),
                master: None,
                path_id: PathId(0),
                action: NodeAction::Insert {
                    segment_index: 1,
                    t: 0.5,
                },
            },
        ),
        (
            "set_metrics",
            Command::SetMetrics {
                glyph: by_name("A"),
                master: Some(master),
                advance_width: Some(500.0),
                lsb: None,
                rsb: Some(20.0),
            },
        ),
        (
            "set_font_info",
            Command::SetFontInfo {
                family_name: Some("Fam".into()),
                style_name: None,
                version_major: Some(1),
                version_minor: None,
                copyright: None,
                designer: None,
                units_per_em: Some(2048),
            },
        ),
        (
            "set_vertical_metrics",
            Command::SetVerticalMetrics {
                master: None,
                ascender: Some(800.0),
                descender: None,
                x_height: None,
                cap_height: Some(700.0),
            },
        ),
        (
            "apply_geometry_fixes",
            Command::ApplyGeometryFixes {
                glyphs: vec![],
                kinds: vec![IssueKind::WrongDirection, IssueKind::OffGrid],
                grid: 1.0,
            },
        ),
    ]
}

#[test]
fn commands_round_trip_through_json_with_snake_case_op_tags() {
    for (op, command) in all_commands() {
        let value = serde_json::to_value(&command).expect("serializes");
        assert_eq!(value["op"], json!(op), "{command:?}");
        let back: Command = serde_json::from_value(value).expect("deserializes");
        assert_eq!(back, command);
    }
}

#[test]
fn commands_parse_from_agent_style_json() {
    let command: Command = serde_json::from_value(json!({
        "op": "nodes_edit",
        "glyph": "A",
        "path_id": 0,
        "action": { "type": "move", "indices": [0, 2], "dx": 5, "dy": -3.5 }
    }))
    .expect("parses");
    assert_eq!(
        command,
        Command::NodesEdit {
            glyph: by_name("A"),
            master: None,
            path_id: PathId(0),
            action: NodeAction::Move {
                indices: vec![0, 2],
                dx: 5.0,
                dy: -3.5
            },
        }
    );

    let command: Command = serde_json::from_value(json!({
        "op": "set_metrics", "glyph": "A", "rsb": 40
    }))
    .expect("parses");
    assert_eq!(
        command,
        Command::SetMetrics {
            glyph: by_name("A"),
            master: None,
            advance_width: None,
            lsb: None,
            rsb: Some(40.0),
        }
    );

    // Optional lists may be left out.
    let command: Command = serde_json::from_value(json!({
        "op": "create_glyph", "name": "space", "advance_width": 250
    }))
    .expect("parses");
    assert_eq!(command, create("space", &[], 250.0, vec![]));
    let command: Command = serde_json::from_value(json!({
        "op": "apply_geometry_fixes", "kinds": ["wrong_direction"], "grid": 0
    }))
    .expect("parses");
    assert!(matches!(command, Command::ApplyGeometryFixes { glyphs, .. } if glyphs.is_empty()));
}

#[test]
fn unknown_ops_and_actions_are_rejected() {
    assert!(serde_json::from_value::<Command>(json!({ "op": "explode" })).is_err());
    assert!(serde_json::from_value::<Command>(json!({ "glyph": "A" })).is_err());
    assert!(
        serde_json::from_value::<Command>(json!({
            "op": "contour_edit", "glyph": "A", "action": { "type": "melt", "path_id": 0 }
        }))
        .is_err()
    );
}

#[test]
fn commands_have_a_json_schema_listing_every_op() {
    let schema = serde_json::to_string(&schemars::schema_for!(Command)).expect("serializes");
    for (op, _) in all_commands() {
        assert!(schema.contains(op), "the schema mentions {op}");
    }
}

// ------------------------------------------------- affected_glyph_count

fn three_glyph_font() -> Font {
    let mut font = base_font();
    add_glyph(&mut font, "C", &[0x43], 500.0, vec![]);
    font
}

#[test]
fn affected_glyph_count_counts_distinct_glyphs() {
    let font = three_glyph_font();
    let a = id_of(&font, "A");

    // Creating adds one glyph.
    assert_eq!(
        affected_glyph_count(&font, &create("D", &[], 1.0, vec![])),
        1
    );

    // Single-glyph commands touch one glyph.
    assert_eq!(affected_glyph_count(&font, &set_advance(by_name("A"))), 1);
    assert_eq!(
        affected_glyph_count(
            &font,
            &Command::ContourEdit {
                glyph: a.into(),
                master: None,
                action: ContourAction::Delete { path_id: PathId(0) },
            }
        ),
        1
    );

    // Deleting counts each glyph once, however it is referenced.
    let delete = |glyphs: Vec<GlyphRef>| Command::DeleteGlyphs { glyphs };
    assert_eq!(
        affected_glyph_count(&font, &delete(vec![by_name("A"), a.into(), by_name("B")])),
        2
    );
    assert_eq!(
        affected_glyph_count(
            &font,
            &delete(vec![by_name("A"), by_name("B"), by_name("C")])
        ),
        3
    );
}

#[test]
fn affected_glyph_count_for_geometry_fixes() {
    let font = three_glyph_font();
    let fix = |glyphs: Vec<GlyphRef>| Command::ApplyGeometryFixes {
        glyphs,
        kinds: vec![IssueKind::WrongDirection],
        grid: 0.0,
    };
    assert_eq!(
        affected_glyph_count(&font, &fix(vec![])),
        3,
        "empty means all"
    );
    assert_eq!(affected_glyph_count(&font, &fix(vec![by_name("A")])), 1);
    assert_eq!(
        affected_glyph_count(&font, &fix(vec![by_name("A"), by_name("A"), by_name("C")])),
        2
    );
}

#[test]
fn affected_glyph_count_is_zero_for_font_level_commands_and_unknown_refs() {
    let font = three_glyph_font();
    assert_eq!(
        affected_glyph_count(
            &font,
            &Command::SetFontInfo {
                family_name: Some("X".into()),
                style_name: None,
                version_major: None,
                version_minor: None,
                copyright: None,
                designer: None,
                units_per_em: None,
            }
        ),
        0
    );
    assert_eq!(
        affected_glyph_count(
            &font,
            &Command::SetVerticalMetrics {
                master: None,
                ascender: Some(1.0),
                descender: None,
                x_height: None,
                cap_height: None,
            }
        ),
        0
    );
    // Unknown refs count as 0 and never fail.
    assert_eq!(
        affected_glyph_count(&font, &set_advance(by_name("Nope"))),
        0
    );
    assert_eq!(
        affected_glyph_count(
            &font,
            &Command::DeleteGlyphs {
                glyphs: vec![by_name("Nope"), by_name("A"), GlyphId::new().into()],
            }
        ),
        1
    );
    assert_eq!(
        affected_glyph_count(&font, &Command::DeleteGlyphs { glyphs: vec![] }),
        0
    );
}
