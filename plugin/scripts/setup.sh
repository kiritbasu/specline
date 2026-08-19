#!/usr/bin/env bash
#
# `/specline:setup` — get from "the plugin is installed" to "Specline is running".
#
# ## Why this is a script and not a list of steps in a prompt
#
# A slash command is a prompt. Anything written there as "then run this, then
# run that" is executed non-deterministically, asks permission at every step,
# and does something slightly different each time. So the command's whole body
# is one line that runs this file, and every decision that matters lives here,
# where it can be read and tested.
#
#   /specline:setup                    what the slash command runs
#   plugin/scripts/setup.sh        the same thing, by hand
#   plugin/scripts/setup.sh --dry-run    say what would happen, change nothing
#
# ## What it does, in order
#
#   1. Refuses early if something is already listening on the port and it is
#      not Specline.
#   2. Downloads and verifies the release for this platform.
#   3. Creates the store, and migrates one that already exists.
#   4. Installs a service so the daemon comes back after a reboot.
#   5. Starts it, and waits until it actually answers.
#
# ## What leaves your machine
#
# One thing: a half-hourly GET of the latest release manifest, so the daemon can
# tell you a new version exists. It sends nothing from your store — not a
# project name, not a count, not an identifier. `--no-update-check` turns it off
# at install time, `SPECLINE_AUTO_UPDATE=0` turns it off afterwards, and `specline
# doctor` reports which it is and when the last check ran.
#
# Said out loud in the output rather than left here, because a tool whose pitch
# is that your project's history stays on your machine has to be the one that
# mentions its own network request. Finding it yourself, later, is the version
# of this that costs trust (KEEL-204).
#
# ## The port is fixed, deliberately
#
# An earlier plan had this resolve a collision by moving to the next free port
# and writing the result into the plugin config. That is the opposite of what
# the daemon does, and the daemon is right: it refuses a busy port rather than
# wandering, because the plugin's MCP entry is written at install time and read
# at startup, so a daemon that quietly moved to 7655 would leave the config
# stale and MCP would fail with nothing to explain it. A wandering port and a
# static configuration file cannot both be right.
#
# So a busy port is a refusal with instructions, not a silent relocation.

set -uo pipefail

REPO="${SPECLINE_REPO:-kiritbasu/specline}"
PORT="${SPECLINE_PORT:-7654}"
# The release installer's own default — `CARGO_HOME`, falling back to
# `~/.cargo/bin`. Kept identical to `plugin/install.sh` deliberately: two
# install paths meant two copies of every binary, one shadowing the other on
# PATH, and a daemon serving a different build from the CLI beside it
# (KEEL-234).
BIN_DIR="${SPECLINE_BIN_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}"
SPECLINE_HOME_DIR="${SPECLINE_HOME:-$HOME/.specline}"
DAEMON_URL="http://127.0.0.1:$PORT"

DRY_RUN=false
# Whether the binary carries an embedding model at all, learned from
# /api/health once the daemon answers. Assumed absent until it says otherwise.
EMBEDDINGS_BUILT_IN=false
# On by default since B-95. It was opt-in, and nobody opted in — including the
# person who wrote it — so every install had keyword search wearing the name of
# a hybrid one. `--no-embeddings` is the way out, and the first start downloads
# 127 MB.
EMBEDDINGS=true
INSTALL_SERVICE=true
# The one thing Specline does that leaves this machine, and the one thing somebody
# installing a local-first tool would want to be asked about. On by default and
# off with a flag, disclosed either way — see "What leaves your machine" below.
UPDATE_CHECK=true

for arg in "$@"; do
    case "$arg" in
        --dry-run)     DRY_RUN=true ;;
        --embeddings)  EMBEDDINGS=true ;;
        --no-embeddings) EMBEDDINGS=false ;;
        --no-service)  INSTALL_SERVICE=false ;;
        --no-update-check) UPDATE_CHECK=false ;;
        -h|--help)     sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *)             echo "setup: unknown argument: $arg" >&2; exit 2 ;;
    esac
done

step()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
ok()    { printf '  \033[32m✓\033[0m %s\n' "$*"; }
info()  { printf '    %s\n' "$*"; }
fail()  { printf '  \033[31m✗\033[0m %s\n' "$*" >&2; }

