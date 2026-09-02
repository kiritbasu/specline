//! The two Claude Code session hooks, as subcommands.
//!
//! `specline hook session-start` and `specline hook stop`. They were 317 lines of bash
//! and are here for four reasons, the last of which is the one that mattered.
//!
//! **`python3` is not on a clean Mac.** It arrives with the Xcode command line
//! tools. Both scripts parsed JSON with it, so on the machine Phase 10 is aimed
//! at — someone who has installed nothing — every parse failed, every failure
//! path exited 0 silently, and the result was indistinguishable from Specline not
//! working. `install.sh` checked for `jq`, which neither script used.
//!
//! **`curl` was the other undeclared dependency**, and `bash` meant neither
//! could ever run on Windows.
//!
//! **Nothing executed them.** Not the test suite, not CI, not once. KEEL-192
//! was found by reading and fixed by reading, and its fix was guarded by
//! nothing. That is the reason this is worth doing rather than tidying: every
//! other surface in this phase describes itself and is tested, and the hooks
//! were the one surface that did neither.
//!
//! # What did not move
//!
//! A shim stays in `plugin/hooks/`, and it has to. The install flow needs a
//! session to be able to say *"the binary is missing, run `/specline:setup`"* — and
//! a hook that **is** the binary cannot report its own absence. So the shim is
//! the smallest thing that can: it execs this if the binary is there, and
//! prints one sentence if it is not. Everything that can change is on this side
//! of the `exec`.
//!
//! # The rule both of these obey
//!
//! **Never block a session, and never write.** Every failure — an unreachable
//! daemon, a payload that will not parse, a timeout — exits 0. A session that
//! starts with a stack trace, or cannot end because a bookkeeping hook is
//! confused, is a far worse outcome than a missed record.

use crate::writes::{Daemon, probe};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Read;
use std::time::Duration;

/// How long either hook will wait on the daemon.
///
/// This runs before the human's first word, and again at the moment they are
/// waiting for a reply. A local daemon answers in milliseconds; five seconds is
/// three orders of magnitude of slack and still short enough that a wedged
/// daemon is a pause rather than a hang.
///
/// **It shares a budget with something it cannot see.** `plugin/hooks/hooks.json`
/// gives session start ten seconds, and Claude Code kills the hook at that
/// point — a killed hook prints nothing, which is precisely the silence
/// [`unreachable_notice`] exists to end. The slow path is this timeout *plus*
/// `writes::PROBE_TIMEOUT`, so today it is five and one against a ceiling of
/// ten. Raising either past nine reintroduces the bug through its own fix, and
/// no test here would fail, because the budget lives in someone else's JSON.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How far back the Stop hook looks for this session's writes.
///
/// Scoped by time rather than by count alone, and that is a fix rather than a
/// preference: the event log returns oldest-first, so a bare `limit` on a busy
/// store returned everything *except* the recent writes being looked for, and
/// the hook nagged a session that had done exactly the right thing. A session
/// cannot have written before it started, so a window longer than any
/// conversation is both correct and bounded.
const ACTIVITY_WINDOW_HOURS: i64 = 12;

/// What Claude Code sends a hook on stdin.
///
/// Every field is optional because this is someone else's payload and a shape
/// change must not be able to break a session. A missing field means the hook
/// declines to act, never that it fails.
#[derive(Debug, Default, Deserialize)]
pub struct Payload {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    /// `startup`, `resume`, `clear` or `compact`.
    #[serde(default)]
    pub source: Option<String>,
    /// Set when Claude Code is already continuing because a Stop hook blocked.
    #[serde(default)]
    pub stop_hook_active: bool,
}

impl Payload {
    /// Parse, and treat anything unparseable as empty rather than as an error.
    pub fn parse(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_default()
    }

    /// The directory the session is in, falling back to this process's own.
    fn directory(&self) -> String {
        match self.cwd.as_deref() {
            Some(c) if !c.is_empty() => c.to_owned(),
            _ => std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        }
    }
}

/// Read stdin without ever failing.
fn read_stdin() -> String {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    raw
}

