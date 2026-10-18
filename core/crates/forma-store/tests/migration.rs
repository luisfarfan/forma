//! Format v2 (components and kerning) and the migration of v1 documents.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use forma_model::{
    Component, Font, FontId, Glyph, GlyphId, KernRef, KernSide, KerningGroup, KerningGroupId,
    Layer, MasterId, Node, NodeKind, Transform,
};
use forma_model::{Path as Contour, PathId};
use forma_store::{
    FORMAT_VERSION, READABLE_VERSIONS, StoreError, load, migrate_font_document,
    migrate_glyph_document, save,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

fn fixtures_v1() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1")
}

fn corner(x: f64, y: f64) -> Node {
    Node {
        x,
        y,
        kind: NodeKind::Line,
        smooth: false,
    }
}

fn layer(advance_width: f64, size: f64) -> Layer {
    Layer {
        advance_width,
        paths: vec![Contour {
            id: PathId(1),
            closed: true,
            nodes: vec![
                corner(50.0, 0.0),
                corner(50.0 + size, 0.0),
                corner(50.0 + size, size),
                corner(50.0, size),
            ],
        }],
        components: Vec::new(),
    }
}

/// A fixed three-glyph font, as format v1 stored it (no components, no
/// kerning). Every id is fixed so that the fixtures are reproducible.
fn sample_v1_font() -> Font {
    let master = MasterId(Uuid::from_u128(0x10));
    let mut font = Font::new("Fixture", 1000);
    font.id = FontId(Uuid::from_u128(0x1));
    font.masters[0].id = master;
    let mut glyphs = BTreeMap::new();
    let mut order = Vec::new();
    for (n, (name, unicode, size)) in [
        ("A", 0x41u32, 600.0),
        ("B", 0x42, 550.0),
        ("space", 0x20, 0.0),
    ]
    .into_iter()
    .enumerate()
    {
        let id = GlyphId(Uuid::from_u128(0x100 + n as u128));
        let mut layers = BTreeMap::new();
        let layer = if size == 0.0 {
            Layer {
                advance_width: 250.0,
                paths: Vec::new(),
                components: Vec::new(),
            }
        } else {
            layer(700.0, size)
        };
        layers.insert(master, layer);
        glyphs.insert(
            id,
            Arc::new(Glyph {
                id,
                name: name.to_owned(),
                unicodes: vec![unicode],
                export: true,
                layers,
            }),
        );
        order.push(id);
    }
    font.glyphs = Arc::new(glyphs);
    font.glyph_order = order;
    font
}

