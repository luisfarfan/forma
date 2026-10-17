use std::collections::BTreeMap;
use std::sync::Arc;

use forma_model::{
    Actor, Component, EntityKey, Font, FontId, Glyph, GlyphId, KernRef, KernSide, Kerning,
    KerningGroup, KerningGroupId, Layer, MasterId, ModelError, Node, NodeKind, Path, PathId, Rev,
    Transform, is_valid_glyph_name,
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
        components: Vec::new(),
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

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

const IDENTITY: Transform = Transform::IDENTITY;

/// A glyph whose default-master layer is made of `components` only.
fn composite(font: &Font, id: GlyphId, name: &str, components: Vec<Component>) -> Glyph {
    let mut glyph = glyph(id, name, &[]);
    glyph.layers.insert(
        font.default_master().id,
        Layer {
            advance_width: 600.0,
            paths: Vec::new(),
            components,
        },
    );
    glyph
}

fn component(base: GlyphId) -> Component {
    Component {
        base,
        transform: IDENTITY,
    }
}

/// A glyph drawn as a square in the default master of `font`.
fn drawn(font: &Font, id: GlyphId, name: &str) -> Glyph {
    let mut glyph = glyph(id, name, &[]);
    glyph
        .layers
        .insert(font.default_master().id, square_layer());
    glyph
}

#[test]
fn transform_identity_and_determinant() {
    assert_eq!(IDENTITY.apply(3.0, 4.0), (3.0, 4.0));
    assert_eq!(Transform::default(), IDENTITY);
    assert_eq!(IDENTITY.determinant(), 1.0);
    assert!(IDENTITY.is_valid());

    let scaled = Transform {
        xx: 2.0,
        ..IDENTITY
    };
    assert_eq!(scaled.determinant(), 2.0);
    assert!(scaled.is_valid());

    let singular = Transform {
        xx: 1.0,
        xy: 2.0,
        yx: 2.0,
        yy: 4.0,
        dx: 0.0,
        dy: 0.0,
    };
    assert_eq!(singular.determinant(), 0.0);
    assert!(!singular.is_valid());
    // Nearly singular (|det| < 1e-6) and non-finite transforms are invalid.
    assert!(
        !Transform {
            xx: 1e-4,
            yy: 1e-3,
            ..IDENTITY
        }
        .is_valid()
    );
    assert!(
        Transform {
            xx: 1e-3,
            yy: 1e-3,
            ..IDENTITY
        }
        .is_valid()
    );
    assert!(
        !Transform {
            dx: f64::NAN,
            ..IDENTITY
        }
        .is_valid()
    );
    assert!(
        !Transform {
            yy: f64::INFINITY,
            ..IDENTITY
        }
        .is_valid()
    );

    // x' = xx·x + yx·y + dx, y' = xy·x + yy·y + dy.
    let t = Transform {
        xx: 1.0,
        xy: 2.0,
        yx: 3.0,
        yy: 4.0,
        dx: 5.0,
        dy: 6.0,
    };
    assert_eq!(t.apply(1.0, 1.0), (9.0, 12.0));
    assert_eq!(t.apply(2.0, 0.0), (7.0, 10.0));

    // `then` applies `self` first and `outer` second.
    let shift = Transform {
        dx: 10.0,
        ..IDENTITY
    };
    let double = Transform {
        xx: 2.0,
        yy: 2.0,
        ..IDENTITY
    };
    assert_eq!(shift.then(&double).apply(1.0, 1.0), (22.0, 2.0));
    assert_eq!(double.then(&shift).apply(1.0, 1.0), (12.0, 2.0));
    let rotate = Transform {
        xx: 0.0,
        xy: 1.0,
        yx: -1.0,
        yy: 0.0,
        dx: 3.0,
        dy: -2.0,
    };
    for (x, y) in [(0.0, 0.0), (1.0, -2.0), (7.5, 3.25)] {
        let (ix, iy) = t.apply(x, y);
        assert_eq!(t.then(&rotate).apply(x, y), rotate.apply(ix, iy));
    }
}

#[test]
fn component_roundtrip_and_v1_json() {
    // A layer written before components existed.
    let v1 = r#"{"advance_width":500.0,"paths":[]}"#;
    let layer: Layer = serde_json::from_str(v1).unwrap();
    assert!(layer.components.is_empty());
    // Without components the layer serializes exactly as in v1.
    assert_eq!(serde_json::to_string(&layer).unwrap(), v1);

    let with = Layer {
        advance_width: 600.0,
        paths: Vec::new(),
        components: vec![Component {
            base: gid(1),
            transform: Transform {
                dx: 20.0,
                ..IDENTITY
            },
        }],
    };
    let json = serde_json::to_string(&with).unwrap();
    assert_eq!(serde_json::from_str::<Layer>(&json).unwrap(), with);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["components"][0]["base"], gid(1).to_string());
    assert_eq!(value["components"][0]["transform"]["dx"], 20.0);
    assert_eq!(value["components"][0]["transform"]["xx"], 1.0);

    // The transform defaults to the identity.
    let bare: Component = serde_json::from_str(&format!(r#"{{"base":"{}"}}"#, gid(1))).unwrap();
    assert_eq!(bare, component(gid(1)));
}

#[test]
fn component_invariants() {
    let master = Font::new("T", 1000).masters[0].id;
    let base_font = || {
        let mut font = Font::new("T", 1000);
        font.masters[0].id = master;
        let a = drawn(&font, gid(1), "A");
        add_glyph(&mut font, a);
        font
    };

    // A valid composite.
    let mut font = base_font();
    let acute = composite(&font, gid(2), "Aacute", vec![component(gid(1))]);
    add_glyph(&mut font, acute);
    assert_eq!(font.validate_invariants(), Ok(()));

    // An unknown base.
    let mut font = base_font();
    let broken = composite(&font, gid(2), "C", vec![component(gid(99))]);
    add_glyph(&mut font, broken);
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::UnknownComponentBase {
            glyph: "C".to_owned(),
            base: gid(99),
        })
    );

    // A → B → A.
    let mut font = Font::new("T", 1000);
    let a = composite(&font, gid(1), "A", vec![component(gid(2))]);
    let b = composite(&font, gid(2), "B", vec![component(gid(1))]);
    add_glyph(&mut font, a);
    add_glyph(&mut font, b);
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::ComponentCycle {
            glyph: "A".to_owned()
        })
    );

    // A glyph that uses itself.
    let mut font = Font::new("T", 1000);
    let selfish = composite(&font, gid(1), "A", vec![component(gid(1))]);
    add_glyph(&mut font, selfish);
    assert!(matches!(
        font.validate_invariants(),
        Err(ModelError::ComponentCycle { .. })
    ));

    // A singular transform.
    let mut font = base_font();
    let flat = composite(
        &font,
        gid(2),
        "C",
        vec![Component {
            base: gid(1),
            transform: Transform {
                yy: 0.0,
                ..IDENTITY
            },
        }],
    );
    add_glyph(&mut font, flat);
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::InvalidTransform {
            glyph: "C".to_owned()
        })
    );

    // A base without a layer in the master of the component.
    let mut font = Font::new("T", 1000);
    font.masters[0].id = master;
    add_glyph(&mut font, glyph(gid(1), "A", &[0x41]));
    let user = composite(&font, gid(2), "C", vec![component(gid(1))]);
    add_glyph(&mut font, user);
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::ComponentMissingLayer {
            glyph: "C".to_owned(),
            base: "A".to_owned(),
            master,
        })
    );
}

