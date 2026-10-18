//! Image tracing for FORMA: decode a PNG or JPEG with limits, find the
//! background and ink colors, build the ink mask and locate glyph candidates
//! ([`analyze`]); vectorize candidates into clean contours in font units
//! ([`trace`]), measure how faithful contours are to the image ([`compare`])
//! and measure the stroke style of drawn glyphs ([`measure`]).
#![forbid(unsafe_code)]

mod annotate;
mod candidates;
mod cleanup;
mod decode;
mod error;
mod fidelity;
mod layout;
mod mask;
mod measure;
mod palette;
mod quality;
mod subpixel;
mod vectorize;

pub use annotate::annotated_png;
pub use candidates::{
    Analysis, Candidate, Extractor, Fidelity, PxRect, Row, TraceOptions, analyze,
};
pub use decode::{MAX_IMAGE_BYTES, MAX_IMAGE_PIXELS, decode};
pub use error::TraceError;
pub use fidelity::{Comparison, compare};
pub use layout::{RowMatch, match_rows, typical_stroke_px};
pub use measure::{StyleMeasure, measure};
pub use palette::PaletteEntry;
pub use quality::{FeatureIssue, QualityReport, QualityThresholds, THRESHOLDS};
pub use vectorize::{Placement, TracedGlyph, trace, trace_groups};

/// The most candidates an analysis may report.
pub const MAX_CANDIDATES: usize = 500;
