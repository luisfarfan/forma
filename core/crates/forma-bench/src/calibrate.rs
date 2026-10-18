//! Calibration of the quality gates (spec §4) and of the style score
//! (§5.5) on the corpus.
//!
//! * **Fidelity.** Every glyph of the corpus is traced as the engine traces
//!   it. A glyph whose outline matches its truth (IoU ≥ 0.97 against the
//!   true shape, same counters) is *good*. Known defects are then injected
//!   into the good outlines (a counter closed, a piece dropped, a 3 px
//!   spike, a 1 px shift, the weight +30 %)
//!   and judged with [`forma_trace::assess`]. For each gate and threshold:
//!   how many good glyphs it flags (false positives) and how many defects
//!   get through every gate (false negatives).
//! * **Style.** The traced glyphs of each alphabet make a style kit; every
//!   recipe is drawn with it (good) and with kits that are wrong in one
//!   respect (stroke ×0.7 and ×1.3, the other kind of terminal, balls in a
//!   font of plain ends), always scored against the true kit.

use std::collections::BTreeMap;
use std::sync::Arc;

use forma_geometry::{
    Cap, Skeleton, SkeletonNode, StrokeStyle, contour_depths, remove_overlap, signed_area, stroke,
    transform_paths,
};
use forma_model::{
    Font, Glyph, GlyphId, Layer, NodeKind, Path, PathId, Provenance, ProvenanceDetail, Source,
    Transform,
};
use forma_trace::{Placement, QualityReport, RowMatch, THRESHOLDS, TraceOptions, TracedGlyph};
use serde_json::{Value, json};

use crate::corpus::{CorpusCase, Level, corpus};
use crate::error::BenchError;
use crate::metrics::{count_holes, shape_iou};
use crate::modec::glyph_px;

/// The cap height and units per em glyphs are traced for (a new document).
const CAP: f64 = 700.0;
const UPM: u16 = 1000;
/// A traced glyph is good when it overlaps its truth at least this much.
const GOOD_IOU: f64 = 0.97;

/// The gates a report is judged with, as numbers to sweep.
#[derive(Clone, Copy, Debug)]
pub struct Gates {
    /// Lowest overlap with the ink.
    pub iou_min: f64,
    /// Highest 95th percentile distance, pixels.
    pub p95: f64,
    /// Highest distance, pixels.
    pub max: f64,
    /// Highest 99th percentile distance of a faithful trace, pixels.
    pub fit: f64,
}

impl Gates {
    /// The thresholds in force.
    #[must_use]
    pub fn current() -> Gates {
        Gates {
            iou_min: THRESHOLDS.iou_min,
            p95: THRESHOLDS.hausdorff_p95_px,
            max: THRESHOLDS.hausdorff_max_px,
            fit: THRESHOLDS.fit_faithful_px,
        }
    }
}

/// Whether `q` fails any gate at `g` (the gates of
/// [`QualityReport::failures`], with the swept numbers).
#[must_use]
pub fn flagged(q: &QualityReport, g: Gates) -> bool {
    let grid = q.grid_px;
    // The fit budget of a faithful trace is swept; a clean or hard-edged
    // one keeps its own.
    let own_fit = q.fit_budget_px - grid;
    let fit_budget = if (own_fit - THRESHOLDS.fit_faithful_px).abs() < 1e-6 {
        g.fit + grid
    } else {
        q.fit_budget_px
    };
    !q.topology_ok
        || !q.features.is_empty()
        || q.crossings > 0
        || q.g1_violations > 0
        || q.hausdorff_max_px > g.max + 2.0 * grid
        || q.hausdorff_p95_px > g.p95 + grid
        || q.fit_deviation_px > fit_budget
        || q.iou_mask < g.iou_min
}

/// A traced glyph of the corpus with what judging it needs.
struct Sample {
    case: String,
    level: Level,
    name: String,
    image: image::RgbImage,
    group: Vec<usize>,
    traced: TracedGlyph,
    stroke_units: f64,
    good: bool,
}

/// The defects injected into good outlines.
pub const DEFECTS: [&str; 5] = [
    "close_counter",
    "drop_piece",
    "spike_3px",
    "shift_1px",
    "bold_30",
];

