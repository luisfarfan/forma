//! Milestone 1, acceptance criteria 3 to 5 over stdio MCP, in both protocol
//! eras: optimistic concurrency, approvals, the path sandbox, payload limits,
//! cancellation, and the robustness of the stdio transport itself.
//!
//! The rmcp-client tests use the harness of `mcp_common`. The transport tests
//! at the end write raw lines to `forma mcp` through `std::process`, as a
//! hand-written client would.
//!
//! Clients must await dependent requests: see the note in `mcp_common`.

mod mcp_common;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mcp_common::{
    ACTOR, Client, TEST_TIMEOUT, call, close, connect, create_squares, glyph_names, is_legacy,
    legacy, mcp_args, modern, new_doc, path_text, rev_of, with_timeout,
};
use rmcp::ClientLifecycleMode;
use rmcp::model::{CallToolRequest, CallToolRequestParams, ClientRequest};
use rmcp::service::PeerRequestOptions;
use rstest::rstest;
use serde_json::{Value, json};
use tempfile::TempDir;

// ------------------------------------------------------- concurrency (crit. 3)

/// A call based on a stale revision is refused when it touches an entity that
/// changed since, and applies when it touches only unchanged entities.
#[rstest]
#[case::legacy(legacy())]
#[case::modern(modern())]
#[tokio::test]
async fn stale_base_rev_conflict(#[case] mode: ClientLifecycleMode) {
    with_timeout("stale_base_rev_conflict", async {
        let (client, _root) = connect(mode, &[]).await;
        let doc = new_doc(&client, "Conflicts").await;
        let rev = create_squares(&client, &doc, 0, &["A".to_owned(), "B".to_owned()]).await;
        let a_id = glyph(&client, &doc, "A").await["id"].clone();

        // Edit A: the document is now at revision N.
        let edited = call(
            &client,
            "metrics_set",
            json!({ "doc": doc, "base_rev": rev,
                    "items": [{ "glyph": "A", "advance_width": 700 }] }),
        )
        .await
        .ok();
        let n = edited["rev"].as_u64().expect("rev");
        assert_eq!(n, rev + 1);

        // Based on N - 1 and touching A: a conflict naming A.
        let conflict = call(
            &client,
            "metrics_set",
            json!({ "doc": doc, "base_rev": n - 1,
                    "items": [{ "glyph": "A", "advance_width": 650 }] }),
        )
        .await
        .error("REVISION_CONFLICT");
        let details = &conflict["details"];
        assert_eq!(details["current_rev"], json!(n), "{conflict}");
        let conflicts = details["conflicts"].as_array().expect("conflicts");
        assert_eq!(conflicts.len(), 1, "{conflict}");
        let a_entity = format!("glyph:{}", a_id.as_str().expect("glyph id"));
        assert_eq!(conflicts[0]["entity"], json!(a_entity), "{conflict}");
        assert_eq!(conflicts[0]["rev"], json!(n), "{conflict}");
        assert_eq!(conflicts[0]["actor"], json!(ACTOR), "{conflict}");
        assert_eq!(rev_of(&client, &doc).await, n);
        assert_eq!(
            glyph(&client, &doc, "A").await["advance_width"],
            json!(700.0)
        );

        // The same stale base, touching only B (unchanged since): applied.
        let applied = call(
            &client,
            "metrics_set",
            json!({ "doc": doc, "base_rev": n - 1,
                    "items": [{ "glyph": "B", "advance_width": 650 }] }),
        )
        .await
        .ok();
        assert_eq!(applied["rev"], json!(n + 1), "{applied}");
        assert_eq!(
            glyph(&client, &doc, "B").await["advance_width"],
            json!(650.0)
        );
        assert_eq!(
            glyph(&client, &doc, "A").await["advance_width"],
            json!(700.0)
        );
        close(client).await;
    })
    .await;
}

// --------------------------------------------------------- approvals (crit. 4)

