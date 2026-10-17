//! Validation, export, previews and the audit log.

use std::fs;
use std::path::{Path, PathBuf};

use forma_engine::{Engine, EngineConfig, ErrorCode, Policy, ToolError, ToolOutput, registry};
use forma_model::Actor;
use serde_json::{Value, json};
use tempfile::TempDir;

// ------------------------------------------------------------------ helpers

fn actor() -> Actor {
    Actor::Agent("test".into())
}

fn config(root: &Path) -> EngineConfig {
    EngineConfig {
        allowed_roots: vec![root.to_path_buf()],
        ..EngineConfig::default()
    }
}

fn engine_in(root: &Path) -> Engine {
    Engine::new(config(root))
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
    match engine.call(&actor(), tool, args) {
        Ok(output) => panic!("{tool} succeeded unexpectedly: {output:?}"),
        Err(error) => error,
    }
}

/// Calls an image tool and returns the PNG and its summary.
#[track_caller]
fn image(engine: &mut Engine, tool: &str, args: Value) -> (Vec<u8>, Value) {
    match engine.call(&actor(), tool, args) {
        Ok(ToolOutput::Image { png, summary }) => (png, summary),
        Ok(ToolOutput::Json(value)) => panic!("{tool} returned JSON: {value}"),
        Err(error) => panic!("{tool} failed: {error:?}"),
    }
}

/// The width and height a PNG declares in its IHDR chunk.
#[track_caller]
fn png_size(png: &[u8]) -> (u32, u32) {
    assert!(
        png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "not a PNG: {:?}",
        &png[..png.len().min(8)]
    );
    assert_eq!(&png[12..16], b"IHDR");
    let word = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
    (word(16), word(20))
}

fn canon(path: &Path) -> PathBuf {
    dunce::canonicalize(path).expect("canonicalize")
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A counter-clockwise square as SVG path data in font coordinates.
const SQUARE: &str = "M100 0 L500 0 L500 400 L100 400 Z";
/// The same square, clockwise (wrong direction for an outer contour).
const CLOCKWISE: &str = "M100 0 L100 400 L500 400 L500 0 Z";

/// A new in-memory document at rev 0.
fn new_doc(engine: &mut Engine) -> String {
    let created = ok(engine, "document_create", json!({ "family_name": "Test" }));
    created["doc"].as_str().expect("doc").to_owned()
}

/// A new in-memory document with the glyph `A` (a square) at rev 1.
fn doc_with_a(engine: &mut Engine) -> String {
    let doc = new_doc(engine);
    ok(
        engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "A", "unicodes": [65],
                "advance_width": 600, "svg_path": SQUARE }),
    );
    doc
}

fn inspect(engine: &mut Engine, doc: &str) -> Value {
    ok(engine, "document_inspect", json!({ "doc": doc }))
}

fn default_master(engine: &mut Engine, doc: &str) -> String {
    inspect(engine, doc)["masters"][0]["id"]
        .as_str()
        .expect("master id")
        .to_owned()
}

#[track_caller]
fn plan_of(error: &ToolError) -> Value {
    assert_eq!(error.code, ErrorCode::ApprovalRequired, "{error:?}");
    error.details["plan"].clone()
}

#[track_caller]
fn commit(engine: &mut Engine, plan: &Value) -> Result<Value, ToolError> {
    call(
        engine,
        "plan_commit",
        json!({ "plan_token": plan["plan_token"] }),
    )
}

/// The lines of the audit log, parsed.
fn audit_lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("read audit log")
        .lines()
        .map(|line| serde_json::from_str(line).expect("audit line is JSON"))
        .collect()
}

// ------------------------------------------------------------------ validate