#[test]
fn components_using_lists_users_once_in_glyph_order() {
    let mut font = Font::new("T", 1000);
    let second = MasterId(Uuid::from_u128(0x5ec));
    font.masters.push(forma_model::Master {
        id: second,
        ..font.masters[0].clone()
    });
    let mut a = drawn(&font, gid(1), "A");
    a.layers.insert(second, square_layer());
    // `Z` comes first in the glyph order and uses `A` in both masters.
    let mut z = composite(
        &font,
        gid(9),
        "Z",
        vec![component(gid(1)), component(gid(1))],
    );
    z.layers.insert(
        second,
        Layer {
            components: vec![component(gid(1))],
            ..square_layer()
        },
    );
    let b = composite(&font, gid(2), "B", vec![component(gid(1))]);
    let c = drawn(&font, gid(3), "C");
    add_glyph(&mut font, z);
    add_glyph(&mut font, a);
    add_glyph(&mut font, b);
    add_glyph(&mut font, c);
    assert_eq!(font.validate_invariants(), Ok(()));

    assert_eq!(font.components_using(gid(1)), vec![gid(9), gid(2)]);
    assert!(font.components_using(gid(3)).is_empty());
    assert!(font.components_using(gid(99)).is_empty());
}

// ---------------------------------------------------------------------------
// Kerning
// ---------------------------------------------------------------------------

