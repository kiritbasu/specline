//! The join between commits and rows — every matching rule, and for each the
//! case where it must *not* match.
//!
//! The negative cases matter more here than the positive ones. A commit that
//! is wrongly counted as linked disappears from the drift list, and a task
//! wrongly excused from "done with no commit" disappears from the other, so
//! each over-match is one more piece of unrowed work that reads as tracked.
//! That is the failure the module exists to make visible, in reverse.
//!
//! Commits are built by hand. Nothing here runs git, reads this checkout, or
//! depends on what day it is beyond "now".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Duration, Utc};
use specline_core::{
    Actor, Close, CloseReason, Entity, EntityId, EntityStore, Project, Provenance, Store, Task,
    close,
    digest::{self, Depth, DriftSection, Surfaces},
    drift::{self, Commit, Source},
};

fn prov() -> Provenance {
    Provenance::anonymous(Actor::Claude).with_session("ses_drift")
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    project: EntityId,
    now: DateTime<Utc>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let project = store
        .create(Project::new("harbour", "Harbour").into(), &prov())
        .unwrap()
        .entity
        .id()
        .clone();
    Fixture {
        _dir: dir,
        store,
        project,
        now: Utc::now(),
    }
}

impl Fixture {
    fn key(&self) -> String {
        match self.store.get(&self.project).unwrap() {
            Some(Entity::Project(p)) => p.key,
            _ => panic!("the project exists"),
        }
    }

    /// A task, returning its number so the test can name it the way a commit
    /// message would.
    fn task(&mut self, title: &str) -> (EntityId, i32) {
        let created = self
            .store
            .create(
                Task::new(self.project.clone(), title, "A row this test needs.").into(),
                &prov(),
            )
            .unwrap()
            .entity;
        match created {
            Entity::Task(t) => (t.id, t.number),
            _ => panic!("a task"),
        }
    }

    fn close_with(&mut self, id: &EntityId, reason: CloseReason, evidence: &[&str]) {
        close(
            &mut self.store,
            id,
            &Close {
                reason,
                message: "Closed by the test.".to_owned(),
                evidence: evidence.iter().map(|e| (*e).to_owned()).collect(),
                other: None,
            },
            &prov(),
        )
        .unwrap();
    }

    fn since(&self) -> DateTime<Utc> {
        self.now - Duration::days(7)
    }

    fn reconcile(&self, commits: &[Commit]) -> drift::Drift {
        drift::reconcile(
            &self.store,
            &self.project,
            commits,
            commits.len(),
            self.since(),
        )
        .unwrap()
    }

    fn archive(&mut self, id: &EntityId) {
        let version = match self.store.get(id).unwrap() {
            Some(Entity::Task(t)) => t.audit.version,
            _ => panic!("a task"),
        };
        self.store.archive(id, version, &prov()).unwrap();
    }
}

fn commit(sha: &str, message: &str, at: DateTime<Utc>) -> Commit {
    Commit {
        sha: sha.to_owned(),
        committed_at: at,
        subject: message.lines().next().unwrap_or("").to_owned(),
        message: message.to_owned(),
    }
}

/// Forty hex characters from a short prefix, so a test can write `aaaaaaa`
/// and still be handing the module a full sha.
fn sha(prefix: &str) -> String {
    format!("{prefix:0<40}")
}

#[test]
fn a_commit_naming_a_key_in_its_body_is_linked_and_one_naming_nothing_is_not() {
    let mut f = fixture();
    let key = f.key();
    let (_, n) = f.task("Write the thing");
    let yesterday = f.now - Duration::days(1);

    let drift = f.reconcile(&[
        commit(
            &sha("aaaaaaa"),
            &format!("feat: the thing\n\nCloses {key}-{n}."),
            yesterday,
        ),
        commit(&sha("bbbbbbb"), "chore: tidy", yesterday),
    ]);

    assert_eq!(drift.commits, 2);
    assert_eq!(drift.linked.len(), 1);
    assert_eq!(drift.linked[0].sha, "aaaaaaa");
    assert_eq!(drift.linked[0].tasks, vec![format!("{key}-{n}")]);
    assert_eq!(drift.unlinked.len(), 1);
    assert_eq!(drift.unlinked[0].sha, "bbbbbbb");
    assert_eq!(drift.unlinked[0].subject, "chore: tidy");
    assert!(drift.unknown.is_empty());
}

