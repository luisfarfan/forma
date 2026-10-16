//! Documents: a font with revisions, per-entity optimistic concurrency and an
//! attributed, undoable history.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::sync::Arc;

use forma_model::{Actor, EntityKey, Font, GlyphId, Rev};

use crate::diff::changed_entities;
use crate::history::{EntryKind, HistoryEntry, TxId, TxOutcome};
use crate::{Command, ConflictInfo, EditError, apply};

/// The label of a commit made with an empty label.
const DEFAULT_LABEL: &str = "Edit";

/// An undoable change, kept on the undo or the redo stack together with the
/// states before and after it.
#[derive(Clone)]
struct Step {
    actor: Actor,
    label: String,
    touched: BTreeSet<EntityKey>,
    before: Arc<Font>,
    after: Arc<Font>,
}

/// A font under edit, with its revisions and history.
///
/// # Revisions
///
/// A new document is at revision 0. Every change of state - a commit, an
/// undo, a redo, a revert or a restore - produces the next revision, gets a
/// new transaction id and is recorded in a linear history together with the
/// actor who made it. The font of every revision is kept as an [`Arc<Font>`];
/// glyphs that a change did not touch are shared between revisions.
///
/// # Optimistic concurrency
///
/// Each [`EntityKey`] remembers the revision and the actor of its last
/// change. A transaction carries the base revision its author last saw; it is
/// rejected with [`EditError::Conflict`] when any entity it touches changed
/// after that revision, so an edit never silently overwrites a change made by
/// someone else in the meantime. The glyph order never conflicts: no command
/// reorders glyphs, and creating or deleting different glyphs concurrently is
/// not a conflict.
///
/// # History
///
/// Commits, reverts and restores are undoable. An actor can only undo the
/// last undoable change of the document, and only when it made that change;
/// any other entry of the history is taken back with [`Document::revert`],
/// which restores the entities of one transaction as long as nobody changed
/// them since.
#[derive(Clone)]
pub struct Document {
    /// The current state; always the last element of `snapshots`.
    font: Arc<Font>,
    rev: Rev,
    /// The font at every revision, indexed by revision number.
    snapshots: Vec<Arc<Font>>,
    /// The revision and actor of the last change of each entity that has
    /// ever been touched.
    entity_revs: BTreeMap<EntityKey, (Rev, Actor)>,
    history: Vec<HistoryEntry>,
    undo_stack: Vec<Step>,
    redo_stack: Vec<Step>,
    next_tx: u64,
}

impl fmt::Debug for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("font_id", &self.font.id)
            .field("rev", &self.rev)
            .field("glyphs", &self.font.glyphs.len())
            .field("history", &self.history.len())
            .field("undo", &self.undo_stack.len())
            .field("redo", &self.redo_stack.len())
            .finish_non_exhaustive()
    }
}

impl Document {
    /// Creates a document at revision 0 with an empty history.
    ///
    /// The font is taken as it is: callers that load untrusted data should
    /// check it with [`Font::validate_invariants`] first. Every change made
    /// through the document keeps the invariants.
    #[must_use]
    pub fn new(font: Font) -> Document {
        let font = Arc::new(font);
        Document {
            snapshots: vec![Arc::clone(&font)],
            font,
            rev: Rev(0),
            entity_revs: BTreeMap::new(),
            history: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            next_tx: 1,
        }
    }

    /// The current revision.
    #[must_use]
    pub fn rev(&self) -> Rev {
        self.rev
    }

    /// The current state of the font.
    #[must_use]
    pub fn font(&self) -> &Arc<Font> {
        &self.font
    }

    /// The revision at which an entity last changed; `Rev(0)` for an entity
    /// that has never been touched.
    #[must_use]
    pub fn entity_rev(&self, key: &EntityKey) -> Rev {
        self.entity_revs.get(key).map_or(Rev(0), |(rev, _)| *rev)
    }

