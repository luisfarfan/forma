//! Image tracing for FORMA: decode a PNG or JPEG with limits, find the
//! background and ink colors, build the ink mask and locate glyph candidates.
#![forbid(unsafe_code)]

mod annotate;
mod candidates;
mod decode;
mod error;
mod mask;
mod palette;

pub use annotate::annotated_png;
pub use candidates::{Analysis, Candidate, PxRect, Row, TraceOptions, analyze};
pub use decode::{MAX_IMAGE_BYTES, MAX_IMAGE_PIXELS, decode};
pub use error::TraceError;
pub use palette::PaletteEntry;

/// The most candidates an analysis may report.
pub const MAX_CANDIDATES: usize = 500;
