//! Glyph profiles: the leftmost and rightmost ink on horizontal lines.

use forma_geometry::{flatten_layer, to_bezpath};
use forma_model::{Font, GlyphId, Layer, MasterId};
use kurbo::{Line, ParamCurve, ParamCurveExtrema, PathSeg, Rect};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SpacingError;

/// The fewest lines a profile may have.
pub const MIN_LINES: usize = 8;
/// The most lines a profile may have.
pub const MAX_LINES: usize = 4096;

/// A glyph with ink on fewer lines than this is measured over its own ink
/// height instead of the zone (a period, quotes).
const MIN_INK_LINES: usize = 8;

/// Overshoot of the measurement zone, as a fraction of the units per em.
const OVERSHOOT_RATIO: f64 = 0.02;

/// Bisection steps when solving a curve piece for a line: 2⁻⁶⁰ of the
/// parameter range is far below a font unit.
const SOLVE_STEPS: usize = 60;

/// A vertical band, in font units, over which profiles are measured.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Zone {
    /// The lower edge.
    pub bottom: f64,
    /// The upper edge.
    pub top: f64,
}

/// The ink extent of a glyph on horizontal lines.
///
/// `ys[i]` is the height of line `i`, bottom to top; `left[i]` and
/// `right[i]` are the leftmost and rightmost x of ink on it in the glyph's
/// own coordinates, or `None` when the line has no ink.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Profile {
    /// The height of each line.
    pub ys: Vec<f64>,
    /// The leftmost ink on each line.
    pub left: Vec<Option<f64>>,
    /// The rightmost ink on each line.
    pub right: Vec<Option<f64>>,
}

impl Profile {
    /// How many lines have ink.
    pub(crate) fn ink_lines(&self) -> usize {
        self.left.iter().filter(|x| x.is_some()).count()
    }
}

/// The measurement zone of `master`: from `−0.02·UPM` to
/// `cap_height + 0.02·UPM`.
///
/// A master that does not exist falls back to the default master, and a
/// font without masters to the cap height of [`Font::new`] (70 % of the
/// UPM).
#[must_use]
pub fn measurement_zone(font: &Font, master: MasterId) -> Zone {
    let upm = f64::from(font.units_per_em);
    let cap_height = font
        .masters
        .iter()
        .find(|m| m.id == master)
        .or_else(|| font.masters.first())
        .map_or(upm * 0.7, |m| m.metrics.cap_height);
    let overshoot = OVERSHOOT_RATIO * upm;
    Zone {
        bottom: -overshoot,
        top: cap_height + overshoot,
    }
}

/// The profile of `glyph` in `master` on `lines` equally spaced lines of
/// `zone`.
///
/// Line `i` sits at the centre of the `i`-th of `lines` equal bands of the
/// zone, so no line falls on its edges. Each line is intersected exactly
/// with the outline (curves included, components expanded), and the ink is
/// where the non-zero winding rule says the outline is filled. If fewer
/// than 8 lines have ink, the profile is measured again over the glyph's
/// own ink height (its bounding box) with the same number of lines.
///
/// # Errors
///
/// * [`SpacingError::InvalidParameter`] if `lines` is not within
///   [`MIN_LINES`]`..=`[`MAX_LINES`], the zone is not finite or not
///   ascending, or the glyph does not exist.
/// * [`SpacingError::MissingLayer`] if the glyph has no layer in `master`.
/// * [`SpacingError::Geometry`] if the outline cannot be expanded.
pub fn profile(
    font: &Font,
    glyph: GlyphId,
    master: MasterId,
    zone: Zone,
    lines: usize,
) -> Result<Profile, SpacingError> {
    check_lines(lines)?;
    if !(zone.bottom.is_finite() && zone.top.is_finite() && zone.bottom < zone.top) {
        return Err(SpacingError::InvalidParameter(format!(
            "the zone {}..{} is not a finite ascending range",
            zone.bottom, zone.top
        )));
    }
    let outline = Outline::of(font, glyph, master)?;
    Ok(outline.glyph_profile(zone, lines))
}

/// Checks a number of profile lines.
pub(crate) fn check_lines(lines: usize) -> Result<(), SpacingError> {
    if (MIN_LINES..=MAX_LINES).contains(&lines) {
        Ok(())
    } else {
        Err(SpacingError::InvalidParameter(format!(
            "lines must be within {MIN_LINES}..={MAX_LINES}, got {lines}"
        )))
    }
}

/// The layer of `glyph` in `master`.
pub(crate) fn layer(font: &Font, glyph: GlyphId, master: MasterId) -> Result<&Layer, SpacingError> {
    let source = font
        .glyphs
        .get(&glyph)
        .ok_or_else(|| SpacingError::InvalidParameter(format!("glyph {glyph} does not exist")))?;
    source.layers.get(&master).ok_or_else(|| {
        SpacingError::MissingLayer(format!(
            "glyph {} has no layer for master {master}",
            source.name
        ))
    })
}

