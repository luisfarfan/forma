//! Collision checks: kerned pairs whose ink comes too close.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use forma_model::{Font, GlyphId, MasterId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SpacingError;
use crate::profile::{Outline, layer};

/// The most pairs one call of [`collisions`] checks.
pub const MAX_CHECK_PAIRS: usize = 250_000;

/// The fewest grid lines across the tallest glyph.
const MIN_LINES_ACROSS: f64 = 256.0;
/// The largest line spacing, as a fraction of the units per em (4 units at
/// 1000 UPM).
const MAX_STEP_RATIO: f64 = 0.004;
/// The most grid lines across the tallest glyph: bounds the memory of a
/// font with an absurdly tall glyph.
const MAX_LINES_ACROSS: f64 = 8192.0;

/// A pair whose ink comes closer than the minimum distance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Collision {
    /// The left glyph.
    pub left: GlyphId,
    /// The right glyph.
    pub right: GlyphId,
    /// The smallest horizontal distance between the ink of the two glyphs,
    /// in font units; negative when they overlap.
    pub distance: f64,
}

/// Horizontal lines shared by every glyph of a check: line `k` sits at
/// `(k + 0.5) · step`, so glyphs sampled on the same grid can be compared
/// line by line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Grid {
    step: f64,
}

impl Grid {
    /// The grid for a font whose tallest glyph is `tallest` units high:
    /// at most `0.004 · UPM` between lines and at least 256 lines across the
    /// tallest glyph (at most 8192).
    pub(crate) fn new(units_per_em: u16, tallest: f64) -> Grid {
        let mut step = MAX_STEP_RATIO * f64::from(units_per_em);
        if tallest.is_finite() && tallest > 0.0 {
            step = step
                .min(tallest / MIN_LINES_ACROSS)
                .max(tallest / MAX_LINES_ACROSS);
        }
        Grid { step }
    }

    /// The height of line `k`.
    pub(crate) fn y(self, k: i64) -> f64 {
        (k as f64 + 0.5) * self.step
    }
}

/// The ink extents of one glyph on the lines of a [`Grid`] that cross its
/// bounding box: `extents[i]` is the leftmost and rightmost ink on line
/// `first + i`, or `None` without ink there.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InkSamples {
    first: i64,
    extents: Vec<Option<(f64, f64)>>,
}

impl InkSamples {
    /// Samples `outline` on `grid`; `None` if it has no outline.
    pub(crate) fn new(outline: &Outline, grid: Grid) -> Option<InkSamples> {
        let bbox = outline.bbox?;
        let first = (bbox.y0 / grid.step - 0.5).ceil() as i64;
        let last = (bbox.y1 / grid.step - 0.5).floor() as i64;
        let extents = (first..=last).map(|k| outline.extent(grid.y(k))).collect();
        Some(InkSamples { first, extents })
    }

    /// The extents on line `k`, if the glyph has ink there.
    pub(crate) fn at(&self, k: i64) -> Option<(f64, f64)> {
        let index = usize::try_from(k - self.first).ok()?;
        *self.extents.get(index)?
    }

    /// One past the last sampled line.
    fn end(&self) -> i64 {
        self.first + self.extents.len() as i64
    }

    /// The lines where both `left` and `right` may have ink.
    pub(crate) fn shared_lines(left: &InkSamples, right: &InkSamples) -> std::ops::Range<i64> {
        left.first.max(right.first)..left.end().min(right.end())
    }

    /// The smallest horizontal distance between the ink of `left` and that
    /// of `right` placed `offset` units to its right, over the lines where
    /// both have ink; `None` if there is no such line.
    pub(crate) fn min_distance(left: &InkSamples, right: &InkSamples, offset: f64) -> Option<f64> {
        InkSamples::shared_lines(left, right)
            .filter_map(|k| Some(offset + right.at(k)?.0 - left.at(k)?.1))
            .reduce(f64::min)
    }
}

/// The outline of every glyph of `glyphs`, each once.
///
/// # Errors
///
/// As [`collisions`]: a glyph that does not exist, has no layer in
/// `master`, or cannot be expanded.
pub(crate) fn load_outlines(
    font: &Font,
    master: MasterId,
    glyphs: impl IntoIterator<Item = GlyphId>,
) -> Result<HashMap<GlyphId, Outline>, SpacingError> {
    let mut outlines: HashMap<GlyphId, Outline> = HashMap::new();
    for glyph in glyphs {
        if let Entry::Vacant(entry) = outlines.entry(glyph) {
            layer(font, glyph, master)?;
            entry.insert(Outline::of(font, glyph, master)?);
        }
    }
    Ok(outlines)
}

