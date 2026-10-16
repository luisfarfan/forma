//! Framing and drawing code shared by every preview.
//!
//! Everything is drawn single-threaded and at the baseline SIMD level of the
//! compilation target, so a given build renders the same input to the same
//! bytes on any machine of that platform, whatever CPU extensions it has.

use forma_geometry::to_bezpath;
use forma_model::{Font, Glyph, Layer, MasterId, Node, NodeKind, Path, VerticalMetrics};
use vello_cpu::color::{AlphaColor, Srgb};
use vello_cpu::kurbo::{Affine, BezPath, Point, Rect, Stroke};
use vello_cpu::peniko::Fill;
use vello_cpu::{
    Level, Pixmap, RasterizerSettings, RenderContext, RenderSettings, Resources, TargetInit,
};

use crate::{MAX_SIZE, MIN_SIZE, RenderError};

pub(crate) type Color = AlphaColor<Srgb>;

pub(crate) const WHITE: Color = Color::from_rgb8(255, 255, 255);
pub(crate) const BLACK: Color = Color::from_rgb8(0, 0, 0);

/// Metric guides.
pub(crate) const METRICS_RGB: [u8; 3] = [0xb0, 0xb0, 0xb0];
/// Handle lines between off-curve and on-curve nodes.
pub(crate) const HANDLE_RGB: [u8; 3] = [0xa0, 0xa0, 0xa0];
/// Off-curve nodes.
pub(crate) const OFF_CURVE_RGB: [u8; 3] = [0xa0, 0xa0, 0xa0];
/// On-curve nodes.
pub(crate) const ON_CURVE_RGB: [u8; 3] = [0x1f, 0x4f, 0xd1];

/// The side of a node square, in pixels.
pub(crate) const NODE_SIDE_PX: f64 = 4.0;

/// The margin added on every side of the visible box, as a fraction of its
/// larger dimension.
const MARGIN: f64 = 0.1;

pub(crate) fn color(rgb: [u8; 3]) -> Color {
    Color::from_rgb8(rgb[0], rgb[1], rgb[2])
}

/// Validates a preview size and narrows it to what the renderer takes.
pub(crate) fn check_size(size: u32) -> Result<u16, RenderError> {
    if !(MIN_SIZE..=MAX_SIZE).contains(&size) {
        return Err(RenderError::InvalidSize(size));
    }
    u16::try_from(size).map_err(|_| RenderError::InvalidSize(size))
}

/// The outline of `glyph` in `master`.
pub(crate) fn layer_of(glyph: &Glyph, master: MasterId) -> Result<&Layer, RenderError> {
    glyph
        .layers
        .get(&master)
        .ok_or_else(|| RenderError::MissingLayer {
            glyph: glyph.name.clone(),
        })
}

/// All contours of a layer as one path, so that the fill rule sees them
/// together.
pub(crate) fn outline(glyph_name: &str, layer: &Layer) -> Result<BezPath, RenderError> {
    let mut all = BezPath::new();
    for path in &layer.paths {
        let bez = to_bezpath(path).map_err(|e| {
            RenderError::Geometry(format!("glyph {glyph_name:?}, contour {}: {e}", path.id.0))
        })?;
        all.extend(bez.elements().iter().copied());
    }
    Ok(all)
}

/// Metrics proportional to the units per em, as [`Font::new`] sets them.
fn proportional_metrics(units_per_em: u16) -> VerticalMetrics {
    let upm = f64::from(units_per_em);
    VerticalMetrics {
        ascender: upm * 4.0 / 5.0,
        descender: -upm / 5.0,
        x_height: upm / 2.0,
        cap_height: upm * 7.0 / 10.0,
    }
}

