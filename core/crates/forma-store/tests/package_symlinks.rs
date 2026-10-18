//! A `.formapackage` holding a symbolic link is refused by both `load` and
//! `save`, so nothing is ever read, written or deleted through a link.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use forma_model::{Font, Glyph, GlyphId, Layer, MasterId};
use forma_store::{StoreError, load, save};
use tempfile::TempDir;
use uuid::Uuid;

/// A font with one glyph per name (no outlines).
fn font_with(names: &[&str]) -> Font {
    let mut font = Font::new("Links", 1000);
    for (n, name) in names.iter().enumerate() {
        let id = GlyphId(Uuid::from_u128(n as u128 + 1));
        let layers: BTreeMap<MasterId, Layer> = font
            .masters
            .iter()
            .map(|master| {
                (
                    master.id,
                    Layer {
                        advance_width: 500.0,
                        paths: Vec::new(),
                        components: Vec::new(),
                    },
                )
            })
            .collect();
        let glyph = Glyph {
            id,
            name: (*name).to_owned(),
            unicodes: Vec::new(),
            export: true,
            layers,
            provenance: None,
        };
        Arc::make_mut(&mut font.glyphs).insert(id, Arc::new(glyph));
        font.glyph_order.push(id);
    }
    font
}

/// Every entry under `dir`, without following links: file contents, link
/// targets and folders.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, String> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(&current).expect("read_dir") {
            let path = entry.expect("entry").path();
            let kind = fs::symlink_metadata(&path).expect("metadata").file_type();
            let value = if kind.is_symlink() {
                format!("link -> {}", fs::read_link(&path).expect("link").display())
            } else if kind.is_dir() {
                pending.push(path.clone());
                "dir".to_owned()
            } else {
                String::from_utf8_lossy(&fs::read(&path).expect("read")).into_owned()
            };
            entries.insert(path, value);
        }
    }
    entries
}

struct Scene {
    _tmp: TempDir,
    package: PathBuf,
    outside: PathBuf,
}

/// A saved package with glyph `a`, and an outside folder holding
/// `important.json`.
fn scene() -> Scene {
    let tmp = TempDir::new().expect("tempdir");
    let package = tmp.path().join("font.formapackage");
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).expect("mkdir");
    fs::write(outside.join("important.json"), b"{\"keep\": true}").expect("write");
    save(&font_with(&["a"]), &package).expect("save");
    Scene {
        _tmp: tmp,
        package,
        outside,
    }
}

/// Moves `package/<name>` into the outside folder and puts a link to it in
/// its place.
fn replace_with_link(scene: &Scene, name: &str) -> PathBuf {
    let inside = scene.package.join(name);
    let moved = scene
        .outside
        .join(format!("moved-{}", name.replace('/', "-")));
    fs::rename(&inside, &moved).expect("move");
    symlink(&moved, &inside).expect("symlink");
    inside
}

#[track_caller]
fn assert_refused(result: Result<impl std::fmt::Debug, StoreError>, link: &Path) {
    match result {
        Err(StoreError::SymlinkInPackage(path)) => assert_eq!(path, link),
        other => panic!(
            "expected SymlinkInPackage({}), got {other:?}",
            link.display()
        ),
    }
}

/// Load and save both refuse the package, and neither the package nor the
/// outside folder changes.
#[track_caller]
fn assert_load_and_save_refused(scene: &Scene, link: &Path) {
    let package_before = snapshot(&scene.package);
    let outside_before = snapshot(&scene.outside);

    assert_refused(load(&scene.package), link);
    // A save with another glyph set would write `b.json` and delete `a.json`
    // (and any other stale `.json`, such as `important.json`).
    assert_refused(save(&font_with(&["b"]), &scene.package), link);
    assert_refused(save(&font_with(&[]), &scene.package), link);

    assert_eq!(snapshot(&scene.package), package_before, "package changed");
    assert_eq!(
        snapshot(&scene.outside),
        outside_before,
        "outside folder changed"
    );
    assert_eq!(
        fs::read(scene.outside.join("important.json")).expect("outside file kept"),
        b"{\"keep\": true}"
    );
}

#[test]
fn manifest_link_is_refused() {
    let scene = scene();
    let link = replace_with_link(&scene, "manifest.json");
    assert_load_and_save_refused(&scene, &link);
}

#[test]
fn font_json_link_is_refused() {
    let scene = scene();
    let link = replace_with_link(&scene, "font.json");
    assert_load_and_save_refused(&scene, &link);
}

#[test]
fn glyphs_folder_link_is_refused() {
    let scene = scene();
    // The reviewer's scenario: `glyphs` points at a folder outside that also
    // holds a file the save would otherwise delete as stale.
    let link = replace_with_link(&scene, "glyphs");
    fs::write(scene.outside.join("moved-glyphs/important.json"), b"{}").expect("write");
    assert_load_and_save_refused(&scene, &link);
    assert!(scene.outside.join("moved-glyphs/a.json").exists());
    assert!(!scene.outside.join("moved-glyphs/b.json").exists());
}

#[test]
fn glyph_file_link_is_refused() {
    let scene = scene();
    let link = scene.package.join("glyphs/evil.json");
    symlink(scene.outside.join("important.json"), &link).expect("symlink");
    assert_load_and_save_refused(&scene, &link);
}

#[test]
fn any_link_in_the_glyphs_folder_is_refused() {
    let scene = scene();
    let link = scene.package.join("glyphs/notes");
    symlink(&scene.outside, &link).expect("symlink");
    assert_load_and_save_refused(&scene, &link);
}

#[test]
fn dangling_link_is_refused() {
    let scene = scene();
    let link = scene.package.join("glyphs/z.json");
    symlink(scene.outside.join("not-yet.json"), &link).expect("symlink");
    assert_load_and_save_refused(&scene, &link);
    assert!(!scene.outside.join("not-yet.json").exists());
}

#[test]
fn package_without_links_still_loads_and_saves() {
    let scene = scene();
    assert_eq!(load(&scene.package).expect("load").glyphs.len(), 1);
    save(&font_with(&["b"]), &scene.package).expect("save");
    let loaded = load(&scene.package).expect("load");
    assert_eq!(loaded.glyphs.len(), 1);
    assert!(scene.package.join("glyphs/b.json").exists());
    assert!(!scene.package.join("glyphs/a.json").exists());
}
