//! The path every mutating tool takes.
//!
//! A mutating tool is a pure translation of its own arguments into
//! [`Command`]s (a crate-private `Translate` function) plus this shared
//! machinery:
//!
//! 1. `Mutation::parse` checks the raw arguments against the limits and
//!    splits them into the common `doc` and `base_rev` and the tool's own
//!    arguments (`Op`, which reject unknown fields). The same `Op` is what an
//!    `edit_batch` operation carries.
//! 2. `Steps` applies the commands, in order, to a scratch copy of the font
//!    as they are added (enforcing the node limit). Before each command it
//!    records what that command does to the state it meets, so the effects
//!    of a call (destructive or not, distinct glyphs affected) are measured
//!    step by step, the way the transaction will run.
//! 3. `mutate` asks the `approval_gate` hook and runs the commands as one
//!    transaction with [`Document::execute`], marking the document dirty.
//!    Conflicts become `REVISION_CONFLICT` with the details of
//!    [`conflict_details`](crate::errors::conflict_details).
//!
//! [`Document::execute`]: forma_edit::Document::execute

use std::borrow::Cow;
use std::sync::Arc;

use forma_edit::Command;
use forma_model::{Actor, EntityKey, Font, Rev};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::DeserializeOwned;
use serde::de::value::MapDeserializer;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::engine::{DocHandle, Engine};
use crate::errors::{ToolError, from_args_error};
use crate::registry::{Effects, ToolOutput};
use crate::tools::json_output;
use crate::tools::limits::check_raw_args;
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
const COMMON_FIELDS: [&str; 2] = ["doc", "base_rev"];

/// The arguments of a mutating tool: the document handle and the base
/// revision, plus the tool's own arguments `Op` (which are, without `doc` and
/// `base_rev`, also what an `edit_batch` operation carries).
///
/// On the wire every field is at the top level; its JSON Schema lists `doc`
/// and `base_rev` first among the required fields, like `Op` does not allow
/// other fields, and is described by [`OpArgs::DESCRIPTION`].
#[derive(Clone, Debug, PartialEq)]
pub struct Mutation<Op> {
    /// The document handle.
    pub doc: DocHandle,
    /// The latest revision the caller received.
    pub base_rev: u64,
    /// The tool's own arguments.
    pub op: Op,
}

impl<Op: DeserializeOwned> Mutation<Op> {
    /// Checks the raw arguments against the limits, then parses them.
    ///
    /// Only `doc` and `base_rev` are taken out of `args`; every other field
    /// goes to `Op`, which rejects the ones it does not know. Nothing is
    /// copied out of `args` beyond what the typed values hold.
    ///
    /// # Errors
    ///
    /// `LIMIT_EXCEEDED` (see [`check_raw_args`]) and `INVALID_ARGUMENT` for
    /// arguments that are not an object, miss `doc` or `base_rev`, or do not
    /// match `Op`.
    pub(crate) fn parse(args: &Value) -> Result<Self, ToolError> {
        check_raw_args(args)?;
        let Some(map) = args.as_object() else {
            return Err(ToolError::invalid_argument(
                "invalid arguments: expected a JSON object",
            ));
        };
        let doc = common_field::<DocHandle>(map, "doc")?;
        let base_rev = common_field::<u64>(map, "base_rev")?;
        let own = map
            .iter()
            .filter(|(key, _)| !COMMON_FIELDS.contains(&key.as_str()))
            .map(|(key, value)| (key.as_str(), value));
        let op = Op::deserialize(MapDeserializer::<_, serde_json::Error>::new(own))
            .map_err(|error| from_args_error(&error))?;
        Ok(Mutation { doc, base_rev, op })
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
        let object = schema.ensure_object();
        object.insert("description".into(), Op::DESCRIPTION.into());
        let properties = object
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert("doc".into(), doc);
            properties.insert("base_rev".into(), base_rev);
        }
        let mut required = vec![json!("doc"), json!("base_rev")];
        if let Some(Value::Array(own)) = object.remove("required") {
            required.extend(own);
        }
        object.insert("required".into(), Value::Array(required));
        schema
    }
}

/// The result of a mutating tool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MutationResult {
    /// The current revision after the call (unchanged when nothing changed).
    pub rev: u64,
    /// The transaction id of the change, or `null` when nothing changed.
    pub tx: Option<u64>,
    /// The entities the change touched: `font_info`,
    /// `vertical_metrics:<master id>`, `glyph_order` or `glyph:<glyph id>`.
    pub touched: Vec<EntityKey>,
}

