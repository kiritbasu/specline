---
name: specline
description: Use for any conversation about an ongoing software project — specs, decisions, tasks, roadmap, bugs, customer feedback, open questions, what shipped — and for any session that is about to *do* some of that work. Read from Specline at the start of such a conversation, claim a task before starting it, and write to it whenever something is decided, planned, learned, or asked and left unanswered. Triggers on "what's the state of", "what did we decide about", "add a task", "we should", "let's go with", "I spoke to a customer", "why did we", "what's blocking", "let's build", "start on", "work on", "implement", "what should I pick up", or any mention of a project by name.
---

# Specline

Specline is where everything about a software project lives except the code: specs,
decisions, tasks, milestones, questions, risks, customer feedback, the glossary,
what is deployed.

You are the main way anything gets in or out. There is a desktop app, but it is
for reading. If you do not write to Specline, nothing does.

---

## You are already oriented

A `SessionStart` hook has put Specline's digest into this conversation before you
read anything — what the project is, the active milestone, what is urgent and
what is blocked, recent decisions, every open question, the glossary.

You did not have to ask for it and you do not have to fetch it. This exists
because relying on the model to load a skill did not work: across thirty
headless sessions and an interactive one, this file was never opened, so
everything in it was advice nobody read (TQ-19).

Two things follow:

- **Do not re-litigate what the digest already settles.** If a decision is
  listed, it is decided. If a question is open, it is open — do not answer it
  as though it were new.
- **Use the glossary's words.** The digest carries them because a project's
  vocabulary is the cheapest thing to get wrong and the most annoying.

Call `specline_context` yourself only when the conversation moves to a *different*
project, or when you need `depth: "full"` because the digest reported that it
trimmed something you need. Pass `cwd` when you do.

If the digest said no project matches this directory, read
"Before creating a project" below — the short version is that you create the
first one and say so.

---

## Put the work on the board before you do it

A row, and `specline_claim` on it *before* you start rather than when you finish.
Two calls at most, and they are the only way a human watching can see what is
happening **now** rather than only what has already landed.

This is the single most-skipped thing in Specline, and it has been measured three
times.
The first time: across sixty-six tasks, the number of transitions into
`in_progress` before work began was **zero** — which is why claiming stopped
being something to remember and became a tool. The second time, on a fresh
project in August 2026, a session was told to build an application, worked for
hours, and left every task in `todo`. Both times the work was fine and the
board was a lie.

The third time was the one that named the gap. Every instruction here was
followed — and it only covered work that already had a row. A request that
arrives as a sentence has none, so there was nothing to claim and the board sat
idle through four commits.

So:

- **If there is no row for what you were just asked to do, make one.** Most
  work arrives as a sentence — "cut the release", "this message is confusing",
  "add the export button" — not as something already filed. `specline_create` it,
  `specline_claim` it, then start. One line of summary is enough; what a person
  watching reads is that the row exists and who is on it, not how well it is
  written. Every other bullet here assumes the task is already there, and this
  is the one that makes that true.
- **`specline_next` is what to ask when the choice is open.** "Build the app" is
  not a task; it is a request to work through several. Ask what to pick up and
  it answers with the ranking Specline actually computes — by what a task unblocks
  before its priority — rather than whatever the digest happened to show you.
  It costs a fraction of a full digest and takes filters: unclaimed, by label,
  by milestone.
- **`specline_claim` before the first edit.** Doing several tasks in a row means
  claiming each one as you reach it, not claiming nothing because there were
  many. If a claim is refused, another session holds it — that is the tool
  working.
- **`specline_close` when it is genuinely done**, with a reason, a message and at
  least one piece of evidence. Never delete a task and never leave one
  `in_progress` across sessions without a note saying where it got to.
- **Found something on the way?** `specline_note` it on the task. A status is a
  colour; the note is what the next session actually needs.

If you do nothing else from this file, do this. Everything below is about
recording what you learned; this is about the work being visible while it is
still happening.

