//! Measurements that need no agent: rasterizing outlines, intersection over
//! union between two outlines and hole counting.
//!
//! All outlines are [`BezPath`]s in pixels (y down), filled with the
//! non-zero rule like the font rasterizers do.

use forma_geometry::from_bezpath;
use forma_model::{Path, Transform};
use forma_render::{Mask, coverage_mask, mask_iou};
use kurbo::{BezPath, Rect, Shape};

use crate::error::BenchError;

/// The identity pixel transform.
const IDENTITY: Transform = Transform {
    xx: 1.0,
    xy: 0.0,
    yx: 0.0,
    yy: 1.0,
    dx: 0.0,
    dy: 0.0,
};

/// The coverage threshold of "inside" (the usual 50 %).
const INSIDE: u8 = 128;

/// The resolution a hole count is measured at, in pixels of the larger side.
const HOLE_RASTER_SIDE: f64 = 480.0;

/// The most a hole count magnifies its shape.
const HOLE_RASTER_MAX_SCALE: f64 = 8.0;

/// `shapes` as model contours with unique ids.
///
/// # Errors
///
/// [`BenchError::Geometry`] if a path is not finite or is degenerate.
pub fn to_model_paths(shapes: &[BezPath]) -> Result<Vec<Path>, BenchError> {
    let mut out = Vec::new();
    for shape in shapes {
        let first = u32::try_from(out.len()).unwrap_or(u32::MAX);
        let paths = from_bezpath(shape, first).map_err(|e| BenchError::Geometry(e.to_string()))?;
        out.extend(paths);
    }
    Ok(out)
}

/// The union of the bounding boxes of `shapes`.
#[must_use]
pub fn bounds(shapes: &[BezPath]) -> Option<Rect> {
    shapes
        .iter()
        .map(Shape::bounding_box)
        .reduce(|a, b| a.union(b))
}

/// Rasterizes `shapes` (pixel coordinates, y down) into a mask of the given
/// size.
///
/// # Errors
///
/// [`BenchError::Geometry`] if the shapes or the size are invalid.
pub fn rasterize(shapes: &[BezPath], width: u32, height: u32) -> Result<Mask, BenchError> {
    let paths = to_model_paths(shapes)?;
    coverage_mask(&paths, &IDENTITY, width, height).map_err(|e| BenchError::Geometry(e.to_string()))
}

/// The intersection over union of two sets of shapes, rasterized together on
/// the pixel grid that holds both (with a margin).
///
/// # Errors
///
/// [`BenchError::Geometry`] if a shape is invalid or the area is too large.
pub fn shape_iou(a: &[BezPath], b: &[BezPath]) -> Result<f64, BenchError> {
    let (Some(ra), Some(rb)) = (bounds(a), bounds(b)) else {
        return Ok(if a.is_empty() && b.is_empty() {
            1.0
        } else {
            0.0
        });
    };
    let frame = ra.union(rb);
    let (ox, oy) = (frame.x0.floor() - 2.0, frame.y0.floor() - 2.0);
    let width = (frame.x1.ceil() + 2.0 - ox).max(1.0);
    let height = (frame.y1.ceil() + 2.0 - oy).max(1.0);
    let shift = kurbo::Affine::translate((-ox, -oy));
    let moved =
        |shapes: &[BezPath]| -> Vec<BezPath> { shapes.iter().map(|s| shift * s.clone()).collect() };
    let (w, h) = (width as u32, height as u32);
    let ma = rasterize(&moved(a), w, h)?;
    let mb = rasterize(&moved(b), w, h)?;
    mask_iou(&ma, &mb, INSIDE).map_err(|e| BenchError::Geometry(e.to_string()))
}

