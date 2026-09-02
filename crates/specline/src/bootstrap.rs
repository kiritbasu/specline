//! `specline bootstrap` — seed Specline's own project. The dogfooding switch.
//!
//! The handoff calls this out: once Phase 1 exits, Specline tracks its own
//! development, and `product/STATUS.md` becomes something `specline render-status`
//! generates rather than something a human maintains. This is the one-time
//! import that makes that possible.
//!
//! # Why this is hand-written rather than parsed
//!
//! Parsing `STATUS.md`, `DECISIONS.md` and `QUESTIONS.md` would be brittle in
//! a way that fails quietly — a heading renamed, a table column reordered, and
//! half the tasks silently vanish. It is also a one-time job: after this runs,
//! Specline is the source of truth and those files are outputs.
//!
//! So the content below is transcribed from the product docs as they stood
//! when Phases 0–3 were built. If it disagrees with them, the docs are right
//! and this is stale — but it should only ever be run once, into an empty
//! store.
//!
//! Everything goes through the ordinary write path, so this exercises
//! validation, idempotency, events and link normalisation. Running it twice
//! creates nothing.

use anyhow::Result;
use chrono::{NaiveDate, TimeZone, Utc};
use specline_core::{
    Actor, CloseReason, Decision, DecisionStatus, Document, EntityId, EntityStore, EntityType,
    Environment, EnvironmentStatus, Metric, MetricDirection, MetricObservation, Milestone,
    MilestoneStatus, NewLink, Project, Provenance, Question, QuestionKind, QuestionStatus,
    Relation, RiskSeverity, Spec, SpecKind, SpecStatus, Store, Surface, Task, TaskKind,
    TaskPriority, TaskStatus, Term,
};

/// What the bootstrap created.
pub struct Summary {
    /// The project's id.
    pub project_id: EntityId,
    /// Entities created.
    pub entities: usize,
    /// Links created.
    pub links: usize,
    /// Document revisions written.
    pub revisions: usize,
}

/// One link in the bootstrap's edge table: source kind and label, relation,
/// target kind and label, and an optional anchor such as `REQ-4`.
type Edge<'a> = (
    &'a str,
    &'a str,
    Relation,
    &'a str,
    &'a str,
    Option<&'a str>,
);

/// One task row: milestone index, title, body, status, priority, kind, labels.
type Row<'a> = (
    usize,
    &'a str,
    &'a str,
    TaskStatus,
    TaskPriority,
    TaskKind,
    &'a [&'a str],
);

