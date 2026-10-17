//! The single writer of a document file: an engine that opens or creates a
//! document holds an OS lock on it until it closes it, a second engine (in
//! another process, or here in the same one) gets a read-only handle, and the
//! crash-recovery journal of the file is only ever touched by the holder.

use std::fs;
use std::path::{Path, PathBuf};

use forma_engine::{Engine, EngineConfig, ErrorCode, ToolError, ToolOutput};
use forma_model::Actor;
use serde_json::{Value, json};
use tempfile::TempDir;

/// The message of every change refused on a read-only handle.
const READ_ONLY: &str =
    "document is open for writing in another FORMA process; this handle is read-only";

const SQUARE: &str = "M100 0 L500 0 L500 400 L100 400 Z";

/// Folders shared by the engines of a test, as two `forma mcp` processes
/// configured alike would share them.
struct Shared {
    _root: TempDir,
    docs: PathBuf,
    journals: PathBuf,
    locks: PathBuf,
}

impl Shared {
    fn new() -> Shared {
        let root = TempDir::new().expect("tempdir");
        let docs = root.path().join("docs");
        fs::create_dir(&docs).expect("mkdir");
        Shared {
            journals: root.path().join("journals"),
            locks: root.path().join("locks"),
            docs,
            _root: root,
        }
    }

    fn config(&self) -> EngineConfig {
        EngineConfig {
            allowed_roots: vec![self.docs.clone()],
            journal_dir: Some(self.journals.clone()),
            lock_dir: Some(self.locks.clone()),
            ..EngineConfig::default()
        }
    }

    fn engine(&self) -> Engine {
        Engine::new(self.config())
    }

    fn read_only_engine(&self) -> Engine {
        Engine::new(EngineConfig {
            read_only: true,
            ..self.config()
        })
    }

    /// The journal files and how many records each holds.
    fn journals(&self) -> Vec<(String, usize)> {
        let Ok(entries) = fs::read_dir(&self.journals) else {
            return Vec::new();
        };
        let mut files: Vec<(String, usize)> = entries
            .map(|entry| {
                let entry = entry.expect("entry");
                let text = fs::read_to_string(entry.path()).expect("read journal");
                let records = text.lines().filter(|line| !line.trim().is_empty()).count();
                (entry.file_name().to_string_lossy().into_owned(), records)
            })
            .collect();
        files.sort();
        files
    }

    /// The number of records of the only journal file.
    #[track_caller]
    fn records(&self) -> usize {
        let files = self.journals();
        assert_eq!(files.len(), 1, "{files:?}");
        files[0].1
    }
}

fn actor(name: &str) -> Actor {
    Actor::Agent(name.into())
}

fn call(engine: &mut Engine, tool: &str, args: Value) -> Result<ToolOutput, ToolError> {
    engine.call(&actor("test"), tool, args)
}

#[track_caller]
fn ok(engine: &mut Engine, tool: &str, args: Value) -> Value {
    match call(engine, tool, args) {
        Ok(ToolOutput::Json(value)) => value,
        Ok(ToolOutput::Image { summary, .. }) => summary,
        Err(error) => panic!("{tool} failed: {error:?}"),
    }
}

#[track_caller]
fn err(engine: &mut Engine, tool: &str, args: Value) -> ToolError {
    match call(engine, tool, args) {
        Ok(_) => panic!("{tool} succeeded unexpectedly"),
        Err(error) => error,
    }
}

#[track_caller]
fn read_only_refusal(engine: &mut Engine, tool: &str, args: Value) {
    let error = err(engine, tool, args);
    assert_eq!(error.code, ErrorCode::Unsupported, "{tool}: {error:?}");
    assert_eq!(error.message, READ_ONLY, "{tool}");
}

fn doc_of(value: &Value) -> String {
    value["doc"].as_str().expect("doc").to_owned()
}

fn glyph_names(engine: &mut Engine, doc: &str) -> Vec<String> {
    ok(engine, "glyph_list", json!({ "doc": doc }))["glyphs"]
        .as_array()
        .expect("glyphs")
        .iter()
        .map(|glyph| glyph["name"].as_str().expect("name").to_owned())
        .collect()
}

