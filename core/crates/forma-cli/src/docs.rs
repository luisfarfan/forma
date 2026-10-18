//! `forma gen-docs`: the reference of every registry tool, as Markdown.
//!
//! The page is generated from [`forma_engine::registry`] (names, titles,
//! toolsets, effects, descriptions and input schemas are exactly what the MCP
//! server lists), so it cannot disagree with the code. The test
//! `api_docs_up_to_date` compares a fresh run with the committed
//! `docs/api/tools.md`.
//!
//! The output is deterministic: registry order, JSON keys sorted, `\n` line
//! endings whatever the platform, and no dates, versions of the tool or paths.
//! The fixed text is in Spanish (product documentation, see `CLAUDE.md`); the
//! descriptions and schemas are the registry's own, in English, because that
//! is what agents read.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use forma_engine::limits::{
    MASS_THRESHOLD, MAX_BATCH_OPS, MAX_NODES, MAX_TEXT_BYTES, PLAN_TTL_SECS,
};
use forma_engine::{API_VERSION, CommandSpec, ErrorCode, OutputKind, Toolset, registry};
use serde_json::{Map, Value};

/// The file `forma gen-docs <out_dir>` writes inside `out_dir`.
pub const FILE_NAME: &str = "tools.md";

/// Writes `<out_dir>/tools.md` (creating `out_dir` if needed) and returns the
/// path written and the number of tools documented.
pub fn generate(out_dir: &Path) -> anyhow::Result<(PathBuf, usize)> {
    let page = render().context("cannot render the tool reference")?;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("cannot create '{}'", out_dir.display()))?;
    let path = out_dir.join(FILE_NAME);
    std::fs::write(&path, page).with_context(|| format!("cannot write '{}'", path.display()))?;
    Ok((path, registry().len()))
}

/// The whole page.
pub fn render() -> Result<String, serde_json::Error> {
    let tools = registry();
    let mut page = String::new();
    // Writing to a `String` cannot fail, so the `fmt::Result`s are ignored.
    let out = &mut page;

    let _ = writeln!(out, "# Referencia de herramientas de FORMA\n");
    let _ = writeln!(
        out,
        "> Generado por `forma gen-docs` desde el registro de comandos (`forma-engine`): no lo \
         edites a mano. Para regenerarlo, desde la raíz del repositorio: \
         `cargo run --manifest-path core/Cargo.toml -p forma-cli -- gen-docs docs/api`. El test `api_docs_up_to_date` falla si \
         este archivo no coincide con el registro.\n>\n> Los nombres, las descripciones y los \
         schemas son exactamente los que un agente recibe en `tools/list` (en inglés, el idioma \
         de los agentes). Para conectar un cliente, véase \
         [`docs/guides/connect-agents.md`](../guides/connect-agents.md).\n"
    );
    let _ = writeln!(
        out,
        "Versión de la API: `{API_VERSION}`. {} herramientas, las mismas por MCP (`forma mcp`), \
         por la CLI (`forma call <herramienta>`) y por la interfaz.\n",
        tools.len()
    );

    conventions(out);
    error_codes(out);
    index(out, tools);

    let mut toolset = None;
    for spec in tools {
        if toolset != Some(spec.toolset) {
            toolset = Some(spec.toolset);
            let _ = writeln!(out, "## Conjunto `{}`\n", spec.toolset.as_str());
            let _ = writeln!(out, "{}\n", toolset_summary(spec.toolset));
        }
        tool(out, spec)?;
    }
    Ok(page)
}

