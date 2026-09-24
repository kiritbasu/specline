//! The work verbs, driven as a person drives them.
//!
//! `ready`, `claim`, `close` and `generate` are the loop this project is built
//! around, and none of them had a test at the CLI layer. The functions
//! underneath are covered; what was not is everything between a shell and them
//! — argument wiring, the daemon-versus-direct decision, exit codes, and
//! whether the output says anything a person could act on.
//!
//! The binary is invoked directly rather than through `assert_cmd`, which the
//! task suggested. Cargo already hands the path over at compile time in
//! `CARGO_BIN_EXE_specline`, so the dependency would buy assertion sugar and
//! nothing else, and scale discipline says a crate has to earn its place.
//!
//! # Two flags on every invocation, and why
//!
//! `--daemon` points at a closed port and every write carries `--force`.
//! Without both, these tests would find whatever daemon the developer running
//! them happens to have up — reading and writing *their* store instead of the
//! temporary one. That is not a theoretical tidiness: the daemon answers reads
//! for a different home entirely, so the failure would be a test that passes
//! against the wrong data.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

/// A port nothing is listening on, so every command takes the direct path.
const NO_DAEMON: &str = "http://127.0.0.1:1";

struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Run {
    fn expect_ok(self, what: &str) -> String {
        assert!(
            self.ok,
            "{what} failed\nstdout: {}\nstderr: {}",
            self.stdout, self.stderr
        );
        self.stdout
    }

    fn expect_failure(self, what: &str) -> String {
        assert!(
            !self.ok,
            "{what} should have failed but succeeded\nstdout: {}",
            self.stdout
        );
        format!("{}{}", self.stdout, self.stderr)
    }
}

