//! Automatic class kerning with a collision guard (spec §6.2).

mod common;

use std::collections::{BTreeSet, HashMap};

use common::{add_svg_glyph, fixture_font, id, master, set_horizontal};
use forma_model::{
    Font, GlyphId, KernRef, KernSide, KerningGroup, KerningGroupId, MasterId, is_valid_glyph_name,
};
use forma_spacing::{
    KerningParams, KerningPlan, PlannedRef, SpacingError, SpacingParams, auto_kerning,
    auto_spacing, collisions,
};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

/// The H of the fixture (stem 80, sidebearings 80).
const H: &str = "M80 0 L160 0 L160 310 L540 310 L540 0 L620 0 L620 700 L540 700 L540 390 \
                 L160 390 L160 700 L80 700 Z";
/// A filled triangle standing on its base, 20 units from both sides.
const A_TRIANGLE: &str = "M20 0 L620 0 L320 700 Z";
/// The same triangle upside down.
const V_TRIANGLE: &str = "M20 700 L320 0 L620 700 Z";

/// A font with the reference `H` and the triangles `A` and `V`.
fn triangles() -> (Font, GlyphId, GlyphId, GlyphId) {
    let mut font = Font::new("Triangles", 1000);
    font.masters[0].metrics.cap_height = 700.0;
    let h = add_svg_glyph(&mut font, "H", 700.0, H);
    let a = add_svg_glyph(&mut font, "A", 640.0, A_TRIANGLE);
    let v = add_svg_glyph(&mut font, "V", 640.0, V_TRIANGLE);
    (font, h, a, v)
}

/// The glyphs of `font` that have ink, in glyph order.
fn inked(font: &Font) -> Vec<GlyphId> {
    font.glyph_order
        .iter()
        .copied()
        .filter(|g| {
            font.glyphs[g]
                .layers
                .values()
                .any(|l| !l.paths.is_empty() || !l.components.is_empty())
        })
        .collect()
}

fn all_pairs(glyphs: &[GlyphId]) -> Vec<(GlyphId, GlyphId)> {
    glyphs
        .iter()
        .flat_map(|a| glyphs.iter().map(move |b| (*a, *b)))
        .collect()
}

/// Writes `plan` into the kerning of `font` (which must have none), as the
/// editing commands will, and checks that the result is a valid model.
fn apply(font: &mut Font, master: MasterId, plan: &KerningPlan) {
    let mut ids = HashMap::new();
    for group in &plan.groups {
        let gid = KerningGroupId::new();
        font.kerning.groups_mut().insert(
            gid,
            KerningGroup {
                id: gid,
                name: group.name.clone(),
                side: group.side,
                members: group.members.clone(),
            },
        );
        ids.insert((group.side, group.name.clone()), gid);
    }
    let resolve = |side: KernSide, r: &PlannedRef| match r {
        PlannedRef::Glyph(g) => KernRef::Glyph(*g),
        PlannedRef::Group(name) => KernRef::Group(ids[&(side, name.clone())]),
    };
    for pair in &plan.pairs {
        let first = resolve(KernSide::First, &pair.first);
        let second = resolve(KernSide::Second, &pair.second);
        let previous = font
            .kerning
            .set_pair(master, first, second, Some(pair.value));
        assert!(previous.is_none(), "pair planned twice: {pair:?}");
    }
    font.validate_invariants()
        .expect("the plan is a valid model");
}

/// Checks the plan is directly applicable: valid, unique group names per
/// side, no glyph twice on a side, integer values within the model range.
fn check_plan(font: &Font, plan: &KerningPlan) {
    let upm = f64::from(font.units_per_em);
    let mut names = BTreeSet::new();
    let mut members = BTreeSet::new();
    for group in &plan.groups {
        assert!(is_valid_glyph_name(&group.name), "{}", group.name);
        assert!(names.insert((group.side, group.name.clone())), "{group:?}");
        assert!(group.members.len() >= 2, "{group:?}");
        assert_eq!(font.glyphs[&group.members[0]].name, group.name);
        for m in &group.members {
            assert!(members.insert((group.side, *m)), "{group:?}");
        }
    }
    for pair in &plan.pairs {
        assert!(pair.value != 0.0, "{pair:?}");
        assert_eq!(pair.value, pair.value.round(), "{pair:?}");
        assert!(pair.value.abs() <= 2.0 * upm, "{pair:?}");
    }
}

