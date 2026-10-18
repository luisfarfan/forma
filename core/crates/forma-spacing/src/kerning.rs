//! Automatic kerning: pairs whose optical gap differs from that of `HH`,
//! gathered into classes of glyphs whose sides look alike, with a guard
//! that never lets a pair's ink come closer than a minimum distance.

use std::collections::HashSet;

use forma_model::{Font, GlyphId, KernSide, MasterId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SpacingError;
use crate::collision::{Grid, InkSamples, grid_for, load_outlines};
use crate::profile::{Outline, Zone, check_lines, layer, line_heights, measurement_zone};
use crate::spacing::{
    DEFAULT_DEPTH_RATIO, DEFAULT_EDGE_WEIGHT, line_weight, reference, stem_width,
};

/// The most distinct glyphs one call of [`auto_kerning`] kerns: every
/// ordered pair is measured, so 500 glyphs are 250 000 pairs, as many as
/// one [`collisions`](crate::collisions) check.
pub const MAX_KERNING_GLYPHS: usize = 500;

/// The glyph that is never kerned.
const NOTDEF: &str = ".notdef";

/// The glyph whose pair with itself sets the reference gap.
const REFERENCE: &str = "H";

/// How far from the advance edge the sides of two glyphs are compared for
/// grouping, as a fraction of the units per em: white deeper than this
/// counts as this deep, and a line without ink as this deep.
const GROUP_DEPTH_RATIO: f64 = 0.25;

/// How many one-unit steps the guard takes past its computed bound to
/// absorb rounding in the distance (one is always enough in practice).
const GUARD_STEPS: usize = 8;

/// The parameters of [`auto_kerning`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KerningParams {
    /// How much of the difference between the reference gap and a pair's
    /// gap the kern corrects (0.8 by default).
    pub strength: f64,
    /// The largest kern in absolute value, as a fraction of the units per
    /// em (0.25 by default); the collision guard may open a pair further.
    pub max_ratio: f64,
    /// Kerns smaller than this in absolute value, in font units, are not
    /// made (10 by default).
    pub min_value: f64,
    /// The smallest horizontal distance a kerned pair's ink may come to, as
    /// a fraction of the units per em (0.02 by default).
    pub min_distance_ratio: f64,
    /// Two glyph sides join a group when their profiles differ by at most
    /// this fraction of the units per em per line on average (0.03 by
    /// default).
    pub group_threshold_ratio: f64,
    /// A member of a group gets its own pair when its kern differs from the
    /// group's by more than this many units (10 by default).
    pub exception_threshold: f64,
    /// Whether glyph sides are gathered into groups (true by default).
    pub grouping: bool,
    /// How many lines compare the sides of two glyphs for grouping (64 by
    /// default).
    pub lines: usize,
    /// How deep into a glyph its own white counts, as a multiple of the
    /// stem width of `H`: the same depth as the spacing (0.4 by default).
    pub depth_ratio: f64,
    /// The weight of the gap on the lines at the top and bottom of the
    /// measurement zone, relative to the middle: the same weights as the
    /// spacing (0.25 by default).
    pub edge_weight: f64,
}

impl Default for KerningParams {
    fn default() -> Self {
        KerningParams {
            strength: 0.8,
            max_ratio: 0.25,
            min_value: 10.0,
            min_distance_ratio: 0.02,
            group_threshold_ratio: 0.03,
            exception_threshold: 10.0,
            grouping: true,
            lines: 64,
            depth_ratio: DEFAULT_DEPTH_RATIO,
            edge_weight: DEFAULT_EDGE_WEIGHT,
        }
    }
}

/// One side of a planned pair: a glyph, or a group of the plan by name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PlannedRef {
    /// One glyph.
    Glyph(GlyphId),
    /// The group of the plan with this name, on the side of the pair it
    /// stands on.
    Group(String),
}

/// A kerning group of a plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedGroup {
    /// The group name: the name of its first member, a valid glyph name,
    /// unique on its side.
    pub name: String,
    /// The side of a pair the group stands on.
    pub side: KernSide,
    /// The members, in name order; at least two.
    pub members: Vec<GlyphId>,
}

/// A kerning pair of a plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedPair {
    /// The left side.
    pub first: PlannedRef,
    /// The right side.
    pub second: PlannedRef,
    /// The kern in whole font units: never 0, at most twice the units per
    /// em in absolute value.
    pub value: f64,
}

