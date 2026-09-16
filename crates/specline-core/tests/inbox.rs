//! The Inbox: untriaged signals, and what must never count them as work.
//!
//! A signal is something somebody wants. A task is work somebody committed to.
//! Every count in the product answers one question or the other, and the whole
//! reason B-90 separates them is that a number mixing the two cannot answer
//! either — "37 open" is not usable for "how much is left" if four of the
//! thirty-seven are ideas nobody has looked at.
//!
//! So these tests are mostly negative: they assert what the Inbox is *not* in.
//! That is the half that cannot be seen by looking at a screen, and the half
//! that regresses silently, because a signal leaking into the task count looks
//! exactly like a project with one more task.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use specline_core::{
    Actor, Close, CloseReason, Direction, EntityId, EntityStore, Feedback, FeedbackKind,
    GraphStore, NewLink, Project, Provenance, Relation, Spec, SpecKind, Store, Task,
    digest::{self, Depth, Surfaces},
    work::TriageOutcome,
};

fn prov() -> Provenance {
    Provenance::anonymous(Actor::Claude).with_session("ses_inbox")
}

fn fixture() -> (tempfile::TempDir, Store, EntityId) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("specline.sqlite")).unwrap();
    let project = store
        .create(Project::new("specline", "Specline").into(), &prov())
        .unwrap()
        .entity
        .id()
        .clone();
    (dir, store, project)
}

fn file_signal(store: &mut Store, project: &EntityId, said: &str) -> EntityId {
    let mut signal = Feedback::new(project.clone(), said);
    signal.kind = FeedbackKind::Idea;
    store
        .create(signal.into(), &prov())
        .unwrap()
        .entity
        .id()
        .clone()
}

#[test]
fn an_untriaged_signal_is_counted_in_the_inbox() {
    let (_d, mut store, project) = fixture();

    let (none, oldest) = store.untriaged_signals(&project).unwrap();
    assert_eq!(none, 0);
    assert!(
        oldest.is_none(),
        "with nothing waiting there is no oldest thing waiting"
    );

    file_signal(&mut store, &project, "this should work with codex");
    file_signal(&mut store, &project, "search should look inside documents");

    let (count, oldest) = store.untriaged_signals(&project).unwrap();
    assert_eq!(count, 2);
    assert!(
        oldest.is_some(),
        "a non-zero count always has an oldest, or the age can never be reported"
    );
}

/// Triaging is what empties the Inbox, and it is the only thing that does.
/// Nothing here archives or deletes the signal — it stays, with its outcome,
/// because B-91 turns on being able to find out what was decided about it.
#[test]
fn a_triaged_signal_leaves_the_inbox_without_leaving_the_store() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "this should work with codex");

    let current = store.get(&signal).unwrap().unwrap();
    let mut changes = serde_json::Map::new();
    changes.insert("triaged".to_owned(), serde_json::Value::Bool(true));
    store
        .update(&signal, current.audit().version, &changes, &prov())
        .unwrap();

    let (count, _) = store.untriaged_signals(&project).unwrap();
    assert_eq!(count, 0, "triaging is what takes it out of the Inbox");
    assert!(
        store.get(&signal).unwrap().is_some(),
        "and the signal itself is still there, because the outcome has to stay findable"
    );
}

/// An archived signal is out of the Inbox too — soft delete is still delete as
/// far as a count of outstanding work is concerned, and a signal somebody
/// archived is not waiting on anybody.
#[test]
fn an_archived_signal_is_not_waiting_for_anyone() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "a thing said once and withdrawn");
    let version = store.get(&signal).unwrap().unwrap().audit().version;

    store.archive(&signal, version, &prov()).unwrap();

    assert_eq!(store.untriaged_signals(&project).unwrap().0, 0);
}