/// The fixture with a V (the fixture's A upside down, no counter).
fn fixture_with_v() -> Font {
    let mut font = fixture_font();
    add_svg_glyph(
        &mut font,
        "V",
        680.0,
        "M290 0 L390 0 L660 700 L560 700 L340 130 L120 700 L20 700 Z",
    );
    font
}

#[test]
fn default_params() {
    let p = KerningParams::default();
    assert_eq!(p.strength, 0.8);
    assert_eq!(p.max_ratio, 0.25);
    assert_eq!(p.min_value, 10.0);
    assert_eq!(p.min_distance_ratio, 0.02);
    assert_eq!(p.group_threshold_ratio, 0.03);
    assert_eq!(p.exception_threshold, 10.0);
    assert!(p.grouping);
    assert_eq!(p.lines, 64);
    // Same depth as the spacing: a multiple of the H stem.
    assert_eq!(p.depth_ratio, SpacingParams::default().depth_ratio);
    assert_eq!(p.edge_weight, SpacingParams::default().edge_weight);
}

#[test]
fn av_pair_gets_negative_kern() {
    let (mut font, h, a, v) = triangles();
    let m = master(&font);
    let plan = auto_kerning(&font, m, &[h, a, v], &KerningParams::default()).expect("plan");
    check_plan(&font, &plan);
    apply(&mut font, m, &plan);
    let av = font.kerning.effective(m, a, v);
    let va = font.kerning.effective(m, v, a);
    assert!(av < -10.0, "A/V {av}: {plan:?}");
    assert!(va < -10.0, "V/A {va}: {plan:?}");
    // The guard keeps the ink apart.
    let found = collisions(&font, m, &all_pairs(&[h, a, v]), 20.0).expect("check");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn hh_gets_no_kern() {
    let font = fixture_font();
    let m = master(&font);
    let h = id(&font, "H");
    let plan = auto_kerning(&font, m, &[h], &KerningParams::default()).expect("plan");
    assert!(plan.pairs.is_empty(), "{plan:?}");
    assert!(plan.groups.is_empty(), "{plan:?}");
    // In the whole fixture too.
    let mut font = fixture_font();
    let (m, h) = (master(&font), id(&font, "H"));
    let glyphs = inked(&font);
    let plan = auto_kerning(&font, m, &glyphs, &KerningParams::default()).expect("plan");
    apply(&mut font, m, &plan);
    assert_eq!(font.kerning.effective(m, h, h), 0.0, "{plan:?}");
}

#[test]
fn similar_glyphs_are_grouped() {
    let font = fixture_font();
    let m = master(&font);
    let glyphs = inked(&font);
    let plan = auto_kerning(&font, m, &glyphs, &KerningParams::default()).expect("plan");
    check_plan(&font, &plan);
    let (o, c) = (id(&font, "O"), id(&font, "C"));
    let group_of = |side: KernSide, g: GlyphId| {
        plan.groups
            .iter()
            .find(|group| group.side == side && group.members.contains(&g))
    };
    let left = group_of(KernSide::Second, o).expect("O is grouped on its left side");
    assert!(left.members.contains(&c), "{plan:?}");
    // Named after its first member in name order.
    assert_eq!(left.name, "C");
    // The right side of C is open: not like O.
    let right = group_of(KernSide::First, o);
    assert!(right.is_none_or(|g| !g.members.contains(&c)), "{plan:?}");
    // Lowercase and capitals do not mix.
    assert!(!left.members.contains(&id(&font, "o")), "{plan:?}");
}

#[test]
fn grouping_can_be_turned_off() {
    let font = fixture_with_v();
    let m = master(&font);
    let glyphs = inked(&font);
    let params = KerningParams {
        grouping: false,
        ..KerningParams::default()
    };
    let plan = auto_kerning(&font, m, &glyphs, &params).expect("plan");
    assert!(plan.groups.is_empty());
    assert!(
        plan.pairs.iter().all(|p| matches!(
            (&p.first, &p.second),
            (PlannedRef::Glyph(_), PlannedRef::Glyph(_))
        )),
        "{plan:?}"
    );
}

/// A font where three right-hand glyphs `n1`..`n3` share their left side,
/// except that `n3` has a dash level with the bar of `T`.
fn exception_font() -> (Font, GlyphId, [GlyphId; 3]) {
    let mut font = Font::new("Exceptions", 1000);
    font.masters[0].metrics.cap_height = 700.0;
    add_svg_glyph(&mut font, "H", 700.0, H);
    let t = add_svg_glyph(
        &mut font,
        "T",
        600.0,
        "M20 620 L580 620 L580 700 L20 700 Z M260 0 L340 0 L340 620 L260 620 Z",
    );
    let stem = "M40 0 L120 0 L120 500 L40 500 Z M320 0 L400 0 L400 500 L320 500 Z";
    let n1 = add_svg_glyph(&mut font, "n1", 440.0, stem);
    let n2 = add_svg_glyph(&mut font, "n2", 440.0, stem);
    let n3 = add_svg_glyph(
        &mut font,
        "n3",
        440.0,
        &format!("{stem} M10 610 L120 610 L120 650 L10 650 Z"),
    );
    (font, t, [n1, n2, n3])
}

#[test]
fn exceptions_when_member_differs() {
    let (mut font, t, [n1, n2, n3]) = exception_font();
    let m = master(&font);
    let glyphs = inked(&font);
    let plan = auto_kerning(&font, m, &glyphs, &KerningParams::default()).expect("plan");
    check_plan(&font, &plan);
    let group = plan
        .groups
        .iter()
        .find(|g| g.side == KernSide::Second && g.members.contains(&n1))
        .expect("n1 is grouped");
    assert_eq!(group.name, "n1");
    assert_eq!(group.members, vec![n1, n2, n3], "{plan:?}");
    // The class pair T/n1 carries the median, n3 gets its own exception.
    let class = plan
        .pairs
        .iter()
        .find(|p| p.first == PlannedRef::Glyph(t) && p.second == PlannedRef::Group("n1".into()))
        .expect("class pair");
    assert!(class.value < -10.0, "{plan:?}");
    let exception = plan
        .pairs
        .iter()
        .find(|p| p.first == PlannedRef::Glyph(t) && p.second == PlannedRef::Glyph(n3))
        .expect("exception for n3");
    assert!(
        (exception.value - class.value).abs() > 10.0,
        "{exception:?} vs {class:?}"
    );
    assert!(
        !plan
            .pairs
            .iter()
            .any(|p| p.second == PlannedRef::Glyph(n1) || p.second == PlannedRef::Glyph(n2)),
        "{plan:?}"
    );
    apply(&mut font, m, &plan);
    assert_eq!(font.kerning.effective(m, t, n1), class.value);
    assert_eq!(font.kerning.effective(m, t, n2), class.value);
    assert_eq!(font.kerning.effective(m, t, n3), exception.value);
    let found = collisions(&font, m, &all_pairs(&glyphs), 20.0).expect("check");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn kerning_is_deterministic() {
    let font = fixture_with_v();
    let m = master(&font);
    let glyphs = inked(&font);
    let params = KerningParams::default();
    let first = auto_kerning(&font, m, &glyphs, &params).expect("plan");
    let mut reversed = glyphs.clone();
    reversed.reverse();
    reversed.extend(glyphs.iter().take(3).copied());
    let second = auto_kerning(&font, m, &reversed, &params).expect("plan");
    assert_eq!(first, second);
    assert!(!first.pairs.is_empty());
}

#[test]
fn fixture_kerning_is_plausible_and_collision_free() {
    let mut font = fixture_with_v();
    let m = master(&font);
    let glyphs = inked(&font);
    // Space it first, as the demo flow does.
    let spaced = auto_spacing(&font, m, &glyphs, &SpacingParams::default()).expect("spacing");
    for sb in &spaced {
        let layer = &font.glyphs[&sb.glyph].layers[&m];
        let bbox = forma_geometry::bbox(&layer.paths).expect("ink");
        let advance = sb.lsb + bbox.width() + sb.rsb;
        set_horizontal(&mut font, sb.glyph, sb.lsb - bbox.x0, advance);
    }
    let plan = auto_kerning(&font, m, &glyphs, &KerningParams::default()).expect("plan");
    check_plan(&font, &plan);
    apply(&mut font, m, &plan);
    let k = |l: &str, r: &str| font.kerning.effective(m, id(&font, l), id(&font, r));
    assert!(k("A", "V") < -10.0, "{plan:?}");
    assert!(k("V", "A") < -10.0, "{plan:?}");
    // Pairs with a straight side were settled by the spacing.
    for other in ["H", "O", "B", "n", "o"] {
        assert!(k("H", other).abs() <= 20.0, "H{other} {}", k("H", other));
        assert!(k(other, "H").abs() <= 20.0, "{other}H {}", k(other, "H"));
    }
    assert!(k("O", "O").abs() <= 20.0, "OO {}", k("O", "O"));
    let found = collisions(&font, m, &all_pairs(&glyphs), 20.0).expect("check");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn errors() {
    let font = fixture_font();
    let m = master(&font);
    let h = id(&font, "H");
    let bad = [
        KerningParams {
            strength: f64::NAN,
            ..KerningParams::default()
        },
        KerningParams {
            strength: -1.0,
            ..KerningParams::default()
        },
        KerningParams {
            max_ratio: -0.1,
            ..KerningParams::default()
        },
        KerningParams {
            max_ratio: 2.5,
            ..KerningParams::default()
        },
        KerningParams {
            min_value: f64::INFINITY,
            ..KerningParams::default()
        },
        KerningParams {
            min_distance_ratio: f64::NAN,
            ..KerningParams::default()
        },
        KerningParams {
            group_threshold_ratio: -1.0,
            ..KerningParams::default()
        },
        KerningParams {
            exception_threshold: -1.0,
            ..KerningParams::default()
        },
        KerningParams {
            lines: 2,
            ..KerningParams::default()
        },
        KerningParams {
            depth_ratio: 0.0,
            ..KerningParams::default()
        },
        KerningParams {
            edge_weight: 2.0,
            ..KerningParams::default()
        },
    ];
    for params in bad {
        let err = auto_kerning(&font, m, &[h], &params).unwrap_err();
        assert!(matches!(err, SpacingError::InvalidParameter(_)), "{err}");
    }
    let err = auto_kerning(&font, MasterId::new(), &[h], &KerningParams::default()).unwrap_err();
    assert!(matches!(err, SpacingError::InvalidParameter(_)), "{err}");
    let err = auto_kerning(&font, m, &[GlyphId::new()], &KerningParams::default()).unwrap_err();
    assert!(matches!(err, SpacingError::InvalidParameter(_)), "{err}");
    let too_many = vec![h; forma_spacing::MAX_KERNING_GLYPHS + 1];
    // Duplicates count once: this is fine.
    auto_kerning(&font, m, &too_many, &KerningParams::default()).expect("one glyph");
    let mut font = fixture_font();
    common::remove_glyph(&mut font, "H");
    let (m, o) = (master(&font), id(&font, "O"));
    let err = auto_kerning(&font, m, &[o], &KerningParams::default()).unwrap_err();
    assert!(matches!(err, SpacingError::MissingReference(_)), "{err}");
}

#[test]
fn too_many_glyphs_is_an_error() {
    let mut font = fixture_font();
    let m = master(&font);
    let glyphs: Vec<GlyphId> = (0..=forma_spacing::MAX_KERNING_GLYPHS)
        .map(|i| {
            add_svg_glyph(
                &mut font,
                &format!("g{i}"),
                300.0,
                "M40 0 L200 0 L200 500 Z",
            )
        })
        .collect();
    let err = auto_kerning(&font, m, &glyphs, &KerningParams::default()).unwrap_err();
    assert!(matches!(err, SpacingError::InvalidParameter(_)), "{err}");
}

#[test]
fn eighty_nine_glyphs_are_kerned_quickly() {
    let mut font = fixture_with_v();
    let mut glyphs = inked(&font);
    let mut i = 0;
    while glyphs.len() < 89 {
        // Triangles, trapezoids and boxes of assorted sizes.
        let w = 200.0 + f64::from(i % 7) * 40.0;
        let top = 400.0 + f64::from(i % 5) * 80.0;
        let lean = f64::from(i % 3) * 60.0;
        let d = format!(
            "M30 0 L{r} 0 L{tr} {top} L{tl} {top} Z",
            r = 30.0 + w,
            tr = 30.0 + w - lean,
            tl = 30.0 + lean
        );
        glyphs.push(add_svg_glyph(&mut font, &format!("x{i}"), w + 60.0, &d));
        i += 1;
    }
    let m = master(&font);
    let start = std::time::Instant::now();
    let plan = auto_kerning(&font, m, &glyphs, &KerningParams::default()).expect("plan");
    let elapsed = start.elapsed();
    eprintln!(
        "89 glyphs: {elapsed:?}, {} groups, {} pairs",
        plan.groups.len(),
        plan.pairs.len()
    );
    assert!(elapsed.as_secs() < 20, "{elapsed:?}");
    check_plan(&font, &plan);
    apply(&mut font, m, &plan);
    let found = collisions(&font, m, &all_pairs(&glyphs), 20.0).expect("check");
    assert!(found.is_empty(), "{found:?}");
}

// ------------------------------------------------------- property tests

/// A fixed seed unless `FORMA_PROPTEST_RANDOM=1` (as in forma-geometry).
fn proptest_config(cases: u32) -> Config {
    let random = std::env::var_os("FORMA_PROPTEST_RANDOM").is_some_and(|value| value == "1");
    Config {
        cases,
        rng_seed: if random {
            RngSeed::Random
        } else {
            RngSeed::Fixed(0x46_4f_52_4d_41)
        },
        ..Config::default()
    }
}

/// A random polygon (possibly self-intersecting) with its ink inside its
/// advance: `(advance, points as fractions of the advance and of 800)`.
fn polygon() -> impl Strategy<Value = (f64, Vec<(f64, f64)>)> {
    (
        100.0..800.0f64,
        prop::collection::vec((0.0..=1.0f64, 0.0..=1.0f64), 3..9),
    )
}

fn svg(advance: f64, points: &[(f64, f64)]) -> String {
    let mut d = String::new();
    for (i, (x, y)) in points.iter().enumerate() {
        let cmd = if i == 0 { 'M' } else { 'L' };
        d.push_str(&format!("{cmd}{} {} ", x * advance, y * 800.0 - 100.0));
    }
    d.push('Z');
    d
}

proptest! {
    #![proptest_config(proptest_config(200))]

    #[test]
    fn kerning_never_creates_collisions(
        polygons in prop::collection::vec(polygon(), 2..4),
        strength in 0.2..3.0f64,
        grouping in any::<bool>(),
    ) {
        let mut font = Font::new("Random", 1000);
        font.masters[0].metrics.cap_height = 700.0;
        let mut glyphs = vec![add_svg_glyph(&mut font, "H", 700.0, H)];
        for (i, (advance, points)) in polygons.iter().enumerate() {
            glyphs.push(add_svg_glyph(&mut font, &format!("p{i}"), *advance, &svg(*advance, points)));
        }
        let m = master(&font);
        let params = KerningParams { strength, grouping, ..KerningParams::default() };
        let plan = auto_kerning(&font, m, &glyphs, &params).expect("plan");
        check_plan(&font, &plan);
        apply(&mut font, m, &plan);
        let found = collisions(&font, m, &all_pairs(&glyphs), 20.0).expect("check");
        prop_assert!(found.is_empty(), "{:?}\n{:?}", found, plan);
    }

    #[test]
    fn class_kerning_never_creates_collisions(
        families in prop::collection::vec(
            (polygon(), prop::collection::vec(prop::collection::vec(-15.0..15.0f64, 9), 2..4)),
            1..4,
        ),
        partners in prop::collection::vec(polygon(), 1..3),
        strength in 0.5..3.0f64,
        exception_threshold in 0.0..80.0f64,
    ) {
        // Families of near-identical glyphs form groups, and a large
        // exception threshold leaves the guard as the only reason for some
        // exceptions.
        let mut font = Font::new("Random", 1000);
        font.masters[0].metrics.cap_height = 700.0;
        let mut glyphs = vec![add_svg_glyph(&mut font, "H", 700.0, H)];
        for (f, ((advance, points), jitters)) in families.iter().enumerate() {
            for (j, jitter) in jitters.iter().enumerate() {
                let moved: Vec<(f64, f64)> = points
                    .iter()
                    .zip(jitter)
                    .map(|(&(x, y), dx)| (((x * advance + dx) / advance).clamp(0.0, 1.0), y))
                    .collect();
                let name = format!("f{f}m{j}");
                glyphs.push(add_svg_glyph(&mut font, &name, *advance, &svg(*advance, &moved)));
            }
        }
        for (i, (advance, points)) in partners.iter().enumerate() {
            glyphs.push(add_svg_glyph(&mut font, &format!("p{i}"), *advance, &svg(*advance, points)));
        }
        let m = master(&font);
        let params = KerningParams { strength, exception_threshold, ..KerningParams::default() };
        let plan = auto_kerning(&font, m, &glyphs, &params).expect("plan");
        check_plan(&font, &plan);
        apply(&mut font, m, &plan);
        let found = collisions(&font, m, &all_pairs(&glyphs), 20.0).expect("check");
        prop_assert!(found.is_empty(), "{:?}\n{:?}", found, plan);
    }
}
