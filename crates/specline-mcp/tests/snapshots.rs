//! Snapshot tests for the tool surface.
//!
//! These are an API contract. An agent's behaviour is shaped by the tool
//! descriptions and by the exact shape of what comes back, so a change to
//! either is a change to the product — and it should show up as a reviewable
//! diff rather than as an agent quietly behaving differently next week.
//!
//! Ids and timestamps are redacted: they are the parts that legitimately
//! differ on every run, and a snapshot that churns is a snapshot people stop
//! reading.
//!
//! Run `cargo insta review` after an intentional change.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use specline_core::{Actor, EntityStore, Project, Provenance, Spec, Store, Task};
use specline_mcp::{ToolCall, dispatch};

/// Replace the values that legitimately change each run.
fn settings() -> insta::Settings {
    let mut s = insta::Settings::clone_current();
    // Every prefix `specline-core` mints, not only the entity ones. `nte` and `blb`
    // were missing, which nothing noticed until a tool that returns a note id
    // got a snapshot — the redaction quietly did not cover a third of the
    // connective ids, so any snapshot holding one would have churned on every
    // run until somebody stopped reading it.
    s.add_filter(
        r"(prj|tsk|spc|dec|que|trm|fbk|dsg|env|mtr|obs|art|lnk|evt|doc|nte|blb)_[0-9A-HJKMNP-TV-Z]{26}",
        "[id]",
    );
    s.add_filter(
        r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})",
        "[timestamp]",
    );
    s.add_filter(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}", "[time]");
    s.add_filter(r"\d{4}-\d{2}-\d{2}", "[date]");
    // The suffix matters, and it was missing until an `-rc.1` went past.
    // `0.1.4` redacted and `0.1.5-rc.1` did not, so the snapshot diffed on the
    // version it exists to ignore — and it failed at the one moment a release
    // is being cut, which is the worst time to be reading a spurious diff.
    s.add_filter(
        r#""version": "\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?""#,
        r#""version": "[semver]""#,
    );
    s.add_filter(r"[0-9a-f]{32}", "[hash]");
    s
}

/// A store with a small, predictable project, and the ids of what is in it.
///
/// The ids matter because most of the surface is addressed by one: `specline_get`,
/// `specline_update`, `specline_link`, `specline_note`, `specline_claim` and `specline_close` all
/// take an id, so a fixture that threw them away is a fixture only a third of
/// the tools can use. They are redacted out of the snapshots themselves.
struct Seed {
    store: Store,
    /// Held so the directory outlives the store. Never read.
    _dir: tempfile::TempDir,
    spec: specline_core::EntityId,
    task: specline_core::EntityId,
}

fn seeded() -> Seed {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let prov = Provenance::anonymous(Actor::Claude).with_session("ses_snapshot");

    let project = store
        .create(Project::new("harbour", "Harbour").into(), &prov)
        .unwrap()
        .entity
        .id()
        .clone();
    let spec = store
        .create(Spec::new(project.clone(), "Usage metering").into(), &prov)
        .unwrap()
        .entity
        .id()
        .clone();
    let task = store
        .create(
            Task::new(
                project,
                "Dedupe usage events by idempotency key",
                "A row this test needs in the store.",
            )
            .into(),
            &prov,
        )
        .unwrap()
        .entity
        .id()
        .clone();
    Seed {
        store,
        _dir: dir,
        spec,
        task,
    }
}

fn call(store: &mut Store, name: &str, arguments: Value) -> Value {
    dispatch(
        store,
        ToolCall {
            name,
            arguments: &arguments,
            client: None,
        },
    )
    .unwrap_or_else(|e| json!({ "error": { "code": e.code, "message": e.message } }))
}

#[test]
fn tool_definitions() {
    // The single most important snapshot in the repo: these descriptions are
    // the only documentation an agent gets, and changing one changes how it
    // behaves.
    settings().bind(|| {
        insta::assert_json_snapshot!("tools_list", specline_mcp::list_result());
    });
}

#[test]
fn server_discovery() {
    settings().bind(|| {
        insta::assert_json_snapshot!("server_discover", specline_mcp::discover_result());
    });
}

#[test]
fn context_digest_shape() {
    let mut seed = seeded();
    let result = call(
        &mut seed.store,
        "specline_context",
        json!({"project": "harbour"}),
    );
    settings().bind(|| {
        insta::assert_json_snapshot!("specline_context", result);
    });
}

