//! Image inspection, tracing, comparison and style measurement tools.

use std::path::{Path, PathBuf};

use forma_engine::{Engine, EngineConfig, ErrorCode, Policy, ToolError, ToolOutput};
use forma_model::Actor;
use serde_json::{Value, json};

// ------------------------------------------------------------------ helpers

fn actor() -> Actor {
    Actor::Agent("test".into())
}

/// An engine whose only allowed root is a fresh folder holding a copy of the
/// specimen image.
fn setup() -> (Engine, tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/hito2/specimen.png");
    let image = dir.path().join("specimen.png");
    std::fs::copy(source, &image).expect("copy the specimen");
    let engine = Engine::new(EngineConfig {
        allowed_roots: vec![dir.path().to_path_buf()],
        policy: Policy::Prompted,
        ..EngineConfig::default()
    });
    (engine, dir, image)
}

fn raw(engine: &mut Engine, tool: &str, args: Value) -> Result<ToolOutput, ToolError> {
    engine.call(&actor(), tool, args)
}

fn call(engine: &mut Engine, tool: &str, args: Value) -> Result<Value, ToolError> {
    match raw(engine, tool, args)? {
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
fn image(engine: &mut Engine, tool: &str, args: Value) -> (Vec<u8>, Value) {
    match raw(engine, tool, args) {
        Ok(ToolOutput::Image { png, summary }) => (png, summary),
        Ok(ToolOutput::Json(value)) => panic!("{tool} returned JSON: {value}"),
        Err(error) => panic!("{tool} failed: {error:?}"),
    }
}

fn rev_of(engine: &mut Engine, doc: &str) -> u64 {
    ok(engine, "document_inspect", json!({ "doc": doc }))["rev"]
        .as_u64()
        .expect("rev")
}

/// A document with UPM 1000, cap height 700.
fn new_doc(engine: &mut Engine) -> String {
    let created = ok(engine, "document_create", json!({ "family_name": "Test" }));
    created["doc"].as_str().expect("doc").to_owned()
}

fn assert_png(png: &[u8]) {
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
}

fn trace_args(doc: &str, rev: u64, path: &Path) -> Value {
    json!({ "doc": doc, "base_rev": rev, "path": path,
        "assignments": [
            { "candidate": 0, "name": "A", "unicodes": ["U+0041"] },
            { "candidate": 1, "name": "B", "unicodes": ["U+0042"] }
        ] })
}

// ------------------------------------------------------------------- tests

#[test]
fn inspect_returns_annotated_png_and_eight_candidates() {
    let (mut engine, _dir, path) = setup();
    let (png, summary) = image(&mut engine, "image_inspect", json!({ "path": path }));
    assert_png(&png);
    assert_eq!(
        summary["candidates"].as_array().expect("candidates").len(),
        8
    );
    assert_eq!(summary["rows"].as_array().expect("rows").len(), 2);
}

#[test]
fn trace_creates_glyphs_with_iou() {
    let (mut engine, _dir, path) = setup();
    let doc = new_doc(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    let result = ok(&mut engine, "image_trace", trace_args(&doc, rev, &path));
    let traced = result["traced"].as_array().expect("traced");
    assert_eq!(traced.len(), 2);
    assert_eq!(traced[0]["name"], json!("A"));
    assert_eq!(traced[0]["candidate"], json!(0));
    for entry in traced {
        assert!(entry["iou"].as_f64().expect("iou") > 0.9, "{entry}");
        assert!(entry["nodes"].as_u64().expect("nodes") > 3, "{entry}");
    }
    assert!(result["rev"].as_u64().expect("rev") > rev);
    let a = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "A" }),
    );
    assert_eq!(a["glyph"]["unicodes"], json!([65]));
    assert!(!a["glyph"]["paths"].as_array().expect("paths").is_empty());
    assert!(a["glyph"]["advance_width"].as_f64().expect("advance") > 400.0);
}

#[test]
fn trace_existing_glyph_needs_plan() {
    let (mut engine, _dir, path) = setup();
    let doc = new_doc(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": rev, "name": "A", "advance_width": 600,
            "svg_path": "M0 0 L100 0 L100 100 L0 100 Z" }),
    );
    let rev = rev_of(&mut engine, &doc);
    let error = call(&mut engine, "image_trace", trace_args(&doc, rev, &path))
        .expect_err("drawn glyph is replaced");
    assert_eq!(error.code, ErrorCode::ApprovalRequired, "{error:?}");
    // An empty existing glyph is not destructive.
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": rev, "name": "B", "advance_width": 600 }),
    );
    let rev = rev_of(&mut engine, &doc);
    let args = json!({ "doc": doc, "base_rev": rev, "path": path,
        "assignments": [{ "candidate": 1, "name": "B" }] });
    ok(&mut engine, "image_trace", args);
    let b = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "B" }),
    );
    assert!(!b["glyph"]["paths"].as_array().expect("paths").is_empty());
}

