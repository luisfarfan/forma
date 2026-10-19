//! Milestone 2, acceptance criteria 3, 4, 5 and 6: a complete `unicase-es`
//! test font (89 glyphs, 119 code points) built through `forma mcp`.
//!
//! The eight letters of the specimen image are traced; the other 18 letters,
//! the digits and the 41 punctuation marks and symbols are simple skeletons
//! drawn with `glyph_stroke` (the table below: shapes, not beauty); the three
//! marks are stroked too and the seven accented letters are components made
//! with `glyph_compose`. Spacing and kerning are automatic.
//!
//! When `FORMA_ACCEPTANCE_OUT` names a folder, the legacy-era run leaves
//! `unicase-es.ttf`, `unicase-es.woff2` and `unicase-expected.json` there for
//! the external oracles (`tools/oracle/verify_hito2.py`). Format of
//! `unicase-expected.json` (a superset of `hito2-expected.json`):
//! * `format`, `family`, `upm`;
//! * `glyphs`: `{name: {"unicodes": ["U+0041", ...], "advance": int}}`;
//! * `cmap`: `{"U+0041": "A", ...}`;
//! * `composites`: the names of the glyphs that are components in the TTF;
//! * `shaping`: `{"text", "glyphs"}`: the glyph names `text` shapes to;
//! * `kerning`: `[{"left", "right", "value"}]`, the effective glyph-to-glyph
//!   kerning (nonzero, integers) between every pair of encoded glyphs.

mod mcp_common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use mcp_common::{
    Client, ModelGlyph, assert_composites_follow_bases, call, close, connect, copy_specimen,
    effective_kern, is_legacy, kerning_maps, legacy, model_glyphs, modern, path_text, rounded,
    unicode_text, with_timeout_for,
};
use read_fonts::tables::glyf::Glyph;
use read_fonts::types::GlyphId;
use read_fonts::{FontRef, TableProvider};
use rmcp::ClientLifecycleMode;
use rstest::rstest;
use serde_json::{Value, json};

/// The letters traced from the specimen image, in candidate order.
const TRACED: [char; 8] = ['A', 'B', 'C', 'H', 'O', 'F', 'M', 'R'];

/// The stroke width of the drawn skeletons, in font units.
const WIDTH: f64 = 64.0;

/// Text the external shaping check sets (every character is in the font).
const SHAPING_TEXT: &str = "¿QUÉ PÍO ÑU? 2026";

