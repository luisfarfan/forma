//! Component, compose, decompose, stroke and overlap tools.

use forma_engine::{DocHandle, Engine, EngineConfig, ErrorCode, Policy, ToolError, ToolOutput};
use forma_model::{Actor, Font};
use serde_json::{Value, json};

// ------------------------------------------------------------------ helpers

fn actor() -> Actor {
    Actor::Agent("test".into())
}

fn engine() -> Engine {
    Engine::new(EngineConfig {
        policy: Policy::Prompted,
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

fn rev_of(engine: &mut Engine, doc: &str) -> u64 {
    ok(engine, "document_inspect", json!({ "doc": doc }))["rev"]
        .as_u64()
        .expect("rev")
}

fn rect_svg(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    format!("M{x0} {y0} L{x1} {y0} L{x1} {y1} L{x0} {y1} Z")
}

/// A document (UPM 1000) with `a` (50..450 x 0..500), the mark `acutecomb`
/// (0..100 x 600..700) and `cedillacomb` (0..100 x -200..-100).
fn font_with_marks(engine: &mut Engine) -> String {
    let created = ok(engine, "document_create", json!({ "family_name": "Test" }));
    let doc = created["doc"].as_str().expect("doc").to_owned();
    let glyphs = [
        ("a", rect_svg(50.0, 0.0, 450.0, 500.0), 500),
        ("acutecomb", rect_svg(0.0, 600.0, 100.0, 700.0), 0),
        ("cedillacomb", rect_svg(0.0, -200.0, 100.0, -100.0), 0),
    ];
    for (name, svg, advance) in glyphs {
        let rev = rev_of(engine, &doc);
        ok(
            engine,
            "glyph_create",
            json!({ "doc": doc, "base_rev": rev, "name": name, "advance_width": advance,
                "svg_path": svg }),
        );
    }
    doc
}

/// The bounding box `[x0, y0, x1, y1]` of the nodes of contour `index`.
fn contour_box(glyph: &Value, index: usize) -> [f64; 4] {
    let nodes = glyph["paths"][index]["nodes"].as_array().expect("nodes");
    let xs: Vec<f64> = nodes.iter().map(|n| n["x"].as_f64().expect("x")).collect();
    let ys: Vec<f64> = nodes.iter().map(|n| n["y"].as_f64().expect("y")).collect();
    let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    [min(&xs), min(&ys), max(&xs), max(&ys)]
}

fn glyph(engine: &mut Engine, doc: &str, name: &str) -> Value {
    ok(engine, "glyph_get", json!({ "doc": doc, "glyph": name }))["glyph"].clone()
}

fn compose_aacute(engine: &mut Engine, doc: &str) -> Value {
    let rev = rev_of(engine, doc);
    ok(
        engine,
        "glyph_compose",
        json!({ "doc": doc, "base_rev": rev, "name": "aacute", "unicodes": ["U+00E1"],
            "base": "a", "marks": [{ "glyph": "acutecomb", "position": "above" }] }),
    )
}

fn stroke_args(doc: &str, rev: u64, glyph: &str, mode: &str) -> Value {
    json!({ "doc": doc, "base_rev": rev, "glyph": glyph, "width": 80,
        "cap": { "type": "round" }, "mode": mode,
        "skeletons": [{ "nodes": [
            { "x": 100, "y": 100, "kind": "line" },
            { "x": 500, "y": 100, "kind": "line" }
        ] }] })
}

fn empty_glyph(engine: &mut Engine, doc: &str, name: &str) {
    let rev = rev_of(engine, doc);
    ok(
        engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": rev, "name": name, "advance_width": 600 }),
    );
}

// ------------------------------------------------------------------- tests

#[test]
fn compose_aacute_places_mark_above() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    compose_aacute(&mut engine, &doc);
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "glyph_decompose",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["aacute"] }),
    );
    let aacute = glyph(&mut engine, &doc, "aacute");
    assert_eq!(aacute["advance_width"], json!(500.0));
    // Gap: 4% of 1000 = 40. Base top 500, so the mark starts at 540.
    let mark = contour_box(&aacute, 1);
    assert!((mark[1] - 540.0).abs() <= 1.0, "{mark:?}");
    // Base centre x = 250; the mark is 100 wide.
    let centre = (mark[0] + mark[2]) / 2.0;
    assert!((centre - 250.0).abs() <= 1.0, "{mark:?}");
    // The base is where it was.
    assert_eq!(contour_box(&aacute, 0), [50.0, 0.0, 450.0, 500.0]);
}

