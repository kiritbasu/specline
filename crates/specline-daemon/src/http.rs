//! The HTTP surface: the MCP endpoint and the local API.

use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use specline_mcp::protocol::{
    Era, HEADER_METHOD, HEADER_NAME, HEADER_PROTOCOL_VERSION, HeaderCheck, PROTOCOL_VERSION,
    Request, Response as RpcResponse, RpcError, check_headers, codes, initialize_result,
    negotiated_version, requested_version,
};
use std::convert::Infallible;

/// The largest request body the daemon will read.
///
/// Generous, because `specline_create` carries an inline image and the tool
/// documents a 1 MB decoded ceiling — base64 inflates that by a third, and a
/// limit that refuses a legitimate screenshot would be discovered by a user
/// rather than by a test. Small enough that a runaway client cannot make the
/// daemon hold hundreds of megabytes it will never use.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The header a mutating request carries its token in.
///
/// Custom on purpose. A form post, an image tag and a stylesheet can all be
/// aimed at this daemon by a page the user did not write, and none of them can
/// set a header — so requiring one turns "any page can reach loopback" into
/// "any page can reach the reads", which is the difference between a nuisance
/// and a writer.
pub const TOKEN_HEADER: &str = "x-specline-token";

/// Refuse a mutating request that does not carry this daemon's token.
///
/// Applied as a layer over a sub-router rather than checked inside each
/// handler, so that a mutating endpoint added later is guarded by where it is
/// registered instead of by whoever adds it remembering. The failure this
/// project keeps meeting is a check that is absent while everything looks
/// healthy; a handler is the easiest place to leave one out.
async fn require_token(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let offered = request
        .headers()
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    if specline_core::token::matches(state.token(), offered) {
        return next.run(request).await;
    }

    // Say what would work. A 401 with no explanation on a local daemon reads as
    // a bug in the caller, and the two callers who will hit this — the CLI
    // against a daemon that restarted, and a page served by something other
    // than the daemon — both have a specific thing to do about it.
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": format!(
                "this request changes something, so it needs the daemon's token in the \
                 {TOKEN_HEADER} header. The CLI reads it from the token file beside the store; \
                 the interface is given it by the daemon that serves it, so a page loaded from \
                 anywhere else will not have one. A token from an earlier daemon is no longer \
                 valid — each daemon mints its own."
            ),
        })),
    )
        .into_response()
}

/// Build the router.
pub fn router(state: AppState) -> Router {
    // Everything that changes something, in one place and behind one layer.
    let guarded = Router::new()
        // Generation writes files into the user's repository, so it is a POST:
        // it is not a safe, cacheable read even though it only reads the store.
        .route("/api/generate", post(api_generate))
        // The write endpoint the interface is allowed (B-75, amending hard
        // constraint 7). It takes no body, no version and no URL: it can only
        // apply what this daemon has already fetched, checksum-verified and
        // staged itself.
        .route("/api/update/apply", post(api_update_apply))
        // Ask the daemon to look now, rather than waiting up to an hour for it
        // to look on its own (KEEL-258). Strictly less than the endpoint above:
        // it can download and stage, and it cannot promote anything into place
        // or restart anything.
        //
        // A person asking whether there is a new version is their own action,
        // the same class as agreeing to a restart, and it is not authoring —
        // which is the line hard constraint 7 actually draws.
        .route("/api/update/check", post(api_update_check))
        // The other half of `specline update`, which replaces the binaries on disk
        // from a process that does not own the daemon and so cannot restart it.
        // Same power as the endpoint above and less: it restarts into whatever
        // is at this process's own path, and cannot cause anything to be
        // downloaded or installed.
        .route("/api/update/restart", post(api_update_restart))
        // What a person does in the interface (B-78, KEEL-240). Every one of
        // these goes through specline-core's write path and is attributed to a
        // human on the `ui` surface. There is deliberately no endpoint that
        // takes a document body: that is the line the constraint draws.
        .route("/api/tasks", post(api_create_task))
        // Filing a signal. Capture, not authoring — somebody typing what they
        // or a colleague want is recording a fact about the world, which is
        // the half of hard constraint 7 the interface is allowed.
        //
        // It deliberately takes no `body`. A signal's verbatim is a document
        // revision, and "an endpoint that accepts a document revision is on
        // the wrong side of it" is the constraint's own checkable test — so
        // the box captures the sentence and a longer verbatim arrives through
        // a session, where the conversation it came from is. That is also what
        // the design wants: capture costing more than the thought did is
        // capture that does not happen.
        .route("/api/signals", post(api_create_signal))
        .route("/api/entity/{id}/notes", post(api_add_note))
        .route("/api/entity/{id}/archive", post(api_archive))
        .route("/api/tasks/{id}/close", post(api_close_task))
        .route("/api/tasks/{id}", patch(api_update_task))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_token,
        ));

    Router::new()
        // The single MCP endpoint. GET and DELETE are answered 405 rather than
        // 404: an older client may try the pre-2026-07-28 GET stream or the
        // session-terminating DELETE, and 405 tells it plainly that this
        // server does not do that, while 404 would suggest no endpoint here.
        .route(
            "/mcp",
            post(mcp_endpoint)
                .get(method_not_allowed)
                .delete(method_not_allowed),
        )
        .route("/api/health", get(health))
        .route("/api/context", get(api_context))
        .route("/api/projects", get(api_projects))
        .route("/api/search", get(api_search))
        .route("/api/ready", get(api_ready))
        .route("/api/activity", get(api_activity))
        .route("/api/changes", get(api_changes))
        .route("/api/entity/{id}", get(api_entity))
        .route("/api/entity/{id}/history", get(api_entity_history))
        .route("/api/entities", get(api_entities))
        .route("/api/inbox", get(api_inbox))
        .route("/api/notes", get(api_notes))
        .route("/api/clients", get(api_clients))
        .route("/api/document/{id}", get(api_document))
        .route("/api/graph/{id}", get(api_graph))
        .route("/api/events", get(api_events_stream))
        // Everything that mutates is in `guarded` below, behind the token.
        // Read-shaped CLI commands, served here because they cannot open the
        // store themselves while this process holds the write lock — which is
        // always (TQ-15, KEEL-57). `fsck` is the one that matters: an integrity
        // check you have to stop the thing you want to check in order to run is
        // not much of a check.
        .route("/api/blob/{id}", get(api_blob))
        .route("/api/fsck", get(api_fsck))
        .route("/api/lint", get(api_lint))
        .route("/api/status", get(api_status))
        .route("/api/render-status", get(api_render_status))
        // Cross-origin reads, for a browser somewhere other than here. Added
        // for the Tauri webview, served from `tauri://localhost`; that shell is
        // off the release path now, and the layer stays because any local
        // origin being able to *read* the store is the arrangement this was
        // given. Undoing it is a decision of its own (B-89).
        //
        // **Reads only.** Everything that mutates is in `guarded`, merged below
        // — after this layer, so it does not carry it. That began as an
        // accident of ordering and is now the intent: nothing needs
        // cross-origin writes while the only interface is the one this daemon
        // serves itself. `tests/cors.rs` asserts both halves, so moving the
        // merge is something somebody decides rather than discovers.
        //
        // Scoped to the local API either way: the MCP endpoint is not called
        // from a browser, and giving it CORS headers would only widen what a
        // hostile page can reach.
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::predicate(|origin, _| {
                    origin
                        .to_str()
                        .is_ok_and(|o| is_local_origin(&o.to_ascii_lowercase()))
                }))
                // GET is what this list is for. `POST` is here from when
                // `/api/generate` was thought to need it, and reaches nothing:
                // every POST route is in `guarded`. It is left rather than
                // removed so that the next person to look does not read a
                // GET-only list as evidence that writes were considered and
                // excluded on some other grounds — they are excluded by where
                // the merge happens, which is the comment above and the test
                // in `tests/cors.rs`.
                //
                // Adding a verb here does not make a write reachable. That was
                // the whole of KEEL-309: a session added `PATCH` believing it
                // did, and the test written to prove it showed `POST` was not
                // reaching the list either.
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
                .allow_headers(tower_http::cors::Any),
        )
        // The read surface, compiled in. Last, as a fallback, so it can only
        // ever answer paths no API route claimed — a new `/api/...` route
        // cannot be shadowed by it, and a typo'd one still 404s as an API call
        // rather than silently returning the app shell.
        //
        // Outside the CORS layer above on purpose. The page is served from the
        // same origin it calls, so it needs no CORS headers of its own, and
        // attaching them to HTML would only widen what another page can read.
        .fallback(crate::site::serve)
        // The mutating routes, already wearing their own layer. Merged rather
        // than chained so that the guard covers exactly them and cannot be
        // widened or narrowed by where a later route happens to be added.
        //
        // Merged *here*, below the CORS layer, so they do not carry it — see
        // that layer's comment. Moving this line up is what would make writes
        // reachable from another origin, and `tests/cors.rs` fails if it does.
        .merge(guarded)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        // A cap on how much the daemon will read, and a handler that explains
        // it in the shape the caller is speaking.
        //
        // Axum's own answer to an oversized body is a bare 413 with no body at
        // all. An MCP client parses that as a broken server rather than as a
        // request it should make smaller — so the one error a caller could
        // actually act on was the one that arrived unreadable.
        .layer(axum::middleware::from_fn(explain_body_limit))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

/// Turn a bare 413 into a JSON-RPC error that names the limit.
///
/// A middleware rather than a change at each handler, because the rejection
/// happens in the extractor — before any handler runs — so there is nowhere
/// else to catch it.
async fn explain_body_limit(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let response = next.run(request).await;
    if response.status() != StatusCode::PAYLOAD_TOO_LARGE {
        return response;
    }

    (
        StatusCode::PAYLOAD_TOO_LARGE,
        Json(json!({
            "jsonrpc": "2.0",
            "error": {
                "code": codes::INVALID_REQUEST,
                "message": format!(
                    "the request body is larger than {} bytes, which is this daemon's limit. \
                     An inline image is the usual cause: pass `image_path` instead, so the \
                     daemon reads the file itself and the bytes never travel as base64.",
                    MAX_BODY_BYTES
                )
            }
        })),
    )
        .into_response()
}

/// Serve a JSON-RPC response with the right status.
fn rpc(id: Value, result: Result<Value, RpcError>, era: Era) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(RpcResponse::ok(id, value, era))).into_response(),
        Err(err) => {
            let status = StatusCode::from_u16(err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(RpcResponse::err(id, err))).into_response()
        }
    }
}

/// GET or DELETE on the MCP endpoint.
async fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(json!({
            "jsonrpc": "2.0",
            "error": {
                "code": codes::METHOD_NOT_FOUND,
                "message": format!(
                    "This endpoint uses POST only. {PROTOCOL_VERSION} removed the GET stream \
                     and the DELETE session teardown along with protocol-level sessions; a \
                     2025-11-25 client should treat this as a server with no server-initiated \
                     messages and carry on."
                )
            }
        })),
    )
        .into_response()
}

