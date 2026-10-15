//! MCP client over SSE/HTTP for the Mikareno student server.
//!
//! Implements the Subset MCP protocol needed by the Study Platform:
//!   1. `initialize`          → establishes a session, captures `mcp-session-id`.
//!   2. `tools/list`          → enumerates available tools.
//!   3. `tools/call`          → invokes a tool and decodes its `text` payload.
//!
//! Transport details (matches the FastMCP `transport="http"` server in
//! `mikareno-mcp/server.py` and the reference client `test_all.py`):
//!   * POST JSON-RPC 2.0 to the SSE endpoint (`base_url`).
//!   * The *first* `initialize` response carries an `Mcp-Session-Id` header
//!     (a UUID). Every subsequent request echoes it back via the same header.
//!   * The body is SSE: one or more `data: <jsonrpc>` lines, but it may also be
//!     a bare JSON document (stateless servers). Both are handled.
//!   * `tools/call` results are `{"content":[{"type":"text","text":"<json>"}]}`
//!     — the `text` is *double-encoded*, so we parse it back into a `Value`.
//!
//! The client is read-only by construction: there is simply no way to issue a
//! write-style method — `call_tool` only forwards the tool names the adapters
//! use, all of which are `get_*` reads. It is also `Clone`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use reqwest::header::{HeaderMap, ACCEPT};
use serde_json::{json, Value};
use thiserror::Error;
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::integrations::mikareno::adapters;
use crate::integrations::mikareno::cache::SseCache;
use crate::integrations::mikareno::models::{
    CalendarioEvent, Horario, Nota, Perfil, Tarea, ToolInfo,
};

/// Default cache TTL (seconds) — mirrors the server's own cache window.
pub const DEFAULT_CACHE_TTL_SECS: u64 = 180;
/// Per-request timeout.
pub const REQUEST_TIMEOUT_SECS: u64 = 30;
/// Maximum number of retries after the initial attempt.
pub const MAX_RETRIES: u32 = 3;
/// Backoff base in milliseconds; doubles with each retry (500, 1000, 2000).
pub const BACKOFF_BASE_MS: u64 = 500;

/// Errors returned by the Mikareno MCP client.
#[derive(Debug, Error)]
pub enum MikarenoError {
    #[error("network or HTTP error: {0}")]
    Http(String),
    #[error("not connected to the MCP session")]
    NotConnected,
    #[error("failed to parse SSE/JSON-RPC response: {0}")]
    SseParse(String),
    #[error("JSON-RPC error (code {code}): {message}")]
    JsonRpc { code: i64, message: String },
    #[error("unexpected response shape: {0}")]
    Unexpected(String),
}

impl MikarenoError {
    pub fn is_connection_error(&self) -> bool {
        matches!(self, MikarenoError::Http(_))
    }
}

/// Shared, interior-mutable state behind the client.
///
/// Held behind an `Arc` so the client can be freely cloned and the methods
/// stay `&self` (required for ergonomic use from request handlers). The mutexes
/// guard only tiny, non-async critical sections.
struct ClientState {
    /// Current `mcp-session-id`, present only after a successful `initialize`.
    session_id: Mutex<Option<String>>,
    /// Response cache (keyed by `tool(args)`), TTL-bounded.
    cache: Mutex<SseCache>,
    /// Whether the client has a valid, initialized session.
    connected: AtomicBool,
    /// Whether `initialize` has already run (separate from `connected`, since a
    /// stateless server may not issue a session id but still answer requests).
    initialized: AtomicBool,
    /// Monotonic request id counter for JSON-RPC `id` fields.
    next_id: AtomicU64,
}

/// A read-only MCP client for the Mikareno student server.
#[derive(Clone)]
pub struct MikarenoClient {
    base_url: String,
    http: reqwest::Client,
    state: std::sync::Arc<ClientState>,
}

impl MikarenoClient {
    /// Create a new client pointing at `base_url` (e.g.
    /// `http://127.0.0.1:8080/mcp`) with the default 180s cache TTL.
    pub fn new(base_url: String) -> Self {
        Self::with_cache_ttl(base_url, DEFAULT_CACHE_TTL_SECS)
    }

