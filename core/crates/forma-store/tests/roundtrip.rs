//! Round trips, on-disk layout, validation on load and write safety of the
//! `.forma` and `.formapackage` formats.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forma_model::{
    Font, Glyph, GlyphId, Layer, Master, MasterId, Node, NodeKind, Path as Contour, PathId,
    VerticalMetrics,
};
use forma_store::{StoreError, glyph_file_name, load, pack, save, to_canonical_json, unpack};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn gid(n: u128) -> GlyphId {
    GlyphId(Uuid::from_u128(n))
}

fn mid(n: u128) -> MasterId {
    MasterId(Uuid::from_u128(n))
}

fn corner(x: f64, y: f64) -> Node {
    Node {
        x,
        y,
        kind: NodeKind::Line,
        smooth: false,
    }
}

fn square(advance_width: f64) -> Layer {
    Layer {
        advance_width,
        paths: vec![Contour {
            id: PathId(1),
            closed: true,
            nodes: vec![
                corner(0.0, 0.0),
                corner(500.0, 0.0),
                Node {
                    x: 500.0,
                    y: 500.25,
                    kind: NodeKind::Curve,
                    smooth: true,
                },
                corner(0.0, 500.0),
            ],
        }],
    }
}

/// Adds a glyph with a layer in every master of the font.
fn add_glyph(font: &mut Font, id: u128, name: &str, unicodes: &[u32]) {
    let layers: BTreeMap<MasterId, Layer> = font
        .masters
        .iter()
        .map(|master| (master.id, square(600.0)))
        .collect();
    let glyph = Glyph {
        id: gid(id),
        name: name.to_owned(),
        unicodes: unicodes.to_vec(),
        export: true,
        layers,
    };
    Arc::make_mut(&mut font.glyphs).insert(glyph.id, Arc::new(glyph));
    font.glyph_order.push(gid(id));
}

/// A font with two masters, three glyphs and some `lib` data.
fn sample_font() -> Font {
    let mut font = Font::new("Test \"Sans\" \u{fc}", 1000);
    font.info.copyright = Some("(c) 2026 \u{4e16}\u{754c}".to_owned());
    font.info.designer = Some("Ada".to_owned());
    font.masters.push(Master {
        id: mid(0xB0),
        name: "Bold".to_owned(),
        metrics: VerticalMetrics {
            ascender: 810.5,
            descender: -190.0,
            x_height: 520.0,
            cap_height: 710.0,
        },
    });
    add_glyph(&mut font, 1, "A", &[0x41]);
    add_glyph(&mut font, 2, "a", &[0x61]);
    add_glyph(&mut font, 3, "space", &[0x20, 0xA0]);
    let lib = json!({
        "com.example.zeta": {"z": 1, "a": [1.5, true, null, "x"]},
        "com.example.alpha": 0.1,
    });
    if let Value::Object(map) = lib {
        font.lib = map;
    }
    font
}

fn forma_in(dir: &TempDir) -> PathBuf {
    dir.path().join("font.forma")
}

fn package_in(dir: &TempDir) -> PathBuf {
    dir.path().join("font.formapackage")
}

fn glyph_files(package: &Path) -> BTreeSet<String> {
    match fs::read_dir(package.join("glyphs")) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect(),
        Err(_) => BTreeSet::new(),
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Rewrites a JSON file in place after letting `edit` change its value.
fn edit_json(path: &Path, edit: impl FnOnce(&mut Value)) {
    let mut value = read_json(path);
    edit(&mut value);
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

fn names_in(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}

fn sqlite_rw(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap()
}

// ---------------------------------------------------------------------------
// Round trips
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_forma_file() {
    let dir = TempDir::new().unwrap();
    let path = forma_in(&dir);
    let font = sample_font();

    save(&font, &path).unwrap();

    assert_eq!(load(&path).unwrap(), font);
}

#[test]
fn roundtrip_package() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    let font = sample_font();

    save(&font, &path).unwrap();

    assert_eq!(load(&path).unwrap(), font);
}

#[test]
fn roundtrip_empty_font_in_both_formats() {
    let dir = TempDir::new().unwrap();
    let font = Font::new("Empty", 2048);

    save(&font, &forma_in(&dir)).unwrap();
    save(&font, &package_in(&dir)).unwrap();

    assert_eq!(load(&forma_in(&dir)).unwrap(), font);
    assert_eq!(load(&package_in(&dir)).unwrap(), font);
}

#[test]
fn extensions_are_matched_case_insensitively() {
    let dir = TempDir::new().unwrap();
    let font = sample_font();

    save(&font, &dir.path().join("font.FORMA")).unwrap();
    save(&font, &dir.path().join("font.FormaPackage")).unwrap();

    assert_eq!(load(&dir.path().join("font.FORMA")).unwrap(), font);
    assert_eq!(load(&dir.path().join("font.FormaPackage")).unwrap(), font);
}

#[test]
fn pack_unpack_roundtrip() {
    let dir = TempDir::new().unwrap();
    let font = sample_font();
    let forma = forma_in(&dir);
    save(&font, &forma).unwrap();

    // .forma -> .formapackage -> .forma, through both entry points.
    let package = package_in(&dir);
    unpack(&forma, &package).unwrap();
    assert_eq!(load(&package).unwrap(), font);

    let repacked = dir.path().join("again.forma");
    pack(&package, &repacked).unwrap();
    assert_eq!(load(&repacked).unwrap(), font);

    // Both functions accept either direction.
    let other_package = dir.path().join("other.formapackage");
    pack(&forma, &other_package).unwrap();
    assert_eq!(load(&other_package).unwrap(), font);
    let other_forma = dir.path().join("other.forma");
    unpack(&package, &other_forma).unwrap();
    assert_eq!(load(&other_forma).unwrap(), font);
}

#[test]
fn pack_refuses_an_unknown_destination_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let forma = forma_in(&dir);
    save(&sample_font(), &forma).unwrap();
    let before = names_in(dir.path());

    for dst in ["out.txt", "out", "out.zip"] {
        let err = pack(&forma, &dir.path().join(dst)).unwrap_err();
        assert!(
            matches!(err, StoreError::UnknownFormat(_)),
            "{dst}: {err:?}"
        );
        let err = unpack(&forma, &dir.path().join(dst)).unwrap_err();
        assert!(
            matches!(err, StoreError::UnknownFormat(_)),
            "{dst}: {err:?}"
        );
    }

    assert_eq!(names_in(dir.path()), before);
}