/// Creates `<docs>/<name>` in `engine` with one glyph `X` (rev 1).
fn create_with_x(shared: &Shared, engine: &mut Engine, name: &str) -> (String, PathBuf) {
    let path = shared.docs.join(name);
    let doc = doc_of(&ok(
        engine,
        "document_create",
        json!({ "family_name": "Locked", "path": path }),
    ));
    ok(
        engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "X", "advance_width": 500,
                "svg_path": SQUARE }),
    );
    (doc, path)
}

#[test]
fn a_second_engine_gets_a_read_only_handle() {
    let shared = Shared::new();
    let mut a = shared.engine();
    let (doc_a, path) = create_with_x(&shared, &mut a, "demo.forma");
    ok(
        &mut a,
        "document_save",
        json!({ "doc": doc_a, "base_rev": 1 }),
    );
    ok(
        &mut a,
        "glyph_create",
        json!({ "doc": doc_a, "base_rev": 1, "name": "W", "advance_width": 500 }),
    );
    assert_eq!(shared.records(), 1);

    let mut b = shared.engine();
    let opened = ok(&mut b, "document_open", json!({ "path": path }));
    assert_eq!(opened["read_only"], json!(true), "{opened}");
    assert_eq!(opened["locked_by_other_process"], json!(true), "{opened}");
    // A's live journal is not B's to recover.
    assert!(opened.get("recovery_available").is_none(), "{opened}");
    let doc_b = doc_of(&opened);
    assert_eq!(
        ok(&mut b, "document_inspect", json!({ "doc": doc_b }))["read_only"],
        json!(true)
    );
    let listed = ok(&mut b, "document_list", json!({}));
    assert_eq!(listed["documents"][0]["read_only"], json!(true), "{listed}");
    // Opening it again in B returns the same read-only handle.
    let again = ok(&mut b, "document_open", json!({ "path": path }));
    assert_eq!(again["already_open"], json!(true));
    assert_eq!(again["read_only"], json!(true));
    assert_eq!(again["locked_by_other_process"], json!(true));

    // Every change of B is refused, dry runs included.
    let create = json!({ "doc": doc_b, "base_rev": 0, "name": "Y", "advance_width": 500 });
    read_only_refusal(&mut b, "glyph_create", create.clone());
    let mut dry_run = create;
    dry_run["dry_run"] = json!(true);
    read_only_refusal(&mut b, "glyph_create", dry_run);
    read_only_refusal(
        &mut b,
        "edit_batch",
        json!({ "doc": doc_b, "base_rev": 0, "ops": [
            { "tool": "glyph_create", "args": { "name": "Y", "advance_width": 1 } }
        ] }),
    );
    read_only_refusal(&mut b, "history_undo", json!({ "doc": doc_b }));
    read_only_refusal(
        &mut b,
        "history_restore",
        json!({ "doc": doc_b, "base_rev": 0, "rev": 0 }),
    );
    read_only_refusal(
        &mut b,
        "document_save",
        json!({ "doc": doc_b, "base_rev": 0 }),
    );
    read_only_refusal(
        &mut b,
        "document_save",
        json!({ "doc": doc_b, "base_rev": 0, "path": shared.docs.join("copy.forma") }),
    );
    let existing = shared.docs.join("existing.ttf");
    fs::write(&existing, b"old").expect("write");
    read_only_refusal(
        &mut b,
        "export_font",
        json!({ "doc": doc_b, "base_rev": 0, "format": "ttf", "path": existing }),
    );

    // Reads, previews, validation and an export to a new file still work.
    assert_eq!(glyph_names(&mut b, &doc_b), ["X"]);
    ok(&mut b, "validate", json!({ "doc": doc_b }));
    ok(
        &mut b,
        "preview_glyph",
        json!({ "doc": doc_b, "glyph": "X", "size": 64 }),
    );
    let exported = ok(
        &mut b,
        "export_font",
        json!({ "doc": doc_b, "base_rev": 0, "format": "ttf",
                "path": shared.docs.join("new.ttf") }),
    );
    assert!(exported["bytes"].as_u64().is_some_and(|bytes| bytes > 0));

    // Closing B's handle leaves A's journal alone, and A keeps journaling.
    ok(&mut b, "document_close", json!({ "doc": doc_b }));
    assert_eq!(shared.records(), 1);
    ok(
        &mut a,
        "glyph_create",
        json!({ "doc": doc_a, "base_rev": 2, "name": "Z", "advance_width": 500 }),
    );
    assert_eq!(shared.records(), 2);

    // Once A closed the document, B opens it for writing.
    ok(
        &mut a,
        "document_save",
        json!({ "doc": doc_a, "base_rev": 3 }),
    );
    ok(&mut a, "document_close", json!({ "doc": doc_a }));
    let reopened = ok(&mut b, "document_open", json!({ "path": path }));
    assert_eq!(reopened["read_only"], json!(false), "{reopened}");
    assert_eq!(reopened["locked_by_other_process"], json!(false));
    ok(
        &mut b,
        "glyph_create",
        json!({ "doc": doc_of(&reopened), "base_rev": 0, "name": "Y", "advance_width": 500 }),
    );
}