    /// Create a new client with a custom cache TTL.
    pub fn with_cache_ttl(base_url: String, ttl_secs: u64) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .user_agent("study-platform/0.1.0 (MikarenoMCPClient)")
            .build()
            .expect("failed to build reqwest client — this is a programming error");
        Self {
            base_url,
            http,
            state: std::sync::Arc::new(ClientState {
                session_id: Mutex::new(None),
                cache: Mutex::new(SseCache::new(ttl_secs)),
                connected: AtomicBool::new(false),
                initialized: AtomicBool::new(false),
                next_id: AtomicU64::new(1),
            }),
        }
    }

    fn next_id(&self) -> u64 {
        self.state.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Returns `true` once `initialize` has completed successfully.
    pub fn is_connected(&self) -> bool {
        self.state.connected.load(Ordering::Acquire)
    }

    /// Mark the session as alive and store the session id (if any).
    fn on_handshake_ok(&self, session_id: Option<String>) {
        {
            let mut s = self.state.session_id.lock().expect("session lock poisoned");
            *s = session_id;
        }
        self.state.initialized.store(true, Ordering::Release);
        self.state.connected.store(true, Ordering::Release);
    }

    /// Mark the session as lost: clears the id and forgets initialization state.
    fn mark_disconnected(&self) {
        {
            let mut s = self.state.session_id.lock().expect("session lock poisoned");
            *s = None;
        }
        self.state.initialized.store(false, Ordering::Release);
        self.state.connected.store(false, Ordering::Release);
    }

    /// Lazily run `initialize` if we don't yet have a session.
    async fn ensure_initialized(&self) -> Result<(), MikarenoError> {
        if self.state.initialized.load(Ordering::Acquire) {
            return Ok(());
        }
        self.initialize().await
    }

    // ── low-level transport ───────────────────────────────────────────

    /// POST a JSON-RPC envelope to the SSE endpoint with retries and backoff.
    ///
    /// Returns the response headers (for `mcp-session-id` harvesting) and the
    /// raw body text. Transport-level failures (connection refused, timeout)
    /// mark the session disconnected so the next call re-initializes.
    async fn post_sse(
        &self,
        body: &Value,
        session: Option<&str>,
    ) -> Result<(HeaderMap, String), MikarenoError> {
        let mut last_err: Option<MikarenoError> = None;

        for attempt in 0..=MAX_RETRIES {
            let mut builder = self
                .http
                .post(&self.base_url)
                .header(ACCEPT, "application/json, text/event-stream")
                .header("content-type", "application/json")
                .json(body);
            if let Some(sid) = session {
                builder = builder.header("mcp-session-id", sid);
            }

            match builder.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let headers = resp.headers().clone();
                    if status.is_success() {
                        match resp.text().await {
                            Ok(text) => return Ok((headers, text)),
                            Err(e) => {
                                debug!("reqwest body read failed on attempt {}: {}", attempt, e);
                                last_err = Some(MikarenoError::Http(e.to_string()));
                            }
                        }
                    } else if status.as_u16() >= 500 {
                        // Server-side fault — retryable; treat as disconnected.
                        warn!("MCP server error HTTP {} on attempt {}", status, attempt);
                        self.mark_disconnected();
                        let snippet = self.snippet(&resp.text().await.unwrap_or_default());
                        last_err =
                            Some(MikarenoError::Http(format!("HTTP {}: {}", status, snippet)));
                    } else {
                        // 4xx — not a retryable transport fault, surface immediately.
                        let snippet = self.snippet(&resp.text().await.unwrap_or_default());
                        return Err(MikarenoError::Http(format!("HTTP {}: {}", status, snippet)));
                    }
                }
                Err(e) => {
                    if e.is_connect() || e.is_timeout() || e.is_request() {
                        self.mark_disconnected();
                    }
                    debug!("reqwest transport error on attempt {}: {}", attempt, e);
                    last_err = Some(MikarenoError::Http(e.to_string()));
                }
            }

            if attempt < MAX_RETRIES {
                let backoff = Duration::from_millis(BACKOFF_BASE_MS * (1u64 << attempt));
                sleep(backoff).await;
            }
        }

        Err(last_err.unwrap_or(MikarenoError::Http("exhausted retries".into())))
    }

    fn snippet(&self, text: &str) -> String {
        let t = text.trim();
        if t.len() > 200 {
            format!("{}...", &t[..200])
        } else {
            t.to_string()
        }
    }

    // ── protocol primitives ───────────────────────────────────────────

    /// Send the `initialize` request and harvest the session id.
    pub async fn initialize(&self) -> Result<(), MikarenoError> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": self.next_id(),
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {
                    "name": "study-platform",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            },
        });

        let (headers, text) = self.post_sse(&body, None).await?;

        // FastMCP returns the session id in `Mcp-Session-Id` (HTTP headers are
        // case-insensitive, so a single lowercase lookup suffices).
        let session_id = headers
            .get("mcp-session-id")
            .or_else(|| headers.get("Mcp-Session-Id"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let messages = parse_sse(&text);
        let _result = extract_jsonrpc(&messages)?; // validates we got a real handshake

        debug!(
            "Mikareno initialize ok; session_id={}",
            session_id.as_deref().unwrap_or("(none)")
        );
        self.on_handshake_ok(session_id);
        Ok(())
    }

    /// Send `tools/list` and decode the advertised tools.
    pub async fn list_tools(&self) -> Result<Vec<ToolInfo>, MikarenoError> {
        // Cache the tool list for the configured TTL.
        let cache_key = "tools:list".to_string();
        {
            let c = self.state.cache.lock().expect("cache lock poisoned");
            if let Some(cached) = c.get(&cache_key) {
                return tools_from_value(cached);
            }
        }

        self.ensure_initialized().await?;
        let session = self
            .state
            .session_id
            .lock()
            .expect("session lock poisoned")
            .clone();

        let body = json!({
            "jsonrpc": "2.0",
            "id": self.next_id(),
            "method": "tools/list",
        });
        let (_headers, text) = self.post_sse(&body, session.as_deref()).await?;
        let result = extract_jsonrpc(&parse_sse(&text))?;

        // Cache the raw result envelope so list_tools stays cheap on repeats.
        {
            let mut c = self.state.cache.lock().expect("cache lock poisoned");
            c.set(cache_key, result.clone());
        }
        tools_from_value(&result)
    }

    /// Invoke a tool by name and return its decoded `text` payload as a `Value`.
    pub async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, MikarenoError> {
        // Read-through cache keyed by tool name + args.
        let cache_key = format!(
            "call:{}:{}",
            name,
            serde_json::to_string(&args).unwrap_or_default()
        );
        {
            let c = self.state.cache.lock().expect("cache lock poisoned");
            if let Some(cached) = c.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

        self.ensure_initialized().await?;
        let session = self
            .state
            .session_id
            .lock()
            .expect("session lock poisoned")
            .clone();

        let body = json!({
            "jsonrpc": "2.0",
            "id": self.next_id(),
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        });

        let (_h, text) = self.post_sse(&body, session.as_deref()).await?;
        let messages = parse_sse(&text);
        let result = extract_jsonrpc(&messages)?;
        let inner = extract_tool_result(&result)?;

        {
            let mut c = self.state.cache.lock().expect("cache lock poisoned");
            c.set(cache_key, inner.clone());
        }
        Ok(inner)
    }

    // ── typed adapters ───────────────────────────────────────────────

    /// `get_calendario` → flat list of calendar events.
    pub async fn calendario(&self) -> Result<Vec<CalendarioEvent>, MikarenoError> {
        let raw = self.call_tool("get_calendario", json!({})).await?;
        adapters::parse_calendario(&raw).map_err(MikarenoError::SseParse)
    }

    /// `get_horario` → grouped weekly schedule.
    pub async fn horario(&self) -> Result<Horario, MikarenoError> {
        let raw = self.call_tool("get_horario", json!({})).await?;
        adapters::parse_horario(&raw).map_err(MikarenoError::SseParse)
    }

    /// `get_perfil` → { nombre, institucion, curso }.
    pub async fn perfil(&self) -> Result<Perfil, MikarenoError> {
        let raw = self.call_tool("get_perfil", json!({})).await?;
        adapters::parse_perfil(&raw).map_err(MikarenoError::SseParse)
    }

    /// `get_tareas` → flat list of homework tasks.
    ///
    /// Live-verified: `get_tareas` IS advertised by the real server
    /// (`tools/list`, Mikareno Student MCP v4.0.11, 2026-10-10) with input
    /// schema `{force_refresh?: boolean}`. `docs/ARCHITECTURE.md` §3 omits it
    /// from its 17-tool list — that doc list is stale (it also names 7
    /// `get_detalles_*/get_mis_grupos/get_detalle_calendario` tools the server
    /// does not advertise), not the client. Callers already degrade
    /// gracefully via `unwrap_or_default()` when the tool errors/absent.
    pub async fn tareas(&self) -> Result<Vec<Tarea>, MikarenoError> {
        let raw = self.call_tool("get_tareas", json!({})).await?;
        adapters::parse_tareas(&raw).map_err(MikarenoError::SseParse)
    }

    /// `get_notas_trimestre` → graded subjects for a term (1/2/3).
    pub async fn notas(&self, trimestre: u8) -> Result<Vec<Nota>, MikarenoError> {
        let raw = self
            .call_tool("get_notas_trimestre", json!({ "trimestre": trimestre }))
            .await?;
        adapters::parse_notas(&raw).map_err(MikarenoError::SseParse)
    }

    // ── introspection (used by tests) ─────────────────────────────────

    /// Current session id, if any. Exposed for diagnostics/tests only.
    pub fn session_id(&self) -> Option<String> {
        self.state
            .session_id
            .lock()
            .expect("session lock poisoned")
            .clone()
    }
}

/// Decode a `tools/call`/`tools/list` result into the inner payload.
///
/// The server wraps tool output as `{"content":[{"type":"text","text":"..."}]}`
/// where `text` is a JSON-encoded string. We parse it back into a `Value`; if
/// the text is not valid JSON we fall back to returning it as a string.
fn extract_tool_result(result: &Value) -> Result<Value, MikarenoError> {
    if result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let msg = result
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("tool reported an error");
        return Err(MikarenoError::Unexpected(format!(
            "tool returned isError: {}",
            msg
        )));
    }

    if let Some(content) = result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
    {
        if content.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(text) = content.get("text").and_then(|t| t.as_str()) {
                if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                    return Ok(parsed);
                }
                return Ok(Value::String(text.to_string()));
            }
        }
    }
    Err(MikarenoError::Unexpected(
        "no text content in tool result".into(),
    ))
}

