//! Arbitrary bytes as an image: `forma_trace::decode` must return an error or
//! an image within its limits, and `analyze` on a decoded image must return an
//! error or an analysis, never panic, hang or run out of memory.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(image) = forma_trace::decode(data) {
        let _ = forma_trace::analyze(&image, &forma_trace::TraceOptions::default());
    }
});
