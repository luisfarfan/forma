//! The `.formapackage` format: a folder of canonical JSON files meant for
//! version control.
//!
//! ```text
//! MyFont.formapackage/
//!   manifest.json     format, format_version, font_id, generator
//!   font.json         the font without its glyphs
//!   glyphs/
//!     a.json          one glyph per file (id and name inside)
//!     A_.json
//! ```
//!
//! # What a save guarantees
//!
//! A folder of files cannot change in one step, so a save is organized around
//! a commit point:
//!
//! 1. *Plan.* Each file the package should hold is compared with what is on
//!    disk; files that are already identical are left alone, so a typical save
//!    touches only the glyphs that changed.
//! 2. *Stage.* Every changed file is written to a temporary `*.tmp-<uuid>`
//!    file next to its target and flushed to disk. If anything fails here, all
//!    temporary files are deleted and the package has not been touched.
//! 3. *Commit.* The staged files are renamed over their targets in one burst:
//!    `manifest.json` first (if it changed), then the glyphs, and `font.json`
//!    last. Each rename is atomic. If one fails, the renames already done are
//!    rolled back (best effort) and the remaining temporary files are deleted.
//! 4. *Clean up.* The folders are flushed to disk, then the glyph files that
//!    no current glyph uses are deleted.
//!
//! A first save that fails removes the folders it created, and a folder left
//! by one that was killed (only temporary files and an empty `glyphs/`) is
//! accepted by the next save, which deletes the temporary files; so a failed
//! first save can always be retried.
//!
//! So an error (a full disk, a permission problem, a file in use) before the
//! commit leaves the previous package exactly as it was, and every file is
//! flushed before it becomes visible, so after a power failure each file holds
//! its old or its new contents, never a truncated mixture. What cannot be
//! guaranteed: a power failure (or a failed rollback) in the middle of the
//! commit burst, a window of a few milliseconds, can leave some files new and
//! some old. Because `font.json` comes last this is usually detected on load
//! (the glyph order no longer matches the glyphs), but not always: after an
//! edit that only changes outlines the mixture loads. A failure while deleting
//! stale glyph files is reported after the new package is complete; loading
//! rejects the leftover files until a new save removes them.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use forma_model::{Font, FontId};
use serde_json::{Value, json};

use crate::DiskEntry;
use crate::StoreError;
use crate::atomic::{
    create_synced, is_temp_name, remove_quietly, sync_dir, temp_path, write_file_atomic,
};
use crate::canonical::canonical_string;
use crate::filenames::{case_key, unique_file_name};
use crate::parts::{
    self, Encoded, FORMAT_VERSION, GENERATOR, assemble, decode_font_document, decode_glyph_document,
};

const MANIFEST: &str = "manifest.json";
const FONT: &str = "font.json";
const GLYPHS: &str = "glyphs";
const FORMAT: &str = "formapackage";

/// Reads a file, naming it in the error.
fn read_file(path: &Path) -> Result<Vec<u8>, StoreError> {
    fs::read(path).map_err(|err| {
        StoreError::Io(io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        ))
    })
}

/// Whether `name` is the name of a glyph document: it ends in `.json`.
fn is_glyph_file_name(name: &str) -> bool {
    name.as_bytes()
        .last_chunk::<5>()
        .is_some_and(|tail| tail.eq_ignore_ascii_case(b".json"))
}

/// The glyph documents in `glyphs_dir`, sorted by name: the regular files
/// whose names end in `.json` and do not start with `.`. Anything else in
/// the folder (editor leftovers, notes, temporary files of an interrupted
/// save, hidden files such as the AppleDouble `._a.json` companions macOS
/// writes on some file systems or `.DS_Store`) is not a glyph: it is neither
/// read nor deleted. Glyph file names never start with `.` (a leading `.` of a
/// glyph name becomes `_`). A folder that does not exist holds no glyphs:
/// version control does not keep empty folders, so a font without glyphs may
/// lose it.
fn glyph_files(glyphs_dir: &Path) -> Result<Vec<(String, PathBuf)>, StoreError> {
    let entries = match fs::read_dir(glyphs_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_glyph_file_name(&name) && !name.starts_with('.') {
            files.push((name, entry.path()));
        }
    }
    files.sort();
    Ok(files)
}

