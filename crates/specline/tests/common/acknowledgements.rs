//! Parsing and rendering `contracts/BREAKING.md`, shared between
//! `classify.rs` (the acknowledgement gate) and `render_breaking_notes.rs`
//! (the release-time renderer, `scripts/render-breaking-notes.sh`'s Rust
//! twin).
//!
//! # Why this is shared rather than duplicated
//!
//! It used to be duplicated, and the two copies disagreed about where a file's
//! entries begin. `classify.rs` found the marker with `str::split_once`, which
//! matches the **first substring** anywhere in the text; `contracts/BREAKING.md`
//! mentions the marker inline, in prose, above the real one — "Everything
//! above `<!-- acknowledgements -->` is instructions" — so `classify.rs` split
//! at line 44 and treated the real entries between there and line 68 as
//! prose, silently. `render-breaking-notes.sh` matched a **whole line**, so it
//! split at line 68 and read the real entries. Two tools reading the same file
//! and agreeing with neither each other nor a human glancing at it (KEEL-299
//! review).
//!
//! One definition now: the marker is a line whose trimmed content equals
//! [`ENTRY_MARKER`] exactly, and entries are what follows the **first** such
//! line. `render_breaking_notes.rs`'s `the_script_and_the_rust_renderer_agree`
//! test runs the shell script and this module against the same fixtures and
//! asserts their outputs match, which is what keeps a future edit to either
//! side from drifting back apart unnoticed.

#![allow(dead_code)]

/// Where the prose stops and the entries begin in `contracts/BREAKING.md`.
///
/// Matched as a whole line, after trimming — never as a substring anywhere in
/// the text. See the module docs for why that distinction is the whole point.
pub const ENTRY_MARKER: &str = "<!-- acknowledgements -->";

/// One acknowledged breaking change, parsed from `contracts/BREAKING.md`.
#[derive(Debug, PartialEq, Eq)]
pub struct Acknowledgement {
    pub what: String,
    pub migration: String,
    pub tells_the_user: String,
}

/// Parse the acknowledgement file.
///
/// Markdown rather than TOML or JSON, because the two fields that matter are
/// prose a human writes for another human, and a format that makes prose
/// awkward gets prose that is awkward. The shape is fixed enough to check:
///
/// ```text
/// ## <the difference, quoted exactly as the classifier reports it>
/// - migration: <what handles it, or `none` and why that is alright>
/// - tells the user: <the sentence they will actually read>
/// ```
///
/// `str::lines()` already treats `\r\n` and `\n` alike, so a file with CRLF
/// endings — as a Windows editor might leave one — parses the same as one
/// with `\n` alone. Nothing here trims trailing whitespace on the marker line
/// specially; ordinary `.trim()` in the comparison already covers a stray
/// trailing space the same way it covers a stray `\r`.
pub fn parse_acknowledgements(text: &str) -> Vec<Acknowledgement> {
    // Everything before the **first line that is exactly the marker** is
    // instructions for a human, and its headings — and any inline mention of
    // the marker itself, in prose — are not entries.
    let mut after_marker: Vec<&str> = Vec::new();
    let mut found = false;
    for line in text.lines() {
        if found {
            after_marker.push(line);
        } else if line.trim() == ENTRY_MARKER {
            found = true;
        }
    }
    if !found {
        return Vec::new();
    }

    let mut out: Vec<Acknowledgement> = Vec::new();
    for line in after_marker {
        let line = line.trim();
        if let Some(what) = line.strip_prefix("## ") {
            out.push(Acknowledgement {
                what: what.trim().to_owned(),
                migration: String::new(),
                tells_the_user: String::new(),
            });
        // A field line before any heading has nothing to attach to, and is
        // dropped rather than guessed at. A second line under a field that is
        // not itself a recognised prefix — a "multi-line" continuation of a
        // migration or a user sentence — is dropped the same way: this format
        // has no continuation syntax, so a field is whatever fit on its own
        // line, and anything after it is silently not part of the entry.
        } else if let Some(v) = line.strip_prefix("- migration:")
            && let Some(last) = out.last_mut()
        {
            last.migration = v.trim().to_owned();
        } else if let Some(v) = line.strip_prefix("- tells the user:")
            && let Some(last) = out.last_mut()
        {
            last.tells_the_user = v.trim().to_owned();
        }
    }
    out
}

/// The Breaking section of the release notes, built from the entries.
///
/// This is the payoff. Notes assembled by hand from a week of commits are how
/// a breaking change reaches users unannounced; notes built from the entries
/// that already had to be written down to describe the change cannot forget
/// one.
///
/// A release with nothing breaking gets no section at all, not an empty
/// "## Breaking" heading — the latter reads as "we checked, and something
/// broke" when nothing did.
pub fn breaking_section(acknowledgements: &[Acknowledgement]) -> String {
    if acknowledgements.is_empty() {
        return String::new();
    }
    let mut out = String::from("## Breaking\n");
    for a in acknowledgements {
        out.push_str(&format!(
            "\n### {}\n\n{}\n\nMigration: {}\n",
            a.what, a.tells_the_user, a.migration
        ));
    }
    out
}

/// Render the notes, or say which entry stopped it.
///
/// This is `render-breaking-notes.sh`'s Rust twin, checked for agreement by
/// `render_breaking_notes.rs`'s `the_script_and_the_rust_renderer_agree`: an
/// entry missing its migration or its "tells the user" field is a hand-off
/// nobody finished, and both sides refuse rather than publish half of it. Like
/// the script, this stops at the *first* bad entry in file order rather than
/// collecting every problem — `classify.rs`'s `gate` is the function that
/// reports all of them at once, for the merge-time check; this one is for the
/// release-time render, which only needs to know whether it can proceed.
pub fn render_breaking_notes(text: &str) -> Result<String, String> {
    let acknowledgements = parse_acknowledgements(text);
    for a in &acknowledgements {
        if a.migration.is_empty() || a.tells_the_user.is_empty() {
            return Err(format!(
                "entry '{}' is missing its migration or \"tells the user\" field",
                a.what
            ));
        }
    }
    Ok(breaking_section(&acknowledgements))
}