---

## Keep the board current

A person reads the board to learn the state of the project. They must not need
to ask you. So update a row when a fact changes, not at the end of the session.

| When this happens | Do this |
|---|---|
| You start work | The task row exists and you claimed it (see above). |
| You learn something | Add a note to the task. |
| The scope changes | Update the task summary, or split the task and link the parts. The summary must describe the work you do now. |
| You are blocked | Draw a `blocks` edge from the blocker to the task, and add a note. |
| You finish | Close the task with a reason, a message and evidence. |
| Every task in a milestone is closed | Find out if the milestone shipped (see below). |
| Something ships | Mark the milestone shipped, and update the environment if a version changed. |
| The session ends | Each task you touched is closed, or has a note that says where you stopped. |

### Milestones and the roadmap

The roadmap derives a milestone's state from its tasks. When every task is
closed, the milestone shows as `complete`. `complete` is not `shipped`: the work
is done, but nobody has said that it was delivered.

- **When it shipped**, `specline_update` the milestone to `status: shipped`.
  The store sets `shipped_at` to now if you leave it out. Give `shipped_at` when
  the real date is earlier, for example the time a release was published. Then
  change the summary to say what shipped.
- **When it was dropped**, set `status: cut`. Record why in a note or a decision.
- **When it stopped but will come back**, set `status: paused`.
- **A release** is a milestone with `kind: release` and a `version_string`.
  Write it, with its summary, before you tag. The summary is the release notes,
  so the tracker, the changelog and the published notes use the same words.

Do not leave a milestone `complete` for many sessions. If you do not know
whether it shipped, ask the human. That is a question about a fact, not a
request for permission.

### When to summarise

- **At the end of a session**, in the conversation: two lines. What landed, and
  what is next.
- **When the human asks for the state, the roadmap or the backlog**, read it
  from Specline with `specline_context` or `specline_next`. Do not answer from
  memory. Group by milestone: what shipped, what is active, what is blocked and
  by what. Link each row. If a list was cut, give the total.
- **When a task closes**, the close message is the summary. Write it for a
  person who did not see the work.

---

## Name an artifact as a link, not as a string

When a tool result carries a `url`, use it: write `[KEEL-42](the url)` rather
than `KEEL-42`. Claude Code renders markdown links in the terminal and in the
desktop app, so that one change is the difference between somebody reading about
a task and somebody being able to open it.

The URL comes from the daemon, never from you. It knows the address it actually
bound — which is not 7654 for anyone running a second daemon — and it knows
which types have a screen. A link you compose from a template is wrong on both
counts sooner than you would think, and a link that opens the wrong page is
worse than plain text, because it reads as the interface being broken.

`specline_get`, `specline_create`, `specline_claim`, `specline_close` and `specline_next` return a
`url` on the artifacts they name. Nothing else does: a digest listing forty rows
would spend more tokens on links than on the rows. If there is no `url`, the
artifact has no screen or nothing is serving the interface — say the reference
plainly and do not invent an address.

---

## Thread a session id through every call

The session-start hook tells you which identifier to use, and it is the one
Claude Code assigned this conversation. Use exactly that on every Specline call,
read and write. Do not invent one, and do not derive one from the date.

Pass `surface` too: `code` in Claude Code, `chat` in Claude chat, `cowork` in
Cowork.

This matters more than it looks. Specline's provenance guarantee is that every
change can be traced to the conversation that made it, and MCP has no protocol
session to borrow — so the identifier has to come from outside the protocol.
Specline accepts a write without one rather than refusing it, but the change is then
attributed only to "some Claude session", which is nearly useless a month later
when the human is asking why something changed.

`specline_context` echoes the `session_id` back. If it comes back `null`, you are
not threading it — fix that before writing anything else.

