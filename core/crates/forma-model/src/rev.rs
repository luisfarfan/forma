//! Revision numbers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A revision number.
///
/// A new or freshly opened document starts at `Rev(0)`; every change of state
/// (commit, undo, redo, revert, restore) increments it by one.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct Rev(pub u64);
