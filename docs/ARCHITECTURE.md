# How Specline is built

Rust, one Cargo workspace, six crates, one SQLite file, and a daemon that owns
the only write path. The app is a React build compiled into the daemon binary.
No services, no containers, no account.

This describes the shape of the thing, and how it got that way.

---

## The pieces

```
crates/specline-core/     domain types, storage, the graph, search,
                          generation, backup — everything that is not I/O
crates/specline-mcp/      the thirteen tools, the digest, protocol handling
crates/specline-daemon/   axum: the MCP endpoint, a local read API, the app
crates/specline-embed/    the local embedding model, kept out of the core
crates/specline-update/   version comparison, download, replace, roll back
crates/specline/          the CLI, and both shipped binaries
apps/desktop/             React. The read surface, and a person's own actions
```

Two binaries ship, `specline` and `specline-daemon`, and one package declares
both. The release tooling builds one installer per package that owns binaries,
and we want one installer rather than two. The daemon's code still lives in
`specline-daemon`; only its entry point moved.

### The boundary that matters

`specline-core` never opens a network socket, never knows about MCP, and never
reads an environment variable. Everything it needs is passed in.

That is what makes the CLI and the daemon cheap to build, and what would make a
third surface cheap. Every shortcut across this boundary costs you later, so it
is worth defending.

Storage goes through three traits:

- `EntityStore` — entity CRUD, links, events
- `DocumentStore` — documents and blobs: revisions, embeddings, search
- `GraphStore` — link traversal

No raw SQL exists outside their implementations. They are named for what they
hold rather than for what holds it, so a complete change of storage engine left
all three unchanged.

---

## Storage

Everything is in **one SQLite file**: entities, links, the event log, document
revisions, images and vectors, in WAL mode. SQLite is compiled in, so the
binaries are self-contained and there is no database to install alongside them.

Search runs two ways at once. FTS5 handles keywords, `sqlite-vec` handles
similarity, and the two result lists are combined by reciprocal rank, because
BM25 scores and vector distances are not on the same scale and cannot simply be
added. Keyword search covers every artifact whether or not the build can embed.

It was two engines until Phase 9: DuckDB for rows and Lance for documents, with
the second attached into the first as a SQL namespace. That worked, but it cost
a 22-minute build, a keyword index rebuilt in full on every write, and two
backup formats to keep in step.

### One writer

Only one process may hold the store open for writing. An advisory lock enforces
it: the daemon takes it for its lifetime, a CLI command for its duration.
Reading takes no lock, because looking at a busy store is when you most need to.

This used to be a convention rather than a rule. The old engine refused a second
read-write connection outright, so nothing had to enforce it. SQLite in WAL mode
does not refuse, and a second daemon started with `--home` forgotten migrated
the store underneath the one already serving it. Hence the lock, and `--force`
as the deliberate way past it.

### Everything that writes goes through one path

A Specline write is seven steps: validate, record provenance, append the event,
append the revision, embed it, index it, commit. Six of those are not things the
database does for you.

So the rule is about the path, not the process. Code that writes rows directly
produces records that are poorer than every other record, and nothing appears to
fail.

### Soft delete

Nothing that is a record is ever deleted: not rows, not links, not notes.
Archiving hides it and keeps it on disk.

Derived indexes are the exception. `fts_source` and `document_chunks` hold
nothing that cannot be recomputed from the revision they came from, so both get
deleted when the thing they describe changes or is archived. A test keeps that
honest by asserting a passage can always be rebuilt byte for byte.

---

## The model

Thirteen artifact types, and thirteen is the cap:

**project**, **milestone**, **task**, **spec**, **decision**, **question**,
**term**, **feedback**, **design**, **environment**, **metric**,
**metric observation**, **artifact**.

"We need a new type for this" nearly always turns out to be a field or a `kind`
value. Adding a fourteenth takes an argument, not a pull request.

The types are joined by a typed graph: a task `implements` a spec, a decision
`supersedes` an earlier one, a task `blocks` another. The graph is what lets you
query for blocked work instead of guessing at it.

There is no `blocked` status for the same reason. Being blocked is a fact about
the graph, and storing it as a status as well meant two facts that had to agree
and sometimes did not.

### Getting graph direction wrong

This is the easiest serious mistake to make here. Walk a relation the wrong way
and you get an empty result, which looks exactly like a legitimate "nothing is
linked to this". No error, no warning, and the product looks fine while it
quietly drops data. The first draft of the spec had both traversals inverted.

Direction reads left to right: **`from` does the verb to `to`.** This table is
the authority. Read it each time rather than remembering it.

| Relation | Reads as | Example |
|---|---|---|
| `implements` | from **implements** to | task → spec `REQ-4` |
| `blocks` | from **blocks** to | task A → task B, A first |
| `depends_on` | from **depends on** to | inverse of `blocks`; never both |
| `supersedes` | from **supersedes** to | decision v2 → decision v1 |
| `derived_from` | from **derives from** to | spec → feedback |
| `resolves` | from **resolves** to | decision → question |
| `references` | from **references** to | anything → anything |
| `duplicates` | from **duplicates** to | task → task |
| `informs` | from **informs** to | feedback → spec |

The rules that follow:

- `blocks` and `depends_on` are inverses. Only `blocks` is stored, and
  `specline-core` swaps the endpoints on write. Nothing in the schema enforces
  that and nothing can, since both are legal values, so it is enforced on write
  and audited by `fsck`.
- Every relation has a test asserting what it returns outbound and what it
  returns inbound.
- Treat an unexpectedly empty graph result as a direction bug until you have
  proved otherwise.

### Provenance

Every change is an event carrying an author, a surface and the session that made
it, so you can always answer who changed this and in which conversation.

