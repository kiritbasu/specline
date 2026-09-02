//! JSON-RPC and the 2026-07-28 stateless wire contract.
//!
//! The headline of this revision is that MCP has **no sessions**. The
//! `initialize`/`notifications/initialized` handshake is gone, `Mcp-Session-Id`
//! is gone, and every request carries its own protocol version and client
//! identity in `_meta`. That is genuinely simpler to serve — but it is also
//! why `session_id` has to be a *domain* concept supplied by the caller
//! (SPEC §6.5, D-10). There is no protocol session to borrow.
//!
//! `product/SPEC.md` §6 was written from the announcement rather than the
//! finished specification, so several things here are not in it. They are
//! recorded in `product/DECISIONS.md` under "MCP deltas"; the important ones:
//! `server/discover` is required, every result carries `resultType`, and
//! `tools/list` must return cache hints.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The current protocol revision.
pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// The previous revision, still spoken by shipping clients.
///
/// Claude Code 2.1.185 — the primary client this whole product exists to serve
/// — opens with `initialize` and declares `2025-11-25`. A server that speaks
/// only the current revision is unusable with it, which would make Phase 2's
/// gate impossible to even attempt. Supporting both is a MAY in the spec's
/// backward-compatibility section; here it is the difference between working
/// and not. See DECISIONS B-17.
pub const LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";

/// The revisions served with legacy behaviour, newest first.
///
/// All three are Streamable HTTP, which is the line that decides membership
/// rather than age. From 2025-03-26 onwards a revision is a POST to one
/// endpoint, an `initialize` handshake, no mirrored headers and no
/// `resultType` — identical to each other for the two methods Specline
/// exposes. Listing them claims that `tools/list` and `tools/call` behave the
/// same across them, which is true, and nothing more.
///
/// **2024-11-05 is deliberately absent.** It is the HTTP+SSE transport, where
/// the client opens a `GET` stream this daemon answers 405 to. Echoing it back
/// would tell such a client its transport is supported and then fail it on the
/// next request; offering it 2025-11-25 instead lets it decide, which is what
/// the counter-offer arm of [`negotiated_version`] is for.
///
/// The list exists so a client is answered with **its own** revision rather
/// than a different one. The specification's version negotiation says a server
/// that supports what was requested MUST echo it, and only otherwise offers an
/// alternative; a client handed an alternative is entitled to hang up. Codex
/// opens with 2025-06-18, and hanging up is exactly what it did (KEEL-355).
pub const LEGACY_VERSIONS: [&str; 3] = [LEGACY_PROTOCOL_VERSION, "2025-06-18", "2025-03-26"];

/// Every revision this daemon serves, newest first.
///
/// Advertised by `server/discover`, so it is a promise rather than a note.
/// `supported_versions_and_legacy_versions_agree` keeps it in step with
/// [`LEGACY_VERSIONS`].
pub const SUPPORTED_VERSIONS: [&str; 4] = [
    PROTOCOL_VERSION,
    LEGACY_PROTOCOL_VERSION,
    "2025-06-18",
    "2025-03-26",
];

/// Which revision a request names, wherever it happens to name it.
///
/// The header is authoritative when present. An `initialize` request declares
/// it in the body instead, because the header did not exist when that method
/// did. Absent everywhere means a client older than the header itself.
///
/// One function rather than two readings of the same three places: the header
/// check and the handshake answer have to agree about what was asked for, and
/// the surest way for them to disagree is to work it out separately.
pub fn requested_version(request: &Request, version_header: Option<&str>) -> Option<String> {
    version_header
        .map(str::to_owned)
        .or_else(|| {
            request
                .params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| request.declared_version())
}

/// The revision to answer `initialize` with, given what the client asked for.
///
/// Echo when the request names something served; otherwise offer the newest
/// legacy revision rather than the newest overall. That looks backwards and is
/// not: a client old enough to name an unknown revision will not be sending
/// mirrored headers, so answering 2026-07-28 would agree on a dialect it
/// cannot then speak. Offering the permissive one leaves it able to continue,
/// and a client that cannot live with the answer disconnects — which is the
/// specification's own remedy and is still better than the error this replaces.
pub fn negotiated_version(requested: Option<&str>) -> &str {
    match requested {
        Some(v) if SUPPORTED_VERSIONS.contains(&v) => v,
        _ => LEGACY_PROTOCOL_VERSION,
    }
}

/// Which revision a request belongs to.
///
/// The two differ in ways that reach the response, not just the request:
/// `Modern` requires `resultType` on every result and mirrored headers on every
/// POST; `Legacy` has an `initialize` handshake and neither of those.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Era {
    /// 2026-07-28: stateless, mirrored headers, `resultType`.
    Modern,
    /// 2025-11-25: `initialize` handshake, no mirrored headers.
    Legacy,
}

