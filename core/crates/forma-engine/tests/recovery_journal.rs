//! The crash-recovery journal: every committed change of a document that has
//! a file is journaled, and `document_open` offers and performs recovery.

use std::fs;
use std::path::PathBuf;

use forma_engine::{Engine, EngineConfig, ErrorCode, ToolError, ToolOutput};
use forma_model::Actor;
use serde_json::{Value, json};
use tempfile::TempDir;

// ------------------------------------------------------------------ helpers

fn actor() -> Actor {
    Actor::Agent("test".into())
}

/// A sandbox with a folder for documents and one for journals.
struct Sandbox {
    _root: TempDir,
    docs: PathBuf,
    journals: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let root = TempDir::new().expect("tempdir");
        let docs = root.path().join("docs");
        let journals = root.path().join("journals");
        fs::create_dir(&docs).expect("mkdir");
        Sandbox {
            docs,
            journals,
            _root: root,
        }
    }

    fn engine(&self) -> Engine {
        Engine::new(EngineConfig {
            allowed_roots: vec![self.docs.clone()],
            journal_dir: Some(self.journals.clone()),
            ..EngineConfig::default()
        })
    }

    /// The journal files, with their sizes.
    fn journal_files(&self) -> Vec<(String, u64)> {
        let Ok(entries) = fs::read_dir(&self.journals) else {
            return Vec::new();
        };
        let mut files: Vec<(String, u64)> = entries
            .map(|entry| {
                let entry = entry.expect("entry");
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.metadata().expect("metadata").len(),
                )
            })
            .collect();
        files.sort();
        files
    }

    /// The number of records in the only journal file.
    fn journal_records(&self) -> usize {
        let files = self.journal_files();
        assert_eq!(files.len(), 1, "{files:?}");
        let text = fs::read_to_string(self.journals.join(&files[0].0)).expect("read journal");
        text.lines().filter(|line| !line.trim().is_empty()).count()
    }
}

fn call(engine: &mut Engine, tool: &str, args: Value) -> Result<Value, ToolError> {
    match engine.call(&actor(), tool, args)? {
        ToolOutput::Json(value) => Ok(value),
        ToolOutput::Image { .. } => panic!("{tool} returned an image"),
    }
}

#[track_caller]
fn ok(engine: &mut Engine, tool: &str, args: Value) -> Value {
    match call(engine, tool, args) {
        Ok(value) => value,
        Err(error) => panic!("{tool} failed: {error:?}"),
    }
}

#[track_caller]
fn err(engine: &mut Engine, tool: &str, args: Value) -> ToolError {
    match call(engine, tool, args) {
        Ok(value) => panic!("{tool} succeeded unexpectedly: {value}"),
        Err(error) => error,
    }
}

#[track_caller]
fn confirmed(engine: &mut Engine, tool: &str, args: Value) -> Value {
    let error = err(engine, tool, args);
    assert_eq!(error.code, ErrorCode::ApprovalRequired, "{tool}: {error:?}");
    let token = error.details["plan"]["plan_token"].clone();
    ok(engine, "plan_commit", json!({ "plan_token": token }))
}

const SQUARE: &str = "M100 0 L500 0 L500 400 L100 400 Z";
const TRIANGLE: &str = "M0 0 L300 0 L150 300 Z";

fn doc_of(value: &Value) -> String {
    value["doc"].as_str().expect("doc").to_owned()
}

/// Everything the tools report about the content of a document.
fn state(engine: &mut Engine, doc: &str) -> Value {
    let inspect = ok(engine, "document_inspect", json!({ "doc": doc }));
    let list = ok(engine, "glyph_list", json!({ "doc": doc, "limit": 500 }));
    let glyphs: Vec<Value> = list["glyphs"]
        .as_array()
        .expect("glyphs")
        .iter()
        .map(|glyph| {
            ok(
                engine,
                "glyph_get",
                json!({ "doc": doc, "glyph": glyph["id"] }),
            )["glyph"]
                .clone()
        })
        .collect();
    json!({
        "info": inspect["info"],
        "units_per_em": inspect["units_per_em"],
        "masters": inspect["masters"],
        "glyph_count": inspect["glyph_count"],
        "list": list["glyphs"],
        "glyphs": glyphs,
    })
}

