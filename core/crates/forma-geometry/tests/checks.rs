//! Integration tests for the geometry checks and automatic fixes.

use forma_geometry::{
    GeometryIssue, IssueKind, bbox, check_paths, fix_paths, normalize_direction, signed_area,
    to_bezpath,
};
use forma_model::{Node, NodeKind, Path, PathId};
use kurbo::{BezPath, ParamCurve, ParamCurveNearest, Point};

// ---------------------------------------------------------------- helpers

const ALL_FIXABLE: [IssueKind; 4] = [
    IssueKind::DuplicatePoints,
    IssueKind::MissingExtrema,
    IssueKind::OffGrid,
    IssueKind::WrongDirection,
];

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

/// A circle of four cubic segments with nodes at the extrema, starting at
/// `(cx + r, cy)`. Off-curve nodes sit at distance `h` from their node, so
/// integer arguments give integer coordinates.
fn circle(id: u32, cx: f64, cy: f64, r: f64, h: f64, ccw: bool) -> Path {
    let s = if ccw { 1.0 } else { -1.0 };
    let p = |dx: f64, dy: f64| (cx + dx, cy + s * dy);
    let n = |(x, y): (f64, f64), kind| node(x, y, kind);
    closed(
        id,
        vec![
            n(p(r, 0.0), NodeKind::Curve),
            n(p(r, h), NodeKind::OffCurve),
            n(p(h, r), NodeKind::OffCurve),
            n(p(0.0, r), NodeKind::Curve),
            n(p(-h, r), NodeKind::OffCurve),
            n(p(-r, h), NodeKind::OffCurve),
            n(p(-r, 0.0), NodeKind::Curve),
            n(p(-r, -h), NodeKind::OffCurve),
            n(p(-h, -r), NodeKind::OffCurve),
            n(p(0.0, -r), NodeKind::Curve),
            n(p(h, -r), NodeKind::OffCurve),
            n(p(r, -h), NodeKind::OffCurve),
        ],
    )
}

/// A counter-clockwise contour whose cubic top edge runs from `(100, 0)` to
/// `(0, 0)` and peaks at `0.75 * height` without a node there.
fn hump(id: u32, height: f64) -> Path {
    closed(
        id,
        vec![
            line(100.0, 0.0),
            off(100.0, height),
            off(0.0, height),
            curve(0.0, 0.0),
        ],
    )
}

/// A counter-clockwise contour whose cubic right edge runs from `(0, 0)` to
/// `(0, 100)` and bulges to `x = 60` without a node there.
fn bulge(id: u32) -> Path {
    closed(
        id,
        vec![
            line(0.0, 0.0),
            off(80.0, 0.0),
            off(80.0, 100.0),
            curve(0.0, 100.0),
        ],
    )
}

/// A counter-clockwise contour with a quadratic top edge peaking at `y = 50`.
fn quad_hump(id: u32) -> Path {
    closed(
        id,
        vec![line(100.0, 0.0), off(50.0, 100.0), qcurve(0.0, 0.0)],
    )
}

fn translate(mut path: Path, dx: f64, dy: f64) -> Path {
    for n in &mut path.nodes {
        n.x += dx;
        n.y += dy;
    }
    path
}

fn kinds_of(issues: &[GeometryIssue]) -> Vec<IssueKind> {
    issues.iter().map(|i| i.kind).collect()
}

fn issues_of(paths: &[Path], grid: f64, kind: IssueKind) -> Vec<GeometryIssue> {
    check_paths(paths, grid)
        .into_iter()
        .filter(|i| i.kind == kind)
        .collect()
}

fn coords(path: &Path) -> Vec<(f64, f64)> {
    path.nodes.iter().map(|n| (n.x, n.y)).collect()
}

fn on_curve_count(path: &Path) -> usize {
    path.nodes
        .iter()
        .filter(|n| n.kind != NodeKind::OffCurve)
        .count()
}

fn ids(paths: &[Path]) -> Vec<PathId> {
    paths.iter().map(|p| p.id).collect()
}

fn bez(path: &Path) -> BezPath {
    to_bezpath(path).expect("valid contour")
}

fn assert_bbox_close(before: &[Path], after: &[Path]) {
    let (b, a) = (bbox(before).expect("bbox"), bbox(after).expect("bbox"));
    let close = |x: f64, y: f64| (x - y).abs() < 0.01;
    assert!(
        close(a.x0, b.x0) && close(a.y0, b.y0) && close(a.x1, b.x1) && close(a.y1, b.y1),
        "bbox changed from {b:?} to {a:?}"
    );
}

/// The largest distance from sample points of `before` to the outline `after`.
fn max_deviation(before: &Path, after: &Path) -> f64 {
    let target = bez(after);
    let mut worst: f64 = 0.0;
    for segment in bez(before).segments() {
        for i in 0..=20 {
            let p: Point = segment.eval(f64::from(i) / 20.0);
            let nearest = target
                .segments()
                .map(|s| s.nearest(p, 1e-12).distance_sq)
                .fold(f64::INFINITY, f64::min);
            worst = worst.max(nearest.sqrt());
        }
    }
    worst
}

/// A tiny deterministic generator, so the randomized tests need no crate.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn coord(&mut self) -> f64 {
        (self.next() * 400.0).round() / 2.0
    }

    /// A coordinate in `0.0..300.0` with one decimal.
    fn fine(&mut self) -> f64 {
        (self.next() * 3000.0).round() / 10.0
    }
}

/// A random closed contour with one cubic and one closing line.
fn random_cubic(rng: &mut Lcg, id: u32) -> Path {
    closed(
        id,
        vec![
            line(rng.coord(), rng.coord()),
            off(rng.coord(), rng.coord()),
            off(rng.coord(), rng.coord()),
            curve(rng.coord(), rng.coord()),
        ],
    )
}

