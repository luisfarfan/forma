//! `font_from_image`: the whole pipeline as one transaction.

use std::path::{Path, PathBuf};

use forma_engine::{Engine, EngineConfig, ErrorCode, Policy, ToolError, ToolOutput, Toolset};
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

fn new_doc(engine: &mut Engine) -> String {
    ok(
        engine,
        "document_create",
        json!({ "family_name": "Specimen" }),
    )["doc"]
        .as_str()
        .expect("doc")
        .to_owned()
}

fn args(doc: &str, base: u64, image: &Path) -> Value {
    json!({ "doc": doc, "base_rev": base, "path": image, "layout": ["ABCH", "OFMR"] })
}

fn glyph_count(engine: &mut Engine, doc: &str) -> usize {
    ok(engine, "glyph_list", json!({ "doc": doc }))["glyphs"]
        .as_array()
        .map_or(0, Vec::len)
}

fn stage_names(out: &Value) -> Vec<String> {
    out["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .filter_map(|s| s["name"].as_str().map(str::to_owned))
        .collect()
}

fn stage<'v>(out: &'v Value, name: &str) -> &'v Value {
    out["stages"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["name"] == name))
        .unwrap_or_else(|| panic!("no stage {name}: {out}"))
}

fn kinds(out: &Value) -> Vec<String> {
    out["attention"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| i["kind"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn s_end_to_end_auto_policy() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let out = ok(&mut engine, "font_from_image", args(&doc, base, &image));
    assert_eq!(out["rev"], base + 1, "one transaction: {out}");
    assert_eq!(
        stage_names(&out),
        [
            "trace",
            "synthesize",
            "compose",
            "space",
            "kern",
            "check_collisions",
            "charset",
            "validate"
        ]
    );
    assert_eq!(stage(&out, "trace")["counts"]["glyphs"], 8);
    assert_eq!(stage(&out, "validate")["counts"]["errors"], 0, "{out}");
    // The figures, punctuation and marks come from the drawing's style; the
    // marks let the accented letters be composed.
    assert!(
        stage(&out, "synthesize")["counts"]["made"].as_u64() > Some(30),
        "{out}"
    );
    assert_eq!(stage(&out, "compose")["counts"]["composed"], 2, "{out}");
    assert!(kinds(&out).contains(&"needs_design".to_owned()), "{out}");
    assert!(out["report_path"].is_string());
    // The letters encode their lowercase too (unicase-es).
    let a = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "A" }),
    );
    assert_eq!(a["glyph"]["unicodes"], json!([0x41, 0x61]));
    assert!(
        serde_json::to_string(&out).expect("json").len() < 4096,
        "{out}"
    );
}

#[test]
fn auto_policy_never_approves_synthesized() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    ok(&mut engine, "font_from_image", args(&doc, base, &image));
    for name in ["zero", "comma", "acutecomb", "space"] {
        let g = ok(
            &mut engine,
            "glyph_get",
            json!({ "doc": doc, "glyph": name }),
        );
        let p = &g["glyph"]["provenance"];
        assert_eq!(p["source"], "synthesized", "{name}: {g}");
        assert_eq!(p["review"], "pending", "{name}: {g}");
        assert!(p.get("approved_geometry").is_none(), "{name}: {g}");
        assert_eq!(p["detail"]["kind"], "synthesized");
        assert!(p["detail"]["style_score"].as_f64() >= Some(0.75), "{g}");
    }
    let space = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "space" }),
    );
    assert_eq!(space["glyph"]["unicodes"], json!([0x20, 0xA0]));
}

#[test]
fn without_synthesis_only_the_traced_letters_are_made() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let mut a = args(&doc, base, &image);
    a["stages"] = json!({ "synthesize": false });
    let out = ok(&mut engine, "font_from_image", a);
    assert_eq!(stage(&out, "synthesize")["status"], "skipped");
    assert!(kinds(&out).contains(&"missing_marks".to_owned()), "{out}");
    assert_eq!(glyph_count(&mut engine, &doc), 8);
}