#[test]
fn a_commit_cited_as_evidence_is_linked_even_when_its_message_names_nothing() {
    let mut f = fixture();
    let key = f.key();
    let (id, n) = f.task("Fix the leak");
    // Seven characters, which is what a person pastes.
    f.close_with(&id, CloseReason::Done, &["commit:cafe123"]);

    let drift = f.reconcile(&[commit(
        &sha("cafe123"),
        "fix: stop leaking",
        f.now - Duration::hours(2),
    )]);

    assert_eq!(drift.linked.len(), 1, "cited, so linked");
    assert_eq!(drift.linked[0].tasks, vec![format!("{key}-{n}")]);
    assert!(drift.unlinked.is_empty());
    assert!(
        drift.done_without_commit.is_empty(),
        "and the task has a commit behind it"
    );
}

#[test]
fn evidence_shorter_than_seven_characters_matches_no_commit() {
    // `commit:c` would otherwise claim every commit starting with a `c`,
    // which is a sixteenth of them.
    let mut f = fixture();
    let (id, _) = f.task("Vague evidence");
    f.close_with(&id, CloseReason::Done, &["commit:cafe12"]);

    let drift = f.reconcile(&[commit(
        &sha("cafe123"),
        "fix: something",
        f.now - Duration::hours(2),
    )]);

    assert!(drift.linked.is_empty(), "six characters is not a citation");
    assert_eq!(drift.unlinked.len(), 1);
}

#[test]
fn a_commit_naming_a_key_with_no_row_is_reported_not_dropped() {
    let f = fixture();
    let key = f.key();
    let drift = f.reconcile(&[commit(
        &sha("dddddd1"),
        &format!("fix: typo ({key}-9999)"),
        f.now - Duration::hours(1),
    )]);

    assert_eq!(drift.unknown.len(), 1);
    assert_eq!(drift.unknown[0].key, format!("{key}-9999"));
    assert_eq!(drift.unknown[0].sha, "dddddd1");
    assert_eq!(
        drift.unlinked.len(),
        1,
        "naming a row that does not exist reaches no row"
    );
}

#[test]
fn another_projects_key_is_not_this_projects_task() {
    let mut f = fixture();
    let (_, n) = f.task("Ours");
    let drift = f.reconcile(&[commit(
        &sha("eeeeee1"),
        &format!("feat: theirs (OTHER-{n})"),
        f.now - Duration::hours(1),
    )]);
    assert!(drift.linked.is_empty());
    assert!(
        drift.unknown.is_empty(),
        "a foreign key is not an unknown one of ours"
    );
    assert_eq!(drift.unlinked.len(), 1);
}

#[test]
fn one_commit_closing_two_tasks_links_both_once() {
    let mut f = fixture();
    let key = f.key();
    let (_, a) = f.task("First");
    let (_, b) = f.task("Second");
    let drift = f.reconcile(&[commit(
        &sha("fffffff"),
        &format!("chore: close {key}-{a} and {key}-{b}\n\nAlso {key}-{a} again."),
        f.now - Duration::hours(1),
    )]);
    assert_eq!(drift.linked.len(), 1);
    assert_eq!(
        drift.linked[0].tasks,
        vec![format!("{key}-{a}"), format!("{key}-{b}")]
    );
}

#[test]
fn a_task_closed_done_in_the_window_with_only_a_test_as_evidence_is_drift() {
    let mut f = fixture();
    let key = f.key();
    let (id, n) = f.task("Green but unrowed");
    f.close_with(
        &id,
        CloseReason::Done,
        &["test:cargo test -p specline-core"],
    );

    let drift = f.reconcile(&[]);

    assert_eq!(drift.commits, 0);
    assert_eq!(drift.done_without_commit.len(), 1);
    assert_eq!(drift.done_without_commit[0].reference, format!("{key}-{n}"));
    assert_eq!(drift.done_without_commit[0].id, id);
}

