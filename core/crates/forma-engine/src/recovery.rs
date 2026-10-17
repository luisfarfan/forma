//! The crash-recovery journal of open documents.
//!
//! When [`EngineConfig::journal_dir`](crate::EngineConfig::journal_dir) is
//! set, every state change of a document that has a file (a commit, undo,
//! redo, revert or restore, plans included) appends one
//! [`JournalRecord`] with the resulting state of the entities it touched to
//! the journal of the document's *file* (`<journal_dir>/<font id>-<hash of
//! the path>.journal`, see [`JournalKey`]). A save empties the journal (a save
//! to another file moves the document, and its journal, to that file); closing
//! the document removes it. After a crash, `document_open` reports the records
//! left over (`recovery_available`) and, with `recover: true`, puts them back
//! over the saved file.
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
//! A journal has a single writer: the engine that holds the lock of the
//! document's file (see `locks`). A read-only handle (the file is open for
//! writing in another process) and a read-only engine never read, extend,
//! empty or delete a journal, and are never offered its records. A document
//! opened while records of an earlier session are pending, and not
//! recovered, leaves them alone (they stay recoverable) and journals nothing
//! until it is saved.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use forma_edit::{Document, EditError, TxOutcome};
use forma_model::{Actor, EntityKey, Font};
use forma_store::{
    Journal, JournalKey, JournalRecord, StoreError, apply_records, remove_journal, replay,
};

use crate::engine::{DocHandle, Engine};
use crate::errors::{ToolError, from_store_error};
use crate::locks::read_only_handle;

/// Where an open document that owns its file stands with its journal.
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

impl Opened {
    /// A font opened without looking at any journal.
    pub(crate) fn untouched(font: Font) -> Opened {
        Opened {
            font,
            dirty: false,
            state: None,
            recovered: None,
            recovery_available: None,
            warning: None,
        }
    }
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
                "the journal of this file holds {records} unsaved changes of an earlier session \
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
        }
    }

    /// Whether closing the document removes the journal: its records are this
    /// session's.
    fn removed_on_close(&self) -> bool {
        matches!(
            self,
            JournalState::Active(_) | JournalState::Stale { ours: true, .. }
        )
    }
}

fn failed(what: &str, error: &StoreError) -> String {
    format!("the crash-recovery journal could not be {what}: {error}")
}

/// The journal record of a change that touched `touched` and left `font`.
///
/// The record holds the parts the change touched: the font info (with the
/// units per em), the masters, the glyph order, each touched glyph and, when
/// any kerning group or pair was touched, the whole kerning.
fn record_of(
    key: &JournalKey,
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
    let kerning = touched
        .iter()
        .any(|key| matches!(key, EntityKey::KerningGroup(_) | EntityKey::KerningPair(..)));
    JournalRecord {
        document: key.document_text(),
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
        kerning: kerning.then(|| font.kerning.clone()),
    }
}

impl Engine {
    /// Runs a state change on the document `handle` (see
    /// [`OpenDoc::change`](crate::OpenDoc::change)) and journals it.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND` for an unknown handle; `UNSUPPORTED` for a read-only
    /// handle; the mapped error of `change`. A journal failure is not an
    /// error: it is reported in [`Committed::journal_warning`].
    pub(crate) fn commit_change(
        &mut self,
        handle: &DocHandle,
        actor: &Actor,
        change: impl FnOnce(&mut Document) -> Result<TxOutcome, EditError>,
    ) -> Result<Committed, ToolError> {
        let open = self.doc_mut(handle)?;
        if open.read_only() {
            return Err(read_only_handle());
        }
        let outcome = open.change(change)?;
        let journal_warning = self.journal_change(handle, actor, &outcome);
        Ok(Committed {
            outcome,
            journal_warning,
        })
    }

    /// The folder and key of the journal of `handle`, if the document is
    /// journaled: there is a journal folder, the document has a file and this
    /// engine owns that file (it is not read-only).
    fn journal_of(&self, handle: &DocHandle) -> Option<(PathBuf, JournalKey)> {
        let dir = self.cfg.journal_dir.clone()?;
        let open = self.docs.get(handle)?;
        if !open.lock.owns_file() {
            return None;
        }
        let path = open.source_path.as_deref()?;
        Some((dir, JournalKey::new(open.font().id, path)))
    }

