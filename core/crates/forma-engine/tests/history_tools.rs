//! History tools: `history_list`, `history_undo`, `history_redo`,
//! `history_revert`, `history_restore` and `document_diff`.

use forma_engine::{Engine, EngineConfig, ErrorCode, ToolError, ToolOutput};
use forma_model::Actor;
use serde_json::{Value, json};

// ------------------------------------------------------------------ helpers

fn agent() -> Actor {
    Actor::Agent("test".into())
}

fn engine() -> Engine {
    Engine::new(EngineConfig::default())
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

/// Runs a call; if it needs approval, commits its plan.
#[track_caller]
fn approved(engine: &mut Engine, tool: &str, args: Value) -> Value {
    match call_as(engine, &agent(), tool, args) {
        Ok(value) => value,
        Err(error) if error.code == ErrorCode::ApprovalRequired => {
            let token = error.details["plan"]["plan_token"].clone();
            ok(engine, "plan_commit", json!({ "plan_token": token }))
        }
        Err(error) => panic!("{tool} failed: {error:?}"),
    }
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

fn glyph_names(engine: &mut Engine, doc: &str) -> Vec<String> {
    ok(engine, "glyph_list", json!({ "doc": doc, "limit": 500 }))["glyphs"]
        .as_array()
        .expect("glyphs")
        .iter()
        .map(|glyph| glyph["name"].as_str().expect("name").to_owned())
        .collect()
}

const SQUARE: &str = "M0 0 L100 0 L100 100 L0 100 Z";

fn create_glyph(engine: &mut Engine, actor: &Actor, doc: &str, name: &str) -> Value {
    let rev = rev_of(engine, doc);
    ok_as(
        engine,
        actor,
        "glyph_create",
        json!({ "doc": doc, "base_rev": rev, "name": name, "advance_width": 500,
                "svg_path": SQUARE }),
    )
}

fn txs(page: &Value) -> Vec<u64> {
    page["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| entry["tx"].as_u64().expect("tx"))
        .collect()
}

/// A batch creating `count` glyphs `n00`, `n01`, ….
fn create_many(doc: &str, base_rev: u64, count: usize) -> Value {
    let ops: Vec<Value> = (0..count)
        .map(|index| {
            json!({ "tool": "glyph_create",
                    "args": { "name": format!("n{index:02}"), "advance_width": 10 } })
        })
        .collect();
    json!({ "doc": doc, "base_rev": base_rev, "ops": ops, "label": "Create many" })
}

// ------------------------------------------------------------ from the brief

#[test]
fn undo_other_actor_not_allowed_suggests_revert() {
    let mut engine = engine();
    let doc = new_doc(&mut engine);
    let created = create_glyph(&mut engine, &Actor::User, &doc, "A");
    assert_eq!(created["rev"], json!(1));

    let error = err(&mut engine, "history_undo", json!({ "doc": doc }));
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("history_revert"),
        "{}",
        error.message
    );
    assert!(error.message.contains("user"), "{}", error.message);
    assert_eq!(rev_of(&mut engine, &doc), 1);

    // history_revert takes the change back instead (deleting the user's
    // drawn glyph is destructive: it needs a plan).
    let reverted = approved(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 1, "tx": 1 }),
    );
    assert_eq!(reverted["plan_committed"], json!(true));
    assert_eq!(reverted["rev"], json!(2));
    assert_eq!(reverted["tx"], json!(2));
    assert!(
        reverted["touched"]
            .as_array()
            .expect("touched")
            .iter()
            .any(|key| key.as_str().is_some_and(|key| key.starts_with("glyph:"))),
        "{reverted}"
    );
    assert!(glyph_names(&mut engine, &doc).is_empty());

    // The user cannot undo the agent's revert; the agent can.
    let error = err_as(
        &mut engine,
        &Actor::User,
        "history_undo",
        json!({ "doc": doc }),
    );
    assert!(error.message.contains("agent:test"), "{}", error.message);
    let undone = ok(&mut engine, "history_undo", json!({ "doc": doc }));
    assert_eq!(undone["rev"], json!(3));
    assert_eq!(glyph_names(&mut engine, &doc), ["A"]);
}