/// The one that matters, and the one that would regress silently. A signal
/// leaking into `open_tasks` looks identical to a project with one more task,
/// so nothing about the output would tell you it had happened.
#[test]
fn a_signal_is_not_a_task_and_is_counted_by_nothing_that_counts_tasks() {
    let (_d, mut store, project) = fixture();

    store
        .create(
            Task::new(
                project.clone(),
                "Wire up the exporter",
                "The exporter writes nothing when the window is empty.",
            )
            .into(),
            &prov(),
        )
        .unwrap();
    for said in [
        "this should work with codex",
        "search should look inside documents",
        "the board should group by phase",
    ] {
        file_signal(&mut store, &project, said);
    }

    let (open, _urgent) = store.task_counts(&project).unwrap();
    assert_eq!(
        open, 1,
        "three signals and one task is one open task, not four"
    );

    let ranked = specline_core::next::rank(&store, &project).unwrap();
    let named: Vec<&str> = ranked
        .ready
        .iter()
        .map(|c| c.title.as_str())
        .chain(ranked.waiting_on_you.iter().map(|c| c.title.as_str()))
        .collect();
    assert!(
        !named.iter().any(|l| l.contains("codex")),
        "a signal is not something to pick up next: {named:?}"
    );
}

/// Hard constraint 4, at the digest. The Inbox may be long by design, so the
/// digest carries its size rather than its contents — but carrying neither is
/// the silent omission the constraint exists to forbid, and it is how four
/// signals sat next to the release work with nothing saying so.
#[test]
fn the_digest_says_how_many_signals_are_waiting() {
    let (_d, mut store, project) = fixture();

    let quiet = digest::build(
        &store,
        Some(&project),
        Depth::Standard,
        None,
        Surfaces::all(),
        specline_core::drift::Source::NoCheckout,
    )
    .unwrap();
    assert_eq!(quiet.project.as_ref().unwrap().inbox, 0);
    assert!(
        !quiet.to_prose().contains("inbox:"),
        "an empty Inbox says nothing — a line reading 0 is one every session reads forever to \
         learn nothing"
    );

    file_signal(&mut store, &project, "this should work with codex");
    file_signal(&mut store, &project, "search should look inside documents");

    let built = digest::build(
        &store,
        Some(&project),
        Depth::Standard,
        None,
        Surfaces::all(),
        specline_core::drift::Source::NoCheckout,
    )
    .unwrap();
    let line = built.project.as_ref().unwrap();
    assert_eq!(line.inbox, 2);
    assert_eq!(
        line.inbox_oldest_days,
        Some(0),
        "filed just now, so nothing has been waiting a day"
    );

    let prose = built.to_prose();
    assert!(
        prose.contains("inbox: 2 untriaged signal(s)"),
        "the digest has to say the Inbox is there: {prose}"
    );
    // On its own line, not folded into the task counts, because the whole
    // point is that they are different kinds of number.
    assert!(
        prose.contains("0 open task(s)"),
        "and the task count stays a task count: {prose}"
    );
}

/// The digest **lists** the Inbox, and this reverses KEEL-321 deliberately.
///
/// That task carried the size and the age only, on the stated reasoning that
/// `specline_search` would fetch the rows when somebody was going to triage
/// them. It cannot: search requires a query and has no filter for "everything
/// untriaged". So a count with no way to reach the contents left a session
/// able to know the Inbox was long and unable to read it, which is precisely
/// the position KEEL-303 complains about.
#[test]
fn the_digest_lists_the_oldest_signals_and_says_how_many_it_left() {
    let (_d, mut store, project) = fixture();
    for i in 0..12 {
        file_signal(&mut store, &project, &format!("somebody wanted thing {i}"));
    }

    let built = digest::build(
        &store,
        Some(&project),
        Depth::Standard,
        None,
        Surfaces::all(),
        specline_core::drift::Source::NoCheckout,
    )
    .unwrap();
    assert_eq!(built.inbox.len(), 8, "a small slice, not the whole pile");
    assert_eq!(
        built.inbox.first().map(|i| i.label.as_str()),
        Some("somebody wanted thing 0"),
        "oldest first, so the thing ignored longest is not buried"
    );

    // Hard constraint 4, and it matters most here: eight of twelve with
    // nothing saying so reads as an Inbox of eight.
    let cut = built
        .truncated
        .iter()
        .find(|t| t.section == "inbox")
        .expect("a cut Inbox says it was cut");
    assert_eq!(cut.shown, 8);
    assert_eq!(cut.total, 12);

    let prose = built.to_prose();
    assert!(prose.contains("Inbox — untriaged"), "{prose}");
    assert!(
        prose.contains("somebody wanted thing 0"),
        "the signals are readable, not merely counted: {prose}"
    );
}

