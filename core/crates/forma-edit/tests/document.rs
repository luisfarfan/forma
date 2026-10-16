//! Documents: revisions, per-entity optimistic concurrency and font diffs.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;

use common::doc::*;
use common::*;
use forma_edit::{EditError, FontDiff, GlyphDiff, TxId, diff};
use forma_model::{EntityKey, Rev};
use serde_json::json;

// ------------------------------------------------------- new and execute

#[test]
fn new_document_starts_at_rev_zero() {
    let doc = new_doc();
    assert_eq!(doc.rev(), Rev(0));
    assert!(doc.history().is_empty());
    assert!(doc.last_entry().is_none());
    let zero = doc.snapshot_at(Rev(0)).expect("revision 0 exists");
    assert!(Arc::ptr_eq(&zero, doc.font()));
    assert!(doc.snapshot_at(Rev(1)).is_none());
    assert_eq!(doc.entity_rev(&EntityKey::FontInfo), Rev(0));
    assert_eq!(doc.entity_rev(&key(&doc, "A")), Rev(0));
    assert_eq!(doc.entity_rev(&EntityKey::GlyphOrder), Rev(0));
}

#[test]
fn execute_commits_a_new_revision() {
    let mut doc = new_doc();
    let a = key(&doc, "A");
    let out = doc
        .execute(&user(), Rev(0), "Widen A", &[advance("A", 650.0)])
        .expect("applies");
    assert_eq!(out.tx, TxId(1));
    assert_eq!(out.rev, Rev(1));
    assert_eq!(out.touched, BTreeSet::from([a]));
    assert_eq!(doc.rev(), Rev(1));
    assert_eq!(width(&doc, "A"), 650.0);
    let snapshot = doc.snapshot_at(Rev(1)).expect("revision 1 exists");
    assert!(Arc::ptr_eq(&snapshot, doc.font()));
    assert_eq!(
        layer(&doc.snapshot_at(Rev(0)).unwrap(), "A").advance_width,
        600.0
    );
}

#[test]
fn execute_applies_several_commands_as_one_transaction() {
    let mut doc = new_doc();
    let out = commit(
        &mut doc,
        &user(),
        "Batch",
        vec![
            advance("A", 610.0),
            advance("A", 620.0),
            set_unicodes("B", &[0x42, 0x62]),
        ],
    );
    assert_eq!(out.rev, Rev(1));
    assert_eq!(
        out.touched,
        BTreeSet::from([key(&doc, "A"), key(&doc, "B")])
    );
    assert_eq!(width(&doc, "A"), 620.0);
    assert_eq!(glyph(doc.font(), "B").unicodes, vec![0x42, 0x62]);
    assert_eq!(doc.history().len(), 1);
}

#[test]
fn execute_is_all_or_nothing() {
    let mut doc = new_doc();
    let before = Arc::clone(doc.font());
    let err = doc
        .execute(
            &user(),
            Rev(0),
            "Half broken",
            &[advance("A", 650.0), advance("Missing", 10.0)],
        )
        .unwrap_err();
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
    assert_eq!(doc.rev(), Rev(0));
    assert!(Arc::ptr_eq(doc.font(), &before));
    assert!(doc.history().is_empty());
    assert_eq!(doc.entity_rev(&key(&doc, "A")), Rev(0));
    assert!(doc.snapshot_at(Rev(1)).is_none());
}

#[test]
fn execute_rejects_an_empty_command_list() {
    let mut doc = new_doc();
    let err = doc.execute(&user(), Rev(0), "Nothing", &[]).unwrap_err();
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
    assert_eq!(doc.rev(), Rev(0));
}

#[test]
fn execute_rejects_a_base_rev_from_the_future() {
    let mut doc = new_doc();
    let err = doc
        .execute(&user(), Rev(1), "Future", &[advance("A", 650.0)])
        .unwrap_err();
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
    assert_eq!(doc.rev(), Rev(0));
    assert_eq!(width(&doc, "A"), 600.0);
}

#[test]
fn empty_label_defaults_to_edit() {
    let mut doc = new_doc();
    commit(&mut doc, &user(), "", vec![advance("A", 650.0)]);
    assert_eq!(doc.history()[0].label, "Edit");
}

#[test]
fn document_can_move_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<forma_edit::Document>();
}

