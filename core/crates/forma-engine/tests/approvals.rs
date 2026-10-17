//! Approval plans: the gate on destructive and massive calls, signed
//! single-use tokens, `plan_commit`, `plan_discard` and `dry_run`.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use forma_engine::{
    Clock, Engine, EngineConfig, ErrorCode, Policy, ToolError, ToolOutput, registry,
};
use forma_model::{Actor, Font};
use serde_json::{Value, json};
use tempfile::TempDir;

// ------------------------------------------------------------------ helpers

/// A clock the test moves by hand.
#[derive(Clone)]
struct TestClock(Arc<Mutex<SystemTime>>);

impl TestClock {
    /// 2027-01-15T08:00:00Z.
    fn new() -> TestClock {
        TestClock(Arc::new(Mutex::new(
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        )))
    }

    fn advance(&self, seconds: u64) {
        *self.0.lock().expect("clock") += Duration::from_secs(seconds);
    }
}

impl Clock for TestClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().expect("clock")
    }
}

fn agent() -> Actor {
    Actor::Agent("test".into())
}

fn engine() -> Engine {
    engine_with_policy(Policy::Prompted)
}

fn engine_with_policy(policy: Policy) -> Engine {
    Engine::new(EngineConfig {
        policy,
        ..EngineConfig::default()
    })
}

fn engine_with_clock(clock: &TestClock) -> Engine {
    Engine::with_clock(EngineConfig::default(), Box::new(clock.clone()))
}

fn call_as(
    engine: &mut Engine,
    actor: &Actor,
    tool: &str,
    args: Value,
) -> Result<Value, ToolError> {
    match engine.call(actor, tool, args)? {
        ToolOutput::Json(value) => Ok(value),
        ToolOutput::Image { .. } => panic!("{tool} returned an image"),
    }
}

#[track_caller]
fn ok_as(engine: &mut Engine, actor: &Actor, tool: &str, args: Value) -> Value {
    match call_as(engine, actor, tool, args) {
        Ok(value) => value,
        Err(error) => panic!("{tool} failed: {error:?}"),
    }
}

#[track_caller]
fn err_as(engine: &mut Engine, actor: &Actor, tool: &str, args: Value) -> ToolError {
    match call_as(engine, actor, tool, args) {
        Ok(value) => panic!("{tool} succeeded unexpectedly: {value}"),
        Err(error) => error,
    }
}

#[track_caller]
fn ok(engine: &mut Engine, tool: &str, args: Value) -> Value {
    ok_as(engine, &agent(), tool, args)
}

#[track_caller]
fn err(engine: &mut Engine, tool: &str, args: Value) -> ToolError {
    err_as(engine, &agent(), tool, args)
}

fn new_doc(engine: &mut Engine) -> String {
    let created = ok(engine, "document_create", json!({ "family_name": "Test" }));
    created["doc"].as_str().expect("doc").to_owned()
}

fn inspect(engine: &mut Engine, doc: &str) -> Value {
    ok(engine, "document_inspect", json!({ "doc": doc }))
}

fn rev_of(engine: &mut Engine, doc: &str) -> u64 {
    inspect(engine, doc)["rev"].as_u64().expect("rev")
}

fn glyph_count(engine: &mut Engine, doc: &str) -> u64 {
    inspect(engine, doc)["glyph_count"]
        .as_u64()
        .expect("glyph_count")
}

fn history_len(engine: &mut Engine, doc: &str) -> usize {
    ok(engine, "history_list", json!({ "doc": doc, "limit": 200 }))["entries"]
        .as_array()
        .expect("entries")
        .len()
}

fn glyph_names(engine: &mut Engine, doc: &str) -> Vec<String> {
    ok(engine, "glyph_list", json!({ "doc": doc, "limit": 500 }))["glyphs"]
        .as_array()
        .expect("glyphs")
        .iter()
        .map(|glyph| glyph["name"].as_str().expect("name").to_owned())
        .collect()
}

const SQUARE: &str = "M0 0 L100 0 L100 100 L0 100 Z";

fn name(index: usize) -> String {
    format!("g{index:02}")
}

fn names(count: usize) -> Vec<String> {
    (0..count).map(name).collect()
}

/// A document with the glyphs `g00`, `g01`, … each drawn with a square,
/// created one call at a time (none needs approval under any policy).
/// Returns the handle and the current revision.
fn doc_with_glyphs(engine: &mut Engine, count: usize) -> (String, u64) {
    let doc = new_doc(engine);
    for index in 0..count {
        ok(
            engine,
            "glyph_create",
            json!({ "doc": doc, "base_rev": index, "name": name(index),
                    "advance_width": 500, "svg_path": SQUARE }),
        );
    }
    (doc, count as u64)
}

/// The plan of an `APPROVAL_REQUIRED` error.
#[track_caller]
fn plan_of(error: &ToolError) -> Value {
    assert_eq!(error.code, ErrorCode::ApprovalRequired, "{error:?}");
    error.details["plan"].clone()
}

#[track_caller]
fn token_of(plan: &Value) -> String {
    plan["plan_token"].as_str().expect("plan_token").to_owned()
}

fn commit(engine: &mut Engine, token: &str) -> Result<Value, ToolError> {
    call_as(
        engine,
        &agent(),
        "plan_commit",
        json!({ "plan_token": token }),
    )
}

fn advance_all(doc: &str, base_rev: u64, count: usize, width: u64) -> Value {
    let items: Vec<Value> = names(count)
        .into_iter()
        .map(|glyph| json!({ "glyph": glyph, "advance_width": width }))
        .collect();
    json!({ "doc": doc, "base_rev": base_rev, "items": items })
}

fn empty_diff() -> Value {
    json!({ "added": [], "added_count": 0, "removed": [], "removed_count": 0,
            "modified": [], "modified_count": 0, "info_changed": false,
            "metrics_changed": false, "kerning_changed": false, "truncated": false })
}

