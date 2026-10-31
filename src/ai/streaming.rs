//! SSE streaming for OpenRouter chat completions.
//!
//! # Wire contract (server → client)
//!
//! The response is `text/event-stream`. Every frame is
//! `event: <name>\ndata: <json>\n\n`:
//!
//! | event       | data                                          | when                                        |
//! |-------------|-----------------------------------------------|---------------------------------------------|
//! | `context`   | `{"mode":"...","web":bool,"sources":[]}`      | initial RAG manifest when `mode` is sent    |
//! | `sources`   | `{"sources":[...]}`                           | real web citations received from provider   |
//! | `delta`     | `{"content":"..."}`                           | one `choices[0].delta.content` chunk        |
//! | `reasoning` | `{"reasoning":"..."}`                         | one `choices[0].delta.reasoning` chunk      |
//! | `done`      | `{"usage":{"input":N,"output":N,"cost":0.0}}` | terminal frame (the stream finished)         |
//! | `error`     | `{"error":"..."}`                             | the upstream failed mid-stream              |
//!
//! Without an API key the stream carries exactly one `done` frame with
//! `{"content":"[Modo simulado] streaming no disponible sin API key"}` —
//! honest: no fabricated content, no invented usage.
//!
//! The provider (OpenRouter) sends `data: {json}` chunks where
//! `choices[0].delta.content` / `choices[0].delta.reasoning` carry the text
//! and a final `usage` object carries the token counts, terminated by the
//! `data: [DONE]` sentinel.
//!
//! # Implementation notes
//!
//! `axum::response::sse::Sse` cannot be used here: it requires a
//! `futures_core::Stream`, and neither `futures`, `futures-core`,
//! `tokio-stream` nor `async-stream` is a direct dependency (`Cargo.toml` is
//! out of scope for this module), so the trait cannot even be named — the
//! `Stream` trait would have to be implemented by hand for a type that axum
//! accepts. The body is therefore a hand-rolled `http_body::Body`
//! impl (`SseBody`) fed by a bounded `tokio::sync::mpsc` channel and
//! accepted by [`axum::body::Body::new`] — a spawned tokio task pumps the
//! provider response into it.
//!
//! The frames written to that body are byte-identical to the ones
//! `axum::response::sse::Sse` would produce (`event: <name>\ndata: <json>\n\n`),
//! so the wire contract above is what any client sees, `EventSource`
//! consumers included.
//!
//! # Limits
//!
//! `message` ≤ [`MAX_MESSAGE_CHARS`] (8000) and `content` ≤
//! [`MAX_CONTENT_CHARS`] (20000), mirroring the buffered chat handler.
//! A body without `message` is rejected with HTTP 400.
//!
//! # Persistence
//!
//! Every streamed turn is written to the database BEFORE the terminal
//! `done` frame is emitted: the user's message and the accumulated
//! assistant answer go to `chat_history` (the same table and the same
//! columns the buffered chat path uses, so `/dashboard/ai` and
//! `GET /api/ai/conversations` also show the streamed turns), and the real
//! token counts go to `usage_metrics` with a 0.0 USD cost (the streaming
//! `usage` object carries no trustworthy cost, so the row is marked
//! `estimated`).
//!
//! The persistence is best-effort: a failing INSERT is logged with
//! `tracing::warn!` and never breaks the stream that was already emitted.

use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Json, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use http_body::Frame;
use serde_json::{json, Value};
use tracing::{debug, warn};

use crate::ai::handlers::{MAX_CONTENT_CHARS, MAX_MESSAGE_CHARS, MAX_SUBJECT_CHARS};
use crate::ai::openrouter::{ensure_usage_metrics_table, web_search_tools, AiError, UsageEntry};
use crate::ai::rag::{RagSource, RetrievalMode};
use crate::AppState;

/// Upstream chat-completions endpoint (same base as [`crate::ai::openrouter`]).
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Message returned by the mock-mode stream (no API key configured).
const MOCK_STREAM_MESSAGE: &str = "[Modo simulado] streaming no disponible sin API key";

/// Frames the client may lag behind before backpressure reaches the pump
/// (bounded channel; one frame per SSE event).
const CHANNEL_BUFFER: usize = 16;

/// Idle cap between provider chunks: a stalled provider ends the relay
/// instead of leaving the client hanging on the stream.
const CHUNK_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Hard cap on the whole relay: bounds the spawned task's lifetime even if
/// the provider streams steadily forever.
const STREAM_TOTAL_TIMEOUT: Duration = Duration::from_secs(300);

/// Timeout for the TCP/TLS connection to the provider (the body itself is
/// long-lived and is bounded by the two caps above).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-stream handle, derivable by clients from the `x-chat-stream-id`
/// response header, for reconnect/telemetry bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatStreamId(String);

