//! Contour, node and geometry-fix commands.

mod common;

use std::sync::Arc;

use common::*;
use forma_edit::{Command, ContourAction, EditError, NodeAction};
use forma_geometry::{IssueKind, check_paths, signed_area};
use forma_model::{EntityKey, Font, NodeKind, Path, PathId};

/// A font whose only glyph, `G`, has the given contours.
fn glyph_font(paths: Vec<Path>) -> Font {
    let mut font = Font::new("Test", 1000);
    add_glyph(&mut font, "G", &[0x47], 600.0, paths);
    font
}

fn contour(action: ContourAction) -> Command {
    Command::ContourEdit {
        glyph: by_name("G"),
        master: None,
        action,
    }
}

fn nodes(path_id: u32, action: NodeAction) -> Command {
    Command::NodesEdit {
        glyph: by_name("G"),
        master: None,
        path_id: PathId(path_id),
        action,
    }
}

fn path0(font: &Font) -> &Path {
    &layer(font, "G").paths[0]
}

// ---------------------------------------------------------- ContourEdit

#[test]
fn contour_add_assigns_the_next_id() {
    let font = glyph_font(vec![
        square(2, 0.0, 0.0, 10.0, 10.0, true),
        square(5, 20.0, 0.0, 30.0, 10.0, true),
    ]);
    let g = id_of(&font, "G");
    let (next, touched) = run(
        &font,
        contour(ContourAction::Add {
            path: square(0, 40.0, 0.0, 50.0, 10.0, false),
        }),
    );
    let paths = &layer(&next, "G").paths;
    let ids: Vec<PathId> = paths.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![PathId(2), PathId(5), PathId(6)]);
    assert!(signed_area(&paths[2]) < 0.0, "direction is kept as given");
    assert_eq!(touched, keys([EntityKey::Glyph(g)]));
}

#[test]
fn contour_add_to_an_empty_layer_starts_at_zero() {
    let font = glyph_font(vec![]);
    let (next, _) = run(
        &font,
        contour(ContourAction::Add {
            path: square(99, 0.0, 0.0, 10.0, 10.0, true),
        }),
    );
    assert_eq!(path0(&next).id, PathId(0));
}

#[test]
fn contour_add_rejects_invalid_geometry() {
    let font = glyph_font(vec![]);
    for bad in [
        closed(0, vec![line(0.0, 0.0)]),
        closed(0, vec![line(0.0, 0.0), line(f64::NAN, 1.0), line(2.0, 2.0)]),
        open(0, vec![off(0.0, 0.0), line(1.0, 1.0)]),
    ] {
        let err = fails(&font, contour(ContourAction::Add { path: bad }));
        assert!(matches!(err, EditError::InvalidGeometry(_)), "{err:?}");
    }
}

#[test]
fn contour_delete_removes_only_that_contour() {
    let font = glyph_font(vec![
        square(0, 0.0, 0.0, 10.0, 10.0, true),
        square(1, 20.0, 0.0, 30.0, 10.0, true),
    ]);
    let (next, _) = run(&font, contour(ContourAction::Delete { path_id: PathId(0) }));
    let paths = &layer(&next, "G").paths;
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].id, PathId(1));
    assert_eq!(paths[0], layer(&font, "G").paths[1]);
}

#[test]
fn contour_actions_on_an_unknown_path_are_not_found() {
    let font = glyph_font(vec![square(0, 0.0, 0.0, 10.0, 10.0, true)]);
    let missing = PathId(7);
    for action in [
        ContourAction::Delete { path_id: missing },
        ContourAction::Reverse { path_id: missing },
        ContourAction::Transform {
            path_id: missing,
            affine: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        },
    ] {
        let err = fails(&font, contour(action));
        assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
    }
    let err = fails(
        &font,
        nodes(
            7,
            NodeAction::SetSmooth {
                index: 0,
                smooth: true,
            },
        ),
    );
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
}

#[test]
fn contour_reverse_flips_direction_and_keeps_the_shape() {
    let font = glyph_font(vec![mixed(0), circle(1, 500.0, 500.0, 50.0)]);
    for (id, original) in [(0, mixed(0)), (1, circle(1, 500.0, 500.0, 50.0))] {
        let (next, touched) = run(
            &font,
            contour(ContourAction::Reverse {
                path_id: PathId(id),
            }),
        );
        let reversed = layer(&next, "G")
            .paths
            .iter()
            .find(|p| p.id == PathId(id))
            .expect("the contour is still there");
        assert!(signed_area(&original) > 0.0);
        assert!(signed_area(reversed) < 0.0);
        assert!((signed_area(reversed) + signed_area(&original)).abs() < 1e-6);
        assert!(deviation(&original, reversed) < 1e-6);
        assert_eq!(reversed.nodes.len(), original.nodes.len());
        assert_eq!(touched, keys([EntityKey::Glyph(id_of(&font, "G"))]));

        // Reversing twice gives the same shape and direction again.
        let (again, _) = run(
            &next,
            contour(ContourAction::Reverse {
                path_id: PathId(id),
            }),
        );
        let twice = again
            .glyphs
            .values()
            .next()
            .and_then(|g| g.layers.values().next())
            .and_then(|l| l.paths.iter().find(|p| p.id == PathId(id)))
            .expect("the contour is still there");
        assert!(signed_area(twice) > 0.0);
        assert!(deviation(&original, twice) < 1e-6);
    }
}