/// The skeletons of the glyphs the specimen does not give: name, character
/// (its upper case form for letters), advance width and strokes. The strokes
/// are separated by `;`; a stroke is `x,y` points (a path of straight
/// segments, closed if it ends with `z`) or `e:cx,cy,rx,ry`, an ellipse. The
/// box is 0..500 wide and 0..700 high (the cap height); stroke ends are round.
#[rustfmt::skip]
const DRAWN: &[(&str, char, f64, &str)] = &[
    // Letters (the 18 the specimen lacks).
    ("D", 'D', 540.0, "60,30 60,670 220,670 440,520 440,180 220,30 z"),
    ("E", 'E', 500.0, "440,670 60,670 60,30 440,30; 60,350 340,350"),
    ("G", 'G', 540.0, "440,540 300,670 200,670 60,540 60,160 200,30 300,30 440,160 440,320 260,320"),
    ("I", 'I', 400.0, "200,30 200,670; 80,670 320,670; 80,30 320,30"),
    ("J", 'J', 500.0, "400,670 400,160 300,30 200,30 100,130"),
    ("K", 'K', 520.0, "60,30 60,670; 440,670 60,310; 180,430 440,30"),
    ("L", 'L', 480.0, "60,670 60,30 440,30"),
    ("N", 'N', 540.0, "60,30 60,670 440,30 440,670"),
    ("P", 'P', 520.0, "60,30 60,670 300,670 440,570 440,440 300,340 60,340"),
    ("Q", 'Q', 560.0, "e:250,350,190,320; 290,200 440,30"),
    ("S", 'S', 500.0, "440,580 340,670 160,670 60,580 60,470 160,370 340,330 440,230 440,120 340,30 160,30 60,120"),
    ("T", 'T', 500.0, "40,670 460,670; 250,670 250,30"),
    ("U", 'U', 540.0, "60,670 60,160 170,30 330,30 440,160 440,670"),
    ("V", 'V', 520.0, "50,670 250,30 450,670"),
    ("W", 'W', 600.0, "40,670 150,30 270,500 390,30 500,670"),
    ("X", 'X', 520.0, "60,670 440,30; 440,670 60,30"),
    ("Y", 'Y', 520.0, "50,670 250,350 450,670; 250,350 250,30"),
    ("Z", 'Z', 500.0, "60,670 440,670 60,30 440,30"),
    // Digits.
    ("zero", '0', 540.0, "e:250,350,190,320"),
    ("one", '1', 400.0, "130,540 280,670 280,30; 130,30 430,30"),
    ("two", '2', 500.0, "70,560 170,670 330,670 430,560 430,450 70,30 440,30"),
    ("three", '3', 500.0, "80,670 400,670 250,400 360,370 440,250 440,140 340,30 160,30 60,120"),
    ("four", '4', 520.0, "340,30 340,670 40,200 460,200"),
    ("five", '5', 500.0, "420,670 90,670 70,380 200,420 340,420 440,320 440,140 340,30 160,30 60,110"),
    ("six", '6', 500.0, "400,640 300,670 200,670 80,540 60,300 60,160 160,30 340,30 440,160 440,260 340,380 160,380 60,300"),
    ("seven", '7', 480.0, "60,670 440,670 200,30"),
    ("eight", '8', 520.0, "e:250,520,150,150; e:250,190,190,160"),
    ("nine", '9', 500.0, "100,60 200,30 300,30 420,160 440,400 440,540 340,670 160,670 60,540 60,440 160,320 340,320 440,400"),
    // Punctuation and symbols.
    ("period", '.', 300.0, "150,30 150,50"),
    ("comma", ',', 300.0, "170,50 110,-90"),
    ("colon", ':', 300.0, "150,30 150,50; 150,380 150,400"),
    ("semicolon", ';', 300.0, "170,50 110,-90; 150,380 150,400"),
    ("exclam", '!', 300.0, "150,670 150,230; 150,30 150,50"),
    ("exclamdown", '¡', 300.0, "150,30 150,470; 150,640 150,660"),
    ("question", '?', 480.0, "100,580 180,670 320,670 410,580 410,480 250,340 250,230; 250,30 250,50"),
    ("questiondown", '¿', 480.0, "100,120 180,30 320,30 410,120 410,220 250,360 250,470; 250,640 250,660"),
    ("quotesingle", '\'', 260.0, "130,670 130,500"),
    ("quotedbl", '"', 380.0, "100,670 100,500; 260,670 260,500"),
    ("quoteleft", '‘', 300.0, "190,670 110,520"),
    ("quoteright", '’', 300.0, "110,670 190,520"),
    ("quotedblleft", '“', 400.0, "190,670 110,520; 310,670 230,520"),
    ("quotedblright", '”', 400.0, "110,670 190,520; 230,670 310,520"),
    ("hyphen", '-', 400.0, "100,350 300,350"),
    ("endash", '–', 500.0, "60,350 440,350"),
    ("emdash", '—', 700.0, "40,350 660,350"),
    ("parenleft", '(', 340.0, "260,670 150,520 150,180 260,30"),
    ("parenright", ')', 340.0, "80,670 190,520 190,180 80,30"),
    ("bracketleft", '[', 340.0, "280,670 120,670 120,30 280,30"),
    ("bracketright", ']', 340.0, "60,670 220,670 220,30 60,30"),
    ("braceleft", '{', 340.0, "280,670 200,640 200,400 120,350 200,300 200,60 280,30"),
    ("braceright", '}', 340.0, "60,670 140,640 140,400 220,350 140,300 140,60 60,30"),
    ("slash", '/', 500.0, "100,30 400,670"),
    ("backslash", '\\', 500.0, "100,670 400,30"),
    ("ampersand", '&', 580.0, "420,30 140,450 140,560 220,670 300,670 380,570 380,500 100,200 100,100 180,30 300,30 440,200"),
    ("at", '@', 640.0, "e:320,330,250,300; e:320,330,70,110"),
    ("numbersign", '#', 540.0, "190,670 150,30; 390,670 350,30; 70,480 460,480; 70,220 460,220"),
    ("dollar", '$', 500.0, "430,560 340,640 170,640 70,560 70,470 170,380 330,340 430,250 430,150 330,70 170,70 70,150; 250,20 250,680"),
    ("percent", '%', 640.0, "110,30 530,670; e:140,540,60,80; e:500,160,60,80"),
    ("asterisk", '*', 440.0, "220,560 220,300; 100,490 340,370; 100,370 340,490"),
    ("plus", '+', 500.0, "250,160 250,540; 70,350 430,350"),
    ("equal", '=', 500.0, "80,260 420,260; 80,440 420,440"),
    ("underscore", '_', 560.0, "20,-50 480,-50"),
    ("less", '<', 500.0, "420,520 90,350 420,180"),
    ("greater", '>', 500.0, "80,520 410,350 80,180"),
    ("ellipsis", '…', 700.0, "100,30 100,50; 300,30 300,50; 500,30 500,50"),
    ("periodcentered", '·', 300.0, "150,340 150,360"),
    ("guillemotleft", '«', 500.0, "230,520 90,350 230,180; 410,520 270,350 410,180"),
    ("guillemotright", '»', 500.0, "90,520 230,350 90,180; 270,520 410,350 270,180"),
    ("Euro", '€', 560.0, "420,570 330,640 200,640 90,540 90,160 200,60 330,60 420,130; 30,420 300,420; 30,280 300,280"),
];

