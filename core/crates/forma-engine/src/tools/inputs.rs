//! Argument types and lookups shared by the glyph, metrics and geometry
//! tools: code points, outlines (SVG path data or structured contours) and
//! their coordinate system, and glyph, master and grid resolution.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use forma_edit::GlyphRef;
use forma_geometry::{CoordinateSystem, paths_from_svg};
use forma_model::{Font, Glyph, GlyphId, MasterId, Node, NodeKind, Path, PathId};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::errors::{ErrorCode, ToolError};
use crate::tools::limits::{check_nodes, path_nodes};

// ------------------------------------------------------------- code points

/// The most hexadecimal digits a code point written as text may have.
const MAX_HEX_DIGITS: usize = 6;

/// Text longer than this is not quoted back in error messages.
const MAX_QUOTED: usize = 32;

/// A Unicode code point given as an integer or as text such as `"U+0041"` or
/// `"0x41"` (case-insensitive, 1 to 6 hexadecimal digits). Results always
/// report code points as integers, and so does its serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CodePoint(pub u32);

/// Parses `U+hhhh` or `0xhhhh` text.
fn parse_code_point(text: &str) -> Option<u32> {
    let digits = ["U+", "u+", "0x", "0X"]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))?;
    if digits.is_empty()
        || digits.len() > MAX_HEX_DIGITS
        || !digits.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

impl<'de> Deserialize<'de> for CodePoint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CodePointVisitor;

        impl Visitor<'_> for CodePointVisitor {
            type Value = CodePoint;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a code point: an integer, or text such as \"U+0041\" or \"0x41\"")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<CodePoint, E> {
                u32::try_from(value)
                    .map(CodePoint)
                    .map_err(|_| E::custom(format!("code point {value} is out of range")))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<CodePoint, E> {
                u32::try_from(value)
                    .map(CodePoint)
                    .map_err(|_| E::custom(format!("code point {value} is out of range")))
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<CodePoint, E> {
                parse_code_point(text).map(CodePoint).ok_or_else(|| {
                    let quoted = if text.len() <= MAX_QUOTED {
                        format!(" {text:?}")
                    } else {
                        String::new()
                    };
                    E::custom(format!(
                        "invalid code point{quoted}: use an integer, or text such as \
                         \"U+0041\" or \"0x41\""
                    ))
                })
            }
        }

        deserializer.deserialize_any(CodePointVisitor)
    }
}

impl JsonSchema for CodePoint {
    fn schema_name() -> Cow<'static, str> {
        "CodePoint".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A Unicode code point: an integer, or text such as \"U+0041\" or \"0x41\".",
            "oneOf": [
                { "type": "integer", "minimum": 0, "maximum": 1_114_111 },
                { "type": "string", "pattern": "^([Uu]\\+|0[xX])[0-9A-Fa-f]{1,6}$" }
            ]
        })
    }
}

/// The values of `code_points`.
#[must_use]
pub(crate) fn code_points(code_points: &[CodePoint]) -> Vec<u32> {
    code_points.iter().map(|code_point| code_point.0).collect()
}

// ---------------------------------------------------------------- outlines

/// The coordinate system of an outline given as input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "CoordinateSystem")]
pub enum CoordinateSystemArg {
    /// Font coordinates: y points up, the baseline is y = 0. The default.
    #[default]
    Font,
    /// SVG coordinates: y points down from the ascender of the default
    /// master (each y becomes `ascender - y`).
    SvgYDown,
}

/// A node of a contour given as input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeInput {
    /// Horizontal position.
    pub x: f64,
    /// Vertical position.
    pub y: f64,
    /// `line`, `curve` (cubic) or `qcurve` (quadratic) for an on-curve node,
    /// which names the segment arriving at it; `offcurve` for a control
    /// point. A cubic segment is two `offcurve` nodes then a `curve`; a
    /// quadratic one is one `offcurve` then a `qcurve`.
    pub kind: NodeKind,
    /// Whether the node joins its segments smoothly.
    #[serde(default)]
    pub smooth: bool,
}

/// A contour given as input. Contours are numbered by position; an `id` is
/// accepted (so that the `paths` of glyph_get can be sent back) and ignored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathInput {
    /// Ignored: contours are numbered by their position.
    #[serde(default)]
    pub id: Option<u32>,
    /// Whether the contour is closed.
    pub closed: bool,
    /// The nodes, in drawing order. Outer contours run counter-clockwise,
    /// inner ones clockwise (y up).
    pub nodes: Vec<NodeInput>,
}

