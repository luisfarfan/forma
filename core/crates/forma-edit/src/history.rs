//! The records of a document's history.
//!
//! These types are plain data with JSON schemas: they are returned to the
//! editor, the CLI and AI agents as they are.

use std::collections::BTreeSet;
use std::fmt;

use forma_model::{Actor, EntityKey, Rev};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Identifier of a change of state of a document (a transaction).
///
/// Transaction ids start at 1 and grow by one with every change of state:
/// commits, undos, redos, reverts and restores all get a new one.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct TxId(pub u64);

impl fmt::Display for TxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// What kind of change of state a history entry records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A transaction of edit commands.
    Commit,
    /// The undo of the actor's own last undoable transaction.
    Undo,
    /// The redo of the actor's own last undone transaction.
    Redo,
    /// The entities of an earlier transaction set back to their previous
    /// values.
    Revert,
    /// The whole document set back to the state of an earlier revision.
    Restore,
}

/// One change of state of a document, attributed to the actor who made it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryEntry {
    /// The transaction id.
    pub tx: TxId,
    /// The revision the change produced.
    pub rev: Rev,
    /// Who made the change.
    pub actor: Actor,
    /// A short human-readable description of the change.
    pub label: String,
    /// The entities the change touched.
    pub touched: BTreeSet<EntityKey>,
    /// What kind of change it was.
    pub kind: EntryKind,
}

/// The result of a successful change of state of a document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TxOutcome {
    /// The transaction id of the change.
    pub tx: TxId,
    /// The new revision of the document.
    pub rev: Rev,
    /// The entities the change touched.
    pub touched: BTreeSet<EntityKey>,
}
