<!-- specline:generated spec spc_01KZKSME2TCPVARX9M04836XD6
     Specline is the source of truth for this file. Edit it there — in the app, or by asking Claude — and regenerate.
     An edit made here is overwritten on the next `specline generate`. -->

# Specline — standing instructions

This file is loaded automatically into every Claude Code session in this repo. It is the contract. `product/HANDOFF.md` is orientation you read once; this is what you follow every time.

---

## Session ritual

**At the start of every session, in this order:**

1. Read `product/STATUS.md`. It tells you the current phase, what's in progress, and what's blocked. It carries **open work only** — what has closed is in `product/CHANGELOG.md` beside it, which you read when you want history rather than orientation. Both are rendered from the rows; read them, never edit them.
2. Read `.specline/questions.md`. It has two halves and you need both. **Open** is undecided: nothing there may be built on without saying so, and anything marked `blocked` halts work that depends on it. **Settled** is decided, with the reasoning — do not re-litigate it. Both halves are generated from the question rows, so there is nothing to keep in step.
3. `git log --oneline -15` — see what the last session actually did.
4. State in one line what you're picking up before you touch anything, and `specline_claim` it. `specline_next` is what to ask if the tracker leaves the choice open. **If it has no row — because it arrived as a sentence rather than out of the tracker — create one first.** That applies again every time the work changes during the session, not only at the start.

**At the end of every session, without exception:**

1. Move the task rows — `specline_close` what you finished, with the reason, a message and the evidence, and put anything you learned as a note on the row it belongs to. The tracker and the changelog both derive from this; there is no second place to update.
2. Add any decisions you made to the decision log.
3. Add any new unknowns as question rows.
4. **Regenerate**: `specline generate specline`. See "Specline is the source of truth" below — the files in `product/` are outputs, and an edit that never reaches Specline is lost on the next run.
5. Commit. Never leave the tree dirty for the next session.
6. Close with a two-line summary: what landed, what's next.

**If you run out of context mid-task**, update the tracker first with enough detail for a fresh session to resume, then stop. An accurate tracker is more valuable than one more file edited.

---

## Specline is the source of truth

Every markdown file in `product/` and `.specline/` is **generated**. Each one carries
a `<!-- specline:generated … -->` banner naming the artifact it came from. Editing
one directly is not wrong so much as futile: the next `specline generate specline`
overwrites it from the store.

The loop is:

1. Edit the prose *in Specline* — `specline_write_doc` over MCP, or in the app.
2. `specline generate specline` writes the files.
3. Commit the result.

**An edit made to a generated file is lost, not recovered.** There is no hook
that captures it. There was one — a `PostToolUse` hook that claimed to turn an
edit into an attributed revision — and it never worked: it called `keel mirror`,
a command that had been renamed out from under it, and swallowed the failure.
Every edit it claimed to capture was silently gone. It was deleted rather than
repaired, because a safety mechanism that quietly does nothing is worse than
none: it is *relied upon*. `scripts/pre-commit` now refuses the commit instead,
which is loud and which cannot be wrong about what it did.

If you have edited files by hand and want them in — during a migration, say —
`specline import <files> --project specline` writes each one back as a revision. Stop
the daemon first. SQLite would let the import open the store alongside a running
daemon, which is exactly why this is worth saying: nothing would stop you, and
you would have two writers against a store whose design assumes one.

`specline generate specline --check` exits non-zero when a file differs from what Specline
would produce. The pre-commit hook runs it for you; run it yourself if you are
committing with `--no-verify`.

**The check runs through whatever is installed.** So a change to a *renderer*
fails it until the new binaries are on the machine — the file on disk is what
the new code produces and the check asks the old code. That is not the hook
being wrong; it is the hook noticing that the tree and the installed generator
disagree. Run `./plugin/install.sh` and it passes.

