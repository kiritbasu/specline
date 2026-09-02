//! PRD UC-1 → UC-4, driven over real HTTP against a real daemon.
//!
//! # What this is, and what it is not
//!
//! Phase 1's exit criterion is "a live Claude session completes UC-1 → UC-4".
//! That gate needs a human and a model, and cannot be run in CI. KB's
//! instruction was to substitute the strongest mechanical equivalent and record
//! honestly that the human half is unverified.
//!
//! This is that substitute. It is a real MCP client: real HTTP, real headers,
//! real JSON-RPC, real store on disk. What it does **not** prove is the part
//! only a model can demonstrate — that the tool descriptions lead an agent to
//! pick the right tool unprompted. That remains unverified until KB runs it.
//! See `product/STATUS.md`, "Phase gates I cannot verify".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use specline_daemon::{AppState, router};
use specline_mcp::protocol::PROTOCOL_VERSION;

/// A running daemon on a real port, plus its store directory.
struct Daemon {
    base: String,
    client: reqwest::Client,
    _dir: tempfile::TempDir,
    _handle: tokio::task::JoinHandle<()>,
}

impl Daemon {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(dir.path(), false).expect("open the store");
        let app = router(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Daemon {
            base: format!("http://{addr}"),
            client: reqwest::Client::new(),
            _dir: dir,
            _handle: handle,
        }
    }

    /// Send a JSON-RPC request with correctly mirrored headers, the way a
    /// conforming client must.
    async fn rpc(&self, method: &str, params: Value) -> (u16, Value) {
        let mut params = params;
        if let Some(obj) = params.as_object_mut() {
            obj.insert(
                "_meta".to_owned(),
                json!({
                    "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
                    "io.modelcontextprotocol/clientInfo": {
                        "name": "specline-test-client", "version": "0.1.0"
                    },
                    "io.modelcontextprotocol/clientCapabilities": {}
                }),
            );
        }
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });

        let mut req = self
            .client
            .post(format!("{}/mcp", self.base))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .header("Mcp-Method", method);

        if let Some(name) = body["params"].get("name").and_then(Value::as_str) {
            req = req.header("Mcp-Name", name);
        }

        let response = req.json(&body).send().await.unwrap();
        let status = response.status().as_u16();
        let json: Value = response.json().await.unwrap_or(Value::Null);
        (status, json)
    }

    /// Call a tool and return its structured content, failing loudly on error.
    async fn call(&self, tool: &str, args: Value) -> Value {
        let (status, body) = self
            .rpc("tools/call", json!({ "name": tool, "arguments": args }))
            .await;
        assert_eq!(
            status,
            200,
            "{tool} failed: {}",
            serde_json::to_string_pretty(&body).unwrap()
        );
        body["result"]["structuredContent"].clone()
    }

    /// The human-readable text a model actually reads.
    async fn call_text(&self, tool: &str, args: Value) -> String {
        let (status, body) = self
            .rpc("tools/call", json!({ "name": tool, "arguments": args }))
            .await;
        assert_eq!(status, 200, "{tool} failed: {body}");
        body["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    /// Call a tool expecting failure.
    async fn call_err(&self, tool: &str, args: Value) -> (u16, Value) {
        let (status, body) = self
            .rpc("tools/call", json!({ "name": tool, "arguments": args }))
            .await;
        (status, body["error"].clone())
    }
}

/// Every call from one conversation carries the same session id.
const SESSION: &str = "ses_01KZTESTSESSIONULID000000";

fn args(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        o.insert("session_id".to_owned(), json!(SESSION));
        o.insert("surface".to_owned(), json!("code"));
    }
    v
}

// --- Protocol conformance ------------------------------------------------

