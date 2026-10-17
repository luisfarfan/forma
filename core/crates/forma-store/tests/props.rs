//! Property tests: any valid font survives a save/load round trip in both
//! formats, and converting between them, bit for bit.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use forma_model::{
    Component, Font, FontId, FontInfo, Glyph, GlyphId, KernRef, KernSide, Kerning, KerningGroup,
    KerningGroupId, Layer, Master, MasterId, Node, NodeKind, Path as Contour, PathId, Transform,
    VerticalMetrics,
};
use forma_store::{load, pack, save, unpack};
use proptest::collection::vec;
use proptest::prelude::*;
use serde_json::{Map, Number, Value};
use tempfile::TempDir;
use uuid::Uuid;

/// Roadmap criterion 7: 1 000 cases per property.
const CASES: u32 = 1000;

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
        components: Vec::new(),
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

/// A transform that the model accepts for a component.
fn transform() -> impl Strategy<Value = Transform> {
    (
        prop_oneof![0.25f64..4.0, -4.0f64..-0.25],
        prop_oneof![0.25f64..4.0, -4.0f64..-0.25],
        -0.2f64..0.2,
        -0.2f64..0.2,
        finite(),
        finite(),
    )
        .prop_map(|(xx, yy, xy, yx, dx, dy)| Transform {
            xx,
            xy,
            yx,
            yy,
            dx,
            dy,
        })
        .prop_map(|t| if t.is_valid() { t } else { Transform::IDENTITY })
}

/// Raw material for components: (glyph, base, transform), as indexes that the
/// font builder reduces to valid choices.
type ComponentSeed = (usize, usize, Transform);

/// Raw material for kerning: groups as (side, name, member indexes), and per
/// master the pairs as (first, second, fraction of twice the em).
type KerningSeed = (
    Vec<(bool, String, Vec<usize>)>,
    Vec<Vec<(usize, usize, f64)>>,
);

fn kerning_seed() -> impl Strategy<Value = KerningSeed> {
    (
        vec(
            (
                any::<bool>(),
                "[A-Za-z_][A-Za-z0-9_.-]{0,12}",
                vec(any::<usize>(), 0..4),
            ),
            0..5,
        ),
        vec(
            vec((any::<usize>(), any::<usize>(), -1.0f64..=1.0), 0..8),
            2,
        ),
    )
}

/// Adds components (each only to a glyph with a higher position than its
/// base, so there are no cycles, and only in masters where both glyphs have a
/// layer) to the glyphs, which are in creation order.
fn add_components(glyphs: &mut [Glyph], seeds: Vec<ComponentSeed>) {
    for (glyph_index, base_index, transform) in seeds {
        if glyphs.len() < 2 {
            return;
        }
        let i = 1 + glyph_index % (glyphs.len() - 1);
        let j = base_index % i;
        let base = glyphs[j].id;
        let base_masters: Vec<MasterId> = glyphs[j].layers.keys().copied().collect();
        for (master, layer) in &mut glyphs[i].layers {
            if base_masters.contains(master) {
                layer.components.push(Component { base, transform });
            }
        }
    }
}

/// Builds valid kerning from the seed: groups with unique names per side and
/// unique members per side, and pairs between existing glyphs and groups of
/// the right sides, with non-zero values within twice the em.
fn build_kerning(
    seed: KerningSeed,
    glyph_ids: &[GlyphId],
    masters: &[Master],
    units_per_em: u16,
    salt: u128,
) -> Kerning {
    let (group_seeds, pair_seeds) = seed;
    let mut kerning = Kerning::default();
    let mut names: HashSet<(KernSide, String)> = HashSet::new();
    let mut used: HashSet<(KernSide, GlyphId)> = HashSet::new();
    let mut by_side: [Vec<KernRef>; 2] = [
        glyph_ids.iter().map(|&id| KernRef::Glyph(id)).collect(),
        glyph_ids.iter().map(|&id| KernRef::Glyph(id)).collect(),
    ];
    for (n, (second, name, members)) in group_seeds.into_iter().enumerate() {
        let side = if second {
            KernSide::Second
        } else {
            KernSide::First
        };
        if glyph_ids.is_empty() || !names.insert((side, name.clone())) {
            continue;
        }
        let mut list = Vec::new();
        for member in members {
            let id = glyph_ids[member % glyph_ids.len()];
            if used.insert((side, id)) {
                list.push(id);
            }
        }
        if list.is_empty() {
            continue;
        }
        let id = KerningGroupId(Uuid::from_u128(salt.rotate_left(7) ^ (0x1000 + n as u128)));
        kerning.groups_mut().insert(
            id,
            KerningGroup {
                id,
                name,
                side,
                members: list,
            },
        );
        by_side[usize::from(second)].push(KernRef::Group(id));
    }
    let limit = 2.0 * f64::from(units_per_em);
    for (master, pairs) in masters.iter().zip(pair_seeds) {
        for (a, b, fraction) in pairs {
            if by_side[0].is_empty() {
                continue;
            }
            let value = fraction * limit;
            if value == 0.0 || !value.is_finite() || value.abs() > limit {
                continue;
            }
            let first = by_side[0][a % by_side[0].len()];
            let second = by_side[1][b % by_side[1].len()];
            kerning.set_pair(master.id, first, second, Some(value));
        }
    }
    kerning
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
        vec((any::<usize>(), any::<usize>(), transform()), 0..8),
        kerning_seed(),
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
                component_seeds,
                kerning_seed,
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
                let mut created: Vec<Glyph> = Vec::new();
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
                    created.push(Glyph {
                        id,
                        name,
                        unicodes,
                        export,
                        layers,
                    });
                    order.push(id);
                }
                add_components(&mut created, component_seeds);
                for glyph in created {
                    glyphs.insert(glyph.id, Arc::new(glyph));
                }
                let kerning =
                    build_kerning(kerning_seed, &order, &masters, units_per_em, master_salt);
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
                    kerning,
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

/// The generator must actually produce components and kerning, or the
/// round-trip properties above would prove nothing about them.
#[test]
fn generator_covers_components_and_kerning() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let (mut components, mut groups, mut pairs) = (0, 0, 0);
    for _ in 0..300 {
        let font = arb_font().new_tree(&mut runner).unwrap().current();
        assert_eq!(font.validate_invariants(), Ok(()));
        components += font
            .glyphs
            .values()
            .flat_map(|g| g.layers.values())
            .map(|l| l.components.len())
            .sum::<usize>();
        groups += font.kerning.groups.len();
        pairs += font.kerning.pairs.values().map(|p| p.len()).sum::<usize>();
    }
    assert!(components > 50, "{components}");
    assert!(groups > 50, "{groups}");
    assert!(pairs > 50, "{pairs}");
}
