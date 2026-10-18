//! Spacing, kerning and character set tools.

use std::time::Instant;

use forma_engine::{Engine, EngineConfig, ErrorCode, Policy, ToolError, ToolOutput};
use forma_model::Actor;
use serde_json::{Value, json};

// ------------------------------------------------------------------ helpers

fn actor() -> Actor {
    Actor::Agent("test".into())
}

fn engine_with(policy: Policy) -> Engine {
    Engine::new(EngineConfig {
        policy,
        ..EngineConfig::default()
    })
}

fn engine() -> Engine {
    engine_with(Policy::Prompted)
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

/// A V: the A of the Milestone 1 fixture turned over, without counter.
const V: &str = "M290 0 L390 0 L660 700 L560 700 L340 130 L120 700 L20 700 Z";

/// The glyphs of the Milestone 1 fixture (UPM 1000, cap height 700): name,
/// advance, code points and outline.
fn fixture_glyphs() -> Vec<(String, f64, Vec<u32>, Option<String>)> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/hito1/glyphs.json");
    let text = std::fs::read_to_string(path).expect("fixture");
    let json: Value = serde_json::from_str(&text).expect("json");
    json["glyphs"]
        .as_array()
        .expect("glyphs")
        .iter()
        .map(|glyph| {
            let unicodes = glyph["unicodes"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .map(|u| {
                            let text = u.as_str().expect("text");
                            u32::from_str_radix(&text[2..], 16).expect("hex")
                        })
                        .collect()
                })
                .unwrap_or_default();
            (
                glyph["name"].as_str().expect("name").to_owned(),
                glyph["advance_width"].as_f64().expect("advance"),
                unicodes,
                glyph["svg_path"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

/// A glyph-creating operation of an `edit_batch`.
fn create_op(name: &str, advance: f64, unicodes: &[u32], svg: Option<&str>) -> Value {
    let mut args = json!({ "name": name, "advance_width": advance, "unicodes": unicodes });
    if let Some(svg) = svg {
        args["svg_path"] = json!(svg);
    }
    json!({ "tool": "glyph_create", "args": args })
}

fn batch(engine: &mut Engine, doc: &str, ops: Vec<Value>) {
    let rev = rev_of(engine, doc);
    ok(
        engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": rev, "ops": ops }),
    );
}

/// A document with the fixture glyphs plus `V`, built under a policy that
/// lets the batch through.
fn fixture_doc(engine: &mut Engine) -> String {
    let created = ok(engine, "document_create", json!({ "family_name": "Test" }));
    let doc = created["doc"].as_str().expect("doc").to_owned();
    let mut ops: Vec<Value> = fixture_glyphs()
        .into_iter()
        .map(|(name, advance, unicodes, svg)| create_op(&name, advance, &unicodes, svg.as_deref()))
        .collect();
    ops.push(create_op("V", 680.0, &[0x56], Some(V)));
    batch(engine, &doc, ops);
    doc
}

/// Calls a tool; when it asks for approval, commits its plan.
#[track_caller]
fn approved(engine: &mut Engine, tool: &str, args: Value) -> Value {
    match call(engine, tool, args) {
        Ok(value) => value,
        Err(error) if error.code == ErrorCode::ApprovalRequired => {
            let token = error.details["plan"]["plan_token"].clone();
            ok(engine, "plan_commit", json!({ "plan_token": token }))
        }
        Err(error) => panic!("{tool} failed: {error:?}"),
    }
}

fn metrics(engine: &mut Engine, doc: &str, glyph: &str) -> Value {
    ok(
        engine,
        "metrics_get",
        json!({ "doc": doc, "glyphs": [glyph] }),
    )["metrics"][0]
        .clone()
}

fn pair_value(kerning: &Value, first: &str, second: &str) -> Option<f64> {
    kerning["pairs"]
        .as_array()
        .expect("pairs")
        .iter()
        .find(|p| p["first"] == json!(first) && p["second"] == json!(second))
        .map(|p| p["value"].as_f64().expect("value"))
}

// ------------------------------------------------------------------- spacing

#[test]
fn spacing_auto_sets_symmetric_h() {
    let mut engine = engine();
    let doc = fixture_doc(&mut engine);
    // Make H lopsided first.
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "metrics_set",
        json!({ "doc": doc, "base_rev": rev, "items": [{ "glyph": "H", "lsb": 20 }, { "glyph": "H", "rsb": 220 }] }),
    );
    let lopsided = metrics(&mut engine, &doc, "H");
    assert_ne!(lopsided["lsb"], lopsided["rsb"]);

    let rev = rev_of(&mut engine, &doc);
    let done = ok(
        &mut engine,
        "spacing_auto",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["H"] }),
    );
    assert_eq!(done["spaced"], json!(1));
    assert_eq!(done["clamped"], json!([]));
    let h = metrics(&mut engine, &doc, "H");
    assert_eq!(h["lsb"], h["rsb"], "{h}");
    // 0.6 of the stem (80 units) is the margin of H: 48.
    assert!((h["lsb"].as_f64().expect("lsb") - 48.0).abs() <= 1.0, "{h}");
}

#[test]
fn spacing_auto_reports_clamped_sides() {
    let mut engine = engine();
    let doc = fixture_doc(&mut engine);
    // A very open glyph: a bar so wide its white is already large, and a
    // tightness so small that its own sides ask for negative bearings is
    // impossible with a rectangle, so use a glyph with deep channels: V.
    let rev = rev_of(&mut engine, &doc);
    let done = ok(
        &mut engine,
        "spacing_auto",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["V", "A"], "tightness": 0.05 }),
    );
    let clamped = done["clamped"].as_array().expect("clamped");
    assert!(!clamped.is_empty(), "{done}");
    assert_eq!(done["clamped_total"], json!(clamped.len()));
    for entry in clamped {
        assert!(entry["left"] == json!(true) || entry["right"] == json!(true));
        let name = entry["glyph"].as_str().expect("glyph");
        let m = metrics(&mut engine, &doc, name);
        let side = if entry["left"] == json!(true) {
            "lsb"
        } else {
            "rsb"
        };
        assert_eq!(m[side], json!(0.0), "{name}: {m}");
    }
    // Out of range.
    let rev = rev_of(&mut engine, &doc);
    let bad = err(
        &mut engine,
        "spacing_auto",
        json!({ "doc": doc, "base_rev": rev, "tightness": 5 }),
    );
    assert_eq!(bad.code, ErrorCode::InvalidArgument);
}