#[tokio::test]
async fn server_discover_advertises_the_protocol() {
    let d = Daemon::start().await;
    let (status, body) = d.rpc("server/discover", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(body["result"]["protocolVersions"][0], PROTOCOL_VERSION);
    assert_eq!(body["result"]["serverInfo"]["name"], "specline");
    assert_eq!(body["result"]["resultType"], "complete");
}

#[tokio::test]
async fn tools_list_returns_thirteen_tools_with_cache_hints() {
    let d = Daemon::start().await;
    let (status, body) = d.rpc("tools/list", json!({})).await;
    assert_eq!(status, 200);
    // Thirteen since the three work verbs (TQ-31). The cap is a real
    // constraint, not a rounding error — if this needs changing again, the
    // reason belongs in tools.rs next to the last one.
    let tools = body["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 13);
    assert!(tools.iter().any(|t| t["name"] == "specline_note"));
    for verb in ["specline_next", "specline_claim", "specline_close"] {
        assert!(tools.iter().any(|t| t["name"] == verb), "missing {verb}");
    }
    assert!(body["result"]["ttlMs"].as_u64().unwrap() > 0);
    assert_eq!(body["result"]["cacheScope"], "public");
}

#[tokio::test]
async fn a_header_that_disagrees_with_the_body_is_rejected() {
    let d = Daemon::start().await;
    let response = d
        .client
        .post(format!("{}/mcp", d.base))
        .header("content-type", "application/json")
        .header("MCP-Protocol-Version", PROTOCOL_VERSION)
        .header("Mcp-Method", "tools/list")
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "specline_context", "arguments": {} }
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status().as_u16(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32020, "HeaderMismatch");
}

#[tokio::test]
async fn a_client_with_no_version_header_is_served_as_legacy() {
    // A client older than the version header itself. TQ-11 refused it; that
    // refusal is what stopped Claude Code 2.1.185 connecting, so it is undone.
    // There is nothing else such a client could be.
    let d = Daemon::start().await;
    let response = d
        .client
        .post(format!("{}/mcp", d.base))
        .header("content-type", "application/json")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .unwrap();

    let body: Value = response.json().await.unwrap();
    assert!(
        body.get("error").is_none(),
        "a client with no version header must be served, not refused: {body}"
    );
    assert!(body["result"]["tools"].is_array());
}

#[tokio::test]
async fn the_2025_11_25_handshake_is_answered_in_its_own_dialect() {
    // The exact request Claude Code 2.1.185 opens with, captured from the
    // wire. B-17 answered it; TQ-11 refused it and `claude mcp list` went to
    // "Failed to connect"; this restores it. The reply must quote 2025-11-25
    // back — answering a handshake in a dialect the caller did not offer kills
    // the connection at the first request with nothing useful in the log.
    let d = Daemon::start().await;
    let response = d
        .client
        .post(format!("{}/mcp", d.base))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": { "roots": {}, "elicitation": {} },
                "clientInfo": { "name": "claude-code", "version": "2.1.185" }
            }
        }))
        .send()
        .await
        .unwrap();

    let body: Value = response.json().await.unwrap();
    assert!(
        body.get("error").is_none(),
        "the client this product exists to serve must connect: {body}"
    );
    assert_eq!(body["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(body["result"]["serverInfo"]["name"], "specline");
}

/// An older revision is served, where it used to be told what it was missing.
///
/// The name this test had — "is told what it does speak" — was the whole
/// mistake in six words. Being told is no use to a client that has been hung up
/// on, and every one of them was: Codex opens with 2025-06-18 and never saw a
/// single tool (KEEL-355). 2024-11-05 is the HTTP+SSE era, and while this
/// daemon does not serve that transport, `tools/list` over POST is identical,
/// so there is nothing to refuse.
#[tokio::test]
async fn a_client_on_an_older_revision_is_served() {
    let d = Daemon::start().await;
    let response = d
        .client
        .post(format!("{}/mcp", d.base))
        .header("content-type", "application/json")
        .header("MCP-Protocol-Version", "2024-11-05")
        .header("Mcp-Method", "tools/list")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.unwrap();
    assert!(body.get("error").is_none(), "{body}");
    assert!(body["result"]["tools"].is_array(), "{body}");
}

#[tokio::test]
async fn get_and_delete_on_the_mcp_endpoint_are_405() {
    // Pre-2026-07-28 clients try the GET stream and the DELETE teardown.
    // 405 says "not that protocol"; 404 would say "no endpoint here".
    let d = Daemon::start().await;
    for method in [reqwest::Method::GET, reqwest::Method::DELETE] {
        let r = d
            .client
            .request(method.clone(), format!("{}/mcp", d.base))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 405, "{method} should be 405");
    }
}

#[tokio::test]
async fn a_hostile_origin_is_refused() {
    let d = Daemon::start().await;
    let r = d
        .client
        .post(format!("{}/mcp", d.base))
        .header("Origin", "https://evil.example")
        .header("MCP-Protocol-Version", PROTOCOL_VERSION)
        .header("Mcp-Method", "tools/list")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
}

#[tokio::test]
async fn an_unknown_method_is_404_with_a_jsonrpc_body() {
    let d = Daemon::start().await;
    let (status, body) = d.rpc("resources/list", json!({})).await;
    assert_eq!(status, 404);
    assert_eq!(body["error"]["code"], -32601);
}

// --- UC-2: conversational capture ---------------------------------------

#[tokio::test]
async fn uc2_conversational_capture() {
    // "KB is talking through an idea. Claude writes a PRD into Specline as a
    // versioned document, creates the milestone, decomposes it into tasks, and
    // records the questions it could not resolve — without KB touching a UI."
    let d = Daemon::start().await;

    // The plugin's discipline: check before creating (UC-8, REQ-8).
    let lookup = d
        .call("specline_projects", args(json!({"query": "Harbour"})))
        .await;
    assert_eq!(lookup["projects"].as_array().unwrap().len(), 0);
    assert_eq!(lookup["requires_confirmation"], false);

    let project = d
        .call(
            "specline_create",
            args(json!({
                "type": "project",
                "title": "Harbour",
                "body": "Usage-based billing for API companies."
            })),
        )
        .await;
    assert_eq!(project["created"], true);
    let project_id = project["entity"]["id"].as_str().unwrap().to_owned();
    assert!(project_id.starts_with("prj_"), "{project_id}");

    let prd = d
        .call(
            "specline_create",
            args(json!({
                "type": "spec",
                "project": project_id,
                "title": "Usage metering",
                "body": "# Metering\n\n## REQ-1 Idempotent ingest\n\nEvery usage event carries a \
                         client-supplied idempotency key. Re-sending must be a no-op.",
                "fields": { "kind": "prd", "status": "review" }
            })),
        )
        .await;
    let spec_id = prd["entity"]["id"].as_str().unwrap().to_owned();
    assert_eq!(prd["document"]["version"], 1, "the body became revision 1");
    assert_eq!(
        prd["entity"]["current_doc_version"], 1,
        "the header points at it"
    );

    let milestone = d
        .call(
            "specline_create",
            args(json!({
                "type": "milestone",
                "project": project_id,
                "title": "Metering v1",
                "summary": "Charge customers for what they actually use, and show them the bill.",
                // No status. `active` is derived from the tasks now, and
                // asking for it is refused — which is the point of B-57.
            })),
        )
        .await;
    let milestone_id = milestone["entity"]["id"].as_str().unwrap().to_owned();

    let mut task_ids = Vec::new();
    for title in [
        "Dedupe usage events by idempotency key",
        "Aggregate meter readings into hourly buckets",
    ] {
        let task = d
            .call(
                "specline_create",
                args(json!({
                    "type": "task",
                    "project": project_id,
                    "title": title,
                    "summary": format!("{title} — one of the pieces this milestone breaks down into."),
                    "fields": { "priority": "p0", "milestone_id": milestone_id }
                })),
            )
            .await;
        task_ids.push(task["entity"]["id"].as_str().unwrap().to_owned());
    }

    // Trace each task to the requirement it implements.
    for id in &task_ids {
        d.call(
            "specline_link",
            args(json!({
                "from": id, "rel": "implements", "to": spec_id, "anchor": "REQ-1"
            })),
        )
        .await;
    }

    // The questions it could not resolve — the thing that otherwise evaporates
    // between sessions.
    let question = d
        .call(
            "specline_create",
            args(json!({
                "type": "question",
                "project": project_id,
                "title": "What happens to an event that arrives after its period closed?",
                "body": "Credit it forward, or reopen the invoice? Reopening a closed invoice \
                         is worse than a small inaccuracy."
            })),
        )
        .await;
    assert_eq!(question["created"], true);

    // Everything is attributed to this conversation.
    let activity = d
        .call("specline_activity", args(json!({"project": project_id})))
        .await;
    let events = activity["events"].as_array().unwrap();
    assert!(!events.is_empty());
    for e in events {
        assert_eq!(
            e["session_id"], SESSION,
            "every write in one conversation must carry its session id (G3, REQ-2)"
        );
    }
}

