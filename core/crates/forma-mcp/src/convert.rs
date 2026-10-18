//! The two translations between the registry and MCP: a [`CommandSpec`]
//! becomes a [`Tool`], and an engine outcome becomes a [`CallToolResult`].
//! Nothing here decides anything about a call; the engine already has.

use std::borrow::Cow;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use forma_engine::{CommandSpec, OutputKind, ToolError, ToolOutput, any_object_schema};
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, MetaObject, Tool, ToolAnnotations};
use serde_json::{Value, json};

/// The tools a client should load up front instead of through tool search:
/// the entry points of every session.
const ALWAYS_LOAD: [&str; 3] = ["forma_capabilities", "document_create", "document_open"];

/// The tools a client must confirm with the user before calling, even when
/// it otherwise runs tools without asking.
const REQUIRES_USER_INTERACTION: [&str; 1] = ["plan_commit"];

/// The MIME type of every preview image.
pub(crate) const PNG_MIME: &str = "image/png";

/// The MCP definition of `spec`: its name, title and description; its
/// argument schema; its result schema for JSON tools (image tools have
/// none); annotations from its effects (a closed world: FORMA never reaches
/// outside the open documents and the allowed roots); and the `_meta` hints
/// Anthropic clients read.
#[cfg(test)]
pub(crate) fn tool_of(spec: &CommandSpec) -> Tool {
    tool_with(spec, true)
}

/// [`tool_of`], leaving out the result schema unless `output_schema`. MCP
/// lets a tool without one still return `structuredContent`; leaving it out
/// makes the tool list much smaller for clients that load every tool.
pub(crate) fn tool_with(spec: &CommandSpec, output_schema: bool) -> Tool {
    let annotations = ToolAnnotations::with_title(spec.title)
        .read_only(spec.effects.read_only)
        .destructive(spec.effects.destructive)
        .idempotent(spec.effects.idempotent)
        .open_world(false);
    let mut tool = Tool::new_with_raw(
        spec.name,
        Some(Cow::Borrowed(spec.description)),
        Arc::new(schema_object((spec.input_schema)())),
    )
    .with_title(spec.title)
    .with_annotations(annotations);
    if output_schema
        && spec.output == OutputKind::Json
        && let Some(output_schema) = spec.output_schema
    {
        tool = tool.with_raw_output_schema(Arc::new(schema_object(output_schema())));
    }
    let mut meta = JsonObject::new();
    if ALWAYS_LOAD.contains(&spec.name) {
        meta.insert("anthropic/alwaysLoad".into(), Value::Bool(true));
    }
    if REQUIRES_USER_INTERACTION.contains(&spec.name) {
        meta.insert(
            "anthropic/requiresUserInteraction".into(),
            Value::Bool(true),
        );
    }
    if !meta.is_empty() {
        tool = tool.with_meta(MetaObject(meta));
    }
    tool
}

/// A registry schema as the JSON object MCP carries. Every registered schema
/// is an object (the engine tests it); anything else would become the
/// any-object schema rather than an invalid tool definition.
fn schema_object(schema: Value) -> JsonObject {
    match schema {
        Value::Object(object) => object,
        _ => match any_object_schema() {
            Value::Object(object) => object,
            _ => JsonObject::new(),
        },
    }
}

/// The MCP result of an engine call:
/// * JSON: `structuredContent` plus the same JSON (compact) as the only text
///   block, for clients that read only `content`;
/// * an image: the PNG (standard base64) and its JSON summary as text, with
///   **no** `structuredContent`, because some clients (Codex) drop every
///   content block when structured content is present;
/// * an error: `isError` with one text block `{"code", "message", "details"}`
///   and no structured content, so the model can read the stable code and
///   correct its call.
pub(crate) fn call_result(outcome: Result<ToolOutput, ToolError>) -> CallToolResult {
    match outcome {
        Ok(ToolOutput::Json(value)) => {
            let mut result = CallToolResult::success(vec![ContentBlock::text(value.to_string())]);
            result.structured_content = Some(value);
            result
        }
        Ok(ToolOutput::Image { png, summary }) => CallToolResult::success(vec![
            ContentBlock::image(BASE64.encode(png), PNG_MIME),
            ContentBlock::text(summary.to_string()),
        ]),
        Err(error) => {
            CallToolResult::error(vec![ContentBlock::text(error_json(&error).to_string())])
        }
    }
}

