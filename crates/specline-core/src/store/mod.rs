//! Storage: three traits and one SQLite implementation.
//!
//! The split is the one `product/CLAUDE.md` mandates:
//!
//! - [`EntityStore`] — entity CRUD, links and events.
//! - [`DocumentStore`] — revisions, blobs, embeddings and search.
//! - [`GraphStore`] — link traversal, and nothing else.
//!
//! The traits predate the engine underneath them and outlived it. Specline ran on
//! DuckDB for rows and Lance for documents until Phase 9 moved everything into
//! one SQLite file; the three traits did not change, which is what that boundary
//! was insisted on in Phase 0 to buy.
//!
//! No raw SQL exists outside these implementations. That is not tidiness: the
//! graph queries are wrong in a way that returns *plausible empty results*,
//! which is the worst failure mode available, so centralising them means
//! getting the direction right once instead of at every call site.

pub mod composite;
pub mod docs;
pub mod entity;
pub mod graph;
pub mod patch;
pub mod rows;
pub mod schema;
pub mod search;
pub mod vector;

pub use patch::{FieldChange, apply_changes};

use crate::{
    Cursor, Direction, Document, DocumentDiff, Entity, EntityId, EntityType, Error, Event, Link,
    NewEvent, NewLink, NewNote, Note, Provenance, Relation, Result, SessionClient,
};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The store file's name inside a Specline home directory.
pub const STORE_FILE: &str = "specline.sqlite";

/// The oldest plugin release this daemon can serve.
///
/// The Claude Code plugin updates itself over git while the binaries update
/// from a GitHub release, so the two are separate channels and will drift in
/// somebody else's install. That is TQ-26 — hand-copied hooks that went stale
/// while nothing said so — except now across a network and on a machine nobody
/// here can look at.
///
/// This is the daemon's half of the handshake and the plugin manifest's
/// `min_daemon_version` is the other, so the mismatch is detectable from
/// whichever side noticed first.
///
/// Raise it only when a plugin older than the new value genuinely cannot work
/// — a removed tool, a changed response shape it reads. Raising it for a
/// cosmetic change makes a working install report itself broken, and a version
/// warning that fires when nothing is wrong is one people learn to ignore
/// exactly as fast as any other false alarm.
///
/// Raised to 0.2.0 for the rename, which is the clearest case there has been:
/// from a 0.1.x plugin's point of view all thirteen tools were removed at
/// once. It also declares its MCP server under the old name, and its hooks
/// call a script that no longer exists. Both directions are broken, so both
/// halves of the handshake moved together.
///
/// Raised to 0.3.0 for a narrower but real break: `specline_ready` became
/// `specline_next` (B-85). A 0.2.x plugin's skill names the old tool, so a
/// session following it calls something this daemon does not serve and has to
/// recover from an error mid-conversation. That is the "removed tool" case
/// this comment already asks for, so both halves moved together again.
pub const MIN_PLUGIN_VERSION: &str = "0.3.0";

/// Where a running daemon records the address it actually bound.
///
/// The name lives here, beside [`STORE_FILE`], for the same reason: the daemon
/// writes this file and the CLI reads it, and two crates that each spell a
/// filename for themselves will one day spell it differently — after which the
/// CLI silently falls back to a default port and reports that no daemon is
/// running, which is the wrong answer in the direction that permits a second
/// writer.
///
/// `specline-core` never reads it and never writes it. This is a name, not a
/// behaviour; the crate still has no idea what a daemon is.
///
/// **Its presence is not liveness.** A daemon killed with `SIGKILL` leaves the
/// file behind, so a reader must confirm with a health probe before trusting
/// what it says.
pub const DAEMON_ENDPOINT_FILE: &str = "daemon.json";

/// Where the store lives inside `home`.
///
/// A one-line function so that no two surfaces can disagree about it. They used
/// to be handed the home directory itself, because the store *was* a directory;
/// it is now a file inside one, and a surface that appends the wrong name
/// silently opens an empty store rather than failing — which is the failure mode
/// worth spending a function on.
pub fn store_path(home: impl AsRef<Path>) -> PathBuf {
    home.as_ref().join(STORE_FILE)
}

/// A Specline store backed by one SQLite file.
///
/// Owns its connection. The daemon still owns the single write path, per the
/// first hard constraint — but the reason it still does is worth stating,
/// because SQLite would now permit otherwise. In WAL mode a second process can
/// read this file while a write is open, and a reader measured at 12 µs during
/// an open ten-thousand-row transaction. The single write path survives because
/// six of the seven steps in a Specline write have nothing to do with locking:
/// validation, provenance, the event, the revision, the embedding and the index
/// all still need one place that knows how to do them.
pub struct Store {
    conn: Connection,
    path: PathBuf,
    /// The exclusive claim, when this handle was opened for writing.
    ///
    /// Held rather than read: dropping it releases the store, so its lifetime is
    /// deliberately the handle's. `None` for a read-only opener — `doctor`,
    /// `fsck` and the desktop app all read alongside a live daemon, and taking
    /// the lock to do that would make looking at the store an act that
    /// interferes with it.
    _lock: Option<crate::lock::StoreLock>,
    embedder: Option<std::sync::Arc<dyn crate::Embedder>>,
    /// Whether `sqlite-vec`'s functions are actually callable on this
    /// connection. Decided once at open, because the alternative is finding out
    /// per query — and a query that fails is a search that returns an error to
    /// a caller who only wanted results.
    vector_search: bool,
}

/// Hand-written because a connection and an embedder have nothing worth
/// printing, and because `expect_err` on a failed open needs *something* — a
/// store that cannot be formatted makes the error path harder to assert on than
/// the success path.
impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("path", &self.path)
            .field("embedder", &self.embedder.is_some())
            .finish()
    }
}