#[test]
fn a_crashed_writer_leaves_its_records_to_the_next_writer() {
    let shared = Shared::new();
    let path = {
        let mut a = shared.engine();
        let (doc_a, path) = create_with_x(&shared, &mut a, "crash.forma");
        // A reader in another process meanwhile cannot disturb the journal.
        let mut b = shared.engine();
        let doc_b = doc_of(&ok(&mut b, "document_open", json!({ "path": path })));
        read_only_refusal(
            &mut b,
            "document_save",
            json!({ "doc": doc_b, "base_rev": 0 }),
        );
        ok(&mut b, "document_close", json!({ "doc": doc_b }));
        ok(
            &mut a,
            "glyph_create",
            json!({ "doc": doc_a, "base_rev": 1, "name": "Z", "advance_width": 500 }),
        );
        path
        // A dies here without saving or closing: its lock goes with it.
    };
    assert_eq!(shared.records(), 2);

    let mut c = shared.engine();
    let opened = ok(&mut c, "document_open", json!({ "path": path }));
    assert_eq!(opened["read_only"], json!(false), "{opened}");
    assert_eq!(opened["recovery_available"], json!({ "records": 2 }));
    let doc = doc_of(&opened);
    ok(&mut c, "document_close", json!({ "doc": doc }));
    // Not recovered: the records stay.
    assert_eq!(shared.records(), 2);
    let recovered = ok(
        &mut c,
        "document_open",
        json!({ "path": path, "recover": true }),
    );
    assert_eq!(recovered["recovered"], json!(2));
    assert_eq!(glyph_names(&mut c, &doc_of(&recovered)), ["X", "Z"]);
}

#[test]
fn a_read_only_engine_never_touches_a_journal() {
    let shared = Shared::new();
    let mut w = shared.engine();
    let (doc_w, path) = create_with_x(&shared, &mut w, "watched.forma");

    let mut r = shared.read_only_engine();
    let opened = ok(&mut r, "document_open", json!({ "path": path }));
    assert_eq!(opened["read_only"], json!(true), "{opened}");
    // A read-only engine never takes the lock, so it cannot tell.
    assert_eq!(opened["locked_by_other_process"], json!(false), "{opened}");
    assert!(opened.get("recovery_available").is_none(), "{opened}");
    ok(&mut r, "document_close", json!({ "doc": doc_of(&opened) }));
    assert_eq!(shared.records(), 1, "the writer's journal survives");

    // Not even the records a crash left behind are a read-only engine's to
    // offer, recover or delete.
    drop(w);
    let opened = ok(&mut r, "document_open", json!({ "path": path }));
    assert!(opened.get("recovery_available").is_none(), "{opened}");
    ok(&mut r, "document_close", json!({ "doc": doc_of(&opened) }));
    let error = err(
        &mut r,
        "document_open",
        json!({ "path": path, "recover": true }),
    );
    assert_eq!(error.code, ErrorCode::Unsupported, "{error:?}");
    assert_eq!(shared.records(), 1);
    // The read-only engine never blocked a writer either.
    let mut next = shared.engine();
    let opened = ok(&mut next, "document_open", json!({ "path": path }));
    assert_eq!(opened["read_only"], json!(false));
    assert_eq!(opened["recovery_available"], json!({ "records": 1 }));
    let _ = doc_w;
}