/// Seed the Specline project.
pub fn run(store: &mut Store, repo_path: Option<String>) -> Result<Summary> {
    // Attributed to Claude Code, because that is what wrote it, and to a
    // session id that says where it came from. G3 is the whole provenance
    // guarantee and a bootstrap is not exempt from it.
    let prov = Provenance {
        actor: Actor::Claude,
        session_id: Some("ses_bootstrap_2026_08_09".to_owned()),
        surface: Some(Surface::Code),

        client: None,
    };
    let mut entities = 0usize;
    let mut links = 0usize;
    let mut revisions = 0usize;

    let mut project = Project::new("specline", "Specline");
    project.description = Some(
        "Local-first store for everything that describes a software project other than the \
         code — specs, decisions, tasks, roadmap, design, feedback. An MCP server is the \
         primary interface; a Tauri desktop app is the read surface."
            .to_owned(),
    );
    project.repo_urls = vec!["https://github.com/kb/specline".to_owned()];
    project.aliases = vec!["the project spine".to_owned(), "project spine".to_owned()];
    project.root_path = repo_path;
    // The tracker is generated from the task and milestone rows, so the file
    // it lands in is a property of the project rather than of any one
    // artifact — nothing in Specline *is* `product/STATUS.md` the way the spec
    // artifact is `product/SPEC.md`.
    project.status_path = Some("product/STATUS.md".to_owned());
    let project_id = store.create(project.into(), &prov)?.entity.id().clone();
    entities += 1;

    // --- Phases as milestones --------------------------------------------
    let phases: [(&str, &str, MilestoneStatus, Option<NaiveDate>, i32); 6] = [
        (
            "Phase 0 — Spine",
            "Storage, schema, event log, graph, search, backup. No network, no UI.",
            MilestoneStatus::Shipped,
            NaiveDate::from_ymd_opt(2026, 8, 9),
            0,
        ),
        (
            "Phase 1 — Daemon",
            "axum, the ten MCP tools, specline_context, concurrency safety, render-status.",
            MilestoneStatus::Shipped,
            NaiveDate::from_ymd_opt(2026, 8, 9),
            1,
        ),
        (
            "Phase 2 — Plugin",
            "Skill, session-ID threading, project confirmation, mirror hooks. Built, but the \
             exit gate needs ten unprompted sessions and has not been run.",
            MilestoneStatus::Open,
            NaiveDate::from_ymd_opt(2026, 8, 9),
            2,
        ),
        (
            "Phase 3 — Desktop",
            "Tauri shell, daemon as sidecar, screens 1–6 and 9.",
            MilestoneStatus::Shipped,
            NaiveDate::from_ymd_opt(2026, 8, 9),
            3,
        ),
        (
            "Phase 4 — Integrations",
            "GitHub App, design artifacts, metrics charts. Needs KB's GitHub account.",
            MilestoneStatus::Open,
            None,
            4,
        ),
        (
            "Phase 5 — Remote",
            "Deployable daemon, auth, mobile client.",
            MilestoneStatus::Open,
            None,
            5,
        ),
    ];
    let mut milestones = Vec::new();
    for (name, summary, status, target, order) in phases {
        let mut m = Milestone::new(project_id.clone(), name, summary);
        m.status = status;
        m.target_date = target;
        m.sort_order = Some(order);
        if status == MilestoneStatus::Shipped {
            m.shipped_at = Some(Utc.with_ymd_and_hms(2026, 8, 9, 12, 0, 0).unwrap());
        }
        milestones.push(store.create(m.into(), &prov)?.entity.id().clone());
        entities += 1;
    }

    // --- Tasks, from STATUS.md -------------------------------------------
    let done = TaskStatus::Done;
    let todo = TaskStatus::Todo;
    let rows: Vec<Row<'_>> = vec![
        // Phase 0
        (
            0,
            "Cargo workspace scaffold, CI, lint/fmt/deny gates",
            "Five crates from SPEC §1.1. unwrap/expect/panic are workspace clippy lints, so the definition of done is a build failure rather than review discipline.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["build"],
        ),
        (
            0,
            "Verify fast-moving dependencies",
            "Verified against running code, not docs. DuckDB 1.5.5 + Lance work end to end; DuckPGQ confirmed absent for 1.5.x (HTTP 404); MCP 2026-07-28 current. Two SPEC §5 syntax errors found and fixed.",
            done,
            TaskPriority::P0,
            TaskKind::Spike,
            &["risk"],
        ),
        (
            0,
            "Domain types, ULID prefixes, the audit block",
            "Thirteen types, prefixed ULIDs, relations, events, document revisions.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["core"],
        ),
        (
            0,
            "DuckDB schema and forward-only migrations",
            "Recorded in _keel_migrations. v_entities built now rather than deferred.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["storage"],
        ),
        (
            0,
            "Lance documents and blobs datasets, ATTACH wiring",
            "Created through the DuckDB extension; no Lance Rust crate needed. Lance CREATE TABLE rejects all column constraints.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["storage"],
        ),
        (
            0,
            "Entity storage layer — CRUD for all 13 types",
            "Field patching goes through a serde round-trip, so enum errors already name the valid values.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["storage"],
        ),
        (
            0,
            "Document revisions — append, fetch by version, diff",
            "Identical content does not grow the history, because the mirror hook regenerates and re-reads constantly.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["storage"],
        ),
        (
            0,
            "Links, GraphStore trait, recursive CTE traversal",
            "21 tests: all nine relations, both directions, plus both inversions. An inverted traversal returns an empty set that looks legitimate.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["graph", "correctness"],
        ),
        (
            0,
            "Event log — append, query since cursor",
            "Cursor paging visits every event exactly once. Only holds because ULIDs are minted monotonically.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["core"],
        ),
        (
            0,
            "Embeddings via fastembed",
            "Behind an Embedder trait, passed in rather than constructed, so tests use a deterministic hash embedder instead of downloading 130 MB.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["search"],
        ),
        (
            0,
            "Hybrid search — BM25 plus vectors, RRF fusion",
            "BM25 lives in DuckDB, not Lance. lance_hybrid_search's keyword half could not be characterised.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["search"],
        ),
        (
            0,
            "Backup: DuckDB and Lance to Parquet, restore",
            "Both engines. Restore refuses a backup missing its Lance half, and refuses to overwrite an existing store.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["backup", "risk"],
        ),
        (
            0,
            "specline fsck — cross-engine referential integrity",
            "27 checks. Every finding says what it breaks and what to do about it.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["correctness"],
        ),
        (
            0,
            "200-entity fixture across all types and relations",
            "212 entities, 31 links, 52 revisions across three projects. Loaded through the ordinary write path.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["testing"],
        ),
        (
            0,
            "Test suite: concurrency, idempotency, OCC, round-trip",
            "Concurrency test written in Phase 0 as ignored, made real in Phase 1.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["testing"],
        ),
        (
            0,
            "Implement idempotency keys and optimistic concurrency",
            "Derived keys normalise whitespace and case. OCC enforced by WHERE version = ?, not a read-then-write.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["correctness"],
        ),
        (
            0,
            "Monotonic ULID generation",
            "Plain ULIDs are not ordered within a millisecond, which would make the event cursor silently skip rows.",
            done,
            TaskPriority::P0,
            TaskKind::Bug,
            &["correctness"],
        ),
        // Phase 1
        (
            1,
            "JSON-RPC and the stateless Streamable HTTP transport",
            "Header/body validation with the renumbered error codes. GET/DELETE answered 405.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["daemon", "mcp"],
        ),
        (
            1,
            "server/discover and tools/list",
            "Both required. Deterministic tool order for prompt-cache hits.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["daemon", "mcp"],
        ),
        (
            1,
            "The nine tool schemas",
            "Descriptions say when to reach for a tool, not just what it does. A test enforces that.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["mcp"],
        ),
        (
            1,
            "specline_context — the digest",
            "3–4k tokens. Questions and glossary terms are never trimmed; everything else degrades and reports what it dropped.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["mcp"],
        ),
        (
            1,
            "Read tools: search, get, activity, projects",
            "specline_get takes version and diff_against, so REQ-2's diff requirement is met at the API layer.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["mcp"],
        ),
        (
            1,
            "Write tools: create, update, write_doc, link",
            "409 carries latest_version, current state and events_since, so a losing writer can usually merge.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["mcp"],
        ),
        (
            1,
            "Shared single write path",
            "One store, one mutex, whole process. Held across synchronous work, never across an await.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["daemon"],
        ),
        (
            1,
            "Local REST and SSE for the desktop app",
            "Specline's own API, dispatching through the same tool layer as MCP so the two cannot drift.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["daemon"],
        ),
        (
            1,
            "Concurrency gate: zero duplicates, zero lost updates",
            "16 concurrent sessions. Exactly one create wins; all 16 updates land under retry; the event log stays gapless.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["testing"],
        ),
        (
            1,
            "Snapshot tests for every tool response",
            "They are an API contract, and drift should show up in a diff rather than as an agent behaving differently.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["testing"],
        ),
        (
            1,
            "specline render-status",
            "The dogfooding switch. Not the §8 mirror, which is prose-only.",
            done,
            TaskPriority::P2,
            TaskKind::Task,
            &["cli"],
        ),
        (
            1,
            "Scripted UC-1 to UC-4 harness",
            "21 tests driving real HTTP against a real daemon. The automated proxy for a gate that needs a human.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["testing"],
        ),
        (
            1,
            "Serve MCP 2025-11-25 as well as 2026-07-28",
            "Claude Code 2.1.185 opens with the legacy initialize handshake. A daemon speaking only the current revision reports Failed to connect and is unusable with its primary client.",
            done,
            TaskPriority::P0,
            TaskKind::Bug,
            &["daemon", "mcp", "interop"],
        ),
        // Phase 2
        (
            2,
            "Markdown mirror generator",
            "Prose only. The module contains no way to read a mirror as truth, and a test asserts that absence.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["plugin", "mirror"],
        ),
        (
            2,
            "The skill that teaches Claude when to write",
            "Leads with orientation, then session threading, then a when-this-happens-write-that table.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["plugin"],
        ),
        (
            2,
            "PostToolUse hook for mirror edits",
            "Verified end to end: an edit typed into a generated spec came back as revision 2, attributed to the session.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["plugin", "mirror"],
        ),
        (
            2,
            "Project-confirmation behaviour",
            "specline_projects returns requires_confirmation on a near miss; the skill makes this the one place the agent must stop and ask.",
            done,
            TaskPriority::P0,
            TaskKind::Task,
            &["plugin"],
        ),
        (
            2,
            "MCP config and install script",
            "The installer prints the configuration rather than editing anyone's settings file.",
            done,
            TaskPriority::P2,
            TaskKind::Task,
            &["plugin"],
        ),
        (
            2,
            "Run the ten unprompted sessions",
            "The exit gate, and the one that tests the premise. Cannot be automated: a test that calls the tool has prompted it. Needs KB.",
            todo,
            TaskPriority::P0,
            TaskKind::Task,
            &["plugin", "gate"],
        ),
        // Phase 3
        (
            3,
            "Tauri v2 shell with the daemon as a sidecar",
            "Starts the daemon only if one is not already running, and only kills what it started.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            3,
            "Screen 1 — Home, all projects at a glance",
            "At-risk projects sort first. This is the Sunday-review screen.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            3,
            "Screen 2 — Project dashboard",
            "The same data specline_context gives an agent, so a human and a model cannot be looking at different summaries.",
            done,
            TaskPriority::P1,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            3,
            "Screen 3 — Roadmap",
            "Built from milestones. Dated first in date order, undated after.",
            done,
            TaskPriority::P2,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            3,
            "Screen 4 — Board",
            "Tasks by status, filterable, keyboard-driven.",
            done,
            TaskPriority::P2,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            3,
            "Screen 5 — Documents with revision diff",
            "The diff is why this screen exists rather than being a markdown viewer.",
            done,
            TaskPriority::P2,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            3,
            "Screen 6 — Search",
            "Faceted by type, scoped by project. Hits say which index found them.",
            done,
            TaskPriority::P2,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            3,
            "Screen 9 — Activity feed",
            "Filterable by actor. Writes with no session_id are marked unattributed.",
            done,
            TaskPriority::P2,
            TaskKind::Task,
            &["desktop"],
        ),
        // Phase 4 and 5
        (
            4,
            "GitHub App and webhook receiver",
            "Blocked: registering the App needs KB's GitHub account and credentials.",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["github"],
        ),
        (
            4,
            "Design artifacts with stored images",
            "Blocked behind TQ-6: how an image gets into Specline from a Claude chat session, where there is no filesystem.",
            TaskStatus::Todo,
            TaskPriority::P3,
            TaskKind::Task,
            &["design"],
        ),
        (
            4,
            "Metric observations charted against target",
            "The data model is done; this is the rendering.",
            todo,
            TaskPriority::P3,
            TaskKind::Task,
            &["desktop", "metrics"],
        ),
        (
            5,
            "Deployable daemon with auth",
            "Blocked: needs hosting and auth decisions from KB.",
            TaskStatus::Todo,
            TaskPriority::P3,
            TaskKind::Task,
            &["remote"],
        ),
        // Open work not tied to a phase
        (
            2,
            "Decide TQ-9: idempotency_key on all thirteen tables",
            "The one storage-format change made without KB. Dropping twelve columns is a forward-only migration if he disagrees.",
            todo,
            TaskPriority::P1,
            TaskKind::Task,
            &["decision-needed"],
        ),
        (
            2,
            "Decide TQ-10: BM25 in DuckDB rather than Lance",
            "A SPEC §5 design change. The swap back is one module if KB prefers to wait for the Lance extension to document its FTS indexing.",
            todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["decision-needed", "search"],
        ),
        (
            2,
            "Decide TQ-11: how long to carry the 2025-11-25 handshake",
            "Needed today. Worth revisiting once clients move on.",
            todo,
            TaskPriority::P3,
            TaskKind::Task,
            &["decision-needed", "mcp"],
        ),
    ];

    let mut tasks: Vec<(String, EntityId)> = Vec::new();
    for (phase, title, body, status, priority, kind, labels) in rows {
        let mut t = Task::new(
            project_id.clone(),
            title,
            "A row this test needs in the store.",
        );
        t.body = Some(body.to_owned());
        // The body is the summary here. These rows carry real prose written
        // when the work was planned, and duplicating it into a second field
        // would give two answers to one question. The create path refuses an
        // empty or title-restating summary either way, so a bad body is caught
        // rather than smuggled in through the back door.
        t.summary = Some(body.to_owned());
        t.status = status;
        t.priority = priority;
        t.kind = kind;
        t.labels = labels.iter().map(|l| (*l).to_owned()).collect();
        t.milestone_id = milestones.get(phase).cloned();
        if status == TaskStatus::Done {
            t.closed_at = Some(Utc.with_ymd_and_hms(2026, 8, 9, 12, 0, 0).unwrap());
            // A create into a terminal status is held to the same rule as a
            // close (KEEL-217), so these rows say why they are finished. The
            // body is the message because it is what was written about the work
            // at the time. The evidence is the repository rather than a commit
            // per row: this is a transcription of Phases 0–3 from `STATUS.md`,
            // which recorded no shas, and inventing forty of them would put
            // fiction in the one field that exists to be checkable.
            t.close_reason = Some(CloseReason::Done);
            t.close_message = Some(body.to_owned());
            t.evidence = vec!["url:https://github.com/kb/specline".to_owned()];
        }
        tasks.push((
            title.to_owned(),
            store.create(t.into(), &prov)?.entity.id().clone(),
        ));
        entities += 1;
    }

    // --- Specs -----------------------------------------------------------
    let specs_src: [(&str, SpecKind, SpecStatus, &str); 3] = [
        (
            "Specline — Product Requirements Document",
            SpecKind::Prd,
            SpecStatus::Approved,
            "See `product/PRD.md` in the repository for the full text.\n\n\
          ## REQ-3 Agent orientation\n\nA single `specline_context` call returns a project digest \
          sized to fit comfortably in an agent's context window.\n\n\
          ## REQ-4 Hybrid search\n\nSemantic and keyword search spans every artifact type that \
          carries text, across all projects.\n\n\
          ## REQ-7 Concurrent writes are safe\n\nCreates are idempotent by key; updates use \
          optimistic concurrency and reject stale writes.\n\n\
          ## REQ-11 Backup\n\nThe whole store is backed up as a single restorable unit.",
        ),
        (
            "Specline — Technical Specification",
            SpecKind::Spec,
            SpecStatus::Approved,
            "*Placeholder. Run `specline import product/SPEC.md --project specline` to replace this with \
          the real document — it lands here as a new revision because the title matches.*\n\n\
          Two corrections were made against running code during the build: §5's Lance call \
          syntax, and §6's claim that the MCP surface is built on 2026-07-28 alone. Both are \
          annotated in place.",
        ),
        (
            "Phase gates that cannot be verified without a human",
            SpecKind::Note,
            SpecStatus::Draft,
            "Phase 2's criterion — nine of ten *unprompted* sessions write to Specline — cannot be \
          automated. \"Unprompted\" is the whole claim, and a test that calls the tool has \
          prompted it.\n\n\
          Phase 1's UC-1→UC-4 gate passes mechanically: 21 tests drive a real daemon over real \
          HTTP. What that does not prove is the part only a model can demonstrate — that the \
          tool descriptions lead an agent to the right tool unprompted. A scripted client is \
          told which tool to call.\n\n\
          `plugin/README.md` has the protocol for running the ten sessions, and what each \
          failure mode means in terms of which part of `SKILL.md` to change.",
        ),
    ];
    let mut specs: Vec<(String, EntityId)> = Vec::new();
    for (title, kind, status, body) in specs_src {
        let mut sp = Spec::new(project_id.clone(), title);
        sp.kind = kind;
        sp.status = status;
        let id = store.create(sp.into(), &prov)?.entity.id().clone();
        entities += 1;
        write_doc(store, &id, &project_id, title, body, &prov)?;
        revisions += 1;
        specs.push((title.to_owned(), id));
    }

    // --- Decisions, from DECISIONS.md ------------------------------------
    let decisions_src: [(&str, &str); 12] = [
        (
            "chrono for time, not jiff",
            "## Decision\n\n`chrono`.\n\n## Reasoning\n\nduckdb-rs ships a first-class chrono \
          feature with ToSql/FromSql for TIMESTAMP; there is no jiff feature. Choosing jiff \
          would mean a conversion shim at every storage boundary — the exact place a timezone \
          bug would hide — for no domain benefit.",
        ),
        (
            "All Lance access goes through the DuckDB extension",
            "## Decision\n\nNo `lance` or `lancedb` Rust crate.\n\n## Reasoning\n\nVerified that \
          ATTACH (TYPE lance) gives full SELECT/INSERT/UPDATE, and that the search functions \
          work. One connection, one SQL surface, one transaction story — and it drops lance v10 \
          and arrow v59 from the build.\n\n## Consequences\n\nDocumentStore is a trait precisely \
          so this can be swapped.",
        ),
        (
            "Bundled DuckDB is a feature, not a requirement",
            "## Context\n\nCompiling DuckDB from source costs about ten minutes on a cold build.\n\n\
          ## Decision\n\n`bundled` on by default, `--no-default-features` links a system \
          libduckdb.\n\n## Reasoning\n\nThe original justification overstated it: INSTALL lance \
          re-fetches for whatever version is running, so a system library self-heals. The real \
          reasons are a self-contained binary that survives `brew upgrade duckdb`, and a build \
          that needs no setup on a fresh machine — both about the *installed* binary.\n\n\
          ## Consequences\n\nSystem-linked builds the workspace in 54s versus roughly ten \
          minutes, with all tests passing either way.",
        ),
        (
            "ULIDs are minted from a monotonic generator",
            "## Context\n\n`Ulid::new()` re-randomises its low 80 bits on every call, so two ids \
          created in the same millisecond sort arbitrarily.\n\n## Decision\n\nOne process-wide \
          monotonic generator.\n\n## Reasoning\n\nSPEC §3.4 rests on ULID order *being* \
          chronological order, so that \"what changed since T\" is a range scan. A burst of \
          writes inside one millisecond is an agent's normal behaviour. Non-monotonic ids would \
          make an event cursor silently skip or repeat rows — the same class of quiet wrong \
          answer as an inverted graph traversal.\n\n## Consequences\n\nFound by a test, not by \
          reading.",
        ),
        (
            "Every table gets idempotency_key, not just tasks",
            "## Context\n\nSPEC §7.2 and REQ-7 say every create is idempotent; §3.2 gives the \
          column only to tasks.\n\n## Decision\n\nAll thirteen tables.\n\n## Reasoning\n\nThe \
          alternative silently drops idempotency for twelve types including projects — the one \
          type where duplicates are called out as the failure that ruins the aggregate view.\n\n\
          ## Consequences\n\nMarked PROVISIONAL; raised as TQ-9 because adding a column is a \
          storage-format change and those are KB's call.",
        ),
        (
            "BM25 moves from Lance to DuckDB",
            "## Context\n\nSPEC §5 put both halves of hybrid search inside lance_hybrid_search.\n\n\
          ## Decision\n\nBM25 in DuckDB's fts extension; Lance does vectors only.\n\n\
          ## Reasoning\n\nThe keyword half could not be characterised. \"onboarding metering\" \
          returned a document containing only *metering*; \"onboarding slow\" returned nothing \
          despite a document containing *onboarding*. The extension documents only single-word \
          examples and no way to build the index that would presumably fix it. A search \
          returning plausible-but-wrong results is the same failure class as an inverted \
          traversal.\n\n## Consequences\n\nThe DuckDB index now covers prose too, so a spec and \
          a task compete in one ranking. Flagged as TQ-10.",
        ),
        (
            "Serve MCP 2025-11-25 alongside 2026-07-28",
            "## Context\n\nClaude Code 2.1.185 opens with the legacy initialize handshake and \
          declares 2025-11-25. A daemon speaking only the current revision reported \"Failed to \
          connect\".\n\n## Decision\n\nServe both.\n\n## Reasoning\n\nA daemon that only speaks \
          the newest spec is unusable with the client this product exists to serve, which would \
          make Phase 2's gate impossible to attempt. The spec makes backward compatibility a \
          MAY; here it is the difference between working and not.\n\n## Consequences\n\n\
          Mirrored headers are required only of a 2026-07-28 caller; resultType goes only to \
          clients whose revision defines it. Flagged as TQ-11.",
        ),
        (
            "Tool responses lift version to the top of the entity",
            "## Context\n\n`version` lives inside the audit block in the domain model.\n\n\
          ## Decision\n\nSurface it at the top of the entity on the wire, alongside the nested \
          block.\n\n## Reasoning\n\nspecline_update documents a `version` argument, so an agent that \
          has just read an entity should be able to copy the field of that name straight across. \
          Making it hunt inside `audit` is the papercut that becomes a 409 and a confused \
          retry.\n\n## Consequences\n\nFound by writing the UC-3 test the way an agent would \
          actually do it.",
        ),
        (
            "Fixture links are addressed by name, never by position",
            "## Context\n\nTwo Harbour feedback items ended up linked to a Specline spec.\n\n\
          ## Decision\n\nLook artifacts up by label; error if the label is missing.\n\n\
          ## Reasoning\n\nThe link section used positional indices, and appending rows near the \
          top of each list shifted every index below. The edges silently rewired themselves and \
          nothing complained, because a link to the wrong artifact is still a valid link.\n\n\
          ## Consequences\n\nA renamed artifact now breaks the fixture loudly rather than \
          quietly dropping an edge.",
        ),
        (
            "The desktop app hand-writes its components",
            "## Decision\n\nNo shadcn/ui generator.\n\n## Reasoning\n\nWhat a read-only, \
          seven-screen app needs is a card, a badge, a status colour and an empty state. Running \
          a generator to obtain those pulls in Radix primitives for a surface with no dialogs or \
          focus traps to manage.\n\n## Consequences\n\n81 packages, a 227 KB bundle. Revisit the \
          moment the app grows anything genuinely interactive.",
        ),
        (
            "Specline's local REST API has more endpoints than the MCP surface has tools",
            "## Decision\n\nUI-facing endpoints are added freely; the MCP surface stays at nine \
          tools.\n\n## Reasoning\n\nThe nine-tool ceiling exists because a *model* chooses worse \
          among forty tools than among nine. That reasoning does not transfer to a UI, which \
          knows exactly what it wants and would otherwise fetch everything and filter \
          client-side.",
        ),
        (
            "Event summaries name artifacts, not ids",
            "## Context\n\n\"linked tsk_01KZK163THQG7 references fbk_01KZK16505G3J\" is not a \
          sentence.\n\n## Decision\n\nUse labels in the summary; keep the ids on the event.\n\n\
          ## Reasoning\n\nThe activity feed and the digest are the two places that text is \
          actually shown to a human. Found by looking at the finished Home screen.",
        ),
    ];
    let mut decisions: Vec<(String, EntityId)> = Vec::new();
    for (title, body) in decisions_src {
        let mut d = Decision::new(project_id.clone(), title);
        d.status = DecisionStatus::Accepted;
        d.decided_at = Some(Utc.with_ymd_and_hms(2026, 8, 9, 12, 0, 0).unwrap());
        let id = store.create(d.into(), &prov)?.entity.id().clone();
        entities += 1;
        write_doc(store, &id, &project_id, title, body, &prov)?;
        revisions += 1;
        decisions.push((title.to_owned(), id));
    }

    // --- Open questions and carried risks --------------------------------
    let questions_src: [(
        &str,
        QuestionKind,
        QuestionStatus,
        Option<RiskSeverity>,
        &str,
    ); 11] = [
        (
            "Where does the store live, and does ~/.specline get a git remote?",
            QuestionKind::Question,
            QuestionStatus::Open,
            None,
            "Working assumption: ~/.specline, local git, no remote. Low cost to get wrong — moving it is a config change.",
        ),
        (
            "What is the retention policy on the event log?",
            QuestionKind::Question,
            QuestionStatus::Open,
            None,
            "It grows forever. Keep everything, which is probably fine for a decade at this write volume, or roll up events older than a year into daily summaries.",
        ),
        (
            "Should Specline ingest anything automatically, or only explicit writes?",
            QuestionKind::Question,
            QuestionStatus::Open,
            None,
            "Working assumption: explicit writes only, except the GitHub webhooks in SPEC §9. Governs push and deployment_status behaviour, and the write-amplification risk.",
        ),
        (
            "How does a design image get into Specline from a Claude chat session?",
            QuestionKind::Question,
            QuestionStatus::Open,
            None,
            "There is no filesystem in chat. Cowork can send files and Claude Code can read them. Unsolved; blocks part of Phase 4.",
        ),
        (
            "Should idempotency_key be on all thirteen tables or only tasks?",
            QuestionKind::Question,
            QuestionStatus::Open,
            None,
            "TQ-9. Implemented on all thirteen. The one storage-format change made without KB, because the alternative silently breaks a v1 must-have for twelve types.",
        ),
        (
            "Should BM25 live in DuckDB rather than Lance?",
            QuestionKind::Question,
            QuestionStatus::Open,
            None,
            "TQ-10. Implemented in DuckDB because lance_hybrid_search's keyword half could not be characterised. The swap back is one module.",
        ),
        (
            "How long should the 2025-11-25 handshake be carried?",
            QuestionKind::Question,
            QuestionStatus::Open,
            None,
            "TQ-11. Needed today, because that is what Claude Code sends. Worth revisiting once clients move on.",
        ),
        (
            "Schema creep kills it",
            QuestionKind::Risk,
            QuestionStatus::Accepted,
            Some(RiskSeverity::High),
            "Thirteen artifact types is a ceiling, not a starting point. Watch for wanting a fourteenth — it is almost always a field or a kind value on an existing type.",
        ),
        (
            "The agent might simply not write to it",
            QuestionKind::Risk,
            QuestionStatus::Open,
            Some(RiskSeverity::High),
            "If Claude has to be reminded every session, the whole thing fails. This is what Phase 2's gate measures, and it has not been run.",
        ),
        (
            "Retrieval quality may be mediocre",
            QuestionKind::Risk,
            QuestionStatus::Mitigated,
            Some(RiskSeverity::Medium),
            "Mitigated by hybrid rather than pure-vector search from day one. Still needs evaluation on real queries.",
        ),
        (
            "Lance is the one unhedged dependency",
            QuestionKind::Risk,
            QuestionStatus::Mitigated,
            Some(RiskSeverity::High),
            "Mitigated by exporting the Lance datasets to Parquet in every backup. A Lance snapshot alone would not be an escape hatch from Lance.",
        ),
    ];
    let mut questions: Vec<(String, EntityId)> = Vec::new();
    for (title, kind, status, severity, body) in questions_src {
        let mut q = Question::new(project_id.clone(), title);
        q.kind = kind;
        q.status = status;
        q.severity = severity;
        if !status.is_unresolved() {
            q.resolved_at = Some(Utc.with_ymd_and_hms(2026, 8, 9, 12, 0, 0).unwrap());
        }
        let id = store.create(q.into(), &prov)?.entity.id().clone();
        entities += 1;
        write_doc(store, &id, &project_id, title, body, &prov)?;
        revisions += 1;
        questions.push((title.to_owned(), id));
    }

    // --- Glossary ---------------------------------------------------------
    let terms: [(&str, &str); 12] = [
        (
            "Artifact",
            "Any stored entity. Used generically, not as the specific `artifact` type.",
        ),
        (
            "Digest",
            "The compact project summary returned by specline_context. Budgeted to roughly 3–4k tokens.",
        ),
        (
            "Mirror",
            "Generated read-only markdown written into a project repo. Never a source of truth.",
        ),
        ("Revision", "One immutable version of a document body."),
        (
            "Session",
            "One Claude conversation, used as the provenance unit. Caller-supplied; Specline never invents one.",
        ),
        (
            "Anchor",
            "A reference to a block inside a document, such as REQ-4, so a task can link to one requirement rather than a whole spec.",
        ),
        (
            "Surface",
            "Where a write came from: chat, cowork, code, ui or cli.",
        ),
        (
            "Traversal direction",
            "Which way an edge is walked. Outbound matches from_id, inbound matches to_id. Getting it wrong returns an empty set that looks legitimate.",
        ),
        (
            "Vertex view",
            "v_entities — the UNION over all thirteen tables that lets a query resolve an id without knowing its type.",
        ),
        (
            "Hybrid search",
            "Keyword and semantic retrieval fused by reciprocal rank, because BM25 scores and vector distances are not on comparable scales.",
        ),
        (
            "Era",
            "Which MCP revision a request belongs to. Modern is 2026-07-28; Legacy is 2025-11-25 and earlier.",
        ),
        (
            "Phase gate",
            "The exit criterion for a build phase. Two of Specline's cannot be verified without a human.",
        ),
    ];
    for (term, definition) in terms {
        store.create(
            Term::new(Some(project_id.clone()), term, definition).into(),
            &prov,
        )?;
        entities += 1;
    }

    // --- Environments ------------------------------------------------------
    let mut local = Environment::new(project_id.clone(), "local");
    local.url = Some("http://127.0.0.1:7654".to_owned());
    local.status = EnvironmentStatus::Healthy;
    local.deployed_version = Some("0.1.0".to_owned());
    local.last_deployed_at = Some(Utc::now());
    store.create(local.into(), &prov)?;
    entities += 1;

    let mut desktop = Environment::new(project_id.clone(), "desktop");
    desktop.url = Some("tauri://localhost".to_owned());
    desktop.status = EnvironmentStatus::Healthy;
    desktop.deployed_version = Some("0.1.0".to_owned());
    store.create(desktop.into(), &prov)?;
    entities += 1;

    // --- Metrics, from PRD §9 ----------------------------------------------
    let metrics: [(&str, &str, f64, MetricDirection, &[f64]); 4] = [
        (
            "Sessions where Claude writes to Specline unprompted",
            "%",
            80.0,
            MetricDirection::Up,
            &[],
        ),
        (
            "Agent orientation cost",
            "tokens",
            4000.0,
            MetricDirection::Down,
            &[2075.0],
        ),
        (
            "Tests passing",
            "count",
            270.0,
            MetricDirection::Up,
            &[54.0, 182.0, 249.0, 264.0, 270.0],
        ),
        (
            "Projects tracked",
            "count",
            5.0,
            MetricDirection::Up,
            &[1.0],
        ),
    ];
    for (name, unit, target, direction, readings) in metrics {
        let mut m = Metric::new(project_id.clone(), name);
        m.unit = Some(unit.to_owned());
        m.target_value = Some(target);
        m.direction = direction;
        let metric_id = store.create(m.into(), &prov)?.entity.id().clone();
        entities += 1;
        for (i, value) in readings.iter().enumerate() {
            let at = Utc
                .with_ymd_and_hms(2026, 8, 9, 9 + i as u32, 0, 0)
                .unwrap();
            store.create(
                MetricObservation::new(metric_id.clone(), project_id.clone(), *value, at).into(),
                &prov,
            )?;
            entities += 1;
        }
    }

    // --- Links -------------------------------------------------------------
    // Addressed by name, never by position — see DECISIONS on the fixture.
    let edges: [Edge<'_>; 10] = [
        (
            "task",
            "specline_context — the digest",
            Relation::Implements,
            "spec",
            "Specline — Product Requirements Document",
            Some("REQ-3"),
        ),
        (
            "task",
            "Hybrid search — BM25 plus vectors, RRF fusion",
            Relation::Implements,
            "spec",
            "Specline — Product Requirements Document",
            Some("REQ-4"),
        ),
        (
            "task",
            "Implement idempotency keys and optimistic concurrency",
            Relation::Implements,
            "spec",
            "Specline — Product Requirements Document",
            Some("REQ-7"),
        ),
        (
            "task",
            "Backup: DuckDB and Lance to Parquet, restore",
            Relation::Implements,
            "spec",
            "Specline — Product Requirements Document",
            Some("REQ-11"),
        ),
        (
            "decision",
            "BM25 moves from Lance to DuckDB",
            Relation::Resolves,
            "question",
            "Should BM25 live in DuckDB rather than Lance?",
            None,
        ),
        (
            "decision",
            "Every table gets idempotency_key, not just tasks",
            Relation::Resolves,
            "question",
            "Should idempotency_key be on all thirteen tables or only tasks?",
            None,
        ),
        (
            "decision",
            "Serve MCP 2025-11-25 alongside 2026-07-28",
            Relation::Resolves,
            "question",
            "How long should the 2025-11-25 handshake be carried?",
            None,
        ),
        (
            "task",
            "Run the ten unprompted sessions",
            Relation::Resolves,
            "question",
            "The agent might simply not write to it",
            None,
        ),
        (
            "task",
            "Run the ten unprompted sessions",
            Relation::References,
            "spec",
            "Phase gates that cannot be verified without a human",
            None,
        ),
        (
            "decision",
            "Bundled DuckDB is a feature, not a requirement",
            Relation::References,
            "question",
            "Where does the store live, and does ~/.specline get a git remote?",
            None,
        ),
    ];
    for (from_kind, from_label, rel, to_kind, to_label, anchor) in edges {
        let from = find(
            from_kind, from_label, &tasks, &specs, &decisions, &questions,
        )?;
        let to = find(to_kind, to_label, &tasks, &specs, &decisions, &questions)?;
        let mut link = NewLink::new(from.clone(), rel, to.clone());
        if let Some(a) = anchor {
            link = link.anchored(a);
        }
        store.link(link, &prov)?;
        links += 1;
    }

    Ok(Summary {
        project_id,
        entities,
        links,
        revisions,
    })
}

