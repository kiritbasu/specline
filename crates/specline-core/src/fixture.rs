//! A realistic fixture corpus.
//!
//! Phase 0's exit criteria ask for 200 entities across all types and relations.
//! The content is deliberately real prose rather than `foo`/`bar`, for two
//! reasons that both bite later if ignored:
//!
//! 1. **It is the search-quality corpus.** R-3 says retrieval quality is a
//!    real risk and must be evaluated on real queries before any UI is built.
//!    You cannot evaluate ranking against a corpus of `task 1`, `task 2` —
//!    every document is equidistant from every query.
//! 2. **It is what `specline_context` gets tuned against.** REQ-3 budgets the
//!    digest to 3–4k tokens. Placeholder text compresses to nothing and would
//!    make the budget look comfortable when it is not.
//!
//! Three projects, because the multi-project roll-up (G7, UC-6) cannot be
//! exercised with one.

use crate::{
    Actor, ArtifactKind, CloseReason, DecisionStatus, DesignState, Document, EntityId, EntityStore,
    EntityType, EnvironmentStatus, FeedbackKind, MetricDirection, MilestoneKind, MilestoneStatus,
    NewLink, ProjectStatus, Provenance, QuestionKind, QuestionStatus, Relation, Result,
    RiskSeverity, Sentiment, SpecKind, SpecStatus, Surface, TaskKind, TaskPriority, TaskStatus,
};
use crate::{
    Artifact, Decision, Design, DocumentStore, Entity, Environment, Feedback, Metric,
    MetricObservation, Milestone, Project, Question, Spec, Task, Term,
};
use chrono::{Duration, NaiveDate, Utc};

/// What the fixture created, so a caller can assert on it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FixtureSummary {
    /// Entities created, per type.
    pub entities: std::collections::BTreeMap<&'static str, usize>,
    /// Links created, per relation.
    pub links: std::collections::BTreeMap<&'static str, usize>,
    /// Document revisions written.
    pub revisions: usize,
}

impl FixtureSummary {
    /// Total entities across all types.
    pub fn total_entities(&self) -> usize {
        self.entities.values().sum()
    }

    /// Total links across all relations.
    pub fn total_links(&self) -> usize {
        self.links.values().sum()
    }
}

/// One milestone row in the fixture table below.
type MilestoneRow<'a> = (
    &'a EntityId,
    &'a str,
    &'a str,
    MilestoneStatus,
    MilestoneKind,
    Option<NaiveDate>,
    Option<&'a str>,
);

/// One task row: project, milestone index, title, body, status, priority,
/// kind, labels.
type TaskRow<'a> = (
    &'a EntityId,
    usize,
    &'a str,
    &'a str,
    TaskStatus,
    TaskPriority,
    TaskKind,
    &'a [&'a str],
);

/// One spec row: project, title, kind, status, body.
type SpecRow<'a> = (&'a EntityId, &'a str, SpecKind, SpecStatus, &'a str);

/// One decision row: project, title, status, body.
type DecisionRow<'a> = (&'a EntityId, &'a str, DecisionStatus, &'a str);

/// One question row: project, title, kind, status, severity, body.
type QuestionRow<'a> = (
    &'a EntityId,
    &'a str,
    QuestionKind,
    QuestionStatus,
    Option<RiskSeverity>,
    &'a str,
);

/// One glossary row: project (None for global), term, definition, aliases.
type TermRow<'a> = (Option<&'a EntityId>, &'a str, &'a str, &'a [&'a str]);

/// One feedback row: project, summary, kind, sentiment, source, body.
type FeedbackRow<'a> = (
    &'a EntityId,
    &'a str,
    FeedbackKind,
    Sentiment,
    &'a str,
    &'a str,
);

/// One design row: project, name, state, caption.
type DesignRow<'a> = (&'a EntityId, &'a str, DesignState, &'a str);

/// One environment row: project, name, url, status, deployed version.
type EnvironmentRow<'a> = (&'a EntityId, &'a str, &'a str, EnvironmentStatus, &'a str);

/// One metric row: project, name, unit, target, direction, readings.
type MetricRow<'a> = (
    &'a EntityId,
    &'a str,
    &'a str,
    f64,
    MetricDirection,
    &'a [f64],
);

/// One artifact row: project, name, kind, url.
type ArtifactRow<'a> = (&'a EntityId, &'a str, ArtifactKind, &'a str);

