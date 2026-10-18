//! A scene: glyph shapes in image pixels, a look (paper, ink, illumination)
//! and a layout. It renders to an image and yields its own ground truth.

use std::collections::BTreeMap;

use image::{Rgb, RgbImage};
use kurbo::{Affine, BezPath};

use crate::corpus::{Truth, TruthGlyph};
use crate::degrade::{blur_gray, encode_png};
use crate::det::{cos, sin};
use crate::error::BenchError;
use crate::metrics::{bounds, count_holes, rasterize};

/// The pieces of one character.
#[derive(Clone, Debug)]
pub struct SceneGlyph {
    /// The character (layout key).
    pub key: String,
    /// The pieces, solids and holes with opposite orientations.
    pub shapes: Vec<BezPath>,
    /// The pen position `[x, baseline y]` if known.
    pub origin: Option<[f64; 2]>,
    /// The advance in pixels if known.
    pub advance: Option<f64>,
}

/// How the scene looks.
#[derive(Clone, Debug)]
pub struct Look {
    /// Background color.
    pub paper: [u8; 3],
    /// Ink color.
    pub ink: [u8; 3],
    /// Illumination at the left and right edge (1.0 is full), linear between.
    pub light: Option<(f64, f64)>,
    /// Spread the ink into the paper (blur and contrast), as a bleeding ink.
    pub bleed: bool,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            paper: [245, 242, 235],
            ink: [28, 28, 32],
            light: None,
            bleed: false,
        }
    }
}

/// A scene.
#[derive(Clone, Debug)]
pub struct Scene {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// The glyphs.
    pub glyphs: Vec<SceneGlyph>,
    /// Pieces drawn in the image that belong to no glyph (dust).
    pub extras: Vec<BezPath>,
    /// The characters of each row.
    pub layout: Vec<String>,
    /// The look.
    pub look: Look,
    /// Injected irregularity, in pixels.
    pub irregularity: Option<f64>,
    /// Pixels per font unit and cap height in pixels, for font alphabets.
    pub font_scale: Option<(f64, f64)>,
}

impl Scene {
    /// A scene without glyphs.
    #[must_use]
    pub fn new(width: u32, height: u32, layout: Vec<String>) -> Self {
        Self {
            width,
            height,
            glyphs: Vec::new(),
            extras: Vec::new(),
            layout,
            look: Look::default(),
            irregularity: None,
            font_scale: None,
        }
    }

    /// The scene rotated by `degrees` about the center of the image (with
    /// platform-independent trigonometry).
    #[must_use]
    pub fn rotated(mut self, degrees: f64) -> Self {
        let a = degrees * std::f64::consts::PI / 180.0;
        let (s, c) = (sin(a), cos(a));
        let (cx, cy) = (f64::from(self.width) / 2.0, f64::from(self.height) / 2.0);
        let affine = Affine::new([c, s, -s, c, cx - c * cx + s * cy, cy - s * cx - c * cy]);
        for glyph in &mut self.glyphs {
            for shape in &mut glyph.shapes {
                *shape = affine * shape.clone();
            }
            if let Some(o) = glyph.origin {
                let p = affine * kurbo::Point::new(o[0], o[1]);
                glyph.origin = Some([p.x, p.y]);
            }
        }
        for shape in &mut self.extras {
            *shape = affine * shape.clone();
        }
        self
    }