/// The marks (no code point) and their strokes, above the cap height (700).
#[rustfmt::skip]
const MARKS: &[(&str, &str)] = &[
    ("acutecomb", "200,740 330,860"),
    ("dieresiscomb", "170,790 170,800; 330,790 330,800"),
    ("tildecomb", "100,780 170,850 250,790 330,860"),
];

/// The accented letters: name, base, mark and the upper case code point.
const ACCENTED: &[(&str, &str, &str, u32)] = &[
    ("Aacute", "A", "acutecomb", 0xC1),
    ("Eacute", "E", "acutecomb", 0xC9),
    ("Iacute", "I", "acutecomb", 0xCD),
    ("Oacute", "O", "acutecomb", 0xD3),
    ("Uacute", "U", "acutecomb", 0xDA),
    ("Udieresis", "U", "dieresiscomb", 0xDC),
    ("Ntilde", "N", "tildecomb", 0xD1),
];

/// The code points of a drawn glyph: a letter also covers its lower case.
fn codes_of(name: &str, c: char) -> Vec<u32> {
    let code = u32::from(c);
    if name.len() == 1 {
        vec![code, code + 32]
    } else {
        vec![code]
    }
}

/// The `skeletons` argument of `glyph_stroke` for the DSL `strokes`.
fn skeletons(strokes: &str) -> Vec<Value> {
    let node = |x: f64, y: f64| json!({ "x": x, "y": y, "kind": "line" });
    let number = |text: &str| -> f64 { text.trim().parse().expect("a number in the table") };
    strokes
        .split(';')
        .map(|stroke| {
            let stroke = stroke.trim();
            if let Some(ellipse) = stroke.strip_prefix("e:") {
                let v: Vec<f64> = ellipse.split(',').map(number).collect();
                let nodes: Vec<Value> = (0..20)
                    .map(|step| {
                        let angle = f64::from(step) * std::f64::consts::TAU / 20.0;
                        node(
                            (v[0] + v[2] * angle.cos()).round(),
                            (v[1] + v[3] * angle.sin()).round(),
                        )
                    })
                    .collect();
                return json!({ "closed": true, "nodes": nodes });
            }
            let (points, closed) = match stroke.strip_suffix('z') {
                Some(points) => (points, true),
                None => (stroke, false),
            };
            let nodes: Vec<Value> = points
                .split_whitespace()
                .map(|point| {
                    let (x, y) = point.split_once(',').expect("x,y");
                    node(number(x), number(y))
                })
                .collect();
            json!({ "closed": closed, "nodes": nodes })
        })
        .collect()
}