If the hook did not run — no identifier appeared at the start of this
conversation — write without one rather than making one up. An invented id looks
like provenance and is not: it joins to no transcript, and two sessions that
both invent a date-based one collide silently. That happened, and it made a run
of ten sessions score five as three.

---

## Do not ask permission to record. Record, and say that you did.

This is the single behaviour that fails the gate. Measured, not guessed: of ten
unprompted sessions, seven wrote nothing — and five of those seven had already
worked out exactly what should be recorded, drafted it, and then stopped to ask.

> *"This looks like a real open risk for Tideline and it isn't tracked yet —
> want me to log it as an open question in Specline? I'll hold off until you say
> so."*

> *"Want me to log the open design question so it's not lost? I'll hold off
> until you say go."*

Both are wrong, and wrong in a way that feels like good manners. The human is
mid-conversation about the code. They do not want a second decision about
bookkeeping; they want the thing not to be lost. Asking converts a free write
into an interruption, and an interruption they ignore into a lost record.

**Write it. Then say so in one line and carry on:**

> Logged that as an open question on Tideline — the datum type may not match
> the source chart.

The reasoning to apply is *"did something become true?"*, not *"have I been
authorised?"* If a decision was made, a risk surfaced, a task agreed, feedback
heard — that already happened. Recording it is describing the conversation, not
acting on the human's behalf.

**The exceptions are narrow, and they are about correctness, not permission:**

- **You are not sure what was decided.** Then it has not become true yet. Ask
  about the *substance* — "are we going with blake3, or parking it?" — not about
  whether to record.
- **Creating a project when a similar one already exists.** Covered below; that
  is a duplicate-data risk, not a politeness question.

Nothing else. In particular, do not ask because the thing seems small, because
the human seems busy, or because you are not certain they want a tracker. They
installed one.

---

## Write when something becomes true

Not at the end of the conversation. Not when asked. When it happens.

| When the human… | Write |
|---|---|
| decides something, or agrees to an approach | a **decision** — with the context and what was rejected, not just the choice |
| describes work to be done | a **task** — or several, if it is genuinely several |
| asks something nobody can answer yet | a **question** — this is the one everyone forgets |
| worries that something might go wrong | a **question** with `kind: risk` |
| describes what to build, at length | a **spec** |
| relays what a customer said | **feedback** — verbatim in the body, not your summary of it |
| uses a domain word in a way you had to infer | a **term** — cheap to add, and it stops the next session guessing |
| says something shipped | update the **milestone**, and the **environment** if a version changed |

The two most valuable and most-skipped are **questions** and **decisions**.
A question that evaporates when the conversation ends gets re-asked in three
weeks. A decision without its reasoning gets re-argued.

### Record the reasoning, not just the outcome

"Chose DuckDB" is nearly worthless. What is worth writing:

> ## Context
> We need relational queries over mutable rows, semantic search over prose, and
> multimodal blobs.
>
> ## Decision
> DuckDB for entities, Lance for documents and blobs.
>
> ## Consequences
> Both are native Rust crates, so no sidecar process. Lance is young and is the
> one unhedged dependency, which is why the Parquet export exists.

In six months neither you nor the human will remember why. That paragraph is
the whole point of writing it down.

---

## How to write: Simplified Technical English

Everything you write into Specline is read later, by a person who was not in
this conversation. Write it in Simplified Technical English (STE).

### The rules

1. **One idea per sentence.** Keep most sentences under 20 words.
2. **Use the active voice.** Say who or what does the action: "The daemon
   refuses the write", not "The write is refused".
3. **Use common words, each in one meaning.** Use the glossary term for a
   thing, and use the same term every time.
4. **Say the fact.** No filler, no praise, no hedging. Do not write "it is
   important to note", "robust", "seamless" or "comprehensive".
5. **Be specific.** Give the number, the file, the command or the error text:
   "Startup takes 4 s", not "startup is slow".