/// The ascender of the default master, the origin of SVG y-down input.
fn default_ascender(font: &Font) -> Result<f64, ToolError> {
    font.masters
        .first()
        .map(|master| master.metrics.ascender)
        .ok_or_else(|| ToolError::invalid_argument("the font has no masters"))
}

/// The contours of an outline given either as SVG path data or as structured
/// contours, in font coordinates and numbered from 0; `None` when neither is
/// given. The direction of every contour is kept as given.
///
/// # Errors
///
/// * `INVALID_ARGUMENT` if both are given;
/// * `INVALID_GEOMETRY` with the parser's message for SVG data that does not
///   parse;
/// * `LIMIT_EXCEEDED` for more than [`MAX_NODES`](crate::limits::MAX_NODES)
///   nodes.
pub(crate) fn outline_paths(
    font: &Font,
    svg_path: Option<String>,
    paths: Option<Vec<PathInput>>,
    coordinate_system: CoordinateSystemArg,
) -> Result<Option<Vec<Path>>, ToolError> {
    let ascender = match coordinate_system {
        CoordinateSystemArg::Font => None,
        CoordinateSystemArg::SvgYDown => Some(default_ascender(font)?),
    };
    match (svg_path, paths) {
        (Some(_), Some(_)) => Err(ToolError::invalid_argument(
            "give the outline either as `svg_path` or as structured contours, not both",
        )),
        (None, None) => Ok(None),
        (Some(svg_path), None) => {
            let system = ascender.map_or(CoordinateSystem::Font, |ascender| {
                CoordinateSystem::SvgYDown { ascender }
            });
            let paths = paths_from_svg(&svg_path, system)
                .map_err(|error| ToolError::new(ErrorCode::InvalidGeometry, error.to_string()))?;
            check_nodes(path_nodes(&paths))?;
            Ok(Some(paths))
        }
        (None, Some(input)) => {
            check_nodes(input.iter().map(|path| path.nodes.len()).sum())?;
            input
                .into_iter()
                .enumerate()
                .map(|(index, path)| model_path(index, path, ascender))
                .collect::<Result<Vec<Path>, ToolError>>()
                .map(Some)
        }
    }
}

fn model_path(index: usize, path: PathInput, ascender: Option<f64>) -> Result<Path, ToolError> {
    let id = u32::try_from(index)
        .map_err(|_| ToolError::invalid_argument("too many contours in one outline"))?;
    Ok(Path {
        id: PathId(id),
        closed: path.closed,
        nodes: path
            .nodes
            .into_iter()
            .map(|node| Node {
                x: node.x,
                y: ascender.map_or(node.y, |ascender| ascender - node.y),
                kind: node.kind,
                smooth: node.smooth,
            })
            .collect(),
    })
}

// ----------------------------------------------------------------- lookups

/// The glyph `glyph` refers to (by id or name, see [`GlyphRef::resolve`]).
///
/// # Errors
///
/// `NOT_FOUND` if there is no such glyph.
pub(crate) fn find_glyph<'f>(
    font: &'f Font,
    glyph: &GlyphRef,
) -> Result<&'f Arc<Glyph>, ToolError> {
    glyph
        .resolve(font)
        .and_then(|id| font.glyphs.get(&id))
        .ok_or_else(|| ToolError::not_found(format!("not found: glyph '{glyph}'")))
}

/// The given master, or the default (first) one.
///
/// # Errors
///
/// `NOT_FOUND` if the font has no master with that id.
pub(crate) fn find_master(font: &Font, master: Option<MasterId>) -> Result<MasterId, ToolError> {
    match master {
        None => font
            .masters
            .first()
            .map(|master| master.id)
            .ok_or_else(|| ToolError::invalid_argument("the font has no masters")),
        Some(id) if font.masters.iter().any(|candidate| candidate.id == id) => Ok(id),
        Some(id) => Err(ToolError::not_found(format!("not found: master '{id}'"))),
    }
}

