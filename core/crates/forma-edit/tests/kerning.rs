//! Kerning commands (`set_kerning_pair`, `set_kerning_group`,
//! `delete_kerning_group`), the kerning cascade of `delete_glyphs`, and how
//! documents commit, undo, revert and restore kerning.

mod common;

use common::doc::{agent, commit, conflict, user};
use common::*;
use forma_edit::{Command, Document, EditError, KernRefInput, affected_glyph_count};
use forma_model::{
    Actor, EntityKey, Font, KernRef, KernSide, Kerning, KerningGroupId, MasterId, Rev,
};
use proptest::collection::vec;
use proptest::prelude::*;

fn g(name: &str) -> KernRefInput {
    KernRefInput::Glyph(by_name(name))
}

fn k(name: &str) -> KernRefInput {
    KernRefInput::Group(name.to_owned())
}

fn pair(first: KernRefInput, second: KernRefInput, value: f64) -> Command {
    Command::SetKerningPair {
        master: None,
        first,
        second,
        value,
    }
}

fn set_group(name: &str, side: KernSide, members: &[&str]) -> Command {
    Command::SetKerningGroup {
        name: name.to_owned(),
        side,
        members: members.iter().map(|member| by_name(member)).collect(),
    }
}

fn delete_group(name: &str, side: KernSide) -> Command {
    Command::DeleteKerningGroup {
        name: name.to_owned(),
        side,
    }
}

/// `base_font()` (`A`, `B`) plus `V`, `T` and `o`, empty glyphs.
fn kern_font() -> Font {
    let mut font = base_font();
    for (name, codepoint) in [("V", 0x56), ("T", 0x54), ("o", 0x6f)] {
        add_glyph(&mut font, name, &[codepoint], 500.0, Vec::new());
    }
    font
}

fn glyph_ref(font: &Font, name: &str) -> KernRef {
    KernRef::Glyph(id_of(font, name))
}

fn group_id(font: &Font, side: KernSide, name: &str) -> KerningGroupId {
    font.kerning
        .group_by_name(side, name)
        .unwrap_or_else(|| panic!("no {side}-side group {name}"))
        .id
}

fn pair_key(font: &Font, first: KernRef, second: KernRef) -> EntityKey {
    EntityKey::KerningPair(font.default_master().id, first, second)
}

