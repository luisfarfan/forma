//! Integration tests for skeleton strokes.

mod common;

use std::f64::consts::PI;

use common::*;
use forma_geometry::{
    Cap, GeometryError, IssueKind, Skeleton, SkeletonNode, StrokeStyle, bbox, check_paths, stroke,
};
use forma_model::NodeKind;
use kurbo::Point;
use proptest::prelude::*;

fn sk(x: f64, y: f64, kind: NodeKind, width: Option<f64>) -> SkeletonNode {
    SkeletonNode { x, y, kind, width }
}

fn open(nodes: Vec<SkeletonNode>) -> Skeleton {
    Skeleton {
        nodes,
        closed: false,
    }
}

/// A vertical stroke from (0, 0) up to (0, 500).
fn vertical(bottom: Option<f64>, top: Option<f64>) -> Skeleton {
    open(vec![
        sk(0.0, 0.0, NodeKind::Line, bottom),
        sk(0.0, 500.0, NodeKind::Line, top),
    ])
}

fn style(width: f64, cap: Cap) -> StrokeStyle {
    StrokeStyle { width, cap }
}

fn assert_clean(paths: &[forma_model::Path]) {
    assert_eq!(crossing_count(paths), 0, "the outline crosses itself");
    let issues: Vec<_> = check_paths(paths, 0.0)
        .into_iter()
        .filter(|issue| {
            matches!(
                issue.kind,
                IssueKind::InvalidContour | IssueKind::OpenContour | IssueKind::DuplicatePoints
            )
        })
        .collect();
    assert!(issues.is_empty(), "{issues:?}");
    assert_filled_on_the_left(paths);
}

fn assert_near(got: f64, expected: f64, tolerance: f64, what: &str) {
    assert!(
        (got - expected).abs() <= tolerance,
        "{what}: {got} is not within {tolerance} of {expected}"
    );
}

#[test]
fn stroke_segment_has_width() {
    let result = stroke(&[vertical(None, None)], &style(80.0, Cap::Round)).expect("stroke");

    assert_eq!(result.len(), 1, "{result:?}");
    assert!(area(&result[0]) > 0.0, "counter-clockwise outline");
    assert_near(width_at(&result, 250.0), 80.0, 1.0, "width at mid-height");
    let bounds = bbox(&result).expect("bounds");
    assert_near(bounds.height(), 500.0 + 80.0, 1.0, "height with round caps");
    assert_near(bounds.width(), 80.0, 1.0, "width of the bounds");
    assert_clean(&result);
}

#[test]
fn stroke_bulb_terminals() {
    let cap = Cap::Bulb { ratio: 1.5 };
    let result = stroke(&[vertical(None, None)], &style(80.0, cap)).expect("stroke");

    assert_near(width_at(&result, 500.0), 120.0, 2.0, "width at the top end");
    assert_near(
        width_at(&result, 0.0),
        120.0,
        2.0,
        "width at the bottom end",
    );
    assert_near(width_at(&result, 250.0), 80.0, 1.0, "width at mid-height");
    let bounds = bbox(&result).expect("bounds");
    assert_near(bounds.height(), 500.0 + 120.0, 2.0, "height with bulbs");
    assert_eq!(result.len(), 1);
    assert_clean(&result);
}

#[test]
fn stroke_variable_width() {
    let skeleton = vertical(Some(60.0), Some(120.0));
    let result = stroke(&[skeleton], &style(10.0, Cap::Round)).expect("stroke");

    assert_near(width_at(&result, 250.0), 90.0, 2.0, "width half-way");
    assert_near(
        width_at(&result, 100.0),
        72.0,
        2.0,
        "width a fifth of the way",
    );
    let bounds = bbox(&result).expect("bounds");
    assert_near(
        bounds.height(),
        500.0 + 30.0 + 60.0,
        1.0,
        "caps as wide as the ends",
    );
    assert_clean(&result);
}

