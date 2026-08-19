# The Specline plugin

Four pieces, and each instruction has exactly one of them as its author. Three
files used to say the same three things in slightly different words, and two of
them contradicted each other about the session identity — so a session could
follow the skill, write correctly, and be told at the end that it had recorded
nothing.

| Piece | What it does | Owns |
|---|---|---|
| `.mcp.json` | Points Claude at the local daemon's MCP endpoint. | — |
| `specline hook session-start` | Injects the digest before the first word. | **The session identity**, and "record it, don't offer to". |
| `skills/specline/SKILL.md` | Teaches Claude *what* belongs where. This is the load-bearing part. | **When to write, and what to write.** |
| `specline hook stop` | Speaks only to a session that recorded nothing. | **The end-of-session check**, in one sentence. |
| `hooks/specline-hook.sh` | Execs the two above, or says the binary is missing. | **Nothing.** It is the only part that must run without Specline. |

The session identity is the hook's because Claude Code already assigns one and
the model inventing its own produced collisions: two date-based ids landed on
the same string and a run of ten sessions scored five as three. The two
instructions that live in the hook rather than the skill are there because a
skill is *model-invoked* and thirty headless sessions with `specline` installed
invoked it zero times. An instruction in a file nobody opens is not an
instruction.

The daemon is the machinery. **The skill is the product.** If Claude has to be
reminded to use Specline every session, the whole idea fails — which is why Phase 2
is a real phase and not an afterthought (PRD R-2).

---

## Install

```bash
./plugin/install.sh
```

That builds the binaries, installs them to `~/.cargo/bin` — where a release
installs too, so a dev build replaces the released one rather than shadowing it
— creates the store at `~/.specline`, copies the skill and hooks to
`~/.claude/skills/specline/`, and prints what to add to your Claude Code
configuration.

**After editing anything under `plugin/`, re-run it:**

```bash
./plugin/install.sh --skill-only
```

That skips the build and copies the three files, reporting which of them
changed. It matters more than it sounds: the copies under `~/.claude` are what
actually run, and when they were made by hand they drifted from the repository
within a day — a plugin edit landing inert, with nothing anywhere to say so
(TQ-26). A full release build to copy three files was the friction that made
people skip it.

The one thing `install.sh` will not do is edit `~/.claude/settings.json`. That
file is yours; `~/.claude/skills/specline/` is Specline's.

Start the daemon:

```bash
specline-daemon
```

It binds `127.0.0.1:7654` and stays there. On the first start it downloads the
embedding model — 127 MB, once — so that search matches by meaning as well as by
word, and it embeds anything already in the store that has no vector yet. Search
answers from the keyword half throughout, including while the model is on its
way. `--no-embeddings` turns all of that off.

Nothing supervises it. There is no launchd job and no `specline restart` — if it
stops, start it again the same way.

## Updating

```bash
specline update
```

It fetches the latest release, checks its SHA-256 against the release manifest,
replaces both binaries, and then asks the running daemon to restart into the new
version — reporting which version came back rather than assuming. `--check` says
what would happen and changes nothing; `--rollback` puts the previous binaries
back, one generation only.

Two things it will not do on its own:

- **Cross a schema version.** A release that changes the store's shape stops and
  tells you what to run, because a migration rewrites your data and `--rollback`
  puts binaries back, not rows.
- **Update a daemon installed somewhere else.** It writes beside the `specline` you
  ran, so a daemon started from a different directory is untouched. It says so
  when that happens, rather than reporting a restart that changed nothing.

---

## Phase 2's exit criterion, and how to run it

> Across 10 unprompted sessions, Claude writes to Specline in ≥9, threads
> `session_id` on every write, and creates 0 duplicate projects.

**This is the one gate that cannot be automated, and it is the one that matters
most.** "Unprompted" is the entire claim. A test that calls the tool has, by
definition, prompted it. Nothing in the test suite touches this.

### How to run it honestly

Do ten ordinary sessions of real work across at least two projects. Do not
mention Specline. Do not say "remember to record that". Just work — talk through a
feature, fix a bug, decide something, take a customer call.

Then score it:

```bash
specline gate --since <the moment you started>
```

That reports, per session: whether it wrote, whether every write carried a
`session_id`, and whether any near-duplicate projects appeared. It excludes the
sentinel writers (`ses_bootstrap`, `ses_import`, …) — they write and they thread
an id, so counting them would make `specline import` ten times a passing grade. Under
ten sessions it reports `INCOMPLETE` and exits 0: not a pass, and not a fail
either.

*(The three commands previously documented here did not work. `specline activity`
was never a command, and `specline fsck` and `specline status` open the store directly,
so they fail while the daemon holds the write lock. The gate that mattered most
had no instrument — see TQ-15.)*

### Two things that will make it fail for the wrong reason

**Run the sessions somewhere other than the Specline repo.** `CLAUDE.md` here is
four hundred lines telling Claude what Specline is and to keep it updated. A session
started in this repository is about as prompted as a session gets.

**Install the skill and register the server for every project**, or the sessions
have nothing to fire:

```bash
./plugin/install.sh --skill-only
claude mcp add --scope user --transport http specline http://127.0.0.1:7654/mcp
```

`scripts/gate-run.sh` does all of this: it checks the preconditions, builds two
throwaway projects that mention Specline nowhere, runs ten ordinary-sounding
sessions across them, and scores the result. **Run it from your own terminal** —
`claude -p` reports "Not logged in" when spawned from inside a Claude Code
session, so this is not something the agent can run for you.

### What the failure modes look like

| Symptom | What it means |
|---|---|
| Claude reads but never writes | The skill's triggers are too narrow, or the "write when something becomes true" table is not landing. |
| Writes appear, but `session_id` is null | The skill is being read but the threading instruction is being skipped. Move it earlier. |
| Forty tasks where eight would do | The consolidation section is losing to the model's instinct to be helpful. Strengthen it. |
| A second project for something that exists | The `specline_projects`-first instruction is not firing. This is the most damaging one — it quietly ruins the cross-project view. |

Each of those is a fix to `SKILL.md`, not to the daemon. Change the wording, run
another ten sessions.

---

## Editing a generated file

There is no hook that captures it. There used to be — `PostToolUse` intercepted
an edit to a generated file and tried to turn it into an attributed revision —
and it did not work: it called `specline mirror`, which had been renamed to
`specline generate` underneath it, and swallowed the failure; and it read
`SPECLINE_SESSION_ID`, which nothing sets. Every edit it claimed to capture was
lost, and the guarantee written here — "the database wins unconditionally
afterwards, the file is regenerated" — was untrue for as long as it was written
down.

What replaced it fails loudly instead. `scripts/pre-commit` refuses a commit
carrying a generated file that does not match what Specline would produce, and says
where to make the change instead:

```bash
ln -sf ../../scripts/pre-commit .git/hooks/pre-commit
```

A mechanism that silently does not work is worse than no mechanism, because it
gets relied upon. This one does nothing except notice.

Files under `product/` and `.keel/` are outputs. To change one, change what
generates it — the prose in Specline, or the task rows — and run
`specline generate <project>`.

### Requirements

`jq` and `curl`. Both are almost certainly already installed.

---

## If the daemon is not running

Every tool call fails with a connection error, and the hook says so and gets out
of the way rather than failing your edit. Start it:

```bash
specline-daemon
```

Check it:

```bash
curl -s http://127.0.0.1:7654/api/health | jq
```
