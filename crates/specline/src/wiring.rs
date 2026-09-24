//! Which of Specline's Claude Code hooks are actually wired, and from where.
//!
//! A hook can reach a session three ways: the user's settings file, the
//! project's, or the Specline plugin's `hooks.json`. Nothing checked any of
//! them, so a hook could be installed and never run and look exactly like
//! Specline having nothing to say. That happened on KB's own machine on
//! 2026-09-24: the plugin had gained the commit hook (KEEL-395), his hooks
//! were hand-wired in `~/.claude/settings.json`, and so the new hook simply
//! never fired, with nothing anywhere saying so (KEEL-396).
//!
//! Reading is split from judging. [`gather`] reads files and never fails —
//! a file it cannot read is a source it could not see, and it says so.
//! [`inspect`] is pure, which is what the tests exercise: they build the
//! settings files they need rather than reading this machine's.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// One hook Specline expects a session to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expected {
    /// Claude Code's event name.
    pub event: &'static str,
    /// The argument `specline-hook.sh` / `specline hook` is given for it.
    pub subcommand: &'static str,
    /// What it does, for the report.
    pub purpose: &'static str,
}

/// The three hooks, in the order a session meets them.
pub const EXPECTED: [Expected; 3] = [
    Expected {
        event: "SessionStart",
        subcommand: "session-start",
        purpose: "puts the project digest in front of the session",
    },
    Expected {
        event: "PostToolUse",
        subcommand: "commit",
        purpose: "tells the session when it commits work no task describes",
    },
    Expected {
        event: "Stop",
        subcommand: "stop",
        purpose: "asks a session that recorded nothing whether it should have",
    },
];

/// A place hooks can come from, already read.
#[derive(Debug, Clone)]
pub struct Source {
    /// What to call it in a report: a path, or "the Specline plugin".
    pub label: String,
    /// The parsed file, whose `hooks` object is what counts.
    pub content: Value,
}

/// What was found for one expected hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub expected: Expected,
    /// Every source that wires it. Empty is missing; more than one runs it
    /// that many times per event.
    pub wired_by: Vec<String>,
}

/// The whole picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wiring {
    pub hooks: Vec<Found>,
    /// Files that exist but could not be read or parsed. A hook wired in one
    /// of these is invisible here, so they are reported rather than dropped.
    pub unreadable: Vec<String>,
    /// A source that sets `disableAllHooks`, which stops every hook however
    /// it is wired.
    pub disabled_by: Option<String>,
}

impl Wiring {
    /// The hooks no source wires.
    pub fn missing(&self) -> Vec<&Found> {
        self.hooks
            .iter()
            .filter(|f| f.wired_by.is_empty())
            .collect()
    }

    /// The hooks wired more than once, which run more than once per event.
    pub fn duplicated(&self) -> Vec<&Found> {
        self.hooks.iter().filter(|f| f.wired_by.len() > 1).collect()
    }

    /// Whether any Specline hook is wired at all — distinguishing "wired
    /// partly" from a setup that never meant to use the hooks (MCP alone).
    pub fn any(&self) -> bool {
        self.hooks.iter().any(|f| !f.wired_by.is_empty())
    }
}

/// Whether a hook command is Specline's, for this subcommand.
///
/// Matches the shim (`specline-hook.sh commit`), the binary called directly
/// (`specline hook commit`, which is how Codex and a hand-written config do
/// it), and the two forwarders install.sh leaves for settings written before
/// KEEL-206 (`…/skills/specline/session-start.sh`, `…/stop.sh`).
fn is_specline_command(command: &str, subcommand: &str) -> bool {
    // Normalised words: quotes stripped, so a quoted path with a space in it
    // (`"…/Application Support/…/specline-hook.sh" commit`) and a command
    // wrapped in `sh -c '…'` both still read as words; backslashes read as
    // separators and `.exe` dropped, so a Windows path matches too. Each of
    // these missed would be a false "not wired" in every session.
    let owned: Vec<String> = command
        .split_whitespace()
        .map(|w| {
            let w = w.trim_matches(|c| c == '"' || c == '\'');
            let w = w.replace('\\', "/");
            w.strip_suffix(".exe").map(str::to_owned).unwrap_or(w)
        })
        .collect();
    let words: Vec<&str> = owned.iter().map(String::as_str).collect();
    let names_it = |i: usize| words.get(i + 1).is_some_and(|w| *w == subcommand);
    words.iter().enumerate().any(|(i, w)| {
        (w.ends_with("specline-hook.sh") && names_it(i))
            || ((w.ends_with("/specline") || *w == "specline")
                && words.get(i + 1) == Some(&"hook")
                && words.get(i + 2).is_some_and(|x| *x == subcommand))
    }) || (words.len() == 1
        && words[0].contains("/skills/specline/")
        && words[0].ends_with(&format!("/{subcommand}.sh")))
}