/// The grid of a check of the glyphs whose outlines are `outlines`: chosen
/// from the tallest of them.
pub(crate) fn grid_for<'a>(
    units_per_em: u16,
    outlines: impl IntoIterator<Item = &'a Outline>,
) -> Grid {
    let tallest = outlines
        .into_iter()
        .filter_map(|outline| outline.bbox)
        .map(|bbox| bbox.height())
        .fold(0.0, f64::max);
    Grid::new(units_per_em, tallest)
}

/// The samples of every glyph of `glyphs` (each once) on one grid chosen
/// from the tallest of them: memory grows with the number of glyphs, not of
/// pairs. Glyphs without outline map to `None`.
///
/// # Errors
///
/// As [`collisions`]: a glyph that does not exist, has no layer in
/// `master`, or cannot be expanded.
pub(crate) fn sample_glyphs(
    font: &Font,
    master: MasterId,
    glyphs: impl IntoIterator<Item = GlyphId>,
) -> Result<(Grid, HashMap<GlyphId, Option<InkSamples>>), SpacingError> {
    let outlines = load_outlines(font, master, glyphs)?;
    let grid = grid_for(font.units_per_em, outlines.values());
    let samples = outlines
        .into_iter()
        .map(|(glyph, outline)| (glyph, InkSamples::new(&outline, grid)))
        .collect();
    Ok((grid, samples))
}

/// Finds the pairs of `pairs` whose ink, with the advance of the left glyph
/// and the effective kerning of the pair in `master` applied, comes closer
/// than `min_distance` horizontally.
///
/// Every glyph is sampled once on a grid of horizontal lines shared by the
/// whole check: at most `0.004 · UPM` apart (4 units at 1000 UPM) and at
/// least 256 lines across the tallest glyph involved. On each line where
/// both glyphs have ink, the distance is the leftmost ink of the right
/// glyph minus the rightmost ink of the left one (non-zero fill, components
/// expanded), and the pair's distance is the smallest of them. A distance
/// below 0 is an overlap, 0 a touch. Pairs with a glyph without ink, or
/// without a common line of ink, never collide. The collisions are sorted
/// by ascending distance; ties keep the order of `pairs`.
///
/// Sampling limit: features thinner than the line spacing (a hairline
/// serif, the very tip of an apex) can fall between two lines and be
/// missed, and the distance compares the extremes of each line, so ink
/// interleaved on one line (a hyphen inside the mouth of a C) is reported
/// as an overlap.
///
/// # Errors
///
/// * [`SpacingError::InvalidParameter`] if there are more than
///   [`MAX_CHECK_PAIRS`] pairs, `min_distance` is not finite, or a glyph does
///   not exist.
/// * [`SpacingError::MissingLayer`] if a glyph has no layer in `master`.
/// * [`SpacingError::Geometry`] if an outline cannot be expanded.
pub fn collisions(
    font: &Font,
    master: MasterId,
    pairs: &[(GlyphId, GlyphId)],
    min_distance: f64,
) -> Result<Vec<Collision>, SpacingError> {
    collisions_among(font, master, &[], pairs, min_distance)
}