#[test]
fn restore_requires_plan() {
    let mut engine = engine();
    let doc = new_doc(&mut engine);
    for name in ["A", "B", "C"] {
        create_glyph(&mut engine, &agent(), &doc, name);
    }
    let restore = json!({ "doc": doc, "base_rev": 3, "rev": 1 });

    let error = err(&mut engine, "history_restore", restore.clone());
    assert_eq!(error.code, ErrorCode::ApprovalRequired);
    let plan = &error.details["plan"];
    assert_eq!(plan["tool"], json!("history_restore"));
    assert_eq!(plan["base_rev"], json!(3));
    assert_eq!(
        plan["summary"],
        json!("Restore revision 1 (2 glyphs affected, destructive)")
    );
    assert_eq!(plan["affected"], json!(["B", "C"]));
    assert_eq!(plan["diff"]["removed"], json!(["B", "C"]));
    assert_eq!(glyph_names(&mut engine, &doc), ["A", "B", "C"]);
    assert_eq!(rev_of(&mut engine, &doc), 3);

    // A dry run plans too.
    let mut dry = restore.clone();
    dry["dry_run"] = json!(true);
    let planned = ok(&mut engine, "history_restore", dry);
    assert_eq!(planned["rev"], json!(3));
    assert_eq!(planned["plan"]["affected_count"], json!(2));

    let committed = ok(
        &mut engine,
        "plan_commit",
        json!({ "plan_token": plan["plan_token"] }),
    );
    assert_eq!(committed["rev"], json!(4));
    assert_eq!(committed["plan_committed"], json!(true));
    assert_eq!(glyph_names(&mut engine, &doc), ["A"]);
    let history = ok(
        &mut engine,
        "history_list",
        json!({ "doc": doc, "limit": 1 }),
    );
    assert_eq!(history["entries"][0]["kind"], json!("restore"));
    assert_eq!(history["entries"][0]["label"], json!("Restore revision 1"));

    // A revision that does not exist is NOT_FOUND, before any plan.
    let error = err(
        &mut engine,
        "history_restore",
        json!({ "doc": doc, "base_rev": 4, "rev": 99 }),
    );
    assert_eq!(error.code, ErrorCode::NotFound);
    let error = err(
        &mut engine,
        "history_restore",
        json!({ "doc": doc, "base_rev": 9, "rev": 0 }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[test]
fn diff_between_revisions() {
    let mut engine = engine();
    let doc = new_doc(&mut engine);
    create_glyph(&mut engine, &agent(), &doc, "A"); // rev 1
    ok(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": 1, "ops": [
            { "tool": "glyph_create", "args": { "name": "B", "advance_width": 10 } },
            { "tool": "metrics_set", "args": { "items": [{ "glyph": "A", "advance_width": 600 }] } }
        ] }),
    ); // rev 2
    ok(
        &mut engine,
        "font_set_info",
        json!({ "doc": doc, "base_rev": 2, "family_name": "Other" }),
    ); // rev 3

    let diff = ok(
        &mut engine,
        "document_diff",
        json!({ "doc": doc, "from_rev": 0, "to_rev": 1 }),
    );
    assert_eq!(diff["rev"], json!(3));
    assert_eq!(
        diff["diff"],
        json!({ "added": ["A"], "added_count": 1, "removed": [], "removed_count": 0,
                "modified": [], "modified_count": 0, "info_changed": false,
                "metrics_changed": false, "truncated": false })
    );

    let diff = ok(
        &mut engine,
        "document_diff",
        json!({ "doc": doc, "from_rev": 1, "to_rev": 2 }),
    );
    assert_eq!(diff["diff"]["added"], json!(["B"]));
    assert_eq!(
        diff["diff"]["modified"],
        json!([{ "name": "A", "outline_changed": false, "advance_changed": true,
                 "unicodes_changed": false }])
    );

    // `to_rev` defaults to the current revision.
    let diff = ok(
        &mut engine,
        "document_diff",
        json!({ "doc": doc, "from_rev": 0 }),
    );
    assert_eq!(diff["diff"]["added"], json!(["A", "B"]));
    assert_eq!(diff["diff"]["info_changed"], json!(true));

    // Backwards.
    let diff = ok(
        &mut engine,
        "document_diff",
        json!({ "doc": doc, "from_rev": 2, "to_rev": 0 }),
    );
    assert_eq!(diff["diff"]["removed"], json!(["A", "B"]));

    for args in [
        json!({ "doc": doc, "from_rev": 4 }),
        json!({ "doc": doc, "from_rev": 0, "to_rev": 99 }),
        json!({ "doc": "doc_nope", "from_rev": 0 }),
    ] {
        let error = err(&mut engine, "document_diff", args.clone());
        assert_eq!(error.code, ErrorCode::NotFound, "{args}");
    }
}

