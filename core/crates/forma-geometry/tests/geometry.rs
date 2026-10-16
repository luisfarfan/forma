//! Integration tests for `forma-geometry`.

use forma_geometry::{
    CoordinateSystem, GeometryError, bbox, contour_depths, from_bezpath, normalize_direction,
    path_to_svg, paths_from_svg, signed_area, to_bezpath,
};
use forma_model::{Node, NodeKind, Path, PathId};
use kurbo::{BezPath, PathEl, PathSeg, Point, Rect};

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

fn off(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::OffCurve)
}

fn curve(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::Curve)
}

fn qcurve(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::QCurve)
}

fn closed(id: u32, nodes: Vec<Node>) -> Path {
    Path {
        id: PathId(id),
        closed: true,
        nodes,
    }
}

fn open(id: u32, nodes: Vec<Node>) -> Path {
    Path {
        id: PathId(id),
        closed: false,
        nodes,
    }
}

/// An axis-aligned square; counter-clockwise when `ccw` is true.
fn square(id: u32, x0: f64, y0: f64, x1: f64, y1: f64, ccw: bool) -> Path {
    let nodes = if ccw {
        vec![line(x0, y0), line(x1, y0), line(x1, y1), line(x0, y1)]
    } else {
        vec![line(x0, y0), line(x0, y1), line(x1, y1), line(x1, y0)]
    };
    closed(id, nodes)
}

const KAPPA: f64 = 0.552_284_749_830_793_4;

/// A circle of four cubic segments starting at `(cx + r, cy)`. The first node
/// is a `Curve` whose two off-curve nodes sit at the end of the list.
fn circle(id: u32, cx: f64, cy: f64, r: f64, ccw: bool) -> Path {
    let s = if ccw { 1.0 } else { -1.0 };
    let p = |dx: f64, dy: f64| (cx + dx * r, cy + s * dy * r);
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

/// A contour mixing lines, a cubic and a quadratic segment, counter-clockwise.
fn mixed(id: u32) -> Path {
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

fn kinds(path: &Path) -> Vec<NodeKind> {
    path.nodes.iter().map(|n| n.kind).collect()
}

fn coords(path: &Path) -> Vec<(f64, f64)> {
    path.nodes.iter().map(|n| (n.x, n.y)).collect()
}

fn assert_invalid(result: Result<BezPath, GeometryError>) {
    assert!(
        matches!(result, Err(GeometryError::InvalidPath(_))),
        "expected InvalidPath, got {result:?}"
    );
}

fn assert_degenerate(result: Result<BezPath, GeometryError>) {
    assert!(
        matches!(result, Err(GeometryError::Degenerate(_))),
        "expected Degenerate, got {result:?}"
    );
}

fn assert_rect_close(actual: Rect, expected: Rect) {
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    assert!(
        close(actual.x0, expected.x0)
            && close(actual.y0, expected.y0)
            && close(actual.x1, expected.x1)
            && close(actual.y1, expected.y1),
        "expected {expected:?}, got {actual:?}"
    );
}

fn quantize(p: Point) -> [i64; 2] {
    // Truncation is fine: values are rounded to micro-units first.
    [(p.x * 1e6).round() as i64, (p.y * 1e6).round() as i64]
}

/// The segments of a contour as direction-independent sorted point lists, so
/// that a contour and its reverse compare equal.
fn undirected_segments(path: &Path) -> Vec<Vec<[i64; 2]>> {
    let bez = to_bezpath(path).expect("valid contour");
    let mut out: Vec<Vec<[i64; 2]>> = bez
        .segments()
        .map(|seg| {
            let pts: Vec<Point> = match seg {
                PathSeg::Line(l) => vec![l.p0, l.p1],
                PathSeg::Quad(q) => vec![q.p0, q.p1, q.p2],
                PathSeg::Cubic(c) => vec![c.p0, c.p1, c.p2, c.p3],
            };
            let forward: Vec<[i64; 2]> = pts.iter().copied().map(quantize).collect();
            let mut backward = forward.clone();
            backward.reverse();
            forward.min(backward)
        })
        .collect();
    out.sort();
    out
}

// ------------------------------------------------------- brief's test names

#[test]
fn square_roundtrip_bezpath() {
    let path = square(7, 0.0, 0.0, 100.0, 100.0, true);
    let bez = to_bezpath(&path).expect("square converts");
    assert_eq!(
        bez.elements(),
        &[
            PathEl::MoveTo(Point::new(0.0, 0.0)),
            PathEl::LineTo(Point::new(100.0, 0.0)),
            PathEl::LineTo(Point::new(100.0, 100.0)),
            PathEl::LineTo(Point::new(0.0, 100.0)),
            PathEl::ClosePath,
        ]
    );

    let back = from_bezpath(&bez, 7).expect("square converts back");
    assert_eq!(back, vec![path]);
}

#[test]
fn svg_square_font_coords() {
    let paths =
        paths_from_svg("M0 0 L100 0 L100 100 L0 100 Z", CoordinateSystem::Font).expect("valid svg");
    assert_eq!(paths.len(), 1);
    let path = &paths[0];
    assert!(path.closed);
    assert_eq!(path.nodes.len(), 4);
    assert!(path.nodes.iter().all(|n| n.kind == NodeKind::Line));
    assert_eq!(
        coords(path),
        vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
    );
    assert!(signed_area(path) > 0.0);
}

#[test]
fn svg_ydown_flips() {
    let paths =
        paths_from_svg("M0 0", CoordinateSystem::SvgYDown { ascender: 800.0 }).expect("valid svg");
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].nodes.len(), 1);
    assert_eq!(paths[0].nodes[0].x, 0.0);
    assert_eq!(paths[0].nodes[0].y, 800.0);

    let paths = paths_from_svg(
        "M10 100 L20 300",
        CoordinateSystem::SvgYDown { ascender: 800.0 },
    )
    .expect("valid svg");
    assert_eq!(coords(&paths[0]), vec![(10.0, 700.0), (20.0, 500.0)]);
}

