//! Document history: undo, redo, revert, restore and the attributed history
//! entries.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;

use common::doc::*;
use common::*;
use forma_edit::{EditError, EntryKind, FontDiff, HistoryEntry, TxId, TxOutcome};
use forma_model::{EntityKey, Rev};
use serde_json::json;

// ------------------------------------------------------------- undo/redo

#[test]
fn undo_with_nothing_to_undo_is_not_allowed() {
    let mut doc = new_doc();
    let err = doc.undo(&user()).unwrap_err();
    assert_eq!(err, EditError::NotAllowed("nothing to undo".to_owned()));
    let err = doc.redo(&user()).unwrap_err();
    assert_eq!(err, EditError::NotAllowed("nothing to redo".to_owned()));
    assert_eq!(doc.rev(), Rev(0));
    assert!(doc.history().is_empty());
}

#[test]
fn undo_only_own_last_transaction() {
    let mut doc = new_doc();
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    commit(&mut doc, &agent(), "Widen B", vec![advance("B", 750.0)]);

    let err = doc.undo(&user()).unwrap_err();
    assert_eq!(
        err,
        EditError::NotAllowed(
            "last transaction belongs to agent:claude; use history_revert".to_owned()
        )
    );
    assert_eq!(doc.rev(), Rev(2));
    assert!(!doc.can_undo(&user()));
    assert!(doc.can_undo(&agent()));

    doc.undo(&agent())
        .expect("the agent undoes its own transaction");
    assert_eq!(width(&doc, "B"), 700.0);
    assert_eq!(width(&doc, "A"), 650.0);

    // Now the user's transaction is the last undoable one.
    doc.undo(&user())
        .expect("the user undoes its own transaction");
    assert_eq!(width(&doc, "A"), 600.0);
    assert_eq!(doc.rev(), Rev(4));
}

#[test]
fn undo_restores_the_previous_state_as_a_new_revision() {
    let mut doc = new_doc();
    let original = Arc::clone(doc.font());
    let (a, b) = (key(&doc, "A"), key(&doc, "B"));
    commit(
        &mut doc,
        &user(),
        "Two glyphs",
        vec![advance("A", 650.0), set_unicodes("B", &[0x42, 0x62])],
    );

    let out = doc.undo(&user()).expect("undoes");
    assert_eq!(out.tx, TxId(2));
    assert_eq!(out.rev, Rev(2));
    assert_eq!(out.touched, BTreeSet::from([a, b]));
    assert_eq!(**doc.font(), *original);
    assert_eq!(doc.rev(), Rev(2));
    // The undone revision is still in the history.
    assert_eq!(
        layer(&doc.snapshot_at(Rev(1)).unwrap(), "A").advance_width,
        650.0
    );
    let entry = doc.last_entry().expect("an entry");
    assert_eq!(entry.kind, EntryKind::Undo);
    assert_eq!(entry.actor, user());
    assert_eq!(entry.touched, BTreeSet::from([a, b]));
    assert_eq!(doc.entity_rev(&a), Rev(2));
}

#[test]
fn undo_counts_as_a_change_for_concurrency() {
    let mut doc = new_doc();
    let a = key(&doc, "A");
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    let agent_base = doc.rev();
    doc.undo(&user()).expect("undoes");
    let err = doc
        .execute(&agent(), agent_base, "Narrow A", &[advance("A", 500.0)])
        .unwrap_err();
    assert_eq!(conflict(err).conflicts, vec![(a, Rev(2), user())]);
}

#[test]
fn redo_reapplies_the_undone_transaction() {
    let mut doc = new_doc();
    let original = Arc::clone(doc.font());
    let a = key(&doc, "A");
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    let changed = Arc::clone(doc.font());
    doc.undo(&user()).expect("undoes");
    assert!(doc.can_redo(&user()));

    let out = doc.redo(&user()).expect("redoes");
    assert_eq!(out.rev, Rev(3));
    assert_eq!(out.tx, TxId(3));
    assert_eq!(out.touched, BTreeSet::from([a]));
    assert_eq!(**doc.font(), *changed);
    assert_eq!(doc.last_entry().unwrap().kind, EntryKind::Redo);
    assert!(!doc.can_redo(&user()));

    // A redone transaction can be undone again.
    doc.undo(&user()).expect("undoes again");
    assert_eq!(**doc.font(), *original);
    assert_eq!(doc.rev(), Rev(4));
}