    /// Every change of state so far, oldest first.
    #[must_use]
    pub fn history(&self) -> &[HistoryEntry] {
        &self.history
    }

    /// The most recent change of state, if any.
    #[must_use]
    pub fn last_entry(&self) -> Option<&HistoryEntry> {
        self.history.last()
    }

    /// Whether [`undo`](Self::undo) would succeed for `actor`.
    #[must_use]
    pub fn can_undo(&self, actor: &Actor) -> bool {
        self.undo_stack
            .last()
            .is_some_and(|step| step.actor == *actor)
    }

    /// Whether [`redo`](Self::redo) would succeed for `actor`.
    #[must_use]
    pub fn can_redo(&self, actor: &Actor) -> bool {
        self.redo_stack
            .last()
            .is_some_and(|step| step.actor == *actor)
    }

    /// The font as it was at revision `rev`, from 0 to the current revision.
    #[must_use]
    pub fn snapshot_at(&self, rev: Rev) -> Option<Arc<Font>> {
        let index = usize::try_from(rev.0).ok()?;
        self.snapshots.get(index).cloned()
    }

    /// Applies `cmds`, in order, as one transaction by `actor`, based on the
    /// revision `base_rev` the actor last saw.
    ///
    /// Either every command applies or nothing changes. On success the
    /// document moves to the next revision, every touched entity is recorded
    /// as changed by `actor` at that revision, the change is added to the
    /// history (an empty `label` becomes `"Edit"`) and it becomes undoable;
    /// anything that was undone can no longer be redone.
    ///
    /// # Errors
    ///
    /// * [`EditError::InvalidArgument`] if `cmds` is empty or `base_rev` is
    ///   newer than the current revision.
    /// * The error of the first command that fails, as returned by
    ///   [`apply`](crate::apply).
    /// * [`EditError::Conflict`] if any touched entity other than the glyph
    ///   order changed after `base_rev`, listing every such entity in
    ///   [`EntityKey`] order with the revision and actor of its last change.
    pub fn execute(
        &mut self,
        actor: &Actor,
        base_rev: Rev,
        label: &str,
        cmds: &[Command],
    ) -> Result<TxOutcome, EditError> {
        self.check_base_rev(base_rev)?;
        let Some((first, rest)) = cmds.split_first() else {
            return Err(EditError::InvalidArgument(
                "no commands to execute: the list is empty".into(),
            ));
        };
        let (mut font, mut touched) = apply(&self.font, first)?;
        for cmd in rest {
            let (next, keys) = apply(&font, cmd)?;
            font = next;
            touched.extend(keys);
        }

        self.check_conflicts(&touched, base_rev)?;

        let label = if label.trim().is_empty() {
            DEFAULT_LABEL.to_owned()
        } else {
            label.to_owned()
        };
        Ok(self.push_undoable(actor, EntryKind::Commit, label, touched, Arc::new(font)))
    }