/// Load the fixture into an empty store.
///
/// Written through the ordinary [`EntityStore`] and [`DocumentStore`] paths
/// rather than by bulk-inserting rows. That is slower and entirely the point:
/// loading the fixture exercises validation, idempotency, event generation and
/// link normalisation, so a fixture that loads is evidence the write path
/// works — not merely that the schema accepts rows.
///
/// Generic over the two traits rather than naming a store, because the
/// migration has to load the same corpus into both engines and compare them —
/// and a fixture that only one store can hold would compare nothing.
pub fn load<S: EntityStore + DocumentStore>(store: &mut S) -> Result<FixtureSummary> {
    let mut s = FixtureSummary::default();
    let now = Utc::now();

    // Provenance varies across the corpus on purpose: the activity feed and
    // the "what did Claude do today" view are only meaningful if the fixture
    // contains more than one actor.
    let claude = Provenance {
        actor: Actor::Claude,
        session_id: Some("ses_fixture_claude".to_owned()),
        surface: Some(Surface::Code),

        client: None,
    };
    let human = Provenance {
        actor: Actor::Human,
        session_id: Some("ses_fixture_human".to_owned()),
        surface: Some(Surface::Ui),

        client: None,
    };
    let chat = Provenance {
        actor: Actor::Claude,
        session_id: Some("ses_fixture_chat".to_owned()),
        surface: Some(Surface::Chat),

        client: None,
    };

    // ---------------------------------------------------------------- Specline
    let mut specline = Project::new("specline", "Specline");
    specline.description = Some(
        "Local-first store for everything that describes a software project other than \
         the code — specs, decisions, tasks, roadmap, design, feedback. MCP server as the \
         primary interface, Tauri desktop app as the read surface."
            .to_owned(),
    );
    specline.repo_urls = vec!["https://github.com/kb/specline".to_owned()];
    specline.aliases = vec!["project spine".to_owned(), "the specline store".to_owned()];
    specline.root_path = Some("~/development/specline".to_owned());
    let specline_id = make(store, specline.into(), &human, &mut s)?;

    // ------------------------------------------------------------- Harbour
    let mut harbour = Project::new("harbour", "Harbour");
    harbour.description = Some(
        "Usage-based billing for API companies. Meters requests, aggregates them into \
         invoices, and reconciles against Stripe. The hard part is idempotency across \
         retries, not the arithmetic."
            .to_owned(),
    );
    harbour.repo_urls = vec!["https://github.com/kb/harbour".to_owned()];
    harbour.aliases = vec!["billing".to_owned()];
    let harbour_id = make(store, harbour.into(), &human, &mut s)?;

    // -------------------------------------------------------------- Sextant
    let mut sextant = Project::new("sextant", "Sextant");
    sextant.description = Some(
        "A command-line tool for exploring OpenTelemetry traces locally, without shipping \
         them anywhere. Reads OTLP files, indexes spans, answers 'what was slow'."
            .to_owned(),
    );
    sextant.status = ProjectStatus::Paused;
    let sextant_id = make(store, sextant.into(), &claude, &mut s)?;

    // --- Milestones ------------------------------------------------------
    let milestones: Vec<MilestoneRow<'_>> = vec![
        (
            &specline_id,
            "Phase 0 — Spine",
            "Storage, schema, event log, graph. No network, no UI.",
            MilestoneStatus::Open,
            MilestoneKind::Milestone,
            NaiveDate::from_ymd_opt(2026, 9, 15),
            None,
        ),
        (
            &specline_id,
            "Phase 1 — Daemon",
            "axum, nine MCP tools, hybrid search wired to the tool surface.",
            MilestoneStatus::Open,
            MilestoneKind::Milestone,
            NaiveDate::from_ymd_opt(2026, 10, 15),
            None,
        ),
        (
            &specline_id,
            "Phase 2 — Plugin",
            "The skill, session threading, mirror hooks. The real test of the premise.",
            MilestoneStatus::Open,
            MilestoneKind::Milestone,
            NaiveDate::from_ymd_opt(2026, 11, 1),
            None,
        ),
        (
            &specline_id,
            "Phase 3 — Desktop",
            "Tauri shell and the seven v1 screens.",
            MilestoneStatus::Open,
            MilestoneKind::Milestone,
            NaiveDate::from_ymd_opt(2026, 12, 1),
            None,
        ),
        (
            &harbour_id,
            "Metering v1",
            "Ingest, dedupe and aggregate usage events.",
            MilestoneStatus::Shipped,
            MilestoneKind::Release,
            NaiveDate::from_ymd_opt(2026, 5, 20),
            Some("1.0.0"),
        ),
        (
            &harbour_id,
            "Invoicing",
            "Turn aggregates into invoices and reconcile with Stripe.",
            MilestoneStatus::Open,
            MilestoneKind::Milestone,
            NaiveDate::from_ymd_opt(2026, 9, 1),
            None,
        ),
        (
            &harbour_id,
            "Self-serve plans",
            "Let customers change plan without a support ticket.",
            MilestoneStatus::Open,
            MilestoneKind::Milestone,
            NaiveDate::from_ymd_opt(2026, 10, 30),
            None,
        ),
        (
            &harbour_id,
            "Reliability hardening",
            "Stop shedding load silently and start alerting on drift.",
            MilestoneStatus::Open,
            MilestoneKind::Milestone,
            NaiveDate::from_ymd_opt(2026, 11, 15),
            None,
        ),
        (
            &harbour_id,
            "Metering v1.1",
            "Dedupe metrics, backfill tooling, documented key contract.",
            MilestoneStatus::Shipped,
            MilestoneKind::Release,
            NaiveDate::from_ymd_opt(2026, 7, 1),
            Some("1.1.0"),
        ),
        (
            &sextant_id,
            "Local trace index",
            "Read OTLP, index spans, query them.",
            MilestoneStatus::Cut,
            MilestoneKind::Milestone,
            None,
            None,
        ),
    ];
    let mut milestone_ids = Vec::new();
    for (project, name, summary, status, kind, target, version) in milestones {
        let mut m = Milestone::new(project.clone(), name, summary);
        m.status = status;
        m.kind = kind;
        m.target_date = target;
        m.version_string = version.map(str::to_owned);
        if status == MilestoneStatus::Shipped {
            m.shipped_at = Some(now - Duration::days(80));
        }
        milestone_ids.push((project.clone(), make(store, m.into(), &human, &mut s)?));
    }

    // --- Tasks -----------------------------------------------------------
    let tasks: Vec<TaskRow<'_>> = vec![
        (
            &specline_id,
            0,
            "Verify the Lance DuckDB extension against running code",
            "The spec was written from documentation. Confirm ATTACH syntax, the hybrid search signature, and whether DuckPGQ can coexist.",
            TaskStatus::Done,
            TaskPriority::P0,
            TaskKind::Spike,
            &["storage", "risk"],
        ),
        (
            &specline_id,
            0,
            "Forward-only DuckDB migrations",
            "Numbered, recorded in _keel_migrations, applied in order. No down migrations.",
            TaskStatus::Done,
            TaskPriority::P0,
            TaskKind::Task,
            &["storage"],
        ),
        (
            &specline_id,
            0,
            "Graph traversal with direction tests for every relation",
            "One test per relation asserting both directions, plus both inversions. An inverted traversal returns an empty set that looks legitimate.",
            TaskStatus::Done,
            TaskPriority::P0,
            TaskKind::Task,
            &["graph", "correctness"],
        ),
        (
            &specline_id,
            0,
            "Document revisions with content-addressed dedupe",
            "Identical bodies must not grow the history — the mirror hook regenerates and re-reads files constantly.",
            TaskStatus::Done,
            TaskPriority::P1,
            TaskKind::Task,
            &["storage"],
        ),
        (
            &specline_id,
            0,
            "Hybrid search fusing Lance and DuckDB FTS",
            "Reciprocal rank fusion. BM25 and vector distances are not on comparable scales.",
            TaskStatus::Done,
            TaskPriority::P1,
            TaskKind::Task,
            &["search"],
        ),
        (
            &specline_id,
            0,
            "Backup including the Lance datasets, not just DuckDB",
            "A Lance snapshot is not an escape hatch from Lance. Parquet export of both.",
            TaskStatus::InProgress,
            TaskPriority::P0,
            TaskKind::Task,
            &["backup", "risk"],
        ),
        (
            &specline_id,
            0,
            "fsck for cross-engine referential integrity",
            "Foreign keys cannot be declared across the two engines, so this is the safety net.",
            TaskStatus::InProgress,
            TaskPriority::P1,
            TaskKind::Task,
            &["correctness"],
        ),
        (
            &specline_id,
            1,
            "axum server with the stateless MCP transport",
            "2026-07-28: no handshake, no session header, Mcp-Method and Mcp-Name required on every POST.",
            TaskStatus::Todo,
            TaskPriority::P0,
            TaskKind::Task,
            &["daemon", "mcp"],
        ),
        (
            &specline_id,
            1,
            "Implement server/discover",
            "Required by the 2026-07-28 spec. Advertises supported versions, capabilities and identity.",
            TaskStatus::Todo,
            TaskPriority::P0,
            TaskKind::Task,
            &["daemon", "mcp"],
        ),
        (
            &specline_id,
            1,
            "specline_context digest within a 4k token budget",
            "Questions and terms are never truncated; everything else degrades and reports what it dropped.",
            TaskStatus::Todo,
            TaskPriority::P0,
            TaskKind::Task,
            &["daemon", "mcp"],
        ),
        (
            &specline_id,
            1,
            "keel-cli render-status for the dogfooding switch",
            "The mirror is prose-only, so STATUS.md needs its own renderer.",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["cli"],
        ),
        (
            &specline_id,
            2,
            "Write the skill that teaches Claude when to write",
            "Phase 2 is the real test. If Specline is not useful with no UI, the premise is wrong.",
            TaskStatus::Todo,
            TaskPriority::P0,
            TaskKind::Task,
            &["plugin"],
        ),
        (
            &specline_id,
            2,
            "PostToolUse hook for mirror edits",
            "Event-triggered, not reconciliation-triggered. Reads the file once, as the payload of a known edit.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Task,
            &["plugin", "mirror"],
        ),
        (
            &specline_id,
            3,
            "Tauri shell with the daemon as a sidecar",
            "Same bundle as the eventual web build, different base URL.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            &specline_id,
            3,
            "Activity feed screen",
            "REQ-10 lists it as v1. 'What did Claude do today.'",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            &harbour_id,
            4,
            "Dedupe usage events by idempotency key",
            "Customers retry aggressively. Double-billing is the failure that loses accounts.",
            TaskStatus::Done,
            TaskPriority::P0,
            TaskKind::Task,
            &["billing", "correctness"],
        ),
        (
            &harbour_id,
            4,
            "Aggregate meter readings into hourly buckets",
            "Hourly is the finest granularity anyone has asked for.",
            TaskStatus::Done,
            TaskPriority::P1,
            TaskKind::Task,
            &["billing"],
        ),
        (
            &harbour_id,
            5,
            "Generate invoices from aggregates",
            "The arithmetic is easy. Deciding which period a late event belongs to is not.",
            TaskStatus::InProgress,
            TaskPriority::P0,
            TaskKind::Task,
            &["billing"],
        ),
        (
            &harbour_id,
            5,
            "Reconcile against Stripe webhooks",
            "Stripe is the source of truth for payment, we are the source of truth for usage.",
            TaskStatus::InProgress,
            TaskPriority::P0,
            TaskKind::Task,
            &["billing", "integration"],
        ),
        (
            &harbour_id,
            5,
            "Invoices round to the wrong cent on partial periods",
            "Reported by two customers. Rounding happens per line item instead of per invoice.",
            TaskStatus::Review,
            TaskPriority::P0,
            TaskKind::Bug,
            &["billing", "bug"],
        ),
        (
            &harbour_id,
            6,
            "Plan change proration",
            "Blocked on deciding whether a downgrade takes effect immediately or at period end.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Task,
            &["billing"],
        ),
        (
            &harbour_id,
            6,
            "Self-serve plan picker UI",
            "Blocked behind proration.",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["frontend"],
        ),
        (
            &harbour_id,
            5,
            "Upgrade the Stripe SDK past the breaking 2026-04 release",
            "Mechanical, but it touches every webhook handler.",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Chore,
            &["maintenance"],
        ),
        (
            &harbour_id,
            5,
            "Metering ingest drops events above 8k requests per second",
            "Reproduced in staging. The batch writer blocks and the queue sheds load silently.",
            TaskStatus::Todo,
            TaskPriority::P0,
            TaskKind::Bug,
            &["billing", "bug", "reliability"],
        ),
        (
            &harbour_id,
            4,
            "Decide whether to keep raw events after aggregation",
            "Storage cost versus the ability to recompute. Leaning keep for 90 days.",
            TaskStatus::WontDo,
            TaskPriority::P3,
            TaskKind::Spike,
            &["storage"],
        ),
        (
            &harbour_id,
            5,
            "Invoice PDF renders the wrong currency symbol for EUR accounts",
            "Hard-coded dollar sign in the template. Cosmetic, but it appears on a legal document.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Bug,
            &["billing", "bug"],
        ),
        (
            &harbour_id,
            5,
            "Add a dry-run mode to invoice generation",
            "Finance want to see next month's invoices before they are issued.",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["billing"],
        ),
        (
            &harbour_id,
            5,
            "Backfill aggregates for the March outage window",
            "Six hours of raw events were never aggregated. The raw events survived, which is the argument for keeping them.",
            TaskStatus::Done,
            TaskPriority::P1,
            TaskKind::Chore,
            &["billing", "recovery"],
        ),
        (
            &harbour_id,
            4,
            "Emit a metric for events rejected as duplicates",
            "Currently invisible. If dedupe starts rejecting legitimate events we would not know.",
            TaskStatus::Done,
            TaskPriority::P2,
            TaskKind::Task,
            &["observability"],
        ),
        (
            &harbour_id,
            4,
            "Document the idempotency key contract for customers",
            "Several customers generate a fresh key per retry, which defeats the point entirely.",
            TaskStatus::Review,
            TaskPriority::P1,
            TaskKind::Task,
            &["docs", "billing"],
        ),
        (
            &harbour_id,
            6,
            "Model the proration arithmetic before building it",
            "Two credible schemes. Write both out with worked examples before writing code.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Spike,
            &["billing"],
        ),
        (
            &harbour_id,
            6,
            "Audit log for plan changes",
            "Who changed what, when. Needed before self-serve is safe to turn on.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Task,
            &["billing", "audit"],
        ),
        (
            &harbour_id,
            5,
            "Retry Stripe webhook delivery failures with backoff",
            "Currently a failed webhook is dropped and reconciliation notices days later.",
            TaskStatus::Todo,
            TaskPriority::P0,
            TaskKind::Task,
            &["integration", "reliability"],
        ),
        (
            &harbour_id,
            5,
            "Alert when reconciliation drift exceeds one cent",
            "The rounding bug went unnoticed for two months because nothing watched for it.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Task,
            &["observability"],
        ),
        (
            &harbour_id,
            4,
            "Move the batch writer off a bounded channel",
            "The silent load-shedding above 8k rps comes from a full channel with a drop policy.",
            TaskStatus::Todo,
            TaskPriority::P0,
            TaskKind::Task,
            &["reliability"],
        ),
        (
            &harbour_id,
            4,
            "Load test the ingest path to 30k rps",
            "The target is 20k. Test past it to find where it actually falls over.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Task,
            &["reliability", "testing"],
        ),
        (
            &harbour_id,
            6,
            "Design the plan comparison table",
            "Customers cannot self-serve if they cannot tell the plans apart.",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["frontend", "design"],
        ),
        (
            &specline_id,
            0,
            "Realistic fixture corpus across all types and relations",
            "It is also the search-quality corpus, so the prose has to be real.",
            TaskStatus::Done,
            TaskPriority::P1,
            TaskKind::Task,
            &["testing"],
        ),
        (
            &specline_id,
            0,
            "Backup verification that asserts rather than eyeballs",
            "Compare row counts per table on both engines against the manifest.",
            TaskStatus::Done,
            TaskPriority::P1,
            TaskKind::Task,
            &["backup", "testing"],
        ),
        (
            &specline_id,
            0,
            "Monotonic ULID generation",
            "Plain ULIDs are not ordered within a millisecond, which would make the event cursor skip rows.",
            TaskStatus::Done,
            TaskPriority::P0,
            TaskKind::Bug,
            &["correctness"],
        ),
        (
            &specline_id,
            0,
            "Rebuild the entity FTS index when the event log moves",
            "DuckDB's FTS index is a snapshot. A task created after the last build is silently unfindable.",
            TaskStatus::Done,
            TaskPriority::P0,
            TaskKind::Bug,
            &["search", "correctness"],
        ),
        (
            &specline_id,
            1,
            "specline_search wired to the Phase 0 hybrid implementation",
            "The fusion already exists; this is the tool surface over it.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Task,
            &["daemon", "mcp"],
        ),
        (
            &specline_id,
            1,
            "specline_get with linked neighbours and a diff_against argument",
            "REQ-2 wants diffs at the API layer, not only in the UI.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Task,
            &["daemon", "mcp"],
        ),
        (
            &specline_id,
            1,
            "specline_projects fuzzy matching for disambiguation",
            "The defence against nine near-duplicate projects. Matches name, slug, aliases and repo URL.",
            TaskStatus::Todo,
            TaskPriority::P0,
            TaskKind::Task,
            &["daemon", "mcp"],
        ),
        (
            &specline_id,
            1,
            "409 payload carrying current state and events since the read",
            "So a losing writer can usually merge instead of giving up.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Task,
            &["daemon"],
        ),
        (
            &specline_id,
            1,
            "Local REST and SSE surface for the desktop app",
            "Same shape as a remote daemon, so the web build is the same bundle.",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["daemon"],
        ),
        (
            &specline_id,
            2,
            "Project confirmation before creating one",
            "UC-8. The plugin must ask the human rather than guessing.",
            TaskStatus::Todo,
            TaskPriority::P0,
            TaskKind::Task,
            &["plugin"],
        ),
        (
            &specline_id,
            2,
            "Markdown mirror generator",
            "One-directional export. Never reconciled against the database.",
            TaskStatus::Todo,
            TaskPriority::P1,
            TaskKind::Task,
            &["plugin", "mirror"],
        ),
        (
            &specline_id,
            3,
            "Roadmap timeline across one or all projects",
            "Built from milestones, which is what they are for.",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            &specline_id,
            3,
            "Search screen with type facets",
            "Cross-project, faceted, keyboard-driven.",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            &specline_id,
            3,
            "Document reader with side-by-side revision diff",
            "The diff already exists in specline-core; this is the rendering.",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["desktop"],
        ),
        (
            &sextant_id,
            7,
            "Flame graph rendering in the terminal",
            "Paused with the project.",
            TaskStatus::Todo,
            TaskPriority::P3,
            TaskKind::Task,
            &["cli"],
        ),
        (
            &sextant_id,
            7,
            "Handle traces larger than memory",
            "Never started. Would need an on-disk index.",
            TaskStatus::WontDo,
            TaskPriority::P3,
            TaskKind::Task,
            &["cli"],
        ),
        (
            &sextant_id,
            7,
            "Parse OTLP protobuf files without a collector",
            "The whole premise: no daemon, no network, just a file.",
            TaskStatus::Done,
            TaskPriority::P1,
            TaskKind::Task,
            &["cli"],
        ),
        (
            &sextant_id,
            7,
            "Span index that fits in memory for a million spans",
            "Paused with the project.",
            TaskStatus::Todo,
            TaskPriority::P2,
            TaskKind::Task,
            &["cli"],
        ),
    ];
    let mut task_ids = Vec::new();
    for (project, milestone_idx, title, body, status, priority, kind, labels) in tasks {
        let mut t = Task::new(
            project.clone(),
            title,
            "A row this test needs in the store.",
        );
        t.body = Some(body.to_owned());
        // Same reasoning as the bootstrap: the body is real prose written for
        // the row, so it is the summary rather than something duplicated
        // beside it. A fixture that could not satisfy the product's own rule
        // would be a fixture worth distrusting.
        t.summary = Some(body.to_owned());
        t.status = status;
        t.priority = priority;
        t.kind = kind;
        t.labels = labels.iter().map(|l| (*l).to_owned()).collect();
        t.milestone_id = milestone_ids.get(milestone_idx).map(|(_, id)| id.clone());
        // The store holds a create into a terminal status to the same rule as a
        // close (KEEL-217), so a closed fixture row carries what a real one
        // does: a reason, a message, and evidence where the reason demands it.
        // `wont_do` is here as well as `done` because a fixture that only
        // exercised one of the two terminal statuses would leave the other
        // untested by the demo store every screen is developed against.
        if status.is_terminal() {
            t.closed_at = Some(now - Duration::days(7));
            t.close_message = Some(body.to_owned());
        }
        if status == TaskStatus::Done {
            t.external_refs = vec![format!("https://github.com/kb/{}/pull/42", "specline")];
            // The pull request it already invents is the evidence; inventing a
            // commit sha beside it would be a second fiction for no gain.
            t.close_reason = Some(CloseReason::Done);
            t.evidence = vec![format!("pr:https://github.com/kb/{}/pull/42", "specline")];
        }
        if status == TaskStatus::WontDo {
            t.close_reason = Some(CloseReason::WontDo);
        }
        let prov = if kind == TaskKind::Bug {
            &human
        } else {
            &claude
        };
        task_ids.push((title.to_owned(), make(store, t.into(), prov, &mut s)?));
    }

    // --- Specs, with bodies ----------------------------------------------
    let specs: Vec<SpecRow<'_>> = vec![
        (
            &specline_id,
            "Specline storage specification",
            SpecKind::Spec,
            SpecStatus::Approved,
            "# Storage\n\n## REQ-1 Two engines, one SQL surface\n\nEntity headers, links, events \
          and metrics live in DuckDB, where the access pattern is update-in-place. Every prose \
          body and every revision lives in a single Lance dataset, where the pattern is \
          append-a-new-version.\n\nThe Lance datasets are attached into DuckDB as a namespace, \
          so one query can join a task to the spec revision that motivated it and rank by \
          vector similarity in the same statement.\n\n## REQ-2 Revisions in user columns\n\n\
          Lance's own dataset versioning is a storage concern serving snapshot and restore. \
          Document revisions are a domain concept that must survive compaction and \
          re-embedding. Do not conflate them.\n\n## REQ-3 Referential integrity is \
          application-level\n\nLinks are polymorphic across thirteen tables and documents live \
          where DuckDB cannot see them. Neither can be a declared constraint. Validate on \
          write; audit with fsck.",
        ),
        (
            &specline_id,
            "MCP tool surface",
            SpecKind::Spec,
            SpecStatus::Draft,
            "# The nine tools\n\n## REQ-4 Few tools, rich arguments\n\nNine tools, not forty CRUD \
          endpoints. Models choose correctly among nine and badly among forty. Expanding this \
          surface makes model selection worse, not capability better.\n\n## REQ-5 Every write \
          returns the resulting entity\n\nNo confirmation read. An agent that has to read back \
          what it just wrote burns a round trip and half its patience.\n\n## REQ-6 Never \
          truncate silently\n\nEvery list carries an explicit truncated flag and a total. \
          Questions and glossary terms are declared unbounded: a truncated task list makes an \
          agent less informed, but a truncated glossary makes it confidently wrong.",
        ),
        (
            &specline_id,
            "Agent orientation digest",
            SpecKind::DesignDoc,
            SpecStatus::Draft,
            "# specline_context\n\nOne call, roughly 3–4k tokens, and a fresh session knows where it \
          is. Project summary, active milestone, open P0 work, recent decisions, unresolved \
          questions, glossary, live environments, and a suggested next action.\n\nIf questions \
          and terms alone exceed the budget, return them in full and set budget_exceeded rather \
          than trimming. A project whose open-question register does not fit in a digest is \
          telling you something real.",
        ),
        (
            &harbour_id,
            "Usage metering",
            SpecKind::Prd,
            SpecStatus::Approved,
            "# Metering\n\n## REQ-1 Idempotent ingest\n\nEvery usage event carries a client-supplied \
          idempotency key. Re-sending an event must be a no-op. Customers retry aggressively \
          and double-billing is the failure that loses accounts rather than annoying them.\n\n\
          ## REQ-2 Late events\n\nAn event that arrives after its period has been invoiced is \
          credited to the next period, never retroactively. Reopening a closed invoice is worse \
          than a small inaccuracy.\n\n## REQ-3 Aggregation granularity\n\nHourly buckets. Nobody \
          has asked for finer and finer would multiply storage by sixty.",
        ),
        (
            &harbour_id,
            "Invoice reconciliation",
            SpecKind::Rfc,
            SpecStatus::Review,
            "# Reconciliation\n\nStripe is the source of truth for whether money moved. Harbour is \
          the source of truth for how much was owed. When they disagree, the disagreement itself \
          is the artifact worth recording — resolve it into a credit note rather than editing \
          either side.\n\nOpen question: what happens when a webhook is replayed after a manual \
          adjustment. Currently the adjustment is silently overwritten.",
        ),
        (
            &harbour_id,
            "Why invoices round wrong on partial periods",
            SpecKind::Note,
            SpecStatus::Draft,
            "Rounding is applied per line item and then summed. For a customer with 340 line items \
          on a partial period the accumulated error reached 1.7 cents, which is enough to fail \
          reconciliation against Stripe.\n\nThe fix is to sum in integer thousandths of a cent \
          and round once, at the invoice level.",
        ),
        (
            &sextant_id,
            "Local trace exploration",
            SpecKind::Prd,
            SpecStatus::Superseded,
            "# Sextant\n\nRead an OTLP file, index the spans, answer 'what was slow' without \
          shipping anything anywhere. Superseded: the same question is answered well enough by \
          existing tooling, and the project is paused.",
        ),
    ];
    let mut spec_ids = Vec::new();
    for (project, title, kind, status, body) in specs {
        let mut sp = Spec::new(project.clone(), title);
        sp.kind = kind;
        sp.status = status;
        sp.mirror_path = Some(format!(
            ".specline/specs/{}.md",
            title.to_lowercase().replace(' ', "-")
        ));
        let id = make(store, sp.into(), &chat, &mut s)?;
        write_doc(store, &id, project, title, body, &chat, &mut s)?;
        spec_ids.push((title.to_owned(), id));
    }

    // --- Decisions -------------------------------------------------------
    let decisions: Vec<DecisionRow<'_>> = vec![
        (
            &specline_id,
            "DuckDB and Lance, not SQLite",
            DecisionStatus::Accepted,
            "## Context\n\nThe store needs relational queries over mutable rows, hybrid semantic \
          search over prose, and multimodal blobs.\n\n## Decision\n\nDuckDB for entities, Lance \
          for documents and blobs, attached into one SQL surface.\n\n## Consequences\n\nBoth are \
          native Rust crates, so there is no sidecar process and no cross-language marshalling \
          on the hot path. Lance is young and is the one genuinely unhedged dependency, which \
          is why the Parquet export exists.",
        ),
        (
            &specline_id,
            "The daemon owns the single write path",
            DecisionStatus::Accepted,
            "## Context\n\nDuckDB is single-process for writes.\n\n## Decision\n\nOne daemon holds \
          the only read-write handle. Everything else goes through its API.\n\n## Consequences\n\n\
          This turns a constraint into a design rule. Note that six of the seven steps in a write \
          — validate, resolve links, embed, write, append revision, append event, regenerate \
          mirror, notify — have nothing to do with locking, so the rule survives even if \
          multi-process writes become possible.",
        ),
        (
            &specline_id,
            "Recursive CTEs for the graph, not DuckPGQ",
            DecisionStatus::Accepted,
            "## Context\n\nThe graph will have a few thousand edges.\n\n## Decision\n\nRecursive \
          CTEs over the links table.\n\n## Consequences\n\nDuckPGQ has no build for the DuckDB \
          line that carries Lance, so the choice is currently forced rather than merely \
          preferred. FalkorDB would be a third datastore and a third process for a graph that \
          fits in memory twice over.",
        ),
        (
            &specline_id,
            "Propose task closure on PR merge, never auto-close",
            DecisionStatus::Accepted,
            "## Context\n\nA merged PR usually means the task is done.\n\n## Decision\n\nSet status \
          to review and surface 'these look done, confirm?' in the next digest.\n\n## \
          Consequences\n\nUsually is not always, and a status field that is silently wrong \
          destroys trust faster than one that is merely stale.",
        ),
        (
            &specline_id,
            "Soft delete only, links included",
            DecisionStatus::Accepted,
            "## Decision\n\nNothing is ever DELETEd. Archiving sets archived_at.\n\n## Consequences\n\n\
          Agents make mistakes and hard deletes make them permanent. The cost is that every \
          query filters on archived_at and every unique index covers archived rows too.",
        ),
        (
            &harbour_id,
            "Aggregate hourly, not per-minute",
            DecisionStatus::Accepted,
            "## Decision\n\nHourly buckets.\n\n## Consequences\n\nPer-minute would multiply storage \
          by sixty for a granularity no customer has requested. Revisit if someone asks.",
        ),
        (
            &harbour_id,
            "Credit notes rather than invoice edits",
            DecisionStatus::Proposed,
            "## Context\n\nWhen Stripe and Harbour disagree, something has to give.\n\n## Decision\n\n\
          Issue a credit note. Never reopen a closed invoice.\n\n## Consequences\n\nThe audit \
          trail stays append-only, which matters more than a tidy invoice.",
        ),
        (
            &harbour_id,
            "Store raw events for 90 days",
            DecisionStatus::Rejected,
            "Rejected in favour of keeping them indefinitely. Storage is cheap and the ability to \
          recompute an invoice from first principles has already paid for itself twice.",
        ),
    ];
    let mut decision_ids = Vec::new();
    for (project, title, status, body) in decisions {
        let mut d = Decision::new(project.clone(), title);
        d.status = status;
        if status == DecisionStatus::Accepted {
            d.decided_at = Some(now - Duration::days(30));
        }
        d.mirror_path = Some(format!(
            ".specline/decisions/{}.md",
            title.to_lowercase().replace(' ', "-")
        ));
        let id = make(store, d.into(), &chat, &mut s)?;
        write_doc(store, &id, project, title, body, &chat, &mut s)?;
        decision_ids.push((title.to_owned(), id));
    }

    // --- Questions and risks ---------------------------------------------
    let questions: Vec<QuestionRow<'_>> = vec![
        (
            &specline_id,
            "Where does the store live, and does it get a git remote?",
            QuestionKind::Question,
            QuestionStatus::Open,
            None,
            "Working assumption is ~/.specline with a local git repo and no remote. Moving it is a config change, so the cost of being wrong is low.",
        ),
        (
            &specline_id,
            "What is the retention policy on the event log?",
            QuestionKind::Question,
            QuestionStatus::Open,
            None,
            "It grows forever. Options: keep everything, which is probably fine for a decade at this write volume, or roll up events older than a year into daily summaries.",
        ),
        (
            &specline_id,
            "Schema creep is the most likely cause of death",
            QuestionKind::Risk,
            QuestionStatus::Accepted,
            Some(RiskSeverity::High),
            "Thirteen artifact types is a ceiling, not a starting point. Watch for wanting a fourteenth — it is almost always a field or a kind value on an existing type.",
        ),
        (
            &specline_id,
            "The agent might simply not write to it",
            QuestionKind::Risk,
            QuestionStatus::Open,
            Some(RiskSeverity::High),
            "If Claude has to be reminded every session, the whole thing fails. This is why the plugin is a real phase rather than an afterthought.",
        ),
        (
            &specline_id,
            "Retrieval quality may be mediocre",
            QuestionKind::Risk,
            QuestionStatus::Mitigated,
            Some(RiskSeverity::Medium),
            "Mitigated by hybrid rather than pure-vector search from day one. Still needs evaluation on real queries before any UI is built.",
        ),
        (
            &specline_id,
            "Lance is the one unhedged dependency",
            QuestionKind::Risk,
            QuestionStatus::Mitigated,
            Some(RiskSeverity::High),
            "Mitigated by exporting the Lance datasets to Parquet in every backup. A Lance snapshot alone would not be an escape hatch from Lance.",
        ),
        (
            &specline_id,
            "Attribution is cooperative, not enforced",
            QuestionKind::Assumption,
            QuestionStatus::Accepted,
            None,
            "A stateless transport has no session to bind to, so session_id is caller-supplied. An agent that does not pass one produces weaker provenance and nothing prevents that.",
        ),
        (
            &harbour_id,
            "Does a downgrade take effect immediately or at period end?",
            QuestionKind::Question,
            QuestionStatus::Open,
            None,
            "Blocks proration, which blocks the self-serve plan picker. Immediate is friendlier; period-end is easier to reconcile.",
        ),
        (
            &harbour_id,
            "What happens when a Stripe webhook is replayed after a manual adjustment?",
            QuestionKind::Question,
            QuestionStatus::Open,
            None,
            "Currently the adjustment is silently overwritten, which is the worst of the available behaviours.",
        ),
        (
            &harbour_id,
            "Are hourly buckets fine enough for enterprise customers?",
            QuestionKind::Question,
            QuestionStatus::Answered,
            None,
            "Yes. Asked three of them; none meters below an hour internally either.",
        ),
        (
            &harbour_id,
            "Ingest sheds load silently above 8k rps",
            QuestionKind::Risk,
            QuestionStatus::Open,
            Some(RiskSeverity::High),
            "Reproduced in staging. Silent shedding on a billing pipeline means undercharging with no signal that it happened.",
        ),
        (
            &sextant_id,
            "Is this problem already solved well enough?",
            QuestionKind::Question,
            QuestionStatus::Answered,
            None,
            "Yes, which is why the project is paused. Existing tooling answers 'what was slow' adequately for the cases that come up.",
        ),
    ];
    let mut question_ids = Vec::new();
    for (project, title, kind, status, severity, body) in questions {
        let mut q = Question::new(project.clone(), title);
        q.kind = kind;
        q.status = status;
        q.severity = severity;
        if !status.is_unresolved() {
            q.resolved_at = Some(now - Duration::days(14));
        }
        q.mirror_path = Some(".specline/questions.md".to_owned());
        let id = make(store, q.into(), &chat, &mut s)?;
        write_doc(store, &id, project, title, body, &chat, &mut s)?;
        question_ids.push((title.to_owned(), id));
    }

    // --- Glossary --------------------------------------------------------
    let terms: Vec<TermRow<'_>> = vec![
        (
            None,
            "Idempotency key",
            "A caller-supplied value that makes repeating a create a no-op rather than a duplicate.",
            &["idem key"],
        ),
        (
            None,
            "Soft delete",
            "Marking a row archived rather than removing it. Nothing in Specline is ever deleted.",
            &["archive"],
        ),
        (
            Some(&specline_id),
            "Traversal direction",
            "Which way an edge is walked. Outbound matches from_id, inbound matches to_id. Getting it wrong returns an empty set that looks legitimate.",
            &["direction"],
        ),
        (
            Some(&specline_id),
            "Vertex view",
            "v_entities — the UNION over all thirteen tables that lets a query resolve an id without knowing its type.",
            &["v_entities"],
        ),
        (
            Some(&specline_id),
            "Surface",
            "Where a write came from: chat, cowork, code, ui or cli.",
            &[],
        ),
        (
            Some(&specline_id),
            "Hybrid search",
            "Keyword and semantic retrieval fused by reciprocal rank, because BM25 and vector distances are not comparable.",
            &["rrf"],
        ),
        (
            Some(&harbour_id),
            "Aggregate",
            "An hourly roll-up of raw meter readings. What invoices are built from.",
            &["bucket"],
        ),
        (
            Some(&harbour_id),
            "Reconciliation",
            "Checking that what Stripe collected matches what Harbour said was owed.",
            &["recon"],
        ),
        (
            Some(&harbour_id),
            "Late event",
            "A usage event arriving after its period closed. Credited forward, never retroactively.",
            &[],
        ),
        (
            Some(&sextant_id),
            "Trace",
            "A tree of spans describing one request end to end.",
            &[],
        ),
        (
            None,
            "Artifact",
            "Any stored entity. Used generically, not as the specific artifact type.",
            &[],
        ),
        (
            None,
            "Provenance",
            "Who wrote a thing, from which surface, in which conversation.",
            &[],
        ),
        (
            Some(&specline_id),
            "Digest",
            "The compact project summary returned by specline_context. Budgeted to roughly 3–4k tokens.",
            &["context digest"],
        ),
        (
            Some(&specline_id),
            "Mirror",
            "Generated read-only markdown written into a project repo. Never a source of truth.",
            &[],
        ),
        (
            Some(&specline_id),
            "Revision",
            "One immutable version of a document body.",
            &["doc version"],
        ),
        (
            Some(&specline_id),
            "Session",
            "One Claude conversation, used as the provenance unit. Caller-supplied.",
            &[],
        ),
        (
            Some(&specline_id),
            "Anchor",
            "A reference to a block inside a document, such as REQ-4, so a task can link to one requirement rather than a whole spec.",
            &[],
        ),
        (
            Some(&harbour_id),
            "Meter",
            "A named quantity being counted for billing — API calls, gigabytes egressed, seats.",
            &["meter reading"],
        ),
        (
            Some(&harbour_id),
            "Period",
            "The billing window an invoice covers. Closed periods are never reopened.",
            &["billing period"],
        ),
        (
            Some(&harbour_id),
            "Proration",
            "Adjusting a charge when a plan changes mid-period.",
            &[],
        ),
        (
            Some(&harbour_id),
            "Credit note",
            "A negative adjustment issued instead of editing a closed invoice.",
            &["credit"],
        ),
        (
            Some(&sextant_id),
            "Span",
            "One timed operation in a trace, with a parent and a duration.",
            &[],
        ),
    ];
    for (project, term, definition, aliases) in terms {
        let mut t = Term::new(project.cloned(), term, definition);
        t.aliases = aliases.iter().map(|a| (*a).to_owned()).collect();
        t.mirror_path = Some(".specline/glossary.md".to_owned());
        make(store, t.into(), &human, &mut s)?;
    }

    // --- Feedback --------------------------------------------------------
    let feedback: Vec<FeedbackRow<'_>> = vec![
        (
            &harbour_id,
            "Invoices do not match our own metering",
            FeedbackKind::Support,
            Sentiment::Negative,
            "Northwind Ltd",
            "Their finance team reconciles our invoice against their own request logs every month and it has been off by a cent or two since April. They are not angry, they are just tired of the reconciliation. This is the rounding bug.",
        ),
        (
            &harbour_id,
            "Changing plan needs a support ticket",
            FeedbackKind::Interview,
            Sentiment::Negative,
            "Cobalt",
            "Verbatim: 'It takes four days to move from Growth to Scale, and three of those are waiting for someone to reply.' They wanted this before they wanted anything else on the roadmap.",
        ),
        (
            &harbour_id,
            "The usage dashboard is the best part",
            FeedbackKind::Interview,
            Sentiment::Positive,
            "Meridian",
            "They screenshot it into their own weekly review. Worth knowing before anyone redesigns it.",
        ),
        (
            &harbour_id,
            "Competitor ships per-second metering",
            FeedbackKind::Competitor,
            Sentiment::Neutral,
            "Ledgerline",
            "Announced per-second granularity. Nobody we have spoken to has asked for it, but it will come up in deals as a checkbox.",
        ),
        (
            &harbour_id,
            "Idea: let customers set a spend cap",
            FeedbackKind::Idea,
            Sentiment::Positive,
            "Internal",
            "Came up twice in one week. A hard cap is dangerous — cutting off a production API is worse than an unexpected bill — but a soft cap with alerting is obviously right.",
        ),
        (
            &harbour_id,
            "Sales keeps being asked about SOC 2",
            FeedbackKind::Sales,
            Sentiment::Mixed,
            "Sales",
            "Three deals in the last quarter asked. Two proceeded anyway. It is not blocking yet.",
        ),
        (
            &harbour_id,
            "Webhook retries would have saved us a week",
            FeedbackKind::Support,
            Sentiment::Negative,
            "Ferrous Systems",
            "A single dropped webhook meant their invoice sat unreconciled for nine days. They found it, not us. That is the part that stung.",
        ),
        (
            &harbour_id,
            "We generate a new idempotency key on every retry",
            FeedbackKind::Support,
            Sentiment::Mixed,
            "Palisade",
            "Discovered while debugging a double charge. Their client library regenerates the key, which defeats dedupe entirely. Our documentation does not say clearly enough that the key must be stable across retries.",
        ),
        (
            &harbour_id,
            "Spend alerts, not spend caps",
            FeedbackKind::Interview,
            Sentiment::Positive,
            "Northwind Ltd",
            "Verbatim: 'Do not ever cut us off. Tell us loudly and let us decide.' Confirms the soft-cap design.",
        ),
        (
            &harbour_id,
            "Enterprise buyers ask about data residency",
            FeedbackKind::Sales,
            Sentiment::Neutral,
            "Sales",
            "Two EU prospects. Not blocking yet, but it will become an architecture question rather than a policy one.",
        ),
        (
            &harbour_id,
            "Dry-run invoices before issuing",
            FeedbackKind::Idea,
            Sentiment::Positive,
            "Finance",
            "Internal. They currently reconstruct next month's invoice in a spreadsheet to sanity-check it.",
        ),
        (
            &harbour_id,
            "Competitor bundles metering with an analytics product",
            FeedbackKind::Competitor,
            Sentiment::Neutral,
            "Ledgerline",
            "Different bet: they are selling insight, we are selling correctness. Worth being explicit about that rather than drifting into feature parity.",
        ),
        (
            &specline_id,
            "Claude re-litigated a decision from two sessions ago",
            FeedbackKind::Observation,
            Sentiment::Negative,
            "KB",
            "It proposed SQLite again, having no way to know the question was settled. This is the single clearest argument for the questions and decisions register.",
        ),
        (
            &specline_id,
            "Vocabulary drifts between sessions",
            FeedbackKind::Observation,
            Sentiment::Negative,
            "KB",
            "Same concept called a digest, a summary and a context blob across three sessions. Cheap to fix with a glossary; expensive to notice.",
        ),
        (
            &specline_id,
            "The desktop app is the tempting thing to build first",
            FeedbackKind::Observation,
            Sentiment::Mixed,
            "KB",
            "It is more fun and more visible than the daemon. Which is exactly why the phase order exists.",
        ),
        (
            &sextant_id,
            "Existing tooling already answers this",
            FeedbackKind::Observation,
            Sentiment::Negative,
            "KB",
            "The honest reason the project is paused. Worth recording rather than letting it look like it merely stalled.",
        ),
        (
            &specline_id,
            "Reading a folder of markdown files is not a status view",
            FeedbackKind::Observation,
            Sentiment::Negative,
            "KB",
            "The original complaint that produced this project. There is no single screen that answers 'what is the state of this project', let alone all of them.",
        ),
        (
            &specline_id,
            "Every new session starts cold",
            FeedbackKind::Observation,
            Sentiment::Negative,
            "KB",
            "Claude cannot know what was decided three sessions ago, what is open, or what it already tried. Context is re-established by hand, expensively and incompletely, every time.",
        ),
        (
            &specline_id,
            "Onboarding a new project should take one sentence",
            FeedbackKind::Idea,
            Sentiment::Positive,
            "KB",
            "If adding a project is harder than saying 'track this repo too', the multi-project premise dies quietly.",
        ),
    ];
    let mut feedback_ids = Vec::new();
    for (project, summary, kind, sentiment, source, body) in feedback {
        let mut f = Feedback::new(project.clone(), summary);
        f.kind = kind;
        f.sentiment = Some(sentiment);
        f.source = Some(source.to_owned());
        f.occurred_at = Some(now - Duration::days(21));
        f.triaged = matches!(kind, FeedbackKind::Support | FeedbackKind::Interview);
        let id = make(store, f.into(), &human, &mut s)?;
        write_doc(store, &id, project, summary, body, &human, &mut s)?;
        feedback_ids.push((summary.to_owned(), id));
    }

    // --- Design artifacts -------------------------------------------------
    let designs: Vec<DesignRow<'_>> = vec![
        (
            &specline_id,
            "Home — all projects at a glance",
            DesignState::Proposed,
            "One row per project: health, what shipped this week, what is at risk. The Sunday-review screen.",
        ),
        (
            &specline_id,
            "Project dashboard",
            DesignState::Proposed,
            "Active milestone, task counts by status, open questions, recent activity, live environments.",
        ),
        (
            &specline_id,
            "Document reader with revision history",
            DesignState::Proposed,
            "Side-by-side diff between any two revisions, plus the link graph for the current document.",
        ),
        (
            &harbour_id,
            "Invoice detail",
            DesignState::Built,
            "Shipped in Metering v1. Line items, period, reconciliation status.",
        ),
        (
            &harbour_id,
            "Self-serve plan picker",
            DesignState::Approved,
            "Approved but not built — blocked behind the proration decision.",
        ),
        (
            &harbour_id,
            "Spend cap settings",
            DesignState::Proposed,
            "Soft cap with alerting. Deliberately no hard cap: cutting off a production API is worse than a surprising bill.",
        ),
    ];
    let mut design_ids = Vec::new();
    for (project, name, state, caption) in designs {
        let mut d = Design::new(project.clone(), name);
        d.state = state;
        d.figma_ref = Some(format!("figma:node/{}", name.len() * 37));
        let id = make(store, d.into(), &human, &mut s)?;
        write_doc(store, &id, project, name, caption, &human, &mut s)?;
        design_ids.push((name.to_owned(), id));
    }

    // --- Environments -----------------------------------------------------
    let environments: Vec<EnvironmentRow<'_>> = vec![
        (
            &harbour_id,
            "sandbox",
            "https://sandbox.harbour.example",
            EnvironmentStatus::Healthy,
            "1.4.2",
        ),
        (
            &specline_id,
            "desktop-dev",
            "tauri://localhost",
            EnvironmentStatus::Unknown,
            "0.1.0",
        ),
        (
            &specline_id,
            "local",
            "http://127.0.0.1:7654",
            EnvironmentStatus::Healthy,
            "0.1.0",
        ),
        (
            &harbour_id,
            "production",
            "https://api.harbour.example",
            EnvironmentStatus::Healthy,
            "1.4.2",
        ),
        (
            &harbour_id,
            "staging",
            "https://staging.harbour.example",
            EnvironmentStatus::Degraded,
            "1.5.0-rc3",
        ),
        (
            &harbour_id,
            "preview",
            "https://preview.harbour.example",
            EnvironmentStatus::Unknown,
            "1.5.0-rc4",
        ),
        (
            &sextant_id,
            "local",
            "file:///dev/null",
            EnvironmentStatus::Unknown,
            "0.0.3",
        ),
    ];
    for (project, name, url, status, version) in environments {
        let mut e = Environment::new(project.clone(), name);
        e.url = Some(url.to_owned());
        e.status = status;
        e.deployed_version = Some(version.to_owned());
        e.deployed_commit = Some(format!("{:x}", name.len() * 0x9e3779b9_usize));
        e.last_deployed_at = Some(now - Duration::days(3));
        make(store, e.into(), &claude, &mut s)?;
    }

    // --- Metrics and observations ----------------------------------------
    let metrics: Vec<MetricRow<'_>> = vec![
        (
            &specline_id,
            "Sessions where Claude writes to Specline unprompted",
            "%",
            80.0,
            MetricDirection::Up,
            &[0.0, 4.0, 12.0, 21.0, 34.0, 43.0, 51.0, 58.0],
        ),
        (
            &specline_id,
            "Agent orientation cost",
            "tokens",
            4000.0,
            MetricDirection::Down,
            &[9200.0, 8100.0, 7300.0, 6100.0, 5200.0, 4400.0, 3900.0],
        ),
        (
            &specline_id,
            "Manual markdown files consulted per week",
            "files",
            0.0,
            MetricDirection::Down,
            &[14.0, 13.0, 11.0, 9.0, 6.0, 4.0, 3.0],
        ),
        (
            &harbour_id,
            "Invoice reconciliation failures",
            "count",
            0.0,
            MetricDirection::Down,
            &[7.0, 6.0, 5.0, 5.0, 4.0, 3.0, 2.0, 2.0],
        ),
        (
            &harbour_id,
            "Metering ingest throughput",
            "events/sec",
            20000.0,
            MetricDirection::Up,
            &[
                6200.0, 6900.0, 7200.0, 7800.0, 7900.0, 8000.0, 8100.0, 8100.0,
            ],
        ),
        (
            &harbour_id,
            "Time to change plan",
            "days",
            0.5,
            MetricDirection::Down,
            &[4.0, 4.5, 4.0, 4.0, 3.8, 3.5, 3.5],
        ),
    ];
    for (project, name, unit, target, direction, readings) in metrics {
        let mut m = Metric::new(project.clone(), name);
        m.unit = Some(unit.to_owned());
        m.target_value = Some(target);
        m.direction = direction;
        let metric_id = make(store, m.into(), &claude, &mut s)?;

        for (i, value) in readings.iter().enumerate() {
            let observed_at = now - Duration::days(((readings.len() - i) * 14) as i64);
            let mut o =
                MetricObservation::new(metric_id.clone(), project.clone(), *value, observed_at);
            o.note = Some(format!("week {}", i * 2));
            make(store, o.into(), &claude, &mut s)?;
        }
    }

    // --- Generic artifacts -------------------------------------------------
    let artifacts: Vec<ArtifactRow<'_>> = vec![
        (
            &specline_id,
            "RRF paper — reciprocal rank fusion",
            ArtifactKind::Link,
            "https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf",
        ),
        (
            &specline_id,
            "fastembed-rs",
            ArtifactKind::Link,
            "https://github.com/Anush008/fastembed-rs",
        ),
        (
            &specline_id,
            "Screenshot: first working specline_context digest",
            ArtifactKind::Image,
            "file:///archive/specline-context-first-run.png",
        ),
        (
            &harbour_id,
            "Rounding bug worked example",
            ArtifactKind::File,
            "file:///archive/rounding-worked-example.md",
        ),
        (
            &harbour_id,
            "Ingest load test results, August",
            ArtifactKind::File,
            "file:///archive/ingest-loadtest-2026-08.json",
        ),
        (
            &harbour_id,
            "Ledgerline pricing page, archived",
            ArtifactKind::Link,
            "https://web.archive.org/ledgerline/pricing",
        ),
        (
            &specline_id,
            "DuckDB Lance extension documentation",
            ArtifactKind::Link,
            "https://duckdb.org/docs/lts/core_extensions/lance",
        ),
        (
            &specline_id,
            "MCP 2026-07-28 specification",
            ArtifactKind::Link,
            "https://modelcontextprotocol.io/specification/2026-07-28",
        ),
        (
            &specline_id,
            "Tauri v2 documentation",
            ArtifactKind::Link,
            "https://v2.tauri.app/",
        ),
        (
            &harbour_id,
            "Stripe webhook reference",
            ArtifactKind::Link,
            "https://stripe.com/docs/webhooks",
        ),
        (
            &harbour_id,
            "Northwind reconciliation spreadsheet",
            ArtifactKind::File,
            "file:///archive/northwind-recon.xlsx",
        ),
        (
            &sextant_id,
            "OTLP protobuf schema",
            ArtifactKind::Link,
            "https://opentelemetry.io/docs/specs/otlp/",
        ),
    ];
    for (project, name, kind, url) in artifacts {
        let mut a = Artifact::new(project.clone(), name);
        a.kind = kind;
        a.url = Some(url.to_owned());
        make(store, a.into(), &claude, &mut s)?;
    }

    // --- Links, covering every relation ----------------------------------
    //
    // Addressed by name, never by position. Positional indices into the
    // lists above are a trap: inserting a row near the top silently rewires
    // every edge below it, which is exactly how two Harbour feedback items
    // ended up linked to a Specline spec. `by_label` fails loudly instead.
    link(
        store,
        by_label(&task_ids, "Forward-only DuckDB migrations")?,
        Relation::Implements,
        by_label(&spec_ids, "Specline storage specification")?,
        Some("REQ-1"),
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(
            &task_ids,
            "Document revisions with content-addressed dedupe",
        )?,
        Relation::Implements,
        by_label(&spec_ids, "Specline storage specification")?,
        Some("REQ-2"),
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&task_ids, "fsck for cross-engine referential integrity")?,
        Relation::Implements,
        by_label(&spec_ids, "Specline storage specification")?,
        Some("REQ-3"),
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&task_ids, "axum server with the stateless MCP transport")?,
        Relation::Implements,
        by_label(&spec_ids, "MCP tool surface")?,
        Some("REQ-4"),
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(
            &task_ids,
            "specline_context digest within a 4k token budget",
        )?,
        Relation::Implements,
        by_label(&spec_ids, "MCP tool surface")?,
        Some("REQ-6"),
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(
            &task_ids,
            "specline_context digest within a 4k token budget",
        )?,
        Relation::Implements,
        by_label(&spec_ids, "Agent orientation digest")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&task_ids, "Dedupe usage events by idempotency key")?,
        Relation::Implements,
        by_label(&spec_ids, "Usage metering")?,
        Some("REQ-1"),
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&task_ids, "Aggregate meter readings into hourly buckets")?,
        Relation::Implements,
        by_label(&spec_ids, "Usage metering")?,
        Some("REQ-3"),
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&task_ids, "Reconcile against Stripe webhooks")?,
        Relation::Implements,
        by_label(&spec_ids, "Invoice reconciliation")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(
            &task_ids,
            "Verify the Lance DuckDB extension against running code",
        )?,
        Relation::Blocks,
        by_label(&task_ids, "Forward-only DuckDB migrations")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&task_ids, "Forward-only DuckDB migrations")?,
        Relation::Blocks,
        by_label(
            &task_ids,
            "Document revisions with content-addressed dedupe",
        )?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(
            &task_ids,
            "Backup including the Lance datasets, not just DuckDB",
        )?,
        Relation::Blocks,
        by_label(&task_ids, "axum server with the stateless MCP transport")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&task_ids, "Self-serve plan picker UI")?,
        Relation::DependsOn,
        by_label(&task_ids, "Plan change proration")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&task_ids, "Plan change proration")?,
        Relation::DependsOn,
        by_label(
            &question_ids,
            "Does a downgrade take effect immediately or at period end?",
        )?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&decision_ids, "DuckDB and Lance, not SQLite")?,
        Relation::Resolves,
        by_label(&question_ids, "Lance is the one unhedged dependency")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&decision_ids, "The daemon owns the single write path")?,
        Relation::Resolves,
        by_label(&question_ids, "Attribution is cooperative, not enforced")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&decision_ids, "Aggregate hourly, not per-minute")?,
        Relation::Resolves,
        by_label(
            &question_ids,
            "Are hourly buckets fine enough for enterprise customers?",
        )?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&decision_ids, "Aggregate hourly, not per-minute")?,
        Relation::Supersedes,
        by_label(&decision_ids, "Store raw events for 90 days")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&spec_ids, "Why invoices round wrong on partial periods")?,
        Relation::DerivedFrom,
        by_label(&feedback_ids, "Invoices do not match our own metering")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&feedback_ids, "Invoices do not match our own metering")?,
        Relation::Informs,
        by_label(&spec_ids, "Invoice reconciliation")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&feedback_ids, "Changing plan needs a support ticket")?,
        Relation::Informs,
        by_label(&spec_ids, "Invoice reconciliation")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&feedback_ids, "Webhook retries would have saved us a week")?,
        Relation::Informs,
        by_label(&spec_ids, "Invoice reconciliation")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(
            &feedback_ids,
            "We generate a new idempotency key on every retry",
        )?,
        Relation::Informs,
        by_label(&spec_ids, "Usage metering")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(
            &feedback_ids,
            "Reading a folder of markdown files is not a status view",
        )?,
        Relation::Informs,
        by_label(&spec_ids, "Agent orientation digest")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&feedback_ids, "Every new session starts cold")?,
        Relation::Informs,
        by_label(&spec_ids, "Agent orientation digest")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&task_ids, "Handle traces larger than memory")?,
        Relation::Duplicates,
        by_label(
            &task_ids,
            "Span index that fits in memory for a million spans",
        )?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&spec_ids, "Specline storage specification")?,
        Relation::References,
        by_label(&decision_ids, "DuckDB and Lance, not SQLite")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&spec_ids, "MCP tool surface")?,
        Relation::References,
        by_label(
            &decision_ids,
            "Propose task closure on PR merge, never auto-close",
        )?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&design_ids, "Home — all projects at a glance")?,
        Relation::References,
        by_label(&spec_ids, "Agent orientation digest")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(&design_ids, "Self-serve plan picker")?,
        Relation::References,
        by_label(&task_ids, "Self-serve plan picker UI")?,
        None,
        &claude,
        &mut s,
    )?;
    link(
        store,
        by_label(
            &task_ids,
            "Invoices round to the wrong cent on partial periods",
        )?,
        Relation::References,
        by_label(&feedback_ids, "Invoices do not match our own metering")?,
        None,
        &claude,
        &mut s,
    )?;

    Ok(s)
}