// --------------------------------------------------------------------- tests

#[test]
fn journal_records_written_and_recovered() {
    let sandbox = Sandbox::new();
    let path = sandbox.docs.join("font.forma");

    let (before_crash, saved_state) = {
        let mut engine = sandbox.engine();
        let created = ok(
            &mut engine,
            "document_create",
            json!({ "family_name": "Journal", "path": path }),
        );
        let doc = doc_of(&created);
        let saved_state = state(&mut engine, &doc);
        let mut rev = 0;
        let mut next = |engine: &mut Engine, tool: &str, mut args: Value| {
            args["doc"] = json!(doc);
            args["base_rev"] = json!(rev);
            let done = ok(engine, tool, args);
            assert!(done.get("journal_warning").is_none(), "{done}");
            rev += 1;
            done
        };
        next(
            &mut engine,
            "glyph_create",
            json!({ "name": "A", "unicodes": [65],
                                                  "advance_width": 600, "svg_path": SQUARE }),
        );
        next(
            &mut engine,
            "glyph_create",
            json!({ "name": "B", "advance_width": 500 }),
        );
        next(
            &mut engine,
            "glyph_create",
            json!({ "name": "C", "advance_width": 400,
                                                  "svg_path": TRIANGLE }),
        );
        next(
            &mut engine,
            "metrics_set",
            json!({ "items": [{ "glyph": "A", "lsb": 30 }] }),
        );
        next(
            &mut engine,
            "font_set_info",
            json!({ "family_name": "Recovered",
                                                   "units_per_em": 2048 }),
        );
        next(
            &mut engine,
            "font_set_vertical_metrics",
            json!({ "ascender": 1600 }),
        );
        next(
            &mut engine,
            "glyph_update",
            json!({ "glyph": "B", "name": "Bee" }),
        );
        // A destructive change, through its plan.
        let deleted = confirmed(
            &mut engine,
            "glyph_delete",
            json!({ "doc": doc, "base_rev": rev, "glyphs": ["C"] }),
        );
        assert!(deleted.get("journal_warning").is_none(), "{deleted}");
        // Undo and redo are state changes too.
        ok(&mut engine, "history_undo", json!({ "doc": doc }));
        ok(&mut engine, "history_redo", json!({ "doc": doc }));
        let rev = ok(&mut engine, "document_inspect", json!({ "doc": doc }))["rev"]
            .as_u64()
            .expect("rev");
        // Take back the rename (transaction 7).
        ok(
            &mut engine,
            "history_revert",
            json!({ "doc": doc, "base_rev": rev, "tx": 7 }),
        );
        assert_eq!(sandbox.journal_records(), 11);
        (state(&mut engine, &doc), saved_state)
        // The engine is dropped without saving: a crash.
    };
    assert_ne!(before_crash, saved_state);

    let mut engine = sandbox.engine();
    let opened = ok(&mut engine, "document_open", json!({ "path": path }));
    assert_eq!(opened["recovery_available"], json!({ "records": 11 }));
    assert!(opened.get("recovered").is_none(), "{opened}");
    let doc = doc_of(&opened);
    // Without `recover` the document is the saved file.
    assert_eq!(state(&mut engine, &doc), saved_state);
    // Closing it clean keeps the journal for a later recovery.
    ok(&mut engine, "document_close", json!({ "doc": doc }));
    assert_eq!(sandbox.journal_records(), 11);

    let recovered = ok(
        &mut engine,
        "document_open",
        json!({ "path": path, "recover": true }),
    );
    assert_eq!(recovered["recovered"], json!(11));
    assert_eq!(recovered["rev"], json!(0));
    assert!(recovered.get("recovery_available").is_none(), "{recovered}");
    let doc = doc_of(&recovered);
    let inspect = ok(&mut engine, "document_inspect", json!({ "doc": doc }));
    assert_eq!(inspect["rev"], json!(0));
    assert_eq!(inspect["dirty"], json!(true));
    assert_eq!(state(&mut engine, &doc), before_crash);
    // The journal is kept until the next save.
    assert_eq!(sandbox.journal_records(), 11);

    // Opening it again while it is open cannot recover twice.
    let error = err(
        &mut engine,
        "document_open",
        json!({ "path": path, "recover": true }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);

    // Edits after a recovery are journaled after the recovered records, and a
    // save makes the journal empty.
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "D", "advance_width": 100 }),
    );
    assert_eq!(sandbox.journal_records(), 12);
    let saved = ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 1 }),
    );
    assert!(saved.get("journal_warning").is_none(), "{saved}");
    assert_eq!(sandbox.journal_records(), 0);
}

