//! Fonts shared by the integration tests.
#![allow(dead_code)]

use std::sync::Arc;

use forma_geometry::{CoordinateSystem, paths_from_svg};
use forma_model::{Font, Glyph, GlyphId, Layer, Node, NodeKind, Path, PathId};

/// The family name of [`demo_font`].
pub const DEMO_FAMILY: &str = "Forma Demo";

/// The units per em of [`demo_font`].
pub const DEMO_UPM: u16 = 1000;

// ---------------------------------------------------------------- SVG shapes
//
// All shapes are SVG path data in font coordinates (y up). Outer shapes run
// counter-clockwise and counters clockwise, as PostScript direction asks.

/// An outer rectangle (counter-clockwise).
pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    format!("M{x0} {y0} H{x1} V{y1} H{x0} Z")
}

/// A rectangular counter (clockwise).
pub fn rect_counter(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    format!("M{x0} {y0} V{y1} H{x1} V{y0} Z")
}

/// An outer circle (counter-clockwise), drawn as four quarter arcs, which the
/// SVG import turns into cubic segments.
pub fn circle(cx: f64, cy: f64, r: f64) -> String {
    let (right, top, left, bottom) = (cx + r, cy + r, cx - r, cy - r);
    format!(
        "M{right} {cy} A{r} {r} 0 0 1 {cx} {top} A{r} {r} 0 0 1 {left} {cy} \
         A{r} {r} 0 0 1 {cx} {bottom} A{r} {r} 0 0 1 {right} {cy} Z"
    )
}

/// A circular counter (clockwise).
pub fn circle_counter(cx: f64, cy: f64, r: f64) -> String {
    let (right, top, left, bottom) = (cx + r, cy + r, cx - r, cy - r);
    format!(
        "M{right} {cy} A{r} {r} 0 0 0 {cx} {bottom} A{r} {r} 0 0 0 {left} {cy} \
         A{r} {r} 0 0 0 {cx} {top} A{r} {r} 0 0 0 {right} {cy} Z"
    )
}

/// Parses SVG path data in font coordinates into contours.
pub fn svg(parts: &[String]) -> Vec<Path> {
    paths_from_svg(&parts.join(" "), CoordinateSystem::Font).expect("valid SVG path data")
}

// ---------------------------------------------------------------- nodes

pub fn node(x: f64, y: f64, kind: NodeKind) -> Node {
    Node {
        x,
        y,
        kind,
        smooth: false,
    }
}

pub fn closed(id: u32, nodes: Vec<Node>) -> Path {
    Path {
        id: PathId(id),
        closed: true,
        nodes,
    }
}

// ---------------------------------------------------------------- fonts

/// Adds an exportable glyph with the same layer in every master of the font,
/// at the end of the glyph order.
pub fn add_glyph(
    font: &mut Font,
    name: &str,
    unicodes: &[u32],
    advance_width: f64,
    paths: Vec<Path>,
) -> GlyphId {
    let id = GlyphId::new();
    let layers = font
        .masters
        .iter()
        .map(|master| {
            (
                master.id,
                Layer {
                    advance_width,
                    paths: paths.clone(),
                },
            )
        })
        .collect();
    let glyph = Glyph {
        id,
        name: name.to_owned(),
        unicodes: unicodes.to_vec(),
        export: true,
        layers,
    };
    font.glyph_order.push(id);
    Arc::make_mut(&mut font.glyphs).insert(id, Arc::new(glyph));
    id
}

/// Edits a glyph of the font in place.
pub fn edit_glyph(font: &mut Font, id: GlyphId, edit: impl FnOnce(&mut Glyph)) {
    let glyphs = Arc::make_mut(&mut font.glyphs);
    let glyph = glyphs.get_mut(&id).expect("the glyph exists");
    edit(Arc::make_mut(glyph));
}

/// The names of the glyphs of [`demo_font`], in glyph order.
pub const DEMO_GLYPHS: [&str; 6] = ["A", "B", "H", "O", "n", "o"];

/// The default-master advance width of the glyph `name` of `font`.
pub fn advance_of(font: &Font, name: &str) -> f64 {
    let glyph = font.glyph_by_name(name).expect("the glyph exists");
    glyph.layers[&font.default_master().id].advance_width
}

/// A small font (1000 units per em, cap height 700, x-height 500) whose
/// glyphs `A`, `B`, `H`, `O`, `n` and `o` are built from rectangles and
/// circles imported from SVG path data, with PostScript contour direction and
/// their usual code points. It has no `.notdef` and no `space`. `A` has a
/// fractional advance width (612.5) so that rounding is exercised.
pub fn demo_font() -> Font {
    let mut font = Font::new(DEMO_FAMILY, DEMO_UPM);

    // A block `A`: a frame with a counter.
    let a = svg(&[
        rect(50.0, 0.0, 562.0, 700.0),
        rect_counter(150.0, 350.0, 462.0, 600.0),
    ]);
    add_glyph(&mut font, "A", &['A' as u32], 612.5, a);

    // `B`: a frame with two counters.
    let b = svg(&[
        rect(60.0, 0.0, 540.0, 700.0),
        rect_counter(160.0, 100.0, 440.0, 300.0),
        rect_counter(160.0, 400.0, 440.0, 600.0),
    ]);
    add_glyph(&mut font, "B", &['B' as u32], 600.0, b);

    // `H`: two stems and a crossbar that touches them.
    let h = svg(&[
        rect(60.0, 0.0, 160.0, 700.0),
        rect(460.0, 0.0, 560.0, 700.0),
        rect(160.0, 300.0, 460.0, 400.0),
    ]);
    add_glyph(&mut font, "H", &['H' as u32], 620.0, h);

    // `O`: a ring.
    let o_upper = svg(&[
        circle(350.0, 350.0, 300.0),
        circle_counter(350.0, 350.0, 200.0),
    ]);
    add_glyph(&mut font, "O", &['O' as u32], 700.0, o_upper);

    // `n`: a stem, a shoulder and a shorter right stem.
    let n = svg(&[
        rect(60.0, 0.0, 140.0, 500.0),
        rect(140.0, 420.0, 440.0, 500.0),
        rect(360.0, 0.0, 440.0, 420.0),
    ]);
    add_glyph(&mut font, "n", &['n' as u32], 500.0, n);

    // `o`: a ring sitting on the baseline and reaching the x-height.
    let o_lower = svg(&[
        circle(260.0, 250.0, 250.0),
        circle_counter(260.0, 250.0, 170.0),
    ]);
    add_glyph(&mut font, "o", &['o' as u32], 520.0, o_lower);

    font
}