#[test]
fn svg_relative_and_arc_commands() {
    let paths = paths_from_svg("m10 10 h50 a20 20 0 0 1 20 20 z", CoordinateSystem::Font)
        .expect("valid svg");
    assert_eq!(paths.len(), 1);
    let path = &paths[0];
    assert!(path.closed);
    assert!(path.nodes.iter().all(|n| matches!(
        n.kind,
        NodeKind::Line | NodeKind::Curve | NodeKind::OffCurve
    )));
    assert!(path.nodes.iter().any(|n| n.kind == NodeKind::Curve));
    assert_eq!((path.nodes[0].x, path.nodes[0].y), (10.0, 10.0));
    assert_eq!((path.nodes[1].x, path.nodes[1].y), (60.0, 10.0));
    let last = path.nodes.last().expect("has nodes");
    assert_eq!(last.kind, NodeKind::Curve);
    assert!((last.x - 80.0).abs() < 1e-9 && (last.y - 30.0).abs() < 1e-9);
    // The imported contour is structurally valid.
    assert!(to_bezpath(path).is_ok());
}

#[test]
fn svg_invalid_is_error() {
    for d in ["M0 0 Q", "L10 10", "M0 0 X", "M0 0 L10", "M0 0 C1 2 3 4 5"] {
        let result = paths_from_svg(d, CoordinateSystem::Font);
        assert!(
            matches!(result, Err(GeometryError::InvalidPath(_))),
            "{d:?} should be InvalidPath, got {result:?}"
        );
    }
}

#[test]
fn o_shape_outer_ccw_inner_cw() {
    // Both circles are drawn the wrong way round: outer clockwise, inner
    // counter-clockwise.
    let mut paths = vec![
        circle(0, 500.0, 350.0, 300.0, false),
        circle(1, 500.0, 350.0, 150.0, true),
    ];
    assert!(signed_area(&paths[0]) < 0.0);
    assert!(signed_area(&paths[1]) > 0.0);
    let before_segments: Vec<_> = paths.iter().map(undirected_segments).collect();
    let before_bbox = bbox(&paths);

    normalize_direction(&mut paths);

    assert!(
        signed_area(&paths[0]) > 0.0,
        "outer must be counter-clockwise"
    );
    assert!(signed_area(&paths[1]) < 0.0, "inner must be clockwise");
    // The geometry itself did not change.
    let after_segments: Vec<_> = paths.iter().map(undirected_segments).collect();
    assert_eq!(before_segments, after_segments);
    assert_rect_close(
        bbox(&paths).expect("has a box"),
        before_bbox.expect("has a box"),
    );
    // Ids are preserved.
    assert_eq!(paths[0].id, PathId(0));
    assert_eq!(paths[1].id, PathId(1));
}

