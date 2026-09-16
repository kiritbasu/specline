//! The join between the repository's commits and the task rows.
//!
//! GitHub knows what changed; Specline knows what was decided and claimed
//! before the diff existed. Nothing joined the two, so the one number the
//! contract cares about most — how much work lands with no row behind it —
//! was measured by hand (2026-08-15: four of six commits against an idle
//! board) and never since. This module makes it a query.
//!
//! **No I/O.** Commits arrive as a slice; the caller ran `git log` and parsed
//! it, or built the slice by hand in a test. `specline-core` never runs a
//! process, and a reconciliation that shelled out would be the first crack in
//! the boundary that keeps the CLI, the daemon and the tests cheap.
//!
//! # What it answers
//!
//! Three lists over one window:
//!
//! - commits that name a task, by key in the message or by being cited as
//!   `commit:` evidence on a row;
//! - commits that name nothing — the drift;
//! - tasks closed `done` in the window with no commit behind them.
//!
//! And a fourth that would otherwise be dropped on the floor: commits naming
//! a key that has no row, which is usually a typo and occasionally a task
//! somebody forgot to create.
//!
//! # Not measured is not zero
//!
//! A [`Drift`] is built only when there were commits to read. When the project
//! has no checkout, or git could not be run, the caller says so *instead* of
//! passing an empty slice — see [`crate::digest::Digest::drift`]. An empty
//! week and a broken git must never look the same, because this is the
//! project's own bug class: an empty result that reads as calm.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{
    CloseReason, Entity, EntityId, EntityQuery, EntityStore, EntityType, Error, Result, Task,
};

/// One commit, as the caller read it from the repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Commit {
    /// The full 40-character sha.
    pub sha: String,
    /// When it was committed. Committer date rather than author date, because
    /// that is what `git log --since` filters on, and a window that disagrees
    /// with its own timestamps is worse than none.
    pub committed_at: DateTime<Utc>,
    /// The first line of the message.
    pub subject: String,
    /// The whole message, subject included. Keys are matched over all of it,
    /// because this repository puts them in bodies as often as in subjects.
    pub message: String,
}

/// The format [`parse_log`] expects, for the caller building the `git log`
/// invocation — to be run with `-z`, so the record terminator is NUL as well.
///
/// NUL between fields and between records, because it is the one byte a commit
/// message cannot contain: git refuses to store one. Any printable separator
/// could be forged by a message that happened to include it, and a forged
/// separator would tear the record and turn the whole window unreadable.
pub const LOG_FORMAT: &str = "%H%x00%cI%x00%B";

/// Parse the output of `git log -z --format=`[`LOG_FORMAT`].
///
/// A record that does not parse is an error rather than a skipped commit: a
/// silently shorter list is the failure mode this module exists to remove.
///
/// Control bytes other than newline and tab are stripped from the message.
/// The text goes straight into a digest a model reads and a card a person
/// reads, and an escape sequence in a commit message should not be able to
/// redraw either.
pub fn parse_log(text: &str) -> Result<Vec<Commit>> {
    // `-z` terminates every record with NUL, so a well-formed stream is a
    // whole number of triples once the last terminator is removed. Anything
    // else is a record that was cut — a trailing `sha NUL date NUL` with no
    // message field would otherwise read as a commit with an empty message.
    let text = text.strip_suffix('\0').unwrap_or(text);
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let fields: Vec<&str> = text.split('\0').collect();
    if !fields.len().is_multiple_of(3) {
        let last = fields
            .chunks(3)
            .last()
            .and_then(|c| c.first())
            .map(|s| s.trim())
            .unwrap_or("");
        return Err(Error::invalid(
            EntityType::Project,
            "commits",
            format!("git log record for {last} has fewer than three fields"),
            format!("output of `git log -z --format={LOG_FORMAT:?}`"),
        ));
    }
    let mut commits = Vec::with_capacity(fields.len() / 3);
    for record in fields.chunks(3) {
        // Always three, by the check above; the pattern is the proof.
        let &[sha, date, message] = record else {
            continue;
        };
        let sha = sha.trim();
        let committed_at = DateTime::parse_from_rfc3339(date.trim())
            .map_err(|e| {
                Error::invalid(
                    EntityType::Project,
                    "commits",
                    format!("commit {sha}: bad committer date {date:?}: {e}"),
                    "an RFC 3339 timestamp, which is what `%cI` prints",
                )
            })?
            .with_timezone(&Utc);
        let message: String = message
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect();
        let message = message.trim_end().to_owned();
        let subject = message.lines().next().unwrap_or("").trim().to_owned();
        commits.push(Commit {
            sha: sha.to_owned(),
            committed_at,
            subject,
            message,
        });
    }
    Ok(commits)
}

