//! The command registry: every editor operation declared once, as a
//! [`CommandSpec`], in the order the facades expose them.
//!
//! The MCP server, the CLI, the UI and the in-app agent are all generated
//! from [`registry`]; none of them holds business logic of its own.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use forma_model::Actor;

use crate::engine::Engine;
use crate::errors::ToolError;
use crate::tools::{capabilities, document, placeholders};

/// A group of related tools that a facade can enable or disable together.
/// Serialized in `snake_case` (for example `"core"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Toolset {
    /// Capabilities, document lifecycle, font-level edits, batches and plans.
    Core,
    /// Glyph listing, reading and editing.
    Glyphs,
    /// Geometry checks and fixes.
    Geometry,
    /// Advance widths and side bearings.
    Spacing,
    /// Validation.
    Quality,
    /// Export to binary and interchange formats.
    Export,
    /// Rendered previews.
    Preview,
    /// History, undo/redo, revert, restore and diff.
    History,
}

impl Toolset {
    /// Every toolset, in the order the registry introduces them.
    pub const ALL: [Toolset; 8] = [
        Self::Core,
        Self::Glyphs,
        Self::Geometry,
        Self::Spacing,
        Self::Quality,
        Self::Export,
        Self::Preview,
        Self::History,
    ];

    /// The serialized name, such as `"core"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Glyphs => "glyphs",
            Self::Geometry => "geometry",
            Self::Spacing => "spacing",
            Self::Quality => "quality",
            Self::Export => "export",
            Self::Preview => "preview",
            Self::History => "history",
        }
    }
}

/// What calling a tool may do; the facades derive their annotations (MCP
/// `readOnlyHint`, `destructiveHint`, `idempotentHint`) from it. The engine
/// itself enforces read-only mode and, from Task 15, approvals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Effects {
    /// The tool never changes a document or a file.
    pub read_only: bool,
    /// The tool can remove or overwrite data; it needs an approved plan.
    pub destructive: bool,
    /// Repeating the same call has no further effect.
    pub idempotent: bool,
}

impl Effects {
    /// A read-only (and therefore idempotent) tool.
    pub const READ: Effects = Effects {
        read_only: true,
        destructive: false,
        idempotent: true,
    };
    /// A tool that makes local, undoable changes.
    pub const WRITE: Effects = Effects {
        read_only: false,
        destructive: false,
        idempotent: false,
    };
    /// A tool that removes or overwrites data.
    pub const DESTRUCTIVE: Effects = Effects {
        read_only: false,
        destructive: true,
        idempotent: false,
    };
}

/// The kind of result a tool produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OutputKind {
    /// Structured JSON ([`ToolOutput::Json`]).
    Json,
    /// An image plus a JSON summary ([`ToolOutput::Image`]); a tool of this
    /// kind may still answer with JSON for a non-image format such as SVG.
    Image,
}

/// The result of a successful tool call.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolOutput {
    /// Structured JSON.
    Json(Value),
    /// A PNG image with a short JSON summary of what it shows.
    Image {
        /// The PNG bytes.
        png: Vec<u8>,
        /// A summary such as `{ "rev": 3, "glyph": "a", "size": 512 }`.
        summary: Value,
    },
}

/// The handler of a tool: runs the call against the engine on behalf of an
/// actor, with the raw JSON arguments (never `null`; an absent argument
/// object arrives as `{}`).
pub type Handler = fn(&mut Engine, &Actor, Value) -> Result<ToolOutput, ToolError>;

/// The declaration of one editor operation.
#[derive(Clone, Copy, Debug)]
pub struct CommandSpec {
    /// The tool name: `snake_case`, at most 40 characters, stable.
    pub name: &'static str,
    /// A short human title, such as `"Create document"`.
    pub title: &'static str,
    /// What the tool does, for humans and agents (at most 1000 characters).
    pub description: &'static str,
    /// The toolset the tool belongs to.
    pub toolset: Toolset,
    /// What calling the tool may do.
    pub effects: Effects,
    /// The kind of result.
    pub output: OutputKind,
    /// The JSON Schema (2020-12) of the arguments; its root is an object.
    pub input_schema: fn() -> Value,
    /// The JSON Schema of the JSON result; `None` for image tools.
    pub output_schema: Option<fn() -> Value>,
    /// Runs the tool.
    pub handler: Handler,
}

/// Every command, in the exact registration order of the public API.
static REGISTRY: [CommandSpec; 35] = [
    // core
    capabilities::SPEC,
    document::CREATE,
    document::OPEN,
    document::SAVE,
    document::CLOSE,
    document::LIST,
    document::INSPECT,
    placeholders::FONT_SET_INFO,
    placeholders::FONT_SET_VERTICAL_METRICS,
    placeholders::EDIT_BATCH,
    placeholders::PLAN_COMMIT,
    placeholders::PLAN_DISCARD,
    // glyphs
    placeholders::GLYPH_LIST,
    placeholders::GLYPH_GET,
    placeholders::GLYPH_CREATE,
    placeholders::GLYPH_UPDATE,
    placeholders::GLYPH_DELETE,
    placeholders::GLYPH_SET_OUTLINE,
    placeholders::CONTOUR_EDIT,
    placeholders::NODES_EDIT,
    // geometry
    placeholders::GEOMETRY_CHECK,
    placeholders::GEOMETRY_FIX,
    // spacing
    placeholders::METRICS_GET,
    placeholders::METRICS_SET,
    // quality
    placeholders::VALIDATE,
    // export
    placeholders::EXPORT_FONT,
    // preview
    placeholders::PREVIEW_GLYPH,
    placeholders::PREVIEW_TEXT,
    placeholders::PREVIEW_COMPARE,
    // history
    placeholders::HISTORY_LIST,
    placeholders::HISTORY_UNDO,
    placeholders::HISTORY_REDO,
    placeholders::HISTORY_REVERT,
    placeholders::HISTORY_RESTORE,
    placeholders::DOCUMENT_DIFF,
];

/// Every command, in the exact registration order of the public API.
#[must_use]
pub fn registry() -> &'static [CommandSpec] {
    &REGISTRY
}

/// The command named `name`, if there is one.
#[must_use]
pub fn find(name: &str) -> Option<&'static CommandSpec> {
    REGISTRY.iter().find(|spec| spec.name == name)
}

/// The JSON Schema (2020-12) of `T`, without the `$schema` key.
#[must_use]
pub fn schema_of<T: JsonSchema>() -> Value {
    let mut schema = schemars::schema_for!(T).to_value();
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
    }
    schema
}

/// `{"type": "object"}`: the schema of a tool whose arguments are not
/// declared yet.
#[must_use]
pub fn any_object_schema() -> Value {
    serde_json::json!({ "type": "object" })
}