fn shifted(paths: &[Path], dx: f64) -> Vec<Path> {
    transform_paths(
        paths,
        &Transform {
            dx,
            ..Transform::IDENTITY
        },
    )
}

/// `paths` with defect `kind`, `None` when it does not apply (no counter to
/// close, a single piece).
fn inject(kind: &str, paths: &[Path], scale: f64, stroke_units: f64) -> Option<Vec<Path>> {
    let px = scale;
    let depths = contour_depths(paths);
    match kind {
        "close_counter" => {
            let hole = (0..paths.len())
                .filter(|&i| depths[i] % 2 == 1)
                .max_by(|&a, &b| {
                    signed_area(&paths[a])
                        .abs()
                        .total_cmp(&signed_area(&paths[b]).abs())
                })?;
            Some(
                paths
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != hole)
                    .map(|(_, p)| p.clone())
                    .collect(),
            )
        }
        "drop_piece" => {
            let outer: Vec<usize> = (0..paths.len())
                .filter(|&i| depths[i].is_multiple_of(2))
                .collect();
            if outer.len() < 2 {
                return None;
            }
            let smallest = *outer.iter().min_by(|&&a, &&b| {
                signed_area(&paths[a])
                    .abs()
                    .total_cmp(&signed_area(&paths[b]).abs())
            })?;
            Some(
                paths
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != smallest)
                    .map(|(_, p)| p.clone())
                    .collect(),
            )
        }
        "spike_3px" => {
            let b = forma_geometry::bbox(paths)?;
            let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
            let mut out = paths.to_vec();
            let (mut best, mut far) = (None, 0.0);
            for (pi, p) in out.iter().enumerate() {
                for (ni, n) in p.nodes.iter().enumerate() {
                    let d = (n.x - cx).hypot(n.y - cy);
                    if n.kind != NodeKind::OffCurve && d > far {
                        far = d;
                        best = Some((pi, ni));
                    }
                }
            }
            let (pi, ni) = best?;
            let n = &mut out[pi].nodes[ni];
            let (dx, dy) = ((n.x - cx) / far, (n.y - cy) / far);
            n.x = (n.x + dx * 3.0 * px).round();
            n.y = (n.y + dy * 3.0 * px).round();
            Some(out)
        }
        "shift_1px" => Some(shifted(paths, (1.0 * px).round().max(1.0))),
        "bold_30" => {
            let skeletons: Vec<Skeleton> = paths
                .iter()
                .map(|p| Skeleton {
                    nodes: p
                        .nodes
                        .iter()
                        .map(|n| SkeletonNode {
                            x: n.x,
                            y: n.y,
                            kind: n.kind,
                            width: None,
                        })
                        .collect(),
                    closed: true,
                })
                .collect();
            let band = stroke(
                &skeletons,
                &StrokeStyle {
                    width: 0.3 * stroke_units,
                    cap: Cap::Round,
                },
            )
            .ok()?;
            let mut all = paths.to_vec();
            all.extend(band);
            for (k, p) in all.iter_mut().enumerate() {
                p.id = PathId(k as u32);
            }
            remove_overlap(&all).ok()
        }
        _ => None,
    }
}

/// The groups of each row of `case`, matched with its layout as mode C
/// does; rows that do not match are left out.
fn groups(case: &CorpusCase, image: &image::RgbImage) -> Vec<(Vec<usize>, char)> {
    let opts = TraceOptions::default();
    let Ok(analysis) = forma_trace::analyze(image, &opts) else {
        return Vec::new();
    };
    if analysis.rows.len() != case.layout.len() {
        return Vec::new();
    }
    let stroke = forma_trace::typical_stroke_px(image, &opts).unwrap_or(0.0);
    let expected: Vec<usize> = case.layout.iter().map(|r| r.chars().count()).collect();
    let mut out = Vec::new();
    for (text, matched) in case
        .layout
        .iter()
        .zip(forma_trace::match_rows(&analysis, &expected, stroke))
    {
        if let RowMatch::Exact { groups } | RowMatch::Grouped { groups } = matched {
            out.extend(groups.into_iter().zip(text.chars()));
        }
    }
    out
}

