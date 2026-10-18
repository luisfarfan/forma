//! Profiles and automatic spacing (spec §6.2).

mod common;

use common::{add_glyph, add_svg_glyph, fixture_font, id, master, remove_glyph, set_horizontal};
use forma_geometry::layer_bbox;
use forma_model::{Component, Transform};
use forma_spacing::{SpacingError, SpacingParams, Zone, auto_spacing, measurement_zone, profile};

const INKED: [&str; 11] = ["A", "B", "C", "H", "O", "a", "b", "c", "n", "o", ".notdef"];

fn ids(font: &forma_model::Font, names: &[&str]) -> Vec<forma_model::GlyphId> {
    names.iter().map(|name| id(font, name)).collect()
}

#[test]
fn default_params_match_the_spec() {
    let params = SpacingParams::default();
    assert_eq!(params.tightness, 0.6);
    assert_eq!(params.references, ["H".to_owned(), "O".to_owned()]);
    assert_eq!(params.depth_ratio, 0.4);
    assert_eq!(params.lines, 64);
    assert_eq!(params.edge_weight, 0.25);
}

#[test]
fn measurement_zone_spans_cap_height_with_overshoot() {
    let font = fixture_font();
    let zone = measurement_zone(&font, master(&font));
    assert_eq!(
        zone,
        Zone {
            bottom: -20.0,
            top: 720.0
        }
    );
}

#[test]
fn profile_follows_straight_and_curved_outlines() {
    let font = fixture_font();
    let zone = measurement_zone(&font, master(&font));
    let h = profile(&font, id(&font, "H"), master(&font), zone, 64).expect("profile");
    assert_eq!(h.ys.len(), 64);
    assert_eq!(h.left.len(), 64);
    assert_eq!(h.right.len(), 64);
    for (i, y) in h.ys.iter().enumerate() {
        if (0.0..700.0).contains(y) {
            assert_eq!(h.left[i], Some(80.0), "H left at {y}");
            assert_eq!(h.right[i], Some(620.0), "H right at {y}");
        } else {
            assert_eq!(h.left[i], None, "H has no ink at {y}");
            assert_eq!(h.right[i], None, "H has no ink at {y}");
        }
    }
    // O: the line nearest to 350 meets the bowl at its extremes (60 and 700).
    let o = profile(&font, id(&font, "O"), master(&font), zone, 64).expect("profile");
    let mid =
        o.ys.iter()
            .enumerate()
            .min_by(|a, b| (a.1 - 350.0).abs().total_cmp(&(b.1 - 350.0).abs()))
            .map(|(i, _)| i)
            .expect("lines");
    let left = o.left[mid].expect("ink");
    let right = o.right[mid].expect("ink");
    assert!((left - 60.0).abs() < 0.5, "O left {left}");
    assert!((right - 700.0).abs() < 0.5, "O right {right}");
    // The counter does not count: the extremes are the outer bowl.
    assert!((left + right - 760.0).abs() < 1e-6, "O is symmetric");
}

#[test]
fn profile_of_short_glyph_uses_its_own_ink_height() {
    let mut font = fixture_font();
    let period = add_svg_glyph(&mut font, "period", 200.0, "M60 0 L140 0 L140 80 L60 80 Z");
    let zone = measurement_zone(&font, master(&font));
    let p = profile(&font, period, master(&font), zone, 64).expect("profile");
    assert_eq!(p.ys.len(), 64);
    assert!(p.ys.iter().all(|y| (0.0..=80.0).contains(y)), "{:?}", p.ys);
    assert!(p.left.iter().all(|x| *x == Some(60.0)));
    assert!(p.right.iter().all(|x| *x == Some(140.0)));
}

#[test]
fn profile_rejects_bad_lines() {
    let font = fixture_font();
    let zone = measurement_zone(&font, master(&font));
    let err = profile(&font, id(&font, "H"), master(&font), zone, 0).unwrap_err();
    assert!(matches!(err, SpacingError::InvalidParameter(_)), "{err}");
}