#[test]
fn single_undo_reverts_pipeline() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    ok(&mut engine, "font_from_image", args(&doc, base, &image));
    ok(&mut engine, "history_undo", json!({ "doc": doc }));
    assert_eq!(glyph_count(&mut engine, &doc), 0);
}

#[test]
fn dry_run_then_plan_commit_on_a_new_document() {
    let (mut engine, _dir, image) = setup(Policy::Prompted);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    // The canonical flow: look at the plan, then commit it. The glyphs the
    // call creates are named in the plan's commands, so the commit (which
    // runs the call again) describes the same change.
    let mut a = args(&doc, base, &image);
    a["dry_run"] = json!(true);
    let planned = ok(&mut engine, "font_from_image", a);
    let token = planned["plan"]["plan_token"]
        .as_str()
        .unwrap_or_else(|| panic!("a plan to approve: {planned}"));
    assert!(planned["stages"].is_array(), "{planned}");
    assert_eq!(glyph_count(&mut engine, &doc), 0);
    let committed = ok(&mut engine, "plan_commit", json!({ "plan_token": token }));
    assert_eq!(committed["rev"], base + 1, "{committed}");
    let count = glyph_count(&mut engine, &doc);
    assert!(count > 40, "{count}");
    let aacute = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "Aacute" }),
    );
    assert_eq!(
        aacute["glyph"]["components"].as_array().map(Vec::len),
        Some(2),
        "{aacute}"
    );
    // Tracing again replaces the letters: destructive, one plan for
    // everything, committed once.
    let base = rev(&mut engine, &doc);
    let error =
        call(&mut engine, "font_from_image", args(&doc, base, &image)).expect_err("needs approval");
    assert_eq!(error.code, ErrorCode::ApprovalRequired);
    assert!(
        error.details["summary"]["stages"].is_array(),
        "{}",
        error.details
    );
    let token = error.details["plan"]["plan_token"].as_str().expect("token");
    let again = ok(&mut engine, "plan_commit", json!({ "plan_token": token }));
    assert_eq!(again["rev"], base + 1, "{again}");
    assert_eq!(glyph_count(&mut engine, &doc), count);
}

#[test]
fn dry_run_changes_nothing_and_shows_the_stages() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let mut a = args(&doc, base, &image);
    a["dry_run"] = json!(true);
    let out = ok(&mut engine, "font_from_image", a);
    assert!(out["plan"].is_object(), "{out}");
    assert!(out["stages"].is_array(), "{out}");
    assert_eq!(rev(&mut engine, &doc), base);
    assert_eq!(glyph_count(&mut engine, &doc), 0);
}

#[test]
fn concurrent_edit_conflicts() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": base, "name": "A", "advance_width": 500 }),
    );
    // Based on the revision before the other change, which touched A.
    let error =
        call(&mut engine, "font_from_image", args(&doc, base, &image)).expect_err("conflict");
    assert_eq!(error.code, ErrorCode::RevisionConflict, "{error:?}");
}

#[test]
fn a_layout_with_the_wrong_number_of_rows_is_refused_with_the_counts() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let mut a = args(&doc, base, &image);
    a["layout"] = json!(["ABCHOFMR"]);
    let error = call(&mut engine, "font_from_image", a).expect_err("rows");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.details["image_rows"], 2);
}

