//! `font_from_image`: from the image of an alphabet to a font in one
//! operation of the engine.
//!
//! The stages are the granular tools' own (tracing, composing accents,
//! spacing, kerning, checks), run in order on one scratch state ([`Steps`])
//! and committed as **one transaction**: one plan to approve under the
//! `prompted` policy, one `history_undo` to take it all back. The
//! granular tools stay available for corrections. The detail goes to a
//! report file; the answer is a compact summary of each stage and of what
//! needs a person's attention.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use forma_edit::{Command, GlyphRef, KernRefInput};
use forma_model::{Actor, Font, GlyphId, KernRef, MasterId, Rev};
use forma_trace::{Extractor, Fidelity, RowMatch};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::engine::Engine;
use crate::errors::{ErrorCode, ToolError, from_spacing_error, from_trace_error};
use crate::limits::MAX_BATCH_OPS;
use crate::registry::{CommandSpec, Effects, OutputKind, ToolOutput, Toolset, schema_of};
use crate::tools::components::{
    GlyphComposeOpArgs, MarkPlacement, MarkPosition, glyph_compose_commands,
};
use crate::tools::inputs::{CodePoint, find_master};
use crate::tools::kerning::{KerningAutoOpArgs, kerning_auto_commands, kerning_glyphs};
use crate::tools::metrics::{SpacingAutoOpArgs, plan_spacing, realign_composites};
use crate::tools::mutate::{Edit, Mutation, MutationResult, OpArgs, mutate};
use crate::tools::plans::OrPlan;
use crate::tools::report::{Detail, ReportRef, write_report};
use crate::tools::steps::Steps;
use crate::tools::synth::synthesize_commands;
use crate::tools::trace::{
    Assignment, cap_height, read_image_hashed, trace_commands, trace_options,
};

/// The most entries of `attention` in an answer.
const MAX_ATTENTION: usize = 50;

/// The accented letters `unicase-es` composes: the letter, its base and
/// its mark.
const ACCENTED: [(char, char, &str); 7] = [
    ('Á', 'A', "acutecomb"),
    ('É', 'E', "acutecomb"),
    ('Í', 'I', "acutecomb"),
    ('Ó', 'O', "acutecomb"),
    ('Ú', 'U', "acutecomb"),
    ('Ü', 'U', "dieresiscomb"),
    ('Ñ', 'N', "tildecomb"),
];

/// `font_from_image`.
pub(crate) const FROM_IMAGE: CommandSpec = CommandSpec {
    name: "font_from_image",
    title: "Font from image",
    description: concat!(
        "Turn the image of an alphabet (PNG/JPEG in an allowed folder) into glyphs of `doc` in \
         one operation: match the shapes to `layout` (one string per image row, top to bottom; \
         \"\" skips a row; `extra` names single shapes by position), group letters drawn in \
         pieces, trace with quality gates, compose the profile's accents, space, kern and \
         check. One transaction: one approval if the policy asks, one history_undo. Rows that \
         do not match are listed in `attention`, never guessed, as are glyphs needing a look. \
         `stages.export` exports afterwards unless glyphs wait for review or a file exists. \
         The answer is a summary; the detail is in `report_path`. Review with preview_sheet. \
         Destructive: may require approval depending on the server policy.",
        base_rev_note!()
    ),
    toolset: Toolset::Trace,
    effects: Effects::DESTRUCTIVE,
    output: OutputKind::Json,
    input_schema: schema_of::<FontFromImageArgs>,
    output_schema: Some(schema_of::<OrPlan<FontFromImageResult>>),
    handler: font_from_image,
};

/// A shape named by its position in a row.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtraPiece {
    /// The row, from 0 at the top.
    pub row: usize,
    /// Its position in the row from the left, from 0. Give this or
    /// `index_from_end`.
    #[serde(default)]
    pub index: Option<usize>,
    /// Its position from the right, 0 being the last.
    #[serde(default)]
    pub index_from_end: Option<usize>,
    /// The character it is (one character).
    pub char: String,
}

/// Where the font goes after the change.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportStage {
    /// A `.ttf` file inside an allowed folder.
    #[serde(default)]
    pub ttf: Option<String>,
    /// A `.woff2` file inside an allowed folder.
    #[serde(default)]
    pub woff2: Option<String>,
    /// Export even if glyphs wait for review; false by default.
    #[serde(default)]
    pub allow_pending: bool,
}

const fn yes() -> bool {
    true
}

/// Which stages run after tracing.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stages {
    /// Make the profile's glyphs the image lacks (figures, punctuation,
    /// marks) in its style, or list them as needing a design (see
    /// glyph_synthesize).
    #[serde(default = "yes")]
    pub synthesize: bool,
    /// Compose the accented letters of the profile from their base and
    /// mark, when both exist.
    #[serde(default = "yes")]
    pub compose: bool,
    /// Space the glyphs the call makes (the others keep their metrics).
    #[serde(default = "yes")]
    pub space: bool,
    /// Kern the glyphs the call makes among themselves (pairs of other
    /// glyphs stay).
    #[serde(default = "yes")]
    pub kern: bool,
    /// Export after the change.
    #[serde(default)]
    pub export: Option<ExportStage>,
}

