//! `specline doctor` — one command that asks whether anything has quietly gone
//! wrong.
//!
//! This codebase's defining fear is a failure that looks calm and correct: an
//! empty search result that should have had rows in it, a mirror describing a
//! store from last week, a database that reads fine and has drifted from the
//! repository beside it. Every one of those has a check somewhere — `fsck`,
//! `generate --check`, a count of documents without vectors — and none of them
//! is anywhere a person would look, because looking means knowing which
//! question to ask.
//!
//! So this asks all of them and prints one page. It is read-only: nothing here
//! writes to the store, generates a file or repairs anything, because the value
//! is in being safe to run at any moment on any store, including one you are
//! worried about.
//!
//! # What "healthy" means here
//!
//! Exit code is non-zero only for a **problem**, not for anything merely worth
//! knowing. A store with no embeddings is degraded and says so; a store whose
//! pages are damaged is broken. Conflating the two would make the exit code
//! useless for a hook, which is the same mistake as a check that cries wolf.

use anyhow::{Context, Result};
use serde::Serialize;
use specline_core::{Entity, EntityQuery, EntityStore, EntityType, Store, fsck, generate};
use std::path::Path;

/// How stale a backup has to be before it is worth mentioning.
const BACKUP_STALE_DAYS: i64 = 7;

/// How far ahead of the wall clock an id can be before the clock has stepped.
///
/// A second of slack absorbs ordinary drift between the moment an id was minted
/// and the moment this reads the clock. Anything beyond it means the machine's
/// clock went backwards — a laptop waking from sleep, an NTP correction — which
/// is the condition that makes the event feed go silent.
const CLOCK_SKEW_SECONDS: i64 = 1;

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Nothing wrong.
    Ok,
    /// Working, but worse than it should be. Does not fail the exit code.
    Degraded,
    /// Actually broken. Fails the exit code.
    Problem,
}

impl Level {
    fn marker(self) -> &'static str {
        match self {
            Level::Ok => "ok     ",
            Level::Degraded => "warn   ",
            Level::Problem => "PROBLEM",
        }
    }
}

/// One thing that was checked.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// A short name, stable enough to grep for.
    pub name: String,
    /// How it went.
    pub level: Level,
    /// What was found, in a sentence.
    pub detail: String,
    /// What to do about it. Empty when there is nothing to do.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remedy: String,
}

impl Check {
    fn ok(name: &str, detail: impl Into<String>) -> Self {
        Check {
            name: name.to_owned(),
            level: Level::Ok,
            detail: detail.into(),
            remedy: String::new(),
        }
    }

    fn degraded(name: &str, detail: impl Into<String>, remedy: impl Into<String>) -> Self {
        Check {
            name: name.to_owned(),
            level: Level::Degraded,
            detail: detail.into(),
            remedy: remedy.into(),
        }
    }

    fn problem(name: &str, detail: impl Into<String>, remedy: impl Into<String>) -> Self {
        Check {
            name: name.to_owned(),
            level: Level::Problem,
            detail: detail.into(),
            remedy: remedy.into(),
        }
    }
}

/// The whole report.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Every check, in the order they ran.
    pub checks: Vec<Check>,
}

impl Report {
    /// Whether anything is actually broken.
    pub fn is_healthy(&self) -> bool {
        !self.checks.iter().any(|c| c.level == Level::Problem)
    }
}

/// Run every check and print the result.
pub fn run(home: &Path, daemon: &str, json: bool) -> Result<()> {
    let report = examine(home, daemon)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for check in &report.checks {
            println!("{}  {}", check.level.marker(), check.name);
            println!("         {}", check.detail);
            if !check.remedy.is_empty() {
                println!("         → {}", check.remedy);
            }
        }
        println!();
        let problems = report
            .checks
            .iter()
            .filter(|c| c.level == Level::Problem)
            .count();
        let degraded = report
            .checks
            .iter()
            .filter(|c| c.level == Level::Degraded)
            .count();
        if problems == 0 && degraded == 0 {
            println!(
                "healthy — {} checks, nothing to report",
                report.checks.len()
            );
        } else {
            println!(
                "{} check(s): {problems} problem(s), {degraded} worth knowing",
                report.checks.len()
            );
        }
    }

    if !report.is_healthy() {
        std::process::exit(1);
    }
    Ok(())
}

