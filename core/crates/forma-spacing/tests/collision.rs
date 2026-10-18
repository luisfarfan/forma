//! Collision checks between kerned glyph pairs (spec §6.2).

mod common;

use common::{add_svg_glyph, fixture_font, id, master};
use forma_model::KernRef;
use forma_spacing::{MAX_CHECK_PAIRS, SpacingError, collisions};

/// A V drawn as the A of the fixture turned upside down (no counter).
const V: &str = "M290 0 L390 0 L660 700 L560 700 L340 130 L120 700 L20 700 Z";

#[test]
fn collision_detects_overlap() {
    let mut font = fixture_font();
    let v = add_svg_glyph(&mut font, "V", 680.0, V);
    let a = id(&font, "A");
    let m = master(&font);
    font.kerning
        .set_pair(m, KernRef::Glyph(a), KernRef::Glyph(v), Some(-400.0));
    let found = collisions(&font, m, &[(a, v)], 0.0).expect("check");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].left, found[0].right), (a, v));
    assert!(found[0].distance < 0.0, "{found:?}");
    // At the baseline the A ends at 660 and the V starts at 680 − 400 + 290.
    assert!(
        (found[0].distance - (570.0 - 660.0)).abs() < 2.0,
        "{found:?}"
    );
}

#[test]
fn no_collision_for_spaced_pair() {
    let mut font = fixture_font();
    let v = add_svg_glyph(&mut font, "V", 680.0, V);
    let a = id(&font, "A");
    let h = id(&font, "H");
    let found = collisions(&font, master(&font), &[(a, v), (h, h), (v, a)], 0.0).expect("check");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn min_distance_and_order() {
    let font = fixture_font();
    let h = id(&font, "H");
    let o = id(&font, "O");
    // H|H: 700 − 620 + 80 = 160. H|O: 700 − 620 + 60 = 140 at the widest
    // line of the bowl (sampled, so a little more).
    let found = collisions(&font, master(&font), &[(h, h), (h, o)], 1000.0).expect("check");
    assert_eq!(found.len(), 2);
    assert_eq!((found[0].left, found[0].right), (h, o));
    assert!((found[0].distance - 140.0).abs() < 1.0, "{found:?}");
    assert_eq!((found[1].left, found[1].right), (h, h));
    assert!((found[1].distance - 160.0).abs() < 1e-9, "{found:?}");
}

#[test]
fn inkless_glyphs_never_collide() {
    let font = fixture_font();
    let space = id(&font, "space");
    let h = id(&font, "H");
    let found = collisions(&font, master(&font), &[(space, h), (h, space)], 1000.0).expect("check");
    assert!(found.is_empty());
}

#[test]
fn too_many_pairs_is_an_error() {
    let font = fixture_font();
    let h = id(&font, "H");
    let pairs = vec![(h, h); MAX_CHECK_PAIRS + 1];
    let err = collisions(&font, master(&font), &pairs, 0.0).unwrap_err();
    assert!(matches!(err, SpacingError::InvalidParameter(_)), "{err}");
    let err = collisions(&font, master(&font), &[(h, h)], f64::NAN).unwrap_err();
    assert!(matches!(err, SpacingError::InvalidParameter(_)), "{err}");
}