#[test]
fn unknown_extensions_are_rejected() {
    let dir = TempDir::new().unwrap();
    for name in ["font.txt", "font", "font.ufo", "font.forma.bak"] {
        let path = dir.path().join(name);
        let err = save(&sample_font(), &path).unwrap_err();
        assert!(
            matches!(err, StoreError::UnknownFormat(_)),
            "{name}: {err:?}"
        );
        assert!(!path.exists(), "{name} must not be created");
        let err = load(&path).unwrap_err();
        assert!(
            matches!(err, StoreError::UnknownFormat(_)),
            "{name}: {err:?}"
        );
    }
}

#[test]
fn loading_a_missing_document_is_an_io_error() {
    let dir = TempDir::new().unwrap();
    for path in [forma_in(&dir), package_in(&dir)] {
        let err = load(&path).unwrap_err();
        assert!(matches!(err, StoreError::Io(_)), "{err:?}");
    }
}

// ---------------------------------------------------------------------------
// .formapackage layout and canonical JSON
// ---------------------------------------------------------------------------

#[test]
fn package_layout() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    let font = sample_font();
    save(&font, &path).unwrap();

    assert_eq!(
        names_in(&path),
        BTreeSet::from([
            "font.json".to_owned(),
            "glyphs".to_owned(),
            "manifest.json".to_owned()
        ])
    );

    let manifest = read_json(&path.join("manifest.json"));
    assert_eq!(manifest["format"], "formapackage");
    assert_eq!(manifest["format_version"], 1);
    assert_eq!(manifest["font_id"], font.id.to_string());
    assert!(
        manifest["generator"]
            .as_str()
            .unwrap()
            .starts_with("forma "),
        "{manifest}"
    );
    assert_eq!(manifest.as_object().unwrap().len(), 4);

    // font.json is the font without its glyphs.
    let font_json = read_json(&path.join("font.json"));
    let keys: BTreeSet<&str> = font_json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "glyph_order",
            "id",
            "info",
            "lib",
            "masters",
            "units_per_em"
        ])
    );

    // One file per glyph holding the full glyph.
    assert_eq!(
        glyph_files(&path),
        BTreeSet::from([
            "A_.json".to_owned(),
            "a.json".to_owned(),
            "space.json".to_owned()
        ])
    );
    let glyph = read_json(&path.join("glyphs").join("A_.json"));
    assert_eq!(glyph["id"], gid(1).to_string());
    assert_eq!(glyph["name"], "A");
}

