//! Style measures of drawn glyphs.

use std::collections::BTreeMap;
use std::sync::Arc;

use forma_geometry::{Cap, Skeleton, SkeletonNode, StrokeStyle, stroke};
use forma_model::{Font, Glyph, GlyphId, Layer, MasterId, Node, NodeKind, Path, PathId};
use forma_trace::{StyleMeasure, measure};

fn line(x: f64, y: f64) -> Node {
    Node {
        x,
        y,
        kind: NodeKind::Line,
        smooth: false,
    }
}

fn font_with(paths: Vec<Path>) -> (Font, GlyphId, MasterId) {
    let mut font = Font::new("Test", 1000);
    let master = font.masters[0].id;
    let id = GlyphId::new();
    let mut layers = BTreeMap::new();
    layers.insert(
        master,
        Layer {
            advance_width: 600.0,
            paths,
            components: Vec::new(),
        },
    );
    let glyph = Glyph {
        id,
        name: "A".to_owned(),
        unicodes: vec![0x41],
        export: true,
        layers,
    };
    Arc::make_mut(&mut font.glyphs).insert(id, Arc::new(glyph));
    font.glyph_order.push(id);
    (font, id, master)
}

fn skeleton_node(x: f64, y: f64) -> SkeletonNode {
    SkeletonNode {
        x,
        y,
        kind: NodeKind::Line,
        width: None,
    }
}

fn measure_paths(paths: Vec<Path>) -> StyleMeasure {
    let (font, glyph, master) = font_with(paths);
    measure(&font, glyph, master).unwrap()
}

#[test]
fn measure_stroke_of_bar() {
    let bar = Path {
        id: PathId(0),
        closed: true,
        nodes: vec![
            line(100.0, 0.0),
            line(180.0, 0.0),
            line(180.0, 700.0),
            line(100.0, 700.0),
        ],
    };
    let m = measure_paths(vec![bar]);
    assert!((m.stroke_median - 80.0).abs() <= 3.0, "{m:?}");
    assert!(m.stroke_p10 <= m.stroke_median && m.stroke_median <= m.stroke_p90);
    assert!((m.stroke_p90 - 80.0).abs() <= 3.0, "{m:?}");
}

#[test]
fn measure_bulb_terminal() {
    let skeleton = Skeleton {
        nodes: vec![skeleton_node(300.0, 0.0), skeleton_node(300.0, 500.0)],
        closed: false,
    };
    let style = StrokeStyle {
        width: 80.0,
        cap: Cap::Bulb { ratio: 1.5 },
    };
    let paths = stroke(&[skeleton], &style).unwrap();
    let m = measure_paths(paths);
    let r = m.terminal_radius.expect("terminals");
    assert!((r - 60.0).abs() <= 4.0, "{m:?}");
    assert!((m.stroke_median - 80.0).abs() <= 3.0, "{m:?}");
}

#[test]
fn measure_join_of_a_t() {
    // A "T" with 80-unit strokes: the largest disc at the junction touches
    // the top of the bar and both inner corners, so its radius r satisfies
    // 80 - s = r = sqrt(40² + s²) (s its height above the corners): r = 50.
    let stem = Path {
        id: PathId(0),
        closed: true,
        nodes: vec![
            line(260.0, 0.0),
            line(340.0, 0.0),
            line(340.0, 700.0),
            line(260.0, 700.0),
        ],
    };
    let bar = Path {
        id: PathId(1),
        closed: true,
        nodes: vec![
            line(0.0, 620.0),
            line(600.0, 620.0),
            line(600.0, 700.0),
            line(0.0, 700.0),
        ],
    };
    let m = measure_paths(vec![stem, bar]);
    let j = m.join_min.expect("a junction");
    assert!((j - 50.0).abs() <= 4.0, "{m:?}");
    // The square ends are terminals as wide as the stroke.
    let r = m.terminal_radius.expect("terminals");
    assert!((r - 40.0).abs() <= 3.0, "{m:?}");
}

#[test]
fn measure_ring_has_no_terminals() {
    let skeleton = Skeleton {
        nodes: vec![
            skeleton_node(300.0, 0.0),
            skeleton_node(600.0, 300.0),
            skeleton_node(300.0, 600.0),
            skeleton_node(0.0, 300.0),
        ],
        closed: true,
    };
    let style = StrokeStyle {
        width: 60.0,
        cap: Cap::Round,
    };
    let paths = stroke(&[skeleton], &style).unwrap();
    let m = measure_paths(paths);
    assert!(m.terminal_radius.is_none(), "{m:?}");
    assert!((m.stroke_median - 60.0).abs() <= 3.0, "{m:?}");
}

#[test]
fn measure_empty_glyph_is_an_error() {
    let (font, glyph, master) = font_with(Vec::new());
    assert!(measure(&font, glyph, master).is_err());
}
