//! Persistence of FORMA fonts.
//!
//! A font is stored in one of two native formats, chosen by the file name:
//!
//! * `.forma`: a single SQLite file, for working;
//! * `.formapackage`: a folder of canonical JSON files, for version control.
//!
//! Both hold the same data and convert losslessly into each other ([`pack`],
//! [`unpack`]). Losing or corrupting a font is the worst thing a font editor
//! can do, so:
//!
//! * saving never edits a document in place: the new contents are written to
//!   temporary files, flushed to disk and only then renamed over the old ones,
//!   and a save that fails before that point leaves the old document
//!   untouched;
//! * nothing that could not be loaded back is written, and [`load`] checks
//!   every font with [`Font::validate_invariants`] before returning it;
//! * saving a package never writes into a folder that is not already a
//!   package (or empty), and neither saving nor loading follows a symbolic
//!   link inside a package ([`StoreError::SymlinkInPackage`]);
//! * the JSON is canonical ([`to_canonical_json`]): saving the same font twice
//!   gives identical bytes.
//!
//! Between saves, the [`Journal`] keeps every committed edit on disk so that a
//! crash loses nothing: on reopening, [`replay`] and [`apply_records`] put the
//! journaled edits back over the last saved file.

#![forbid(unsafe_code)]

mod atomic;
mod canonical;
mod filenames;
mod journal;
mod package;
mod parts;
mod sqlite;

use std::path::{Path, PathBuf};

use forma_model::Font;
use thiserror::Error;

pub use canonical::{to_canonical_json, try_to_canonical_json};
pub use filenames::glyph_file_name;
pub use journal::{Journal, JournalRecord, apply_records, remove as remove_journal, replay};

/// Everything that can go wrong while saving or loading a document.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The path has neither the `.forma` nor the `.formapackage` extension.
    #[error("{}: not a FORMA document (expected a .forma file or a .formapackage folder)", .0.display())]
    UnknownFormat(PathBuf),
    /// The file is not a `.forma` file: it is not an SQLite database, or
    /// belongs to another application.
    #[error("{}: not a FORMA file", .0.display())]
    NotAFormaFile(PathBuf),
    /// The folder is not a `.formapackage`: it has no `manifest.json` that
    /// says so.
    #[error("{}: not a FORMA package", .0.display())]
    NotAFormaPackage(PathBuf),
    /// The document was written by a newer version of FORMA.
    #[error("unsupported format version {0}; this FORMA reads version 1")]
    UnsupportedVersion(u32),
    /// The document is damaged, or breaks a data model rule.
    #[error("invalid document: {0}")]
    Invalid(String),
    /// JSON could not be produced or parsed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// SQLite reported an error.
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A file system operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A part of a `.formapackage` (`manifest.json`, `font.json`, the
    /// `glyphs` folder or an entry in it) is a symbolic link (on Windows, also
    /// a junction). Links are never followed: one could make a load read, or
    /// a save write or delete, files outside the package.
    #[error("{}: symbolic links are not allowed inside a package", .0.display())]
    SymlinkInPackage(PathBuf),
}

impl StoreError {
    /// Whether the error comes from the environment (the file system, the
    /// disk, permissions, locks) rather than from the document itself:
    /// [`StoreError::Io`], and SQLite failures for a full disk (`SQLITE_FULL`),
    /// an I/O error (`SQLITE_IOERR*`), a file that cannot be opened
    /// (`SQLITE_CANTOPEN`), a read-only database (`SQLITE_READONLY`), a denied
    /// permission (`SQLITE_PERM`) or a busy or locked database (`SQLITE_BUSY`,
    /// `SQLITE_LOCKED`). A file that is not a database (`SQLITE_NOTADB`) is a
    /// problem of the document, not of the environment.
    #[must_use]
    pub fn is_io_failure(&self) -> bool {
        use rusqlite::ErrorCode as Code;
        match self {
            Self::Io(_) => true,
            Self::Sqlite(rusqlite::Error::SqliteFailure(error, _)) => matches!(
                error.code,
                Code::DiskFull
                    | Code::SystemIoFailure
                    | Code::CannotOpen
                    | Code::ReadOnly
                    | Code::PermissionDenied
                    | Code::DatabaseBusy
                    | Code::DatabaseLocked
            ),
            _ => false,
        }
    }
}

/// The two document formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    /// A single SQLite file.
    Forma,
    /// A folder of JSON files.
    Package,
}

/// Picks the format from the extension of `path`, ignoring case.
fn format_of(path: &Path) -> Result<Format, StoreError> {
    let extension = path.extension().and_then(|extension| extension.to_str());
    match extension {
        Some(extension) if extension.eq_ignore_ascii_case("forma") => Ok(Format::Forma),
        Some(extension) if extension.eq_ignore_ascii_case("formapackage") => Ok(Format::Package),
        _ => Err(StoreError::UnknownFormat(path.to_path_buf())),
    }
}

