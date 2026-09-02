//! The session hooks, executed.
//!
//! This file is the point of KEEL-206. The hooks were 317 lines of bash that
//! **nothing in the workspace or in CI had ever run** — not once. KEEL-192 was
//! a real bug in them, found by reading and fixed by reading, and the fix was
//! guarded by nothing at all. Every other surface in this phase describes
//! itself and is tested; the hooks did neither.
//!
//! So these drive the real binary, over a real socket, with the payload on
//! stdin and the JSON read back off stdout — the same way Claude Code invokes
//! them. Unit tests in `hook.rs` cover the decisions; these cover the wiring,
//! which is where the bash version's dependencies were hiding.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

/// A daemon that answers `/api/context` and `/api/activity` with fixed bodies.
///
/// Serves in a loop on a background thread: the Stop hook makes two calls, and
/// a one-shot stub would make the second one fail — which is a silent path, so
/// the test would pass for the wrong reason.
fn stub_daemon(context: &'static str, activity: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut socket) = stream else { return };
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            // Drain the headers so the client is not left writing into a
            // socket nobody is reading.
            loop {
                let mut header = String::new();
                match reader.read_line(&mut header) {
                    Ok(0) => break,
                    Ok(_) if header.trim().is_empty() => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }

            let body = if request_line.contains("/api/activity") {
                activity
            } else {
                context
            };
            let _ = socket.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });

    base
}