#[test]
fn package_is_canonical_json() {
    let dir = TempDir::new().unwrap();
    let first = dir.path().join("one.formapackage");
    let second = dir.path().join("two.formapackage");
    let font = sample_font();

    save(&font, &first).unwrap();
    save(&font, &second).unwrap();
    // Saving again over the first one must not change a byte either.
    save(&font, &first).unwrap();

    let mut checked = Vec::new();
    for relative in [
        "manifest.json",
        "font.json",
        "glyphs/A_.json",
        "glyphs/a.json",
    ] {
        let a = fs::read_to_string(first.join(relative)).unwrap();
        let b = fs::read_to_string(second.join(relative)).unwrap();
        assert_eq!(a, b, "{relative} differs between two saves");

        // A final newline, exactly one, and 2-space indentation.
        assert!(a.ends_with("}\n") && !a.ends_with("\n\n"), "{relative}");
        assert!(a.starts_with("{\n  \""), "{relative}: {a}");
        assert!(!a.contains('\t'), "{relative}");

        // Keys are sorted at every level: writing the parsed value back out
        // canonically must reproduce the file byte for byte.
        let parsed: Value = serde_json::from_str(&a).unwrap();
        assert_eq!(to_canonical_json(&parsed), a, "{relative} is not canonical");
        checked.push(a);
    }

    // The top-level keys of the manifest, in file order.
    let manifest = &checked[0];
    let positions: Vec<usize> = ["font_id", "format\"", "format_version", "generator"]
        .iter()
        .map(|key| manifest.find(key).unwrap())
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{manifest}"
    );
}

#[test]
fn canonical_json_sorts_keys_and_keeps_array_order() {
    let value = json!({"b": [3, 1, 2], "a": {"y": null, "x": 1.5}, "c": "text"});
    assert_eq!(
        to_canonical_json(&value),
        "{\n  \"a\": {\n    \"x\": 1.5,\n    \"y\": null\n  },\n  \"b\": [\n    3,\n    1,\n    2\n  ],\n  \"c\": \"text\"\n}\n"
    );
}

#[test]
fn case_colliding_names_roundtrip() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    let mut font = Font::new("Collide", 1000);
    add_glyph(&mut font, 1, "a", &[0x61]);
    add_glyph(&mut font, 2, "A", &[0x41]);
    add_glyph(&mut font, 3, "con", &[]);
    add_glyph(&mut font, 4, "CON.alt", &[]);
    add_glyph(&mut font, 5, ".notdef", &[]);
    add_glyph(&mut font, 6, "a_", &[]);

    save(&font, &path).unwrap();

    assert_eq!(load(&path).unwrap(), font);

    // No file was overwritten: six glyphs, six files, distinct even when
    // compared case-insensitively. Glyphs get file names in id order, so `A`
    // (-> `A_.json`) is placed before `a_` (-> `a_.json`) and the latter is
    // numbered.
    let files = glyph_files(&path);
    assert_eq!(
        files,
        BTreeSet::from([
            "A_.json".to_owned(),
            "_C_O_N_.alt.json".to_owned(),
            "_con.json".to_owned(),
            "_notdef.json".to_owned(),
            "a.json".to_owned(),
            "a_#2.json".to_owned(),
        ])
    );
    let lowered: HashSet<String> = files.iter().map(|name| name.to_lowercase()).collect();
    assert_eq!(lowered.len(), 6);
}

#[test]
fn numbered_file_names_that_collide_again_are_numbered_again() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    let mut font = Font::new("Chain", 1000);
    add_glyph(&mut font, 1, "A", &[]);
    add_glyph(&mut font, 2, "a_", &[]);
    // The glyph literally called `a_#2` wants the file name that numbering
    // `a_` has just taken.
    add_glyph(&mut font, 3, "a_#2", &[]);

    save(&font, &path).unwrap();

    assert_eq!(
        glyph_files(&path),
        BTreeSet::from([
            "A_.json".to_owned(),
            "a_#2.json".to_owned(),
            "a_#2#2.json".to_owned(),
        ])
    );
    assert_eq!(load(&path).unwrap(), font);
}

#[test]
fn load_does_not_depend_on_glyph_file_names() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    let font = sample_font();
    save(&font, &path).unwrap();

    let glyphs = path.join("glyphs");
    for (n, name) in glyph_files(&path).into_iter().enumerate() {
        fs::rename(
            glyphs.join(&name),
            glyphs.join(format!("zz-{}.json", 9 - n)),
        )
        .unwrap();
    }

    assert_eq!(load(&path).unwrap(), font);
}

#[test]
fn load_ignores_files_that_are_not_glyph_json() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    let font = sample_font();
    save(&font, &path).unwrap();
    fs::write(path.join("glyphs").join(".DS_Store"), b"junk").unwrap();
    fs::write(path.join("glyphs").join("notes.txt"), b"junk").unwrap();
    fs::write(path.join("glyphs").join("a.json.tmp-1234"), b"{").unwrap();
    fs::create_dir(path.join("glyphs").join("sub.json")).unwrap();

    assert_eq!(load(&path).unwrap(), font);
}

#[test]
fn package_without_a_glyphs_folder_loads_as_a_font_without_glyphs() {
    // Git does not track empty folders, so a cloned empty font has no
    // `glyphs/` directory.
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    let font = Font::new("Empty", 1000);
    save(&font, &path).unwrap();
    fs::remove_dir(path.join("glyphs")).unwrap();

    assert_eq!(load(&path).unwrap(), font);
}

