//! Custom MCP server (Model Context Protocol).
//! Agent F implementation.
//!
//! Exposes tools for note search, academic context, and note management.
//! Routes are mounted under `/mcp` and `/mcp/sse` in the main application router.
//! Authentication via `X-API-Key` header (dev mode when no key configured).

pub mod handlers;
pub mod server;
pub mod tools;

use crate::AppState;
use axum::Router;
use std::sync::Arc;

/// Register MCP server routes onto a stateful router.
/// Routes are at `/mcp` (POST JSON-RPC) and `/mcp/sse` (POST SSE).
/// The `state` parameter is used to create the MCP server instance.
pub fn register_routes(
    router: Router<Arc<AppState>>,
    _state: Arc<AppState>,
) -> Router<Arc<AppState>> {
    let mcp = Router::new()
        .route("/mcp", axum::routing::post(handlers::mcp_handler))
        .route("/mcp/sse", axum::routing::post(handlers::mcp_sse_handler));

    router.merge(mcp)
}
