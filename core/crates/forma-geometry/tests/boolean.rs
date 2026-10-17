//! Integration tests for overlap removal (non-zero union).

mod common;

use std::f64::consts::PI;

use common::*;
use forma_geometry::{
    CoordinateSystem, GeometryError, IssueKind, Linesweeper, PathOps, check_paths, paths_from_svg,
    remove_overlap, to_bezpath,
};
use forma_model::{NodeKind, Path, PathId};
use kurbo::Point;
use proptest::prelude::*;

/// A five-pointed star drawn as one self-intersecting contour (every second
/// vertex of a regular pentagon). Under the non-zero rule its centre is
/// filled twice.
fn pentagram(cx: f64, cy: f64, r: f64) -> Path {
    let nodes = (0..5)
        .map(|i| {
            let angle = PI / 2.0 + f64::from(i) * 4.0 * PI / 5.0;
            line(cx + r * angle.cos(), cy + r * angle.sin())
        })
        .collect();
    closed(nodes)
}

/// The sweep leaves no artefacts: no segment shorter than a unit and no
/// repeated node.
fn assert_no_artefacts(paths: &[Path]) {
    let artefacts: Vec<_> = check_paths(paths, 0.0)
        .into_iter()
        .filter(|issue| {
            matches!(
                issue.kind,
                IssueKind::TinySegment | IssueKind::DuplicatePoints
            )
        })
        .collect();
    assert!(artefacts.is_empty(), "{artefacts:?}");
}

#[test]
fn union_of_two_circles_has_expected_area() {
    let r = 100.0;
    let d = 100.0;
    let paths = [circle(0.0, 0.0, r, true), circle(d, 0.0, r, true)];
    let result = remove_overlap(&paths).expect("union of two circles");

    assert_eq!(result.len(), 1, "one contour expected, got {result:?}");
    let got = area(&result[0]);
    assert!(got > 0.0, "the outer contour must run counter-clockwise");

    let lens = 2.0 * r * r * (d / (2.0 * r)).acos() - (d / 2.0) * (4.0 * r * r - d * d).sqrt();
    let expected = 2.0 * PI * r * r - lens;
    let error = (got - expected).abs() / expected;
    assert!(
        error <= 0.005,
        "area {got} vs expected {expected} ({:.3}% off)",
        error * 100.0
    );
    assert_filled_on_the_left(&result);
    // The two circles cross at (50, ±86.6): the sweep leaves a segment a few
    // 1e-10 units long at one of them, which is removed.
    assert_no_artefacts(&result);
}

#[test]
fn union_keeps_counters() {
    // An "O" (exterior counter-clockwise, counter clockwise) and a bar that
    // overlaps the ring from the outside, away from the counter.
    let paths = [
        circle(0.0, 0.0, 200.0, true),
        circle(0.0, 0.0, 100.0, false),
        rect(150.0, -30.0, 320.0, 30.0, true),
    ];
    // Through the trait object: the engine can be swapped behind `PathOps`.
    let ops: &dyn PathOps = &Linesweeper;
    let result = ops.union(&paths).expect("union of the O and the bar");

    assert_eq!(result.len(), 2, "one exterior and one counter: {result:?}");
    let areas = areas(&result);
    let outer: Vec<f64> = areas.iter().copied().filter(|a| *a > 0.0).collect();
    let inner: Vec<f64> = areas.iter().copied().filter(|a| *a < 0.0).collect();
    assert_eq!(outer.len(), 1, "areas {areas:?}");
    assert_eq!(inner.len(), 1, "areas {areas:?}");

    let counter = PI * 100.0 * 100.0;
    assert!(
        (inner[0] + counter).abs() <= 0.005 * counter,
        "the counter must be kept as it was: {} vs {}",
        inner[0],
        -counter
    );
    // The ring grows by the part of the bar that sticks out of it.
    let ring = PI * (200.0 * 200.0 - 100.0 * 100.0);
    assert!(outer[0] + inner[0] > ring);
    assert_filled_on_the_left(&result);
    assert_eq!(remove_overlap(&paths).expect("union"), result);
}

