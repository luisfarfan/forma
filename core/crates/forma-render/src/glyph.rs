//! Single glyph previews.

use forma_model::{Font, Glyph, MasterId};

use crate::draw::{
    BLACK, Canvas, Frame, check_size, draw_metrics, draw_nodes, layer_of, metrics_of, outline,
};
use crate::{PreviewOptions, RenderError};

/// Renders a glyph as a PNG: black on opaque white, in a square image.
///
/// The visible box runs from the master's descender to its ascender
/// vertically and from 0 to the glyph's advance horizontally (a glyph without
/// advance gets a box as wide as it is tall, centred on x = 0), with a 10%
/// margin, scaled uniformly to the image and centred. A `master` the font does
/// not have falls back to the default master's metrics. If the master's
/// ascender and descender are unusable (not finite, or the ascender is not
/// above the descender), proportional metrics are used instead: ascender 80%
/// and descender -20% of the units per em.
///
/// # Errors
///
/// * [`RenderError::InvalidSize`] if `opts.size` is outside `16..=1024`;
/// * [`RenderError::MissingLayer`] if the glyph has no layer for `master`;
/// * [`RenderError::Geometry`] if a contour cannot be converted to a curve, or
///   if the box cannot be framed ("degenerate framing": the descender-to-
///   ascender extent or the advance is zero, overflows, or maps to fewer than
///   1e-6 or more than 1e6 pixels per font unit).
pub fn glyph_png(
    font: &Font,
    glyph: &Glyph,
    master: MasterId,
    opts: &PreviewOptions,
) -> Result<Vec<u8>, RenderError> {
    let size = check_size(opts.size)?;
    let layer = layer_of(glyph, master)?;
    let path = outline(&glyph.name, layer)?;

    let metrics = metrics_of(font, Some(master));
    let frame = Frame::new(&metrics, layer.advance_width, size)?;
    let to_px = frame.to_pixels();

    let mut canvas = Canvas::new(size, size);
    if opts.show_metrics {
        draw_metrics(&mut canvas, &frame, &metrics, layer.advance_width);
    }
    canvas.fill(&path, to_px, BLACK);
    if opts.show_nodes {
        draw_nodes(&mut canvas, layer, to_px);
    }
    canvas.finish()
}