#[test]
fn contour_reverse_handles_open_contours() {
    let path = open(
        0,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(150.0, 50.0),
            off(150.0, 100.0),
            curve(100.0, 150.0),
        ],
    );
    let font = glyph_font(vec![path.clone()]);
    let (next, _) = run(
        &font,
        contour(ContourAction::Reverse { path_id: PathId(0) }),
    );
    let reversed = path0(&next);
    assert!(!reversed.closed);
    assert_eq!(
        coords(reversed),
        vec![
            (100.0, 150.0),
            (150.0, 100.0),
            (150.0, 50.0),
            (100.0, 0.0),
            (0.0, 0.0)
        ]
    );
    assert_eq!(
        kinds(reversed),
        vec![
            NodeKind::Line,
            NodeKind::OffCurve,
            NodeKind::OffCurve,
            NodeKind::Curve,
            NodeKind::Line
        ]
    );
    assert!(deviation(&path, reversed) < 1e-6);
}

#[test]
fn contour_transform_applies_the_affine_to_every_node() {
    let font = glyph_font(vec![mixed(0)]);
    // Scale by 2 about the origin, then move by (10, 20).
    let (next, touched) = run(
        &font,
        contour(ContourAction::Transform {
            path_id: PathId(0),
            affine: [2.0, 0.0, 0.0, 2.0, 10.0, 20.0],
        }),
    );
    let original = mixed(0);
    let moved = path0(&next);
    assert_eq!(moved.nodes.len(), original.nodes.len());
    for (before, after) in original.nodes.iter().zip(&moved.nodes) {
        assert_eq!(after.x, before.x * 2.0 + 10.0);
        assert_eq!(after.y, before.y * 2.0 + 20.0);
        assert_eq!(after.kind, before.kind);
        assert_eq!(after.smooth, before.smooth);
    }
    assert_eq!(touched, keys([EntityKey::Glyph(id_of(&font, "G"))]));

    // A shear uses all six coefficients: x' = a*x + c*y + e, y' = b*x + d*y + f.
    let (next, _) = run(
        &font,
        contour(ContourAction::Transform {
            path_id: PathId(0),
            affine: [1.0, 0.5, 0.25, 1.0, 3.0, 4.0],
        }),
    );
    let first = &path0(&next).nodes[2]; // off-curve at (150, 20)
    assert_eq!(first.x, 150.0 + 0.25 * 20.0 + 3.0);
    assert_eq!(first.y, 0.5 * 150.0 + 20.0 + 4.0);
}

#[test]
fn transform_rejects_non_finite() {
    let font = glyph_font(vec![square(0, 0.0, 0.0, 10.0, 10.0, true)]);
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for slot in 0..6 {
            let mut affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
            affine[slot] = bad;
            let err = fails(
                &font,
                contour(ContourAction::Transform {
                    path_id: PathId(0),
                    affine,
                }),
            );
            assert!(
                matches!(err, EditError::InvalidArgument(_)),
                "{affine:?}: {err:?}"
            );
        }
    }
    // Finite coefficients whose result overflows are caught as bad geometry.
    let err = fails(
        &font,
        contour(ContourAction::Transform {
            path_id: PathId(0),
            affine: [f64::MAX, 0.0, 0.0, 1.0, 0.0, 0.0],
        }),
    );
    assert!(matches!(err, EditError::InvalidGeometry(_)), "{err:?}");
}

#[test]
fn contour_edit_targets_the_chosen_master() {
    let mut font = glyph_font(vec![square(0, 0.0, 0.0, 10.0, 10.0, true)]);
    let bold = add_master(&mut font);
    let (next, _) = run(
        &font,
        Command::ContourEdit {
            glyph: by_name("G"),
            master: Some(bold),
            action: ContourAction::Delete { path_id: PathId(0) },
        },
    );
    assert!(layer_of(&next, "G", bold).paths.is_empty());
    assert_eq!(layer(&next, "G").paths.len(), 1);
}

