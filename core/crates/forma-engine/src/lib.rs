//! The FORMA engine: the command registry and the host of open documents.
//!
//! Every editor operation is declared once, as a [`CommandSpec`] in
//! [`registry`]: name, title, description, toolset, effects, JSON Schemas of
//! its arguments and result, and a handler. The MCP server, the CLI, the UI
//! and the in-app agent are generated from the registry and call
//! [`Engine::call`]; none of them holds business logic.
//!
//! The [`Engine`] owns the open documents (addressed by opaque
//! [`DocHandle`]s), enforces read-only mode and the path sandbox (only paths
//! inside the allowed roots, see [`Engine::check_allowed`]), and maps every
//! failure onto a [`ToolError`] with one of the stable [`ErrorCode`]s.

#![forbid(unsafe_code)]

/// The sentence every mutating tool that takes `base_rev` appends to its
/// description.
macro_rules! base_rev_note {
    () => {
        " Pass the latest `rev` you received as `base_rev`; if an entity this call touches \
         changed after it, the call fails with REVISION_CONFLICT: re-read and retry."
    };
}

mod engine;
pub mod errors;
pub mod limits;
pub mod paths;
pub mod plans;
mod registry;
pub mod tools;

pub use engine::{DocHandle, DocSummary, Engine, EngineConfig, OpenDoc, Policy};
pub use errors::{ErrorCode, ToolError};
pub use plans::{Clock, SystemClock};
pub use registry::{
    CommandSpec, Effects, Handler, OutputKind, ToolOutput, Toolset, any_object_schema, find,
    registry, schema_of,
};
pub use tools::capabilities::API_VERSION;
