//! Executing a tool call against `specline-core`.
//!
//! This is the only place that knows both the MCP argument shapes and the
//! domain API. `specline-core` stays ignorant of MCP, and the daemon stays
//! ignorant of what the tools mean — it owns the socket and the lock, nothing
//! more.
//!
//! # Errors are part of the surface
//!
//! A validation error here is read by a model that has to work out what to
//! send instead. `specline-core` already builds those messages carefully; this
//! layer's job is to not flatten them into "bad request", and to map a stale
//! update onto the 409 payload from SPEC §7.3 — current state plus the events
//! since the caller's read — so an agent can usually merge rather than give up.

use crate::protocol::{RpcError, codes};
use serde_json::{Map, Value, json};
use specline_core::{
    Actor, Cursor, Direction, Entity, EntityId, EntityQuery, EntityStore, EntityType, Error,
    EventId, GraphStore, NewLink, NewNote, Provenance, Relation, SearchQuery, Surface,
};
use specline_core::{
    Artifact, Decision, Design, Environment, Feedback, Metric, MetricObservation, Milestone,
    Project, Question, Spec, Store, Task, Term,
};

/// One tool invocation.
pub struct ToolCall<'a> {
    /// The tool name.
    pub name: &'a str,
    /// The `arguments` object.
    pub arguments: &'a Value,
}

/// Turn a domain error into a JSON-RPC error, preserving what the caller needs.
pub fn to_rpc_error(store: &Store, err: Error) -> RpcError {
    match &err {
        Error::StaleVersion { id, latest, .. } => {
            // SPEC §7.3. Returning the current state and the events since the
            // caller's read is what lets an agent resolve the conflict itself
            // rather than clobbering or giving up.
            let current_state = EntityId::parse(id)
                .ok()
                .and_then(|i| store.get(&i).ok().flatten())
                .map(|e| entity_json(&e));

            // Newest first, scoped to this row, and asked of the index rather
            // than filtered in Rust.
            //
            // This read used to take the oldest 500 events in the whole store
            // and filter them down. The live store passed 500 long ago, so an
            // agent handed a 409 was shown history from before the conflict and
            // nothing near it — which is worse than showing none, because it
            // looks like an answer.
            let events_since = EntityId::parse(id)
                .ok()
                .and_then(|i| {
                    store
                        .recent_events(specline_core::store::EventScope::Entity(&i), 20)
                        .ok()
                })
                .and_then(|page| serde_json::to_value(page.items).ok());

            RpcError::new(codes::CONFLICT, err.to_string()).with_data(json!({
                "latest_version": latest,
                "current_state": current_state,
                "events_since": events_since,
            }))
        }
        e if e.is_caller_error() => RpcError::new(codes::INVALID_PARAMS, err.to_string()),
        // The chain, not just the context. An agent reading "count matching
        // rows" cannot act; one reading the engine error underneath it can,
        // and so can whoever is debugging at 3am.
        _ => RpcError::new(codes::INTERNAL_ERROR, err.chain()),
    }
}

/// What a caller may write where an id is expected.
const ID_OR_REF: &str = "a prefixed ULID such as `tsk_01H8…`, or a readable reference such as \
     `KEEL-42` for a task or `KEEL-B12` for a decision";

/// Turn what the caller wrote into an id, accepting `KEEL-42` and `KEEL-B12`
/// as well as a ULID.
///
/// `Ok(None)` means it was a well-formed reference that names nothing, which is
/// a different answer from "that is not a reference at all" — the first is a
/// task that does not exist, the second is a typo in the shape of the argument,
/// and a model correcting itself needs to know which.
fn resolve_optional(store: &Store, field: &str, raw: &str) -> Result<Option<EntityId>, RpcError> {
    if EntityId::parse(raw).is_err()
        && specline_core::types::parse_readable_ref(raw).is_none()
        && specline_core::types::parse_decision_ref(raw).is_none()
    {
        return Err(bad_arg(
            field,
            &format!("`{raw}` is neither an id nor a readable reference"),
            ID_OR_REF,
        ));
    }
    store.resolve_ref(raw).map_err(|e| to_rpc_error(store, e))
}

/// As [`resolve_optional`], for the callers where the target has to exist.
fn resolve_required(store: &Store, field: &str, raw: &str) -> Result<EntityId, RpcError> {
    resolve_optional(store, field, raw)?.ok_or_else(|| {
        bad_arg(
            field,
            &format!("`{raw}` does not name anything in this store"),
            ID_OR_REF,
        )
    })
}

/// The readable label for an entity, if it has one: `KEEL-42`.
///
/// Only tasks have one. Returns `None` rather than inventing something for the
/// other twelve types, because a made-up reference that does not resolve is
/// worse than no reference at all.
fn readable_ref(store: &Store, entity: &Entity) -> Option<String> {
    let Entity::Task(task) = entity else {
        return None;
    };
    match store.get(&task.project_id) {
        Ok(Some(Entity::Project(project))) => Some(format!("{}-{}", project.key, task.number)),
        _ => None,
    }
}

/// Where this artifact lives in the interface, if it has a screen and there is
/// an interface running.
///
/// `None` for the types with no screen of their own — a milestone, a term, an
/// environment, a metric. A link that opens the app's fallback and shows
/// something unrelated is worse than no link, because it reads as the interface
/// being broken rather than as the thing having no page.
fn interface_url(store: &Store, entity: &Entity) -> Option<String> {
    let base = crate::links::interface()?;

    // A project is its own screen and has no project of its own to look up.
    if let Entity::Project(project) = entity {
        return Some(crate::links::project(base, &project.slug));
    }

    let Ok(Some(Entity::Project(project))) = store.get(entity.project_id()?) else {
        return None;
    };

    match entity {
        Entity::Task(task) => Some(crate::links::task(
            base,
            &project.slug,
            &format!("{}-{}", project.key, task.number),
        )),
        Entity::Spec(_)
        | Entity::Decision(_)
        | Entity::Question(_)
        | Entity::Feedback(_)
        | Entity::Design(_) => Some(crate::links::document(
            base,
            &project.slug,
            entity.id().as_ref(),
        )),
        _ => None,
    }
}

/// [`entity_json`] with the interface link attached, for the responses that
/// name one artifact.
///
/// Separate from `entity_json` rather than folded into it, because that
/// function is the wire shape for every surface including bulk listings, and a
/// URL on each of several hundred rows is tokens spent on links nobody will
/// click. This is for the results a reply actually quotes back.
fn entity_json_linked(store: &Store, entity: &Entity) -> Value {
    let mut value = entity_json(entity);
    if let (Some(obj), Some(url)) = (value.as_object_mut(), interface_url(store, entity)) {
        obj.insert("url".to_owned(), json!(url));
    }
    value
}

/// A missing or malformed argument.
fn bad_arg(field: &str, problem: &str, expected: &str) -> RpcError {
    RpcError::new(
        codes::INVALID_PARAMS,
        format!("argument `{field}`: {problem}. Expected: {expected}"),
    )
}

/// Read a required string argument.
fn req_str(args: &Value, field: &str) -> Result<String, RpcError> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| bad_arg(field, "missing or not a string", "a string"))
}

/// Read an optional string argument.
fn opt_str(args: &Value, field: &str) -> Option<String> {
    args.get(field).and_then(Value::as_str).map(str::to_owned)
}

/// Read an optional integer argument.
fn opt_i64(args: &Value, field: &str) -> Option<i64> {
    args.get(field).and_then(Value::as_i64)
}

/// Read an optional boolean argument.
fn opt_bool(args: &Value, field: &str) -> bool {
    args.get(field).and_then(Value::as_bool).unwrap_or(false)
}

/// Read an optional timestamp argument.
fn opt_time(args: &Value, field: &str) -> Result<Option<chrono::DateTime<chrono::Utc>>, RpcError> {
    parse_time(args.get(field).and_then(Value::as_str), field)
}

/// Parse a timestamp that has already been found, wherever it was found.
///
/// Split from [`opt_time`] so a caller that looked somewhere other than the top
/// level gets the same error text. Two spellings of "that is not a timestamp"
/// is two chances for one of them to be less useful than the other.
fn parse_time(
    raw: Option<&str>,
    field: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, RpcError> {
    match raw {
        None => Ok(None),
        Some(raw) => chrono::DateTime::parse_from_rfc3339(raw)
            .map(|t| Some(t.with_timezone(&chrono::Utc)))
            .map_err(|_| {
                bad_arg(
                    field,
                    &format!("`{raw}` is not a timestamp"),
                    "an RFC 3339 timestamp such as 2026-08-09T14:22:01Z",
                )
            }),
    }
}

/// Read an argument from the top level, or from `fields` if it is there
/// instead.
///
/// `specline_create` documents `fields` as "any other column on the type", and for
/// three columns that was not true: a metric observation's `metric_id`, `value`
/// and `observed_at` are read by the constructor from the top level, and are
/// not in the published schema. A caller following the tool's own description
/// put them in `fields` and got "argument `metric_id`: missing or not a string"
/// — so recording a measurement was the one write the surface could not
/// actually do, and the metrics page went stale for want of it (KEEL-172).
///
/// Looking in both places rather than moving the read is deliberate. The
/// top-level form is what the CLI and the existing tests send, and a fix that
/// swapped one undiscoverable spelling for another would have been the same bug
/// pointing the other way.
fn arg_or_field<'a>(args: &'a Value, field: &str) -> Option<&'a Value> {
    args.get(field)
        .or_else(|| args.get("fields").and_then(|f| f.get(field)))
}

/// The arguments a type's constructor takes for itself, which must not then be
/// re-applied as if they were ordinary columns.
///
/// Only metric observations have any. `metric_id` is immutable — an observation
/// belongs to the metric it was recorded against — so handing it to
/// `apply_changes` after the constructor already consumed it turns a working
/// call into "the metric_id is set at creation", which is true and unhelpful.
fn consumed_by_constructor(entity_type: EntityType) -> &'static [&'static str] {
    match entity_type {
        EntityType::MetricObservation => &["metric_id", "value", "observed_at"],
        _ => &[],
    }
}

/// Build provenance from the ambient arguments.
///
/// The default actor is `claude`: this is the MCP transport, and SPEC §6.5
/// says to fall back to the transport's identity rather than refusing a write.
/// Losing attribution is bad; refusing the write is worse.
pub fn provenance_from(args: &Value) -> Result<Provenance, RpcError> {
    let surface = match opt_str(args, "surface") {
        None => None,
        Some(s) => Some(
            Surface::parse(&s).map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))?,
        ),
    };
    Ok(Provenance {
        actor: Actor::Claude,
        session_id: opt_str(args, "session_id"),
        surface,
    })
}

