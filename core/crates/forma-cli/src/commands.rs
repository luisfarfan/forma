//! The subcommands. Each one is a short sequence of registry calls through a
//! [`Session`]; none holds typography logic.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use directories::ProjectDirs;
use forma_engine::{CommandSpec, EngineConfig, ErrorCode, ToolError, ToolOutput, Toolset, find};
use serde_json::{Value, json};

use crate::cli::{CallArgs, Command, ConvertArgs, ExportArgs, McpArgs, NewArgs, PathArg};
use crate::output::{EXIT_FAILURE, Exit, Failure, print_json, report};
use crate::session::{OpenedDoc, Session, path_value};

/// Runs `command` and returns the process exit code.
pub fn run(command: Command) -> ExitCode {
    let result = match command {
        Command::New(args) => new(&args),
        Command::Info(args) => info(&args),
        Command::Export(args) => export(&args),
        Command::Validate(args) => validate(&args),
        Command::Pack(args) => convert("pack", &args, forma_store::pack),
        Command::Unpack(args) => convert("unpack", &args, forma_store::unpack),
        Command::Call(args) => call(&args),
        Command::Mcp(args) => mcp(args),
    };
    ExitCode::from(result.unwrap_or_else(report))
}

/// `forma new`: `document_create` with a path.
fn new(args: &NewArgs) -> Exit {
    let mut session = Session::new(std::slice::from_ref(&args.path), false)?;
    let created = session.call_json(
        "document_create",
        json!({
            "family_name": args.family,
            "units_per_em": args.upm,
            "path": path_value(&args.path)?,
        }),
    )?;
    print_json(&created)?;
    Ok(0)
}

/// `forma info`: `document_open`, then `document_inspect`.
fn info(args: &PathArg) -> Exit {
    let mut session = Session::new(std::slice::from_ref(&args.path), false)?;
    let doc = session.open(&args.path)?;
    let inspected = session.call_json("document_inspect", json!({ "doc": doc.handle }))?;
    print_json(&inspected)?;
    Ok(0)
}

/// `forma export`: `document_open`, then `export_font`.
fn export(args: &ExportArgs) -> Exit {
    let mut session = Session::new(&[args.path.clone(), args.out.clone()], args.yes)?;
    let doc = session.open(&args.path)?;
    let exported = session.call_json(
        "export_font",
        json!({
            "doc": doc.handle,
            "base_rev": doc.rev,
            "format": args.format.as_str(),
            "path": path_value(&args.out)?,
        }),
    )?;
    print_json(&exported)?;
    Ok(0)
}

/// `forma validate`: `document_open`, then `validate`. The exit code is 0 if
/// the report has no errors and 1 if it has some; the report is printed
/// either way.
fn validate(args: &PathArg) -> Exit {
    let mut session = Session::new(std::slice::from_ref(&args.path), false)?;
    let doc = session.open(&args.path)?;
    let result = session.call_json("validate", json!({ "doc": doc.handle }))?;
    print_json(&result)?;
    match result.pointer("/report/errors").and_then(Value::as_u64) {
        Some(0) => Ok(0),
        Some(_) => Ok(EXIT_FAILURE),
        None => Err(anyhow::anyhow!("validate returned a report without an error count").into()),
    }
}

/// `forma pack` and `forma unpack`: pure format conversions, which have no
/// registry tool. The human names both paths, so there is no sandbox; but an
/// existing destination is only replaced with `--yes`, like an export.
fn convert(
    tool: &str,
    args: &ConvertArgs,
    conversion: fn(&Path, &Path) -> Result<(), forma_store::StoreError>,
) -> Exit {
    // `symlink_metadata`: a dangling link is something that would be replaced.
    if !args.yes && std::fs::symlink_metadata(&args.dst).is_ok() {
        return Err(Failure::NeedsApproval(json!({
            "tool": tool,
            "summary": format!("overwrites {}", args.dst.display()),
        })));
    }
    conversion(&args.src, &args.dst)
        .map_err(|error| Failure::Tool(forma_engine::errors::from_store_error(error)))?;
    print_json(&json!({
        "src": args.src.display().to_string(),
        "dst": args.dst.display().to_string(),
    }))?;
    Ok(0)
}

