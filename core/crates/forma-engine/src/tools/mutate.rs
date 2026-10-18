//! The path every mutating tool takes.
//!
//! A mutating tool is a pure translation of its own arguments into
//! [`Command`]s (a crate-private `Translate` function) plus this shared
//! machinery:
//!
//! 1. `Mutation::parse` checks the raw contour nodes against the node limit
//!    (text sizes are checked for every tool by `Engine::call`) and splits
//!    the arguments into the common `doc`, `base_rev` and `dry_run` and the
//!    tool's own arguments (`Op`, which reject unknown fields). The same `Op`
//!    is what an `edit_batch` operation carries (so an operation cannot be a
//!    dry run on its own).
//! 2. `Steps` applies the commands, in order, to a scratch copy of the font
//!    as they are added (enforcing the node limit). Before each command it
//!    records what that command does to the state it meets, so the effects
//!    of a call (destructive or not, distinct glyphs affected) are measured
//!    step by step, the way the transaction will run.
//! 3. `mutate` checks the touched entities for revision conflicts, asks the
//!    approval gate (`tools::plans`), which may answer instead (a dry run
//!    returns a plan, a destructive or massive call needs one), and runs the
//!    commands as one transaction with [`Document::execute`], marking the
//!    document dirty. Conflicts become `REVISION_CONFLICT` with the details of
//!    [`conflict_details`](crate::errors::conflict_details), before any plan
//!    is made.
//!
//! [`Document::execute`]: forma_edit::Document::execute

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::Arc;

use forma_edit::Command;
use forma_model::{Actor, EntityKey, Font, Rev};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::DeserializeOwned;
use serde::de::value::MapDeserializer;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::engine::{DocHandle, Engine};
use crate::errors::{ToolError, from_args_error, from_edit_error_in};
use crate::recovery::Committed;
use crate::registry::ToolOutput;
use crate::tools::json_output;
use crate::tools::limits::check_raw_nodes;
use crate::tools::plans::{Change, Gate, Proposal, approval_gate};
use crate::tools::steps::Steps;

/// A history label and the commands that carry out one tool call.
pub(crate) type Translation = (String, Vec<Command>);

/// Translates the own arguments of a mutating tool into commands, against
/// the font they will apply to (inside `edit_batch`, the state left by the
/// previous operations). Pure: it never changes anything.
pub(crate) type Translate<Op> = fn(&Font, Op) -> Result<Translation, ToolError>;

/// The own arguments of a mutating tool: every argument but `doc` and
/// `base_rev`, which is also what an `edit_batch` operation carries. Their
/// own schema (and doc comment) describes them in that role.
pub trait OpArgs: DeserializeOwned + JsonSchema {
    /// The description of the tool's complete arguments (`doc`, `base_rev`
    /// and these), for the top level of the tool's input schema.
    const DESCRIPTION: &'static str;
}

/// The fields every mutating tool takes besides its own.
const COMMON_FIELDS: [&str; 3] = ["doc", "base_rev", "dry_run"];

/// The description of `dry_run` in every schema that has it.
const DRY_RUN_DESCRIPTION: &str = "Only plan the call: change nothing and return \
     `{rev, plan}`, the plan of what the call would do, with a `plan_token` that plan_commit \
     can run once the user confirms.";

/// The arguments of a mutating tool: the document handle, the base revision
/// and `dry_run`, plus the tool's own arguments `Op` (which are, without
/// these, also what an `edit_batch` operation carries).
///
/// On the wire every field is at the top level; its JSON Schema lists `doc`
/// and `base_rev` first among the required fields (`dry_run` is optional,
/// default `false`), like `Op` does not allow other fields, and is described
/// by [`OpArgs::DESCRIPTION`].
#[derive(Clone, Debug, PartialEq)]
pub struct Mutation<Op> {
    /// The document handle.
    pub doc: DocHandle,
    /// The latest revision the caller received.
    pub base_rev: u64,
    /// Only plan the call, changing nothing.
    pub dry_run: bool,
    /// The tool's own arguments.
    pub op: Op,
}