fn samples(case: &CorpusCase) -> Result<Vec<Sample>, BenchError> {
    let image = forma_trace::decode(&case.image)?;
    let opts = TraceOptions::default();
    let groups = groups(case, &image);
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let list: Vec<Vec<usize>> = groups.iter().map(|(g, _)| g.clone()).collect();
    let traced = forma_trace::trace_groups(&image, &opts, &list, CAP, UPM)?;
    let stroke_px = forma_trace::typical_stroke_px(&image, &opts).unwrap_or(4.0);
    let mut out = Vec::new();
    for ((group, ch), g) in groups.into_iter().zip(traced) {
        let name = ch.to_string();
        let good = case
            .truth
            .as_ref()
            .and_then(|t| t.glyphs.get(&name))
            .and_then(|truth| {
                let shapes = glyph_px(&g.paths, truth)?;
                let iou = shape_iou(std::slice::from_ref(&truth.path), &shapes).ok()?;
                let holes = count_holes(&shapes).ok()?;
                Some(iou >= GOOD_IOU && holes == truth.holes)
            })
            .unwrap_or(false);
        let stroke_units = stroke_px * g.placement.scale;
        out.push(Sample {
            case: case.id.clone(),
            level: case.level,
            name,
            image: image.clone(),
            group,
            traced: g,
            stroke_units,
            good,
        });
    }
    Ok(out)
}

fn rate(hits: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (1000.0 * hits as f64 / total as f64).round() / 1000.0
    }
}

/// Sweeps one gate, the others at their current values.
fn sweep(
    name: &str,
    values: &[f64],
    good: &[&QualityReport],
    organic: &[&QualityReport],
    defects: &[(String, QualityReport)],
    set: impl Fn(&mut Gates, f64),
) -> Value {
    let rows: Vec<Value> = values
        .iter()
        .map(|&v| {
            let mut g = Gates::current();
            set(&mut g, v);
            let fp = good.iter().filter(|q| flagged(q, g)).count();
            let organic_kept = organic.iter().filter(|q| !flagged(q, g)).count();
            let missed: BTreeMap<&str, usize> = defects
                .iter()
                .filter(|(_, q)| !flagged(q, g))
                .fold(BTreeMap::new(), |mut m, (k, _)| {
                    *m.entry(k.as_str()).or_default() += 1;
                    m
                });
            let missed_total: usize = missed.values().sum();
            json!({
                "value": v,
                "good_flagged": rate(fp, good.len()),
                "organic_kept": rate(organic_kept, organic.len()),
                "defects_missed": rate(missed_total, defects.len()),
                "missed_by_kind": missed,
            })
        })
        .collect();
    json!({ "gate": name, "rows": rows })
}

