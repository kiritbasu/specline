#!/usr/bin/env bash
#
# Take Specline off this machine.
#
# The counterpart to `setup.sh`, and it exists for the same reason: a list of
# steps written in prose gets run non-deterministically, in the wrong order,
# with one of them skipped. The README used to carry nine commands here. One of
# them was `rm -rf ~/.specline`.
#
#   plugin/scripts/uninstall.sh              the service and the binaries
#   plugin/scripts/uninstall.sh --dry-run    say what would happen, change nothing
#   plugin/scripts/uninstall.sh --purge      the store too, backed up first
#
# ## What it removes
#
#   1. The service — a launchd agent on macOS, a systemd user unit on Linux.
#   2. The `specline` and `specline-daemon` binaries.
#   3. The store, only with `--purge`, and only after backing it up.
#
# ## What it deliberately does not touch
#
# **Your editors' configuration.** `install.sh` already refuses to edit
# `settings.json` on the way in — "rewriting someone's settings from a shell
# script is the kind of helpfulness that is indistinguishable from damage the
# one time it gets it wrong" — and the same holds on the way out. Removing an
# MCP server is one clean command per editor and this prints them.
#
# **The store, unless asked.** This is the asymmetry that shapes the whole
# script: a bad install is repeated, a bad uninstall is gone. `~/.specline`
# holds every decision, question and note ever recorded, and nothing else on
# disk holds a copy. So it survives by default, and `--purge` backs it up
# before removing it rather than trusting that somebody meant it.

# `set -u` and no `pipefail`, deliberately. This is meant to be runnable as
# `curl … | sh`, and `/bin/sh` is dash on Debian and Ubuntu, where
# `set -o pipefail` is not an option and the script dies before doing
# anything, with `Illegal option`. macOS hides that completely: its `/bin/sh`
# is bash in POSIX mode and accepts it. No pipeline here has an exit status
# worth checking, so
# the option was decoration that only broke the platforms it was never tested
# on. Everything below is POSIX; the bash shebang is for running it directly.
set -u

BIN_DIR="${SPECLINE_BIN_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}"
SPECLINE_HOME_DIR="${SPECLINE_HOME:-$HOME/.specline}"
PLIST="$HOME/Library/LaunchAgents/sh.specline.daemon.plist"
UNIT="$HOME/.config/systemd/user/specline.service"
# Matches setup.sh's default. Overridable so a test can point the whole
# script at a scratch HOME rather than at the machine it runs on.
PORT="${SPECLINE_PORT:-7654}"
DAEMON_URL="http://127.0.0.1:$PORT"

DRY_RUN=false
PURGE=false

for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=true ;;
        --purge)   PURGE=true ;;
        -h|--help) sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *)         echo "uninstall: unknown argument: $arg" >&2; exit 2 ;;
    esac
done