/// The vertical metrics of `master`, or of the default master when the font
/// has no such master (or `master` is `None`), with an unusable
/// ascender/descender pair (not finite, or not ascending) replaced by
/// proportional ones.
pub(crate) fn metrics_of(font: &Font, master: Option<MasterId>) -> VerticalMetrics {
    let found = master
        .and_then(|id| font.masters.iter().find(|m| m.id == id))
        .or_else(|| font.masters.first());
    let mut metrics = found.map_or_else(
        || proportional_metrics(font.units_per_em),
        |m| m.metrics.clone(),
    );
    let usable = metrics.ascender.is_finite()
        && metrics.descender.is_finite()
        && metrics.ascender > metrics.descender;
    if !usable {
        let fallback = proportional_metrics(font.units_per_em);
        metrics.ascender = fallback.ascender;
        metrics.descender = fallback.descender;
    }
    metrics
}

/// The part of the font plane a glyph preview shows: a square, in font units,
/// centred on the glyph box with a margin around it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    /// Horizontal centre.
    pub cx: f64,
    /// Vertical centre.
    pub cy: f64,
    /// Side of the square.
    pub side: f64,
}

impl Frame {
    /// The box runs from the descender to the ascender vertically and from 0
    /// to `advance` horizontally. A glyph without advance gets a box as wide
    /// as it is tall, centred on x = 0. `metrics` must be as returned by
    /// [`metrics_of`].
    pub(crate) fn new(metrics: &VerticalMetrics, advance: f64) -> Frame {
        let height = metrics.ascender - metrics.descender;
        let (width, cx) = if advance.is_finite() && advance > 0.0 {
            (advance, advance / 2.0)
        } else {
            (height, 0.0)
        };
        Frame {
            cx,
            cy: (metrics.ascender + metrics.descender) / 2.0,
            side: width.max(height) * (1.0 + 2.0 * MARGIN),
        }
    }

    /// Pixels per font unit on a square canvas of `size` pixels.
    fn scale(self, size: f64) -> f64 {
        size / self.side
    }

    /// Maps font coordinates (y up) to canvas pixels (y down), centring the
    /// frame on a square canvas of `size` pixels.
    pub(crate) fn to_pixels(self, size: f64) -> Affine {
        let s = self.scale(size);
        Affine::new([
            s,
            0.0,
            0.0,
            -s,
            size / 2.0 - s * self.cx,
            size / 2.0 + s * self.cy,
        ])
    }
}

/// An RGB image being drawn on a white background.
pub(crate) struct Canvas {
    ctx: RenderContext,
    width: u16,
    height: u16,
}

impl Canvas {
    pub(crate) fn new(width: u16, height: u16) -> Canvas {
        let settings = RenderSettings {
            level: Level::baseline(),
            num_threads: 0,
        };
        let mut ctx = RenderContext::new_with(width, height, settings);
        // The rule font rasterizers use: a contour of the wrong direction
        // still fills, and overlapping contours of one direction merge.
        ctx.set_fill_rule(Fill::NonZero);
        Canvas { ctx, width, height }
    }

    /// Fills `path`, given in the coordinates `to_pixels` maps to pixels.
    pub(crate) fn fill(&mut self, path: &BezPath, to_pixels: Affine, color: Color) {
        if path.elements().is_empty() {
            return;
        }
        self.ctx.set_transform(to_pixels);
        self.ctx.set_paint(color);
        self.ctx.fill_path(path);
        self.ctx.reset_transform();
    }

    /// Strokes `path`, given in the coordinates `to_pixels` maps to pixels,
    /// with a stroke `width_px` pixels wide whatever the scale.
    pub(crate) fn stroke(
        &mut self,
        path: &BezPath,
        to_pixels: Affine,
        width_px: f64,
        color: Color,
    ) {
        if path.elements().is_empty() {
            return;
        }
        let mut in_pixels = path.clone();
        in_pixels.apply_affine(to_pixels);
        self.ctx.set_stroke(Stroke::new(width_px));
        self.ctx.set_paint(color);
        self.ctx.stroke_path(&in_pixels);
    }

    /// Fills a rectangle given in pixels.
    pub(crate) fn rect(&mut self, rect: Rect, color: Color) {
        self.ctx.set_paint(color);
        self.ctx.fill_rect(&rect);
    }