#[test]
fn context_rollup_shape() {
    let mut seed = seeded();
    let result = call(&mut seed.store, "specline_context", json!({}));
    settings().bind(|| {
        insta::assert_json_snapshot!("specline_context_rollup", result);
    });
}

#[test]
fn create_response_shape() {
    let mut seed = seeded();
    let result = call(
        &mut seed.store,
        "specline_create",
        json!({
            "type": "decision",
            "project": "harbour",
            "title": "Aggregate hourly, not per-minute",
            "body": "## Decision\n\nHourly buckets.\n",
            "session_id": "ses_snapshot"
        }),
    );
    settings().bind(|| {
        insta::assert_json_snapshot!("specline_create", result);
    });
}

#[test]
fn search_response_shape() {
    let mut seed = seeded();
    let result = call(
        &mut seed.store,
        "specline_search",
        json!({"query": "idempotency"}),
    );
    settings().bind(|| {
        insta::assert_json_snapshot!("specline_search", result);
    });
}

#[test]
fn projects_response_shape() {
    let mut seed = seeded();
    let result = call(&mut seed.store, "specline_projects", json!({}));
    settings().bind(|| {
        insta::assert_json_snapshot!("specline_projects", result);
    });
}

#[test]
fn error_shapes() {
    // Errors are read by a model that has to work out what to send instead, so
    // their wording is as much a contract as the success path.
    let mut seed = seeded();
    let cases = json!({
        "unknown_field": call(
            &mut seed.store, "specline_update",
            json!({"id": "tsk_01H8XK4RPVBQ2N7DZM9C3FGTWY", "version": 1,
                   "changes": {"asignee": "kb"}})
        ),
        "missing_argument": call(&mut seed.store, "specline_search", json!({})),
        "unknown_project": call(
            &mut seed.store, "specline_context", json!({"project": "does-not-exist"})
        ),
        "unknown_tool": call(&mut seed.store, "specline_delete", json!({})),
        "bad_timestamp": call(
            &mut seed.store, "specline_activity", json!({"since": "last tuesday"})
        ),
    });
    settings().bind(|| {
        insta::assert_json_snapshot!("errors", cases);
    });
}

#[test]
fn every_advertised_tool_is_dispatchable_and_vice_versa() {
    // The two lists are maintained by hand in different files: `tools::all()`
    // is what a client is told exists, and the `match` in `dispatch` is what
    // actually runs. Nothing tied them together, so a tool could be advertised
    // and unimplemented, or implemented and invisible — and the only symptom
    // would be a model calling something that answers "no tool named that".
    //
    // Dispatching every advertised name with empty arguments is enough to tell
    // the two apart: a *missing* tool answers METHOD_NOT_FOUND, while a present
    // one fails on its arguments, which is a different code.
    let mut seed = seeded();

    for tool in specline_mcp::tools::all() {
        let result = dispatch(
            &mut seed.store,
            ToolCall {
                name: tool.name,
                arguments: &json!({}),
                client: None,
            },
        );
        if let Err(e) = result {
            assert!(
                !e.message.contains("no tool named"),
                "`{}` is advertised by tools::all() but dispatch has no arm for it",
                tool.name
            );
        }
    }

    // And the other direction: a name dispatch does not know must not be
    // silently tolerated.
    let unknown = dispatch(
        &mut seed.store,
        ToolCall {
            name: "specline_teleport",
            arguments: &json!({}),
            client: None,
        },
    )
    .unwrap_err();
    // A bad argument, not a missing method — so 400, not the 404 that means
    // "there is no MCP server at this address".
    assert_eq!(unknown.code, specline_mcp::protocol::codes::INVALID_PARAMS);
    assert_eq!(unknown.http_status(), 400);
    assert!(
        unknown.message.contains("specline_context"),
        "the error should list what does exist: {}",
        unknown.message
    );
}

#[test]
fn the_tool_count_is_what_the_documentation_claims() {
    // "Nine" was written in five places after the tenth tool landed, and "ten"
    // in as many after the thirteenth. A number in prose drifts; a number in an
    // assertion does not.
    assert_eq!(
        specline_mcp::tools::all().len(),
        13,
        "thirteen is the ceiling and the count — if this changes, every place \
         that states it has to change with it"
    );
}

// --- The rest of the surface ---------------------------------------------
//
// Four of the thirteen tools had a response snapshot. The other nine are the
// ones a model spends most of its time in — reading an artifact, moving a task,
// writing prose — and a renamed key on any of them could ship without anything
// saying so. The count is asserted below, so a tenth gap cannot open quietly.