#[rstest]
#[case::legacy(legacy())]
#[case::modern(modern())]
#[tokio::test]
async fn unicase_es_complete_font(#[case] mode: ClientLifecycleMode) {
    with_timeout_for(
        "unicase_es_complete_font",
        Duration::from_secs(180),
        scenario(mode),
    )
    .await;
}

/// A glyph to draw: its name, advance width, code points and DSL strokes.
struct Drawn<'a> {
    name: &'a str,
    advance: f64,
    unicodes: Vec<u32>,
    strokes: &'a str,
}

/// Creates the glyph (empty, with its code points) and strokes it with `cap`;
/// returns the new rev.
async fn draw(client: &Client, doc: &str, mut rev: u64, glyph: &Drawn<'_>, cap: &Value) -> u64 {
    let Drawn {
        name,
        advance,
        unicodes,
        strokes,
    } = glyph;
    let codes: Vec<String> = unicodes.iter().map(|code| unicode_text(*code)).collect();
    let created = call(
        client,
        "glyph_create",
        json!({ "doc": doc, "base_rev": rev, "name": name, "advance_width": advance,
                "unicodes": codes }),
    )
    .await
    .ok();
    rev = created["rev"].as_u64().expect("rev");
    let stroked = call(
        client,
        "glyph_stroke",
        json!({ "doc": doc, "base_rev": rev, "glyph": name, "skeletons": skeletons(strokes),
                "width": WIDTH, "cap": cap, "mode": "replace" }),
    )
    .await
    .ok();
    stroked["rev"].as_u64().expect("rev")
}

/// Runs a mass call as a dry run and commits its plan; returns the new rev.
async fn plan_and_commit(client: &Client, tool: &str, args: Value) -> u64 {
    let mut args = args;
    args["dry_run"] = json!(true);
    let planned = call(client, tool, args).await.ok();
    let token = planned["plan"]["plan_token"]
        .as_str()
        .unwrap_or_else(|| panic!("{tool}: no plan token: {planned}"))
        .to_owned();
    let committed = call(client, "plan_commit", json!({ "plan_token": token }))
        .await
        .ok();
    assert_eq!(committed["plan_committed"], json!(true), "{committed}");
    committed["rev"].as_u64().expect("rev")
}

