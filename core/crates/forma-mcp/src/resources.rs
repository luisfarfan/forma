//! Resources: read-only views of the open documents. Each one is read
//! through the registry tool that mirrors it (`document_list`, `glyph_get`,
//! `preview_glyph`), so a resource never shows anything a tool call would
//! not, and every read reaches the engine (its toolsets and its audit log)
//! like a tool call.
//!
//! * `forma://docs`: the JSON of `document_list`;
//! * `forma://doc/{doc}/glyph/{name}`: the JSON of `glyph_get` (default
//!   master);
//! * `forma://doc/{doc}/glyph/{name}/preview.png`: the PNG of
//!   `preview_glyph` (default size and master).
//!
//! Path segments are percent-decoded (RFC 3986), so a glyph name with `/`,
//! `%`, `?` or `#` is written `%2F`, `%25`, `%3F` or `%23`. A URI that names
//! nothing, or a document or glyph that does not exist, is an MCP error with
//! code -32602 (invalid params, the "resource not found" code of 2026-07-28).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use forma_engine::{ErrorCode, ToolError, ToolOutput};
use rmcp::model::{ErrorData, Resource, ResourceContents, ResourceTemplate};
use serde_json::{Value, json};

use crate::convert::{PNG_MIME, error_json};

const SCHEME: &str = "forma://";
/// The URI of the list of open documents.
const DOCS_URI: &str = "forma://docs";
const GLYPH_TEMPLATE: &str = "forma://doc/{doc}/glyph/{name}";
const PREVIEW_TEMPLATE: &str = "forma://doc/{doc}/glyph/{name}/preview.png";
const JSON_MIME: &str = "application/json";

/// The registry tools resources are read through.
const DOCUMENT_LIST: &str = "document_list";
const GLYPH_GET: &str = "glyph_get";
const PREVIEW_GLYPH: &str = "preview_glyph";

/// The most characters of a URI an error message repeats.
const MAX_ECHOED_URI_CHARS: usize = 200;

/// The static resources, each with the tool it is read through.
pub(crate) fn resources() -> Vec<(&'static str, Resource)> {
    vec![(
        DOCUMENT_LIST,
        Resource::new(DOCS_URI, "docs")
            .with_title("Open documents")
            .with_description(
                "The open documents with their `doc` handle, path, family name, current `rev` \
                 and whether they have unsaved changes (the JSON of document_list).",
            )
            .with_mime_type(JSON_MIME),
    )]
}

/// The resource templates, each with the tool its resources are read
/// through.
pub(crate) fn templates() -> Vec<(&'static str, ResourceTemplate)> {
    vec![
        (
            GLYPH_GET,
            ResourceTemplate::new(GLYPH_TEMPLATE, "glyph")
                .with_title("Glyph")
                .with_description(
                    "A glyph of the open document `doc`, by name, in the default master (the \
                     JSON of glyph_get). Percent-encode the name.",
                )
                .with_mime_type(JSON_MIME),
        ),
        (
            PREVIEW_GLYPH,
            ResourceTemplate::new(PREVIEW_TEMPLATE, "glyph-preview")
                .with_title("Glyph preview")
                .with_description(
                    "A PNG preview of a glyph of the open document `doc`, by name, at the \
                     default size and master (the image of preview_glyph). Percent-encode the \
                     name.",
                )
                .with_mime_type(PNG_MIME),
        ),
    ]
}

/// What a resource URI names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// `forma://docs`.
    Docs,
    /// `forma://doc/{doc}/glyph/{name}`.
    Glyph { doc: String, name: String },
    /// `forma://doc/{doc}/glyph/{name}/preview.png`.
    Preview { doc: String, name: String },
}

impl Target {
    /// Parses a resource URI.
    ///
    /// # Errors
    ///
    /// An invalid-params error for a URI that is not one of the three
    /// shapes, has a query or a fragment, or has a segment that is not valid
    /// percent-encoded UTF-8.
    pub(crate) fn parse(uri: &str) -> Result<Target, ErrorData> {
        let rest = uri.strip_prefix(SCHEME).ok_or_else(|| unknown(uri))?;
        if rest.contains(['?', '#']) {
            return Err(invalid(
                uri,
                format!(
                    "resource URI '{}' has a query or fragment, which FORMA resources do not \
                     take; percent-encode '?' and '#' in glyph names",
                    echo(uri)
                ),
            ));
        }
        if rest == "docs" {
            return Ok(Target::Docs);
        }
        let path = rest.strip_prefix("doc/").ok_or_else(|| unknown(uri))?;
        let segments: Vec<&str> = path.split('/').collect();
        let (doc, name, preview) = match segments.as_slice() {
            [doc, "glyph", name] => (*doc, *name, false),
            [doc, "glyph", name, "preview.png"] => (*doc, *name, true),
            _ => return Err(unknown(uri)),
        };
        let doc = decode(uri, doc)?;
        let name = decode(uri, name)?;
        if name.is_empty() {
            return Err(invalid(
                uri,
                format!("resource URI '{}' has an empty glyph name", echo(uri)),
            ));
        }
        Ok(if preview {
            Target::Preview { doc, name }
        } else {
            Target::Glyph { doc, name }
        })
    }

