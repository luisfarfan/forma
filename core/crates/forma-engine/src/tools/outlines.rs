//! Outline edits: `glyph_set_outline`, `contour_edit` and `nodes_edit`.

use forma_edit::{Command, ContourAction, GlyphRef, NodeAction};
use forma_model::{Actor, Font, MasterId, PathId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::engine::Engine;
use crate::errors::ToolError;
use crate::registry::{CommandSpec, Effects, OutputKind, ToolOutput, Toolset, schema_of};
use crate::tools::inputs::{CoordinateSystemArg, PathInput, glyph_label, outline_paths};
use crate::tools::mutate::{Mutation, MutationResult, OpArgs, Translation, run_edit};
use crate::tools::plans::OrPlan;

/// `glyph_set_outline`.
pub(crate) const SET_OUTLINE: CommandSpec = CommandSpec {
    name: "glyph_set_outline",
    title: "Set glyph outline",
    description: concat!(
        "Replace every contour of one glyph layer of `doc` (`glyph` is a name or id; the \
         default master when `master` is omitted) with a new outline, given either as \
         `svg_path` (SVG path data) or as `paths` (structured contours, such as those of \
         glyph_get), not both; `paths: []` clears the layer. `coordinate_system` works as in \
         glyph_create. Destructive when the layer is not empty, and then may require approval \
         depending on the server policy (APPROVAL_REQUIRED with a plan to confirm).",
        base_rev_note!()
    ),
    toolset: Toolset::Glyphs,
    effects: Effects::DESTRUCTIVE,
    output: OutputKind::Json,
    input_schema: schema_of::<GlyphSetOutlineArgs>,
    output_schema: Some(schema_of::<OrPlan<MutationResult>>),
    handler: glyph_set_outline,
};

/// `contour_edit`.
pub(crate) const CONTOUR_EDIT: CommandSpec = CommandSpec {
    name: "contour_edit",
    title: "Edit contours",
    description: concat!(
        "Add, delete, reverse or transform contours of one glyph layer of `doc` (the default \
         master when `master` is omitted). `action` is {\"type\": \"add\", \"svg_path\" | \
         \"path\", \"coordinate_system\"?} (an SVG path with several subpaths adds one contour \
         each), {\"type\": \"delete\", \"path_id\"}, {\"type\": \"reverse\", \"path_id\"} or \
         {\"type\": \"transform\", \"path_id\", \"affine\": [a, b, c, d, e, f]}, which maps (x, \
         y) to (a*x + c*y + e, b*x + d*y + f). Contour ids come from glyph_get. Deleting the \
         last contour of a drawn layer, or a transform that collapses the contour (a*d - b*c \
         = 0), is destructive and may require approval depending on the server policy.",
        base_rev_note!()
    ),
    toolset: Toolset::Glyphs,
    effects: Effects::DESTRUCTIVE,
    output: OutputKind::Json,
    input_schema: schema_of::<ContourEditArgs>,
    output_schema: Some(schema_of::<OrPlan<MutationResult>>),
    handler: contour_edit,
};

/// `nodes_edit`.
pub(crate) const NODES_EDIT: CommandSpec = CommandSpec {
    name: "nodes_edit",
    title: "Edit nodes",
    description: concat!(
        "Edit the nodes of one contour (`path_id`) of a glyph of `doc` (the default master \
         when `master` is omitted). `action` is {\"type\": \"move\", \"indices\", \"dx\", \
         \"dy\"} (handles do not follow: list them too), {\"type\": \"insert\", \
         \"segment_index\", \"t\"} (split a segment, keeping the shape), {\"type\": \
         \"delete\", \"index\"} (an on-curve node and its handles) or {\"type\": \
         \"set_smooth\", \"index\", \"smooth\"}. Indices are positions in the contour's \
         `nodes` from glyph_get.",
        base_rev_note!()
    ),
    toolset: Toolset::Glyphs,
    effects: Effects::WRITE,
    output: OutputKind::Json,
    input_schema: schema_of::<NodesEditArgs>,
    output_schema: Some(schema_of::<OrPlan<MutationResult>>),
    handler: nodes_edit,
};

/// Arguments of `glyph_set_outline`.
pub type GlyphSetOutlineArgs = Mutation<GlyphSetOutlineOpArgs>;

/// Arguments of `glyph_set_outline` (inside `edit_batch`, without `doc` and
/// `base_rev`). Give the outline as `svg_path` or as `paths`, not both.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GlyphSetOutlineOpArgs {
    /// The glyph, by name or id.
    pub glyph: GlyphRef,
    /// The master; the default master when omitted.
    #[serde(default)]
    pub master: Option<MasterId>,
    /// The new outline as SVG path data; every subpath is a contour.
    #[serde(default)]
    pub svg_path: Option<String>,
    /// The new outline as structured contours; `[]` clears the layer.
    #[serde(default)]
    pub paths: Option<Vec<PathInput>>,
    /// The coordinate system of the outline.
    #[serde(default)]
    pub coordinate_system: CoordinateSystemArg,
}

