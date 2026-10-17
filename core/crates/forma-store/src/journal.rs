//! The crash-recovery journal.
//!
//! After every committed edit the engine appends the resulting state of the
//! entities the edit touched to an append-only file, one [`JournalRecord`] per
//! line. If the program dies before the next save, [`replay`] reads the records
//! back and [`apply_records`] puts them over the last saved file, so committed
//! work is never lost. A successful save makes the journal redundant:
//! [`Journal::truncate`] empties it, or [`remove`] deletes it once the document
//! is closed.
//!
//! # File format
//!
//! A journal belongs to one document *file*, not just to one font: two copies
//! of a font (same font id) in two files have two journals. Its name is
//! `<dir>/<font id>-<hash>.journal` ([`JournalKey::file_name`]), where `<hash>`
//! is the first 16 hexadecimal digits of the SHA-256 of the canonical path of
//! the document, and every record names that path again (`document`), so a
//! journal is only ever replayed over the file it was written for.
//!
//! The file is JSONL: each line is one record as compact canonical JSON (keys
//! sorted at every level, as in the saved documents, but no indentation) ended
//! by `\n`. A record holds *states*, not changes: the new value of the font
//! information, the masters, the glyph order and each touched glyph (`null`
//! for a glyph that was deleted). Applying a record twice therefore gives the
//! same font as applying it once, which keeps replay safe when an append
//! reported an error but its line reached the disk anyway.
//!
//! # Crashes
//!
//! A record is written with one `write_all` and flushed with `sync_data` before
//! [`Journal::append`] returns, so an edit whose append returned survives a
//! crash or a power failure. A crash in the middle of an append leaves a
//! partial last line: [`replay`] ignores it (the edit it belonged to was never
//! reported as saved) and [`Journal::open`] removes it, so that the records
//! appended next are not glued to it. Any other unreadable line cannot be
//! explained by a crash and is reported as corruption rather than skipped.
//!
//! A journal has a single writer: one open [`Journal`] per document, in the
//! one process that holds the document open for writing (the engine's
//! document lock enforces that across processes).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forma_model::{Actor, Font, FontId, FontInfo, Glyph, GlyphId, Master, Rev};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::StoreError;
use crate::atomic::{sync_dir, sync_file, sync_file_data};
use crate::canonical::canonical_line;
use crate::parts::{ensure_glyph_finite, ensure_masters_finite};

/// The document a journal belongs to: a font, in one file.
///
/// The file is named by its canonical path (as the engine resolves it), so
/// that two copies of a font never share a journal and a journal is never
/// replayed over another file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct JournalKey {
    font_id: FontId,
    document: PathBuf,
}

impl JournalKey {
    /// The journal of the font `font_id` stored at `document`, the canonical
    /// path of a `.forma` file or `.formapackage` folder.
    #[must_use]
    pub fn new(font_id: FontId, document: &Path) -> JournalKey {
        JournalKey {
            font_id,
            document: document.to_path_buf(),
        }
    }

    /// The font id.
    #[must_use]
    pub fn font_id(&self) -> FontId {
        self.font_id
    }

    /// The canonical path of the document file.
    #[must_use]
    pub fn document(&self) -> &Path {
        &self.document
    }

    /// The path as the records carry it in [`JournalRecord::document`]
    /// (lossy for a path that is not valid Unicode).
    #[must_use]
    pub fn document_text(&self) -> String {
        self.document.to_string_lossy().into_owned()
    }

    /// The name of the journal file: `<font id>-<hash>.journal`, where
    /// `<hash>` is the first 16 lowercase hexadecimal digits of the SHA-256 of
    /// the path (of its bytes as the operating system encodes them).
    #[must_use]
    pub fn file_name(&self) -> String {
        let digest = Sha256::digest(self.document.as_os_str().as_encoded_bytes());
        let hash: String = digest
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("{}-{hash}.journal", self.font_id)
    }
}

/// The state of everything one committed edit touched.
///
/// Parts the edit did not touch are `None` (or absent from `glyphs`), so a
/// record stays small however large the font is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalRecord {
    /// The canonical path of the document file the record belongs to (see
    /// [`JournalKey::document_text`]). A journal only takes, and only
    /// replays, records of its own document.
    pub document: String,
    /// The revision of the document after the edit.
    pub rev: Rev,
    /// Who made the edit.
    pub actor: Actor,
    /// A short description of the edit, for history.
    pub label: String,
    /// The new font information, if the edit changed it.
    pub font_info: Option<FontInfo>,
    /// The new units per em, when the edit changed the font information (the
    /// units per em are part of it as far as edits are concerned). Absent
    /// from the line when `None`, so records written before this field
    /// existed read back unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units_per_em: Option<u16>,
    /// The new list of masters, if the edit changed it.
    pub masters: Option<Vec<Master>>,
    /// The new glyph order, if the edit changed it.
    pub glyph_order: Option<Vec<GlyphId>>,
    /// The new state of each glyph the edit touched; `None` means the glyph
    /// was deleted. Serialized as an array of `[id, glyph-or-null]` pairs.
    pub glyphs: Vec<(GlyphId, Option<Glyph>)>,
}

