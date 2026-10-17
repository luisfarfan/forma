//! The `.forma` format: one SQLite database.
//!
//! ```sql
//! PRAGMA application_id = 0x464F524D;  -- "FORM"
//! PRAGMA user_version   = 1;           -- format version
//! PRAGMA journal_mode   = DELETE;      -- one self-contained file, no -wal
//!
//! CREATE TABLE manifest (key TEXT PRIMARY KEY, value TEXT NOT NULL);
//! CREATE TABLE entities (
//!     kind TEXT NOT NULL,              -- 'font' or 'glyph'
//!     id   TEXT NOT NULL,              -- the font's or the glyph's UUID
//!     json TEXT NOT NULL,              -- canonical JSON, as in a package
//!     PRIMARY KEY (kind, id)
//! );
//! ```
//!
//! `manifest` holds `format` (`forma`), `format_version`, `font_id` and
//! `generator`. `entities` holds one `font` row (the font without its glyphs)
//! and one `glyph` row per glyph.
//!
//! The database is built from scratch in a temporary file in a single
//! transaction and renamed over the target, so saving never edits an existing
//! `.forma` in place.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use forma_model::{Font, FontId};
use rusqlite::{Connection, ErrorCode, OpenFlags, params};

use crate::StoreError;
use crate::atomic::{remove_quietly, write_atomic_at};
use crate::parts::{
    self, Encoded, FORMAT_VERSION, GENERATOR, assemble, decode_font_document, decode_glyph_document,
};

/// `application_id` of every `.forma` file: the ASCII bytes `FORM`.
const APPLICATION_ID: i32 = 0x464F_524D;

const FORMAT: &str = "forma";

const SCHEMA: &str = "
CREATE TABLE manifest (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE entities (
    kind TEXT NOT NULL,
    id TEXT NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (kind, id)
);
";

/// Opening flags that never reinterpret the path as a URI.
const WRITE_FLAGS: OpenFlags = OpenFlags::SQLITE_OPEN_READ_WRITE
    .union(OpenFlags::SQLITE_OPEN_CREATE)
    .union(OpenFlags::SQLITE_OPEN_NO_MUTEX);
const READ_FLAGS: OpenFlags =
    OpenFlags::SQLITE_OPEN_READ_ONLY.union(OpenFlags::SQLITE_OPEN_NO_MUTEX);

/// Writes `font` to `path`, replacing any file there atomically.
pub(crate) fn save(font: &Font, path: &Path) -> Result<(), StoreError> {
    // Validate and render everything before touching the disk.
    let encoded = parts::encode(font)?;
    write_atomic_at(path, |temp| build_database(temp, &encoded))
}

/// Builds the complete database at `temp`, which must not exist yet.
fn build_database(temp: &Path, encoded: &Encoded) -> Result<(), StoreError> {
    let result = build_database_inner(temp, encoded);
    if result.is_err() {
        // The connection is closed by now; a failed transaction leaves no
        // journal, but make sure none is left behind.
        remove_quietly(&journal_path(temp));
    }
    result
}

fn journal_path(database: &Path) -> PathBuf {
    let mut name = database.as_os_str().to_os_string();
    name.push("-journal");
    PathBuf::from(name)
}

fn build_database_inner(temp: &Path, encoded: &Encoded) -> Result<(), StoreError> {
    let mut conn = Connection::open_with_flags(temp, WRITE_FLAGS)?;
    let journal_mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("delete") {
        return Err(StoreError::Invalid(format!(
            "SQLite refused journal_mode = DELETE (it is {journal_mode})"
        )));
    }

    // The file is flushed to disk as a whole before it replaces the target
    // (see `write_atomic_at`), so SQLite need not do it at every step of building it.
    conn.pragma_update(None, "synchronous", "OFF")?;

    {
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA)?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
        tx.pragma_update(None, "user_version", FORMAT_VERSION)?;

        let mut manifest = tx.prepare("INSERT INTO manifest (key, value) VALUES (?1, ?2)")?;
        for (key, value) in [
            ("format", FORMAT.to_owned()),
            ("format_version", FORMAT_VERSION.to_string()),
            ("font_id", encoded.font_id.to_string()),
            ("generator", GENERATOR.to_owned()),
        ] {
            manifest.execute(params![key, value])?;
        }
        drop(manifest);

        let mut entity = tx.prepare("INSERT INTO entities (kind, id, json) VALUES (?1, ?2, ?3)")?;
        entity.execute(params![
            "font",
            encoded.font_id.to_string(),
            encoded.font_json
        ])?;
        for glyph in &encoded.glyphs {
            entity.execute(params!["glyph", glyph.id.to_string(), glyph.json])?;
        }
        drop(entity);

        tx.commit()?;
    }
    conn.close().map_err(|(_, err)| err)?;
    Ok(())
}

