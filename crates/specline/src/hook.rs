//! The Claude Code session hooks, as subcommands.
//!
//! `specline hook session-start` and `specline hook stop` were 317 lines of bash
//! and are here for four reasons, the last of which is the one that mattered.
//! `specline hook commit` (KEEL-395) was born here, for the same last reason.
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
//! # The rule all of these obey
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

/// How long the session-start and Stop hooks will wait on the daemon.
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
///
/// The commit hook has its own, shorter one — see [`COMMIT_TIMEOUT`].
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
    /// `PostToolUse` only: which tool just ran.
    #[serde(default)]
    pub tool_name: Option<String>,
    /// `PostToolUse` only: what the tool was given. For Bash, `{"command": …}`.
    #[serde(default)]
    pub tool_input: Option<Value>,
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
    get_json_within(daemon, path, query, TIMEOUT)
}

/// [`get_json`] with a timeout other than the default.
fn get_json_within(
    daemon: &str,
    path: &str,
    query: &[(&str, &str)],
    timeout: Duration,
) -> Option<Value> {
    let mut request = ureq::get(&format!("{}{path}", daemon.trim_end_matches('/')))
        .timeout(timeout)
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

/// How long session start will wait to learn which hooks are wired.
///
/// The files are small, but the project's live on whatever volume the session
/// is in, and on 2026-09-24 that volume stalled an `open()` for minutes
/// (KEEL-403). This hook has ten seconds in all and the digest matters more
/// than the notice, so the read gets a fraction of a second on a side thread
/// and is abandoned, not waited for.
const WIRING_BUDGET: Duration = Duration::from_millis(300);

/// [`crate::wiring::session_notice`] for `directory`, or `None` if reading the
/// settings takes longer than `budget`.
fn wiring_notice_within(directory: String, budget: Duration) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let notice = crate::wiring::examine(Some(std::path::Path::new(&directory)))
            .as_ref()
            .and_then(crate::wiring::session_notice);
        let _ = tx.send(notice);
    });
    rx.recv_timeout(budget).ok().flatten()
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
            // One line, and only when something is wrong: a hook that is
            // installed and never runs looks exactly like Specline having
            // nothing to say, and nobody runs `doctor` unprompted (KEEL-396).
            Some(context) => match wiring_notice_within(payload.directory(), WIRING_BUDGET) {
                Some(notice) => format!("{context}\n\n{notice}"),
                None => context,
            },
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

/// The Stop hook's once-per-session markers.
const STOP_MARKERS: &str = "specline-stop-hook";

/// Where one hook's once-per-session markers live.
///
/// One directory per hook, so asking once at a commit does not count as
/// having asked at Stop — they are different questions.
fn marker_dir(hook: &str) -> std::path::PathBuf {
    std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(hook)
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
    let marker = marker_dir(STOP_MARKERS).join(claude_session);
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
    let _ = std::fs::create_dir_all(marker_dir(STOP_MARKERS));
    let _ = std::fs::write(&marker, b"");

    println!("{}", json!({ "decision": "block", "reason": NUDGE }));
}

// --- commit -----------------------------------------------------------------

/// The commit hook's once-per-session markers.
const COMMIT_MARKERS: &str = "specline-commit-hook";

/// How long each of the commit hook's daemon calls may take.
///
/// It makes up to three, after asking git, inside the ten seconds
/// `plugin/hooks/hooks.json` gives it. At the default five a slow daemon took
/// the hook past that line, and Claude Code reports a killed hook as an error
/// after the commit — the opposite of advice that stays out of the way. Three
/// calls at two seconds plus git's own limit leaves room.
const COMMIT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long `git log` may take before the hook gives up on it.
///
/// A repository on a network mount that has gone away hangs rather than
/// failing, and a hook that hangs is killed noisily.
const GIT_TIMEOUT: Duration = Duration::from_secs(2);

/// How recently this session must have touched a task for a commit with no
/// key to count as covered.
///
/// The contract's own end-of-session order is close, regenerate, commit, and
/// closing releases the claim — so without this the ritual done correctly
/// drew the nag. Fifteen minutes covers that tail, and is short enough that
/// the case this hook exists for (a task closed at 20:18, then forty-four
/// minutes of commits no row describes) is still told.
const RECENT_TASK_MINUTES: i64 = 15;

