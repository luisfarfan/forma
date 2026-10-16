//! Registry contract, document lifecycle tools and path sandbox.

use std::path::{Path, PathBuf};

use forma_engine::{
    Effects, Engine, EngineConfig, ErrorCode, OutputKind, Policy, ToolError, ToolOutput, Toolset,
    registry,
};
use forma_model::{Actor, Font};
use serde_json::{Value, json};
use tempfile::TempDir;

// ------------------------------------------------------------------ helpers

fn actor() -> Actor {
    Actor::Agent("test".into())
}

fn engine_with(roots: &[&Path]) -> Engine {
    Engine::new(EngineConfig {
        allowed_roots: roots.iter().map(|root| root.to_path_buf()).collect(),
        ..EngineConfig::default()
    })
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

fn canon(path: &Path) -> PathBuf {
    dunce::canonicalize(path).expect("canonicalize")
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn doc_of(value: &Value) -> String {
    value["doc"].as_str().expect("doc handle").to_owned()
}

/// Writes a font to `path` directly through the store.
fn write_font(path: &Path, family: &str) {
    forma_store::save(&Font::new(family, 1000), path).expect("save fixture");
}

const PLACEHOLDERS: [&str; 28] = [
    "font_set_info",
    "font_set_vertical_metrics",
    "edit_batch",
    "plan_commit",
    "plan_discard",
    "glyph_list",
    "glyph_get",
    "glyph_create",
    "glyph_update",
    "glyph_delete",
    "glyph_set_outline",
    "contour_edit",
    "nodes_edit",
    "geometry_check",
    "geometry_fix",
    "metrics_get",
    "metrics_set",
    "validate",
    "export_font",
    "preview_glyph",
    "preview_text",
    "preview_compare",
    "history_list",
    "history_undo",
    "history_redo",
    "history_revert",
    "history_restore",
    "document_diff",
];

// ----------------------------------------------------------------- registry

#[test]
fn registry_order_and_names_match_spec() {
    let expected = vec![
        "forma_capabilities",
        "document_create",
        "document_open",
        "document_save",
        "document_close",
        "document_list",
        "document_inspect",
        "font_set_info",
        "font_set_vertical_metrics",
        "edit_batch",
        "plan_commit",
        "plan_discard",
        "glyph_list",
        "glyph_get",
        "glyph_create",
        "glyph_update",
        "glyph_delete",
        "glyph_set_outline",
        "contour_edit",
        "nodes_edit",
        "geometry_check",
        "geometry_fix",
        "metrics_get",
        "metrics_set",
        "validate",
        "export_font",
        "preview_glyph",
        "preview_text",
        "preview_compare",
        "history_list",
        "history_undo",
        "history_redo",
        "history_revert",
        "history_restore",
        "document_diff",
    ];
    let names: Vec<&str> = registry().iter().map(|spec| spec.name).collect();
    assert_eq!(names, expected);
}

#[test]
fn all_names_le_40_chars_snake_case() {
    for spec in registry() {
        let name = spec.name;
        assert!(!name.is_empty() && name.len() <= 40, "{name}: length");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{name}: not snake_case"
        );
        assert!(
            name.starts_with(|c: char| c.is_ascii_lowercase())
                && !name.ends_with('_')
                && !name.contains("__"),
            "{name}: not snake_case"
        );
    }
}

#[test]
fn every_spec_input_schema_is_object() {
    for spec in registry() {
        let schema = (spec.input_schema)();
        assert_eq!(schema["type"], json!("object"), "{}: {schema}", spec.name);
        assert!(
            schema.get("$schema").is_none(),
            "{}: $schema kept",
            spec.name
        );
    }
}

#[test]
fn implemented_schemas_deny_unknown_fields_and_describe_results() {
    let implemented = [
        "forma_capabilities",
        "document_create",
        "document_open",
        "document_save",
        "document_close",
        "document_list",
        "document_inspect",
    ];
    for name in implemented {
        let spec = forma_engine::find(name).expect("registered");
        let input = (spec.input_schema)();
        assert_eq!(
            input["additionalProperties"],
            json!(false),
            "{name}: {input}"
        );
        let output = spec
            .output_schema
            .expect("JSON tools have an output schema")();
        assert_eq!(output["type"], json!("object"), "{name}: {output}");
        assert!(output.get("$schema").is_none(), "{name}: $schema kept");
    }
    let create = (forma_engine::find("document_create")
        .expect("registered")
        .input_schema)();
    assert_eq!(create["required"], json!(["family_name"]));
    assert_eq!(create["properties"]["units_per_em"]["default"], json!(1000));
}

#[test]
fn toolsets_effects_and_outputs_match_rulings() {
    use Toolset::*;
    let expected_toolset = |name: &str| -> Toolset {
        match name {
            "forma_capabilities"
            | "document_create"
            | "document_open"
            | "document_save"
            | "document_close"
            | "document_list"
            | "document_inspect"
            | "font_set_info"
            | "font_set_vertical_metrics"
            | "edit_batch"
            | "plan_commit"
            | "plan_discard" => Core,
            "glyph_list" | "glyph_get" | "glyph_create" | "glyph_update" | "glyph_delete"
            | "glyph_set_outline" | "contour_edit" | "nodes_edit" => Glyphs,
            "geometry_check" | "geometry_fix" => Geometry,
            "metrics_get" | "metrics_set" => Spacing,
            "validate" => Quality,
            "export_font" => Export,
            "preview_glyph" | "preview_text" | "preview_compare" => Preview,
            "history_list" | "history_undo" | "history_redo" | "history_revert"
            | "history_restore" | "document_diff" => History,
            other => panic!("unexpected tool {other}"),
        }
    };
    let read_only = [
        "forma_capabilities",
        "document_open",
        "document_list",
        "document_inspect",
        "glyph_list",
        "glyph_get",
        "geometry_check",
        "metrics_get",
        "validate",
        "preview_glyph",
        "preview_text",
        "preview_compare",
        "history_list",
        "document_diff",
    ];
    let destructive = [
        "glyph_delete",
        "glyph_set_outline",
        "history_restore",
        "plan_commit",
    ];
    let images = ["preview_glyph", "preview_text", "preview_compare"];
    for spec in registry() {
        let name = spec.name;
        assert_eq!(spec.toolset, expected_toolset(name), "{name}: toolset");
        let is_read = read_only.contains(&name);
        assert_eq!(
            spec.effects,
            Effects {
                read_only: is_read,
                destructive: destructive.contains(&name),
                idempotent: is_read,
            },
            "{name}: effects"
        );
        let output = if images.contains(&name) {
            OutputKind::Image
        } else {
            OutputKind::Json
        };
        assert_eq!(spec.output, output, "{name}: output kind");
        if spec.output == OutputKind::Image {
            assert!(
                spec.output_schema.is_none(),
                "{name}: image with output schema"
            );
        }
    }
}

#[test]
fn descriptions_are_bounded_and_mutating_tools_explain_base_rev() {
    for spec in registry() {
        let name = spec.name;
        assert!(
            !spec.title.is_empty() && spec.title.len() <= 40,
            "{name}: title"
        );
        assert!(
            !spec.description.is_empty() && spec.description.chars().count() <= 1000,
            "{name}: description length"
        );
        let schema = (spec.input_schema)();
        if schema["properties"].get("base_rev").is_some() {
            assert!(spec.description.contains("base_rev"), "{name}: base_rev");
        }
    }
    // The placeholders of mutating tools that will take `base_rev` already
    // explain it.
    for name in [
        "font_set_info",
        "glyph_create",
        "metrics_set",
        "history_restore",
    ] {
        let spec = forma_engine::find(name).expect("registered");
        assert!(spec.description.contains("REVISION_CONFLICT"), "{name}");
    }
}

#[test]
fn placeholder_tools_return_unsupported() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[root.path()]);
    for name in PLACEHOLDERS {
        let error = err(&mut engine, name, json!({}));
        assert_eq!(
            error,
            ToolError {
                code: ErrorCode::Unsupported,
                message: "not implemented yet".into(),
                details: Value::Null,
            },
            "{name}"
        );
    }
}

