//! Deterministic CPU previews of glyphs and text for FORMA.
//!
//! Renders glyph outlines headlessly (no GPU, no windowing) with `vello_cpu`
//! in single-threaded mode, so the same input produces the same PNG bytes on
//! the same platform. The previews are what AI agents and the CLI "see" when
//! they edit a font:
//!
//! * [`glyph_png`] and [`glyph_svg`] draw one glyph, optionally with metric
//!   guides and nodes;
//! * [`text_png`] lays out a line of text with the font's cmap and advances;
//! * [`coverage_mask`] and [`mask_iou`] rasterize outlines to numeric
//!   coverage and compare them;
//! * [`compare_png`] overlays two versions of a glyph.
//!
//! Outlines are filled with the non-zero winding rule, like font rasterizers
//! do, in black on an opaque white background. Coordinates follow the model:
//! `f64` font units with the y axis pointing up.

#![forbid(unsafe_code)]

mod compare;
mod draw;
mod glyph;
mod mask;
mod sheet;
mod svg;
mod text;

pub use compare::compare_png;
pub use glyph::glyph_png;
pub use mask::{MAX_MASK_PIXELS, Mask, coverage_mask, mask_iou};
pub use sheet::{
    MAX_CELL, MAX_CELLS, MAX_SHEET_SIDE, MIN_CELL, Origin, Overlay, ReviewMark, SheetCell,
    SheetImage, sheet_png,
};
pub use svg::glyph_svg;
pub use text::{MAX_WRAPPED_HEIGHT, WrappedText, text_png, text_png_wrapped};

/// The smallest preview size, in pixels.
pub const MIN_SIZE: u32 = 16;
/// The largest preview size, in pixels.
pub const MAX_SIZE: u32 = 1024;
/// The default preview size, in pixels.
pub const DEFAULT_SIZE: u32 = 512;
/// The widest image [`text_png`] produces, in pixels; longer lines are clipped.
pub const MAX_TEXT_WIDTH: u32 = 2048;

/// Options for glyph previews.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewOptions {
    /// Side of the square image in pixels, within [`MIN_SIZE`]`..=`[`MAX_SIZE`].
    pub size: u32,
    /// Draw the nodes: on-curve points as dark blue squares, off-curve points
    /// as light gray squares joined to their on-curve neighbours.
    pub show_nodes: bool,
    /// Draw 1-pixel gray guides at the baseline, x-height, cap height,
    /// ascender and descender, and at the glyph origin and advance.
    pub show_metrics: bool,
}

impl Default for PreviewOptions {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
            show_nodes: false,
            show_metrics: false,
        }
    }
}

/// An error raised while rendering a preview.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    /// The requested size is outside the allowed range.
    #[error("size {0} is outside the allowed range {MIN_SIZE}..={MAX_SIZE}")]
    InvalidSize(u32),
    /// The text to render is empty.
    #[error("the text to render is empty")]
    InvalidText,
    /// No glyph with the given name exists.
    #[error("glyph {0:?} not found")]
    GlyphNotFound(String),
    /// The glyph has no outline for the requested master.
    #[error("glyph {glyph:?} has no layer for the requested master")]
    MissingLayer {
        /// The name of the glyph.
        glyph: String,
    },
    /// A contour cannot be converted to a curve.
    #[error("invalid outline: {0}")]
    Geometry(String),
    /// The image could not be encoded.
    #[error("PNG encoding failed: {0}")]
    Encode(String),
}