impl ChatStreamId {
    /// Generate a random 32-hex-char id (not a secret; safe to log).
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ChatStreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Streamed turn (what gets persisted when the relay ends)
// ---------------------------------------------------------------------------

/// The conversation turn a stream belongs to.
///
/// The relay task owns a clone of it, so the pump can store the turn in
/// SQLite without borrowing the handler's state.
#[derive(Clone)]
pub struct StreamTurn {
    /// Shared app state (holds the SQLite pool).
    pub state: Arc<AppState>,
    /// Conversation subject (as validated by the handler).
    pub subject: String,
    /// The user's message (the prompt that was sent to the provider).
    pub user_message: String,
}

// ---------------------------------------------------------------------------
// SSE frame helpers (server → client)
// ---------------------------------------------------------------------------

/// Serialize one SSE frame: `event: <name>\ndata: <payload>\n\n`.
///
/// Byte-compatible with `axum::response::sse::Event`. `serde_json` escapes
/// newlines inside the payload, so a frame always carries exactly one
/// `data:` field. Returns `None` (instead of panicking) when the payload
/// cannot be serialized.
fn sse_frame(event: &str, payload: &Value) -> Option<String> {
    let data = serde_json::to_string(payload).ok()?;
    let mut frame = String::with_capacity(data.len() + event.len() + 16);
    frame.push_str("event: ");
    frame.push_str(event);
    frame.push('\n');
    frame.push_str("data: ");
    frame.push_str(&data);
    frame.push('\n');
    frame.push('\n');
    Some(frame)
}

/// Send one SSE frame. Returns `false` when the client is gone, so the
/// caller stops pumping the upstream connection.
async fn send_frame(tx: &mut Tx, event: &str, payload: &Value) -> bool {
    let Some(frame) = sse_frame(event, payload) else {
        debug!("chat/stream: frame skipped, payload did not serialize");
        return false;
    };
    match tx.send(frame).await {
        Ok(()) => true,
        Err(_) => {
            debug!("chat/stream: client disconnected, relay stops");
            false
        }
    }
}

/// Token/cost usage reported by the provider.
#[derive(Debug, Default, Clone, Copy)]
struct Usage {
    input: u64,
    output: u64,
    cost: f64,
}

impl Usage {
    /// Capture the provider's `usage` object (absent fields keep their
    /// current value).
    fn capture(&mut self, usage: &Value) {
        if let Some(v) = usage.get("prompt_tokens").and_then(Value::as_u64) {
            self.input = v;
        }
        if let Some(v) = usage.get("completion_tokens").and_then(Value::as_u64) {
            self.output = v;
        }
        if let Some(v) = usage.get("cost").and_then(Value::as_f64) {
            self.cost = v;
        }
    }

    /// Payload of the terminal `done` frame.
    fn payload(&self) -> Value {
        json!({
            "usage": {
                "input": self.input,
                "output": self.output,
                "cost": self.cost,
            }
        })
    }
}

/// Response headers of a streaming reply (same as axum's `Sse`).
fn sse_response(body: Body) -> Result<Response, AiError> {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(body)
        .map_err(|e| AiError::Http(e.to_string()))
}

/// Attach the stream id header (best-effort: the id is stable ASCII hex).
fn with_stream_id(mut resp: Response, stream_id: &str) -> Response {
    if let Ok(value) = axum::http::HeaderValue::from_str(stream_id) {
        resp.headers_mut().insert(
            axum::http::HeaderName::from_static("x-chat-stream-id"),
            value,
        );
    }
    resp
}

/// JSON error response (HTTP 400) with a generic, client-safe message.
fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}

/// JSON error response for an upstream failure, mapped exactly like the
/// buffered proxy in [`crate::ai::openrouter::chat_completion`].
fn error_response(e: &AiError) -> Response {
    (
        e.status_code(),
        Json(json!({ "error": e.client_message() })),
    )
        .into_response()
}

/// Server→client frame channel owned by the relay task (`pump_stream`).
///
/// Cancellation by client disconnect: dropping the response drops `rx`,
/// every subsequent send returns `Err` and the pump exits.
type Tx = tokio::sync::mpsc::Sender<String>;

/// SSE response body: frames queued by the pump, polled by axum.
///
/// `axum::response::sse::Sse` is not usable here (it requires a
/// `futures_core::Stream` and no stream crate is a direct dependency),
/// so this hand-roll implements the same wire protocol over the trait
/// `axum::body::Body::new` accepts: `http_body::Body`.
struct SseBody {
    rx: tokio::sync::mpsc::Receiver<String>,
}

