//! Style synthesis on a font of stroked letters standing for traced ones.

use std::collections::BTreeMap;
use std::sync::Arc;

use forma_geometry::{Cap, Skeleton, SkeletonNode, StrokeStyle, bbox, stroke};
use forma_model::{Font, Glyph, GlyphId, Layer, NodeKind, Provenance, ProvenanceDetail, Source};
use forma_synth::{
    Class, MIN_STYLE_SCORE, Outcome, RecipeBook, StyleKit, StyleScore, SynthError, UNICASE_ES,
    score, synthesize,
};

const STROKE: f64 = 90.0;

/// A skeleton from points on a 0..1 box of `w` by 700 units, inset by half
/// the stroke; `None` separates strokes.
fn lines(w: f64, points: &[(f64, f64)]) -> Skeleton {
    let h = STROKE / 2.0;
    Skeleton {
        nodes: points
            .iter()
            .map(|&(x, y)| SkeletonNode {
                x: h + x * (w - STROKE),
                y: h + y * (700.0 - STROKE),
                kind: NodeKind::Line,
                width: None,
            })
            .collect(),
        closed: false,
    }
}

/// A closed ellipse skeleton in a box of `w` by 700 units.
fn ellipse(w: f64) -> Skeleton {
    let (rx, ry) = ((w - STROKE) / 2.0, (700.0 - STROKE) / 2.0);
    let (cx, cy) = (w / 2.0, 350.0);
    let k = 0.552_284_75;
    let n = |x: f64, y: f64, kind| SkeletonNode {
        x,
        y,
        kind,
        width: None,
    };
    Skeleton {
        nodes: vec![
            n(cx + rx, cy, NodeKind::Curve),
            n(cx + rx, cy + k * ry, NodeKind::OffCurve),
            n(cx + k * rx, cy + ry, NodeKind::OffCurve),
            n(cx, cy + ry, NodeKind::Curve),
            n(cx - k * rx, cy + ry, NodeKind::OffCurve),
            n(cx - rx, cy + k * ry, NodeKind::OffCurve),
            n(cx - rx, cy, NodeKind::Curve),
            n(cx - rx, cy - k * ry, NodeKind::OffCurve),
            n(cx - k * rx, cy - ry, NodeKind::OffCurve),
            n(cx, cy - ry, NodeKind::Curve),
            n(cx + k * rx, cy - ry, NodeKind::OffCurve),
            n(cx + rx, cy - k * ry, NodeKind::OffCurve),
        ],
        closed: true,
    }
}

fn add(font: &mut Font, name: &str, skeletons: &[Skeleton], width: f64, traced: bool) {
    add_with(font, name, skeletons, width, traced, Cap::Round);
}

fn add_with(
    font: &mut Font,
    name: &str,
    skeletons: &[Skeleton],
    width: f64,
    traced: bool,
    cap: Cap,
) {
    let master = font.default_master().id;
    let mut paths = stroke(skeletons, &StrokeStyle { width: STROKE, cap }).unwrap_or_default();
    for (k, p) in paths.iter_mut().enumerate() {
        p.id = forma_model::PathId(k as u32);
    }
    let id = GlyphId::new();
    let provenance = traced.then(|| Provenance::pending(Source::Traced, 1, ProvenanceDetail::None));
    Arc::make_mut(&mut font.glyphs).insert(
        id,
        Arc::new(Glyph {
            id,
            name: name.to_owned(),
            unicodes: name.chars().map(u32::from).collect(),
            export: true,
            layers: BTreeMap::from([(
                master,
                Layer {
                    advance_width: width + 100.0,
                    paths: forma_geometry::transform_paths(
                        &paths,
                        &forma_model::Transform {
                            dx: 50.0,
                            ..forma_model::Transform::IDENTITY
                        },
                    ),
                    components: Vec::new(),
                },
            )]),
            provenance,
        }),
    );
    font.glyph_order.push(id);
}

/// A monoline font: H, I, L, E, N, V, O, C and S as stroked skeletons.
fn monoline() -> Font {
    monoline_with(Cap::Round)
}