/// Whether an `Origin` header is acceptable.
///
/// The transport makes this a MUST, and the reason is specific: a local server
/// is reachable from any web page the user has open, so a DNS-rebinding attack
/// can drive it from a hostile origin. Only same-origin loopback is allowed —
/// a browser page on the public internet has no business here.
fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        // Absent is fine: a non-browser client (which is every MCP client)
        // does not send one.
        return true;
    };
    let normalised = origin.trim().to_ascii_lowercase();
    // `null` used to be allowed, and it is the one value that must not be. A
    // browser sends `Origin: null` from a sandboxed iframe, a `file://` page
    // and a redirected cross-origin request — which is to say, from exactly
    // the contexts an attacker can arrange and a local client never uses. It
    // was the widest hole in the check, wearing the costume of an edge case.
    is_local_origin(&normalised)
}

/// Whether a lowercased origin string names this machine.
fn is_local_origin(normalised: &str) -> bool {
    // The host is compared exactly, never by prefix. `starts_with("https://
    // localhost")` accepts `https://localhost.evil.example`, which is a
    // domain an attacker can simply register — the check would then wave
    // through precisely the request it exists to stop. Caught by the test
    // below, which is why that test names the near-miss explicitly.
    let Some((scheme, rest)) = normalised.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https" | "tauri") {
        return false;
    }
    // An Origin has no path, but strip one defensively rather than trusting
    // that.
    let authority = rest.split('/').next().unwrap_or("");
    let host = match authority.rsplit_once(':') {
        // Only treat the tail as a port when it actually is one, so an IPv6
        // literal is not truncated at its last colon.
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() => h,
        _ => authority,
    };

    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

/// The MCP endpoint.
async fn mcp_endpoint(State(state): State<AppState>, headers: HeaderMap, body: String) -> Response {
    // First, before anything is spent on this request.
    if !origin_ok(&headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": codes::INVALID_REQUEST,
                    "message": "rejected by Origin check: this daemon serves local clients only"
                }
            })),
        )
            .into_response();
    }

    // After the Origin check, and that order is the point. The limiter used to
    // run first, so any web page the user had open could spend the whole
    // budget on requests that were going to be refused anyway — a denial of
    // service that costs the attacker one fetch and the user their next tool
    // call. Checking who is asking before charging them is free.
    if let Err(retry_after) = state.rate_limit.check() {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", retry_after.as_secs().to_string())],
            Json(json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": codes::INVALID_REQUEST,
                    "message": format!(
                        "rate limited: too many calls in a short window. Retry in {}s. \
                         If you are retrying a failing call, read the error rather than \
                         sending it again — the same call will fail the same way.",
                        retry_after.as_secs()
                    )
                }
            })),
        )
            .into_response();
    }

    let request: Request = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            return rpc(
                Value::Null,
                Err(RpcError::new(
                    codes::PARSE_ERROR,
                    format!("could not parse the request body as JSON-RPC: {e}"),
                )),
                Era::Modern,
            );
        }
    };

    if request.jsonrpc != "2.0" {
        return rpc(
            request.id.clone().unwrap_or(Value::Null),
            Err(RpcError::new(
                codes::INVALID_REQUEST,
                format!("`jsonrpc` must be \"2.0\", got \"{}\"", request.jsonrpc),
            )),
            Era::Modern,
        );
    }

    let header_of = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    // Read before the era, because the two answer different questions and the
    // handshake needs this one: the era says how to read the request, this says
    // which revision to claim back. Answering in a revision the caller did not
    // offer is what locked Codex out (KEEL-355).
    let asked_for = requested_version(&request, header_of(HEADER_PROTOCOL_VERSION));
    // Who is calling, so a write can say which editor made it (KEEL-360).
    // Read here because `header_of` borrows the headers and the dispatch below
    // needs the answer, and computed for every request rather than only for a
    // write: it is two `Option`s and a `split_once`, and a branch to skip that
    // would cost more to read than it saves.
    let caller = specline_mcp::protocol::client_of(&request, header_of("user-agent"));
    let era = match check_headers(
        &request,
        header_of(HEADER_METHOD),
        header_of(HEADER_NAME),
        header_of(HEADER_PROTOCOL_VERSION),
    ) {
        HeaderCheck::Ok(era) => era,
        HeaderCheck::Reject(err) => {
            return rpc(
                request.id.clone().unwrap_or(Value::Null),
                Err(err),
                Era::Modern,
            );
        }
    };

    // Notifications get 202 and no body. The current revision defines none
    // client-to-server, but 2025-11-25's `notifications/initialized` arrives
    // here and must be accepted rather than 404'd — a client that gets an
    // error for it treats the connection as failed.
    if request.is_notification() {
        return StatusCode::ACCEPTED.into_response();
    }

    let id = request.id.clone().unwrap_or(Value::Null);
    if let Some(info) = request.client_info() {
        tracing::debug!(method = %request.method, client = %info, "mcp request");
    }

    let result = match request.method.as_str() {
        // The 2025-11-25 handshake. Removed in the current revision, but this
        // is what Claude Code actually opens with.
        "initialize" => Ok(initialize_result(negotiated_version(asked_for.as_deref()))),
        // Also 2025-11-25. Cheap to answer and its absence looks like a dead
        // connection to a client that uses it as a keep-alive.
        "ping" => Ok(json!({})),
        "server/discover" => Ok(specline_mcp::discover_result()),
        "tools/list" => Ok(specline_mcp::list_result()),
        "tools/call" => {
            let Some(name) = request.tool_name() else {
                return rpc(
                    id,
                    Err(RpcError::new(
                        codes::INVALID_PARAMS,
                        "`params.name` is required for tools/call",
                    )),
                    era,
                );
            };
            // Embedding the query is model inference — the one expensive thing
            // on a read path, and the last thing that should happen while every
            // other request waits on the store. Done here, before the lock, so
            // the critical section is two SQL queries.
            let query_vector = state.embed_query(name, request.arguments());

            let mut store = state.store();
            let before = latest_event(&store);
            let outcome = specline_mcp::dispatch_prepared(
                &mut store,
                specline_mcp::ToolCall {
                    name,
                    arguments: request.arguments(),
                    client: caller.as_ref(),
                },
                query_vector,
            );
            // Announce after the lock is released, so a slow subscriber can
            // never hold the write handle.
            let after = latest_event(&store);
            drop(store);
            if let (Some(after_id), true) = (after.clone(), before != after) {
                state.announce(after_id, format!("{name} completed"));
            } else if name == "specline_note"
                && let Ok(value) = &outcome
                && !value
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                // A note writes no event row, so the check above cannot see it
                // and an open app kept showing a stale note stream with nothing
                // to say it was stale (TQ-29). Announced under its own kind so
                // a client can tell the two apart.
                let entity_id = value
                    .pointer("/structuredContent/note/entity_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                state.announce_note(entity_id, "specline_note completed");
            }
            outcome
        }
        other => Err(RpcError::new(
            codes::METHOD_NOT_FOUND,
            format!(
                "this server implements initialize, ping, server/discover, tools/list \
                 and tools/call. `{other}` is not one of them."
            ),
        )),
    };

    rpc(id, result, era)
}

/// The newest event id, used to detect that a call changed something.
fn latest_event(store: &specline_core::Store) -> Option<specline_core::EventId> {
    use specline_core::EntityStore;
    store.latest_event_id().ok().flatten()
}

// --- The local API -------------------------------------------------------
//
// Specline's own surface, not MCP. Identical in shape to what a remote daemon
// would serve, so the desktop app and any future web build are one bundle
// with a different base URL.

/// Liveness, and it must never block.
///
/// This is the probe the CLI uses to decide whether a daemon owns the store, so
/// it is asked at exactly the moment a slow write is in progress — and it used
/// to take the store lock, which meant the question could not be answered
/// precisely when it mattered. A `specline generate` holding the lock for thirty
/// seconds made health hang for thirty seconds, the CLI concluded the daemon
/// was unreachable, and it opened the store itself. The probe caused the second
/// writer it existed to prevent.
///
/// So: `try_store`, never `store`. When the lock is held the last known project
/// count is reported and `store_busy` says the number may be stale. A stale
/// count on a health page costs nothing; a health page that hangs costs the
/// constraint.
async fn health(State(state): State<AppState>) -> Json<Value> {
    // Read once: it is reported as a version and again as a link, and asking
    // the filesystem twice for one fact is how the two come to disagree.
    //
    // Null is the answer when nothing is staged *and* when the install
    // directory cannot be read; the difference does not matter to a caller, and
    // reporting an error here would put a red state on a healthy daemon for
    // something that is not about its health.
    let install_dir = specline_update::install_dir().ok();
    let staged: Option<String> = install_dir
        .as_ref()
        .and_then(|dir| specline_update::staged_version(dir).ok().flatten());

    // Whether checking is happening at all, which is a different question from
    // what it found and the one nobody could answer. `staged_version` being
    // null says "nothing is waiting", and that reads as "you are current" —
    // whether the daemon checked an hour ago, has been failing since March, or
    // has checks switched off entirely. Three states, one appearance
    // (KEEL-227).
    let last_check = install_dir
        .as_ref()
        .and_then(|dir| specline_update::last_check(dir));

    // `loaded` is read in the same borrow as the project count rather than in a
    // second `try_store`, because two looks at a busy store can disagree and
    // one health response saying two different things about one moment is worse
    // than a stale number.
    let (projects, busy, embedder_loaded) = match state.try_store() {
        Some(store) => {
            use specline_core::{EntityQuery, EntityStore, EntityType};
            let n = store
                .list(&EntityQuery::default().of_type(EntityType::Project))
                .map(|p| p.total)
                .unwrap_or(0);
            let loaded = store.embedder().is_some();
            drop(store);
            state.remember_project_count(n);
            (n, false, Some(loaded))
        }
        // Unknown rather than false. A busy store is not a store without a
        // model, and reporting the second for the first is how "semantic search
        // is off" gets believed about a daemon that is merely mid-write.
        None => (state.last_project_count().unwrap_or(0), true, None),
    };

    Json(json!({
        "status": "ok",
        "protocol": PROTOCOL_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        // The number another process should compare itself against. `version`
        // moves for reasons that have nothing to do with the tables — a CLI
        // one patch release ahead of the daemon is fine, and a CLI one
        // migration ahead is not. Reported from what this binary ships rather
        // than from the store, because it answers "what does the process
        // holding the store believe the tables look like", and after startup
        // those are the same number.
        "schema": specline_core::shipped_schema_version(),
        // Which store this daemon is holding. Answers "is the daemon that
        // replied to me holding *my* store?", which is the question every
        // write command's daemon probe needed and could not ask — see
        // `AppState::home` for the failure that produced it.
        "home": state.home().display().to_string(),
        // The oldest plugin this daemon can serve. A plugin updates over git
        // and the binary updates from a release, so the two drift; this is the
        // half of the handshake the daemon owns, and the plugin manifest
        // carries the other.
        "min_plugin_version": specline_core::MIN_PLUGIN_VERSION,
        // What is downloaded, verified and waiting, or null. The interface
        // reads health already, so an update becoming available costs no new
        // endpoint and no new capability — it is a field appearing on a
        // response the app was fetching anyway. See the binding above for what
        // null covers.
        "staged_version": staged.clone(),
        // The state of update *checking*, which the interface needs in order to
        // say "nothing is waiting" and mean it. Present from this version on,
        // so a caller seeing no `update_check` at all is talking to a daemon
        // that predates the updater — the absence is the answer, and it is the
        // population most in need of one.
        "update_check": {
            // `SPECLINE_AUTO_UPDATE=0`. A deliberate choice, and one worth showing
            // rather than silently rendering as "up to date".
            "enabled": specline_update::auto_update_enabled(),
            "last_checked_at": last_check.as_ref().map(|c| c.at.clone()),
            "last_error": last_check.as_ref().and_then(|c| c.error.clone()),
        },
        // Whether this build can do semantic search at all, which a version
        // number cannot say. Two of the three release targets cannot link the
        // ONNX runtime, so `specline 0.1.x` on Intel macOS and on arm64 are
        // different binaries with the same name (KEEL-220).
        //
        // `built_in` is a property of the build, `loaded` of this process right
        // now — a model loads in the background and takes a moment, and null
        // means the store was busy rather than that no model is there. Three
        // fields because "cannot", "could and has not yet" and "is" are three
        // different answers and only one of them is worth acting on.
        // So the interface can hide the nav item rather than showing a screen
        // whose every request 404s. Reported as a fact about this daemon, not
        // as a permission — the app has no say in it.
        "surfaces": { "inbox": state.surfaces.inbox },
        "embeddings": {
            "built_in": crate::EMBEDDINGS_BUILT_IN,
            "loaded": embedder_loaded,
        },
        // Which binary this is, not only what version it claims. KB had two
        // `specline` installs and the one on his PATH was not the one he had
        // updated, so "0.1.0" was true of the process and misleading about the
        // machine (KEEL-221, and again in KEEL-227).
        "executable": std::env::current_exe()
            .ok()
            .map(|p| p.display().to_string()),
        // Where to read what these versions contain. Minted here rather than
        // composed by the interface: the repository is configurable and a
        // template in the frontend would be right only for the default. Two
        // fields rather than one because the two answer different questions —
        // "what am I running" and "what would I be taking".
        "release_notes": specline_update::release_notes_url(env!("CARGO_PKG_VERSION")),
        "staged_release_notes": staged
            .as_deref()
            .map(specline_update::release_notes_url),
        "projects": projects,
        "store_busy": busy,
    }))
}

