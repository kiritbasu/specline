<div align="center">

<img src="docs/images/logo.png" width="120" alt="Specline" />

# Specline

**A local issue tracker built for long-horizon work with AI agents.**

[![CI](https://github.com/kiritbasu/specline/actions/workflows/ci.yml/badge.svg)](https://github.com/kiritbasu/specline/actions/workflows/ci.yml) [![Release](https://img.shields.io/github/v/release/kiritbasu/specline?color=blue)](https://github.com/kiritbasu/specline/releases/latest) [![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE) [![Platform](https://img.shields.io/badge/platform-macOS%20arm64-lightgrey.svg)](#requirements)

[Install](#install) · [What is in it](#what-is-in-it) · [Full install guide](docs/INSTALL.md) · [Architecture](docs/ARCHITECTURE.md) · [CLI](docs/CLI.md)

</div>

---

Specline stores and organises everything about a long-running AI development
project: specs, PRDs, roadmaps, tasks and feature requests. It is a personal,
local alternative to Linear, Shortcut or any hosted ticketing system. You work
with Claude Code or Codex the way you already do, and Specline records it as you
go.

![The project overview: open work, questions and risks, recent decisions](docs/images/overview.png)

## The problem

Two things go wrong on a project that runs for months with an AI agent. An agent
opens every conversation without the project's history, so you supply it again.
And the reasoning behind a decision does not survive the conversation it happened
in, which is the part you need six months later when something has to change.

Writing it down is the obvious answer, and it means a wiki, a `NOTES.md`, or a
tracker built for a team of thirty. All three go stale for the same reason:
keeping them current is a separate job from the work, so it is the job that gets
dropped.

## What Specline does

Claude writes to Specline as you work, in the same conversation, so there is no
second step to remember.

| You do this | Specline gets |
|---|---|
| mention a constraint | a **decision**, with your reasoning |
| say "we should probably…" | a **task** |
| leave something undecided | an **open question**, which later sessions see before they argue it out again |
| work out why something is slow | a **note** on the task, tagged with the conversation it came from |

The next session reads the store before it reads anything else.

**The app.** `specline ui` opens it. The daemon serves it from the binary, so
there is no Node and nothing else to start. The board puts what to pick up next
at the top, and says why:

![The board, with a ranked "next" strip above the columns](docs/images/board.png)

A document shows the decision behind it and the tasks doing the work, and a task
can point at one requirement in a spec rather than the whole thing:

![A spec with requirement anchors and a panel of connected decisions and tasks](docs/images/document.png)

The app files things and Claude writes them. Creating, closing and moving tasks
are your own actions. The body of a spec or a decision is written by Claude, in
the conversation where you worked it out. The full boundary is in
[what the app may write](docs/ARCHITECTURE.md#what-the-app-may-write).

**The command line.** You will not need it often. Four are worth knowing:

```bash
specline doctor      # has anything gone wrong? every read-only check, one page
specline next        # what to work on next
specline generate    # write the markdown into your repo
specline backup      # snapshot the store; `restore` puts it back
```

All of it works whether or not the daemon is running. All 26 commands are in
[docs/CLI.md](docs/CLI.md).

**Your data stays on your disk.** Everything lives in `~/.specline`, with no
account and no cloud behind it. The daemon listens on `127.0.0.1`, so nothing off
your machine can reach it. One thing does leave: the daemon checks for a new
release every half hour, sending nothing from your store and installing nothing
without you agreeing to the restart. `--no-update-check` at install time or
`SPECLINE_AUTO_UPDATE=0` afterwards turns that off, and then Specline makes no
network requests at all.

### What it is not

- A team tracker. One person, one machine, no permissions and no assignees.
- A replacement for GitHub Issues if your team already uses them.
- A note-taking app. You can file and close things yourself, but Claude writes the
  reasoning, in the conversation where it came up.
- A chat log. It keeps what turned out to be true.

---

## Install

### Requirements

**macOS on Apple Silicon.** Intel Macs and Linux build from source; the released
binary is arm64 only, for the reason in
[the one build flag](docs/ARCHITECTURE.md#the-one-build-flag).

### Claude Code

Three commands, inside Claude Code:

```
/plugin marketplace add kiritbasu/specline
```

```
/plugin install specline@specline
```

```
/specline:setup
```

`/specline:setup` downloads the binaries from the latest GitHub release, checks
them against the SHA-256 in `specline-release.json`, creates the store in
`~/.specline`, and starts the daemon. No Rust needed, despite the binaries
landing in `~/.cargo/bin`.

**Then restart Claude Code.** MCP servers connect at startup, so the `specline_*`
tools will not appear in the session you installed from.

There is no `claude mcp add` to run and no `settings.json` to edit. Installing
the plugin also installs three hooks: one puts a summary of the project at the
top of every conversation, one tells the session once when it commits work that
names no task and has nothing claimed, and one asks a session that recorded
nothing whether it should have.

To check the install:

```bash
specline doctor
```

<details>
<summary><b>Codex</b> — no plugin, so four steps</summary>

<br>

Run `./specline/plugin/scripts/setup.sh` from a clone to get the daemon running,
then:

```bash
codex mcp add specline --url http://127.0.0.1:7654/mcp
```

The hooks have no equivalent command. They go in `~/.codex/config.toml` by hand
and then have to be approved with `/hooks` in the interactive CLI — **Codex skips
any hook it has not been shown, with no error and no warning**, which is the
usual reason a Specline install looks fine but sessions start with no project
summary.

**[The full walkthrough is in docs/INSTALL.md](docs/INSTALL.md)**, including the
ChatGPT-desktop `PATH` wrapper, the exact TOML, and a one-command check that
proves all three pieces at once.

</details>

**Running two editors, managing the service, and uninstalling** are all in
**[docs/INSTALL.md](docs/INSTALL.md)**.

---

## What is in it

Thirteen types, and thirteen is the cap. "We need a new type for this" nearly
always turns out to be a field or a label.

| | |
|---|---|
| **task**, **milestone** | the work, and what it ships in |
| **spec**, **decision**, **question** | what to build, what you chose, what is still open |
| **feedback**, **design**, **artifact** | what came in from outside, and anything that fits nowhere else |
| **term**, **environment**, **metric**, **metric observation** | the project's own words, where it runs, what you measure |

Everything belongs to a **project**, and typed links join them: a task
*implements* a spec, a decision *resolves* a question, a task *blocks* another.
That turns "what is blocked, and by what" into a query.

---

## Markdown in your repository

Specline can write its documents into your repository as markdown. This is
optional: nothing in Specline reads the files back, and everything works the
same without them. The files are for people, and for tools that read the
repository rather than the store.

What you get from turning it on:

- Specs, decisions and questions you can read with `cat` on a machine that has
  no Specline.
- The same text in `grep` and in your editor's search, next to the code it
  describes.
- An agent or editor with no MCP connection still sees them.

What it costs: a change in the store shows up as a diff in files nobody edited.
Commit the files or gitignore them, whichever suits the project.

`specline generate <project>` writes them. A new project gets four files:

```
.specline/README.md       what the project is
.specline/questions.md    open questions, and settled ones with their answers
.specline/glossary.md     the project's own vocabulary
.specline/manifest.json   what was written, and what it came from
```

Specs and decisions get one file each under `.specline/specs/` and
`.specline/decisions/`. A document can have a path of its own, so a spec can
live at `docs/SPEC.md` instead.

Each file says at the top that it is generated, and the next run overwrites any
edit. To change one, change the source: ask Claude, or edit it in the app.
`specline import <file>` takes a hand edit back into the store as a revision.
`specline generate <project> --check` fails when a file differs from the store;
[docs/CLI.md](docs/CLI.md#specline-generate-project) shows how to run it from a
pre-commit hook.

---

## How it is built

Rust, one workspace, six crates, one SQLite file, and a daemon that owns the only
write path. Search is FTS5 for keywords and `sqlite-vec` cosine distance over
stored embeddings for meaning. Every change is an event that records who made it
and which conversation it came from. Agents use it through
[thirteen MCP tools](docs/ARCHITECTURE.md#the-mcp-surface).

Specline is built with Specline. Its own tasks, decisions and open questions are
in a store on the machine it was written on.

**[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** covers the crates, storage,
graph direction and what the app may write.
**[CONTRIBUTING.md](CONTRIBUTING.md)** covers building from source.

---

<div align="center">
<sub>Apache-2.0 · <a href="https://github.com/kiritbasu/specline/issues">Issues</a> · <a href="docs/INSTALL.md">Install</a> · <a href="docs/ARCHITECTURE.md">Architecture</a> · <a href="docs/CLI.md">CLI</a></sub>
</div>
