//! Alphabets rendered from the OFL test fonts (levels M and M-hand).
//!
//! The 26 capitals are laid out in three rows (9, 9 and 8), with the
//! font's own advances plus a constant gap, so the spacing of the original is
//! known. Outlines come from `skrifa` (unhinted, in font units) and are
//! rasterized with the same renderer as the rest of FORMA.

use kurbo::{Affine, BezPath, Shape};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider};

use crate::corpus::{CorpusCase, HAND_FONT, Level, M_CAPS, M_FONTS};
use crate::degrade::{Degradation, finish};
use crate::error::BenchError;
use crate::scene::{Scene, SceneGlyph};

/// The rows of the alphabet.
pub const ALPHABET_ROWS: [&str; 3] = ["ABCDEFGHI", "JKLMNOPQR", "STUVWXYZ"];

/// The gap added to every advance, as a fraction of the cap height.
const GAP: f64 = 0.15;
/// The margin around the alphabet, as a fraction of the cap height.
const MARGIN: f64 = 0.8;
/// The distance between baselines, as a fraction of the cap height.
const ROW_PITCH: f64 = 2.0;

/// The bytes of a corpus font, by key.
#[must_use]
pub fn font_bytes(key: &str) -> Option<&'static [u8]> {
    match key {
        "inter" => Some(include_bytes!(
            "../../../../tests/fixtures/inter/Inter-Regular.ttf"
        )),
        "eb-garamond" => Some(include_bytes!(
            "../../../../tests/fixtures/eb-garamond/EBGaramond-Regular.ttf"
        )),
        "fraunces" => Some(include_bytes!(
            "../../../../tests/fixtures/fraunces/Fraunces72pt-Regular.ttf"
        )),
        "patrick-hand" => Some(include_bytes!(
            "../../../../tests/fixtures/patrick-hand/PatrickHand-Regular.ttf"
        )),
        _ => None,
    }
}

/// An outline in font units (y up) and its advance.
#[derive(Clone, Debug)]
pub struct Outline {
    /// The contours, as the font draws them.
    pub path: BezPath,
    /// The horizontal advance in font units.
    pub advance: f64,
}

#[derive(Default)]
struct PathPen(BezPath);

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((f64::from(x), f64::from(y)));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((f64::from(x), f64::from(y)));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to(
            (f64::from(cx0), f64::from(cy0)),
            (f64::from(x), f64::from(y)),
        );
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.curve_to(
            (f64::from(cx0), f64::from(cy0)),
            (f64::from(cx1), f64::from(cy1)),
            (f64::from(x), f64::from(y)),
        );
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

/// The unhinted outline of `ch` in the font `bytes`.
///
/// # Errors
///
/// [`BenchError::Font`] if the font cannot be read or lacks the glyph.
pub fn outline(bytes: &[u8], ch: char) -> Result<Outline, BenchError> {
    let font = FontRef::new(bytes).map_err(|e| BenchError::Font(e.to_string()))?;
    let gid = font
        .charmap()
        .map(ch)
        .ok_or_else(|| BenchError::Font(format!("no glyph for {ch:?}")))?;
    let glyph = font
        .outline_glyphs()
        .get(gid)
        .ok_or_else(|| BenchError::Font(format!("no outline for {ch:?}")))?;
    let mut pen = PathPen::default();
    glyph
        .draw(
            DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
            &mut pen,
        )
        .map_err(|e| BenchError::Font(e.to_string()))?;
    let advance = font
        .glyph_metrics(Size::unscaled(), LocationRef::default())
        .advance_width(gid)
        .map_or(0.0, f64::from);
    Ok(Outline {
        path: pen.0,
        advance,
    })
}

/// The units per em of the font.
///
/// # Errors
///
/// [`BenchError::Font`] if the font cannot be read.
pub fn units_per_em(bytes: &[u8]) -> Result<f64, BenchError> {
    let font = FontRef::new(bytes).map_err(|e| BenchError::Font(e.to_string()))?;
    Ok(f64::from(
        font.metrics(Size::unscaled(), LocationRef::default())
            .units_per_em,
    ))
}

/// The scene of the alphabet of font `key` with capitals `cap_px` tall.
///
/// # Errors
///
/// [`BenchError::Font`] for an unknown font or a missing glyph.
pub fn alphabet_scene(key: &str, cap_px: u32) -> Result<Scene, BenchError> {
    let bytes = font_bytes(key).ok_or_else(|| BenchError::Unknown(format!("font {key:?}")))?;
    let cap = f64::from(cap_px);
    let h_box = outline(bytes, 'H')?.path.bounding_box();
    let scale = cap / h_box.height();
    let margin = (MARGIN * cap).round();
    let gap = GAP * cap;

    let mut glyphs = Vec::new();
    let mut max_row = 0.0_f64;
    for (r, row) in ALPHABET_ROWS.iter().enumerate() {
        let baseline = margin + cap * 1.2 + ROW_PITCH * cap * r as f64;
        let mut pen = margin;
        for ch in row.chars() {
            let o = outline(bytes, ch)?;
            let to_px = Affine::new([scale, 0.0, 0.0, -scale, pen, baseline]);
            let advance = o.advance * scale;
            glyphs.push(SceneGlyph {
                key: ch.to_string(),
                shapes: vec![to_px * o.path],
                origin: Some([pen, baseline]),
                advance: Some(advance),
            });
            pen += advance + gap;
        }
        max_row = max_row.max(pen);
    }
    let width = (max_row + margin).ceil() as u32;
    let height =
        (margin * 2.0 + cap * 1.2 + ROW_PITCH * cap * (ALPHABET_ROWS.len() - 1) as f64 + 0.4 * cap)
            .ceil() as u32;
    let layout = ALPHABET_ROWS.iter().map(|r| (*r).to_owned()).collect();
    let mut scene = Scene::new(width, height, layout);
    scene.glyphs = glyphs;
    scene.font_scale = Some((scale, cap));
    Ok(scene)
}

/// Builds the M or M-hand case with this id (`m-<font>-c<cap>-<degradation>`).
///
/// # Errors
///
/// [`BenchError::Unknown`] if the id is malformed.
pub fn alphabet_case(id: &str, level: Level) -> Result<CorpusCase, BenchError> {
    let bad = || BenchError::Unknown(format!("alphabet case id {id:?}"));
    let prefix = if level == Level::MHand {
        "mhand-"
    } else {
        "m-"
    };
    let rest = id.strip_prefix(prefix).ok_or_else(bad)?;
    let mut parts = rest.rsplitn(3, '-');
    let degradation = parts.next().and_then(Degradation::parse).ok_or_else(bad)?;
    let cap: u32 = parts
        .next()
        .and_then(|c| c.strip_prefix('c'))
        .and_then(|c| c.parse().ok())
        .ok_or_else(bad)?;
    let font = parts.next().ok_or_else(bad)?;
    if !M_CAPS.contains(&cap) || !(M_FONTS.contains(&font) || font == HAND_FONT) {
        return Err(bad());
    }
    let scene = alphabet_scene(font, cap)?;
    let layout = scene.layout.clone();
    let (image, extension, truth) = finish(scene, degradation)?;
    Ok(CorpusCase {
        id: id.to_owned(),
        level,
        image,
        extension,
        layout,
        truth: Some(truth),
    })
}
