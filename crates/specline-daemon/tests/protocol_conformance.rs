//! The JSON-RPC envelope, independent of what the tools do.
//!
//! Everything else in this suite asserts on Specline: what a tool returns, what a
//! store holds. This asserts on the wire contract a client is entitled to
//! whatever the tools are — the handshake, the error codes, the header rules,
//! the shape of a response. A client does not fail gracefully on an envelope it
//! cannot parse; it concludes the server is broken.
//!
//! The header rules in particular are worth pinning here rather than only in
//! `protocol.rs`'s unit tests, because the interesting version of the question
//! is what a *request* gets back, not what a function returns. This daemon once
//! closed legacy negotiation and stopped serving the one client the product
//! exists for — Claude Code speaks the older revision — and the unit tests were
//! all still green.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use specline_daemon::{AppState, http::router};
use specline_mcp::protocol::{
    HEADER_METHOD, HEADER_NAME, HEADER_PROTOCOL_VERSION, LEGACY_PROTOCOL_VERSION, PROTOCOL_VERSION,
    codes,
};

async fn daemon() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path(), false).expect("open the store");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), dir)
}

/// Post a body with whatever headers, and return status plus parsed body.
async fn post(base: &str, headers: &[(&str, &str)], body: Value) -> (u16, Value) {
    let mut request = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .json(&body);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let payload: Value = response
        .json()
        .await
        .unwrap_or_else(|e| panic!("every answer has to be JSON; this one was not ({e})"));
    (status, payload)
}

fn rpc(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params})
}

/// The handshake, in the revision a current client speaks.
#[tokio::test]
async fn initialize_answers_with_a_version_and_capabilities() {
    let (base, _dir) = daemon().await;

    let (status, body) = post(
        &base,
        &[
            (HEADER_PROTOCOL_VERSION, PROTOCOL_VERSION),
            (HEADER_METHOD, "initialize"),
        ],
        rpc(
            "initialize",
            json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}}),
        ),
    )
    .await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(
        body["id"], 7,
        "the id has to come back, or a client cannot match the reply"
    );
    assert_eq!(body["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert!(
        body["result"]["capabilities"]["tools"].is_object(),
        "a server that does not declare tools is one a client will not call: {body}"
    );
    assert!(body["result"]["serverInfo"]["name"].is_string());
}

/// And in the revision Claude Code actually speaks.
///
/// The daemon closed legacy negotiation once and immediately stopped serving
/// its only client. This is the test that would have said so.
#[tokio::test]
async fn the_legacy_revision_still_negotiates() {
    let (base, _dir) = daemon().await;

    let (status, body) = post(
        &base,
        &[],
        rpc(
            "initialize",
            json!({"protocolVersion": LEGACY_PROTOCOL_VERSION, "capabilities": {}}),
        ),
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["result"]["protocolVersion"], LEGACY_PROTOCOL_VERSION);
}

/// A client older than the version header itself sends none of it and must
/// still be served. That is the arm Claude Code's very first request lands on.
#[tokio::test]
async fn a_request_with_no_version_anywhere_is_served_as_legacy() {
    let (base, _dir) = daemon().await;

    let (status, body) = post(&base, &[], rpc("tools/list", json!({}))).await;

    assert_eq!(status, 200);
    assert!(body["result"]["tools"].is_array(), "{body}");
}

/// An unrecognised version is served, not refused.
///
/// This asserted the refusal, and listed the two revisions that did work, and
/// was green for as long as the daemon locked out every client that spoke
/// anything else. Codex speaks 2025-06-18: it was told the server speaks
/// 2026-07-28 and 2025-11-25, retried once and gave up, and not one of the
/// thirteen tools ever appeared (KEEL-355).
///
/// Naming what does work is only useful to a client still connected to read it.
#[tokio::test]
async fn an_unrecognised_version_is_served_rather_than_refused() {
    let (base, _dir) = daemon().await;

    let (status, body) = post(
        &base,
        &[(HEADER_PROTOCOL_VERSION, "1999-01-01")],
        rpc("tools/list", json!({})),
    )
    .await;

    assert_eq!(status, 200, "{body}");
    assert!(body["result"]["tools"].is_array(), "{body}");
}

/// The handshake Codex actually sends, end to end through the daemon.
///
/// Captured off the wire from `codex-mcp-client/0.148.0-alpha.15` — no
/// mirrored headers, `2025-06-18` in the body. The version it gets back has to
/// be its own: a client handed a revision it did not offer is entitled to hang
/// up, and hanging up is what it did.
#[tokio::test]
async fn the_handshake_codex_sends_is_answered_in_codexs_own_revision() {
    let (base, _dir) = daemon().await;

    let (status, body) = post(
        &base,
        &[],
        rpc(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "elicitation": { "form": {}, "url": {} } },
                "clientInfo": {
                    "name": "codex-mcp-client",
                    "title": "Codex",
                    "version": "0.148.0-alpha.15"
                }
            }),
        ),
    )
    .await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18", "{body}");

    // And the tools have to be reachable afterwards, which is the thing the
    // handshake was in the way of.
    let (status, body) = post(&base, &[], rpc("tools/list", json!({}))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["result"]["tools"].as_array().map(Vec::len),
        Some(13),
        "{body}"
    );
}

