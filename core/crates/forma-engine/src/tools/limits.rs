//! Enforcement of the per-call limits of [`crate::limits`]: the size of text
//! arguments, the number of `edit_batch` operations and the number of outline
//! nodes.
//!
//! Two walks inspect the arguments as received, before they are parsed into
//! typed values, so that oversized input is refused before anything is
//! allocated for it:
//!
//! * [`check_raw_args`] (text size and nesting depth) runs in
//!   [`Engine::call`](crate::Engine::call) for **every** tool;
//! * [`check_raw_nodes`] (structured contour nodes) runs in the tools that
//!   take outlines, through [`parse_checked`] and `Mutation::parse`.
//!
//! Nodes produced by parsing SVG path data are counted afterwards, with
//! [`check_nodes`], and `edit_batch` counts its operations with
//! [`check_batch_ops`].

use std::collections::BTreeSet;

use forma_edit::{Command, ContourAction};
use forma_model::{Font, GlyphId, Path};
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

/// The number of outline nodes a command, applied to `font`, brings or
/// works on: the nodes of the contours it brings into the font and, for
/// `RemoveOverlap`, the current nodes of the layers it unites (each listed
/// glyph once). Other commands count 0.
#[must_use]
pub(crate) fn command_nodes(font: &Font, cmd: &Command) -> usize {
    match cmd {
        Command::CreateGlyph { paths, .. } | Command::SetOutline { paths, .. } => path_nodes(paths),
        Command::ContourEdit {
            action: ContourAction::Add { path },
            ..
        } => path.nodes.len(),
        Command::RemoveOverlap { glyphs, master } => {
            let Some(master) = master.or_else(|| font.masters.first().map(|master| master.id))
            else {
                return 0;
            };
            let ids: BTreeSet<GlyphId> = glyphs
                .iter()
                .filter_map(|glyph| glyph.resolve(font))
                .collect();
            ids.iter()
                .filter_map(|id| font.glyphs.get(id)?.layers.get(&master))
                .map(|layer| path_nodes(&layer.paths))
                .fold(0, usize::saturating_add)
        }
        _ => 0,
    }
}

/// The number of nodes of `paths`.
#[must_use]
pub(crate) fn path_nodes(paths: &[Path]) -> usize {
    paths.iter().map(|path| path.nodes.len()).sum()
}

/// Checks the arguments of any call as received, before they are parsed:
/// no string value and no object key is longer than [`MAX_TEXT_BYTES`], and
/// nothing is nested more than 128 levels deep.
/// [`Engine::call`](crate::Engine::call) runs it for every tool.
///
/// # Errors
///
/// `LIMIT_EXCEEDED` with `details = { limit, actual, argument }`, where
/// `argument` locates the text (for example `ops[1].args.svg_path`);
/// `INVALID_ARGUMENT` for arguments nested more than 128 levels deep.
pub(crate) fn check_raw_args(args: &Value) -> Result<(), ToolError> {
    Walk::new(Check::Text).value(args)
}

/// Checks that the structured contours of a call's raw arguments (arrays
/// under a `nodes` key, wherever they are) carry at most [`MAX_NODES`] nodes
/// in total.
///
/// # Errors
///
/// `LIMIT_EXCEEDED` with `details = { limit, actual }`; `INVALID_ARGUMENT`
/// for arguments nested more than 128 levels deep.
pub(crate) fn check_raw_nodes(args: &Value) -> Result<(), ToolError> {
    Walk::new(Check::Nodes).value(args)
}

/// [`check_raw_nodes`], then [`parse_args`].
///
/// # Errors
///
/// The errors of both.
pub(crate) fn parse_checked<T: DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    check_raw_nodes(&args)?;
    parse_args(args)
}

/// [`check_raw_nodes`], then [`parse_args_ref`], keeping the arguments.
///
/// # Errors
///
/// The errors of both.
pub(crate) fn parse_checked_ref<T: DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    check_raw_nodes(args)?;
    parse_args_ref(args)
}

/// One step of the path to an argument.
enum Segment<'a> {
    Key(&'a str),
    Index(usize),
}

/// What a [`Walk`] checks, besides the nesting depth.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
    /// The size of every string value and object key.
    Text,
    /// The total of the structured contour nodes.
    Nodes,
}

/// A depth-first walk over the raw arguments.
struct Walk<'a> {
    /// What the walk checks.
    check: Check,
    /// The path to the value being visited.
    path: Vec<Segment<'a>>,
    /// The structured nodes seen so far.
    nodes: usize,
}

impl<'a> Walk<'a> {
    fn new(check: Check) -> Walk<'a> {
        Walk {
            check,
            path: Vec::new(),
            nodes: 0,
        }
    }

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
                    if self.check == Check::Nodes
                        && key == NODES_KEY
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
        if self.check != Check::Text || len <= MAX_TEXT_BYTES {
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
        assert_eq!(check_raw_nodes(&ok), Ok(()));
        let too_many = json!({
            "paths": [{ "nodes": nodes(50_000) }, { "nodes": nodes(50_001) }]
        });
        let error = check_raw_nodes(&too_many).expect_err("too many nodes");
        assert_eq!(error.code, ErrorCode::LimitExceeded);
        assert_eq!(
            error.details,
            json!({ "limit": MAX_NODES, "actual": 100_001 })
        );
        // Counting nodes is the business of the tools that take outlines.
        assert_eq!(check_raw_args(&too_many), Ok(()));
        let big = json!({ "name": "x".repeat(MAX_TEXT_BYTES + 1) });
        assert_eq!(check_raw_nodes(&big), Ok(()));
    }

    #[test]
    fn deep_nesting_is_refused() {
        let mut value = json!(1);
        for _ in 0..200 {
            value = json!([value]);
        }
        assert_eq!(exceeded(&value).code, ErrorCode::InvalidArgument);
        assert_eq!(
            check_raw_nodes(&value).map_err(|error| error.code),
            Err(ErrorCode::InvalidArgument)
        );
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
        let font = Font::new("Test", 1000);
        assert_eq!(
            command_nodes(
                &font,
                &Command::SetOutline {
                    glyph: glyph.clone(),
                    master: None,
                    paths: vec![path.clone(), path.clone()],
                }
            ),
            6
        );
        assert_eq!(
            command_nodes(
                &font,
                &Command::ContourEdit {
                    glyph: glyph.clone(),
                    master: None,
                    action: ContourAction::Add { path },
                }
            ),
            3
        );
        assert_eq!(
            command_nodes(
                &font,
                &Command::DeleteGlyphs {
                    glyphs: vec![glyph]
                }
            ),
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