die() {
    fail "$1"
    shift
    for line in "$@"; do printf '    %s\n' "$line" >&2; done
    exit 1
}

run() {
    if [ "$DRY_RUN" = true ]; then
        info "would run: $*"
        return 0
    fi
    "$@"
}

# --- 0. is the port already taken, and by what? -----------------------------
#
# Asked first, because everything below is wasted if the answer is "something
# else is on 7654" — and because a Specline already running is a *success*, not a
# collision. Re-running setup is the ordinary thing someone does when they are
# not sure it worked, and it must not punish them.

step "Checking the port"

health="$(curl -sf --max-time 3 "$DAEMON_URL/api/health" 2>/dev/null)"
if [ -n "$health" ] && printf '%s' "$health" | grep -q '"status"'; then
    ok "a Specline daemon is already answering on $PORT"
    info "setup will reinstall the binaries and restart it"
    ALREADY_RUNNING=true
elif command -v nc >/dev/null 2>&1 && nc -z 127.0.0.1 "$PORT" 2>/dev/null; then
    die "something is listening on $PORT and it is not Specline." \
        "Specline does not pick another port: the plugin's MCP entry names this one" \
        "and is read when Claude Code starts, so a daemon that moved would leave" \
        "the config stale with nothing to explain the failure." \
        "" \
        "Stop whatever is on $PORT, or set SPECLINE_PORT and pass the same value to" \
        "the daemon and to the MCP config."
else
    ok "port $PORT is free"
    ALREADY_RUNNING=false
fi

# --- 1. the binaries --------------------------------------------------------

step "Installing the binaries"

installer_url="https://github.com/$REPO/releases/latest/download/specline-installer.sh"

# **A token does not make the ordinary download URL work.**
#
# This is the bug the first real install on a second machine hit, and it is
# worth writing down because the intuition is wrong. For a private repository
# `https://github.com/OWNER/REPO/releases/download/...` returns 404 *even with a
# valid Bearer token* — GitHub serves private release assets only through the
# API, at `/repos/OWNER/REPO/releases/assets/{id}` with
# `Accept: application/octet-stream`. Measured both ways: 404 with the token,
# 404 without it, 200 through the API.
#
# So the earlier version of this step was correct in intent and pointed at a URL
# that cannot work, and every private-repo install would have hit the same wall
# with a message blaming the token.
#
# It is two downloads, not one. Fetching the installer through the API is half
# the problem: the installer then fetches the *archive* from the same shape of
# URL. Both have to come through the API, which is why the assets are pulled
# into a directory and the installer is pointed at it with `SPECLINE_DOWNLOAD_URL` —
# the generator's own override, and the same route `verify-release-tier1.sh`
# uses to test a release before it is published.
#
# `gh` does the API download rather than curl plus a JSON parser. Asset ids have
# to be looked up by name, and parsing JSON in shell is what put `python3` on
# the critical path of the session hooks — a dependency absent from a clean Mac,
# failing silently. Not making that mistake twice.
download_via_api() {
    local dest="$1"
    command -v gh >/dev/null 2>&1 || return 1
    gh auth status >/dev/null 2>&1 || return 1
    # `--pattern '*'` is not decoration: without a tag argument `gh release
    # download` refuses unless given `--pattern` or `--archive`, and the refusal
    # is an exit 1 that looks exactly like "no access" from a caller that only
    # checks the status. No tag, so this always takes the latest release.
    gh release download --repo "$REPO" --dir "$dest" --pattern '*' --clobber \
        >/dev/null 2>&1
}

if [ "$DRY_RUN" = true ]; then
    info "would download and run $installer_url"
