//! Contact sheets: many glyphs in one image, each in a cell with marks for
//! how it was made and whether it was reviewed, so that an agent or a
//! person reviews a whole alphabet in one look instead of one preview per
//! glyph.
//!
//! There is no text in the image (FORMA draws no system font): cells are
//! in reading order, and whoever asks for the sheet gets the name of each
//! cell alongside it. The marks are shapes in the cell's top-left corner:
//!
//! * origin: a blue disc (traced), a violet diamond (synthesized), a gray
//!   square (composed), nothing (drawn by hand);
//! * review: an orange ring (pending), a green tick bar (approved), a red
//!   cross (rejected), a red triangle (needs a look).

use forma_model::{Font, Glyph, MasterId};
use vello_cpu::kurbo::{Affine, BezPath, Circle, Point, Rect, Shape};

use crate::draw::{BLACK, Canvas, Frame, color, flat_paths, metrics_of, outline};
use crate::{MAX_SIZE, RenderError};

/// The smallest and largest cell, in pixels.
pub const MIN_CELL: u32 = 96;
pub const MAX_CELL: u32 = 256;
/// The largest side of a sheet, in pixels.
pub const MAX_SHEET_SIDE: u32 = 1600;
/// The most cells a sheet holds.
pub const MAX_CELLS: usize = 120;

const GRID_RGB: [u8; 3] = [0xd8, 0xd8, 0xd8];
const OVERLAY_RGB: [u8; 3] = [0xff, 0xc4, 0xb4];
const TRACED_RGB: [u8; 3] = [0x1f, 0x6f, 0xd1];
const SYNTH_RGB: [u8; 3] = [0x8a, 0x3f, 0xd1];
const COMPOSED_RGB: [u8; 3] = [0x80, 0x80, 0x80];
const PENDING_RGB: [u8; 3] = [0xf0, 0x8c, 0x00];
const APPROVED_RGB: [u8; 3] = [0x1f, 0x9d, 0x55];
const ALERT_RGB: [u8; 3] = [0xe5, 0x48, 0x4d];

/// How a glyph was made, as the sheet marks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Origin {
    /// Traced from an image.
    Traced,
    /// Synthesized from the font's style.
    Synthesized,
    /// Built from components.
    Composed,
    /// Drawn by hand (no mark).
    Manual,
}

/// The review state, as the sheet marks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReviewMark {
    /// No mark.
    None,
    /// Not reviewed yet.
    Pending,
    /// Approved.
    Approved,
    /// Rejected.
    Rejected,
    /// The engine asks for a look (a quality gate failed).
    NeedsLook,
}

/// The ink of the image a glyph was traced from, drawn under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    /// Width of `alpha`, in samples.
    pub width: u32,
    /// Height of `alpha`, in samples.
    pub height: u32,
    /// Coverage of each sample, row by row from the top.
    pub alpha: Vec<u8>,
    /// Where the samples go, in font units: left, bottom, right, top.
    pub rect: [f64; 4],
}

/// One cell of a sheet.
#[derive(Clone, Debug)]
pub struct SheetCell<'a> {
    /// The glyph.
    pub glyph: &'a Glyph,
    /// How it was made.
    pub origin: Origin,
    /// Its review state.
    pub review: ReviewMark,
    /// The image ink under it, if any.
    pub overlay: Option<Overlay>,
}

/// A rendered sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetImage {
    /// The PNG.
    pub png: Vec<u8>,
    /// Columns of cells.
    pub columns: u32,
    /// Rows of cells.
    pub rows: u32,
    /// The side of a cell, in pixels.
    pub cell: u32,
}

/// The grid for `n` cells of at most `wanted` pixels in a sheet at most
/// [`MAX_SHEET_SIDE`] on each side: columns, rows and the cell side.
fn grid(n: u32, wanted: u32) -> (u32, u32, u32) {
    let columns = (f64::from(n).sqrt().ceil() as u32).max(1);
    let rows = n.div_ceil(columns).max(1);
    let fit = (MAX_SHEET_SIDE / columns).min(MAX_SHEET_SIDE / rows);
    let cell = wanted.min(fit).clamp(MIN_CELL.min(fit.max(1)), MAX_CELL);
    (columns, rows, cell)
}

fn disc(at: Point, r: f64) -> BezPath {
    Circle::new(at, r).to_path(0.1)
}

fn polygon(points: &[Point]) -> BezPath {
    let mut path = BezPath::new();
    if let Some((&first, rest)) = points.split_first() {
        path.move_to(first);
        for &p in rest {
            path.line_to(p);
        }
        path.close_path();
    }
    path
}