/// Who asked and how long it has waited, together, because that is what makes
/// a signal triageable without opening it — one person's passing idea from
/// yesterday and a request three people have made over two months want
/// different answers.
#[test]
fn a_listed_signal_carries_its_source_and_its_age() {
    let (_d, mut store, project) = fixture();
    let mut signal = Feedback::new(project.clone(), "this should work with codex");
    signal.kind = FeedbackKind::Idea;
    signal.source = Some("Madhu".to_owned());
    store.create(signal.into(), &prov()).unwrap();

    let built = digest::build(
        &store,
        Some(&project),
        Depth::Standard,
        None,
        Surfaces::all(),
        specline_core::drift::Source::NoCheckout,
    )
    .unwrap();
    let detail = built.inbox[0].detail.as_deref().unwrap_or_default();
    assert!(detail.contains("Madhu"), "{detail}");
    assert!(detail.contains("today"), "{detail}");
}

// --- Picking a signal up (KEEL-323) ---------------------------------------

/// A signal that survives triage becomes a **feature spec**, not a task.
///
/// That is the structural call B-90 turns on: the thinking outlives the work.
/// The spec exists whether or not the thing is ever built, which is what a
/// session picking up child task nine reads to learn why the other eight
/// matter — and it is what keeps an unbuilt idea off the board entirely, since
/// the epic task is created only at the moment somebody decides to build.
#[test]
fn a_picked_up_signal_becomes_a_feature_spec_that_remembers_where_it_came_from() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "this should work with codex");

    let mut spec = Spec::new(project.clone(), "Specline works with OpenAI Codex");
    spec.kind = SpecKind::Feature;
    let feature = store
        .create_with_document(
            spec.into(),
            Some(
                "Madhu asked for this. The open question is whether it means the MCP endpoint \
                 or the whole plugin surface."
                    .to_owned(),
            ),
            None,
            &prov(),
        )
        .unwrap()
        .entity
        .id()
        .clone();

    store
        .link(
            NewLink::new(feature.clone(), Relation::DerivedFrom, signal.clone()),
            &prov(),
        )
        .unwrap();

    // Both directions, because an inverted traversal returns an empty set that
    // is indistinguishable from "nothing is linked here" — which here would
    // mean a feature whose origin is silently unrecoverable.
    let out: Vec<EntityId> = store
        .neighbours(&feature, Direction::Outbound, &[Relation::DerivedFrom], 1)
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert!(
        out.contains(&signal),
        "the feature derives from the signal: {out:?}"
    );

    let inbound: Vec<EntityId> = store
        .neighbours(&signal, Direction::Inbound, &[Relation::DerivedFrom], 1)
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert!(
        inbound.contains(&feature),
        "and the signal knows what it became, which is how the loop gets closed \
         back to whoever asked: {inbound:?}"
    );
}

/// `feature` is a real value of the enum and the refusal lists it, because a
/// model reading "`feature` is not valid" without being shown that it is would
/// simply guess something else.
#[test]
fn feature_is_one_of_the_spec_kinds_and_the_refusal_says_so() {
    assert_eq!(SpecKind::parse("feature").unwrap(), SpecKind::Feature);
    let err = SpecKind::parse("epic").unwrap_err().to_string();
    assert!(err.contains("feature"), "{err}");
}

// --- Triage: picked up, or set down (KEEL-324) ----------------------------

fn a_feature(store: &mut Store, project: &EntityId, title: &str) -> EntityId {
    let mut spec = Spec::new(project.clone(), title);
    spec.kind = SpecKind::Feature;
    store
        .create_with_document(
            spec.into(),
            Some("The case for building it, which is the whole of what a feature is.".to_owned()),
            None,
            &prov(),
        )
        .unwrap()
        .entity
        .id()
        .clone()
}

