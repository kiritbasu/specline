#!/usr/bin/env bash
#
# Build Specline, install the binaries and the skill, and print the Claude Code
# configuration.
#
# Two kinds of file live under ~/.claude, and this script treats them
# differently on purpose.
#
#   settings.json is *yours*. This script never edits it. Rewriting someone's
#   settings from a shell script is the kind of helpfulness that is
#   indistinguishable from damage the one time it gets it wrong, so this prints
#   what to add and lets you paste it.
#
#   ~/.claude/skills/specline/ is *Specline's*. Its contents are this repository's
#   files and nothing else authors them, so copying them there is installation
#   rather than interference.
#
# That distinction is why TQ-26 exists. The skill and hooks were hand-copied
# once and then drifted: the repository was edited, the copies were not, and
# nothing anywhere said so — a plugin change simply landed inert. The
# hand-copies were already stale again within one session of being made.
#
# `--skill-only` skips the build and installs just the skill and hooks, which
# is what you want after editing them. A full build to copy three files is the
# friction that made people skip the copy in the first place.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Where a *release* installs, and therefore where this installs too.
#
# `dist`'s shell installer uses `CARGO_HOME`, falling back to `~/.cargo/bin`,
# and `install-path` is unset in `dist-workspace.toml` so that default is what a
# real install actually does.
#
# This used to default to `~/.local/bin`, which produced two of everything: a
# dev build there and a released one in `~/.cargo/bin`, with the first shadowing
# the second on PATH. On 2026-08-15 that left the CLI running one build while
# the daemon ran another, several hours apart, with nothing anywhere saying so
# (KEEL-234). A development install that lands somewhere a release never touches
# is not a rehearsal of the thing users get; it is a second installation to keep
# in step by hand.
bin_dir="${SPECLINE_BIN_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}"
specline_home="${SPECLINE_HOME:-$HOME/.specline}"
skill_dir="${SPECLINE_SKILL_DIR:-$HOME/.claude/skills/specline}"
# Adoption is its own skill because it is used once per project and the everyday
# one is loaded in every project conversation. Folding eighty lines about
# backfilling a repository into that would tax every session for a workflow
# nobody runs twice.
adopt_dir="${SPECLINE_ADOPT_SKILL_DIR:-$HOME/.claude/skills/specline-adopt}"

skill_only=false
case "${1:-}" in
  --skill-only) skill_only=true ;;
  "") ;;
  *) echo "usage: $0 [--skill-only]" >&2; exit 2 ;;
esac

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
note() { printf '  %s\n' "$*"; }

# Copy one file only if it differs, and say which of the three things happened.
# "unchanged" is worth printing: it is the evidence that the copy is in step,
# which is the single fact this whole step exists to establish.
install_file() {
  local src="$1" dest="$2" mode="$3"
  local name; name="$(basename "$src")"
  if [ -f "$dest" ] && cmp -s "$src" "$dest"; then
    note "$name — unchanged"
    return
  fi
  local verb="installed"
  [ -f "$dest" ] && verb="updated"
  install -m "$mode" "$src" "$dest"
  note "$name — $verb"
}

install_skill() {
  say "Installing the skill and hooks to $skill_dir"
  mkdir -p "$skill_dir"
  install_file "$repo_root/plugin/skills/specline/SKILL.md" "$skill_dir/SKILL.md" 644
  mkdir -p "$adopt_dir"
  install_file "$repo_root/plugin/skills/specline-adopt/SKILL.md" "$adopt_dir/SKILL.md" 644
  # One shim now, not two scripts. KEEL-206 moved the logic into the binary as
  # `specline hook session-start` and `specline hook stop`; what is left here is the
  # only part that has to run *without* the binary, so a session between
  # installing the plugin and running setup can say the binary is missing.
  install_file "$repo_root/plugin/hooks/specline-hook.sh" "$skill_dir/specline-hook.sh" 755

  # The two scripts this replaced become forwarders rather than disappearing.
  #
  # This is not tidiness, it is the upgrade path. `settings.json` is the user's
  # file and this script will not edit it — which is right, and it means an
  # existing install still names `session-start.sh` and `stop.sh` by absolute
  # path. Deleting them outright breaks every hook on the machine at the moment
  # of upgrade, silently, until somebody edits a file they were never told to
  # edit. That was tried here and it did exactly that.
  #
  # Three lines each, forwarding to the shim. Nothing needs to change for an
  # upgrade to work, and a settings.json that is simplified later keeps working
  # too.
  for pair in "session-start.sh session-start" "stop.sh stop"; do
    stale="${pair%% *}"
    event="${pair##* }"
    cat > "$skill_dir/$stale" <<FORWARD
#!/bin/sh
# Compatibility forwarder. The hooks moved into the binary in KEEL-206; this
# exists so a settings.json written before that keeps working unchanged.
# Nothing needs it once settings.json points at specline-hook.sh directly.
exec "\$(dirname "\$0")/specline-hook.sh" $event
FORWARD
    chmod 755 "$skill_dir/$stale"
    note "$stale — forwards to specline-hook.sh"
  done

  # Read-only inspection, not a rewrite. A settings file that does not mention
  # these paths means the hooks are installed and never run, which looks
  # exactly like the hooks not working.
  local settings="$HOME/.claude/settings.json"
  if [ -f "$settings" ] && ! grep -q "$skill_dir/specline-hook.sh" "$settings" 2>/dev/null; then
    note ""
    note "NOTE: $settings does not reference these hooks, so they will not run."
    note "See the settings snippet printed at the end."
  fi
}