impl Default for Stages {
    fn default() -> Self {
        Stages {
            synthesize: true,
            compose: true,
            space: true,
            kern: true,
            export: None,
        }
    }
}

/// Arguments of `font_from_image`.
pub type FontFromImageArgs = Mutation<FontFromImageOpArgs>;

/// Arguments of `font_from_image` (without `doc` and `base_rev`).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FontFromImageOpArgs {
    /// The image file (PNG or JPEG) inside an allowed folder.
    pub path: String,
    /// The characters of each row of the image, top to bottom, in reading
    /// order (for example `["ABCDEFGHI", "JKLMNOPQ", "RSTUVWXYZ"]`); an
    /// empty string skips a row. From 1 to 32 rows.
    #[schemars(length(min = 1, max = 32))]
    pub layout: Vec<String>,
    /// Shapes named by their position in a row, taken before the row is
    /// matched with its string.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub extra: Vec<ExtraPiece>,
    /// The character set profile: `unicase-es` (the default: a capital also
    /// encodes its lowercase).
    #[serde(default)]
    pub profile: Option<String>,
    /// How closely outlines follow the ink: `faithful` (default) or `clean`.
    #[serde(default)]
    pub fidelity: Option<Fidelity>,
    /// How the ink's edge is read (see image_trace).
    #[serde(default)]
    pub extractor: Option<Extractor>,
    /// The coverage level of the edge, 0.3 to 0.7 (see image_trace).
    #[serde(default)]
    #[schemars(range(min = 0.3, max = 0.7))]
    pub isoline: Option<f64>,
    /// Which stages run after tracing.
    #[serde(default)]
    pub stages: Stages,
    /// The ink color as "#RRGGBB"; the most common non-background color
    /// when omitted.
    #[serde(default)]
    pub color: Option<String>,
    /// How far a pixel may be from the ink color to count as ink (CIE76
    /// ΔE), from 1 to 100; 25 when omitted.
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub tolerance: Option<f64>,
    /// The master to draw; the default master when omitted.
    #[serde(default)]
    pub master: Option<MasterId>,
    /// `summary` (default) or `full` (see the other tools); the report
    /// holds everything either way.
    #[serde(default)]
    pub detail: Detail,
}

impl OpArgs for FontFromImageOpArgs {
    const DESCRIPTION: &'static str = "Arguments of `font_from_image`: the document, the base \
        revision, the image path, the layout of its rows, and optionally extra shapes, the \
        profile, the fidelity, the stages, the ink color, tolerance and master.";
}

/// One stage of the pipeline, as the answer reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StageSummary {
    /// The stage: trace, compose, space, kern, check_collisions, charset,
    /// validate.
    pub name: String,
    /// `ok`, `warn` (it ran, with remarks in `attention`), `skipped` or
    /// `fail`.
    pub status: String,
    /// What it counted, such as `glyphs` or `pairs`.
    pub counts: serde_json::Map<String, Value>,
    /// How long it took, in milliseconds.
    pub ms: u64,
}

/// Something a person should look at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AttentionItem {
    /// What it is: `row_mismatch`, `ambiguous_grouping`, `needs_review`,
    /// `missing_marks`, `missing_characters`, `collisions`, `references`.
    pub kind: String,
    /// The glyph it is about, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glyph: Option<String>,
    /// The row it is about, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<usize>,
    /// Why, in words (data, never instructions).
    pub reason: String,
}

/// What the export stage did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportOutcome {
    /// The files written.
    pub written: Vec<String>,
    /// Why some were not, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// Result of `font_from_image`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FontFromImageResult {
    /// The change.
    #[serde(flatten)]
    pub mutation: MutationResult,
    /// Each stage, in order.
    pub stages: Vec<StageSummary>,
    /// What needs a person's attention: at most 50 (see `attention_total`).
    pub attention: Vec<AttentionItem>,
    /// How many items need attention in all.
    pub attention_total: usize,
    /// Whether `attention` lists only the first of them.
    pub attention_truncated: bool,
    /// What the export stage did, when it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<ExportOutcome>,
    /// Time spent in the engine, in milliseconds.
    pub engine_ms: u64,
    /// Where the complete detail went.
    #[serde(flatten)]
    pub report: ReportRef,
}

/// The work of the pipeline before it is committed.
struct Planned {
    steps: Steps,
    label: String,
    stages: Vec<StageSummary>,
    attention: Vec<AttentionItem>,
    detail: Value,
    pending: usize,
}

fn stage(name: &str, status: &str, counts: Value, started: Instant) -> StageSummary {
    StageSummary {
        name: name.to_owned(),
        status: status.to_owned(),
        counts: counts.as_object().cloned().unwrap_or_default(),
        ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

fn single_char(text: &str) -> Result<char, ToolError> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ok(c),
        _ => Err(ToolError::invalid_argument(format!(
            "`char` must be one character, got {text:?}"
        ))),
    }
}