/// Deleting 25 glyphs needs approval: the plan comes back with a token,
/// `plan_commit` applies it, `history_undo` restores the glyphs, and the token
/// cannot be used twice.
#[rstest]
#[case::legacy(legacy())]
#[case::modern(modern())]
#[tokio::test]
async fn delete_25_requires_approval_then_commit_then_undo(#[case] mode: ClientLifecycleMode) {
    with_timeout("delete_25_requires_approval_then_commit_then_undo", async {
        let (client, _root) = connect(mode, &[]).await;
        let doc = new_doc(&client, "Approvals").await;
        let all: Vec<String> = (0..26).map(|index| format!("g{index:02}")).collect();
        // Two batches of 13, each under the mass-edit threshold (20).
        let rev = create_squares(&client, &doc, 0, &all[..13]).await;
        let rev = create_squares(&client, &doc, rev, &all[13..]).await;
        assert_eq!(glyph_names(&client, &doc).await, all);

        let doomed = &all[..25];
        let refused = call(
            &client,
            "glyph_delete",
            json!({ "doc": doc, "base_rev": rev, "glyphs": doomed }),
        )
        .await
        .error("APPROVAL_REQUIRED");
        let plan = &refused["details"]["plan"];
        let token = plan["plan_token"].as_str().expect("plan_token").to_owned();
        assert_eq!(plan["affected_count"], json!(25), "{refused}");
        assert_eq!(plan["tool"], json!("glyph_delete"), "{refused}");
        assert_eq!(plan["base_rev"], json!(rev), "{refused}");
        // Nothing happened yet.
        assert_eq!(rev_of(&client, &doc).await, rev);
        assert_eq!(glyph_names(&client, &doc).await, all);

        let committed = call(&client, "plan_commit", json!({ "plan_token": token }))
            .await
            .ok();
        assert_eq!(committed["plan_committed"], json!(true), "{committed}");
        assert_eq!(committed["rev"], json!(rev + 1), "{committed}");
        assert_eq!(glyph_names(&client, &doc).await, ["g25"]);

        let undone = call(&client, "history_undo", json!({ "doc": doc }))
            .await
            .ok();
        assert_eq!(undone["rev"], json!(rev + 2), "{undone}");
        assert_eq!(glyph_names(&client, &doc).await, all);

        // A used token is refused, and changes nothing.
        let reused = call(&client, "plan_commit", json!({ "plan_token": token }))
            .await
            .error("INVALID_ARGUMENT");
        assert_eq!(reused["message"], json!("unknown or already used plan"));
        assert_eq!(rev_of(&client, &doc).await, rev + 2);
        assert_eq!(glyph_names(&client, &doc).await, all);

        // The approved deletion is the test client's.
        let history = call(&client, "history_list", json!({ "doc": doc }))
            .await
            .ok();
        let entries = history["entries"].as_array().expect("entries");
        assert!(!entries.is_empty(), "{history}");
        for entry in entries {
            assert_eq!(entry["actor"], json!(ACTOR), "{entry}");
        }
        close(client).await;
    })
    .await;
}

// ----------------------------------------------------------- security (crit. 5)

/// Paths outside `--allow-dir` are refused with PATH_NOT_ALLOWED, and nothing
/// is written there: an absolute path elsewhere, a `..` escape and (on Unix) a
/// symbolic link inside the root that points out of it.
#[rstest]
#[case::legacy(legacy())]
#[case::modern(modern())]
#[tokio::test]
async fn path_outside_allow_dir_rejected(#[case] mode: ClientLifecycleMode) {
    with_timeout("path_outside_allow_dir_rejected", async {
        let (client, root) = connect(mode, &[]).await;
        let root = root.path();
        let outside = TempDir::new().expect("a folder outside the root");

        let mut targets = vec![
            outside.path().join("elsewhere.forma"),
            root.join("..").join("escaped.forma"),
        ];
        #[cfg(unix)]
        {
            let link = root.join("link");
            std::os::unix::fs::symlink(outside.path(), &link).expect("symlink");
            targets.push(link.join("through-link.forma"));
        }
        for target in &targets {
            let refused = call(
                &client,
                "document_create",
                json!({ "family_name": "Escape", "path": path_text(target) }),
            )
            .await
            .error("PATH_NOT_ALLOWED");
            assert!(
                !target.exists(),
                "{} was written: {refused}",
                target.display()
            );
        }
        let listed = call(&client, "document_list", json!({})).await.ok();
        assert_eq!(listed["documents"], json!([]), "{listed}");

        // Exports are sandboxed too.
        let doc = new_doc(&client, "Sandboxed").await;
        let target = outside.path().join("escaped.ttf");
        call(
            &client,
            "export_font",
            json!({ "doc": doc, "base_rev": 0, "format": "ttf", "path": path_text(&target) }),
        )
        .await
        .error("PATH_NOT_ALLOWED");
        assert!(!target.exists());
        assert_eq!(
            std::fs::read_dir(outside.path()).expect("list").count(),
            0,
            "nothing may be written outside the root"
        );
        close(client).await;
    })
    .await;
}