/// Look for a new release now, and stage it if it is safe to apply.
///
/// The same call the scheduled task makes, on a person's say-so instead of a
/// timer. It exists because there was no way to ask: KB published 0.1.5, opened
/// the interface and saw nothing, and the reason was that the last automatic
/// check had run twenty-four minutes before the release existed (KEEL-258).
///
/// **Refused when `SPECLINE_AUTO_UPDATE=0`.** `specline doctor` tells that person "off
/// — Specline makes no network requests at all", and a button that fires one anyway
/// would make a statement we print false. Somebody who has switched automation
/// off and wants a one-off look has `specline update` in a terminal, which is them
/// making the request rather than the daemon making it for them.
///
/// The outcome is named rather than described, so the interface can render each
/// case without parsing prose: `up_to_date`, `staged`, `needs_a_person`,
/// `ahead`. The last is not exotic — anybody running an `-rc` is ahead of what
/// `releases/latest` resolves to, and reporting that as "no update" would be
/// true and useless.
async fn api_update_check(State(state): State<AppState>) -> Response {
    if !specline_update::auto_update_enabled() {
        return bad_request(
            "update checks are switched off for this daemon (SPECLINE_AUTO_UPDATE=0), so it will \
             not make the request. That setting is what `specline doctor` reports as \"Specline makes \
             no network requests at all\", and this endpoint honouring it is what keeps that \
             true. To look once without turning automation back on, run `specline update` — that \
             is you making the request rather than the daemon.",
        );
    }

    let dir = match specline_update::install_dir() {
        Ok(dir) => dir,
        Err(e) => return internal_error(&format!("cannot find the install directory: {e:#}")),
    };
    let target = match specline_update::target() {
        Ok(t) => t,
        Err(e) => return internal_error(&format!("cannot tell what platform this is: {e:#}")),
    };

    // Blocking: an HTTP fetch, a hash over 11 MB and `tar`. On a runtime worker
    // that would stall every request the daemon is meant to be answering, and
    // the interface stays live while this runs precisely so a slow network
    // reads as a slow button rather than a hung page.
    let stamp_dir = dir.clone();
    let outcome =
        tokio::task::spawn_blocking(move || specline_update::check_and_stage(&dir, target)).await;

    // Stamped whichever way it went, the same as the scheduled task, so "when did
    // this last check" has one answer regardless of who asked (KEEL-227).
    let failure = match &outcome {
        Ok(Ok(_)) => None,
        Ok(Err(e)) => Some(format!("{e:#}")),
        Err(e) => Some(e.to_string()),
    };
    if let Err(e) = specline_update::record_check(&stamp_dir, failure) {
        tracing::debug!("could not record the update check: {e:#}");
    }

    match outcome {
        Ok(Ok(specline_update::Plan::UpToDate)) => Json(json!({
            "outcome": "up_to_date",
            "version": env!("CARGO_PKG_VERSION"),
        }))
        .into_response(),
        Ok(Ok(specline_update::Plan::Ahead { published })) => Json(json!({
            "outcome": "ahead",
            "version": env!("CARGO_PKG_VERSION"),
            "published": published,
        }))
        .into_response(),
        Ok(Ok(specline_update::Plan::Apply { version, .. })) => {
            tracing::info!(%version, "staged {version} on request");
            // Announced as well as answered. The window that pressed the button
            // learns from the response, but a second one open on the same
            // daemon would not — and "which window asked" is not a distinction
            // an update waiting to be taken should depend on.
            state.announce_update(&version);
            Json(json!({
                "outcome": "staged",
                "version": version,
                "release_notes": specline_update::release_notes_url(&version),
            }))
            .into_response()
        }
        // Pressing the button with something already staged. Its own outcome
        // rather than `staged`, which would claim this check found it, and
        // certainly not `up_to_date`, which would say the opposite of the truth
        // while an update sat on disk.
        Ok(Ok(specline_update::Plan::AlreadyStaged { version })) => Json(json!({
            "outcome": "already_staged",
            "version": version,
            "release_notes": specline_update::release_notes_url(&version),
        }))
        .into_response(),
        Ok(Ok(specline_update::Plan::NeedsAPerson { version, from, to })) => Json(json!({
            "outcome": "needs_a_person",
            "version": version,
            "schema_from": from,
            "schema_to": to,
            "release_notes": specline_update::release_notes_url(&version),
        }))
        .into_response(),
        // A failed check is ordinary — a laptop that just woke, no network, a
        // release without a manifest. Reported rather than raised, and with the
        // reason attached, because "it did not work" without a cause is what
        // sends somebody to the logs.
        Ok(Err(e)) => {
            Json(json!({ "outcome": "failed", "error": format!("{e:#}") })).into_response()
        }
        Err(e) => internal_error(&format!("the update check did not run: {e}")),
    }
}

/// Apply the staged update and restart into it.
///
/// The endpoint B-75 permits, and the whole of what it permits. There is no
/// body to parse because there is no choice to make: either something is staged
/// or nothing is, and what was staged was chosen by this daemon.
///
/// The response goes out *before* the restart. `exec` replaces the process
/// immediately, so re-execing inline would drop the connection and the caller
/// would see a network error for something that worked — which is exactly the
/// "failure that looks like something else" this project keeps meeting. A short
/// delay lets the response flush first.
async fn api_update_apply() -> Response {
    let dir = match specline_update::install_dir() {
        Ok(dir) => dir,
        Err(e) => return internal_error(&format!("cannot find the install directory: {e:#}")),
    };

    match specline_update::apply_staged(&dir) {
        Ok(None) => bad_request(
            "nothing is staged, so there is nothing to apply. The daemon stages an update only \
             after it has downloaded and verified one.",
        ),
        Ok(Some(version)) => {
            let restarting_into = version.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                tracing::info!(
                    version = %restarting_into,
                    "restarting into the update that was just agreed to"
                );
                crate::run::reexec();
            });
            Json(json!({ "applied": version, "restarting": true })).into_response()
        }
        Err(e) => internal_error(&format!("the staged update was not applied: {e:#}")),
    }
}

/// Restart into the binary now at this process's own path.
///
/// `specline update` replaces both binaries on disk and then has a problem: the
/// daemon is a different process it does not own, still running the code it
/// loaded at startup, and there is nothing supervising it that would bring it
/// back. Until this existed the CLI's only move was to print "restart the
/// daemon" and leave, which is a chore handed over without the means to do it.
///
/// It installs nothing and fetches nothing — every byte it might run is already
/// on disk, put there by whoever ran the update. That makes it strictly less
/// than `/api/update/apply`, which can promote something this daemon staged.
///
/// The version it was running goes back in the response, and the caller is
/// expected to ask `/api/health` afterwards for the version that came back. The
/// two disagreeing is worth knowing about: it means the daemon's binary is not
/// the one the update replaced, which happens when it was started from a
/// different directory than the `specline` doing the updating.
///
/// The response goes out *before* the restart, for the reason
/// [`api_update_apply`] gives: `exec` would otherwise drop the connection and
/// the caller would see a network error for something that worked.
async fn api_update_restart() -> Response {
    let was = env!("CARGO_PKG_VERSION");
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        tracing::info!("restarting into the binary now on disk, as asked");
        crate::run::reexec();
    });
    Json(json!({ "restarting": true, "was": was })).into_response()
}

// --- What a person does in the interface ------------------------------------
//
// Hard constraint 7, as rewritten by B-78: the interface writes what a person
// *does* — create a task, comment on one, archive a row, close one — and never
// what a person *reasons*. There is no endpoint here that takes a document
// body, and that absence is the constraint rather than an oversight.
//
// Every one of these is behind the token, so the daemon knows the caller is a
// page it served rather than any page the browser has open. That is what makes
// the attribution below honest.

/// Why the Inbox can be switched off, and what that does and does not hide.
///
/// Off by default (KEEL-341). v0.4.0 shipped filing, the nav item and the
/// digest count without triage, so signals could go in and not come out; the
/// flag hides every surface this phase added until the lifecycle is finished.
///
/// **It hides surfaces and never data.** Nothing is archived, nothing is
/// deleted, and the signals already in a store reappear intact when it goes
/// on. `specline_create(type: "feedback")` is deliberately *not* gated —
/// feedback predates this phase and two rows were written by earlier sessions.
/// The refusal, which has to say how to switch it on.
///
/// 404 rather than 403: as far as a caller is concerned the route does not
/// exist in this configuration, which is the honest answer and the one that
/// does not imply a permissions problem somebody could fix by authenticating.
fn inbox_is_off() -> Response {
    api_error(
        StatusCode::NOT_FOUND,
        codes::INVALID_PARAMS,
        "the Inbox is switched off. Set SPECLINE_INBOX=1 to switch it on — it is off by \
         default while the feature-request lifecycle is unfinished, and turning it on hides \
         nothing and loses nothing"
            .to_owned(),
    )
}