#[test]
fn spacing_auto_mass_needs_plan() {
    let mut engine = engine();
    let doc = fixture_doc(&mut engine);
    let ops: Vec<Value> = (0..25)
        .map(|i| {
            create_op(
                &format!("bar{i}"),
                700.0,
                &[],
                Some(&rect_svg(10.0 + f64::from(i), 0.0, 300.0, 500.0)),
            )
        })
        .collect();
    for chunk in ops.chunks(10) {
        batch(&mut engine, &doc, chunk.to_vec());
    }

    let rev = rev_of(&mut engine, &doc);
    let args = json!({ "doc": doc, "base_rev": rev });
    let refused = err(&mut engine, "spacing_auto", args.clone());
    assert_eq!(refused.code, ErrorCode::ApprovalRequired, "{refused:?}");
    let mut planned_args = args.clone();
    planned_args["dry_run"] = json!(true);
    let planned = ok(&mut engine, "spacing_auto", planned_args);
    assert!(planned["plan"]["plan_token"].is_string(), "{planned}");
    assert_eq!(planned["rev"], json!(rev));
    // The plan commits.
    let token = refused.details["plan"]["plan_token"].clone();
    let done = ok(&mut engine, "plan_commit", json!({ "plan_token": token }));
    assert_eq!(done["plan_committed"], json!(true));
    assert!(rev_of(&mut engine, &doc) > rev);
}

