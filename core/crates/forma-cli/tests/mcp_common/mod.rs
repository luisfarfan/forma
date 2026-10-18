//! The shared harness of the stdio MCP integration tests (`mcp_scenario`,
//! `mcp_safety`).
//!
//! Every test spawns the real `forma mcp` binary, exactly as Claude Code or
//! Codex would, and talks to it over stdin/stdout with rmcp's own client in
//! one of the two protocol eras:
//! * **legacy** ([`legacy`]): the `initialize` handshake at 2025-06-18, as
//!   Codex does; the client's name travels once, in the handshake;
//! * **modern** ([`modern`]): 2026-07-28 `server/discover`, no handshake, and
//!   the client's identity in every request's `_meta`.
//!
//! The server only ever sees a fresh temporary folder: it is the only
//! `--allow-dir`, and the crash-recovery journal lives inside it
//! (`--journal-dir`), so no test touches the user's data folder.
//!
//! **Clients must await dependent calls.** rmcp serves requests concurrently,
//! so requests a client pipelines without waiting for the answers may reach
//! the engine in any order. Each call is still correct on its own (the engine
//! serializes them and every mutation checks its `base_rev`), but a request
//! that depends on the result of another must be sent after that result has
//! arrived. Every helper here awaits its response before returning.

// Each test binary uses its own subset of the helpers.
#![allow(dead_code)]

use std::future::Future;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig, Implementation,
    ProtocolVersion,
};
use rmcp::service::RunningService;
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientLifecycleMode, ClientServiceExt, RoleClient};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::process::Command;

/// The `clientInfo.name` of the test client.
pub const CLIENT_NAME: &str = "forma-test-agent";

/// The actor the server records for the test client.
pub const ACTOR: &str = "agent:forma-test-agent";

/// How long one test may take before it fails instead of hanging CI.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The revision a legacy client asks for in `initialize`.
pub const LEGACY_VERSION: ProtocolVersion = ProtocolVersion::V_2025_06_18;

/// The revision a modern client discovers.
pub const MODERN_VERSION: ProtocolVersion = ProtocolVersion::V_2026_07_28;

/// A connected MCP client.
pub type Client = RunningService<RoleClient, ClientConfig>;

/// The legacy lifecycle: the `initialize` handshake.
pub fn legacy() -> ClientLifecycleMode {
    ClientLifecycleMode::Initialize
}

/// The 2026-07-28 lifecycle: `server/discover`, then self-contained requests.
pub fn modern() -> ClientLifecycleMode {
    ClientLifecycleMode::Discover {
        preferred_versions: vec![MODERN_VERSION],
    }
}

/// Whether `mode` is the legacy lifecycle.
pub fn is_legacy(mode: &ClientLifecycleMode) -> bool {
    matches!(mode, ClientLifecycleMode::Initialize)
}

/// The `forma` binary under test.
pub fn forma_bin() -> &'static str {
    env!("CARGO_BIN_EXE_forma")
}

/// The arguments that start `forma mcp` sandboxed to `root`, with its journal
/// and its document locks inside it, followed by `extra_args`.
pub fn mcp_args(root: &Path, extra_args: &[&str]) -> Vec<String> {
    let mut args = vec![
        "mcp".to_owned(),
        "--allow-dir".to_owned(),
        path_text(root),
        "--journal-dir".to_owned(),
        path_text(&root.join("journal")),
        "--lock-dir".to_owned(),
        path_text(&root.join("locks")),
    ];
    args.extend(extra_args.iter().map(|arg| (*arg).to_owned()));
    args
}

/// A path as UTF-8 text (temporary folders always are).
pub fn path_text(path: &Path) -> String {
    path.to_str()
        .unwrap_or_else(|| panic!("not UTF-8: {}", path.display()))
        .to_owned()
}

/// Runs `test` and fails it if it takes longer than [`TEST_TIMEOUT`].
pub async fn with_timeout<F: Future>(name: &str, test: F) -> F::Output {
    match tokio::time::timeout(TEST_TIMEOUT, test).await {
        Ok(output) => output,
        Err(_) => panic!("{name} did not finish within {TEST_TIMEOUT:?}"),
    }
}

/// Like [`with_timeout`], with a longer `limit` for the heavier scenarios.
pub async fn with_timeout_for<F: Future>(name: &str, limit: Duration, test: F) -> F::Output {
    match tokio::time::timeout(limit, test).await {
        Ok(output) => output,
        Err(_) => panic!("{name} did not finish within {limit:?}"),
    }
}