6. **Put the main point first.** Put the reason after it.
7. **Write what is true.** Do not write what you intend to write.
8. **Keep other people's words as they said them.** Customer feedback, error
   messages and quoted documents stay verbatim, in a block quote or a code
   span. Do not rewrite them into STE.

The store refuses some filler phrases and warns about others. The warning comes
back with a write that succeeded. If you get a refusal or a warning, rewrite the
sentence. Do not swap the word for a synonym and keep the same sentence.

### Each kind of row

**Task summary.** One or two sentences: what is wrong or wanted, what it
affects, and what done looks like.

- Bad: "Improve the board filter to provide a more robust experience."
- Good: "The board filter resets when the page reloads, so you lose your view.
  Done when the filter survives a reload."

**Close message.** What changed, and where the evidence is. Say what you did
not do.

- Bad: "Done! Implemented the feature comprehensively."
- Good: "The filter is now in the URL, so a reload keeps it. Tested in
  `board_filter.rs`. The mobile layout is not changed."

**Note.** One finding, and how you know it.

- Bad: "Investigated the issue further."
- Good: "The router clears query parameters on navigation. That is the cause,
  not the store. Seen in the debug log."

**Decision.** The context, the decision, the consequences, and what you
rejected and why.

- Bad: "We decided to go with SQLite as it is the best option."
- Good: "Use SQLite. Rejected Postgres: it needs a server process, and this is
  a local tool for one user."

**Question.** One sentence that ends in "?". Then what depends on the answer,
the options, and what you recommend.

- Bad: "Thoughts on caching?"
- Good: "Do we cache the digest per project? Each session start now costs
  300 ms. Options: cache it against the latest event, or do nothing. I
  recommend nothing until it costs more than 1 s."

**Spec.** What and why, the requirements, and what is out of scope. Give each
requirement a number (REQ-1, REQ-2), so a task can link to one. One requirement
per line. Use "must".

- Bad: "The export should be flexible and support a variety of formats."
- Good: "REQ-1: The export must write one CSV file for each project."

**Milestone or release summary.** One or two sentences, 280 characters at most.
Say what the phase delivers. When it ships, say what shipped.

- Bad: "A pivotal phase that lays the groundwork for future success."
- Good: "Each task shows its milestone on the board. Done when no open task is
  without one."

**Feedback.** The customer's words, verbatim. Your reading goes in a linked
spec or note, not in the body.

---

## Before creating a project, ask

**Always call `specline_projects` first.** It fuzzy-matches on name, slug, aliases
and repository URL.

If it comes back with `requires_confirmation: true` — meaning something that
*looks like* this project already exists — **stop and ask the human**:

> I don't see an exact match. There is "Harbour" (`harbour`) which looks close —
> is this the same thing, or should I create a new project called
> *Harbour Billing*?

Nine near-identical projects is the failure that quietly ruins the cross-project
view, and merging them afterwards is far more work than asking now.

### But do not stall on an empty store

**If nothing resembles it at all, create the project and get on with it.** Say
that you did, in one line, and carry on:

> Nothing in Specline matched this repository, so I've created the project
> **Tideline** and recorded the decision under it.

The rule above exists to stop you creating a *second* project for something that
already exists. Creating the *first* one for a directory Specline has never seen is
not that failure, and treating it as though it were has a cost that was measured
rather than guessed: in the ten-session gate, **nine sessions understood exactly
what should be recorded, said so, and wrote nothing** — because there was no
project to write into and they were waiting for permission that a working
session never pauses to give.

Pass `cwd` to `specline_context` and it will tell you outright whether any project
owns the directory you are in. "No project matches this directory" means create
one. It does not mean stop.

---

## Consolidate. Do not shred.

A project with forty trivial tasks that should be eight is worse than useless —
the human stops reading the list, and then stops trusting it.

- One task per meaningful unit of work, not per step you imagine.
- "Add the login page" is a task. "Create the file", "add the route", "write the
  test" are not — they are how you would do it.
