//! Per-call limits and approval thresholds, reported by
//! `forma_capabilities` and enforced by the tools.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The most operations one `edit_batch` call may carry.
pub const MAX_BATCH_OPS: usize = 2000;
/// The most outline nodes one call may carry in total.
pub const MAX_NODES: usize = 100_000;
/// The largest text argument (such as an SVG path), in bytes: 4 MiB.
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
/// An operation affecting more glyphs than this is massive and needs an
/// approved plan.
pub const MASS_THRESHOLD: usize = 20;
/// How long an approval plan token stays valid, in seconds.
pub const PLAN_TTL_SECS: u64 = 300;

/// The limits as reported by `forma_capabilities`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Limits {
    /// The most operations one `edit_batch` call may carry.
    pub max_batch_ops: usize,
    /// The most outline nodes one call may carry in total.
    pub max_nodes: usize,
    /// The largest text argument, in bytes.
    pub max_text_bytes: usize,
    /// Operations affecting more glyphs than this need an approved plan.
    pub mass_threshold: usize,
    /// Lifetime of a plan token, in seconds.
    pub plan_ttl_secs: u64,
}

impl Limits {
    /// The limits this engine enforces.
    pub const CURRENT: Limits = Limits {
        max_batch_ops: MAX_BATCH_OPS,
        max_nodes: MAX_NODES,
        max_text_bytes: MAX_TEXT_BYTES,
        mass_threshold: MASS_THRESHOLD,
        plan_ttl_secs: PLAN_TTL_SECS,
    };
}
