//! A `.formapackage` folder in which one file holds arbitrary bytes:
//! `forma_store::load` must return an error or a valid font, never panic, hang
//! or run out of memory.
//!
//! Arbitrary bytes for the whole package would almost never get past the
//! manifest, so the rest of the package stays valid: the first input byte
//! picks which file the remaining bytes replace. The valid files are the seeds
//! in `seeds/load_package/` (a seed is the index of its file plus its contents),
//! which are also the starting corpus of the nightly run.
#![no_main]

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

/// The files of a small valid package, with their paths inside it. The first
/// byte of each seed is its index in this list; the rest is the file.
const FILES: [(&str, &[u8]); 4] = [
    (
        "manifest.json",
        include_bytes!("../seeds/load_package/manifest.seed"),
    ),
    (
        "font.json",
        include_bytes!("../seeds/load_package/font.seed"),
    ),
    (
        "glyphs/A_.json",
        include_bytes!("../seeds/load_package/glyph_a.seed"),
    ),
    (
        "glyphs/o.json",
        include_bytes!("../seeds/load_package/glyph_o.seed"),
    ),
];

/// A scratch folder per process, so parallel fuzzing jobs do not clash.
fn scratch() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("forma-fuzz-package-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create the scratch folder");
        dir
    })
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, replacement)) = data.split_first() else {
        return;
    };
    let replaced = usize::from(selector) % FILES.len();

    let package = scratch().join("input.formapackage");
    let _ = fs::remove_dir_all(&package);
    fs::create_dir_all(package.join("glyphs")).expect("create the package folder");
    for (index, (name, seed)) in FILES.iter().enumerate() {
        let contents = if index == replaced {
            replacement
        } else {
            &seed[1..]
        };
        fs::write(package.join(name), contents).expect("write a package file");
    }
    let _ = forma_store::load(&package);
});
