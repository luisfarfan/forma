//! The page `font_from_image` writes to look at its result: one
//! self-contained HTML file (`stages.export.html`).
//!
//! It shows the image beside what the engine saw in it (each shape, the
//! character it was matched to, the lines of each row), every glyph drawn
//! from its outlines with how it was made and whether it waits for review,
//! a text field set in the font itself, the stages with their times, what
//! needs a person's attention and the arguments of the call.
//!
//! Everything is inside the file (the image, the font, the outlines) and
//! the page loads nothing: its content security policy forbids any request.
//! The text of the document and of the call (glyph names, layout, reasons)
//! is data and is escaped; the script is fixed and reads only the page.
//! The page is a view: it changes nothing and approves nothing.

use std::collections::BTreeMap;
use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use forma_model::{Font, Glyph, MasterId, Review, Source};
use image::RgbImage;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use serde_json::Value;

use crate::errors::ToolError;
use crate::tools::pipeline::{AttentionItem, PageLang, StageSummary};

/// The template; `{{KEY}}` marks are filled in one pass.
const TEMPLATE: &str = include_str!("page.html");

/// The longest side of the image as the page embeds it, in pixels.
const PHOTO_MAX_SIDE: u32 = 1400;

/// The JPEG quality of the embedded image.
const PHOTO_QUALITY: u8 = 82;

/// The most glyphs the page draws.
const MAX_PAGE_GLYPHS: usize = 600;

/// What the page is made of.
pub(crate) struct PageInput<'a> {
    /// The font after the change.
    pub font: &'a Font,
    /// The master that was drawn.
    pub master: MasterId,
    /// The image, as decoded.
    pub image: &'a RgbImage,
    /// What the engine found in it.
    pub analysis: &'a forma_trace::Analysis,
    /// The shapes of each glyph and its character.
    pub matched: &'a [(Vec<usize>, char)],
    /// The rows of the layout.
    pub layout: &'a [String],
    /// Each stage, in order.
    pub stages: &'a [StageSummary],
    /// Everything that needs attention.
    pub attention: &'a [AttentionItem],
    /// Glyphs that need a look or wait for review.
    pub pending: usize,
    /// Time spent in the engine, in milliseconds.
    pub engine_ms: u64,
    /// The arguments of the call.
    pub args: &'a Value,
    /// The language of the page.
    pub lang: PageLang,
}