fn kid(n: u128) -> KerningGroupId {
    KerningGroupId(Uuid::from_u128(n))
}

fn group(id: KerningGroupId, name: &str, side: KernSide, members: &[GlyphId]) -> KerningGroup {
    KerningGroup {
        id,
        name: name.to_owned(),
        side,
        members: members.to_vec(),
    }
}

/// A font with the glyphs `A` (1), `V` (2), `O` (3) and `T` (4).
fn kerning_font() -> Font {
    let mut font = Font::new("Kern", 1000);
    for (n, name) in [(1, "A"), (2, "V"), (3, "O"), (4, "T")] {
        let glyph = drawn(&font, gid(n), name);
        add_glyph(&mut font, glyph);
    }
    font
}

fn add_group(font: &mut Font, group: KerningGroup) {
    font.kerning.groups.insert(group.id, group);
}

fn set_pair(font: &mut Font, first: KernRef, second: KernRef, value: f64) {
    let master = font.masters[0].id;
    font.kerning
        .pairs
        .entry(master)
        .or_default()
        .insert((first, second), value);
}

#[test]
fn kerning_serde_is_canonical() {
    let mut font = kerning_font();
    let master = font.masters[0].id;
    add_group(&mut font, group(kid(20), "V", KernSide::Second, &[gid(2)]));
    add_group(&mut font, group(kid(10), "O", KernSide::First, &[gid(3)]));
    // Inserted out of order.
    set_pair(
        &mut font,
        KernRef::Group(kid(10)),
        KernRef::Glyph(gid(2)),
        -40.0,
    );
    set_pair(
        &mut font,
        KernRef::Glyph(gid(4)),
        KernRef::Group(kid(20)),
        -60.0,
    );
    set_pair(
        &mut font,
        KernRef::Glyph(gid(1)),
        KernRef::Glyph(gid(2)),
        -80.0,
    );
    assert_eq!(font.validate_invariants(), Ok(()));

    let value = serde_json::to_value(&font).unwrap();
    assert_eq!(
        value["kerning"]["groups"],
        serde_json::json!([
            {"id": kid(10).to_string(), "name": "O", "side": "first", "members": [gid(3).to_string()]},
            {"id": kid(20).to_string(), "name": "V", "side": "second", "members": [gid(2).to_string()]},
        ])
    );
    assert_eq!(
        value["kerning"]["pairs"][master.to_string()],
        serde_json::json!([
            {"first": format!("g.{}", gid(1)), "second": format!("g.{}", gid(2)), "value": -80.0},
            {"first": format!("g.{}", gid(4)), "second": format!("k.{}", kid(20)), "value": -60.0},
            {"first": format!("k.{}", kid(10)), "second": format!("g.{}", gid(2)), "value": -40.0},
        ])
    );
    let back: Font = serde_json::from_value(value).unwrap();
    assert_eq!(back, font);

    // A font without kerning does not emit the key, and reads back without it.
    let plain = kerning_font();
    let value = serde_json::to_value(&plain).unwrap();
    assert!(value.get("kerning").is_none(), "{value}");
    let back: Font = serde_json::from_value(value).unwrap();
    assert!(back.kerning.is_empty());
    assert_eq!(back, plain);
}