/// `GET {daemon}{path}` with the query, or `None` for anything that goes wrong.
///
/// One place where a network answer becomes an `Option`, so no caller has to
/// remember that a failed answer is not an answer.
///
/// `None` used to mean "say nothing" at every call site, which is right for the
/// Stop hook and was wrong for session start — see [`unreachable_notice`].
fn get_json(daemon: &str, path: &str, query: &[(&str, &str)]) -> Option<Value> {
    let mut request = ureq::get(&format!("{}{path}", daemon.trim_end_matches('/')))
        .timeout(TIMEOUT)
        .set("accept", "application/json");
    for (key, value) in query {
        request = request.query(key, value);
    }
    request.call().ok()?.into_json::<Value>().ok()
}

// --- session-start ----------------------------------------------------------

/// The instructions that travel with the digest.
///
/// They live here rather than in `SKILL.md` for one measured reason: a skill is
/// model-invoked, and across thirty headless sessions with `specline` installed and
/// listed it was invoked zero times (TQ-19). An instruction that only exists in
/// a file nobody opens is not an instruction.
///
/// Everything *else* about writing — what each artifact type is for, when a
/// task is really a spec, how to handle a conflict — stays in the skill, which
/// is read once the model has decided to engage. Three statements of the same
/// thing is how they come to disagree.
const PREAMBLE: &str = "Specline holds this project's specs, decisions, tasks, questions and history. \
You did not have to ask for this — it is here so you start oriented. Write back to it when \
something becomes true, with the specline_* tools; the `specline` skill has the detail on what belongs \
where.\n\n\
Record it rather than offering to. In a measured run, five of ten sessions worked out exactly \
what should be captured, drafted it, then asked permission and stopped — so it was lost. Write \
it, then say in one line that you did.\n\n\
If you pick up one of the tasks under Next below, set it to in_progress before you start. It is \
one call, and it is the only way the human can see what is being worked on now rather than only \
what has finished.\n\n";

/// The line that pins the session id to Claude Code's own.
///
/// This removes a whole failure class. Asked to invent a unique id, sessions
/// minted date-based ones; two sharing a day collided, and a gate run scored
/// five writing sessions as three — which is the number a strategy was then
/// built on. It also makes the event log joinable to the transcript.
fn session_hint(claude_session: &str) -> String {
    format!(
        "Use exactly this on every Specline call: session_id = \"ses_{claude_session}\". It is this \
         conversation's own identifier — do not invent one, and do not derive one from the \
         date.\n\n"
    )
}

/// How much of the digest to inject.
///
/// An unmatched directory gets the first paragraph and no more. The digest
/// already leads with the "no project matches" sentence in that case, and the
/// rest is a roll-up of unrelated projects — context spent on other people's
/// business is context taken from the work in front of the session.
///
/// Returns `None` when there is nothing worth saying, which the caller turns
/// into silence rather than into an empty injection.
fn digest_to_inject(body: &Value) -> Option<String> {
    let summary = body.get("summary")?.as_str()?.trim();
    if summary.is_empty() {
        return None;
    }

    let matched = body
        .get("data")
        .and_then(|d| d.get("project"))
        .is_some_and(|p| !p.is_null() && p.as_object().is_some_and(|o| !o.is_empty()));

    if matched {
        Some(summary.to_owned())
    } else {
        Some(
            summary
                .split("\n\n")
                .next()
                .unwrap_or(summary)
                .trim()
                .to_owned(),
        )
    }
}

/// The full `additionalContext`, or `None` for silence.
fn session_start_context(body: &Value, claude_session: Option<&str>) -> Option<String> {
    let digest = digest_to_inject(body)?;
    let hint = claude_session
        .filter(|s| !s.is_empty())
        .map(session_hint)
        .unwrap_or_default();
    Some(format!("{PREAMBLE}{hint}{digest}"))
}

