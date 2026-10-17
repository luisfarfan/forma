//! Integration tests for contour nesting and direction on contours that touch
//! each other: the nesting test must not depend on where a contour starts.

mod common;

use common::*;
use forma_geometry::{IssueKind, check_paths, contour_depths, fix_paths, normalize_direction};
use forma_model::{Path, PathId};

fn with_id(mut path: Path, id: u32) -> Path {
    path.id = PathId(id);
    path
}

fn wrong_directions(paths: &[Path]) -> Vec<PathId> {
    check_paths(paths, 0.0)
        .into_iter()
        .filter(|issue| issue.kind == IssueKind::WrongDirection)
        .map(|issue| issue.path)
        .collect()
}

#[test]
fn touching_counter_is_detected_as_inner() {
    // A counter whose start node lies on the right edge of the outer square:
    // it points out of the square 100..300 and touches the outer contour
    // there, but its interior is inside the outer contour.
    let outer = with_id(rect(0.0, 0.0, 400.0, 400.0, true), 0);
    let counter = with_id(
        closed(vec![
            line(400.0, 200.0),
            line(300.0, 100.0),
            line(100.0, 100.0),
            line(100.0, 300.0),
            line(300.0, 300.0),
        ]),
        1,
    );
    assert!(area(&counter) < 0.0, "the counter is drawn clockwise");
    let paths = vec![outer, counter];

    assert_eq!(contour_depths(&paths), vec![0, 1]);
    assert_eq!(wrong_directions(&paths), vec![]);
    assert_eq!(fix_paths(&paths, &[IssueKind::WrongDirection], 0.0), paths);
    let mut normalized = paths.clone();
    normalize_direction(&mut normalized);
    assert_eq!(normalized, paths);

    // The same counter starting on the top edge of the outer square.
    let outer = with_id(rect(0.0, 0.0, 400.0, 400.0, true), 0);
    let counter = with_id(
        closed(vec![
            line(200.0, 400.0),
            line(300.0, 300.0),
            line(300.0, 100.0),
            line(100.0, 100.0),
            line(100.0, 300.0),
        ]),
        1,
    );
    assert!(area(&counter) < 0.0, "the counter is drawn clockwise");
    let paths = vec![outer, counter];
    assert_eq!(contour_depths(&paths), vec![0, 1]);
    assert_eq!(wrong_directions(&paths), vec![]);
}

#[test]
fn touching_outer_contours_are_both_outer() {
    // Two squares sharing an edge; the second one starts at a shared corner.
    let right = with_id(
        closed(vec![
            line(100.0, 0.0),
            line(200.0, 0.0),
            line(200.0, 100.0),
            line(100.0, 100.0),
        ]),
        0,
    );
    let left = with_id(
        closed(vec![
            line(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
            line(0.0, 0.0),
        ]),
        1,
    );
    let paths = vec![right, left];
    assert!(areas(&paths).iter().all(|a| *a > 0.0), "both run CCW");

    assert_eq!(contour_depths(&paths), vec![0, 0]);
    assert_eq!(wrong_directions(&paths), vec![]);
    assert_eq!(fix_paths(&paths, &[IssueKind::WrongDirection], 0.0), paths);

    // Two squares sharing only a corner, the first one starting there.
    let lower = with_id(
        closed(vec![
            line(100.0, 100.0),
            line(0.0, 100.0),
            line(0.0, 0.0),
            line(100.0, 0.0),
        ]),
        0,
    );
    let upper = with_id(rect(100.0, 100.0, 200.0, 200.0, true), 1);
    let paths = vec![lower, upper];
    assert!(areas(&paths).iter().all(|a| *a > 0.0), "both run CCW");

    assert_eq!(contour_depths(&paths), vec![0, 0]);
    assert_eq!(wrong_directions(&paths), vec![]);
    assert_eq!(fix_paths(&paths, &[IssueKind::WrongDirection], 0.0), paths);
    let mut normalized = paths.clone();
    normalize_direction(&mut normalized);
    assert_eq!(normalized, paths);
}

#[test]
fn curved_counter_touching_its_outer_contour_is_inner() {
    // A circular counter tangent to the outer circle from inside, starting
    // at the point where they touch.
    let outer = with_id(circle(0.0, 0.0, 200.0, true), 0);
    let counter = with_id(circle(100.0, 0.0, 100.0, false), 1);
    // `circle` starts at its rightmost point, (200, 0): the tangent point.
    assert_eq!((counter.nodes[0].x, counter.nodes[0].y), (200.0, 0.0));
    let paths = vec![outer, counter];

    assert_eq!(contour_depths(&paths), vec![0, 1]);
    assert_eq!(wrong_directions(&paths), vec![]);
}

#[test]
fn nested_contours_that_do_not_touch_keep_their_depths() {
    // Behaviour on contours that do not touch is unchanged, wherever they
    // start: three nested levels, an island in the counter, and a separate
    // contour.
    let paths = vec![
        with_id(rect(0.0, 0.0, 1000.0, 1000.0, true), 0),
        with_id(rect(100.0, 100.0, 900.0, 900.0, false), 1),
        with_id(circle(500.0, 500.0, 200.0, true), 2),
        with_id(circle(500.0, 500.0, 100.0, false), 3),
        with_id(rect(2000.0, 0.0, 2100.0, 100.0, true), 4),
    ];
    assert_eq!(contour_depths(&paths), vec![0, 1, 2, 3, 0]);
    assert_eq!(wrong_directions(&paths), vec![]);
}

#[test]
fn counter_hanging_from_the_outer_edge_is_inner() {
    // A clockwise counter whose top side lies on the top edge of the outer
    // square, drawn as 40 edges of 20 units starting on that edge: the
    // longest edges of the counter all lie on the outer contour, so every
    // probe next to them is ambiguous and the shorter edges must be tried.
    let outer = with_id(rect(0.0, 0.0, 1000.0, 1000.0, true), 0);
    let mut nodes = Vec::new();
    for i in 0..40 {
        nodes.push(line(100.0 + 20.0 * f64::from(i), 1000.0)); // top, to the right
    }
    for i in 0..10 {
        nodes.push(line(900.0, 1000.0 - 10.0 * f64::from(i))); // right side, down
    }
    for i in 0..80 {
        nodes.push(line(900.0 - 10.0 * f64::from(i), 900.0)); // bottom, to the left
    }
    for i in 0..10 {
        nodes.push(line(100.0, 900.0 + 10.0 * f64::from(i))); // left side, up
    }
    let counter = with_id(closed(nodes), 1);
    assert!(area(&counter) < 0.0, "the counter is drawn clockwise");
    let paths = vec![outer, counter];

    assert_eq!(contour_depths(&paths), vec![0, 1]);
    assert_eq!(wrong_directions(&paths), vec![]);
    assert_eq!(fix_paths(&paths, &[IssueKind::WrongDirection], 0.0), paths);
}
