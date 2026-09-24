//! `scripts/render-breaking-notes.sh`, exercised for real.
//!
//! contracts/BREAKING.md's own header prose says the release's "Breaking"
//! section is generated from the entries below it. Nothing did that
//! (KEEL-299): `release.yml` built its notes from the tag message alone, and
//! nothing ever read this file, so v0.3.0 shipped a removed MCP tool with
//! release notes that never mentioned it — the entry describing it, migration
//! and all, was sitting in `contracts/BREAKING.md` the whole time.
//!
//! `scripts/render-breaking-notes.sh` is the fix, and this file is the half
//! that keeps it from rotting back: without a test, a future edit to the
//! script (or to the file it reads) can stop producing a "Breaking" section
//! and nothing would say so until somebody read a release by hand and noticed
//! a tool was missing from it — exactly the failure this task exists to close.
//!
//! Same pattern as `installer_checksum.rs` and `installer_embedded_checksums.rs`:
//! the mechanism is a shell script because `release.yml`'s publish job has no
//! built `specline` binary for its own platform to call (it merges artifacts
//! built on other runners), so the script is what actually runs at release
//! time and the test is what runs the script for real rather than reimplementing
//! its logic in Rust and testing that instead.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "common/acknowledgements.rs"]
mod acknowledgements;
use acknowledgements::render_breaking_notes;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/specline`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the manifest directory has a grandparent")
        .to_path_buf()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/breaking/{name}"))
}

fn run(fixture_name: &str) -> Output {
    Command::new(repo_root().join("scripts/render-breaking-notes.sh"))
        .arg(fixture(fixture_name))
        .output()
        .expect("the script runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The half that matters: an acknowledged entry has to actually show up in
/// what gets published, migration and all, or this whole mechanism is
/// decoration.
#[test]
fn an_acknowledged_entry_appears_in_the_rendered_notes() {
    let out = run("with-entries.md");
    assert!(out.status.success(), "{}", stderr(&out));
    let notes = stdout(&out);

    assert!(notes.starts_with("## Breaking\n"), "{notes}");
    assert!(
        notes.contains("### tool `specline_note` was removed"),
        "{notes}"
    );
    assert!(
        notes.contains("`specline_note` is gone. Notes are a field on `specline_update` now."),
        "{notes}"
    );
    assert!(
        notes.contains("Migration: none — callers move to `specline_update` with a `notes` field"),
        "{notes}"
    );
    // A second entry has to appear too — this must not silently stop at the
    // first heading.
    assert!(
        notes.contains("### `specline_update` argument `status` is now required"),
        "{notes}"
    );
}

/// A release with nothing breaking gets no section at all — not an empty
/// "## Breaking" heading, which would read as "we checked and something
/// broke" when nothing did.
#[test]
fn no_entries_means_no_section_at_all() {
    let out = run("no-entries.md");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "", "an empty file must render nothing");
}

/// A file whose marker went missing acknowledges nothing — the failing side,
/// same as the gate in classify.rs — rather than rendering the prose above the
/// marker as though it were entries.
#[test]
fn a_missing_marker_renders_nothing() {
    let out = run("no-marker.md");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
}

/// An entry missing its migration is a hand-off nobody finished. The script
/// has to refuse it, loudly, rather than publish a release note with a blank
/// where the migration should be.
#[test]
fn a_malformed_entry_fails_clearly_instead_of_being_dropped() {
    let out = run("malformed.md");
    assert!(
        !out.status.success(),
        "a malformed entry must fail the release rather than ship silently"
    );
    assert_eq!(stdout(&out), "", "nothing partial should reach stdout");
    let err = stderr(&out);
    assert!(
        err.contains("tool `specline_note` was removed"),
        "the error must name which entry is broken: {err}"
    );
    assert!(
        err.to_lowercase().contains("migration") || err.to_lowercase().contains("tells the user"),
        "the error must say what is missing: {err}"
    );
}

/// A file that does not exist is refused rather than read as "no entries" —
/// those are different failures and must not look the same.
#[test]
fn a_missing_file_is_refused() {
    let out = Command::new(repo_root().join("scripts/render-breaking-notes.sh"))
        .arg(fixture("does-not-exist.md"))
        .output()
        .expect("the script runs");

    assert!(!out.status.success());
    assert_eq!(stdout(&out), "");
}

/// The real file in this repository must always render cleanly. If it does
/// not, `release.yml` would fail the next release the same way this test
/// fails right now — better to see it here.
#[test]
fn the_real_breaking_md_renders_without_error() {
    let out = Command::new(repo_root().join("scripts/render-breaking-notes.sh"))
        .arg(repo_root().join("contracts/BREAKING.md"))
        .output()
        .expect("the script runs");

    assert!(
        out.status.success(),
        "contracts/BREAKING.md does not render cleanly: {}",
        stderr(&out)
    );
}

/// The regression this task is actually named for: KEEL-299 was not the
/// script being wrong, it was `release.yml` never calling anything. Every
/// test above would keep passing if someone deleted the two lines that wire
/// the script into the workflow — this is the one that fails instead.
///
/// A string match on the workflow source, not a run of the workflow itself:
/// nothing here can execute `release.yml` (it needs a pushed tag and a
/// self-hosted runner), so this pins the one fact a `cargo test` run can
/// check — that the publish step still reaches for the script and the file —
/// the same way this repo already accepts that its release-only steps are
/// unverifiable end to end (see release.yml's own comments on what running it
/// for real would take).
///
/// This checks the actual invocation, assembled from its two source lines
/// (`render-breaking-notes.sh` is called with the path on the line after it),
/// rather than two independent substrings. Two independent `contains` calls
/// would both still pass if someone left the mention in this file's own
/// header comment ("`render-breaking-notes.sh` reads ... contracts/BREAKING.md")
/// and deleted the call beneath it — release.yml's comments say both words
/// too, on purpose, so a looser check here would not have caught the bug this
/// test exists for.
#[test]
fn release_yml_still_calls_the_renderer_against_breaking_md() {
    let workflow = std::fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("release.yml exists");

    let call = concat!(
        r#"scripts/render-breaking-notes.sh" \"#,
        "\n            ",
        r#""$GITHUB_WORKSPACE/contracts/BREAKING.md")"#,
    );
    assert!(
        workflow.contains(call),
        "release.yml no longer calls scripts/render-breaking-notes.sh against \
         contracts/BREAKING.md the way the publish step is supposed to — the \
         Breaking section will silently stop appearing on releases. Looked for:\n{call}"
    );
}

// --- the script and the Rust renderer must agree ---------------------------
//
// KEEL-299 review: `classify.rs` used to define its own copy of
// `parse_acknowledgements`, which found the marker with `str::split_once` — a
// substring search — while this script matches only a whole line. Two parsers
// of the same file, reading the same real contracts/BREAKING.md differently,
// and nothing said so because nothing ever ran them against each other. Both
// now go through `crates/specline/tests/common/acknowledgements.rs`, and this
// is the test that makes disagreement between the shell and Rust sides visible
// rather than assumed away by "the shared module says so".

/// A CRLF line ending on the marker line must not hide it. Windows editors
/// leave these, and the marker existing at all should not depend on which
/// editor wrote the file.
#[test]
fn a_crlf_marker_line_is_still_found() {
    let out = run("crlf.md");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("### tool `x` was removed"),
        "{}",
        stdout(&out)
    );
}

/// Trailing whitespace after the marker — a stray space nobody meant to
/// type — must not hide it either.
#[test]
fn a_trailing_space_after_the_marker_is_still_found() {
    let out = run("trailing-whitespace-marker.md");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("### tool `y` was removed"),
        "{}",
        stdout(&out)
    );
}

/// The marker mentioned inline, in prose, above the real one — exactly what
/// contracts/BREAKING.md itself does — must not be mistaken for it. This is
/// the fixture that would have failed before classify.rs's parser switched
/// from a substring search to a whole-line match.
#[test]
fn a_marker_mentioned_inline_in_prose_is_not_mistaken_for_the_real_one() {
    let out = run("inline-mention.md");
    assert!(out.status.success(), "{}", stderr(&out));
    let notes = stdout(&out);
    assert!(notes.contains("### tool `v` was removed"), "{notes}");
    assert!(
        !notes.contains("How to add one"),
        "the prose heading above the real marker must never be read as an entry: {notes}"
    );
}

/// A field line with nothing above it to attach to is dropped, not guessed
/// at — it must not silently become part of the next real entry either.
#[test]
fn an_orphan_field_before_any_heading_is_dropped() {
    let out = run("orphan-field.md");
    assert!(out.status.success(), "{}", stderr(&out));
    let notes = stdout(&out);
    assert!(notes.contains("### tool `z` was removed"), "{notes}");
    assert!(
        notes.contains("Migration: none") && !notes.contains("this line has no heading"),
        "the orphan field must not leak into the real entry's migration: {notes}"
    );
}

/// This format has no line-continuation syntax. A second line under a field
/// is dropped rather than appended, and both sides must agree on that rather
/// than one of them silently growing the field.
#[test]
fn a_multiline_looking_field_keeps_only_its_first_line() {
    let out = run("multiline-field.md");
    assert!(out.status.success(), "{}", stderr(&out));
    let notes = stdout(&out);
    assert!(notes.contains("Migration: none"), "{notes}");
    assert!(
        !notes.contains("continuation line"),
        "a second line under a field must not be appended to it: {notes}"
    );
}

fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("fixture reads")
}

/// The actual point of the shared module: for every fixture this file knows
/// about, and for the real `contracts/BREAKING.md`, the shell script's stdout
/// must equal what `render_breaking_notes` (the Rust twin, in
/// `tests/common/acknowledgements.rs`) computes from the same bytes — and the
/// two must agree on *whether* it succeeds, not only on what it prints when it
/// does.
#[test]
fn the_script_and_the_rust_renderer_agree() {
    let fixtures = [
        "with-entries.md",
        "no-entries.md",
        "no-marker.md",
        "malformed.md",
        "crlf.md",
        "trailing-whitespace-marker.md",
        "inline-mention.md",
        "orphan-field.md",
        "multiline-field.md",
    ];

    for name in fixtures {
        let out = run(name);
        let rust_result = render_breaking_notes(&read_fixture(name));

        match rust_result {
            Ok(rust_notes) => {
                assert!(
                    out.status.success(),
                    "{name}: the script failed but the Rust renderer succeeded: {}",
                    stderr(&out)
                );
                assert_eq!(
                    stdout(&out),
                    rust_notes,
                    "{name}: the script and the Rust renderer disagree on the rendered notes"
                );
            }
            Err(rust_err) => {
                assert!(
                    !out.status.success(),
                    "{name}: the Rust renderer refused ({rust_err}) but the script \
                     succeeded with: {}",
                    stdout(&out)
                );
            }
        }
    }

    // The real file, separately: it is not one of the checked-in fixtures, and
    // it is the one that actually ships in a release.
    let out = Command::new(repo_root().join("scripts/render-breaking-notes.sh"))
        .arg(repo_root().join("contracts/BREAKING.md"))
        .output()
        .expect("the script runs");
    let real_text =
        std::fs::read_to_string(repo_root().join("contracts/BREAKING.md")).expect("file reads");
    match render_breaking_notes(&real_text) {
        Ok(rust_notes) => {
            assert!(out.status.success(), "{}", stderr(&out));
            assert_eq!(stdout(&out), rust_notes);
        }
        Err(rust_err) => panic!("contracts/BREAKING.md does not render in Rust either: {rust_err}"),
    }
}