/// How a write from the interface is attributed.
///
/// `actor: human` because a person clicked something, and `surface: ui` because
/// that is where. Neither is taken from the request body: an actor a caller can
/// name is an actor a caller can lie about, and the whole reason this is
/// trustworthy is that the transport already established who is asking.
///
/// **No session id.** Hard constraint 5 says the daemon never invents one, and
/// there is no conversation behind a button — so the write is attributed to a
/// person on a surface, with the honest absence of a session rather than a
/// plausible-looking string.
fn person_at_the_interface() -> specline_core::Provenance {
    specline_core::Provenance {
        actor: specline_core::Actor::Human,
        session_id: None,
        // No client either, and for the same reason as the session: the caller
        // is a person, and `ui` already says everything a reader needs. A
        // browser's user agent would name Chrome, which is true and answers a
        // question nobody asked.
        client: None,
        surface: Some(specline_core::Surface::Ui),
    }
}

/// Create a task.
async fn api_create_task(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    use specline_core::{Entity, EntityStore, Task};

    let Some(project) = body.get("project").and_then(Value::as_str) else {
        return bad_request("`project` is required — the project id, slug or name");
    };
    let title = body
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let summary = body
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if title.trim().is_empty() {
        return bad_request("`title` is required");
    }

    let mut store = state.store();
    let project_id = match specline_mcp::resolve_project(&store, project) {
        Ok(id) => id,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, e.code, e.message),
    };

    let mut task = Task::new(project_id, title.trim(), summary.trim());
    if let Some(priority) = body.get("priority").and_then(Value::as_str) {
        match specline_core::TaskPriority::parse(priority) {
            Ok(p) => task.priority = p,
            Err(e) => return bad_request(&e.to_string()),
        }
    }
    if let Some(kind) = body.get("kind").and_then(Value::as_str) {
        match specline_core::TaskKind::parse(kind) {
            Ok(k) => task.kind = k,
            Err(e) => return bad_request(&e.to_string()),
        }
    }
    // The phase, when one was chosen. A row with no milestone is invisible in
    // every phase-scoped view, which is where somebody watching a project
    // actually looks — so this being settable at creation is the difference
    // between a task existing and a task being seen.
    if let Some(milestone) = body.get("milestone").and_then(Value::as_str)
        && !milestone.is_empty()
    {
        match specline_core::EntityId::parse_as(milestone, specline_core::EntityType::Milestone) {
            Ok(id) => task.milestone_id = Some(id),
            Err(e) => return bad_request(&format!("`milestone`: {e}")),
        }
    }
    if let Some(labels) = body.get("labels").and_then(Value::as_array) {
        task.labels = labels
            .iter()
            .filter_map(Value::as_str)
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect();
    }

    match store.create(Entity::Task(task), &person_at_the_interface()) {
        Ok(created) => Json(json!({
            "data": specline_mcp::entity_json(&created.entity),
            "created": created.created,
        }))
        .into_response(),
        Err(e) => internal_error(&format!("the task was not created: {e}")),
    }
}

/// File a signal into the Inbox.
///
/// Everything is optional except the project and the sentence, and that is the
/// requirement rather than an omission: the Inbox only works if filing costs
/// no more than typing the thought did, so there is no type picker, no
/// priority and nothing to choose. `kind` defaults to `idea`, which is what an
/// unprompted thought is; naming a `source` is what distinguishes somebody
/// else's request from your own.
///
/// No `body`. See the route table for why — the constraint's own test is that
/// an endpoint accepting a document revision is on the wrong side of the line.
async fn api_create_signal(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    use specline_core::{Entity, EntityStore, Feedback};

    if !state.surfaces.inbox {
        return inbox_is_off();
    }

    let Some(project) = body.get("project").and_then(Value::as_str) else {
        return bad_request("`project` is required — the project id, slug or name");
    };
    // `summary`, not `title`, and the refusal has to say so: feedback has no
    // title column, on the grounds that what somebody said has no name and
    // inventing one is a small lie about the record.
    let summary = body
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if summary.is_empty() {
        return bad_request("`summary` is required — what was said, in their words");
    }
    if body.get("body").is_some() {
        return bad_request(
            "`body` is not accepted here. The interface captures what was said; a longer \
             verbatim is written from the session it came from, which is where the conversation \
             is. Hard constraint 7.",
        );
    }

    let mut store = state.store();
    let project_id = match specline_mcp::resolve_project(&store, project) {
        Ok(id) => id,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, e.code, e.message),
    };

    let mut signal = Feedback::new(project_id, summary);
    // `idea`, not the table's own default of `observation`. The two are a real
    // distinction — an observation is something noticed rather than told — and
    // what arrives through the Inbox is almost always told or thought, whether
    // it is KB's own or somebody else's. The CLI's `--kind` defaults the same
    // way, so a signal filed from a terminal and one filed from the app are
    // the same row.
    signal.kind = specline_core::FeedbackKind::Idea;
    if let Some(kind) = body.get("kind").and_then(Value::as_str) {
        match specline_core::FeedbackKind::parse(kind) {
            Ok(k) => signal.kind = k,
            Err(e) => return bad_request(&e.to_string()),
        }
    }
    // Trimmed and emptied to `None`, so a field somebody tabbed through
    // becomes an absent source rather than an empty string that renders as a
    // blank attribution — which reads as "somebody said this" and is worse
    // than saying nothing.
    for (field, slot) in [
        ("source", &mut signal.source),
        ("contact", &mut signal.contact),
    ] {
        if let Some(value) = body.get(field).and_then(Value::as_str) {
            let value = value.trim();
            if !value.is_empty() {
                *slot = Some(value.to_owned());
            }
        }
    }

    match store.create(Entity::Feedback(signal), &person_at_the_interface()) {
        Ok(created) => Json(json!({
            "data": specline_mcp::entity_json(&created.entity),
            "created": created.created,
        }))
        .into_response(),
        Err(e) => internal_error(&format!("the signal was not filed: {e}")),
    }
}

/// Add a note to a row — the comment KB asked for.
async fn api_add_note(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    use specline_core::{EntityId, EntityStore, NewNote};

    let text = body.get("body").and_then(Value::as_str).unwrap_or_default();
    if text.trim().is_empty() {
        return bad_request("`body` is required — a note with nothing in it says nothing");
    }

    let entity_id = match EntityId::parse(&id) {
        Ok(id) => id,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, codes::INVALID_PARAMS, e),
    };

    let mut store = state.store();
    let note = NewNote::new(entity_id, text.trim(), specline_core::Actor::Human);
    match store.add_note(note, &person_at_the_interface()) {
        Ok(note) => Json(json!({ "data": note })).into_response(),
        Err(e) => internal_error(&format!("the note was not added: {e}")),
    }
}

/// Archive a row.
///
/// Named for what it does. The affordance may say Delete, because that is the
/// word for what somebody means, but hard constraint 3 is soft delete only —
/// the row stays readable and stays in the history, and an endpoint called
/// `delete` would be the first step towards somebody making that true.
async fn api_archive(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    use specline_core::{EntityId, EntityStore};

    let entity_id = match EntityId::parse(&id) {
        Ok(id) => id,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, codes::INVALID_PARAMS, e),
    };

    let mut store = state.store();
    // Archive whatever version is current rather than one the page was holding.
    // A stale version here would mean "somebody edited the title while you were
    // deciding", which is not a reason to refuse an archive — and the interface
    // has no way to resolve that conflict anyway.
    let current = match store.get(&entity_id) {
        Ok(Some(entity)) => entity.audit().version,
        Ok(None) => {
            return api_error(
                StatusCode::NOT_FOUND,
                codes::INVALID_PARAMS,
                format!("no artifact with id {id}"),
            );
        }
        Err(e) => return internal_error(&format!("could not read {id}: {e}")),
    };

    match store.archive(&entity_id, current, &person_at_the_interface()) {
        Ok(entity) => Json(json!({ "data": specline_mcp::entity_json(&entity) })).into_response(),
        Err(e) => internal_error(&format!("it was not archived: {e}")),
    }
}

/// Close a task, with the reason, the message and the evidence the storage
/// layer requires.
///
/// The requirement is not relaxed for the interface. A close with no reason is
/// a colour change, and the check lives under both surfaces precisely so that
/// neither can be the easy way round it — which means the form has to ask.
async fn api_close_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    use specline_core::{Close, CloseReason, EntityId, work};

    let entity_id = match EntityId::parse(&id) {
        Ok(id) => id,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, codes::INVALID_PARAMS, e),
    };

    let reason = match CloseReason::parse(body.get("reason").and_then(Value::as_str).unwrap_or(""))
    {
        Ok(reason) => reason,
        Err(e) => return bad_request(&e.to_string()),
    };
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let evidence: Vec<String> = body
        .get("evidence")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let other = match body.get("other").and_then(Value::as_str) {
        Some(raw) => match EntityId::parse(raw) {
            Ok(id) => Some(id),
            Err(e) => return api_error(StatusCode::BAD_REQUEST, codes::INVALID_PARAMS, e),
        },
        None => None,
    };

    let mut store = state.store();
    let close = Close {
        reason,
        message,
        evidence,
        other,
    };
    match work::close(&mut *store, &entity_id, &close, &person_at_the_interface()) {
        Ok(closed) => Json(json!({
            "data": specline_mcp::entity_json(&specline_core::Entity::Task(closed.task)),
        }))
        .into_response(),
        // The storage layer's refusals are the interesting ones here — no
        // reason, no message, no evidence — and they are written to be read by
        // whoever has to fix the form, so they are passed through rather than
        // flattened.
        Err(e) => api_error(StatusCode::BAD_REQUEST, codes::INVALID_PARAMS, e),
    }
}

