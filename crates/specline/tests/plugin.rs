//! The plugin's wiring, checked rather than assumed.
//!
//! Every file here is read by Claude Code rather than by this workspace, so
//! nothing else would notice a broken reference: a marketplace entry pointing
//! at a directory that moved, a hook naming a script that was renamed, or a
//! slash command running a file that is not there. All three are silent
//! failures at someone else's install time, which is the worst place to find
//! them.
//!
//! This is the same argument KEEL-206 made about the hooks — every other
//! surface in this phase describes itself and is tested — applied to the four
//! JSON files and two scripts that make the plugin a plugin.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves")
}

fn json_at(relative: &str) -> Value {
    let path = root().join(relative);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} could not be read: {e}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

/// The entry point. Without this at the repository root, `/plugin marketplace
/// add <owner>/specline` finds nothing and the whole install flow has no first
/// step.
#[test]
fn the_marketplace_entry_points_at_a_plugin_that_exists() {
    let market = json_at(".claude-plugin/marketplace.json");
    let plugins = market["plugins"].as_array().expect("plugins is an array");
    assert!(!plugins.is_empty(), "a marketplace with no plugins in it");

    for plugin in plugins {
        let source = plugin["source"]
            .as_str()
            .expect("every plugin has a source");
        let dir = root().join(source.trim_start_matches("./"));
        assert!(
            dir.join(".claude-plugin/plugin.json").is_file(),
            "{source} has no plugin.json — the marketplace points at nothing"
        );
    }
}

/// The two names must agree, or the plugin installs under one name and its
/// commands are namespaced under another.
#[test]
fn the_plugin_name_matches_between_the_two_manifests() {
    let market = json_at(".claude-plugin/marketplace.json");
    let plugin = json_at("plugin/.claude-plugin/plugin.json");
    assert_eq!(
        market["plugins"][0]["name"], plugin["name"],
        "the marketplace and the plugin manifest disagree about the name, so \
         `/specline:setup` would not be the command's real name"
    );
}

/// HTTP, not stdio, and this is load-bearing rather than a preference: a stdio
/// server is one process per session, and several of those would each open the
/// store and fight over the single write path.
#[test]
fn the_mcp_server_talks_to_the_daemon_over_http() {
    let mcp = json_at("plugin/.mcp.json");
    let specline = &mcp["mcpServers"]["specline"];
    assert_eq!(
        specline["type"], "http",
        "a stdio server would be a second writer"
    );
    let url = specline["url"].as_str().expect("the server has a url");
    assert!(url.contains("/mcp"), "{url} is not the MCP endpoint");
    assert!(
        url.contains("127.0.0.1"),
        "{url} must be loopback — this API has no authentication"
    );
}

/// Every hook must run something that is actually on disk.
#[test]
fn every_hook_names_a_script_that_exists_and_is_executable() {
    let hooks = json_at("plugin/hooks/hooks.json");
    let events = hooks["hooks"].as_object().expect("hooks is an object");
    for event in ["SessionStart", "Stop", "PostToolUse"] {
        assert!(
            events.contains_key(event),
            "the {event} hook should be registered"
        );
    }

    let mut checked = 0;
    for (event, entries) in events {
        for entry in entries.as_array().expect("entries is an array") {
            for hook in entry["hooks"].as_array().expect("hooks is an array") {
                let command = hook["command"].as_str().expect("a command");
                let relative = command
                    .replace("${CLAUDE_PLUGIN_ROOT}", "plugin")
                    .split_whitespace()
                    .next()
                    .expect("a script path")
                    .to_owned();
                let path = root().join(&relative);
                assert!(
                    path.is_file(),
                    "the {event} hook runs {relative}, which does not exist"
                );
                assert!(is_executable(&path), "{relative} is not executable");
                checked += 1;
            }
        }
    }
    assert!(checked >= 3, "expected all three hooks to be checked");
}

