//! Style synthesis: the glyphs an alphabet drawing lacks (figures,
//! punctuation, symbols, combining marks), made in the drawing's own style,
//! or refused.
//!
//! The style is measured on the traced glyphs ([`StyleKit`]); each
//! character is made by a recipe ([`RecipeBook`], data in `recipes/`) that
//! first tries to **derive** it from glyphs the font has and then to dress
//! a **skeleton** with the measured stroke, contrast and terminals. Every
//! glyph made is scored against the style ([`StyleScore`]); one that does
//! not reach the threshold, or has a component below the floor, is not
//! made: it is reported as needing a design. A glyph that is made is never
//! approved here; it waits for a person's review.

#![forbid(unsafe_code)]

mod dress;
mod error;
mod kit;
mod recipe;
mod score;
mod selftest;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use forma_geometry::{bbox, transform_paths};
use forma_model::{Font, Glyph, GlyphId, Layer, Path, Transform};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use error::SynthError;
pub use kit::{COLOR_MIN_WIDTH, CONTRAST_RATIO, FLAT_FILL, MIN_SOURCES, StyleKit, Widths};
pub use recipe::{
    Class, Dot, Part, Recipe, RecipeBook, Seg, Strategy, StrokeSpec, UNICASE_ES, parse_code_point,
};
pub use score::{COMPONENT_FLOOR, MIN_STYLE_SCORE, StyleScore, THRESHOLDS_VERSION, score};

/// Below this overlap between the font's letters and the same letters drawn
/// from skeletons in its style, synthesized glyphs ask for a look
/// (experimental, calibrated in `docs/qa/hito2-5/calibracion.md`).
pub const SELF_TEST_MIN: f64 = 0.9;

/// The gap between the cap height and a combining mark, in cap heights
/// (the composing tools place marks again over each base).
const MARK_GAP: f64 = 0.1;

/// A glyph synthesis made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Made {
    /// The glyph name.
    pub name: String,
    /// The code points of its recipe.
    pub unicodes: Vec<u32>,
    /// Its kind.
    pub class: Class,
    /// The strategy that made it (`derive:O`, `skeleton`).
    pub strategy: String,
    /// Its contours, in font units, with the font's median side bearings
    /// (a mark is centred on x = 0, above the cap height).
    #[serde(skip)]
    pub paths: Vec<Path>,
    /// Its advance width (0 for a mark).
    pub advance_width: f64,
    /// The names of the glyphs it reused (the kit's sources stand behind
    /// every glyph made).
    pub sources: Vec<String>,
    /// How consistent it is with the font's style.
    pub score: StyleScore,
}

/// What synthesis did with one character.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Outcome {
    /// The glyph was made.
    Made(Made),
    /// No strategy made a glyph consistent with the style: it needs a
    /// design (drawn, or traced from a drawing of it).
    NeedsDesign {
        /// The glyph name.
        name: String,
        /// The code points of its recipe.
        unicodes: Vec<u32>,
        /// Why, in words.
        reason: String,
        /// The best score reached, when a strategy ran.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        score: Option<StyleScore>,
    },
}

impl Outcome {
    /// The glyph name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Made(m) => &m.name,
            Self::NeedsDesign { name, .. } => name,
        }
    }
}

fn shift(paths: &[Path], dx: f64, dy: f64) -> Vec<Path> {
    transform_paths(
        paths,
        &Transform {
            dx,
            dy,
            ..Transform::IDENTITY
        },
    )
}

/// The code points of a recipe.
fn code_points(recipe: &Recipe) -> Vec<u32> {
    recipe
        .unicodes
        .iter()
        .filter_map(|u| parse_code_point(u))
        .collect()
}

