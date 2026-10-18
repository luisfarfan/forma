//! The crash-recovery journal: appending, replaying (also after a crash in
//! the middle of an append), truncating and applying records to a font.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forma_model::{
    Actor, Font, FontInfo, Glyph, GlyphId, KernRef, KernSide, Kerning, KerningGroup,
    KerningGroupId, Layer, Master, MasterId, Node, NodeKind, Path as Contour, PathId, Rev,
    VerticalMetrics,
};
use forma_store::{
    Journal, JournalKey, JournalRecord, StoreError, apply_records, remove_journal, replay,
};
use proptest::collection::vec;
use proptest::prelude::*;
use tempfile::TempDir;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn gid(n: u128) -> GlyphId {
    GlyphId(Uuid::from_u128(n))
}

fn corner(x: f64, y: f64) -> Node {
    Node {
        x,
        y,
        kind: NodeKind::Line,
        smooth: false,
    }
}

/// A glyph with one triangle in the default master of `font`.
fn glyph_in(font: &Font, n: u128, name: &str, advance_width: f64, apex: f64) -> Glyph {
    Glyph {
        id: gid(n),
        name: name.to_owned(),
        unicodes: vec![0x100 + u32::try_from(n).unwrap_or(0)],
        export: true,
        layers: [(
            font.default_master().id,
            Layer {
                advance_width,
                paths: vec![Contour {
                    id: PathId(1),
                    closed: true,
                    nodes: vec![corner(0.0, 0.0), corner(500.0, 0.0), corner(250.0, apex)],
                }],
                components: Vec::new(),
            },
        )]
        .into(),
        provenance: None,
    }
}

fn actor_for(n: usize) -> Actor {
    match n % 3 {
        0 => Actor::User,
        1 => Actor::Cli,
        _ => Actor::Agent("claude-code".to_owned()),
    }
}

/// What a transaction touched.
#[derive(Default)]
struct Touched {
    info: bool,
    masters: bool,
    order: bool,
    glyphs: Vec<GlyphId>,
}

/// A stand-in for the engine: edits a font and records, after each commit,
/// the resulting state of the entities it touched.
struct Sim {
    font: Font,
    records: Vec<JournalRecord>,
}

impl Sim {
    fn new(font: Font) -> Sim {
        Sim {
            font,
            records: Vec::new(),
        }
    }

    fn commit(&mut self, label: &str, touched: &Touched, edit: impl FnOnce(&mut Font)) {
        edit(&mut self.font);
        let font = &self.font;
        self.records.push(JournalRecord {
            document: DOCUMENT.to_owned(),
            rev: Rev(self.records.len() as u64 + 1),
            actor: actor_for(self.records.len()),
            label: label.to_owned(),
            font_info: touched.info.then(|| font.info.clone()),
            units_per_em: touched.info.then_some(font.units_per_em),
            masters: touched.masters.then(|| font.masters.clone()),
            glyph_order: touched.order.then(|| font.glyph_order.clone()),
            glyphs: touched
                .glyphs
                .iter()
                .map(|id| (*id, font.glyphs.get(id).map(|glyph| (**glyph).clone())))
                .collect(),
            kerning: None,
        });
    }

    fn add_glyph(&mut self, glyph: Glyph) {
        let id = glyph.id;
        let touched = Touched {
            order: true,
            glyphs: vec![id],
            ..Touched::default()
        };
        self.commit("add glyph", &touched, |font| {
            Arc::make_mut(&mut font.glyphs).insert(id, Arc::new(glyph));
            font.glyph_order.push(id);
        });
    }

    fn replace_glyph(&mut self, glyph: Glyph) {
        let id = glyph.id;
        let touched = Touched {
            glyphs: vec![id],
            ..Touched::default()
        };
        self.commit("edit glyph", &touched, |font| {
            Arc::make_mut(&mut font.glyphs).insert(id, Arc::new(glyph));
        });
    }

    fn delete_glyph(&mut self, id: GlyphId) {
        let touched = Touched {
            order: true,
            glyphs: vec![id],
            ..Touched::default()
        };
        self.commit("delete glyph", &touched, |font| {
            Arc::make_mut(&mut font.glyphs).remove(&id);
            font.glyph_order.retain(|listed| *listed != id);
        });
    }
}

