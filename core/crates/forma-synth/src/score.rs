//! The style score: how consistent a synthesized glyph is with the font's
//! style, component by component, so that a glyph that does not belong is
//! refused rather than created.
//!
//! Each component is in 0..=1. The total is their weighted mean over the
//! components that apply to the glyph (a period has no stroke to compare;
//! only figures are compared for darkness). A glyph is accepted when the
//! total reaches the threshold **and** no component falls below
//! [`COMPONENT_FLOOR`]: a good average never hides a stroke or a terminal
//! foreign to the font.

use forma_geometry::{bbox, signed_area};
use forma_model::{Font, Glyph, GlyphId, Layer, Path};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::kit::{COLOR_MIN_WIDTH, StyleKit};
use crate::recipe::{Class, Recipe, Seg, Strategy};

/// The version of the synthesis thresholds, recorded in provenance.
pub const THRESHOLDS_VERSION: u32 = 1;
/// The default lowest total (spec §5.5; experimental until calibrated).
pub const MIN_STYLE_SCORE: f64 = 0.75;
/// The lowest any component may be.
pub const COMPONENT_FLOOR: f64 = 0.5;
/// The weights of the components (spec §5.5). Curvature and irregularity
/// are not measured yet: they take no part rather than counting as perfect.
const W_STROKE: f64 = 0.30;
const W_TERMINAL: f64 = 0.20;
const W_COLOR: f64 = 0.15;
const W_PROPORTION: f64 = 0.10;
/// The least spread of stroke widths a comparison allows, as a fraction of
/// the stroke: the measure itself varies by a few percent.
const SPREAD_FLOOR: f64 = 0.1;
/// How far, as a share, a dot's size may be from the style's before its
/// score falls (and how fast it falls beyond).
const DOT_TOLERANCE: f64 = 0.1;
/// How fast the terminal score falls outside the letters' range of end
/// shapes (round and flat ends differ by about 0.2).
const FILL_SCALE: f64 = 0.06;
/// A glyph's width is compared only when its skeleton spans at least this
/// share of its box (a period or an exclamation mark do not: their width is
/// their stroke's).
const PROPORTION_MIN_SPAN: f64 = 0.8;
/// The narrowest glyph, in stroke widths, whose width is compared.
const PROPORTION_MIN_STROKES: f64 = 2.5;

/// The style score of a glyph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StyleScore {
    /// The weighted mean of the components that apply.
    pub total: f64,
    /// Stroke width against the font's.
    pub stroke: Option<f64>,
    /// Terminals against the font's: ball size and end shape.
    pub terminal: Option<f64>,
    /// Darkness against the font's letters (figures at least 0.4 cap
    /// heights wide only).
    pub color: Option<f64>,
    /// Width against the recipe's.
    pub proportion: Option<f64>,
    /// Whether the glyph keeps the pieces and counters of its skeleton (1)
    /// or its strokes merge them (0): not weighed, but a 0 refuses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure: Option<f64>,
    /// Why a component is low, in words.
    pub reasons: Vec<String>,
}

impl StyleScore {
    /// Whether the glyph is accepted at `min_total`.
    #[must_use]
    pub fn accepts(&self, min_total: f64) -> bool {
        self.total >= min_total
            && [
                self.stroke,
                self.terminal,
                self.color,
                self.proportion,
                self.structure,
            ]
            .iter()
            .flatten()
            .all(|c| *c >= COMPONENT_FLOOR)
    }
}

/// How much of its box's width a strategy's skeleton spans (0 to 1).
fn span(strategy: &Strategy) -> f64 {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for spec in &strategy.strokes {
        for seg in &spec.segs {
            let (a, b) = match *seg {
                Seg::Line([x, _]) => (x, x),
                Seg::Arc([cx, _, rx, _, _, _]) | Seg::Ellipse([cx, _, rx, _]) => (cx - rx, cx + rx),
            };
            lo = lo.min(a);
            hi = hi.max(b);
        }
    }
    for dot in &strategy.dots {
        lo = lo.min(dot.at[0]);
        hi = hi.max(dot.at[0]);
    }
    (hi.min(1.0) - lo.max(0.0)).max(0.0)
}

/// Scores `paths` (a glyph made for `recipe` by `strategy`) against `kit`,
/// measuring it in a copy of `font`. The stroke is compared with the width
/// the strategy asks for (a hyphen is drawn a little lighter than the
/// letters); a glyph drawn from a skeleton only also has its width
/// compared with the recipe's (one derived from the font's glyphs takes
/// their width).
#[must_use]
pub fn score(
    font: &Font,
    kit: &StyleKit,
    recipe: &Recipe,
    strategy: &Strategy,
    paths: &[Path],
) -> StyleScore {
    score_with(font, kit, recipe, strategy, paths, 1.0)
}