/// `{"code", "message", "details"}` of an engine error.
pub(crate) fn error_json(error: &ToolError) -> Value {
    json!({
        "code": error.code.as_str(),
        "message": error.message,
        "details": error.details,
    })
}

#[cfg(test)]
mod tests {
    use forma_engine::{ErrorCode, find, registry};

    use super::*;

    fn spec(name: &str) -> &'static CommandSpec {
        find(name).unwrap_or_else(|| panic!("no tool {name}"))
    }

    #[test]
    fn json_results_carry_structured_content_and_the_same_text() {
        let value = json!({ "rev": 3, "names": ["a", "b"] });
        let result = call_result(Ok(ToolOutput::Json(value.clone())));
        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.structured_content, Some(value.clone()));
        assert_eq!(result.content.len(), 1);
        let text = &result.content[0].as_text().expect("text").text;
        assert!(!text.contains(char::is_whitespace), "compact JSON: {text}");
        assert_eq!(serde_json::from_str::<Value>(text).expect("JSON"), value);
    }

    #[test]
    fn images_carry_base64_png_and_summary_without_structured_content() {
        let result = call_result(Ok(ToolOutput::Image {
            png: vec![0x89, b'P', b'N', b'G', 0xff],
            summary: json!({ "rev": 1, "glyph": "a" }),
        }));
        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.structured_content, None);
        assert_eq!(result.content.len(), 2);
        let image = result.content[0].as_image().expect("image");
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.data, "iVBOR/8=", "standard base64 with padding");
        let text = &result.content[1].as_text().expect("text").text;
        assert_eq!(
            serde_json::from_str::<Value>(text).expect("JSON"),
            json!({ "rev": 1, "glyph": "a" })
        );
    }

    #[test]
    fn errors_are_is_error_with_code_message_and_details() {
        let error = ToolError::new(ErrorCode::RevisionConflict, "stale")
            .with_details(json!({ "current_rev": 4 }));
        let result = call_result(Err(error));
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content, None);
        assert_eq!(result.content.len(), 1);
        let text = &result.content[0].as_text().expect("text").text;
        let parsed: Value = serde_json::from_str(text).expect("JSON");
        assert_eq!(
            parsed,
            json!({ "code": "REVISION_CONFLICT", "message": "stale",
                    "details": { "current_rev": 4 } })
        );

        let plain = call_result(Err(ToolError::new(ErrorCode::NotFound, "gone")));
        let text = &plain.content[0].as_text().expect("text").text;
        let parsed: Value = serde_json::from_str(text).expect("JSON");
        assert_eq!(parsed["details"], Value::Null);
    }

    #[test]
    fn tools_mirror_their_specs() {
        for spec in registry() {
            let tool = tool_of(spec);
            assert_eq!(tool.name, spec.name);
            assert_eq!(tool.title.as_deref(), Some(spec.title));
            assert_eq!(tool.description.as_deref(), Some(spec.description));
            assert_eq!(
                Value::Object(tool.input_schema.as_ref().clone()),
                (spec.input_schema)()
            );
            let annotations = tool.annotations.expect("annotations");
            assert_eq!(annotations.read_only_hint, Some(spec.effects.read_only));
            assert_eq!(annotations.destructive_hint, Some(spec.effects.destructive));
            assert_eq!(annotations.idempotent_hint, Some(spec.effects.idempotent));
            assert_eq!(annotations.open_world_hint, Some(false));
        }
        assert!(tool_of(spec("preview_glyph")).output_schema.is_none());
        assert!(tool_of(spec("document_list")).output_schema.is_some());
    }

    #[test]
    fn meta_marks_entry_points_and_the_commit() {
        for spec in registry() {
            let meta = tool_of(spec).meta.map(|meta| Value::Object(meta.0));
            let expected = match spec.name {
                "forma_capabilities" | "document_create" | "document_open" => {
                    Some(json!({ "anthropic/alwaysLoad": true }))
                }
                "plan_commit" => Some(json!({ "anthropic/requiresUserInteraction": true })),
                _ => None,
            };
            assert_eq!(meta, expected, "{}", spec.name);
        }
    }

    #[test]
    fn a_schema_that_is_not_an_object_becomes_the_any_object_schema() {
        assert_eq!(
            Value::Object(schema_object(json!(true))),
            json!({ "type": "object" })
        );
        let object = json!({ "type": "object", "properties": {} });
        assert_eq!(Value::Object(schema_object(object.clone())), object);
    }
}