/// Places a built glyph: a mark centred on x = 0 above the cap height,
/// anything else after the font's median left side bearing. Returns its
/// contours and advance width.
fn place(kit: &StyleKit, recipe: &Recipe, paths: &[Path]) -> (Vec<Path>, f64) {
    let Some(b) = bbox(paths) else {
        return (paths.to_vec(), 0.0);
    };
    if recipe.class == Class::Mark {
        let dx = (-(b.x0 + b.x1) / 2.0).round();
        let dy = ((1.0 + MARK_GAP) * kit.cap_height - b.y0).round();
        return (shift(paths, dx, dy), 0.0);
    }
    let lsb = kit.lsb.max(0.0).round();
    let rsb = kit.rsb.max(0.0).round();
    let placed = shift(paths, lsb - b.x0.round(), 0.0);
    (placed, (b.width() + lsb + rsb).round())
}

/// Synthesizes the glyphs `names` (those with a recipe in `book` and not in
/// `font` yet) in the style `kit` measured, in the order of the book: a
/// glyph made can be a piece of a later one (the `;` of the `.` and the
/// `,`). A glyph is made only if its score accepts it at `min_score` (see
/// [`StyleScore::accepts`]); otherwise it needs a design.
///
/// # Errors
///
/// [`SynthError::Geometry`] if dressing a skeleton or uniting its strokes
/// fails.
pub fn synthesize(
    font: &Font,
    kit: &StyleKit,
    book: &RecipeBook,
    names: &[String],
    min_score: f64,
) -> Result<Vec<Outcome>, SynthError> {
    let wanted: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let mut work = font.clone();
    let mut out = Vec::new();
    for recipe in &book.glyphs {
        if !wanted.contains(recipe.name.as_str()) || work.glyph_by_name(&recipe.name).is_some() {
            continue;
        }
        let unicodes = code_points(recipe);
        if let Some(reason) = &recipe.needs_design {
            out.push(Outcome::NeedsDesign {
                name: recipe.name.clone(),
                unicodes,
                reason: reason.clone(),
                score: None,
            });
            continue;
        }
        if recipe.class == Class::Space {
            let advance = (recipe.width * kit.cap_height * kit.proportion()).round();
            let made = Made {
                name: recipe.name.clone(),
                unicodes,
                class: recipe.class,
                strategy: "space".into(),
                paths: Vec::new(),
                advance_width: advance,
                sources: Vec::new(),
                score: StyleScore {
                    total: 1.0,
                    stroke: None,
                    terminal: None,
                    color: None,
                    proportion: None,
                    structure: None,
                    reasons: Vec::new(),
                },
            };
            add(&mut work, kit, &made);
            out.push(Outcome::Made(made));
            continue;
        }
        let mut best: Option<StyleScore> = None;
        let mut missing: Vec<String> = Vec::new();
        let mut made = None;
        for strategy in &recipe.strategies {
            let Some(built) = dress::build(&work, kit, recipe, strategy)? else {
                missing.extend(
                    strategy
                        .parts
                        .iter()
                        .filter(|p| work.glyph_by_name(&p.glyph).is_none())
                        .map(|p| p.glyph.clone()),
                );
                continue;
            };
            let (paths, advance) = place(kit, recipe, &built.paths);
            let s = judge_built(&work, kit, recipe, strategy, &built);
            if s.accepts(min_score) {
                let sources = built
                    .sources
                    .iter()
                    .filter_map(|id| work.glyphs.get(id).map(|g| g.name.clone()))
                    .collect();
                made = Some(Made {
                    name: recipe.name.clone(),
                    unicodes: unicodes.clone(),
                    class: recipe.class,
                    strategy: strategy.id.clone(),
                    paths,
                    advance_width: advance,
                    sources,
                    score: s,
                });
                break;
            }
            if best.as_ref().is_none_or(|b| s.total > b.total) {
                best = Some(s);
            }
        }
        match made {
            Some(m) => {
                add(&mut work, kit, &m);
                out.push(Outcome::Made(m));
            }
            None => {
                let reason = match &best {
                    Some(s) => format!(
                        "no strategy reached the style score {min_score:.2} (best {:.2}{}{})",
                        s.total,
                        if s.reasons.is_empty() { "" } else { ": " },
                        s.reasons.join("; ")
                    ),
                    None => {
                        missing.sort();
                        missing.dedup();
                        format!(
                            "it is made from {}, which the font lacks",
                            missing.join(", ")
                        )
                    }
                };
                out.push(Outcome::NeedsDesign {
                    name: recipe.name.clone(),
                    unicodes,
                    reason,
                    score: best,
                });
            }
        }
    }
    Ok(out)
}

