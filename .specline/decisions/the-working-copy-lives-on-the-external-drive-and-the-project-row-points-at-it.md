<!-- specline:generated decision dec_01M1HPZ7E0S93P1D6SMZMG0VHJ v1 2026-09-02T18:46:15Z
     source of truth is Specline — edits here are not saved -->
# B-96 — The working copy lives on the external drive, and the project row points at it

**Status:** `accepted`  
**Id:** `dec_01M1HPZ7E0S93P1D6SMZMG0VHJ`

The checkout moved from `/Users/h8hcn/development/specline` to `/Volumes/mydrv/development/specline`, and the project's `root_path` now names the second one.

**Why it needed deciding rather than just doing.** For a while both existed. They were byte-identical — same HEAD `fe45fcd`, the same four branches at the same shas, both clean, neither carrying a stash — so nothing was at stake in the copies themselves. What was at stake is `root_path`: it is what `specline generate` resolves when nobody passes `--repo`, so while it named the old directory, a generate run from the new one would have written the mirror into a checkout nobody was editing. Two copies is survivable; two copies and a store that disagrees about which is real is how a generated file silently goes stale.

**Verified before switching.** A full clean rebuild, `cargo fmt --all --check`, both clippy configurations and both test suites pass from the external volume — 1,266 tests with embeddings on and 1,265 without. Nothing in the build turned out to be bound to the old path (KEEL-352).

**What the old directory is now.** Not deleted, and not a second working copy either — its `target/` is gone, which is where 9.7 GB of the 10 GB was. The internal disk had 1.3 GB free before that and has 11 GB after, which is the immediate reason it happened at all. What remains is source at the same commit, plus `node_modules`, `.gate-runs` and `mutants.out`.

**The thing to watch.** An external volume can be unmounted, and `root_path` will then name a directory that is not there. That is a worse failure than the one this fixes, because it is intermittent: the store stays right and the filesystem goes away underneath it. Nothing checks for this today.