/// The document path of the journals of most tests.
const DOCUMENT: &str = "/fonts/test.forma";

/// The key of the journal of `font` in [`DOCUMENT`].
fn key(font: &Font) -> JournalKey {
    JournalKey::new(font.id, Path::new(DOCUMENT))
}

/// A record of `key`'s document that changes nothing.
fn record_for(key: &JournalKey, rev: u64, label: &str) -> JournalRecord {
    JournalRecord {
        document: key.document_text(),
        ..empty_record(rev, label)
    }
}

/// A record that changes nothing, for tests about the file and not the font.
fn empty_record(rev: u64, label: &str) -> JournalRecord {
    JournalRecord {
        document: DOCUMENT.to_owned(),
        rev: Rev(rev),
        actor: Actor::User,
        label: label.to_owned(),
        font_info: None,
        units_per_em: None,
        masters: None,
        glyph_order: None,
        glyphs: Vec::new(),
        kerning: None,
    }
}

fn journal_file(dir: &Path, font: &Font) -> PathBuf {
    dir.join(key(font).file_name())
}

/// A font with two glyphs, `a` and `b`.
fn base_font() -> Font {
    let mut font = Font::new("Journal Test", 1000);
    let a = glyph_in(&font, 1, "a", 500.0, 700.0);
    let b = glyph_in(&font, 2, "b", 600.0, 710.0);
    let glyphs = Arc::make_mut(&mut font.glyphs);
    glyphs.insert(a.id, Arc::new(a));
    glyphs.insert(b.id, Arc::new(b));
    font.glyph_order = vec![gid(1), gid(2)];
    font
}

fn append_all(journal: &mut Journal, records: &[JournalRecord]) {
    for record in records {
        journal.append(record).unwrap();
    }
}

// ---------------------------------------------------------------------------
// The three tests of the brief
// ---------------------------------------------------------------------------

#[test]
fn replay_restores_state() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut sim = Sim::new(base.clone());

    // Ten transactions that between them add, edit, rename, reorder and
    // delete glyphs and change the font information and the masters.
    let c = glyph_in(&sim.font, 3, "c", 520.0, 650.0);
    sim.add_glyph(c);
    let d = glyph_in(&sim.font, 4, "d", 530.0, 660.0);
    sim.add_glyph(d);
    let edited_a = glyph_in(&sim.font, 1, "a", 505.5, 701.25);
    sim.replace_glyph(edited_a);
    let mut renamed_b = glyph_in(&sim.font, 2, "b.alt", 600.0, 710.0);
    renamed_b.unicodes.clear();
    sim.replace_glyph(renamed_b);
    sim.delete_glyph(gid(3));
    sim.commit(
        "reorder",
        &Touched {
            order: true,
            ..Touched::default()
        },
        |font| font.glyph_order.reverse(),
    );
    sim.commit(
        "font info",
        &Touched {
            info: true,
            ..Touched::default()
        },
        |font| {
            font.info.family_name = "Journal Test \u{201c}2\u{201d}".to_owned();
            font.info.version_minor = 7;
            font.info.designer = Some("Ana".to_owned());
        },
    );
    sim.commit(
        "second master",
        &Touched {
            masters: true,
            ..Touched::default()
        },
        |font| {
            font.masters.push(Master {
                id: MasterId(Uuid::from_u128(99)),
                name: "Bold".to_owned(),
                metrics: VerticalMetrics {
                    ascender: 810.0,
                    descender: -190.0,
                    x_height: 520.0,
                    cap_height: 710.0,
                },
            });
        },
    );
    // One transaction touching everything at once: add a glyph and delete one
    // while changing the font information and the order.
    let e = glyph_in(&sim.font, 5, "e", 540.0, 670.0);
    sim.commit(
        "everything",
        &Touched {
            info: true,
            masters: true,
            order: true,
            glyphs: vec![gid(5), gid(1)],
        },
        |font| {
            font.info.style_name = "Book".to_owned();
            font.masters[0].name = "Book".to_owned();
            Arc::make_mut(&mut font.glyphs).insert(e.id, Arc::new(e));
            Arc::make_mut(&mut font.glyphs).remove(&gid(1));
            font.glyph_order = vec![gid(4), gid(5), gid(2)];
        },
    );
    let f = glyph_in(&sim.font, 6, "f", 550.0, 680.0);
    sim.add_glyph(f);
    assert_eq!(sim.records.len(), 10);

    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    append_all(&mut journal, &sim.records);
    drop(journal);

    let replayed = replay(dir.path(), &key(&base)).unwrap();
    assert_eq!(replayed, sim.records);
    assert_eq!(apply_records(base, &replayed).unwrap(), sim.font);
}