else
    install_log="$(mktemp)"
    assets="$(mktemp -d)"

    if download_via_api "$assets" && [ -f "$assets/specline-installer.sh" ]; then
        # Private or public, this route works for both, so it is tried first
        # rather than kept as a fallback — the failure it avoids is silent.
        info "fetched the release through the GitHub API"
        if SPECLINE_DOWNLOAD_URL="file://$assets" sh "$assets/specline-installer.sh" \
            >>"$install_log" 2>&1
        then
            ok "binaries installed"
        else
            die "the installer failed. Log: $install_log"
        fi
    elif curl --proto '=https' --tlsv1.2 -LsSf "$installer_url" 2>"$install_log" \
        | sh >>"$install_log" 2>&1
    then
        ok "binaries installed"
    else
        # Say which of the causes it is rather than printing curl's exit code.
        if grep -qiE '404|not found' "$install_log" 2>/dev/null; then
            die "the release could not be downloaded (404)." \
                "" \
                "If this repository is private, the plain download URL returns 404" \
                "even with a valid token — private assets are only served through" \
                "the API, and reaching them needs the GitHub CLI:" \
                "" \
                "  brew install gh && gh auth login" \
                "" \
                "Otherwise no release has been published yet." \
                "" \
                "Log: $install_log"
        fi
        die "the installer failed. Log: $install_log"
    fi
fi

specline_bin="$BIN_DIR/specline"
daemon_bin="$BIN_DIR/specline-daemon"

if [ "$DRY_RUN" = false ]; then
    # The installer's own default is `$CARGO_HOME/bin`, falling back to
    # `~/.cargo/bin`, so both are looked for and `CARGO_HOME` comes first —
    # anyone with it set puts the binary somewhere none of the other candidates
    # name, and this found a *different* `specline` further down the list instead.
    # An install that reports the version of a binary it did not install is
    # worse than one that fails.
    # `~/.local/bin` was on this list until KEEL-234. It was never a place a
    # release installs — it was where `plugin/install.sh` used to put dev
    # builds — so searching it meant a stale development copy could be found
    # and reported as the install. One place a release writes, one place this
    # looks.
    for candidate in "${CARGO_HOME:+$CARGO_HOME/bin}" "$BIN_DIR" "$HOME/.cargo/bin"; do
        [ -n "$candidate" ] || continue
        if [ -x "$candidate/specline" ] && [ -x "$candidate/specline-daemon" ]; then
            specline_bin="$candidate/specline"
            daemon_bin="$candidate/specline-daemon"
            break
        fi
    done
    [ -x "$specline_bin" ] || die "specline is not where the installer said it would be" \
        "Looked in: $BIN_DIR, $HOME/.cargo/bin"
    ok "specline $("$specline_bin" --version 2>/dev/null | awk '{print $2}') at $specline_bin"
fi

# --- 2. the store -----------------------------------------------------------

step "Preparing the store"

if [ "$DRY_RUN" = true ]; then
    info "would create or migrate $SPECLINE_HOME_DIR"
elif [ -f "$SPECLINE_HOME_DIR/keel.sqlite" ]; then
    # An existing store may be behind this binary. Migrating is the daemon's
    # precondition, not an optional tidy-up — it refuses to open a store newer
    # than itself and will not silently upgrade one that is older.
    if [ "$ALREADY_RUNNING" = true ]; then
        info "a daemon is holding the store; it will be stopped before migrating"
        stop_daemon_for_migrate=true
    fi
    ok "store exists at $SPECLINE_HOME_DIR"
else
    # `--daemon "$DAEMON_URL"`, never the default, and this is the second thing
    # the first real install got wrong.
    #
    # Read commands go *through* a daemon when one answers, and `--daemon`
    # defaults to 127.0.0.1:7654. So on a machine that already runs Specline, this
    # asked the live daemon about the store it serves, got a cheerful exit 0
    # about somebody else's data, and created nothing here — then the check
    # below failed with "the store was not created", which is true and explains
    # nothing. Pointing it at the port being set up means it opens this store
    # directly when nothing is listening, which is the case on a clean machine
    # and the case that matters.
    run "$specline_bin" --home "$SPECLINE_HOME_DIR" fsck --daemon "$DAEMON_URL" >/dev/null 2>&1
    if [ -f "$SPECLINE_HOME_DIR/keel.sqlite" ]; then
        ok "store created at $SPECLINE_HOME_DIR"
    else
        die "the store was not created at $SPECLINE_HOME_DIR" \
            "The daemon creates one on first start, so this is recoverable —" \
            "but something is wrong if opening it directly did not."
    fi
fi

# --- 3. stop anything already running, then migrate -------------------------