/// The size and modification time of every part of the package in `dir`
/// that exists (`manifest.json`, `font.json` and the glyph documents of
/// `glyphs/`), by path relative to the package, sorted; `None` if nothing
/// is at `dir`. A part that is a symbolic link is refused, as by a load or a
/// save: nothing is looked at through it. See [`crate::disk_state`].
pub(crate) fn disk_state(dir: &Path) -> Result<Option<Vec<DiskEntry>>, StoreError> {
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(err) if is_missing(&err) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    if !metadata.is_dir() {
        return Ok(Some(vec![DiskEntry::of(String::new(), &metadata)]));
    }
    ensure_no_links(dir)?;
    let mut entries = Vec::new();
    for name in [MANIFEST, FONT] {
        match fs::symlink_metadata(dir.join(name)) {
            Ok(metadata) => entries.push(DiskEntry::of(name.to_owned(), &metadata)),
            Err(err) if is_missing(&err) => {}
            Err(err) => return Err(err.into()),
        }
    }
    for (name, path) in glyph_files(&dir.join(GLYPHS))? {
        let metadata = fs::symlink_metadata(&path)?;
        entries.push(DiskEntry::of(format!("{GLYPHS}/{name}"), &metadata));
    }
    entries.sort();
    Ok(Some(entries))
}

/// Reads the manifest of the package in `dir`.
///
/// A folder without a readable manifest that says `"format": "formapackage"`
/// is [`StoreError::NotAFormaPackage`].
fn read_manifest(dir: &Path) -> Result<serde_json::Map<String, Value>, StoreError> {
    let not_a_package = || StoreError::NotAFormaPackage(dir.to_path_buf());
    let bytes = match fs::read(dir.join(MANIFEST)) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Err(not_a_package()),
        Err(err) => return Err(err.into()),
    };
    let Ok(Value::Object(manifest)) = serde_json::from_slice::<Value>(&bytes) else {
        return Err(not_a_package());
    };
    if manifest.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(not_a_package());
    }
    Ok(manifest)
}

/// The format version a manifest declares, if it declares one.
fn manifest_version(manifest: &serde_json::Map<String, Value>) -> Option<u64> {
    manifest.get("format_version").and_then(Value::as_u64)
}

/// Rejects a package written by a newer FORMA than this one.
fn ensure_version_is_supported(version: u64) -> Result<(), StoreError> {
    if version > u64::from(FORMAT_VERSION) {
        return Err(StoreError::UnsupportedVersion(
            u32::try_from(version).unwrap_or(u32::MAX),
        ));
    }
    Ok(())
}