#[test]
fn degenerate_single_node_rejected() {
    assert_degenerate(to_bezpath(&closed(0, vec![line(5.0, 5.0)])));
    assert_degenerate(to_bezpath(&closed(0, vec![])));
    assert_degenerate(to_bezpath(&open(0, vec![])));
    // A single on-curve node with handles is still not a closed shape.
    assert_degenerate(to_bezpath(&closed(
        0,
        vec![curve(0.0, 0.0), off(10.0, 0.0), off(10.0, 10.0)],
    )));
    // An import that closes a single point is rejected too.
    assert!(matches!(
        paths_from_svg("M5 5 Z", CoordinateSystem::Font),
        Err(GeometryError::Degenerate(_))
    ));
}

#[test]
fn bbox_of_square() {
    let paths = vec![square(0, 0.0, 0.0, 100.0, 100.0, true)];
    assert_eq!(bbox(&paths), Some(Rect::new(0.0, 0.0, 100.0, 100.0)));
}

// ---------------------------------------------------------- bbox, extras

#[test]
fn bbox_of_nothing_is_none() {
    assert_eq!(bbox(&[]), None);
}

#[test]
fn bbox_is_the_union_and_uses_curve_extrema() {
    let bulge = open(
        0,
        vec![
            line(0.0, 0.0),
            off(0.0, 100.0),
            off(100.0, 100.0),
            curve(100.0, 0.0),
        ],
    );
    let far = square(1, 200.0, -50.0, 300.0, -10.0, true);
    // The cubic peaks at y = 75, well below its control points.
    let alone = bbox(std::slice::from_ref(&bulge)).expect("has a box");
    assert_rect_close(alone, Rect::new(0.0, 0.0, 100.0, 75.0));
    let both = bbox(&[bulge, far]).expect("has a box");
    assert_rect_close(both, Rect::new(0.0, -50.0, 300.0, 75.0));
}

#[test]
fn bbox_of_a_lone_point_and_skipping_invalid_paths() {
    let lone = open(0, vec![line(10.0, 20.0)]);
    assert_eq!(
        bbox(std::slice::from_ref(&lone)),
        Some(Rect::new(10.0, 20.0, 10.0, 20.0))
    );
    let broken = closed(1, vec![line(1000.0, 1000.0)]);
    assert_eq!(
        bbox(&[broken, lone]),
        Some(Rect::new(10.0, 20.0, 10.0, 20.0))
    );
    assert_eq!(bbox(&[closed(0, vec![])]), None);
}

// ------------------------------------------------------- to_bezpath, extras

#[test]
fn to_bezpath_cubic_and_quadratic_segments() {
    let bez = to_bezpath(&mixed(0)).expect("valid");
    assert_eq!(
        bez.elements(),
        &[
            PathEl::MoveTo(Point::new(0.0, 0.0)),
            PathEl::LineTo(Point::new(100.0, 0.0)),
            PathEl::CurveTo(
                Point::new(150.0, 20.0),
                Point::new(150.0, 80.0),
                Point::new(100.0, 100.0)
            ),
            PathEl::LineTo(Point::new(50.0, 100.0)),
            PathEl::QuadTo(Point::new(0.0, 120.0), Point::new(0.0, 50.0)),
            PathEl::ClosePath,
        ]
    );
}

#[test]
fn to_bezpath_closing_curve_uses_trailing_offcurves() {
    // The first node is a Curve: the segment arriving at it is drawn last.
    let path = circle(0, 0.0, 0.0, 100.0, true);
    let bez = to_bezpath(&path).expect("valid");
    assert_eq!(bez.elements().len(), 1 + 4 + 1);
    assert_eq!(bez.elements()[0], PathEl::MoveTo(Point::new(100.0, 0.0)));
    assert!(matches!(
        bez.elements()[4],
        PathEl::CurveTo(_, _, p) if p == Point::new(100.0, 0.0)
    ));
    assert_eq!(bez.elements()[5], PathEl::ClosePath);
}

