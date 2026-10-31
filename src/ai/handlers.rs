//! HTTP handlers for the AI assistant.

//!

//! All API endpoints (`/api/ai/*`) AND the UI page (`GET /ai`) require

//! DB-backed session authentication via `require_auth_state` (Mission 11:

//! the API was previously public, which exposed paid LLM usage to

//! unauthenticated callers).

//!

//! Uses mock mode when `OPENROUTER_API_KEY` is not configured — no secrets

//! are ever hardcoded; the API key is read from the environment via `Config`.

//!

//! Hard monthly budget: before any provider call the handlers compare the

//! month's persisted spend (`usage_metrics`) against

//! `config.openrouter_max_cost_usd`; when the cap is reached they answer

//! HTTP 429 with `{ "error": "Presupuesto de IA agotado para este mes",

//! "budget_usd": X, "spent_usd": Y }` and never reach OpenRouter. Demo mode

//! (no API key) is not blocked by this gate — mock calls cost 0.0.

//!

//! Input validation: `message` ≤ 8000 chars, `content` ≤ 20000 chars,

//! `subject` ≤ 200 chars, `num_questions` clamped to 1..=10. Violations

//! return 400 with generic messages. Internal errors are mapped to generic

//! client messages (see `AiError::client_message`); details go to `tracing`

//! with input lengths only — never user content, credentials, or tokens.

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    Json,
};

use serde::Deserialize;

use std::sync::Arc;

use crate::ai::rag::RetrievalMode;
use crate::ai::{AiAssistant, AiError, CostSummary};

use crate::AppState;

/// Generic public message for 500-class failures (never leaks internals).

const INTERNAL_ERROR: &str = "Internal error";

// ---------------------------------------------------------------------------

// Request bodies

// ---------------------------------------------------------------------------

/// Body for `POST /api/ai/chat`.

#[derive(Debug, Deserialize)]

pub struct ChatRequest {
    pub message: String,

    pub subject: String,

    /// Ausente = general, para conservar el comportamiento de clientes previos.
    #[serde(default)]
    pub mode: Option<RetrievalMode>,
}

/// Body for `POST /api/ai/summary`.

#[derive(Debug, Deserialize)]

pub struct SummaryRequest {
    pub content: String,
}

/// Body for `POST /api/ai/quiz`.

#[derive(Debug, Deserialize)]

pub struct QuizRequest {
    pub content: String,

    #[serde(default = "default_num_questions")]
    pub num_questions: usize,
}

fn default_num_questions() -> usize {
    5
}

/// Body for `POST /api/ai/study-sheet`.

#[derive(Debug, Deserialize)]

pub struct StudySheetRequest {
    pub content: String,
}

// ---------------------------------------------------------------------------

// Input validation + safe error mapping

// ---------------------------------------------------------------------------

/// Maximum `message` length (chars) for `POST /api/ai/chat`.

pub const MAX_MESSAGE_CHARS: usize = 8000;

/// Maximum `content` length (chars) for summary/quiz/study-sheet endpoints.

pub const MAX_CONTENT_CHARS: usize = 20000;

/// Maximum `subject` length (chars) for `POST /api/ai/chat`.

pub const MAX_SUBJECT_CHARS: usize = 200;

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Validate chat inputs. Returns the effective subject (`general` when the

/// caller sends an empty one) or a 400 message.

fn validate_chat(message: &str, subject: &str) -> Result<String, (StatusCode, String)> {
    if message.trim().is_empty() || char_len(message) > MAX_MESSAGE_CHARS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Invalid request: message must be 1..={} characters.",
                MAX_MESSAGE_CHARS
            ),
        ));
    }

    if char_len(subject) > MAX_SUBJECT_CHARS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Invalid request: subject must be at most {} characters.",
                MAX_SUBJECT_CHARS
            ),
        ));
    }

    let subject = subject.trim();

    Ok(if subject.is_empty() {
        "general".to_string()
    } else {
        subject.to_string()
    })
}

/// Validate `content` inputs for summary/quiz/study-sheet endpoints.

fn validate_content(content: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if content.trim().is_empty() || char_len(content) > MAX_CONTENT_CHARS {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({

                "error": format!(

                    "Invalid request: content must be 1..={} characters.",

                    MAX_CONTENT_CHARS

                )

            })),
        ));
    }

    Ok(())
}

/// Map an internal AI error to a safe response (chat endpoint).
///
/// Logs the failure with input lengths only — never user content.
/// Budget exhaustion answers with the shared JSON budget body.
fn chat_error(endpoint: &'static str, input_len: usize, e: AiError) -> Response {
    tracing::warn!(

        endpoint = endpoint,

        input_len = input_len,

        detail = %e.log_detail(),

        "ai request failed"

    );

    match e {
        AiError::BudgetExceeded { spent, budget } => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(budget_exhausted_body(spent, budget)),
        )
            .into_response(),
        other => (other.status_code(), other.client_message()).into_response(),
    }
}