#[test]
fn picking_a_signal_up_names_the_feature_and_leaves_the_inbox() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "this should work with codex");
    let feature = a_feature(&mut store, &project, "Specline works with OpenAI Codex");

    let triaged = specline_core::work::triage(
        &mut store,
        &signal,
        &TriageOutcome::PickedUp {
            feature: Some(feature.clone()),
        },
        &prov(),
    )
    .unwrap();

    assert!(triaged.signal.triaged);
    assert_eq!(triaged.linked, Some((Relation::DerivedFrom, feature)));
    assert_eq!(store.untriaged_signals(&project).unwrap().0, 0);
}

/// Setting down **appends**. The signal's body is the verbatim, and replacing
/// it with the reason for setting it down would destroy what somebody actually
/// said in the act of recording why we are not doing it.
#[test]
fn setting_a_signal_down_keeps_the_verbatim_and_adds_the_argument() {
    let (_d, mut store, project) = fixture();
    let mut signal = Feedback::new(project.clone(), "we should build a public request portal");
    signal.kind = FeedbackKind::Idea;
    let signal = store
        .create_with_document(
            signal.into(),
            Some("Said in passing, twice in one week.".to_owned()),
            None,
            &prov(),
        )
        .unwrap()
        .entity
        .id()
        .clone();

    let triaged = specline_core::work::triage(
        &mut store,
        &signal,
        &TriageOutcome::SetDown {
            reason: "There is no customer stream yet, so a portal would collect nothing. \
                     Worth revisiting once somebody other than us is filing."
                .to_owned(),
        },
        &prov(),
    )
    .unwrap();

    assert!(triaged.signal.triaged);
    let body = triaged.revision.expect("a set-down writes a revision").body;
    assert!(
        body.contains("Said in passing, twice in one week."),
        "the verbatim survives: {body}"
    );
    assert!(
        body.contains("Set down"),
        "and the argument is on it: {body}"
    );

    // The original is still readable on its own, which is what makes the
    // append safe rather than merely tidy.
    let first = store.revision(&signal, Some(1)).unwrap().unwrap();
    assert!(!first.body.contains("Set down"));
}

/// B-91 lets set-down reasoning live on the signal rather than in the decision
/// log *on the grounds that it stays findable*. A one-word reason is not
/// findable — nobody searches for it and it answers nothing when found — so
/// the promise only holds if there is something there.
#[test]
fn a_set_down_with_no_argument_is_refused() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "add dark mode to the emails");

    let err = specline_core::work::triage(
        &mut store,
        &signal,
        &TriageOutcome::SetDown {
            reason: "no".to_owned(),
        },
        &prov(),
    )
    .unwrap_err()
    .to_string();

    assert!(err.contains("argument"), "{err}");
    assert_eq!(
        store.untriaged_signals(&project).unwrap().0,
        1,
        "a refused triage leaves the signal in the Inbox, not half out of it"
    );
}

/// Picking up has to name a *feature*, not any spec. Pointing at a PRD would
/// record an outcome that does not say why, which is the whole thing a pick-up
/// is supposed to produce.
#[test]
fn picking_up_something_that_is_not_a_feature_spec_is_refused() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "this should work with codex");

    let mut spec = Spec::new(project.clone(), "The technical specification");
    spec.kind = SpecKind::Spec;
    let plain = store
        .create_with_document(
            spec.into(),
            Some("Not a feature.".to_owned()),
            None,
            &prov(),
        )
        .unwrap()
        .entity
        .id()
        .clone();

    let err = specline_core::work::triage(
        &mut store,
        &signal,
        &TriageOutcome::PickedUp {
            feature: Some(plain),
        },
        &prov(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("feature"), "{err}");
    assert_eq!(store.untriaged_signals(&project).unwrap().0, 1);
}

/// A second outcome would replace the first, and the first is the one somebody
/// reasoned about.
#[test]
fn a_signal_cannot_be_triaged_twice() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "this should work with codex");
    let feature = a_feature(&mut store, &project, "Codex support");

    specline_core::work::triage(
        &mut store,
        &signal,
        &TriageOutcome::PickedUp {
            feature: Some(feature.clone()),
        },
        &prov(),
    )
    .unwrap();

    let err = specline_core::work::triage(
        &mut store,
        &signal,
        &TriageOutcome::PickedUp {
            feature: Some(feature),
        },
        &prov(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("already been triaged"), "{err}");
}

