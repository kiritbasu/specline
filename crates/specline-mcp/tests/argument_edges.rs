//! Arguments that are the right shape and the wrong value.
//!
//! Each of these used to be accepted and then produce a plausible result: a
//! version that wrapped past the concurrency check, a rank placed outside the
//! range the caller named, a `duplicates` edge between two things that cannot
//! stand in that relation. None errored, which is what makes them worth a file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use specline_core::{Actor, EntityId, EntityStore, Project, Provenance, Spec, Store, Task};
use specline_mcp::{ToolCall, dispatch};

struct Fixture {
    store: Store,
    _dir: tempfile::TempDir,
    spec: EntityId,
    first: EntityId,
    second: EntityId,
    third: EntityId,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let prov = Provenance::anonymous(Actor::Claude);
    let project = store
        .create(Project::new("edges", "Edges").into(), &prov)
        .unwrap()
        .entity
        .id()
        .clone();
    let spec = store
        .create(Spec::new(project.clone(), "A spec").into(), &prov)
        .unwrap()
        .entity
        .id()
        .clone();

    let mut ids = Vec::new();
    for name in ["first", "second", "third"] {
        ids.push(
            store
                .create(
                    Task::new(project.clone(), name, "A row for an argument test.").into(),
                    &prov,
                )
                .unwrap()
                .entity
                .id()
                .clone(),
        );
    }

    Fixture {
        store,
        _dir: dir,
        spec,
        first: ids[0].clone(),
        second: ids[1].clone(),
        third: ids[2].clone(),
    }
}

fn call(store: &mut Store, name: &str, arguments: Value) -> Result<Value, String> {
    dispatch(
        store,
        ToolCall {
            name,
            arguments: &arguments,
            client: None,
        },
    )
    .map_err(|e| e.message)
}

/// A version too large for an `i32` is refused, not truncated into a real one.
///
/// `as i32` wraps. 4294967297 became 1, and 1 is a version artifacts actually
/// have — so the stale-write check that `version` exists for compared against
/// it and passed. The single guard against a lost update, defeated by a number
/// too big to be a version at all.
#[test]
fn a_version_that_cannot_be_a_version_is_refused_rather_than_wrapped() {
    let mut f = fixture();
    let id = f.first.to_string();

    let error = call(
        &mut f.store,
        "specline_update",
        json!({"id": id, "version": 4_294_967_297i64, "changes": {"priority": "p1"}}),
    )
    .expect_err("a version outside i32 must not be narrowed into a plausible one");
    assert!(error.contains("version"), "{error}");

    // The row is untouched, which is the part that matters: had it wrapped to
    // 1 the update would have succeeded against a stale read.
    let after = call(&mut f.store, "specline_get", json!({"ids": [id]})).unwrap();
    assert_eq!(
        after.pointer("/structuredContent/artifacts/0/entity/priority"),
        Some(&json!("p2")),
        "the update must not have landed"
    );
}

/// Placing a task "after X and before Y" when X already sits below Y is
/// refused.
///
/// `rank_between` takes the midpoint, so the pair in the wrong order produced a
/// number outside the range the caller named — a move that succeeds and lands
/// somewhere neither anchor implies, which is worse than one that is refused.
#[test]
fn a_rank_between_two_anchors_in_the_wrong_order_is_refused() {
    let mut f = fixture();
    let (second, third, moving) = (
        f.second.to_string(),
        f.third.to_string(),
        f.first.to_string(),
    );

    // `second` was created before `third`, so it ranks below it. Asking to go
    // after `third` and before `second` names an empty interval.
    let error = call(
        &mut f.store,
        "specline_update",
        json!({
            "id": moving,
            "version": 1,
            "changes": {"rank_after": third, "rank_before": second}
        }),
    )
    .expect_err("an interval with nothing in it should be refused");
    assert!(error.contains("no room"), "{error}");

    // And the right way round still works, which is the half that would break
    // silently if the check were too eager.
    call(
        &mut f.store,
        "specline_update",
        json!({
            "id": moving,
            "version": 1,
            "changes": {"rank_after": second, "rank_before": third}
        }),
    )
    .expect("the same two anchors in the order they actually sit in is a real placement");
}

/// An update naming neither rank anchor is not a rank change.
///
/// The arm that handles it used to be an `unreachable!` guarded by a condition
/// thirty lines above — a panic in library code whose safety depended on
/// something that could be edited independently of it.
#[test]
fn an_update_with_no_rank_arguments_is_an_ordinary_update() {
    let mut f = fixture();
    let id = f.first.to_string();

    let result = call(
        &mut f.store,
        "specline_update",
        json!({"id": id, "version": 1, "changes": {"priority": "p1"}}),
    )
    .expect("an update that says nothing about rank is just an update");
    assert_eq!(
        result.pointer("/structuredContent/entity/priority"),
        Some(&json!("p1"))
    );
}