impl Era {
    /// The version string to echo back.
    pub const fn version(self) -> &'static str {
        match self {
            Era::Modern => PROTOCOL_VERSION,
            Era::Legacy => LEGACY_PROTOCOL_VERSION,
        }
    }
}

/// `_meta` key carrying the protocol version.
pub const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
/// `_meta` key carrying client identity.
pub const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
/// `_meta` key carrying server identity on results.
pub const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// Header mirroring the JSON-RPC `method`.
pub const HEADER_METHOD: &str = "mcp-method";
/// Header mirroring `params.name` or `params.uri`.
pub const HEADER_NAME: &str = "mcp-name";
/// Header carrying the protocol version.
pub const HEADER_PROTOCOL_VERSION: &str = "mcp-protocol-version";

/// The Base64 sentinel wrapping a header value that is not plain ASCII.
const B64_PREFIX: &str = "=?base64?";
/// The closing half of the sentinel.
const B64_SUFFIX: &str = "?=";

/// JSON-RPC and MCP error codes.
///
/// The MCP-specific numbers were renumbered late in the revision: the
/// `-3200{1,3,4}` values that appeared in drafts are wrong, and `-32020`
/// upwards is the range the specification reserves for itself.
pub mod codes {
    /// Malformed JSON.
    pub const PARSE_ERROR: i32 = -32700;
    /// Not a valid JSON-RPC request.
    pub const INVALID_REQUEST: i32 = -32600;
    /// Unknown method. Served with HTTP 404, which distinguishes a modern
    /// server from a legacy one that does not host the MCP endpoint at all.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Bad arguments. Also used for "resource not found", which was moved
    /// here from `-32002` to match JSON-RPC.
    pub const INVALID_PARAMS: i32 = -32602;
    /// Something failed inside the server.
    pub const INTERNAL_ERROR: i32 = -32603;
    /// Headers disagree with the body, or a required header is missing.
    pub const HEADER_MISMATCH: i32 = -32020;
    /// The client did not declare a capability the server needs.
    pub const MISSING_REQUIRED_CLIENT_CAPABILITY: i32 = -32021;
    /// The requested protocol version is not served.
    pub const UNSUPPORTED_PROTOCOL_VERSION: i32 = -32022;
    /// Specline's own: an update lost an optimistic-concurrency race. Inside the
    /// implementation-defined range, which is where a server's own errors
    /// belong.
    pub const CONFLICT: i32 = -32001;
}

/// An incoming JSON-RPC request.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Absent for a notification.
    #[serde(default)]
    pub id: Option<Value>,
    /// The method name.
    pub method: String,
    /// Method arguments.
    #[serde(default)]
    pub params: Value,
}

impl Request {
    /// Whether this is a notification rather than a request.
    ///
    /// Notifications get `202 Accepted` and no body. This revision defines no
    /// client-to-server notifications in the core protocol, so in practice
    /// this is only reached by a non-conforming client — but answering
    /// correctly costs one branch.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    /// The value `Mcp-Name` must match, if the method requires one.
    ///
    /// `params.name` for `tools/call`. The specification also defines this for
    /// `prompts/get` and `resources/read`, and both were handled here — but
    /// this server advertises neither capability and routes neither method, so
    /// the branches could only ever be reached by a client inventing a call
    /// that would then 404. Validation for a method that does not exist reads
    /// as support for it.
    pub fn expected_name(&self) -> Option<String> {
        match self.method.as_str() {
            "tools/call" => self
                .params
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            _ => None,
        }
    }

