//! MCP JSON-RPC handler dispatch logic.
//!
//! All handlers extract `State<Arc<AppState>>` so they work seamlessly when
//! merged into the main application router.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::mcp_server::server::{JsonRpcRequest, JsonRpcResponse, McpError};
use crate::mcp_server::tools::all_tools;
use crate::AppState;

/// Extract a required string parameter from the arguments Value.
/// Returns -32602 (Invalid params) with a descriptive message when the key is
/// missing or the value is not a JSON string.
fn require_str_param(args: &Value, key: &str) -> Result<String, McpError> {
    match args.get(key) {
        Some(v) if v.is_string() => Ok(v.as_str().unwrap().to_string()),
        Some(v) => Err(McpError::InvalidParams(format!(
            "Parameter '{}' must be a string, got: {}",
            key, v
        ))),
        None => Err(McpError::InvalidParams(format!(
            "Missing '{}' parameter",
            key
        ))),
    }
}

/// Extract an optional string parameter from the arguments Value.
/// Returns -32602 when the key is present but not a JSON string.
fn opt_str_param(args: &Value, key: &str) -> Result<Option<String>, McpError> {
    match args.get(key) {
        Some(v) if v.is_string() => Ok(Some(v.as_str().unwrap().to_string())),
        Some(v) => Err(McpError::InvalidParams(format!(
            "Parameter '{}' must be a string, got: {}",
            key, v
        ))),
        None => Ok(None),
    }
}

/// Handle JSON-RPC POST requests to /mcp
pub async fn mcp_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> (StatusCode, HeaderMap, Json<JsonRpcResponse>) {
    // Authentication
    if !crate::mcp_server::server::check_api_key(&state.config, &headers) {
        let resp = JsonRpcResponse::error(request.id, -32001, "Unauthorized".to_string());
        let h = HeaderMap::new();
        return (StatusCode::OK, h, Json(resp));
    }

    // Validate JSON-RPC version
    if request.jsonrpc != "2.0" {
        let resp = JsonRpcResponse::error(
            request.id,
            -32600,
            "Invalid Request: jsonrpc must be 2.0".to_string(),
        );
        let h = HeaderMap::new();
        return (StatusCode::BAD_REQUEST, h, Json(resp));
    }

    let result = dispatch(
        &state,
        &request.method,
        request.params.unwrap_or(json!({})),
        &request.id,
    )
    .await;

    match result {
        Ok(resp) => {
            let mut h = HeaderMap::new();
            h.insert("content-type", "application/json".parse().unwrap());
            (StatusCode::OK, h, Json(resp))
        }
        Err(e) => {
            let resp = JsonRpcResponse::error(request.id, e.code(), e.to_string());
            let h = HeaderMap::new();
            (StatusCode::OK, h, Json(resp))
        }
    }
}

/// Dispatch a JSON-RPC method to the appropriate handler.
async fn dispatch(
    state: &AppState,
    method: &str,
    params: Value,
    id: &Option<Value>,
) -> Result<JsonRpcResponse, McpError> {
    match method {
        "initialize" => Ok(JsonRpcResponse::success(
            id.clone(),
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {
                    "tools": {},
                    "prompts": {},
                },
                "serverInfo": {
                    "name": "study-platform-mcp",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        )),
        "tools/list" => {
            let tools = all_tools();
            Ok(JsonRpcResponse::success(
                id.clone(),
                json!({ "tools": tools }),
            ))
        }
        "tools/call" => {
            let tool_name = require_str_param(&params, "name")?;

            let args = params.get("arguments").cloned().unwrap_or(json!({}));

            handle_tool_call(state, &tool_name, args, id).await
        }
        _ => Err(McpError::MethodNotFound(method.to_string())),
    }
}

/// Handle individual tool calls.
async fn handle_tool_call(
    state: &AppState,
    tool_name: &str,
    args: Value,
    id: &Option<Value>,
) -> Result<JsonRpcResponse, McpError> {
    let knowledge = crate::knowledge::KnowledgeService::new(Arc::new(state.clone()));

    let result = match tool_name {
        "search_notes" => {
            let query = require_str_param(&args, "query")?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

            let results = knowledge
                .search(&query, limit)
                .await
                .map_err(McpError::Internal)?;

            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&results).unwrap_or("[]".to_string())
                }]
            })
        }
        "get_note" => {
            let slug = require_str_param(&args, "slug")?;

            match knowledge.get_note(&slug).await {
                Ok((note, content)) => json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&note).unwrap_or_default() + &format!("\n\n{}", content)
                    }]
                }),
                Err(e) => return Ok(JsonRpcResponse::error(id.clone(), -32000, e)),
            }
        }
        "create_note" => {
            let title = require_str_param(&args, "title")?;
            let content = require_str_param(&args, "content")?;
            let tags = args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            match knowledge.create_note(&title, content, tags).await {
                Ok(note) => json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string(&note).unwrap_or_default()
                    }]
                }),
                Err(e) => return Ok(JsonRpcResponse::error(id.clone(), -32000, e)),
            }
        }
        "get_academic_context" => {
            let ctx = if let Some(client) = crate::academic::mikareno_client(state) {
                if client.initialize().await.is_ok() {
                    let calendario = client.calendario().await.unwrap_or_default();
                    let horario = client.horario().await.unwrap_or_default();
                    let perfil = client.perfil().await.unwrap_or_default();
                    json!({
                        "connected": true,
                        "calendario": calendario,
                        "horario": horario,
                        "perfil": perfil,
                    })
                } else {
                    json!({ "connected": false, "error": "MCP not reachable" })
                }
            } else {
                json!({ "connected": false, "error": "MCP URL not configured for localhost" })
            };

            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&ctx).unwrap_or("{}".to_string())
                }]
            })
        }
        "get_subject_notes" => {
            let subject = require_str_param(&args, "subject")?;
            let results = knowledge
                .search(&subject, 20)
                .await
                .map_err(McpError::Internal)?;
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&results).unwrap_or("[]".to_string())
                }]
            })
        }
        "get_study_sheets" => {
            let subject = opt_str_param(&args, "subject")?;
            let query = match subject {
                Some(s) => format!("estudio {}", s),
                None => "estudio".to_string(),
            };
            let results = knowledge
                .search(&query, 20)
                .await
                .map_err(McpError::Internal)?;
            json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&results).unwrap_or("[]".to_string())
                }]
            })
        }
        _ => return Err(McpError::MethodNotFound(tool_name.to_string())),
    };

    Ok(JsonRpcResponse::success(id.clone(), result))
}

/// Handle SSE endpoint (GET /mcp/sse for streaming)
pub async fn mcp_sse_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    _request: Request<Body>,
) -> Response {
    if !crate::mcp_server::server::check_api_key(&state.config, &headers) {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::from("Unauthorized"))
            .unwrap();
    }

    // SSE stream with initialized notification
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(Body::from(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n\n"
        ))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_error_codes() {
        let e = McpError::Unauthorized;
        assert_eq!(e.code(), -32001);

        let e = McpError::Parse("test".to_string());
        assert_eq!(e.code(), -32700);

        let e = McpError::MethodNotFound("foo".to_string());
        assert_eq!(e.code(), -32601);
    }

    #[test]
    fn test_jsonrpc_success_response() {
        let resp = JsonRpcResponse::success(Some(json!(1)), json!({"result": "ok"}));
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_jsonrpc_error_response() {
        let resp = JsonRpcResponse::error(Some(json!(1)), -32601, "Method not found".to_string());
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, -32601);
    }
}