#[test]
fn set_outline_creates_a_missing_layer() {
    let mut font = glyph_font(vec![square(0, 0.0, 0.0, 10.0, 10.0, true)]);
    let bold = add_master(&mut font);
    let g = id_of(&font, "G");
    Arc::make_mut(
        Arc::make_mut(&mut font.glyphs)
            .get_mut(&g)
            .expect("glyph G"),
    )
    .layers
    .remove(&bold);
    let (next, _) = run(
        &font,
        Command::SetOutline {
            glyph: by_name("G"),
            master: Some(bold),
            paths: vec![square(0, 0.0, 0.0, 20.0, 20.0, true)],
        },
    );
    let created = layer_of(&next, "G", bold);
    assert_eq!(created.paths.len(), 1);
    assert_eq!(
        created.advance_width, 600.0,
        "borrowed from the existing layer"
    );
}

// ------------------------------------------------------------- NodesEdit

#[test]
fn nodes_move_moves_exactly_the_listed_nodes() {
    let font = glyph_font(vec![mixed(0)]);
    let g = id_of(&font, "G");
    let (next, touched) = run(
        &font,
        nodes(
            0,
            NodeAction::Move {
                indices: vec![1, 3],
                dx: 5.0,
                dy: -2.0,
            },
        ),
    );
    let original = mixed(0);
    let moved = path0(&next);
    for (i, (before, after)) in original.nodes.iter().zip(&moved.nodes).enumerate() {
        if i == 1 || i == 3 {
            assert_eq!(
                (after.x, after.y),
                (before.x + 5.0, before.y - 2.0),
                "node {i}"
            );
        } else {
            assert_eq!(
                (after.x, after.y),
                (before.x, before.y),
                "node {i} must not move"
            );
        }
        assert_eq!(after.kind, before.kind);
    }
    assert_eq!(touched, keys([EntityKey::Glyph(g)]));
}

#[test]
fn nodes_move_counts_a_repeated_index_once() {
    let font = glyph_font(vec![mixed(0)]);
    let (next, _) = run(
        &font,
        nodes(
            0,
            NodeAction::Move {
                indices: vec![0, 0],
                dx: 5.0,
                dy: 0.0,
            },
        ),
    );
    assert_eq!(path0(&next).nodes[0].x, 5.0);
}

#[test]
fn nodes_move_rejects_bad_input() {
    let font = glyph_font(vec![mixed(0)]);
    for (indices, dx, dy) in [
        (vec![8], 1.0, 1.0),
        (vec![0, 99], 1.0, 1.0),
        (vec![], 1.0, 1.0),
        (vec![0], f64::NAN, 0.0),
        (vec![0], 0.0, f64::INFINITY),
    ] {
        let err = fails(&font, nodes(0, NodeAction::Move { indices, dx, dy }));
        assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
    }
}

#[test]
fn nodes_move_that_overflows_is_invalid_geometry() {
    let font = glyph_font(vec![closed(
        0,
        vec![line(f64::MAX, 0.0), line(1.0, 0.0), line(1.0, 1.0)],
    )]);
    let err = fails(
        &font,
        nodes(
            0,
            NodeAction::Move {
                indices: vec![0],
                dx: f64::MAX,
                dy: 0.0,
            },
        ),
    );
    assert!(matches!(err, EditError::InvalidGeometry(_)), "{err:?}");
}

/// Asserts that two coordinate lists agree to within rounding.
fn assert_coords(actual: &[(f64, f64)], expected: &[(f64, f64)]) {
    assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
    for (a, e) in actual.iter().zip(expected) {
        assert!(
            (a.0 - e.0).abs() < 1e-9 && (a.1 - e.1).abs() < 1e-9,
            "{actual:?} vs {expected:?}"
        );
    }
}

fn insert(segment_index: usize, t: f64) -> Command {
    nodes(0, NodeAction::Insert { segment_index, t })
}

fn edited_path(font: &Font, cmd: Command) -> Path {
    let (next, touched) = run(font, cmd);
    assert_eq!(touched, keys([EntityKey::Glyph(id_of(font, "G"))]));
    path0(&next).clone()
}

#[test]
fn nodes_insert_preserves_shape() {
    for original in [mixed(0), circle(0, 500.0, 500.0, 80.0)] {
        let font = glyph_font(vec![original.clone()]);
        let segments = original
            .nodes
            .iter()
            .filter(|n| n.kind != NodeKind::OffCurve)
            .count();
        for segment_index in 0..segments {
            for t in [0.1, 0.5, 0.83] {
                let edited = edited_path(&font, insert(segment_index, t));
                assert!(
                    deviation(&original, &edited) < 0.01,
                    "segment {segment_index} at t = {t}"
                );
                assert!(edited.closed);
                assert!(edited.nodes.len() > original.nodes.len());
            }
        }
    }
}