/// Change the fields on a task that a person moves while looking at it.
///
/// Hard constraint 7 names this in as many words — moving a status or a
/// priority is a person's own action, and the interface performs it. What the
/// constraint refuses is *authoring*, and the shape of this endpoint is what
/// keeps it on the right side of that line: five named fields and nothing
/// else, so there is no argument here that could carry a document body. That
/// is B-78's own test for whether a write endpoint belongs.
///
/// Three of the five statuses are refused rather than accepted, each because
/// the transition belongs somewhere that asks for more than this does:
///
/// - `done` and `wont_do` owe a reason, a message and — for `done` — evidence.
///   `/api/tasks/{id}/close` collects them, and the storage layer refuses
///   without them on every path. Taking a bare terminal status here would only
///   produce a rejection this endpoint could not explain as well as that form
///   already does.
/// - `in_progress` is a claim, and a claim records *who*. A person at the
///   interface has no session to record, so saying so is more honest than
///   leaving the board showing work in flight against nobody — which is the
///   state `specline_claim` exists to prevent.
///
/// Moving *out* of `in_progress` clears the claim, which `close` does not do
/// and does not need to: a closed row cannot be claimed again, so a claim left
/// on it is only history. A row moved back to `todo` can, and a claim still
/// standing there would have `specline_claim` refuse it for up to three days
/// in the name of a session that walked away.
async fn api_update_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    use specline_core::{EntityId, EntityStore, EntityType, TaskStatus};

    let entity_id = match EntityId::parse_as(&id, EntityType::Task) {
        Ok(id) => id,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, codes::INVALID_PARAMS, e),
    };

    let mut store = state.store();
    let Some(specline_core::Entity::Task(current)) = (match store.get(&entity_id) {
        Ok(found) => found,
        Err(e) => return internal_error(&format!("could not read {id}: {e}")),
    }) else {
        return api_error(
            StatusCode::NOT_FOUND,
            codes::INVALID_PARAMS,
            format!("no task with id {id}"),
        );
    };

    // The version the page was holding. Unlike the archive endpoint, which
    // takes whatever is current on the grounds that a title edit is no reason
    // to refuse an archive, this one is editing the very fields a concurrent
    // write would have touched — so a conflict is real and the caller is told.
    let Some(expected) = body.get("version").and_then(Value::as_i64) else {
        return bad_request(
            "`version` is required — the version you read, so a concurrent edit is a conflict \
             rather than a silent overwrite",
        );
    };
    let Ok(expected) = i32::try_from(expected) else {
        return bad_request("`version` is not a version any row has had");
    };

    // A field present but of the wrong shape is refused rather than skipped.
    // `and_then(as_str)` on its own turns `{"status": 5}` into a silent no-op,
    // and a caller told its write succeeded when one field of it vanished will
    // build on that — which is the reasoning `store::patch` gives for rejecting
    // rather than ignoring, applied one layer out.
    macro_rules! string_field {
        ($key:expr) => {
            match body.get($key) {
                None | Some(Value::Null) => None,
                Some(Value::String(raw)) => Some(raw.as_str()),
                Some(_) => return bad_request(concat!("`", $key, "` must be a string")),
            }
        };
    }

    let mut changes = serde_json::Map::new();

    if let Some(raw) = string_field!("status") {
        let status = match TaskStatus::parse(raw) {
            Ok(status) => status,
            Err(e) => return bad_request(&e.to_string()),
        };
        match status {
            TaskStatus::Done | TaskStatus::WontDo => {
                return bad_request(
                    "a task cannot be closed here — closing owes a reason, a message and, for done, \
                     evidence. Use /api/tasks/{id}/close, which asks for them.",
                );
            }
            TaskStatus::InProgress => {
                return bad_request(
                    "starting a task is a claim, and a claim records which session is on it — \
                     which is what makes the board answer 'who is doing this' rather than only \
                     'something is'. Ask Claude to claim it, or use `specline claim`.",
                );
            }
            TaskStatus::Todo | TaskStatus::Review => {
                changes.insert("status".to_owned(), json!(status.as_str()));
                if current.status == TaskStatus::InProgress {
                    changes.insert("claimed_by".to_owned(), Value::Null);
                    changes.insert("claimed_at".to_owned(), Value::Null);
                }
            }
        }
    }

    if let Some(raw) = string_field!("priority") {
        match specline_core::TaskPriority::parse(raw) {
            Ok(priority) => changes.insert("priority".to_owned(), json!(priority.as_str())),
            Err(e) => return bad_request(&e.to_string()),
        };
    }

    if let Some(raw) = string_field!("kind") {
        match specline_core::TaskKind::parse(raw) {
            Ok(kind) => changes.insert("kind".to_owned(), json!(kind.as_str())),
            Err(e) => return bad_request(&e.to_string()),
        };
    }

    // An empty string is "no phase", which is what the select's `none` option
    // sends. Distinct from the key being absent, which means "leave it alone" —
    // and the difference matters, because clearing a milestone is a thing
    // somebody means to do.
    if let Some(raw) = string_field!("milestone") {
        if raw.is_empty() {
            changes.insert("milestone_id".to_owned(), Value::Null);
        } else {
            match EntityId::parse_as(raw, EntityType::Milestone) {
                Ok(milestone) => {
                    changes.insert("milestone_id".to_owned(), json!(milestone.to_string()))
                }
                Err(e) => return bad_request(&format!("`milestone`: {e}")),
            };
        }
    }

    // Taken as given, beyond trimming the blanks the create path also drops.
    // The fold that stops `ui` and `UI` becoming two labels lives in the
    // picker, deliberately (B-86); a second copy of it here would be a second
    // rule to keep in step.
    match body.get("labels") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            let mut labels = Vec::with_capacity(items.len());
            for item in items {
                let Some(label) = item.as_str() else {
                    return bad_request("`labels` must be an array of strings");
                };
                let label = label.trim();
                if !label.is_empty() {
                    labels.push(label.to_owned());
                }
            }
            changes.insert("labels".to_owned(), json!(labels));
        }
        Some(_) => return bad_request("`labels` must be an array of strings"),
    }

    if changes.is_empty() {
        return bad_request(
            "nothing to change — send at least one of status, priority, kind, milestone or labels",
        );
    }

    match store.update(&entity_id, expected, &changes, &person_at_the_interface()) {
        Ok(entity) => Json(json!({ "data": specline_mcp::entity_json(&entity) })).into_response(),
        // A stale version is the caller's to resolve, and it needs the current
        // state to do it — the same 409 payload SPEC §7.3 gives an agent,
        // minus the event history a form has no use for.
        Err(specline_core::Error::StaleVersion { latest, .. }) => {
            let current_state = store.get(&entity_id).ok().flatten();
            (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": {
                        "code": codes::CONFLICT,
                        "message": "this task changed while you were editing it",
                    },
                    "latest_version": latest,
                    "current_state": current_state.as_ref().map(specline_mcp::entity_json),
                })),
            )
                .into_response()
        }
        Err(e) if e.is_caller_error() => {
            api_error(StatusCode::BAD_REQUEST, codes::INVALID_PARAMS, e)
        }
        Err(e) => internal_error(&format!("the task was not changed: {e}")),
    }
}

/// Turn a tool call into an HTTP response, for the REST surface.
/// Regenerate a project's repository files from Specline.
///
/// Lives here rather than in the CLI because D-5 says non-daemon processes go
/// through this API. Generation reads the whole store and writes files from it,
/// so it wants the state the single writer has actually committed — and the
/// daemon is the only thing that can answer for that.
async fn api_generate(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    use specline_core::{Entity, EntityQuery, EntityStore, EntityType, Mode, generate};

    let reference = body
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if reference.is_empty() {
        return bad_request("`project` is required — pass a project id, slug or name");
    }
    let check = body.get("check").and_then(Value::as_bool).unwrap_or(false);

    let store = state.store();

    let projects = match store.list(&EntityQuery::default().of_type(EntityType::Project)) {
        Ok(page) => page,
        Err(e) => return internal_error(&format!("list projects: {e}")),
    };
    let needle = reference.to_lowercase();
    let Some(Entity::Project(project)) = projects.items.into_iter().find(|p| match p {
        Entity::Project(pr) => {
            pr.id.as_str() == reference
                || pr.slug.eq_ignore_ascii_case(&reference)
                || pr.name.to_lowercase() == needle
        }
        _ => false,
    }) else {
        return bad_request(&format!("no project matches `{reference}`"));
    };

    let repo_root = match body.get("repo").and_then(Value::as_str) {
        Some(path) => std::path::PathBuf::from(path),
        None => match project.root_path.as_deref() {
            Some(path) => std::path::PathBuf::from(path),
            None => {
                return bad_request(&format!(
                    "{} has no root_path recorded, so there is nowhere to write. Pass `repo`, or \
                     set root_path on the project",
                    project.slug
                ));
            }
        },
    };

    let mode = if check { Mode::Check } else { Mode::Write };

    // Decide with the store, write without it.
    //
    // This used to be one `generate::all` under the lock, and the lock covered
    // several dozen small file writes as well as every read. A generate against
    // this project's own store took long enough that the CLI's health probe
    // timed out and concluded no daemon was there — so the daemon produced the
    // second writer the probe exists to prevent.
    let plan = match generate::plan(&store, &project.id, &repo_root) {
        Ok(plan) => plan,
        Err(e) => return internal_error(&format!("plan the generate for {}: {e}", project.slug)),
    };
    let slug = project.slug.clone();
    drop(store);

    match plan.apply(mode) {
        Ok(report) => (
            StatusCode::OK,
            Json(json!({ "data": {
                "written": report.written,
                "unchanged": report.unchanged,
                "unrepresented": report.unrepresented,
                "orphans": report.orphans,
                "legacy_mirror": report.legacy_mirror,
                "checked": check,
            }})),
        )
            .into_response(),
        Err(e) => internal_error(&format!("generate {slug}: {e}")),
    }
}

/// One error shape for the whole local API.
///
/// There were three: a bare string, `{message}`, and the full `{code, message}`
/// that the MCP side returns. The desktop client reads `error.message`, so the
/// bare-string form arrived as `undefined` and the app showed "Request failed
/// (400)" — the one case where the daemon had actually explained itself.
///
/// The shape is the MCP one, because that is the one a caller may already know
/// and because the two surfaces are supposed to be the same surface.
fn api_error(status: StatusCode, code: i32, message: impl std::fmt::Display) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message.to_string() } })),
    )
        .into_response()
}

fn bad_request(message: &str) -> Response {
    api_error(StatusCode::BAD_REQUEST, codes::INVALID_PARAMS, message)
}

fn internal_error(message: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": { "code": codes::INTERNAL_ERROR, "message": message } })),
    )
        .into_response()
}

fn as_api(result: Result<Value, RpcError>) -> Response {
    match result {
        // The REST surface wants the data, not the MCP content envelope.
        Ok(value) => {
            let structured = value
                .get("structuredContent")
                .cloned()
                .unwrap_or(value.clone());
            let text = value
                .get("content")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("text"))
                .cloned()
                .unwrap_or(Value::Null);
            (
                StatusCode::OK,
                Json(json!({ "data": structured, "summary": text })),
            )
                .into_response()
        }
        Err(err) => {
            let status = StatusCode::from_u16(err.http_status())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(json!({ "error": err }))).into_response()
        }
    }
}

/// Turn a query string into tool arguments, using the tool's own schema.
///
/// This is what makes the REST surface and the MCP surface the same surface
/// rather than two that resemble each other. The previous version guessed from
/// the *value*: anything that parsed as an integer became a number, "true" and
/// "false" became booleans, everything else stayed a string. Two bugs fell out
/// of that guess, and both were live:
///
///  - **`?types=spec` was silently dropped.** The schema says `types` is an
///    array; a bare string was passed through, the tool ignored it, and the
///    search returned every type with no error at all. A filter that is ignored
///    without complaint is worse than one that fails.
///  - **`?query=404` failed with "query must be a string".** It parsed as an
///    integer, so it arrived as the number 404 and the tool rejected it. The
///    one search term guaranteed to be numeric is an HTTP status code, which is
///    exactly the sort of thing anyone would search a project for.
///
/// Reading the declared type instead of guessing fixes both, and cannot drift:
/// the schema being consulted is the one the tool advertises.
fn params_to_json(tool: &str, params: std::collections::HashMap<String, String>) -> Value {
    let schema = specline_mcp::tools::all()
        .into_iter()
        .find(|t| t.name == tool)
        .map(|t| t.input_schema);
    let properties = schema
        .as_ref()
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object);

    let mut out = serde_json::Map::new();
    for (key, raw) in params {
        let declared = properties
            .and_then(|p| p.get(&key))
            .and_then(|p| p.get("type"))
            .and_then(Value::as_str);

        let value = match declared {
            Some("array") => Value::Array(
                raw.split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(|part| Value::String(part.to_owned()))
                    .collect(),
            ),
            Some("integer") | Some("number") => raw
                .parse::<i64>()
                .map(|n| json!(n))
                .unwrap_or_else(|_| json!(raw)),
            Some("boolean") => json!(raw == "true"),
            Some("string") => json!(raw),
            // Undeclared: pass it through untouched. Guessing is what caused
            // both bugs above, and a parameter the schema does not mention is
            // the last place to start guessing.
            _ => json!(raw),
        };
        out.insert(key, value);
    }
    Value::Object(out)
}