/// Maps "this is not an SQLite database" to [`StoreError::NotAFormaFile`].
fn not_a_database(path: &Path, err: rusqlite::Error) -> StoreError {
    match &err {
        rusqlite::Error::SqliteFailure(failure, _) if failure.code == ErrorCode::NotADatabase => {
            StoreError::NotAFormaFile(path.to_path_buf())
        }
        _ => StoreError::Sqlite(err),
    }
}

/// Reads the database at `path`.
pub(crate) fn load(path: &Path) -> Result<Font, StoreError> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(StoreError::NotAFormaFile(path.to_path_buf()));
    }
    let conn =
        Connection::open_with_flags(path, READ_FLAGS).map_err(|err| not_a_database(path, err))?;

    let application_id: i32 = conn
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|err| not_a_database(path, err))?;
    if application_id != APPLICATION_ID {
        return Err(StoreError::NotAFormaFile(path.to_path_buf()));
    }
    let user_version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match u32::try_from(user_version) {
        Ok(FORMAT_VERSION) => {}
        Ok(0) | Err(_) => {
            return Err(StoreError::Invalid(format!(
                "user_version {user_version} is not a format version"
            )));
        }
        Ok(newer) => return Err(StoreError::UnsupportedVersion(newer)),
    }

    let manifest = read_manifest(&conn)?;
    let (font_row, glyph_rows) = read_entities(&conn)?;

    let (font_row_id, font_json) = font_row;
    let font = decode_font_document(font_json.as_bytes(), "entities[font]")?;
    if font_row_id != font.id.to_string() {
        return Err(StoreError::Invalid(format!(
            "the font row is stored under id {font_row_id} but holds font {}",
            font.id
        )));
    }
    if manifest.font_id != font.id {
        return Err(StoreError::Invalid(format!(
            "the manifest is for font {} but the database holds font {}",
            manifest.font_id, font.id
        )));
    }

    let mut glyphs = Vec::with_capacity(glyph_rows.len());
    for (row_id, json) in glyph_rows {
        let context = format!("entities[glyph {row_id}]");
        let glyph = decode_glyph_document(json.as_bytes(), &context)?;
        if row_id != glyph.id.to_string() {
            return Err(StoreError::Invalid(format!(
                "a glyph row is stored under id {row_id} but holds glyph {}",
                glyph.id
            )));
        }
        glyphs.push(glyph);
    }

    assemble(font, glyphs)
}

/// What the `manifest` table says.
struct Manifest {
    font_id: FontId,
}

fn read_manifest(conn: &Connection) -> Result<Manifest, StoreError> {
    let mut statement = conn.prepare("SELECT key, value FROM manifest")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let entries: BTreeMap<String, String> = rows.collect::<Result<_, _>>()?;

    let get = |key: &str| {
        entries
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| StoreError::Invalid(format!("the manifest has no {key:?}")))
    };

    if get("format")? != FORMAT {
        return Err(StoreError::Invalid(format!(
            "the manifest format is {:?}, not {FORMAT:?}",
            get("format")?
        )));
    }
    let version: u64 = get("format_version")?.parse().map_err(|_| {
        StoreError::Invalid("the manifest format_version is not a number".to_owned())
    })?;
    if version != u64::from(FORMAT_VERSION) {
        return match u32::try_from(version) {
            Ok(newer) if newer > FORMAT_VERSION => Err(StoreError::UnsupportedVersion(newer)),
            Err(_) => Err(StoreError::UnsupportedVersion(u32::MAX)),
            Ok(other) => Err(StoreError::Invalid(format!(
                "the manifest format_version {other} is not a format version"
            ))),
        };
    }
    let font_id = get("font_id")?
        .parse::<FontId>()
        .map_err(|_| StoreError::Invalid("the manifest font_id is not a UUID".to_owned()))?;
    Ok(Manifest { font_id })
}

/// The `entities` rows: the font row as `(id, json)` and the glyph rows.
type Entities = ((String, String), Vec<(String, String)>);

fn read_entities(conn: &Connection) -> Result<Entities, StoreError> {
    let mut statement = conn.prepare("SELECT kind, id, json FROM entities ORDER BY kind, id")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    let mut font_row = None;
    let mut glyph_rows = Vec::new();
    for row in rows {
        let (kind, id, json) = row?;
        match kind.as_str() {
            "font" => {
                if font_row.replace((id, json)).is_some() {
                    return Err(StoreError::Invalid(
                        "the database holds more than one font".to_owned(),
                    ));
                }
            }
            "glyph" => glyph_rows.push((id, json)),
            other => {
                return Err(StoreError::Invalid(format!(
                    "unknown entity kind {other:?} in a format {FORMAT_VERSION} file"
                )));
            }
        }
    }
    let font_row =
        font_row.ok_or_else(|| StoreError::Invalid("the database holds no font".to_owned()))?;
    Ok((font_row, glyph_rows))
}