#[test]
fn a_task_closed_done_with_a_pull_request_as_evidence_is_not_drift() {
    let mut f = fixture();
    let (id, _) = f.task("Landed by PR");
    f.close_with(
        &id,
        CloseReason::Done,
        &["pr:https://github.com/kb/harbour/pull/7"],
    );
    let drift = f.reconcile(&[]);
    assert!(drift.done_without_commit.is_empty());
}

#[test]
fn a_task_a_commit_named_is_not_drift_even_if_its_evidence_never_said_so() {
    let mut f = fixture();
    let key = f.key();
    let (id, n) = f.task("Named by the commit, cited by nothing");
    f.close_with(&id, CloseReason::Done, &["doc:spc_01H8"]);

    let drift = f.reconcile(&[commit(
        &sha("1234567"),
        &format!("feat: it ({key}-{n})"),
        f.now - Duration::hours(1),
    )]);

    assert_eq!(drift.linked.len(), 1);
    assert!(
        drift.done_without_commit.is_empty(),
        "the message reached the row; the evidence being thin is a different complaint"
    );
}

#[test]
fn tasks_closed_for_any_reason_but_done_are_never_drift() {
    // `wont_do` and `no_change` legitimately have no commit behind them. If
    // they showed up here the list would be noise within a week and nobody
    // would read it.
    let mut f = fixture();
    let (cut, _) = f.task("Deliberately not doing this");
    f.close_with(&cut, CloseReason::WontDo, &[]);
    let (looked, _) = f.task("Looked, nothing needed");
    f.close_with(&looked, CloseReason::NoChange, &[]);

    let drift = f.reconcile(&[]);
    assert!(drift.done_without_commit.is_empty());
}

#[test]
fn a_task_closed_before_the_window_is_outside_it() {
    let mut f = fixture();
    let (id, _) = f.task("Old news");
    f.close_with(&id, CloseReason::Done, &["test:cargo test"]);
    // Push the close back past the window, the way a row that predates the
    // window would sit in the store.
    let version = match f.store.get(&id).unwrap() {
        Some(Entity::Task(t)) => t.audit.version,
        _ => panic!("a task"),
    };
    let long_ago = (f.now - Duration::days(30)).to_rfc3339();
    let changes = serde_json::json!({"closed_at": long_ago});
    f.store
        .update(&id, version, changes.as_object().unwrap(), &prov())
        .unwrap();

    let drift = f.reconcile(&[]);
    assert!(drift.done_without_commit.is_empty());
}

#[test]
fn an_open_task_is_not_drift_however_thin_its_evidence() {
    let mut f = fixture();
    f.task("Still open");
    let drift = f.reconcile(&[]);
    assert!(drift.done_without_commit.is_empty());
}

#[test]
fn the_scan_reports_how_many_rows_it_read() {
    let mut f = fixture();
    f.task("One");
    f.task("Two");
    let drift = f.reconcile(&[]);
    assert_eq!(drift.tasks_scanned, 2);
    assert_eq!(drift.tasks_total, 2);
}

#[test]
fn the_window_defaults_to_seven_days_back_and_honours_what_was_asked() {
    let now = Utc::now();
    assert_eq!(drift::window(None, now), now - Duration::days(7));
    let asked = now - Duration::days(2);
    assert_eq!(drift::window(Some(asked), now), asked);
}

#[test]
fn a_commit_naming_the_same_missing_key_twice_is_one_unknown_entry() {
    let f = fixture();
    let key = f.key();
    let drift = f.reconcile(&[commit(
        &sha("2222222"),
        &format!("fix: {key}-9999\n\nSee {key}-9999 again, and {key}-8888."),
        f.now - Duration::hours(1),
    )]);
    let keys: Vec<&str> = drift.unknown.iter().map(|u| u.key.as_str()).collect();
    assert_eq!(keys, vec![format!("{key}-9999"), format!("{key}-8888")]);
}

