# Specline

Specline stores everything about a software project except the code: the specs,
the decisions, the tasks, the open questions, the feedback. It runs on your
machine. Claude Code and Codex read and write it through
[MCP](https://modelcontextprotocol.io) while you work, and an app shows you what
is in there.

---

## The problem

Two things go wrong when you build software with an AI agent.

**You explain the project again every session.** What it is, what you decided
last week, why the obvious approach does not work, what is half-finished. Claude
is good at the work and starts each session knowing none of it.

**Decisions do not survive the conversation.** You spend forty minutes working
out that the queue has to be idempotent because retries are at-least-once. You
agree, the code gets written, and the reason is nowhere. Six months later you
find the code, cannot remember why it is like that, and either work it out again
or break it.

Writing things down is the obvious fix. In practice that means a wiki nobody
updates, a `NOTES.md` that is stale in a fortnight, or an issue tracker built for
teams of thirty. They fail the same way: updating them is a separate job from
doing the work, so it does not get done.

## What Specline does

Claude writes to Specline as you work, in the same conversation. There is no
second step and nothing to remember.

- You mention a constraint. It becomes a **decision**, with your reasoning.
- You say "we should probably…". It becomes a **task**.
- Something turns out to be undecided. It becomes an **open question**, and later sessions see it before they argue it out again.
- Claude works out why something is slow. That goes on the task as a **note**, tagged with the conversation it came from.

The next session reads the store first and knows where things stand.

![The project overview: open work, questions and risks, recent decisions](docs/images/overview.png)

**Your data stays put.** Everything is in `~/.specline` on your disk. No
account, no cloud, no telemetry. The daemon listens on `127.0.0.1` and nothing
else can reach it.

**You get readable files.** Specline writes markdown into your repository, so
it is greppable, diffable, and committed alongside your code. If Specline went
away tomorrow you would still have the files.

### What it is not

- A team tracker. One person, one machine, no permissions, no assignees.
- A replacement for GitHub Issues if your team already uses them.
- A note-taking app. You can file and close things yourself, but Claude writes the reasoning, in the conversation where it came up.
- A chat log. It keeps what turned out to be true, not what was said.

---

## Install

Specline runs as one local daemon that every client talks to over HTTP, so
installing it is two jobs: get the daemon running, then tell your editor where
it is. [Claude Code](https://claude.com/claude-code) does both in three
commands. [Codex](https://developers.openai.com/codex) does the first the same
way and the second by hand.

### Claude Code

You do not need Rust, and there is nothing to edit by hand.

Run these three inside Claude Code:

```
/plugin marketplace add kiritbasu/specline
```

```
/plugin install specline@specline
```

```
/specline:setup
```

`/specline:setup` downloads the binaries, creates the store in `~/.specline`,
and starts the daemon. Then restart Claude Code. MCP servers connect when Claude
Code starts, so the `specline_*` tools will not appear in the session you
installed from.

Installing the plugin is what registers the MCP server and the two session
hooks. There is no `claude mcp add` to run and no `settings.json` to edit.

The hooks are what make this work without you asking:

- **SessionStart** puts a summary of the project at the top of every
  conversation, so Claude knows where things stand before you type anything.
- **Stop** notices a session ending without having recorded anything, and asks
  it to. Sessions that already wrote get nothing. A reminder that fires when you
  have done the right thing is one you would turn off.

To check on it at any point:

```bash
specline doctor
```

### Codex

Codex has no plugin to install, so the three things the plugin does — get the
daemon running, register the MCP server, install the hooks — are three separate
steps here. None of them needs Claude Code.

**First, check you can run `codex` at all.** Two of the steps below are CLI
commands, and if you installed Codex as the ChatGPT desktop app there is no
`codex` on your `PATH` — the binary lives inside the app bundle.

```bash
command -v codex || ls /Applications/ChatGPT.app/Contents/Resources/codex
```

If only the second half printed anything, link it somewhere on your `PATH`
once:

```bash
ln -s "/Applications/ChatGPT.app/Contents/Resources/codex" ~/.local/bin/codex
```

Use a directory that is actually on your `PATH` — `~/.local/bin` is common but
not universal, and `echo $PATH` settles it. Everything below assumes `codex`
runs.

**1. Get the daemon running.** The setup script is the same one
`/specline:setup` runs, and it does not know or care which editor you use.

```bash
git clone https://github.com/kiritbasu/specline.git
```

```bash
./specline/plugin/scripts/setup.sh
```

The clone is only how you get the script; it downloads the released binaries
rather than building them, so you do not need Rust for this either.

Skip both if Specline is already installed — one daemon serves every client, and
a second one would only fight the first for the store.

**2. Point Codex at it.**

```bash
codex mcp add specline --url http://127.0.0.1:7654/mcp
```

No token, no headers. The daemon binds `127.0.0.1` only, and refuses any request
carrying an `Origin` that is not this machine — which is a browser and never a
local client, since a local client sends none.

Older Codex builds only understood servers launched as a subprocess. If yours
rejects `--url`, add `experimental_use_rmcp_client = true` under `[features]` in
`~/.codex/config.toml`, or upgrade — `codex mcp add --help` says whether `--url`
is there.

**3. Install the hooks.** This is the step that matters most, and the one with
no equivalent command, so it goes in `~/.codex/config.toml` by hand:

```toml
[[hooks.SessionStart]]
matcher = "startup|resume"

[[hooks.SessionStart.hooks]]
type = "command"
command = "/Users/you/.cargo/bin/specline hook session-start"
timeout = 15

[[hooks.Stop]]

[[hooks.Stop.hooks]]
type = "command"
command = "/Users/you/.cargo/bin/specline hook stop"
timeout = 20
```

Write the path out in full rather than using `~`. TOML does not expand it, and
whether the runner does before executing is not something to find out by having
a hook quietly do nothing. `command -v specline` prints the path to paste.

**4. Trust them, from the terminal.** Run `codex` with no arguments to get the
interactive CLI, then type `/hooks` and approve the two entries. They appear as
*"New hook — review required"* until you do.

`/hooks` is a command of that CLI. It is **not** in the ChatGPT desktop app's
`/` menu, which lists skills — typing `/hooks` there finds nothing, and it is
easy to conclude the hooks are not supported rather than that you are in the
wrong place. Trust is recorded against a hash of each hook in your Codex config,
so granting it once in the terminal applies wherever Codex runs.

Do not skip this and do not assume it worked. Codex **silently skips any hook it
has not been shown** — no error, no warning, exactly the same as having
configured nothing. If Specline seems installed but sessions start with no
project summary, this is almost always why. Editing a hook's command afterwards
changes its hash and needs `/hooks` again.

Then restart Codex. MCP servers connect at startup, so the tools will not appear
in the session you set this up from.

**5. Check it.** `specline doctor` from a terminal says whether the daemon is
up and what it is serving. Inside Codex, ask it to call `specline_context` — if
the tools are connected it will answer with the project, and if they are not it
will say the tool is unavailable rather than guessing.

### Running both at once

You can. One daemon holds the store and every client is an HTTP client of it, so
there is never a second writer — that is the same arrangement two Claude Code
windows already use, and it is tested with sixteen concurrent sessions rather
than two.

Two consequences worth knowing. Claims are real across editors: if a Codex
session has claimed a task, Claude Code is refused it and told which session
holds it, which is the behaviour you want rather than a collision. And the
daemon's rate limit is one budget shared by everything connected, generous
enough that only a runaway loop reaches it.

### What leaves your machine

One thing, and you should hear it here rather than find it later. The daemon
checks for a new release every half hour. It sends nothing from your store: no
project names, no counts, no identifier. Nothing installs without you agreeing
to the restart.

Turn it off at install time with `--no-update-check`, or afterwards with
`SPECLINE_AUTO_UPDATE=0`. With it off, Specline makes no network requests at
all.

---

## Using it

### Mostly you do not

That is the idea. Work with Claude the way you already do, and Specline fills up
on its own.

Things that get Claude writing:

> "Let's go with the second option — Postgres, because we already run one."
> "That's a bug, the retry loop doesn't back off."
> "I don't know whether we need per-tenant keys. Leave it for now."

Things that get it reading:

> "What's the state of the auth work?"
> "Why did we pick SQLite?"
> "What's blocking the release?"
> "What should I do next?"

### The app

```bash
specline ui
```

The daemon serves the app itself, compiled into the binary, so there is no Node
and nothing else to start. It opens whatever address the daemon is listening on,
so a non-default port needs no arguments.

A board, with what to pick up next at the top — grouped by whether it is in an
open phase, and saying why each one is where it is:

![The board, with a ranked "next" strip above the columns](docs/images/board.png)

Documents that keep their reasoning. Requirements are anchored, so a task can
point at one requirement rather than a whole spec, and each document shows the
decision behind it and the tasks doing the work:

![A spec with requirement anchors and a panel of connected decisions and tasks](docs/images/document.png)

A roadmap, search across everything, and a feed of what changed and which
conversation changed it:

![The roadmap: shipped, active and planned milestones](docs/images/roadmap.png)

**The app files things. Claude writes them.** Creating a task, commenting,
closing, archiving, and moving a task's status, priority, kind, phase or
labels — drag a card between columns, or use the controls on the task itself.
Those are your own actions. The body of a spec or a decision gets written by
Claude in the conversation where you worked it out. That is the part worth
keeping, and it is not something anyone wants to type into a form.

Two moves the app will not make, and it says so rather than failing quietly.
Closing needs a reason, a message and evidence, so it opens the form that asks
for them instead of setting a status. And starting a task is a claim, which
records *which conversation* is on it — a person clicking a dropdown has none,
so the board asks you to have Claude pick it up.

### The command line

You will not need it often. Four are worth knowing:

```bash
specline doctor
```

```bash
specline next <project>
```

```bash
specline generate <project>
```

```bash
specline backup
```

`doctor` answers "has anything gone wrong": it runs every read-only check there
is, `fsck` included, and prints one page. `next` says what to work on next.
`generate` writes the markdown into your repo. `backup` takes a snapshot, and
`restore` puts it back.

All of it works whether or not the daemon is running. The CLI asks the daemon
when there is one and opens the store directly when there is not.

**All 24 commands are in [docs/CLI.md](docs/CLI.md).**

---

## What is in it

Thirteen kinds of thing, and that is the limit. "We need a new type for this"
nearly always turns out to be a field or a label:

**project**, **milestone**, **task**, **spec**, **decision**, **question**,
**term**, **feedback**, **design**, **environment**, **metric**,
**metric observation**, **artifact**.

They are joined by a typed graph. A task implements a spec, a decision
supersedes an older one, a task blocks another. The graph is what lets you ask
what is blocked instead of guessing.

Claude sees thirteen tools: `specline_context`, `specline_search`,
`specline_get`, `specline_projects`, `specline_activity`, `specline_create`,
`specline_update`, `specline_write_doc`, `specline_note`, `specline_link`,
`specline_next`, `specline_claim`, `specline_close`. Thirteen rather than
forty, because a model picks well from a short list and badly from a long one.

---

## Generated files

Point a project at your repository and Specline writes markdown into it. A new
project gets four files:

```
.specline/README.md       what the project is
.specline/questions.md    open questions, and settled ones with their answers
.specline/glossary.md     the project's own vocabulary
.specline/manifest.json   what was written, and what it came from
```

As documents accumulate, `.specline/specs/` and `.specline/decisions/` fill up
with one file each.

Anything past that is opt-in. A document can take a path of its own: tell
Specline that a spec lives at `docs/SPEC.md` and that is where it goes from then
on. A project can also say where its tracker and decision log belong. That is
why this repository has `product/SPEC.md`, `product/STATUS.md` and
`product/DECISIONS.md`. You do not get those by creating a project. Someone
asked for them.

**These files are output.** Each one says so at the top. Editing them is not
forbidden so much as pointless: the next `specline generate` writes over them
and your words are gone.

To change what they say, change the source. Ask Claude to rewrite it, or edit it
in the app. If you have already edited a file by hand and want the words kept,
`specline import <file>` puts them back in as a proper revision.

A pre-commit hook rejects any commit containing a hand-edited generated file, so
you find out then rather than after the next regeneration:

```bash
ln -sf ../../scripts/pre-commit .git/hooks/pre-commit
```

---

## Getting more out of it

**Talk about the project rather than dictating records.** "We're going with
Postgres because we already run one" gets you a decision with a reason in it.
"Create a decision record titled Postgres" gets you a row that means nothing in
six months.

**Say why out loud.** The reason is the part you will want later. The choice
usually looks obvious in hindsight; the option you rejected almost never does.

**Use the short IDs.** Tasks are `KEEL-42`, decisions are `B-12`. Use them in
conversation. They do not change, and they work anywhere an ID is accepted.

**Leave open questions open.** If something genuinely is not decided, recording
it as a question beats a confident guess. Every session sees open questions
before it starts, which stops Claude quietly re-deciding something you settled.

**Do not hand-edit generated files.** Change the source instead.

**Run `specline doctor` now and then**, and `specline backup` before anything
drastic.

**Restart the daemon after upgrading.** Specline will not start if the binary is
older than the store's schema. That turns a corrupted store into an error
message you can read, but you still have to restart it.

---

## How it is built

Rust, one workspace, six crates, one SQLite file, and a daemon that owns the
only write path. Search combines FTS5 keyword matching with `sqlite-vec`
similarity. Every change is an event carrying an author and the conversation it
came from.

**[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** has the detail: the crate
layout, how storage works, why the direction of a graph query is the easiest
thing to get wrong, what the app may and may not write, and the one build flag
that decides whether a platform compiles.

### Building from source

You need [Rust](https://rustup.rs). This is for working on Specline. To use it,
follow the plugin install above.

```bash
git clone https://github.com/kiritbasu/specline.git && cd specline
```

```bash
./plugin/install.sh
```

That builds the binaries and puts them in `~/.cargo/bin`, which is where a
release installs them too, so you only ever have one copy. It also creates the
store and copies the skill and hooks into `~/.claude/`.

After editing anything under `plugin/`, run `./plugin/install.sh --skill-only`.
It skips the build and copies the three files across. The copies under
`~/.claude` are what actually run, so a change you make in the repository and do
not copy does nothing.

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo fmt --all --check
```

The screenshots above come from `specline fixture`, which loads an invented
corpus into an empty store. `scripts/shoot-screenshots.mjs` retakes them.

### Where the documentation is

The prose is in `product/`, generated from the store:

- `product/PRD.md` — what this is for
- `product/SPEC.md` — how it works
- `product/DECISIONS.md` — every decision and why
- `product/STATUS.md` — what is open and what is next
- `product/CHANGELOG.md` — what has closed, with the reason and the evidence
- `product/JOURNAL.md` — what happened, session by session
- `product/GATE.md` — the one measurement that mattered, and why it stopped

Everything else the store holds is written the same way: one file per spec
under `.specline/specs/`, one per decision under `.specline/decisions/`, and
the open questions and glossary beside them. Those are the phase specs, the
build-time decisions and the terms, in the same generated form.
