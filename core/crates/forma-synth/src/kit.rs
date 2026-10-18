//! The style kit: what a font's own glyphs say about its style, measured on
//! the glyphs traced from the drawing.
//!
//! Every value comes from the font: stroke widths along the skeletons, the
//! contrast between thick and thin, whether ends swell into balls and how
//! much, how narrow joins get, the widths of round, straight and diagonal
//! letters, how dark the letters are, and their side bearings. Synthesis
//! dresses its skeletons with these, and the style score compares what it
//! made against them.

use std::collections::BTreeMap;

use forma_geometry::{bbox, signed_area};
use forma_model::{Font, GlyphId, MasterId, Source};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SynthError;

/// Ends swell into balls when the terminal is this much wider than the
/// stroke (diameter over width).
const BALL_RATIO: f64 = 1.2;
/// Letters whose shapes tell the widths of round, straight and diagonal
/// glyphs.
const ROUND: [&str; 5] = ["O", "C", "G", "Q", "D"];
const STRAIGHT: [&str; 4] = ["H", "N", "U", "M"];
const DIAGONAL: [&str; 5] = ["A", "V", "X", "Y", "K"];
/// Strokes whose thick-to-thin ratio is below this are monoline.
pub const CONTRAST_RATIO: f64 = 1.3;
/// The least half-width of the range of end shapes.
const FILL_RANGE_MIN: f64 = 0.04;
/// Ends at least this full are flat (a flat end measures about 1, a round
/// one about 0.79).
pub const FLAT_FILL: f64 = 0.9;
/// The narrowest glyph whose darkness counts, in cap heights.
pub const COLOR_MIN_WIDTH: f64 = 0.4;
/// The fewest traced glyphs a kit is learned from.
pub const MIN_SOURCES: usize = 3;

/// The ink widths of letters by shape class, in font units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Widths {
    /// Round letters (O, C, G, Q, D).
    pub round: Option<f64>,
    /// Straight letters (H, N, U, M).
    pub straight: Option<f64>,
    /// Diagonal letters (A, V, X, Y, K).
    pub diagonal: Option<f64>,
    /// Every source letter.
    pub median: f64,
}

/// The style of a font, measured on its traced glyphs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StyleKit {
    /// The master measured.
    pub master: MasterId,
    /// The cap height of the master.
    pub cap_height: f64,
    /// The typical stroke width: the median of the glyphs' median widths.
    pub stroke: f64,
    /// How much a stroke width varies within glyphs: half the spread
    /// between the typical thin and thick widths (the contrast).
    pub spread: f64,
    /// How much the typical stroke varies from glyph to glyph: half the
    /// spread between the 10th and 90th percentiles of the glyphs' median
    /// strokes, at least a tenth of the stroke. A glyph's weight is judged
    /// against it.
    pub stroke_spread: f64,
    /// The thinnest and thickest typical widths (10th and 90th
    /// percentiles), the stroke's contrast.
    pub thin: f64,
    pub thick: f64,
    /// The diameter of the ball at the open ends over the stroke width, when
    /// ends swell into balls; `None` for plain ends.
    pub ball: Option<f64>,
    /// How square the ends of the strokes are (see
    /// `forma_trace::StyleMeasure::terminal_fill`): the median over the
    /// letters and the range it spans (10th and 90th percentiles); `None`
    /// without measurable ends.
    pub terminal_fill: Option<f64>,
    pub terminal_fill_range: Option<[f64; 2]>,
    /// How narrow joins get, as a fraction of the stroke width.
    pub join_pinch: Option<f64>,
    /// The widths of letters by shape.
    pub widths: Widths,
    /// How dark the letters are: ink area over the box of ink width by cap
    /// height (median), and the range it spans from letter to letter (10th
    /// and 90th percentiles), over letters at least [`COLOR_MIN_WIDTH`] cap
    /// heights wide (in a narrow letter the stroke is most of the box).
    pub color: f64,
    pub color_range: [f64; 2],
    /// The median side bearings.
    pub lsb: f64,
    pub rsb: f64,
    /// The glyphs measured, by name.
    pub sources: BTreeMap<String, GlyphId>,
    /// How closely skeletons dressed in this style reproduce the font's own
    /// letters (I, H, O, L, E, T, those it has): the median overlap, 1 for
    /// a perfect match; `None` when it has none of them. Low, the drawing
    /// has what skeletons do not draw (serifs, tapers, irregular strokes).
    pub reproduction: Option<f64>,
}