// --- UC-1: agent orientation --------------------------------------------

#[tokio::test]
async fn uc1_agent_orientation_in_one_call() {
    // "A fresh session calls one tool and receives a compact digest. It is now
    // oriented without reading a single file."
    let d = Daemon::start().await;
    seed(&d).await;

    let text = d
        .call_text("specline_context", args(json!({"project": "harbour"})))
        .await;

    // The digest must actually orient: what it is, what is urgent, what is
    // unresolved, what words mean.
    assert!(text.contains("Harbour"), "{text}");
    assert!(text.contains("Needs attention"), "{text}");
    assert!(text.contains("Open questions"), "{text}");
    assert!(text.contains("Glossary"), "{text}");
    assert!(text.contains("Metering v1"), "the active milestone: {text}");

    let digest = d
        .call("specline_context", args(json!({"project": "harbour"})))
        .await;
    assert_eq!(
        digest["session_id"], SESSION,
        "the digest echoes the session so a long conversation can self-check (§6.5)"
    );

    // REQ-3: sized to fit comfortably in an agent's context window.
    let tokens = digest["estimated_tokens"].as_u64().unwrap();
    assert!(
        tokens < 4_000,
        "the digest is {tokens} tokens; REQ-3 budgets 3–4k"
    );
    assert_eq!(digest["budget_exceeded"], false);

    // And the cross-project roll-up.
    let all = d.call_text("specline_context", args(json!({}))).await;
    assert!(all.contains("All projects"), "{all}");
    assert!(all.contains("harbour"), "{all}");
}

#[tokio::test]
async fn the_digest_never_truncates_questions_or_terms() {
    // SPEC §6.3. A truncated task list makes an agent less informed; a
    // truncated question register makes it confidently wrong.
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    for i in 0..60 {
        d.call(
            "specline_create",
            args(json!({
                "type": "question", "project": project_id,
                "title": format!("Open question number {i} that is deliberately quite long so \
                                  that sixty of them comfortably exceed any sane digest budget"),
                // Prose-bearing types arrive with prose (KEEL-171). This test
                // is about a digest that has to leave things out, so the sixty
                // rows only need to be real rows.
                "body": format!("The reasoning behind open question number {i}."),
            })),
        )
        .await;
        d.call(
            "specline_create",
            args(json!({
                "type": "term", "project": project_id,
                "title": format!("Term{i}"),
                "body": format!("The definition of term {i}, also written at length so the \
                                 glossary alone pushes past the budget.")
            })),
        )
        .await;
    }

    let digest = d
        .call("specline_context", args(json!({"project": project_id})))
        .await;

    assert!(
        digest["questions"].as_array().unwrap().len() >= 60,
        "questions were trimmed: {} returned",
        digest["questions"].as_array().unwrap().len()
    );
    assert!(
        digest["terms"].as_array().unwrap().len() >= 60,
        "terms were trimmed: {} returned",
        digest["terms"].as_array().unwrap().len()
    );
    assert_eq!(
        digest["budget_exceeded"], true,
        "over budget because the unbounded sections are returned in full — that is the \
         designed behaviour, and it is real information about the project"
    );

    let text = d
        .call_text("specline_context", args(json!({"project": project_id})))
        .await;
    assert!(
        text.contains("never trimmed"),
        "the response must say why it is large: {}",
        &text[text.len().saturating_sub(400)..]
    );
}

// --- UC-3: implementation handoff ---------------------------------------