#[test]
fn snapshots_share_unchanged_glyphs() {
    let mut doc = new_doc();
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    let zero = doc.snapshot_at(Rev(0)).unwrap();
    let one = doc.snapshot_at(Rev(1)).unwrap();
    let b = id_of(&zero, "B");
    let a = id_of(&zero, "A");
    assert!(Arc::ptr_eq(&zero.glyphs[&b], &one.glyphs[&b]));
    assert!(!Arc::ptr_eq(&zero.glyphs[&a], &one.glyphs[&a]));
}

// ------------------------------------------------- optimistic concurrency

#[test]
fn stale_base_rev_on_touched_entity_conflicts() {
    let mut doc = new_doc();
    let base = doc.rev();
    let a = key(&doc, "A");
    doc.execute(&user(), base, "Widen A", &[advance("A", 650.0)])
        .expect("applies");
    let before = Arc::clone(doc.font());

    let err = doc
        .execute(&agent(), base, "Narrow A", &[advance("A", 500.0)])
        .unwrap_err();
    let info = conflict(err);
    assert_eq!(info.current_rev, Rev(1));
    assert_eq!(info.conflicts, vec![(a, Rev(1), user())]);

    // Nothing changed.
    assert_eq!(doc.rev(), Rev(1));
    assert!(Arc::ptr_eq(doc.font(), &before));
    assert_eq!(width(&doc, "A"), 650.0);
    assert_eq!(doc.history().len(), 1);

    // Rebasing on the latest revision succeeds.
    let latest = doc.rev();
    doc.execute(&agent(), latest, "Narrow A", &[advance("A", 500.0)])
        .expect("applies once rebased");
    assert_eq!(width(&doc, "A"), 500.0);
}

#[test]
fn stale_base_rev_on_other_entity_applies() {
    let mut doc = new_doc();
    let base = doc.rev();
    doc.execute(&user(), base, "Widen A", &[advance("A", 650.0)])
        .expect("applies");
    let out = doc
        .execute(&agent(), base, "Widen B", &[advance("B", 750.0)])
        .expect("B was not changed since the base revision");
    assert_eq!(out.rev, Rev(2));
    assert_eq!(width(&doc, "A"), 650.0);
    assert_eq!(width(&doc, "B"), 750.0);
    assert_eq!(doc.entity_rev(&key(&doc, "A")), Rev(1));
    assert_eq!(doc.entity_rev(&key(&doc, "B")), Rev(2));
}

#[test]
fn conflict_lists_every_conflicting_entity_sorted() {
    let mut doc = new_doc();
    let master = doc.font().default_master().id;
    let (a, b, c) = (key(&doc, "A"), key(&doc, "B"), key(&doc, "C"));
    commit(
        &mut doc,
        &user(),
        "Several",
        vec![
            version_minor(5),
            advance("C", 660.0),
            advance("A", 610.0),
            x_height(510.0),
        ],
    );
    let err = doc
        .execute(
            &agent(),
            Rev(0),
            "Overlapping",
            &[
                advance("A", 620.0),
                advance("B", 720.0),
                advance("C", 670.0),
                version_minor(6),
                x_height(520.0),
            ],
        )
        .unwrap_err();
    let info = conflict(err);
    assert_eq!(info.current_rev, Rev(1));
    let mut expected = vec![
        (EntityKey::FontInfo, Rev(1), user()),
        (EntityKey::VerticalMetrics(master), Rev(1), user()),
        (a, Rev(1), user()),
        (c, Rev(1), user()),
    ];
    expected.sort_by_key(|(entity, _, _)| *entity);
    assert_eq!(info.conflicts, expected);
    assert!(!info.conflicts.iter().any(|(entity, _, _)| *entity == b));
    assert_eq!(doc.rev(), Rev(1));
}

#[test]
fn glyph_order_never_conflicts() {
    let mut doc = new_doc();
    let base = doc.rev();
    for (actor, cmd) in [
        (user(), create("D", &[], 500.0, vec![])),
        (agent(), create("E", &[], 500.0, vec![])),
        (user(), delete(&["A"])),
        (agent(), delete(&["B"])),
    ] {
        doc.execute(&actor, base, "Concurrent", &[cmd])
            .expect("disjoint glyphs never conflict, even though all touch the glyph order");
    }
    assert_eq!(names_in_order(&doc), vec!["C", "D", "E"]);
    assert_eq!(doc.rev(), Rev(4));
    assert_eq!(doc.entity_rev(&EntityKey::GlyphOrder), Rev(4));
}

#[test]
fn deleting_a_glyph_modified_by_another_actor_conflicts() {
    let mut doc = new_doc();
    let a = key(&doc, "A");
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    let err = doc
        .execute(&agent(), Rev(0), "Delete A", &[delete(&["A"])])
        .unwrap_err();
    let info = conflict(err);
    assert_eq!(info.conflicts, vec![(a, Rev(1), user())]);
    assert!(doc.font().glyph_by_name("A").is_some());
}