/// The words of the page in one language.
struct Words {
    lang: &'static str,
    eyebrow: &'static str,
    hero: &'static str,
    lede: &'static str,
    family: &'static str,
    stat_time: &'static str,
    stat_traced: &'static str,
    stat_covered: &'static str,
    stat_errors: &'static str,
    banner: &'static str,
    s1_title: &'static str,
    s1_sub: &'static str,
    photo_alt: &'static str,
    tag_photo: &'static str,
    tag_seen: &'static str,
    legend_matched: &'static str,
    legend_unmatched: &'static str,
    legend_cap: &'static str,
    legend_base: &'static str,
    s2_title: &'static str,
    s2_sub: &'static str,
    traced: &'static str,
    synthesized: &'static str,
    composed: &'static str,
    manual: &'static str,
    pending: &'static str,
    more_glyphs: &'static str,
    s3_title: &'static str,
    s3_sub: &'static str,
    s3_hint: &'static str,
    no_font: &'static str,
    size: &'static str,
    samples: [(&'static str, &'static str); 2],
    alphabet: &'static str,
    inks: [&'static str; 4],
    s4_title: &'static str,
    s4_sub: &'static str,
    skipped: &'static str,
    s5_title: &'static str,
    s5_sub: &'static str,
    more_attention: &'static str,
    s6_title: &'static str,
    s6_sub: &'static str,
    footer: &'static str,
}

const EN: Words = Words {
    lang: "en",
    eyebrow: "FORMA · image → font",
    hero: "FROM AN IMAGE TO A FONT",
    lede: "The image of an alphabet. FORMA found each character, turned it into curves, \
           completed what was missing and built a font. The large text of this page is set in \
           that font.",
    family: "Family",
    stat_time: "in the engine, no AI",
    stat_traced: "letters traced from the image",
    stat_covered: "characters in the font",
    stat_errors: "validation errors",
    banner: "{n} glyph(s) wait for a look or a review (outlined in orange). Review them with \
             glyph_review before publishing the font.",
    s1_title: "The image, and what FORMA sees in it",
    s1_sub: "Drag the line. On the left, the image as it is; on the right, each shape found, the \
             character it was matched to, and the lines of each row.",
    photo_alt: "The source image",
    tag_photo: "Image",
    tag_seen: "What FORMA sees",
    legend_matched: "shape and its character",
    legend_unmatched: "shape not matched",
    legend_cap: "cap height",
    legend_base: "baseline (tilted if the row is)",
    s2_title: "Each letter, turned into curves",
    s2_sub: "Vector outlines, drawn here from the font. Synthesized glyphs are born waiting for \
             review: they are never approved on their own.",
    traced: "traced",
    synthesized: "synthesized",
    composed: "composed",
    manual: "drawn",
    pending: "waits for review",
    more_glyphs: "and {n} more glyphs",
    s3_title: "Write with it",
    s3_sub: "The font, loaded in this page. Write anything on the sheet.",
    s3_hint: "Characters the font does not have yet show in the system font.",
    no_font: "The font could not be compiled for this page: {e}",
    size: "Size",
    samples: [
        ("Question", "WHO SAID HANDWRITING COULD NOT BE A FONT?"),
        ("Pangram", "THE QUICK BROWN FOX JUMPS OVER THE LAZY DOG."),
    ],
    alphabet: "Alphabet",
    inks: ["Blue ink", "Black ink", "Red ink", "Green ink"],
    s4_title: "What the engine did, stage by stage",
    s4_sub: "One call, font_from_image, as one transaction that one undo takes back whole. \
             The bars show how long each stage took.",
    skipped: "skipped",
    s5_title: "What to look at",
    s5_sub: "What the engine did not decide alone. The reasons are the engine's own words.",
    more_attention: "and {n} more in the report",
    s6_title: "The call",
    s6_sub: "The arguments of font_from_image (file names only).",
    footer: "Made by FORMA {v} with font_from_image, without AI",
};

const ES: Words = Words {
    lang: "es",
    eyebrow: "FORMA · imagen → fuente",
    hero: "DE UNA FOTO A UNA FUENTE",
    lede: "La imagen de un alfabeto. FORMA encontró cada carácter, lo convirtió en curvas, \
           completó lo que faltaba y armó una fuente. El texto grande de esta página está \
           escrito con esa fuente.",
    family: "Familia",
    stat_time: "de motor, sin IA",
    stat_traced: "letras trazadas de la imagen",
    stat_covered: "caracteres en la fuente",
    stat_errors: "errores de validación",
    banner: "{n} glifo(s) esperan una mirada o una revisión (con borde naranja). Revísalos con \
             glyph_review antes de publicar la fuente.",
    s1_title: "La imagen, y lo que FORMA ve en ella",
    s1_sub: "Arrastra la línea. A la izquierda, la imagen tal cual; a la derecha, cada forma \
             encontrada, el carácter que se le asignó y las líneas de cada fila.",
    photo_alt: "La imagen de origen",
    tag_photo: "Imagen",
    tag_seen: "Lo que ve FORMA",
    legend_matched: "forma y su carácter",
    legend_unmatched: "forma sin asignar",
    legend_cap: "altura de las mayúsculas",
    legend_base: "línea base (inclinada si la fila lo está)",
    s2_title: "Cada letra, convertida en curvas",
    s2_sub: "Contornos vectoriales, dibujados aquí desde la fuente. Los sintetizados nacen \
             pendientes de revisión: nunca se aprueban solos.",
    traced: "trazado",
    synthesized: "sintetizado",
    composed: "compuesto",
    manual: "dibujado",
    pending: "espera revisión",
    more_glyphs: "y {n} glifos más",
    s3_title: "Escribe con ella",
    s3_sub: "La fuente, cargada en esta página. Escribe lo que quieras en la hoja.",
    s3_hint: "Los caracteres que la fuente todavía no tiene se ven con la letra del sistema.",
    no_font: "No se pudo compilar la fuente para esta página: {e}",
    size: "Tamaño",
    samples: [
        (
            "Pregunta",
            "¿QUIÉN DIJO QUE LA LETRA DE MANO NO PODÍA SER UNA FUENTE?",
        ),
        (
            "Pangrama",
            "EL PINGÜINO WENCESLAO HIZO KILÓMETROS BAJO EXHAUSTIVA LLUVIA Y FRÍO, AÑORABA A SU \
             QUERIDO CACHORRO.",
        ),
    ],
    alphabet: "Alfabeto",
    inks: ["Tinta azul", "Tinta negra", "Tinta roja", "Tinta verde"],
    s4_title: "Qué hizo el motor, etapa por etapa",
    s4_sub: "Una sola llamada, font_from_image, en una sola transacción que un solo deshacer \
             revierte completa. Las barras muestran cuánto tardó cada etapa.",
    skipped: "omitida",
    s5_title: "Qué conviene mirar",
    s5_sub: "Lo que el motor no decidió solo. Los motivos son las palabras del motor, en inglés.",
    more_attention: "y {n} más en el informe",
    s6_title: "La llamada",
    s6_sub: "Los argumentos de font_from_image (solo los nombres de los archivos).",
    footer: "Hecho por FORMA {v} con font_from_image, sin IA",
};

const INK_COLORS: [&str; 4] = ["#1c2456", "#17171a", "#b23a2a", "#2c7a4b"];

/// Escapes text for HTML content and attribute values.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Fills the `{{KEY}}` marks of `template` in one pass: a value that
/// contains a mark is copied as it is, never filled again. An unknown mark
/// is left empty.
fn fill(template: &str, values: &BTreeMap<&str, String>) -> String {
    let mut out = String::with_capacity(template.len() * 2);
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                if let Some(value) = values.get(&after[..end]) {
                    out.push_str(value);
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// A number with at most one decimal, without trailing zeros.
fn short(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    let text = format!("{rounded:.1}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// SVG path data with its numbers rounded to one decimal.
fn rounded_path(data: &str) -> String {
    let mut out = String::with_capacity(data.len());
    let mut number = String::new();
    let flush = |number: &mut String, out: &mut String| {
        if !number.is_empty() {
            match number.parse::<f64>() {
                Ok(value) => out.push_str(&short(value)),
                Err(_) => out.push_str(number),
            }
            number.clear();
        }
    };
    for c in data.chars() {
        if c.is_ascii_digit() || c == '.' {
            number.push(c);
        } else if c == '-' {
            flush(&mut number, &mut out);
            number.push(c);
        } else {
            flush(&mut number, &mut out);
            out.push(c);
        }
    }
    flush(&mut number, &mut out);
    out
}

/// Milliseconds as seconds, with a decimal comma in Spanish.
fn seconds(ms: u64, lang: PageLang) -> String {
    #[allow(clippy::cast_precision_loss)]
    let text = format!("{:.1} s", ms as f64 / 1000.0);
    match lang {
        PageLang::Es => text.replace('.', ","),
        PageLang::En => text,
    }
}

fn count(stages: &[StageSummary], stage: &str, key: &str) -> Option<u64> {
    stages
        .iter()
        .find(|s| s.name == stage)
        .and_then(|s| s.counts.get(key))
        .and_then(Value::as_u64)
}

/// The image as a JPEG, its longest side at most [`PHOTO_MAX_SIDE`].
fn photo_jpeg(image: &RgbImage) -> Result<Vec<u8>, ToolError> {
    let (w, h) = image.dimensions();
    let longest = w.max(h);
    let scaled;
    let source = if longest > PHOTO_MAX_SIDE {
        let ratio = f64::from(PHOTO_MAX_SIDE) / f64::from(longest);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (nw, nh) = (
            ((f64::from(w) * ratio).round() as u32).max(1),
            ((f64::from(h) * ratio).round() as u32).max(1),
        );
        scaled = image::imageops::resize(image, nw, nh, FilterType::Triangle);
        &scaled
    } else {
        image
    };
    let mut out = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut out, PHOTO_QUALITY)
        .encode_image(source)
        .map_err(|error| ToolError::io(format!("cannot encode the image of the page: {error}")))?;
    Ok(out.into_inner())
}

/// The shapes found, the character of each glyph and the lines of each
/// row, in the image's pixels.
fn overlay(analysis: &forma_trace::Analysis, matched: &[(Vec<usize>, char)]) -> String {
    let by_index: BTreeMap<usize, &forma_trace::Candidate> =
        analysis.candidates.iter().map(|c| (c.index, c)).collect();
    let mut heights: Vec<u32> = analysis
        .candidates
        .iter()
        .map(|c| c.bbox.y1.saturating_sub(c.bbox.y0))
        .collect();
    heights.sort_unstable();
    let typical = heights.get(heights.len() / 2).copied().unwrap_or(40);
    let label = (f64::from(typical) * 0.34).clamp(10.0, 64.0);
    let width = f64::from(analysis.width);
    let mut out = String::new();
    for row in &analysis.rows {
        for (y, color) in [(row.cap_px, "#d63cd6"), (row.baseline_px, "#1fb3d6")] {
            let y0 = y + row.slope * (0.0 - row.center_x);
            let y1 = y + row.slope * (width - row.center_x);
            out.push_str(&format!(
                "<line x1=\"0\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{color}\" \
                 stroke-width=\"1.5\" vector-effect=\"non-scaling-stroke\" opacity=\".85\"/>",
                short(y0),
                short(width),
                short(y1)
            ));
        }
    }
    let mut assigned = std::collections::BTreeSet::new();
    for (group, ch) in matched {
        let boxes: Vec<_> = group.iter().filter_map(|i| by_index.get(i)).collect();
        let (Some(x0), Some(y0), Some(x1), Some(y1)) = (
            boxes.iter().map(|c| c.bbox.x0).min(),
            boxes.iter().map(|c| c.bbox.y0).min(),
            boxes.iter().map(|c| c.bbox.x1).max(),
            boxes.iter().map(|c| c.bbox.y1).max(),
        ) else {
            continue;
        };
        assigned.extend(group.iter().copied());
        out.push_str(&format!(
            "<rect x=\"{x0}\" y=\"{y0}\" width=\"{}\" height=\"{}\" fill=\"none\" \
             stroke=\"#1f9d48\" stroke-width=\"2\" vector-effect=\"non-scaling-stroke\"/>\
             <text x=\"{x0}\" y=\"{}\" font-size=\"{}\" font-family=\"ui-monospace, monospace\" \
             font-weight=\"700\" fill=\"#1f9d48\" stroke=\"#fff\" stroke-width=\"{}\" \
             paint-order=\"stroke\">{}</text>",
            x1.saturating_sub(x0),
            y1.saturating_sub(y0),
            short((f64::from(y0) - label * 0.25).max(label)),
            short(label),
            short(label * 0.18),
            esc(&ch.to_string())
        ));
    }
    for c in &analysis.candidates {
        if assigned.contains(&c.index) {
            continue;
        }
        out.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"#e0452f\" \
             stroke-width=\"2\" stroke-dasharray=\"5 4\" vector-effect=\"non-scaling-stroke\"/>",
            c.bbox.x0,
            c.bbox.y0,
            c.bbox.x1.saturating_sub(c.bbox.x0),
            c.bbox.y1.saturating_sub(c.bbox.y0)
        ));
    }
    out
}

/// How a glyph was made, as a class of the page.
fn origin(glyph: &Glyph) -> &'static str {
    match glyph.provenance.as_ref().map(|p| p.source) {
        Some(Source::Traced) => "traced",
        Some(Source::Synthesized) => "synthesized",
        Some(Source::Composed) => "composed",
        _ => "manual",
    }
}

/// Whether the glyph needs a look or waits for review (what holds the
/// export back).
fn waits(glyph: &Glyph) -> bool {
    glyph.provenance.as_ref().is_some_and(|p| {
        p.needs_look() || (p.source == Source::Synthesized && p.review == Review::Pending)
    })
}

/// One cell per glyph with an outline, drawn from its flattened contours,
/// all at one scale: the frame spans the master's ascender and descender
/// and whatever reaches past them (accents above capitals).
fn glyph_cells(font: &Font, master: MasterId, words: &Words) -> (String, usize) {
    let (mut top, mut bottom) = font.masters.iter().find(|m| m.id == master).map_or_else(
        || {
            let em = f64::from(font.units_per_em);
            (em * 0.8, -em * 0.2)
        },
        |m| (m.metrics.ascender, m.metrics.descender),
    );
    let mut drawn = Vec::new();
    let mut total = 0;
    for id in &font.glyph_order {
        let Some(glyph) = font.glyphs.get(id) else {
            continue;
        };
        if glyph.name == ".notdef" {
            continue;
        }
        let Some(layer) = glyph.layers.get(&master) else {
            continue;
        };
        let Ok(paths) = forma_geometry::flatten_layer(font, glyph.id, master) else {
            continue;
        };
        let Some(bounds) = forma_geometry::bbox(&paths) else {
            continue;
        };
        total += 1;
        if drawn.len() >= MAX_PAGE_GLYPHS {
            continue;
        }
        top = top.max(bounds.y1);
        bottom = bottom.min(bounds.y0);
        // A glyph without advance (a mark) is centered on its ink.
        let center = if layer.advance_width > 0.0 {
            layer.advance_width / 2.0
        } else {
            (bounds.x0 + bounds.x1) / 2.0
        };
        drawn.push((glyph, center, paths));
    }
    let margin = (top - bottom) * 0.04;
    let (top, bottom) = (top + margin, bottom - margin);
    let side = top - bottom;
    let mut cells = String::new();
    for (glyph, center, paths) in &drawn {
        let data: Vec<String> = paths
            .iter()
            .map(|p| rounded_path(&forma_geometry::path_to_svg(p)))
            .collect();
        let kind = origin(glyph);
        let pending = waits(glyph);
        let what = match kind {
            "traced" => words.traced,
            "synthesized" => words.synthesized,
            "composed" => words.composed,
            _ => words.manual,
        };
        let codes: Vec<String> = glyph
            .unicodes
            .iter()
            .map(|u| format!("U+{u:04X}"))
            .collect();
        let mut title = format!("{} · {what}", glyph.name);
        if !codes.is_empty() {
            title.push_str(&format!(" · {}", codes.join(" ")));
        }
        if pending {
            title.push_str(&format!(" · {}", words.pending));
        }
        cells.push_str(&format!(
            "<div class=\"g{}\" title=\"{}\"><span class=\"n\">{}</span><span class=\"d {kind}\">\
             </span><svg viewBox=\"{} {} {} {}\"><path transform=\"scale(1 -1)\" \
             fill-rule=\"nonzero\" d=\"{}\"/></svg></div>",
            if pending { " pending" } else { "" },
            esc(&title),
            esc(&glyph.name),
            short(center - side / 2.0),
            short(-top),
            short(side),
            short(side),
            data.join(" ")
        ));
    }
    (cells, total - drawn.len())
}

fn stage_detail(stage: &StageSummary, lang: PageLang) -> String {
    let n = |key: &str| {
        stage
            .counts
            .get(key)
            .and_then(Value::as_u64)
            .map_or_else(|| "?".to_owned(), |v| v.to_string())
    };
    let es = lang == PageLang::Es;
    match stage.name.as_str() {
        "trace" if es => format!("contornos desde la imagen: {} glifos", n("glyphs")),
        "trace" => format!("outlines from the image: {} glyphs", n("glyphs")),
        "synthesize" if es => format!(
            "lo que faltaba: {} hechos, {} por dibujar",
            n("made"),
            n("needs_design")
        ),
        "synthesize" => format!(
            "what was missing: {} made, {} to draw",
            n("made"),
            n("needs_design")
        ),
        "compose" if es => format!("letras con acento: {}", n("composed")),
        "compose" => format!("accented letters: {}", n("composed")),
        "space" if es => format!("espaciado: {} glifos", n("spaced")),
        "space" => format!("spacing: {} glyphs", n("spaced")),
        "kern" if es => format!("kerning: {} ajustes", n("commands")),
        "kern" => format!("kerning: {} adjustments", n("commands")),
        "check_collisions" if es => {
            format!("choques: {} pares, {} choques", n("pairs"), n("collisions"))
        }
        "check_collisions" => format!(
            "collisions: {} pairs, {} collisions",
            n("pairs"),
            n("collisions")
        ),
        "charset" if es => format!("cobertura: {} de {}", n("covered"), n("required")),
        "charset" => format!("coverage: {} of {}", n("covered"), n("required")),
        "validate" if es => format!(
            "validación: {} errores, {} avisos",
            n("errors"),
            n("warnings")
        ),
        "validate" => format!(
            "validation: {} errors, {} warnings",
            n("errors"),
            n("warnings")
        ),
        _ => String::new(),
    }
}

fn pipeline_rows(stages: &[StageSummary], words: &Words, lang: PageLang) -> String {
    let longest = stages.iter().map(|s| s.ms).max().unwrap_or(0).max(1);
    let mut out = String::new();
    for stage in stages {
        let class = match stage.status.as_str() {
            "ok" => "ok",
            "warn" => "warn",
            "fail" => "fail",
            _ => "",
        };
        let detail = if stage.status == "skipped" {
            words.skipped.to_owned()
        } else {
            stage_detail(stage, lang)
        };
        #[allow(clippy::cast_precision_loss)]
        let share = (100.0 * stage.ms as f64 / longest as f64).max(1.5);
        out.push_str(&format!(
            "<div class=\"row\"><div class=\"name\"><span class=\"st {class}\"></span>{}\
             <small>{}</small></div><div class=\"bar\"><i style=\"--w:{}%\"></i></div>\
             <div class=\"ms\">{} ms</div></div>",
            esc(&stage.name),
            esc(&detail),
            short(share),
            stage.ms
        ));
    }
    out
}

/// The arguments of the call without the document handle, the revision
/// and folders: files by their names.
fn shown_args(args: &Value) -> String {
    let mut args = args.clone();
    let name_only = |value: &mut Value| {
        if let Some(text) = value.as_str() {
            let name = std::path::Path::new(text)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(text)
                .to_owned();
            *value = Value::String(name);
        }
    };
    if let Some(map) = args.as_object_mut() {
        map.remove("doc");
        map.remove("base_rev");
        map.remove("dry_run");
        if let Some(path) = map.get_mut("path") {
            name_only(path);
        }
        if let Some(export) = map
            .get_mut("stages")
            .and_then(|s| s.get_mut("export"))
            .and_then(Value::as_object_mut)
        {
            for key in ["ttf", "woff2", "html"] {
                if let Some(path) = export.get_mut(key) {
                    name_only(path);
                }
            }
        }
    }
    serde_json::to_string_pretty(&args).unwrap_or_default()
}

fn tester(words: &Words, layout: &[String], number: usize) -> String {
    let mut chips = String::new();
    let alphabet: Vec<&str> = layout
        .iter()
        .map(String::as_str)
        .filter(|r| !r.is_empty())
        .collect();
    let samples = words
        .samples
        .iter()
        .map(|(label, text)| ((*label).to_owned(), (*text).to_owned()))
        .chain(std::iter::once((
            words.alphabet.to_owned(),
            alphabet.join("\n"),
        )));
    for (i, (label, text)) in samples.enumerate() {
        chips.push_str(&format!(
            "<button class=\"chip\" data-t=\"{}\" aria-pressed=\"{}\">{}</button>",
            esc(&text),
            i == 0,
            esc(&label)
        ));
    }
    let mut inks = String::new();
    for (i, (color, label)) in INK_COLORS.iter().zip(words.inks).enumerate() {
        inks.push_str(&format!(
            "<button class=\"sw\" data-c=\"{color}\" style=\"background:{color}\" \
             aria-pressed=\"{}\" aria-label=\"{}\"></button>",
            i == 0,
            esc(label)
        ));
    }
    format!(
        "<section><div class=\"step-head\"><span class=\"num\">{number}</span><div>\
         <h2>{}</h2><p class=\"sub\">{}</p></div></div><div class=\"card tester\">\
         <div class=\"controls\"><div class=\"chips\" role=\"group\">{chips}</div>\
         <label class=\"size\">{} <input type=\"range\" id=\"fs\" min=\"20\" max=\"120\" \
         value=\"52\"> <span id=\"fsv\">52</span></label><div class=\"swatches\" \
         role=\"group\">{inks}</div></div><div class=\"paper hand\" id=\"paper\" \
         contenteditable=\"true\" spellcheck=\"false\" \
         style=\"--fs:52px;--lh:66px;--pen:#1c2456\"></div><div class=\"hint\">{}</div>\
         </div></section>",
        esc(words.s3_title),
        esc(words.s3_sub),
        esc(words.size),
        esc(words.s3_hint)
    )
}

fn attention_section(items: &[AttentionItem], words: &Words, number: usize) -> String {
    const SHOWN: usize = 50;
    let mut list = String::new();
    for item in items.iter().take(SHOWN) {
        let mut what = esc(&item.kind);
        if let Some(glyph) = &item.glyph {
            what.push_str(&format!(" · {}", esc(glyph)));
        }
        if let Some(row) = item.row {
            what.push_str(&format!(" · row {row}"));
        }
        list.push_str(&format!(
            "<li><code>{what}</code><span class=\"why\">{}</span></li>",
            esc(&item.reason)
        ));
    }
    if items.len() > SHOWN {
        list.push_str(&format!(
            "<li class=\"why\">{}</li>",
            esc(&words
                .more_attention
                .replace("{n}", &(items.len() - SHOWN).to_string()))
        ));
    }
    format!(
        "<section><div class=\"step-head\"><span class=\"num\">{number}</span><div>\
         <h2>{}</h2><p class=\"sub\">{}</p></div></div><div class=\"card\"><ul class=\"att\">\
         {list}</ul></div></section>",
        esc(words.s5_title),
        esc(words.s5_sub)
    )
}

/// Renders the page.
///
/// # Errors
///
/// [`ToolError`] with `IO_ERROR` if the image cannot be encoded. A font that
/// cannot be compiled leaves the text field out and says why.
pub(crate) fn render(input: &PageInput<'_>) -> Result<String, ToolError> {
    let words = match input.lang {
        PageLang::Es => &ES,
        PageLang::En => &EN,
    };
    let photo = STANDARD.encode(photo_jpeg(input.image)?);
    let (font_face, font_note) =
        match forma_compile::compile(input.font, forma_compile::OutputFormat::Woff2) {
            Ok((bytes, _)) => (
                format!(
                    "@font-face {{ font-family: \"FormaPage\"; src: url(data:font/woff2;base64,{}) \
                     format(\"woff2\"); font-display: block; }}",
                    STANDARD.encode(bytes)
                ),
                None,
            ),
            Err(error) => (String::new(), Some(error.to_string())),
        };
    let (glyphs, more) = glyph_cells(input.font, input.master, words);

    let stat = |value: String, label: &str| {
        format!(
            "<div class=\"stat\"><b>{}</b><span>{}</span></div>",
            esc(&value),
            esc(label)
        )
    };
    let opt = |v: Option<u64>| v.map_or_else(|| "–".to_owned(), |v| v.to_string());
    let stats = [
        stat(seconds(input.engine_ms, input.lang), words.stat_time),
        stat(
            opt(count(input.stages, "trace", "glyphs")),
            words.stat_traced,
        ),
        stat(
            opt(count(input.stages, "charset", "covered")),
            words.stat_covered,
        ),
        stat(
            opt(count(input.stages, "validate", "errors")),
            words.stat_errors,
        ),
    ]
    .concat();

    let mut number = 3;
    let tester_html = match &font_note {
        None => {
            let html = tester(words, input.layout, number);
            number += 1;
            html
        }
        Some(error) => format!(
            "<section><div class=\"banner\">{}</div></section>",
            esc(&words.no_font.replace("{e}", error))
        ),
    };
    let n_pipe = number;
    number += 1;
    let attention_html = if input.attention.is_empty() {
        String::new()
    } else {
        let html = attention_section(input.attention, words, number);
        number += 1;
        html
    };
    let n_args = number;

    let legend = [
        ("#1f9d48", words.legend_matched),
        ("#e0452f", words.legend_unmatched),
        ("#d63cd6", words.legend_cap),
        ("#1fb3d6", words.legend_base),
    ]
    .iter()
    .map(|(color, label)| {
        format!(
            "<span><i style=\"background:{color}\"></i>{}</span>",
            esc(label)
        )
    })
    .collect::<String>();
    let keys = [
        ("traced", words.traced),
        ("synthesized", words.synthesized),
        ("composed", words.composed),
        ("manual", words.manual),
    ]
    .iter()
    .map(|(class, label)| format!("<span><i class=\"d {class}\"></i>{}</span>", esc(label)))
    .chain(std::iter::once(format!(
        "<span><i class=\"pend\"></i>{}</span>",
        esc(words.pending)
    )))
    .collect::<String>();
    let banner = if input.pending > 0 {
        format!(
            "<div class=\"banner\">{}</div>",
            esc(&words.banner.replace("{n}", &input.pending.to_string()))
        )
    } else {
        String::new()
    };
    let glyphs_more = if more > 0 {
        format!(
            "<div class=\"more\">{}</div>",
            esc(&words.more_glyphs.replace("{n}", &more.to_string()))
        )
    } else {
        String::new()
    };
    let family = &input.font.info.family_name;
    let version = env!("CARGO_PKG_VERSION");

    let mut values: BTreeMap<&str, String> = BTreeMap::new();
    values.insert("LANG", words.lang.to_owned());
    values.insert("VERSION", esc(version));
    values.insert("TITLE", esc(&format!("{family} · FORMA")));
    values.insert("FONT_FACE", font_face);
    values.insert("EYEBROW", esc(words.eyebrow));
    values.insert("HERO", esc(words.hero));
    values.insert("LEDE", esc(words.lede));
    values.insert("FAMILY", esc(&format!("{}: {family}", words.family)));
    values.insert("STATS", stats);
    values.insert("BANNER", banner);
    values.insert("S1_TITLE", esc(words.s1_title));
    values.insert("S1_SUB", esc(words.s1_sub));
    values.insert("IMG_W", input.analysis.width.to_string());
    values.insert("IMG_H", input.analysis.height.to_string());
    values.insert("PHOTO", photo);
    values.insert("PHOTO_ALT", esc(words.photo_alt));
    values.insert("OVERLAY", overlay(input.analysis, input.matched));
    values.insert("TAG_PHOTO", esc(words.tag_photo));
    values.insert("TAG_SEEN", esc(words.tag_seen));
    values.insert("LEGEND", legend);
    values.insert("S2_TITLE", esc(words.s2_title));
    values.insert("S2_SUB", esc(words.s2_sub));
    values.insert("GLYPHS", glyphs);
    values.insert("GLYPHS_MORE", glyphs_more);
    values.insert("KEYS", keys);
    values.insert("TESTER", tester_html);
    values.insert("N_PIPE", n_pipe.to_string());
    values.insert("S4_TITLE", esc(words.s4_title));
    values.insert("S4_SUB", esc(words.s4_sub));
    values.insert("PIPE", pipeline_rows(input.stages, words, input.lang));
    values.insert("ATTENTION", attention_html);
    values.insert("N_ARGS", n_args.to_string());
    values.insert("S6_TITLE", esc(words.s6_title));
    values.insert("S6_SUB", esc(words.s6_sub));
    values.insert("ARGS", esc(&shown_args(input.args)));
    values.insert("FOOTER", esc(&words.footer.replace("{v}", version)));
    Ok(fill(TEMPLATE, &values))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_filled_once() {
        let mut values = BTreeMap::new();
        values.insert("A", "{{B}}".to_owned());
        values.insert("B", "no".to_owned());
        assert_eq!(fill("x{{A}}y{{C}}z{{", &values), "x{{B}}yz{{");
    }

    #[test]
    fn text_is_escaped() {
        assert_eq!(
            esc("<script>\"a\" & 'b'</script>"),
            "&lt;script&gt;&quot;a&quot; &amp; &#39;b&#39;&lt;/script&gt;"
        );
    }

    #[test]
    fn path_numbers_are_rounded() {
        assert_eq!(
            rounded_path("M10.04 -5.26 C1.25 2 3.333 -0.04 5 6 Z"),
            "M10 -5.3 C1.3 2 3.3 0 5 6 Z"
        );
    }

    #[test]
    fn seconds_use_the_decimal_mark_of_the_language() {
        assert_eq!(seconds(3721, PageLang::Es), "3,7 s");
        assert_eq!(seconds(3721, PageLang::En), "3.7 s");
    }

    #[test]
    fn the_args_show_file_names_only() {
        let args = serde_json::json!({
            "doc": "d1", "base_rev": 3, "path": "/home/someone/photos/a.jpg",
            "stages": { "export": { "html": "/home/someone/out/page.html" } }
        });
        let shown = shown_args(&args);
        assert!(!shown.contains("someone"), "{shown}");
        assert!(!shown.contains("base_rev"), "{shown}");
        assert!(
            shown.contains("a.jpg") && shown.contains("page.html"),
            "{shown}"
        );
    }
}
