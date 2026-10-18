//! Provenance written by the engine, `glyph_review`, and the report files of
//! `detail`.

use std::path::{Path, PathBuf};

use forma_engine::{Engine, EngineConfig, ErrorCode, Policy, ToolError, ToolOutput};
use forma_model::Actor;
use serde_json::{Value, json};

fn actor() -> Actor {
    Actor::Agent("test".into())
}

fn setup_with(policy: Policy, read_only: bool) -> (Engine, tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/hito2/specimen.png");
    let image = dir.path().join("specimen.png");
    std::fs::copy(source, &image).expect("copy the specimen");
    let engine = Engine::new(EngineConfig {
        allowed_roots: vec![dir.path().to_path_buf()],
        policy,
        read_only,
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

/// A document, saved in the allowed root, with `A` and `B` traced.
fn traced_doc(engine: &mut Engine, dir: &Path, image: &Path) -> String {
    let created = ok(engine, "document_create", json!({ "family_name": "Test" }));
    let doc = created["doc"].as_str().expect("doc").to_owned();
    let path = dir.join("test.formapackage");
    let base = rev(engine, &doc);
    ok(
        engine,
        "document_save",
        json!({ "doc": doc, "base_rev": base, "path": path }),
    );
    let base = rev(engine, &doc);
    ok(
        engine,
        "image_trace",
        json!({ "doc": doc, "base_rev": base, "path": image,
            "assignments": [
                { "candidate": 0, "name": "A", "unicodes": ["U+0041"] },
                { "candidate": 1, "name": "B", "unicodes": ["U+0042"] }
            ] }),
    );
    doc
}

fn provenance(engine: &mut Engine, doc: &str, glyph: &str) -> Value {
    ok(engine, "glyph_get", json!({ "doc": doc, "glyph": glyph }))["glyph"]["provenance"].clone()
}

#[test]
fn trace_sets_traced_provenance() {
    let (mut engine, dir, image) = setup_with(Policy::Prompted, false);
    let doc = traced_doc(&mut engine, dir.path(), &image);
    let p = provenance(&mut engine, &doc, "A");
    assert_eq!(p["source"], "traced", "{p}");
    assert_eq!(p["review"], "pending");
    assert_eq!(p["detail"]["kind"], "traced");
    assert_eq!(p["detail"]["candidates"], json!([0]));
    assert_eq!(p["detail"]["image_sha256"].as_str().map(str::len), Some(64));
    let iou = p["detail"]["iou_mask"].as_f64().expect("iou");
    assert!(iou > 0.9 && iou <= 1.0, "{iou}");
}

#[test]
fn review_approve_then_undo() {
    let (mut engine, dir, image) = setup_with(Policy::Prompted, false);
    let doc = traced_doc(&mut engine, dir.path(), &image);
    let base = rev(&mut engine, &doc);
    let reviewed = ok(
        &mut engine,
        "glyph_review",
        json!({ "doc": doc, "base_rev": base, "glyphs": ["A"], "decision": "approve" }),
    );
    assert_eq!(reviewed["touched_count"], 1);
    let p = provenance(&mut engine, &doc, "A");
    assert_eq!(p["review"], "approved");
    assert_eq!(p["approved_geometry"].as_str().map(str::len), Some(64));

    // Spacing keeps the approval; editing a node does not.
    let base = rev(&mut engine, &doc);
    ok(
        &mut engine,
        "metrics_set",
        json!({ "doc": doc, "base_rev": base, "items": [{ "glyph": "A", "lsb": 31, "rsb": 47 }] }),
    );
    assert_eq!(provenance(&mut engine, &doc, "A")["review"], "approved");
    let base = rev(&mut engine, &doc);
    ok(
        &mut engine,
        "nodes_edit",
        json!({ "doc": doc, "base_rev": base, "glyph": "A", "path_id": 0,
            "action": { "type": "move", "indices": [0], "dx": 0, "dy": 9 } }),
    );
    let p = provenance(&mut engine, &doc, "A");
    assert_eq!(
        (p["review"].clone(), p["edited"].clone()),
        (json!("pending"), json!(true))
    );

    // Undo restores the approval with the design.
    ok(&mut engine, "history_undo", json!({ "doc": doc }));
    assert_eq!(provenance(&mut engine, &doc, "A")["review"], "approved");
}

#[test]
fn reject_excludes_from_charset_and_validate_warns_pending() {
    let (mut engine, dir, image) = setup_with(Policy::Prompted, false);
    let doc = traced_doc(&mut engine, dir.path(), &image);
    let report = ok(&mut engine, "validate", json!({ "doc": doc }));
    let pending = report["report"]["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter(|f| f["check"] == "glyph.pending_review")
        .count();
    assert_eq!(pending, 2, "{report}");
    let base = rev(&mut engine, &doc);
    ok(
        &mut engine,
        "glyph_review",
        json!({ "doc": doc, "base_rev": base, "glyphs": ["B"], "decision": "reject" }),
    );
    let charset = ok(
        &mut engine,
        "charset_check",
        json!({ "doc": doc, "codepoints": ["U+0041", "U+0042"] }),
    );
    assert_eq!(charset["covered"], 1, "{charset}");
}

#[test]
fn review_without_provenance_is_invalid_argument() {
    let (mut engine, _dir, _image) = setup_with(Policy::Prompted, false);
    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Test" }),
    );
    let doc = created["doc"].as_str().expect("doc").to_owned();
    let base = rev(&mut engine, &doc);
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": base, "name": "Z", "advance_width": 500 }),
    );
    let base = rev(&mut engine, &doc);
    let error = call(
        &mut engine,
        "glyph_review",
        json!({ "doc": doc, "base_rev": base, "glyphs": ["Z"], "decision": "approve" }),
    )
    .expect_err("hand-drawn");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

// ------------------------------------------------------------------ reports

fn reports(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let root = dir.join(".forma-reports");
    let Ok(docs) = std::fs::read_dir(&root) else {
        return found;
    };
    for doc in docs.flatten() {
        for file in std::fs::read_dir(doc.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            found.push(file.path());
        }
    }
    found.sort();
    found
}

#[test]
fn summary_is_default_and_report_has_the_detail() {
    let (mut engine, dir, image) = setup_with(Policy::Auto, false);
    let doc = traced_doc(&mut engine, dir.path(), &image);
    let base = rev(&mut engine, &doc);
    let spaced = ok(
        &mut engine,
        "spacing_auto",
        json!({ "doc": doc, "base_rev": base, "references": ["A", "B"] }),
    );
    let path = spaced["report_path"].as_str().expect("report_path");
    assert!(
        Path::new(path).starts_with(dir.path().canonicalize().expect("canon")),
        "{path}"
    );
    let text = std::fs::read_to_string(path).expect("report");
    let report: Value = serde_json::from_str(&text).expect("json");
    assert_eq!(report["header"]["tool"], "spacing_auto");
    // The revision the report describes: the state after the call.
    assert_eq!(report["header"]["rev"], json!(base + 1));
    assert!(
        report["body"]["commands"]
            .as_array()
            .is_some_and(|c| !c.is_empty())
    );
    // Canonical: keys sorted, compact.
    assert!(
        text.starts_with("{\"body\":"),
        "{}",
        &text[..40.min(text.len())]
    );
    assert!(
        serde_json::to_string(&spaced).expect("json").len() < 2048,
        "{spaced}"
    );
}

#[test]
fn full_keeps_longer_lists_and_summary_cuts_touched() {
    let (mut engine, dir, image) = setup_with(Policy::Auto, false);
    let doc = traced_doc(&mut engine, dir.path(), &image);
    let summary = ok(&mut engine, "validate", json!({ "doc": doc }));
    let full = ok(
        &mut engine,
        "validate",
        json!({ "doc": doc, "detail": "full" }),
    );
    let count = |v: &Value| v["report"]["findings"].as_array().map_or(0, Vec::len);
    assert!(count(&full) >= count(&summary));
    assert!(
        summary["report"]["findings"]
            .as_array()
            .expect("findings")
            .iter()
            .all(|f| f["severity"] != "info")
    );
    assert!(full["report_path"].is_string() && summary["report_path"].is_string());
}

#[test]
fn reports_never_overwrite_and_retention_prunes_oldest() {
    let (mut engine, dir, image) = setup_with(Policy::Auto, false);
    let doc = traced_doc(&mut engine, dir.path(), &image);
    for _ in 0..25 {
        ok(&mut engine, "validate", json!({ "doc": doc }));
    }
    let files: Vec<PathBuf> = reports(dir.path())
        .into_iter()
        .filter(|p| p.to_string_lossy().contains("-validate-"))
        .collect();
    assert_eq!(files.len(), 20, "{files:?}");
}

#[test]
fn read_only_server_writes_no_report() {
    let (mut writer, dir, image) = setup_with(Policy::Auto, false);
    let doc = traced_doc(&mut writer, dir.path(), &image);
    let base = rev(&mut writer, &doc);
    ok(
        &mut writer,
        "document_save",
        json!({ "doc": doc, "base_rev": base }),
    );
    ok(&mut writer, "document_close", json!({ "doc": doc }));
    std::fs::remove_dir_all(dir.path().join(".forma-reports")).ok();

    let mut reader = Engine::new(EngineConfig {
        allowed_roots: vec![dir.path().to_path_buf()],
        read_only: true,
        ..EngineConfig::default()
    });
    let opened = ok(
        &mut reader,
        "document_open",
        json!({ "path": dir.path().join("test.formapackage") }),
    );
    let doc = opened["doc"].as_str().expect("doc");
    let checked = ok(&mut reader, "validate", json!({ "doc": doc }));
    assert!(checked["report_path"].is_null(), "{checked}");
    assert!(
        checked["report_note"]
            .as_str()
            .is_some_and(|n| n.contains("read-only"))
    );
    assert!(reports(dir.path()).is_empty());
}

#[test]
fn deleting_reports_loses_nothing() {
    let (mut engine, dir, image) = setup_with(Policy::Auto, false);
    let doc = traced_doc(&mut engine, dir.path(), &image);
    let before = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "A" }),
    );
    std::fs::remove_dir_all(dir.path().join(".forma-reports")).expect("delete reports");
    let after = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "A" }),
    );
    assert_eq!(before["glyph"], after["glyph"]);
}