#[test]
fn kerning_json_rejects_duplicates() {
    let master = MasterId(Uuid::from_u128(1));
    let pair = serde_json::json!({
        "first": format!("g.{}", gid(1)), "second": format!("g.{}", gid(2)), "value": -10.0
    });
    let twice = serde_json::json!({"groups": [], "pairs": {master.to_string(): [pair, pair]}});
    assert!(serde_json::from_value::<Kerning>(twice).is_err());

    let one =
        serde_json::json!({"id": kid(1).to_string(), "name": "O", "side": "first", "members": []});
    let twice = serde_json::json!({"groups": [one, one], "pairs": {}});
    assert!(serde_json::from_value::<Kerning>(twice).is_err());

    let once = serde_json::json!({"groups": [one], "pairs": {master.to_string(): [pair]}});
    let kerning: Kerning = serde_json::from_value(once).unwrap();
    assert_eq!(kerning.groups.len(), 1);
    assert_eq!(
        kerning.pair(master, KernRef::Glyph(gid(1)), KernRef::Glyph(gid(2))),
        Some(-10.0)
    );
}

#[test]
fn kerning_invariants() {
    // A glyph in two groups of the same side.
    let mut font = kerning_font();
    add_group(&mut font, group(kid(1), "O", KernSide::First, &[gid(3)]));
    add_group(
        &mut font,
        group(kid(2), "round", KernSide::First, &[gid(3)]),
    );
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::GlyphInTwoGroups {
            glyph: "O".to_owned(),
            side: KernSide::First,
        })
    );
    // One group per side is fine.
    font.kerning.groups.get_mut(&kid(2)).unwrap().side = KernSide::Second;
    assert_eq!(font.validate_invariants(), Ok(()));

    // A second-side group used as the first member of a pair.
    let mut font = kerning_font();
    add_group(&mut font, group(kid(1), "V", KernSide::Second, &[gid(2)]));
    set_pair(
        &mut font,
        KernRef::Group(kid(1)),
        KernRef::Glyph(gid(1)),
        -20.0,
    );
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::KerningGroupWrongSide {
            group: "V".to_owned()
        })
    );

    // Zero, NaN and out-of-range values.
    let pair = (KernRef::Glyph(gid(1)), KernRef::Glyph(gid(2)));
    let mut font = kerning_font();
    set_pair(&mut font, pair.0, pair.1, 0.0);
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::InvalidKerningValue { value: 0.0 })
    );
    set_pair(&mut font, pair.0, pair.1, f64::NAN);
    assert!(matches!(
        font.validate_invariants(),
        Err(ModelError::InvalidKerningValue { value }) if value.is_nan()
    ));
    set_pair(&mut font, pair.0, pair.1, 2000.5);
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::InvalidKerningValue { value: 2000.5 })
    );
    for value in [-2000.0, 2000.0, -0.5] {
        set_pair(&mut font, pair.0, pair.1, value);
        assert_eq!(font.validate_invariants(), Ok(()), "{value}");
    }

    // Group names follow the glyph name rules and are unique per side.
    let mut font = kerning_font();
    add_group(&mut font, group(kid(1), "1x", KernSide::First, &[]));
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::InvalidGroupName("1x".to_owned()))
    );
    let mut font = kerning_font();
    add_group(&mut font, group(kid(1), "O", KernSide::First, &[gid(3)]));
    add_group(&mut font, group(kid(2), "O", KernSide::Second, &[gid(3)]));
    assert_eq!(font.validate_invariants(), Ok(()));
    add_group(&mut font, group(kid(3), "O", KernSide::First, &[gid(1)]));
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::DuplicateGroupName {
            side: KernSide::First,
            name: "O".to_owned(),
        })
    );

    // References that do not exist.
    let mut font = kerning_font();
    add_group(&mut font, group(kid(1), "O", KernSide::First, &[gid(99)]));
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::KerningUnknownGlyph(gid(99)))
    );
    let mut font = kerning_font();
    set_pair(
        &mut font,
        KernRef::Glyph(gid(98)),
        KernRef::Glyph(gid(1)),
        -5.0,
    );
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::KerningUnknownGlyph(gid(98)))
    );
    let mut font = kerning_font();
    set_pair(
        &mut font,
        KernRef::Glyph(gid(1)),
        KernRef::Group(kid(7)),
        -5.0,
    );
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::KerningUnknownGroup(kid(7)))
    );
    let mut font = kerning_font();
    let stranger = MasterId(Uuid::from_u128(0xdead));
    font.kerning
        .pairs
        .entry(stranger)
        .or_default()
        .insert(pair, -5.0);
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::KerningUnknownMaster(stranger))
    );

    // A group stored under another id.
    let mut font = kerning_font();
    font.kerning
        .groups
        .insert(kid(1), group(kid(2), "O", KernSide::First, &[]));
    assert_eq!(
        font.validate_invariants(),
        Err(ModelError::GroupKeyMismatch {
            key: kid(1),
            id: kid(2),
        })
    );
}

