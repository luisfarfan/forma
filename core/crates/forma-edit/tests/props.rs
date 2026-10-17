//! Property tests of documents: undo is an exact inverse of commit, edits of
//! disjoint glyphs never conflict, and editing a glyph another actor changed
//! after the base revision always does.

mod common;

use std::sync::Arc;

use common::*;
use forma_edit::{Command, Document, EditError, GlyphRef};
use forma_model::{Actor, EntityKey, Font, GlyphId, Rev};
use proptest::collection::vec;
use proptest::prelude::*;

const CASES: u32 = 1000;

/// Private-use code points that only the generated commands assign. Each
/// actor draws from its own pool, so actors never collide on a code point.
const SHARED_POOL: u32 = 0xE000;
const POOL_A: u32 = 0xE100;
const POOL_B: u32 = 0xE200;

/// A one-master font with five glyphs `a` to `e`, each a square.
fn five_glyph_font() -> Font {
    let mut font = Font::new("Props", 1000);
    for (index, name) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
        let offset = f64::from(u32::try_from(index).unwrap()) * 10.0;
        let codepoint = 0x61 + u32::try_from(index).unwrap();
        add_glyph(
            &mut font,
            name,
            &[codepoint],
            500.0,
            vec![square(0, 50.0 + offset, 0.0, 350.0 + offset, 500.0, true)],
        );
    }
    font
}

// ------------------------------------------------------------ generators

/// An abstract edit. Glyph indices are resolved, modulo the number of
/// candidates, against the glyphs an actor may edit when the edit runs.
#[derive(Clone, Debug)]
enum Op {
    Advance {
        glyph: usize,
        width: u16,
    },
    Outline {
        glyph: usize,
        x: u16,
        y: u16,
        w: u16,
        h: u16,
    },
    Unicodes {
        glyph: usize,
        slots: Vec<u8>,
    },
    Create {
        slot: Option<u8>,
    },
    Delete {
        glyph: usize,
    },
    XHeight {
        value: u16,
    },
    VersionMinor {
        value: u16,
    },
}

fn slots() -> impl Strategy<Value = Vec<u8>> {
    vec(0u8..16, 0..3).prop_map(|mut slots| {
        slots.sort_unstable();
        slots.dedup();
        slots
    })
}

/// Edits of glyphs only: they touch glyph entities and the glyph order.
fn glyph_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (any::<usize>(), 0u16..2000).prop_map(|(glyph, width)| Op::Advance { glyph, width }),
        3 => (any::<usize>(), 0u16..500, 0u16..500, 1u16..500, 1u16..500)
            .prop_map(|(glyph, x, y, w, h)| Op::Outline { glyph, x, y, w, h }),
        2 => (any::<usize>(), slots()).prop_map(|(glyph, slots)| Op::Unicodes { glyph, slots }),
        1 => proptest::option::of(0u8..16).prop_map(|slot| Op::Create { slot }),
        1 => any::<usize>().prop_map(|glyph| Op::Delete { glyph }),
    ]
}

/// Any edit, including font-wide ones.
fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        8 => glyph_op(),
        1 => (300u16..700).prop_map(|value| Op::XHeight { value }),
        1 => any::<u16>().prop_map(|value| Op::VersionMinor { value }),
    ]
}

/// Where an actor's generated commands draw fresh names and code points from.
struct Lane {
    prefix: &'static str,
    pool: u32,
    created: u32,
}

impl Lane {
    fn new(prefix: &'static str, pool: u32) -> Self {
        Self {
            prefix,
            pool,
            created: 0,
        }
    }
}

