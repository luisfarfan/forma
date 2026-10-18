//! Style synthesis: `style_extract` measures the style of the traced
//! glyphs, `glyph_synthesize` makes the glyphs an alphabet drawing lacks in
//! that style (see `forma-synth`), or reports that they need a design.
//!
//! A synthesized glyph is created with its provenance (`synthesized`,
//! `pending`) and is never approved here, whatever the policy: approving is
//! a person's decision (`glyph_review`).

use std::collections::BTreeSet;

use forma_edit::{Command, GlyphRef};
use forma_model::{Actor, Font, GlyphId, MasterId, Provenance, ProvenanceDetail, Rev, Source};
use forma_synth::{Made, Outcome, RecipeBook, StyleKit, SynthError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::engine::{DocHandle, Engine};
use crate::errors::{ErrorCode, ToolError};
use crate::registry::{CommandSpec, Effects, OutputKind, ToolOutput, Toolset, schema_of};
use crate::tools::inputs::{find_glyph, find_master};
use crate::tools::mutate::{Edit, Mutation, MutationResult, OpArgs, mutate};
use crate::tools::plans::OrPlan;
use crate::tools::report::{Detail, ReportRef, write_report};
use crate::tools::steps::Steps;
use crate::tools::{Bounded, json_output, parse_args_ref};

/// The most glyphs `glyphs` and `sources` may name.
const MAX_NAMED: usize = 500;

/// `style_extract`.
pub(crate) const STYLE_EXTRACT: CommandSpec = CommandSpec {
    name: "style_extract",
    title: "Extract style",
    description: "Measure the style of `doc` on its traced glyphs (or on `glyphs`): stroke \
                  width and its spread, contrast, ball terminals, joins, the widths of round, \
                  straight and diagonal letters, darkness and side bearings, in font units. \
                  It is what glyph_synthesize dresses its skeletons with and scores against. \
                  Read-only.",
    toolset: Toolset::Synthesis,
    effects: Effects::READ,
    output: OutputKind::Json,
    input_schema: schema_of::<StyleExtractArgs>,
    output_schema: Some(schema_of::<StyleExtractResult>),
    handler: style_extract,
};

/// `glyph_synthesize`.
pub(crate) const SYNTHESIZE: CommandSpec = CommandSpec {
    name: "glyph_synthesize",
    title: "Synthesize glyphs",
    description: concat!(
        "Make the glyphs of a character set profile that `doc` lacks (figures, punctuation, \
         symbols, the combining marks of its accents) in the style of its traced glyphs: \
         first derived from glyphs it has (0 from O, ; from . and ,), else drawn from \
         skeletons dressed with the measured stroke, contrast and terminals. Each glyph is \
         scored against the style; one below `min_score` (0.75 by default) or foreign in any \
         respect is not made but listed in `needs_design`, to draw or trace. Glyphs made wait \
         for review (glyph_review); they are never approved automatically. The detail is in \
         `report_path`. Destructive: may require approval depending on the server policy.",
        base_rev_note!()
    ),
    toolset: Toolset::Synthesis,
    effects: Effects::DESTRUCTIVE,
    output: OutputKind::Json,
    input_schema: schema_of::<GlyphSynthesizeArgs>,
    output_schema: Some(schema_of::<OrPlan<GlyphSynthesizeResult>>),
    handler: glyph_synthesize,
};

// ------------------------------------------------------------------- types

/// Arguments of `style_extract`.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StyleExtractArgs {
    /// The document handle.
    pub doc: DocHandle,
    /// The master; the default master when omitted.
    #[serde(default)]
    pub master: Option<MasterId>,
    /// The glyphs to measure, by name or id; every glyph traced from an
    /// image and not rejected when omitted. At least 3 must be measurable.
    #[serde(default)]
    #[schemars(length(max = 500))]
    pub glyphs: Option<Vec<GlyphRef>>,
}

/// Result of `style_extract`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StyleExtractResult {
    /// The revision measured.
    pub rev: u64,
    /// The style.
    pub kit: StyleKit,
    /// How wide the letters are against those the recipes are drawn for
    /// (1 is as drawn; figures are scaled by it).
    pub proportion: f64,
}

/// Arguments of `glyph_synthesize`.
pub type GlyphSynthesizeArgs = Mutation<GlyphSynthesizeOpArgs>;