    /// The protocol version the body declares.
    pub fn declared_version(&self) -> Option<String> {
        self.params
            .get("_meta")
            .and_then(|m| m.get(META_PROTOCOL_VERSION))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// The client's self-reported identity, for logging.
    pub fn client_info(&self) -> Option<Value> {
        self.params
            .get("_meta")
            .and_then(|m| m.get(META_CLIENT_INFO))
            .cloned()
    }

    /// The `arguments` object of a `tools/call`.
    pub fn arguments(&self) -> &Value {
        self.params.get("arguments").unwrap_or(&Value::Null)
    }

    /// The tool name of a `tools/call`.
    pub fn tool_name(&self) -> Option<&str> {
        self.params.get("name").and_then(Value::as_str)
    }
}

/// A JSON-RPC error, ready to serialise.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RpcError {
    /// The numeric code.
    pub code: i32,
    /// A human- and model-readable message.
    pub message: String,
    /// Structured detail. Carries the 409 payload for [`codes::CONFLICT`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// An error with no structured data.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Attach structured detail.
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// The HTTP status this error should be served with.
    ///
    /// The mapping matters more than it looks: a client uses `400` plus a
    /// recognised modern error code to tell a 2026-07-28 server apart from a
    /// legacy one, and `404` with `-32601` to tell "no such method" apart from
    /// "no MCP endpoint here".
    pub fn http_status(&self) -> u16 {
        match self.code {
            codes::METHOD_NOT_FOUND => 404,
            codes::HEADER_MISMATCH
            | codes::UNSUPPORTED_PROTOCOL_VERSION
            | codes::MISSING_REQUIRED_CLIENT_CAPABILITY
            | codes::PARSE_ERROR
            | codes::INVALID_REQUEST
            | codes::INVALID_PARAMS => 400,
            codes::CONFLICT => 409,
            _ => 500,
        }
    }
}

