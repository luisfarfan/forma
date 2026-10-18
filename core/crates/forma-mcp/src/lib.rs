//! The FORMA MCP server: a thin facade, generated from the `forma-engine`
//! command registry, that lets any MCP client (Claude Code, Codex, ...)
//! drive every editor operation.
//!
//! It holds **no business logic**. Each [registry](forma_engine::registry)
//! command the engine enables becomes a tool, and every tool call and
//! resource read goes through [`Engine::call`](forma_engine::Engine::call),
//! which validates arguments and enforces revisions, approvals, toolsets,
//! read-only mode and the path sandbox.
//!
//! * **Both protocol eras.** The `initialize` handshake (every revision from
//!   2024-11-05 to 2025-11-25; Codex speaks 2025-06-18) and the 2026-07-28
//!   `server/discover` lifecycle, where every request carries its own
//!   `_meta`.
//! * **Actors.** Every call is made as `agent:<client name>`, from the
//!   handshake's `clientInfo` or the request's `_meta` `clientInfo`,
//!   sanitized (control characters removed, trimmed, at most 64 characters;
//!   `agent:unknown` when nothing is left).
//! * **Results.** JSON results carry `structuredContent` and the same JSON as
//!   text; previews carry the PNG and a JSON summary as text, without
//!   `structuredContent`; errors are `isError` results with
//!   `{"code", "message", "details"}` as text.
//! * **Resources.** `forma://docs`, `forma://doc/{doc}/glyph/{name}` and
//!   `forma://doc/{doc}/glyph/{name}/preview.png`, read through
//!   `document_list`, `glyph_get` and `preview_glyph`.
//! * **No elicitation, sampling, Tasks or MCP Apps**: approvals use the plan
//!   tools (`plan_commit` asks clients for user interaction).
//!
//! Nothing in this crate prints: on stdio, stdout carries only JSON-RPC, and
//! diagnostics go to whatever `tracing` subscriber the binary installs (on
//! stderr).

#![forbid(unsafe_code)]

mod convert;
mod resources;
mod server;

use forma_engine::EngineConfig;
use rmcp::service::{QuitReason, ServerInitializeError};
use rmcp::transport::IntoTransport;
use rmcp::{RoleServer, ServiceExt};

pub use server::{FormaServer, ServerOptions};

/// The size of the tool list a client receives, in characters of compact
/// JSON, by part. Tokens are only estimated (characters / 4): what a client
/// actually spends depends on how it loads tools; measure it per client.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CatalogSize {
    /// How many tools are listed.
    pub tools: usize,
    /// The whole list.
    pub chars_total: usize,
    /// The argument schemas.
    pub chars_input_schemas: usize,
    /// The result schemas.
    pub chars_output_schemas: usize,
    /// The descriptions.
    pub chars_descriptions: usize,
    /// A rough estimate of the tokens of the whole list.
    pub est_tokens: usize,
}

/// The size of the tool list a server with `cfg` and `options` sends.
#[must_use]
pub fn catalog_size(cfg: EngineConfig, options: ServerOptions) -> CatalogSize {
    let engine = forma_engine::Engine::new(cfg);
    let tools = server::tools_of(&engine, options);
    let len = |value: &serde_json::Value| serde_json::to_string(value).map_or(0, |t| t.len());
    let mut size = CatalogSize {
        tools: tools.len(),
        chars_total: 0,
        chars_input_schemas: 0,
        chars_output_schemas: 0,
        chars_descriptions: 0,
        est_tokens: 0,
    };
    for tool in &tools {
        let value = serde_json::to_value(tool).unwrap_or_default();
        size.chars_total += len(&value);
        size.chars_input_schemas += len(&value["inputSchema"]);
        if !value["outputSchema"].is_null() {
            size.chars_output_schemas += len(&value["outputSchema"]);
        }
        size.chars_descriptions += value["description"].as_str().map_or(0, str::len);
    }
    size.est_tokens = size.chars_total / 4;
    size
}

/// Serves MCP over `transport` with a new engine configured by `cfg`, until
/// the client goes away.
///
/// # Errors
///
/// A failed start (other than the client leaving before it began) or a
/// transport failure. The client closing the connection, before or after the
/// lifecycle started, is a clean end.
pub async fn serve<T, E, A>(cfg: EngineConfig, transport: T) -> anyhow::Result<()>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    serve_with(cfg, ServerOptions::default(), transport).await
}

/// [`serve`] with presentation options for the tool list.
///
/// # Errors
///
/// As [`serve`].
pub async fn serve_with<T, E, A>(
    cfg: EngineConfig,
    options: ServerOptions,
    transport: T,
) -> anyhow::Result<()>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let running = match FormaServer::with_options(cfg, options)
        .serve(transport)
        .await
    {
        Ok(running) => running,
        // End of input before any request: nothing to serve.
        Err(ServerInitializeError::ConnectionClosed(_)) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    match running.waiting().await? {
        QuitReason::JoinError(error) => Err(error.into()),
        _ => Ok(()),
    }
}

/// Serves MCP over stdin and stdout with a new engine configured by `cfg`,
/// until stdin ends. Only JSON-RPC is written to stdout.
///
/// # Errors
///
/// As [`serve`].
pub async fn serve_stdio(cfg: EngineConfig) -> anyhow::Result<()> {
    serve(cfg, rmcp::transport::stdio()).await
}

/// [`serve_stdio`] with presentation options for the tool list.
///
/// # Errors
///
/// As [`serve`].
pub async fn serve_stdio_with(cfg: EngineConfig, options: ServerOptions) -> anyhow::Result<()> {
    serve_with(cfg, options, rmcp::transport::stdio()).await
}