#[test]
fn to_bezpath_closed_contour_may_start_with_offcurves() {
    // The same contour as `mixed`'s quadratic corner, rotated so that the
    // list begins with the off-curve node leading into the first on-curve.
    let path = closed(
        0,
        vec![
            off(0.0, 120.0),
            qcurve(0.0, 50.0),
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(150.0, 20.0),
            off(150.0, 80.0),
            curve(100.0, 100.0),
            line(50.0, 100.0),
        ],
    );
    let bez = to_bezpath(&path).expect("valid");
    assert_eq!(
        bez.elements(),
        &[
            PathEl::MoveTo(Point::new(0.0, 50.0)),
            PathEl::LineTo(Point::new(0.0, 0.0)),
            PathEl::LineTo(Point::new(100.0, 0.0)),
            PathEl::CurveTo(
                Point::new(150.0, 20.0),
                Point::new(150.0, 80.0),
                Point::new(100.0, 100.0)
            ),
            PathEl::LineTo(Point::new(50.0, 100.0)),
            PathEl::QuadTo(Point::new(0.0, 120.0), Point::new(0.0, 50.0)),
            PathEl::ClosePath,
        ]
    );
    // Same geometry as the unrotated contour.
    assert_eq!(undirected_segments(&path), undirected_segments(&mixed(0)));
}

#[test]
fn to_bezpath_open_contour_first_node_is_a_move_to() {
    // Whatever its kind, the first node of an open contour is a move-to.
    let path = open(0, vec![curve(1.0, 2.0), line(3.0, 4.0)]);
    let bez = to_bezpath(&path).expect("valid");
    assert_eq!(
        bez.elements(),
        &[
            PathEl::MoveTo(Point::new(1.0, 2.0)),
            PathEl::LineTo(Point::new(3.0, 4.0)),
        ]
    );
}

#[test]
fn to_bezpath_rejects_unterminated_offcurves() {
    // Open contour ending on an off-curve node.
    assert_invalid(to_bezpath(&open(
        0,
        vec![line(0.0, 0.0), line(10.0, 0.0), off(20.0, 0.0)],
    )));
    // Open contour starting with an off-curve node.
    assert_invalid(to_bezpath(&open(
        0,
        vec![off(0.0, 0.0), line(10.0, 0.0), line(20.0, 0.0)],
    )));
    // Closed contour whose trailing off-curve leads into a Line.
    assert_invalid(to_bezpath(&closed(
        0,
        vec![
            line(0.0, 0.0),
            line(10.0, 0.0),
            line(10.0, 10.0),
            off(0.0, 10.0),
        ],
    )));
    // Off-curve nodes in front of a Line.
    assert_invalid(to_bezpath(&open(
        0,
        vec![line(0.0, 0.0), off(5.0, 5.0), line(10.0, 0.0)],
    )));
}

#[test]
fn to_bezpath_rejects_wrong_offcurve_counts() {
    // Three off-curve nodes before a Curve.
    assert_invalid(to_bezpath(&open(
        0,
        vec![
            line(0.0, 0.0),
            off(1.0, 1.0),
            off(2.0, 2.0),
            off(3.0, 3.0),
            curve(4.0, 0.0),
        ],
    )));
    // A Curve with one handle, or none, is ambiguous and rejected.
    assert_invalid(to_bezpath(&open(
        0,
        vec![line(0.0, 0.0), off(1.0, 1.0), curve(4.0, 0.0)],
    )));
    assert_invalid(to_bezpath(&open(0, vec![line(0.0, 0.0), curve(4.0, 0.0)])));
    // Several off-curve nodes before a QCurve imply on-curve points, which is
    // not supported yet; it must not be silently misread.
    assert_invalid(to_bezpath(&open(
        0,
        vec![
            line(0.0, 0.0),
            off(1.0, 1.0),
            off(2.0, 2.0),
            qcurve(4.0, 0.0),
        ],
    )));
    assert_invalid(to_bezpath(&open(0, vec![line(0.0, 0.0), qcurve(4.0, 0.0)])));
}

#[test]
fn to_bezpath_rejects_non_finite_coordinates() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_invalid(to_bezpath(&closed(
            0,
            vec![line(0.0, 0.0), line(bad, 0.0), line(10.0, 10.0)],
        )));
        assert_invalid(to_bezpath(&open(0, vec![line(0.0, bad)])));
        // A control point counts too.
        assert_invalid(to_bezpath(&open(
            0,
            vec![
                line(0.0, 0.0),
                off(bad, 1.0),
                off(2.0, 2.0),
                curve(4.0, 0.0),
            ],
        )));
    }
}