#[test]
fn insert_splits_a_line() {
    let font = glyph_font(vec![mixed(0)]);
    // Segment 1 is the line (0,0) -> (100,0).
    let edited = edited_path(&font, insert(1, 0.25));
    assert_eq!(edited.nodes.len(), 9);
    assert_coords(
        &coords(&edited)[..3],
        &[(0.0, 0.0), (25.0, 0.0), (100.0, 0.0)],
    );
    assert_eq!(edited.nodes[1].kind, NodeKind::Line);
    assert!(!edited.nodes[1].smooth);
}

#[test]
fn insert_splits_a_cubic_by_de_casteljau() {
    let font = glyph_font(vec![mixed(0)]);
    // Segment 2 is the cubic (100,0) (150,20) (150,80) (100,100). Splitting at
    // 0.5 gives the handles and the point below.
    let edited = edited_path(&font, insert(2, 0.5));
    assert_eq!(edited.nodes.len(), 11);
    assert_coords(
        &coords(&edited)[2..7],
        &[
            (125.0, 10.0),
            (137.5, 30.0),
            (137.5, 50.0),
            (137.5, 70.0),
            (125.0, 90.0),
        ],
    );
    assert_eq!(
        kinds(&edited)[2..8],
        [
            NodeKind::OffCurve,
            NodeKind::OffCurve,
            NodeKind::Curve,
            NodeKind::OffCurve,
            NodeKind::OffCurve,
            NodeKind::Curve
        ]
    );
    assert!(
        edited.nodes[4].smooth,
        "a node inserted in a curve is smooth"
    );
    assert_eq!((edited.nodes[7].x, edited.nodes[7].y), (100.0, 100.0));
    assert!(
        !edited.nodes[7].smooth,
        "the original end node is unchanged"
    );
}

#[test]
fn insert_splits_a_quadratic() {
    let font = glyph_font(vec![mixed(0)]);
    // Segment 4 is the quadratic (50,100) (0,120) (0,50).
    let edited = edited_path(&font, insert(4, 0.5));
    assert_eq!(edited.nodes.len(), 10);
    assert_coords(
        &coords(&edited)[6..9],
        &[(25.0, 110.0), (12.5, 97.5), (0.0, 85.0)],
    );
    assert_eq!(
        kinds(&edited)[6..9],
        [NodeKind::OffCurve, NodeKind::QCurve, NodeKind::OffCurve]
    );
    assert!(edited.nodes[7].smooth);
    assert_eq!(edited.nodes[9].kind, NodeKind::QCurve);
}

#[test]
fn insert_segment_index_wraps_on_closed_contour() {
    // Segment 0 ends at the first on-curve node and starts at the last one.
    let font = glyph_font(vec![mixed(0)]);
    let edited = edited_path(&font, insert(0, 0.5));
    assert_eq!(edited.nodes.len(), 9);
    // The closing line (0,50) -> (0,0) gets its node at the end of the list;
    // the first node stays first.
    assert_coords(&coords(&edited)[7..], &[(0.0, 50.0), (0.0, 25.0)]);
    assert_eq!((edited.nodes[0].x, edited.nodes[0].y), (0.0, 0.0));

    // A wrapping curve: the closing segment of the circle runs from
    // (0,-r) to (r,0) through the two handles at the end of the list.
    let circle = circle(0, 0.0, 0.0, 100.0);
    let font = glyph_font(vec![circle.clone()]);
    let edited = edited_path(&font, insert(0, 0.5));
    assert_eq!(edited.nodes.len(), 15);
    assert_eq!(&edited.nodes[..10], &circle.nodes[..10]);
    assert_eq!(
        kinds(&edited)[10..],
        [
            NodeKind::OffCurve,
            NodeKind::OffCurve,
            NodeKind::Curve,
            NodeKind::OffCurve,
            NodeKind::OffCurve
        ]
    );
    let middle = &edited.nodes[12];
    let r = middle.x.hypot(middle.y);
    assert!(
        (r - 100.0).abs() < 0.1,
        "the new node lies on the circle: {r}"
    );
    assert!(
        middle.x > 0.0 && middle.y < 0.0,
        "in the lower right quadrant"
    );
    assert!(deviation(&circle, &edited) < 0.01);
}

#[test]
fn insert_works_when_a_closed_contour_starts_with_off_curve_nodes() {
    // Rotate the circle so that the list starts with the two handles that
    // lead into its first on-curve node.
    let mut circle = circle(0, 0.0, 0.0, 100.0);
    circle.nodes.rotate_right(2);
    assert_eq!(circle.nodes[0].kind, NodeKind::OffCurve);
    let font = glyph_font(vec![circle.clone()]);
    for segment_index in 0..4 {
        let edited = edited_path(&font, insert(segment_index, 0.4));
        assert_eq!(edited.nodes.len(), 15);
        assert!(
            deviation(&circle, &edited) < 0.01,
            "segment {segment_index}"
        );
    }
}

