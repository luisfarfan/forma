//! The engine: configuration, open documents and call dispatch.

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forma_edit::{Document, EditError};
use forma_model::{Actor, Font, Rev};
use forma_store::DiskState;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::audit::AuditEntry;
use crate::errors::{ToolError, from_edit_error};
use crate::locks::FileLock;
use crate::plans::{Clock, PlanStore, SystemClock};
use crate::recovery::JournalState;
use crate::registry::{CommandSpec, ToolOutput, Toolset, find, registry};
use crate::tools::limits::check_raw_args;
use crate::tools::plans::Committing;

/// Tools that are not read-only but stay callable in read-only mode, because
/// they only change the engine's in-memory state. Their effects (and so their
/// annotations) still say they are not read-only.
const ALLOWED_IN_READ_ONLY: [&str; 1] = ["document_close"];

/// Tools that are not read-only but may be called with a read-only document
/// handle (one whose file another process holds open for writing), because
/// only some of their calls change something: they refuse those themselves
/// (closing with `discard_changes`, exporting over an existing file).
const CHECK_READ_ONLY_HANDLE_THEMSELVES: [&str; 2] = ["document_close", "export_font"];

/// How destructive or massive operations are approved.
///
/// Serialized in lowercase. Milestone 1 implements `prompted` and `auto`;
/// `strict` is accepted but its operations report `UNSUPPORTED`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    /// A plan confirmed by the client (`plan_commit`) is enough. The default.
    #[default]
    Prompted,
    /// Massive non-destructive operations apply directly; destructive ones
    /// still need a plan.
    Auto,
    /// Plans need approval in the GUI or by elicitation (Milestone 3).
    Strict,
}

impl Policy {
    /// The serialized name, such as `"prompted"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prompted => "prompted",
            Self::Auto => "auto",
            Self::Strict => "strict",
        }
    }
}

/// The configuration of an [`Engine`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EngineConfig {
    /// The directories documents may be read from and written to. Relative
    /// paths resolve against the first one; with none, every path is refused.
    pub allowed_roots: Vec<PathBuf>,
    /// The approval policy.
    pub policy: Policy,
    /// The file to append the audit log to (created if missing; its folder
    /// must exist): one JSON line per call, with the time, the actor, the
    /// tool, the document and its revision before and after, the outcome and
    /// the SHA-256 of the arguments, never the arguments themselves.
    pub audit_log: Option<PathBuf>,
    /// The folder of the crash-recovery journals (created if missing): every
    /// change of a document that has a file is journaled there until the
    /// document is saved, and `document_open` offers to recover what a crash
    /// left unsaved. A journal belongs to one document file and is only
    /// touched by the engine that holds that file's lock.
    pub journal_dir: Option<PathBuf>,
    /// The folder of the document locks (created if missing): an engine holds
    /// an exclusive OS lock on `<lock_dir>/<sha256 of the path>.lock` for
    /// every document file it has open, so that a document has a single
    /// writer; another engine with the same `lock_dir` (another `forma`
    /// process) opens it read-only. `None` locks nothing. Processes only
    /// exclude each other when they use the same folder.
    pub lock_dir: Option<PathBuf>,
    /// Refuse every tool that is not read-only (except `document_close`,
    /// which only changes the engine's in-memory state).
    pub read_only: bool,
    /// The enabled toolsets; `None` enables all of them. The tools of a
    /// disabled toolset are refused and not listed.
    pub toolsets: Option<Vec<Toolset>>,
}

/// The handle of an open document: `"doc_<32 hex digits>"`, random, valid
/// until the document is closed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocHandle(String);

impl DocHandle {
    /// A fresh random handle.
    #[must_use]
    pub fn generate() -> DocHandle {
        DocHandle(format!("doc_{}", Uuid::new_v4().simple()))
    }

    /// The handle text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DocHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DocHandle {
    fn from(text: &str) -> Self {
        DocHandle(text.to_owned())
    }
}

impl From<String> for DocHandle {
    fn from(text: String) -> Self {
        DocHandle(text)
    }
}

impl Borrow<str> for DocHandle {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl JsonSchema for DocHandle {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DocHandle".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "Document handle (`doc_...`) returned by document_create or document_open."
        })
    }
}

