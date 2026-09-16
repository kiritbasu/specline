//! The MCP protocol layer: tool schemas, argument decoding, response shaping.
//!
//! Split from `specline-daemon` so the tool surface can be exercised without
//! binding a port — which is what makes the snapshot tests possible, and they
//! are the API contract.
//!
//! Three modules, in the order a request meets them:
//!
//! - [`protocol`] — JSON-RPC and the 2026-07-28 stateless wire contract.
//! - [`tools`] — the thirteen tool definitions. These descriptions *are* the
//!   product; they are the only documentation an agent gets. The count is
//!   asserted in the snapshot suite, because it has been wrong here before:
//!   this line said "nine" through the tenth tool and the thirteenth.
//! - [`dispatch`] — executing a call against `specline-core`, and turning domain
//!   errors into something a model can act on.
//!
//! [`context`] builds the `specline_context` digest, which is important enough to
//! live on its own.

pub mod dispatch;
pub mod git;
pub mod image_roots;
pub mod links;
pub mod protocol;
pub mod tools;

pub use dispatch::{
    Prepared, ToolCall, dispatch, dispatch_prepared, entity_json, payload, resolve_project,
    to_rpc_error,
};
pub use protocol::{
    HeaderCheck, PROTOCOL_VERSION, Request, Response, RpcError, check_headers, codes,
};
/// The digest lives in `specline-core` now.
///
/// It was 1,000 lines of pure store logic sitting behind the MCP layer, so the
/// only way to ask for a project's digest was to speak JSON-RPC — which is
/// backwards for the one call the whole product is organised around. `specline
/// doctor` and the CLI could not reach it at all.
///
/// Re-exported rather than moved silently, because `specline_mcp::Digest` is a
/// path callers already write.
pub use specline_core::digest::{Depth, Digest};
pub use tools::{Tool, all as all_tools, discover_result, find as find_tool, list_result};