impl http_body::Body for SseBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut(); // `Receiver<String>` is `Unpin`
        match this.rx.poll_recv(cx) {
            std::task::Poll::Ready(Some(frame)) => {
                std::task::Poll::Ready(Some(Ok(Frame::data(Bytes::from(frame)))))
            }
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

// ---------------------------------------------------------------------------
// Upstream → client pump
// ---------------------------------------------------------------------------

/// Outcome of dispatching one accumulated provider event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dispatch {
    /// More events may follow.
    Continue,
    /// The provider signalled the end of the stream (`data: [DONE]`).
    Done,
    /// The client is gone; the relay must stop.
    Aborted,
}

/// Relay the OpenRouter response body into the client channel.
///
/// Consumes `tx`. On a clean upstream end the pump appends the terminal
/// `done` frame with the usage summary. On an upstream failure it appends
/// one `error` frame with a safe [`AiError::client_message`] payload —
/// never provider internals — and ends the response.
///
/// The accumulated `content`/`reasoning` and the usage are stored in the
/// database (see [`persist_turn`]) right before the terminal frame, so a
/// turn is never lost because the client disconnected after the last
/// delta.
async fn pump_stream(
    mut response: reqwest::Response,
    mut tx: Tx,
    turn: StreamTurn,
    initial_context: Option<Value>,
) {
    let mut buffer = String::new();
    let mut data_lines: Vec<String> = Vec::new();
    let mut usage = Usage::default();
    let mut failure: Option<AiError> = None;
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut seen_web_sources = HashSet::new();
    let started_at = Instant::now();

    // Las notas se conocen antes de llamar al proveedor: la UI puede abrir
    // el panel de fuentes mientras la respuesta todavía se está generando.
    if let Some(context) = initial_context {
        if !send_frame(&mut tx, "context", &context).await {
            return;
        }
    }

    'relay: loop {
        let chunk = match tokio::time::timeout(CHUNK_IDLE_TIMEOUT, response.chunk()).await {
            Ok(Ok(Some(chunk))) => chunk,
            Ok(Ok(None)) => break, // provider closed the stream
            Ok(Err(e)) => {
                failure = Some(AiError::Http(e.to_string()));
                break 'relay;
            }
            Err(_) => {
                failure = Some(AiError::Api("provider stream stalled".to_string()));
                break 'relay;
            }
        };

        let text = match std::str::from_utf8(&chunk) {
            Ok(t) => t,
            Err(e) => {
                failure = Some(AiError::Parse(format!("provider body is not UTF-8: {e}")));
                break 'relay;
            }
        };
        buffer.push_str(text);

        while let Some(pos) = buffer.find('\n') {
            let raw: String = buffer.drain(..=pos).collect();
            let line = raw.trim_end_matches('\r');
            if line.is_empty() {
                // Blank line: the accumulated event is complete.
                if !data_lines.is_empty() {
                    match dispatch_event(
                        &mut tx,
                        &mut usage,
                        &mut content,
                        &mut reasoning,
                        &mut seen_web_sources,
                        &mut data_lines,
                    )
                    .await
                    {
                        Dispatch::Continue => {}
                        Dispatch::Done => break 'relay,
                        Dispatch::Aborted => break 'relay,
                    }
                }
                continue;
            }
            if line.starts_with(':') {
                // Comment (e.g. `: OPENROUTER PROCESSING`) — ignored.
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                // One leading space after the field name is syntax, not data.
                data_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
            // `event:`/`id:`/`retry:` lines are ignored: this relay re-emits
            // its own `delta`/`reasoning`/`done`/`error` event names.
        }

        if started_at.elapsed() > STREAM_TOTAL_TIMEOUT {
            failure = Some(AiError::Api("provider stream timed out".to_string()));
            break 'relay;
        }
    }

    // Flush the tail: a final event cut before its blank-line delimiter is
    // still processed when it parses.
    if failure.is_none() && !data_lines.is_empty() {
        let _ = dispatch_event(
            &mut tx,
            &mut usage,
            &mut content,
            &mut reasoning,
            &mut seen_web_sources,
            &mut data_lines,
        )
        .await;
    }

    // The turn is stored BEFORE the terminal frame: the client may drop
    // right after the last delta and the conversation must still be in
    // `chat_history`. A failed INSERT is logged, never fatal.
    let completed = failure.is_none();
    persist_turn(&turn, &content, &reasoning, &usage, completed).await;

    match failure {
        None => {
            let _ = send_frame(&mut tx, "done", &usage.payload()).await;
        }
        Some(e) => {
            warn!(detail = %e.log_detail(), "chat/stream: upstream failed mid-stream");
            let _ = send_frame(&mut tx, "error", &json!({ "error": e.client_message() })).await;
        }
    }
}