#[test]
fn duplicate_glyph_ids_across_files_are_invalid() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    save(&sample_font(), &path).unwrap();
    let glyphs = path.join("glyphs");
    fs::copy(glyphs.join("a.json"), glyphs.join("copy-of-a.json")).unwrap();

    let err = load(&path).unwrap_err();

    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
}

#[test]
fn unsupported_package_version_is_reported() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    save(&sample_font(), &path).unwrap();
    edit_json(&path.join("manifest.json"), |m| {
        m["format_version"] = json!(2)
    });

    let err = load(&path).unwrap_err();

    assert!(matches!(err, StoreError::UnsupportedVersion(2)), "{err:?}");
}

#[test]
fn package_manifest_must_be_a_forma_manifest() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    save(&sample_font(), &path).unwrap();

    edit_json(&path.join("manifest.json"), |m| {
        m["format"] = json!("other")
    });
    let err = load(&path).unwrap_err();
    assert!(matches!(err, StoreError::NotAFormaPackage(_)), "{err:?}");

    fs::remove_file(path.join("manifest.json")).unwrap();
    let err = load(&path).unwrap_err();
    assert!(matches!(err, StoreError::NotAFormaPackage(_)), "{err:?}");
}

#[test]
fn a_package_with_a_damaged_version_is_rejected_but_can_be_repaired_by_saving() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    let font = sample_font();
    save(&font, &path).unwrap();

    for version in [json!(0), json!("one"), Value::Null] {
        edit_json(&path.join("manifest.json"), |m| {
            m["format_version"] = version
        });
        let err = load(&path).unwrap_err();
        assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
    }
    edit_json(&path.join("manifest.json"), |m| {
        m.as_object_mut().unwrap().remove("format_version");
    });
    assert!(matches!(load(&path).unwrap_err(), StoreError::Invalid(_)));

    save(&font, &path).unwrap();

    assert_eq!(load(&path).unwrap(), font);
}

#[test]
fn package_manifest_must_name_the_font() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    save(&sample_font(), &path).unwrap();
    edit_json(&path.join("manifest.json"), |m| {
        m["font_id"] = json!(Uuid::from_u128(99).to_string());
    });

    let err = load(&path).unwrap_err();

    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
}

#[test]
fn corrupt_glyph_json_is_an_error_that_names_the_file() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    save(&sample_font(), &path).unwrap();
    fs::write(path.join("glyphs").join("a.json"), b"{ not json").unwrap();

    let err = load(&path).unwrap_err();

    assert!(matches!(err, StoreError::Json(_)), "{err:?}");
    assert!(err.to_string().contains("a.json"), "{err}");
}

// ---------------------------------------------------------------------------
// load() always validates the model invariants
// ---------------------------------------------------------------------------

#[test]
fn package_without_masters_is_rejected() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    save(&sample_font(), &path).unwrap();
    edit_json(&path.join("font.json"), |f| f["masters"] = json!([]));

    let err = load(&path).unwrap_err();

    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
}

#[test]
fn package_with_a_broken_glyph_order_is_rejected() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    save(&sample_font(), &path).unwrap();

    // A glyph missing from the order.
    edit_json(&path.join("font.json"), |f| {
        f["glyph_order"].as_array_mut().unwrap().pop();
    });
    let err = load(&path).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");

    // A glyph listed twice.
    edit_json(&path.join("font.json"), |f| {
        let order = f["glyph_order"].as_array_mut().unwrap();
        let first = order[0].clone();
        order.push(first.clone());
        order.push(first);
    });
    let err = load(&path).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
}

#[test]
fn forma_file_without_masters_or_with_a_broken_glyph_order_is_rejected() {
    let dir = TempDir::new().unwrap();
    let path = forma_in(&dir);
    save(&sample_font(), &path).unwrap();
    let font_row: String = sqlite_rw(&path)
        .query_row("SELECT json FROM entities WHERE kind = 'font'", [], |r| {
            r.get(0)
        })
        .unwrap();

    for (field, value) in [("masters", json!([])), ("glyph_order", json!([]))] {
        let mut broken: Value = serde_json::from_str(&font_row).unwrap();
        broken[field] = value;
        sqlite_rw(&path)
            .execute(
                "UPDATE entities SET json = ?1 WHERE kind = 'font'",
                [serde_json::to_string(&broken).unwrap()],
            )
            .unwrap();

        let err = load(&path).unwrap_err();
        assert!(matches!(err, StoreError::Invalid(_)), "{field}: {err:?}");
    }
}