/// The number of holes of `shapes`: the connected regions of background
/// that do not reach the border, measured on a raster (so overlapping or
/// badly oriented contours do not matter). The shape is magnified so thin
/// slits are still resolved.
///
/// # Errors
///
/// [`BenchError::Geometry`] if a shape is invalid.
pub fn count_holes(shapes: &[BezPath]) -> Result<usize, BenchError> {
    let Some(frame) = bounds(shapes) else {
        return Ok(0);
    };
    let side = frame.width().max(frame.height()).max(1.0);
    let scale = (HOLE_RASTER_SIDE / side).clamp(1.0, HOLE_RASTER_MAX_SCALE);
    let pad = 3.0;
    let map = kurbo::Affine::translate((pad, pad))
        * kurbo::Affine::scale(scale)
        * kurbo::Affine::translate((-frame.x0, -frame.y0));
    let width = (frame.width() * scale + 2.0 * pad).ceil().max(1.0) as u32;
    let height = (frame.height() * scale + 2.0 * pad).ceil().max(1.0) as u32;
    let moved: Vec<BezPath> = shapes.iter().map(|s| map * s.clone()).collect();
    let mask = rasterize(&moved, width, height)?;
    Ok(holes_in_mask(&mask))
}

/// The enclosed background regions (4-connected) of `mask`.
#[must_use]
pub fn holes_in_mask(mask: &Mask) -> usize {
    let (w, h) = (mask.width as usize, mask.height as usize);
    let ink = |i: usize| mask.data[i] >= INSIDE;
    let mut seen = vec![false; w * h];
    let mut holes = 0;
    // The background that touches the border comes first (the outside), then
    // every region that is left is a hole.
    let border = (0..w)
        .chain((0..w).map(|x| (h - 1) * w + x))
        .chain((0..h).map(|y| y * w))
        .chain((0..h).map(|y| y * w + w - 1));
    let mut starts: Vec<usize> = border.collect();
    let mut pass = 0;
    loop {
        for &start in &starts {
            if seen[start] || ink(start) {
                continue;
            }
            flood(&mut seen, start, w, h, &ink);
            if pass > 0 {
                holes += 1;
            }
        }
        if pass > 0 {
            break;
        }
        pass = 1;
        starts = (0..w * h).collect();
    }
    holes
}

fn flood(seen: &mut [bool], start: usize, w: usize, h: usize, ink: &impl Fn(usize) -> bool) {
    let mut stack = vec![start];
    seen[start] = true;
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let mut push = |j: usize| {
            if !seen[j] && !ink(j) {
                seen[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            push(i - 1);
        }
        if x + 1 < w {
            push(i + 1);
        }
        if y > 0 {
            push(i - w);
        }
        if y + 1 < h {
            push(i + w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Rect as R;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64, positive: bool) -> BezPath {
        let mut p = R::new(x0, y0, x1, y1).to_path(0.1);
        if (p.area() > 0.0) != positive {
            p = BezPath::from_vec(p.reverse_subpaths().elements().to_vec());
        }
        p
    }

    #[test]
    fn counts_holes_of_a_frame_and_of_a_solid() {
        let solid = rect(0.0, 0.0, 60.0, 60.0, true);
        assert_eq!(count_holes(std::slice::from_ref(&solid)).ok(), Some(0));
        let hole = rect(20.0, 20.0, 40.0, 40.0, false);
        assert_eq!(count_holes(&[solid.clone(), hole]).ok(), Some(1));
        let slit = rect(10.0, 29.5, 50.0, 30.5, false);
        assert_eq!(count_holes(&[solid, slit]).ok(), Some(1));
    }

    #[test]
    fn iou_of_identical_shapes_is_one() {
        let a = rect(0.0, 0.0, 30.0, 30.0, true);
        let iou = shape_iou(std::slice::from_ref(&a), std::slice::from_ref(&a)).ok();
        assert_eq!(iou, Some(1.0));
        let b = rect(15.0, 0.0, 45.0, 30.0, true);
        let iou = shape_iou(&[a], &[b]).unwrap_or(0.0);
        assert!((iou - 1.0 / 3.0).abs() < 0.02, "{iou}");
    }
}
