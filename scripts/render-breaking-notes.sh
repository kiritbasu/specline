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
#   - No marker, or nothing under it: prints nothing and exits 0. A release
#     with nothing breaking gets no section at all, not an empty heading.
#   - An entry missing its `migration` or `tells the user` field: refuses,
#     with a message naming the entry, and exits non-zero. A half-finished
#     acknowledgement failing loudly here is the whole point — the alternative
#     is a release that quietly ships without the sentence someone meant to
#     write.
#
# This mirrors `parse_acknowledgements` / `breaking_section` in
# crates/specline/tests/classify.rs line for line, on purpose: that file
# already parses this exact shape to gate a release on whether every breaking
# difference has an entry, and the two must agree about what a well-formed
# entry looks like. They are not shared code — one is a shell script this repo
# has no Rust runtime available to call at release time, the other is exercised
# by `cargo test` — so crates/specline/tests/render_breaking_notes.rs is what
# keeps them from drifting apart: it runs this script against fixtures and
# pins its behaviour the same way installer_checksum.rs pins scripts/patch-installer.sh.
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

# A file with no marker acknowledges nothing — the failing side on purpose, so
# that renaming the marker cannot silently wave every breaking change through.
if ! grep -qF "$marker" "$file"; then
    exit 0
fi

# Split out into a real file rather than a shell variable: the entries are
# somebody's prose, not code, and command substitution or a heredoc happily
# evaluates a stray `$(...)` inside prose it never should have looked at.
entries_file="$(mktemp)"
trap 'rm -f "$entries_file"' EXIT
awk -v marker="$marker" 'found { print } $0 == marker { found = 1 }' "$file" > "$entries_file"

if ! grep -q '[^[:space:]]' "$entries_file"; then
    exit 0
fi

# Trim leading and trailing whitespace — a plain parameter-expansion idiom
# rather than a `sed` subshell per line, since this runs once per line of
# every entry.
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
            # dropped rather than guessed at — same rule as classify.rs's
            # `parse_acknowledgements`.
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
