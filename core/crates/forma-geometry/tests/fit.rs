//! Integration tests for cubic curve fitting and path simplification.

mod common;

use forma_geometry::{GeometryError, fit_cubics, simplify_path, to_bezpath};
use forma_model::{Node, NodeKind, Path, PathId};
use kurbo::{CubicBez, ParamCurve, ParamCurveDeriv, ParamCurveNearest, PathSeg, Point, Vec2};
use proptest::prelude::*;

// ---------------------------------------------------------------- helpers

fn node(x: f64, y: f64, kind: NodeKind, smooth: bool) -> Node {
    Node { x, y, kind, smooth }
}

fn line(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::Line, false)
}

fn off(x: f64, y: f64) -> Node {
    node(x, y, NodeKind::OffCurve, false)
}

fn curve(x: f64, y: f64, smooth: bool) -> Node {
    node(x, y, NodeKind::Curve, smooth)
}

fn closed(id: u32, nodes: Vec<Node>) -> Path {
    Path {
        id: PathId(id),
        closed: true,
        nodes,
    }
}

fn pt(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

fn circle_points(count: usize, radius: f64) -> Vec<Point> {
    (0..count)
        .map(|i| {
            let angle = std::f64::consts::TAU * (i as f64) / (count as f64);
            pt(radius * angle.cos(), radius * angle.sin())
        })
        .collect()
}

/// The distance from `p` to the nearest point of `curves`.
fn distance_to_curves(curves: &[CubicBez], p: Point) -> f64 {
    curves
        .iter()
        .map(|c| c.nearest(p, 1e-6).distance_sq)
        .fold(f64::INFINITY, f64::min)
        .sqrt()
}

/// The distance from `p` to the nearest point of the contour `path`.
fn distance_to_path(path: &Path, p: Point) -> f64 {
    let bez = to_bezpath(path).expect("valid path");
    bez.segments()
        .map(|segment: PathSeg| segment.nearest(p, 1e-6).distance_sq)
        .fold(f64::INFINITY, f64::min)
        .sqrt()
}

/// Points sampled along the contour `path`.
fn sample_path(path: &Path, per_segment: usize) -> Vec<Point> {
    let bez = to_bezpath(path).expect("valid path");
    bez.segments()
        .flat_map(|segment| {
            (0..per_segment).map(move |i| segment.eval(i as f64 / per_segment as f64))
        })
        .collect()
}

fn assert_chain(curves: &[CubicBez]) {
    for pair in curves.windows(2) {
        assert_eq!(pair[0].p3, pair[1].p0, "segments must connect");
    }
}

fn unit(v: Vec2) -> Option<Vec2> {
    (v.hypot() > 1e-9).then(|| v / v.hypot())
}

/// The direction in which the curve leaves its start.
fn start_direction(c: &CubicBez) -> Option<Vec2> {
    unit(c.deriv().eval(0.0).to_vec2())
}

/// The direction in which the curve arrives at its end.
fn end_direction(c: &CubicBez) -> Option<Vec2> {
    unit(c.deriv().eval(1.0).to_vec2())
}

fn assert_tangent_continuous(a: &CubicBez, b: &CubicBez) {
    let (Some(da), Some(db)) = (end_direction(a), start_direction(b)) else {
        return;
    };
    let cross = da.cross(db).abs();
    assert!(
        cross < 1e-6 && da.dot(db) > 0.0,
        "tangents differ at the join: {da:?} vs {db:?}"
    );
}

// ---------------------------------------------------------------- fit_cubics

fn noisy_samples(
    control: &[(f64, f64); 4],
    count: usize,
    noise: &[(f64, f64)],
) -> (CubicBez, Vec<Point>) {
    let cubic = CubicBez::new(
        pt(control[0].0, control[0].1),
        pt(control[1].0, control[1].1),
        pt(control[2].0, control[2].1),
        pt(control[3].0, control[3].1),
    );
    let points = (0..count)
        .map(|i| {
            let t = i as f64 / (count - 1) as f64;
            let p = cubic.eval(t);
            // The first and last samples carry noise too: nothing is exact.
            pt(p.x + noise[i].0, p.y + noise[i].1)
        })
        .collect();
    (cubic, points)
}

proptest! {
    // A fixed seed unless FORMA_PROPTEST_RANDOM=1 (see `proptest_config`).
    #![proptest_config(common::proptest_config(1000))]

    #[test]
    fn fit_respects_max_error(
        control in prop::array::uniform4((-500.0f64..500.0, -500.0f64..500.0)),
        count in 8usize..120,
        noise in prop::collection::vec((-0.14f64..0.14, -0.14f64..0.14), 120),
    ) {
        let max_error = 1.5;
        let (_, points) = noisy_samples(&control, count, &noise);
        let fitted = fit_cubics(&points, max_error, false);
        // Fewer than 2 distinct points can give nothing; otherwise something.
        if !fitted.is_empty() {
            assert_chain(&fitted);
            prop_assert_eq!(fitted[0].p0, points[0]);
            let end = fitted[fitted.len() - 1].p3;
            prop_assert!(end.distance(points[points.len() - 1]) < 1e-6);
            for p in &points {
                let d = distance_to_curves(&fitted, *p);
                prop_assert!(d <= max_error, "point {:?} is {} away (> {})", p, d, max_error);
            }
        }
    }

    #[test]
    fn closed_fit_respects_max_error_and_closes(
        radius_x in 50.0f64..400.0,
        radius_y in 50.0f64..400.0,
        count in 12usize..200,
        noise in prop::collection::vec((-0.14f64..0.14, -0.14f64..0.14), 200),
    ) {
        let max_error = 1.5;
        let points: Vec<Point> = (0..count)
            .map(|i| {
                let angle = std::f64::consts::TAU * (i as f64) / (count as f64);
                pt(
                    radius_x * angle.cos() + noise[i].0,
                    radius_y * angle.sin() + noise[i].1,
                )
            })
            .collect();
        let fitted = fit_cubics(&points, max_error, true);
        prop_assert!(!fitted.is_empty());
        assert_chain(&fitted);
        prop_assert_eq!(fitted[0].p0, fitted[fitted.len() - 1].p3);
        for p in &points {
            let d = distance_to_curves(&fitted, *p);
            prop_assert!(d <= max_error, "point {:?} is {} away (> {})", p, d, max_error);
        }
    }
}

#[test]
fn fit_circle_uses_few_segments() {
    let points = circle_points(360, 300.0);
    for closed in [false, true] {
        let fitted = fit_cubics(&points, 0.5, closed);
        assert!(
            (4..=8).contains(&fitted.len()),
            "closed={closed}: {} cubics",
            fitted.len()
        );
        assert_chain(&fitted);
        for p in &points {
            assert!(distance_to_curves(&fitted, *p) <= 0.5);
        }
    }
}

#[test]
fn a_loose_fit_of_a_circle_is_not_pulled_inward() {
    // At a tolerance of 1 % of the radius four cubics suffice. The fit must
    // be the refined one, which strays about as far out as in; the first
    // fit within the tolerance (chord-length parameters) went 0.62 of the
    // tolerance inward and only 0.25 outward, shrinking every traced curve.
    let radius = 100.0;
    let points: Vec<Point> = (0..400)
        .map(|i| {
            let angle = std::f64::consts::TAU * (i as f64) / 400.0 + 0.3;
            pt(radius * angle.cos(), radius * angle.sin())
        })
        .collect();
    let tolerance = 1.0;
    let fitted = fit_cubics(&points, tolerance, true);
    assert_eq!(fitted.len(), 4);
    let (mut inward, mut outward) = (0.0f64, 0.0f64);
    for c in &fitted {
        for k in 0..=100 {
            let off = c.eval(f64::from(k) / 100.0).to_vec2().hypot() - radius;
            inward = inward.max(-off);
            outward = outward.max(off);
        }
    }
    assert!(
        inward <= 0.4 * tolerance && outward <= 0.4 * tolerance,
        "strays {inward} in, {outward} out"
    );
}

#[test]
fn closed_fit_joins_with_a_continuous_tangent() {
    let points = circle_points(360, 300.0);
    let fitted = fit_cubics(&points, 0.5, true);
    assert_eq!(fitted[0].p0, fitted[fitted.len() - 1].p3);
    for pair in fitted.windows(2) {
        assert_tangent_continuous(&pair[0], &pair[1]);
    }
    // The join between the last and the first segment is smooth too.
    assert_tangent_continuous(&fitted[fitted.len() - 1], &fitted[0]);
}

#[test]
fn closed_fit_accepts_a_repeated_first_point() {
    let mut points = circle_points(120, 200.0);
    let without = fit_cubics(&points, 0.5, true);
    points.push(points[0]);
    let with = fit_cubics(&points, 0.5, true);
    assert_eq!(without, with);
}

#[test]
fn open_fit_ends_at_the_end_points_with_no_bend_in_a_line() {
    let points: Vec<Point> = (0..50)
        .map(|i| pt(i as f64 * 10.0, i as f64 * 5.0))
        .collect();
    let fitted = fit_cubics(&points, 0.5, false);
    assert_eq!(fitted.len(), 1, "collinear points need one cubic");
    assert_eq!(fitted[0].p0, points[0]);
    assert_eq!(fitted[0].p3, points[49]);
    for p in &points {
        assert!(distance_to_curves(&fitted, *p) <= 0.5);
    }
}

#[test]
fn a_cubic_sampled_exactly_comes_back_as_one_segment() {
    let cubic = CubicBez::new(
        pt(0.0, 0.0),
        pt(100.0, 300.0),
        pt(300.0, 300.0),
        pt(400.0, 0.0),
    );
    let points: Vec<Point> = (0..=40).map(|i| cubic.eval(i as f64 / 40.0)).collect();
    let fitted = fit_cubics(&points, 0.5, false);
    assert_eq!(fitted.len(), 1);
    for p in &points {
        assert!(distance_to_curves(&fitted, *p) <= 0.5);
    }
}

#[test]
fn fit_a_corner_by_splitting() {
    // An L: the fit has to split near the corner, never overshoot the error.
    let mut points = Vec::new();
    for i in 0..=20 {
        points.push(pt(i as f64 * 10.0, 0.0));
    }
    for i in 1..=20 {
        points.push(pt(200.0, i as f64 * 10.0));
    }
    let fitted = fit_cubics(&points, 1.0, false);
    assert!(fitted.len() >= 2);
    assert_chain(&fitted);
    for p in &points {
        assert!(distance_to_curves(&fitted, *p) <= 1.0);
    }
}

#[test]
fn fit_with_nothing_to_fit_gives_nothing() {
    assert!(fit_cubics(&[], 1.0, false).is_empty());
    assert!(fit_cubics(&[pt(1.0, 2.0)], 1.0, false).is_empty());
    assert!(fit_cubics(&[pt(1.0, 2.0), pt(1.0, 2.0)], 1.0, false).is_empty());
    assert!(fit_cubics(&[pt(1.0, 2.0), pt(1.0, 2.0)], 1.0, true).is_empty());
}

#[test]
fn fit_two_points_is_a_straight_cubic() {
    let fitted = fit_cubics(&[pt(0.0, 0.0), pt(30.0, 0.0)], 1.0, false);
    assert_eq!(fitted.len(), 1);
    let c = fitted[0];
    assert_eq!((c.p0, c.p3), (pt(0.0, 0.0), pt(30.0, 0.0)));
    assert!((c.p1.x - 10.0).abs() < 1e-9 && c.p1.y.abs() < 1e-9);
    assert!((c.p2.x - 20.0).abs() < 1e-9 && c.p2.y.abs() < 1e-9);
}

#[test]
fn closed_fit_of_fewer_than_three_points_is_open() {
    let fitted = fit_cubics(&[pt(0.0, 0.0), pt(30.0, 0.0)], 1.0, true);
    assert_eq!(fitted.len(), 1);
}

#[test]
fn fit_ignores_consecutive_duplicates() {
    let clean = circle_points(90, 200.0);
    let mut doubled = Vec::new();
    for p in &clean {
        doubled.push(*p);
        doubled.push(*p);
    }
    assert_eq!(
        fit_cubics(&clean, 0.5, false),
        fit_cubics(&doubled, 0.5, false)
    );
}

#[test]
fn fit_refuses_non_finite_input() {
    let points = [pt(0.0, 0.0), pt(f64::NAN, 1.0), pt(5.0, 5.0)];
    assert!(fit_cubics(&points, 1.0, false).is_empty());
    let points = [pt(0.0, 0.0), pt(f64::INFINITY, 1.0), pt(5.0, 5.0)];
    assert!(fit_cubics(&points, 1.0, false).is_empty());
}

#[test]
fn fit_with_a_useless_tolerance_still_terminates() {
    let points = circle_points(40, 100.0);
    for max_error in [0.0, -1.0, f64::NAN] {
        let fitted = fit_cubics(&points, max_error, false);
        assert!(!fitted.is_empty());
        assert_chain(&fitted);
        assert_eq!(fitted[0].p0, points[0]);
    }
}

// ---------------------------------------------------------------- simplify_path

/// A 1000 x 1000 square whose bottom side carries 20 collinear nodes.
fn oversampled_square(smooth: bool) -> Path {
    let mut nodes = Vec::new();
    for i in 0..20 {
        nodes.push(node(i as f64 * 50.0, 0.0, NodeKind::Line, smooth));
    }
    nodes.push(line(1000.0, 0.0));
    nodes.push(line(1000.0, 1000.0));
    nodes.push(line(0.0, 1000.0));
    closed(4, nodes)
}

#[test]
fn simplify_keeps_corners() {
    for smooth in [false, true] {
        let simplified = simplify_path(&oversampled_square(smooth), 1.0).expect("simplify");
        assert_eq!(simplified.id, PathId(4));
        assert!(simplified.closed);
        assert_eq!(simplified.nodes.len(), 4, "smooth={smooth}");
        assert!(simplified.nodes.iter().all(|n| n.kind == NodeKind::Line));
        let corners: Vec<(f64, f64)> = simplified.nodes.iter().map(|n| (n.x, n.y)).collect();
        assert_eq!(
            corners,
            vec![(0.0, 0.0), (1000.0, 0.0), (1000.0, 1000.0), (0.0, 1000.0)]
        );
    }
}

#[test]
fn simplify_leaves_an_already_minimal_path_alone() {
    let square = closed(
        0,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    assert_eq!(simplify_path(&square, 1.0).expect("simplify"), square);

    // A rounded corner joined tangentially to straight edges stays as drawn.
    let rounded = closed(
        1,
        vec![
            curve(50.0, 0.0, true),
            line(450.0, 0.0),
            off(477.6, 0.0),
            off(500.0, 22.4),
            curve(500.0, 50.0, true),
            line(500.0, 450.0),
            off(500.0, 477.6),
            off(477.6, 500.0),
            curve(450.0, 500.0, true),
            line(50.0, 500.0),
            off(22.4, 500.0),
            off(0.0, 477.6),
            curve(0.0, 450.0, true),
            line(0.0, 50.0),
            off(0.0, 22.4),
            off(22.4, 0.0),
        ],
    );
    assert_eq!(simplify_path(&rounded, 1.0).expect("simplify"), rounded);
}

#[test]
fn simplify_refits_a_flattened_circle() {
    // 360 `line` nodes, one degree apart: no corner, one smooth loop.
    let polygon = closed(
        2,
        circle_points(360, 300.0)
            .into_iter()
            .map(|p| line(p.x, p.y))
            .collect(),
    );
    let simplified = simplify_path(&polygon, 0.5).expect("simplify");
    assert_eq!(simplified.id, PathId(2));
    assert!(simplified.closed);
    // Few segments, all cubic, and the first node is where it was.
    let on_curve = simplified
        .nodes
        .iter()
        .filter(|n| n.kind != NodeKind::OffCurve)
        .count();
    assert!((4..=8).contains(&on_curve), "{on_curve} on-curve nodes");
    assert!(simplified.nodes.iter().all(|n| n.kind != NodeKind::Line));
    assert_eq!(
        (simplified.nodes[0].x, simplified.nodes[0].y),
        (polygon.nodes[0].x, polygon.nodes[0].y)
    );
    // The new nodes between segments are tangent-continuous, hence smooth.
    assert!(
        simplified
            .nodes
            .iter()
            .skip(1)
            .filter(|n| n.kind == NodeKind::Curve)
            .all(|n| n.smooth)
    );
    for p in sample_path(&polygon, 1) {
        assert!(distance_to_path(&simplified, p) <= 0.5 + 1e-6);
    }
    for p in sample_path(&simplified, 40) {
        assert!(distance_to_path(&polygon, p) <= 0.5 + 1e-6);
    }
}

#[test]
fn simplify_keeps_the_corners_of_a_half_disc() {
    // A flat base and an arc made of 180 line nodes, one degree apart.
    let mut nodes = vec![line(-300.0, 0.0), line(300.0, 0.0)];
    for degrees in 1..180 {
        let angle = (degrees as f64).to_radians();
        nodes.push(line(300.0 * angle.cos(), 300.0 * angle.sin()));
    }
    let path = closed(0, nodes);
    let simplified = simplify_path(&path, 0.5).expect("simplify");
    let first = &simplified.nodes[0];
    assert_eq!((first.x, first.y), (-300.0, 0.0));
    // The base is still a single straight segment ending at the other corner.
    assert_eq!(simplified.nodes[1].kind, NodeKind::Line);
    assert_eq!((simplified.nodes[1].x, simplified.nodes[1].y), (300.0, 0.0));
    assert!(simplified.nodes.len() < path.nodes.len() / 10);
    // The arc now closes onto the first node with a curve.
    assert_eq!(first.kind, NodeKind::Curve);
    for p in sample_path(&path, 1) {
        assert!(distance_to_path(&simplified, p) <= 0.5 + 1e-6);
    }
}

/// A cubic split in two halves by de Casteljau, as two path segments.
fn split_cubic_path(middle_smooth: bool) -> Path {
    let whole = CubicBez::new(
        pt(0.0, 0.0),
        pt(100.0, 300.0),
        pt(300.0, 300.0),
        pt(400.0, 0.0),
    );
    let (a, b) = whole.subdivide();
    closed(
        0,
        vec![
            line(a.p0.x, a.p0.y),
            off(a.p1.x, a.p1.y),
            off(a.p2.x, a.p2.y),
            curve(a.p3.x, a.p3.y, middle_smooth),
            off(b.p1.x, b.p1.y),
            off(b.p2.x, b.p2.y),
            curve(b.p3.x, b.p3.y, false),
        ],
    )
}

#[test]
fn simplify_merges_smooth_segments_and_keeps_non_smooth_nodes() {
    // The middle node is smooth: the two halves fit in one cubic.
    let merged = simplify_path(&split_cubic_path(true), 0.5).expect("simplify");
    assert_eq!(merged.nodes.len(), 4, "{:?}", merged.nodes);
    // The closing line keeps its kind on the first node.
    assert_eq!(merged.nodes[0].kind, NodeKind::Line);
    assert_eq!(merged.nodes[3].kind, NodeKind::Curve);
    assert_eq!((merged.nodes[3].x, merged.nodes[3].y), (400.0, 0.0));

    // The same geometry with a non-smooth middle node: the designer's node
    // is kept, and so is the path.
    let kept = split_cubic_path(false);
    assert_eq!(simplify_path(&kept, 0.5).expect("simplify"), kept);
}

#[test]
fn simplify_keeps_a_sharp_curve_join_even_when_flagged_smooth() {
    // Two cubics meeting at about 30 degrees: a real corner, whatever the flag.
    let path = closed(
        0,
        vec![
            line(0.0, 0.0),
            off(50.0, 0.0),
            off(100.0, 20.0),
            curve(150.0, 50.0, true),
            off(200.0, 40.0),
            off(250.0, 0.0),
            curve(300.0, 0.0, false),
        ],
    );
    assert_eq!(simplify_path(&path, 0.5).expect("simplify"), path);
}

#[test]
fn simplify_an_open_path_keeps_its_end_points() {
    let points = circle_points(360, 300.0);
    let open = Path {
        id: PathId(8),
        closed: false,
        nodes: points[..181].iter().map(|p| line(p.x, p.y)).collect(),
    };
    let simplified = simplify_path(&open, 0.5).expect("simplify");
    assert!(!simplified.closed);
    assert_eq!(simplified.id, PathId(8));
    assert_eq!(simplified.nodes[0], open.nodes[0]);
    let last = simplified.nodes.last().expect("nodes");
    let original_last = open.nodes.last().expect("nodes");
    assert_eq!((last.x, last.y), (original_last.x, original_last.y));
    assert!(simplified.nodes.len() < 20);
    for p in sample_path(&open, 1) {
        assert!(distance_to_path(&simplified, p) <= 0.5 + 1e-6);
    }
}

#[test]
fn simplify_collapses_a_nearly_straight_run_to_one_line() {
    // 10 nodes a fraction of a unit off a straight line: one line segment.
    let open = Path {
        id: PathId(0),
        closed: false,
        nodes: (0..10)
            .map(|i| line(i as f64 * 10.0, if i % 2 == 0 { 0.0 } else { 0.2 }))
            .chain([line(100.0, 0.0)])
            .collect(),
    };
    let simplified = simplify_path(&open, 1.0).expect("simplify");
    assert_eq!(simplified.nodes.len(), 2);
    assert!(simplified.nodes.iter().all(|n| n.kind == NodeKind::Line));
}

#[test]
fn simplify_converts_a_dense_quadratic_run() {
    // Many quadratic segments along one parabola: refit with fewer cubics.
    let mut nodes = vec![node(0.0, 0.0, NodeKind::Line, false)];
    let n = 24;
    for i in 1..=n {
        let t0 = (i - 1) as f64 / n as f64;
        let t1 = i as f64 / n as f64;
        let tm = (t0 + t1) / 2.0;
        // The parabola y = 4x(1-x) * 300 over x in 0..1000, x = 1000 t.
        let f = |t: f64| (1000.0 * t, 1200.0 * t * (1.0 - t));
        let (x0, y0) = f(t0);
        let (x1, y1) = f(t1);
        let (xm, ym) = f(tm);
        // Quadratic through the chord ends with the tangent-intersection control.
        let cx = 2.0 * xm - (x0 + x1) / 2.0;
        let cy = 2.0 * ym - (y0 + y1) / 2.0;
        nodes.push(off(cx, cy));
        nodes.push(node(x1, y1, NodeKind::QCurve, true));
    }
    let path = Path {
        id: PathId(0),
        closed: false,
        nodes,
    };
    let simplified = simplify_path(&path, 0.5).expect("simplify");
    assert!(simplified.nodes.len() < path.nodes.len());
    assert!(simplified.nodes.iter().all(|n| matches!(
        n.kind,
        NodeKind::Line | NodeKind::OffCurve | NodeKind::Curve
    )));
    for p in sample_path(&path, 4) {
        assert!(distance_to_path(&simplified, p) <= 0.5 + 1e-6);
    }
}

#[test]
fn simplify_rejects_bad_input() {
    let square = oversampled_square(false);
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            simplify_path(&square, bad),
            Err(GeometryError::InvalidPath(_))
        ));
    }
    let empty = closed(0, vec![]);
    assert!(matches!(
        simplify_path(&empty, 1.0),
        Err(GeometryError::Degenerate(_))
    ));
    let malformed = closed(0, vec![line(0.0, 0.0), line(10.0, 0.0), off(10.0, 10.0)]);
    assert!(matches!(
        simplify_path(&malformed, 1.0),
        Err(GeometryError::InvalidPath(_))
    ));
}