/// A random closed contour with one quadratic and one closing line.
fn random_quad(rng: &mut Lcg, id: u32) -> Path {
    closed(
        id,
        vec![
            line(rng.coord(), rng.coord()),
            off(rng.coord(), rng.coord()),
            qcurve(rng.coord(), rng.coord()),
        ],
    )
}

/// A random closed contour of two to four segments of random kinds, with
/// fractional coordinates.
fn random_mixed(rng: &mut Lcg, id: u32) -> Path {
    let mut nodes = vec![line(rng.fine(), rng.fine())];
    for _ in 0..2 + (rng.next() * 3.0) as usize {
        match (rng.next() * 3.0) as usize {
            0 => nodes.push(line(rng.fine(), rng.fine())),
            1 => {
                nodes.push(off(rng.fine(), rng.fine()));
                nodes.push(qcurve(rng.fine(), rng.fine()));
            }
            _ => {
                nodes.push(off(rng.fine(), rng.fine()));
                nodes.push(off(rng.fine(), rng.fine()));
                nodes.push(curve(rng.fine(), rng.fine()));
            }
        }
    }
    closed(id, nodes)
}

// ------------------------------------------------------------- detection

#[test]
fn detects_each_issue_kind() {
    let fixtures = vec![
        // 1: open contour.
        open(
            1,
            vec![line(0.0, 0.0), line(100.0, 0.0), line(100.0, 100.0)],
        ),
        // 2: clockwise outer contour.
        square(2, 0.0, 0.0, 100.0, 100.0, false),
        // 3: the node at index 2 repeats the one before it.
        closed(
            3,
            vec![
                line(0.0, 0.0),
                line(100.0, 0.0),
                line(100.0, 0.0),
                line(100.0, 100.0),
                line(0.0, 100.0),
            ],
        ),
        // 4: the segment arriving at node 3 is 0.5 units long.
        closed(
            4,
            vec![
                line(0.0, 0.0),
                line(100.0, 0.0),
                line(100.0, 99.5),
                line(100.0, 100.0),
                line(0.0, 100.0),
            ],
        ),
        // 5: node 2 is off the 0.5 grid.
        closed(
            5,
            vec![
                line(0.0, 0.0),
                line(100.0, 0.0),
                line(100.25, 100.0),
                line(0.0, 100.0),
            ],
        ),
        // 6: the cubic peaks at y = 60 without a node.
        hump(6, 80.0),
        // 7: a line preceded by an off-curve node cannot be converted.
        closed(
            7,
            vec![
                line(0.0, 0.0),
                off(10.0, 10.0),
                line(20.0, 0.0),
                line(10.0, -10.0),
            ],
        ),
    ];
    let paths: Vec<Path> = fixtures
        .into_iter()
        .map(|p| {
            let dx = 1000.0 * f64::from(p.id.0);
            translate(p, dx, 0.0)
        })
        .collect();

    let issues = check_paths(&paths, 0.5);
    let summary: Vec<(IssueKind, u32, Option<usize>, bool)> = issues
        .iter()
        .map(|i| (i.kind, i.path.0, i.node, i.fixable))
        .collect();
    assert_eq!(
        summary,
        vec![
            (IssueKind::OpenContour, 1, None, false),
            (IssueKind::WrongDirection, 2, None, true),
            (IssueKind::DuplicatePoints, 3, Some(2), true),
            (IssueKind::TinySegment, 4, Some(3), false),
            (IssueKind::OffGrid, 5, Some(2), true),
            (IssueKind::MissingExtrema, 6, Some(3), true),
            (IssueKind::InvalidContour, 7, None, false),
        ]
    );
    assert!(issues.iter().all(|i| !i.message.is_empty()));
}

#[test]
fn clean_paths_have_no_issues() {
    // An "o": counter-clockwise outer circle with a clockwise counter, nodes
    // at every extremum and integer coordinates; a square; and a hump whose
    // top already has a node at its extremum.
    let clean_hump = closed(
        4,
        vec![
            line(100.0, 0.0),
            off(100.0, 40.0),
            off(75.0, 60.0),
            curve(50.0, 60.0),
            off(25.0, 60.0),
            off(0.0, 40.0),
            curve(0.0, 0.0),
        ],
    );
    let paths = vec![
        circle(1, 200.0, 200.0, 100.0, 55.0, true),
        circle(2, 200.0, 200.0, 50.0, 28.0, false),
        square(3, 500.0, 0.0, 600.0, 100.0, true),
        translate(clean_hump, 800.0, 0.0),
    ];
    assert!(check_paths(&paths, 1.0).is_empty(), "grid 1");
    assert!(check_paths(&paths, 0.0).is_empty(), "no grid");

    // Fractional curves are fine too when no grid is requested.
    let kappa = 0.552_284_749_830_793_4;
    let round = vec![
        circle(1, 0.0, 0.0, 100.0, 100.0 * kappa, true),
        circle(2, 0.0, 0.0, 40.0, 40.0 * kappa, false),
    ];
    assert!(check_paths(&round, 0.0).is_empty());
}

#[test]
fn empty_input_has_no_issues() {
    assert!(check_paths(&[], 1.0).is_empty());
    assert!(fix_paths(&[], &ALL_FIXABLE, 1.0).is_empty());
}