/// The groups and pairs computed by [`auto_kerning`] for one master.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KerningPlan {
    /// The groups: first-side groups, then second-side ones, each by name.
    pub groups: Vec<PlannedGroup>,
    /// The pairs, sorted by the names of their sides (a glyph before a
    /// group of the same name).
    pub pairs: Vec<PlannedPair>,
}

/// A glyph being kerned.
struct Kerned {
    id: GlyphId,
    name: String,
    advance: f64,
    /// The left and right of its bounding box.
    x0: f64,
    x1: f64,
    samples: InkSamples,
    outline: Outline,
}

/// The kern a pair of glyphs wants on its own.
#[derive(Clone, Copy, Debug)]
struct PairKern {
    /// The kern after the minimum and the guard: what the pair should get.
    target: f64,
    /// The kern without the minimum, after the guard: what a pair that
    /// needs its own value but targets 0 stores (0 cannot be stored).
    unfiltered: f64,
}

impl PairKern {
    /// The value an exception for this pair stores: its target, or for a
    /// target of 0 (which cannot be stored) its kern before the minimum, or
    /// 1 if that is 0 too. Both are guard-safe: the unfiltered kern was
    /// guarded, and 1 opens the pair more than 0, which is safe.
    fn stored(self) -> f64 {
        if self.target != 0.0 {
            self.target
        } else if self.unfiltered != 0.0 {
            self.unfiltered
        } else {
            1.0
        }
    }
}

/// How pairs are measured and guarded.
struct Measure {
    grid: Grid,
    zone: Zone,
    edge_weight: f64,
    depth: f64,
    min_distance: f64,
    /// The largest storable kern: twice the units per em.
    limit: f64,
}

impl Measure {
    /// The optical gap between `left` and `right` set side by side without
    /// kerning, or `None` if they have no line of ink in common.
    ///
    /// On each grid line where both have ink, the gap is `g = (advance_L −
    /// right_L) + left_R`. Two clipped versions of it are compared:
    /// * each glyph's own white clipped at `depth` from its bounding box, as
    ///   in the spacing: what the spacing has already balanced;
    /// * the gap clipped at `depth` beyond the pair's closest approach: a
    ///   channel of white the two glyphs never close (`AV`, `To`) counts in
    ///   full, while white that opens away from the closest point (`AH`,
    ///   `OO`) does not count more than the spacing let it.
    ///
    /// The line's gap is the larger of the two, and the optical gap is
    /// their mean weighted as in the spacing.
    fn gap(&self, left: &Kerned, right: &Kerned) -> Option<f64> {
        let lines: Vec<(f64, f64, f64)> = InkSamples::shared_lines(&left.samples, &right.samples)
            .filter_map(|k| {
                let (_, r) = left.samples.at(k)?;
                let (l, _) = right.samples.at(k)?;
                let gap = left.advance - r + l;
                let own = (left.advance - left.x1)
                    + (left.x1 - r).clamp(0.0, self.depth)
                    + right.x0
                    + (l - right.x0).clamp(0.0, self.depth);
                Some((self.grid.y(k), gap, own))
            })
            .collect();
        let closest = lines.iter().map(|line| line.1).reduce(f64::min)?;
        let reach = closest + self.depth;
        let mean = |weight: &dyn Fn(f64) -> f64| {
            let (mut total, mut sum) = (0.0, 0.0);
            for &(y, gap, own) in &lines {
                let w = weight(y);
                total += w;
                sum += w * own.max(gap.min(reach));
            }
            (total > 0.0).then(|| sum / total)
        };
        mean(&|y| line_weight(y, self.zone, self.edge_weight)).or_else(|| mean(&|_| 1.0))
    }

    /// Whether `left` then `right` kerned by `value` keep their ink at
    /// least the minimum distance apart, measured as
    /// [`collisions`](crate::collisions) does.
    fn safe(&self, left: &Kerned, right: &Kerned, value: f64) -> bool {
        InkSamples::min_distance(&left.samples, &right.samples, left.advance + value)
            .is_none_or(|distance| distance >= self.min_distance)
    }