/// [`score`] for a glyph whose strokes the contrast thickens or thins by
/// `width_factor` (see `dress::Built::width_factor`): a figure 1 of
/// vertical strokes is as heavy as the font's verticals, not as its
/// median stroke.
pub(crate) fn score_with(
    font: &Font,
    kit: &StyleKit,
    recipe: &Recipe,
    strategy: &Strategy,
    paths: &[Path],
    width_factor: f64,
) -> StyleScore {
    let skeleton = strategy.parts.is_empty();
    let weight = if skeleton && !strategy.strokes.is_empty() {
        let mut w: Vec<f64> = strategy.strokes.iter().map(|k| k.w).collect();
        w.sort_by(f64::total_cmp);
        w[w.len() / 2]
    } else {
        1.0
    };
    let mut reasons = Vec::new();
    let b = bbox(paths);
    let ink_width = b.map_or(0.0, |r| r.width());
    // Measure on a copy of the font holding the glyph.
    let id = GlyphId::new();
    let mut scratch = font.clone();
    Arc::make_mut(&mut scratch.glyphs).insert(
        id,
        Arc::new(Glyph {
            id,
            name: format!("{}.synth", recipe.name),
            unicodes: Vec::new(),
            export: false,
            layers: BTreeMap::from([(
                kit.master,
                Layer {
                    advance_width: ink_width,
                    paths: paths.to_vec(),
                    components: Vec::new(),
                },
            )]),
            provenance: None,
        }),
    );
    let measured = forma_trace::measure(&scratch, id, kit.master).ok();

    // A glyph of dots only (a period, a colon, a dieresis): its weight is
    // the size of its dots.
    let dots_only = strategy.strokes.is_empty() && !strategy.dots.is_empty() && skeleton;
    let stroke = match (&measured, recipe.stroke_check) {
        (Some(m), _) if dots_only && m.stroke_median > 0.0 => {
            let wanted = 2.0 * crate::dress::dot_radius(kit, strategy.dots[0].r);
            let off = (m.stroke_median / wanted - 1.0).abs();
            let s = (-(off - DOT_TOLERANCE).max(0.0) / DOT_TOLERANCE).exp();
            if s < 0.6 {
                reasons.push(format!(
                    "dots {:.0} units across against the {wanted:.0} of the font's style",
                    m.stroke_median
                ));
            }
            Some(s)
        }
        (Some(m), true) if m.stroke_median > 0.0 => {
            let wanted = kit.stroke * weight * width_factor;
            let z =
                (m.stroke_median - wanted).abs() / kit.stroke_spread.max(SPREAD_FLOOR * kit.stroke);
            let s = (-(z - 1.0).max(0.0)).exp();
            if s < 0.6 {
                reasons.push(format!(
                    "stroke {:.0} units against the {:.0} of the font's style",
                    m.stroke_median, wanted
                ));
            }
            Some(s)
        }
        _ => None,
    };
    let terminal = match (&measured, kit.ball) {
        (Some(m), Some(ball)) if recipe.stroke_check => m.terminal_radius.map(|r| {
            let ratio = 2.0 * r / m.stroke_median.max(1e-9);
            let s = (-(ratio - ball).abs() / (0.25 * ball)).exp();
            if s < 0.6 {
                reasons.push(format!(
                    "ends {ratio:.2} times the stroke against the font's balls of {ball:.2}"
                ));
            }
            s
        }),
        (Some(m), None) if recipe.stroke_check => m.terminal_radius_median.map(|r| {
            let ratio = 2.0 * r / m.stroke_median.max(1e-9);
            if ratio > 1.4 {
                reasons.push("swollen ends in a font with plain ends".to_owned());
                0.4
            } else {
                1.0
            }
        }),
        _ => None,
    };
    // The shape of the ends: inside the range the letters span, they are
    // the font's; outside it, the score falls with the distance (a round
    // end in a font of flat ends is foreign however good the rest is).
    let shape = match (&measured, kit.terminal_fill, kit.terminal_fill_range) {
        (Some(m), Some(_), Some([low, high])) if recipe.stroke_check => m.terminal_fill.map(|f| {
            let off = (low - f).max(f - high).max(0.0);
            let s = (-off / FILL_SCALE).exp();
            if s < 0.6 {
                reasons.push(format!(
                    "ends {:.2} square against the letters' {low:.2} to {high:.2}",
                    f
                ));
            }
            s
        }),
        _ => None,
    };
    let terminal = match (terminal, shape) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let color = if recipe.class == Class::Digit
        && kit.color > 0.0
        && ink_width >= COLOR_MIN_WIDTH * kit.cap_height
    {
        // Inside the range the letters span, a figure is as dark as they
        // are; outside it, the score falls with the distance.
        let area: f64 = paths.iter().map(signed_area).sum::<f64>().abs();
        let c = area / (ink_width * kit.cap_height);
        let [low, high] = kit.color_range;
        let off = (low - c).max(c - high).max(0.0);
        let s = (-off / (0.3 * kit.color)).exp();
        if s < 0.6 {
            reasons.push(format!(
                "darkness {c:.2} outside the letters' {low:.2} to {high:.2}"
            ));
        }
        Some(s)
    } else {
        None
    };
    // The width tells something only where the skeleton spans it: in a
    // glyph about as narrow as its stroke or dot, the stroke sets it.
    let expected = recipe.width * kit.cap_height * kit.proportion();
    let proportion = (skeleton
        && recipe.stroke_check
        && recipe.class != Class::Mark
        && ink_width > 0.0
        && span(strategy) >= PROPORTION_MIN_SPAN
        && expected >= PROPORTION_MIN_STROKES * kit.stroke)
        .then(|| {
            let s = (-(ink_width / expected - 1.0).abs() / 0.2).exp();
            if s < 0.6 {
                reasons.push(format!(
                    "width {ink_width:.0} units against the {expected:.0} the recipe expects"
                ));
            }
            s
        });
    let parts = [
        (stroke, W_STROKE),
        (terminal, W_TERMINAL),
        (color, W_COLOR),
        (proportion, W_PROPORTION),
    ];
    let (sum, weight) = parts
        .iter()
        .filter_map(|(c, w)| c.map(|c| (c * w, *w)))
        .fold((0.0, 0.0), |(s, t), (cw, w)| (s + cw, t + w));
    let total = if weight > 0.0 {
        sum / weight
    } else {
        // Nothing could be compared: no evidence against the glyph, and
        // none for it. A person looks.
        reasons
            .push("nothing of this glyph could be compared with the style (unscored)".to_owned());
        1.0
    };
    StyleScore {
        total: (total * 1000.0).round() / 1000.0,
        stroke,
        terminal,
        color,
        proportion,
        structure: None,
        reasons,
    }
}