impl<Op: DeserializeOwned> Mutation<Op> {
    /// Checks the raw structured contours against the node limit (text sizes
    /// were checked by [`Engine::call`](crate::Engine::call)), then parses
    /// the arguments.
    ///
    /// Only `doc`, `base_rev` and `dry_run` are taken out of `args`; every
    /// other field goes to `Op`, which rejects the ones it does not know.
    /// Nothing is copied out of `args` beyond what the typed values hold.
    ///
    /// # Errors
    ///
    /// `LIMIT_EXCEEDED` (see [`check_raw_nodes`]) and `INVALID_ARGUMENT` for
    /// arguments that are not an object, miss `doc` or `base_rev`, have a
    /// `dry_run` that is not a boolean, or do not match `Op`.
    pub(crate) fn parse(args: &Value) -> Result<Self, ToolError> {
        check_raw_nodes(args)?;
        let Some(map) = args.as_object() else {
            return Err(ToolError::invalid_argument(
                "invalid arguments: expected a JSON object",
            ));
        };
        let doc = common_field::<DocHandle>(map, "doc")?;
        let base_rev = common_field::<u64>(map, "base_rev")?;
        let dry_run = match map.get("dry_run") {
            Some(_) => common_field::<bool>(map, "dry_run")?,
            None => false,
        };
        let own = map
            .iter()
            .filter(|(key, _)| !COMMON_FIELDS.contains(&key.as_str()))
            .map(|(key, value)| (key.as_str(), value));
        let op = Op::deserialize(MapDeserializer::<_, serde_json::Error>::new(own))
            .map_err(|error| from_args_error(&error))?;
        Ok(Mutation {
            doc,
            base_rev,
            dry_run,
            op,
        })
    }
}

fn common_field<T: DeserializeOwned>(map: &Map<String, Value>, key: &str) -> Result<T, ToolError> {
    let value = map.get(key).ok_or_else(|| {
        ToolError::invalid_argument(format!("invalid arguments: missing field `{key}`"))
    })?;
    T::deserialize(value).map_err(|error| {
        ToolError::invalid_argument(format!("invalid arguments: `{key}`: {error}"))
    })
}

impl<Op: OpArgs> JsonSchema for Mutation<Op> {
    /// `<Tool>OpArgs` becomes `<Tool>Args`.
    fn schema_name() -> Cow<'static, str> {
        let name = Op::schema_name();
        match name.strip_suffix("OpArgs") {
            Some(stem) => format!("{stem}Args").into(),
            None => name,
        }
    }

    fn schema_id() -> Cow<'static, str> {
        format!("forma_engine::Mutation<{}>", Op::schema_id()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = Op::json_schema(generator);
        let doc = generator.subschema_for::<DocHandle>().to_value();
        let base_rev = json_schema!({
            "type": "integer",
            "format": "uint64",
            "minimum": 0,
            "description": "The latest `rev` you received for this document."
        })
        .to_value();
        let dry_run = json_schema!({
            "type": "boolean",
            "default": false,
            "description": DRY_RUN_DESCRIPTION
        })
        .to_value();
        let object = schema.ensure_object();
        object.insert("description".into(), Op::DESCRIPTION.into());
        let properties = object
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert("doc".into(), doc);
            properties.insert("base_rev".into(), base_rev);
            properties.insert("dry_run".into(), dry_run);
        }
        let mut required = vec![json!("doc"), json!("base_rev")];
        if let Some(Value::Array(own)) = object.remove("required") {
            required.extend(own);
        }
        object.insert("required".into(), Value::Array(required));
        schema
    }
}

/// The most entities a result lists in its `touched` list; its
/// `touched_count` counts them all.
pub const MAX_TOUCHED_ENTRIES: usize = 100;

/// The result of a mutating tool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MutationResult {
    /// The current revision after the call (unchanged when nothing changed).
    pub rev: u64,
    /// The transaction id of the change, or `null` when nothing changed.
    pub tx: Option<u64>,
    /// The entities the change touched, in entity order and at most 100 of
    /// them (see `touched_count`): `font_info`, `vertical_metrics:<master
    /// id>`, `glyph_order` or `glyph:<glyph id>`.
    pub touched: Vec<EntityKey>,
    /// How many entities the change touched.
    pub touched_count: usize,
    /// Whether `touched` lists only the first 100 of them.
    pub touched_truncated: bool,
    /// Present when the change applied but is not kept in the
    /// crash-recovery journal: why. The change itself is fine; save the
    /// document to make it safe (after a journal failure, no change is
    /// journaled until the next save).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_warning: Option<String>,
}

impl MutationResult {
    /// The result of a call that changed nothing, at revision `rev`.
    #[must_use]
    pub(crate) fn unchanged(rev: Rev) -> MutationResult {
        MutationResult {
            rev: rev.0,
            tx: None,
            touched: Vec::new(),
            touched_count: 0,
            touched_truncated: false,
            journal_warning: None,
        }
    }

    /// The result of a committed change.
    #[must_use]
    pub(crate) fn committed(committed: Committed) -> MutationResult {
        let Committed {
            outcome,
            journal_warning,
        } = committed;
        let touched = BoundedTouched::of(&outcome.touched);
        MutationResult {
            rev: outcome.rev.0,
            tx: Some(outcome.tx.0),
            touched: touched.touched,
            touched_count: touched.count,
            touched_truncated: touched.truncated,
            journal_warning,
        }
    }
}