#[test]
fn engine_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Engine>();
}

// ------------------------------------------------------------------ engine

#[test]
fn unknown_tool_is_invalid_argument() {
    let mut engine = engine_with(&[]);
    let error = err(&mut engine, "glyph_explode", json!({}));
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, "unknown tool 'glyph_explode'");
}

#[test]
fn unknown_argument_is_invalid_argument() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[root.path()]);
    let error = err(&mut engine, "document_list", json!({ "verbose": true }));
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("verbose"), "{}", error.message);

    let error = err(
        &mut engine,
        "document_create",
        json!({ "family_name": "Test", "bogus": 1 }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("bogus"), "{}", error.message);

    // A missing or mistyped field is reported the same way.
    let error = err(&mut engine, "document_create", json!({}));
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("family_name"), "{}", error.message);

    // Arguments must be an object, for every tool.
    for tool in ["document_list", "glyph_get"] {
        let error = err(&mut engine, tool, json!(["doc_1"]));
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{tool}");
        assert_eq!(error.message, "invalid arguments: expected a JSON object");
    }
    assert!(engine.open_docs().is_empty());
}

#[test]
fn null_arguments_are_an_empty_object() {
    let mut engine = engine_with(&[]);
    assert_eq!(
        ok(&mut engine, "document_list", Value::Null),
        json!({ "documents": [] })
    );
}