/// Whether an error means that a path, or a folder on the way to it, does not
/// exist.
fn is_missing(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// Refuses `path` if it is a symbolic link (on Windows, also a junction).
/// A path that does not exist is fine.
fn refuse_link(path: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(StoreError::SymlinkInPackage(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(err) if is_missing(&err) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Refuses the package in `dir` if `manifest.json`, `font.json`, the `glyphs`
/// folder or any entry of `glyphs/` is a symbolic link, so that neither a load
/// nor a save ever reads, writes or deletes anything through a link: a link
/// could point outside the package (and outside the folders a caller allowed).
/// Parts that do not exist are fine.
///
/// The check runs before anything is read or written; a link created by
/// another process while the load or save is under way is not detected.
fn ensure_no_links(dir: &Path) -> Result<(), StoreError> {
    for name in [MANIFEST, FONT, GLYPHS] {
        refuse_link(&dir.join(name))?;
    }
    let entries = match fs::read_dir(dir.join(GLYPHS)) {
        Ok(entries) => entries,
        Err(err) if is_missing(&err) => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    for entry in entries {
        let entry = entry?;
        // `DirEntry::file_type` does not follow links.
        if entry.file_type()?.is_symlink() {
            return Err(StoreError::SymlinkInPackage(entry.path()));
        }
    }
    Ok(())
}

/// The temporary files in a folder that holds nothing but the debris of a
/// first save that was interrupted before any file became visible: temporary
/// files, and a `glyphs` folder that holds nothing but temporary files.
/// Returns `None` if there is anything else.
fn interrupted_first_save(dir: &Path) -> Result<Option<Vec<PathBuf>>, StoreError> {
    let is_temp_file = |entry: &fs::DirEntry| -> io::Result<bool> {
        Ok(entry.file_type()?.is_file() && is_temp_name(&entry.file_name().to_string_lossy()))
    };
    let mut temps = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if is_temp_file(&entry)? {
            temps.push(entry.path());
        } else if entry.file_type()?.is_dir() && entry.file_name() == GLYPHS {
            for inner in fs::read_dir(entry.path())? {
                let inner = inner?;
                if !is_temp_file(&inner)? {
                    return Ok(None);
                }
                temps.push(inner.path());
            }
        } else {
            return Ok(None);
        }
    }
    Ok(Some(temps))
}

/// Makes sure `dir` can receive a package, creating it if it is missing, and
/// returns whether it had to be created.
///
/// An existing folder is only used if it is empty, already a package, or the
/// remains of a first save that was interrupted (see
/// [`interrupted_first_save`]; its temporary files are deleted). So a save can
/// never scatter files into (or delete files from) a folder that belongs to
/// something else.
fn prepare_directory(dir: &Path) -> Result<bool, StoreError> {
    match fs::metadata(dir) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(dir)?;
            Ok(true)
        }
        Err(err) => Err(err.into()),
        Ok(metadata) if !metadata.is_dir() => Err(StoreError::NotAFormaPackage(dir.to_path_buf())),
        Ok(_) => {
            if fs::read_dir(dir)?.next().is_none() {
                return Ok(false);
            }
            match read_manifest(dir) {
                Ok(manifest) => {
                    // A package whose manifest has no usable version can still
                    // be saved over, so that saving repairs it; one written by
                    // a newer FORMA is refused, since saving would downgrade
                    // it.
                    manifest_version(&manifest).map_or(Ok(()), ensure_version_is_supported)?;
                }
                Err(StoreError::NotAFormaPackage(_)) => {
                    let temps = interrupted_first_save(dir)?
                        .ok_or_else(|| StoreError::NotAFormaPackage(dir.to_path_buf()))?;
                    for temp in temps {
                        fs::remove_file(temp)?;
                    }
                }
                Err(err) => return Err(err),
            }
            Ok(false)
        }
    }
}

/// The text of `manifest.json`.
fn manifest_text(font_id: FontId) -> Result<String, StoreError> {
    let manifest = json!({
        "format": FORMAT,
        "format_version": FORMAT_VERSION,
        "font_id": font_id,
        "generator": GENERATOR,
    });
    Ok(canonical_string(&manifest)?)
}

/// Deletes the glyph documents of `glyphs_dir` that this save did not write.
///
/// `written` are the file names just written. File systems that ignore case
/// complicate this: if `a_.json` existed and this save wrote `A_.json`, the
/// disk holds one file, still listed under its old spelling, and it must not
/// be deleted. When both spellings are listed the file system tells them
/// apart, and the one not written is stale.
///
/// Returns whether any file was deleted.
fn remove_stale_glyph_files(glyphs_dir: &Path, written: &[String]) -> Result<bool, StoreError> {
    let written_exact: HashSet<&str> = written.iter().map(String::as_str).collect();
    let written_by_key: HashMap<String, &str> = written
        .iter()
        .map(|name| (case_key(name), name.as_str()))
        .collect();

    let files = glyph_files(glyphs_dir)?;
    let listed: HashSet<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    let mut removed = false;
    for (name, path) in &files {
        if written_exact.contains(name.as_str()) {
            continue;
        }
        let same_file_as_written = written_by_key
            .get(&case_key(name))
            .is_some_and(|written_name| !listed.contains(written_name));
        if !same_file_as_written {
            fs::remove_file(path)?;
            removed = true;
        }
    }
    Ok(removed)
}

/// The file operations of a save, behind a trait so that tests can make them
/// fail.
trait Files {
    /// Creates a new file at `path` holding `contents`, flushed to disk. The
    /// path does not exist yet.
    fn create(&mut self, path: &Path, contents: &[u8]) -> io::Result<()>;

    /// Moves `from` over `to`, replacing it atomically.
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()>;
}

/// The real file system.
struct RealFiles;

impl Files for RealFiles {
    fn create(&mut self, path: &Path, contents: &[u8]) -> io::Result<()> {
        create_synced(path, contents)
    }

    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }
}

/// A file that a save has to write because it differs from the disk.
struct Change {
    target: PathBuf,
    contents: Vec<u8>,
    /// What the file holds now, or `None` if it does not exist.
    previous: Option<Vec<u8>>,
    /// Where the new contents are staged before they replace the target.
    temp: PathBuf,
}

/// Compares the files a package should hold, given in the order they are to
/// be committed, with the disk, and returns those that need writing.
fn plan_changes(wanted: Vec<(PathBuf, Vec<u8>)>) -> Result<Vec<Change>, StoreError> {
    let mut changes = Vec::new();
    for (target, contents) in wanted {
        let previous = match fs::read(&target) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => {
                return Err(StoreError::Io(io::Error::new(
                    err.kind(),
                    format!("{}: {err}", target.display()),
                )));
            }
        };
        if previous.as_deref() == Some(contents.as_slice()) {
            continue;
        }
        let temp = temp_path(&target)?;
        changes.push(Change {
            target,
            contents,
            previous,
            temp,
        });
    }
    Ok(changes)
}