// ----------------------------------------------------------- from the rulings

#[test]
fn history_list_newest_first_paginates() {
    let mut engine = engine();
    let doc = new_doc(&mut engine);
    for name in ["A", "B", "C", "D", "E"] {
        create_glyph(&mut engine, &agent(), &doc, name);
    }

    let all = ok(&mut engine, "history_list", json!({ "doc": doc }));
    assert_eq!(all["rev"], json!(5));
    assert_eq!(txs(&all), [5, 4, 3, 2, 1]);
    assert_eq!(all["next_cursor"], Value::Null);
    let newest = &all["entries"][0];
    assert_eq!(newest["rev"], json!(5));
    assert_eq!(newest["actor"], json!("agent:test"));
    assert_eq!(newest["label"], json!("Create glyph 'E'"));
    assert_eq!(newest["kind"], json!("commit"));
    assert!(
        newest["touched"]
            .as_array()
            .is_some_and(|keys| !keys.is_empty())
    );

    let first = ok(
        &mut engine,
        "history_list",
        json!({ "doc": doc, "limit": 2 }),
    );
    assert_eq!(txs(&first), [5, 4]);
    let cursor = first["next_cursor"].as_str().expect("cursor").to_owned();

    // A change between pages does not shift the next page.
    create_glyph(&mut engine, &agent(), &doc, "F");
    let second = ok(
        &mut engine,
        "history_list",
        json!({ "doc": doc, "limit": 2, "cursor": cursor }),
    );
    assert_eq!(second["rev"], json!(6));
    assert_eq!(txs(&second), [3, 2]);
    let cursor = second["next_cursor"].as_str().expect("cursor").to_owned();
    let third = ok(
        &mut engine,
        "history_list",
        json!({ "doc": doc, "limit": 2, "cursor": cursor }),
    );
    assert_eq!(txs(&third), [1]);
    assert_eq!(third["next_cursor"], Value::Null);

    // An exact last page has no next cursor.
    let page = ok(
        &mut engine,
        "history_list",
        json!({ "doc": doc, "limit": 6 }),
    );
    assert_eq!(txs(&page), [6, 5, 4, 3, 2, 1]);
    assert_eq!(page["next_cursor"], Value::Null);

    for args in [
        json!({ "doc": doc, "limit": 0 }),
        json!({ "doc": doc, "limit": 201 }),
        json!({ "doc": doc, "cursor": "not a cursor" }),
        json!({ "doc": doc, "extra": 1 }),
    ] {
        let error = err(&mut engine, "history_list", args.clone());
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{args}");
    }
    let limit = ok(
        &mut engine,
        "history_list",
        json!({ "doc": doc, "limit": 200 }),
    );
    assert_eq!(txs(&limit).len(), 6);
    let error = err(&mut engine, "history_list", json!({ "doc": "doc_nope" }));
    assert_eq!(error.code, ErrorCode::NotFound);

    // An empty history.
    let fresh = new_doc(&mut engine);
    let empty = ok(&mut engine, "history_list", json!({ "doc": fresh }));
    assert_eq!(
        empty,
        json!({ "rev": 0, "entries": [], "next_cursor": null })
    );
}

// ------------------------------------------------------------------- extras