/// Commits the plan of a call that needs approval, as an agent does once the
/// user confirms.
#[track_caller]
fn confirmed(engine: &mut Engine, tool: &str, args: Value) -> Value {
    let error = err(engine, tool, args);
    let token = token_of(&plan_of(&error));
    commit(engine, &token).expect("commit")
}

/// Every tool that changes a document or a file, and so takes `dry_run`.
const PLANNABLE: [&str; 16] = [
    "document_save",
    "document_close",
    "font_set_info",
    "font_set_vertical_metrics",
    "edit_batch",
    "glyph_create",
    "glyph_update",
    "glyph_delete",
    "glyph_set_outline",
    "contour_edit",
    "nodes_edit",
    "geometry_fix",
    "metrics_set",
    "history_revert",
    "history_restore",
    "export_font",
];

// ------------------------------------------------------------ from the brief

#[test]
fn delete_25_glyphs_requires_plan() {
    let clock = TestClock::new();
    let mut engine = engine_with_clock(&clock);
    let (doc, rev) = doc_with_glyphs(&mut engine, 25);
    let all = names(25);

    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev, "glyphs": all }),
    );
    let plan = plan_of(&error);
    let summary = plan["summary"].as_str().expect("summary");
    assert_eq!(
        summary,
        "Delete 25 glyphs (25 glyphs affected, destructive)"
    );
    assert_eq!(
        error.message,
        format!(
            "This operation needs approval: {summary}. Show the plan to the user and call \
             plan_commit with the plan_token only after they confirm."
        )
    );
    assert_eq!(plan["tool"], json!("glyph_delete"));
    assert_eq!(plan["base_rev"], json!(rev));
    assert_eq!(plan["affected_count"], json!(25));
    assert_eq!(plan["affected"], json!(all));
    assert_eq!(plan["diff"]["removed"], json!(all));
    assert_eq!(plan["diff"]["added"], json!([]));
    assert_eq!(plan["diff"]["modified"], json!([]));
    assert_eq!(plan["expires_at"], json!("2027-01-15T08:05:00Z"));
    let token = token_of(&plan);
    assert_eq!(token.len(), 64, "{token}");
    assert!(
        token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "{token}"
    );
    let mut keys: Vec<&str> = plan
        .as_object()
        .expect("plan object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "affected",
            "affected_count",
            "base_rev",
            "diff",
            "expires_at",
            "plan_token",
            "summary",
            "tool"
        ]
    );

    // Nothing changed.
    assert_eq!(rev_of(&mut engine, &doc), rev);
    assert_eq!(glyph_count(&mut engine, &doc), 25);

    // Deleting a single glyph is destructive too.
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["g00"] }),
    );
    let plan = plan_of(&error);
    assert_eq!(plan["affected_count"], json!(1));
    assert_eq!(
        plan["summary"],
        json!("Delete glyph 'g00' (1 glyph affected, destructive)")
    );
    assert_eq!(glyph_count(&mut engine, &doc), 25);
}

#[test]
fn plan_commit_applies_and_undo_restores() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 25);
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev, "glyphs": names(25) }),
    );
    let token = token_of(&plan_of(&error));

    let committed = commit(&mut engine, &token).expect("commit");
    assert_eq!(committed["plan_committed"], json!(true));
    assert_eq!(committed["rev"], json!(rev + 1));
    assert!(committed["tx"].as_u64().is_some(), "{committed}");
    assert!(
        committed["touched"].as_array().expect("touched").len() >= 25,
        "{committed}"
    );
    assert_eq!(glyph_count(&mut engine, &doc), 0);
    assert_eq!(rev_of(&mut engine, &doc), rev + 1);

    let undone = ok(&mut engine, "history_undo", json!({ "doc": doc }));
    assert_eq!(undone["rev"], json!(rev + 2));
    assert_eq!(glyph_count(&mut engine, &doc), 25);
    let glyph = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "g07" }),
    );
    assert_eq!(glyph["glyph"]["paths"].as_array().expect("paths").len(), 1);

    let history = ok(
        &mut engine,
        "history_list",
        json!({ "doc": doc, "limit": 2 }),
    );
    let entries = history["entries"].as_array().expect("entries");
    assert_eq!(entries[0]["kind"], json!("undo"));
    assert_eq!(entries[1]["kind"], json!("commit"));
    assert_eq!(entries[1]["actor"], json!("agent:test"));
    assert_eq!(entries[1]["label"], json!("Delete 25 glyphs"));
}

#[test]
fn expired_token_rejected() {
    let clock = TestClock::new();
    let mut engine = engine_with_clock(&clock);
    let (doc, rev) = doc_with_glyphs(&mut engine, 2);
    let delete = |glyph: &str| json!({ "doc": doc, "base_rev": rev, "glyphs": [glyph] });

    let token = token_of(&plan_of(&err(&mut engine, "glyph_delete", delete("g00"))));
    clock.advance(301);
    let error = commit(&mut engine, &token).expect_err("expired");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, "plan expired");
    // An expired plan is removed.
    let error = commit(&mut engine, &token).expect_err("removed");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, "unknown or already used plan");
    assert_eq!(glyph_count(&mut engine, &doc), 2);

    // A plan is still valid at exactly 300 s.
    let token = token_of(&plan_of(&err(&mut engine, "glyph_delete", delete("g01"))));
    clock.advance(300);
    commit(&mut engine, &token).expect("still valid");
    assert_eq!(glyph_names(&mut engine, &doc), ["g00"]);
}

#[test]
fn reused_token_rejected() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 3);
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["g00"] }),
    );
    let token = token_of(&plan_of(&error));
    commit(&mut engine, &token).expect("first use");
    let rev = rev_of(&mut engine, &doc);

    let error = commit(&mut engine, &token).expect_err("second use");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, "unknown or already used plan");
    assert_eq!(rev_of(&mut engine, &doc), rev);

    // A discarded plan cannot be committed either.
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["g01"] }),
    );
    let token = token_of(&plan_of(&error));
    let discarded = ok(&mut engine, "plan_discard", json!({ "plan_token": token }));
    assert_eq!(discarded, json!({ "discarded": true }));
    let error = commit(&mut engine, &token).expect_err("discarded");
    assert_eq!(error.message, "unknown or already used plan");
    let error = err(&mut engine, "plan_discard", json!({ "plan_token": token }));
    assert_eq!(error.message, "unknown or already used plan");
    assert_eq!(glyph_names(&mut engine, &doc), ["g01", "g02"]);
}