/// What the caller found when it went to read the repository.
///
/// Three states rather than an `Option<&[Commit]>`, because the two ways of
/// having no commits mean opposite things: a project with no checkout has
/// nothing to measure, and a checkout git could not read has something to
/// measure that was not measured. The digest says nothing for the first and
/// says *why* for the second, and a type with only `None` could not tell them
/// apart.
#[derive(Debug, Clone, Copy)]
pub enum Source<'a> {
    /// The project records no `root_path`. Nothing to say.
    NoCheckout,
    /// There is a checkout and reading it failed — git missing, not a
    /// repository, non-zero exit. Reported, never hidden.
    Unreadable(&'a str),
    /// The commits since the start of the window.
    Commits {
        /// The start of the window they were read over.
        since: DateTime<Utc>,
        /// What was read — the newest `total` at most, capped by the caller.
        commits: &'a [Commit],
        /// How many there were in the window, whether or not all were read.
        /// Hard constraint 4: a cut list says it was cut, with the total.
        total: usize,
    },
}

/// The default window, when the caller has not asked for one.
///
/// Seven days, because that is the cadence the number is for — "what landed
/// this week with no row" — and because the measurement the task was cut
/// against was a week of this repository's own commits.
pub const DEFAULT_WINDOW_DAYS: i64 = 7;

/// The start of the window: what the caller asked for, or the default back
/// from `now`. Here rather than at the call site so the digest and whoever
/// reads the log agree on it by construction.
pub fn window(since: Option<DateTime<Utc>>, now: DateTime<Utc>) -> DateTime<Utc> {
    since.unwrap_or_else(|| now - chrono::Duration::days(DEFAULT_WINDOW_DAYS))
}

/// A commit and the task keys it named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkedCommit {
    /// Short sha, seven characters — what a person reads.
    pub sha: String,
    /// The subject line.
    pub subject: String,
    /// `KEEL-42` and so on: every row this commit reached, by key in the
    /// message or by citation as evidence.
    pub tasks: Vec<String>,
}

/// A commit that reached no row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnlinkedCommit {
    /// Short sha.
    pub sha: String,
    /// The subject line.
    pub subject: String,
    /// When.
    pub committed_at: DateTime<Utc>,
}

/// A commit naming a key that has no row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnknownKey {
    /// Short sha.
    pub sha: String,
    /// The key as written, `KEEL-999`.
    pub key: String,
}

/// A task closed `done` in the window with nothing in the repository to show
/// for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoneWithoutCommit {
    /// `KEEL-42`.
    pub reference: String,
    /// The task id, for a caller that wants to follow it.
    pub id: EntityId,
    /// The title.
    pub title: String,
    /// When it closed.
    pub closed_at: DateTime<Utc>,
}

/// The reconciliation, over one window.
///
/// Every list carries its own total, and the lists are complete — trimming
/// for display is the renderer's job, and hard constraint 4 says it reports
/// the cut when it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Drift {
    /// The start of the window.
    pub since: DateTime<Utc>,
    /// How many commits were read.
    pub commits: usize,
    /// How many commits the window held. Greater than `commits` only when the
    /// caller capped the read, in which case the newest were kept and the
    /// lists below say nothing about the rest.
    pub commits_total: usize,
    /// Commits that reached a row.
    pub linked: Vec<LinkedCommit>,
    /// Commits that reached none. The drift.
    pub unlinked: Vec<UnlinkedCommit>,
    /// Commits naming a key with no row behind it.
    pub unknown: Vec<UnknownKey>,
    /// Tasks closed `done` in the window with no commit behind them.
    pub done_without_commit: Vec<DoneWithoutCommit>,
    /// How many task rows were read, and how many there are. Unequal only if
    /// the project has more tasks than the scan cap. The store returns the
    /// newest rows first, so past the cap it is the *oldest* tasks that go
    /// unread — and a commit naming one of them is then reported as naming a
    /// task that does not exist. Incomplete is the polite word; wrong is the
    /// accurate one, which is why the renderers say so.
    pub tasks_scanned: usize,
    /// See [`Drift::tasks_scanned`].
    pub tasks_total: usize,
}