#[test]
fn save_truncates_journal() {
    let sandbox = Sandbox::new();
    let path = sandbox.docs.join("font.formapackage");
    let mut engine = sandbox.engine();
    let doc = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Package", "path": path }),
    ));

    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "A", "advance_width": 500 }),
    );
    ok(
        &mut engine,
        "metrics_set",
        json!({ "doc": doc, "base_rev": 1, "items": [{ "glyph": "A", "advance_width": 510 }] }),
    );
    assert_eq!(sandbox.journal_records(), 2);
    ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 2 }),
    );
    let files = sandbox.journal_files();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].1, 0, "the journal is empty after a save");

    ok(
        &mut engine,
        "glyph_update",
        json!({ "doc": doc, "base_rev": 2, "glyph": "A", "export": false }),
    );
    assert_eq!(sandbox.journal_records(), 1);
    ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 3 }),
    );
    assert_eq!(sandbox.journal_records(), 0);

    // Reopened after the save: nothing to recover.
    ok(&mut engine, "document_close", json!({ "doc": doc }));
    let opened = ok(&mut engine, "document_open", json!({ "path": path }));
    assert!(opened.get("recovery_available").is_none(), "{opened}");
}

#[test]
fn closing_removes_the_journal_of_the_session() {
    let sandbox = Sandbox::new();
    let path = sandbox.docs.join("font.forma");
    let mut engine = sandbox.engine();
    let doc = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Close", "path": path }),
    ));
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "A", "advance_width": 500 }),
    );
    ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 1 }),
    );
    let closed = ok(&mut engine, "document_close", json!({ "doc": doc }));
    assert!(closed.get("journal_warning").is_none(), "{closed}");
    assert!(
        sandbox.journal_files().is_empty(),
        "{:?}",
        sandbox.journal_files()
    );

    // Discarding unsaved changes (an approved plan) discards their journal.
    let doc = doc_of(&ok(&mut engine, "document_open", json!({ "path": path })));
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "B", "advance_width": 500 }),
    );
    assert_eq!(sandbox.journal_records(), 1);
    confirmed(
        &mut engine,
        "document_close",
        json!({ "doc": doc, "discard_changes": true }),
    );
    assert!(sandbox.journal_files().is_empty());
    let opened = ok(&mut engine, "document_open", json!({ "path": path }));
    assert!(opened.get("recovery_available").is_none(), "{opened}");
}

#[test]
fn stale_journals_are_kept_and_not_extended() {
    let sandbox = Sandbox::new();
    let path = sandbox.docs.join("font.forma");
    {
        let mut engine = sandbox.engine();
        let doc = doc_of(&ok(
            &mut engine,
            "document_create",
            json!({ "family_name": "Stale", "path": path }),
        ));
        ok(
            &mut engine,
            "glyph_create",
            json!({ "doc": doc, "base_rev": 0, "name": "A", "advance_width": 500 }),
        );
    }
    let mut engine = sandbox.engine();
    let opened = ok(&mut engine, "document_open", json!({ "path": path }));
    assert_eq!(opened["recovery_available"], json!({ "records": 1 }));
    let doc = doc_of(&opened);
    // Changes made without recovering are not added to the old journal.
    let done = ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "Z", "advance_width": 500 }),
    );
    let warning = done["journal_warning"].as_str().expect("journal_warning");
    assert!(warning.contains("not journaled"), "{warning}");
    assert_eq!(sandbox.journal_records(), 1);
    // Saving replaces the old journal with a new, empty one.
    ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 1 }),
    );
    assert_eq!(sandbox.journal_records(), 0);
    let done = ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 1, "name": "Y", "advance_width": 500 }),
    );
    assert!(done.get("journal_warning").is_none(), "{done}");
    assert_eq!(sandbox.journal_records(), 1);
}