#[test]
fn saving_an_invalid_font_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let mut no_masters = sample_font();
    no_masters.masters.clear();
    let mut bad_order = sample_font();
    bad_order.glyph_order.pop();
    let mut not_finite = sample_font();
    let glyph = Arc::make_mut(&mut not_finite.glyphs)
        .get_mut(&gid(1))
        .unwrap();
    let layer = Arc::make_mut(glyph).layers.values_mut().next().unwrap();
    layer.paths[0].nodes[0].x = f64::NAN;

    for (n, font) in [no_masters, bad_order, not_finite].iter().enumerate() {
        for path in [
            dir.path().join(format!("{n}.forma")),
            dir.path().join(format!("{n}.formapackage")),
        ] {
            let err = save(font, &path).unwrap_err();
            assert!(matches!(err, StoreError::Invalid(_)), "{path:?}: {err:?}");
            assert!(!path.exists(), "{path:?} must not be created");
        }
    }
    assert!(names_in(dir.path()).is_empty());
}

// ---------------------------------------------------------------------------
// .forma (SQLite)
// ---------------------------------------------------------------------------

#[test]
fn forma_file_header_schema_and_rows() {
    let dir = TempDir::new().unwrap();
    let path = forma_in(&dir);
    let font = sample_font();
    save(&font, &path).unwrap();

    let conn = sqlite_rw(&path);
    let application_id: i64 = conn
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .unwrap();
    let user_version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    let journal_mode: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(application_id, 0x464F_524D);
    assert_eq!(user_version, 1);
    assert_eq!(journal_mode, "delete");

    let columns = |table: &str| -> Vec<(String, String, bool, i64)> {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT name, type, \"notnull\", pk FROM pragma_table_info('{table}')"
            ))
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        columns("manifest"),
        vec![
            ("key".to_owned(), "TEXT".to_owned(), false, 1),
            ("value".to_owned(), "TEXT".to_owned(), true, 0),
        ]
    );
    assert_eq!(
        columns("entities"),
        vec![
            ("kind".to_owned(), "TEXT".to_owned(), true, 1),
            ("id".to_owned(), "TEXT".to_owned(), true, 2),
            ("json".to_owned(), "TEXT".to_owned(), true, 0),
        ]
    );

    let manifest: BTreeMap<String, String> = conn
        .prepare("SELECT key, value FROM manifest")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(manifest["format"], "forma");
    assert_eq!(manifest["format_version"], "1");
    assert_eq!(manifest["font_id"], font.id.to_string());
    assert!(manifest["generator"].starts_with("forma "));
    assert_eq!(manifest.len(), 4);

    let rows: Vec<(String, String, String)> = conn
        .prepare("SELECT kind, id, json FROM entities ORDER BY kind, id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let kinds: Vec<&str> = rows.iter().map(|row| row.0.as_str()).collect();
    assert_eq!(kinds, ["font", "glyph", "glyph", "glyph"]);
    assert_eq!(rows[0].1, font.id.to_string());
    for (_, id, json) in &rows {
        // The stored JSON is canonical, like the files of a package.
        let parsed: Value = serde_json::from_str(json).unwrap();
        assert_eq!(&to_canonical_json(&parsed), json);
        assert_eq!(parsed["id"], *id);
    }
    assert!(!rows[0].2.contains("\"glyphs\""));
}

#[test]
fn foreign_sqlite_rejected() {
    let dir = TempDir::new().unwrap();

    // An ordinary SQLite database: application_id 0.
    let plain = dir.path().join("plain.forma");
    let conn = Connection::open(&plain).unwrap();
    conn.execute_batch("CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT);")
        .unwrap();
    drop(conn);
    let err = load(&plain).unwrap_err();
    assert!(matches!(err, StoreError::NotAFormaFile(_)), "{err:?}");

    // Another application's identifier.
    let foreign = dir.path().join("foreign.forma");
    let conn = Connection::open(&foreign).unwrap();
    conn.execute_batch("PRAGMA application_id = 1234; CREATE TABLE t (x);")
        .unwrap();
    drop(conn);
    let err = load(&foreign).unwrap_err();
    assert!(matches!(err, StoreError::NotAFormaFile(_)), "{err:?}");

    // Not SQLite at all, and an empty file.
    let text = dir.path().join("text.forma");
    fs::write(
        &text,
        b"this is definitely not a database, but long enough to look like one",
    )
    .unwrap();
    let err = load(&text).unwrap_err();
    assert!(matches!(err, StoreError::NotAFormaFile(_)), "{err:?}");
    let empty = dir.path().join("empty.forma");
    fs::write(&empty, b"").unwrap();
    let err = load(&empty).unwrap_err();
    assert!(matches!(err, StoreError::NotAFormaFile(_)), "{err:?}");
}

#[test]
fn newer_forma_version_is_unsupported() {
    let dir = TempDir::new().unwrap();
    let path = forma_in(&dir);
    save(&sample_font(), &path).unwrap();
    sqlite_rw(&path)
        .execute_batch("PRAGMA user_version = 2;")
        .unwrap();

    let err = load(&path).unwrap_err();

    assert!(matches!(err, StoreError::UnsupportedVersion(2)), "{err:?}");
}

