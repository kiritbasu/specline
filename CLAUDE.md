# Specline

Local-first store for everything that describes a software project other than the code — specs, decisions, tasks, roadmap, design, feedback — with an MCP server as the primary interface and a local web app, served by the daemon, as the read surface.

The standing rules are imported here so they load in every session regardless of working directory:

@.claude/CONTRACT.md

## How to work: agents and models

KB's standing instruction, 2026-09-17. This is about how a session spends its effort, and it sits here rather than in the contract because it is about the session's own tooling, not the product.

- **Parallelise with subagents whenever the work splits.** A five-axis review, a test-writing pass, a search across the codebase, and the build you are in the middle of are four jobs that do not need to wait on each other. Run them as agents in the background and keep the conclusion, not the transcript. One agent per independent job; do not fan out for a task that is one file and one thought.
- **Pick the model for the job, not the biggest one.** Sonnet or Haiku for searching, reading, summarising, running a suite and reporting, writing a test against a spec that is already clear. Opus for the design, the review, and anything where a wrong answer is expensive to notice. Say which you chose when it is not obvious.
- **Fable is for the hardest problems.** When a problem is genuinely difficult — a design with no clean option, a bug that has survived two attempts, a storage-format or graph-direction question — put it to a Fable agent, and use Fable as the advisor when stuck: state what has been tried, what the evidence says, and what the choice is. Do not spend Fable on routine work.
- **The contract's definition of done still applies to every agent's output.** An agent's report is a claim; the checks and the review are what make it true. Read what came back before building on it.

## Where the rest of the documentation is

Everything else is generated from the store and is **not tracked in git**, because it is internal reasoning rather than anything a reader of this repository needs. On a fresh clone it will not be there. Run `specline generate specline` to write it:

- `product/STATUS.md` — the tracker; open work only
- `product/CHANGELOG.md` — what has closed, with the reason and the evidence
- `product/DECISIONS.md` — build-time decision log
- `product/JOURNAL.md` — what happened, session by session
- `product/PRD.md` and `product/SPEC.md` — what and why, and how
- `product/HANDOFF.md` — orientation, read once
- `product/GATE.md` — the unprompted-write measurement, and why it is frozen
- `.specline/questions.md` — every question and risk, open and settled
- `.specline/specs/` and `.specline/decisions/` — one file each

`.specline/glossary.md` is the exception and is tracked, because the project's vocabulary is useful to anyone reading the code.

The daemon serves all of it without the files existing at all, so `specline_context`, `specline_search` and the app work on a clean checkout before anything is generated.