async fn scenario(mode: ClientLifecycleMode) {
    let started = Instant::now();
    let legacy = is_legacy(&mode);
    let (client, root) = connect(mode, &[]).await;
    let root = root.path();
    let image = path_text(&copy_specimen(root, "specimen.png"));
    let round = json!({ "type": "round" });

    let created = call(
        &client,
        "document_create",
        json!({ "family_name": "Unicase Test", "units_per_em": 1000 }),
    )
    .await
    .ok();
    let doc = created["doc"].as_str().expect("doc").to_owned();
    let mut rev = created["rev"].as_u64().expect("rev");

    // The eight letters of the image, upper and lower case.
    let assignments: Vec<Value> = TRACED
        .iter()
        .enumerate()
        .map(|(candidate, letter)| {
            let upper = u32::from(*letter);
            json!({ "candidate": candidate, "name": letter.to_string(),
                    "unicodes": [unicode_text(upper), unicode_text(upper + 32)] })
        })
        .collect();
    let traced = call(
        &client,
        "image_trace",
        json!({ "doc": doc, "base_rev": rev, "path": image, "assignments": assignments }),
    )
    .await
    .ok();
    rev = traced["rev"].as_u64().expect("rev");

    // .notdef and the space (two code points).
    let batch = call(
        &client,
        "edit_batch",
        json!({ "doc": doc, "base_rev": rev, "label": "Basic glyphs", "ops": [
            { "tool": "glyph_create", "args": { "name": ".notdef", "advance_width": 500,
              "svg_path": "M50 0 L450 0 L450 700 L50 700 Z M100 50 L100 650 L400 650 L400 50 Z" } },
            { "tool": "glyph_create", "args": { "name": "space", "advance_width": 250,
              "unicodes": ["U+0020", "U+00A0"] } },
        ] }),
    )
    .await
    .ok();
    rev = batch["rev"].as_u64().expect("rev");

    // The drawn letters, digits and symbols, then the marks.
    let bulb = json!({ "type": "bulb", "ratio": 1.4 });
    for &(name, c, advance, strokes) in DRAWN {
        let glyph = Drawn {
            name,
            advance,
            unicodes: codes_of(name, c),
            strokes,
        };
        rev = draw(&client, &doc, rev, &glyph, &round).await;
    }
    for &(name, strokes) in MARKS {
        let cap = if name == "acutecomb" { &bulb } else { &round };
        let glyph = Drawn {
            name,
            advance: 0.0,
            unicodes: Vec::new(),
            strokes,
        };
        rev = draw(&client, &doc, rev, &glyph, cap).await;
    }

    // The accented letters are components.
    for &(name, base, mark, upper) in ACCENTED {
        let composed = call(
            &client,
            "glyph_compose",
            json!({ "doc": doc, "base_rev": rev, "name": name,
                    "unicodes": [unicode_text(upper), unicode_text(upper + 32)],
                    "base": base, "marks": [{ "glyph": mark, "position": "above" }] }),
        )
        .await
        .ok();
        rev = composed["rev"].as_u64().expect("rev");
    }
    let built = started.elapsed();

    // Spacing and kerning (mass operations: dry run, then the plan).
    let spacing_started = Instant::now();
    rev = plan_and_commit(
        &client,
        "spacing_auto",
        json!({ "doc": doc, "base_rev": rev }),
    )
    .await;
    let spacing_time = spacing_started.elapsed();
    // The accented letters followed their respaced bases.
    let composites: Vec<(&str, &str, &str)> = ACCENTED
        .iter()
        .map(|&(name, base, mark, _)| (name, base, mark))
        .collect();
    assert_composites_follow_bases(&client, &doc, &composites).await;
    let kerning_started = Instant::now();
    rev = plan_and_commit(
        &client,
        "kerning_auto",
        json!({ "doc": doc, "base_rev": rev }),
    )
    .await;
    let kerning_time = kerning_started.elapsed();

    // Criterion 5: no collisions, with kerning_auto's own distance and with
    // overlaps only.
    for min_distance in [20.0, 0.0] {
        let checked = call(
            &client,
            "kerning_check",
            json!({ "doc": doc, "min_distance": min_distance }),
        )
        .await
        .ok();
        assert_eq!(
            checked["collisions_total"],
            json!(0),
            "{min_distance}: {checked}"
        );
        assert!(
            checked["checked_pairs"].as_u64().unwrap_or(0) > 0,
            "{checked}"
        );
        eprintln!(
            "[unicase] kerning_check min_distance {min_distance}: {} pairs",
            checked["checked_pairs"]
        );
    }

    // Criterion 3: the whole profile is covered.
    let charset = call(
        &client,
        "charset_check",
        json!({ "doc": doc, "profile": "unicase-es" }),
    )
    .await
    .ok();
    assert_eq!(charset["coverage"].as_f64(), Some(1.0), "{charset}");
    assert_eq!(charset["missing"], json!([]), "{charset}");
    let accented: Vec<&str> = ACCENTED.iter().map(|accent| accent.0).collect();
    let mut composite: Vec<&str> = charset["composite"]
        .as_array()
        .expect("composite")
        .iter()
        .map(|name| name.as_str().expect("name"))
        .collect();
    composite.sort_unstable();
    let mut sorted_accented = accented.clone();
    sorted_accented.sort_unstable();
    assert_eq!(composite, sorted_accented, "{charset}");

    let validated = call(&client, "validate", json!({ "doc": doc })).await.ok();
    assert_eq!(validated["report"]["errors"], json!(0), "{validated}");
    assert_eq!(validated["report"]["compiled"], json!(true), "{validated}");

    // 89 glyphs.
    let model = model_glyphs(&client, &doc).await;
    assert_eq!(
        model.len(),
        89,
        "{:?}",
        model.iter().map(|g| &g.name).collect::<Vec<_>>()
    );

    // Export TTF and WOFF2; the accented letters are composites in the TTF.
    let ttf_path = root.join("unicase-es.ttf");
    call(
        &client,
        "export_font",
        json!({ "doc": doc, "base_rev": rev, "format": "ttf", "path": path_text(&ttf_path) }),
    )
    .await
    .ok();
    let ttf = std::fs::read(&ttf_path).expect("the TTF");
    check_ttf(&ttf, &model, &accented);
    let woff2_path = root.join("unicase-es.woff2");
    call(
        &client,
        "export_font",
        json!({ "doc": doc, "base_rev": rev, "format": "woff2", "path": path_text(&woff2_path) }),
    )
    .await
    .ok();
    assert!(
        std::fs::read(&woff2_path)
            .expect("the WOFF2")
            .starts_with(b"wOF2"),
        "not a WOFF2 file"
    );

    if legacy && let Some(out) = std::env::var_os("FORMA_ACCEPTANCE_OUT") {
        let kerning = full_kerning(&client, &doc, &model).await;
        eprintln!(
            "[unicase] kerning: {} pairs, {} groups",
            kerning["pairs"].as_array().map_or(0, Vec::len),
            kerning["groups"].as_array().map_or(0, Vec::len)
        );
        write_acceptance(Path::new(&out), root, &model, &kerning, &accented);
    }
    eprintln!(
        "[unicase] built in {built:?}, spacing_auto {spacing_time:?}, kerning_auto {kerning_time:?}, total {:?}",
        started.elapsed()
    );
    close(client).await;
}