#[test]
fn truncated_last_line_is_ignored() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut sim = Sim::new(base.clone());
    for n in 3..6 {
        let glyph = glyph_in(&sim.font, n, &format!("g{n}"), 500.0, 700.0);
        sim.add_glyph(glyph);
    }
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    append_all(&mut journal, &sim.records);
    drop(journal);
    let path = journal_file(dir.path(), &base);
    let committed = fs::read(&path).unwrap();

    // A crash can stop the fourth append after any number of bytes, even in
    // the middle of a multi-byte character.
    let mut next = empty_record(4, "a label with accents \u{e9}\u{e8}\u{4e2d}");
    next.font_info = Some(base.info.clone());
    let line = {
        let scratch = TempDir::new().unwrap();
        let mut journal = Journal::open(scratch.path(), &key(&base)).unwrap();
        journal.append(&next).unwrap();
        drop(journal);
        fs::read(journal_file(scratch.path(), &base)).unwrap()
    };
    assert!(line.ends_with(b"\n"));

    // Every proper prefix of the line, but not the line without its newline
    // (that one is a complete record, see `open_completes_a_record_that_lost_its_newline`).
    for cut in 0..line.len() - 1 {
        let mut bytes = committed.clone();
        bytes.extend_from_slice(&line[..cut]);
        fs::write(&path, &bytes).unwrap();

        let replayed = replay(dir.path(), &key(&base)).unwrap();

        assert_eq!(replayed, sim.records, "cut after {cut} bytes");
    }
}

#[test]
fn truncate_empties_journal() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    append_all(
        &mut journal,
        &[empty_record(1, "one"), empty_record(2, "two")],
    );
    assert!(
        !fs::read(journal_file(dir.path(), &base))
            .unwrap()
            .is_empty()
    );

    journal.truncate().unwrap();

    assert_eq!(
        fs::metadata(journal_file(dir.path(), &base)).unwrap().len(),
        0
    );
    assert!(replay(dir.path(), &key(&base)).unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

#[test]
fn missing_journal_replays_empty() {
    let dir = TempDir::new().unwrap();

    assert!(replay(dir.path(), &key(&base_font())).unwrap().is_empty());
    // Not even the folder exists.
    assert!(
        replay(&dir.path().join("nowhere"), &key(&base_font()))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn corrupt_middle_line_is_an_error() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(1, "one")).unwrap();
    drop(journal);
    let path = journal_file(dir.path(), &base);
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(b"{\"rev\": 2, this is not json\n").unwrap();
    drop(file);
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(3, "three")).unwrap();
    drop(journal);

    let err = replay(dir.path(), &key(&base)).unwrap_err();

    assert!(
        matches!(&err, StoreError::Invalid(message) if message == "journal corrupt at line 2"),
        "{err:?}"
    );
}

#[test]
fn a_bad_line_followed_only_by_blank_lines_is_a_torn_tail() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(1, "one")).unwrap();
    drop(journal);
    let path = journal_file(dir.path(), &base);
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(b"{\"rev\": 2, tor\n\n  \n").unwrap();
    drop(file);

    let replayed = replay(dir.path(), &key(&base)).unwrap();

    assert_eq!(replayed, [empty_record(1, "one")]);
}

