use std::collections::BTreeMap;
use std::sync::Arc;

use forma_model::{
    Actor, EntityKey, Font, FontId, Glyph, GlyphId, Layer, MasterId, ModelError, Node, NodeKind,
    Path, PathId, Rev, is_valid_glyph_name,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn gid(n: u128) -> GlyphId {
    GlyphId(Uuid::from_u128(n))
}

fn glyph(id: GlyphId, name: &str, unicodes: &[u32]) -> Glyph {
    Glyph {
        id,
        name: name.to_owned(),
        unicodes: unicodes.to_vec(),
        export: true,
        layers: BTreeMap::new(),
    }
}

/// Inserts the glyph into both the glyph map and the glyph order.
fn add_glyph(font: &mut Font, glyph: Glyph) {
    let id = glyph.id;
    Arc::make_mut(&mut font.glyphs).insert(id, Arc::new(glyph));
    font.glyph_order.push(id);
}

fn square_layer() -> Layer {
    let corner = |x: f64, y: f64| Node {
        x,
        y,
        kind: NodeKind::Line,
        smooth: false,
    };
    Layer {
        advance_width: 600.0,
        paths: vec![Path {
            id: PathId(1),
            closed: true,
            nodes: vec![
                corner(0.0, 0.0),
                corner(500.0, 0.0),
                corner(500.0, 500.0),
                corner(0.0, 500.0),
            ],
        }],
    }
}

// ---------------------------------------------------------------------------
// Font::new
// ---------------------------------------------------------------------------

#[test]
fn new_font_has_one_master_and_no_glyphs() {
    let font = Font::new("Test Sans", 1000);

    assert_eq!(font.info.family_name, "Test Sans");
    assert_eq!(font.info.style_name, "Regular");
    assert_eq!(font.info.version_major, 1);
    assert_eq!(font.info.version_minor, 0);
    assert_eq!(font.info.copyright, None);
    assert_eq!(font.info.designer, None);
    assert_eq!(font.units_per_em, 1000);
    assert!(font.glyphs.is_empty());
    assert!(font.glyph_order.is_empty());
    assert!(font.lib.is_empty());

    assert_eq!(font.masters.len(), 1);
    let master = font.default_master();
    assert_eq!(master.name, "Regular");
    assert_eq!(master.metrics.ascender, 800.0);
    assert_eq!(master.metrics.descender, -200.0);
    assert_eq!(master.metrics.x_height, 500.0);
    assert_eq!(master.metrics.cap_height, 700.0);
    assert_eq!(master, &font.masters[0]);

    assert_eq!(font.validate_invariants(), Ok(()));
}

#[test]
fn metrics_scale_with_upm() {
    let font = Font::new("Test Sans", 2048);
    let metrics = &font.default_master().metrics;

    assert_eq!(metrics.ascender, 1638.4);
    assert_eq!(metrics.descender, -409.6);
    assert_eq!(metrics.x_height, 1024.0);
    assert_eq!(metrics.cap_height, 1433.6);
}

#[test]
fn new_fonts_and_masters_get_distinct_ids() {
    let a = Font::new("A", 1000);
    let b = Font::new("B", 1000);

    assert_ne!(a.id, b.id);
    assert_ne!(a.masters[0].id, b.masters[0].id);
}

// ---------------------------------------------------------------------------
// Font lookups
// ---------------------------------------------------------------------------

#[test]
fn glyph_by_name_finds_glyphs() {
    let mut font = Font::new("Test Sans", 1000);
    add_glyph(&mut font, glyph(gid(1), "A", &[0x41]));
    add_glyph(&mut font, glyph(gid(2), "B", &[0x42]));

    assert_eq!(font.glyph_by_name("B").map(|g| g.id), Some(gid(2)));
    assert!(font.glyph_by_name("C").is_none());
}

#[test]
fn cloning_a_font_shares_glyph_storage() {
    let mut font = Font::new("Test Sans", 1000);
    add_glyph(&mut font, glyph(gid(1), "A", &[0x41]));

    let snapshot = font.clone();
    assert!(Arc::ptr_eq(&font.glyphs, &snapshot.glyphs));

    // Copy-on-write: mutating the original leaves the snapshot untouched.
    add_glyph(&mut font, glyph(gid(2), "B", &[0x42]));
    assert_eq!(font.glyphs.len(), 2);
    assert_eq!(snapshot.glyphs.len(), 1);
    assert_ne!(font, snapshot);
}

// ---------------------------------------------------------------------------
// Invariants
// ---------------------------------------------------------------------------

#[test]
fn invariants_accept_valid_font() {
    let mut font = Font::new("Test Sans", 1000);
    let master = font.masters[0].id;
    let mut a = glyph(gid(1), "A", &[0x41]);
    a.layers.insert(master, square_layer());
    add_glyph(&mut font, a);
    add_glyph(&mut font, glyph(gid(2), ".notdef", &[]));

    assert_eq!(font.validate_invariants(), Ok(()));
}

#[test]
fn invariants_reject_duplicate_glyph_names() {
    let mut font = Font::new("Test Sans", 1000);
    add_glyph(&mut font, glyph(gid(1), "A", &[0x41]));
    add_glyph(&mut font, glyph(gid(2), "A", &[0x42]));

    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::DuplicateGlyphName {
            name: "A".to_owned(),
            first: gid(1),
            second: gid(2),
        })
    );
}