#[tokio::test]
async fn uc3_implementation_handoff() {
    // "A Claude Code session asks for the current spec and the tasks under the
    // active milestone, implements one, and marks it done with a link to the
    // PR."
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    // Find the spec, and what implements it — the inbound traversal.
    let hits = d
        .call(
            "specline_search",
            args(json!({"query": "idempotent ingest", "project": project_id})),
        )
        .await;
    let spec_id = hits["hits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["entity_type"] == "spec")
        .expect("the spec should be findable by its content")["entity_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let traced = d
        .call(
            "specline_get",
            args(json!({
                "ids": [spec_id], "depth": 2, "direction": "inbound", "rels": ["implements"]
            })),
        )
        .await;
    let neighbours = traced["artifacts"][0]["neighbours"].as_array().unwrap();
    assert!(
        !neighbours.is_empty(),
        "UC-7: inbound `implements` must find the tasks that implement this spec"
    );
    let task_id = neighbours[0]["id"].as_str().unwrap().to_owned();

    // Read it, then close it with the PR.
    let task = d
        .call("specline_get", args(json!({"ids": [task_id]})))
        .await;
    // `version` is lifted to the top of the entity precisely so this is a
    // straight copy into specline_update rather than a hunt inside `audit`.
    let version = task["artifacts"][0]["entity"]["version"]
        .as_i64()
        .expect("specline_get must surface `version` where specline_update asks for it");

    let linked = d
        .call(
            "specline_update",
            args(json!({
                "id": task_id,
                "version": version,
                "changes": {
                    // A list since TQ-23: a task routinely spans a pull request
                    // and the issue it closes.
                    "external_refs": [
                        "https://github.com/kb/harbour/pull/128",
                        "https://github.com/kb/harbour/issues/91"
                    ]
                }
            })),
        )
        .await;
    assert_eq!(linked["entity"]["version"], version + 1);
    assert_eq!(
        linked["entity"]["external_refs"].as_array().map(Vec::len),
        Some(2)
    );

    // Finishing it is `specline_close`, not a status change. The PR that shipped it
    // is the evidence, which is the shape this use case was already reaching for
    // when it attached the URL by hand.
    let done = d
        .call(
            "specline_close",
            args(json!({
                "id": task_id,
                "reason": "done",
                "message": "Ingest is idempotent on the client-supplied key, so a re-send is \
                            a no-op.",
                "evidence": ["pr:https://github.com/kb/harbour/pull/128"]
            })),
        )
        .await;
    assert_eq!(done["task"]["status"], "done");
    assert_eq!(done["task"]["close_reason"], "done");

    // The timeline shows it.
    let activity = d
        .call("specline_activity", args(json!({"project": project_id})))
        .await;
    let status_changes: Vec<&Value> = activity["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action"] == "status_changed")
        .collect();
    assert!(!status_changes.is_empty());
    assert_eq!(status_changes.last().unwrap()["after"], "done");
}

// --- UC-4: customer feedback triage -------------------------------------

#[tokio::test]
async fn uc4_customer_feedback_triage() {
    // "KB types three raw notes. A week later he asks what customers said about
    // onboarding — Claude searches all feedback, clusters it, proposes tasks
    // and a spec amendment."
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    let notes = [
        (
            "Onboarding takes four days",
            "Cobalt say it takes four days to change plan and three of those are waiting for a reply.",
        ),
        (
            "Onboarding felt slow",
            "Northwind's finance team found the initial setup confusing and slow.",
        ),
        (
            "Loved the dashboard",
            "Meridian screenshot the usage dashboard into their weekly review.",
        ),
    ];
    let mut feedback_ids = Vec::new();
    for (summary, body) in notes {
        let f = d
            .call(
                "specline_create",
                args(json!({
                    "type": "feedback", "project": project_id,
                    "title": summary, "body": body,
                    "fields": { "kind": "interview", "sentiment": "negative" }
                })),
            )
            .await;
        feedback_ids.push(f["entity"]["id"].as_str().unwrap().to_owned());
    }

    // A week later: search across every piece of feedback.
    let hits = d
        .call(
            "specline_search",
            args(json!({ "query": "onboarding is slow", "types": ["feedback"] })),
        )
        .await;
    let found = hits["hits"].as_array().unwrap();
    assert!(
        found.len() >= 2,
        "both onboarding complaints should surface, got {}",
        found.len()
    );
    assert!(found.iter().all(|h| h["entity_type"] == "feedback"));

    // Propose a task and connect it to the evidence.
    let task = d
        .call(
            "specline_create",
            args(json!({
                "type": "task", "project": project_id,
                "title": "Shorten the onboarding flow",
                "summary": "New users drop out partway through signup. Done when the flow is \
                            shorter and completion is measured.",
                "fields": { "priority": "p1" }
            })),
        )
        .await;
    let task_id = task["entity"]["id"].as_str().unwrap().to_owned();

    let spec = d
        .call(
            "specline_create",
            args(json!({
                "type": "spec", "project": project_id,
                "title": "Onboarding redesign",
                "body": "Derived from customer interviews.",
                "fields": { "kind": "spec" }
            })),
        )
        .await;
    let spec_id = spec["entity"]["id"].as_str().unwrap().to_owned();

    d.call(
        "specline_link",
        args(json!({"from": spec_id, "rel": "derived_from", "to": feedback_ids[0]})),
    )
    .await;
    d.call(
        "specline_link",
        args(json!({"from": feedback_ids[1], "rel": "informs", "to": spec_id})),
    )
    .await;
    d.call(
        "specline_link",
        args(json!({"from": task_id, "rel": "implements", "to": spec_id})),
    )
    .await;

    // The evidence trail is walkable in both directions.
    let from_spec = d
        .call(
            "specline_get",
            args(json!({"ids": [spec_id], "depth": 1, "direction": "outbound"})),
        )
        .await;
    let outbound = from_spec["artifacts"][0]["neighbours"].as_array().unwrap();
    assert!(
        outbound.iter().any(|n| n["id"] == feedback_ids[0].as_str()),
        "outbound from the spec should reach the feedback it derives from"
    );

    let into_spec = d
        .call(
            "specline_get",
            args(json!({"ids": [spec_id], "depth": 1, "direction": "inbound"})),
        )
        .await;
    let inbound = into_spec["artifacts"][0]["neighbours"].as_array().unwrap();
    assert!(
        inbound.iter().any(|n| n["id"] == task_id.as_str()),
        "inbound to the spec should reach the task implementing it"
    );
}

// --- Behaviour an agent depends on --------------------------------------

#[tokio::test]
async fn a_retried_create_does_not_duplicate() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    let first = d
        .call(
            "specline_create",
            args(
                json!({"type": "task", "project": project_id, "title": "Ship the thing",
                "summary": "The thing is built and not released. Done when it is on the server."}),
            ),
        )
        .await;
    let second = d
        .call(
            "specline_create",
            args(
                json!({"type": "task", "project": project_id, "title": "Ship the thing",
                "summary": "The thing is built and not released. Done when it is on the server."}),
            ),
        )
        .await;

    assert_eq!(first["created"], true);
    assert_eq!(second["created"], false);
    assert_eq!(first["entity"]["id"], second["entity"]["id"]);
}

