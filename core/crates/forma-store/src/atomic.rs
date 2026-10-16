//! Crash-safe file replacement.
//!
//! A document is never written in place. The new contents go to a temporary
//! file next to the target and only a complete, flushed file is renamed over
//! it: the rename replaces the target atomically (`rename(2)` on Unix,
//! `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` on Windows), so a reader or
//! a crash sees either the old file or the new one, never a mixture. Whatever
//! goes wrong, the temporary file is removed and the target is left alone.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use uuid::Uuid;

/// The name of the temporary file used to replace `target`: the target's file
/// name plus `.tmp-<uuid>`, in the same directory so the final rename never
/// crosses a file system.
pub(crate) fn temp_path(target: &Path) -> io::Result<PathBuf> {
    let name = target.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no file name", target.display()),
        )
    })?;
    let mut temp_name = name.to_os_string();
    temp_name.push(format!(".tmp-{}", Uuid::new_v4()));
    Ok(target.with_file_name(temp_name))
}

/// Whether files and directories are flushed to the storage device.
///
/// Always true, except when this crate's own tests enable the `test-no-fsync`
/// feature (through the self dev-dependency in `Cargo.toml`) to avoid spending
/// minutes in `fsync` on a property test. Nothing else may enable it: a build
/// without durability can lose a saved font on power failure.
const FSYNC: bool = !cfg!(feature = "test-no-fsync");

/// Flushes an open file's data to the storage device.
pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    if FSYNC { file.sync_all() } else { Ok(()) }
}

/// Creates a new file at `path`, which must not exist, holding `contents`
/// flushed to disk. On failure the partial file is left for the caller to
/// remove.
pub(crate) fn create_synced(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    file.flush()?;
    sync_file(&file)
}

/// Replaces `target` with a file that `fill` creates at the path it is given.
///
/// `fill` must create a complete file at that path (the path is unique and
/// does not exist yet) and close it. The file is then flushed to disk, renamed
/// over `target` and the directory is flushed. If anything fails the temporary
/// file is deleted and `target` is untouched. This is the general form, for
/// writers that need a path rather than an open file (SQLite).
pub(crate) fn write_atomic_at<E>(
    target: &Path,
    fill: impl FnOnce(&Path) -> Result<(), E>,
) -> Result<(), E>
where
    E: From<io::Error>,
{
    let temp = temp_path(target)?;
    let result = fill(&temp)
        .and_then(|()| flush_file(&temp).map_err(E::from))
        .and_then(|()| fs::rename(&temp, target).map_err(E::from));
    match result {
        Ok(()) => {
            sync_parent(target);
            Ok(())
        }
        Err(err) => {
            remove_quietly(&temp);
            Err(err)
        }
    }
}

/// Data must be on disk before the rename makes the file visible, or a power
/// failure could leave an empty file under the final name.
fn flush_file(path: &Path) -> io::Result<()> {
    if FSYNC {
        OpenOptions::new().write(true).open(path)?.sync_all()
    } else {
        Ok(())
    }
}

/// Replaces `target` with the file that `fill` writes.
///
/// `fill` receives a freshly created temporary file. See [`write_atomic_at`]
/// for the failure behavior.
pub(crate) fn write_atomic(
    target: &Path,
    fill: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    write_atomic_at(target, |temp| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(temp)?;
        fill(&mut file)?;
        file.flush()
    })
}

/// Atomically replaces `target` with `contents`.
pub(crate) fn write_file_atomic(target: &Path, contents: &[u8]) -> io::Result<()> {
    write_atomic(target, |file| file.write_all(contents))
}

/// Flushes the directory entry created by a rename, where the platform lets us.
///
/// This is best effort: the rename already happened and a failure to flush it
/// is not a failed save.
pub(crate) fn sync_parent(path: &Path) {
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    sync_dir(dir);
}

/// Flushes a directory so that the files created, renamed and deleted in it
/// survive a power failure. Windows offers no directory flush; NTFS journals
/// its metadata.
#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) {
    if FSYNC && let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
}

/// See the Unix version.
#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) {}

/// Deletes a leftover temporary file, ignoring errors: the original failure
/// is the one worth reporting.
pub(crate) fn remove_quietly(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{self, Write};

    use tempfile::TempDir;

    use super::{write_atomic, write_atomic_at, write_file_atomic};

    fn names(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn failed_save_keeps_original() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("font.forma");
        fs::write(&target, b"original contents").unwrap();

        // A writer that gets partway through and then fails, like a disk
        // that fills up or a process that is interrupted.
        let result = write_atomic(&target, |file| {
            file.write_all(b"partial new conte")?;
            Err(io::Error::other("injected failure"))
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original contents");
        assert_eq!(
            names(dir.path()),
            ["font.forma"],
            "no temporary file may remain"
        );
    }

    #[test]
    fn successful_save_replaces_the_original_and_leaves_nothing_behind() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("font.forma");
        fs::write(&target, b"original contents that are rather long").unwrap();

        write_file_atomic(&target, b"new").unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(names(dir.path()), ["font.forma"]);
    }

    #[test]
    fn creates_the_target_when_it_does_not_exist() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("new.json");

        write_file_atomic(&target, b"{}").unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"{}");
        assert_eq!(names(dir.path()), ["new.json"]);
    }

    #[test]
    fn failure_in_a_path_based_writer_removes_the_temporary_file() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("font.forma");
        fs::write(&target, b"original").unwrap();

        let result: io::Result<()> = write_atomic_at(&target, |temp| {
            fs::write(temp, b"half a database")?;
            Err(io::Error::other("injected failure"))
        });

        assert!(result.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert_eq!(names(dir.path()), ["font.forma"]);
    }

    #[test]
    fn failing_rename_removes_the_temporary_file() {
        // A directory cannot be replaced by a file.
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("font.forma");
        fs::create_dir(&target).unwrap();

        let result = write_file_atomic(&target, b"data");

        assert!(result.is_err());
        assert!(target.is_dir());
        assert_eq!(names(dir.path()), ["font.forma"]);
    }

    #[test]
    fn temporary_files_are_unique_siblings_of_the_target() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("font.forma");

        let first = super::temp_path(&target).unwrap();
        let second = super::temp_path(&target).unwrap();

        assert_ne!(first, second);
        assert_eq!(first.parent(), target.parent());
        assert!(
            first
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("font.forma.tmp-"))
        );
    }
}