#[test]
fn corruption_in_the_first_line_is_reported_with_its_number() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(2, "two")).unwrap();
    drop(journal);
    let path = journal_file(dir.path(), &base);
    let good = fs::read(&path).unwrap();
    let mut bytes = b"\x00\xff garbage\n".to_vec();
    bytes.extend_from_slice(&good);
    fs::write(&path, bytes).unwrap();

    let err = replay(dir.path(), &key(&base)).unwrap_err();

    assert!(
        matches!(&err, StoreError::Invalid(message) if message == "journal corrupt at line 1"),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

#[test]
fn appends_after_truncate_work() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    append_all(
        &mut journal,
        &[empty_record(1, "one"), empty_record(2, "two")],
    );
    journal.truncate().unwrap();

    append_all(
        &mut journal,
        &[empty_record(3, "three"), empty_record(4, "four")],
    );
    drop(journal);

    let replayed = replay(dir.path(), &key(&base)).unwrap();
    assert_eq!(
        replayed,
        [empty_record(3, "three"), empty_record(4, "four")],
        "nothing from before the truncate may come back"
    );
}

#[test]
fn reopening_keeps_appending() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(1, "one")).unwrap();
    drop(journal);

    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(2, "two")).unwrap();
    drop(journal);

    assert_eq!(
        replay(dir.path(), &key(&base)).unwrap(),
        [empty_record(1, "one"), empty_record(2, "two")]
    );
}

#[test]
fn open_creates_the_folder() {
    let dir = TempDir::new().unwrap();
    let nested = dir.path().join("recovery").join("fonts");
    let base = base_font();

    let mut journal = Journal::open(&nested, &key(&base)).unwrap();
    journal.append(&empty_record(1, "one")).unwrap();

    assert!(journal_file(&nested, &base).is_file());
}

#[test]
fn each_font_has_its_own_journal() {
    let dir = TempDir::new().unwrap();
    let first = base_font();
    let second = base_font();
    let mut first_journal = Journal::open(dir.path(), &key(&first)).unwrap();
    let mut second_journal = Journal::open(dir.path(), &key(&second)).unwrap();

    first_journal.append(&empty_record(1, "first")).unwrap();
    second_journal.append(&empty_record(1, "second")).unwrap();

    assert_eq!(
        replay(dir.path(), &key(&first)).unwrap(),
        [empty_record(1, "first")]
    );
    assert_eq!(
        replay(dir.path(), &key(&second)).unwrap(),
        [empty_record(1, "second")]
    );
}

#[test]
fn lines_are_compact_canonical_json() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    let mut with_label = empty_record(1, "line one\nline \"two\"");
    with_label.actor = Actor::Agent("claude-code".to_owned());
    journal.append(&with_label).unwrap();
    journal.append(&empty_record(2, "x")).unwrap();
    drop(journal);

    let text = fs::read_to_string(journal_file(dir.path(), &base)).unwrap();

    assert_eq!(
        text,
        concat!(
            r#"{"actor":"agent:claude-code","document":"/fonts/test.forma","font_info":null,"#,
            r#""glyph_order":null,"glyphs":[],"label":"line one\nline \"two\"","masters":null,"#,
            r#""rev":1}"#,
            "\n",
            r#"{"actor":"user","document":"/fonts/test.forma","font_info":null,"#,
            r#""glyph_order":null,"glyphs":[],"label":"x","masters":null,"rev":2}"#,
            "\n",
        )
    );
}

#[test]
fn glyph_changes_are_pairs_of_id_and_glyph_or_null() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut record = empty_record(1, "pairs");
    record.glyphs = vec![
        (gid(1), Some(glyph_in(&base, 1, "a", 500.0, 700.0))),
        (gid(2), None),
    ];
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&record).unwrap();
    drop(journal);

    let text = fs::read_to_string(journal_file(dir.path(), &base)).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();

    let glyphs = value["glyphs"].as_array().unwrap();
    assert_eq!(glyphs.len(), 2);
    assert_eq!(glyphs[0][0], gid(1).to_string());
    assert_eq!(glyphs[0][1]["name"], "a");
    assert_eq!(glyphs[1], serde_json::json!([gid(2).to_string(), null]));
}

