//! Which editor wrote a row, recorded once per conversation.
//!
//! `Surface` says what *kind* of place a write came from and there are five of
//! them. It cannot say which editor, and the moment Claude Code and Codex are
//! both writing `code` the two are indistinguishable — which is what KEEL-360
//! is for.
//!
//! A table rather than a column on thirteen entity types: every write already
//! stamps a `session_id`, so one row per session answers the question for tasks,
//! notes, revisions and events alike.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use specline_core::*;

fn store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("create a temp dir");
    let store = Store::open(dir.path().join("specline.sqlite")).expect("open the store");
    (store, dir)
}

fn codex() -> Client {
    Client {
        name: "codex-mcp-client".to_owned(),
        title: Some("Codex".to_owned()),
        version: Some("0.148.0-alpha.15".to_owned()),
    }
}

fn claude_code() -> Client {
    Client {
        name: "claude-code".to_owned(),
        title: None,
        version: Some("2.1.185".to_owned()),
    }
}

/// A write, and the provenance describing who made it.
fn write_as(store: &mut Store, provenance: &Provenance, title: &str) -> EntityId {
    let project = Project::new("Demo", "demo");
    let id = project.id.clone();
    // A project only needs creating once per store; ignore a duplicate.
    let _ = store.create(Entity::Project(project), provenance);

    let task = Task::new(id.clone(), title, "what it is for");
    let task_id = task.id.clone();
    store
        .create(Entity::Task(task), provenance)
        .expect("create a task");
    task_id
}

#[test]
fn a_write_records_the_client_that_made_it() {
    let (mut store, _dir) = store();
    let provenance = Provenance::anonymous(Actor::Claude)
        .with_session("ses_codex")
        .with_surface(Surface::Code)
        .with_client(codex());

    write_as(&mut store, &provenance, "Something Codex did");

    let recorded = store
        .client_for_session("ses_codex")
        .unwrap()
        .expect("the session names its client");
    assert_eq!(recorded.client.name, "codex-mcp-client");
    assert_eq!(recorded.client.version.as_deref(), Some("0.148.0-alpha.15"));
    assert_eq!(
        recorded.client.display_name(),
        "Codex",
        "a title is what a reader should see"
    );
}

/// The case the whole feature exists for.
///
/// Both write `code` as their surface, so before this the store could not tell
/// them apart at all.
#[test]
fn two_editors_writing_the_same_surface_stay_distinguishable() {
    let (mut store, _dir) = store();

    let from_codex = Provenance::anonymous(Actor::Claude)
        .with_session("ses_a")
        .with_surface(Surface::Code)
        .with_client(codex());
    let from_claude = Provenance::anonymous(Actor::Claude)
        .with_session("ses_b")
        .with_surface(Surface::Code)
        .with_client(claude_code());

    write_as(&mut store, &from_codex, "One");
    write_as(&mut store, &from_claude, "Two");

    assert_eq!(
        store
            .client_for_session("ses_a")
            .unwrap()
            .unwrap()
            .client
            .display_name(),
        "Codex"
    );
    assert_eq!(
        store
            .client_for_session("ses_b")
            .unwrap()
            .unwrap()
            .client
            .display_name(),
        "claude-code",
        "no title, so the raw name is what there is"
    );
}

/// Unknown is a real answer and must not be dressed up as Claude Code.
///
/// Three different things arrive here — a session older than this table, a
/// transport that reported no client, and a session that never wrote — and all
/// three are honestly unknown. Guessing would be right often enough to be
/// believed and wrong exactly where a second editor makes it interesting.
#[test]
fn a_session_with_no_client_is_unknown_rather_than_assumed() {
    let (mut store, _dir) = store();
    let anonymous = Provenance::anonymous(Actor::Claude)
        .with_session("ses_no_client")
        .with_surface(Surface::Code);

    write_as(&mut store, &anonymous, "Written by something unnamed");

    assert!(
        store.client_for_session("ses_no_client").unwrap().is_none(),
        "no client reported means no row, not a default"
    );
    assert!(
        store
            .client_for_session("ses_never_existed")
            .unwrap()
            .is_none(),
        "a session that never wrote is equally unknown"
    );
}

/// A conversation keeps its first sighting and moves its last.
#[test]
fn a_second_write_moves_last_seen_and_leaves_first_seen() {
    let (mut store, _dir) = store();
    let provenance = Provenance::anonymous(Actor::Claude)
        .with_session("ses_twice")
        .with_surface(Surface::Code)
        .with_client(codex());

    write_as(&mut store, &provenance, "First");
    let after_one = store.client_for_session("ses_twice").unwrap().unwrap();

    write_as(&mut store, &provenance, "Second");
    let after_two = store.client_for_session("ses_twice").unwrap().unwrap();

    assert_eq!(
        after_one.first_seen, after_two.first_seen,
        "when the conversation started does not change"
    );
    assert!(
        after_two.last_seen >= after_one.last_seen,
        "when it was last heard from does"
    );
}