#[test]
fn stroke_figure_eight_is_clean() {
    // An "8" drawn in one smooth closed stroke that crosses itself at the
    // waist (0, 200): up and round the top loop, through the waist, round the
    // bottom loop and back up to the start.
    let skeleton = Skeleton {
        nodes: vec![
            sk(0.0, 200.0, NodeKind::Curve, None),
            sk(80.0, 280.0, NodeKind::OffCurve, None),
            sk(110.0, 400.0, NodeKind::OffCurve, None),
            sk(0.0, 400.0, NodeKind::Curve, None),
            sk(-110.0, 400.0, NodeKind::OffCurve, None),
            sk(-80.0, 280.0, NodeKind::OffCurve, None),
            sk(0.0, 200.0, NodeKind::Curve, None),
            sk(80.0, 120.0, NodeKind::OffCurve, None),
            sk(110.0, 0.0, NodeKind::OffCurve, None),
            sk(0.0, 0.0, NodeKind::Curve, None),
            sk(-110.0, 0.0, NodeKind::OffCurve, None),
            sk(-80.0, 120.0, NodeKind::OffCurve, None),
        ],
        closed: true,
    };
    let result = stroke(&[skeleton], &style(30.0, Cap::Round)).expect("stroke");

    assert_clean(&result);
    let areas = areas(&result);
    assert_eq!(areas.iter().filter(|a| **a > 0.0).count(), 1, "{areas:?}");
    assert_eq!(
        areas.iter().filter(|a| **a < 0.0).count(),
        2,
        "two counters: {areas:?}"
    );
    // The waist is filled; the middle of each loop is not.
    assert_eq!(winding(&result, Point::new(0.0, 200.0)), 1);
    assert_eq!(winding(&result, Point::new(0.0, 310.0)), 0);
    assert_eq!(winding(&result, Point::new(0.0, 90.0)), 0);
}

#[test]
fn stroke_closed_loop_has_counter() {
    // A circle of radius 200 drawn as a closed skeleton gives a ring.
    let skeleton = circle_skeleton(200.0);
    let result = stroke(&[skeleton], &style(40.0, Cap::Round)).expect("stroke");

    assert_eq!(result.len(), 2, "{result:?}");
    let ring = PI * (220.0 * 220.0 - 180.0 * 180.0);
    assert_near(total_area(&result), ring, 0.005 * ring, "ring area");
    assert_near(width_at(&result, 0.0), 440.0, 1.0, "outer diameter");
    assert_clean(&result);
}

/// A closed circular skeleton of four cubic quarter arcs.
fn circle_skeleton(r: f64) -> Skeleton {
    let k = 0.552_284_749_830_793_4 * r;
    Skeleton {
        nodes: vec![
            sk(r, 0.0, NodeKind::Curve, None),
            sk(r, k, NodeKind::OffCurve, None),
            sk(k, r, NodeKind::OffCurve, None),
            sk(0.0, r, NodeKind::Curve, None),
            sk(-k, r, NodeKind::OffCurve, None),
            sk(-r, k, NodeKind::OffCurve, None),
            sk(-r, 0.0, NodeKind::Curve, None),
            sk(-r, -k, NodeKind::OffCurve, None),
            sk(-k, -r, NodeKind::OffCurve, None),
            sk(0.0, -r, NodeKind::Curve, None),
            sk(k, -r, NodeKind::OffCurve, None),
            sk(r, -k, NodeKind::OffCurve, None),
        ],
        closed: true,
    }
}

#[test]
fn tight_curves_leave_no_false_counters() {
    // A loop of radius 10 stroked 40 wide is a disc of radius 30: the inner
    // offset would fold back past the centre and cancel the winding there.
    let result = stroke(&[circle_skeleton(10.0)], &style(40.0, Cap::Round)).expect("stroke");

    assert_eq!(result.len(), 1, "a disc, no counter: {:?}", areas(&result));
    let disc = PI * 30.0 * 30.0;
    assert_near(total_area(&result), disc, 0.01 * disc, "disc area");
    assert_eq!(winding(&result, Point::new(0.0, 0.0)), 1);
    assert_clean(&result);

    // An open hook whose bend is tighter than the stroke.
    let hook = open(vec![
        sk(0.0, 0.0, NodeKind::Line, None),
        sk(0.0, 200.0, NodeKind::Line, None),
        sk(0.0, 215.0, NodeKind::OffCurve, None),
        sk(30.0, 215.0, NodeKind::OffCurve, None),
        sk(30.0, 200.0, NodeKind::Curve, None),
        sk(30.0, 100.0, NodeKind::Line, None),
    ]);
    let result = stroke(&[hook], &style(60.0, Cap::Round)).expect("stroke");
    assert_eq!(result.len(), 1, "no false counter: {:?}", areas(&result));
    assert_eq!(winding(&result, Point::new(15.0, 205.0)), 1);
    assert_clean(&result);
}

