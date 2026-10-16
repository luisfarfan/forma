//! Property tests: any valid font survives a save/load round trip in both
//! formats, and converting between them, bit for bit.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use forma_model::{
    Font, FontId, FontInfo, Glyph, GlyphId, Layer, Master, MasterId, Node, NodeKind,
    Path as Contour, PathId, VerticalMetrics,
};
use forma_store::{load, pack, save, unpack};
use proptest::collection::vec;
use proptest::prelude::*;
use serde_json::{Map, Number, Value};
use tempfile::TempDir;
use uuid::Uuid;

const CASES: u32 = 500;

/// Names that are hard on file systems: upper case, `_`, `.`, `-`, Windows
/// device names, characters that are illegal in file names.
const AWKWARD_NAMES: &[&str] = &[
    "a", "A", "a_", "A_", "_", "-", "a-b", "a.b", "a.", ".notdef", "con", "CON", "Con", "CON.alt",
    "con.alt", "prn", "AUX", "nul", "NUL.x", "com1", "COM9", "lpt1", "LPT9.sc", "a#2", "A#2",
    "a*b", "a\"b", "a/b", "a\\b", "a:b", "a<b>", "a?b", "a[b]", "a|b", "a+b", "x~",
];

/// Names whose file names collide on a file system that ignores case: `A` and
/// `a_` both become `a_.json`, `A_` and `a__` become `a__.json`, `A#2` and
/// `a_#2` become `a_#2.json`, and `a_#2` is also what numbering `a_` gives.
const COLLIDING_NAMES: &[&str] = &["a", "A", "a_", "A_", "a__", "a_#2", "A#2", "a#2"];

fn glyph_name() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => proptest::sample::select(AWKWARD_NAMES).prop_map(str::to_owned),
        3 => proptest::sample::select(COLLIDING_NAMES).prop_map(str::to_owned),
        // Anything printable; a name may not start with a digit or a period.
        3 => "[!-~]{1,10}".prop_map(|name| {
            if name.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
                format!("g{name}")
            } else {
                name
            }
        }),
        2 => "[A-Za-z_][A-Za-z0-9_.-]{0,12}",
        // The longest name allowed.
        1 => "[A-Za-z]{63}",
    ]
}

fn finite() -> impl Strategy<Value = f64> {
    prop_oneof![
        3 => -2000.0f64..2000.0,
        // Every bit pattern of a finite double, including subnormals and zeros.
        1 => any::<f64>().prop_filter("finite", |v| v.is_finite()),
        1 => (-4000i32..4000).prop_map(|n| f64::from(n) / 8.0),
    ]
}

fn node() -> impl Strategy<Value = Node> {
    (
        finite(),
        finite(),
        prop_oneof![
            Just(NodeKind::Line),
            Just(NodeKind::Curve),
            Just(NodeKind::QCurve),
            Just(NodeKind::OffCurve),
        ],
        any::<bool>(),
    )
        .prop_map(|(x, y, kind, smooth)| Node { x, y, kind, smooth })
}

fn contours() -> impl Strategy<Value = Vec<Contour>> {
    vec((any::<bool>(), vec(node(), 0..8)), 0..4).prop_map(|contours| {
        contours
            .into_iter()
            .enumerate()
            .map(|(n, (closed, nodes))| Contour {
                id: PathId(u32::try_from(n).unwrap_or(0) + 1),
                closed,
                nodes,
            })
            .collect()
    })
}

fn layer() -> impl Strategy<Value = Layer> {
    (finite(), contours()).prop_map(|(advance_width, paths)| Layer {
        advance_width,
        paths,
    })
}

/// What the strategy draws for one glyph, before ids and masters are known.
type GlyphSeed = (String, Vec<u32>, bool, Vec<Option<Layer>>);

fn glyph_seed() -> impl Strategy<Value = GlyphSeed> {
    (
        glyph_name(),
        vec(0x20u32..0x3000, 0..4),
        any::<bool>(),
        vec(proptest::option::of(layer()), 2),
    )
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        any::<u64>().prop_map(|n| Value::Number(n.into())),
        finite().prop_map(|v| Number::from_f64(v).map_or(Value::Null, Value::Number)),
        ".{0,12}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..4).prop_map(Value::Array),
            vec((".{0,6}", inner), 0..4).prop_map(|entries| {
                let map: Map<String, Value> = entries.into_iter().collect();
                Value::Object(map)
            }),
        ]
    })
}

