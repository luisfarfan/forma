//! The reproduction self-test: the font's own letters drawn from skeletons
//! in its measured style, compared with the letters as traced. Skeletons
//! draw strokes, their contrast and their ends; a drawing whose letters
//! they reproduce poorly has what they do not draw (serifs, tapers,
//! irregular strokes), and what synthesis makes for it needs a look.

use forma_geometry::{bbox, flatten_layer, to_bezpath};
use forma_model::{Font, Path};
use kurbo::{BezPath, Point, Shape};

use crate::kit::StyleKit;
use crate::recipe::{Class, Recipe, Seg, Strategy, StrokeSpec};

/// Samples per side of the comparison grid.
const GRID: usize = 96;

fn line(points: &[[f64; 2]]) -> StrokeSpec {
    StrokeSpec {
        segs: points.iter().map(|p| Seg::Line(*p)).collect(),
        w: 1.0,
    }
}

/// Skeletons of letters most alphabets draw plainly.
fn letters() -> Vec<(&'static str, Vec<StrokeSpec>)> {
    vec![
        ("I", vec![line(&[[0.5, 0.0], [0.5, 1.0]])]),
        (
            "H",
            vec![
                line(&[[0.0, 0.0], [0.0, 1.0]]),
                line(&[[1.0, 0.0], [1.0, 1.0]]),
                line(&[[0.0, 0.5], [1.0, 0.5]]),
            ],
        ),
        (
            "O",
            vec![StrokeSpec {
                segs: vec![Seg::Ellipse([0.5, 0.5, 0.5, 0.5])],
                w: 1.0,
            }],
        ),
        ("L", vec![line(&[[0.0, 1.0], [0.0, 0.0], [1.0, 0.0]])]),
        (
            "E",
            vec![
                line(&[[1.0, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0]]),
                line(&[[0.0, 0.5], [0.85, 0.5]]),
            ],
        ),
        (
            "T",
            vec![
                line(&[[0.0, 1.0], [1.0, 1.0]]),
                line(&[[0.5, 1.0], [0.5, 0.0]]),
            ],
        ),
    ]
}

fn beziers(paths: &[Path]) -> Vec<BezPath> {
    paths.iter().filter_map(|p| to_bezpath(p).ok()).collect()
}

/// Whether the non-zero fill of `shapes` covers `p`.
fn covers(shapes: &[BezPath], p: Point) -> bool {
    shapes.iter().map(|s| s.winding(p)).sum::<i32>() != 0
}

/// The overlap of two outlines, each moved so its ink starts at the same
/// place, sampled on a grid over both.
fn overlap(a: &[Path], b: &[Path]) -> Option<f64> {
    let (ra, rb) = (bbox(a)?, bbox(b)?);
    let shift = |paths: &[Path], dx: f64| {
        forma_geometry::transform_paths(
            paths,
            &forma_model::Transform {
                dx,
                ..forma_model::Transform::IDENTITY
            },
        )
    };
    let (a, b) = (beziers(&shift(a, -ra.x0)), beziers(&shift(b, -rb.x0)));
    let (w, y0, y1) = (
        ra.width().max(rb.width()),
        ra.y0.min(rb.y0),
        ra.y1.max(rb.y1),
    );
    if w <= 0.0 || y1 <= y0 {
        return None;
    }
    let (mut both, mut either) = (0usize, 0usize);
    for j in 0..GRID {
        for i in 0..GRID {
            let p = Point::new(
                (i as f64 + 0.5) / GRID as f64 * w,
                y0 + (j as f64 + 0.5) / GRID as f64 * (y1 - y0),
            );
            let (in_a, in_b) = (covers(&a, p), covers(&b, p));
            both += usize::from(in_a && in_b);
            either += usize::from(in_a || in_b);
        }
    }
    (either > 0).then(|| both as f64 / either as f64)
}

/// The median overlap between the font's letters and the same letters drawn
/// from skeletons in the style of `kit`; `None` when the font has none of
/// them or none could be drawn.
pub(crate) fn reproduction(font: &Font, kit: &StyleKit) -> Option<f64> {
    let mut found = Vec::new();
    for (name, strokes) in letters() {
        let Some(glyph) = font.glyph_by_name(name) else {
            continue;
        };
        let Ok(traced) = flatten_layer(font, glyph.id, kit.master) else {
            continue;
        };
        let Some(b) = bbox(&traced) else { continue };
        if kit.cap_height <= 0.0 {
            return None;
        }
        // Drawn as wide as the traced letter.
        let recipe = Recipe {
            name: name.to_owned(),
            unicodes: Vec::new(),
            class: Class::Symbol,
            width: b.width() / (kit.cap_height * kit.proportion()),
            height: 1.0,
            strategies: Vec::new(),
            needs_design: None,
            stroke_check: true,
        };
        let strategy = Strategy {
            id: "self-test".to_owned(),
            parts: Vec::new(),
            strokes,
            dots: Vec::new(),
        };
        let Ok(Some(drawn)) = crate::dress::build(font, kit, &recipe, &strategy) else {
            continue;
        };
        if let Some(o) = overlap(&drawn.paths, &traced) {
            found.push(o);
        }
    }
    if found.is_empty() {
        return None;
    }
    found.sort_by(f64::total_cmp);
    Some(found[found.len() / 2])
}
