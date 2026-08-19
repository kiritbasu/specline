//! Specline's domain core: types, validation, storage and provenance.
//!
//! This crate is deliberately inert. It never opens a network socket, never
//! reads an environment variable, and knows nothing about MCP. Everything it
//! needs is passed in by a caller. That boundary is what makes the CLI, the
//! daemon and any future surface cheap to add — and it is the reason a change
//! to the transport can never quietly change the data model.
//!
//! # Layout
//!
//! - [`entity`] / [`types`] / [`enums`] — the thirteen artifact types and the
//!   closed value sets they carry.
//! - [`id`] — type-prefixed ULIDs.
//! - [`audit`] — the provenance block every row carries.
//! - [`link`] — typed edges and, crucially, their direction.
//! - [`event`] — the append-only mutation log.
//! - [`document`] — prose revisions, versioned and diffable.
//!
//! # The one thing to read first
//!
//! [`link`]. Graph direction is the most dangerous bug class here, because an
//! inverted traversal returns an empty result that is indistinguishable from a
//! legitimate "nothing is linked". Everything else fails loudly.

pub mod atomic;
pub mod audit;
pub mod backup;
pub mod changes;
pub mod chunk;
pub mod digest;
pub mod document;
pub mod embed;
pub mod entity;
pub mod enums;
pub mod environment;
pub mod error;
pub mod event;
pub mod fixture;
pub mod fsck;
pub mod generate;
pub mod hex;
pub mod id;
pub mod link;
pub mod lint;
pub mod lock;
pub mod mirror;
pub mod next;
pub mod note;
pub mod relocate;
pub mod render_changelog;
pub mod render_decisions;
pub mod render_status;
pub mod safe_path;
pub mod store;
pub mod style;
pub mod token;
pub mod types;
pub mod vocabulary;
pub mod work;

pub use audit::{Audit, Provenance, now, to_stored_precision};
pub use backup::{BackupManifest, backup, restore, verify_restore};
// `by_session` keeps its module path at call sites rather than being re-exported
// bare: `changes` is already a local name in `store::patch`, and a crate-root
// function of that name shadows it inside that module.
pub use changes::{Change, ChangeKind, ChangeLog, ChangeQuery, SessionChanges};
pub use digest::{Depth, Digest, build as build_digest};
pub use document::{
    DocStatus, Document, DocumentDiff, EMBEDDING_DIM, EMBEDDING_MODEL, EMBEDDING_VERSION,
    body_hash, sha256_hex,
};
pub use embed::{Embedder, HashEmbedder};
pub use entity::{Actor, EntityType, ProjectScope, Surface};
pub use enums::{
    ArtifactKind, CloseReason, DecisionStatus, DesignState, EnvironmentStatus, FeedbackKind,
    MetricDirection, MilestoneKind, MilestoneProgress, MilestoneState, MilestoneStatus,
    ProjectStatus, QuestionKind, QuestionStatus, RiskSeverity, Sentiment, SpecKind, SpecStatus,
    TaskKind, TaskPriority, TaskStatus, TaskTally,
};
pub use environment::{Hazard, hazards};
pub use error::{Error, Result};
pub use event::{Action, Cursor, Event, NewEvent};
pub use fsck::{Finding, FsckReport, Severity};
pub use generate::{GenerateReport, Mode};
pub use id::{BlobId, ChunkId, DocId, EntityId, EventId, LinkId, NoteId};
pub use link::{DEFAULT_DEPTH, Direction, Link, MAX_DEPTH, NewLink, Relation};
pub use lint::{LintFinding, LintReport, lint};
pub use mirror::{Manifest, MirrorFile, MirrorReport};
pub use next::{Candidate, NextUp, Ready, ReadyFilter, ready};
pub use note::{NewNote, Note};
pub use store::{
    Blob, Created, DAEMON_ENDPOINT_FILE, DocumentStore, EntityQuery, EntityStore, GraphStore,
    HalfStatus, MIN_PLUGIN_VERSION, Neighbour, Page, SearchHit, SearchQuery, SearchReport,
    SearchResults, SearchSource, Store, pending_migrations_at, shipped_schema_version, store_path,
};
pub use style::{Warning, check as check_style};
pub use types::{
    Artifact, CLAIM_STALE_AFTER, Decision, Design, EVIDENCE_KINDS, Entity, Environment, Feedback,
    Metric, MetricObservation, Milestone, Project, Question, Spec, Task, Term,
    derive_idempotency_key, validate_evidence,
};
pub use vocabulary::{Resolved, Source as WordSource, resolve_type};
pub use work::{Claimed, Close, Closed, claim, close};