#[test]
fn copies_of_a_font_do_not_share_a_journal() {
    let sandbox = Sandbox::new();
    let first = sandbox.docs.join("first.forma");
    let copy = sandbox.docs.join("copy.forma");
    let mut engine = sandbox.engine();
    ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Twins", "path": first }),
    );
    fs::copy(&first, &copy).expect("copy");
    let doc = doc_of(&ok(&mut engine, "document_open", json!({ "path": first })));
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "A", "advance_width": 500 }),
    );
    assert_eq!(sandbox.journal_records(), 1);

    // The copy has the same font id but is another file: it neither sees the
    // first one's records as recoverable nor writes into its journal; it
    // keeps a journal of its own.
    let twin = ok(&mut engine, "document_open", json!({ "path": copy }));
    assert!(twin.get("recovery_available").is_none(), "{twin}");
    assert!(twin.get("journal_warning").is_none(), "{twin}");
    let twin = doc_of(&twin);
    let done = ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": twin, "base_rev": 0, "name": "B", "advance_width": 500 }),
    );
    assert!(done.get("journal_warning").is_none(), "{done}");
    let files = sandbox.journal_files();
    assert_eq!(files.len(), 2, "{files:?}");

    // After a crash, each file recovers its own changes.
    drop(engine);
    let mut engine = sandbox.engine();
    for (path, glyph) in [(&first, "A"), (&copy, "B")] {
        let opened = ok(
            &mut engine,
            "document_open",
            json!({ "path": path, "recover": true }),
        );
        assert_eq!(opened["recovered"], json!(1), "{opened}");
        let names = ok(&mut engine, "glyph_list", json!({ "doc": doc_of(&opened) }))["glyphs"]
            .as_array()
            .expect("glyphs")
            .iter()
            .map(|glyph| glyph["name"].clone())
            .collect::<Vec<_>>();
        assert_eq!(names, [json!(glyph)]);
    }
    let error = err(
        &mut engine,
        "document_open",
        json!({ "path": sandbox.docs.join("missing.forma"), "recover": true }),
    );
    assert_eq!(error.code, ErrorCode::NotFound);
}

#[test]
fn journal_write_failure_is_a_warning() {
    let root = TempDir::new().expect("tempdir");
    // The journal folder cannot be created: a file is in the way.
    let blocker = root.path().join("journals");
    fs::write(&blocker, b"not a folder").expect("write");
    let mut engine = Engine::new(EngineConfig {
        allowed_roots: vec![root.path().to_path_buf()],
        journal_dir: Some(blocker.clone()),
        ..EngineConfig::default()
    });
    let doc = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Warn", "path": "font.forma" }),
    ));
    let done = ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "A", "advance_width": 500 }),
    );
    assert_eq!(done["rev"], json!(1));
    let warning = done["journal_warning"].as_str().expect("journal_warning");
    assert!(warning.contains("journal"), "{warning}");
    let undone = ok(&mut engine, "history_undo", json!({ "doc": doc }));
    assert!(undone["journal_warning"].is_string(), "{undone}");
    assert_eq!(fs::read(&blocker).expect("read"), b"not a folder");
}

#[test]
fn documents_without_a_file_are_not_journaled() {
    let sandbox = Sandbox::new();
    let mut engine = sandbox.engine();
    let doc = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Memory" }),
    ));
    let done = ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "A", "advance_width": 500 }),
    );
    assert!(done.get("journal_warning").is_none(), "{done}");
    assert!(sandbox.journal_files().is_empty());

    // Once saved to a file, it is.
    ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 1, "path": sandbox.docs.join("now.forma") }),
    );
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 1, "name": "B", "advance_width": 500 }),
    );
    assert_eq!(sandbox.journal_records(), 1);

    // Without a journal folder nothing is journaled and nothing is offered.
    let mut plain = Engine::new(EngineConfig {
        allowed_roots: vec![sandbox.docs.clone()],
        ..EngineConfig::default()
    });
    let opened = ok(
        &mut plain,
        "document_open",
        json!({ "path": sandbox.docs.join("now.forma"), "recover": true }),
    );
    assert_eq!(opened["recovered"], json!(0));
    assert!(opened.get("recovery_available").is_none());
    assert_eq!(
        ok(
            &mut plain,
            "document_inspect",
            json!({ "doc": doc_of(&opened) })
        )["dirty"],
        json!(false)
    );
}