    /// The `journal_warning` for a result about `handle` when its changes
    /// are not journaled (until the next save), without touching the
    /// journal; `None` when they are, or when nothing is journaled at all.
    pub(crate) fn journal_notice(&self, handle: &DocHandle) -> Option<String> {
        self.journal_of(handle)?;
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
        let (dir, key) = self.journal_of(handle)?;
        // Not journaled: answer before building a record for nothing.
        if let Some(notice) = self.journals.get(handle).and_then(JournalState::notice) {
            return Some(notice);
        }
        let open = self.docs.get(handle)?;
        let label = open
            .document
            .last_entry()
            .map_or_else(String::new, |entry| entry.label.clone());
        let record = record_of(
            &key,
            open.font(),
            &outcome.touched,
            outcome.rev,
            actor,
            label,
        );
        let state = self
            .journals
            .entry(handle.clone())
            .or_insert(JournalState::Active(None));
        let JournalState::Active(slot) = state else {
            return state.notice();
        };
        let written = match slot {
            Some(journal) => journal.append(&record).map_err(|error| ("written", error)),
            None => Journal::open(&dir, &key)
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

    /// After a successful save of `handle`, which belonged to the file
    /// `previous` before (`None` for a document without a file): its journal
    /// is emptied, since the file now holds everything (this also retires the
    /// records of an earlier session that were not recovered). A save to
    /// another file moves the journal along: the journal of the previous file
    /// is removed when its records are this session's (they are saved in the
    /// new file), and kept when they are an earlier session's (they still
    /// belong to the previous file, which did not change). Returns why
    /// something failed, if it did.
    pub(crate) fn journal_saved(
        &mut self,
        handle: &DocHandle,
        previous: Option<&Path>,
    ) -> Option<String> {
        let (dir, key) = self.journal_of(handle)?;
        let state = self.journals.remove(handle);
        let mut warnings = Vec::new();
        let state = match previous {
            Some(previous) if previous != key.document() => {
                // `state`, and any journal it holds, is closed before the
                // previous file's journal is deleted (Windows refuses to
                // delete an open file).
                let removes = state.as_ref().is_some_and(JournalState::removed_on_close);
                drop(state);
                if removes {
                    let old = JournalKey::new(key.font_id(), previous);
                    if let Err(error) = remove_journal(&dir, &old) {
                        warnings.push(failed("removed", &error));
                    }
                }
                None
            }
            _ => state,
        };
        let journal = match state {
            Some(JournalState::Active(Some(journal))) => Ok(journal),
            _ => Journal::open(&dir, &key),
        };
        let emptied = journal.and_then(|mut journal| journal.truncate().map(|()| journal));
        let state = match emptied {
            Ok(journal) => JournalState::Active(Some(journal)),
            // Records that no longer match the saved file must never be
            // extended: journal nothing until a save manages to empty them.
            Err(error) => {
                warnings.push(format!(
                    "{}; changes are not journaled until a save empties it",
                    failed("emptied", &error)
                ));
                JournalState::Stale {
                    warning: not_journaled(&format!(
                        "the journal could not be emptied after the last save ({})",
                        failed("emptied", &error)
                    )),
                    ours: true,
                }
            }
        };
        self.journals.insert(handle.clone(), state);
        (!warnings.is_empty()).then(|| warnings.join("; "))
    }

    /// When `handle` is about to close: the journal of its session is removed
    /// (a clean document has nothing to recover; a document closed with
    /// `discard_changes` had its changes discarded on purpose), also when it
    /// stopped after a failed write. Records of an earlier session and a
    /// journal that cannot be read are kept, and a document that does not own
    /// its file (a read-only handle or engine) never removes anything.
    /// Returns why the removal failed, if it did.
    pub(crate) fn journal_closed(&mut self, handle: &DocHandle) -> Option<String> {
        let (dir, key) = self.release_journal(handle)?;
        debug_assert!(!self.holds_journal(handle));
        remove_journal(&dir, &key)
            .err()
            .map(|error| failed("removed", &error))
    }

    /// Forgets the journal state of `handle`, closing its journal file, and
    /// says which file that is if closing the document must delete it. Only
    /// a folder and a key come out, never the open [`Journal`], so the file
    /// is always closed before it is deleted (Windows refuses to delete an
    /// open file).
    fn release_journal(&mut self, handle: &DocHandle) -> Option<(PathBuf, JournalKey)> {
        // `state`, and any journal it holds, is dropped right here.
        let removes = self
            .journals
            .remove(handle)
            .is_some_and(|state| state.removed_on_close());
        let journal = self.journal_of(handle)?;
        removes.then_some(journal)
    }

    /// Whether the engine holds the journal file of `handle` open.
    fn holds_journal(&self, handle: &DocHandle) -> bool {
        matches!(
            self.journals.get(handle),
            Some(JournalState::Active(Some(_)))
        )
    }

    /// For `document_open` of the file `path`, which this engine owns (it
    /// holds its lock, or locks nothing): what the journal of the freshly
    /// loaded `font` holds and, with `recover`, the font with its records
    /// applied.
    ///
    /// # Errors
    ///
    /// With `recover` only: `INVALID_ARGUMENT` if the journal is damaged,
    /// belongs to another file or does not apply to the file; `IO_ERROR` if
    /// it cannot be read.
    pub(crate) fn journal_on_open(
        &self,
        font: Font,
        path: &Path,
        recover: bool,
    ) -> Result<Opened, ToolError> {
        let mut opened = Opened {
            recovered: recover.then_some(0),
            ..Opened::untouched(font)
        };
        let Some(dir) = self.cfg.journal_dir.as_deref() else {
            return Ok(opened);
        };
        let key = JournalKey::new(opened.font.id, path);
        let records = match replay(dir, &key) {
            Ok(records) => records,
            Err(error) if recover => return Err(from_store_error(error)),
            Err(error) => {
                opened.warning = Some(format!(
                    "{}; changes are not journaled until the document is saved",
                    failed("read", &error)
                ));
                opened.state = Some(JournalState::Stale {
                    warning: not_journaled(&format!(
                        "the journal of this file cannot be read ({error})"
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
        let open = engine.doc(&doc).expect("open");
        let key = JournalKey::new(open.font().id, open.source_path.as_deref().expect("a file"));
        let file = journals.join(key.file_name());
        assert!(file.is_file());
        assert!(
            engine.holds_journal(&doc),
            "the journal is open while journaling"
        );

        // What comes out of the release is where the file is, and the engine
        // no longer holds it open: the file can be deleted (on Windows too).
        let (dir, released) = engine.release_journal(&doc).expect("removed on close");
        assert_eq!((dir.as_path(), &released), (journals.as_path(), &key));
        assert!(!engine.holds_journal(&doc));
        assert!(!engine.journals.contains_key(&doc));
        remove_journal(&dir, &released).expect("remove");
        assert!(!file.exists());

        // A journal this session stopped is removed too; one of an earlier
        // session, or one the engine knows nothing about, is only released.
        let stopped = JournalState::Stale {
            warning: String::new(),
            ours: true,
        };
        engine.journals.insert(doc.clone(), stopped);
        assert!(engine.release_journal(&doc).is_some());
        engine
            .journals
            .insert(doc.clone(), JournalState::earlier_session(3));
        assert!(engine.release_journal(&doc).is_none());
        assert!(!engine.journals.contains_key(&doc));
        assert!(engine.release_journal(&doc).is_none());
        // A read-only handle never removes a journal, whatever its state.
        let lock = std::mem::replace(
            &mut engine.doc_mut(&doc).expect("open").lock,
            crate::locks::FileLock::HeldElsewhere,
        );
        engine
            .journals
            .insert(doc.clone(), JournalState::Active(None));
        assert!(engine.release_journal(&doc).is_none());
        engine.doc_mut(&doc).expect("open").lock = lock;

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

    #[test]
    fn records_carry_the_kerning_when_a_kerning_entity_is_touched() {
        use forma_model::{GlyphId, KernRef, KernSide, KerningGroup, KerningGroupId, Rev};

        let mut font = Font::new("Kern", 1000);
        let group = KerningGroupId::new();
        font.kerning.groups.insert(
            group,
            KerningGroup {
                id: group,
                name: "O".into(),
                side: KernSide::First,
                members: Vec::new(),
            },
        );
        let key = JournalKey::new(font.id, Path::new("/fonts/kern.forma"));
        let master = font.masters[0].id;
        let pair = EntityKey::KerningPair(
            master,
            KernRef::Group(group),
            KernRef::Glyph(GlyphId::new()),
        );
        let record = |touched: &[EntityKey]| {
            let touched: BTreeSet<EntityKey> = touched.iter().copied().collect();
            record_of(&key, &font, &touched, Rev(1), &Actor::User, "x".into())
        };

        for touched in [
            vec![pair],
            vec![EntityKey::KerningGroup(group)],
            vec![EntityKey::FontInfo, pair],
        ] {
            assert_eq!(record(&touched).kerning.as_ref(), Some(&font.kerning));
        }
        let glyph = record(&[EntityKey::Glyph(GlyphId::new()), EntityKey::GlyphOrder]);
        assert_eq!(glyph.kerning, None);
        assert_eq!(glyph.glyphs.len(), 1);
    }
}
