//! TrueType compilation: the font goes to fontc as a UFO in a temporary
//! folder.

use std::any::Any;
use std::io;
use std::panic;
use std::path::Path;

use fontc::{Input, Options};
use forma_interop::{InteropError, write_ufo};
use forma_model::Font;

use crate::CompileError;

/// The UFO inside the temporary folder; it does not exist until written.
const UFO_NAME: &str = "font.ufo";

/// Compiles the default master of a preflighted font to TrueType-flavoured
/// OpenType bytes.
pub(crate) fn build(font: &Font) -> Result<Vec<u8>, CompileError> {
    let master = font
        .masters
        .first()
        .ok_or_else(|| CompileError::Preflight("the font has no masters".to_owned()))?
        .id;
    // The folder and everything in it are removed when `workspace` is
    // dropped: on success, on error and while unwinding from a panic.
    let workspace = tempfile::Builder::new()
        .prefix("forma-compile-")
        .tempdir()?;
    let ufo = workspace.path().join(UFO_NAME);
    write_ufo(font, master, &ufo).map_err(ufo_error)?;
    run_fontc(&ufo)
}

/// Classifies a failure to write the UFO.
///
/// The preflight has already rejected every font that `write_ufo` refuses
/// (missing layers, invalid names, code points or contours), so what is left is
/// the file system: [`InteropError::Io`], and the rare write failure of norad
/// that has no file system cause ([`InteropError::Norad`]), which is treated
/// as an I/O failure too.
fn ufo_error(error: InteropError) -> CompileError {
    match error {
        InteropError::Io(error) => CompileError::Io(error),
        InteropError::Norad(message) => CompileError::Io(io::Error::other(message)),
        error @ InteropError::DestinationExists(_) => CompileError::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            error.to_string(),
        )),
        other => CompileError::Preflight(other.to_string()),
    }
}

/// Runs fontc on the UFO at `ufo` and returns the font it builds.
fn run_fontc(ufo: &Path) -> Result<Vec<u8>, CompileError> {
    // fontc's default flags (those of its command line) and nothing written to
    // disk: no debug files, and the font comes back as bytes.
    let options = Options {
        compile_debg: false,
        debug_dir: None,
        output_file: None,
        ..Options::default()
    };
    // A panic inside fontc must not take the caller (an editor, a server)
    // down with it; it becomes an error like any other compiler failure.
    let compiled = panic::catch_unwind(|| {
        let source = Input::new(ufo)?.create_source()?;
        fontc::generate_font(source, options)
    });
    match compiled {
        Ok(result) => result.map_err(|error| CompileError::Fontc(error.to_string())),
        Err(payload) => Err(CompileError::Fontc(format!(
            "the compiler panicked: {}",
            panic_message(payload.as_ref())
        ))),
    }
}

/// The message a panic was raised with, when it has one.
fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else {
        "no message"
    }
}