/// A document open in the engine.
#[derive(Clone, Debug)]
pub struct OpenDoc {
    /// The document with its revisions and history.
    pub document: Document,
    /// The canonical path the document was opened from or last saved to.
    pub source_path: Option<PathBuf>,
    /// Whether there are changes that have not been saved.
    pub dirty: bool,
    /// The lock of the document's file (see `locks`).
    pub(crate) lock: FileLock,
    /// What the document's file looked like on disk when it was opened or
    /// last saved, to tell whether another program changed it since; `None`
    /// without a file (or when it could not be read).
    pub(crate) disk: Option<DiskState>,
}

impl OpenDoc {
    /// An open document that holds no lock (see [`OpenDoc::read_only`]).
    #[must_use]
    pub fn new(document: Document, source_path: Option<PathBuf>, dirty: bool) -> OpenDoc {
        OpenDoc {
            document,
            source_path,
            dirty,
            lock: FileLock::Unlocked,
            disk: None,
        }
    }

    /// Whether the handle is read-only: another FORMA process held the file
    /// open for writing when it was opened, or the engine is read-only.
    #[must_use]
    pub fn read_only(&self) -> bool {
        self.lock.read_only()
    }

    /// Whether another FORMA process held the file open for writing when the
    /// document was opened (the handle is then read-only).
    #[must_use]
    pub fn locked_by_other_process(&self) -> bool {
        self.lock.held_elsewhere()
    }

    /// The current revision.
    #[must_use]
    pub fn rev(&self) -> Rev {
        self.document.rev()
    }

    /// The current state of the font.
    #[must_use]
    pub fn font(&self) -> &Arc<Font> {
        self.document.font()
    }

    /// Runs a state change on the document (`execute`, `undo`, `redo`,
    /// `revert`, `restore`), marks the document dirty if it succeeds and maps
    /// a failure with [`from_edit_error`]. A failed change leaves the
    /// document untouched.
    ///
    /// # Errors
    ///
    /// The mapped [`EditError`] of `change`.
    pub fn change<T>(
        &mut self,
        change: impl FnOnce(&mut Document) -> Result<T, EditError>,
    ) -> Result<T, ToolError> {
        let result = change(&mut self.document).map_err(from_edit_error)?;
        self.dirty = true;
        Ok(result)
    }

    /// The source path as text (lossy for non-UTF-8 paths), for results.
    #[must_use]
    pub fn path_text(&self) -> Option<String> {
        self.source_path.as_deref().map(path_text)
    }
}

/// One line of `document_list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocSummary {
    /// The document handle.
    pub doc: DocHandle,
    /// The file the document belongs to, if any.
    pub path: Option<String>,
    /// The family name of the font.
    pub family_name: String,
    /// The current revision.
    pub rev: u64,
    /// Whether there are unsaved changes.
    pub dirty: bool,
    /// Whether the handle is read-only (another FORMA process holds the file
    /// open for writing, or the server is read-only).
    pub read_only: bool,
}

/// The FORMA engine: open documents, the path sandbox and the dispatch of
/// tool calls through the [registry](crate::registry()).
#[derive(Debug)]
pub struct Engine {
    pub(crate) cfg: EngineConfig,
    /// The canonical allowed roots that exist, in configuration order.
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) docs: BTreeMap<DocHandle, OpenDoc>,
    /// The pending approval plans and the key that signs their tokens.
    pub(crate) plans: PlanStore,
    /// The plan `plan_commit` is running, for the one pass through the
    /// approval gate it grants; `None` otherwise.
    pub(crate) committing: Option<Committing>,
    /// The crash-recovery journal state of the open documents that have one
    /// (see `recovery`).
    pub(crate) journals: BTreeMap<DocHandle, JournalState>,
}

impl Engine {
    /// Creates an engine with no open documents.
    ///
    /// Each allowed root is canonicalized; a root that does not exist (or
    /// cannot be canonicalized) is dropped with a warning. The key that signs
    /// approval plans is drawn here, so plans never outlive the engine.
    #[must_use]
    pub fn new(cfg: EngineConfig) -> Engine {
        Engine::with_clock(cfg, Box::new(SystemClock))
    }