fn median(values: &mut [f64]) -> Option<f64> {
    percentile(values, 0.5)
}

/// The value at fraction `q` of the sorted values (nearest rank).
fn percentile(values: &mut [f64], q: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let last = values.len() - 1;
    let at = ((last as f64) * q).round() as usize;
    Some(values[at.min(last)])
}

/// The typical ink width of a capital, in cap heights, that recipes'
/// widths are drawn for.
const REFERENCE_WIDTH: f64 = 0.75;

impl StyleKit {
    /// The thin and thick widths of the stroke's contrast, when the font
    /// has one (thick over thin at least [`CONTRAST_RATIO`]) and its ends do
    /// not swell into balls: in a font of balls and pinches the spread of
    /// widths comes from them, not from the direction of the strokes.
    #[must_use]
    pub fn contrast(&self) -> Option<(f64, f64)> {
        (self.ball.is_none() && self.thin > 0.0 && self.thick / self.thin >= CONTRAST_RATIO)
            .then_some((self.thin, self.thick))
    }

    /// Whether the ends of the strokes are cut flat rather than rounded:
    /// their median fill is at least [`FLAT_FILL`].
    #[must_use]
    pub fn flat_ends(&self) -> bool {
        self.ball.is_none() && self.terminal_fill.is_some_and(|f| f >= FLAT_FILL)
    }

    /// How wide the font's letters are against those recipes are drawn
    /// for: the median letter ink width over 0.75 cap heights, between 0.75
    /// and 1.35. Recipe widths are multiplied by it, so a condensed font
    /// gets condensed figures.
    #[must_use]
    pub fn proportion(&self) -> f64 {
        if self.cap_height > 0.0 && self.widths.median > 0.0 {
            (self.widths.median / (REFERENCE_WIDTH * self.cap_height)).clamp(0.75, 1.35)
        } else {
            1.0
        }
    }

