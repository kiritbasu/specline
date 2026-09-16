# Installing, running and removing Specline

The [README](../README.md) covers the Claude Code install, which is three
commands. This file has everything else: the full Codex walkthrough, running two
editors against one store, managing the service, and uninstalling.

- [Codex, in full](#codex-in-full)
- [Running both editors at once](#running-both-editors-at-once)
- [Stopping and starting the daemon](#stopping-and-starting-the-daemon)
- [Uninstalling](#uninstalling)

---

## Codex, in full

Codex has no plugin, so the three things the Claude Code plugin does — start the
daemon, register the MCP server, install the hooks — are separate steps here.
None of them needs Claude Code.

### First, check that `codex` runs

Two of the steps below are CLI commands. If you installed Codex as the ChatGPT
desktop app there is no `codex` on your `PATH`, because the binary sits inside
the app bundle.

```bash
command -v codex || ls /Applications/ChatGPT.app/Contents/Resources/codex
```

If only the second half printed anything, put a wrapper on your `PATH`:

```bash
printf '#!/bin/sh\nexec "/Applications/ChatGPT.app/Contents/Resources/codex" "$@"\n' \
  > ~/.local/bin/codex && chmod +x ~/.local/bin/codex
```

**Do not use a symlink here.** Codex finds its helper executables,
`codex-code-mode-host` among them, relative to the binary it was launched as, so
a symlink sends it hunting for siblings in your `bin` directory that only exist
inside the app bundle. Code Mode then fails and takes tool calls down with it,
reporting a missing host. Running `exec` on the absolute path makes the bundled
binary the running process, and the siblings resolve.

Use a directory that is on your `PATH`; `~/.local/bin` is common but not
universal, and `echo $PATH` settles it.

The same wrapper is the answer if `command -v codex` finds an older install
from npm or Homebrew. The desktop app's bundled binary is usually newer, and
the two `--version` outputs tell you which you have. The MCP step below needs
one that knows `--url`.

### 1. Start the daemon

This is the same script `/specline:setup` runs, and it does not care which editor
you use.

```bash
git clone https://github.com/kiritbasu/specline.git
```

```bash
./specline/plugin/scripts/setup.sh
```

The clone is only how you get the script. It downloads the released binaries, so
you do not need Rust.

Skip both if Specline is already installed. One daemon serves every client, and a
second would fight the first for the store.

### 2. Point Codex at it

```bash
codex mcp add specline --url http://127.0.0.1:7654/mcp
```

No token and no headers. The daemon binds `127.0.0.1` only, and rejects any
request carrying an `Origin` from another machine.

Older Codex builds only understood servers launched as a subprocess. If yours
rejects `--url`, add `experimental_use_rmcp_client = true` under `[features]` in
`~/.codex/config.toml`, or upgrade; `codex mcp add --help` says whether `--url`
is there.

### 3. Install the hooks

There is no command for this one, so it goes in `~/.codex/config.toml` by hand:

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

Write the path out in full rather than using `~`, which TOML does not expand.
`command -v specline` prints the path to paste.

### 4. Trust the hooks, from the terminal

Run `codex` with no arguments to get the interactive CLI, then type `/hooks` and
approve both entries. They show as *"New hook — review required"* until you do.

`/hooks` belongs to that CLI and is **not** in the ChatGPT desktop app's `/`
menu, which lists skills, so typing it there finds nothing. Trust is recorded
against a hash of each hook, which means granting it once in the terminal applies
wherever Codex runs.

**Do not skip this step.** Codex skips any hook it has not been shown, with no
error and no warning. If Specline seems installed but your sessions start with no
project summary, this is almost always the reason. Editing a hook's command
changes its hash and needs `/hooks` again.

Then restart Codex, since MCP servers connect at startup.

### 5. Check it

One command proves all three pieces at once:

```bash
codex exec --sandbox read-only "Call the specline_projects tool and reply with just the number it returned."
```

A working install prints four things: `hook: SessionStart`, then
`mcp: specline/specline_projects started` and `completed`, then a number, then
`hook: Stop`. Anything missing tells you which piece is not wired: no `hook:`
lines means the hooks are not trusted, and a tool the model reports as
unavailable means the MCP server is not connected.

---

## Running both editors at once

You can. One daemon holds the store and every client is an HTTP client of it, so
there is never a second writer. Two Claude Code windows already work this way,
and it is tested with sixteen concurrent sessions.

Claims work across editors, so a task claimed by a Codex session is refused to
Claude Code, which is told which session holds it. The daemon's rate limit is one
budget shared by everything connected, generous enough that only a runaway loop
reaches it.

---

## Stopping and starting the daemon

**`kill` does not work, and that is deliberate.** The daemon runs under a service
manager that restarts it, `KeepAlive` on macOS and `Restart` on Linux, so killing
the process brings it straight back. Go through the service manager instead.

```bash
launchctl bootout gui/$(id -u)/sh.specline.daemon      # stop, macOS
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/sh.specline.daemon.plist
```

```bash
systemctl --user stop specline.service                 # stop, Linux
systemctl --user start specline.service
```

To restart in one step, after replacing the binaries:

```bash
launchctl kickstart -k gui/$(id -u)/sh.specline.daemon   # macOS
systemctl --user restart specline.service                # Linux
```

Stopping the daemon does not stop Specline. The CLI opens the store directly when
nothing is listening, so `specline status`, `search` and the rest keep working;
what stops is the MCP surface your agent talks to, and the app.

**Restart the daemon after upgrading.** Specline will not start if the binary is
older than the store's schema.

---

## Uninstalling

One command, the same way installing is:

```bash
curl -fsSL https://github.com/kiritbasu/specline/releases/latest/download/specline-uninstall.sh | sh
```

It stops the service through the service manager, removes it, and removes both
binaries. It ships as a release asset with its checksum in
`specline-release.json`, so you can download and read it before running it. For a
script whose job is deleting things, do that.

From a clone it is `./plugin/scripts/uninstall.sh` with the same flags, and
`--dry-run` either way prints what it would do without doing it.

**Your store is kept.** `~/.specline` holds every decision, question and note
Specline has recorded, nothing else on disk has a copy of it, and reinstalling
picks it up where it was. If the schema has moved in between, the daemon
migrates the store forward on its first start; there is no separate step and
nothing to reset. The other direction is refused: a binary older than the
store will not open it. Removing the store takes a separate flag:

```bash
curl -fsSL https://github.com/kiritbasu/specline/releases/latest/download/specline-uninstall.sh | sh -s -- --purge
```

`--purge` backs the store up to your home directory before deleting it, using
`specline backup` if the binary is still there and a directory copy if it is not,
and it refuses to delete anything it could not first copy.

**It leaves your editor's configuration alone** and prints what to remove:
`/plugin uninstall specline` in Claude Code, and `codex mcp remove specline` plus
deleting the two `[[hooks.*]]` blocks from `~/.codex/config.toml` in Codex.
