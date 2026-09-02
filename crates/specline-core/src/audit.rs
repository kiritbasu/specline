//! The audit block every table carries, and the provenance a caller supplies.
//!
//! SPEC §3.1 writes this as `<audit>` rather than repeating it thirteen times.
//! Here it is a struct that every entity embeds, so the same reasoning applies:
//! one definition, one place to get the semantics right.
//!
//! `created_by` and `session_id` are not decoration. G3 and REQ-2 — the whole
//! provenance guarantee — live in these two fields.

use crate::{Actor, Surface};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Which program is talking, as it describes itself.
///
/// The `Surface` on a write says what kind of place it came from; this says
/// which product, and they are different questions. `code` covers Claude Code
/// and Codex alike, and once both are in use the surface alone cannot tell a
/// reader which of them wrote a row (KEEL-360).
///
/// **Observed, not declared, and that is what makes it different from
/// `session_id`.** D-10 has the daemon refuse to invent a session because a
/// stateless transport has none to borrow, so attribution there is cooperative
/// and a model has to pass it. Client identity is not like that: MCP carries it
/// on the request itself, so recording it is writing down what the transport
/// said. No tool argument, nothing for a model to get wrong or omit.
///
/// Self-reported all the same. `claude-code` and `codex-mcp-client` are names
/// the client chose for itself, trivially spoofable, and fine for saying where
/// a row came from — this is provenance for a reader, not identity for a
/// decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client {
    /// The client's own name, e.g. `claude-code` or `codex-mcp-client`.
    pub name: String,
    /// A display name when the client offers one, e.g. `Codex`.
    pub title: Option<String>,
    /// The client's version string, if it sent one.
    pub version: Option<String>,
}

impl Client {
    /// What to show a reader: the title if there is one, else the raw name.
    pub fn display_name(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }
}

/// Who is writing, on behalf of what conversation, from where, using what.
///
/// Supplied by the caller on every mutation. The daemon never invents a
/// `session_id` (D-10): a stateless transport has no session to borrow, so
/// attribution is cooperative. A write with no session is weaker provenance
/// but still a write — refusing it would be worse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Who acted. Becomes `created_by`/`updated_by` on the row and `actor` on
    /// the event, which is why the two can never disagree.
    pub actor: Actor,
    /// The conversation this write belongs to, if the caller supplied one.
    pub session_id: Option<String>,
    /// The surface the write arrived on.
    pub surface: Option<Surface>,
    /// The program that sent the request, when the transport reported one.
    ///
    /// Recorded against the session rather than against the row: every write
    /// already carries a `session_id`, one conversation is one client, and a
    /// row per session is a great deal less than a column on thirteen tables
    /// that would record only the first and last writer anyway.
    pub client: Option<Client>,
}

impl Provenance {
    /// Provenance for a caller that supplied no session.
    ///
    /// Used for the transport fallbacks in SPEC §6.5 — MCP defaults to
    /// `claude`, the local REST API to `human` — and for tests.
    pub fn anonymous(actor: Actor) -> Self {
        Provenance {
            actor,
            session_id: None,
            surface: None,
            client: None,
        }
    }

    /// Provenance for Specline acting on its own behalf: migrations, fixtures,
    /// the mirror regenerator.
    pub fn system(surface: Surface) -> Self {
        Provenance {
            actor: Actor::System,
            session_id: Some(surface.as_str().to_owned()),
            surface: Some(surface),
            client: None,
        }
    }

    /// Attach a session identifier.
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Attach a surface.
    pub fn with_surface(mut self, surface: Surface) -> Self {
        self.surface = Some(surface);
        self
    }

    /// Attach the client the transport reported.
    pub fn with_client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }
}