/// A 5 MiB `svg_path` (over the 4 MiB limit per text argument) is refused with
/// LIMIT_EXCEEDED before anything changes.
#[rstest]
#[case::legacy(legacy())]
#[case::modern(modern())]
#[tokio::test]
async fn oversized_payload_limit_exceeded(#[case] mode: ClientLifecycleMode) {
    with_timeout("oversized_payload_limit_exceeded", async {
        let (client, _root) = connect(mode, &[]).await;
        let doc = new_doc(&client, "Limits").await;
        let size = 5 * 1024 * 1024;
        let svg_path: String = "M0 0 L1 0 L1 1 Z ".chars().cycle().take(size).collect();
        let refused = call(
            &client,
            "glyph_create",
            json!({ "doc": doc, "base_rev": 0, "name": "huge", "advance_width": 500,
                    "svg_path": svg_path }),
        )
        .await
        .error("LIMIT_EXCEEDED");
        let details = &refused["details"];
        assert_eq!(details["limit"], json!(4 * 1024 * 1024), "{details}");
        assert_eq!(details["actual"], json!(size), "{details}");
        assert_eq!(details["argument"], json!("svg_path"), "{details}");
        assert_eq!(rev_of(&client, &doc).await, 0);
        assert!(glyph_names(&client, &doc).await.is_empty());
        close(client).await;
    })
    .await;
}

// ------------------------------------------------------------- cancellation

/// Cancelling a request in flight (`notifications/cancelled`, through rmcp's
/// cancellation API) leaves the server serving: the next calls answer.
#[rstest]
#[case::legacy(legacy())]
#[case::modern(modern())]
#[tokio::test]
async fn cancellation_does_not_kill_server(#[case] mode: ClientLifecycleMode) {
    with_timeout("cancellation_does_not_kill_server", async {
        let (client, root) = connect(mode, &[]).await;
        let doc = new_doc(&client, "Cancel").await;
        let names: Vec<String> = (0..20).map(|index| format!("g{index:02}")).collect();
        let rev = create_squares(&client, &doc, 0, &names).await;

        let target = root.path().join("cancelled.ttf");
        let Value::Object(args) = json!({ "doc": doc, "base_rev": rev, "format": "ttf",
                                          "path": path_text(&target) })
        else {
            unreachable!("an object literal");
        };
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(
            CallToolRequestParams::new("export_font").with_arguments(args),
        ));
        let handle = client
            .send_cancellable_request(request, PeerRequestOptions::no_options())
            .await
            .expect("send export_font");
        handle
            .cancel(Some("the test cancels the export".to_owned()))
            .await
            .expect("send notifications/cancelled");

        // The server is still there and answers.
        let listed = call(&client, "document_list", json!({})).await.ok();
        let documents = listed["documents"].as_array().expect("documents");
        assert_eq!(documents.len(), 1, "{listed}");
        assert_eq!(documents[0]["doc"], json!(doc), "{listed}");
        assert_eq!(glyph_names(&client, &doc).await, names);
        // And it still does real work. (The cancelled export may or may not
        // have written its file, so this one goes to another path.)
        let again = root.path().join("again.ttf");
        let exported = call(
            &client,
            "export_font",
            json!({ "doc": doc, "base_rev": rev, "format": "ttf", "path": path_text(&again) }),
        )
        .await
        .ok();
        assert_eq!(exported["format"], json!("ttf"), "{exported}");
        assert!(again.is_file());
        close(client).await;
    })
    .await;
}

// ------------------------------------------------------ the stdio transport

/// What a raw `forma mcp` session printed.
struct RawRun {
    /// Every stdout line, parsed (each must be a JSON-RPC 2.0 message).
    messages: Vec<Value>,
    status: ExitStatus,
    stderr: String,
}

impl RawRun {
    /// The response to request `id`.
    #[track_caller]
    fn response(&self, id: u64) -> &Value {
        self.messages
            .iter()
            .find(|message| message["id"] == json!(id))
            .unwrap_or_else(|| panic!("no response to request {id}: {:#?}", self.messages))
    }
}

/// Spawns `forma mcp` sandboxed to `root`, writes `lines` to its stdin, waits
/// for the responses to the requests `ids`, then closes stdin and waits for
/// the server to exit. Every stdout line must be a JSON-RPC 2.0 message. The
/// whole run must end within [`TEST_TIMEOUT`].
fn run_raw(root: &Path, lines: &[String], ids: &[u64]) -> RawRun {
    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut child = Command::new(mcp_common::forma_bin())
        .args(mcp_args(root, &[]))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn forma mcp");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");

    // stdout is read line by line on its own thread; stderr in one piece.
    let (lines_tx, lines_rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut text);
        text
    });

    for line in lines {
        stdin.write_all(line.as_bytes()).expect("write a line");
        stdin.write_all(b"\n").expect("write a newline");
    }
    stdin.flush().expect("flush stdin");

    let mut messages = Vec::new();
    let parse = |line: String, messages: &mut Vec<Value>| {
        let message: Value = serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("stdout line is not JSON ({error}): {line}"));
        assert_eq!(message["jsonrpc"], json!("2.0"), "not JSON-RPC 2.0: {line}");
        assert!(
            message.get("result").is_some()
                || message.get("error").is_some()
                || message.get("method").is_some(),
            "not a JSON-RPC message: {line}"
        );
        messages.push(message);
    };
    let answered = |messages: &[Value]| {
        ids.iter()
            .all(|id| messages.iter().any(|m| m["id"] == json!(id)))
    };
    while !answered(&messages) {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines_rx.recv_timeout(left) {
            Ok(line) => parse(line, &mut messages),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                panic!("no answer to {ids:?} within {TEST_TIMEOUT:?}: {messages:#?}");
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = child.kill();
                panic!("stdout closed before the answers to {ids:?}: {messages:#?}");
            }
        }
    }

    // EOF: the server must exit by itself, cleanly.
    drop(stdin);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait for forma mcp") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("forma mcp did not exit after EOF within {TEST_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    reader.join().expect("stdout reader");
    while let Ok(line) = lines_rx.try_recv() {
        parse(line, &mut messages);
    }
    let stderr = stderr_reader.join().expect("stderr reader");
    RawRun {
        messages,
        status,
        stderr,
    }
}

