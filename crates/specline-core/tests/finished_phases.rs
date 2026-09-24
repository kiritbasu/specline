//! A phase whose work is finished, and the digest saying so.
//!
//! `complete` is derived and `shipped` is declared, deliberately: `done` and
//! `wont_do` both close a task, so a full tally cannot mean the phase shipped
//! (B-57). That part was right and is not what these tests are about.
//!
//! What was wrong is that `complete` had nowhere to appear. The digest's phase
//! section filtered to `active` and `blocked`, so a phase dropped out of every
//! session's first call at the exact moment it needed a person to declare
//! something — and three of this project's own phases sat that way unnoticed,
//! because closing the last task told nobody (KEEL-284).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use specline_core::{
    Actor, Close, CloseReason, EntityId, EntityStore, Milestone, MilestoneStatus, Project,
    Provenance, Store, Task, close,
    digest::{Depth, Surfaces, build},
};

struct Fixture {
    store: Store,
    project: EntityId,
    _dir: tempfile::TempDir,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let project = store
        .create(
            Project::new("demo", "Demo").into(),
            &Provenance::anonymous(Actor::Human),
        )
        .unwrap()
        .entity
        .id()
        .clone();
    Fixture {
        store,
        project,
        _dir: dir,
    }
}

impl Fixture {
    fn phase(&mut self, name: &str) -> EntityId {
        self.store
            .create(
                Milestone::new(self.project.clone(), name, "A phase this test needs.").into(),
                &Provenance::anonymous(Actor::Human),
            )
            .unwrap()
            .entity
            .id()
            .clone()
    }

    fn task_in(&mut self, phase: &EntityId, title: &str) -> EntityId {
        let mut task = Task::new(
            self.project.clone(),
            title,
            "A row this test needs in the store.",
        );
        task.milestone_id = Some(phase.clone());
        self.store
            .create(task.into(), &Provenance::anonymous(Actor::Human))
            .unwrap()
            .entity
            .id()
            .clone()
    }

    fn finish(&mut self, task: &EntityId) {
        close(
            &mut self.store,
            task,
            &Close {
                reason: CloseReason::Done,
                message: "Finished, as this test needs it to be.".to_owned(),
                evidence: vec!["commit:abc1234".to_owned()],
                other: None,
            },
            &Provenance::anonymous(Actor::Claude).with_session("ses_alpha"),
        )
        .unwrap();
    }

    /// A phase with every task closed — the state the digest used to hide.
    fn finished_phase(&mut self, name: &str) -> EntityId {
        let phase = self.phase(name);
        let task = self.task_in(&phase, &format!("The only task in {name}"));
        self.finish(&task);
        phase
    }

    fn declare(&mut self, phase: &EntityId, status: MilestoneStatus) {
        let version = self.store.get(phase).unwrap().unwrap().audit().version;
        let mut changes = serde_json::Map::new();
        changes.insert("status".to_owned(), status.as_str().into());
        self.store
            .update(
                phase,
                version,
                &changes,
                &Provenance::anonymous(Actor::Human),
            )
            .unwrap();
    }

    fn digest(&self) -> specline_core::digest::Digest {
        build(
            &self.store,
            Some(&self.project),
            Depth::Standard,
            None,
            Surfaces::all(),
            specline_core::drift::Source::NoCheckout,
        )
        .unwrap()
    }
}

#[test]
fn a_phase_with_every_task_closed_is_reported_as_finished() {
    let mut f = setup();
    let phase = f.finished_phase("Phase 1 — Spine");

    let digest = f.digest();
    let ids: Vec<&EntityId> = digest.complete.iter().map(|i| &i.id).collect();
    assert_eq!(ids, vec![&phase], "a finished phase belongs in `complete`");
    assert!(
        digest.active.is_empty(),
        "and it is no longer in flight: {:?}",
        digest.active
    );
}

#[test]
fn the_prose_says_which_decision_is_owed() {
    let mut f = setup();
    f.finished_phase("Phase 1 — Spine");

    let prose = f.digest().to_prose();
    assert!(
        prose.contains("Finished — ask the user"),
        "the section is missing:\n{prose}"
    );
    assert!(
        prose.contains("Phase 1 — Spine"),
        "the phase is not named:\n{prose}"
    );
}

/// KEEL-372. The passive wording sat unread for five weeks, so the section now
/// says who to ask, how to record the answer, and the date the answer needs.
#[test]
fn the_prose_asks_for_the_decision_and_gives_the_date_it_needs() {
    // Either side of the setup, so a run that straddles midnight UTC still
    // finds the date the task actually closed on.
    let before = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mut f = setup();
    f.finished_phase("Phase 1 — Spine");
    let after = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let prose = f.digest().to_prose();
    assert!(prose.contains("Ask them once"), "{prose}");
    assert!(prose.contains("specline_update"), "{prose}");
    assert!(prose.contains("shipped_at"), "{prose}");
    assert!(
        prose.contains(&format!("the last closed {before}"))
            || prose.contains(&format!("the last closed {after}")),
        "each phase carries when its last task closed:\n{prose}"
    );
}

/// The failure case that matters most: an unfinished phase must not appear
/// here, or the section becomes noise a reader learns to skip.
#[test]
fn a_phase_with_one_task_still_open_is_not_reported() {
    let mut f = setup();
    let phase = f.phase("Phase 1 — Spine");
    let done = f.task_in(&phase, "Finished");
    f.task_in(&phase, "Still going");
    f.finish(&done);

    let digest = f.digest();
    assert!(
        digest.complete.is_empty(),
        "one open task means the phase is not finished: {:?}",
        digest.complete
    );
}

/// Declaring is the way out of the section. A phase somebody has already called
/// shipped or cut is a decision taken, and re-asking for it is how a prompt
/// turns into something people ignore.
#[test]
fn a_declared_phase_leaves_the_section() {
    for declared in [MilestoneStatus::Shipped, MilestoneStatus::Cut] {
        let mut f = setup();
        let phase = f.finished_phase("Phase 1 — Spine");
        assert_eq!(f.digest().complete.len(), 1, "{declared:?}: before");

        f.declare(&phase, declared);
        assert!(
            f.digest().complete.is_empty(),
            "{declared:?}: a declared phase still being asked about"
        );
    }
}

/// A phase in flight carries the state that was *derived*, not the `open` in
/// its column. The two say different things and only one is worth reading.
#[test]
fn an_active_phase_reports_its_derived_state() {
    let mut f = setup();
    let phase = f.phase("Phase 1 — Spine");
    let done = f.task_in(&phase, "Finished");
    f.task_in(&phase, "Still going");
    f.finish(&done);

    let digest = f.digest();
    let item = digest
        .active
        .iter()
        .find(|i| i.id == phase)
        .expect("the phase is in flight");
    assert_eq!(item.status.as_deref(), Some("active"));
}

/// Hard constraint 4, on the one list where a silent cut would recreate the bug
/// the list exists to fix.
#[test]
fn cutting_the_section_reports_what_it_dropped() {
    let mut f = setup();
    for n in 1..=5 {
        f.finished_phase(&format!("Phase {n}"));
    }

    // `brief` carries three items per section, so five is two over.
    let digest = build(
        &f.store,
        Some(&f.project),
        Depth::Brief,
        None,
        Surfaces::all(),
        specline_core::drift::Source::NoCheckout,
    )
    .unwrap();
    assert_eq!(digest.complete.len(), 3);
    let cut = digest
        .truncated
        .iter()
        .find(|t| t.section == "complete")
        .expect("the cut is reported");
    assert_eq!((cut.shown, cut.total), (3, 5));
}
