//! `EntityStore` against SQLite: entity CRUD, links, events and notes.
//!
//! Twenty methods, and the interesting half of each of them is not about
//! SQLite at all — the idempotency ladder, the readable-number assignment
//! order, the `closed_at` rules, one event per changed field. That is domain
//! logic, and it came through the move off DuckDB unchanged. What the move
//! touched was statement text and parameter binding, and nothing else.
//!
//! # Where the schema and the structs disagree
//!
//! The `events` table names two columns differently from the struct: `op` for
//! the action and `at` for the timestamp. Both are aliased in
//! [`EVENT_COLUMNS`] rather than special-cased in the reader, so the reader
//! addresses every column by the name its struct field has.
//!
//! It also, for a while, had no `summary` and no `meta` column at all. That was
//! found here and fixed in migration 1 rather than documented and left, because
//! the consequence was not a missing feature: [`crate::Event`] carries both,
//! `render_status` renders the summary into the changelog and the daemon's
//! activity feed returns it, so every event written after the move would have
//! read back with an empty sentence. The changelog would have gone blank
//! quietly, for new history only, which is exactly the plausible-looking loss
//! this codebase keeps warning about. `meta` matters for a narrower reason:
//! `unlink` marks its event `{"removed": true}`, and without it an unlink is
//! indistinguishable from a link in the log.
//!
//! # Why every helper is a free function taking `&Connection`
//!
//! Two reasons, and the second is the load-bearing one.
//!
//! Sibling modules in `store` add their own inherent `impl Store` blocks, and
//! two modules defining a method of the same name on one type is a compile
//! error. The names a store's private helpers want — `count`, `query_one`,
//! `find_link` — are exactly the ones that would collide.
//!
//! And a helper that reaches for `store.connection()` itself can only ever
//! autocommit, which is why nothing above these was atomic. A
//! `rusqlite::Transaction` derefs to `Connection`, so a helper that takes
//! `&Connection` serves both an ordinary write and one bracketed by a
//! transaction, with the same statement text and no second copy to drift.
//! `insert_entity_row`, `append_event_inner`, `write_back`, `insert_link_row`,
//! `set_link_archived`, `archive_links_touching` and `insert_note_row` are that
//! substrate.

use super::Store;
use super::rows::{
    LINK_COLUMNS, col_err, from_row, get_ots, get_ts, insert_params, insert_stmt, ots, read_link,
    select_from, ts,
};
use crate::SessionClient;
use crate::store::patch::{apply_changes, is_status_change};
use crate::store::rows::{Col, spec_for};
use crate::store::{Created, EntityQuery, EntityStore, Page};
use crate::{
    Action, Actor, Audit, Cursor, Entity, EntityId, EntityType, Error, Event, EventId, Link,
    LinkId, NewEvent, NewLink, NewNote, Note, NoteId, ProjectScope, Provenance, Relation, Result,
    Surface,
};
use chrono::{DateTime, Utc};
use rusqlite::types::Value;
use rusqlite::{Connection, Row, params_from_iter};

/// The default cap on any list that does not specify one.
///
/// Not a silent truncation: every [`Page`] reports its total, so a caller that
/// hits this cap is told so — hard constraint 4.
pub const DEFAULT_LIST_LIMIT: usize = 200;

/// The event projection, named once so the three event queries cannot drift.
///
/// `op AS action` and `at AS created_at` are the only column renames in the
/// file: the schema chose different names for the same two things, and aliasing
/// them here means the row reader below can address every column by the name
/// its struct field has, with no rename to remember at three call sites.
const EVENT_COLUMNS: &str = "SELECT id, project_id, entity_type, entity_id, op AS action, field, \
                             before, after, summary, meta, actor, session_id, surface, \
                             at AS created_at";

/// The note projection, named once for the same reason.
const NOTE_COLUMNS: &str = "SELECT id, project_id, entity_type, entity_id, body, author, \
                            session_id, surface, created_at, archived_at";

// --- Binding helpers -----------------------------------------------------

/// Bind a string.
fn text(v: impl Into<String>) -> Value {
    Value::Text(v.into())
}

/// Bind an optional string.
fn otext(v: Option<impl Into<String>>) -> Value {
    v.map(|x| Value::Text(x.into())).unwrap_or(Value::Null)
}

/// Bind an id.
fn id_param(id: &EntityId) -> Value {
    Value::Text(id.as_str().to_owned())
}

/// Bind an optional JSON value as its text form.
fn json_param(v: Option<&serde_json::Value>) -> Value {
    v.map(|j| Value::Text(j.to_string())).unwrap_or(Value::Null)
}

// --- Reading helpers -----------------------------------------------------

/// Wrap a read failure with the column and table that caused it.
/// Read an optional id column, treating an empty string as absent.
///
/// Deliberately *not* `rows::get_oid`, which is the strict one and is right for
/// the thirteen tables: there an id column either holds an id or holds NULL.
/// This is for `v_entities`, which spells "no project" as `''` for a global
/// term, because the index that lets a global and a project term coexist
/// coalesces the same way (Q-4). `EntityId::parse("")` is an error rather than
/// the "no project" it means, so the strict reader would reject a row that is
/// correct.
///
/// The name says `_or_empty` because the two used to both be called `get_oid`,
/// in sibling modules, differing only in whether they accepted `''` — which is
/// the kind of pair that gets "tidied" into one by someone who sees the
/// duplication and not the reason for it.
fn get_oid_or_empty(
    row: &Row<'_>,
    table: &'static str,
    col: &'static str,
) -> Result<Option<EntityId>> {
    match row
        .get::<_, Option<String>>(col)
        .map_err(col_err(table, col))?
    {
        Some(raw) if !raw.is_empty() => Ok(Some(EntityId::parse(&raw)?)),
        _ => Ok(None),
    }
}

/// The seventeen link params, in insert order.
fn link_params(l: &Link) -> Vec<Value> {
    vec![
        text(l.id.as_str()),
        l.project_id.as_ref().map(id_param).unwrap_or(Value::Null),
        text(l.from_type.as_str()),
        id_param(&l.from_id),
        text(l.rel.as_str()),
        text(l.to_type.as_str()),
        id_param(&l.to_id),
        text(l.anchor.clone()),
        otext(l.note.clone()),
        ts(l.audit.created_at),
        ts(l.audit.updated_at),
        Value::Integer(i64::from(l.audit.version)),
        text(l.audit.created_by.as_str()),
        text(l.audit.updated_by.as_str()),
        otext(l.audit.session_id.clone()),
        otext(l.audit.surface.map(|s| s.as_str())),
        ots(l.audit.archived_at),
    ]
}

/// Rebuild an event from a row selected through [`EVENT_COLUMNS`].
///
/// `summary` is `NOT NULL DEFAULT ''`, so an event copied in without one reads
/// back as an empty sentence rather than failing the read. History that predates
/// the column is worth less than history that cannot be read at all.
fn read_event(row: &Row<'_>) -> Result<Event> {
    let json = |v: Option<String>| v.and_then(|s| serde_json::from_str(&s).ok());
    Ok(Event {
        id: EventId::parse(
            &row.get::<_, String>("id")
                .map_err(col_err("events", "id"))?,
        )?,
        project_id: get_oid_or_empty(row, "events", "project_id")?,
        entity_type: EntityType::parse(
            &row.get::<_, String>("entity_type")
                .map_err(col_err("events", "entity_type"))?,
        )?,
        entity_id: EntityId::parse(
            &row.get::<_, String>("entity_id")
                .map_err(col_err("events", "entity_id"))?,
        )?,
        action: Action::parse(
            &row.get::<_, String>("action")
                .map_err(col_err("events", "op"))?,
        )?,
        field: row
            .get::<_, Option<String>>("field")
            .map_err(col_err("events", "field"))?,
        before: json(
            row.get::<_, Option<String>>("before")
                .map_err(col_err("events", "before"))?,
        ),
        after: json(
            row.get::<_, Option<String>>("after")
                .map_err(col_err("events", "after"))?,
        ),
        actor: Actor::parse(
            &row.get::<_, String>("actor")
                .map_err(col_err("events", "actor"))?,
        )?,
        session_id: row
            .get::<_, Option<String>>("session_id")
            .map_err(col_err("events", "session_id"))?,
        surface: match row
            .get::<_, Option<String>>("surface")
            .map_err(col_err("events", "surface"))?
        {
            Some(s) => Some(Surface::parse(&s)?),
            None => None,
        },
        summary: row
            .get::<_, String>("summary")
            .map_err(col_err("events", "summary"))?,
        meta: json(
            row.get::<_, Option<String>>("meta")
                .map_err(col_err("events", "meta"))?,
        ),
        created_at: get_ts(row, "events", "created_at")?,
    })
}

/// Rebuild a note from a row selected through [`NOTE_COLUMNS`].
fn read_note(row: &Row<'_>) -> Result<Note> {
    Ok(Note {
        id: NoteId::parse(&row.get::<_, String>("id").map_err(col_err("notes", "id"))?)?,
        project_id: get_oid_or_empty(row, "notes", "project_id")?,
        entity_type: EntityType::parse(
            &row.get::<_, String>("entity_type")
                .map_err(col_err("notes", "entity_type"))?,
        )?,
        entity_id: EntityId::parse(
            &row.get::<_, String>("entity_id")
                .map_err(col_err("notes", "entity_id"))?,
        )?,
        body: row
            .get::<_, String>("body")
            .map_err(col_err("notes", "body"))?,
        author: Actor::parse(
            &row.get::<_, String>("author")
                .map_err(col_err("notes", "author"))?,
        )?,
        session_id: row
            .get::<_, Option<String>>("session_id")
            .map_err(col_err("notes", "session_id"))?,
        surface: match row
            .get::<_, Option<String>>("surface")
            .map_err(col_err("notes", "surface"))?
        {
            Some(s) => Some(Surface::parse(&s)?),
            None => None,
        },
        created_at: get_ts(row, "notes", "created_at")?,
        archived_at: get_ots(row, "notes", "archived_at")?,
    })
}

// --- Validation ----------------------------------------------------------