/// The mirrored headers exist because an intermediary may route on the header
/// while the server executes on the body. A mismatch is a security problem
/// rather than a formatting one, and is refused as such.
#[tokio::test]
async fn a_header_that_disagrees_with_the_body_is_refused() {
    let (base, _dir) = daemon().await;

    let (status, body) = post(
        &base,
        &[
            (HEADER_PROTOCOL_VERSION, PROTOCOL_VERSION),
            (HEADER_METHOD, "tools/list"),
            (HEADER_NAME, "specline_search"),
        ],
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
               "params": {"name": "specline_projects", "arguments": {}}}),
    )
    .await;

    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], codes::HEADER_MISMATCH);
}

/// The headers agreeing is the other half, and the half that would break
/// silently if the check were too strict.
#[tokio::test]
async fn matching_headers_are_served() {
    let (base, _dir) = daemon().await;

    let (status, body) = post(
        &base,
        &[
            (HEADER_PROTOCOL_VERSION, PROTOCOL_VERSION),
            (HEADER_METHOD, "tools/call"),
            (HEADER_NAME, "specline_projects"),
        ],
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
               "params": {"name": "specline_projects", "arguments": {}}}),
    )
    .await;

    assert_eq!(status, 200, "{body}");
    assert!(body["result"].is_object(), "{body}");
}

/// Each error class gets its own code, and the codes are what a client
/// branches on.
#[tokio::test]
async fn the_error_codes_are_the_ones_the_specification_names() {
    let (base, _dir) = daemon().await;

    // A method that does not exist.
    let (status, body) = post(&base, &[], rpc("tools/teleport", json!({}))).await;
    assert_eq!(body["error"]["code"], codes::METHOD_NOT_FOUND, "{body}");
    assert_eq!(
        status, 404,
        "METHOD_NOT_FOUND is the one error the specification maps to 404"
    );

    // A tool that does not exist is *not* a missing method: the method is
    // `tools/call` and it exists. Serving it as 404 would tell a client there
    // is no MCP server at this address.
    let (status, body) = post(
        &base,
        &[],
        rpc(
            "tools/call",
            json!({"name": "specline_teleport", "arguments": {}}),
        ),
    )
    .await;
    assert_eq!(body["error"]["code"], codes::INVALID_PARAMS, "{body}");
    assert_eq!(status, 400);

    // Arguments that are the wrong shape. Specline answers these in the envelope
    // rather than as a tool result with `isError` — both are permitted, and
    // this is the one it picked: the argument never reached the tool, so there
    // is no tool result to put an error in. Pinned because a client branches on
    // which of the two it gets.
    let (status, body) = post(
        &base,
        &[],
        rpc(
            "tools/call",
            json!({"name": "specline_search", "arguments": {}}),
        ),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], codes::INVALID_PARAMS);
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("query")),
        "the message names the argument, which is what makes it actionable: {body}"
    );
}

/// A body declaring the current revision without the mirrored headers is
/// refused.
///
/// This is the rule the mirrored headers exist for: an intermediary may route
/// on the header while the server executes on the body, so a request that
/// declares one and omits the other cannot be served safely. It applies to
/// `initialize` like everything else, which is worth pinning because
/// `initialize` is the one method that may declare its version in the body.
#[tokio::test]
async fn a_current_revision_request_without_its_headers_is_refused() {
    let (base, _dir) = daemon().await;

    let (status, body) = post(
        &base,
        &[],
        rpc(
            "initialize",
            json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}}),
        ),
    )
    .await;

    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], codes::HEADER_MISMATCH);
}

/// Every response carries the id it was asked with, including the error ones.
///
/// A client matches replies to requests by id and has nothing else to go on. An
/// error that drops it is an error that belongs to no request.
#[tokio::test]
async fn the_id_survives_every_path() {
    let (base, _dir) = daemon().await;

    for body in [
        rpc("tools/list", json!({})),
        rpc("tools/teleport", json!({})),
        rpc(
            "tools/call",
            json!({"name": "specline_search", "arguments": {}}),
        ),
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {}}),
    ] {
        let (_, answer) = post(&base, &[], body.clone()).await;
        assert_eq!(
            answer["id"], 7,
            "the id did not come back for {}: {answer}",
            body["method"]
        );
        assert_eq!(answer["jsonrpc"], "2.0");
    }
}

/// `tools/list` is a shape a client parses before it knows anything about Specline.
#[tokio::test]
async fn the_tool_list_has_the_shape_a_client_expects() {
    let (base, _dir) = daemon().await;

    let (_, body) = post(&base, &[], rpc("tools/list", json!({}))).await;
    let tools = body
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{body}"));

    assert_eq!(tools.len(), 13);
    for tool in tools {
        assert!(tool["name"].is_string(), "{tool}");
        assert!(
            tool["description"].as_str().is_some_and(|d| !d.is_empty()),
            "a tool with no description is one a model cannot choose: {tool}"
        );
        assert_eq!(
            tool["inputSchema"]["type"], "object",
            "every input schema is an object schema: {tool}"
        );
    }
}