#[test]
fn journals_belong_to_the_document_file() {
    let shared = Shared::new();
    let mut engine = shared.engine();
    let (first, first_path) = create_with_x(&shared, &mut engine, "first.forma");
    ok(
        &mut engine,
        "document_save",
        json!({ "doc": first, "base_rev": 1 }),
    );
    // A copy of the file: the same font id, another file.
    let copy_path = shared.docs.join("copy.forma");
    fs::copy(&first_path, &copy_path).expect("copy");
    let copy = ok(&mut engine, "document_open", json!({ "path": copy_path }));
    assert!(copy.get("journal_warning").is_none(), "{copy}");
    let copy = doc_of(&copy);
    for (doc, base_rev) in [(&first, 1), (&copy, 0)] {
        let done = ok(
            &mut engine,
            "glyph_create",
            json!({ "doc": doc, "base_rev": base_rev, "name": "Y", "advance_width": 500 }),
        );
        assert!(done.get("journal_warning").is_none(), "{done}");
    }
    let files = shared.journals();
    assert_eq!(files.len(), 2, "one journal per file: {files:?}");
    assert!(files.iter().all(|(_, records)| *records == 1), "{files:?}");

    // Saving the copy elsewhere moves its journal along with it.
    let moved = shared.docs.join("moved.forma");
    ok(
        &mut engine,
        "document_save",
        json!({ "doc": copy, "base_rev": 1, "path": moved }),
    );
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": copy, "base_rev": 1, "name": "W", "advance_width": 500 }),
    );
    let files = shared.journals();
    assert_eq!(files.len(), 2, "{files:?}");
    // The old file's lock went with it: another engine may write it now,
    // while the new file is locked.
    let mut other = shared.engine();
    let old = ok(&mut other, "document_open", json!({ "path": copy_path }));
    assert_eq!(old["read_only"], json!(false), "{old}");
    assert!(old.get("recovery_available").is_none(), "{old}");
    let new = ok(&mut other, "document_open", json!({ "path": moved }));
    assert_eq!(new["read_only"], json!(true), "{new}");
}

#[test]
fn a_file_locked_elsewhere_cannot_be_created_or_saved_over() {
    let shared = Shared::new();
    let mut a = shared.engine();
    let (_, path) = create_with_x(&shared, &mut a, "taken.forma");

    let mut b = shared.engine();
    let doc_b = doc_of(&ok(
        &mut b,
        "document_create",
        json!({ "family_name": "Other" }),
    ));
    let error = err(
        &mut b,
        "document_save",
        json!({ "doc": doc_b, "base_rev": 0, "path": path }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
    assert!(
        error
            .message
            .contains("open for writing in another FORMA process"),
        "{}",
        error.message
    );
    // The file disappears from under A, but A still holds its lock.
    fs::remove_file(&path).expect("remove");
    let error = err(
        &mut b,
        "document_create",
        json!({ "family_name": "Other", "path": path }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
    assert!(
        error.message.contains("another FORMA process"),
        "{}",
        error.message
    );
}

#[test]
fn lock_files_are_named_by_the_document_path() {
    let shared = Shared::new();
    let mut a = shared.engine();
    create_with_x(&shared, &mut a, "named.forma");
    let names: Vec<String> = fs::read_dir(&shared.locks)
        .expect("lock folder")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    let stem = names[0].strip_suffix(".lock").expect(".lock");
    assert_eq!(stem.len(), 64, "{stem}");
    assert!(stem.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    // Without a lock folder nothing is locked.
    let mut plain = Engine::new(EngineConfig {
        allowed_roots: vec![shared.docs.clone()],
        ..EngineConfig::default()
    });
    let opened = ok(
        &mut plain,
        "document_open",
        json!({ "path": shared.docs.join("named.forma") }),
    );
    assert_eq!(opened["read_only"], json!(false), "{opened}");
    let _ = Path::new("");
}
