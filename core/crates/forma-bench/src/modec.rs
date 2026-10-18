//! Mode C: the pipeline without an agent, through the engine registry:
//! `document_create`, `font_from_image` (trace, synthesize, compose, space,
//! kern, checks: one transaction) and `export_font` (TTF and WOFF2).
//!
//! Each case reports **fidelity** (the traced outlines against the truth:
//! overlap, counters, review flags, outline quality) and **typography**
//! (synthesis, composition, spacing, kerning, collisions, coverage,
//! validation) separately: a faithful trace is not yet a good font, and a
//! good-looking font may not be faithful.

use std::path::{Path, PathBuf};
use std::time::Instant;

use forma_engine::{Engine, EngineConfig, Policy, ToolError, ToolOutput};
use forma_geometry::to_bezpath;
use forma_model::{Actor, Path as ModelPath};
use kurbo::{Affine, BezPath, Shape};
use serde_json::{Value, json};

use crate::corpus::{CorpusCase, Level, TruthGlyph, corpus};
use crate::error::BenchError;
use crate::metrics::{count_holes, shape_iou};

/// One stage of the pipeline.
#[derive(Clone, Debug)]
struct Stage {
    name: &'static str,
    ms: f64,
    error: Option<String>,
}

struct Run {
    engine: Engine,
    actor: Actor,
    stages: Vec<Stage>,
}

impl Run {
    fn call(
        &mut self,
        name: &'static str,
        tool: &str,
        args: Value,
    ) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let result = self.engine.call(&self.actor, tool, args);
        self.stages.push(Stage {
            name,
            ms: started.elapsed().as_secs_f64() * 1000.0,
            error: result.as_ref().err().map(ToString::to_string),
        });
        result
    }

    fn json(&mut self, name: &'static str, tool: &str, args: Value) -> Result<Value, ToolError> {
        match self.call(name, tool, args)? {
            ToolOutput::Json(v) => Ok(v),
            ToolOutput::Image { summary, .. } => Ok(summary),
        }
    }

    fn rev(&mut self, doc: &str) -> Result<u64, ToolError> {
        let inspected = self
            .engine
            .call(&self.actor, "document_inspect", json!({ "doc": doc }))?;
        match inspected {
            ToolOutput::Json(v) => Ok(v["rev"].as_u64().unwrap_or(0)),
            ToolOutput::Image { .. } => Ok(0),
        }
    }
}

fn codepoint(ch: char) -> String {
    format!("U+{:04X}", u32::from(ch))
}

/// The assignments of the candidates to the characters of the layout, and the
/// rows that did not match.
/// The assignments of a case: its layout matched with the image's rows
/// (see [`forma_trace::match_rows`]); rows that do not match exactly or
/// with a clear grouping of pieces are reported and left unassigned.
fn assignments(image: &[u8], layout: &[String]) -> (Vec<Value>, Vec<Value>, usize, bool) {
    let mut assigned = Vec::new();
    let mut unmatched = Vec::new();
    let opts = forma_trace::TraceOptions::default();
    let Ok(decoded) = forma_trace::decode(image) else {
        unmatched.push(json!({ "reason": "decode" }));
        return (assigned, unmatched, 0, false);
    };
    let Ok(analysis) = forma_trace::analyze(&decoded, &opts) else {
        unmatched.push(json!({ "reason": "analyze" }));
        return (assigned, unmatched, 0, false);
    };
    let stroke = forma_trace::typical_stroke_px(&decoded, &opts).unwrap_or(0.0);
    let candidates = analysis.candidates.len();
    if analysis.rows.len() != layout.len() {
        unmatched.push(json!({
            "reason": "row_count",
            "image_rows": analysis.rows.len(),
            "layout_rows": layout.len(),
        }));
        return (assigned, unmatched, candidates, false);
    }
    let expected: Vec<usize> = layout.iter().map(|r| r.chars().count()).collect();
    for (r, (text, matched)) in layout
        .iter()
        .zip(forma_trace::match_rows(&analysis, &expected, stroke))
        .enumerate()
    {
        let groups = match matched {
            forma_trace::RowMatch::Exact { groups } | forma_trace::RowMatch::Grouped { groups } => {
                groups
            }
            other => {
                unmatched.push(json!({ "row": r, "match": other }));
                continue;
            }
        };
        for (group, ch) in groups.iter().zip(text.chars()) {
            let mut a = json!({ "name": ch.to_string(), "unicodes": [codepoint(ch)] });
            if group.len() == 1 {
                a["candidate"] = json!(group[0]);
            } else {
                a["candidates"] = json!(group);
            }
            assigned.push(a);
        }
    }
    let ok = unmatched.is_empty();
    (assigned, unmatched, candidates, ok)
}