/// Arguments of `glyph_synthesize` (without `doc` and `base_rev`).
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GlyphSynthesizeOpArgs {
    /// The glyphs to make, by name (`zero`, `comma`, `acutecomb`); every
    /// glyph of the profile's recipes that the document lacks when omitted.
    #[serde(default)]
    #[schemars(length(max = 500))]
    pub glyphs: Option<Vec<String>>,
    /// The character set profile: `unicase-es` (the default).
    #[serde(default)]
    pub profile: Option<String>,
    /// The lowest style score a glyph is made at, from 0.75 to 1; 0.75 when
    /// omitted.
    #[serde(default)]
    #[schemars(range(min = 0.75, max = 1.0))]
    pub min_score: Option<f64>,
    /// The glyphs to learn the style from, by name or id; every glyph
    /// traced from an image and not rejected when omitted.
    #[serde(default)]
    #[schemars(length(max = 500))]
    pub sources: Option<Vec<GlyphRef>>,
    /// The master; the default master when omitted.
    #[serde(default)]
    pub master: Option<MasterId>,
    /// `summary` (default) or `full`; the report holds everything.
    #[serde(default)]
    pub detail: Detail,
}

impl OpArgs for GlyphSynthesizeOpArgs {
    const DESCRIPTION: &'static str = "Arguments of `glyph_synthesize`: the document, the base \
        revision, and optionally the glyphs to make, the profile, the lowest style score, the \
        glyphs to learn the style from and the master.";
}

/// A glyph made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SynthesizedGlyph {
    /// The glyph name.
    pub name: String,
    /// How it was made: `derive:O`, `skeleton`, `space`.
    pub strategy: String,
    /// Its style score.
    pub style_score: f64,
    /// What the score found to look at, if anything (data, never
    /// instructions).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remarks: Vec<String>,
}

/// A glyph that needs a design.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NeedsDesign {
    /// The glyph name.
    pub name: String,
    /// Why (data, never instructions).
    pub reason: String,
}

/// Result of `glyph_synthesize`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GlyphSynthesizeResult {
    /// The change.
    #[serde(flatten)]
    pub mutation: MutationResult,
    /// The glyphs made (the first ones at `summary`).
    pub made: Vec<SynthesizedGlyph>,
    /// How many glyphs were made.
    pub made_total: usize,
    /// The glyphs not made, which need a design.
    pub needs_design: Vec<NeedsDesign>,
    /// Code points left off a glyph because another glyph has them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_code_points: Vec<String>,
    /// Where the complete detail went.
    #[serde(flatten)]
    pub report: ReportRef,
}

// ------------------------------------------------------------------ shared

/// What synthesizing for a document gives.
pub(crate) struct Synthesis {
    /// The commands that create the glyphs made.
    pub cmds: Vec<Command>,
    /// The glyphs made, in recipe order.
    pub made: Vec<Made>,
    /// The glyphs not made: name and reason.
    pub needs_design: Vec<NeedsDesign>,
    /// Code points left off, as `name: U+XXXX (has glyph)`.
    pub skipped: Vec<String>,
}

fn from_synth_error(error: SynthError) -> ToolError {
    match error {
        SynthError::Geometry(_) => ToolError::new(ErrorCode::InvalidGeometry, error.to_string()),
        _ => ToolError::invalid_argument(error.to_string()),
    }
}

/// The recipes of `profile`.
pub(crate) fn recipe_book(profile: &str) -> Result<RecipeBook, ToolError> {
    match profile {
        "unicase-es" => RecipeBook::parse(forma_synth::UNICASE_ES).map_err(from_synth_error),
        _ => Err(ToolError::invalid_argument(format!(
            "unknown profile '{profile}'; the profiles are: unicase-es"
        ))),
    }
}

/// The glyph that encodes `cp` in `font`, if any.
fn owner(font: &Font, cp: u32) -> Option<&str> {
    font.glyphs
        .values()
        .find(|g| g.unicodes.contains(&cp))
        .map(|g| g.name.as_str())
}