if [ "$DRY_RUN" = false ] && [ "$ALREADY_RUNNING" = true ]; then
    step "Stopping the running daemon"
    pkill -f "specline-daemon" 2>/dev/null
    for _ in $(seq 1 10); do
        curl -sf --max-time 1 "$DAEMON_URL/api/health" >/dev/null 2>&1 || break
        sleep 1
    done
    ok "stopped"
fi

if [ "$DRY_RUN" = false ]; then
    step "Applying migrations"
    if "$specline_bin" --home "$SPECLINE_HOME_DIR" migrate --daemon "$DAEMON_URL" >/dev/null 2>&1; then
        ok "store is at the schema this binary ships"
    else
        info "nothing to migrate, or the store is already current"
    fi
fi

# --- 4. the service ---------------------------------------------------------
#
# So the daemon survives a reboot. Everything Specline does depends on it being up,
# and "start it yourself after every restart" is a step people stop doing.

install_launchd() {
    local plist="$HOME/Library/LaunchAgents/sh.specline.daemon.plist"
    # Written into the service's own environment, not a shell profile: the
    # daemon is started by launchd at login and never reads one.
    local update_env=""
    [ "$UPDATE_CHECK" = false ] && update_env="
        <key>SPECLINE_AUTO_UPDATE</key><string>0</string>"
    local args="<string>$daemon_bin</string>"
    [ "$EMBEDDINGS" = false ] && args="$args
        <string>--no-embeddings</string>"

    mkdir -p "$HOME/Library/LaunchAgents"
    cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>sh.specline.daemon</string>
    <key>ProgramArguments</key>
    <array>
        $args
    </array>
    <key>KeepAlive</key>
    <dict><key>SuccessfulExit</key><false/></dict>
    <key>RunAtLoad</key><true/>
    <key>StandardOutPath</key><string>$SPECLINE_HOME_DIR/daemon.log</string>
    <key>StandardErrorPath</key><string>$SPECLINE_HOME_DIR/daemon.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>SPECLINE_HOME</key><string>$SPECLINE_HOME_DIR</string>
        <key>SPECLINE_BIND</key><string>127.0.0.1:$PORT</string>$update_env
    </dict>
</dict>
</plist>
PLIST
    launchctl unload "$plist" 2>/dev/null
    launchctl load "$plist" 2>/dev/null
    info "launchd agent at $plist"
}

install_systemd() {
    local unit="$HOME/.config/systemd/user/specline.service"
    local update_env=""
    [ "$UPDATE_CHECK" = false ] && update_env="Environment=SPECLINE_AUTO_UPDATE=0"
    local exec="$daemon_bin"
    [ "$EMBEDDINGS" = false ] && exec="$exec --no-embeddings"

    mkdir -p "$HOME/.config/systemd/user"
    cat > "$unit" <<UNIT
[Unit]
Description=Specline daemon
After=network.target

[Service]
Environment=SPECLINE_HOME=$SPECLINE_HOME_DIR
Environment=SPECLINE_BIND=127.0.0.1:$PORT
$update_env
ExecStart=$exec
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
UNIT
    systemctl --user daemon-reload 2>/dev/null
    systemctl --user enable --now specline.service 2>/dev/null
    info "systemd user unit at $unit"
}

if [ "$INSTALL_SERVICE" = true ]; then
    step "Installing the service"
    if [ "$DRY_RUN" = true ]; then
        info "would install a $([ "$(uname -s)" = "Darwin" ] && echo launchd agent || echo systemd user unit)"
    elif [ "$(uname -s)" = "Darwin" ]; then
        install_launchd
        ok "the daemon will start at login"
    elif command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
        install_systemd
        ok "the daemon will start at login"
    else
        # Not fatal. A machine with no user systemd session can still run Specline;
        # it just has to be started by hand, and saying so is better than
        # failing an install over it.
        fail "no supported service manager found — start the daemon yourself:"
        info "$daemon_bin"
        INSTALL_SERVICE=false
    fi
fi

# --- 5. start it, and prove it answers --------------------------------------

step "Starting the daemon"

if [ "$DRY_RUN" = true ]; then
    info "would start the daemon and wait for /api/health"