#[test]
fn compose_below_and_center_and_explicit_gap() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "glyph_compose",
        json!({ "doc": doc, "base_rev": rev, "name": "ac", "base": "a", "marks": [
            { "glyph": "cedillacomb", "position": "below", "gap": 10 },
            { "glyph": "acutecomb", "position": "center" }
        ] }),
    );
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "glyph_decompose",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["ac"] }),
    );
    let ac = glyph(&mut engine, &doc, "ac");
    let below = contour_box(&ac, 1);
    assert!((below[3] - -10.0).abs() <= 1.0, "{below:?}");
    let centred = contour_box(&ac, 2);
    assert!(((centred[1] + centred[3]) / 2.0 - 250.0).abs() <= 1.0);
}

#[test]
fn compose_result_has_id_and_components() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    let result = compose_aacute(&mut engine, &doc);
    assert_eq!(result["glyph"]["name"], json!("aacute"));
    let id = result["glyph"]["id"].as_str().expect("id").to_owned();
    assert!(result["tx"].is_u64());
    let fetched = glyph(&mut engine, &doc, &id);
    assert_eq!(fetched["name"], json!("aacute"));
    assert_eq!(fetched["unicodes"], json!([0xE1]));
    // Two components (base and mark), no contours of its own.
    let handle = DocHandle::from(doc.as_str());
    let font: &Font = engine.doc(&handle).expect("doc").font();
    let layer = font
        .glyph_by_name("aacute")
        .and_then(|g| g.layers.values().next())
        .expect("layer");
    assert_eq!(layer.components.len(), 2);
    assert!(layer.paths.is_empty());
}

#[test]
fn compose_errors_are_clean() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    empty_glyph(&mut engine, &doc, "blank");
    let rev = rev_of(&mut engine, &doc);
    let base = |base: &str, mark: &str, marks: Value| {
        json!({ "doc": doc, "base_rev": rev, "name": "x", "base": base,
            "marks": if marks.is_null() { json!([{ "glyph": mark, "position": "above" }]) } else { marks } })
    };
    let blank = err(
        &mut engine,
        "glyph_compose",
        base("blank", "acutecomb", Value::Null),
    );
    assert_eq!(blank.code, ErrorCode::InvalidArgument);
    let missing = err(
        &mut engine,
        "glyph_compose",
        base("zz", "acutecomb", Value::Null),
    );
    assert_eq!(missing.code, ErrorCode::NotFound);
    let none = err(&mut engine, "glyph_compose", base("a", "", json!([])));
    assert_eq!(none.code, ErrorCode::InvalidArgument);
}

#[test]
fn component_set_replacing_requires_plan() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    empty_glyph(&mut engine, &doc, "target");
    let set = |rev: u64, dx: f64| {
        json!({ "doc": doc, "base_rev": rev, "glyph": "target", "components": [
            { "base": "a", "transform": { "xx": 1, "xy": 0, "yx": 0, "yy": 1, "dx": dx, "dy": 0 } }
        ] })
    };
    // The first call fills an empty component list: not destructive.
    let rev = rev_of(&mut engine, &doc);
    ok(&mut engine, "component_set", set(rev, 0.0));
    // The second replaces existing components: it needs a plan.
    let rev = rev_of(&mut engine, &doc);
    let refused = err(&mut engine, "component_set", set(rev, 10.0));
    assert_eq!(refused.code, ErrorCode::ApprovalRequired);
    let token = refused.details["plan"]["plan_token"].clone();
    let done = ok(&mut engine, "plan_commit", json!({ "plan_token": token }));
    assert_eq!(done["plan_committed"], json!(true));
    // An empty list removes the components (destructive again).
    let rev = rev_of(&mut engine, &doc);
    let cleared = err(
        &mut engine,
        "component_set",
        json!({ "doc": doc, "base_rev": rev, "glyph": "target", "components": [] }),
    );
    assert_eq!(cleared.code, ErrorCode::ApprovalRequired);
}