#[test]
fn redo_only_own_undone_transaction() {
    let mut doc = new_doc();
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    doc.undo(&user()).expect("undoes");
    let message = not_allowed(doc.redo(&agent()).unwrap_err());
    assert!(message.contains("user"), "{message}");
    assert!(!doc.can_redo(&agent()));
    doc.redo(&user())
        .expect("the user redoes its own transaction");
    assert_eq!(width(&doc, "A"), 650.0);
}

#[test]
fn a_new_commit_clears_the_redo_stack() {
    let mut doc = new_doc();
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    doc.undo(&user()).expect("undoes");
    commit(&mut doc, &user(), "Widen B", vec![advance("B", 750.0)]);
    let err = doc.redo(&user()).unwrap_err();
    assert_eq!(err, EditError::NotAllowed("nothing to redo".to_owned()));
    assert_eq!(width(&doc, "A"), 600.0);
}

#[test]
fn undo_and_redo_interleave_across_actors() {
    let mut doc = new_doc();
    let s0 = Arc::clone(doc.font());
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    let s1 = Arc::clone(doc.font());
    commit(&mut doc, &agent(), "Widen B", vec![advance("B", 750.0)]);
    let s2 = Arc::clone(doc.font());

    doc.undo(&agent()).expect("agent undoes B");
    assert_eq!(**doc.font(), *s1);
    doc.undo(&user()).expect("user undoes A");
    assert_eq!(**doc.font(), *s0);

    // The user's transaction was undone last, so only the user can redo now.
    assert!(matches!(doc.redo(&agent()), Err(EditError::NotAllowed(_))));
    doc.redo(&user()).expect("user redoes A");
    assert_eq!(**doc.font(), *s1);
    doc.redo(&agent()).expect("agent redoes B");
    assert_eq!(**doc.font(), *s2);
    assert_eq!(doc.rev(), Rev(6));
}

#[test]
fn last_entry_and_can_undo_report_the_latest_state() {
    let mut doc = new_doc();
    assert!(!doc.can_undo(&user()));
    assert!(!doc.can_redo(&user()));
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    assert!(doc.can_undo(&user()));
    assert!(!doc.can_undo(&agent()));
    assert_eq!(doc.last_entry(), doc.history().last());
    assert_eq!(doc.last_entry().unwrap().kind, EntryKind::Commit);
    doc.undo(&user()).expect("undoes");
    assert!(!doc.can_undo(&user()));
    assert!(doc.can_redo(&user()));
}

// ------------------------------------------------------------------ revert

#[test]
fn revert_restores_only_the_entities_of_the_transaction() {
    let mut doc = new_doc();
    let (a, b) = (key(&doc, "A"), key(&doc, "B"));
    let first = commit(
        &mut doc,
        &user(),
        "Widen A",
        vec![advance("A", 650.0), set_unicodes("B", &[0x42, 0x62])],
    );
    commit(&mut doc, &agent(), "Widen C", vec![advance("C", 700.0)]);

    let out = doc.revert(&agent(), first.tx).expect("reverts");
    assert_eq!(out.rev, Rev(3));
    assert_eq!(out.tx, TxId(3));
    assert_eq!(out.touched, BTreeSet::from([a, b]));
    assert_eq!(width(&doc, "A"), 600.0);
    assert_eq!(glyph(doc.font(), "B").unicodes, vec![0x42]);
    assert_eq!(width(&doc, "C"), 700.0, "later changes are kept");
    let entry = doc.last_entry().unwrap();
    assert_eq!(entry.kind, EntryKind::Revert);
    assert_eq!(entry.actor, agent());
    assert_eq!(doc.entity_rev(&a), Rev(3));

    // A revert is an undoable transaction of the actor who made it.
    assert!(doc.can_undo(&agent()));
    doc.undo(&agent()).expect("undoes the revert");
    assert_eq!(width(&doc, "A"), 650.0);
}