/// The fidelity calibration over `levels`.
///
/// # Errors
///
/// [`BenchError`] if a case cannot be built or traced.
pub fn fidelity(levels: &[Level]) -> Result<Value, BenchError> {
    let mut all = Vec::new();
    for &level in levels {
        for case in corpus(level)? {
            all.extend(samples(&case)?);
        }
    }
    let opts = TraceOptions::default();
    let mut defects: Vec<(String, QualityReport)> = Vec::new();
    let mut applied: BTreeMap<&str, usize> = BTreeMap::new();
    // The samples of a case share its image: judge them in one batch.
    let mut start = 0;
    while start < all.len() {
        let end = all[start..]
            .iter()
            .position(|s| s.case != all[start].case)
            .map_or(all.len(), |k| start + k);
        let mut injected: Vec<(&str, usize, Vec<Path>)> = Vec::new();
        for (i, s) in all[start..end].iter().enumerate().filter(|(_, s)| s.good) {
            for kind in DEFECTS {
                if let Some(paths) = inject(
                    kind,
                    &s.traced.paths,
                    s.traced.placement.scale,
                    s.stroke_units,
                ) {
                    injected.push((kind, start + i, paths));
                }
            }
        }
        if let Some(first) = all.get(start) {
            let items: Vec<(&[usize], &Placement, &[Path])> = injected
                .iter()
                .map(|(_, i, paths)| {
                    (
                        all[*i].group.as_slice(),
                        &all[*i].traced.placement,
                        paths.as_slice(),
                    )
                })
                .collect();
            let results = forma_trace::assess_batch(&first.image, &opts, CAP, &items)?;
            for ((kind, _, _), result) in injected.iter().zip(results) {
                // A defect that leaves no outline to judge is not counted.
                if let Ok(q) = result {
                    *applied.entry(kind).or_default() += 1;
                    defects.push(((*kind).to_owned(), q));
                }
            }
        }
        start = end;
    }
    let good: Vec<&QualityReport> = all
        .iter()
        .filter(|s| s.good)
        .map(|s| &s.traced.quality)
        .collect();
    let organic: Vec<&QualityReport> = all
        .iter()
        .filter(|s| s.good && s.level == Level::O)
        .map(|s| &s.traced.quality)
        .collect();
    let now = Gates::current();
    let missed_now: Vec<Value> = defects
        .iter()
        .filter(|(_, q)| !flagged(q, now))
        .map(|(k, q)| {
            json!({ "kind": k, "iou": q.iou_mask, "p95": q.hausdorff_p95_px,
            "max": q.hausdorff_max_px, "fit": q.fit_deviation_px })
        })
        .collect();
    let flagged_good: Vec<Value> = all
        .iter()
        .filter(|s| s.good && flagged(&s.traced.quality, now))
        .map(|s| json!({ "case": s.case, "glyph": s.name, "reasons": s.traced.reasons }))
        .collect();
    let pct = |qs: &[&QualityReport], f: &dyn Fn(&QualityReport) -> f64| {
        let mut v: Vec<f64> = qs.iter().map(|q| f(q)).collect();
        v.sort_by(f64::total_cmp);
        let at = |p: f64| {
            v.get(((v.len().max(1) - 1) as f64 * p).round() as usize)
                .copied()
        };
        json!({ "p50": at(0.5), "p90": at(0.9), "p99": at(0.99), "max": v.last() })
    };
    Ok(json!({
        "glyphs": all.len(),
        "good": good.len(),
        "organic_good": organic.len(),
        "defects_applied": applied,
        "good_distribution": {
            "iou": pct(&good, &|q| 1.0 - q.iou_mask),
            "p95_px": pct(&good, &|q| q.hausdorff_p95_px),
            "max_px": pct(&good, &|q| q.hausdorff_max_px),
            "fit_px": pct(&good, &|q| q.fit_deviation_px),
        },
        "current": {
            "good_flagged": rate(flagged_good.len(), good.len()),
            "defects_missed": rate(missed_now.len(), defects.len()),
            "missed": missed_now,
            "flagged_good": flagged_good,
        },
        "sweeps": [
            sweep("iou_min", &[0.95, 0.96, 0.97, 0.98, 0.985], &good, &organic, &defects,
                |g, v| g.iou_min = v),
            sweep("hausdorff_p95_px", &[0.5, 0.6, 0.75, 1.0, 1.25], &good, &organic, &defects,
                |g, v| g.p95 = v),
            sweep("hausdorff_max_px", &[1.5, 2.0, 2.5, 3.0], &good, &organic, &defects,
                |g, v| g.max = v),
            sweep("fit_faithful_px", &[0.35, 0.5, 0.75, 1.0], &good, &organic, &defects,
                |g, v| g.fit = v),
        ],
    }))
}

/// A font of the traced glyphs of `case`, with traced provenance.
fn traced_font(case: &CorpusCase) -> Result<Option<Font>, BenchError> {
    let list = samples(case)?;
    if list.len() < forma_synth::MIN_SOURCES {
        return Ok(None);
    }
    let mut font = Font::new(&case.id, UPM);
    let master = font.default_master().id;
    for s in list {
        let id = GlyphId::new();
        Arc::make_mut(&mut font.glyphs).insert(
            id,
            Arc::new(Glyph {
                id,
                name: s.name.clone(),
                unicodes: s.name.chars().map(u32::from).collect(),
                export: true,
                layers: BTreeMap::from([(
                    master,
                    Layer {
                        advance_width: s.traced.advance_width,
                        paths: s.traced.paths,
                        components: Vec::new(),
                    },
                )]),
                provenance: Some(Provenance::pending(
                    Source::Traced,
                    1,
                    ProvenanceDetail::None,
                )),
            }),
        );
        font.glyph_order.push(id);
    }
    Ok(Some(font))
}