/// Map an internal AI error to a safe JSON response. Logs with lengths only.
fn json_error(
    endpoint: &'static str,

    input_len: usize,

    e: AiError,
) -> (StatusCode, Json<serde_json::Value>) {
    tracing::warn!(

        endpoint = endpoint,

        input_len = input_len,

        detail = %e.log_detail(),

        "ai request failed"

    );

    match e {
        AiError::BudgetExceeded { spent, budget } => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(budget_exhausted_body(spent, budget)),
        ),
        other => (
            other.status_code(),
            Json(serde_json::json!({ "error": other.client_message() })),
        ),
    }
}

// ---------------------------------------------------------------------------
// Hard monthly budget gate
// ---------------------------------------------------------------------------

/// JSON body returned when the month's AI spend already reached the cap.
///
/// Identical for every endpoint so clients can parse it uniformly.
fn budget_exhausted_body(spent_usd: f64, budget_usd: f64) -> serde_json::Value {
    serde_json::json!({
        "error": "Presupuesto de IA agotado para este mes",
        "budget_usd": budget_usd,
        "spent_usd": spent_usd,
    })
}

/// Reject the request BEFORE any provider call when the month's persisted
/// spend already reached `config.openrouter_max_cost_usd`.
///
/// Demo mode (no `OPENROUTER_API_KEY`) is never blocked here: mock calls
/// cost 0.0, so there is no provider spend to protect. The provider-side
/// gate in `ai::openrouter` still applies to both modes (fail-closed).
///
/// A DB failure does not fabricate a budget decision: the request proceeds
/// and that same gate (same table) decides.
async fn enforce_budget(
    state: &Arc<AppState>,

    endpoint: &'static str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if state.config.openrouter_api_key.is_none() {
        return Ok(());
    }

    let budget_usd = state.config.openrouter_max_cost_usd;

    let spent_usd = match crate::ai::openrouter::persistent_summary(&state.db, &state.config).await
    {
        Ok(summary) => summary.total_cost,
        Err(e) => {
            tracing::warn!(

                endpoint = endpoint,

                detail = %e.log_detail(),

                "ai budget check failed; deferring to the provider-side gate"

            );

            return Ok(());
        }
    };

    if spent_usd >= budget_usd {
        tracing::warn!(
            endpoint = endpoint,
            budget_usd = budget_usd,
            spent_usd = spent_usd,
            "ai request blocked: monthly budget exhausted"
        );

        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(budget_exhausted_body(spent_usd, budget_usd)),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------

// API handlers (auth-protected via `require_auth` in `register_routes`)

// ---------------------------------------------------------------------------

/// `POST /api/ai/chat` — chat with the AI assistant.

///

/// Accepts JSON `{ "message": "...", "subject": "..." }` and returns the

/// AI response as `text/plain`. In mock mode the response begins with

/// `[Modo simulado]`. Budget exhaustion answers HTTP 429 with JSON.

pub async fn chat_handler(
    State(state): State<Arc<AppState>>,

    headers: HeaderMap,

    Json(payload): Json<ChatRequest>,
) -> Response {
    let input_len = char_len(&payload.message);

    let subject = match validate_chat(&payload.message, &payload.subject) {
        Ok(s) => s,

        Err((status, message)) => return (status, message).into_response(),
    };

    // Hard budget gate — before any provider call.
    if let Err(budget) = enforce_budget(&state, "chat").await {
        return budget.into_response();
    }

    let mode = payload.mode.unwrap_or_default();
    let prepared = match crate::ai::rag::prepare(state.clone(), &payload.message, mode).await {
        Ok(context) => context,
        Err(error) => return chat_error("chat/context", input_len, error),
    };
    let assistant = AiAssistant::new(state);

    match assistant
        .chat_grounded(&payload.message, &subject, prepared)
        .await
    {
        Ok(answer) => {
            let wants_json = headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("application/json"));
            if wants_json {
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "content": answer.content,
                        "mode": answer.mode,
                        "sources": answer.sources,
                    })),
                )
                    .into_response()
            } else {
                (StatusCode::OK, answer.content).into_response()
            }
        }

        Err(e) => chat_error("chat", input_len, e),
    }
}

/// `POST /api/ai/summary` — generate a summary from academic text.

///

/// Accepts JSON `{ "content": "..." }` and returns

/// `{ "summary": "..." }`.

pub async fn summarize_handler(
    State(state): State<Arc<AppState>>,

    Json(payload): Json<SummaryRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Err(e) = validate_content(&payload.content) {
        return e;
    }

    let input_len = char_len(&payload.content);

    // Hard budget gate — before any provider call.
    if let Err(budget) = enforce_budget(&state, "summary").await {
        return budget;
    }

    let max_tokens = state.config.openrouter_max_tokens;

    let assistant = AiAssistant::new(state);

    match assistant.summarize(&payload.content, max_tokens).await {
        Ok(response) => (
            StatusCode::OK,
            Json(serde_json::json!({ "summary": response })),
        ),

        Err(e) => json_error("summary", input_len, e),
    }
}