#[test]
fn get_response_shape() {
    let mut seed = seeded();
    let task = seed.task.to_string();
    let result = call(
        &mut seed.store,
        "specline_get",
        json!({"ids": [task], "depth": 1}),
    );
    settings().bind(|| insta::assert_json_snapshot!("specline_get", result));
}

#[test]
fn update_response_shape() {
    let mut seed = seeded();
    let task = seed.task.to_string();
    let result = call(
        &mut seed.store,
        "specline_update",
        json!({
            "id": task,
            "version": 1,
            "changes": {"priority": "p1"},
            "session_id": "ses_snapshot"
        }),
    );
    settings().bind(|| insta::assert_json_snapshot!("specline_update", result));
}

#[test]
fn write_doc_response_shape() {
    let mut seed = seeded();
    let spec = seed.spec.to_string();
    let result = call(
        &mut seed.store,
        "specline_write_doc",
        json!({
            "id": spec,
            "body": "## Metering\n\nUsage is counted hourly.\n",
            "session_id": "ses_snapshot"
        }),
    );
    settings().bind(|| insta::assert_json_snapshot!("specline_write_doc", result));
}

#[test]
fn link_response_shape() {
    let mut seed = seeded();
    let (task, spec) = (seed.task.to_string(), seed.spec.to_string());
    let result = call(
        &mut seed.store,
        "specline_link",
        json!({
            "from": task,
            "to": spec,
            "rel": "implements",
            "session_id": "ses_snapshot"
        }),
    );
    settings().bind(|| insta::assert_json_snapshot!("specline_link", result));
}

#[test]
fn note_response_shape() {
    let mut seed = seeded();
    let task = seed.task.to_string();
    let result = call(
        &mut seed.store,
        "specline_note",
        json!({
            "id": task,
            "body": "The duplicate rows all came from one retrying client.",
            "session_id": "ses_snapshot"
        }),
    );
    settings().bind(|| insta::assert_json_snapshot!("specline_note", result));
}

#[test]
fn activity_response_shape() {
    let mut seed = seeded();
    let result = call(
        &mut seed.store,
        "specline_activity",
        json!({"project": "harbour"}),
    );
    settings().bind(|| insta::assert_json_snapshot!("specline_activity", result));
}

#[test]
fn ready_response_shape() {
    let mut seed = seeded();
    let result = call(
        &mut seed.store,
        "specline_next",
        json!({"project": "harbour"}),
    );
    settings().bind(|| insta::assert_json_snapshot!("specline_next", result));
}

#[test]
fn claim_response_shape() {
    let mut seed = seeded();
    let task = seed.task.to_string();
    let result = call(
        &mut seed.store,
        "specline_claim",
        json!({"id": task, "session_id": "ses_snapshot"}),
    );
    settings().bind(|| insta::assert_json_snapshot!("specline_claim", result));
}

#[test]
fn close_response_shape() {
    let mut seed = seeded();
    let task = seed.task.to_string();
    let result = call(
        &mut seed.store,
        "specline_close",
        json!({
            "id": task,
            "reason": "done",
            "message": "Deduped on the idempotency key at write time.",
            "evidence": ["commit:0000000"],
            "session_id": "ses_snapshot"
        }),
    );
    settings().bind(|| insta::assert_json_snapshot!("specline_close", result));
}

/// Every advertised tool has a response snapshot.
///
/// The gap this closes is not a missing assertion, it is a missing *habit*.
/// Four tools had snapshots and nine did not, and nothing in the suite could
/// tell the difference — so the fourteenth tool would have arrived without one
/// too, and the surface would have kept drifting in the parts nobody had
/// pinned. This reads the directory rather than a list, because a list is one
/// more thing to forget to add to.
#[test]
fn every_tool_has_a_response_snapshot() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots");

    let missing: Vec<&str> = specline_mcp::tools::all()
        .iter()
        .map(|t| t.name)
        .filter(|name| !dir.join(format!("snapshots__{name}.snap")).is_file())
        .collect();

    assert!(
        missing.is_empty(),
        "these tools are advertised but have no response snapshot: {missing:?}\n\
         Add a test that calls the tool against `seeded()` and snapshots the result as \
         `specline_<name>`. A tool whose response shape nothing has pinned can be renamed in a \
         way no diff shows."
    );
}