#[test]
fn to_bezpath_accepts_a_lone_open_node() {
    let bez = to_bezpath(&open(0, vec![line(3.0, 4.0)])).expect("valid");
    assert_eq!(bez.elements(), &[PathEl::MoveTo(Point::new(3.0, 4.0))]);
}

// ----------------------------------------------------- from_bezpath, extras

#[test]
fn from_bezpath_roundtrips_canonical_contours() {
    for path in [
        mixed(3),
        circle(3, 10.0, 20.0, 50.0, true),
        square(3, 0.0, 0.0, 5.0, 5.0, false),
    ] {
        let bez = to_bezpath(&path).expect("valid");
        let back = from_bezpath(&bez, 3).expect("valid");
        assert_eq!(back, vec![path]);
    }
}

#[test]
fn from_bezpath_assigns_sequential_ids_per_subpath() {
    let mut bez = BezPath::new();
    bez.move_to((0.0, 0.0));
    bez.line_to((10.0, 0.0));
    bez.line_to((10.0, 10.0));
    bez.close_path();
    bez.move_to((20.0, 20.0));
    bez.line_to((30.0, 30.0));
    bez.move_to((40.0, 40.0));
    bez.quad_to((50.0, 60.0), (60.0, 40.0));
    bez.line_to((40.0, 30.0));
    bez.close_path();

    let paths = from_bezpath(&bez, 5).expect("valid");
    assert_eq!(paths.len(), 3);
    assert_eq!(
        paths.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![PathId(5), PathId(6), PathId(7)]
    );
    assert_eq!(
        paths.iter().map(|p| p.closed).collect::<Vec<_>>(),
        vec![true, false, true]
    );
    assert_eq!(paths[1].nodes.len(), 2);
    assert_eq!(
        kinds(&paths[2]),
        vec![
            NodeKind::Line,
            NodeKind::OffCurve,
            NodeKind::QCurve,
            NodeKind::Line
        ]
    );
    assert!(paths.iter().flat_map(|p| &p.nodes).all(|n| !n.smooth));
}

#[test]
fn from_bezpath_drops_a_closing_node_on_the_start_point() {
    // A line back to the start before the close: no duplicate node.
    let mut bez = BezPath::new();
    bez.move_to((0.0, 0.0));
    bez.line_to((100.0, 0.0));
    bez.line_to((100.0, 100.0));
    bez.line_to((0.0, 0.0));
    bez.close_path();
    let paths = from_bezpath(&bez, 0).expect("valid");
    assert_eq!(
        coords(&paths[0]),
        vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)]
    );
    assert!(paths[0].nodes.iter().all(|n| n.kind == NodeKind::Line));

    // A curve back to the start: its kind moves to the first node and its
    // handles stay at the end of the list.
    let mut bez = BezPath::new();
    bez.move_to((0.0, 0.0));
    bez.line_to((100.0, 0.0));
    bez.curve_to((100.0, 60.0), (60.0, 100.0), (0.0, 0.0));
    bez.close_path();
    let paths = from_bezpath(&bez, 0).expect("valid");
    assert_eq!(
        kinds(&paths[0]),
        vec![
            NodeKind::Curve,
            NodeKind::Line,
            NodeKind::OffCurve,
            NodeKind::OffCurve
        ]
    );
    // ... and the geometry survives the round trip through the model.
    let again = to_bezpath(&paths[0]).expect("valid");
    assert_eq!(
        again.elements(),
        &[
            PathEl::MoveTo(Point::new(0.0, 0.0)),
            PathEl::LineTo(Point::new(100.0, 0.0)),
            PathEl::CurveTo(
                Point::new(100.0, 60.0),
                Point::new(60.0, 100.0),
                Point::new(0.0, 0.0)
            ),
            PathEl::ClosePath,
        ]
    );

    // An open path is never trimmed.
    let mut bez = BezPath::new();
    bez.move_to((0.0, 0.0));
    bez.line_to((10.0, 0.0));
    bez.line_to((0.0, 0.0));
    let paths = from_bezpath(&bez, 0).expect("valid");
    assert!(!paths[0].closed);
    assert_eq!(paths[0].nodes.len(), 3);
}