#[test]
fn issue_kinds_serialize_as_snake_case() {
    let expected = [
        (IssueKind::OpenContour, "open_contour"),
        (IssueKind::WrongDirection, "wrong_direction"),
        (IssueKind::DuplicatePoints, "duplicate_points"),
        (IssueKind::TinySegment, "tiny_segment"),
        (IssueKind::OffGrid, "off_grid"),
        (IssueKind::MissingExtrema, "missing_extrema"),
        (IssueKind::InvalidContour, "invalid_contour"),
    ];
    for (kind, name) in expected {
        let json = serde_json::to_string(&kind).expect("serialize");
        assert_eq!(json, format!("\"{name}\""));
        let back: IssueKind = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, kind);
    }
    // They can key sets and maps, and describe themselves for tool schemas.
    let set: std::collections::BTreeSet<IssueKind> = expected.iter().map(|(k, _)| *k).collect();
    assert_eq!(set.len(), 7);
    let schema = serde_json::to_string(&schemars::schema_for!(IssueKind)).expect("schema");
    assert!(schema.contains("off_grid"));
}

#[test]
fn issues_round_trip_through_json() {
    let issues = check_paths(&[square(4, 0.0, 0.0, 10.0, 10.0, false)], 0.0);
    assert_eq!(issues.len(), 1);
    let json = serde_json::to_value(&issues[0]).expect("serialize");
    assert_eq!(json["kind"], "wrong_direction");
    assert_eq!(json["path"], 4);
    assert!(json["node"].is_null());
    assert_eq!(json["fixable"], true);
    let back: GeometryIssue = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back, issues[0]);
}

#[test]
fn open_contours_are_reported_but_never_wrongly_directed() {
    let open_path = open(
        1,
        vec![line(0.0, 0.0), line(100.0, 0.0), line(100.0, 100.0)],
    );
    let issues = check_paths(&[open_path], 0.0);
    assert_eq!(kinds_of(&issues), vec![IssueKind::OpenContour]);
    assert!(!issues[0].fixable);
}

// ---------------------------------------------------------- wrong direction

#[test]
fn wrong_direction_follows_nesting_depth() {
    // Outer and inner both counter-clockwise: the inner one is wrong.
    let paths = vec![
        square(1, 0.0, 0.0, 100.0, 100.0, true),
        square(2, 25.0, 25.0, 75.0, 75.0, true),
    ];
    let issues = check_paths(&paths, 0.0);
    assert_eq!(kinds_of(&issues), vec![IssueKind::WrongDirection]);
    assert_eq!(issues[0].path, PathId(2));

    // Outer and inner both clockwise: the outer one is wrong.
    let paths = vec![
        square(1, 0.0, 0.0, 100.0, 100.0, false),
        square(2, 25.0, 25.0, 75.0, 75.0, false),
    ];
    let issues = check_paths(&paths, 0.0);
    assert_eq!(kinds_of(&issues), vec![IssueKind::WrongDirection]);
    assert_eq!(issues[0].path, PathId(1));

    // Three levels deep: counter-clockwise, clockwise, counter-clockwise.
    let paths = vec![
        square(1, 0.0, 0.0, 100.0, 100.0, true),
        square(2, 10.0, 10.0, 90.0, 90.0, false),
        square(3, 20.0, 20.0, 80.0, 80.0, true),
    ];
    assert!(check_paths(&paths, 0.0).is_empty());
}

#[test]
fn zero_area_contours_are_never_wrongly_directed() {
    // Collinear points enclose nothing.
    let flat = closed(1, vec![line(0.0, 0.0), line(50.0, 0.0), line(100.0, 0.0)]);
    assert_eq!(signed_area(&flat), 0.0);
    assert!(
        !kinds_of(&check_paths(&[flat], 0.0)).contains(&IssueKind::WrongDirection),
        "zero area has no direction"
    );
}

#[test]
fn near_zero_area_contours_have_no_direction() {
    // A degenerate sliver: almost collinear, its area (about -1e-10) is
    // float noise that a reversal cannot be trusted to flip, so flagging it
    // would make an agent loop on geometry_fix.
    let sliver = closed(
        1,
        vec![line(0.0, 0.0), line(1000.0, 1e-13), line(2000.0, 0.0)],
    );
    let area = signed_area(&sliver);
    assert!(area < 0.0 && area.abs() < 1e-9, "{area}");
    assert!(
        !kinds_of(&check_paths(std::slice::from_ref(&sliver), 0.0))
            .contains(&IssueKind::WrongDirection),
        "below 1e-9 absolute"
    );

    // A bow tie whose lobes almost cancel: tiny next to its bounding box.
    let bow_tie = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 100.0),
            line(100.0, 0.0),
            line(0.0, 100.0001),
        ],
    );
    let area = signed_area(&bow_tie);
    assert!(area != 0.0 && area.abs() < 1e-6 * 100.0 * 100.0, "{area}");
    assert!(
        !kinds_of(&check_paths(std::slice::from_ref(&bow_tie), 0.0))
            .contains(&IssueKind::WrongDirection),
        "below 1e-6 of the bounding box"
    );
    // Nothing for geometry_fix to do with them either.
    let paths = vec![sliver, bow_tie];
    assert_eq!(fix_paths(&paths, &[IssueKind::WrongDirection], 0.0), paths);

    // A thin but real triangle keeps its direction check.
    let thin = closed(1, vec![line(0.0, 0.0), line(0.0, 1.0), line(1000.0, 0.5)]);
    assert!(signed_area(&thin) < 0.0);
    assert!(
        kinds_of(&check_paths(&[thin], 0.0)).contains(&IssueKind::WrongDirection),
        "a clockwise outer contour"
    );
}

// --------------------------------------------------------- duplicate points

#[test]
fn duplicate_points_wrap_around_closed_contours() {
    // The closing segment arrives at node 0 from the identical node 4.
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
            line(0.0, 0.0),
        ],
    );
    let issues = issues_of(&[path], 0.0, IssueKind::DuplicatePoints);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].node, Some(0));
    assert!(issues[0].fixable);
}

