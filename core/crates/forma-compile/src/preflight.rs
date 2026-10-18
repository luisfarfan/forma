//! The preflight: checks that a font can be compiled and adds the glyphs that
//! every font needs.

use std::sync::Arc;

use forma_geometry::{flatten_layer, to_bezpath};
use forma_model::{Font, Glyph, GlyphId, Layer, Master, MasterId, Node, NodeKind, Path, PathId};

use crate::{CompileError, CompileReport};

/// The glyph shown for characters the font does not have.
const NOTDEF: &str = ".notdef";
/// The name of the space glyph the preflight adds.
const SPACE: &str = "space";
/// U+0020 SPACE.
const SPACE_CODEPOINT: u32 = 0x20;
/// The largest advance width an `hmtx` entry can hold.
const MAX_ADVANCE: f64 = 65535.0;
/// The range of a TrueType coordinate (and of the vertical metrics, which
/// are stored in the same 16-bit type).
const COORDINATE_RANGE: std::ops::RangeInclusive<f64> = -32768.0..=32767.0;

/// Checks that `font` can be compiled and returns a copy with the glyphs every
/// font needs; `font` itself is not changed.
///
/// Checks, in this order:
///
/// 1. the model invariants ([`Font::validate_invariants`]);
/// 2. the vertical metrics of every master are finite and round to
///    `-32768..=32767`;
/// 3. for every glyph marked for export: it has a layer for the default
///    master; its code points are Unicode scalar values; and in every layer,
///    its advance width is finite and rounds to `0..=65535`, every contour
///    converts to a Bezier path ([`to_bezpath`]) and every node coordinate
///    rounds to `-32768..=32767`. Glyphs that are not exported are not
///    checked, since they are left out of the binary.
///
/// The numeric ranges are those of the binary tables: fontc would silently
/// clamp a value outside them, producing a font that differs from the model.
///
/// Then, in the copy:
///
/// - `.notdef` is moved to the front of the glyph order, so that it becomes
///   glyph 0. If the font has no glyph of that name, one is added: an advance
///   of `round(upm / 2)` and a frame made of an outer rectangle
///   (counter-clockwise) from `(m, 0)` to `(advance - m, h)` and an inner one
///   (clockwise) inset by `m` on every side, where `m = round(upm * 0.05)` and
///   `h` is the cap height of the default master, rounded. If that is not
///   larger than `2 * m`, so that the frame would be degenerate, `h` is
///   `round(upm * 0.7)`, the cap height of a new font instead. It has no code
///   points and the same outline in every master.
/// - If no glyph is named `space` and no glyph encodes U+0020, a `space` glyph
///   is added at the end of the glyph order: U+0020, an advance of
///   `round(upm / 4)` and no contours, in every master.
///
/// The report lists the names of the added glyphs.
///
/// # Errors
///
/// [`CompileError::Preflight`] with a description of the first problem found.
pub fn preflight(font: &Font) -> Result<(Font, CompileReport), CompileError> {
    font.validate_invariants()
        .map_err(|error| CompileError::Preflight(format!("invalid font: {error}")))?;
    for master in &font.masters {
        check_metrics(master)?;
    }
    // The invariants guarantee a master, so the default master exists.
    let default_master = font.default_master().id;
    for glyph in font.glyph_order.iter().filter_map(|id| font.glyphs.get(id)) {
        if glyph.is_exported() {
            check_glyph(font, glyph, default_master)?;
        }
    }

    let mut out = font.clone();
    let mut report = CompileReport::default();
    if ensure_notdef(&mut out) {
        report.added_glyphs.push(NOTDEF.to_owned());
    }
    if needs_space(&out) {
        add_space(&mut out);
        report.added_glyphs.push(SPACE.to_owned());
    }
    debug_assert!(out.validate_invariants().is_ok());
    Ok((out, report))
}

/// Whether `value` is finite and rounds to a 16-bit signed integer.
fn fits_i16(value: f64) -> bool {
    value.is_finite() && COORDINATE_RANGE.contains(&value.round())
}

/// Checks that the vertical metrics of `master` can be stored.
fn check_metrics(master: &Master) -> Result<(), CompileError> {
    let metrics = &master.metrics;
    for (what, value) in [
        ("ascender", metrics.ascender),
        ("descender", metrics.descender),
        ("x-height", metrics.x_height),
        ("cap height", metrics.cap_height),
    ] {
        if !fits_i16(value) {
            return Err(CompileError::Preflight(format!(
                "master {:?} has the {what} {value}; \
                 it must be a finite number that rounds to -32768..=32767",
                master.name
            )));
        }
    }
    Ok(())
}

