//! `specline ready`, `specline claim` and `specline close` — the three work verbs.
//!
//! # Why these go through the daemon
//!
//! Writes have to: hard constraint 1, the daemon owns the single write path.
//! Reads go the same way so that the CLI and a model see the same store at the
//! same moment — the daemon is the only thing that has seen every write (TQ-15).
//!
//! So all three call the daemon and fall back to the store only when nothing is
//! listening, which is the one moment opening it directly is unambiguous. `ready` uses
//! the local API, and the two writes use the MCP endpoint, so the CLI and a model
//! are calling literally the same code rather than two implementations that agree
//! until they do not.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use specline_core::{CloseReason, Store};
use std::path::Path;

/// Print `specline ready`.
#[allow(clippy::too_many_arguments)]
pub fn ready(
    home: &Path,
    daemon: &str,
    project: &str,
    unclaimed: bool,
    labels: &[String],
    no_labels: &[String],
    milestone: Option<&str>,
    limit: usize,
    json_out: bool,
) -> Result<()> {
    let mut args = json!({
        "project": project,
        "limit": limit,
        "surface": "cli",
    });
    if unclaimed {
        args["unclaimed"] = json!(true);
    }
    if !labels.is_empty() {
        args["labels"] = json!(labels);
    }
    if !no_labels.is_empty() {
        args["without_labels"] = json!(no_labels);
    }
    if let Some(m) = milestone {
        args["milestone"] = json!(m);
    }

    let structured = match call_daemon(daemon, "specline_next", &args)? {
        Some(v) => v,
        None => directly(home, |store| {
            let mut s = store;
            specline_mcp::dispatch(
                &mut s,
                specline_mcp::ToolCall {
                    name: "specline_next",
                    arguments: &args,
                    client: None,
                },
            )
            .map(|v| specline_mcp::payload(&v))
            .map_err(|e| anyhow::anyhow!("{}", e.message))
        })?,
    };

    if json_out {
        println!("{}", serde_json::to_string_pretty(&structured)?);
        return Ok(());
    }

    let items = structured
        .get("ready")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        println!("nothing ready");
        return Ok(());
    }
    for item in &items {
        let field = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or("");
        println!("  {:<10} {}", field("reference"), field("title"));
        println!("             {}", field("why"));
    }

    // Hard constraint 4: a list that was cut says so, with the total. Ten of ten
    // reads exactly like ten of ninety otherwise.
    let total = structured.get("total").and_then(Value::as_u64).unwrap_or(0);
    if structured
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        println!(
            "\n{} of {total} shown — raise --limit for the rest",
            items.len()
        );
    } else {
        println!("\n{total} ready");
    }
    Ok(())
}

/// Print `specline lint`.
///
/// Read-only, and read through the daemon for the reason `fsck` is: a report you
/// have to stop the daemon to run is one nobody runs.
pub fn lint(
    daemon: &str,
    project: &str,
    check: Option<&str>,
    limit: usize,
    json_out: bool,
) -> Result<()> {
    let mut url = format!("/api/lint?project={}&limit={limit}", urlencode(project));
    // Asked for after the limit rather than before it, because filtering here
    // would report a total for the whole project and a list for one rule, and
    // the two numbers would look like a bug.
    if check.is_some() {
        url = format!("/api/lint?project={}&limit=10000", urlencode(project));
    }

    let Some(report) = crate::writes::read(daemon, &url, std::time::Duration::from_secs(60))?
    else {
        bail!(
            "no daemon at {daemon}. `specline lint` reads through it, because the daemon is the \
             one process that has seen every write — start it with `specline-daemon`."
        );
    };

    let mut findings: Vec<&Value> = report
        .get("findings")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    if let Some(want) = check {
        findings.retain(|f| f.get("check").and_then(Value::as_str) == Some(want));
        findings.truncate(limit);
    }

    if json_out {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let scanned = report.get("scanned").and_then(Value::as_u64).unwrap_or(0);
    let total = report.get("total").and_then(Value::as_u64).unwrap_or(0);

    if total == 0 {
        println!("{scanned} row(s) scanned, nothing to report");
        return Ok(());
    }

    for finding in &findings {
        let field = |k: &str| finding.get(k).and_then(Value::as_str).unwrap_or("");
        println!("  {:<10} {}", field("reference"), field("detail"));
    }

    println!();
    for entry in report
        .get("by_check")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let name = entry.get("check").and_then(Value::as_str).unwrap_or("");
        let count = entry.get("count").and_then(Value::as_u64).unwrap_or(0);
        println!("  {count:>4}  {name}");
    }
    // What "the rest" means depends on whether a rule was asked for. Reporting
    // the project total against a filtered list would read as a missing 231
    // findings when the rule genuinely has none left, which is the difference
    // between "there is more to see" and "this one is clear".
    match check {
        Some(want) => {
            let for_rule = report
                .get("by_check")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .find(|e| e.get("check").and_then(Value::as_str) == Some(want))
                .and_then(|e| e.get("count").and_then(Value::as_u64))
                .unwrap_or(0);
            println!("\n{for_rule} {want} across {scanned} row(s), of {total} in total");
            if (findings.len() as u64) < for_rule {
                println!("{} shown — raise --limit for the rest", findings.len());
            }
        }
        None => {
            println!("\n{total} finding(s) across {scanned} row(s)");
            if (findings.len() as u64) < total {
                println!("{} shown — raise --limit for the rest", findings.len());
            }
        }
    }
    // Exits zero on purpose. These are rows that predate the rules that would
    // have refused them, so a non-zero exit would fail every build until a
    // person had worked through ninety of them by hand.
    Ok(())
}