#[test]
fn stroke_corner_has_round_join() {
    let skeleton = open(vec![
        sk(0.0, 300.0, NodeKind::Line, None),
        sk(0.0, 0.0, NodeKind::Line, None),
        sk(200.0, 0.0, NodeKind::Line, None),
    ]);
    let result = stroke(&[skeleton], &style(80.0, Cap::Round)).expect("stroke");

    assert_eq!(result.len(), 1);
    let bounds = bbox(&result).expect("bounds");
    assert_near(bounds.x0, -40.0, 0.5, "left");
    assert_near(bounds.y0, -40.0, 0.5, "bottom");
    assert_near(bounds.x1, 240.0, 0.5, "right");
    assert_near(bounds.y1, 340.0, 0.5, "top");
    // Round, not mitred: the outer corner is 40 units from the node.
    let diagonal = std::f64::consts::FRAC_1_SQRT_2;
    assert_eq!(
        winding(&result, Point::new(-38.0 * diagonal, -38.0 * diagonal)),
        1
    );
    assert_eq!(
        winding(&result, Point::new(-42.0 * diagonal, -42.0 * diagonal)),
        0
    );
    assert_clean(&result);
}

#[test]
fn stroke_rejects_degenerate() {
    let round = style(80.0, Cap::Round);
    let invalid = |result: Result<Vec<forma_model::Path>, GeometryError>| {
        matches!(result, Err(GeometryError::InvalidPath(_)))
    };

    // A single node.
    let lone = open(vec![sk(0.0, 0.0, NodeKind::Line, None)]);
    assert!(invalid(stroke(&[lone], &round)));
    // A closed skeleton with one on-curve node.
    let loop_of_one = Skeleton {
        nodes: vec![
            sk(0.0, 0.0, NodeKind::Curve, None),
            sk(100.0, 100.0, NodeKind::OffCurve, None),
            sk(-100.0, 100.0, NodeKind::OffCurve, None),
        ],
        closed: true,
    };
    assert!(invalid(stroke(&[loop_of_one], &round)));
    // Widths that are zero, negative or not finite.
    assert!(invalid(stroke(
        &[vertical(None, None)],
        &style(0.0, Cap::Round)
    )));
    assert!(invalid(stroke(
        &[vertical(None, None)],
        &style(f64::NAN, Cap::Round)
    )));
    assert!(invalid(stroke(&[vertical(Some(f64::NAN), None)], &round)));
    assert!(invalid(stroke(&[vertical(None, Some(-5.0))], &round)));
    assert!(invalid(stroke(&[vertical(Some(0.0), None)], &round)));
    assert!(invalid(stroke(
        &[vertical(None, Some(f64::INFINITY))],
        &round
    )));
    // A bulb ratio that is not a positive number.
    assert!(invalid(stroke(
        &[vertical(None, None)],
        &style(80.0, Cap::Bulb { ratio: f64::NAN })
    )));
    // A coordinate that is not finite.
    let far = open(vec![
        sk(0.0, 0.0, NodeKind::Line, None),
        sk(f64::INFINITY, 0.0, NodeKind::Line, None),
    ]);
    assert!(invalid(stroke(&[far], &round)));
}

#[test]
fn several_skeletons_are_united() {
    // A cross: two strokes that overlap in the middle make one outline.
    let horizontal = open(vec![
        sk(-200.0, 250.0, NodeKind::Line, None),
        sk(200.0, 250.0, NodeKind::Line, None),
    ]);
    let result = stroke(
        &[vertical(None, None), horizontal],
        &style(40.0, Cap::Round),
    )
    .expect("stroke");

    assert_eq!(result.len(), 1, "{result:?}");
    assert_clean(&result);
    assert!(
        stroke(&[], &style(40.0, Cap::Round))
            .expect("nothing")
            .is_empty()
    );
}