- Long-form detail belongs in a **spec**, linked from the task with `implements`,
  not in twelve task bodies.
- If you find yourself creating a fifth task in one turn, stop and ask whether
  it is really one task with a spec behind it.

Creates are idempotent, so a retry is safe: calling twice with the same project,
type and title returns the existing artifact with `created: false` rather than
duplicating it. Capitalisation and spacing are normalised, so "Add login page"
and "add  Login  Page" are the same task.

---

## Link things, and get the direction right

Direction reads left to right: **`from` does the verb to `to`.**

| Say it like this | Not like this |
|---|---|
| task **implements** spec | spec implements task |
| blocker **blocks** the thing waiting | the waiting thing blocks its blocker |
| newer decision **supersedes** older | older supersedes newer |
| decision **resolves** question | question resolves decision |
| feedback **informs** spec | spec informs feedback |
| spec **derives from** feedback | feedback derives from spec |

If "A depends on B" is the natural way to say it, use `depends_on` — Specline stores
it the right way round and tells you it did.

Use `anchor` to link to one requirement inside a spec rather than the whole
document:

```
specline_link(from: task_id, rel: "implements", to: spec_id, anchor: "REQ-4")
```

That is what makes "is this spec actually built?" answerable requirement by
requirement instead of as a yes/no guess.

---

## Updating: pass the version you read

`specline_update` needs the `version` from when you read the artifact. `specline_get`
returns it at the top level of the entity, so it is a straight copy.

If someone else changed it in between, you get a 409 carrying the current state
and the events since your read. **Merge and retry** — do not clobber, and do not
give up:

1. Look at `current_state` in the error.
2. Decide whether your change still makes sense against it.
3. Re-send with the new `latest_version`.

Most conflicts resolve themselves this way without troubling the human.

---

## Things to avoid

- **Don't invent a `session_id`.** Use the one the session-start hook gave you,
  or none at all.
- **Don't create a project without asking.** See above.
- **Don't write a task for every step.** See above.
- **Don't edit an accepted decision.** Supersede it with a new one linked by
  `supersedes`. Specline will refuse the edit and tell you this.
- **Don't summarise customer feedback into the body.** Put the verbatim words
  there and your reading in the linked spec. The verbatim version is the part
  that stays useful.
- **Don't use `specline_update` to change a document body.** That is
  `specline_write_doc`, which versions it. `specline_update` is for the fields around
  it: title, status, kind.
- **Don't ask permission to write.** If something was decided, write it down.
  Writing is cheap, reversible (nothing is ever deleted), and the whole point.

---

## When you are unsure whether something is worth recording

Record it. Nothing in Specline is ever deleted — archiving is a soft delete — so the
cost of writing something that turns out not to matter is close to zero, and the
cost of losing a decision is a re-litigated argument in three weeks.

The exception is the shredding failure above: one meaningful artifact beats five
trivial ones.

---

## The tools

Counted in the title until this table was wrong twice — it said nine while
listing ten, and omitted three others entirely. The number is in
`tools::all()`, where it cannot go stale.

| Tool | Reach for it when |
|---|---|
| `specline_context` | starting any project conversation — **first**, always |
| `specline_next` | "what should I work on" — the ranking, not a guess |
| `specline_claim` | **before** starting a task, every time |
| `specline_close` | it is finished — with a reason, a message and evidence |
| `specline_search` | "what do we know about X", "has this come up before" |
| `specline_get` | you have an id, or you want the graph around something |
| `specline_projects` | before creating a project; resolving a name |
| `specline_activity` | "what changed since I last looked" |
| `specline_create` | anything new — including the task for what you were just asked to do |
| `specline_update` | status, priority, fields |
| `specline_write_doc` | the prose body of a spec, decision, question or feedback |
| `specline_link` | connecting two artifacts |
| `specline_note` | you learned something — a finding, a gotcha, why it was harder than expected |

Each tool's own description says more. Read them.
