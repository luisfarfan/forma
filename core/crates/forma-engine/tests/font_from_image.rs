//! `font_from_image`: the whole pipeline as one transaction.

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

#[test]
fn s_end_to_end_auto_policy() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    let out = ok(&mut engine, "font_from_image", args(&doc, base, &image));
    assert_eq!(out["rev"], base + 1, "one transaction: {out}");
    let stages: Vec<&str> = out["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert_eq!(
        stages,
        [
            "trace",
            "compose",
            "space",
            "kern",
            "check_collisions",
            "charset",
            "validate"
        ]
    );
    assert_eq!(out["stages"][0]["counts"]["glyphs"], 8);
    assert_eq!(out["stages"][6]["counts"]["errors"], 0, "{out}");
    assert!(out["report_path"].is_string());
    assert_eq!(glyph_count(&mut engine, &doc), 8);
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
fn single_undo_reverts_pipeline() {
    let (mut engine, _dir, image) = setup(Policy::Auto);
    let doc = new_doc(&mut engine);
    let base = rev(&mut engine, &doc);
    ok(&mut engine, "font_from_image", args(&doc, base, &image));
    ok(&mut engine, "history_undo", json!({ "doc": doc }));
    assert_eq!(glyph_count(&mut engine, &doc), 0);
}

#[test]
fn prompted_policy_returns_one_plan_with_the_summary_and_commits_once() {
    let (mut engine, _dir, image) = setup(Policy::Prompted);
    let doc = new_doc(&mut engine);
    // Eight new glyphs are not a mass change: no approval.
    let first = rev(&mut engine, &doc);
    ok(&mut engine, "font_from_image", args(&doc, first, &image));
    // Tracing again replaces them: destructive, one plan for everything.
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
    let committed = ok(&mut engine, "plan_commit", json!({ "plan_token": token }));
    assert_eq!(committed["rev"], base + 1, "{committed}");
    assert_eq!(glyph_count(&mut engine, &doc), 8);
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