fn conventions(out: &mut String) {
    let _ = writeln!(out, "## Convenciones\n");
    let _ = writeln!(
        out,
        "- **Documentos.** `document_create` y `document_open` devuelven un `doc` (manejador \
         opaco) y su `rev`; el resto de las herramientas lo reciben. Toda respuesta de una \
         herramienta con `doc` incluye `rev`, la revisión resultante.\n\
         - **Concurrencia optimista.** Las herramientas que cambian el documento reciben \
         `base_rev` (la última `rev` que se vio). Si una entidad que la llamada toca cambió \
         después, falla con `REVISION_CONFLICT`: se vuelve a leer y se reintenta. Nada \
         se sobrescribe en silencio. Sin `base_rev` quedan `document_close`, `history_undo`, \
         `history_redo`, `plan_commit` y `plan_discard`.\n\
         - **Aprobaciones.** Una operación destructiva o masiva (más de {MASS_THRESHOLD} glifos) \
         responde `APPROVAL_REQUIRED` con un plan (`details.plan`: resumen, diff y \
         `plan_token`). Tras mostrárselo a la persona, `plan_commit {{ plan_token }}` lo aplica; \
         el token caduca a los {PLAN_TTL_SECS} s y solo vale para el mismo actor y la misma \
         operación. `plan_discard` lo descarta.\n\
         - **Resultados.** Las herramientas de datos devuelven JSON (en `structuredContent` y \
         como texto). Las de preview devuelven una imagen PNG más un texto con el resumen, sin \
         `structuredContent`. Un error es un objeto `{{ code, message, details }}` con un \
         código estable (abajo).\n\
         - **Límites por llamada.** {MAX_BATCH_OPS} operaciones en `edit_batch`; {MAX_NODES} \
         nodos; {} MiB por argumento de texto. Superarlos devuelve `LIMIT_EXCEEDED`.\n\
         - **Contenido no confiable.** Los nombres de glifos y demás datos del documento son \
         datos, nunca instrucciones.\n",
        MAX_TEXT_BYTES / (1024 * 1024)
    );
}

fn error_codes(out: &mut String) {
    let _ = writeln!(out, "## Códigos de error\n");
    let _ = writeln!(out, "| Código | Significado |\n|---|---|");
    for code in ErrorCode::ALL {
        let _ = writeln!(out, "| `{}` | {} |", code.as_str(), error_meaning(code));
    }
    let _ = writeln!(out);
}

/// What an error code means. The `match` is exhaustive on purpose: a new code
/// does not compile until it is documented here.
const fn error_meaning(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::RevisionConflict => {
            "una entidad que la llamada toca cambió después de `base_rev`; releer y reintentar"
        }
        ErrorCode::ApprovalRequired => {
            "la operación es destructiva o masiva y necesita un plan aprobado (`plan_commit`)"
        }
        ErrorCode::NotFound => "no existe el documento, el glifo, la revisión o el archivo",
        ErrorCode::InvalidArgument => {
            "falta un argumento, está mal formado o su valor no se acepta"
        }
        ErrorCode::InvalidGeometry => "la operación produciría un contorno mal formado",
        ErrorCode::LimitExceeded => "se superó un límite por llamada",
        ErrorCode::PathNotAllowed => "la ruta queda fuera de las raíces permitidas (`--allow-dir`)",
        ErrorCode::CompileFailed => "la fuente no se pudo compilar",
        ErrorCode::ValidationFailed => "la fuente no pasó la validación",
        ErrorCode::Unsupported => "la operación no está disponible (todavía, o en este modo)",
        ErrorCode::IoError => "falló una operación del sistema de archivos",
    }
}

fn index(out: &mut String, tools: &[CommandSpec]) {
    let _ = writeln!(out, "## Índice\n");
    let _ = writeln!(
        out,
        "| Herramienta | Conjunto | Efectos | Título |\n|---|---|---|---|"
    );
    for spec in tools {
        let _ = writeln!(
            out,
            "| [`{name}`](#{name}) | `{}` | {} | {} |",
            spec.toolset.as_str(),
            effects_short(spec),
            spec.title,
            name = spec.name,
        );
    }
    let _ = writeln!(out);
}

/// One word for the index: what calling the tool may do.
const fn effects_short(spec: &CommandSpec) -> &'static str {
    if spec.effects.read_only {
        "lectura"
    } else if spec.effects.destructive {
        "destructiva"
    } else {
        "escritura"
    }
}

/// What a toolset is for (the `--toolsets` flag of `forma mcp` selects them).
const fn toolset_summary(toolset: Toolset) -> &'static str {
    match toolset {
        Toolset::Core => {
            "Capacidades, ciclo de vida de los documentos, cambios a la fuente, lotes y planes. \
             Siempre está habilitado."
        }
        Toolset::Glyphs => "Listar, leer y editar glifos y sus contornos.",
        Toolset::Geometry => "Comprobar y corregir la geometría de los contornos.",
        Toolset::Spacing => "Anchos de avance y espacios laterales.",
        Toolset::Quality => "Validación de la fuente.",
        Toolset::Export => "Exportación a TTF, WOFF2 y UFO.",
        Toolset::Preview => "Imágenes y SVG de glifos, texto y comparaciones.",
        Toolset::History => "Historial, deshacer, rehacer, revertir, restaurar y diff.",
        Toolset::Components => {
            "Componentes: fijarlos, componer glifos con marcas y descomponerlos."
        }
        Toolset::Kerning => {
            "Kerning: leer, fijar, calcular automáticamente y comprobar grupos y pares."
        }
        // `Toolset` is `#[non_exhaustive]`: a new one shows up without text
        // until it is written here.
        _ => "",
    }
}