    /// Measures the style of `font` in `master` on its traced glyphs (by
    /// default every glyph traced from an image and not rejected), or on
    /// `only` when given.
    ///
    /// # Errors
    ///
    /// [`SynthError::NoSources`] with fewer than [`MIN_SOURCES`] glyphs that
    /// can be measured.
    pub fn extract(
        font: &Font,
        master: MasterId,
        only: Option<&[GlyphId]>,
    ) -> Result<StyleKit, SynthError> {
        let cap_height = font
            .masters
            .iter()
            .find(|m| m.id == master)
            .map_or(0.0, |m| m.metrics.cap_height);
        let candidates: Vec<GlyphId> = match only {
            Some(ids) => ids.to_vec(),
            None => font
                .glyph_order
                .iter()
                .filter(|id| {
                    font.glyphs.get(id).is_some_and(|g| {
                        g.provenance.as_ref().is_some_and(|p| {
                            p.source == Source::Traced && p.review != forma_model::Review::Rejected
                        })
                    })
                })
                .copied()
                .collect(),
        };
        let (mut stroke, mut thin, mut thick, mut balls, mut pinches) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut widths_all, mut colors, mut lsbs, mut rsbs) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut fills = Vec::new();
        let mut by_class: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        let mut sources = BTreeMap::new();
        for id in candidates {
            let Some(glyph) = font.glyphs.get(&id) else {
                continue;
            };
            let Some(layer) = glyph.layers.get(&master) else {
                continue;
            };
            let Some(bounds) = bbox(&layer.paths) else {
                continue;
            };
            let Ok(m) = forma_trace::measure(font, id, master) else {
                continue;
            };
            if !(m.stroke_median.is_finite() && m.stroke_median > 0.0) {
                continue;
            }
            stroke.push(m.stroke_median);
            thin.push(m.stroke_p10);
            thick.push(m.stroke_p90);
            if let Some(r) = m.terminal_radius {
                balls.push(2.0 * r / m.stroke_median);
            }
            if let Some(f) = m.terminal_fill {
                fills.push(f);
            }
            if let Some(j) = m.join_min {
                pinches.push(2.0 * j / m.stroke_median);
            }
            let w = bounds.width();
            widths_all.push(w);
            for (class, names) in [
                ("round", &ROUND[..]),
                ("straight", &STRAIGHT[..]),
                ("diagonal", &DIAGONAL[..]),
            ] {
                if names.contains(&glyph.name.as_str()) {
                    by_class.entry(class).or_default().push(w);
                }
            }
            let area: f64 = layer.paths.iter().map(signed_area).sum::<f64>().abs();
            if cap_height > 0.0 && w >= COLOR_MIN_WIDTH * cap_height {
                colors.push(area / (w * cap_height));
            }
            lsbs.push(bounds.x0);
            rsbs.push(layer.advance_width - bounds.x1);
            sources.insert(glyph.name.clone(), id);
        }
        if sources.len() < MIN_SOURCES {
            return Err(SynthError::NoSources(format!(
                "{} glyph(s) could be measured; at least {MIN_SOURCES} traced glyphs are needed",
                sources.len()
            )));
        }
        let stroke_m = median(&mut stroke).unwrap_or(0.0);
        let stroke_spread = (percentile(&mut stroke, 0.9).unwrap_or(stroke_m)
            - percentile(&mut stroke, 0.1).unwrap_or(stroke_m))
            / 2.0;
        let thin_m = median(&mut thin).unwrap_or(stroke_m);
        let thick_m = median(&mut thick).unwrap_or(stroke_m);
        let fill = median(&mut fills);
        // One class of ends: flat ends are flat even where serifs or flared
        // ends make the end wider than the stroke; balls only where ends are
        // round.
        let ball = median(&mut balls)
            .filter(|r| *r >= BALL_RATIO)
            .filter(|_| fill.is_none_or(|f| f < FLAT_FILL));
        // The range of end shapes, at least ±0.04 around the median: the
        // measure itself varies that much with the direction of a stroke.
        let fill_range = percentile(&mut fills, 0.1)
            .zip(percentile(&mut fills, 0.9))
            .zip(fill)
            .map(|((low, high), m)| [low.min(m - FILL_RANGE_MIN), high.max(m + FILL_RANGE_MIN)]);
        let mut kit = StyleKit {
            master,
            cap_height,
            stroke: stroke_m,
            spread: ((thick_m - thin_m) / 2.0).max(0.05 * stroke_m),
            stroke_spread: stroke_spread.max(0.1 * stroke_m),
            thin: thin_m,
            thick: thick_m,
            ball,
            terminal_fill_range: fill_range,
            terminal_fill: fill,
            join_pinch: median(&mut pinches),
            widths: Widths {
                round: by_class.get_mut("round").and_then(|v| median(v)),
                straight: by_class.get_mut("straight").and_then(|v| median(v)),
                diagonal: by_class.get_mut("diagonal").and_then(|v| median(v)),
                median: median(&mut widths_all).unwrap_or(0.0),
            },
            color_range: [
                percentile(&mut colors, 0.1).unwrap_or(0.0),
                percentile(&mut colors, 0.9).unwrap_or(0.0),
            ],
            color: median(&mut colors).unwrap_or(0.0),
            lsb: median(&mut lsbs).unwrap_or(0.0),
            rsb: median(&mut rsbs).unwrap_or(0.0),
            sources,
            reproduction: None,
        };
        kit.reproduction = crate::selftest::reproduction(font, &kit);
        Ok(kit)
    }
}