#[test]
fn validate_reports_on_the_current_state() {
    let mut engine = Engine::new(EngineConfig::default());
    let doc = doc_with_a(&mut engine);

    let clean = ok(&mut engine, "validate", json!({ "doc": doc }));
    assert_eq!(clean["rev"], json!(1));
    assert_eq!(clean["report"]["errors"], json!(0), "{clean}");
    assert_eq!(clean["report"]["compiled"], json!(true));
    assert!(clean["report"]["findings"].is_array());

    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 1, "name": "B", "advance_width": 600,
                "svg_path": CLOCKWISE }),
    );
    let report = ok(&mut engine, "validate", json!({ "doc": doc }));
    assert_eq!(report["rev"], json!(2));
    assert!(report["report"]["errors"].as_u64().expect("errors") >= 1);
    let findings = report["report"]["findings"].as_array().expect("findings");
    assert!(
        findings.iter().any(
            |finding| finding["check"] == json!("geometry.wrong_direction")
                && finding["glyph"] == json!("B")
        ),
        "{report}"
    );
    // Validating changes nothing.
    assert_eq!(inspect(&mut engine, &doc)["rev"], json!(2));

    let missing = err(&mut engine, "validate", json!({ "doc": "doc_missing" }));
    assert_eq!(missing.code, ErrorCode::NotFound);
    let unknown = err(&mut engine, "validate", json!({ "doc": doc, "deep": true }));
    assert_eq!(unknown.code, ErrorCode::InvalidArgument);
}

// -------------------------------------------------------------------- export

#[test]
fn export_ttf_writes_file_and_reports() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_in(root.path());
    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Test", "path": "font.forma" }),
    );
    let doc = created["doc"].as_str().expect("doc").to_owned();
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "A", "unicodes": [65],
                "advance_width": 600, "svg_path": SQUARE }),
    );
    ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 1 }),
    );
    assert_eq!(inspect(&mut engine, &doc)["dirty"], json!(false));

    let target = canon(root.path()).join("out.ttf");
    let exported = ok(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "ttf", "path": "out.ttf" }),
    );
    let written = fs::read(&target).expect("ttf written");
    assert_eq!(exported["rev"], json!(1));
    assert_eq!(exported["path"], json!(text(&target)));
    assert_eq!(exported["format"], json!("ttf"));
    assert_eq!(exported["bytes"], json!(written.len()));
    assert_eq!(
        exported["report"],
        json!({ "added_glyphs": [".notdef", "space"], "warnings": [] })
    );
    assert_eq!(&written[..4], &[0, 1, 0, 0], "TrueType sfnt version");
    // Exporting neither changes the document nor marks it dirty.
    let after = inspect(&mut engine, &doc);
    assert_eq!(after["rev"], json!(1));
    assert_eq!(after["dirty"], json!(false));
    let leftovers: Vec<String> = fs::read_dir(root.path())
        .expect("list")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");

    // WOFF2, and a stale base revision exports the current state.
    let woff2 = ok(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 0, "format": "woff2", "path": "out.woff2" }),
    );
    let compressed = fs::read(root.path().join("out.woff2")).expect("woff2 written");
    assert_eq!(&compressed[..4], b"wOF2");
    assert_eq!(woff2["bytes"], json!(compressed.len()));
    assert_eq!(woff2["format"], json!("woff2"));

    // UFO: a folder, so no byte count.
    let ufo = ok(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "ufo", "path": "out.ufo" }),
    );
    assert!(root.path().join("out.ufo").join("metainfo.plist").is_file());
    assert_eq!(ufo["bytes"], Value::Null);
    assert_eq!(ufo["format"], json!("ufo"));
    assert_eq!(ufo["report"], json!({ "added_glyphs": [], "warnings": [] }));

    // A base revision newer than the current one is refused.
    let error = err(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 9, "format": "ttf", "path": "later.ttf" }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(!root.path().join("later.ttf").exists());
}

