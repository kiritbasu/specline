# The `specline` command line

You rarely need this. Claude does the writing through MCP and the app does the
reading. The CLI is for what happens outside a conversation: checking the store
is sound, backing it up, regenerating the files in your repo, and fixing things
when they go wrong.

Every command is `specline <command>`. `specline <command> --help` prints the
same detail as this page, and is right if the two ever disagree.

---

## Flags every command takes

| Flag | Default | What it does |
|---|---|---|
| `--home <path>` | `~/.specline` | Where the store lives. Also `SPECLINE_HOME`. |
| `--json` | off | Machine-readable output instead of prose. |
| `--force` | off | Write even though a daemon appears to be running. |

`specline-core` never reads the environment, so resolving `--home` is the CLI's
job. That is why the flag lives here and not in the library.

Most commands also take `--daemon <url>`, which defaults to
`$SPECLINE_DAEMON_URL` and then `http://127.0.0.1:7654`. A command reads through
the daemon when one is running, since the daemon is the single writer and going
through it is what makes the read consistent. With no daemon answering, the
command opens the store directly.

**About `--force`.** Commands that write check for a daemon first and refuse if
one answers. The daemon owns the single write path, and a write that goes around
it skips six of the seven steps in a Specline write: validation, provenance, the
event, the revision, the embedding and the index. `--force` is for when you know
better, such as a wedged daemon or a store you are repairing. It is a flag rather
than an environment variable so that using it shows up in your shell history.

---

## Every day

### `specline status`

One line saying what is in the store. The quickest way to confirm the daemon and
the store agree.

### `specline next <project>`

What you can work on now, best first. Open work with nothing live in its way,
grouped — anything in an open phase, then bugs, then everything else oldest
first — and saying why each row is where it is.

What a task unblocks still sorts first where that means something. On a store
where nothing open blocks anything it means nothing, so the group decides
(B-83).

`specline ready` still works, as an alias. That was the name until B-85.

The MCP tool `specline_next` and the app read the same computation, so the three
cannot disagree.

| Flag | What it does |
|---|---|
| `--unclaimed` | Only work nobody is holding |
| `--label <label>` | Only tasks carrying all of these. Repeatable |
| `--no-label <label>` | Skip tasks carrying any of these. Repeatable |
| `--milestone <m>` | Only work under this milestone. An id, or a name like "Phase 8" |
| `--limit <n>` | How many to show (default 10) |

### `specline claim <task>`

Take a task: move it to `in_progress` and record which session is on it.
Refused if another session holds it, unless that claim has gone stale after
three days or you pass `--force`. Closing releases it.

`--session <id>` names the session doing the work, or set `SPECLINE_SESSION`.
Specline never invents one.

### `specline close <task>`

Close a task, saying why and showing the work.

| Flag | What it does |
|---|---|
| `--reason <r>` | `done`, `wont_do`, `duplicate`, `superseded`, `no_change` |
| `-m, --message <m>` | What happened, in a sentence or two |
| `--evidence <e>` | Typed proof. Repeatable |
| `--other <task>` | For `duplicate` and `superseded`: the other task |
| `--session <id>` | Who closed it, for attribution |

`done` needs a message and at least one piece of evidence. `wont_do` and
`no_change` need a message. `duplicate` and `superseded` name the other task and
draw the edge themselves.

Evidence is typed, so that "what shipped this week, with the commits" is a query
rather than prose: `commit:<sha>`, `pr:<url>`, `test:<command>`,
`doc:<entity-id>`, `url:<url>`, `image:<blob-id>`. A bare sha is refused.

### `specline task <title> --project <project>`

Create a task row.

| Flag | Default | What it does |
|---|---|---|
| `--project <p>` | — | Project id, slug or name. Required |
| `--body <text>` | — | Longer description |
| `--status <s>` | `todo` | `todo`, `in_progress`, `review`, `done`, `wont_do` |
| `--priority <p>` | `p2` | `p0`, `p1`, `p2`, `p3` |

> The built-in `--help` currently lists the statuses as "todo, in_progress,
> blocked, done, dropped". That is wrong. There is no `blocked` status — being
> blocked is derived from `blocks` edges — and `dropped` has never existed. The
> five values above are the real ones.

### `specline note`

Append to a row's running commentary, or read it back. This is what the
tracker's Notes column used to hold: what a session learned, attributed to the
conversation that learned it.