step() { printf '\n\033[1m%s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
info() { printf '    %s\n' "$*"; }
skip() { printf '  \033[2m·\033[0m %s\n' "$*"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$*" >&2; }

# `would` in a dry run, past tense otherwise. One helper so no message can
# describe something the script did not do.
did() { if [ "$DRY_RUN" = true ]; then info "would $1"; else ok "$2"; fi; }

step "Stopping the daemon"

# Through the service manager, never `kill`. The agent carries `KeepAlive` and
# the unit carries `Restart=always`, so a killed daemon comes straight back and
# the next step removes the binaries out from under a running process.
if [ "$(uname -s)" = "Darwin" ]; then
    if [ -f "$PLIST" ]; then
        [ "$DRY_RUN" = true ] || launchctl bootout "gui/$(id -u)/sh.specline.daemon" 2>/dev/null
        did "stop and unload the launchd agent" "the launchd agent is stopped"
    else
        skip "no launchd agent at $PLIST"
    fi
elif command -v systemctl >/dev/null 2>&1; then
    if [ -f "$UNIT" ]; then
        [ "$DRY_RUN" = true ] || systemctl --user disable --now specline.service 2>/dev/null
        did "stop and disable the systemd unit" "the systemd unit is stopped"
    else
        skip "no systemd unit at $UNIT"
    fi
else
    skip "no service manager found; nothing to stop"
fi

# A daemon started by hand answers no service manager, so say so rather than
# removing its binary while it runs.
#
# Not in a dry run: nothing was stopped, so of course something is still
# answering, and warning about it there is the script complaining about a state
# it chose. The first version did exactly that.
if [ "$DRY_RUN" = false ] \
    && curl -fsS -m 2 "$DAEMON_URL/api/health" >/dev/null 2>&1; then
    fail "something is still answering at $DAEMON_URL"
    info "a daemon started by hand is not managed by launchd or systemd."
    info "stop it before continuing, or its binary goes while it is running."
fi

step "Removing the service definition"
for file in "$PLIST" "$UNIT"; do
    if [ -f "$file" ]; then
        [ "$DRY_RUN" = true ] || rm -f "$file"
        did "remove $file" "removed $file"
    fi
done
if [ ! -f "$PLIST" ] && [ ! -f "$UNIT" ] && [ "$DRY_RUN" = false ]; then
    skip "nothing left to remove"
fi

step "Removing the binaries"
found_binary=false
for name in specline specline-daemon; do
    path="$BIN_DIR/$name"
    if [ -e "$path" ]; then
        found_binary=true
        [ "$DRY_RUN" = true ] || rm -f "$path"
        did "remove $path" "removed $path"
    fi
done
[ "$found_binary" = true ] || skip "no binaries in $BIN_DIR"

step "The store"
if [ ! -d "$SPECLINE_HOME_DIR" ]; then
    skip "no store at $SPECLINE_HOME_DIR"
elif [ "$PURGE" = false ]; then
    ok "kept at $SPECLINE_HOME_DIR"
    info "Every decision, question and note Specline holds is in there, and"
    info "nothing else on disk has a copy. Reinstalling picks it up again."
    info "To remove it too: $0 --purge"
elif [ "$DRY_RUN" = true ]; then
    info "would back up $SPECLINE_HOME_DIR and then remove it"
else
    # Back up before removing, with the binary that is about to be deleted —
    # which is why this runs before the `rm` and not after. A `--purge` that
    # deleted first and then found it could not back up would be the one
    # mistake in this script nobody could undo.
    backup_dir="$HOME/specline-backup-$(date -u +%Y%m%dT%H%M%SZ)"
    if [ -x "$BIN_DIR/specline" ]; then
        "$BIN_DIR/specline" backup --dest "$backup_dir" >/dev/null 2>&1 \
            && ok "backed up to $backup_dir"
    fi
    if [ ! -d "$backup_dir" ]; then
        # No binary left, or the backup failed. Copy the directory wholesale
        # rather than proceed: a coarse copy beats no copy.
        cp -R "$SPECLINE_HOME_DIR" "$backup_dir" 2>/dev/null \
            && ok "copied the store to $backup_dir" \
            || { fail "could not back up $SPECLINE_HOME_DIR — stopping here"
                 info "remove it by hand once you have a copy you trust."
                 exit 1; }
    fi
    rm -rf "$SPECLINE_HOME_DIR"
    ok "removed $SPECLINE_HOME_DIR"
fi

step "Two things this script will not do for you"
info "Your editors' configuration is yours, so it is left alone:"
info ""
info "  Claude Code   /plugin uninstall specline"
info "  Codex         codex mcp remove specline"
info "                then delete the [[hooks.SessionStart]] and [[hooks.Stop]]"
info "                blocks naming specline from ~/.codex/config.toml"

if [ "$DRY_RUN" = true ]; then
    step "Nothing was changed"
    info "That was a dry run. Drop --dry-run to do it."
fi
