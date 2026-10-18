//! Provenance: setting it, and which edits send an approved design back to
//! review (design changes) and which do not (metrics and metadata).

mod common;

use common::{base_font, glyph, id_of};
use std::collections::BTreeSet;

use forma_edit::{Command, EditError, GlyphRef, NodeAction, mark_design_changes};
use forma_model::{EntityKey, Font, PathId, Provenance, ProvenanceDetail, Review, Source};

/// One command as a transaction: applied, then the review rule.
fn apply(font: &Font, cmd: &Command) -> Result<(Font, BTreeSet<EntityKey>), EditError> {
    let (mut next, touched) = forma_edit::apply(font, cmd)?;
    mark_design_changes(font, &mut next, &touched)?;
    Ok((next, touched))
}

fn approved(font: &Font, name: &str) -> Provenance {
    let mut provenance = Provenance::pending(
        Source::Traced,
        1,
        ProvenanceDetail::Traced {
            image_sha256: "00".repeat(32),
            candidates: vec![0],
            px_rect: [0.0, 0.0, 10.0, 10.0],
            iou_mask: 0.99,
            hausdorff_p95_px: None,
            topology_ok: None,
        },
    );
    provenance.review = Review::Approved;
    provenance.approved_geometry = Some(forma_geometry::design_hash(font, glyph(font, name)));
    provenance
}

fn with_approved_a() -> Font {
    let font = base_font();
    let provenance = approved(&font, "A");
    let cmd = Command::SetProvenance {
        glyph: GlyphRef::Name("A".into()),
        provenance: Some(provenance),
    };
    apply(&font, &cmd).expect("set provenance").0
}

fn review_of(font: &Font, name: &str) -> (Review, bool) {
    let provenance = glyph(font, name).provenance.as_ref().expect("provenance");
    (provenance.review, provenance.edited)
}

#[test]
fn set_provenance_touches_the_glyph_once() {
    let font = base_font();
    let cmd = Command::SetProvenance {
        glyph: GlyphRef::Name("A".into()),
        provenance: Some(approved(&font, "A")),
    };
    let (next, touched) = apply(&font, &cmd).expect("apply");
    assert!(touched.contains(&EntityKey::Glyph(id_of(&font, "A"))));
    // The same provenance again changes nothing.
    let (_, again) = apply(&next, &cmd).expect("apply again");
    assert!(again.is_empty());
}

#[test]
fn out_of_range_scores_are_refused() {
    let font = base_font();
    let mut provenance = approved(&font, "A");
    provenance.detail = ProvenanceDetail::Synthesized {
        recipe: "x".into(),
        recipe_version: 1,
        sources: vec![],
        style_score: 1.5,
    };
    let cmd = Command::SetProvenance {
        glyph: GlyphRef::Name("A".into()),
        provenance: Some(provenance),
    };
    assert!(apply(&font, &cmd).is_err());
}

#[test]
fn editing_approved_glyph_geometry_resets_review_and_sets_edited() {
    let font = with_approved_a();
    let cmd = Command::NodesEdit {
        glyph: GlyphRef::Name("A".into()),
        master: None,
        path_id: PathId(0),
        action: NodeAction::Move {
            indices: vec![2],
            dx: 0.0,
            dy: 15.0,
        },
    };
    let (next, _) = apply(&font, &cmd).expect("move");
    assert_eq!(review_of(&next, "A"), (Review::Pending, true));
    let provenance = glyph(&next, "A").provenance.as_ref().expect("provenance");
    assert_eq!(provenance.source, Source::Traced, "the origin is kept");
    assert!(provenance.approved_geometry.is_none());
}

#[test]
fn metrics_and_rename_keep_approval() {
    let font = with_approved_a();
    let metrics = [
        Command::SetMetrics {
            glyph: GlyphRef::Name("A".into()),
            master: None,
            advance_width: None,
            lsb: Some(37.25),
            rsb: Some(61.5),
        },
        Command::SetMetrics {
            glyph: GlyphRef::Name("A".into()),
            master: None,
            advance_width: Some(900.0),
            lsb: None,
            rsb: None,
        },
        Command::UpdateGlyph {
            glyph: GlyphRef::Name("A".into()),
            name: Some("A.alt".into()),
            unicodes: Some(vec![]),
            export: Some(false),
        },
    ];
    let mut current = font;
    for cmd in &metrics {
        current = apply(&current, cmd).expect("apply").0;
    }
    assert_eq!(review_of(&current, "A.alt"), (Review::Approved, false));
    let g = glyph(&current, "A.alt");
    assert_eq!(
        g.provenance
            .as_ref()
            .and_then(|p| p.approved_geometry.clone()),
        Some(forma_geometry::design_hash(&current, g)),
        "moving the outline sideways keeps the design hash"
    );
}

#[test]
fn glyphs_without_provenance_are_left_alone() {
    let font = base_font();
    let cmd = Command::NodesEdit {
        glyph: GlyphRef::Name("B".into()),
        master: None,
        path_id: PathId(0),
        action: NodeAction::Move {
            indices: vec![0],
            dx: 3.0,
            dy: 0.0,
        },
    };
    let (next, _) = apply(&font, &cmd).expect("move");
    assert!(glyph(&next, "B").provenance.is_none());
}

#[test]
fn a_rejection_stands_after_a_design_change() {
    let font = with_approved_a();
    let mut rejected = glyph(&font, "A").provenance.clone().expect("provenance");
    rejected.review = Review::Rejected;
    rejected.approved_geometry = None;
    let cmd = Command::SetProvenance {
        glyph: GlyphRef::Name("A".into()),
        provenance: Some(rejected),
    };
    let font = apply(&font, &cmd).expect("reject").0;
    let edit = Command::NodesEdit {
        glyph: GlyphRef::Name("A".into()),
        master: None,
        path_id: PathId(0),
        action: NodeAction::Move {
            indices: vec![1],
            dx: 4.0,
            dy: 0.0,
        },
    };
    let (next, _) = apply(&font, &edit).expect("move");
    assert_eq!(review_of(&next, "A"), (Review::Rejected, true));
    assert!(!glyph(&next, "A").is_exported());
}