/// `POST /api/ai/quiz` — generate a multiple-choice quiz.

///

/// Accepts JSON `{ "content": "...", "num_questions": 5 }`.

/// Returns the `Quiz` struct as JSON.

pub async fn quiz_handler(
    State(state): State<Arc<AppState>>,

    Json(payload): Json<QuizRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Err(e) = validate_content(&payload.content) {
        return e;
    }

    let input_len = char_len(&payload.content);

    // Clamp into 1..=10 (`generate_quiz` clamps again defensively).

    let num_questions = payload.num_questions.clamp(1, 10);

    // Hard budget gate — before any provider call.
    if let Err(budget) = enforce_budget(&state, "quiz").await {
        return budget;
    }

    let assistant = AiAssistant::new(state);

    match assistant
        .generate_quiz(&payload.content, num_questions)
        .await
    {
        Ok(quiz) => (
            StatusCode::OK,
            Json(
                serde_json::to_value(quiz)
                    .unwrap_or_else(|_| serde_json::json!({ "error": "Failed to serialize quiz" })),
            ),
        ),

        Err(e) => json_error("quiz", input_len, e),
    }
}

/// `POST /api/ai/study-sheet` — generate a Markdown study sheet.

///

/// Accepts JSON `{ "content": "..." }` and returns

/// `{ "study_sheet": "..." }`.

pub async fn study_sheet_handler(
    State(state): State<Arc<AppState>>,

    Json(payload): Json<StudySheetRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Err(e) = validate_content(&payload.content) {
        return e;
    }

    let input_len = char_len(&payload.content);

    // Hard budget gate — before any provider call.
    if let Err(budget) = enforce_budget(&state, "study-sheet").await {
        return budget;
    }

    let assistant = AiAssistant::new(state);

    match assistant.generate_study_sheet(&payload.content).await {
        Ok(response) => (
            StatusCode::OK,
            Json(serde_json::json!({ "study_sheet": response })),
        ),

        Err(e) => json_error("study-sheet", input_len, e),
    }
}

/// `GET /api/ai/cost` — current month's cost summary (auth-protected).

///

/// Returns `{ "total_cost": f64, "budget_limit": f64, "requests_count": u64,

/// "alert": bool }` where `alert` is true at ≥90% of budget. Spend is read

/// from the persistent `usage_metrics` table, so it survives restarts.

pub async fn cost_handler(State(state): State<Arc<AppState>>) -> Json<CostSummary> {
    let assistant = AiAssistant::new(state);

    Json(assistant.cost_tracking().await)
}

// ---------------------------------------------------------------------------

// Conversation history (read-only views over the existing `chat_history`

// table that `AiAssistant::chat` already persists every turn to).

// ---------------------------------------------------------------------------

/// `GET /api/ai/conversations` — conversation sessions (distinct subjects),

/// most-recent first, with a preview of the last assistant message + counts.

/// Auth-protected. Does not change the `/api/ai/chat` contract.

pub async fn conversations_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    #[derive(sqlx::FromRow)]

    struct SubjectRow {
        subject: String,
    }

    let subjects: Vec<SubjectRow> =

        sqlx::query_as("SELECT subject FROM chat_history WHERE role = 'assistant' GROUP BY subject ORDER BY MAX(created_at) DESC LIMIT 50")

            .fetch_all(&state.db)

            .await

            .unwrap_or_default();

    let mut conversations: Vec<serde_json::Value> = Vec::new();

    for s in subjects {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chat_history WHERE subject = ? AND role IN ('user','assistant')",
        )
        .bind(&s.subject)
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

        let last_preview: Option<String> = sqlx::query_scalar(

            "SELECT content FROM chat_history WHERE subject = ? AND role = 'assistant' ORDER BY created_at DESC LIMIT 1",

        )

        .bind(&s.subject)

        .fetch_all(&state.db)

        .await

        .ok()

        .and_then(|v: Vec<String>| v.into_iter().next());

        let updated: Option<String> = sqlx::query_scalar(

            "SELECT created_at FROM chat_history WHERE subject = ? AND role = 'assistant' ORDER BY created_at DESC LIMIT 1",

        )

        .bind(&s.subject)

        .fetch_all(&state.db)

        .await

        .ok()

        .and_then(|v: Vec<String>| v.into_iter().next());

        let preview = last_preview.unwrap_or_default();

        conversations.push(serde_json::json!({

            "subject": s.subject,

            "last_message": preview,

            "message_count": count,

            "updated_at": updated,

        }));
    }

    Json(serde_json::json!({ "conversations": conversations }))
}

/// `GET /api/ai/conversations/{subject}/messages` — ordered message history for

/// a single conversation session (filters out the internal system prompt).

