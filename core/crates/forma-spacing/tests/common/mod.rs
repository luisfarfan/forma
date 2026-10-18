//! Shared helpers: the Milestone 1 fixture font and small glyph builders.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path as FsPath;
use std::sync::Arc;

use forma_geometry::{CoordinateSystem, paths_from_svg, transform_paths};
use forma_model::{Component, Font, Glyph, GlyphId, Layer, MasterId, Path, Transform};
use serde_json::Value;

/// The fixture font of Milestone 1 (`tests/fixtures/hito1/glyphs.json`):
/// UPM 1000, cap height 700, one master.
pub fn fixture_font() -> Font {
    let path =
        FsPath::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/hito1/glyphs.json");
    let text = std::fs::read_to_string(&path).expect("fixture is readable");
    let json: Value = serde_json::from_str(&text).expect("fixture is JSON");
    let upm = u16::try_from(json["units_per_em"].as_u64().expect("upm")).expect("upm fits");
    let mut font = Font::new("Fixture", upm);
    let metrics = &json["vertical_metrics"];
    font.masters[0].metrics.cap_height = metrics["cap_height"].as_f64().expect("cap");
    font.masters[0].metrics.x_height = metrics["x_height"].as_f64().expect("x");
    for glyph in json["glyphs"].as_array().expect("glyphs") {
        let name = glyph["name"].as_str().expect("name");
        let advance = glyph["advance_width"].as_f64().expect("advance");
        let paths = match glyph["svg_path"].as_str() {
            Some(d) => paths_from_svg(d, CoordinateSystem::Font).expect("svg"),
            None => Vec::new(),
        };
        add_glyph(&mut font, name, advance, paths, Vec::new());
    }
    font
}

pub fn master(font: &Font) -> MasterId {
    font.masters[0].id
}

pub fn id(font: &Font, name: &str) -> GlyphId {
    font.glyph_by_name(name).expect("glyph exists").id
}

pub fn add_glyph(
    font: &mut Font,
    name: &str,
    advance: f64,
    paths: Vec<Path>,
    components: Vec<Component>,
) -> GlyphId {
    let id = GlyphId::new();
    let mut layers = BTreeMap::new();
    layers.insert(
        font.masters[0].id,
        Layer {
            advance_width: advance,
            paths,
            components,
        },
    );
    let glyph = Glyph {
        id,
        name: name.to_owned(),
        unicodes: Vec::new(),
        export: true,
        layers,
    };
    Arc::make_mut(&mut font.glyphs).insert(id, Arc::new(glyph));
    font.glyph_order.push(id);
    id
}

pub fn add_svg_glyph(font: &mut Font, name: &str, advance: f64, d: &str) -> GlyphId {
    let paths = paths_from_svg(d, CoordinateSystem::Font).expect("svg");
    add_glyph(font, name, advance, paths, Vec::new())
}

/// Moves the outline of `glyph` horizontally by `dx` and sets its advance.
pub fn set_horizontal(font: &mut Font, glyph: GlyphId, dx: f64, advance: f64) {
    let master = master(font);
    let glyphs = Arc::make_mut(&mut font.glyphs);
    let glyph = Arc::make_mut(glyphs.get_mut(&glyph).expect("glyph"));
    let layer = glyph.layers.get_mut(&master).expect("layer");
    let shift = Transform {
        dx,
        ..Transform::IDENTITY
    };
    layer.paths = transform_paths(&layer.paths, &shift);
    for component in &mut layer.components {
        component.transform = component.transform.then(&shift);
    }
    layer.advance_width = advance;
}

pub fn remove_glyph(font: &mut Font, name: &str) {
    let glyph = id(font, name);
    Arc::make_mut(&mut font.glyphs).remove(&glyph);
    font.glyph_order.retain(|g| *g != glyph);
}
