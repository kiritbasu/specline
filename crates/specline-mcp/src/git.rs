//! Reading a project's commits, for the digest's drift section.
//!
//! This is the one place the reconciliation touches the machine. It runs
//! `git log` against the project's recorded `root_path` and hands the output
//! to [`specline_core::drift::parse_log`]; the matching happens in
//! `specline-core`, which never runs a process. Same split as
//! [`crate::image_roots`]: the pure half is testable without a repository,
//! and the impure half is small enough to read in one sitting.
//!
//! # Every failure is a reason, not a silence
//!
//! A project with no `root_path` has nothing to measure and says nothing. A
//! project *with* one whose history cannot be read — git not installed, the
//! path gone, not a repository, a non-zero exit, a hang — is reported as
//! [`Read::Unreadable`] with the reason, and the digest prints it. The
//! alternative, an empty list, would make a broken git indistinguishable from
//! a quiet week, and that is the failure this whole feature exists to expose.
//!
//! # Where it runs
//!
//! Not under the store lock. The daemon serves every request through one
//! mutex, and a `git log` on a slow mount would hold the desktop app and
//! every other session behind it. So a caller with a lock does this in two
//! halves: [`Plan::for_context`] under the lock, which is two row reads, and
//! [`Plan::read`] after releasing it, which is the process. [`commits_since`]
//! is both at once, for callers with no lock to be careful with.
//!
//! # What it reads
//!
//! The current branch, `HEAD`, and only that. A checkout parked on a feature
//! branch measures that branch's week; `main` on the same machine is not
//! consulted. That is the honest reading of "what landed here" for a
//! one-developer repository, and reading `--all` would count the same work
//! once per branch it was rebased across.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::Value;
use specline_core::drift::{self, Commit, LOG_FORMAT};
use specline_core::{Entity, EntityId, EntityStore, Store};

/// The most commits one read returns. Newest first, so past the cap it is
/// the oldest of the window that go unread. The total is read separately and
/// carried, so a capped week says it was capped rather than looking short.
///
/// Five hundred is an order of magnitude above any week this repository has
/// had; the cap exists because `since` is caller-supplied and `2020-01-01`
/// on a real repository is every commit it has.
pub const MAX_COMMITS: usize = 500;

/// How long git may take before the read is reported as unreadable rather
/// than waited for. A local log over a week is milliseconds; anything that
/// approaches this is a mount that has gone away.
const GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// What was read, owned. [`drift::Source`] borrows, so the caller keeps this
/// alive for the length of the digest build and lends it a view.
#[derive(Debug)]
pub enum Read {
    /// No checkout recorded.
    NoCheckout,
    /// A checkout, and this is why it could not be read.
    Unreadable(String),
    /// The commits since the window started.
    Commits {
        /// The start of the window.
        since: DateTime<Utc>,
        /// What `git log` returned, parsed — at most [`MAX_COMMITS`].
        commits: Vec<Commit>,
        /// How many the window held.
        total: usize,
    },
}

impl Read {
    /// The borrowed view the digest takes.
    pub fn source(&self) -> drift::Source<'_> {
        match self {
            Read::NoCheckout => drift::Source::NoCheckout,
            Read::Unreadable(reason) => drift::Source::Unreadable(reason),
            Read::Commits {
                since,
                commits,
                total,
            } => drift::Source::Commits {
                since: *since,
                commits,
                total: *total,
            },
        }
    }
}

/// The two things a read needs from the store, resolved under the lock so
/// the process itself can run outside it.
#[derive(Debug, Clone)]
pub struct Plan {
    root: Option<PathBuf>,
    since: DateTime<Utc>,
}

impl Plan {
    /// Resolve what a `specline_context` call would read, from its arguments.
    ///
    /// Mirrors the tool's own project resolution — `project` if given, else
    /// the project whose `root_path` contains `cwd` — but never fails: a bad
    /// argument here becomes "no checkout", and the tool itself raises the
    /// real error a moment later. Likewise a `since` that does not parse
    /// falls back to the default window; the tool will refuse it properly.
    pub fn for_context(store: &Store, args: &Value) -> Plan {
        let project = match args.get("project").and_then(Value::as_str) {
            Some(p) => crate::resolve_project(store, p).ok(),
            None => args
                .get("cwd")
                .and_then(Value::as_str)
                .and_then(|d| crate::dispatch::project_for_directory(store, d)),
        };
        let since = args
            .get("since")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        Plan {
            root: project.and_then(|id| project_root(store, &id)),
            since: drift::window(since, specline_core::now()),
        }
    }

    /// A plan for one project, for callers that already know which.
    pub fn for_project(store: &Store, project: &EntityId, since: Option<DateTime<Utc>>) -> Plan {
        Plan {
            root: project_root(store, project),
            since: drift::window(since, specline_core::now()),
        }
    }

    /// Run git. Safe to call with no lock held.
    pub fn read(&self) -> Read {
        commits_since(self.root.as_deref(), self.since)
    }
}

