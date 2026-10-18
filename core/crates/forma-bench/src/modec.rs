//! Mode C: the pipeline without an agent. For now it is the granular
//! sequence of Milestone 2 driven through the engine registry, with the
//! assignments built from the layout of the case:
//!
//! `document_create`, `image_inspect`, `image_trace`, `spacing_auto`,
//! `kerning_auto`, `kerning_check`, `validate`, `export_font` (TTF and WOFF2).
//!
//! The layout is matched to the candidates by reading order with exact row
//! and count matching: a row whose number of candidates differs from the
//! number of characters is reported and left unassigned, never guessed.

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
fn assignments(summary: &Value, layout: &[String]) -> (Vec<Value>, Vec<Value>, usize, bool) {
    let empty = Vec::new();
    let candidates = summary["candidates"].as_array().unwrap_or(&empty);
    let truncated = summary["candidates_truncated"].as_bool().unwrap_or(false);
    let rows = candidates
        .iter()
        .filter_map(|c| c["row"].as_u64())
        .max()
        .map_or(0, |m| m as usize + 1);
    let mut assigned = Vec::new();
    let mut unmatched = Vec::new();
    if rows != layout.len() || truncated {
        unmatched.push(json!({
            "reason": "row_count",
            "image_rows": rows,
            "layout_rows": layout.len(),
        }));
        return (assigned, unmatched, candidates.len(), false);
    }
    for (r, text) in layout.iter().enumerate() {
        let mut in_row: Vec<&Value> = candidates
            .iter()
            .filter(|c| c["row"].as_u64() == Some(r as u64))
            .collect();
        in_row.sort_by_key(|c| c["index"].as_u64().unwrap_or(0));
        let chars: Vec<char> = text.chars().collect();
        if in_row.len() != chars.len() {
            unmatched.push(json!({
                "reason": "count",
                "row": r,
                "candidates": in_row.len(),
                "characters": chars.len(),
            }));
            continue;
        }
        for (c, ch) in in_row.iter().zip(chars) {
            assigned.push(json!({
                "candidate": c["index"],
                "name": ch.to_string(),
                "unicodes": [codepoint(ch)],
            }));
        }
    }
    let ok = unmatched.is_empty();
    (assigned, unmatched, candidates.len(), ok)
}

/// The traced outline of `name`, mapped over the true outline: same center
/// and the scale that makes the heights equal. The comparison measures the
/// shape, not the placement the tracer chose.
fn glyph_px(paths: &[ModelPath], truth: &TruthGlyph) -> Option<Vec<BezPath>> {
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
    let inspected = run.json("image_inspect", "image_inspect", json!({ "path": image }))?;
    let (assigned, unmatched, candidates, matched) = assignments(&inspected, &case.layout);
    let layout_chars: usize = case.layout.iter().map(|r| r.chars().count()).sum();
    let mut out = json!({
        "candidates": candidates,
        "layout_chars": layout_chars,
        "layout_matched": matched,
        "unmatched": unmatched,
        "assigned": assigned.len(),
    });
    if assigned.is_empty() {
        out["pipeline_ok"] = json!(false);
        return Ok(out);
    }

    let rev = run.rev(&doc)?;
    let traced = run.json(
        "image_trace",
        "image_trace",
        json!({ "doc": doc, "base_rev": rev, "path": image, "assignments": assigned, "detail": "full" }),
    )?;
    let mut glyphs = Vec::new();
    for entry in traced["traced"].as_array().cloned().unwrap_or_default() {
        let name = entry["name"].as_str().unwrap_or_default().to_owned();
        let mut g = json!({
            "name": name,
            "iou": entry["iou"],
            "nodes": entry["nodes"],
        });
        if let Some(truth) = case.truth.as_ref().and_then(|t| t.glyphs.get(&name)) {
            measure_against_truth(run, &doc, &name, truth, &mut g);
        }
        glyphs.push(g);
    }
    out["glyphs"] = json!(glyphs);

    // Spacing, kerning and checks; a failing stage is recorded and the
    // pipeline goes on, so one problem does not hide the others.
    let rev = run.rev(&doc)?;
    // The reference glyphs are H and O; an alphabet without them (a set of
    // other letters) falls back to the first two traced glyphs so that the
    // rest of the pipeline can still be measured.
    let names: Vec<&str> = glyphs.iter().filter_map(|g| g["name"].as_str()).collect();
    let mut spacing_args = json!({ "doc": doc, "base_rev": rev });
    if !(names.contains(&"H") && names.contains(&"O")) && !names.is_empty() {
        spacing_args["references"] = json!([names[0], names[names.len().min(2) - 1]]);
    }
    let spacing = run.json("spacing_auto", "spacing_auto", spacing_args);
    let rev = run.rev(&doc)?;
    let kerning = run.json(
        "kerning_auto",
        "kerning_auto",
        json!({ "doc": doc, "base_rev": rev }),
    );
    let checked = run.json(
        "kerning_check",
        "kerning_check",
        json!({ "doc": doc, "min_distance": 20.0 }),
    );
    let validated = run.json("validate", "validate", json!({ "doc": doc }));
    out["spacing_ok"] = json!(spacing.is_ok());
    out["kerning_pairs"] = kerning
        .as_ref()
        .map_or(Value::Null, |k| k["pairs_set"].clone());
    out["collisions_total"] = checked
        .as_ref()
        .map_or(Value::Null, |c| c["collisions_total"].clone());
    let report = validated
        .as_ref()
        .map_or(Value::Null, |v| v["report"].clone());
    out["validate"] = json!({
        "errors": report["errors"],
        "warnings": report["warnings"],
        "compiled": report["compiled"],
    });

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
    out["pipeline_ok"] =
        json!(matched && run.stages.iter().all(|s| s.error.is_none()) && errors == Some(0));
    out["summary"] = summarize(&glyphs);
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