`session_id` comes from the caller and the daemon never invents one. A stateless
transport has no session to borrow, so this only works if callers cooperate.
Leaving it out still writes, but the write is attributed to nothing more
specific than "some Claude session".

Surfaces are `chat`, `cowork`, `code`, `ui` and `cli`. That is how you tell
"somebody clicked it" from "a page did it".

---

## The daemon

One axum server on `127.0.0.1:7654`, serving three things:

- the MCP endpoint at `/mcp`, over streamable HTTP
- a local read API under `/api`, which the app uses
- the app itself, compiled into the binary, so there is no Node and no second
  process

It binds loopback only. There is no account, no cloud and no telemetry.

One request leaves your machine: the daemon fetches the latest release manifest
every half hour so it can tell you a new version exists. It sends nothing from your
store. `SPECLINE_AUTO_UPDATE=0` turns it off, and with it off Specline makes no
network requests at all.

---

## The MCP surface

Thirteen tools:

| | |
|---|---|
| `specline_context` | The digest. What this project is, right now |
| `specline_search` | Hybrid search across everything |
| `specline_get` | Fetch by id, with graph traversal and revision diffs |
| `specline_projects` | What projects exist |
| `specline_activity` | What changed, and who changed it |
| `specline_create` | Create any of the thirteen types |
| `specline_update` | The fields around a document: title, status, kind |
| `specline_write_doc` | Append a revision of a prose body |
| `specline_note` | Append to a row's running commentary |
| `specline_link` | Draw an edge |
| `specline_next` | What to work on next |
| `specline_claim` | Take a task |
| `specline_close` | Close one, with a reason and evidence |

Thirteen rather than forty, because a model picks well from a short list and
badly from a long one. The cap was nine, then ten when `specline_note` earned a
slot, then thirteen when the three work verbs did. Each rise needed an argument
at least as good as the last, and they are recorded in the doc comment on
`tools::all()`.

How well this surface reads to a model is the actual product. Optimise for that,
for correctness, and for clarity. There is nothing here to make faster: one user
and a few thousand rows.

---

## Generation runs one way

Specline holds the truth. The markdown in your repository is output.

`specline generate <project>` writes the adopted prose files at their recorded
paths, the `.specline/` mirror, and the tracker. Every generated file says so at
the top.

Nothing reads a generated file back into the store on its own. `specline import`
exists for deliberate migrations and a person runs it. Code that diffs mirror
state against database state is a bug.

There was once a hook that claimed to turn a hand edit into an attributed
revision. It never worked: it called a command that had been renamed out from
under it and swallowed the error. Every edit it claimed to capture was silently
lost. It got deleted rather than fixed, because a safety mechanism that quietly
does nothing is worse than not having one — people rely on it. A pre-commit
check refuses the commit instead. It is noisy and it cannot be wrong about what
it did.

---

## What the app may write

The app is not read-only, and the split is not reads versus writes.

**The app writes what a person does.** Creating a task, commenting, archiving,
closing, and moving a status, priority, kind, phase or labels — from the task
screen, or by dragging a card between board columns. Those are your own actions.
They go through `specline-core`'s write path like everything else, attributed
`actor: human`, `surface: ui`, with no session id, because the daemon never
invents one.

Field changes go through one narrow endpoint, `PATCH /api/tasks/{id}`, which
takes those five fields and a version and nothing else. A generic entity patch
would have been less code and was rejected on the test below: five named fields
make a document body unreachable by construction rather than by a rule somebody
has to remember.

**Two transitions it refuses, and the reasons are not the same.** A terminal
status owes a reason, a message and evidence, which the storage layer demands on
every path — so the endpoint refuses `done` and `wont_do` and the interface
routes you to the form that collects them. `in_progress` is refused outright,
because starting work is a *claim* and a claim records which session holds it;
a person at the interface has none, and a board reporting work in flight against
nobody is the state `specline_claim` exists to prevent. Moving out of
`in_progress` releases the claim, which closing does not need to do, since a
closed row cannot be claimed again.

**It does not do the authoring.** The body of a spec, a decision or a question
gets written by Claude, in the conversation where the thinking happened. That is
not squeamishness about forms. The reasoning is what Specline is for, and a
person typing into a textarea gets you a tracker with an AI feature bolted on,
which is the thing this is trying not to be.

So the line is capture versus authoring, and you can check it: an endpoint that
accepts a document revision is on the wrong side.

---

## The one build flag

`embeddings` is declared by `specline` and `specline-daemon`, on by default, and
it is the only feature in the workspace.

It exists because it decides whether a platform compiles at all. The embedding
model needs the ONNX runtime, which has no prebuilt Intel macOS library and
wants a newer glibc than the Linux build is pinned to. Two of the three release
targets could not link while it was in the graph.

Released binaries are therefore built without it and cannot do semantic search.
Keyword search covers every artifact either way. `specline doctor` and
`/api/health` both report which build you are running, since a version number
cannot tell you.

A build from source gets embeddings wherever it links.

---

## Working on it

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo fmt --all --check
```

Then the configuration two of the three released platforms actually ship:

```bash
cargo clippy --workspace --exclude specline-embed --all-targets --no-default-features -- -D warnings
```

`--exclude specline-embed` is required there, not optional. That crate is a
workspace member, so `--workspace` builds it whatever features were asked for,
and building it means building the ONNX runtime. Without the exclusion the
no-embeddings run links the very thing it is meant to prove absent, and on Linux
or an Intel Mac it cannot link at all.

`rust-toolchain.toml` pins the compiler. A Homebrew `cargo` earlier on `PATH`
ignores that file, so the checks can pass against a different compiler from the
one CI uses. Put `~/.cargo/bin` first, or run them through `rustup run`.

To work on the interface, `npm run dev --prefix apps/desktop` gives you hot
reload against the same daemon.