/// Run a hook exactly as Claude Code would: payload in, JSON out.
///
/// `TMPDIR` is redirected so the Stop hook's once-per-session marker lands in
/// the test's own directory. Without it, a marker from one test would silence
/// another, and the suite would pass by accident.
fn run_hook(which: &str, daemon: &str, payload: &str, tmpdir: &std::path::Path) -> (String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_specline"))
        .args(["hook", which, "--daemon", daemon])
        .env("TMPDIR", tmpdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the specline binary runs");

    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();

    let output = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

fn scratch() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary directory")
}

const MATCHED: &str = r#"{"summary":"Specline (specline)\nstatus: active\n\n## Next\n- do the thing","data":{"project":{"slug":"specline"}}}"#;
const UNMATCHED: &str = r#"{"summary":"specline_context matched nothing for this checkout\n\nAcme Corp\n\nWidgets Ltd","data":{"project":null}}"#;
const NO_EVENTS: &str = r#"{"data":{"events":[]}}"#;
const WROTE: &str = r#"{"data":{"events":[{"session_id":"ses_abc123"}]}}"#;

// --- session-start ----------------------------------------------------------

#[test]
fn session_start_injects_the_digest_and_pins_the_session_id() {
    let dir = scratch();
    let daemon = stub_daemon(MATCHED, NO_EVENTS);

    let (stdout, code) = run_hook(
        "session-start",
        &daemon,
        r#"{"cwd":"/tmp/x","session_id":"abc123","source":"startup"}"#,
        dir.path(),
    );

    assert_eq!(code, 0);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON on stdout");
    let context = parsed["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext is a string");
    assert!(context.contains("do the thing"), "{context}");
    assert!(context.contains("ses_abc123"), "{context}");
    assert_eq!(
        parsed["hookSpecificOutput"]["hookEventName"], "SessionStart",
        "the event name is what tells Claude Code where this belongs"
    );
}

/// A compaction is not a session start, and re-injecting there spends the most
/// context at the moment there is least.
#[test]
fn session_start_says_nothing_on_a_compaction() {
    let dir = scratch();
    let daemon = stub_daemon(MATCHED, NO_EVENTS);

    let (stdout, code) = run_hook(
        "session-start",
        &daemon,
        r#"{"cwd":"/tmp/x","session_id":"abc123","source":"compact"}"#,
        dir.path(),
    );

    assert_eq!(code, 0);
    assert!(stdout.trim().is_empty(), "{stdout}");
}

/// An unrelated repository must not be handed a roll-up of other projects.
#[test]
fn session_start_in_an_unknown_directory_injects_one_paragraph() {
    let dir = scratch();
    let daemon = stub_daemon(UNMATCHED, NO_EVENTS);

    let (stdout, _) = run_hook(
        "session-start",
        &daemon,
        r#"{"cwd":"/tmp/elsewhere","session_id":"abc123","source":"startup"}"#,
        dir.path(),
    );

    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let context = parsed["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("matched nothing"), "{context}");
    assert!(
        !context.contains("Acme Corp"),
        "another project's name must not reach an unrelated session: {context}"
    );
}

/// This test used to assert the bug.
///
/// It read `assert!(stdout.trim().is_empty())` and passed for as long as a
/// session start against a dead daemon said nothing at all — which is what a
/// user eventually reported as *"it currently fails silently"* (KEEL-354). A
/// test that pins the wrong behaviour is worse than no test, because it makes
/// the wrong behaviour look deliberate to whoever reads it next.
///
/// The constraint it was protecting is real and still asserted below: this runs
/// before the human's first word, so it must never fail a session start. Exit 0
/// was never the part that was wrong.
#[test]
fn session_start_says_the_daemon_is_down_rather_than_saying_nothing() {
    let dir = scratch();
    let (stdout, code) = run_hook(
        "session-start",
        // Port 1 on loopback refuses immediately, so this is the
        // connection-refused arm rather than a timeout.
        "http://127.0.0.1:1",
        r#"{"cwd":"/tmp/x","session_id":"abc123","source":"startup"}"#,
        dir.path(),
    );

    assert_eq!(code, 0, "a hook must never fail a session start");

    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON on stdout");
    let context = parsed["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext is a string");

    assert!(
        context.contains("is not running"),
        "it must name the cause it actually found: {context}"
    );
    assert!(
        context.contains("specline-daemon"),
        "a heads-up nobody can act on is half a heads-up: {context}"
    );
    assert!(
        context.contains("Say so"),
        "the model has to pass this on, or the human still never hears it: {context}"
    );
}

/// A daemon that holds the port but cannot answer must not be reported as
/// absent — that sends someone to start a process that is already up.
#[test]
fn session_start_separates_a_broken_daemon_from_a_missing_one() {
    let dir = scratch();
    let daemon = stub_daemon("this is not a digest", NO_EVENTS);

    let (stdout, code) = run_hook(
        "session-start",
        &daemon,
        r#"{"cwd":"/tmp/x","session_id":"abc123","source":"startup"}"#,
        dir.path(),
    );

    assert_eq!(code, 0);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON on stdout");
    let context = parsed["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext is a string");

    assert!(context.contains("Something is listening"), "{context}");
    assert!(
        !context.contains("is not running"),
        "something is holding the port, so this must not say nothing is: {context}"
    );
}

/// The silence that is still correct.
///
/// A daemon that answers with nothing worth injecting stays quiet. The fix
/// above must not turn every empty digest into a warning.
#[test]
fn session_start_stays_quiet_when_the_daemon_answers_with_nothing() {
    let dir = scratch();
    let daemon = stub_daemon(r#"{"summary":"   ","data":{"project":null}}"#, NO_EVENTS);

    let (stdout, code) = run_hook(
        "session-start",
        &daemon,
        r#"{"cwd":"/tmp/x","session_id":"abc123","source":"startup"}"#,
        dir.path(),
    );

    assert_eq!(code, 0);
    assert!(
        stdout.trim().is_empty(),
        "an empty digest is not a failure and must not be announced: {stdout}"
    );
}

#[test]
fn session_start_survives_a_payload_it_cannot_parse() {
    let dir = scratch();
    let daemon = stub_daemon(MATCHED, NO_EVENTS);
    let (_, code) = run_hook("session-start", &daemon, "this is not json", dir.path());
    assert_eq!(code, 0);
}

// --- stop -------------------------------------------------------------------

#[test]
fn stop_asks_when_a_session_in_a_specline_project_recorded_nothing() {
    let dir = scratch();
    let daemon = stub_daemon(MATCHED, NO_EVENTS);

    let (stdout, code) = run_hook(
        "stop",
        &daemon,
        r#"{"cwd":"/tmp/x","session_id":"abc123","stop_hook_active":false}"#,
        dir.path(),
    );

    assert_eq!(code, 0);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON on stdout");
    assert_eq!(parsed["decision"], "block");
    assert!(
        parsed["reason"]
            .as_str()
            .unwrap()
            .contains("Nothing from this session reached Specline"),
        "{stdout}"
    );
}

/// Seven of ten sessions already write unprompted. A forcing function that
/// fires on correct behaviour is one a user disables within a week.
#[test]
fn stop_is_silent_for_a_session_that_already_wrote() {
    let dir = scratch();
    let daemon = stub_daemon(MATCHED, WROTE);

    let (stdout, code) = run_hook(
        "stop",
        &daemon,
        r#"{"cwd":"/tmp/x","session_id":"abc123","stop_hook_active":false}"#,
        dir.path(),
    );

    assert_eq!(code, 0);
    assert!(stdout.trim().is_empty(), "{stdout}");
}

/// **KEEL-192, and the reason this file exists.** The activity check is global,
/// so a session in an unrelated repository has no events and was nagged for not
/// filing notes about a project that does not exist. This behaviour was fixed
/// by reading and, until now, guarded by nothing.
#[test]
fn stop_is_silent_in_a_directory_specline_has_never_heard_of() {
    let dir = scratch();
    let daemon = stub_daemon(UNMATCHED, NO_EVENTS);

    let (stdout, code) = run_hook(
        "stop",
        &daemon,
        r#"{"cwd":"/tmp/somebody-elses-repo","session_id":"abc123","stop_hook_active":false}"#,
        dir.path(),
    );

    assert_eq!(code, 0);
    assert!(
        stdout.trim().is_empty(),
        "a session in a directory Specline does not know must not be nagged: {stdout}"
    );
}

/// Without this the hook blocks its own continuation, for ever.
#[test]
fn stop_does_not_block_a_continuation_it_caused() {
    let dir = scratch();
    let daemon = stub_daemon(MATCHED, NO_EVENTS);

    let (stdout, code) = run_hook(
        "stop",
        &daemon,
        r#"{"cwd":"/tmp/x","session_id":"abc123","stop_hook_active":true}"#,
        dir.path(),
    );

    assert_eq!(code, 0);
    assert!(stdout.trim().is_empty(), "{stdout}");
}

/// One nudge per session, held by a file rather than by trust.
#[test]
fn stop_asks_at_most_once_for_the_same_session() {
    let dir = scratch();
    let daemon = stub_daemon(MATCHED, NO_EVENTS);
    let payload = r#"{"cwd":"/tmp/x","session_id":"abc123","stop_hook_active":false}"#;

    let (first, _) = run_hook("stop", &daemon, payload, dir.path());
    assert!(first.contains("block"), "the first ask should happen");

    let (second, code) = run_hook("stop", &daemon, payload, dir.path());
    assert_eq!(code, 0);
    assert!(
        second.trim().is_empty(),
        "a session must not be asked twice: {second}"
    );
}

/// A false nudge on every session in a project whose daemon is down would make
/// this the most annoying thing in the toolchain.
#[test]
fn stop_is_silent_when_no_daemon_answers() {
    let dir = scratch();
    let (stdout, code) = run_hook(
        "stop",
        "http://127.0.0.1:1",
        r#"{"cwd":"/tmp/x","session_id":"abc123","stop_hook_active":false}"#,
        dir.path(),
    );

    assert_eq!(code, 0, "a hook must never stop a session from ending");
    assert!(stdout.trim().is_empty(), "{stdout}");
}

#[test]
fn stop_says_nothing_without_a_session_id() {
    let dir = scratch();
    let daemon = stub_daemon(MATCHED, NO_EVENTS);
    let (stdout, code) = run_hook("stop", &daemon, r#"{"cwd":"/tmp/x"}"#, dir.path());
    assert_eq!(code, 0);
    assert!(stdout.trim().is_empty(), "{stdout}");
}

/// The dependency the bash version had and never declared. `python3` is absent
/// on a Mac until the Xcode command line tools arrive, and every parse failure
/// exited 0 silently — so on a fresh machine the hooks did nothing and it
/// looked exactly like Specline being broken.
#[test]
fn the_hooks_need_neither_python_nor_curl() {
    let dir = scratch();
    let daemon = stub_daemon(MATCHED, NO_EVENTS);

    // `env -i` with a path holding nothing but the shell's own utilities: no
    // python3 on a stock Mac, and nothing the binary could shell out to.
    let mut child = Command::new("/usr/bin/env")
        .arg("-i")
        .arg("PATH=/nonexistent")
        .arg(format!("TMPDIR={}", dir.path().display()))
        .arg(env!("CARGO_BIN_EXE_specline"))
        .args(["hook", "session-start", "--daemon", &daemon])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("the binary runs with nothing on PATH");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"cwd":"/tmp/x","session_id":"abc123","source":"startup"}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("do the thing"),
        "the hook must work with an empty PATH — that is the whole point of \
         moving it out of bash: {stdout}"
    );
}

// --- the shim ---------------------------------------------------------------
//
// Three lines of shell that cannot be moved into the binary, because their
// whole job is to speak when the binary is not there. Small, and every one of
// these cases was a real failure rather than a hypothetical.

fn shim() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugin/hooks/specline-hook.sh")
}

fn run_shim(
    event: &str,
    specline_bin: &str,
    payload: &str,
    tmpdir: &std::path::Path,
) -> (String, i32) {
    let mut child = Command::new("/bin/sh")
        .arg(shim())
        .arg(event)
        .env("SPECLINE_BIN", specline_bin)
        .env("TMPDIR", tmpdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the shim runs");
    // The write is allowed to fail, and that is not laziness.
    //
    // When there is no binary to hand off to, the shim prints its one sentence
    // and exits without ever reading stdin — so the payload lands in a pipe
    // whose reader is already gone and the write gets EPIPE. Nothing is wrong:
    // a hook is not obliged to read its input, and Claude Code does not require
    // it to. macOS hid this because the pipe buffer swallowed the write before
    // the child exited; the Linux leg of CI failed on it immediately.
    let _ = child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(payload.as_bytes());
    let output = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

/// With a binary, the shim is a pass-through.
#[test]
fn the_shim_hands_off_to_the_binary() {
    let dir = scratch();
    let daemon = stub_daemon(MATCHED, NO_EVENTS);
    // The shim does not take `--daemon`, so the address arrives by environment,
    // which is the same route Claude Code would use.
    let mut child = Command::new("/bin/sh")
        .arg(shim())
        .arg("session-start")
        .env("SPECLINE_BIN", env!("CARGO_BIN_EXE_specline"))
        .env("SPECLINE_DAEMON_URL", &daemon)
        .env("TMPDIR", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"cwd":"/tmp/x","session_id":"abc123","source":"startup"}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("do the thing"), "{stdout}");
}

/// Without one, it says so — and only at session start. This is the reason the
/// shim exists: a hook that *is* the binary cannot report the binary's absence.
#[test]
fn the_shim_reports_a_missing_binary_at_session_start() {
    let dir = scratch();
    let (stdout, code) = run_shim(
        "session-start",
        "/nonexistent/specline",
        r#"{"session_id":"abc"}"#,
        dir.path(),
    );

    assert_eq!(code, 0);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    let context = parsed["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("/specline:setup"), "{context}");
}

/// A session that is ending is the wrong moment to be told about installation,
/// and Stop output would block it.
#[test]
fn the_shim_says_nothing_about_a_missing_binary_at_stop() {
    let dir = scratch();
    let (stdout, code) = run_shim(
        "stop",
        "/nonexistent/specline",
        r#"{"session_id":"abc"}"#,
        dir.path(),
    );
    assert_eq!(code, 0);
    assert!(stdout.trim().is_empty(), "{stdout}");
}

/// The upgrade path, and a real failure rather than a hypothetical: between
/// updating the plugin and updating the binary, `specline` exists but has no `hook`
/// subcommand. With `exec`, clap's "unrecognized subcommand" went straight to
/// Claude Code — and a non-zero Stop hook means *block, using stderr as the
/// reason*, so a stale binary would have injected a usage message as a
/// blocking instruction.
#[test]
fn a_binary_too_old_to_know_hook_is_silent_rather_than_blocking() {
    let dir = scratch();
    let fake = dir.path().join("old-specline");
    std::fs::write(
        &fake,
        "#!/bin/sh\necho 'error: unrecognized subcommand' >&2\nexit 2\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    for event in ["session-start", "stop"] {
        let (stdout, code) = run_shim(
            event,
            fake.to_str().unwrap(),
            r#"{"session_id":"abc"}"#,
            dir.path(),
        );
        assert_eq!(code, 0, "{event} must exit 0 with a stale binary");
        assert!(
            stdout.trim().is_empty(),
            "{event} must say nothing rather than pass through a usage message: {stdout}"
        );
    }
}

// --- The pre-commit hook -------------------------------------------------
//
// `scripts/pre-commit` is the mechanism that stops a hand-edited generated file
// being committed, and it was the second hook in this repository that nothing
// had ever run. KEEL-136 is what that cost: it asked `specline generate --check`
// about the project's *recorded* checkout rather than the tree being committed,
// so from a git worktree it reported drift that was not there and — the half
// that matters — compared a hand edit against a copy of the file that nobody
// had touched. A check that can pass for the wrong tree is the failure this
// hook exists to replace.

/// Somewhere to run a hook that is not this repository.
struct Sandbox {
    dir: tempfile::TempDir,
    /// Where the stub `specline` writes the arguments it was called with.
    argv: std::path::PathBuf,
    /// The directory holding the stub, which is the whole of `PATH`.
    bin: std::path::PathBuf,
}

/// A git repository with one commit, a stub `specline` on `PATH`, and nothing else.
///
/// The stub is the point: the hook's job is to ask the right question, and a
/// real binary would answer a question about a store that has nothing to do
/// with this test. Recording argv makes "which tree did it check" assertable.
fn sandbox(check_exit: i32, drift: &str) -> Sandbox {
    let dir = scratch();
    let bin = dir.path().join("bin");
    let argv = dir.path().join("argv.txt");
    std::fs::create_dir_all(&bin).unwrap();

    let stub = bin.join("specline");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = generate ] && [ \"$2\" = --help ]; then exit 0; fi\n\
             printf '%s\\n' \"$@\" > {argv}\n\
             printf '%s\\n' '{drift}'\n\
             exit {check_exit}\n",
            argv = argv.display(),
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    Sandbox { dir, argv, bin }
}

fn git(cwd: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Stage a generated file, so the hook has something to check.
fn stage_a_generated_file(tree: &std::path::Path, text: &str) {
    std::fs::create_dir_all(tree.join("product")).unwrap();
    std::fs::write(tree.join("product/STATUS.md"), text).unwrap();
    git(tree, &["add", "product/STATUS.md"]);
}

/// Run `scripts/pre-commit` in `cwd`, with only the stub on `PATH`.
fn run_pre_commit(sandbox: &Sandbox, cwd: &std::path::Path) -> (String, i32) {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/pre-commit")
        .canonicalize()
        .expect("the hook is in the repository");
    let out = Command::new("bash")
        .arg(&script)
        .current_dir(cwd)
        .env("PATH", format!("{}:/usr/bin:/bin", sandbox.bin.display()))
        .output()
        .expect("bash runs");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.code().unwrap_or(-1),
    )
}

/// The arguments the stub was handed, one per line.
fn recorded(sandbox: &Sandbox) -> Vec<String> {
    std::fs::read_to_string(&sandbox.argv)
        .expect("the hook called specline at all")
        .lines()
        .map(str::to_owned)
        .collect()
}

/// KEEL-136. The tree being committed is a worktree, and that is the tree the
/// check has to read — not the checkout Specline has on file.
#[test]
fn the_pre_commit_check_reads_the_worktree_it_is_committing_from() {
    let sandbox = sandbox(0, "");
    let main = sandbox.dir.path().join("main");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("README.md"), "hello\n").unwrap();
    git(&main, &["add", "README.md"]);
    git(&main, &["commit", "-qm", "first"]);

    let side = sandbox.dir.path().join("side");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            side.to_str().unwrap(),
            "-b",
            "side",
        ],
    );
    stage_a_generated_file(&side, "a tracker rendered in the worktree\n");

    let (output, code) = run_pre_commit(&sandbox, &side);
    assert_eq!(code, 0, "a clean check permits the commit: {output}");

    let args = recorded(&sandbox);
    let repo = args
        .iter()
        .position(|a| a == "--repo")
        .and_then(|i| args.get(i + 1))
        .expect("the check is told which tree to read");
    assert_eq!(
        std::path::Path::new(repo).canonicalize().unwrap(),
        side.canonicalize().unwrap(),
        "the check must read the worktree the commit is in, not the recorded checkout. \
         Got: {args:?}"
    );
}