/// One mutation, translated and ready to be applied.
#[derive(Clone, Debug)]
pub(crate) struct Edit<'a> {
    /// The tool that was called.
    pub tool: &'static str,
    /// The arguments of the call, exactly as received.
    #[expect(
        dead_code,
        reason = "read by the approval policy (plans), Milestone 1 Task 15"
    )]
    pub args: &'a Value,
    /// The document to change.
    pub doc: &'a DocHandle,
    /// The revision the caller based the call on.
    pub base_rev: Rev,
    /// The label of the transaction in the history.
    pub label: String,
    /// The commands, applied in order as one transaction, with the effects
    /// measured on the document's current state.
    pub steps: Steps,
}

/// What [`approval_gate`] decides on: a mutation about to be applied.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MutationRequest<'a> {
    /// Who makes the call.
    pub actor: &'a Actor,
    /// The call: tool, arguments, document, base revision, label, and the
    /// measured commands (`edit.steps`: states before and after, distinct
    /// affected glyph ids).
    pub edit: &'a Edit<'a>,
    /// The number of distinct glyphs the commands touch (`edit.steps.affected`).
    pub affected_glyphs: usize,
    /// What the call does, judged command by command on the state each one
    /// meets: [`Effects::DESTRUCTIVE`] when it deletes glyphs or replaces the
    /// contours of a non-empty layer of a glyph that existed before the
    /// call, [`Effects::WRITE`] otherwise.
    pub effects: Effects,
}

/// The approval hook of every mutation: called by [`mutate`] right before the
/// commands are applied, after the limits are checked. An error stops the
/// mutation and is returned to the caller unchanged.
///
/// Milestone 1 Task 15 fills it with the approval policy (destructive or
/// massive operations answer `APPROVAL_REQUIRED` with a plan, depending on
/// the engine's policy); until then it allows everything.
///
/// # Errors
///
/// None yet.
pub(crate) fn approval_gate(
    _engine: &mut Engine,
    request: &MutationRequest<'_>,
) -> Result<(), ToolError> {
    let edit = request.edit;
    tracing::debug!(
        actor = %request.actor,
        tool = edit.tool,
        doc = %edit.doc,
        base_rev = edit.base_rev.0,
        label = %edit.label,
        commands = edit.steps.cmds.len(),
        affected_glyphs = request.affected_glyphs,
        destructive = request.effects.destructive,
        "approval gate: allowed (no approval policy yet)"
    );
    Ok(())
}

/// Applies a mutation and answers the call: answers a call without commands
/// without changing anything (`tx: null`), asks [`approval_gate`], runs the
/// commands as one transaction (marking the document dirty), and finally lets
/// `finish` turn the [`MutationResult`] into the tool's result (adding
/// tool-specific fields, such as the new glyph of `glyph_create`). [`plain`]
/// keeps it as it is.
///
/// Every mutating tool answers through this function, so a policy that
/// answers without applying (a dry run, Task 15) needs no change in the
/// tools. `edit.steps` must start from the document's current state.
///
/// # Errors
///
/// * `NOT_FOUND` for an unknown document handle;
/// * `INVALID_ARGUMENT` if `base_rev` is newer than the current revision, or
///   if the steps were not measured on the current state;
/// * the error of [`approval_gate`];
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
    if edit.steps.cmds.is_empty() {
        if edit.base_rev > rev {
            return Err(ToolError::invalid_argument(format!(
                "base revision {} is newer than the current revision {}",
                edit.base_rev.0, rev.0
            )));
        }
        let unchanged = MutationResult {
            rev: rev.0,
            tx: None,
            touched: Vec::new(),
        };
        return json_output(&finish(engine, unchanged)?);
    }

    let request = MutationRequest {
        actor,
        edit: &edit,
        affected_glyphs: edit.steps.affected.len(),
        effects: edit.steps.effects(),
    };
    approval_gate(engine, &request)?;

    let cmds = &edit.steps.cmds;
    let outcome = engine
        .doc_mut(edit.doc)?
        .change(|document| document.execute(actor, edit.base_rev, &edit.label, cmds))?;
    let applied = MutationResult {
        rev: outcome.rev.0,
        tx: Some(outcome.tx.0),
        touched: outcome.touched.into_iter().collect(),
    };
    json_output(&finish(engine, applied)?)
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
        assert!(schema["properties"]["name"].is_object());
    }
}