/// Runs `tool` as `actor`.
#[track_caller]
fn ok_as(engine: &mut Engine, actor: &Actor, tool: &str, args: Value) -> Value {
    match engine.call(actor, tool, args) {
        Ok(ToolOutput::Json(value)) => value,
        Ok(ToolOutput::Image { .. }) => panic!("{tool} returned an image"),
        Err(error) => panic!("{tool} failed: {error:?}"),
    }
}

/// The `journal_warning` of a result, which must be there.
#[track_caller]
fn warning_of(result: &Value) -> String {
    result["journal_warning"]
        .as_str()
        .unwrap_or_else(|| panic!("no journal_warning in {result}"))
        .to_owned()
}

/// Opens `path` with `recover: true` in a fresh engine (after a crash) and
/// returns the engine, the handle and the number of records recovered.
fn recover(sandbox: &Sandbox, path: &std::path::Path) -> (Engine, String, Value) {
    let mut engine = sandbox.engine();
    let opened = ok(
        &mut engine,
        "document_open",
        json!({ "path": path, "recover": true }),
    );
    let doc = doc_of(&opened);
    (engine, doc, opened["recovered"].clone())
}

#[test]
fn a_failed_journal_write_stops_journaling_until_the_next_save() {
    let sandbox = Sandbox::new();
    let path = sandbox.docs.join("font.forma");
    // A record whose actor cannot be encoded (an agent without a name) cannot
    // be written: a write failure that only that change meets.
    let nameless = Actor::Agent(String::new());

    let prefix = {
        let mut engine = sandbox.engine();
        let doc = doc_of(&ok(
            &mut engine,
            "document_create",
            json!({ "family_name": "Gap", "path": path }),
        ));
        let created = ok(
            &mut engine,
            "glyph_create",
            json!({ "doc": doc, "base_rev": 0, "name": "A", "advance_width": 500 }),
        );
        assert!(created.get("journal_warning").is_none(), "{created}");
        assert_eq!(sandbox.journal_records(), 1);
        let prefix = state(&mut engine, &doc);

        let failed = ok_as(
            &mut engine,
            &nameless,
            "glyph_create",
            json!({ "doc": doc, "base_rev": 1, "name": "B", "advance_width": 500 }),
        );
        let warning = warning_of(&failed);
        assert!(warning.contains("could not be written"), "{warning}");
        assert!(warning.contains("until the document is saved"), "{warning}");

        // Later changes would be written fine, but never after the gap: a
        // record of `B` without the record that created it would make the
        // whole journal unrecoverable.
        for (tool, args) in [
            (
                "metrics_set",
                json!({ "doc": doc, "base_rev": 2,
                        "items": [{ "glyph": "B", "advance_width": 640 }] }),
            ),
            (
                "glyph_update",
                json!({ "doc": doc, "base_rev": 3, "glyph": "A", "export": false }),
            ),
            // Nothing to fix: no change, but the warning still says that
            // changes are not journaled.
            ("geometry_fix", json!({ "doc": doc, "base_rev": 4 })),
        ] {
            let done = ok(&mut engine, tool, args);
            let warning = warning_of(&done);
            assert!(warning.contains("not journaled"), "{tool}: {warning}");
            assert!(warning.contains("save the document"), "{tool}: {warning}");
        }
        let undone = ok(&mut engine, "history_undo", json!({ "doc": doc }));
        warning_of(&undone);
        assert_eq!(sandbox.journal_records(), 1);
        prefix
        // A crash: the engine goes away without saving.
    };

    // Recovery gives the state as of the last journaled change: a consistent
    // prefix of the session.
    let (mut engine, doc, recovered) = recover(&sandbox, &path);
    assert_eq!(recovered, json!(1));
    assert_eq!(state(&mut engine, &doc), prefix);

    // The same in this session, then a save starts a new journal.
    let failed = ok_as(
        &mut engine,
        &nameless,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "C", "advance_width": 300 }),
    );
    warning_of(&failed);
    let later = ok(
        &mut engine,
        "metrics_set",
        json!({ "doc": doc, "base_rev": 1, "items": [{ "glyph": "C", "advance_width": 310 }] }),
    );
    warning_of(&later);
    assert_eq!(sandbox.journal_records(), 1, "only the recovered record");
    let saved = ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 2 }),
    );
    assert!(saved.get("journal_warning").is_none(), "{saved}");
    assert_eq!(sandbox.journal_records(), 0);
    let journaled = ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 2, "name": "D", "advance_width": 200 }),
    );
    assert!(journaled.get("journal_warning").is_none(), "{journaled}");
    assert_eq!(sandbox.journal_records(), 1);
    let before_crash = state(&mut engine, &doc);
    drop(engine);

    let (mut engine, doc, recovered) = recover(&sandbox, &path);
    assert_eq!(recovered, json!(1));
    assert_eq!(state(&mut engine, &doc), before_crash);

    // A journal stopped by a failure is this session's: discarding the
    // changes removes it, as it does an active journal.
    let failed = ok_as(
        &mut engine,
        &nameless,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "E", "advance_width": 100 }),
    );
    warning_of(&failed);
    confirmed(
        &mut engine,
        "document_close",
        json!({ "doc": doc, "discard_changes": true }),
    );
    assert!(
        sandbox.journal_files().is_empty(),
        "{:?}",
        sandbox.journal_files()
    );
}