#[test]
fn undo_and_redo_report_the_new_revision() {
    let mut engine = engine();
    let doc = new_doc(&mut engine);
    let error = err(&mut engine, "history_undo", json!({ "doc": doc }));
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(error.message, "nothing to undo");
    let error = err(&mut engine, "history_redo", json!({ "doc": doc }));
    assert_eq!(error.message, "nothing to redo");

    create_glyph(&mut engine, &agent(), &doc, "A");
    let undone = ok(&mut engine, "history_undo", json!({ "doc": doc }));
    assert_eq!(undone["rev"], json!(2));
    assert_eq!(undone["tx"], json!(2));
    assert_eq!(undone["touched"].as_array().expect("touched").len(), 2);
    assert!(glyph_names(&mut engine, &doc).is_empty());

    let error = err_as(
        &mut engine,
        &Actor::User,
        "history_redo",
        json!({ "doc": doc }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    let redone = ok(&mut engine, "history_redo", json!({ "doc": doc }));
    assert_eq!(redone["rev"], json!(3));
    assert_eq!(glyph_names(&mut engine, &doc), ["A"]);
    assert!(
        inspect(&mut engine, &doc)["dirty"]
            .as_bool()
            .expect("dirty")
    );

    let error = err(
        &mut engine,
        "history_undo",
        json!({ "doc": doc, "base_rev": 3 }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    let error = err(&mut engine, "history_undo", json!({ "doc": "doc_nope" }));
    assert_eq!(error.code, ErrorCode::NotFound);
}

#[test]
fn revert_of_mass_change_requires_plan() {
    let mut engine = engine();
    let doc = new_doc(&mut engine);
    approved(&mut engine, "edit_batch", create_many(&doc, 0, 25)); // tx 1
    let items: Vec<Value> = (0..25)
        .map(|index| json!({ "glyph": format!("n{index:02}"), "advance_width": 99 }))
        .collect();
    approved(
        &mut engine,
        "edit_batch",
        json!({ "doc": doc, "base_rev": 1, "label": "Widen many", "ops": [
            { "tool": "metrics_set", "args": { "items": items } }
        ] }),
    ); // tx 2
    let width = |engine: &mut Engine| {
        ok(engine, "glyph_get", json!({ "doc": doc, "glyph": "n24" }))["glyph"]["advance_width"]
            .clone()
    };
    assert_eq!(width(&mut engine), json!(99.0));

    // Massive, not destructive.
    let error = err(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 2, "tx": 2 }),
    );
    assert_eq!(error.code, ErrorCode::ApprovalRequired);
    let plan = &error.details["plan"];
    assert_eq!(plan["tool"], json!("history_revert"));
    assert_eq!(plan["affected_count"], json!(25));
    assert_eq!(
        plan["summary"],
        json!("Revert tx 2: Widen many (25 glyphs affected)")
    );
    assert_eq!(plan["diff"]["modified_count"], json!(25));
    assert_eq!(rev_of(&mut engine, &doc), 2);
    let committed = ok(
        &mut engine,
        "plan_commit",
        json!({ "plan_token": plan["plan_token"] }),
    );
    assert_eq!(committed["rev"], json!(3));
    assert_eq!(width(&mut engine), json!(10.0));

    // Taking back tx 1 would delete glyphs that changed since (tx 2 and 3):
    // it conflicts, and no plan is offered, not even for a dry run.
    let error = err(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 3, "tx": 1, "dry_run": true }),
    );
    assert_eq!(error.code, ErrorCode::RevisionConflict);

    // A small revert that destroys nothing applies at once.
    ok(
        &mut engine,
        "metrics_set",
        json!({ "doc": doc, "base_rev": 3, "items": [{ "glyph": "n00", "advance_width": 50 }] }),
    ); // tx 4
    let reverted = ok(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 4, "tx": 4 }),
    );
    assert_eq!(reverted["rev"], json!(5));
    assert!(reverted.get("plan_committed").is_none());

    // A base revision newer than the document, an unknown transaction.
    let error = err(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 6, "tx": 2 }),
    );
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    let error = err(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 5, "tx": 99 }),
    );
    assert_eq!(error.code, ErrorCode::NotFound);
}