    /// The registry tool the resource is read through.
    pub(crate) fn tool(&self) -> &'static str {
        match self {
            Target::Docs => DOCUMENT_LIST,
            Target::Glyph { .. } => GLYPH_GET,
            Target::Preview { .. } => PREVIEW_GLYPH,
        }
    }

    /// The arguments of [`Target::tool`].
    pub(crate) fn args(&self) -> Value {
        match self {
            Target::Docs => json!({}),
            Target::Glyph { doc, name } | Target::Preview { doc, name } => {
                json!({ "doc": doc, "glyph": name })
            }
        }
    }
}

/// The contents of the resource `uri` from the output of its tool: JSON as
/// compact `application/json` text, an image as an `image/png` blob
/// (standard base64).
pub(crate) fn contents(uri: &str, output: ToolOutput) -> ResourceContents {
    match output {
        ToolOutput::Json(value) => {
            ResourceContents::text(value.to_string(), uri).with_mime_type(JSON_MIME)
        }
        ToolOutput::Image { png, .. } => {
            ResourceContents::blob(BASE64.encode(png), uri).with_mime_type(PNG_MIME)
        }
    }
}

/// The MCP error of a resource read the engine refused: invalid params
/// (-32602) when the URI names something that does not exist or cannot be
/// read here (an unknown handle or glyph, a disabled toolset), internal error
/// (-32603) for a file-system failure. The engine's `{code, message,
/// details}` travel as the error data.
pub(crate) fn read_error(uri: &str, error: &ToolError) -> ErrorData {
    let message = format!("cannot read '{}': {}", echo(uri), error.message);
    let data = Some(error_json(error));
    if error.code == ErrorCode::IoError {
        ErrorData::internal_error(message, data)
    } else {
        ErrorData::invalid_params(message, data)
    }
}

/// The error for a URI that names no FORMA resource.
fn unknown(uri: &str) -> ErrorData {
    invalid(
        uri,
        format!(
            "unknown resource '{}': expected {DOCS_URI}, {GLYPH_TEMPLATE} or {PREVIEW_TEMPLATE}",
            echo(uri)
        ),
    )
}

/// An invalid-params error about `uri`.
fn invalid(uri: &str, message: String) -> ErrorData {
    ErrorData::invalid_params(message, Some(json!({ "uri": echo(uri) })))
}

/// The first [`MAX_ECHOED_URI_CHARS`] characters of `uri`, for messages.
fn echo(uri: &str) -> String {
    let mut chars = uri.chars();
    let mut echoed: String = chars.by_ref().take(MAX_ECHOED_URI_CHARS).collect();
    if chars.next().is_some() {
        echoed.push('…');
    }
    echoed
}

/// The percent-decoded text of a URI segment.
fn decode(uri: &str, segment: &str) -> Result<String, ErrorData> {
    percent_decode(segment).ok_or_else(|| {
        invalid(
            uri,
            format!(
                "resource URI '{}' is not valid percent-encoded UTF-8",
                echo(uri)
            ),
        )
    })
}

