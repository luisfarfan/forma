//! The `forma` binary, driven as a user (or an MCP client) would: through its
//! arguments, stdin, stdout, stderr and exit code.
//!
//! Exit codes under test: 0 success; 1 a tool error (the error JSON is on
//! stdout) or a failed `validate`; 2 a usage error, or an approval that needs
//! `--yes`.

use std::fs;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

use assert_cmd::Command;
use assert_cmd::assert::Assert;
use assert_cmd::cargo::cargo_bin_cmd;
use forma_engine::{Toolset, registry};
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;

// ------------------------------------------------------------------ harness

/// `forma` run in `cwd`, with no logging configuration inherited.
fn forma(cwd: &Path) -> Command {
    let mut cmd = cargo_bin_cmd!("forma");
    cmd.current_dir(cwd)
        .env_remove("FORMA_LOG")
        .env_remove("RUST_LOG")
        .timeout(std::time::Duration::from_secs(120));
    cmd
}

fn stdout_text(assert: &Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

/// The JSON document on stdout.
fn stdout_json(assert: &Assert) -> Value {
    let text = stdout_text(assert);
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("stdout is not JSON ({error}): {text:?}"))
}

fn temp() -> TempDir {
    tempfile::tempdir().expect("temp dir")
}

/// A closed rectangle, counter-clockwise (an outer contour).
const SQUARE: &str = "M100 0 L500 0 L500 700 L100 700 Z";

/// `forma call glyph_create ... --doc-path <doc> --save` for a glyph `name`.
fn add_glyph(cwd: &Path, doc: &str, name: &str, unicode: u32, svg_path: &str) {
    let args = json!({
        "name": name, "unicodes": [unicode], "advance_width": 600, "svg_path": svg_path
    });
    forma(cwd)
        .args(["call", "glyph_create", "--args", &args.to_string()])
        .args(["--doc-path", doc, "--save"])
        .assert()
        .success();
}

/// A new document `doc` (in `cwd`) with the glyph `A`.
fn font_with_a(cwd: &Path, doc: &str) {
    forma(cwd)
        .args(["new", doc, "--family", "Test Sans"])
        .assert()
        .success();
    add_glyph(cwd, doc, "A", 65, SQUARE);
}

fn glyph_count(cwd: &Path, doc: &str) -> u64 {
    let info = forma(cwd).args(["info", doc]).assert().success();
    stdout_json(&info)["glyph_count"]
        .as_u64()
        .expect("glyph_count")
}

// ----------------------------------------------------------- the user flow

