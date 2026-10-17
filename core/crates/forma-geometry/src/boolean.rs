//! Overlap removal: the non-zero union of contours.
//!
//! The union is computed by an engine behind the [`PathOps`] trait so that it
//! can be replaced without touching the callers (ADR 0009 keeps Skia PathOps
//! as plan B). The engine used today is [`Linesweeper`], built on the
//! `linesweeper` crate.

use std::any::Any;
use std::panic;

use forma_model::{Path, PathId};
use kurbo::{BezPath, PathEl, Point, Shape};
use linesweeper::topology::Topology;

use crate::{GeometryError, IssueKind, fix_paths, from_bezpath, to_bezpath};

/// The smallest sweep tolerance, in font units.
const MIN_SWEEP_TOLERANCE: f64 = 1e-6;

/// How many times [`Linesweeper`] unites the contours at most (see `sweep`).
/// One union is the rule. In testing, a second one removed the only crossing
/// left in 8 000 random strokes and cut the results with crossings in 3 000
/// random self-intersecting contours from 194 to 10; a third one made a few
/// more of those results stable.
const MAX_SWEEPS: usize = 3;

/// The size under which a piece of the sweep's output is an artefact of the
/// sweep rather than part of the shape (see `tidy`), in absolute font units:
/// it does not scale with the glyph or the units per em. Whenever
/// [`Linesweeper`] computes a union (not on the clean-input fast path),
/// segments no longer than this, contour ends this close to their start and
/// slivers no wider than this are dropped, including features of the input
/// that small. The artefacts measured grow with the square root of the size
/// of the curves, about 0.01 units at a radius of 100 and 0.1 at 10 000;
/// anything this small disappears when coordinates are rounded to the font's
/// integer grid.
const ARTEFACT_SIZE: f64 = 0.1;

/// A Boolean-operations engine for contours.
pub trait PathOps {
    /// The union of the closed contours of `paths` under the non-zero fill
    /// rule: the outline of the area a rasterizer fills.
    ///
    /// What every engine promises:
    ///
    /// * Outer contours run counter-clockwise and counters clockwise
    ///   (PostScript direction). Counters (areas the contours surround but
    ///   do not fill) are kept as counters.
    /// * Contours that run against each other cancel where they overlap, as
    ///   they do when rendered: a clockwise contour over a counter-clockwise
    ///   one cuts it out.
    /// * Overlaps and self-intersections are resolved as well as the engine
    ///   can: an engine that checks and retries its result does so a bounded
    ///   number of times and then returns what it has, so on adversarial
    ///   input (wild self-intersecting curves) a crossing can remain, and
    ///   the contours around it may not nest cleanly.
    /// * Open contours enclose nothing: they are kept unchanged after the
    ///   united ones.
    /// * When the closed contours already are the outline of their union (no
    ///   overlaps, no self-intersections, PostScript direction), `paths` is
    ///   returned unchanged: same nodes and ids, nothing added, nothing
    ///   renumbered, so that callers can tell that there was nothing to do.
    ///
    /// Everything else (where nodes go, whether extrema get nodes, how
    /// contours are numbered, the tolerances) is up to the engine; see
    /// [`Linesweeper`].
    ///
    /// # Errors
    ///
    /// * [`GeometryError::InvalidPath`] or [`GeometryError::Degenerate`] if a
    ///   contour cannot be converted (see [`to_bezpath`]).
    /// * [`GeometryError::Degenerate`] if the engine fails on the input.
    fn union(&self, paths: &[Path]) -> Result<Vec<Path>, GeometryError>;
}

/// The [`PathOps`] engine built on the `linesweeper` crate (0.5), a robust
/// sweep-line algorithm that works on Bezier paths directly, so curves stay
/// curves (quadratic segments stay quadratic).
///
/// When the input is not clean already (see [`PathOps::union`] for the fast
/// path), the united contours are numbered from 0 in order, curves get
/// on-curve nodes at their x and y extrema, and nodes are not smooth unless a
/// curve was split at an extremum. Pieces of the result no larger than 0.1
/// font units (absolute, whatever the size of the glyph) are dropped as
/// artefacts of the sweep: segments that short, contour ends that close to
/// their start, slivers that thin; features of the input that small go too.
///
/// The results are exact up to the sweep's tolerance (about 1e-6 font units
/// for glyph-sized coordinates): contours may touch, or share a stretch of
/// boundary, within it. The result is swept again as a check and united
/// again if needed, at most 3 unions in all. On wild input (random
/// self-intersecting curves) the result can still, rarely, keep a sliver
/// whose sides cross; and in testing, `linesweeper` itself once left out a
/// piece of the filled area (one random stroke in some 200 000).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Linesweeper;