/// A client that updates mid-conversation reads as the version now running.
#[test]
fn an_upgraded_client_refreshes_its_version() {
    let (mut store, _dir) = store();
    let session = "ses_upgrade";

    let before = Provenance::anonymous(Actor::Claude)
        .with_session(session)
        .with_surface(Surface::Code)
        .with_client(Client {
            name: "codex-mcp-client".to_owned(),
            title: Some("Codex".to_owned()),
            version: Some("0.148.0".to_owned()),
        });
    write_as(&mut store, &before, "Before the upgrade");

    let after = Provenance::anonymous(Actor::Claude)
        .with_session(session)
        .with_surface(Surface::Code)
        .with_client(Client {
            name: "codex-mcp-client".to_owned(),
            title: Some("Codex".to_owned()),
            version: Some("0.149.0".to_owned()),
        });
    write_as(&mut store, &after, "After it");

    assert_eq!(
        store
            .client_for_session(session)
            .unwrap()
            .unwrap()
            .client
            .version
            .as_deref(),
        Some("0.149.0"),
    );
}

/// The list behind "which editors are talking to Specline", newest first.
#[test]
fn the_clients_that_have_written_come_back_most_recent_first() {
    let (mut store, _dir) = store();

    for (session, client) in [("ses_1", claude_code()), ("ses_2", codex())] {
        let provenance = Provenance::anonymous(Actor::Claude)
            .with_session(session)
            .with_surface(Surface::Code)
            .with_client(client);
        write_as(&mut store, &provenance, &format!("work in {session}"));
    }

    let listed = store.session_clients(10).unwrap();
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert_eq!(
        listed[0].session_id, "ses_2",
        "the most recently seen comes first"
    );

    assert!(
        store.session_clients(1).unwrap().len() == 1,
        "the limit is respected"
    );
}

/// The interface writes as a person and has no client, and that is deliberate.
///
/// A browser's user agent would name Chrome — true, and an answer to a question
/// nobody asked. `ui` already says everything a reader needs.
#[test]
fn a_write_from_the_interface_records_no_client() {
    let (mut store, _dir) = store();
    let person = Provenance::anonymous(Actor::Human)
        .with_session("ses_ui")
        .with_surface(Surface::Ui);

    write_as(&mut store, &person, "Filed from the app");

    assert!(store.client_for_session("ses_ui").unwrap().is_none());
}

/// A note is the mutation that appends no event, so it needs its own recording.
///
/// This is the case the feature was asked for — "browsing any tickets **or
/// notes**" — and the first implementation missed it, because the recording
/// rode on the event path and a note does not take it. A session that only ever
/// annotates is the ordinary shape of a conversation once the row exists.
#[test]
fn a_note_records_the_client_even_though_it_appends_no_event() {
    let (mut store, _dir) = store();

    // One session creates the task, a different one only annotates it.
    let author = Provenance::anonymous(Actor::Claude)
        .with_session("ses_creator")
        .with_surface(Surface::Code)
        .with_client(claude_code());
    let task_id = write_as(&mut store, &author, "Something to annotate");

    let annotator = Provenance::anonymous(Actor::Claude)
        .with_session("ses_annotator")
        .with_surface(Surface::Code)
        .with_client(codex());
    store
        .add_note(
            NewNote::new(
                task_id,
                "Codex looked at this and had a thought",
                Actor::Claude,
            ),
            &annotator,
        )
        .expect("add a note");

    let recorded = store
        .client_for_session("ses_annotator")
        .unwrap()
        .expect("a note-only session still names its client");
    assert_eq!(recorded.client.display_name(), "Codex");
}

/// A limit larger than the column type still means "everything".
///
/// Worth being exact about what this does and does not prove, because the
/// first version of it claimed more. `usize::MAX as i64` is -1 and SQLite
/// reads a negative LIMIT as unbounded, so the old cast and the saturating
/// `try_from` return the *same rows* — any ceiling at or above the row count
/// returns all of them. The cast was lossy and made a reader stop; it was
/// never a behaviour bug, and a test asserting that it was would pass for a
/// reason that is not true.
///
/// What is worth guarding is the direction a careless fix would break. Clamping
/// to a smaller type — `u32::try_from(limit).unwrap_or(0)`, say — would turn an
/// enormous limit into no rows at all, which is silent and wrong and is exactly
/// the shape of thing somebody reaches for when a cast looks unsafe.
#[test]
fn an_enormous_limit_asks_for_everything_rather_than_nothing() {
    let (mut store, _dir) = store();

    for n in 0..3 {
        let provenance = Provenance::anonymous(Actor::Claude)
            .with_session(format!("ses_{n}"))
            .with_surface(Surface::Code)
            .with_client(codex());
        write_as(&mut store, &provenance, &format!("task {n}"));
    }

    assert_eq!(
        store.session_clients(usize::MAX).unwrap().len(),
        3,
        "a limit past what the column can hold must not collapse to zero"
    );
    assert_eq!(
        store.session_clients(2).unwrap().len(),
        2,
        "and an ordinary limit still limits"
    );
}