/// The whole kerning of `doc`: `kerning_get` cuts its lists at 200 entries, so
/// this asks glyph by glyph (the pairs that involve it, directly or through a
/// group) and merges the answers into one `{groups, pairs}`.
async fn full_kerning(client: &Client, doc: &str, model: &[ModelGlyph]) -> Value {
    let mut groups = BTreeMap::new();
    let mut pairs = BTreeMap::new();
    for glyph in model {
        let got = call(
            client,
            "kerning_get",
            json!({ "doc": doc, "glyphs": [glyph.name], "detail": "full" }),
        )
        .await
        .ok();
        assert_eq!(
            got["pairs_truncated"],
            json!(false),
            "{}: {got}",
            glyph.name
        );
        assert_eq!(
            got["groups_truncated"],
            json!(false),
            "{}: {got}",
            glyph.name
        );
        for group in got["groups"].as_array().expect("groups") {
            let key = format!("{}/{}", group["side"], group["name"]);
            groups.insert(key, group.clone());
        }
        for pair in got["pairs"].as_array().expect("pairs") {
            let key = (
                pair["first"].as_str().expect("first").to_owned(),
                pair["second"].as_str().expect("second").to_owned(),
            );
            pairs.insert(key, pair.clone());
        }
    }
    json!({ "groups": groups.into_values().collect::<Vec<_>>(),
            "pairs": pairs.into_values().collect::<Vec<_>>() })
}