if [ "$skill_only" = true ]; then
  install_skill
  say "Done"
  note "Skipped the build and the binaries — drop --skill-only for those."
  exit 0
fi

say "Building Specline"
note "SQLite is compiled in, so the installed binary is self-contained: there"
note "is no database to install alongside it and nothing to keep in step."
cd "$repo_root"
cargo build --release --workspace

say "Installing binaries to $bin_dir"
mkdir -p "$bin_dir"
install -m 755 target/release/specline "$bin_dir/specline"
install -m 755 target/release/specline-daemon "$bin_dir/specline-daemon"
note "specline"
note "specline-daemon"

if ! command -v specline >/dev/null 2>&1; then
  note ""
  note "WARNING: $bin_dir is not on your PATH. Add it:"
  note "  export PATH=\"$bin_dir:\$PATH\""
# Installing is not the same as being the one that runs. An older copy earlier
# on PATH wins, and everything downstream then describes a binary this script
# did not write — which is how a CLI and a daemon ended up hours apart with
# nothing saying so (KEEL-234). Checked by resolution rather than by directory,
# so a symlink farm or a shim is caught too.
elif [ "$(command -v specline)" != "$bin_dir/specline" ]; then
  note ""
  note "WARNING: this is not the specline your shell will run."
  note "  installed:  $bin_dir/specline"
  note "  PATH finds: $(command -v specline)"
  note ""
  note "Remove the other copy, or put $bin_dir earlier on PATH. Until then the"
  note "binaries this script just built are installed and not in use."
fi

say "Creating the store at $specline_home"
"$bin_dir/specline" --home "$specline_home" status >/dev/null
note "done"

# ~/.specline is its own git repo, which is recovery tier 1 (SPEC §11): full
# fidelity, including revision history. No remote — that is KB's call (Q-2).
if [ ! -d "$specline_home/.git" ]; then
  say "Initialising $specline_home as a git repository"
  git -C "$specline_home" init -q
  cat > "$specline_home/.gitignore" <<'GITIGNORE'
# Model weights are large and re-downloadable.
models/
GITIGNORE
  git -C "$specline_home" add -A
  git -C "$specline_home" commit -q -m "chore: initialise the Specline store" || true
  note "done — no remote configured, which is deliberate (QUESTIONS Q-2)"
fi

install_skill

# The dependency check that used to be here warned about `jq` and `curl` and
# named the session hooks as the reason. It was wrong twice over: the hooks
# never used `jq`, and the thing they *did* need — `python3`, absent on a Mac
# until the Xcode command line tools arrive — was never checked at all. So the
# one dependency check in the installer was checking the wrong tools for the
# wrong component.
#
# KEEL-206 removed the need rather than correcting the warning. The hooks are
# `specline hook session-start` and `specline hook stop` now, and the only shell left is
# `specline-hook.sh`, which is POSIX `sh` and shells out to nothing. There is
# nothing here to warn about, so there is no warning.

say "Next"
cat <<EOF
  1. Start the daemon, and leave it running:

       specline-daemon

     It loads the embedding model on first start — 127 MB, downloaded once —
     so search matches by meaning as well as by word. Add --no-embeddings if
     you would rather it did not; keyword search works either way.

  2. Wire up the hooks. The files are installed; nothing runs them until
     $HOME/.claude/settings.json says so. Add this yourself — the one thing
     this script will not touch:

       {
         "hooks": {
           "SessionStart": [
             { "hooks": [ { "type": "command", "timeout": 10,
                 "command": "$skill_dir/specline-hook.sh session-start" } ] }
           ],
           "Stop": [
             { "hooks": [ { "type": "command", "timeout": 15,
                 "command": "$skill_dir/specline-hook.sh stop" } ] }
           ]
         }
       }

  3. Or register the repository as a Claude Code plugin instead, which brings
     its own hooks.json and needs no settings edit:

       $repo_root/plugin

  4. Or wire up the MCP server alone, without the skill or hooks:

       claude mcp add --transport http specline http://127.0.0.1:7654/mcp

  5. Check it:

       curl -s http://127.0.0.1:7654/api/health | jq

  6. Load the sample corpus into a scratch store to see what it looks like:

       specline --home /tmp/specline-demo fixture
       specline --home /tmp/specline-demo render-status specline

  After editing anything under plugin/, re-run:

       ./plugin/install.sh --skill-only

EOF

say "Phase 2's gate"
cat <<'EOF'
  Met and frozen. ">=9 of 10 unprompted sessions write to Specline" closed at 18 of
  20 across two independent draws, and nobody is running it any more. The
  harness is kept and still tested, because the next time the agent's
  orientation changes it is the only way to find out what that did.

  product/GATE.md is the whole story, including the five evenings spent fixing
  a problem that turned out not to exist.
EOF
