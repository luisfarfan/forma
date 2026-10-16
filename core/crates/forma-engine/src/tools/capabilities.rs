//! `forma_capabilities`: what this server supports.

use forma_model::Actor;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::{Engine, Policy};
use crate::errors::ToolError;
use crate::limits::Limits;
use crate::registry::{CommandSpec, Effects, OutputKind, ToolOutput, Toolset, schema_of};
use crate::tools::{json_output, parse_args};

/// The version of the tool API (tool names, arguments, results, errors).
pub const API_VERSION: &str = "1.0.0";

/// The native document formats, by extension.
pub const FORMATS: [&str; 2] = ["forma", "formapackage"];

/// `forma_capabilities`.
pub(crate) const SPEC: CommandSpec = CommandSpec {
    name: "forma_capabilities",
    title: "Get capabilities",
    description: "Report what this FORMA server supports: the API version, the server version, \
                  the enabled toolsets, the per-call limits, the native document formats, the \
                  approval policy and whether the server is read-only. Call it first to plan \
                  your work.",
    toolset: Toolset::Core,
    effects: Effects::READ,
    output: OutputKind::Json,
    input_schema: schema_of::<CapabilitiesArgs>,
    output_schema: Some(schema_of::<CapabilitiesResult>),
    handler: forma_capabilities,
};

/// Arguments of `forma_capabilities`: none.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesArgs {}

/// Result of `forma_capabilities`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilitiesResult {
    /// The version of the tool API.
    pub api_version: String,
    /// The version of this server.
    pub server_version: String,
    /// The enabled toolsets.
    pub toolsets: Vec<Toolset>,
    /// The per-call limits and approval thresholds.
    pub limits: Limits,
    /// The native document formats, by extension.
    pub formats: Vec<String>,
    /// The approval policy.
    pub policy: Policy,
    /// Whether every tool that is not read-only is refused.
    pub read_only: bool,
}

fn forma_capabilities(
    engine: &mut Engine,
    _actor: &Actor,
    args: Value,
) -> Result<ToolOutput, ToolError> {
    let CapabilitiesArgs {} = parse_args(args)?;
    json_output(&CapabilitiesResult {
        api_version: API_VERSION.to_owned(),
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        toolsets: Toolset::ALL.to_vec(),
        limits: Limits::CURRENT,
        formats: FORMATS.iter().map(|format| (*format).to_owned()).collect(),
        policy: engine.config().policy,
        read_only: engine.config().read_only,
    })
}
