//! Building a glyph from a strategy: pieces of the font's own glyphs,
//! skeleton strokes dressed with the measured style, and dots.
//!
//! A skeleton is dressed with the kit: its stroke width; its contrast,
//! when the font has one (strokes thicken as they turn vertical, between
//! the measured thin and thick widths); its terminals (balls of the
//! measured size when the drawing's ends swell, round ends otherwise).
//! Everything is united, given the PostScript direction and put on the
//! 1-unit grid.

use forma_geometry::{
    Cap, IssueKind, Skeleton, SkeletonNode, StrokeStyle, bbox, fix_paths, flatten_layer,
    normalize_direction, remove_overlap, stroke, transform_paths,
};
use forma_model::{Font, GlyphId, Node, NodeKind, Path, PathId, Transform};
use kurbo::{Point, Vec2};

use crate::SynthError;
use crate::kit::StyleKit;
use crate::recipe::{Class, Dot, Part, Recipe, Seg, Strategy, StrokeSpec};

/// The width of the hairline structure is compared with, in strokes.
const HAIRLINE: f64 = 0.15;
/// A turn sharper than this, in degrees, is a corner.
const CORNER_TURN: f64 = 60.0;
/// A ball goes on an end whose arm (to the first corner) is at least this
/// many ball diameters long.
const BALL_MIN_ARM: f64 = 0.9;
/// A skeleton box is at least this many reaches (half strokes, or ball
/// radii) wide, so a stroke has room to travel between its ends.
const MIN_TRAVEL_REACHES: f64 = 2.0;
/// A mark's box is at least this many stroke widths (or ball widths) on
/// each side.
const MARK_MIN_STROKES: f64 = 2.0;
/// A dot is at least this many stroke widths across...
const DOT_MIN: f64 = 1.24;
/// The cubic handle length of a quarter ellipse, as a fraction of a radius.
const KAPPA_QUARTER: f64 = 0.552_284_749_830_793_4;

/// A glyph made by a strategy, before it is placed.
pub(crate) struct Built {
    /// Its contours, in font units, ink starting at x = 0.
    pub paths: Vec<Path>,
    /// The glyphs it reused.
    pub sources: Vec<GlyphId>,
    /// Its pieces and counters.
    pub structure: (usize, usize),
    /// The pieces and counters of its skeleton drawn with a hairline.
    pub expected: (usize, usize),
}

fn geometry(error: impl std::fmt::Display) -> SynthError {
    SynthError::Geometry(error.to_string())
}

/// Maps the normalized box of a recipe to font units: x across the ink
/// width, y up the cap height, both inset by half the stroke.
struct Frame {
    x0: f64,
    ink_width: f64,
    height: f64,
    stroke: f64,
}

impl Frame {
    fn point(&self, [x, y]: [f64; 2], w: f64) -> Point {
        let half = w / 2.0;
        Point::new(
            self.x0 + half + x * (self.ink_width - w).max(0.0),
            half + y * (self.height - w).max(0.0),
        )
    }
}

/// The skeleton nodes of a stroke, in font units, without widths yet.
fn skeleton(frame: &Frame, spec: &StrokeSpec, w: f64) -> (Vec<(Point, NodeKind)>, bool) {
    let mut nodes: Vec<(Point, NodeKind)> = Vec::new();
    let mut closed = false;
    for seg in &spec.segs {
        match *seg {
            Seg::Line(p) => nodes.push((frame.point(p, w), NodeKind::Line)),
            Seg::Ellipse([cx, cy, rx, ry]) => {
                closed = true;
                arc(frame, w, [cx, cy, rx, ry, 0.0, 360.0], &mut nodes);
            }
            Seg::Arc(a) => arc(frame, w, a, &mut nodes),
        }
    }
    if closed && nodes.len() > 1 {
        // The arc returned to its start: drop the repeated node, and let
        // the first node close the last segment.
        if let Some((_, kind)) = nodes.pop() {
            nodes[0].1 = kind;
        }
    }
    (nodes, closed)
}