#[test]
fn a_non_finite_number_is_refused_and_nothing_is_written() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(1, "fine")).unwrap();
    let before = fs::read(journal_file(dir.path(), &base)).unwrap();

    let mut bad_glyph = empty_record(2, "nan coordinate");
    bad_glyph.glyphs = vec![(gid(7), Some(glyph_in(&base, 7, "g", 500.0, f64::NAN)))];
    let mut bad_master = empty_record(3, "infinite metric");
    let mut masters = base.masters.clone();
    masters[0].metrics.ascender = f64::INFINITY;
    bad_master.masters = Some(masters);

    for record in [bad_glyph, bad_master] {
        let err = journal.append(&record).unwrap_err();
        assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    }

    assert_eq!(fs::read(journal_file(dir.path(), &base)).unwrap(), before);
    // The journal is still usable.
    journal.append(&empty_record(4, "after")).unwrap();
    drop(journal);
    assert_eq!(replay(dir.path(), &key(&base)).unwrap().len(), 2);
}

#[test]
fn remove_deletes_the_journal() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(1, "one")).unwrap();
    drop(journal);

    remove_journal(dir.path(), &key(&base)).unwrap();

    assert!(!journal_file(dir.path(), &base).exists());
    assert!(replay(dir.path(), &key(&base)).unwrap().is_empty());
    // Removing what is not there is not an error.
    remove_journal(dir.path(), &key(&base)).unwrap();
    remove_journal(&dir.path().join("nowhere"), &key(&base)).unwrap();
}

// ---------------------------------------------------------------------------
// Appending after a crash
// ---------------------------------------------------------------------------

#[test]
fn open_after_a_torn_append_does_not_glue_the_next_record_to_it() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(1, "one")).unwrap();
    drop(journal);
    let path = journal_file(dir.path(), &base);
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(br#"{"actor":"user","font_info":nu"#)
        .unwrap();
    drop(file);

    // The session after the crash keeps working on the same journal.
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(2, "two")).unwrap();
    journal.append(&empty_record(3, "three")).unwrap();
    drop(journal);

    assert_eq!(
        replay(dir.path(), &key(&base)).unwrap(),
        [
            empty_record(1, "one"),
            empty_record(2, "two"),
            empty_record(3, "three")
        ]
    );
}

#[test]
fn open_completes_a_record_that_lost_its_newline() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(1, "one")).unwrap();
    journal.append(&empty_record(2, "two")).unwrap();
    drop(journal);
    let path = journal_file(dir.path(), &base);
    // Only the final newline of the second record never reached the disk.
    let mut bytes = fs::read(&path).unwrap();
    assert_eq!(bytes.pop(), Some(b'\n'));
    fs::write(&path, &bytes).unwrap();
    assert_eq!(replay(dir.path(), &key(&base)).unwrap().len(), 2);

    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&empty_record(3, "three")).unwrap();
    drop(journal);

    assert_eq!(
        replay(dir.path(), &key(&base)).unwrap(),
        [
            empty_record(1, "one"),
            empty_record(2, "two"),
            empty_record(3, "three")
        ]
    );
}

#[test]
fn open_drops_a_torn_first_line() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let path = journal_file(dir.path(), &base);
    fs::write(&path, b"{\"actor\":\"us").unwrap();

    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    journal.append(&empty_record(1, "one")).unwrap();
    drop(journal);

    assert_eq!(
        replay(dir.path(), &key(&base)).unwrap(),
        [empty_record(1, "one")]
    );
}

// ---------------------------------------------------------------------------
// apply_records
// ---------------------------------------------------------------------------

#[test]
fn apply_records_without_records_returns_the_base() {
    let base = base_font();

    assert_eq!(apply_records(base.clone(), &[]).unwrap(), base);
}

#[test]
fn applying_the_same_records_twice_gives_the_same_font() {
    let base = base_font();
    let mut sim = Sim::new(base.clone());
    let c = glyph_in(&sim.font, 3, "c", 520.0, 650.0);
    sim.add_glyph(c);
    sim.delete_glyph(gid(1));
    let edited = glyph_in(&sim.font, 2, "b", 611.0, 712.0);
    sim.replace_glyph(edited);

    let once = apply_records(base, &sim.records).unwrap();
    let twice = apply_records(once.clone(), &sim.records).unwrap();

    assert_eq!(once, sim.font);
    assert_eq!(twice, sim.font);
}