impl Store {
    /// Open or create a store at `path`, **without** migrating one that
    /// already exists.
    ///
    /// This is the door for everything that is not the store's owner. An
    /// existing store with migrations pending is refused, with a message
    /// naming `specline migrate`, because applying them here is how a newer binary
    /// alters the schema underneath a running older daemon — the corruption
    /// the newer-store guard in [`Store::migrate`] was written after, arriving
    /// through the front door instead of the back. Six of the seven steps in a
    /// Specline write are not locking, and neither is a migration: it is a change
    /// to what every other process believes the tables look like.
    ///
    /// A store this call *creates* is migrated, and that is not an exception
    /// to the rule. Nothing else can own a file that did not exist a moment
    /// ago, so there is no second process whose beliefs could be invalidated.
    ///
    /// Creating the parent directory is deliberate: the daemon's first run has
    /// no `~/.specline`, and failing with "unable to open database file" for a
    /// missing directory is a worse first experience than making it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let existed = path.as_ref().exists();
        Self::open_inner(
            path,
            if existed {
                Pending::Refuse
            } else {
                Pending::Apply
            },
        )
    }

    /// Open a store and apply every migration it is missing.
    ///
    /// The owner's door, and there are meant to be few callers: the daemon at
    /// startup, `specline migrate`, and a restore — which reads a snapshot that may
    /// have been written by an older binary, migrations being forward-only.
    /// Everything else uses [`Store::open`] and is told to run the command.
    pub fn open_and_migrate(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_inner(path, Pending::Apply)
    }

    fn open_inner(path: impl AsRef<Path>, pending: Pending) -> Result<Self> {
        Self::open_locked(path, pending, Exclusive::No)
    }

    /// Open the store and hold it against every other writer (B-60).
    ///
    /// For the two processes that write: the daemon, which holds this for its
    /// lifetime, and a CLI command writing directly because no daemon is
    /// running. A second caller is refused at once with a message saying what
    /// has it, rather than succeeding and being noticed later — or never.
    pub fn open_exclusive(path: impl AsRef<Path>) -> Result<Self> {
        let existed = path.as_ref().exists();
        Self::open_locked(
            path,
            if existed {
                Pending::Refuse
            } else {
                Pending::Apply
            },
            Exclusive::Yes,
        )
    }

    /// [`Store::open_and_migrate`], holding the store while it happens.
    ///
    /// This is the one that would have caught 2026-08-13: a second daemon
    /// migrating a store the first was already serving.
    pub fn open_and_migrate_exclusive(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_locked(path, Pending::Apply, Exclusive::Yes)
    }

    fn open_locked(path: impl AsRef<Path>, pending: Pending, exclusive: Exclusive) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                context: format!("create the store directory at {}", parent.display()),
                source,
            })?;
        }

        // Claimed before the connection is opened, not after. A window between
        // opening and locking is a window two processes can both be inside, and
        // this exists precisely to close that kind of gap.
        let lock = match exclusive {
            Exclusive::Yes => Some(crate::lock::StoreLock::acquire(&path)?),
            Exclusive::No => None,
        };

        vector::register();

        let conn = Connection::open(&path).map_err(Error::storage(format!(
            "open the store at {}",
            path.display()
        )))?;

        let mut store = Store {
            conn,
            _lock: lock,
            path,
            embedder: None,
            vector_search: false,
        };
        store.configure()?;
        store.migrate(pending)?;
        store.seed_id_generator()?;
        Ok(store)
    }

    /// An in-memory store, for tests.
    ///
    /// Its own constructor rather than `open(":memory:")` so that a test cannot
    /// accidentally get a temporary file, and so the leak KEEL-119 describes —
    /// a killed test run leaving a store behind in `TMPDIR` — has a
    /// zero-footprint alternative for the tests that do not need a path.
    pub fn in_memory() -> Result<Self> {
        vector::register();
        let conn =
            Connection::open_in_memory().map_err(Error::storage("open an in-memory store"))?;
        let mut store = Store {
            conn,
            // Nothing to contend for: an in-memory store is reachable only from
            // the process that made it.
            _lock: None,
            path: PathBuf::from(":memory:"),
            embedder: None,
            vector_search: false,
        };
        store.configure()?;
        store.migrate(Pending::Apply)?;
        store.seed_id_generator()?;
        Ok(store)
    }

    /// Make sure this process cannot mint an id below one already stored.
    ///
    /// The id generator is monotonic within a process and starts from nothing
    /// in a new one, so a clock that has moved backwards since the last write —
    /// sleep and wake is the common case, not a restart — makes a fresh process
    /// mint ids that sort *below* what is already there. Nothing errors: the
    /// live-update stream simply stops noticing writes, and the activity cursor
    /// skips them permanently.
    ///
    /// Two tables, because two orderings depend on it: `events.id` is the feed,
    /// and entity ids are creation order in every list.
    fn seed_id_generator(&self) -> Result<()> {
        for sql in [
            "SELECT max(id) FROM events",
            "SELECT max(id) FROM v_entities",
        ] {
            let highest: Option<String> = self
                .conn
                .query_row(sql, [], |r| r.get(0))
                .map_err(Error::storage("read the highest stored id"))?;
            if let Some(highest) = highest
                && crate::id::ensure_above(&highest)
            {
                tracing::warn!(
                    %highest,
                    "the newest stored id is at or ahead of this machine's clock, so the clock \
                     has moved backwards since the last write. Ids are primed above it; nothing \
                     is lost, but check the system clock"
                );
            }
        }
        Ok(())
    }

    /// Attach an embedder, enabling the semantic half of hybrid search.
    ///
    /// Optional on purpose: a store with no embedder is still fully usable and
    /// still searchable by keyword, so search degrades rather than failing. Passing it in rather than building
    /// it here is what keeps `specline-core` free of decisions about model files
    /// and network access.
    ///
    /// **Attaching it is not optional in practice, though.** Without it,
    /// `search` returns keyword hits only — and that failure is silent, since
    /// results keep arriving and are merely worse. Every caller that opens a
    /// store for a human or a model should attach one.
    pub fn with_embedder(mut self, embedder: std::sync::Arc<dyn crate::Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Attach an embedder to a store that is already open and shared.
    ///
    /// The builder form above consumes `self`, which is fine at startup and
    /// impossible once the store is behind the daemon's mutex. That matters
    /// because loading the model takes long enough to be worth doing *after*
    /// the socket is bound — a daemon unreachable for the length of a 130 MB
    /// download looks broken, and on a first run it is a download.
    pub fn set_embedder(&mut self, embedder: std::sync::Arc<dyn crate::Embedder>) {
        self.embedder = Some(embedder);
    }

    /// Whether the vector half of hybrid search can run at all.
    ///
    /// `false` means `sqlite-vec` did not register, so `vec_distance_cosine`
    /// does not exist and every semantic query would fail. Search degrades to
    /// keyword-only rather than erroring — but silently degraded search is the
    /// exact failure this codebase keeps warning about, so whoever opens a
    /// store is expected to *say* when this is false.
    pub fn vector_search_available(&self) -> bool {
        self.vector_search
    }

    /// How many live current revisions have no passages, and how many there are.
    ///
    /// The number that made semantic search a fiction for months: every
    /// document in the live store had a null embedding and nothing anywhere
    /// said so, because the keyword half kept answering.
    ///
    /// "Has no vector" is now "has no passages" (B-55). The two are the same
    /// question — a revision with passages is a revision the semantic half can
    /// reach — but only one of them is still true of the schema, and this
    /// function is what `doctor` and `specline reembed --missing` both read.
    ///
    /// Archived entities are excluded from both halves of the ratio, and that
    /// matters more than it looks. Archiving deletes their passages, so
    /// counting them would report a permanent shortfall that no amount of
    /// re-embedding could close — `doctor` would say "13 of 135 have no vector"
    /// forever, and the honest reading of a check that can never go green is
    /// that people stop reading it.
    ///
    /// `model` is what makes this answer TQ-3. Passing `None` asks the broad
    /// question — "is anything unembedded at all" — which is what `doctor`
    /// wants, because it has no embedder attached and no business loading one.
    /// Passing `Some(name)` asks the question a re-embedding pass needs: which
    /// revisions have no passages **from this model**. A model change makes
    /// every document missing under the second reading and none under the
    /// first, which is exactly the distinction TQ-3 was about.
    pub fn documents_missing_embeddings(&self, model: Option<&str>) -> Result<(i64, i64)> {
        let same_model = match model {
            Some(_) => " AND c.embedding_model = ?1",
            None => "",
        };
        let params: Vec<rusqlite::types::Value> = match model {
            Some(m) => vec![rusqlite::types::Value::Text(m.to_owned())],
            None => Vec::new(),
        };
        let current: i64 = self
            .conn
            .query_row(
                "SELECT count(*) FROM documents d \
                 WHERE d.status = 'current' AND NOT EXISTS (\
                    SELECT 1 FROM v_entities v \
                     WHERE v.id = d.entity_id AND v.archived_at IS NOT NULL)",
                [],
                |r| r.get(0),
            )
            .map_err(Error::storage("count the current revisions"))?;
        let missing: i64 = self
            .conn
            .query_row(
                &format!(
                    "SELECT count(*) FROM documents d \
                     WHERE d.status = 'current' \
                       AND NOT EXISTS (SELECT 1 FROM document_chunks c \
                                        WHERE c.doc_id = d.doc_id{same_model}) \
                       AND NOT EXISTS (SELECT 1 FROM v_entities v \
                                        WHERE v.id = d.entity_id AND v.archived_at IS NOT NULL)"
                ),
                rusqlite::params_from_iter(params),
                |r| r.get(0),
            )
            .map_err(Error::storage("count the revisions with no passages"))?;
        Ok((current, missing))
    }

    /// The attached embedder, if any.
    pub fn embedder(&self) -> Option<&dyn crate::Embedder> {
        self.embedder.as_deref()
    }

    /// The embedder as something that outlives the borrow.
    ///
    /// For a caller that holds the store behind a lock and wants to embed
    /// *without* it: clone the handle, let the lock go, do the model inference,
    /// come back. [`Store::embedder`] cannot be used that way because the
    /// borrow is the lock.
    pub fn embedder_handle(&self) -> Option<std::sync::Arc<dyn crate::Embedder>> {
        self.embedder.clone()
    }

    /// Where this store lives. `:memory:` for an in-memory one.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The connection.
    ///
    /// Public because `fsck`, `backup` and the store's own tests need to ask the
    /// engine questions that no trait method should grow a signature for —
    /// table row counts, `PRAGMA integrity_check`, the migration ledger. The rule it does not weaken is that no *call site*
    /// writes SQL; these are the store's own tools reading their own store.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Fold the write-ahead log back into the database file.
    ///
    /// Called on shutdown, and it is a convenience rather than a safeguard: a
    /// killed process leaves a `-wal` beside the store which the next open
    /// replays, so nothing is lost either way. What it buys is that the file on disk is the
    /// whole store — which is what a person copying it, or a backup taken by
    /// something that does not know about SQLite, would otherwise get wrong.
    ///
    /// `TRUNCATE` rather than `PASSIVE` so the log is actually emptied instead
    /// of merely checkpointed; a reader mid-query blocks it, and that is fine,
    /// because failing to checkpoint costs nothing.
    pub fn checkpoint(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(Error::storage("checkpoint the store before shutting down"))
    }

    /// How many of a project's tasks are open, and how many of those are
    /// urgent.
    ///
    /// Counted by the database rather than by loading every row and looping.
    /// The digest asks this once per project on the single most-called tool in
    /// the surface, and the old shape read up to two thousand full task rows —
    /// every column, every label list, every close message — to produce two
    /// integers.
    ///
    /// The two definitions stay in Rust. The `IN` lists are built from
    /// [`crate::TaskStatus::is_open`] and [`crate::TaskPriority::is_urgent`]
    /// rather than written out in SQL, because a status added to the enum and
    /// forgotten in a hand-written query is a count that is quietly wrong — and
    /// a count nobody can tell is wrong is worse than a slow one.
    pub fn task_counts(&self, project_id: &EntityId) -> Result<(usize, usize)> {
        let open: Vec<&str> = crate::TaskStatus::ALL
            .iter()
            .filter(|s| s.is_open())
            .map(|s| s.as_str())
            .collect();
        let urgent: Vec<&str> = crate::TaskPriority::ALL
            .iter()
            .filter(|p| p.is_urgent())
            .map(|p| p.as_str())
            .collect();

        let quoted = |values: &[&str]| {
            values
                .iter()
                .map(|v| format!("'{v}'"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        // Interpolated rather than bound, and safe because every value comes
        // from a `&'static str` in an enum — there is no caller input anywhere
        // in this statement. Binding a variable-length `IN` list means
        // generating placeholders and threading the params, for a query whose
        // shape is fixed at compile time.
        let sql = format!(
            "SELECT
               COALESCE(sum(CASE WHEN status IN ({open}) THEN 1 ELSE 0 END), 0),
               COALESCE(sum(CASE WHEN status IN ({open}) AND priority IN ({urgent})
                            THEN 1 ELSE 0 END), 0)
             FROM tasks WHERE project_id = ?1 AND archived_at IS NULL",
            open = quoted(&open),
            urgent = quoted(&urgent),
        );

        self.conn
            .query_row(&sql, [project_id.as_str()], |r| {
                Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize))
            })
            .map_err(Error::storage(format!("count the tasks in {project_id}")))
    }

    /// How many of a project's signals are still untriaged, and how long the
    /// oldest has been waiting.
    ///
    /// Untriaged is `triaged = 0`, which is the whole definition of the Inbox:
    /// a signal nobody has picked up or set down yet. Counted here rather than
    /// by listing, for the same reason [`Store::task_counts`] is — the digest
    /// asks once per project on the most-called tool in the surface.
    ///
    /// The age comes back with the count because the count alone does not say
    /// whether anything is wrong. Forty signals filed this week is a good
    /// week; four that have sat for two months is the pile KEEL-303 is about,
    /// and only the second number tells them apart.
    ///
    /// Returns `(count, oldest_created_at)`. The timestamp is `None` exactly
    /// when the count is zero.
    pub fn untriaged_signals(
        &self,
        project_id: &EntityId,
    ) -> Result<(usize, Option<DateTime<Utc>>)> {
        self.conn
            .query_row(
                "SELECT count(*), min(created_at) FROM feedback
                 WHERE project_id = ?1 AND archived_at IS NULL AND triaged = 0",
                [project_id.as_str()],
                |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, Option<String>>(1)?)),
            )
            .map_err(Error::storage(format!(
                "count the untriaged signals in {project_id}"
            )))
            .and_then(|(count, oldest)| {
                // Parsed rather than passed on as text, so a caller computing
                // an age cannot accidentally compare two different renderings
                // of the same instant — `parse_ts` is deliberately lenient
                // about the ones the DuckDB migration left behind, and that
                // leniency only helps if everything goes through it.
                let oldest = oldest
                    .map(|raw| crate::store::rows::parse_ts("feedback", "created_at", &raw))
                    .transpose()?;
                Ok((count, oldest))
            })
    }

    /// The Inbox: untriaged signals, oldest first, cut to `limit`.
    ///
    /// Oldest first rather than newest, which is the opposite of every other
    /// list in the product and is the point. A newest-first Inbox buries the
    /// thing that has been ignored longest under whatever was filed this
    /// morning, and the whole failure KEEL-303 describes is a pile whose
    /// bottom nobody reaches.
    ///
    /// The [`Page`] carries the true total, so a cut list says it was cut —
    /// hard constraint 4, and it matters more here than almost anywhere: an
    /// Inbox showing twenty of two hundred with no total reads as an Inbox of
    /// twenty, and somebody would empty it and believe they were finished.
    pub fn inbox(&self, project_id: &EntityId, limit: usize) -> Result<Page<Entity>> {
        let (total, _) = self.untriaged_signals(project_id)?;

        let mut statement = self
            .conn
            .prepare(
                "SELECT * FROM feedback
                 WHERE project_id = ?1 AND archived_at IS NULL AND triaged = 0
                 ORDER BY created_at ASC, id ASC
                 LIMIT ?2",
            )
            .map_err(Error::storage("prepare the inbox query"))?;

        // `id` breaks the tie on `created_at`, because ULIDs are sortable by
        // creation and two signals filed in the same millisecond would
        // otherwise come back in whatever order the engine felt like — which
        // is a list that reorders itself between two renders of the same data.
        let rows = statement
            .query_map(
                rusqlite::params![project_id.as_str(), limit as i64],
                |row| Ok(rows::from_row(EntityType::Feedback, row)),
            )
            .map_err(Error::storage(format!("read the inbox of {project_id}")))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(Error::storage("read a signal from the inbox"))??);
        }
        Ok(Page::new(items, total))
    }

    /// Every milestone in a project, with where it has actually got to.
    ///
    /// Three queries for the whole project rather than four per phase. The
    /// alternative is what the digest used to do for blocked tasks — walk once
    /// per row — and this is the same question one level up.
    ///
    /// Returned as a map because every caller has the milestones already and
    /// wants the progress beside them: the digest, the tracker, and the API
    /// that hands them to the desktop app.
    ///
    /// This used to return the derived state alone, which meant handing back a
    /// conclusion and discarding the counts it was drawn from. Both other
    /// callers then recounted: `render_status` filtered the task list itself,
    /// and the roadmap screen had no counts at all, so its right-hand column
    /// fell back to a target date nobody had ever set (KEEL-332).
    pub fn milestone_progress(
        &self,
        project_id: &EntityId,
    ) -> Result<std::collections::HashMap<EntityId, crate::MilestoneProgress>> {
        use crate::{MilestoneProgress, MilestoneState, MilestoneStatus, TaskTally};

        // The task distribution per phase. `closed` and `started` are built
        // from the enum predicates rather than spelled in SQL, for the reason
        // `task_counts` gives: a status added to the enum and forgotten in a
        // hand-written query is a count that is quietly wrong.
        let closed: Vec<&str> = crate::TaskStatus::ALL
            .iter()
            .filter(|s| !s.is_open())
            .map(|s| s.as_str())
            .collect();
        let quoted = |v: &[&str]| {
            v.iter()
                .map(|x| format!("'{x}'"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let closed_list = quoted(&closed);

        let sql = format!(
            "SELECT milestone_id,
                    count(*),
                    sum(CASE WHEN status IN ({closed_list}) THEN 1 ELSE 0 END),
                    sum(CASE WHEN status NOT IN ({closed_list}) AND status <> 'todo'
                             THEN 1 ELSE 0 END)
             FROM tasks
             WHERE project_id = ?1 AND archived_at IS NULL AND milestone_id IS NOT NULL
             GROUP BY milestone_id"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(Error::storage("prepare the per-phase task tally"))?;
        let mut tallies: std::collections::HashMap<String, TaskTally> = Default::default();
        let mut rows = stmt
            .query([project_id.as_str()])
            .map_err(Error::storage("run the per-phase task tally"))?;
        while let Some(row) = rows
            .next()
            .map_err(Error::storage("read a per-phase task tally"))?
        {
            let id: String = row.get(0).map_err(Error::storage("read a milestone id"))?;
            tallies.insert(
                id,
                TaskTally {
                    total: row.get::<_, i64>(1).unwrap_or(0) as usize,
                    closed: row.get::<_, i64>(2).unwrap_or(0) as usize,
                    started: row.get::<_, i64>(3).unwrap_or(0) as usize,
                },
            );
        }
        drop(rows);
        drop(stmt);

        // What is blocked. Live `blocks` edges pointing at a milestone, with a
        // live source — a finished blocker is not a blocker, the same rule
        // `next::blocked_tasks` applies one level down.
        let mut blocked: std::collections::HashSet<String> = Default::default();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT l.to_id FROM links l
                 JOIN v_entities v ON v.id = l.from_id
                 WHERE l.rel = 'blocks' AND l.archived_at IS NULL
                   AND l.to_type = 'milestone' AND v.archived_at IS NULL",
            )
            .map_err(Error::storage("prepare the blocked-phase query"))?;
        let mut rows = stmt
            .query([])
            .map_err(Error::storage("run the blocked-phase query"))?;
        while let Some(row) = rows
            .next()
            .map_err(Error::storage("read a blocked phase"))?
        {
            blocked.insert(row.get(0).map_err(Error::storage("read a milestone id"))?);
        }
        drop(rows);
        drop(stmt);

        // When each phase last moved. Events on its tasks rather than on the
        // phase row: a milestone row is written once and then almost never
        // again, so its own `updated_at` says when somebody renamed it, not
        // when the work last progressed. Archived tasks are left in, because a
        // task being archived is itself the phase moving.
        //
        // **The maximum is taken here, not by SQL.** `max(e.at)` looks obvious
        // and is wrong on any store that came through the DuckDB migration:
        // `at` is TEXT, so `max` is lexicographic, and that migration wrote
        // some timestamps with a space where the `T` should be. Space sorts
        // before `T`, so a phase with one of each on the same day reports the
        // older row as its newest. Parsing first and comparing
        // `DateTime`s cannot care what shape the text was — and at a few
        // thousand events the row count is not worth an assumption.
        let mut latest: std::collections::HashMap<String, chrono::DateTime<chrono::Utc>> =
            Default::default();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT t.milestone_id, e.at
                 FROM events e
                 JOIN tasks t ON t.id = e.entity_id
                 WHERE t.project_id = ?1 AND t.milestone_id IS NOT NULL",
            )
            .map_err(Error::storage("prepare the per-phase activity query"))?;
        let mut rows = stmt
            .query([project_id.as_str()])
            .map_err(Error::storage("run the per-phase activity query"))?;
        while let Some(row) = rows
            .next()
            .map_err(Error::storage("read a per-phase activity time"))?
        {
            let id: String = row.get(0).map_err(Error::storage("read a milestone id"))?;
            let raw: String = row
                .get(1)
                .map_err(Error::storage("read a per-phase activity time"))?;
            // `parse_ts`, not `parse_from_rfc3339`: it is the one place that
            // knows every shape a timestamp in this store can legitimately
            // have. A row it cannot read is logged and skipped rather than
            // failing the whole digest — one unreadable event should not blank
            // the roadmap — but it is not swallowed silently either.
            match rows::parse_ts("events", "at", &raw) {
                Ok(at) => {
                    latest
                        .entry(id)
                        .and_modify(|held| {
                            if at > *held {
                                *held = at;
                            }
                        })
                        .or_insert(at);
                }
                Err(e) => {
                    tracing::warn!(
                        milestone = %id,
                        value = %raw,
                        error = %e,
                        "skipping an event whose timestamp will not parse while dating a phase"
                    );
                }
            }
        }
        drop(rows);
        drop(stmt);

        let page = self.list(
            &EntityQuery::in_project(project_id.clone())
                .of_type(EntityType::Milestone)
                .limited(1_000),
        )?;

        let mut out = std::collections::HashMap::new();
        for entity in page.items {
            let Entity::Milestone(m) = entity else {
                continue;
            };
            let key = m.id.as_str().to_owned();
            let declared: MilestoneStatus = m.status;
            let tally: TaskTally = tallies.get(&key).copied().unwrap_or_default();
            let state = MilestoneState::derive(declared, tally, blocked.contains(&key));
            out.insert(
                m.id,
                MilestoneProgress {
                    state,
                    tally,
                    last_activity: latest.get(&key).copied(),
                },
            );
        }
        Ok(out)
    }

    /// How many pages the write-ahead log currently holds.
    ///
    /// The number that says whether checkpointing is keeping up. It should
    /// hover around the autocheckpoint threshold and come back down; a figure
    /// that only ever climbs means something is holding a read snapshot open,
    /// and the symptom of that is not an error — it is a `-wal` file quietly
    /// larger than the database beside it, with every query still answering
    /// correctly from it.
    ///
    /// A `PASSIVE` checkpoint, so asking the question does not block on a
    /// reader and does not change what any other connection sees.
    pub fn wal_pages(&self) -> Result<i64> {
        self.conn
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| r.get::<_, i64>(1))
            .map_err(Error::storage("measure the write-ahead log"))
    }

    /// Connection settings, each of which is load-bearing.
    fn configure(&mut self) -> Result<()> {
        // WAL is the whole reason the app stops stalling behind a write. In the
        // default rollback journal a reader blocks for the duration of a write
        // transaction; in WAL it takes a consistent pre-transaction snapshot
        // and does not wait at all — 12 µs against an open ten-thousand-row
        // write, measured before this was written.
        //
        // `query_row` rather than `execute_batch`: setting journal_mode returns
        // the mode it ended up in, and a statement that returns a row is an
        // error when run as a batch. It is also worth reading, because the
        // request can silently fail — an in-memory database cannot use WAL.
        let mode: String = self
            .conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .map_err(Error::storage("put the store into WAL mode"))?;
        tracing::debug!(journal_mode = %mode, "store opened");

        self.conn
            .execute_batch(
                // NORMAL rather than FULL: in WAL mode this fsyncs at
                // checkpoints instead of at every commit. The exposure is that
                // a power cut can lose the last few transactions; it cannot
                // corrupt the database, which is the property that matters.
                // FULL would put an fsync in the path of every note.
                "PRAGMA synchronous = NORMAL;
                 -- Enforced, not decorative. Nothing in Specline is ever DELETEd,
                 -- so these never cascade; what they catch is a link or a note
                 -- written against a row that does not exist, which used to be
                 -- something only `fsck` could find, after the fact.
                 PRAGMA foreign_keys = ON;
                 -- Five seconds before giving up on a locked database. The
                 -- daemon is the only writer, so this should never be reached;
                 -- if it is, waiting beats failing, because the alternative is
                 -- a tool call that returns an error for a store that was
                 -- merely busy.
                 PRAGMA busy_timeout = 5000;
                 -- Fold the log back at 1,000 pages, which is SQLite's own
                 -- default and is set here so it is a decision rather than an
                 -- inheritance. The daemon runs for days and holds a
                 -- server-sent-events connection open the whole time; a reader
                 -- whose snapshot is never released stops the checkpoint
                 -- advancing, and the log then grows without bound while every
                 -- read still answers correctly from it. Stating the number
                 -- gives the `wal_pages` test something to assert against and
                 -- gives a future reader somewhere to change it.
                 PRAGMA wal_autocheckpoint = 1000;",
            )
            .map_err(Error::storage("configure the store connection"))?;

        // Ask once whether `sqlite-vec` actually registered, rather than
        // discovering it on the first semantic query.
        //
        // Two single-element vectors, so the answer is arithmetic rather than
        // a table read. Any error at all means the function is not there.
        self.vector_search = self
            .conn
            .query_row(
                "SELECT vec_distance_cosine(?1, ?1)",
                [rusqlite::types::Value::Blob(vec![0, 0, 128, 63])],
                |r| r.get::<_, f64>(0),
            )
            .is_ok();

        Ok(())
    }

    /// Apply every migration this store has not seen, in order.
    ///
    /// Forward-only. There is no `down`: rolling a schema backwards on a
    /// single-user store is a fiction that costs more to maintain than it
    /// repays, and SPEC §11 runs a backup before every migration anyway —
    /// restoring is the rollback.
    ///
    /// `pending` decides what happens when there is something to apply, which
    /// is the difference between the owner's door and everyone else's. See
    /// [`Store::open`].
    fn migrate(&mut self, pending: Pending) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS _keel_migrations (
                   id         INTEGER PRIMARY KEY,
                   name       TEXT NOT NULL,
                   applied_at TEXT NOT NULL
                 ) STRICT;",
            )
            .map_err(Error::storage("create the migration ledger"))?;

        // Refuse to run against a store newer than this binary understands.
        //
        // Written after the failure it prevents actually happened, in the store
        // this one replaced: a migration added a column, a daemon
        // built before that migration kept running, found every migration it
        // knew about already applied, concluded it was up to date, and went on
        // inserting rows with the new column left NULL. The corruption surfaced
        // two days later as an unrelated-looking read error.
        //
        // An older binary is not merely missing features — it writes rows that
        // are wrong in ways the schema cannot express. Refusing to open turns a
        // silent corruption into a startup error, which is the whole trade.
        let shipped = schema::migrations().iter().map(|m| m.id).max().unwrap_or(0);
        let newest: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(max(id), 0) FROM _keel_migrations",
                [],
                |r| r.get(0),
            )
            .map_err(Error::storage("read the migration ledger"))?;
        if newest > i64::from(shipped) {
            return Err(Error::Invariant {
                operation: "open the store".to_owned(),
                problem: format!(
                    "this store is at schema {newest}; this binary only understands {shipped}, \
                     so it is older than the store.\n\n\
                     It would write rows the newer schema expects to be populated and leave them \
                     empty, which does not fail until something else reads them.\n\n\
                     Rebuild and reinstall: ./plugin/install.sh\n\
                     To run an old binary deliberately, point it at another store with --home."
                ),
            });
        }

        // Refuse to migrate from a process that does not own the store.
        //
        // The guard above catches a binary older than the store. This catches
        // the other order — a binary newer than the store — and that one is
        // reached by ordinary use rather than by accident: install a new build,
        // run any CLI command while the daemon is still up from the old one,
        // and without this the schema changes underneath a process that has
        // already decided what the tables look like.
        let outstanding = self.pending_migrations()?;
        if !outstanding.is_empty() && pending == Pending::Refuse {
            let list = outstanding
                .iter()
                .map(|m| format!("{} ({})", m.0, m.1))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::Invariant {
                operation: "open the store".to_owned(),
                problem: format!(
                    "this store is at schema {newest} and this binary ships {shipped}, so \
                     {} migration(s) have not been applied: {list}.\n\n\
                     They are not applied here. Migrating is a schema change every other \
                     process can see, and a daemon that is already running has decided what \
                     the tables look like — so doing it from whichever command happened to \
                     open the store next is how the schema moves under a live reader.\n\n\
                     Run `specline migrate`. It stops on a running daemon and tells you to stop \
                     it first, which is the point.",
                    outstanding.len()
                ),
            });
        }

        for migration in schema::migrations() {
            let seen: i64 = self
                .conn
                .query_row(
                    "SELECT count(*) FROM _keel_migrations WHERE id = ?1",
                    [migration.id],
                    |r| r.get(0),
                )
                .map_err(Error::storage("read the migration ledger"))?;
            if seen > 0 {
                continue;
            }

            // The DDL and the ledger entry go in together. A migration that
            // ran but was not recorded is a migration that runs again on the
            // next open, against a schema that already has its tables.
            let tx = self
                .conn
                .transaction()
                .map_err(Error::storage("begin a migration"))?;
            tx.execute_batch(migration.sql)
                .map_err(Error::storage(format!(
                    "apply migration {} ({})",
                    migration.id, migration.name
                )))?;
            tx.execute(
                "INSERT INTO _keel_migrations (id, name, applied_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    migration.id,
                    migration.name,
                    chrono::Utc::now().to_rfc3339()
                ],
            )
            .map_err(Error::storage("record a migration"))?;
            tx.commit()
                .map_err(Error::storage(format!("commit migration {}", migration.id)))?;

            tracing::info!(
                id = migration.id,
                name = migration.name,
                "migration applied"
            );
        }
        Ok(())
    }

    /// The migrations this binary ships that this store has not recorded.
    ///
    /// Public because the answer is what `specline migrate` prints before it does
    /// anything and what `specline doctor` reports, and because a caller that has
    /// been refused an open deserves to be able to ask why without parsing the
    /// refusal.
    pub fn pending_migrations(&self) -> Result<Vec<(i32, String)>> {
        let mut out = Vec::new();
        for migration in schema::migrations() {
            let seen: i64 = self
                .conn
                .query_row(
                    "SELECT count(*) FROM _keel_migrations WHERE id = ?1",
                    [migration.id],
                    |r| r.get(0),
                )
                .map_err(Error::storage("read the migration ledger"))?;
            if seen == 0 {
                out.push((migration.id, migration.name.to_owned()));
            }
        }
        Ok(out)
    }

    /// The newest migration this store has recorded, or 0 for an empty one.
    ///
    /// This is the number worth comparing between two processes. The package
    /// version moves for reasons that have nothing to do with the tables; this
    /// moves only when the shape of the data does.
    pub fn schema_version(&self) -> Result<i32> {
        self.conn
            .query_row(
                "SELECT COALESCE(max(id), 0) FROM _keel_migrations",
                [],
                |r| r.get(0),
            )
            .map_err(Error::storage("read the migration ledger"))
    }
}