/// A part of an outline segment over which y only grows or only shrinks.
#[derive(Clone, Debug)]
struct Piece {
    segment: PathSeg,
    /// The parameter range of the piece on its segment.
    t0: f64,
    t1: f64,
    /// `y` at `t0` and `t1`.
    y0: f64,
    y1: f64,
    /// +1 when the piece goes up, −1 when it goes down.
    winding: i32,
}

impl Piece {
    /// The x where the piece crosses the line at height `y`, which lies
    /// within the piece's y range.
    fn x_at(&self, y: f64) -> f64 {
        if let PathSeg::Line(line) = self.segment {
            let (p0, p1) = (line.p0, line.p1);
            return p0.x + (y - p0.y) / (p1.y - p0.y) * (p1.x - p0.x);
        }
        // y is monotonic on [t0, t1]: bisect on which side of the line the
        // point is, keeping `lo` on the side of `y0`.
        let (mut lo, mut hi) = (self.t0, self.t1);
        let rising = self.y1 > self.y0;
        for _ in 0..SOLVE_STEPS {
            let mid = 0.5 * (lo + hi);
            let below = self.segment.eval(mid).y < y;
            if below == rising {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        self.segment.eval(0.5 * (lo + hi)).x
    }
}

/// A glyph outline prepared for horizontal line intersections.
#[derive(Clone, Debug)]
pub(crate) struct Outline {
    pieces: Vec<Piece>,
    /// The bounding box of the ink, if there is any outline.
    pub(crate) bbox: Option<Rect>,
}

impl Outline {
    /// The outline of `glyph` in `master`, with components expanded.
    pub(crate) fn of(
        font: &Font,
        glyph: GlyphId,
        master: MasterId,
    ) -> Result<Outline, SpacingError> {
        layer(font, glyph, master)?;
        let paths = flatten_layer(font, glyph, master)?;
        let mut pieces = Vec::new();
        for path in &paths {
            let bez = to_bezpath(path)?;
            let segments: Vec<PathSeg> = bez.segments().collect();
            for segment in &segments {
                split(*segment, &mut pieces);
            }
            // An open contour is filled as if it were closed.
            if let (Some(first), Some(last)) = (segments.first(), segments.last()) {
                let (start, end) = (first.eval(0.0), last.eval(1.0));
                if start != end {
                    split(PathSeg::Line(Line::new(end, start)), &mut pieces);
                }
            }
        }
        Ok(Outline {
            pieces,
            bbox: forma_geometry::bbox(&paths),
        })
    }

    /// The runs of ink on the line at height `y`, left to right, as
    /// `(start, end)`; touching runs are merged.
    pub(crate) fn spans(&self, y: f64) -> Vec<(f64, f64)> {
        // Half-open rule: a piece counts when y0 ≤ y < y1 (in its own
        // direction), so a line through a vertex shared by two pieces counts
        // it once, and a horizontal piece never counts.
        let mut crossings: Vec<(f64, i32)> = self
            .pieces
            .iter()
            .filter(|piece| {
                let (lo, hi) = if piece.y0 < piece.y1 {
                    (piece.y0, piece.y1)
                } else {
                    (piece.y1, piece.y0)
                };
                lo <= y && y < hi
            })
            .map(|piece| (piece.x_at(y), piece.winding))
            .collect();
        crossings.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

        let mut spans: Vec<(f64, f64)> = Vec::new();
        let mut winding = 0;
        let mut start = 0.0;
        for (x, delta) in crossings {
            let before = winding;
            winding += delta;
            if before == 0 && winding != 0 {
                start = x;
            } else if before != 0 && winding == 0 {
                match spans.last_mut() {
                    Some(last) if start <= last.1 => last.1 = last.1.max(x),
                    _ => spans.push((start, x)),
                }
            }
        }
        spans
    }

    /// The leftmost and rightmost ink on the line at height `y`.
    pub(crate) fn extent(&self, y: f64) -> Option<(f64, f64)> {
        let spans = self.spans(y);
        Some((spans.first()?.0, spans.last()?.1))
    }

    /// The profile on `lines` lines centred in equal bands of `zone`.
    pub(crate) fn raw_profile(&self, zone: Zone, lines: usize) -> Profile {
        let ys = line_heights(zone, lines);
        let (left, right) = ys
            .iter()
            .map(|&y| match self.extent(y) {
                Some((l, r)) => (Some(l), Some(r)),
                None => (None, None),
            })
            .unzip();
        Profile { ys, left, right }
    }

    /// [`profile`] on a prepared outline: the zone first, then the glyph's
    /// own ink height when the zone has fewer than 8 lines of ink.
    pub(crate) fn glyph_profile(&self, zone: Zone, lines: usize) -> Profile {
        let profile = self.raw_profile(zone, lines);
        match self.bbox {
            Some(bbox) if profile.ink_lines() < MIN_INK_LINES && bbox.height() > 0.0 => self
                .raw_profile(
                    Zone {
                        bottom: bbox.y0,
                        top: bbox.y1,
                    },
                    lines,
                ),
            _ => profile,
        }
    }
}

/// The heights of `lines` lines centred in equal bands of `zone`.
pub(crate) fn line_heights(zone: Zone, lines: usize) -> Vec<f64> {
    let step = (zone.top - zone.bottom) / lines as f64;
    (0..lines)
        .map(|i| zone.bottom + (i as f64 + 0.5) * step)
        .collect()
}

/// Splits `segment` at its extrema into pieces monotonic in y, dropping the
/// horizontal ones.
fn split(segment: PathSeg, pieces: &mut Vec<Piece>) {
    let mut ts = vec![0.0];
    ts.extend(segment.extrema());
    ts.push(1.0);
    for pair in ts.windows(2) {
        let (t0, t1) = (pair[0], pair[1]);
        let (y0, y1) = (segment.eval(t0).y, segment.eval(t1).y);
        if y0 == y1 || !(y0.is_finite() && y1.is_finite()) {
            continue;
        }
        pieces.push(Piece {
            segment,
            t0,
            t1,
            y0,
            y1,
            winding: if y1 > y0 { 1 } else { -1 },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{CubicBez, Point};

    fn outline(segments: &[PathSeg]) -> Outline {
        let mut pieces = Vec::new();
        for s in segments {
            split(*s, &mut pieces);
        }
        Outline { pieces, bbox: None }
    }

    fn line(a: (f64, f64), b: (f64, f64)) -> PathSeg {
        PathSeg::Line(Line::new(a, b))
    }

    fn square(x0: f64, x1: f64, ccw: bool) -> Vec<PathSeg> {
        let pts = [(x0, 0.0), (x1, 0.0), (x1, 100.0), (x0, 100.0)];
        let mut segs: Vec<PathSeg> = (0..4).map(|i| line(pts[i], pts[(i + 1) % 4])).collect();
        if !ccw {
            segs = segs
                .into_iter()
                .rev()
                .map(|s| match s {
                    PathSeg::Line(l) => PathSeg::Line(Line::new(l.p1, l.p0)),
                    other => other,
                })
                .collect();
        }
        segs
    }

    #[test]
    fn nonzero_fill_merges_overlaps_and_skips_holes() {
        // Two overlapping squares wound the same way: one run.
        let mut segs = square(0.0, 60.0, true);
        segs.extend(square(40.0, 100.0, true));
        assert_eq!(outline(&segs).spans(50.0), vec![(0.0, 100.0)]);
        // A hole wound the other way: two runs.
        let mut segs = square(0.0, 100.0, true);
        segs.extend(square(30.0, 70.0, false));
        assert_eq!(outline(&segs).spans(50.0), vec![(0.0, 30.0), (70.0, 100.0)]);
        // Two touching squares: merged.
        let mut segs = square(0.0, 50.0, true);
        segs.extend(square(50.0, 100.0, true));
        assert_eq!(outline(&segs).spans(50.0), vec![(0.0, 100.0)]);
        // Outside: nothing; on the top edge: nothing (half-open).
        assert!(outline(&segs).spans(100.0).is_empty());
        assert_eq!(outline(&segs).extent(0.0), Some((0.0, 100.0)));
    }

    #[test]
    fn curves_are_intersected_exactly() {
        // A cubic arc bulging right from (0,0) to (0,100), closed by a line.
        let arc = PathSeg::Cubic(CubicBez::new(
            Point::new(0.0, 0.0),
            Point::new(80.0, 0.0),
            Point::new(80.0, 100.0),
            Point::new(0.0, 100.0),
        ));
        let o = outline(&[arc, line((0.0, 100.0), (0.0, 0.0))]);
        let (l, r) = o.extent(50.0).expect("ink");
        assert_eq!(l, 0.0);
        // At t = 0.5 the arc is at x = 60 and y = 50.
        assert!((r - 60.0).abs() < 1e-9, "{r}");
    }

    #[test]
    fn lines_sit_in_band_centres() {
        let ys = line_heights(
            Zone {
                bottom: 0.0,
                top: 80.0,
            },
            8,
        );
        assert_eq!(ys, vec![5.0, 15.0, 25.0, 35.0, 45.0, 55.0, 65.0, 75.0]);
    }
}