#[test]
fn other_actor_token_rejected() {
    let mut engine = engine();
    let alice = Actor::Agent("alice".into());
    let bob = Actor::Agent("bob".into());
    let (doc, rev) = doc_with_glyphs(&mut engine, 2);
    let error = err_as(
        &mut engine,
        &alice,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["g00"] }),
    );
    let token = token_of(&plan_of(&error));

    for actor in [&bob, &Actor::User, &Actor::Cli] {
        for tool in ["plan_commit", "plan_discard"] {
            let error = err_as(&mut engine, actor, tool, json!({ "plan_token": token }));
            assert_eq!(error.code, ErrorCode::InvalidArgument, "{actor} {tool}");
            assert_eq!(error.message, "plan belongs to another actor");
        }
    }
    assert_eq!(glyph_count(&mut engine, &doc), 2);

    // The attempts did not use the plan up.
    let committed = ok_as(
        &mut engine,
        &alice,
        "plan_commit",
        json!({ "plan_token": token }),
    );
    assert_eq!(committed["plan_committed"], json!(true));
    assert_eq!(glyph_names(&mut engine, &doc), ["g01"]);
    let history = ok(
        &mut engine,
        "history_list",
        json!({ "doc": doc, "limit": 1 }),
    );
    assert_eq!(history["entries"][0]["actor"], json!("agent:alice"));
}

#[test]
fn tampered_token_rejected() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 2);
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["g00"] }),
    );
    let token = token_of(&plan_of(&error));

    let flip = |index: usize| -> String {
        let mut chars: Vec<char> = token.chars().collect();
        chars[index] = if chars[index] == 'A' { 'B' } else { 'A' };
        chars.into_iter().collect()
    };
    // The signature part (after the 16-byte plan id).
    for index in [30, 63] {
        let error = commit(&mut engine, &flip(index)).expect_err("tampered signature");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert_eq!(error.message, "plan token is not valid", "{index}");
    }
    // The plan id part.
    let error = commit(&mut engine, &flip(0)).expect_err("tampered id");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, "unknown or already used plan");
    // Not a token at all.
    for bad in [
        String::new(),
        token[..63].to_owned(),
        format!("{token}AA"),
        format!("{}!", &token[..63]),
        format!("{token}="),
        "not a token".to_owned(),
    ] {
        let error = commit(&mut engine, &bad).expect_err("malformed");
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{bad}");
        assert_eq!(error.message, "malformed plan token", "{bad}");
        let error = err(&mut engine, "plan_discard", json!({ "plan_token": bad }));
        assert_eq!(error.message, "malformed plan token");
    }
    // The commit takes the token only: the planned call cannot be redirected.
    for extra in [
        json!({ "plan_token": token, "doc": doc }),
        json!({ "plan_token": token, "args": { "glyphs": ["g01"] } }),
        json!({ "plan_token": token, "base_rev": 0 }),
    ] {
        let error = err(&mut engine, "plan_commit", extra);
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }
    assert_eq!(glyph_count(&mut engine, &doc), 2);

    // None of the attempts used the plan up.
    commit(&mut engine, &token).expect("genuine token");
    assert_eq!(glyph_names(&mut engine, &doc), ["g01"]);
}

#[test]
fn dry_run_returns_plan_without_changes() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 25);
    let history = history_len(&mut engine, &doc);

    let planned = ok(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev, "glyphs": names(25), "dry_run": true }),
    );
    let mut keys: Vec<&String> = planned.as_object().expect("object").keys().collect();
    keys.sort_unstable();
    assert_eq!(keys, ["plan", "rev"]);
    assert_eq!(planned["rev"], json!(rev));
    let plan = &planned["plan"];
    assert_eq!(plan["tool"], json!("glyph_delete"));
    assert_eq!(plan["affected_count"], json!(25));
    assert_eq!(plan["diff"]["removed"], json!(names(25)));
    assert_eq!(rev_of(&mut engine, &doc), rev);
    assert_eq!(glyph_count(&mut engine, &doc), 25);
    assert_eq!(history_len(&mut engine, &doc), history);

    // The plan of a dry run is committable, and the committed call is no
    // longer a dry run.
    let committed = commit(&mut engine, &token_of(plan)).expect("commit");
    assert_eq!(committed["rev"], json!(rev + 1));
    assert_eq!(committed["plan_committed"], json!(true));
    assert_eq!(glyph_count(&mut engine, &doc), 0);

    // `dry_run` must be a boolean.
    let error = err(
        &mut engine,
        "glyph_update",
        json!({ "doc": doc, "base_rev": rev + 1, "glyph": "x", "export": false, "dry_run": "yes" }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("dry_run"), "{}", error.message);
}

#[test]
fn auto_policy_applies_mass_nondestructive() {
    let mut engine = engine_with_policy(Policy::Auto);
    let (doc, rev) = doc_with_glyphs(&mut engine, 25);
    let applied = ok(&mut engine, "metrics_set", advance_all(&doc, rev, 25, 700));
    assert_eq!(applied["rev"], json!(rev + 1));
    assert!(applied.get("plan_committed").is_none());
    let glyph = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "g24" }),
    );
    assert_eq!(glyph["glyph"]["advance_width"], json!(700.0));

    // Destructive calls still need a plan.
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev + 1, "glyphs": ["g00"] }),
    );
    assert_eq!(plan_of(&error)["tool"], json!("glyph_delete"));
    assert_eq!(glyph_count(&mut engine, &doc), 25);

    // Under the default policy the same massive call needs a plan.
    let mut engine = engine_with_policy(Policy::Prompted);
    let (doc, rev) = doc_with_glyphs(&mut engine, 25);
    let error = err(&mut engine, "metrics_set", advance_all(&doc, rev, 25, 700));
    let plan = plan_of(&error);
    assert_eq!(plan["affected_count"], json!(25));
    assert!(
        plan["summary"]
            .as_str()
            .expect("summary")
            .ends_with("(25 glyphs affected)"),
        "{plan}"
    );
    assert_eq!(rev_of(&mut engine, &doc), rev);
    // Twenty glyphs are not massive.
    let applied = ok(&mut engine, "metrics_set", advance_all(&doc, rev, 20, 650));
    assert_eq!(applied["rev"], json!(rev + 1));
    // The plan made before is now outdated: the glyphs changed after it.
    let error = commit(&mut engine, &token_of(&plan)).expect_err("conflict");
    assert_eq!(error.code, ErrorCode::RevisionConflict);
}