#[test]
fn invariants_reject_invalid_glyph_names() {
    let mut font = Font::new("Test Sans", 1000);
    add_glyph(&mut font, glyph(gid(1), "1a", &[]));

    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::InvalidGlyphName {
            glyph: gid(1),
            name: "1a".to_owned(),
        })
    );
}

#[test]
fn invariants_reject_glyph_order_mismatch() {
    // A glyph that is not listed in the glyph order.
    let mut font = Font::new("Test Sans", 1000);
    add_glyph(&mut font, glyph(gid(1), "A", &[0x41]));
    font.glyph_order.clear();
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::GlyphMissingFromOrder(gid(1)))
    );

    // A glyph order entry that does not exist.
    let mut font = Font::new("Test Sans", 1000);
    add_glyph(&mut font, glyph(gid(1), "A", &[0x41]));
    font.glyph_order.push(gid(99));
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::GlyphOrderUnknownGlyph(gid(99)))
    );

    // A glyph listed twice.
    let mut font = Font::new("Test Sans", 1000);
    add_glyph(&mut font, glyph(gid(1), "A", &[0x41]));
    font.glyph_order.push(gid(1));
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::DuplicateInGlyphOrder(gid(1)))
    );
}

#[test]
fn invariants_reject_duplicate_unicodes_across_glyphs() {
    let mut font = Font::new("Test Sans", 1000);
    add_glyph(&mut font, glyph(gid(1), "A", &[0x41]));
    add_glyph(&mut font, glyph(gid(2), "A.alt", &[0x61, 0x41]));

    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::DuplicateUnicode {
            codepoint: 0x41,
            first: gid(1),
            second: gid(2),
        })
    );
}

#[test]
fn invariants_reject_layers_for_unknown_masters() {
    let mut font = Font::new("Test Sans", 1000);
    let stranger = MasterId(Uuid::from_u128(0xdead));
    let mut a = glyph(gid(1), "A", &[0x41]);
    a.layers.insert(stranger, square_layer());
    add_glyph(&mut font, a);

    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::UnknownMaster {
            glyph: gid(1),
            master: stranger,
        })
    );
}

#[test]
fn invariants_reject_glyph_key_that_differs_from_glyph_id() {
    let mut font = Font::new("Test Sans", 1000);
    Arc::make_mut(&mut font.glyphs).insert(gid(1), Arc::new(glyph(gid(2), "A", &[])));
    font.glyph_order.push(gid(1));

    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::GlyphKeyMismatch {
            key: gid(1),
            id: gid(2),
        })
    );
}

#[test]
fn invariants_require_units_per_em_in_opentype_range() {
    for upm in [16, 1000, 2048, 16384] {
        assert_eq!(Font::new("T", upm).validate_invariants(), Ok(()), "{upm}");
    }
    for upm in [0, 15, 16385, u16::MAX] {
        assert_eq!(
            Font::new("T", upm).validate_invariants(),
            Err(ModelError::UnitsPerEmOutOfRange(upm)),
            "{upm}"
        );
    }
}

#[test]
fn invariants_require_at_least_one_master() {
    let mut font = Font::new("Test Sans", 1000);
    font.masters.clear();

    assert_eq!(font.validate_invariants(), Err(ModelError::NoMasters));
}

#[test]
fn invariants_reject_duplicate_master_ids() {
    let mut font = Font::new("Test Sans", 1000);
    let copy = font.masters[0].clone();
    font.masters.push(copy);

    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::DuplicateMasterId(font.masters[0].id))
    );
}

// ---------------------------------------------------------------------------
// Glyph names
// ---------------------------------------------------------------------------

#[test]
fn glyph_name_rules() {
    assert!(is_valid_glyph_name("a"));
    assert!(is_valid_glyph_name("A.sc"));
    assert!(is_valid_glyph_name(".notdef"));
    assert!(is_valid_glyph_name("uni0041"));
    assert!(is_valid_glyph_name("a_b-c"));
    assert!(is_valid_glyph_name(&"a".repeat(63)));

    assert!(!is_valid_glyph_name(""));
    assert!(!is_valid_glyph_name("1a"));
    assert!(!is_valid_glyph_name("a b"));
    assert!(!is_valid_glyph_name(" a"));
    assert!(!is_valid_glyph_name("a\tb"));
    assert!(!is_valid_glyph_name(".foo"));
    assert!(!is_valid_glyph_name(".notdef2"));
    assert!(!is_valid_glyph_name("é"));
    assert!(!is_valid_glyph_name(&"a".repeat(64)));
}

