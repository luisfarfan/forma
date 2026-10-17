//! Milestone 1, acceptance criterion 1: an AI agent creates a font through
//! MCP, compiles it and validates it, with no GUI, in both protocol eras.
//!
//! The test drives the real `forma mcp` binary over stdio with rmcp's client
//! (see `mcp_common`), exactly as an agent client would, through the ten
//! steps of `docs/04-roadmap.md`. The glyphs come from
//! `tests/fixtures/hito1/glyphs.json`, whose `o` has a counter drawn the
//! wrong way on purpose.
//!
//! When `FORMA_ACCEPTANCE_OUT` names a folder, the legacy-era run also leaves
//! its artefacts there for the external oracles (`scripts/acceptance.*`):
//! `forma-demo.ttf`, `forma-demo.woff2`, `forma-demo.forma` and
//! `expected.json`. A relative folder is resolved against the test's working
//! directory (the crate folder).

mod mcp_common;

use std::collections::BTreeMap;
use std::path::Path;

use mcp_common::{
    ACTOR, Client, call, close, connect, decode_png, fixture, is_legacy, legacy, modern,
    with_timeout,
};
use read_fonts::types::GlyphId;
use read_fonts::{FontRef, TableProvider};
use rmcp::ClientLifecycleMode;
use rstest::rstest;
use serde_json::{Value, json};

/// The text the external shaping check sets (Task 20).
const SHAPING_TEXT: &str = "HOnoABCabc";

/// The characters whose cmap entries the test checks in the exported TTF.
const MAPPED: &str = "HOnoABCabc ";

#[rstest]
#[case::legacy(legacy())]
#[case::modern(modern())]
#[tokio::test]
async fn agent_scenario(#[case] mode: ClientLifecycleMode) {
    with_timeout("agent_scenario", scenario(mode)).await;
}