/// What a store at `path` has not applied, without opening it as a [`Store`].
///
/// `specline migrate` needs this and cannot get it from a `Store`, because the
/// store it is about to migrate is precisely the one [`Store::open`] refuses —
/// and that refusal names `specline migrate`, so going through it would be a loop.
/// A plain connection and one `SELECT` is also honestly what the question is:
/// no configure, no vector registration, no id seeding, nothing that writes.
///
/// A store with no ledger table at all has applied nothing, which is what a
/// file that is not a Specline store looks like too. That is the right answer for
/// the first and a harmless one for the second, since migrating it would fail
/// on its own terms a moment later.
pub fn pending_migrations_at(path: impl AsRef<Path>) -> Result<Vec<(i32, String)>> {
    let path = path.as_ref();
    let conn = Connection::open(path).map_err(Error::storage(format!(
        "open the store at {} to read its migration ledger",
        path.display()
    )))?;

    let mut applied = std::collections::HashSet::new();
    if let Ok(mut stmt) = conn.prepare("SELECT id FROM _keel_migrations") {
        let rows = stmt
            .query_map([], |r| r.get::<_, i32>(0))
            .map_err(Error::storage("read the migration ledger"))?;
        for id in rows {
            applied.insert(id.map_err(Error::storage("read the migration ledger"))?);
        }
    }

    Ok(schema::migrations()
        .into_iter()
        .filter(|m| !applied.contains(&m.id))
        .map(|m| (m.id, m.name.to_owned()))
        .collect())
}

