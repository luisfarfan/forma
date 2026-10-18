//! Integration tests for transforms and component flattening.

use std::collections::BTreeMap;
use std::sync::Arc;

use forma_geometry::{
    GeometryError, bbox, flatten_layer, layer_bbox, signed_area, to_bezpath, transform_paths,
};
use forma_model::{
    Component, Font, Glyph, GlyphId, Layer, MasterId, Node, NodeKind, Path, PathId, Transform,
};
use kurbo::Rect;

// ---------------------------------------------------------------- helpers

fn node(x: f64, y: f64, kind: NodeKind) -> Node {
    Node {
        x,
        y,
        kind,
        smooth: false,
    }
}

fn line(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::Line)
}

fn closed(id: u32, nodes: Vec<Node>) -> Path {
    Path {
        id: PathId(id),
        closed: true,
        nodes,
    }
}

/// A counter-clockwise rectangle.
fn rect(id: u32, x0: f64, y0: f64, x1: f64, y1: f64) -> Path {
    closed(
        id,
        vec![line(x0, y0), line(x1, y0), line(x1, y1), line(x0, y1)],
    )
}

/// A clockwise rectangle (a hole).
fn hole(id: u32, x0: f64, y0: f64, x1: f64, y1: f64) -> Path {
    closed(
        id,
        vec![line(x0, y0), line(x0, y1), line(x1, y1), line(x1, y0)],
    )
}

fn translate(dx: f64, dy: f64) -> Transform {
    Transform {
        dx,
        dy,
        ..Transform::IDENTITY
    }
}

fn mirror_x(dx: f64) -> Transform {
    Transform {
        xx: -1.0,
        dx,
        ..Transform::IDENTITY
    }
}

fn component(base: GlyphId, transform: Transform) -> Component {
    Component { base, transform }
}

/// A font with one master and the given glyphs, each drawn only in that
/// master: `(name, paths, components)`.
struct Fixture {
    font: Font,
    master: MasterId,
    ids: BTreeMap<String, GlyphId>,
}

impl Fixture {
    fn new() -> Fixture {
        let font = Font::new("Test", 1000);
        let master = font.masters[0].id;
        Fixture {
            font,
            master,
            ids: BTreeMap::new(),
        }
    }

    /// Reserves the id of a glyph so that components can refer to it before
    /// it is added.
    fn id(&mut self, name: &str) -> GlyphId {
        *self.ids.entry(name.to_owned()).or_insert_with(GlyphId::new)
    }

    fn add(&mut self, name: &str, paths: Vec<Path>, components: Vec<Component>) -> GlyphId {
        let id = self.id(name);
        let mut layers = BTreeMap::new();
        layers.insert(
            self.master,
            Layer {
                advance_width: 600.0,
                paths,
                components,
            },
        );
        let glyph = Glyph {
            id,
            name: name.to_owned(),
            unicodes: Vec::new(),
            export: true,
            layers,
        };
        Arc::make_mut(&mut self.font.glyphs).insert(id, Arc::new(glyph));
        self.font.glyph_order.push(id);
        id
    }
}

fn approx_rect(actual: Rect, expected: Rect) {
    let tolerance = 1e-9;
    assert!(
        (actual.x0 - expected.x0).abs() < tolerance
            && (actual.y0 - expected.y0).abs() < tolerance
            && (actual.x1 - expected.x1).abs() < tolerance
            && (actual.y1 - expected.y1).abs() < tolerance,
        "expected {expected:?}, got {actual:?}"
    );
}

// ---------------------------------------------------------------- transforms

#[test]
fn transform_paths_moves_every_node_and_keeps_ids() {
    let paths = vec![
        rect(7, 0.0, 0.0, 100.0, 50.0),
        rect(9, 10.0, 10.0, 20.0, 20.0),
    ];
    let moved = transform_paths(&paths, &translate(5.0, -3.0));
    assert_eq!(moved.len(), 2);
    assert_eq!(moved[0].id, PathId(7));
    assert_eq!(moved[1].id, PathId(9));
    approx_rect(
        bbox(&moved).expect("bbox"),
        Rect::new(5.0, -3.0, 105.0, 47.0),
    );
    // Nothing else changes: same kinds, same flags, same order.
    assert_eq!(
        moved[0]
            .nodes
            .iter()
            .map(|n| (n.kind, n.smooth))
            .collect::<Vec<_>>(),
        paths[0]
            .nodes
            .iter()
            .map(|n| (n.kind, n.smooth))
            .collect::<Vec<_>>()
    );
}

