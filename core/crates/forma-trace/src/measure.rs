//! Style measures of a drawn glyph: stroke thickness, terminal radius and
//! the narrowest join, from the distance transform of its raster and the
//! skeleton of local maxima.
//!
//! The glyph is rasterized (non-zero fill) at 4 pixels per unit for a
//! 1000-unit em (proportionally for other ems), reduced so that the longer
//! side stays within 2048 pixels. The exact Euclidean distance transform of
//! Felzenszwalb and Huttenlocher ("Distance Transforms of Sampled
//! Functions", 2012; written from the paper) gives every ink pixel its
//! distance to the background. The skeleton is the ridge of that distance:
//! the pixels no neighbour of which is farther from the background by close
//! to their separation (the centres of maximal inscribed discs). It is
//! thinned to one pixel (Zhang–Suen) and short spurs (branches to blunt
//! corners, raster noise) are pruned. Then
//!
//! * the stroke thickness is twice the distance along the skeleton;
//! * the terminal radius is the largest distance at an end of the skeleton
//!   (a pixel with one neighbour run): the ball of a ball terminal, half the
//!   stroke for a round or square end (the branches to the corners of a
//!   square end are pruned, and ends thinner than 0.6 of the typical half
//!   width, spurs into a corner, do not count);
//! * the narrowest join is the smallest distance at a branch point (three or
//!   more neighbour runs).

use forma_geometry::flatten_layer;
use forma_model::{Font, GlyphId, MasterId, Transform};
use forma_render::coverage_mask;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::TraceError;

/// Pixels per font unit for a 1000-unit em.
const PIXELS_PER_UNIT_1000: f64 = 4.0;
/// The longest side of the raster.
const MAX_SIDE: f64 = 2048.0;
/// Background pixels around the glyph.
const MARGIN: f64 = 2.0;
/// A pixel is on the ridge unless a neighbour is farther from the
/// background by at least this fraction of their separation (1 on a plain
/// slope, at most 1/√2 along the bisector of a corner).
const RIDGE_RATIO: f64 = 0.8;
/// Pixels this close to the background (in pixels, centre to centre) are
/// never on the ridge: at the edge the raster's steps make false ridges.
const MIN_RIDGE_DISTANCE: f64 = 3.0;
/// A spur shorter than this many times the distance at its branch point is
/// pruned: it leads to a corner of 70° or blunter, or is raster noise.
const SPUR_RATIO: f64 = 1.75;
/// The most rounds of pruning.
const PRUNE_ROUNDS: usize = 8;
/// Stands for "infinitely far" in the distance transform; far above any
/// squared distance in a 2048-pixel raster.
const FAR: f64 = 1e20;

/// Stroke style measures of a glyph, in font units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StyleMeasure {
    /// Median stroke thickness along the skeleton.
    pub stroke_median: f64,
    /// 10th percentile of the stroke thickness.
    pub stroke_p10: f64,
    /// 90th percentile of the stroke thickness.
    pub stroke_p90: f64,
    /// The radius of the largest disc that fits at an end of a stroke (a
    /// ball terminal gives its ball's radius); `None` without ends.
    pub terminal_radius: Option<f64>,
    /// The radius of the smallest disc at a join of strokes; `None` without
    /// joins.
    pub join_min: Option<f64>,
    /// The median of the radii at the ends of the strokes (where the
    /// largest is a ball, or an end near a join); `None` without ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_radius_median: Option<f64>,
    /// How square the ends of the strokes are: the share of ink in the
    /// rectangle ahead of each end of the skeleton (as long as the stroke's
    /// half width, as wide as the stroke), median over the ends. About 1
    /// for flat ends, about 0.79 (π/4) for round ends and balls; `None`
    /// without measurable ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_fill: Option<f64>,
}