/// Find an artifact the fixture created, by the label it was created with.
///
/// Returns an error rather than silently skipping, so a renamed artifact
/// breaks the fixture loudly instead of quietly dropping an edge — a missing
/// edge is invisible, which is the whole reason graph bugs are dangerous here.
fn by_label<'a>(items: &'a [(String, EntityId)], label: &str) -> Result<&'a EntityId> {
    items
        .iter()
        .find(|(name, _)| name == label)
        .map(|(_, id)| id)
        .ok_or_else(|| crate::Error::Invariant {
            operation: format!("link the fixture artifact “{label}”"),
            problem: "no artifact with that label was created; a title changed but the \
                      link that names it did not"
                .to_owned(),
        })
}

/// Create an entity through the ordinary write path and count it.
fn make<S: EntityStore>(
    store: &mut S,
    entity: Entity,
    provenance: &Provenance,
    summary: &mut FixtureSummary,
) -> Result<EntityId> {
    let entity_type = entity.entity_type().as_str();
    let created = store.create(entity, provenance)?;
    *summary.entities.entry(entity_type).or_insert(0) += 1;
    Ok(created.entity.id().clone())
}

/// Create an edge and count it.
fn link<S: EntityStore>(
    store: &mut S,
    from: &EntityId,
    rel: Relation,
    to: &EntityId,
    anchor: Option<&str>,
    provenance: &Provenance,
    summary: &mut FixtureSummary,
) -> Result<()> {
    let mut new_link = NewLink::new(from.clone(), rel, to.clone());
    if let Some(a) = anchor {
        new_link = new_link.anchored(a);
    }
    store.link(new_link, provenance)?;
    // Counted under the *requested* relation, so the summary shows that
    // `depends_on` was asked for even though `blocks` was stored.
    *summary.links.entry(rel.as_str()).or_insert(0) += 1;
    Ok(())
}

/// Write a document revision and count it.
fn write_doc<S: DocumentStore>(
    store: &mut S,
    entity_id: &EntityId,
    project_id: &EntityId,
    title: &str,
    body: &str,
    provenance: &Provenance,
    summary: &mut FixtureSummary,
) -> Result<()> {
    let doc = Document::first(
        entity_id.entity_type(),
        entity_id.clone(),
        Some(project_id.clone()),
        title,
        body,
        provenance.actor,
        Utc::now(),
    )?
    .attributed(provenance.session_id.clone(), provenance.surface);
    store.write_revision(doc)?;
    summary.revisions += 1;
    Ok(())
}

/// Every entity type the fixture is expected to populate.
///
/// A separate constant so the "covers all thirteen" assertion cannot be
/// weakened by simply deleting a type from the fixture.
pub const EXPECTED_TYPES: [EntityType; 13] = EntityType::ALL;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_totals_add_up() {
        let mut s = FixtureSummary::default();
        s.entities.insert("task", 27);
        s.entities.insert("spec", 7);
        s.links.insert("implements", 9);
        assert_eq!(s.total_entities(), 34);
        assert_eq!(s.total_links(), 9);
    }
}