async fn scenario(mode: ClientLifecycleMode) {
    let legacy = is_legacy(&mode);
    let (client, root) = connect(mode, &[]).await;
    let root = root.path();
    let fixture = fixture();
    let family = fixture["family_name"].as_str().expect("family_name");
    let glyphs = fixture["glyphs"].as_array().expect("glyphs");
    let names: Vec<&str> = glyphs
        .iter()
        .map(|glyph| glyph["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        [
            ".notdef", "space", "A", "B", "C", "H", "O", "a", "b", "c", "n", "o"
        ]
    );

    // 1. document_create "Forma Demo".
    let created = call(
        &client,
        "document_create",
        json!({ "family_name": family, "units_per_em": fixture["units_per_em"] }),
    )
    .await
    .ok();
    let doc = created["doc"].as_str().expect("doc").to_owned();
    assert_eq!(created["rev"], json!(0));
    // The fixture is drawn for the vertical metrics of a new document.
    let inspected = call(&client, "document_inspect", json!({ "doc": doc }))
        .await
        .ok();
    assert_eq!(inspected["info"]["family_name"], json!(family));
    assert_eq!(inspected["units_per_em"], fixture["units_per_em"]);
    let metrics = &inspected["masters"][0]["metrics"];
    for (key, value) in fixture["vertical_metrics"].as_object().expect("metrics") {
        assert_eq!(metrics[key].as_f64(), value.as_f64(), "{key}: {inspected}");
    }

    // 2. One edit_batch with the 12 glyph_create operations (12 glyphs is
    //    under the mass-edit threshold of 20: no approval).
    let ops: Vec<Value> = glyphs
        .iter()
        .map(|glyph| json!({ "tool": "glyph_create", "args": glyph }))
        .collect();
    let batch = call(
        &client,
        "edit_batch",
        json!({ "doc": doc, "base_rev": 0, "label": "Draw the Forma Demo glyphs", "ops": ops }),
    )
    .await
    .ok();
    assert_eq!(batch["rev"], json!(1), "{batch}");
    assert!(batch["tx"].is_u64(), "{batch}");
    // The glyph order plus the 12 glyphs.
    assert_eq!(batch["touched_count"], json!(13), "{batch}");
    // Every glyph as the fixture describes it, in its order.
    let created = model_glyphs(&client, &doc).await;
    assert_eq!(created.len(), glyphs.len());
    for (glyph, expected) in created.iter().zip(glyphs) {
        assert_eq!(glyph.name, expected["name"].as_str().expect("name"));
        assert_eq!(
            Some(glyph.advance_width),
            expected["advance_width"].as_f64()
        );
        let unicodes: Vec<u32> = expected["unicodes"]
            .as_array()
            .expect("unicodes")
            .iter()
            .map(|code| {
                let hex = code.as_str().and_then(|code| code.strip_prefix("U+"));
                u32::from_str_radix(hex.expect("U+hhhh"), 16).expect("hex")
            })
            .collect();
        assert_eq!(glyph.unicodes, unicodes, "{}", glyph.name);
        let contours = expected["svg_path"]
            .as_str()
            .map_or(0, |path| path.matches('M').count());
        assert_eq!(glyph.contours, contours, "{}", glyph.name);
    }

    // 3. metrics_set: wider side bearings for `n` (its outline moves 10
    //    units right and its advance grows by 20).
    let set = call(
        &client,
        "metrics_set",
        json!({ "doc": doc, "base_rev": 1, "items": [{ "glyph": "n", "lsb": 90, "rsb": 90 }] }),
    )
    .await
    .ok();
    assert_eq!(set["rev"], json!(2), "{set}");
    let measured = call(
        &client,
        "metrics_get",
        json!({ "doc": doc, "glyphs": ["n"] }),
    )
    .await
    .ok();
    let n = &measured["metrics"][0];
    assert_eq!(n["glyph"], json!("n"));
    assert_eq!(n["advance_width"].as_f64(), Some(580.0), "{measured}");
    assert_eq!(n["lsb"].as_f64(), Some(90.0), "{measured}");
    assert_eq!(n["rsb"].as_f64(), Some(90.0), "{measured}");

    // 4. geometry_check finds the counter of `o` drawn the wrong way, and
    //    nothing else: no other error (open or invalid contour, direction)
    //    and no other fixable issue.
    let check = call(&client, "geometry_check", json!({ "doc": doc }))
        .await
        .ok();
    assert_eq!(check["rev"], json!(2));
    let issues = check["issues"].as_array().expect("issues");
    assert_eq!(issues.len(), 1, "{check}");
    let issue = &issues[0];
    assert_eq!(issue["glyph"], json!("o"), "{check}");
    assert_eq!(issue["kind"], json!("wrong_direction"), "{check}");
    assert_eq!(issue["fixable"], json!(true), "{check}");
    assert_eq!(issue["path"], json!(1), "the counter is the second contour");

    // 5. geometry_fix repairs it; nothing fixable is left.
    let fixed = call(
        &client,
        "geometry_fix",
        json!({ "doc": doc, "base_rev": 2 }),
    )
    .await
    .ok();
    assert_eq!(fixed["rev"], json!(3), "{fixed}");
    assert_eq!(fixed["fixed"], json!(1), "{fixed}");
    assert_eq!(fixed["touched_count"], json!(1), "{fixed}");
    let check = call(&client, "geometry_check", json!({ "doc": doc }))
        .await
        .ok();
    assert_eq!(check["rev"], json!(3));
    assert_eq!(check["issues"], json!([]), "{check}");

    // 6. validate: no errors, and the font compiled.
    let validated = call(&client, "validate", json!({ "doc": doc })).await.ok();
    assert_eq!(validated["rev"], json!(3));
    assert_eq!(validated["report"]["errors"], json!(0), "{validated}");
    assert_eq!(validated["report"]["compiled"], json!(true), "{validated}");

    // 7. export_font TTF and WOFF2 into the allowed root.
    let model = model_glyphs(&client, &doc).await;
    let ttf_path = root.join("forma-demo.ttf");
    let ttf = export(&client, &doc, 3, "ttf", &ttf_path).await;
    check_ttf(&ttf, &model);
    let woff2_path = root.join("forma-demo.woff2");
    let woff2 = export(&client, &doc, 3, "woff2", &woff2_path).await;
    assert!(woff2.starts_with(b"wOF2"), "not a WOFF2 file");

    // 8. preview_glyph `o`: a decodable 512 x 512 PNG with a JSON summary, and
    //    no structured content (clients such as Codex drop images otherwise).
    let preview = call(
        &client,
        "preview_glyph",
        json!({ "doc": doc, "glyph": "o" }),
    )
    .await;
    assert!(!preview.is_error, "{:?}", preview.json);
    assert!(
        !preview.structured,
        "an image result has no structured content"
    );
    let png = preview.image.as_deref().expect("an image block");
    let pixmap = decode_png(png);
    assert_eq!((pixmap.width(), pixmap.height()), (512, 512));
    let inked = pixmap
        .data()
        .iter()
        .filter(|pixel| pixel.r < 128 && pixel.g < 128 && pixel.b < 128)
        .count();
    assert!(inked > 0, "the preview of `o` is blank");
    let summary = preview.json.expect("summary");
    assert_eq!(summary["glyph"], json!("o"), "{summary}");
    assert_eq!(summary["size"], json!(512), "{summary}");
    assert_eq!(summary["rev"], json!(3), "{summary}");

    // 9. history_list: the three changes, newest first, all attributed to the
    //    test client.
    let history = call(&client, "history_list", json!({ "doc": doc }))
        .await
        .ok();
    let entries = history["entries"].as_array().expect("entries");
    let revs: Vec<&Value> = entries.iter().map(|entry| &entry["rev"]).collect();
    assert_eq!(revs, [&json!(3), &json!(2), &json!(1)], "{history}");
    for entry in entries {
        assert_eq!(entry["actor"], json!(ACTOR), "{entry}");
    }
    assert_eq!(entries[2]["label"], json!("Draw the Forma Demo glyphs"));

    // 10. document_save as .forma, document_close, document_open: the same
    //     content, `o` included.
    let before = content(&client, &doc).await;
    let o_before = call(&client, "glyph_get", json!({ "doc": doc, "glyph": "o" }))
        .await
        .ok();
    let forma_path = root.join("forma-demo.forma");
    let saved = call(
        &client,
        "document_save",
        json!({ "doc": doc, "base_rev": 3, "path": mcp_common::path_text(&forma_path) }),
    )
    .await
    .ok();
    assert_eq!(saved["rev"], json!(3), "{saved}");
    assert!(forma_path.is_file());
    let closed = call(&client, "document_close", json!({ "doc": doc }))
        .await
        .ok();
    assert_eq!(closed["closed"], json!(true), "{closed}");
    let listed = call(&client, "document_list", json!({})).await.ok();
    assert_eq!(listed["documents"], json!([]), "{listed}");

    let opened = call(
        &client,
        "document_open",
        json!({ "path": mcp_common::path_text(&forma_path) }),
    )
    .await
    .ok();
    assert_eq!(opened["rev"], json!(0), "{opened}");
    assert_eq!(opened["already_open"], json!(false), "{opened}");
    // Saved and closed cleanly: no crash-recovery records are pending.
    assert!(opened.get("recovery_available").is_none(), "{opened}");
    let reopened = opened["doc"].as_str().expect("doc").to_owned();
    let o_after = call(
        &client,
        "glyph_get",
        json!({ "doc": reopened, "glyph": "o" }),
    )
    .await
    .ok();
    assert_eq!(o_after["glyph"]["paths"], o_before["glyph"]["paths"]);
    assert_eq!(
        o_after["glyph"]["advance_width"],
        o_before["glyph"]["advance_width"]
    );
    assert_eq!(o_after["glyph"], o_before["glyph"]);
    assert_eq!(content(&client, &reopened).await, before);
    // The repaired direction was saved too.
    let check = call(&client, "geometry_check", json!({ "doc": reopened }))
        .await
        .ok();
    assert_eq!(check["issues"], json!([]), "{check}");

    if legacy && let Some(out) = std::env::var_os("FORMA_ACCEPTANCE_OUT") {
        let model = model_glyphs(&client, &reopened).await;
        write_acceptance(Path::new(&out), root, family, &fixture, &model);
    }
    close(client).await;
}