impl PathOps for Linesweeper {
    fn union(&self, paths: &[Path]) -> Result<Vec<Path>, GeometryError> {
        let mut all = BezPath::new();
        let mut open: Vec<&Path> = Vec::new();
        for path in paths {
            let bez = to_bezpath(path)?;
            if path.closed {
                all.extend(bez);
            } else {
                open.push(path);
            }
        }
        if all.elements().is_empty() {
            return Ok(paths.to_vec());
        }

        // A panic inside the engine must not take the caller (an editor, a
        // server) down with it; it becomes an error like any other failure.
        let outcome = panic::catch_unwind(|| sweep(&all)).map_err(|payload| {
            GeometryError::Degenerate(format!(
                "overlap removal failed: the engine panicked: {}",
                panic_message(payload.as_ref())
            ))
        })?;
        let Some(contours) = outcome? else {
            return Ok(paths.to_vec());
        };

        // The sweep orients its contours already (outer counter-clockwise,
        // counters clockwise), so they are not normalized again.
        let mut united = Vec::with_capacity(contours.len() + open.len());
        for contour in &contours {
            let first_id = next_id(united.len())?;
            united.extend(from_bezpath(contour, first_id)?);
        }
        let mut united = fix_paths(
            &united,
            &[IssueKind::MissingExtrema, IssueKind::DuplicatePoints],
            0.0,
        );
        for path in open {
            united.push(Path {
                id: PathId(next_id(united.len())?),
                ..path.clone()
            });
        }
        Ok(united)
    }
}

/// Removes the overlaps of `paths` with the default engine, [`Linesweeper`].
///
/// See [`PathOps::union`] and [`Linesweeper`] for what the result looks
/// like; in particular, a glyph without overlaps comes back unchanged.
///
/// # Errors
///
/// As [`PathOps::union`]: a contour that cannot be converted, or
/// [`GeometryError::Degenerate`] if `linesweeper` fails.
pub fn remove_overlap(paths: &[Path]) -> Result<Vec<Path>, GeometryError> {
    Linesweeper.union(paths)
}

/// Sweeps the closed contours in `all` and returns the contours of their
/// non-zero union, cleaned by `tidy`, or `None` if `all` already is that
/// outline.
///
/// The union is the one `linesweeper::binary_op(all, empty, NonZero, Union)`
/// computes, with the same automatic tolerance, run on a [`Topology`]
/// directly so that the same sweep also tells whether the contours are
/// already clean (`has_normal_contours`: every boundary has the set on its
/// right in linesweeper's y-down convention, which is the PostScript
/// direction in font coordinates, with winding 1 inside). Its output
/// contours are oriented that way too: outer contours counter-clockwise and
/// counters clockwise in font coordinates.
///
/// Linesweeper's default output is only approximately right: it rebuilds
/// curves from pieces of the input, and where curves nearly touch, the result
/// can, rarely, keep a sliver whose sides cross. So the result is swept again
/// as a check and, if it is not clean, united again, up to [`MAX_SWEEPS`]
/// unions in all. The check is strict (contours that touch within the
/// tolerance fail it too), so after the last union the result is returned
/// as it is.
fn sweep(all: &BezPath) -> Result<Option<Vec<BezPath>>, GeometryError> {
    let mut swept = topology(all)?;
    if swept.has_normal_contours(()) {
        return Ok(None);
    }
    let mut contours = unite(&swept);
    for _ in 1..MAX_SWEEPS {
        swept = topology(&concatenate(&contours))?;
        if swept.has_normal_contours(()) {
            break;
        }
        contours = unite(&swept);
    }
    Ok(Some(contours))
}

/// The contours of the non-zero union of the swept paths, cleaned.
fn unite(topology: &Topology<i32>) -> Vec<BezPath> {
    topology
        .contours(|winding| *winding != 0)
        .into_contours()
        .filter_map(|contour| tidy(&contour.path))
        .collect()
}

