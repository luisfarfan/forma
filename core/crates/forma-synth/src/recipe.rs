//! Recipes: how each character is made, as data (`recipes/*.json`).
//!
//! A recipe lists strategies in order. The first strategies **derive** the
//! glyph from glyphs the font already has (the `0` from the `O`, the `;`
//! from the period and the comma): their shapes carry the drawing's style
//! as it is. The last is a **skeleton**: centre lines in a box normalized to
//! the glyph (x from 0 to 1 across its ink, y from 0 to 1 up the cap height,
//! both inset by half a stroke), dressed with the font's measured stroke,
//! contrast and terminals (see [`crate::dress`]). A recipe without a
//! strategy that can run asks for a design instead.
//!
//! The recipes and their skeletons are original work of FORMA, under the
//! repository's license.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SynthError;

/// What kind of glyph a recipe makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// A figure: as tall and as dark as the letters.
    Digit,
    /// Punctuation.
    Punct,
    /// A symbol.
    Symbol,
    /// A combining mark: no advance, centred on x = 0, above the cap height.
    Mark,
    /// The space: no ink.
    Space,
}

/// One segment of a skeleton stroke, in the normalized box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Seg {
    /// A straight line to this point (the first one starts the stroke).
    Line([f64; 2]),
    /// An elliptical arc: centre, radii, and the start and end angles in
    /// degrees (counter-clockwise from the +x axis; the end may be below the
    /// start to go clockwise).
    Arc([f64; 6]),
    /// A whole ellipse: centre and radii (a closed stroke on its own).
    Ellipse([f64; 4]),
}

/// A skeleton stroke.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StrokeSpec {
    /// The segments, in order.
    pub segs: Vec<Seg>,
    /// The width relative to the font's stroke; 1 by default.
    #[serde(default = "one")]
    pub w: f64,
}

/// A round dot (a period, the dot of an exclamation mark): as big as the
/// font's balls, or a little bigger than its stroke.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Dot {
    /// Its centre in the normalized box.
    pub at: [f64; 2],
    /// Its size relative to the font's dot; 1 by default.
    #[serde(default = "one")]
    pub r: f64,
    /// Whether a stroke of the glyph starts in it by design (the head of a
    /// comma): it then counts as one piece with that stroke.
    #[serde(default)]
    pub joined: bool,
}

/// A glyph of the font reused as a piece.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Part {
    /// The glyph's name.
    pub glyph: String,
    /// Turned half round (180°), within its own ink box.
    #[serde(default)]
    pub rotate: bool,
    /// Mirrored left to right, within its own ink box.
    #[serde(default)]
    pub mirror: bool,
    /// The ink width to scale the piece to, in cap heights (scaled with the
    /// font's proportions like a recipe's width); its height is kept.
    #[serde(default)]
    pub width: Option<f64>,
    /// Horizontal offset, in cap heights: of the piece's ink left edge from
    /// x = 0, or of its ink centre when `center` is true.
    #[serde(default)]
    pub dx: f64,
    /// Further horizontal offset, in widths of the piece's own ink (2.2
    /// puts a second dot 1.2 dots after the first).
    #[serde(default)]
    pub advance: f64,
    /// Whether `dx` places the piece's ink centre instead of its left edge.
    #[serde(default)]
    pub center: bool,
    /// Vertical offset, in cap heights.
    #[serde(default)]
    pub dy: f64,
    /// When given, the height of the piece's ink top, in cap heights
    /// (instead of `dy`).
    #[serde(default)]
    pub top: Option<f64>,
}

/// One way of making a glyph: pieces of existing glyphs, skeleton strokes
/// and dots, all drawn together.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Strategy {
    /// What it is, for the provenance (`derive:O`, `skeleton`).
    pub id: String,
    /// Glyphs of the font reused as pieces; the strategy runs only if they
    /// all exist.
    #[serde(default)]
    pub parts: Vec<Part>,
    /// Skeleton strokes.
    #[serde(default)]
    pub strokes: Vec<StrokeSpec>,
    /// Dots.
    #[serde(default)]
    pub dots: Vec<Dot>,
}