// ---------------------------------------------------------------------------
// Actor
// ---------------------------------------------------------------------------

#[test]
fn actor_serde_roundtrip() {
    let cases = [
        (Actor::User, "\"user\""),
        (Actor::Cli, "\"cli\""),
        (
            Actor::Agent("claude-code".to_owned()),
            "\"agent:claude-code\"",
        ),
    ];
    for (actor, json) in cases {
        assert_eq!(serde_json::to_string(&actor).unwrap(), json);
        assert_eq!(serde_json::from_str::<Actor>(json).unwrap(), actor);
    }
}

#[test]
fn actor_display_matches_serialized_form() {
    assert_eq!(Actor::User.to_string(), "user");
    assert_eq!(Actor::Cli.to_string(), "cli");
    assert_eq!(
        Actor::Agent("claude-code".to_owned()).to_string(),
        "agent:claude-code"
    );
    // Names may themselves contain colons; only the first one is a separator.
    let actor = Actor::Agent("vendor:tool".to_owned());
    let json = serde_json::to_string(&actor).unwrap();
    assert_eq!(json, "\"agent:vendor:tool\"");
    assert_eq!(serde_json::from_str::<Actor>(&json).unwrap(), actor);
}

#[test]
fn actor_rejects_invalid_values() {
    for json in [
        "\"agent:\"",
        "\"agent\"",
        "\"USER\"",
        "\"root\"",
        "\"\"",
        "7",
    ] {
        assert!(serde_json::from_str::<Actor>(json).is_err(), "{json}");
    }
    // An unnamed agent has no valid serialized form.
    assert!(serde_json::to_string(&Actor::Agent(String::new())).is_err());
}

#[test]
fn actor_parses_from_str() {
    assert_eq!("cli".parse::<Actor>(), Ok(Actor::Cli));
    assert_eq!("agent:x".parse::<Actor>(), Ok(Actor::Agent("x".to_owned())));
    assert_eq!(
        "agent:".parse::<Actor>(),
        Err(ModelError::InvalidActor("agent:".to_owned()))
    );
}

// ---------------------------------------------------------------------------
// Identifiers, revisions, node kinds
// ---------------------------------------------------------------------------

#[test]
fn ids_are_random_v4_and_serialize_as_uuid_strings() {
    let id = GlyphId::new();
    assert_eq!(id.0.get_version_num(), 4);
    assert_ne!(id, GlyphId::new());

    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, format!("\"{}\"", id.0.hyphenated()));
    assert_eq!(serde_json::from_str::<GlyphId>(&json).unwrap(), id);

    assert_eq!(id.to_string().parse::<GlyphId>().unwrap(), id);
    assert!("not-a-uuid".parse::<GlyphId>().is_err());

    let font_id = FontId::new();
    assert_eq!(
        serde_json::from_str::<FontId>(&serde_json::to_string(&font_id).unwrap()).unwrap(),
        font_id
    );
    let master_id = MasterId::new();
    assert_eq!(
        serde_json::from_str::<MasterId>(&serde_json::to_string(&master_id).unwrap()).unwrap(),
        master_id
    );
}

#[test]
fn path_id_and_rev_serialize_as_bare_numbers() {
    assert_eq!(serde_json::to_string(&PathId(7)).unwrap(), "7");
    assert_eq!(serde_json::to_string(&Rev(42)).unwrap(), "42");
    assert_eq!(serde_json::from_str::<Rev>("42").unwrap(), Rev(42));
    assert_eq!(Rev::default(), Rev(0));
    assert!(Rev(1) < Rev(2));
}

#[test]
fn node_kind_serializes_lowercase() {
    let cases = [
        (NodeKind::Line, "\"line\""),
        (NodeKind::Curve, "\"curve\""),
        (NodeKind::QCurve, "\"qcurve\""),
        (NodeKind::OffCurve, "\"offcurve\""),
    ];
    for (kind, json) in cases {
        assert_eq!(serde_json::to_string(&kind).unwrap(), json);
        assert_eq!(serde_json::from_str::<NodeKind>(json).unwrap(), kind);
    }
}

// ---------------------------------------------------------------------------
// EntityKey
// ---------------------------------------------------------------------------

#[test]
fn entity_key_string_forms() {
    let master = MasterId(Uuid::from_u128(1));
    let glyph = gid(2);

    let cases = [
        (EntityKey::FontInfo, "font_info".to_owned()),
        (
            EntityKey::VerticalMetrics(master),
            format!("vertical_metrics:{master}"),
        ),
        (EntityKey::GlyphOrder, "glyph_order".to_owned()),
        (EntityKey::Glyph(glyph), format!("glyph:{glyph}")),
    ];
    for (key, text) in cases {
        assert_eq!(key.to_string(), text);
        assert_eq!(text.parse::<EntityKey>(), Ok(key));
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(serde_json::from_str::<EntityKey>(&json).unwrap(), key);
    }
}