/// The most rows one reconciliation reads. Well above any project this has
/// been run on; the point is that a project past it is *told*, through
/// [`Drift::tasks_total`], rather than quietly under-counted.
const TASK_SCAN_CAP: usize = 5000;

/// The shortest `commit:` evidence that is allowed to match. Git's own
/// abbreviation floor; shorter than this and `commit:a` would claim every
/// commit whose sha starts with an `a`.
const MIN_SHA_PREFIX: usize = 7;

/// Reconcile the commits against the project's task rows.
///
/// `since` is the window the caller read the commits over. It is carried on
/// the result and used to bound the `done_without_commit` scan; the commits
/// themselves are taken as given, so a caller that read a wider window than it
/// claims is not second-guessed.
///
/// `commits_total` is how many the window held; the caller may have read
/// fewer, and the number is carried through so the renderer can say so.
///
/// Archived tasks are read along with live ones. They still own their number,
/// and a commit naming one is a commit that reached a row, not a commit
/// naming a task that does not exist. They are never listed as drift
/// themselves: an archived row is one somebody has already set aside.
///
/// One known blind spot, by design: a commit that named a task on Friday
/// and a close on Monday citing only a `test:` is fine while both sit inside
/// the window, and becomes a "done with no commit" the day Friday leaves it.
/// Widening the commit window would hide that at the cost of reading more
/// history on every digest; the number is a prompt to look, not a verdict.
pub fn reconcile(
    store: &impl EntityStore,
    project: &EntityId,
    commits: &[Commit],
    commits_total: usize,
    since: DateTime<Utc>,
) -> Result<Drift> {
    let commits_total = commits_total.max(commits.len());
    let key = match store.get(project)? {
        Some(Entity::Project(p)) => p.key,
        Some(other) => {
            return Err(Error::invalid(
                EntityType::Project,
                "project",
                format!("{project} is a {}, not a project", other.entity_type()),
                "a `prj_…` id",
            ));
        }
        None => {
            return Err(Error::NotFound {
                entity_type: EntityType::Project,
                id: project.to_string(),
            });
        }
    };

    let page = store.list(
        &EntityQuery::in_project(project.clone())
            .of_type(EntityType::Task)
            .including_archived()
            .limited(TASK_SCAN_CAP),
    )?;
    let tasks: Vec<&Task> = page
        .items
        .iter()
        .filter_map(|e| match e {
            Entity::Task(t) => Some(t),
            _ => None,
        })
        .collect();

    let mut linked = Vec::new();
    let mut unlinked = Vec::new();
    let mut unknown = Vec::new();
    // Every task some commit in the window reached, by number. A `done` row
    // in this set has a commit behind it even if its evidence never said so.
    let mut reached: Vec<i32> = Vec::new();

    for commit in commits {
        let mut numbers: Vec<i32> = Vec::new();
        let mut unknown_here: Vec<String> = Vec::new();
        for named in keys_in(&commit.message, &key) {
            match named {
                Named::Number(n) if tasks.iter().any(|t| t.number == n) => {
                    if !numbers.contains(&n) {
                        numbers.push(n);
                    }
                }
                Named::Number(n) => {
                    let written = format!("{key}-{n}");
                    if !unknown_here.contains(&written) {
                        unknown_here.push(written);
                    }
                }
                Named::Unparseable(written) => {
                    if !unknown_here.contains(&written) {
                        unknown_here.push(written);
                    }
                }
            }
        }
        for written in unknown_here {
            unknown.push(UnknownKey {
                sha: short(&commit.sha),
                key: written,
            });
        }
        for t in &tasks {
            if cites(t, &commit.sha) && !numbers.contains(&t.number) {
                numbers.push(t.number);
            }
        }
        if numbers.is_empty() {
            unlinked.push(UnlinkedCommit {
                sha: short(&commit.sha),
                subject: commit.subject.clone(),
                committed_at: commit.committed_at,
            });
        } else {
            numbers.sort_unstable();
            reached.extend(&numbers);
            linked.push(LinkedCommit {
                sha: short(&commit.sha),
                subject: commit.subject.clone(),
                tasks: numbers.iter().map(|n| format!("{key}-{n}")).collect(),
            });
        }
    }

    let mut done_without_commit: Vec<DoneWithoutCommit> = tasks
        .iter()
        .filter(|t| t.audit.archived_at.is_none())
        .filter(|t| t.close_reason == Some(CloseReason::Done))
        .filter(|t| t.closed_at.is_some_and(|at| at >= since))
        .filter(|t| !reached.contains(&t.number))
        .filter(|t| !has_repository_evidence(t))
        .filter_map(|t| {
            Some(DoneWithoutCommit {
                reference: format!("{key}-{}", t.number),
                id: t.id.clone(),
                title: t.title.clone(),
                closed_at: t.closed_at?,
            })
        })
        .collect();
    done_without_commit.sort_by_key(|t| std::cmp::Reverse(t.closed_at));

    Ok(Drift {
        since,
        commits: commits.len(),
        commits_total,
        linked,
        unlinked,
        unknown,
        done_without_commit,
        tasks_scanned: page.items.len(),
        tasks_total: page.total,
    })
}