#[test]
fn insert_segment_index_on_open_contour_starts_after_the_move_to() {
    let path = open(
        0,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(150.0, 50.0),
            off(150.0, 100.0),
            curve(100.0, 150.0),
        ],
    );
    let font = glyph_font(vec![path.clone()]);
    // Segment 0 ends at the second on-curve node: the line (0,0) -> (100,0).
    let edited = edited_path(&font, insert(0, 0.5));
    assert_eq!(edited.nodes.len(), 6);
    assert_coords(
        &coords(&edited)[..3],
        &[(0.0, 0.0), (50.0, 0.0), (100.0, 0.0)],
    );
    assert!(!edited.closed);

    // Segment 1 is the cubic.
    let edited = edited_path(&font, insert(1, 0.5));
    assert_eq!(edited.nodes.len(), 8);
    assert_eq!(edited.nodes[4].kind, NodeKind::Curve);
    assert!(deviation(&path, &edited) < 0.01);

    // An open contour with three on-curve nodes has two segments.
    let err = fails(&font, insert(2, 0.5));
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
}

#[test]
fn insert_rejects_bad_t_and_segment_index() {
    let font = glyph_font(vec![mixed(0)]);
    for t in [0.0, 1.0, -0.1, 1.5, f64::NAN, f64::INFINITY] {
        let err = fails(&font, insert(1, t));
        assert!(
            matches!(err, EditError::InvalidArgument(_)),
            "t = {t}: {err:?}"
        );
    }
    // `mixed` has five on-curve nodes, hence five segments.
    let err = fails(&font, insert(5, 0.5));
    assert!(matches!(err, EditError::InvalidArgument(_)), "{err:?}");
}

fn delete(index: usize) -> Command {
    nodes(0, NodeAction::Delete { index })
}

#[test]
fn nodes_delete_merges_cubics() {
    let circle = circle(0, 0.0, 0.0, 100.0);
    let font = glyph_font(vec![circle.clone()]);
    // Delete the on-curve node at the top: the cubic from the right to the
    // top and the cubic from the top to the left become one cubic whose
    // handles are the first handle of the incoming and the last handle of the
    // outgoing segment.
    let edited = edited_path(&font, delete(3));
    let n = &circle.nodes;
    let expected: Vec<_> = [
        &n[0], &n[1], &n[5], &n[6], &n[7], &n[8], &n[9], &n[10], &n[11],
    ]
    .iter()
    .map(|node| (node.x, node.y))
    .collect();
    assert_coords(&coords(&edited), &expected);
    assert_eq!(
        kinds(&edited)[..4],
        [
            NodeKind::Curve,
            NodeKind::OffCurve,
            NodeKind::OffCurve,
            NodeKind::Curve
        ]
    );
}

#[test]
fn nodes_delete_merges_cubics_across_the_wrap() {
    let circle = circle(0, 0.0, 0.0, 100.0);
    let font = glyph_font(vec![circle.clone()]);
    // Deleting the first node merges the closing cubic with the first one.
    let edited = edited_path(&font, delete(0));
    let n = &circle.nodes;
    let expected: Vec<_> = [
        &n[3], &n[4], &n[5], &n[6], &n[7], &n[8], &n[9], &n[10], &n[2],
    ]
    .iter()
    .map(|node| (node.x, node.y))
    .collect();
    assert_coords(&coords(&edited), &expected);
    assert_eq!(
        edited.nodes.last().map(|n| n.kind),
        Some(NodeKind::OffCurve)
    );
}

#[test]
fn nodes_delete_replaces_other_pairs_by_a_line() {
    let font = glyph_font(vec![mixed(0)]);

    // curve + line -> line
    let edited = edited_path(&font, delete(4));
    assert_coords(
        &coords(&edited),
        &[
            (0.0, 0.0),
            (100.0, 0.0),
            (50.0, 100.0),
            (0.0, 120.0),
            (0.0, 50.0),
        ],
    );
    assert_eq!(kinds(&edited)[2], NodeKind::Line);

    // line + quadratic -> line, and the end node becomes a line node
    let edited = edited_path(&font, delete(5));
    assert_coords(
        &coords(&edited),
        &[
            (0.0, 0.0),
            (100.0, 0.0),
            (150.0, 20.0),
            (150.0, 80.0),
            (100.0, 100.0),
            (0.0, 50.0),
        ],
    );
    assert_eq!(edited.nodes[5].kind, NodeKind::Line);

    // line + cubic -> line, and the cubic's end node becomes a line node
    let edited = edited_path(&font, delete(1));
    assert_coords(
        &coords(&edited),
        &[
            (0.0, 0.0),
            (100.0, 100.0),
            (50.0, 100.0),
            (0.0, 120.0),
            (0.0, 50.0),
        ],
    );
    assert_eq!(edited.nodes[1].kind, NodeKind::Line);

    // quadratic + line -> line
    let edited = edited_path(&font, delete(7));
    assert_eq!(edited.nodes.len(), 6);
    assert_eq!(edited.nodes[0].kind, NodeKind::Line);

    // line + line, across the wrap
    let edited = edited_path(&font, delete(0));
    assert_eq!(edited.nodes.len(), 7);
    assert_eq!((edited.nodes[0].x, edited.nodes[0].y), (100.0, 0.0));
}