/// Appends an elliptical arc, in cubic pieces of at most 90 degrees; the
/// first point is added when the stroke starts with it.
fn arc(
    frame: &Frame,
    w: f64,
    [cx, cy, rx, ry, from, to]: [f64; 6],
    nodes: &mut Vec<(Point, NodeKind)>,
) {
    let span = to - from;
    let pieces = (span.abs() / 90.0).ceil().max(1.0) as usize;
    let step = span / pieces as f64;
    let at = |deg: f64| {
        [
            cx + rx * deg.to_radians().cos(),
            cy + ry * deg.to_radians().sin(),
        ]
    };
    let tangent = |deg: f64| {
        let r = deg.to_radians();
        [-rx * r.sin(), ry * r.cos()]
    };
    let start = frame.point(at(from), w);
    if nodes
        .last()
        .is_none_or(|(p, _)| (*p - start).hypot() > 1e-6)
    {
        nodes.push((start, NodeKind::Line));
    }
    let k = 4.0 / 3.0 * (step.to_radians() / 4.0).tan();
    for i in 0..pieces {
        let (a0, a1) = (from + step * i as f64, from + step * (i + 1) as f64);
        let (p0, p3) = (at(a0), at(a1));
        let (t0, t1) = (tangent(a0), tangent(a1));
        let p1 = [p0[0] + k * t0[0], p0[1] + k * t0[1]];
        let p2 = [p3[0] - k * t1[0], p3[1] - k * t1[1]];
        nodes.push((frame.point(p1, w), NodeKind::OffCurve));
        nodes.push((frame.point(p2, w), NodeKind::OffCurve));
        nodes.push((frame.point(p3, w), NodeKind::Curve));
    }
}

/// Whether a stroke spec is a closed loop (an ellipse).
fn closed_spec(spec: &StrokeSpec) -> bool {
    spec.segs.iter().any(|s| matches!(s, Seg::Ellipse(_)))
}

/// Moves the first and last nodes of an open skeleton `by` further along
/// the directions the skeleton leaves and arrives in.
fn extend(nodes: &mut [(Point, NodeKind)], by: f64) {
    let n = nodes.len();
    for (end, next) in [(0, 1), (n - 1, n - 2)] {
        let away = nodes[end].0 - nodes[next].0;
        let len = away.hypot();
        if len > 1e-9 {
            nodes[end].0 += away * (by / len);
        }
    }
}

/// The width of the stroke at node `i`: the base width, modulated by the
/// direction of the stroke there when the font has contrast.
fn width_at(nodes: &[(Point, NodeKind)], i: usize, closed: bool, base: f64, kit: &StyleKit) -> f64 {
    let Some((thin, thick)) = kit.contrast() else {
        return base;
    };
    let n = nodes.len();
    let prev = if i > 0 {
        Some(nodes[i - 1].0)
    } else if closed {
        Some(nodes[n - 1].0)
    } else {
        None
    };
    let next = if i + 1 < n {
        Some(nodes[i + 1].0)
    } else if closed {
        Some(nodes[0].0)
    } else {
        None
    };
    let here = nodes[i].0;
    let dir: Vec2 = match (prev, next) {
        (Some(a), Some(b)) => b - a,
        (Some(a), None) => here - a,
        (None, Some(b)) => b - here,
        (None, None) => return base,
    };
    let len = dir.hypot();
    if len <= 1e-9 {
        return base;
    }
    let vertical = (dir.y / len).abs();
    let (thin, thick) = (thin / kit.stroke, thick / kit.stroke);
    base * (thin + (thick - thin) * vertical)
}

/// A circle as a closed contour of four cubics, counter-clockwise.
fn circle(c: Point, r: f64) -> Path {
    let k = KAPPA_QUARTER * r;
    let node = |x: f64, y: f64, kind: NodeKind| Node {
        x,
        y,
        kind,
        smooth: kind == NodeKind::Curve,
    };
    let (x, y) = (c.x, c.y);
    Path {
        id: PathId(0),
        closed: true,
        nodes: vec![
            node(x + r, y, NodeKind::Curve),
            node(x + r, y + k, NodeKind::OffCurve),
            node(x + k, y + r, NodeKind::OffCurve),
            node(x, y + r, NodeKind::Curve),
            node(x - k, y + r, NodeKind::OffCurve),
            node(x - r, y + k, NodeKind::OffCurve),
            node(x - r, y, NodeKind::Curve),
            node(x - r, y - k, NodeKind::OffCurve),
            node(x - k, y - r, NodeKind::OffCurve),
            node(x, y - r, NodeKind::Curve),
            node(x + k, y - r, NodeKind::OffCurve),
            node(x + r, y - k, NodeKind::OffCurve),
        ],
    }
}

/// The radius of a dot of size `r`.
pub(crate) fn dot_radius(kit: &StyleKit, r: f64) -> f64 {
    let ball = kit.ball.map_or(0.0, |b| b * kit.stroke);
    r * (DOT_MIN * kit.stroke).max(ball) / 2.0
}