/// The glyphs `glyphs` refers to, or every glyph when it is `None`, in glyph
/// order and without repetition.
///
/// # Errors
///
/// `INVALID_ARGUMENT` for an empty list; `NOT_FOUND` for a reference to a
/// glyph that does not exist.
pub(crate) fn selected_glyphs<'f>(
    font: &'f Font,
    glyphs: Option<&[GlyphRef]>,
) -> Result<Vec<&'f Arc<Glyph>>, ToolError> {
    let wanted: Option<BTreeSet<GlyphId>> = match glyphs {
        None => None,
        Some([]) => {
            return Err(ToolError::invalid_argument(
                "`glyphs` must not be empty; omit it to select every glyph",
            ));
        }
        Some(refs) => Some(
            refs.iter()
                .map(|glyph| find_glyph(font, glyph).map(|found| found.id))
                .collect::<Result<_, _>>()?,
        ),
    };
    Ok(font
        .glyph_order
        .iter()
        .filter(|id| wanted.as_ref().is_none_or(|wanted| wanted.contains(id)))
        .filter_map(|id| font.glyphs.get(id))
        .collect())
}

/// The name of the glyph `glyph` refers to, or the reference itself, for
/// history labels.
#[must_use]
pub(crate) fn glyph_label(font: &Font, glyph: &GlyphRef) -> String {
    find_glyph(font, glyph).map_or_else(|_| glyph.to_string(), |found| found.name.clone())
}

/// The default grid, in font units, for geometry checks and fixes.
pub(crate) const fn default_grid() -> f64 {
    1.0
}

/// Checks a grid argument: a finite number, 0 (no grid) or more.
///
/// # Errors
///
/// `INVALID_ARGUMENT` otherwise.
pub(crate) fn check_grid(grid: f64) -> Result<(), ToolError> {
    if grid.is_finite() && grid >= 0.0 {
        Ok(())
    } else {
        Err(ToolError::invalid_argument(format!(
            "grid must be a number of font units, 0 (no grid) or more, got {grid}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(value: serde_json::Value) -> Result<u32, String> {
        serde_json::from_value::<CodePoint>(value)
            .map(|code_point| code_point.0)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn code_points_accept_integers_and_hex_text() {
        assert_eq!(parse(json!(65)), Ok(65));
        assert_eq!(parse(json!("U+0041")), Ok(65));
        assert_eq!(parse(json!("u+1f600")), Ok(0x1F600));
        assert_eq!(parse(json!("0x41")), Ok(65));
        assert_eq!(parse(json!("0X10FFFF")), Ok(0x10_FFFF));
        for bad in [
            json!("65"),
            json!("U+"),
            json!("0x"),
            json!("U+0041 "),
            json!("U+-41"),
            json!("0x1234567"),
            json!(-1),
            json!(4_294_967_296_u64),
            json!(6.5),
            json!(null),
            json!([65]),
        ] {
            assert!(parse(bad.clone()).is_err(), "{bad}");
        }
        let long = "x".repeat(100);
        let message = parse(json!(long)).expect_err("invalid");
        assert!(!message.contains(&long), "{message}");
    }

    #[test]
    fn outline_needs_exactly_one_form() {
        let font = Font::new("Test", 1000);
        let error = outline_paths(
            &font,
            Some("M0 0 L1 0 L1 1 Z".into()),
            Some(Vec::new()),
            CoordinateSystemArg::Font,
        )
        .expect_err("both");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert_eq!(
            outline_paths(&font, None, None, CoordinateSystemArg::Font),
            Ok(None)
        );
        let error = outline_paths(
            &font,
            Some("M0 0 L".into()),
            None,
            CoordinateSystemArg::Font,
        )
        .expect_err("malformed");
        assert_eq!(error.code, ErrorCode::InvalidGeometry);
    }

    #[test]
    fn structured_contours_are_renumbered_and_flipped() {
        let font = Font::new("Test", 1000);
        let input = |id| PathInput {
            id,
            closed: true,
            nodes: vec![
                NodeInput {
                    x: 1.0,
                    y: 10.0,
                    kind: NodeKind::Line,
                    smooth: false,
                },
                NodeInput {
                    x: 2.0,
                    y: 20.0,
                    kind: NodeKind::Line,
                    smooth: true,
                },
            ],
        };
        let paths = outline_paths(
            &font,
            None,
            Some(vec![input(Some(7)), input(None)]),
            CoordinateSystemArg::SvgYDown,
        )
        .expect("valid")
        .expect("given");
        assert_eq!(paths[0].id, PathId(0));
        assert_eq!(paths[1].id, PathId(1));
        assert_eq!((paths[0].nodes[0].x, paths[0].nodes[0].y), (1.0, 790.0));
        assert!(paths[1].nodes[1].smooth);
    }
}