/// The project's recorded checkout, `~` expanded, if it has one.
///
/// `root_path` is allowed a leading `~` (`safe_path::validate_root_path`
/// says so and stores it as written), and `Path::is_dir` on the literal
/// string is false forever — so without this a project adopted as
/// `~/development/x` would read "not a directory" on every digest.
/// `HOME` is read here, in the crate that may.
pub fn project_root(store: &Store, project: &EntityId) -> Option<PathBuf> {
    store
        .get(project)
        .ok()
        .flatten()
        .and_then(|entity| match entity {
            Entity::Project(project) => project.root_path,
            _ => None,
        })
        .map(|p| expand_tilde(&p))
}

/// Expand a leading `~/`, leaving anything else as written.
fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(rest),
            None => PathBuf::from(path),
        },
        None => PathBuf::from(path),
    }
}

/// Read the commits in `root` since `since`.
///
/// `--no-merges` because a merge commit names the branch, not the work, and
/// counting "Merge pull request #12" as an unrowed commit would put one line
/// of noise into the drift list per pull request. A squash merge is an
/// ordinary commit and is counted.
///
/// `--since` filters on committer date and [`LOG_FORMAT`] prints `%cI`, so the
/// window and the timestamps on the rows agree.
pub fn commits_since(root: Option<&Path>, since: DateTime<Utc>) -> Read {
    let Some(root) = root else {
        return Read::NoCheckout;
    };
    if !root.is_dir() {
        return Read::Unreadable(format!("{} is not a directory", root.display()));
    }
    let since_arg = format!("--since={}", since.to_rfc3339());

    // The count first, so a capped read still knows what it was cut from.
    let total = match git(
        root,
        &["rev-list", "--count", "--no-merges", &since_arg, "HEAD"],
    ) {
        Ok(out) => match out.trim().parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                return Read::Unreadable(format!(
                    "git rev-list --count printed {:?}, not a number",
                    out.trim()
                ));
            }
        },
        Err(GitError::Unborn) => {
            // A repository with no commits yet: measured, and empty. This is
            // the state of every freshly adopted project, and it is not a
            // failure.
            return Read::Commits {
                since,
                commits: Vec::new(),
                total: 0,
            };
        }
        Err(e) => return Read::Unreadable(e.to_string()),
    };

    let max = format!("--max-count={MAX_COMMITS}");
    let format = format!("--format={LOG_FORMAT}");
    let text = match git(
        root,
        &[
            "log",
            "-z",
            "--no-merges",
            &max,
            &since_arg,
            &format,
            "HEAD",
        ],
    ) {
        Ok(text) => text,
        Err(GitError::Unborn) => {
            return Read::Commits {
                since,
                commits: Vec::new(),
                total: 0,
            };
        }
        Err(e) => return Read::Unreadable(e.to_string()),
    };

    match drift::parse_log(&text) {
        Ok(commits) => Read::Commits {
            since,
            commits,
            total,
        },
        Err(e) => {
            tracing::debug!(root = %root.display(), error = %e, "git log output did not parse");
            Read::Unreadable(format!("git log output did not parse: {e}"))
        }
    }
}

/// Why a git invocation did not return output.
#[derive(Debug)]
enum GitError {
    /// `HEAD` names no commit yet.
    Unborn,
    /// Everything else, already worded for the digest.
    Failed(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::Unborn => f.write_str("the repository has no commits yet"),
            GitError::Failed(reason) => f.write_str(reason),
        }
    }
}