**`specline_write_doc` takes the whole document, not a patch.** Changing one
paragraph of a long file means re-emitting all of it, which is the one editing
operation here that can go wrong silently: a dropped line in a 900-line
transcription rewrites the source and nothing downstream notices, because the
next `specline generate` writes out whatever you sent. After a full-body write, run
`specline generate specline` and read `git diff` — it should contain your change and
nothing else. If it contains more, the previous revision is still in Specline.

**One file is not generated and must not be**: the repository root's
`CLAUDE.md`. Claude Code loads it before anything else and imports this file
from it, so it is the bootstrap and cannot itself depend on a generation step
having run. Everything under `product/` and `.specline/`, this file included, is an
output.

---

## Tracker discipline

KB's primary window into this project is the desktop app, with `product/STATUS.md` as its committed shadow. Both read the same rows, so a stale tracker now means stale *rows* — there is nothing to update separately, and nothing that can drift.

**Rules:**

- **Make the row if there isn't one.** Most work arrives as a sentence — "cut the release", "this message is confusing", "have a look at why X is slow" — not as something already filed. `specline_create` it and then `specline_claim` it, before the first edit. One line of summary is enough. Measured on 2026-08-15: a session claimed its one task at 20:02 and closed it at 20:18, then spent the next forty-four minutes cutting a release and building a feature against no row at all — four of six commits landing while the board sat idle. Every other rule here assumes the row exists; this is the one that makes it true.
- **Claim a task before starting it**, not after: `specline_claim` over MCP, or `specline claim KEEL-42` from a terminal. That records who is on it as well as moving the status, so the app answers "what is happening right now" and not only "what has finished". This used to be an instruction you had to remember, and across sixty-six tasks the number of transitions into `in_progress` before work began was zero — which is why it is a tool now.
- **Ask `specline_next` what to pick up.** It is the ranking the digest carries, with a front door of its own and filters: unclaimed, by label, by milestone. Reaching for it costs a fraction of a full digest. It groups the work — an open phase first, then bugs, then everything else oldest first — because what a task unblocks turned out to be zero on every open row (B-83).
- A task is `done` only when it meets the definition of done below. Not when the code is written.
- **Cutting a release is a task, like everything else.** Create it and claim it *before* the tag is pushed, label it `release`, and close it with the tag's commit and the published release URL as evidence. 0.1.2 and 0.1.3 both went out as bare commits with no row, so neither shows on the board while it is happening nor in the changelog afterwards — and the changelog is exactly where "what shipped, and when" is supposed to be answerable.

  **The order matters, and it is the opposite of what it used to be.** The release *row* — a milestone with `kind: release` and a `version_string` — is written **before** the tag, not reconstructed from it afterwards. The workflow publishes the annotated tag's message as the release notes, and the row's summary is where that prose comes from, so writing the row first is what makes the tracker, the changelog and GitHub say the same words instead of three approximations of them.

  So: write the row with its summary and no `shipped_at`; `git tag -a v0.4.1 -F -` and paste that summary in; push the tag; then set `shipped_at` from the time GitHub actually published, and close the task with the tag's commit and the release URL.

  **A lightweight tag is refused by the workflow**, and separately an annotated one with an empty message. Not pedantry: `%(contents)` on a lightweight tag falls through to the commit it points at, so `git tag v0.5.0` without `-a` would publish the last commit's subject as the release notes — measured in a scratch repository, where it produced notes reading `init`.
- If a task turns out to be bigger than one task, split it and record the split. Don't silently expand scope.
- If you're blocked, say so on the row. **`blocked` is not a status** — it is derived from the `blocks` edges, so the way to mark something blocked is to draw the edge that blocks it. Never leave something `in_progress` across sessions without a note.
- Record what you *found* as a note on the task — `specline_note` over MCP, or `specline note add <task-id> "…"` from a terminal — not as a line in a markdown table. A status without the finding behind it is a colour, not information.
- The changelog writes itself, from the closed rows and the event log, into `product/CHANGELOG.md`. A session that achieved nothing still leaves a trace, which was the point of insisting on the entry — and a session whose work never became a row leaves one that says nothing about the work.
- **Never delete a task.** Close it with `specline_close` and a reason. The five reasons are `done`, `wont_do`, `duplicate`, `superseded` and `no_change`; every one needs a message, and `done` needs at least one piece of evidence — `commit:<sha>`, `pr:<url>`, `test:<command>`, `doc:<id>`, `url:<url>` or `image:<blob-id>`. `duplicate` and `superseded` name the other task and draw the edge themselves.

  This line used to say "mark it `dropped` with a reason". There has never been a `dropped` status, so a session following it literally got an enum rejection listing five values, none of them the word — quietly, for as long as it was written down.