/// Resolve a project reference — id, slug or name — to an id.
pub fn resolve_project(store: &Store, reference: &str) -> Result<EntityId, RpcError> {
    if let Ok(id) = EntityId::parse_as(reference, EntityType::Project)
        && store.get(&id).ok().flatten().is_some()
    {
        return Ok(id);
    }

    let candidates = store
        .list(&EntityQuery::default().of_type(EntityType::Project))
        .map_err(|e| to_rpc_error(store, e))?;

    let needle = reference.to_lowercase();
    let matched = candidates.items.iter().find(|p| match p {
        Entity::Project(pr) => {
            pr.slug.eq_ignore_ascii_case(reference)
                || pr.name.to_lowercase() == needle
                || pr.aliases.iter().any(|a| a.to_lowercase() == needle)
        }
        _ => false,
    });

    match matched {
        Some(p) => Ok(p.id().clone()),
        None => Err(bad_arg(
            "project",
            &format!("no project matches `{reference}`"),
            &format!(
                "one of: {}. Call specline_projects to list them",
                candidates
                    .items
                    .iter()
                    .filter_map(|p| match p {
                        Entity::Project(pr) => Some(pr.slug.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// Serialise an entity for the wire, lifting the concurrency version to the
/// top level.
///
/// Public because *every* surface must use it. `/api/entities` once serialised
/// entities directly and `/api/entity/{id}` went through here, so the same
/// field appeared in two shapes depending on which endpoint you asked — which
/// is the kind of inconsistency a caller discovers at the worst moment.
///
/// `version` lives inside the audit block in the domain model, which is right
/// there and wrong here: `specline_update` documents a `version` argument, and an
/// agent that has just read an entity should be able to copy the field of that
/// name straight across. Making it hunt inside `audit` for it is the kind of
/// papercut that turns into a 409 and a confused retry.
///
/// The audit block is left intact — this adds a field, it does not move one.
pub fn entity_json(entity: &Entity) -> Value {
    let mut value = serde_json::to_value(entity).unwrap_or(Value::Null);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("version".to_owned(), json!(entity.audit().version));
        obj.insert("archived".to_owned(), json!(entity.audit().is_archived()));
    }
    value
}

/// Wrap a structured result in the `tools/call` content shape.
///
/// Both halves are populated on purpose: `structuredContent` for a client that
/// can use it, and a text rendering for one that cannot — and for the model,
/// which reads the text.
fn tool_result(summary: String, structured: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": structured,
        "isError": false,
    })
}

/// The structured half of a tool result.
///
/// A tool returns `{content, structuredContent, isError}` — the text for a
/// model to read and the data for a caller to use. Everything that consumes a
/// result wants the second, and there are three such places: the daemon's REST
/// surface, the CLI's daemon path, and the CLI's fallback when no daemon is
/// listening.
///
/// The third one did not do it. `specline ready` with no daemon running looked for
/// `ready` at the top level, found the envelope instead, and printed "nothing
/// ready" against a store with sixteen ready tasks in it. `claim` and `close`
/// printed a bare id where a title belonged. All three were silent, plausible
/// and wrong, which is why this is a named function rather than two lines
/// repeated at each site.
///
/// A value that is already unwrapped is returned as-is, so calling this twice
/// is safe.
pub fn payload(result: &Value) -> Value {
    result
        .get("structuredContent")
        .cloned()
        .unwrap_or_else(|| result.clone())
}

/// Execute a tool call.
pub fn dispatch(store: &mut Store, call: ToolCall<'_>) -> Result<Value, RpcError> {
    dispatch_prepared(store, call, None)
}

/// Execute a tool call with the search query already embedded.
///
/// The one thing on the read path that costs real time is turning a query into
/// a vector, and a caller that holds the store behind a lock wants that done
/// before it takes the lock. Only `specline_search` looks at `query_vector`;
/// passing one for anything else is harmless and ignored.
///
/// [`dispatch`] is this with `None`, for every caller that has no lock to be
/// careful with.
pub fn dispatch_prepared(
    store: &mut Store,
    call: ToolCall<'_>,
    query_vector: Option<Vec<f32>>,
) -> Result<Value, RpcError> {
    let args = call.arguments;
    match call.name {
        "specline_context" => specline_context(store, args),
        "specline_search" => specline_search(store, args, query_vector),
        "specline_get" => specline_get(store, args),
        "specline_projects" => specline_projects(store, args),
        "specline_activity" => specline_activity(store, args),
        "specline_create" => specline_create(store, args),
        "specline_update" => specline_update(store, args),
        "specline_write_doc" => specline_write_doc(store, args),
        "specline_link" => specline_link(store, args),
        "specline_note" => specline_note(store, args),
        "specline_next" => specline_next(store, args),
        "specline_claim" => specline_claim(store, args),
        "specline_close" => specline_close(store, args),
        // INVALID_PARAMS, not METHOD_NOT_FOUND. The JSON-RPC *method* here is
        // `tools/call` and it exists; the tool name is one of its arguments.
        // The distinction is not pedantry: METHOD_NOT_FOUND is served as HTTP
        // 404, and the specification makes 404 on the MCP endpoint mean "there
        // is no server at this address" — so a mistyped tool name looked
        // exactly like the server having vanished.
        other => Err(RpcError::new(
            codes::INVALID_PARAMS,
            format!(
                "no tool named `{other}`. Available: {}",
                crate::tools::all()
                    .iter()
                    .map(|t| t.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn specline_context(store: &Store, args: &Value) -> Result<Value, RpcError> {
    // `cwd` resolves to a project by its recorded `root_path`, and — more
    // importantly — says plainly when nothing matches. TQ-17: nine of ten gate
    // sessions called this, saw a roll-up listing some *other* project, and
    // had to work out for themselves that the thing they were working on was
    // simply absent. One of them said so outright and wrote nothing.
    let cwd = opt_str(args, "cwd");
    let matched_by_cwd = cwd.as_deref().and_then(|d| project_for_directory(store, d));

    let project = match opt_str(args, "project") {
        Some(p) => Some(resolve_project(store, &p)?),
        None => matched_by_cwd.clone(),
    };
    let depth = specline_core::digest::Depth::parse(
        opt_str(args, "depth").as_deref().unwrap_or("standard"),
    )
    .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e))?;
    let since = opt_time(args, "since")?;

    let digest = specline_core::digest::build(store, project.as_ref(), depth, since, surfaces())
        .map_err(|e| to_rpc_error(store, e))?;

    let unmatched = cwd.as_deref().filter(|_| matched_by_cwd.is_none());

    let mut summary = digest.to_prose();
    if let Some(dir) = unmatched {
        // Stated before the digest, not after: an agent that reads "here is
        // project X" first has already decided what it is looking at.
        summary = format!(
            "**No project in Specline matches `{dir}`.** If you are working on something \
             that belongs here, create it — `specline_create(type: \"project\", title: …, \
             fields: {{\"root_path\": \"{dir}\"}})` — and say that you did. Creating the \
             *first* project for a directory is not the duplicate-project failure; \
             creating a second one for a project that already exists is.\n\n{summary}"
        );
    }

    let mut structured = serde_json::to_value(&digest)
        .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, e.to_string()))?;

    // Echo the session back so a long conversation can self-check that it is
    // still threading correctly (SPEC §6.5).
    if let Some(obj) = structured.as_object_mut() {
        obj.insert(
            "session_id".to_owned(),
            opt_str(args, "session_id")
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        if let Some(dir) = cwd.as_deref() {
            obj.insert(
                "directory".to_owned(),
                json!({
                    "path": dir,
                    "matched_project": matched_by_cwd.as_ref().map(|p| p.to_string()),
                }),
            );
        }
    }
    Ok(tool_result(summary, structured))
}

/// Normalise a filesystem path for comparison.
///
/// Collapses repeated separators and drops a trailing one. Found by a gate
/// session, which reported `matched_project: null` for a directory that plainly
/// had a project: the caller's `cwd` contained `T//specline-gate` and the stored
/// `root_path` contained `T/specline-gate`. A naive prefix comparison called those
/// different directories, so the session started unoriented and said so — and
/// the only reason anyone noticed is that it mentioned the null in its reply.
///
/// Not canonicalisation: this must not touch the filesystem. `cwd` may name a
/// directory this process cannot see, and a lookup that silently returns
/// nothing for an unreadable path would be the same class of quiet wrong
/// answer.
fn normalise_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut last_was_sep = false;
    for c in path.chars() {
        if c == '/' {
            if !last_was_sep {
                out.push(c);
            }
            last_was_sep = true;
        } else {
            out.push(c);
            last_was_sep = false;
        }
    }
    while out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// The project whose checkout contains `dir`, if any.
///
/// Longest `root_path` wins, so a project nested inside another checkout
/// resolves to the inner one rather than whichever happened to be listed first.
fn project_for_directory(store: &Store, dir: &str) -> Option<EntityId> {
    let dir = normalise_path(dir);
    let dir = dir.as_str();
    let page = store
        .list(
            &EntityQuery::default()
                .of_type(EntityType::Project)
                .limited(500),
        )
        .ok()?;
    let mut best: Option<(usize, EntityId)> = None;
    for entity in page.items {
        let Entity::Project(p) = entity else { continue };
        let Some(root) = p.root_path.as_deref() else {
            continue;
        };
        let root_trimmed = normalise_path(root);
        let root_trimmed = root_trimmed.as_str();
        if dir == root_trimmed || dir.starts_with(&format!("{root_trimmed}/")) {
            let len = root_trimmed.len();
            if best.as_ref().is_none_or(|(best_len, _)| len > *best_len) {
                best = Some((len, p.id));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// Bring a caller's revision number into range without wrapping.
///
/// Saturating, so a number outside `i32` becomes one no revision has rather
/// than some unrelated revision that happens to exist.
fn narrow_version(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn specline_search(
    store: &Store,
    args: &Value,
    query_vector: Option<Vec<f32>>,
) -> Result<Value, RpcError> {
    let text = req_str(args, "query")?;
    let project_id = match opt_str(args, "project") {
        Some(p) => Some(resolve_project(store, &p)?),
        None => None,
    };
    let entity_types = parse_types(args.get("types"))?;

    let query = SearchQuery {
        text,
        project_id,
        entity_types,
        since: opt_time(args, "since")?,
        until: opt_time(args, "until")?,
        limit: opt_i64(args, "limit").unwrap_or(20).clamp(1, 100) as usize,
    };

    let results = store
        .search_prepared(&query, store.embedder(), query_vector.as_deref())
        .map_err(|e| to_rpc_error(store, e))?;
    let (page, report) = (results.page, results.report);

    // Which halves were silent, and why. The tool's description promises
    // hybrid retrieval; when only one half ran, the response has to say so or
    // the promise is what the reader believes (KEEL-251).
    let mut silent: Vec<(&'static str, &'static str)> = Vec::new();
    if let Some(why) = report.keyword.why() {
        silent.push(("keyword", why));
    }
    if let Some(why) = report.semantic.why() {
        silent.push(("semantic", why));
    }
    let caveat = silent
        .iter()
        .map(|(half, why)| format!("The {half} half did not run: {why}."))
        .collect::<Vec<_>>()
        .join(" ");
    // What the reader has actually lost, which is not the same for the two
    // halves: without the semantic one, wording that differs from the query
    // goes missing; without the keyword one, the exact words do.
    let consequence = match (report.keyword.ran(), report.semantic.ran()) {
        (true, false) => "anything worded differently from your query would not have been found",
        (false, true) => "the words you typed were never matched literally",
        _ => "neither index was asked",
    };

    let summary = if page.items.is_empty() {
        if silent.is_empty() {
            format!(
                "No matches for “{}”. Try fewer words, or drop the type filter.",
                query.text
            )
        } else {
            // The dangerous case, and the reason for all of the above. "No
            // matches" from half a search reads as a fact about the store, and
            // a model that believes it goes and writes down again whatever was
            // already there.
            format!(
                "No matches for “{}”, but only part of the search ran. {caveat} So this is not \
                 evidence that nothing is stored about it — {consequence}.",
                query.text
            )
        }
    } else {
        let mut lines = vec![format!(
            "{} match(es) for “{}”:",
            page.items.len(),
            query.text
        )];
        for hit in &page.items {
            lines.push(format!(
                "  [{}] {} — {}\n      {}",
                hit.entity_type, hit.title, hit.entity_id, hit.excerpt
            ));
        }
        if page.truncated {
            lines.push(format!(
                "  … {} more not shown",
                page.total - page.items.len()
            ));
        }
        if !silent.is_empty() {
            lines.push(format!(
                "  … and this is a partial search. {caveat} Expect gaps: {consequence}."
            ));
        }
        lines.join("\n")
    };

    Ok(tool_result(
        summary,
        json!({
            "hits": page.items,
            "total": page.total,
            "truncated": page.truncated,
            // Named halves rather than a boolean, because there are two of
            // them and either can be the one that is missing.
            "searched": report.ran(),
            "not_searched": silent
                .iter()
                .map(|(half, why)| ((*half).to_owned(), Value::String((*why).to_owned())))
                .collect::<serde_json::Map<_, _>>(),
        }),
    ))
}

fn specline_get(store: &Store, args: &Value) -> Result<Value, RpcError> {
    let ids: Vec<String> = args
        .get("ids")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .ok_or_else(|| bad_arg("ids", "missing", "an array of prefixed ULIDs"))?;
    if ids.is_empty() {
        return Err(bad_arg("ids", "empty", "at least one id"));
    }

    let include_body = args
        .get("include_body")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let depth = opt_i64(args, "depth").unwrap_or(0).clamp(0, 16) as u8;
    let direction = Direction::parse(opt_str(args, "direction").as_deref().unwrap_or("both"))
        .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))?;
    let rels = parse_rels(args.get("rels"))?;
    // Clamped rather than wrapped. On a read the consequence of a silly number
    // is only "no such revision", so there is nothing to protect — but `as i32`
    // would turn 4294967297 into 1 and return revision 1, which is a wrong
    // answer where "not found" was the right one.
    let version = opt_i64(args, "version").map(narrow_version);
    let diff_against = opt_i64(args, "diff_against").map(narrow_version);

    let mut found = Vec::new();
    let mut entities = Vec::new();
    let mut missing = Vec::new();

    for raw in &ids {
        let Some(id) = resolve_optional(store, "ids", raw)? else {
            missing.push(raw.clone());
            continue;
        };
        let Some(entity) = store.get(&id).map_err(|e| to_rpc_error(store, e))? else {
            missing.push(raw.clone());
            continue;
        };

        let mut item = json!({ "entity": entity_json_linked(store, &entity) });

        if include_body && id.entity_type().has_document() {
            let doc = store
                .revision(&id, version)
                .map_err(|e| to_rpc_error(store, e))?;
            item["document"] = serde_json::to_value(&doc).unwrap_or(Value::Null);

            if let Some(other) = diff_against {
                let from = version.unwrap_or_else(|| doc.as_ref().map(|d| d.version).unwrap_or(1));
                let diff = store
                    .diff(&id, other.min(from), other.max(from))
                    .map_err(|e| to_rpc_error(store, e))?;
                item["diff"] = serde_json::to_value(&diff).unwrap_or(Value::Null);
            }
        }

        if depth > 0 {
            let neighbours = store
                .neighbours(&id, direction, &rels, depth)
                .map_err(|e| to_rpc_error(store, e))?;
            item["neighbours"] = serde_json::to_value(&neighbours).unwrap_or(Value::Null);
        }

        found.push(item);
        entities.push(entity);
    }

    let mut summary = format!("{} artifact(s):", found.len());
    for (item, entity) in found.iter().zip(&entities) {
        if let Some(e) = item.get("entity") {
            // The readable reference where there is one, since that is what a
            // human will type back at the model in the next turn.
            let identifier = readable_ref(store, entity).unwrap_or_else(|| {
                e.get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_owned()
            });
            summary.push_str(&format!(
                "\n  [{}] {} — {}",
                e.get("type").and_then(Value::as_str).unwrap_or("?"),
                label_of(e),
                identifier,
            ));
        }
    }
    if !missing.is_empty() {
        // Not silent: an agent given fewer artifacts than it asked for, with
        // no indication, will assume the missing ones do not exist.
        summary.push_str(&format!("\n  not found: {}", missing.join(", ")));
    }

    Ok(tool_result(
        summary,
        json!({ "artifacts": found, "not_found": missing }),
    ))
}

fn specline_projects(store: &Store, args: &Value) -> Result<Value, RpcError> {
    let include_archived = opt_bool(args, "include_archived");
    let page = store
        .list(&EntityQuery {
            include_archived,
            ..EntityQuery::default().of_type(EntityType::Project)
        })
        .map_err(|e| to_rpc_error(store, e))?;

    let projects: Vec<&Project> = page
        .items
        .iter()
        .filter_map(|e| match e {
            Entity::Project(p) => Some(p),
            _ => None,
        })
        .collect();

    let Some(query) = opt_str(args, "query") else {
        let summary = if projects.is_empty() {
            "No projects yet.".to_owned()
        } else {
            let mut lines = vec![format!("{} project(s):", projects.len())];
            for p in &projects {
                lines.push(format!("  {} ({}) — {}", p.name, p.slug, p.status));
            }
            lines.join("\n")
        };
        return Ok(tool_result(summary, json!({ "projects": projects })));
    };

    // Fuzzy match on name, slug, aliases and repo URL — §6.4's disambiguation
    // surface. Scored rather than boolean so near-misses still surface: the
    // failure being prevented is a *second* project for something that already
    // exists, and a near-miss is exactly when that happens.
    let needle = query.to_lowercase();
    let mut scored: Vec<(u32, &&Project)> = projects
        .iter()
        .filter_map(|p| {
            let score = match_score(&needle, p);
            (score > 0).then_some((score, p))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));

    let candidates: Vec<&Project> = scored.iter().map(|(_, p)| **p).collect();
    let exact = scored.first().is_some_and(|(s, _)| *s >= 100);
    let requires_confirmation = !candidates.is_empty() && !exact;

    let summary = if candidates.is_empty() {
        format!(
            "No project matches “{query}”. If you are about to create one, confirm the name \
             with the human first."
        )
    } else if exact {
        format!("“{query}” resolves to {}.", candidates[0].slug)
    } else {
        let mut lines = vec![format!(
            "“{query}” did not match exactly, but these are close. Ask the human before \
             creating a new project:"
        )];
        for p in &candidates {
            lines.push(format!("  {} ({})", p.name, p.slug));
        }
        lines.join("\n")
    };

    Ok(tool_result(
        summary,
        json!({
            "projects": candidates,
            "requires_confirmation": requires_confirmation,
        }),
    ))
}

/// Score a project against a query. 100+ means an exact identifier match.
fn match_score(needle: &str, p: &Project) -> u32 {
    if p.slug.to_lowercase() == needle || p.name.to_lowercase() == needle {
        return 120;
    }
    if p.aliases.iter().any(|a| a.to_lowercase() == needle) {
        return 110;
    }
    if p.repo_urls
        .iter()
        .any(|u| u.to_lowercase().contains(needle))
    {
        return 90;
    }
    if p.slug.to_lowercase().contains(needle) || p.name.to_lowercase().contains(needle) {
        return 60;
    }
    if needle.len() >= 4
        && (needle.contains(&p.slug.to_lowercase()) || needle.contains(&p.name.to_lowercase()))
    {
        return 50;
    }
    0
}

/// Render a page of events for a model to read.
///
/// `more` names the way to get the rest, which differs by caller: a project
/// feed is paged by cursor, a single row's history is not paged at all and
/// simply takes a larger limit. Hard constraint 4 — a list that was cut says
/// so, with a total, and with what to do about it.
fn event_summary(page: &specline_core::Page<specline_core::Event>, more: &str) -> String {
    if page.items.is_empty() {
        return "Nothing has changed.".to_owned();
    }
    let mut lines = vec![format!("{} change(s):", page.items.len())];
    for e in &page.items {
        lines.push(format!(
            "  {} {} {} — {}",
            e.created_at.format("%Y-%m-%d %H:%M"),
            e.actor,
            e.action,
            e.summary
        ));
    }
    if page.truncated {
        lines.push(format!(
            "  … {} more; {more}",
            page.total - page.items.len()
        ));
    }
    lines.join("\n")
}

/// The project feed, oldest first.
///
/// It used to take an `entity` too, returning one row's whole history. That was
/// added without being asked for and removed on 2026-08-10 (TQ-24, KB's call).
/// The capability survives where it belongs: `GET /api/entity/{id}/history`
/// serves the desktop app's history panel, which is the only thing that ever
/// wanted it. B-15 is the rule — the local API has more endpoints than the tool
/// surface has tools, because a UI knows exactly what it wants and a model
/// chooses worse among more options.
fn specline_activity(store: &Store, args: &Value) -> Result<Value, RpcError> {
    let project = match opt_str(args, "project") {
        Some(p) => Some(resolve_project(store, &p)?),
        None => None,
    };
    let cursor = match opt_str(args, "cursor") {
        Some(raw) => Cursor::After(
            EventId::parse(&raw)
                .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))?,
        ),
        None => match opt_time(args, "since")? {
            Some(t) => Cursor::Since(t),
            None => Cursor::Beginning,
        },
    };
    let limit = opt_i64(args, "limit").unwrap_or(50).clamp(1, 500) as usize;

    let page = store
        .events(&cursor, project.as_ref(), limit)
        .map_err(|e| to_rpc_error(store, e))?;

    let next_cursor = page.items.last().map(|e| e.id.as_str().to_owned());
    let summary = event_summary(&page, "pass cursor to continue");

    Ok(tool_result(
        summary,
        json!({
            "events": page.items,
            "total": page.total,
            "truncated": page.truncated,
            "cursor": next_cursor,
        }),
    ))
}

/// The largest image accepted through a tool call, decoded.
///
/// A tool call is model context, and base64 costs about a third again on top of
/// the bytes — so a 1 MB image is roughly 1.4 MB of a context window. That is
/// the real constraint, not storage: the store would happily take a 50 MB
/// screenshot, and the model would drown carrying it there.
///
/// Refused with the actual size rather than truncated. A truncated image is a
/// corrupt file that looks like a successful write.
const MAX_IMAGE_BYTES: usize = 1_048_576;

/// The largest image the daemon will read off the disk.
///
/// Ten times the base64 ceiling, and the asymmetry is the whole point of this
/// path: nothing here enters model context, so the constraint stops being the
/// context window and becomes only "is this a picture or a mistake". A retina
/// screenshot is 300 KB to 2 MB and is the case this exists for.
const MAX_FILE_BYTES: usize = 10 * 1_048_576;

/// The folders this call may read a picture from.
///
/// `HOME` is read here rather than in [`crate::image_roots`] so that module
/// stays a pure function a test can drive. `specline-core` is the crate that may
/// not read the environment; this one is where the process's shape is already
/// known.
///
/// The project's own directory joins the list when the call names a project
/// that has one — a diagram committed beside the code is exactly as legitimate
/// as a screenshot on the Desktop, and refusing it would push people back to
/// base64 for the case the path argument exists to serve.
fn image_roots_for(store: &Store, project: Option<&EntityId>) -> Vec<std::path::PathBuf> {
    let home = std::env::var("HOME").ok().map(std::path::PathBuf::from);
    let project_root = project
        .and_then(|id| store.get(id).ok().flatten())
        .and_then(|entity| match entity {
            Entity::Project(project) => project.root_path,
            _ => None,
        })
        .map(std::path::PathBuf::from);

    crate::image_roots::roots(home.as_deref(), project_root.as_deref())
}

/// Read an image the caller named by path, if they named one.
///
/// TQ-33, KB's call: the daemon may read a file that is already on the same
/// machine. TQ-6's reasoning does not reach this — there is no outbound request,
/// nothing has to be published first, and the bytes never pass through the model.
/// That is what makes a real screenshot possible from Claude Code, where base64
/// through a tool call would cost 350,000 to 450,000 output tokens for 1 MB.
///
/// # The boundary this must keep
///
/// A local path and a URL look similar and are not. One touches the machine Specline
/// is already running on; the other gives a model the ability to make the daemon
/// talk to the internet, which TQ-6 declined. So anything URL-shaped is refused
/// here explicitly rather than left to whatever the filesystem makes of it — if
/// that check ever goes, TQ-33 has been reversed by accident.
fn read_image_file(
    args: &Value,
    roots: &[std::path::PathBuf],
) -> Result<Option<(Vec<u8>, String)>, RpcError> {
    let Some(raw) = opt_str(args, "image_path") else {
        return Ok(None);
    };
    let path = raw.trim();

    if let Some((scheme, _)) = path.split_once("://") {
        return Err(bad_arg(
            "image_path",
            &format!(
                "`{scheme}://…` is a URL, and this reads a file on the machine Specline is running \
                 on. The daemon makes no outbound requests on a model's instruction (TQ-6)"
            ),
            "an absolute path such as /Users/you/Desktop/screenshot.png",
        ));
    }
    if path.is_empty() {
        return Err(bad_arg(
            "image_path",
            "is empty",
            "a path to a file on disk",
        ));
    }

    let file = std::path::Path::new(path);
    if !file.is_absolute() {
        // The daemon's working directory is its own, not the caller's, so a
        // relative path resolves against something the caller cannot see. Better
        // to refuse than to read the wrong file or none.
        return Err(bad_arg(
            "image_path",
            &format!("`{path}` is relative, and the daemon's working directory is not yours"),
            "an absolute path",
        ));
    }

    // Resolved before anything is read, so `..` and symlinks are settled as
    // locations rather than as spellings — and so a file outside the allowed
    // folders is refused without its bytes ever being opened.
    let resolved = file.canonicalize().map_err(|e| {
        bad_arg(
            "image_path",
            &format!("could not read `{path}`: {e}"),
            "a readable file on this machine. If the image is somewhere the daemon cannot \
             reach, pass it as base64 in `image` instead",
        )
    })?;

    if !crate::image_roots::contains(roots, &resolved) {
        return Err(bad_arg(
            "image_path",
            &format!(
                "`{path}` is outside the folders Specline reads pictures from. The model choosing \
                 this path may be acting on text it did not write, so the answer is a list \
                 rather than a judgement"
            ),
            &format!(
                "a file in one of: {}. Anywhere else, read the image yourself and pass it as \
                 base64 in `image`",
                crate::image_roots::describe(roots)
            ),
        ));
    }

    let bytes = std::fs::read(&resolved).map_err(|e| {
        bad_arg(
            "image_path",
            &format!("could not read `{path}`: {e}"),
            "a readable file on this machine. If the image is somewhere the daemon cannot \
             reach, pass it as base64 in `image` instead",
        )
    })?;

    if bytes.is_empty() {
        return Err(bad_arg(
            "image_path",
            &format!("`{path}` is empty"),
            "a file with an image in it",
        ));
    }
    if bytes.len() > MAX_FILE_BYTES {
        return Err(bad_arg(
            "image_path",
            &format!(
                "the file is {} bytes, over the {} byte limit",
                bytes.len(),
                MAX_FILE_BYTES
            ),
            "an image under 10 MB",
        ));
    }

    // Sniffed, never inferred from the extension. A `.png` that is a text file is
    // a corrupt blob the app will try to render, and the extension is whatever
    // somebody typed.
    let Some(media_type) = sniff_media_type(&bytes) else {
        return Err(bad_arg(
            "image_path",
            &format!("`{path}` is not an image Specline recognises"),
            "a PNG, JPEG, GIF, WebP or SVG file",
        ));
    };

    Ok(Some((bytes, media_type.to_owned())))
}

/// Decode an inline base64 image, if one was supplied.
///
/// Base64 in the tool call is the only ingestion path that works from every
/// surface (TQ-6, KB's call). A filesystem path works only where there is a
/// filesystem, which excludes chat and Cowork — the two places design images
/// actually come from — and fetching a URL would make the daemon issue outbound
/// requests on a model's instruction, which is a larger capability than this
/// needs.
///
/// Accepts a bare base64 payload or a full `data:` URL, because a model that
/// has just been handed an image will produce either.
fn decode_image(args: &Value) -> Result<Option<(Vec<u8>, String)>, RpcError> {
    use base64::Engine as _;

    if args.get("image_path").is_some() && args.get("image").is_some() {
        return Err(bad_arg(
            "image",
            "both `image` and `image_path` were given, and they are two answers to one \
             question",
            "one of them — `image_path` for a file on this machine, `image` for base64 you \
             are already holding",
        ));
    }

    let Some(raw) = opt_str(args, "image") else {
        return Ok(None);
    };

    let (declared_type, payload) = match raw.strip_prefix("data:") {
        Some(rest) => match rest.split_once(";base64,") {
            Some((mime, data)) => (Some(mime.to_owned()), data.to_owned()),
            None => {
                return Err(bad_arg(
                    "image",
                    "a `data:` URL must be base64-encoded",
                    "`data:image/png;base64,<data>`, or the bare base64 payload",
                ));
            }
        },
        None => (None, raw),
    };

    // Whitespace is stripped first: a model wrapping a long payload across
    // lines is producing valid intent and invalid base64, and failing on it
    // would be a papercut with no upside.
    let cleaned: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| {
            bad_arg(
                "image",
                &format!("could not decode as base64: {e}"),
                "standard base64, optionally as a `data:` URL",
            )
        })?;

    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(bad_arg(
            "image",
            &format!(
                "the image is {} bytes, over the {} byte limit",
                bytes.len(),
                MAX_IMAGE_BYTES
            ),
            "an image under 1 MB — resize or crop it, or store it elsewhere and \
             put the URL in the body",
        ));
    }
    if bytes.is_empty() {
        return Err(bad_arg(
            "image",
            "decoded to zero bytes",
            "a non-empty base64 payload",
        ));
    }

    // Sniffed from the bytes rather than trusted from the caller: a `data:`
    // URL's declared type is whatever the sender wrote, and the app decides how
    // to render from this. Falls back to the declaration, then to a type that
    // makes a browser download rather than guess.
    let media_type = sniff_media_type(&bytes)
        .map(str::to_owned)
        .or(declared_type)
        .unwrap_or_else(|| "application/octet-stream".to_owned());

    Ok(Some((bytes, media_type)))
}

/// Identify an image from its magic bytes.
fn sniff_media_type(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, ..] => Some("image/png"),
        [0xff, 0xd8, 0xff, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        _ if bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml") => Some("image/svg+xml"),
        _ => None,
    }
}

fn specline_create(store: &mut Store, args: &Value) -> Result<Value, RpcError> {
    let type_name = req_str(args, "type")?;
    let provenance = provenance_from(args)?;

    let title = opt_str(args, "title")
        .or_else(|| opt_str(args, "name"))
        .or_else(|| opt_str(args, "term"));
    let body = opt_str(args, "body");

    // The project is resolved before the type, which is the wrong-looking order
    // and the necessary one: a project's glossary is what says whether "phase"
    // means anything here, so the type cannot be resolved without knowing which
    // project is asking.
    let project_id = match opt_str(args, "project") {
        Some(p) => Some(resolve_project(store, &p)?),
        None => None,
    };

    // Accept the word this project actually uses, from its own glossary before
    // Specline's built-in list. Carried through to the summary rather than dropped:
    // a silent success teaches the session nothing and it guesses the same way
    // next time, where a narrated one teaches the vocabulary in one round trip.
    // KEEL-116 / KEEL-121.
    let resolved = specline_core::resolve_type(store, project_id.as_ref(), &type_name)
        .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))?;
    let entity_type = resolved.entity_type;
    let alias = resolved.from.clone();

    if entity_type != EntityType::Project && entity_type != EntityType::Term && project_id.is_none()
    {
        return Err(bad_arg(
            "project",
            &format!("{entity_type} must belong to a project"),
            "a project id or slug; call specline_projects to list them",
        ));
    }

    // Decoded and size-checked before anything is written, so a bad image
    // fails without leaving a half-made design behind for someone to find.
    let image = match decode_image(args)? {
        Some(inline) => Some(inline),
        None => read_image_file(args, &image_roots_for(store, project_id.as_ref()))?,
    };
    if image.is_some() && !matches!(entity_type, EntityType::Design | EntityType::Artifact) {
        return Err(bad_arg(
            "image",
            &format!("{entity_type} does not hold an image"),
            "type `design` for a mockup or screenshot, or `artifact` for anything else",
        ));
    }

    let mut entity = build_entity(
        entity_type,
        project_id.clone(),
        title.clone(),
        body.clone(),
        args,
    )?;

    // Extra columns, applied through the same validated path an update uses,
    // so `fields: {status: "dun"}` produces the same helpful error either way.
    if let Some(Value::Object(fields)) = args.get("fields") {
        let consumed = consumed_by_constructor(entity_type);
        let mut cleaned = Map::new();
        for (k, v) in fields {
            if consumed.contains(&k.as_str()) {
                continue;
            }
            cleaned.insert(k.clone(), v.clone());
        }
        specline_core::store::apply_changes(&mut entity, &cleaned)
            .map_err(|e| to_rpc_error(store, e))?;
    }

    if let Some(key) = opt_str(args, "idempotency_key") {
        entity.set_idempotency_key(key);
    }

    // House style, B-46. Before the write and at the authoring boundary rather
    // than inside `write_revision`, because `specline import` writes revisions too
    // and that is a person deliberately migrating text they did not write — the
    // same carve-out the mirror rule already makes for it.
    //
    // Checked *before* the create, not after. It used to run once the row had
    // landed, so a refused body left a headless row behind and the caller saw
    // an error that did not mention it (KEEL-171).
    let mut style_warnings = Vec::new();
    if let Some(text) = body.as_deref().filter(|_| entity_type.has_document()) {
        style_warnings = specline_core::check_style(entity_type, "body", text, title.as_deref())
            .map_err(|e| to_rpc_error(store, e))?;
    }

    let byte_length = image.as_ref().map(|(bytes, _)| bytes.len());
    let media_type = image.as_ref().map(|(_, media)| media.clone());

    // One call, one transaction. The entity, its first revision, the image and
    // the row's pointer at that image land together or not at all — this used
    // to be four store calls orchestrated here, and a crash between any two of
    // them left a body-less entity or a blob nothing points at.
    let created = store
        .create_with_document(entity, body.clone(), image, &provenance)
        .map_err(|e| to_rpc_error(store, e))?;

    let document = created
        .document
        .as_ref()
        .and_then(|d| serde_json::to_value(d).ok())
        .unwrap_or(Value::Null);

    if let Some(blob_id) = &created.blob_id {
        tracing::info!(
            entity = %created.entity.id(),
            %blob_id,
            media_type,
            byte_length,
            "stored an inline image"
        );
    }

    // A project that says what it calls a milestone gets that word in its
    // glossary, which is the one part of the digest that is never truncated. That
    // is what turns "the board says Phase" into "a session is told, in its first
    // call, that this project calls milestones phases" — and it makes the word an
    // input alias as well as a label, through the same mechanism any other
    // project-specific word uses.
    let mut seeded_term = Value::Null;
    if let Entity::Project(project) = &created.entity
        && created.created
        && let Some(noun) = project.milestone_noun.clone()
    {
        let term = specline_core::vocabulary::milestone_noun_term(&project.id, &noun);
        match store.create(term.into(), &provenance) {
            Ok(term) => seeded_term = json!(term.entity.id().to_string()),
            // A term already using that word is not a reason to refuse the
            // project. The noun still works — resolution consults it directly as
            // well as through the glossary — so this is worth a log line and not
            // an error.
            Err(e) => tracing::warn!(
                error = %e,
                %noun,
                "could not seed the glossary term for this project's milestone noun"
            ),
        }
    }

    // `create_with_document` already re-read the row, so `current_doc_version`
    // and `blob_id` reflect what the transaction wrote. Seeding a glossary term
    // above does not touch this row, so there is nothing else to catch up on.
    let entity = created.entity;

    let identifier =
        readable_ref(store, &entity).unwrap_or_else(|| entity.id().as_str().to_owned());
    let mut summary = if created.created {
        format!(
            "Created {} “{}” — {}",
            entity_type,
            entity.label(),
            identifier
        )
    } else {
        format!(
            "{} “{}” already exists — {} (nothing was created)",
            entity_type,
            entity.label(),
            identifier
        )
    };
    if let Some(alias) = &alias {
        // Saying *why* is the same argument as saying *what*, one step further:
        // "because this project's glossary says so" is actionable, where
        // "because Specline accepts it" leaves a session none the wiser about where
        // the vocabulary lives.
        summary.push_str(&format!(
            "\n\nYou said “{alias}” — in Specline that is a {entity_type}, because {}. Same \
             thing, one word.",
            resolved.source.because()
        ));
    }
    summary.push_str(&style_note(&style_warnings));

    Ok(tool_result(
        summary,
        json!({
            "entity": entity_json_linked(store, &entity),
            "created": created.created,
            "document": document,
            "style_warnings": style_warnings,
            "resolved_from": alias,
            "seeded_term": seeded_term,
        }),
    ))
}

/// Construct the right struct for a type, from the common arguments.
fn build_entity(
    entity_type: EntityType,
    project_id: Option<EntityId>,
    title: Option<String>,
    body: Option<String>,
    args: &Value,
) -> Result<Entity, RpcError> {
    let need_title = || -> Result<String, RpcError> {
        title.clone().ok_or_else(|| {
            bad_arg(
                "title",
                &format!("{entity_type} needs a name"),
                "a short one-line title",
            )
        })
    };
    let need_project = || -> Result<EntityId, RpcError> {
        project_id
            .clone()
            .ok_or_else(|| bad_arg("project", "required", "a project id or slug"))
    };

    Ok(match entity_type {
        EntityType::Project => {
            let name = need_title()?;
            let slug = opt_str(args, "slug").unwrap_or_else(|| slugify(&name));
            let mut p = Project::new(slug, name);
            p.description = body;
            p.into()
        }
        EntityType::Milestone => {
            // `summary` first, then `body`, because a caller reaching for the
            // generic prose field means the same thing here. Before this,
            // `body` was accepted and silently discarded — every milestone
            // written over MCP landed on the roadmap as a bare name, and the
            // caller had no way to find out. B-45.
            let summary = opt_str(args, "summary")
                .or_else(|| body.clone())
                .unwrap_or_default();
            Milestone::new(need_project()?, need_title()?, summary).into()
        }
        EntityType::Task => {
            let mut t = Task::new(
                need_project()?,
                need_title()?,
                "A row this test needs in the store.",
            );
            t.body = body.clone();
            // `summary` first, then `body` — a caller reaching for the generic
            // prose field means the same thing, and the store refuses an empty
            // one either way rather than accepting and discarding it. TQ-34.
            t.summary = opt_str(args, "summary").or(body);
            t.into()
        }
        EntityType::Spec => Spec::new(need_project()?, need_title()?).into(),
        EntityType::Decision => Decision::new(need_project()?, need_title()?).into(),
        EntityType::Question => Question::new(need_project()?, need_title()?).into(),
        EntityType::Term => {
            let definition = body
                .clone()
                .or_else(|| opt_str(args, "definition"))
                .ok_or_else(|| {
                    bad_arg(
                        "body",
                        "a term needs a definition",
                        "what the word means in this project",
                    )
                })?;
            Term::new(project_id.clone(), need_title()?, definition).into()
        }
        EntityType::Feedback => {
            // The column is `summary`, not `title`, and §3.2 says why: a piece
            // of feedback is what somebody said, so titling it would mean
            // inventing one. Accept `summary` first — that is the name a
            // caller who read the schema will reach for — then fall back to
            // `title`/`name` for the generic create shape.
            //
            // Before this the arm was `need_title()`, so the one type whose
            // design is "do not invent a title" was the only type that
            // refused to be created without one, and the refusal named a
            // field the table does not have. `body` is deliberately not a
            // fallback here as it is for a task: a feedback body is the
            // verbatim, which is exactly the thing too long to be a summary.
            let summary = opt_str(args, "summary")
                .or_else(|| title.clone())
                .ok_or_else(|| {
                    bad_arg(
                        "summary",
                        "feedback needs the thing that was said",
                        "one line of what somebody actually said, in their own words",
                    )
                })?;
            Feedback::new(need_project()?, summary).into()
        }
        EntityType::Design => Design::new(need_project()?, need_title()?).into(),
        EntityType::Environment => Environment::new(need_project()?, need_title()?).into(),
        EntityType::Metric => Metric::new(need_project()?, need_title()?).into(),
        EntityType::MetricObservation => {
            // All three read from the top level *or* from `fields`, because the
            // schema promises `fields` takes any other column and these are the
            // three it did not (KEEL-172).
            let metric_raw = arg_or_field(args, "metric_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    bad_arg(
                        "metric_id",
                        "missing or not a string",
                        "the `mtr_…` id of the metric being measured — `specline_search` with \
                         type `metric` will find it",
                    )
                })?;
            let metric_id = EntityId::parse_as(metric_raw, EntityType::Metric)
                .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))?;
            let value = arg_or_field(args, "value")
                .and_then(Value::as_f64)
                .ok_or_else(|| bad_arg("value", "missing", "the measured number"))?;
            let observed_at = parse_time(
                arg_or_field(args, "observed_at").and_then(Value::as_str),
                "observed_at",
            )?
            .unwrap_or_else(chrono::Utc::now);
            MetricObservation::new(metric_id, need_project()?, value, observed_at).into()
        }
        EntityType::Artifact => Artifact::new(need_project()?, need_title()?).into(),
    })
}