/// Saves `font` to `path`: a `.forma` file or a `.formapackage` folder,
/// according to the extension.
///
/// An existing `.forma` file is replaced atomically. An existing
/// `.formapackage` folder is updated in a stage-then-commit sequence: every
/// changed file is written and flushed under a temporary name first, so a
/// failure there leaves the package untouched, and only then are they renamed
/// into place (see the `package` module for the exact guarantee). Files that
/// did not change are not rewritten, and glyph files of glyphs that no longer
/// exist are deleted at the end. A folder that is neither empty nor a package
/// is refused and left untouched.
///
/// # Errors
///
/// * [`StoreError::UnknownFormat`] if the extension is neither `.forma` nor
///   `.formapackage`;
/// * [`StoreError::Invalid`] if the font breaks a data model rule or holds a
///   number that JSON cannot represent (NaN, infinity);
/// * [`StoreError::NotAFormaPackage`] if the target is a folder or file that
///   is not a package;
/// * [`StoreError::UnsupportedVersion`] if the target is a package written by
///   a newer FORMA;
/// * [`StoreError::SymlinkInPackage`] if a part of the target package is a
///   symbolic link (nothing is written or deleted);
/// * [`StoreError::Io`] and [`StoreError::Sqlite`] if writing fails; the
///   previous document is then intact (see above for packages).
pub fn save(font: &Font, path: &Path) -> Result<(), StoreError> {
    match format_of(path)? {
        Format::Forma => sqlite::save(font, path),
        Format::Package => package::save(font, path),
    }
}

/// Loads the font stored at `path`, a `.forma` file or a `.formapackage`
/// folder according to the extension.
///
/// The font is checked with [`Font::validate_invariants`]; a document that
/// breaks the data model is rejected rather than returned.
///
/// # Errors
///
/// * [`StoreError::UnknownFormat`] for any other extension;
/// * [`StoreError::NotAFormaFile`] or [`StoreError::NotAFormaPackage`] if the
///   file or folder is not a FORMA document;
/// * [`StoreError::UnsupportedVersion`] if it was written by a newer FORMA;
/// * [`StoreError::Invalid`] if it is damaged or breaks a data model rule;
/// * [`StoreError::SymlinkInPackage`] if a part of a package is a symbolic
///   link (nothing is read through it);
/// * [`StoreError::Json`], [`StoreError::Sqlite`] and [`StoreError::Io`] if it
///   cannot be read.
pub fn load(path: &Path) -> Result<Font, StoreError> {
    match format_of(path)? {
        Format::Forma => sqlite::load(path),
        Format::Package => package::load(path),
    }
}

/// Converts the document at `src` into the document at `dst`; the format of
/// each is chosen by its extension. Typically `src` is a `.formapackage` and
/// `dst` a `.forma` file.
///
/// The source is loaded and validated completely before anything is written,
/// and is never modified.
///
/// # Errors
///
/// [`StoreError::UnknownFormat`] if `dst` has neither extension (checked
/// first, so nothing is read or written), plus every error of [`load`] for
/// `src` and of [`save`] for `dst`.
pub fn pack(src: &Path, dst: &Path) -> Result<(), StoreError> {
    convert(src, dst)
}

/// Converts the document at `src` into the document at `dst`; the format of
/// each is chosen by its extension. Typically `src` is a `.forma` file and
/// `dst` a `.formapackage`.
///
/// This is [`pack`] under the name that matches the usual direction.
///
/// # Errors
///
/// The same as [`pack`].
pub fn unpack(src: &Path, dst: &Path) -> Result<(), StoreError> {
    convert(src, dst)
}

fn convert(src: &Path, dst: &Path) -> Result<(), StoreError> {
    format_of(dst)?;
    let font = load(src)?;
    save(&font, dst)
}

#[cfg(test)]
mod tests {
    use rusqlite::ffi;

    use super::*;

    fn sqlite(code: std::os::raw::c_int) -> StoreError {
        StoreError::Sqlite(rusqlite::Error::SqliteFailure(ffi::Error::new(code), None))
    }

    #[test]
    fn io_failures_are_told_apart_from_document_problems() {
        for code in [
            ffi::SQLITE_FULL,
            ffi::SQLITE_IOERR,
            ffi::SQLITE_IOERR_WRITE,
            ffi::SQLITE_IOERR_FSYNC,
            ffi::SQLITE_CANTOPEN,
            ffi::SQLITE_READONLY,
            ffi::SQLITE_PERM,
            ffi::SQLITE_BUSY,
            ffi::SQLITE_LOCKED,
        ] {
            assert!(sqlite(code).is_io_failure(), "code {code}");
        }
        assert!(StoreError::Io(std::io::Error::other("disk")).is_io_failure());

        for code in [
            ffi::SQLITE_NOTADB,
            ffi::SQLITE_CORRUPT,
            ffi::SQLITE_CONSTRAINT,
        ] {
            assert!(!sqlite(code).is_io_failure(), "code {code}");
        }
        assert!(!StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows).is_io_failure());
        assert!(!StoreError::Invalid("broken".into()).is_io_failure());
        assert!(!StoreError::SymlinkInPackage(PathBuf::from("glyphs")).is_io_failure());
        assert!(!StoreError::UnknownFormat(PathBuf::from("a.txt")).is_io_failure());
    }
}