/// Writes the v1 fixtures. Run once, while the format was still version 1:
/// `cargo test -p forma-store --test migration -- --ignored generate_v1_fixtures`.
/// The output is versioned and must keep loading forever.
#[test]
#[ignore = "generates the committed v1 fixtures; only meaningful with FORMAT_VERSION 1"]
fn generate_v1_fixtures() {
    let dir = fixtures_v1();
    std::fs::create_dir_all(&dir).unwrap();
    let font = sample_v1_font();
    forma_store::save(&font, &dir.join("doc.forma")).unwrap();
    forma_store::save(&font, &dir.join("doc.formapackage")).unwrap();
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn user_version(path: &std::path::Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

fn manifest_version(path: &std::path::Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    let text: String = conn
        .query_row(
            "SELECT value FROM manifest WHERE key = 'format_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    text.parse().unwrap()
}

/// The v1 sample plus a component, a kerning group and two pairs.
fn sample_v2_font() -> Font {
    let mut font = sample_v1_font();
    let master = font.masters[0].id;
    let a = GlyphId(Uuid::from_u128(0x100));
    let b = GlyphId(Uuid::from_u128(0x101));
    let space = GlyphId(Uuid::from_u128(0x102));
    // Glyph "space" gets a component of "A" (acyclic).
    let mut glyphs = (*font.glyphs).clone();
    let mut glyph = (*glyphs[&space]).clone();
    if let Some(layer) = glyph.layers.get_mut(&master) {
        layer.components.push(Component {
            base: a,
            transform: Transform {
                xx: 0.5,
                xy: 0.0,
                yx: 0.0,
                yy: 0.5,
                dx: 12.5,
                dy: -3.0,
            },
        });
    }
    glyphs.insert(space, Arc::new(glyph));
    font.glyphs = Arc::new(glyphs);
    let group = KerningGroupId(Uuid::from_u128(0x200));
    font.kerning.groups_mut().insert(
        group,
        KerningGroup {
            id: group,
            name: "round".to_owned(),
            side: KernSide::Second,
            members: vec![b],
        },
    );
    font.kerning
        .set_pair(master, KernRef::Glyph(b), KernRef::Glyph(a), Some(-30.0));
    font.kerning.set_pair(
        master,
        KernRef::Glyph(a),
        KernRef::Group(group),
        Some(-12.5),
    );
    font
}

#[test]
fn v1_forma_loads_and_saves_as_v2() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("doc.forma");
    std::fs::copy(fixtures_v1().join("doc.forma"), &path).unwrap();
    assert_eq!(user_version(&path), 1);

    let font = load(&path).unwrap();
    assert_eq!(font, sample_v1_font());
    save(&font, &path).unwrap();
    assert_eq!(user_version(&path), 2);
    assert_eq!(manifest_version(&path), 2);
    assert_eq!(load(&path).unwrap(), font);
}

#[test]
fn v1_package_loads_and_saves_as_v2() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("doc.formapackage");
    copy_dir(&fixtures_v1().join("doc.formapackage"), &path);

    let font = load(&path).unwrap();
    assert_eq!(font, sample_v1_font());
    save(&font, &path).unwrap();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(path.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["format_version"], json!(2));
    assert_eq!(load(&path).unwrap(), font);
    // Same content: the font and glyph documents did not change.
    for file in [
        "font.json",
        "glyphs/A_.json",
        "glyphs/B_.json",
        "glyphs/space.json",
    ] {
        assert_eq!(
            std::fs::read(path.join(file)).unwrap(),
            std::fs::read(fixtures_v1().join("doc.formapackage").join(file)).unwrap(),
            "{file}"
        );
    }
}

#[test]
fn v3_is_rejected_with_unsupported_version() {
    let dir = TempDir::new().unwrap();

    let forma = dir.path().join("doc.forma");
    save(&sample_v2_font(), &forma).unwrap();
    Connection::open(&forma)
        .unwrap()
        .pragma_update(None, "user_version", 3)
        .unwrap();
    assert!(matches!(
        load(&forma),
        Err(StoreError::UnsupportedVersion(3))
    ));

    let package = dir.path().join("doc.formapackage");
    save(&sample_v2_font(), &package).unwrap();
    let manifest_path = package.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["format_version"] = json!(3);
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(matches!(
        load(&package),
        Err(StoreError::UnsupportedVersion(3))
    ));
}

#[test]
fn versions_are_declared() {
    assert_eq!(FORMAT_VERSION, 2);
    assert_eq!(READABLE_VERSIONS, [1, 2]);
}

#[test]
fn migrate_documents_are_pure() {
    let font_doc = json!({"id": "x", "units_per_em": 1000});
    let glyph_doc = json!({"id": "y", "layers": {}});
    for version in READABLE_VERSIONS {
        assert_eq!(
            migrate_font_document(version, font_doc.clone()).unwrap(),
            migrate_font_document(version, font_doc.clone()).unwrap()
        );
        assert_eq!(
            migrate_glyph_document(version, glyph_doc.clone()).unwrap(),
            migrate_glyph_document(version, glyph_doc.clone()).unwrap()
        );
    }
    // v2 -> v2 is the identity, and v1 -> v2 changes nothing.
    assert_eq!(
        migrate_font_document(2, font_doc.clone()).unwrap(),
        font_doc
    );
    assert_eq!(
        migrate_glyph_document(2, glyph_doc.clone()).unwrap(),
        glyph_doc
    );
    assert_eq!(
        migrate_font_document(1, font_doc.clone()).unwrap(),
        font_doc
    );
    assert_eq!(
        migrate_glyph_document(1, glyph_doc.clone()).unwrap(),
        glyph_doc
    );
    assert!(matches!(
        migrate_font_document(3, font_doc),
        Err(StoreError::UnsupportedVersion(3))
    ));
    assert!(matches!(
        migrate_glyph_document(0, glyph_doc),
        Err(StoreError::Invalid(_))
    ));
}

#[test]
fn kerning_and_components_roundtrip_both_formats() {
    let font = sample_v2_font();
    assert_eq!(font.validate_invariants(), Ok(()));
    let dir = TempDir::new().unwrap();
    for name in ["k.forma", "k.formapackage"] {
        let path = dir.path().join(name);
        save(&font, &path).unwrap();
        assert_eq!(load(&path).unwrap(), font, "{name}");
        save(&font, &path).unwrap();
        assert_eq!(load(&path).unwrap(), font, "{name} again");
    }
}

#[test]
fn kerning_in_font_json_is_one_entry_per_pair() {
    let font = sample_v2_font();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("k.formapackage");
    save(&font, &path).unwrap();
    let doc: Value =
        serde_json::from_slice(&std::fs::read(path.join("font.json")).unwrap()).unwrap();
    let master = font.masters[0].id.to_string();
    let pairs = doc["kerning"]["pairs"][&master].as_array().unwrap();
    assert_eq!(pairs.len(), 2);
    // Sorted by (first, second); glyph references sort before groups, and
    // g.…100 (A) before g.…101 (B).
    assert_eq!(
        pairs[0]["first"],
        json!("g.00000000-0000-0000-0000-000000000100")
    );
    assert_eq!(
        pairs[0]["second"],
        json!("k.00000000-0000-0000-0000-000000000200")
    );
    assert_eq!(pairs[0]["value"], json!(-12.5));
    assert_eq!(
        pairs[1]["first"],
        json!("g.00000000-0000-0000-0000-000000000101")
    );
    assert_eq!(doc["kerning"]["groups"].as_array().unwrap().len(), 1);
}

#[test]
fn non_finite_component_and_kerning_values_are_refused_on_save() {
    let dir = TempDir::new().unwrap();
    let master = sample_v2_font().masters[0].id;
    let space = GlyphId(Uuid::from_u128(0x102));

    let mut font = sample_v2_font();
    let mut glyphs = (*font.glyphs).clone();
    let mut glyph = (*glyphs[&space]).clone();
    glyph.layers.get_mut(&master).unwrap().components[0]
        .transform
        .dx = f64::NAN;
    glyphs.insert(space, Arc::new(glyph));
    font.glyphs = Arc::new(glyphs);
    assert!(save(&font, &dir.path().join("a.forma")).is_err());

    let mut font = sample_v2_font();
    let a = KernRef::Glyph(GlyphId(Uuid::from_u128(0x100)));
    font.kerning.set_pair(master, a, a, Some(f64::INFINITY));
    assert!(save(&font, &dir.path().join("b.formapackage")).is_err());
}

/// Writes the format v2 fuzz seeds: a `.forma` file with components and
/// kerning, and a package manifest of version 2 (first byte: the index of the
/// file in the `load_package` target, 0 for the manifest).
/// `cargo test -p forma-store --test migration -- --ignored generate_v2_fuzz_seeds`
#[test]
#[ignore = "regenerates the committed fuzz seeds"]
fn generate_v2_fuzz_seeds() {
    let seeds = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds");
    let font = sample_v2_font();
    let dir = TempDir::new().unwrap();
    save(&font, &dir.path().join("doc.forma")).unwrap();
    std::fs::copy(
        dir.path().join("doc.forma"),
        seeds.join("load_forma/valid_v2.seed"),
    )
    .unwrap();

    // The font id of the other `load_package` seeds, so the package still
    // loads when only the manifest is replaced by this one.
    let manifest = "{\n  \"font_id\": \"36572393-700e-4a1e-854f-89655a574bc1\",\n  \"format\": \"formapackage\",\n  \"format_version\": 2,\n  \"generator\": \"forma 0.0.1\"\n}\n";
    let mut bytes = vec![0u8];
    bytes.extend_from_slice(manifest.as_bytes());
    std::fs::write(seeds.join("load_package/manifest_v2.seed"), bytes).unwrap();
}