/// Dispatch one accumulated provider event (its `data:` lines joined with
/// `\n`, per the SSE spec).
///
/// Returns [`Dispatch::Done`] when the provider sent the `data: [DONE]`
/// sentinel. Malformed frames are skipped with a `debug` log instead of
/// aborting the stream.
async fn dispatch_event(
    tx: &mut Tx,
    usage: &mut Usage,
    content: &mut String,
    reasoning: &mut String,
    seen_web_sources: &mut HashSet<String>,
    data_lines: &mut Vec<String>,
) -> Dispatch {
    let joined = data_lines.join("\n");
    data_lines.clear();

    if joined.trim() == "[DONE]" {
        return Dispatch::Done;
    }

    match serde_json::from_str::<Value>(&joined) {
        Ok(value) => {
            relay_chunk(
                tx,
                usage,
                content,
                reasoning,
                seen_web_sources,
                &value,
            )
            .await
        }
        Err(e) => {
            // Recovery: providers that emit one JSON object per `data:` line
            // without blank-line delimiters — retry each line on its own.
            let mut relayed = false;
            for line in joined.split('\n') {
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    match relay_chunk(
                        tx,
                        usage,
                        content,
                        reasoning,
                        seen_web_sources,
                        &value,
                    )
                    .await
                    {
                        Dispatch::Continue => relayed = true,
                        Dispatch::Aborted => return Dispatch::Aborted,
                        Dispatch::Done => return Dispatch::Done,
                    }
                }
            }
            if !relayed {
                debug!(detail = %e, "chat/stream: skipped malformed provider frame");
            }
            Dispatch::Continue
        }
    }
}

/// Relay one parsed provider chunk: the `content` and `reasoning` deltas
/// become `delta`/`reasoning` frames; a `usage` object updates the usage
/// summary carried by the terminal `done` frame.
///
/// The deltas are also accumulated in `content`/`reasoning`: that text is
/// what gets stored in `chat_history` when the relay ends.
///
/// Missing or unexpected fields are ignored (never panics).
async fn relay_chunk(
    tx: &mut Tx,
    usage: &mut Usage,
    content_buf: &mut String,
    reasoning_buf: &mut String,
    seen_web_sources: &mut HashSet<String>,
    value: &Value,
) -> Dispatch {
    if let Some(u) = value.get("usage").filter(|u| u.is_object()) {
        usage.capture(u);
    }

    let choice = value.get("choices").and_then(|choices| choices.get(0));
    let delta = choice.and_then(|choice| choice.get("delta"));
    let annotations = delta
        .and_then(|delta| delta.get("annotations"))
        .or_else(|| {
            choice
                .and_then(|choice| choice.get("message"))
                .and_then(|message| message.get("annotations"))
        })
        .and_then(Value::as_array);
    if let Some(annotations) = annotations {
        for source in annotations.iter().filter_map(RagSource::from_web_annotation) {
            if seen_web_sources.insert(source.url.clone())
                && !send_frame(tx, "sources", &json!({ "sources": [source] })).await
            {
                return Dispatch::Aborted;
            }
        }
    }

    let Some(delta) = delta else {
        // Provider bookkeeping frames (role-only deltas, usage-only chunks)
        // produce no client-visible content.
        return Dispatch::Continue;
    };

    let content = delta.get("content").and_then(Value::as_str).unwrap_or("");
    let reasoning = delta.get("reasoning").and_then(Value::as_str).unwrap_or("");

    if !content.is_empty() {
        // Accumulated before the frame is sent: a failed send still keeps
        // the text that was already produced.
        content_buf.push_str(content);
        if !send_frame(tx, "delta", &json!({ "content": content })).await {
            return Dispatch::Aborted;
        }
    }
    if !reasoning.is_empty() {
        reasoning_buf.push_str(reasoning);
        if !send_frame(tx, "reasoning", &json!({ "reasoning": reasoning })).await {
            return Dispatch::Aborted;
        }
    }
    Dispatch::Continue
}

// ---------------------------------------------------------------------------
// Core streamer
// ---------------------------------------------------------------------------

/// POST the OpenRouter chat-completions request with `"stream": true` and
/// return an SSE response relaying it.
///
/// The `messages` argument is the provider's `messages` array (objects of
/// `{"role": ..., "content": ...}`); it is forwarded as received so the
/// caller fully controls the prompt.
///
/// `turn` carries the conversation to store when the relay ends: the relay
/// task owns a clone of it and writes the turn to `chat_history` /
/// `usage_metrics` before the terminal frame (see [`persist_turn`]).
///
/// Non-2xx provider responses (401 invalid key, 429 rate limit, 5xx) are
/// returned as [`AiError`] before any response body is produced, so the
/// caller maps them with [`AiError::status_code`] + [`AiError::client_message`]
/// — parity with the buffered proxy. Errors that appear mid-stream (drop,
/// stall) surface as an `error` SSE frame.
pub async fn stream_chat(
    http: reqwest::Client,
    api_key: &str,
    model: &str,
    messages: Value,
    max_tokens: u64,
    turn: StreamTurn,
) -> Result<Response, AiError> {
    stream_chat_grounded(
        http, api_key, model, messages, max_tokens, turn, None, false,
    )
    .await
}