#[test]
fn export_overwrite_requires_plan() {
    let root = TempDir::new().expect("tempdir");
    for policy in [Policy::Prompted, Policy::Auto] {
        let mut engine = Engine::new(EngineConfig {
            policy,
            ..config(root.path())
        });
        let doc = doc_with_a(&mut engine);
        let target = root.path().join("old.ttf");
        fs::write(&target, b"old contents").expect("write");
        let args = json!({ "doc": doc, "base_rev": 1, "format": "ttf", "path": "old.ttf" });

        let error = err(&mut engine, "export_font", args.clone());
        let plan = plan_of(&error);
        assert_eq!(plan["tool"], json!("export_font"));
        assert_eq!(
            plan["summary"],
            json!(format!("overwrites {}", text(&canon(&target))))
        );
        assert_eq!(fs::read(&target).expect("read"), b"old contents");

        let committed = commit(&mut engine, &plan).expect("commit");
        assert_eq!(committed["plan_committed"], json!(true));
        assert_eq!(committed["format"], json!("ttf"));
        let written = fs::read(&target).expect("read");
        assert_eq!(&written[..4], &[0, 1, 0, 0], "{policy:?}");
        assert_eq!(committed["bytes"], json!(written.len()));

        // The plan pins the revision: a change after planning refuses it.
        fs::write(&target, b"old contents").expect("write");
        let plan = plan_of(&err(&mut engine, "export_font", args));
        ok(
            &mut engine,
            "metrics_set",
            json!({ "doc": doc, "base_rev": 1,
                    "items": [{ "glyph": "A", "advance_width": 700 }] }),
        );
        let error = commit(&mut engine, &plan).expect_err("document changed");
        assert_eq!(error.code, ErrorCode::RevisionConflict);
        assert_eq!(fs::read(&target).expect("read"), b"old contents");
    }
}

#[test]
fn export_outside_roots_rejected() {
    let root = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("tempdir");
    let mut engine = engine_in(root.path());
    let doc = doc_with_a(&mut engine);

    for path in [
        text(&outside.path().join("out.ttf")),
        "../escape.ttf".to_owned(),
    ] {
        let error = err(
            &mut engine,
            "export_font",
            json!({ "doc": doc, "base_rev": 1, "format": "ttf", "path": path }),
        );
        assert_eq!(error.code, ErrorCode::PathNotAllowed, "{path}: {error:?}");
    }
    let error = err(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "ufo",
                "path": text(&outside.path().join("out.ufo")) }),
    );
    assert_eq!(error.code, ErrorCode::PathNotAllowed);
    assert_eq!(fs::read_dir(outside.path()).expect("list").count(), 0);

    // With no allowed roots at all, nothing can be exported.
    let mut closed = Engine::new(EngineConfig::default());
    let doc = doc_with_a(&mut closed);
    let error = err(
        &mut closed,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "ttf",
                "path": text(&root.path().join("out.ttf")) }),
    );
    assert_eq!(error.code, ErrorCode::PathNotAllowed);
}

#[test]
fn export_extension_must_match_format() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_in(root.path());
    let doc = doc_with_a(&mut engine);

    for (format, path) in [
        ("ttf", "out.woff2"),
        ("ttf", "out.otf"),
        ("ttf", "out"),
        ("woff2", "out.woff"),
        ("woff2", "out.ttf"),
        ("ufo", "out.ttf"),
        ("ufo", "out.forma"),
    ] {
        let error = err(
            &mut engine,
            "export_font",
            json!({ "doc": doc, "base_rev": 1, "format": format, "path": path }),
        );
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{format} {path}");
        assert!(error.message.contains(format), "{}", error.message);
    }
    assert_eq!(fs::read_dir(root.path()).expect("list").count(), 0);

    // The extension is compared without regard to case.
    ok(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "ttf", "path": "Upper.TTF" }),
    );
    assert!(root.path().join("Upper.TTF").is_file());

    // Unknown formats and fields are refused by the schema.
    let error = err(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "otf", "path": "out.otf" }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    let error = err(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "ttf", "path": "o.ttf", "x": 1 }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);

    // A folder where a file would go is refused before any plan.
    fs::create_dir(root.path().join("folder.ttf")).expect("mkdir");
    let error = err(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "ttf", "path": "folder.ttf" }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(root.path().join("folder.ttf").is_dir());
}