#[test]
fn apply_records_rejects_inconsistent_result() {
    let base = base_font();

    // The order lists a glyph that the same record removes.
    let mut dangling_order = empty_record(1, "delete without fixing the order");
    dangling_order.glyphs = vec![(gid(2), None)];
    // The glyph map and the order disagree the other way round.
    let mut unlisted = empty_record(1, "add without listing");
    unlisted.glyphs = vec![(gid(3), Some(glyph_in(&base, 3, "c", 500.0, 700.0)))];
    // A glyph stored under an id that is not its own.
    let mut wrong_key = empty_record(1, "wrong key");
    wrong_key.glyphs = vec![(gid(9), Some(glyph_in(&base, 3, "c", 500.0, 700.0)))];
    wrong_key.glyph_order = Some(vec![gid(1), gid(2), gid(9)]);
    // A name that is already taken.
    let mut duplicate_name = empty_record(1, "duplicate name");
    duplicate_name.glyphs = vec![(gid(3), Some(glyph_in(&base, 3, "a", 500.0, 700.0)))];
    duplicate_name.glyph_order = Some(vec![gid(1), gid(2), gid(3)]);
    // No masters left.
    let mut no_masters = empty_record(1, "no masters");
    no_masters.masters = Some(Vec::new());

    for (what, record) in [
        ("dangling order", dangling_order),
        ("unlisted glyph", unlisted),
        ("wrong key", wrong_key),
        ("duplicate name", duplicate_name),
        ("no masters", no_masters),
    ] {
        let err = apply_records(base.clone(), &[record]).unwrap_err();
        assert!(matches!(err, StoreError::Invalid(_)), "{what}: {err:?}");
    }
}

#[test]
fn apply_records_checks_the_final_state_only() {
    let base = base_font();
    // The first record leaves the order dangling; the second repairs it.
    let mut delete = empty_record(1, "delete");
    delete.glyphs = vec![(gid(2), None)];
    let mut fix_order = empty_record(2, "fix order");
    fix_order.glyph_order = Some(vec![gid(1)]);

    let font = apply_records(base, &[delete, fix_order]).unwrap();

    assert_eq!(font.glyph_order, [gid(1)]);
    assert!(!font.glyphs.contains_key(&gid(2)));
}

#[test]
fn apply_records_replaces_font_info() {
    let base = base_font();
    let mut record = empty_record(1, "info");
    record.font_info = Some(FontInfo {
        family_name: "Other".to_owned(),
        style_name: "Italic".to_owned(),
        version_major: 2,
        version_minor: 3,
        copyright: Some("(c) someone".to_owned()),
        designer: None,
    });

    let font = apply_records(base.clone(), std::slice::from_ref(&record)).unwrap();

    assert_eq!(Some(&font.info), record.font_info.as_ref());
    assert_eq!(font.glyphs, base.glyphs);
    assert_eq!(font.masters, base.masters);
}

#[test]
fn units_per_em_is_journaled_and_older_lines_still_read() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut record = empty_record(1, "upm");
    record.font_info = Some(base.info.clone());
    record.units_per_em = Some(2048);
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&record).unwrap();
    journal.append(&empty_record(2, "none")).unwrap();
    drop(journal);

    let text = fs::read_to_string(journal_file(dir.path(), &base)).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].contains(r#""units_per_em":2048"#), "{}", lines[0]);
    // A record that does not change it leaves the field out, as records
    // written before the field existed do.
    assert!(!lines[1].contains("units_per_em"), "{}", lines[1]);

    let records = replay(dir.path(), &key(&base)).unwrap();
    assert_eq!(records[0].units_per_em, Some(2048));
    assert_eq!(records[1].units_per_em, None);
    let font = apply_records(base.clone(), &records).unwrap();
    assert_eq!(font.units_per_em, 2048);
    assert_eq!(font.info, base.info);
    let unchanged = apply_records(base.clone(), &records[1..]).unwrap();
    assert_eq!(unchanged.units_per_em, base.units_per_em);
}

/// The kerning of `base_font()`: the first-side group `a` with glyph `a`,
/// and the pair `a`/`b` of `value`.
fn kerning_of(font: &Font, value: f64) -> Kerning {
    let group = KerningGroupId(Uuid::from_u128(0x6a));
    let mut kerning = Kerning::default();
    kerning.groups_mut().insert(
        group,
        KerningGroup {
            id: group,
            name: "a".to_owned(),
            side: KernSide::First,
            members: vec![gid(1)],
        },
    );
    kerning.set_pair(
        font.default_master().id,
        KernRef::Group(group),
        KernRef::Glyph(gid(2)),
        Some(value),
    );
    kerning
}