#[test]
fn only_a_signal_can_be_triaged() {
    let (_d, mut store, project) = fixture();
    let task = store
        .create(
            Task::new(project.clone(), "A task", "Not a signal.").into(),
            &prov(),
        )
        .unwrap()
        .entity
        .id()
        .clone();

    let err = specline_core::work::triage(
        &mut store,
        &task,
        &TriageOutcome::SetDown {
            reason: "this reason is long enough to pass the length check".to_owned(),
        },
        &prov(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("only a signal can be triaged"), "{err}");
}

// --- Triage through `close` (B-94, KEEL-325) ------------------------------
//
// Closing is what you do to anything that is dealt with, not only to a task.
// These assert the translation, not the invariants — `close_signal` delegates
// to `triage`, so the invariants are already covered above and duplicating
// them here would mean two sets to keep in step.

fn closing(reason: CloseReason, message: &str) -> Close {
    Close {
        reason,
        message: message.to_owned(),
        evidence: vec![],
        other: None,
    }
}

#[test]
fn closing_a_signal_as_done_picks_it_up_and_names_the_feature() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "this should work with codex");
    let feature = a_feature(&mut store, &project, "Codex support");

    let mut request = closing(CloseReason::Done, "Picked up — Madhu and two others.");
    request.evidence = vec![format!("doc:{feature}")];

    let triaged =
        specline_core::work::close_signal(&mut store, &signal, &request, &prov()).unwrap();
    assert!(triaged.signal.triaged);
    assert_eq!(triaged.linked, Some((Relation::DerivedFrom, feature)));
}

/// Not every want becomes an epic. Some become a commit, and a signal that has
/// genuinely been answered still has to be closable.
///
/// This assertion is the reverse of the one that was here first, and the
/// reversal was found by using the thing: the first real triage pass hit KB's
/// complaint about the rail's `·1` markers, which had already been answered by
/// a small interface fix — so demanding a feature spec made the one signal in
/// the Inbox that was genuinely finished the one that could not be closed.
#[test]
fn a_signal_answered_by_a_commit_rather_than_a_feature_still_closes() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "the rail markers read as unclear");

    let mut request = closing(CloseReason::Done, "Fixed — the digits are keycaps now.");
    request.evidence = vec!["commit:abc1234".to_owned()];

    let triaged =
        specline_core::work::close_signal(&mut store, &signal, &request, &prov()).unwrap();
    assert!(triaged.signal.triaged);
    assert!(
        triaged.linked.is_none(),
        "there is no feature to point at, and inventing an edge would be worse than none"
    );
    assert_eq!(store.untriaged_signals(&project).unwrap().0, 0);
}

/// Two features would leave "which one did it become?" unanswerable, and the
/// edge can only point at one.
#[test]
fn closing_a_signal_as_done_naming_two_features_is_refused() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "this should work with codex");
    let one = a_feature(&mut store, &project, "Codex support");
    let two = a_feature(&mut store, &project, "Any host support");

    let mut request = closing(CloseReason::Done, "Both, somehow.");
    request.evidence = vec![format!("doc:{one}"), format!("doc:{two}")];

    let err = specline_core::work::close_signal(&mut store, &signal, &request, &prov())
        .unwrap_err()
        .to_string();
    assert!(err.contains("a signal becomes one"), "{err}");
    assert_eq!(store.untriaged_signals(&project).unwrap().0, 1);
}

#[test]
fn closing_a_signal_as_wont_do_sets_it_down_with_the_message_as_the_argument() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "build a public request portal");

    let triaged = specline_core::work::close_signal(
        &mut store,
        &signal,
        &closing(
            CloseReason::WontDo,
            "There is no customer stream yet, so a portal would collect nothing.",
        ),
        &prov(),
    )
    .unwrap();

    let body = triaged.revision.expect("a set-down writes a revision").body;
    assert!(body.contains("would collect nothing"), "{body}");
}