#[test]
fn duplicate_points_need_coinciding_handles() {
    // A loop that leaves a point and returns to it is not a duplicate.
    let looped = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(180.0, 50.0),
            off(180.0, -50.0),
            curve(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    assert!(issues_of(&[looped], 0.0, IssueKind::DuplicatePoints).is_empty());

    // The same shape with the handles collapsed onto the point is one.
    let collapsed = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(100.0, 0.0),
            off(100.0, 0.0),
            curve(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    let issues = issues_of(&[collapsed], 0.0, IssueKind::DuplicatePoints);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].node, Some(4));
}

#[test]
fn zero_length_handles_are_not_duplicate_points() {
    // A handle sitting on its own node is not a duplicate on-curve node.
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(100.0, 0.0),
            off(100.0, 50.0),
            curve(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    assert!(issues_of(&[path], 0.0, IssueKind::DuplicatePoints).is_empty());
}

#[test]
fn duplicate_points_that_would_collapse_a_contour_are_not_fixable() {
    // Removing a node would leave a closed contour with a single on-curve node.
    let path = closed(1, vec![line(10.0, 10.0), line(10.0, 10.0)]);
    let issues = issues_of(std::slice::from_ref(&path), 0.0, IssueKind::DuplicatePoints);
    assert!(!issues.is_empty());
    assert!(issues.iter().all(|i| !i.fixable));
    assert_eq!(
        fix_paths(
            std::slice::from_ref(&path),
            &[IssueKind::DuplicatePoints],
            0.0
        ),
        vec![path]
    );

    // Three identical nodes: one removal is allowed, the rest are reported.
    let triple = closed(2, vec![line(5.0, 5.0), line(5.0, 5.0), line(5.0, 5.0)]);
    let issues = issues_of(
        std::slice::from_ref(&triple),
        0.0,
        IssueKind::DuplicatePoints,
    );
    assert_eq!(issues.iter().filter(|i| i.fixable).count(), 1);
    let fixed = fix_paths(&[triple], &[IssueKind::DuplicatePoints], 0.0);
    assert_eq!(on_curve_count(&fixed[0]), 2);
}

// ------------------------------------------------------------- tiny segment

#[test]
fn tiny_segment_threshold_is_one_unit() {
    let with_edge = |length: f64| {
        closed(
            1,
            vec![
                line(0.0, 0.0),
                line(100.0, 0.0),
                line(100.0, 100.0 - length),
                line(100.0, 100.0),
                line(0.0, 100.0),
            ],
        )
    };
    let tiny = issues_of(&[with_edge(0.75)], 0.0, IssueKind::TinySegment);
    assert_eq!(tiny.len(), 1);
    assert_eq!(tiny[0].node, Some(3));
    assert!(!tiny[0].fixable);
    assert!(issues_of(&[with_edge(1.0)], 0.0, IssueKind::TinySegment).is_empty());
    assert!(issues_of(&[with_edge(1.5)], 0.0, IssueKind::TinySegment).is_empty());
}

#[test]
fn tiny_curved_segments_and_closing_segments_are_reported() {
    // A short cubic.
    let cubic = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(100.2, 0.1),
            off(100.4, 0.2),
            curve(100.6, 0.3),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    let issues = issues_of(&[cubic], 0.0, IssueKind::TinySegment);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].node, Some(4));

    // The closing segment arrives at node 0.
    let closing = closed(
        2,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
            line(0.0, 0.5),
        ],
    );
    let issues = issues_of(&[closing], 0.0, IssueKind::TinySegment);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].node, Some(0));
}

#[test]
fn zero_length_segments_are_duplicates_not_tiny() {
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    let kinds = kinds_of(&check_paths(&[path], 0.0));
    assert_eq!(kinds, vec![IssueKind::DuplicatePoints]);
}

// ----------------------------------------------------------------- off grid

#[test]
fn off_grid_is_only_checked_with_a_positive_grid() {
    let path = closed(
        1,
        vec![
            line(0.3, 0.0),
            line(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    assert!(check_paths(std::slice::from_ref(&path), 0.0).is_empty());
    assert!(check_paths(std::slice::from_ref(&path), -1.0).is_empty());
    assert_eq!(
        kinds_of(&check_paths(std::slice::from_ref(&path), 1.0)),
        vec![IssueKind::OffGrid]
    );
}

#[test]
fn off_grid_reports_one_issue_per_contour_at_the_first_offending_node() {
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.5),
            line(100.5, 100.5),
            line(0.0, 100.0),
        ],
    );
    let issues = check_paths(&[path], 1.0);
    assert_eq!(kinds_of(&issues), vec![IssueKind::OffGrid]);
    assert_eq!(issues[0].node, Some(1));
    assert!(issues[0].fixable);
}

#[test]
fn off_grid_uses_a_tolerance_of_1e_9() {
    // 0.1 + 0.2 is not exactly 0.3, but is within tolerance of the grid.
    let on = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(0.1 + 0.2, 0.0),
            line(0.1 + 0.2, 0.3),
            line(0.0, 0.3),
        ],
    );
    assert!(issues_of(&[on], 0.1, IssueKind::OffGrid).is_empty());

    let near = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0 + 1e-8, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    assert_eq!(issues_of(&[near], 1.0, IssueKind::OffGrid).len(), 1);
}

#[test]
fn off_grid_checks_off_curve_nodes_too() {
    let path = closed(
        1,
        vec![
            line(100.0, 0.0),
            off(100.0, 80.0),
            off(0.0, 80.5),
            curve(0.0, 0.0),
        ],
    );
    let issues = issues_of(&[path], 1.0, IssueKind::OffGrid);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].node, Some(2));
}

// --------------------------------------------------------- missing extrema

#[test]
fn missing_extrema_in_y_and_x_and_quadratics() {
    for (path, node) in [(hump(1, 80.0), 3), (bulge(1), 3), (quad_hump(1), 2)] {
        let issues = issues_of(std::slice::from_ref(&path), 0.0, IssueKind::MissingExtrema);
        assert_eq!(issues.len(), 1, "{path:?}");
        assert_eq!(issues[0].node, Some(node));
        assert!(issues[0].fixable);
    }
}