#[test]
fn entity_key_rejects_malformed_strings() {
    for text in [
        "",
        "font_info:",
        "glyph",
        "glyph:",
        "glyph:not-a-uuid",
        "glyph_order:00000000-0000-0000-0000-000000000001",
        "vertical_metrics",
        "vertical_metrics:00000000000000000000000000000001",
        "layer:00000000-0000-0000-0000-000000000001",
    ] {
        assert_eq!(
            text.parse::<EntityKey>(),
            Err(ModelError::InvalidEntityKey(text.to_owned())),
            "{text}"
        );
    }
}

#[test]
fn entity_key_is_ordered_and_usable_as_json_map_key() {
    let master = MasterId(Uuid::from_u128(5));
    let mut map = BTreeMap::new();
    map.insert(EntityKey::Glyph(gid(2)), Rev(4));
    map.insert(EntityKey::GlyphOrder, Rev(3));
    map.insert(EntityKey::VerticalMetrics(master), Rev(2));
    map.insert(EntityKey::FontInfo, Rev(1));
    map.insert(EntityKey::Glyph(gid(1)), Rev(5));

    // Variant order first, then id order.
    let keys: Vec<_> = map.keys().copied().collect();
    assert_eq!(
        keys,
        vec![
            EntityKey::FontInfo,
            EntityKey::VerticalMetrics(master),
            EntityKey::GlyphOrder,
            EntityKey::Glyph(gid(1)),
            EntityKey::Glyph(gid(2)),
        ]
    );

    let json = serde_json::to_string(&map).unwrap();
    assert!(json.contains("\"font_info\":1"), "{json}");
    assert!(json.contains(&format!("\"glyph:{}\":4", gid(2))), "{json}");
    let back: BTreeMap<EntityKey, Rev> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, map);
}

// ---------------------------------------------------------------------------
// Whole-document serialization
// ---------------------------------------------------------------------------

#[test]
fn font_json_roundtrip() {
    let mut font = Font::new("Test Sans", 2048);
    font.info.copyright = Some("Copyright (c) 2026".to_owned());
    font.info.designer = Some("Ada".to_owned());
    font.lib.insert(
        "com.example.note".to_owned(),
        serde_json::json!({ "keep": [1, 2, 3], "me": null }),
    );

    let master = font.masters[0].id;
    let mut a = glyph(gid(1), "A", &[0x41]);
    a.layers.insert(master, square_layer());
    a.layers.get_mut(&master).unwrap().paths[0]
        .nodes
        .push(Node {
            x: 12.5,
            y: -3.25,
            kind: NodeKind::OffCurve,
            smooth: true,
        });
    add_glyph(&mut font, a);
    add_glyph(&mut font, glyph(gid(2), ".notdef", &[]));

    let json = serde_json::to_string(&font).unwrap();
    let back: Font = serde_json::from_str(&json).unwrap();

    assert_eq!(back, font);
    assert_eq!(back.validate_invariants(), Ok(()));

    // Pretty printing must round-trip too.
    let pretty = serde_json::to_string_pretty(&font).unwrap();
    assert_eq!(serde_json::from_str::<Font>(&pretty).unwrap(), font);
}

#[test]
fn font_json_shape_is_stable() {
    let mut font = Font::new("Test Sans", 1000);
    add_glyph(&mut font, glyph(gid(1), "A", &[0x41]));

    let value = serde_json::to_value(&font).unwrap();
    assert_eq!(value["units_per_em"], 1000);
    assert_eq!(value["info"]["family_name"], "Test Sans");
    assert_eq!(value["masters"][0]["name"], "Regular");
    assert_eq!(value["masters"][0]["metrics"]["ascender"], 800.0);
    assert_eq!(value["glyph_order"][0], gid(1).to_string());
    assert_eq!(value["glyphs"][gid(1).to_string()]["name"], "A");
    assert_eq!(value["glyphs"][gid(1).to_string()]["unicodes"][0], 0x41);
}

#[test]
fn json_schemas_can_be_generated() {
    let schema = serde_json::to_value(schemars::schema_for!(Font)).unwrap();
    assert!(schema["properties"]["units_per_em"].is_object());
    assert!(schema["properties"]["glyphs"].is_object());

    let actor = serde_json::to_value(schemars::schema_for!(Actor)).unwrap();
    assert_eq!(actor["type"], "string");
    let key = serde_json::to_value(schemars::schema_for!(EntityKey)).unwrap();
    assert_eq!(key["type"], "string");
}