#[test]
fn component_set_rejects_a_cycle_and_unknown_base() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    let cycle = err(
        &mut engine,
        "component_set",
        json!({ "doc": doc, "base_rev": rev, "glyph": "a", "components": [{ "base": "a" }] }),
    );
    assert_eq!(cycle.code, ErrorCode::InvalidArgument, "{cycle:?}");
    let unknown = err(
        &mut engine,
        "component_set",
        json!({ "doc": doc, "base_rev": rev, "glyph": "a", "components": [{ "base": "nope" }] }),
    );
    assert_eq!(unknown.code, ErrorCode::NotFound);
}

#[test]
fn decompose_tool_matches_render() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    compose_aacute(&mut engine, &doc);
    let svg = |engine: &mut Engine| {
        ok(
            engine,
            "preview_glyph",
            json!({ "doc": doc, "glyph": "aacute", "format": "svg" }),
        )["svg"]
            .as_str()
            .expect("svg")
            .to_owned()
    };
    let before = svg(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    let done = ok(
        &mut engine,
        "glyph_decompose",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["aacute"] }),
    );
    assert!(done["tx"].is_u64());
    assert_eq!(
        glyph(&mut engine, &doc, "aacute")["paths"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(svg(&mut engine), before);
    // Decomposing again changes nothing.
    let rev = rev_of(&mut engine, &doc);
    let again = ok(
        &mut engine,
        "glyph_decompose",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["aacute"] }),
    );
    assert_eq!(again["tx"], Value::Null);
    assert_eq!(again["rev"], json!(rev));
}

#[test]
fn stroke_tool_creates_clean_outline() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    empty_glyph(&mut engine, &doc, "bar");
    let rev = rev_of(&mut engine, &doc);
    let done = ok(
        &mut engine,
        "glyph_stroke",
        stroke_args(&doc, rev, "bar", "replace"),
    );
    assert!(done["tx"].is_u64());
    let bar = glyph(&mut engine, &doc, "bar");
    let outline = contour_box(&bar, 0);
    // A round-capped bar: 100-40 .. 500+40 wide, 100 +- 40 tall.
    assert!((outline[0] - 60.0).abs() <= 1.0, "{outline:?}");
    assert!((outline[2] - 540.0).abs() <= 1.0, "{outline:?}");
    let check = ok(
        &mut engine,
        "geometry_check",
        json!({ "doc": doc, "glyphs": ["bar"], "grid": 0 }),
    );
    assert_eq!(check["issues"], json!([]), "{check}");
}

#[test]
fn stroke_replacing_a_drawn_layer_requires_plan() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    let refused = err(
        &mut engine,
        "glyph_stroke",
        stroke_args(&doc, rev, "a", "replace"),
    );
    assert_eq!(refused.code, ErrorCode::ApprovalRequired);
    // Appending destroys nothing.
    ok(
        &mut engine,
        "glyph_stroke",
        stroke_args(&doc, rev, "a", "append"),
    );
}

#[test]
fn stroke_append_unions_with_existing() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    // The bar crosses the rectangle `a` (50..450 x 0..500): one contour.
    ok(
        &mut engine,
        "glyph_stroke",
        stroke_args(&doc, rev, "a", "append"),
    );
    let a = glyph(&mut engine, &doc, "a");
    assert_eq!(a["paths"].as_array().map(Vec::len), Some(1), "{a}");
    let outline = contour_box(&a, 0);
    assert!((outline[0] - 50.0).abs() <= 1.0 && (outline[2] - 540.0).abs() <= 1.0);
}

#[test]
fn stroke_degenerate_is_invalid_argument() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    empty_glyph(&mut engine, &doc, "bar");
    let rev = rev_of(&mut engine, &doc);
    let one_node = json!({ "doc": doc, "base_rev": rev, "glyph": "bar", "width": 80,
        "cap": { "type": "round" },
        "skeletons": [{ "nodes": [{ "x": 1, "y": 1, "kind": "line" }] }] });
    let error = err(&mut engine, "glyph_stroke", one_node);
    assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
    let no_length = json!({ "doc": doc, "base_rev": rev, "glyph": "bar", "width": 80,
        "cap": { "type": "round" },
        "skeletons": [{ "nodes": [
            { "x": 1, "y": 1, "kind": "line" }, { "x": 1, "y": 1, "kind": "line" }] }] });
    assert_eq!(
        err(&mut engine, "glyph_stroke", no_length).code,
        ErrorCode::InvalidArgument
    );
    let mut zero_width = stroke_args(&doc, rev, "bar", "replace");
    zero_width["width"] = json!(0);
    assert_eq!(
        err(&mut engine, "glyph_stroke", zero_width).code,
        ErrorCode::InvalidArgument
    );
    let mut empty = stroke_args(&doc, rev, "bar", "replace");
    empty["skeletons"] = json!([]);
    assert_eq!(
        err(&mut engine, "glyph_stroke", empty).code,
        ErrorCode::InvalidArgument
    );
    let mut missing = stroke_args(&doc, rev, "nope", "replace");
    missing["glyph"] = json!("nope");
    assert_eq!(
        err(&mut engine, "glyph_stroke", missing).code,
        ErrorCode::NotFound
    );
}