/// Every check, without printing anything.
///
/// Separate from [`run`] so a test can assert on the findings rather than on
/// stdout, and so a future surface can render the same report differently.
pub fn examine(home: &Path, daemon: &str) -> Result<Report> {
    let mut checks = Vec::new();

    // --- Is a daemon up ---------------------------------------------------
    //
    // Not a problem either way: the CLI works without one. It is the first
    // thing to say because it changes what every other line means — a check
    // run against a store nothing is writing to is a different check.
    // Kept, not only pushed: the semantic-search check below needs to know
    // whether there is a daemon at all, because "no model loaded" and "no
    // daemon to load one into" are different findings and only the first is
    // worth a remedy here.
    let daemon_state = crate::writes::probe(daemon);
    checks.push(match &daemon_state {
        crate::writes::Daemon::Listening => Check::ok(
            "daemon",
            format!("a daemon is listening at {daemon} and owns the write path"),
        ),
        crate::writes::Daemon::NotRunning => Check::degraded(
            "daemon",
            format!("nothing is listening at {daemon}"),
            "start it with `specline serve`, or ignore this if you meant to run without one — \
             MCP and the desktop app both need it",
        ),
        crate::writes::Daemon::Unknown(why) => Check::degraded(
            "daemon",
            format!("could not tell whether a daemon is running at {daemon}: {why}"),
            "check the address",
        ),
    });

    // --- Is the store somewhere safe to keep a SQLite database ------------
    //
    // Degraded rather than a problem, because the detection is a heuristic and
    // an exit code that fails on a false positive is one nobody can put in a
    // hook. It is high in the report on purpose: it is the condition that
    // explains a `page_integrity` finding further down.
    let hazards = specline_core::hazards(home);
    checks.push(if hazards.is_empty() {
        Check::ok(
            "location",
            format!(
                "{} is not under a known sync or network root",
                home.display()
            ),
        )
    } else {
        Check::degraded(
            "location",
            hazards
                .iter()
                .map(specline_core::Hazard::detail)
                .collect::<Vec<_>>()
                .join("; "),
            hazards
                .iter()
                .map(specline_core::Hazard::remedy)
                .collect::<Vec<_>>()
                .join("; "),
        )
    });

    // --- Is the schema where this binary expects it -----------------------
    //
    // Before the store is opened, because it decides whether it can be. This
    // is also the one check that has to run first for a duller reason: with
    // migrations pending `Store::open` refuses, so asking anything else would
    // mean reporting "could not open the store" for a condition that has a
    // name and a fix.
    let path = specline_core::store_path(home);
    let pending = specline_core::pending_migrations_at(&path).unwrap_or_default();
    checks.push(if pending.is_empty() {
        Check::ok(
            "schema",
            format!(
                "the store is at schema {}, which is what this binary ships",
                specline_core::shipped_schema_version()
            ),
        )
    } else {
        let names = pending
            .iter()
            .map(|(id, name)| format!("{id} ({name})"))
            .collect::<Vec<_>>()
            .join(", ");
        Check::problem(
            "schema",
            format!(
                "{} migration(s) have not been applied: {names}",
                pending.len()
            ),
            "stop the daemon and run `specline migrate`. Nothing applies them on its own, \
             deliberately — a schema change from whichever process opened the store next is \
             how the tables move under a daemon that is already running",
        )
    });
    if !pending.is_empty() {
        // Everything below reads the store, and this binary will not open one
        // whose schema it has not been allowed to bring up to date. Report what
        // is known rather than failing with a message about a lock.
        return Ok(Report { checks });
    }

    // Opening alongside a live daemon is safe in WAL mode, which is the whole
    // reason `fsck` stopped requiring one to be stopped (TQ-15).
    let store = crate::open(home).context("open the store to examine it")?;

    // --- The file itself --------------------------------------------------
    checks.push(match fsck::page_integrity(&store, "quick_check") {
        Ok(None) => Check::ok(
            "page_integrity",
            "SQLite reports the database file as sound",
        ),
        Ok(Some(problems)) => Check::problem(
            "page_integrity",
            format!("the database file is damaged: {problems}"),
            "restore from a backup (`specline restore`), then check whether ~/.specline is inside a \
             Dropbox, iCloud or network folder — copying .sqlite, -wal and -shm at different \
             moments is the usual cause",
        ),
        Err(e) => Check::degraded(
            "page_integrity",
            format!("could not run the integrity check: {e}"),
            "this is not a report that the store is damaged, only that nothing could tell",
        ),
    });

    // --- Referential integrity -------------------------------------------
    let fsck_report = fsck::check(&store).context("run the integrity checks")?;
    let errors = fsck_report.errors().count();
    let warnings = fsck_report.findings.len() - errors;
    checks.push(if errors > 0 {
        Check::problem(
            "fsck",
            format!(
                "{errors} error-level finding(s) across {} checks: {}",
                fsck_report.checks_run,
                fsck_report
                    .errors()
                    .map(|f| f.check.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "run `specline fsck` for the detail and the remedy for each",
        )
    } else if warnings > 0 {
        Check::degraded(
            "fsck",
            format!(
                "{warnings} warning(s) across {} checks",
                fsck_report.checks_run
            ),
            "run `specline fsck` to see them — none of them stops anything working",
        )
    } else {
        Check::ok(
            "fsck",
            format!("{} checks, nothing found", fsck_report.checks_run),
        )
    });

    // --- Semantic search --------------------------------------------------
    //
    // The silent one. Search keeps answering with keyword hits, so a store with
    // no vectors at all looks exactly like a store with them — which is how
    // 227 documents went unembedded for months without anything saying so.
    let (current, without) = embedding_coverage(&store)?;
    checks.push(if !specline_daemon::EMBEDDINGS_BUILT_IN {
        // A property of the binary, not of the store, and it outranks the count
        // below: "none of your documents has a vector" reads as something to
        // fix, and on this build it is not. Two of the three release targets
        // cannot link the ONNX runtime at all (KEEL-220), so a version number
        // does not tell you which one you have and this does.
        Check::ok(
            "embeddings",
            format!(
                "not built into this binary, so search is keyword-only by construction — \
                 {without} of {current} current document(s) have no vector and nothing here \
                 can add one"
            ),
        )
    } else if current == 0 {
        Check::ok("embeddings", "no documents yet, so nothing to embed")
    } else if without == 0 {
        Check::ok(
            "embeddings",
            format!("all {current} current document(s) have a vector"),
        )
    } else if without == current {
        Check::degraded(
            "embeddings",
            format!(
                "none of the {current} current document(s) has a vector, so hybrid search has \
                 only ever returned keyword hits"
            ),
            "run `specline reembed --missing`, and start the daemon with embeddings enabled so new \
             revisions are embedded on the way in",
        )
    } else {
        Check::degraded(
            "embeddings",
            format!("{without} of {current} current document(s) have no vector"),
            "run `specline reembed --missing`",
        )
    });

    // --- Is the half that reads meaning actually running ------------------
    //
    // The check above is about the store: are there vectors. This one is about
    // the process: is anything able to make a query vector to compare them
    // with. They come apart completely — a fully embedded store served by a
    // daemon with no model does keyword search and looks perfect here.
    //
    // Only asked when this binary could have a model at all; when it could
    // not, the check above has already said so and a second line saying it
    // again is noise.
    if specline_daemon::EMBEDDINGS_BUILT_IN && daemon_state == crate::writes::Daemon::Listening {
        checks.push(match crate::writes::embedder_loaded(daemon) {
            Some(true) => Check::ok(
                "semantic_search",
                format!("the daemon at {daemon} has the embedding model loaded, so searches run both halves"),
            ),
            Some(false) => Check::degraded(
                "semantic_search",
                format!(
                    "the daemon at {daemon} has no embedding model loaded, so every search is \
                     keyword-only — a query that shares no words with what it is looking for \
                     finds nothing, and says so as though the store were empty"
                ),
                "restart it as `specline-daemon --embeddings`",
            ),
            // The daemon answered the port but not this question: an older
            // build whose health payload predates the field, or a store busy
            // enough that it declined to guess.
            None => Check::degraded(
                "semantic_search",
                format!("the daemon at {daemon} did not say whether it has a model loaded"),
                "check its version — `/api/health` reports this from 0.4.1",
            ),
        });
    }

    // --- Is the passage index still describing the store? -----------------
    //
    // The check above answers "is anything missing". This one answers the
    // harder question: does what *is* there still describe what the documents
    // now say. A passage left behind by an edit ranks for ever and search gives
    // no sign of it — the semantic half has no status predicate, deliberately,
    // because the triggers are supposed to have deleted it.
    //
    // Read out of the `fsck` report rather than queried again here. `doctor`
    // already had its own copy of the embedding-coverage query, which stayed
    // behind when the store's version learned to exclude archived entities —
    // two copies of a question drift in exactly one direction, towards the one
    // nobody is looking at.
    checks.push(passage_index(&fsck_report));

    // --- The repository beside the store ---------------------------------
    checks.extend(mirror_drift(&store)?);

    // --- The one thing that leaves this machine ---------------------------
    checks.push(update_check());

    // --- Backups ----------------------------------------------------------
    checks.push(backup_age(home));

    // --- The clock --------------------------------------------------------
    //
    // Everything about event ordering assumes ULIDs only ever grow, which is
    // true within one process and false the moment the wall clock steps back.
    // The newest id is the cheapest place to notice.
    checks.push(clock_sanity(&store)?);

    Ok(Report { checks })
}

/// How many current revisions there are, and how many lack an embedding.
/// Whether the passage index still matches the documents it was built from.
///
/// Two findings feed this, and they mean different things. `stale_passage` is a
/// problem: a trigger that should have deleted a passage did not, and search is
/// returning text the store has moved on from as though it were current.
/// `passages_from_mixed_models` is not a fault at all — it is the ordinary
/// state during a model change, and it is reported because vectors of a
/// different width are *skipped* by search rather than failing it, so the rows
/// stop being findable without anything going wrong out loud.
fn passage_index(report: &fsck::FsckReport) -> Check {
    let find = |name: &str| report.findings.iter().find(|f| f.check == name);

    match (find("stale_passage"), find("passages_from_mixed_models")) {
        (Some(stale), _) => Check::problem(
            "passage_index",
            format!(
                "{} passage(s) describe a revision that has been superseded, archived or \
                 edited since. Semantic search returns them as current",
                stale.count
            ),
            &stale.remedy,
        ),
        (None, Some(mixed)) => Check::degraded(
            "passage_index",
            format!(
                "{} passage(s) were written by a different model from the rest. Vectors of \
                 another width are skipped by search, so those rows are not findable",
                mixed.count
            ),
            &mixed.remedy,
        ),
        (None, None) => Check::ok(
            "passage_index",
            "every passage matches the revision it was built from",
        ),
    }
}

fn embedding_coverage(store: &Store) -> Result<(i64, i64)> {
    // Delegated rather than asked directly, and that is the whole point of the
    // function still existing. This used to be its own pair of `SELECT count(*)`
    // statements over `documents`, which was the same question the store already
    // answered — until the store's version learned to exclude archived entities
    // and this one did not. Two copies of a query drift in exactly one
    // direction: the one nobody is looking at. Here it would have reported
    // thirteen documents permanently missing vectors they can never be given,
    // which is a check that can only ever be red and therefore a check nobody
    // reads.
    store
        .documents_missing_embeddings(None)
        .context("count how many current revisions have no vector")
}

/// Whether each project's committed markdown still matches the store.
///
/// One check per project with a checkout, because "the mirror is stale" is only
/// actionable if it says *which* repository. Projects with no `root_path`
/// generate nothing and are skipped silently rather than reported as fine.
fn mirror_drift(store: &Store) -> Result<Vec<Check>> {
    let mut out = Vec::new();
    let projects = store
        .list(
            &EntityQuery::default()
                .of_type(EntityType::Project)
                .limited(200),
        )
        .context("list the projects")?;

    for entity in &projects.items {
        let Entity::Project(project) = entity else {
            continue;
        };
        let Some(root) = project.root_path.as_deref() else {
            continue;
        };
        let expanded = expand_tilde(root);
        if !expanded.exists() {
            out.push(Check::degraded(
                &format!("mirror:{}", project.slug),
                format!("{} has no checkout at {root}", project.name),
                "point root_path at the repository, or clear it if this project has none",
            ));
            continue;
        }

        match generate::all(store, &project.id, &expanded, generate::Mode::Check) {
            Ok(report) if report.is_current() => out.push(Check::ok(
                &format!("mirror:{}", project.slug),
                format!(
                    "{} matches the store ({} files)",
                    root,
                    report.unchanged.len()
                ),
            )),
            Ok(report) => out.push(Check::degraded(
                &format!("mirror:{}", project.slug),
                format!(
                    "{} file(s) in {root} differ from the store and {} are orphaned: {}",
                    report.written.len(),
                    report.orphans.len(),
                    report
                        .written
                        .iter()
                        .chain(report.orphans.iter())
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                format!("run `specline generate {}`", project.slug),
            )),
            // A generation that cannot even be attempted is worth saying, and
            // it is a problem rather than drift: the confinement check refusing
            // a stored path lands here.
            Err(e) => out.push(Check::problem(
                &format!("mirror:{}", project.slug),
                format!("could not compare {root} against the store: {e}"),
                "the message above names what is wrong with the recorded paths",
            )),
        }
    }
    Ok(out)
}

/// Whether Specline is calling out, and when it last did.
///
/// The one outbound request this product makes, reported as a check rather than
/// left in the source. The pitch is that your project's history lives on your
/// machine, and an undisclosed network request undermines that claim whether or
/// not it carries anything — so "is this thing phoning home?" should be a
/// command with an answer, not a reading of the code (KEEL-204).
///
/// It says what the request *is*, in the detail line, because the honest answer
/// is narrower and better than a reassurance: a plain GET of a release manifest,
/// with nothing from the store attached.
///
/// Never a problem, at either setting. Checking is not a fault and switching it
/// off is a choice somebody made; a doctor that scolds you for your own
/// configuration is a doctor people stop running.
fn update_check() -> Check {
    if !specline_update::auto_update_enabled() {
        return Check::ok(
            "update_check",
            "off (SPECLINE_AUTO_UPDATE=0) — Specline makes no network requests at all",
        );
    }

    let last = specline_update::install_dir()
        .ok()
        .and_then(|dir| specline_update::last_check(&dir));

    let detail = match last {
        Some(stamp) if stamp.error.is_none() => format!(
            "on — fetches the release manifest every half hour, sending nothing from the \
             store. Last checked {}",
            stamp.at
        ),
        Some(stamp) => format!(
            "on — last check at {} did not complete: {}",
            stamp.at,
            stamp.error.unwrap_or_default()
        ),
        // No stamp is the ordinary state for a daemon that has not been up
        // half an hour, and also what a daemon too old to record one looks like. Both
        // mean the same thing to a reader: nothing here can tell you the check
        // is working.
        None => "on — fetches the release manifest every half hour, sending nothing from the \
                 store. No completed check on record"
            .to_owned(),
    };

    Check::ok("update_check", detail)
}

/// When the most recent backup was taken.
fn backup_age(home: &Path) -> Check {
    let dir = home.join("backups");
    let newest = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().join("specline.sqlite").is_file())
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max();

    let Some(at) = newest else {
        return Check::degraded(
            "backup",
            format!("no backup in {}", dir.display()),
            "run `specline backup`. The store is one file and the whole recovery story starts here",
        );
    };

    let age = std::time::SystemTime::now()
        .duration_since(at)
        .unwrap_or_default();
    let days = age.as_secs() / 86_400;
    if i64::try_from(days).unwrap_or(i64::MAX) > BACKUP_STALE_DAYS {
        Check::degraded(
            "backup",
            format!("the most recent backup is {days} day(s) old"),
            "run `specline backup`",
        )
    } else {
        Check::ok("backup", format!("backed up {days} day(s) ago"))
    }
}

/// Whether the newest id was minted in the future.
///
/// A ULID carries the millisecond its minter's clock said it was, so an id
/// ahead of the wall clock means the clock went backwards after it was written.
/// Every event read that assumes ids only grow is wrong until the clock catches
/// up — the live feed goes silent, and the activity cursor skips whatever was
/// written in between.
fn clock_sanity(store: &Store) -> Result<Check> {
    // `latest_event_id`, not a `SELECT max(id)` of our own. It is the same one
    // indexed row and it already carries the reasoning for why that is enough,
    // but the point is the boundary: this crate does not write SQL. The rule is
    // not compiler-enforced — `Store::connection()` is public so that `fsck` and
    // `backup` can ask the engine questions inside `specline-core` — and a call site
    // out here duplicating a trait method is how that exception widens.
    let newest = match store.latest_event_id() {
        Ok(None) => return Ok(Check::ok("clock", "no events yet, so nothing to compare")),
        Ok(Some(id)) => id,
        // Doctor reports; it does not fail. An unreadable id is a real finding
        // and has to arrive as one, or the run stops here and the checks after
        // it never get to speak — which is the failure this whole command
        // exists to avoid.
        Err(e) => {
            return Ok(Check::problem(
                "clock",
                format!("the newest event id could not be read: {e}"),
                "something wrote an event id outside specline-core; run `specline fsck`",
            ));
        }
    };

    // Still checked separately: `EventId::parse` validates the prefix, not that
    // the body is a ULID, so `evt_nonsense` gets this far.
    let Some(minted) = specline_core::id::minted_at(newest.as_str()) else {
        return Ok(Check::problem(
            "clock",
            format!("the newest event id `{newest}` is not a ULID"),
            "something wrote an event id outside specline-core; run `specline fsck`",
        ));
    };

    let now = chrono::Utc::now();
    let ahead = (minted - now).num_seconds();
    if ahead > CLOCK_SKEW_SECONDS {
        Ok(Check::problem(
            "clock",
            format!(
                "the newest event claims to have been written {ahead}s in the future \
                 ({minted}). Ids are the ordering everything else relies on, so until the \
                 clock catches up the live feed will not advance and the activity cursor will \
                 skip whatever is written in the meantime"
            ),
            "check the system clock. A laptop waking from sleep or an NTP correction is the \
             usual cause; the store repairs itself once the wall clock passes that instant",
        ))
    } else {
        Ok(Check::ok(
            "clock",
            format!("the newest event id agrees with the wall clock (minted {minted})"),
        ))
    }
}

/// Expand a leading `~`, which `root_path` is allowed to carry.
fn expand_tilde(path: &str) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => std::path::PathBuf::from(home).join(rest),
            None => std::path::PathBuf::from(path),
        },
        None => std::path::PathBuf::from(path),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// `root_path` is the one field allowed a leading `~`, because a human
    /// types it into `specline adopt` and means their home directory.
    #[test]
    fn a_tilde_path_expands_and_anything_else_is_left_alone() {
        // Read HOME rather than setting it: `set_var` is unsafe in edition
        // 2024 and the workspace denies unsafe, and a test that mutates
        // process-wide state is a test that breaks its neighbours anyway.
        let home = std::env::var_os("HOME").expect("HOME is set in any environment this runs in");
        assert_eq!(
            expand_tilde("~/development/specline"),
            std::path::PathBuf::from(&home).join("development/specline")
        );
        assert_eq!(
            expand_tilde("/absolute/path"),
            std::path::PathBuf::from("/absolute/path")
        );
        assert_eq!(
            expand_tilde("relative/path"),
            std::path::PathBuf::from("relative/path")
        );
    }

    /// Port 1 on loopback: privileged, nothing binds it, so the probe reports
    /// `NotRunning` without waiting.
    const NO_DAEMON: &str = "http://127.0.0.1:1";

    /// Every check that made the report unhealthy, with what it said.
    ///
    /// `is_healthy()` is a bool, so asserting on it bare reports "expected true,
    /// got false" and leaves whoever reads the failure to reconstruct which of
    /// forty checks went. That cost real time on an intermittent failure here,
    /// which is exactly when the information is hardest to get back.
    fn problems(report: &Report) -> Vec<(&str, &str)> {
        report
            .checks
            .iter()
            .filter(|c| c.level == Level::Problem)
            .map(|c| (c.name.as_str(), c.detail.as_str()))
            .collect()
    }

    fn find<'a>(report: &'a Report, name: &str) -> &'a Check {
        report
            .checks
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "no check called `{name}`; there were {:?}",
                    report.checks.iter().map(|c| &c.name).collect::<Vec<_>>()
                )
            })
    }

    /// A store inside a sync client's folder is called out, with the fix.
    ///
    /// Degraded rather than a problem: the detection is a path heuristic, and a
    /// check that fails the exit code on a false positive is one nobody can put
    /// in a hook.
    #[test]
    fn a_store_in_a_synced_folder_is_reported_without_failing_the_run() {
        let _serial = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("Dropbox").join(".specline");
        std::fs::create_dir_all(&home).unwrap();
        let _ = crate::create_or_open(&home).unwrap();

        let report = examine(&home, NO_DAEMON).unwrap();
        let location = find(&report, "location");

        assert_eq!(location.level, Level::Degraded);
        assert!(location.detail.contains("Dropbox"), "{location:?}");
        assert!(
            location.remedy.contains("specline backup"),
            "the remedy should say to take a consistent snapshot first: {location:?}"
        );
        assert!(
            report.is_healthy(),
            "a location warning must not fail the exit code"
        );
    }

    /// And an ordinary home says so, which is the half that would break
    /// silently if the matcher were too eager.
    #[test]
    fn an_ordinary_home_reports_its_location_as_fine() {
        let _serial = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let _ = crate::create_or_open(dir.path()).unwrap();

        let report = examine(dir.path(), NO_DAEMON).unwrap();
        assert_eq!(find(&report, "location").level, Level::Ok);
    }

    /// A store the binary is not allowed to open still gets a report.
    ///
    /// This is the case that would otherwise be the worst experience in the
    /// tool: `Store::open` refuses a store with migrations pending, and doctor
    /// is the first thing anyone runs when a command starts refusing. Failing
    /// with "could not open the store" would be true and useless.
    #[test]
    fn migrations_pending_are_a_problem_and_not_a_crash() {
        let _serial = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = specline_core::store_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::File::create(&path).unwrap();

        let report = examine(dir.path(), NO_DAEMON).expect("doctor must still produce a report");

        let schema = find(&report, "schema");
        assert_eq!(schema.level, Level::Problem);
        assert!(
            schema.remedy.contains("specline migrate"),
            "the remedy has to name the command: {schema:?}"
        );
        assert!(!report.is_healthy());
    }

    /// Serialises every test that calls [`examine`].
    ///
    /// The id generator is process-global and monotonic, which is right in
    /// production and leaks between tests: `an_event_id_from_the_future_is_a_problem`
    /// writes a ULID an hour ahead, and opening that store primes the generator
    /// to match. It resets afterwards — but a test running *inside* that window
    /// mints future ids into its own store, and the next open of that store
    /// primes the generator again, after the reset. So the reset is not enough
    /// on its own and every one of these has to hold the lock, not just the
    /// ones that look clock-related.
    ///
    /// Three of the nine held it before, which is why KEEL-179 failed roughly
    /// one run in three and passed whenever it ran alone.
    static CLOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A store with nothing wrong with it reports nothing wrong with it.
    ///
    /// The check that matters most: a doctor that finds problems in a healthy
    /// store is one nobody runs twice.
    #[test]
    fn a_fresh_store_has_no_problems() {
        let _serial = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let _ = crate::create_or_open(dir.path()).unwrap();

        let report = examine(dir.path(), NO_DAEMON).unwrap();
        assert!(
            report.is_healthy(),
            "a fresh store should be healthy: {:#?}",
            report
                .checks
                .iter()
                .filter(|c| c.level == Level::Problem)
                .collect::<Vec<_>>()
        );
        assert_eq!(find(&report, "schema").level, Level::Ok);
        // KEEL-204. The one request Specline makes should be answerable with a
        // command rather than by reading the source, and reporting it is not
        // the same as complaining about it — a doctor that scolds you for your
        // own configuration is a doctor people stop running.
        let network = find(&report, "update_check");
        assert_eq!(network.level, Level::Ok);
        assert!(
            network.detail.contains("sending nothing from the store")
                || network.detail.contains("no network requests at all"),
            "the check should say what the request carries, not only that it happens: {}",
            network.detail
        );
        assert_eq!(find(&report, "page_integrity").level, Level::Ok);
        assert_eq!(find(&report, "fsck").level, Level::Ok);
        assert_eq!(
            find(&report, "backup").level,
            Level::Degraded,
            "a store that has never been backed up is worth saying, and is not broken"
        );
    }

    /// The silent failure this command exists for: search that has only ever
    /// returned half its results and never said so.
    #[test]
    fn documents_with_no_vectors_are_reported_without_failing() {
        let _serial = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
        use specline_core::{Actor, EntityStore, Project, Provenance, Spec};

        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::create_or_open(dir.path()).unwrap();
        let prov = Provenance::anonymous(Actor::Claude);
        let project = store
            .create(Project::new("demo", "Demo").into(), &prov)
            .unwrap()
            .entity
            .id()
            .clone();
        store
            .create_with_document(
                Spec::new(project, "A spec").into(),
                Some("Prose with no vector attached, because no embedder is set.".to_owned()),
                None,
                &prov,
            )
            .unwrap();
        drop(store);

        let report = examine(dir.path(), NO_DAEMON).unwrap();
        let check = find(&report, "embeddings");
        // Two answers, because there are two builds (KEEL-220). A missing
        // vector is something to fix where a model exists and simply a fact
        // where none was compiled in — reporting the second as degraded would
        // put a permanent warning on a store that is working as well as that
        // binary can, which is how a report stops being read.
        if cfg!(feature = "embeddings") {
            assert_eq!(check.level, Level::Degraded);
        } else {
            assert_eq!(check.level, Level::Ok);
            assert!(
                check.detail.contains("not built into this binary"),
                "and say why it is not a fault: {}",
                check.detail
            );
        }
        assert!(
            check.detail.contains("keyword"),
            "it has to say what the user is actually losing: {}",
            check.detail
        );
        assert!(
            report.is_healthy(),
            "degraded search is not a broken store; problems were {:?}",
            problems(&report)
        );
    }

    /// A clean store has to say the index is clean, or a green check means
    /// nothing.
    #[test]
    fn a_coherent_passage_index_reports_itself_coherent() {
        let _serial = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::create_or_open(dir.path()).unwrap();
        store.set_embedder(std::sync::Arc::new(specline_core::HashEmbedder::new()));
        let id = seed_spec(&mut store, "Coherent", "Prose that gets passages.\n");
        assert!(id.as_str().starts_with("spc_"));
        drop(store);

        let report = examine(dir.path(), NO_DAEMON).unwrap();
        assert_eq!(find(&report, "passage_index").level, Level::Ok);
        assert!(report.is_healthy(), "problems were {:?}", problems(&report));
    }

    /// The failure case, which is the whole point: an edit that the passages
    /// did not follow has to be reported, not absorbed.
    #[test]
    fn a_passage_left_behind_by_an_edit_is_reported() {
        let _serial = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::create_or_open(dir.path()).unwrap();
        store.set_embedder(std::sync::Arc::new(specline_core::HashEmbedder::new()));
        let id = seed_spec(&mut store, "Edited", "The original prose.\n");

        // Behind the API, because `write_revision` would supersede the old
        // revision and the trigger would take its passages. This is the state a
        // crash between two writes leaves.
        store
            .connection()
            .execute(
                "UPDATE documents SET body = 'Something else entirely.', \
                 body_hash = 'a-hash-no-passage-was-built-from' WHERE entity_id = ?1",
                [id.as_str()],
            )
            .unwrap();
        drop(store);

        let report = examine(dir.path(), NO_DAEMON).unwrap();
        let check = find(&report, "passage_index");
        assert_eq!(check.level, Level::Problem, "{}", check.detail);
        assert!(
            check.detail.contains("superseded, archived or edited"),
            "the detail has to say what went wrong: {}",
            check.detail
        );
        assert!(!check.remedy.is_empty(), "a problem needs a remedy");
    }

    /// Create a spec with prose through the real write path.
    fn seed_spec(store: &mut Store, title: &str, body: &str) -> specline_core::EntityId {
        use specline_core::{Actor, EntityStore, EntityType, Project, Provenance, Spec};
        let prov = Provenance::anonymous(Actor::Claude);
        let project = store
            .create(Project::new("demo", "Demo").into(), &prov)
            .unwrap()
            .entity
            .id()
            .clone();
        let id = store
            .create(Spec::new(project.clone(), title).into(), &prov)
            .unwrap()
            .entity
            .id()
            .clone();
        store
            .write_revision(
                specline_core::Document::first(
                    EntityType::Spec,
                    id.clone(),
                    Some(project),
                    title,
                    body,
                    Actor::Claude,
                    chrono::Utc::now(),
                )
                .unwrap(),
            )
            .unwrap();
        id
    }

    /// An archived document must not hold the check red for ever.
    ///
    /// Archiving clears the vector and nothing will ever give it another, so
    /// counting archived rows as "missing an embedding" produces a check that
    /// cannot go green no matter what anyone runs. `specline reembed --missing`
    /// would report nothing to do while `doctor` went on asking for it.
    #[test]
    fn an_archived_document_is_not_counted_as_missing_a_vector() {
        let _serial = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
        use specline_core::{Actor, EntityStore, Project, Provenance, Spec};

        let dir = tempfile::tempdir().unwrap();
        let mut store = crate::create_or_open(dir.path()).unwrap();
        let prov = Provenance::anonymous(Actor::Claude);
        let project = store
            .create(Project::new("demo", "Demo").into(), &prov)
            .unwrap()
            .entity
            .id()
            .clone();
        let spec = store
            .create_with_document(
                Spec::new(project, "A spec someone put away").into(),
                Some("Prose nobody should be offered any more.".to_owned()),
                None,
                &prov,
            )
            .unwrap();
        let id = spec.entity.id().clone();
        store
            .archive(&id, spec.entity.audit().version, &prov)
            .unwrap();
        drop(store);

        let report = examine(dir.path(), NO_DAEMON).unwrap();
        let check = find(&report, "embeddings");
        assert_eq!(
            check.level,
            Level::Ok,
            "the only document is archived, so there is nothing to embed: {}",
            check.detail
        );
    }

    /// A clock that stepped backwards makes every event read wrong until it
    /// catches up, and nothing else would ever mention it.
    #[test]
    fn an_event_id_from_the_future_is_a_problem() {
        // Held for the whole test: opening this store primes the process-wide
        // id generator an hour ahead, and anything minting an id meanwhile
        // gets a future one in its own store.
        let _serial = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let store = crate::create_or_open(dir.path()).unwrap();

        // A ULID whose timestamp is an hour ahead. Written directly, because
        // the generator takes its stamp from the clock and there is no way to
        // ask it for one from the future.
        let future = chrono::Utc::now() + chrono::Duration::hours(1);
        let ulid = ulid_at(future);
        store
            .connection()
            .execute(
                "INSERT INTO events (id, entity_id, entity_type, op, summary, actor, at) \
                 VALUES (?1, 'tsk_01ZZZZZZZZZZZZZZZZZZZZZZZZ', 'task', 'created', 'from the \
                 future', 'claude', ?2)",
                [format!("evt_{ulid}"), future.to_rfc3339()],
            )
            .unwrap();
        drop(store);

        let report = examine(dir.path(), NO_DAEMON).unwrap();
        // Put the generator back before anything else runs. `examine` opened a
        // store whose newest id is an hour ahead, which primed it forward for
        // the rest of this process.
        specline_core::id::reset_for_tests();
        let check = find(&report, "clock");
        assert_eq!(check.level, Level::Problem);
        assert!(
            check.detail.contains("future"),
            "the message must name the condition: {}",
            check.detail
        );
        assert!(!report.is_healthy(), "this one does fail the exit code");
    }

    /// Build a ULID string whose timestamp is `at`. Crockford base-32, 26
    /// characters: 10 for the millisecond, 16 of randomness we can leave zero.
    fn ulid_at(at: chrono::DateTime<chrono::Utc>) -> String {
        const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let mut ms = u64::try_from(at.timestamp_millis()).unwrap();
        let mut head = [b'0'; 10];
        for slot in head.iter_mut().rev() {
            *slot = ALPHABET[(ms & 0x1f) as usize];
            ms >>= 5;
        }
        format!("{}{}", std::str::from_utf8(&head).unwrap(), "0".repeat(16))
    }

    #[test]
    fn a_report_with_only_warnings_is_still_healthy() {
        let report = Report {
            checks: vec![
                Check::ok("a", "fine"),
                Check::degraded("b", "worse than it should be", "do something"),
            ],
        };
        assert!(
            report.is_healthy(),
            "degraded is not broken, and conflating them makes the exit code useless"
        );

        let broken = Report {
            checks: vec![Check::problem("c", "broken", "fix it")],
        };
        assert!(!broken.is_healthy());
    }
}