#[test]
fn transform_paths_identity_is_a_no_op() {
    let paths = vec![rect(0, 0.0, 0.0, 100.0, 50.0)];
    assert_eq!(transform_paths(&paths, &Transform::IDENTITY), paths);
}

#[test]
fn transform_paths_scales_and_rotates_curves() {
    // A quarter turn counter-clockwise: (x, y) -> (-y, x).
    let turn = Transform {
        xx: 0.0,
        xy: 1.0,
        yx: -1.0,
        yy: 0.0,
        dx: 0.0,
        dy: 0.0,
    };
    let blob = closed(
        0,
        vec![
            line(0.0, 0.0),
            node(100.0, 0.0, NodeKind::OffCurve),
            node(100.0, 100.0, NodeKind::OffCurve),
            node(0.0, 100.0, NodeKind::Curve),
        ],
    );
    let turned = transform_paths(std::slice::from_ref(&blob), &turn);
    let before = bbox(std::slice::from_ref(&blob)).expect("bbox");
    let after = bbox(&turned).expect("bbox");
    // The box (0..~75, 0..100) turns into (-100..0, 0..~75).
    approx_rect(
        after,
        Rect::new(-before.y1, before.x0, -before.y0, before.x1),
    );
    assert!(signed_area(&turned[0]).signum() == signed_area(&blob).signum());
}

#[test]
fn transform_paths_mirror_keeps_the_direction() {
    let outer = rect(0, 0.0, 0.0, 100.0, 100.0);
    let inner = hole(1, 20.0, 20.0, 80.0, 80.0);
    let mirrored = transform_paths(&[outer.clone(), inner.clone()], &mirror_x(100.0));
    assert!(
        signed_area(&mirrored[0]) > 0.0,
        "outer stays counter-clockwise"
    );
    assert!(signed_area(&mirrored[1]) < 0.0, "hole stays clockwise");
    assert!((signed_area(&mirrored[0]) - signed_area(&outer)).abs() < 1e-6);
    assert!((signed_area(&mirrored[1]) - signed_area(&inner)).abs() < 1e-6);
    assert_eq!(mirrored[0].id, PathId(0));
    assert_eq!(mirrored[1].id, PathId(1));
    // Same shape: the mirror of a square about its own centre line.
    approx_rect(
        bbox(&mirrored).expect("bbox"),
        Rect::new(0.0, 0.0, 100.0, 100.0),
    );
}

#[test]
fn mirrored_curves_stay_valid_and_keep_their_shape() {
    let blob = closed(
        0,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            node(160.0, 20.0, NodeKind::OffCurve),
            node(160.0, 80.0, NodeKind::OffCurve),
            node(100.0, 100.0, NodeKind::Curve),
            line(0.0, 100.0),
        ],
    );
    let mirrored = transform_paths(std::slice::from_ref(&blob), &mirror_x(0.0));
    assert!(to_bezpath(&mirrored[0]).is_ok());
    let before = bbox(std::slice::from_ref(&blob)).expect("bbox");
    approx_rect(
        bbox(&mirrored).expect("bbox"),
        Rect::new(-before.x1, before.y0, -before.x0, before.y1),
    );
    assert!(signed_area(&mirrored[0]) > 0.0);
    // The area is that of the original (the mirror only reflects).
    assert!((signed_area(&mirrored[0]) - signed_area(&blob)).abs() < 1.0);
}

#[test]
fn transform_paths_leaves_open_contours_in_drawing_order() {
    let open = Path {
        id: PathId(3),
        closed: false,
        nodes: vec![line(0.0, 0.0), line(10.0, 0.0), line(10.0, 10.0)],
    };
    let mirrored = transform_paths(std::slice::from_ref(&open), &mirror_x(0.0));
    let xs: Vec<f64> = mirrored[0].nodes.iter().map(|n| n.x).collect();
    assert_eq!(xs, vec![0.0, -10.0, -10.0]);
    assert!(!mirrored[0].closed);
}

#[test]
fn transform_paths_tolerates_malformed_contours() {
    // An off-curve node with nothing after it cannot be reversed; the nodes
    // are still moved and nothing panics.
    let bad = closed(
        0,
        vec![
            line(0.0, 0.0),
            line(10.0, 0.0),
            node(10.0, 10.0, NodeKind::OffCurve),
        ],
    );
    let mirrored = transform_paths(&[bad], &mirror_x(0.0));
    assert_eq!(mirrored.len(), 1);
    assert_eq!(mirrored[0].nodes.len(), 3);
    assert_eq!(mirrored[0].nodes[1].x, -10.0);
}

