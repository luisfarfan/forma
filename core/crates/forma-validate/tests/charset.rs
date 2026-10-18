//! Character-set profiles and the coverage report.

mod common;

use common::{add_glyph, clean_rect, default_layer, id_of};
use forma_model::{Component, Font, Transform};
use forma_validate::charset::{check_charset, profile};

#[test]
fn charset_profile_has_119_codepoints() {
    let codes = profile("unicase-es").expect("the profile exists");
    assert_eq!(codes.len(), 119);
    let mut sorted = codes.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 119, "no duplicates");
    for code in [
        0x20, 0xA0, 0x41, 0x7A, 0xD1, 0xFC, 0x30, 0x39, 0x20AC, 0x2026, 0xB7,
    ] {
        assert!(codes.contains(&code), "U+{code:04X}");
    }
    assert!(!codes.contains(&0x7E), "~ is not in the profile");
    assert!(profile("nope").is_none());
}

#[test]
fn charset_reports_missing_and_composites() {
    let mut font = Font::new("Charset", 1000);
    let a = add_glyph(
        &mut font,
        "A",
        &[0x41, 0x61],
        600.0,
        vec![clean_rect(0, 0, 0, 500, 700)],
    );
    let mark = add_glyph(
        &mut font,
        "acutecomb",
        &[],
        0.0,
        vec![clean_rect(0, 0, 0, 50, 50)],
    );
    let aacute = add_glyph(&mut font, "Aacute", &[0xC1, 0xE1], 600.0, Vec::new());
    default_layer(&mut font, aacute, |layer| {
        layer.components = vec![
            Component {
                base: a,
                transform: Transform::default(),
            },
            Component {
                base: mark,
                transform: Transform::default(),
            },
        ];
    });
    add_glyph(&mut font, "space", &[0x20], 250.0, Vec::new());
    // Not exported: does not count.
    let hidden = add_glyph(&mut font, "B", &[0x42], 600.0, Vec::new());
    common::edit_glyph(&mut font, hidden, |glyph| glyph.export = false);
    assert_eq!(id_of(&font, "A"), a);

    let required = [0x20, 0x41, 0x42, 0x61, 0xC1, 0xE1, 0xF1, 0x41];
    let report = check_charset(&font, &required);
    assert_eq!(report.required, 7, "duplicates count once");
    assert_eq!(report.covered, 5);
    assert_eq!(report.missing, vec![0x42, 0xF1]);
    assert_eq!(report.composite, vec!["Aacute".to_owned()]);
    assert_eq!(report.outlined, vec!["A".to_owned()]);

    let full = check_charset(&font, &[0x41]);
    assert_eq!((full.required, full.covered), (1, 1));
    assert!(full.missing.is_empty());
}

#[test]
fn glyph_names_follow_the_agl() {
    use forma_validate::charset::glyph_name;
    assert_eq!(glyph_name(u32::from('A')), "A");
    assert_eq!(glyph_name(0x30), "zero");
    assert_eq!(glyph_name(0x0301), "acutecomb");
    assert_eq!(glyph_name(0x0416), "uni0416");
    assert_eq!(glyph_name(0x1F600), "u1F600");
}