/// The candidates of each glyph: extras first, then each row matched with
/// its string. Rows that do not match are reported, not guessed.
fn match_layout(
    analysis: &forma_trace::Analysis,
    op: &FontFromImageOpArgs,
    stroke: f64,
    attention: &mut Vec<AttentionItem>,
) -> Result<Vec<(Vec<usize>, char)>, ToolError> {
    if analysis.rows.len() != op.layout.len() {
        let counts: Vec<usize> = (0..analysis.rows.len())
            .map(|r| analysis.candidates.iter().filter(|c| c.row == r).count())
            .collect();
        return Err(ToolError::invalid_argument(format!(
            "the image has {} row(s) of shapes and `layout` {}: give one string per row (an \
             empty string skips a row)",
            analysis.rows.len(),
            op.layout.len()
        ))
        .with_details(json!({ "image_rows": analysis.rows.len(), "shapes_per_row": counts })));
    }
    let mut out = Vec::new();
    let mut taken = BTreeSet::new();
    for extra in &op.extra {
        let ch = single_char(&extra.char)?;
        let row: Vec<usize> = analysis
            .candidates
            .iter()
            .filter(|c| c.row == extra.row)
            .map(|c| c.index)
            .collect();
        let at = match (extra.index, extra.index_from_end) {
            (Some(i), None) => row.get(i),
            (None, Some(i)) => i
                .checked_add(1)
                .and_then(|n| row.len().checked_sub(n))
                .and_then(|k| row.get(k)),
            _ => {
                return Err(ToolError::invalid_argument(
                    "an extra shape needs exactly one of `index` and `index_from_end`",
                ));
            }
        };
        let Some(&index) = at else {
            return Err(ToolError::invalid_argument(format!(
                "row {} has no shape at that position (it has {})",
                extra.row,
                row.len()
            )));
        };
        if !taken.insert(index) {
            return Err(ToolError::invalid_argument(format!(
                "two extra shapes name the same shape (row {}): name each shape once",
                extra.row
            )));
        }
        out.push((vec![index], ch));
    }
    let mut rest = analysis.clone();
    rest.candidates.retain(|c| !taken.contains(&c.index));
    let expected: Vec<usize> = op.layout.iter().map(|r| r.chars().count()).collect();
    for (r, (text, matched)) in op
        .layout
        .iter()
        .zip(forma_trace::match_rows(&rest, &expected, stroke))
        .enumerate()
    {
        let groups = match matched {
            RowMatch::Exact { groups } | RowMatch::Grouped { groups } => groups,
            RowMatch::Ambiguous {
                best,
                second,
                ratio,
            } => {
                attention.push(AttentionItem {
                    kind: "ambiguous_grouping".into(),
                    glyph: None,
                    row: Some(r),
                    reason: format!(
                        "two ways to group the shapes of this row are about as likely \
                         ({best:?} or {second:?}, cost ratio {ratio:.2}); trace it with \
                         image_trace and `candidates`"
                    ),
                });
                continue;
            }
            RowMatch::Mismatch {
                expected,
                found,
                reason,
                ..
            } => {
                if expected > 0 {
                    attention.push(AttentionItem {
                        kind: "row_mismatch".into(),
                        glyph: None,
                        row: Some(r),
                        reason: format!("{expected} character(s), {found} shape(s): {reason}"),
                    });
                }
                continue;
            }
            _ => continue,
        };
        out.extend(groups.into_iter().zip(text.chars()));
    }
    Ok(out)
}

