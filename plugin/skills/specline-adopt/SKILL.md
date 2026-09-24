---
name: specline-adopt
description: Use when putting an existing project into Specline for the first time — a repository with history, docs, ADRs and a backlog that predate Specline. Triggers on "adopt Specline", "set Specline up for this repo", "backfill Specline", "import our docs into Specline", "start using Specline on this project", or a Specline project that exists but is empty while the repository plainly is not. Not for the everyday loop; the `specline` skill covers that.
---

# Adopting Specline on a project that already exists

A new project starts empty and fills up as work happens. An existing one arrives
with years of it — specs, ADRs, a README, a backlog, decisions that live in
someone's head and nowhere else. This is how to get the useful part of that into
Specline without getting the rest.

**The failure mode is enthusiasm.** Not laziness, not missing something — putting
too much in. A repository will offer you hundreds of candidates and every one of
them will look reasonable in isolation. Read the rest of this before you create
anything.

---

## Why too much is worse than too little

`specline_context` is the first thing every future session reads. It is budgeted at
roughly 3–4k tokens and it returns the open questions and the glossary **in
full, never trimmed**. So every speculative question and every half-term you add
is paid for again at the start of every conversation anyone has about this
project, for as long as it stays open.

A missing task costs one person one minute when they notice. A register full of
questions nobody will answer costs every session a slice of its budget and
teaches the human to skim the digest — and once they skim it, the store has
stopped being the thing they trust.

Judge this work by what you left out. When you report at the end, say what you
left out and why, in those words.

---

## The order

### 1. Look before you touch anything

Do not create the project yet, and do not import anything.

Read the repository: the README, whatever is in `docs/`, any `adr/` or
`decisions/` directory, the issue templates, the contributing guide, the test
layout. Get the shape of it. Then say what you found and what you propose,
roughly:

> This looks like a Rust service with about 40k lines and two years of history.
> I found 6 documents worth keeping as specs, 14 ADRs, a README with a
> terminology section worth 9 glossary terms, and roughly 20 open issues that
> would make about 8 tasks. I would skip the 60-odd `TODO` comments — I read a
> sample and they are mostly stale. Does that match how you think about it?

Then wait. This is the step that makes the difference, because the human knows
which of those documents is dead and you do not.

### 2. Create the project, and get the paths right

`specline_projects` first to check it does not already exist, then `specline_create`.

Set `root_path` to the repository, and set it correctly — it is what every
generated file is written relative to later. Getting it wrong is quiet and
annoying to undo.

### 3. Import the documents mechanically

Anything that is already a markdown document should go in as one. Do not paste
prose into task bodies or retype a spec into `specline_write_doc`.

```bash
specline import docs/architecture.md docs/api.md --project <slug> --as spec --dry-run
```

Read the dry run. It tells you what would be created, what would be revised,
what would be left alone, and — when it would change — the path the artifact
would claim. That path is what `specline generate` later writes back over, so a
surprise there costs someone a file.

Then run it without `--dry-run`. Stop the daemon first, or pass `--force` if you
know what you are doing: import is a direct write and refuses while a daemon
holds the store.

ADRs go in the same way with `--as decision`. One file, one decision.

### 4. Now do the judged part

Tasks, glossary terms, milestones and the questions that are genuinely open.
This is the part no parser can do, which is why you are doing it.

- **Tasks**: from the real backlog — issues, a `TODO.md` someone maintains, what
  the human tells you is next. One task per meaningful unit of work. If you are
  creating a fifth task in one turn, stop and ask whether it is one task with a
  spec behind it.
- **Glossary**: only words this project uses in a way an outsider would get
  wrong. Not every noun in the README.
- **Milestones**: only if the project actually works in phases and someone can
  name them. An invented roadmap is worse than none.
- **Questions**: only ones that are genuinely open *and* that someone intends to
  answer. See below.

Write what you author here in Simplified Technical English, as the `specline`
skill describes: short sentences, one idea each, no filler. A task summary says
what is wrong or wanted, what it affects, and what done looks like. Imported
documents are different. Import them as they are, and do not rewrite them.

**Look for decisions outside the decisions directory.** This is the part that
most repays reading rather than parsing. Surveying one real repository for this
skill turned up nine tidy ADRs in `decisions/` — and a further set buried in
task notes in `BACKLOG.md`, lines like `RESOLVED (founder 2026-07-18): query
logs are a first-class scan input…`, reasoning attached, nowhere near a file
called a decision. A parser pointed at `decisions/` finds nine and misses those.
Check the backlog, the meeting notes, the long comments in the README, and any
file where someone was arguing with themselves.

### 5. Generate, and check

`specline generate <slug>` writes the mirror. Then look at the diff: it is the first
time the human sees what you decided, in a form they can read.

---

## What not to bring

**Stale `TODO` comments.** Read a sample before deciding. In most repositories
they are years old, refer to code that has moved, and were never a commitment.
A `TODO` that a person will actually do is already in the backlog.

**Closed work that produced no decision.** A merged pull request is not a
decision. A decision is a choice that constrains what happens next and that
someone would otherwise re-litigate. If nothing would go wrong from not knowing
it, leave it out.

**Git history as events.** The event log is Specline's own audit trail of writes *to
Specline*. Filling it from commits does not record history, it fabricates an audit
trail — and every provenance claim in the store gets quieter for it. There is no
tool for this; do not build one.

**Questions nobody is going to answer.** "Should we use gRPC?" from a design doc
in 2023 is not an open question, it is an artefact of one. Open means someone
intends to decide it. Everything else is noise in the digest for ever, and the
digest is where it hurts most.

**Anything you are inferring rather than reading.** If you find yourself writing
"this was probably decided because…", stop. Ask, or leave it out. A confident
decision record that nobody made is the single worst thing you can put in this
store, because it will be believed.

---

## Provenance is why this is a session and not a script

Everything you write carries your `session_id` and your actor. That is honest:
a real session made these judgements, and someone reading later can see which.

A script that inferred the same rows would have to mark them as derived rather
than asserted, because rows that look attributed and are not dilute the one
thing Specline is for. You do not have that problem, and it is the reason the
judged half is done this way — so do not undermine it by asserting things you
guessed at.

Imported documents are different and fine: they carry `Actor::Human`, truthfully,
because a person wrote the file.

---

## What "done" looks like

- The digest is worth reading. Open it with `specline_context` and see for yourself.
  If it is over budget on day one, you put too much in.
- Every open question is one someone means to answer.
- Every task is a unit of work, not a step.
- The generated mirror is a diff the human can read.
- You can say what you left out, and why.

That last one is not a formality. If you cannot name anything you left out, you
did not make any judgements, and the whole reason this is not a parser is that
judgements were needed.