#[tokio::test]
async fn a_stale_update_returns_409_with_enough_to_merge() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;
    let task = d
        .call(
            "specline_create",
            args(json!({"type": "task", "project": project_id, "title": "Contended",
                "summary": "Two writers touch this row at once. Done when neither loses an update."})),
        )
        .await;
    let id = task["entity"]["id"].as_str().unwrap().to_owned();

    d.call(
        "specline_update",
        args(json!({"id": id, "version": 1, "changes": {"status": "in_progress"}})),
    )
    .await;

    let (status, error) = d
        .call_err(
            "specline_update",
            args(json!({"id": id, "version": 1, "changes": {"status": "done"}})),
        )
        .await;

    assert_eq!(status, 409);
    assert_eq!(error["data"]["latest_version"], 2);
    assert_eq!(
        error["data"]["current_state"]["status"], "in_progress",
        "the loser needs the current state to merge against (SPEC §7.3)"
    );
    assert!(
        error["data"]["events_since"]
            .as_array()
            .is_some_and(|e| !e.is_empty()),
        "and the events since its read, so it can usually resolve this itself"
    );
}

#[tokio::test]
async fn depends_on_is_stored_as_blocks_and_the_response_says_so() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;
    let a = d
        .call(
            "specline_create",
            args(json!({"type": "task", "project": project_id, "title": "A",
                "summary": "The first of a pair, so the link between them has two ends."})),
        )
        .await["entity"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let b = d
        .call(
            "specline_create",
            args(json!({"type": "task", "project": project_id, "title": "B",
                "summary": "The second of a pair, so the link between them has two ends."})),
        )
        .await["entity"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let text = d
        .call_text(
            "specline_link",
            args(json!({"from": a, "rel": "depends_on", "to": b})),
        )
        .await;
    assert!(
        text.contains("stored as") && text.contains("blocks"),
        "a normalised link must say what was actually written, or the next reader \
         concludes the endpoints went in backwards: {text}"
    );
}

#[tokio::test]
async fn an_invalid_enum_value_tells_the_agent_what_would_work() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;
    let (status, error) = d
        .call_err(
            "specline_create",
            args(json!({
                "type": "task", "project": project_id, "title": "Bad status",
                "summary": "Sends a status the enum does not have, to see what the error says.",
                "fields": { "status": "finished" }
            })),
        )
        .await;

    assert_eq!(status, 400);
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("finished"), "{message}");
    assert!(
        message.contains("wont_do") || message.contains("in_progress"),
        "an agent must be able to retry from the message alone: {message}"
    );
}

/// House style is refused at the authoring boundary, on every prose field.
///
/// B-46. The check lives in `specline-core` so the CLI and MCP cannot diverge, but
/// it is asserted here because the tool surface is where prose is authored.
#[tokio::test]
async fn machine_written_prose_is_refused_with_a_replacement() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;
    let (status, error) = d
        .call_err(
            "specline_create",
            args(json!({
                "type": "decision", "project": project_id, "title": "Use one parser",
                "body": "We should leverage the existing parser in order to avoid duplication."
            })),
        )
        .await;

    assert_eq!(status, 400);
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("leverage"), "{message}");
    assert!(
        message.contains("use"),
        "an agent must be able to retry from the message alone: {message}"
    );
}

/// Quoted material is someone else's words, and refusing it would stop the
/// store recording the world as it is.
///
/// This is the exemption that makes the rule usable at all: without it, a note
/// carrying an error message or a decision quoting what a customer said becomes
/// unwritable.
#[tokio::test]
async fn a_quoted_error_message_is_not_refused_as_house_style() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;
    let created = d
        .call(
            "specline_create",
            args(json!({
                "type": "decision", "project": project_id, "title": "Pin the pool size",
                "body": "The driver said:\n\n> Failed to utilize the connection pool\n\n\
                         That wording is theirs, and the fix is to pin the size at eight."
            })),
        )
        .await;

    assert_eq!(
        created["created"],
        json!(true),
        "quotation is not authorship"
    );
}

/// A soft tell warns rather than refusing, and the warning rides along with the
/// write that landed.
///
/// Refusing on a signal is how a model learns to write around the check instead
/// of writing plainly — it swaps the word and keeps the shape.
#[tokio::test]
async fn a_soft_tell_lands_the_write_and_says_so() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;
    let created = d
        .call(
            "specline_create",
            args(json!({
                "type": "decision", "project": project_id, "title": "Cache the digest",
                "body": "Recomputing the digest per call is a crucial cost we can avoid by \
                         caching it against the latest event id."
            })),
        )
        .await;

    assert_eq!(created["created"], json!(true), "a signal must not block");
    let warnings = created["style_warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["found"], json!("crucial"));
}

/// A milestone reaches the roadmap with an explainer or it does not reach it.
///
/// This is asserted at the daemon rather than only in `specline-core` because the
/// MCP path is where the bug was: `specline_create` accepted a `body` for a
/// milestone and discarded it, so every milestone written over the tool surface
/// landed as a bare name and the caller was told it had succeeded. A store-level
/// test would not have caught that — the store was never asked. B-45.
#[tokio::test]
async fn a_milestone_without_an_explainer_is_refused_over_mcp() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;
    let (status, error) = d
        .call_err(
            "specline_create",
            args(json!({
                "type": "milestone", "project": project_id, "title": "Phase 9"
            })),
        )
        .await;

    assert_eq!(status, 400);
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("summary"), "{message}");
    assert!(
        message.contains("one or two sentences"),
        "an agent must be able to retry from the message alone: {message}"
    );
}