/// The newest schema this binary knows how to produce.
///
/// Free-standing because the interesting comparison — mine against yours — is
/// made by processes that have no store open, `specline migrate` deciding whether
/// to bother and the CLI reading a daemon's `/api/health` among them.
pub fn shipped_schema_version() -> i32 {
    schema::migrations().iter().map(|m| m.id).max().unwrap_or(0)
}

/// What an open should do about migrations it finds outstanding.
///
/// Two words rather than a `bool`, because `Store::open(path, true)` at a call
/// site says nothing about which way `true` goes, and this is a decision where
/// guessing wrong corrupts a store rather than merely misbehaving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// Apply them. Only the owner may.
    Apply,
    /// Refuse the open and say to run `specline migrate`.
    Refuse,
}

/// Whether an open claims the store against other writers.
///
/// A separate type rather than a `bool`, because `open_locked(path, pending,
/// true)` at a call site says nothing about what is true, and this is the
/// argument that decides whether two daemons can run at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exclusive {
    /// Take the lock and hold it for the life of the handle.
    Yes,
    /// Read alongside whoever is writing.
    No,
}

/// The outcome of a create.
///
/// `created: false` means an entity with this idempotency key already existed
/// and is being returned unchanged. A retrying agent gets a sane result rather
/// than a duplicate or an error it has to reason about (SPEC §7.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Created {
    /// The entity, new or pre-existing.
    pub entity: Entity,
    /// Whether this call is what brought it into being.
    pub created: bool,
}