#[test]
fn small_overshoots_are_not_missing_extrema() {
    // Peak at 0.75 * 0.6 = 0.45: within the 0.5 allowance.
    assert!(issues_of(&[hump(1, 0.6)], 0.0, IssueKind::MissingExtrema).is_empty());
    // Peak at 0.75 * 0.8 = 0.6: beyond it.
    assert_eq!(
        issues_of(&[hump(1, 0.8)], 0.0, IssueKind::MissingExtrema).len(),
        1
    );
}

#[test]
fn extrema_in_the_outer_one_percent_of_a_segment_are_ignored() {
    // x dips to -1 at t = 0.005, close to the start of the segment.
    let path = closed(
        1,
        vec![line(0.0, 0.0), off(-200.0, 0.0), qcurve(39_600.0, 0.0)],
    );
    assert!(issues_of(&[path], 0.0, IssueKind::MissingExtrema).is_empty());
}

#[test]
fn monotonic_curves_have_no_missing_extrema() {
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            off(30.0, 0.0),
            off(100.0, 70.0),
            curve(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    assert!(issues_of(&[path], 0.0, IssueKind::MissingExtrema).is_empty());
}

#[test]
fn one_issue_is_reported_per_segment_even_with_several_extrema() {
    // An S-shaped edge with a maximum and a minimum in y.
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            off(30.0, 100.0),
            off(70.0, -100.0),
            curve(100.0, 0.0),
            line(100.0, -200.0),
            line(0.0, -200.0),
        ],
    );
    let issues = issues_of(&[path], 0.0, IssueKind::MissingExtrema);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].node, Some(3));
}

// ---------------------------------------------------------- invalid contours

#[test]
fn checks_skip_invalid_contours_except_invalid_contour_issue() {
    let bad_line = closed(
        1,
        vec![
            line(0.3, 0.7),
            off(10.5, 10.5),
            line(20.7, 0.1),
            line(10.0, -10.0),
        ],
    );
    let empty = closed(2, Vec::new());
    let leading_off_curve = open(3, vec![off(0.5, 0.5), line(10.5, 0.5), line(10.5, 10.5)]);
    let not_finite = closed(
        4,
        vec![line(0.0, 0.0), line(f64::NAN, 0.0), line(100.0, 100.0)],
    );
    let too_few = closed(5, vec![line(0.2, 0.2)]);
    let paths = vec![bad_line, empty, leading_off_curve, not_finite, too_few];

    let issues = check_paths(&paths, 1.0);
    assert_eq!(issues.len(), paths.len(), "{issues:?}");
    for (issue, path) in issues.iter().zip(&paths) {
        assert_eq!(issue.kind, IssueKind::InvalidContour);
        assert_eq!(issue.path, path.id);
        assert_eq!(issue.node, None);
        assert!(!issue.fixable);
        let error = to_bezpath(path).expect_err("invalid");
        assert_eq!(issue.message, error.to_string());
    }
}

#[test]
fn invalid_contours_do_not_disturb_valid_neighbours() {
    // The invalid contour surrounds the square but is ignored for nesting.
    let invalid = closed(
        1,
        vec![
            line(-50.0, -50.0),
            off(10.0, 10.0),
            line(200.0, -50.0),
            line(200.0, 200.0),
        ],
    );
    let inner = square(2, 0.0, 0.0, 100.0, 100.0, true);
    let issues = check_paths(&[invalid, inner], 0.0);
    assert_eq!(kinds_of(&issues), vec![IssueKind::InvalidContour]);
}

// ------------------------------------------------------------------- fixing

#[test]
fn fix_wrong_direction() {
    let paths = vec![
        square(1, 0.0, 0.0, 100.0, 100.0, false), // outer, clockwise: wrong
        square(2, 25.0, 25.0, 75.0, 75.0, true),  // counter, counter-clockwise: wrong
        translate(square(3, 0.0, 0.0, 50.0, 50.0, true), 500.0, 0.0), // fine
    ];
    let fixed = fix_paths(&paths, &[IssueKind::WrongDirection], 0.0);

    assert!(signed_area(&fixed[0]) > 0.0);
    assert!(signed_area(&fixed[1]) < 0.0);
    assert_eq!(fixed[2], paths[2], "a correct contour is untouched");
    assert!(check_paths(&fixed, 0.0).is_empty());
    assert_bbox_close(&paths, &fixed);

    // It is the very reversal that `normalize_direction` performs.
    let mut expected = paths.clone();
    normalize_direction(&mut expected);
    assert_eq!(fixed, expected);
}

#[test]
fn fix_wrong_direction_keeps_curves_and_smooth_flags() {
    let mut path = circle(1, 100.0, 100.0, 50.0, 28.0, false);
    path.nodes[3].smooth = true;
    let fixed = fix_paths(
        std::slice::from_ref(&path),
        &[IssueKind::WrongDirection],
        0.0,
    );
    assert!(signed_area(&fixed[0]) > 0.0);
    assert_eq!(fixed[0].nodes.len(), path.nodes.len());
    assert_eq!(fixed[0].nodes.iter().filter(|n| n.smooth).count(), 1);
    assert!(max_deviation(&path, &fixed[0]) < 1e-6);
}

#[test]
fn fix_adds_extrema_and_preserves_shape() {
    for path in [hump(1, 80.0), bulge(2), quad_hump(3)] {
        let before = vec![path];
        let fixed = fix_paths(&before, &[IssueKind::MissingExtrema], 0.0);

        assert_bbox_close(&before, &fixed);
        assert!(
            fixed[0].nodes.len() > before[0].nodes.len(),
            "no node was added to {:?}",
            before[0]
        );
        assert!(max_deviation(&before[0], &fixed[0]) < 1e-6);
        assert!(max_deviation(&fixed[0], &before[0]) < 1e-6);
        assert!(issues_of(&fixed, 0.0, IssueKind::MissingExtrema).is_empty());
        assert_eq!(fixed[0].id, before[0].id);
        assert!(fixed[0].closed);
    }
}