/// One glyph of the model, as `glyph_list` reports it.
#[derive(Clone, Debug)]
struct ModelGlyph {
    name: String,
    unicodes: Vec<u32>,
    advance_width: f64,
    contours: usize,
}

/// Every glyph of `doc`, in glyph order.
async fn model_glyphs(client: &Client, doc: &str) -> Vec<ModelGlyph> {
    let listed = call(client, "glyph_list", json!({ "doc": doc, "limit": 500 }))
        .await
        .ok();
    listed["glyphs"]
        .as_array()
        .expect("glyphs")
        .iter()
        .map(|glyph| ModelGlyph {
            name: glyph["name"].as_str().expect("name").to_owned(),
            unicodes: glyph["unicodes"]
                .as_array()
                .expect("unicodes")
                .iter()
                .map(|code| u32::try_from(code.as_u64().expect("code point")).expect("u32"))
                .collect(),
            advance_width: glyph["advance_width"].as_f64().expect("advance"),
            contours: glyph["contours"]
                .as_u64()
                .and_then(|count| usize::try_from(count).ok())
                .expect("contours"),
        })
        .collect()
}

/// The model glyph that encodes `c`.
fn glyph_for(model: &[ModelGlyph], c: char) -> &ModelGlyph {
    model
        .iter()
        .find(|glyph| glyph.unicodes.contains(&u32::from(c)))
        .unwrap_or_else(|| panic!("no glyph encodes {c:?}"))
}