/// Streaming con manifiesto de fuentes y web opcional. El wrapper anterior se
/// conserva para los consumidores que no envían un modo RAG.
#[allow(clippy::too_many_arguments)]
pub async fn stream_chat_grounded(
    http: reqwest::Client,
    api_key: &str,
    model: &str,
    messages: Value,
    max_tokens: u64,
    turn: StreamTurn,
    initial_context: Option<Value>,
    web_search: bool,
) -> Result<Response, AiError> {
    let mut payload = json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": 0.7,
        "stream": true,
    });
    if web_search {
        payload["tools"] = web_search_tools();
    }

    let response = http
        .post(OPENROUTER_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("HTTP-Referer", "https://study-platform.local")
        .header("X-Title", "Study Platform")
        .json(&payload)
        .send()
        .await
        .map_err(|e| AiError::Http(e.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        return Err(map_upstream_status(status, &body_text));
    }

    // The 30s total timeout of the buffered proxy would kill a long-lived
    // SSE body; the pump owns its own idle/total timeouts instead.
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(CHANNEL_BUFFER);
    tokio::spawn(pump_stream(response, tx, turn, initial_context));

    sse_response(Body::new(SseBody { rx }))
}

/// Map a failed pre-stream upstream response to [`AiError`], mirroring the
/// buffered proxy in [`crate::ai::openrouter::chat_completion`].
fn map_upstream_status(status: StatusCode, body: &str) -> AiError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        AiError::Api("Rate limited".to_string())
    } else {
        // Char-boundary-safe truncation of the provider body (byte slicing
        // could panic on UTF-8 boundaries).
        let snippet: String = body.chars().take(200).collect();
        AiError::Api(format!("HTTP {status}: {snippet}"))
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `POST /api/ai/chat/stream` — stream a chat completion as SSE.
///
/// Route contract for the orchestrator (NOT registered here): wire as
/// `axum::routing::post(streaming::chat_stream_handler)` inside the
/// existing auth-protected layer of [`crate::ai::register_routes`].
///
/// Request body: JSON `{ "message": "...", "content": "..."?, "subject": "..."? }`
/// — `message` is required (1..=8000 chars), `content` optional
/// (≤ 20000 chars), `subject` optional (defaults to `general`).
/// A missing/oversized input is rejected with HTTP 400 and
/// `{ "error": "..." }`.
///
/// Without an API key the reply is a single `done` frame stating that
/// streaming is unavailable (no fabricated content). With an API key the
/// provider response is relayed as `delta`/`reasoning` frames followed by
/// the terminal `done` frame.
pub async fn chat_stream_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Value>,
) -> Response {
    // --- validation ------------------------------------------------------
    let Some(message) = req.get("message").and_then(Value::as_str) else {
        debug!("chat/stream rejected: missing message");
        return bad_request("Invalid request: message is required.");
    };
    if message.trim().is_empty() {
        debug!("chat/stream rejected: empty message");
        return bad_request("Invalid request: message must not be empty.");
    }
    if message.chars().count() > MAX_MESSAGE_CHARS {
        debug!("chat/stream rejected: message too long");
        return bad_request(&format!(
            "Invalid request: message must be at most {MAX_MESSAGE_CHARS} characters."
        ));
    }

    let content = req
        .get("content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|c| !c.is_empty());
    if let Some(content) = content {
        if content.chars().count() > MAX_CONTENT_CHARS {
            debug!("chat/stream rejected: content too long");
            return bad_request(&format!(
                "Invalid request: content must be at most {MAX_CONTENT_CHARS} characters."
            ));
        }
    }

    // The subject only shapes the system prompt: truncated (never rejected)
    // so the input contract stays exactly `message` + `content`.
    let subject = req
        .get("subject")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("general");
    let subject: String = subject.chars().take(MAX_SUBJECT_CHARS).collect();

    let mode_requested = req.get("mode").is_some();
    let mode = match req.get("mode") {
        None => RetrievalMode::General,
        Some(value) => match serde_json::from_value::<RetrievalMode>(value.clone()) {
            Ok(mode) => mode,
            Err(_) => {
                return bad_request("Invalid request: mode must be hybrid, notes or general.");
            }
        },
    };
    let prepared = match crate::ai::rag::prepare(state.clone(), message, mode).await {
        Ok(context) => context,
        Err(error) => {
            warn!(detail = %error.log_detail(), "chat/stream: context retrieval failed");
            return error_response(&error);
        }
    };

    let stream_id = ChatStreamId::generate();

    // The turn to store when the stream ends. The relay task owns a clone
    // of it (it needs the SQLite pool); the mock path stores it inline.
    let turn = StreamTurn {
        state: state.clone(),
        subject: subject.clone(),
        user_message: message.to_string(),
    };

    // --- mock mode (no API key) ------------------------------------------
    let api_key = state
        .config
        .openrouter_api_key
        .as_deref()
        .filter(|k| !k.is_empty());
    let Some(api_key) = api_key else {
        debug!(stream_id = %stream_id, "chat/stream: mock mode (no API key)");
        // The mock turn is a real turn too: it is stored like any other
        // one, so the dashboard is not empty after a reload.
        persist_turn(&turn, MOCK_STREAM_MESSAGE, "", &Usage::default(), true).await;
        return if mode_requested {
            mock_stream_response_with_context(&stream_id, prepared.manifest())
        } else {
            mock_stream_response(&stream_id)
        };
    };

    // --- live mode --------------------------------------------------------
    // La misma composición alimenta buffered y SSE, incluidas las notas.
    let messages = serde_json::to_value(prepared.messages(message, &subject, content))
        .unwrap_or_else(|_| Value::Array(Vec::new()));

    // El modelo puede haberlo cambiado el usuario en la interfaz (se guarda
    // en `app_config`); el del `.env` es sólo el valor por defecto.
    let model = crate::ai::openrouter::modelo_configurado(&state.db, &state.config).await;
    let max_tokens = u64::from(state.config.openrouter_max_tokens);

    // Long-lived streams own their own timeouts (see `stream_chat`), so the
    // client is built without a total timeout; only connection setup is
    // capped. Mirrors the per-request client pattern of the buffered proxy.
    let http = match reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            let err = AiError::Http(e.to_string());
            warn!(stream_id = %stream_id, detail = %err.log_detail(), "chat/stream: client build failed");
            return error_response(&err);
        }
    };

    debug!(stream_id = %stream_id, "chat/stream: live mode");
    let result = if mode_requested {
        stream_chat_grounded(
            http,
            api_key,
            &model,
            messages,
            max_tokens,
            turn,
            Some(prepared.manifest()),
            prepared.web_enabled(),
        )
        .await
    } else {
        stream_chat(http, api_key, &model, messages, max_tokens, turn).await
    };
    match result {
        Ok(resp) => with_stream_id(resp, stream_id.as_str()),
        Err(e) => {
            warn!(stream_id = %stream_id, detail = %e.log_detail(), "chat/stream: request failed");
            error_response(&e)
        }
    }
}

