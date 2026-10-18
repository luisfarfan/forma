//! Matching the candidates of an image to a known layout: how many glyphs
//! each row holds.
//!
//! When a row has as many candidates as glyphs, they match one to one in
//! reading order. When it has more, some glyphs were drawn in several
//! pieces (an H whose bar does not touch a stem, an i with its dot apart):
//! the pieces are grouped, only with neighbours of the same row, only when
//! they are close and overlap vertically, choosing the partition that
//! costs least (gaps relative to the stroke, widths unlike the row's).
//! When two partitions cost about the same, the row is ambiguous and left
//! to a person; nothing is ever merged across rows, and letters that touch
//! (fewer candidates than glyphs) are reported, never split.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use image::RgbImage;

use crate::TraceError;
use crate::candidates::{Analysis, TraceOptions, analyze_parts};
use crate::measure::distance_transform;

/// Pieces merge only when the gap between them is at most this many
/// strokes (or [`MAX_GAP_MIN_PX`] pixels if that is more)...
const MAX_GAP_STROKES: f64 = 0.8;
const MAX_GAP_MIN_PX: f64 = 1.5;
/// ...and the gap inside a group is less than this fraction of the
/// smallest gap between the groups of the row (a letter's pieces sit
/// closer together than the letters do).
const GAP_CONTRAST: f64 = 0.5;
/// ...and they overlap vertically by at least this fraction of the shorter.
const MIN_VERTICAL_OVERLAP: f64 = 0.5;
/// A group wider than this many times the row's median width pays
/// [`WIDE_PENALTY`].
const WIDE_FACTOR: f64 = 1.6;
const WIDE_PENALTY: f64 = 10.0;
/// A row is ambiguous when its second-best partition costs less than this
/// many times the best. Conservative: merging the wrong pieces would join
/// two letters, and the gates cannot see it (each glyph matches its own,
/// wrongly grouped, ink).
const AMBIGUITY_RATIO: f64 = 2.0;
/// Costs below this are equal (a partition of single pieces costs 0).
const COST_EPSILON: f64 = 1e-9;

/// How a row's candidates match its glyphs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RowMatch {
    /// One candidate per glyph, in reading order.
    Exact {
        /// The candidates of each glyph, left to right.
        groups: Vec<Vec<usize>>,
    },
    /// More candidates than glyphs, grouped without doubt.
    Grouped {
        /// The candidates of each glyph, left to right.
        groups: Vec<Vec<usize>>,
    },
    /// More candidates than glyphs, and two groupings about as likely.
    Ambiguous {
        /// The likelier grouping.
        best: Vec<Vec<usize>>,
        /// The other one.
        second: Vec<Vec<usize>>,
        /// The cost of the second over the best.
        ratio: f64,
    },
    /// The candidates cannot be the row's glyphs: fewer of them (letters
    /// that touch, or a glyph missing), or more that cannot be grouped.
    Mismatch {
        /// Glyphs expected.
        expected: usize,
        /// Candidates found.
        found: usize,
        /// Why, in words.
        reason: String,
    },
}

/// One piece of a row, as the grouping sees it.
#[derive(Clone, Copy, Debug)]
struct Piece {
    index: usize,
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
}

/// The cost of making `pieces[from..to]` one glyph, or `None` if they may
/// not be merged.
fn group_cost(pieces: &[Piece], from: usize, to: usize, stroke: f64, median_w: f64) -> Option<f64> {
    let group = &pieces[from..to];
    let mut cost = 0.0;
    let mut right = group[0].x1;
    for pair in group.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let gap = (b.x0 - right).max(0.0);
        let shorter = (a.y1 - a.y0).min(b.y1 - b.y0).max(1.0);
        let overlap = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0) / shorter;
        if gap > (MAX_GAP_STROKES * stroke).max(MAX_GAP_MIN_PX) || overlap < MIN_VERTICAL_OVERLAP {
            return None;
        }
        cost += gap / stroke.max(1.0);
        right = right.max(b.x1);
    }
    if group.len() > 1 {
        let width = right - group[0].x0;
        cost += (width - median_w).abs() / median_w.max(1.0);
        if width > WIDE_FACTOR * median_w {
            cost += WIDE_PENALTY;
        }
    }
    Some(cost)
}