#[test]
fn from_bezpath_empty_and_error_cases() {
    assert_eq!(from_bezpath(&BezPath::new(), 0), Ok(vec![]));

    // Drawing after a close, without a new move-to.
    let mut bez = BezPath::new();
    bez.move_to((0.0, 0.0));
    bez.line_to((10.0, 0.0));
    bez.close_path();
    bez.line_to((10.0, 10.0));
    assert!(matches!(
        from_bezpath(&bez, 0),
        Err(GeometryError::InvalidPath(_))
    ));
    // A close with nothing open.
    let mut bez = BezPath::new();
    bez.move_to((0.0, 0.0));
    bez.line_to((10.0, 0.0));
    bez.close_path();
    bez.close_path();
    assert!(matches!(
        from_bezpath(&bez, 0),
        Err(GeometryError::InvalidPath(_))
    ));
    // Non-finite coordinates.
    let bez = BezPath::from_vec(vec![
        PathEl::MoveTo(Point::new(0.0, 0.0)),
        PathEl::LineTo(Point::new(f64::NAN, 1.0)),
    ]);
    assert!(matches!(
        from_bezpath(&bez, 0),
        Err(GeometryError::InvalidPath(_))
    ));
    // A closed lone point.
    let bez = BezPath::from_vec(vec![
        PathEl::MoveTo(Point::new(0.0, 0.0)),
        PathEl::ClosePath,
    ]);
    assert!(matches!(
        from_bezpath(&bez, 0),
        Err(GeometryError::Degenerate(_))
    ));
}

// ------------------------------------------------------------- SVG, extras

#[test]
fn svg_does_not_normalize_direction() {
    // Clockwise in font coordinates; must be preserved as given.
    let paths =
        paths_from_svg("M0 0 L0 100 L100 100 L100 0 Z", CoordinateSystem::Font).expect("valid svg");
    assert_eq!(
        coords(&paths[0]),
        vec![(0.0, 0.0), (0.0, 100.0), (100.0, 100.0), (100.0, 0.0)]
    );
    assert!(signed_area(&paths[0]) < 0.0);
}

#[test]
fn svg_ydown_flip_mirrors_orientation() {
    // Flipping y mirrors the orientation: a contour that is counter-clockwise
    // as written ends up clockwise in font coordinates. The import never
    // re-orients anything on its own.
    let d = "M0 0 L100 0 L100 100 L0 100 Z";
    let font = paths_from_svg(d, CoordinateSystem::Font).expect("valid svg");
    let flipped =
        paths_from_svg(d, CoordinateSystem::SvgYDown { ascender: 800.0 }).expect("valid svg");
    assert!(signed_area(&font[0]) > 0.0);
    assert!(signed_area(&flipped[0]) < 0.0);
    assert_eq!(
        coords(&flipped[0]),
        vec![(0.0, 800.0), (100.0, 800.0), (100.0, 700.0), (0.0, 700.0)]
    );
}

#[test]
fn svg_empty_data_is_an_empty_outline() {
    assert_eq!(paths_from_svg("", CoordinateSystem::Font), Ok(vec![]));
    assert_eq!(paths_from_svg("  \n", CoordinateSystem::Font), Ok(vec![]));
}

#[test]
fn svg_multiple_subpaths_get_sequential_ids_from_zero() {
    let paths = paths_from_svg(
        "M0 0 L10 0 L10 10 Z M20 20 L30 20 L30 30 Z m-5 -5 l1 1",
        CoordinateSystem::Font,
    )
    .expect("valid svg");
    assert_eq!(paths.len(), 3);
    assert_eq!(
        paths.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![PathId(0), PathId(1), PathId(2)]
    );
    assert!(paths[0].closed && paths[1].closed && !paths[2].closed);
}

#[test]
fn svg_curves_and_quadratics_map_to_the_model_convention() {
    let paths = paths_from_svg(
        "M0 0 L100 0 C100 50 50 100 0 100 Q-50 50 0 0 Z",
        CoordinateSystem::Font,
    )
    .expect("valid svg");
    let path = &paths[0];
    // The closing quadratic ends on the start point: the start node becomes
    // the QCurve and its handle goes to the end of the list.
    assert_eq!(
        kinds(path),
        vec![
            NodeKind::QCurve,
            NodeKind::Line,
            NodeKind::OffCurve,
            NodeKind::OffCurve,
            NodeKind::Curve,
            NodeKind::OffCurve,
        ]
    );
    assert!(to_bezpath(path).is_ok());
}

