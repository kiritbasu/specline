# Breaking changes, acknowledged

Instructions live above the marker and must never be read as entries,
including a heading that looks like one: see "## How to add one" below.

## How to add one

Not a real entry.

<!-- acknowledgements -->

## tool `specline_note` was removed
- migration: none — callers move to `specline_update` with a `notes` field
- tells the user: `specline_note` is gone. Notes are a field on `specline_update` now.

## `specline_update` argument `status` is now required
- migration: pass the row's current status if you are not changing it
- tells the user: `status` is now required on `specline_update`.