pub async fn conversation_messages_handler(
    State(state): State<Arc<AppState>>,

    Path(subject): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !is_safe_subject(&subject) {
        return Err((StatusCode::BAD_REQUEST, "Invalid subject".to_string()));
    }

    #[derive(sqlx::FromRow)]

    struct Msg {
        role: String,

        content: String,

        created_at: String,
    }

    let rows: Vec<Msg> = sqlx::query_as(
        "SELECT role, content, created_at FROM chat_history \

         WHERE subject = ? AND role IN ('user','assistant') ORDER BY created_at ASC",
    )
    .bind(&subject)
    .fetch_all(&state.db)
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            INTERNAL_ERROR.to_string(),
        )
    })?;

    let messages: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|m| {
            serde_json::json!({

                "role": m.role,

                "content": m.content,

                "created_at": m.created_at,

            })
        })
        .collect();

    Ok(Json(
        serde_json::json!({ "subject": subject, "messages": messages }),
    ))
}

/// Validate a path-bound subject (no path separators / traversal).

fn is_safe_subject(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.contains('/')
        && !s.contains('\\')
        && s != "."
        && s != ".."
}

// ---------------------------------------------------------------------------

// UI handler (auth-protected)

// ---------------------------------------------------------------------------

/// `GET /ai` — AI chat UI page (Stitch "Synthetic Cognition" design system).

///

/// Requires authentication. Renders a three-panel chat interface built on the

/// shared Stitch shell (nav-rail + top-bar):

/// - Left: conversation sessions (loaded from `/api/ai/conversations`)

/// - Center: chat messages + composer with action toolbar

/// - Right: sources/context rail (empty until backend provides)

pub async fn ai_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // El modelo mostrado es el activo: el que el usuario eligió en la
    // interfaz (`app_config`) o, si no, el del `.env`.
    let model = crate::ai::openrouter::modelo_configurado(&state.db, &state.config).await;
    let model = model.as_str();

    let mock_mode = state.config.openrouter_api_key.is_none();

    let (mode_label, mode_color, mode_dot) = if mock_mode {
        ("Modo simulado", "text-yellow-400", "bg-yellow-400")
    } else {
        ("Conectado", "text-green-400", "bg-green-400")
    };

    let body_html = AI_PAGE_TEMPLATE
        .replace("__MODEL__", model)
        .replace("__MODE_COLOR__", mode_color)
        .replace("__MODE_LABEL__", mode_label)
        .replace("__MODE_DOT__", mode_dot);

    let html = crate::academic::templates::page_ext(
        "Asistente Académico — Study Platform",
        "Asistente",
        "/ai",
        &body_html,
    );

    // `page_ext` no rellena los slots del shell (nombre, estado, badge MCP):
    // la versión stitchada (/dashboard/ai) lo hace vía `stitch_chat::page`;
    // aquí reutilizamos el mismo llenado para no servir placeholders.
    let shell = crate::handlers::stitch_panel::fetch_shell_data(&state).await;
    Html(crate::handlers::stitch_panel::fill_shell(&html, &shell))
}

/// HTML template for the AI chat UI page (Stitch "Synthetic Cognition" design system).

/// Rendered inside the shared Stitch shell (nav-rail + top-bar) via `page_ext`;

/// this const holds ONLY the main slot content so all screens share the same shell.

