//! Draws recipes in the style of a document and prints their scores and
//! outlines as SVG: `cargo run -p forma-synth --example probe -- doc.forma name...`.

use forma_synth::{RecipeBook, StyleKit, UNICASE_ES, draw, score};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((path, names)) = args.split_first() else {
        eprintln!("usage: probe doc.forma name...");
        return;
    };
    let font = forma_store::load(std::path::Path::new(path)).expect("load");
    let master = font.default_master().id;
    let kit = StyleKit::extract(&font, master, None).expect("kit");
    let book = RecipeBook::parse(UNICASE_ES).expect("recipes");
    for name in names {
        let Some(recipe) = book.get(name) else {
            continue;
        };
        for strategy in &recipe.strategies {
            let Ok(Some((paths, advance))) = draw(&font, &kit, recipe, strategy) else {
                println!("{name} {}: not drawn", strategy.id);
                continue;
            };
            let s = score(&font, &kit, recipe, strategy, &paths);
            let m = {
                let mut f = font.clone();
                let id = forma_model::GlyphId::new();
                std::sync::Arc::make_mut(&mut f.glyphs).insert(
                    id,
                    std::sync::Arc::new(forma_model::Glyph {
                        id,
                        name: "probe".into(),
                        unicodes: vec![],
                        export: false,
                        layers: [(
                            master,
                            forma_model::Layer {
                                advance_width: advance,
                                paths: paths.clone(),
                                components: vec![],
                            },
                        )]
                        .into(),
                        provenance: None,
                    }),
                );
                forma_trace::measure(&f, id, master).ok()
            };
            println!("{name} {}: {s:?}\n  measure {m:?}", strategy.id);
            let d: Vec<String> = paths.iter().map(forma_geometry::path_to_svg).collect();
            println!("  <path d=\"{}\"/>", d.join(" "));
        }
    }
}