/// Copies the Milestone 2 specimen image (`tests/fixtures/hito2/specimen.png`
/// at the root of the repository) into `root` as `name`; returns its path.
pub fn copy_specimen(root: &Path, name: &str) -> PathBuf {
    let from =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/hito2/specimen.png");
    let to = root.join(name);
    std::fs::copy(&from, &to)
        .unwrap_or_else(|error| panic!("cannot copy {}: {error}", from.display()));
    to
}

/// Spawns `forma mcp` on a fresh temporary root and connects the test client
/// to it in `mode`. Checks the negotiated revision: 2025-06-18 for the legacy
/// handshake, 2026-07-28 for discovery.
///
/// The returned folder is the server's only allowed root; keep it alive for
/// the whole test.
pub async fn connect(mode: ClientLifecycleMode, extra_args: &[&str]) -> (Client, TempDir) {
    let root = TempDir::new().expect("temporary root");
    let client = connect_to(mode, root.path(), extra_args).await;
    (client, root)
}

/// Spawns `forma mcp` on the existing folder `root` (its only allowed root,
/// which also holds its journal and lock folders) and connects the test
/// client to it in `mode`, like [`connect`]. Two servers started on the same
/// root share their journals and document locks, as two processes of one
/// user do.
pub async fn connect_to(mode: ClientLifecycleMode, root: &Path, extra_args: &[&str]) -> Client {
    let mut command = Command::new(forma_bin());
    // The server's logs (warnings only, unless FORMA_LOG says otherwise) go
    // to the test's stderr. A server left behind by a failing test is killed.
    command.args(mcp_args(root, extra_args)).kill_on_drop(true);
    let transport = TokioChildProcess::new(command).expect("spawn forma mcp");

    let info = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new(CLIENT_NAME, "1.0.0"),
    );
    let (info, expected) = if is_legacy(&mode) {
        (info.with_protocol_version(LEGACY_VERSION), LEGACY_VERSION)
    } else {
        (info, MODERN_VERSION)
    };
    let client = match info.serve_with_lifecycle(transport, mode.clone()).await {
        Ok(client) => client,
        Err(error) => panic!("{mode:?}: the client could not connect: {error:?}"),
    };
    let peer = client.peer_info().expect("server info after the lifecycle");
    assert_eq!(peer.protocol_version, expected, "{mode:?}");
    assert_eq!(
        peer.server_info.as_ref().map(|server| server.name.as_str()),
        Some("forma")
    );
    client
}

/// Closes the client: stdin reaches EOF and the server exits.
pub async fn close(client: Client) {
    client.cancel().await.expect("close the client");
}

/// What a `tools/call` returned, in the shape the facade promises.
#[derive(Clone, Debug)]
pub struct CallOutcome {
    /// For a data result, its structured content (checked to equal the JSON
    /// of its text block); for an image result, the JSON summary of its text
    /// block; for an error, its `{code, message, details}`.
    pub json: Option<Value>,
    /// The PNG of an image result (its base64 data, decoded).
    pub image: Option<Vec<u8>>,
    /// Whether the result is a tool error (`isError: true`).
    pub is_error: bool,
    /// The stable error code of a tool error.
    pub error_code: Option<String>,
    /// Whether the result carried `structuredContent`.
    pub structured: bool,
}

impl CallOutcome {
    /// The JSON of a successful data result.
    #[track_caller]
    pub fn ok(self) -> Value {
        assert!(!self.is_error, "the call failed: {:?}", self.json);
        assert!(self.structured, "a data result without structured content");
        self.json.expect("result JSON")
    }

    /// The `{code, message, details}` of a tool error, after checking that its
    /// code is `code`.
    #[track_caller]
    pub fn error(self, code: &str) -> Value {
        assert!(
            self.is_error,
            "expected {code}, the call succeeded: {:?}",
            self.json
        );
        assert_eq!(self.error_code.as_deref(), Some(code), "{:?}", self.json);
        self.json.expect("error JSON")
    }
}

/// Sends `tools/call` for `tool` with `args` (a JSON object) and checks the
/// result's shape:
/// * data results have structured content and one text block with the same
///   JSON;
/// * image results have an `image/png` block and a JSON text summary, and no
///   structured content;
/// * errors have `isError: true`, no structured content and one text block
///   with exactly `{code, message, details}`.
///
/// A failure at the protocol level fails the test.
pub async fn call(client: &Client, tool: &str, args: Value) -> CallOutcome {
    let Value::Object(args) = args else {
        panic!("{tool}: the arguments must be a JSON object");
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args))
        .await
        .unwrap_or_else(|error| panic!("tools/call {tool} failed at the protocol level: {error}"));
    outcome(tool, &result)
}