/// Mock-mode reply: a single `done` frame stating that streaming needs an
/// API key. No content is fabricated and no usage is invented.
fn mock_stream_response(stream_id: &ChatStreamId) -> Response {
    mock_stream_response_inner(stream_id, None)
}

fn mock_stream_response_with_context(stream_id: &ChatStreamId, context: Value) -> Response {
    mock_stream_response_inner(stream_id, Some(context))
}

fn mock_stream_response_inner(stream_id: &ChatStreamId, context: Option<Value>) -> Response {
    let mut frame = String::new();
    if let Some(context) = context {
        if let Some(initial) = sse_frame("context", &context) {
            frame.push_str(&initial);
        }
    }
    let payload = json!({ "content": MOCK_STREAM_MESSAGE });
    let done = sse_frame("done", &payload).unwrap_or_else(|| {
        "event: error\ndata: {\"error\":\"AI response could not be processed. Please try again later.\"}\n\n"
            .to_string()
    });
    frame.push_str(&done);
    match sse_response(Body::from(frame)) {
        Ok(resp) => with_stream_id(resp, stream_id.as_str()),
        Err(e) => error_response(&e),
    }
}

// ---------------------------------------------------------------------------
// Persistence (`chat_history` + `usage_metrics`)
// ---------------------------------------------------------------------------

/// The exact statement the buffered chat path uses to store a turn
/// (`crate::ai::chat::AiAssistant::save_conversation`).
///
/// `chat_history` has no token/cost columns and `created_at` defaults to
/// `datetime('now')`, so the streaming path stores the same three columns.
const CHAT_HISTORY_INSERT: &str =
    "INSERT INTO chat_history (subject, role, content) VALUES (?, ?, ?)";

/// The statement that records the usage of one call (`usage_metrics`,
/// same columns as the buffered path).
const USAGE_METRICS_INSERT: &str =
    "INSERT INTO usage_metrics (model, input_tokens, output_tokens, cost_usd, estimated, endpoint) \
     VALUES (?, ?, ?, ?, ?, ?)";

/// Endpoint label of the usage row: the streaming route is the chat API.
const USAGE_ENDPOINT: &str = "chat";

/// Heading that separates the model's reasoning from its answer inside
/// the stored `assistant` message.
const REASONING_HEADING: &str = "[Razonamiento]";