#[test]
fn no_self_intersections_after_union() {
    let paths = [
        pentagram(0.0, 0.0, 150.0),
        circle(200.0, 0.0, 120.0, true),
        circle(300.0, 60.0, 90.0, true),
        rect(-50.0, -20.0, 420.0, 25.0, true),
        rect(250.0, -200.0, 280.0, 200.0, true),
    ];
    assert!(
        crossing_count(&paths) > 0,
        "the input is meant to overlap itself"
    );

    let result = remove_overlap(&paths).expect("union");

    assert!(!result.is_empty());
    assert_eq!(crossing_count(&result), 0, "the union crosses itself");
    for path in &result {
        assert!(path.closed);
        to_bezpath(path).expect("every contour is valid");
    }
    assert_filled_on_the_left(&result);
    // Everything overlaps everything: one outline, no holes.
    let areas = areas(&result);
    assert_eq!(areas.iter().filter(|a| **a > 0.0).count(), 1, "{areas:?}");
    // The non-zero rule fills the centre of the star.
    assert_eq!(winding(&result, Point::new(0.0, 0.0)), 1);
}

#[test]
fn duplicate_contours_leave_no_slivers() {
    // Coincident curves are where the sweep leaves its largest artefacts: a
    // sliver of zero area between the copies and segments a few hundredths
    // long where it rejoins them.
    for r in [100.0, 2000.0] {
        let paths = [circle(0.0, 0.0, r, true), circle(0.0, 0.0, r, true)];
        let result = remove_overlap(&paths).expect("union");

        assert_eq!(result.len(), 1, "radius {r}: {result:?}");
        let disc = PI * r * r;
        assert!((total_area(&result) - disc).abs() <= 1e-3 * disc);
        assert_no_artefacts(&result);
    }
}

#[test]
fn contours_touching_their_counter_keep_their_direction() {
    // A self-intersecting contour whose union is an outer contour with a
    // counter that touches it at the counter's first node. Nesting by probing
    // that node is ambiguous; `normalize_direction` reversed both contours of
    // this very case (the counter outward, the outer inward).
    let svg = "M100 -150 C-200 -150 -250 -250 -150 -200 C-50 -150 150 200 300 0 \
               L150 -150 Q-50 -200 150 200 Q300 250 -250 -100 Z";
    let paths = paths_from_svg(svg, CoordinateSystem::Font).expect("valid svg");
    let result = remove_overlap(&paths).expect("union");

    let areas = areas(&result);
    let counters: Vec<f64> = areas.iter().copied().filter(|a| *a < 0.0).collect();
    assert_eq!(counters.len(), 1, "{areas:?}");
    assert!((counters[0] + 8508.7).abs() < 1.0, "{areas:?}");
    let largest = areas.iter().copied().fold(f64::MIN, f64::max);
    assert!((largest - 79_371.0).abs() < 1.0, "{areas:?}");
    assert_filled_on_the_left(&result);
}

#[test]
fn quadratics_stay_quadratic() {
    // A base line closed by a quadratic arch, crossed by a vertical bar.
    let arch = closed(vec![
        node(0.0, 0.0, NodeKind::QCurve),
        line(100.0, 0.0),
        off(50.0, 100.0),
    ]);
    let result = remove_overlap(&[arch, rect(40.0, -20.0, 60.0, 200.0, true)]).expect("union");

    assert_eq!(result.len(), 1, "{result:?}");
    let kinds: Vec<NodeKind> = result[0].nodes.iter().map(|n| n.kind).collect();
    assert!(kinds.contains(&NodeKind::QCurve), "{kinds:?}");
    assert!(!kinds.contains(&NodeKind::Curve), "{kinds:?}");
    assert_filled_on_the_left(&result);
}

#[test]
fn clean_contours_are_returned_unchanged() {
    // Review Focus 5: removing overlaps from a clean glyph changes nothing.
    let mut outer = circle(0.0, 0.0, 200.0, true);
    outer.id = PathId(7);
    outer.nodes[3].smooth = true;
    let mut inner = circle(0.0, 0.0, 100.0, false);
    inner.id = PathId(3);
    let separate = rect(300.0, 0.0, 400.0, 100.0, true);
    let paths = vec![outer, inner, separate];

    assert_eq!(remove_overlap(&paths).expect("union"), paths);
}