#[test]
fn spacing_without_a_measurable_h_says_why() {
    let mut engine = engine();
    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "T" }),
    );
    let doc = created["doc"].as_str().expect("doc").to_owned();
    // H and O exist but H has ink only near the baseline, far from the
    // middle half of the cap height where its stem is measured.
    batch(
        &mut engine,
        &doc,
        vec![
            create_op("H", 700.0, &[], Some(&rect_svg(80.0, 0.0, 160.0, 100.0))),
            create_op("O", 700.0, &[], Some(&rect_svg(80.0, 0.0, 600.0, 700.0))),
        ],
    );
    let rev = rev_of(&mut engine, &doc);
    let error = err(
        &mut engine,
        "spacing_auto",
        json!({ "doc": doc, "base_rev": rev }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("no vertical stem"),
        "{}",
        error.message
    );
    assert!(!error.message.contains("no ink"), "{}", error.message);
    let rev = rev_of(&mut engine, &doc);
    let error = err(
        &mut engine,
        "kerning_auto",
        json!({ "doc": doc, "base_rev": rev }),
    );
    assert!(
        error.message.contains("no vertical stem"),
        "{}",
        error.message
    );
}

// ------------------------------------------------------------------- kerning

#[test]
fn kerning_set_and_get_roundtrip() {
    let mut engine = engine();
    let doc = fixture_doc(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "kerning_set",
        json!({ "doc": doc, "base_rev": rev,
            "groups": [
                { "name": "A", "side": "first", "members": ["A", "V"] },
                { "name": "V", "side": "second", "members": ["V"] }
            ],
            "pairs": [
                { "first": { "group": "A" }, "second": { "group": "V" }, "value": -60 },
                { "first": { "glyph": "H" }, "second": { "glyph": "O" }, "value": 12 }
            ] }),
    );
    let got = ok(&mut engine, "kerning_get", json!({ "doc": doc }));
    assert_eq!(got["groups_total"], json!(2));
    assert_eq!(
        got["groups"][0],
        json!({ "name": "A", "side": "first", "members": ["A", "V"] })
    );
    assert_eq!(got["groups"][1]["side"], json!("second"));
    assert_eq!(pair_value(&got, "@A", "@V"), Some(-60.0));
    assert_eq!(pair_value(&got, "H", "O"), Some(12.0));
    assert_eq!(got["rev"], json!(rev_of(&mut engine, &doc)));

    // Filtered by glyph: V belongs to group A, so @A/@V qualifies; H/O does not.
    let only_v = ok(
        &mut engine,
        "kerning_get",
        json!({ "doc": doc, "glyphs": ["V"] }),
    );
    assert_eq!(pair_value(&only_v, "@A", "@V"), Some(-60.0));
    assert_eq!(pair_value(&only_v, "H", "O"), None);

    // 0 removes a pair; deleting a group is destructive.
    let rev = rev_of(&mut engine, &doc);
    let removed = err(
        &mut engine,
        "kerning_set",
        json!({ "doc": doc, "base_rev": rev,
            "pairs": [{ "first": { "glyph": "H" }, "second": { "glyph": "O" }, "value": 0 }] }),
    );
    assert_eq!(removed.code, ErrorCode::ApprovalRequired);
    let token = removed.details["plan"]["plan_token"].clone();
    ok(&mut engine, "plan_commit", json!({ "plan_token": token }));
    let got = ok(&mut engine, "kerning_get", json!({ "doc": doc }));
    assert_eq!(pair_value(&got, "H", "O"), None);
}

#[test]
fn kerning_set_group_conflict_is_invalid_argument() {
    let mut engine = engine();
    let doc = fixture_doc(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    // The same glyph in two groups of one side.
    let conflict = err(
        &mut engine,
        "kerning_set",
        json!({ "doc": doc, "base_rev": rev, "groups": [
            { "name": "one", "side": "first", "members": ["A", "V"] },
            { "name": "two", "side": "first", "members": ["V"] }
        ] }),
    );
    assert_eq!(conflict.code, ErrorCode::InvalidArgument, "{conflict:?}");
    // Nothing was applied.
    assert_eq!(rev_of(&mut engine, &doc), rev);
    // At least one list.
    let empty = err(
        &mut engine,
        "kerning_set",
        json!({ "doc": doc, "base_rev": rev }),
    );
    assert_eq!(empty.code, ErrorCode::InvalidArgument);
    // An unknown glyph in a pair.
    let unknown = err(
        &mut engine,
        "kerning_set",
        json!({ "doc": doc, "base_rev": rev,
            "pairs": [{ "first": { "glyph": "zz" }, "second": { "glyph": "O" }, "value": 5 }] }),
    );
    assert_eq!(unknown.code, ErrorCode::NotFound);
}

#[test]
fn kerning_set_is_batchable() {
    let mut engine = engine();
    let doc = fixture_doc(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": rev, "ops": [
            { "tool": "kerning_set", "args": { "pairs": [
                { "first": { "glyph": "A" }, "second": { "glyph": "V" }, "value": -40 }] } }
        ] }),
    );
    let got = ok(&mut engine, "kerning_get", json!({ "doc": doc }));
    assert_eq!(pair_value(&got, "A", "V"), Some(-40.0));
}

