//! Backups of documents written by an older format, taken before the first
//! save in the current format replaces them.
//!
//! Migrations are pure and lose nothing (see `parts`), but an older FORMA
//! cannot read the newer format. Keeping an untouched copy of the old
//! document is the way back: `<name>.v<n>.bak` next to it, for a `.forma`
//! file or a `.formapackage` folder alike. A backup is taken once and never
//! overwritten.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::atomic::{remove_quietly, sync_parent, write_file_atomic};
use crate::parts::FORMAT_VERSION;
use crate::{Format, StoreError, format_of};

/// The path of the backup of the format `version` document at `path`: a
/// sibling named `<name>.v<version>.bak` (a trailing separator in `path` is
/// ignored, so the backup never lands inside a package).
#[must_use]
pub fn backup_path(path: &Path, version: u32) -> PathBuf {
    let clean: PathBuf = path.components().collect();
    let mut name: OsString = clean.into_os_string();
    name.push(format!(".v{version}.bak"));
    PathBuf::from(name)
}

/// The format version of the document at `path`, read from its header only:
/// `Ok(None)` if there is no document there (nothing to back up), or a
/// `.formapackage` path that is not a package (the save refuses it).
///
/// # Errors
///
/// [`StoreError::Io`] when a `.forma` file is there but its version cannot
/// be read (a lock, a permission, a damaged header): the save must not
/// replace what it could not back up.
fn stored_version(path: &Path, format: Format) -> Result<Option<u32>, StoreError> {
    let unreadable = |cause: &dyn std::fmt::Display| {
        StoreError::Io(io::Error::other(format!(
            "cannot read the format version of '{}' to back it up before saving over it: \
             {cause}",
            path.display()
        )))
    };
    match format {
        Format::Forma => {
            match fs::metadata(path) {
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(err) => return Err(err.into()),
                Ok(meta) if meta.len() == 0 => return Ok(None),
                Ok(_) => {}
            }
            let conn = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|err| unreadable(&err))?;
            let version: i64 = conn
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .map_err(|err| unreadable(&err))?;
            Ok(u32::try_from(version).ok())
        }
        Format::Package => {
            // A folder without a readable FORMA manifest is not a package:
            // the save refuses it (`NotAFormaPackage`) and nothing is lost.
            let Ok(bytes) = fs::read(path.join("manifest.json")) else {
                return Ok(None);
            };
            let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) else {
                return Ok(None);
            };
            if manifest.get("format").and_then(Value::as_str) != Some("formapackage") {
                return Ok(None);
            }
            Ok(manifest
                .get("format_version")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok()))
        }
    }
}

/// Copies the folder `from` to `to`, which must not exist: into a temporary
/// sibling first, renamed into place at the end, so a failure leaves no
/// partial backup under the final name. Links are not followed (a package
/// has none; one that did would not be saved anyway).
fn copy_dir_atomic(from: &Path, to: &Path) -> io::Result<()> {
    let mut temp_name = to.as_os_str().to_os_string();
    temp_name.push(".tmp");
    let temp = PathBuf::from(temp_name);
    if temp.exists() {
        fs::remove_dir_all(&temp)?;
    }
    let result = copy_tree(from, &temp).and_then(|()| fs::rename(&temp, to));
    if result.is_err() {
        let _ = fs::remove_dir_all(&temp);
    }
    result
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
            fs::File::open(&target)?.sync_all()?;
        }
    }
    Ok(())
}

/// Before saving over `path`: if the document there was written by an older
/// format, copies it to [`backup_path`] unless that backup already exists.
/// Returns the backup written, if any.
///
/// # Errors
///
/// [`StoreError::Io`] if the copy fails; the save must then not go ahead,
/// so the old document stays untouched.
pub(crate) fn backup_if_older(path: &Path) -> Result<Option<PathBuf>, StoreError> {
    let format = format_of(path)?;
    let Some(version) = stored_version(path, format)? else {
        return Ok(None);
    };
    if version == 0 || version >= FORMAT_VERSION {
        return Ok(None);
    }
    let backup = backup_path(path, version);
    if backup.exists() {
        return Ok(None);
    }
    match format {
        Format::Forma => {
            let bytes = fs::read(path)?;
            if let Err(err) = write_file_atomic(&backup, &bytes) {
                remove_quietly(&backup);
                return Err(err.into());
            }
        }
        Format::Package => copy_dir_atomic(path, &backup)?,
    }
    sync_parent(&backup);
    Ok(Some(backup))
}