#[test]
fn entity_revs_record_the_last_change() {
    let mut doc = new_doc();
    let a = key(&doc, "A");
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    commit(&mut doc, &agent(), "Version", vec![version_minor(3)]);
    assert_eq!(doc.entity_rev(&a), Rev(1));
    assert_eq!(doc.entity_rev(&EntityKey::FontInfo), Rev(2));
    assert_eq!(doc.entity_rev(&key(&doc, "B")), Rev(0));
    // The actor of the last change is reported in conflicts.
    let err = doc
        .execute(&user(), Rev(1), "Version", &[version_minor(4)])
        .unwrap_err();
    assert_eq!(
        conflict(err).conflicts,
        vec![(EntityKey::FontInfo, Rev(2), agent())]
    );
}

#[test]
fn a_new_glyph_never_conflicts() {
    let mut doc = new_doc();
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    let out = doc
        .execute(
            &agent(),
            Rev(0),
            "Create D",
            &[create("D", &[], 400.0, vec![])],
        )
        .expect("a glyph created by the transaction has no history");
    assert!(out.touched.contains(&key(&doc, "D")));
    assert!(out.touched.contains(&EntityKey::GlyphOrder));
}

// --------------------------------------------------------------------- diff

#[test]
fn diff_reports_added_modified_removed() {
    let doc = new_doc();
    let before = Arc::clone(doc.font());
    let (after, _) = run(&before, create("D", &[0x44], 500.0, vec![]));
    let (after, _) = run(&after, delete(&["C"]));
    let (after, _) = run(&after, advance("A", 650.0));
    let (after, _) = run(&after, outline("B", 300.0));
    let (after, _) = run(&after, set_unicodes("B", &[0x42, 0x62]));

    let report = diff(&before, &after);
    assert_eq!(
        report,
        FontDiff {
            added: vec!["D".to_owned()],
            removed: vec!["C".to_owned()],
            modified: vec![
                GlyphDiff {
                    name: "A".to_owned(),
                    outline_changed: false,
                    advance_changed: true,
                    unicodes_changed: false,
                },
                GlyphDiff {
                    name: "B".to_owned(),
                    outline_changed: true,
                    advance_changed: false,
                    unicodes_changed: true,
                },
            ],
            info_changed: false,
            metrics_changed: false,
        }
    );
}

#[test]
fn diff_of_identical_fonts_is_empty() {
    let doc = new_doc();
    let report = diff(doc.font(), doc.font());
    assert_eq!(
        report,
        FontDiff {
            added: vec![],
            removed: vec![],
            modified: vec![],
            info_changed: false,
            metrics_changed: false,
        }
    );
}

#[test]
fn diff_reports_info_and_metrics_changes() {
    let doc = new_doc();
    let before = Arc::clone(doc.font());
    let (info, _) = run(&before, version_minor(2));
    let report = diff(&before, &info);
    assert!(report.info_changed);
    assert!(!report.metrics_changed);

    let (upm, _) = run(&before, units_per_em(2048));
    assert!(diff(&before, &upm).info_changed);

    let (metrics, _) = run(&before, x_height(480.0));
    let report = diff(&before, &metrics);
    assert!(!report.info_changed);
    assert!(report.metrics_changed);
    assert!(report.modified.is_empty());
}

#[test]
fn diff_lists_a_renamed_glyph_under_its_new_name() {
    let doc = new_doc();
    let before = Arc::clone(doc.font());
    let (after, _) = run(&before, rename("B", "Bee"));
    let report = diff(&before, &after);
    assert!(report.added.is_empty());
    assert!(report.removed.is_empty());
    assert_eq!(
        report.modified,
        vec![GlyphDiff {
            name: "Bee".to_owned(),
            outline_changed: false,
            advance_changed: false,
            unicodes_changed: false,
        }]
    );
}

#[test]
fn diff_serializes_as_documented() {
    let doc = new_doc();
    let before = Arc::clone(doc.font());
    let (after, _) = run(&before, advance("A", 650.0));
    let value = serde_json::to_value(diff(&before, &after)).unwrap();
    assert_eq!(
        value,
        json!({
            "added": [],
            "removed": [],
            "modified": [{
                "name": "A",
                "outline_changed": false,
                "advance_changed": true,
                "unicodes_changed": false,
            }],
            "info_changed": false,
            "metrics_changed": false,
        })
    );
}