async fn api_context(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let args = params_to_json("specline_context", params);
    let mut store = state.store();
    as_api(specline_mcp::dispatch(
        &mut store,
        specline_mcp::ToolCall {
            name: "specline_context",
            arguments: &args,
            client: None,
        },
    ))
}

async fn api_projects(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let args = params_to_json("specline_projects", params);
    let mut store = state.store();
    as_api(specline_mcp::dispatch(
        &mut store,
        specline_mcp::ToolCall {
            name: "specline_projects",
            arguments: &args,
            client: None,
        },
    ))
}

async fn api_search(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let args = params_to_json("specline_search", params);
    let mut store = state.store();
    as_api(specline_mcp::dispatch(
        &mut store,
        specline_mcp::ToolCall {
            name: "specline_search",
            arguments: &args,
            client: None,
        },
    ))
}

/// What can be worked on right now.
///
/// The same `specline_next` the CLI and a model call, reached the same way every
/// other read is: through the tool, with the query string mapped by the tool's
/// own schema. That is what makes "the app agrees with the session" a property of
/// the code rather than a thing to keep checking — there is one ranking, and all
/// three surfaces read it.
///
/// `?blocked=true` adds the ids of the tasks something live is blocking. It is
/// not a tool parameter and deliberately not one: a model asking "what can I
/// pick up" does not want the stuck list, and the board does — it draws a
/// blocked column. The board's alternative was the whole digest, which costs
/// twenty-seven kilobytes and every section of a project summary to read one
/// field, so the parameter exists to stop a view paying for a briefing (B-15:
/// the local API may have more than the tool surface does, because a UI knows
/// what it wants).
///
/// The ids come from [`specline_core::next::blocked_tasks`], which is *the*
/// definition of blocked. Recomputing it here in any other way is how the app
/// and the digest would come to disagree.
///
/// It is not free, and the cost is written down rather than left to be
/// rediscovered. Asking for blocked walks the `blocks` edges a second time: the
/// ranking inside the tool has already walked them and thrown that half away.
/// Measured over fifteen rounds against a copy of the live store, all three on
/// the same build — ranking alone 316 ms, ranking with blocked 558 ms, the
/// digest this replaces 724 ms and twenty-three times the bytes.
///
/// So it is still the cheaper call, and it is one of four the board makes in
/// parallel. The version with no second walk means either the daemon stops
/// going through the tool — and the app's ranking stops being the tool's by
/// construction — or `specline_next` starts returning a stuck list no model asked
/// for. Neither is worth 240 ms on a screen that loads once. If it ever is, the
/// fix is a `blocked` field on [`specline_core::Ready`] carrying what the ranking
/// already computed, not a second ranking here.
async fn api_ready(
    State(state): State<AppState>,
    Query(mut params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    // Taken out before the arguments are built: `specline_next` has no `blocked`
    // in its schema, and passing an undeclared parameter through to a tool is
    // how a filter gets silently ignored.
    let want_blocked = params.remove("blocked").is_some_and(|v| v == "true");
    let project = params.get("project").cloned();

    let args = params_to_json("specline_next", params);
    let mut store = state.store();
    let mut result = specline_mcp::dispatch(
        &mut store,
        specline_mcp::ToolCall {
            name: "specline_next",
            arguments: &args,
            client: None,
        },
    );

    if want_blocked && let Ok(value) = &mut result {
        let Some(slug) = project else {
            return bad_request("`blocked=true` needs `project` — blocked is per project");
        };
        let project_id = match specline_mcp::dispatch::resolve_project(&store, &slug) {
            Ok(id) => id,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
            }
        };
        match specline_core::next::blocked_tasks(&*store, &project_id) {
            Ok(ids) => {
                // Sorted so two identical stores answer identically. The set is
                // a `HashSet`, and an order that wobbles between calls would
                // make a snapshot test flap for no reason.
                let mut ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
                ids.sort();
                if let Some(obj) = value
                    .get_mut("structuredContent")
                    .and_then(Value::as_object_mut)
                {
                    obj.insert("blocked".to_owned(), json!(ids));
                }
            }
            Err(e) => return internal_error(&format!("list what is blocked in {slug}: {e}")),
        }
    }

    as_api(result)
}

/// One row's whole history — every field change, with its before and after.
///
/// Its own endpoint rather than a parameter on `/api/activity`, because
/// `/api/activity` *is* `specline_activity` and that tool no longer takes one
/// (TQ-24). B-15 is why this is not a contradiction: the local API has more
/// endpoints than the tool surface has tools, since a UI knows exactly what it
/// wants and a model chooses worse among more options.
///
/// Not paged from the feed and filtered, which is what a caller would otherwise
/// have to do: that silently misses anything older than the page, and a history
/// that quietly starts partway through is worse than no history at all.
async fn api_entity_history(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    use specline_core::EntityStore as _;

    let store = state.store();
    let entity_id = match store.resolve_ref(&id) {
        Ok(Some(id)) => id,
        Ok(None) => {
            return api_error(
                StatusCode::NOT_FOUND,
                codes::INVALID_PARAMS,
                format!("`{id}` names nothing in this store"),
            );
        }
        Err(e) => return api_error(StatusCode::BAD_REQUEST, codes::INVALID_PARAMS, e),
    };
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(500)
        .clamp(1, 5_000);
    match store.events_for(&entity_id, limit) {
        Ok(page) => (
            StatusCode::OK,
            Json(json!({ "data": {
                "events": page.items,
                "total": page.total,
                "truncated": page.truncated,
            }})),
        )
            .into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, codes::INTERNAL_ERROR, e),
    }
}

/// The bytes of a stored blob, with its own content type.
///
/// Served raw rather than base64 in JSON: this is what an `<img src>` points
/// at, and making the app decode a megabyte of JSON to show a screenshot would
/// be paying the tool-call tax twice for no reason.
async fn api_blob(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let blob_id = match specline_core::BlobId::parse(&id) {
        Ok(b) => b,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, codes::INVALID_PARAMS, e),
    };
    let store = state.store();
    match store.get_blob(&blob_id) {
        Ok(Some(blob)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, blob.media_type.clone()),
                // Content-addressed and never rewritten, so it can be cached
                // hard. A blob id names one sequence of bytes forever.
                (
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable".to_owned(),
                ),
                // A blob is bytes an agent put in the store, and the agent was
                // reading prose it did not write. Two headers stand between
                // that and script execution in whatever renders it.
                //
                // `nosniff` stops a browser deciding a blob declared
                // `image/png` is really HTML because it starts with `<`.
                // Without it the declared type is a suggestion.
                //
                // The CSP is the one that matters for SVG. An SVG is a document
                // that may contain `<script>`, and it is served with an image
                // media type — so a diagram written by a prompt-influenced
                // agent is stored cross-site scripting the moment something
                // renders it as a document rather than as an image.
                // `sandbox` with no allowances denies scripts, forms, plugins
                // and same-origin access to whatever a blob response is loaded
                // into, whatever it turns out to contain.
                (
                    header::HeaderName::from_static("x-content-type-options"),
                    "nosniff".to_owned(),
                ),
                (
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; sandbox".to_owned(),
                ),
            ],
            blob.bytes,
        )
            .into_response(),
        Ok(None) => api_error(
            StatusCode::NOT_FOUND,
            codes::INVALID_PARAMS,
            format!("no blob `{id}`"),
        ),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, codes::INTERNAL_ERROR, e),
    }
}

/// Cross-engine integrity, run inside the process that holds the lock.
/// What changed, grouped by the conversation that changed it.
///
/// Its own endpoint rather than a shape on `/api/activity`, because that URL *is*
/// the `specline_activity` tool and this is a different question: the tool answers
/// "every mutation since a cursor", paged, for a model catching up, and this
/// answers "what did each session do", for a person who left Claude working and
/// came back. B-15 is the rule — the local API has more endpoints than the tool
/// surface has tools, because a UI knows exactly what it wants.
///
/// The union with notes is the part that could not be done client-side: notes
/// leave no row in `events` (TQ-29), so a per-session count built from the feed
/// alone silently misses the part most worth reading.
async fn api_changes(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let store = state.store();

    let project_id = match params.get("project") {
        None => None,
        Some(reference) => match specline_mcp::resolve_project(&store, reference) {
            Ok(id) => Some(id),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": { "code": e.code, "message": e.message } })),
                )
                    .into_response();
            }
        },
    };

    let since = match params.get("since") {
        None => None,
        Some(raw) => match chrono::DateTime::parse_from_rfc3339(raw) {
            Ok(t) => Some(t.with_timezone(&chrono::Utc)),
            Err(_) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    codes::INVALID_PARAMS,
                    specline_core::Error::Invariant {
                        operation: "read what changed".to_owned(),
                        problem: format!("`since` is not an RFC 3339 timestamp: {raw}"),
                    },
                );
            }
        },
    };

    let actor = match params.get("actor") {
        None => None,
        Some(raw) => match specline_core::Actor::parse(raw) {
            Ok(a) => Some(a),
            Err(e) => return api_error(StatusCode::BAD_REQUEST, codes::INVALID_PARAMS, e),
        },
    };

    let query = specline_core::ChangeQuery {
        project_id,
        since,
        actor,
        limit: params
            .get("limit")
            .and_then(|l| l.parse::<usize>().ok())
            .unwrap_or(300)
            .clamp(1, 2_000),
    };

    match specline_core::changes::by_session(&store, &query) {
        Ok(log) => (
            StatusCode::OK,
            Json(json!({
                "data": {
                    "sessions": log.sessions.iter().map(|s| json!({
                        "session_id": s.session_id,
                        "actor": s.actor.as_str(),
                        "started_at": s.started_at,
                        "ended_at": s.ended_at,
                        "headline": s.headline,
                        "projects": s.projects,
                        "changes": s.changes.iter().map(|c| json!({
                            "id": c.id,
                            "kind": c.kind.as_str(),
                            "entity_id": c.entity_id.to_string(),
                            "entity_type": c.entity_type.as_str(),
                            "reference": c.reference,
                            "summary": c.summary,
                            "at": c.at,
                        })).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                    "changes": log.changes,
                    "truncated": log.truncated,
                }
            })),
        )
            .into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, codes::INTERNAL_ERROR, e),
    }
}

/// Which rows a reader would struggle with.
///
/// Served here for the same reason `fsck` is: the CLI cannot open the store while
/// this process holds the write lock, and a report you have to stop the daemon to
/// read is one nobody runs.
///
/// Not an MCP tool, deliberately. Thirteen is the ceiling and this is
/// housekeeping a person works through — a model handed a list of ninety rows to
/// improve would improve them by inventing prose, which is the failure the rule
/// exists to prevent.
async fn api_lint(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let store = state.store();
    let Some(reference) = params.get("project") else {
        return api_error(
            StatusCode::BAD_REQUEST,
            codes::INVALID_PARAMS,
            specline_core::Error::Invariant {
                operation: "lint a project".to_owned(),
                problem: "no `project` given, and lint reports on one project at a time".to_owned(),
            },
        );
    };
    let project = match specline_mcp::resolve_project(&store, reference) {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": { "code": e.code, "message": e.message } })),
            )
                .into_response();
        }
    };
    let limit = params.get("limit").and_then(|l| l.parse::<usize>().ok());
    match specline_core::lint(&store, &project, limit) {
        Ok(report) => (
            StatusCode::OK,
            Json(json!({
                "data": {
                    "findings": report.findings.iter().map(|f| json!({
                        "check": f.check,
                        "id": f.id.to_string(),
                        "reference": f.reference,
                        "detail": f.detail,
                    })).collect::<Vec<_>>(),
                    "by_check": report.by_check().iter()
                        .map(|(c, n)| json!({ "check": c, "count": n }))
                        .collect::<Vec<_>>(),
                    "scanned": report.scanned,
                    "total": report.total,
                    "truncated": report.truncated,
                }
            })),
        )
            .into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, codes::INTERNAL_ERROR, e),
    }
}