/// Whether a `PostToolUse` matcher lets Bash through.
///
/// Absent, empty and `*` match everything; otherwise Claude Code treats the
/// matcher as a regex over the tool name, and the ones that matter here are
/// an exact `Bash` or an alternation containing it. A matcher this cannot
/// judge counts as not matching, so the report errs toward "missing" — a
/// false alarm someone can dismiss, rather than a hook that silently never runs.
fn matcher_covers_bash(matcher: Option<&str>) -> bool {
    match matcher.map(str::trim) {
        None | Some("") | Some("*") | Some(".*") => true,
        Some(m) => m.split('|').any(|part| {
            let part = part.trim().trim_start_matches('^').trim_end_matches('$');
            part == "Bash" || part == "Bash.*"
        }),
    }
}

/// Which sources wire one expected hook.
fn wired_by(expected: Expected, sources: &[Source]) -> Vec<String> {
    sources
        .iter()
        .filter(|source| {
            source
                .content
                .get("hooks")
                .and_then(|h| h.get(expected.event))
                .and_then(Value::as_array)
                .is_some_and(|groups| {
                    groups.iter().any(|group| {
                        let bash_ok = expected.event != "PostToolUse"
                            || matcher_covers_bash(group.get("matcher").and_then(Value::as_str));
                        bash_ok
                            && group
                                .get("hooks")
                                .and_then(Value::as_array)
                                .is_some_and(|hooks| {
                                    hooks.iter().any(|h| {
                                        h.get("command").and_then(Value::as_str).is_some_and(|c| {
                                            is_specline_command(c, expected.subcommand)
                                        })
                                    })
                                })
                    })
                })
        })
        .map(|s| s.label.clone())
        .collect()
}

/// Judge what the sources wire. Pure.
pub fn inspect(sources: &[Source], unreadable: Vec<String>) -> Wiring {
    let disabled_by = sources
        .iter()
        .find(|s| s.content.get("disableAllHooks").and_then(Value::as_bool) == Some(true))
        .map(|s| s.label.clone());
    Wiring {
        disabled_by,
        hooks: EXPECTED
            .iter()
            .map(|&expected| Found {
                expected,
                wired_by: wired_by(expected, sources),
            })
            .collect(),
        unreadable,
    }
}

/// Claude Code's configuration directory: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
}

/// The most of a settings file this will read. A real one is a few KB; a file
/// past this is not something to parse on the way into a session.
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// Read one JSON file into a source. `Ok(None)` when it does not exist.
fn read(path: &Path, label: String) -> Result<Option<Source>, String> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{} ({e})", path.display())),
    };
    let mut text = String::new();
    file.take(MAX_SETTINGS_BYTES)
        .read_to_string(&mut text)
        .map_err(|e| format!("{} ({e})", path.display()))?;
    serde_json::from_str(&text)
        .map(|content| Some(Source { label, content }))
        .map_err(|e| format!("{} ({e})", path.display()))
}

/// Read every place a Specline hook could be wired from.
///
/// `claude_dir` is the user's Claude Code directory; `project` the directory
/// the session is in, whose `.claude/` may carry settings of its own. The
/// plugin counts only when it is installed and not switched off in
/// `enabledPlugins` — an installed, disabled plugin wires nothing.
pub fn gather(claude_dir: &Path, project: Option<&Path>) -> (Vec<Source>, Vec<String>) {
    let mut sources = Vec::new();
    let mut unreadable = Vec::new();
    // Each file once. A session in the home directory has `~/.claude` as its
    // project directory too, and reading the user settings under two labels
    // reported every hook as wired twice.
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut take = |path: PathBuf, label: String| {
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if seen.contains(&key) {
            return;
        }
        seen.push(key);
        match read(&path, label) {
            Ok(Some(s)) => sources.push(s),
            Ok(None) => {}
            Err(e) => unreadable.push(e),
        }
    };

    let user_settings = claude_dir.join("settings.json");
    take(user_settings.clone(), user_settings.display().to_string());
    if let Some(project) = project {
        for name in ["settings.json", "settings.local.json"] {
            let path = project.join(".claude").join(name);
            take(path.clone(), path.display().to_string());
        }
    }

    // The plugin: installed for this user or this project, and not disabled.
    let enabled = read(&user_settings, String::new())
        .ok()
        .flatten()
        .and_then(|s| s.content.get("enabledPlugins").cloned());
    let disabled = |key: &str| {
        enabled
            .as_ref()
            .and_then(|e| e.get(key))
            .and_then(Value::as_bool)
            == Some(false)
    };
    let project_key = project.map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned()));
    let installed = claude_dir.join("plugins").join("installed_plugins.json");
    match read(&installed, String::new()) {
        Ok(Some(list)) => {
            if let Some(plugins) = list.content.get("plugins").and_then(Value::as_object) {
                for (key, installs) in plugins {
                    if !key.starts_with("specline@") || disabled(key) {
                        continue;
                    }
                    for install in installs.as_array().into_iter().flatten() {
                        if !applies_here(install, project_key.as_deref()) {
                            continue;
                        }
                        if let Some(path) = install.get("installPath").and_then(Value::as_str) {
                            take(
                                Path::new(path).join("hooks").join("hooks.json"),
                                format!("the Specline plugin ({key})"),
                            );
                        }
                    }
                }
            }
        }
        Ok(None) => {}
        Err(e) => unreadable.push(e),
    }

    (sources, unreadable)
}