const AI_PAGE_TEMPLATE: &str = r##"
<div class="nx-chat nova-page nova-chat-page nova-conversations-page">
  <button class="nx-chat-scrim" type="button" aria-label="Cerrar paneles laterales"></button>
  <aside class="nx-chat-history" id="chat-history" aria-label="Historial de conversaciones">
    <header class="nx-chat-history-head"><div><p class="nx-eyebrow">Asistente</p><h1>Conversaciones</h1></div><button class="nx-icon-button nx-chat-mobile-close" id="close-history-btn" type="button" aria-label="Cerrar historial"><span class="material-symbols-outlined">close</span></button></header>
    <button id="btn-new-chat" class="nx-button nx-button-primary nx-new-chat" type="button"><span class="material-symbols-outlined">add</span>Nueva conversación</button>
    <label class="nx-chat-search" for="session-search"><span class="material-symbols-outlined">search</span><input id="session-search" placeholder="Buscar conversación…" type="search"></label>
    <div id="conversations-list" class="nx-chat-sessions"><div class="nx-loading-copy">Cargando conversaciones…</div></div>
    <div class="nx-chat-quota"><div><span><i class="__MODE_DOT__"></i><button type="button" id="modelo-chip" data-sp-modelo title="Cambiar de modelo de IA"><span data-sp-modelo-nombre>__MODEL__</span><span class="material-symbols-outlined">expand_more</span></button></span><strong id="mode-badge" class="__MODE_COLOR__">__MODE_LABEL__</strong></div><div id="cost-display"><span>Cargando coste…</span></div></div>
  </aside>
  <section class="nx-conversation" aria-label="Conversación actual">
    <header class="nx-conversation-head">
      <div class="nx-conversation-title"><button class="nx-icon-button nx-chat-history-toggle" id="toggle-history-btn" type="button" aria-label="Abrir historial" aria-controls="chat-history" aria-expanded="false"><span class="material-symbols-outlined">menu</span></button><span class="nx-conversation-mark material-symbols-outlined">forum</span><div><span>Conversación actual</span><div class="nx-chat-context"><div id="active-subject" class="nx-context-subject">general</div></div></div></div>
      <div class="nx-conversation-actions"><button class="nx-button nx-button-secondary" id="toggle-sources-btn" type="button"><span class="material-symbols-outlined">menu_book</span><span>Fuentes</span></button><button id="btn-clear-chat" class="nx-icon-button" type="button" title="Limpiar pantalla" aria-label="Limpiar conversación"><span class="material-symbols-outlined">restart_alt</span></button></div>
    </header>
    <div id="chat-messages" class="nx-chat-messages"><div class="nx-chat-welcome" data-chat-empty="1"><div class="nx-chat-welcome-badge"><span class="material-symbols-outlined">forum</span>Tu espacio de estudio</div><h2>Continúa una idea.</h2><p>Elige una conversación o escribe una pregunta nueva. También puedes empezar por aquí:</p><div class="nx-chat-starters"><button type="button" data-chat-prompt="Explícame este tema paso a paso y con un ejemplo: "><span class="material-symbols-outlined">lightbulb</span><span><strong>Entender un tema</strong><small>Explicación clara y con ejemplos</small></span><span class="material-symbols-outlined">north_east</span></button><button type="button" data-chat-prompt="Resume estos apuntes en ideas clave: "><span class="material-symbols-outlined">summarize</span><span><strong>Resumir apuntes</strong><small>Lo esencial, sin rodeos</small></span><span class="material-symbols-outlined">north_east</span></button><button type="button" data-chat-prompt="Crea un cuestionario de 5 preguntas sobre: "><span class="material-symbols-outlined">quiz</span><span><strong>Practicar</strong><small>Un cuestionario para repasar</small></span><span class="material-symbols-outlined">north_east</span></button></div></div></div>
    <footer class="nx-composer">
      <div class="nx-composer-tools" aria-label="Acciones de estudio"><button id="btn-summarize" class="nx-chat-tool" type="button"><span class="material-symbols-outlined">summarize</span>Resumir</button><button id="btn-explain" class="nx-chat-tool" type="button"><span class="material-symbols-outlined">search_insights</span>Explicar a fondo</button><button id="btn-quiz" class="nx-chat-tool" type="button"><span class="material-symbols-outlined">quiz</span>Cuestionario</button><button id="btn-study-sheet" class="nx-chat-tool" type="button"><span class="material-symbols-outlined">style</span>Ficha de estudio</button></div>
      <div class="nx-composer-box"><label for="prompt-input" class="sr-only">Mensaje para el asistente</label><textarea id="prompt-input" placeholder="Pregunta, pega tus apuntes o describe lo que necesitas…" rows="2"></textarea><div class="nx-composer-bottom"><div class="nx-composer-attachments"><label for="subject-input">Materia</label><input id="subject-input" type="text" value="general" aria-label="Materia de la conversación"></div><button id="send-btn" class="nx-send-button" type="button"><span>Enviar</span><span class="material-symbols-outlined">arrow_upward</span></button></div></div>
      <div class="nx-composer-meta"><span>La IA puede equivocarse. Contrasta lo importante.</span><span><kbd>Enter</kbd> enviar · <kbd>Shift</kbd> + <kbd>Enter</kbd> nueva línea</span></div>
    </footer>
  </section>
  <aside id="sources-sidebar" class="nx-chat-sources hidden" aria-label="Fuentes y contexto"><header><div><p class="nx-eyebrow">Contexto</p><h2>Fuentes</h2></div><button id="close-sources-btn" class="nx-icon-button" type="button" aria-label="Cerrar fuentes"><span class="material-symbols-outlined">close</span></button></header><div class="nx-sources-list"><p>Las fuentes aparecerán aquí cuando el backend las proporcione.</p></div></aside>
