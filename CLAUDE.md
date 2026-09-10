# Specline

Local-first store for everything that describes a software project other than the code — specs, decisions, tasks, roadmap, design, feedback — with an MCP server as the primary interface and a local web app, served by the daemon, as the read surface.

The standing rules are imported here so they load in every session regardless of working directory:

@.claude/CONTRACT.md

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