#[test]
fn effective_kerning_lookup_order() {
    let mut font = kerning_font();
    let master = font.masters[0].id;
    let (a, v) = (gid(1), gid(2));
    add_group(&mut font, group(kid(10), "A", KernSide::First, &[a]));
    add_group(&mut font, group(kid(11), "V", KernSide::Second, &[v]));
    let glyph_glyph = (KernRef::Glyph(a), KernRef::Glyph(v));
    let glyph_group = (KernRef::Glyph(a), KernRef::Group(kid(11)));
    let group_glyph = (KernRef::Group(kid(10)), KernRef::Glyph(v));
    let group_group = (KernRef::Group(kid(10)), KernRef::Group(kid(11)));
    set_pair(&mut font, glyph_glyph.0, glyph_glyph.1, -50.0);
    set_pair(&mut font, glyph_group.0, glyph_group.1, -30.0);
    set_pair(&mut font, group_group.0, group_group.1, -10.0);
    assert_eq!(font.validate_invariants(), Ok(()));

    let kerning = &mut font.kerning;
    assert_eq!(kerning.group_of(a, KernSide::First), Some(kid(10)));
    assert_eq!(kerning.group_of(a, KernSide::Second), None);
    assert_eq!(kerning.group_of(v, KernSide::Second), Some(kid(11)));
    assert_eq!(
        kerning.group_by_name(KernSide::Second, "V").map(|g| g.id),
        Some(kid(11))
    );
    assert!(kerning.group_by_name(KernSide::First, "V").is_none());
    assert_eq!(
        kerning.pair(master, glyph_group.0, glyph_group.1),
        Some(-30.0)
    );
    assert_eq!(kerning.pair(master, group_glyph.0, group_glyph.1), None);

    assert_eq!(kerning.effective(master, a, v), -50.0);
    let pairs = kerning.pairs.get_mut(&master).unwrap();
    pairs.remove(&glyph_glyph);
    assert_eq!(kerning.effective(master, a, v), -30.0);
    let pairs = kerning.pairs.get_mut(&master).unwrap();
    pairs.remove(&glyph_group);
    pairs.insert(group_glyph, -20.0);
    assert_eq!(kerning.effective(master, a, v), -20.0);
    let pairs = kerning.pairs.get_mut(&master).unwrap();
    pairs.remove(&group_glyph);
    assert_eq!(kerning.effective(master, a, v), -10.0);
    // Nothing for the reversed pair, another master or no pairs at all.
    assert_eq!(kerning.effective(master, v, a), 0.0);
    assert_eq!(kerning.effective(MasterId(Uuid::from_u128(77)), a, v), 0.0);
    kerning.pairs.clear();
    assert_eq!(kerning.effective(master, a, v), 0.0);
    assert!(!kerning.is_empty());
    kerning.groups.clear();
    assert!(kerning.is_empty());
}