/// How recent `HEAD` has to be to count as the commit this Bash call made.
///
/// The hook runs after the command, and git stamps a commit after its own
/// pre-commit hook has finished, so a commit this call made is seconds old.
/// Two minutes is slack for a slow machine, and short enough that a `git log`
/// run long after the last commit does not read as a new one.
const FRESH_SECS: i64 = 120;

/// The commit `HEAD` points at, as far as this hook needs it.
#[derive(Debug, PartialEq)]
struct Head {
    sha: String,
    committed_at: i64,
    parents: usize,
    message: String,
}

/// Parse `git log -1 --format=%H%x1f%ct%x1f%P%x1f%B`.
///
/// `None` for anything that does not look like that, which the caller turns
/// into silence: this reads someone else's repository, and a shape it did not
/// expect is a reason to say nothing rather than to guess.
fn parse_head(raw: &str) -> Option<Head> {
    let mut fields = raw.splitn(4, '\u{1f}');
    let sha = fields.next()?.trim();
    let committed_at = fields.next()?.trim().parse().ok()?;
    let parents = fields.next()?.split_whitespace().count();
    let message = fields.next()?.trim();
    if sha.len() < 7 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(Head {
        sha: sha.to_owned(),
        committed_at,
        parents,
        message: message.to_owned(),
    })
}

/// Whether a Bash command could have made a commit.
///
/// A cheap gate, not a parser. Commits arrive as `-m`, heredocs, `-F`, `-am`,
/// `git -C dir commit` and chains, and parsing all of that is how this would
/// come to miss one. So the gate only decides whether it is worth asking git,
/// and git says what actually happened. It runs on every Bash call, so it has
/// to be the thing that makes the common case cost nothing.
fn could_commit(command: &str) -> bool {
    command.contains("git") && command.contains("commit")
}

/// Whether `HEAD` is a commit this Bash call plausibly just made.
///
/// A merge is left out: its message is git's, not the author's, and the work
/// it brings in was committed — and judged — on its own branch.
fn is_fresh_commit(head: &Head, now: i64) -> bool {
    head.parents <= 1 && now - head.committed_at <= FRESH_SECS
}