#[test]
fn kerning_auto_then_check_has_no_collisions() {
    let mut engine = engine_with(Policy::Auto);
    let doc = fixture_doc(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    let done = ok(
        &mut engine,
        "kerning_auto",
        json!({ "doc": doc, "base_rev": rev }),
    );
    assert!(done["tx"].is_u64(), "{done}");
    let got = ok(&mut engine, "kerning_get", json!({ "doc": doc }));
    assert!(got["pairs_total"].as_u64().expect("total") > 0, "{got}");
    // A and V close up.
    let av = pair_value(&got, "A", "V").or_else(|| {
        // Or through groups: any negative pair counts.
        got["pairs"]
            .as_array()
            .expect("pairs")
            .iter()
            .filter_map(|p| p["value"].as_f64())
            .find(|v| *v < 0.0)
    });
    assert!(av.is_some_and(|v| v < 0.0), "{got}");

    // Defaults: overlaps only. And at the guard's own distance (2% of 1000).
    let check = ok(&mut engine, "kerning_check", json!({ "doc": doc }));
    assert_eq!(check["collisions"], json!([]), "{check}");
    assert!(check["checked_pairs"].as_u64().expect("pairs") > 100);
    let strict = ok(
        &mut engine,
        "kerning_check",
        json!({ "doc": doc, "min_distance": 20 }),
    );
    assert_eq!(strict["collisions"], json!([]), "{strict}");
    assert_eq!(strict["collisions_total"], json!(0));
}

#[test]
fn kerning_check_finds_overlaps_without_kerning() {
    let mut engine = engine();
    let doc = fixture_doc(&mut engine);
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "kerning_set",
        json!({ "doc": doc, "base_rev": rev,
            "pairs": [{ "first": { "glyph": "A" }, "second": { "glyph": "V" }, "value": -400 }] }),
    );
    let check = ok(
        &mut engine,
        "kerning_check",
        json!({ "doc": doc, "glyphs": ["A", "V", "H"] }),
    );
    assert_eq!(check["checked_pairs"], json!(9));
    let collisions = check["collisions"].as_array().expect("collisions");
    assert_eq!(collisions.len(), 1, "{check}");
    assert_eq!(collisions[0]["left"], json!("A"));
    assert_eq!(collisions[0]["right"], json!("V"));
    assert!(collisions[0]["distance"].as_f64().expect("distance") < 0.0);
}

#[test]
fn kerning_check_limits_pairs() {
    let mut engine = engine_with(Policy::Auto);
    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "T" }),
    );
    let doc = created["doc"].as_str().expect("doc").to_owned();
    let ops: Vec<Value> = (0..501)
        .map(|i| {
            create_op(
                &format!("g{i}"),
                500.0,
                &[],
                Some(&rect_svg(50.0, 0.0, 450.0, 500.0)),
            )
        })
        .collect();
    batch(&mut engine, &doc, ops);
    let error = err(&mut engine, "kerning_check", json!({ "doc": doc }));
    assert_eq!(error.code, ErrorCode::LimitExceeded, "{error:?}");
    assert_eq!(error.details["limit"], json!(250_000));
    assert_eq!(error.details["actual"], json!(501 * 501));
}