impl OpArgs for GlyphSetOutlineOpArgs {
    const DESCRIPTION: &'static str = "Arguments of `glyph_set_outline`: the document, the base revision, the glyph, the master (the default master when omitted) and the new outline, as `svg_path` or as `paths`, not both.";
}

/// Arguments of `contour_edit`.
pub type ContourEditArgs = Mutation<ContourEditOpArgs>;

/// Arguments of `contour_edit` (inside `edit_batch`, without `doc` and
/// `base_rev`).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContourEditOpArgs {
    /// The glyph, by name or id.
    pub glyph: GlyphRef,
    /// The master; the default master when omitted.
    #[serde(default)]
    pub master: Option<MasterId>,
    /// What to do.
    pub action: ContourActionArg,
}

impl OpArgs for ContourEditOpArgs {
    const DESCRIPTION: &'static str = "Arguments of `contour_edit`: the document, the base revision, the glyph, the master (the default master when omitted) and the action.";
}

/// An edit of the contours of a glyph layer.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "ContourAction")]
pub enum ContourActionArg {
    /// Appends contours, given as `svg_path` (one contour per subpath) or as
    /// one structured `path`, not both. New contours get the ids after the
    /// highest existing one.
    Add {
        /// The contours as SVG path data.
        #[serde(default)]
        svg_path: Option<String>,
        /// One structured contour.
        #[serde(default)]
        path: Option<PathInput>,
        /// The coordinate system of the new contours.
        #[serde(default)]
        coordinate_system: CoordinateSystemArg,
    },
    /// Removes a contour.
    Delete {
        /// The contour to remove.
        path_id: PathId,
    },
    /// Reverses the direction of a contour, keeping its shape.
    Reverse {
        /// The contour to reverse.
        path_id: PathId,
    },
    /// Applies an affine transform `[a, b, c, d, e, f]` to every node of a
    /// contour: (x, y) becomes (a*x + c*y + e, b*x + d*y + f). A mirroring
    /// transform also flips the direction, which is not corrected.
    Transform {
        /// The contour to transform.
        path_id: PathId,
        /// The affine coefficients `[a, b, c, d, e, f]`.
        affine: [f64; 6],
    },
}

/// Arguments of `nodes_edit`.
pub type NodesEditArgs = Mutation<NodesEditOpArgs>;

/// Arguments of `nodes_edit` (inside `edit_batch`, without `doc` and
/// `base_rev`).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodesEditOpArgs {
    /// The glyph, by name or id.
    pub glyph: GlyphRef,
    /// The master; the default master when omitted.
    #[serde(default)]
    pub master: Option<MasterId>,
    /// The contour to edit (an id from glyph_get).
    pub path_id: PathId,
    /// What to do.
    pub action: NodeActionArg,
}

impl OpArgs for NodesEditOpArgs {
    const DESCRIPTION: &'static str = "Arguments of `nodes_edit`: the document, the base revision, the glyph, the master (the default master when omitted), the contour and the action.";
}

/// An edit of the nodes of one contour. Nodes are addressed by their index
/// in the contour's `nodes` (from glyph_get).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "NodeAction")]
pub enum NodeActionArg {
    /// Moves exactly the listed nodes by `(dx, dy)`. Handles do not follow
    /// their on-curve node: list them too to move them.
    Move {
        /// Indices of the nodes to move; each must exist.
        indices: Vec<usize>,
        /// Horizontal offset in font units.
        dx: f64,
        /// Vertical offset in font units.
        dy: f64,
    },
    /// Splits segment `segment_index` at `t` with a new on-curve node,
    /// keeping the shape. Segment `k` ends at the `k`-th on-curve node of a
    /// closed contour; in an open contour it starts there.
    Insert {
        /// The segment to split.
        segment_index: usize,
        /// Where to split, strictly between 0 and 1.
        t: f64,
    },
    /// Deletes an on-curve node and its handles; the segments around it
    /// merge into one.
    Delete {
        /// Index of the on-curve node to delete.
        index: usize,
    },
    /// Sets whether an on-curve node is smooth; handles are not realigned.
    SetSmooth {
        /// Index of the on-curve node.
        index: usize,
        /// The new value of the flag.
        smooth: bool,
    },
}