/// A JSON-RPC response envelope.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    /// Always `"2.0"`.
    pub jsonrpc: &'static str,
    /// Echoes the request id.
    pub id: Value,
    /// Present on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Present on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    /// A successful response.
    ///
    /// For [`Era::Modern`] this stamps `resultType: "complete"` and the server
    /// identity — `resultType` is **required** there, and omitting it makes a
    /// conforming client treat the result as coming from an older server. A
    /// `Legacy` client predates both fields, so they are left off rather than
    /// sent as noise it has to ignore.
    pub fn ok(id: Value, mut result: Value, era: Era) -> Self {
        if era == Era::Modern
            && let Some(obj) = result.as_object_mut()
        {
            obj.entry("resultType").or_insert(json!("complete"));
            let meta = obj.entry("_meta").or_insert(json!({}));
            if let Some(m) = meta.as_object_mut() {
                m.insert(META_SERVER_INFO.to_owned(), server_info());
            }
        }
        Response {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    /// A failed response.
    pub fn err(id: Value, error: RpcError) -> Self {
        Response {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// This server's identity, returned on every result.
pub fn server_info() -> Value {
    json!({
        "name": "specline",
        "version": env!("CARGO_PKG_VERSION"),
        "title": "Specline",
    })
}

/// Decode a header value that may carry the Base64 sentinel.
///
/// Tool names and resource URIs are only *SHOULD*-constrained to header-safe
/// characters, so a client must Base64-wrap anything else — including a plain
/// ASCII value that happens to look like the sentinel. A server comparing the
/// header to the body has to decode first or it will reject valid requests.
pub fn decode_header_value(raw: &str) -> String {
    let Some(inner) = raw
        .strip_prefix(B64_PREFIX)
        .and_then(|r| r.strip_suffix(B64_SUFFIX))
    else {
        return raw.to_owned();
    };
    match base64_decode(inner) {
        Some(bytes) => String::from_utf8(bytes).unwrap_or_else(|_| raw.to_owned()),
        None => raw.to_owned(),
    }
}

/// Minimal standard-alphabet Base64 decoder.
///
/// Hand-written rather than pulled in as a dependency: this is the only
/// Base64 in the codebase, it decodes a header at most a few dozen bytes long,
/// and a decoder is easier to read than a supply-chain entry to justify.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn sextet(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }

    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let unpadded: Vec<u8> = bytes.iter().copied().take_while(|b| *b != b'=').collect();
    if unpadded.len() != bytes.iter().filter(|b| **b != b'=').count() {
        return None;
    }

    let mut out = Vec::with_capacity(unpadded.len() * 3 / 4);
    for chunk in unpadded.chunks(4) {
        let mut acc = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            acc |= sextet(*c)? << (18 - 6 * i);
        }
        let produced = match chunk.len() {
            4 => 3,
            3 => 2,
            2 => 1,
            _ => return None,
        };
        for i in 0..produced {
            out.push(((acc >> (16 - 8 * i)) & 0xff) as u8);
        }
    }
    Some(out)
}

/// The outcome of validating headers against the body.
#[derive(Debug, Clone, PartialEq)]
pub enum HeaderCheck {
    /// Everything matches. Carries the revision the request belongs to.
    Ok(Era),
    /// Reject with this error and HTTP 400.
    Reject(RpcError),
}

/// Validate the mirrored headers against the request body.
///
/// The specification is explicit about why this matters: an intermediary may
/// route on the header while the server executes on the body, and a mismatch
/// between the two is a security problem, not a formatting one.
pub fn check_headers(
    request: &Request,
    method_header: Option<&str>,
    name_header: Option<&str>,
    version_header: Option<&str>,
) -> HeaderCheck {
    let declared = requested_version(request, version_header);

    // Both revisions negotiate. TQ-11 closed legacy negotiation and the
    // consequence was immediate and total: Claude Code 2.1.185 speaks
    // 2025-11-25, so the daemon stopped serving the one client the product
    // exists for. The decision is reversed rather than softened — a store an
    // agent cannot write to is not a store, it is a filing cabinet.
    //
    // What made the removal safe to undo is that nothing was actually deleted:
    // `Era` stayed, because the response envelope branches on it regardless.
    // Only the current revision gets the strict treatment. Everything else —
    // a listed older revision, an unrecognised one, or nothing at all — is
    // served as legacy.
    //
    // The unrecognised arm used to be a refusal, and the refusal was the bug.
    // Codex opens with 2025-06-18, got `this server speaks 2026-07-28 and
    // 2025-11-25, not 2025-06-18`, retried once and gave up, so none of the
    // thirteen tools ever appeared (KEEL-355). B-17 had already fixed this
    // exact failure for Claude Code by adding one version; adding a second
    // would fix Codex and leave the next client to find it again. So the arm
    // itself goes. Version negotiation belongs in the answer — see
    // [`negotiated_version`] — not in a door that only opens for names on a
    // list.
    //
    // The fair objection is that a client can now skip the mirrored-header
    // check by naming a revision that does not require it. It could already:
    // the `None` arm has always read a request with no version at all as
    // legacy, because that is what Claude Code's first request looks like. So
    // the opt-out was one absent header away before this change and is one
    // absent header away after it — what changed is how many spellings reach
    // it, which is not a boundary anybody was defending.
    //
    // The check is a consistency guarantee between an intermediary that routes
    // on the header and a server that executes on the body, and it is worth
    // keeping for clients that opt into the strict revision. It is not an
    // access control, and nothing here is: `origin_ok` and the token layer are,
    // and neither of them moved.
    let era = match declared.as_deref() {
        Some(PROTOCOL_VERSION) => Era::Modern,
        _ => Era::Legacy,
    };

    // The mirrored headers are required only by the current revision. A legacy
    // client sends none of them, and demanding them is precisely how this
    // daemon locked out the client it exists to serve.
    if era == Era::Legacy {
        return HeaderCheck::Ok(era);
    }

    if let Some(body_version) = request.declared_version()
        && Some(body_version.as_str()) != version_header
    {
        return HeaderCheck::Reject(RpcError::new(
            codes::HEADER_MISMATCH,
            format!(
                "MCP-Protocol-Version header value `{}` does not match the body's \
                 {META_PROTOCOL_VERSION} value `{body_version}`",
                version_header.unwrap_or("(absent)")
            ),
        ));
    }

    match method_header {
        None => {
            return HeaderCheck::Reject(RpcError::new(
                codes::HEADER_MISMATCH,
                "missing required header `Mcp-Method`",
            ));
        }
        Some(m) if m != request.method => {
            return HeaderCheck::Reject(RpcError::new(
                codes::HEADER_MISMATCH,
                format!(
                    "Mcp-Method header value `{m}` does not match body method `{}`",
                    request.method
                ),
            ));
        }
        Some(_) => {}
    }

    if let Some(expected) = request.expected_name() {
        match name_header.map(decode_header_value) {
            None => {
                return HeaderCheck::Reject(RpcError::new(
                    codes::HEADER_MISMATCH,
                    format!(
                        "missing required header `Mcp-Name` — {} requires it",
                        request.method
                    ),
                ));
            }
            Some(got) if got != expected => {
                return HeaderCheck::Reject(RpcError::new(
                    codes::HEADER_MISMATCH,
                    format!("Mcp-Name header value `{got}` does not match body value `{expected}`"),
                ));
            }
            Some(_) => {}
        }
    }

    HeaderCheck::Ok(era)
}

/// What the server tells a client it is for.
///
/// One definition, used by both `initialize` and `server/discover`. It existed
/// word for word in two files, so editing one made the two ways a client can
/// ask "who are you" answer differently — and nothing would have said so.
///
/// The session-identity sentence is deliberately vaguer than the hook's: over
/// MCP alone there is no session to name, and telling a client to "mint" one is
/// what produced colliding date-based identifiers.
pub const INSTRUCTIONS: &str = "Specline stores everything about a software project except the code. Call \
     `specline_context` first to orient. Pass the `session_id` your host gave you on every call, so \
     writes are attributed to this conversation. Before creating a project, call \
     `specline_projects` and confirm with the human.";

/// The `initialize` result, in the caller's own revision.
///
/// Took an era only after the hardcoded legacy version became a live bug: a
/// 2026-07-28 client would open the handshake correctly, pass the version
/// check, and be told the server speaks 2025-11-25. Answering a handshake in a
/// dialect the caller did not offer is how a connection dies at the first
/// request with nothing useful in the log.
///
/// It now takes the version rather than the era, because those stopped being
/// the same question. An era is which dialect the *request* is read in, and
/// there are two; the version is which one the *answer* claims, and there are
/// five. Passing the era meant every legacy client was answered `2025-11-25`
/// whatever it had asked for — the same "dialect the caller did not offer"
/// failure the paragraph above describes, surviving inside its own fix.
/// [`negotiated_version`] is what to pass.
pub fn initialize_result(version: &str) -> Value {
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": server_info(),
        "instructions": INSTRUCTIONS,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn request(method: &str, params: Value) -> Request {
        Request {
            jsonrpc: "2.0".to_owned(),
            id: Some(json!(1)),
            method: method.to_owned(),
            params,
        }
    }

    fn call(tool: &str) -> Request {
        request(
            "tools/call",
            json!({
                "name": tool,
                "arguments": {},
                "_meta": { META_PROTOCOL_VERSION: PROTOCOL_VERSION }
            }),
        )
    }

    #[test]
    fn matching_headers_pass() {
        let r = call("specline_context");
        assert_eq!(
            check_headers(
                &r,
                Some("tools/call"),
                Some("specline_context"),
                Some(PROTOCOL_VERSION)
            ),
            HeaderCheck::Ok(Era::Modern)
        );
    }

    #[test]
    fn a_legacy_client_is_served_again() {
        // Claude Code 2.1.185 declares 2025-11-25 and sends none of the
        // mirrored headers. B-17 served it; TQ-11 refused it and thereby
        // locked out the only client the product exists for; this restores it.
        // The refusal is the regression, not the acceptance.
        let r = call("specline_context");
        assert_eq!(
            check_headers(&r, None, None, Some(LEGACY_PROTOCOL_VERSION)),
            HeaderCheck::Ok(Era::Legacy),
        );
    }

    #[test]
    fn an_initialize_request_is_read_from_the_body() {
        // `initialize` predates the MCP-Protocol-Version header, so the only
        // place the version appears is `params.protocolVersion`. Reading it
        // from there is what lets the handshake be answered in the caller's
        // own dialect rather than the server's favourite.
        let r = request(
            "initialize",
            json!({"protocolVersion": LEGACY_PROTOCOL_VERSION, "capabilities": {}}),
        );
        assert_eq!(
            check_headers(&r, None, None, None),
            HeaderCheck::Ok(Era::Legacy),
        );
    }

    #[test]
    fn a_missing_version_everywhere_is_treated_as_legacy() {
        // A client older than the header itself. There is nothing else it
        // could be, and refusing it bought nothing.
        let r = request("tools/list", json!({}));
        assert_eq!(
            check_headers(&r, None, None, None),
            HeaderCheck::Ok(Era::Legacy),
        );
    }

    /// This used to assert the refusal, and the refusal was the bug.
    ///
    /// It named 2024-11-05 as a revision "this server never spoke" and checked
    /// that saying so listed the alternatives. Listing alternatives in an error
    /// is not negotiation — the client has already been turned away by the time
    /// it reads them, which is exactly what Codex did (KEEL-355). The old
    /// revisions are served now, so the request goes through instead.
    #[test]
    fn an_older_revision_is_served_rather_than_turned_away() {
        let r = call("specline_context");
        assert_eq!(
            check_headers(
                &r,
                Some("tools/call"),
                Some("specline_context"),
                Some("2024-11-05"),
            ),
            HeaderCheck::Ok(Era::Legacy),
            "an older revision is read as legacy, not refused",
        );
    }

    #[test]
    fn a_header_that_disagrees_with_the_body_is_rejected() {
        // The security case: a load balancer routes on the header while the
        // server executes on the body.
        let r = call("specline_create");
        match check_headers(
            &r,
            Some("tools/call"),
            Some("specline_context"),
            Some(PROTOCOL_VERSION),
        ) {
            HeaderCheck::Reject(e) => {
                assert_eq!(e.code, codes::HEADER_MISMATCH);
                assert!(e.message.contains("specline_create"), "{}", e.message);
            }
            HeaderCheck::Ok(_) => panic!("should have been rejected"),
        }
    }

    #[test]
    fn a_method_header_that_disagrees_is_rejected() {
        let r = call("specline_context");
        match check_headers(
            &r,
            Some("tools/list"),
            Some("specline_context"),
            Some(PROTOCOL_VERSION),
        ) {
            HeaderCheck::Reject(e) => assert_eq!(e.code, codes::HEADER_MISMATCH),
            HeaderCheck::Ok(_) => panic!("should have been rejected"),
        }
    }

    #[test]
    fn a_body_version_that_disagrees_with_the_header_is_rejected() {
        let r = request(
            "tools/list",
            json!({ "_meta": { META_PROTOCOL_VERSION: "2025-11-25" } }),
        );
        match check_headers(&r, Some("tools/list"), None, Some(PROTOCOL_VERSION)) {
            HeaderCheck::Reject(e) => {
                assert_eq!(e.code, codes::HEADER_MISMATCH);
                assert!(e.message.contains("2025-11-25"), "{}", e.message);
            }
            HeaderCheck::Ok(_) => panic!("should have been rejected"),
        }
    }

    #[test]
    fn methods_without_a_name_do_not_require_the_header() {
        let r = request("tools/list", json!({}));
        assert_eq!(
            check_headers(&r, Some("tools/list"), None, Some(PROTOCOL_VERSION)),
            HeaderCheck::Ok(Era::Modern)
        );
    }

    #[test]
    fn a_base64_wrapped_name_is_decoded_before_comparison() {
        // "specline_context" base64-encoded.
        let encoded = "=?base64?c3BlY2xpbmVfY29udGV4dA==?=";
        assert_eq!(decode_header_value(encoded), "specline_context");

        let r = call("specline_context");
        assert_eq!(
            check_headers(
                &r,
                Some("tools/call"),
                Some(encoded),
                Some(PROTOCOL_VERSION)
            ),
            HeaderCheck::Ok(Era::Modern)
        );
    }

    #[test]
    fn base64_decodes_padded_and_unpadded_input() {
        assert_eq!(decode_header_value("=?base64?aGk=?="), "hi");
        assert_eq!(decode_header_value("=?base64?aGVsbG8=?="), "hello");
        assert_eq!(
            decode_header_value("=?base64?SGVsbG8sIOS4lueVjA==?="),
            "Hello, 世界"
        );
    }

    #[test]
    fn a_plain_value_passes_through_untouched() {
        assert_eq!(decode_header_value("specline_search"), "specline_search");
        // Malformed sentinel: return it verbatim rather than guessing.
        assert_eq!(decode_header_value("=?base64?!!!?="), "=?base64?!!!?=");
    }

    #[test]
    fn a_modern_result_carries_result_type_and_server_info() {
        let r = Response::ok(json!(1), json!({"content": []}), Era::Modern);
        let result = r.result.unwrap();
        assert_eq!(
            result["resultType"], "complete",
            "required in this revision; omitting it makes clients treat us as an older server"
        );
        assert_eq!(result["_meta"][META_SERVER_INFO]["name"], "specline");
    }

    #[test]
    fn a_legacy_result_carries_neither() {
        // Both fields postdate 2025-11-25. Sending them is not harmful, but a
        // response should not contain fields the client's revision cannot
        // explain.
        let r = Response::ok(json!(1), json!({"content": []}), Era::Legacy);
        let result = r.result.unwrap();
        assert!(result.get("resultType").is_none());
        assert!(result.get("_meta").is_none());
    }

    #[test]
    fn the_initialize_result_answers_in_the_callers_own_revision() {
        // Was hardcoded to the legacy version, which meant a 2026-07-28 client
        // opened the handshake correctly and was told the server speaks
        // 2025-11-25 — a connection that dies at the first request with
        // nothing useful in the log.
        let legacy = initialize_result(LEGACY_PROTOCOL_VERSION);
        assert_eq!(legacy["protocolVersion"], LEGACY_PROTOCOL_VERSION);
        assert_eq!(legacy["serverInfo"]["name"], "specline");
        assert!(legacy["capabilities"]["tools"].is_object());

        let modern = initialize_result(PROTOCOL_VERSION);
        assert_eq!(modern["protocolVersion"], PROTOCOL_VERSION);
    }

    /// Two lists that must not drift.
    ///
    /// `SUPPORTED_VERSIONS` is what `server/discover` promises; `LEGACY_VERSIONS`
    /// is what gets legacy treatment. They are written out separately because
    /// Rust cannot concatenate const arrays, which is exactly the kind of
    /// duplication that rots — a revision added to one and not the other would
    /// either be advertised and then refused, or served and never mentioned.
    #[test]
    fn supported_versions_and_legacy_versions_agree() {
        let mut expected = vec![PROTOCOL_VERSION];
        expected.extend(LEGACY_VERSIONS);
        assert_eq!(SUPPORTED_VERSIONS.to_vec(), expected);
    }

    /// The HTTP+SSE revision is not claimed.
    ///
    /// A 2024-11-05 client opens a `GET` stream, which this daemon answers 405.
    /// Echoing its version back would say the transport works and then fail it
    /// on the very next request, so it gets a counter-offer instead.
    #[test]
    fn the_http_sse_revision_is_offered_an_alternative_rather_than_echoed() {
        assert!(!SUPPORTED_VERSIONS.contains(&"2024-11-05"));
        assert_eq!(
            negotiated_version(Some("2024-11-05")),
            LEGACY_PROTOCOL_VERSION
        );
    }

    /// The half the era could not express.
    ///
    /// Passing an era answered every pre-2026 client `2025-11-25` whatever it
    /// had asked for, which is the same "dialect the caller did not offer"
    /// failure the test above exists to prevent, one revision down.
    #[test]
    fn a_served_revision_is_echoed_rather_than_replaced() {
        for asked in LEGACY_VERSIONS {
            assert_eq!(
                negotiated_version(Some(asked)),
                asked,
                "a revision this daemon serves must come back as itself"
            );
        }
        assert_eq!(negotiated_version(Some(PROTOCOL_VERSION)), PROTOCOL_VERSION);
    }

    /// An unknown revision is answered, not refused.
    ///
    /// The offer is the *permissive* revision rather than the newest, and that
    /// is deliberate: a client naming something unrecognised will not be
    /// mirroring headers, so agreeing on 2026-07-28 would settle on a dialect
    /// it cannot speak and fail on the request after this one.
    #[test]
    fn an_unknown_revision_is_offered_one_that_works() {
        for asked in [Some("2027-01-01"), Some("banana"), Some(""), None] {
            assert_eq!(
                negotiated_version(asked),
                LEGACY_PROTOCOL_VERSION,
                "asked for {asked:?}"
            );
        }
    }

    /// The request Codex actually sends, byte for byte off the wire.
    ///
    /// Captured from `codex-mcp-client/0.148.0-alpha.15` through a logging
    /// proxy in front of the daemon. It was refused with
    /// `-32022 this server speaks 2026-07-28 and 2025-11-25, not 2025-06-18`,
    /// Codex retried once and gave up, and none of the thirteen tools ever
    /// appeared (KEEL-355). Written from the capture rather than from the
    /// documentation, because the two had already disagreed once.
    #[test]
    fn the_request_codex_actually_sends_negotiates() {
        let init = request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "elicitation": { "form": {}, "url": {} } },
                "clientInfo": {
                    "name": "codex-mcp-client",
                    "title": "Codex",
                    "version": "0.148.0-alpha.15"
                }
            }),
        );

        // Codex sends none of the mirrored headers, so this must pass on the
        // body alone.
        assert_eq!(
            check_headers(&init, None, None, None),
            HeaderCheck::Ok(Era::Legacy),
            "the client must get in"
        );

        let asked = requested_version(&init, None);
        assert_eq!(asked.as_deref(), Some("2025-06-18"));
        assert_eq!(
            initialize_result(negotiated_version(asked.as_deref()))["protocolVersion"],
            "2025-06-18",
            "answering with a different revision is what it hung up over"
        );
    }

    #[test]
    fn a_legacy_client_negotiates_without_the_mirrored_headers() {
        // The exact shape of Claude Code's opening request: no version header,
        // no Mcp-Method, no Mcp-Name. Demanding them is what locked it out.
        let init = request(
            "initialize",
            json!({
                "protocolVersion": LEGACY_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "claude-code", "version": "2.1.185" }
            }),
        );
        assert_eq!(
            check_headers(&init, None, None, None),
            HeaderCheck::Ok(Era::Legacy),
            "the client this product exists to serve must connect",
        );
    }

    #[test]
    fn a_modern_client_still_has_to_mirror_its_headers() {
        // Restoring legacy must not relax the current revision: the mirrored
        // headers exist because an intermediary may route on the header while
        // the server executes on the body, and a mismatch is a security
        // problem rather than a formatting one.
        let c = call("specline_context");
        assert!(matches!(
            check_headers(&c, None, None, Some(PROTOCOL_VERSION)),
            HeaderCheck::Reject(_),
        ));
    }

    /// An unknown revision negotiates, and is answered with a real one.
    ///
    /// The inverse of this test used to stand — "an unknown revision must not
    /// negotiate" — and it is the sentence that locked Codex out. Refusing a
    /// name protects nothing: whatever the request calls itself, it still has
    /// to be a well-formed `tools/call` for a tool that exists.
    ///
    /// What is worth asserting is that the answer is honest. A client told
    /// `1999-01-01` back would have no way to know the version was never
    /// understood.
    #[test]
    fn an_unknown_revision_negotiates_and_is_answered_honestly() {
        let init = request(
            "initialize",
            json!({ "protocolVersion": "1999-01-01", "capabilities": {} }),
        );

        assert_eq!(
            check_headers(&init, None, None, None),
            HeaderCheck::Ok(Era::Legacy),
        );

        let asked = requested_version(&init, None);
        let answered = negotiated_version(asked.as_deref());
        assert_eq!(answered, LEGACY_PROTOCOL_VERSION);
        assert_ne!(
            answered, "1999-01-01",
            "echoing a revision back unread would tell the client it is understood",
        );
    }

    #[test]
    fn error_codes_map_to_the_right_http_status() {
        assert_eq!(
            RpcError::new(codes::METHOD_NOT_FOUND, "").http_status(),
            404
        );
        assert_eq!(RpcError::new(codes::HEADER_MISMATCH, "").http_status(), 400);
        assert_eq!(
            RpcError::new(codes::UNSUPPORTED_PROTOCOL_VERSION, "").http_status(),
            400
        );
        assert_eq!(RpcError::new(codes::CONFLICT, "").http_status(), 409);
        assert_eq!(RpcError::new(codes::INTERNAL_ERROR, "").http_status(), 500);
    }

    #[test]
    fn the_renumbered_codes_are_used_not_the_draft_ones() {
        // The draft values -32001/-32003/-32004 were renumbered before the
        // revision shipped. Using them would make a conforming client
        // misinterpret every error.
        assert_eq!(codes::HEADER_MISMATCH, -32020);
        assert_eq!(codes::MISSING_REQUIRED_CLIENT_CAPABILITY, -32021);
        assert_eq!(codes::UNSUPPORTED_PROTOCOL_VERSION, -32022);
    }

    #[test]
    fn a_request_without_an_id_is_a_notification() {
        let r = Request {
            jsonrpc: "2.0".to_owned(),
            id: None,
            method: "whatever".to_owned(),
            params: json!({}),
        };
        assert!(r.is_notification());
        assert!(!call("specline_get").is_notification());
    }
}