/// Reads the TTF independently of the engine: every code point maps to the
/// glyph (by `post` name) with the model's rounded advance, and exactly the
/// `composites` are composite glyphs.
fn check_ttf(ttf: &[u8], model: &[ModelGlyph], composites: &[&str]) {
    let font = FontRef::new(ttf).expect("a readable TrueType font");
    let names: Vec<String> = font
        .post()
        .expect("post")
        .glyph_names()
        .map(|(_, name)| name.to_owned())
        .collect();
    assert_eq!(names.len(), model.len());
    assert_eq!(names.first().map(String::as_str), Some(".notdef"));
    let cmap = font.cmap().expect("cmap");
    let hmtx = font.hmtx().expect("hmtx");
    let mut encoded = 0;
    for glyph in model {
        for &code in &glyph.unicodes {
            let c = char::from_u32(code).expect("scalar");
            let gid = cmap
                .map_codepoint(c)
                .unwrap_or_else(|| panic!("the cmap does not map {c:?}"));
            assert_ne!(gid, GlyphId::NOTDEF, "{c:?} maps to .notdef");
            let name = usize::try_from(gid.to_u32())
                .ok()
                .and_then(|index| names.get(index))
                .unwrap_or_else(|| panic!("{c:?} has no glyph name"));
            assert_eq!(name, &glyph.name, "{c:?}");
            assert_eq!(
                hmtx.advance(gid).map(f64::from),
                Some(glyph.advance_width.round()),
                "advance of {name}"
            );
            encoded += 1;
        }
    }
    assert_eq!(encoded, 119, "code points in the font");
    let loca = font.loca(None).expect("loca");
    let glyf = font.glyf().expect("glyf");
    let mut found = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let gid = GlyphId::new(u32::try_from(index).expect("glyph index"));
        if let Some(Glyph::Composite(_)) = loca.get_glyf(gid, &glyf).expect("glyph") {
            found.push(name.as_str());
        }
    }
    found.sort_unstable();
    let mut expected = composites.to_vec();
    expected.sort_unstable();
    assert_eq!(found, expected, "composite glyphs of the TTF");
}

fn write_acceptance(
    out: &Path,
    root: &Path,
    model: &[ModelGlyph],
    kerning: &Value,
    composites: &[&str],
) {
    std::fs::create_dir_all(out)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", out.display()));
    for file in ["unicase-es.ttf", "unicase-es.woff2"] {
        std::fs::copy(root.join(file), out.join(file))
            .unwrap_or_else(|error| panic!("cannot copy {file}: {error}"));
    }
    let maps = kerning_maps(kerning);
    // Every encoded glyph pairs with every other one.
    let encoded: Vec<&ModelGlyph> = model.iter().filter(|g| !g.unicodes.is_empty()).collect();
    let mut pairs = Vec::new();
    for left in &encoded {
        for right in &encoded {
            let value = effective_kern(
                &maps.pairs,
                &maps.first_group,
                &maps.second_group,
                &left.name,
                &right.name,
            );
            if value.round() != 0.0 {
                pairs.push(
                    json!({ "left": left.name, "right": right.name, "value": rounded(value) }),
                );
            }
        }
    }
    assert!(!pairs.is_empty(), "no kerning in the font");
    eprintln!("[unicase] {} nonzero glyph pairs", pairs.len());
    let glyphs: BTreeMap<&str, Value> = model
        .iter()
        .map(|glyph| {
            (
                glyph.name.as_str(),
                json!({
                    "unicodes": glyph.unicodes.iter().map(|c| unicode_text(*c)).collect::<Vec<_>>(),
                    "advance": rounded(glyph.advance_width),
                }),
            )
        })
        .collect();
    let cmap: BTreeMap<String, &str> = model
        .iter()
        .flat_map(|glyph| {
            glyph
                .unicodes
                .iter()
                .map(|c| (unicode_text(*c), glyph.name.as_str()))
        })
        .collect();
    let shaped: Vec<&str> = SHAPING_TEXT
        .chars()
        .map(|c| {
            let code = u32::from(c.to_uppercase().next().expect("a character"));
            model
                .iter()
                .find(|glyph| {
                    glyph.unicodes.contains(&code) || glyph.unicodes.contains(&u32::from(c))
                })
                .unwrap_or_else(|| panic!("no glyph encodes {c:?}"))
                .name
                .as_str()
        })
        .collect();
    let expected = json!({
        "format": "family, upm; glyphs {name: {unicodes, advance}}; cmap {U+XXXX: name}; \
                   composites [name]; shaping {text, glyphs}; kerning [{left, right, value}] \
                   effective glyph-glyph values (nonzero integers) between every pair of encoded glyphs",
        "family": "Unicase Test",
        "upm": 1000,
        "glyphs": glyphs,
        "cmap": cmap,
        "composites": composites,
        "shaping": { "text": SHAPING_TEXT, "glyphs": shaped },
        "kerning": pairs,
    });
    let mut text = serde_json::to_string_pretty(&expected).expect("JSON");
    text.push('\n');
    std::fs::write(out.join("unicase-expected.json"), text).expect("write expected");
}
