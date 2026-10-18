//! Roadmap criterion 6.2: every real output validates against its
//! `outputSchema`.
//!
//! One scenario, through the MCP facade (an in-process rmcp client), calls
//! every tool that declares an output schema, on the paths that succeed:
//! plain calls, dry runs (`{rev, plan}`), plans committed with plan_commit
//! (validated against both plan_commit's schema and the planned tool's) and
//! discarded. Each structured result is validated with an independent JSON
//! Schema validator (draft 2020-12) against the schema `tools/list` declares,
//! and at the end every tool with an output schema must have been validated.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use forma_engine::{EngineConfig, Policy};
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig};
use rmcp::model::{Implementation, ProtocolVersion};
use rmcp::service::RunningService;
use rmcp::{ClientLifecycleMode, ClientServiceExt, RoleClient};
use serde_json::{Value, json};
use tempfile::TempDir;

type Client = RunningService<RoleClient, ClientConfig>;

const SQUARE: &str = "M100 0 L500 0 L500 400 L100 400 Z";

/// The client, the output schema of every tool that has one, and what was
/// validated so far.
struct Checker {
    client: Client,
    schemas: BTreeMap<String, jsonschema::Validator>,
    /// Tool name → how many results were validated against its schema.
    validated: BTreeMap<String, usize>,
}