/// A set of touched entities cut to its first [`MAX_TOUCHED_ENTRIES`], with
/// the full count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BoundedTouched {
    /// The first entities, in entity order.
    pub touched: Vec<EntityKey>,
    /// How many there are in all.
    pub count: usize,
    /// Whether `touched` was cut.
    pub truncated: bool,
}

impl BoundedTouched {
    /// The first [`MAX_TOUCHED_ENTRIES`] of `touched`, and their count.
    #[must_use]
    pub(crate) fn of(touched: &BTreeSet<EntityKey>) -> BoundedTouched {
        BoundedTouched {
            touched: touched.iter().take(MAX_TOUCHED_ENTRIES).copied().collect(),
            count: touched.len(),
            truncated: touched.len() > MAX_TOUCHED_ENTRIES,
        }
    }
}

/// One mutation, translated and ready to be applied.
#[derive(Clone, Debug)]
pub(crate) struct Edit<'a> {
    /// The tool that was called.
    pub tool: &'static str,
    /// The arguments of the call, exactly as received (a plan keeps them to
    /// run the call again on commit).
    pub args: &'a Value,
    /// The document to change.
    pub doc: &'a DocHandle,
    /// The revision the caller based the call on.
    pub base_rev: Rev,
    /// Only plan the call, changing nothing.
    pub dry_run: bool,
    /// The label of the transaction in the history.
    pub label: String,
    /// The commands, applied in order as one transaction, with the effects
    /// measured on the document's current state.
    pub steps: Steps,
}

/// Applies a mutation and answers the call:
///
/// 1. a call that is not a dry run and has no commands, or whose commands
///    touch no entity (such as `remove_overlap` on glyphs that do not
///    overlap), is answered without changing anything (`tx: null`);
/// 2. the entities the commands touch are checked for revision conflicts
///    ([`Document::check_conflicts`](forma_edit::Document::check_conflicts)),
///    so that no plan is offered for a call that cannot succeed;
/// 3. the approval gate decides on the call, measured by `edit.steps`
///    (destructive or not, distinct glyphs affected, the font before and
///    after); it may answer instead (a dry run returns `{rev, plan}`) or
///    refuse (`APPROVAL_REQUIRED` with a plan, `UNSUPPORTED` under the
///    `strict` policy);
/// 4. the commands run as one transaction, marking the document dirty, and
///    the change is journaled for crash recovery
///    (`Engine::commit_change`);
/// 5. `finish` turns the [`MutationResult`] into the tool's result (adding
///    tool-specific fields, such as the new glyph of `glyph_create`);
///    [`plain`] keeps it as it is.
///
/// Every mutating tool answers through this function, so the approval
/// policy and dry runs need nothing in the tools. `edit.steps` must start
/// from the document's current state.
///
/// # Errors
///
/// * `NOT_FOUND` for an unknown document handle;
/// * `INVALID_ARGUMENT` if `base_rev` is newer than the current revision, or
///   if the steps were not measured on the current state;
/// * `REVISION_CONFLICT` if an entity the commands touch changed after
///   `base_rev`;
/// * the errors of the approval gate;
/// * the mapped error of [`Document::execute`](forma_edit::Document::execute),
///   including `REVISION_CONFLICT`;
/// * the error of `finish`.
pub(crate) fn mutate<R: Serialize>(
    engine: &mut Engine,
    actor: &Actor,
    edit: Edit<'_>,
    finish: impl FnOnce(&Engine, MutationResult) -> Result<R, ToolError>,
) -> Result<ToolOutput, ToolError> {
    let open = engine.doc(edit.doc)?;
    if !Arc::ptr_eq(open.font(), &edit.steps.before) {
        // Cannot happen while the engine is borrowed mutably throughout a
        // call; refusing keeps the measured effects honest regardless.
        return Err(ToolError::invalid_argument(
            "the document changed while the call was prepared; retry",
        ));
    }
    let rev = open.rev();
    if edit.base_rev > rev {
        return Err(ToolError::invalid_argument(format!(
            "base revision {} is newer than the current revision {}",
            edit.base_rev.0, rev.0
        )));
    }
    if (edit.steps.cmds.is_empty() || edit.steps.touched.is_empty()) && !edit.dry_run {
        let mut unchanged = MutationResult::unchanged(rev);
        // Nothing to journal, but say so if this document's changes are not
        // journaled.
        unchanged.journal_warning = engine.journal_notice(edit.doc);
        return json_output(&finish(engine, unchanged)?);
    }
    open.document
        .check_conflicts(&edit.steps.touched, edit.base_rev)
        .map_err(|error| from_edit_error_in(error, &open.document))?;

    let proposal = Proposal {
        actor,
        tool: edit.tool,
        args: edit.args,
        doc: edit.doc,
        base_rev: edit.base_rev,
        dry_run: edit.dry_run,
        destructive: edit.steps.effects().destructive,
        label: edit.label.clone(),
        change: Change::Font {
            before: &edit.steps.before,
            after: &edit.steps.after,
            affected: &edit.steps.affected,
            commands: Some(&edit.steps.cmds),
        },
    };
    if let Gate::Answer(answer) = approval_gate(engine, &proposal)? {
        return Ok(answer);
    }

    let cmds = &edit.steps.cmds;
    let committed = engine.commit_change(edit.doc, actor, |document| {
        document.execute(actor, edit.base_rev, &edit.label, cmds)
    })?;
    json_output(&finish(engine, MutationResult::committed(committed))?)
}