#[test]
fn revert_conflicts_when_entity_changed_later() {
    let mut doc = new_doc();
    let a = key(&doc, "A");
    let first = commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    let second = commit(
        &mut doc,
        &agent(),
        "Retouch",
        vec![advance("A", 660.0), advance("B", 750.0)],
    );

    let err = doc.revert(&user(), first.tx).unwrap_err();
    let info = conflict(err);
    assert_eq!(info.current_rev, Rev(2));
    assert_eq!(info.conflicts, vec![(a, Rev(2), agent())]);
    assert_eq!(doc.rev(), Rev(2));
    assert_eq!(width(&doc, "A"), 660.0);

    // The later transaction itself can be reverted.
    doc.revert(&user(), second.tx).expect("reverts");
    assert_eq!(width(&doc, "A"), 650.0);
    assert_eq!(width(&doc, "B"), 700.0);
}

#[test]
fn revert_of_a_creation_removes_the_glyph_and_keeps_later_glyphs() {
    let mut doc = new_doc();
    let created = commit(
        &mut doc,
        &user(),
        "Create D",
        vec![create("D", &[0x44], 500.0, vec![])],
    );
    let d = key(&doc, "D");
    commit(
        &mut doc,
        &agent(),
        "Create E",
        vec![create("E", &[0x45], 500.0, vec![])],
    );

    let out = doc.revert(&user(), created.tx).expect("reverts");
    assert!(doc.font().glyph_by_name("D").is_none());
    assert_eq!(names_in_order(&doc), vec!["A", "B", "C", "E"]);
    assert!(out.touched.contains(&d));
    assert!(out.touched.contains(&EntityKey::GlyphOrder));
    doc.font().validate_invariants().expect("valid");
}

#[test]
fn revert_of_a_deletion_reinserts_the_glyph_at_its_former_index() {
    let mut doc = new_doc();
    let original_b = Arc::clone(doc.font().glyph_by_name("B").unwrap());
    let deleted = commit(&mut doc, &user(), "Delete B", vec![delete(&["B"])]);
    commit(
        &mut doc,
        &agent(),
        "Create D",
        vec![create("D", &[], 500.0, vec![])],
    );

    doc.revert(&agent(), deleted.tx).expect("reverts");
    assert_eq!(names_in_order(&doc), vec!["A", "B", "C", "D"]);
    assert_eq!(**doc.font().glyph_by_name("B").unwrap(), *original_b);
}

#[test]
fn revert_clamps_the_reinsertion_index_to_the_end() {
    let mut doc = new_doc();
    let deleted = commit(&mut doc, &user(), "Delete C", vec![delete(&["C"])]);
    commit(&mut doc, &agent(), "Delete A B", vec![delete(&["A", "B"])]);
    doc.revert(&user(), deleted.tx).expect("reverts");
    assert_eq!(names_in_order(&doc), vec!["C"]);
}

#[test]
fn revert_restores_font_info_and_vertical_metrics() {
    let mut doc = new_doc();
    let first = commit(
        &mut doc,
        &user(),
        "Font-wide",
        vec![version_minor(7), units_per_em(2048), x_height(520.0)],
    );
    commit(&mut doc, &agent(), "Widen A", vec![advance("A", 650.0)]);
    doc.revert(&agent(), first.tx).expect("reverts");
    let font = doc.font();
    assert_eq!(font.info.version_minor, 0);
    assert_eq!(font.units_per_em, 1000);
    assert_eq!(font.default_master().metrics.x_height, 500.0);
    assert_eq!(width(&doc, "A"), 650.0);
}

#[test]
fn revert_rejects_unknown_and_non_revertible_transactions() {
    let mut doc = new_doc();
    let err = doc.revert(&user(), TxId(99)).unwrap_err();
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");

    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    let undo = doc.undo(&user()).expect("undoes");
    let err = doc.revert(&user(), undo.tx).unwrap_err();
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
    let redo = doc.redo(&user()).expect("redoes");
    let err = doc.revert(&user(), redo.tx).unwrap_err();
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
    assert_eq!(doc.rev(), Rev(3));
}

