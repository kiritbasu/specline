//! Whether the branch a session is on is passing CI (KEEL-409).
//!
//! On 2026-09-24 `main` had been red for eight days: one job, the advisory
//! check, failed on every push, every other job was green, and work was
//! pushed straight to `main` by sessions that had run the checks locally and
//! reasonably called that done. CI said so every time, to nobody. This makes
//! a failing branch something a session is told rather than something it has
//! to remember to go and look for.
//!
//! It asks `gh`, which is what a person would ask, and it never fails: no
//! `gh`, no GitHub remote, no network, no runs for the branch — each is a
//! reason it could not tell, which the doctor reports and session start keeps
//! quiet about. Only a run that finished and failed is worth a line in a
//! session's context.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One finished workflow run, as far as a report needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub workflow: String,
    pub conclusion: String,
    pub title: String,
    pub sha: String,
    pub url: String,
}

impl Run {
    /// Whether this run's result means the branch is broken.
    ///
    /// `cancelled` and `skipped` are not failures: a run superseded by the next
    /// push, or a job that chose not to run, says nothing about the code.
    ///
    /// `action_required` is not one either: it is a first-time contributor's
    /// run waiting for a maintainer to approve it, which says nothing about
    /// the branch.
    pub fn failed(&self) -> bool {
        matches!(
            self.conclusion.as_str(),
            "failure" | "timed_out" | "startup_failure"
        )
    }
}

/// What could be learned about the branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiState {
    /// The newest finished run of every workflow on the branch, none failed.
    Passing { branch: String, runs: Vec<Run> },
    /// At least one workflow's newest finished run failed.
    Failing { branch: String, failed: Vec<Run> },
    /// Could not tell, and why.
    Unknown(String),
}

/// The newest *completed* run of each workflow, from
/// `gh run list --json status,conclusion,displayTitle,headSha,url,workflowName`.
///
/// Newest first is how `gh` lists them. A run still in progress is skipped
/// rather than counted, so a push that is being checked right now does not
/// hide the result of the one before it.
pub fn latest_completed_per_workflow(json: &str) -> Vec<Run> {
    let Ok(Value::Array(runs)) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for run in runs {
        let text = |key: &str| {
            run.get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        if text("status") != "completed" {
            continue;
        }
        let workflow = text("workflowName");
        if seen.contains(&workflow) {
            continue;
        }
        seen.push(workflow.clone());
        out.push(Run {
            workflow: bounded(&workflow, 60),
            conclusion: text("conclusion"),
            title: bounded(&text("displayTitle"), 120),
            sha: text("headSha")
                .chars()
                .take(7)
                .filter(char::is_ascii_hexdigit)
                .collect(),
            url: github_url(&text("url")),
        });
    }
    out
}

/// One line of someone else's text, cut to `limit` characters, with control
/// characters removed.
///
/// Everything here ends up in a session's context under Specline's name, and
/// a commit subject or workflow name is text anybody with push access wrote.
/// Bounded rather than trusted: long enough to recognise, too short to carry
/// a paragraph.
pub fn bounded(text: &str, limit: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let clean: String = line.chars().filter(|c| !c.is_control()).collect();
    if clean.chars().count() > limit {
        format!("{}…", clean.chars().take(limit).collect::<String>())
    } else {
        clean
    }
}

/// The run's link, only if it is a plain link into GitHub.
fn github_url(url: &str) -> String {
    let url = url.trim();
    if url.starts_with("https://github.com/")
        && url.len() <= 200
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        url.to_owned()
    } else {
        String::new()
    }
}

/// Judge the runs for `branch`.
pub fn judge(branch: &str, runs: Vec<Run>) -> CiState {
    if runs.is_empty() {
        return CiState::Unknown(format!("no finished CI runs for `{branch}`"));
    }
    let failed: Vec<Run> = runs.iter().filter(|r| r.failed()).cloned().collect();
    if failed.is_empty() {
        CiState::Passing {
            branch: branch.to_owned(),
            runs,
        }
    } else {
        CiState::Failing {
            branch: branch.to_owned(),
            failed,
        }
    }
}

/// The line a session is told, or `None` when there is nothing wrong to say.
pub fn session_line(state: &CiState) -> Option<String> {
    let CiState::Failing { branch, failed } = state else {
        return None;
    };
    let which: Vec<String> = failed
        .iter()
        .map(|r| {
            let link = if r.url.is_empty() {
                String::new()
            } else {
                format!(" — {}", r.url)
            };
            format!("{} on {} (\"{}\"){link}", r.workflow, r.sha, r.title)
        })
        .collect();
    Some(format!(
        "CI is failing on `{branch}`: the latest finished run of {}. Mention it to the user, \
         and do not treat work pushed on top of it as checked until it is green again.",
        which.join("; ")
    ))
}

