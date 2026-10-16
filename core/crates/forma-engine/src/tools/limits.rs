//! Enforcement of the per-call limits of [`crate::limits`]: the size of text
//! arguments, the number of `edit_batch` operations and the number of outline
//! nodes.
//!
//! [`check_raw_args`] inspects the arguments as received, before they are
//! parsed into typed values, so that oversized input is refused before
//! anything is allocated for it. Nodes produced by parsing SVG path data are
//! counted afterwards, with [`check_nodes`].

use forma_edit::{Command, ContourAction};
use forma_model::Path;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::errors::{ErrorCode, ToolError};
use crate::limits::{MAX_BATCH_OPS, MAX_NODES, MAX_TEXT_BYTES};
use crate::tools::{parse_args, parse_args_ref};

/// Arguments nested deeper than this are refused. JSON text never gets here
/// that deep: serde_json stops parsing at 128 levels.
const MAX_DEPTH: usize = 128;

/// Keys longer than this are shortened when an argument path is reported.
const MAX_KEY_IN_PATH: usize = 64;

/// The key under which structured contours carry their nodes.
const NODES_KEY: &str = "nodes";

/// A `LIMIT_EXCEEDED` error with `details = { limit, actual }`.
#[must_use]
pub(crate) fn limit_exceeded(message: String, limit: usize, actual: usize) -> ToolError {
    ToolError::new(ErrorCode::LimitExceeded, message)
        .with_details(json!({ "limit": limit, "actual": actual }))
}

/// Refuses a call that carries more than [`MAX_NODES`] outline nodes in
/// total.
///
/// # Errors
///
/// `LIMIT_EXCEEDED` with `details = { limit, actual }`.
pub(crate) fn check_nodes(total: usize) -> Result<(), ToolError> {
    if total > MAX_NODES {
        return Err(limit_exceeded(
            format!(
                "the call carries {total} outline nodes; the limit is {MAX_NODES} per call: \
                 split the work into several calls"
            ),
            MAX_NODES,
            total,
        ));
    }
    Ok(())
}

/// Refuses an `edit_batch` with more than [`MAX_BATCH_OPS`] operations.
///
/// # Errors
///
/// `LIMIT_EXCEEDED` with `details = { limit, actual }`.
pub(crate) fn check_batch_ops(count: usize) -> Result<(), ToolError> {
    if count > MAX_BATCH_OPS {
        return Err(limit_exceeded(
            format!(
                "edit_batch carries {count} operations; the limit is {MAX_BATCH_OPS}: split \
                 the work into several batches"
            ),
            MAX_BATCH_OPS,
            count,
        ));
    }
    Ok(())
}

/// The number of nodes of the contours a command brings into the font.
#[must_use]
pub(crate) fn command_nodes(cmd: &Command) -> usize {
    match cmd {
        Command::CreateGlyph { paths, .. } | Command::SetOutline { paths, .. } => path_nodes(paths),
        Command::ContourEdit {
            action: ContourAction::Add { path },
            ..
        } => path.nodes.len(),
        _ => 0,
    }
}

/// The number of nodes of `paths`.
#[must_use]
pub(crate) fn path_nodes(paths: &[Path]) -> usize {
    paths.iter().map(|path| path.nodes.len()).sum()
}

/// Checks the arguments of a call as received, before they are parsed:
///
/// * no string value and no object key is longer than [`MAX_TEXT_BYTES`];
/// * the structured contours (arrays under a `nodes` key, wherever they are)
///   carry at most [`MAX_NODES`] nodes in total.
///
/// # Errors
///
/// `LIMIT_EXCEEDED`: for text, `details = { limit, actual, argument }`, where
/// `argument` locates the text (for example `ops[1].args.svg_path`); for
/// nodes, `details = { limit, actual }`. `INVALID_ARGUMENT` for arguments
/// nested more than 128 levels deep.
pub(crate) fn check_raw_args(args: &Value) -> Result<(), ToolError> {
    Walk {
        path: Vec::new(),
        nodes: 0,
    }
    .value(args)
}

/// [`check_raw_args`], then [`parse_args`].
///
/// # Errors
///
/// The errors of both.
pub(crate) fn parse_checked<T: DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    check_raw_args(&args)?;
    parse_args(args)
}

/// [`check_raw_args`], then [`parse_args_ref`], keeping the arguments.
///
/// # Errors
///
/// The errors of both.
pub(crate) fn parse_checked_ref<T: DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    check_raw_args(args)?;
    parse_args_ref(args)
}

/// One step of the path to an argument.
enum Segment<'a> {
    Key(&'a str),
    Index(usize),
}

/// A depth-first walk over the raw arguments.
struct Walk<'a> {
    /// The path to the value being visited.
    path: Vec<Segment<'a>>,
    /// The structured nodes seen so far.
    nodes: usize,
}