/// Decodes `%XX` escapes (RFC 3986); `None` for a malformed escape or a
/// result that is not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let mut bytes = text.bytes();
    let mut decoded = Vec::with_capacity(text.len());
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = hex_digit(bytes.next()?)?;
            let low = hex_digit(bytes.next()?)?;
            decoded.push(high << 4 | low);
        } else {
            decoded.push(byte);
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use forma_engine::{OutputKind, find};
    use rmcp::model::ErrorCode as McpCode;

    use super::*;

    fn glyph(doc: &str, name: &str) -> Target {
        Target::Glyph {
            doc: doc.into(),
            name: name.into(),
        }
    }

    #[test]
    fn uris_parse_into_their_targets() {
        assert_eq!(Target::parse("forma://docs"), Ok(Target::Docs));
        assert_eq!(
            Target::parse("forma://doc/doc_1/glyph/A"),
            Ok(glyph("doc_1", "A"))
        );
        assert_eq!(
            Target::parse("forma://doc/doc_1/glyph/A/preview.png"),
            Ok(Target::Preview {
                doc: "doc_1".into(),
                name: "A".into()
            })
        );
        // A glyph may be called `preview.png`.
        assert_eq!(
            Target::parse("forma://doc/d/glyph/preview.png"),
            Ok(glyph("d", "preview.png"))
        );
        assert_eq!(
            Target::parse("forma://doc/d/glyph/a%2Fb%25c%3Fd%23e"),
            Ok(glyph("d", "a/b%c?d#e"))
        );
        assert_eq!(
            Target::parse("forma://doc/d/glyph/%C3%A9"),
            Ok(glyph("d", "é"))
        );
    }

    #[test]
    fn bad_uris_are_invalid_params() {
        for uri in [
            "",
            "forma://",
            "forma:/docs",
            "FORMA://docs",
            "file:///etc/passwd",
            "forma://docs/",
            "forma://doc/d",
            "forma://doc/d/glyph",
            "forma://doc/d/glyph/",
            "forma://doc/d/glyphs/A",
            "forma://doc/d/glyph/A/preview.svg",
            "forma://doc/d/glyph/A/preview.png/x",
            "forma://doc/d/glyph/a/b",
            "forma://doc/d/glyph/A?master=m",
            "forma://doc/d/glyph/A#top",
            "forma://doc/d/glyph/%",
            "forma://doc/d/glyph/%4",
            "forma://doc/d/glyph/%G0",
            "forma://doc/d/glyph/%FF",
        ] {
            let error = Target::parse(uri).expect_err(uri);
            assert_eq!(error.code, McpCode::INVALID_PARAMS, "{uri}");
            assert_eq!(error.code.0, -32602);
        }
    }

    #[test]
    fn targets_read_through_registered_read_only_tools() {
        let targets = [
            Target::Docs,
            glyph("d", "A"),
            Target::parse("forma://doc/d/glyph/A/preview.png").expect("parse"),
        ];
        for target in &targets {
            let spec = find(target.tool()).expect("registered");
            assert!(spec.effects.read_only, "{}", spec.name);
        }
        assert_eq!(
            find(PREVIEW_GLYPH).map(|s| s.output),
            Some(OutputKind::Image)
        );
        assert_eq!(targets[0].args(), json!({}));
        assert_eq!(targets[1].args(), json!({ "doc": "d", "glyph": "A" }));
        assert_eq!(targets[2].args(), json!({ "doc": "d", "glyph": "A" }));
        for (tool, _) in resources() {
            assert!(find(tool).is_some(), "{tool}");
        }
        for (tool, _) in templates() {
            assert!(find(tool).is_some(), "{tool}");
        }
    }

    #[test]
    fn long_uris_are_cut_in_messages() {
        let uri = format!("forma://{}", "x".repeat(10_000));
        let error = Target::parse(&uri).expect_err("unknown");
        assert!(error.message.len() < 600, "{}", error.message.len());
        assert!(error.message.contains('…'));
    }

    #[test]
    fn engine_refusals_map_to_invalid_params_and_io_failures_to_internal_errors() {
        let missing = read_error(
            "forma://docs",
            &ToolError::not_found("unknown document handle"),
        );
        assert_eq!(missing.code, McpCode::INVALID_PARAMS);
        assert!(missing.message.contains("unknown document handle"));
        assert_eq!(
            missing.data,
            Some(
                json!({ "code": "NOT_FOUND", "message": "unknown document handle",
                         "details": null })
            )
        );
        let io = read_error("forma://docs", &ToolError::new(ErrorCode::IoError, "disk"));
        assert_eq!(io.code, McpCode::INTERNAL_ERROR);
    }

    #[test]
    fn contents_follow_the_output_kind() {
        let text = contents("forma://docs", ToolOutput::Json(json!({ "documents": [] })));
        assert_eq!(
            text,
            ResourceContents::TextResourceContents {
                uri: "forma://docs".into(),
                mime_type: Some("application/json".into()),
                text: r#"{"documents":[]}"#.into(),
                meta: None,
            }
        );
        let blob = contents(
            "forma://doc/d/glyph/A/preview.png",
            ToolOutput::Image {
                png: vec![1, 2, 3],
                summary: json!({}),
            },
        );
        assert_eq!(
            blob,
            ResourceContents::BlobResourceContents {
                uri: "forma://doc/d/glyph/A/preview.png".into(),
                mime_type: Some("image/png".into()),
                blob: "AQID".into(),
                meta: None,
            }
        );
    }
}