/// The commit hook runs after Bash and nothing else (KEEL-395).
///
/// Without the matcher it would start a process after every Read and Edit
/// as well, to leave at once — cost for nothing, on the busiest path there is.
#[test]
fn the_commit_hook_runs_only_after_bash() {
    let hooks = json_at("plugin/hooks/hooks.json");
    let entries = hooks["hooks"]["PostToolUse"]
        .as_array()
        .expect("PostToolUse is registered");
    assert!(!entries.is_empty());
    for entry in entries {
        assert_eq!(entry["matcher"], "Bash", "{entry}");
        for hook in entry["hooks"].as_array().expect("hooks is an array") {
            let command = hook["command"].as_str().expect("a command");
            assert!(command.ends_with(" commit"), "{command}");
        }
    }
}

/// The slash command's whole job is to run one script. If it names a file that
/// is not there, `/specline:setup` fails at the one moment a new user is watching.
#[test]
fn the_setup_command_runs_a_script_that_exists() {
    let command = std::fs::read_to_string(root().join("plugin/commands/setup.md"))
        .expect("the setup command exists");
    assert!(
        command.contains("scripts/setup.sh"),
        "the setup command should run the setup script"
    );

    let script = root().join("plugin/scripts/setup.sh");
    assert!(script.is_file(), "plugin/scripts/setup.sh is missing");
    assert!(is_executable(&script), "setup.sh is not executable");
}

/// It has to run under `sh` on a machine with nothing installed. A bashism
/// here is a syntax error at install time on a distribution where `/bin/sh` is
/// dash — which is most of them, and none of them this Mac.
#[test]
fn the_shipped_scripts_parse() {
    for script in ["plugin/scripts/setup.sh", "plugin/hooks/specline-hook.sh"] {
        let shell = if script.ends_with("specline-hook.sh") {
            // The hook shim claims to be POSIX, so check it as POSIX.
            "/bin/sh"
        } else {
            "/bin/bash"
        };
        let status = std::process::Command::new(shell)
            .arg("-n")
            .arg(root().join(script))
            .status()
            .expect("the shell runs");
        assert!(status.success(), "{script} does not parse under {shell}");
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

/// Every tool the daemon offers is named in the skill.
///
/// This exists because the skill went stale in exactly the way nothing else
/// would catch. Its table was headed "The nine tools", listed ten, and omitted
/// `specline_claim`, `specline_close` and `specline_next` — the three work verbs that
/// raised the cap to thirteen. So on every project except this repository,
/// which has its own standing instructions, a session had nothing telling it to
/// claim a task, and tasks sat in `todo` while the work was being done. The
/// board was wrong and nothing failed.
///
/// The skill is prose read by a model, so there is no compiler to notice. This
/// is the compiler.
#[test]
fn the_skill_names_every_tool_the_daemon_offers() {
    let skill = std::fs::read_to_string(root().join("plugin/skills/specline/SKILL.md"))
        .expect("the specline skill is readable");

    let missing: Vec<&str> = specline_mcp::tools::all()
        .iter()
        .map(|tool| tool.name)
        .filter(|name| !skill.contains(name))
        .collect();

    assert!(
        missing.is_empty(),
        "plugin/skills/specline/SKILL.md never mentions {missing:?}. A tool a model is never told \
         about is a tool it does not call — which is how claiming went unmentioned for three \
         releases. Add it to the table and say when to reach for it."
    );
}

/// The skill does not put a count in a heading.
///
/// "The nine tools" outlived two additions. A number in prose has to be
/// remembered by whoever adds the eleventh, and it was not, twice.
#[test]
fn the_skill_does_not_count_the_tools_in_a_heading() {
    let skill = std::fs::read_to_string(root().join("plugin/skills/specline/SKILL.md"))
        .expect("the specline skill is readable");

    for line in skill.lines().filter(|l| l.starts_with('#')) {
        for word in ["nine", "ten", "eleven", "twelve", "thirteen", "fourteen"] {
            assert!(
                !line.to_lowercase().contains(word),
                "heading {line:?} counts the tools. The count belongs in `tools::all()`, where \
                 adding one keeps it right; in a heading it goes stale silently."
            );
        }
    }
}
