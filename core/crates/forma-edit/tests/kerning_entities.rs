//! Components and kerning as seen by diffs and documents. The kerning
//! commands arrive later; here the kerning is set directly on the fonts.

mod common;

use std::sync::Arc;

use common::square;
use forma_edit::{Command, Document, GlyphDiff, GlyphRef, diff};
use forma_model::{
    Actor, Component, EntityKey, Font, Glyph, GlyphId, KernRef, KernSide, KerningGroup,
    KerningGroupId, Layer, Rev, Transform,
};

fn uuid(n: u128) -> String {
    format!("00000000-0000-0000-0000-{n:012x}")
}

fn gid(n: u128) -> GlyphId {
    uuid(n).parse().expect("a glyph id")
}

fn kid(n: u128) -> KerningGroupId {
    uuid(n).parse().expect("a group id")
}

fn layer(components: Vec<Component>) -> Layer {
    Layer {
        advance_width: 600.0,
        paths: vec![square(0, 0.0, 0.0, 500.0, 500.0, true)],
        components,
    }
}

/// A font with `A` (1), `V` (2) and `Aacute` (3), a composite of `A`, plus
/// the first-side group `O` with `A` and the pair `A`/`V` of -50.
fn font() -> Font {
    let mut font = Font::new("Kern", 1000);
    let master = font.masters[0].id;
    let glyphs = Arc::make_mut(&mut font.glyphs);
    for (n, name, components) in [
        (1, "A", Vec::new()),
        (2, "V", Vec::new()),
        (
            3,
            "Aacute",
            vec![Component {
                base: gid(1),
                transform: Transform::IDENTITY,
            }],
        ),
    ] {
        glyphs.insert(
            gid(n),
            Arc::new(Glyph {
                id: gid(n),
                name: name.to_owned(),
                unicodes: Vec::new(),
                export: true,
                layers: [(master, layer(components))].into(),
                provenance: None,
            }),
        );
        font.glyph_order.push(gid(n));
    }
    font.kerning.groups_mut().insert(
        kid(1),
        KerningGroup {
            id: kid(1),
            name: "O".to_owned(),
            side: KernSide::First,
            members: vec![gid(1)],
        },
    );
    font.kerning.set_pair(
        master,
        KernRef::Glyph(gid(1)),
        KernRef::Glyph(gid(2)),
        Some(-50.0),
    );
    font.validate_invariants().expect("a valid font");
    font
}

fn glyph_mut(font: &mut Font, id: GlyphId) -> &mut Glyph {
    Arc::make_mut(Arc::make_mut(&mut font.glyphs).get_mut(&id).expect("glyph"))
}

#[test]
fn diff_flags_kerning_and_components() {
    let before = font();
    let master = before.masters[0].id;

    let same = diff(&before, &before.clone());
    assert!(!same.kerning_changed);
    assert!(same.modified.is_empty());

    // A pair value.
    let mut after = before.clone();
    after.kerning.set_pair(
        master,
        KernRef::Glyph(gid(1)),
        KernRef::Glyph(gid(2)),
        Some(-40.0),
    );
    let report = diff(&before, &after);
    assert!(report.kerning_changed);
    assert!(report.modified.is_empty());
    assert!(!report.info_changed && !report.metrics_changed);

    // A new pair, and the members of a group.
    let mut after = before.clone();
    after.kerning.set_pair(
        master,
        KernRef::Group(kid(1)),
        KernRef::Glyph(gid(2)),
        Some(-30.0),
    );
    assert!(diff(&before, &after).kerning_changed);
    let mut after = before.clone();
    after
        .kerning
        .groups_mut()
        .get_mut(&kid(1))
        .expect("group")
        .members
        .push(gid(3));
    assert!(diff(&before, &after).kerning_changed);

    // The transform of a component.
    let mut after = before.clone();
    glyph_mut(&mut after, gid(3))
        .layers
        .get_mut(&master)
        .expect("layer")
        .components[0]
        .transform
        .dy = 120.0;
    let report = diff(&before, &after);
    assert!(!report.kerning_changed);
    assert_eq!(
        report.modified,
        vec![GlyphDiff {
            name: "Aacute".to_owned(),
            outline_changed: false,
            advance_changed: false,
            unicodes_changed: false,
            components_changed: true,
        }]
    );

    // An outline change alone does not flag the components.
    let mut after = before.clone();
    glyph_mut(&mut after, gid(3))
        .layers
        .get_mut(&master)
        .expect("layer")
        .paths
        .clear();
    let report = diff(&before, &after);
    assert!(report.modified[0].outline_changed);
    assert!(!report.modified[0].components_changed);

    // A layer in only one of the fonts changes the components only when it
    // has some.
    let mut after = before.clone();
    glyph_mut(&mut after, gid(2)).layers.clear();
    glyph_mut(&mut after, gid(3)).layers.clear();
    let report = diff(&before, &after);
    let flags: Vec<(&str, bool)> = report
        .modified
        .iter()
        .map(|glyph| (glyph.name.as_str(), glyph.components_changed))
        .collect();
    assert_eq!(flags, [("V", false), ("Aacute", true)]);

    // Both flags are part of the JSON of a diff.
    let value = serde_json::to_value(&report).expect("json");
    assert_eq!(value["kerning_changed"], false);
    assert_eq!(value["modified"][1]["components_changed"], true);
}

#[test]
fn edits_keep_the_kerning_and_restore_touches_only_what_changed() {
    let original = font();
    let mut doc = Document::new(original.clone());
    let actor = Actor::User;
    let rename = Command::UpdateGlyph {
        glyph: GlyphRef::Name("V".into()),
        name: Some("V.alt".into()),
        unicodes: None,
        export: None,
    };
    doc.execute(&actor, Rev(0), "rename", &[rename])
        .expect("rename");
    assert_eq!(doc.font().kerning, original.kerning);

    // The kerning did not change, so restoring touches the glyph alone.
    let outcome = doc.restore(&actor, Rev(1), Rev(0)).expect("restore");
    assert_eq!(
        outcome.touched.into_iter().collect::<Vec<_>>(),
        [EntityKey::Glyph(gid(2))]
    );
    assert_eq!(**doc.font(), original);
}