/// Writes and flushes every changed file to its temporary name. On failure
/// all temporary files are deleted and nothing else has been touched.
fn stage(files: &mut impl Files, changes: &[Change]) -> io::Result<()> {
    for (n, change) in changes.iter().enumerate() {
        if let Err(err) = files.create(&change.temp, &change.contents) {
            // The failed file may be half written; remove it with the rest.
            for staged in &changes[..=n] {
                remove_quietly(&staged.temp);
            }
            return Err(err);
        }
    }
    Ok(())
}

/// Renames the staged files over their targets, in order. If a rename fails
/// the earlier ones are undone (best effort) and the remaining temporary
/// files are deleted.
fn commit(files: &mut impl Files, changes: &[Change]) -> io::Result<()> {
    for (n, change) in changes.iter().enumerate() {
        if let Err(err) = files.rename(&change.temp, &change.target) {
            for pending in &changes[n..] {
                remove_quietly(&pending.temp);
            }
            roll_back(&changes[..n]);
            return Err(err);
        }
    }
    Ok(())
}

/// Puts back what the already renamed files held, newest first. Errors are
/// ignored: the original failure is the one to report, and there is nothing
/// further to try.
fn roll_back(done: &[Change]) {
    for change in done.iter().rev() {
        match &change.previous {
            Some(previous) => {
                let _ = write_file_atomic(&change.target, previous);
            }
            None => {
                let _ = fs::remove_file(&change.target);
            }
        }
    }
}

/// Writes `font` as a package in `dir`.
pub(crate) fn save(font: &Font, dir: &Path) -> Result<(), StoreError> {
    save_with(font, dir, &mut RealFiles)
}

/// [`save`] with the given file operations.
fn save_with(font: &Font, dir: &Path, files: &mut impl Files) -> Result<(), StoreError> {
    // Validate and render everything before touching the disk.
    let Encoded {
        font_id,
        font_json,
        glyphs,
    } = parts::encode(font)?;
    let manifest = manifest_text(font_id)?;

    let glyphs_dir = dir.join(GLYPHS);

    // Everything the package should hold, in commit order: the manifest, the
    // glyphs, and font.json last. Glyphs get their file names in identifier
    // order, which never changes for a glyph: renaming one glyph cannot move
    // another glyph's file.
    let mut wanted = Vec::with_capacity(glyphs.len() + 2);
    wanted.push((dir.join(MANIFEST), manifest.into_bytes()));
    let mut taken = HashSet::new();
    let mut written = Vec::with_capacity(glyphs.len());
    for glyph in glyphs {
        let file_name = unique_file_name(&glyph.name, |candidate| {
            taken.contains(&case_key(candidate))
        });
        taken.insert(case_key(&file_name));
        wanted.push((glyphs_dir.join(&file_name), glyph.json.into_bytes()));
        written.push(file_name);
    }
    wanted.push((dir.join(FONT), font_json.into_bytes()));

    ensure_no_links(dir)?;
    let created_dir = prepare_directory(dir)?;
    let created_glyphs_dir = !glyphs_dir.exists();
    let result = fs::create_dir_all(&glyphs_dir)
        .map_err(StoreError::from)
        .and_then(|()| write_package(files, dir, &glyphs_dir, wanted, &written));

    if result.is_err() {
        // Do not leave the folders this save created behind: an empty
        // `glyphs/` or package folder is not a package and would be in the way
        // of the next attempt. `remove_dir` only removes empty folders, so
        // nothing that holds files is touched, and nothing that was there
        // before this save is removed.
        if created_glyphs_dir {
            let _ = fs::remove_dir(&glyphs_dir);
        }
        if created_dir {
            let _ = fs::remove_dir(dir);
        }
    }
    result
}