/// Turns an abstract edit into a command on one of `targets`; `None` when it
/// needs a glyph and there is none to edit.
fn to_command(op: &Op, targets: &[GlyphId], lane: &mut Lane) -> Option<Command> {
    let pick = |glyph: usize| -> Option<GlyphRef> {
        (!targets.is_empty()).then(|| GlyphRef::Id(targets[glyph % targets.len()]))
    };
    let cmd = match op {
        Op::Advance { glyph, width } => Command::SetMetrics {
            glyph: pick(*glyph)?,
            master: None,
            advance_width: Some(f64::from(*width)),
            lsb: None,
            rsb: None,
        },
        Op::Outline { glyph, x, y, w, h } => {
            let (x, y) = (f64::from(*x), f64::from(*y));
            Command::SetOutline {
                glyph: pick(*glyph)?,
                master: None,
                paths: vec![square(0, x, y, x + f64::from(*w), y + f64::from(*h), true)],
            }
        }
        Op::Unicodes { glyph, slots } => Command::UpdateGlyph {
            glyph: pick(*glyph)?,
            name: None,
            unicodes: Some(slots.iter().map(|&s| lane.pool + u32::from(s)).collect()),
            export: None,
        },
        Op::Create { slot } => {
            lane.created += 1;
            Command::CreateGlyph {
                name: format!("{}{}", lane.prefix, lane.created),
                unicodes: slot.iter().map(|&s| lane.pool + u32::from(s)).collect(),
                advance_width: 500.0,
                paths: vec![],
            }
        }
        Op::Delete { glyph } => Command::DeleteGlyphs {
            glyphs: vec![pick(*glyph)?],
        },
        Op::XHeight { value } => Command::SetVerticalMetrics {
            master: None,
            ascender: None,
            descender: None,
            x_height: Some(f64::from(*value)),
            cap_height: None,
        },
        Op::VersionMinor { value } => Command::SetFontInfo {
            family_name: None,
            style_name: None,
            version_major: None,
            version_minor: Some(*value),
            copyright: None,
            designer: None,
            units_per_em: None,
        },
    };
    Some(cmd)
}

/// Runs randomly generated transactions by `actor` on every glyph except
/// `spare`, each based on the current revision, skipping the ones that are
/// rejected. Returns how many committed.
fn run_txs(
    doc: &mut Document,
    actor: &Actor,
    txs: &[Vec<Op>],
    lane: &mut Lane,
    spare: Option<GlyphId>,
) -> u64 {
    let mut committed = 0;
    for tx in txs {
        let targets: Vec<GlyphId> = doc
            .font()
            .glyph_order
            .iter()
            .copied()
            .filter(|id| Some(*id) != spare)
            .collect();
        let cmds: Vec<Command> = tx
            .iter()
            .filter_map(|op| to_command(op, &targets, lane))
            .collect();
        if cmds.is_empty() {
            continue;
        }
        let base = doc.rev();
        if doc.execute(actor, base, "prefix", &cmds).is_ok() {
            committed += 1;
        }
    }
    committed
}

fn json(font: &Font) -> String {
    serde_json::to_string(font).expect("a font serializes")
}

/// A single-glyph edit for the overlap property.
#[derive(Clone, Debug)]
enum Edit {
    Advance(u16),
    Outline(u16, u16),
    Unicodes(Vec<u8>),
    Rename,
    Delete,
}

fn modifying_edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (0u16..2000).prop_map(Edit::Advance),
        (1u16..500, 1u16..500).prop_map(|(w, h)| Edit::Outline(w, h)),
        slots().prop_map(Edit::Unicodes),
        Just(Edit::Rename),
    ]
}

fn any_edit() -> impl Strategy<Value = Edit> {
    prop_oneof![4 => modifying_edit(), 1 => Just(Edit::Delete)]
}