/// `forma call`: any registry tool.
fn call(args: &CallArgs) -> Exit {
    let mut call_args: Value = serde_json::from_str(&args.args)
        .map_err(|error| Failure::Usage(format!("--args is not valid JSON: {error}")))?;
    if call_args.is_null() {
        call_args = json!({});
    }

    // The paths the human gave are the sandbox: `--doc-path`, and a `path`
    // argument of the tool.
    let mut paths: Vec<PathBuf> = args.doc_path.iter().cloned().collect();
    if let Some(path) = call_args.get("path").and_then(Value::as_str) {
        paths.push(PathBuf::from(path));
    }
    let mut session = Session::new(&paths, args.yes)?;

    let spec = find(&args.tool);
    let doc = match &args.doc_path {
        Some(path) => {
            let doc = session.open(path)?;
            if let Some(spec) = spec {
                inject_session(&mut call_args, &(spec.input_schema)(), &doc);
            }
            Some(doc)
        }
        None => None,
    };

    match session.call(&args.tool, call_args)? {
        ToolOutput::Json(mut result) => {
            if args.out.is_some() {
                eprintln!(
                    "note: --out ignored: {} returned JSON, not an image",
                    args.tool
                );
            }
            if args.save
                && let (Some(spec), Some(doc)) = (spec, &doc)
                && let Some(saved) = save_if_changed(&mut session, spec, doc)?
                && let Value::Object(map) = &mut result
            {
                map.insert("saved".to_owned(), saved);
            }
            print_json(&result)?;
        }
        ToolOutput::Image { png, summary } => {
            let Some(out) = &args.out else {
                return Err(Failure::Usage(
                    "this tool returns an image; pass --out <file.png>".to_owned(),
                ));
            };
            std::fs::write(out, png).map_err(|error| {
                Failure::Tool(ToolError::io(format!(
                    "cannot write '{}': {error}",
                    out.display()
                )))
            })?;
            print_json(&summary)?;
        }
    }
    Ok(0)
}

/// Adds the opened document's `doc` handle and `base_rev` to `args`, for each
/// of them that the tool's input schema declares and `args` lacks.
fn inject_session(args: &mut Value, schema: &Value, doc: &OpenedDoc) {
    let Some(object) = args.as_object_mut() else {
        return;
    };
    if declares(schema, "doc") && !object.contains_key("doc") {
        object.insert("doc".to_owned(), Value::String(doc.handle.clone()));
    }
    if declares(schema, "base_rev") && !object.contains_key("base_rev") {
        object.insert("base_rev".to_owned(), Value::from(doc.rev));
    }
}

/// Whether a JSON Schema declares the object property `property`, directly or
/// in one of its `allOf`/`anyOf`/`oneOf` parts.
fn declares(schema: &Value, property: &str) -> bool {
    let direct = schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|properties| properties.contains_key(property));
    direct
        || ["allOf", "anyOf", "oneOf"].into_iter().any(|keyword| {
            schema
                .get(keyword)
                .and_then(Value::as_array)
                .is_some_and(|parts| parts.iter().any(|part| declares(part, property)))
        })
}

/// `--save`: after a call that can change the document, saves it to its file
/// if it has unsaved changes. Returns the `document_save` result, or `None`
/// if there was nothing to save.
fn save_if_changed(
    session: &mut Session,
    spec: &CommandSpec,
    doc: &OpenedDoc,
) -> Result<Option<Value>, Failure> {
    if spec.effects.read_only {
        return Ok(None);
    }
    let state = match session.call_json("document_inspect", json!({ "doc": doc.handle })) {
        Ok(state) => state,
        // The call closed the document: nothing is left to save.
        Err(Failure::Tool(error)) if error.code == ErrorCode::NotFound => return Ok(None),
        Err(failure) => return Err(failure),
    };
    if state.get("dirty").and_then(Value::as_bool) != Some(true) {
        return Ok(None);
    }
    let rev = state
        .get("rev")
        .and_then(Value::as_u64)
        .context("document_inspect returned no revision")?;
    let saved = session.call_json(
        "document_save",
        json!({ "doc": doc.handle, "base_rev": rev }),
    )?;
    Ok(Some(saved))
}