#[test]
fn a_commit_naming_an_archived_task_reached_a_row() {
    // An archived task still owns its number. Reporting it as "no row" tells
    // a model the task was never created, and the natural response to that
    // is to create it — resurrecting something somebody set aside.
    let mut f = fixture();
    let key = f.key();
    let (id, n) = f.task("Set aside");
    f.archive(&id);
    let drift = f.reconcile(&[commit(
        &sha("3333333"),
        &format!("chore: touches {key}-{n}"),
        f.now - Duration::hours(1),
    )]);
    assert!(drift.unknown.is_empty(), "archived is not absent");
    assert_eq!(drift.linked.len(), 1);
}

#[test]
fn an_archived_task_is_never_itself_drift() {
    let mut f = fixture();
    let (id, _) = f.task("Done, then set aside");
    f.close_with(&id, CloseReason::Done, &["test:cargo test"]);
    f.archive(&id);
    let drift = f.reconcile(&[]);
    assert!(drift.done_without_commit.is_empty());
}

#[test]
fn upper_case_commit_evidence_still_cites_the_commit() {
    let mut f = fixture();
    let (id, _) = f.task("Cited in upper case");
    f.close_with(&id, CloseReason::Done, &["commit:CAFE123"]);
    let drift = f.reconcile(&[commit(
        &sha("cafe123"),
        "fix: something",
        f.now - Duration::hours(2),
    )]);
    assert_eq!(drift.linked.len(), 1);
    assert!(drift.done_without_commit.is_empty());
}

#[test]
fn a_capped_read_carries_the_total_it_was_cut_from() {
    let f = fixture();
    let one = [commit(
        &sha("4444444"),
        "chore: newest",
        f.now - Duration::hours(1),
    )];
    let drift = drift::reconcile(&f.store, &f.project, &one, 40, f.since()).unwrap();
    assert_eq!(drift.commits, 1);
    assert_eq!(drift.commits_total, 40);
    // A total smaller than what was read is a caller's arithmetic error,
    // and the read count is the floor.
    let drift = drift::reconcile(&f.store, &f.project, &one, 0, f.since()).unwrap();
    assert_eq!(drift.commits_total, 1);
}

// --- The digest ---------------------------------------------------------------

fn build(f: &Fixture, repository: Source<'_>) -> digest::Digest {
    digest::build(
        &f.store,
        Some(&f.project),
        Depth::Standard,
        None,
        Surfaces::all(),
        repository,
    )
    .unwrap()
}

#[test]
fn no_checkout_says_nothing_and_leaves_the_prose_untouched() {
    let f = fixture();
    let built = build(&f, Source::NoCheckout);
    assert!(built.drift.is_none());
    assert!(
        !built.to_prose().contains("## Commits"),
        "nothing to measure, nothing to say"
    );
}

#[test]
fn an_unreadable_checkout_is_said_out_loud_rather_than_reading_as_a_quiet_week() {
    let f = fixture();
    let built = build(
        &f,
        Source::Unreadable("git exited 128: not a git repository"),
    );
    assert_eq!(
        built.drift,
        Some(DriftSection::Unreadable {
            reason: "git exited 128: not a git repository".to_owned()
        })
    );
    let prose = built.to_prose();
    assert!(
        prose.contains("## Commits\nNot measured: git exited 128"),
        "{prose}"
    );
}