#[test]
fn capabilities_report_limits() {
    let mut engine = Engine::new(EngineConfig {
        policy: Policy::Auto,
        ..EngineConfig::default()
    });
    let caps = ok(&mut engine, "forma_capabilities", json!({}));
    assert_eq!(caps["api_version"], json!("1.0.0"));
    assert_eq!(caps["server_version"], json!(env!("CARGO_PKG_VERSION")));
    assert_eq!(
        caps["toolsets"],
        json!([
            "core", "glyphs", "geometry", "spacing", "quality", "export", "preview", "history"
        ])
    );
    assert_eq!(
        caps["limits"],
        json!({
            "max_batch_ops": 2000,
            "max_nodes": 100000,
            "max_text_bytes": 4194304,
            "mass_threshold": 20,
            "plan_ttl_secs": 300
        })
    );
    assert_eq!(caps["formats"], json!(["forma", "formapackage"]));
    assert_eq!(caps["policy"], json!("auto"));
    assert_eq!(caps["read_only"], json!(false));
}

#[test]
fn read_only_mode_blocks_mutations() {
    let root = TempDir::new().expect("tempdir");
    let existing = root.path().join("existing.forma");
    write_font(&existing, "Existing");
    let mut engine = Engine::new(EngineConfig {
        allowed_roots: vec![root.path().to_path_buf()],
        read_only: true,
        ..EngineConfig::default()
    });

    let error = err(
        &mut engine,
        "document_create",
        json!({ "family_name": "X" }),
    );
    assert_eq!(error.code, ErrorCode::Unsupported);
    assert_eq!(error.message, "server is in read-only mode");
    assert!(engine.open_docs().is_empty());

    // Read-only tools still work, including opening (in-memory state only).
    let caps = ok(&mut engine, "forma_capabilities", json!({}));
    assert_eq!(caps["read_only"], json!(true));
    let opened = ok(
        &mut engine,
        "document_open",
        json!({ "path": text(&existing) }),
    );
    let doc = doc_of(&opened);
    ok(&mut engine, "document_inspect", json!({ "doc": doc }));
    ok(&mut engine, "document_list", json!({}));

    for (tool, args) in [
        ("document_save", json!({ "doc": doc, "base_rev": 0 })),
        ("glyph_create", json!({ "doc": doc, "base_rev": 0 })),
        ("history_undo", json!({ "doc": doc })),
    ] {
        let error = err(&mut engine, tool, args);
        assert_eq!(error.code, ErrorCode::Unsupported, "{tool}");
        assert_eq!(error.message, "server is in read-only mode", "{tool}");
    }
}

#[test]
fn unknown_handle_is_not_found() {
    let mut engine = engine_with(&[]);
    for (tool, args) in [
        (
            "document_inspect",
            json!({ "doc": "doc_00000000000000000000000000000000" }),
        ),
        ("document_save", json!({ "doc": "doc_nope", "base_rev": 0 })),
        ("document_close", json!({ "doc": "doc_nope" })),
    ] {
        let error = err(&mut engine, tool, args);
        assert_eq!(error.code, ErrorCode::NotFound, "{tool}");
        assert_eq!(error.message, "unknown document handle", "{tool}");
    }
}