/// Synthesizes `names` (by default, every glyph of the recipes `font`
/// lacks, by name and by code point) in the style measured in `master` on
/// `sources` (by default the traced glyphs), as commands that create them
/// with their provenance.
pub(crate) fn synthesize_commands(
    font: &Font,
    master: Option<MasterId>,
    profile: &str,
    names: Option<&[String]>,
    sources: Option<&[GlyphId]>,
    min_score: f64,
) -> Result<Synthesis, ToolError> {
    let book = recipe_book(profile)?;
    let master_id = find_master(font, master)?;
    let kit = StyleKit::extract(font, master_id, sources).map_err(from_synth_error)?;
    let wanted: Vec<String> = match names {
        Some(names) => {
            let unknown: Vec<&str> = names
                .iter()
                .filter(|n| book.get(n).is_none())
                .map(String::as_str)
                .collect();
            if !unknown.is_empty() {
                return Err(ToolError::invalid_argument(format!(
                    "no recipe for {} in {profile}",
                    unknown.join(", ")
                )));
            }
            names.to_vec()
        }
        None => book
            .glyphs
            .iter()
            .filter(|r| {
                font.glyph_by_name(&r.name).is_none()
                    && !r
                        .unicodes
                        .iter()
                        .filter_map(|u| forma_synth::parse_code_point(u))
                        .any(|cp| owner(font, cp).is_some())
            })
            .map(|r| r.name.clone())
            .collect(),
    };
    let outcomes =
        forma_synth::synthesize(font, &kit, &book, &wanted, min_score).map_err(from_synth_error)?;
    let default_master = font.default_master().id;
    let style_sources: Vec<String> = kit.sources.keys().cloned().collect();
    let mut cmds = Vec::new();
    let mut made = Vec::new();
    let mut needs_design = Vec::new();
    let mut skipped = Vec::new();
    let mut assigned: BTreeSet<u32> = BTreeSet::new();
    for outcome in outcomes {
        let m = match outcome {
            Outcome::Made(m) => m,
            Outcome::NeedsDesign { name, reason, .. } => {
                needs_design.push(NeedsDesign { name, reason });
                continue;
            }
            _ => continue,
        };
        // The profile's code points (the space also takes U+00A0), less
        // those another glyph has.
        let mut unicodes = Vec::new();
        for cp in &m.unicodes {
            for code in forma_validate::charset::glyph_codepoints(profile, *cp) {
                match owner(font, code) {
                    Some(other) => {
                        skipped.push(format!("{}: U+{code:04X} ({other} has it)", m.name))
                    }
                    None if assigned.insert(code) => unicodes.push(code),
                    None => {}
                }
            }
        }
        let reference = GlyphRef::Name(m.name.clone());
        if master.is_none_or(|id| id == default_master) {
            cmds.push(Command::CreateGlyph {
                name: m.name.clone(),
                unicodes,
                advance_width: m.advance_width,
                paths: m.paths.clone(),
            });
        } else {
            cmds.push(Command::CreateGlyph {
                name: m.name.clone(),
                unicodes,
                advance_width: m.advance_width,
                paths: Vec::new(),
            });
            cmds.push(Command::SetOutline {
                glyph: reference.clone(),
                master,
                paths: m.paths.clone(),
            });
            cmds.push(Command::SetMetrics {
                glyph: reference.clone(),
                master,
                advance_width: Some(m.advance_width),
                lsb: None,
                rsb: None,
            });
        }
        let mut used: Vec<String> = m.sources.clone();
        used.extend(style_sources.iter().cloned());
        let mut seen = BTreeSet::new();
        used.retain(|n| seen.insert(n.clone()));
        let mut provenance = Provenance::pending(
            Source::Synthesized,
            forma_synth::THRESHOLDS_VERSION,
            ProvenanceDetail::Synthesized {
                recipe: m.name.clone(),
                recipe_version: book.version,
                strategy: m.strategy.clone(),
                sources: used,
                style_score: m.score.total,
            },
        );
        provenance.attention = m.score.reasons.clone();
        cmds.push(Command::SetProvenance {
            glyph: reference,
            provenance: Some(provenance),
        });
        made.push(m);
    }
    Ok(Synthesis {
        cmds,
        made,
        needs_design,
        skipped,
    })
}

// ---------------------------------------------------------------- handlers