proptest! {
    // A fixed seed unless FORMA_PROPTEST_RANDOM=1 (see `proptest_config`).
    #![proptest_config(common::proptest_config(200))]

    /// A noisy flattened circle simplifies to no more nodes, stays within the
    /// tolerance of the input nodes and remains a valid contour.
    #[test]
    fn simplify_is_valid_and_within_tolerance(
        count in 20usize..300,
        radius in 100.0f64..500.0,
        noise in prop::collection::vec((-0.2f64..0.2, -0.2f64..0.2), 300),
        smooth in any::<bool>(),
    ) {
        let max_error = 1.0;
        let nodes: Vec<Node> = (0..count)
            .map(|i| {
                let angle = std::f64::consts::TAU * (i as f64) / (count as f64);
                node(
                    radius * angle.cos() + noise[i].0,
                    radius * angle.sin() + noise[i].1,
                    NodeKind::Line,
                    smooth,
                )
            })
            .collect();
        let path = closed(3, nodes);
        let simplified = simplify_path(&path, max_error).expect("simplify");
        prop_assert!(to_bezpath(&simplified).is_ok());
        prop_assert!(simplified.nodes.len() <= path.nodes.len());
        prop_assert_eq!(simplified.id, PathId(3));
        prop_assert!(simplified.closed);
        for n in &path.nodes {
            let d = distance_to_path(&simplified, pt(n.x, n.y));
            prop_assert!(d <= max_error + 1e-6, "node ({}, {}) is {} away", n.x, n.y, d);
        }
    }
}