/// Plans, stages and commits `wanted` (see the module documentation), then
/// deletes the glyph files that are not in `written`.
fn write_package(
    files: &mut impl Files,
    dir: &Path,
    glyphs_dir: &Path,
    wanted: Vec<(PathBuf, Vec<u8>)>,
    written: &[String],
) -> Result<(), StoreError> {
    let changes = plan_changes(wanted)?;
    stage(files, &changes)?;
    commit(files, &changes)?;
    if !changes.is_empty() {
        sync_dir(glyphs_dir);
        sync_dir(dir);
    }

    // Only a complete new package may delete anything.
    if remove_stale_glyph_files(glyphs_dir, written)? {
        sync_dir(glyphs_dir);
    }
    Ok(())
}

/// Reads the package in `dir`.
pub(crate) fn load(dir: &Path) -> Result<Font, StoreError> {
    if !fs::metadata(dir)?.is_dir() {
        return Err(StoreError::NotAFormaPackage(dir.to_path_buf()));
    }
    ensure_no_links(dir)?;
    let manifest = read_manifest(dir)?;
    let version = manifest_version(&manifest)
        .filter(|&version| version > 0)
        .ok_or_else(|| {
            StoreError::Invalid(format!(
                "{MANIFEST}: format_version must be a positive integer"
            ))
        })?;
    ensure_version_is_supported(version)?;

    let font = decode_font_document(&read_file(&dir.join(FONT))?, FONT)?;

    let manifest_font_id = manifest
        .get("font_id")
        .and_then(Value::as_str)
        .and_then(|id| id.parse::<FontId>().ok())
        .ok_or_else(|| StoreError::Invalid(format!("{MANIFEST}: font_id is missing or invalid")))?;
    if manifest_font_id != font.id {
        return Err(StoreError::Invalid(format!(
            "{MANIFEST} is for font {manifest_font_id} but {FONT} holds font {}",
            font.id
        )));
    }

    let mut glyphs = Vec::new();
    for (name, path) in glyph_files(&dir.join(GLYPHS))? {
        let context = format!("{GLYPHS}/{name}");
        glyphs.push(decode_glyph_document(&read_file(&path)?, &context)?);
    }

    assemble(font, glyphs)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use forma_model::{Glyph, GlyphId, Layer, Node, NodeKind, Path as Contour, PathId};
    use tempfile::TempDir;
    use uuid::Uuid;

    use super::*;

    /// File operations that count what a save does and fail on demand.
    #[derive(Default)]
    struct Flaky {
        /// Fail the n-th file creation (counting from 0) after writing half
        /// of it, like a disk that fills up.
        fail_create_at: Option<usize>,
        /// Fail the n-th rename (counting from 0).
        fail_rename_at: Option<usize>,
        creates: usize,
        rename_attempts: usize,
        /// Names of the files that were renamed into place, in order.
        renamed: Vec<String>,
    }

    impl Files for Flaky {
        fn create(&mut self, path: &Path, contents: &[u8]) -> io::Result<()> {
            let n = self.creates;
            self.creates += 1;
            if self.fail_create_at == Some(n) {
                fs::write(path, &contents[..contents.len() / 2])?;
                return Err(io::Error::other("injected create failure"));
            }
            RealFiles.create(path, contents)
        }

        fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
            let n = self.rename_attempts;
            self.rename_attempts += 1;
            if self.fail_rename_at == Some(n) {
                return Err(io::Error::other("injected rename failure"));
            }
            self.renamed
                .push(to.file_name().unwrap().to_string_lossy().into_owned());
            RealFiles.rename(from, to)
        }
    }

    fn square(advance_width: f64) -> Layer {
        let corner = |x: f64, y: f64| Node {
            x,
            y,
            kind: NodeKind::Line,
            smooth: false,
        };
        Layer {
            advance_width,
            paths: vec![Contour {
                id: PathId(1),
                closed: true,
                nodes: vec![corner(0.0, 0.0), corner(500.0, 0.0), corner(500.0, 500.0)],
            }],
            components: Vec::new(),
        }
    }

    /// `base` with exactly the given `(id, name, advance width)` glyphs.
    fn with_glyphs(base: &Font, glyphs: &[(u128, &str, f64)]) -> Font {
        let mut font = base.clone();
        let master = font.masters[0].id;
        let map = glyphs
            .iter()
            .map(|&(id, name, width)| {
                let id = GlyphId(Uuid::from_u128(id));
                let glyph = Glyph {
                    id,
                    name: name.to_owned(),
                    unicodes: Vec::new(),
                    export: true,
                    layers: BTreeMap::from([(master, square(width))]),
                };
                (id, Arc::new(glyph))
            })
            .collect();
        font.glyph_order = glyphs
            .iter()
            .map(|&(id, ..)| GlyphId(Uuid::from_u128(id)))
            .collect();
        font.glyphs = Arc::new(map);
        font
    }

    /// Everything under `dir`, by relative path: files with their contents,
    /// folders (marked by a trailing `/`) with empty contents.
    fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if path.is_dir() {
                    into.insert(format!("{relative}/"), Vec::new());
                    walk(root, &path, into);
                } else {
                    into.insert(relative, fs::read(&path).unwrap());
                }
            }
        }
        let mut files = BTreeMap::new();
        walk(dir, dir, &mut files);
        files
    }

    fn assert_no_temp_files(files: &BTreeMap<String, Vec<u8>>, context: &str) {
        assert!(
            files.keys().all(|name| !name.contains(".tmp-")),
            "{context}: temporary files left behind: {:?}",
            files.keys().collect::<Vec<_>>()
        );
    }

    /// Pairs of (old, new) fonts covering the ways a save can differ from what
    /// is on disk.
    fn scenarios() -> Vec<(&'static str, Font, Font)> {
        let base = Font::new("Scenario", 1000);
        let old = with_glyphs(&base, &[(1, "a", 600.0), (2, "b", 600.0), (3, "c", 600.0)]);
        let mut everything =
            with_glyphs(&base, &[(1, "A", 700.0), (2, "B", 700.0), (3, "c", 700.0)]);
        everything.info.family_name = "Renamed family".to_owned();
        vec![
            (
                // Glyphs renamed: some files get new names, old ones go stale.
                "rename",
                old.clone(),
                with_glyphs(&base, &[(1, "A", 600.0), (2, "B", 600.0), (3, "c", 600.0)]),
            ),
            (
                // Only outlines change: same file names, same font.json.
                "outline only",
                old.clone(),
                with_glyphs(&base, &[(1, "a", 650.0), (2, "b", 650.0), (3, "c", 650.0)]),
            ),
            (
                // A glyph removed and another added: font.json changes too.
                "add and remove",
                old.clone(),
                with_glyphs(&base, &[(2, "b", 600.0), (3, "c", 600.0), (4, "d", 600.0)]),
            ),
            ("everything", old, everything),
        ]
    }

    #[test]
    fn failure_while_staging_leaves_the_old_package_untouched() {
        for (name, old, new) in scenarios() {
            let mut failures = 0;
            for n in 0.. {
                let dir = TempDir::new().unwrap();
                let package = dir.path().join("font.formapackage");
                save(&old, &package).unwrap();
                let before = snapshot(&package);

                let mut files = Flaky {
                    fail_create_at: Some(n),
                    ..Flaky::default()
                };
                match save_with(&new, &package, &mut files) {
                    // Past the last file that needed staging.
                    Ok(()) => {
                        assert_eq!(load(&package).unwrap(), new, "{name}");
                        break;
                    }
                    Err(err) => {
                        failures += 1;
                        assert!(matches!(err, StoreError::Io(_)), "{name}: {err:?}");
                        let after = snapshot(&package);
                        assert_no_temp_files(&after, &format!("{name}, create #{n}"));
                        assert_eq!(after, before, "{name}: create #{n} changed the package");
                        assert_eq!(load(&package).unwrap(), old, "{name}, create #{n}");
                    }
                }
            }
            assert!(failures >= 2, "{name}: only {failures} files were staged");
        }
    }

    #[test]
    fn failure_while_renaming_is_rolled_back() {
        for (name, old, new) in scenarios() {
            let mut failures = 0;
            for n in 0.. {
                let dir = TempDir::new().unwrap();
                let package = dir.path().join("font.formapackage");
                save(&old, &package).unwrap();
                let before = snapshot(&package);

                let mut files = Flaky {
                    fail_rename_at: Some(n),
                    ..Flaky::default()
                };
                match save_with(&new, &package, &mut files) {
                    Ok(()) => {
                        assert_eq!(load(&package).unwrap(), new, "{name}");
                        break;
                    }
                    Err(err) => {
                        failures += 1;
                        assert!(matches!(err, StoreError::Io(_)), "{name}: {err:?}");
                        let after = snapshot(&package);
                        assert_no_temp_files(&after, &format!("{name}, rename #{n}"));
                        assert_eq!(after, before, "{name}: rename #{n} changed the package");
                        assert_eq!(load(&package).unwrap(), old, "{name}, rename #{n}");
                    }
                }
            }
            assert!(failures >= 2, "{name}: only {failures} files were renamed");
        }
    }

    /// Runs a first save of `font` into `package` that fails at the `n`-th
    /// staged file (`stage`) or at the `n`-th rename; `None` if there is no
    /// such file or rename, so the save succeeded.
    fn failing_first_save(font: &Font, package: &Path, stage: bool, n: usize) -> Option<()> {
        let mut files = if stage {
            Flaky {
                fail_create_at: Some(n),
                ..Flaky::default()
            }
        } else {
            Flaky {
                fail_rename_at: Some(n),
                ..Flaky::default()
            }
        };
        save_with(font, package, &mut files).err().map(|_| ())
    }

    #[test]
    fn a_failed_first_save_leaves_nothing_it_created() {
        let font = scenarios().remove(0).1;
        for stage in [true, false] {
            let mut failures = 0;
            for n in 0.. {
                // The package folder did not exist: it must not exist after.
                let dir = TempDir::new().unwrap();
                let package = dir.path().join("font.formapackage");
                if failing_first_save(&font, &package, stage, n).is_none() {
                    break;
                }
                failures += 1;
                assert!(
                    !package.exists(),
                    "stage {stage}, #{n}: {:?}",
                    snapshot(&package)
                );

                // The user had made an empty folder: it is left as it was.
                let dir = TempDir::new().unwrap();
                let package = dir.path().join("font.formapackage");
                fs::create_dir(&package).unwrap();
                assert!(failing_first_save(&font, &package, stage, n).is_some());
                assert!(snapshot(&package).is_empty(), "stage {stage}, #{n}");
            }
            assert!(
                failures >= 5,
                "stage {stage}: only {failures} failure points"
            );
        }
    }

    #[test]
    fn a_failed_first_save_can_be_retried() {
        let font = scenarios().remove(0).1;
        for stage in [true, false] {
            for precreated in [false, true] {
                for n in 0.. {
                    let dir = TempDir::new().unwrap();
                    let package = dir.path().join("font.formapackage");
                    if precreated {
                        fs::create_dir(&package).unwrap();
                    }
                    if failing_first_save(&font, &package, stage, n).is_none() {
                        break;
                    }

                    save(&font, &package).unwrap();

                    let context = format!("stage {stage}, precreated {precreated}, #{n}");
                    assert_eq!(load(&package).unwrap(), font, "{context}");
                    assert_no_temp_files(&snapshot(&package), &context);
                }
            }
        }
    }

    /// The leftovers of a first save that was killed while staging: the
    /// folder holds an empty `glyphs/` and temporary files, but no manifest.
    fn interrupted_first_save(package: &Path) -> Vec<PathBuf> {
        let stray = |name: &str, n: u128| format!("{name}.tmp-{}", Uuid::from_u128(n));
        fs::create_dir_all(package.join(GLYPHS)).unwrap();
        let temps = [
            package.join(stray(MANIFEST, 1)),
            package.join(stray(FONT, 2)),
            package.join(GLYPHS).join(stray("a.json", 3)),
            package.join(GLYPHS).join(stray("b.json", 4)),
        ];
        for temp in &temps {
            fs::write(temp, b"half a fi").unwrap();
        }
        temps.to_vec()
    }

    #[test]
    fn a_folder_left_by_an_interrupted_first_save_is_accepted_and_cleaned() {
        let font = scenarios().remove(0).1;
        let dir = TempDir::new().unwrap();
        let package = dir.path().join("font.formapackage");
        let temps = interrupted_first_save(&package);

        save(&font, &package).unwrap();

        assert!(temps.iter().all(|temp| !temp.exists()));
        assert_no_temp_files(&snapshot(&package), "after the retry");
        assert_eq!(load(&package).unwrap(), font);
    }

    #[test]
    fn an_empty_glyphs_folder_alone_is_accepted() {
        let font = scenarios().remove(0).1;
        let dir = TempDir::new().unwrap();
        let package = dir.path().join("font.formapackage");
        fs::create_dir_all(package.join(GLYPHS)).unwrap();

        save(&font, &package).unwrap();

        assert_eq!(load(&package).unwrap(), font);
    }

    #[test]
    fn a_folder_with_anything_else_in_it_is_still_refused() {
        let font = scenarios().remove(0).1;
        let uuid = Uuid::from_u128(7);
        // (relative path of the extra file, its contents)
        let extras = [
            ("notes.txt", "precious"),
            (".DS_Store", "junk"),
            ("glyphs/notes.txt", "precious"),
            // Real glyph data without a manifest is not ours to overwrite.
            ("glyphs/a.json", "{}"),
            // Not the name of one of our temporary files.
            ("manifest.json.tmp-not-a-uuid", "x"),
            ("font.json.tmp-", "x"),
            ("tmp-7", "x"),
        ];
        for (relative, contents) in extras {
            let dir = TempDir::new().unwrap();
            let package = dir.path().join("font.formapackage");
            let temps = interrupted_first_save(&package);
            fs::write(package.join(relative), contents).unwrap();
            let before = snapshot(&package);

            let err = save(&font, &package).unwrap_err();

            assert!(
                matches!(err, StoreError::NotAFormaPackage(_)),
                "{relative}: {err:?}"
            );
            // Nothing was deleted, not even the temporary files.
            assert_eq!(snapshot(&package), before, "{relative}");
            assert!(temps.iter().all(|temp| temp.exists()), "{relative}");
        }
        // A temporary name needs a real UUID after `.tmp-`.
        assert!(is_temp_name(&format!("x.json.tmp-{uuid}")));
        assert!(!is_temp_name("x.json.tmp-7"));
    }

    #[test]
    fn a_temporary_folder_entry_that_is_a_directory_is_not_debris() {
        let font = scenarios().remove(0).1;
        let dir = TempDir::new().unwrap();
        let package = dir.path().join("font.formapackage");
        interrupted_first_save(&package);
        let folder = package.join(format!("sub.tmp-{}", Uuid::from_u128(9)));
        fs::create_dir(&folder).unwrap();

        let err = save(&font, &package).unwrap_err();

        assert!(matches!(err, StoreError::NotAFormaPackage(_)), "{err:?}");
        assert!(folder.is_dir());
    }

    #[test]
    fn saving_an_unchanged_font_writes_nothing() {
        let (_, font, _) = scenarios().remove(0);
        let dir = TempDir::new().unwrap();
        let package = dir.path().join("font.formapackage");
        save(&font, &package).unwrap();
        let before = snapshot(&package);

        let mut files = Flaky::default();
        save_with(&font, &package, &mut files).unwrap();

        assert_eq!(files.creates, 0);
        assert_eq!(files.rename_attempts, 0);
        assert_eq!(snapshot(&package), before);
    }

    #[test]
    fn only_changed_files_are_written() {
        let base = Font::new("Changed", 1000);
        let old = with_glyphs(&base, &[(1, "a", 600.0), (2, "b", 600.0), (3, "c", 600.0)]);
        let new = with_glyphs(&base, &[(1, "a", 600.0), (2, "b", 601.0), (3, "c", 600.0)]);
        let dir = TempDir::new().unwrap();
        let package = dir.path().join("font.formapackage");
        save(&old, &package).unwrap();

        let mut files = Flaky::default();
        save_with(&new, &package, &mut files).unwrap();

        // Only the glyph `b` changed: not the manifest, not font.json.
        assert_eq!(files.creates, 1);
        assert_eq!(files.renamed, ["b.json"]);
        assert_eq!(load(&package).unwrap(), new);
    }

    #[test]
    fn the_manifest_is_renamed_first_and_font_json_last() {
        let (_, old, mut new) = scenarios().remove(3);
        let dir = TempDir::new().unwrap();
        let package = dir.path().join("font.formapackage");
        save(&old, &package).unwrap();
        // A different font id changes the manifest as well as everything else.
        new.id = forma_model::FontId(Uuid::from_u128(0xF0F0));

        let mut files = Flaky::default();
        save_with(&new, &package, &mut files).unwrap();

        assert_eq!(
            files.renamed.first().map(String::as_str),
            Some("manifest.json")
        );
        assert_eq!(files.renamed.last().map(String::as_str), Some("font.json"));
        assert_eq!(files.renamed.len(), 5, "{:?}", files.renamed);
        assert_eq!(load(&package).unwrap(), new);
    }

    #[test]
    fn stale_files_survive_a_save_that_fails_before_the_commit() {
        // `a.json` is stale after the rename of glyph `a`, but only a
        // finished save may delete it.
        let (_, old, new) = scenarios().remove(0);
        let dir = TempDir::new().unwrap();
        let package = dir.path().join("font.formapackage");
        save(&old, &package).unwrap();

        let mut files = Flaky {
            fail_create_at: Some(1),
            ..Flaky::default()
        };
        assert!(save_with(&new, &package, &mut files).is_err());

        assert!(package.join("glyphs").join("a.json").is_file());
    }
}