/// [`collisions`] with the sampling grid chosen from the tallest of
/// `grid_glyphs` as well as of the glyphs in `pairs`.
///
/// [`auto_kerning`](crate::auto_kerning) chooses its grid from every glyph it
/// is given, `.notdef` included, though it never kerns `.notdef`. To check a
/// plan on that same grid, pass the same glyph list as `grid_glyphs` and the
/// pairs without `.notdef`: the guard and the check then sample alike.
///
/// # Errors
///
/// As [`collisions`], also for a glyph of `grid_glyphs`.
pub fn collisions_among(
    font: &Font,
    master: MasterId,
    grid_glyphs: &[GlyphId],
    pairs: &[(GlyphId, GlyphId)],
    min_distance: f64,
) -> Result<Vec<Collision>, SpacingError> {
    if pairs.len() > MAX_CHECK_PAIRS {
        return Err(SpacingError::InvalidParameter(format!(
            "at most {MAX_CHECK_PAIRS} pairs can be checked at once, got {}",
            pairs.len()
        )));
    }
    if !min_distance.is_finite() {
        return Err(SpacingError::InvalidParameter(format!(
            "min_distance must be finite, got {min_distance}"
        )));
    }
    let (_, samples) = sample_glyphs(
        font,
        master,
        grid_glyphs
            .iter()
            .copied()
            .chain(pairs.iter().flat_map(|&(left, right)| [left, right])),
    )?;
    let mut found = Vec::new();
    for &(left, right) in pairs {
        let (Some(Some(left_ink)), Some(Some(right_ink))) =
            (samples.get(&left), samples.get(&right))
        else {
            continue;
        };
        let offset =
            layer(font, left, master)?.advance_width + font.kerning.effective(master, left, right);
        if let Some(distance) = InkSamples::min_distance(left_ink, right_ink, offset)
            && distance < min_distance
        {
            found.push(Collision {
                left,
                right,
                distance,
            });
        }
    }
    found.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use forma_geometry::{CoordinateSystem, paths_from_svg};
    use forma_model::{Glyph, Layer};

    use super::*;

    fn add(font: &mut Font, name: &str, d: &str) -> GlyphId {
        let id = GlyphId::new();
        let mut layers = BTreeMap::new();
        layers.insert(
            font.masters[0].id,
            Layer {
                advance_width: 500.0,
                paths: paths_from_svg(d, CoordinateSystem::Font).expect("svg"),
                components: Vec::new(),
            },
        );
        let glyph = Glyph {
            id,
            name: name.to_owned(),
            unicodes: Vec::new(),
            export: true,
            layers,
            provenance: None,
        };
        Arc::make_mut(&mut font.glyphs).insert(id, Arc::new(glyph));
        font.glyph_order.push(id);
        id
    }

    #[test]
    fn grid_spacing_follows_the_font() {
        assert_eq!(Grid::new(1000, 700.0).step, 700.0 / 256.0);
        assert_eq!(Grid::new(1000, 2000.0).step, 4.0);
        assert_eq!(Grid::new(2048, 0.0).step, 0.004 * 2048.0);
        assert_eq!(Grid::new(1000, 1e7).step, 1e7 / 8192.0);
    }

    #[test]
    fn every_glyph_is_sampled_once_whatever_the_pairs() {
        // Glyphs of all different heights: the samples grow with the
        // glyphs, not with the pairs.
        let mut font = Font::new("T", 1000);
        let glyphs: Vec<GlyphId> = (0..40)
            .map(|i| {
                let top = 500.0 + f64::from(i) * 7.3;
                add(
                    &mut font,
                    &format!("g{i}"),
                    &format!("M0 0 L100 0 L100 {top} L0 {top} Z"),
                )
            })
            .collect();
        let pairs = glyphs
            .iter()
            .flat_map(|a| glyphs.iter().map(move |b| [*a, *b]))
            .flatten();
        let (grid, samples) = sample_glyphs(&font, font.masters[0].id, pairs).expect("samples");
        assert_eq!(samples.len(), glyphs.len());
        let tallest = 500.0 + 39.0 * 7.3;
        assert_eq!(grid, Grid::new(1000, tallest));
        let lines: usize = samples
            .values()
            .map(|s| s.as_ref().expect("ink").extents.len())
            .sum();
        assert!(lines <= glyphs.len() * 260, "{lines}");
    }

    #[test]
    fn samples_compare_on_shared_lines_only() {
        let mut font = Font::new("T", 1000);
        let low = add(&mut font, "low", "M0 0 L100 0 L100 100 L0 100 Z");
        let high = add(&mut font, "high", "M0 500 L100 500 L100 600 L0 600 Z");
        let (_, samples) = sample_glyphs(&font, font.masters[0].id, [low, high]).expect("samples");
        let (low, high) = (
            samples[&low].as_ref().expect("ink"),
            samples[&high].as_ref().expect("ink"),
        );
        assert_eq!(InkSamples::min_distance(low, high, 0.0), None);
        assert_eq!(InkSamples::min_distance(low, low, 150.0), Some(50.0));
        assert_eq!(low.at(low.first - 1), None);
        assert_eq!(low.at(low.end()), None);
    }
}