/// One row of `chat_history`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HistoryRow {
    subject: String,
    role: String,
    content: String,
}

/// Build the `chat_history` rows of one turn: the user's message and the
/// accumulated assistant answer.
///
/// Mirrors the buffered path: one `user` row with the question and one
/// `assistant` row with the answer. The `assistant` row is omitted when
/// the turn produced no text at all, or when the provider failed
/// mid-stream (there is no answer to store).
fn build_history_rows(
    subject: &str,
    user_message: &str,
    content: &str,
    reasoning: &str,
    completed: bool,
) -> Vec<HistoryRow> {
    let mut rows = Vec::with_capacity(2);
    rows.push(HistoryRow {
        subject: subject.to_string(),
        role: "user".to_string(),
        content: user_message.to_string(),
    });
    let answer = compose_assistant_text(content, reasoning);
    if completed && !answer.is_empty() {
        rows.push(HistoryRow {
            subject: subject.to_string(),
            role: "assistant".to_string(),
            content: answer,
        });
    }
    rows
}

/// The assistant's answer as it is stored: the streamed text, preceded by
/// the model's reasoning when it emitted one.
fn compose_assistant_text(content: &str, reasoning: &str) -> String {
    let content = content.trim();
    let reasoning = reasoning.trim();
    match (content.is_empty(), reasoning.is_empty()) {
        (true, true) => String::new(),
        (false, true) => content.to_string(),
        (true, false) => format!("{REASONING_HEADING}\n{reasoning}"),
        (false, false) => format!("{REASONING_HEADING}\n{reasoning}\n\n{content}"),
    }
}

/// Build the `usage_metrics` row of one call: the real token counts from
/// the provider's `usage` object and a 0.0 USD cost.
///
/// The streaming `usage` object carries no trustworthy cost (the free
/// models used here report none), so the row is always 0.0 and marked
/// `estimated` — the same convention the buffered path uses when the
/// provider reports no cost.
fn build_usage_entry(model: &str, usage: &Usage) -> UsageEntry {
    UsageEntry {
        model: model.to_string(),
        input_tokens: usage.input.min(i64::MAX as u64) as i64,
        output_tokens: usage.output.min(i64::MAX as u64) as i64,
        cost_usd: 0.0,
        estimated: true,
        endpoint: USAGE_ENDPOINT.to_string(),
    }
}

/// Persist one streamed turn: the user's message and the accumulated
/// assistant answer in `chat_history`, plus the usage row in
/// `usage_metrics`.
///
/// Best-effort by design — a failing INSERT is logged and swallowed: the
/// frames already emitted to the client must never be invalidated by a
/// database error.
async fn persist_turn(
    turn: &StreamTurn,
    content: &str,
    reasoning: &str,
    usage: &Usage,
    completed: bool,
) {
    for row in build_history_rows(
        &turn.subject,
        &turn.user_message,
        content,
        reasoning,
        completed,
    ) {
        if let Err(e) = sqlx::query(CHAT_HISTORY_INSERT)
            .bind(&row.subject)
            .bind(&row.role)
            .bind(&row.content)
            .execute(&turn.state.db)
            .await
        {
            warn!(error = %e, "chat/stream: could not store the turn in chat_history");
        }
    }

    if let Err(e) = ensure_usage_metrics_table(&turn.state.db).await {
        warn!(detail = %e.log_detail(), "chat/stream: usage_metrics is not available");
        return;
    }
    let entry = build_usage_entry(&turn.state.config.openrouter_model, usage);
    if let Err(e) = sqlx::query(USAGE_METRICS_INSERT)
        .bind(&entry.model)
        .bind(entry.input_tokens)
        .bind(entry.output_tokens)
        .bind(entry.cost_usd)
        .bind(i64::from(entry.estimated))
        .bind(&entry.endpoint)
        .execute(&turn.state.db)
        .await
    {
        warn!(error = %e, "chat/stream: could not store the usage row");
    }
}

