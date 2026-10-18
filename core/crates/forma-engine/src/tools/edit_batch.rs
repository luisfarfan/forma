//! `edit_batch`: several mutating operations as one transaction.
//!
//! Each operation names a mutating tool and carries that tool's own
//! arguments (its `…OpArgs`, without `doc` and `base_rev`), translated by the
//! same function the tool itself uses. The operations are first applied one
//! by one to a scratch copy of the font with [`forma_edit::apply`], each
//! translated against the state the previous ones left, so that a failure
//! names its operation (`details.op_index`) and the effects of the batch are
//! measured command by command; then every command is committed at once with
//! a single `Document::execute`, so the whole batch is one undoable
//! transaction and one new revision, or nothing at all.

use std::sync::Arc;

use forma_edit::Command;
use forma_model::{Actor, Font, Rev};
use schemars::JsonSchema;
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::engine::Engine;
use crate::errors::{ToolError, from_args_error};
use crate::registry::{CommandSpec, Effects, OutputKind, ToolOutput, Toolset, schema_of};
use crate::tools::limits::check_batch_ops;
use crate::tools::mutate::{
    Edit, Mutation, MutationResult, OpArgs, Translate, Translation, mutate, plain,
};
use crate::tools::plans::OrPlan;
use crate::tools::steps::Steps;
use crate::tools::{components, font, geometry, glyphs, kerning, metrics, outlines, stroke};

/// `edit_batch`.
pub(crate) const SPEC: CommandSpec = CommandSpec {
    name: "edit_batch",
    title: "Edit in batch",
    description: concat!(
        "Apply several operations to `doc` atomically, as one undoable transaction with one \
         new `rev`. Each item of `ops` is {\"tool\", \"args\"}: `tool` is font_set_info, font_set_vertical_metrics, \
         glyph_create, glyph_update, glyph_delete, glyph_set_outline, contour_edit, \
         nodes_edit, metrics_set, geometry_fix, component_set, glyph_compose, glyph_decompose, \
         glyph_stroke, remove_overlap or kerning_set, and `args` are that tool's arguments without \
         `doc` and `base_rev`. Operations run in order, each on the previous result. A \
         failing one is named by `details.op_index`. At most 2000 operations. \
         A batch that destroys data (as glyph_delete does) or touches more than 20 entities \
         (glyphs, kerning pairs or groups) may require approval depending on the server policy \
         (APPROVAL_REQUIRED with a plan; with `dry_run: true` the call only returns the plan).",
        base_rev_note!()
    ),
    toolset: Toolset::Core,
    effects: Effects::DESTRUCTIVE,
    output: OutputKind::Json,
    input_schema: schema_of::<EditBatchArgs>,
    output_schema: Some(schema_of::<OrPlan<MutationResult>>),
    handler: edit_batch,
};

/// Arguments of `edit_batch`.
pub type EditBatchArgs = Mutation<EditBatchOpArgs>;

/// Arguments of `edit_batch`, besides `doc` and `base_rev`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditBatchOpArgs {
    /// The label of the transaction in the history; "Batch (N operations)"
    /// when omitted.
    #[serde(default)]
    pub label: Option<String>,
    /// The operations, applied in order: from 1 to 2000.
    #[schemars(with = "Vec<BatchOp>", length(min = 1, max = 2000))]
    pub ops: Vec<IgnoredAny>,
}

impl OpArgs for EditBatchOpArgs {
    const DESCRIPTION: &'static str = "Arguments of `edit_batch`: the document, the base revision, an optional history label and the operations, applied in order as one transaction.";
}

/// One operation of a batch.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchOp {
    /// The mutating tool to run.
    pub tool: BatchTool,
    /// The arguments of `tool`, without `doc` and `base_rev`.
    #[serde(default)]
    #[schemars(with = "Map<String, Value>")]
    pub args: IgnoredAny,
}

/// The tools an `edit_batch` operation can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BatchTool {
    /// `font_set_info`.
    FontSetInfo,
    /// `font_set_vertical_metrics`.
    FontSetVerticalMetrics,
    /// `glyph_create`.
    GlyphCreate,
    /// `glyph_update`.
    GlyphUpdate,
    /// `glyph_delete`.
    GlyphDelete,
    /// `glyph_set_outline`.
    GlyphSetOutline,
    /// `contour_edit`.
    ContourEdit,
    /// `nodes_edit`.
    NodesEdit,
    /// `metrics_set`.
    MetricsSet,
    /// `geometry_fix`.
    GeometryFix,
    /// `component_set`.
    ComponentSet,
    /// `glyph_compose`.
    GlyphCompose,
    /// `glyph_decompose`.
    GlyphDecompose,
    /// `glyph_stroke`.
    GlyphStroke,
    /// `remove_overlap`.
    RemoveOverlap,
    /// `kerning_set`.
    KerningSet,
}