#[test]
fn kerning_is_journaled_and_older_lines_still_read() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut record = empty_record(1, "kern");
    record.kerning = Some(kerning_of(&base, -45.0));
    let mut cleared = empty_record(3, "no kerning");
    cleared.kerning = Some(Kerning::default());
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    journal.append(&record).unwrap();
    journal.append(&empty_record(2, "none")).unwrap();
    journal.append(&cleared).unwrap();
    drop(journal);

    let text = fs::read_to_string(journal_file(dir.path(), &base)).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(
        first["kerning"]["pairs"][base.default_master().id.to_string()][0]["value"],
        -45.0
    );
    // A record that does not touch the kerning leaves the field out, as
    // records written before the field existed do.
    assert!(!lines[1].contains("kerning"), "{}", lines[1]);
    assert!(
        lines[2].contains(r#""kerning":{"groups":[],"pairs":{}}"#),
        "{}",
        lines[2]
    );

    let records = replay(dir.path(), &key(&base)).unwrap();
    assert_eq!(records[0], record);
    assert_eq!(records[1].kerning, None);
    assert_eq!(records[2], cleared);

    // A record with kerning replaces the font's; one without leaves it.
    let font = apply_records(base.clone(), &records[..2]).unwrap();
    assert_eq!(font.kerning, kerning_of(&base, -45.0));
    assert_eq!(font.glyphs, base.glyphs);
    let font = apply_records(base.clone(), &records).unwrap();
    assert!(font.kerning.is_empty());
    assert_eq!(font, base);
}

#[test]
fn kerning_records_are_checked() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
    let mut bad = empty_record(1, "nan kerning");
    bad.kerning = Some(kerning_of(&base, f64::NAN));
    let err = journal.append(&bad).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    assert_eq!(replay(dir.path(), &key(&base)).unwrap().len(), 0);

    // Kerning that breaks a model rule does not apply.
    let mut zero = empty_record(1, "zero pair");
    zero.kerning = Some(kerning_of(&base, 0.0));
    let err = apply_records(base, &[zero]).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
}

// ---------------------------------------------------------------------------
// Property: whatever the edits, replaying the journal restores the font
// ---------------------------------------------------------------------------

fn finite() -> impl Strategy<Value = f64> {
    prop_oneof![
        3 => -2000.0f64..2000.0,
        // Every bit pattern of a finite double, including subnormals and zeros.
        1 => any::<f64>().prop_filter("finite", |v| v.is_finite()),
        1 => (-4000i32..4000).prop_map(|n| f64::from(n) / 8.0),
    ]
}

/// One edit: a kind, which glyph it applies to, and two numbers.
type Step = (u8, usize, f64, f64);

fn step() -> impl Strategy<Value = Step> {
    (0u8..6, any::<usize>(), finite(), finite())
}