#[test]
fn forma_file_with_a_mismatched_glyph_row_is_invalid() {
    let dir = TempDir::new().unwrap();
    let path = forma_in(&dir);
    save(&sample_font(), &path).unwrap();
    // The row id no longer matches the id inside the JSON.
    sqlite_rw(&path)
        .execute(
            "UPDATE entities SET id = ?1 WHERE kind = 'glyph' AND id = ?2",
            [Uuid::from_u128(77).to_string(), gid(1).to_string()],
        )
        .unwrap();

    let err = load(&path).unwrap_err();

    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
}

#[test]
fn forma_file_with_an_unknown_entity_kind_is_invalid() {
    let dir = TempDir::new().unwrap();
    let path = forma_in(&dir);
    save(&sample_font(), &path).unwrap();
    sqlite_rw(&path)
        .execute(
            "INSERT INTO entities (kind, id, json) VALUES ('kerning', 'x', '{}')",
            [],
        )
        .unwrap();

    let err = load(&path).unwrap_err();

    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
}

// ---------------------------------------------------------------------------
// Replacing existing documents
// ---------------------------------------------------------------------------

#[test]
fn saving_a_forma_file_over_an_existing_one_replaces_it_completely() {
    let dir = TempDir::new().unwrap();
    let path = forma_in(&dir);
    save(&sample_font(), &path).unwrap();

    // A different, smaller font: nothing of the first may survive.
    let mut small = Font::new("Small", 500);
    add_glyph(&mut small, 9, "only", &[0x6F]);
    save(&small, &path).unwrap();

    assert_eq!(load(&path).unwrap(), small);
    let conn = sqlite_rw(&path);
    let glyph_rows: i64 = conn
        .query_row(
            "SELECT count(*) FROM entities WHERE kind = 'glyph'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let font_rows: i64 = conn
        .query_row(
            "SELECT count(*) FROM entities WHERE kind = 'font'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!((glyph_rows, font_rows), (1, 1));
    drop(conn);

    // No temporary file is left next to it.
    assert_eq!(
        names_in(dir.path()),
        BTreeSet::from(["font.forma".to_owned()])
    );
}

#[test]
fn a_failed_forma_save_leaves_the_target_and_no_temporary_files() {
    // A directory cannot be replaced by a file, on any platform.
    let dir = TempDir::new().unwrap();
    let path = forma_in(&dir);
    fs::create_dir(&path).unwrap();
    fs::write(path.join("keep.txt"), b"keep").unwrap();

    let err = save(&sample_font(), &path).unwrap_err();

    assert!(matches!(err, StoreError::Io(_)), "{err:?}");
    assert_eq!(fs::read(path.join("keep.txt")).unwrap(), b"keep");
    assert_eq!(
        names_in(dir.path()),
        BTreeSet::from(["font.forma".to_owned()])
    );
}

#[test]
fn saving_into_a_missing_folder_fails_cleanly() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("missing").join("font.forma");

    assert!(save(&sample_font(), &path).is_err());

    assert!(!dir.path().join("missing").exists());
}

#[test]
fn resaving_a_package_removes_glyph_files_that_no_longer_exist() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    save(&sample_font(), &path).unwrap();
    assert_eq!(glyph_files(&path).len(), 3);
    // Files that are not glyph JSON are never touched.
    fs::write(path.join("README.md"), b"hello").unwrap();
    fs::write(path.join("glyphs").join("notes.txt"), b"mine").unwrap();

    let mut smaller = sample_font();
    let doomed = [gid(1), gid(3)];
    Arc::make_mut(&mut smaller.glyphs).retain(|id, _| !doomed.contains(id));
    smaller.glyph_order.retain(|id| !doomed.contains(id));
    save(&smaller, &path).unwrap();

    assert_eq!(
        glyph_files(&path),
        BTreeSet::from(["a.json".to_owned(), "notes.txt".to_owned()])
    );
    assert_eq!(fs::read(path.join("README.md")).unwrap(), b"hello");
    assert_eq!(load(&path).unwrap(), smaller);
}

/// `font` with every layer of every glyph set to the given advance width.
fn with_advance_width(font: &Font, width: f64) -> Font {
    let mut font = font.clone();
    for glyph in Arc::make_mut(&mut font.glyphs).values_mut() {
        for layer in Arc::make_mut(glyph).layers.values_mut() {
            layer.advance_width = width;
        }
    }
    font
}