#[test]
fn missing_roots_are_dropped() {
    let root = TempDir::new().expect("tempdir");
    let missing = root.path().join("does-not-exist");
    let engine = engine_with(&[&missing, root.path()]);
    assert_eq!(engine.allowed_roots(), &[canon(root.path())]);
}

// ---------------------------------------------------------------- documents

#[test]
fn create_save_open_roundtrip() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[root.path()]);

    for file in ["roundtrip.forma", "roundtrip.formapackage"] {
        let created = ok(
            &mut engine,
            "document_create",
            json!({ "family_name": "Roundtrip Sans", "units_per_em": 2048 }),
        );
        let doc = doc_of(&created);
        assert!(doc.starts_with("doc_") && doc.len() == 36, "{doc}");
        assert!(doc[4..].chars().all(|c| c.is_ascii_hexdigit()), "{doc}");
        assert_eq!(created["rev"], json!(0));
        assert_eq!(created["path"], Value::Null);

        let inspected = ok(&mut engine, "document_inspect", json!({ "doc": doc }));
        assert_eq!(inspected["dirty"], json!(true));
        assert_eq!(inspected["path"], Value::Null);

        let target = root.path().join(file);
        let saved = ok(
            &mut engine,
            "document_save",
            json!({ "doc": doc, "base_rev": 0, "path": text(&target) }),
        );
        let expected_path = text(&canon(&target));
        assert_eq!(
            saved,
            json!({ "doc": doc, "rev": 0, "path": expected_path })
        );
        assert!(target.exists());
        // Saving again without a path writes to the same file.
        let again = ok(
            &mut engine,
            "document_save",
            json!({ "doc": doc, "base_rev": 0 }),
        );
        assert_eq!(again["path"], json!(expected_path));

        let closed = ok(&mut engine, "document_close", json!({ "doc": doc }));
        assert_eq!(closed, json!({ "doc": doc, "rev": 0, "closed": true }));

        let opened = ok(
            &mut engine,
            "document_open",
            json!({ "path": text(&target) }),
        );
        assert_eq!(opened["rev"], json!(0));
        assert_eq!(opened["path"], json!(expected_path));
        assert_eq!(opened["already_open"], json!(false));
        let reopened = doc_of(&opened);
        assert_ne!(reopened, doc, "handles are not reused");

        let inspected = ok(&mut engine, "document_inspect", json!({ "doc": reopened }));
        assert_eq!(inspected["doc"], json!(reopened));
        assert_eq!(inspected["rev"], json!(0));
        assert_eq!(inspected["info"]["family_name"], json!("Roundtrip Sans"));
        assert_eq!(inspected["info"]["style_name"], json!("Regular"));
        assert_eq!(inspected["units_per_em"], json!(2048));
        assert_eq!(inspected["masters"].as_array().map(Vec::len), Some(1));
        assert_eq!(inspected["masters"][0]["name"], json!("Regular"));
        assert_eq!(inspected["glyph_count"], json!(0));
        assert_eq!(inspected["dirty"], json!(false));
        assert_eq!(inspected["path"], json!(expected_path));

        ok(&mut engine, "document_close", json!({ "doc": reopened }));
    }
    assert!(engine.open_docs().is_empty());
}

#[test]
fn create_with_path_writes_immediately_and_lists() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[root.path()]);
    let target = root.path().join("new.forma");
    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Fresh", "path": text(&target) }),
    );
    assert!(target.is_file());
    assert_eq!(created["path"], json!(text(&canon(&target))));
    let doc = doc_of(&created);
    let memory = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Memory" }),
    ));

    let listed = ok(&mut engine, "document_list", json!({}));
    let mut expected = vec![
        json!({ "doc": doc, "path": text(&canon(&target)), "family_name": "Fresh", "rev": 0, "dirty": false }),
        json!({ "doc": memory, "path": null, "family_name": "Memory", "rev": 0, "dirty": true }),
    ];
    expected.sort_by(|a, b| a["doc"].as_str().cmp(&b["doc"].as_str()));
    assert_eq!(listed, json!({ "documents": expected }));
    assert_eq!(engine.open_docs().len(), 2);

    // The file loads back with the requested defaults.
    let font = forma_store::load(&target).expect("load");
    assert_eq!(font.info.family_name, "Fresh");
    assert_eq!(font.units_per_em, 1000);
}