#[test]
fn a_row_that_does_not_match_is_reported_not_guessed() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let mut a = args(&doc, base, &image);
    a["layout"] = json!(["ABCHX", "OFMR"]);
    let out = ok(&mut engine, "font_from_image", a);
    assert_eq!(out["stages"][0]["counts"]["glyphs"], 4, "{out}");
    let kinds: Vec<&str> = out["attention"]
        .as_array()
        .expect("attention")
        .iter()
        .filter_map(|a| a["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"row_mismatch"), "{out}");
}

#[test]
fn export_is_skipped_while_glyphs_wait_for_review() {
    let (mut engine, dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let mut a = args(&doc, base, &image);
    let ttf = dir.path().join("out.ttf");
    a["stages"] = json!({ "export": { "ttf": ttf } });
    let out = ok(&mut engine, "font_from_image", a);
    assert!(
        out["export"]["written"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "{out}"
    );
    assert!(
        out["export"]["skipped"]
            .as_str()
            .is_some_and(|s| s.contains("review"))
    );
    assert!(!ttf.exists());
}

#[test]
fn export_obeys_the_disabled_export_toolset() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/hito2/specimen.png");
    let image = dir.path().join("specimen.png");
    std::fs::copy(source, &image).expect("copy the specimen");
    let mut engine = Engine::new(EngineConfig {
        allowed_roots: vec![dir.path().to_path_buf()],
        policy: Policy::Auto,
        toolsets: Some(vec![Toolset::Core, Toolset::Glyphs, Toolset::Trace]),
        ..EngineConfig::default()
    });
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let ttf = dir.path().join("bypass.ttf");
    let mut a = args(&doc, base, &image);
    a["stages"] = json!({ "export": { "ttf": ttf, "allow_pending": true } });
    let out = ok(&mut engine, "font_from_image", a);
    assert!(!ttf.exists(), "{out}");
    assert!(
        out["export"]["skipped"]
            .as_str()
            .is_some_and(|s| s.contains("disabled")),
        "{out}"
    );
}

#[test]
fn glyphs_and_kerning_of_the_user_stay() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let mut base = rev(&mut engine, &doc);
    for name in ["x", "z"] {
        ok(
            &mut engine,
            "glyph_create",
            json!({ "doc": doc, "base_rev": base, "name": name, "advance_width": 600,
                "svg_path": "M100 0 L500 0 L500 500 L100 500 Z" }),
        );
        base = rev(&mut engine, &doc);
    }
    ok(
        &mut engine,
        "kerning_set",
        json!({ "doc": doc, "base_rev": base, "pairs": [
            { "first": { "glyph": "x" }, "second": { "glyph": "z" }, "value": -77 } ] }),
    );
    let base = rev(&mut engine, &doc);
    ok(&mut engine, "font_from_image", args(&doc, base, &image));
    let x = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "x" }),
    );
    assert_eq!(x["glyph"]["advance_width"], 600.0, "{x}");
    assert_eq!(x["glyph"]["lsb"], 100.0, "{x}");
    let kerning = ok(
        &mut engine,
        "kerning_get",
        json!({ "doc": doc, "glyphs": ["x"] }),
    );
    assert!(
        kerning["pairs"]
            .as_array()
            .is_some_and(|p| p.iter().any(|p| p["value"] == -77.0)),
        "{kerning}"
    );
}

#[test]
fn a_lowercase_the_document_has_is_left_to_it() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": base, "name": "a", "unicodes": ["U+0061"],
            "advance_width": 500 }),
    );
    let base = rev(&mut engine, &doc);
    let out = ok(&mut engine, "font_from_image", args(&doc, base, &image));
    assert!(kinds(&out).contains(&"unicase_skipped".to_owned()), "{out}");
    let a = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "A" }),
    );
    assert_eq!(a["glyph"]["unicodes"], json!([0x41]));
    // A layout that draws both cases gives each its own code point.
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let mut both = args(&doc, base, &image);
    both["layout"] = json!(["ABCa", "OFMR"]);
    ok(&mut engine, "font_from_image", both);
    let a = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "A" }),
    );
    assert_eq!(a["glyph"]["unicodes"], json!([0x41]));
    let small = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "a" }),
    );
    assert_eq!(small["glyph"]["unicodes"], json!([0x61]));
}

