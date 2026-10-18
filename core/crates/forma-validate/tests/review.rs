//! Review-state checks of generated glyphs, and rejected glyphs left out of
//! the exported font.

mod common;

use common::{add_glyph, clean_font, edit_glyph, id_of};
use forma_compile::{OutputFormat, compile};
use forma_model::{Component, Font, Provenance, ProvenanceDetail, Review, Source, Transform};
use forma_validate::validate;

fn provenance(source: Source, review: Review) -> Provenance {
    let mut p = Provenance::pending(source, 1, ProvenanceDetail::None);
    p.review = review;
    p
}

fn set(font: &mut Font, name: &str, p: Provenance) {
    let id = id_of(font, name);
    edit_glyph(font, id, |glyph| glyph.provenance = Some(p));
}

fn checks(font: &Font) -> Vec<(String, Option<String>)> {
    validate(font)
        .findings
        .into_iter()
        .filter(|f| f.check.starts_with("glyph.") && f.check != "glyph.mixed_contours_components")
        .map(|f| (f.check, f.glyph))
        .collect()
}

#[test]
fn pending_generated_glyphs_are_warned_hand_drawn_and_composed_are_not() {
    let mut font = clean_font();
    set(
        &mut font,
        "H",
        provenance(Source::Synthesized, Review::Pending),
    );
    set(
        &mut font,
        "I",
        provenance(Source::Composed, Review::Pending),
    );
    assert_eq!(
        checks(&font),
        vec![("glyph.pending_review".to_owned(), Some("H".to_owned()))]
    );
}

#[test]
fn approval_stale_when_design_changed_outside_the_editor() {
    let mut font = clean_font();
    let id = id_of(&font, "H");
    let mut p = provenance(Source::Traced, Review::Approved);
    p.approved_geometry = Some(font.glyphs[&id].geometry_hash());
    set(&mut font, "H", p);
    assert!(checks(&font).is_empty());
    edit_glyph(&mut font, id, |glyph| {
        if let Some(layer) = glyph.layers.values_mut().next() {
            layer.paths[0].nodes[1].y += 20.0;
        }
    });
    assert_eq!(
        checks(&font),
        vec![("glyph.approval_stale".to_owned(), Some("H".to_owned()))]
    );
}

#[test]
fn rejected_glyph_is_not_exported_and_using_it_is_an_error() {
    let mut font = clean_font();
    set(
        &mut font,
        "I",
        provenance(Source::Synthesized, Review::Rejected),
    );
    let base = id_of(&font, "I");
    let user = add_glyph(&mut font, "J", &['J' as u32], 300.0, vec![]);
    edit_glyph(&mut font, user, |glyph| {
        if let Some(layer) = glyph.layers.values_mut().next() {
            layer.components.push(Component {
                base,
                transform: Transform {
                    xx: 1.0,
                    xy: 0.0,
                    yx: 0.0,
                    yy: 1.0,
                    dx: 0.0,
                    dy: 0.0,
                },
            });
        }
    });
    let found = checks(&font);
    assert!(
        found.contains(&(
            "glyph.rejected_used_by_component".to_owned(),
            Some("J".to_owned())
        )),
        "{found:?}"
    );
    assert!(!font.glyphs[&base].is_exported());
    let (ttf, _) = compile(&font, OutputFormat::Ttf).expect("compile");
    let names = forma_validate_names(&ttf);
    assert!(!names.contains(&"I".to_owned()), "{names:?}");
}

/// The glyph names in a compiled font's `post` table.
fn forma_validate_names(ttf: &[u8]) -> Vec<String> {
    use read_fonts::{FontRef, TableProvider};
    let font = FontRef::new(ttf).expect("font");
    let post = font.post().expect("post");
    let count = font.maxp().map(|m| m.num_glyphs()).unwrap_or(0);
    (0..count)
        .filter_map(|gid| post.glyph_name(read_fonts::types::GlyphId16::new(gid)))
        .map(|name| name.to_string())
        .collect()
}