    /// The concurrency check of [`execute`](Self::execute), on its own: the
    /// entities in `touched`, other than the glyph order, must not have
    /// changed after `base_rev`. Callers that measure a transaction before
    /// committing it (on a scratch copy of the font) use it to report a
    /// conflict early; `execute` runs it again.
    ///
    /// # Errors
    ///
    /// * [`EditError::InvalidArgument`] if `base_rev` is newer than the
    ///   current revision.
    /// * [`EditError::Conflict`] listing every such entity, as in
    ///   [`execute`](Self::execute).
    pub fn check_conflicts(
        &self,
        touched: &BTreeSet<EntityKey>,
        base_rev: Rev,
    ) -> Result<(), EditError> {
        self.check_base_rev(base_rev)?;
        let conflicts = self.changed_since(touched, |rev| rev > base_rev);
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(self.conflict(conflicts))
        }
    }

    /// Undoes the last undoable change (a commit, revert or restore) of the
    /// document, which must have been made by `actor`.
    ///
    /// The document returns to the state before that change, as a new
    /// revision that touches the same entities. The undone change can then be
    /// redone with [`redo`](Self::redo).
    ///
    /// # Errors
    ///
    /// [`EditError::NotAllowed`] if there is nothing to undo, or if the last
    /// undoable change was made by another actor; that change can be taken
    /// back with [`revert`](Self::revert) instead.
    pub fn undo(&mut self, actor: &Actor) -> Result<TxOutcome, EditError> {
        let step = match self.undo_stack.pop() {
            Some(step) if step.actor == *actor => step,
            Some(step) => {
                let message = format!(
                    "last transaction belongs to {}; use history_revert",
                    step.actor
                );
                self.undo_stack.push(step);
                return Err(EditError::NotAllowed(message));
            }
            None => return Err(EditError::NotAllowed("nothing to undo".into())),
        };
        // Invariant: only undo and redo run between pushing a step and
        // undoing it, so the current font is the after-state of the top undo
        // step.
        debug_assert!(
            Arc::ptr_eq(&self.font, &step.after),
            "the current font is the after-state of the undone step"
        );
        let outcome = self.record(
            actor,
            EntryKind::Undo,
            format!("Undo: {}", step.label),
            step.touched.clone(),
            Arc::clone(&step.before),
        );
        self.redo_stack.push(step);
        Ok(outcome)
    }

    /// Redoes the last undone change, which must have been made by `actor`.
    ///
    /// The document returns to the state right after that change, as a new
    /// revision, and the change can be undone again.
    ///
    /// # Errors
    ///
    /// [`EditError::NotAllowed`] if there is nothing to redo (a new commit,
    /// revert or restore discards everything that was undone), or if the last
    /// undone change was made by another actor.
    pub fn redo(&mut self, actor: &Actor) -> Result<TxOutcome, EditError> {
        let step = match self.redo_stack.pop() {
            Some(step) if step.actor == *actor => step,
            Some(step) => {
                let message = format!(
                    "last undone transaction belongs to {}; only that actor can redo it",
                    step.actor
                );
                self.redo_stack.push(step);
                return Err(EditError::NotAllowed(message));
            }
            None => return Err(EditError::NotAllowed("nothing to redo".into())),
        };
        // Invariant: any commit, revert or restore clears the redo stack, so
        // the current font is the before-state of the top redo step.
        debug_assert!(
            Arc::ptr_eq(&self.font, &step.before),
            "the current font is the before-state of the redone step"
        );
        let outcome = self.record(
            actor,
            EntryKind::Redo,
            format!("Redo: {}", step.label),
            step.touched.clone(),
            Arc::clone(&step.after),
        );
        self.undo_stack.push(step);
        Ok(outcome)
    }

    /// Takes back the transaction `tx` by setting each entity it touched back
    /// to its value right before it, as a new undoable revision by `actor`.
    ///
    /// Any history entry can be reverted: reverting a commit, revert or
    /// restore takes that change back, reverting an undo re-applies what was
    /// undone, and reverting a redo takes the redone change back again. This
    /// is how an actor takes back a change it cannot [`undo`](Self::undo)
    /// because another actor made it.
    ///
    /// Glyphs the transaction created are removed and glyphs it deleted come
    /// back. The glyph order is reconciled rather than copied: glyphs that no
    /// longer exist are dropped from the current order, and restored glyphs
    /// are inserted at their former position (or at the end, if the order is
    /// now shorter), so glyphs created later by anyone are kept.
    ///
    /// # Errors
    ///
    /// * [`EditError::NotFound`] if there is no transaction `tx`.
    /// * [`EditError::InvalidArgument`] if the result would break a font
    ///   invariant (for example, a restored glyph whose name or code point has
    ///   been taken since).
    /// * [`EditError::Conflict`] if any entity of `tx` other than the glyph
    ///   order changed after `tx`.
    pub fn revert(&mut self, actor: &Actor, tx: TxId) -> Result<TxOutcome, EditError> {
        let entry = self
            .history
            .binary_search_by_key(&tx, |entry| entry.tx)
            .ok()
            .and_then(|index| self.history.get(index))
            .ok_or_else(|| EditError::NotFound(format!("transaction {tx}")))?;
        let (target_rev, touched) = (entry.rev, entry.touched.clone());
        let label = format!("Revert tx {tx}: {}", entry.label);

        let conflicts = self.changed_since(&touched, |rev| rev != target_rev);
        if !conflicts.is_empty() {
            return Err(self.conflict(conflicts));
        }

        let before = target_rev
            .0
            .checked_sub(1)
            .and_then(|rev| self.snapshot_at(Rev(rev)))
            .ok_or_else(|| EditError::NotFound(format!("the state before transaction {tx}")))?;
        let font = reverted(&self.font, &before, &touched);
        font.validate_invariants().map_err(|error| {
            EditError::InvalidArgument(format!(
                "cannot revert transaction {tx}: the result would break a font invariant: {error}"
            ))
        })?;

        let mut touched = touched;
        if font.glyph_order != self.font.glyph_order {
            touched.insert(EntityKey::GlyphOrder);
        }
        Ok(self.push_undoable(actor, EntryKind::Revert, label, touched, Arc::new(font)))
    }

    /// Sets the whole document back to its state at revision `rev`, as a new
    /// undoable revision by `actor` that touches every entity whose value
    /// differs, based on the revision `base_rev` the actor last saw.
    ///
    /// # Errors
    ///
    /// * [`EditError::InvalidArgument`] if `base_rev` is newer than the
    ///   current revision, or if the document already has the content of
    ///   revision `rev`.
    /// * [`EditError::NotFound`] if revision `rev` does not exist.
    /// * [`EditError::Conflict`] if any entity the restore would change, other
    ///   than the glyph order, changed after `base_rev`: restoring would
    ///   silently overwrite a change the actor has not seen. The conflicts are
    ///   listed as in [`execute`](Self::execute).
    pub fn restore(
        &mut self,
        actor: &Actor,
        base_rev: Rev,
        rev: Rev,
    ) -> Result<TxOutcome, EditError> {
        self.check_base_rev(base_rev)?;
        let target = self
            .snapshot_at(rev)
            .ok_or_else(|| EditError::NotFound(format!("revision {}", rev.0)))?;
        let touched = changed_entities(&self.font, &target);
        if touched.is_empty() {
            return Err(EditError::InvalidArgument(format!(
                "already at that state: revision {} has the same content as the current one",
                rev.0
            )));
        }
        let conflicts = self.changed_since(&touched, |changed| changed > base_rev);
        if !conflicts.is_empty() {
            return Err(self.conflict(conflicts));
        }
        let label = format!("Restore revision {}", rev.0);
        Ok(self.push_undoable(actor, EntryKind::Restore, label, touched, target))
    }

    // ------------------------------------------------------------ internals

    fn check_base_rev(&self, base_rev: Rev) -> Result<(), EditError> {
        if base_rev > self.rev {
            return Err(EditError::InvalidArgument(format!(
                "base revision {} is newer than the current revision {}",
                base_rev.0, self.rev.0
            )));
        }
        Ok(())
    }

    /// The touched entities, other than the glyph order, whose last change
    /// revision satisfies `changed`, with that revision and its actor, in
    /// [`EntityKey`] order.
    fn changed_since(
        &self,
        touched: &BTreeSet<EntityKey>,
        changed: impl Fn(Rev) -> bool,
    ) -> Vec<(EntityKey, Rev, Actor)> {
        touched
            .iter()
            .filter(|key| **key != EntityKey::GlyphOrder)
            .filter_map(|key| {
                let (rev, actor) = self.entity_revs.get(key)?;
                changed(*rev).then(|| (*key, *rev, actor.clone()))
            })
            .collect()
    }

    fn conflict(&self, conflicts: Vec<(EntityKey, Rev, Actor)>) -> EditError {
        EditError::Conflict(ConflictInfo {
            current_rev: self.rev,
            conflicts,
        })
    }

    /// Records an undoable change (commit, revert or restore): it goes on the
    /// undo stack and discards everything that was undone.
    fn push_undoable(
        &mut self,
        actor: &Actor,
        kind: EntryKind,
        label: String,
        touched: BTreeSet<EntityKey>,
        font: Arc<Font>,
    ) -> TxOutcome {
        let before = Arc::clone(&self.font);
        let outcome = self.record(
            actor,
            kind,
            label.clone(),
            touched.clone(),
            Arc::clone(&font),
        );
        self.undo_stack.push(Step {
            actor: actor.clone(),
            label,
            touched,
            before,
            after: font,
        });
        self.redo_stack.clear();
        outcome
    }

    /// Moves the document to `font` as the next revision and records the
    /// change in the history and in the entity revisions.
    fn record(
        &mut self,
        actor: &Actor,
        kind: EntryKind,
        label: String,
        touched: BTreeSet<EntityKey>,
        font: Arc<Font>,
    ) -> TxOutcome {
        let rev = Rev(self.rev.0 + 1);
        let tx = TxId(self.next_tx);
        self.next_tx += 1;
        for key in &touched {
            self.entity_revs.insert(*key, (rev, actor.clone()));
        }
        self.snapshots.push(Arc::clone(&font));
        self.font = font;
        self.rev = rev;
        self.history.push(HistoryEntry {
            tx,
            rev,
            actor: actor.clone(),
            label,
            touched: touched.clone(),
            kind,
        });
        TxOutcome { tx, rev, touched }
    }
}