impl<'a> Walk<'a> {
    fn value(&mut self, value: &'a Value) -> Result<(), ToolError> {
        if self.path.len() > MAX_DEPTH {
            return Err(ToolError::invalid_argument(format!(
                "invalid arguments: nested more than {MAX_DEPTH} levels deep"
            )));
        }
        match value {
            Value::String(text) => self.text(text.len(), None),
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    self.path.push(Segment::Index(index));
                    self.value(item)?;
                    self.path.pop();
                }
                Ok(())
            }
            Value::Object(map) => {
                for (key, item) in map {
                    self.text(key.len(), Some("an object key in "))?;
                    self.path.push(Segment::Key(key));
                    if key == NODES_KEY
                        && let Value::Array(nodes) = item
                    {
                        self.nodes = self.nodes.saturating_add(nodes.len());
                        check_nodes(self.nodes)?;
                    }
                    self.value(item)?;
                    self.path.pop();
                }
                Ok(())
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        }
    }

    /// Refuses text of `len` bytes at the current path if it is too long;
    /// `prefix` qualifies what the text is.
    fn text(&self, len: usize, prefix: Option<&str>) -> Result<(), ToolError> {
        if len <= MAX_TEXT_BYTES {
            return Ok(());
        }
        let argument = self.rendered_path();
        let what = format!("{}`{argument}`", prefix.unwrap_or("argument "));
        Err(ToolError::new(
            ErrorCode::LimitExceeded,
            format!("{what} is {len} bytes long; the limit is {MAX_TEXT_BYTES} bytes (4 MiB)"),
        )
        .with_details(json!({ "limit": MAX_TEXT_BYTES, "actual": len, "argument": argument })))
    }

    /// The current path as text, such as `ops[1].args.svg_path`; long keys
    /// are shortened.
    fn rendered_path(&self) -> String {
        let mut out = String::new();
        for segment in &self.path {
            match segment {
                Segment::Key(key) => {
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(shortened(key));
                    if key.len() > MAX_KEY_IN_PATH {
                        out.push('…');
                    }
                }
                Segment::Index(index) => {
                    out.push('[');
                    out.push_str(&index.to_string());
                    out.push(']');
                }
            }
        }
        out
    }
}

/// The first [`MAX_KEY_IN_PATH`] bytes of `key`, cut at a character boundary.
fn shortened(key: &str) -> &str {
    let mut end = key.len().min(MAX_KEY_IN_PATH);
    while !key.is_char_boundary(end) {
        end -= 1;
    }
    &key[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exceeded(args: &Value) -> ToolError {
        match check_raw_args(args) {
            Ok(()) => panic!("accepted {args}"),
            Err(error) => error,
        }
    }

    #[test]
    fn text_limit_reports_where_the_text_is() {
        let big = "x".repeat(MAX_TEXT_BYTES + 1);
        let error = exceeded(&json!({ "ops": [{ "args": {} }, { "args": { "svg_path": big } }] }));
        assert_eq!(error.code, ErrorCode::LimitExceeded);
        assert_eq!(
            error.details,
            json!({
                "limit": MAX_TEXT_BYTES,
                "actual": MAX_TEXT_BYTES + 1,
                "argument": "ops[1].args.svg_path"
            })
        );
        let mut args = serde_json::Map::new();
        args.insert(big.clone(), json!(1));
        let error = exceeded(&json!({ "outer": args }));
        assert_eq!(error.details["argument"], json!("outer"));
        assert!(error.message.starts_with("an object key in `outer`"));

        let exact = "x".repeat(MAX_TEXT_BYTES);
        assert_eq!(check_raw_args(&json!({ "svg_path": exact })), Ok(()));
    }

    #[test]
    fn long_keys_are_shortened_in_paths() {
        let key = "é".repeat(40);
        let mut inner = serde_json::Map::new();
        inner.insert(key, json!("x".repeat(MAX_TEXT_BYTES + 1)));
        let error = exceeded(&Value::Object(inner));
        let argument = error.details["argument"].as_str().expect("argument");
        assert_eq!(argument, format!("{}…", "é".repeat(32)));
    }

    #[test]
    fn structured_nodes_are_counted_everywhere() {
        let nodes = |count: usize| json!(vec![json!({ "x": 0, "y": 0, "kind": "line" }); count]);
        let ok = json!({
            "paths": [{ "nodes": nodes(50_000) }],
            "action": { "path": { "nodes": nodes(50_000) } }
        });
        assert_eq!(check_raw_args(&ok), Ok(()));
        let error = exceeded(&json!({
            "paths": [{ "nodes": nodes(50_000) }, { "nodes": nodes(50_001) }]
        }));
        assert_eq!(
            error.details,
            json!({ "limit": MAX_NODES, "actual": 100_001 })
        );
    }

    #[test]
    fn deep_nesting_is_refused() {
        let mut value = json!(1);
        for _ in 0..200 {
            value = json!([value]);
        }
        assert_eq!(exceeded(&value).code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn counts_nodes_of_incoming_contours() {
        use forma_edit::GlyphRef;
        use forma_model::{Node, NodeKind, PathId};
        let path = Path {
            id: PathId(0),
            closed: false,
            nodes: vec![
                Node {
                    x: 0.0,
                    y: 0.0,
                    kind: NodeKind::Line,
                    smooth: false,
                };
                3
            ],
        };
        let glyph = GlyphRef::Name("a".into());
        assert_eq!(
            command_nodes(&Command::SetOutline {
                glyph: glyph.clone(),
                master: None,
                paths: vec![path.clone(), path.clone()],
            }),
            6
        );
        assert_eq!(
            command_nodes(&Command::ContourEdit {
                glyph: glyph.clone(),
                master: None,
                action: ContourAction::Add { path },
            }),
            3
        );
        assert_eq!(
            command_nodes(&Command::DeleteGlyphs {
                glyphs: vec![glyph]
            }),
            0
        );
        assert!(check_nodes(MAX_NODES).is_ok());
        assert!(check_batch_ops(MAX_BATCH_OPS).is_ok());
        assert_eq!(
            check_batch_ops(MAX_BATCH_OPS + 1).map_err(|error| error.code),
            Err(ErrorCode::LimitExceeded)
        );
    }
}