/// Whether this session touched a task recently, from `/api/activity`.
///
/// A feed the daemon cut short counts as yes, for the reason
/// [`session_may_hold_claim`] gives.
fn session_touched_a_task(body: &Value, claude_session: &str) -> bool {
    let data = body.get("data");
    if data
        .and_then(|d| d.get("truncated"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    let prefixed = format!("ses_{claude_session}");
    data.and_then(|d| d.get("events"))
        .and_then(Value::as_array)
        .is_some_and(|events| {
            events.iter().any(|e| {
                e.get("entity_type").and_then(Value::as_str) == Some("task")
                    && e.get("session_id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| id == prefixed || id == claude_session)
            })
        })
}

/// Whether this session may hold a claim, from `/api/entities`.
///
/// Both spellings of the session id, for the same reason as [`session_wrote`].
/// A list the daemon says it cut short counts as "may": the claim could be in
/// the part that was not sent, and a nag built on half a list is a false one.
fn session_may_hold_claim(body: &Value, claude_session: &str) -> bool {
    let data = body.get("data");
    if data
        .and_then(|d| d.get("truncated"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    let prefixed = format!("ses_{claude_session}");
    data.and_then(|d| d.get("items"))
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .filter_map(|t| t.get("claimed_by").and_then(Value::as_str))
                .any(|by| by == prefixed || by == claude_session)
        })
}

/// What the agent is told.
///
/// Advice, and phrased as advice, because the decision (KEEL-395) is that
/// this never blocks: a check that refuses commits teaches a model to paste
/// any recent key into the message, which makes the key meaningless. So it
/// says what it saw, what would fix it, and that carrying on is allowed.
fn commit_notice(key: &str, head: &Head) -> String {
    let short: String = head.sha.chars().take(7).collect();
    let subject = subject_for_notice(&head.message);
    format!(
        "Specline: commit {short} (\"{subject}\") names no {key} task, and this session has \
         not claimed one, so nothing on the board describes this work. If a row should, \
         create it with specline_create, claim it with specline_claim, and name it in the \
         next commit or cite commit:{short} when you close it. If this commit genuinely needs \
         no row, carry on. This is said once per session."
    )
}

/// The subject line as it goes into the model's context: one line, no control
/// characters, and short.
///
/// The text is usually the agent's own, but it is a commit message, and it
/// lands in context under Specline's name. So it is bounded rather than
/// trusted: long enough to recognise, too short to carry a paragraph.
fn subject_for_notice(message: &str) -> String {
    const LIMIT: usize = 120;
    let line = message.lines().next().unwrap_or("").trim();
    let clean: String = line.chars().filter(|c| !c.is_control()).collect();
    if clean.chars().count() > LIMIT {
        let cut: String = clean.chars().take(LIMIT).collect();
        format!("{cut}…")
    } else {
        clean
    }
}

/// A session id that is safe to use as a file name.
///
/// Claude Code's are UUIDs; anything else is somebody else's payload, and it
/// becomes a path under `$TMPDIR` here.
fn safe_session(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Mark this session as told, and say whether this call is the one that did.
///
/// `create_new` is the test and the set in one step. Checking for the file
/// and then writing it let two copies of the hook — one from the plugin, one
/// from a hand-written settings file — both pass the check and both speak.
fn first_to_tell(marker: &std::path::Path) -> bool {
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker)
        .is_ok()
}

/// Run `git log -1` in `dir`, or `None` for anything that goes wrong.
///
/// `--no-show-signature` because `log.showSignature` in someone's config puts
/// gpg's output ahead of the format, and every signed commit would then fail
/// to parse — silence that looks like the hook working.
fn read_head(dir: &str) -> Option<Head> {
    let mut child = std::process::Command::new("git")
        .args([
            "-C",
            dir,
            "log",
            "-1",
            "--no-show-signature",
            "--format=%H%x1f%ct%x1f%P%x1f%B",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let deadline = std::time::Instant::now() + GIT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_head(&String::from_utf8_lossy(&output.stdout))
}

/// Tell the agent, once, when it commits work no row describes.
///
/// Registered on `PostToolUse` for Bash. Silent unless every one of these
/// holds: the call was Bash, the command mentions a commit, `HEAD` in the
/// session's directory is a fresh non-merge commit, the directory is a
/// Specline project, the message names none of its tasks, this session holds
/// no claim and has touched no task in the last [`RECENT_TASK_MINUTES`], and
/// it has not been told before.
///
/// Known blind spots, accepted because this is advice rather than a gate:
/// `HEAD` is read in the session's directory, so `git -C elsewhere commit`
/// is judged by the wrong repository — usually silence. A commit another
/// session made within [`FRESH_SECS`] can be attributed to this one if this
/// one then runs a command mentioning git and commit. A backgrounded Bash call
/// returns before its commit exists, so it is not seen at all.
///
/// **An unreachable daemon means silence.** Without it there is no project key
/// to look for and no way to tell a Specline checkout from any other, and the
/// session-start hook has already said the daemon is down. Guessing at a key
/// pattern would nag in every repository on the machine.
///
/// Writes nothing to the store. It is the agent's job to decide whether the
/// work deserves a row; this only makes sure it is asked while it can still
/// act, rather than in the next session's digest.
pub fn commit(daemon: &str) {
    let payload = Payload::parse(&read_stdin());

    if payload.tool_name.as_deref() != Some("Bash") {
        return;
    }
    let command = payload
        .tool_input
        .as_ref()
        .and_then(|i| i.get("command"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !could_commit(command) {
        return;
    }
    let Some(claude_session) = payload.session_id.as_deref().filter(|s| safe_session(s)) else {
        return;
    };
    let marker = marker_dir(COMMIT_MARKERS).join(claude_session);
    if marker.exists() {
        return;
    }

    let directory = payload.directory();
    let Some(head) = read_head(&directory) else {
        return;
    };
    if !is_fresh_commit(&head, chrono::Utc::now().timestamp()) {
        return;
    }

    let Some(context) = get_json_within(
        daemon,
        "/api/context",
        &[("cwd", &directory), ("depth", "brief")],
        COMMIT_TIMEOUT,
    ) else {
        return;
    };
    let project = context.get("data").and_then(|d| d.get("project"));
    let (Some(key), Some(project_id)) = (
        project
            .and_then(|p| p.get("key"))
            .and_then(Value::as_str)
            .filter(|k| !k.is_empty()),
        project.and_then(|p| p.get("id")).and_then(Value::as_str),
    ) else {
        return;
    };
    if specline_core::drift::names_a_task(&head.message, key) {
        return;
    }

    // Not knowing whether this session is keeping the tracker means saying
    // nothing: the Stop hook's rule, for the Stop hook's reason.
    let since = (chrono::Utc::now() - chrono::Duration::minutes(RECENT_TASK_MINUTES))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let Some(recent) = get_json_within(
        daemon,
        "/api/activity",
        &[("limit", "500"), ("since", &since)],
        COMMIT_TIMEOUT,
    ) else {
        return;
    };
    if session_touched_a_task(&recent, claude_session) {
        return;
    }
    let Some(in_progress) = get_json_within(
        daemon,
        "/api/entities",
        &[
            ("project", project_id),
            ("type", "task"),
            ("status", "in_progress"),
            ("limit", "500"),
        ],
        COMMIT_TIMEOUT,
    ) else {
        return;
    };
    if session_may_hold_claim(&in_progress, claude_session) {
        return;
    }

    if !first_to_tell(&marker) {
        return;
    }

    println!(
        "{}",
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": commit_notice(key, &head),
            }
        })
    );
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

    // --- what commit decides ------------------------------------------------

    fn head(message: &str, parents: usize, committed_at: i64) -> Head {
        Head {
            sha: "0123456789abcdef0123456789abcdef01234567".into(),
            committed_at,
            parents,
            message: message.into(),
        }
    }

    #[test]
    fn a_git_log_line_parses_into_a_head() {
        let raw = "0123456789abcdef\u{1f}1700000000\u{1f}aaaa\u{1f}fix: a thing\n\nBody KEEL-4\n";
        let parsed = parse_head(raw).unwrap();
        assert_eq!(parsed.sha, "0123456789abcdef");
        assert_eq!(parsed.committed_at, 1_700_000_000);
        assert_eq!(parsed.parents, 1);
        assert!(
            parsed.message.contains("Body KEEL-4"),
            "the body, not only the subject"
        );
    }

    #[test]
    fn a_root_commit_has_no_parents_and_still_parses() {
        let parsed = parse_head("0123456789abcdef\u{1f}1700000000\u{1f}\u{1f}init").unwrap();
        assert_eq!(parsed.parents, 0);
    }

    /// Someone else's output: a shape this did not expect is silence, never a guess.
    #[test]
    fn output_that_is_not_a_git_log_line_is_not_a_head() {
        assert!(parse_head("").is_none());
        assert!(parse_head("fatal: not a git repository").is_none());
        assert!(parse_head("nothex!\u{1f}1\u{1f}\u{1f}m").is_none());
        assert!(parse_head("0123456789abcdef\u{1f}yesterday\u{1f}\u{1f}m").is_none());
    }

    #[test]
    fn the_gate_passes_every_way_a_commit_is_spelled() {
        for command in [
            "git commit -m 'x'",
            "git add -A && git commit -am x",
            "git -C crates commit -F msg.txt",
            "git commit -m \"$(cat <<'EOF'\nfix: x\nEOF\n)\"",
        ] {
            assert!(could_commit(command), "{command}");
        }
    }

    /// The common case, which has to cost nothing because it is every Bash call.
    #[test]
    fn the_gate_turns_away_commands_that_cannot_commit() {
        for command in ["cargo test", "ls -la", "git status", "echo commit"] {
            assert!(!could_commit(command), "{command}");
        }
    }

    #[test]
    fn only_a_recent_non_merge_commit_is_fresh() {
        let now = 1_700_000_000;
        assert!(is_fresh_commit(&head("m", 1, now - 5), now));
        assert!(is_fresh_commit(&head("m", 0, now), now), "a root commit");
        assert!(
            !is_fresh_commit(&head("m", 1, now - FRESH_SECS - 1), now),
            "an old HEAD is not this call's commit"
        );
        assert!(
            !is_fresh_commit(&head("Merge branch 'x'", 2, now), now),
            "a merge's message is git's, not the author's"
        );
    }

    #[test]
    fn a_claim_is_found_under_either_spelling_of_the_session() {
        let prefixed = json!({"data": {"items": [{"claimed_by": "ses_abc"}], "truncated": false}});
        assert!(session_may_hold_claim(&prefixed, "abc"));
        let bare = json!({"data": {"items": [{"claimed_by": "abc"}]}});
        assert!(session_may_hold_claim(&bare, "abc"));
    }

    #[test]
    fn another_sessions_claim_is_not_this_ones() {
        let body = json!({"data": {"items": [{"claimed_by": "ses_other"}, {"claimed_by": null}]}});
        assert!(!session_may_hold_claim(&body, "abc"));
        assert!(!session_may_hold_claim(&json!({}), "abc"));
    }

    /// Half a list cannot prove a claim is absent, so it must not produce a nag.
    #[test]
    fn a_truncated_list_is_treated_as_possibly_holding_the_claim() {
        let body = json!({"data": {"items": [{"claimed_by": "ses_other"}], "truncated": true}});
        assert!(session_may_hold_claim(&body, "abc"));
    }

    #[test]
    fn the_notice_names_the_commit_and_says_carrying_on_is_allowed() {
        let notice = commit_notice("KEEL", &head("fix: the thing\n\nbody", 1, 0));
        assert!(notice.contains("0123456"), "{notice}");
        assert!(
            notice.contains("\"fix: the thing\""),
            "subject only: {notice}"
        );
        assert!(!notice.contains("body"), "{notice}");
        assert!(notice.contains("KEEL"), "{notice}");
        assert!(
            notice.contains("carry on"),
            "advice, not a refusal: {notice}"
        );
    }

    /// The contract's own order is close, regenerate, commit — and closing
    /// releases the claim. That commit must not be told off.
    #[test]
    fn a_task_this_session_just_closed_covers_the_commit_after_it() {
        let feed = json!({"data": {"events": [
            {"entity_type": "task", "session_id": "ses_abc", "action": "status_changed"}
        ], "truncated": false}});
        assert!(session_touched_a_task(&feed, "abc"));
    }

    #[test]
    fn another_sessions_task_or_this_sessions_note_does_not_count() {
        let feed = json!({"data": {"events": [
            {"entity_type": "task", "session_id": "ses_other"},
            {"entity_type": "question", "session_id": "ses_abc"}
        ]}});
        assert!(!session_touched_a_task(&feed, "abc"));
        assert!(!session_touched_a_task(&json!({}), "abc"));
        assert!(session_touched_a_task(
            &json!({"data": {"events": [], "truncated": true}}),
            "abc"
        ));
    }

    #[test]
    fn the_subject_is_one_clean_bounded_line() {
        assert_eq!(subject_for_notice("fix: x\n\nbody"), "fix: x");
        assert_eq!(subject_for_notice("a\u{7}b\tc"), "abc");
        let long = "x".repeat(500);
        let cut = subject_for_notice(&long);
        assert_eq!(cut.chars().count(), 121, "120 and an ellipsis");
    }

    #[test]
    fn only_a_plain_session_id_becomes_a_file_name() {
        assert!(safe_session("0df284ed-4b35-4da4-907f-e7b1d74792a8"));
        assert!(safe_session("ses_abc123"));
        assert!(!safe_session(""));
        assert!(!safe_session("../../etc"));
        assert!(!safe_session("a/b"));
    }

    #[test]
    fn only_the_first_caller_gets_to_tell() {
        let dir = std::env::temp_dir().join(format!("specline-first-{}", std::process::id()));
        let marker = dir.join("session");
        let _ = std::fs::remove_file(&marker);
        assert!(first_to_tell(&marker));
        assert!(!first_to_tell(&marker));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_payload_reads_the_tool_fields_the_commit_hook_uses() {
        let payload = Payload::parse(
            r#"{"session_id":"abc","tool_name":"Bash","tool_input":{"command":"git commit -m x"}}"#,
        );
        assert_eq!(payload.tool_name.as_deref(), Some("Bash"));
        assert_eq!(
            payload.tool_input.unwrap()["command"].as_str(),
            Some("git commit -m x")
        );
    }
}