/// `current` with every entity in `touched` set back to its value in
/// `before`, and the glyph order reconciled.
fn reverted(current: &Font, before: &Font, touched: &BTreeSet<EntityKey>) -> Font {
    let mut font = current.clone();
    let mut restored = HashSet::new();
    for key in touched {
        match key {
            EntityKey::FontInfo => {
                font.info = before.info.clone();
                font.units_per_em = before.units_per_em;
            }
            EntityKey::VerticalMetrics(id) => {
                let source = before.masters.iter().find(|master| master.id == *id);
                let target = font.masters.iter_mut().find(|master| master.id == *id);
                if let (Some(source), Some(target)) = (source, target) {
                    target.metrics = source.metrics.clone();
                }
            }
            EntityKey::GlyphOrder => {}
            EntityKey::Glyph(id) => {
                let glyphs = Arc::make_mut(&mut font.glyphs);
                match before.glyphs.get(id) {
                    Some(glyph) => {
                        glyphs.insert(*id, Arc::clone(glyph));
                        restored.insert(*id);
                    }
                    None => {
                        glyphs.remove(id);
                    }
                }
            }
        }
    }
    font.glyph_order = reconciled_order(&font, &before.glyph_order, &restored);
    font
}

/// The current glyph order without the glyphs that no longer exist, with the
/// `restored` glyphs it lacks inserted at their index in `before_order`,
/// clamped to the end.
fn reconciled_order(
    font: &Font,
    before_order: &[GlyphId],
    restored: &HashSet<GlyphId>,
) -> Vec<GlyphId> {
    let mut order: Vec<GlyphId> = font
        .glyph_order
        .iter()
        .copied()
        .filter(|id| font.glyphs.contains_key(id))
        .collect();
    let mut listed: HashSet<GlyphId> = order.iter().copied().collect();
    for (index, id) in before_order.iter().enumerate() {
        if restored.contains(id) && listed.insert(*id) {
            order.insert(index.min(order.len()), *id);
        }
    }
    order
}
