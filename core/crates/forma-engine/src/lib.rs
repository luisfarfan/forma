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
//! failure onto a [`ToolError`] with one of the stable [`ErrorCode`]s. When
//! configured, it also appends every call to an audit log
//! ([`EngineConfig::audit_log`]) and journals every committed change of a
//! document that has a file for crash recovery
//! ([`EngineConfig::journal_dir`]).

#![forbid(unsafe_code)]

/// The sentence every mutating tool that takes `base_rev` appends to its
/// description.
macro_rules! base_rev_note {
    () => {
        " Pass the latest `rev` you received as `base_rev`; if an entity this call touches \
         changed after it, the call fails with REVISION_CONFLICT: re-read and retry."
    };
}

mod audit;
mod engine;
pub mod errors;
pub mod limits;
mod locks;
pub mod paths;
pub mod plans;
mod recovery;
mod registry;
mod stale;
pub mod tools;

pub use engine::{DocHandle, DocSummary, Engine, EngineConfig, OpenDoc, Policy};
pub use errors::{ErrorCode, ToolError};
pub use plans::{Clock, SystemClock};
pub use registry::{
    CommandSpec, Effects, Handler, OutputKind, PROFILES, ToolOutput, Toolset, any_object_schema,
    find, profile, registry, schema_of,
};
pub use tools::capabilities::API_VERSION;
