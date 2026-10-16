//! WOFF2 compression of a compiled font.

use crate::CompileError;

/// Brotli quality, from 0 to 11: the maximum, which is also what fontTools
/// uses by default. Fonts are small, so the extra time is negligible.
const BROTLI_QUALITY: usize = 11;

/// Compresses TrueType-flavoured OpenType bytes as WOFF2, with the `glyf` and
/// `loca` transforms that make WOFF2 smaller, and no extended metadata.
pub(crate) fn compress(ttf: &[u8]) -> Result<Vec<u8>, CompileError> {
    woofwoof::compress(ttf, "", BROTLI_QUALITY, true).ok_or_else(|| {
        CompileError::Woff2("the WOFF2 encoder could not compress the compiled font".to_owned())
    })
}