/// A page of results that always tells the truth about what it left out.
///
/// Hard constraint 4: every list that can be cut reports that it was cut, with
/// a total. An agent that receives 10 of 40 open questions with no indication
/// will confidently re-litigate settled decisions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    /// The results.
    pub items: Vec<T>,
    /// How many matched in total, before any limit.
    pub total: usize,
    /// Whether `items` is shorter than `total`.
    pub truncated: bool,
}

impl<T> Page<T> {
    /// A page that was cut from a known total.
    pub fn new(items: Vec<T>, total: usize) -> Self {
        Page {
            truncated: items.len() < total,
            items,
            total,
        }
    }

    /// A page containing everything that matched.
    pub fn complete(items: Vec<T>) -> Self {
        Page {
            total: items.len(),
            truncated: false,
            items,
        }
    }

    /// Map the items, preserving the truncation report.
    pub fn map<U>(self, f: impl FnMut(T) -> U) -> Page<U> {
        Page {
            items: self.items.into_iter().map(f).collect(),
            total: self.total,
            truncated: self.truncated,
        }
    }
}

/// What a newest-first event read covers.
///
/// Its own type rather than two optional parameters, because "no project and no
/// entity" and "every project" are the same call with different meanings, and a
/// pair of `Option`s cannot say which was meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventScope<'a> {
    /// Every event in the store.
    Everything,
    /// Every event tagged with this project.
    Project(&'a EntityId),
    /// One row's own history.
    Entity(&'a EntityId),
}

/// Filters for listing entities.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EntityQuery {
    /// Restrict to one project. `None` means all projects.
    pub project_id: Option<EntityId>,
    /// Restrict to these types. Empty means all.
    pub entity_types: Vec<EntityType>,
    /// Restrict to these status values. Empty means all.
    pub statuses: Vec<String>,
    /// Include soft-deleted rows. Defaults to false — archived rows exist for
    /// recovery and audit, not for everyday lists.
    pub include_archived: bool,
    /// Created at or after this instant.
    pub since: Option<DateTime<Utc>>,
    /// Created strictly before this instant.
    pub until: Option<DateTime<Utc>>,
    /// Maximum rows to return. `None` means the store's default cap.
    pub limit: Option<usize>,
    /// Rows to skip.
    pub offset: usize,
}

impl EntityQuery {
    /// Everything in one project.
    pub fn in_project(project_id: EntityId) -> Self {
        EntityQuery {
            project_id: Some(project_id),
            ..Default::default()
        }
    }

    /// Restrict to one type.
    pub fn of_type(mut self, entity_type: EntityType) -> Self {
        self.entity_types = vec![entity_type];
        self
    }

    /// Restrict to several types.
    ///
    /// [`EntityQuery::of_type`] replaces rather than appends, so chaining it
    /// twice silently keeps only the second — which is a fair reading of "of
    /// type", and is also why a cross-type query could not be built with the
    /// builder at all and the offset bug in cross-type paging went years
    /// without a caller.
    pub fn of_types(mut self, types: impl IntoIterator<Item = EntityType>) -> Self {
        self.entity_types = types.into_iter().collect();
        self
    }

    /// Restrict to a set of statuses.
    pub fn with_status(mut self, statuses: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.statuses = statuses.into_iter().map(Into::into).collect();
        self
    }

    /// Cap the result count.
    pub fn limited(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// One node reached by a graph traversal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Neighbour {
    /// The entity reached.
    pub id: EntityId,
    /// Its type, denormalised on the edge so reaching it costs no join.
    pub entity_type: EntityType,
    /// What it is called, resolved through `v_entities`.
    ///
    /// Carried because a traversal result that is only an id cannot be
    /// rendered or reasoned about without a second round of lookups, and every
    /// caller was doing that round differently — the document reader showed
    /// bare ULIDs where a title belonged, and an agent walking the graph had to
    /// follow every hop with a `specline_get` to learn what it had found. Empty
    /// only if the edge points at a row that no longer resolves, which `fsck`
    /// reports as a dangling link.
    pub label: String,
    /// The relation on the edge that reached it.
    pub rel: Relation,
    /// The anchor on that edge, e.g. `REQ-4`. Empty means whole-entity.
    pub anchor: String,
    /// How many hops from the root. 1 is a direct neighbour.
    pub depth: u8,
    /// The full path from the root, inclusive of both ends. Carried so a
    /// caller can explain *why* something is reachable, which is most of the
    /// value of a traceability query.
    pub path: Vec<EntityId>,
}

/// A search request spanning both indexes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchQuery {
    /// The natural-language or keyword query.
    pub text: String,
    /// Restrict to one project.
    pub project_id: Option<EntityId>,
    /// Restrict to these types. Empty means every searchable type.
    pub entity_types: Vec<EntityType>,
    /// Created at or after.
    pub since: Option<DateTime<Utc>>,
    /// Created strictly before.
    pub until: Option<DateTime<Utc>>,
    /// How many results to return.
    pub limit: usize,
}

