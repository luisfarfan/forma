//! Font-level edits: `font_set_info` and `font_set_vertical_metrics`.

use forma_edit::Command;
use forma_model::{Actor, Font, MasterId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::engine::Engine;
use crate::errors::ToolError;
use crate::registry::{CommandSpec, Effects, OutputKind, ToolOutput, Toolset, schema_of};
use crate::tools::mutate::{Mutation, MutationResult, Translation, run_edit};

/// `font_set_info`.
pub(crate) const SET_INFO: CommandSpec = CommandSpec {
    name: "font_set_info",
    title: "Set font info",
    description: concat!(
        "Change the naming and version information of the font in `doc`: `family_name`, \
         `style_name`, `version_major`, `version_minor`, `copyright` and `designer` (an empty \
         string removes these two) and `units_per_em` (16 to 16384; only the number changes, \
         outlines are not scaled). Give at least one field.",
        base_rev_note!()
    ),
    toolset: Toolset::Core,
    effects: Effects::WRITE,
    output: OutputKind::Json,
    input_schema: schema_of::<FontSetInfoArgs>,
    output_schema: Some(schema_of::<MutationResult>),
    handler: font_set_info,
};

/// `font_set_vertical_metrics`.
pub(crate) const SET_VERTICAL_METRICS: CommandSpec = CommandSpec {
    name: "font_set_vertical_metrics",
    title: "Set vertical metrics",
    description: concat!(
        "Change the vertical metrics (`ascender`, `descender`, `x_height`, `cap_height`, in \
         font units) of one master of `doc`: `master` is a master id from document_inspect, \
         the default master when omitted. Give at least one value.",
        base_rev_note!()
    ),
    toolset: Toolset::Core,
    effects: Effects::WRITE,
    output: OutputKind::Json,
    input_schema: schema_of::<FontSetVerticalMetricsArgs>,
    output_schema: Some(schema_of::<MutationResult>),
    handler: font_set_vertical_metrics,
};

/// Arguments of `font_set_info`.
pub type FontSetInfoArgs = Mutation<FontSetInfoOpArgs>;

/// Arguments of `font_set_info` (inside `edit_batch`, without `doc` and
/// `base_rev`). Give at least one field.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FontSetInfoOpArgs {
    /// The family name; not empty.
    #[serde(default)]
    pub family_name: Option<String>,
    /// The style name, such as `Regular`; not empty.
    #[serde(default)]
    pub style_name: Option<String>,
    /// The major version number.
    #[serde(default)]
    pub version_major: Option<u16>,
    /// The minor version number.
    #[serde(default)]
    pub version_minor: Option<u16>,
    /// The copyright notice; an empty string removes it.
    #[serde(default)]
    pub copyright: Option<String>,
    /// The designer; an empty string removes it.
    #[serde(default)]
    pub designer: Option<String>,
    /// Units per em, from 16 to 16384. Outlines are not scaled.
    #[serde(default)]
    #[schemars(range(min = 16, max = 16384))]
    pub units_per_em: Option<u16>,
}

/// Arguments of `font_set_vertical_metrics`.
pub type FontSetVerticalMetricsArgs = Mutation<FontSetVerticalMetricsOpArgs>;

/// Arguments of `font_set_vertical_metrics` (inside `edit_batch`, without
/// `doc` and `base_rev`). Give at least one value.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FontSetVerticalMetricsOpArgs {
    /// The master to change (an id from document_inspect); the default
    /// master when omitted.
    #[serde(default)]
    pub master: Option<MasterId>,
    /// The ascender height.
    #[serde(default)]
    pub ascender: Option<f64>,
    /// The descender depth (negative).
    #[serde(default)]
    pub descender: Option<f64>,
    /// The height of lowercase letters such as `x`.
    #[serde(default)]
    pub x_height: Option<f64>,
    /// The height of capital letters.
    #[serde(default)]
    pub cap_height: Option<f64>,
}

/// The commands of `font_set_info`.
///
/// # Errors
///
/// None: the values are checked when the command applies.
pub(crate) fn font_set_info_commands(
    _font: &Font,
    op: FontSetInfoOpArgs,
) -> Result<Translation, ToolError> {
    let cmd = Command::SetFontInfo {
        family_name: op.family_name,
        style_name: op.style_name,
        version_major: op.version_major,
        version_minor: op.version_minor,
        copyright: op.copyright,
        designer: op.designer,
        units_per_em: op.units_per_em,
    };
    Ok(("Set font info".to_owned(), vec![cmd]))
}

/// The commands of `font_set_vertical_metrics`.
///
/// # Errors
///
/// None: the values are checked when the command applies.
pub(crate) fn font_set_vertical_metrics_commands(
    _font: &Font,
    op: FontSetVerticalMetricsOpArgs,
) -> Result<Translation, ToolError> {
    let cmd = Command::SetVerticalMetrics {
        master: op.master,
        ascender: op.ascender,
        descender: op.descender,
        x_height: op.x_height,
        cap_height: op.cap_height,
    };
    Ok(("Set vertical metrics".to_owned(), vec![cmd]))
}

fn font_set_info(engine: &mut Engine, actor: &Actor, args: Value) -> Result<ToolOutput, ToolError> {
    run_edit(engine, actor, SET_INFO.name, &args, font_set_info_commands)
}

fn font_set_vertical_metrics(
    engine: &mut Engine,
    actor: &Actor,
    args: Value,
) -> Result<ToolOutput, ToolError> {
    run_edit(
        engine,
        actor,
        SET_VERTICAL_METRICS.name,
        &args,
        font_set_vertical_metrics_commands,
    )
}