/// Percent-encode a query value, for the two places that build one by hand.
fn urlencode(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => other
                .to_string()
                .bytes()
                .map(|b| format!("%{b:02X}"))
                .collect(),
        })
        .collect()
}

/// Print `specline claim`.
pub fn claim(
    home: &Path,
    daemon: &str,
    task: &str,
    force: bool,
    session: Option<&str>,
    json_out: bool,
) -> Result<()> {
    let session = require_session(session)?;
    let mut args = json!({
        "id": task,
        "session_id": session,
        "surface": "cli",
    });
    if force {
        args["force"] = json!(true);
    }

    let structured = run_write(home, daemon, "specline_claim", &args)?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&structured)?);
        return Ok(());
    }
    let reference = structured
        .get("reference")
        .and_then(Value::as_str)
        .unwrap_or(task);
    let title = structured
        .pointer("/task/title")
        .and_then(Value::as_str)
        .unwrap_or("");
    println!("{reference} claimed — {title}");
    if let Some(previous) = structured.get("took_over_from").and_then(Value::as_str) {
        println!("  taken over from session {previous}, whose claim had gone stale");
    }
    Ok(())
}

/// Print `specline close`.
#[allow(clippy::too_many_arguments)]
pub fn close(
    home: &Path,
    daemon: &str,
    task: &str,
    reason: &str,
    message: &str,
    evidence: &[String],
    other: Option<&str>,
    session: Option<&str>,
    json_out: bool,
) -> Result<()> {
    // Parsed here rather than left to the daemon, so a typo costs no round trip
    // and the error names the five values.
    let reason = CloseReason::parse(reason).map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut args = json!({
        "id": task,
        "reason": reason.as_str(),
        "message": message,
        "surface": "cli",
    });
    if !evidence.is_empty() {
        args["evidence"] = json!(evidence);
    }
    if let Some(other) = other {
        args["other"] = json!(other);
    }
    if let Some(s) = session {
        args["session_id"] = json!(s);
    }

    let structured = run_write(home, daemon, "specline_close", &args)?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&structured)?);
        return Ok(());
    }
    // A signal has no `KEEL-42` and nothing called a title, so it reports what
    // it *became* rather than what it was — which is the interesting half, and
    // the reason closing one is worth a different sentence (B-94).
    if let Some(summary) = structured
        .pointer("/signal/summary")
        .and_then(Value::as_str)
    {
        println!(
            "{} — {summary}",
            match reason {
                CloseReason::Done => "picked up",
                CloseReason::Duplicate => "already asked for",
                _ => "set down",
            }
        );
    } else {
        let reference = structured
            .get("reference")
            .and_then(Value::as_str)
            .unwrap_or(task);
        println!("{reference} closed as {reason}");
    }
    if let Some(to) = structured.pointer("/linked/to").and_then(Value::as_str) {
        let rel = structured
            .pointer("/linked/rel")
            .and_then(Value::as_str)
            .unwrap_or("linked");
        println!("  {rel} {to}");
    }
    Ok(())
}