/// The `finish` of [`mutate`] for tools whose result is the plain
/// [`MutationResult`].
///
/// # Errors
///
/// None.
pub(crate) fn plain(_engine: &Engine, result: MutationResult) -> Result<MutationResult, ToolError> {
    Ok(result)
}

/// Parses, translates and applies a mutating tool whose result is the plain
/// [`MutationResult`].
///
/// # Errors
///
/// The errors of [`Mutation::parse`], `translate` and [`mutate`].
pub(crate) fn run_edit<Op: DeserializeOwned>(
    engine: &mut Engine,
    actor: &Actor,
    tool: &'static str,
    args: &Value,
    translate: Translate<Op>,
) -> Result<ToolOutput, ToolError> {
    let call = Mutation::<Op>::parse(args)?;
    let font = Arc::clone(engine.doc(&call.doc)?.font());
    let (label, cmds) = translate(&font, call.op)?;
    let edit = Edit {
        tool,
        args,
        doc: &call.doc,
        base_rev: Rev(call.base_rev),
        dry_run: call.dry_run,
        label,
        steps: Steps::of(&font, cmds)?,
    };
    mutate(engine, actor, edit, plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct DemoOpArgs {
        /// A name.
        name: String,
        #[serde(default)]
        count: Option<u32>,
    }

    impl OpArgs for DemoOpArgs {
        const DESCRIPTION: &'static str = "Arguments of `demo`.";
    }

    #[test]
    fn parse_splits_common_fields_and_rejects_unknown_ones() {
        let parsed = Mutation::<DemoOpArgs>::parse(
            &json!({ "doc": "doc_1", "base_rev": 3, "name": "x", "count": 2 }),
        )
        .expect("parse");
        assert_eq!(parsed.doc.as_str(), "doc_1");
        assert_eq!(parsed.base_rev, 3);
        assert_eq!(parsed.op.name, "x");
        assert_eq!(parsed.op.count, Some(2));
        assert!(!parsed.dry_run);
        let planned = Mutation::<DemoOpArgs>::parse(
            &json!({ "doc": "doc_1", "base_rev": 3, "name": "x", "dry_run": true }),
        )
        .expect("parse");
        assert!(planned.dry_run);
        let error = Mutation::<DemoOpArgs>::parse(
            &json!({ "doc": "doc_1", "base_rev": 3, "name": "x", "dry_run": 1 }),
        )
        .expect_err("mistyped dry_run");
        assert!(
            error.message.starts_with("invalid arguments: `dry_run`:"),
            "{}",
            error.message
        );

        let error = Mutation::<DemoOpArgs>::parse(
            &json!({ "doc": "doc_1", "base_rev": 3, "name": "x", "other": 1 }),
        )
        .expect_err("unknown field");
        assert!(error.message.contains("other"), "{}", error.message);
        let error = Mutation::<DemoOpArgs>::parse(&json!({ "doc": "doc_1", "name": "x" }))
            .expect_err("missing base_rev");
        assert_eq!(error.message, "invalid arguments: missing field `base_rev`");
        let error = Mutation::<DemoOpArgs>::parse(&json!({ "doc": 5, "base_rev": 3, "name": "x" }))
            .expect_err("mistyped doc");
        assert!(
            error.message.starts_with("invalid arguments: `doc`:"),
            "{}",
            error.message
        );
    }

    #[test]
    fn schema_puts_common_fields_at_the_top_level() {
        let schema = crate::registry::schema_of::<Mutation<DemoOpArgs>>();
        assert_eq!(schema["title"], json!("DemoArgs"));
        assert_eq!(schema["description"], json!("Arguments of `demo`."));
        assert_eq!(schema["type"], json!("object"));
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["required"], json!(["doc", "base_rev", "name"]));
        assert_eq!(schema["properties"]["doc"]["type"], json!("string"));
        assert_eq!(schema["properties"]["base_rev"]["type"], json!("integer"));
        assert_eq!(schema["properties"]["dry_run"]["type"], json!("boolean"));
        assert_eq!(schema["properties"]["dry_run"]["default"], json!(false));
        assert!(schema["properties"]["name"].is_object());
    }
}