/// Computes every stage on one scratch state.
fn plan(engine: &Engine, font: &Arc<Font>, op: &FontFromImageOpArgs) -> Result<Planned, ToolError> {
    let profile = op
        .profile
        .clone()
        .unwrap_or_else(|| "unicase-es".to_owned());
    let codes = forma_validate::charset::profile(&profile).ok_or_else(|| {
        ToolError::invalid_argument(format!(
            "unknown profile '{profile}'; the profiles are: unicase-es"
        ))
    })?;
    let mut opts = trace_options(op.color.as_deref(), op.tolerance)?;
    if let Some(f) = op.fidelity {
        opts.fidelity = f;
    }
    if let Some(e) = op.extractor {
        opts.extractor = e;
    }
    if let Some(level) = op.isoline {
        if !(0.3..=0.7).contains(&level) {
            return Err(ToolError::invalid_argument(format!(
                "`isoline` must be between 0.3 and 0.7, not {level}"
            )));
        }
        opts.isoline = level;
    }
    let master = find_master(font, op.master)?;
    let mut stages = Vec::new();
    let mut attention = Vec::new();

    // Trace.
    let started = Instant::now();
    let (image, sha) = read_image_hashed(engine, &op.path)?;
    let analysis = forma_trace::analyze(&image, &opts).map_err(from_trace_error)?;
    let stroke = forma_trace::typical_stroke_px(&image, &opts).map_err(from_trace_error)?;
    let matched = match_layout(&analysis, op, stroke, &mut attention)?;
    if matched.is_empty() {
        return Err(ToolError::invalid_argument(
            "no row of the image matches the layout: nothing to trace",
        )
        .with_details(json!({ "attention": attention })));
    }
    let mut names = BTreeSet::new();
    let mut assignments = Vec::with_capacity(matched.len());
    let primaries: BTreeSet<u32> = matched.iter().map(|(_, ch)| u32::from(*ch)).collect();
    let mut unicase_skipped = Vec::new();
    for (group, ch) in &matched {
        let cp = u32::from(*ch);
        let name = forma_validate::charset::glyph_name(cp);
        if !names.insert(name.clone()) {
            return Err(ToolError::invalid_argument(format!(
                "the character {ch:?} appears twice in the layout"
            )));
        }
        let (candidate, candidates) = if group.len() == 1 {
            (Some(group[0]), None)
        } else {
            (None, Some(group.clone()))
        };
        // The profile's extra code points (a capital's lowercase) are left
        // off when the layout draws that character too or another glyph of
        // the document has it.
        let mut unicodes = Vec::new();
        for code in forma_validate::charset::glyph_codepoints(&profile, cp) {
            let taken_elsewhere = font
                .glyphs
                .values()
                .any(|g| g.name != name && g.unicodes.contains(&code));
            if code != cp && (primaries.contains(&code) || taken_elsewhere) {
                unicase_skipped.push(format!("{name} without U+{code:04X}"));
                continue;
            }
            unicodes.push(CodePoint(code));
        }
        assignments.push(Assignment {
            candidate,
            candidates,
            name,
            unicodes,
        });
    }
    if !unicase_skipped.is_empty() {
        attention.push(AttentionItem {
            kind: "unicase_skipped".into(),
            glyph: None,
            row: None,
            reason: format!(
                "code points another glyph already has were left off: {}",
                unicase_skipped.join(", ")
            ),
        });
    }
    let groups: Vec<Vec<usize>> = matched.iter().map(|(g, _)| g.clone()).collect();
    let cap = cap_height(font, master)?;
    let traced = forma_trace::trace_groups(&image, &opts, &groups, cap, font.units_per_em)
        .map_err(from_trace_error)?;
    let mut steps = Steps::of(
        font,
        trace_commands(font, op.master, &assignments, &traced, &sha),
    )?;
    let mut traced_detail = Vec::new();
    for (a, g) in assignments.iter().zip(&traced) {
        if g.needs_review {
            attention.push(AttentionItem {
                kind: "needs_review".into(),
                glyph: Some(a.name.clone()),
                row: None,
                reason: brief(&g.reasons),
            });
        }
        traced_detail.push(json!({
            "name": a.name, "candidates": g.candidates, "iou": g.iou,
            "quality": g.quality, "attempt": g.attempt, "reasons": g.reasons,
        }));
    }
    let review = traced.iter().filter(|g| g.needs_review).count();
    stages.push(stage(
        "trace",
        if review > 0 { "warn" } else { "ok" },
        json!({ "glyphs": traced.len(), "needs_review": review, "shapes": analysis.candidates.len() }),
        started,
    ));

    // Glyphs traced again move under the composites built on them before
    // this call (an Á from an earlier run): those follow, as they follow
    // spacing.
    let retraced: Vec<GlyphId> = assignments
        .iter()
        .filter_map(|a| font.glyph_by_name(&a.name).map(|g| g.id))
        .collect();
    if !retraced.is_empty() {
        let (cmds, _) = realign_composites(font, &steps.after, master, &retraced)?;
        for cmd in cmds {
            steps.push(cmd)?;
        }
    }

    // A stage runs only if the toolset of its tool is enabled: what the
    // operator turned off on this server the pipeline does not do either.
    // (A tool profile only narrows the catalogue an agent sees; the stages
    // still run.)
    let mut disabled = Vec::new();
    let mut allowed = |stage: &str, tool: &str, wanted: bool| -> bool {
        if !wanted {
            return false;
        }
        let ok =
            crate::registry::find(tool).is_some_and(|spec| engine.is_toolset_enabled(spec.toolset));
        if !ok {
            disabled.push(format!("{stage} (the toolset of {tool} is disabled)"));
        }
        ok
    };
    let run_synthesize = allowed("synthesize", "glyph_synthesize", op.stages.synthesize);
    let run_compose = allowed("compose", "glyph_compose", op.stages.compose);
    let run_space = allowed("space", "spacing_auto", op.stages.space);
    let run_kern = allowed("kern", "kerning_auto", op.stages.kern);
    if !disabled.is_empty() {
        attention.push(AttentionItem {
            kind: "stage_disabled".into(),
            glyph: None,
            row: None,
            reason: format!("stages not run: {}", disabled.join(", ")),
        });
    }

    // The glyphs the call makes: only they are spaced and kerned.
    let mut produced: BTreeSet<String> = assignments.iter().map(|a| a.name.clone()).collect();
    let mut composed_names: BTreeSet<String> = BTreeSet::new();
    let mut unspaced: BTreeSet<String> = BTreeSet::new();
    let mut synthesized: BTreeSet<String> = BTreeSet::new();
    let default_master = op.master.is_none_or(|m| m == font.default_master().id);

    // Synthesize what the image lacks.
    let started = Instant::now();
    let mut synth_detail = Value::Null;
    if run_synthesize {
        match synthesize_commands(
            &steps.after,
            op.master,
            &profile,
            None,
            None,
            forma_synth::MIN_STYLE_SCORE,
        ) {
            Ok(synthesis) => {
                let made = synthesis.made.len();
                let mut by_reason: std::collections::BTreeMap<String, Vec<String>> =
                    std::collections::BTreeMap::new();
                for m in &synthesis.made {
                    produced.insert(m.name.clone());
                    synthesized.insert(m.name.clone());
                    if matches!(
                        m.class,
                        forma_synth::Class::Mark | forma_synth::Class::Space
                    ) {
                        unspaced.insert(m.name.clone());
                    }
                    if !m.score.reasons.is_empty() {
                        by_reason
                            .entry(brief(&m.score.reasons))
                            .or_default()
                            .push(m.name.clone());
                    }
                }
                // One item per reason, naming its glyphs: the same remark on
                // fifty glyphs is one thing to look at, not fifty.
                for (reason, names) in by_reason {
                    let (glyph, reason) = match names.as_slice() {
                        [one] => (Some(one.clone()), reason),
                        _ => (None, format!("{}: {reason}", names.join(", "))),
                    };
                    attention.push(AttentionItem {
                        kind: "needs_review".into(),
                        glyph,
                        row: None,
                        reason,
                    });
                }
                if !synthesis.needs_design.is_empty() {
                    attention.push(AttentionItem {
                        kind: "needs_design".into(),
                        glyph: None,
                        row: None,
                        reason: format!(
                            "no glyph in the drawing's style could be made for {}: draw or \
                             trace them (glyph_synthesize lists why)",
                            synthesis
                                .needs_design
                                .iter()
                                .map(|n| n.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    });
                }
                if !synthesis.skipped.is_empty() {
                    attention.push(AttentionItem {
                        kind: "unicase_skipped".into(),
                        glyph: None,
                        row: None,
                        reason: format!(
                            "code points another glyph already has were left off: {}",
                            synthesis.skipped.join(", ")
                        ),
                    });
                }
                let needs_design = synthesis.needs_design.len();
                synth_detail = json!({
                    "made": synthesis.made, "needs_design": synthesis.needs_design,
                });
                for cmd in synthesis.cmds {
                    steps.push(cmd)?;
                }
                stages.push(stage(
                    "synthesize",
                    if needs_design == 0 { "ok" } else { "warn" },
                    json!({ "made": made, "needs_design": needs_design }),
                    started,
                ));
            }
            Err(error) => {
                // Too few traced glyphs to learn the style from is expected
                // (a partial image): the stage is skipped, not failed.
                let sparse = error.code == ErrorCode::InvalidArgument
                    && error.message.contains("traced glyphs");
                attention.push(AttentionItem {
                    kind: if sparse {
                        "synthesis_skipped"
                    } else {
                        "stage_failed"
                    }
                    .into(),
                    glyph: None,
                    row: None,
                    reason: format!("synthesis: {}", error.message),
                });
                stages.push(stage(
                    "synthesize",
                    if sparse { "skipped" } else { "fail" },
                    json!({}),
                    started,
                ));
            }
        }
    } else {
        stages.push(stage("synthesize", "skipped", json!({}), started));
    }

    // Compose the accented letters of the profile.
    let started = Instant::now();
    if run_compose && profile == "unicase-es" && !default_master {
        attention.push(AttentionItem {
            kind: "compose_skipped".into(),
            glyph: None,
            row: None,
            reason: "accented letters are composed in the default master only: compose them \
                     there with glyph_compose"
                .into(),
        });
        stages.push(stage("compose", "skipped", json!({}), started));
    } else if run_compose && profile == "unicase-es" {
        let mut composed = 0;
        let mut missing_marks = BTreeSet::new();
        for (accented, base, mark) in ACCENTED {
            let name = forma_validate::charset::glyph_name(u32::from(accented));
            let base_name = base.to_string();
            if steps.after.glyph_by_name(&name).is_some()
                || steps.after.glyph_by_name(&base_name).is_none()
            {
                continue;
            }
            if steps.after.glyph_by_name(mark).is_none() {
                missing_marks.insert(mark);
                continue;
            }
            let unicodes: Vec<CodePoint> =
                forma_validate::charset::glyph_codepoints(&profile, u32::from(accented))
                    .into_iter()
                    .filter(|code| {
                        !steps
                            .after
                            .glyphs
                            .values()
                            .any(|g| g.unicodes.contains(code))
                    })
                    .map(CodePoint)
                    .collect();
            let (_, cmds) = glyph_compose_commands(
                &steps.after,
                GlyphComposeOpArgs {
                    name: name.clone(),
                    unicodes,
                    base: GlyphRef::Name(base_name),
                    marks: vec![MarkPlacement {
                        glyph: GlyphRef::Name(mark.to_owned()),
                        position: MarkPosition::Above,
                        gap: None,
                    }],
                },
            )?;
            for cmd in cmds {
                steps.push(cmd)?;
            }
            produced.insert(name.clone());
            composed_names.insert(name);
            composed += 1;
        }
        if !missing_marks.is_empty() {
            attention.push(AttentionItem {
                kind: "missing_marks".into(),
                glyph: None,
                row: None,
                reason: format!(
                    "the accented letters need the marks {}: synthesize or draw them, then \
                     compose with glyph_compose",
                    missing_marks.into_iter().collect::<Vec<_>>().join(", ")
                ),
            });
        }
        stages.push(stage(
            "compose",
            "ok",
            json!({ "composed": composed }),
            started,
        ));
    } else {
        stages.push(stage("compose", "skipped", json!({}), started));
    }

    // Space the glyphs the call made (composites follow their bases).
    let started = Instant::now();
    let to_space: Vec<GlyphRef> = produced
        .iter()
        .filter(|n| !composed_names.contains(*n) && !unspaced.contains(*n))
        .map(|n| GlyphRef::Name(n.clone()))
        .collect();
    if run_space && !to_space.is_empty() {
        let refs_exist = ["H", "O"]
            .iter()
            .all(|n| steps.after.glyph_by_name(n).is_some());
        if refs_exist {
            let before = steps.after.clone();
            let spaced = plan_spacing(
                &steps.after,
                SpacingAutoOpArgs {
                    master: op.master,
                    glyphs: Some(to_space),
                    tightness: None,
                    references: None,
                    detail: Detail::Summary,
                },
            )
            .and_then(|planned| {
                let scratch = Steps::of(&Arc::new(steps.after.clone()), planned.translation.1)?;
                let (realign, realigned) =
                    realign_composites(&before, &scratch.after, planned.master, &planned.spaced)?;
                let mut cmds = scratch.cmds;
                cmds.extend(realign);
                Ok((cmds, planned.spaced.len(), realigned, planned.clamped.len()))
            });
            match spaced {
                Ok((cmds, count, realigned, clamped)) => {
                    for cmd in cmds {
                        steps.push(cmd)?;
                    }
                    stages.push(stage(
                        "space",
                        if clamped == 0 { "ok" } else { "warn" },
                        json!({ "spaced": count, "realigned": realigned, "clamped": clamped }),
                        started,
                    ));
                }
                Err(error) => {
                    attention.push(AttentionItem {
                        kind: "stage_failed".into(),
                        glyph: None,
                        row: None,
                        reason: format!("spacing: {}", error.message),
                    });
                    stages.push(stage("space", "fail", json!({}), started));
                }
            }
        } else {
            attention.push(AttentionItem {
                kind: "references".into(),
                glyph: None,
                row: None,
                reason: "spacing needs the glyphs H and O as references; space with \
                         spacing_auto and `references`"
                    .into(),
            });
            stages.push(stage("space", "skipped", json!({}), started));
        }
    } else {
        stages.push(stage("space", "skipped", json!({}), started));
    }

    // Kern the letters the call traced or composed among themselves; the
    // synthesized glyphs wait for their review (then kerning_auto).
    let started = Instant::now();
    let to_kern: Vec<GlyphRef> = produced
        .iter()
        .filter(|n| !unspaced.contains(*n) && !synthesized.contains(*n))
        .map(|n| GlyphRef::Name(n.clone()))
        .collect();
    if run_kern && steps.after.glyph_by_name("H").is_some() && to_kern.len() > 1 {
        // Only the pairs between two glyphs of this call are replaced: the
        // user's pairs, and every pair with a glyph the call did not make,
        // stay as they are.
        let mine: BTreeSet<GlyphId> = to_kern
            .iter()
            .filter_map(|r| r.resolve(&steps.after))
            .collect();
        let stale: Vec<Command> = steps
            .after
            .kerning
            .pairs
            .get(&master)
            .into_iter()
            .flat_map(|pairs| pairs.keys())
            .filter_map(|(first, second)| match (first, second) {
                (KernRef::Glyph(a), KernRef::Glyph(b)) if mine.contains(a) && mine.contains(b) => {
                    Some(Command::SetKerningPair {
                        master: Some(master),
                        first: KernRefInput::Glyph(GlyphRef::Id(*a)),
                        second: KernRefInput::Glyph(GlyphRef::Id(*b)),
                        value: 0.0,
                    })
                }
                _ => None,
            })
            .collect();
        for cmd in stale {
            steps.push(cmd)?;
        }
        match kerning_auto_commands(
            &steps.after,
            KerningAutoOpArgs {
                master: op.master,
                glyphs: Some(to_kern),
                strength: None,
                min_value: None,
                max_ratio: None,
                min_distance: None,
                grouping: Some(false),
                replace: false,
                detail: Detail::Summary,
            },
        ) {
            Ok((_, cmds)) if steps.cmds.len() + cmds.len() > MAX_BATCH_OPS => {
                attention.push(AttentionItem {
                    kind: "stage_failed".into(),
                    glyph: None,
                    row: None,
                    reason: format!(
                        "kerning takes {} commands, too many for the same transaction: kern \
                         with kerning_auto afterwards",
                        cmds.len()
                    ),
                });
                stages.push(stage(
                    "kern",
                    "skipped",
                    json!({ "commands": cmds.len() }),
                    started,
                ));
            }
            Ok((_, cmds)) => {
                let count = cmds.len();
                for cmd in cmds {
                    steps.push(cmd)?;
                }
                stages.push(stage("kern", "ok", json!({ "commands": count }), started));
            }
            Err(error) => {
                attention.push(AttentionItem {
                    kind: "stage_failed".into(),
                    glyph: None,
                    row: None,
                    reason: format!("kerning: {}", error.message),
                });
                stages.push(stage("kern", "fail", json!({}), started));
            }
        }
    } else {
        stages.push(stage("kern", "skipped", json!({}), started));
    }

    // Checks: collisions, coverage, the font.
    let started = Instant::now();
    let after = &steps.after;
    let ids = kerning_glyphs(after, None, master)?;
    let pairs: Vec<(GlyphId, GlyphId)> = ids
        .iter()
        .flat_map(|l| ids.iter().map(move |r| (*l, *r)))
        .collect();
    let min_distance = (f64::from(after.units_per_em) * 0.02).round();
    let checked = pairs.len() <= 250_000;
    let collisions = if checked {
        forma_spacing::collisions_among(after, master, &ids, &pairs, min_distance)
            .map_err(from_spacing_error)?
            .len()
    } else {
        0
    };
    if collisions > 0 {
        attention.push(AttentionItem {
            kind: "collisions".into(),
            glyph: None,
            row: None,
            reason: format!(
                "{collisions} pair(s) come closer than {min_distance} units; see kerning_check"
            ),
        });
    }
    stages.push(stage(
        "check_collisions",
        match (checked, collisions) {
            (false, _) => "skipped",
            (true, 0) => "ok",
            _ => "warn",
        },
        json!({ "pairs": pairs.len(), "collisions": collisions }),
        started,
    ));
    let started = Instant::now();
    let charset = forma_validate::charset::check_charset(after, codes);
    if !charset.missing.is_empty() {
        let labels: Vec<String> = charset
            .missing
            .iter()
            .take(40)
            .map(|cp| char::from_u32(*cp).map_or_else(|| format!("U+{cp:04X}"), |c| c.to_string()))
            .collect();
        attention.push(AttentionItem {
            kind: "missing_characters".into(),
            glyph: None,
            row: None,
            reason: format!(
                "{} of {} characters of {profile} have no glyph yet: {}",
                charset.missing.len(),
                charset.required,
                labels.join(" ")
            ),
        });
    }
    stages.push(stage(
        "charset",
        if charset.missing.is_empty() {
            "ok"
        } else {
            "warn"
        },
        json!({ "required": charset.required, "covered": charset.covered }),
        started,
    ));
    let started = Instant::now();
    let validated = forma_validate::validate(after);
    // The export waits while the engine asks for a look at a glyph or a
    // synthesized glyph waits for review.
    let pending = after
        .glyphs
        .values()
        .filter(|g| {
            g.provenance.as_ref().is_some_and(|p| {
                p.needs_look()
                    || (p.source == forma_model::Source::Synthesized
                        && p.review == forma_model::Review::Pending)
            })
        })
        .count();
    stages.push(stage(
        "validate",
        if validated.errors > 0 { "fail" } else { "ok" },
        json!({ "errors": validated.errors, "warnings": validated.warnings }),
        started,
    ));
    if steps.cmds.len() > MAX_BATCH_OPS {
        return Err(ToolError::new(
            ErrorCode::LimitExceeded,
            format!(
                "the change takes {} commands; the limit of one transaction is {MAX_BATCH_OPS}: \
                 run the stages in several calls (see `stages`)",
                steps.cmds.len()
            ),
        )
        .with_details(json!({ "limit": MAX_BATCH_OPS, "actual": steps.cmds.len() })));
    }
    stable_refs(font, &mut steps)?;
    let detail = json!({
        "synthesized": synth_detail,
        "traced": traced_detail,
        "attention": attention,
        "validate": validated,
        "stroke_px": stroke,
    });
    Ok(Planned {
        label: format!("Font from image ({} glyphs)", traced.len()),
        steps,
        stages,
        attention,
        detail,
        pending,
    })
}

/// Refers to the glyphs the transaction creates by their names in its
/// commands: their ids on the scratch state are drawn anew on every run,
/// and a plan must describe the same commands when it is committed (the
/// call runs again then). Names are unique, so the commands do the same.
fn stable_refs(before: &Font, steps: &mut Steps) -> Result<(), ToolError> {
    let created: std::collections::HashMap<String, String> = steps
        .after
        .glyphs
        .values()
        .filter(|g| !before.glyphs.contains_key(&g.id))
        .map(|g| (g.id.to_string(), g.name.clone()))
        .collect();
    if created.is_empty() {
        return Ok(());
    }
    fn walk(value: &mut Value, created: &std::collections::HashMap<String, String>) {
        match value {
            Value::String(text) => {
                if let Some(name) = created.get(text.as_str()) {
                    *text = name.clone();
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| walk(v, created)),
            Value::Object(map) => map.values_mut().for_each(|v| walk(v, created)),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(&steps.cmds).map_err(|e| {
        ToolError::invalid_argument(format!("internal error: cannot encode the commands: {e}"))
    })?;
    walk(&mut value, &created);
    steps.cmds = serde_json::from_value(value).map_err(|e| {
        ToolError::invalid_argument(format!("internal error: cannot decode the commands: {e}"))
    })?;
    Ok(())
}

/// The first two of `reasons`, and how many more there are (the report
/// holds them all).
fn brief(reasons: &[String]) -> String {
    let mut text = reasons
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if reasons.len() > 2 {
        text.push_str(&format!("; +{} more in the report", reasons.len() - 2));
    }
    text
}

fn bounded(mut attention: Vec<AttentionItem>) -> (Vec<AttentionItem>, usize, bool) {
    let total = attention.len();
    attention.truncate(MAX_ATTENTION);
    (attention, total, total > MAX_ATTENTION)
}

/// Exports after the change: each format whose file does not exist yet.
fn export_after(
    engine: &mut Engine,
    actor: &Actor,
    doc: &crate::engine::DocHandle,
    export: &ExportStage,
    pending: usize,
) -> ExportOutcome {
    let mut out = ExportOutcome::default();
    if pending > 0 && !export.allow_pending {
        out.skipped = Some(format!(
            "{pending} glyph(s) need a look or, synthesized, wait for review: review them with \
             glyph_review (or pass `allow_pending`), then export with export_font"
        ));
        return out;
    }
    let mut skipped = Vec::new();
    for (format, path) in [("ttf", &export.ttf), ("woff2", &export.woff2)] {
        let Some(path) = path else { continue };
        let resolved = engine.check_allowed(std::path::Path::new(path), false);
        if resolved.as_ref().is_ok_and(|p| p.exists()) {
            skipped.push(format!("{path} exists; use export_font to replace it"));
            continue;
        }
        let rev = engine.doc(doc).map(|d| d.rev().0).unwrap_or(0);
        // Through the engine's own entry: the toolsets, the profile,
        // read-only mode and the audit log apply as to a direct call.
        match engine.call(
            actor,
            "export_font",
            json!({ "doc": doc, "base_rev": rev, "format": format, "path": path }),
        ) {
            Ok(_) => out.written.push(path.clone()),
            Err(error) => skipped.push(format!("{path}: {}", error.message)),
        }
    }
    if !skipped.is_empty() {
        out.skipped = Some(skipped.join("; "));
    }
    out
}

fn font_from_image(
    engine: &mut Engine,
    actor: &Actor,
    args: Value,
) -> Result<ToolOutput, ToolError> {
    let started = Instant::now();
    let call = Mutation::<FontFromImageOpArgs>::parse(&args)?;
    let font = Arc::clone(engine.doc(&call.doc)?.font());
    let planned = plan(engine, &font, &call.op)?;
    let Planned {
        steps,
        label,
        stages,
        attention,
        detail,
        pending,
    } = planned;
    let summary = |engine_ms: u64| {
        let (items, total, truncated) = bounded(attention.clone());
        json!({
            "stages": stages, "attention": items, "attention_total": total,
            "attention_truncated": truncated, "engine_ms": engine_ms,
        })
    };
    let edit = Edit {
        tool: FROM_IMAGE.name,
        args: &args,
        doc: &call.doc,
        base_rev: Rev(call.base_rev),
        dry_run: call.dry_run,
        label,
        steps,
    };
    let engine_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let outcome = mutate(engine, actor, edit, |engine, result| {
        let file = write_report(engine, Some(&call.doc), FROM_IMAGE.name, &args, &detail);
        let (items, total, truncated) = bounded(attention.clone());
        Ok(FontFromImageResult {
            mutation: result.shortened(call.op.detail),
            stages: stages.clone(),
            attention: items,
            attention_total: total,
            attention_truncated: truncated,
            export: None,
            engine_ms: engine_ms(),
            report: file,
        })
    });
    match outcome {
        // Committed: export if asked.
        Ok(ToolOutput::Json(mut value)) if value.get("tx").is_some_and(|tx| !tx.is_null()) => {
            if let Some(export) = &call.op.stages.export {
                let done = export_after(engine, actor, &call.doc, export, pending);
                value["export"] = serde_json::to_value(done).unwrap_or(Value::Null);
                value["rev"] = json!(engine.doc(&call.doc)?.rev().0);
            }
            value["engine_ms"] = json!(engine_ms());
            Ok(ToolOutput::Json(value))
        }
        // A dry run answers with the plan: add the summary of the stages.
        Ok(ToolOutput::Json(mut value)) => {
            if let (Some(obj), Value::Object(extra)) = (value.as_object_mut(), summary(engine_ms()))
            {
                for (k, v) in extra {
                    obj.entry(k).or_insert(v);
                }
            }
            Ok(ToolOutput::Json(value))
        }
        // Approval needed: the plan carries the summary too.
        Err(mut error) if error.code == ErrorCode::ApprovalRequired => {
            if let Value::Object(details) = &mut error.details {
                details.insert("summary".into(), summary(engine_ms()));
            }
            Err(error)
        }
        other => other,
    }
}
