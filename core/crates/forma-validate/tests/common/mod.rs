//! Fonts shared by the integration tests.
#![allow(dead_code)]

use std::sync::Arc;

use forma_geometry::{CoordinateSystem, paths_from_svg};
use forma_model::{
    Font, Glyph, GlyphId, Layer, Master, MasterId, Node, NodeKind, Path, PathId, VerticalMetrics,
};

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

/// An outer circle (counter-clockwise), drawn as four quarter arcs.
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

/// A closed rectangle with integer corners, counter-clockwise, as plain line
/// nodes: nothing for any geometry check to say about it.
pub fn clean_rect(id: u32, x0: i32, y0: i32, x1: i32, y1: i32) -> Path {
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
    Path {
        id: PathId(id),
        closed: true,
        nodes: corners
            .into_iter()
            .map(|(x, y)| Node {
                x: f64::from(x),
                y: f64::from(y),
                kind: NodeKind::Line,
                smooth: false,
            })
            .collect(),
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
                    components: Vec::new(),
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

/// The id of the glyph called `name`.
pub fn id_of(font: &Font, name: &str) -> GlyphId {
    font.glyph_by_name(name).expect("the glyph exists").id
}

/// The default-master layer of the glyph `id`, for editing.
pub fn default_layer(font: &mut Font, id: GlyphId, edit: impl FnOnce(&mut Layer)) {
    let master = font.default_master().id;
    edit_glyph(font, id, |glyph| {
        edit(glyph.layers.get_mut(&master).expect("default layer"));
    });
}

/// Adds a second master called `name` to the font, with a copy of the default
/// layer of every glyph. Returns its id.
pub fn add_master(font: &mut Font, name: &str) -> MasterId {
    let default = font.default_master().id;
    let id = MasterId::new();
    font.masters.push(Master {
        id,
        name: name.to_owned(),
        metrics: font.default_master().metrics.clone(),
    });
    let ids: Vec<GlyphId> = font.glyphs.keys().copied().collect();
    for glyph_id in ids {
        edit_glyph(font, glyph_id, |glyph| {
            let layer = glyph.layers[&default].clone();
            glyph.layers.insert(id, layer);
        });
    }
    id
}

/// Sets the vertical metrics of the default master.
pub fn set_vertical_metrics(font: &mut Font, ascender: f64, descender: f64) {
    font.masters[0].metrics = VerticalMetrics {
        ascender,
        descender,
        ..font.masters[0].metrics.clone()
    };
}

/// Reverses the drawing order of a contour's nodes, which flips its direction
/// (for the all-line rectangles used here).
pub fn reverse(path: &mut Path) {
    path.nodes.reverse();
}

/// A tiny font whose geometry checks have nothing to say: `H` and `I` are
/// plain integer rectangles with Unicode values. It has no `.notdef` and no
/// `space`.
pub fn clean_font() -> Font {
    let mut font = Font::new("Forma Clean", 1000);
    add_glyph(
        &mut font,
        "H",
        &['H' as u32],
        600.0,
        vec![clean_rect(0, 50, 0, 550, 700)],
    );
    add_glyph(
        &mut font,
        "I",
        &['I' as u32],
        300.0,
        vec![clean_rect(0, 50, 0, 250, 700)],
    );
    font
}

/// The default-master advance width of the glyph `name` of `font`.
pub fn advance_of(font: &Font, name: &str) -> f64 {
    let glyph = font.glyph_by_name(name).expect("the glyph exists");
    glyph.layers[&font.default_master().id].advance_width
}

/// The names of the glyphs of [`demo_font`], in glyph order.
pub const DEMO_GLYPHS: [&str; 6] = ["A", "B", "H", "O", "n", "o"];

/// A small font (1000 units per em, cap height 700, x-height 500) whose
/// glyphs `A`, `B`, `H`, `O`, `n` and `o` are built from rectangles and
/// circles imported from SVG path data, with PostScript contour direction and
/// their usual code points. It has no `.notdef` and no `space`. `A` has a
/// fractional advance width (612.5) so that rounding is exercised.
pub fn demo_font() -> Font {
    let mut font = Font::new("Forma Demo", 1000);

    let a = svg(&[
        rect(50.0, 0.0, 562.0, 700.0),
        rect_counter(150.0, 350.0, 462.0, 600.0),
    ]);
    add_glyph(&mut font, "A", &['A' as u32], 612.5, a);

    let b = svg(&[
        rect(60.0, 0.0, 540.0, 700.0),
        rect_counter(160.0, 100.0, 440.0, 300.0),
        rect_counter(160.0, 400.0, 440.0, 600.0),
    ]);
    add_glyph(&mut font, "B", &['B' as u32], 600.0, b);

    let h = svg(&[
        rect(60.0, 0.0, 160.0, 700.0),
        rect(460.0, 0.0, 560.0, 700.0),
        rect(160.0, 300.0, 460.0, 400.0),
    ]);
    add_glyph(&mut font, "H", &['H' as u32], 620.0, h);

    let o_upper = svg(&[
        circle(350.0, 350.0, 300.0),
        circle_counter(350.0, 350.0, 200.0),
    ]);
    add_glyph(&mut font, "O", &['O' as u32], 700.0, o_upper);

    let n = svg(&[
        rect(60.0, 0.0, 140.0, 500.0),
        rect(140.0, 420.0, 440.0, 500.0),
        rect(360.0, 0.0, 440.0, 420.0),
    ]);
    add_glyph(&mut font, "n", &['n' as u32], 500.0, n);

    let o_lower = svg(&[
        circle(260.0, 250.0, 250.0),
        circle_counter(260.0, 250.0, 170.0),
    ]);
    add_glyph(&mut font, "o", &['o' as u32], 520.0, o_lower);

    font
}