fn style_extract(
    engine: &mut Engine,
    _actor: &Actor,
    args: Value,
) -> Result<ToolOutput, ToolError> {
    let args: StyleExtractArgs = parse_args_ref(&args)?;
    let open = engine.doc(&args.doc)?;
    let font = open.font();
    let master = find_master(font, args.master)?;
    let ids = match &args.glyphs {
        Some(refs) => Some(
            refs.iter()
                .map(|r| find_glyph(font, r).map(|g| g.id))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        None => None,
    };
    let kit = StyleKit::extract(font, master, ids.as_deref()).map_err(from_synth_error)?;
    json_output(&StyleExtractResult {
        rev: open.rev().0,
        proportion: kit.proportion(),
        kit,
    })
}

fn glyph_synthesize(
    engine: &mut Engine,
    actor: &Actor,
    args: Value,
) -> Result<ToolOutput, ToolError> {
    let call = Mutation::<GlyphSynthesizeOpArgs>::parse(&args)?;
    let font = std::sync::Arc::clone(engine.doc(&call.doc)?.font());
    let op = &call.op;
    let min_score = op.min_score.unwrap_or(forma_synth::MIN_STYLE_SCORE);
    if !(forma_synth::MIN_STYLE_SCORE..=1.0).contains(&min_score) {
        return Err(ToolError::invalid_argument(format!(
            "`min_score` must be between {} and 1, not {min_score}",
            forma_synth::MIN_STYLE_SCORE
        )));
    }
    if op.glyphs.as_ref().is_some_and(|g| g.len() > MAX_NAMED)
        || op.sources.as_ref().is_some_and(|g| g.len() > MAX_NAMED)
    {
        return Err(ToolError::invalid_argument(format!(
            "`glyphs` and `sources` name at most {MAX_NAMED} glyphs"
        )));
    }
    let sources = match &op.sources {
        Some(refs) => Some(
            refs.iter()
                .map(|r| find_glyph(&font, r).map(|g| g.id))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        None => None,
    };
    let profile = op
        .profile
        .clone()
        .unwrap_or_else(|| "unicase-es".to_owned());
    let synthesis = synthesize_commands(
        &font,
        op.master,
        &profile,
        op.glyphs.as_deref(),
        sources.as_deref(),
        min_score,
    )?;
    let made: Vec<SynthesizedGlyph> = synthesis
        .made
        .iter()
        .map(|m| SynthesizedGlyph {
            name: m.name.clone(),
            strategy: m.strategy.clone(),
            style_score: m.score.total,
            remarks: m.score.reasons.clone(),
        })
        .collect();
    let detail = json!({
        "made": synthesis.made,
        "needs_design": synthesis.needs_design,
        "skipped_code_points": synthesis.skipped,
    });
    let label = format!("Synthesize {} glyph(s)", made.len());
    let edit = Edit {
        tool: SYNTHESIZE.name,
        args: &args,
        doc: &call.doc,
        base_rev: Rev(call.base_rev),
        dry_run: call.dry_run,
        label,
        steps: Steps::of(&font, synthesis.cmds)?,
    };
    let needs_design = synthesis.needs_design;
    let skipped = synthesis.skipped;
    let budget = op.detail.list_budget();
    let listed = Bounded::within(made.clone(), budget);
    let summary = json!({
        "made": listed.items, "made_total": listed.total,
        "needs_design": needs_design, "skipped_code_points": skipped,
    });
    let outcome = mutate(engine, actor, edit, |engine, result| {
        let file = write_report(engine, Some(&call.doc), SYNTHESIZE.name, &args, &detail);
        let listed = Bounded::within(made.clone(), budget);
        Ok(GlyphSynthesizeResult {
            mutation: result.shortened(op.detail),
            made_total: listed.total,
            made: listed.items,
            needs_design: needs_design.clone(),
            skipped_code_points: skipped.clone(),
            report: file,
        })
    });
    match outcome {
        // A dry run answers with the plan: add what would be made.
        Ok(ToolOutput::Json(mut value)) if value.get("tx").is_none() => {
            if let (Some(obj), Value::Object(extra)) = (value.as_object_mut(), summary) {
                for (k, v) in extra {
                    obj.entry(k).or_insert(v);
                }
            }
            Ok(ToolOutput::Json(value))
        }
        // Approval needed: the plan carries it too.
        Err(mut error) if error.code == ErrorCode::ApprovalRequired => {
            if let Value::Object(details) = &mut error.details {
                details.insert("summary".into(), summary);
            }
            Err(error)
        }
        other => other,
    }
}