/// Parses `args` into a tool's own arguments and translates them.
fn translate_with<Op: DeserializeOwned>(
    font: &Font,
    args: &Value,
    translate: Translate<Op>,
) -> Result<Translation, ToolError> {
    let op = Op::deserialize(args).map_err(|error| from_args_error(&error))?;
    translate(font, op)
}

/// Translates the arguments of one operation with its tool's translation.
fn translate(tool: BatchTool, font: &Font, args: &Value) -> Result<Translation, ToolError> {
    match tool {
        BatchTool::FontSetInfo => translate_with(font, args, font::font_set_info_commands),
        BatchTool::FontSetVerticalMetrics => {
            translate_with(font, args, font::font_set_vertical_metrics_commands)
        }
        BatchTool::GlyphCreate => translate_with(font, args, glyphs::glyph_create_commands),
        BatchTool::GlyphUpdate => translate_with(font, args, glyphs::glyph_update_commands),
        BatchTool::GlyphDelete => translate_with(font, args, glyphs::glyph_delete_commands),
        BatchTool::GlyphSetOutline => {
            translate_with(font, args, outlines::glyph_set_outline_commands)
        }
        BatchTool::ContourEdit => translate_with(font, args, outlines::contour_edit_commands),
        BatchTool::NodesEdit => translate_with(font, args, outlines::nodes_edit_commands),
        BatchTool::MetricsSet => translate_with(font, args, metrics::metrics_set_commands),
        BatchTool::GeometryFix => translate_with(font, args, geometry::geometry_fix_commands),
        BatchTool::ComponentSet => translate_with(font, args, components::component_set_commands),
        BatchTool::GlyphCompose => translate_with(font, args, components::glyph_compose_commands),
        BatchTool::GlyphDecompose => {
            translate_with(font, args, components::glyph_decompose_commands)
        }
        BatchTool::GlyphStroke => translate_with(font, args, stroke::glyph_stroke_commands),
        BatchTool::RemoveOverlap => translate_with(font, args, geometry::remove_overlap_commands),
        BatchTool::KerningSet => translate_with(font, args, kerning::kerning_set_commands),
    }
}

/// The commands of one operation (`{tool, args}`), translated against
/// `font`.
fn op_commands(font: &Font, op: &Value) -> Result<Vec<Command>, ToolError> {
    if op.get("tool").and_then(Value::as_str) == Some(SPEC.name) {
        return Err(ToolError::invalid_argument(
            "edit_batch cannot contain edit_batch: list every operation in one batch",
        ));
    }
    let header = BatchOp::deserialize(op).map_err(|error| from_args_error(&error))?;
    let empty = Value::Object(Map::new());
    let args = match op.get("args") {
        None | Some(Value::Null) => &empty,
        Some(args) => args,
    };
    translate(header.tool, font, args).map(|(_, cmds)| cmds)
}

/// `error`, as the error of operation `index`: the message names the
/// operation and `details.op_index` is merged into its details.
fn at_op(index: usize, mut error: ToolError) -> ToolError {
    error.message = format!("operation {index}: {}", error.message);
    error.details = match error.details {
        Value::Object(mut details) => {
            details.insert("op_index".into(), json!(index));
            Value::Object(details)
        }
        Value::Null => json!({ "op_index": index }),
        other => json!({ "op_index": index, "details": other }),
    };
    error
}

/// Translates every operation against the state the previous ones left and
/// applies its commands to the scratch state, stopping at the first that
/// fails; the steps measure the effects of the whole batch, command by
/// command.
fn batch_steps(font: &Arc<Font>, ops: &[Value]) -> Result<Steps, ToolError> {
    let mut steps = Steps::new(font);
    for (index, op) in ops.iter().enumerate() {
        let cmds = op_commands(&steps.after, op).map_err(|error| at_op(index, error))?;
        for cmd in cmds {
            steps.push(cmd).map_err(|error| at_op(index, error))?;
        }
    }
    Ok(steps)
}

fn edit_batch(engine: &mut Engine, actor: &Actor, args: Value) -> Result<ToolOutput, ToolError> {
    // Counted before anything is parsed.
    let ops = args
        .get("ops")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    check_batch_ops(ops.len())?;
    let call = Mutation::<EditBatchOpArgs>::parse(&args)?;
    if ops.is_empty() {
        return Err(ToolError::invalid_argument("`ops` must not be empty"));
    }
    let font = Arc::clone(engine.doc(&call.doc)?.font());
    let steps = batch_steps(&font, ops)?;
    let label = call
        .op
        .label
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| format!("Batch ({} operations)", ops.len()));
    let edit = Edit {
        tool: SPEC.name,
        args: &args,
        doc: &call.doc,
        base_rev: Rev(call.base_rev),
        dry_run: call.dry_run,
        label,
        steps,
    };
    mutate(engine, actor, edit, plain)
}