    /// Renders the scene to an image.
    ///
    /// # Errors
    ///
    /// [`BenchError::Geometry`] if a shape is invalid.
    pub fn render(&self) -> Result<RgbImage, BenchError> {
        let all: Vec<BezPath> = self
            .glyphs
            .iter()
            .flat_map(|g| g.shapes.iter().cloned())
            .chain(self.extras.iter().cloned())
            .collect();
        let mut coverage = rasterize(&all, self.width, self.height)?.data;
        if self.look.bleed {
            coverage = bleed(&coverage, self.width, self.height);
        }
        let mut img = RgbImage::new(self.width, self.height);
        let (w, h) = (self.width as usize, self.height as usize);
        for y in 0..h {
            for x in 0..w {
                let a = u32::from(coverage[y * w + x]);
                let gain = match self.look.light {
                    Some((l, r)) => {
                        let t = x as f64 / (w.max(2) - 1) as f64;
                        ((l + (r - l) * t) * 256.0).round() as u32
                    }
                    None => 256,
                };
                let mut px = [0u8; 3];
                for ((out, &p), &i) in px.iter_mut().zip(&self.look.paper).zip(&self.look.ink) {
                    let mixed = (u32::from(p) * (255 - a) + u32::from(i) * a + 127) / 255;
                    *out = ((mixed * gain + 128) >> 8).min(255) as u8;
                }
                img.put_pixel(x as u32, y as u32, Rgb(px));
            }
        }
        Ok(img)
    }

    /// The ground truth of the scene.
    ///
    /// # Errors
    ///
    /// [`BenchError::Geometry`] if a shape is invalid.
    pub fn truth(&self) -> Result<Truth, BenchError> {
        let mut glyphs = BTreeMap::new();
        for glyph in &self.glyphs {
            let mut path = BezPath::new();
            for shape in &glyph.shapes {
                path.extend(shape.elements().iter().copied());
            }
            let b = bounds(&glyph.shapes).unwrap_or_default();
            glyphs.insert(
                glyph.key.clone(),
                TruthGlyph {
                    holes: count_holes(&glyph.shapes)?,
                    px_rect: [b.x0, b.y0, b.x1, b.y1],
                    pieces: count_pieces(&glyph.shapes)?,
                    origin_px: glyph.origin,
                    advance_px: glyph.advance,
                    path,
                },
            );
        }
        Ok(Truth {
            glyphs,
            irregularity_amplitude_px: self.irregularity,
            scale_px_per_unit: self.font_scale.map(|(s, _)| s),
            cap_height_px: self.font_scale.map(|(_, c)| c),
            stray_pieces: self.extras.len(),
        })
    }

    /// The scene as PNG bytes.
    ///
    /// # Errors
    ///
    /// Any error of [`Scene::render`] or of the encoder.
    pub fn png(&self) -> Result<Vec<u8>, BenchError> {
        encode_png(&self.render()?)
    }
}

/// The number of separate pieces of ink in `shapes` (8-connected components
/// of the foreground, measured on a raster of the shapes).
fn count_pieces(shapes: &[BezPath]) -> Result<usize, BenchError> {
    let Some(frame) = bounds(shapes) else {
        return Ok(0);
    };
    let pad = 3.0;
    let map = Affine::translate((pad - frame.x0, pad - frame.y0));
    let moved: Vec<BezPath> = shapes.iter().map(|s| map * s.clone()).collect();
    let w = (frame.width() + 2.0 * pad).ceil().max(1.0) as u32;
    let h = (frame.height() + 2.0 * pad).ceil().max(1.0) as u32;
    let mask = rasterize(&moved, w, h)?;
    let (w, h) = (w as usize, h as usize);
    let mut seen = vec![false; w * h];
    let mut pieces = 0;
    for start in 0..w * h {
        if seen[start] || mask.data[start] < 128 {
            continue;
        }
        pieces += 1;
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            for dy in -1..=1_i64 {
                for dx in -1..=1_i64 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !seen[j] && mask.data[j] >= 128 {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
    }
    Ok(pieces)
}

/// Bleeding ink: the coverage is blurred and its contrast raised, so the
/// edges swell and soften.
fn bleed(coverage: &[u8], width: u32, height: u32) -> Vec<u8> {
    let blurred = blur_gray(coverage, width as usize, height as usize, 1.5);
    blurred
        .iter()
        .map(|&v| {
            let stretched = (i32::from(v) - 90) * 255 / 110;
            stretched.clamp(0, 255) as u8
        })
        .collect()
}
