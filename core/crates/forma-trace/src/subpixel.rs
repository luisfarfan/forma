//! Sub-pixel boundaries: the 50 % isoline of a candidate's own ink
//! coverage, by marching squares.
//!
//! The coverage of each pixel (see [`crate::mask`]) is a sample at the
//! pixel's centre; between four samples it is read bilinearly, so the
//! isoline crosses each cell edge where the coverage, interpolated along
//! that edge, meets the level. That is the same curve the upsampled mask
//! approximates with steps, without the steps: no staircase to smooth away,
//! and a 1-pixel slit whose coverage dips below the level stays open.
//!
//! Only the candidate's own ink counts: pixels of other candidates, and
//! specks, read as background (see [`Field::own`]).

use kurbo::Point;

use crate::mask::Coverage;
use crate::vectorize::CandidateInk;

/// Pixels of margin around the candidate's box in the field.
const MARGIN: i64 = 2;

/// A window of coverage values, one per original pixel, row-major.
pub(crate) struct Field {
    /// The original pixel of `values[0]`.
    pub x0: i64,
    pub y0: i64,
    pub width: usize,
    pub height: usize,
    pub values: Vec<f32>,
}

impl Field {
    /// The coverage that belongs to the candidate: the image's coverage at
    /// pixels within one pixel of the candidate's own ink (its anti-aliased
    /// fringe included), unless the pixel holds ink of another piece; zero
    /// elsewhere. A margin of [`MARGIN`] pixels of zeros surrounds the box,
    /// so every isoline closes.
    pub(crate) fn own(
        coverage: &Coverage,
        mask_ink: &dyn Fn(i64, i64) -> bool,
        ink: &CandidateInk,
    ) -> Field {
        let b = ink.bbox;
        let (x0, y0) = (i64::from(b.x0) - MARGIN, i64::from(b.y0) - MARGIN);
        let (x1, y1) = (i64::from(b.x1) + MARGIN, i64::from(b.y1) + MARGIN);
        let (width, height) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let u = i64::from(ink.upsample);
        // Per original pixel: does it hold own ink, ink of another piece?
        let mut own = vec![false; width * height];
        let mut foreign = vec![false; width * height];
        for j in 0..height {
            for i in 0..width {
                let (ox, oy) = (x0 + i as i64, y0 + j as i64);
                for my in oy * u..(oy + 1) * u {
                    for mx in ox * u..(ox + 1) * u {
                        if ink.at(mx, my) {
                            own[j * width + i] = true;
                        } else if mask_ink(mx, my) {
                            foreign[j * width + i] = true;
                        }
                    }
                }
            }
        }
        let near_own = |i: usize, j: usize| -> bool {
            let (lo_i, hi_i) = (i.saturating_sub(1), (i + 1).min(width - 1));
            let (lo_j, hi_j) = (j.saturating_sub(1), (j + 1).min(height - 1));
            (lo_j..=hi_j).any(|y| (lo_i..=hi_i).any(|x| own[y * width + x]))
        };
        let mut values = vec![0f32; width * height];
        for j in 0..height {
            for i in 0..width {
                let k = j * width + i;
                let edge = i == 0 || j == 0 || i + 1 == width || j + 1 == height;
                if !edge && !foreign[k] && near_own(i, j) {
                    values[k] = coverage.at(x0 + i as i64, y0 + j as i64);
                }
            }
        }
        Field {
            x0,
            y0,
            width,
            height,
            values,
        }
    }

    fn at(&self, i: usize, j: usize) -> f32 {
        self.values[j * self.width + i]
    }
}

/// An edge of the sample grid: horizontal (between samples `(i, j)` and
/// `(i + 1, j)`) or vertical (between `(i, j)` and `(i, j + 1)`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
struct Edge {
    vertical: bool,
    i: usize,
    j: usize,
}