**Task IDs are stable and never reused.** `KEEL-42` means the same thing forever, and it is what to use in conversation — the ULID underneath it is for machines.

**The tracker is rows, not prose.** `product/STATUS.md` is rendered from the
task rows by `specline render-status`. There is no tracker document to edit and no
markdown table to keep in step — changing what the tracker says means changing a
row. This closed TQ-14, and it closed the gap that made the question worth
asking: task rows now carry a **note stream**, so the findings that used to live
in the tracker's Notes column live on the task itself, attributed to the session
that learned them.

Three consequences worth internalising:

- **Never hand-edit `product/STATUS.md` or `product/CHANGELOG.md`.** Neither has
  a stored copy at all — both are projections of rows, so there is nothing an
  edit could become a revision *of*. The next render overwrites it.
- **The tracker is open work and the changelog is closed work.** They were one
  file until it reached 488 KB, 87% of it finished tasks, at which point the
  ritual's first instruction could not be carried out because the file exceeded
  what a reader would open. If you want to know what shipped, the changelog is
  the file; the tracker will tell you how many rows it left out and where they
  went.
- **The narrative moved.** Session-by-session accounts — what was tried, what
  broke, what a measurement actually said — are in `product/JOURNAL.md`, which
  *is* a document and is edited like any other prose. Findings that belong to
  one task go on that task as a note; the story of a session goes in the
  journal.

---

## Definition of done

A task is not done until all of these are true:

- [ ] Code compiles with zero warnings: `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] **And in the other configuration**: `cargo clippy --workspace --exclude specline-embed --all-targets --no-default-features -- -D warnings`, plus the same suite. That is what two of the three released platforms ship — see "The one feature" below — and without it the configuration most people install is the one nobody runs.
- [ ] Formatted: `cargo fmt --all --check`
- [ ] Tests written **and** passing — including at least one failure case, not only the happy path. The one exception is a *forward-looking* test for behaviour a later phase delivers: mark it `#[ignore = "unblocks in Phase N — see STATUS.md KEEL-x"]` so CI stays green and the intent stays visible. Never `#[ignore]` a test for behaviour the current phase is supposed to deliver.
- [ ] **Reviewed** with `/agent-skills:code-review-and-quality`, against all five axes — correctness, readability, architecture, security, performance — with every Critical and Required finding either fixed or filed as a row that names it
- [ ] No `unwrap()`, `expect()`, or `panic!()` in library code (binaries and tests may, with a message)
- [ ] Public items in `specline-core` have doc comments explaining *why*, not restating the signature
- [ ] The task **closed in Specline** with `specline_close` — reason `done`, a message saying what happened, and at least one piece of evidence — plus anything learned recorded as a note on it, and `specline generate specline` run
- [ ] Committed with a message that explains the change, not the diff

**Two words, and they are not interchangeable.** *The checks* are `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`. *The review* is reading the code against the five axes. Do not call either of them "the gate": one word for two things that catch different failures is how a session reports that something was verified when the other half never happened.

**Run the checks through the pinned toolchain.** `rust-toolchain.toml` pins 1.97, and a Homebrew `cargo` on `PATH` ignores it entirely — so `cargo clippy` can be checking a different compiler from the one CI uses, and pass. That happened on 2026-08-15: a session reported clippy clean all evening against 1.91 while CI failed on a lint 1.97 has. Either put `~/.cargo/bin` ahead of Homebrew on `PATH`, or run them as `rustup run 1.97 cargo …`.