| Subcommand | What it does |
|---|---|
| `note add <id> "…"` | Append a note |
| `note ls <id>` | Print a row's notes, oldest first |
| `note retract <note-id>` | Retract one. Soft, like every removal here |

This is the only write path that does not go through MCP, so you can still
record a note when the MCP surface is down.

### `specline signal <summary> --project <project>`

File a signal into the Inbox: something somebody wants, before anybody has
decided whether to build it.

A signal is not a task. Nothing has been committed to, there is nothing to claim,
and it stays out of `next` and out of the open count until somebody triages it.
The only required argument is what was said.

| Flag | Default | What it does |
|---|---|---|
| `--project <p>` | — | Project id, slug or name. Required |
| `--kind <k>` | `idea` | `interview`, `support`, `sales`, `idea`, `competitor`, `observation` |
| `--source <s>` | — | Who said it, or where it came from |
| `--contact <c>` | — | How to reach them, if closing the loop will need it |
| `--occurred-at <t>` | today | When it was said |
| `--body <text>` | — | The verbatim, or the context |

### `specline triage <signal-id>`

Pick a signal up, or set it down with the argument.

A signal cannot leave the Inbox without an outcome, which is why this is a verb
rather than a field somebody sets. Setting one down does not delete it: the
argument is written onto the signal where search will find it, so the same idea
arriving in four months finds the reasoning instead of silence.

| Flag | What it does |
|---|---|
| `--feature <spc_…>` | Pick it up, naming the feature spec that makes the case |
| `--set-down <text>` | Set it down: why, in a sentence worth finding later |

### `specline ui`

Open the interface in a browser.

The daemon serves the app itself, compiled in, so this needs no Node and no
second process. It works out where the daemon is listening — the daemon records
the address it bound — so a non-default port still opens without anyone
remembering the number.

`--print` prints the address instead of opening anything, for a machine with no
browser.

If the daemon is not running it says so rather than opening a browser at a dead
port.

### `specline generate <project>`

Rewrite a project's repository files from the store. Specline holds the truth
and the markdown in your repo is output. This writes the adopted prose files at
their recorded paths, the `.specline/` mirror, and the tracker.

| Flag | What it does |
|---|---|
| `--repo <path>` | Repository root. Defaults to the project's recorded `root_path` |
| `--check` | Report what would change and exit non-zero if anything would |

It runs one way only: nothing here reads a generated file back into the store.

`--check` is what a pre-commit hook or CI runs. It turns a hand edit to a
generated file into a failure someone sees, rather than work someone loses.

---

## Is anything wrong

### `specline doctor`

Start here. It runs every read-only check there is — the file's own integrity,
referential integrity, whether search has any vectors, whether the committed
markdown still matches the store, how old the backup is, whether the clock has
stepped — and prints one page.

It exits non-zero only for a real problem. A degraded store says so without
failing, so you can put this in a hook.

`doctor` runs `fsck` for you as part of its sweep.

### `specline fsck`

Referential integrity on its own, in more depth than `doctor` reports. Exits
non-zero if something is actually broken, so it can gate a backup or a deploy.
Reach for it when `doctor` points you at it, and in release verification.

### `specline lint <project>`

List the rows a reader would struggle with. It never rewrites one.

| Flag | Default | What it does |
|---|---|---|
| `--check <rule>` | all | `task_without_summary`, `unexpanded_identifier`, `closed_without_reason` |
| `--limit <n>` | 40 | How many findings to print. The total is reported either way |

Three rules arrived after most of the store already existed and none can be
applied backwards, so this is the list you work through by hand.

It fixes nothing on purpose. A machine filling in a missing summary would write
exactly the confident, plausible, wrong prose that the requirement exists to
prevent.

---

## Moving data around

### `specline backup`

One consistent snapshot, plus a manifest. `--dest <path>` chooses where. It
defaults to `<home>/backups/<timestamp>`.

### `specline restore <source> <target>`

Restore a backup into a directory that does not already hold a store.

### `specline migrate`

Apply any schema migrations the store is missing. `--dry-run` says what would be
applied and applies nothing.

Nothing else migrates an existing store. A migration changes what every process
believes the tables look like, so it happens when you ask for it and the daemon
is stopped, not as a side effect of whichever command opened the store first
after an upgrade.

### `specline import <files…> --project <project>`

Import markdown into Specline as versioned documents.