#[test]
fn revert_that_would_break_an_invariant_is_rejected() {
    let mut doc = new_doc();
    let deleted = commit(&mut doc, &user(), "Delete C", vec![delete(&["C"])]);
    commit(
        &mut doc,
        &agent(),
        "New C",
        vec![create("C", &[0x43], 500.0, vec![])],
    );
    let before = Arc::clone(doc.font());
    let err = doc.revert(&user(), deleted.tx).unwrap_err();
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
    assert_eq!(doc.rev(), Rev(2));
    assert!(Arc::ptr_eq(doc.font(), &before));
}

#[test]
fn revert_clears_the_redo_stack() {
    let mut doc = new_doc();
    let first = commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    commit(&mut doc, &agent(), "Widen B", vec![advance("B", 750.0)]);
    doc.undo(&agent()).expect("undoes");
    doc.revert(&user(), first.tx).expect("reverts");
    assert!(!doc.can_redo(&agent()));
}

// ----------------------------------------------------------------- restore

#[test]
fn restore_creates_new_revision() {
    let mut doc = new_doc();
    let (b, c) = (key(&doc, "B"), key(&doc, "C"));
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    commit(&mut doc, &user(), "Delete C", vec![delete(&["C"])]);
    commit(&mut doc, &agent(), "Rename B", vec![rename("B", "Bee")]);
    let latest = doc.snapshot_at(Rev(3)).unwrap();

    let out = doc.restore(&agent(), Rev(1)).expect("restores");
    assert_eq!(out.rev, Rev(4));
    assert_eq!(out.tx, TxId(4));
    assert_eq!(out.touched, BTreeSet::from([b, c, EntityKey::GlyphOrder]));
    assert_eq!(**doc.font(), *doc.snapshot_at(Rev(1)).unwrap());
    assert_eq!(width(&doc, "A"), 650.0);
    assert_eq!(names_in_order(&doc), vec!["A", "B", "C"]);
    // Earlier revisions are kept.
    assert!(Arc::ptr_eq(&doc.snapshot_at(Rev(3)).unwrap(), &latest));
    let entry = doc.last_entry().unwrap();
    assert_eq!(entry.kind, EntryKind::Restore);
    assert_eq!(entry.actor, agent());
    assert_eq!(doc.entity_rev(&c), Rev(4));
}

#[test]
fn restore_rejects_an_unknown_revision() {
    let mut doc = new_doc();
    let err = doc.restore(&user(), Rev(5)).unwrap_err();
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
}