/// The traced outline of `name`, mapped over the true outline: same center
/// and the scale that makes the heights equal. The comparison measures the
/// shape, not the placement the tracer chose.
pub(crate) fn glyph_px(paths: &[ModelPath], truth: &TruthGlyph) -> Option<Vec<BezPath>> {
    let beziers: Vec<BezPath> = paths.iter().filter_map(|p| to_bezpath(p).ok()).collect();
    let (mut union, mut have) = (kurbo::Rect::ZERO, false);
    for b in &beziers {
        let r = b.bounding_box();
        union = if have { union.union(r) } else { r };
        have = true;
    }
    let t = truth.px_rect;
    if !have || union.height() <= 0.0 || t[3] <= t[1] {
        return None;
    }
    let scale = (t[3] - t[1]) / union.height();
    let (gcx, gcy) = ((union.x0 + union.x1) / 2.0, (union.y0 + union.y1) / 2.0);
    let (tcx, tcy) = ((t[0] + t[2]) / 2.0, (t[1] + t[3]) / 2.0);
    let affine = Affine::new([
        scale,
        0.0,
        0.0,
        -scale,
        tcx - scale * gcx,
        tcy + scale * gcy,
    ]);
    Some(beziers.into_iter().map(|b| affine * b).collect())
}

fn absolute(path: &Path) -> Result<PathBuf, BenchError> {
    std::path::absolute(path).map_err(BenchError::from)
}

