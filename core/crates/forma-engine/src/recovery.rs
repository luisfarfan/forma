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
//! `journal_warning` instead. After a journal that could not be opened or
//! written, the document journals nothing more until it is saved (which
//! starts a new journal): the records already written stay a consistent
//! prefix of the session, and every later change result carries a
//! `journal_warning`. A record never goes after a gap, because a record holds
//! only what its change touched and replaying it without the missing ones
//! would rebuild a state that never existed, or none at all.
//!
//! A journal has a single writer. A document opened while records of an
//! earlier session are pending, and not recovered, leaves them alone (they
//! stay recoverable) and journals nothing until it is saved; a document with
//! the same font id as another open one (a copy of its file) journals
//! nothing.

use std::collections::BTreeSet;
use std::path::PathBuf;

use forma_edit::{Document, EditError, TxOutcome};
use forma_model::{Actor, EntityKey, Font, FontId};
use forma_store::{Journal, JournalRecord, StoreError, apply_records, remove_journal, replay};

use crate::engine::{DocHandle, Engine};
use crate::errors::{ToolError, from_store_error};

/// Where an open document stands with the journal of its font.
#[derive(Debug)]
pub(crate) enum JournalState {
    /// The document's session writes the journal (opened on first use).
    Active(Option<Journal>),
    /// Nothing is appended to the journal until a save starts a new one: it
    /// holds records of an earlier session that were not recovered, or it
    /// cannot be read, or a write to it failed (its records are then a
    /// prefix of this session's changes), or it could not be emptied after a
    /// save.
    Stale {
        /// The `journal_warning` of every change until the next save.
        warning: String,
        /// Whether the records are this session's, so that closing the
        /// document removes them like an active journal: after a failed open
        /// or write (a prefix of this session's changes) and after a failed
        /// truncate following a save (records the saved file already holds).
        /// Records of an earlier session and an unreadable journal are not,
        /// and are kept for a later recovery.
        ours: bool,
    },
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

/// The warning of a change that is not journaled, and why.
fn not_journaled(reason: &str) -> String {
    format!(
        "this change is not journaled for crash recovery: {reason}; save the document to start \
         a new journal"
    )
}

impl JournalState {
    /// The state of a document opened while the journal holds `records`
    /// unsaved changes of an earlier session that it did not recover.
    fn earlier_session(records: usize) -> JournalState {
        JournalState::Stale {
            warning: not_journaled(&format!(
                "the journal of this font holds {records} unsaved changes of an earlier session \
                 (see `recovery_available` of document_open), which it must not extend"
            )),
            ours: false,
        }
    }

    /// The `journal_warning` of a change made in this state, without
    /// touching the journal; `None` while the document is journaled.
    fn notice(&self) -> Option<String> {
        match self {
            JournalState::Active(_) => None,
            JournalState::Stale { warning, .. } => Some(warning.clone()),
            JournalState::Shared(owner) => Some(shared(owner)),
        }
    }
}

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

    /// The `journal_warning` for a result about `handle` when its changes
    /// are not journaled (until the next save), without touching the
    /// journal; `None` when they are, or when nothing is journaled at all.
    pub(crate) fn journal_notice(&self, handle: &DocHandle) -> Option<String> {
        self.cfg.journal_dir.as_ref()?;
        self.docs.get(handle)?.source_path.as_ref()?;
        self.journals.get(handle)?.notice()
    }