/// The failure case, which is the one the hook exists for: the check says a
/// generated file differs, and the commit is refused with somewhere to go.
#[test]
fn the_pre_commit_hook_refuses_a_commit_when_a_generated_file_has_drifted() {
    let sandbox = sandbox(1, "stale product/STATUS.md");
    let repo = sandbox.dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-qm", "first"]);
    stage_a_generated_file(&repo, "edited by hand\n");

    let (output, code) = run_pre_commit(&sandbox, &repo);
    assert_eq!(code, 1, "drift blocks the commit: {output}");
    assert!(
        output.contains("stale product/STATUS.md"),
        "the refusal repeats what the check said: {output}"
    );
    assert!(
        output.contains("--no-verify"),
        "and says how to override it deliberately: {output}"
    );
}

/// Nothing generated in the commit means no store round-trip, and no opinion.
#[test]
fn the_pre_commit_hook_ignores_a_commit_that_touches_no_generated_file() {
    let sandbox = sandbox(1, "this must never be reached");
    let repo = sandbox.dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("src.rs"), "fn main() {}\n").unwrap();
    git(&repo, &["add", "src.rs"]);

    let (output, code) = run_pre_commit(&sandbox, &repo);
    assert_eq!(code, 0, "{output}");
    assert!(
        !sandbox.argv.exists(),
        "the hook must not call specline at all when nothing generated is staged"
    );
}