    /// Draws a 1-pixel line between two points given in pixels.
    pub(crate) fn line(&mut self, from: Point, to: Point, color: Color) {
        let mut path = BezPath::new();
        path.move_to(from);
        path.line_to(to);
        self.stroke(&path, Affine::IDENTITY, 1.0, color);
    }

    /// Rasterizes the scene and encodes it as a PNG.
    pub(crate) fn finish(mut self) -> Result<Vec<u8>, RenderError> {
        let mut pixmap = Pixmap::new(self.width, self.height);
        self.ctx.flush();
        let settings = RasterizerSettings {
            target_init: TargetInit::Clear(WHITE),
            ..RasterizerSettings::default()
        };
        self.ctx
            .render_with(&mut pixmap, &mut Resources::new(), settings);
        pixmap
            .into_png()
            .map_err(|e| RenderError::Encode(e.to_string()))
    }
}

/// Draws the metric guides: horizontal lines at the baseline, x-height, cap
/// height, ascender and descender, vertical ones at the origin and the
/// advance.
pub(crate) fn draw_metrics(
    canvas: &mut Canvas,
    frame: &Frame,
    size: f64,
    metrics: &VerticalMetrics,
    advance: f64,
) {
    let to_px = frame.to_pixels(size);
    let guide = color(METRICS_RGB);

    let heights = [
        0.0,
        metrics.x_height,
        metrics.cap_height,
        metrics.ascender,
        metrics.descender,
    ];
    for y in heights {
        let row = (to_px * Point::new(0.0, y)).y.floor();
        if (0.0..size).contains(&row) {
            canvas.rect(Rect::new(0.0, row, size, row + 1.0), guide);
        }
    }

    let advance = (advance.is_finite() && advance > 0.0).then_some(advance);
    for x in std::iter::once(0.0).chain(advance) {
        let col = (to_px * Point::new(x, 0.0)).x.floor();
        if (0.0..size).contains(&col) {
            canvas.rect(Rect::new(col, 0.0, col + 1.0, size), guide);
        }
    }
}

/// The off-curve nodes of a contour paired with each on-curve node they
/// directly neighbour: the control handles.
pub(crate) fn handles(path: &Path) -> Vec<(&Node, &Node)> {
    let nodes = &path.nodes;
    let n = nodes.len();
    let wraps = path.closed && n > 1;
    let mut pairs = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        if node.kind != NodeKind::OffCurve {
            continue;
        }
        let prev = if i > 0 {
            Some(i - 1)
        } else if wraps {
            Some(n - 1)
        } else {
            None
        };
        let next = if i + 1 < n {
            Some(i + 1)
        } else if wraps {
            Some(0)
        } else {
            None
        };
        for neighbour in [prev, next].into_iter().flatten() {
            let other = &nodes[neighbour];
            if other.kind != NodeKind::OffCurve {
                pairs.push((node, other));
            }
        }
    }
    pairs
}

/// Draws the nodes of a layer: handle lines first, then 4-pixel squares, dark
/// blue for on-curve nodes and light gray for off-curve ones.
pub(crate) fn draw_nodes(canvas: &mut Canvas, layer: &Layer, to_px: Affine) {
    let at = |node: &Node| to_px * Point::new(node.x, node.y);

    for path in &layer.paths {
        for (off, on) in handles(path) {
            canvas.line(at(off), at(on), color(HANDLE_RGB));
        }
    }
    for path in &layer.paths {
        for node in &path.nodes {
            let rgb = if node.kind == NodeKind::OffCurve {
                OFF_CURVE_RGB
            } else {
                ON_CURVE_RGB
            };
            // Snap to the pixel grid so the square is exactly 4 x 4 pixels.
            let centre = at(node);
            let (x, y) = (centre.x.round(), centre.y.round());
            let half = NODE_SIDE_PX / 2.0;
            canvas.rect(
                Rect::new(x - half, y - half, x + half, y + half),
                color(rgb),
            );
        }
    }
}