fn monoline_with(cap: Cap) -> Font {
    let mut font = Font::new("Monoline", 1000);
    let add = |font: &mut Font, name: &str, sk: &[Skeleton], w: f64, traced: bool| {
        add_with(font, name, sk, w, traced, cap);
    };
    add(
        &mut font,
        "H",
        &[
            lines(560.0, &[(0.0, 0.0), (0.0, 1.0)]),
            lines(560.0, &[(1.0, 0.0), (1.0, 1.0)]),
            lines(560.0, &[(0.0, 0.5), (1.0, 0.5)]),
        ],
        560.0,
        true,
    );
    add(
        &mut font,
        "I",
        &[lines(90.0, &[(0.5, 0.0), (0.5, 1.0)])],
        90.0,
        true,
    );
    add(
        &mut font,
        "L",
        &[lines(440.0, &[(0.0, 1.0), (0.0, 0.0), (1.0, 0.0)])],
        440.0,
        true,
    );
    add(
        &mut font,
        "E",
        &[
            lines(460.0, &[(1.0, 1.0), (0.0, 1.0), (0.0, 0.0), (1.0, 0.0)]),
            lines(460.0, &[(0.0, 0.5), (0.85, 0.5)]),
        ],
        460.0,
        true,
    );
    add(
        &mut font,
        "N",
        &[lines(
            560.0,
            &[(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0)],
        )],
        560.0,
        true,
    );
    add(
        &mut font,
        "V",
        &[lines(600.0, &[(0.0, 1.0), (0.5, 0.0), (1.0, 1.0)])],
        600.0,
        true,
    );
    add(&mut font, "O", &[ellipse(660.0)], 660.0, true);
    add(
        &mut font,
        "S",
        &[lines(
            500.0,
            &[
                (1.0, 0.9),
                (0.7, 1.0),
                (0.2, 1.0),
                (0.0, 0.8),
                (0.15, 0.58),
                (0.85, 0.42),
                (1.0, 0.2),
                (0.8, 0.0),
                (0.3, 0.0),
                (0.0, 0.1),
            ],
        )],
        500.0,
        true,
    );
    font
}

fn book() -> RecipeBook {
    RecipeBook::parse(UNICASE_ES).unwrap_or_else(|e| panic!("recipes: {e}"))
}

fn all_names(book: &RecipeBook) -> Vec<String> {
    book.glyphs.iter().map(|r| r.name.clone()).collect()
}

#[test]
fn recipes_parse_and_reuse_only_earlier_glyphs() {
    let book = book();
    assert_eq!(book.version, 1);
    assert!(book.glyphs.len() >= 50, "{}", book.glyphs.len());
    let bad = RecipeBook::parse(
        r#"{"version":1,"glyphs":[
          {"name":"b","class":"punct","width":0.3,"strategies":[{"id":"d","parts":[{"glyph":"a"}]}]},
          {"name":"a","class":"punct","width":0.3,"strategies":[{"id":"s","dots":[{"at":[0.5,0]}]}]}]}"#,
    );
    assert!(matches!(bad, Err(SynthError::Recipes(_))), "{bad:?}");
}

#[test]
fn kit_measures_the_traced_glyphs() {
    let font = monoline();
    let master = font.default_master().id;
    let kit = StyleKit::extract(&font, master, None).unwrap_or_else(|e| panic!("{e}"));
    assert!((kit.stroke - STROKE).abs() < 15.0, "stroke {}", kit.stroke);
    assert_eq!(kit.ball, None, "round ends are not balls");
    assert_eq!(kit.cap_height, 700.0);
    assert_eq!(kit.sources.len(), 8);
    assert!(kit.widths.round.is_some() && kit.widths.straight.is_some());
}

#[test]
fn sparse_font_yields_needs_design() {
    let mut font = Font::new("Sparse", 1000);
    add(
        &mut font,
        "I",
        &[lines(90.0, &[(0.5, 0.0), (0.5, 1.0)])],
        90.0,
        true,
    );
    // A glyph drawn by hand does not teach the style.
    add(
        &mut font,
        "L",
        &[lines(440.0, &[(0.0, 1.0), (0.0, 0.0), (1.0, 0.0)])],
        440.0,
        false,
    );
    let master = font.default_master().id;
    let kit = StyleKit::extract(&font, master, None);
    assert!(matches!(kit, Err(SynthError::NoSources(_))), "{kit:?}");
}