/// The audit columns shared by all thirteen entity tables.
///
/// Not carried by `events`, which is append-only and immutable: it has no
/// `updated_at`, no `version` and no `archived_at` because none of them could
/// ever change. That exception is deliberate and is stated in SPEC §3.1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audit {
    /// When the row was first written.
    pub created_at: DateTime<Utc>,
    /// When the row last changed.
    pub updated_at: DateTime<Utc>,
    /// The optimistic-concurrency counter. Starts at 1 and increments on every
    /// update. Distinct from `current_doc_version`, which counts document
    /// revisions — SPEC §7.3 names the 409 field `latest_version` precisely so
    /// the two never get confused.
    pub version: i32,
    /// Who created the row.
    pub created_by: Actor,
    /// Who last updated it. Always equals the `actor` of the event that
    /// produced the current state.
    pub updated_by: Actor,
    /// The conversation responsible for the most recent write, if supplied.
    pub session_id: Option<String>,
    /// The surface the most recent write arrived on.
    pub surface: Option<Surface>,
    /// When the row was archived. Soft delete only (D-9) — nothing is ever
    /// `DELETE`d, links included.
    pub archived_at: Option<DateTime<Utc>>,
}

/// Round a timestamp down to the precision the store actually keeps.
///
/// The timestamp columns are TEXT with six sub-second digits — deliberately,
/// and documented on `TIMESTAMP_FORMAT`: six is what SPEC §3.1 stores and what
/// the rows carried over from DuckDB have, and nine would invent three zeroes
/// and imply an accuracy the source never had.
///
/// `Utc::now()` is nanosecond-precise, so an audit block stamped straight from
/// it holds three digits the store is about to discard. Nothing failed, because
/// the entity returned by `create` and the entity returned by a later `get`
/// were never compared by anything that ran — and on macOS they usually agree
/// anyway, since the clock there rarely fills those digits.
///
/// On Linux it does. The first CI run this project ever executed found
/// `…:40.360446Z` from the store against `…:40.360446996Z` in memory, and the
/// round-trip test had been passing on one developer's Mac for months.
///
/// Truncating here rather than at each `Utc::now()` call site is deliberate:
/// this is the one place every audit block is built, so the invariant holds by
/// construction rather than by everyone remembering.
pub fn to_stored_precision(t: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(t.timestamp_micros()).unwrap_or(t)
}

/// The current time, at the precision Specline stores.
///
/// Use this rather than `Utc::now()` for any timestamp that will be written to
/// the store: `decided_at`, `occurred_at`, `last_deployed_at`, a blob's
/// `created_at`, an observation's `observed_at`. The audit block is handled for
/// you — [`Audit::new`] truncates whatever it is given — but the fields a
/// caller sets directly are the caller's to get right, and `Utc::now()` gives
/// them three digits the store will silently drop.
///
/// The symptom, when it goes wrong, is that the entity handed back by `create`
/// is not equal to the one a later `get` returns, and only on a machine whose
/// clock fills those digits. That is a bad afternoon, and it is the reason this
/// function exists rather than a comment asking people to remember.
pub fn now() -> DateTime<Utc> {
    to_stored_precision(Utc::now())
}

impl Audit {
    /// The audit block for a row being created now.
    pub fn new(provenance: &Provenance, now: DateTime<Utc>) -> Self {
        let now = to_stored_precision(now);
        Audit {
            created_at: now,
            updated_at: now,
            version: 1,
            created_by: provenance.actor,
            updated_by: provenance.actor,
            session_id: provenance.session_id.clone(),
            surface: provenance.surface,
            archived_at: None,
        }
    }

    /// Advance the block for an update: bump the version, restamp the mutable
    /// half, leave the creation half alone.
    ///
    /// Takes the new version rather than incrementing in place because the
    /// storage layer decides it — the `UPDATE … WHERE version = ?` is what
    /// actually enforces optimistic concurrency, and this struct must reflect
    /// what the database accepted rather than what the caller hoped for.
    pub fn touched(&self, provenance: &Provenance, now: DateTime<Utc>, new_version: i32) -> Self {
        Audit {
            created_at: self.created_at,
            updated_at: to_stored_precision(now),
            version: new_version,
            created_by: self.created_by,
            updated_by: provenance.actor,
            session_id: provenance.session_id.clone(),
            surface: provenance.surface,
            archived_at: self.archived_at,
        }
    }

