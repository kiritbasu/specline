#!/bin/sh
#
# The only shell left, and it stays for one reason: a hook that *is* the binary
# cannot report the binary's absence.
#
# Between installing the plugin and running `/specline:setup` there is no `specline` on
# the machine, and the session should say so rather than starting silently
# unoriented. That sentence has to come from something that runs without Specline —
# so it comes from here, and everything that can change is on the other side of
# the `exec`.
#
# Deliberately dependency-free: `sh`, not `bash`; no `python3`, no `curl`, no
# `jq`. That is the whole point of KEEL-206 — the scripts this replaces needed
# python3 and curl, neither declared anywhere, and python3 is absent on a Mac
# until the Xcode command line tools arrive. Every failure path exited 0
# silently, so on a fresh machine the hooks did nothing and it looked exactly
# like Specline being broken.
#
# Nothing here holds logic worth testing. Everything that does is in
# `specline hook`, with `crates/specline/tests/hooks.rs` running it for real.
#
# Usage: specline-hook.sh session-start | stop | commit

set -u

event="${1:-}"
[ -n "$event" ] || exit 0

# Where the installer puts it, then whatever is on PATH. `command -v` rather
# than a hardcoded path so a non-standard install still works.
#
# `~/.local/bin/specline` used to be first on this list, and it was the wrong place:
# no release has ever written there, only the old default of
# `plugin/install.sh`. So a machine with both had its session hook running a
# development build while everything else ran the released one (KEEL-234). One
# location, and it is the one a release installs to.
specline="${SPECLINE_BIN:-}"
if [ -z "$specline" ]; then
    for candidate in "${CARGO_HOME:-$HOME/.cargo}/bin/specline" "$HOME/.cargo/bin/specline"; do
        [ -x "$candidate" ] && specline="$candidate" && break
    done
fi
[ -n "$specline" ] || specline="$(command -v specline 2>/dev/null || true)"

if [ -n "$specline" ] && [ -x "$specline" ]; then
    # Run rather than `exec`, so a binary that is present but too old to know
    # `hook` cannot speak for this script.
    #
    # That state is not hypothetical — it is exactly what an upgrade looks like
    # between updating the plugin and updating the binary. `exec` hands clap's
    # "unrecognized subcommand" straight to Claude Code, and for a Stop hook a
    # non-zero exit means *block, using stderr as the reason* — so a stale
    # binary would inject a usage message as a blocking instruction. Swallowing
    # it costs one buffered string and turns that into silence.
    output="$("$specline" hook "$event" 2>/dev/null)" || exit 0
    [ -n "$output" ] && printf '%s\n' "$output"
    exit 0
fi

# No binary. Say so once, at session start, and say nothing at all on stop —
# a session that is ending is the wrong moment to be told about installation,
# and `Stop` output would block it.
[ "$event" = "session-start" ] || exit 0

printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"The Specline plugin is installed but the `specline` binary is not on this machine yet, so this session has no project context and the specline_* tools will not answer. Run /specline:setup to install it, then restart Claude Code."}}'