/// The coordinator's example: the record that creates a glyph is lost, a
/// later change of that glyph would be journaled without it.
#[cfg(unix)]
#[test]
fn a_journal_that_cannot_be_opened_stops_journaling_until_the_next_save() {
    use std::os::unix::fs::PermissionsExt;

    /// Makes the folder writable again, even when the test panics.
    struct Unlock<'a>(&'a std::path::Path);
    impl Drop for Unlock<'_> {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o755));
        }
    }

    let sandbox = Sandbox::new();
    let path = sandbox.docs.join("font.forma");
    fs::create_dir(&sandbox.journals).expect("mkdir");
    let unlock = Unlock(&sandbox.journals);
    fs::set_permissions(&sandbox.journals, fs::Permissions::from_mode(0o555)).expect("lock");
    if fs::write(sandbox.journals.join("probe"), b"").is_ok() {
        return; // Permissions do not apply (running as root): nothing to test.
    }

    let mut engine = sandbox.engine();
    let doc = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Locked", "path": path }),
    ));
    let created = ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "A", "advance_width": 500,
                "svg_path": SQUARE }),
    );
    let warning = warning_of(&created);
    assert!(warning.contains("could not be opened"), "{warning}");

    // The folder is writable again, but the creation of `A` is not in the
    // journal: a record of a change to `A` must not be written either.
    drop(unlock);
    let changed = ok(
        &mut engine,
        "metrics_set",
        json!({ "doc": doc, "base_rev": 1, "items": [{ "glyph": "A", "lsb": 20 }] }),
    );
    warning_of(&changed);
    assert!(
        sandbox.journal_files().is_empty(),
        "{:?}",
        sandbox.journal_files()
    );

    // A save starts a new journal; the changes after it are journaled and
    // recoverable.
    let saved = ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 2 }),
    );
    assert!(saved.get("journal_warning").is_none(), "{saved}");
    let changed = ok(
        &mut engine,
        "metrics_set",
        json!({ "doc": doc, "base_rev": 2, "items": [{ "glyph": "A", "advance_width": 700 }] }),
    );
    assert!(changed.get("journal_warning").is_none(), "{changed}");
    assert_eq!(sandbox.journal_records(), 1);
    let before_crash = state(&mut engine, &doc);
    drop(engine);

    let (mut engine, doc, recovered) = recover(&sandbox, &path);
    assert_eq!(recovered, json!(1));
    assert_eq!(state(&mut engine, &doc), before_crash);
}