#[test]
fn an_io_error_while_saving_a_package_leaves_the_previous_package_intact() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    let old = sample_font();
    save(&old, &path).unwrap();
    // The file of the second glyph cannot be read or replaced.
    let glyphs = path.join("glyphs");
    fs::remove_file(glyphs.join("a.json")).unwrap();
    fs::create_dir(glyphs.join("a.json")).unwrap();
    let files = [
        "manifest.json",
        "font.json",
        "glyphs/A_.json",
        "glyphs/space.json",
    ];
    let before: Vec<Vec<u8>> = files
        .iter()
        .map(|f| fs::read(path.join(f)).unwrap())
        .collect();

    // Every glyph changes, so the first and third files would be rewritten.
    let err = save(&with_advance_width(&old, 700.0), &path).unwrap_err();

    assert!(matches!(err, StoreError::Io(_)), "{err:?}");
    let after: Vec<Vec<u8>> = files
        .iter()
        .map(|f| fs::read(path.join(f)).unwrap())
        .collect();
    assert_eq!(after, before);
    assert_eq!(
        glyph_files(&path),
        BTreeSet::from([
            "A_.json".to_owned(),
            "a.json".to_owned(),
            "space.json".to_owned()
        ])
    );
    assert!(glyphs.join("a.json").is_dir());
}

/// The file identity (inode) of the files of a package.
#[cfg(unix)]
fn file_ids(package: &Path) -> BTreeMap<String, u64> {
    use std::os::unix::fs::MetadataExt;

    let mut ids = BTreeMap::new();
    for relative in ["manifest.json", "font.json"] {
        ids.insert(
            relative.to_owned(),
            fs::metadata(package.join(relative)).unwrap().ino(),
        );
    }
    for name in glyph_files(package) {
        let ino = fs::metadata(package.join("glyphs").join(&name))
            .unwrap()
            .ino();
        ids.insert(format!("glyphs/{name}"), ino);
    }
    ids
}

#[cfg(unix)]
#[test]
fn resaving_a_package_only_replaces_the_files_that_changed() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    let font = sample_font();
    save(&font, &path).unwrap();
    let first = file_ids(&path);

    // Nothing changed: not one file is replaced (a replaced file is a new
    // inode), so nothing is written or flushed.
    save(&font, &path).unwrap();
    assert_eq!(file_ids(&path), first);

    // One glyph changed: only its file is replaced.
    let mut edited = font.clone();
    let glyph = Arc::make_mut(&mut edited.glyphs).get_mut(&gid(3)).unwrap();
    Arc::make_mut(glyph)
        .layers
        .values_mut()
        .for_each(|l| l.advance_width = 321.0);
    save(&edited, &path).unwrap();
    let second = file_ids(&path);
    let changed: Vec<&String> = first
        .keys()
        .filter(|key| first[*key] != second[*key])
        .collect();
    assert_eq!(changed, ["glyphs/space.json"]);
    assert_eq!(load(&path).unwrap(), edited);
}

#[test]
fn resaving_a_package_after_a_case_only_rename_leaves_one_file() {
    // `a_` is stored as `a_.json`, `A` as `A_.json`: the same name on a
    // case-insensitive file system.
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    let mut first = Font::new("Case", 1000);
    add_glyph(&mut first, 1, "a_", &[]);
    save(&first, &path).unwrap();

    let mut second = Font::new("Case", 1000);
    second.id = first.id;
    second.masters = first.masters.clone();
    add_glyph(&mut second, 2, "A", &[]);
    save(&second, &path).unwrap();

    assert_eq!(glyph_files(&path).len(), 1, "{:?}", glyph_files(&path));
    assert_eq!(load(&path).unwrap(), second);
}

// ---------------------------------------------------------------------------
// Never clobbering folders that are not packages
// ---------------------------------------------------------------------------

#[test]
fn saving_into_a_folder_that_is_not_a_package_is_refused() {
    let dir = TempDir::new().unwrap();
    let target = package_in(&dir);
    fs::create_dir(&target).unwrap();
    fs::write(target.join("notes.txt"), b"precious").unwrap();
    fs::create_dir(target.join("photos")).unwrap();

    let err = save(&sample_font(), &target).unwrap_err();

    assert!(matches!(err, StoreError::NotAFormaPackage(_)), "{err:?}");
    assert_eq!(
        names_in(&target),
        BTreeSet::from(["notes.txt".to_owned(), "photos".to_owned()])
    );
    assert_eq!(fs::read(target.join("notes.txt")).unwrap(), b"precious");
}