/// A partition: its cost and the positions where its groups end.
type Partition = (f64, Vec<usize>);

/// The two cheapest partitions of `pieces` into exactly `k` contiguous
/// groups (dynamic programming keeping the two best per state).
fn best_two(pieces: &[Piece], k: usize, stroke: f64, median_w: f64) -> Vec<Partition> {
    let n = pieces.len();
    // best[g][i]: up to two partitions of the first i pieces in g groups.
    let mut best: Vec<Vec<Vec<Partition>>> = vec![vec![Vec::new(); n + 1]; k + 1];
    best[0][0].push((0.0, Vec::new()));
    for g in 1..=k {
        for i in g..=n {
            let mut found: Vec<Partition> = Vec::new();
            for (j, before) in best[g - 1].iter().enumerate().take(i).skip(g - 1) {
                if before.is_empty() {
                    continue;
                }
                let Some(c) = group_cost(pieces, j, i, stroke, median_w) else {
                    continue;
                };
                for (prev, cuts) in before {
                    let mut cuts = cuts.clone();
                    cuts.push(i);
                    found.push((prev + c, cuts));
                }
            }
            found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            found.truncate(2);
            best[g][i] = found;
        }
    }
    std::mem::take(&mut best[k][n])
}

/// The gaps inside the groups of a partition and between them: whether
/// every gap inside is less than [`GAP_CONTRAST`] times the smallest gap
/// between groups (true when nothing is merged or there is one group).
fn contrasted(pieces: &[Piece], cuts: &[usize]) -> bool {
    let mut inside: f64 = 0.0;
    let mut between = f64::INFINITY;
    let mut from = 0;
    let mut previous_right: Option<f64> = None;
    for &to in cuts {
        let group = &pieces[from..to];
        let mut right = group[0].x1;
        for p in &group[1..] {
            inside = inside.max((p.x0 - right).max(0.0));
            right = right.max(p.x1);
        }
        if let Some(prev) = previous_right {
            between = between.min((group[0].x0 - prev).max(0.0));
        }
        previous_right = Some(right);
        from = to;
    }
    between.is_infinite() || inside < GAP_CONTRAST * between
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// Matches each row of `analysis` with the number of glyphs it should hold
/// (`expected[row]`), given the typical stroke width in pixels.
///
/// Rows beyond `expected`, or missing from it, are `Mismatch`.
#[must_use]
pub fn match_rows(analysis: &Analysis, expected: &[usize], stroke_px: f64) -> Vec<RowMatch> {
    let mut widths: Vec<f64> = analysis
        .candidates
        .iter()
        .map(|c| f64::from(c.bbox.x1 - c.bbox.x0))
        .collect();
    let overall_w = median(&mut widths);
    let rows = analysis.rows.len().max(expected.len());
    (0..rows)
        .map(|row| {
            let want = expected.get(row).copied().unwrap_or(0);
            let mut pieces: Vec<Piece> = analysis
                .candidates
                .iter()
                .filter(|c| c.row == row)
                .map(|c| Piece {
                    index: c.index,
                    x0: f64::from(c.bbox.x0),
                    x1: f64::from(c.bbox.x1),
                    y0: f64::from(c.bbox.y0),
                    y1: f64::from(c.bbox.y1),
                })
                .collect();
            pieces.sort_by(|a, b| a.x0.total_cmp(&b.x0).then(a.index.cmp(&b.index)));
            let found = pieces.len();
            if want == 0 || found == 0 || found < want {
                return RowMatch::Mismatch {
                    expected: want,
                    found,
                    reason: if found < want {
                        "fewer shapes than glyphs: letters that touch, or a glyph missing"
                            .to_owned()
                    } else {
                        "no glyphs expected in this row, or no shapes found".to_owned()
                    },
                };
            }
            if found == want {
                return RowMatch::Exact {
                    groups: pieces.iter().map(|p| vec![p.index]).collect(),
                };
            }
            let mut row_widths: Vec<f64> = pieces.iter().map(|p| p.x1 - p.x0).collect();
            let row_w = median(&mut row_widths).max(overall_w);
            let options = best_two(&pieces, want, stroke_px, row_w);
            let to_groups = |cuts: &[usize]| -> Vec<Vec<usize>> {
                let mut from = 0;
                cuts.iter()
                    .map(|&to| {
                        let g = pieces[from..to].iter().map(|p| p.index).collect();
                        from = to;
                        g
                    })
                    .collect()
            };
            match options.as_slice() {
                [] => RowMatch::Mismatch {
                    expected: want,
                    found,
                    reason: "more shapes than glyphs, and they cannot be grouped (pieces too far \
                             apart or not side by side)"
                        .to_owned(),
                },
                [only] if contrasted(&pieces, &only.1) => RowMatch::Grouped {
                    groups: to_groups(&only.1),
                },
                [only] => RowMatch::Ambiguous {
                    best: to_groups(&only.1),
                    second: Vec::new(),
                    ratio: 1.0,
                },
                [first, second, ..] => {
                    let ratio = if first.0 <= COST_EPSILON {
                        if second.0 <= COST_EPSILON {
                            1.0
                        } else {
                            f64::INFINITY
                        }
                    } else {
                        second.0 / first.0
                    };
                    if ratio < AMBIGUITY_RATIO || !contrasted(&pieces, &first.1) {
                        RowMatch::Ambiguous {
                            best: to_groups(&first.1),
                            second: to_groups(&second.1),
                            ratio,
                        }
                    } else {
                        RowMatch::Grouped {
                            groups: to_groups(&first.1),
                        }
                    }
                }
            }
        })
        .collect()
}

/// The typical stroke width of the ink of `image`, in pixels: twice the
/// median distance to the background along the ridge of the ink (the
/// pixels at least as far from the background as their four neighbours:
/// the middle of each stroke), at the image's own resolution. 0 without
/// ink.
///
/// # Errors
///
/// The errors of [`crate::analyze`].
pub fn typical_stroke_px(image: &RgbImage, opts: &TraceOptions) -> Result<f64, TraceError> {
    let parts = analyze_parts(image, opts)?;
    let cov = &parts.coverage;
    let (w, h) = (cov.width as usize, cov.height as usize);
    let inside: Vec<bool> = cov.data.iter().map(|&v| v >= 0.5).collect();
    if !inside.iter().any(|&v| v) {
        return Ok(0.0);
    }
    let edt = distance_transform(&inside, w, h);
    let at = |x: usize, y: usize| edt[y * w + x];
    let mut ridge = Vec::new();
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let d = at(x, y);
            if d > 0.0
                && d >= at(x - 1, y)
                && d >= at(x + 1, y)
                && d >= at(x, y - 1)
                && d >= at(x, y + 1)
            {
                ridge.push(d.sqrt());
            }
        }
    }
    // A pixel centre in the middle of a stroke of width w is w/2 from the
    // background's pixel centres, give or take half a pixel.
    Ok((2.0 * median(&mut ridge) - 1.0).max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidates::{Candidate, PxRect, Row};

    fn analysis(boxes: &[(u32, u32, u32, u32)]) -> Analysis {
        Analysis {
            width: 1000,
            height: 200,
            background: [255; 3],
            ink: [0; 3],
            palette: Vec::new(),
            rows: vec![Row {
                index: 0,
                top: 10.0,
                cap_px: 10.0,
                baseline_px: 110.0,
                bottom: 110.0,
            }],
            candidates: boxes
                .iter()
                .enumerate()
                .map(|(index, &(x0, y0, x1, y1))| Candidate {
                    index,
                    row: 0,
                    bbox: PxRect { x0, y0, x1, y1 },
                    area: 100,
                })
                .collect(),
            note: None,
        }
    }

    #[test]
    fn exact_rows_match_in_reading_order() {
        let a = analysis(&[(10, 10, 60, 110), (80, 10, 130, 110)]);
        assert_eq!(
            match_rows(&a, &[2], 12.0),
            vec![RowMatch::Exact {
                groups: vec![vec![0], vec![1]]
            }]
        );
    }

    #[test]
    fn a_letter_in_two_close_pieces_is_grouped() {
        // A, then an H drawn as a left stem with bar and a right stem
        // 1 px apart, then I.
        let a = analysis(&[
            (10, 10, 70, 110),
            (100, 10, 140, 110),
            (141, 10, 160, 110),
            (200, 10, 216, 110),
        ]);
        assert_eq!(
            match_rows(&a, &[3], 12.0),
            vec![RowMatch::Grouped {
                groups: vec![vec![0], vec![1, 2], vec![3]]
            }]
        );
    }

    #[test]
    fn a_stray_piece_far_from_letters_is_not_merged() {
        let a = analysis(&[(10, 10, 70, 110), (100, 50, 104, 54), (200, 10, 260, 110)]);
        let out = match_rows(&a, &[2], 12.0);
        assert!(matches!(out[0], RowMatch::Mismatch { .. }), "{out:?}");
    }

    #[test]
    fn a_dot_above_its_stem_is_left_to_the_analysis_and_never_merged_sideways() {
        // Pieces side by side without vertical overlap do not merge.
        let a = analysis(&[(10, 10, 30, 30), (34, 60, 54, 110), (100, 10, 150, 110)]);
        let out = match_rows(&a, &[2], 12.0);
        assert!(matches!(out[0], RowMatch::Mismatch { .. }), "{out:?}");
    }

    #[test]
    fn two_equally_good_groupings_are_ambiguous() {
        // Three pieces evenly spaced 1 px apart, two glyphs: (0,1)(2) or
        // (0)(1,2).
        let a = analysis(&[(10, 10, 40, 110), (41, 10, 71, 110), (72, 10, 102, 110)]);
        let out = match_rows(&a, &[2], 12.0);
        assert!(matches!(out[0], RowMatch::Ambiguous { .. }), "{out:?}");
    }

    #[test]
    fn touching_letters_are_reported_not_split() {
        let a = analysis(&[(10, 10, 120, 110)]);
        let out = match_rows(&a, &[2], 12.0);
        assert!(
            matches!(
                &out[0],
                RowMatch::Mismatch {
                    expected: 2,
                    found: 1,
                    ..
                }
            ),
            "{out:?}"
        );
    }

    #[test]
    fn neighbouring_letters_are_never_joined_on_a_guess() {
        // H I O with a 10 px stroke: the H's bar stops 7 px short of its
        // right stem, the I is 4 px from it. Either merge would be a guess.
        let a = analysis(&[
            (10, 10, 60, 110),
            (67, 10, 77, 110),
            (81, 10, 91, 110),
            (120, 10, 180, 110),
        ]);
        let out = match_rows(&a, &[3], 10.0);
        assert!(
            !matches!(out[0], RowMatch::Grouped { .. } | RowMatch::Exact { .. }),
            "{out:?}"
        );
    }

    #[test]
    fn rows_never_mix() {
        let mut a = analysis(&[(10, 10, 60, 110), (64, 10, 80, 110)]);
        a.rows.push(Row {
            index: 1,
            top: 150.0,
            cap_px: 150.0,
            baseline_px: 190.0,
            bottom: 190.0,
        });
        a.candidates[1].row = 1;
        let out = match_rows(&a, &[1, 1], 12.0);
        assert!(matches!(out[0], RowMatch::Exact { .. }));
        assert!(matches!(out[1], RowMatch::Exact { .. }));
    }
}