#[test]
fn create_rejects_invalid_values() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[root.path()]);
    for args in [
        json!({ "family_name": "X", "units_per_em": 8 }),
        json!({ "family_name": "X", "units_per_em": 20000 }),
        json!({ "family_name": "X", "units_per_em": 70000 }),
        json!({ "family_name": "   " }),
    ] {
        let error = err(&mut engine, "document_create", args.clone());
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{args}");
    }
    assert!(engine.open_docs().is_empty());
}

#[test]
fn create_with_existing_path_is_rejected() {
    let root = TempDir::new().expect("tempdir");
    let existing = root.path().join("taken.forma");
    write_font(&existing, "Taken");
    let before = std::fs::read(&existing).expect("read");
    let mut engine = engine_with(&[root.path()]);

    let error = err(
        &mut engine,
        "document_create",
        json!({ "family_name": "Other", "path": text(&existing) }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("already exists; use document_open"),
        "{}",
        error.message
    );
    assert_eq!(
        std::fs::read(&existing).expect("read"),
        before,
        "file untouched"
    );
    assert!(engine.open_docs().is_empty());
}

#[test]
fn open_same_path_twice_returns_same_handle() {
    let root = TempDir::new().expect("tempdir");
    let path = root.path().join("shared.formapackage");
    write_font(&path, "Shared");
    let mut engine = engine_with(&[root.path()]);

    let first = ok(&mut engine, "document_open", json!({ "path": text(&path) }));
    assert_eq!(first["already_open"], json!(false));
    // A different spelling of the same file (relative to the first root).
    let second = ok(
        &mut engine,
        "document_open",
        json!({ "path": "./shared.formapackage" }),
    );
    assert_eq!(second["doc"], first["doc"]);
    assert_eq!(second["rev"], first["rev"]);
    assert_eq!(second["path"], first["path"]);
    assert_eq!(second["already_open"], json!(true));
    assert_eq!(engine.open_docs().len(), 1);
}

#[test]
fn open_missing_or_invalid_file_fails() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[root.path()]);
    let error = err(
        &mut engine,
        "document_open",
        json!({ "path": text(&root.path().join("missing.forma")) }),
    );
    assert_eq!(error.code, ErrorCode::NotFound);

    let garbage = root.path().join("garbage.forma");
    std::fs::write(&garbage, b"not a database").expect("write");
    let error = err(
        &mut engine,
        "document_open",
        json!({ "path": text(&garbage) }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(engine.open_docs().is_empty());
}

#[test]
fn document_paths_need_a_native_extension() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[root.path()]);
    for name in ["font.ttf", "font", "font.forma.bak"] {
        let error = err(
            &mut engine,
            "document_create",
            json!({ "family_name": "X", "path": text(&root.path().join(name)) }),
        );
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{name}");
    }
    // The extension is case-insensitive.
    ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "X", "path": text(&root.path().join("UPPER.FORMA")) }),
    );
}

#[test]
fn save_validates_base_rev_and_requires_a_path() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[root.path()]);
    let doc = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "X" }),
    ));

    let error = err(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 0 }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("path required"), "{}", error.message);

    let error = err(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 1, "path": text(&root.path().join("x.forma")) }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(!root.path().join("x.forma").exists());

    let inspected = ok(&mut engine, "document_inspect", json!({ "doc": doc }));
    assert_eq!(
        inspected["dirty"],
        json!(true),
        "a failed save keeps the document dirty"
    );
}