// ---------------------------------------------------------------- flattening

/// `A` is a 400 x 700 rectangle with a hole; `acutecomb` a 100 x 60 box.
fn accent_fixture() -> (Fixture, GlyphId, GlyphId, GlyphId, GlyphId) {
    let mut fx = Fixture::new();
    let a = fx.add(
        "A",
        vec![
            rect(0, 50.0, 0.0, 450.0, 700.0),
            hole(1, 150.0, 100.0, 350.0, 400.0),
        ],
        vec![],
    );
    let acute = fx.add("acutecomb", vec![rect(0, 0.0, 0.0, 100.0, 60.0)], vec![]);
    let aacute = fx.add(
        "Aacute",
        vec![],
        vec![
            component(a, Transform::IDENTITY),
            component(acute, translate(200.0, 750.0)),
        ],
    );
    let aacute2 = fx.add(
        "Aacute2",
        vec![],
        vec![component(aacute, translate(10.0, 0.0))],
    );
    fx.font.validate_invariants().expect("valid fixture");
    (fx, a, acute, aacute, aacute2)
}

#[test]
fn flatten_nested_components() {
    let (fx, _, _, aacute, aacute2) = accent_fixture();

    let flat = flatten_layer(&fx.font, aacute, fx.master).expect("flatten");
    assert_eq!(flat.len(), 3, "A has two contours and the accent one");
    approx_rect(
        bbox(&flat).expect("bbox"),
        Rect::new(50.0, 0.0, 450.0, 810.0),
    );

    // Aacute2 = Aacute shifted by +10: A and the accent both move.
    let flat2 = flatten_layer(&fx.font, aacute2, fx.master).expect("flatten");
    assert_eq!(flat2.len(), 3);
    let a_box = Rect::new(60.0, 0.0, 460.0, 700.0);
    let accent_box = Rect::new(210.0, 750.0, 310.0, 810.0);
    approx_rect(bbox(&flat2).expect("bbox"), a_box.union(accent_box));
    let measured = layer_bbox(&fx.font, aacute2, fx.master)
        .expect("layer_bbox")
        .expect("not empty");
    approx_rect(measured, a_box.union(accent_box));
}

#[test]
fn flatten_renumbers_ids_from_zero_in_drawing_order() {
    let mut fx = Fixture::new();
    let acute = fx.add("acutecomb", vec![rect(5, 0.0, 0.0, 100.0, 60.0)], vec![]);
    let g = fx.add(
        "g",
        vec![
            rect(40, 0.0, 0.0, 10.0, 10.0),
            rect(41, 20.0, 0.0, 30.0, 10.0),
        ],
        vec![component(acute, translate(0.0, 100.0))],
    );
    let flat = flatten_layer(&fx.font, g, fx.master).expect("flatten");
    let ids: Vec<u32> = flat.iter().map(|p| p.id.0).collect();
    assert_eq!(ids, vec![0, 1, 2]);
    // Own contours first, then the component's.
    assert_eq!(flat[0].nodes[0].x, 0.0);
    assert_eq!(flat[1].nodes[0].x, 20.0);
    assert_eq!(flat[2].nodes[0].y, 100.0);
}

#[test]
fn flatten_without_components_returns_the_own_contours() {
    let mut fx = Fixture::new();
    let a = fx.add("A", vec![rect(9, 0.0, 0.0, 10.0, 10.0)], vec![]);
    let flat = flatten_layer(&fx.font, a, fx.master).expect("flatten");
    assert_eq!(flat.len(), 1);
    assert_eq!(flat[0].id, PathId(0));
    assert_eq!(flat[0].nodes, rect(0, 0.0, 0.0, 10.0, 10.0).nodes);
}

