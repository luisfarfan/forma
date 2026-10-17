//! REVISION_CONFLICT details name the glyphs they are about, and a call
//! that refers to a glyph another actor renamed or deleted after the call's
//! `base_rev` gets a REVISION_CONFLICT about that glyph, not NOT_FOUND.

use forma_engine::{Engine, EngineConfig, ErrorCode, ToolError, ToolOutput};
use forma_model::Actor;
use serde_json::{Value, json};

fn user() -> Actor {
    Actor::User
}

fn agent() -> Actor {
    Actor::Agent("claude".into())
}

fn engine() -> Engine {
    Engine::new(EngineConfig::default())
}

#[track_caller]
fn ok(engine: &mut Engine, actor: &Actor, tool: &str, args: Value) -> Value {
    match engine.call(actor, tool, args) {
        Ok(ToolOutput::Json(value)) => value,
        other => panic!("{tool}: {other:?}"),
    }
}

#[track_caller]
fn err(engine: &mut Engine, actor: &Actor, tool: &str, args: Value) -> ToolError {
    match engine.call(actor, tool, args) {
        Ok(output) => panic!("{tool} succeeded: {output:?}"),
        Err(error) => error,
    }
}

const SQUARE: &str = "M100 0 L500 0 L500 400 L100 400 Z";

/// A document with the glyphs `a` and `b` (drawn), at rev 1; returns the
/// handle and the id of `a`.
fn setup(engine: &mut Engine) -> (String, String) {
    let created = ok(
        engine,
        &agent(),
        "document_create",
        json!({ "family_name": "Conflicts" }),
    );
    let doc = created["doc"].as_str().expect("doc").to_owned();
    ok(
        engine,
        &agent(),
        "edit_batch",
        json!({ "doc": doc, "base_rev": 0, "ops": [
            { "tool": "glyph_create", "args": { "name": "a", "advance_width": 500, "svg_path": SQUARE } },
            { "tool": "glyph_create", "args": { "name": "b", "advance_width": 500, "svg_path": SQUARE } }
        ] }),
    );
    let got = ok(
        engine,
        &agent(),
        "glyph_get",
        json!({ "doc": doc, "glyph": "a" }),
    );
    let id = got["glyph"]["id"].as_str().expect("id").to_owned();
    (doc, id)
}

#[test]
fn conflict_details_name_the_glyph() {
    let mut engine = engine();
    let (doc, id) = setup(&mut engine);
    // The user changes `a` (rev 2); the agent, still at rev 1, edits it.
    ok(
        &mut engine,
        &user(),
        "metrics_set",
        json!({ "doc": doc, "base_rev": 1, "items": [{ "glyph": "a", "advance_width": 510 }] }),
    );
    let error = err(
        &mut engine,
        &agent(),
        "metrics_set",
        json!({ "doc": doc, "base_rev": 1, "items": [{ "glyph": "a", "advance_width": 520 }] }),
    );
    assert_eq!(error.code, ErrorCode::RevisionConflict, "{error:?}");
    assert_eq!(
        error.details,
        json!({ "current_rev": 2, "conflicts": [
            { "entity": format!("glyph:{id}"), "rev": 2, "actor": "user", "name": "a" }
        ] })
    );

    // A glyph the user deleted is named as it was.
    let deleted = err(
        &mut engine,
        &user(),
        "glyph_delete",
        json!({ "doc": doc, "base_rev": 2, "glyphs": ["b"] }),
    );
    let token = deleted.details["plan"]["plan_token"].clone();
    ok(
        &mut engine,
        &user(),
        "plan_commit",
        json!({ "plan_token": token }),
    );
    let b_id = ok(
        &mut engine,
        &agent(),
        "history_list",
        json!({ "doc": doc, "limit": 1 }),
    )["entries"][0]["touched"]
        .as_array()
        .expect("touched")
        .iter()
        .filter_map(Value::as_str)
        .find(|key| key.starts_with("glyph:"))
        .expect("the deleted glyph")
        .to_owned();
    // Reverting the batch that created `a` and `b` conflicts with both:
    // `a` (changed) is named as it is, `b` (deleted) as it was.
    let error = err(
        &mut engine,
        &agent(),
        "history_revert",
        json!({ "doc": doc, "base_rev": 1, "tx": 1 }),
    );
    assert_eq!(error.code, ErrorCode::RevisionConflict, "{error:?}");
    let conflicts = error.details["conflicts"].as_array().expect("conflicts");
    let named: Vec<(String, String)> = conflicts
        .iter()
        .map(|conflict| {
            (
                conflict["entity"].as_str().expect("entity").to_owned(),
                conflict["name"].as_str().expect("name").to_owned(),
            )
        })
        .collect();
    assert!(
        named.contains(&(format!("glyph:{id}"), "a".to_owned())),
        "{named:?}"
    );
    assert!(named.contains(&(b_id, "b".to_owned())), "{named:?}");
}

