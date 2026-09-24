# Breaking changes, acknowledged

Every breaking difference the classifier finds has to appear below the marker in
this file before a release can merge. That gate — `the_real_release_is_gated_when_a_baseline_is_named`
in `crates/specline/tests/classify.rs` — fails in both directions: a breaking
change with no entry, and an entry describing nothing that changed. **It only
runs when `CONTRACTS_BASELINE` is set**, and nothing in CI sets it yet, so
right now it is a command someone has to remember to run by hand against the
previous tag before cutting a release — not yet something a release can fail
on without a person choosing to check.

This is the mechanism. The version number is decoration — on 0.x, additive and
breaking both mean a minor bump, so a gate that checked the number would be
satisfied by every release forever while looking like a guard.

## How to add one

Run the classifier against the previous release tag:

```
CONTRACTS_BASELINE=<last-tag> cargo test -p specline --test classify -- --nocapture
```

Copy each `BREAKING` line **exactly** as printed, and write the two fields
underneath it:

```markdown
## tool `specline_note` was removed
- migration: none — callers move to `specline_update` with a `notes` field
- tells the user: `specline_note` is gone. Notes are a field on `specline_update` now.
```

The heading has to match the classifier's sentence word for word. That is
deliberate: if the description of what breaks has changed, whoever signed it off
should read it again, rather than have a stable key quietly carry an old
agreement forward.

`migration: none` is a real answer when nothing needs migrating. Blank is not —
an entry with an empty field is somebody acknowledging that a problem exists
rather than the problem.

## Why the marker

Everything above `<!-- acknowledgements -->` is instructions, and its headings
are not entries — including the example directly above, which is a real-looking
entry that must never be counted as one.

Without the marker the parser read `## How to add one` as an acknowledgement of
a difference by that name, which then failed as stale and blocked a release that
was otherwise fine. The instructions live in the same file on purpose, because
they are what somebody needs at the moment they are writing an entry, so the
boundary has to be explicit rather than inferred from heading levels.

A file with no marker acknowledges nothing. That is the failing side on purpose:
renaming the marker must gate everything rather than wave everything through.

The **Breaking** section of the release notes is generated from the entries
below, which is the point of writing them here rather than in a commit message.
Notes assembled by hand from a week of commits are how a breaking change reaches
users unannounced; notes built from the same entries a human wrote down to
describe the change cannot forget one. `scripts/render-breaking-notes.sh` is
what does the generating — `.github/workflows/release.yml` runs it against
this file and prepends whatever it prints to the tag's own notes (KEEL-299).

That script has no idea which release an entry belongs to, because nothing
below this line says so. It renders every entry under the marker, every time.
**So delete an entry once the release carrying it has shipped.** Nothing in CI
currently stops you from forgetting — the stale-entry gate above only runs when
somebody remembers to set `CONTRACTS_BASELINE` — which is exactly how the two
entries acknowledging the `specline_ready` → `specline_next` rename survived
five releases (0.4.0, 0.4.1, 0.5.0, 0.5.1, 0.6.0) after the rename had already
shipped, undetected until this paragraph removed them. Leaving a shipped entry
in place does not corrupt anything; it just publishes the same "Breaking" note
again on the next release, for a change nobody made this time.

<!-- acknowledgements -->