// ---------------------------------------------------------------------------
// Unit tests (pure; no network, no live provider calls)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_frame_format() {
        let frame = sse_frame("delta", &json!({ "content": "Hola" }));
        assert_eq!(
            frame,
            Some("event: delta\ndata: {\"content\":\"Hola\"}\n\n".to_string())
        );
    }

    #[test]
    fn test_sse_frame_is_one_data_field() {
        // serde_json escapes "\n" inside the payload, so the frame keeps a
        // single `data:` field and ends with the blank-line delimiter.
        let frame = sse_frame("delta", &json!({ "content": "a\nb" })).expect("serializable");
        assert!(frame.ends_with("\n\n"));
        assert_eq!(frame.lines().filter(|l| l.starts_with("data: ")).count(), 1);
    }

    #[test]
    fn test_usage_payload_shape() {
        let usage = Usage {
            input: 12,
            output: 7,
            cost: 0.0,
        };
        let payload = usage.payload();
        assert_eq!(payload["usage"]["input"].as_u64(), Some(12));
        assert_eq!(payload["usage"]["output"].as_u64(), Some(7));
        assert_eq!(payload["usage"]["cost"].as_f64(), Some(0.0));
    }

    #[test]
    fn test_usage_capture_ignores_missing_fields() {
        let mut usage = Usage::default();
        usage.capture(&json!({ "prompt_tokens": 5 }));
        assert_eq!(usage.input, 5);
        assert_eq!(usage.output, 0);
        assert_eq!(usage.cost, 0.0);
    }

    #[test]
    fn test_mock_frame_is_the_documented_one() {
        let payload = json!({ "content": MOCK_STREAM_MESSAGE });
        let frame = sse_frame("done", &payload);
        assert_eq!(
            frame,
            Some(
                "event: done\ndata: {\"content\":\"[Modo simulado] streaming no disponible sin API key\"}\n\n"
                    .to_string()
            )
        );
    }

    #[tokio::test]
    async fn test_web_annotations_emit_one_deduplicated_sources_frame() {
        let (mut tx, mut rx) = tokio::sync::mpsc::channel(4);
        let mut usage = Usage::default();
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut seen = HashSet::new();
        let chunk = json!({
            "choices": [{
                "delta": {
                    "content": "Respuesta",
                    "annotations": [
                        {"type":"url_citation","url_citation":{
                            "url":"https://example.com/fuente","title":"Fuente"
                        }},
                        {"type":"url_citation","url_citation":{
                            "url":"https://example.com/fuente","title":"Fuente"
                        }}
                    ]
                }
            }]
        });
        assert_eq!(
            relay_chunk(
                &mut tx,
                &mut usage,
                &mut content,
                &mut reasoning,
                &mut seen,
                &chunk,
            )
            .await,
            Dispatch::Continue
        );
        drop(tx);
        let mut frames = Vec::new();
        while let Some(frame) = rx.recv().await {
            frames.push(frame);
        }
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame.starts_with("event: sources"))
                .count(),
            1
        );
        assert!(frames.iter().any(|frame| frame.starts_with("event: delta")));
        assert!(frames[0].contains("https://example.com/fuente"));
        assert_eq!(content, "Respuesta");
    }

    #[test]
    fn test_map_upstream_status() {
        assert!(matches!(
            map_upstream_status(StatusCode::TOO_MANY_REQUESTS, "body"),
            AiError::Api(_)
        ));
        let e = map_upstream_status(StatusCode::UNAUTHORIZED, "Key invalid");
        assert_eq!(e.status_code(), StatusCode::BAD_GATEWAY);
        // Provider internals never reach the client.
        assert!(!e.client_message().contains("Key invalid"));
    }

    #[test]
    fn test_chat_stream_id_roundtrip() {
        let id = ChatStreamId::generate();
        assert_eq!(id.as_str().len(), 32);
        assert_eq!(id.to_string(), format!("{id}"));
        assert_ne!(id, ChatStreamId::generate());
    }

    #[test]
    fn test_build_history_rows_matches_the_buffered_insert() {
        let rows = build_history_rows(
            "biologia",
            "¿Qué es la mitocondria?",
            "Es la central energética de la célula.",
            "El usuario pregunta por la mitocondria.",
            true,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].role, "user");
        assert_eq!(rows[0].subject, "biologia");
        assert_eq!(rows[0].content, "¿Qué es la mitocondria?");
        assert_eq!(rows[1].role, "assistant");
        assert_eq!(rows[1].subject, "biologia");
        assert!(rows[1]
            .content
            .contains("Es la central energética de la célula."));
        assert!(rows[1]
            .content
            .contains("El usuario pregunta por la mitocondria."));
        // The stored statement is the one the buffered path uses.
        assert_eq!(
            CHAT_HISTORY_INSERT,
            "INSERT INTO chat_history (subject, role, content) VALUES (?, ?, ?)"
        );
    }

    #[test]
    fn test_build_history_rows_without_an_answer() {
        // The provider failed mid-stream: the question is stored, the
        // (missing) answer is not.
        let rows = build_history_rows("biologia", "pregunta", "", "", false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].role, "user");
    }

    #[test]
    fn test_build_usage_entry_keeps_zero_cost_and_real_tokens() {
        let usage = Usage {
            input: 12,
            output: 7,
            cost: 0.0,
        };
        let entry = build_usage_entry("cohere/north-mini-code:free", &usage);
        assert_eq!(entry.input_tokens, 12);
        assert_eq!(entry.output_tokens, 7);
        assert_eq!(entry.cost_usd, 0.0);
        assert!(entry.estimated);
        assert_eq!(entry.endpoint, "chat");
    }
}