/// The lines that open a session in `mode` (request id 1) and list the tools
/// (request id 2), written by hand as a raw client would.
fn opening(mode: &ClientLifecycleMode) -> Vec<String> {
    if is_legacy(mode) {
        vec![
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                                "clientInfo": { "name": "raw-client", "version": "1" } } })
            .to_string(),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string(),
        ]
    } else {
        let meta = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": { "name": "raw-client", "version": "1" },
            "io.modelcontextprotocol/clientCapabilities": {},
        });
        vec![
            json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover",
                    "params": { "_meta": meta } })
            .to_string(),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list",
                    "params": { "_meta": meta } })
            .to_string(),
        ]
    }
}

/// Lines that are not JSON-RPC requests at all.
fn garbage() -> Vec<String> {
    [
        "this is not JSON",
        "{\"jsonrpc\": \"2.0\", \"id\": 7, \"method\":",
        "",
        "   ",
        "[1, 2, 3]",
        "{\"hello\": \"world\"}",
        "null",
        "42",
        "\"a string\"",
        "{\"jsonrpc\": \"1.0\", \"id\": 8, \"method\": \"tools/list\"}",
        "\u{feff}{ garbage with a byte-order mark",
    ]
    .iter()
    .map(|line| (*line).to_owned())
    .collect()
}

/// Garbage lines before a valid session (and between its messages) do not
/// break it: the requests still get their answers, stdout carries nothing but
/// JSON-RPC lines, and the server exits with 0 at EOF.
#[rstest]
#[case::legacy(legacy())]
#[case::modern(modern())]
fn garbage_lines_do_not_break_the_session(#[case] mode: ClientLifecycleMode) {
    let root = TempDir::new().expect("root");
    let mut lines = garbage();
    let opening = opening(&mode);
    lines.push(opening[0].clone());
    lines.extend(garbage());
    lines.extend(opening[1..].iter().cloned());
    let run = run_raw(root.path(), &lines, &[1, 2]);

    let opened = &run.response(1)["result"];
    if is_legacy(&mode) {
        assert_eq!(opened["protocolVersion"], json!("2025-06-18"), "{opened}");
        assert_eq!(opened["serverInfo"]["name"], json!("forma"), "{opened}");
    } else {
        let versions = opened["supportedVersions"].as_array().expect("versions");
        assert!(versions.contains(&json!("2026-07-28")), "{opened}");
    }
    let tools = run.response(2)["result"]["tools"]
        .as_array()
        .expect("tools/list result");
    assert_eq!(tools.len(), 35);
    // Nothing but answers: the garbage got no reply or a JSON-RPC error.
    for message in &run.messages {
        let answered = message["id"] == json!(1) || message["id"] == json!(2);
        assert!(answered || message.get("error").is_some(), "{message}");
    }
    assert_eq!(run.status.code(), Some(0), "stderr: {}", run.stderr);
    assert!(
        !run.stderr.contains("jsonrpc"),
        "protocol on stderr: {}",
        run.stderr
    );
}

/// EOF before any message (the client went away at once) is a clean exit:
/// code 0 and nothing on stdout. Both eras start the same way here, since
/// no message says which one the client speaks.
#[test]
fn immediate_eof_exits_with_code_0() {
    let root = TempDir::new().expect("root");
    let run = run_raw(root.path(), &[], &[]);
    assert_eq!(run.status.code(), Some(0), "stderr: {}", run.stderr);
    assert!(run.messages.is_empty(), "{:#?}", run.messages);
}

/// The `glyph_get` detail of the glyph `name`.
async fn glyph(client: &Client, doc: &str, name: &str) -> Value {
    let got = call(client, "glyph_get", json!({ "doc": doc, "glyph": name }))
        .await
        .ok();
    got["glyph"].clone()
}