/// Checks the data of an exported glyph that the compiler relies on.
fn check_glyph(font: &Font, glyph: &Glyph, default_master: MasterId) -> Result<(), CompileError> {
    let fail =
        |problem: String| CompileError::Preflight(format!("glyph {:?} {problem}", glyph.name));

    if !glyph.layers.contains_key(&default_master) {
        return Err(fail(format!(
            "has no layer for the default master {default_master}"
        )));
    }
    if let Some(codepoint) = glyph
        .unicodes
        .iter()
        .find(|&&codepoint| char::from_u32(codepoint).is_none())
    {
        return Err(fail(format!(
            "has the code point U+{codepoint:04X}, which is not a Unicode scalar value"
        )));
    }
    for (master, layer) in &glyph.layers {
        let advance = layer.advance_width;
        if !(advance.is_finite() && (0.0..=MAX_ADVANCE).contains(&advance.round())) {
            return Err(fail(format!(
                "has the advance width {advance} in master {master}; \
                 it must be a finite number that rounds to 0..=65535"
            )));
        }
        for path in &layer.paths {
            to_bezpath(path).map_err(|error| {
                fail(format!(
                    "has a malformed contour {} in master {master}: {error}",
                    path.id.0
                ))
            })?;
            if let Some(node) = path
                .nodes
                .iter()
                .find(|n| !(fits_i16(n.x) && fits_i16(n.y)))
            {
                return Err(fail(format!(
                    "has the node ({}, {}) in contour {} of master {master}, \
                     outside the TrueType coordinate range -32768..=32767",
                    node.x, node.y, path.id.0
                )));
            }
        }
        if !layer.components.is_empty() {
            check_flattened(font, glyph, *master).map_err(fail)?;
        }
    }
    Ok(())
}

/// Checks that the glyph, with its components expanded, still fits the
/// TrueType coordinate range. The expansion also fails for a component whose
/// base has no layer in `master`, which the model invariants rule out.
fn check_flattened(font: &Font, glyph: &Glyph, master: MasterId) -> Result<(), String> {
    let paths = flatten_layer(font, glyph.id, master).map_err(|error| {
        format!("has components that cannot be expanded in master {master}: {error}")
    })?;
    for path in &paths {
        if let Some(node) = path
            .nodes
            .iter()
            .find(|n| !(fits_i16(n.x) && fits_i16(n.y)))
        {
            return Err(format!(
                "has the node ({}, {}) once its components are expanded in master {master}, \
                 outside the TrueType coordinate range -32768..=32767",
                node.x, node.y
            ));
        }
    }
    Ok(())
}

/// Puts `.notdef` first in the glyph order, adding one if the font has none.
/// Returns whether it was added.
fn ensure_notdef(font: &mut Font) -> bool {
    let (id, added) = match font.glyph_by_name(NOTDEF) {
        Some(glyph) => (glyph.id, false),
        None => {
            let glyph = notdef_glyph(font);
            let id = glyph.id;
            Arc::make_mut(&mut font.glyphs).insert(id, Arc::new(glyph));
            (id, true)
        }
    };
    font.glyph_order.retain(|&listed| listed != id);
    font.glyph_order.insert(0, id);
    added
}

/// A `.notdef` glyph for `font`, as described on [`preflight`].
fn notdef_glyph(font: &Font) -> Glyph {
    let upm = f64::from(font.units_per_em);
    let advance = (upm / 2.0).round();
    // `upm / 20` is `upm * 0.05` without the rounding error of 0.05.
    let margin = (upm / 20.0).round();
    let cap_height = font.default_master().metrics.cap_height.round();
    let height = if cap_height > 2.0 * margin {
        cap_height
    } else {
        (upm * 7.0 / 10.0).round()
    };

    let outer = rectangle(PathId(0), [margin, 0.0, advance - margin, height], true);
    let inner = rectangle(
        PathId(1),
        [
            2.0 * margin,
            margin,
            advance - 2.0 * margin,
            height - margin,
        ],
        false,
    );
    let layer = Layer {
        advance_width: advance,
        paths: vec![outer, inner],
        components: Vec::new(),
    };
    Glyph {
        id: GlyphId::new(),
        name: NOTDEF.to_owned(),
        unicodes: Vec::new(),
        export: true,
        layers: font
            .masters
            .iter()
            .map(|master| (master.id, layer.clone()))
            .collect(),
        provenance: None,
    }
}

/// A closed rectangular contour from `(x0, y0)` to `(x1, y1)`, starting at
/// `(x0, y0)`; counter-clockwise when `ccw`, clockwise otherwise.
fn rectangle(id: PathId, [x0, y0, x1, y1]: [f64; 4], ccw: bool) -> Path {
    let corners = if ccw {
        [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    } else {
        [(x0, y0), (x0, y1), (x1, y1), (x1, y0)]
    };
    Path {
        id,
        closed: true,
        nodes: corners
            .into_iter()
            .map(|(x, y)| Node {
                x,
                y,
                kind: NodeKind::Line,
                smooth: false,
            })
            .collect(),
    }
}

/// Whether the font has neither a glyph named `space` nor one encoding U+0020.
fn needs_space(font: &Font) -> bool {
    !font
        .glyphs
        .values()
        .any(|glyph| glyph.name == SPACE || glyph.unicodes.contains(&SPACE_CODEPOINT))
}

/// Adds an empty `space` glyph for U+0020 at the end of the glyph order.
fn add_space(font: &mut Font) {
    let layer = Layer {
        advance_width: (f64::from(font.units_per_em) / 4.0).round(),
        paths: Vec::new(),
        components: Vec::new(),
    };
    let glyph = Glyph {
        id: GlyphId::new(),
        name: SPACE.to_owned(),
        unicodes: vec![SPACE_CODEPOINT],
        export: true,
        layers: font
            .masters
            .iter()
            .map(|master| (master.id, layer.clone()))
            .collect(),
        provenance: None,
    };
    font.glyph_order.push(glyph.id);
    Arc::make_mut(&mut font.glyphs).insert(glyph.id, Arc::new(glyph));
}