#[test]
fn fix_inserts_smooth_nodes_of_the_segment_kind() {
    let fixed = fix_paths(&[hump(1, 80.0)], &[IssueKind::MissingExtrema], 0.0);
    // line, off, off, curve (new), off, off, curve (original)
    let kinds: Vec<NodeKind> = fixed[0].nodes.iter().map(|n| n.kind).collect();
    assert_eq!(
        kinds,
        vec![
            NodeKind::Line,
            NodeKind::OffCurve,
            NodeKind::OffCurve,
            NodeKind::Curve,
            NodeKind::OffCurve,
            NodeKind::OffCurve,
            NodeKind::Curve,
        ]
    );
    let new = &fixed[0].nodes[3];
    assert!(new.smooth);
    assert!((new.x - 50.0).abs() < 1e-9 && (new.y - 60.0).abs() < 1e-9);
    // The original end node is kept as it was.
    assert_eq!(fixed[0].nodes[6], hump(1, 80.0).nodes[3]);

    let quad = fix_paths(&[quad_hump(1)], &[IssueKind::MissingExtrema], 0.0);
    let kinds: Vec<NodeKind> = quad[0].nodes.iter().map(|n| n.kind).collect();
    assert_eq!(
        kinds,
        vec![
            NodeKind::Line,
            NodeKind::OffCurve,
            NodeKind::QCurve,
            NodeKind::OffCurve,
            NodeKind::QCurve,
        ]
    );
    assert!(quad[0].nodes[2].smooth);
}

#[test]
fn fix_splits_at_every_overshooting_extremum() {
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            off(30.0, 100.0),
            off(70.0, -100.0),
            curve(100.0, 0.0),
            line(100.0, -200.0),
            line(0.0, -200.0),
        ],
    );
    let fixed = fix_paths(
        std::slice::from_ref(&path),
        &[IssueKind::MissingExtrema],
        0.0,
    );
    assert_eq!(on_curve_count(&fixed[0]), on_curve_count(&path) + 2);
    assert!(max_deviation(&path, &fixed[0]) < 1e-6);
    assert!(issues_of(&fixed, 0.0, IssueKind::MissingExtrema).is_empty());
}

#[test]
fn fix_extrema_leaves_the_node_layout_of_untouched_contours_alone() {
    // A closed contour that starts with off-curve nodes keeps its layout when
    // nothing needs fixing.
    let path = closed(
        1,
        vec![
            off(100.0, 55.0),
            off(55.0, 100.0),
            curve(0.0, 100.0),
            line(0.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 1.0),
        ],
    );
    let fixed = fix_paths(
        std::slice::from_ref(&path),
        &[IssueKind::MissingExtrema, IssueKind::OffGrid],
        1.0,
    );
    assert_eq!(fixed, vec![path]);
}

#[test]
fn fix_extrema_works_on_the_closing_segment_of_a_contour_starting_with_handles() {
    // The cubic arriving at the first on-curve node is split across the wrap.
    let path = closed(
        1,
        vec![
            off(0.0, 80.0),
            off(100.0, 80.0),
            curve(100.0, 0.0),
            line(0.0, 0.0),
        ],
    );
    // Segment: (0,0) -> (100,0) with handles (0,80), (100,80) peaks at y = 60.
    let issues = issues_of(std::slice::from_ref(&path), 0.0, IssueKind::MissingExtrema);
    assert_eq!(issues.len(), 1);
    let fixed = fix_paths(
        std::slice::from_ref(&path),
        &[IssueKind::MissingExtrema],
        0.0,
    );
    assert_eq!(on_curve_count(&fixed[0]), 3);
    assert!(max_deviation(&path, &fixed[0]) < 1e-6);
    assert!(max_deviation(&fixed[0], &path) < 1e-6);
    assert!(
        check_paths(&fixed, 0.0)
            .iter()
            .all(|i| i.kind != IssueKind::MissingExtrema)
    );
}

#[test]
fn fix_extrema_converges_on_random_curves() {
    let mut rng = Lcg(0x5EED);
    let mut paths = Vec::new();
    for i in 0..300 {
        paths.push(random_cubic(&mut rng, 2 * i));
        paths.push(random_quad(&mut rng, 2 * i + 1));
    }
    // Plenty of them really lack extrema, so this is not vacuous.
    let flagged = issues_of(&paths, 0.0, IssueKind::MissingExtrema).len();
    assert!(
        flagged > 100,
        "only {flagged} of {} are flagged",
        paths.len()
    );
    let fixed = fix_paths(&paths, &[IssueKind::MissingExtrema], 0.0);
    assert_eq!(ids(&fixed), ids(&paths));
    for (before, after) in paths.iter().zip(&fixed) {
        assert!(
            issues_of(std::slice::from_ref(after), 0.0, IssueKind::MissingExtrema).is_empty(),
            "{before:?} -> {after:?}"
        );
        assert!(max_deviation(before, after) < 1e-6, "{before:?}");
        assert!(max_deviation(after, before) < 1e-6, "{before:?}");
        assert_bbox_close(std::slice::from_ref(before), std::slice::from_ref(after));
    }
    // And a second pass changes nothing.
    assert_eq!(fix_paths(&fixed, &[IssueKind::MissingExtrema], 0.0), fixed);
}

#[test]
fn fix_duplicate_points_removes_the_redundant_node() {
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    let fixed = fix_paths(&[path], &[IssueKind::DuplicatePoints], 0.0);
    assert_eq!(
        coords(&fixed[0]),
        vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
    );
    assert!(check_paths(&fixed, 0.0).is_empty());
}