impl SearchQuery {
    /// A query with the default result count.
    pub fn new(text: impl Into<String>) -> Self {
        SearchQuery {
            text: text.into(),
            project_id: None,
            entity_types: Vec::new(),
            since: None,
            until: None,
            limit: 20,
        }
    }

    /// The inner retrieval depth.
    ///
    /// `k_inner = k_outer * 4` per SPEC §5. Retrieving exactly `k` from the
    /// index and *then* filtering by project and date is a classic way to
    /// return three results when forty exist.
    pub fn inner_limit(&self) -> usize {
        self.limit.saturating_mul(4).max(20)
    }
}

/// One search hit, from either index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    /// The entity found.
    pub entity_id: EntityId,
    /// Its type.
    pub entity_type: EntityType,
    /// The project it belongs to.
    pub project_id: Option<EntityId>,
    /// Its title or label.
    pub title: String,
    /// A short excerpt of the matching text.
    pub excerpt: String,
    /// The fused relevance score. Higher is better.
    pub score: f64,
    /// Which index produced this hit, kept so retrieval quality (R-3) can be
    /// evaluated per index rather than in aggregate — "is the semantic half
    /// earning its keep" is otherwise unanswerable.
    pub source: SearchSource,
}

/// Which index a hit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSource {
    /// The FTS5 keyword index, which covers every searchable artifact.
    Keyword,
    /// The `sqlite-vec` index, over prose embeddings.
    Semantic,
    /// Both found it. The strongest signal available here: an independent
    /// keyword match and an independent semantic match agreeing.
    Both,
}

/// Whether one half of hybrid search ran, and if not, why not.
///
/// Search degrades rather than failing: a daemon with no model, a build with no
/// vector extension, a type filter naming only types that have no prose — each
/// leaves one half silent while the other answers normally. That is the right
/// behaviour and it is also the dangerous one, because the results look the
/// same either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HalfStatus {
    /// It ran against its index. It may still have matched nothing.
    Ran,
    /// No embedding model was available, so there was no query vector to
    /// compare with. The usual cause is a daemon started without embeddings.
    NoModel,
    /// `sqlite-vec` did not register, so `vec_distance_cosine` does not exist.
    /// A property of the build, not of the store.
    NoVectorExtension,
    /// The `types` filter left this half nothing it could look at — asking the
    /// semantic half for tasks, say, when only five types carry prose.
    NoTypesInScope,
    /// The query text held no words to match on. Keyword only: punctuation is
    /// a search with no terms rather than a search that failed.
    NoTerms,
    /// It errored and the other half answered alone. The reason is in the log,
    /// because an error here is about the store rather than about the query.
    Failed,
}

impl HalfStatus {
    /// Whether this half contributed to the results.
    pub fn ran(self) -> bool {
        matches!(self, HalfStatus::Ran)
    }

    /// Why it did not run, in a sentence the caller can act on.
    ///
    /// `None` when it ran. Returned as prose rather than as a code because the
    /// caller who most needs this is a model reading a tool response, and
    /// `no_model` alone does not tell it what to do about it.
    pub fn why(self) -> Option<&'static str> {
        match self {
            HalfStatus::Ran => None,
            HalfStatus::NoModel => Some(
                "no embedding model is loaded, so nothing could be matched by meaning — the \
                 daemon was started without embeddings, or this build has none",
            ),
            HalfStatus::NoVectorExtension => {
                Some("the vector extension did not load, so this build cannot match by meaning")
            }
            HalfStatus::NoTypesInScope => {
                Some("the type filter left this half nothing it could search")
            }
            HalfStatus::NoTerms => Some("the query text held no words to match on"),
            HalfStatus::Failed => {
                Some("it failed and the other half answered alone; the daemon log has the reason")
            }
        }
    }
}

/// Which halves of hybrid search answered this query.
///
/// This exists because an empty result set is a claim about the store, and it
/// is only true if both halves were asked. A model told "no matches" by a
/// keyword-only search concludes nothing has been written about the subject and
/// goes on to re-derive it — which is the failure this codebase is organised
/// around, arriving through the one path that reports everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchReport {
    /// BM25 over every searchable artifact.
    pub keyword: HalfStatus,
    /// Cosine neighbours over the passages of the five prose types.
    pub semantic: HalfStatus,
}

impl SearchReport {
    /// Both halves ran. The only case in which "nothing matched" describes the
    /// store rather than the search.
    pub fn complete(&self) -> bool {
        self.keyword.ran() && self.semantic.ran()
    }

    /// The halves that ran, named. Empty when neither did.
    pub fn ran(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.keyword.ran() {
            out.push("keyword");
        }
        if self.semantic.ran() {
            out.push("semantic");
        }
        out
    }
}

/// A page of search hits, and which halves produced it.
///
/// One type rather than a tuple so that the report cannot be dropped by a
/// caller destructuring only what it wanted — which is how it went unreported
/// for as long as it did.
#[derive(Debug, Clone)]
pub struct SearchResults {
    /// The fused hits, with the usual truncation report.
    pub page: Page<SearchHit>,
    /// Which halves ran.
    pub report: SearchReport,
}

/// A stored binary blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob {
    /// `blb_…`
    pub blob_id: crate::BlobId,
    /// The entity it belongs to.
    pub entity_id: Option<EntityId>,
    /// The project it belongs to.
    pub project_id: Option<EntityId>,
    /// MIME type.
    pub media_type: String,
    /// The bytes.
    pub bytes: Vec<u8>,
    /// Content address.
    pub sha256: String,
    /// When it was stored.
    pub created_at: DateTime<Utc>,
}

impl Blob {
    /// A blob from raw bytes, content-addressed.
    ///
    /// `sha256` is computed here rather than accepted from the caller: it is
    /// what makes a re-upload of the same image detectable, and a hash the
    /// caller supplies is a hash nobody has checked.
    pub fn new(bytes: Vec<u8>, media_type: impl Into<String>, at: DateTime<Utc>) -> Self {
        let sha256 = crate::sha256_hex(&bytes);
        Blob {
            blob_id: crate::BlobId::generate(),
            entity_id: None,
            project_id: None,
            media_type: media_type.into(),
            bytes,
            sha256,
            created_at: at,
        }
    }

    /// Attach this blob to an entity and its project.
    pub fn owned_by(mut self, entity_id: EntityId, project_id: EntityId) -> Self {
        self.entity_id = Some(entity_id);
        self.project_id = Some(project_id);
        self
    }
}

/// Entity CRUD, links and events.
pub trait EntityStore {
    /// Create an entity, or return the existing one with the same idempotency
    /// key.
    ///
    /// Takes `&mut self` to express the single-writer rule at the type level
    /// (D-5), not because the engine requires it. SQLite in WAL mode would
    /// permit a second writer; the rule is Specline's, and the signature is where
    /// it is stated.
    fn create(&mut self, entity: Entity, provenance: &Provenance) -> Result<Created>;

    /// Fetch by id, archived or not. `None` means it never existed.
    fn get(&self, id: &EntityId) -> Result<Option<Entity>>;

    /// Apply a set of field changes under optimistic concurrency.
    ///
    /// `expected_version` is the `version` the caller read. A mismatch is
    /// [`crate::Error::StaleVersion`], which the daemon turns into SPEC §7.3's
    /// 409 with the current state attached.
    fn update(
        &mut self,
        id: &EntityId,
        expected_version: i32,
        changes: &serde_json::Map<String, serde_json::Value>,
        provenance: &Provenance,
    ) -> Result<Entity>;

    /// Soft-delete. Nothing is ever `DELETE`d (D-9).
    fn archive(
        &mut self,
        id: &EntityId,
        expected_version: i32,
        provenance: &Provenance,
    ) -> Result<Entity>;

    /// List entities matching a query.
    fn list(&self, query: &EntityQuery) -> Result<Page<Entity>>;

    /// Create an edge, normalising `depends_on` into `blocks` on the way in.
    fn link(&mut self, link: NewLink, provenance: &Provenance) -> Result<Link>;

    /// Archive an edge. Sets `archived_at`; it does not `DELETE`.
    fn unlink(
        &mut self,
        from_id: &EntityId,
        rel: Relation,
        to_id: &EntityId,
        anchor: &str,
        provenance: &Provenance,
    ) -> Result<Link>;

    /// Append to the mutation log.
    fn append_event(&mut self, event: NewEvent, provenance: &Provenance) -> Result<Event>;

    /// Which program drove a conversation, if it was ever recorded.
    ///
    /// `None` covers three different things on purpose, and a caller must not
    /// tell them apart by guessing: a session that predates KEEL-360, one whose
    /// transport reported no client, and one that never wrote anything. All
    /// three are honestly *unknown*, and the one wrong answer is to assume
    /// Claude Code — right often enough to be believed, wrong exactly where a
    /// second editor makes the question worth asking.
    fn client_for_session(&self, session_id: &str) -> Result<Option<SessionClient>>;

    /// Every session that has named its client, most recently seen first.
    ///
    /// The answer to "which editors are talking to Specline", subject to the
    /// caveat that this reports *last seen* and not *connected*: MCP over HTTP
    /// is stateless, so there is no connection to report and a caller that
    /// renders one is inventing it.
    fn session_clients(&self, limit: usize) -> Result<Vec<SessionClient>>;

    /// Read the mutation log from a cursor.
    fn events(
        &self,
        cursor: &Cursor,
        project_id: Option<&EntityId>,
        limit: usize,
    ) -> Result<Page<Event>>;

