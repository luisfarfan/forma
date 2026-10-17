//! Format v2 (components and kerning) and the migration of v1 documents.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use forma_model::{Font, FontId, Glyph, GlyphId, Layer, MasterId, Node, NodeKind};
use forma_model::{Path as Contour, PathId};
use uuid::Uuid;

fn fixtures_v1() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1")
}

fn corner(x: f64, y: f64) -> Node {
    Node {
        x,
        y,
        kind: NodeKind::Line,
        smooth: false,
    }
}

fn layer(advance_width: f64, size: f64) -> Layer {
    Layer {
        advance_width,
        paths: vec![Contour {
            id: PathId(1),
            closed: true,
            nodes: vec![
                corner(50.0, 0.0),
                corner(50.0 + size, 0.0),
                corner(50.0 + size, size),
                corner(50.0, size),
            ],
        }],
        components: Vec::new(),
    }
}

/// A fixed three-glyph font, as format v1 stored it (no components, no
/// kerning). Every id is fixed so that the fixtures are reproducible.
fn sample_v1_font() -> Font {
    let master = MasterId(Uuid::from_u128(0x10));
    let mut font = Font::new("Fixture", 1000);
    font.id = FontId(Uuid::from_u128(0x1));
    font.masters[0].id = master;
    let mut glyphs = BTreeMap::new();
    let mut order = Vec::new();
    for (n, (name, unicode, size)) in [
        ("A", 0x41u32, 600.0),
        ("B", 0x42, 550.0),
        ("space", 0x20, 0.0),
    ]
    .into_iter()
    .enumerate()
    {
        let id = GlyphId(Uuid::from_u128(0x100 + n as u128));
        let mut layers = BTreeMap::new();
        let layer = if size == 0.0 {
            Layer {
                advance_width: 250.0,
                paths: Vec::new(),
                components: Vec::new(),
            }
        } else {
            layer(700.0, size)
        };
        layers.insert(master, layer);
        glyphs.insert(
            id,
            Arc::new(Glyph {
                id,
                name: name.to_owned(),
                unicodes: vec![unicode],
                export: true,
                layers,
            }),
        );
        order.push(id);
    }
    font.glyphs = Arc::new(glyphs);
    font.glyph_order = order;
    font
}

/// Writes the v1 fixtures. Run once, while the format was still version 1:
/// `cargo test -p forma-store --test migration -- --ignored generate_v1_fixtures`.
/// The output is versioned and must keep loading forever.
#[test]
#[ignore = "generates the committed v1 fixtures; only meaningful with FORMAT_VERSION 1"]
fn generate_v1_fixtures() {
    let dir = fixtures_v1();
    std::fs::create_dir_all(&dir).unwrap();
    let font = sample_v1_font();
    forma_store::save(&font, &dir.join("doc.forma")).unwrap();
    forma_store::save(&font, &dir.join("doc.formapackage")).unwrap();
}