/// Checks and unpacks a `tools/call` result (see [`call`]).
pub fn outcome(tool: &str, result: &CallToolResult) -> CallOutcome {
    let text_json = |index: usize| -> Value {
        let block = result
            .content
            .get(index)
            .unwrap_or_else(|| panic!("{tool}: no content block {index}: {result:?}"));
        let text = block
            .as_text()
            .unwrap_or_else(|| panic!("{tool}: block {index} is not text: {result:?}"));
        serde_json::from_str(&text.text)
            .unwrap_or_else(|error| panic!("{tool}: text block is not JSON ({error}): {result:?}"))
    };

    if result.is_error == Some(true) {
        assert_eq!(result.structured_content, None, "{tool}: {result:?}");
        assert_eq!(result.content.len(), 1, "{tool}: {result:?}");
        let error = text_json(0);
        let mut keys: Vec<&str> = error
            .as_object()
            .unwrap_or_else(|| panic!("{tool}: the error is not an object: {error}"))
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["code", "details", "message"], "{tool}: {error}");
        let code = error["code"].as_str().map(str::to_owned);
        return CallOutcome {
            json: Some(error),
            image: None,
            is_error: true,
            error_code: code,
            structured: false,
        };
    }

    if let Some(image) = result.content.first().and_then(|block| block.as_image()) {
        assert_eq!(image.mime_type, "image/png", "{tool}");
        assert_eq!(result.content.len(), 2, "{tool}: {result:?}");
        let png = BASE64
            .decode(&image.data)
            .unwrap_or_else(|error| panic!("{tool}: image data is not base64: {error}"));
        return CallOutcome {
            json: Some(text_json(1)),
            image: Some(png),
            is_error: false,
            error_code: None,
            structured: result.structured_content.is_some(),
        };
    }

    assert_eq!(result.content.len(), 1, "{tool}: {result:?}");
    let echoed = text_json(0);
    if let Some(structured) = &result.structured_content {
        assert_eq!(
            &echoed, structured,
            "{tool}: text and structured content differ"
        );
    }
    CallOutcome {
        json: Some(result.structured_content.clone().unwrap_or(echoed)),
        image: None,
        is_error: false,
        error_code: None,
        structured: result.structured_content.is_some(),
    }
}

/// Decodes a whole PNG; fails the test if it is not one.
#[track_caller]
pub fn decode_png(png: &[u8]) -> vello_cpu::Pixmap {
    vello_cpu::Pixmap::from_png(Cursor::new(png)).expect("a decodable PNG")
}

/// Creates an empty in-memory document; returns its handle.
pub async fn new_doc(client: &Client, family: &str) -> String {
    let created = call(client, "document_create", json!({ "family_name": family }))
        .await
        .ok();
    assert_eq!(created["rev"], json!(0));
    created["doc"].as_str().expect("doc handle").to_owned()
}

/// A counter-clockwise square, as SVG path data in font units.
pub const SQUARE: &str = "M100 0 L500 0 L500 400 L100 400 Z";

/// Creates the glyphs `names` (squares) in one `edit_batch` at `base_rev`;
/// returns the new revision.
pub async fn create_squares(client: &Client, doc: &str, base_rev: u64, names: &[String]) -> u64 {
    let ops: Vec<Value> = names
        .iter()
        .map(|name| {
            json!({ "tool": "glyph_create",
                    "args": { "name": name, "advance_width": 600, "svg_path": SQUARE } })
        })
        .collect();
    let batch = call(
        client,
        "edit_batch",
        json!({ "doc": doc, "base_rev": base_rev, "ops": ops }),
    )
    .await
    .ok();
    assert_eq!(batch["rev"], json!(base_rev + 1), "{batch}");
    base_rev + 1
}

/// The glyph names of `doc`, in glyph order.
pub async fn glyph_names(client: &Client, doc: &str) -> Vec<String> {
    let listed = call(client, "glyph_list", json!({ "doc": doc, "limit": 500 }))
        .await
        .ok();
    assert_eq!(listed["next_cursor"], Value::Null, "{listed}");
    listed["glyphs"]
        .as_array()
        .expect("glyphs")
        .iter()
        .map(|glyph| glyph["name"].as_str().expect("name").to_owned())
        .collect()
}

/// The current revision of `doc`.
pub async fn rev_of(client: &Client, doc: &str) -> u64 {
    let inspected = call(client, "document_inspect", json!({ "doc": doc }))
        .await
        .ok();
    inspected["rev"].as_u64().expect("rev")
}

/// The fixture of the Milestone 1 scenario: `tests/fixtures/hito1/glyphs.json`
/// at the root of the repository.
pub fn fixture() -> Value {
    let path = fixture_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()))
}

/// Where the fixture lives.
pub fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/hito1/glyphs.json")
}