#[test]
fn a_measured_week_renders_its_counts_and_its_lists() {
    let mut f = fixture();
    let key = f.key();
    let (_, n) = f.task("Rowed");
    let (drifted, m) = f.task("Closed on a test alone");
    f.close_with(&drifted, CloseReason::Done, &["test:cargo test"]);
    let yesterday = f.now - Duration::days(1);
    let commits = vec![
        commit(
            &sha("aaaaaaa"),
            &format!("feat: rowed ({key}-{n})"),
            yesterday,
        ),
        commit(&sha("bbbbbbb"), "chore: unrowed", yesterday),
        commit(
            &sha("ccccccc"),
            &format!("fix: ghost ({key}-4242)"),
            yesterday,
        ),
    ];

    let built = build(
        &f,
        Source::Commits {
            since: f.since(),
            commits: &commits,
            total: commits.len(),
        },
    );
    let Some(DriftSection::Measured(d)) = &built.drift else {
        panic!("measured");
    };
    assert_eq!(d.commits, 3);
    assert_eq!(d.linked.len(), 1);
    assert_eq!(d.unlinked.len(), 2, "the ghost reached no row either");
    assert_eq!(d.unknown.len(), 1);
    assert_eq!(d.done_without_commit.len(), 1);

    let prose = built.to_prose();
    assert!(
        prose.contains("3 commit(s): 1 name a task, 2 name none. 1 task(s) closed done with no commit behind them. 1 commit(s) name a task that does not exist."),
        "{prose}"
    );
    assert!(
        prose.contains("- bbbbbbb chore: unrowed — no task\n"),
        "{prose}"
    );
    assert!(
        prose.contains(&format!(
            "- {key}-{m} Closed on a test alone — closed done, no commit cited\n"
        )),
        "{prose}"
    );
    assert!(
        prose.contains(&format!("- ccccccc names {key}-4242, which has no row\n")),
        "{prose}"
    );
}

#[test]
fn a_long_list_is_cut_at_five_and_says_how_many_it_cut() {
    let f = fixture();
    let yesterday = f.now - Duration::days(1);
    let commits: Vec<Commit> = (0..8)
        .map(|i| {
            commit(
                &sha(&format!("{i}{i}{i}{i}{i}{i}{i}")),
                &format!("chore: {i}"),
                yesterday,
            )
        })
        .collect();
    let built = build(
        &f,
        Source::Commits {
            since: f.since(),
            commits: &commits,
            total: commits.len(),
        },
    );
    let prose = built.to_prose();
    assert!(
        prose.contains("8 commit(s): 0 name a task, 8 name none."),
        "{prose}"
    );
    assert!(
        prose.contains("- …and 3 more commit(s) naming no task\n"),
        "{prose}"
    );
    let Some(DriftSection::Measured(d)) = &built.drift else {
        panic!("measured");
    };
    assert_eq!(d.unlinked.len(), 8, "the JSON is never cut");
}

#[test]
fn a_capped_week_says_how_many_it_read_and_the_drift_reaches_the_suggestions() {
    let mut f = fixture();
    let (drifted, _) = f.task("Closed on a test");
    f.close_with(&drifted, CloseReason::Done, &["test:cargo test"]);
    let commits = vec![commit(
        &sha("5555555"),
        "chore: unrowed",
        f.now - Duration::hours(1),
    )];
    let built = build(
        &f,
        Source::Commits {
            since: f.since(),
            commits: &commits,
            total: 700,
        },
    );
    let prose = built.to_prose();
    assert!(
        prose.contains(
            "700 commit(s), of which the newest 1 were read: 0 name a task, 1 name none."
        ),
        "{prose}"
    );
    assert!(
        built
            .next
            .iter()
            .any(|l| l.starts_with("1 of 1 commit(s) since")),
        "{:?}",
        built.next
    );
    assert!(
        built
            .next
            .iter()
            .any(|l| l.starts_with("1 task(s) closed done since")),
        "{:?}",
        built.next
    );
}

#[test]
fn a_measured_empty_week_says_none_and_is_not_the_same_as_unmeasured() {
    let f = fixture();
    let built = build(
        &f,
        Source::Commits {
            since: f.since(),
            commits: &[],
            total: 0,
        },
    );
    assert!(matches!(built.drift, Some(DriftSection::Measured(_))));
    assert!(
        built.to_prose().contains("## Commits since"),
        "measured, and said so"
    );
    assert!(built.to_prose().contains("\nNone.\n"));
}