/// A URL-safe slug from a display name.
fn slugify(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    s.split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Fields inside `changes` whose value is another row's id.
///
/// These have to accept `KEEL-42` for the same reason the `id` argument does:
/// the readable identifier is only an identifier if it works everywhere one is
/// taken. Missing this made `{"parent_id": "KEEL-92"}` fail with "no task with
/// id KEEL-92 exists" about a task that plainly does.
const REFERENCE_FIELDS: &[&str] = &["parent_id"];

/// Rewrite readable references in `changes` into the ids the store stores.
///
/// Only touches a value that *is* a readable reference. A ULID is already
/// right, and anything else is left alone so that serde's own error — which
/// names the field and the valid shape — is what the caller sees.
fn resolve_reference_fields(
    store: &Store,
    changes: &mut Map<String, Value>,
) -> Result<(), RpcError> {
    for field in REFERENCE_FIELDS {
        let Some(Value::String(raw)) = changes.get(*field) else {
            continue;
        };
        if specline_core::types::parse_readable_ref(raw).is_none() {
            continue;
        }
        let id = resolve_required(store, field, &raw.clone())?;
        changes.insert((*field).to_owned(), json!(id.as_str()));
    }
    Ok(())
}

/// Turn "put it above the auth work" into a number.
///
/// `rank` is a float so that a move touches one row, but no caller should ever
/// have to pick the float. `rank_after` and `rank_before` name a *task* — by
/// readable reference or by id — and this resolves the value, which is the
/// difference between a surface a model can use and one it has to compute
/// against.
///
/// Both are consumed here rather than reaching the entity, because `Task` has
/// no such fields and `apply_changes` would rightly reject them.
fn resolve_rank_placement(store: &Store, changes: &mut Map<String, Value>) -> Result<(), RpcError> {
    let after = changes.remove("rank_after");
    let before = changes.remove("rank_before");

    // A task's own rank, and the rank of whatever currently sits next to it on
    // the far side — placing "after A" means between A and A's successor.
    let neighbour = |raw: &Value, field: &str| -> Result<f64, RpcError> {
        let reference = raw
            .as_str()
            .ok_or_else(|| bad_arg(field, "not a string", ID_OR_REF))?;
        let id = resolve_required(store, field, reference)?;
        match store.get(&id).map_err(|e| to_rpc_error(store, e))? {
            Some(Entity::Task(t)) => Ok(t.rank),
            _ => Err(bad_arg(
                field,
                &format!("`{reference}` is not a task"),
                "a task — only tasks carry a rank",
            )),
        }
    };

    // The early return is an arm rather than a guard thirty lines above. It
    // used to be the guard, and the arm it made impossible was an
    // `unreachable!` — a panic in library code whose safety depended on a
    // condition far enough away to be edited independently of it.
    let (low, high) = match (&after, &before) {
        (None, None) => return Ok(()),
        (Some(a), None) => {
            let anchor = neighbour(a, "rank_after")?;
            (Some(anchor), successor_rank(store, anchor)?)
        }
        (None, Some(b)) => {
            let anchor = neighbour(b, "rank_before")?;
            (predecessor_rank(store, anchor)?, Some(anchor))
        }
        (Some(a), Some(b)) => (
            Some(neighbour(a, "rank_after")?),
            Some(neighbour(b, "rank_before")?),
        ),
    };

    // "After X and before Y" where X is already below Y describes a place that
    // does not exist. `rank_between` takes the midpoint of the two, so it would
    // have answered with a number *outside* the range the caller named and put
    // the task somewhere neither anchor implies — a move that succeeds and
    // lands in the wrong place is worse than one that is refused.
    if let (Some(low), Some(high)) = (low, high)
        && low >= high
    {
        return Err(bad_arg(
            "rank_after",
            &format!(
                "rank_after sits at {low} and rank_before at {high}, so there is no room \
                 between them in that order"
            ),
            "two tasks with rank_after currently ranked below rank_before, or just one of them",
        ));
    }

    let rank = store
        .rank_between(low, high)
        .map_err(|e| to_rpc_error(store, e))?;
    changes.insert("rank".to_owned(), json!(rank));
    Ok(())
}

/// The rank immediately above `anchor`, if anything is there.
fn successor_rank(store: &Store, anchor: f64) -> Result<Option<f64>, RpcError> {
    Ok(store
        .list(
            &EntityQuery::default()
                .of_type(EntityType::Task)
                .limited(5_000),
        )
        .map_err(|e| to_rpc_error(store, e))?
        .items
        .iter()
        .filter_map(|e| match e {
            Entity::Task(t) if t.rank > anchor => Some(t.rank),
            _ => None,
        })
        .min_by(|a, b| a.total_cmp(b)))
}

/// The rank immediately below `anchor`, if anything is there.
fn predecessor_rank(store: &Store, anchor: f64) -> Result<Option<f64>, RpcError> {
    Ok(store
        .list(
            &EntityQuery::default()
                .of_type(EntityType::Task)
                .limited(5_000),
        )
        .map_err(|e| to_rpc_error(store, e))?
        .items
        .iter()
        .filter_map(|e| match e {
            Entity::Task(t) if t.rank < anchor => Some(t.rank),
            _ => None,
        })
        .max_by(|a, b| a.total_cmp(b)))
}

fn specline_update(store: &mut Store, args: &Value) -> Result<Value, RpcError> {
    let raw_id = req_str(args, "id")?;
    let id = resolve_required(store, "id", &raw_id)?;
    let version = opt_i64(args, "version").ok_or_else(|| {
        bad_arg(
            "version",
            "missing",
            "the `version` you read, so a concurrent edit can be detected",
        )
    })?;
    // Narrowed by checking rather than by `as i32`, which wraps. A caller
    // sending 4294967297 got 1 — and 1 is a real version, so the stale-write
    // check the argument exists for would have compared against it and passed.
    // The one guard against a lost update, defeated by a number too large to
    // be a version at all.
    let version = i32::try_from(version).map_err(|_| {
        bad_arg(
            "version",
            &format!("{version} is not a version any artifact could have"),
            "the `version` field from the artifact you read",
        )
    })?;
    let provenance = provenance_from(args)?;

    if opt_bool(args, "archive") {
        let archived = store
            .archive(&id, version, &provenance)
            .map_err(|e| to_rpc_error(store, e))?;
        return Ok(tool_result(
            format!("Archived {} “{}”", archived.entity_type(), archived.label()),
            json!({ "entity": entity_json(&archived), "archived": true }),
        ));
    }

    let mut changes = match args.get("changes") {
        Some(Value::Object(m)) => m.clone(),
        _ => {
            return Err(bad_arg(
                "changes",
                "missing or not an object",
                "an object of field names to new values",
            ));
        }
    };

    resolve_reference_fields(store, &mut changes)?;
    resolve_rank_placement(store, &mut changes)?;

    // Attaching an image to something that already exists. TQ-33 approved this
    // as a `keel_attach(id, path)` tool; it is a field on `specline_update` instead,
    // because TQ-31 set thirteen tools as the ceiling hours earlier and the
    // standing rule is that an awkward capability is almost always a field. The
    // capability is the one KB approved and the count is the one KB set — see
    // B-49 for the argument, and note that `specline_create` takes `image_path` too,
    // so a design born with its screenshot needs no second call.
    //
    // Read before the update, so a bad path refuses the whole call rather than
    // leaving a version bump behind with no image attached to it.
    let attachment = match changes.remove("image_path") {
        Some(Value::String(path)) => {
            if !matches!(id.entity_type(), EntityType::Design | EntityType::Artifact) {
                return Err(bad_arg(
                    "image_path",
                    &format!("{} does not hold an image", id.entity_type()),
                    "a design or an artifact",
                ));
            }
            // The project this row belongs to, so a diagram committed beside
            // the code counts as being somewhere Specline may read from.
            let owner = store
                .get(&id)
                .ok()
                .flatten()
                .and_then(|e| e.project_id().cloned());
            read_image_file(
                &json!({ "image_path": path }),
                &image_roots_for(store, owner.as_ref()),
            )?
        }
        Some(other) => {
            return Err(bad_arg(
                "image_path",
                &format!("must be a path, got {other}"),
                "an absolute path to an image on this machine",
            ));
        }
        None => None,
    };

    // An update whose only instruction was the attachment has nothing to change
    // on the row itself, and `update` would reject an empty change set as a
    // missing argument. The blob write below is the change.
    let attach_only = attachment.is_some() && changes.is_empty();
    let updated = if attach_only {
        store
            .get(&id)
            .map_err(|e| to_rpc_error(store, e))?
            .ok_or_else(|| {
                bad_arg(
                    "id",
                    &format!("{id} does not exist"),
                    "an existing artifact",
                )
            })?
    } else {
        store
            .update(&id, version, &changes, &provenance)
            .map_err(|e| to_rpc_error(store, e))?
    };

    if let Some((bytes, media_type)) = attachment {
        let byte_length = bytes.len();
        let project = updated
            .project_id()
            .cloned()
            .unwrap_or_else(|| updated.id().clone());
        let blob = specline_core::store::Blob::new(bytes, media_type.clone(), specline_core::now())
            .owned_by(updated.id().clone(), project);
        let blob_id = store.put_blob(blob).map_err(|e| to_rpc_error(store, e))?;

        let mut pointer = Map::new();
        pointer.insert("blob_id".to_owned(), json!(blob_id.as_str()));
        let repointed = store
            .update(updated.id(), updated.audit().version, &pointer, &provenance)
            .map_err(|e| to_rpc_error(store, e))?;
        tracing::info!(
            entity = %repointed.id(),
            %blob_id,
            media_type,
            byte_length,
            "attached an image the daemon read from disk"
        );
        return Ok(tool_result(
            format!(
                "Attached a {media_type} of {byte_length} bytes to “{}”. The bytes went \
                 straight from the file to the store, so none of them entered your context.",
                repointed.label()
            ),
            json!({
                "entity": entity_json(&repointed),
                "attached": {
                    "blob_id": blob_id.as_str(),
                    "media_type": media_type,
                    "bytes": byte_length,
                },
            }),
        ));
    }

    // A project that has just been given a word for its milestones gets that word
    // in its glossary, the same as one created with it. Doing this only on create
    // would mean an existing project could set the noun and have the interface
    // change while the digest — the one place a session reads the vocabulary
    // before it needs it — never mentioned it.
    if changes.contains_key("milestone_noun")
        && let Entity::Project(project) = &updated
        && let Some(noun) = project.milestone_noun.clone()
    {
        let term = specline_core::vocabulary::milestone_noun_term(&project.id, &noun);
        if let Err(e) = store.create(term.into(), &provenance) {
            tracing::warn!(error = %e, %noun, "could not seed the glossary term for this noun");
        }
    }

    let changed: Vec<&String> = changes.keys().collect();
    let summary = if changed.is_empty() {
        format!(
            "{} was already in that state; nothing changed.",
            updated.label()
        )
    } else {
        format!(
            "Updated {} “{}” ({}) — now version {}",
            updated.entity_type(),
            updated.label(),
            changed
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            updated.audit().version
        )
    };

    Ok(tool_result(
        summary,
        json!({ "entity": entity_json(&updated) }),
    ))
}

fn specline_write_doc(store: &mut Store, args: &Value) -> Result<Value, RpcError> {
    let raw_id = req_str(args, "id")?;
    let id = resolve_required(store, "id", &raw_id)?;
    let body = req_str(args, "body")?;
    let provenance = provenance_from(args)?;

    if !id.entity_type().has_document() {
        return Err(bad_arg(
            "id",
            &format!("{} has no prose body", id.entity_type()),
            "a spec, decision, question, feedback or design id",
        ));
    }

    let entity = store
        .get(&id)
        .map_err(|e| to_rpc_error(store, e))?
        .ok_or_else(|| {
            bad_arg(
                "id",
                &format!("no artifact with id {id}"),
                "an id returned by specline_create or specline_search",
            )
        })?;

    let title = opt_str(args, "title").unwrap_or_else(|| entity.label().to_owned());
    let previous = store
        .revision(&id, None)
        .map_err(|e| to_rpc_error(store, e))?
        .map(|d| d.version);

    // House style, B-46. See the note on the same check in `create`: this is the
    // authoring door, and `specline import` deliberately does not come through it.
    let style_warnings = specline_core::check_style(id.entity_type(), "body", &body, Some(&title))
        .map_err(|e| to_rpc_error(store, e))?;

    let doc = specline_core::Document::first(
        id.entity_type(),
        id.clone(),
        entity.project_id().cloned(),
        title,
        body,
        provenance.actor,
        specline_core::now(),
    )
    .map_err(|e| to_rpc_error(store, e))?
    .attributed(provenance.session_id.clone(), provenance.surface);

    let written = store
        .write_revision(doc)
        .map_err(|e| to_rpc_error(store, e))?;

    let unchanged = previous == Some(written.version);
    let mut summary = if unchanged {
        format!(
            "Body of “{}” is unchanged; still at revision {}.",
            written.title, written.version
        )
    } else {
        format!(
            "Wrote revision {} of “{}” ({} bytes).",
            written.version,
            written.title,
            written.body.len()
        )
    };
    summary.push_str(&style_note(&style_warnings));

    Ok(tool_result(
        summary,
        json!({
            "document": written,
            "created_revision": !unchanged,
            "style_warnings": style_warnings,
        }),
    ))
}

/// Render style warnings as a line appended to a successful write's summary.
///
/// Attached to the write rather than raised instead of it. These are signals,
/// not rules — "crucial" is common in machine-written prose and also in good
/// human prose — and refusing on a signal is how a model learns to write around
/// the check rather than to write plainly. Saying it out loud on a write that
/// landed teaches at the only moment the lesson is cheap.
fn style_note(warnings: &[specline_core::Warning]) -> String {
    if warnings.is_empty() {
        return String::new();
    }
    let items: Vec<String> = warnings
        .iter()
        .map(|w| format!("“{}” — {}", w.found, w.instead))
        .collect();
    format!(
        "\n\nHouse style, worth a look next time: {}.",
        items.join("; ")
    )
}

/// Append to an artifact's running commentary, list it, or retract one entry.
///
/// Three modes on one tool rather than three tools, because listing and
/// retracting are rare and the ceiling on the tool surface is real. Adding is
/// the default and needs no flag: the common case should cost the model no
/// decision at all.
fn specline_note(store: &mut Store, args: &Value) -> Result<Value, RpcError> {
    let provenance = provenance_from(args)?;

    if let Some(note_id) = opt_str(args, "retract") {
        let id = specline_core::NoteId::parse(&note_id)
            .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))?;
        let note = store
            .retract_note(&id, &provenance)
            .map_err(|e| to_rpc_error(store, e))?;
        return Ok(tool_result(
            format!(
                "Retracted {}. It stays readable as a record of what was believed.",
                note.id
            ),
            json!({ "note": note, "retracted": true }),
        ));
    }

    let id = resolve_required(store, "id", &req_str(args, "id")?)?;

    if opt_bool(args, "list") {
        let notes = store
            .notes_for(&id, false)
            .map_err(|e| to_rpc_error(store, e))?;
        let summary = if notes.is_empty() {
            format!("No notes on {id} yet.")
        } else {
            format!("{} note(s) on {id}, oldest first.", notes.len())
        };
        return Ok(tool_result(summary, json!({ "notes": notes })));
    }

    let body = req_str(args, "body")?;
    let mut new_note = NewNote::new(id.clone(), body, Actor::Claude);
    if let Some(session) = provenance.session_id.clone() {
        new_note = new_note.in_session(session);
    }
    let note = store
        .add_note(new_note, &provenance)
        .map_err(|e| to_rpc_error(store, e))?;

    Ok(tool_result(
        format!("Noted on {id}. {}", note.headline()),
        json!({ "note": note }),
    ))
}

