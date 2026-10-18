//! Automatic spacing: sidebearings that give every glyph the white of `H`.

use std::collections::HashSet;

use forma_model::{Font, GlyphId, MasterId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SpacingError;
use crate::profile::{Outline, Profile, Zone, check_lines, layer, line_heights, measurement_zone};

/// Bisection stops when the margin is known to within this many units.
const TOLERANCE: f64 = 0.5;

/// The parameters of [`auto_spacing`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpacingParams {
    /// The margin of the first reference as a fraction of its stem width:
    /// lower is tighter. 0.6 by default, suited to display sizes.
    pub tightness: f64,
    /// The names of the reference glyphs: the first (`H`) sets the target
    /// white; the second (`O`) must exist and is spaced from the same white.
    pub references: [String; 2],
    /// How deep into the glyph the profile counts, as a fraction of the
    /// units per em (0.15 by default).
    pub depth_ratio: f64,
    /// How many profile lines measure each glyph (64 by default).
    pub lines: usize,
}

impl Default for SpacingParams {
    fn default() -> Self {
        SpacingParams {
            tightness: 0.6,
            references: ["H".to_owned(), "O".to_owned()],
            depth_ratio: 0.15,
            lines: 64,
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
    /// The leftmost and rightmost ink of the profile.
    left_edge: f64,
    right_edge: f64,
    /// The mean depth of the clipped profile on each side: the white inside
    /// the glyph's extremes, per unit of zone height.
    left_depth: f64,
    right_depth: f64,
}

impl Sides {
    /// The sides of `profile`, with the profile clipped `depth` units in from
    /// the extremes; lines without ink count as the full depth. `None` if
    /// the profile has no ink.
    fn of(profile: &Profile, depth: f64) -> Option<Sides> {
        let left_edge = profile.left.iter().flatten().copied().reduce(f64::min)?;
        let right_edge = profile.right.iter().flatten().copied().reduce(f64::max)?;
        let lines = profile.ys.len() as f64;
        let mean = |values: &mut dyn Iterator<Item = f64>| values.sum::<f64>() / lines;
        let left_depth = mean(
            &mut profile
                .left
                .iter()
                .map(|x| x.map_or(depth, |x| (x - left_edge).min(depth))),
        );
        let right_depth = mean(
            &mut profile
                .right
                .iter()
                .map(|x| x.map_or(depth, |x| (right_edge - x).min(depth))),
        );
        Some(Sides {
            left_edge,
            right_edge,
            left_depth,
            right_depth,
        })
    }
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
    if !(params.depth_ratio.is_finite() && params.depth_ratio > 0.0 && params.depth_ratio <= 1.0) {
        return Err(SpacingError::InvalidParameter(format!(
            "depth_ratio must be within (0, 1], got {}",
            params.depth_ratio
        )));
    }
    check_lines(params.lines)
}

/// A reference glyph, its outline and its sides.
fn reference(
    font: &Font,
    master: MasterId,
    name: &str,
    zone: Zone,
    params: &SpacingParams,
    depth: f64,
) -> Result<(Outline, Sides), SpacingError> {
    let glyph = font
        .glyph_by_name(name)
        .ok_or_else(|| SpacingError::MissingReference(format!("glyph {name} does not exist")))?;
    let outline = Outline::of(font, glyph.id, master)?;
    let sides = Sides::of(&outline.glyph_profile(zone, params.lines), depth)
        .ok_or_else(|| SpacingError::MissingReference(format!("glyph {name} has no ink")))?;
    Ok((outline, sides))
}

/// Computes the sidebearings of `glyphs` in `master` (spec §6.2).
///
/// 1. Each glyph is profiled on `params.lines` lines of the
///    [measurement zone](measurement_zone) (its own ink height if it has
///    ink on fewer than 8 of them), and the profile is clipped
///    `depth_ratio × UPM` units in from the leftmost and rightmost ink.
/// 2. The white of a side is the area between the clipped profile and the
///    sidebearing edge divided by the zone height: the margin (edge to
///    extreme) plus the mean clipped depth, lines without ink counting the
///    full depth.
/// 3. The stem width of the first reference (`H`) is the median width of
///    its ink runs on the lines of the middle half of the cap height; its
///    margin is `tightness ×` that width, and the target white is the mean
///    white of its two sides with that margin.
/// 4. Each side of each glyph gets the margin whose white matches the
///    target, by bisection to within 0.5 units, converted to a sidebearing
///    from the bounding box and rounded to an integer. The second reference
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
///   finite, `depth_ratio` is not within `(0, 1]`, `lines` is not within
///   [`MIN_LINES`](crate::MIN_LINES)`..=`[`MAX_LINES`](crate::MAX_LINES),
///   the master or a glyph does not exist.
/// * [`SpacingError::MissingReference`] if a reference glyph does not exist
///   or has no ink.
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
    let depth = params.depth_ratio * upm;

    let [first, second] = &params.references;
    let (h_outline, h_sides) = reference(font, master, first, zone, params, depth)?;
    reference(font, master, second, zone, params, depth)?;
    let stem = stem_width(&h_outline, cap_height, params.lines)
        .ok_or_else(|| SpacingError::MissingReference(format!("glyph {first} has no stem")))?;
    let margin = params.tightness * stem;
    let target = 0.5 * (white(margin, h_sides.left_depth) + white(margin, h_sides.right_depth));
    let range = (-2.0 * upm, 2.0 * upm);

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
        let Some(bbox) = outline.bbox else {
            continue;
        };
        let Some(sides) = Sides::of(&outline.glyph_profile(zone, params.lines), depth) else {
            continue;
        };
        let left_margin = solve_margin(target, sides.left_depth, range);
        let right_margin = solve_margin(target, sides.right_depth, range);
        // The margins run to the profile's extremes; the sidebearings run
        // to the bounding box, which can reach further (ink outside the
        // zone, or between the lines).
        result.push(Sidebearings {
            glyph,
            lsb: (left_margin - (sides.left_edge - bbox.x0)).round(),
            rsb: (right_margin - (bbox.x1 - sides.right_edge)).round(),
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
    fn sides_count_missing_lines_as_full_depth() {
        let profile = Profile {
            ys: vec![0.0, 1.0, 2.0, 3.0],
            left: vec![Some(10.0), Some(40.0), Some(500.0), None],
            right: vec![Some(90.0), Some(90.0), Some(60.0), None],
        };
        let sides = Sides::of(&profile, 100.0).expect("ink");
        assert_eq!((sides.left_edge, sides.right_edge), (10.0, 90.0));
        assert_eq!(sides.left_depth, (0.0 + 30.0 + 100.0 + 100.0) / 4.0);
        assert_eq!(sides.right_depth, (0.0 + 0.0 + 30.0 + 100.0) / 4.0);
        let empty = Profile {
            ys: vec![0.0],
            left: vec![None],
            right: vec![None],
        };
        assert!(Sides::of(&empty, 100.0).is_none());
    }
}