/// `export_font` of `doc` at `base_rev` to `path`; returns the file's bytes
/// after checking the result.
async fn export(client: &Client, doc: &str, base_rev: u64, format: &str, path: &Path) -> Vec<u8> {
    let exported = call(
        client,
        "export_font",
        json!({ "doc": doc, "base_rev": base_rev, "format": format,
                "path": mcp_common::path_text(path) }),
    )
    .await
    .ok();
    assert_eq!(exported["format"], json!(format), "{exported}");
    assert_eq!(exported["rev"], json!(base_rev), "{exported}");
    // The fixture has its own .notdef and space: nothing was added.
    assert_eq!(exported["report"]["added_glyphs"], json!([]), "{exported}");
    let bytes = std::fs::read(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    assert_eq!(exported["bytes"].as_u64(), u64::try_from(bytes.len()).ok());
    bytes
}

/// Reads the TTF with read-fonts, independently of the engine: `.notdef` is
/// glyph 0, and each character of [`MAPPED`] maps to the glyph of the model
/// that encodes it (matched by its `post` name), with the model's advance
/// width, rounded.
fn check_ttf(ttf: &[u8], model: &[ModelGlyph]) {
    let font = FontRef::new(ttf).expect("a readable TrueType font");
    let names: Vec<String> = font
        .post()
        .expect("post")
        .glyph_names()
        .map(|(_, name)| name.to_owned())
        .collect();
    let count = font.maxp().expect("maxp").num_glyphs();
    assert_eq!(names.len(), usize::from(count), "{names:?}");
    assert_eq!(names.first().map(String::as_str), Some(".notdef"));
    let cmap = font.cmap().expect("cmap");
    let hmtx = font.hmtx().expect("hmtx");
    for c in MAPPED.chars() {
        let expected = glyph_for(model, c);
        let gid = cmap
            .map_codepoint(c)
            .unwrap_or_else(|| panic!("the cmap does not map {c:?}"));
        assert_ne!(gid, GlyphId::NOTDEF, "{c:?} maps to .notdef");
        let name = usize::try_from(gid.to_u32())
            .ok()
            .and_then(|index| names.get(index))
            .unwrap_or_else(|| panic!("{c:?} maps to glyph {gid}, which has no name"));
        assert_eq!(name, &expected.name, "{c:?}");
        assert_eq!(
            hmtx.advance(gid).map(f64::from),
            Some(expected.advance_width.round()),
            "advance of {name}"
        );
    }
}

/// What must survive a save and a reopen: the font info, the units per em,
/// the masters and every glyph as `glyph_get` reports it.
async fn content(client: &Client, doc: &str) -> Value {
    let inspected = call(client, "document_inspect", json!({ "doc": doc }))
        .await
        .ok();
    let mut glyphs = Vec::new();
    for glyph in mcp_common::glyph_names(client, doc).await {
        let got = call(client, "glyph_get", json!({ "doc": doc, "glyph": glyph }))
            .await
            .ok();
        glyphs.push(got["glyph"].clone());
    }
    json!({
        "info": inspected["info"],
        "units_per_em": inspected["units_per_em"],
        "masters": inspected["masters"],
        "glyph_count": inspected["glyph_count"],
        "glyphs": glyphs,
    })
}

/// Leaves the artefacts of the scenario in `out` for the external oracles:
/// the TTF, the WOFF2, the `.forma` document and `expected.json`, which is
/// computed from the model (not from the binary): the family, the units per
/// em, the cmap as character → glyph name, and the glyphs and advance widths
/// (rounded) that shaping [`SHAPING_TEXT`] must give.
fn write_acceptance(out: &Path, root: &Path, family: &str, fixture: &Value, model: &[ModelGlyph]) {
    std::fs::create_dir_all(out)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", out.display()));
    for file in ["forma-demo.ttf", "forma-demo.woff2", "forma-demo.forma"] {
        std::fs::copy(root.join(file), out.join(file))
            .unwrap_or_else(|error| panic!("cannot copy {file} to {}: {error}", out.display()));
    }
    let cmap: BTreeMap<String, String> = model
        .iter()
        .flat_map(|glyph| {
            glyph.unicodes.iter().map(|&code| {
                let c = char::from_u32(code).expect("a Unicode scalar value");
                (c.to_string(), glyph.name.clone())
            })
        })
        .collect();
    let shaped: Vec<&ModelGlyph> = SHAPING_TEXT.chars().map(|c| glyph_for(model, c)).collect();
    let expected = json!({
        "family": family,
        "upm": fixture["units_per_em"],
        "cmap": cmap,
        "shaping": {
            "text": SHAPING_TEXT,
            "glyphs": shaped.iter().map(|glyph| glyph.name.as_str()).collect::<Vec<_>>(),
            "advances": shaped
                .iter()
                .map(|glyph| rounded(glyph.advance_width))
                .collect::<Vec<_>>(),
        },
    });
    let mut text = serde_json::to_string_pretty(&expected).expect("JSON");
    text.push('\n');
    let path = out.join("expected.json");
    std::fs::write(&path, text)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

/// An advance width rounded with [`f64::round`], as a JSON integer.
fn rounded(advance: f64) -> i64 {
    let rounded = advance.round();
    assert!(rounded.abs() < 1e9, "advance width {advance} out of range");
    // Exact: a whole number well inside the range of i64.
    rounded as i64
}