#[test]
fn kerning_auto_with_a_tall_notdef_has_no_false_collisions() {
    let mut engine = engine_with(Policy::Auto);
    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "T" }),
    );
    let doc = created["doc"].as_str().expect("doc").to_owned();
    // `.notdef` far taller than every other glyph: it sets the sampling grid.
    let mut ops: Vec<Value> = fixture_glyphs()
        .into_iter()
        .map(|(name, advance, unicodes, svg)| {
            if name == ".notdef" {
                create_op(
                    &name,
                    500.0,
                    &[],
                    Some(&rect_svg(50.0, -300.0, 450.0, 1000.0)),
                )
            } else {
                create_op(&name, advance, &unicodes, svg.as_deref())
            }
        })
        .collect();
    ops.push(create_op("V", 680.0, &[0x56], Some(V)));
    batch(&mut engine, &doc, ops);

    let rev = rev_of(&mut engine, &doc);
    approved(
        &mut engine,
        "kerning_auto",
        json!({ "doc": doc, "base_rev": rev, "strength": 3, "max_ratio": 0.5 }),
    );
    for min_distance in [json!(null), json!(20)] {
        let mut args = json!({ "doc": doc });
        if !min_distance.is_null() {
            args["min_distance"] = min_distance;
        }
        let check = ok(&mut engine, "kerning_check", args);
        assert_eq!(check["collisions"], json!([]), "{check}");
    }
    // The same on an explicit list that includes .notdef.
    let listed = json!({ "doc": doc, "min_distance": 20,
        "glyphs": [".notdef", "A", "V", "H", "O"] });
    let rev = rev_of(&mut engine, &doc);
    approved(
        &mut engine,
        "kerning_auto",
        json!({ "doc": doc, "base_rev": rev, "strength": 3, "max_ratio": 0.5,
            "glyphs": [".notdef", "A", "V", "H", "O"] }),
    );
    let check = ok(&mut engine, "kerning_check", listed);
    assert_eq!(check["collisions"], json!([]), "{check}");
    assert_eq!(check["checked_pairs"], json!(16));
}

#[test]
fn kerning_auto_min_distance_can_be_negative() {
    let mut engine = engine_with(Policy::Auto);
    let doc = fixture_doc(&mut engine);
    let glyphs = json!(["A", "V", "H", "O"]);
    let av = |kerning: &Value| pair_value(kerning, "A", "V").expect("A V is kerned");

    // Strong kerning with the default guard: 20 units stay between the inks.
    let rev = rev_of(&mut engine, &doc);
    approved(
        &mut engine,
        "kerning_auto",
        json!({ "doc": doc, "base_rev": rev, "glyphs": glyphs, "strength": 3, "max_ratio": 0.5 }),
    );
    let guarded = av(&ok(&mut engine, "kerning_get", json!({ "doc": doc })));
    let check = ok(
        &mut engine,
        "kerning_check",
        json!({ "doc": doc, "glyphs": glyphs, "min_distance": 20 }),
    );
    assert_eq!(check["collisions"], json!([]), "{check}");

    // A negative distance lets the letters overlap by 30 units, and no more.
    let rev = rev_of(&mut engine, &doc);
    approved(
        &mut engine,
        "kerning_auto",
        json!({ "doc": doc, "base_rev": rev, "glyphs": glyphs, "strength": 3,
            "max_ratio": 0.5, "min_distance": -30 }),
    );
    let touching = av(&ok(&mut engine, "kerning_get", json!({ "doc": doc })));
    assert!(touching < guarded, "{touching} {guarded}");
    let overlaps = ok(
        &mut engine,
        "kerning_check",
        json!({ "doc": doc, "glyphs": glyphs }),
    );
    assert!(
        !overlaps["collisions"].as_array().expect("list").is_empty(),
        "{overlaps}"
    );
    let within = ok(
        &mut engine,
        "kerning_check",
        json!({ "doc": doc, "glyphs": glyphs, "min_distance": -30 }),
    );
    assert_eq!(within["collisions"], json!([]), "{within}");
}