/// The generic prose field is accepted for a milestone rather than dropped.
///
/// A caller reaching for `body` means the same thing here, and silently
/// discarding it is the exact failure this work exists to remove.
#[tokio::test]
async fn a_milestone_takes_its_explainer_from_body_as_well_as_summary() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;
    let created = d
        .call(
            "specline_create",
            args(json!({
                "type": "milestone", "project": project_id, "title": "Phase 9",
                "body": "Fold DuckDB and Lance into one database."
            })),
        )
        .await;

    assert_eq!(
        created["entity"]["summary"].as_str(),
        Some("Fold DuckDB and Lance into one database."),
        "the body became the explainer rather than being dropped"
    );
}

#[tokio::test]
async fn a_near_duplicate_project_requires_confirmation() {
    // UC-8 / REQ-8. The defence against nine near-identical projects.
    let d = Daemon::start().await;
    seed(&d).await;

    let lookup = d
        .call(
            "specline_projects",
            args(json!({"query": "harbour billing"})),
        )
        .await;
    assert_eq!(
        lookup["requires_confirmation"], true,
        "a near miss must ask, not create"
    );
    assert!(!lookup["projects"].as_array().unwrap().is_empty());

    let text = d
        .call_text(
            "specline_projects",
            args(json!({"query": "harbour billing"})),
        )
        .await;
    assert!(text.contains("Ask the human"), "{text}");

    // An exact match resolves without a prompt.
    let exact = d
        .call("specline_projects", args(json!({"query": "harbour"})))
        .await;
    assert_eq!(exact["requires_confirmation"], false);
}

#[tokio::test]
async fn a_document_can_be_revised_and_diffed_over_mcp() {
    // REQ-2: any two versions can be fetched and diffed via MCP, not only in
    // the UI.
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    let spec = d
        .call(
            "specline_create",
            args(json!({
                "type": "decision", "project": project_id,
                "title": "Aggregate hourly", "body": "## Decision\n\nHourly buckets.\n"
            })),
        )
        .await;
    let id = spec["entity"]["id"].as_str().unwrap().to_owned();

    let revised = d
        .call(
            "specline_write_doc",
            args(json!({
                "id": id,
                "body": "## Decision\n\nHourly buckets.\n\n## Consequences\n\nPer-minute would \
                         multiply storage by sixty.\n"
            })),
        )
        .await;
    assert_eq!(revised["document"]["version"], 2);
    assert_eq!(revised["created_revision"], true);

    // Writing the same content again is a no-op.
    let again = d
        .call(
            "specline_write_doc",
            args(json!({
                "id": id,
                "body": "## Decision\n\nHourly buckets.\n\n## Consequences\n\nPer-minute would \
                         multiply storage by sixty.\n"
            })),
        )
        .await;
    assert_eq!(again["created_revision"], false);
    assert_eq!(again["document"]["version"], 2);

    let diffed = d
        .call(
            "specline_get",
            args(json!({"ids": [id], "version": 2, "diff_against": 1})),
        )
        .await;
    let diff = &diffed["artifacts"][0]["diff"];
    assert!(diff["added"].as_u64().unwrap() > 0, "{diff}");
    assert!(
        diff["unified"].as_str().unwrap().contains("Consequences"),
        "{diff}"
    );
}