/// A piece of an existing glyph, transformed and placed; `None` if the
/// glyph does not exist or has no outline.
fn part(font: &Font, kit: &StyleKit, p: &Part) -> Result<Option<(Vec<Path>, GlyphId)>, SynthError> {
    let Some(glyph) = font.glyph_by_name(&p.glyph) else {
        return Ok(None);
    };
    let paths = flatten_layer(font, glyph.id, kit.master).map_err(geometry)?;
    let Some(b) = bbox(&paths) else {
        return Ok(None);
    };
    let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
    let affine = |xx: f64, yy: f64, dx: f64, dy: f64| Transform {
        xx,
        xy: 0.0,
        yx: 0.0,
        yy,
        dx,
        dy,
    };
    let mut out = paths;
    if p.rotate {
        out = transform_paths(&out, &affine(-1.0, -1.0, 2.0 * cx, 2.0 * cy));
    }
    if p.mirror {
        out = transform_paths(&out, &affine(-1.0, 1.0, 2.0 * cx, 0.0));
    }
    // Scale to the width asked for, then place.
    let cap = kit.cap_height;
    let sx = match p.width {
        Some(w) if b.width() > 0.0 => w * cap * kit.proportion() / b.width(),
        _ => 1.0,
    };
    let width = b.width() * sx;
    let left = if p.center {
        p.dx * cap - width / 2.0
    } else {
        p.dx * cap
    } + p.advance * width;
    let dy = match p.top {
        Some(top) => top * cap - b.y1,
        None => p.dy * cap,
    };
    let t = affine(sx, 1.0, left - b.x0 * sx, dy);
    Ok(Some((transform_paths(&out, &t), glyph.id)))
}

/// Builds the glyph of `recipe` by `strategy`; `None` when a piece it
/// reuses is missing. The result also tells the pieces and counters the
/// glyph has, and those its skeleton has drawn with a hairline: where they
/// differ, the font's strokes merge pieces or close counters, and the
/// glyph would not read as itself.
///
/// # Errors
///
/// [`SynthError::Geometry`] if stroking or uniting fails.
pub(crate) fn build(
    font: &Font,
    kit: &StyleKit,
    recipe: &Recipe,
    strategy: &Strategy,
) -> Result<Option<Built>, SynthError> {
    let Some((paths, sources)) = dress(font, kit, recipe, strategy, false)? else {
        return Ok(None);
    };
    let structure = topology(&paths);
    let expected = match dress(font, kit, recipe, strategy, true)? {
        Some((hairline, _)) => topology(&hairline),
        None => structure,
    };
    Ok(Some(Built {
        paths,
        sources,
        structure,
        expected,
    }))
}

/// Pieces (outer contours) and counters (holes) of united contours.
fn topology(paths: &[Path]) -> (usize, usize) {
    let depths = forma_geometry::contour_depths(paths);
    let holes = depths.iter().filter(|d| *d % 2 == 1).count();
    (depths.len() - holes, holes)
}

/// The length of the arm at an end of a skeleton: along the stroke from
/// that end to the first corner (a turn of more than [`CORNER_TURN`]
/// degrees), or to the other end.
fn arm_length(nodes: &[(Point, NodeKind)], last: bool) -> f64 {
    let ordered: Vec<Point> = if last {
        nodes.iter().rev().map(|(p, _)| *p).collect()
    } else {
        nodes.iter().map(|(p, _)| *p).collect()
    };
    let kinds: Vec<NodeKind> = if last {
        nodes.iter().rev().map(|(_, k)| *k).collect()
    } else {
        nodes.iter().map(|(_, k)| *k).collect()
    };
    let mut length = 0.0;
    for j in 1..ordered.len() {
        length += (ordered[j] - ordered[j - 1]).hypot();
        if j + 1 < ordered.len() && kinds[j] != NodeKind::OffCurve {
            let (a, b) = (ordered[j] - ordered[j - 1], ordered[j + 1] - ordered[j]);
            let turn = a.cross(b).atan2(a.dot(b)).abs().to_degrees();
            if turn > CORNER_TURN {
                break;
            }
        }
    }
    length
}

/// Contours, and the glyphs reused as pieces.
type Drawn = (Vec<Path>, Vec<GlyphId>);