#[test]
fn kerning_auto_replace_existing_is_destructive() {
    let mut engine = engine();
    let doc = fixture_doc(&mut engine);
    // A pair outside the glyphs, and a group of them.
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "kerning_set",
        json!({ "doc": doc, "base_rev": rev,
            "groups": [{ "name": "round", "side": "first", "members": ["C", "O"] }],
            "pairs": [{ "first": { "glyph": "B" }, "second": { "glyph": "b" }, "value": 33 }] }),
    );

    // The plan to replace: destructive (it deletes a pair and a group).
    let rev = rev_of(&mut engine, &doc);
    let args = json!({ "doc": doc, "base_rev": rev, "glyphs": ["A", "V", "H", "O", "C"] });
    let refused = err(&mut engine, "kerning_auto", args.clone());
    assert_eq!(refused.code, ErrorCode::ApprovalRequired, "{refused:?}");
    let plan = refused.details["plan"].to_string();
    assert!(plan.contains("kerning"), "{plan}");
    // Nothing changed yet.
    assert_eq!(rev_of(&mut engine, &doc), rev);
    let before = ok(&mut engine, "kerning_get", json!({ "doc": doc }));
    assert_eq!(pair_value(&before, "B", "b"), Some(33.0));

    let token = refused.details["plan"]["plan_token"].clone();
    ok(&mut engine, "plan_commit", json!({ "plan_token": token }));
    let after = ok(&mut engine, "kerning_get", json!({ "doc": doc }));
    // The old pair is gone (replace deletes every pair of the master), and
    // the old group "round" (a member among the glyphs) with it.
    assert_eq!(pair_value(&after, "B", "b"), None, "{after}");
    assert!(
        after["groups"]
            .as_array()
            .expect("groups")
            .iter()
            .all(|g| g["name"] != json!("round") || g["members"] != json!(["C", "O"])),
        "{after}"
    );
}

#[test]
fn kerning_auto_without_replace_keeps_pairs_and_refuses_group_name_clash() {
    let mut engine = engine_with(Policy::Auto);
    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "T" }),
    );
    let doc = created["doc"].as_str().expect("doc").to_owned();
    let fixture: Vec<Value> = fixture_glyphs()
        .into_iter()
        .filter(|(name, ..)| ["H", "O"].contains(&name.as_str()))
        .map(|(name, advance, unicodes, svg)| create_op(&name, advance, &unicodes, svg.as_deref()))
        .collect();
    let mut ops = fixture;
    // Two identical bars group together and get the group name "bar1".
    for name in ["bar1", "bar2"] {
        ops.push(create_op(
            name,
            500.0,
            &[],
            Some(&rect_svg(80.0, 0.0, 160.0, 700.0)),
        ));
    }
    batch(&mut engine, &doc, ops);
    let rev = rev_of(&mut engine, &doc);
    ok(
        &mut engine,
        "kerning_set",
        json!({ "doc": doc, "base_rev": rev,
            "groups": [{ "name": "bar1", "side": "first", "members": ["H"] }],
            "pairs": [{ "first": { "glyph": "H" }, "second": { "glyph": "O" }, "value": 7 }] }),
    );
    let rev = rev_of(&mut engine, &doc);
    let clash = err(
        &mut engine,
        "kerning_auto",
        json!({ "doc": doc, "base_rev": rev, "replace": false, "glyphs": ["bar1", "bar2", "O"] }),
    );
    assert_eq!(clash.code, ErrorCode::InvalidArgument, "{clash:?}");
    assert!(clash.message.contains("bar1"), "{}", clash.message);
    // The existing pair is untouched by the refused call.
    let got = ok(&mut engine, "kerning_get", json!({ "doc": doc }));
    assert_eq!(pair_value(&got, "H", "O"), Some(7.0));
}

// ------------------------------------------------------------------- charset

#[test]
fn charset_check_unicase_es_reports_missing() {
    let mut engine = engine();
    let doc = fixture_doc(&mut engine);
    let result = ok(
        &mut engine,
        "charset_check",
        json!({ "doc": doc, "profile": "unicase-es" }),
    );
    assert_eq!(result["profile"], json!("unicase-es"));
    assert_eq!(result["required"], json!(119));
    // Space, A B C H O V and a b c n o are encoded.
    assert_eq!(result["covered"], json!(12));
    let missing = result["missing"].as_array().expect("missing");
    assert_eq!(missing.len(), 107);
    assert_eq!(result["missing_total"], json!(107));
    assert!(missing.contains(&json!("U+0044 D")), "{result}");
    assert!(!missing.contains(&json!("U+0041 A")));
    let coverage = result["coverage"].as_f64().expect("coverage");
    assert!((coverage - 12.0 / 119.0).abs() < 1e-9);
    let outlined = result["outlined"].as_array().expect("outlined");
    assert!(outlined.contains(&json!("A")));
    assert_eq!(result["composite"], json!([]));
    assert_eq!(result["rev"], json!(rev_of(&mut engine, &doc)));

    // By code points, as integers and text.
    let some = ok(
        &mut engine,
        "charset_check",
        json!({ "doc": doc, "codepoints": [65, "U+0044", "0x48"] }),
    );
    assert_eq!(some["profile"], Value::Null);
    assert_eq!(some["required"], json!(3));
    assert_eq!(some["missing"], json!(["U+0044 D"]));
}

