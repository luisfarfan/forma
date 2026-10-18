//! The corpus: determinism, shape of the levels and ground truth.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use forma_bench::metrics::{rasterize, shape_iou};
use forma_bench::{L_IDS, Level, O_IDS, build_case, case_ids, corpus};
use forma_render::mask_iou;

#[test]
fn corpus_is_deterministic() {
    for level in [Level::S, Level::L, Level::O] {
        let a = corpus(level).expect("first build");
        let b = corpus(level).expect("second build");
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.id, y.id);
            assert_eq!(x.image, y.image, "{} differs between builds", x.id);
            assert_eq!(x.layout, y.layout);
        }
    }
    // One alphabet at the smallest size, under every degradation.
    for degradation in ["none", "jpeg70", "blurnoise", "rot15"] {
        let id = format!("m-inter-c60-{degradation}");
        let a = build_case(&id).expect("first");
        let b = build_case(&id).expect("second");
        assert_eq!(a.image, b.image, "{id} differs between builds");
    }
}

#[test]
fn degradations_change_the_image_and_keep_the_layout() {
    let clean = build_case("m-fraunces-c60-none").expect("clean");
    let jpeg = build_case("m-fraunces-c60-jpeg70").expect("jpeg");
    let noisy = build_case("m-fraunces-c60-blurnoise").expect("noisy");
    let turned = build_case("m-fraunces-c60-rot15").expect("rotated");
    assert_eq!(jpeg.extension, "jpg");
    assert_eq!(&jpeg.image[..2], &[0xFF, 0xD8]);
    assert_eq!(clean.extension, "png");
    assert_ne!(clean.image, noisy.image);
    assert_ne!(clean.image, turned.image);
    for case in [&clean, &jpeg, &noisy, &turned] {
        assert_eq!(case.layout, vec!["ABCDEFGHI", "JKLMNOPQR", "STUVWXYZ"]);
        let truth = case.truth.as_ref().expect("truth");
        assert_eq!(truth.glyphs.len(), 26);
    }
    // The rotation moves the true outlines with the image.
    let (a, b) = (clean.truth.expect("truth"), turned.truth.expect("truth"));
    assert_ne!(a.glyphs["Z"].px_rect, b.glyphs["Z"].px_rect);
}

#[test]
fn m_has_three_fonts_times_sizes_times_degradations() {
    let ids = case_ids(Level::M);
    assert_eq!(ids.len(), 36);
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 36);
    for font in ["inter", "eb-garamond", "fraunces"] {
        for cap in [60, 110, 200] {
            for degradation in ["none", "jpeg70", "blurnoise", "rot15"] {
                let id = format!("m-{font}-c{cap}-{degradation}");
                assert!(ids.contains(&id), "{id}");
            }
        }
    }
    let hand = case_ids(Level::MHand);
    assert_eq!(hand.len(), 12);
    assert!(hand.iter().all(|id| id.contains("patrick-hand")));
}

#[test]
fn l_ids_are_stable() {
    let ids: Vec<String> = corpus(Level::L)
        .expect("L")
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(
        ids,
        [
            "multi_piece_h",
            "slit_1px",
            "slit_2px",
            "slit_3px",
            "small_counter",
            "organic_wobble",
            "uneven_light",
            "decorative_row",
            "touching_pair",
            "stray_dot",
            "i_dot",
        ]
    );
    assert_eq!(ids, L_IDS);
    let o: Vec<String> = case_ids(Level::O);
    assert_eq!(o, O_IDS);
}

#[test]
fn truth_hole_counts() {
    let case = build_case("m-inter-c110-none").expect("inter");
    let truth = case.truth.expect("truth");
    let holes = |c: &str| truth.glyphs[c].holes;
    assert_eq!(holes("O"), 1);
    assert_eq!(holes("B"), 2);
    assert_eq!(holes("H"), 0);
    assert_eq!(holes("A"), 1);
    assert_eq!(holes("D"), 1);
    assert_eq!(holes("L"), 0);
}

#[test]
fn l_cases_know_their_topology() {
    let truth = |id: &str| build_case(id).expect(id).truth.expect("truth");
    for id in ["slit_1px", "slit_2px", "slit_3px", "small_counter"] {
        assert_eq!(truth(id).glyphs["O"].holes, 1, "{id}");
    }
    let h = truth("multi_piece_h");
    assert_eq!(h.glyphs["H"].pieces, 3);
    assert_eq!(h.glyphs["H"].holes, 0);
    let i = truth("i_dot");
    assert_eq!(i.glyphs["i"].pieces, 2);
    assert_eq!(truth("stray_dot").stray_pieces, 1);
    // The row of the touching pair has one candidate fewer than characters
    // in the image (the pair is one piece); the truth keeps both letters.
    assert_eq!(truth("touching_pair").glyphs.len(), 4);
}

