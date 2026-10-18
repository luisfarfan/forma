//! `style_extract` and `glyph_synthesize`.

use std::path::{Path, PathBuf};

use forma_engine::{Engine, EngineConfig, ErrorCode, Policy, ToolError, ToolOutput};
use forma_model::Actor;
use serde_json::{Value, json};

fn actor() -> Actor {
    Actor::Agent("test".into())
}

fn setup(policy: Policy) -> (Engine, tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/hito2/specimen.png");
    let image = dir.path().join("specimen.png");
    std::fs::copy(source, &image).expect("copy the specimen");
    let engine = Engine::new(EngineConfig {
        allowed_roots: vec![dir.path().to_path_buf()],
        policy,
        ..EngineConfig::default()
    });
    (engine, dir, image)
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

fn rev(engine: &mut Engine, doc: &str) -> u64 {
    ok(engine, "document_inspect", json!({ "doc": doc }))["rev"]
        .as_u64()
        .expect("rev")
}

/// A document with the specimen's eight letters traced, nothing else.
fn traced_doc(engine: &mut Engine, image: &Path) -> String {
    let doc = ok(
        engine,
        "document_create",
        json!({ "family_name": "Specimen" }),
    )["doc"]
        .as_str()
        .expect("doc")
        .to_owned();
    let base = rev(engine, &doc);
    ok(
        engine,
        "font_from_image",
        json!({ "doc": doc, "base_rev": base, "path": image, "layout": ["ABCH", "OFMR"],
            "stages": { "synthesize": false, "compose": false, "kern": false } }),
    );
    doc
}

#[test]
fn style_extract_measures_the_traced_letters() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = traced_doc(&mut engine, &image);
    let out = ok(&mut engine, "style_extract", json!({ "doc": doc }));
    let kit = &out["kit"];
    assert_eq!(
        kit["sources"].as_object().map(|s| s.len()),
        Some(8),
        "{out}"
    );
    assert!(kit["stroke"].as_f64() > Some(20.0), "{out}");
    assert!(out["proportion"].as_f64().is_some());
    assert_eq!(out["rev"], rev(&mut engine, &doc));
}

#[test]
fn synthesize_makes_pending_glyphs_and_lists_what_needs_a_design() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = traced_doc(&mut engine, &image);
    let base = rev(&mut engine, &doc);
    let out = ok(
        &mut engine,
        "glyph_synthesize",
        json!({ "doc": doc, "base_rev": base }),
    );
    assert_eq!(out["rev"], base + 1, "{out}");
    assert!(out["made_total"].as_u64() > Some(30), "{out}");
    let needs: Vec<&str> = out["needs_design"]
        .as_array()
        .expect("needs_design")
        .iter()
        .filter_map(|n| n["name"].as_str())
        .collect();
    for name in ["ampersand", "at", "Euro", "dollar"] {
        assert!(needs.contains(&name), "{name}: {out}");
    }
    assert!(out["report_path"].is_string(), "{out}");
    let zero = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "zero" }),
    );
    let p = &zero["glyph"]["provenance"];
    assert_eq!(p["review"], "pending", "{zero}");
    assert_eq!(p["detail"]["recipe"], "zero");
    assert!(
        p["detail"]["sources"]
            .as_array()
            .is_some_and(|s| s.contains(&json!("O"))),
        "{zero}"
    );
    // Asked again, nothing is left to make: every glyph exists or needs a
    // design.
    let base = rev(&mut engine, &doc);
    let again = ok(
        &mut engine,
        "glyph_synthesize",
        json!({ "doc": doc, "base_rev": base, "glyphs": ["zero"] }),
    );
    assert_eq!(again["made_total"], 0, "{again}");
}

#[test]
fn a_font_without_traced_glyphs_cannot_be_learned_from() {
    let (mut engine, _dir, _image) = setup(Policy::Auto);
    let doc = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Empty" }),
    )["doc"]
        .as_str()
        .expect("doc")
        .to_owned();
    let base = rev(&mut engine, &doc);
    let error = call(
        &mut engine,
        "glyph_synthesize",
        json!({ "doc": doc, "base_rev": base }),
    )
    .expect_err("no sources");
    assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
    assert!(error.message.contains("traced"), "{error:?}");
}

#[test]
fn the_threshold_cannot_be_lowered() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = traced_doc(&mut engine, &image);
    let base = rev(&mut engine, &doc);
    let error = call(
        &mut engine,
        "glyph_synthesize",
        json!({ "doc": doc, "base_rev": base, "min_score": 0.5 }),
    )
    .expect_err("too low");
    assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
    let error = call(
        &mut engine,
        "glyph_synthesize",
        json!({ "doc": doc, "base_rev": base, "glyphs": ["nosuch"] }),
    )
    .expect_err("no recipe");
    assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
}