/// Resolve an artifact by kind and label.
fn find(
    kind: &str,
    label: &str,
    tasks: &[(String, EntityId)],
    specs: &[(String, EntityId)],
    decisions: &[(String, EntityId)],
    questions: &[(String, EntityId)],
) -> Result<EntityId> {
    let pool = match kind {
        "task" => tasks,
        "spec" => specs,
        "decision" => decisions,
        "question" => questions,
        other => anyhow::bail!("unknown artifact kind `{other}` in the bootstrap link table"),
    };
    pool.iter()
        .find(|(name, _)| name == label)
        .map(|(_, id)| id.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no {kind} titled “{label}” — a title changed but the link naming it did not"
            )
        })
}

/// Write the first revision of an artifact's body.
fn write_doc(
    store: &mut Store,
    entity_id: &EntityId,
    project_id: &EntityId,
    title: &str,
    body: &str,
    prov: &Provenance,
) -> Result<()> {
    let doc = Document::first(
        entity_id.entity_type(),
        entity_id.clone(),
        Some(project_id.clone()),
        title,
        body,
        prov.actor,
        Utc::now(),
    )?
    .attributed(prov.session_id.clone(), prov.surface);
    store.write_revision(doc)?;
    Ok(())
}

/// Archive every project that is not the one named.
///
/// Soft delete, like everything else — the rows stay, they just stop appearing.
pub fn archive_other_projects(store: &mut Store, keep: &EntityId) -> Result<usize> {
    use specline_core::EntityQuery;
    let prov = Provenance {
        actor: Actor::Human,
        session_id: Some("ses_bootstrap_2026_08_09".to_owned()),
        surface: Some(Surface::Cli),

        client: None,
    };

    let projects = store.list(&EntityQuery::default().of_type(EntityType::Project))?;
    let mut archived = 0;
    for entity in projects.items {
        if entity.id() == keep || entity.audit().is_archived() {
            continue;
        }
        let id = entity.id().clone();
        let version = entity.audit().version;
        store.archive(&id, version, &prov)?;
        archived += 1;

        // Archiving a project does not cascade to its children (SPEC §3.1),
        // which is deliberate — but a demo project left behind as a cloud of
        // orphans is worse than untidy, it is confusing. So its artifacts are
        // archived explicitly rather than left for fsck to complain about.
        for entity_type in EntityType::ALL {
            if entity_type == EntityType::Project {
                continue;
            }
            let page = store.list(
                &EntityQuery::in_project(id.clone())
                    .of_type(entity_type)
                    .limited(5_000),
            )?;
            for child in page.items {
                if child.audit().is_archived() {
                    continue;
                }
                let (child_id, child_version) = (child.id().clone(), child.audit().version);
                store.archive(&child_id, child_version, &prov)?;
                archived += 1;
            }
        }
    }
    Ok(archived)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_bootstrap_seeds_a_coherent_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
        let summary = run(&mut store, None).unwrap();

        assert!(summary.entities > 80, "got {}", summary.entities);
        assert_eq!(summary.links, 10);
        assert!(summary.revisions > 20);

        // It must pass its own integrity checks, or it is not a good seed.
        let report = specline_core::fsck::check(&store).unwrap();
        assert!(
            report.is_clean(),
            "bootstrap produced an inconsistent store: {:#?}",
            report.errors().collect::<Vec<_>>()
        );
    }

    #[test]
    fn running_the_bootstrap_twice_creates_nothing_new() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
        run(&mut store, None).unwrap();
        let before: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        run(&mut store, None).unwrap();
        let after: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn archiving_other_projects_leaves_the_named_one_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
        specline_core::fixture::load(&mut store).unwrap();
        let projects_before: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(projects_before, 3, "the fixture makes three");

        let summary = run(&mut store, None).unwrap();

        // The fixture's demo project is also called `specline`, so the bootstrap
        // resolves to it by idempotency key rather than making a fourth. That
        // is the designed behaviour — one slug is one project — but it is why
        // the bootstrap belongs in an empty store, not on top of demo data.
        let projects_after: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(projects_after, projects_before);

        let archived = archive_other_projects(&mut store, &summary.project_id).unwrap();
        assert!(archived > 0);

        use specline_core::EntityQuery;
        let live = store
            .list(&EntityQuery::default().of_type(EntityType::Project))
            .unwrap();
        assert_eq!(live.items.len(), 1, "only Specline should remain visible");
        assert_eq!(live.items[0].id(), &summary.project_id);

        // Soft delete: nothing was actually removed.
        let rows: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, projects_before, "archived projects stay on disk");

        // And their artifacts went with them, rather than being left as a
        // cloud of orphans for fsck to complain about.
        let live_tasks = store
            .list(&EntityQuery::default().of_type(EntityType::Task))
            .unwrap();
        assert!(
            live_tasks
                .items
                .iter()
                .all(|t| t.project_id() == Some(&summary.project_id)),
            "a task survived from an archived project"
        );
    }
}