impl JournalRecord {
    /// The line that stores this record, newline included.
    ///
    /// JSON has no NaN or infinity and `serde_json` would write them as `null`,
    /// a line that could never be read back; they are refused here, before
    /// anything reaches the file.
    fn to_line(&self) -> Result<Vec<u8>, StoreError> {
        if let Some(masters) = &self.masters {
            ensure_masters_finite(masters)?;
        }
        for glyph in self.glyphs.iter().filter_map(|(_, glyph)| glyph.as_ref()) {
            ensure_glyph_finite(glyph)?;
        }
        Ok(canonical_line(&serde_json::to_value(self)?)?.into_bytes())
    }
}

/// An open journal, ready to take records.
///
/// There must be only one per document at a time.
#[derive(Debug)]
pub struct Journal {
    /// Opened in append mode, so every write goes to the end of the file
    /// (also after [`Journal::truncate`] emptied it).
    file: File,
    path: PathBuf,
    /// The `document` every record must carry.
    document: String,
    /// The length of the file as far as this journal knows: where the next
    /// record starts.
    len: u64,
}

/// The journal file of a document.
fn journal_path(dir: &Path, key: &JournalKey) -> PathBuf {
    dir.join(key.file_name())
}

/// The error of a record that belongs to another document.
fn another_document(found: &str, key: &JournalKey) -> StoreError {
    StoreError::Invalid(format!(
        "the journal holds records of another document ('{found}', not '{}')",
        key.document_text()
    ))
}

/// Cuts the file at `path` to `len` bytes and flushes it.
///
/// This opens the file again for writing rather than using the append handle:
/// on Windows a file opened for appending may not be resized.
fn truncate_to(path: &Path, len: u64) -> io::Result<()> {
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(len)?;
    sync_file(&file)
}

/// Whether `line` has nothing in it but white space.
fn is_blank(line: &[u8]) -> bool {
    line.iter().all(u8::is_ascii_whitespace)
}

impl Journal {
    /// Opens the journal of the document `key` in `dir`, creating the folder
    /// and the file if they do not exist. New records are added after the ones
    /// already there.
    ///
    /// If the file ends in the middle of a line, as after a crash in the
    /// middle of an append, the incomplete line is removed; if it is a whole
    /// record that only lost its newline, the newline is added.
    ///
    /// # Errors
    ///
    /// [`StoreError::Io`] if the folder or the file cannot be created, opened
    /// or repaired.
    pub fn open(dir: &Path, key: &JournalKey) -> Result<Journal, StoreError> {
        fs::create_dir_all(dir)?;
        let path = journal_path(dir, key);
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&path)?;
        // A new file is only safe once its directory entry is.
        sync_dir(dir);
        let len = repair_tail(&mut file, &path)?;
        Ok(Journal {
            file,
            path,
            document: key.document_text(),
            len,
        })
    }

    /// Appends `rec` and flushes it to disk before returning.
    ///
    /// If the write fails, the journal is cut back to the end of the last
    /// whole record (as far as the file system lets it), so a later append
    /// does not land after half a line.
    ///
    /// # Errors
    ///
    /// * [`StoreError::Invalid`] if the record belongs to another document
    ///   (its `document` is not this journal's) or holds a number that JSON
    ///   cannot represent (NaN, infinity); nothing is written;
    /// * [`StoreError::Json`] if it cannot be serialized;
    /// * [`StoreError::Io`] if writing or flushing fails.
    pub fn append(&mut self, rec: &JournalRecord) -> Result<(), StoreError> {
        if rec.document != self.document {
            return Err(StoreError::Invalid(format!(
                "a record of '{}' cannot go into the journal of '{}'",
                rec.document, self.document
            )));
        }
        let line = rec.to_line()?;
        if let Err(err) = self.file.write_all(&line) {
            // Best effort: the write error is the one worth reporting.
            let _ = truncate_to(&self.path, self.len);
            return Err(err.into());
        }
        self.len += line.len() as u64;
        sync_file_data(&self.file)?;
        Ok(())
    }

    /// Empties the journal and flushes that to disk; later appends start a
    /// fresh journal. Call it once everything in the journal has been saved.
    ///
    /// # Errors
    ///
    /// [`StoreError::Io`] if the file cannot be emptied.
    pub fn truncate(&mut self) -> Result<(), StoreError> {
        truncate_to(&self.path, 0)?;
        self.len = 0;
        Ok(())
    }
}