#[test]
fn the_same_shape_named_twice_is_refused() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let mut a = args(&doc, base, &image);
    a["extra"] = json!([
        { "row": 0, "index": 0, "char": "Q" },
        { "row": 0, "index": 0, "char": "X" }
    ]);
    let error = call(&mut engine, "font_from_image", a).expect_err("twice");
    assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
    let mut a = args(&doc, base, &image);
    a["extra"] = json!([{ "row": 0, "index_from_end": usize::MAX, "char": "Q" }]);
    let error = call(&mut engine, "font_from_image", a).expect_err("no such shape");
    assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
}

#[test]
fn export_runs_when_allowed_and_skips_existing_files() {
    let (mut engine, dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let ttf = dir.path().join("out.ttf");
    let woff2 = dir.path().join("out.woff2");
    std::fs::write(&woff2, b"keep").expect("write");
    let mut a = args(&doc, base, &image);
    a["stages"] = json!({ "export": { "ttf": ttf, "woff2": woff2, "allow_pending": true } });
    let out = ok(&mut engine, "font_from_image", a);
    assert_eq!(
        out["export"]["written"].as_array().map(Vec::len),
        Some(1),
        "{out}"
    );
    assert!(ttf.is_file());
    assert_eq!(std::fs::read(&woff2).expect("read"), b"keep");
    assert!(
        out["export"]["skipped"]
            .as_str()
            .is_some_and(|s| s.contains("exists"))
    );
}

#[test]
fn a_rerun_keeps_the_users_pairs_with_the_glyphs_it_made() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    ok(&mut engine, "font_from_image", args(&doc, base, &image));
    let base = rev(&mut engine, &doc);
    ok(
        &mut engine,
        "kerning_set",
        json!({ "doc": doc, "base_rev": base, "pairs": [
            { "first": { "glyph": "A" }, "second": { "glyph": "quoteright" }, "value": -91 },
            { "first": { "glyph": "F" }, "second": { "glyph": "period" }, "value": -77 } ] }),
    );
    let base = rev(&mut engine, &doc);
    let error = call(&mut engine, "font_from_image", args(&doc, base, &image))
        .expect_err("re-tracing replaces glyphs: a plan");
    let token = error.details["plan"]["plan_token"].as_str().expect("token");
    let again = ok(&mut engine, "plan_commit", json!({ "plan_token": token }));
    let kerning = ok(&mut engine, "kerning_get", json!({ "doc": doc }));
    assert_eq!(again["rev"], base + 1, "{again}");
    // The composites of the first run follow their re-traced bases.
    let check = ok(&mut engine, "kerning_check", json!({ "doc": doc }));
    assert_eq!(check["collisions_total"], 0, "{check}");
    let values: Vec<f64> = kerning["pairs"]
        .as_array()
        .map(|p| p.iter().filter_map(|p| p["value"].as_f64()).collect())
        .unwrap_or_default();
    assert!(
        values.contains(&-91.0) && values.contains(&-77.0),
        "{kerning}"
    );
}

#[test]
fn a_disabled_toolset_disables_its_stage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/hito2/specimen.png");
    let image = dir.path().join("specimen.png");
    std::fs::copy(source, &image).expect("copy the specimen");
    let mut engine = Engine::new(EngineConfig {
        allowed_roots: vec![dir.path().to_path_buf()],
        policy: Policy::Auto,
        toolsets: Some(
            Toolset::ALL
                .into_iter()
                .filter(|t| *t != Toolset::Synthesis)
                .collect(),
        ),
        ..EngineConfig::default()
    });
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let out = ok(&mut engine, "font_from_image", args(&doc, base, &image));
    assert_eq!(stage(&out, "synthesize")["status"], "skipped", "{out}");
    assert!(kinds(&out).contains(&"stage_disabled".to_owned()), "{out}");
    assert_eq!(glyph_count(&mut engine, &doc), 8);
}