    /// [`Engine::new`], reading the time (for the expiry of approval plans)
    /// from `clock` instead of the system clock.
    #[must_use]
    pub fn with_clock(cfg: EngineConfig, clock: Box<dyn Clock + Send>) -> Engine {
        let roots = cfg
            .allowed_roots
            .iter()
            .filter_map(|root| match dunce::canonicalize(root) {
                Ok(canonical) => Some(canonical),
                Err(error) => {
                    tracing::warn!(
                        root = %root.display(),
                        %error,
                        "dropping an allowed root that cannot be canonicalized"
                    );
                    None
                }
            })
            .collect();
        Engine {
            cfg,
            roots,
            docs: BTreeMap::new(),
            plans: PlanStore::new(clock),
            committing: None,
            journals: BTreeMap::new(),
        }
    }

    /// The configuration the engine was created with.
    #[must_use]
    pub fn config(&self) -> &EngineConfig {
        &self.cfg
    }

    /// The canonical allowed roots in use (those that exist).
    #[must_use]
    pub fn allowed_roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Whether the tools of `toolset` are enabled.
    #[must_use]
    pub fn is_toolset_enabled(&self, toolset: Toolset) -> bool {
        self.cfg
            .toolsets
            .as_ref()
            .is_none_or(|enabled| enabled.contains(&toolset))
    }

    /// The enabled toolsets, in [`Toolset::ALL`] order.
    #[must_use]
    pub fn enabled_toolsets(&self) -> Vec<Toolset> {
        Toolset::ALL
            .into_iter()
            .filter(|toolset| self.is_toolset_enabled(*toolset))
            .collect()
    }

    /// The commands this engine accepts, in registry order: those of the
    /// enabled toolsets, minus, in read-only mode, those refused there. This
    /// is the list a facade exposes.
    #[must_use]
    pub fn enabled_specs(&self) -> Vec<&'static CommandSpec> {
        registry()
            .iter()
            .filter(|spec| self.check_available(spec).is_ok())
            .collect()
    }

    /// Refuses a command of a disabled toolset, or one that is not read-only
    /// in read-only mode (save the exemptions of [`ALLOWED_IN_READ_ONLY`]).
    pub(crate) fn check_available(&self, spec: &CommandSpec) -> Result<(), ToolError> {
        if !self.is_toolset_enabled(spec.toolset) {
            return Err(ToolError::unsupported(format!(
                "toolset '{}' is disabled",
                spec.toolset.as_str()
            )));
        }
        if self.cfg.read_only
            && !spec.effects.read_only
            && !ALLOWED_IN_READ_ONLY.contains(&spec.name)
        {
            return Err(ToolError::unsupported("server is in read-only mode"));
        }
        Ok(())
    }

    /// Runs the tool `tool` with `args` on behalf of `actor`.
    ///
    /// `args` must be a JSON object; `null` is treated as `{}`.
    ///
    /// # Errors
    ///
    /// * `INVALID_ARGUMENT` for an unknown tool, or arguments that are not an
    ///   object, are nested more than 128 levels deep or do not match the
    ///   tool's schema (including unknown fields);
    /// * `LIMIT_EXCEEDED` for a text argument (or object key) longer than
    ///   [`MAX_TEXT_BYTES`](crate::limits::MAX_TEXT_BYTES), whatever the tool,
    ///   with `details = { limit, actual, argument }`, and for the tool's own
    ///   limits (outline nodes, `edit_batch` operations);
    /// * `UNSUPPORTED` for a tool of a disabled toolset, or for a tool that is
    ///   not read-only when the engine is in read-only mode (`document_close`
    ///   excepted);
    /// * any error of the tool itself.
    ///
    /// With [`EngineConfig::audit_log`] set, the call (whatever its outcome)
    /// is recorded there; failing to write that record never fails the call.
    pub fn call(
        &mut self,
        actor: &Actor,
        tool: &str,
        args: Value,
    ) -> Result<ToolOutput, ToolError> {
        let audit = self
            .cfg
            .audit_log
            .is_some()
            .then(|| AuditEntry::begin(self, actor, tool, &args));
        let result = self.dispatch(actor, tool, args);
        if let Some(entry) = audit {
            entry.finish(self, &result);
        }
        result
    }

    /// [`Engine::call`] without the audit log.
    fn dispatch(
        &mut self,
        actor: &Actor,
        tool: &str,
        args: Value,
    ) -> Result<ToolOutput, ToolError> {
        // A commit's pass through the approval gate belongs to the call
        // `plan_commit` makes from inside its own handler, never to a call
        // that arrives here.
        self.committing = None;
        let spec = find(tool)
            .ok_or_else(|| ToolError::invalid_argument(format!("unknown tool '{tool}'")))?;
        self.check_available(spec)?;
        let args = match args {
            Value::Null => Value::Object(serde_json::Map::new()),
            Value::Object(_) => args,
            _ => {
                return Err(ToolError::invalid_argument(
                    "invalid arguments: expected a JSON object",
                ));
            }
        };
        // The text and nesting limits hold for every tool, before any
        // argument is parsed; tools that take outlines also count their nodes.
        check_raw_args(&args)?;
        self.check_handle_writable(spec, &args)?;
        (spec.handler)(self, actor, args)
    }

    /// Refuses a tool that changes the document `args.doc` when that handle
    /// is read-only (see `locks`), before the tool does anything; the tools
    /// of [`CHECK_READ_ONLY_HANDLE_THEMSELVES`] refuse their changing calls
    /// themselves.
    fn check_handle_writable(&self, spec: &CommandSpec, args: &Value) -> Result<(), ToolError> {
        if spec.effects.read_only || CHECK_READ_ONLY_HANDLE_THEMSELVES.contains(&spec.name) {
            return Ok(());
        }
        match args.get("doc").and_then(Value::as_str) {
            Some(handle) => self.ensure_writable(handle),
            None => Ok(()),
        }
    }

    /// A summary of every open document, sorted by handle.
    #[must_use]
    pub fn open_docs(&self) -> Vec<DocSummary> {
        self.docs
            .iter()
            .map(|(handle, open)| DocSummary {
                doc: handle.clone(),
                path: open.path_text(),
                family_name: open.font().info.family_name.clone(),
                rev: open.rev().0,
                dirty: open.dirty,
                read_only: open.read_only(),
            })
            .collect()
    }

    /// The open document `handle`.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND` if no open document has that handle.
    pub fn doc(&self, handle: &DocHandle) -> Result<&OpenDoc, ToolError> {
        self.docs.get(handle).ok_or_else(unknown_handle)
    }

    /// The open document `handle`, mutably.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND` if no open document has that handle.
    pub fn doc_mut(&mut self, handle: &DocHandle) -> Result<&mut OpenDoc, ToolError> {
        self.docs.get_mut(handle).ok_or_else(unknown_handle)
    }

    /// Registers a document under a fresh handle and returns the handle.
    pub(crate) fn insert_doc(&mut self, open: OpenDoc) -> DocHandle {
        let handle = DocHandle::generate();
        self.docs.insert(handle.clone(), open);
        handle
    }

    /// Closes the document `handle`, returning it. Its pending approval
    /// plans are dropped, and so is its journal handle (the journal file is
    /// the business of `document_close`).
    pub(crate) fn remove_doc(&mut self, handle: &DocHandle) -> Option<OpenDoc> {
        self.plans.remove_doc(handle);
        self.journals.remove(handle);
        self.docs.remove(handle)
    }

    /// The handle of the open document whose source is `path`, if any.
    pub(crate) fn handle_for_path(&self, path: &Path) -> Option<&DocHandle> {
        self.docs
            .iter()
            .find(|(_, open)| open.source_path.as_deref() == Some(path))
            .map(|(handle, _)| handle)
    }
}