fn topology(all: &BezPath) -> Result<Topology<i32>, GeometryError> {
    Topology::<i32>::from_path(all, sweep_tolerance(all))
        .map_err(|error| GeometryError::Degenerate(format!("overlap removal failed: {error}")))
}

fn concatenate(contours: &[BezPath]) -> BezPath {
    let mut all = BezPath::new();
    for contour in contours {
        all.extend(contour.iter());
    }
    all
}

/// Cleans one contour of the sweep's output, or returns `None` if it
/// encloses nothing worth keeping.
///
/// The sweep perturbs the input slightly where curves meet or nearly
/// coincide, which leaves artefacts: segments a hair long (a few 1e-10 units
/// where two circles cross, a few hundredths where two curves coincide), a last
/// segment that ends a hair away from the start, and slivers of zero area
/// between coincident curves. Segments whose points all lie within
/// [`ARTEFACT_SIZE`] of their start are dropped (the next segment then starts
/// where the dropped one did, so the shape moves by less than that), the last
/// segment is snapped onto the start, and contours whose area is no more than
/// a band [`ARTEFACT_SIZE`] wide along their bounding box are discarded.
fn tidy(contour: &BezPath) -> Option<BezPath> {
    let mut out = BezPath::new();
    let mut start: Option<Point> = None;
    let mut current = Point::ZERO;
    let mut segments = 0usize;
    for el in contour.elements() {
        let points: &[Point] = match el {
            PathEl::MoveTo(p) => {
                out.move_to(*p);
                start = Some(*p);
                current = *p;
                continue;
            }
            PathEl::ClosePath => {
                if let Some(start) = start
                    && current != start
                    && current.distance(start) <= ARTEFACT_SIZE
                {
                    snap_end(&mut out, start);
                    current = start;
                }
                out.close_path();
                continue;
            }
            PathEl::LineTo(p) => &[*p],
            PathEl::QuadTo(c, p) => &[*c, *p],
            PathEl::CurveTo(c1, c2, p) => &[*c1, *c2, *p],
        };
        if points.iter().all(|p| p.distance(current) <= ARTEFACT_SIZE) {
            continue;
        }
        out.push(*el);
        segments += 1;
        current = el.end_point().unwrap_or(current);
    }
    // As in `from_bezpath`: one on-curve node per segment plus the start,
    // which the last segment lands on when it closes the contour itself.
    let on_curve = 1 + segments - usize::from(start == Some(current));
    let bounds = out.bounding_box();
    let sliver = ARTEFACT_SIZE * (bounds.width() + bounds.height());
    (on_curve >= 2 && out.area().abs() > sliver).then_some(out)
}

/// Moves the end point of the last segment of `bez` to `to`.
fn snap_end(bez: &mut BezPath, to: Point) {
    if let Some(last) = bez.elements_mut().last_mut() {
        match last {
            PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => *p = to,
            PathEl::MoveTo(_) | PathEl::ClosePath => {}
        }
    }
}

/// The tolerance `linesweeper::binary_op` chooses: 64 ulps of the largest
/// coordinate, and never below 1e-6. The output contours stay within it of
/// the input.
fn sweep_tolerance(all: &BezPath) -> f64 {
    let bounds = all.bounding_box();
    let largest = [bounds.x0, bounds.y0, bounds.x1, bounds.y1]
        .into_iter()
        .map(f64::abs)
        .fold(0.0, f64::max);
    (largest * 64.0 * f64::EPSILON).max(MIN_SWEEP_TOLERANCE)
}

fn next_id(count: usize) -> Result<u32, GeometryError> {
    u32::try_from(count).map_err(|_| GeometryError::InvalidPath("too many contours".into()))
}

/// The message a panic was raised with, when it has one.
fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else {
        "no message"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerance_follows_the_size_of_the_input() {
        let mut small = BezPath::new();
        small.move_to((0.0, 0.0));
        small.line_to((10.0, 0.0));
        small.line_to((0.0, 10.0));
        small.close_path();
        assert_eq!(sweep_tolerance(&small), MIN_SWEEP_TOLERANCE);

        let mut huge = BezPath::new();
        huge.move_to((0.0, 0.0));
        huge.line_to((-1e12, 0.0));
        huge.line_to((0.0, 1e12));
        huge.close_path();
        assert!((sweep_tolerance(&huge) - 1e12 * 64.0 * f64::EPSILON).abs() < 1e-9);
    }
}