/// A duplicate is not a set-down, and the difference carries information: two
/// people asking is the demand signal the Inbox has no other way to hold.
#[test]
fn closing_a_signal_as_a_duplicate_points_at_the_one_that_keeps_the_history() {
    let (_d, mut store, project) = fixture();
    let first = file_signal(&mut store, &project, "search should look inside documents");
    let again = file_signal(&mut store, &project, "search only reads titles");

    let mut request = closing(CloseReason::Duplicate, "Same want, said differently.");
    request.other = Some(first.clone());

    let triaged = specline_core::work::close_signal(&mut store, &again, &request, &prov()).unwrap();
    assert_eq!(triaged.linked, Some((Relation::Duplicates, first)));
    assert!(
        triaged.revision.is_none(),
        "a duplicate records where the want went, not an argument against it"
    );
}

#[test]
fn a_duplicate_of_an_already_triaged_signal_is_refused() {
    let (_d, mut store, project) = fixture();
    let first = file_signal(&mut store, &project, "search should look inside documents");
    let again = file_signal(&mut store, &project, "search only reads titles");
    specline_core::work::triage(
        &mut store,
        &first,
        &TriageOutcome::SetDown {
            reason: "Not now — the index rebuild is the expensive part.".to_owned(),
        },
        &prov(),
    )
    .unwrap();

    let mut request = closing(CloseReason::Duplicate, "Same want.");
    request.other = Some(first);
    let err = specline_core::work::close_signal(&mut store, &again, &request, &prov())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("already been triaged"),
        "pointing a live want at a dead row hides it: {err}"
    );
}

/// The two that mean nothing about a want. Refused with the three that do,
/// because a caller told only that `superseded` is wrong will try `no_change`.
#[test]
fn superseded_and_no_change_do_not_describe_a_signal() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "this should work with codex");

    for reason in [CloseReason::Superseded, CloseReason::NoChange] {
        let err = specline_core::work::close_signal(
            &mut store,
            &signal,
            &closing(reason, "Trying it anyway."),
            &prov(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("does not describe"), "{err}");
        assert!(
            err.contains("wont_do") && err.contains("duplicate"),
            "the refusal has to list the ones that do work: {err}"
        );
    }
}

// --- Switched off (KEEL-341) ----------------------------------------------

/// The flag hides **surfaces**, never data. v0.4.0 shipped filing, the nav
/// item and the digest count without triage, so signals could go in and not
/// come out — but the signals themselves are somebody's captured thinking, and
/// hiding a screen must not amount to losing them.
#[test]
fn switching_the_inbox_off_hides_it_from_the_digest_and_keeps_the_signals() {
    let (_d, mut store, project) = fixture();
    file_signal(&mut store, &project, "this should work with codex");
    file_signal(&mut store, &project, "search should look inside documents");

    let off = digest::build(
        &store,
        Some(&project),
        Depth::Standard,
        None,
        Surfaces::default(),
        specline_core::drift::Source::NoCheckout,
    )
    .unwrap();
    assert!(off.inbox.is_empty(), "no section");
    assert_eq!(off.project.as_ref().unwrap().inbox, 0, "no count either");
    assert_eq!(off.project.as_ref().unwrap().inbox_oldest_days, None);
    assert!(!off.to_prose().contains("Inbox"));
    assert!(
        !off.truncated.iter().any(|t| t.section == "inbox"),
        "a hidden section is not a cut one — reporting it as truncated would \
         advertise the thing being hidden"
    );

    // Still there, and reappearing intact is the property that makes the flag
    // safe to leave off.
    assert_eq!(store.untriaged_signals(&project).unwrap().0, 2);
    let on = digest::build(
        &store,
        Some(&project),
        Depth::Standard,
        None,
        Surfaces::all(),
        specline_core::drift::Source::NoCheckout,
    )
    .unwrap();
    assert_eq!(on.inbox.len(), 2);
    assert_eq!(on.project.as_ref().unwrap().inbox, 2);
}

/// Triage still works underneath. The flag is a surface decision taken by a
/// caller, and burying it in the write path would mean a switched-off store
/// silently behaving differently from a switched-on one at the level where
/// data lives.
#[test]
fn the_flag_does_not_reach_the_write_path() {
    let (_d, mut store, project) = fixture();
    let signal = file_signal(&mut store, &project, "this should work with codex");

    specline_core::work::triage(
        &mut store,
        &signal,
        &TriageOutcome::SetDown {
            reason: "Not now, and the argument stays findable either way.".to_owned(),
        },
        &prov(),
    )
    .expect("core takes no view on which surfaces a caller shows");
}