fn specline_link(store: &mut Store, args: &Value) -> Result<Value, RpcError> {
    let from = resolve_required(store, "from", &req_str(args, "from")?)?;
    let to = resolve_required(store, "to", &req_str(args, "to")?)?;
    let rel = Relation::parse(&req_str(args, "rel")?)
        .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))?;
    let anchor = opt_str(args, "anchor").unwrap_or_default();
    let provenance = provenance_from(args)?;

    if opt_bool(args, "remove") {
        let removed = store
            .unlink(&from, rel, &to, &anchor, &provenance)
            .map_err(|e| to_rpc_error(store, e))?;
        return Ok(tool_result(
            format!("Removed {from} {rel} {to}"),
            json!({ "link": removed, "removed": true }),
        ));
    }

    let mut new_link = NewLink::new(from.clone(), rel, to.clone());
    if !anchor.is_empty() {
        new_link = new_link.anchored(anchor.clone());
    }
    if let Some(note) = opt_str(args, "note") {
        new_link = new_link.noted(note);
    }

    let link = store
        .link(new_link, &provenance)
        .map_err(|e| to_rpc_error(store, e))?;

    // Say what was actually stored when the caller asked for `depends_on`.
    // Leaving it implicit is how the next reader concludes the endpoints went
    // in backwards.
    let summary = if rel == Relation::DependsOn {
        format!(
            "Linked {from} depends_on {to}, stored as {} blocks {} — `depends_on` and `blocks` \
             are the same fact, and Specline keeps one direction.",
            link.from_id, link.to_id
        )
    } else {
        format!(
            "Linked {} {} {}{}",
            link.from_id,
            link.rel,
            link.to_id,
            if anchor.is_empty() {
                String::new()
            } else {
                format!(" at {anchor}")
            }
        )
    };

    Ok(tool_result(summary, json!({ "link": link })))
}