fn metrics() -> impl Strategy<Value = VerticalMetrics> {
    (finite(), finite(), finite(), finite()).prop_map(
        |(ascender, descender, x_height, cap_height)| VerticalMetrics {
            ascender,
            descender,
            x_height,
            cap_height,
        },
    )
}

fn arb_font() -> impl Strategy<Value = Font> {
    (
        any::<u128>(),
        (".{0,16}", ".{0,8}", any::<u16>(), any::<u16>()),
        (
            proptest::option::of(".{0,20}"),
            proptest::option::of(".{0,20}"),
        ),
        16u16..=16384,
        vec(("[A-Za-z ]{0,8}", metrics()), 1..=2),
        any::<u128>(),
        vec(glyph_seed(), 0..=20),
        any::<u64>(),
        vec((".{0,10}", json_value()), 0..4),
    )
        .prop_map(
            |(
                font_id,
                (family_name, style_name, version_major, version_minor),
                (copyright, designer),
                units_per_em,
                masters,
                master_salt,
                seeds,
                shuffle,
                lib,
            )| {
                let masters: Vec<Master> = masters
                    .into_iter()
                    .enumerate()
                    .map(|(n, (name, metrics))| Master {
                        id: MasterId(Uuid::from_u128(master_salt ^ (n as u128 + 1))),
                        name,
                        metrics,
                    })
                    .collect();

                // Drop repeated names and repeated code points so the font is
                // valid, then give every glyph a distinct id.
                let mut names = HashSet::new();
                let mut codepoints = HashSet::new();
                let mut glyphs = BTreeMap::new();
                let mut order = Vec::new();
                for (n, (name, unicodes, export, layers)) in seeds.into_iter().enumerate() {
                    if !names.insert(name.clone()) {
                        continue;
                    }
                    let mut unicodes: Vec<u32> = unicodes
                        .into_iter()
                        .filter(|codepoint| codepoints.insert(*codepoint))
                        .collect();
                    unicodes.truncate(3);
                    let id = GlyphId(Uuid::from_u128(
                        (u128::from(shuffle) << 8) | (n as u128 + 1),
                    ));
                    let layers = masters
                        .iter()
                        .zip(layers)
                        .filter_map(|(master, layer)| layer.map(|layer| (master.id, layer)))
                        .collect();
                    glyphs.insert(
                        id,
                        Arc::new(Glyph {
                            id,
                            name,
                            unicodes,
                            export,
                            layers,
                        }),
                    );
                    order.push(id);
                }
                // The order is a permutation of the glyphs, not just sorted.
                if !order.is_empty() {
                    let len = order.len();
                    let by = usize::try_from(shuffle % len as u64).unwrap_or(0);
                    order.rotate_left(by);
                    if shuffle & 1 == 1 {
                        order.reverse();
                    }
                }

                Font {
                    id: FontId(Uuid::from_u128(font_id)),
                    info: FontInfo {
                        family_name,
                        style_name,
                        version_major,
                        version_minor,
                        copyright,
                        designer,
                    },
                    units_per_em,
                    masters,
                    glyphs: Arc::new(glyphs),
                    glyph_order: order,
                    lib: lib.into_iter().collect(),
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    #[test]
    fn prop_roundtrip_both_formats(font in arb_font()) {
        prop_assert_eq!(font.validate_invariants(), Ok(()));
        let dir = TempDir::new().unwrap();

        let forma = dir.path().join("font.forma");
        save(&font, &forma).unwrap();
        prop_assert_eq!(&load(&forma).unwrap(), &font);

        let package = dir.path().join("font.formapackage");
        save(&font, &package).unwrap();
        prop_assert_eq!(&load(&package).unwrap(), &font);

        // Saving again over an existing package changes nothing.
        save(&font, &package).unwrap();
        prop_assert_eq!(&load(&package).unwrap(), &font);
    }

    #[test]
    fn prop_pack_unpack_roundtrip(font in arb_font()) {
        let dir = TempDir::new().unwrap();
        let package = dir.path().join("font.formapackage");
        save(&font, &package).unwrap();

        let forma = dir.path().join("font.forma");
        pack(&package, &forma).unwrap();
        prop_assert_eq!(&load(&forma).unwrap(), &font);

        let unpacked = dir.path().join("again.formapackage");
        unpack(&forma, &unpacked).unwrap();
        prop_assert_eq!(&load(&unpacked).unwrap(), &font);
    }
}
