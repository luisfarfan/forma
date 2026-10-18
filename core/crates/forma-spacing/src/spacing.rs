//! Automatic spacing: sidebearings that give every glyph the white of `H`.

use std::collections::HashSet;

use forma_model::{Font, GlyphId, MasterId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SpacingError;
use crate::profile::{Outline, Profile, Zone, check_lines, layer, line_heights, measurement_zone};

/// Bisection stops when the margin is known to within this many units.
const TOLERANCE: f64 = 0.5;

/// Default [`SpacingParams::depth_ratio`].
pub const DEFAULT_DEPTH_RATIO: f64 = 0.4;
/// Default [`SpacingParams::edge_weight`].
pub const DEFAULT_EDGE_WEIGHT: f64 = 0.25;

/// The parameters of [`auto_spacing`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpacingParams {
    /// The margin of the first reference as a fraction of its stem width:
    /// lower is tighter. 0.6 by default, suited to display sizes.
    pub tightness: f64,
    /// The names of the reference glyphs: the first (`H`) sets the target
    /// white; the second (`O`) must exist and is spaced from the same white.
    pub references: [String; 2],
    /// How deep into the glyph the profile counts, as a multiple of the
    /// stem width of the first reference (0.4 by default).
    pub depth_ratio: f64,
    /// How many profile lines measure each glyph (64 by default).
    pub lines: usize,
    /// The weight of the white on the lines at the top and bottom of the
    /// zone, relative to the middle (0.25 by default); see
    /// [`auto_spacing`].
    pub edge_weight: f64,
}

impl Default for SpacingParams {
    fn default() -> Self {
        SpacingParams {
            tightness: 0.6,
            references: ["H".to_owned(), "O".to_owned()],
            depth_ratio: DEFAULT_DEPTH_RATIO,
            lines: 64,
            edge_weight: DEFAULT_EDGE_WEIGHT,
        }
    }
}

/// The sidebearings computed for a glyph, in whole font units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Sidebearings {
    /// The glyph.
    pub glyph: GlyphId,
    /// The left sidebearing: from the origin to the left of the bounding
    /// box.
    pub lsb: f64,
    /// The right sidebearing: from the right of the bounding box to the
    /// advance.
    pub rsb: f64,
}

/// What a side of a glyph contributes to the white next to it.
struct Sides {
    /// The weighted mean depth of the clipped profile on each side: the
    /// white between the bounding box and the ink, per unit of zone height.
    left_depth: f64,
    right_depth: f64,
}

impl Sides {
    /// The sides of `profile` for a glyph whose ink spans `left_edge` to
    /// `right_edge` (its bounding box): each line's depth is the distance
    /// from the edge to the ink, clipped at `depth` (lines without ink count
    /// as the full depth), and the lines are averaged with
    /// [`line_weights`]`(lines, edge_weight)`. `None` if the profile has no
    /// ink.
    fn of(
        profile: &Profile,
        (left_edge, right_edge): (f64, f64),
        depth: f64,
        edge_weight: f64,
    ) -> Option<Sides> {
        profile.left.iter().flatten().next()?;
        let weights = line_weights(profile.ys.len(), edge_weight);
        let total: f64 = weights.iter().sum();
        let mean = |values: &mut dyn Iterator<Item = f64>| {
            values.zip(&weights).map(|(v, w)| v * w).sum::<f64>() / total
        };
        let left_depth = mean(
            &mut profile
                .left
                .iter()
                .map(|x| x.map_or(depth, |x| (x - left_edge).clamp(0.0, depth))),
        );
        let right_depth = mean(
            &mut profile
                .right
                .iter()
                .map(|x| x.map_or(depth, |x| (right_edge - x).clamp(0.0, depth))),
        );
        Some(Sides {
            left_depth,
            right_depth,
        })
    }
}

/// The weight of each of `lines` lines: `edge_weight + (1 − edge_weight) ·
/// sin(π·t)`, with `t` the line's position in the zone (band centres), so
/// the middle of the zone counts fully and the edges count `edge_weight`.
fn line_weights(lines: usize, edge_weight: f64) -> Vec<f64> {
    (0..lines)
        .map(|i| {
            let t = (i as f64 + 0.5) / lines as f64;
            edge_weight + (1.0 - edge_weight) * (std::f64::consts::PI * t).sin()
        })
        .collect()
}

/// The white of a side with `margin` units between the edge and the
/// glyph's extreme: the area between the clipped profile and the edge,
/// divided by the zone height.
fn white(margin: f64, depth: f64) -> f64 {
    margin + depth
}