#[test]
fn a_glyph_renamed_after_base_rev_is_a_conflict() {
    let mut engine = engine();
    let (doc, id) = setup(&mut engine);
    ok(
        &mut engine,
        &user(),
        "glyph_update",
        json!({ "doc": doc, "base_rev": 1, "glyph": "a", "name": "a.alt" }),
    );
    for (tool, args) in [
        (
            "glyph_update",
            json!({ "doc": doc, "base_rev": 1, "glyph": "a", "unicodes": [97] }),
        ),
        (
            "metrics_set",
            json!({ "doc": doc, "base_rev": 1, "items": [{ "glyph": "a", "lsb": 10 }] }),
        ),
        (
            "edit_batch",
            json!({ "doc": doc, "base_rev": 1, "ops": [
                { "tool": "glyph_update", "args": { "glyph": "b", "unicodes": [98] } },
                { "tool": "glyph_update", "args": { "glyph": "a", "unicodes": [97] } }
            ] }),
        ),
    ] {
        let error = err(&mut engine, &agent(), tool, args);
        assert_eq!(error.code, ErrorCode::RevisionConflict, "{tool}: {error:?}");
        assert_eq!(error.details["current_rev"], json!(2), "{tool}");
        assert_eq!(
            error.details["conflicts"],
            json!([{ "entity": format!("glyph:{id}"), "rev": 2, "actor": "user",
                     "name": "a.alt" }]),
            "{tool}"
        );
        assert!(error.message.contains("'a'"), "{tool}: {}", error.message);
    }
    // In a batch the failing operation is still named.
    let error = err(
        &mut engine,
        &agent(),
        "edit_batch",
        json!({ "doc": doc, "base_rev": 1, "ops": [
            { "tool": "glyph_update", "args": { "glyph": "b", "unicodes": [98] } },
            { "tool": "glyph_update", "args": { "glyph": "a", "unicodes": [97] } }
        ] }),
    );
    assert_eq!(error.details["op_index"], json!(1), "{error:?}");

    // Based on the current revision, the old name is simply not found.
    let error = err(
        &mut engine,
        &agent(),
        "glyph_update",
        json!({ "doc": doc, "base_rev": 2, "glyph": "a", "unicodes": [97] }),
    );
    assert_eq!(error.code, ErrorCode::NotFound, "{error:?}");
    // A glyph that never existed is not found either.
    let error = err(
        &mut engine,
        &agent(),
        "glyph_update",
        json!({ "doc": doc, "base_rev": 1, "glyph": "zzz", "unicodes": [97] }),
    );
    assert_eq!(error.code, ErrorCode::NotFound, "{error:?}");
}

#[test]
fn a_glyph_deleted_after_base_rev_is_a_conflict() {
    let mut engine = engine();
    let (doc, id) = setup(&mut engine);
    let error = err(
        &mut engine,
        &user(),
        "glyph_delete",
        json!({ "doc": doc, "base_rev": 1, "glyphs": ["a"] }),
    );
    ok(
        &mut engine,
        &user(),
        "plan_commit",
        json!({ "plan_token": error.details["plan"]["plan_token"] }),
    );
    for (tool, args) in [
        (
            "glyph_set_outline",
            json!({ "doc": doc, "base_rev": 1, "glyph": "a", "svg_path": SQUARE }),
        ),
        (
            "contour_edit",
            json!({ "doc": doc, "base_rev": 1, "glyph": id,
                    "action": { "type": "reverse", "path_id": 0 } }),
        ),
        (
            "geometry_fix",
            json!({ "doc": doc, "base_rev": 1, "glyphs": ["a"] }),
        ),
    ] {
        let error = err(&mut engine, &agent(), tool, args);
        assert_eq!(error.code, ErrorCode::RevisionConflict, "{tool}: {error:?}");
        assert_eq!(
            error.details["conflicts"],
            json!([{ "entity": format!("glyph:{id}"), "rev": 2, "actor": "user", "name": "a" }]),
            "{tool}"
        );
    }
    // An edit of `b`, which was never touched, still applies.
    ok(
        &mut engine,
        &agent(),
        "glyph_update",
        json!({ "doc": doc, "base_rev": 1, "glyph": "b", "unicodes": [98] }),
    );
}

#[test]
fn a_plan_whose_glyph_was_deleted_is_a_conflict() {
    let mut engine = engine();
    let (doc, id) = setup(&mut engine);
    // The agent plans to redraw `a`; the user deletes `a` before the agent
    // commits the plan.
    let planned = ok(
        &mut engine,
        &agent(),
        "glyph_set_outline",
        json!({ "doc": doc, "base_rev": 1, "glyph": "a", "svg_path": SQUARE, "dry_run": true }),
    );
    let error = err(
        &mut engine,
        &user(),
        "glyph_delete",
        json!({ "doc": doc, "base_rev": 1, "glyphs": ["a"] }),
    );
    ok(
        &mut engine,
        &user(),
        "plan_commit",
        json!({ "plan_token": error.details["plan"]["plan_token"] }),
    );
    let error = err(
        &mut engine,
        &agent(),
        "plan_commit",
        json!({ "plan_token": planned["plan"]["plan_token"] }),
    );
    assert_eq!(error.code, ErrorCode::RevisionConflict, "{error:?}");
    assert_eq!(
        error.details["conflicts"],
        json!([{ "entity": format!("glyph:{id}"), "rev": 2, "actor": "user", "name": "a" }])
    );
}