/// The score of a glyph `built` for `recipe` by `strategy`, judged against
/// `kit`: the style components, then its structure (merged pieces, closed
/// counters, stray ink, an oversized mark), and whether the font's own
/// letters are reproduced by skeletons at all.
fn judge_built(
    font: &Font,
    kit: &StyleKit,
    recipe: &Recipe,
    strategy: &Strategy,
    built: &dress::Built,
) -> StyleScore {
    let mut s = score::score_with(
        font,
        kit,
        recipe,
        strategy,
        &built.paths,
        built.width_factor,
    );
    let merged = built.structure != built.expected;
    if merged {
        s.reasons.push(format!(
            "the strokes merge: {} piece(s) and {} counter(s) where the skeleton has {} and {}",
            built.structure.0, built.structure.1, built.expected.0, built.expected.1
        ));
    }
    s.reasons.extend(built.defects.iter().cloned());
    s.structure = Some(if merged || !built.defects.is_empty() {
        0.0
    } else {
        1.0
    });
    // A drawing whose own letters the skeletons do not reproduce (serifs,
    // tapers, irregular strokes): what they draw needs a person's look,
    // however well it scores.
    if strategy.parts.is_empty()
        && let Some(r) = kit.reproduction
        && r < SELF_TEST_MIN
    {
        s.reasons.push(format!(
            "skeletons match this drawing's letters at {r:.2} only (serifs, tapers or irregular \
             strokes are not drawn)"
        ));
    }
    s
}

/// Draws `recipe` by `strategy` in the style `dress_kit` and judges it
/// against `kit` as synthesis does (style components and structure): for
/// calibrating the score with kits that are wrong in one respect. `None`
/// when a piece it reuses is missing.
///
/// # Errors
///
/// [`SynthError::Geometry`] if dressing a skeleton or uniting fails.
pub fn judge(
    font: &Font,
    dress_kit: &StyleKit,
    kit: &StyleKit,
    recipe: &Recipe,
    strategy: &Strategy,
) -> Result<Option<(Vec<Path>, StyleScore)>, SynthError> {
    Ok(
        dress::build(font, dress_kit, recipe, strategy)?.map(|built| {
            (
                built.paths.clone(),
                judge_built(font, kit, recipe, strategy, &built),
            )
        }),
    )
}

/// Draws `recipe` by `strategy` in the style `kit`, placed as synthesis
/// places it, without scoring it: for inspecting what a strategy makes.
/// `None` when a piece it reuses is missing.
///
/// # Errors
///
/// [`SynthError::Geometry`] if dressing a skeleton or uniting fails.
pub fn draw(
    font: &Font,
    kit: &StyleKit,
    recipe: &Recipe,
    strategy: &Strategy,
) -> Result<Option<(Vec<Path>, f64)>, SynthError> {
    Ok(dress::build(font, kit, recipe, strategy)?.map(|b| place(kit, recipe, &b.paths)))
}

/// Adds a glyph made to the working copy, so later recipes can reuse it.
fn add(work: &mut Font, kit: &StyleKit, made: &Made) {
    let id = GlyphId::new();
    Arc::make_mut(&mut work.glyphs).insert(
        id,
        Arc::new(Glyph {
            id,
            name: made.name.clone(),
            unicodes: made.unicodes.clone(),
            export: true,
            layers: BTreeMap::from([(
                kit.master,
                Layer {
                    advance_width: made.advance_width,
                    paths: made.paths.clone(),
                    components: Vec::new(),
                },
            )]),
            provenance: None,
        }),
    );
    work.glyph_order.push(id);
}