#[test]
fn symmetric_glyph_gets_symmetric_sidebearings() {
    let font = fixture_font();
    let glyphs = ids(&font, &["H", "O", "A"]);
    let result =
        auto_spacing(&font, master(&font), &glyphs, &SpacingParams::default()).expect("spacing");
    assert_eq!(result.len(), 3);
    for (sb, name) in result.iter().zip(["H", "O", "A"]) {
        assert_eq!(sb.glyph, id(&font, name));
        assert!(
            (sb.lsb - sb.rsb).abs() <= 1.0,
            "{name}: lsb {} rsb {}",
            sb.lsb,
            sb.rsb
        );
        assert_eq!(sb.lsb, sb.lsb.round(), "integer sidebearings");
        assert_eq!(sb.rsb, sb.rsb.round(), "integer sidebearings");
    }
}

#[test]
fn h_margin_is_tightness_times_stem() {
    // The stem of H is 80 units: 0.6 × 80 = 48.
    let font = fixture_font();
    let result = auto_spacing(
        &font,
        master(&font),
        &ids(&font, &["H"]),
        &SpacingParams::default(),
    )
    .expect("spacing");
    assert_eq!((result[0].lsb, result[0].rsb), (48.0, 48.0));
}

#[test]
fn spacing_is_idempotent() {
    let mut font = fixture_font();
    let glyphs = ids(&font, &INKED);
    let params = SpacingParams::default();
    let first = auto_spacing(&font, master(&font), &glyphs, &params).expect("spacing");
    assert_eq!(first.len(), INKED.len());
    for sb in &first {
        let bbox = layer_bbox(&font, sb.glyph, master(&font))
            .expect("bbox")
            .expect("ink");
        let advance = sb.lsb + bbox.width() + sb.rsb;
        set_horizontal(&mut font, sb.glyph, sb.lsb - bbox.x0, advance);
    }
    let second = auto_spacing(&font, master(&font), &glyphs, &params).expect("spacing");
    for (a, b) in first.iter().zip(&second) {
        assert_eq!(a.glyph, b.glyph);
        assert!((a.lsb - b.lsb).abs() <= 1.0, "{a:?} -> {b:?}");
        assert!((a.rsb - b.rsb).abs() <= 1.0, "{a:?} -> {b:?}");
    }
}

#[test]
fn spacing_is_deterministic() {
    let font = fixture_font();
    let glyphs = ids(&font, &INKED);
    let params = SpacingParams::default();
    let a = auto_spacing(&font, master(&font), &glyphs, &params).expect("spacing");
    let b = auto_spacing(&font, master(&font), &glyphs, &params).expect("spacing");
    assert_eq!(a, b);
}

#[test]
fn composites_are_skipped() {
    let mut font = fixture_font();
    let a = id(&font, "A");
    let shifted = Transform {
        dx: 30.0,
        ..Transform::IDENTITY
    };
    let composite = add_glyph(
        &mut font,
        "Acomposite",
        680.0,
        Vec::new(),
        vec![Component {
            base: a,
            transform: shifted,
        }],
    );
    let space = id(&font, "space");
    let result = auto_spacing(
        &font,
        master(&font),
        &[a, composite, space],
        &SpacingParams::default(),
    )
    .expect("spacing");
    assert_eq!(result.len(), 1, "{result:?}");
    assert_eq!(result[0].glyph, a);
}

#[test]
fn tightness_changes_spacing() {
    let font = fixture_font();
    let h = ids(&font, &["H"]);
    let loose = auto_spacing(&font, master(&font), &h, &SpacingParams::default()).expect("0.6");
    let tight = auto_spacing(
        &font,
        master(&font),
        &h,
        &SpacingParams {
            tightness: 0.3,
            ..SpacingParams::default()
        },
    )
    .expect("0.3");
    assert!(tight[0].lsb < loose[0].lsb, "{tight:?} vs {loose:?}");
    assert!(tight[0].rsb < loose[0].rsb, "{tight:?} vs {loose:?}");
    assert_eq!((tight[0].lsb, tight[0].rsb), (24.0, 24.0));
}

#[test]
fn missing_reference_is_an_error() {
    let mut font = fixture_font();
    let h = ids(&font, &["H"]);
    let params = SpacingParams {
        references: ["H".to_owned(), "Q".to_owned()],
        ..SpacingParams::default()
    };
    let err = auto_spacing(&font, master(&font), &h, &params).unwrap_err();
    assert!(matches!(err, SpacingError::MissingReference(_)), "{err}");

    remove_glyph(&mut font, "O");
    let err = auto_spacing(&font, master(&font), &h, &SpacingParams::default()).unwrap_err();
    assert!(matches!(err, SpacingError::MissingReference(_)), "{err}");
}