/// Draws the marks of a cell whose top-left corner is `(x, y)`.
fn marks(canvas: &mut Canvas, x: f64, y: f64, unit: f64, origin: Origin, review: ReviewMark) {
    let at = Point::new(x + unit, y + unit);
    let r = unit * 0.6;
    match origin {
        Origin::Traced => canvas.fill(&disc(at, r), Affine::IDENTITY, color(TRACED_RGB)),
        Origin::Synthesized => canvas.fill(
            &polygon(&[
                Point::new(at.x, at.y - r),
                Point::new(at.x + r, at.y),
                Point::new(at.x, at.y + r),
                Point::new(at.x - r, at.y),
            ]),
            Affine::IDENTITY,
            color(SYNTH_RGB),
        ),
        Origin::Composed => canvas.rect(
            Rect::new(at.x - r, at.y - r, at.x + r, at.y + r),
            color(COMPOSED_RGB),
        ),
        _ => {}
    }
    let at = Point::new(x + 3.0 * unit, y + unit);
    match review {
        ReviewMark::Pending => {
            canvas.stroke(
                &disc(at, r),
                Affine::IDENTITY,
                unit * 0.3,
                color(PENDING_RGB),
            );
        }
        ReviewMark::Approved => canvas.rect(
            Rect::new(at.x - r, at.y - r * 0.3, at.x + r, at.y + r * 0.3),
            color(APPROVED_RGB),
        ),
        ReviewMark::Rejected => {
            for (a, b) in [
                (
                    Point::new(at.x - r, at.y - r),
                    Point::new(at.x + r, at.y + r),
                ),
                (
                    Point::new(at.x - r, at.y + r),
                    Point::new(at.x + r, at.y - r),
                ),
            ] {
                canvas.stroke(
                    &polygon(&[a, b]),
                    Affine::IDENTITY,
                    unit * 0.35,
                    color(ALERT_RGB),
                );
            }
        }
        ReviewMark::NeedsLook => canvas.fill(
            &polygon(&[
                Point::new(at.x, at.y - r),
                Point::new(at.x + r, at.y + r),
                Point::new(at.x - r, at.y + r),
            ]),
            Affine::IDENTITY,
            color(ALERT_RGB),
        ),
        _ => {}
    }
}

/// Draws an overlay's samples as small rectangles under the glyph.
fn draw_overlay(canvas: &mut Canvas, overlay: &Overlay, to_px: Affine) {
    let [x0, y0, x1, y1] = overlay.rect;
    let (w, h) = (overlay.width as usize, overlay.height as usize);
    if w == 0 || h == 0 || overlay.alpha.len() != w * h {
        return;
    }
    let (sx, sy) = ((x1 - x0) / w as f64, (y1 - y0) / h as f64);
    for j in 0..h {
        for i in 0..w {
            if overlay.alpha[j * w + i] < 128 {
                continue;
            }
            let (fx, fy) = (x0 + i as f64 * sx, y1 - (j + 1) as f64 * sy);
            let a = to_px * Point::new(fx, fy);
            let b = to_px * Point::new(fx + sx, fy + sy);
            canvas.rect(
                Rect::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)),
                color(OVERLAY_RGB),
            );
        }
    }
}

/// Renders `cells` (at most [`MAX_CELLS`]) as a contact sheet of the
/// glyphs in `master`: each glyph framed like a glyph preview (descender to
/// ascender, its advance wide), in reading order, in square cells of at
/// most `cell` pixels (96..=256 and reduced so the sheet is at
/// most 1600 pixels on each side).
///
/// # Errors
///
/// * [`RenderError::InvalidSize`] for no cells, more than [`MAX_CELLS`], or
///   a `cell` outside 96..=256;
/// * [`RenderError::Geometry`] if a glyph cannot be drawn or framed.
pub fn sheet_png(
    font: &Font,
    master: MasterId,
    cells: &[SheetCell<'_>],
    cell: u32,
) -> Result<SheetImage, RenderError> {
    if cells.is_empty() || cells.len() > MAX_CELLS || !(MIN_CELL..=MAX_CELL).contains(&cell) {
        return Err(RenderError::InvalidSize(cell));
    }
    let n = u32::try_from(cells.len()).map_err(|_| RenderError::InvalidSize(cell))?;
    let (columns, rows, side) = grid(n, cell);
    let (width, height) = (columns * side, rows * side);
    let to_u16 =
        |v: u32| u16::try_from(v.min(MAX_SHEET_SIDE)).map_err(|_| RenderError::InvalidSize(v));
    let mut canvas = Canvas::new(to_u16(width)?, to_u16(height)?);
    let metrics = metrics_of(font, Some(master));
    let side_u16 = u16::try_from(side.min(MAX_SIZE)).map_err(|_| RenderError::InvalidSize(side))?;
    let unit = f64::from(side) / 16.0;
    for (k, c) in cells.iter().enumerate() {
        let k = k as u32;
        let (x, y) = (
            f64::from((k % columns) * side),
            f64::from((k / columns) * side),
        );
        let advance = c
            .glyph
            .layers
            .get(&master)
            .map_or(0.0, |layer| layer.advance_width);
        let frame = Frame::new(&metrics, advance, side_u16)?;
        let to_px = Affine::translate((x, y)) * frame.to_pixels();
        if let Some(overlay) = &c.overlay {
            draw_overlay(&mut canvas, overlay, to_px);
        }
        if c.glyph.layers.contains_key(&master) {
            let path = outline(&c.glyph.name, &flat_paths(font, c.glyph, master)?)?;
            canvas.fill(&path, to_px, BLACK);
        }
        marks(&mut canvas, x, y, unit, c.origin, c.review);
        // Cell borders: right and bottom edges.
        let (r, b) = (x + f64::from(side), y + f64::from(side));
        canvas.rect(Rect::new(r - 1.0, y, r, b), color(GRID_RGB));
        canvas.rect(Rect::new(x, b - 1.0, r, b), color(GRID_RGB));
    }
    Ok(SheetImage {
        png: canvas.finish()?,
        columns,
        rows,
        cell: side,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grids_fit_the_budget() {
        assert_eq!(grid(1, 200), (1, 1, 200));
        let (c, r, s) = grid(120, 256);
        assert!(
            c * s <= MAX_SHEET_SIDE && r * s <= MAX_SHEET_SIDE,
            "{c} {r} {s}"
        );
        assert!(c * r >= 120);
        let (c, r, s) = grid(26, 128);
        assert_eq!((c, r, s), (6, 5, 128));
    }
}