#[test]
fn figures_punctuation_and_marks_are_made_in_the_style() {
    let font = monoline();
    let master = font.default_master().id;
    let kit = StyleKit::extract(&font, master, None).unwrap_or_else(|e| panic!("{e}"));
    let book = book();
    let out = synthesize(&font, &kit, &book, &all_names(&book), MIN_STYLE_SCORE)
        .unwrap_or_else(|e| panic!("{e}"));
    let made: BTreeMap<&str, &forma_synth::Made> = out
        .iter()
        .filter_map(|o| match o {
            Outcome::Made(m) => Some((m.name.as_str(), m)),
            _ => None,
        })
        .collect();
    let refused: Vec<String> = out
        .iter()
        .filter_map(|o| match o {
            Outcome::NeedsDesign { name, reason, .. } => Some(format!("{name}: {reason}")),
            _ => None,
        })
        .collect();
    for name in [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "period",
        "comma",
        "colon",
        "semicolon",
        "exclam",
        "question",
        "hyphen",
        "parenleft",
        "parenright",
        "slash",
        "dollar",
        "acutecomb",
        "dieresiscomb",
        "tildecomb",
        "space",
    ] {
        assert!(
            made.contains_key(name),
            "{name} not made; refused: {refused:#?}"
        );
    }
    for name in ["ampersand", "at", "Euro"] {
        assert!(
            refused.iter().any(|r| r.starts_with(&format!("{name}:"))),
            "{name} should need a design"
        );
    }
    for m in made.values() {
        assert!(
            m.score.accepts(MIN_STYLE_SCORE),
            "{}: {:?}",
            m.name,
            m.score
        );
        if m.class == Class::Space {
            assert!(m.paths.is_empty() && m.advance_width > 0.0);
            continue;
        }
        let b = bbox(&m.paths).unwrap_or_else(|| panic!("{} has no ink", m.name));
        if m.class == Class::Mark {
            assert_eq!(m.advance_width, 0.0);
            assert!(b.y0 > 700.0, "{} sits above the cap height: {b:?}", m.name);
            assert!((b.x0 + b.x1).abs() <= 2.0, "{} is centred: {b:?}", m.name);
        } else {
            assert!(
                m.advance_width > b.x1,
                "{}: advance {} ink {b:?}",
                m.name,
                m.advance_width
            );
        }
        for p in &m.paths {
            for n in &p.nodes {
                assert!(
                    n.x == n.x.round() && n.y == n.y.round(),
                    "{} is on the grid",
                    m.name
                );
            }
        }
    }
    // Pieces: the semicolon reuses the comma and the period made before it.
    assert_eq!(made["semicolon"].sources, vec!["comma", "period"]);
    assert_eq!(made["dollar"].sources, vec!["S"]);
    // Squeezed to a figure's width, this wide O's stems would be too thin:
    // the zero is drawn from its skeleton instead.
    assert_eq!(made["zero"].strategy, "skeleton");
    // Figures are as tall as the capitals.
    let four = bbox(&made["four"].paths).unwrap_or_else(|| panic!("four"));
    assert!(
        (four.y1 - 700.0).abs() < 2.0 && four.y0.abs() < 2.0,
        "{four:?}"
    );
}

#[test]
fn synthesis_is_deterministic_and_skips_existing_glyphs() {
    let font = monoline();
    let master = font.default_master().id;
    let kit = StyleKit::extract(&font, master, None).unwrap_or_else(|e| panic!("{e}"));
    let book = book();
    let names = vec!["O".to_owned(), "two".to_owned(), "semicolon".to_owned()];
    let a = synthesize(&font, &kit, &book, &names, MIN_STYLE_SCORE).unwrap_or_default();
    let b = synthesize(&font, &kit, &book, &names, MIN_STYLE_SCORE).unwrap_or_default();
    assert_eq!(a, b);
    // O exists; the semicolon's pieces were not asked for and do not exist.
    let named: Vec<&str> = a.iter().map(Outcome::name).collect();
    assert_eq!(named, vec!["two", "semicolon"]);
    assert!(matches!(&a[1], Outcome::NeedsDesign { reason, .. } if reason.contains("comma")));
}

#[test]
fn a_glyph_foreign_to_the_style_is_refused() {
    let font = monoline();
    let master = font.default_master().id;
    let kit = StyleKit::extract(&font, master, None).unwrap_or_else(|e| panic!("{e}"));
    let book = book();
    // Dressed with a stroke 2.5 times the font's, a figure is too dark for
    // the font even by its own measure...
    let mut heavy = kit.clone();
    heavy.stroke *= 2.5;
    heavy.thin *= 2.5;
    heavy.thick *= 2.5;
    let out = synthesize(&font, &heavy, &book, &["seven".to_owned()], MIN_STYLE_SCORE)
        .unwrap_or_default();
    assert!(
        matches!(out.first(), Some(Outcome::NeedsDesign { reason, .. }) if reason.contains("darkness")),
        "{out:?}"
    );
    // ...and a slash, measured against the font's stroke, does not belong.
    let out = synthesize(&font, &heavy, &book, &["slash".to_owned()], MIN_STYLE_SCORE)
        .unwrap_or_default();
    let Some(Outcome::Made(m)) = out.first() else {
        panic!("{out:?}");
    };
    let recipe = book.get("slash").unwrap_or_else(|| panic!("slash"));
    let s = score(&font, &kit, recipe, &recipe.strategies[0], &m.paths);
    assert!(!s.accepts(MIN_STYLE_SCORE), "{s:?}");
    assert!(s.stroke.is_some_and(|c| c < 0.5), "{s:?}");
    assert!(!s.reasons.is_empty());
}