#[test]
fn stroke_arguments_round_trip_through_json() {
    let json = r#"{
        "skeleton": {"nodes": [
            {"x": 0, "y": 0, "kind": "line"},
            {"x": 0, "y": 500, "kind": "line", "width": 120}
        ]},
        "style": {"width": 80, "cap": {"type": "bulb", "ratio": 1.5}}
    }"#;
    #[derive(serde::Deserialize)]
    struct Args {
        skeleton: Skeleton,
        style: StrokeStyle,
    }
    let args: Args = serde_json::from_str(json).expect("valid arguments");
    assert!(!args.skeleton.closed);
    assert_eq!(args.skeleton.nodes[0].width, None);
    assert_eq!(args.skeleton.nodes[1].width, Some(120.0));
    assert_eq!(args.style.cap, Cap::Bulb { ratio: 1.5 });

    let round: StrokeStyle =
        serde_json::from_str(r#"{"width": 80, "cap": {"type": "round"}}"#).expect("round cap");
    assert_eq!(round.cap, Cap::Round);
    let back = serde_json::to_value(args.style).expect("serializes");
    assert_eq!(
        back,
        serde_json::json!({"width": 80.0, "cap": {"type": "bulb", "ratio": 1.5}})
    );
    // Unknown fields are refused, so a misspelt argument is not ignored.
    assert!(
        serde_json::from_str::<StrokeStyle>(r#"{"widht": 80, "cap": {"type": "round"}}"#).is_err()
    );
    // The schema (the tool argument in MCP) documents the shape of a cap.
    let schema = serde_json::to_string(&schemars::schema_for!(Cap)).expect("schema");
    assert!(
        schema.contains(r#"`{\"type\": \"bulb\", \"ratio\": 1.4}`"#),
        "{schema}"
    );
}

#[test]
fn crossing_slivers_are_swept_again() {
    // Found by a random search: the first union of this stroke kept a sliver
    // about 1.3 units long whose sides cross, at the start of a counter. The
    // check sweep catches it and the second union removes it.
    let skeleton = Skeleton {
        nodes: vec![
            sk(
                -265.159_539_088_497_24,
                72.289_348_374_155_68,
                NodeKind::Line,
                None,
            ),
            sk(
                137.046_303_971_585_36,
                -4.714_820_799_378_97,
                NodeKind::OffCurve,
                None,
            ),
            sk(
                47.870_988_371_761_43,
                -128.295_654_836_625_5,
                NodeKind::OffCurve,
                None,
            ),
            sk(
                -88.958_232_015_119_62,
                198.314_273_430_971_24,
                NodeKind::Curve,
                Some(102.856_677_987_274_62),
            ),
            sk(
                252.995_676_953_599_5,
                92.315_179_206_939_43,
                NodeKind::Line,
                Some(75.632_303_930_883_41),
            ),
            sk(
                -157.361_524_306_068_56,
                -200.600_376_609_136_5,
                NodeKind::OffCurve,
                None,
            ),
            sk(
                -92.902_146_064_942_17,
                -198.770_897_092_385_27,
                NodeKind::OffCurve,
                None,
            ),
            sk(
                -126.122_800_476_831_8,
                187.592_308_235_286_34,
                NodeKind::Curve,
                Some(61.378_440_552_470_63),
            ),
        ],
        closed: true,
    };
    let result = stroke(&[skeleton], &style(15.138_592_594_971_3, Cap::Round)).expect("stroke");

    assert_eq!(result.len(), 5, "{:?}", areas(&result));
    assert_clean(&result);
}

/// A random skeleton: 2 to 5 on-curve nodes joined by lines or cubics, open
/// or closed, some nodes with their own width.
fn skeleton() -> impl Strategy<Value = Skeleton> {
    let coordinate = -300.0f64..300.0;
    let point = (coordinate.clone(), coordinate);
    let width = prop::option::weighted(0.3, 10.0f64..110.0);
    let segment = (
        prop::bool::weighted(0.6),
        point.clone(),
        point.clone(),
        point,
        width,
    );
    (prop::collection::vec(segment, 2..6), any::<bool>()).prop_map(|(segments, closed)| {
        let mut nodes = Vec::new();
        for (i, (curved, c1, c2, end, width)) in segments.into_iter().enumerate() {
            // The first node of an open skeleton is its start: no handles.
            if curved && (i > 0 || closed) {
                nodes.push(sk(c1.0, c1.1, NodeKind::OffCurve, None));
                nodes.push(sk(c2.0, c2.1, NodeKind::OffCurve, None));
                nodes.push(sk(end.0, end.1, NodeKind::Curve, width));
            } else {
                nodes.push(sk(end.0, end.1, NodeKind::Line, width));
            }
        }
        if closed {
            // The handles of the closing segment go last.
            let lead = nodes
                .iter()
                .take_while(|node| node.kind == NodeKind::OffCurve)
                .count();
            nodes.rotate_left(lead);
        }
        Skeleton { nodes, closed }
    })
}

fn cap() -> impl Strategy<Value = Cap> {
    prop_oneof![
        Just(Cap::Round),
        (1.0f64..2.0).prop_map(|ratio| Cap::Bulb { ratio }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any skeleton, however tangled, strokes to a clean outline.
    #[test]
    fn random_strokes_are_clean(
        skeleton in skeleton(),
        width in 10.0f64..130.0,
        cap in cap(),
    ) {
        let result = stroke(&[skeleton], &style(width, cap)).expect("stroke");
        prop_assert!(!result.is_empty());
        prop_assert!(total_area(&result) > 0.0);
        assert_clean(&result);
    }
}
