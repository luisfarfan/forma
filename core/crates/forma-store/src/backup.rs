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

/// The path of the backup of the format `version` document at `path`.
#[must_use]
pub fn backup_path(path: &Path, version: u32) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_os_string();
    name.push(format!(".v{version}.bak"));
    PathBuf::from(name)
}

/// The format version of the document at `path`, read from its header only;
/// `None` if there is no document there, or it cannot be identified (the
/// save that follows reports that).
fn stored_version(path: &Path) -> Option<u32> {
    match format_of(path).ok()? {
        Format::Forma => {
            if !path.is_file() {
                return None;
            }
            let conn = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .ok()?;
            let version: i64 = conn
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .ok()?;
            u32::try_from(version).ok()
        }
        Format::Package => {
            let bytes = fs::read(path.join("manifest.json")).ok()?;
            let manifest: Value = serde_json::from_slice(&bytes).ok()?;
            u32::try_from(manifest.get("format_version")?.as_u64()?).ok()
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
    let Some(version) = stored_version(path) else {
        return Ok(None);
    };
    if version == 0 || version >= FORMAT_VERSION {
        return Ok(None);
    }
    let backup = backup_path(path, version);
    if backup.exists() {
        return Ok(None);
    }
    match format_of(path)? {
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