#[test]
fn the_debris_of_a_killed_first_save_can_be_saved_over() {
    // A first save killed while staging leaves temporary files and an empty
    // `glyphs/`, and no manifest.
    let dir = TempDir::new().unwrap();
    let target = package_in(&dir);
    let temp = |name: &str, n: u128| format!("{name}.tmp-{}", Uuid::from_u128(n));
    fs::create_dir_all(target.join("glyphs")).unwrap();
    fs::write(target.join(temp("manifest.json", 1)), b"{ \"for").unwrap();
    fs::write(target.join("glyphs").join(temp("a.json", 2)), b"{ \"id").unwrap();
    let font = sample_font();

    save(&font, &target).unwrap();

    assert_eq!(load(&target).unwrap(), font);
    assert_eq!(
        names_in(&target),
        BTreeSet::from([
            "font.json".to_owned(),
            "glyphs".to_owned(),
            "manifest.json".to_owned()
        ])
    );
    assert!(
        glyph_files(&target)
            .iter()
            .all(|name| !name.contains(".tmp-"))
    );

    // The same folder with one unknown file in it is still not ours.
    let other = dir.path().join("other.formapackage");
    fs::create_dir_all(other.join("glyphs")).unwrap();
    fs::write(other.join("glyphs").join(temp("a.json", 3)), b"{").unwrap();
    fs::write(other.join("notes.txt"), b"precious").unwrap();
    let err = save(&font, &other).unwrap_err();
    assert!(matches!(err, StoreError::NotAFormaPackage(_)), "{err:?}");
    assert!(other.join("glyphs").join(temp("a.json", 3)).exists());
    assert_eq!(fs::read(other.join("notes.txt")).unwrap(), b"precious");
}

#[test]
fn a_manifest_of_another_format_does_not_make_a_package() {
    let dir = TempDir::new().unwrap();
    for (n, manifest) in [
        r#"{"format": "something-else", "format_version": 1}"#,
        r#"{"format_version": 1}"#,
        r#"["formapackage"]"#,
        "not json at all",
    ]
    .into_iter()
    .enumerate()
    {
        let target = dir.path().join(format!("{n}.formapackage"));
        fs::create_dir(&target).unwrap();
        fs::write(target.join("manifest.json"), manifest).unwrap();

        let err = save(&sample_font(), &target).unwrap_err();

        assert!(
            matches!(err, StoreError::NotAFormaPackage(_)),
            "{manifest}: {err:?}"
        );
        assert_eq!(
            names_in(&target),
            BTreeSet::from(["manifest.json".to_owned()])
        );
        assert_eq!(
            fs::read_to_string(target.join("manifest.json")).unwrap(),
            manifest
        );
    }
}

#[test]
fn a_regular_file_is_not_replaced_by_a_package() {
    let dir = TempDir::new().unwrap();
    let target = package_in(&dir);
    fs::write(&target, b"just a file").unwrap();

    let err = save(&sample_font(), &target).unwrap_err();

    assert!(matches!(err, StoreError::NotAFormaPackage(_)), "{err:?}");
    assert_eq!(fs::read(&target).unwrap(), b"just a file");
}

#[test]
fn an_empty_folder_and_a_missing_one_are_fine() {
    let dir = TempDir::new().unwrap();
    let font = sample_font();

    let empty = dir.path().join("empty.formapackage");
    fs::create_dir(&empty).unwrap();
    save(&font, &empty).unwrap();
    assert_eq!(load(&empty).unwrap(), font);

    let nested = dir
        .path()
        .join("new")
        .join("deeper")
        .join("font.formapackage");
    save(&font, &nested).unwrap();
    assert_eq!(load(&nested).unwrap(), font);
}

#[test]
fn a_package_of_a_newer_version_is_not_overwritten() {
    let dir = TempDir::new().unwrap();
    let path = package_in(&dir);
    save(&sample_font(), &path).unwrap();
    edit_json(&path.join("manifest.json"), |m| {
        m["format_version"] = json!(2)
    });
    let before = fs::read(path.join("font.json")).unwrap();

    let err = save(&Font::new("Other", 1000), &path).unwrap_err();

    assert!(matches!(err, StoreError::UnsupportedVersion(2)), "{err:?}");
    assert_eq!(fs::read(path.join("font.json")).unwrap(), before);
}

// ---------------------------------------------------------------------------
// glyph_file_name through the public API
// ---------------------------------------------------------------------------

#[test]
fn glyph_file_name_public_contract() {
    let none = HashSet::new();
    assert_eq!(glyph_file_name("a", &none), "a.json");
    assert_eq!(glyph_file_name("A", &none), "A_.json");
    assert_eq!(glyph_file_name("Aacute", &none), "A_acute.json");
    assert_eq!(glyph_file_name(".notdef", &none), "_notdef.json");
    assert_eq!(glyph_file_name("con", &none), "_con.json");
    assert_eq!(glyph_file_name("CON.alt", &none), "_C_O_N_.alt.json");
    assert_eq!(glyph_file_name("a/b:c", &none), "a_b_c.json");

    let taken: HashSet<String> = ["A_.json".to_owned()].into();
    assert_eq!(glyph_file_name("a_", &taken), "a_#2.json");
    let taken: HashSet<String> = ["a_.json".to_owned(), "a_#2.json".to_owned()].into();
    assert_eq!(glyph_file_name("a_", &taken), "a_#3.json");
    assert_eq!(glyph_file_name("A_", &taken), "A__.json");
}