#[test]
fn zero_derives_from_a_narrow_o() {
    let mut font = monoline();
    let master = font.default_master().id;
    let wide = font.glyph_by_name("O").map(|g| g.id);
    if let Some(id) = wide {
        Arc::make_mut(&mut font.glyphs).remove(&id);
        font.glyph_order.retain(|g| *g != id);
    }
    add(&mut font, "O", &[ellipse(470.0)], 470.0, true);
    let kit = StyleKit::extract(&font, master, None).unwrap_or_else(|e| panic!("{e}"));
    let out =
        synthesize(&font, &kit, &book(), &["zero".to_owned()], MIN_STYLE_SCORE).unwrap_or_default();
    match out.first() {
        Some(Outcome::Made(m)) => {
            assert_eq!(m.strategy, "derive:O");
            assert_eq!(m.sources, vec!["O"]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn flat_ends_are_kept_and_round_ones_refused() {
    let font = monoline_with(Cap::Flat);
    let master = font.default_master().id;
    let kit = StyleKit::extract(&font, master, None).unwrap_or_else(|e| panic!("{e}"));
    assert!(kit.flat_ends(), "{kit:?}");
    let book = book();
    let names: Vec<String> = ["seven", "two", "hyphen", "slash", "parenleft"]
        .iter()
        .map(|n| (*n).to_owned())
        .collect();
    let out = synthesize(&font, &kit, &book, &names, MIN_STYLE_SCORE).unwrap_or_default();
    for o in &out {
        assert!(matches!(o, Outcome::Made(_)), "{o:?}");
    }
    // The same glyph with round ends does not belong to this font.
    let round = monoline_with(Cap::Round);
    let round_kit =
        StyleKit::extract(&round, master_of(&round), None).unwrap_or_else(|e| panic!("{e}"));
    let made = synthesize(
        &round,
        &round_kit,
        &book,
        &["seven".to_owned()],
        MIN_STYLE_SCORE,
    )
    .unwrap_or_default();
    let Some(Outcome::Made(m)) = made.first() else {
        panic!("{made:?}");
    };
    let recipe = book.get("seven").unwrap_or_else(|| panic!("seven"));
    let s = score(&font, &kit, recipe, &recipe.strategies[0], &m.paths);
    assert!(!s.accepts(MIN_STYLE_SCORE), "{s:?}");
    assert!(s.reasons.iter().any(|r| r.contains("square")), "{s:?}");
}

#[test]
fn ball_terminals_go_where_arms_have_room() {
    let font = monoline_with(Cap::Bulb { ratio: 1.6 });
    let master = font.default_master().id;
    let kit = StyleKit::extract(&font, master, None).unwrap_or_else(|e| panic!("{e}"));
    assert!(kit.ball.is_some_and(|b| b > 1.3), "{kit:?}");
    assert!(kit.contrast().is_none(), "balls are not contrast: {kit:?}");
    let book = book();
    let out = synthesize(&font, &kit, &book, &all_names(&book), MIN_STYLE_SCORE)
        .unwrap_or_else(|e| panic!("{e}"));
    let made: Vec<&forma_synth::Made> = out
        .iter()
        .filter_map(|o| match o {
            Outcome::Made(m) => Some(m),
            _ => None,
        })
        .collect();
    assert!(made.len() > 20, "{out:#?}");
    for m in &made {
        assert_eq!(
            m.score.structure,
            if m.class == Class::Space {
                None
            } else {
                Some(1.0)
            },
            "{}",
            m.name
        );
    }
    // The hyphen is a bone: balls at both ends, taller than its stroke.
    let hyphen = made
        .iter()
        .find(|m| m.name == "hyphen")
        .unwrap_or_else(|| panic!("hyphen: {out:#?}"));
    let b = bbox(&hyphen.paths).unwrap_or_else(|| panic!("ink"));
    assert!(b.height() > 1.3 * STROKE, "{b:?}");
}

fn master_of(font: &Font) -> forma_model::MasterId {
    font.default_master().id
}

#[test]
fn single_bad_component_rejects() {
    let s = StyleScore {
        total: 0.9,
        stroke: Some(0.98),
        terminal: Some(0.45),
        color: None,
        proportion: Some(1.0),
        structure: Some(1.0),
        reasons: vec!["swollen ends".into()],
    };
    assert!(!s.accepts(MIN_STYLE_SCORE));
    let ok = StyleScore {
        terminal: Some(0.6),
        ..s.clone()
    };
    assert!(ok.accepts(MIN_STYLE_SCORE));
    let low = StyleScore {
        total: 0.7,
        ..ok.clone()
    };
    assert!(!low.accepts(MIN_STYLE_SCORE));
    // A glyph whose strokes merge is refused whatever its style.
    let merged = StyleScore {
        structure: Some(0.0),
        ..ok
    };
    assert!(!merged.accepts(MIN_STYLE_SCORE));
}