/// Parse a `types` argument.
fn parse_types(value: Option<&Value>) -> Result<Vec<EntityType>, RpcError> {
    let Some(Value::Array(items)) = value else {
        return Ok(Vec::new());
    };
    items
        .iter()
        .filter_map(Value::as_str)
        .map(|s| {
            EntityType::parse(s).map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))
        })
        .collect()
}

/// Parse a `rels` argument.
fn parse_rels(value: Option<&Value>) -> Result<Vec<Relation>, RpcError> {
    let Some(Value::Array(items)) = value else {
        return Ok(Vec::new());
    };
    items
        .iter()
        .filter_map(Value::as_str)
        .map(|s| {
            Relation::parse(s).map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))
        })
        .collect()
}

/// Read an optional array-of-strings argument.
///
/// A bare string is accepted as a one-item list. Models send `"desktop"` where
/// the schema says array often enough that refusing it would cost a round trip
/// to teach nothing — and there is no other reading of a single string here.
fn opt_str_list(args: &Value, field: &str) -> Vec<String> {
    match args.get(field) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(one)) => vec![one.clone()],
        _ => Vec::new(),
    }
}

fn specline_next(store: &Store, args: &Value) -> Result<Value, RpcError> {
    let project = resolve_project(store, &req_str(args, "project")?)?;

    // A milestone by name as well as by id: "what is next in Phase 8" is how the
    // question gets asked, and making the caller find a ULID first would mean
    // two calls to answer one question.
    let milestone = match opt_str(args, "milestone") {
        None => None,
        Some(raw) => Some(resolve_milestone(store, &project, &raw)?),
    };

    let filter = specline_core::ReadyFilter {
        unclaimed: opt_bool(args, "unclaimed"),
        labels: opt_str_list(args, "labels"),
        without_labels: opt_str_list(args, "without_labels"),
        milestone,
        limit: Some(opt_i64(args, "limit").unwrap_or(10).clamp(1, 100) as usize),
    };

    let ready =
        specline_core::ready(store, &project, &filter).map_err(|e| to_rpc_error(store, e))?;

    // The slug once, not once per row. `specline_next` is what a session calls to
    // decide what to work on, so these are the references it is about to say
    // out loud — which makes them the ones most worth being clickable.
    let ready_slug = match store.get(&project) {
        Ok(Some(Entity::Project(p))) => Some(p.slug),
        _ => None,
    };

    let summary = if ready.items.is_empty() {
        "Nothing is ready. Either everything open is blocked or waiting on a person — \
         `specline_context` says which — or the filters are narrower than the work."
            .to_owned()
    } else {
        let mut lines = vec![format!(
            "{} ready, best first:",
            if ready.truncated {
                format!("{} of {}", ready.items.len(), ready.total)
            } else {
                ready.total.to_string()
            }
        )];
        for c in &ready.items {
            lines.push(format!("- **{}** {} — {}", c.reference, c.title, c.why));
        }
        if ready.truncated {
            lines.push(format!(
                "\n{} more were ready and are not listed. Raise `limit` to see them.",
                ready.total - ready.items.len()
            ));
        }
        lines.push("\nClaim the one you pick with `specline_claim`.".to_owned());
        lines.join("\n")
    };

    Ok(tool_result(
        summary,
        json!({
            "ready": ready
                .items
                .iter()
                .map(|c| candidate_json(c, ready_slug.as_deref()))
                .collect::<Vec<_>>(),
            "total": ready.total,
            "truncated": ready.truncated,
        }),
    ))
}

