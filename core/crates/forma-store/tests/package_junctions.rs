//! Windows: a `.formapackage` holding a directory junction (the link that
//! `mklink /J` makes without administrator rights) is refused by `load` and
//! `save`, like a symbolic link on Unix (`package_symlinks.rs`), so nothing
//! is read, written or deleted through it.
#![cfg(windows)]

use std::fs;
use std::path::Path;
use std::process::Command;

use forma_model::Font;
use forma_store::{StoreError, disk_state, load, save};
use tempfile::TempDir;

/// Makes `link` a directory junction to `target` with `cmd /C mklink /J`.
fn junction(link: &Path, target: &Path) {
    let output = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("run cmd");
    assert!(
        output.status.success(),
        "mklink /J failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::symlink_metadata(link)
            .expect("junction")
            .file_type()
            .is_symlink(),
        "std sees a junction as a link"
    );
}

#[track_caller]
fn assert_refused<T: std::fmt::Debug>(result: Result<T, StoreError>, link: &Path) {
    match result {
        Err(StoreError::SymlinkInPackage(path)) => assert_eq!(path, link),
        other => panic!(
            "expected SymlinkInPackage({}), got {other:?}",
            link.display()
        ),
    }
}

/// A saved package, and an outside folder holding `important.json`.
fn scene() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = TempDir::new().expect("tempdir");
    let package = tmp.path().join("font.formapackage");
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).expect("mkdir");
    fs::write(outside.join("important.json"), b"{\"keep\": true}").expect("write");
    save(&Font::new("Junctions", 1000), &package).expect("save");
    (tmp, package, outside)
}

#[test]
fn a_glyphs_junction_is_refused() {
    let (_tmp, package, outside) = scene();
    let glyphs = package.join("glyphs");
    fs::remove_dir_all(&glyphs).expect("rmdir");
    junction(&glyphs, &outside);

    assert_refused(load(&package), &glyphs);
    // A save would otherwise delete `important.json` as a stale glyph file.
    assert_refused(save(&Font::new("Other", 1000), &package), &glyphs);
    assert_refused(disk_state(&package), &glyphs);
    assert_eq!(
        fs::read(outside.join("important.json")).expect("kept"),
        b"{\"keep\": true}"
    );
    let names: Vec<_> = fs::read_dir(&outside)
        .expect("read_dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(names, ["important.json"]);
}

#[test]
fn a_junction_inside_glyphs_is_refused() {
    let (_tmp, package, outside) = scene();
    let link = package.join("glyphs").join("notes");
    fs::create_dir_all(package.join("glyphs")).expect("mkdir");
    junction(&link, &outside);

    assert_refused(load(&package), &link);
    assert_refused(save(&Font::new("Other", 1000), &package), &link);
    assert!(outside.join("important.json").is_file());
}