/// How one character is made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Recipe {
    /// The glyph name.
    pub name: String,
    /// The code points it encodes, as `U+XXXX` (none for a mark).
    #[serde(default)]
    pub unicodes: Vec<String>,
    /// Its kind.
    pub class: Class,
    /// The ink width of the skeleton, in cap heights, scaled with the
    /// font's proportions (see [`crate::StyleKit::proportion`]); for the
    /// space, its advance width.
    #[serde(default)]
    pub width: f64,
    /// The height of the skeleton's box, in cap heights; 1 by default (a
    /// mark's box is lower; it is placed above the cap height).
    #[serde(default = "one")]
    pub height: f64,
    /// The strategies, in order of preference.
    #[serde(default)]
    pub strategies: Vec<Strategy>,
    /// Present when no recipe can make this glyph faithfully: why, and what
    /// to do.
    #[serde(default)]
    pub needs_design: Option<String>,
    /// Whether the glyph has strokes to compare with the font's (a period,
    /// a dot-only glyph, does not); true by default.
    #[serde(default = "yes")]
    pub stroke_check: bool,
}

/// A file of recipes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RecipeBook {
    /// The version of the recipes, recorded in each glyph's provenance.
    pub version: u32,
    /// The recipes, in an order where every piece a recipe reuses comes
    /// before it.
    pub glyphs: Vec<Recipe>,
}

const fn one() -> f64 {
    1.0
}

const fn yes() -> bool {
    true
}

fn positive(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

/// The recipes of the `unicase-es` profile.
pub const UNICASE_ES: &str = include_str!("../recipes/unicase-es.json");

impl RecipeBook {
    /// Parses and checks a recipe file.
    ///
    /// # Errors
    ///
    /// [`SynthError::Recipes`] for malformed JSON, an unknown code point
    /// text, a non-positive width or stroke width, or a recipe that reuses
    /// a recipe glyph defined after it.
    pub fn parse(text: &str) -> Result<RecipeBook, SynthError> {
        let book: RecipeBook =
            serde_json::from_str(text).map_err(|e| SynthError::Recipes(e.to_string()))?;
        let mut seen = std::collections::BTreeSet::new();
        let names: std::collections::BTreeSet<&str> =
            book.glyphs.iter().map(|r| r.name.as_str()).collect();
        for r in &book.glyphs {
            for u in &r.unicodes {
                parse_code_point(u).ok_or_else(|| {
                    SynthError::Recipes(format!("{}: bad code point {u:?}", r.name))
                })?;
            }
            for s in &r.strategies {
                if s.strokes.iter().any(|k| !positive(k.w)) {
                    return Err(SynthError::Recipes(format!("{}: stroke width", r.name)));
                }
                for p in &s.parts {
                    if names.contains(p.glyph.as_str()) && !seen.contains(p.glyph.as_str()) {
                        return Err(SynthError::Recipes(format!(
                            "{} reuses {} before its recipe",
                            r.name, p.glyph
                        )));
                    }
                }
            }
            if r.needs_design.is_none() && !(positive(r.width) && positive(r.height)) {
                return Err(SynthError::Recipes(format!("{}: width or height", r.name)));
            }
            if r.strategies.iter().flat_map(|s| &s.parts).any(|p| {
                p.width.is_some_and(|w| !positive(w)) || !(p.dx.is_finite() && p.dy.is_finite())
            }) {
                return Err(SynthError::Recipes(format!("{}: piece placement", r.name)));
            }
            seen.insert(r.name.as_str());
        }
        Ok(book)
    }

    /// The recipe of `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Recipe> {
        self.glyphs.iter().find(|r| r.name == name)
    }
}

/// `U+XXXX` as a number.
#[must_use]
pub fn parse_code_point(text: &str) -> Option<u32> {
    let hex = text.strip_prefix("U+")?;
    u32::from_str_radix(hex, 16).ok()
}