impl Checker {
    async fn connect(root: &Path) -> Checker {
        let cfg = EngineConfig {
            allowed_roots: vec![root.to_path_buf()],
            journal_dir: Some(root.join("journal")),
            lock_dir: Some(root.join("locks")),
            policy: Policy::Prompted,
            ..EngineConfig::default()
        };
        let (server_io, client_io) = tokio::io::duplex(1024 * 1024);
        tokio::spawn(forma_mcp::serve(cfg, server_io));
        let info = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("schema-checker", "1.0.0"),
        )
        .with_protocol_version(ProtocolVersion::V_2025_06_18);
        let client = info
            .serve_with_lifecycle(client_io, ClientLifecycleMode::Initialize)
            .await
            .expect("connect");
        let mut schemas = BTreeMap::new();
        for tool in client.list_all_tools().await.expect("tools/list") {
            if let Some(schema) = &tool.output_schema {
                let schema = Value::Object((**schema).clone());
                let validator = jsonschema::validator_for(&schema)
                    .unwrap_or_else(|error| panic!("{}: invalid schema: {error}", tool.name));
                schemas.insert(tool.name.to_string(), validator);
            }
        }
        Checker {
            client,
            schemas,
            validated: BTreeMap::new(),
        }
    }

    /// Calls `tool` and returns the raw result.
    async fn raw(&self, tool: &str, args: Value) -> CallToolResult {
        let Value::Object(args) = args else {
            panic!("arguments must be an object");
        };
        self.client
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args))
            .await
            .unwrap_or_else(|error| panic!("tools/call {tool}: {error}"))
    }

    /// Validates `value` against the output schema of `tool`.
    #[track_caller]
    fn check(&mut self, tool: &str, value: &Value) {
        let validator = self
            .schemas
            .get(tool)
            .unwrap_or_else(|| panic!("{tool} has no output schema"));
        let errors: Vec<String> = validator
            .iter_errors(value)
            .map(|error| format!("{} at {}", error, error.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{tool}: {errors:#?}\n{value:#}");
        *self.validated.entry(tool.to_owned()).or_default() += 1;
    }

    /// Calls `tool`, which must succeed with structured content, and
    /// validates the result against its schema.
    async fn ok(&mut self, tool: &str, args: Value) -> Value {
        let result = self.raw(tool, args).await;
        assert_ne!(result.is_error, Some(true), "{tool} failed: {result:?}");
        let structured = result
            .structured_content
            .clone()
            .unwrap_or_else(|| panic!("{tool}: no structured content: {result:?}"));
        self.check(tool, &structured);
        structured
    }

    /// Calls `tool`, which must fail with `code`; returns the error.
    async fn fails(&self, tool: &str, args: Value, code: &str) -> Value {
        let result = self.raw(tool, args).await;
        assert_eq!(result.is_error, Some(true), "{tool}: {result:?}");
        let text = result.content[0].as_text().expect("text").text.clone();
        let error: Value = serde_json::from_str(&text).expect("JSON error");
        assert_eq!(error["code"], json!(code), "{tool}: {error}");
        error
    }

    /// Commits `plan` of `tool`: the result is validated against both
    /// plan_commit's schema and the planned tool's.
    async fn commit(&mut self, tool: &str, plan: &Value) -> Value {
        let committed = self
            .ok("plan_commit", json!({ "plan_token": plan["plan_token"] }))
            .await;
        assert_eq!(committed["plan_committed"], json!(true));
        self.check(tool, &committed);
        committed
    }

    /// Calls `tool` with `dry_run: true` and commits the plan.
    async fn planned(&mut self, tool: &str, mut args: Value) -> Value {
        args["dry_run"] = json!(true);
        let planned = self.ok(tool, args).await;
        self.commit(tool, &planned["plan"]).await
    }
}

fn rev(value: &Value) -> u64 {
    value["rev"].as_u64().expect("rev")
}

#[tokio::test]
async fn every_output_validates_against_its_output_schema() {
    let root = TempDir::new().expect("root");
    let mut c = Checker::connect(root.path()).await;
    let path = root.path().join("schema.forma");
    // The validator is not vacuous: results missing a required field, or
    // with a field of the wrong type, are refused.
    assert!(!c.schemas["document_list"].is_valid(&json!({})));
    assert!(
        !c.schemas["geometry_check"]
            .is_valid(&json!({ "rev": 0, "issues": [], "total": "0", "truncated": false }))
    );

    // Core and document lifecycle.
    c.ok("forma_capabilities", json!({})).await;
    let created = c
        .ok(
            "document_create",
            json!({ "family_name": "Schemas", "path": path }),
        )
        .await;
    let doc = created["doc"].as_str().expect("doc").to_owned();
    c.ok("document_create", json!({ "family_name": "Memory" }))
        .await;
    c.ok("document_list", json!({})).await;
    c.ok("document_inspect", json!({ "doc": doc })).await;
    let again = c.ok("document_open", json!({ "path": path })).await;
    assert_eq!(again["already_open"], json!(true));

    // Font-wide edits, plain and as dry runs.
    let info = json!({ "doc": doc, "base_rev": 0, "designer": "Schema" });
    c.planned("font_set_info", info.clone()).await;
    let r = rev(&c
        .ok(
            "font_set_vertical_metrics",
            json!({ "doc": doc, "base_rev": 1, "ascender": 810 }),
        )
        .await);
    let mut info = info;
    info["base_rev"] = json!(r);
    info["designer"] = json!("Schema Two");
    let r = rev(&c.ok("font_set_info", info).await);

    // Glyphs.
    let created = c
        .ok(
            "glyph_create",
            json!({ "doc": doc, "base_rev": r, "name": "A", "unicodes": ["U+0041"],
                    "advance_width": 500, "svg_path": SQUARE }),
        )
        .await;
    let r = rev(&created);
    let planned = c
        .planned(
            "glyph_create",
            json!({ "doc": doc, "base_rev": r, "name": "B", "advance_width": 500 }),
        )
        .await;
    let r = rev(&planned);
    let ops: Vec<Value> = (0..25)
        .map(|i| {
            json!({ "tool": "glyph_create",
                    "args": { "name": format!("g{i}"), "advance_width": 100, "svg_path": SQUARE } })
        })
        .collect();
    let error = c
        .fails(
            "edit_batch",
            json!({ "doc": doc, "base_rev": r, "ops": ops.clone() }),
            "APPROVAL_REQUIRED",
        )
        .await;
    let r = rev(&c.commit("edit_batch", &error["details"]["plan"]).await);
    let r = rev(&c
        .ok(
            "edit_batch",
            json!({ "doc": doc, "base_rev": r, "label": "two", "ops": [
                { "tool": "glyph_update", "args": { "glyph": "g0", "name": "g0x" } },
                { "tool": "metrics_set", "args": { "items": [{ "glyph": "B", "advance_width": 450 }] } }
            ] }),
        )
        .await);
    c.ok("glyph_list", json!({ "doc": doc, "limit": 5 })).await;
    c.ok("glyph_get", json!({ "doc": doc, "glyph": "A" })).await;
    let r = rev(&c
        .ok(
            "glyph_update",
            json!({ "doc": doc, "base_rev": r, "glyph": "g1", "name": "g1x", "export": false }),
        )
        .await);
    let r = rev(&c
        .planned(
            "glyph_update",
            json!({ "doc": doc, "base_rev": r, "glyph": "g1x", "export": true }),
        )
        .await);

    // Outlines: a drawn layer replaced is destructive (plan); an empty one
    // is drawn directly.
    let r = rev(&c
        .planned(
            "glyph_set_outline",
            json!({ "doc": doc, "base_rev": r, "glyph": "A", "svg_path": SQUARE }),
        )
        .await);
    let r = rev(&c
        .ok(
            "glyph_set_outline",
            json!({ "doc": doc, "base_rev": r, "glyph": "B", "svg_path": SQUARE }),
        )
        .await);
    let r = rev(&c
        .ok(
            "contour_edit",
            json!({ "doc": doc, "base_rev": r, "glyph": "A",
                    "action": { "type": "reverse", "path_id": 0 } }),
        )
        .await);
    let r = rev(&c
        .planned(
            "contour_edit",
            json!({ "doc": doc, "base_rev": r, "glyph": "A",
                    "action": { "type": "reverse", "path_id": 0 } }),
        )
        .await);
    let r = rev(&c
        .ok(
            "nodes_edit",
            json!({ "doc": doc, "base_rev": r, "glyph": "A", "path_id": 0,
                    "action": { "type": "move", "indices": [0], "dx": 5, "dy": 0 } }),
        )
        .await);
    let r = rev(&c
        .planned(
            "nodes_edit",
            json!({ "doc": doc, "base_rev": r, "glyph": "A", "path_id": 0,
                    "action": { "type": "move", "indices": [0], "dx": -5, "dy": 0 } }),
        )
        .await);

    // Spacing and geometry.
    c.ok("metrics_get", json!({ "doc": doc })).await;
    c.ok("metrics_get", json!({ "doc": doc, "glyphs": ["A"] }))
        .await;
    let r = rev(&c
        .ok(
            "metrics_set",
            json!({ "doc": doc, "base_rev": r, "items": [{ "glyph": "A", "lsb": 20 }] }),
        )
        .await);
    let r = rev(&c
        .planned(
            "metrics_set",
            json!({ "doc": doc, "base_rev": r, "items": [{ "glyph": "A", "rsb": 30 }] }),
        )
        .await);
    // g2 is drawn: redrawing it off the grid needs a plan.
    let r = rev(&c
        .planned(
            "glyph_set_outline",
            json!({ "doc": doc, "base_rev": r, "glyph": "g2",
                    "svg_path": "M0.5 0 L100 0 L100 100 L0 100 Z" }),
        )
        .await);
    c.ok("geometry_check", json!({ "doc": doc })).await;
    c.ok(
        "geometry_check",
        json!({ "doc": doc, "glyphs": ["g2"], "grid": 0 }),
    )
    .await;
    let r = rev(&c
        .planned("geometry_fix", json!({ "doc": doc, "base_rev": r }))
        .await);
    let r = rev(&c
        .ok(
            "geometry_fix",
            json!({ "doc": doc, "base_rev": r, "glyphs": ["g2"], "kinds": ["off_grid"] }),
        )
        .await);

    // Components, strokes and overlaps.
    let composed = c
        .ok(
            "glyph_compose",
            json!({ "doc": doc, "base_rev": r, "name": "AB", "base": "A",
                    "marks": [{ "glyph": "B", "position": "above" }] }),
        )
        .await;
    assert_eq!(composed["glyph"]["name"], json!("AB"));
    let r = rev(&composed);
    let r = rev(&c
        .planned(
            "component_set",
            json!({ "doc": doc, "base_rev": r, "glyph": "AB",
                    "components": [{ "base": "A" }, { "base": "B" }] }),
        )
        .await);
    let r = rev(&c
        .ok(
            "glyph_decompose",
            json!({ "doc": doc, "base_rev": r, "glyphs": ["AB"] }),
        )
        .await);
    let r = rev(&c
        .ok(
            "remove_overlap",
            json!({ "doc": doc, "base_rev": r, "glyphs": ["AB"] }),
        )
        .await);
    let r = rev(&c
        .ok(
            "glyph_create",
            json!({ "doc": doc, "base_rev": r, "name": "S", "advance_width": 600 }),
        )
        .await);
    let stroke = json!({ "glyph": "S", "width": 60, "cap": { "type": "round" },
        "skeletons": [{ "nodes": [
            { "x": 100, "y": 100, "kind": "line" },
            { "x": 400, "y": 100, "kind": "line" }] }] });
    let mut args = stroke.clone();
    args["doc"] = json!(doc);
    args["base_rev"] = json!(r);
    let r = rev(&c.ok("glyph_stroke", args).await);
    let mut args = stroke;
    args["doc"] = json!(doc);
    args["base_rev"] = json!(r);
    let r = rev(&c.planned("glyph_stroke", args).await);

    // Deletion: refused without a plan, discarded, then committed.
    let error = c
        .fails(
            "glyph_delete",
            json!({ "doc": doc, "base_rev": r, "glyphs": ["g3"] }),
            "APPROVAL_REQUIRED",
        )
        .await;
    c.ok(
        "plan_discard",
        json!({ "plan_token": error["details"]["plan"]["plan_token"] }),
    )
    .await;
    let r = rev(&c
        .planned(
            "glyph_delete",
            json!({ "doc": doc, "base_rev": r, "glyphs": ["g3"] }),
        )
        .await);

    // Quality and export.
    c.ok("validate", json!({ "doc": doc })).await;
    for format in ["ttf", "woff2", "ufo"] {
        c.ok(
            "export_font",
            json!({ "doc": doc, "base_rev": r, "format": format,
                    "path": root.path().join(format!("schema.{format}")) }),
        )
        .await;
    }
    c.planned(
        "export_font",
        json!({ "doc": doc, "base_rev": r, "format": "ttf",
                "path": root.path().join("schema.ttf") }),
    )
    .await;
    // The preview tools answer with images (no output schema); the SVG form
    // of preview_glyph is JSON without a schema.
    let svg = c
        .raw(
            "preview_glyph",
            json!({ "doc": doc, "glyph": "A", "format": "svg" }),
        )
        .await;
    assert_ne!(svg.is_error, Some(true), "{svg:?}");

    // History.
    c.ok("history_list", json!({ "doc": doc, "limit": 3 }))
        .await;
    c.ok("history_undo", json!({ "doc": doc })).await;
    let r = rev(&c.ok("history_redo", json!({ "doc": doc })).await);
    let entries = c
        .ok("history_list", json!({ "doc": doc, "limit": 100 }))
        .await["entries"]
        .clone();
    // Reverting the vertical metrics change destroys nothing: plain. Then
    // reverting that revert, as a dry run committed.
    let is_metrics = |key: &Value| {
        key.as_str()
            .is_some_and(|key| key.starts_with("vertical_metrics:"))
    };
    let metrics_tx = entries
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| {
            entry["touched"]
                .as_array()
                .is_some_and(|keys| keys.iter().any(is_metrics))
        })
        .expect("the vertical metrics transaction")["tx"]
        .clone();
    let reverted = c
        .ok(
            "history_revert",
            json!({ "doc": doc, "base_rev": r, "tx": metrics_tx }),
        )
        .await;
    let r = rev(&c
        .planned(
            "history_revert",
            json!({ "doc": doc, "base_rev": rev(&reverted), "tx": reverted["tx"] }),
        )
        .await);
    let r = rev(&c
        .planned(
            "history_restore",
            json!({ "doc": doc, "base_rev": r, "rev": 2 }),
        )
        .await);
    c.ok("document_diff", json!({ "doc": doc, "from_rev": 0 }))
        .await;
    c.ok(
        "document_diff",
        json!({ "doc": doc, "from_rev": 1, "to_rev": 3 }),
    )
    .await;

    // Saving and closing.
    c.planned("document_save", json!({ "doc": doc, "base_rev": r }))
        .await;
    c.ok("document_save", json!({ "doc": doc, "base_rev": r }))
        .await;
    c.ok(
        "document_save",
        json!({ "doc": doc, "base_rev": r, "path": root.path().join("copy.forma") }),
    )
    .await;
    c.ok(
        "glyph_create",
        json!({ "doc": doc, "base_rev": r, "name": "dirty", "advance_width": 1 }),
    )
    .await;
    c.planned(
        "document_close",
        json!({ "doc": doc, "discard_changes": true }),
    )
    .await;
    let reopened = c.ok("document_open", json!({ "path": path })).await;
    c.ok("document_close", json!({ "doc": reopened["doc"] }))
        .await;

    // Every tool with an output schema was validated.
    let declared: BTreeSet<&String> = c.schemas.keys().collect();
    let seen: BTreeSet<&String> = c.validated.keys().collect();
    assert_eq!(
        declared.difference(&seen).collect::<Vec<_>>(),
        Vec::<&&String>::new(),
        "tools never validated"
    );
    assert_eq!(declared.len(), 37, "{declared:?}");
    let total: usize = c.validated.values().sum();
    assert!(total > 80, "{total} results validated");
    c.client.cancel().await.expect("close");
}
