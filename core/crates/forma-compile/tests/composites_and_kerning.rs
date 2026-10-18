//! Components and kerning end to end: they flow through the UFO into `glyf`
//! composites and `GPOS`.

mod common;

use common::{add_glyph, demo_font, edit_glyph, rect, svg};
use forma_compile::{CompileError, OutputFormat, compile, preflight};
use forma_model::{Component, Font, KernRef, Transform};
use read_fonts::tables::glyf::Glyph as TtGlyph;
use read_fonts::types::{GlyphId, GlyphId16, Tag};
use read_fonts::{FontRef, TableProvider};

/// The demo font plus an `acute` mark and an `Aacute` built from `A` and the
/// mark, and a kerning pair `O`/`A`.
fn font_with_composite(acute_exported: bool) -> Font {
    let mut font = demo_font();
    let mark = svg(&[rect(0.0, 0.0, 100.0, 100.0)]);
    let acute = add_glyph(&mut font, "acute", &[], 300.0, mark);
    edit_glyph(&mut font, acute, |glyph| glyph.export = acute_exported);
    let a = font.glyph_by_name("A").expect("A").id;
    let o = font.glyph_by_name("O").expect("O").id;
    let aacute = add_glyph(&mut font, "Aacute", &[0xC1], 612.5, Vec::new());
    edit_glyph(&mut font, aacute, |glyph| {
        for layer in glyph.layers.values_mut() {
            layer.components = vec![
                Component {
                    base: a,
                    transform: Transform::default(),
                },
                Component {
                    base: acute,
                    transform: Transform {
                        dx: 200.0,
                        dy: 750.0,
                        ..Transform::default()
                    },
                },
            ];
        }
    });
    let master = font.default_master().id;
    font.kerning
        .set_pair(master, KernRef::Glyph(o), KernRef::Glyph(a), Some(-40.0));
    font
}

fn glyph_index(font: &FontRef, name: &str) -> u16 {
    let post = font.post().expect("post");
    (0..font.maxp().expect("maxp").num_glyphs())
        .find(|&gid| post.glyph_name(GlyphId16::new(gid)) == Some(name))
        .unwrap_or_else(|| panic!("no glyph {name}"))
}

fn glyph_of<'a>(font: &FontRef<'a>, name: &str) -> Option<TtGlyph<'a>> {
    let gid = GlyphId::new(u32::from(glyph_index(font, name)));
    let loca = font.loca(None).expect("loca");
    let glyf = font.glyf().expect("glyf");
    loca.get_glyf(gid, &glyf).expect("readable glyph")
}

#[test]
fn compiled_font_has_composite_glyphs_and_gpos() {
    let font = font_with_composite(true);
    let (binary, _) = compile(&font, OutputFormat::Ttf).expect("compiles");
    let compiled = FontRef::new(&binary).expect("parses");
    assert!(compiled.table_data(Tag::new(b"GPOS")).is_some(), "GPOS");
    let aacute = glyph_of(&compiled, "Aacute").expect("Aacute has an outline");
    assert!(
        matches!(aacute, TtGlyph::Composite(_)),
        "Aacute is composite"
    );
    assert!(matches!(
        glyph_of(&compiled, "A").expect("A"),
        TtGlyph::Simple(_)
    ));
}

#[test]
fn unexported_base_is_decomposed_in_the_binary() {
    let font = font_with_composite(false);
    let (binary, _) = compile(&font, OutputFormat::Ttf).expect("compiles");
    let compiled = FontRef::new(&binary).expect("parses");
    // Without the mark glyph, the composite cannot be one with two parts of
    // which one is missing: the mark is drawn into the glyph.
    let post = compiled.post().expect("post");
    let names: Vec<_> = (0..compiled.maxp().expect("maxp").num_glyphs())
        .filter_map(|g| post.glyph_name(GlyphId16::new(g)))
        .collect();
    assert!(!names.contains(&"acute"));
    assert!(glyph_of(&compiled, "Aacute").is_some());
}

#[test]
fn preflight_rejects_components_that_leave_the_coordinate_range() {
    let mut font = font_with_composite(true);
    let aacute = font.glyph_by_name("Aacute").expect("Aacute").id;
    edit_glyph(&mut font, aacute, |glyph| {
        for layer in glyph.layers.values_mut() {
            layer.components[1].transform.dx = 40_000.0;
        }
    });
    let error = preflight(&font).expect_err("out of range");
    assert!(matches!(error, CompileError::Preflight(_)));
    assert!(error.to_string().contains("components"), "{error}");
}