#[test]
fn trace_candidate_out_of_range_is_invalid_argument() {
    let (mut engine, _dir, path) = setup();
    let doc = new_doc(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    let args = json!({ "doc": doc, "base_rev": rev, "path": path,
        "assignments": [
            { "candidate": 0, "name": "A" },
            { "candidate": 99, "name": "Zed" }
        ] });
    let error = call(&mut engine, "image_trace", args).expect_err("out of range");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    // The error names the failing candidate and the name assigned to it.
    assert!(error.message.contains("99"), "{}", error.message);
    assert!(error.message.contains("Zed"), "{}", error.message);
    assert_eq!(error.details["candidate"], json!(99));
    assert_eq!(error.details["name"], json!("Zed"));
    // Nothing was created.
    assert_eq!(rev_of(&mut engine, &doc), rev);
}

#[test]
fn inspect_outside_allowed_dir_is_path_not_allowed() {
    let (mut engine, _dir, _path) = setup();
    let other = tempfile::tempdir().expect("tempdir");
    let outside = other.path().join("specimen.png");
    std::fs::write(&outside, b"not an image").expect("write");
    let error = raw(&mut engine, "image_inspect", json!({ "path": outside }))
        .expect_err("outside the roots");
    assert_eq!(error.code, ErrorCode::PathNotAllowed);
}

#[test]
fn oversized_file_is_limit_exceeded_without_reading() {
    let (mut engine, dir, _path) = setup();
    let big = dir.path().join("big.png");
    let file = std::fs::File::create(&big).expect("create");
    file.set_len(65 * 1024 * 1024).expect("sparse");
    let error = raw(&mut engine, "image_inspect", json!({ "path": big })).expect_err("too big");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
}

#[test]
fn garbage_image_and_bad_options_are_invalid_argument() {
    let (mut engine, dir, path) = setup();
    let junk = dir.path().join("junk.png");
    std::fs::write(&junk, b"definitely not a PNG").expect("write");
    let error = raw(&mut engine, "image_inspect", json!({ "path": junk })).expect_err("junk");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    for args in [
        json!({ "path": path, "color": "orange" }),
        json!({ "path": path, "tolerance": 0.5 }),
        json!({ "path": path, "tolerance": 101 }),
    ] {
        let error = raw(&mut engine, "image_inspect", args).expect_err("bad option");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }
    // An explicit ink color works.
    image(
        &mut engine,
        "image_inspect",
        json!({ "path": path, "color": "#FF5A1F", "tolerance": 30 }),
    );
}

#[test]
fn trace_compare_reports_iou() {
    let (mut engine, _dir, path) = setup();
    let doc = new_doc(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    ok(&mut engine, "image_trace", trace_args(&doc, rev, &path));
    let rev = rev_of(&mut engine, &doc);
    let (png, summary) = image(
        &mut engine,
        "trace_compare",
        json!({ "doc": doc, "path": path, "glyph": "A", "candidate": 0 }),
    );
    assert_png(&png);
    assert_eq!(summary["rev"], json!(rev));
    assert_eq!(summary["glyph"], json!("A"));
    assert_eq!(summary["candidate"], json!(0));
    assert!(summary["iou"].as_f64().expect("iou") > 0.9, "{summary}");
    // The wrong candidate fits worse.
    let (_, wrong) = image(
        &mut engine,
        "trace_compare",
        json!({ "doc": doc, "path": path, "glyph": "A", "candidate": 1 }),
    );
    assert!(wrong["iou"].as_f64().expect("iou") < summary["iou"].as_f64().expect("iou"));
}

#[test]
fn glyph_measure_reports_stroke() {
    let (mut engine, _dir, path) = setup();
    let doc = new_doc(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    // H is candidate 3: two stems and a bar, 80 units thick.
    let args = json!({ "doc": doc, "base_rev": rev, "path": path,
        "assignments": [{ "candidate": 3, "name": "H" }] });
    ok(&mut engine, "image_trace", args);
    let measure = ok(
        &mut engine,
        "glyph_measure",
        json!({ "doc": doc, "glyph": "H" }),
    );
    assert_eq!(measure["glyph"], json!("H"));
    assert!(measure["rev"].as_u64().is_some());
    let median = measure["stroke_median"].as_f64().expect("median");
    assert!((60.0..=100.0).contains(&median), "{measure}");
    assert!(measure["stroke_p10"].as_f64().expect("p10") <= median);
    assert!(measure["stroke_p90"].as_f64().expect("p90") >= median);
    assert!(measure.get("terminal_radius").is_some());
    assert!(measure.get("join_min").is_some());
    let missing = call(
        &mut engine,
        "glyph_measure",
        json!({ "doc": doc, "glyph": "Q" }),
    )
    .expect_err("unknown glyph");
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[test]
fn capabilities_report_image_limits() {
    let (mut engine, _dir, _path) = setup();
    let capabilities = ok(&mut engine, "forma_capabilities", json!({}));
    let limits = &capabilities["limits"];
    assert_eq!(limits["max_image_bytes"], json!(64 * 1024 * 1024));
    assert_eq!(limits["max_image_pixels"], json!(50_000_000));
    assert_eq!(limits["max_candidates"], json!(500));
    assert_eq!(limits["max_assignments"], json!(200));
    assert_eq!(limits["max_kerning_check_pairs"], json!(250_000));
    assert!(
        capabilities["toolsets"]
            .as_array()
            .expect("toolsets")
            .contains(&json!("trace"))
    );
}

#[test]
fn too_many_assignments_is_limit_exceeded() {
    let (mut engine, _dir, path) = setup();
    let doc = new_doc(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    let assignments: Vec<Value> = (0..201)
        .map(|i| json!({ "candidate": 0, "name": format!("g{i}") }))
        .collect();
    let args = json!({ "doc": doc, "base_rev": rev, "path": path, "assignments": assignments });
    let error = call(&mut engine, "image_trace", args).expect_err("too many");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
}