fn edit_command(edit: &Edit, glyph: GlyphId, lane: &mut Lane) -> Command {
    let glyph = GlyphRef::Id(glyph);
    match edit {
        Edit::Advance(width) => Command::SetMetrics {
            glyph,
            master: None,
            advance_width: Some(f64::from(*width)),
            lsb: None,
            rsb: None,
        },
        Edit::Outline(w, h) => Command::SetOutline {
            glyph,
            master: None,
            paths: vec![square(0, 0.0, 0.0, f64::from(*w), f64::from(*h), true)],
        },
        Edit::Unicodes(slots) => Command::UpdateGlyph {
            glyph,
            name: None,
            unicodes: Some(slots.iter().map(|&s| lane.pool + u32::from(s)).collect()),
            export: None,
        },
        Edit::Rename => {
            lane.created += 1;
            Command::UpdateGlyph {
                glyph,
                name: Some(format!("{}{}", lane.prefix, lane.created)),
                unicodes: None,
                export: None,
            }
        }
        Edit::Delete => Command::DeleteGlyphs {
            glyphs: vec![glyph],
        },
    }
}

// ------------------------------------------------------------ properties

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    /// Committing N transactions and undoing them N times gives back the
    /// original font, walking back through every intermediate state; redoing
    /// them all gives back the final font.
    #[test]
    fn prop_n_commands_then_n_undos_is_identity(txs in vec(vec(op(), 1..=3), 1..=12)) {
        let mut doc = Document::new(five_glyph_font());
        let initial = Arc::clone(doc.font());
        let initial_json = json(&initial);
        let actor = Actor::User;
        let mut lane = Lane::new("new", SHARED_POOL);
        let mut states = vec![Arc::clone(&initial)];

        for tx in &txs {
            let targets = doc.font().glyph_order.clone();
            let cmds: Vec<Command> = tx
                .iter()
                .filter_map(|op| to_command(op, &targets, &mut lane))
                .collect();
            if cmds.is_empty() {
                continue;
            }
            let base = doc.rev();
            let before = Arc::clone(doc.font());
            match doc.execute(&actor, base, "prop", &cmds) {
                Ok(outcome) => {
                    prop_assert_eq!(outcome.rev, Rev(base.0 + 1));
                    states.push(Arc::clone(doc.font()));
                }
                Err(EditError::Conflict(info)) => {
                    prop_assert!(false, "an up-to-date base never conflicts: {:?}", info);
                }
                Err(_) => {
                    // A rejected transaction changes nothing.
                    prop_assert_eq!(doc.rev(), base);
                    prop_assert!(Arc::ptr_eq(doc.font(), &before));
                }
            }
        }

        let committed = u64::try_from(states.len() - 1).unwrap();
        let final_state = Arc::clone(doc.font());
        let final_json = json(&final_state);

        for expected in states.iter().rev().skip(1) {
            doc.undo(&actor).expect("the actor's own transaction can be undone");
            prop_assert_eq!(&**doc.font(), &**expected);
        }
        prop_assert_eq!(doc.undo(&actor), Err(EditError::NotAllowed("nothing to undo".to_owned())));
        prop_assert_eq!(&**doc.font(), &*initial);
        prop_assert_eq!(json(doc.font()), initial_json);
        prop_assert_eq!(doc.rev(), Rev(2 * committed));
        prop_assert!(doc.font().validate_invariants().is_ok());

        for _ in 0..committed {
            doc.redo(&actor).expect("the actor's own undone transaction can be redone");
        }
        prop_assert_eq!(&**doc.font(), &*final_state);
        prop_assert_eq!(json(doc.font()), final_json);
        prop_assert_eq!(doc.rev(), Rev(3 * committed));
    }

    /// Two actors that start from the same base revision and edit disjoint
    /// sets of glyphs, interleaved, never conflict. Each actor bases every
    /// transaction on the latest revision it has received (its starting base,
    /// then the revision of its own last transaction).
    #[test]
    fn prop_disjoint_edits_never_conflict(
        prefix in vec(vec(op(), 1..=2), 0..=3),
        owner_mask in vec(any::<bool>(), 8),
        steps in vec((any::<bool>(), vec(glyph_op(), 1..=2)), 1..=16),
    ) {
        let mut doc = Document::new(five_glyph_font());
        run_txs(&mut doc, &Actor::Cli, &prefix, &mut Lane::new("pre", SHARED_POOL), None);

        let shared_base = doc.rev();
        let actors = [Actor::Agent("alpha".to_owned()), Actor::User];
        let initial: Vec<Vec<GlyphId>> = [true, false]
            .into_iter()
            .map(|mine| {
                doc.font()
                    .glyph_order
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| owner_mask[index % owner_mask.len()] == mine)
                    .map(|(_, id)| *id)
                    .collect()
            })
            .collect();
        let mut lanes = [Lane::new("alpha", POOL_A), Lane::new("beta", POOL_B)];
        let mut bases = [shared_base, shared_base];

        for (first, ops) in &steps {
            let who = usize::from(!*first);
            let font = Arc::clone(doc.font());
            // The glyphs this actor owns: its initial share plus the ones it created.
            let targets: Vec<GlyphId> = font
                .glyph_order
                .iter()
                .copied()
                .filter(|id| {
                    initial[who].contains(id) || font.glyphs[id].name.starts_with(lanes[who].prefix)
                })
                .collect();
            let cmds: Vec<Command> = ops
                .iter()
                .filter_map(|op| to_command(op, &targets, &mut lanes[who]))
                .collect();
            if cmds.is_empty() {
                continue;
            }
            match doc.execute(&actors[who], bases[who], "disjoint", &cmds) {
                Ok(outcome) => bases[who] = outcome.rev,
                Err(EditError::Conflict(info)) => {
                    prop_assert!(false, "disjoint edits conflicted: {:?}", info);
                }
                Err(_) => {}
            }
        }
    }

    /// When actor A changes a glyph after actor B's base revision, any edit of
    /// that glyph by B based on that revision is a conflict naming the glyph,
    /// the revision and A, and changes nothing.
    #[test]
    fn prop_overlapping_edit_always_conflicts(
        prefix in vec(vec(op(), 1..=2), 0..=3),
        target in any::<usize>(),
        a_edit in modifying_edit(),
        a_more in vec(glyph_op(), 0..=2),
        b_edit in any_edit(),
    ) {
        let mut doc = Document::new(five_glyph_font());
        run_txs(&mut doc, &Actor::Cli, &prefix, &mut Lane::new("pre", SHARED_POOL), None);
        let order = doc.font().glyph_order.clone();
        prop_assume!(!order.is_empty());
        let glyph = order[target % order.len()];
        let b_base = doc.rev();

        let alice = Actor::Agent("alpha".to_owned());
        let mut a_lane = Lane::new("alpha", POOL_A);
        let base = doc.rev();
        let edited = doc
            .execute(&alice, base, "A edits", &[edit_command(&a_edit, glyph, &mut a_lane)])
            .expect("A's edit is valid");
        // A may go on editing other glyphs; that does not matter.
        run_txs(&mut doc, &alice, &[a_more], &mut Lane::new("alphamore", POOL_A + 0x80), Some(glyph));

        let rev_before = doc.rev();
        let font_before = Arc::clone(doc.font());
        let mut b_lane = Lane::new("beta", POOL_B);
        let cmd = edit_command(&b_edit, glyph, &mut b_lane);
        match doc.execute(&Actor::User, b_base, "B edits", &[cmd]) {
            Err(EditError::Conflict(info)) => {
                prop_assert_eq!(info.current_rev, rev_before);
                let entry = info
                    .conflicts
                    .iter()
                    .find(|(entity, _, _)| *entity == EntityKey::Glyph(glyph));
                prop_assert_eq!(entry, Some(&(EntityKey::Glyph(glyph), edited.rev, alice.clone())));
                prop_assert!(info.conflicts.iter().all(|(entity, _, _)| *entity != EntityKey::GlyphOrder));
            }
            other => prop_assert!(false, "expected a conflict, got {:?}", other),
        }
        prop_assert_eq!(doc.rev(), rev_before);
        prop_assert!(Arc::ptr_eq(doc.font(), &font_before));
    }
}