async fn api_fsck(State(state): State<AppState>) -> Response {
    let store = state.store();
    match specline_core::fsck::check(&store) {
        Ok(report) => (StatusCode::OK, Json(json!({ "data": report }))).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, codes::INTERNAL_ERROR, e),
    }
}

/// A one-line summary of what is in the store.
async fn api_status(State(state): State<AppState>) -> Response {
    use specline_core::{EntityQuery, EntityStore, EntityType};
    let store = state.store();
    let counts = (|| -> specline_core::Result<Value> {
        let projects = store.list(&EntityQuery::default().of_type(EntityType::Project))?;
        let tasks = store.list(
            &EntityQuery::default()
                .of_type(EntityType::Task)
                .with_status(["todo", "in_progress", "review"]),
        )?;
        let questions = store.list(
            &EntityQuery::default()
                .of_type(EntityType::Question)
                .with_status(["open"]),
        )?;
        Ok(json!({
            "projects": projects.total,
            "open_tasks": tasks.total,
            "open_questions": questions.total,
        }))
    })();
    match counts {
        Ok(v) => (StatusCode::OK, Json(json!({ "data": v }))).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, codes::INTERNAL_ERROR, e),
    }
}

/// The tracker as markdown, rendered from the task rows.
///
/// Returns the text rather than writing a file: where it goes is the caller's
/// business, and the daemon has no idea which repository the caller is standing
/// in. `POST /api/generate` is the one that writes.
async fn api_render_status(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    use specline_core::EntityStore as _;

    let Some(project) = params.get("project") else {
        return api_error(
            StatusCode::BAD_REQUEST,
            codes::INVALID_PARAMS,
            "`project` is required: a tracker belongs to one project",
        );
    };
    use specline_core::{EntityQuery, EntityType};

    let store = state.store();
    // Matched by slug, key or name, the same three a person would type. The
    // CLI resolves the same way; a project the CLI can name and the daemon
    // cannot would be a difference nobody could explain.
    let needle = project.to_lowercase();
    let found = match store.list(&EntityQuery::default().of_type(EntityType::Project)) {
        Ok(page) => page.items.into_iter().find(|e| match e {
            specline_core::Entity::Project(p) => {
                p.slug.to_lowercase() == needle
                    || p.key.to_lowercase() == needle
                    || p.name.to_lowercase() == needle
            }
            _ => false,
        }),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, codes::INTERNAL_ERROR, e),
    };
    let Some(found) = found else {
        return api_error(
            StatusCode::NOT_FOUND,
            codes::INVALID_PARAMS,
            format!("no project named `{project}`"),
        );
    };
    match specline_core::render_status::render(&store, found.id()) {
        Ok(markdown) => (
            StatusCode::OK,
            Json(json!({ "data": { "markdown": markdown } })),
        )
            .into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, codes::INTERNAL_ERROR, e),
    }
}

async fn api_activity(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let args = params_to_json("specline_activity", params);
    let mut store = state.store();
    as_api(specline_mcp::dispatch(
        &mut store,
        specline_mcp::ToolCall {
            name: "specline_activity",
            arguments: &args,
            client: None,
        },
    ))
}

/// Resolve a path parameter that may be a ULID or a readable reference.
///
/// The app puts `KEEL-42` in its URLs, because that is what a person copies out
/// of a conversation and pastes into the address bar. A 400 distinguishes "that
/// is not a reference" from a 404's "no such thing".
// The error variant is a whole `Response`, which clippy would rather was boxed.
// It is not: this is the one-per-request path, the alternative is an allocation
// on the failure branch of a handler that is about to allocate a JSON body
// anyway, and boxing it would put a `*` at every call site for nothing.
#[allow(clippy::result_large_err)]
fn resolve_path_id(
    store: &specline_core::Store,
    raw: &str,
) -> std::result::Result<specline_core::EntityId, Response> {
    use specline_core::EntityStore;
    match store.resolve_ref(raw) {
        Ok(Some(id)) => Ok(id),
        Ok(None) => Err(api_error(
            StatusCode::NOT_FOUND,
            codes::INVALID_PARAMS,
            format!("`{raw}` does not name anything"),
        )),
        Err(e) => Err(api_error(
            StatusCode::BAD_REQUEST,
            codes::INVALID_PARAMS,
            e.to_string(),
        )),
    }
}

async fn api_entity(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let mut args = params_to_json("specline_get", params);
    if let Some(obj) = args.as_object_mut() {
        obj.insert("ids".to_owned(), json!([id]));
    }
    let mut store = state.store();
    as_api(specline_mcp::dispatch(
        &mut store,
        specline_mcp::ToolCall {
            name: "specline_get",
            arguments: &args,
            client: None,
        },
    ))
}

/// Which editor drove a conversation (KEEL-361).
///
/// Two questions off one table, because they are the same question read in two
/// directions. `?session_id=` resolves one row's origin — every task, note and
/// revision already carries the session that wrote it, so this is the join that
/// turns that id into "Codex 0.148". No argument lists the sessions that have
/// written, most recently first, which is "what is talking to Specline".
///
/// **`last_wrote`, not `connected`, and the field name is the point.** MCP over
/// HTTP is stateless: there is no connection to report, so a caller rendering a
/// green light would be inventing one — wrong for an editor quit an hour ago,
/// and wrong again for one sitting idle mid-conversation. A name for this field
/// that implied liveness would be a lie told once in the daemon and repeated by
/// every surface that read it.
///
/// A session with no row is absent rather than defaulted. Three different
/// things land there — a conversation older than the table, a transport that
/// named no client, and one that never wrote — and all three are honestly
/// unknown.
async fn api_clients(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    use specline_core::EntityStore;

    let store = state.store();
    let found = match params.get("session_id") {
        Some(session_id) => store
            .client_for_session(session_id)
            .map(|one| one.into_iter().collect::<Vec<_>>()),
        None => {
            // Ordered by last write, so a limit keeps what somebody is most
            // likely to be looking at rather than an arbitrary slice.
            let limit = params
                .get("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(200)
                .clamp(1, 1000);
            store.session_clients(limit)
        }
    };

    match found {
        Ok(clients) => {
            let rows: Vec<Value> = clients
                .iter()
                .map(|c| {
                    json!({
                        "session_id": c.session_id,
                        "name": c.client.name,
                        "title": c.client.title,
                        "version": c.client.version,
                        "display_name": c.client.display_name(),
                        "first_seen": c.first_seen,
                        "last_wrote": c.last_seen,
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "data": { "clients": rows, "total": rows.len() } })),
            )
                .into_response()
        }
        Err(e) => internal_error(&e.to_string()),
    }
}

/// A row's running commentary.
///
/// Its own endpoint rather than a field on `/api/entities`: a board renders
/// seventy cards and wants none of the note bodies, while a detail view wants
/// one card's in full. Folding them into the list would make the common case
/// pay for the rare one.
///
/// `entity` fetches one stream; `project` fetches every live note in a project,
/// which is what a view showing several cards at once actually needs.
///
/// `?counts=true` returns `{entity_id: n}` instead of the notes themselves. The
/// board renders a hundred and twenty cards and puts a number on each one; it
/// was reading a hundred and fifty kilobytes of note prose across the wire to
/// count them and then throwing every body away. The read against the store is
/// the same either way — the saving is the transfer and the parse, which is the
/// part the browser was actually waiting on.
async fn api_notes(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    use specline_core::EntityStore;

    let counts_only = params.get("counts").is_some_and(|v| v == "true");
    let store = state.store();
    let notes = if let Some(entity) = params.get("entity") {
        match resolve_path_id(&store, entity) {
            Ok(id) => store.notes_for(&id, params.get("all").is_some_and(|v| v == "true")),
            Err(response) => return response,
        }
    } else if let Some(project) = params.get("project") {
        match specline_mcp::dispatch::resolve_project(&store, project) {
            Ok(id) => store.notes_in_project(&id),
            // `RpcError` is a wire shape, not a Display type — pass it through
            // as the structured error it already is.
            Err(e) => {
                // `RpcError` already serialises as `{code, message}` — the
                // same shape `api_error` builds — so it is passed through
                // whole rather than flattened to its message.
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
            }
        }
    } else {
        return bad_request(
            "pass `entity` for one row's notes, or `project` for all of a project's",
        );
    };

    match notes {
        Ok(notes) if counts_only => {
            let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
            for note in &notes {
                *counts.entry(note.entity_id.to_string()).or_default() += 1;
            }
            (
                StatusCode::OK,
                Json(json!({ "data": { "counts": counts, "total": notes.len() } })),
            )
                .into_response()
        }
        Ok(notes) => (
            StatusCode::OK,
            Json(json!({ "data": { "notes": notes, "total": notes.len() } })),
        )
            .into_response(),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            codes::INTERNAL_ERROR,
            e.to_string(),
        ),
    }
}