/// A store relocation must not land in a `--json` payload.
///
/// `resolve_home` runs before every command that touches the store, and every
/// one of them can be asked for `--json` — at which point stdout is a document
/// somebody parses. A relocation notice printed above it makes the whole
/// thing unparseable, once, on the first run after an upgrade: the single run
/// where somebody is watching to see whether the upgrade worked.
///
/// The session hook is not exposed, because it is dispatched before the home
/// is resolved. That is a real protection but an incidental one — it was put
/// there so a hook would not exit non-zero with `HOME` unset — so this asserts
/// the property on a command that genuinely reaches the code.
///
/// Drives the real binary with a real Keel-shaped home, because the bug lives
/// in the seam between two functions that are each correct alone.
#[test]
fn a_store_relocation_does_not_land_in_a_json_payload() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let fake_home = scratch.path().join("home");
    let legacy = fake_home.join(".keel");
    std::fs::create_dir_all(&legacy).expect("the old home");

    let output = Command::new(env!("CARGO_BIN_EXE_specline"))
        .args(["--json", "status"])
        .env("HOME", &fake_home)
        .env("TMPDIR", scratch.path())
        // Constructed, not inherited. `SPECLINE_HOME` names the store
        // explicitly, and an explicit home is deliberately never relocated —
        // so a run with it set skips the code this test exists for and the
        // assertions below pass on nothing. CI sets it on all three jobs, so
        // this test went green locally and red there: the exact shape the
        // contract warns about, a test reading the machine it runs on.
        .env_remove("SPECLINE_HOME")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the specline binary runs");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        fake_home.join(".specline").is_dir(),
        "the relocation should have happened, or this test proves nothing"
    );
    assert!(
        stderr.contains("moved your store"),
        "and it should still tell somebody: stderr was {stderr:?}"
    );
    assert!(
        !stdout.contains("moved your store"),
        "but not on the stream the payload goes to: stdout was {stdout:?}"
    );
}