    /// The newest `limit` events in a scope, most recent first.
    ///
    /// The counterpart to [`EntityStore::events`], which is a *feed*: it goes
    /// oldest-first because a cursor-following caller must see every event
    /// exactly once, and a limit there keeps the beginning.
    ///
    /// Four callers wanted the other end and all four asked the feed for a
    /// generous number of rows and reversed the answer in Rust. That is right
    /// until the log passes the number, and then it is silently, plausibly
    /// wrong: it keeps the *oldest* rows and calls them recent. One of the four
    /// was already broken in the live store — the 409 payload read the oldest
    /// 500 events out of 804, so an agent resolving a stale write was shown
    /// history from before the conflict and nothing near it.
    ///
    /// `total` is the number of events in scope, so a caller can still say what
    /// it left out.
    fn recent_events(&self, scope: EventScope<'_>, limit: usize) -> Result<Page<Event>>;

    /// Turn whatever a caller wrote into an id: a ULID, or `KEEL-42`.
    ///
    /// The point of a readable identifier is that a human can say it out loud
    /// and type it into a conversation, so every place that takes an id has to
    /// take one. `Ok(None)` means it names nothing — which is a legitimate
    /// answer to "does this exist", and distinct from a malformed reference.
    fn resolve_ref(&self, reference: &str) -> Result<Option<EntityId>>;

    /// The next unused task number in a project. Never reuses one.
    fn next_task_number(&self, project_id: &EntityId) -> Result<i32>;

    /// A rank that puts a new task at the end of the deliberate order.
    fn next_task_rank(&self, project_id: &EntityId) -> Result<f64>;

    /// A rank that sits between two neighbours, either of which may be absent.
    ///
    /// This is what "move it above the auth work" resolves to. Fractional, so
    /// the move touches one row rather than renumbering everything below it.
    fn rank_between(&self, before: Option<f64>, after: Option<f64>) -> Result<f64>;

    /// Reject a parent that does not exist, is in another project, or would
    /// make a cycle. Called on the way in, because a cycle is unrenderable and
    /// the store is the only place that can see the whole chain.
    fn check_task_parent(&self, task: &crate::Task) -> Result<()>;

    /// A project key that no other project holds, starting from `base`.
    fn unique_project_key(&self, base: &str) -> Result<String>;

    /// The id of the most recent event, if there is one.
    ///
    /// One row, not a scan. The daemon reads this twice per tool call to notice
    /// that something changed, and it used to do so by fetching up to 100,000
    /// events and taking the last — twice, per call, while holding the global
    /// write lock. On a store of a few hundred events that was merely wasteful;
    /// it is quadratic in the wrong direction and the lock made it everyone's
    /// problem.
    fn latest_event_id(&self) -> Result<Option<crate::EventId>>;

    /// One entity's history, oldest first.
    ///
    /// Separate from [`EntityStore::events`] rather than another parameter on
    /// it: that one is a cursor-following feed over a whole project, where
    /// paging must visit every event exactly once, and a filter that removes
    /// rows from under a cursor breaks that guarantee. This one answers a
    /// different question — "what has happened to *this*" — and a row's whole
    /// history is small enough to want in one piece.
    fn events_for(&self, entity_id: &EntityId, limit: usize) -> Result<Page<Event>>;

    /// Append a note to a row's running commentary.
    ///
    /// Fails if the subject does not exist. A note pointing at nothing is
    /// unrecoverable in a way an ordinary orphan is not — nothing links to a
    /// note, so there is no traversal that would ever surface it again.
    fn add_note(&mut self, note: NewNote, provenance: &Provenance) -> Result<Note>;

    /// One row's notes, oldest first.
    ///
    /// Retracted notes are excluded unless `include_retracted`, because the
    /// overwhelmingly common caller is a renderer showing current commentary,
    /// and making that caller filter is how retracted notes end up in output.
    fn notes_for(&self, entity_id: &EntityId, include_retracted: bool) -> Result<Vec<Note>>;

    /// Every live note in a project, oldest first.
    ///
    /// The renderer needs fifty streams at once; asking for them one row at a
    /// time is fifty round trips to answer one question.
    fn notes_in_project(&self, project_id: &EntityId) -> Result<Vec<Note>>;

    /// Retract a note. Soft, like every other removal in the store.
    fn retract_note(&mut self, id: &crate::NoteId, provenance: &Provenance) -> Result<Note>;
}

/// Link traversal. Nobody hand-writes a recursive CTE at a call site.
pub trait GraphStore {
    /// Walk the graph from `root`.
    ///
    /// `direction` is [`Direction::Outbound`] to follow edges away from the
    /// root and [`Direction::Inbound`] to follow edges into it. Getting this
    /// backwards returns an empty set that looks exactly like a legitimate
    /// "nothing is linked here" — read `product/SPEC.md` §3.3 before choosing.
    ///
    /// An empty `rels` means every stored relation. `depth` is clamped to
    /// [`crate::MAX_DEPTH`].
    fn neighbours(
        &self,
        root: &EntityId,
        direction: Direction,
        rels: &[Relation],
        depth: u8,
    ) -> Result<Vec<Neighbour>>;

    /// The edges immediately touching an entity, unwalked.
    fn links_of(&self, id: &EntityId, direction: Direction) -> Result<Vec<Link>>;

    /// Every live edge of one relation within a project.
    ///
    /// For the questions that are about a whole project rather than about one
    /// row: which of these thirty tasks is blocked, and by what. Answering that
    /// with [`GraphStore::links_of`] means one query per task, three times over
    /// in a single digest — fine at thirty and a latency cliff at three
    /// hundred, all of it under the daemon's one lock.
    ///
    /// `blocks` is the only relation with a caller today, and the parameter is
    /// there because the shape of the question is not specific to it.
    fn links_in_project(&self, project_id: &EntityId, rel: Relation) -> Result<Vec<Link>>;
}

/// Revisions, blobs, embeddings and search.
pub trait DocumentStore {
    /// Append a revision. Returns the stored document, whose `version` is
    /// whatever the store actually assigned.
    fn write_revision(&mut self, document: Document) -> Result<Document>;

    /// Fetch a revision — the current one if `version` is `None`.
    fn revision(&self, entity_id: &EntityId, version: Option<i32>) -> Result<Option<Document>>;

    /// Every revision of a document, oldest first.
    fn revisions(&self, entity_id: &EntityId) -> Result<Vec<Document>>;

    /// A unified diff between two revisions. Satisfies REQ-2 at the API layer,
    /// not only in the UI.
    fn diff(&self, entity_id: &EntityId, from: i32, to: i32) -> Result<DocumentDiff>;

    /// Hybrid search across both indexes, fused.
    fn search(&self, query: &SearchQuery) -> Result<Page<SearchHit>>;

    /// Store bytes.
    fn put_blob(&mut self, blob: Blob) -> Result<crate::BlobId>;

    /// Fetch bytes.
    fn get_blob(&self, blob_id: &crate::BlobId) -> Result<Option<Blob>>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_page_reports_truncation_honestly() {
        let cut = Page::new(vec![1, 2, 3], 40);
        assert!(cut.truncated);
        assert_eq!(cut.total, 40);

        let whole = Page::new(vec![1, 2, 3], 3);
        assert!(!whole.truncated);

        let complete = Page::complete(vec![1, 2, 3]);
        assert!(!complete.truncated);
        assert_eq!(complete.total, 3);
    }

    #[test]
    fn mapping_a_page_preserves_the_truncation_report() {
        let mapped = Page::new(vec![1, 2], 9).map(|n| n * 2);
        assert_eq!(mapped.items, vec![2, 4]);
        assert_eq!(mapped.total, 9);
        assert!(mapped.truncated);
    }

    #[test]
    fn inner_search_limit_is_four_times_the_outer() {
        let q = SearchQuery {
            limit: 25,
            ..SearchQuery::new("onboarding")
        };
        assert_eq!(q.inner_limit(), 100, "SPEC §5: k_inner = k_outer * 4");
    }

    #[test]
    fn a_tiny_outer_limit_still_retrieves_enough_to_filter() {
        // k=1 would otherwise retrieve 4 rows, and a project filter could
        // then discard all of them and report "no results" wrongly.
        let q = SearchQuery {
            limit: 1,
            ..SearchQuery::new("x")
        };
        assert!(q.inner_limit() >= 20);
    }