#[test]
fn svg_rejects_non_finite_results() {
    for ascender in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let result = paths_from_svg("M0 0 L1 1", CoordinateSystem::SvgYDown { ascender });
        assert!(
            matches!(result, Err(GeometryError::InvalidPath(_))),
            "ascender {ascender} should be rejected, got {result:?}"
        );
    }
    let result = paths_from_svg("M0 0 L1e999 1 L5 5 Z", CoordinateSystem::Font);
    assert!(matches!(result, Err(GeometryError::InvalidPath(_))));
}

// ----------------------------------------------------------------- to SVG

#[test]
fn path_to_svg_formats_numbers_without_trailing_zeros() {
    let path = closed(
        0,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 12.5),
            line(-0.0, 100.0),
        ],
    );
    assert_eq!(path_to_svg(&path), "M0 0 L100 0 L100 12.5 L0 100 Z");
}

#[test]
fn path_to_svg_curves_quads_and_open_contours() {
    assert_eq!(
        path_to_svg(&mixed(0)),
        "M0 0 L100 0 C150 20 150 80 100 100 L50 100 Q0 120 0 50 Z"
    );
    let open_path = open(0, vec![line(1.0, 2.0), line(3.5, -4.25)]);
    assert_eq!(path_to_svg(&open_path), "M1 2 L3.5 -4.25");
}

#[test]
fn path_to_svg_of_an_unrepresentable_contour_is_empty() {
    assert_eq!(path_to_svg(&closed(0, vec![line(1.0, 1.0)])), "");
    assert_eq!(path_to_svg(&open(0, vec![line(f64::NAN, 1.0)])), "");
}

#[test]
fn svg_roundtrips_through_path_to_svg() {
    for path in [
        mixed(0),
        circle(0, 10.5, 20.25, 50.0, true),
        square(0, 0.0, 0.0, 5.0, 5.0, false),
    ] {
        let d = path_to_svg(&path);
        let back = paths_from_svg(&d, CoordinateSystem::Font).expect("valid svg");
        assert_eq!(back.len(), 1);
        assert_eq!(
            undirected_segments(&back[0]),
            undirected_segments(&path),
            "{d}"
        );
        assert_eq!(back[0].closed, path.closed);
    }
}

// ------------------------------------------------------------ signed area

#[test]
fn signed_area_sign_follows_direction() {
    let ccw = square(0, 0.0, 0.0, 100.0, 100.0, true);
    let cw = square(0, 0.0, 0.0, 100.0, 100.0, false);
    assert_eq!(signed_area(&ccw), 10_000.0);
    assert_eq!(signed_area(&cw), -10_000.0);
}

#[test]
fn signed_area_of_curved_contours() {
    let expected = std::f64::consts::PI * 100.0 * 100.0;
    let ccw = signed_area(&circle(0, 0.0, 0.0, 100.0, true));
    let cw = signed_area(&circle(0, 0.0, 0.0, 100.0, false));
    assert!((ccw - expected).abs() < 0.01 * expected, "ccw area {ccw}");
    assert!((cw + expected).abs() < 0.01 * expected, "cw area {cw}");
}

#[test]
fn signed_area_of_open_or_invalid_contours_is_zero() {
    assert_eq!(
        signed_area(&open(
            0,
            vec![line(0.0, 0.0), line(100.0, 0.0), line(100.0, 100.0)]
        )),
        0.0
    );
    assert_eq!(signed_area(&closed(0, vec![line(1.0, 1.0)])), 0.0);
    assert_eq!(signed_area(&closed(0, vec![])), 0.0);
}

// ------------------------------------------------------------ nesting

#[test]
fn contour_depths_count_enclosing_contours() {
    let paths = vec![
        square(0, 0.0, 0.0, 1000.0, 1000.0, true),
        square(1, 100.0, 100.0, 900.0, 900.0, false),
        square(2, 200.0, 200.0, 800.0, 800.0, true),
        square(3, 2000.0, 0.0, 3000.0, 1000.0, true),
    ];
    assert_eq!(contour_depths(&paths), vec![0, 1, 2, 0]);
    assert_eq!(contour_depths(&[]), Vec::<usize>::new());
}

#[test]
fn contour_depths_ignore_open_and_invalid_contours() {
    let paths = vec![
        square(0, 0.0, 0.0, 1000.0, 1000.0, true),
        // Open contours get depth 0 and never contain anything.
        open(
            1,
            vec![line(100.0, 100.0), line(900.0, 100.0), line(900.0, 900.0)],
        ),
        square(2, 200.0, 200.0, 300.0, 300.0, true),
        // Invalid contours behave like open ones.
        closed(3, vec![line(500.0, 500.0)]),
    ];
    assert_eq!(contour_depths(&paths), vec![0, 0, 1, 0]);
}