/// The style score calibration over `levels`: scores of glyphs drawn with
/// the font's kit, and with kits wrong in one respect.
///
/// # Errors
///
/// [`BenchError`] if a case cannot be built or traced.
pub fn style(levels: &[Level]) -> Result<Value, BenchError> {
    let book = forma_synth::RecipeBook::parse(forma_synth::UNICASE_ES)
        .map_err(|e| BenchError::Invalid(e.to_string()))?;
    let mut by_kind: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut accepted: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut slipped: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut refused_good: BTreeMap<String, usize> = BTreeMap::new();
    let mut fonts = 0usize;
    let mut reproduction: BTreeMap<String, Value> = BTreeMap::new();
    for &level in levels {
        for case in corpus(level)? {
            let Some(font) = traced_font(&case)? else {
                continue;
            };
            let master = font.default_master().id;
            let Ok(kit) = forma_synth::StyleKit::extract(&font, master, None) else {
                continue;
            };
            fonts += 1;
            reproduction.insert(case.id.clone(), json!(kit.reproduction));
            let mut variants: Vec<(&str, forma_synth::StyleKit)> = vec![("good", kit.clone())];
            for (name, factor) in [("stroke_x0.7", 0.7), ("stroke_x1.3", 1.3)] {
                let mut k = kit.clone();
                k.stroke *= factor;
                k.thin *= factor;
                k.thick *= factor;
                variants.push((name, k));
            }
            let mut other_ends = kit.clone();
            if kit.flat_ends() {
                other_ends.terminal_fill = Some(0.79);
            } else if kit.ball.is_none() {
                other_ends.terminal_fill = Some(1.0);
            }
            variants.push(("other_terminals", other_ends));
            if kit.ball.is_none() {
                let mut balls = kit.clone();
                balls.ball = Some(1.6);
                variants.push(("spurious_balls", balls));
            }
            for (kind, dress_kit) in &variants {
                for recipe in book.glyphs.iter().filter(|r| r.needs_design.is_none()) {
                    let Some(strategy) = recipe.strategies.iter().find(|s| s.parts.is_empty())
                    else {
                        continue;
                    };
                    let Ok(Some((paths, s))) =
                        forma_synth::judge(&font, dress_kit, &kit, recipe, strategy)
                    else {
                        continue;
                    };
                    if paths.is_empty() {
                        continue;
                    }
                    by_kind.entry((*kind).to_owned()).or_default().push(s.total);
                    let e = accepted.entry((*kind).to_owned()).or_default();
                    e.1 += 1;
                    if s.accepts(forma_synth::MIN_STYLE_SCORE) {
                        e.0 += 1;
                        if *kind != "good" {
                            *slipped
                                .entry((*kind).to_owned())
                                .or_default()
                                .entry(recipe.name.clone())
                                .or_default() += 1;
                        }
                    } else if *kind == "good" {
                        let why = s.reasons.first().map_or_else(
                            || "components below the floor".to_owned(),
                            |r| r.split(' ').take(2).collect::<Vec<_>>().join(" "),
                        );
                        *refused_good
                            .entry(format!("{} ({why})", recipe.name))
                            .or_default() += 1;
                    }
                }
            }
        }
    }
    let thresholds = [0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9];
    let table: BTreeMap<String, Value> = by_kind
        .iter()
        .map(|(k, v)| {
            let rows: Vec<Value> = thresholds
                .iter()
                .map(|t| json!({ "threshold": t, "total_at_least": rate(v.iter().filter(|s| **s >= *t).count(), v.len()) }))
                .collect();
            let (a, n) = accepted.get(k).copied().unwrap_or((0, 0));
            (
                k.clone(),
                json!({ "glyphs": v.len(), "accepted_now": rate(a, n), "by_threshold": rows,
                    "accepted_names": slipped.get(k) }),
            )
        })
        .collect();
    Ok(
        json!({ "fonts": fonts, "kinds": table, "good_refused": refused_good,
        "reproduction": reproduction }),
    )
}