#[test]
fn restore_to_an_identical_state_is_rejected() {
    let mut doc = new_doc();
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    commit(&mut doc, &user(), "Narrow A", vec![advance("A", 600.0)]);
    for rev in [Rev(0), Rev(2)] {
        let err = doc.restore(&user(), rev).unwrap_err();
        match err {
            EditError::InvalidArgument(message) => {
                assert!(message.contains("already at that state"), "{message}");
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }
    assert_eq!(doc.rev(), Rev(2));
}

#[test]
fn restore_is_undoable_and_clears_the_redo_stack() {
    let mut doc = new_doc();
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    commit(&mut doc, &user(), "Widen B", vec![advance("B", 750.0)]);
    doc.undo(&user()).expect("undoes");
    let before_restore = Arc::clone(doc.font());

    doc.restore(&agent(), Rev(0)).expect("restores");
    assert_eq!(width(&doc, "A"), 600.0);
    assert!(!doc.can_redo(&user()));
    assert!(!doc.can_undo(&user()));

    doc.undo(&agent()).expect("undoes the restore");
    assert_eq!(**doc.font(), *before_restore);
}

// ------------------------------------------------------------------ history

#[test]
fn history_records_actor_and_label() {
    let mut doc = new_doc();
    let (a, b) = (key(&doc, "A"), key(&doc, "B"));
    commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    commit(&mut doc, &agent(), "", vec![advance("B", 750.0)]);
    doc.undo(&agent()).expect("undoes");

    let history = doc.history();
    assert_eq!(history.len(), 3);
    assert_eq!(
        history[0],
        HistoryEntry {
            tx: TxId(1),
            rev: Rev(1),
            actor: user(),
            label: "Widen A".to_owned(),
            touched: BTreeSet::from([a]),
            kind: EntryKind::Commit,
        }
    );
    assert_eq!(
        history[1],
        HistoryEntry {
            tx: TxId(2),
            rev: Rev(2),
            actor: agent(),
            label: "Edit".to_owned(),
            touched: BTreeSet::from([b]),
            kind: EntryKind::Commit,
        }
    );
    assert_eq!(history[2].tx, TxId(3));
    assert_eq!(history[2].rev, Rev(3));
    assert_eq!(history[2].actor, agent());
    assert_eq!(history[2].kind, EntryKind::Undo);
    assert!(history[2].label.contains("Edit"), "{}", history[2].label);
}

#[test]
fn every_state_change_increments_rev_by_one() {
    let mut doc = new_doc();
    let mut outcomes = vec![commit(
        &mut doc,
        &user(),
        "Widen A",
        vec![advance("A", 650.0)],
    )];
    outcomes.push(doc.undo(&user()).expect("undo"));
    outcomes.push(doc.redo(&user()).expect("redo"));
    let widen_b = commit(&mut doc, &agent(), "Widen B", vec![advance("B", 750.0)]);
    outcomes.push(widen_b.clone());
    outcomes.push(doc.revert(&user(), widen_b.tx).expect("revert"));
    outcomes.push(doc.restore(&agent(), Rev(0)).expect("restore"));

    for (index, outcome) in outcomes.iter().enumerate() {
        let n = u64::try_from(index).unwrap() + 1;
        assert_eq!(outcome.rev, Rev(n));
        assert_eq!(outcome.tx, TxId(n));
        assert_eq!(doc.history()[index].tx, TxId(n));
        assert_eq!(doc.history()[index].rev, Rev(n));
    }
    let kinds: Vec<EntryKind> = doc.history().iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        vec![
            EntryKind::Commit,
            EntryKind::Undo,
            EntryKind::Redo,
            EntryKind::Commit,
            EntryKind::Revert,
            EntryKind::Restore,
        ]
    );
    assert_eq!(doc.rev(), Rev(6));
    for rev in 0..=6 {
        assert!(doc.snapshot_at(Rev(rev)).is_some(), "revision {rev}");
    }
    assert!(doc.snapshot_at(Rev(7)).is_none());
}

#[test]
fn history_types_serialize_as_documented() {
    let mut doc = new_doc();
    let a = id_of(doc.font(), "A");
    let out = commit(&mut doc, &user(), "Widen A", vec![advance("A", 650.0)]);
    let touched = json!([format!("glyph:{a}")]);

    assert_eq!(
        serde_json::to_value(&out).unwrap(),
        json!({"tx": 1, "rev": 1, "touched": touched})
    );
    let entry = &doc.history()[0];
    let value = serde_json::to_value(entry).unwrap();
    assert_eq!(
        value,
        json!({
            "tx": 1,
            "rev": 1,
            "actor": "user",
            "label": "Widen A",
            "touched": touched,
            "kind": "commit",
        })
    );
    let back: HistoryEntry = serde_json::from_value(value).unwrap();
    assert_eq!(&back, entry);

    let kinds = [
        (EntryKind::Commit, "commit"),
        (EntryKind::Undo, "undo"),
        (EntryKind::Redo, "redo"),
        (EntryKind::Revert, "revert"),
        (EntryKind::Restore, "restore"),
    ];
    for (kind, text) in kinds {
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(text));
    }

    for schema in [
        serde_json::to_value(schemars::schema_for!(HistoryEntry)).unwrap(),
        serde_json::to_value(schemars::schema_for!(TxOutcome)).unwrap(),
        serde_json::to_value(schemars::schema_for!(FontDiff)).unwrap(),
        serde_json::to_value(schemars::schema_for!(EntryKind)).unwrap(),
        serde_json::to_value(schemars::schema_for!(TxId)).unwrap(),
    ] {
        assert!(schema.is_object());
    }
}