#[test]
fn fix_duplicate_points_removes_the_handles_of_the_zero_length_segment() {
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(100.0, 0.0),
            off(100.0, 0.0),
            curve(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    let fixed = fix_paths(&[path], &[IssueKind::DuplicatePoints], 0.0);
    assert_eq!(
        coords(&fixed[0]),
        vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
    );
    assert!(fixed[0].nodes.iter().all(|n| n.kind == NodeKind::Line));
}

#[test]
fn fix_duplicate_points_across_the_wrap_keeps_the_shape() {
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
            line(0.0, 0.0),
        ],
    );
    let fixed = fix_paths(
        std::slice::from_ref(&path),
        &[IssueKind::DuplicatePoints],
        0.0,
    );
    assert_eq!(fixed[0].nodes.len(), 4);
    assert!(signed_area(&fixed[0]) > 0.0);
    assert_bbox_close(std::slice::from_ref(&path), &fixed);
    assert!(check_paths(&fixed, 0.0).is_empty());
}

#[test]
fn fix_duplicate_points_keeps_curves_that_follow_the_removed_node() {
    // The curve arriving at (100, 100) starts at the duplicated point.
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 0.0),
            off(100.0, 50.0),
            off(80.0, 100.0),
            curve(50.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    let fixed = fix_paths(
        std::slice::from_ref(&path),
        &[IssueKind::DuplicatePoints],
        0.0,
    );
    assert_eq!(fixed[0].nodes.len(), 6);
    assert_eq!(fixed[0].nodes[2].kind, NodeKind::OffCurve);
    assert!(max_deviation(&path, &fixed[0]) < 1e-6);
    assert!(max_deviation(&fixed[0], &path) < 1e-6);
}

#[test]
fn fix_duplicate_points_in_open_contours() {
    let path = open(
        1,
        vec![
            line(0.0, 0.0),
            line(0.0, 0.0),
            line(50.0, 0.0),
            line(50.0, 50.0),
        ],
    );
    let fixed = fix_paths(&[path], &[IssueKind::DuplicatePoints], 0.0);
    assert_eq!(
        coords(&fixed[0]),
        vec![(0.0, 0.0), (50.0, 0.0), (50.0, 50.0)]
    );
    assert!(!fixed[0].closed);
}

#[test]
fn fix_off_grid_rounds_only_the_flagged_contours() {
    let fractional = closed(
        1,
        vec![
            line(0.4, 0.0),
            line(99.6, 0.0),
            line(100.4, 100.0),
            line(-0.4, 100.0),
        ],
    );
    let on_grid = translate(square(2, 0.0, 0.0, 50.0, 50.0, true), 500.0, 0.0);
    let paths = vec![fractional, on_grid];
    let fixed = fix_paths(&paths, &[IssueKind::OffGrid], 1.0);

    assert_eq!(
        coords(&fixed[0]),
        vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
    );
    assert_eq!(fixed[1], paths[1]);
    assert!(check_paths(&fixed, 1.0).is_empty());

    // A coarser grid snaps to its multiples.
    let coarse = fix_paths(&paths, &[IssueKind::OffGrid], 10.0);
    assert!(
        coarse[0]
            .nodes
            .iter()
            .all(|n| n.x % 10.0 == 0.0 && n.y % 10.0 == 0.0)
    );
}