#[test]
fn flatten_mirror_component_keeps_the_exterior_counter_clockwise() {
    let mut fx = Fixture::new();
    let a = fx.add(
        "A",
        vec![
            rect(0, 50.0, 0.0, 450.0, 700.0),
            hole(1, 150.0, 100.0, 350.0, 400.0),
        ],
        vec![],
    );
    // xx = -1 mirrors about the vertical axis; dx = 500 puts it back in place.
    let mirrored = fx.add("A.mirror", vec![], vec![component(a, mirror_x(500.0))]);
    let flat = flatten_layer(&fx.font, mirrored, fx.master).expect("flatten");
    assert_eq!(flat.len(), 2);
    assert!(
        signed_area(&flat[0]) > 0.0,
        "exterior stays counter-clockwise"
    );
    assert!(signed_area(&flat[1]) < 0.0, "hole stays clockwise");
    approx_rect(
        bbox(&flat).expect("bbox"),
        Rect::new(50.0, 0.0, 450.0, 700.0),
    );
}

#[test]
fn flatten_composes_nested_mirrors_without_reversing_twice() {
    let mut fx = Fixture::new();
    let a = fx.add("A", vec![rect(0, 0.0, 0.0, 100.0, 50.0)], vec![]);
    let m1 = fx.add("m1", vec![], vec![component(a, mirror_x(0.0))]);
    let m2 = fx.add("m2", vec![], vec![component(m1, mirror_x(0.0))]);
    let flat = flatten_layer(&fx.font, m2, fx.master).expect("flatten");
    assert_eq!(flat.len(), 1);
    // Two mirrors give the identity: same nodes in the same order.
    assert_eq!(flat[0].nodes, rect(0, 0.0, 0.0, 100.0, 50.0).nodes);
}

#[test]
fn flatten_applies_the_component_transform_before_the_parent_one() {
    let mut fx = Fixture::new();
    let a = fx.add("A", vec![rect(0, 0.0, 0.0, 10.0, 10.0)], vec![]);
    // Inner: move by (100, 0). Outer: scale by 2. The result is the point
    // (0..10) -> (100..110) -> (200..220).
    let inner = fx.add("inner", vec![], vec![component(a, translate(100.0, 0.0))]);
    let scale = Transform {
        xx: 2.0,
        yy: 2.0,
        ..Transform::IDENTITY
    };
    let outer = fx.add("outer", vec![], vec![component(inner, scale)]);
    let measured = layer_bbox(&fx.font, outer, fx.master)
        .expect("layer_bbox")
        .expect("not empty");
    approx_rect(measured, Rect::new(200.0, 0.0, 220.0, 20.0));
}

#[test]
fn flatten_repeats_a_base_used_twice() {
    let mut fx = Fixture::new();
    let dot = fx.add("dot", vec![rect(0, 0.0, 0.0, 10.0, 10.0)], vec![]);
    let colon = fx.add(
        "colon",
        vec![],
        vec![
            component(dot, Transform::IDENTITY),
            component(dot, translate(0.0, 100.0)),
        ],
    );
    let flat = flatten_layer(&fx.font, colon, fx.master).expect("flatten");
    assert_eq!(flat.len(), 2);
    approx_rect(bbox(&flat).expect("bbox"), Rect::new(0.0, 0.0, 10.0, 110.0));
}

#[test]
fn flatten_rejects_a_missing_glyph() {
    let fx = Fixture::new();
    let missing = GlyphId::new();
    assert!(matches!(
        flatten_layer(&fx.font, missing, fx.master),
        Err(GeometryError::InvalidPath(_))
    ));
}

#[test]
fn flatten_rejects_a_missing_layer() {
    let mut fx = Fixture::new();
    let a = fx.add("A", vec![rect(0, 0.0, 0.0, 10.0, 10.0)], vec![]);
    let other_master = MasterId::new();
    assert!(matches!(
        flatten_layer(&fx.font, a, other_master),
        Err(GeometryError::InvalidPath(_))
    ));
    assert!(matches!(
        layer_bbox(&fx.font, a, other_master),
        Err(GeometryError::InvalidPath(_))
    ));
}

#[test]
fn flatten_rejects_a_component_whose_base_is_missing() {
    let mut fx = Fixture::new();
    let ghost = GlyphId::new();
    let g = fx.add("g", vec![], vec![component(ghost, Transform::IDENTITY)]);
    assert!(matches!(
        flatten_layer(&fx.font, g, fx.master),
        Err(GeometryError::InvalidPath(_))
    ));
}