#[test]
fn wrong_direction_is_corrected() {
    let result = remove_overlap(&[rect(0.0, 0.0, 100.0, 100.0, false)]).expect("union");

    assert_eq!(result.len(), 1);
    assert!((area(&result[0]) - 10_000.0).abs() < 1e-6);
}

#[test]
fn opposite_directions_cancel_under_non_zero() {
    // A clockwise square over a counter-clockwise one cuts it out (winding
    // 1 - 1 = 0), exactly as a rasterizer fills them.
    let paths = [
        rect(0.0, 0.0, 100.0, 100.0, true),
        rect(50.0, 0.0, 150.0, 100.0, false),
    ];
    let result = remove_overlap(&paths).expect("union");

    let total = total_area(&result);
    assert!(
        (total - 10_000.0).abs() < 1e-6,
        "areas {:?}",
        areas(&result)
    );
    assert_eq!(winding(&result, Point::new(75.0, 50.0)), 0);
}

#[test]
fn open_contours_pass_through() {
    let open = Path {
        id: PathId(9),
        closed: false,
        nodes: vec![line(500.0, 0.0), line(600.0, 100.0)],
    };
    let paths = vec![
        rect(0.0, 0.0, 100.0, 100.0, true),
        open.clone(),
        rect(50.0, 50.0, 150.0, 150.0, true),
    ];
    let result = remove_overlap(&paths).expect("union");

    assert_eq!(result.len(), 2);
    assert!(result[0].closed);
    assert_eq!(
        result[1].nodes, open.nodes,
        "the open contour is kept as is"
    );
    assert!(!result[1].closed);
    let ids: Vec<u32> = result.iter().map(|path| path.id.0).collect();
    assert_eq!(ids, vec![0, 1], "contours are numbered from 0");
    assert!((total_area(&result[..1]) - 17_500.0).abs() < 1e-6);
}

#[test]
fn empty_input_gives_empty_output() {
    assert_eq!(remove_overlap(&[]), Ok(vec![]));
}

#[test]
fn malformed_contours_are_rejected() {
    let mut bad = rect(0.0, 0.0, 10.0, 10.0, true);
    bad.nodes[1].x = f64::NAN;
    assert!(matches!(
        remove_overlap(&[bad]),
        Err(GeometryError::InvalidPath(_))
    ));
}

#[derive(Clone, Debug)]
enum Shape {
    Rect(f64, f64, f64, f64),
    Circle(f64, f64, f64),
}

impl Shape {
    fn path(&self) -> Path {
        match *self {
            Shape::Rect(x, y, w, h) => rect(x, y, x + w, y + h, true),
            Shape::Circle(x, y, r) => circle(x, y, r, true),
        }
    }
}

fn shape() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (
            -300.0f64..300.0,
            -300.0f64..300.0,
            10.0f64..300.0,
            10.0f64..300.0
        )
            .prop_map(|(x, y, w, h)| Shape::Rect(x, y, w, h)),
        (-300.0f64..300.0, -300.0f64..300.0, 10.0f64..200.0)
            .prop_map(|(x, y, r)| Shape::Circle(x, y, r)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Removing overlaps twice gives the same contours, area for area.
    #[test]
    fn union_is_idempotent(shapes in prop::collection::vec(shape(), 1..6)) {
        let paths: Vec<Path> = shapes.iter().map(Shape::path).collect();
        let once = remove_overlap(&paths).expect("first union");
        let twice = remove_overlap(&once).expect("second union");

        prop_assert_eq!(once.len(), twice.len());
        let (a1, a2) = (total_area(&once), total_area(&twice));
        prop_assert!((a1 - a2).abs() <= 1e-3 * a1.abs(), "{} vs {}", a1, a2);

        // The union is at least the largest shape and at most their sum.
        let shape_areas = areas(&paths);
        let largest = shape_areas.iter().copied().fold(0.0, f64::max);
        let sum: f64 = shape_areas.iter().sum();
        prop_assert!(a1 >= largest * (1.0 - 1e-3), "{} < {}", a1, largest);
        prop_assert!(a1 <= sum * (1.0 + 1e-3), "{} > {}", a1, sum);
    }
}
