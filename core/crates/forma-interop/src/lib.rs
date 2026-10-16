//! Interchange between FORMA and other font tools.
//!
//! Today this is the UFO export ([`write_ufo`]): the Unified Font Object
//! folder is both a format users can open in other editors and the source the
//! font compiler is fed.

#![forbid(unsafe_code)]

mod ufo;

pub use ufo::{InteropError, write_ufo};