    /// `value` rounded and raised, if needed, until the pair is safe: the
    /// collision guard. Capped at the largest storable kern.
    fn guard(&self, left: &Kerned, right: &Kerned, value: f64) -> f64 {
        let mut v = value.round();
        if let Some(distance) =
            InkSamples::min_distance(&left.samples, &right.samples, left.advance)
        {
            v = v.max((self.min_distance - distance).ceil());
        }
        for _ in 0..GUARD_STEPS {
            if self.safe(left, right, v) {
                break;
            }
            v += 1.0;
        }
        v.clamp(-self.limit, self.limit) + 0.0
    }
}

/// Checks the parameters of [`auto_kerning`].
fn check_params(params: &KerningParams) -> Result<(), SpacingError> {
    let invalid = |what: &str, value: f64| {
        Err(SpacingError::InvalidParameter(format!(
            "{what}, got {value}"
        )))
    };
    if !(params.strength.is_finite() && params.strength >= 0.0) {
        return invalid("strength must be a finite number ≥ 0", params.strength);
    }
    if !(0.0..=2.0).contains(&params.max_ratio) {
        return invalid("max_ratio must be within [0, 2]", params.max_ratio);
    }
    if !(params.min_value.is_finite() && params.min_value >= 0.0) {
        return invalid("min_value must be a finite number ≥ 0", params.min_value);
    }
    if !(-2.0..=2.0).contains(&params.min_distance_ratio) {
        return invalid(
            "min_distance_ratio must be within [-2, 2]",
            params.min_distance_ratio,
        );
    }
    if !(params.group_threshold_ratio.is_finite() && params.group_threshold_ratio >= 0.0) {
        return invalid(
            "group_threshold_ratio must be a finite number ≥ 0",
            params.group_threshold_ratio,
        );
    }
    if !(params.exception_threshold.is_finite() && params.exception_threshold >= 0.0) {
        return invalid(
            "exception_threshold must be a finite number ≥ 0",
            params.exception_threshold,
        );
    }
    if !(params.depth_ratio.is_finite() && params.depth_ratio > 0.0 && params.depth_ratio <= 10.0) {
        return invalid("depth_ratio must be within (0, 10]", params.depth_ratio);
    }
    if !(0.0..=1.0).contains(&params.edge_weight) {
        return invalid("edge_weight must be within [0, 1]", params.edge_weight);
    }
    check_lines(params.lines)
}

/// The white on one side of a glyph on each line of `ys`, measured from
/// its advance edge (origin on the left, advance on the right) and clipped
/// to `±cap`; `None` on a line without ink.
fn side_profile(glyph: &Kerned, side: KernSide, ys: &[f64], cap: f64) -> Vec<Option<f64>> {
    ys.iter()
        .map(|&y| {
            let (l, r) = glyph.outline.extent(y)?;
            let white = match side {
                KernSide::First => glyph.advance - r,
                KernSide::Second => l,
            };
            Some(white.clamp(-cap, cap))
        })
        .collect()
}

/// The mean absolute difference between two side profiles over the lines
/// where either has ink, a line without ink counting as `cap`; `None` if
/// neither has ink on any line.
fn profile_distance(a: &[Option<f64>], b: &[Option<f64>], cap: f64) -> Option<f64> {
    let (mut lines, mut sum) = (0usize, 0.0);
    for (x, y) in a.iter().zip(b) {
        if x.is_none() && y.is_none() {
            continue;
        }
        lines += 1;
        sum += (x.unwrap_or(cap) - y.unwrap_or(cap)).abs();
    }
    (lines > 0).then(|| sum / lines as f64)
}

/// Gathers the sides `side` of `glyphs` (in name order) into clusters:
/// each glyph joins the first cluster whose first member's profile differs
/// from its own by at most `threshold`, or starts a new one. Clusters are
/// lists of indices into `glyphs`, in creation order, members in name
/// order.
fn cluster(
    glyphs: &[Kerned],
    side: KernSide,
    ys: &[f64],
    cap: f64,
    threshold: f64,
) -> Vec<Vec<usize>> {
    let profiles: Vec<Vec<Option<f64>>> = glyphs
        .iter()
        .map(|g| side_profile(g, side, ys, cap))
        .collect();
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for (i, profile) in profiles.iter().enumerate() {
        let home = clusters.iter_mut().find(|members| {
            profile_distance(&profiles[members[0]], profile, cap).is_some_and(|d| d <= threshold)
        });
        match home {
            Some(members) => members.push(i),
            None => clusters.push(vec![i]),
        }
    }
    clusters
}