#[tokio::test]
async fn asking_for_something_that_does_not_exist_says_so_rather_than_returning_less() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;
    let real = d
        .call(
            "specline_create",
            args(json!({"type": "task", "project": project_id, "title": "Real",
                "summary": "A row that exists, so a lookup for one that does not can be told apart."})),
        )
        .await["entity"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let ghost = "tsk_01ZZZZZZZZZZZZZZZZZZZZZZZZ";
    let result = d
        .call("specline_get", args(json!({"ids": [real, ghost]})))
        .await;

    assert_eq!(result["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["not_found"].as_array().unwrap().len(),
        1,
        "an agent given fewer artifacts than it asked for, with no indication, \
         will assume the missing ones do not exist"
    );

    let text = d
        .call_text("specline_get", args(json!({"ids": [ghost]})))
        .await;
    assert!(text.contains("not found"), "{text}");
}

#[tokio::test]
async fn the_local_api_serves_the_same_data_as_mcp() {
    let d = Daemon::start().await;
    seed(&d).await;

    let health: Value = d
        .client
        .get(format!("{}/api/health", d.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["status"], "ok");
    assert_eq!(health["protocol"], PROTOCOL_VERSION);
    assert_eq!(health["projects"], 1);

    let context: Value = d
        .client
        .get(format!("{}/api/context?project=harbour", d.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(context["data"]["project"]["slug"], "harbour");
    assert!(context["summary"].as_str().unwrap().contains("Harbour"));
}

/// Seed one project with enough shape for the digest to be meaningful.
///
/// Returns the project id.
async fn seed(d: &Daemon) -> String {
    let project = d
        .call(
            "specline_create",
            args(json!({
                "type": "project", "title": "Harbour",
                "body": "Usage-based billing for API companies."
            })),
        )
        .await;
    let project_id = project["entity"]["id"].as_str().unwrap().to_owned();

    let milestone = d
        .call(
            "specline_create",
            args(json!({
                "type": "milestone", "project": project_id, "title": "Metering v1",
                "summary": "Charge customers for what they actually use, and show them the bill.",
                // No status. `active` is derived from the tasks now, and
                // asking for it is refused — which is the point of B-57.
            })),
        )
        .await["entity"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let spec = d
        .call(
            "specline_create",
            args(json!({
                "type": "spec", "project": project_id, "title": "Usage metering",
                "body": "# Metering\n\n## REQ-1 Idempotent ingest\n\nEvery usage event carries a \
                         client-supplied idempotency key so that re-sending it is a no-op.",
                "fields": { "kind": "prd", "status": "approved" }
            })),
        )
        .await["entity"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let task = d
        .call(
            "specline_create",
            args(json!({
                "type": "task", "project": project_id,
                "title": "Dedupe usage events by idempotency key",
                "summary": "Retries write the same usage event twice and customers get billed \
                            for both. Done when a repeated key is a no-op.",
                "fields": { "priority": "p0", "milestone_id": milestone }
            })),
        )
        .await["entity"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    d.call(
        "specline_link",
        args(json!({"from": task, "rel": "implements", "to": spec, "anchor": "REQ-1"})),
    )
    .await;

    d.call(
        "specline_create",
        args(json!({
            "type": "question", "project": project_id,
            "title": "Does a downgrade take effect immediately or at period end?",
            // Prose-bearing types arrive with prose (KEEL-171).
            "body": "Immediately is what people expect and at period end is what the \
                     invoice can express. Nobody has decided."
        })),
    )
    .await;

    d.call(
        "specline_create",
        args(json!({
            "type": "term", "project": project_id, "title": "Meter",
            "body": "A named quantity being counted for billing."
        })),
    )
    .await;

    project_id
}

/// A client that will not stop is refused rather than served.
///
/// The threat is not abuse — this daemon binds to loopback and serves one
/// person. It is an agent in a loop: a model retrying a failing call as fast as
/// the transport allows holds the store's global write lock and makes the
/// product unusable for the human sitting in front of it. The MCP specification
/// lists rate limiting under what a server should do, and there was none.
#[tokio::test]
async fn a_client_in_a_loop_is_rate_limited() {
    let d = Daemon::start().await;

    // Has to actually exceed the burst rather than assert against a number
    // small enough to be a coincidence.
    //
    // **`ping`, not `tools/list`, and that is not a detail.** The bucket refills
    // at 50 a second, so a sequential loop only outruns it if each call is
    // faster than 20ms. This test used to hammer `tools/list` and started
    // failing the moment that response grew from ten tool definitions to
    // thirteen — the limiter was fine and the test's margin was not. `ping`
    // returns `{}` and passes through the same check, which is the one the test
    // is about, so the assertion no longer depends on how big some other
    // endpoint's response happens to be.
    //
    // Concurrently, too. A client in a loop does not wait for each answer, and
    // firing them in batches is both a better likeness and the thing that makes
    // the burst genuinely unreachable by refill.
    let mut limited = None;
    'outer: for _ in 0..8 {
        let batch = futures_util::future::join_all((0..200).map(|_| d.rpc("ping", json!({}))));
        for (status, body) in batch.await {
            if status == 429 {
                limited = Some(body);
                break 'outer;
            }
        }
    }

    let body = limited.expect("a caller that never pauses must eventually be refused");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("rate limited"), "{message}");
    // Actionable, like every other error on this surface: a model reading it
    // should stop hammering rather than retry immediately.
    assert!(message.contains("Retry in"), "{message}");
}

/// And the limit does not hold ordinary work back.
///
/// A limiter that interrupts a session making its normal handful of calls in a
/// row would be a worse bug than the one it prevents.
#[tokio::test]
async fn an_ordinary_run_of_calls_is_not_limited() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    for _ in 0..10 {
        let (status, _) = d
            .rpc(
                "tools/call",
                json!({ "name": "specline_context", "arguments": args(json!({ "project": project_id })) }),
            )
            .await;
        assert_eq!(status, 200, "a normal session must not be throttled");
    }
}

/// The Phase 8 exit criterion for 8F: this project says "phase" on every
/// screen, and `specline_create(type: "phase")` used to fail with an enum error
/// listing thirteen types, none of which was the word.
#[tokio::test]
async fn a_project_can_say_phase_and_be_understood() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    let created = d
        .call(
            "specline_create",
            args(json!({
                "type": "phase", "project": project_id, "title": "Phase 9 — One database",
                "summary": "Fold DuckDB and Lance into one database."
            })),
        )
        .await;

    assert_eq!(created["entity"]["type"], json!("milestone"));
    assert_eq!(created["resolved_from"], json!("phase"));
}

/// And it says so, rather than succeeding silently.
///
/// A silent success teaches the session nothing and it guesses the same way
/// next time. The narration is the whole point of carrying the alias back.
#[tokio::test]
async fn saying_phase_is_narrated_rather_than_quietly_accepted() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    let text = d
        .call_text(
            "specline_create",
            args(json!({
                "type": "sprint", "project": project_id, "title": "Sprint 4",
                "summary": "Ship the intake form and the triage column."
            })),
        )
        .await;

    assert!(text.contains("sprint"), "{text}");
    assert!(text.contains("milestone"), "{text}");
}

/// Failure case: an alias is a spelling, not an escape hatch.
#[tokio::test]
async fn a_word_nobody_taught_it_still_fails_usefully() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    let (status, error) = d
        .call_err(
            "specline_create",
            args(json!({ "type": "widget", "project": project_id, "title": "A widget" })),
        )
        .await;

    assert_eq!(status, 400);
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("widget"), "{message}");
    assert!(
        message.contains("milestone"),
        "the valid names must be listed: {message}"
    );
}