/// A task cannot be closed as a duplicate of something that is not a task.
///
/// `duplicate` and `superseded` draw an edge to whatever `other` names, and
/// nothing checked it was a task — so closing a task as a duplicate of a *spec*
/// succeeded, leaving a `duplicates` edge between two things that cannot stand
/// in that relation and a close message that reads perfectly well.
#[test]
fn closing_a_task_as_a_duplicate_of_a_spec_is_refused() {
    let mut f = fixture();
    let (task, spec) = (f.first.to_string(), f.spec.to_string());

    let error = call(
        &mut f.store,
        "specline_close",
        json!({
            "id": task,
            "reason": "duplicate",
            "message": "Same work as the spec, apparently.",
            "other": spec
        }),
    )
    .expect_err("only a task can be the other end of a duplicates edge");
    assert!(error.contains("not a task"), "{error}");

    // A real duplicate still closes.
    call(
        &mut f.store,
        "specline_close",
        json!({
            "id": task,
            "reason": "duplicate",
            "message": "Same work as the second row.",
            "other": f.second.to_string()
        }),
    )
    .expect("a task duplicating a task is the case this exists for");
}

/// KEEL-172. `fields` is documented as "any other column on the type", and for
/// a metric observation's three columns that was false: the constructor read
/// them from the top level and the schema declared none of them, so a caller
/// following the tool's own description was refused. Recording a measurement
/// was the one write the surface could not do, which is a fair explanation for
/// why the metrics page went stale.
#[test]
fn a_measurement_can_be_recorded_the_way_the_schema_says() {
    let mut f = fixture();
    let metric = call(
        &mut f.store,
        "specline_create",
        json!({"type": "metric", "project": "edges", "title": "Unprompted writes"}),
    )
    .unwrap();
    let metric_id = metric
        .pointer("/structuredContent/entity/id")
        .and_then(Value::as_str)
        .unwrap()
        .to_owned();

    let recorded = call(
        &mut f.store,
        "specline_create",
        json!({
            "type": "metric_observation",
            "project": "edges",
            "fields": {
                "metric_id": metric_id,
                "value": 0.62,
                "observed_at": "2026-08-15T09:00:00Z",
            },
        }),
    )
    .expect("`fields` has to take the three columns it says it takes");

    let entity = recorded.pointer("/structuredContent/entity").unwrap();
    assert_eq!(entity.get("metric_id"), Some(&json!(metric_id)));
    assert_eq!(entity.get("value"), Some(&json!(0.62)));
    assert!(
        entity
            .get("observed_at")
            .and_then(Value::as_str)
            .is_some_and(|t| t.starts_with("2026-08-15T09:00:00")),
        "the supplied time is kept rather than replaced with now: {entity}"
    );
}

/// And the spelling that already worked keeps working. A fix that moved the
/// arguments rather than widening where they are looked for would have been the
/// same bug pointing the other way — the CLI and every existing caller send
/// them at the top level.
#[test]
fn a_measurement_can_still_be_recorded_from_the_top_level() {
    let mut f = fixture();
    let metric = call(
        &mut f.store,
        "specline_create",
        json!({"type": "metric", "project": "edges", "title": "Digest size"}),
    )
    .unwrap();
    let metric_id = metric
        .pointer("/structuredContent/entity/id")
        .and_then(Value::as_str)
        .unwrap()
        .to_owned();

    let recorded = call(
        &mut f.store,
        "specline_create",
        json!({
            "type": "metric_observation",
            "project": "edges",
            "metric_id": metric_id,
            "value": 3400.0,
        }),
    )
    .expect("the top-level form is what the CLI sends");
    assert_eq!(
        recorded.pointer("/structuredContent/entity/value"),
        Some(&json!(3400.0))
    );
}

/// The failure case, and the one a model actually needs: no metric named at
/// all. The refusal has to say where to find one, because "missing or not a
/// string" leaves a caller guessing at a ULID it has never seen.
#[test]
fn recording_a_measurement_against_no_metric_says_how_to_find_one() {
    let mut f = fixture();
    let error = call(
        &mut f.store,
        "specline_create",
        json!({"type": "metric_observation", "project": "edges", "fields": {"value": 1.0}}),
    )
    .expect_err("an observation of nothing is not an observation");
    assert!(error.contains("metric_id"), "{error}");
    assert!(
        error.contains("specline_search"),
        "the refusal should say how to find the metric: {error}"
    );
}