/// Per-type validation, applied on the way in on both the create and update
/// paths.
///
/// Here rather than in the MCP layer so the CLI, the daemon and `specline import`
/// cannot disagree about what is storable. The two surfaces having their own
/// opinion of a valid row is how a rule becomes a convention.
fn validate_entity(entity: &Entity) -> Result<()> {
    // Every path a caller can set, checked before it reaches storage.
    //
    // These four columns are the only place in Specline where a stored value names
    // a file that Specline will later write. They arrive from a model that can be
    // prompt-injected, and `POST /api/generate` acts on them unattended — see
    // `crate::safe_path` for what that buys an attacker, and why the same check
    // runs again at every join.
    let entity_type = entity.entity_type();
    if let Some(path) = entity.mirror_path() {
        crate::safe_path::validate_repo_relative(entity_type, "mirror_path", path)?;
    }
    if let Entity::Project(p) = entity {
        if let Some(path) = p.status_path.as_deref() {
            crate::safe_path::validate_repo_relative(entity_type, "status_path", path)?;
        }
        if let Some(path) = p.decisions_path.as_deref() {
            crate::safe_path::validate_repo_relative(entity_type, "decisions_path", path)?;
        }
        if let Some(path) = p.root_path.as_deref() {
            crate::safe_path::validate_root_path(path)?;
        }
    }

    match entity {
        Entity::Milestone(m) => m.validate(),
        // A project calling milestones "tasks" would make every
        // `specline_create(type: "task")` ambiguous, and the resolution order hides
        // that rather than surfacing it — the canonical name wins, so the noun
        // silently does nothing. Refused where it is set instead.
        Entity::Project(p) => match p.milestone_noun.as_deref() {
            Some(noun) => crate::vocabulary::validate_milestone_noun(noun),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

/// Validation that applies when a row is *created* and not when it is changed.
///
/// The asymmetry is deliberate. A task summary is required (TQ-34), but
/// ninety-four rows predate the rule; running the check on update as well would
/// mean none of those rows could ever be touched again — moving one to `done`
/// would be refused for a summary nobody was being asked to write. The
/// requirement belongs where it can be met, and `specline lint` reports whatever
/// falls through the hole that leaves.
fn validate_on_create(entity: &Entity) -> Result<()> {
    match entity {
        Entity::Task(t) => {
            t.validate_summary()?;
            // The same rule the update path enforces on the transition into a
            // terminal status, applied here because a create *is not* a
            // transition and so slipped past it. KEEL-216 was filed with
            // `status: done` and landed closed with no reason, no message, no
            // evidence and no `closed_at` — a row that reads as finished and
            // says nothing about why, which is precisely what the rule exists
            // to prevent.
            //
            // Holding a create to the rule rather than refusing one outright
            // keeps the two backfills in this repository legal: `specline
            // bootstrap` and `specline fixture` both seed rows that were already
            // closed when they were written, and adopting a finished backlog
            // is the same shape. What they now have to supply is what any
            // other close supplies.
            if t.status.is_terminal() {
                t.validate_close()?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

// --- Query helpers -------------------------------------------------------

/// Run a query expected to yield at most one entity.
fn query_one(
    conn: &Connection,
    entity_type: EntityType,
    sql: &str,
    params: Vec<Value>,
) -> Result<Option<Entity>> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(Error::storage(format!("prepare a {entity_type} lookup")))?;
    let mut rows = stmt
        .query(params_from_iter(params))
        .map_err(Error::storage(format!("run a {entity_type} lookup")))?;
    match rows
        .next()
        .map_err(Error::storage(format!("read a {entity_type} row")))?
    {
        Some(row) => Ok(Some(from_row(entity_type, row)?)),
        None => Ok(None),
    }
}

/// Run a query yielding many entities of one type.
fn query_many(
    conn: &Connection,
    entity_type: EntityType,
    sql: &str,
    params: Vec<Value>,
) -> Result<Vec<Entity>> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(Error::storage(format!("prepare a {entity_type} list")))?;
    let mut rows = stmt
        .query(params_from_iter(params))
        .map_err(Error::storage(format!("run a {entity_type} list")))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(Error::storage(format!("read a {entity_type} row")))?
    {
        out.push(from_row(entity_type, row)?);
    }
    Ok(out)
}

/// Count the rows a predicate matches.
fn count(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<usize> {
    let n: i64 = conn
        .query_row(sql, params_from_iter(params), |r| r.get(0))
        .map_err(Error::storage("count matching rows"))?;
    Ok(n.max(0) as usize)
}

/// The columns [`read_session_client`] expects, in one place so a query and its
/// reader cannot drift apart.
const SESSION_CLIENT_COLUMNS: &str = "SELECT session_id, client_name, client_title, \
                                      client_version, first_seen, last_seen";

/// Read one `session_clients` row.
fn read_session_client(row: &Row<'_>) -> Result<SessionClient> {
    Ok(SessionClient {
        session_id: row
            .get::<_, String>("session_id")
            .map_err(col_err("session_clients", "session_id"))?,
        client: crate::Client {
            name: row
                .get::<_, String>("client_name")
                .map_err(col_err("session_clients", "client_name"))?,
            title: row
                .get::<_, Option<String>>("client_title")
                .map_err(col_err("session_clients", "client_title"))?,
            version: row
                .get::<_, Option<String>>("client_version")
                .map_err(col_err("session_clients", "client_version"))?,
        },
        first_seen: get_ts(row, "session_clients", "first_seen")?,
        last_seen: get_ts(row, "session_clients", "last_seen")?,
    })
}

/// Run a session-client query and read the rows.
fn query_session_clients(
    conn: &Connection,
    sql: &str,
    params: Vec<Value>,
) -> Result<Vec<SessionClient>> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(Error::storage("prepare a session client query"))?;
    let mut rows = stmt
        .query(params_from_iter(params))
        .map_err(Error::storage("run a session client query"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(Error::storage("read a session client row"))?
    {
        out.push(read_session_client(row)?);
    }
    Ok(out)
}

/// Run a note query and read the rows.
fn query_notes(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<Vec<Note>> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(Error::storage("prepare a note query"))?;
    let mut rows = stmt
        .query(params_from_iter(params))
        .map_err(Error::storage("run a note query"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(Error::storage("read a note row"))? {
        out.push(read_note(row)?);
    }
    Ok(out)
}

/// Run an event query and read the rows.
fn query_events(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<Vec<Event>> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(Error::storage("prepare an event query"))?;
    let mut rows = stmt
        .query(params_from_iter(params))
        .map_err(Error::storage("run an event query"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(Error::storage("read an event row"))? {
        out.push(read_event(row)?);
    }
    Ok(out)
}

/// Find an existing entity by idempotency key, honouring each type's
/// uniqueness scope.
fn find_by_key(
    conn: &Connection,
    entity_type: EntityType,
    project_id: Option<&EntityId>,
    key: &str,
) -> Result<Option<Entity>> {
    let spec = spec_for(entity_type);
    let (predicate, params): (&str, Vec<Value>) = match entity_type.project_scope() {
        // Projects are globally unique on their key — they have no parent to
        // scope by.
        ProjectScope::IsTheProject => ("idempotency_key = ?", vec![text(key)]),
        // A global term and a project-scoped one of the same name must be able
        // to coexist (Q-4), so the COALESCE here mirrors the index.
        ProjectScope::Optional => (
            "COALESCE(project_id, '') = ? AND idempotency_key = ?",
            vec![
                text(project_id.map(EntityId::as_str).unwrap_or("")),
                text(key),
            ],
        ),
        ProjectScope::Required => (
            "project_id = ? AND idempotency_key = ?",
            vec![
                text(project_id.map(EntityId::as_str).unwrap_or("")),
                text(key),
            ],
        ),
    };
    let sql = format!("{} WHERE {predicate}", select_from(&spec));
    query_one(conn, entity_type, &sql, params)
}

/// Read one entity by id through whatever connection is handed in.
///
/// The `&Connection` twin of [`EntityStore::get`], so a helper running inside a
/// transaction reads what that transaction has written rather than what the
/// store looked like before it opened.
fn get_entity(conn: &Connection, id: &EntityId) -> Result<Option<Entity>> {
    let entity_type = id.entity_type();
    let sql = format!("{} WHERE id = ?", select_from(&spec_for(entity_type)));
    query_one(conn, entity_type, &sql, vec![id_param(id)])
}

/// A live entity of this type in this project whose label means the same thing
/// as `label`.
///
/// Only ever consulted on create, and only after the exact key has missed.
/// Archived rows are excluded here — unlike the exact-key path, which
/// deliberately matches them: reviving an archived row on a *fuzzy* match would
/// resurrect something a human chose to put away.
fn find_by_similar_label(
    store: &Store,
    entity_type: EntityType,
    project_id: Option<&EntityId>,
    label: &str,
) -> Result<Option<Entity>> {
    // Three types are excluded, each for its own reason:
    //
    // - `metric_observation`: its label is a note, and two readings of one
    //   metric are emphatically not the same row.
    // - `artifact`: named by URL or filename, where a near-match is a
    //   different file.
    // - `term`: a glossary entry's name *is* its identity, and Q-4 requires a
    //   global term and a project-scoped one of the same name to coexist. The
    //   COALESCE index already expresses that exactly; guessing on top of it
    //   can only be wrong.
    if matches!(
        entity_type,
        EntityType::MetricObservation | EntityType::Artifact | EntityType::Term
    ) {
        return Ok(None);
    }

    let mut query = EntityQuery::default().of_type(entity_type).limited(2_000);
    if let Some(p) = project_id {
        query = EntityQuery::in_project(p.clone())
            .of_type(entity_type)
            .limited(2_000);
    }
    let page = store.list(&query)?;

    let mut best: Option<(f64, Entity)> = None;
    for candidate in page.items {
        if !crate::types::same_thing(candidate.label(), label) {
            continue;
        }
        let score = crate::types::title_similarity(candidate.label(), label);
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, candidate));
        }
    }
    Ok(best.map(|(_, e)| e))
}

/// The next readable number for `table` within a project.
///
/// `MAX + 1` over every row including archived ones. A number is never reused:
/// `KEEL-42` must mean the same task forever, and handing it to a second task
/// after the first was archived is the one way to make a readable identifier
/// actively misleading. The same holds for `B-12`, with the extra sting that
/// decision numbers are cited in prose that nothing rewrites.
///
/// `table` is never caller-supplied — it comes from this module's own match on
/// entity type, which is why interpolating it is not an injection.
fn next_number_in(conn: &Connection, table: &str, project_id: &EntityId) -> Result<i32> {
    let next = count(
        conn,
        &format!("SELECT COALESCE(MAX(number), 0) + 1 FROM {table} WHERE project_id = ?"),
        vec![id_param(project_id)],
    )?;
    Ok(next as i32)
}

/// Write the full row back, under optimistic concurrency.
///
/// The `WHERE version = ?` is what actually enforces REQ-7. Checking the
/// version in Rust and then updating unconditionally would leave a window
/// between the two in which another writer could land — the exact race the
/// requirement exists to close, and the reason this asserts on the number of
/// rows the statement changed rather than trusting the earlier read.
fn write_back(conn: &Connection, entity: &Entity, expected_version: i32) -> Result<()> {
    let entity_type = entity.entity_type();
    let spec = spec_for(entity_type);
    let assignments: Vec<String> = spec
        .cols
        .iter()
        .skip(1) // never reassign `id`
        .map(|c| match c {
            // Both variants are a plain assignment under SQLite: a list column
            // already holds JSON text, so there is nothing for the engine to
            // convert on the way in.
            Col::Plain(n) | Col::Array(n) => format!("{n} = ?"),
        })
        .chain(
            [
                "created_at = ?",
                "updated_at = ?",
                "version = ?",
                "created_by = ?",
                "updated_by = ?",
                "session_id = ?",
                "surface = ?",
                "archived_at = ?",
            ]
            .iter()
            .map(|s| (*s).to_owned()),
        )
        .collect();

    let sql = format!(
        "UPDATE {} SET {} WHERE id = ? AND version = ?",
        spec.table,
        assignments.join(", ")
    );

    // `insert_params` yields id first; drop it and re-append it for the WHERE
    // clause so the SET list and the parameter list stay aligned.
    let mut params = insert_params(entity);
    params.remove(0);
    params.push(id_param(entity.id()));
    params.push(Value::Integer(i64::from(expected_version)));

    let affected = conn
        .execute(&sql, params_from_iter(params))
        .map_err(Error::storage(format!("update {}", entity.id())))?;

    if affected == 0 {
        // Either the row moved under us or it never existed. Re-read to tell
        // the caller which, because "stale" and "missing" need different
        // responses from an agent.
        return match get_entity(conn, entity.id())? {
            Some(current) => Err(Error::StaleVersion {
                entity_type,
                id: entity.id().to_string(),
                supplied: expected_version,
                latest: current.audit().version,
            }),
            None => Err(Error::NotFound {
                entity_type,
                id: entity.id().to_string(),
            }),
        };
    }
    Ok(())
}

/// Insert an entity's row through whatever connection is handed in.
///
/// One of the write primitives that take a `&Connection` rather than a `&Store`
/// (KEEL-140). A `rusqlite::Transaction` derefs to `Connection`, so the same
/// function serves an autocommitting write and one bracketed by a transaction —
/// which is the whole reason anything above it can be made atomic.
fn insert_entity_row(conn: &Connection, entity: &Entity) -> Result<()> {
    let entity_type = entity.entity_type();
    let spec = spec_for(entity_type);
    conn.execute(&insert_stmt(&spec), params_from_iter(insert_params(entity)))
        .map_err(Error::storage(format!(
            "create the {entity_type} `{}`",
            entity.label()
        )))?;
    Ok(())
}

/// Insert a link row.
fn insert_link_row(conn: &Connection, link: &Link) -> Result<()> {
    conn.execute(
        "INSERT INTO links (id, project_id, from_type, from_id, rel, to_type, to_id, \
         anchor, note, created_at, updated_at, version, created_by, updated_by, \
         session_id, surface, archived_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params_from_iter(link_params(link)),
    )
    .map_err(Error::storage(format!(
        "create the link {} {} {}",
        link.from_id, link.rel, link.to_id
    )))?;
    Ok(())
}

/// Set or clear a link's `archived_at`. Soft delete only, hard constraint 3.
fn set_link_archived(
    conn: &Connection,
    id: &LinkId,
    archived: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    provenance: &Provenance,
) -> Result<()> {
    conn.execute(
        "UPDATE links SET archived_at = ?, updated_at = ?, version = version + 1, \
         updated_by = ? WHERE id = ?",
        params_from_iter(vec![
            ots(archived),
            ts(now),
            text(provenance.actor.as_str()),
            text(id.as_str()),
        ]),
    )
    .map_err(Error::storage(format!("archive the link {id}")))?;
    Ok(())
}

/// Archive every live link touching an entity, in one statement.
fn archive_links_touching(
    conn: &Connection,
    id: &EntityId,
    now: DateTime<Utc>,
    provenance: &Provenance,
) -> Result<()> {
    conn.execute(
        "UPDATE links SET archived_at = ?, updated_at = ?, version = version + 1, \
         updated_by = ? WHERE (from_id = ? OR to_id = ?) AND archived_at IS NULL",
        params_from_iter(vec![
            ts(now),
            ts(now),
            text(provenance.actor.as_str()),
            id_param(id),
            id_param(id),
        ]),
    )
    .map_err(Error::storage(format!("archive the links touching {id}")))?;
    Ok(())
}

/// Insert a note row.
fn insert_note_row(conn: &Connection, note: &Note) -> Result<()> {
    conn.execute(
        "INSERT INTO notes (id, project_id, entity_type, entity_id, body, author, \
         session_id, surface, created_at, archived_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
        params_from_iter(vec![
            text(note.id.as_str()),
            note.project_id
                .as_ref()
                .map(id_param)
                .unwrap_or(Value::Null),
            text(note.entity_type.as_str()),
            id_param(&note.entity_id),
            text(note.body.clone()),
            text(note.author.as_str()),
            otext(note.session_id.clone()),
            otext(note.surface.map(|s| s.as_str())),
            ts(note.created_at),
        ]),
    )
    .map_err(Error::storage(format!(
        "append a note to {}",
        note.entity_id
    )))?;
    Ok(())
}

/// Append an event at a caller-chosen instant.
///
/// Separate from the trait method so that a create and the event describing it
/// carry the same timestamp rather than two instants a microsecond apart, and
/// taking a `&Connection` so it can be the second statement of a transaction
/// rather than a separate autocommitting write that a crash can lose.
///
/// `pub(super)` because `docs::write_revision` needs it too: a revision is a
/// mutation like any other and has to appear in the changelog, and its
/// transaction is already open by the time it wants one.
pub(super) fn append_event_inner(
    conn: &Connection,
    event: NewEvent,
    provenance: &Provenance,
    now: DateTime<Utc>,
) -> Result<Event> {
    let stored = Event {
        id: EventId::generate(),
        project_id: event.project_id,
        entity_type: event.entity_id.entity_type(),
        entity_id: event.entity_id,
        action: event.action,
        field: event.field,
        before: event.before,
        after: event.after,
        actor: provenance.actor,
        session_id: provenance.session_id.clone(),
        surface: provenance.surface,
        summary: event.summary,
        meta: event.meta,
        created_at: now,
    };

    let params: Vec<Value> = vec![
        text(stored.id.as_str()),
        stored
            .project_id
            .as_ref()
            .map(id_param)
            .unwrap_or(Value::Null),
        id_param(&stored.entity_id),
        text(stored.entity_type.as_str()),
        otext(stored.field.clone()),
        text(stored.action.as_str()),
        json_param(stored.before.as_ref()),
        json_param(stored.after.as_ref()),
        text(&stored.summary),
        json_param(stored.meta.as_ref()),
        text(stored.actor.as_str()),
        otext(stored.session_id.clone()),
        otext(stored.surface.map(|s| s.as_str())),
        ts(stored.created_at),
    ];

    conn.execute(
        "INSERT INTO events (id, project_id, entity_id, entity_type, field, op, \
         before, after, summary, meta, actor, session_id, surface, at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params_from_iter(params),
    )
    .map_err(Error::storage(format!(
        "append a `{}` event for {}",
        stored.action, stored.entity_id
    )))?;

    record_session_client(conn, provenance, now)?;

    Ok(stored)
}

/// Note which program this session is being driven by (KEEL-360).
///
/// Here rather than at each call site because this function is already the one
/// place every mutation passes through — a create, an update, an archive and a
/// document revision all land on it, in the transaction that is writing them.
/// Anywhere else and the answer would be right for whichever writes somebody
/// remembered.
///
/// Needs both halves to say anything. No `session_id` and there is nothing to
/// key on; no client and there is nothing to record, which is every write from
/// the CLI and from the interface, and both of those are adequately described
/// by their surface already.
///
/// `last_seen` moves on every write and `first_seen` does not, so the row says
/// when a conversation started as well as when it was last heard from. The
/// name and version are refreshed too: a client that updates mid-session should
/// read as the version now running rather than the one it opened with.
fn record_session_client(
    conn: &Connection,
    provenance: &Provenance,
    now: DateTime<Utc>,
) -> Result<()> {
    let (Some(session_id), Some(client)) = (&provenance.session_id, &provenance.client) else {
        return Ok(());
    };

    conn.execute(
        "INSERT INTO session_clients \
           (session_id, client_name, client_title, client_version, first_seen, last_seen) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(session_id) DO UPDATE SET \
           client_name = excluded.client_name, \
           client_title = excluded.client_title, \
           client_version = excluded.client_version, \
           last_seen = excluded.last_seen",
        params_from_iter(vec![
            text(session_id),
            text(&client.name),
            otext(client.title.clone()),
            otext(client.version.clone()),
            ts(now),
            ts(now),
        ]),
    )
    .map_err(Error::storage(format!(
        "record the client for session {session_id}"
    )))?;

    Ok(())
}

/// Resolve the project an entity belongs to, for event tagging.
fn project_of(entity: &Entity) -> Option<EntityId> {
    match entity {
        // A project's own events are tagged with itself, so that "everything
        // that happened in project X" includes X's creation.
        Entity::Project(p) => Some(p.id.clone()),
        other => other.project_id().cloned(),
    }
}

/// Check that a link's endpoint exists and is not archived.
///
/// This is the foreign key the schema cannot declare: `links` is polymorphic
/// across thirteen tables (SPEC §3.1). Skipping it would let a typo create an
/// edge to nothing, which a traversal would then silently drop.
fn require_live(conn: &Connection, id: &EntityId, role: &str) -> Result<Entity> {
    match get_entity(conn, id)? {
        None => Err(Error::Invariant {
            operation: format!("link {role} {id}"),
            problem: format!("no {} exists with id {id}", id.entity_type()),
        }),
        Some(e) if e.audit().is_archived() => Err(Error::Invariant {
            operation: format!("link {role} {id}"),
            problem: format!("{id} is archived; restore it before linking, or link a live entity"),
        }),
        Some(e) => Ok(e),
    }
}

/// Resolve any id to its type, project and archived state in one query.
///
/// This is what `v_entities` is for: the caller has an id and no idea which of
/// thirteen tables it lives in, and a `match` over all thirteen would have to be
/// updated every time a type is added.
fn resolve_vertex(
    conn: &Connection,
    id: &EntityId,
) -> Result<Option<(EntityType, Option<EntityId>, bool)>> {
    let mut stmt = conn
        .prepare("SELECT entity_type, project_id, archived_at FROM v_entities WHERE id = ? LIMIT 1")
        .map_err(Error::storage("prepare a vertex lookup"))?;
    let mut rows = stmt
        .query(params_from_iter(vec![id_param(id)]))
        .map_err(Error::storage("run a vertex lookup"))?;
    let Some(row) = rows.next().map_err(Error::storage("read a vertex row"))? else {
        return Ok(None);
    };
    let entity_type = EntityType::parse(
        &row.get::<_, String>("entity_type")
            .map_err(col_err("v_entities", "entity_type"))?,
    )?;
    let project_id = get_oid_or_empty(row, "v_entities", "project_id")?;
    let archived = get_ots(row, "v_entities", "archived_at")?.is_some();
    Ok(Some((entity_type, project_id, archived)))
}

/// Find one edge by its unique key.
fn find_link(
    conn: &Connection,
    from_id: &EntityId,
    rel: Relation,
    to_id: &EntityId,
    anchor: &str,
    include_archived: bool,
) -> Result<Option<Link>> {
    let archived = if include_archived {
        ""
    } else {
        " AND archived_at IS NULL"
    };
    let sql = format!(
        "{LINK_COLUMNS} WHERE from_id = ? AND rel = ? AND to_id = ? AND anchor = ?{archived}"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(Error::storage("prepare a link lookup"))?;
    let mut rows = stmt
        .query(params_from_iter(vec![
            id_param(from_id),
            text(rel.as_str()),
            id_param(to_id),
            text(anchor),
        ]))
        .map_err(Error::storage("run a link lookup"))?;
    match rows.next().map_err(Error::storage("read a link row"))? {
        Some(row) => Ok(Some(read_link(row)?)),
        None => Ok(None),
    }
}

/// Shorten a label for a one-line summary, on a word boundary where possible.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    match cut.rsplit_once(' ') {
        Some((head, _)) if head.len() > max / 2 => format!("{head}…"),
        _ => format!("{cut}…"),
    }
}

/// Render a JSON value for an event summary, without the quotes a raw
/// `to_string` would add around every string.
/// How a field's old and new values appear in an event summary.
///
/// The rule lives in [`crate::event::render_field_value`] because the changelog
/// renderer applies it a second time, to events written before it existed. One
/// definition, two call sites — a rule that exists twice is a rule that will
/// differ once.
use crate::event::render_field_value as render;

/// What a create turns out to be, once the store has been consulted.
///
/// Split out of `create` so the composite create — an entity, its first
/// revision and an image, all in one transaction — can reach the same
/// validation, idempotency and numbering rules without a second copy of them.
/// A rule that exists twice is a rule that will differ once.
pub(super) enum Prepared {
    /// Something with this key, or a title that means the same thing, is
    /// already here. Nothing is written.
    Existing(Entity),
    /// A genuinely new row, numbered and stamped, ready to insert.
    Fresh(Entity),
}

impl Store {
    /// Everything `create` does before it opens a transaction.
    pub(super) fn prepare_create(
        &self,
        mut entity: Entity,
        provenance: &Provenance,
        now: DateTime<Utc>,
    ) -> Result<Prepared> {
        // Before the idempotency lookup, so a bad write is refused rather than
        // quietly matching an existing row and reporting success.
        validate_entity(&entity)?;
        validate_on_create(&entity)?;

        let entity_type = entity.entity_type();
        let project_id = entity.project_id().cloned();

        if let Some(existing) = find_by_key(
            self.connection(),
            entity_type,
            project_id.as_ref(),
            entity.idempotency_key(),
        )? {
            // SPEC §7.2: a repeat call returns the existing entity rather than
            // erroring, so a retrying agent gets a sane result. Note this also
            // returns *archived* matches — deliberately, because silently
            // minting a second row alongside an archived one is how a store
            // fills up with near-duplicates.
            return Ok(Prepared::Existing(existing));
        }

        // A near-miss on the title is the same failure the key exists to
        // prevent, one step less exact. The key is a hash, so it cannot see
        // that two titles describe one thing; this can.
        //
        // Unless the caller supplied their own key. That is them saying "these
        // are different things that happen to share a title" — two `Deploy`
        // tasks keyed `deploy-staging` and `deploy-production` — and guessing
        // over an explicit assertion is exactly the false merge that hides
        // work. A derived key carries no such claim, so only then do we look.
        let key_was_derived = entity.idempotency_key()
            == crate::types::derive_idempotency_key(
                project_id.as_ref(),
                entity_type,
                entity.natural_key(),
            );
        if key_was_derived
            && let Some(existing) =
                find_by_similar_label(self, entity_type, project_id.as_ref(), entity.label())?
        {
            tracing::info!(
                existing = %existing.label(),
                attempted = %entity.label(),
                "returning an existing {entity_type} with a near-identical title rather than \
                 creating a second row"
            );
            return Ok(Prepared::Existing(existing));
        }

        // Readable identifiers are assigned here rather than in the
        // constructors, because both need to know about rows the constructor
        // cannot see: whether a key is taken, and what the last number was.
        // Assigning after the idempotency checks matters — a create that turns
        // out to be a repeat must not burn a number, or the sequence develops
        // gaps that look like deleted work.
        match &mut entity {
            Entity::Project(p) if p.key.is_empty() => {
                p.key = self.unique_project_key(&crate::types::derive_project_key(&p.slug))?;
            }
            Entity::Project(p) => {
                p.key = self.unique_project_key(&p.key.to_uppercase())?;
            }
            Entity::Task(t) if t.number == 0 => {
                t.number = self.next_task_number(&t.project_id)?;
                if t.rank == 0.0 {
                    t.rank = self.next_task_rank(&t.project_id)?;
                }
            }
            Entity::Decision(d) if d.number == 0 => {
                d.number = next_number_in(self.connection(), "decisions", &d.project_id)?;
            }
            _ => {}
        }
        if let Entity::Task(t) = &entity {
            self.check_task_parent(t)?;
        }

        // A row that arrives closed gets the same two stamps the update path
        // applies on the way in, or a create is a second definition of what a
        // closed task looks like. `closed_at` matters most: without it the row
        // is invisible to everything asking what was closed and when, which is
        // the changelog and every "what shipped this week" query. A caller
        // supplying its own — a backfill, which knows the real date — keeps it.
        if let Entity::Task(t) = &mut entity
            && t.status.is_terminal()
        {
            t.closed_at = t.closed_at.or(Some(now));
            t.claimed_by = None;
            t.claimed_at = None;
        }

        *entity.audit_mut() = Audit::new(provenance, now);
        Ok(Prepared::Fresh(entity))
    }
}

/// Insert a prepared entity and the event that records it.
///
/// The two statements every create shares, so the composite create adds a
/// revision and a blob to *this* rather than reimplementing it.
pub(super) fn insert_created(
    conn: &Connection,
    entity: &Entity,
    provenance: &Provenance,
    now: DateTime<Utc>,
) -> Result<()> {
    let entity_type = entity.entity_type();
    insert_entity_row(conn, entity)?;
    let summary = format!("created {entity_type} “{}”", entity.label());
    append_event_inner(
        conn,
        NewEvent::new(entity.id().clone(), Action::Created, summary).in_project(project_of(entity)),
        provenance,
        now,
    )?;
    Ok(())
}

impl EntityStore for Store {
    fn create(&mut self, entity: Entity, provenance: &Provenance) -> Result<Created> {
        let now = Utc::now();
        let entity = match self.prepare_create(entity, provenance, now)? {
            Prepared::Existing(existing) => {
                return Ok(Created {
                    entity: existing,
                    created: false,
                });
            }
            Prepared::Fresh(entity) => entity,
        };

        // The row and the event that records it commit together or not at all.
        //
        // This is not a theoretical crash window. The idempotent retry returns
        // `created: false` *before* re-writing anything, so a row that landed
        // without its event never gets one on a second attempt: the history is
        // gone permanently, and the store reads as though the entity simply
        // appeared. That is the shape of failure this codebase is built to
        // refuse — plausible, quiet, and unrecoverable.
        let entity_type = entity.entity_type();
        let tx = self
            .conn
            .transaction()
            .map_err(Error::storage(format!("create the {entity_type}")))?;
        insert_created(&tx, &entity, provenance, now)?;
        tx.commit().map_err(Error::storage(format!(
            "commit the {entity_type} `{}`",
            entity.label()
        )))?;

        Ok(Created {
            entity,
            created: true,
        })
    }

    fn get(&self, id: &EntityId) -> Result<Option<Entity>> {
        get_entity(self.connection(), id)
    }

    fn update(
        &mut self,
        id: &EntityId,
        expected_version: i32,
        changes: &serde_json::Map<String, serde_json::Value>,
        provenance: &Provenance,
    ) -> Result<Entity> {
        let entity_type = id.entity_type();
        let mut entity = self.get(id)?.ok_or_else(|| Error::NotFound {
            entity_type,
            id: id.to_string(),
        })?;

        let current_version = entity.audit().version;
        if current_version != expected_version {
            return Err(Error::StaleVersion {
                entity_type,
                id: id.to_string(),
                supplied: expected_version,
                latest: current_version,
            });
        }

        let applied = apply_changes(&mut entity, changes)?;
        if applied.is_empty() {
            // A no-op is a success, not an error: a retrying agent re-sending
            // the same update should get the row back rather than a version
            // bump and an event saying nothing happened.
            return Ok(entity);
        }

        // Checked on update as well as create, or the requirement holds only
        // for the first write and a later call can blank the explainer back
        // out. A rule enforced on one of two doors is a rule with a door.
        validate_entity(&entity)?;

        // A row read back with no readable number gets one before it is written
        // again. Reading NULL as zero keeps one unnumbered row from making a
        // whole table unreadable; writing zero back would trade that for two
        // rows colliding on the unique index, which is a worse trade.
        match &mut entity {
            Entity::Task(t) if t.number == 0 => {
                t.number = next_number_in(self.connection(), "tasks", &t.project_id)?;
            }
            Entity::Decision(d) if d.number == 0 => {
                d.number = next_number_in(self.connection(), "decisions", &d.project_id)?;
            }
            _ => {}
        }

        // Re-checked on update, not only on create. A parent set later is the
        // one that can close a cycle: A is created, then B under A, then A is
        // moved under B.
        if let Entity::Task(t) = &entity {
            self.check_task_parent(t)?;
        }

        // `closed_at` follows the status, unless the caller set it explicitly
        // in the same call. Cleared on the way back out, too: a task reopened
        // keeps no stale completion date, or it would be counted as closed by
        // every question that filters on one.
        if let Entity::Task(task) = &mut entity
            && applied.iter().any(|c| c.field == "status")
        {
            let terminal = task.status.is_terminal();
            if !changes.contains_key("closed_at") {
                task.closed_at = match (terminal, task.closed_at) {
                    (true, None) => Some(Utc::now()),
                    (true, existing) => existing,
                    (false, _) => None,
                };
            }
            // A finished task is not being worked on. Released here rather than
            // only in `close_task` so that every path into a terminal status
            // agrees — otherwise a plain `specline_update(status: done)` would leave
            // a claim standing and `specline ready --unclaimed` would keep skipping
            // work nobody is doing.
            if terminal {
                task.claimed_by = None;
                task.claimed_at = None;
            }
            // The rule that makes the definition of done an invariant rather
            // than a checklist someone is asked to honour: nothing reaches a
            // terminal status without saying why, and `done` needs evidence.
            //
            // Checked on the *transition* only. A hundred and seven tasks
            // closed before this existed and carry none of it; running the
            // check on every write would freeze every one of them.
            if terminal {
                task.validate_close()?;
            }
        }

        let now = Utc::now();
        let next_version = current_version + 1;
        *entity.audit_mut() = entity.audit().touched(provenance, now, next_version);

        let project = project_of(&entity);
        let action = if is_status_change(&applied) {
            Action::StatusChanged
        } else {
            Action::Updated
        };

        // The row and every event describing it, together. An update that lands
        // its version bump and loses its events is worse than one that fails:
        // the optimistic-concurrency check will happily accept the next write,
        // so nothing ever notices the hole.
        let tx = self
            .conn
            .transaction()
            .map_err(Error::storage(format!("update {id}")))?;
        write_back(&tx, &entity, expected_version)?;

        // One event per field. Verbose, but "what changed" is the question the
        // activity feed exists to answer, and a single event with a bag of
        // fields cannot be filtered by field later.
        for change in &applied {
            let summary = format!(
                "{} {} → {}",
                change.field,
                render(&change.field, &change.before),
                render(&change.field, &change.after)
            );
            append_event_inner(
                &tx,
                NewEvent::new(entity.id().clone(), action, summary)
                    .in_project(project.clone())
                    .field_change(
                        change.field.clone(),
                        change.before.clone(),
                        change.after.clone(),
                    ),
                provenance,
                now,
            )?;
        }
        tx.commit()
            .map_err(Error::storage(format!("commit the update to {id}")))?;

        Ok(entity)
    }

    fn archive(
        &mut self,
        id: &EntityId,
        expected_version: i32,
        provenance: &Provenance,
    ) -> Result<Entity> {
        let entity_type = id.entity_type();
        let mut entity = self.get(id)?.ok_or_else(|| Error::NotFound {
            entity_type,
            id: id.to_string(),
        })?;

        let current_version = entity.audit().version;
        if current_version != expected_version {
            return Err(Error::StaleVersion {
                entity_type,
                id: id.to_string(),
                supplied: expected_version,
                latest: current_version,
            });
        }
        // Archiving something already archived is what the caller wanted to be
        // true, and it is. Returning it unchanged beats an error nobody can act
        // on.
        if entity.audit().is_archived() {
            return Ok(entity);
        }

        let now = Utc::now();
        let mut audit = entity.audit().touched(provenance, now, current_version + 1);
        audit.archived_at = Some(now);
        *entity.audit_mut() = audit;

        // Three statements, one outcome. Half an archive — the row put away
        // with its links still live — is a graph that traverses into something
        // nothing shows, which is precisely the empty-looking-but-wrong result
        // the graph rules warn about.
        let tx = self
            .conn
            .transaction()
            .map_err(Error::storage(format!("archive {id}")))?;
        write_back(&tx, &entity, expected_version)?;

        // Archiving a parent archives its links but never its children
        // (SPEC §3.1). Orphaned children surface in `fsck` rather than
        // disappearing, because a cascade is unrecoverable and an orphan is
        // merely untidy.
        archive_links_touching(&tx, id, now, provenance)?;

        append_event_inner(
            &tx,
            NewEvent::new(
                entity.id().clone(),
                Action::Archived,
                format!("archived {entity_type} “{}”", entity.label()),
            )
            .in_project(project_of(&entity)),
            provenance,
            now,
        )?;
        tx.commit()
            .map_err(Error::storage(format!("commit the archive of {id}")))?;

        Ok(entity)
    }

    fn list(&self, query: &EntityQuery) -> Result<Page<Entity>> {
        let types: Vec<EntityType> = if query.entity_types.is_empty() {
            EntityType::ALL.to_vec()
        } else {
            query.entity_types.clone()
        };

        let limit = query.limit.unwrap_or(DEFAULT_LIST_LIMIT);
        let mut all = Vec::new();
        let mut total = 0usize;

        for entity_type in types {
            let spec = spec_for(entity_type);
            let mut clauses: Vec<String> = Vec::new();
            let mut params: Vec<Value> = Vec::new();

            if let Some(p) = &query.project_id {
                match entity_type.project_scope() {
                    ProjectScope::IsTheProject => {
                        clauses.push("id = ?".to_owned());
                        params.push(id_param(p));
                    }
                    // A global term belongs to every project's glossary, so a
                    // project-scoped list must include globals as well as
                    // overrides — that is what "project-first resolution" means
                    // in practice (Q-4).
                    ProjectScope::Optional => {
                        clauses.push("(project_id = ? OR project_id IS NULL)".to_owned());
                        params.push(id_param(p));
                    }
                    ProjectScope::Required => {
                        clauses.push("project_id = ?".to_owned());
                        params.push(id_param(p));
                    }
                }
            }

            if !query.include_archived {
                clauses.push("archived_at IS NULL".to_owned());
            }

            if !query.statuses.is_empty() {
                // Four types have no lifecycle at all. Filtering them by status
                // should exclude them, not error — a cross-type query for
                // "everything blocked" is a reasonable thing to ask.
                let status_col = match entity_type {
                    EntityType::Design => Some("state"),
                    EntityType::Term
                    | EntityType::Feedback
                    | EntityType::Metric
                    | EntityType::MetricObservation
                    | EntityType::Artifact => None,
                    _ => Some("status"),
                };
                match status_col {
                    None => continue,
                    Some(col) => {
                        let placeholders = vec!["?"; query.statuses.len()].join(", ");
                        clauses.push(format!("{col} IN ({placeholders})"));
                        params.extend(query.statuses.iter().map(text));
                    }
                }
            }

            if let Some(since) = query.since {
                clauses.push("created_at >= ?".to_owned());
                params.push(ts(since));
            }
            if let Some(until) = query.until {
                clauses.push("created_at < ?".to_owned());
                params.push(ts(until));
            }

            let where_clause = if clauses.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", clauses.join(" AND "))
            };

            total += count(
                self.connection(),
                &format!("SELECT count(*) FROM {}{where_clause}", spec.table),
                params.clone(),
            )?;

            // No per-type OFFSET. The offset is a position in the *merged*
            // list, and applying it to each table skipped that many rows of
            // every type — so page two of a cross-type list dropped rows nobody
            // had seen and showed rows that belonged on page five. Fetching
            // `offset + limit` per type is enough by construction: a row in the
            // global window cannot be further than that into its own type's
            // ordering, because every row ahead of it globally is ahead of it
            // there too.
            let sql = format!(
                "{}{where_clause} ORDER BY id DESC LIMIT {}",
                select_from(&spec),
                query.offset + limit
            );
            all.extend(query_many(self.connection(), entity_type, &sql, params)?);
        }

        // Across types, order by id — which is creation order, since ULIDs sort
        // chronologically (B-9).
        all.sort_by(|a, b| b.id().cmp(a.id()));
        let page: Vec<Entity> = all.into_iter().skip(query.offset).take(limit).collect();
        Ok(Page::new(page, total))
    }

    fn link(&mut self, link: NewLink, provenance: &Provenance) -> Result<Link> {
        let requested_rel = link.rel;
        // `depends_on` is normalised into `blocks` by swapping the endpoints.
        // Only `blocks` is ever stored (D-11) — see the graph-direction section
        // of `product/CLAUDE.md` for why storing both is the bug that fails
        // silently and plausibly.
        let (from_id, rel, to_id, anchor, note) = link.normalised()?;

        let from = require_live(self.connection(), &from_id, "source")?;
        let to = require_live(self.connection(), &to_id, "target")?;

        // Re-creating an existing edge returns it rather than erroring: the
        // unique index would reject it, and an agent re-asserting a true fact
        // should not be punished for it.
        if let Some(existing) = find_link(self.connection(), &from_id, rel, &to_id, &anchor, true)?
        {
            if existing.audit.is_archived() {
                // Un-archive rather than insert a duplicate: the unique index
                // covers archived rows too, so a second insert would fail.
                let now = Utc::now();
                set_link_archived(self.connection(), &existing.id, None, now, provenance)?;
                return find_link(self.connection(), &from_id, rel, &to_id, &anchor, true)?
                    .ok_or_else(|| Error::Invariant {
                        operation: format!("restore the link {}", existing.id),
                        problem: "the link vanished between restoring and re-reading it".to_owned(),
                    });
            }
            return Ok(existing);
        }

        let now = Utc::now();
        let project_id = from.project_id().or_else(|| to.project_id()).cloned();
        let stored = Link {
            id: LinkId::generate(),
            project_id,
            from_type: from_id.entity_type(),
            from_id: from_id.clone(),
            rel,
            to_type: to_id.entity_type(),
            to_id: to_id.clone(),
            anchor: anchor.clone(),
            note,
            audit: Audit::new(provenance, now),
        };

        let tx = self.conn.transaction().map_err(Error::storage(format!(
            "create the link {from_id} {rel} {to_id}"
        )))?;
        insert_link_row(&tx, &stored)?;

        // Summaries name the artifacts, not their ids. This text is what the
        // activity feed and the Sunday-review digest actually show a human.
        //
        // The direction stated is the *stored* one. When a caller asked for
        // `depends_on`, saying so as well is what stops the next reader
        // thinking the endpoints were recorded backwards.
        let from_label = truncate(from.label(), 60);
        let to_label = truncate(to.label(), 60);
        let summary = if requested_rel == Relation::DependsOn {
            format!(
                "“{to_label}” depends on “{from_label}” (stored as “{from_label}” blocks \
                 “{to_label}”)"
            )
        } else {
            format!("“{from_label}” {rel} “{to_label}”")
        };
        append_event_inner(
            &tx,
            NewEvent::new(from_id, Action::Linked, summary)
                .in_project(stored.project_id.clone())
                .with_meta(serde_json::json!({
                    "rel": rel.as_str(),
                    "to_id": to_id.as_str(),
                    "anchor": anchor,
                })),
            provenance,
            now,
        )?;
        tx.commit()
            .map_err(Error::storage(format!("commit the link {}", stored.id)))?;

        Ok(stored)
    }

    fn unlink(
        &mut self,
        from_id: &EntityId,
        rel: Relation,
        to_id: &EntityId,
        anchor: &str,
        provenance: &Provenance,
    ) -> Result<Link> {
        let (from, rel, to) = Relation::normalise(from_id.clone(), rel, to_id.clone());

        let existing =
            find_link(self.connection(), &from, rel, &to, anchor, false)?.ok_or_else(|| {
                Error::Invariant {
                    operation: format!("remove the link {from} {rel} {to}"),
                    problem: "no live link matches those endpoints, relation and anchor".to_owned(),
                }
            })?;

        // Soft delete, links included (hard constraint 3): this sets
        // `archived_at` and never issues a DELETE.
        let now = Utc::now();

        // Labels are read before the transaction opens, because the closure
        // borrowing `self` and the transaction borrowing `self.conn` mutably
        // cannot both be live — and because the labels are the same either way:
        // an unlink does not change what the endpoints are called.
        let label_of = |id: &EntityId| {
            self.get(id)
                .ok()
                .flatten()
                .map(|e| truncate(e.label(), 60))
                .unwrap_or_else(|| id.to_string())
        };
        let summary = format!("unlinked “{}” {rel} “{}”", label_of(&from), label_of(&to));

        let tx = self
            .conn
            .transaction()
            .map_err(Error::storage(format!("remove the link {}", existing.id)))?;
        set_link_archived(&tx, &existing.id, Some(now), now, provenance)?;
        append_event_inner(
            &tx,
            NewEvent::new(from.clone(), Action::Linked, summary)
                .in_project(existing.project_id.clone())
                .with_meta(serde_json::json!({ "removed": true, "rel": rel.as_str() })),
            provenance,
            now,
        )?;
        tx.commit().map_err(Error::storage(format!(
            "commit the removal of link {}",
            existing.id
        )))?;

        let mut archived = existing;
        archived.audit.archived_at = Some(now);
        Ok(archived)
    }

    fn append_event(&mut self, event: NewEvent, provenance: &Provenance) -> Result<Event> {
        append_event_inner(self.connection(), event, provenance, Utc::now())
    }

    fn client_for_session(&self, session_id: &str) -> Result<Option<SessionClient>> {
        Ok(query_session_clients(
            self.connection(),
            &format!("{SESSION_CLIENT_COLUMNS} FROM session_clients WHERE session_id = ?"),
            vec![text(session_id)],
        )?
        .pop())
    }

    fn session_clients(&self, limit: usize) -> Result<Vec<SessionClient>> {
        // Saturate rather than cast, for legibility rather than for a bug.
        // `usize::MAX as i64` is -1 and SQLite reads a negative LIMIT as no
        // upper bound, so the cast and this returned identical rows — any
        // ceiling at or above the row count returns all of them. What it cost
        // was a reader working out that a wrapped negative happened to mean the
        // right thing. `try_from` says the intent instead of arriving at it.
        let ceiling = i64::try_from(limit).unwrap_or(i64::MAX);
        query_session_clients(
            self.connection(),
            &format!(
                "{SESSION_CLIENT_COLUMNS} FROM session_clients ORDER BY last_seen DESC LIMIT ?"
            ),
            vec![Value::Integer(ceiling)],
        )
    }

    fn events(
        &self,
        cursor: &Cursor,
        project_id: Option<&EntityId>,
        limit: usize,
    ) -> Result<Page<Event>> {
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();

        match cursor {
            Cursor::After(id) => {
                clauses.push("id > ?".to_owned());
                params.push(text(id.as_str()));
            }
            Cursor::Since(t) => {
                clauses.push("at >= ?".to_owned());
                params.push(ts(*t));
            }
            Cursor::Beginning => {}
        }
        if let Some(p) = project_id {
            clauses.push("project_id = ?".to_owned());
            params.push(id_param(p));
        }

        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };

        let total = count(
            self.connection(),
            &format!("SELECT count(*) FROM events{where_clause}"),
            params.clone(),
        )?;

        // Ascending by id: a cursor-following caller needs the *oldest* unseen
        // events first, otherwise a limit silently skips the middle of the
        // range and "catch me up" is quietly wrong.
        let sql =
            format!("{EVENT_COLUMNS} FROM events{where_clause} ORDER BY id ASC LIMIT {limit}");
        Ok(Page::new(
            query_events(self.connection(), &sql, params)?,
            total,
        ))
    }

    fn recent_events(
        &self,
        scope: crate::store::EventScope<'_>,
        limit: usize,
    ) -> Result<Page<Event>> {
        use crate::store::EventScope;

        let (where_clause, params): (&str, Vec<Value>) = match scope {
            EventScope::Everything => ("", Vec::new()),
            EventScope::Project(p) => (" WHERE project_id = ?", vec![id_param(p)]),
            EventScope::Entity(e) => (" WHERE entity_id = ?", vec![id_param(e)]),
        };

        let total = count(
            self.connection(),
            &format!("SELECT count(*) FROM events{where_clause}"),
            params.clone(),
        )?;

        // Descending in SQL, so the *engine* decides which rows to keep. The
        // whole bug this replaces was making that decision in Rust, after the
        // engine had already thrown the interesting half away.
        let sql =
            format!("{EVENT_COLUMNS} FROM events{where_clause} ORDER BY id DESC LIMIT {limit}");
        Ok(Page::new(
            query_events(self.connection(), &sql, params)?,
            total,
        ))
    }

    fn resolve_ref(&self, reference: &str) -> Result<Option<EntityId>> {
        // A ULID is already the answer. Checking it first means the common case
        // costs nothing and a readable reference can never shadow a real id.
        if let Ok(id) = EntityId::parse(reference) {
            return Ok(Some(id));
        }
        // Decisions first: `KEEL-B12` would otherwise be tried as a task
        // reference, fail the alphanumeric check on `KEEL-B`, and return None
        // before anything looked at decisions.
        let (table, key, number) = match crate::types::parse_decision_ref(reference) {
            Some((key, number)) => ("decisions", key, number),
            None => match crate::types::parse_readable_ref(reference) {
                Some((key, number)) => ("tasks", key, number),
                None => return Ok(None),
            },
        };

        let found: std::result::Result<String, _> = self.connection().query_row(
            &format!(
                "SELECT e.id FROM {table} e JOIN projects p ON p.id = e.project_id \
                 WHERE upper(p.key) = ? AND e.number = ?"
            ),
            params_from_iter(vec![text(key), Value::Integer(i64::from(number))]),
            |row| row.get::<_, String>(0),
        );
        match found {
            Ok(id) => Ok(Some(EntityId::parse(&id)?)),
            // A reference that resolves to nothing is not an error — the caller
            // asked whether it names something, and the answer is no.
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Error::storage(format!(
                "resolve the reference `{reference}`"
            ))(e)),
        }
    }

    fn next_task_number(&self, project_id: &EntityId) -> Result<i32> {
        next_number_in(self.connection(), "tasks", project_id)
    }

    fn next_task_rank(&self, project_id: &EntityId) -> Result<f64> {
        // New work lands at the end of the deliberate order rather than the
        // start. A task nobody has placed has not been prioritised, and putting
        // it at the top would be the store making that claim on their behalf.
        let max: Option<f64> = self
            .connection()
            .query_row(
                "SELECT max(rank) FROM tasks WHERE project_id = ?",
                params_from_iter(vec![id_param(project_id)]),
                |row| row.get::<_, Option<f64>>(0),
            )
            .map_err(Error::storage("read the last rank in a project"))?;
        Ok(max.unwrap_or(0.0) + 1.0)
    }

    fn rank_between(&self, before: Option<f64>, after: Option<f64>) -> Result<f64> {
        match (before, after) {
            // Between two neighbours: the midpoint, which touches one row where
            // a renumbering would touch every row below it.
            (Some(a), Some(b)) => {
                if (a - b).abs() < f64::EPSILON {
                    return Err(Error::Invariant {
                        operation: "place a task between two others".to_owned(),
                        problem: format!(
                            "both neighbours have rank {a}, so there is no space between them"
                        ),
                    });
                }
                Ok((a + b) / 2.0)
            }
            (Some(a), None) => Ok(a + 1.0),
            (None, Some(b)) => Ok(b - 1.0),
            (None, None) => Ok(1.0),
        }
    }

    fn check_task_parent(&self, task: &crate::Task) -> Result<()> {
        let Some(parent_id) = &task.parent_id else {
            return Ok(());
        };

        if parent_id == &task.id {
            return Err(Error::invalid(
                EntityType::Task,
                "parent_id",
                "a task cannot be its own parent".to_owned(),
                "the id of a different task in the same project".to_owned(),
            ));
        }

        let Some(Entity::Task(parent)) = self.get(parent_id)? else {
            return Err(Error::invalid(
                EntityType::Task,
                "parent_id",
                format!("no task with id {parent_id} exists"),
                "the id of a task in the same project — a parent is a task, not a milestone or \
                 a spec"
                    .to_owned(),
            ));
        };

        if parent.project_id != task.project_id {
            return Err(Error::invalid(
                EntityType::Task,
                "parent_id",
                "the parent belongs to a different project".to_owned(),
                "a task in the same project; composition does not cross project boundaries"
                    .to_owned(),
            ));
        }

        // Walk up from the proposed parent. A cycle here is not a tidiness
        // problem: every rollup and every render of the tree would recurse
        // until something ran out of stack, and the store is the only place
        // that can see the whole chain.
        let mut seen = vec![task.id.clone()];
        let mut cursor = Some(parent.id.clone());
        while let Some(id) = cursor {
            if seen.contains(&id) {
                return Err(Error::invalid(
                    EntityType::Task,
                    "parent_id",
                    "that would make a task its own ancestor".to_owned(),
                    "a task that is not already below this one in the tree".to_owned(),
                ));
            }
            seen.push(id.clone());
            if seen.len() > crate::types::MAX_PARENT_DEPTH {
                return Err(Error::invalid(
                    EntityType::Task,
                    "parent_id",
                    format!(
                        "the chain of parents is deeper than {}",
                        crate::types::MAX_PARENT_DEPTH
                    ),
                    "a shallower tree — work nested that deep is usually a milestone".to_owned(),
                ));
            }
            cursor = match self.get(&id)? {
                Some(Entity::Task(t)) => t.parent_id,
                _ => None,
            };
        }
        Ok(())
    }

    fn unique_project_key(&self, base: &str) -> Result<String> {
        // Suffix until free. Bounded because an unbounded loop over a taken key
        // is the shape of a hang, and a store with a hundred projects sharing
        // five letters has a naming problem that a suffix was never going to
        // fix.
        for attempt in 1..=99 {
            let candidate = if attempt == 1 {
                base.to_owned()
            } else {
                format!("{base}{attempt}")
            };
            let taken = count(
                self.connection(),
                "SELECT count(*) FROM projects WHERE upper(key) = ?",
                vec![text(candidate.to_uppercase())],
            )?;
            if taken == 0 {
                return Ok(candidate);
            }
        }
        Err(Error::Invariant {
            operation: format!("assign a project key starting from `{base}`"),
            problem: "ninety-nine projects already share these letters".to_owned(),
        })
    }

    fn latest_event_id(&self) -> Result<Option<EventId>> {
        // ULIDs are minted monotonically (B-9), so the largest id *is* the most
        // recent event. That is what makes this one indexed row rather than an
        // ordering over the whole table — it used to fetch up to a hundred
        // thousand events and take the last, twice per tool call, while holding
        // the write lock.
        let found: std::result::Result<Option<String>, _> =
            self.connection()
                .query_row("SELECT max(id) FROM events", [], |row| {
                    row.get::<_, Option<String>>(0)
                });
        match found {
            Ok(None) => Ok(None),
            Ok(Some(id)) if id.is_empty() => Ok(None),
            Ok(Some(id)) => Ok(Some(EventId::parse(&id)?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Error::storage("read the latest event id")(e)),
        }
    }

    fn events_for(&self, entity_id: &EntityId, limit: usize) -> Result<Page<Event>> {
        let params = vec![id_param(entity_id)];
        let total = count(
            self.connection(),
            "SELECT count(*) FROM events WHERE entity_id = ?",
            params.clone(),
        )?;

        // Ascending, like the project feed: a history reads forwards, and a
        // caller that wants the newest first can reverse a list it already has
        // in full. Descending here would make the `limit` keep the *newest* and
        // drop the beginning, which is the half that explains the rest.
        let sql = format!(
            "{EVENT_COLUMNS} FROM events WHERE entity_id = ? ORDER BY id ASC LIMIT {limit}"
        );
        Ok(Page::new(
            query_events(self.connection(), &sql, params)?,
            total,
        ))
    }

    fn add_note(&mut self, note: NewNote, provenance: &Provenance) -> Result<Note> {
        note.validate()?;

        // The subject must exist. `v_entities` is why this is one query rather
        // than a match over thirteen tables — resolving an id without knowing
        // its type is exactly what the view was built for.
        let Some((entity_type, project_id, archived)) =
            resolve_vertex(self.connection(), &note.entity_id)?
        else {
            // Deliberately not `NotFound`: its message quotes the id inside
            // backticks, so appending an explanation there produced prose
            // inside what reads as the identifier, and a model parsing that has
            // been handed a malformed id.
            return Err(Error::Invalid {
                entity_type: EntityType::Task,
                field: "id".to_owned(),
                problem: format!(
                    "no row with id {} exists, so it cannot be annotated",
                    note.entity_id
                ),
                expected:
                    "an id returned by specline_context, specline_search or specline_get — a note \
                           cannot outlive the row it hangs off, and nothing links to a note, \
                           so an orphaned one would never surface again"
                        .to_owned(),
            });
        };
        if archived {
            return Err(Error::Invalid {
                entity_type,
                field: "entity_id".to_owned(),
                problem: format!("{} is archived", note.entity_id),
                expected: "a live row — annotating an archived one writes commentary that \
                           nothing will ever show. Restore it, or note the row that replaced it"
                    .to_owned(),
            });
        }

        let stored = Note {
            id: NoteId::generate(),
            project_id,
            entity_type,
            entity_id: note.entity_id,
            body: note.body,
            author: note.author,
            // Provenance wins over anything the caller put on the note, for the
            // same reason it does on every other write: one source of truth for
            // who is acting, decided at the boundary and not per call.
            session_id: note.session_id.or_else(|| provenance.session_id.clone()),
            surface: note.surface.or(provenance.surface),
            created_at: Utc::now(),
            archived_at: None,
        };

        insert_note_row(self.connection(), &stored)?;
        // A note is the one mutation that appends no event, so the recording
        // that rides on `append_event_inner` never fires for it. A session that
        // only ever annotates — which is most of what a conversation does once
        // the row exists — would otherwise have no client on file at all.
        record_session_client(self.connection(), provenance, stored.created_at)?;

        Ok(stored)
    }

    fn notes_for(&self, entity_id: &EntityId, include_retracted: bool) -> Result<Vec<Note>> {
        let filter = if include_retracted {
            ""
        } else {
            " AND archived_at IS NULL"
        };
        query_notes(
            self.connection(),
            &format!("{NOTE_COLUMNS} FROM notes WHERE entity_id = ?{filter} ORDER BY id ASC"),
            vec![id_param(entity_id)],
        )
    }

    fn notes_in_project(&self, project_id: &EntityId) -> Result<Vec<Note>> {
        query_notes(
            self.connection(),
            &format!(
                "{NOTE_COLUMNS} FROM notes WHERE project_id = ? AND archived_at IS NULL \
                 ORDER BY id ASC"
            ),
            vec![id_param(project_id)],
        )
    }

    fn retract_note(&mut self, id: &NoteId, provenance: &Provenance) -> Result<Note> {
        let now = Utc::now();
        let changed = self
            .connection()
            .execute(
                "UPDATE notes SET archived_at = ? WHERE id = ? AND archived_at IS NULL",
                params_from_iter(vec![ts(now), text(id.as_str())]),
            )
            .map_err(Error::storage(format!("retract note {id}")))?;
        if changed == 0 {
            // Either it never existed or it is already retracted. Both are the
            // caller believing something false about the store, and both are
            // worth saying out loud rather than returning a silent success.
            return Err(Error::NotFound {
                entity_type: EntityType::Task,
                id: format!("{id} — no live note with this id"),
            });
        }

        let note = query_notes(
            self.connection(),
            &format!("{NOTE_COLUMNS} FROM notes WHERE id = ?"),
            vec![text(id.as_str())],
        )?
        .pop()
        .ok_or_else(|| Error::NotFound {
            entity_type: EntityType::Task,
            id: id.to_string(),
        })?;

        // `provenance` used to be discarded here — `let _ = provenance;` — in a
        // store whose entire argument is that every write says who made it. A
        // retraction is the one note operation that removes something from
        // view, so it is the one most worth being able to attribute, and it
        // was the only one that could not be.
        append_event_inner(
            self.connection(),
            NewEvent::new(
                note.entity_id.clone(),
                Action::Archived,
                format!("retracted a note on {}", note.entity_id),
            )
            .in_project(note.project_id.clone())
            .with_meta(serde_json::json!({ "note_id": id.as_str(), "retracted": true })),
            provenance,
            now,
        )?;

        Ok(note)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{
        Artifact, ArtifactKind, CloseReason, Decision, DecisionStatus, Design, DesignState,
        Environment, EnvironmentStatus, Feedback, FeedbackKind, Metric, MetricDirection,
        MetricObservation, Milestone, MilestoneKind, MilestoneStatus, Project, Question,
        QuestionKind, RiskSeverity, Sentiment, Spec, SpecKind, SpecStatus, Task, TaskKind,
        TaskPriority, Term,
    };
    use chrono::NaiveDate;
    use serde_json::json;

    fn store() -> Store {
        Store::in_memory().expect("open an in-memory store")
    }

    fn claude() -> Provenance {
        Provenance::anonymous(Actor::Claude)
            .with_session("ses_test")
            .with_surface(Surface::Code)
    }

    /// An event's sentence and its metadata have to survive being written.
    ///
    /// This is a regression test for a real gap: the `events` table was built
    /// without `summary` or `meta`, and nothing failed. Every write succeeded,
    /// every count was right, and every event read back with an empty sentence
    /// — so `render_status` would have written a changelog of blank lines and
    /// the activity feed would have returned rows saying nothing, for new
    /// history only, while the old history still read correctly.
    ///
    /// Forty-two tests passed over that hole, because they all asserted on
    /// rows and statuses rather than on the sentence a person reads.
    #[test]
    fn an_events_summary_and_meta_survive_the_write() {
        let mut s = store();
        let project_id = project(&mut s);

        let created = s.events_for(&project_id, 10).expect("read the events");
        let first = created
            .items
            .first()
            .expect("creating a project logs an event");
        assert!(
            !first.summary.is_empty(),
            "a created event should carry the sentence it was written with"
        );
        assert!(
            first.summary.contains("Specline"),
            "the summary should name what was created, got {:?}",
            first.summary
        );

        // `meta` is what tells an unlink apart from a link in the log, so it
        // gets its own assertion rather than riding on the summary's.
        let event = NewEvent::new(project_id.clone(), Action::Linked, "unlinked something")
            .with_meta(serde_json::json!({ "removed": true }));
        s.append_event(event, &claude()).expect("append the event");

        let back = s.events_for(&project_id, 10).expect("read the events");
        let marked = back
            .items
            .iter()
            .find(|e| e.summary == "unlinked something")
            .expect("the appended event should be readable");
        assert_eq!(
            marked.meta,
            Some(serde_json::json!({ "removed": true })),
            "meta should survive the round trip"
        );
    }

    fn project(store: &mut Store) -> EntityId {
        store
            .create(Project::new("specline", "Specline").into(), &claude())
            .expect("create the project")
            .entity
            .id()
            .clone()
    }

    /// One fully populated instance of every non-project type, so the round
    /// trip exercises optional columns rather than only the required ones.
    fn one_of_each(project_id: &EntityId, metric_id: &EntityId) -> Vec<Entity> {
        let mut milestone = Milestone::new(
            project_id.clone(),
            "Phase 9 — One database",
            "Collapse DuckDB and Lance into one SQLite file.",
        );
        milestone.kind = MilestoneKind::Release;
        milestone.status = MilestoneStatus::Paused;
        milestone.target_date = NaiveDate::from_ymd_opt(2026, 9, 30);
        milestone.version_string = Some("0.9.0".into());
        milestone.sort_order = Some(1);

        let mut task = Task::new(
            project_id.clone(),
            "Write the SQLite entity store",
            "Twenty trait methods against rusqlite, with the same semantics.",
        );
        task.kind = TaskKind::Chore;
        task.body = Some("One file, one engine, one write path.".into());
        task.priority = TaskPriority::P0;
        task.labels = vec!["storage".into(), "phase-9".into()];
        task.external_refs = vec!["https://github.com/kb/specline/pull/9".into()];

        let mut spec = Spec::new(project_id.clone(), "Storage specification");
        spec.kind = SpecKind::DesignDoc;
        spec.status = SpecStatus::Approved;
        spec.mirror_path = Some(".specline/specs/storage.md".into());

        let mut decision = Decision::new(project_id.clone(), "SQLite, one file");
        decision.status = DecisionStatus::Accepted;
        decision.decided_at = Some(Utc::now());

        let mut question = Question::new(project_id.clone(), "Where does the store live?");
        question.kind = QuestionKind::Risk;
        question.severity = Some(RiskSeverity::High);

        let mut term = Term::new(
            Some(project_id.clone()),
            "Digest",
            "The compact project summary returned by specline_context",
        );
        term.aliases = vec!["context digest".into()];

        let mut feedback = Feedback::new(project_id.clone(), "Onboarding felt slow");
        feedback.kind = FeedbackKind::Interview;
        feedback.source = Some("Customer A".into());
        feedback.contact = Some("a@example.com".into());
        feedback.sentiment = Some(Sentiment::Negative);
        feedback.occurred_at = Some(Utc::now());

        let mut design = Design::new(project_id.clone(), "Home screen");
        design.state = DesignState::Approved;
        design.figma_ref = Some("figma:node/123".into());

        let mut environment = Environment::new(project_id.clone(), "production");
        environment.url = Some("https://specline.local".into());
        environment.deployed_version = Some("0.1.0".into());
        environment.deployed_commit = Some("abc1234".into());
        environment.status = EnvironmentStatus::Healthy;
        environment.last_deployed_at = Some(Utc::now());

        let mut metric = Metric::new(
            project_id.clone(),
            "Sessions where Claude writes to Specline",
        );
        metric.id = metric_id.clone();
        metric.unit = Some("%".into());
        metric.target_value = Some(80.0);
        metric.direction = MetricDirection::Up;

        let mut observation =
            MetricObservation::new(metric_id.clone(), project_id.clone(), 62.5, Utc::now());
        observation.note = Some("Before the skill landed".into());

        let mut artifact = Artifact::new(project_id.clone(), "Competitor teardown");
        artifact.kind = ArtifactKind::Link;
        artifact.url = Some("https://example.com/teardown".into());

        vec![
            milestone.into(),
            task.into(),
            spec.into(),
            decision.into(),
            question.into(),
            term.into(),
            feedback.into(),
            design.into(),
            environment.into(),
            metric.into(),
            observation.into(),
            artifact.into(),
        ]
    }

    /// A field that exists on this type and takes a string, for the update leg
    /// of the round trip. Picked per type because there is no field all
    /// thirteen share that a caller may set.
    fn a_settable_field(entity_type: EntityType) -> &'static str {
        match entity_type {
            EntityType::Project => "description",
            EntityType::Milestone => "summary",
            EntityType::Task => "body",
            EntityType::Spec | EntityType::Decision | EntityType::Question => "mirror_path",
            EntityType::Term => "definition",
            EntityType::Feedback => "source",
            EntityType::Design => "figma_ref",
            EntityType::Environment => "url",
            EntityType::Metric => "unit",
            EntityType::MetricObservation => "note",
            EntityType::Artifact => "url",
        }
    }

    // --- Round trip ------------------------------------------------------

    #[test]
    fn every_entity_type_round_trips_through_create_read_update_and_archive() {
        let mut s = store();
        let project_id = project(&mut s);
        let metric_id = EntityId::generate(EntityType::Metric);

        let mut all: Vec<Entity> = vec![
            s.get(&project_id)
                .unwrap()
                .expect("the project must read back"),
        ];
        for entity in one_of_each(&project_id, &metric_id) {
            let created = s.create(entity, &claude()).expect("create");
            assert!(created.created, "the first create must actually create");
            all.push(created.entity);
        }

        assert_eq!(
            all.len(),
            EntityType::ALL.len(),
            "the round trip must cover all thirteen types"
        );

        for entity in all {
            let entity_type = entity.entity_type();
            let id = entity.id().clone();

            let read = s
                .get(&id)
                .unwrap()
                .unwrap_or_else(|| panic!("{entity_type} must read back"));
            assert_eq!(read.audit().version, 1, "{entity_type} starts at version 1");

            let field = a_settable_field(entity_type);
            let mut changes = serde_json::Map::new();
            changes.insert(field.to_owned(), json!("changed by the round trip"));
            let updated = s
                .update(&id, 1, &changes, &claude())
                .unwrap_or_else(|e| panic!("update {entity_type}: {e}"));
            assert_eq!(
                updated.audit().version,
                2,
                "{entity_type} bumps its version"
            );

            let archived = s
                .archive(&id, 2, &claude())
                .unwrap_or_else(|e| panic!("archive {entity_type}: {e}"));
            assert!(
                archived.audit().is_archived(),
                "{entity_type} must come back archived"
            );

            // Archived rows stay readable — soft delete only.
            let after = s.get(&id).unwrap().expect("archived rows still read back");
            assert!(after.audit().is_archived());
        }
    }

    #[test]
    fn an_update_that_changes_nothing_bumps_no_version_and_writes_no_event() {
        let mut s = store();
        let project_id = project(&mut s);
        let before = s.events_for(&project_id, 100).unwrap().total;

        let mut changes = serde_json::Map::new();
        changes.insert("name".to_owned(), json!("Specline"));
        let same = s.update(&project_id, 1, &changes, &claude()).unwrap();

        assert_eq!(same.audit().version, 1);
        assert_eq!(s.events_for(&project_id, 100).unwrap().total, before);
    }

    #[test]
    fn updating_a_row_that_is_not_there_is_not_found() {
        let mut s = store();
        let missing = EntityId::generate(EntityType::Task);
        let mut changes = serde_json::Map::new();
        changes.insert("title".to_owned(), json!("nope"));
        assert!(matches!(
            s.update(&missing, 1, &changes, &claude()),
            Err(Error::NotFound { .. })
        ));
    }

    #[test]
    fn a_field_the_type_does_not_have_is_refused_with_the_ones_it_does() {
        let mut s = store();
        let project_id = project(&mut s);
        let mut changes = serde_json::Map::new();
        changes.insert("nonsense".to_owned(), json!(1));
        let err = s
            .update(&project_id, 1, &changes, &claude())
            .expect_err("an unknown field must be refused");
        let message = err.chain();
        assert!(message.contains("nonsense"), "{message}");
        assert!(
            message.contains("slug"),
            "the error should list what is settable: {message}"
        );
    }

    // --- Idempotency -----------------------------------------------------

    #[test]
    fn the_same_create_twice_returns_the_same_row_and_burns_no_number() {
        let mut s = store();
        let project_id = project(&mut s);

        let first = s
            .create(
                Task::new(project_id.clone(), "Ship the daemon", "It has to start.").into(),
                &claude(),
            )
            .unwrap();
        assert!(first.created);

        let again = s
            .create(
                Task::new(project_id.clone(), "Ship the daemon", "It has to start.").into(),
                &claude(),
            )
            .unwrap();
        assert!(!again.created, "the repeat must not create a second row");
        assert_eq!(again.entity.id(), first.entity.id());

        // The repeat must not have consumed a number, or the sequence grows
        // gaps that look like deleted work.
        let next = s
            .create(
                Task::new(
                    project_id.clone(),
                    "Draw the roadmap screen",
                    "Something else entirely.",
                )
                .into(),
                &claude(),
            )
            .unwrap();
        let Entity::Task(t) = next.entity else {
            panic!("a task create must return a task");
        };
        assert_eq!(t.number, 2, "the second real task must be number 2");
    }

    #[test]
    fn an_archived_row_is_matched_by_its_key_rather_than_duplicated() {
        let mut s = store();
        let project_id = project(&mut s);
        let created = s
            .create(
                Task::new(project_id.clone(), "Archive me", "It goes away.").into(),
                &claude(),
            )
            .unwrap();
        s.archive(created.entity.id(), 1, &claude()).unwrap();

        let again = s
            .create(
                Task::new(project_id.clone(), "Archive me", "It goes away.").into(),
                &claude(),
            )
            .unwrap();
        assert!(!again.created);
        assert!(
            again.entity.audit().is_archived(),
            "the archived row is what comes back — minting a second one beside it is how a \
             store fills with near-duplicates"
        );
    }

    #[test]
    fn a_caller_supplied_key_is_not_second_guessed_on_a_similar_title() {
        let mut s = store();
        let project_id = project(&mut s);

        let mut first = Task::new(project_id.clone(), "Deploy", "Push it to staging.");
        first.idempotency_key = "deploy-staging".to_owned();
        let mut second = Task::new(project_id.clone(), "Deploy", "Push it to production.");
        second.idempotency_key = "deploy-production".to_owned();

        let a = s.create(first.into(), &claude()).unwrap();
        let b = s.create(second.into(), &claude()).unwrap();
        assert!(a.created && b.created);
        assert_ne!(
            a.entity.id(),
            b.entity.id(),
            "an explicit key is the caller asserting these are different things"
        );
    }

    #[test]
    fn a_near_identical_title_with_a_derived_key_returns_the_existing_row() {
        let mut s = store();
        let project_id = project(&mut s);
        // The exact pair two gate runs produced: the same work, said once with
        // a trailing word and once without. The key is a hash of the title, so
        // only the similarity check can see they are one thing.
        let first = s
            .create(
                Task::new(
                    project_id.clone(),
                    "Validate constituent phases to 0–360 degrees",
                    "The storage spine.",
                )
                .into(),
                &claude(),
            )
            .unwrap();

        let again = s
            .create(
                Task::new(
                    project_id.clone(),
                    "Validate constituent phases to 0–360",
                    "The storage spine.",
                )
                .into(),
                &claude(),
            )
            .unwrap();
        assert!(!again.created);
        assert_eq!(again.entity.id(), first.entity.id());
        assert_eq!(
            again.entity.label(),
            "Validate constituent phases to 0–360 degrees",
            "the first title wins; the caller is told nothing was created"
        );
    }

    // --- Optimistic concurrency ------------------------------------------

    #[test]
    fn a_stale_version_update_is_refused_and_names_the_current_one() {
        let mut s = store();
        let project_id = project(&mut s);

        let mut changes = serde_json::Map::new();
        changes.insert("description".to_owned(), json!("first"));
        s.update(&project_id, 1, &changes, &claude()).unwrap();

        let mut second = serde_json::Map::new();
        second.insert("description".to_owned(), json!("second"));
        match s.update(&project_id, 1, &second, &claude()) {
            Err(Error::StaleVersion {
                supplied, latest, ..
            }) => {
                assert_eq!(supplied, 1);
                assert_eq!(latest, 2, "the error must carry the current state");
            }
            other => panic!("expected a stale-version refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_second_writer_cannot_lose_the_first_writers_update() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("specline.sqlite");

        let mut writer_a = Store::open(&path).unwrap();
        let mut writer_b = Store::open(&path).unwrap();

        let project_id = project(&mut writer_a);

        // B reads the row and holds version 1 while A moves it on.
        let seen_by_b = writer_b.get(&project_id).unwrap().unwrap();
        assert_eq!(seen_by_b.audit().version, 1);

        let mut by_a = serde_json::Map::new();
        by_a.insert("description".to_owned(), json!("written by A"));
        writer_a.update(&project_id, 1, &by_a, &claude()).unwrap();

        let mut by_b = serde_json::Map::new();
        by_b.insert("description".to_owned(), json!("written by B"));
        let refused = writer_b.update(&project_id, 1, &by_b, &claude());
        assert!(
            matches!(refused, Err(Error::StaleVersion { latest: 2, .. })),
            "B's write landed on top of A's: {refused:?}"
        );

        let Entity::Project(final_state) = writer_a.get(&project_id).unwrap().unwrap() else {
            panic!("a project id must read back a project");
        };
        assert_eq!(final_state.description.as_deref(), Some("written by A"));
    }

    #[test]
    fn archiving_at_a_stale_version_is_refused() {
        let mut s = store();
        let project_id = project(&mut s);
        assert!(matches!(
            s.archive(&project_id, 7, &claude()),
            Err(Error::StaleVersion { supplied: 7, .. })
        ));
    }

    #[test]
    fn archiving_something_already_archived_returns_it_unchanged() {
        let mut s = store();
        let project_id = project(&mut s);
        let once = s.archive(&project_id, 1, &claude()).unwrap();
        let twice = s
            .archive(&project_id, once.audit().version, &claude())
            .unwrap();
        assert_eq!(once.audit().version, twice.audit().version);
    }

    // --- Task lifecycle --------------------------------------------------

    /// Everything a close needs, so the terminal-status guard is satisfied.
    fn closing_changes() -> serde_json::Map<String, serde_json::Value> {
        let mut changes = serde_json::Map::new();
        changes.insert("status".to_owned(), json!("done"));
        changes.insert("close_reason".to_owned(), json!("done"));
        changes.insert("close_message".to_owned(), json!("Built and tested."));
        changes.insert("evidence".to_owned(), json!(["commit:abc1234"]));
        changes
    }

    fn a_task(s: &mut Store, project_id: &EntityId, title: &str) -> EntityId {
        s.create(
            Task::new(project_id.clone(), title, "A row this test needs.").into(),
            &claude(),
        )
        .unwrap()
        .entity
        .id()
        .clone()
    }

    #[test]
    fn closing_a_task_stamps_closed_at_and_reopening_clears_it() {
        let mut s = store();
        let project_id = project(&mut s);
        let task_id = a_task(&mut s, &project_id, "Close me");

        let Entity::Task(closed) = s
            .update(&task_id, 1, &closing_changes(), &claude())
            .unwrap()
        else {
            panic!("a task id must read back a task");
        };
        assert!(
            closed.closed_at.is_some(),
            "a terminal status must stamp a completion date"
        );
        assert_eq!(closed.close_reason, Some(CloseReason::Done));

        let mut reopen = serde_json::Map::new();
        reopen.insert("status".to_owned(), json!("in_progress"));
        let Entity::Task(reopened) = s.update(&task_id, 2, &reopen, &claude()).unwrap() else {
            panic!("a task id must read back a task");
        };
        assert_eq!(
            reopened.closed_at, None,
            "a reopened task counts as closed in every query that filters on a date"
        );
    }

    #[test]
    fn a_terminal_status_releases_the_claim_even_by_a_plain_update() {
        let mut s = store();
        let project_id = project(&mut s);
        let task_id = a_task(&mut s, &project_id, "Claim me");

        let mut claim = serde_json::Map::new();
        claim.insert("claimed_by".to_owned(), json!("ses_test"));
        claim.insert("claimed_at".to_owned(), json!(Utc::now()));
        claim.insert("status".to_owned(), json!("in_progress"));
        s.update(&task_id, 1, &claim, &claude()).unwrap();

        let Entity::Task(done) = s
            .update(&task_id, 2, &closing_changes(), &claude())
            .unwrap()
        else {
            panic!("a task id must read back a task");
        };
        assert_eq!(done.claimed_by, None, "a finished task is not claimed");
        assert_eq!(done.claimed_at, None);
    }

    #[test]
    fn a_terminal_status_with_no_reason_is_refused() {
        let mut s = store();
        let project_id = project(&mut s);
        let task_id = a_task(&mut s, &project_id, "Refuse me");

        let mut bare = serde_json::Map::new();
        bare.insert("status".to_owned(), json!("done"));
        let err = s
            .update(&task_id, 1, &bare, &claude())
            .expect_err("done with no reason must be refused");
        assert!(err.chain().contains("close_reason"), "{}", err.chain());

        // And the refusal must not have written a version bump.
        assert_eq!(s.get(&task_id).unwrap().unwrap().audit().version, 1);
    }

    #[test]
    fn done_with_a_reason_but_no_evidence_is_refused() {
        let mut s = store();
        let project_id = project(&mut s);
        let task_id = a_task(&mut s, &project_id, "No evidence");

        let mut changes = serde_json::Map::new();
        changes.insert("status".to_owned(), json!("done"));
        changes.insert("close_reason".to_owned(), json!("done"));
        changes.insert("close_message".to_owned(), json!("It is finished."));
        let err = s
            .update(&task_id, 1, &changes, &claude())
            .expect_err("done with nothing to show for it must be refused");
        assert!(err.chain().contains("evidence"), "{}", err.chain());
    }

    #[test]
    fn a_status_change_records_a_status_changed_event_per_field() {
        let mut s = store();
        let project_id = project(&mut s);
        let task_id = a_task(&mut s, &project_id, "Move me");

        let mut changes = serde_json::Map::new();
        changes.insert("status".to_owned(), json!("in_progress"));
        changes.insert("priority".to_owned(), json!("p1"));
        s.update(&task_id, 1, &changes, &claude()).unwrap();

        let history = s.events_for(&task_id, 100).unwrap();
        let changed: Vec<&Event> = history
            .items
            .iter()
            .filter(|e| e.action == Action::StatusChanged)
            .collect();
        assert_eq!(
            changed.len(),
            2,
            "one event per changed field, so a feed can be filtered by field later"
        );
        assert!(changed.iter().any(|e| e.field.as_deref() == Some("status")));
        assert!(
            changed
                .iter()
                .any(|e| e.field.as_deref() == Some("priority"))
        );
    }

    #[test]
    fn a_task_cannot_become_its_own_ancestor() {
        let mut s = store();
        let project_id = project(&mut s);
        let a = a_task(&mut s, &project_id, "Parent work");
        let b = a_task(&mut s, &project_id, "Child work");

        let mut under_a = serde_json::Map::new();
        under_a.insert("parent_id".to_owned(), json!(b.as_str()));
        s.update(&a, 1, &under_a, &claude()).unwrap();

        // A is now under B; moving B under A would close the cycle. Caught on
        // update, not only on create — the later parent is the dangerous one.
        let mut under_b = serde_json::Map::new();
        under_b.insert("parent_id".to_owned(), json!(a.as_str()));
        let err = s
            .update(&b, 1, &under_b, &claude())
            .expect_err("a cycle must be refused");
        assert!(err.chain().contains("ancestor"), "{}", err.chain());
    }

    // --- Numbers ---------------------------------------------------------

    #[test]
    fn a_number_is_never_reused_after_a_row_is_archived() {
        let mut s = store();
        let project_id = project(&mut s);

        let first = a_task(&mut s, &project_id, "The first piece of work");
        s.archive(&first, 1, &claude()).unwrap();

        let second = a_task(&mut s, &project_id, "A completely separate thing");
        let Entity::Task(t) = s.get(&second).unwrap().unwrap() else {
            panic!("a task id must read back a task");
        };
        assert_eq!(
            t.number, 2,
            "SPEC-1 must mean the same task forever, archived or not"
        );
    }

    #[test]
    fn a_readable_reference_resolves_and_nonsense_resolves_to_nothing() {
        let mut s = store();
        let project_id = project(&mut s);
        let task_id = a_task(&mut s, &project_id, "Resolve me");

        assert_eq!(s.resolve_ref("SPEC-1").unwrap().as_ref(), Some(&task_id));
        assert_eq!(
            s.resolve_ref(task_id.as_str()).unwrap().as_ref(),
            Some(&task_id)
        );
        // Naming nothing is a legitimate answer, distinct from a malformed
        // reference.
        assert_eq!(s.resolve_ref("SPEC-999").unwrap(), None);
        assert_eq!(s.resolve_ref("not a reference at all").unwrap(), None);
    }

    #[test]
    fn a_second_project_with_the_same_letters_gets_a_distinct_key() {
        let mut s = store();
        project(&mut s);
        // Shares the first four letters with the fixture project's slug, which
        // is what makes the keys collide. Pick the pair deliberately: two slugs
        // that happen not to collide turn this into a test of nothing.
        let other = s
            .create(Project::new("specboat", "Specboat").into(), &claude())
            .unwrap();
        let Entity::Project(p) = other.entity else {
            panic!("a project create must return a project");
        };
        assert_eq!(
            p.key, "SPEC2",
            "project keys are unique, suffixed until free"
        );
    }

    #[test]
    fn ranks_are_fractional_so_a_move_touches_one_row() {
        let s = store();
        assert_eq!(s.rank_between(Some(1.0), Some(2.0)).unwrap(), 1.5);
        assert_eq!(s.rank_between(Some(4.0), None).unwrap(), 5.0);
        assert_eq!(s.rank_between(None, Some(3.0)).unwrap(), 2.0);
        assert_eq!(s.rank_between(None, None).unwrap(), 1.0);
        assert!(
            s.rank_between(Some(2.0), Some(2.0)).is_err(),
            "there is no space between two identical ranks"
        );
    }

    // --- Links -----------------------------------------------------------

    /// The stored rel and endpoints of every live link, read straight from the
    /// table so the assertion does not depend on `GraphStore`.
    fn stored_links(s: &Store) -> Vec<(String, String, String, bool)> {
        let mut stmt = s
            .connection()
            .prepare("SELECT from_id, rel, to_id, archived_at FROM links ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?.is_some(),
                ))
            })
            .unwrap();
        rows.collect::<std::result::Result<Vec<_>, _>>().unwrap()
    }

    #[test]
    fn depends_on_is_stored_as_blocks_with_the_endpoints_swapped() {
        let mut s = store();
        let project_id = project(&mut s);
        let a = a_task(&mut s, &project_id, "The dependent piece of work");
        let b = a_task(&mut s, &project_id, "The thing it waits for");

        // "A depends on B" must be stored as "B blocks A".
        let stored = s
            .link(
                NewLink::new(a.clone(), Relation::DependsOn, b.clone()),
                &claude(),
            )
            .unwrap();
        assert_eq!(stored.rel, Relation::Blocks);
        assert_eq!(stored.from_id, b);
        assert_eq!(stored.to_id, a);

        let rows = stored_links(&s);
        assert_eq!(rows.len(), 1, "only one edge is ever stored");
        assert_eq!(
            rows[0],
            (
                b.as_str().to_owned(),
                "blocks".to_owned(),
                a.as_str().to_owned(),
                false
            ),
            "`depends_on` must never appear in the table"
        );
    }

    #[test]
    fn linking_the_same_edge_twice_returns_the_first_one() {
        let mut s = store();
        let project_id = project(&mut s);
        let a = a_task(&mut s, &project_id, "The blocking piece of work");
        let b = a_task(&mut s, &project_id, "The blocked piece of work");

        let first = s
            .link(
                NewLink::new(a.clone(), Relation::Blocks, b.clone()),
                &claude(),
            )
            .unwrap();
        let again = s
            .link(
                NewLink::new(a.clone(), Relation::Blocks, b.clone()),
                &claude(),
            )
            .unwrap();
        assert_eq!(first.id, again.id);
        assert_eq!(stored_links(&s).len(), 1);
    }

    #[test]
    fn an_edge_to_something_that_does_not_exist_is_refused() {
        let mut s = store();
        let project_id = project(&mut s);
        let a = a_task(&mut s, &project_id, "A real piece of work");
        let ghost = EntityId::generate(EntityType::Task);

        let err = s
            .link(NewLink::new(a, Relation::Blocks, ghost), &claude())
            .expect_err("an edge to nothing must be refused");
        assert!(err.chain().contains("no task exists"), "{}", err.chain());
    }

    #[test]
    fn unlinking_soft_deletes_and_unlinking_nothing_is_refused() {
        let mut s = store();
        let project_id = project(&mut s);
        let a = a_task(&mut s, &project_id, "The blocking piece of work");
        let b = a_task(&mut s, &project_id, "The blocked piece of work");
        s.link(
            NewLink::new(a.clone(), Relation::Blocks, b.clone()),
            &claude(),
        )
        .unwrap();

        s.unlink(&a, Relation::Blocks, &b, "", &claude()).unwrap();
        let rows = stored_links(&s);
        assert_eq!(rows.len(), 1, "nothing is ever DELETEd, links included");
        assert!(rows[0].3, "the edge must be archived rather than removed");

        assert!(
            s.unlink(&a, Relation::Blocks, &b, "", &claude()).is_err(),
            "unlinking an edge that is not live must say so"
        );
    }

    #[test]
    fn archiving_a_row_archives_its_links_and_leaves_its_children_alone() {
        let mut s = store();
        let project_id = project(&mut s);
        let parent = a_task(&mut s, &project_id, "The parent piece of work");
        let other = a_task(&mut s, &project_id, "Something else it blocks");
        let child = a_task(&mut s, &project_id, "The child piece of work");

        let mut under_parent = serde_json::Map::new();
        under_parent.insert("parent_id".to_owned(), json!(parent.as_str()));
        s.update(&child, 1, &under_parent, &claude()).unwrap();

        s.link(
            NewLink::new(parent.clone(), Relation::Blocks, other.clone()),
            &claude(),
        )
        .unwrap();

        s.archive(&parent, 1, &claude()).unwrap();

        let rows = stored_links(&s);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].3, "the links touching an archived row are archived");

        // The child survives. An orphan is untidy and `fsck` reports it; a
        // cascade would be unrecoverable.
        let surviving = s.get(&child).unwrap().expect("the child must still exist");
        assert!(
            !surviving.audit().is_archived(),
            "archiving a parent must never archive its children (SPEC §3.1)"
        );
    }

    // --- Events ----------------------------------------------------------

    #[test]
    fn events_page_from_a_cursor_and_visit_each_event_exactly_once() {
        let mut s = store();
        let project_id = project(&mut s);
        for n in 1..=5 {
            a_task(&mut s, &project_id, &format!("Piece of work number {n}"));
        }

        let mut seen: Vec<EventId> = Vec::new();
        let mut cursor = Cursor::Beginning;
        loop {
            let page = s.events(&cursor, Some(&project_id), 2).unwrap();
            if page.items.is_empty() {
                break;
            }
            let last = page.items.last().map(|e| e.id.clone());
            seen.extend(page.items.into_iter().map(|e| e.id));
            match last {
                Some(id) => cursor = Cursor::After(id),
                None => break,
            }
        }

        let mut deduped = seen.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(
            seen.len(),
            deduped.len(),
            "a cursor must not repeat an event"
        );
        assert_eq!(
            seen.len(),
            s.events(&Cursor::Beginning, Some(&project_id), 1_000)
                .unwrap()
                .total,
            "a cursor must not skip an event either"
        );
        assert!(
            seen.windows(2).all(|w| w[0] < w[1]),
            "ascending, oldest first"
        );
    }

    #[test]
    fn a_page_of_events_reports_that_it_was_cut() {
        let mut s = store();
        let project_id = project(&mut s);
        for n in 1..=4 {
            a_task(&mut s, &project_id, &format!("Piece of work number {n}"));
        }
        let page = s.events(&Cursor::Beginning, Some(&project_id), 2).unwrap();
        assert_eq!(page.items.len(), 2);
        assert!(page.truncated, "hard constraint 4: say so, with a total");
        assert!(page.total > 2);
    }

    #[test]
    fn the_latest_event_id_is_the_largest_one() {
        let mut s = store();
        assert_eq!(
            s.latest_event_id().unwrap(),
            None,
            "an empty store has none"
        );

        let project_id = project(&mut s);
        a_task(&mut s, &project_id, "Something to log");

        let all = s.events(&Cursor::Beginning, None, 1_000).unwrap();
        let largest = all.items.iter().map(|e| e.id.clone()).max();
        assert_eq!(s.latest_event_id().unwrap(), largest);
    }

    #[test]
    fn a_lists_truncation_is_reported_with_a_total() {
        let mut s = store();
        let project_id = project(&mut s);
        for n in 1..=5 {
            a_task(&mut s, &project_id, &format!("Piece of work number {n}"));
        }
        let page = s
            .list(
                &EntityQuery::in_project(project_id)
                    .of_type(EntityType::Task)
                    .limited(2),
            )
            .unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total, 5);
        assert!(page.truncated);
    }

    #[test]
    fn a_list_excludes_archived_rows_unless_asked() {
        let mut s = store();
        let project_id = project(&mut s);
        let task_id = a_task(&mut s, &project_id, "Gone but not deleted");
        s.archive(&task_id, 1, &claude()).unwrap();

        let live = s
            .list(&EntityQuery::in_project(project_id.clone()).of_type(EntityType::Task))
            .unwrap();
        assert_eq!(live.total, 0);

        let everything = EntityQuery {
            include_archived: true,
            ..EntityQuery::in_project(project_id).of_type(EntityType::Task)
        };
        assert_eq!(s.list(&everything).unwrap().total, 1);
    }

    // --- Notes -----------------------------------------------------------

    #[test]
    fn a_note_against_a_subject_that_does_not_exist_is_refused() {
        let mut s = store();
        let ghost = EntityId::generate(EntityType::Task);
        let err = s
            .add_note(
                NewNote::new(ghost, "a finding with nowhere to hang", Actor::Claude),
                &claude(),
            )
            .expect_err("a note pointing at nothing must be refused");
        let message = err.chain();
        assert!(
            message.contains("cannot be annotated"),
            "the message must say why: {message}"
        );
    }

    #[test]
    fn a_note_against_an_archived_row_is_refused() {
        let mut s = store();
        let project_id = project(&mut s);
        let task_id = a_task(&mut s, &project_id, "Put away");
        s.archive(&task_id, 1, &claude()).unwrap();

        assert!(
            s.add_note(
                NewNote::new(task_id, "nobody will ever read this", Actor::Claude),
                &claude(),
            )
            .is_err()
        );
    }

    #[test]
    fn notes_read_back_in_order_and_retracted_ones_are_excluded_by_default() {
        let mut s = store();
        let project_id = project(&mut s);
        let task_id = a_task(&mut s, &project_id, "Annotate me");

        let first = s
            .add_note(
                NewNote::new(task_id.clone(), "the first finding", Actor::Claude),
                &claude(),
            )
            .unwrap();
        s.add_note(
            NewNote::new(task_id.clone(), "the second finding", Actor::Claude),
            &claude(),
        )
        .unwrap();

        let live = s.notes_for(&task_id, false).unwrap();
        assert_eq!(live.len(), 2);
        assert_eq!(live[0].body, "the first finding", "oldest first");
        assert_eq!(
            live[0].session_id.as_deref(),
            Some("ses_test"),
            "provenance decides attribution, not the caller's note"
        );

        s.retract_note(&first.id, &claude()).unwrap();
        assert_eq!(
            s.notes_for(&task_id, false).unwrap().len(),
            1,
            "the common caller is a renderer; making it filter is how retracted notes ship"
        );
        assert_eq!(s.notes_for(&task_id, true).unwrap().len(), 2);
        assert_eq!(s.notes_in_project(&project_id).unwrap().len(), 1);
    }

    #[test]
    fn retracting_a_note_twice_is_refused() {
        let mut s = store();
        let project_id = project(&mut s);
        let task_id = a_task(&mut s, &project_id, "Annotate me once");
        let note = s
            .add_note(NewNote::new(task_id, "a finding", Actor::Claude), &claude())
            .unwrap();

        s.retract_note(&note.id, &claude()).unwrap();
        assert!(
            matches!(
                s.retract_note(&note.id, &claude()),
                Err(Error::NotFound { .. })
            ),
            "a second retraction is the caller believing something false"
        );
    }

    #[test]
    fn a_note_on_a_global_term_is_accepted_even_with_no_project() {
        let mut s = store();
        let created = s
            .create(
                Term::new(None, "Digest", "The compact project summary.").into(),
                &claude(),
            )
            .unwrap();
        let note = s
            .add_note(
                NewNote::new(
                    created.entity.id().clone(),
                    "a global term belongs to every glossary",
                    Actor::Claude,
                ),
                &claude(),
            )
            .expect("v_entities spells `no project` as an empty string, not a bad id");
        assert_eq!(note.project_id, None);
    }

    // --- Validation ------------------------------------------------------

    #[test]
    fn a_milestone_with_no_explainer_is_refused_before_the_idempotency_lookup() {
        let mut s = store();
        let project_id = project(&mut s);
        let mut milestone = Milestone::new(project_id, "Phase 9", "placeholder");
        milestone.summary = None;

        let err = s
            .create(milestone.into(), &claude())
            .expect_err("a milestone with no summary must be refused");
        assert!(err.chain().contains("summary"), "{}", err.chain());
    }

    #[test]
    fn a_task_status_that_is_not_a_status_is_refused_by_name() {
        let mut s = store();
        let project_id = project(&mut s);
        let task_id = a_task(&mut s, &project_id, "Reject my status");

        let mut changes = serde_json::Map::new();
        changes.insert("status".to_owned(), json!("blocked"));
        let err = s
            .update(&task_id, 1, &changes, &claude())
            .expect_err("`blocked` has never been a status");
        assert!(err.chain().contains("status"), "{}", err.chain());
        assert_eq!(
            s.get(&task_id).unwrap().unwrap().audit().version,
            1,
            "a refused update must leave the row alone"
        );
    }

    #[test]
    fn a_task_with_no_summary_cannot_be_created_but_can_still_be_moved() {
        let mut s = store();
        let project_id = project(&mut s);

        let mut bare = Task::new(project_id.clone(), "No summary", "placeholder");
        bare.summary = None;
        assert!(
            s.create(bare.into(), &claude()).is_err(),
            "TQ-34: a summary is required where it can be met"
        );

        // The same rule must not run on update, or the ninety-four rows that
        // predate it could never be touched again.
        let task_id = a_task(&mut s, &project_id, "Has a summary");
        let mut blank = serde_json::Map::new();
        blank.insert("summary".to_owned(), serde_json::Value::Null);
        blank.insert("status".to_owned(), json!("in_progress"));
        let moved = s.update(&task_id, 1, &blank, &claude()).unwrap();
        assert_eq!(moved.audit().version, 2);
    }

    #[test]
    fn a_project_that_renames_milestones_to_tasks_is_refused() {
        let mut s = store();
        let mut p = Project::new("ambiguous", "Ambiguous");
        p.milestone_noun = Some("task".to_owned());
        assert!(
            s.create(p.into(), &claude()).is_err(),
            "a project noun that shadows a canonical type makes every create ambiguous"
        );
    }

    #[test]
    fn statuses_are_a_filter_and_types_without_one_are_excluded_rather_than_an_error() {
        let mut s = store();
        let project_id = project(&mut s);
        a_task(&mut s, &project_id, "Something to find");

        let page = s
            .list(&EntityQuery::in_project(project_id).with_status(["todo"]))
            .expect("a cross-type status filter is a reasonable thing to ask");
        assert!(
            page.items
                .iter()
                .all(|e| e.entity_type() != EntityType::Artifact),
            "types with no lifecycle are excluded, not errored on"
        );
        assert!(page.total >= 1);
    }
}