#[test]
fn charset_requires_exactly_one_selector() {
    let mut engine = engine();
    let doc = fixture_doc(&mut engine);
    for args in [
        json!({ "doc": doc }),
        json!({ "doc": doc, "profile": "unicase-es", "codepoints": [65] }),
        json!({ "doc": doc, "profile": "klingon" }),
        json!({ "doc": doc, "codepoints": [] }),
    ] {
        let error = err(&mut engine, "charset_check", args.clone());
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{args}: {error:?}");
    }
}

// --------------------------------------------------------------- descriptions

#[test]
fn descriptions_carry_the_new_rules() {
    let spec = |name: &str| {
        forma_engine::registry()
            .iter()
            .find(|s| s.name == name)
            .expect(name)
    };
    let auto = spec("kerning_auto").description;
    assert!(auto.contains("negative value"), "{auto}");
    assert!(auto.contains("min_distance"), "{auto}");
    for name in ["edit_batch", "history_revert"] {
        let description = spec(name).description;
        assert!(!description.contains("more than 20 glyphs"), "{name}");
        assert!(description.contains("more than 20 "), "{name}");
        assert!(description.contains("kerning pairs"), "{name}");
        assert!(description.len() <= 1000, "{name}: {}", description.len());
    }
    for name in [
        "spacing_auto",
        "kerning_get",
        "kerning_set",
        "kerning_auto",
        "kerning_check",
        "charset_check",
    ] {
        assert!(
            spec(name).description.len() <= 1000,
            "{name}: {}",
            spec(name).description.len()
        );
    }
}

// ----------------------------------------------------------------- performance

/// 89 glyphs (the fixture's letters, plus variants) through the tool, end to
/// end: the plan, `Steps`, the transaction and the invariants of each command.
#[test]
fn kerning_auto_on_89_glyphs_is_fast_enough() {
    let mut engine = engine_with(Policy::Auto);
    let doc = fixture_doc(&mut engine);
    let mut ops = Vec::new();
    let base = fixture_glyphs();
    let drawn: Vec<_> = base.iter().filter(|g| g.3.is_some()).collect();
    for i in 0..(89 - 13) {
        let (_, advance, _, svg) = drawn[i % drawn.len()];
        ops.push(create_op(
            &format!("v{i}"),
            *advance + (i as f64),
            &[],
            svg.as_deref(),
        ));
    }
    batch(&mut engine, &doc, ops);
    let glyphs = ok(
        &mut engine,
        "metrics_get",
        json!({ "doc": doc, "glyphs": ["H"] }),
    );
    assert!(glyphs["metrics"].is_array());

    let rev = rev_of(&mut engine, &doc);
    let started = Instant::now();
    let done = ok(
        &mut engine,
        "kerning_auto",
        json!({ "doc": doc, "base_rev": rev }),
    );
    let elapsed = started.elapsed();
    let got = ok(&mut engine, "kerning_get", json!({ "doc": doc }));
    eprintln!(
        "kerning_auto on {} glyphs: {elapsed:?}, {} pairs, {} groups, touched {}",
        ok(&mut engine, "glyph_list", json!({ "doc": doc }))["glyphs"]
            .as_array()
            .map_or(0, Vec::len),
        got["pairs_total"],
        got["groups_total"],
        done["touched_count"]
    );
    // The target is under 5 s; debug builds on a loaded machine need room.
    assert!(elapsed.as_secs() < 20, "{elapsed:?}");
}
