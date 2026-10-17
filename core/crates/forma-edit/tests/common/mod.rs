//! Helpers shared by the integration tests.
#![allow(dead_code)]

pub mod doc;

use std::collections::BTreeSet;
use std::sync::Arc;

use forma_edit::{Command, EditError, GlyphRef, apply};
use forma_geometry::to_bezpath;
use forma_model::{
    EntityKey, Font, Glyph, GlyphId, Layer, Master, MasterId, Node, NodeKind, Path, PathId,
    VerticalMetrics,
};
use kurbo::{BezPath, ParamCurve, ParamCurveNearest};

// ------------------------------------------------------------------ nodes

pub fn node(x: f64, y: f64, kind: NodeKind) -> Node {
    Node {
        x,
        y,
        kind,
        smooth: false,
    }
}

pub fn line(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::Line)
}

pub fn off(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::OffCurve)
}

pub fn curve(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::Curve)
}

pub fn qcurve(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::QCurve)
}

pub fn closed(id: u32, nodes: Vec<Node>) -> Path {
    Path {
        id: PathId(id),
        closed: true,
        nodes,
    }
}

pub fn open(id: u32, nodes: Vec<Node>) -> Path {
    Path {
        id: PathId(id),
        closed: false,
        nodes,
    }
}

/// An axis-aligned square; counter-clockwise when `ccw` is true.
pub fn square(id: u32, x0: f64, y0: f64, x1: f64, y1: f64, ccw: bool) -> Path {
    let nodes = if ccw {
        vec![line(x0, y0), line(x1, y0), line(x1, y1), line(x0, y1)]
    } else {
        vec![line(x0, y0), line(x0, y1), line(x1, y1), line(x1, y0)]
    };
    closed(id, nodes)
}

const KAPPA: f64 = 0.552_284_749_830_793_4;

/// A counter-clockwise circle of four cubic segments starting at
/// `(cx + r, cy)`. The first node is a `Curve` whose two off-curve nodes sit
/// at the end of the list.
pub fn circle(id: u32, cx: f64, cy: f64, r: f64) -> Path {
    let p = |dx: f64, dy: f64| (cx + dx * r, cy + dy * r);
    let n = |(x, y): (f64, f64), kind| node(x, y, kind);
    let k = KAPPA;
    closed(
        id,
        vec![
            n(p(1.0, 0.0), NodeKind::Curve),
            n(p(1.0, k), NodeKind::OffCurve),
            n(p(k, 1.0), NodeKind::OffCurve),
            n(p(0.0, 1.0), NodeKind::Curve),
            n(p(-k, 1.0), NodeKind::OffCurve),
            n(p(-1.0, k), NodeKind::OffCurve),
            n(p(-1.0, 0.0), NodeKind::Curve),
            n(p(-1.0, -k), NodeKind::OffCurve),
            n(p(-k, -1.0), NodeKind::OffCurve),
            n(p(0.0, -1.0), NodeKind::Curve),
            n(p(k, -1.0), NodeKind::OffCurve),
            n(p(1.0, -k), NodeKind::OffCurve),
        ],
    )
}

/// A counter-clockwise contour mixing lines, a cubic and a quadratic segment.
pub fn mixed(id: u32) -> Path {
    closed(
        id,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(150.0, 20.0),
            off(150.0, 80.0),
            curve(100.0, 100.0),
            line(50.0, 100.0),
            off(0.0, 120.0),
            qcurve(0.0, 50.0),
        ],
    )
}

pub fn kinds(path: &Path) -> Vec<NodeKind> {
    path.nodes.iter().map(|n| n.kind).collect()
}

pub fn coords(path: &Path) -> Vec<(f64, f64)> {
    path.nodes.iter().map(|n| (n.x, n.y)).collect()
}

// ------------------------------------------------------------------ fonts

/// Adds a glyph with a layer for every master of the font.
pub fn add_glyph(
    font: &mut Font,
    name: &str,
    unicodes: &[u32],
    advance_width: f64,
    paths: Vec<Path>,
) -> GlyphId {
    add_glyph_with_id(font, GlyphId::new(), name, unicodes, advance_width, paths)
}

/// Like [`add_glyph`], with a chosen glyph id.
pub fn add_glyph_with_id(
    font: &mut Font,
    id: GlyphId,
    name: &str,
    unicodes: &[u32],
    advance_width: f64,
    paths: Vec<Path>,
) -> GlyphId {
    let layers = font
        .masters
        .iter()
        .map(|m| {
            (
                m.id,
                Layer {
                    advance_width,
                    paths: paths.clone(),
                    components: Vec::new(),
                },
            )
        })
        .collect();
    Arc::make_mut(&mut font.glyphs).insert(
        id,
        Arc::new(Glyph {
            id,
            name: name.to_owned(),
            unicodes: unicodes.to_vec(),
            export: true,
            layers,
        }),
    );
    font.glyph_order.push(id);
    id
}