/// What a session is told when the digest could not be fetched.
///
/// The silence this replaces was the whole of a user report — *"a heads-up when
/// Specline isn't connected would help, since it currently fails silently"*. A
/// session starting against a daemon that was down looked exactly like one
/// starting against a daemon that was up: no orientation, no warning. So the
/// model worked unoriented, and — the part that actually costs something —
/// never ran the ritual that records anything. A failed write announces itself.
/// A ritual that never fires is indistinguishable from a quiet day.
///
/// The sibling case has said its piece for a long time: the shim in
/// `plugin/hooks/specline-hook.sh` tells a session when the *binary* is missing.
/// This is the same sentence for the other cause, and it belongs here rather
/// than there because a hook that reached the binary can name the address it
/// tried and say why it failed.
///
/// Never returns `None`. Deciding there was nothing worth saying is what
/// produced the bug.
fn unreachable_notice(daemon: &str) -> String {
    let base = daemon.trim_end_matches('/');

    // Which of the three it is changes the advice, so it is worth the extra
    // second on a path that has already failed. Telling someone to start a
    // daemon that is already running sends them to fix the wrong thing.
    let cause = match probe(base) {
        Daemon::NotRunning => format!("Specline's daemon is not running at {base}"),
        Daemon::Unknown(reason) => {
            format!("Specline's daemon could not be reached at {base} ({reason})")
        }
        Daemon::Listening => {
            format!("Something is listening at {base}, but it did not answer with a project digest")
        }
    };

    format!(
        "{cause}, so this session has no project context and the specline_* tools will not \
         answer.\n\nSay so rather than working as though Specline were not installed: nothing \
         this conversation decides or learns will be recorded until it is reachable. Start it \
         with `specline-daemon`, or run /specline:setup to reinstall the agent that keeps it \
         running."
    )
}

/// Put the digest into the session before anything else does.
///
/// Always exits 0. Prints nothing when the daemon answered and had nothing
/// worth injecting — and says why when it did not answer at all.
pub fn session_start(daemon: &str) {
    let payload = Payload::parse(&read_stdin());

    // A compaction is not a session start. Claude Code fires `SessionStart`
    // again after every compaction, and re-injecting the preamble and digest
    // there spent hundreds of tokens restating what the conversation already
    // knew — at the one moment context was scarcest, which is why compaction
    // was happening. Both the identity and the orientation survive in the
    // summary; what was being re-sent was noise.
    if payload.source.as_deref() == Some("compact") {
        return;
    }

    let context = match get_json(
        daemon,
        "/api/context",
        &[("cwd", &payload.directory()), ("depth", "brief")],
    ) {
        // A digest came back. It can still hold nothing worth injecting, and
        // that silence is the deliberate one: a directory Specline has never
        // heard of has nothing to say and should not spend context saying it.
        Some(body) => match session_start_context(&body, payload.session_id.as_deref()) {
            Some(context) => context,
            None => return,
        },
        None => unreachable_notice(daemon),
    };

    // Printed as JSON rather than as bare text so the payload cannot be
    // mistaken for a transcript line.
    println!(
        "{}",
        json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": context,
            }
        })
    );
}

// --- stop -------------------------------------------------------------------

/// What the Stop hook says when a session recorded nothing.
///
/// One sentence, and a question about *this* conversation rather than an
/// instruction to use a tool — the failure it addresses is not reluctance, it
/// is that Specline was out of mind entirely. It used to spend a paragraph
/// re-teaching what to write, which the session-start hook and the skill had
/// both already covered.
const NUDGE: &str = "Nothing from this session reached Specline. If anything became true here, \
record it now and say in one line what you recorded; if nothing did, say so and stop.";

/// Whether any event in the feed belongs to this session.
///
/// Both spellings are accepted because the hook hands the model `ses_<uuid>`
/// while Claude Code's own id is the bare uuid, and a session that used either
/// has written.
fn session_wrote(events: &Value, claude_session: &str) -> bool {
    let prefixed = format!("ses_{claude_session}");
    events
        .get("data")
        .and_then(|d| d.get("events"))
        .and_then(Value::as_array)
        .is_some_and(|list| {
            list.iter()
                .filter_map(|e| e.get("session_id").and_then(Value::as_str))
                .any(|id| id == prefixed || id == claude_session)
        })
}

/// Whether `/api/context` resolved the directory to a project.
fn directory_is_a_project(body: &Value) -> bool {
    body.get("data")
        .and_then(|d| d.get("project"))
        .is_some_and(|p| !p.is_null() && p.as_object().is_some_and(|o| !o.is_empty()))
}

/// Where the once-per-session markers live.
fn marker_dir() -> std::path::PathBuf {
    std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("specline-stop-hook")
}