#[test]
fn set_pair_and_remove_with_zero() {
    let font = kern_font();
    let master = font.default_master().id;
    let (a, v) = (glyph_ref(&font, "A"), glyph_ref(&font, "V"));

    let (next, touched) = run(&font, pair(g("A"), g("V"), -50.0));
    assert_eq!(touched, keys([pair_key(&font, a, v)]));
    assert_eq!(next.kerning.pair(master, a, v), Some(-50.0));
    assert_eq!(affected_glyph_count(&font, &pair(g("A"), g("V"), -50.0)), 0);

    let (next, touched) = run(&next, pair(g("A"), g("V"), -60.0));
    assert_eq!(touched, keys([pair_key(&font, a, v)]));
    assert_eq!(next.kerning.pair(master, a, v), Some(-60.0));
    // The same value again changes nothing and touches nothing.
    let (same, touched) = run(&next, pair(g("A"), g("V"), -60.0));
    assert!(touched.is_empty());
    assert_eq!(same, next);

    // 0 removes the pair; the only pair of the master leaves no trace.
    let (cleared, touched) = run(&next, pair(g("A"), g("V"), 0.0));
    assert_eq!(touched, keys([pair_key(&font, a, v)]));
    assert_eq!(cleared.kerning, Kerning::default());
    let json = serde_json::to_value(&cleared).expect("json");
    assert!(json.get("kerning").is_none(), "{json}");
    // Removing a pair that is not there touches nothing.
    let (again, touched) = run(&cleared, pair(g("A"), g("V"), 0.0));
    assert!(touched.is_empty());
    assert_eq!(again.kerning, Kerning::default());

    // The limit is twice the units per em (1000).
    let (next, _) = run(&font, pair(g("V"), g("A"), -2000.0));
    assert_eq!(next.kerning.pair(master, v, a), Some(-2000.0));
    for value in [2000.5, -2001.0, f64::NAN, f64::INFINITY] {
        let error = fails(&font, pair(g("A"), g("V"), value));
        assert!(
            matches!(error, EditError::InvalidArgument(_)),
            "{value}: {error:?}"
        );
    }

    // Unknown glyphs, groups and masters.
    for cmd in [
        pair(g("Z"), g("V"), -10.0),
        pair(k("O"), g("V"), -10.0),
        Command::SetKerningPair {
            master: Some(MasterId::new()),
            first: g("A"),
            second: g("V"),
            value: -10.0,
        },
    ] {
        let error = fails(&font, cmd);
        assert!(matches!(error, EditError::NotFound(_)), "{error:?}");
    }

    // A group is looked up on the side of the pair it stands on.
    let (grouped, _) = run(&font, set_group("O", KernSide::First, &["B", "o"]));
    let error = fails(&grouped, pair(g("V"), k("O"), -10.0));
    assert!(
        matches!(&error, EditError::NotFound(message) if message.contains("second-side")),
        "{error:?}"
    );
    let (next, touched) = run(&grouped, pair(k("O"), g("V"), -25.0));
    let o = KernRef::Group(group_id(&grouped, KernSide::First, "O"));
    assert_eq!(touched, keys([pair_key(&grouped, o, v)]));
    assert_eq!(
        next.kerning
            .effective(master, id_of(&next, "o"), id_of(&next, "V")),
        -25.0
    );

    // Another master.
    let mut two = kern_font();
    let bold = add_master(&mut two);
    let (next, touched) = run(
        &two,
        Command::SetKerningPair {
            master: Some(bold),
            first: g("T"),
            second: g("o"),
            value: -70.0,
        },
    );
    let (t, o) = (glyph_ref(&two, "T"), glyph_ref(&two, "o"));
    assert_eq!(touched, keys([EntityKey::KerningPair(bold, t, o)]));
    assert_eq!(next.kerning.pair(bold, t, o), Some(-70.0));
    assert_eq!(next.kerning.pair(two.default_master().id, t, o), None);

    // The wire form of a side is externally tagged.
    let cmd: Command = serde_json::from_value(serde_json::json!({
        "op": "set_kerning_pair",
        "first": {"group": "O"},
        "second": {"glyph": "V"},
        "value": -25
    }))
    .expect("a command");
    assert_eq!(cmd, pair(k("O"), g("V"), -25.0));
}

#[test]
fn group_create_replace_members() {
    let font = kern_font();
    let (next, touched) = run(&font, set_group("round", KernSide::First, &["B", "o"]));
    let round = group_id(&next, KernSide::First, "round");
    assert_eq!(touched, keys([EntityKey::KerningGroup(round)]));
    let group = &next.kerning.groups[&round];
    assert_eq!(group.side, KernSide::First);
    assert_eq!(group.members, vec![id_of(&font, "B"), id_of(&font, "o")]);

    // Replacing the members keeps the group.
    let (next, touched) = run(&next, set_group("round", KernSide::First, &["o"]));
    assert_eq!(touched, keys([EntityKey::KerningGroup(round)]));
    assert_eq!(next.kerning.groups[&round].members, vec![id_of(&font, "o")]);
    assert_eq!(next.kerning.groups.len(), 1);
    // The same members again change nothing.
    let (same, touched) = run(&next, set_group("round", KernSide::First, &["o"]));
    assert!(touched.is_empty());
    assert_eq!(same, next);

    // The same name on the other side is another group; a group may be
    // empty.
    let (next, _) = run(&next, set_group("round", KernSide::Second, &[]));
    let second = group_id(&next, KernSide::Second, "round");
    assert_ne!(second, round);
    assert!(next.kerning.groups[&second].members.is_empty());

    // Refused: invalid names, unknown or repeated members.
    for name in ["", "1x", "a b", ".hidden"] {
        let error = fails(&font, set_group(name, KernSide::First, &["A"]));
        assert!(
            matches!(error, EditError::InvalidArgument(_)),
            "{name}: {error:?}"
        );
    }
    let error = fails(&font, set_group("O", KernSide::First, &["Z"]));
    assert!(matches!(error, EditError::NotFound(_)), "{error:?}");
    let error = fails(&font, set_group("O", KernSide::First, &["A", "A"]));
    assert!(matches!(error, EditError::InvalidArgument(_)), "{error:?}");

    // Deleting a group removes the pairs that use it, and only those.
    let (next, _) = run(
        &next,
        pair(KernRefInput::Group("round".into()), g("V"), -30.0),
    );
    let (next, _) = run(
        &next,
        pair(g("T"), KernRefInput::Group("round".into()), -20.0),
    );
    let (next, _) = run(&next, pair(g("A"), g("V"), -50.0));
    let (after, touched) = run(&next, delete_group("round", KernSide::First));
    let v = glyph_ref(&font, "V");
    assert_eq!(
        touched,
        keys([
            EntityKey::KerningGroup(round),
            pair_key(&font, KernRef::Group(round), v),
        ])
    );
    assert!(
        after
            .kerning
            .group_by_name(KernSide::First, "round")
            .is_none()
    );
    assert!(
        after
            .kerning
            .group_by_name(KernSide::Second, "round")
            .is_some()
    );
    let master = font.default_master().id;
    assert_eq!(after.kerning.pairs[&master].len(), 2);
    let error = fails(&after, delete_group("round", KernSide::First));
    assert!(matches!(error, EditError::NotFound(_)), "{error:?}");
}