/// One ranked candidate as JSON.
fn candidate_json(c: &specline_core::Candidate, slug: Option<&str>) -> Value {
    // Absent rather than null when there is no interface to point at. A `url`
    // key holding nothing is a field every caller has to test, and a model
    // reading one is being shown a link-shaped hole.
    let mut value = json!({
        "id": c.id.to_string(),
        "reference": c.reference,
        "title": c.title,
        "priority": c.priority,
        "unblocks": c.unblocks,
        "group": c.group.as_str(),
        "why": c.why,
    });

    if let (Some(obj), Some(url)) = (
        value.as_object_mut(),
        slug.zip(crate::links::interface())
            .map(|(slug, base)| crate::links::task(base, slug, &c.reference)),
    ) {
        obj.insert("url".to_owned(), json!(url));
    }

    value
}

/// Resolve a milestone by id or by name within one project.
fn resolve_milestone(store: &Store, project: &EntityId, raw: &str) -> Result<EntityId, RpcError> {
    if let Ok(id) = EntityId::parse_as(raw, EntityType::Milestone)
        && store.get(&id).ok().flatten().is_some()
    {
        return Ok(id);
    }

    let page = store
        .list(
            &EntityQuery::in_project(project.clone())
                .of_type(EntityType::Milestone)
                .limited(500),
        )
        .map_err(|e| to_rpc_error(store, e))?;

    let needle = raw.to_lowercase();
    // Prefix as well as exact, because the names here are "Phase 8 — The
    // working loop" and nobody types the dash and the subtitle.
    let matched = page.items.iter().find(|m| match m {
        Entity::Milestone(ms) => {
            let name = ms.name.to_lowercase();
            name == needle || name.starts_with(&needle)
        }
        _ => false,
    });

    match matched {
        Some(m) => Ok(m.id().clone()),
        None => Err(bad_arg(
            "milestone",
            &format!("no milestone in this project matches `{raw}`"),
            &format!(
                "one of: {}",
                page.items
                    .iter()
                    .map(|m| m.label().to_owned())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn specline_claim(store: &mut Store, args: &Value) -> Result<Value, RpcError> {
    let id = resolve_required(store, "id", &req_str(args, "id")?)?;
    let provenance = provenance_from(args)?;
    let force = opt_bool(args, "force");

    let claimed =
        specline_core::claim(store, &id, force, &provenance).map_err(|e| to_rpc_error(store, e))?;

    let reference =
        readable_ref(store, &Entity::Task(claimed.task.clone())).unwrap_or_else(|| id.to_string());
    let mut summary = format!("{reference} is yours — {}.", claimed.task.title);
    if let Some(previous) = &claimed.took_over_from {
        summary.push_str(&format!(
            " Taken over from session {previous}, whose claim had gone stale."
        ));
    }

    Ok(tool_result(
        summary,
        json!({
            "task": entity_json_linked(store, &Entity::Task(claimed.task)),
            "reference": reference,
            "took_over_from": claimed.took_over_from,
        }),
    ))
}

fn specline_close(store: &mut Store, args: &Value) -> Result<Value, RpcError> {
    let id = resolve_required(store, "id", &req_str(args, "id")?)?;
    let reason = specline_core::CloseReason::parse(&req_str(args, "reason")?)
        .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))?;
    // Closing is what you do to anything that is dealt with, not only to a task
    // (B-94). A signal takes the same verb, and `close_signal` translates the
    // reason into a triage outcome rather than reimplementing the invariants.
    let closing_a_signal = id.entity_type() == specline_core::EntityType::Feedback;

    let other = match opt_str(args, "other") {
        None => None,
        Some(raw) => {
            let resolved = resolve_required(store, "other", &raw)?;
            // `duplicate` and `superseded` draw an edge to whatever this names,
            // and nothing downstream checks it is a task. Closing a task as a
            // duplicate of a *spec* therefore succeeded, leaving a `duplicates`
            // edge between two things that cannot stand in that relation and a
            // close message that reads perfectly well.
            //
            // A signal duplicates a signal, for the same reason and with the
            // same consequence if it is not checked.
            let wanted = if closing_a_signal {
                specline_core::EntityType::Feedback
            } else {
                specline_core::EntityType::Task
            };
            if resolved.entity_type() != wanted {
                return Err(bad_arg(
                    "other",
                    &format!("`{raw}` is a {}, not a {wanted}", resolved.entity_type()),
                    if closing_a_signal {
                        "the signal that keeps the history — two people asking for the same thing"
                    } else {
                        "the task this one duplicates or was superseded by"
                    },
                ));
            }
            Some(resolved)
        }
    };
    let provenance = provenance_from(args)?;

    let request = specline_core::Close {
        reason,
        message: req_str(args, "message")?,
        evidence: opt_str_list(args, "evidence"),
        other,
    };

    if closing_a_signal {
        // Triage is part of the Inbox, and the Inbox is off by default until
        // the lifecycle is finished (KEEL-341). Refused rather than silently
        // doing nothing, and the refusal says how to switch it on — a caller
        // told only "no" would reasonably conclude signals cannot be closed.
        if !surfaces().inbox {
            return Err(bad_arg(
                "id",
                "the Inbox is switched off, so signals cannot be triaged",
                "a task id — or set SPECLINE_INBOX=1 to switch the Inbox on, which is off by \
                 default while the feature-request lifecycle is unfinished",
            ));
        }
        return close_a_signal(store, &id, &request, &provenance);
    }

    let closed = specline_core::close(store, &id, &request, &provenance)
        .map_err(|e| to_rpc_error(store, e))?;

    let reference =
        readable_ref(store, &Entity::Task(closed.task.clone())).unwrap_or_else(|| id.to_string());
    let mut summary = format!("{reference} closed as `{reason}` — {}.", closed.task.title);
    if let Some((rel, target)) = &closed.linked {
        summary.push_str(&format!(" Linked {rel} {target}."));
    }
    if !closed.task.evidence.is_empty() {
        summary.push_str(&format!(" Evidence: {}.", closed.task.evidence.join(", ")));
    }
    // Said rather than assumed. A close is the natural moment to record what was
    // learned, and the note stream is where the next session looks — but nothing
    // in a status transition can carry it.
    summary.push_str(
        "\n\nIf you found something the next session should know, put it on the row with \
         `specline_note`.",
    );

    Ok(tool_result(
        summary,
        json!({
            "task": entity_json_linked(store, &Entity::Task(closed.task)),
            "reference": reference,
            "linked": closed.linked.map(|(rel, to)| json!({
                "rel": rel.as_str(),
                "to": to.to_string(),
            })),
        }),
    ))
}

/// Close a signal — triage, reached through the verb that already exists.
///
/// Separate from the task path rather than woven into it, because almost
/// nothing is shared: a signal has no reference like `KEEL-42`, no status to
/// report, and the interesting result is what it *became* rather than what it
/// was. Sharing the argument parsing and splitting here keeps both readable.
fn close_a_signal(
    store: &mut Store,
    id: &specline_core::EntityId,
    request: &specline_core::Close,
    provenance: &specline_core::Provenance,
) -> Result<Value, RpcError> {
    let triaged = specline_core::work::close_signal(store, id, request, provenance)
        .map_err(|e| to_rpc_error(store, e))?;

    let mut summary = match (&request.reason, &triaged.linked) {
        (specline_core::CloseReason::Done, Some((_, feature))) => format!(
            "Signal picked up — “{}” became {feature}.",
            triaged.signal.summary
        ),
        (specline_core::CloseReason::Duplicate, Some((_, other))) => format!(
            "Signal marked a duplicate of {other} — “{}”. Two people wanting the same thing is \
             worth more than one, and the other row carries it.",
            triaged.signal.summary
        ),
        _ => format!(
            "Signal set down — “{}”. The argument is on the signal, so the same idea arriving \
             again finds the reasoning rather than silence.",
            triaged.signal.summary
        ),
    };
    // B-91: the reasoning lives on the signal by default and is promoted to a
    // numbered decision only when it is a no that binds future choices. Said
    // at the moment of setting down, because that is when somebody can tell
    // which kind of no it was.
    if request.reason == specline_core::CloseReason::WontDo {
        summary.push_str(
            "\n\nIf this is a no that constrains what gets built next, rather than just \
             \"not this\", record it as a decision too.",
        );
    }

    Ok(tool_result(
        summary,
        json!({
            "signal": entity_json_linked(store, &Entity::Feedback(triaged.signal)),
            "linked": triaged.linked.map(|(rel, to)| json!({
                "rel": rel.as_str(),
                "to": to.to_string(),
            })),
            "revision": triaged.revision.map(|d| json!({ "version": d.version })),
        }),
    ))
}

/// Which unfinished surfaces are switched on, read from the environment.
///
/// Resolved here rather than in `specline-core`, which never reads the
/// environment — that boundary is what makes every surface cheap to add, and a
/// config lookup inside the digest would be the first crack in it.
///
/// Read per call rather than cached. This is one process serving one user and
/// the cost is a `getenv`; caching it would mean a flag that needs a restart
/// to take effect, which for something switched on to try it out is exactly
/// the wrong trade.
pub fn surfaces() -> specline_core::digest::Surfaces {
    specline_core::digest::Surfaces {
        inbox: flag_is_on("SPECLINE_INBOX"),
    }
}

/// Whether an environment flag reads as on.
///
/// Deliberately narrow about what counts as yes. Anything else — including an
/// empty string, which is what `SPECLINE_INBOX=` gives you — is off, because a
/// half-finished surface appearing because a variable was set to something
/// unintended is the failure this exists to prevent.
fn flag_is_on(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref().map(str::trim),
        Ok("1" | "true" | "yes" | "on")
    )
}

/// The display label of a serialised entity.
fn label_of(entity: &Value) -> &str {
    for key in ["title", "name", "term", "summary"] {
        if let Some(v) = entity.get(key).and_then(Value::as_str) {
            return v;
        }
    }
    "(unnamed)"
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_url_safe() {
        assert_eq!(slugify("Specline"), "specline");
        assert_eq!(slugify("Harbour Billing"), "harbour-billing");
        assert_eq!(slugify("  Weird — Name!  "), "weird-name");
        assert_eq!(slugify("v1.0 Release"), "v1-0-release");
    }

    #[test]
    fn an_exact_slug_beats_a_substring() {
        let mut exact = Project::new("specline", "Specline");
        exact.aliases = vec!["spine".to_owned()];
        let partial = Project::new("specline-desktop", "Specline Desktop");

        assert!(match_score("specline", &exact) > match_score("specline", &partial));
        assert_eq!(match_score("spine", &exact), 110);
        assert_eq!(match_score("nothing-like-it", &exact), 0);
    }

    #[test]
    fn a_repo_url_matches() {
        let mut p = Project::new("harbour", "Harbour");
        p.repo_urls = vec!["https://github.com/kb/harbour".to_owned()];
        assert!(match_score("github.com/kb/harbour", &p) > 0);
    }

    #[test]
    fn missing_arguments_say_what_was_expected() {
        let err = req_str(&json!({}), "query").unwrap_err();
        assert!(err.message.contains("query"), "{}", err.message);
        assert!(err.message.contains("Expected"), "{}", err.message);
        assert_eq!(err.code, codes::INVALID_PARAMS);
    }

    #[test]
    fn a_malformed_timestamp_shows_the_shape_wanted() {
        let err = opt_time(&json!({"since": "yesterday"}), "since").unwrap_err();
        assert!(err.message.contains("RFC 3339"), "{}", err.message);
    }

    #[test]
    fn provenance_defaults_to_claude_but_keeps_a_supplied_session() {
        let p = provenance_from(&json!({"session_id": "ses_x", "surface": "chat"})).unwrap();
        assert_eq!(p.actor, Actor::Claude);
        assert_eq!(p.session_id.as_deref(), Some("ses_x"));
        assert_eq!(p.surface, Some(Surface::Chat));

        // No session is legal — refusing the write would be worse (D-10).
        let bare = provenance_from(&json!({})).unwrap();
        assert_eq!(bare.session_id, None);
        assert_eq!(bare.actor, Actor::Claude);
    }

    #[test]
    fn an_unknown_surface_is_rejected_with_the_valid_ones() {
        let err = provenance_from(&json!({"surface": "fax"})).unwrap_err();
        assert!(err.message.contains("chat"), "{}", err.message);
    }

    #[test]
    fn unknown_types_and_relations_are_rejected() {
        assert!(parse_types(Some(&json!(["task", "epic"]))).is_err());
        assert!(parse_types(Some(&json!(["task", "spec"]))).is_ok());
        assert!(parse_rels(Some(&json!(["relates_to"]))).is_err());
        assert!(parse_rels(Some(&json!(["implements"]))).is_ok());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod path_tests {
    use super::normalise_path;

    #[test]
    fn redundant_separators_do_not_make_two_paths_different() {
        // The live failure: a session reported `matched_project: null` for a
        // directory that had a project, because `cwd` carried a doubled
        // separator and the stored root_path did not. It started unoriented,
        // and only mentioned it in passing.
        assert_eq!(
            normalise_path("/tmp//specline-gate"),
            normalise_path("/tmp/specline-gate")
        );
        assert_eq!(normalise_path("/a///b//c"), "/a/b/c");
        assert_eq!(normalise_path("/a/b/"), "/a/b");
        assert_eq!(normalise_path("/a/b///"), "/a/b");
        // Root itself survives, rather than normalising to the empty string.
        assert_eq!(normalise_path("/"), "/");
    }

    #[test]
    fn a_sibling_with_a_shared_prefix_is_not_inside_the_project() {
        // `/a/bee` must not resolve to a project rooted at `/a/b`. The match
        // appends a separator for exactly this, and normalisation must not
        // undo it.
        let root = normalise_path("/a/b");
        let sibling = normalise_path("/a/bee");
        assert_ne!(sibling, root);
        assert!(!sibling.starts_with(&format!("{root}/")));

        let child = normalise_path("/a/b//c");
        assert!(child.starts_with(&format!("{root}/")));
    }
}
