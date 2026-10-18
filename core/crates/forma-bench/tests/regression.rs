//! The mode C quality gate: the corpus traced as the engine traces it must
//! not get worse than the saved baseline (`baseline.json`). S, L and O run
//! with every test run; M and M-hand, which take minutes, run nightly
//! (`--ignored`).
//!
//! The gate: no topology error goes undetected, every counter and slit of
//! the generated cases is kept, the font validates, the overlap with the
//! truth does not fall more than 0.003 below the baseline, and on the
//! organic level at least 80 % of the irregular glyphs keep their shape
//! (overlap with their irregular truth ≥ 0.97; drawings whose image bleeds
//! are left out, since a faithful trace follows the bleeding edge).

use forma_bench::{Level, run_level};
use serde_json::Value;

/// Allowed fall of the mean overlap with the truth.
const IOU_SLACK: f64 = 0.003;

fn baseline() -> Value {
    let text = include_str!("../baseline.json");
    serde_json::from_str(text).expect("baseline.json")
}

struct Measured {
    cases_ok: u64,
    iou_truth_mean: f64,
    holes_correct_pct: f64,
    undetected: u64,
    validate_errors: u64,
    irregularity_kept_pct: f64,
    synthesized: u64,
    needs_design: u64,
}

fn measure(level: Level) -> Measured {
    let dir = tempfile::tempdir().expect("tempdir");
    let summary = run_level(level, dir.path()).expect("run");
    let cases = summary["cases"].as_array().cloned().unwrap_or_default();
    let (mut holes, mut checked, mut undetected, mut errors) = (0, 0, 0, 0);
    let (mut synthesized, mut needs_design) = (0, 0);
    let mut ious = Vec::new();
    let (mut kept, mut glyphs) = (0usize, 0usize);
    for c in &cases {
        let s = &c["summary"];
        holes += s["holes_correct"].as_u64().unwrap_or(0);
        checked += s["holes_checked"].as_u64().unwrap_or(0);
        undetected += s["undetected_topology_errors"].as_u64().unwrap_or(0);
        errors += c["validate_errors"].as_u64().unwrap_or(0);
        synthesized += c["typography"]["synthesized"].as_u64().unwrap_or(0);
        needs_design += c["typography"]["needs_design"].as_u64().unwrap_or(0);
        if let Some(m) = s["iou_truth_mean"].as_f64() {
            ious.push(m);
        }
        if c["irregular"] == true {
            glyphs += s["glyphs_traced"].as_u64().unwrap_or(0) as usize;
            kept += s["truth_good"].as_u64().unwrap_or(0) as usize;
        }
    }
    let pct = |a: f64, b: f64| if b > 0.0 { 100.0 * a / b } else { 100.0 };
    Measured {
        cases_ok: summary["cases_ok"].as_u64().unwrap_or(0),
        iou_truth_mean: ious.iter().sum::<f64>() / ious.len().max(1) as f64,
        holes_correct_pct: pct(holes as f64, checked as f64),
        undetected,
        validate_errors: errors,
        irregularity_kept_pct: pct(kept as f64, glyphs as f64),
        synthesized,
        needs_design,
    }
}

fn gate(level: Level) {
    let base = &baseline()[level.name()];
    let m = measure(level);
    let name = level.name();
    assert_eq!(m.undetected, 0, "{name}: topology errors went undetected");
    assert_eq!(m.validate_errors, 0, "{name}: the fonts must validate");
    assert!(
        m.cases_ok >= base["cases_ok"].as_u64().unwrap_or(0),
        "{name}: {} cases ok, the baseline has {}",
        m.cases_ok,
        base["cases_ok"]
    );
    let iou = base["iou_truth_mean"].as_f64().unwrap_or(0.0);
    assert!(
        m.iou_truth_mean >= iou - IOU_SLACK,
        "{name}: overlap with the truth {:.4}, baseline {iou}",
        m.iou_truth_mean
    );
    if let Some(holes) = base["holes_correct_pct"].as_f64() {
        assert!(
            m.holes_correct_pct >= holes,
            "{name}: {:.1} % of counters kept, baseline {holes}",
            m.holes_correct_pct
        );
    }
    if let Some(kept) = base["irregularity_kept_pct"].as_f64() {
        assert!(
            m.irregularity_kept_pct >= kept,
            "{name}: {:.1} % of the organic glyphs kept their shape (at least {kept})",
            m.irregularity_kept_pct
        );
    }
    if let Some(made) = base["synthesized"].as_u64() {
        assert!(
            m.synthesized >= made,
            "{name}: {} synthesized, baseline {made}",
            m.synthesized
        );
    }
    if let Some(design) = base["needs_design"].as_u64() {
        assert!(
            m.needs_design <= design,
            "{name}: {} need a design, baseline {design}",
            m.needs_design
        );
    }
}

#[test]
fn regression_gate_on_s_l_and_o() {
    for level in [Level::S, Level::L, Level::O] {
        gate(level);
    }
}

#[test]
#[ignore = "minutes long: run nightly (cargo test -p forma-bench --release -- --ignored)"]
fn m_metrics_against_truth() {
    for level in [Level::M, Level::MHand] {
        gate(level);
    }
}