/// Makes the journal end at a line boundary and returns its length.
///
/// Each record is written in one piece with its newline last, so a file that
/// does not end in a newline ends in the debris of an interrupted append (or,
/// after a power failure, in bytes the file system extended the file with). If
/// the last line is nevertheless a whole record, only its newline is missing
/// and it is kept; otherwise it is cut off.
fn repair_tail(file: &mut File, path: &Path) -> Result<u64, StoreError> {
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(0);
    }
    file.seek(SeekFrom::Start(len - 1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(len);
    }

    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let start = bytes
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |newline| newline + 1);
    if serde_json::from_slice::<JournalRecord>(&bytes[start..]).is_ok() {
        file.write_all(b"\n")?;
        sync_file_data(file)?;
        Ok(bytes.len() as u64 + 1)
    } else {
        let start = start as u64;
        truncate_to(path, start)?;
        Ok(start)
    }
}

/// Reads the records of the journal of the document `key` in `dir`, oldest
/// first.
///
/// A journal that does not exist has no records. An incomplete last line, the
/// trace of a crash in the middle of an append, is ignored.
///
/// # Errors
///
/// * [`StoreError::Invalid`] (`journal corrupt at line N`, counting from 1) if
///   a line that is not the last one cannot be read as a record: that is not
///   something a crash does, and silently skipping it would drop edits;
/// * [`StoreError::Invalid`] if a record belongs to another document (its
///   `document` is not the path of `key`): the file was copied or renamed,
///   and its records must not be put over this document;
/// * [`StoreError::Io`] if the file exists but cannot be read.
pub fn replay(dir: &Path, key: &JournalKey) -> Result<Vec<JournalRecord>, StoreError> {
    let bytes = match fs::read(journal_path(dir, key)) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };

    // Work on bytes: a crash can cut a multi-byte character in two.
    let document = key.document_text();
    let lines: Vec<&[u8]> = bytes.split(|&byte| byte == b'\n').collect();
    let last = lines.iter().rposition(|line| !is_blank(line));
    let mut records = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if is_blank(line) {
            continue;
        }
        match serde_json::from_slice::<JournalRecord>(line) {
            Ok(record) if record.document != document => {
                return Err(another_document(&record.document, key));
            }
            Ok(record) => records.push(record),
            Err(_) if Some(index) == last => break,
            Err(_) => {
                return Err(StoreError::Invalid(format!(
                    "journal corrupt at line {}",
                    index + 1
                )));
            }
        }
    }
    Ok(records)
}

/// Deletes the journal of the document `key` in `dir`. A journal that does
/// not exist is not an error. Call it after the document was saved and
/// closed, when there is nothing left to recover.
///
/// # Errors
///
/// [`StoreError::Io`] if the file exists but cannot be deleted.
pub fn remove(dir: &Path, key: &JournalKey) -> Result<(), StoreError> {
    match fs::remove_file(journal_path(dir, key)) {
        Ok(()) => {
            sync_dir(dir);
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Applies `recs`, oldest first, over `base` (normally the last saved file)
/// and returns the font as it was at the last record.
///
/// Per record: a present `font_info`, `units_per_em`, `masters` or
/// `glyph_order` replaces the font's (`document` only says which file the
/// record belongs to, see [`replay`]); a glyph with `Some` state is inserted
/// or replaced and one with `None` is removed. Other parts of the font (its
/// id, `lib`) are never touched by a record and stay as they are in `base`.
///
/// The records are only trusted once the result checks out: it is validated
/// with [`Font::validate_invariants`] after the last record (an edit that
/// needed several records may be inconsistent in between). With no records the
/// result is `base`, validated.
///
/// # Errors
///
/// [`StoreError::Invalid`] if the result breaks a data model rule, which
/// means the records do not belong to `base` or the journal was damaged.
pub fn apply_records(base: Font, recs: &[JournalRecord]) -> Result<Font, StoreError> {
    let mut font = base;
    for rec in recs {
        if let Some(info) = &rec.font_info {
            font.info = info.clone();
        }
        if let Some(units) = rec.units_per_em {
            font.units_per_em = units;
        }
        if let Some(masters) = &rec.masters {
            font.masters = masters.clone();
        }
        if let Some(order) = &rec.glyph_order {
            font.glyph_order = order.clone();
        }
        if !rec.glyphs.is_empty() {
            let glyphs = Arc::make_mut(&mut font.glyphs);
            for (id, state) in &rec.glyphs {
                match state {
                    Some(glyph) => {
                        glyphs.insert(*id, Arc::new(glyph.clone()));
                    }
                    None => {
                        glyphs.remove(id);
                    }
                }
            }
        }
    }
    font.validate_invariants().map_err(|err| {
        StoreError::Invalid(format!(
            "the journal does not apply to the saved font: {err}"
        ))
    })?;
    Ok(font)
}
