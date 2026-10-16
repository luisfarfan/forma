//! The tools: one module per area, each holding the `CommandSpec`s, the
//! `…Args`/`…Result` types and the handlers (free functions) of its tools.
//!
//! The specs are listed, in order, by [`registry`](crate::registry()).

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::errors::{ToolError, from_args_error};
use crate::registry::ToolOutput;

pub mod capabilities;
pub mod document;
pub mod edit_batch;
pub mod font;
pub mod geometry;
pub mod glyphs;
pub mod history;
pub mod inputs;
pub(crate) mod limits;
pub mod metrics;
pub mod mutate;
pub mod outlines;
pub(crate) mod placeholders;
pub mod plans;
pub(crate) mod steps;

/// Deserializes the arguments of a tool; a mismatch (missing, mistyped or
/// unknown field) is `INVALID_ARGUMENT` with serde's message.
///
/// # Errors
///
/// `INVALID_ARGUMENT` if `args` does not match `T`.
pub fn parse_args<T: DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|error| from_args_error(&error))
}

/// [`parse_args`] from borrowed arguments, for tools that keep the arguments
/// as received (a plan stores them).
///
/// # Errors
///
/// `INVALID_ARGUMENT` if `args` does not match `T`.
pub(crate) fn parse_args_ref<T: DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    T::deserialize(args).map_err(|error| from_args_error(&error))
}

/// Wraps a result struct as [`ToolOutput::Json`].
///
/// # Errors
///
/// `INVALID_ARGUMENT` if `result` cannot be represented as JSON, which the
/// result types of this crate never trigger.
pub fn json_output<T: Serialize>(result: &T) -> Result<ToolOutput, ToolError> {
    serde_json::to_value(result)
        .map(ToolOutput::Json)
        .map_err(|error| ToolError::invalid_argument(format!("cannot encode the result: {error}")))
}
