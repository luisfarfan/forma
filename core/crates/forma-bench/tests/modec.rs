//! Mode C: the pipeline without an agent over the corpus.

use std::process::Command;

use forma_bench::{build_case, run_case};
use serde_json::Value;

#[test]
fn mode_c_builds_a_valid_font_from_the_specimen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let case = build_case("specimen").expect("specimen");
    let result = run_case(&case, &dir.path().join("specimen")).expect("run");
    assert_eq!(result["pipeline_ok"], true, "{result}");
    assert_eq!(result["layout_matched"], true);
    assert_eq!(result["assigned"], 8);
    assert_eq!(result["validate"]["errors"], 0);
    let summary = &result["summary"];
    assert_eq!(summary["glyphs_traced"], 8);
    assert_eq!(summary["holes_correct"], 8);
    assert!(summary["iou_mean"].as_f64().expect("iou") > 0.97);
    assert!(summary["iou_truth_mean"].as_f64().expect("iou truth") > 0.95);
    let stages = result["stages"].as_array().expect("stages");
    let names: Vec<&str> = stages.iter().filter_map(|s| s["stage"].as_str()).collect();
    for expected in [
        "document_create",
        "image_inspect",
        "image_trace",
        "spacing_auto",
        "kerning_auto",
        "kerning_check",
        "validate",
        "export_ttf",
        "export_woff2",
    ] {
        assert!(names.contains(&expected), "missing stage {expected}");
    }
    assert!(stages.iter().all(|s| s["ms"].as_f64().is_some()));
    assert!(dir.path().join("specimen/results.json").exists());
    let ttf = std::fs::read(dir.path().join("specimen/font.ttf")).expect("ttf");
    assert!(ttf.len() > 100);
    let woff2 = std::fs::read(dir.path().join("specimen/font.woff2")).expect("woff2");
    assert_eq!(&woff2[..4], b"wOF2");
}

#[test]
fn rows_that_do_not_match_are_reported_not_guessed() {
    let dir = tempfile::tempdir().expect("tempdir");
    // The H has three pieces: the row has more candidates than characters.
    let case = build_case("multi_piece_h").expect("case");
    let result = run_case(&case, &dir.path().join("multi_piece_h")).expect("run");
    assert_eq!(result["layout_matched"], false);
    assert_eq!(result["assigned"], 0);
    assert_eq!(result["pipeline_ok"], false);
    let unmatched = result["unmatched"].as_array().expect("unmatched");
    assert_eq!(unmatched[0]["reason"], "count");
    assert!(
        unmatched[0]["candidates"].as_u64().expect("candidates")
            > unmatched[0]["characters"].as_u64().expect("characters")
    );
}

#[test]
fn the_command_line_runs_a_level() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = Command::new(env!("CARGO_BIN_EXE_forma-bench"))
        .args(["run", "--mode", "c", "--level", "s", "--out"])
        .arg(dir.path())
        .output()
        .expect("run forma-bench");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("summary.json")).expect("summary"),
    )
    .expect("JSON");
    assert_eq!(summary["cases_ok"], 1);
    assert_eq!(summary["level"], "s");
}