// ----------------------------------------------------------- from the rulings

#[test]
fn dry_run_on_non_destructive_returns_committable_plan() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 1);
    let planned = ok(
        &mut engine,
        "glyph_update",
        json!({ "doc": doc, "base_rev": rev, "glyph": "g00", "name": "x", "dry_run": true }),
    );
    let plan = &planned["plan"];
    assert_eq!(planned["rev"], json!(rev));
    assert_eq!(plan["tool"], json!("glyph_update"));
    assert_eq!(
        plan["summary"],
        json!("Update glyph 'g00' (1 glyph affected)")
    );
    assert_eq!(plan["affected"], json!(["x"]));
    assert_eq!(plan["diff"]["modified"][0]["name"], json!("x"));
    assert_eq!(glyph_names(&mut engine, &doc), ["g00"]);

    let committed = commit(&mut engine, &token_of(plan)).expect("commit");
    assert_eq!(committed["rev"], json!(rev + 1));
    assert_eq!(committed["plan_committed"], json!(true));
    assert!(committed["tx"].as_u64().is_some());
    assert_eq!(glyph_names(&mut engine, &doc), ["x"]);

    // A committed plan answers with the tool's own result.
    let planned = ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": rev + 1, "name": "y", "advance_width": 10,
                "dry_run": true }),
    );
    assert_eq!(planned["plan"]["diff"]["added"], json!(["y"]));
    let committed = commit(&mut engine, &token_of(&planned["plan"])).expect("commit");
    assert_eq!(committed["glyph"]["name"], json!("y"));
    assert_eq!(committed["plan_committed"], json!(true));

    // Every mutating tool takes `dry_run` (default false), not required.
    for name in PLANNABLE {
        let spec = forma_engine::find(name).expect("registered");
        let schema = (spec.input_schema)();
        let dry_run = &schema["properties"]["dry_run"];
        assert_eq!(dry_run["type"], json!("boolean"), "{name}: {schema}");
        assert_eq!(dry_run["default"], json!(false), "{name}");
        assert!(
            !schema["required"]
                .as_array()
                .expect("required")
                .contains(&json!("dry_run")),
            "{name}"
        );
        assert_eq!(schema["additionalProperties"], json!(false), "{name}");
        // The result schema describes both answers.
        let output = spec.output_schema.expect("output schema")();
        assert_eq!(output["type"], json!("object"), "{name}");
        assert!(output["properties"]["plan"].is_object(), "{name}: {output}");
        assert!(output["properties"]["rev"].is_object(), "{name}: {output}");
    }
    // Inside a batch an operation cannot carry `dry_run`.
    let error = err(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": rev + 2, "ops": [
            { "tool": "glyph_update", "args": { "glyph": "x", "export": false, "dry_run": true } }
        ] }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(error.message.contains("dry_run"), "{}", error.message);
}

#[test]
fn strict_policy_refuses_approval_needing_ops_only() {
    let mut engine = engine_with_policy(Policy::Strict);
    let (doc, rev) = doc_with_glyphs(&mut engine, 25);
    let strict = "strict policy requires the FORMA app (milestone 3)";

    for (tool, args) in [
        (
            "glyph_delete",
            json!({ "doc": doc, "base_rev": rev, "glyphs": ["g00"] }),
        ),
        (
            "glyph_delete",
            json!({ "doc": doc, "base_rev": rev, "glyphs": ["g00"], "dry_run": true }),
        ),
        ("metrics_set", advance_all(&doc, rev, 25, 700)),
        (
            "glyph_set_outline",
            json!({ "doc": doc, "base_rev": rev, "glyph": "g01", "svg_path": SQUARE }),
        ),
        (
            "history_restore",
            json!({ "doc": doc, "base_rev": rev, "rev": 1 }),
        ),
    ] {
        let error = err(&mut engine, tool, args);
        assert_eq!(error.code, ErrorCode::Unsupported, "{tool}");
        assert_eq!(error.message, strict, "{tool}");
    }
    assert_eq!(rev_of(&mut engine, &doc), rev);

    // Operations that need no approval run normally.
    let applied = ok(&mut engine, "metrics_set", advance_all(&doc, rev, 2, 700));
    assert_eq!(applied["rev"], json!(rev + 1));
    let planned = ok(
        &mut engine,
        "glyph_update",
        json!({ "doc": doc, "base_rev": rev + 1, "glyph": "g00", "export": false,
                "dry_run": true }),
    );
    let committed = commit(&mut engine, &token_of(&planned["plan"])).expect("commit");
    assert_eq!(committed["rev"], json!(rev + 2));
}