#[test]
fn nodes_delete_rejects_off_curve_and_out_of_range_nodes() {
    let font = glyph_font(vec![mixed(0)]);
    for index in [2, 3, 6, 8, 100] {
        let err = fails(&font, delete(index));
        assert!(
            matches!(err, EditError::InvalidArgument(_)),
            "{index}: {err:?}"
        );
    }
}

#[test]
fn nodes_delete_keeps_closed_contours_closed_with_two_nodes() {
    let triangle = closed(0, vec![line(0.0, 0.0), line(100.0, 0.0), line(50.0, 100.0)]);
    let font = glyph_font(vec![triangle]);
    let (two, _) = run(&font, delete(2));
    assert_eq!(path0(&two).nodes.len(), 2);
    let err = fails(&two, delete(0));
    assert!(matches!(err, EditError::InvalidGeometry(_)), "{err:?}");
}

#[test]
fn nodes_delete_on_open_contours() {
    let path = open(
        0,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(150.0, 50.0),
            off(150.0, 100.0),
            curve(100.0, 150.0),
        ],
    );
    let font = glyph_font(vec![path]);

    // The last node takes its handles with it.
    let edited = edited_path(&font, delete(4));
    assert_coords(&coords(&edited), &[(0.0, 0.0), (100.0, 0.0)]);

    // A middle node merges the segments into a line.
    let edited = edited_path(&font, delete(1));
    assert_coords(&coords(&edited), &[(0.0, 0.0), (100.0, 150.0)]);
    assert_eq!(edited.nodes[1].kind, NodeKind::Line);

    // Deleting the first node makes the next one the starting point.
    let starts_with_curve = open(
        0,
        vec![
            line(0.0, 0.0),
            off(10.0, 0.0),
            off(20.0, 10.0),
            curve(30.0, 10.0),
            line(40.0, 40.0),
        ],
    );
    let edited = edited_path(&glyph_font(vec![starts_with_curve]), delete(0));
    assert_coords(&coords(&edited), &[(30.0, 10.0), (40.0, 40.0)]);
    assert_eq!(
        edited.nodes[0].kind,
        NodeKind::Line,
        "the move-to is a line node"
    );

    // The last remaining node cannot be deleted.
    let font = glyph_font(vec![open(0, vec![line(5.0, 5.0)])]);
    let err = fails(&font, delete(0));
    assert!(matches!(err, EditError::InvalidGeometry(_)), "{err:?}");
}

#[test]
fn nodes_set_smooth_only_sets_the_flag() {
    let font = glyph_font(vec![mixed(0)]);
    let edited = edited_path(
        &font,
        nodes(
            0,
            NodeAction::SetSmooth {
                index: 4,
                smooth: true,
            },
        ),
    );
    assert!(edited.nodes[4].smooth);
    let mut expected = mixed(0);
    expected.nodes[4].smooth = true;
    assert_eq!(edited, expected, "no handle moved");

    let (back, _) = run(
        &font,
        nodes(
            0,
            NodeAction::SetSmooth {
                index: 1,
                smooth: false,
            },
        ),
    );
    assert_eq!(path0(&back), &mixed(0));
}

#[test]
fn nodes_set_smooth_rejects_off_curve_and_out_of_range_nodes() {
    let font = glyph_font(vec![mixed(0)]);
    for index in [2, 3, 6, 8] {
        let err = fails(
            &font,
            nodes(
                0,
                NodeAction::SetSmooth {
                    index,
                    smooth: true,
                },
            ),
        );
        assert!(
            matches!(err, EditError::InvalidArgument(_)),
            "{index}: {err:?}"
        );
    }
}

#[test]
fn nodes_edit_leaves_other_contours_and_glyphs_alone() {
    let mut font = glyph_font(vec![mixed(0), square(1, 500.0, 0.0, 600.0, 100.0, true)]);
    add_glyph(&mut font, "H", &[0x48], 500.0, vec![mixed(0)]);
    let (next, _) = run(
        &font,
        nodes(
            0,
            NodeAction::Move {
                indices: vec![0],
                dx: 1.0,
                dy: 1.0,
            },
        ),
    );
    assert_eq!(layer(&next, "G").paths[1], layer(&font, "G").paths[1]);
    let h = id_of(&font, "H");
    assert!(Arc::ptr_eq(&next.glyphs[&h], &font.glyphs[&h]));
}