#[test]
fn new_info_export_validate_flow() {
    let dir = temp();
    let cwd = dir.path();

    let new = forma(cwd)
        .args(["new", "a.forma", "--family", "Test Sans", "--upm", "2048"])
        .assert()
        .success();
    let created = stdout_json(&new);
    assert!(
        created["doc"]
            .as_str()
            .is_some_and(|d| d.starts_with("doc_")),
        "{created}"
    );
    assert_eq!(created["rev"], json!(0));
    assert!(
        created["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("a.forma")),
        "{created}"
    );
    assert!(cwd.join("a.forma").is_file());
    // Pretty JSON with 2-space indentation and a final newline.
    let text = stdout_text(&new);
    assert!(text.starts_with("{\n  \""), "{text:?}");
    assert!(text.ends_with("}\n"), "{text:?}");

    add_glyph(cwd, "a.forma", "A", 65, SQUARE);

    let info = forma(cwd).args(["info", "a.forma"]).assert().success();
    let info = stdout_json(&info);
    assert_eq!(info["info"]["family_name"], json!("Test Sans"));
    assert_eq!(info["units_per_em"], json!(2048));
    assert_eq!(info["glyph_count"], json!(1));

    for (format, out, magic) in [
        ("ttf", "a.ttf", Some(&[0u8, 1, 0, 0][..])),
        ("woff2", "a.woff2", Some(&b"wOF2"[..])),
        ("ufo", "a.ufo", None),
    ] {
        let export = forma(cwd)
            .args(["export", "a.forma", "--format", format, "-o", out])
            .assert()
            .success();
        let result = stdout_json(&export);
        assert_eq!(result["format"], json!(format), "{result}");
        assert!(result["path"].as_str().is_some_and(|p| p.ends_with(out)));
        match magic {
            Some(magic) => {
                let bytes = fs::read(cwd.join(out)).expect("exported file");
                assert!(
                    bytes.starts_with(magic),
                    "{out}: {:?}",
                    &bytes[..8.min(bytes.len())]
                );
            }
            None => assert!(cwd.join(out).join("fontinfo.plist").is_file()),
        }
    }

    let validate = forma(cwd).args(["validate", "a.forma"]).assert().success();
    let report = stdout_json(&validate);
    assert_eq!(report["report"]["errors"], json!(0), "{report}");
}

#[test]
fn pack_unpack_cli() {
    let dir = temp();
    let cwd = dir.path();
    font_with_a(cwd, "a.forma");

    let pack = forma(cwd)
        .args(["pack", "a.forma", "a.formapackage"])
        .assert()
        .success();
    assert!(stdout_json(&pack).is_object());
    assert!(cwd.join("a.formapackage").join("manifest.json").is_file());
    assert!(cwd.join("a.formapackage").join("font.json").is_file());

    forma(cwd)
        .args(["unpack", "a.formapackage", "b.forma"])
        .assert()
        .success();
    assert!(cwd.join("b.forma").is_file());

    let a = stdout_json(&forma(cwd).args(["info", "a.forma"]).assert().success());
    let b = stdout_json(&forma(cwd).args(["info", "b.forma"]).assert().success());
    assert_eq!(a["info"], b["info"]);
    assert_eq!(a["glyph_count"], b["glyph_count"]);
    assert_eq!(b["glyph_count"], json!(1));
}

#[test]
fn pack_over_an_existing_destination_needs_yes() {
    let dir = temp();
    let cwd = dir.path();
    font_with_a(cwd, "a.forma");
    forma(cwd)
        .args(["new", "b.forma", "--family", "Other"])
        .assert()
        .success();
    let before = fs::read(cwd.join("b.forma")).expect("read");

    let planned = forma(cwd)
        .args(["pack", "a.forma", "b.forma"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--yes"));
    assert_eq!(stdout_json(&planned)["tool"], json!("pack"));
    assert_eq!(fs::read(cwd.join("b.forma")).expect("read"), before);

    forma(cwd)
        .args(["pack", "a.forma", "b.forma", "--yes"])
        .assert()
        .success();
    let b = stdout_json(&forma(cwd).args(["info", "b.forma"]).assert().success());
    assert_eq!(b["info"]["family_name"], json!("Test Sans"));
}

#[test]
fn pack_errors_are_tool_errors_and_leave_no_files() {
    let dir = temp();
    let cwd = dir.path();
    font_with_a(cwd, "a.forma");

    // Unknown destination extension: INVALID_ARGUMENT, exit 1.
    let bad_dst = forma(cwd)
        .args(["pack", "a.forma", "out.txt"])
        .assert()
        .code(1);
    assert_eq!(stdout_json(&bad_dst)["code"], json!("INVALID_ARGUMENT"));
    assert!(!cwd.join("out.txt").exists());

    // A source that does not exist is not created by looking at it.
    forma(cwd)
        .args(["pack", "missing.forma", "x.formapackage"])
        .assert()
        .code(1);
    assert!(!cwd.join("missing.forma").exists());
    assert!(!cwd.join("x.formapackage").exists());
}

#[test]
fn call_injects_doc_and_base_rev() {
    let dir = temp();
    let cwd = dir.path();
    forma(cwd)
        .args(["new", "a.forma", "--family", "Test"])
        .assert()
        .success();

    let args = json!({
        "name": "A", "unicodes": [65], "advance_width": 600, "svg_path": SQUARE
    });
    let create = forma(cwd)
        .args(["call", "glyph_create", "--args", &args.to_string()])
        .args(["--doc-path", "a.forma", "--save"])
        .assert()
        .success();
    let result = stdout_json(&create);
    assert_eq!(result["rev"], json!(1), "{result}");
    assert_eq!(result["glyph"]["name"], json!("A"), "{result}");
    assert!(result["saved"]["path"].as_str().is_some(), "{result}");

    // Saved: a later process sees the glyph.
    assert_eq!(glyph_count(cwd, "a.forma"), 1);
    let list = forma(cwd)
        .args(["call", "glyph_list", "--doc-path", "a.forma"])
        .assert()
        .success();
    assert!(
        stdout_text(&list).contains("\"A\""),
        "{}",
        stdout_text(&list)
    );
}

#[test]
fn call_without_save_does_not_persist() {
    let dir = temp();
    let cwd = dir.path();
    font_with_a(cwd, "a.forma");

    let args = json!({ "name": "B", "advance_width": 500 });
    let create = forma(cwd)
        .args(["call", "glyph_create", "--args", &args.to_string()])
        .args(["--doc-path", "a.forma"])
        .assert()
        .success();
    assert_eq!(stdout_json(&create)["rev"], json!(1));
    assert!(stdout_json(&create).get("saved").is_none());
    assert_eq!(glyph_count(cwd, "a.forma"), 1);
}

#[test]
fn call_save_only_saves_what_changed() {
    let dir = temp();
    let cwd = dir.path();
    font_with_a(cwd, "a.forma");
    let saved = |tool: &str, args: &str| {
        let assert = forma(cwd)
            .args(["call", tool, "--args", args])
            .args(["--doc-path", "a.forma", "--save"])
            .assert()
            .success();
        stdout_json(&assert).get("saved").is_some()
    };

    // A read-only call has nothing to save.
    assert!(!saved("glyph_list", "{}"));
    // A dry run changes nothing either, although the tool is a mutation.
    assert!(!saved("glyph_delete", r#"{"glyphs":["A"],"dry_run":true}"#));
    // A real change is saved.
    assert!(saved("glyph_create", r#"{"name":"B","advance_width":500}"#));
}

#[test]
fn call_does_not_inject_doc_for_tools_without_doc() {
    let dir = temp();
    let cwd = dir.path();
    forma(cwd)
        .args(["new", "a.forma", "--family", "Test"])
        .assert()
        .success();

    // `forma_capabilities` has no `doc` property (and denies unknown fields):
    // injecting `doc` would make it fail.
    let capabilities = forma(cwd)
        .args(["call", "forma_capabilities", "--doc-path", "a.forma"])
        .assert()
        .success();
    assert_eq!(stdout_json(&capabilities)["api_version"], json!("1.0.0"));

    // What the caller provides is never overridden.
    let foreign = forma(cwd)
        .args([
            "call",
            "document_inspect",
            "--args",
            r#"{"doc":"doc_nope"}"#,
        ])
        .args(["--doc-path", "a.forma"])
        .assert()
        .code(1);
    assert_eq!(stdout_json(&foreign)["code"], json!("NOT_FOUND"));

    let args = json!({ "name": "B", "advance_width": 500, "base_rev": 7 });
    let stale = forma(cwd)
        .args(["call", "glyph_create", "--args", &args.to_string()])
        .args(["--doc-path", "a.forma"])
        .assert()
        .code(1);
    assert_eq!(stdout_json(&stale)["code"], json!("INVALID_ARGUMENT"));
}

#[test]
fn call_save_needs_a_document() {
    let dir = temp();
    forma(dir.path())
        .args(["call", "forma_capabilities", "--save"])
        .assert()
        .code(2);
}

#[test]
fn call_args_must_be_json() {
    let dir = temp();
    forma(dir.path())
        .args(["call", "forma_capabilities", "--args", "{nope"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--args"));
}

#[test]
fn call_tool_errors_exit_1_with_the_error_json() {
    let dir = temp();
    let unknown = forma(dir.path())
        .args(["call", "no_such_tool"])
        .assert()
        .code(1);
    let error = stdout_json(&unknown);
    assert_eq!(error["code"], json!("INVALID_ARGUMENT"));
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("unknown tool")),
        "{error}"
    );
    assert!(error.get("details").is_some());

    let missing = forma(dir.path())
        .args(["info", "missing.forma"])
        .assert()
        .code(1);
    assert_eq!(stdout_json(&missing)["code"], json!("NOT_FOUND"));
}

#[test]
fn new_refuses_an_existing_path() {
    let dir = temp();
    let cwd = dir.path();
    forma(cwd)
        .args(["new", "a.forma", "--family", "Test"])
        .assert()
        .success();
    let again = forma(cwd)
        .args(["new", "a.forma", "--family", "Other"])
        .assert()
        .code(1);
    assert_eq!(stdout_json(&again)["code"], json!("INVALID_ARGUMENT"));
}

#[test]
fn usage_errors_exit_2() {
    let dir = temp();
    let cwd = dir.path();
    // A required option is missing.
    forma(cwd).args(["export", "a.forma"]).assert().code(2);
    forma(cwd)
        .args(["export", "a.forma", "--format", "pdf", "-o", "x"])
        .assert()
        .code(2);
    forma(cwd).arg("frobnicate").assert().code(2);
    forma(cwd).assert().code(2);
}

#[test]
fn help_and_version() {
    let dir = temp();
    let help = forma(dir.path()).arg("--help").assert().success();
    let text = stdout_text(&help);
    for command in [
        "new", "info", "export", "validate", "pack", "unpack", "call", "mcp",
    ] {
        assert!(text.contains(command), "--help lacks {command}:\n{text}");
    }
    forma(dir.path())
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("forma "));
}

// -------------------------------------------------------------- approvals

#[test]
fn export_existing_target_needs_yes() {
    let dir = temp();
    let cwd = dir.path();
    font_with_a(cwd, "a.forma");
    fs::write(cwd.join("a.ttf"), b"precious").expect("write");

    // Without --yes: the plan on stdout, exit 2, a hint on stderr, no change.
    let planned = forma(cwd)
        .args(["export", "a.forma", "--format", "ttf", "-o", "a.ttf"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--yes"));
    let plan = stdout_json(&planned);
    assert_eq!(plan["tool"], json!("export_font"), "{plan}");
    assert!(plan["plan_token"].as_str().is_some(), "{plan}");
    assert!(
        plan["summary"]
            .as_str()
            .is_some_and(|s| s.contains("overwrites")),
        "{plan}"
    );
    assert_eq!(fs::read(cwd.join("a.ttf")).expect("read"), b"precious");

    // With --yes: approved, committed and replaced.
    let done = forma(cwd)
        .args([
            "export", "a.forma", "--format", "ttf", "-o", "a.ttf", "--yes",
        ])
        .assert()
        .success();
    let result = stdout_json(&done);
    assert_eq!(result["format"], json!("ttf"), "{result}");
    assert_eq!(result["plan_committed"], json!(true), "{result}");
    let bytes = fs::read(cwd.join("a.ttf")).expect("read");
    assert!(bytes.starts_with(&[0, 1, 0, 0]));
}

#[test]
fn call_destructive_needs_yes() {
    let dir = temp();
    let cwd = dir.path();
    font_with_a(cwd, "a.forma");
    let delete = |extra: &[&str]| {
        forma(cwd)
            .args(["call", "glyph_delete", "--args", r#"{"glyphs":["A"]}"#])
            .args(["--doc-path", "a.forma", "--save"])
            .args(extra)
            .assert()
    };

    let planned = delete(&[])
        .code(2)
        .stderr(predicate::str::contains("--yes"));
    let plan = stdout_json(&planned);
    assert_eq!(plan["tool"], json!("glyph_delete"), "{plan}");
    assert_eq!(glyph_count(cwd, "a.forma"), 1);

    let done = delete(&["--yes"]).success();
    assert_eq!(stdout_json(&done)["plan_committed"], json!(true));
    assert_eq!(glyph_count(cwd, "a.forma"), 0);
}

#[test]
fn call_dry_run_prints_the_plan_and_succeeds() {
    let dir = temp();
    let cwd = dir.path();
    font_with_a(cwd, "a.forma");
    let planned = forma(cwd)
        .args(["call", "glyph_delete"])
        .args(["--args", r#"{"glyphs":["A"],"dry_run":true}"#])
        .args(["--doc-path", "a.forma"])
        .assert()
        .success();
    assert_eq!(stdout_json(&planned)["plan"]["tool"], json!("glyph_delete"));
    assert_eq!(glyph_count(cwd, "a.forma"), 1);
}

// ----------------------------------------------------------------- images

#[test]
fn call_image_requires_out() {
    let dir = temp();
    let cwd = dir.path();
    font_with_a(cwd, "a.forma");
    let preview = |extra: &[&str]| {
        forma(cwd)
            .args([
                "call",
                "preview_glyph",
                "--args",
                r#"{"glyph":"A","size":64}"#,
            ])
            .args(["--doc-path", "a.forma"])
            .args(extra)
            .assert()
    };

    preview(&[])
        .code(2)
        .stderr(predicate::str::contains("--out"))
        .stdout(predicate::str::is_empty());

    let done = preview(&["--out", "a.png"]).success();
    let summary = stdout_json(&done);
    assert_eq!(summary["glyph"], json!("A"), "{summary}");
    assert_eq!(summary["size"], json!(64), "{summary}");
    let png = fs::read(cwd.join("a.png")).expect("png");
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

    // SVG previews are JSON: no --out needed.
    let svg = forma(cwd)
        .args([
            "call",
            "preview_glyph",
            "--args",
            r#"{"glyph":"A","format":"svg"}"#,
        ])
        .args(["--doc-path", "a.forma"])
        .assert()
        .success();
    assert!(stdout_json(&svg).is_object());
}

// --------------------------------------------------------------- validate

#[test]
fn validate_exit_code_reflects_errors() {
    let dir = temp();
    let cwd = dir.path();
    font_with_a(cwd, "ok.forma");
    let ok = forma(cwd).args(["validate", "ok.forma"]).assert().code(0);
    assert_eq!(stdout_json(&ok)["report"]["errors"], json!(0));

    // An open contour is an error.
    forma(cwd)
        .args(["new", "bad.forma", "--family", "Bad"])
        .assert()
        .success();
    add_glyph(cwd, "bad.forma", "A", 65, "M0 0 L100 0 L100 100");
    let bad = forma(cwd).args(["validate", "bad.forma"]).assert().code(1);
    let report = stdout_json(&bad);
    assert!(
        report["report"]["errors"].as_u64().is_some_and(|e| e > 0),
        "{report}"
    );
}

// ------------------------------------------------------------ the sandbox

#[test]
fn cli_paths_outside_cwd_are_allowed_when_given_explicitly() {
    let cwd = temp();
    let docs = temp();
    let exports = temp();
    let doc = docs.path().join("b.forma");
    let out = exports.path().join("b.ttf");
    let doc_text = doc.to_str().expect("utf-8");
    let out_text = out.to_str().expect("utf-8");

    forma(cwd.path())
        .args(["new", doc_text, "--family", "Away"])
        .assert()
        .success();
    assert!(doc.is_file());
    forma(cwd.path())
        .args(["info", doc_text])
        .assert()
        .success();
    forma(cwd.path())
        .args(["export", doc_text, "--format", "ttf", "-o", out_text])
        .assert()
        .success();
    assert!(out.is_file());
    forma(cwd.path())
        .args(["validate", doc_text])
        .assert()
        .success();

    // `call`: the --doc-path and a `path` argument are explicit too.
    forma(cwd.path())
        .args(["call", "document_inspect", "--doc-path", doc_text])
        .assert()
        .success();
    let args = json!({ "path": doc_text });
    forma(cwd.path())
        .args(["call", "document_open", "--args", &args.to_string()])
        .assert()
        .success();

    // Nothing was written next to the working directory.
    assert_eq!(fs::read_dir(cwd.path()).expect("read_dir").count(), 0);
}

#[test]
fn relative_paths_with_dotdot_resolve_against_cwd() {
    let dir = temp();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).expect("mkdir");
    forma(&sub)
        .args(["new", "../up.forma", "--family", "Up"])
        .assert()
        .success();
    assert!(dir.path().join("up.forma").is_file());
}

// --------------------------------------------------------------- parity

#[test]
fn parity_every_registry_tool_is_callable() {
    let dir = temp();
    let tools = registry();
    assert_eq!(tools.len(), 35);
    for spec in tools {
        let assert = forma(dir.path())
            .args(["call", spec.name, "--args", "{}"])
            .assert();
        let output = assert.get_output();
        let code = output.status.code();
        assert!(
            matches!(code, Some(0 | 1)),
            "{}: exit {code:?}, stderr {}",
            spec.name,
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stdout);
        let json: Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{}: stdout is not JSON ({error}): {text}", spec.name));
        if code == Some(1) {
            let message = json["message"].as_str().unwrap_or_default();
            assert!(
                !message.contains("unknown tool"),
                "{} is not wired to the registry: {json}",
                spec.name
            );
            assert!(json["code"].is_string(), "{}: {json}", spec.name);
        }
    }
}

// -------------------------------------------------------------------- mcp

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"cli-test","version":"1.0.0"}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
const TOOLS_LIST: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;

fn tools_call(id: u64, name: &str, arguments: &Value) -> String {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
    .to_string()
}

/// What `forma mcp` wrote.
struct McpRun {
    /// Every stdout line, each of which parsed as a JSON-RPC message.
    messages: Vec<Value>,
    stderr: String,
}

impl McpRun {
    /// The response to request `id`.
    fn response(&self, id: u64) -> &Value {
        self.messages
            .iter()
            .find(|message| message["id"] == json!(id))
            .unwrap_or_else(|| panic!("no response to request {id}: {:#?}", self.messages))
    }

    /// The tool names of the `tools/list` response (request 2).
    fn tool_names(&self) -> Vec<String> {
        self.response(2)["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("name").to_owned())
            .collect()
    }

    /// The `structuredContent` of the `tools/call` response `id`.
    fn structured(&self, id: u64) -> &Value {
        &self.response(id)["result"]["structuredContent"]
    }
}

/// Runs `forma mcp <args>` in `cwd` with `requests` on stdin, then closes
/// stdin and collects the output. Fails if stdout holds anything but
/// JSON-RPC.
fn run_mcp(cwd: &Path, args: &[&str], requests: &[String]) -> McpRun {
    let mut stdin = requests.join("\n");
    stdin.push('\n');
    let assert = forma(cwd).arg("mcp").args(args).write_stdin(stdin).assert();
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "forma mcp failed: {stderr}");
    let messages = stdout
        .lines()
        .map(|line| {
            let message: Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("stdout line is not JSON ({error}): {line:?}"));
            assert_eq!(message["jsonrpc"], json!("2.0"), "not JSON-RPC: {line}");
            message
        })
        .collect();
    McpRun { messages, stderr }
}

/// The standard opening: initialize, initialized, tools/list.
fn opening() -> Vec<String> {
    vec![
        INITIALIZE.to_owned(),
        INITIALIZED.to_owned(),
        TOOLS_LIST.to_owned(),
    ]
}

#[test]
fn mcp_stdout_is_protocol_only() {
    let dir = temp();
    let allowed = dir.path().to_str().expect("utf-8");
    // A root that does not exist makes the engine log a warning: logs must
    // reach stderr and never stdout.
    let missing = dir.path().join("missing");
    let run = run_mcp(
        dir.path(),
        &[
            "--allow-dir",
            allowed,
            "--allow-dir",
            missing.to_str().expect("utf-8"),
            "--no-journal",
        ],
        &opening(),
    );

    // Every stdout line is JSON-RPC (checked by `run_mcp`), and the session
    // worked: the handshake and the 35 tools.
    assert_eq!(
        run.response(1)["result"]["serverInfo"]["name"],
        json!("forma")
    );
    assert_eq!(run.tool_names().len(), 35);
    assert_eq!(run.messages.len(), 2, "{:#?}", run.messages);
    // stderr is for logs only: the warning is there, and no protocol.
    assert!(run.stderr.contains("WARN"), "{}", run.stderr);
    assert!(!run.stderr.contains("jsonrpc"), "{}", run.stderr);
}

#[test]
fn mcp_logging_follows_forma_log() {
    let dir = temp();
    let missing = dir.path().join("missing");
    let args = [
        "mcp",
        "--allow-dir",
        missing.to_str().expect("utf-8"),
        "--no-journal",
    ];
    let mut stdin = opening().join("\n");
    stdin.push('\n');
    let stderr_with = |filter: Option<&str>| {
        let mut cmd = forma(dir.path());
        if let Some(filter) = filter {
            cmd.env("FORMA_LOG", filter);
        }
        let assert = cmd.args(args).write_stdin(stdin.clone()).assert().success();
        String::from_utf8_lossy(&assert.get_output().stderr).into_owned()
    };

    // `warn` is the default; a stricter filter silences the warning.
    assert!(stderr_with(None).contains("WARN"));
    assert!(!stderr_with(Some("error")).contains("WARN"));
    assert!(stderr_with(Some("forma_engine=warn")).contains("WARN"));
}

#[test]
fn mcp_toolsets_always_include_core() {
    let dir = temp();
    let run = run_mcp(
        dir.path(),
        &["--toolsets", "history", "--no-journal"],
        &opening(),
    );
    let expected: Vec<&str> = registry()
        .iter()
        .filter(|spec| matches!(spec.toolset, Toolset::Core | Toolset::History))
        .map(|spec| spec.name)
        .collect();
    assert_eq!(run.tool_names(), expected);
    let names = run.tool_names();
    for core in [
        "forma_capabilities",
        "document_create",
        "document_open",
        "plan_commit",
    ] {
        assert!(names.iter().any(|name| name == core), "{core} missing");
    }
    assert!(!names.iter().any(|name| name == "glyph_list"));

    // Several toolsets, comma-separated.
    let several = run_mcp(
        dir.path(),
        &["--toolsets", "quality,export", "--no-journal"],
        &opening(),
    );
    let names = several.tool_names();
    for wanted in ["forma_capabilities", "validate", "export_font"] {
        assert!(names.iter().any(|name| name == wanted), "{wanted} missing");
    }
    assert!(!names.iter().any(|name| name == "history_list"));

    // Without the flag, everything.
    let all = run_mcp(dir.path(), &["--no-journal"], &opening());
    assert_eq!(all.tool_names().len(), 35);
}

#[test]
fn mcp_unknown_values_are_usage_errors() {
    let dir = temp();
    forma(dir.path())
        .args(["mcp", "--toolsets", "nope"])
        .assert()
        .code(2);
    forma(dir.path())
        .args(["mcp", "--policy", "reckless"])
        .assert()
        .code(2);
    forma(dir.path())
        .args(["mcp", "--no-journal", "--journal-dir", "x"])
        .assert()
        .code(2);
}

#[test]
fn mcp_flags_reach_the_engine() {
    let dir = temp();
    let audit = dir.path().join("audit.jsonl");
    let mut requests = opening();
    requests.push(tools_call(3, "forma_capabilities", &json!({})));
    let run = run_mcp(
        dir.path(),
        &[
            "--policy",
            "auto",
            "--read-only",
            "--audit-log",
            audit.to_str().expect("utf-8"),
            "--no-journal",
        ],
        &requests,
    );
    let capabilities = run.structured(3);
    assert_eq!(capabilities["policy"], json!("auto"), "{capabilities}");
    assert_eq!(capabilities["read_only"], json!(true), "{capabilities}");
    // Read-only: no mutating tool is listed.
    let names = run.tool_names();
    assert!(names.iter().any(|name| name == "glyph_list"));
    assert!(!names.iter().any(|name| name == "document_create"));

    let log = fs::read_to_string(&audit).expect("audit log");
    assert!(log.contains("forma_capabilities"), "{log}");
    // The actor of the MCP session is the client's name.
    assert!(log.contains("agent:cli-test"), "{log}");
}

#[test]
fn mcp_default_allow_dir_is_the_current_directory() {
    let cwd = temp();
    let elsewhere = temp();
    let outside = elsewhere.path().join("away.forma");
    let mut requests = opening();
    requests.push(tools_call(
        3,
        "document_create",
        &json!({ "family_name": "Here", "path": "here.forma" }),
    ));
    requests.push(tools_call(
        4,
        "document_create",
        &json!({ "family_name": "Away", "path": outside.to_str().expect("utf-8") }),
    ));
    let run = run_mcp(cwd.path(), &["--no-journal"], &requests);

    assert_eq!(run.response(3)["result"]["isError"], json!(false));
    assert!(cwd.path().join("here.forma").is_file());
    assert_eq!(run.response(4)["result"]["isError"], json!(true));
    let error = run.response(4)["result"]["content"][0]["text"]
        .as_str()
        .expect("error text")
        .to_owned();
    assert!(error.contains("PATH_NOT_ALLOWED"), "{error}");
    assert!(!outside.exists());
}

#[test]
fn mcp_allow_dir_is_repeatable() {
    let cwd = temp();
    let first = temp();
    let second = temp();
    let mut requests = opening();
    for (id, root) in [(3, &first), (4, &second)] {
        requests.push(tools_call(
            id,
            "document_create",
            &json!({
                "family_name": "F",
                "path": root.path().join("f.forma").to_str().expect("utf-8")
            }),
        ));
    }
    // The current directory is NOT allowed once --allow-dir is given.
    requests.push(tools_call(
        5,
        "document_create",
        &json!({ "family_name": "F", "path": "cwd.forma" }),
    ));
    let run = run_mcp(
        cwd.path(),
        &[
            "--allow-dir",
            first.path().to_str().expect("utf-8"),
            "--allow-dir",
            second.path().to_str().expect("utf-8"),
            "--no-journal",
        ],
        &requests,
    );
    assert_eq!(run.response(3)["result"]["isError"], json!(false));
    assert_eq!(run.response(4)["result"]["isError"], json!(false));
    assert!(first.path().join("f.forma").is_file());
    assert!(second.path().join("f.forma").is_file());
}

#[test]
fn mcp_journal_dir_is_created() {
    let dir = temp();
    let journal = dir.path().join("deep").join("journal");
    run_mcp(
        dir.path(),
        &["--journal-dir", journal.to_str().expect("utf-8")],
        &opening(),
    );
    assert!(journal.is_dir());
}

/// `…/recovery` somewhere under `root`. Only the Unix-only test of the default journal
/// folder uses it (it redirects `HOME`), so it is not built elsewhere.
#[cfg(unix)]
fn find_recovery_dir(root: &Path) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "recovery") {
                return Some(path);
            }
            if let Some(found) = find_recovery_dir(&path) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(unix)]
#[test]
fn mcp_default_journal_dir_is_in_the_os_data_dir() {
    let cwd = temp();
    let home = temp();
    let run = |extra: &[&str]| {
        let mut stdin = opening().join("\n");
        stdin.push('\n');
        forma(cwd.path())
            .env("HOME", home.path())
            .env("XDG_DATA_HOME", home.path().join("xdg"))
            .arg("mcp")
            .args(extra)
            .write_stdin(stdin)
            .assert()
            .success();
    };

    // --no-journal creates nothing.
    run(&["--no-journal"]);
    assert_eq!(find_recovery_dir(home.path()), None);

    // The default is the OS data dir of the app, plus `recovery`.
    run(&[]);
    let found = find_recovery_dir(home.path()).expect("recovery dir under the data dir");
    assert!(found.starts_with(home.path()));
    assert!(
        found.to_string_lossy().to_lowercase().contains("forma"),
        "{}",
        found.display()
    );
}

// ------------------------------------------------------- generated API docs

/// `docs/api/tools.md` is generated from the command registry by the hidden
/// `forma gen-docs`: this fails when a tool, a description or a schema changed
/// and the file was not regenerated. From the repository root:
/// `cargo run --manifest-path core/Cargo.toml -p forma-cli -- gen-docs docs/api`.
#[test]
fn api_docs_up_to_date() {
    let committed_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/tools.md");
    let committed = fs::read_to_string(&committed_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", committed_path.display()))
        // A Windows checkout may have turned the line endings into CRLF
        // (`.gitattributes` asks for LF, but do not depend on it).
        .replace("\r\n", "\n");

    let out = temp();
    let generated_dir = out.path().join("api");
    let run = forma(out.path())
        .arg("gen-docs")
        .arg(&generated_dir)
        .assert()
        .success();
    assert_eq!(
        stdout_json(&run)["tools"],
        json!(registry().len()),
        "gen-docs reports how many tools it documented"
    );
    let generated = fs::read_to_string(generated_dir.join("tools.md")).expect("generated tools.md");

    assert!(
        generated == committed,
        "docs/api/tools.md is out of date: regenerate it, from the repository root, with \
         `cargo run --manifest-path core/Cargo.toml -p forma-cli -- gen-docs docs/api`"
    );
    // Every tool of the registry is documented, once, in registry order.
    let mut from = 0;
    for spec in registry() {
        let heading = format!("### `{}`", spec.name);
        let at = committed[from..]
            .find(&heading)
            .unwrap_or_else(|| panic!("{} is missing or out of order in tools.md", spec.name));
        from += at + heading.len();
    }
}