/// The margin, within `range`, whose white matches `target` to within
/// [`TOLERANCE`], by bisection.
fn solve_margin(target: f64, depth: f64, range: (f64, f64)) -> f64 {
    let (mut lo, mut hi) = range;
    while hi - lo > TOLERANCE {
        let mid = 0.5 * (lo + hi);
        if white(mid, depth) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// The stem width of a glyph: the median width of the ink runs on the lines
/// of the middle half of the cap height (centred on half the cap height).
fn stem_width(outline: &Outline, cap_height: f64, lines: usize) -> Option<f64> {
    let band = Zone {
        bottom: 0.25 * cap_height,
        top: 0.75 * cap_height,
    };
    let mut widths: Vec<f64> = line_heights(band, lines)
        .into_iter()
        .flat_map(|y| outline.spans(y))
        .map(|(start, end)| end - start)
        .collect();
    if widths.is_empty() {
        return None;
    }
    widths.sort_by(f64::total_cmp);
    let mid = widths.len() / 2;
    Some(if widths.len() % 2 == 1 {
        widths[mid]
    } else {
        0.5 * (widths[mid - 1] + widths[mid])
    })
}

/// Checks the parameters of [`auto_spacing`].
fn check_params(params: &SpacingParams) -> Result<(), SpacingError> {
    if !(params.tightness.is_finite() && params.tightness >= 0.0) {
        return Err(SpacingError::InvalidParameter(format!(
            "tightness must be a finite number ≥ 0, got {}",
            params.tightness
        )));
    }
    if !(params.depth_ratio.is_finite() && params.depth_ratio > 0.0 && params.depth_ratio <= 10.0) {
        return Err(SpacingError::InvalidParameter(format!(
            "depth_ratio must be within (0, 10], got {}",
            params.depth_ratio
        )));
    }
    if !(0.0..=1.0).contains(&params.edge_weight) {
        return Err(SpacingError::InvalidParameter(format!(
            "edge_weight must be within [0, 1], got {}",
            params.edge_weight
        )));
    }
    check_lines(params.lines)
}

/// The sides of a glyph measured over `zone`, or `None` without ink.
fn sides_of(outline: &Outline, zone: Zone, params: &SpacingParams, depth: f64) -> Option<Sides> {
    let bbox = outline.bbox?;
    Sides::of(
        &outline.glyph_profile(zone, params.lines),
        (bbox.x0, bbox.x1),
        depth,
        params.edge_weight,
    )
}

/// The outline of the reference glyph `name`.
fn reference(font: &Font, master: MasterId, name: &str) -> Result<Outline, SpacingError> {
    let glyph = font
        .glyph_by_name(name)
        .ok_or_else(|| SpacingError::MissingReference(format!("glyph {name} does not exist")))?;
    Outline::of(font, glyph.id, master)
}

/// Computes the sidebearings of `glyphs` in `master` (spec §6.2).
///
/// 1. The stem width of the first reference (`H`) is the median width of
///    its ink runs on `params.lines` lines of the middle half of the cap
///    height. Its margin is `tightness ×` the stem, and the profile depth
///    is `depth_ratio ×` the stem: the white the eye takes in next to a
///    letter is bounded by the letter's own stroke weight, not by the em.
/// 2. Each glyph is profiled on `params.lines` lines of the
///    [measurement zone](measurement_zone) (its own ink height if it has
///    ink on fewer than 8 of them). On each line the depth of a side is
///    the distance from the bounding box to the ink, clipped at the profile
///    depth; lines without ink count the full depth.
/// 3. The white of a side is the margin (sidebearing) plus the weighted
///    mean depth, the weight of a line being `edge_weight + (1 −
///    edge_weight)·sin(π·t)` at position `t` of the zone: white near the
///    top and bottom of the zone (above an apex, below a bowl) reads as
///    line space more than as letter space, so it counts less.
/// 4. The target white is the mean white of the two sides of `H` with its
///    margin. Each side of each glyph gets the margin whose white matches
///    the target, by bisection to within 0.5 units, rounded to an integer.
///    A margin that would be negative is clamped to 0: pairs that need to
///    come closer (`AV`, `AA`) are left to kerning. The second reference
///    (`O`) must exist and have ink, but it does not set the target: it is
///    spaced from the same white as every other glyph.
///
/// Glyphs whose layer has components and no contours of its own
/// (composites inherit the spacing of their bases) and glyphs without ink
/// are skipped. The result follows the order of `glyphs`, each glyph once.
///
/// # Errors
///
/// * [`SpacingError::InvalidParameter`] if `tightness` is negative or not
///   finite, `depth_ratio` is not within `(0, 10]`, `edge_weight` is not
///   within `[0, 1]`, `lines` is not within
///   [`MIN_LINES`](crate::MIN_LINES)`..=`[`MAX_LINES`](crate::MAX_LINES),
///   or the master or a glyph does not exist.
/// * [`SpacingError::MissingReference`] if a reference glyph does not exist
///   or has no ink (or the first one has no stem).
/// * [`SpacingError::MissingLayer`] if a reference or a listed glyph has no
///   layer in `master`.
/// * [`SpacingError::Geometry`] if an outline cannot be expanded.
pub fn auto_spacing(
    font: &Font,
    master: MasterId,
    glyphs: &[GlyphId],
    params: &SpacingParams,
) -> Result<Vec<Sidebearings>, SpacingError> {
    check_params(params)?;
    let cap_height = font
        .masters
        .iter()
        .find(|m| m.id == master)
        .ok_or_else(|| SpacingError::InvalidParameter(format!("master {master} does not exist")))?
        .metrics
        .cap_height;
    let upm = f64::from(font.units_per_em);
    let zone = measurement_zone(font, master);

    let [first, second] = &params.references;
    let no_ink = |name: &str| SpacingError::MissingReference(format!("glyph {name} has no ink"));
    let h_outline = reference(font, master, first)?;
    let o_outline = reference(font, master, second)?;
    let stem = stem_width(&h_outline, cap_height, params.lines).ok_or_else(|| no_ink(first))?;
    let depth = params.depth_ratio * stem;
    let h_sides = sides_of(&h_outline, zone, params, depth).ok_or_else(|| no_ink(first))?;
    sides_of(&o_outline, zone, params, depth).ok_or_else(|| no_ink(second))?;
    let margin = params.tightness * stem;
    let target = 0.5 * (white(margin, h_sides.left_depth) + white(margin, h_sides.right_depth));
    let range = (-2.0 * upm, 2.0 * upm);
    let side = |depth: f64| solve_margin(target, depth, range).max(0.0).round();

    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for &glyph in glyphs {
        if !seen.insert(glyph) {
            continue;
        }
        let own = layer(font, glyph, master)?;
        if own.paths.is_empty() && !own.components.is_empty() {
            continue;
        }
        let outline = Outline::of(font, glyph, master)?;
        let Some(sides) = sides_of(&outline, zone, params, depth) else {
            continue;
        };
        result.push(Sidebearings {
            glyph,
            lsb: side(sides.left_depth),
            rsb: side(sides.right_depth),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bisection_lands_within_tolerance() {
        let m = solve_margin(100.0, 30.0, (-2000.0, 2000.0));
        assert!((m - 70.0).abs() <= TOLERANCE, "{m}");
        // Out of range: the nearest end.
        let m = solve_margin(1e9, 0.0, (-10.0, 10.0));
        assert!((m - 10.0).abs() <= TOLERANCE, "{m}");
    }

    #[test]
    fn weights_are_full_in_the_middle_and_edge_weight_at_the_edges() {
        let w = line_weights(1000, 0.25);
        assert!((w[0] - 0.25).abs() < 0.01 && (w[999] - 0.25).abs() < 0.01);
        assert!((w[500] - 1.0).abs() < 1e-4);
        assert!(
            line_weights(8, 1.0)
                .iter()
                .all(|&x| (x - 1.0).abs() < 1e-12)
        );
    }

    #[test]
    fn sides_count_missing_lines_as_full_depth() {
        let profile = Profile {
            ys: vec![0.0, 1.0, 2.0, 3.0],
            left: vec![Some(10.0), Some(40.0), Some(500.0), None],
            right: vec![Some(90.0), Some(90.0), Some(60.0), None],
        };
        // Uniform weights: plain means.
        let sides = Sides::of(&profile, (10.0, 90.0), 100.0, 1.0).expect("ink");
        assert_eq!(sides.left_depth, (0.0 + 30.0 + 100.0 + 100.0) / 4.0);
        assert_eq!(sides.right_depth, (0.0 + 0.0 + 30.0 + 100.0) / 4.0);
        let empty = Profile {
            ys: vec![0.0],
            left: vec![None],
            right: vec![None],
        };
        assert!(Sides::of(&empty, (0.0, 1.0), 100.0, 1.0).is_none());
    }
}
