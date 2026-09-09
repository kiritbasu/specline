<div align="center">

<img src="docs/images/logo.png" width="120" alt="Specline" />

# Specline

**A local issue tracker built for long-horizon work with AI agents.**

[![CI](https://github.com/kiritbasu/specline/actions/workflows/ci.yml/badge.svg)](https://github.com/kiritbasu/specline/actions/workflows/ci.yml) [![Release](https://img.shields.io/github/v/release/kiritbasu/specline?color=blue)](https://github.com/kiritbasu/specline/releases/latest) [![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE) [![Platform](https://img.shields.io/badge/platform-macOS%20arm64-lightgrey.svg)](#requirements)

[Install](#install) · [Using it](#using-it) · [What is in it](#what-is-in-it) · [Full install guide](docs/INSTALL.md) · [Architecture](docs/ARCHITECTURE.md) · [CLI](docs/CLI.md)

</div>

---

Specline stores and organises everything about a long-running AI development
project: specs, PRDs, roadmaps, tasks and feature requests. You work with Claude
Code or Codex the way you already do, and Specline records it as you go.

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

**Your data stays on your disk.** Everything lives in `~/.specline`, with no
account and no cloud behind it. The daemon listens on `127.0.0.1`, so nothing off
your machine can reach it. One thing does leave: the daemon checks for a new
release every half hour, sending nothing from your store and installing nothing
without you agreeing to the restart. `--no-update-check` at install time or
`SPECLINE_AUTO_UPDATE=0` afterwards turns that off, and then Specline makes no
network requests at all.

**You get readable files.** Specline writes markdown into your repository, where
you can grep it and diff it and commit it beside the code it describes. If
Specline went away tomorrow, the files would still be there.

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
the plugin also installs two session hooks: one puts a summary of the project at
the top of every conversation, and one asks a session that recorded nothing
whether it should have.

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

## Using it

### Mostly you do not

These get Claude writing:

> "Let's go with the second option — Postgres, because we already run one."
> "That's a bug, the retry loop doesn't back off."
> "I don't know whether we need per-tenant keys. Leave it for now."

These get it reading:

> "What's the state of the auth work?"
> "Why did we pick SQLite?"
> "What's blocking the release?"
> "What should I do next?"

### What that looks like

Partway through a conversation about the board, you say:

> "Let's not add a new task type for that — a label already does it."

Nothing else happens. You carry on. Specline has a decision, and this one is real
— it is [B-25](product/DECISIONS.md), recorded by the session that had the
conversation:

> **"Waiting on a human decision" is the decision-needed label, not a new task kind**
>
> The bootstrap already used the label, so the data existed. A new `TaskKind`
> would be a schema change to express something a label expresses. The cost is
> that it is a convention: nothing enforces it, and a decision task without the
> label ranks as ordinary work.

The cost is in there because you would want it in six months, and because the
session that wrote it had just finished arguing about it. That is the difference
between this and a row that says "use a label".

### The app

```bash
specline ui
```

The daemon serves the app itself, compiled into the binary, so there is no Node
and nothing else to start.

A board, with what to pick up next at the top, grouped by whether it is in an open
phase and saying why each one is where it is:

![The board, with a ranked "next" strip above the columns](docs/images/board.png)

Documents that keep their reasoning. Requirements are anchored, so a task can point
at one requirement instead of a whole spec, and each document shows the decision
behind it alongside the tasks doing the work:

![A spec with requirement anchors and a panel of connected decisions and tasks](docs/images/document.png)

**The app files things and Claude writes them.** Creating a task, commenting,
closing, archiving, and moving a task's status, priority, kind, phase or labels
are all your own actions. The body of a spec or a decision gets written by Claude,
in the conversation where you worked it out. There are two moves the app will not
make: closing needs a reason, a message and evidence, so it opens a form asking
for them; and starting a task is a claim, which has to name the conversation doing
the work, so the board asks you to have Claude pick it up. The full boundary is in
[what the app may write](docs/ARCHITECTURE.md#what-the-app-may-write).

### The command line

You will not need it often. Four are worth knowing:

```bash
specline doctor      # has anything gone wrong? every read-only check, one page
specline next        # what to work on next
specline generate    # write the markdown into your repo
specline backup      # snapshot the store; `restore` puts it back
```

All of it works whether or not the daemon is running.

**All 24 commands are in [docs/CLI.md](docs/CLI.md).**

### Getting the most out of it

**Talk about the project rather than dictating records.** "We're going with
Postgres because we already run one" gets you a decision with a reason in it,
where "Create a decision record titled Postgres" gets you a row that means nothing
in six months.

**Say why out loud.** What you rejected, and why, is the part you will want later.

**Use the short IDs.** Tasks are `KEEL-42` and decisions are `B-12`, they do not
change, and they work anywhere an ID is accepted.

**Leave open questions open.** Every session sees them before it starts, which
stops Claude quietly re-deciding something you settled.

---

## What is in it

Thirteen kinds of thing, and that is the limit. "We need a new type for this"
nearly always turns out to be a field or a label.

**project**, **milestone**, **task**, **spec**, **decision**, **question**,
**term**, **feedback**, **design**, **environment**, **metric**,
**metric observation**, **artifact**.

A typed graph joins them, where a task implements a spec, a decision supersedes an
older one, and a task blocks another. That graph is how you ask what is blocked.

An agent sees [thirteen tools](docs/ARCHITECTURE.md#the-mcp-surface), because a
model picks well from a short list.

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
with one file each, and a document can take a path of its own — tell Specline that
a spec lives at `docs/SPEC.md` and that is where it goes from then on.

**These files are output**, and each one says so at the top. The next
`specline generate` writes over anything you change. To change what they say,
change the source: ask Claude to rewrite it, or edit it in the app. If you have
already edited a file by hand and want the words kept, `specline import <file>`
puts them back as a proper revision.

To catch a hand edit before it lands, put this in `.git/hooks/pre-commit`:

```bash
#!/bin/sh
specline generate <your-project> --check
```

---

## How it is built

Rust, one workspace, six crates, one SQLite file, and a daemon that owns the only
write path. Search combines FTS5 keyword matching with `sqlite-vec` similarity,
and every change is an event carrying an author and the conversation it came from.

This repository runs on it: [product/DECISIONS.md](product/DECISIONS.md) and
[product/JOURNAL.md](product/JOURNAL.md) are generated from the store.

**[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** has the crate layout, how storage
works, why the direction of a graph query is the easiest thing to get wrong, and
what the app may and may not write. **[CONTRIBUTING.md](CONTRIBUTING.md)** covers
building it yourself.

---

<div align="center">
<sub>Apache-2.0 · <a href="https://github.com/kiritbasu/specline/issues">Issues</a> · <a href="docs/INSTALL.md">Install</a> · <a href="docs/ARCHITECTURE.md">Architecture</a> · <a href="docs/CLI.md">CLI</a></sub>
</div>