/// Run a command in `dir`, returning stdout if it succeeded within `limit`.
fn output_within(program: &str, args: &[&str], dir: &Path, limit: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Ask `gh` about the branch checked out in `dir`, within `limit` in all.
pub fn check(dir: &Path, limit: Duration) -> CiState {
    let started = Instant::now();
    let remaining = || limit.saturating_sub(started.elapsed());

    let Some(branch) = output_within(
        "git",
        &["rev-parse", "--abbrev-ref", "HEAD"],
        dir,
        remaining(),
    )
    .map(|b| b.trim().to_owned())
    .filter(|b| !b.is_empty() && b != "HEAD") else {
        return CiState::Unknown("not on a git branch".into());
    };
    let Some(json) = output_within(
        "gh",
        &[
            "run",
            "list",
            "--branch",
            &branch,
            // Pushes only. A pull request from a fork's own `main` also has head
            // branch `main`, so without this a stranger's PR — its title, and a
            // run waiting on approval — would be reported to every session as
            // this repository's `main` failing.
            "--event",
            "push",
            "--limit",
            "20",
            "--json",
            "status,conclusion,displayTitle,headSha,url,workflowName",
        ],
        dir,
        remaining(),
    ) else {
        return CiState::Unknown(
            "`gh run list` did not answer (not installed, not signed in, no GitHub remote, \
             or no network)"
                .into(),
        );
    };
    judge(&branch, latest_completed_per_workflow(&json))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const LISTING: &str = r#"[
        {"status":"in_progress","conclusion":"","displayTitle":"newest, still running","headSha":"aaaaaaaaaa","url":"https://github.com/o/r/actions/runs/0","workflowName":"CI"},
        {"status":"completed","conclusion":"failure","displayTitle":"fix: x","headSha":"bbbbbbbbbb","url":"https://github.com/o/r/actions/runs/1","workflowName":"CI"},
        {"status":"completed","conclusion":"success","displayTitle":"older","headSha":"cccccccccc","url":"https://github.com/o/r/actions/runs/2","workflowName":"CI"},
        {"status":"completed","conclusion":"success","displayTitle":"docs","headSha":"dddddddddd","url":"https://github.com/o/r/actions/runs/3","workflowName":"Pages"}
    ]"#;

    #[test]
    fn the_newest_finished_run_of_each_workflow_is_what_counts() {
        let runs = latest_completed_per_workflow(LISTING);
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert_eq!(runs[0].workflow, "CI");
        assert_eq!(
            runs[0].conclusion, "failure",
            "in-progress skipped, older ignored"
        );
        assert_eq!(runs[0].sha, "bbbbbbb");
        assert_eq!(runs[1].workflow, "Pages");
    }

    #[test]
    fn one_failing_workflow_makes_the_branch_failing() {
        let state = judge("main", latest_completed_per_workflow(LISTING));
        let CiState::Failing { branch, failed } = &state else {
            panic!("{state:?}");
        };
        assert_eq!(branch, "main");
        assert_eq!(failed.len(), 1);
        let line = session_line(&state).unwrap();
        assert!(line.contains("CI is failing on `main`"), "{line}");
        assert!(
            line.contains("bbbbbbb") && line.contains("actions/runs/1"),
            "{line}"
        );
    }

    #[test]
    fn a_cancelled_or_skipped_run_is_not_a_failure() {
        for conclusion in ["cancelled", "skipped", "success", "neutral"] {
            let run = Run {
                workflow: "CI".into(),
                conclusion: conclusion.into(),
                title: String::new(),
                sha: String::new(),
                url: String::new(),
            };
            assert!(!run.failed(), "{conclusion}");
        }
    }

    #[test]
    fn passing_and_unknown_say_nothing_to_a_session() {
        let passing = judge(
            "main",
            latest_completed_per_workflow(
                r#"[{"status":"completed","conclusion":"success","workflowName":"CI"}]"#,
            ),
        );
        assert!(matches!(passing, CiState::Passing { .. }));
        assert!(session_line(&passing).is_none());
        assert!(session_line(&judge("main", Vec::new())).is_none());
        assert!(session_line(&CiState::Unknown("no gh".into())).is_none());
    }

    #[test]
    fn output_that_is_not_a_listing_is_no_runs() {
        assert!(latest_completed_per_workflow("").is_empty());
        assert!(latest_completed_per_workflow("{\"message\":\"Not Found\"}").is_empty());
    }

    #[test]
    fn a_directory_that_does_not_exist_is_unknown() {
        assert!(matches!(
            check(
                Path::new("/nonexistent/specline-ci-test"),
                Duration::from_secs(2)
            ),
            CiState::Unknown(_)
        ));
    }

    #[test]
    fn a_run_waiting_for_approval_is_not_a_failure() {
        let runs = latest_completed_per_workflow(
            r#"[{"status":"completed","conclusion":"action_required","workflowName":"CI"}]"#,
        );
        assert!(matches!(judge("main", runs), CiState::Passing { .. }));
    }

    /// Text in a run is written by whoever pushed it, and goes into context.
    #[test]
    fn titles_are_bounded_and_links_must_be_github() {
        let long = "x".repeat(500);
        let listing = format!(
            r#"[{{"status":"completed","conclusion":"failure","displayTitle":"{long}\nsecond line","headSha":"zz;rm -rf","url":"https://evil.example/x","workflowName":"CI"}}]"#
        );
        let run = &latest_completed_per_workflow(&listing)[0];
        assert_eq!(run.title.chars().count(), 121, "120 and an ellipsis");
        assert!(!run.title.contains("second line"));
        assert_eq!(run.sha, "", "only hex survives");
        assert_eq!(run.url, "", "only a github.com link survives");
    }
}
