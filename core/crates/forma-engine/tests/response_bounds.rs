//! Responses stay within the budget of docs/07 §10 (≤ 10 000 tokens): the
//! lists of `geometry_check`, `validate` and `metrics_get` are cut at 200
//! entries, and before their JSON passes 32 KiB, with exact totals and a
//! note on how to see the rest.

use forma_engine::{Engine, EngineConfig, Policy, ToolOutput};
use forma_model::Actor;
use serde_json::{Value, json};
use tempfile::TempDir;

/// The most entries a list of these results holds.
const CAP: usize = 200;

/// About 10 000 tokens.
const BUDGET_BYTES: usize = 40_000;

/// Two off-grid squares running the same way: every glyph has off-grid
/// nodes and an inner contour in the wrong direction.
const SQUARES: &str = "M 0.3 0.3 L 100.3 0.3 L 100.3 100.3 L 0.3 100.3 Z \
                       M 20.5 20.5 L 80.5 20.5 L 80.5 80.5 L 20.5 80.5 Z";

fn actor() -> Actor {
    Actor::Agent("test".into())
}

#[track_caller]
fn ok(engine: &mut Engine, tool: &str, args: Value) -> Value {
    match engine.call(&actor(), tool, args) {
        Ok(ToolOutput::Json(value)) => value,
        other => panic!("{tool}: {other:?}"),
    }
}

/// The size of the JSON a client receives.
fn bytes(value: &Value) -> usize {
    serde_json::to_string(value).expect("json").len()
}

/// The name of glyph `index`: short (`g7`), or 60 characters long.
fn name(index: usize, long: bool) -> String {
    if long {
        format!("a_glyph_with_a_long_name_that_makes_every_entry_bigger_{index:05}")
    } else {
        format!("g{index}")
    }
}

/// A document with `count` glyphs ([`name`]) drawn with [`SQUARES`], saved
/// to a file in `dir`; returns the engine, the handle and the file.
fn big_font(dir: &TempDir, count: usize, long: bool) -> (Engine, String, std::path::PathBuf) {
    let mut engine = Engine::new(EngineConfig {
        allowed_roots: vec![dir.path().to_path_buf()],
        policy: Policy::Auto,
        ..EngineConfig::default()
    });
    let path = dir.path().join("big.forma");
    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "Big", "path": path }),
    );
    let doc = created["doc"].as_str().expect("doc").to_owned();
    let ops: Vec<Value> = (0..count)
        .map(|index| {
            json!({ "tool": "glyph_create", "args": {
                "name": name(index, long), "advance_width": 120, "svg_path": SQUARES } })
        })
        .collect();
    ok(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": 0, "ops": ops }),
    );
    ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 1 }),
    );
    (engine, doc, path)
}

#[test]
fn long_lists_are_cut_with_exact_totals() {
    for long in [false, true] {
        let dir = TempDir::new().expect("tempdir");
        let (mut engine, doc, path) = big_font(&dir, 300, long);
        let first = name(0, long);

        // geometry_check: every glyph has the same issues.
        let one = ok(
            &mut engine,
            "geometry_check",
            json!({ "doc": doc, "glyphs": [first] }),
        );
        let per_glyph = one["issues"].as_array().expect("issues").len();
        assert!(per_glyph > 1, "{one}");
        assert_eq!(one["total"], json!(per_glyph));
        assert_eq!(one["truncated"], json!(false));
        assert!(one.get("note").is_none(), "{one}");
        // The summary lists a few; `full` keeps the long, bounded list.
        let summary = ok(&mut engine, "geometry_check", json!({ "doc": doc }));
        let listed = summary["issues"].as_array().map_or(0, Vec::len);
        assert!((1..=20).contains(&listed), "{listed}");
        assert_eq!(summary["truncated"], json!(true));
        assert!(summary["report_path"].is_string(), "{summary}");
        assert_eq!(summary["total"], json!(300 * per_glyph));
        let checked = ok(
            &mut engine,
            "geometry_check",
            json!({ "doc": doc, "detail": "full" }),
        );
        let issues = checked["issues"].as_array().expect("issues");
        assert!(
            issues.len() <= CAP && issues.len() > 100,
            "{}",
            issues.len()
        );
        assert_eq!(issues[0], one["issues"][0]);
        assert_eq!(checked["total"], json!(300 * per_glyph));
        assert_eq!(checked["truncated"], json!(true));
        let note = checked["note"].as_str().expect("note");
        assert!(note.contains("`glyphs`"), "{note}");
        assert!(
            note.starts_with(&format!(
                "showing the first {} of {}",
                issues.len(),
                300 * per_glyph
            )),
            "{note}"
        );
        assert!(bytes(&checked) < BUDGET_BYTES, "{} bytes", bytes(&checked));

        // metrics_get: one entry per glyph, small enough for all 200.
        let metrics = ok(&mut engine, "metrics_get", json!({ "doc": doc }));
        assert_eq!(metrics["metrics"].as_array().expect("metrics").len(), CAP);
        assert_eq!(metrics["metrics"][0]["glyph"], json!(first));
        assert_eq!(metrics["total"], json!(300));
        assert_eq!(metrics["truncated"], json!(true));
        let note = metrics["note"].as_str().expect("note");
        assert!(note.contains("`glyphs`"), "{note}");
        assert!(bytes(&metrics) < BUDGET_BYTES, "{} bytes", bytes(&metrics));

        // validate: the first findings (errors first) of the full report,
        // whose counts cover every finding.
        let font = forma_store::load(&path).expect("load");
        let full = forma_validate::validate(&font);
        assert!(full.findings.len() > CAP, "{}", full.findings.len());
        let validated = ok(
            &mut engine,
            "validate",
            json!({ "doc": doc, "detail": "full" }),
        );
        let report = &validated["report"];
        let listed = report["findings"].as_array().expect("findings").len();
        assert!(listed <= CAP && listed > 100, "{listed}");
        assert_eq!(
            report["findings"],
            serde_json::to_value(&full.findings[..listed]).expect("findings")
        );
        assert_eq!(report["errors"], json!(full.errors));
        assert_eq!(report["warnings"], json!(full.warnings));
        assert_eq!(validated["total"], json!(full.findings.len()));
        assert_eq!(validated["truncated"], json!(true));
        assert!(validated["note"].is_string(), "{validated}");
        assert!(
            bytes(&validated) < BUDGET_BYTES,
            "{} bytes",
            bytes(&validated)
        );
    }
}

#[test]
fn short_lists_are_whole() {
    let dir = TempDir::new().expect("tempdir");
    let (mut engine, doc, _) = big_font(&dir, 3, true);
    for (tool, list) in [
        ("geometry_check", "/issues"),
        ("metrics_get", "/metrics"),
        ("validate", "/report/findings"),
    ] {
        let result = ok(&mut engine, tool, json!({ "doc": doc }));
        let entries = result.pointer(list).and_then(Value::as_array).expect(list);
        assert_eq!(result["total"], json!(entries.len()), "{tool}");
        assert_eq!(result["truncated"], json!(false), "{tool}");
        assert!(result.get("note").is_none(), "{tool}: {result}");
    }
}