#[test]
fn save_as_over_existing_file_requires_approval() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[root.path()]);
    let a = root.path().join("a.forma");
    let b = root.path().join("b.forma");
    write_font(&a, "A");
    let doc_b = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "B", "path": text(&b) }),
    ));
    let before = std::fs::read(&a).expect("read");

    let error = err(
        &mut engine,
        "document_save",
        json!({ "doc": doc_b, "base_rev": 0, "path": text(&a) }),
    );
    assert_eq!(error.code, ErrorCode::ApprovalRequired);
    assert_eq!(
        error.details,
        json!({ "reason": "overwrites an existing file" })
    );
    assert_eq!(
        std::fs::read(&a).expect("read"),
        before,
        "a.forma untouched"
    );

    // Saving over the document's own file is not an overwrite of another file.
    let saved = ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc_b, "base_rev": 0, "path": text(&b) }),
    );
    assert_eq!(saved["path"], json!(text(&canon(&b))));

    // Save as to a new file moves the document there.
    let c = root.path().join("c.formapackage");
    ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc_b, "base_rev": 0, "path": text(&c) }),
    );
    let inspected = ok(&mut engine, "document_inspect", json!({ "doc": doc_b }));
    assert_eq!(inspected["path"], json!(text(&canon(&c))));
}

#[test]
fn a_file_is_never_open_under_two_handles() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[root.path()]);
    let a = root.path().join("a.forma");
    let doc_a = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "A", "path": text(&a) }),
    ));
    let doc_b = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "B" }),
    ));

    // Save as onto the file of another open document.
    let error = err(
        &mut engine,
        "document_save",
        json!({ "doc": doc_b, "base_rev": 0, "path": text(&a) }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains(&doc_a), "{}", error.message);

    // Even when that file has been deleted behind the engine's back.
    std::fs::remove_file(&a).expect("remove");
    let error = err(
        &mut engine,
        "document_save",
        json!({ "doc": doc_b, "base_rev": 0, "path": text(&a) }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    let error = err(
        &mut engine,
        "document_create",
        json!({ "family_name": "C", "path": text(&a) }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(!a.exists());

    // The owner itself can still save there.
    ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc_a, "base_rev": 0 }),
    );
    assert!(a.exists());
}

#[test]
fn close_dirty_requires_discard() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[root.path()]);
    let doc = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Draft" }),
    ));

    let error = err(&mut engine, "document_close", json!({ "doc": doc }));
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(
        error.message,
        "unsaved changes; save first or pass discard_changes: true"
    );
    assert_eq!(engine.open_docs().len(), 1, "still open");

    let closed = ok(
        &mut engine,
        "document_close",
        json!({ "doc": doc, "discard_changes": true }),
    );
    assert_eq!(closed["closed"], json!(true));
    assert!(engine.open_docs().is_empty());
    let error = err(&mut engine, "document_inspect", json!({ "doc": doc }));
    assert_eq!(error.code, ErrorCode::NotFound);
}

// -------------------------------------------------------------------- paths

#[test]
fn path_outside_roots_is_rejected() {
    let root = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("tempdir");
    let existing = outside.path().join("secret.forma");
    write_font(&existing, "Secret");
    let mut engine = engine_with(&[root.path()]);

    let error = err(
        &mut engine,
        "document_open",
        json!({ "path": text(&existing) }),
    );
    assert_eq!(error.code, ErrorCode::PathNotAllowed);

    let target = outside.path().join("new.forma");
    let error = err(
        &mut engine,
        "document_create",
        json!({ "family_name": "X", "path": text(&target) }),
    );
    assert_eq!(error.code, ErrorCode::PathNotAllowed);
    assert!(!target.exists());

    // Escaping with `..` from inside the root.
    let escape = root
        .path()
        .join("..")
        .join(outside.path().file_name().expect("name"))
        .join("dotdot.forma");
    let error = err(
        &mut engine,
        "document_create",
        json!({ "family_name": "X", "path": text(&escape) }),
    );
    assert_eq!(error.code, ErrorCode::PathNotAllowed);
    let error = err(
        &mut engine,
        "document_create",
        json!({ "family_name": "X", "path": "../escape.forma" }),
    );
    assert_eq!(error.code, ErrorCode::PathNotAllowed);

    // Save as outside the roots.
    let doc = doc_of(&ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "X" }),
    ));
    let error = err(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 0, "path": text(&target) }),
    );
    assert_eq!(error.code, ErrorCode::PathNotAllowed);
    assert!(!target.exists());

    // A missing parent outside the roots does not reveal anything either.
    let deep = outside.path().join("no").join("such").join("dir.forma");
    let error = err(
        &mut engine,
        "document_create",
        json!({ "family_name": "X", "path": text(&deep) }),
    );
    assert_eq!(error.code, ErrorCode::PathNotAllowed);

    // Inside the roots, a missing parent is an invalid argument.
    let nested = root.path().join("no").join("such").join("dir.forma");
    let error = err(
        &mut engine,
        "document_create",
        json!({ "family_name": "X", "path": text(&nested) }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, "parent directory does not exist");

    // The sandbox is also available to later tools directly.
    assert_eq!(
        engine.check_allowed(&existing, true).map_err(|e| e.code),
        Err(ErrorCode::PathNotAllowed)
    );
    let inside = root.path().join("inside.forma");
    assert_eq!(
        engine.check_allowed(&inside, false),
        Ok(canon(root.path()).join("inside.forma"))
    );
    assert_eq!(
        engine.check_allowed(&inside, true).map_err(|e| e.code),
        Err(ErrorCode::NotFound)
    );
}