#[test]
fn o_cases_have_known_irregularity() {
    for id in O_IDS {
        let case = build_case(id).expect(id);
        let truth = case.truth.expect("truth");
        let amplitude = truth.irregularity_amplitude_px;
        assert!(amplitude.is_some(), "{id} records no irregularity");
        assert!(amplitude.unwrap_or(-1.0) >= 0.0, "{id}");
    }
    let amp = |id: &str| {
        build_case(id)
            .expect(id)
            .truth
            .expect("truth")
            .irregularity_amplitude_px
            .expect("amplitude")
    };
    assert!(amp("rough_brush") >= 2.0);
    assert!(amp("bulb_organic") >= 1.2);
    assert!(amp("hand_pressure") > 1.0);
    let wobble = build_case("organic_wobble")
        .expect("wobble")
        .truth
        .expect("truth");
    assert!(wobble.irregularity_amplitude_px.expect("amplitude") >= 1.5);
}

#[test]
fn inline_outline_nested_holes() {
    let truth = build_case("inline_outline")
        .expect("inline_outline")
        .truth
        .expect("truth");
    // A bar: a hole around a core. A ring: that gap, and the counter.
    assert_eq!(truth.glyphs["I"].holes, 1);
    assert_eq!(truth.glyphs["O"].holes, 2);
    assert!(truth.glyphs["B"].holes >= 3);
    let counters = build_case("complex_counters")
        .expect("complex_counters")
        .truth
        .expect("truth");
    assert_eq!(counters.glyphs["B"].holes, 2);
    assert_eq!(counters.glyphs["E"].holes, 2);
    assert_eq!(counters.glyphs["H"].holes, 6);
    assert_eq!(counters.glyphs["A"].holes, 1);
}

#[test]
fn stencil_has_many_pieces_per_letter() {
    let truth = build_case("stencil")
        .expect("stencil")
        .truth
        .expect("truth");
    assert!(
        truth.glyphs["O"].pieces >= 3,
        "{}",
        truth.glyphs["O"].pieces
    );
    assert!(truth.glyphs["I"].pieces >= 3);
}

#[test]
fn s_truth_matches_the_image() {
    let case = build_case("specimen").expect("specimen");
    let truth = case.truth.expect("truth");
    assert_eq!(truth.glyphs.len(), 8);
    let image = image::load_from_memory(&case.image)
        .expect("decode")
        .to_rgb8();
    let (w, h) = (image.width(), image.height());
    // The ink is orange: its red channel is far above the background's.
    let ink: Vec<u8> = image
        .pixels()
        .map(|p| if p.0[0] > 136 && p.0[2] < 120 { 255 } else { 0 })
        .collect();
    let from_image = forma_render::Mask {
        width: w,
        height: h,
        data: ink,
    };
    let shapes: Vec<_> = truth.glyphs.values().map(|g| g.path.clone()).collect();
    let from_truth = rasterize(&shapes, w, h).expect("raster");
    let iou = mask_iou(&from_image, &from_truth, 128).expect("iou");
    assert!(iou > 0.97, "iou {iou}");
    assert_eq!(truth.glyphs["O"].holes, 1);
    assert_eq!(truth.glyphs["B"].holes, 2);
}

#[test]
fn iou_helper_agrees_with_itself() {
    let truth = build_case("slit_2px").expect("case").truth.expect("truth");
    let g = &truth.glyphs["H"];
    let iou = shape_iou(std::slice::from_ref(&g.path), std::slice::from_ref(&g.path)).expect("iou");
    assert_eq!(iou, 1.0);
}

#[test]
fn ground_truth_py_runs_on_inter() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let python = if cfg!(windows) {
        root.join("tools/oracle/.venv/Scripts/python.exe")
    } else {
        root.join("tools/oracle/.venv/bin/python")
    };
    if !python.exists() {
        eprintln!("skipped: the oracle venv {python:?} is missing");
        return;
    }
    let output = Command::new(&python)
        .arg(root.join("tools/bench/ground_truth.py"))
        .arg(root.join("tests/fixtures/inter/Inter-Regular.ttf"))
        .output()
        .expect("run ground_truth.py");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let truth: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(truth["units_per_em"], 2048);
    assert!(truth["glyphs"]["A"]["advance"].as_f64().expect("advance") > 0.0);
    assert!(truth["glyphs"]["A"]["lsb"].is_number());
    let kerning = truth["kerning"].as_array().expect("kerning");
    assert!(!kerning.is_empty(), "Inter has GPOS kerning");
    assert!(kerning[0]["left"].is_string() && kerning[0]["value"].is_number());
}