/// Run one git command in `root`, with the environment pinned and a deadline.
///
/// - `LC_ALL=C`, so the `fatal:` line that reaches the digest is in one
///   language whatever the machine's locale, and so the unborn-HEAD check
///   below can match it.
/// - `GIT_DIR` and `GIT_WORK_TREE` removed, so an inherited environment
///   cannot point `-C` somewhere else.
/// - `log.showSignature=false`, so a repository whose config turns it on does
///   not make every digest spawn gpg.
/// - The child is killed at [`GIT_TIMEOUT`] rather than waited for: the
///   caller is a request handler, and a mount that has gone away should cost
///   one request five seconds, not every request forever.
fn git(root: &Path, args: &[&str]) -> Result<String, GitError> {
    let mut child = match Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "log.showSignature=false"])
        .args(args)
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            // Not on PATH, most likely.
            tracing::debug!(root = %root.display(), error = %e, "could not run git");
            return Err(GitError::Failed(format!("could not run git: {e}")));
        }
    };

    // Drain the pipes on their own threads so a large log cannot deadlock
    // against a full pipe while we poll for exit.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_thread = std::thread::spawn(move || read_all(stdout));
    let err_thread = std::thread::spawn(move || read_all(stderr));

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < GIT_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                // Best effort; a child that will not die is not this
                // function's problem to solve, and the reason is the same.
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitError::Failed(format!(
                    "git {} took longer than {}s in {}",
                    args.first().unwrap_or(&""),
                    GIT_TIMEOUT.as_secs(),
                    root.display()
                )));
            }
            Err(e) => return Err(GitError::Failed(format!("could not wait for git: {e}"))),
        }
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();

    if status.success() {
        return Ok(String::from_utf8_lossy(&stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&stderr);
    let first = stderr.trim().lines().next().unwrap_or("").trim();
    // Both spellings git has used for an unborn branch, under LC_ALL=C.
    if first.contains("does not have any commits yet")
        || first.contains("bad revision 'HEAD'")
        || first.contains("unknown revision")
    {
        return Err(GitError::Unborn);
    }
    Err(GitError::Failed(format!(
        "git exited {}: {first}",
        status
            .code()
            .map_or("by signal".to_owned(), |c| c.to_string())
    )))
}

/// Everything a pipe has, or nothing if there was no pipe.
fn read_all(pipe: Option<impl std::io::Read>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        // A read error here means the child went away mid-write; what was
        // read is still the best answer, and the exit status says the rest.
        let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
    }
    buf
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

    fn have_git() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// A repository built here, with identity set per invocation so nothing
    /// depends on the machine's git config. A test that read this checkout's
    /// history would pass on one machine and fail on the next.
    fn repo_with(messages: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args([
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        for (i, message) in messages.iter().enumerate() {
            std::fs::write(dir.path().join(format!("f{i}")), message).unwrap();
            git(&["add", "-A"]);
            git(&["commit", "-q", "-m", message]);
        }
        dir
    }

    fn a_day_ago() -> DateTime<Utc> {
        Utc::now() - ChronoDuration::days(1)
    }

    #[test]
    fn no_root_is_no_checkout() {
        assert!(matches!(commits_since(None, Utc::now()), Read::NoCheckout));
    }

    #[test]
    fn a_missing_directory_is_unreadable_with_the_path_in_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("nope");
        match commits_since(Some(&gone), Utc::now()) {
            Read::Unreadable(reason) => assert!(reason.contains("nope"), "{reason}"),
            other => panic!("expected unreadable, got {other:?}"),
        }
    }

    #[test]
    fn a_directory_that_is_not_a_repository_is_unreadable_and_says_why() {
        if !have_git() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        match commits_since(Some(dir.path()), a_day_ago()) {
            Read::Unreadable(reason) => {
                // Under LC_ALL=C, so the wording is git's English whatever
                // the machine's locale.
                assert!(reason.starts_with("git exited 128"), "{reason}");
                assert!(reason.contains("not a git repository"), "{reason}");
            }
            other => panic!("expected unreadable, got {other:?}"),
        }
    }

    #[test]
    fn a_repository_with_no_commits_is_measured_and_empty() {
        // Every freshly adopted project starts here. `git log` exits 128 on
        // an unborn HEAD, and reporting that as unreadable would tell a new
        // user their checkout is broken on the day they set it up.
        if !have_git() {
            return;
        }
        let repo = repo_with(&[]);
        match commits_since(Some(repo.path()), a_day_ago()) {
            Read::Commits { commits, total, .. } => {
                assert!(commits.is_empty());
                assert_eq!(total, 0);
            }
            other => panic!("expected commits, got {other:?}"),
        }
    }

    #[test]
    fn a_repository_yields_its_commits_newest_first_with_whole_messages() {
        if !have_git() {
            return;
        }
        let repo = repo_with(&["feat: first\n\nCloses HARB-1.", "fix: second"]);
        match commits_since(Some(repo.path()), a_day_ago()) {
            Read::Commits { commits, total, .. } => {
                assert_eq!(commits.len(), 2);
                assert_eq!(total, 2);
                assert_eq!(commits[0].subject, "fix: second");
                assert_eq!(commits[1].subject, "feat: first");
                assert!(
                    commits[1].message.contains("Closes HARB-1."),
                    "the body is kept"
                );
                assert_eq!(commits[0].sha.len(), 40);
            }
            other => panic!("expected commits, got {other:?}"),
        }
    }

    #[test]
    fn a_window_that_starts_in_the_future_is_measured_and_empty() {
        // Measured-and-empty is a different answer from unreadable, and the
        // one a quiet week gives.
        if !have_git() {
            return;
        }
        let repo = repo_with(&["chore: only"]);
        match commits_since(Some(repo.path()), Utc::now() + ChronoDuration::days(1)) {
            Read::Commits { commits, total, .. } => {
                assert!(commits.is_empty());
                assert_eq!(total, 0);
            }
            other => panic!("expected commits, got {other:?}"),
        }
    }

    #[test]
    fn a_tilde_root_expands_against_home_and_anything_else_is_left_alone() {
        // Pure: no store, no HOME dependence beyond the variable being set,
        // which the assertion reads back rather than assumes.
        match std::env::var_os("HOME") {
            Some(home) => assert_eq!(
                expand_tilde("~/development/x"),
                PathBuf::from(home).join("development/x")
            ),
            None => assert_eq!(
                expand_tilde("~/development/x"),
                PathBuf::from("~/development/x")
            ),
        }
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("~user/x"), PathBuf::from("~user/x"));
    }
}