#[test]
fn no_roots_refuse_every_path() {
    let root = TempDir::new().expect("tempdir");
    let existing = root.path().join("font.forma");
    write_font(&existing, "Font");
    let mut engine = engine_with(&[]);
    let error = err(
        &mut engine,
        "document_open",
        json!({ "path": text(&existing) }),
    );
    assert_eq!(error.code, ErrorCode::PathNotAllowed);
    let error = err(
        &mut engine,
        "document_create",
        json!({ "family_name": "X", "path": "relative.forma" }),
    );
    assert_eq!(error.code, ErrorCode::PathNotAllowed);
}

#[test]
fn relative_path_resolves_against_first_root() {
    let first = TempDir::new().expect("tempdir");
    let second = TempDir::new().expect("tempdir");
    let mut engine = engine_with(&[first.path(), second.path()]);

    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Rel", "path": "rel.forma" }),
    );
    let expected = canon(first.path()).join("rel.forma");
    assert_eq!(created["path"], json!(text(&expected)));
    assert!(expected.is_file());
    assert!(!second.path().join("rel.forma").exists());

    // Absolute paths inside the second root are allowed too.
    let other = second.path().join("other.forma");
    ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Other", "path": text(&other) }),
    );
    assert!(other.is_file());
}

#[test]
#[cfg_attr(windows, ignore)]
fn symlink_escape_is_rejected() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().expect("tempdir");
        let outside = TempDir::new().expect("tempdir");
        let secret = outside.path().join("secret.forma");
        write_font(&secret, "Secret");
        let mut engine = engine_with(&[root.path()]);

        // A link to a file outside the roots.
        let file_link = root.path().join("link.forma");
        symlink(&secret, &file_link).expect("symlink");
        let error = err(
            &mut engine,
            "document_open",
            json!({ "path": text(&file_link) }),
        );
        assert_eq!(error.code, ErrorCode::PathNotAllowed);

        // A link to a directory outside the roots.
        let dir_link = root.path().join("outdir");
        symlink(outside.path(), &dir_link).expect("symlink");
        let through = dir_link.join("planted.forma");
        let error = err(
            &mut engine,
            "document_create",
            json!({ "family_name": "X", "path": text(&through) }),
        );
        assert_eq!(error.code, ErrorCode::PathNotAllowed);
        assert!(!outside.path().join("planted.forma").exists());
        let error = err(
            &mut engine,
            "document_open",
            json!({ "path": "outdir/secret.forma" }),
        );
        assert_eq!(error.code, ErrorCode::PathNotAllowed);

        // A dangling link inside the root, pointing outside.
        let dangling = root.path().join("dangling.forma");
        symlink(outside.path().join("future.forma"), &dangling).expect("symlink");
        let error = err(
            &mut engine,
            "document_create",
            json!({ "family_name": "X", "path": text(&dangling) }),
        );
        assert_eq!(error.code, ErrorCode::PathNotAllowed);
        assert!(!outside.path().join("future.forma").exists());

        // A link that stays inside the roots is fine.
        let inner = root.path().join("real.forma");
        write_font(&inner, "Real");
        let inner_link = root.path().join("alias.forma");
        symlink(&inner, &inner_link).expect("symlink");
        let opened = ok(
            &mut engine,
            "document_open",
            json!({ "path": text(&inner_link) }),
        );
        assert_eq!(opened["path"], json!(text(&canon(&inner))));
    }
}