    /// Appends the record of `outcome` to the journal of `handle`, if the
    /// document is journaled; returns why it could not. A journal that
    /// cannot be opened or written makes the document stop journaling until
    /// its next save (see the module documentation).
    fn journal_change(
        &mut self,
        handle: &DocHandle,
        actor: &Actor,
        outcome: &TxOutcome,
    ) -> Option<String> {
        let dir = self.cfg.journal_dir.clone()?;
        let open = self.docs.get(handle)?;
        open.source_path.as_ref()?;
        // Not journaled: answer before building a record for nothing.
        if let Some(notice) = self.journals.get(handle).and_then(JournalState::notice) {
            return Some(notice);
        }
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
        let JournalState::Active(slot) = state else {
            return state.notice();
        };
        let written = match slot {
            Some(journal) => journal.append(&record).map_err(|error| ("written", error)),
            None => Journal::open(&dir, font_id)
                .map_err(|error| ("opened", error))
                .and_then(|journal| {
                    slot.insert(journal)
                        .append(&record)
                        .map_err(|error| ("written", error))
                }),
        };
        let (what, error) = written.err()?;
        // A later record after this gap would replay into a state that never
        // existed: stop here, the records so far being a consistent prefix.
        *state = JournalState::Stale {
            warning: not_journaled(&format!(
                "an earlier change could not be journaled ({})",
                failed(what, &error)
            )),
            ours: true,
        };
        Some(format!(
            "{}; this change and the ones after it are not journaled until the document is saved",
            failed(what, &error)
        ))
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
            JournalState::Active(None) | JournalState::Stale { .. } => Journal::open(&dir, font_id),
        };
        let emptied = journal.and_then(|mut journal| journal.truncate().map(|()| journal));
        let (state, warning) = match emptied {
            Ok(journal) => (JournalState::Active(Some(journal)), None),
            // Records that no longer match the saved file must never be
            // extended: journal nothing until a save manages to empty them.
            Err(error) => (
                JournalState::Stale {
                    warning: not_journaled(&format!(
                        "the journal could not be emptied after the last save ({})",
                        failed("emptied", &error)
                    )),
                    ours: true,
                },
                Some(format!(
                    "{}; changes are not journaled until a save empties it",
                    failed("emptied", &error)
                )),
            ),
        };
        self.journals.insert(handle.clone(), state);
        warning
    }

    /// When `handle` is about to close: the journal of its session is removed
    /// (a clean document has nothing to recover; a document closed with
    /// `discard_changes` had its changes discarded on purpose), also when it
    /// stopped after a failed write. Records of an earlier session, a journal
    /// that cannot be read, and the journal of another document, are kept.
    /// Returns why the removal failed, if it did.
    pub(crate) fn journal_closed(&mut self, handle: &DocHandle) -> Option<String> {
        let (dir, font_id) = self.release_journal(handle)?;
        debug_assert!(!self.holds_journal(handle));
        remove_journal(&dir, font_id)
            .err()
            .map(|error| failed("removed", &error))
    }

    /// Forgets the journal state of `handle`, closing its journal file, and
    /// says where that file is if closing the document must delete it. Only
    /// a folder and a font id come out, never the open [`Journal`], so the
    /// file is always closed before it is deleted (Windows refuses to delete
    /// an open file).
    fn release_journal(&mut self, handle: &DocHandle) -> Option<(PathBuf, FontId)> {
        let removes = self.journals.remove(handle).is_none_or(|state| {
            // `state`, and any journal it holds, is dropped right here.
            matches!(
                state,
                JournalState::Active(_) | JournalState::Stale { ours: true, .. }
            )
        });
        let dir = self.cfg.journal_dir.clone()?;
        let open = self.docs.get(handle)?;
        open.source_path.as_ref()?;
        removes.then(|| (dir, open.font().id))
    }

    /// Whether the engine holds the journal file of `handle` open.
    fn holds_journal(&self, handle: &DocHandle) -> bool {
        matches!(
            self.journals.get(handle),
            Some(JournalState::Active(Some(_)))
        )
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
                opened.state = Some(JournalState::Stale {
                    warning: not_journaled(&format!(
                        "the journal of this font cannot be read ({error})"
                    )),
                    ours: false,
                });
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
            opened.state = Some(JournalState::earlier_session(records.len()));
        }
        Ok(opened)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::engine::EngineConfig;
    use crate::registry::ToolOutput;

    fn json_of(output: Result<ToolOutput, ToolError>) -> Value {
        match output {
            Ok(ToolOutput::Json(value)) => value,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn closing_releases_the_journal_before_removing_it() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let docs = root.path().join("docs");
        let journals = root.path().join("journals");
        std::fs::create_dir(&docs).expect("mkdir");
        let mut engine = Engine::new(EngineConfig {
            allowed_roots: vec![docs.clone()],
            journal_dir: Some(journals.clone()),
            ..EngineConfig::default()
        });
        let actor = Actor::Agent("test".into());
        let created = json_of(engine.call(
            &actor,
            "document_create",
            json!({ "family_name": "Close", "path": docs.join("font.forma") }),
        ));
        let doc = DocHandle::from(created["doc"].as_str().expect("doc"));
        json_of(engine.call(
            &actor,
            "glyph_create",
            json!({ "doc": doc, "base_rev": 0, "name": "A", "advance_width": 500 }),
        ));
        let font_id = engine.doc(&doc).expect("open").font().id;
        let file = journals.join(format!("{font_id}.journal"));
        assert!(file.is_file());
        assert!(
            engine.holds_journal(&doc),
            "the journal is open while journaling"
        );

        // What comes out of the release is where the file is, and the engine
        // no longer holds it open: the file can be deleted (on Windows too).
        let (dir, released) = engine.release_journal(&doc).expect("removed on close");
        assert_eq!((dir.as_path(), released), (journals.as_path(), font_id));
        assert!(!engine.holds_journal(&doc));
        assert!(!engine.journals.contains_key(&doc));
        remove_journal(&dir, released).expect("remove");
        assert!(!file.exists());

        // A journal this session stopped is removed too; one of an earlier
        // session or of another document is only released.
        let stopped = JournalState::Stale {
            warning: String::new(),
            ours: true,
        };
        engine.journals.insert(doc.clone(), stopped);
        assert!(engine.release_journal(&doc).is_some());
        for kept in [
            JournalState::earlier_session(3),
            JournalState::Shared(DocHandle::from("doc_other")),
        ] {
            engine.journals.insert(doc.clone(), kept);
            assert!(engine.release_journal(&doc).is_none());
            assert!(!engine.journals.contains_key(&doc));
        }

        // Through document_close: the file is gone after a clean close.
        let saved = json_of(engine.call(
            &actor,
            "document_save",
            json!({ "doc": doc, "base_rev": 1 }),
        ));
        assert!(saved.get("journal_warning").is_none(), "{saved}");
        assert!(
            engine.holds_journal(&doc),
            "the save opened a fresh journal"
        );
        let closed = json_of(engine.call(&actor, "document_close", json!({ "doc": doc })));
        assert!(closed.get("journal_warning").is_none(), "{closed}");
        assert!(!file.exists());
    }
}