#[test]
fn contour_depths_work_on_curved_contours() {
    let paths = vec![
        circle(0, 500.0, 350.0, 300.0, true),
        circle(1, 500.0, 350.0, 150.0, false),
    ];
    assert_eq!(contour_depths(&paths), vec![0, 1]);
}

// ----------------------------------------------------- direction fixing

#[test]
fn normalize_direction_leaves_correct_contours_untouched() {
    let mut paths = vec![
        square(0, 0.0, 0.0, 1000.0, 1000.0, true),
        square(1, 100.0, 100.0, 900.0, 900.0, false),
        circle(2, 500.0, 500.0, 100.0, true),
    ];
    let before = paths.clone();
    normalize_direction(&mut paths);
    // The circle is at depth 2 (inside both squares): counter-clockwise.
    assert_eq!(paths, before);
}

#[test]
fn normalize_direction_fixes_every_nesting_level() {
    let mut paths = vec![
        square(0, 0.0, 0.0, 1000.0, 1000.0, false),
        square(1, 100.0, 100.0, 900.0, 900.0, true),
        square(2, 200.0, 200.0, 800.0, 800.0, false),
        square(3, 2000.0, 0.0, 3000.0, 1000.0, false),
    ];
    normalize_direction(&mut paths);
    let signs: Vec<bool> = paths.iter().map(|p| signed_area(p) > 0.0).collect();
    assert_eq!(signs, vec![true, false, true, true]);
}

#[test]
fn normalize_direction_reversal_preserves_geometry() {
    // A counter-clockwise contour at depth 1 must be reversed to clockwise.
    let inner = mixed(1);
    let mut paths = vec![square(0, -500.0, -500.0, 500.0, 500.0, true), inner.clone()];
    let area_before = signed_area(&inner);
    assert!(area_before > 0.0);

    normalize_direction(&mut paths);

    let reversed = &paths[1];
    assert_eq!(reversed.id, PathId(1));
    assert!(reversed.closed);
    // Exactly the mirrored node list, with each curve kind re-assigned to the
    // node the reversed segment now arrives at.
    assert_eq!(
        reversed.nodes,
        vec![
            line(0.0, 0.0),
            line(0.0, 50.0),
            off(0.0, 120.0),
            qcurve(50.0, 100.0),
            line(100.0, 100.0),
            off(150.0, 80.0),
            off(150.0, 20.0),
            curve(100.0, 0.0),
        ]
    );
    assert_eq!(undirected_segments(reversed), undirected_segments(&inner));
    assert_rect_close(
        bbox(std::slice::from_ref(reversed)).expect("has a box"),
        bbox(&[inner]).expect("has a box"),
    );
    let area_after = signed_area(reversed);
    assert!(area_after < 0.0);
    assert!((area_after + area_before).abs() < 1e-9);
}

#[test]
fn normalize_direction_handles_contours_starting_with_offcurves() {
    let rotated = closed(
        1,
        vec![
            off(0.0, 120.0),
            qcurve(0.0, 50.0),
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(150.0, 20.0),
            off(150.0, 80.0),
            curve(100.0, 100.0),
            line(50.0, 100.0),
        ],
    );
    let mut paths = vec![
        square(0, -500.0, -500.0, 500.0, 500.0, true),
        rotated.clone(),
    ];
    normalize_direction(&mut paths);
    assert!(signed_area(&paths[1]) < 0.0);
    assert_eq!(
        undirected_segments(&paths[1]),
        undirected_segments(&rotated)
    );
    assert!(to_bezpath(&paths[1]).is_ok());
}

#[test]
fn normalize_direction_never_touches_open_or_invalid_contours() {
    let open_path = open(
        1,
        vec![line(100.0, 100.0), line(200.0, 100.0), line(200.0, 200.0)],
    );
    let invalid = closed(2, vec![line(300.0, 300.0)]);
    let mut paths = vec![
        square(0, 0.0, 0.0, 1000.0, 1000.0, false),
        open_path.clone(),
        invalid.clone(),
    ];
    normalize_direction(&mut paths);
    assert!(signed_area(&paths[0]) > 0.0);
    assert_eq!(paths[1], open_path);
    assert_eq!(paths[2], invalid);
}