    /// Every column the row mapping declares must exist in the schema, for all
    /// thirteen types.
    ///
    /// This is the highest-value test in the file. `TableSpec` drives both the
    /// `SELECT` list and the `INSERT` parameter order, so a column the spec
    /// names and the schema lacks is not a compile error and not a nice
    /// message — it is an insert that fails at runtime for one entity type
    /// while the other twelve work, or worse, a select that silently returns
    /// nothing for a field nobody looks at often.
    ///
    /// Asked of a real database via `PRAGMA table_info` rather than by
    /// grepping the DDL string, because the question is what SQLite actually
    /// built, not what the text appears to say.
    #[test]
    fn the_schema_has_every_column_the_row_specs_declare() {
        let store = Store::in_memory().unwrap();
        let mut missing: Vec<String> = Vec::new();

        for ty in crate::EntityType::ALL {
            let spec = crate::store::rows::spec_for(ty);

            let mut stmt = store
                .connection()
                .prepare(&format!("PRAGMA table_info({})", spec.table))
                .unwrap();
            let actual: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap();

            assert!(
                !actual.is_empty(),
                "the schema has no table called {} for {}",
                spec.table,
                ty.as_str()
            );

            // The audit block is shared and is appended to every insert, so it
            // has to be present on every entity table too.
            const AUDIT: &[&str] = &[
                "created_at",
                "updated_at",
                "version",
                "created_by",
                "updated_by",
                "session_id",
                "surface",
                "archived_at",
            ];

            let wanted = spec
                .cols
                .iter()
                .map(|c| match c {
                    crate::store::rows::Col::Plain(n) | crate::store::rows::Col::Array(n) => *n,
                })
                .chain(AUDIT.iter().copied());

            for col in wanted {
                if !actual.iter().any(|a| a == col) {
                    missing.push(format!("{}.{col} (for {})", spec.table, ty.as_str()));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "the row specs name columns the schema does not have:\n  {}",
            missing.join("\n  ")
        );
    }

    /// The keyword index updates as rows change, with no rebuild.
    ///
    /// This is the whole of KEEL-123's stall, asserted. The store this replaced
    /// could not do it: its full-text index did not update when its table
    /// changed, so the first search after *any* write rebuilt the entire index
    /// — 217 ms against a 13 ms mean, measured on the live store while a
    /// decision was being written. FTS5's triggers keep the index in step with
    /// the rows, so there is no rebuild to pay for.
    ///
    /// A test that only inserted would pass against an index that is never
    /// maintained again, so this also changes a row and archives one.
    #[test]
    fn the_keyword_index_follows_the_rows_without_a_rebuild() {
        let store = Store::in_memory().unwrap();
        let conn = store.connection();

        // Quoted, because `MATCH` takes a query *language*, not a string.
        // `local-first` unquoted parses as the term `local` with a column
        // filter, and fails with "no such column: first" — which names a word
        // from the text and sounds like a schema problem. Whatever lands
        // KEEL-126 has to quote caller input for exactly this reason.
        let matches = |q: &str| -> i64 {
            conn.query_row(
                "SELECT count(*) FROM fts_entities WHERE fts_entities MATCH ?1",
                [format!("\"{q}\"")],
                |r| r.get(0),
            )
            .unwrap()
        };

        conn.execute_batch(
            "INSERT INTO projects
               (id, slug, key, name, description, idempotency_key, created_at,
                updated_at, created_by, updated_by, version)
             VALUES ('prj_1','specline','KEEL','Specline','a local-first store','k1',
                     '2026-08-11T00:00:00.000000Z','2026-08-11T00:00:00.000000Z',
                     'claude','claude',1);
             INSERT INTO tasks
               (id, project_id, number, title, body, summary, idempotency_key,
                created_at, updated_at, created_by, updated_by, version)
             VALUES ('tsk_1','prj_1',1,'The board is slow',
                     'the keyword index is rebuilt on every write','a summary','k2',
                     '2026-08-11T00:00:00.000000Z','2026-08-11T00:00:00.000000Z',
                     'claude','claude',1);",
        )
        .unwrap();

        // Findable immediately, in the very next statement, with nothing having
        // asked the index to catch up.
        assert_eq!(
            matches("keyword"),
            1,
            "a row written should be findable at once"
        );
        assert_eq!(
            matches("local-first"),
            1,
            "the project should be indexed too"
        );

        // An update has to move the index with it, or search keeps returning
        // text that is no longer there.
        conn.execute(
            "UPDATE tasks SET body = 'now it says something else entirely' WHERE id = 'tsk_1'",
            [],
        )
        .unwrap();
        assert_eq!(
            matches("keyword"),
            0,
            "the old text should have left the index"
        );
        assert_eq!(matches("entirely"), 1, "the new text should be in it");

        // Archiving takes it out. Search must not offer something a person put
        // away, and doing that here means no query has to remember to filter.
        conn.execute(
            "UPDATE tasks SET archived_at = '2026-08-11T01:00:00.000000Z' WHERE id = 'tsk_1'",
            [],
        )
        .unwrap();
        assert_eq!(
            matches("entirely"),
            0,
            "an archived row should leave the index"
        );
    }

    /// A prose type is indexed from its document, not its row, and a new
    /// revision replaces its predecessor rather than piling up beside it.
    ///
    /// Without the replace, a heavily-edited spec would appear once per version
    /// and outrank everything by sheer repetition.
    #[test]
    fn a_document_is_indexed_once_however_many_revisions_it_has() {
        let store = Store::in_memory().unwrap();
        let conn = store.connection();

        conn.execute_batch(
            "INSERT INTO documents
               (doc_id, entity_type, entity_id, project_id, version, title, body,
                body_hash, status, author, created_at)
             VALUES ('doc_1','spec','spc_1','prj_1',1,'A spec','the original wording',
                     'h1','current','claude','2026-08-11T00:00:00.000000Z');",
        )
        .unwrap();

        conn.execute_batch(
            "UPDATE documents SET status = 'superseded' WHERE doc_id = 'doc_1';
             INSERT INTO documents
               (doc_id, entity_type, entity_id, project_id, version, title, body,
                body_hash, status, author, created_at)
             VALUES ('doc_2','spec','spc_1','prj_1',2,'A spec','the replacement wording',
                     'h2','current','claude','2026-08-11T00:00:01.000000Z');",
        )
        .unwrap();

        let count = |q: &str| -> i64 {
            conn.query_row(
                "SELECT count(*) FROM fts_entities WHERE fts_entities MATCH ?1",
                [q],
                |r| r.get(0),
            )
            .unwrap()
        };

        assert_eq!(
            count("replacement"),
            1,
            "the current revision should be findable"
        );
        assert_eq!(
            count("original"),
            0,
            "the superseded revision should not still be in the index"
        );

        let rows: i64 = conn
            .query_row(
                "SELECT count(*) FROM fts_source WHERE entity_id = 'spc_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1, "two revisions should occupy one slot, not two");
    }

    #[test]
    fn a_new_store_has_every_table() {
        let store = Store::in_memory().unwrap();
        let mut stmt = store
            .connection()
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view') ORDER BY name")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();

        for expected in [
            "projects",
            "milestones",
            "tasks",
            "specs",
            "decisions",
            "questions",
            "terms",
            "feedback",
            "design_artifacts",
            "environments",
            "artifacts",
            "metrics",
            "metric_observations",
            "links",
            "notes",
            "events",
            "documents",
            "blobs",
            "v_entities",
            "fts_entities",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "a new store is missing {expected}; it has {names:?}"
            );
        }
    }

    /// Opening twice must not re-run migration 1 against tables that exist.
    /// This is the failure the ledger prevents, and it is silent until the
    /// second open.
    #[test]
    fn opening_an_existing_store_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("specline.sqlite");

        let first = Store::open(&path).unwrap();
        let applied: i64 = first
            .connection()
            .query_row("SELECT count(*) FROM _keel_migrations", [], |r| r.get(0))
            .unwrap();
        drop(first);

        let second = Store::open(&path).unwrap();
        let again: i64 = second
            .connection()
            .query_row("SELECT count(*) FROM _keel_migrations", [], |r| r.get(0))
            .unwrap();

        assert_eq!(
            applied, again,
            "reopening applied a migration a second time"
        );
    }

    /// A file store must be in WAL, or a reader blocks behind every write and
    /// the board stalls exactly as it did before.
    #[test]
    fn a_file_store_uses_write_ahead_logging() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("specline.sqlite")).unwrap();
        let mode: String = store
            .connection()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    /// `STRICT` is what makes a bad write fail at the write. Without it SQLite
    /// stores the string in the integer column and the failure surfaces
    /// somewhere else entirely, usually as a deserialisation error on a read
    /// that has nothing to do with whoever wrote it.
    ///
    /// The direction matters and it is easy to get backwards: STRICT converts
    /// where the conversion is lossless, so an integer *into* a TEXT column is
    /// accepted and becomes text. What it refuses is text that is not a number
    /// going into an INTEGER column, which is the mistake worth catching.
    #[test]
    fn text_in_an_integer_column_is_refused() {
        let store = Store::in_memory().unwrap();
        let bad = store.connection().execute(
            "INSERT INTO projects
               (id, slug, name, idempotency_key, created_at, updated_at,
                created_by, updated_by, version)
             VALUES ('prj_1', 'p', 'P', 'k', 'now', 'now', 'claude', 'claude', 'not a number')",
            [],
        );
        assert!(
            bad.is_err(),
            "STRICT should have refused non-numeric text in an INTEGER column"
        );

        // The lossless direction is accepted, and asserting it keeps the test
        // honest about what STRICT does rather than what it is hoped to do.
        let ok = store.connection().execute(
            "INSERT INTO projects
               (id, slug, name, idempotency_key, created_at, updated_at,
                created_by, updated_by, version)
             VALUES ('prj_2', 'q', 'Q', 'k2', 'now', 'now', 'claude', 'claude', 3)",
            [],
        );
        assert!(ok.is_ok(), "a well-typed insert should have been accepted");
    }

    #[test]
    fn the_store_reports_where_it_lives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("specline.sqlite");
        let store = Store::open(&path).unwrap();
        assert_eq!(store.path(), path);
        assert!(
            path.exists(),
            "open should have created the parent directory"
        );
    }
}