#[test]
fn stroke_node_limit() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    empty_glyph(&mut engine, &doc, "bar");
    let rev = rev_of(&mut engine, &doc);
    let nodes: Vec<Value> = (0..10_001)
        .map(|i| json!({ "x": i % 500, "y": i / 500 * 10, "kind": "line" }))
        .collect();
    let mut args = stroke_args(&doc, rev, "bar", "replace");
    args["skeletons"] = json!([{ "nodes": nodes }]);
    let error = err(&mut engine, "glyph_stroke", args);
    assert_eq!(error.code, ErrorCode::LimitExceeded, "{error:?}");
    assert_eq!(error.details["limit"], json!(10_000));
    assert_eq!(error.details["actual"], json!(10_001));
}

#[test]
fn remove_overlap_unites_and_noop_returns_null_tx() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    empty_glyph(&mut engine, &doc, "two");
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "glyph_set_outline",
        json!({ "doc": doc, "base_rev": rev, "glyph": "two",
            "svg_path": format!("{} {}", rect_svg(0.0, 0.0, 100.0, 100.0),
                rect_svg(50.0, 50.0, 150.0, 150.0)) }),
    );
    let rev = rev_of(&mut engine, &doc);
    let done = ok(
        &mut engine,
        "remove_overlap",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["two"] }),
    );
    assert!(done["tx"].is_u64());
    assert_eq!(
        glyph(&mut engine, &doc, "two")["paths"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    // Nothing overlaps now: no transaction, same revision.
    let rev = rev_of(&mut engine, &doc);
    let noop = ok(
        &mut engine,
        "remove_overlap",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["two", "a"] }),
    );
    assert_eq!(noop["tx"], Value::Null, "{noop}");
    assert_eq!(noop["rev"], json!(rev));
    assert_eq!(rev_of(&mut engine, &doc), rev);
    assert_eq!(
        err(
            &mut engine,
            "remove_overlap",
            json!({ "doc": doc, "base_rev": rev, "glyphs": [] })
        )
        .code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn batch_can_compose_and_stroke() {
    let mut engine = engine();
    let doc = font_with_marks(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    let done = ok(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": rev, "ops": [
            { "tool": "glyph_create", "args": { "name": "bar", "advance_width": 600 } },
            { "tool": "glyph_stroke", "args": { "glyph": "bar", "width": 80,
                "cap": { "type": "round" },
                "skeletons": [{ "nodes": [
                    { "x": 100, "y": 100, "kind": "line" },
                    { "x": 500, "y": 100, "kind": "line" }] }] } },
            { "tool": "glyph_compose", "args": { "name": "abar", "base": "bar",
                "marks": [{ "glyph": "acutecomb", "position": "above" }] } },
            { "tool": "glyph_decompose", "args": { "glyphs": ["abar"] } },
            { "tool": "remove_overlap", "args": { "glyphs": ["abar"] } },
            { "tool": "component_set", "args": { "glyph": "abar", "components": [] } }
        ] }),
    );
    assert!(done["tx"].is_u64(), "{done}");
    let abar = glyph(&mut engine, &doc, "abar");
    // The bar (y 60..140) and the mark above it: two separate contours.
    assert_eq!(abar["paths"].as_array().map(Vec::len), Some(2));
    let mark = contour_box(&abar, 1);
    assert!((mark[1] - 180.0).abs() <= 1.0, "{mark:?}");
}

#[test]
fn tools_with_empty_arguments_fail_cleanly() {
    let mut engine = engine();
    for tool in [
        "component_set",
        "glyph_compose",
        "glyph_decompose",
        "glyph_stroke",
        "remove_overlap",
    ] {
        let error = err(&mut engine, tool, json!({}));
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{tool}");
    }
}