#[test]
fn batch_with_destructive_op_requires_plan() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 2);
    let batch = json!({ "doc": doc, "base_rev": rev, "ops": [
        { "tool": "glyph_create", "args": { "name": "B", "advance_width": 10 } },
        { "tool": "glyph_delete", "args": { "glyphs": ["g00"] } }
    ] });
    let error = err(&mut engine, "edit_batch", batch);
    let plan = plan_of(&error);
    assert_eq!(plan["tool"], json!("edit_batch"));
    assert_eq!(plan["affected_count"], json!(2));
    assert_eq!(
        plan["summary"],
        json!("Batch (2 operations) (2 glyphs affected, destructive)")
    );
    assert_eq!(plan["diff"]["added"], json!(["B"]));
    assert_eq!(plan["diff"]["removed"], json!(["g00"]));
    assert_eq!(glyph_names(&mut engine, &doc), ["g00", "g01"]);

    let committed = commit(&mut engine, &token_of(&plan)).expect("commit");
    assert_eq!(committed["rev"], json!(rev + 1));
    assert_eq!(glyph_names(&mut engine, &doc), ["g01", "B"]);

    // Replacing a drawn outline inside a batch is destructive too.
    let error = err(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": rev + 1, "ops": [
            { "tool": "glyph_update", "args": { "glyph": "g01", "name": "renamed" } },
            { "tool": "glyph_set_outline", "args": { "glyph": "renamed", "svg_path": SQUARE } }
        ] }),
    );
    assert_eq!(plan_of(&error)["affected_count"], json!(1));

    // A small batch that destroys nothing applies at once.
    let applied = ok(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": rev + 1, "ops": [
            { "tool": "glyph_update", "args": { "glyph": "g01", "export": false } }
        ] }),
    );
    assert_eq!(applied["rev"], json!(rev + 2));
}

#[test]
fn set_outline_on_empty_layer_needs_no_plan() {
    let mut engine = engine();
    let doc = new_doc(&mut engine);
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "E", "advance_width": 500 }),
    );
    let applied = ok(
        &mut engine,
        "glyph_set_outline",
        json!({ "doc": doc, "base_rev": 1, "glyph": "E", "svg_path": SQUARE }),
    );
    assert_eq!(applied["rev"], json!(2));

    // Now the layer has a contour: replacing it destroys it.
    let error = err(
        &mut engine,
        "glyph_set_outline",
        json!({ "doc": doc, "base_rev": 2, "glyph": "E", "svg_path": "M0 0 L5 0 L5 5 Z" }),
    );
    let plan = plan_of(&error);
    assert_eq!(plan["diff"]["modified"][0]["outline_changed"], json!(true));
    assert_eq!(rev_of(&mut engine, &doc), 2);

    // A glyph created and drawn in the same batch holds nothing older.
    let applied = ok(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": 2, "ops": [
            { "tool": "glyph_create", "args": { "name": "F", "advance_width": 10 } },
            { "tool": "glyph_set_outline", "args": { "glyph": "F", "svg_path": SQUARE } }
        ] }),
    );
    assert_eq!(applied["rev"], json!(3));
}

#[test]
fn plan_commit_conflicts_if_glyph_changed_after_planning() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 2);
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["g00"] }),
    );
    let token = token_of(&plan_of(&error));

    // Someone changes the glyph after the plan was made.
    ok_as(
        &mut engine,
        &Actor::User,
        "metrics_set",
        json!({ "doc": doc, "base_rev": rev, "items": [{ "glyph": "g00", "advance_width": 9 }] }),
    );
    let error = commit(&mut engine, &token).expect_err("conflict");
    assert_eq!(error.code, ErrorCode::RevisionConflict);
    assert_eq!(error.details["current_rev"], json!(rev + 1));
    assert_eq!(error.details["conflicts"][0]["actor"], json!("user"));
    assert_eq!(glyph_names(&mut engine, &doc), ["g00", "g01"]);

    // The plan was used up although it failed.
    let error = commit(&mut engine, &token).expect_err("used");
    assert_eq!(error.message, "unknown or already used plan");

    // The failed commit leaves no approval behind.
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev + 1, "glyphs": ["g00"] }),
    );
    assert_eq!(error.code, ErrorCode::ApprovalRequired);
    assert_eq!(glyph_count(&mut engine, &doc), 2);
}

#[test]
fn plan_removed_when_document_closes() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 1);
    let (other, other_rev) = doc_with_glyphs(&mut engine, 1);
    let delete = |doc: &str, rev: u64| json!({ "doc": doc, "base_rev": rev, "glyphs": ["g00"] });
    let token = token_of(&plan_of(&err(
        &mut engine,
        "glyph_delete",
        delete(&doc, rev),
    )));
    let other_token = token_of(&plan_of(&err(
        &mut engine,
        "glyph_delete",
        delete(&other, other_rev),
    )));

    // The document is unsaved: discarding it needs a plan too.
    confirmed(
        &mut engine,
        "document_close",
        json!({ "doc": doc, "discard_changes": true }),
    );
    let error = commit(&mut engine, &token).expect_err("closed");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, "unknown or already used plan");

    // The plans of other documents stay.
    commit(&mut engine, &other_token).expect("other document");
    assert_eq!(glyph_count(&mut engine, &other), 0);
}

#[test]
fn plan_store_cap_evicts_oldest() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 1);
    let tokens: Vec<String> = (0..1001)
        .map(|index| {
            let planned = ok(
                &mut engine,
                "glyph_update",
                json!({ "doc": doc, "base_rev": rev, "glyph": "g00",
                        "export": index % 2 == 0, "dry_run": true }),
            );
            token_of(&planned["plan"])
        })
        .collect();

    let error = commit(&mut engine, &tokens[0]).expect_err("evicted");
    assert_eq!(error.message, "unknown or already used plan");
    // The second oldest is still pending.
    let committed = commit(&mut engine, &tokens[1]).expect("pending");
    assert_eq!(committed["rev"], json!(rev + 1));
    // And so is the newest (it now conflicts: the glyph changed since).
    let error = commit(&mut engine, &tokens[1000]).expect_err("conflict");
    assert_eq!(error.code, ErrorCode::RevisionConflict);
}

