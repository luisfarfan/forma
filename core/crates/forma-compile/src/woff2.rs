//! WOFF2 compression of a compiled font, in pure Rust (no C++ toolchain is
//! needed on any platform).

use ttf2woff2::BrotliQuality;

use crate::CompileError;

/// Compresses TrueType-flavoured OpenType bytes as WOFF2.
///
/// Brotli runs at quality 11, the maximum (also what fontTools uses by
/// default), single-threaded so that the output is deterministic. Tables are
/// stored without the optional `glyf`/`loca` transform, which ttf2woff2 leaves
/// off by default for compatibility with every decoder.
pub(crate) fn compress(ttf: &[u8]) -> Result<Vec<u8>, CompileError> {
    ttf2woff2::encode(ttf, BrotliQuality::from(11))
        .map_err(|error| CompileError::Woff2(error.to_string()))
}