#[test]
fn glyph_in_two_groups_same_side_is_refused() {
    let font = kern_font();
    let (next, _) = run(&font, set_group("O", KernSide::First, &["B", "o"]));
    let error = fails(&next, set_group("round", KernSide::First, &["V", "o"]));
    match error {
        EditError::InvalidArgument(message) => {
            assert!(
                message.contains("'o'") && message.contains("'O'"),
                "{message}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
    // Replacing the members of the same group with a glyph it already has
    // is fine; so is the other side.
    let (next, _) = run(&next, set_group("O", KernSide::First, &["o", "V"]));
    let (next, _) = run(&next, set_group("round", KernSide::Second, &["o"]));
    assert_eq!(
        next.kerning.group_of(id_of(&next, "o"), KernSide::Second),
        Some(group_id(&next, KernSide::Second, "round"))
    );
}

/// A document over `kern_font()` with the first-side group `O` (`A`, `B`)
/// and the pairs `A/V`, `V/A`, `@O/T` and `T/V`, committed by the user as
/// revision 1.
fn kerned_doc() -> Document {
    let mut doc = Document::new(kern_font());
    commit(
        &mut doc,
        &user(),
        "kern",
        vec![
            set_group("O", KernSide::First, &["A", "B"]),
            pair(g("A"), g("V"), -50.0),
            pair(g("V"), g("A"), -40.0),
            pair(k("O"), g("T"), -20.0),
            pair(g("T"), g("V"), -10.0),
        ],
    );
    doc
}

#[test]
fn delete_glyph_cascades_kerning_and_undo_restores() {
    let mut doc = kerned_doc();
    let before = doc.font().clone();
    let master = before.default_master().id;
    let (a, v, t) = (
        glyph_ref(&before, "A"),
        glyph_ref(&before, "V"),
        glyph_ref(&before, "T"),
    );
    let o = group_id(&before, KernSide::First, "O");

    let outcome = commit(
        &mut doc,
        &user(),
        "delete A",
        vec![Command::DeleteGlyphs {
            glyphs: vec![by_name("A")],
        }],
    );
    assert_eq!(
        outcome.touched,
        keys([
            EntityKey::GlyphOrder,
            EntityKey::Glyph(id_of(&before, "A")),
            EntityKey::KerningGroup(o),
            EntityKey::KerningPair(master, a, v),
            EntityKey::KerningPair(master, v, a),
        ])
    );
    let font = doc.font();
    assert_eq!(font.kerning.groups[&o].members, vec![id_of(&before, "B")]);
    assert_eq!(font.kerning.pair(master, a, v), None);
    assert_eq!(font.kerning.pair(master, v, a), None);
    assert_eq!(font.kerning.pair(master, KernRef::Group(o), t), Some(-20.0));
    assert_eq!(font.kerning.pair(master, t, v), Some(-10.0));

    doc.undo(&user()).expect("undo");
    assert_eq!(**doc.font(), *before);
    let redone = doc.redo(&user()).expect("redo");
    assert!(doc.font().glyph_by_name("A").is_none());
    // Reverting the deletion brings the glyph, its membership and its pairs
    // back too.
    doc.revert(&agent(), redone.tx).expect("revert");
    assert_eq!(doc.font().kerning, before.kerning);
    assert!(doc.font().glyph_by_name("A").is_some());
}

#[test]
fn pairs_by_two_actors_do_not_conflict() {
    let mut doc = Document::new(kern_font());
    let x = Actor::Agent("x".to_owned());
    doc.execute(&user(), Rev(0), "A/V", &[pair(g("A"), g("V"), -50.0)])
        .expect("user");
    doc.execute(&x, Rev(0), "T/o", &[pair(g("T"), g("o"), -70.0)])
        .expect("an older base revision is fine for another pair");
    let font = doc.font();
    let master = font.default_master().id;
    assert_eq!(
        font.kerning
            .pair(master, glyph_ref(font, "A"), glyph_ref(font, "V")),
        Some(-50.0)
    );
    assert_eq!(
        font.kerning
            .pair(master, glyph_ref(font, "T"), glyph_ref(font, "o")),
        Some(-70.0)
    );
    // Groups of different names do not conflict either.
    doc.execute(
        &user(),
        Rev(2),
        "O",
        &[set_group("O", KernSide::First, &["B"])],
    )
    .expect("user group");
    doc.execute(&x, Rev(2), "H", &[set_group("H", KernSide::First, &["A"])])
        .expect("agent group");
}

#[test]
fn same_pair_conflicts() {
    let mut doc = Document::new(kern_font());
    let key = pair_key(
        doc.font(),
        glyph_ref(doc.font(), "A"),
        glyph_ref(doc.font(), "V"),
    );
    doc.execute(&user(), Rev(0), "A/V", &[pair(g("A"), g("V"), -50.0)])
        .expect("user");
    let error = doc
        .execute(&agent(), Rev(0), "A/V", &[pair(g("A"), g("V"), -80.0)])
        .expect_err("the pair changed after revision 0");
    let info = conflict(error);
    assert_eq!(info.conflicts, vec![(key, Rev(1), user())]);
    // Removing it from the old revision conflicts as well.
    let error = doc
        .execute(&agent(), Rev(0), "A/V", &[pair(g("A"), g("V"), 0.0)])
        .expect_err("the pair changed after revision 0");
    assert_eq!(conflict(error).conflicts, vec![(key, Rev(1), user())]);
    // A group another actor changed after the base revision conflicts too.
    doc.execute(
        &user(),
        Rev(1),
        "O",
        &[set_group("O", KernSide::First, &["B"])],
    )
    .expect("user group");
    let error = doc
        .execute(
            &agent(),
            Rev(1),
            "O",
            &[set_group("O", KernSide::First, &["o"])],
        )
        .expect_err("the group changed after revision 1");
    let group = EntityKey::KerningGroup(group_id(doc.font(), KernSide::First, "O"));
    assert_eq!(conflict(error).conflicts, vec![(group, Rev(2), user())]);
}

#[test]
fn revert_of_an_added_pair_removes_it() {
    let mut doc = Document::new(kern_font());
    let added = doc
        .execute(&user(), Rev(0), "A/V", &[pair(g("A"), g("V"), -50.0)])
        .expect("user");
    doc.execute(&agent(), Rev(1), "T/o", &[pair(g("T"), g("o"), -70.0)])
        .expect("agent");
    let font = doc.font().clone();
    let master = font.default_master().id;

    let outcome = doc.revert(&agent(), added.tx).expect("revert");
    let (a, v) = (glyph_ref(&font, "A"), glyph_ref(&font, "V"));
    assert_eq!(
        outcome.touched,
        keys([EntityKey::KerningPair(master, a, v)])
    );
    let kerning = &doc.font().kerning;
    assert_eq!(kerning.pair(master, a, v), None);
    assert_eq!(
        kerning.pair(master, glyph_ref(&font, "T"), glyph_ref(&font, "o")),
        Some(-70.0)
    );

    // Reverting the only pair of a master leaves no trace of it.
    let mut doc = Document::new(kern_font());
    let added = doc
        .execute(&user(), Rev(0), "A/V", &[pair(g("A"), g("V"), -50.0)])
        .expect("user");
    doc.revert(&user(), added.tx).expect("revert");
    assert_eq!(doc.font().kerning, Kerning::default());
}

#[test]
fn restore_brings_back_a_deleted_group() {
    let mut doc = kerned_doc();
    let at_one = doc.font().clone();
    let o = group_id(&at_one, KernSide::First, "O");
    let t = glyph_ref(&at_one, "T");
    commit(
        &mut doc,
        &agent(),
        "drop O",
        vec![delete_group("O", KernSide::First)],
    );
    assert!(doc.font().kerning.groups.is_empty());

    let outcome = doc.restore(&user(), Rev(2), Rev(1)).expect("restore");
    assert_eq!(
        outcome.touched,
        keys([
            EntityKey::KerningGroup(o),
            pair_key(&at_one, KernRef::Group(o), t),
        ])
    );
    assert_eq!(doc.font().kerning, at_one.kerning);
    assert_eq!(**doc.font(), *at_one);
}

// ------------------------------------------------------------- properties

/// An abstract kerning edit; glyph and group indices pick from small pools,
/// so commands often hit the same pairs and groups, and sometimes fail.
#[derive(Clone, Debug)]
enum Op {
    Pair {
        first: Side,
        second: Side,
        value: i16,
    },
    Group {
        name: usize,
        side: bool,
        members: Vec<usize>,
    },
    DeleteGroup {
        name: usize,
        side: bool,
    },
    DeleteGlyph {
        glyph: usize,
    },
}

#[derive(Clone, Debug)]
enum Side {
    Glyph(usize),
    Group(usize),
}

const GLYPHS: [&str; 5] = ["A", "B", "V", "T", "o"];
const GROUPS: [&str; 3] = ["O", "H", "round"];

fn side_strategy() -> impl Strategy<Value = Side> {
    prop_oneof![
        3 => (0..GLYPHS.len()).prop_map(Side::Glyph),
        1 => (0..GROUPS.len()).prop_map(Side::Group),
    ]
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        5 => (side_strategy(), side_strategy(), -3i16..=3).prop_map(|(first, second, value)| {
            Op::Pair { first, second, value }
        }),
        3 => (0..GROUPS.len(), any::<bool>(), vec(0..GLYPHS.len(), 0..3)).prop_map(
            |(name, side, mut members)| {
                members.sort_unstable();
                members.dedup();
                Op::Group { name, side, members }
            }
        ),
        1 => (0..GROUPS.len(), any::<bool>()).prop_map(|(name, side)| Op::DeleteGroup { name, side }),
        1 => (0..GLYPHS.len()).prop_map(|glyph| Op::DeleteGlyph { glyph }),
    ]
}

fn kern_side(first: bool) -> KernSide {
    if first {
        KernSide::First
    } else {
        KernSide::Second
    }
}

fn side_input(side: &Side) -> KernRefInput {
    match side {
        Side::Glyph(index) => g(GLYPHS[*index]),
        Side::Group(index) => k(GROUPS[*index]),
    }
}

fn command(op: &Op) -> Command {
    match op {
        Op::Pair {
            first,
            second,
            value,
        } => pair(
            side_input(first),
            side_input(second),
            f64::from(*value) * 15.0,
        ),
        Op::Group {
            name,
            side,
            members,
        } => Command::SetKerningGroup {
            name: GROUPS[*name].to_owned(),
            side: kern_side(*side),
            members: members
                .iter()
                .map(|index| by_name(GLYPHS[*index]))
                .collect(),
        },
        Op::DeleteGroup { name, side } => delete_group(GROUPS[*name], kern_side(*side)),
        Op::DeleteGlyph { glyph } => Command::DeleteGlyphs {
            glyphs: vec![by_name(GLYPHS[*glyph])],
        },
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn property_n_kerning_commands_then_n_undos_restore(ops in vec(op_strategy(), 1..12)) {
        let start = kern_font();
        let mut doc = Document::new(start.clone());
        let mut states = vec![doc.font().clone()];
        for op in &ops {
            let base = doc.rev();
            // Commands that do not apply (an unknown group, a glyph already
            // in a group of that side) change nothing.
            if doc.execute(&user(), base, "op", &[command(op)]).is_ok() {
                doc.font().validate_invariants().expect("valid");
                states.push(doc.font().clone());
            } else {
                prop_assert_eq!(doc.rev(), base);
            }
        }
        let end = doc.font().clone();
        // Undo walks back through every state, to the initial one.
        for expected in states.iter().rev().skip(1) {
            doc.undo(&user()).expect("undo");
            prop_assert_eq!(&**doc.font(), &**expected);
        }
        prop_assert_eq!(&**doc.font(), &start);
        prop_assert!(doc.undo(&user()).is_err());
        // And redo walks forward again.
        for _ in 1..states.len() {
            doc.redo(&user()).expect("redo");
        }
        prop_assert_eq!(&**doc.font(), &*end);
    }
}
