//! Before/after glyph comparison.

use forma_model::{Font, MasterId};

use crate::draw::{
    Canvas, Color, Frame, check_size, draw_metrics, draw_nodes, metrics_of, outline,
};
use crate::{PreviewOptions, RenderError};

/// The stroke width of the `before` outline, in pixels.
const BEFORE_STROKE_PX: f64 = 2.0;
/// The `before` outline: red, #e02020.
const BEFORE_COLOR: Color = Color::from_rgb8(0xe0, 0x20, 0x20);
/// The `after` fill: black at 60% opacity.
const AFTER_COLOR: Color = Color::from_rgba8(0, 0, 0, 153);

/// Renders two versions of a glyph over each other as a PNG.
///
/// The glyph is looked up by `glyph` name in each font. `before` is drawn as
/// a 2-pixel red outline and `after` as a black fill at 60% opacity, with the
/// framing of [`glyph_png`](crate::glyph_png) taken from `after` (from
/// `before` when `after` has no such glyph). A glyph present in one font only
/// is drawn alone. `show_metrics` adds the guides and `show_nodes` the nodes
/// of the glyph that sets the framing.
///
/// # Errors
///
/// * [`RenderError::InvalidSize`] if `opts.size` is outside `16..=1024`;
/// * [`RenderError::GlyphNotFound`] if neither font has the glyph;
/// * [`RenderError::MissingLayer`] if the glyph exists but has no layer for
///   `master` in either font;
/// * [`RenderError::Geometry`] if a contour cannot be converted to a curve.
pub fn compare_png(
    before: &Font,
    after: &Font,
    glyph: &str,
    master: MasterId,
    opts: &PreviewOptions,
) -> Result<Vec<u8>, RenderError> {
    let size = check_size(opts.size)?;

    let before_glyph = before.glyph_by_name(glyph);
    let after_glyph = after.glyph_by_name(glyph);
    if before_glyph.is_none() && after_glyph.is_none() {
        return Err(RenderError::GlyphNotFound(glyph.to_owned()));
    }
    let before_layer = before_glyph.and_then(|g| g.layers.get(&master));
    let after_layer = after_glyph.and_then(|g| g.layers.get(&master));
    let (frame_font, frame_layer) = match (after_layer, before_layer) {
        (Some(layer), _) => (after, layer),
        (None, Some(layer)) => (before, layer),
        (None, None) => {
            return Err(RenderError::MissingLayer {
                glyph: glyph.to_owned(),
            });
        }
    };
    let before_path = before_layer.map(|l| outline(glyph, l)).transpose()?;
    let after_path = after_layer.map(|l| outline(glyph, l)).transpose()?;

    let metrics = metrics_of(frame_font, Some(master));
    let frame = Frame::new(&metrics, frame_layer.advance_width);
    let size_px = f64::from(size);
    let to_px = frame.to_pixels(size_px);

    let mut canvas = Canvas::new(size, size);
    if opts.show_metrics {
        draw_metrics(
            &mut canvas,
            &frame,
            size_px,
            &metrics,
            frame_layer.advance_width,
        );
    }
    if let Some(path) = &after_path {
        canvas.fill(path, to_px, AFTER_COLOR);
    }
    if let Some(path) = &before_path {
        // The outline is stroked in pixel space so it is 2 pixels wide at
        // every size.
        canvas.stroke(path, to_px, BEFORE_STROKE_PX, BEFORE_COLOR);
    }
    if opts.show_nodes {
        draw_nodes(&mut canvas, frame_layer, to_px);
    }
    canvas.finish()
}