#[test]
fn save_over_other_existing_file_returns_plan_then_commit_writes() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = Engine::new(EngineConfig {
        allowed_roots: vec![root.path().to_path_buf()],
        ..EngineConfig::default()
    });
    let a = dunce::canonicalize(root.path())
        .expect("root")
        .join("a.forma");
    let b = a.with_file_name("b.forma");
    forma_store::save(&Font::new("A", 1000), &a).expect("fixture");
    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "B", "path": text(&b) }),
    );
    let doc = created["doc"].as_str().expect("doc").to_owned();
    let before = std::fs::read(&a).expect("read");

    let error = err(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 0, "path": text(&a) }),
    );
    let plan = plan_of(&error);
    assert_eq!(plan["tool"], json!("document_save"));
    assert_eq!(plan["summary"], json!(format!("overwrites {}", text(&a))));
    assert_eq!(plan["diff"], empty_diff());
    assert_eq!(plan["affected"], json!([]));
    assert_eq!(plan["affected_count"], json!(0));
    assert_eq!(
        std::fs::read(&a).expect("read"),
        before,
        "a.forma untouched"
    );

    // A dry run plans without writing, also for a save that needs no approval.
    let planned = ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 0, "path": text(&a), "dry_run": true }),
    );
    assert_eq!(planned["rev"], json!(0));
    assert_eq!(planned["plan"]["summary"], plan["summary"]);
    let c = a.with_file_name("c.forma");
    let planned = ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 0, "path": text(&c), "dry_run": true }),
    );
    assert_eq!(
        planned["plan"]["summary"],
        json!(format!("saves to {}", text(&c)))
    );
    assert!(!c.exists());
    assert_eq!(std::fs::read(&a).expect("read"), before);

    let committed = commit(&mut engine, &token_of(&plan)).expect("commit");
    assert_eq!(committed["plan_committed"], json!(true));
    assert_eq!(committed["path"], json!(text(&a)));
    assert_eq!(committed["doc"], json!(doc));
    let saved = forma_store::load(&a).expect("load");
    assert_eq!(saved.info.family_name, "B");
    assert_eq!(inspect(&mut engine, &doc)["path"], json!(text(&a)));
}

// ------------------------------------------------------------------- extras

#[test]
fn plan_tools_are_registered_with_their_schemas() {
    for name in ["plan_commit", "plan_discard"] {
        let spec = forma_engine::find(name).expect("registered");
        let input = (spec.input_schema)();
        assert_eq!(input["required"], json!(["plan_token"]), "{name}");
        assert_eq!(input["additionalProperties"], json!(false), "{name}");
        let output = spec.output_schema.expect("output schema")();
        assert_eq!(output["type"], json!("object"), "{name}");
    }
    let commit = forma_engine::find("plan_commit").expect("registered");
    assert!(commit.effects.destructive);
    let output = commit.output_schema.expect("output schema")();
    assert!(
        output["properties"]["plan_committed"].is_object(),
        "{output}"
    );
    assert!(registry().iter().any(|spec| spec.name == "plan_discard"));
}

#[test]
fn committing_runs_only_the_planned_call() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 3);
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev, "glyphs": ["g00"] }),
    );
    let token = token_of(&plan_of(&error));
    commit(&mut engine, &token).expect("commit");
    assert_eq!(glyph_names(&mut engine, &doc), ["g01", "g02"]);

    // The next destructive call needs its own plan.
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev + 1, "glyphs": ["g01"] }),
    );
    assert_eq!(error.code, ErrorCode::ApprovalRequired);
    assert_eq!(glyph_count(&mut engine, &doc), 2);
}

#[test]
fn a_commit_that_fails_early_grants_nothing() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 3);
    let delete_g00 = json!({ "doc": doc, "base_rev": rev, "glyphs": ["g00"] });
    let first = token_of(&plan_of(&err(
        &mut engine,
        "glyph_delete",
        delete_g00.clone(),
    )));
    let second = token_of(&plan_of(&err(&mut engine, "glyph_delete", delete_g00)));
    commit(&mut engine, &second).expect("commit");

    // The first plan's call now fails before it reaches the gate (`g00` was
    // deleted after the plan's base revision: a conflict) ...
    let error = commit(&mut engine, &first).expect_err("no glyph to delete");
    assert_eq!(error.code, ErrorCode::RevisionConflict, "{error:?}");
    // ... and the pass it carried does not linger for the next call.
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev + 1, "glyphs": ["g01"] }),
    );
    assert_eq!(error.code, ErrorCode::ApprovalRequired);
    assert_eq!(glyph_names(&mut engine, &doc), ["g01", "g02"]);
}

#[test]
fn commit_refuses_commands_the_plan_did_not_show() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 1);
    let before = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "g00" }),
    );
    // Replace the drawn outline, in SVG coordinates (flipped at the
    // ascender).
    let error = err(
        &mut engine,
        "glyph_set_outline",
        json!({ "doc": doc, "base_rev": rev, "glyph": "g00",
                "coordinate_system": "svg_y_down", "svg_path": "M0 0 L10 0 L10 10 Z" }),
    );
    let token = token_of(&plan_of(&error));

    // A change that does not touch the glyph but changes what the call
    // would draw: the plan no longer describes the call.
    ok(
        &mut engine,
        "font_set_vertical_metrics",
        json!({ "doc": doc, "base_rev": rev, "ascender": 900 }),
    );
    let error = commit(&mut engine, &token).expect_err("outdated plan");
    assert_eq!(error.code, ErrorCode::RevisionConflict);
    assert!(error.message.contains("new plan"), "{}", error.message);
    assert_eq!(error.details["current_rev"], json!(rev + 1));
    let after = ok(
        &mut engine,
        "glyph_get",
        json!({ "doc": doc, "glyph": "g00" }),
    );
    assert_eq!(after["glyph"], before["glyph"]);
}

#[test]
fn commit_refuses_an_overwrite_the_plan_did_not_show() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = Engine::new(EngineConfig {
        allowed_roots: vec![root.path().to_path_buf()],
        ..EngineConfig::default()
    });
    let doc = new_doc(&mut engine);
    let target = dunce::canonicalize(root.path())
        .expect("root")
        .join("later.forma");
    let planned = ok(
        &mut engine,
        "document_save",
        json!({ "doc": doc, "base_rev": 0, "path": text(&target), "dry_run": true }),
    );
    assert_eq!(
        planned["plan"]["summary"],
        json!(format!("saves to {}", text(&target)))
    );

    // The file appears after the plan was made: committing would overwrite
    // it, which the plan did not say.
    forma_store::save(&Font::new("Other", 1000), &target).expect("fixture");
    let existing = std::fs::read(&target).expect("read");
    let error = commit(&mut engine, &token_of(&planned["plan"])).expect_err("outdated");
    assert_eq!(error.code, ErrorCode::RevisionConflict);
    assert_eq!(std::fs::read(&target).expect("read"), existing);
    assert_eq!(inspect(&mut engine, &doc)["path"], Value::Null);
}