/// Draws the glyph: in the font's style, or with a hairline along the same
/// skeleton (for comparing structure).
fn dress(
    font: &Font,
    kit: &StyleKit,
    recipe: &Recipe,
    strategy: &Strategy,
    hairline: bool,
) -> Result<Option<Drawn>, SynthError> {
    let mut paths: Vec<Path> = Vec::new();
    let mut sources = Vec::new();
    for p in &strategy.parts {
        let Some((piece, id)) = part(font, kit, p)? else {
            return Ok(None);
        };
        paths.extend(piece);
        sources.push(id);
    }
    // How far ink reaches around a skeleton point: half the stroke, or the
    // ball's radius at an end.
    let reach_ratio = kit.ball.unwrap_or(1.0).max(1.0);
    // Skeletons are drawn in the box of the pieces when there are pieces
    // (the bar of a dollar across its S), in the recipe's box otherwise,
    // wide enough for a stroke to travel between its ends.
    let (x0, ink_width) = match bbox(&paths) {
        Some(b) => (b.x0, b.width()),
        None => (
            0.0,
            (recipe.width * kit.cap_height * kit.proportion())
                .max(MIN_TRAVEL_REACHES * kit.stroke * reach_ratio),
        ),
    };
    let mut frame = Frame {
        x0,
        ink_width,
        height: recipe.height * kit.cap_height,
        stroke: kit.stroke,
    };
    if recipe.class == Class::Mark {
        // A mark's box leaves room for its strokes to travel, however heavy
        // the font.
        let least = MARK_MIN_STROKES * kit.stroke * reach_ratio;
        frame.ink_width = frame.ink_width.max(least);
        frame.height = frame.height.max(least);
    }
    let flat = !hairline && kit.flat_ends();
    let cap = if flat { Cap::Flat } else { Cap::Round };
    let mut skeleton_points: Vec<Point> = Vec::new();
    for spec in &strategy.strokes {
        let base = frame.stroke * spec.w;
        let reach = if closed_spec(spec) {
            base
        } else {
            base * reach_ratio
        };
        let (mut nodes, closed) = skeleton(&frame, spec, reach);
        if nodes.len() < 2 {
            continue;
        }
        skeleton_points.extend(nodes.iter().map(|(p, _)| *p));
        if flat && !closed {
            // A flat end stops where the skeleton does, half a width short
            // of where a round one reaches: lengthen the ends to match.
            extend(&mut nodes, base / 2.0);
        }
        let width = |i: usize| {
            if hairline {
                HAIRLINE * kit.stroke
            } else {
                width_at(&nodes, i, closed, base, kit)
            }
        };
        let skel = Skeleton {
            nodes: nodes
                .iter()
                .enumerate()
                .map(|(i, &(p, kind))| SkeletonNode {
                    x: p.x,
                    y: p.y,
                    kind,
                    width: (kind != NodeKind::OffCurve).then(|| width(i)),
                })
                .collect(),
            closed,
        };
        let style = StrokeStyle {
            width: if hairline {
                HAIRLINE * kit.stroke
            } else {
                base
            },
            cap,
        };
        let drawn = stroke(&[skel], &style).map_err(geometry)?;
        paths.extend(drawn);
        // Balls only on ends with room for them: on a short arm (of a
        // bracket, a guillemet) a ball would swallow the arm.
        if let (Some(ratio), false, false) = (kit.ball, hairline, closed) {
            let diameter = ratio * base;
            for (last, end) in [(false, 0), (true, nodes.len() - 1)] {
                if arm_length(&nodes, last) >= BALL_MIN_ARM * diameter {
                    paths.push(circle(nodes[end].0, diameter / 2.0));
                }
            }
        }
    }
    for Dot { at, r } in &strategy.dots {
        let radius = dot_radius(kit, *r);
        let centre = frame.point(*at, 2.0 * radius);
        // A dot a stroke starts in (the head of a comma) is part of that
        // stroke by design: it keeps its size in the hairline too.
        let joined = skeleton_points
            .iter()
            .any(|p| (*p - centre).hypot() <= radius);
        let drawn = if hairline && !joined {
            HAIRLINE * radius
        } else {
            radius
        };
        paths.push(circle(centre, drawn));
    }
    if paths.is_empty() {
        return Ok(None);
    }
    for (k, p) in paths.iter_mut().enumerate() {
        p.id = PathId(k as u32);
    }
    let mut united = remove_overlap(&paths).map_err(geometry)?;
    normalize_direction(&mut united);
    let fixed = fix_paths(
        &united,
        &[
            IssueKind::DuplicatePoints,
            IssueKind::TinySegment,
            IssueKind::MissingExtrema,
            IssueKind::OffGrid,
            IssueKind::WrongDirection,
        ],
        1.0,
    );
    // Ink from x = 0 (it is placed later).
    let Some(b) = bbox(&fixed) else {
        return Ok(None);
    };
    let shift = Transform {
        dx: -b.x0.round(),
        ..Transform::IDENTITY
    };
    let mut placed = transform_paths(&fixed, &shift);
    for (k, p) in placed.iter_mut().enumerate() {
        p.id = PathId(k as u32);
    }
    Ok(Some((placed, sources)))
}