#[test]
fn revert_that_destroys_requires_plan() {
    let mut engine = engine();
    let doc = new_doc(&mut engine);
    create_glyph(&mut engine, &Actor::User, &doc, "A"); // tx 1: drawn
    ok_as(
        &mut engine,
        &Actor::User,
        "glyph_create",
        json!({ "doc": doc, "base_rev": 1, "name": "E", "advance_width": 10 }),
    ); // tx 2: empty
    ok_as(
        &mut engine,
        &Actor::User,
        "glyph_set_outline",
        json!({ "doc": doc, "base_rev": 2, "glyph": "E", "svg_path": SQUARE }),
    ); // tx 3: draws the empty layer
    ok_as(
        &mut engine,
        &Actor::User,
        "metrics_set",
        json!({ "doc": doc, "base_rev": 3, "items": [{ "glyph": "A", "advance_width": 70 }] }),
    ); // tx 4: a width

    // Reverting tx 3 empties a drawn layer.
    let error = err(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 4, "tx": 3 }),
    );
    assert_eq!(error.code, ErrorCode::ApprovalRequired);
    assert_eq!(
        error.details["plan"]["summary"],
        json!("Revert tx 3: Set outline of 'E' (1 glyph affected, destructive)")
    );
    // Reverting tx 4 only restores a width.
    let reverted = ok(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 4, "tx": 4 }),
    );
    assert_eq!(reverted["rev"], json!(5));
    // Taking back tx 1 or tx 2 would delete `A` or `E`, which changed since:
    // they conflict.
    let error = err(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 5, "tx": 1 }),
    );
    assert_eq!(error.code, ErrorCode::RevisionConflict);
    let error = err(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 5, "tx": 2 }),
    );
    assert_eq!(error.code, ErrorCode::RevisionConflict, "E changed in tx 3");

    // Deleting a glyph and reverting the deletion: the revert brings it back
    // and destroys nothing.
    approved(
        &mut engine,
        "glyph_delete",
        json!({ "doc": doc, "base_rev": 5, "glyphs": ["E"] }),
    ); // tx 6
    let reverted = ok(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 6, "tx": 6 }),
    );
    assert_eq!(reverted["rev"], json!(7));
    assert_eq!(glyph_names(&mut engine, &doc), ["A", "E"]);

    // A drawn glyph created by another actor and untouched since: reverting
    // its creation deletes it.
    create_glyph(&mut engine, &Actor::User, &doc, "B"); // tx 8
    let error = err(
        &mut engine,
        "history_revert",
        json!({ "doc": doc, "base_rev": 8, "tx": 8 }),
    );
    let plan = &error.details["plan"];
    assert_eq!(error.code, ErrorCode::ApprovalRequired);
    assert_eq!(plan["diff"]["removed"], json!(["B"]));
    assert_eq!(glyph_names(&mut engine, &doc), ["A", "E", "B"]);
}

#[test]
fn history_schemas_are_strict() {
    for name in [
        "history_list",
        "history_undo",
        "history_redo",
        "history_revert",
        "history_restore",
        "document_diff",
    ] {
        let spec = forma_engine::find(name).expect("registered");
        let input = (spec.input_schema)();
        assert_eq!(input["additionalProperties"], json!(false), "{name}");
        assert!(input["properties"]["doc"].is_object(), "{name}");
        let output = spec.output_schema.expect("output schema")();
        assert!(output["properties"]["rev"].is_object(), "{name}: {output}");
    }
    let list = (forma_engine::find("history_list")
        .expect("registered")
        .input_schema)();
    assert_eq!(list["properties"]["limit"]["default"], json!(50));
    assert_eq!(list["properties"]["limit"]["maximum"], json!(200));
    for name in ["history_revert", "history_restore"] {
        let input = (forma_engine::find(name).expect("registered").input_schema)();
        assert!(input["properties"]["base_rev"].is_object(), "{name}");
    }
}