fn specline_with_session(home: &Path, session: Option<&str>, args: &[&str]) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_specline"));
    command
        .arg("--home")
        .arg(home)
        .args(args)
        // Run from the test's own home, not the repository: `specline doctor`
        // reads the Claude Code settings and asks `gh` about the branch it is
        // standing in (KEEL-396, KEEL-409), and neither may be this machine's.
        .current_dir(home)
        .env("CLAUDE_CONFIG_DIR", home)
        .env_remove("SPECLINE_DAEMON_URL")
        .env_remove("SPECLINE_HOME");
    match session {
        Some(s) => command.env("SPECLINE_SESSION", s),
        None => command.env_remove("SPECLINE_SESSION"),
    };
    let output = command.output().expect("run the specline binary");
    Run {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn specline(home: &Path, args: &[&str]) -> Run {
    specline_with_session(home, Some("ses_cli_verbs"), args)
}

/// The same, with the Inbox switched on.
///
/// Set on the child process rather than on this one: `std::env::set_var` is
/// `unsafe` in edition 2024 and racy across parallel tests either way, and the
/// thing under test is a subprocess, so its environment is ours to compose.
/// Switched on explicitly so these tests keep saying which configuration they
/// exercise rather than inheriting whatever the default happens to be.
fn specline_with_inbox(home: &Path, args: &[&str]) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_specline"));
    command
        .arg("--home")
        .arg(home)
        .args(args)
        .env("SPECLINE_INBOX", "1")
        .env("SPECLINE_SESSION", "ses_cli_verbs")
        .current_dir(home)
        .env("CLAUDE_CONFIG_DIR", home)
        .env_remove("SPECLINE_DAEMON_URL")
        .env_remove("SPECLINE_HOME");
    let output = command.output().expect("run the specline binary");
    Run {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// A home holding the fixture corpus, plus one task of this test's own.
///
/// The fixture rather than a hand-built store, because it is the corpus the CLI
/// is meant to be usable against and because there is no `create project` verb
/// — which is itself worth knowing: every project in existence arrives through
/// MCP, `bootstrap` or `fixture`.
fn seeded() -> (tempfile::TempDir, String) {
    let home = tempfile::tempdir().unwrap();
    specline(home.path(), &["--force", "fixture"]).expect_ok("load the fixture");

    let created = specline(
        home.path(),
        &[
            "--force",
            "--json",
            "task",
            "--project",
            "harbour",
            "Wire up the thing",
            "--body",
            "A task the CLI verb tests move through its whole lifecycle.",
        ],
    )
    .expect_ok("create a task");

    let value: serde_json::Value = serde_json::from_str(&created).expect("--json should be json");
    let id = value
        .pointer("/entity/id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("no id in the create output: {created}"))
        .to_owned();

    (home, id)
}

#[test]
fn ready_lists_work_and_says_why_it_is_first() {
    let (home, _task) = seeded();

    let out = specline(
        home.path(),
        &["ready", "harbour", "--limit", "50", "--daemon", NO_DAEMON],
    )
    .expect_ok("specline ready");

    assert!(
        out.contains("Wire up the thing"),
        "ready should name the work: {out}"
    );
    // The reason used to read "nothing is blocking it", which was true of every
    // row on a list where being unblocked is the entry condition — so it said
    // the same thing about all of them (B-83). It now names the phase, the kind
    // or how long the task has waited.
    assert!(
        out.contains("in no phase") || out.contains("in an active phase") || out.contains("a bug"),
        "and say why it is where it is, in terms that differ between rows: {out}"
    );
}

/// A cut list says it was cut, with a total. Hard constraint 4, at the surface
/// a person actually reads.
#[test]
fn a_truncated_ready_list_says_how_much_it_left_out() {
    let (home, _task) = seeded();

    let out = specline(
        home.path(),
        &["ready", "harbour", "--limit", "3", "--daemon", NO_DAEMON],
    )
    .expect_ok("specline ready --limit 3");

    assert!(
        out.contains("3 of 16 shown"),
        "a cut list that does not say how much it left out reads as the whole list: {out}"
    );
}

#[test]
fn ready_against_a_project_that_does_not_exist_says_so() {
    let (home, _task) = seeded();

    let out = specline(home.path(), &["ready", "nope", "--daemon", NO_DAEMON])
        .expect_failure("specline ready against a missing project");

    assert!(
        out.contains("nope"),
        "the refusal should name what was asked for: {out}"
    );
}

/// A claim has to name a session and Specline never invents one, so the failure
/// case here is the more interesting half.
#[test]
fn claim_records_the_session_and_refuses_without_one() {
    let (home, task) = seeded();

    let out = specline(
        home.path(),
        &["--force", "claim", &task, "--daemon", NO_DAEMON],
    )
    .expect_ok("specline claim with SPECLINE_SESSION set");
    assert!(out.contains("claimed"), "{out}");

    // `SPECLINE_SESSION` removed rather than blanked: an empty variable and an
    // absent one are different arguments to clap, and only one of them is what
    // a fresh shell looks like.
    let out = specline_with_session(
        home.path(),
        None,
        &["--force", "claim", &task, "--daemon", NO_DAEMON],
    )
    .expect_failure("a claim naming nobody");
    assert!(
        out.contains("--session") || out.contains("SPECLINE_SESSION"),
        "and say how to fix it: {out}"
    );
}

/// `done` needs evidence. The storage layer enforces it; this asserts the CLI
/// carries the refusal through rather than swallowing it into an exit code
/// nobody can read.
#[test]
fn close_needs_evidence_for_done_and_takes_it_when_given() {
    let (home, task) = seeded();

    let refused = specline(
        home.path(),
        &[
            "--force",
            "close",
            &task,
            "--reason",
            "done",
            "-m",
            "Finished it.",
            "--daemon",
            NO_DAEMON,
        ],
    )
    .expect_failure("close as done with no evidence");
    assert!(
        refused.contains("evidence"),
        "the refusal has to say what is missing: {refused}"
    );

    let closed = specline(
        home.path(),
        &[
            "--force",
            "close",
            &task,
            "--reason",
            "done",
            "-m",
            "Wired it up and tested it.",
            "--evidence",
            "commit:0000000",
            "--daemon",
            NO_DAEMON,
        ],
    )
    .expect_ok("close as done with evidence");
    assert!(closed.contains("done"), "{closed}");

    // And it is actually closed, not merely reported as such.
    let ready = specline(
        home.path(),
        &["ready", "harbour", "--limit", "50", "--daemon", NO_DAEMON],
    )
    .expect_ok("specline ready after the close");
    assert!(
        !ready.contains("Wire up the thing"),
        "a closed task should not still be offered as work: {ready}"
    );
}

/// An unknown reason is refused with the real ones listed.
///
/// This is the case `product/CLAUDE.md` records going wrong from the other
/// side: the contract said to use a status that never existed, and a session
/// following it literally got an enum rejection listing five values, none of
/// them the word it had been told to use.
#[test]
fn close_with_a_reason_that_is_not_one_lists_the_real_ones() {
    let (home, task) = seeded();

    let out = specline(
        home.path(),
        &[
            "--force",
            "close",
            &task,
            "--reason",
            "dropped",
            "-m",
            "Not doing it.",
            "--daemon",
            NO_DAEMON,
        ],
    )
    .expect_failure("close with an invented reason");

    assert!(
        out.contains("wont_do") && out.contains("superseded"),
        "the refusal should list what is valid: {out}"
    );
}

#[test]
fn generate_writes_the_mirror_and_check_agrees_afterwards() {
    let (home, _task) = seeded();
    let repo = tempfile::tempdir().unwrap();
    let repo_path = repo.path().to_str().unwrap().to_owned();

    let out = specline(
        home.path(),
        &[
            "generate", "harbour", "--repo", &repo_path, "--daemon", NO_DAEMON,
        ],
    )
    .expect_ok("specline generate");
    assert!(
        out.contains("wrote") || out.contains("written") || out.contains("file"),
        "generate should say what it did: {out}"
    );
    assert!(
        repo.path().join(".specline/manifest.json").is_file(),
        "generate should have written the mirror manifest"
    );

    // The second run has nothing to do, and `--check` agrees. A generator whose
    // output is not stable across two runs makes the pre-commit hook cry wolf,
    // which is how a check ends up bypassed with --no-verify.
    specline(
        home.path(),
        &[
            "generate", "harbour", "--repo", &repo_path, "--check", "--daemon", NO_DAEMON,
        ],
    )
    .expect_ok("specline generate --check straight after a generate");
}

/// `--check` against a hand-edited file fails, which is the whole reason the
/// pre-commit hook can be trusted.
#[test]
fn generate_check_notices_a_hand_edit() {
    let (home, _task) = seeded();
    let repo = tempfile::tempdir().unwrap();
    let repo_path = repo.path().to_str().unwrap().to_owned();
    let args = [
        "generate", "harbour", "--repo", &repo_path, "--daemon", NO_DAEMON,
    ];
    specline(home.path(), &args).expect_ok("specline generate");

    let glossary = repo.path().join(".specline/glossary.md");
    assert!(glossary.is_file(), "the fixture should produce a glossary");
    std::fs::write(&glossary, "I edited this by hand.\n").unwrap();

    let mut check: Vec<&str> = args.to_vec();
    check.push("--check");
    let out = specline(home.path(), &check).expect_failure("--check over a hand-edited file");
    assert!(
        out.contains("glossary"),
        "the report should name the file that differs: {out}"
    );
}

/// Every command that takes `--daemon` defaults to the same place.
///
/// `specline migrate` shipped with `7171` where everything else has `7654`, which
/// is worse than a cosmetic slip: a migrate pointed at a port nothing is
/// listening on concludes no daemon is running and changes the schema under one
/// that is. That is the exact failure the command exists to prevent, arriving
/// through a typo in its own default.
#[test]
fn every_daemon_flag_points_at_the_same_daemon() {
    let source = include_str!("../src/main.rs");

    let mut defaults: Vec<&str> = source
        .match_indices("default_value = \"http://")
        .filter_map(|(at, _)| {
            let rest = &source[at + "default_value = \"".len()..];
            rest.find('"').map(|end| &rest[..end])
        })
        .collect();
    defaults.sort_unstable();
    defaults.dedup();

    assert_eq!(
        defaults.len(),
        1,
        "the CLI has more than one idea of where the daemon lives: {defaults:?}"
    );
}

/// The quiet half of the daemonless bug, which nothing here caught.
///
/// `ready` broke loudly — it printed "nothing ready" with work in the store —
/// and two tests above fail if the envelope leaks back into that path. `claim`
/// and `close` broke quietly: their renderers fall back to the argument when
/// `reference` is missing, so the line still read like a success while naming
/// the ULID that was typed rather than the task that was claimed.
///
/// Removing the payload unwrap from the *write* path left all nine tests here
/// green, which is what these two are for. The ULID is passed deliberately:
/// a readable `HARB-n` in the output can only have been read off the response,
/// so neither half of the line can be the argument echoed back.
#[test]
fn claim_without_a_daemon_names_the_reference_and_not_the_ulid() {
    let (home, task) = seeded();

    let out = specline(
        home.path(),
        &["--force", "claim", &task, "--daemon", NO_DAEMON],
    )
    .expect_ok("specline claim on the daemonless path");

    assert!(
        out.contains("HARB-"),
        "the reference has to come from the response: {out}"
    );
    assert!(
        out.contains("Wire up the thing"),
        "and so does the title: {out}"
    );
    assert!(
        !out.contains(&task),
        "the raw id being echoed means the renderer fell back to its argument: {out}"
    );
}

#[test]
fn close_without_a_daemon_reports_the_reference_it_closed() {
    let (home, task) = seeded();

    let out = specline(
        home.path(),
        &[
            "--force",
            "close",
            &task,
            "--reason",
            "done",
            "--message",
            "Closed by a test, to check what the daemonless path prints.",
            "--evidence",
            "test:cargo test -p specline --test verbs",
            "--daemon",
            NO_DAEMON,
        ],
    )
    .expect_ok("specline close on the daemonless path");

    assert!(
        out.contains("HARB-"),
        "the reference has to come from the response: {out}"
    );
    assert!(
        !out.contains(&task),
        "the raw id being echoed means the renderer fell back to its argument: {out}"
    );
}

/// KEEL-137. A read that falls back to the store because no daemon answered
/// must not *make* the store on its way past.
///
/// `Store::open` creates and migrates when the file is absent, which is right
/// for the two commands asked to produce a store and wrong for a read. The
/// visible failure was the second half: an empty store answers "no project
/// matches specline. Expected: one of: " — blaming the project name, with the empty
/// list as the only clue that there was nothing to match against.
#[test]
fn a_read_against_a_home_with_no_store_says_so_and_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    let store = home.path().join("specline.sqlite");

    let out = specline(home.path(), &["ready", "specline", "--daemon", NO_DAEMON])
        .expect_failure("a read cannot answer from a store that does not exist");

    assert!(
        out.contains("no Specline store"),
        "the error must name the missing store rather than the project: {out}"
    );
    assert!(
        out.contains(&store.display().to_string()),
        "and say which path it looked at: {out}"
    );
    assert!(
        out.contains("specline bootstrap"),
        "and how to make one: {out}"
    );
    assert!(
        !store.exists(),
        "a read must leave no store behind in a directory nobody asked it to write to"
    );
}

/// The other half, which is what stops the fix from being a regression: the two
/// commands whose job is to make a store still make one.
#[test]
fn the_commands_that_are_asked_to_make_a_store_still_do() {
    let home = tempfile::tempdir().unwrap();
    specline(home.path(), &["--force", "fixture"]).expect_ok("fixture makes a store");
    assert!(home.path().join("specline.sqlite").exists());
}

/// KEEL-220. `reembed` is the one command whose whole job is the model, so a
/// build without one has to say so rather than fail obscurely or succeed
/// having done nothing.
///
/// Only meaningful in the build that has no model — and running the real thing
/// in the other configuration would download 133 MB, which is not a test.
#[cfg(not(feature = "embeddings"))]
#[test]
fn reembed_in_a_build_with_no_model_says_that_is_why() {
    let (home, _task) = seeded();

    let out = specline(home.path(), &["--force", "reembed", "--missing"])
        .expect_failure("a build with no embedder cannot re-embed");

    assert!(
        out.contains("no embedding model"),
        "the refusal has to name the cause: {out}"
    );
    assert!(
        out.to_lowercase().contains("keyword search"),
        "and say what still works, because it is most of what search does: {out}"
    );
    assert!(
        out.contains("specline doctor"),
        "and where to find out which build this is: {out}"
    );
}

/// `specline doctor` reports the capability either way, because "none of your
/// documents has a vector" reads as something to fix and on a build with no
/// model it is not.
#[test]
fn doctor_says_which_build_this_is_where_embeddings_are_concerned() {
    let (home, _task) = seeded();

    let out =
        specline(home.path(), &["doctor", "--daemon", NO_DAEMON]).expect_ok("specline doctor");

    assert!(out.contains("embeddings"), "{out}");
    if cfg!(feature = "embeddings") {
        assert!(
            !out.contains("not built into this binary"),
            "a build that has a model must not claim otherwise: {out}"
        );
    } else {
        assert!(
            out.contains("not built into this binary"),
            "and one that does not must say so rather than reporting an empty corpus: {out}"
        );
    }
}

// --- Filing a signal (KEEL-319) -------------------------------------------
//
// The Inbox's write path, driven the way a person drives it. Worth testing at
// this layer specifically because everything interesting about a signal is
// what it is *not* — not a task, not on the board, not in `next` — and none of
// that is visible from the function underneath.

#[test]
fn a_signal_is_filed_with_only_the_thing_that_was_said() {
    let (home, _task) = seeded();

    let out = specline_with_inbox(
        home.path(),
        &[
            "--force",
            "signal",
            "--project",
            "harbour",
            "this should work with codex",
        ],
    )
    .expect_ok("file a signal");

    assert!(out.contains("fbk_"), "a signal gets a feedback id: {out}");
    // The line has to say where it went, because somebody who has just filed
    // one will otherwise look for it on the board and conclude it was lost.
    assert!(
        out.contains("Inbox") && out.contains("untriaged"),
        "the confirmation should say where it landed: {out}"
    );
}

#[test]
fn a_signal_keeps_who_said_it_and_when() {
    let (home, _task) = seeded();

    let out = specline_with_inbox(
        home.path(),
        &[
            "--force",
            "--json",
            "signal",
            "--project",
            "harbour",
            "this should work with codex",
            "--source",
            "Madhu",
            "--kind",
            "idea",
            "--occurred-at",
            "2026-08-18",
        ],
    )
    .expect_ok("file a sourced signal");

    let value: serde_json::Value = serde_json::from_str(&out).expect("--json should be json");
    assert_eq!(value.get("source"), Some(&serde_json::json!("Madhu")));
    assert_eq!(value.get("kind"), Some(&serde_json::json!("idea")));
    assert_eq!(value.get("triaged"), Some(&serde_json::json!(false)));
    assert!(
        value
            .get("occurred_at")
            .and_then(|v| v.as_str())
            .is_some_and(|t| t.starts_with("2026-08-18")),
        "a bare date becomes midnight UTC rather than being refused: {out}"
    );
}

/// The failure cases. Both are things a person will actually type, and both
/// used to be the difference between a signal and a lost one.
#[test]
fn a_signal_refuses_a_time_it_cannot_read_and_says_what_it_would_take() {
    let (home, _task) = seeded();

    let out = specline_with_inbox(
        home.path(),
        &[
            "--force",
            "signal",
            "--project",
            "harbour",
            "somebody said a thing",
            "--occurred-at",
            "last tuesday",
        ],
    )
    .expect_failure("`last tuesday` is not a date");

    assert!(
        out.contains("2026-08-18"),
        "the refusal should show a shape that would work: {out}"
    );
}

#[test]
fn a_signal_refuses_a_kind_that_is_not_one_and_lists_the_ones_that_are() {
    let (home, _task) = seeded();

    let out = specline_with_inbox(
        home.path(),
        &[
            "--force",
            "signal",
            "--project",
            "harbour",
            "somebody said a thing",
            "--kind",
            "wish",
        ],
    )
    .expect_failure("`wish` is not a feedback kind");

    assert!(
        out.contains("observation") && out.contains("competitor"),
        "the refusal should list the values that exist: {out}"
    );
}

/// Off by default, and the refusal has to name the variable.
///
/// Somebody who typed `specline signal` meant to file one. "Unknown command"
/// or a bare "no" would send them looking for a build that has the feature,
/// when what they need is one word (KEEL-341).
#[test]
fn filing_a_signal_is_refused_while_the_inbox_is_switched_off() {
    let (home, _task) = seeded();

    let out = specline(
        home.path(),
        &["--force", "signal", "--project", "harbour", "a thing said"],
    )
    .expect_failure("the Inbox is off by default");

    assert!(out.contains("SPECLINE_INBOX"), "{out}");
    assert!(
        out.contains("loses nothing"),
        "and it should say that switching it on is safe, or nobody will: {out}"
    );
}

#[test]
fn triaging_is_refused_while_the_inbox_is_switched_off() {
    let (home, _task) = seeded();

    let out = specline(
        home.path(),
        &[
            "--force",
            "triage",
            "fbk_01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "--set-down",
            "no longer wanted",
        ],
    )
    .expect_failure("the Inbox is off by default");
    assert!(out.contains("SPECLINE_INBOX"), "{out}");
}