</div>
<script>
(function() {
    'use strict';
    var conversationsList = document.getElementById('conversations-list');
    var chatMessages = document.getElementById('chat-messages');
    var promptInput = document.getElementById('prompt-input');
    var subjectInput = document.getElementById('subject-input');
    var activeSubjectSpan = document.getElementById('active-subject');
    var costDisplay = document.getElementById('cost-display');

    // ---------- UI helpers ----------
    function escapeHtml(s) {
        return String(s).replace(/[&<>"']/g, function(c) {
            return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
        });
    }

    function formatTime(isoStr) {
        try {
            var d = new Date(isoStr);
            return d.toLocaleTimeString('es-ES', { hour: '2-digit', minute: '2-digit' });
        } catch (e) { return ''; }
    }

    function showStatus(text, isError) {
        var id = 'status-' + Date.now();
        var div = document.createElement('div');
        div.id = id;
        div.className = 'nx-chat-state' + (isError ? ' is-error' : '');
        div.innerHTML = '<span class="material-symbols-outlined">' + (isError ? 'error' : 'hourglass_empty') + '</span>' +
                        '<span>' + escapeHtml(text) + '</span>';
        chatMessages.appendChild(div);
        chatMessages.scrollTop = chatMessages.scrollHeight;
        return id;
    }

    function removeStatus(id) {
        var el = document.getElementById(id);
        if (el) el.remove();
    }

    function showLoading() {
        var id = 'loading-' + Date.now();
        var div = document.createElement('div');
        div.id = id;
        div.className = 'nx-chat-loading';
        div.setAttribute('role', 'status');
        div.innerHTML = '<span class="nx-message-avatar nx-message-assistant-avatar"><span class="material-symbols-outlined">auto_awesome</span></span><span>El asistente está escribiendo</span><i></i><i></i><i></i>';
        chatMessages.appendChild(div);
        chatMessages.scrollTop = chatMessages.scrollHeight;
        return id;
    }

    function clearChat() {
        chatMessages.innerHTML =
            '<div class="nx-chat-welcome" data-chat-empty="1">' +
            '  <div class="nx-chat-welcome-badge"><span class="material-symbols-outlined">auto_awesome</span>Nueva conversación</div>' +
            '  <h2>Empieza una nueva idea.</h2>' +
            '  <p>Escribe tu primera pregunta. El historial guardado no se ha eliminado.</p>' +
            '</div>';
        promptInput.focus();
    }

    function renderMessage(role, content) {
        var div = document.createElement('div');
        var isUser = role === 'user';
        div.className = isUser ? 'nx-message nx-message-user flex-row-reverse' : 'nx-message nx-message-assistant';
        div.setAttribute('data-chat-turn', isUser ? 'user' : 'assistant');
        if (isUser) {
            div.innerHTML =
                '<div class="nx-message-avatar nx-message-user-avatar">TÚ</div>' +
                '<div class="nx-message-content">' +
                '  <div class="nx-message-meta"><span class="nx-message-author">Tú</span><span class="nx-message-time">Ahora</span></div>' +
                '  <div class="nx-message-bubble"><p class="nx-message-text">' + escapeHtml(content) + '</p></div>' +
                '</div>';
        } else {
            div.innerHTML =
                '<div class="nx-message-avatar nx-message-assistant-avatar"><span class="material-symbols-outlined">auto_awesome</span></div>' +
                '<div class="nx-message-content">' +
                '  <div class="nx-message-meta"><span class="nx-message-author">Asistente</span><span class="nx-message-time">Ahora</span></div>' +
                '  <div class="nx-message-bubble"><p class="nx-message-text">' + escapeHtml(content) + '</p></div>' +
                '  <div class="nx-message-actions">' +
                '    <button onclick="copyAIMessage(this)" type="button"><span class="material-symbols-outlined">content_copy</span><span>Copiar</span></button>' +
                '    <button onclick="createQuizFromMsg(this)" type="button"><span class="material-symbols-outlined">quiz</span><span>Cuestionario</span></button>' +
                '  </div>' +
                '</div>';
        }
        chatMessages.appendChild(div);
        chatMessages.scrollTop = chatMessages.scrollHeight;
    }

    function getLastAIMessage() {
        var msgs = chatMessages.querySelectorAll('[data-chat-turn="assistant"] .nx-message-text');
        return msgs.length ? msgs[msgs.length - 1].textContent : '';
    }

    function messageText(button) {
        var row = button.closest('[data-chat-turn="assistant"]');
        var text = row && row.querySelector('.nx-message-text');
        return text ? text.textContent : '';
    }

    window.copyAIMessage = function(button) {
        var content = messageText(button);
        if (!content) return;
        var labels = button.querySelectorAll('span');
        var label = labels[labels.length - 1];
        function legacyCopy() {
            var area = document.createElement('textarea');
            area.value = content;
            area.style.position = 'fixed';
            area.style.opacity = '0';
            document.body.appendChild(area);
            area.select();
            var copied = document.execCommand('copy');
            area.remove();
            if (!copied) throw new Error('copy failed');
        }
        var copy;
        if (navigator.clipboard && navigator.clipboard.writeText) {
            copy = navigator.clipboard.writeText(content).catch(function() { legacyCopy(); });
        } else {
            copy = Promise.resolve().then(function() { legacyCopy(); });
        }
        copy.then(function() {
            label.textContent = 'Copiado';
            setTimeout(function() { label.textContent = 'Copiar'; }, 1600);
        }).catch(function() { showError('No se pudo copiar el mensaje.'); });
    };

    window.createQuizFromMsg = function(button) {
        var content = messageText(button);
        if (content) createQuizFromContent(content);
    };

    function showError(message) {
        var div = document.createElement('div');
        div.className = 'nx-chat-state is-error';
        div.innerHTML =
            '<span class="material-symbols-outlined">error</span>' +
            '<div><strong>No se pudo completar</strong><p>' + escapeHtml(message) + '</p></div>';
        chatMessages.appendChild(div);
        chatMessages.scrollTop = chatMessages.scrollHeight;
    }

    function showOffline() {
        var div = document.createElement('div');
        div.className = 'nx-chat-state';
        div.innerHTML = '<span class="material-symbols-outlined">wifi_off</span><span>Sin conexión. Revisa tu red y vuelve a intentar.</span>';
        chatMessages.appendChild(div);
        chatMessages.scrollTop = chatMessages.scrollHeight;
    }

    // Error bodies are plain text, except the budget 429 which is JSON.
    function errorText(raw) {
        try {
            var parsed = JSON.parse(raw);
            if (parsed && parsed.error) return parsed.error;
        } catch (e) { /* not JSON: show the raw text */ }
        return raw;
    }

    // ---------- API calls ----------
    async function loadConversations() {
        try {
            var resp = await fetch('/api/ai/conversations');
            if (!resp.ok) throw new Error('no ok');
            var data = await resp.json();
            if (!data.conversations || data.conversations.length === 0) {
                conversationsList.innerHTML = '<div class="font-body-sm text-body-sm text-outline text-center py-space-md">No hay conversaciones. ¡Crea una nueva!</div>';
                return;
            }
            conversationsList.innerHTML = data.conversations.map(function(c) {
                var t = formatTime(c.updated_at || '');
                var subj = escapeHtml(c.subject);
                var prev = escapeHtml((c.last_message || '').substring(0, 100));
                var count = c.message_count || 0;
                return '<button type="button" class="conversation-item nx-session" data-subject="' + escapeHtml(c.subject) + '">' +
                    '<div class="nx-session-top"><strong>' + subj + '</strong><time>' + t + '</time></div>' +
                    '<p>' + prev + '</p>' +
                    '<span class="nx-session-footer"><span>' + subj + '</span><span><span class="material-symbols-outlined">chat_bubble</span> ' + count + '</span></span>' +
                    '</button>';
            }).join('');
        } catch (err) {
            conversationsList.innerHTML = '<div class="font-body-sm text-body-sm text-error text-center py-space-md">Error al cargar conversaciones.</div>';
        }
    }

    async function loadConversation(subject) {
        activeSubjectSpan.textContent = subject;
        subjectInput.value = subject;
        chatMessages.innerHTML =
            '<div class="flex items-center justify-center gap-2 max-w-4xl mx-auto w-full py-4">' +
            '  <span class="material-symbols-outlined text-primary animate-spin text-[20px]">autorenew</span>' +
            '  <span class="font-body-sm text-body-sm text-on-surface-variant">Cargando conversación...</span>' +
            '</div>';
        try {
            var resp = await fetch('/api/ai/conversations/' + encodeURIComponent(subject) + '/messages');
            var data = await resp.json();
            chatMessages.innerHTML = '';
            if (data.messages && data.messages.length > 0) {
                data.messages.forEach(function(m) { renderMessage(m.role, m.content); });
            } else {
                chatMessages.innerHTML = '<div class="text-center py-8"><p class="font-body-sm text-body-sm text-on-surface-variant">No hay mensajes en esta conversación.</p></div>';
            }
        } catch (err) {
            chatMessages.innerHTML = '<div class="text-center py-8"><p class="font-body-sm text-body-sm text-error">Error al cargar la conversación.</p></div>';
        }
    }

    async function sendMessage() {
        var message = promptInput.value.trim();
        var subject = subjectInput.value.trim() || 'general';
        if (!message) return;

        activeSubjectSpan.textContent = subject;
        renderMessage('user', message);
        promptInput.value = '';

        var loadingId = showLoading();
        try {
            var response = await fetch('/api/ai/chat', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ message: message, subject: subject })
            });
            var text = await response.text();
            removeStatus(loadingId);
            if (response.ok) {
                renderMessage('assistant', text);
                loadConversations();
            } else {
                showError('Error: ' + escapeHtml(errorText(text)));
            }
        } catch (err) {
            removeStatus(loadingId);
            showError('Error de conexión: ' + escapeHtml(err.message));
        }
    }

    async function summarizeContent() {
        var content = getLastAIMessage();
        if (!content) { showStatus('Genera un mensaje primero para resumir.', true); return; }
        var statusId = showLoading();
        try {
            var response = await fetch('/api/ai/summary', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ content: content })
            });
            var data = await response.json();
            removeStatus(statusId);
            if (response.ok) {
                renderMessage('assistant', '## Resumen\n\n' + (data.summary || JSON.stringify(data)));
            } else {
                showError('Error: ' + (data.error || 'resumen falló'));
            }
        } catch (err) {
            removeStatus(statusId);
            showError('Error de conexión: ' + escapeHtml(err.message));
        }
    }

    async function explainContent() {
        var content = promptInput.value.trim();
        var subject = subjectInput.value.trim() || 'general';
        if (!content) { showStatus('Escribe algo en el mensaje para explicar.', true); return; }
        activeSubjectSpan.textContent = subject;
        renderMessage('user', 'Explicar a fondo: ' + content);
        promptInput.value = '';
        var statusId = showLoading();
        try {
            var response = await fetch('/api/ai/chat', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ message: 'Explica esto a fondo y con detalle: ' + content, subject: subject })
            });
            var text = await response.text();
            removeStatus(statusId);
            if (response.ok) {
                renderMessage('assistant', text);
                loadConversations();
            } else {
                showError('Error: ' + escapeHtml(errorText(text)));
            }
        } catch (err) {
            removeStatus(statusId);
            showError('Error de conexión: ' + escapeHtml(err.message));
        }
    }

    async function createQuizFromContent(sourceContent) {
        var content = typeof sourceContent === 'string' ? sourceContent : getLastAIMessage();
        if (!content) { showStatus('Genera un mensaje primero para crear un cuestionario.', true); return; }
        var statusId = showLoading();
        try {
            var response = await fetch('/api/ai/quiz', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ content: content, num_questions: 5 })
            });
            var data = await response.json();
            removeStatus(statusId);
            if (response.ok) {
                var quizText = '## Cuestionario\n\n';
                if (data.questions && Array.isArray(data.questions)) {
                    data.questions.forEach(function(q, i) {
                        quizText += (i+1) + '. ' + (q.question || q.text || '') + '\n';
                        if (q.options) {
                            q.options.forEach(function(opt) { quizText += '   - ' + opt + '\n'; });
                        }
                        quizText += '   **Respuesta: **' + (q.answer || q.correct_answer || '') + '\n\n';
                    });
                } else {
                    quizText += JSON.stringify(data, null, 2);
                }
                renderMessage('assistant', quizText);
            } else {
                showError('Error: ' + (data.error || 'cuestionario falló'));
            }
        } catch (err) {
            removeStatus(statusId);
            showError('Error de conexión: ' + escapeHtml(err.message));
        }
    }

    async function createStudySheet() {
        var content = getLastAIMessage();
        if (!content) { showStatus('Genera un mensaje primero para crear una ficha de estudio.', true); return; }
        var statusId = showLoading();
        try {
            var response = await fetch('/api/ai/study-sheet', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ content: content })
            });
            var data = await response.json();
            removeStatus(statusId);
            if (response.ok) {
                renderMessage('assistant', data.study_sheet || JSON.stringify(data));
            } else {
                showError('Error: ' + (data.error || 'ficha falló'));
            }
        } catch (err) {
            removeStatus(statusId);
            showError('Error de conexión: ' + escapeHtml(err.message));
        }
    }

    // ---------- cost telemetry ----------
    async function loadCost() {
        try {
            var resp = await fetch('/api/ai/cost');
            var data = await resp.json();
            var pct = data.budget_limit > 0 ? Math.round((data.total_cost / data.budget_limit) * 100) : 0;
            costDisplay.innerHTML =
                '<span class="font-code-inline text-[10px] text-primary-fixed-dim">$' + data.total_cost.toFixed(2) + ' / $' + data.budget_limit.toFixed(2) + '</span>' +
                '<div class="w-24 h-1.5 bg-surface-container rounded-full overflow-hidden mt-1">' +
                '<div class="bg-gradient-to-r from-secondary to-primary-container h-full rounded-full" style="width: ' + Math.min(pct, 100) + '%"></div>' +
                '</div>';
        } catch (err) {
            costDisplay.innerHTML = '<span class="font-code-inline text-[10px] text-outline">No disponible</span>';
        }
    }

    // ---------- event binding ----------
    document.getElementById('send-btn').addEventListener('click', sendMessage);
    promptInput.addEventListener('keydown', function(e) {
        if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendMessage(); }
    });

    document.getElementById('btn-summarize').addEventListener('click', summarizeContent);
    document.getElementById('btn-explain').addEventListener('click', explainContent);
    document.getElementById('btn-quiz').addEventListener('click', createQuizFromContent);
    document.getElementById('btn-study-sheet').addEventListener('click', createStudySheet);

    document.getElementById('btn-new-chat').addEventListener('click', function() { clearChat(); });
    document.getElementById('btn-clear-chat').addEventListener('click', function() { clearChat(); });

    document.getElementById('toggle-sources-btn').addEventListener('click', function() {
        document.getElementById('sources-sidebar').classList.toggle('hidden');
    });
    document.getElementById('close-sources-btn').addEventListener('click', function() {
        document.getElementById('sources-sidebar').classList.add('hidden');
    });

    conversationsList.addEventListener('click', function(e) {
        var item = e.target.closest('.conversation-item');
        if (!item) return;
        var subject = item.dataset.subject;
        if (!subject) return;
        loadConversation(subject);
    });

    loadConversations();
    loadCost();
})();
</script>
"##;