// ------------------------------------------------------------ fix round 1

/// The size of a value as JSON text.
fn json_len(value: &Value) -> usize {
    value.to_string().len()
}

#[test]
fn big_changes_give_bounded_plans_and_diffs() {
    let mut engine = engine();
    let doc = new_doc(&mut engine);
    let ops: Vec<Value> = (0..500)
        .map(|index| {
            json!({ "tool": "glyph_create",
                    "args": { "name": format!("g{index:03}"), "advance_width": 500 } })
        })
        .collect();
    let error = err(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": 0, "ops": ops }),
    );
    let plan = plan_of(&error);
    assert!(
        json_len(&error.details) < 16_384,
        "{}",
        json_len(&error.details)
    );
    assert_eq!(plan["affected_count"], json!(500));
    assert_eq!(plan["affected"].as_array().expect("affected").len(), 100);
    let diff = &plan["diff"];
    assert_eq!(diff["added_count"], json!(500));
    assert_eq!(diff["added"].as_array().expect("added").len(), 100);
    assert_eq!(diff["added"][0], json!("g000"));
    assert_eq!(diff["removed_count"], json!(0));
    assert_eq!(diff["truncated"], json!(true));
    commit(&mut engine, &token_of(&plan)).expect("commit");

    let items: Vec<Value> = (0..500)
        .map(|index| json!({ "glyph": format!("g{index:03}"), "advance_width": 600 }))
        .collect();
    let error = err(
        &mut engine,
        "metrics_set",
        json!({ "doc": doc, "base_rev": 1, "items": items }),
    );
    let plan = plan_of(&error);
    assert!(
        json_len(&error.details) < 16_384,
        "{}",
        json_len(&error.details)
    );
    assert_eq!(plan["diff"]["modified_count"], json!(500));
    assert_eq!(
        plan["diff"]["modified"].as_array().expect("modified").len(),
        100
    );
    assert_eq!(plan["diff"]["truncated"], json!(true));
    commit(&mut engine, &token_of(&plan)).expect("commit");

    let diff = ok(
        &mut engine,
        "document_diff",
        json!({ "doc": doc, "from_rev": 0 }),
    );
    assert!(json_len(&diff) < 16_384, "{}", json_len(&diff));
    assert_eq!(diff["diff"]["added_count"], json!(500));
    assert_eq!(diff["diff"]["added"].as_array().expect("added").len(), 100);
    assert_eq!(diff["diff"]["truncated"], json!(true));
    let diff = ok(
        &mut engine,
        "document_diff",
        json!({ "doc": doc, "from_rev": 1, "to_rev": 2 }),
    );
    assert_eq!(diff["diff"]["modified_count"], json!(500));
    assert_eq!(diff["diff"]["added_count"], json!(0));

    // A small diff is complete.
    let planned = ok(
        &mut engine,
        "glyph_update",
        json!({ "doc": doc, "base_rev": 2, "glyph": "g000", "export": false,
                "dry_run": true }),
    );
    assert_eq!(planned["plan"]["diff"]["modified_count"], json!(1));
    assert_eq!(planned["plan"]["diff"]["truncated"], json!(false));
}

#[test]
fn closing_a_dirty_document_with_discard_requires_plan() {
    let mut engine = engine();
    let doc = new_doc(&mut engine); // in memory: unsaved
    let close = json!({ "doc": doc, "discard_changes": true });
    let error = err(&mut engine, "document_close", close.clone());
    let plan = plan_of(&error);
    assert_eq!(plan["tool"], json!("document_close"));
    assert_eq!(
        plan["summary"],
        json!(format!("closes {doc} and discards its unsaved changes"))
    );
    assert_eq!(plan["diff"], empty_diff());
    assert_eq!(plan["base_rev"], json!(0));
    assert_eq!(rev_of(&mut engine, &doc), 0, "still open");

    let mut dry = close.clone();
    dry["dry_run"] = json!(true);
    let planned = ok(&mut engine, "document_close", dry);
    assert_eq!(planned["rev"], json!(0));
    assert_eq!(rev_of(&mut engine, &doc), 0, "still open");

    let closed = commit(&mut engine, &token_of(&plan)).expect("commit");
    assert_eq!(closed["closed"], json!(true));
    assert_eq!(closed["plan_committed"], json!(true));
    let error = err(&mut engine, "document_inspect", json!({ "doc": doc }));
    assert_eq!(error.code, ErrorCode::NotFound);

    // A saved document closes at once, with or without `discard_changes`.
    let root = TempDir::new().expect("tempdir");
    let mut engine = Engine::new(EngineConfig {
        allowed_roots: vec![root.path().to_path_buf()],
        ..EngineConfig::default()
    });
    for discard in [false, true] {
        let path = root.path().join(format!("clean-{discard}.forma"));
        let created = ok(
            &mut engine,
            "document_create",
            json!({ "family_name": "Clean", "path": text(&path) }),
        );
        let closed = ok(
            &mut engine,
            "document_close",
            json!({ "doc": created["doc"], "discard_changes": discard }),
        );
        assert_eq!(closed["closed"], json!(true));
    }
}

