//! A one-shot engine session: the sandbox the CLI grants, the CLI actor and
//! the `--yes` approval of plans.
//!
//! Nothing here knows about typography: every operation is an
//! [`Engine::call`] on a registry tool, made as [`Actor::Cli`].

use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use forma_engine::{Engine, EngineConfig, ErrorCode, Policy, ToolError, ToolOutput};
use forma_model::Actor;
use serde_json::{Value, json};

use crate::output::Failure;

/// The document a session opened.
#[derive(Debug)]
pub struct OpenedDoc {
    /// The `doc` handle.
    pub handle: String,
    /// The revision the document was opened at.
    pub rev: u64,
}

/// An engine for one command: documents opened by it live and die with the
/// process.
pub struct Session {
    engine: Engine,
    yes: bool,
}

impl Session {
    /// A session whose sandbox is the current directory plus the folder that
    /// holds each of `explicit_paths`: the human typed those paths, so they
    /// are the permission. With `yes`, plans that need approval are approved
    /// at once.
    pub fn new(explicit_paths: &[PathBuf], yes: bool) -> Result<Session, Failure> {
        let cwd = std::env::current_dir().context("cannot determine the current directory")?;
        let engine = Engine::new(EngineConfig {
            allowed_roots: sandbox_roots(&cwd, explicit_paths),
            policy: Policy::Prompted,
            ..EngineConfig::default()
        });
        Ok(Session { engine, yes })
    }

    /// Calls `tool` as the CLI. A call that needs approval is committed with
    /// `plan_commit` when the session has `yes`; otherwise it fails with
    /// [`Failure::NeedsApproval`], carrying the plan.
    pub fn call(&mut self, tool: &str, args: Value) -> Result<ToolOutput, Failure> {
        match self.engine.call(&Actor::Cli, tool, args) {
            Ok(output) => Ok(output),
            Err(error) if error.code == ErrorCode::ApprovalRequired => self.approve(error),
            Err(error) => Err(Failure::Tool(error)),
        }
    }

    /// [`Session::call`] for a tool that answers with JSON.
    pub fn call_json(&mut self, tool: &str, args: Value) -> Result<Value, Failure> {
        json_of(tool, self.call(tool, args)?)
    }

    /// Opens the document at `path` (`document_open`).
    pub fn open(&mut self, path: &Path) -> Result<OpenedDoc, Failure> {
        let opened = self.call_json("document_open", json!({ "path": path_value(path)? }))?;
        let handle = opened.get("doc").and_then(Value::as_str);
        let rev = opened.get("rev").and_then(Value::as_u64);
        match (handle, rev) {
            (Some(handle), Some(rev)) => Ok(OpenedDoc {
                handle: handle.to_owned(),
                rev,
            }),
            _ => Err(anyhow!("document_open returned an unexpected result: {opened}").into()),
        }
    }

    /// Handles `APPROVAL_REQUIRED`: the plan is the human's to approve, and
    /// `--yes` is that approval.
    fn approve(&mut self, error: ToolError) -> Result<ToolOutput, Failure> {
        let plan = error.details.get("plan").cloned();
        let Some(plan) = plan else {
            return Err(Failure::Tool(error));
        };
        if !self.yes {
            return Err(Failure::NeedsApproval(plan));
        }
        let Some(token) = plan.get("plan_token").and_then(Value::as_str) else {
            return Err(Failure::Tool(error));
        };
        self.engine
            .call(&Actor::Cli, "plan_commit", json!({ "plan_token": token }))
            .map_err(Failure::Tool)
    }
}

/// The JSON of a tool's result.
pub fn json_of(tool: &str, output: ToolOutput) -> Result<Value, Failure> {
    match output {
        ToolOutput::Json(value) => Ok(value),
        ToolOutput::Image { .. } => Err(anyhow!("{tool} returned an image, not JSON").into()),
    }
}

/// An absolute path as a JSON string, for a tool argument. Paths that are not
/// valid UTF-8 cannot be sent to a tool (and are refused rather than changed
/// into another path).
pub fn path_value(path: &Path) -> Result<Value, Failure> {
    let absolute = std::path::absolute(path)
        .with_context(|| format!("cannot resolve the path '{}'", path.display()))?;
    match absolute.to_str() {
        Some(text) => Ok(Value::String(text.to_owned())),
        None => Err(Failure::Usage(format!(
            "the path '{}' is not valid UTF-8",
            path.display()
        ))),
    }
}

/// The allowed roots of a CLI session: the working directory, first (relative
/// paths resolve against it), then the existing folder that holds each of
/// `paths`. The engine canonicalizes them.
fn sandbox_roots(cwd: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = vec![cwd.to_path_buf()];
    for path in paths {
        let Ok(absolute) = std::path::absolute(path) else {
            continue;
        };
        if let Some(parent) = absolute.parent()
            && parent.is_dir()
            && !roots.iter().any(|root| root == parent)
        {
            roots.push(parent.to_path_buf());
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_are_the_cwd_then_the_folders_of_the_paths() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cwd = dir.path().join("cwd");
        let docs = dir.path().join("docs");
        std::fs::create_dir(&cwd).expect("mkdir");
        std::fs::create_dir(&docs).expect("mkdir");
        let roots = sandbox_roots(
            &cwd,
            &[
                docs.join("a.forma"),
                docs.join("b.forma"),
                // A target in a folder that does not exist: no root.
                dir.path().join("missing").join("c.forma"),
                // A path next to the cwd's own files: already a root.
                cwd.join("d.forma"),
            ],
        );
        assert_eq!(roots, vec![cwd, docs]);
    }

    #[test]
    fn paths_become_absolute_json_strings() {
        let value = path_value(Path::new("a.forma")).expect("path");
        let text = value.as_str().expect("string");
        assert!(Path::new(text).is_absolute());
        assert!(text.ends_with("a.forma"));
    }

    #[test]
    fn every_call_is_made_as_the_cli_actor() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut session = Session::new(&[dir.path().join("a.forma")], false).expect("session");
        let created = session
            .call_json("document_create", json!({ "family_name": "T" }))
            .expect("create");
        let doc = created["doc"].clone();
        session
            .call_json(
                "glyph_create",
                json!({ "doc": doc, "base_rev": 0, "name": "A", "advance_width": 500 }),
            )
            .expect("glyph_create");
        let history = session
            .call_json("history_list", json!({ "doc": doc }))
            .expect("history_list");
        assert_eq!(history["entries"][0]["actor"], json!("cli"), "{history}");
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_a_usage_error() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(OsStr::from_bytes(b"/tmp/\xff.forma"));
        assert!(matches!(path_value(path), Err(Failure::Usage(_))));
    }
}