#[test]
fn export_dry_run_returns_plan_for_existing_target() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_in(root.path());
    let doc = doc_with_a(&mut engine);
    let target = root.path().join("font.ttf");
    fs::write(&target, b"keep me").expect("write");

    let planned = ok(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "ttf", "path": "font.ttf",
                "dry_run": true }),
    );
    assert_eq!(planned["rev"], json!(1));
    let plan = &planned["plan"];
    assert_eq!(plan["tool"], json!("export_font"));
    assert_eq!(
        plan["summary"],
        json!(format!("overwrites {}", text(&canon(&target))))
    );
    assert_eq!(plan["affected_count"], json!(0));
    assert_eq!(fs::read(&target).expect("read"), b"keep me");
    let committed = commit(&mut engine, plan).expect("commit");
    assert_eq!(committed["plan_committed"], json!(true));
    assert_eq!(&fs::read(&target).expect("read")[..4], &[0, 1, 0, 0]);

    // A dry run for a new file plans the write without writing.
    let planned = ok(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "woff2", "path": "new.woff2",
                "dry_run": true }),
    );
    let fresh = canon(root.path()).join("new.woff2");
    assert_eq!(
        planned["plan"]["summary"],
        json!(format!("writes {}", text(&fresh)))
    );
    assert!(!fresh.exists());
    commit(&mut engine, &planned["plan"]).expect("commit");
    assert!(fresh.is_file());

    // The schema declares `dry_run` and the planned answer.
    let spec = forma_engine::find("export_font").expect("registered");
    let input = (spec.input_schema)();
    assert_eq!(input["properties"]["dry_run"]["default"], json!(false));
    assert_eq!(input["additionalProperties"], json!(false));
    let output = spec.output_schema.expect("output schema")();
    assert!(output["properties"]["plan"].is_object(), "{output}");
}

#[test]
fn export_ufo_over_non_ufo_dir_is_rejected() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = Engine::new(EngineConfig {
        policy: Policy::Auto,
        ..config(root.path())
    });
    let doc = doc_with_a(&mut engine);
    let folder = root.path().join("notes.ufo");
    fs::create_dir(&folder).expect("mkdir");
    fs::write(folder.join("keep.txt"), b"precious").expect("write");

    for dry_run in [false, true] {
        let error = err(
            &mut engine,
            "export_font",
            json!({ "doc": doc, "base_rev": 1, "format": "ufo", "path": "notes.ufo",
                    "dry_run": dry_run }),
        );
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
        assert!(
            error
                .message
                .contains("destination exists and is not a UFO"),
            "{}",
            error.message
        );
    }
    assert_eq!(
        fs::read(folder.join("keep.txt")).expect("read"),
        b"precious"
    );
    assert_eq!(fs::read_dir(&folder).expect("list").count(), 1);

    // An existing UFO is replaced, after approval.
    ok(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "ufo", "path": "font.ufo" }),
    );
    let error = err(
        &mut engine,
        "export_font",
        json!({ "doc": doc, "base_rev": 1, "format": "ufo", "path": "font.ufo" }),
    );
    let plan = plan_of(&error);
    commit(&mut engine, &plan).expect("commit");
    assert!(
        root.path()
            .join("font.ufo")
            .join("metainfo.plist")
            .is_file()
    );
}

// ------------------------------------------------------------------ previews