/// Whether one plugin install applies to a session in `project`.
///
/// A user-scoped install applies everywhere. A project-scoped one applies in
/// the project it was installed for and nowhere else — counting it elsewhere
/// reported a plugin installed for another repository as wiring this one.
/// An install that names no scope is counted, because that is the older
/// shape of the file and it only ever held user installs.
fn applies_here(install: &Value, project: Option<&Path>) -> bool {
    match install.get("scope").and_then(Value::as_str) {
        None | Some("user") => true,
        Some(_) => {
            let Some(for_project) = install.get("projectPath").and_then(Value::as_str) else {
                return false;
            };
            let for_project =
                std::fs::canonicalize(for_project).unwrap_or_else(|_| PathBuf::from(for_project));
            project.is_some_and(|p| p == for_project)
        }
    }
}

/// Gather and inspect, for a session in `project`. `None` when there is no
/// Claude Code directory to look in at all.
pub fn examine(project: Option<&Path>) -> Option<Wiring> {
    let dir = claude_dir()?;
    let (sources, unreadable) = gather(&dir, project);
    Some(inspect(&sources, unreadable))
}

/// The line a session is told when its own hooks are partly wired, or `None`.
///
/// Only when the `SessionStart` hook is itself visibly wired: that is the hook
/// saying this, so if it cannot find its own wiring it is being run from
/// somewhere this cannot see (Codex, a `--settings` flag), and anything it
/// concluded about the other two would be a guess.
pub fn session_notice(wiring: &Wiring) -> Option<String> {
    let start_is_wired = wiring
        .hooks
        .iter()
        .any(|f| f.expected.event == "SessionStart" && !f.wired_by.is_empty());
    if !start_is_wired {
        return None;
    }
    let missing: Vec<String> = wiring
        .missing()
        .iter()
        .map(|f| {
            format!(
                "the {} hook ({})",
                f.expected.subcommand, f.expected.purpose
            )
        })
        .collect();
    let doubled: Vec<&str> = wiring
        .duplicated()
        .iter()
        .map(|f| f.expected.subcommand)
        .collect();
    if missing.is_empty() && doubled.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !missing.is_empty() {
        parts.push(format!(
            "{} is not wired, so it never runs",
            missing.join(" and ")
        ));
    }
    if !doubled.is_empty() {
        parts.push(format!(
            "the {} hook is wired more than once, so it runs more than once",
            doubled.join(" and ")
        ));
    }
    Some(format!(
        "Specline's hooks are only partly set up here: {}. Mention this to the user once, \
         briefly, and suggest `specline doctor`, which says where each hook is wired and \
         what to change.",
        parts.join("; ")
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings(label: &str, hooks: Value) -> Source {
        Source {
            label: label.into(),
            content: json!({ "hooks": hooks }),
        }
    }

    fn hook(command: &str) -> Value {
        json!([{ "hooks": [{ "type": "command", "command": command }] }])
    }

    fn all_three(prefix: &str) -> Value {
        json!({
            "SessionStart": hook(&format!("{prefix} session-start")),
            "PostToolUse": [{ "matcher": "Bash",
                "hooks": [{ "type": "command", "command": format!("{prefix} commit") }] }],
            "Stop": hook(&format!("{prefix} stop")),
        })
    }

    #[test]
    fn all_three_through_the_shim_is_complete() {
        let w = inspect(
            &[settings("user", all_three("/x/specline-hook.sh"))],
            vec![],
        );
        assert!(w.missing().is_empty(), "{w:?}");
        assert!(w.duplicated().is_empty());
    }

    #[test]
    fn the_binary_called_directly_counts() {
        let w = inspect(
            &[settings(
                "codex-style",
                all_three("/Users/you/.cargo/bin/specline hook"),
            )],
            vec![],
        );
        assert!(w.missing().is_empty(), "{w:?}");
    }

    /// KB's machine before KEEL-400: the session hooks, and not the new one.
    #[test]
    fn hand_wiring_from_before_the_commit_hook_is_missing_it() {
        let w = inspect(
            &[settings(
                "user",
                json!({
                    "SessionStart": hook("/x/specline-hook.sh session-start"),
                    "Stop": hook("/x/specline-hook.sh stop"),
                }),
            )],
            vec![],
        );
        let missing: Vec<_> = w.missing().iter().map(|f| f.expected.subcommand).collect();
        assert_eq!(missing, vec!["commit"]);
        assert!(w.any());
    }

    #[test]
    fn a_commit_hook_behind_a_matcher_that_excludes_bash_does_not_count() {
        let hooks = json!({
            "PostToolUse": [{ "matcher": "Edit|Write",
                "hooks": [{ "type": "command", "command": "/x/specline-hook.sh commit" }] }],
        });
        let w = inspect(&[settings("user", hooks)], vec![]);
        assert!(
            w.missing()
                .iter()
                .any(|f| f.expected.subcommand == "commit"),
            "a matcher that never lets Bash through wires nothing: {w:?}"
        );
    }

    #[test]
    fn matchers_that_let_bash_through() {
        for m in [None, Some(""), Some("*"), Some("Bash"), Some("Edit|Bash")] {
            assert!(matcher_covers_bash(m), "{m:?}");
        }
        for m in [Some("Edit"), Some("Bashful"), Some("mcp__.*")] {
            assert!(!matcher_covers_bash(m), "{m:?}");
        }
    }

    /// The plugin and a hand-wired settings file together inject the digest
    /// twice into every session.
    #[test]
    fn the_plugin_and_settings_together_are_reported_as_duplicates() {
        let w = inspect(
            &[
                settings("user", all_three("/x/specline-hook.sh")),
                settings(
                    "plugin",
                    all_three("${CLAUDE_PLUGIN_ROOT}/hooks/specline-hook.sh"),
                ),
            ],
            vec![],
        );
        assert_eq!(w.duplicated().len(), 3, "{w:?}");
    }

    #[test]
    fn the_pre_keel_206_forwarders_count() {
        let w = inspect(
            &[settings(
                "user",
                json!({
                    "SessionStart": hook("/Users/you/.claude/skills/specline/session-start.sh"),
                    "Stop": hook("/Users/you/.claude/skills/specline/stop.sh"),
                }),
            )],
            vec![],
        );
        let missing: Vec<_> = w.missing().iter().map(|f| f.expected.subcommand).collect();
        assert_eq!(missing, vec!["commit"]);
    }

    /// Somebody else's hook with a similar word in it is not Specline's.
    #[test]
    fn other_hooks_are_not_mistaken_for_speclines() {
        for command in [
            "/x/other-hook.sh stop",
            "echo specline-hook.sh",
            "specline hook",
            "/x/specline-hook.sh session-start --and-stop",
        ] {
            assert!(!is_specline_command(command, "stop"), "{command}");
        }
    }

    #[test]
    fn a_session_is_told_what_is_missing_only_when_its_own_hook_is_visible() {
        let partly = inspect(
            &[settings(
                "user",
                json!({ "SessionStart": hook("/x/specline-hook.sh session-start") }),
            )],
            vec![],
        );
        let notice = session_notice(&partly).unwrap();
        assert!(notice.contains("the commit hook"), "{notice}");
        assert!(notice.contains("the stop hook"), "{notice}");
        assert!(notice.contains("specline doctor"), "{notice}");

        let complete = inspect(
            &[settings("user", all_three("/x/specline-hook.sh"))],
            vec![],
        );
        assert!(session_notice(&complete).is_none());

        let invisible = inspect(&[settings("user", json!({}))], vec![]);
        assert!(
            session_notice(&invisible).is_none(),
            "a session-start hook that cannot see its own wiring must not guess"
        );
    }

    #[test]
    fn a_session_is_told_when_its_hooks_run_twice() {
        let w = inspect(
            &[
                settings("user", all_three("/x/specline-hook.sh")),
                settings("plugin", all_three("/p/specline-hook.sh")),
            ],
            vec![],
        );
        let notice = session_notice(&w).unwrap();
        assert!(notice.contains("more than once"), "{notice}");
    }

    #[test]
    fn real_world_command_shapes_are_recognised() {
        for command in [
            "\"/Users/x/Library/Application Support/specline/specline-hook.sh\" commit",
            "sh -c '/x/specline-hook.sh commit'",
            "C:\\Users\\x\\.cargo\\bin\\specline.exe hook commit",
            "${CLAUDE_PLUGIN_ROOT}/hooks/specline-hook.sh commit",
        ] {
            assert!(is_specline_command(command, "commit"), "{command}");
        }
    }

    #[test]
    fn anchored_matchers_let_bash_through() {
        for m in ["^Bash$", "Bash.*", "Edit|^Bash$"] {
            assert!(matcher_covers_bash(Some(m)), "{m}");
        }
    }

    #[test]
    fn disable_all_hooks_is_reported() {
        let mut source = settings("user", all_three("/x/specline-hook.sh"));
        source.content["disableAllHooks"] = json!(true);
        assert_eq!(
            inspect(&[source], vec![]).disabled_by.as_deref(),
            Some("user")
        );
    }

    /// A session in the home directory: the project's `.claude` is the user's.
    #[test]
    fn the_same_file_reached_twice_is_read_once() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        write(
            &claude.join("settings.json"),
            &json!({ "hooks": all_three("/x/specline-hook.sh") }),
        );
        let (sources, _) = gather(&claude, Some(dir.path()));
        let w = inspect(&sources, vec![]);
        assert!(w.duplicated().is_empty(), "{w:?}");
        assert!(w.missing().is_empty());
    }

    #[test]
    fn a_plugin_installed_for_another_project_does_not_wire_this_one() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        let here = dir.path().join("here");
        let there = dir.path().join("there");
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&there).unwrap();
        let install = dir.path().join("cache/specline/0.6.0");
        write(
            &install.join("hooks/hooks.json"),
            &json!({ "hooks": all_three("${CLAUDE_PLUGIN_ROOT}/hooks/specline-hook.sh") }),
        );
        write(
            &claude.join("plugins/installed_plugins.json"),
            &json!({ "plugins": { "specline@specline": [
                { "scope": "project", "projectPath": there, "installPath": install }
            ] } }),
        );

        let (sources, _) = gather(&claude, Some(&here));
        assert!(
            !inspect(&sources, vec![]).any(),
            "installed for another project"
        );
        let (sources, _) = gather(&claude, Some(&there));
        assert!(
            inspect(&sources, vec![]).missing().is_empty(),
            "installed for this one"
        );
    }

    #[test]
    fn nothing_wired_is_distinguished_from_partly_wired() {
        let w = inspect(&[settings("user", json!({}))], vec![]);
        assert!(!w.any());
        assert_eq!(w.missing().len(), 3);
    }

    // --- reading files -----------------------------------------------------

    fn write(path: &Path, value: &Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    }

    #[test]
    fn an_installed_enabled_plugin_is_read_and_a_disabled_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude");
        let install = dir.path().join("cache/specline/0.6.0");
        write(
            &install.join("hooks/hooks.json"),
            &json!({ "hooks": all_three("${CLAUDE_PLUGIN_ROOT}/hooks/specline-hook.sh") }),
        );
        write(
            &claude.join("plugins/installed_plugins.json"),
            &json!({ "plugins": { "specline@specline": [{ "installPath": install }] } }),
        );

        let (sources, unreadable) = gather(&claude, None);
        assert!(unreadable.is_empty(), "{unreadable:?}");
        assert!(inspect(&sources, vec![]).missing().is_empty());

        write(
            &claude.join("settings.json"),
            &json!({ "enabledPlugins": { "specline@specline": false } }),
        );
        let (sources, _) = gather(&claude, None);
        assert!(
            !inspect(&sources, vec![]).any(),
            "a disabled plugin wires nothing"
        );
    }

    #[test]
    fn project_settings_count_and_a_broken_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path().join(".claude-user");
        let project = dir.path().join("repo");
        write(
            &project.join(".claude/settings.local.json"),
            &json!({ "hooks": all_three("/x/specline-hook.sh") }),
        );
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("settings.json"), "{ not json").unwrap();

        let (sources, unreadable) = gather(&claude, Some(&project));
        assert!(inspect(&sources, vec![]).missing().is_empty());
        assert_eq!(unreadable.len(), 1, "{unreadable:?}");
        assert!(unreadable[0].contains("settings.json"));
    }

    #[test]
    fn no_files_at_all_is_nothing_wired_and_nothing_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let (sources, unreadable) = gather(dir.path(), Some(dir.path()));
        assert!(sources.is_empty());
        assert!(unreadable.is_empty());
    }
}