#[test]
fn file_and_close_plans_pin_the_document_revision() {
    let root = TempDir::new().expect("tempdir");
    let mut engine = Engine::new(EngineConfig {
        allowed_roots: vec![root.path().to_path_buf()],
        ..EngineConfig::default()
    });
    let a = dunce::canonicalize(root.path())
        .expect("root")
        .join("a.forma");
    forma_store::save(&Font::new("A", 1000), &a).expect("fixture");
    let created = ok(
        &mut engine,
        "document_create",
        json!({ "family_name": "B", "path": text(&a.with_file_name("b.forma")) }),
    );
    let doc = created["doc"].as_str().expect("doc").to_owned();
    let save = json!({ "doc": doc, "base_rev": 0, "path": text(&a) });
    let token = token_of(&plan_of(&err(&mut engine, "document_save", save)));

    // The document changes after the plan: what would be written is not what
    // the user approved.
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 0, "name": "x", "advance_width": 1 }),
    );
    let before = std::fs::read(&a).expect("read");
    let error = commit(&mut engine, &token).expect_err("pinned");
    assert_eq!(error.code, ErrorCode::RevisionConflict);
    assert_eq!(error.details["current_rev"], json!(1));
    assert_eq!(std::fs::read(&a).expect("read"), before);

    // A plan made at the current revision writes; a stale `base_rev` is still
    // accepted by document_save.
    let save = json!({ "doc": doc, "base_rev": 0, "path": text(&a) });
    let token = token_of(&plan_of(&err(&mut engine, "document_save", save)));
    commit(&mut engine, &token).expect("commit");
    assert_eq!(
        forma_store::load(&a).expect("load").glyphs.len(),
        1,
        "the new glyph is saved"
    );

    // Discarding unsaved changes is pinned the same way.
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 1, "name": "y", "advance_width": 1 }),
    );
    let close = json!({ "doc": doc, "discard_changes": true });
    let token = token_of(&plan_of(&err(&mut engine, "document_close", close)));
    ok(
        &mut engine,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 2, "name": "z", "advance_width": 1 }),
    );
    let error = commit(&mut engine, &token).expect_err("pinned");
    assert_eq!(error.code, ErrorCode::RevisionConflict);
    assert_eq!(rev_of(&mut engine, &doc), 3, "still open");
}

#[test]
fn no_plan_for_a_call_that_would_conflict() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 2);
    ok_as(
        &mut engine,
        &Actor::User,
        "metrics_set",
        json!({ "doc": doc, "base_rev": rev, "items": [{ "glyph": "g00", "advance_width": 9 }] }),
    );
    for dry_run in [false, true] {
        let error = err(
            &mut engine,
            "glyph_delete",
            json!({ "doc": doc, "base_rev": rev, "glyphs": ["g00"], "dry_run": dry_run }),
        );
        assert_eq!(error.code, ErrorCode::RevisionConflict, "dry_run {dry_run}");
        assert_eq!(error.details["current_rev"], json!(rev + 1));
        assert_eq!(error.details["conflicts"][0]["actor"], json!("user"));
    }
    // Based on the latest revision, the plan is offered.
    let error = err(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": rev + 1, "glyphs": ["g00"] }),
    );
    assert_eq!(error.code, ErrorCode::ApprovalRequired);
}

#[test]
fn emptying_a_drawn_layer_or_collapsing_a_contour_requires_plan() {
    let mut engine = engine();
    let (doc, rev) = doc_with_glyphs(&mut engine, 1);
    let delete_contour = json!({ "glyph": "g00", "action": { "type": "delete", "path_id": 0 } });

    // Deleting the last contour of a drawn layer.
    let mut args = delete_contour.clone();
    args["doc"] = json!(doc);
    args["base_rev"] = json!(rev);
    let plan = plan_of(&err(&mut engine, "contour_edit", args));
    assert!(
        plan["summary"]
            .as_str()
            .expect("summary")
            .ends_with("destructive)"),
        "{plan}"
    );
    // ... also when a later operation of the batch draws the layer again.
    let error = err(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": rev, "ops": [
            { "tool": "contour_edit", "args": delete_contour },
            { "tool": "glyph_set_outline", "args": { "glyph": "g00", "svg_path": SQUARE } }
        ] }),
    );
    assert_eq!(error.code, ErrorCode::ApprovalRequired);

    // A singular transform collapses the contour.
    for affine in [
        [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [2.0, 4.0, 1.0, 2.0, 5.0, 5.0],
    ] {
        let error = err(
            &mut engine,
            "contour_edit",
            json!({ "doc": doc, "base_rev": rev, "glyph": "g00",
                    "action": { "type": "transform", "path_id": 0, "affine": affine } }),
        );
        assert_eq!(error.code, ErrorCode::ApprovalRequired, "{affine:?}");
    }
    assert_eq!(rev_of(&mut engine, &doc), rev);

    // Deleting one of two contours, or an invertible transform, applies.
    let applied = ok(
        &mut engine,
        "contour_edit",
        json!({ "doc": doc, "base_rev": rev, "glyph": "g00",
                "action": { "type": "transform", "path_id": 0,
                            "affine": [1.0, 0.0, 0.0, 1.0, 10.0, 0.0] } }),
    );
    assert_eq!(applied["rev"], json!(rev + 1));
    ok(
        &mut engine,
        "contour_edit",
        json!({ "doc": doc, "base_rev": rev + 1, "glyph": "g00",
                "action": { "type": "add", "svg_path": "M200 0 L300 0 L300 100 Z" } }),
    );
    let applied = ok(
        &mut engine,
        "contour_edit",
        json!({ "doc": doc, "base_rev": rev + 2, "glyph": "g00",
                "action": { "type": "delete", "path_id": 0 } }),
    );
    assert_eq!(applied["rev"], json!(rev + 3));
}

#[test]
fn descriptions_do_not_promise_approval_unconditionally() {
    for spec in registry() {
        assert!(
            !spec.description.contains("answers APPROVAL_REQUIRED"),
            "{}: {}",
            spec.name,
            spec.description
        );
    }
    for name in [
        "document_save",
        "document_close",
        "glyph_delete",
        "glyph_set_outline",
        "contour_edit",
        "history_revert",
        "history_restore",
        "export_font",
    ] {
        let spec = forma_engine::find(name).expect("registered");
        assert!(
            spec.description
                .contains("may require approval depending on the server policy"),
            "{name}: {}",
            spec.description
        );
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