fn tools_from_value(result: &Value) -> Result<Vec<ToolInfo>, MikarenoError> {
    let arr = result
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| MikarenoError::Unexpected("tools/list: missing 'tools' array".into()))?;
    let mut out = Vec::with_capacity(arr.len());
    for t in arr {
        let name = t
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = t
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(ToolInfo { name, description });
    }
    Ok(out)
}

/// Split an SSE body into its JSON-RPC `data:` messages.
///
/// Handles multi-line `data:` concatenation per the SSE spec, the
/// `data: ` (with space) and `data:` (no space) prefixes, and falls back to
/// treating the whole body as a bare JSON document when no `data:` lines exist
/// (stateless servers).
fn parse_sse(body: &str) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut data = String::new();
    let mut have_data = false;

    for raw in body.split('\n') {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            flush_data(&data, have_data, &mut messages);
            data.clear();
            have_data = false;
        } else if let Some(rest) = line.strip_prefix("data:") {
            have_data = true;
            let content = rest.strip_prefix(' ').unwrap_or(rest);
            if data.is_empty() {
                data.push_str(content);
            } else {
                data.push('\n');
                data.push_str(content);
            }
        }
        // Lines we don't care about (`:` comments, `retry:`, `event:`) are ignored.
    }
    // Trailing flush for bodies that don't end in a blank line.
    flush_data(&data, have_data, &mut messages);

    if messages.is_empty() {
        // Not SSE — try the whole body as a single JSON-RPC document.
        if let Ok(v) = serde_json::from_str::<Value>(body.trim()) {
            messages.push(v);
        }
    }
    messages
}

