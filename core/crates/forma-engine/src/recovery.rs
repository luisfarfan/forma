//! The crash-recovery journal of open documents.
//!
//! When [`EngineConfig::journal_dir`](crate::EngineConfig::journal_dir) is
//! set, every state change of a document that has a file (a commit, undo,
//! redo, revert or restore, plans included) appends one
//! [`JournalRecord`] with the resulting state of the entities it touched to
//! the journal of the document's font (`<journal_dir>/<font id>.journal`).
//! A save empties the journal; closing the document removes it. After a
//! crash, `document_open` reports the records left over
//! (`recovery_available`) and, with `recover: true`, puts them back over the
//! saved file.
//!
//! A journal failure never fails the change it follows: the result gains a
//! `journal_warning` instead.
//!
//! A journal has a single writer. A document opened while records of an
//! earlier session are pending, and not recovered, leaves them alone (they
//! stay recoverable) and journals nothing until it is saved; a document with
//! the same font id as another open one (a copy of its file) journals
//! nothing.

use std::collections::BTreeSet;

use forma_edit::{Document, EditError, TxOutcome};
use forma_model::{Actor, EntityKey, Font};
use forma_store::{Journal, JournalRecord, StoreError, apply_records, remove_journal, replay};

use crate::engine::{DocHandle, Engine};
use crate::errors::{ToolError, from_store_error};

/// Where an open document stands with the journal of its font.
#[derive(Debug)]
pub(crate) enum JournalState {
    /// The document's session writes the journal (opened on first use).
    Active(Option<Journal>),
    /// The journal holds records of an earlier session that were not
    /// recovered (or cannot be read): they are kept for a later recovery and
    /// nothing is appended to them until a save makes them obsolete.
    Stale,
    /// Another open document, with the same font id, owns the journal of the
    /// font: this one journals nothing.
    Shared(DocHandle),
}

/// A state change committed to a document.
#[derive(Debug)]
pub(crate) struct Committed {
    /// What the change did.
    pub outcome: TxOutcome,
    /// Why the change could not be journaled, if it could not.
    pub journal_warning: Option<String>,
}

/// A font loaded by `document_open`, with what its journal holds.
#[derive(Debug)]
pub(crate) struct Opened {
    /// The font: as loaded, or with the journal's records applied.
    pub font: Font,
    /// Whether the font differs from its file (records were recovered).
    pub dirty: bool,
    /// The journal state of the new document; `None` without a journal folder.
    pub state: Option<JournalState>,
    /// With `recover: true`, the number of records recovered.
    pub recovered: Option<usize>,
    /// Without `recover`, the number of records waiting to be recovered.
    pub recovery_available: Option<usize>,
    /// A problem with the journal that the caller should know about.
    pub warning: Option<String>,
}

const STALE: &str = "this change is not journaled for crash recovery: the journal of this font \
                     holds records this session must not extend (unsaved changes of an earlier \
                     session, see `recovery_available` of document_open, or a journal that \
                     cannot be read or emptied); save the document to start a new journal";

fn shared(owner: &DocHandle) -> String {
    format!(
        "this change is not journaled for crash recovery: the open document {owner} has the same \
         font id (it is a copy of the same font) and keeps the journal of that font"
    )
}

fn failed(what: &str, error: &StoreError) -> String {
    format!("the crash-recovery journal could not be {what}: {error}")
}

/// The journal record of a change that touched `touched` and left `font`.
fn record_of(
    font: &Font,
    touched: &BTreeSet<EntityKey>,
    rev: forma_model::Rev,
    actor: &Actor,
    label: String,
) -> JournalRecord {
    let info = touched.contains(&EntityKey::FontInfo);
    let masters = touched
        .iter()
        .any(|key| matches!(key, EntityKey::VerticalMetrics(_)));
    JournalRecord {
        rev,
        actor: actor.clone(),
        label,
        font_info: info.then(|| font.info.clone()),
        units_per_em: info.then_some(font.units_per_em),
        masters: masters.then(|| font.masters.clone()),
        glyph_order: touched
            .contains(&EntityKey::GlyphOrder)
            .then(|| font.glyph_order.clone()),
        glyphs: touched
            .iter()
            .filter_map(|key| match key {
                EntityKey::Glyph(id) => {
                    Some((*id, font.glyphs.get(id).map(|glyph| (**glyph).clone())))
                }
                _ => None,
            })
            .collect(),
    }
}

impl Engine {
    /// Runs a state change on the document `handle` (see
    /// [`OpenDoc::change`](crate::OpenDoc::change)) and journals it.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND` for an unknown handle; the mapped error of `change`. A
    /// journal failure is not an error: it is reported in
    /// [`Committed::journal_warning`].
    pub(crate) fn commit_change(
        &mut self,
        handle: &DocHandle,
        actor: &Actor,
        change: impl FnOnce(&mut Document) -> Result<TxOutcome, EditError>,
    ) -> Result<Committed, ToolError> {
        let outcome = self.doc_mut(handle)?.change(change)?;
        let journal_warning = self.journal_change(handle, actor, &outcome);
        Ok(Committed {
            outcome,
            journal_warning,
        })
    }