#[cfg(test)]
mod tests {
    use forma_model::EntityKey;

    use crate::errors::ErrorCode;

    use super::*;

    #[test]
    fn op_errors_name_the_operation_and_keep_their_details() {
        let bare = at_op(3, ToolError::invalid_argument("bad"));
        assert_eq!(bare.message, "operation 3: bad");
        assert_eq!(bare.details, json!({ "op_index": 3 }));

        let detailed = at_op(
            1,
            ToolError::new(ErrorCode::LimitExceeded, "big")
                .with_details(json!({ "limit": 1, "actual": 2 })),
        );
        assert_eq!(
            detailed.details,
            json!({ "limit": 1, "actual": 2, "op_index": 1 })
        );

        let odd = at_op(0, ToolError::invalid_argument("x").with_details(json!([1])));
        assert_eq!(odd.details, json!({ "op_index": 0, "details": [1] }));
    }

    /// A font with the glyph `A`, drawn.
    fn drawn_font() -> Arc<Font> {
        let font = Font::new("Test", 1000);
        let create = json!({ "op": "create_glyph", "name": "A", "advance_width": 500,
            "paths": [{ "id": 0, "closed": true, "nodes": [
                { "x": 0, "y": 0, "kind": "line", "smooth": false },
                { "x": 10, "y": 0, "kind": "line", "smooth": false },
                { "x": 10, "y": 10, "kind": "line", "smooth": false }
            ] }] });
        let create: Command = serde_json::from_value(create).expect("a command");
        Arc::new(forma_edit::apply(&font, &create).expect("create").0)
    }

    #[test]
    fn batch_effects_follow_the_operations_in_order() {
        let font = drawn_font();
        let a = font.glyph_by_name("A").expect("A").id;
        // Renaming `A` and then replacing the outline of `B` replaces the
        // outline `A` had: destructive, and one glyph.
        let steps = batch_steps(
            &font,
            &[
                json!({ "tool": "glyph_update", "args": { "glyph": "A", "name": "B" } }),
                json!({ "tool": "glyph_set_outline", "args": { "glyph": "B",
                    "svg_path": "M0 0 L5 0 L5 5 Z" } }),
                json!({ "tool": "metrics_set", "args": { "items": [
                    { "glyph": "B", "advance_width": 1 },
                    { "glyph": "B", "advance_width": 2 }
                ] } }),
            ],
        )
        .expect("valid batch");
        assert_eq!(steps.effects(), Effects::DESTRUCTIVE);
        assert_eq!(
            steps.affected.iter().copied().collect::<Vec<_>>(),
            [EntityKey::Glyph(a)]
        );
        assert_eq!(steps.cmds.len(), 4);

        // A glyph the batch creates and then redraws destroys nothing.
        let steps = batch_steps(
            &font,
            &[
                json!({ "tool": "glyph_create", "args": { "name": "C", "advance_width": 1,
                    "svg_path": "M0 0 L5 0 L5 5 Z" } }),
                json!({ "tool": "glyph_set_outline", "args": { "glyph": "C",
                    "svg_path": "M0 0 L6 0 L6 6 Z" } }),
            ],
        )
        .expect("valid batch");
        assert_eq!(steps.effects(), Effects::WRITE);
        assert_eq!(steps.affected.len(), 1);
    }

    #[test]
    fn every_batch_tool_is_a_registered_mutating_tool() {
        let tools: Vec<BatchTool> = serde_json::from_value(json!([
            "font_set_info",
            "font_set_vertical_metrics",
            "glyph_create",
            "glyph_update",
            "glyph_delete",
            "glyph_set_outline",
            "contour_edit",
            "nodes_edit",
            "metrics_set",
            "geometry_fix",
            "component_set",
            "glyph_compose",
            "glyph_decompose",
            "glyph_stroke",
            "remove_overlap",
            "kerning_set"
        ]))
        .expect("tool names");
        for tool in tools {
            let name = serde_json::to_value(tool).expect("name");
            let spec = crate::registry::find(name.as_str().expect("text")).expect("registered");
            assert!(!spec.effects.read_only, "{}", spec.name);
            let schema = (spec.input_schema)();
            assert!(
                schema["properties"].get("base_rev").is_some(),
                "{}",
                spec.name
            );
        }
    }
}