/// The squared distance transform of one line: `f` holds the costs (0 on
/// the background, [`FAR`] on ink); writes into `d`. Felzenszwalb and
/// Huttenlocher's lower envelope of parabolas.
fn edt_line(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    let sep = |q: usize, p: usize| {
        let (qf, pf) = (q as f64, p as f64);
        ((f[q] + qf * qf) - (f[p] + pf * pf)) / (2.0 * qf - 2.0 * pf)
    };
    for q in 1..n {
        let mut s = sep(q, v[k]);
        // z[0] is -inf, so the search always stops at k = 0.
        while k > 0 && s <= z[k] {
            k -= 1;
            s = sep(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate().take(n) {
        let qf = q as f64;
        while z[k + 1] < qf {
            k += 1;
        }
        let p = v[k] as f64;
        *out = (qf - p) * (qf - p) + f[v[k]];
    }
}

/// The squared Euclidean distance from each pixel to the nearest background
/// pixel (0 on the background).
pub(crate) fn distance_transform(inside: &[bool], width: usize, height: usize) -> Vec<f64> {
    let n = width.max(height);
    let mut f = vec![0.0; n];
    let mut d = vec![0.0; n];
    let mut v = vec![0usize; n];
    let mut z = vec![0.0; n + 1];
    let mut grid: Vec<f64> = inside.iter().map(|&i| if i { FAR } else { 0.0 }).collect();
    for x in 0..width {
        for y in 0..height {
            f[y] = grid[y * width + x];
        }
        edt_line(&f[..height], &mut d[..height], &mut v, &mut z);
        for y in 0..height {
            grid[y * width + x] = d[y];
        }
    }
    for y in 0..height {
        f[..width].copy_from_slice(&grid[y * width..(y + 1) * width]);
        edt_line(&f[..width], &mut d[..width], &mut v, &mut z);
        grid[y * width..(y + 1) * width].copy_from_slice(&d[..width]);
    }
    grid
}

/// The 8 neighbours in clockwise order from north.
const RING: [(i64, i64); 8] = [
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
];

struct Grid {
    width: usize,
    height: usize,
}

impl Grid {
    fn index(&self, x: i64, y: i64) -> Option<usize> {
        (x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height)
            .then(|| y as usize * self.width + x as usize)
    }

    fn get(&self, set: &[bool], x: i64, y: i64) -> bool {
        self.index(x, y).is_some_and(|i| set[i])
    }

    /// The neighbours of `(x, y)` in `set`, clockwise from north.
    fn ring(&self, set: &[bool], x: i64, y: i64) -> [bool; 8] {
        RING.map(|(dx, dy)| self.get(set, x + dx, y + dy))
    }
}

/// How many neighbours are set, and how many runs of set neighbours there
/// are around the pixel.
fn neighbourhood(ring: &[bool; 8]) -> (usize, usize) {
    let count = ring.iter().filter(|&&b| b).count();
    let runs = (0..8).filter(|&i| !ring[i] && ring[(i + 1) % 8]).count();
    (count, runs)
}

/// Zhang–Suen thinning of `set` to one-pixel lines.
fn thin(grid: &Grid, set: &mut [bool]) {
    loop {
        let mut changed = false;
        for pass in 0..2 {
            let mut remove = Vec::new();
            for y in 0..grid.height as i64 {
                for x in 0..grid.width as i64 {
                    if !grid.get(set, x, y) {
                        continue;
                    }
                    let r = grid.ring(set, x, y);
                    let (count, runs) = neighbourhood(&r);
                    if !(2..=6).contains(&count) || runs != 1 {
                        continue;
                    }
                    // r: 0 = N, 2 = E, 4 = S, 6 = W.
                    let ok = if pass == 0 {
                        !(r[0] && r[2] && r[4]) && !(r[2] && r[4] && r[6])
                    } else {
                        !(r[0] && r[2] && r[6]) && !(r[0] && r[4] && r[6])
                    };
                    if ok && let Some(i) = grid.index(x, y) {
                        remove.push(i);
                    }
                }
            }
            changed |= !remove.is_empty();
            for i in remove {
                set[i] = false;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Ends (one neighbour run) and branch points (three or more) of a thinned
/// skeleton.
fn ends_and_branches(grid: &Grid, set: &[bool]) -> (Vec<usize>, Vec<usize>) {
    let (mut ends, mut branches) = (Vec::new(), Vec::new());
    for y in 0..grid.height as i64 {
        for x in 0..grid.width as i64 {
            let Some(i) = grid.index(x, y).filter(|&i| set[i]) else {
                continue;
            };
            let (count, runs) = neighbourhood(&grid.ring(set, x, y));
            if count >= 1 && runs == 1 && count <= 2 {
                ends.push(i);
            } else if runs >= 3 {
                branches.push(i);
            }
        }
    }
    (ends, branches)
}

/// The share of ink in the rectangle ahead of the skeleton end `end`: as
/// long as the radius there and twice as wide, across the direction the
/// skeleton arrives from (walked back about one radius). `None` when the
/// end is too small or its direction unclear.
fn end_fill(
    grid: &Grid,
    skeleton: &[bool],
    inside: &[bool],
    root: &[f64],
    end: usize,
) -> Option<f64> {
    let r = root[end];
    if r < 2.0 {
        return None;
    }
    let at = |i: usize| ((i % grid.width) as f64, (i / grid.width) as f64);
    let (px, py) = at(end);
    // Walk back along the skeleton, breadth first, about one radius.
    let mut seen = std::collections::BTreeSet::from([end]);
    let mut frontier = vec![end];
    for _ in 0..(r.ceil() as usize) {
        let mut next = Vec::new();
        for &i in &frontier {
            let (x, y) = ((i % grid.width) as i64, (i / grid.width) as i64);
            for &(dx, dy) in &RING {
                if let Some(j) = grid.index(x + dx, y + dy).filter(|&j| skeleton[j])
                    && seen.insert(j)
                {
                    next.push(j);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        if next.len() > 2 {
            // A branch close to the end: no single direction.
            return None;
        }
        frontier = next;
    }
    // Where the walk got to: the mean of its last step (one pixel or two
    // side by side), so the direction does not depend on the order the
    // neighbours are visited in.
    let (qx, qy) = {
        let (sx, sy) = frontier
            .iter()
            .map(|&i| at(i))
            .fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
        (sx / frontier.len() as f64, sy / frontier.len() as f64)
    };
    let (dx, dy) = (px - qx, py - qy);
    let len = dx.hypot(dy);
    if len < 1.5 {
        return None;
    }
    let reach = (r * 1.5).ceil() as i64;
    let (cx, cy) = (px as i64, py as i64);
    // The walk gives the direction only roughly (on a curve, a chord): the
    // fill is the best over a fan of directions around it. A round end
    // fills the same in every direction; a flat one only when square on.
    let base = dy.atan2(dx);
    let mut best: Option<f64> = None;
    for step in -END_FAN_STEPS..=END_FAN_STEPS {
        let angle = base + f64::from(step) * END_FAN_STEP;
        let (ux, uy) = (angle.cos(), angle.sin());
        let (mut ink, mut all) = (0usize, 0usize);
        for y in cy - reach..=cy + reach {
            for x in cx - reach..=cx + reach {
                let (vx, vy) = (x as f64 - px, y as f64 - py);
                let along = vx * ux + vy * uy;
                let across = (vx * uy - vy * ux).abs();
                if along <= 0.0 || along > r || across > r {
                    continue;
                }
                all += 1;
                if grid.get(inside, x, y) {
                    ink += 1;
                }
            }
        }
        if all >= 8 {
            let fill = ink as f64 / all as f64;
            best = Some(best.map_or(fill, |b: f64| b.max(fill)));
        }
    }
    best
}

/// Ends thinner than this share of the typical half width are not
/// terminals.
const TERMINAL_MIN_RATIO: f64 = 0.6;

/// The directions tried around an end's: this many steps of
/// [`END_FAN_STEP`] radians to each side (±24°).
const END_FAN_STEPS: i32 = 4;
const END_FAN_STEP: f64 = std::f64::consts::PI / 30.0;

/// Removes the spurs: branches from an end to a branch point shorter than
/// [`SPUR_RATIO`] times the distance at the branch point.
fn prune(grid: &Grid, set: &mut [bool], dist: &[f64]) -> bool {
    let (ends, _) = ends_and_branches(grid, set);
    let mut remove: Vec<usize> = Vec::new();
    for end in ends {
        let mut walk = vec![end];
        let mut visited = std::collections::BTreeSet::from([end]);
        let mut length = 0.0;
        let mut current = end;
        let branch = loop {
            let (x, y) = ((current % grid.width) as i64, (current / grid.width) as i64);
            let ring = grid.ring(set, x, y);
            if current != end && neighbourhood(&ring).1 >= 3 {
                break Some(current);
            }
            // A branch point next to the walk ends it; otherwise straight
            // steps go first, so the walk does not cut a corner past one.
            let mut next: Option<(usize, f64, bool)> = None;
            for &(dx, dy) in &RING {
                let Some(j) = grid
                    .index(x + dx, y + dy)
                    .filter(|&j| set[j] && !visited.contains(&j))
                else {
                    continue;
                };
                let (jx, jy) = (x + dx, y + dy);
                let is_branch = neighbourhood(&grid.ring(set, jx, jy)).1 >= 3;
                let diagonal = dx != 0 && dy != 0;
                let step = if diagonal { 2f64.sqrt() } else { 1.0 };
                let better = match next {
                    None => true,
                    Some((_, s, b)) => (is_branch && !b) || (is_branch == b && step < s),
                };
                if better {
                    next = Some((j, step, is_branch));
                }
            }
            let Some((j, step, _)) = next else {
                break None;
            };
            visited.insert(j);
            length += step;
            walk.push(j);
            current = j;
            if walk.len() > set.len() {
                break None;
            }
        };
        if let Some(b) = branch
            && length < SPUR_RATIO * dist[b]
        {
            walk.pop();
            remove.extend(walk);
        }
    }
    let pruned = !remove.is_empty();
    for i in remove {
        set[i] = false;
    }
    pruned
}

/// The `p` quantile (0 to 1) of sorted values, by nearest rank.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let k = (p * (sorted.len() - 1) as f64).round() as usize;
    sorted[k.min(sorted.len() - 1)]
}

/// Measures the stroke style of `glyph` in `master`, components included.
///
/// See the module documentation for the method. Distances are measured to
/// the edge of the ink, so a stroke `w` units wide measures `w` give or take
/// a pixel of the raster.
///
/// # Errors
///
/// [`TraceError::Geometry`] if the glyph has no layer in `master`, a
/// component cannot be expanded, or the glyph has no outline;
/// [`TraceError::Render`] if it cannot be rasterized.
pub fn measure(font: &Font, glyph: GlyphId, master: MasterId) -> Result<StyleMeasure, TraceError> {
    let paths =
        flatten_layer(font, glyph, master).map_err(|e| TraceError::Geometry(e.to_string()))?;
    let bounds = forma_geometry::bbox(&paths)
        .filter(|r| r.width() > 0.0 && r.height() > 0.0)
        .ok_or_else(|| TraceError::Geometry("the glyph has no outline".into()))?;
    let upm = f64::from(font.units_per_em.max(1));
    let longest = bounds.width().max(bounds.height());
    let ppu = (PIXELS_PER_UNIT_1000 * 1000.0 / upm).min((MAX_SIDE - 2.0 * MARGIN) / longest);
    let width = (bounds.width() * ppu).ceil() + 2.0 * MARGIN;
    let height = (bounds.height() * ppu).ceil() + 2.0 * MARGIN;
    let (width, height) = (width as u32, height as u32);
    let to_pixels = Transform {
        xx: ppu,
        xy: 0.0,
        yx: 0.0,
        yy: -ppu,
        dx: MARGIN - bounds.x0 * ppu,
        dy: MARGIN + bounds.y1 * ppu,
    };
    let coverage = coverage_mask(&paths, &to_pixels, width, height)
        .map_err(|e| TraceError::Render(e.to_string()))?;
    let grid = Grid {
        width: width as usize,
        height: height as usize,
    };
    let inside: Vec<bool> = coverage.data.iter().map(|&c| c >= 128).collect();
    let squared = distance_transform(&inside, grid.width, grid.height);
    let root: Vec<f64> = squared.iter().map(|d| d.sqrt()).collect();
    // Distance to the edge of the ink: half a pixel short of the nearest
    // background pixel's centre.
    let dist: Vec<f64> = root.iter().map(|d| (d - 0.5).max(0.0)).collect();

    let mut skeleton = vec![false; inside.len()];
    for y in 0..grid.height as i64 {
        for x in 0..grid.width as i64 {
            let Some(i) = grid.index(x, y).filter(|&i| root[i] >= MIN_RIDGE_DISTANCE) else {
                continue;
            };
            let ridge = RING.iter().all(|&(dx, dy)| {
                let step = if dx != 0 && dy != 0 { 2f64.sqrt() } else { 1.0 };
                let there = grid.index(x + dx, y + dy).map_or(0.0, |j| root[j]);
                there - root[i] < RIDGE_RATIO * step
            });
            skeleton[i] = ridge;
        }
    }
    // Close the one- and two-pixel gaps that the raster leaves in the ridge
    // where strokes meet, then thin it to a line.
    let mut closed = skeleton.clone();
    for y in 0..grid.height as i64 {
        for x in 0..grid.width as i64 {
            let Some(i) = grid.index(x, y).filter(|&i| inside[i] && !skeleton[i]) else {
                continue;
            };
            closed[i] = RING
                .iter()
                .any(|&(dx, dy)| grid.get(&skeleton, x + dx, y + dy));
        }
    }
    let mut skeleton = closed;
    thin(&grid, &mut skeleton);
    // Pruning can leave a stub at the branch point; a few rounds clear it.
    for _ in 0..PRUNE_ROUNDS {
        if !prune(&grid, &mut skeleton, &dist) {
            break;
        }
        thin(&grid, &mut skeleton);
    }

    let mut thickness: Vec<f64> = skeleton
        .iter()
        .zip(&dist)
        .filter(|(s, _)| **s)
        .map(|(_, d)| 2.0 * d / ppu)
        .collect();
    if thickness.is_empty() {
        // Strokes a few pixels thin leave no ridge away from the edge: fall
        // back on the thickest point.
        let deepest = dist.iter().copied().fold(0.0, f64::max);
        thickness.push(2.0 * deepest / ppu);
    }
    thickness.sort_by(f64::total_cmp);
    let (ends, branches) = ends_and_branches(&grid, &skeleton);
    // An end much thinner than the stroke is a spur into the corner of a
    // cut end, not a terminal: it says nothing of the end's size or shape.
    let half_px = percentile(&thickness, 0.5) * ppu / 2.0;
    let terminals: Vec<usize> = ends
        .iter()
        .copied()
        .filter(|&i| root[i] >= TERMINAL_MIN_RATIO * half_px)
        .collect();
    let terminal_radius = terminals.iter().map(|&i| dist[i] / ppu).reduce(f64::max);
    let terminal_radius_median = {
        let mut radii: Vec<f64> = terminals.iter().map(|&i| dist[i] / ppu).collect();
        radii.sort_by(f64::total_cmp);
        (!radii.is_empty()).then(|| percentile(&radii, 0.5))
    };
    let join_min = branches.iter().map(|&i| dist[i] / ppu).reduce(f64::min);
    let mut fills: Vec<f64> = terminals
        .iter()
        .filter_map(|&i| end_fill(&grid, &skeleton, &inside, &root, i))
        .collect();
    fills.sort_by(f64::total_cmp);
    let terminal_fill = (!fills.is_empty()).then(|| percentile(&fills, 0.5));
    Ok(StyleMeasure {
        stroke_median: percentile(&thickness, 0.5),
        stroke_p10: percentile(&thickness, 0.1),
        stroke_p90: percentile(&thickness, 0.9),
        terminal_radius,
        terminal_radius_median,
        join_min,
        terminal_fill,
    })
}