fn flush_data(data: &str, have: bool, out: &mut Vec<Value>) {
    if have {
        let trimmed = data.trim();
        if !trimmed.is_empty() {
            if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                out.push(v);
            }
        }
    }
}

/// Pick the first JSON-RPC message carrying a `result` (or surface an `error`).
fn extract_jsonrpc(messages: &[Value]) -> Result<Value, MikarenoError> {
    for msg in messages {
        if let Some(err) = msg.get("error") {
            let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let message = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
                .to_string();
            return Err(MikarenoError::JsonRpc { code, message });
        }
        if let Some(result) = msg.get("result") {
            return Ok(result.clone());
        }
    }
    Err(MikarenoError::SseParse(
        "no jsonrpc result or error in response".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sse_single_data_line_parses() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        let msgs = parse_sse(body);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["result"]["ok"], json!(true));
    }

    #[test]
    fn sse_no_space_prefix_parses() {
        let body = "data:{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":42}\n";
        let msgs = parse_sse(body);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["result"], json!(42));
    }

    #[test]
    fn sse_bare_json_fallback_parses() {
        let body = "{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"tools\":[]}}";
        let msgs = parse_sse(body);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0]["result"]["tools"].is_array());
    }

    #[test]
    fn sse_multiline_data_concatenates() {
        let body =
            "data: {\"jsonrpc\":\"2.0\"".to_string() + "\ndata: ,\"id\":1,\"result\":true}\n\n";
        let msgs = parse_sse(&body);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["result"], json!(true));
    }

    #[test]
    fn extract_jsonrpc_picks_result_and_errors() {
        let ok = parse_sse("data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"a\":1}}\n\n");
        assert_eq!(extract_jsonrpc(&ok).unwrap()["a"], json!(1));

        let bad = parse_sse("data: {\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32601,\"message\":\"not found\"}}\n\n");
        let err = extract_jsonrpc(&bad).unwrap_err();
        match err {
            MikarenoError::JsonRpc { code, message } => {
                assert_eq!(code, -32601);
                assert_eq!(message, "not found");
            }
            other => panic!("wrong error variant: {:?}", other),
        }
    }

    #[test]
    fn extract_tool_result_unwraps_text_payload() {
        // tools/call result envelope with a double-encoded text payload
        let result = json!({
            "content": [{"type":"text","text": "{\"trimestre\":1,\"materias_y_notas\":[]}"}]
        });
        let inner = extract_tool_result(&result).unwrap();
        assert_eq!(inner["trimestre"], json!(1));
        assert!(inner["materias_y_notas"].is_array());
    }

    #[test]
    fn extract_tool_result_flags_is_error() {
        let result = json!({"isError": true, "content":[{"type":"text","text":"boom"}]});
        assert!(extract_tool_result(&result).is_err());
    }

    #[test]
    fn extract_tool_result_non_json_text_falls_back_to_string() {
        let result = json!({"content":[{"type":"text","text":"plain text"}]});
        let inner = extract_tool_result(&result).unwrap();
        assert_eq!(inner, json!("plain text"));
    }

    #[test]
    fn client_builds_with_custom_ttl() {
        // Smoke test: construction must not panic and the default client must
        // start disconnected (no initialize has run yet).
        let c = MikarenoClient::with_cache_ttl("http://invalid.invalid/mcp".into(), 1);
        assert!(!c.is_connected());
        assert!(c.session_id().is_none());
    }
}