/// What a message named: a number, or a run of digits too long to be one.
///
/// The second exists so that `KEEL-99999999999` is reported as a key with no
/// row rather than dropped — the module promises nothing is dropped, and a
/// typo with one digit too many is the case that promise is for.
#[derive(Debug, PartialEq, Eq)]
enum Named {
    Number(i32),
    Unparseable(String),
}

/// The task keys a message names for this project: `KEEL-42` with a boundary
/// on both sides, so `KEEL-42a`, `XKEEL-42` and `KEEL-` match nothing. One
/// entry per occurrence, in order; the caller de-duplicates.
/// Case-sensitive, because the key is.
///
/// Byte-indexed, and every index it slices at is a char boundary: `start`
/// and `after_key` come from a match of `key`, and the digit scan only ever
/// advances over ASCII. `bytes[start - 1]` is a byte *read*, not a slice, and
/// a UTF-8 continuation byte is not ASCII-alphanumeric — so `éKEEL-42`
/// matches, which is the right answer for a boundary check that means
/// "not part of a longer identifier". An empty key would match at every
/// position and is refused up front.
fn keys_in(message: &str, key: &str) -> Vec<Named> {
    let mut found = Vec::new();
    if key.is_empty() {
        return found;
    }
    let bytes = message.as_bytes();
    let mut from = 0;
    while let Some(at) = message[from..].find(key) {
        let start = from + at;
        let after_key = start + key.len();
        // Resume after the match, not one byte in: one byte in can be the
        // middle of a multi-byte character if the key is not ASCII, and
        // slicing there panics.
        from = after_key;
        // Left boundary: nothing alphanumeric before the key.
        if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
            continue;
        }
        if bytes.get(after_key) != Some(&b'-') {
            continue;
        }
        let digits_start = after_key + 1;
        let digits_end = digits_start
            + message[digits_start..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
        if digits_end == digits_start {
            continue;
        }
        // Right boundary: nothing alphanumeric after the digits.
        if bytes.get(digits_end).is_some_and(u8::is_ascii_alphanumeric) {
            continue;
        }
        let digits = &message[digits_start..digits_end];
        found.push(match digits.parse::<i32>() {
            Ok(n) => Named::Number(n),
            Err(_) => Named::Unparseable(format!("{key}-{digits}")),
        });
        from = digits_end;
    }
    found
}

/// Whether a task's evidence cites this commit.
///
/// Case-insensitive on the hex, because some interfaces copy a sha in upper
/// case and a citation that fails on letter case is a citation nobody can
/// see failing.
fn cites(task: &Task, sha: &str) -> bool {
    task.evidence.iter().any(|e| {
        e.strip_prefix("commit:")
            .map(str::trim)
            .is_some_and(|cited| {
                cited.len() >= MIN_SHA_PREFIX
                    && sha
                        .get(..cited.len())
                        .is_some_and(|head| head.eq_ignore_ascii_case(cited))
            })
    })
}

/// Whether a task's evidence points at the repository at all — a commit or a
/// pull request. `test:`, `doc:`, `url:` and `image:` are proof of something,
/// but not of a change landing.
fn has_repository_evidence(task: &Task) -> bool {
    task.evidence
        .iter()
        .any(|e| e.starts_with("commit:") || e.starts_with("pr:"))
}

