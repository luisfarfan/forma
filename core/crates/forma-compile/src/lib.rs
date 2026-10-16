//! Font compilation for FORMA: turns the edited model into an installable font.
//!
//! [`compile`] runs the [`preflight()`], writes the default master of the
//! preflighted font as a UFO into a private temporary folder, compiles that
//! with [fontc](https://github.com/googlefonts/fontc) used as a library, and
//! returns TrueType-flavoured OpenType bytes, compressed to WOFF2 on request.
//!
//! Nothing is written anywhere but that temporary folder, which is removed
//! when the compilation ends, whether it succeeds or fails. fontc runs with its
//! default flags, the same defaults as its command line: among others, contours
//! are reversed from the PostScript direction of the model to the clockwise
//! outer contours of TrueType, as fontmake also does.

#![forbid(unsafe_code)]

mod preflight;
mod ttf;
mod woff2;

use forma_model::Font;
use thiserror::Error;

pub use preflight::preflight;

/// The binary format to compile to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OutputFormat {
    /// OpenType with TrueType (quadratic) outlines, as written to a `.ttf`.
    Ttf,
    /// The same font compressed as WOFF2, for the web.
    Woff2,
}

/// What a compilation did besides producing the font.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompileReport {
    /// Warnings about the compiled font. fontc reports none through its
    /// library interface, so today this is always empty.
    pub warnings: Vec<String>,
    /// The names of the glyphs the preflight added (`.notdef`, `space`), in
    /// the order they were added.
    pub added_glyphs: Vec<String>,
}

/// Why a compilation failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CompileError {
    /// The font breaks a model rule or holds data that cannot be compiled;
    /// the message says what and where.
    #[error("the font cannot be compiled: {0}")]
    Preflight(String),
    /// fontc rejected the font or failed while compiling it.
    #[error("the font compiler failed: {0}")]
    Fontc(String),
    /// The compiled font could not be compressed as WOFF2.
    #[error("WOFF2 compression failed: {0}")]
    Woff2(String),
    /// Writing the temporary compiler input failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Compiles the default master of `font` to `format`.
///
/// The font is first passed through [`preflight()`], which checks it and adds a
/// `.notdef` and a `space` if it lacks them; the returned report lists what was
/// added. Only glyphs marked for export end up in the binary, in glyph order
/// after `.notdef`, which is always glyph 0. (A font's own `.notdef` that is
/// not marked for export is left out like any other such glyph, and fontc then
/// generates a `.notdef` of its own.) `font` itself is not changed.
///
/// The binary is not byte-for-byte reproducible: fontc stamps the current time
/// into `head` unless the `SOURCE_DATE_EPOCH` environment variable is set.
///
/// # Errors
///
/// - [`CompileError::Preflight`] if the preflight rejects the font;
/// - [`CompileError::Io`] if the temporary UFO cannot be written;
/// - [`CompileError::Fontc`] if fontc fails (including if it panics);
/// - [`CompileError::Woff2`] if WOFF2 compression fails.
pub fn compile(
    font: &Font,
    format: OutputFormat,
) -> Result<(Vec<u8>, CompileReport), CompileError> {
    let (font, report) = preflight(font)?;
    let ttf = ttf::build(&font)?;
    let bytes = match format {
        OutputFormat::Ttf => ttf,
        OutputFormat::Woff2 => woff2::compress(&ttf)?,
    };
    Ok((bytes, report))
}