/// The closed isolines of `field` at `level`, in original pixel
/// coordinates (a pixel `(x, y)` spans `x..x+1`, its sample sits at
/// `x + 0.5`), each as a polygon whose last point equals its first.
/// Outer boundaries run one way and counters the other (see
/// [`orient_by_nesting`]). Saddle cells are resolved by the mean of their
/// four samples.
pub(crate) fn isolines(field: &Field, level: f32) -> Vec<Vec<Point>> {
    let (w, h) = (field.width, field.height);
    if w < 2 || h < 2 {
        return Vec::new();
    }
    let inside = |i: usize, j: usize| field.at(i, j) >= level;
    // The crossing point on an edge.
    let point = |e: Edge| -> Point {
        let (a, b) = if e.vertical {
            (field.at(e.i, e.j), field.at(e.i, e.j + 1))
        } else {
            (field.at(e.i, e.j), field.at(e.i + 1, e.j))
        };
        let t = if (b - a).abs() > f32::EPSILON {
            f64::from(((level - a) / (b - a)).clamp(0.0, 1.0))
        } else {
            0.5
        };
        let (x, y) = (
            field.x0 as f64 + e.i as f64 + 0.5,
            field.y0 as f64 + e.j as f64 + 0.5,
        );
        if e.vertical {
            Point::new(x, y + t)
        } else {
            Point::new(x + t, y)
        }
    };
    // Each cell joins the edges its isoline crosses, in pairs; every
    // crossing is shared by the two cells on either side of its edge, so
    // the pairs chain into closed loops.
    let mut links: std::collections::HashMap<Edge, Vec<Edge>> = std::collections::HashMap::new();
    for j in 0..h - 1 {
        for i in 0..w - 1 {
            // Corners: bit 0 top-left, 1 top-right, 2 bottom-right, 3
            // bottom-left (y down).
            let code = usize::from(inside(i, j))
                | usize::from(inside(i + 1, j)) << 1
                | usize::from(inside(i + 1, j + 1)) << 2
                | usize::from(inside(i, j + 1)) << 3;
            let top = Edge {
                vertical: false,
                i,
                j,
            };
            let right = Edge {
                vertical: true,
                i: i + 1,
                j,
            };
            let bottom = Edge {
                vertical: false,
                i,
                j: j + 1,
            };
            let left = Edge {
                vertical: true,
                i,
                j,
            };
            let mean =
                (field.at(i, j) + field.at(i + 1, j) + field.at(i + 1, j + 1) + field.at(i, j + 1))
                    / 4.0;
            let centre_in = mean >= level;
            let pairs: &[(Edge, Edge)] = match code {
                1 | 14 => &[(top, left)],
                2 | 13 => &[(top, right)],
                3 | 12 => &[(left, right)],
                4 | 11 => &[(right, bottom)],
                6 | 9 => &[(top, bottom)],
                7 | 8 => &[(left, bottom)],
                // Saddles: with ink at the centre the two ink corners join
                // and the two background corners are cut off; otherwise the
                // ink corners are cut off.
                5 if centre_in => &[(top, right), (left, bottom)],
                5 => &[(top, left), (right, bottom)],
                10 if centre_in => &[(top, left), (right, bottom)],
                10 => &[(top, right), (left, bottom)],
                _ => &[],
            };
            for &(a, b) in pairs {
                links.entry(a).or_default().push(b);
                links.entry(b).or_default().push(a);
            }
        }
    }
    // Stitch the links into loops, in a deterministic order.
    let mut starts: Vec<Edge> = links.keys().copied().collect();
    starts.sort();
    let mut seen = std::collections::HashSet::new();
    let mut loops = Vec::new();
    for start in starts {
        if seen.contains(&start) {
            continue;
        }
        let mut ring = vec![point(start)];
        seen.insert(start);
        let (mut prev, mut here) = (start, start);
        while let Some(next) = links
            .get(&here)
            .and_then(|n| n.iter().copied().find(|&e| e != prev || n.len() == 1))
        {
            if next == start || !seen.insert(next) {
                break;
            }
            ring.push(point(next));
            prev = here;
            here = next;
        }
        if ring.len() >= 3 {
            let first = ring[0];
            ring.push(first);
            loops.push(ring);
        }
    }
    orient_by_nesting(&mut loops);
    loops
}

/// Whether `p` is inside the closed polygon `ring` (even-odd).
fn contains(ring: &[Point], p: Point) -> bool {
    let mut inside = false;
    for pair in ring.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Twice the signed area of a closed polygon (shoelace).
fn area2(ring: &[Point]) -> f64 {
    ring.windows(2)
        .map(|pair| pair[0].x * pair[1].y - pair[1].x * pair[0].y)
        .sum()
}

/// Orients the loops by how deeply they nest: outer boundaries (inside an
/// even number of other loops) one way, counters (odd) the other, so that
/// a non-zero fill gives the ink. Isolines never cross, so nesting is told
/// by any one point of a loop.
fn orient_by_nesting(loops: &mut [Vec<Point>]) {
    let depths: Vec<usize> = (0..loops.len())
        .map(|i| {
            let probe = loops[i][0];
            (0..loops.len())
                .filter(|&j| j != i && contains(&loops[j], probe))
                .count()
        })
        .collect();
    for (ring, depth) in loops.iter_mut().zip(depths) {
        let positive = area2(ring) > 0.0;
        if positive != (depth % 2 == 0) {
            ring.reverse();
        }
    }
}

/// How soft the edges of the field are: of the edge pixels at a *step*
/// (with neighbours across the level both sideways and up or down, which
/// only slanted and curved edges have), the fraction whose coverage is in
/// between (0.15 to 0.85). Anti-aliased drawings give most of them;
/// hard-edged pixel art and thresholded scans give almost none, and then
/// the steps of the pixels are noise, half a pixel high. Edges along the
/// pixel grid are exact either way and do not count; a field with no steps
/// reads as soft.
pub(crate) fn edge_softness(field: &Field, level: f32) -> f64 {
    let (w, h) = (field.width, field.height);
    let (mut steps, mut soft) = (0usize, 0usize);
    let across =
        |x: usize, y: usize, side: bool| x < w && y < h && (field.at(x, y) >= level) != side;
    for j in 0..h {
        for i in 0..w {
            let v = field.at(i, j);
            let side = v >= level;
            let sideways = across(i.wrapping_sub(1), j, side) || across(i + 1, j, side);
            let vertical = across(i, j.wrapping_sub(1), side) || across(i, j + 1, side);
            if sideways && vertical {
                steps += 1;
                if (0.15..=0.85).contains(&v) {
                    soft += 1;
                }
            }
        }
    }
    if steps == 0 {
        1.0
    } else {
        soft as f64 / steps as f64
    }
}