// ------------------------------------------------------- ApplyGeometryFixes

fn fix(glyphs: Vec<forma_edit::GlyphRef>, kinds: Vec<IssueKind>, grid: f64) -> Command {
    Command::ApplyGeometryFixes {
        glyphs,
        kinds,
        grid,
    }
}

fn clockwise() -> Path {
    square(0, 0.0, 0.0, 100.0, 100.0, false)
}

#[test]
fn apply_geometry_fixes_corrects_wrong_direction() {
    let font = glyph_font(vec![clockwise()]);
    let g = id_of(&font, "G");
    let (next, touched) = run(&font, fix(vec![], vec![IssueKind::WrongDirection], 0.0));
    assert!(signed_area(path0(&next)) > 0.0);
    assert!(check_paths(&layer(&next, "G").paths, 0.0).is_empty());
    assert_eq!(touched, keys([EntityKey::Glyph(g)]));
    assert_eq!(layer(&next, "G").advance_width, 600.0);
}

#[test]
fn apply_geometry_fixes_only_touches_changed_glyphs() {
    let mut font = glyph_font(vec![clockwise()]);
    add_glyph(
        &mut font,
        "clean",
        &[],
        500.0,
        vec![square(0, 0.0, 0.0, 10.0, 10.0, true)],
    );
    add_glyph(&mut font, "empty", &[], 500.0, vec![]);
    let g = id_of(&font, "G");
    let clean = id_of(&font, "clean");

    let (next, touched) = run(&font, fix(vec![], vec![IssueKind::WrongDirection], 0.0));
    assert_eq!(touched, keys([EntityKey::Glyph(g)]));
    assert!(Arc::ptr_eq(&next.glyphs[&clean], &font.glyphs[&clean]));

    // Nothing left to fix: no entity is touched and the font is unchanged.
    let (again, touched) = run(&next, fix(vec![], vec![IssueKind::WrongDirection], 0.0));
    assert!(touched.is_empty());
    assert_eq!(again, next);
}

#[test]
fn apply_geometry_fixes_respects_the_glyph_list() {
    let mut font = glyph_font(vec![clockwise()]);
    add_glyph(&mut font, "H", &[], 500.0, vec![clockwise()]);
    let g = id_of(&font, "G");
    let (next, touched) = run(
        &font,
        fix(vec![by_name("G")], vec![IssueKind::WrongDirection], 0.0),
    );
    assert_eq!(touched, keys([EntityKey::Glyph(g)]));
    assert!(signed_area(&layer(&next, "G").paths[0]) > 0.0);
    assert!(
        signed_area(&layer(&next, "H").paths[0]) < 0.0,
        "H was not listed"
    );

    let err = fails(
        &font,
        fix(vec![by_name("Nope")], vec![IssueKind::WrongDirection], 0.0),
    );
    assert!(matches!(err, EditError::NotFound(_)), "{err:?}");
}

#[test]
fn apply_geometry_fixes_covers_every_master() {
    let mut font = glyph_font(vec![clockwise()]);
    let bold = add_master(&mut font);
    let (next, _) = run(&font, fix(vec![], vec![IssueKind::WrongDirection], 0.0));
    assert!(signed_area(&layer(&next, "G").paths[0]) > 0.0);
    assert!(signed_area(&layer_of(&next, "G", bold).paths[0]) > 0.0);
}

#[test]
fn apply_geometry_fixes_rounds_to_the_grid() {
    let off_grid = closed(
        0,
        vec![
            line(0.2, 0.0),
            line(100.4, 0.0),
            line(100.4, 99.6),
            line(0.2, 99.6),
        ],
    );
    let font = glyph_font(vec![off_grid]);
    let (next, _) = run(&font, fix(vec![], vec![IssueKind::OffGrid], 1.0));
    assert_eq!(
        coords(path0(&next)),
        vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]
    );
    // Only the requested kinds are fixed.
    let (next, touched) = run(&font, fix(vec![], vec![IssueKind::WrongDirection], 1.0));
    assert!(touched.is_empty());
    assert_eq!(next, font);
}

#[test]
fn apply_geometry_fixes_rejects_a_bad_grid() {
    let font = glyph_font(vec![clockwise()]);
    for grid in [-1.0, f64::NAN, f64::INFINITY] {
        let err = fails(&font, fix(vec![], vec![IssueKind::OffGrid], grid));
        assert!(
            matches!(err, EditError::InvalidArgument(_)),
            "{grid}: {err:?}"
        );
    }
}