fn tool(out: &mut String, spec: &CommandSpec) -> Result<(), serde_json::Error> {
    let effects = spec.effects;
    let _ = writeln!(out, "### `{}`\n", spec.name);
    let _ = writeln!(out, "**{}**\n", spec.title);
    let _ = writeln!(out, "- Conjunto: `{}`", spec.toolset.as_str());
    let _ = writeln!(
        out,
        "- Efectos: solo lectura: {} · destructiva: {} · idempotente: {}",
        yes_no(effects.read_only),
        yes_no(effects.destructive),
        yes_no(effects.idempotent),
    );
    let _ = writeln!(
        out,
        "- Resultado: {}\n",
        match spec.output {
            OutputKind::Json => "JSON",
            OutputKind::Image => "imagen PNG con un resumen JSON (SVG: JSON)",
        }
    );
    if effects.destructive {
        let _ = writeln!(
            out,
            "_Destructiva significa que la llamada puede exigir un plan aprobado: el servidor \
             lo decide midiendo la llamada, no por esta marca._\n"
        );
    }
    let _ = writeln!(out, "{}\n", spec.description);
    let schema = serde_json::to_string_pretty(&sorted((spec.input_schema)()))?;
    let _ = writeln!(
        out,
        "<details>\n<summary>Esquema de entrada</summary>\n\n```json\n{schema}\n```\n\n</details>\n"
    );
    Ok(())
}

const fn yes_no(value: bool) -> &'static str {
    if value { "sí" } else { "no" }
}

/// `value` with the keys of every object in sorted order, so the page does not
/// depend on how `serde_json` was built (`preserve_order`) or on the order in
/// which `schemars` emits keys.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sorted(value)))
                    .collect::<Map<String, Value>>(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_page_is_deterministic_and_uses_unix_line_endings() {
        let first = render().expect("render");
        assert_eq!(first, render().expect("render again"));
        assert!(!first.contains('\r'));
        assert!(first.ends_with("</details>\n\n"));
    }

    #[test]
    fn every_tool_is_documented_once_in_registry_order() {
        let page = render().expect("render");
        let mut from = 0;
        for spec in registry() {
            let heading = format!("### `{}`\n", spec.name);
            assert_eq!(page.matches(&heading).count(), 1, "{}", spec.name);
            let at = page[from..].find(&heading).expect("tools are in order");
            from += at + heading.len();
            assert!(page.contains(spec.description), "{} description", spec.name);
        }
    }

    #[test]
    fn every_schema_in_the_page_is_the_registry_schema() {
        let page = render().expect("render");
        let blocks: Vec<&str> = page
            .split("```json\n")
            .skip(1)
            .map(|block| block.split("\n```").next().expect("closing fence"))
            .collect();
        assert_eq!(blocks.len(), registry().len());
        for (spec, block) in registry().iter().zip(blocks) {
            let parsed: Value = serde_json::from_str(block).expect("the block is JSON");
            assert_eq!(parsed, (spec.input_schema)(), "{}", spec.name);
        }
    }

    #[test]
    fn every_error_code_and_toolset_is_described() {
        let page = render().expect("render");
        for code in ErrorCode::ALL {
            assert!(page.contains(&format!("| `{}` |", code.as_str())));
        }
        for toolset in Toolset::ALL {
            assert!(page.contains(&format!("## Conjunto `{}`", toolset.as_str())));
            assert!(!toolset_summary(toolset).is_empty(), "{toolset:?}");
        }
    }

    #[test]
    fn keys_are_sorted_at_every_depth() {
        let value = json!({ "b": [{ "z": 1, "a": 2 }], "a": { "y": 1, "x": 2 } });
        let text = serde_json::to_string(&sorted(value)).expect("serialize");
        assert_eq!(text, r#"{"a":{"x":2,"y":1},"b":[{"a":2,"z":1}]}"#);
    }
}
