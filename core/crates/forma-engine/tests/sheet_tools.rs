//! `preview_sheet` (selection, paging, limits, overlay) and the wrapped
//! form of `preview_text`.

use std::path::{Path, PathBuf};

use forma_engine::{Engine, EngineConfig, ErrorCode, Policy, ToolError, ToolOutput};
use forma_model::Actor;
use serde_json::{Value, json};

fn actor() -> Actor {
    Actor::Agent("test".into())
}

fn setup() -> (Engine, tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/hito2/specimen.png");
    let image = dir.path().join("specimen.png");
    std::fs::copy(source, &image).expect("copy the specimen");
    let engine = Engine::new(EngineConfig {
        allowed_roots: vec![dir.path().to_path_buf()],
        policy: Policy::Auto,
        ..EngineConfig::default()
    });
    (engine, dir, image)
}

#[track_caller]
fn json(engine: &mut Engine, tool: &str, args: Value) -> Value {
    match engine.call(&actor(), tool, args) {
        Ok(ToolOutput::Json(value)) => value,
        other => panic!("{tool}: {other:?}"),
    }
}

/// The PNG and summary of an image tool.
fn image(engine: &mut Engine, tool: &str, args: Value) -> Result<(Vec<u8>, Value), ToolError> {
    match engine.call(&actor(), tool, args)? {
        ToolOutput::Image { png, summary } => Ok((png, summary)),
        ToolOutput::Json(value) => panic!("{tool} answered JSON: {value}"),
    }
}

fn rev(engine: &mut Engine, doc: &str) -> u64 {
    json(engine, "document_inspect", json!({ "doc": doc }))["rev"]
        .as_u64()
        .expect("rev")
}

/// The specimen through the whole pipeline: traced letters, synthesized
/// glyphs and two composites.
fn font_doc(engine: &mut Engine, image: &Path) -> String {
    let doc = json(engine, "document_create", json!({ "family_name": "Sheet" }))["doc"]
        .as_str()
        .expect("doc")
        .to_owned();
    let base = rev(engine, &doc);
    json(
        engine,
        "font_from_image",
        json!({ "doc": doc, "base_rev": base, "path": image, "layout": ["ABCH", "OFMR"] }),
    );
    doc
}

