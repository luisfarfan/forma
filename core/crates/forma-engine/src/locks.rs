//! The single writer of a document file (docs/03-architecture.md §7).
//!
//! When an engine with a lock folder ([`EngineConfig::lock_dir`]) opens or
//! creates a document that has a file, it takes an exclusive operating-system
//! lock ([`File::try_lock`]) on `<lock_dir>/<sha256 of the canonical path>.lock`
//! and keeps it until the document is closed (or the process ends, crash
//! included: the system releases the lock). Every FORMA process configured
//! with the same lock folder (the CLI and `forma mcp` default to the same
//! one) therefore agrees on who writes a file:
//!
//! * the engine that holds the lock is the only one that saves the file and
//!   the only one that opens, extends, empties or deletes its crash-recovery
//!   journal;
//! * an engine that finds the lock taken opens the document **read-only**:
//!   reads, previews, validation and exports to a new file work, while every
//!   change, save, export over an existing file or discarding close is
//!   refused with `UNSUPPORTED`, and the journal is never looked at;
//! * a read-only engine ([`EngineConfig::read_only`]) never takes a lock (it
//!   must not keep a writer out) and never touches a journal either.
//!
//! Lock files are never deleted: deleting one while another process waits on
//! it would let two processes hold "the" lock of the same file.
//!
//! [`EngineConfig::lock_dir`]: crate::EngineConfig::lock_dir
//! [`EngineConfig::read_only`]: crate::EngineConfig::read_only

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::{Engine, path_text};
use crate::errors::ToolError;
use crate::plans::sha256;

/// The message of every change refused on a read-only handle.
pub(crate) const READ_ONLY_HANDLE: &str =
    "document is open for writing in another FORMA process; this handle is read-only";

/// An exclusive lock on a document file, released when the last clone of the
/// open document that holds it is dropped.
#[derive(Debug)]
pub(crate) struct LockFile {
    /// The locked file; the lock lives as long as this handle.
    _file: File,
}

/// How an open document stands with the lock of its file.
#[derive(Clone, Debug, Default)]
pub(crate) enum FileLock {
    /// Nothing is locked: the document has no file, or the engine has no lock
    /// folder (it then assumes it is the only writer, as before locks
    /// existed).
    #[default]
    Unlocked,
    /// This engine holds the lock: it is the file's only writer. The lock is
    /// released when the last clone of the guard is dropped.
    Held {
        /// Kept only for its lifetime.
        _guard: Arc<LockFile>,
    },
    /// Another process held the lock when the document was opened: the
    /// handle is read-only.
    HeldElsewhere,
    /// The engine is read-only and never takes locks.
    ReadOnlyEngine,
}

impl FileLock {
    /// Whether this engine may write the document's file and its journal.
    #[must_use]
    pub(crate) fn owns_file(&self) -> bool {
        matches!(self, FileLock::Unlocked | FileLock::Held { .. })
    }

    /// Whether the handle is read-only.
    #[must_use]
    pub(crate) fn read_only(&self) -> bool {
        !self.owns_file()
    }

    /// Whether another process held the file open for writing.
    #[must_use]
    pub(crate) fn held_elsewhere(&self) -> bool {
        matches!(self, FileLock::HeldElsewhere)
    }
}

/// The error of a change refused on a read-only handle.
#[must_use]
pub(crate) fn read_only_handle() -> ToolError {
    ToolError::unsupported(READ_ONLY_HANDLE)
}

/// The error of a file another process holds open for writing, as the target
/// of a new document or of a save.
#[must_use]
pub(crate) fn locked_target(path: &Path) -> ToolError {
    ToolError::invalid_argument(format!(
        "'{}' is open for writing in another FORMA process; close it there or choose another \
         path",
        path_text(path)
    ))
}

/// The name of the lock file of the document at `path`: the lowercase
/// hexadecimal SHA-256 of the path (of its bytes as the operating system
/// encodes them), then `.lock`.
#[must_use]
pub(crate) fn lock_file_name(path: &Path) -> String {
    let digest = sha256(path.as_os_str().as_encoded_bytes());
    let mut name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    name.push_str(".lock");
    name
}

impl Engine {
    /// Takes the lock of the document file at `path` (a canonical path) for
    /// this engine, if it can; see the module documentation.
    ///
    /// # Errors
    ///
    /// `IO_ERROR` if the lock folder or the lock file cannot be created or
    /// opened, or the lock cannot be queried.
    pub(crate) fn lock_document(&self, path: &Path) -> Result<FileLock, ToolError> {
        if self.cfg.read_only {
            return Ok(FileLock::ReadOnlyEngine);
        }
        let Some(dir) = self.cfg.lock_dir.as_deref() else {
            return Ok(FileLock::Unlocked);
        };
        let lock_path: PathBuf = dir.join(lock_file_name(path));
        let failed = |what: &str, error: &dyn std::fmt::Display| {
            ToolError::io(format!(
                "cannot {what} the lock of '{}' ('{}'): {error}",
                path_text(path),
                path_text(&lock_path)
            ))
        };
        fs::create_dir_all(dir).map_err(|error| failed("create the folder of", &error))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| failed("open", &error))?;
        match file.try_lock() {
            Ok(()) => Ok(FileLock::Held {
                _guard: Arc::new(LockFile { _file: file }),
            }),
            Err(TryLockError::WouldBlock) => Ok(FileLock::HeldElsewhere),
            Err(TryLockError::Error(error)) => Err(failed("take", &error)),
        }
    }

    /// Refuses a change of the document `handle` when its handle is
    /// read-only. An unknown handle passes (the tool reports it).
    pub(crate) fn ensure_writable(&self, handle: &str) -> Result<(), ToolError> {
        match self.docs.get(handle) {
            Some(open) if open.lock.read_only() => Err(read_only_handle()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineConfig;

    #[test]
    fn lock_files_are_named_by_the_sha256_of_the_path() {
        let name = lock_file_name(Path::new("/fonts/a.forma"));
        assert_eq!(name.len(), 64 + ".lock".len());
        assert_ne!(name, lock_file_name(Path::new("/fonts/b.forma")));
        assert_eq!(name, lock_file_name(Path::new("/fonts/a.forma")));
    }

    #[test]
    fn a_lock_is_exclusive_until_released() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cfg = EngineConfig {
            lock_dir: Some(dir.path().join("locks")),
            ..EngineConfig::default()
        };
        let (first, second) = (Engine::new(cfg.clone()), Engine::new(cfg));
        let path = Path::new("/fonts/a.forma");
        let held = first.lock_document(path).expect("lock");
        assert!(matches!(held, FileLock::Held { .. }));
        assert!(second.lock_document(path).expect("lock").held_elsewhere());
        // Another file is another lock.
        let other = second.lock_document(Path::new("/fonts/b.forma"));
        assert!(matches!(other, Ok(FileLock::Held { .. })));
        drop(held);
        assert!(matches!(
            second.lock_document(path),
            Ok(FileLock::Held { .. })
        ));

        let read_only = Engine::new(EngineConfig {
            read_only: true,
            lock_dir: Some(dir.path().join("locks")),
            ..EngineConfig::default()
        });
        assert!(matches!(
            read_only.lock_document(path),
            Ok(FileLock::ReadOnlyEngine)
        ));
        let unlocked = Engine::new(EngineConfig::default());
        assert!(matches!(
            unlocked.lock_document(path),
            Ok(FileLock::Unlocked)
        ));
    }
}