| Flag | Default | What it does |
|---|---|---|
| `--project <p>` | — | Project id, slug or name. Required |
| `--as <type>` | `spec` | What to store them as |
| `--kind <k>` | inferred | `prd`, `spec`, `rfc`, `design-doc`, `note` |
| `--title <t>` | first heading | Override the title |
| `--dry-run` | off | Say what would land and write nothing |

Re-importing is safe. The same file lands on the same artifact, and unchanged
content adds no revision.

Use `--dry-run` first on a repository you have not imported before. Soft delete
is the only delete there is, so an artifact created by a wrong guess gets
archived rather than removed. It stays on disk, out of every view, for good.

Stop the daemon before importing. Nothing prevents the import opening the store
alongside a running daemon, so nothing will warn you if you forget.

### `specline archive <id> --version <n>`

Archive a row. Soft delete: it stays on disk and stops appearing. `--version` is
the version you believe is current, for optimistic concurrency.

### `specline reembed`

Give every current revision that has no vector one.

Embedding happens when a new revision is written and at no other time. So
turning the feature on leaves everything already in the store invisible to the
vector half of search, and it stays that way, because nothing would otherwise
rewrite those rows.

`--missing` is the default and, for now, the only mode. The first run downloads
the model and needs network access.

### `specline fixture`

Load the fixture corpus into an empty store: three invented projects with tasks,
specs, decisions, questions and feedback. This is what the screenshots in this
repository are taken from.

### `specline bootstrap`

Seed Specline's own project, which is the dogfooding switch. It imports the real
state from the product docs: phases as milestones, the task list, the decision
log, the open questions and the glossary.

`--repo <path>` records the repository path for the markdown mirror. `--only`
archives every other project, leaving just this one visible.

---

## Releases and updates

### `specline update`

Install the latest release, as long as it cannot change the store's shape.

| Flag | What it does |
|---|---|
| `--check` | Say what would happen and change nothing |
| `--rollback` | Put back the binaries the last update replaced |

A release that agrees with this one about the schema is interchangeable as far
as your store is concerned, so it gets applied without asking. One that moves
the schema stops and waits for you, because a migration rewrites data and
`--rollback` only puts binaries back — one generation of them, at that.

It downloads over plain HTTPS with no account and no token, and checks the
SHA-256 from the release manifest before moving anything into place.

Replacing the binaries is only half an update. The daemon is a separate process
still running what it loaded at startup, so it gets asked to restart afterwards
and checked to see which version came back.

### `specline release-manifest`

Print what a release of this binary promises, as JSON.

The updater has to decide whether a new version is safe to apply before it
downloads it, and it cannot ask the candidate binary — running the thing you are
deciding whether to run is the problem itself. So each release publishes this
alongside its artifacts and the updater reads it first.

It opens no store, so it can run in a release job against a freshly built binary
on a machine that has no Specline home at all.

---

## Called by something else

### `specline hook <session-start|stop>`

Run a Claude Code session hook. The plugin calls these, not you.

Both read JSON on stdin and write JSON on stdout, and both exit 0 whatever
happens. A hook that can block a session is worse than one that misses a record.

- `session-start` puts the project digest into a session as it starts.
- `stop` asks, once, whether anything from this session should have been
  recorded.

These were shell scripts before they were rewritten in Rust. They needed
`python3` and `curl`, neither declared anywhere, and `python3` is missing on a
Mac until the Xcode command line tools arrive. Every failure path exited 0
silently, so on a fresh machine they did nothing at all and it looked exactly
like Specline not working.

### `specline render-status <project>`

Print the generated tracker for a project to standard output. `--out <path>`
writes there instead.

`--force` means something narrower here than elsewhere: write even if the result
is dramatically smaller than what is already there.

### `specline gate`

Score the Phase 2 exit criterion from the event log.

| Flag | Default | What it does |
|---|---|---|
| `--project <p>` | all | Restrict to one project |
| `--since <ts>` | — | Only count activity after this instant (RFC 3339) |
| `--run <dir>` | — | Score from an archived run directory instead of the event log |
| `--sessions <n>` | 10 | How many sessions were run: the denominator |

It does not run the sessions. The claim being measured is that Claude writes
without being prompted, and a test that calls the tool has prompted it. This
scores what the sessions did.

`--sessions` is not derived from the log, because a session that wrote nothing
leaves no event and the log cannot tell you it happened. Use `--run` for a real
measurement: one transcript file per session, so ids cannot collide and a
session that only offered to write is still visible.