#[test]
fn preview_glyph_returns_png_image_output() {
    let mut engine = Engine::new(EngineConfig::default());
    let doc = doc_with_a(&mut engine);
    let master = default_master(&mut engine, &doc);

    let (png, summary) = image(
        &mut engine,
        "preview_glyph",
        json!({ "doc": doc, "glyph": "A" }),
    );
    assert_eq!(png_size(&png), (512, 512));
    assert_eq!(
        summary,
        json!({ "rev": 1, "glyph": "A", "master": master, "size": 512 })
    );

    let (png, summary) = image(
        &mut engine,
        "preview_glyph",
        json!({ "doc": doc, "glyph": "A", "master": master, "size": 64, "show_nodes": true,
                "show_metrics": true, "format": "png" }),
    );
    assert_eq!(png_size(&png), (64, 64));
    assert_eq!(summary["size"], json!(64));

    // By id, too.
    let id = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "A" }),
    )["glyph"]["id"]
        .clone();
    let (_, summary) = image(
        &mut engine,
        "preview_glyph",
        json!({ "doc": doc, "glyph": id }),
    );
    assert_eq!(summary["glyph"], json!("A"));

    for size in [8, 1025, 4096] {
        let error = err(
            &mut engine,
            "preview_glyph",
            json!({ "doc": doc, "glyph": "A", "size": size }),
        );
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{size}");
    }
    let error = err(
        &mut engine,
        "preview_glyph",
        json!({ "doc": doc, "glyph": "Z" }),
    );
    assert_eq!(error.code, ErrorCode::NotFound);
    let error = err(
        &mut engine,
        "preview_glyph",
        json!({ "doc": doc, "glyph": "A", "master": "9f1c1b8e-4a4e-4c55-9b7e-3f1f3a1d2c3b" }),
    );
    assert_eq!(error.code, ErrorCode::NotFound);
    let error = err(
        &mut engine,
        "preview_glyph",
        json!({ "doc": doc, "glyph": "A", "format": "jpeg" }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    let error = err(
        &mut engine,
        "preview_glyph",
        json!({ "doc": "doc_x", "glyph": "A" }),
    );
    assert_eq!(error.code, ErrorCode::NotFound);
}

#[test]
fn preview_svg_returns_json() {
    let mut engine = Engine::new(EngineConfig::default());
    let doc = doc_with_a(&mut engine);
    let master = default_master(&mut engine, &doc);

    let svg = ok(
        &mut engine,
        "preview_glyph",
        json!({ "doc": doc, "glyph": "A", "format": "svg", "size": 128, "show_nodes": true }),
    );
    assert_eq!(svg["rev"], json!(1));
    assert_eq!(svg["glyph"], json!("A"));
    assert_eq!(svg["master"], json!(master));
    let markup = svg["svg"].as_str().expect("svg text");
    assert!(markup.starts_with("<svg"), "{markup}");
    assert!(markup.contains("<path"), "{markup}");
    assert!(markup.contains("id=\"nodes\""), "{markup}");
    let keys: Vec<&String> = svg.as_object().expect("object").keys().collect();
    assert_eq!(keys.len(), 4, "{svg}");
}

#[test]
fn preview_text_summary_reports_dimensions() {
    let mut engine = Engine::new(EngineConfig::default());
    let doc = doc_with_a(&mut engine);

    let (png, summary) = image(
        &mut engine,
        "preview_text",
        json!({ "doc": doc, "text": "AAA" }),
    );
    let (width, height) = png_size(&png);
    assert_eq!(height, 128);
    assert!(width > 128, "{width}");
    assert_eq!(
        summary,
        json!({ "rev": 1, "text": "AAA", "width": width, "height": 128 })
    );

    let (png, summary) = image(
        &mut engine,
        "preview_text",
        json!({ "doc": doc, "text": "A?", "height": 64 }),
    );
    assert_eq!(png_size(&png).1, 64);
    assert_eq!(summary["height"], json!(64));
    assert_eq!(summary["width"], json!(png_size(&png).0));

    let error = err(
        &mut engine,
        "preview_text",
        json!({ "doc": doc, "text": "" }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    let error = err(
        &mut engine,
        "preview_text",
        json!({ "doc": doc, "text": "A", "height": 2048 }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);

    // At most 4096 characters (not bytes): more is refused, never cut.
    let longest = "Á".repeat(4096);
    let (_, summary) = image(
        &mut engine,
        "preview_text",
        json!({ "doc": doc, "text": longest, "height": 16 }),
    );
    assert_eq!(summary["text"], json!(longest));
    let error = err(
        &mut engine,
        "preview_text",
        json!({ "doc": doc, "text": "A".repeat(4097), "height": 16 }),
    );
    assert_eq!(error.code, ErrorCode::LimitExceeded, "{error:?}");
    assert_eq!(error.details, json!({ "limit": 4096, "actual": 4097 }));
    let schema = (forma_engine::find("preview_text")
        .expect("tool")
        .input_schema)();
    assert_eq!(schema["properties"]["text"]["maxLength"], json!(4096));
}

#[test]
fn preview_compare_needs_valid_revs() {
    let mut engine = Engine::new(EngineConfig::default());
    let doc = doc_with_a(&mut engine);
    ok(
        &mut engine,
        "metrics_set",
        json!({ "doc": doc, "base_rev": 1, "items": [{ "glyph": "A", "lsb": 40 }] }),
    );

    let (png, summary) = image(
        &mut engine,
        "preview_compare",
        json!({ "doc": doc, "glyph": "A", "from_rev": 1 }),
    );
    assert_eq!(png_size(&png), (512, 512));
    assert_eq!(
        summary,
        json!({ "rev": 2, "glyph": "A", "from_rev": 1, "to_rev": 2, "size": 512 })
    );
    let (png, summary) = image(
        &mut engine,
        "preview_compare",
        json!({ "doc": doc, "glyph": "A", "from_rev": 2, "to_rev": 1, "size": 100,
                "show_nodes": true, "show_metrics": true }),
    );
    assert_eq!(png_size(&png), (100, 100));
    assert_eq!(summary["to_rev"], json!(1));
    // A glyph that exists at one revision only is drawn alone.
    let (_, summary) = image(
        &mut engine,
        "preview_compare",
        json!({ "doc": doc, "glyph": "A", "from_rev": 0 }),
    );
    assert_eq!(summary["from_rev"], json!(0));

    for args in [
        json!({ "doc": doc, "glyph": "A", "from_rev": 3 }),
        json!({ "doc": doc, "glyph": "A", "from_rev": 0, "to_rev": 99 }),
        json!({ "doc": doc, "glyph": "A", "from_rev": u64::MAX }),
    ] {
        let error = err(&mut engine, "preview_compare", args.clone());
        assert_eq!(error.code, ErrorCode::NotFound, "{args}");
        assert!(error.message.contains("revision"), "{}", error.message);
    }
    let error = err(
        &mut engine,
        "preview_compare",
        json!({ "doc": doc, "glyph": "Q", "from_rev": 0 }),
    );
    assert_eq!(error.code, ErrorCode::NotFound);
    let error = err(
        &mut engine,
        "preview_compare",
        json!({ "doc": doc, "glyph": "A", "from_rev": 1, "size": 2 }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[test]
fn render_error_mapping() {
    let mut engine = Engine::new(EngineConfig::default());
    let doc = new_doc(&mut engine);
    // An advance so wide that the glyph box cannot be framed.
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "wide", "advance_width": 1e12,
                "svg_path": SQUARE }),
    );
    for format in ["png", "svg"] {
        let error = err(
            &mut engine,
            "preview_glyph",
            json!({ "doc": doc, "glyph": "wide", "format": format }),
        );
        assert_eq!(
            error.code,
            ErrorCode::InvalidArgument,
            "{format}: {error:?}"
        );
        assert!(
            error.message.contains("degenerate framing"),
            "{}",
            error.message
        );
    }
    let error = err(
        &mut engine,
        "preview_compare",
        json!({ "doc": doc, "glyph": "wide", "from_rev": 0 }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[test]
fn image_tools_have_no_output_schema_and_strict_inputs() {
    for name in ["preview_glyph", "preview_text", "preview_compare"] {
        let spec = forma_engine::find(name).expect("registered");
        assert!(spec.output_schema.is_none(), "{name}");
        let input = (spec.input_schema)();
        assert_eq!(input["additionalProperties"], json!(false), "{name}");
        assert_eq!(input["required"][0], json!("doc"), "{name}: {input}");
    }
    let glyph = (forma_engine::find("preview_glyph")
        .expect("registered")
        .input_schema)();
    assert_eq!(glyph["properties"]["size"]["default"], json!(512));
    assert_eq!(glyph["properties"]["size"]["maximum"], json!(1024));
    let text_schema = (forma_engine::find("preview_text")
        .expect("registered")
        .input_schema)();
    assert_eq!(text_schema["properties"]["height"]["default"], json!(128));
}

// ------------------------------------------------------------------- registry

#[test]
fn no_placeholder_handlers_remain() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_in(root.path());
    for spec in registry() {
        if let Err(error) = engine.call(&actor(), spec.name, json!({})) {
            assert_ne!(
                error.message, "not implemented yet",
                "{} is still a placeholder",
                spec.name
            );
            assert_ne!(
                error.code,
                ErrorCode::Unsupported,
                "{}: {error:?}",
                spec.name
            );
        }
        // Every input schema is declared (not the placeholder's bare object).
        let input = (spec.input_schema)();
        assert_eq!(input["additionalProperties"], json!(false), "{}", spec.name);
    }
}

// ------------------------------------------------------------- response size

#[test]
fn mutation_results_bound_their_touched_lists() {
    let mut engine = Engine::new(EngineConfig {
        policy: Policy::Auto,
        ..EngineConfig::default()
    });
    let doc = new_doc(&mut engine);
    let ops: Vec<Value> = (0..500)
        .map(|index| {
            json!({ "tool": "glyph_create",
                    "args": { "name": format!("g{index:03}"), "advance_width": 500,
                              "svg_path": SQUARE } })
        })
        .collect();
    let created = ok(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": 0, "ops": ops }),
    );
    assert_eq!(created["touched"].as_array().expect("touched").len(), 100);
    assert_eq!(created["touched_count"], json!(501));
    assert_eq!(created["touched_truncated"], json!(true));

    let items: Vec<Value> = (0..500)
        .map(|index| json!({ "glyph": format!("g{index:03}"), "advance_width": 640 }))
        .collect();
    let set = ok(
        &mut engine,
        "metrics_set",
        json!({ "doc": doc, "base_rev": 1, "items": items }),
    );
    let size = serde_json::to_string(&set).expect("encode").len();
    assert!(size < 8 * 1024, "metrics_set result is {size} bytes");
    assert_eq!(set["touched"].as_array().expect("touched").len(), 100);
    assert_eq!(set["touched_count"], json!(500));
    assert_eq!(set["touched_truncated"], json!(true));

    // A small change lists everything.
    let small = ok(
        &mut engine,
        "metrics_set",
        json!({ "doc": doc, "base_rev": 2, "items": [{ "glyph": "g000", "advance_width": 1 }] }),
    );
    assert_eq!(small["touched"].as_array().expect("touched").len(), 1);
    assert_eq!(small["touched_count"], json!(1));
    assert_eq!(small["touched_truncated"], json!(false));

    // The entries of history_list are bounded the same way.
    let history = ok(&mut engine, "history_list", json!({ "doc": doc }));
    let size = serde_json::to_string(&history).expect("encode").len();
    assert!(size < 16 * 1024, "history_list result is {size} bytes");
    let entries = history["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["touched_count"], json!(1));
    assert_eq!(entries[0]["touched_truncated"], json!(false));
    for entry in &entries[1..] {
        assert_eq!(entry["touched"].as_array().expect("touched").len(), 100);
        assert_eq!(entry["touched_truncated"], json!(true));
    }
    assert_eq!(entries[2]["touched_count"], json!(501));
}

// ----------------------------------------------------------------- audit log

#[test]
fn audit_log_lines_written() {
    let root = TempDir::new().expect("tempdir");
    let log = root.path().join("audit.jsonl");
    let mut engine = Engine::new(EngineConfig {
        audit_log: Some(log.clone()),
        ..config(root.path())
    });
    let secret_outline = "M100 0 L512 0 L512 777 L100 777 Z";

    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Secret Family" }),
    );
    let doc = created["doc"].as_str().expect("doc").to_owned();
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "A", "advance_width": 600,
                "svg_path": secret_outline }),
    );
    ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "A" }),
    );
    image(
        &mut engine,
        "preview_glyph",
        json!({ "doc": doc, "glyph": "A", "size": 32 }),
    );
    ok(&mut engine, "forma_capabilities", json!({}));
    ok(
        &mut engine,
        "document_close",
        json!({ "doc": doc, "discard_changes": true,
                                              "dry_run": true }),
    );

    let lines = audit_lines(&log);
    assert_eq!(lines.len(), 6);
    let tools: Vec<&str> = lines
        .iter()
        .map(|line| line["tool"].as_str().expect("tool"))
        .collect();
    assert_eq!(
        tools,
        [
            "document_create",
            "glyph_create",
            "glyph_get",
            "preview_glyph",
            "forma_capabilities",
            "document_close"
        ]
    );
    for line in &lines {
        let keys: Vec<&str> = line
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            [
                "actor",
                "args_sha256",
                "doc",
                "error_code",
                "ok",
                "rev_after",
                "rev_before",
                "tool",
                "ts"
            ],
            "{line}"
        );
        assert_eq!(line["actor"], json!("agent:test"));
        assert_eq!(line["ok"], json!(true));
        assert_eq!(line["error_code"], Value::Null);
        let digest = line["args_sha256"].as_str().expect("digest");
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        let ts = line["ts"].as_str().expect("ts");
        assert!(
            ts.len() == 20 && ts.ends_with('Z') && ts.as_bytes()[10] == b'T',
            "{ts}"
        );
    }
    // The document and its revisions around each call.
    assert_eq!(lines[0]["doc"], json!(doc));
    assert_eq!(lines[0]["rev_before"], Value::Null);
    assert_eq!(lines[0]["rev_after"], json!(0));
    assert_eq!(lines[1]["doc"], json!(doc));
    assert_eq!(lines[1]["rev_before"], json!(0));
    assert_eq!(lines[1]["rev_after"], json!(1));
    assert_eq!(lines[2]["rev_before"], json!(1));
    assert_eq!(lines[2]["rev_after"], json!(1));
    assert_eq!(lines[4]["doc"], Value::Null);
    assert_eq!(lines[4]["rev_before"], Value::Null);
    // Different arguments, different digests; never the arguments themselves.
    assert_ne!(lines[1]["args_sha256"], lines[2]["args_sha256"]);
    let raw = fs::read_to_string(&log).expect("read");
    assert!(!raw.contains("L512 777"), "{raw}");
    assert!(!raw.contains("Secret Family"), "{raw}");
    assert!(raw.ends_with('\n'));
}