#[test]
fn kern_ref_text_forms() {
    let cases = [
        (KernRef::Glyph(gid(1)), format!("g.{}", gid(1))),
        (KernRef::Group(kid(2)), format!("k.{}", kid(2))),
    ];
    for (reference, text) in cases {
        assert_eq!(reference.to_string(), text);
        assert_eq!(text.parse::<KernRef>(), Ok(reference));
        let json = serde_json::to_string(&reference).unwrap();
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(serde_json::from_str::<KernRef>(&json).unwrap(), reference);
    }
    // Uppercase hexadecimal digits are not canonical.
    let upper = Uuid::from_u128(0xabc)
        .hyphenated()
        .to_string()
        .to_uppercase();
    for text in [
        String::new(),
        gid(1).to_string(),
        format!("G.{}", gid(1)),
        format!("x.{}", gid(1)),
        format!("g:{}", gid(1)),
        format!("g.{upper}"),
        format!("k.{upper}"),
        format!("g.{}", gid(1).0.simple()),
        "g.".to_owned(),
    ] {
        assert!(text.parse::<KernRef>().is_err(), "{text}");
        assert!(serde_json::from_value::<KernRef>(text.clone().into()).is_err());
    }
    // Glyph references sort before group references.
    assert!(KernRef::Glyph(gid(9)) < KernRef::Group(kid(1)));
}

#[test]
fn entity_key_text_forms() {
    let master = MasterId(Uuid::from_u128(1));
    let glyph = gid(2);
    let group = kid(0xabc);

    let cases = [
        (
            EntityKey::KerningGroup(group),
            format!("kerning_group:{group}"),
        ),
        (
            EntityKey::KerningPair(master, KernRef::Glyph(glyph), KernRef::Group(group)),
            format!("kerning_pair:{master}:g.{glyph}:k.{group}"),
        ),
        (
            EntityKey::KerningPair(master, KernRef::Group(group), KernRef::Glyph(glyph)),
            format!("kerning_pair:{master}:k.{group}:g.{glyph}"),
        ),
    ];
    for (key, text) in cases {
        assert_eq!(key.to_string(), text);
        assert_eq!(text.parse::<EntityKey>(), Ok(key));
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(serde_json::from_str::<EntityKey>(&json).unwrap(), key);
    }

    let upper = group.0.hyphenated().to_string().to_uppercase();
    for text in [
        "kerning_group".to_owned(),
        "kerning_group:".to_owned(),
        format!("kerning_group:{upper}"),
        format!("kerning_group:{}", group.0.simple()),
        format!("kerning_group:k.{group}"),
        format!("kerning_pair:{master}"),
        format!("kerning_pair:{master}:g.{glyph}"),
        format!("kerning_pair:{master}:{glyph}:k.{group}"),
        format!("kerning_pair:{master}:G.{glyph}:k.{group}"),
        format!("kerning_pair:{master}:g.{glyph}:k.{upper}"),
        format!("kerning_pair:{master}:g.{glyph}:k.{group}:"),
        format!("kerning_pair:{master}:g.{glyph}:k.{group}:g.{glyph}"),
        format!("kerning_pair:g.{glyph}:k.{group}"),
        format!("kerning_pair::g.{glyph}:k.{group}"),
        format!("Kerning_pair:{master}:g.{glyph}:k.{group}"),
    ] {
        assert_eq!(
            text.parse::<EntityKey>(),
            Err(ModelError::InvalidEntityKey(text.clone())),
            "{text}"
        );
    }

    // The new keys sort after every older one, groups before pairs.
    let mut keys = vec![
        EntityKey::KerningPair(master, KernRef::Glyph(glyph), KernRef::Glyph(glyph)),
        EntityKey::KerningGroup(group),
        EntityKey::Glyph(glyph),
        EntityKey::FontInfo,
    ];
    keys.sort();
    assert_eq!(
        keys,
        vec![
            EntityKey::FontInfo,
            EntityKey::Glyph(glyph),
            EntityKey::KerningGroup(group),
            EntityKey::KerningPair(master, KernRef::Glyph(glyph), KernRef::Glyph(glyph)),
        ]
    );

    // The schema pattern lists the new forms.
    let schema = serde_json::to_value(schemars::schema_for!(EntityKey)).unwrap();
    let pattern = schema["pattern"].as_str().unwrap();
    assert!(pattern.contains("kerning_group"), "{pattern}");
    assert!(pattern.contains("kerning_pair"), "{pattern}");
    let reference = serde_json::to_value(schemars::schema_for!(KernRef)).unwrap();
    assert_eq!(reference["type"], "string");
    assert!(reference["pattern"].as_str().unwrap().contains("[gk]"));
}