/// The median of `values` (not empty): the middle value or, with an even
/// count, the one of the two middle values nearer 0 (both minimise the
/// sum of differences; the smaller kern means fewer and smaller pairs).
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else if values[mid - 1].abs() <= values[mid].abs() {
        values[mid - 1]
    } else {
        values[mid]
    }
}

/// Computes kerning groups and pairs for `glyphs` in `master` (spec §6.2).
///
/// **Pairs.** Every glyph is sampled once on the grid of
/// [`collisions`](crate::collisions) for the same glyphs. For each ordered
/// pair `(L, R)` the optical gap is measured as described below, and the
/// reference is the gap of `H` with itself. The kern is
/// `clamp((reference − gap) × strength, ±max_ratio·UPM)`, dropped (0) when
/// smaller than `min_value` in absolute value. The **collision guard** then
/// raises it, if needed, until the pair's ink is at least
/// `min_distance_ratio·UPM` apart on every shared line — the same measure
/// as [`collisions`](crate::collisions), so checking the applied plan with
/// that distance finds nothing (up to the storable limit of 2·UPM). Kerns
/// are whole units. Pairs without a common line of ink are not kerned.
///
/// **Optical gap.** On each shared line the gap is
/// `(advance_L − right_L) + left_R`. The line counts the larger of two clipped gaps: the glyphs'
/// own white clipped at `depth` (`depth_ratio ×` the stem width of `H`)
/// from their bounding boxes, which is what the spacing balances, and the
/// gap clipped at `depth` beyond the pair's closest approach, which
/// catches channels of white the pair never closes (`AV`, `To`). The lines
/// are averaged with the spacing's weights (`edge_weight` at the top and
/// bottom of the measurement zone, 1 in its middle).
///
/// **Groups.** With `grouping`, the right sides (groups on the `First`
/// side of pairs) and left sides (`Second`) are gathered separately: each
/// side is profiled on `lines` lines spanning all the glyphs, as its white
/// from the advance edge clipped at 25 % of the UPM (a line without ink
/// counts as that deep). In name order, a glyph joins the first group
/// whose first member's profile differs from its own by at most
/// `group_threshold_ratio·UPM` per line on average (over the lines where
/// either has ink); groups are named after that first member, and groups
/// of one glyph are not made.
///
/// **Class values.** Each pair of classes (a group or an ungrouped glyph
/// on each side) gets the median of its members' kerns, omitted when
/// smaller than `min_value`. A member pair gets its own glyph-to-glyph
/// pair (an exception) when its kern differs from the class value by more
/// than `exception_threshold`, or when the class value would break the
/// guard for it; an exception for a pair that wants 0 stores its kern
/// before the minimum, or 1 if that rounds to 0 too (a stored kern is
/// never 0).
///
/// The plan assumes it replaces the master's kerning for these glyphs:
/// applied to a master without other pairs or groups for them, each
/// glyph pair gets exactly the kern described. Glyphs without ink and
/// `.notdef` (never typeset on purpose) are left out. The result is deterministic: it does not depend on the order of
/// `glyphs` or on duplicates in it.
///
/// # Errors
///
/// * [`SpacingError::InvalidParameter`] if a parameter is out of range
///   (non-finite or negative `strength`, `max_ratio` outside `[0, 2]`,
///   negative `min_value`, `min_distance_ratio` outside `[−2, 2]`, negative
///   thresholds, `lines` outside
///   [`MIN_LINES`](crate::MIN_LINES)`..=`[`MAX_LINES`](crate::MAX_LINES),
///   `depth_ratio` outside `(0, 10]`, `edge_weight` outside `[0, 1]`), there
///   are more than [`MAX_KERNING_GLYPHS`] distinct glyphs, or the master or
///   a glyph does not exist.
/// * [`SpacingError::MissingReference`] if `H` does not exist or has no
///   ink.
/// * [`SpacingError::MissingLayer`] if `H` or a glyph has no layer in
///   `master`.
/// * [`SpacingError::Geometry`] if an outline cannot be expanded.
pub fn auto_kerning(
    font: &Font,
    master: MasterId,
    glyphs: &[GlyphId],
    params: &KerningParams,
) -> Result<KerningPlan, SpacingError> {
    check_params(params)?;
    let cap_height = font
        .masters
        .iter()
        .find(|m| m.id == master)
        .ok_or_else(|| SpacingError::InvalidParameter(format!("master {master} does not exist")))?
        .metrics
        .cap_height;
    let mut seen = HashSet::new();
    let distinct: Vec<GlyphId> = glyphs.iter().copied().filter(|g| seen.insert(*g)).collect();
    // `.notdef` stands in for missing characters: it is never kerned.
    let notdef = font.glyph_by_name(NOTDEF).map(|glyph| glyph.id);
    let distinct: Vec<GlyphId> = distinct
        .into_iter()
        .filter(|g| Some(*g) != notdef)
        .collect();
    if distinct.len() > MAX_KERNING_GLYPHS {
        return Err(SpacingError::InvalidParameter(format!(
            "at most {MAX_KERNING_GLYPHS} glyphs can be kerned at once, got {}",
            distinct.len()
        )));
    }
    let upm = f64::from(font.units_per_em);
    let mut outlines = load_outlines(font, master, distinct.iter().copied())?;
    let grid = grid_for(font.units_per_em, outlines.values());

    // The reference: the stem of H sets the depth, HH the gap.
    let no_ink = || SpacingError::MissingReference(format!("glyph {REFERENCE} has no ink"));
    let h_outline = reference(font, master, REFERENCE)?;
    let h_id = font
        .glyph_by_name(REFERENCE)
        .map(|glyph| glyph.id)
        .ok_or_else(no_ink)?;
    let stem = stem_width(&h_outline, cap_height, params.lines).ok_or_else(no_ink)?;
    let measure = Measure {
        grid,
        zone: measurement_zone(font, master),
        edge_weight: params.edge_weight,
        depth: params.depth_ratio * stem,
        min_distance: params.min_distance_ratio * upm,
        limit: 2.0 * upm,
    };
    let h = kerned(font, master, h_id, h_outline, grid)?.ok_or_else(no_ink)?;
    let reference_gap = measure.gap(&h, &h).ok_or_else(no_ink)?;

    // The glyphs with ink, in name order.
    let mut kerned_glyphs = Vec::new();
    for id in distinct {
        if let Some(outline) = outlines.remove(&id)
            && let Some(glyph) = kerned(font, master, id, outline, grid)?
        {
            kerned_glyphs.push(glyph);
        }
    }
    kerned_glyphs.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    let glyphs = kerned_glyphs;

    // What each pair wants on its own.
    let max = params.max_ratio * upm;
    let kerns: Vec<Vec<Option<PairKern>>> = glyphs
        .iter()
        .map(|left| {
            glyphs
                .iter()
                .map(|right| {
                    let gap = measure.gap(left, right)?;
                    let raw = ((reference_gap - gap) * params.strength).clamp(-max, max);
                    let filtered = if raw.abs() < params.min_value {
                        0.0
                    } else {
                        raw
                    };
                    Some(PairKern {
                        target: measure.guard(left, right, filtered),
                        unfiltered: measure.guard(left, right, raw),
                    })
                })
                .collect()
        })
        .collect();

    // The classes of each side.
    let singletons = || (0..glyphs.len()).map(|i| vec![i]).collect::<Vec<_>>();
    let (first, second) = match (params.grouping, grouping_lines(&glyphs, params.lines)) {
        (true, Some(ys)) => {
            let cap = GROUP_DEPTH_RATIO * upm;
            let threshold = params.group_threshold_ratio * upm;
            (
                cluster(&glyphs, KernSide::First, &ys, cap, threshold),
                cluster(&glyphs, KernSide::Second, &ys, cap, threshold),
            )
        }
        _ => (singletons(), singletons()),
    };
    // A class's reference and its sort key: its name, a glyph before a
    // group of the same name.
    let class_ref = |members: &[usize]| {
        let name = glyphs[members[0]].name.as_str();
        match members {
            [single] => (PlannedRef::Glyph(glyphs[*single].id), (name, 0)),
            _ => (PlannedRef::Group(name.to_owned()), (name, 1)),
        }
    };

    // Class values and exceptions.
    type Key<'a> = ((&'a str, u8), (&'a str, u8));
    let mut pairs: Vec<(Key, PlannedPair)> = Vec::new();
    let mut push = |(first, first_key), (second, second_key), value: f64| {
        pairs.push((
            (first_key, second_key),
            PlannedPair {
                first,
                second,
                value,
            },
        ));
    };
    for a in &first {
        for b in &second {
            let mut members: Vec<(usize, usize, PairKern)> = Vec::new();
            for &l in a {
                for &r in b {
                    if let Some(kern) = kerns[l][r] {
                        members.push((l, r, kern));
                    }
                }
            }
            if members.is_empty() {
                continue;
            }
            let mut targets: Vec<f64> = members.iter().map(|m| m.2.target).collect();
            let mut class = median(&mut targets);
            if class.abs() < params.min_value {
                class = 0.0;
            }
            if class != 0.0 {
                push(class_ref(a), class_ref(b), class);
            }
            for (l, r, kern) in members {
                let (left, right) = (&glyphs[l], &glyphs[r]);
                let differs = (class - kern.target).abs() > params.exception_threshold;
                let exception =
                    kern.target != class && (differs || !measure.safe(left, right, class));
                if exception {
                    push(
                        (PlannedRef::Glyph(left.id), (left.name.as_str(), 0)),
                        (PlannedRef::Glyph(right.id), (right.name.as_str(), 0)),
                        kern.stored(),
                    );
                }
            }
        }
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let mut groups = Vec::new();
    for (side, clusters) in [(KernSide::First, &first), (KernSide::Second, &second)] {
        let mut side_groups: Vec<PlannedGroup> = clusters
            .iter()
            .filter(|members| members.len() >= 2)
            .map(|members| PlannedGroup {
                name: glyphs[members[0]].name.clone(),
                side,
                members: members.iter().map(|&i| glyphs[i].id).collect(),
            })
            .collect();
        side_groups.sort_by(|a, b| a.name.cmp(&b.name));
        groups.extend(side_groups);
    }
    Ok(KerningPlan {
        groups,
        pairs: pairs.into_iter().map(|(_, pair)| pair).collect(),
    })
}

/// The glyph `id` prepared for kerning, or `None` without ink.
fn kerned(
    font: &Font,
    master: MasterId,
    id: GlyphId,
    outline: Outline,
    grid: Grid,
) -> Result<Option<Kerned>, SpacingError> {
    let advance = layer(font, id, master)?.advance_width;
    let name = font
        .glyphs
        .get(&id)
        .map(|glyph| glyph.name.clone())
        .unwrap_or_default();
    let (Some(bbox), Some(samples)) = (outline.bbox, InkSamples::new(&outline, grid)) else {
        return Ok(None);
    };
    Ok(Some(Kerned {
        id,
        name,
        advance,
        x0: bbox.x0,
        x1: bbox.x1,
        samples,
        outline,
    }))
}

/// The lines on which the sides of `glyphs` are compared for grouping:
/// `lines` lines over the union of their ink heights; `None` if that union
/// has no height.
fn grouping_lines(glyphs: &[Kerned], lines: usize) -> Option<Vec<f64>> {
    let bottom = glyphs
        .iter()
        .filter_map(|g| g.outline.bbox)
        .map(|b| b.y0)
        .reduce(f64::min)?;
    let top = glyphs
        .iter()
        .filter_map(|g| g.outline.bbox)
        .map(|b| b.y1)
        .reduce(f64::max)?;
    (top > bottom).then(|| line_heights(Zone { bottom, top }, lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_prefers_the_middle_value_nearer_zero() {
        assert_eq!(median(&mut [3.0, -40.0, 12.0]), 3.0);
        assert_eq!(median(&mut [0.0, 0.0, 24.0, 24.0]), 0.0);
        assert_eq!(median(&mut [-30.0, -20.0, 5.0, 40.0]), 5.0);
        assert_eq!(median(&mut [-30.0, -20.0, -15.0, 40.0]), -15.0);
        assert_eq!(median(&mut [-7.0]), -7.0);
    }

    #[test]
    fn exceptions_never_store_zero() {
        let kern = |target, unfiltered| PairKern { target, unfiltered };
        assert_eq!(kern(-40.0, -40.0).stored(), -40.0);
        assert_eq!(kern(0.0, -6.0).stored(), -6.0);
        assert_eq!(kern(0.0, 0.0).stored(), 1.0);
    }

    #[test]
    fn profile_distance_counts_lines_with_ink() {
        let cap = 250.0;
        let a = [Some(10.0), Some(20.0), None, None];
        let b = [Some(30.0), None, Some(250.0), None];
        // |10 − 30| + |20 − 250| + |250 − 250|, over 3 lines.
        assert_eq!(profile_distance(&a, &b, cap), Some((20.0 + 230.0) / 3.0));
        assert_eq!(profile_distance(&[None], &[None], cap), None);
    }
}
