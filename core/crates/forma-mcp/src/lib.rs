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
//! * **Both protocol eras.** The `initialize` handshake (2025-06-18 and
//!   2025-11-25, what Codex speaks) and the 2026-07-28 `server/discover`
//!   lifecycle, where every request carries its own `_meta`.
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

pub use server::FormaServer;

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
    let running = match FormaServer::new(cfg).serve(transport).await {
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
