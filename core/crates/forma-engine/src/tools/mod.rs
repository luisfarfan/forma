//! The tools: one module per area, each holding the `CommandSpec`s, the
//! `…Args`/`…Result` types and the handlers (free functions) of its tools.
//!
//! The specs are listed, in order, by [`registry`](crate::registry()).

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::errors::{ToolError, from_args_error};
use crate::limits::{MAX_RESULT_ENTRIES, MAX_RESULT_LIST_BYTES};
use crate::registry::ToolOutput;

pub mod capabilities;
pub mod charset;
pub mod components;
pub mod document;
pub mod edit_batch;
pub mod export;
pub mod font;
pub mod geometry;
pub mod glyphs;
pub mod history;
pub mod inputs;
pub mod kerning;
pub(crate) mod limits;
pub mod metrics;
pub mod mutate;
pub mod outlines;
pub mod pipeline;
pub mod plans;
pub mod preview;
pub mod quality;
pub mod report;
pub mod sheet;
pub(crate) mod steps;
pub mod stroke;
pub mod synth;
pub mod trace;

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

/// A list cut at [`MAX_RESULT_ENTRIES`] entries and
/// [`MAX_RESULT_LIST_BYTES`] bytes of JSON, with how many entries there were.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bounded<T> {
    /// The first entries.
    pub items: Vec<T>,
    /// How many entries there were in all.
    pub total: usize,
    /// Whether `items` lost some.
    pub truncated: bool,
}

impl<T: Serialize> Bounded<T> {
    /// The longest start of `items` with at most [`MAX_RESULT_ENTRIES`]
    /// entries whose compact JSON (commas included) fits in
    /// [`MAX_RESULT_LIST_BYTES`]; the first entry is always kept.
    pub(crate) fn of(items: Vec<T>) -> Bounded<T> {
        Bounded::within(items, (MAX_RESULT_ENTRIES, MAX_RESULT_LIST_BYTES))
    }

    /// [`Bounded::of`] with another budget: at most `max_entries` entries
    /// whose JSON fits in `max_bytes` (see
    /// [`Detail::list_budget`](report::Detail::list_budget)).
    pub(crate) fn within(
        mut items: Vec<T>,
        (max_entries, max_bytes): (usize, usize),
    ) -> Bounded<T> {
        let total = items.len();
        let mut bytes = 0;
        let mut kept = 0;
        for item in items.iter().take(max_entries) {
            let size = serde_json::to_vec(item).map_or(0, |json| json.len()) + 1;
            if kept > 0 && bytes + size > max_bytes {
                break;
            }
            bytes += size;
            kept += 1;
        }
        items.truncate(kept);
        Bounded {
            items,
            total,
            truncated: kept < total,
        }
    }

    /// The note of a cut list: `"showing the first <n> of <total> <what>;
    /// <hint>"`; `None` when nothing was cut.
    pub(crate) fn note(&self, what: &str, hint: &str) -> Option<String> {
        self.truncated.then(|| {
            format!(
                "showing the first {} of {} {what}; {hint}",
                self.items.len(),
                self.total
            )
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_lists_keep_a_counted_start() {
        let short = Bounded::of(vec![1, 2, 3]);
        assert_eq!(
            (short.items.len(), short.total, short.truncated),
            (3, 3, false)
        );
        assert_eq!(short.note("numbers", "ask again"), None);

        let many = Bounded::of((0..500).collect::<Vec<u32>>());
        assert_eq!(many.items.len(), MAX_RESULT_ENTRIES);
        assert_eq!((many.total, many.truncated), (500, true));
        assert_eq!(
            many.note("numbers", "ask again").as_deref(),
            Some("showing the first 200 of 500 numbers; ask again")
        );

        // Large entries stop the list before the byte budget is passed.
        let big = "x".repeat(1000);
        let large = Bounded::of(vec![big.clone(); 100]);
        assert_eq!(large.items.len(), MAX_RESULT_LIST_BYTES / (big.len() + 3));
        assert!(large.truncated);
        // One entry larger than the whole budget is still listed.
        let huge = Bounded::of(vec!["y".repeat(MAX_RESULT_LIST_BYTES * 2)]);
        assert_eq!((huge.items.len(), huge.truncated), (1, false));
    }
}
