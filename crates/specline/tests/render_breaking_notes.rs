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
#[test]
fn release_yml_still_calls_the_renderer_against_breaking_md() {
    let workflow = std::fs::read_to_string(repo_root().join(".github/workflows/release.yml"))
        .expect("release.yml exists");

    assert!(
        workflow.contains("scripts/render-breaking-notes.sh"),
        "release.yml no longer calls the script that renders contracts/BREAKING.md \
         into the release notes — the Breaking section will silently stop appearing"
    );
    assert!(
        workflow.contains("contracts/BREAKING.md"),
        "release.yml no longer points the renderer at contracts/BREAKING.md"
    );
}
