//! MCP server core: JSON-RPC routing and session management.
//!
//! Authentication: `X-API-Key` header checked on every request.
//! Runs on 127.0.0.1 only (configured in main.rs).

use std::sync::Arc;

use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::config::Config;
use crate::AppState;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("unauthorized: missing or invalid API key")]
    Unauthorized,
    #[error("invalid JSON-RPC: {0}")]
    Parse(String),
    #[error("method not found: {0}")]
    MethodNotFound(String),
    #[error("invalid params: {0}")]
    InvalidParams(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl McpError {
    pub fn code(&self) -> i64 {
        match self {
            McpError::Unauthorized => -32001,
            McpError::Parse(_) => -32700,
            McpError::MethodNotFound(_) => -32601,
            McpError::InvalidParams(_) => -32602,
            McpError::Internal(_) => -32603,
        }
    }
}

/// JSON-RPC 2.0 request
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<Value>,
    pub id: Option<Value>,
}

/// JSON-RPC 2.0 response
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn error(id: Option<Value>, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }
}

/// Check API key against configured key.
/// Returns true if no key is configured (dev mode) or the key matches.
pub fn check_api_key(config: &Config, headers: &HeaderMap) -> bool {
    let configured = match &config.custom_mcp_api_key {
        Some(k) if !k.is_empty() => k,
        _ => return true,
    };

    let provided = headers.get("x-api-key").and_then(|v| v.to_str().ok());

    match provided {
        Some(p) => crate::middleware::auth::constant_time_eq(p, configured),
        None => false,
    }
}

/// Create the MCP server router with `/mcp` and `/mcp/sse` routes.
/// The router has state type `Arc<AppState>` — handlers use `State<Arc<AppState>>`.
pub fn create_router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route(
            "/mcp",
            axum::routing::post(crate::mcp_server::handlers::mcp_handler),
        )
        .route(
            "/mcp/sse",
            axum::routing::post(crate::mcp_server::handlers::mcp_sse_handler),
        )
}