else
    if [ "$INSTALL_SERVICE" = false ]; then
        embed_flag=""
        [ "$EMBEDDINGS" = false ] && embed_flag="--no-embeddings"
        auto_update=1
        [ "$UPDATE_CHECK" = false ] && auto_update=0
        SPECLINE_HOME="$SPECLINE_HOME_DIR" SPECLINE_AUTO_UPDATE="$auto_update" \
            nohup "$daemon_bin" --bind "127.0.0.1:$PORT" $embed_flag \
            >>"$SPECLINE_HOME_DIR/daemon.log" 2>&1 &
    fi

    answered=false
    for _ in $(seq 1 20); do
        if health="$(curl -sf --max-time 2 "$DAEMON_URL/api/health" 2>/dev/null)"; then
            answered=true
            break
        fi
        sleep 1
    done

    # Ask the daemon what it can do rather than reporting what we asked for.
    # Every *released* binary is built without the embedding model — no
    # prebuilt ONNX Runtime exists for Intel macOS, so the release ships the
    # feature off on all three platforms (KEEL-252, KEEL-349). Printing
    # "Embeddings on" at somebody who has just downloaded one would be this
    # script promising a capability the binary does not have, which is worse
    # than the capability being absent.
    case "$health" in
        *'"built_in":true'*|*'"built_in": true'*) EMBEDDINGS_BUILT_IN=true ;;
        *) EMBEDDINGS_BUILT_IN=false ;;
    esac

    if [ "$answered" = true ]; then
        ok "answering on $DAEMON_URL"
    else
        die "the daemon did not answer within 20 seconds." \
            "Log: $SPECLINE_HOME_DIR/daemon.log"
    fi
fi

# --- done -------------------------------------------------------------------

step "Done"
printf '  Store      %s\n' "$SPECLINE_HOME_DIR"
printf '  Daemon     %s\n' "$DAEMON_URL"
printf '  Interface  specline ui\n'
if [ "$EMBEDDINGS_BUILT_IN" = false ]; then
    printf '  Embeddings %s\n' \
        "not in this build — search matches words, not meaning"
else
    printf '  Embeddings %s\n' \
        "$([ "$EMBEDDINGS" = true ] \
            && echo "on — the first start downloads a 127 MB model" \
            || echo "off — keyword search works either way")"
fi
printf '  Updates    %s\n\n' \
    "$([ "$UPDATE_CHECK" = true ] \
        && echo "checks every half hour for a new release — see below" \
        || echo "off — Specline makes no network requests at all")"

# Said plainly, every install, whichever way it is set. A tool whose pitch is
# that your project stays on your machine has to be the one that mentions its
# own network request; discovering it later is the version that costs trust.
if [ "$UPDATE_CHECK" = true ]; then
    printf '  \033[1mWhat leaves your machine:\033[0m one request every half hour, fetching the\n'
    printf '  latest release manifest so Specline can tell you a new version exists. It\n'
    printf '  sends nothing from your store — no project names, no counts, no identifier.\n'
    printf '  Turn it off with SPECLINE_AUTO_UPDATE=0, or re-run this with\n'
    printf '  --no-update-check. `specline doctor` says which it is and when it last ran.\n\n'
fi
# `printf`, not a heredoc: a heredoc does not interpret escapes, so the bold
# sequence printed literally as \033[1m — in the one line that most needs to be
# read, which is a fair demonstration of why the dry run exists.
printf '  \033[1mRestart Claude Code now.\033[0m MCP servers are connected at startup,\n'
printf '  and nothing was listening when this session began — so the specline_* tools\n'
printf '  will not appear until you do.\n\n'

# Said plainly, and only to the person it is true of. A downloaded binary
# cannot do this at all, and finding that out from a thin search result later
# is the version that costs trust.
if [ "$EMBEDDINGS_BUILT_IN" = false ]; then
    printf '  \033[1mThis build searches by keyword only.\033[0m Released binaries carry no\n'
    printf '  embedding model: the ONNX runtime it needs has no build for Intel macOS,\n'
    printf '  so the release ships without it on every platform. Every artifact is still\n'
    printf '  searchable, and a search tells you which halves of it ran. Building from\n'
    printf '  source is the way to get the other half today.\n\n'
elif [ "$EMBEDDINGS" = false ]; then
    printf '  Search will be keyword-only. For meaning as well as words, re-run\n  without --no-embeddings (the first start downloads a 127 MB model).\n\n'
fi

exit 0
