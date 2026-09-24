#!/usr/bin/env bash
#
# Render the "Breaking" section of a release's notes from the acknowledged
# entries in contracts/BREAKING.md.
#
# ## Why this exists
#
# contracts/BREAKING.md says, in its own header prose, that "the Breaking
# section of the release notes is generated from the entries below." Nothing
# did that (KEEL-299): release.yml built its notes from the tag message alone,
# and v0.3.0 shipped a removed MCP tool with release notes that never
# mentioned it, even though the entry describing it — migration and all — was
# sitting in this file the whole time. This script is what closes that gap:
# release.yml calls it and prepends what it prints to the notes it already
# publishes.
#
# ## What it does
#
# Takes the acknowledgement file's path as `$1` (default `contracts/BREAKING.md`,
# resolved against the working directory), and prints the "## Breaking"
# section built from every entry under its `<!-- acknowledgements -->` marker.
#
#   - No marker anywhere in the file: prints nothing and exits 0. A file with
#     no marker acknowledges nothing — the failing side, deliberately, so that
#     renaming the marker cannot silently wave every breaking change through.
#   - The marker text appears somewhere, but never as a line of its own once
#     trailing whitespace and a `\r` are trimmed: refuses rather than guessing
#     which occurrence was meant. `contracts/BREAKING.md` mentions the marker
#     inline, in prose, above the real one, so a plain substring search would
#     have stopped there instead of at the real one — this is the failure mode
#     `the_marker_mentioned_only_inline_is_refused_not_guessed_at` in
#     crates/specline/tests/render_breaking_notes.rs pins.
#   - Nothing under a real marker line: prints nothing and exits 0, same as no
#     marker at all.
#   - An entry missing its `migration` or `tells the user` field: refuses,
#     with a message naming the entry, and exits non-zero. A half-finished
#     acknowledgement failing loudly here is the whole point — the alternative
#     is a release that quietly ships without the sentence someone meant to
#     write.
#
# ## Agreeing with the Rust side
#
# `crates/specline/tests/common/acknowledgements.rs` parses this exact file
# shape in Rust, for `classify.rs`'s acknowledgement gate and for this script's
# own test. The two are not shared code — this runs in a release job with no
# built `specline` binary for its own platform, the other is exercised by
# `cargo test` — so they used to disagree about where entries start (see that
# module's doc comment for the bug this caused). What keeps them from drifting
# apart now is `crates/specline/tests/render_breaking_notes.rs`'s
# `the_script_and_the_rust_renderer_agree`, which runs both against the same
# fixtures — including a CRLF file, a marker with trailing whitespace, and the
# real contracts/BREAKING.md — and asserts their output is identical, the same
# way `installer_checksum.rs` pins `scripts/patch-installer.sh`.
#
# ## The marker, again
#
# Everything above `<!-- acknowledgements -->` is instructions for a human
# writing an entry, and its headings — including "## How to add one" — are not
# entries. That boundary already burned this file's Rust sibling once (see
# classify.rs's `prose_headings_before_the_marker_are_not_entries`), so it is
# kept explicit here too rather than inferred from heading levels.

set -euo pipefail

file="${1:-contracts/BREAKING.md}"
marker='<!-- acknowledgements -->'

if [ ! -f "$file" ]; then
    echo "render-breaking-notes: no such file: $file" >&2
    exit 1
fi

# A file with no marker anywhere acknowledges nothing — the failing side on
# purpose. This is a plain substring search, deliberately looser than the
# line-exact match below: it only decides "is the marker text present at all,
# in any form", so that the stricter check after it can tell "genuinely
# absent" apart from "present, but not readably so".
if ! grep -qF "$marker" "$file"; then
    exit 0
fi

# Split out into a real file rather than a shell variable: the entries are
# somebody's prose, not code, and command substitution or a heredoc happily
# evaluates a stray `$(...)` inside prose it never should have looked at.
#
# The marker line itself is matched after stripping a trailing `\r` (a file
# with CRLF endings, as a Windows editor might leave one) and surrounding
# whitespace (a trailing space nobody meant to type) — `rest` starts printing
# only once that trimmed line equals the marker exactly, never on a substring
# match, so the marker mentioned inline in this file's own prose ("Everything
# above `<!-- acknowledgements -->` is instructions...") is not mistaken for
# the real one below it.
entries_file="$(mktemp)"
trap 'rm -f "$entries_file"' EXIT
if ! awk -v marker="$marker" '
    {
        line = $0
        sub(/\r$/, "", line)
        gsub(/^[ \t]+|[ \t]+$/, "", line)
        if (found) { print }
        if (line == marker) { found = 1 }
    }
    END { exit(found ? 0 : 1) }
' "$file" > "$entries_file"; then
    echo "render-breaking-notes: found '$marker' in $file, but not as a line of its own" >&2
    echo "(after trimming whitespace and any trailing carriage return) — refusing to guess" >&2
    echo "which occurrence marks where entries begin." >&2
    exit 1
fi

if ! grep -q '[^[:space:]]' "$entries_file"; then
    exit 0
fi

# Trim leading and trailing whitespace. A parameter-expansion idiom rather
# than shelling out to `sed` per line — cheaper, though each call below still
# goes through a command-substitution subshell to capture its result, the same
# cost a `sed` call would have paid.
trim() {
    local s="$1"
    s="${s#"${s%%[![:space:]]*}"}"
    s="${s%"${s##*[![:space:]]}"}"
    printf '%s' "$s"
}

what=""
migration=""
tells=""
count=0
out=""

flush() {
    [ -n "$what" ] || return 0
    if [ -z "$migration" ] || [ -z "$tells" ]; then
        echo "render-breaking-notes: entry '$what' is missing its migration or \"tells the user\" field" >&2
        exit 1
    fi
    count=$((count + 1))
    out="${out}
### ${what}

${tells}

Migration: ${migration}
"
}

while IFS= read -r raw_line || [ -n "$raw_line" ]; do
    line="$(trim "$raw_line")"
    case "$line" in
        "## "*)
            flush
            what="$(trim "${line#\#\# }")"
            migration=""
            tells=""
            ;;
        "- migration:"*)
            # A field line before any heading has nothing to attach to, and is
            # dropped rather than guessed at — same rule as
            # `parse_acknowledgements` in
            # crates/specline/tests/common/acknowledgements.rs. A second line
            # under a field that matches neither prefix here — a "multi-line"
            # continuation someone hoped would extend it — is dropped the same
            # way: this format has no continuation syntax.
            [ -n "$what" ] && migration="$(trim "${line#*- migration:}")"
            ;;
        "- tells the user:"*)
            [ -n "$what" ] && tells="$(trim "${line#*- tells the user:}")"
            ;;
    esac
done < "$entries_file"
flush

[ "$count" -gt 0 ] || exit 0

printf '## Breaking\n%s' "$out"