/// `forma mcp`: serves MCP on stdio until the client goes away.
fn mcp(args: McpArgs) -> Exit {
    let allowed_roots = if args.allow_dir.is_empty() {
        vec![std::env::current_dir().context("cannot determine the current directory")?]
    } else {
        args.allow_dir
    };
    let cfg = EngineConfig {
        allowed_roots,
        policy: args.policy.into(),
        audit_log: args.audit_log,
        journal_dir: journal_dir(args.journal_dir, args.no_journal),
        read_only: args.read_only,
        toolsets: args.toolsets.map(with_core),
    };

    if std::io::stdin().is_terminal() {
        eprintln!(
            "forma mcp: serving MCP on stdin/stdout; an MCP client starts this (Ctrl-D to quit)"
        );
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    let served = runtime.block_on(forma_mcp::serve_stdio(cfg));
    // The stdin reader is a blocking task: do not wait for it for long.
    runtime.shutdown_timeout(Duration::from_secs(2));
    served?;
    Ok(0)
}

/// `toolsets` plus `core`, without repeats, in registry order.
fn with_core(toolsets: Vec<Toolset>) -> Vec<Toolset> {
    Toolset::ALL
        .into_iter()
        .filter(|toolset| *toolset == Toolset::Core || toolsets.contains(toolset))
        .collect()
}

/// The crash-recovery journal folder: `explicit`, or the `recovery` folder of
/// the OS data directory for FORMA; created if missing. `None` if `disabled`,
/// if there is no data directory, or if the folder cannot be created (a
/// warning says so: the server still starts, without crash recovery).
fn journal_dir(explicit: Option<PathBuf>, disabled: bool) -> Option<PathBuf> {
    if disabled {
        return None;
    }
    let dir = explicit.or_else(default_journal_dir)?;
    match std::fs::create_dir_all(&dir) {
        Ok(()) => Some(dir),
        Err(error) => {
            tracing::warn!(
                dir = %dir.display(),
                %error,
                "cannot create the recovery journal folder; crash recovery is off"
            );
            None
        }
    }
}

/// `<OS data dir>/recovery` for the app `org.forma.forma`.
fn default_journal_dir() -> Option<PathBuf> {
    ProjectDirs::from("org", "forma", "forma").map(|dirs| dirs.data_dir().join("recovery"))
}

#[cfg(test)]
mod tests {
    use forma_engine::registry;

    use super::*;

    fn doc() -> OpenedDoc {
        OpenedDoc {
            handle: "doc_1".to_owned(),
            rev: 4,
        }
    }

    #[test]
    fn injection_follows_the_input_schema() {
        let mut args = json!({ "name": "A" });
        let schema = json!({ "properties": { "doc": {}, "base_rev": {}, "name": {} } });
        inject_session(&mut args, &schema, &doc());
        assert_eq!(args, json!({ "name": "A", "doc": "doc_1", "base_rev": 4 }));

        // Only what the schema declares.
        let mut args = json!({});
        inject_session(&mut args, &json!({ "properties": { "doc": {} } }), &doc());
        assert_eq!(args, json!({ "doc": "doc_1" }));
        let mut args = json!({});
        inject_session(&mut args, &json!({ "properties": {} }), &doc());
        assert_eq!(args, json!({}));
    }

    #[test]
    fn injection_never_overrides_the_caller() {
        let mut args = json!({ "doc": "doc_mine", "base_rev": 9 });
        let schema = json!({ "properties": { "doc": {}, "base_rev": {} } });
        inject_session(&mut args, &schema, &doc());
        assert_eq!(args, json!({ "doc": "doc_mine", "base_rev": 9 }));
    }

    #[test]
    fn schema_parts_count_as_declared() {
        let schema = json!({ "allOf": [{ "properties": { "doc": {} } }] });
        assert!(declares(&schema, "doc"));
        assert!(!declares(&schema, "base_rev"));
        assert!(!declares(&json!({ "type": "object" }), "doc"));
    }

    /// The registry's real schemas declare `doc` and `base_rev` where the
    /// tools take them: if a schema were reshaped so the detection missed, the
    /// injection would silently stop.
    #[test]
    fn registry_schemas_declare_doc_and_base_rev_as_the_tools_take_them() {
        const NO_DOC: [&str; 6] = [
            "forma_capabilities",
            "document_create",
            "document_open",
            "document_list",
            "plan_commit",
            "plan_discard",
        ];
        for spec in registry() {
            let schema = (spec.input_schema)();
            assert_eq!(
                declares(&schema, "doc"),
                !NO_DOC.contains(&spec.name),
                "{}: doc",
                spec.name
            );
            let mutating_with_base_rev = !spec.effects.read_only
                && !NO_DOC.contains(&spec.name)
                && !["document_close", "history_undo", "history_redo"].contains(&spec.name);
            if mutating_with_base_rev {
                assert!(declares(&schema, "base_rev"), "{}: base_rev", spec.name);
            }
        }
    }

    #[test]
    fn core_is_always_enabled_once_and_in_registry_order() {
        assert_eq!(
            with_core(vec![Toolset::History]),
            vec![Toolset::Core, Toolset::History]
        );
        assert_eq!(
            with_core(vec![
                Toolset::Export,
                Toolset::Core,
                Toolset::Glyphs,
                Toolset::Core
            ]),
            vec![Toolset::Core, Toolset::Glyphs, Toolset::Export]
        );
        assert_eq!(with_core(vec![]), vec![Toolset::Core]);
    }

    #[test]
    fn no_journal_wins_over_everything() {
        assert_eq!(journal_dir(None, true), None);
        let dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(journal_dir(Some(dir.path().join("j")), true), None);
        assert!(!dir.path().join("j").exists());
    }

    #[test]
    fn the_journal_folder_is_created() {
        let dir = tempfile::tempdir().expect("temp dir");
        let wanted = dir.path().join("a").join("b");
        assert_eq!(
            journal_dir(Some(wanted.clone()), false),
            Some(wanted.clone())
        );
        assert!(wanted.is_dir());
    }

    #[test]
    fn an_uncreatable_journal_folder_turns_the_journal_off() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("file");
        std::fs::write(&file, b"x").expect("write");
        assert_eq!(journal_dir(Some(file.join("sub")), false), None);
    }

    #[test]
    fn the_default_journal_folder_is_recovery_in_the_data_dir() {
        if let Some(dir) = default_journal_dir() {
            assert_eq!(
                dir.file_name().and_then(|name| name.to_str()),
                Some("recovery")
            );
            assert!(dir.to_string_lossy().to_lowercase().contains("forma"));
        }
    }
}