/// The 8G exit criterion: no task can be created without a summary, and the
/// MCP path refuses it as surely as the store does.
#[tokio::test]
async fn a_task_without_a_summary_is_refused_over_mcp() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    let (status, error) = d
        .call_err(
            "specline_create",
            args(json!({ "type": "task", "project": project_id, "title": "Do the thing" })),
        )
        .await;

    assert_eq!(status, 400);
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("summary"), "{message}");
    assert!(
        message.contains("done looks like"),
        "an agent must be able to retry from the message alone: {message}"
    );
}

/// And a good one round-trips, so the requirement is a gate rather than a wall.
#[tokio::test]
async fn a_task_with_a_summary_keeps_it() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    let created = d
        .call(
            "specline_create",
            args(json!({
                "type": "task", "project": project_id,
                "title": "Show the milestone on every row",
                "summary": "The board never says which phase a task is in, so you have to open \
                            each one. Done when every row shows it."
            })),
        )
        .await;

    assert!(
        created["entity"]["summary"]
            .as_str()
            .unwrap_or_default()
            .starts_with("The board never says"),
        "{created}"
    );
}

// --- The three verbs (Phase 8, §8A) --------------------------------------

/// `specline ready` promises one ranking behind three doors. This is the assertion
/// that they are the same door.
///
/// The CLI is not spawned as a process here — it calls `specline_next` over this
/// same endpoint, which is the property worth pinning: the tool, the REST
/// endpoint the app reads, and `specline_core::ready` itself return the same list in
/// the same order. If the app ever disagreed with the session, this is the test
/// that would have caught it.
#[tokio::test]
async fn ready_gives_the_same_answer_over_mcp_and_over_the_local_api() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;

    for (title, priority) in [
        ("Rebuild the ingest path", "p2"),
        ("Rename the meter column", "p1"),
        ("Ship the invoice screen", "p0"),
    ] {
        d.call(
            "specline_create",
            args(json!({
                "type": "task", "project": project_id, "title": title,
                "summary": format!("{title}. Done when it works and there is a test."),
                "fields": { "priority": priority }
            })),
        )
        .await;
    }

    let over_mcp = d
        .call("specline_next", args(json!({"project": project_id})))
        .await;

    let over_rest: Value = d
        .client
        .get(format!("{}/api/ready?project={project_id}", d.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let refs = |v: &Value| -> Vec<String> {
        v["ready"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["reference"].as_str().unwrap().to_owned())
            .collect()
    };

    let from_mcp = refs(&over_mcp);
    assert!(!from_mcp.is_empty(), "seeded work should be ready");
    assert_eq!(
        from_mcp,
        refs(&over_rest["data"]),
        "the app and the session must be reading one ranking, in one order"
    );
    assert_eq!(over_mcp["total"], over_rest["data"]["total"]);
}

#[tokio::test]
async fn claiming_shows_on_the_row_and_a_second_session_is_refused() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;
    let task = d
        .call(
            "specline_create",
            args(json!({
                "type": "task", "project": project_id, "title": "Only one of us",
                "summary": "A task two sessions will both try to take. Done when one of them \
                            is told who has it."
            })),
        )
        .await["entity"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let claimed = d.call("specline_claim", args(json!({"id": task}))).await;
    assert_eq!(claimed["task"]["status"], "in_progress");
    assert_eq!(claimed["task"]["claimed_by"], SESSION);

    // A different conversation, which is the case the claim exists for.
    let (status, error) = d
        .call_err(
            "specline_claim",
            json!({"id": task, "session_id": "ses_someone_else", "surface": "code"}),
        )
        .await;
    assert_eq!(status, 400);
    let message = error["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(SESSION),
        "the refusal names the holder, so the caller knows who to ask: {message}"
    );

    // And the ranked list can be asked to leave claimed work out.
    let unclaimed = d
        .call(
            "specline_next",
            args(json!({"project": project_id, "unclaimed": true})),
        )
        .await;
    assert!(
        !unclaimed["ready"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == task.as_str()),
        "a claimed task is not unclaimed work"
    );
}

// The Phase 8 exit criterion, over the real transport: a task cannot reach
// `done` without a reason, a message and evidence, and `specline_update` is held to
// the same rule as `specline_close`.
#[tokio::test]
async fn a_task_cannot_be_finished_without_saying_why_or_showing_the_work() {
    let d = Daemon::start().await;
    let project_id = seed(&d).await;
    let created = d
        .call(
            "specline_create",
            args(json!({
                "type": "task", "project": project_id, "title": "Finished properly or not at all",
                "summary": "A task used to check that closing states a reason. Done when the \
                            bare status change is refused."
            })),
        )
        .await;
    let task = created["entity"]["id"].as_str().unwrap().to_owned();
    let version = created["entity"]["version"].as_i64().unwrap();

    // The workaround path: move the status directly. Refused, which is what
    // makes the rule an invariant rather than a convention in a markdown file.
    let (status, error) = d
        .call_err(
            "specline_update",
            args(json!({
                "id": task, "version": version, "changes": { "status": "done" }
            })),
        )
        .await;
    assert_eq!(status, 400);
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("why"),
        "{error}"
    );

    // The tool, with no evidence. Also refused.
    let (_, error) = d
        .call_err(
            "specline_close",
            args(json!({
                "id": task, "reason": "done", "message": "It is finished, honestly."
            })),
        )
        .await;
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("evidence"),
        "{error}"
    );

    // And properly.
    let closed = d
        .call(
            "specline_close",
            args(json!({
                "id": task, "reason": "done",
                "message": "The close path now asks for the reason, the message and the \
                            evidence together.",
                "evidence": ["commit:0f1e2d3", "test:cargo test -p specline-daemon"]
            })),
        )
        .await;
    assert_eq!(closed["task"]["status"], "done");
    assert_eq!(closed["task"]["close_reason"], "done");
    assert!(closed["task"]["closed_at"].is_string());
}