#[test]
fn audit_log_records_failures_too() {
    let root = TempDir::new().expect("tempdir");
    let log = root.path().join("audit.jsonl");
    fs::write(&log, "{\"earlier\":true}\n").expect("write");
    let mut engine = Engine::new(EngineConfig {
        audit_log: Some(log.clone()),
        ..config(root.path())
    });
    let doc = doc_with_a(&mut engine);

    err(&mut engine, "no_such_tool", json!({}));
    err(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "missing" }),
    );
    err(
        &mut engine,
        "glyph_get",
        json!({ "doc": "doc_forged_handle", "glyph": "A" }),
    );
    err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": 1, "glyphs": ["A"] }),
    );
    err(&mut engine, "glyph_get", json!([1, 2]));

    let lines = audit_lines(&log);
    // The earlier content is kept: the log is appended to.
    assert_eq!(lines[0], json!({ "earlier": true }));
    let lines = &lines[3..]; // after the earlier line, document_create and glyph_create
    assert_eq!(lines.len(), 5);
    let codes: Vec<&Value> = lines.iter().map(|line| &line["error_code"]).collect();
    assert_eq!(
        codes,
        [
            &json!("INVALID_ARGUMENT"),
            &json!("NOT_FOUND"),
            &json!("NOT_FOUND"),
            &json!("APPROVAL_REQUIRED"),
            &json!("INVALID_ARGUMENT"),
        ]
    );
    for line in lines {
        assert_eq!(line["ok"], json!(false), "{line}");
    }
    assert_eq!(lines[0]["tool"], json!("no_such_tool"));
    assert_eq!(lines[1]["doc"], json!(doc));
    assert_eq!(lines[1]["rev_before"], json!(1));
    assert_eq!(lines[1]["rev_after"], json!(1));
    // A handle that names no open document is not recorded.
    assert_eq!(lines[2]["doc"], Value::Null);
    assert_eq!(lines[3]["doc"], json!(doc));
    assert_eq!(lines[3]["rev_after"], json!(1));

    // A log that cannot be written never fails a call.
    let mut engine = Engine::new(EngineConfig {
        audit_log: Some(root.path().join("missing").join("audit.jsonl")),
        ..config(root.path())
    });
    let doc = doc_with_a(&mut engine);
    ok(&mut engine, "validate", json!({ "doc": doc }));
    assert!(!root.path().join("missing").exists());
}