/// Ask, once, whether anything from this session should have been recorded.
///
/// Silent unless every one of these holds: the payload names a session, Claude
/// Code is not already continuing because of a stop hook, this session has not
/// been asked before, the directory resolves to a project, and the store has no
/// event carrying this session's id.
pub fn stop(daemon: &str) {
    let payload = Payload::parse(&read_stdin());

    let Some(claude_session) = payload.session_id.as_deref().filter(|s| !s.is_empty()) else {
        return;
    };

    // Without this the hook blocks its own continuation, for ever.
    if payload.stop_hook_active {
        return;
    }

    // One nudge per session, held by a file rather than by trust.
    let marker = marker_dir().join(claude_session);
    if marker.exists() {
        return;
    }

    // Is this directory a Specline project at all? KEEL-192: the activity check
    // below is global, so a session in an unrelated repository has no events
    // and was nagged for not filing notes about a project that does not exist.
    // Resolving the directory first turns "wrote nothing" into "wrote nothing
    // about the project it is standing in", which is the question this meant to
    // ask all along.
    //
    // An unreachable daemon means silence, which is the *opposite* of the
    // choice made for the activity check below, and deliberately so. There, not
    // knowing means "assume it wrote"; here, not knowing means "assume no
    // project". Both roads lead to saying nothing, which is the only safe thing
    // a bookkeeping hook can do when it cannot tell.
    let Some(context) = get_json(
        daemon,
        "/api/context",
        &[("cwd", &payload.directory()), ("depth", "brief")],
    ) else {
        return;
    };
    if !directory_is_a_project(&context) {
        return;
    }

    // Did this session already record something? The store is the only honest
    // answer — a session can talk about recording without doing it, which is
    // the entire failure this project exists to measure.
    let since = (chrono::Utc::now() - chrono::Duration::hours(ACTIVITY_WINDOW_HOURS))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let Some(events) = get_json(
        daemon,
        "/api/activity",
        &[("limit", "500"), ("since", &since)],
    ) else {
        // Unreachable or unparseable: assume it wrote, and stay quiet. A false
        // nudge on every session in a project whose daemon is down would make
        // this the most annoying thing in the toolchain.
        return;
    };
    if session_wrote(&events, claude_session) {
        return;
    }

    // Best effort. A marker that cannot be written means a session might be
    // asked twice, which is a much smaller cost than refusing to ask at all.
    let _ = std::fs::create_dir_all(marker_dir());
    let _ = std::fs::write(&marker, b"");

    println!("{}", json!({ "decision": "block", "reason": NUDGE }));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // --- what a session is told when the digest never arrives ---------------

    /// The third arm, which the integration tests cannot reach.
    ///
    /// `hooks.rs` covers connection-refused and listening-but-broken by binding
    /// real sockets. Reaching `Unknown` that way would need a dropped packet or
    /// a DNS failure — machine-dependent, and this repository has twice shipped
    /// a test that passed on a Mac and failed on Linux for exactly that kind of
    /// reason. An address with no port fails in `socket_addr` before anything
    /// touches the network, so it is the same branch without the coin toss.
    #[test]
    fn an_address_with_no_port_is_reported_as_unreachable_not_as_absent() {
        let notice = unreachable_notice("http://127.0.0.1");

        assert!(
            notice.contains("could not be reached"),
            "an address that cannot be parsed is not the same as nobody listening: {notice}"
        );
        assert!(
            !notice.contains("is not running"),
            "claiming the daemon is down would send someone to start one that may well be up: \
             {notice}"
        );
    }

    /// Every arm ends with something the reader can act on.
    ///
    /// The point of this function is that it is never silent, so the failure
    /// worth guarding is an arm that says what went wrong and stops there.
    #[test]
    fn every_notice_says_what_to_do_about_it() {
        for daemon in ["http://127.0.0.1:1", "http://127.0.0.1"] {
            let notice = unreachable_notice(daemon);
            assert!(notice.contains("specline-daemon"), "{daemon}: {notice}");
            assert!(notice.contains("Say so"), "{daemon}: {notice}");
            assert!(!notice.trim().is_empty(), "{daemon}");
        }
    }

    // --- payload parsing ----------------------------------------------------

    /// Someone else's payload, so a shape change must not break a session.
    #[test]
    fn an_unparseable_payload_is_empty_rather_than_an_error() {
        let payload = Payload::parse("not json at all");
        assert!(payload.session_id.is_none());
        assert!(!payload.stop_hook_active);
    }

    #[test]
    fn a_payload_reads_the_fields_the_hooks_use() {
        let payload = Payload::parse(
            r#"{"cwd":"/tmp/x","session_id":"abc","source":"startup","stop_hook_active":true}"#,
        );
        assert_eq!(payload.cwd.as_deref(), Some("/tmp/x"));
        assert_eq!(payload.session_id.as_deref(), Some("abc"));
        assert_eq!(payload.source.as_deref(), Some("startup"));
        assert!(payload.stop_hook_active);
    }

    /// Fields this does not know about must be ignored, not rejected.
    #[test]
    fn an_unknown_field_does_not_discard_the_payload() {
        let payload = Payload::parse(r#"{"session_id":"abc","something_new":42}"#);
        assert_eq!(payload.session_id.as_deref(), Some("abc"));
    }

    // --- what session-start injects ----------------------------------------

    fn digest(summary: &str, project: Value) -> Value {
        json!({ "summary": summary, "data": { "project": project } })
    }

    #[test]
    fn a_matched_directory_gets_the_whole_digest() {
        let body = digest(
            "first para\n\nsecond para\n\nthird",
            json!({"slug": "specline"}),
        );
        let context = session_start_context(&body, Some("abc")).unwrap();
        assert!(context.contains("first para"));
        assert!(
            context.contains("third"),
            "the whole digest, not just the head"
        );
    }

    /// The rule that keeps a session in an unrelated repository from being
    /// handed three screens of other projects' business.
    #[test]
    fn an_unmatched_directory_gets_only_the_first_paragraph() {
        let body = digest(
            "no project matches this directory\n\nAcme Corp\n\nWidgets",
            json!(null),
        );
        let context = session_start_context(&body, Some("abc")).unwrap();
        assert!(context.contains("no project matches this directory"));
        assert!(
            !context.contains("Acme Corp"),
            "an unrelated project's name must not be injected: {context}"
        );
    }

    #[test]
    fn an_empty_summary_injects_nothing() {
        assert!(
            session_start_context(&digest("   ", json!({"slug": "specline"})), Some("a")).is_none()
        );
        assert!(session_start_context(&json!({}), Some("a")).is_none());
    }

    /// The id has to be the one Claude Code assigned, or two sessions in a day
    /// collide and the event log stops joining to the transcript.
    #[test]
    fn the_session_id_is_pinned_to_claude_codes_own() {
        let body = digest("something", json!({"slug": "specline"}));
        let context = session_start_context(&body, Some("11112222")).unwrap();
        assert!(context.contains("ses_11112222"), "{context}");
    }

    #[test]
    fn no_session_id_means_no_hint_rather_than_a_broken_one() {
        let body = digest("something", json!({"slug": "specline"}));
        let context = session_start_context(&body, None).unwrap();
        assert!(!context.contains("ses_"), "{context}");
        assert!(context.contains("something"));
    }

    // --- what stop decides --------------------------------------------------

    #[test]
    fn a_session_that_wrote_is_recognised_by_either_spelling() {
        let events = json!({"data": {"events": [{"session_id": "ses_abc"}]}});
        assert!(session_wrote(&events, "abc"));

        let bare = json!({"data": {"events": [{"session_id": "abc"}]}});
        assert!(session_wrote(&bare, "abc"));
    }

    #[test]
    fn another_sessions_writes_do_not_count() {
        let events = json!({"data": {"events": [{"session_id": "ses_someone_else"}]}});
        assert!(!session_wrote(&events, "abc"));
    }

    #[test]
    fn an_empty_or_malformed_feed_counts_as_no_writes() {
        assert!(!session_wrote(&json!({"data": {"events": []}}), "abc"));
        assert!(!session_wrote(&json!({}), "abc"));
    }

    /// KEEL-192, and the reason this file exists: the behaviour was fixed by
    /// reading and guarded by nothing.
    #[test]
    fn a_directory_with_no_project_is_not_a_specline_directory() {
        assert!(directory_is_a_project(
            &json!({"data": {"project": {"slug": "specline"}}})
        ));
        assert!(!directory_is_a_project(&json!({"data": {"project": null}})));
        assert!(!directory_is_a_project(&json!({"data": {"project": {}}})));
        assert!(!directory_is_a_project(&json!({"data": {}})));
        assert!(!directory_is_a_project(&json!({})));
    }
}