fn sources(summary: &Value) -> Vec<String> {
    summary["cells"]
        .as_array()
        .expect("cells")
        .iter()
        .filter_map(|c| c["source"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn filters_select_by_origin_and_review() {
    let (mut engine, _dir, image) = setup();
    let doc = font_doc(&mut engine, &image);
    let (_, all) = image_ok(&mut engine, json!({ "doc": doc }));
    let total = all["total"].as_u64().expect("total");
    assert_eq!(all["cells"].as_array().map(Vec::len), Some(total as usize));
    assert_eq!(all["pages"], 1);
    for (filter, source) in [
        ("traced", "traced"),
        ("synthesized", "synthesized"),
        ("composed", "composed"),
    ] {
        let (_, s) = image_ok(&mut engine, json!({ "doc": doc, "filter": filter }));
        let got = sources(&s);
        assert!(!got.is_empty(), "{filter}");
        assert!(got.iter().all(|g| g == source), "{filter}: {got:?}");
    }
    let (_, traced) = image_ok(&mut engine, json!({ "doc": doc, "filter": "traced" }));
    assert_eq!(traced["total"], 8);
    let (_, pending) = image_ok(&mut engine, json!({ "doc": doc, "filter": "pending" }));
    assert_eq!(pending["total"], total, "everything the engine made waits");
}

#[track_caller]
fn image_ok(engine: &mut Engine, args: Value) -> (Vec<u8>, Value) {
    image(engine, "preview_sheet", args).unwrap_or_else(|e| panic!("preview_sheet: {e:?}"))
}

#[test]
fn limits_and_pages_are_checked() {
    let (mut engine, _dir, image_path) = setup();
    let doc = font_doc(&mut engine, &image_path);
    for args in [
        json!({ "doc": doc, "cell": 64 }),
        json!({ "doc": doc, "cell": 300 }),
        json!({ "doc": doc, "page": 1 }),
        json!({ "doc": doc, "glyphs": ["A"], "filter": "traced" }),
        json!({ "doc": doc, "glyphs": ["nosuch"] }),
    ] {
        let error = image(&mut engine, "preview_sheet", args.clone()).expect_err("refused");
        assert!(
            matches!(error.code, ErrorCode::InvalidArgument | ErrorCode::NotFound),
            "{args}: {error:?}"
        );
    }
    // More than 120 glyphs: two pages.
    let mut base = rev(&mut engine, &doc);
    for k in 0..70 {
        json(
            &mut engine,
            "glyph_create",
            json!({ "doc": doc, "base_rev": base, "name": format!("extra{k}"),
                "advance_width": 500, "svg_path": "M100 0 L400 0 L400 600 L100 600 Z" }),
        );
        base = rev(&mut engine, &doc);
    }
    let (_, first) = image_ok(&mut engine, json!({ "doc": doc }));
    assert_eq!(first["pages"], 2, "{first}");
    assert_eq!(first["cells"].as_array().map(Vec::len), Some(120));
    let (_, second) = image_ok(&mut engine, json!({ "doc": doc, "page": 1 }));
    let total = second["total"].as_u64().expect("total") as usize;
    assert_eq!(second["cells"].as_array().map(Vec::len), Some(total - 120));
    // Glyphs drawn by hand have no origin and no review.
    let (_, manual) = image_ok(&mut engine, json!({ "doc": doc, "filter": "manual" }));
    assert_eq!(manual["total"], 70);
}

#[test]
fn overlay_draws_the_ink_under_traced_glyphs() {
    let (mut engine, dir, image_path) = setup();
    let doc = font_doc(&mut engine, &image_path);
    let plain = json!({ "doc": doc, "filter": "traced" });
    let (without, _) = image_ok(&mut engine, plain);
    let (with, summary) = image_ok(
        &mut engine,
        json!({ "doc": doc, "filter": "traced", "overlay": { "path": image_path } }),
    );
    assert_ne!(without, with, "the overlay shows");
    assert!(summary.get("note").is_none(), "{summary}");
    // Another image: the glyphs were not traced from it, and the sheet says
    // so instead of drawing the wrong ink.
    let other = dir.path().join("other.png");
    let mut img = ::image::RgbImage::from_pixel(200, 100, ::image::Rgb([255, 255, 255]));
    for x in 50..150 {
        for y in 20..80 {
            img.put_pixel(x, y, ::image::Rgb([0, 0, 0]));
        }
    }
    img.save(&other).expect("save");
    let (_, summary) = image_ok(
        &mut engine,
        json!({ "doc": doc, "filter": "traced", "overlay": { "path": other } }),
    );
    assert!(
        summary["note"]
            .as_str()
            .is_some_and(|n| n.contains("another image")),
        "{summary}"
    );
}

#[test]
fn wrapped_text_reports_its_lines() {
    let (mut engine, _dir, image_path) = setup();
    let doc = font_doc(&mut engine, &image_path);
    let text = "ABBA CABO HORA ".repeat(40);
    let (_, one) = image(
        &mut engine,
        "preview_text",
        json!({ "doc": doc, "text": text, "height": 64 }),
    )
    .expect("one line");
    assert!(one.get("lines").is_none(), "{one}");
    let (_, wrapped) = image(
        &mut engine,
        "preview_text",
        json!({ "doc": doc, "text": text, "height": 64, "wrap": true }),
    )
    .expect("wrapped");
    let lines = wrapped["lines"].as_u64().expect("lines");
    assert!(lines > 1, "{wrapped}");
    assert_eq!(wrapped["height"].as_u64(), Some(lines * 64), "{wrapped}");
    assert!(wrapped.get("lines_dropped").is_none(), "{wrapped}");
    // Taller than 4096 pixels: the lines left out are counted.
    let (_, cut) = image(
        &mut engine,
        "preview_text",
        json!({ "doc": doc, "text": text, "height": 1024, "wrap": true }),
    )
    .expect("cut");
    assert_eq!(cut["lines"], 4, "{cut}");
    assert!(cut["lines_dropped"].as_u64() > Some(0), "{cut}");
}
