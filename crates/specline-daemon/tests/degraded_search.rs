//! What happens when the embedding model cannot load.
//!
//! This is the branch the whole "search degrades, it never fails" argument
//! rests on, and nothing exercised it. The failure it guards against is the one
//! this project keeps returning to: results keep arriving, they are merely
//! worse, and nothing says so. A store went months with 227 unembedded
//! documents for exactly that reason.
//!
//! So the assertion here is not "search works". It is "search works *and* the
//! daemon is still up", against a model cache that cannot possibly load —
//! because a daemon that dies when a 130 MB download fails is worse than one
//! with weaker search, and a daemon that hangs waiting for it is worse than
//! both.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use specline_daemon::{AppState, http::router};
use std::time::Duration;

/// A home whose `models` path is a *file*, so nothing can ever put a model in
/// it.
///
/// Deliberately not "a directory with the wrong bytes in it": that would
/// exercise whatever fastembed does with a corrupt ONNX file, which is
/// fastembed's business. A path that cannot be a directory fails immediately
/// and offline, which is what a test wants — the branch under test is Specline's
/// reaction, not the loader's diagnosis.
fn poisoned_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("models"), b"not a directory").unwrap();
    dir
}

async fn daemon(home: &std::path::Path) -> String {
    let state = AppState::open(home, true).expect("the daemon must open despite the model cache");
    let app = router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn tool_call(name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    })
}

/// The daemon boots, serves, and searches — with no model and no waiting.
#[tokio::test]
async fn a_broken_model_cache_leaves_keyword_search_working() {
    let home = poisoned_home();

    // Bounded, because the original failure mode was the opposite of a crash:
    // the model used to load inline before the socket was bound, so a first run
    // left the daemon unreachable for the length of a download. Booting has to
    // be immediate whether the model loads or not.
    let base = tokio::time::timeout(Duration::from_secs(10), daemon(home.path()))
        .await
        .expect("the daemon must bind its socket without waiting for a model");

    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{base}/api/health"))
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "a daemon that cannot load its model must still be alive"
    );

    // Something to find.
    let created = client
        .post(format!("{base}/mcp"))
        .json(&tool_call(
            "specline_create",
            json!({"type": "project", "title": "Metering", "slug": "metering"}),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 200);

    client
        .post(format!("{base}/mcp"))
        .json(&tool_call(
            "specline_create",
            json!({
                "type": "decision",
                "project": "metering",
                "title": "Aggregate hourly, not per-minute",
                "body": "Per-minute buckets cost more than they are worth.\n"
            }),
        ))
        .send()
        .await
        .unwrap();

    let response = client
        .post(format!("{base}/mcp"))
        .json(&tool_call("specline_search", json!({"query": "hourly"})))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let payload: Value = response.json().await.unwrap();
    let hits = payload
        .pointer("/result/structuredContent/hits")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no hits array in {payload}"));
    assert!(
        !hits.is_empty(),
        "the keyword half must answer on its own; a search that returns nothing here \
         would read to a model as an empty store: {payload}"
    );
    assert!(
        hits.iter().any(|h| h
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|s| s == "keyword")),
        "and the hits should say they came from the keyword half: {hits:?}"
    );
}

/// A degraded search has to say it was degraded, in the response itself.
///
/// The half that did not run leaves no trace in `hits` — the results of a
/// keyword-only search and a hybrid one are the same shape, the same fields and
/// often the same rows. So the only place the difference can live is a field
/// that names it, and the case that matters most is the one with no hits at
/// all: "no matches" from half a search reads as a fact about the store, and a
/// model that believes it goes and writes down again what was already there.
#[tokio::test]
async fn a_search_with_no_model_says_which_halves_ran() {
    let home = poisoned_home();
    let base = tokio::time::timeout(Duration::from_secs(10), daemon(home.path()))
        .await
        .expect("the daemon must bind its socket without waiting for a model");
    let client = reqwest::Client::new();

    client
        .post(format!("{base}/mcp"))
        .json(&tool_call(
            "specline_create",
            json!({"type": "project", "title": "Metering", "slug": "metering"}),
        ))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{base}/mcp"))
        .json(&tool_call(
            "specline_create",
            json!({
                "type": "decision",
                "project": "metering",
                "title": "Aggregate hourly, not per-minute",
                "body": "Per-minute buckets cost more than they are worth.\n"
            }),
        ))
        .send()
        .await
        .unwrap();

    let found: Value = client
        .post(format!("{base}/mcp"))
        .json(&tool_call("specline_search", json!({"query": "hourly"})))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        found.pointer("/result/structuredContent/searched"),
        Some(&json!(["keyword"])),
        "only the keyword half ran and the response has to name it: {found}"
    );
    let why = found
        .pointer("/result/structuredContent/not_searched/semantic")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no reason for the silent half in {found}"));
    assert!(
        why.contains("embedding model"),
        "the reason must name what is missing: {why}"
    );

    // And in the prose, because a model reads that first and some clients show
    // nothing else.
    let text = found
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        text.contains("partial search"),
        "a degraded search must say so in its summary: {text}"
    );

    // The dangerous case. Nothing matched, and the answer must not be read as
    // "the store has nothing about this".
    let empty: Value = client
        .post(format!("{base}/mcp"))
        .json(&tool_call(
            "specline_search",
            json!({"query": "kubernetes ingress"}),
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        empty
            .pointer("/result/structuredContent/hits")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "the setup is wrong if this matched something: {empty}"
    );
    let text = empty
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        text.contains("not \u{200b}evidence") || text.contains("not evidence"),
        "an empty half-search must refuse to be read as an empty store: {text}"
    );
}

/// A model that never arrives must not make writes fail either.
///
/// Embedding happens on the way into a revision. If a missing model turned that
/// into an error, a store with no network would refuse to record prose at all —
/// the opposite of degrading.
#[tokio::test]
async fn prose_can_still_be_written_with_no_embedder() {
    let home = poisoned_home();
    let base = daemon(home.path()).await;
    let client = reqwest::Client::new();

    client
        .post(format!("{base}/mcp"))
        .json(&tool_call(
            "specline_create",
            json!({"type": "project", "title": "Offline", "slug": "offline"}),
        ))
        .send()
        .await
        .unwrap();

    let created: Value = client
        .post(format!("{base}/mcp"))
        .json(&tool_call(
            "specline_create",
            json!({
                "type": "spec",
                "project": "offline",
                "title": "A specification",
                // Prose-bearing types arrive with prose (KEEL-171). This one
                // then gets a second revision below, which is what the test is
                // actually about.
                "body": "The first revision, written with no embedder loaded.",
            }),
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created
        .pointer("/result/structuredContent/entity/id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no id in {created}"))
        .to_owned();

    let written: Value = client
        .post(format!("{base}/mcp"))
        .json(&tool_call(
            "specline_write_doc",
            json!({"id": id, "body": "Prose written with no model anywhere.\n"}),
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        written.pointer("/result/structuredContent/document/version"),
        // Two, not one: the create carries the first revision now that a
        // prose-bearing type has to arrive with prose (KEEL-171). What this
        // test is about is that a revision lands at all with no model loaded,
        // and which number it is was never the point.
        Some(&json!(2)),
        "a revision must land without an embedder: {written}"
    );
    assert_eq!(
        written.pointer("/result/structuredContent/document/embedding"),
        Some(&Value::Null),
        "and honestly report that it has no vector rather than inventing one"
    );
}