#[test]
fn edits_never_change_direction() {
    // A clockwise outer contour stays clockwise through node and contour edits.
    let font = glyph_font(vec![clockwise()]);
    let (a, _) = run(
        &font,
        nodes(
            0,
            NodeAction::Move {
                indices: vec![0],
                dx: 1.0,
                dy: 0.0,
            },
        ),
    );
    let (b, _) = run(&a, insert_into(0, 1, 0.5));
    let (c, _) = run(
        &b,
        contour(ContourAction::Transform {
            path_id: PathId(0),
            affine: [2.0, 0.0, 0.0, 2.0, 0.0, 0.0],
        }),
    );
    assert!(signed_area(path0(&c)) < 0.0);
}

fn insert_into(path_id: u32, segment_index: usize, t: f64) -> Command {
    nodes(path_id, NodeAction::Insert { segment_index, t })
}

// ------------------------------------------------- untrusted numeric input

#[test]
fn insert_rejects_huge_segment_indices_without_overflowing() {
    // Agent JSON is untrusted: indices near `usize::MAX` must be a clean
    // error on closed and open contours alike, never an arithmetic overflow.
    let open_path = open(
        0,
        vec![
            line(0.0, 0.0),
            line(100.0, 0.0),
            off(150.0, 50.0),
            off(150.0, 100.0),
            curve(100.0, 150.0),
        ],
    );
    for path in [mixed(0), circle(0, 0.0, 0.0, 100.0), open_path] {
        let font = glyph_font(vec![path]);
        for segment_index in [usize::MAX, usize::MAX - 1, usize::MAX / 2, 1 << 40] {
            let err = fails(&font, insert(segment_index, 0.5));
            assert!(
                matches!(err, EditError::InvalidArgument(_)),
                "{segment_index}: {err:?}"
            );
        }
    }
}

#[test]
fn other_huge_indices_are_rejected_cleanly_too() {
    let font = glyph_font(vec![mixed(0)]);
    for index in [usize::MAX, usize::MAX - 1] {
        for action in [
            NodeAction::Delete { index },
            NodeAction::SetSmooth {
                index,
                smooth: true,
            },
            NodeAction::Move {
                indices: vec![index],
                dx: 1.0,
                dy: 1.0,
            },
        ] {
            let err = fails(&font, nodes(0, action));
            assert!(
                matches!(err, EditError::InvalidArgument(_)),
                "{index}: {err:?}"
            );
        }
    }
}

#[test]
fn apply_geometry_fixes_rejects_a_grid_too_small_to_be_real() {
    // 1e-320 is finite and positive, but dividing a coordinate by it overflows
    // to infinity: the fix used to write infinite coordinates into the font.
    let font = glyph_font(vec![square(0, 0.0, 0.0, 100.0, 100.0, true)]);
    for grid in [1e-320, f64::MIN_POSITIVE, 1e-9, 9.9e-7] {
        let err = fails(&font, fix(vec![], vec![IssueKind::OffGrid], grid));
        assert!(
            matches!(err, EditError::InvalidArgument(_)),
            "{grid}: {err:?}"
        );
    }
    // The smallest accepted grid, and no grid at all, are fine.
    run(&font, fix(vec![], vec![IssueKind::OffGrid], 1e-6));
    run(&font, fix(vec![], vec![IssueKind::OffGrid], 0.0));
}

#[test]
fn apply_geometry_fixes_validates_the_contours_it_writes() {
    // A valid grid can still overflow on huge coordinates: 1e305 / 1e-6 is
    // infinite. The fixed contour must be rejected, not stored.
    let huge = closed(
        0,
        vec![line(1.5e305, 0.5), line(1.5e305, 1.5), line(-1.5e305, 1.5)],
    );
    let font = glyph_font(vec![huge]);
    let err = fails(&font, fix(vec![], vec![IssueKind::OffGrid], 1e-6));
    assert!(matches!(err, EditError::InvalidGeometry(_)), "{err:?}");
}

#[test]
fn apply_geometry_fixes_ignores_contours_that_were_already_malformed() {
    // A contour that was already malformed is left alone by the fixes and must
    // not block fixing the other contours of the glyph.
    let font = glyph_font(vec![clockwise(), closed(1, vec![line(0.0, 0.0)])]);
    let (next, touched) = run(&font, fix(vec![], vec![IssueKind::WrongDirection], 0.0));
    assert!(signed_area(&layer(&next, "G").paths[0]) > 0.0);
    assert_eq!(layer(&next, "G").paths[1], layer(&font, "G").paths[1]);
    assert_eq!(touched, keys([EntityKey::Glyph(id_of(&font, "G"))]));
}