impl From<NodeActionArg> for NodeAction {
    fn from(action: NodeActionArg) -> Self {
        match action {
            NodeActionArg::Move { indices, dx, dy } => NodeAction::Move { indices, dx, dy },
            NodeActionArg::Insert { segment_index, t } => NodeAction::Insert { segment_index, t },
            NodeActionArg::Delete { index } => NodeAction::Delete { index },
            NodeActionArg::SetSmooth { index, smooth } => NodeAction::SetSmooth { index, smooth },
        }
    }
}

/// The command of `glyph_set_outline`.
///
/// # Errors
///
/// `INVALID_ARGUMENT` when neither or both outline forms are given, and the
/// other errors of [`outline_paths`].
pub(crate) fn glyph_set_outline_commands(
    font: &Font,
    op: GlyphSetOutlineOpArgs,
) -> Result<Translation, ToolError> {
    let paths =
        outline_paths(font, op.svg_path, op.paths, op.coordinate_system)?.ok_or_else(|| {
            ToolError::invalid_argument("give the new outline as `svg_path` or as `paths`")
        })?;
    let label = format!("Set outline of '{}'", glyph_label(font, &op.glyph));
    let cmd = Command::SetOutline {
        glyph: op.glyph,
        master: op.master,
        paths,
    };
    Ok((label, vec![cmd]))
}

/// The commands of `contour_edit`: one per added contour, or one for the
/// other actions.
///
/// # Errors
///
/// `INVALID_ARGUMENT` for an `add` without exactly one outline form or with
/// no contour, and the other errors of [`outline_paths`].
pub(crate) fn contour_edit_commands(
    font: &Font,
    op: ContourEditOpArgs,
) -> Result<Translation, ToolError> {
    let name = glyph_label(font, &op.glyph);
    let command = |action: ContourAction| Command::ContourEdit {
        glyph: op.glyph.clone(),
        master: op.master,
        action,
    };
    let (label, cmds) = match op.action {
        ContourActionArg::Add {
            svg_path,
            path,
            coordinate_system,
        } => {
            let paths = outline_paths(
                font,
                svg_path,
                path.map(|path| vec![path]),
                coordinate_system,
            )?
            .ok_or_else(|| {
                ToolError::invalid_argument("give the contour to add as `svg_path` or `path`")
            })?;
            if paths.is_empty() {
                return Err(ToolError::invalid_argument(
                    "nothing to add: the SVG path data has no contour",
                ));
            }
            let label = match paths.len() {
                1 => format!("Add a contour to '{name}'"),
                count => format!("Add {count} contours to '{name}'"),
            };
            let cmds = paths
                .into_iter()
                .map(|path| command(ContourAction::Add { path }))
                .collect();
            (label, cmds)
        }
        ContourActionArg::Delete { path_id } => (
            format!("Delete contour {} of '{name}'", path_id.0),
            vec![command(ContourAction::Delete { path_id })],
        ),
        ContourActionArg::Reverse { path_id } => (
            format!("Reverse contour {} of '{name}'", path_id.0),
            vec![command(ContourAction::Reverse { path_id })],
        ),
        ContourActionArg::Transform { path_id, affine } => (
            format!("Transform contour {} of '{name}'", path_id.0),
            vec![command(ContourAction::Transform { path_id, affine })],
        ),
    };
    Ok((label, cmds))
}

/// The command of `nodes_edit`.
///
/// # Errors
///
/// None: the action is checked when the command applies.
pub(crate) fn nodes_edit_commands(
    font: &Font,
    op: NodesEditOpArgs,
) -> Result<Translation, ToolError> {
    let label = format!(
        "Edit nodes of contour {} of '{}'",
        op.path_id.0,
        glyph_label(font, &op.glyph)
    );
    let cmd = Command::NodesEdit {
        glyph: op.glyph,
        master: op.master,
        path_id: op.path_id,
        action: op.action.into(),
    };
    Ok((label, vec![cmd]))
}

fn glyph_set_outline(
    engine: &mut Engine,
    actor: &Actor,
    args: Value,
) -> Result<ToolOutput, ToolError> {
    run_edit(
        engine,
        actor,
        SET_OUTLINE.name,
        &args,
        glyph_set_outline_commands,
    )
}

fn contour_edit(engine: &mut Engine, actor: &Actor, args: Value) -> Result<ToolOutput, ToolError> {
    run_edit(
        engine,
        actor,
        CONTOUR_EDIT.name,
        &args,
        contour_edit_commands,
    )
}

fn nodes_edit(engine: &mut Engine, actor: &Actor, args: Value) -> Result<ToolOutput, ToolError> {
    run_edit(engine, actor, NODES_EDIT.name, &args, nodes_edit_commands)
}