    /// Whether this row has been archived.
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap_or_else(Utc::now)
    }

    #[test]
    fn a_new_audit_block_starts_at_version_one() {
        let p = Provenance::anonymous(Actor::Claude).with_session("ses_abc");
        let a = Audit::new(&p, at(1000));
        assert_eq!(a.version, 1);
        assert_eq!(a.created_at, a.updated_at);
        assert_eq!(a.created_by, Actor::Claude);
        assert_eq!(a.updated_by, Actor::Claude);
        assert_eq!(a.session_id.as_deref(), Some("ses_abc"));
        assert!(!a.is_archived());
    }

    #[test]
    fn touching_preserves_creation_and_replaces_the_mutable_half() {
        let created = Provenance::anonymous(Actor::Human);
        let a = Audit::new(&created, at(1000));

        let updated = Provenance::anonymous(Actor::Claude)
            .with_session("ses_xyz")
            .with_surface(Surface::Code);
        let b = a.touched(&updated, at(2000), 2);

        assert_eq!(b.created_at, at(1000), "creation time must not move");
        assert_eq!(
            b.created_by,
            Actor::Human,
            "original author must not change"
        );
        assert_eq!(b.updated_at, at(2000));
        assert_eq!(b.updated_by, Actor::Claude);
        assert_eq!(b.version, 2);
        assert_eq!(b.surface, Some(Surface::Code));
    }

    #[test]
    fn touching_does_not_resurrect_an_archived_row() {
        let p = Provenance::anonymous(Actor::Claude);
        let mut a = Audit::new(&p, at(1000));
        a.archived_at = Some(at(1500));
        let b = a.touched(&p, at(2000), 2);
        assert_eq!(
            b.archived_at,
            Some(at(1500)),
            "an ordinary update must not un-archive; that needs an explicit restore"
        );
    }

    #[test]
    fn a_write_without_a_session_still_records_the_actor() {
        // D-10: losing attribution is bad, refusing the write is worse.
        let a = Audit::new(&Provenance::anonymous(Actor::Claude), at(1));
        assert_eq!(a.session_id, None);
        assert_eq!(a.created_by, Actor::Claude);
    }

    /// Sub-microsecond precision is dropped when the block is stamped, not when
    /// it is stored.
    ///
    /// Deliberately built from a fixed nanosecond value rather than from
    /// `Utc::now()`. The bug this covers hid for months precisely because it
    /// depended on the clock: macOS rarely fills the last three digits, Linux
    /// does, and the round-trip test passed on one machine and failed on the
    /// first CI run that ever executed. A test that reproduces only on some
    /// hardware is not a test.
    #[test]
    fn a_stamp_carries_only_the_precision_the_store_keeps() {
        let with_nanos = DateTime::from_timestamp_nanos(1_775_000_000_360_446_996);
        assert_eq!(
            with_nanos.timestamp_subsec_nanos() % 1_000,
            996,
            "the fixture has to actually carry sub-microsecond digits, or this proves nothing"
        );

        let created = Audit::new(&Provenance::anonymous(Actor::Claude), with_nanos);
        assert_eq!(
            created.created_at.timestamp_subsec_nanos() % 1_000,
            0,
            "created_at kept digits the store will discard: {}",
            created.created_at
        );
        assert_eq!(created.created_at, created.updated_at);

        let touched = created.touched(&Provenance::anonymous(Actor::Human), with_nanos, 2);
        assert_eq!(
            touched.updated_at.timestamp_subsec_nanos() % 1_000,
            0,
            "updated_at kept digits the store will discard: {}",
            touched.updated_at
        );
        assert_eq!(
            touched.created_at, created.created_at,
            "an update must not restamp the creation half"
        );
    }
}

/// A session and the client that drove it, as stored.
///
/// [`Client`] is what a request reports; this is what the store keeps, with the
/// session it belongs to and the window it was seen in. Separate types because
/// they answer different questions — one is an input to a write, the other is a
/// row somebody reads back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionClient {
    /// The conversation this describes.
    pub session_id: String,
    /// The program that drove it.
    pub client: Client,
    /// The first write this session made.
    pub first_seen: DateTime<Utc>,
    /// The most recent one.
    ///
    /// Not a heartbeat. A session that is reading rather than writing does not
    /// move this, so an old `last_seen` means "has not written lately" and
    /// never "has gone away".
    pub last_seen: DateTime<Utc>,
}
