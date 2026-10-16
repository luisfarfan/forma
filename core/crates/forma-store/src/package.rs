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
//! Every file is replaced atomically, but a folder of files cannot change in
//! one step: a save that is interrupted halfway can leave new glyph files next
//! to an old `font.json`. Such a package is rejected when loaded (the glyph
//! order no longer matches the glyphs) and saving again repairs it.
//!
//! Files are not flushed to the storage device ([`Flush::Lazy`]): that costs
//! milliseconds per glyph, and a package is the interchange and version
//! control format, whose history lives in Git. The `.forma` file, the working
//! format, is flushed completely.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use forma_model::{Font, FontId};
use serde_json::{Value, json};

use crate::StoreError;
use crate::atomic::{Flush, write_file_atomic};
use crate::canonical::canonical_string;
use crate::filenames::{case_key, unique_file_name};
use crate::parts::{
    self, FORMAT_VERSION, GENERATOR, assemble, decode_font_document, decode_glyph_document,
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
/// whose names end in `.json`. Anything else in the folder (editor leftovers,
/// notes, temporary files of an interrupted save) is not a glyph. A folder
/// that does not exist holds no glyphs: version control does not keep empty
/// folders, so a font without glyphs may lose it.
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
        if is_glyph_file_name(&name) {
            files.push((name, entry.path()));
        }
    }
    files.sort();
    Ok(files)
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

/// Makes sure `dir` can receive a package, creating it if it is missing.
///
/// An existing folder is only used if it is empty or already a package, so a
/// save can never scatter files into (or delete files from) a folder that
/// belongs to something else.
fn prepare_directory(dir: &Path) -> Result<(), StoreError> {
    match fs::metadata(dir) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(dir)?;
            Ok(())
        }
        Err(err) => Err(err.into()),
        Ok(metadata) if !metadata.is_dir() => Err(StoreError::NotAFormaPackage(dir.to_path_buf())),
        Ok(_) => {
            if fs::read_dir(dir)?.next().is_none() {
                return Ok(());
            }
            // A package whose manifest has no usable version can still be saved
            // over, so that saving repairs it; one written by a newer FORMA is
            // refused, since saving would downgrade it.
            let manifest = read_manifest(dir)?;
            manifest_version(&manifest).map_or(Ok(()), ensure_version_is_supported)
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
fn remove_stale_glyph_files(glyphs_dir: &Path, written: &[String]) -> Result<(), StoreError> {
    let written_exact: HashSet<&str> = written.iter().map(String::as_str).collect();
    let written_by_key: HashMap<String, &str> = written
        .iter()
        .map(|name| (case_key(name), name.as_str()))
        .collect();

    let files = glyph_files(glyphs_dir)?;
    let listed: HashSet<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    for (name, path) in &files {
        if written_exact.contains(name.as_str()) {
            continue;
        }
        let same_file_as_written = written_by_key
            .get(&case_key(name))
            .is_some_and(|written_name| !listed.contains(written_name));
        if !same_file_as_written {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// Writes `font` as a package in `dir`.
pub(crate) fn save(font: &Font, dir: &Path) -> Result<(), StoreError> {
    // Validate and render everything before touching the disk.
    let encoded = parts::encode(font)?;
    let manifest = manifest_text(encoded.font_id)?;

    prepare_directory(dir)?;
    let glyphs_dir = dir.join(GLYPHS);
    fs::create_dir_all(&glyphs_dir)?;

    // The manifest goes first, so that an interrupted first save leaves a
    // folder that is recognizably a package and can be saved over.
    write_file_atomic(&dir.join(MANIFEST), Flush::Lazy, manifest.as_bytes())?;

    // Glyphs get their file names in identifier order, which never changes
    // for a glyph: renaming one glyph cannot move another glyph's file.
    let mut taken = HashSet::new();
    let mut written = Vec::with_capacity(encoded.glyphs.len());
    for glyph in &encoded.glyphs {
        let file_name = unique_file_name(&glyph.name, |candidate| {
            taken.contains(&case_key(candidate))
        });
        taken.insert(case_key(&file_name));
        write_file_atomic(
            &glyphs_dir.join(&file_name),
            Flush::Lazy,
            glyph.json.as_bytes(),
        )?;
        written.push(file_name);
    }

    write_file_atomic(&dir.join(FONT), Flush::Lazy, encoded.font_json.as_bytes())?;
    remove_stale_glyph_files(&glyphs_dir, &written)
}

/// Reads the package in `dir`.
pub(crate) fn load(dir: &Path) -> Result<Font, StoreError> {
    if !fs::metadata(dir)?.is_dir() {
        return Err(StoreError::NotAFormaPackage(dir.to_path_buf()));
    }
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