#[test]
fn simplify_never_loops_away_from_a_hairpin() {
    // A narrow wedge of many short lines (the inner corner where the
    // diagonal of an N meets its stem): a least-squares fit through its
    // vertices may loop far away; the simplified contour must stay near.
    let mut nodes = Vec::new();
    for k in 0..40 {
        let t = f64::from(k);
        nodes.push(Node {
            x: 600.0 + t * 1.5,
            y: 400.0 + t * 2.0 + (t * 0.7).sin() * 0.8,
            kind: NodeKind::Line,
            smooth: false,
        });
    }
    for k in (0..40).rev() {
        let t = f64::from(k);
        nodes.push(Node {
            x: 600.0 + t * 1.5 + 4.0,
            y: 400.0 + t * 2.0 - 3.0,
            kind: NodeKind::Line,
            smooth: false,
        });
    }
    let path = Path {
        id: PathId(0),
        closed: true,
        nodes,
    };
    let before = forma_geometry::bbox(std::slice::from_ref(&path)).expect("bbox");
    let simplified = simplify_path(&path, 1.5).expect("simplify");
    let after = forma_geometry::bbox(std::slice::from_ref(&simplified)).expect("bbox");
    assert!(
        after.y1 <= before.y1 + 3.0 && after.y0 >= before.y0 - 3.0,
        "{after:?} vs {before:?}"
    );
    assert!(
        after.x1 <= before.x1 + 3.0 && after.x0 >= before.x0 - 3.0,
        "{after:?} vs {before:?}"
    );
}
