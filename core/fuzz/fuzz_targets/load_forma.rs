//! Arbitrary bytes as a `.forma` file (an SQLite database): `forma_store::load`
//! must return an error or a valid font, never panic, hang or run out of memory.
//!
//! The input goes to a file in a scratch folder, because the loader takes a path.
#![no_main]

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

/// A scratch folder per process, so parallel fuzzing jobs do not clash.
fn scratch() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("forma-fuzz-forma-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create the scratch folder");
        dir
    })
}

fuzz_target!(|data: &[u8]| {
    let path = scratch().join("input.forma");
    // SQLite may leave side files next to a database it could not open cleanly.
    for suffix in ["-journal", "-wal", "-shm"] {
        let _ = fs::remove_file(scratch().join(format!("input.forma{suffix}")));
    }
    fs::write(&path, data).expect("write the input file");
    let _ = forma_store::load(&path);
});
