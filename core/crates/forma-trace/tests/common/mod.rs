//! Shared helpers for the specimen tests: the ground-truth outlines and the
//! deterministic generator of the specimen image.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use forma_geometry::{CoordinateSystem, paths_from_svg};
use forma_model::{Font, Glyph, GlyphId, Layer, Path, Transform};
use forma_render::coverage_mask;
use image::{ImageEncoder, Rgb, RgbImage};
use serde_json::Value;

pub const WIDTH: u32 = 1600;
pub const HEIGHT: u32 = 700;
pub const BACKGROUND: [u8; 3] = [0x11, 0x11, 0x11];
pub const INK: [u8; 3] = [0xFF, 0x5A, 0x1F];
pub const BLUE: [u8; 3] = [0x2A, 0x5B, 0xFF];
/// Pixels per font unit: cap height 700 is 112 px.
pub const SCALE: f64 = 0.16;
pub const CAP_HEIGHT: f64 = 700.0;
pub const UPM: u16 = 1000;

/// Letters in image order: row 0, then row 1.
pub const ORDER: [&str; 8] = ["A", "B", "C", "H", "O", "F", "M", "R"];
/// Baselines of the two rows and the x of the four columns, in pixels.
const BASELINES: [f64; 2] = [230.0, 450.0];
const COLUMNS: [f64; 4] = [120.0, 470.0, 820.0, 1170.0];

/// A true glyph: name, advance width and outline in font units.
pub struct TruthGlyph {
    pub name: String,
    pub advance_width: f64,
    pub paths: Vec<Path>,
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/hito2")
}

pub fn specimen_path() -> PathBuf {
    fixture_dir().join("specimen.png")
}

/// The true glyphs of `truth.json`, in image order.
pub fn truth_glyphs() -> Vec<TruthGlyph> {
    let text = std::fs::read_to_string(fixture_dir().join("truth.json")).expect("read truth.json");
    let json: Value = serde_json::from_str(&text).expect("parse truth.json");
    let list = json["glyphs"].as_array().expect("glyphs array");
    ORDER
        .iter()
        .map(|name| {
            let g = list
                .iter()
                .find(|g| g["name"] == *name)
                .expect("glyph in truth.json");
            TruthGlyph {
                name: (*name).to_owned(),
                advance_width: g["advance_width"].as_f64().expect("advance_width"),
                paths: paths_from_svg(
                    g["svg_path"].as_str().expect("svg_path"),
                    CoordinateSystem::Font,
                )
                .expect("valid svg_path"),
            }
        })
        .collect()
}

/// The ground truth as a one-master font (UPM 1000, cap height 700).
pub fn truth_font() -> Font {
    let mut font = Font::new("Forma Specimen", UPM);
    let master = font.masters[0].id;
    let mut glyphs = BTreeMap::new();
    let mut order = Vec::new();
    for t in truth_glyphs() {
        let id = GlyphId::new();
        let layer = Layer {
            advance_width: t.advance_width,
            paths: t.paths,
            components: Vec::new(),
        };
        let unicode = t.name.chars().next().map_or(0, u32::from);
        glyphs.insert(
            id,
            Arc::new(Glyph {
                id,
                name: t.name,
                unicodes: vec![unicode],
                export: true,
                layers: BTreeMap::from([(master, layer)]),
            }),
        );
        order.push(id);
    }
    font.glyphs = Arc::new(glyphs);
    font.glyph_order = order;
    font
}

fn blend(img: &mut RgbImage, coverage: &[u8], colour: [u8; 3]) {
    for (px, &c) in img.pixels_mut().zip(coverage) {
        if c == 0 {
            continue;
        }
        let a = u32::from(c);
        for (k, &new) in colour.iter().enumerate() {
            let (old, new) = (u32::from(px.0[k]), u32::from(new));
            // Linear blend with round-to-nearest, in integers.
            px.0[k] = ((old * (255 - a) + new * a + 127) / 255) as u8;
        }
    }
}

fn circle(cx: f64, cy: f64, r: f64) -> Vec<Path> {
    // Four cubic arcs (kappa), in pixel coordinates drawn with an identity
    // y-flip below.
    let k = 0.552_284_749_830_793_4 * r;
    let d = format!(
        "M{} {} C{} {} {} {} {} {} C{} {} {} {} {} {} C{} {} {} {} {} {} C{} {} {} {} {} {} Z",
        cx + r,
        cy,
        cx + r,
        cy + k,
        cx + k,
        cy + r,
        cx,
        cy + r,
        cx - k,
        cy + r,
        cx - r,
        cy + k,
        cx - r,
        cy,
        cx - r,
        cy - k,
        cx - k,
        cy - r,
        cx,
        cy - r,
        cx + k,
        cy - r,
        cx + r,
        cy - k,
        cx + r,
        cy,
    );
    paths_from_svg(&d, CoordinateSystem::Font).expect("circle path")
}

/// Renders the specimen: eight letters in two rows of four and a blue disc,
/// anti-aliased with the FORMA rasterizer. Deterministic: integer blending of
/// the coverage into the colours, single-threaded rasterizer.
pub fn render_specimen(truth: &[TruthGlyph]) -> RgbImage {
    let mut img = RgbImage::from_pixel(WIDTH, HEIGHT, Rgb(BACKGROUND));
    for (i, glyph) in truth.iter().enumerate() {
        let (row, col) = (i / 4, i % 4);
        let to_pixels = Transform {
            xx: SCALE,
            xy: 0.0,
            yx: 0.0,
            yy: -SCALE,
            dx: COLUMNS[col],
            dy: BASELINES[row],
        };
        let mask = coverage_mask(&glyph.paths, &to_pixels, WIDTH, HEIGHT).expect("letter mask");
        blend(&mut img, &mask.data, INK);
    }
    // Bottom-right area, clear of the letters.
    let identity = Transform {
        xx: 1.0,
        xy: 0.0,
        yx: 0.0,
        yy: 1.0,
        dx: 0.0,
        dy: 0.0,
    };
    let disc =
        coverage_mask(&circle(1480.0, 600.0, 60.0), &identity, WIDTH, HEIGHT).expect("disc mask");
    blend(&mut img, &disc.data, BLUE);
    img
}

/// The PNG encoding of `img` (default encoder settings, no metadata).
pub fn encode_png(img: &RgbImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .expect("encode png");
    bytes
}