#[test]
fn invalid_params_are_rejected() {
    let font = fixture_font();
    let h = ids(&font, &["H"]);
    for params in [
        SpacingParams {
            tightness: f64::NAN,
            ..SpacingParams::default()
        },
        SpacingParams {
            tightness: -1.0,
            ..SpacingParams::default()
        },
        SpacingParams {
            depth_ratio: 0.0,
            ..SpacingParams::default()
        },
        SpacingParams {
            lines: 2,
            ..SpacingParams::default()
        },
        SpacingParams {
            depth_ratio: f64::INFINITY,
            ..SpacingParams::default()
        },
        SpacingParams {
            edge_weight: 1.5,
            ..SpacingParams::default()
        },
    ] {
        let err = auto_spacing(&font, master(&font), &h, &params).unwrap_err();
        assert!(matches!(err, SpacingError::InvalidParameter(_)), "{err}");
    }
}

/// The default spacing of `names`, by name.
fn spaced(
    font: &forma_model::Font,
    names: &[&str],
    params: &SpacingParams,
) -> std::collections::BTreeMap<String, (f64, f64)> {
    let result = auto_spacing(font, master(font), &ids(font, names), params).expect("spacing");
    assert_eq!(result.len(), names.len());
    names
        .iter()
        .zip(result)
        .map(|(name, sb)| ((*name).to_owned(), (sb.lsb, sb.rsb)))
        .collect()
}

const OUTCOME: [&str; 5] = ["H", "O", "A", "B", "C"];

#[test]
fn round_letters_get_a_fraction_of_the_h_margin() {
    let font = fixture_font();
    let sb = spaced(&font, &OUTCOME, &SpacingParams::default());
    let h = sb["H"].0;
    assert!((sb["H"].0 - sb["H"].1).abs() <= 1.0, "{sb:?}");
    let (l, r) = sb["O"];
    assert!((l - r).abs() <= 1.0, "{sb:?}");
    for side in [l, r] {
        assert!(
            0.5 * h <= side && side <= 0.9 * h,
            "O {side} vs H {h}: {sb:?}"
        );
    }
}

#[test]
fn diagonal_letters_get_a_small_non_negative_margin() {
    let font = fixture_font();
    let sb = spaced(&font, &OUTCOME, &SpacingParams::default());
    let h = sb["H"].0;
    let (l, r) = sb["A"];
    assert!((l - r).abs() <= 1.0, "{sb:?}");
    for side in [l, r] {
        assert!((0.0..=0.6 * h).contains(&side), "A {side} vs H {h}: {sb:?}");
    }
}

#[test]
fn bowls_and_open_letters_get_non_negative_sidebearings() {
    let font = fixture_font();
    let sb = spaced(&font, &OUTCOME, &SpacingParams::default());
    for name in ["B", "C"] {
        let (l, r) = sb[name];
        assert!(l >= 0.0 && r >= 0.0, "{name}: {sb:?}");
    }
}

#[test]
fn tightness_scales_every_glyph_monotonically() {
    let font = fixture_font();
    let loose = spaced(&font, &OUTCOME, &SpacingParams::default());
    let tight = spaced(
        &font,
        &OUTCOME,
        &SpacingParams {
            tightness: 0.3,
            ..SpacingParams::default()
        },
    );
    for name in OUTCOME {
        let (a, b) = (tight[name], loose[name]);
        assert!(a.0 <= b.0 && a.1 <= b.1, "{name}: {a:?} vs {b:?}");
    }
    assert!(tight["H"].0 < loose["H"].0);
    assert!(tight["O"].0 < loose["O"].0);
}

#[test]
fn negative_margins_are_clamped_to_zero() {
    // With no margin on H, the white of A and O is already more than the
    // target: their margins would be negative, and are clamped to 0.
    let font = fixture_font();
    let sb = spaced(
        &font,
        &OUTCOME,
        &SpacingParams {
            tightness: 0.0,
            ..SpacingParams::default()
        },
    );
    assert_eq!(sb["H"], (0.0, 0.0));
    assert_eq!(sb["A"], (0.0, 0.0));
    assert_eq!(sb["O"], (0.0, 0.0));
    assert!(sb.values().all(|&(l, r)| l >= 0.0 && r >= 0.0), "{sb:?}");
}
