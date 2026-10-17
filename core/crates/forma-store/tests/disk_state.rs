//! `disk_state`: what a document looks like on disk, to tell whether another
//! program changed it while FORMA had it open.

use std::fs;

use forma_model::Font;
use forma_store::{StoreError, disk_state, save};
use tempfile::TempDir;

#[test]
fn a_forma_file_changes_state_when_it_is_rewritten() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("font.forma");
    assert_eq!(disk_state(&path).expect("state"), None);

    save(&Font::new("Disk", 1000), &path).expect("save");
    let saved = disk_state(&path).expect("state").expect("a file");
    assert_eq!(disk_state(&path).expect("state"), Some(saved.clone()));

    save(
        &Font::new("Rewritten with a longer family name", 1000),
        &path,
    )
    .expect("save");
    assert_ne!(disk_state(&path).expect("state"), Some(saved));
}

#[test]
fn a_package_changes_state_when_a_part_is_added_or_removed() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("font.formapackage");
    assert_eq!(disk_state(&path).expect("state"), None);

    save(&Font::new("Disk", 1000), &path).expect("save");
    let saved = disk_state(&path).expect("state").expect("a package");

    let extra = path.join("glyphs").join("extra.json");
    fs::write(&extra, b"{}").expect("write");
    assert_ne!(disk_state(&path).expect("state"), Some(saved.clone()));
    fs::remove_file(&extra).expect("remove");
    assert_eq!(disk_state(&path).expect("state"), Some(saved.clone()));

    // Files that are not parts of the package do not count.
    fs::write(path.join("notes.txt"), b"mine").expect("write");
    fs::write(path.join("glyphs").join("notes.txt"), b"mine").expect("write");
    assert_eq!(disk_state(&path).expect("state"), Some(saved.clone()));

    fs::remove_file(path.join("font.json")).expect("remove");
    assert_ne!(disk_state(&path).expect("state"), Some(saved));
}

#[test]
fn other_extensions_are_not_documents() {
    let dir = TempDir::new().expect("tempdir");
    let error = disk_state(&dir.path().join("font.txt")).expect_err("not a document");
    assert!(matches!(error, StoreError::UnknownFormat(_)), "{error:?}");
}

#[cfg(unix)]
#[test]
fn nothing_is_looked_at_through_a_link_in_a_package() {
    let dir = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("tempdir");
    let path = dir.path().join("font.formapackage");
    save(&Font::new("Disk", 1000), &path).expect("save");
    fs::remove_dir(path.join("glyphs")).expect("rmdir");
    std::os::unix::fs::symlink(outside.path(), path.join("glyphs")).expect("symlink");
    let error = disk_state(&path).expect_err("a link");
    assert!(
        matches!(error, StoreError::SymlinkInPackage(_)),
        "{error:?}"
    );
}

#[test]
fn hidden_files_of_a_package_do_not_count() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("font.formapackage");
    save(&Font::new("Disk", 1000), &path).expect("save");
    let saved = disk_state(&path).expect("state");
    fs::write(path.join("glyphs").join("._a.json"), b"AppleDouble").expect("write");
    fs::write(path.join("glyphs").join(".DS_Store"), b"Finder").expect("write");
    assert_eq!(disk_state(&path).expect("state"), saved);
}