/// Runs mode C on one case. Writes the image and the exported fonts into
/// `case_dir` and returns the `results.json` content.
///
/// # Errors
///
/// [`BenchError`] on I/O failures. Failures of the pipeline itself are
/// reported inside the result, not raised.
pub fn run_case(case: &CorpusCase, case_dir: &Path) -> Result<Value, BenchError> {
    std::fs::create_dir_all(case_dir)?;
    let dir = absolute(case_dir)?;
    let image_path = dir.join(format!("input.{}", case.extension));
    std::fs::write(&image_path, &case.image)?;
    let engine = Engine::new(EngineConfig {
        allowed_roots: vec![dir.clone()],
        policy: Policy::Auto,
        ..EngineConfig::default()
    });
    let mut run = Run {
        engine,
        actor: Actor::Agent("forma-bench".into()),
        stages: Vec::new(),
    };
    let started = Instant::now();
    let outcome = pipeline(&mut run, case, &image_path, &dir);
    let total_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut result = match outcome {
        Ok(v) => v,
        Err(error) => json!({ "pipeline_error": error.to_string() }),
    };
    let stages: Vec<Value> = run
        .stages
        .iter()
        .map(|s| json!({ "stage": s.name, "ms": round3(s.ms), "ok": s.error.is_none(), "error": s.error }))
        .collect();
    result["case"] = json!(case.id);
    result["level"] = json!(case.level.name());
    result["image"] = json!({ "bytes": case.image.len(), "extension": case.extension });
    result["layout"] = json!(case.layout);
    result["stages"] = json!(stages);
    result["total_ms"] = json!(round3(total_ms));
    std::fs::write(
        case_dir.join("results.json"),
        serde_json::to_string_pretty(&result)?,
    )?;
    Ok(result)
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn pipeline(
    run: &mut Run,
    case: &CorpusCase,
    image: &Path,
    dir: &Path,
) -> Result<Value, ToolError> {
    let created = run.json(
        "document_create",
        "document_create",
        json!({ "family_name": "Bench" }),
    )?;
    let doc = created["doc"].as_str().unwrap_or_default().to_owned();
    // The engine's own matching decides; this read only reports what a
    // reading-order match would assign, for the layout statistics.
    let (assigned, unmatched, candidates, matched) = assignments(&case.image, &case.layout);
    let layout_chars: usize = case.layout.iter().map(|r| r.chars().count()).sum();
    let mut out = json!({
        "candidates": candidates,
        "layout_chars": layout_chars,
        "layout_matched": matched,
        "unmatched": unmatched,
        "assigned": assigned.len(),
    });
    let rev = run.rev(&doc)?;
    let made = run.json(
        "font_from_image",
        "font_from_image",
        json!({ "doc": doc, "base_rev": rev, "path": image, "layout": case.layout,
            "detail": "full" }),
    );
    let made = match made {
        Ok(v) => v,
        Err(error) => {
            out["pipeline_ok"] = json!(false);
            out["pipeline_error"] = json!(error.to_string());
            return Ok(out);
        }
    };
    let stage = |name: &str| -> Value {
        made["stages"]
            .as_array()
            .and_then(|s| s.iter().find(|s| s["name"] == name))
            .cloned()
            .unwrap_or(Value::Null)
    };
    // The detail of each traced glyph is in the call's report.
    let report: Value = made["report_path"]
        .as_str()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let mut glyphs = Vec::new();
    for entry in report["body"]["traced"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let name = entry["name"].as_str().unwrap_or_default().to_owned();
        let quality = &entry["quality"];
        let mut g = json!({
            "name": name,
            "iou": entry["iou"],
            "hausdorff_p95_px": quality["hausdorff_p95_px"],
            "fit_deviation_px": quality["fit_deviation_px"],
            "topology_ok": quality["topology_ok"],
            "g1_violations": quality["g1_violations"],
            "nodes_per_1000": quality["nodes_per_1000"],
            "attempt": entry["attempt"],
            "needs_review": !entry["reasons"].as_array().is_none_or(Vec::is_empty),
            "reasons": entry["reasons"],
        });
        if let Some(truth) = case.truth.as_ref().and_then(|t| t.glyphs.get(&name)) {
            measure_against_truth(run, &doc, &name, truth, &mut g);
        }
        glyphs.push(g);
    }
    out["glyphs"] = json!(glyphs);
    out["engine_ms"] = made["engine_ms"].clone();
    out["stages_engine"] = made["stages"].clone();
    out["attention"] = made["attention"].clone();
    let validated = stage("validate");
    out["validate"] = json!({
        "errors": validated["counts"]["errors"],
        "warnings": validated["counts"]["warnings"],
    });
    out["collisions_total"] = stage("check_collisions")["counts"]["collisions"].clone();

    let rev = run.rev(&doc)?;
    let mut exports = serde_json::Map::new();
    for (format, file) in [("ttf", "font.ttf"), ("woff2", "font.woff2")] {
        let path = dir.join(file);
        let done = run.json(
            if format == "ttf" {
                "export_ttf"
            } else {
                "export_woff2"
            },
            "export_font",
            json!({ "doc": doc, "base_rev": rev, "format": format, "path": path }),
        );
        let bytes = std::fs::metadata(&path).map(|m| m.len()).ok();
        exports.insert(
            format.to_owned(),
            json!({ "ok": done.is_ok(), "bytes": bytes }),
        );
    }
    out["export"] = Value::Object(exports);

    let errors = out["validate"]["errors"].as_u64();
    let rows_ok = made["attention"].as_array().is_none_or(|a| {
        !a.iter()
            .any(|i| i["kind"] == "row_mismatch" || i["kind"] == "ambiguous_grouping")
    });
    out["pipeline_ok"] =
        json!(rows_ok && run.stages.iter().all(|s| s.error.is_none()) && errors == Some(0));
    out["summary"] = summarize(&glyphs);
    // Irregularity kept counts only drawings with irregularity of their own
    // and an image that shows them as they are (a bleeding ink moves the
    // edge, and a faithful trace follows it).
    out["irregular"] =
        json!(case.truth.as_ref().is_some_and(|t| {
            t.irregularity_amplitude_px.is_some_and(|a| a > 0.0) && !t.bleeds
        }));
    out["typography"] = json!({
        "synthesized": stage("synthesize")["counts"]["made"],
        "needs_design": stage("synthesize")["counts"]["needs_design"],
        "composed": stage("compose")["counts"]["composed"],
        "spaced": stage("space")["counts"]["spaced"],
        "kerning_commands": stage("kern")["counts"]["commands"],
        "collisions": out["collisions_total"],
        "charset_covered": stage("charset")["counts"]["covered"],
        "charset_required": stage("charset")["counts"]["required"],
        "validate_errors": errors,
    });
    Ok(out)
}

fn measure_against_truth(run: &mut Run, doc: &str, name: &str, truth: &TruthGlyph, g: &mut Value) {
    // This read is not a stage of the pipeline.
    let fetched = run.engine.call(
        &run.actor,
        "glyph_get",
        json!({ "doc": doc, "glyph": name }),
    );
    let Ok(ToolOutput::Json(v)) = fetched else {
        return;
    };
    let paths: Vec<ModelPath> =
        serde_json::from_value(v["glyph"]["paths"].clone()).unwrap_or_default();
    let Some(shapes) = glyph_px(&paths, truth) else {
        return;
    };
    let truth_shapes = [truth.path.clone()];
    if let Ok(iou) = shape_iou(&truth_shapes, &shapes) {
        g["iou_truth"] = json!(round3(iou));
    }
    if let Ok(holes) = count_holes(&shapes) {
        g["holes"] = json!(holes);
        g["holes_truth"] = json!(truth.holes);
        g["holes_match"] = json!(holes == truth.holes);
    }
}

fn summarize(glyphs: &[Value]) -> Value {
    let ious: Vec<f64> = glyphs.iter().filter_map(|g| g["iou"].as_f64()).collect();
    let truth_ious: Vec<f64> = glyphs
        .iter()
        .filter_map(|g| g["iou_truth"].as_f64())
        .collect();
    let mean = |v: &[f64]| (!v.is_empty()).then(|| round3(v.iter().sum::<f64>() / v.len() as f64));
    let min = |v: &[f64]| v.iter().copied().reduce(f64::min).map(round3);
    let with_holes = glyphs
        .iter()
        .filter(|g| g["holes_match"].is_boolean())
        .count();
    let holes_ok = glyphs
        .iter()
        .filter(|g| g["holes_match"] == json!(true))
        .count();
    json!({
        "glyphs_traced": glyphs.len(),
        "iou_mean": mean(&ious),
        "iou_min": min(&ious),
        "iou_truth_mean": mean(&truth_ious),
        "iou_truth_min": min(&truth_ious),
        "holes_correct": holes_ok,
        "holes_checked": with_holes,
        "needs_review": glyphs.iter().filter(|g| g["needs_review"] == json!(true)).count(),
        // Glyphs whose outline keeps the true shape (irregular, on level O).
        "truth_good": truth_ious.iter().filter(|i| **i >= 0.97).count(),
        "undetected_topology_errors": glyphs
            .iter()
            .filter(|g| g["holes_match"] == json!(false) && g["needs_review"] != json!(true))
            .count(),
    })
}

/// Runs mode C over a whole level, writing `<out>/<case>/results.json` and
/// `<out>/summary.json`. Returns the summary.
///
/// # Errors
///
/// [`BenchError`] on I/O failures or if a case cannot be built.
pub fn run_level(level: Level, out: &Path) -> Result<Value, BenchError> {
    std::fs::create_dir_all(out)?;
    let mut cases = Vec::new();
    for case in corpus(level)? {
        let result = run_case(&case, &out.join(&case.id))?;
        cases.push(json!({
            "case": case.id,
            "irregular": result["irregular"],
            "typography": result["typography"],
            "engine_ms": result["engine_ms"],
            "pipeline_ok": result["pipeline_ok"],
            "layout_matched": result["layout_matched"],
            "assigned": result["assigned"],
            "candidates": result["candidates"],
            "layout_chars": result["layout_chars"],
            "summary": result["summary"],
            "validate_errors": result["validate"]["errors"],
            "total_ms": result["total_ms"],
        }));
    }
    let ok = cases
        .iter()
        .filter(|c| c["pipeline_ok"] == json!(true))
        .count();
    let summary = json!({
        "mode": "c",
        "level": level.name(),
        "cases": cases,
        "cases_total": cases.len(),
        "cases_ok": ok,
    });
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_string_pretty(&summary)?,
    )?;
    Ok(summary)
}