#[test]
fn fix_off_grid_does_not_produce_negative_zero() {
    let path = closed(
        1,
        vec![
            line(-0.2, 0.0),
            line(100.0, -0.1),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    let fixed = fix_paths(&[path], &[IssueKind::OffGrid], 1.0);
    for n in &fixed[0].nodes {
        assert!(n.x.is_sign_positive() && n.y.is_sign_positive(), "{n:?}");
    }
}

#[test]
fn fix_off_grid_needs_a_positive_grid() {
    let path = closed(
        1,
        vec![
            line(0.4, 0.0),
            line(100.0, 0.0),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    for grid in [0.0, -1.0, f64::NAN] {
        assert_eq!(
            fix_paths(std::slice::from_ref(&path), &[IssueKind::OffGrid], grid),
            vec![path.clone()]
        );
    }
}

#[test]
fn fix_rounds_the_nodes_it_inserted_for_extrema() {
    // MissingExtrema runs before OffGrid: the new node at (50, 60.375) lands
    // on the grid in the same call.
    let path = hump(1, 80.5);
    let fixed = fix_paths(
        std::slice::from_ref(&path),
        &[IssueKind::OffGrid, IssueKind::MissingExtrema],
        1.0,
    );
    assert_eq!(fixed[0].nodes.len(), 7);
    assert!(issues_of(&fixed, 1.0, IssueKind::OffGrid).is_empty());
    assert_eq!(fixed[0].nodes[3].y, 60.0);
}

#[test]
fn fix_only_applies_the_requested_kinds() {
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(0.0, 100.0),
            line(100.3, 100.0),
            line(100.3, 100.0),
            line(100.0, 0.0),
        ],
    );
    let paths = vec![path];

    let only_grid = fix_paths(&paths, &[IssueKind::OffGrid], 1.0);
    let found = kinds_of(&check_paths(&only_grid, 1.0));
    assert!(found.contains(&IssueKind::WrongDirection));
    assert!(found.contains(&IssueKind::DuplicatePoints));
    assert!(!found.contains(&IssueKind::OffGrid));

    assert_eq!(fix_paths(&paths, &[], 1.0), paths);

    let everything = fix_paths(&paths, &ALL_FIXABLE, 1.0);
    assert!(check_paths(&everything, 1.0).is_empty(), "{everything:?}");
}

#[test]
fn fix_leaves_unfixable_issues_alone() {
    let open_path = open(
        1,
        vec![line(0.0, 0.0), line(100.0, 0.0), line(100.0, 100.0)],
    );
    let tiny = closed(
        2,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            line(100.0, 99.5),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    let invalid = closed(
        3,
        vec![
            line(0.3, 0.7),
            off(10.5, 10.5),
            line(20.7, 0.1),
            line(10.0, -10.0),
        ],
    );
    let paths = vec![open_path, tiny, invalid];
    let every_kind = [
        IssueKind::OpenContour,
        IssueKind::WrongDirection,
        IssueKind::DuplicatePoints,
        IssueKind::TinySegment,
        IssueKind::OffGrid,
        IssueKind::MissingExtrema,
        IssueKind::InvalidContour,
    ];
    // The problems are really there, and really unfixable.
    let issues = check_paths(&paths, 0.5);
    assert_eq!(
        kinds_of(&issues),
        vec![
            IssueKind::OpenContour,
            IssueKind::TinySegment,
            IssueKind::InvalidContour
        ]
    );
    assert!(issues.iter().all(|i| !i.fixable));
    assert_eq!(fix_paths(&paths, &every_kind, 0.5), paths);
}

#[test]
fn fix_preserves_path_ids() {
    let paths = vec![
        square(7, 0.0, 0.0, 100.0, 100.0, false),
        translate(hump(3, 80.0), 500.0, 0.0),
        translate(
            closed(
                9,
                vec![
                    line(0.4, 0.0),
                    line(100.0, 0.0),
                    line(100.0, 0.0),
                    line(100.0, 100.0),
                ],
            ),
            1000.0,
            0.0,
        ),
        translate(open(4, vec![line(0.0, 0.0), line(10.0, 0.0)]), 1500.0, 0.0),
    ];
    let fixed = fix_paths(&paths, &ALL_FIXABLE, 1.0);
    assert_ne!(fixed, paths, "the fixture needs fixing");
    assert_eq!(fixed.len(), paths.len());
    assert_eq!(ids(&fixed), ids(&paths));
    assert_eq!(
        ids(&fixed),
        vec![PathId(7), PathId(3), PathId(9), PathId(4)]
    );
    for (before, after) in paths.iter().zip(&fixed) {
        assert_eq!(before.closed, after.closed);
    }
}

#[test]
fn fix_returns_new_paths_and_leaves_the_input_unchanged() {
    let paths = vec![
        square(1, 0.0, 0.0, 100.0, 100.0, false),
        translate(hump(2, 80.0), 500.0, 0.0),
    ];
    let snapshot = paths.clone();
    let fixed = fix_paths(&paths, &ALL_FIXABLE, 1.0);
    assert_eq!(paths, snapshot);
    assert_ne!(fixed, snapshot);
}

#[test]
fn fix_is_idempotent() {
    let paths = vec![
        // Clockwise outer square with a duplicate and fractional coordinates.
        closed(
            1,
            vec![
                line(0.4, 0.0),
                line(0.0, 100.0),
                line(100.0, 100.0),
                line(100.0, 100.0),
                line(100.0, 0.0),
            ],
        ),
        // A curve with a missing extremum, wrongly wound, with a counter.
        translate(hump(2, 80.0), 500.0, 0.0),
        translate(bulge(3), 800.0, 0.0),
        translate(quad_hump(4), 1100.0, 0.0),
        translate(circle(5, 200.0, 200.0, 100.0, 55.0, false), 1500.0, 0.0),
        translate(circle(6, 200.0, 200.0, 50.0, 28.0, true), 1500.0, 0.0),
        // Things that can only be reported.
        translate(open(7, vec![line(0.0, 0.0), line(10.0, 0.0)]), 2200.0, 0.0),
        translate(
            closed(8, vec![line(0.0, 0.0), off(5.0, 5.0), line(10.0, 0.0)]),
            2400.0,
            0.0,
        ),
    ];
    for grid in [0.0, 1.0] {
        assert!(
            check_paths(&paths, grid).iter().any(|i| i.fixable),
            "the fixture needs fixing"
        );
        let once = fix_paths(&paths, &ALL_FIXABLE, grid);
        assert_ne!(once, paths);
        let twice = fix_paths(&once, &ALL_FIXABLE, grid);
        assert_eq!(twice, once, "grid {grid}");
        assert!(
            check_paths(&once, grid).iter().all(|i| !i.fixable),
            "grid {grid}: {:?}",
            check_paths(&once, grid)
        );
    }
}

#[test]
fn fix_is_idempotent_on_random_contours() {
    let mut rng = Lcg(42);
    let paths: Vec<Path> = (0..400).map(|i| random_mixed(&mut rng, i)).collect();
    for grid in [0.0, 0.5, 1.0, 5.0] {
        let once = fix_paths(&paths, &ALL_FIXABLE, grid);
        assert_ne!(once, paths);
        assert_eq!(ids(&once), ids(&paths));
        // The fixed contours are stable, and nothing fixable is left.
        let twice = fix_paths(&once, &ALL_FIXABLE, grid);
        assert_eq!(twice, once, "grid {grid}");
        let left: Vec<GeometryIssue> = check_paths(&once, grid)
            .into_iter()
            .filter(|i| i.fixable)
            .collect();
        assert!(left.is_empty(), "grid {grid}: {left:?}");
    }
}

#[test]
fn fix_repairs_what_rounding_to_the_grid_causes() {
    // Rounding to the grid moves (100.4, 0.2) onto (100, 0): the duplicate
    // that creates is removed in the same call.
    let path = closed(
        1,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            line(100.4, 0.2),
            line(100.0, 100.0),
            line(0.0, 100.0),
        ],
    );
    let fixed = fix_paths(&[path], &ALL_FIXABLE, 1.0);
    assert_eq!(
        coords(&fixed[0]),
        vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
    );
}