/// Plays `steps` on `sim`, one commit each.
fn play(sim: &mut Sim, steps: &[Step]) {
    for (n, &(kind, pick, a, b)) in (1u128..).zip(steps) {
        let ids: Vec<GlyphId> = sim.font.glyph_order.clone();
        let target = (!ids.is_empty()).then(|| ids[pick % ids.len()]);
        match (kind, target) {
            // Additions are the most common edit.
            (0 | 1, _) | (_, None) => {
                let glyph = glyph_in(&sim.font, 1000 + n, &format!("g{n}"), a, b);
                sim.add_glyph(glyph);
            }
            (2, Some(id)) => {
                let mut glyph = (*sim.font.glyphs[&id]).clone();
                for layer in glyph.layers.values_mut() {
                    layer.advance_width = a;
                    if let Some(node) = layer.paths.iter_mut().flat_map(|p| &mut p.nodes).next() {
                        node.x = b;
                    }
                }
                sim.replace_glyph(glyph);
            }
            (3, Some(id)) => sim.delete_glyph(id),
            (4, Some(_)) => sim.commit(
                "font info",
                &Touched {
                    info: true,
                    ..Touched::default()
                },
                |font| {
                    font.info.family_name = format!("Family \u{e9} {n}");
                    font.info.copyright = Some(format!("(c) {a}"));
                },
            ),
            (_, Some(id)) => {
                // Rename and move to the front in one transaction.
                let mut glyph = (*sim.font.glyphs[&id]).clone();
                glyph.name = format!("renamed{n}");
                let touched = Touched {
                    order: true,
                    glyphs: vec![id],
                    ..Touched::default()
                };
                sim.commit("rename and move", &touched, |font| {
                    Arc::make_mut(&mut font.glyphs).insert(id, Arc::new(glyph));
                    font.glyph_order.retain(|listed| *listed != id);
                    font.glyph_order.insert(0, id);
                });
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn records_roundtrip_through_journal(steps in vec(step(), 1..40)) {
        let dir = TempDir::new().unwrap();
        let base = base_font();
        let mut sim = Sim::new(base.clone());
        play(&mut sim, &steps);

        let mut journal = Journal::open(dir.path(), &key(&base)).unwrap();
        append_all(&mut journal, &sim.records);
        drop(journal);
        let replayed = replay(dir.path(), &key(&base)).unwrap();

        prop_assert_eq!(&replayed, &sim.records);
        let restored = apply_records(base, &replayed).unwrap();
        prop_assert_eq!(&restored, &sim.font);
        restored.validate_invariants().unwrap();
    }
}

// ---------------------------------------------------------------------------
// Journals belong to one document file
// ---------------------------------------------------------------------------

#[test]
fn journals_are_keyed_by_font_and_document_path() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    // Two copies of the same font (same id) in two files.
    let first = JournalKey::new(base.id, Path::new("/fonts/first.forma"));
    let copy = JournalKey::new(base.id, Path::new("/fonts/copy.forma"));
    assert_ne!(first.file_name(), copy.file_name());
    let name = first.file_name();
    let (font_part, rest) = name.split_at(36);
    assert_eq!(font_part, base.id.to_string());
    let hash = rest
        .strip_prefix('-')
        .and_then(|rest| rest.strip_suffix(".journal"))
        .expect("<font id>-<hash>.journal");
    assert_eq!(hash.len(), 16, "{name}");
    assert!(
        hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "{name}"
    );
    assert_eq!(first.document(), Path::new("/fonts/first.forma"));
    assert_eq!(first.font_id(), base.id);

    let mut first_journal = Journal::open(dir.path(), &first).unwrap();
    let mut copy_journal = Journal::open(dir.path(), &copy).unwrap();
    first_journal
        .append(&record_for(&first, 1, "first"))
        .unwrap();
    copy_journal.append(&record_for(&copy, 1, "copy")).unwrap();
    drop((first_journal, copy_journal));

    assert_eq!(
        replay(dir.path(), &first).unwrap(),
        [record_for(&first, 1, "first")]
    );
    assert_eq!(
        replay(dir.path(), &copy).unwrap(),
        [record_for(&copy, 1, "copy")]
    );
    remove_journal(dir.path(), &copy).unwrap();
    assert!(dir.path().join(first.file_name()).is_file());
    assert!(!dir.path().join(copy.file_name()).exists());
}

#[test]
fn records_name_their_document_and_replay_checks_it() {
    let dir = TempDir::new().unwrap();
    let base = base_font();
    let mine = JournalKey::new(base.id, Path::new("/fonts/mine.forma"));
    let other = JournalKey::new(base.id, Path::new("/fonts/other.forma"));
    let mut journal = Journal::open(dir.path(), &mine).unwrap();

    // A record of another document is refused, and nothing is written.
    let err = journal
        .append(&record_for(&other, 1, "not mine"))
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    journal.append(&record_for(&mine, 1, "mine")).unwrap();
    drop(journal);
    let text = fs::read_to_string(dir.path().join(mine.file_name())).unwrap();
    assert!(text.contains(r#""document":"/fonts/mine.forma""#), "{text}");

    // A journal file that holds the records of another document (copied or
    // renamed by hand) is never offered as this document's.
    fs::copy(
        dir.path().join(mine.file_name()),
        dir.path().join(other.file_name()),
    )
    .unwrap();
    let err = replay(dir.path(), &other).unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(message) if message.contains("another document")),
        "{err:?}"
    );
}
