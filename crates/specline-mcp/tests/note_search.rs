//! `specline_search` finds what `specline_note` wrote (KEEL-339).
//!
//! The store-level tests in `specline-core` hold the index to its contract;
//! this holds the tool to it, end to end through the two tools a session
//! actually uses, because "record what you found as a note" is advice given to
//! a model and the model only ever sees this surface. The daemon's
//! `/api/search` dispatches to the same tool, so this covers it as well.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use specline_core::{Actor, EntityStore, Project, Provenance, Store, Task};
use specline_mcp::{ToolCall, dispatch};

fn call(store: &mut Store, name: &str, arguments: Value) -> Value {
    dispatch(
        store,
        ToolCall {
            name,
            arguments: &arguments,
            client: None,
        },
    )
    .unwrap_or_else(|e| panic!("{name} failed: {}", e.message))
}

#[test]
fn a_note_written_through_the_tool_is_found_by_the_search_tool_until_retracted() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let prov = Provenance::anonymous(Actor::Claude);
    let project = store
        .create(Project::new("specline", "Specline").into(), &prov)
        .unwrap()
        .entity
        .id()
        .clone();
    let task = store
        .create(
            Task::new(project, "The board is slow", "A row this test needs.").into(),
            &prov,
        )
        .unwrap()
        .entity
        .id()
        .clone();

    let noted = call(
        &mut store,
        "specline_note",
        json!({ "id": task.as_str(), "body": "the wallaby index was never rebuilt" }),
    );
    let note_id = noted
        .pointer("/structuredContent/note/id")
        .and_then(Value::as_str)
        .unwrap()
        .to_owned();

    let found = call(&mut store, "specline_search", json!({ "query": "wallaby" }));
    let hits = found
        .pointer("/structuredContent/hits")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(hits.len(), 1, "{found:#}");
    assert_eq!(
        hits[0]["entity_id"],
        task.as_str(),
        "the hit names the task"
    );
    assert_eq!(hits[0]["entity_type"], "task");
    assert_eq!(hits[0]["note_id"], note_id, "and the note that matched");
    let text = found
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap();
    assert!(
        text.contains(&format!("matched in note {note_id}")),
        "the text a model reads says where the match was: {text}"
    );

    call(&mut store, "specline_note", json!({ "retract": note_id }));
    let gone = call(&mut store, "specline_search", json!({ "query": "wallaby" }));
    assert_eq!(
        gone.pointer("/structuredContent/total"),
        Some(&json!(0)),
        "a retracted note stops matching: {gone:#}"
    );
}