/// A claim has to name a session, and Specline never invents one.
fn require_session(session: Option<&str>) -> Result<String> {
    match session.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => Ok(s.to_owned()),
        None => bail!(
            "a claim has to name the session doing the work, and none was given.\n\n\
             Pass `--session <id>` or set `SPECLINE_SESSION`. Specline never invents one: a claim \
             without it would say the task is taken and not by whom, which is worse than \
             leaving it unclaimed."
        ),
    }
}

/// Run a write through the daemon, falling back to the store when none is up.
///
/// Attribution note: on the daemon path the write is recorded as `claude`,
/// because the MCP endpoint falls back to the transport's identity and cannot
/// see who is at the other end of it. `surface: cli` is sent so the record still
/// says where it came from, which is the part that is knowable.
fn run_write(home: &Path, daemon: &str, tool: &str, args: &Value) -> Result<Value> {
    // Before the write, not after: an older daemon accepts it and stores it in
    // the shape it knows, so there is nothing to notice afterwards.
    crate::writes::refuse_if_daemon_is_older(daemon)?;
    match call_daemon(daemon, tool, args)? {
        Some(v) => Ok(v),
        None => {
            tracing::debug!("no daemon listening, opening the store directly");
            directly(home, |mut store| {
                specline_mcp::dispatch(
                    &mut store,
                    specline_mcp::ToolCall {
                        name: tool,
                        arguments: args,
                        client: None,
                    },
                )
                // The same unwrapping the daemon path does. Without it every
                // renderer below looks for its fields one level too high and
                // finds nothing — quietly, because a missing field reads as an
                // absent value rather than as a mistake.
                .map(|v| specline_mcp::payload(&v))
                .map_err(|e| anyhow::anyhow!("{}", e.message))
            })
        }
    }
}

/// Open the store and run one dispatch against it.
///
/// Safe only because we got here by failing to reach a daemon, which is the one
/// condition under which nothing else is writing.
fn directly(home: &Path, f: impl FnOnce(Store) -> Result<Value>) -> Result<Value> {
    // Not `Store::open`, which would create one. A read that fell back because
    // no daemon answered must not leave an empty store behind (KEEL-137).
    let store = crate::open(home)?;
    f(store)
}

/// Call one MCP tool on the daemon.
///
/// `Ok(None)` means nothing is listening, which is the signal to fall back.
/// Anything else — a refusal, a validation error — is returned as an error
/// carrying what the daemon said, because that message is written to be acted
/// on rather than retried.
fn call_daemon(base: &str, tool: &str, args: &Value) -> Result<Option<Value>> {
    use specline_mcp::protocol::{
        HEADER_METHOD, HEADER_NAME, HEADER_PROTOCOL_VERSION, PROTOCOL_VERSION,
    };

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": args },
    });

    let response = match ureq::post(&format!("{base}/mcp"))
        .set(HEADER_METHOD, "tools/call")
        .set(HEADER_NAME, tool)
        .set(HEADER_PROTOCOL_VERSION, PROTOCOL_VERSION)
        .timeout(std::time::Duration::from_secs(30))
        .send_json(&body)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or(text);
            bail!("the daemon at {base} refused {tool} ({code}): {message}");
        }
        // No listener. The only case worth falling back on.
        Err(_) => return Ok(None),
    };

    let envelope: Value = response
        .into_json()
        .with_context(|| format!("read the daemon's response to {tool}"))?;

    if let Some(message) = envelope.pointer("/error/message").and_then(Value::as_str) {
        bail!("{message}");
    }

    // A tool error arrives as a successful JSON-RPC response with `isError`, so
    // it has to be looked for rather than assumed away by the HTTP status.
    let result = envelope.get("result").cloned().unwrap_or(envelope);
    if result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let text = result
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or("the daemon reported an error with no message");
        bail!("{text}");
    }

    Ok(Some(
        result
            .get("structuredContent")
            .cloned()
            .unwrap_or_else(|| result.clone()),
    ))
}