    /// Appends the record of `outcome` to the journal of `handle`, if the
    /// document is journaled; returns why it could not.
    fn journal_change(
        &mut self,
        handle: &DocHandle,
        actor: &Actor,
        outcome: &TxOutcome,
    ) -> Option<String> {
        let dir = self.cfg.journal_dir.clone()?;
        let open = self.docs.get(handle)?;
        open.source_path.as_ref()?;
        let font = open.font();
        let label = open
            .document
            .last_entry()
            .map_or_else(String::new, |entry| entry.label.clone());
        let record = record_of(font, &outcome.touched, outcome.rev, actor, label);
        let font_id = font.id;
        let state = self
            .journals
            .entry(handle.clone())
            .or_insert(JournalState::Active(None));
        match state {
            JournalState::Active(slot) => {
                if slot.is_none() {
                    match Journal::open(&dir, font_id) {
                        Ok(journal) => *slot = Some(journal),
                        Err(error) => return Some(failed("opened", &error)),
                    }
                }
                slot.as_mut()?
                    .append(&record)
                    .err()
                    .map(|error| failed("written", &error))
            }
            JournalState::Stale => Some(STALE.to_owned()),
            JournalState::Shared(owner) => Some(shared(owner)),
        }
    }

    /// After a successful save of `handle`: its journal is emptied, since the
    /// file now holds everything (this also retires the records of an earlier
    /// session that were not recovered). Returns why that failed, if it did.
    pub(crate) fn journal_saved(&mut self, handle: &DocHandle) -> Option<String> {
        let dir = self.cfg.journal_dir.clone()?;
        let font_id = self.docs.get(handle)?.font().id;
        let state = self
            .journals
            .remove(handle)
            .unwrap_or(JournalState::Active(None));
        let journal = match state {
            shared @ JournalState::Shared(_) => {
                self.journals.insert(handle.clone(), shared);
                return None;
            }
            JournalState::Active(Some(journal)) => Ok(journal),
            JournalState::Active(None) | JournalState::Stale => Journal::open(&dir, font_id),
        };
        let emptied = journal.and_then(|mut journal| journal.truncate().map(|()| journal));
        let (state, warning) = match emptied {
            Ok(journal) => (JournalState::Active(Some(journal)), None),
            // Records that no longer match the saved file must never be
            // extended: journal nothing until a save manages to empty them.
            Err(error) => (JournalState::Stale, Some(failed("emptied", &error))),
        };
        self.journals.insert(handle.clone(), state);
        warning
    }

    /// When `handle` is about to close: the journal of its session is removed
    /// (a clean document has nothing to recover; a document closed with
    /// `discard_changes` had its changes discarded on purpose). Records of an
    /// earlier session, and the journal of another document, are kept.
    /// Returns why the removal failed, if it did.
    pub(crate) fn journal_closed(&mut self, handle: &DocHandle) -> Option<String> {
        let state = self.journals.remove(handle);
        let dir = self.cfg.journal_dir.clone()?;
        let open = self.docs.get(handle)?;
        open.source_path.as_ref()?;
        match state.unwrap_or(JournalState::Active(None)) {
            JournalState::Active(journal) => {
                drop(journal);
                remove_journal(&dir, open.font().id)
                    .err()
                    .map(|error| failed("removed", &error))
            }
            JournalState::Stale | JournalState::Shared(_) => None,
        }
    }

    /// For `document_open`: what the journal of the freshly loaded `font`
    /// holds and, with `recover`, the font with its records applied.
    ///
    /// # Errors
    ///
    /// With `recover` only: `INVALID_ARGUMENT` if another open document has
    /// the same font id, or if the journal is damaged or does not apply to
    /// the file; `IO_ERROR` if it cannot be read.
    pub(crate) fn journal_on_open(&self, font: Font, recover: bool) -> Result<Opened, ToolError> {
        let mut opened = Opened {
            font,
            dirty: false,
            state: None,
            recovered: recover.then_some(0),
            recovery_available: None,
            warning: None,
        };
        let Some(dir) = self.cfg.journal_dir.as_deref() else {
            return Ok(opened);
        };
        let font_id = opened.font.id;
        if let Some((owner, _)) = self.docs.iter().find(|(_, open)| open.font().id == font_id) {
            if recover {
                return Err(ToolError::invalid_argument(format!(
                    "the open document {owner} has the same font id (it is a copy of the same \
                     font); close it before recovering this one"
                )));
            }
            opened.warning = Some(shared(owner));
            opened.state = Some(JournalState::Shared(owner.clone()));
            return Ok(opened);
        }
        let records = match replay(dir, font_id) {
            Ok(records) => records,
            Err(error) if recover => return Err(from_store_error(error)),
            Err(error) => {
                opened.warning = Some(format!(
                    "{}; changes are not journaled until the document is saved",
                    failed("read", &error)
                ));
                opened.state = Some(JournalState::Stale);
                return Ok(opened);
            }
        };
        if records.is_empty() {
            opened.state = Some(JournalState::Active(None));
        } else if recover {
            opened.font = apply_records(opened.font, &records).map_err(from_store_error)?;
            opened.dirty = true;
            opened.recovered = Some(records.len());
            // New records follow the recovered ones, which stay until the
            // next save.
            opened.state = Some(JournalState::Active(None));
        } else {
            opened.recovery_available = Some(records.len());
            opened.state = Some(JournalState::Stale);
        }
        Ok(opened)
    }
}