/// A one-master font with `A` (a square, advance 600) and `B` (a circle,
/// advance 700).
pub fn base_font() -> Font {
    let mut font = Font::new("Test", 1000);
    add_glyph(
        &mut font,
        "A",
        &[0x41],
        600.0,
        vec![square(0, 100.0, 0.0, 500.0, 700.0, true)],
    );
    add_glyph(
        &mut font,
        "B",
        &[0x42],
        700.0,
        vec![circle(0, 350.0, 350.0, 300.0)],
    );
    font
}

/// Adds a second master named `Bold` and returns its id. Existing glyphs get a
/// copy of their default layer in it.
pub fn add_master(font: &mut Font) -> MasterId {
    let id = MasterId::new();
    font.masters.push(Master {
        id,
        name: "Bold".to_owned(),
        metrics: VerticalMetrics {
            ascender: 800.0,
            descender: -200.0,
            x_height: 500.0,
            cap_height: 700.0,
        },
    });
    let default = font.masters[0].id;
    let glyphs = Arc::make_mut(&mut font.glyphs);
    for glyph in glyphs.values_mut() {
        let glyph = Arc::make_mut(glyph);
        if let Some(layer) = glyph.layers.get(&default).cloned() {
            glyph.layers.insert(id, layer);
        }
    }
    id
}

pub fn id_of(font: &Font, name: &str) -> GlyphId {
    font.glyph_by_name(name)
        .unwrap_or_else(|| panic!("no glyph named {name}"))
        .id
}

pub fn glyph<'a>(font: &'a Font, name: &str) -> &'a Glyph {
    font.glyph_by_name(name)
        .unwrap_or_else(|| panic!("no glyph named {name}"))
}

/// The layer of a glyph in the default master.
pub fn layer<'a>(font: &'a Font, name: &str) -> &'a Layer {
    layer_of(font, name, font.default_master().id)
}

pub fn layer_of<'a>(font: &'a Font, name: &str, master: MasterId) -> &'a Layer {
    glyph(font, name)
        .layers
        .get(&master)
        .unwrap_or_else(|| panic!("glyph {name} has no layer for {master}"))
}

pub fn by_name(name: &str) -> GlyphRef {
    GlyphRef::Name(name.to_owned())
}

// --------------------------------------------------------------- commands

pub fn create(name: &str, unicodes: &[u32], advance_width: f64, paths: Vec<Path>) -> Command {
    Command::CreateGlyph {
        name: name.to_owned(),
        unicodes: unicodes.to_vec(),
        advance_width,
        paths,
    }
}

/// Applies a command that must succeed, checking the invariants of the result.
pub fn run(font: &Font, cmd: Command) -> (Font, BTreeSet<EntityKey>) {
    let (next, touched) = apply(font, &cmd).unwrap_or_else(|e| panic!("{cmd:?} failed: {e}"));
    next.validate_invariants()
        .expect("the result of apply satisfies the font invariants");
    (next, touched)
}

/// Applies a command that must fail.
pub fn fails(font: &Font, cmd: Command) -> EditError {
    match apply(font, &cmd) {
        Ok(_) => panic!("{cmd:?} unexpectedly succeeded"),
        Err(e) => e,
    }
}

pub fn keys<const N: usize>(keys: [EntityKey; N]) -> BTreeSet<EntityKey> {
    keys.into_iter().collect()
}

// --------------------------------------------------------------- geometry

pub fn bez(path: &Path) -> BezPath {
    to_bezpath(path).expect("the contour converts to a Bezier path")
}

/// The largest distance from points sampled along `a` to the curve `b`.
fn one_way_deviation(a: &BezPath, b: &BezPath) -> f64 {
    let mut worst = 0.0_f64;
    for segment in a.segments() {
        for i in 0..=100 {
            let point = segment.eval(f64::from(i) / 100.0);
            let nearest = b
                .segments()
                .map(|s| s.nearest(point, 1e-9).distance_sq)
                .fold(f64::INFINITY, f64::min);
            worst = worst.max(nearest.sqrt());
        }
    }
    worst
}

/// How far two outlines differ, sampling both against each other.
pub fn deviation(a: &Path, b: &Path) -> f64 {
    let (a, b) = (bez(a), bez(b));
    one_way_deviation(&a, &b).max(one_way_deviation(&b, &a))
}