#[test]
fn flatten_rejects_a_component_whose_base_has_no_layer_in_the_master() {
    let mut fx = Fixture::new();
    let base = fx.add("base", vec![rect(0, 0.0, 0.0, 10.0, 10.0)], vec![]);
    let g = fx.add("g", vec![], vec![component(base, Transform::IDENTITY)]);
    // Remove the base's only layer.
    {
        let glyphs = Arc::make_mut(&mut fx.font.glyphs);
        let glyph = glyphs.get_mut(&base).expect("base");
        Arc::make_mut(glyph).layers.clear();
    }
    assert!(matches!(
        flatten_layer(&fx.font, g, fx.master),
        Err(GeometryError::InvalidPath(_))
    ));
}

#[test]
fn flatten_rejects_cycles_instead_of_looping() {
    let mut fx = Fixture::new();
    let a_id = fx.id("a");
    let b_id = fx.id("b");
    fx.add("a", vec![], vec![component(b_id, Transform::IDENTITY)]);
    fx.add("b", vec![], vec![component(a_id, Transform::IDENTITY)]);
    // The font is invalid (the model rejects cycles), but flattening must
    // still terminate.
    assert!(fx.font.validate_invariants().is_err());
    assert!(matches!(
        flatten_layer(&fx.font, a_id, fx.master),
        Err(GeometryError::InvalidPath(_))
    ));
}

#[test]
fn flatten_rejects_a_glyph_that_contains_itself() {
    let mut fx = Fixture::new();
    let a_id = fx.id("a");
    fx.add(
        "a",
        vec![rect(0, 0.0, 0.0, 10.0, 10.0)],
        vec![component(a_id, translate(1.0, 0.0))],
    );
    assert!(matches!(
        flatten_layer(&fx.font, a_id, fx.master),
        Err(GeometryError::InvalidPath(_))
    ));
}

#[test]
fn flatten_rejects_an_invalid_transform() {
    let mut fx = Fixture::new();
    let base = fx.add("base", vec![rect(0, 0.0, 0.0, 10.0, 10.0)], vec![]);
    let flat = Transform {
        yy: 0.0,
        ..Transform::IDENTITY
    };
    let g = fx.add("g", vec![], vec![component(base, flat)]);
    assert!(matches!(
        flatten_layer(&fx.font, g, fx.master),
        Err(GeometryError::InvalidPath(_))
    ));
    let nan = Transform {
        dx: f64::NAN,
        ..Transform::IDENTITY
    };
    let h = fx.add("h", vec![], vec![component(base, nan)]);
    assert!(matches!(
        flatten_layer(&fx.font, h, fx.master),
        Err(GeometryError::InvalidPath(_))
    ));
}

#[test]
fn flatten_stops_a_chain_that_is_too_deep() {
    // Not a cycle, just absurdly deep: it must fail cleanly, not overflow the
    // stack.
    let mut fx = Fixture::new();
    let depth = 5_000;
    let ids: Vec<GlyphId> = (0..depth).map(|i| fx.id(&format!("g{i}"))).collect();
    for i in 0..depth {
        let components = ids
            .get(i + 1)
            .map(|&next| vec![component(next, Transform::IDENTITY)])
            .unwrap_or_default();
        let paths = if i + 1 == depth {
            vec![rect(0, 0.0, 0.0, 10.0, 10.0)]
        } else {
            vec![]
        };
        fx.add(&format!("g{i}"), paths, components);
    }
    assert!(matches!(
        flatten_layer(&fx.font, ids[0], fx.master),
        Err(GeometryError::InvalidPath(_))
    ));
}

#[test]
fn flatten_stops_an_exponential_expansion() {
    // Each level uses the next one twice: 2^40 leaves without a cycle.
    let mut fx = Fixture::new();
    let levels = 40;
    let ids: Vec<GlyphId> = (0..levels).map(|i| fx.id(&format!("l{i}"))).collect();
    for i in 0..levels {
        let components = ids
            .get(i + 1)
            .map(|&next| {
                vec![
                    component(next, Transform::IDENTITY),
                    component(next, translate(1.0, 0.0)),
                ]
            })
            .unwrap_or_default();
        let paths = if i + 1 == levels {
            vec![rect(0, 0.0, 0.0, 10.0, 10.0)]
        } else {
            vec![]
        };
        fx.add(&format!("l{i}"), paths, components);
    }
    assert!(matches!(
        flatten_layer(&fx.font, ids[0], fx.master),
        Err(GeometryError::InvalidPath(_))
    ));
}

#[test]
fn layer_bbox_of_an_empty_layer_is_none() {
    let mut fx = Fixture::new();
    let space = fx.add("space", vec![], vec![]);
    assert_eq!(layer_bbox(&fx.font, space, fx.master), Ok(None));
}