/// List entities with filters.
///
/// Part of Specline's own API, not MCP. The tool surface is capped at ten because
/// more tools makes a model choose worse (SPEC §6.1) — that reasoning does not
/// apply to a UI, which knows exactly what it wants and would otherwise have to
/// fetch everything and filter client-side.
/// The Inbox — untriaged signals, oldest first.
///
/// A read, so it sits outside the token layer with every other read. The limit
/// is generous by default because the Inbox is meant to be worked through in
/// one sitting, and the page reports the true total either way.
async fn api_inbox(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if !state.surfaces.inbox {
        return inbox_is_off();
    }
    let store = state.store();
    let Some(project) = params.get("project") else {
        return bad_request("`project` is required — the project id, slug or name");
    };
    let project_id = match specline_mcp::dispatch::resolve_project(&store, project) {
        Ok(id) => id,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    };
    let limit = params
        .get("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(200);

    match store.inbox(&project_id, limit) {
        Ok(page) => {
            let items: Vec<Value> = page.items.iter().map(specline_mcp::entity_json).collect();
            // The same envelope every other list uses, so a caller does not
            // have to learn a second shape for the one endpoint that happens
            // to be newest.
            Json(json!({
                "data": {
                    "items": items,
                    "total": page.total,
                    "truncated": page.truncated,
                }
            }))
            .into_response()
        }
        Err(e) => internal_error(&format!("the inbox could not be read: {e}")),
    }
}

async fn api_entities(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    use specline_core::{EntityQuery, EntityStore, EntityType};

    let store = state.store();
    let mut query = EntityQuery::default();

    if let Some(project) = params.get("project") {
        match specline_mcp::dispatch::resolve_project(&store, project) {
            Ok(id) => query.project_id = Some(id),
            Err(e) => {
                // `RpcError` already serialises as `{code, message}` — the
                // same shape `api_error` builds — so it is passed through
                // whole rather than flattened to its message.
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response();
            }
        }
    }
    if let Some(types) = params.get("type") {
        let parsed: Result<Vec<EntityType>, _> = types.split(',').map(EntityType::parse).collect();
        match parsed {
            Ok(t) => query.entity_types = t,
            Err(e) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    codes::INVALID_PARAMS,
                    e.to_string(),
                );
            }
        }
    }
    if let Some(status) = params.get("status") {
        query.statuses = status.split(',').map(str::to_owned).collect();
    }
    query.include_archived = params.get("include_archived").is_some_and(|v| v == "true");
    query.limit = params.get("limit").and_then(|l| l.parse().ok());

    match store.list(&query) {
        Ok(page) => {
            // A milestone's `status` is only what was *declared*; what the
            // phase is doing is derived (B-57). The app has no task counts to
            // work it out from, so it is computed here and sent alongside.
            // Without it the roadmap shows `open` for everything.
            //
            // The counts travel with it rather than staying behind the
            // derivation. The roadmap needs them to say how far a phase has
            // got, and the alternative is the browser fetching every task in
            // the project to count them again (KEEL-332).
            //
            // Computed per project rather than only for the one that was
            // asked for. The roadmap has an all-projects mode, and scoping
            // this to `query.project_id` left every row in it with no counts —
            // which the screen would have rendered as "not scoped", a claim
            // that is false rather than merely unhelpful. There are four
            // projects in the store, so the loop is not worth avoiding.
            //
            // Only for the projects this page actually has a milestone in.
            // `api_entities` is the generic list endpoint: the board asks it
            // for two thousand tasks, and on the all-projects board with no
            // `project` in the query that was running three aggregates and a
            // thousand-row milestone read per project, for a result that could
            // never match a single task id. Deriving from the page's own
            // milestone rows makes the work proportional to what was asked
            // for, and zero when no milestone was.
            let mut wanted: Vec<specline_core::EntityId> = Vec::new();
            for e in &page.items {
                if let specline_core::Entity::Milestone(m) = e
                    && !wanted.contains(&m.project_id)
                {
                    wanted.push(m.project_id.clone());
                }
            }
            let mut states = std::collections::HashMap::new();
            for p in &wanted {
                // A project whose progress cannot be read is skipped rather
                // than failing the whole list: the rows still render, with the
                // counts absent, which is the same degradation an older daemon
                // produces and the app already handles.
                match store.milestone_progress(p) {
                    Ok(map) => states.extend(map),
                    Err(e) => {
                        tracing::warn!(project = %p, error = %e, "could not derive phase progress")
                    }
                }
            }

            let items: Vec<Value> = page
                .items
                .iter()
                .map(|e| {
                    let mut json = specline_mcp::entity_json(e);
                    if let (Some(progress), Some(map)) = (states.get(e.id()), json.as_object_mut())
                    {
                        map.insert(
                            "state".to_owned(),
                            Value::String(progress.state.as_str().to_owned()),
                        );
                        map.insert("tasks_total".to_owned(), json!(progress.tally.total));
                        map.insert("tasks_closed".to_owned(), json!(progress.tally.closed));
                        map.insert("tasks_started".to_owned(), json!(progress.tally.started));
                        map.insert(
                            "last_activity".to_owned(),
                            progress
                                .last_activity
                                .map_or(Value::Null, |t| json!(t.to_rfc3339())),
                        );
                    }
                    json
                })
                .collect();

            (
                StatusCode::OK,
                Json(json!({
                    "data": {
                        // The same shaping every other surface uses, so `version`
                        // is where a caller expects it regardless of endpoint.
                        "items": items,
                        "total": page.total,
                        "truncated": page.truncated
                    }
                })),
            )
                .into_response()
        }
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            codes::INTERNAL_ERROR,
            e.to_string(),
        ),
    }
}

/// A document's full revision history, and optionally a diff.
async fn api_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let store = state.store();
    let entity_id = match resolve_path_id(&store, &id) {
        Ok(i) => i,
        Err(response) => return response,
    };

    let history = store.revisions(&entity_id).unwrap_or_default();
    let current = params
        .get("version")
        .and_then(|v| v.parse::<i32>().ok())
        .or_else(|| history.last().map(|d| d.version));

    let body = current.and_then(|v| history.iter().find(|d| d.version == v).cloned());

    let diff = match (
        params
            .get("diff_against")
            .and_then(|v| v.parse::<i32>().ok()),
        current,
    ) {
        (Some(other), Some(v)) => store
            .diff(&entity_id, other.min(v), other.max(v))
            .ok()
            .map(|d| serde_json::to_value(d).unwrap_or(Value::Null)),
        _ => None,
    };

    (
        StatusCode::OK,
        Json(json!({
            "data": {
                "revisions": history.iter().map(|d| json!({
                    "version": d.version,
                    "title": d.title,
                    "author": d.author,
                    "session_id": d.session_id,
                    "surface": d.surface,
                    "created_at": d.created_at,
                    "status": d.status,
                })).collect::<Vec<_>>(),
                "document": body,
                "diff": diff,
            }
        })),
    )
        .into_response()
}

/// The graph around an entity.
async fn api_graph(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    use specline_core::{DEFAULT_DEPTH, Direction, GraphStore};

    let store = state.store();
    let entity_id = match resolve_path_id(&store, &id) {
        Ok(i) => i,
        Err(response) => return response,
    };
    let direction = params
        .get("direction")
        .and_then(|d| Direction::parse(d).ok())
        .unwrap_or(Direction::Both);
    let depth = params
        .get("depth")
        .and_then(|d| d.parse::<u8>().ok())
        .unwrap_or(DEFAULT_DEPTH);

    match store.neighbours(&entity_id, direction, &[], depth) {
        Ok(neighbours) => {
            let links = store.links_of(&entity_id, direction).unwrap_or_default();
            (
                StatusCode::OK,
                Json(json!({ "data": { "neighbours": neighbours, "links": links } })),
            )
                .into_response()
        }
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            codes::INTERNAL_ERROR,
            e.to_string(),
        ),
    }
}

/// Live change notifications for the desktop app.
async fn api_events_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.changes.subscribe();
    let stream = async_stream::stream! {
        let mut rx = rx;

        // Say something immediately, before waiting for a change.
        //
        // Nothing else would flow until the first write or the first keep-alive
        // fifteen seconds later, and a stream that sends no bytes is one an
        // intermediary is free to sit on: a proxy that buffers until it has a
        // body holds the *headers* too, so the browser's EventSource never
        // fires `open` and live refresh is silently dead. That is exactly what
        // happened behind the dev server's proxy.
        //
        // A comment rather than an event: `EventSource` ignores it, so no
        // client has to know this exists, and it costs one line on the wire.
        yield Ok(SseEvent::default().comment("specline"));

        loop {
            match rx.recv().await {
                Ok(change) => {
                    let data = serde_json::to_string(&change).unwrap_or_else(|_| "{}".to_owned());
                    yield Ok(SseEvent::default().event("change").data(data));
                }
                // Lagged means this subscriber fell behind and lost messages.
                // Say so rather than pretending: a UI that missed changes
                // should refetch, and silently continuing would leave it
                // showing stale state indefinitely.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    yield Ok(SseEvent::default()
                        .event("lagged")
                        .data(json!({ "missed": n }).to_string()));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn headers_with(origin: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::ORIGIN, origin.parse().unwrap());
        h
    }

    #[test]
    fn loopback_origins_are_allowed() {
        for ok in [
            "http://localhost:1420",
            "http://127.0.0.1:7654",
            "https://localhost",
            "tauri://localhost",
            "HTTP://LOCALHOST:1420",
        ] {
            assert!(origin_ok(&headers_with(ok)), "{ok} should be allowed");
        }
    }

    #[test]
    fn a_remote_origin_is_rejected() {
        // The DNS-rebinding case the transport requires this check for.
        for bad in [
            "https://evil.example",
            "http://specline.attacker.test",
            // The near-misses. Each of these defeats a prefix or substring
            // check, and each is a hostname an attacker can simply register.
            "https://localhost.evil.example",
            "http://127.0.0.1.evil.example",
            "https://notlocalhost",
            "http://evil.example/localhost",
            "https://evil.example#http://localhost",
            "file:///etc/passwd",
            "localhost",
            // The one that used to be on the allowed list. A browser sends
            // it from a sandboxed iframe, a `file://` page and a redirected
            // cross-origin request — every context an attacker can arrange,
            // and none that a real MCP client ever produces.
            "null",
        ] {
            assert!(!origin_ok(&headers_with(bad)), "{bad} should be rejected");
        }
    }

    #[test]
    fn ipv6_loopback_is_allowed_and_not_truncated_at_its_colons() {
        assert!(origin_ok(&headers_with("http://[::1]:7654")));
        assert!(origin_ok(&headers_with("http://[::1]")));
    }

    #[test]
    fn an_absent_origin_is_allowed() {
        // Every MCP client is a non-browser client and sends none.
        assert!(origin_ok(&HeaderMap::new()));
    }

    #[test]
    fn query_parameters_are_typed_on_the_way_in() {
        let mut params = std::collections::HashMap::new();
        params.insert("limit".to_owned(), "25".to_owned());
        params.insert("query".to_owned(), "onboarding".to_owned());

        let json = params_to_json("specline_search", params);
        assert_eq!(json["limit"], 25);
        assert_eq!(json["query"], "onboarding");

        // A boolean, from the tool that actually declares one. This assertion
        // used to name `include_archived` on `specline_search`, which does not
        // take it — the old value-guessing conversion turned it into a boolean
        // anyway, so the test passed while describing a parameter that was
        // being silently discarded one layer down.
        let mut params = std::collections::HashMap::new();
        params.insert("include_archived".to_owned(), "true".to_owned());
        assert_eq!(
            params_to_json("specline_projects", params)["include_archived"],
            true
        );
    }

    #[test]
    fn a_list_parameter_arrives_as_a_list() {
        // `?types=spec` used to be passed through as the string "spec", which
        // the tool ignored — so a search restricted to specs returned every
        // type, with no error. A filter that is ignored without complaint is
        // worse than one that fails.
        let mut params = std::collections::HashMap::new();
        params.insert("types".to_owned(), "spec,decision".to_owned());
        let json = params_to_json("specline_search", params);
        assert_eq!(json["types"], json!(["spec", "decision"]));
    }

    #[test]
    fn a_numeric_looking_search_term_stays_a_string() {
        // The one search term guaranteed to be numeric is an HTTP status code,
        // and `?query=404` failed with "query must be a string".
        let mut params = std::collections::HashMap::new();
        params.insert("query".to_owned(), "404".to_owned());
        let json = params_to_json("specline_search", params);
        assert_eq!(json["query"], "404");
    }

    #[test]
    fn a_number_still_arrives_as_a_number() {
        // The schema says `limit` is an integer, so it must not become "25".
        let mut params = std::collections::HashMap::new();
        params.insert("limit".to_owned(), "25".to_owned());
        assert_eq!(params_to_json("specline_search", params)["limit"], 25);
    }
}
