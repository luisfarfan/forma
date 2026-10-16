//! Text line previews.

use std::collections::HashMap;

use forma_model::{Font, Glyph, MasterId};
use vello_cpu::kurbo::{Affine, BezPath, Point};

use crate::draw::{BLACK, Canvas, check_size, metrics_of, outline};
use crate::{MAX_TEXT_WIDTH, RenderError};

/// The blank space at each end of the line, as a fraction of its height.
const SIDE_MARGIN: f64 = 0.1;

/// The most characters laid out. Glyphs without advance (or none at all, for
/// empty glyphs) never fill the 2048 px width, so this bounds the work done
/// for very long text.
const MAX_LAID_OUT_CHARS: usize = 4096;

/// A glyph ready to draw: its outline in font units and its advance.
struct Placed {
    path: BezPath,
    advance: f64,
}

/// Renders a line of text as a PNG: black on opaque white.
///
/// `height` is the image height in pixels, which the vertical extent from the
/// default master's descender to its ascender fills exactly. Characters are
/// mapped to glyphs through the glyphs' Unicode values (the model's cmap) and
/// spaced by the advances of the default master. A character without a glyph
/// (or whose glyph has no outline in the default master) is drawn as the
/// `.notdef` glyph when the font has one, and otherwise as a hollow box
/// `units_per_em / 2` wide. The width is the sum of the advances plus a
/// margin of 10% of the height at each end, capped at 2048 px: whatever does
/// not fit is clipped, not an error. At most 4096 characters are laid out.
///
/// # Errors
///
/// * [`RenderError::InvalidSize`] if `height` is outside `16..=1024`;
/// * [`RenderError::InvalidText`] if `text` is empty;
/// * [`RenderError::Geometry`] if a contour of a used glyph cannot be
///   converted to a curve.
pub fn text_png(font: &Font, text: &str, height: u32) -> Result<Vec<u8>, RenderError> {
    let height_px = check_size(height)?;
    if text.is_empty() {
        return Err(RenderError::InvalidText);
    }

    let master = font.masters.first().map(|m| m.id);
    let metrics = metrics_of(font, master);
    let scale = f64::from(height) / (metrics.ascender - metrics.descender);
    let baseline = metrics.ascender * scale;
    let margin = f64::from(height) * SIDE_MARGIN;
    let max_width = f64::from(MAX_TEXT_WIDTH);

    let cmap = cmap(font);
    let fallback = match font.glyph_by_name(".notdef") {
        Some(glyph) => place(glyph, master)?,
        None => None,
    }
    .unwrap_or_else(|| missing_box(font, metrics.cap_height));

    // `placed[0]` is the fallback; every distinct character is resolved once.
    let mut placed = vec![fallback];
    let mut index_of: HashMap<char, usize> = HashMap::new();
    let mut items: Vec<(usize, f64)> = Vec::new();
    let mut pen = margin;
    for ch in text.chars().take(MAX_LAID_OUT_CHARS) {
        if pen >= max_width {
            break;
        }
        let index = match index_of.get(&ch) {
            Some(&index) => index,
            None => {
                let found = match cmap.get(&u32::from(ch)) {
                    Some(glyph) => place(glyph, master)?,
                    None => None,
                };
                let index = found.map_or(0, |p| {
                    placed.push(p);
                    placed.len() - 1
                });
                index_of.insert(ch, index);
                index
            }
        };
        items.push((index, pen));
        pen += placed[index].advance * scale;
    }

    // The epsilon keeps floating-point noise from adding a column.
    let width = (pen + margin - 1e-9).ceil().clamp(1.0, max_width);
    // `width` is within 1..=2048.
    let mut canvas = Canvas::new(width as u16, height_px);
    for (index, x) in items {
        let to_px = Affine::new([scale, 0.0, 0.0, -scale, x, baseline]);
        canvas.fill(&placed[index].path, to_px, BLACK);
    }
    canvas.finish()
}

/// Maps code points to the first glyph (in glyph order) that encodes them.
fn cmap(font: &Font) -> HashMap<u32, &Glyph> {
    let ordered = font
        .glyph_order
        .iter()
        .filter_map(|id| font.glyphs.get(id))
        .chain(font.glyphs.values());
    let mut cmap = HashMap::new();
    for glyph in ordered {
        for &code_point in &glyph.unicodes {
            cmap.entry(code_point).or_insert(&**glyph);
        }
    }
    cmap
}

/// The glyph in the default master, or `None` if it has no layer there.
fn place(glyph: &Glyph, master: Option<MasterId>) -> Result<Option<Placed>, RenderError> {
    let Some(layer) = master.and_then(|m| glyph.layers.get(&m)) else {
        return Ok(None);
    };
    let advance = layer.advance_width;
    Ok(Some(Placed {
        path: outline(&glyph.name, layer)?,
        advance: if advance.is_finite() && advance > 0.0 {
            advance
        } else {
            0.0
        },
    }))
}

/// A hollow rectangle `units_per_em / 2` wide: an outer contour
/// counter-clockwise and an inner one clockwise, like a real `.notdef`.
fn missing_box(font: &Font, cap_height: f64) -> Placed {
    let upm = f64::from(font.units_per_em);
    let advance = upm / 2.0;
    let inset = upm * 0.05;
    let top = if cap_height.is_finite() && cap_height > 2.0 * inset {
        cap_height
    } else {
        upm * 0.7
    };

    let mut path = BezPath::new();
    path.move_to(Point::new(inset, 0.0));
    path.line_to(Point::new(advance - inset, 0.0));
    path.line_to(Point::new(advance - inset, top));
    path.line_to(Point::new(inset, top));
    path.close_path();
    path.move_to(Point::new(2.0 * inset, inset));
    path.line_to(Point::new(2.0 * inset, top - inset));
    path.line_to(Point::new(advance - 2.0 * inset, top - inset));
    path.line_to(Point::new(advance - 2.0 * inset, inset));
    path.close_path();
    Placed { path, advance }
}