/// The error for a document handle that is not open.
fn unknown_handle() -> ToolError {
    ToolError::not_found("unknown document handle")
}

/// A path as text for results (lossy for non-UTF-8 paths).
#[must_use]
pub(crate) fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use forma_edit::Document;
    use forma_model::{Actor, Font};

    use super::*;
    use crate::errors::ErrorCode;

    fn open_doc() -> OpenDoc {
        OpenDoc::new(Document::new(Font::new("Test", 1000)), None, false)
    }

    #[test]
    fn change_marks_dirty_only_on_success() {
        let mut open = open_doc();
        let error = open
            .change(|document| document.undo(&Actor::User))
            .expect_err("nothing to undo");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert_eq!(error.message, "nothing to undo");
        assert!(!open.dirty);

        open.change(|_| Ok(())).expect("change");
        assert!(open.dirty);
    }

    #[test]
    fn handles_are_random_and_well_formed() {
        let first = DocHandle::generate();
        let second = DocHandle::generate();
        assert_ne!(first, second);
        for handle in [first, second] {
            let text = handle.as_str();
            assert_eq!(text.len(), 36);
            assert!(text.starts_with("doc_"));
            assert!(
                text[4..]
                    .chars()
                    .all(|c| matches!(c, '0'..='9' | 'a'..='f'))
            );
        }
    }
}