**The one feature.** `embeddings` is declared by `specline` and `specline-daemon`, on by default, and it is the only feature in the workspace. It exists because it decides whether a platform can be *built at all*: the ONNX runtime the embedding model needs has no prebuilt Intel macOS library and wants a newer glibc than the Linux build is pinned to, so two of three release targets could not link while it was in the graph. Released binaries are built without it and cannot do semantic search; keyword search covers every artifact either way.

Two consequences for anyone running the checks:

- **`--exclude specline-embed` is not optional in the second configuration.** That crate is a workspace member, so `--workspace` builds it whatever features anything asked for — and building it *is* building the ONNX runtime. Without the exclusion the no-embeddings run links the very thing it exists to prove absent, and on Linux or an Intel Mac it cannot link at all.
- **`--all-features` is not used and buys nothing.** The only feature is already on by default. It was dropped on 2026-08-09 for a different reason that has since expired with the storage engine it named; there is simply nothing for it to turn on.

**Why the review is on the list rather than a good habit.** On 2026-08-16 a session ran the review over its own thirty-five commits and found three real defects — a callback that reloaded the whole page when a button only looked for an update, a progress line that cleared only because the parent happened to destroy it, and two writers racing on one staging file. `fmt`, `clippy`, the whole suite and CI were green throughout, and had been for every one of those commits. The checks tell you the code compiles and does what its tests say; they cannot tell you the tests are asking the right question. Nothing but reading finds that, and two of the three were introduced in the last hour of the session, when the work was going fastest and each change looked small.

Reviewing your own work counts, and is the usual case here — there is one developer. What does not count is skipping it because the tests are green, which is the exact condition under which those three shipped.

Three of these are now enforced rather than asked for. A task cannot reach a terminal status without a reason, a message and — for `done` — evidence: the check is in the storage layer, so the CLI and MCP cannot disagree and moving the status by hand does not get round it. The rest of this list, the review included, is still a list.

If you can't tick all of them, the task is `in_progress`.

---

## Engineering standards

**Language and tooling**

- Rust stable, edition 2024. One Cargo workspace.
- `thiserror` for library error types; `anyhow` only in binaries.
- `tracing` for all logging. No `println!` outside the CLI's user-facing output.
- `serde` for serialisation. `ulid` for IDs. `chrono` for time — never `jiff`, and never both (DECISIONS B-1).
- `cargo deny check` in CI for licences and advisories.

**Structure**

- `specline-core` never opens a network socket, never knows about MCP, never reads env vars. Everything it needs is passed in. This boundary is what makes the CLI, daemon, and future surfaces cheap — protect it.
- Storage access goes through three traits, named here since the spec only names the first: **`GraphStore`** (link traversal), **`DocumentStore`** (documents and blobs — revisions, embeddings, search), **`EntityStore`** (entity CRUD, links, events). No raw SQL outside their implementations. The traits are named for what they hold, never for what holds it — they came through a complete change of storage engine unchanged, which is the whole return on having drawn them in Phase 0.
- All graph traversal goes through `GraphStore`. Nobody hand-writes a recursive CTE at a call site. See "Graph direction" below for why.
- Six crates: `specline` (both binaries), `specline-core`, `specline-daemon`, `specline-mcp`, `specline-update`, `specline-embed`. `specline` builds `specline-daemon` as well as itself, which is why there is one installer rather than two.

**Errors**

- Errors carry context about *what the caller was trying to do*, not just what failed.
- Never swallow an error to keep going. If recovery is correct, log at `warn` with the reason.
- Validation errors returned over MCP must be actionable by a model reading them — say what was wrong and what would be valid.

**Testing**

Write tests as you go, not in a batch at the end of a phase. Required coverage:

- **Round-trip** for every entity type: create → read → update → archive.
- **Graph direction**: one test per relation in `product/SPEC.md` §3.3 asserting traversal in both directions. This is non-negotiable — see below.
- **Concurrency**: two simultaneous writers producing zero duplicates and zero lost updates.
- **Idempotency**: the same create called twice returns the same entity with `created: false`.
- **Optimistic concurrency**: a stale-version update is rejected and returns the current state.
- **Backup round-trip**: back up, wipe, restore, diff. Assert equality, don't eyeball it.
- **Snapshot tests** (`insta`) for MCP tool responses — they're an API contract, and drift should be visible in a diff.
- **Property tests** (`proptest`) for the graph traversal and the revision chain, where invariants matter more than examples.

**A test that reads the machine it runs on is a test that passes here and fails on Linux.** Twice in one day: an assertion that a refusal named `Desktop`, when the folder list drops folders that do not exist and a CI runner has none; and a hook that picked its `stat` flags in an order only BSD survives. Both were green on every Mac. If a test touches the home directory, the filesystem layout or a platform tool's flags, construct what it needs rather than inheriting it — and `HOME=/tmp/empty cargo test …` reproduces most of that class locally.

**Git**

- Small, focused commits. One logical change each.
- Conventional commit prefixes: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`.
- Short-lived branches off `main`, merged when green. Don't accumulate long-running branches — you're the only developer and they only create merge pain.
- Never commit secrets, `~/.specline` contents, or model weights. **The repository is public**, so a push publishes immediately and rewriting history does not unpublish it. Secret scanning and push protection are on, which is a backstop and not a permission.
- Never force-push `main`. Enforced rather than asked for since 2026-08-16: branch protection refuses a force-push or a deletion, admins included.

---

## Graph direction — read this before touching `links`

The first draft of the spec had **both** graph traversals inverted. This is the most dangerous bug class in the codebase, because an inverted traversal returns an empty result set that is indistinguishable from a legitimate "nothing is linked here." It fails silently, plausibly, and in a direction that makes the product look calm and correct while it quietly loses data.

Rules:

- `product/SPEC.md` §3.3 has the normative direction table. It is the only authority. Read it every time.
- `blocks` and `depends_on` are inverses. Only `blocks` is ever stored; `specline-core` swaps the endpoints on write. Never store both.
- Every relation gets an explicit test asserting what it returns traversing **outbound** and what it returns traversing **inbound**.
- Any query returning an empty graph result in development gets treated as a suspected direction bug until proven otherwise.

---

## Hard constraints

Violating these means rework, not a refactor:

1. **Everything that writes goes through `specline-core`'s write path.** That is the thing being protected, and it is worth saying in those words rather than as "no other process writes to the store" — six of the seven steps in a Specline write are validation, provenance, the event, the revision, the embedding and the index, none of which the database does for you, and a writer that goes round them produces rows that are poorer than every other row without anything failing.

   Only one process may hold the store open for writing at a time, and since B-60 that is enforced rather than asked for: an advisory lock, taken by the daemon for its lifetime and by a CLI command for its duration. The previous storage engine enforced it by refusing a second read-write connection outright; SQLite in WAL mode does not, so for a while it was a convention — until a second daemon was started with `--home` forgotten and migrated the store under the one already serving it. Reading takes no lock, because looking at a busy store is when you most need to. `--force` skips it, for the wedged daemon that is the reason the flag exists.
2. **The mirror is one-directional, with no exceptions.** Nothing reads a generated file back into the store on its own. `specline import` exists for deliberate migrations and is run by a person. Any code that diffs mirror state against database state is a bug.
3. **Soft delete only, for anything that is a record.** Nothing is ever `DELETE`d — rows, links, notes. The one exception is a *derived index*, which is not a record: `fts_source` and `document_chunks` hold nothing that cannot be rebuilt from the revision they came from, and both are deleted when the thing they describe changes or is archived (B-55). The test that keeps the exception honest asserts a passage can always be recomputed byte-for-byte; if that ever fails, the carve-out is unsound and passages go back to being archived like everything else.
4. **No silent truncation.** Every list that can be cut reports that it was cut, with a total.
5. **`session_id` is caller-supplied.** The daemon never invents one.
6. **No new artifact types** without KB's agreement. Thirteen is the ceiling.
7. **The interface may write what a person *does*. Claude keeps what a person *reasons*.**

   Creating a task, commenting on one, archiving or closing a row, moving a status or a priority — those are a person's own actions, and the interface performs them: through `specline-core`'s write path like everything else, attributed `actor: human`, `surface: ui`, and carrying the daemon's token, which is what makes "somebody clicked it" distinguishable from "a page did it" (KEEL-238).

   **Authoring is the half it does not do.** The body of a spec, a decision or a question is written by Claude in the conversation where the thinking happened. That is not squeamishness about forms: the reasoning *is* the product. Specline exists because why-this-and-not-that is the part that normally evaporates, and a person typing into a textarea produces a tracker with an AI feature attached — which is the thing this is trying not to be.

   The line, then, is **capture versus authoring**, and it is checkable: an endpoint that accepts a document revision is on the wrong side of it.

   Asking the daemon to *do* something it already knows how to do is on the permitted side, and two endpoints show where that lands: applying a staged update (B-75), and checking whether one exists (KEEL-258). Neither chooses a version, neither takes a body, and both are a person's own action.

   This replaces "the desktop app is read-only", which had been amended twice and was about to be a third time. KB has said authoring reaches the interface eventually (B-78), so the sentence says where this is going rather than something everyone had to read three exceptions past. When it does arrive, the question to answer first is not "can we build a form" but "what stops the reasoning becoming a field somebody fills in because the form asked".

---

## Scale discipline

This is one user and a few thousand rows. There is nothing to optimise.

Do not add caches, queues, connection pools, sharding, background workers, or async where sync would do, unless a measurement says otherwise. If you want to add one, put the measurement in `product/DECISIONS.md` first.

Optimise instead for: correctness, clarity, and how pleasant the MCP surface is for a model to use. The last one is the actual product.

---

## Working with KB

- **He is not always around.** If you're blocked on a question, do the preparatory work you safely can, record the question in Specline with the specific options and your recommendation, and move to another task. Don't idle.
- **Ask about**: anything touching storage format, the MCP tool surface, phase order, or the decisions in `product/SPEC.md` §13.
- **Decide yourself about**: anything reversible — naming, internal structure, library choices within the constraints above, test approach. Record it in `product/DECISIONS.md` with one line of reasoning.
- **Don't ask permission to do the obvious.** If a task in `product/STATUS.md` is unambiguous, do it.
- **Ask in plain English.** A question that needs a decision should be readable by somebody who has not just spent an hour in the code: what the options are, what each one costs, what you would pick. Not a paragraph of implementation detail with a question mark on the end.
- **Tell him when something is wrong.** If the spec is unbuildable, if a phase is misordered, if a decision turns out to be a mistake — say so plainly and early. He wants to know at the point it's cheap.

---

## Anti-patterns

Things that look like progress and aren't:

- Writing the desktop app because the daemon is hard.
- Adding an artifact type because the modelling is awkward — it's almost always a field or a `kind` value.
- Building the GitHub integration before the surface it decorates is finished, because it's more fun.
- Expanding the MCP surface past **thirteen** tools. More tools means worse model selection, not more capability. Nine was the cap, then ten when `specline_note` earned a slot, then thirteen when the three work verbs did (TQ-31). Each rise needed KB's agreement and an argument at least as good as the last — both are in the doc comment on `tools::all()`.
- Refactoring for elegance while the tracker says something is blocked.
- Hand-editing a generated file and committing it with `--no-verify`. The pre-commit check exists to stop exactly this, the next `specline generate` reverts it, and the reasoning in it is lost.
- Marking a task done because the code exists but the tests don't.
- Treating a green suite as a review. They answer different questions, and the three defects that produced this line were all in code where every check passed.
- Doing an hour of work that no row describes. It is not that Specline goes unwritten — decisions and notes get recorded — it is that the one artifact a person watches stays empty while the work happens.