/// Feedback is the one type whose column is `summary` rather than `title`, and
/// §3.2 says why: what somebody said has no name, so titling it would mean
/// inventing one. The create path nonetheless required `title` and refused
/// `summary` — the field the table actually has — so a caller who had read the
/// schema was told "feedback needs a name" about a column that does not exist.
#[test]
fn a_signal_is_created_from_the_summary_the_table_actually_has() {
    let mut f = fixture();
    let created = call(
        &mut f.store,
        "specline_create",
        json!({
            "type": "feedback",
            "project": "edges",
            "summary": "Specline should work with OpenAI Codex, not only Claude Code",
            "fields": {"kind": "idea", "source": "Madhu"},
        }),
    )
    .expect("`summary` is the column feedback has");

    let entity = created.pointer("/structuredContent/entity").unwrap();
    assert_eq!(
        entity.get("summary"),
        Some(&json!(
            "Specline should work with OpenAI Codex, not only Claude Code"
        ))
    );
    assert_eq!(entity.get("kind"), Some(&json!("idea")));
    assert_eq!(entity.get("source"), Some(&json!("Madhu")));
    assert_eq!(
        entity.get("triaged"),
        Some(&json!(false)),
        "a new signal is untriaged, which is what puts it in the Inbox"
    );
}

/// And the spelling that already worked keeps working, for the same reason the
/// metric fix kept both: moving the argument rather than widening where it is
/// looked for would be the same bug pointing the other way.
#[test]
fn a_signal_can_still_be_created_from_a_title() {
    let mut f = fixture();
    let created = call(
        &mut f.store,
        "specline_create",
        json!({"type": "feedback", "project": "edges", "title": "Onboarding felt slow"}),
    )
    .expect("the older spelling is what existing callers send");
    assert_eq!(
        created.pointer("/structuredContent/entity/summary"),
        Some(&json!("Onboarding felt slow"))
    );
}

/// The failure case. A signal with neither has nothing in it, and the refusal
/// has to name `summary` — pointing a caller at `title` would send them to a
/// column the table does not have, which is how this went wrong in the first
/// place.
#[test]
fn a_signal_with_nothing_said_in_it_is_refused_by_the_right_name() {
    let mut f = fixture();
    let error = call(
        &mut f.store,
        "specline_create",
        json!({"type": "feedback", "project": "edges", "fields": {"kind": "idea"}}),
    )
    .expect_err("a signal is what somebody said, so there has to be something said");
    assert!(
        error.contains("summary"),
        "the refusal should name the column that exists: {error}"
    );
    assert!(
        !error.contains("needs a name"),
        "feedback has no name to need: {error}"
    );
}

/// An epic and its children, over the surface a session actually uses.
///
/// `parent_id` has existed since Phase 0 and nothing ever wrote to it, so this
/// is the first time the argument has been sent. It arrives through `fields`
/// like any other column, which is worth a test precisely because it is *not*
/// a declared property of the schema — a caller has to know the column exists.
#[test]
fn an_epic_takes_children_through_fields() {
    let mut f = fixture();
    let epic = call(
        &mut f.store,
        "specline_create",
        json!({
            "type": "task",
            "project": "edges",
            "title": "Codex support",
            "summary": "One decided feature, holding the tasks that build it.",
            "fields": {"kind": "feature"},
        }),
    )
    .expect("a feature task is an epic");
    let epic_id = epic
        .pointer("/structuredContent/entity/id")
        .and_then(Value::as_str)
        .unwrap()
        .to_owned();

    let child = call(
        &mut f.store,
        "specline_create",
        json!({
            "type": "task",
            "project": "edges",
            "title": "Reach the endpoint from Codex",
            "summary": "Prove the MCP endpoint answers a non-Claude client.",
            "fields": {"parent_id": epic_id},
        }),
    )
    .expect("a child names its epic");
    assert_eq!(
        child.pointer("/structuredContent/entity/parent_id"),
        Some(&json!(epic_id))
    );
}

/// A parent that is not a task at all — the refusal a caller reaching for
/// `parent_id` is most likely to hit, since a spec id looks as much like a
/// container as a task id does.
#[test]
fn a_parent_that_is_not_a_task_is_refused_by_name() {
    let mut f = fixture();
    let error = call(
        &mut f.store,
        "specline_create",
        json!({
            "type": "task",
            "project": "edges",
            "title": "A task under a spec",
            "summary": "Which is what `implements` is for, not `parent_id`.",
            "fields": {"parent_id": f.spec.to_string()},
        }),
    )
    .expect_err("a parent is a task");

    assert!(error.contains("parent_id"), "{error}");
    assert!(
        error.contains("not a milestone or a spec"),
        "and it says what a parent has to be: {error}"
    );
}