/// The seven characters a person reads.
fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn numbers(message: &str, key: &str) -> Vec<i32> {
        keys_in(message, key)
            .into_iter()
            .map(|n| match n {
                Named::Number(n) => n,
                Named::Unparseable(s) => panic!("{s} should have parsed"),
            })
            .collect()
    }

    #[test]
    fn keys_are_matched_with_boundaries_on_both_sides() {
        assert_eq!(numbers("chore: 0.6.0 (KEEL-391)", "KEEL"), vec![391]);
        assert_eq!(
            numbers("close KEEL-380 and KEEL-381, regenerate", "KEEL"),
            vec![380, 381]
        );
        assert_eq!(
            numbers("KEEL-42\n\nBody names KEEL-7 too.", "KEEL"),
            vec![42, 7]
        );
        assert_eq!(numbers("KEEL-42", "KEEL"), vec![42], "the whole message");
        assert_eq!(numbers("KEEL-42a is not a key", "KEEL"), Vec::<i32>::new());
        assert_eq!(numbers("XKEEL-42 is not either", "KEEL"), Vec::<i32>::new());
        assert_eq!(numbers("KEEL- has no number", "KEEL"), Vec::<i32>::new());
        assert_eq!(
            numbers("keel-42 is the wrong case", "KEEL"),
            Vec::<i32>::new()
        );
        assert_eq!(
            numbers("STEEL-42 is a different project", "KEEL"),
            Vec::<i32>::new()
        );
        assert_eq!(
            numbers("KEEL-1KEEL-2", "KEEL"),
            Vec::<i32>::new(),
            "each is the other's boundary violation"
        );
    }

    #[test]
    fn an_empty_or_non_ascii_key_does_not_panic() {
        // Both reachable: `key` is settable through `specline_update` and
        // nothing validates it yet. `find("")` matches at every position
        // including the end, and resuming one byte into a multi-byte
        // character slices off a char boundary — either was a panic inside
        // the daemon's request handler.
        assert!(keys_in("x -42 anything", "").is_empty());
        assert_eq!(numbers("ÜNIT-5 and ÜNIT-6", "ÜNIT"), vec![5, 6]);
        assert_eq!(
            numbers("éKEEL-42", "KEEL"),
            vec![42],
            "a continuation byte is not a letter"
        );
        assert_eq!(numbers("KEEL-42é", "KEEL"), vec![42]);
    }

    #[test]
    fn a_number_too_long_to_be_one_is_reported_rather_than_dropped() {
        assert_eq!(
            keys_in("KEEL-99999999999 is a typo", "KEEL"),
            vec![Named::Unparseable("KEEL-99999999999".to_owned())]
        );
    }

    #[test]
    fn a_log_parses_into_one_commit_per_record() {
        let text = "aaaaaaa1\x002026-09-16T10:00:00+01:00\0feat: one\n\nBody KEEL-1\n\0\
                    bbbbbbb2\x002026-09-15T09:00:00Z\0fix: two\n\0";
        let commits = parse_log(text).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "aaaaaaa1");
        assert_eq!(commits[0].subject, "feat: one");
        assert_eq!(commits[0].message, "feat: one\n\nBody KEEL-1");
        assert_eq!(
            commits[0].committed_at.to_rfc3339(),
            "2026-09-16T09:00:00+00:00"
        );
        assert_eq!(commits[1].subject, "fix: two");
    }

    #[test]
    fn control_bytes_in_a_message_are_stripped_and_newlines_kept() {
        let text = "aaaaaaa1\x002026-09-16T10:00:00Z\0feat: \x1b[31mred\x1b[0m\n\nbody\tkept\x07\0";
        let commits = parse_log(text).unwrap();
        assert_eq!(commits[0].subject, "feat: [31mred[0m");
        assert_eq!(commits[0].message, "feat: [31mred[0m\n\nbody\tkept");
    }

    #[test]
    fn an_empty_log_is_no_commits_and_a_torn_record_is_an_error() {
        assert!(parse_log("").unwrap().is_empty());
        assert!(parse_log("\0").unwrap().is_empty());
        let err = parse_log("aaaaaaa1\x002026-09-16T10:00:00Z\0").unwrap_err();
        assert!(err.to_string().contains("fewer than three fields"), "{err}");
        let err = parse_log("aaaaaaa1\0yesterday\0subject\0").unwrap_err();
        assert!(err.to_string().contains("bad committer date"), "{err}");
    }
}
