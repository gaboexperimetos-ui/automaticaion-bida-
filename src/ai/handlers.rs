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
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    Json,
};

use serde::Deserialize;

use std::sync::Arc;

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

    let spent_usd =
        match crate::ai::openrouter::persistent_summary(&state.db, &state.config).await {
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

    let assistant = AiAssistant::new(state);

    match assistant.chat(&payload.message, &subject).await {
        Ok(response) => (StatusCode::OK, response).into_response(),

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
        "/dashboard/ai",
        &body_html,
    );

    Html(html)
}

/// HTML template for the AI chat UI page (Stitch "Synthetic Cognition" design system).

/// Rendered inside the shared Stitch shell (nav-rail + top-bar) via `page_ext`;

/// this const holds ONLY the main slot content so all screens share the same shell.

const AI_PAGE_TEMPLATE: &str = r##"

<!-- Three-panel chat workspace -->
<div class="flex w-full h-[calc(100vh-7rem)] min-w-0 gap-space-sm">
    <!-- ================= LEFT: Sessions panel ================= -->
    <aside class="w-[280px] shrink-0 flex flex-col bg-surface-container-low select-none">
        <div class="p-space-md">
            <button id="btn-new-chat" class="w-full flex items-center justify-center gap-space-xs py-space-sm px-space-md rounded-lg bg-secondary text-on-secondary font-label-md font-semibold hover:bg-secondary-fixed transition-all shadow-md">
                <span class="material-symbols-outlined text-[18px]">auto_awesome</span>
                <span>Nueva conversación</span>
            </button>
        </div>
        <div class="px-space-md mb-space-sm">
            <div class="flex items-center gap-space-xs bg-surface-container px-space-sm py-1.5 rounded-lg text-outline focus-within:text-on-surface">
                <span class="material-symbols-outlined text-[18px]">search</span>
                <input id="session-search" class="bg-transparent border-none outline-none font-body-sm text-body-sm text-on-surface placeholder:text-outline w-full" placeholder="Filtrar sesiones..." type="text">
            </div>
        </div>
        <div id="conversations-list" class="flex-1 overflow-y-auto px-space-sm space-y-space-xs" style="scrollbar-width: thin;">
            <div class="font-body-sm text-body-sm text-outline text-center py-space-md">Cargando conversaciones...</div>
        </div>
        <div class="p-space-sm bg-surface-container-lowest text-on-surface-variant border-t border-outline-variant/40">
            <div class="flex items-center justify-between mb-1">
                <span class="font-label-sm text-[11px] text-on-surface font-semibold flex items-center gap-1">
                    <span class="w-1.5 h-1.5 rounded-full __MODE_DOT__ animate-pulse"></span>
                    __MODEL__
                </span>
                <span id="mode-badge" class="px-2 py-0.5 rounded-full bg-surface-container font-label-sm text-[10px] __MODE_COLOR__">__MODE_LABEL__</span>
            </div>
            <div id="cost-display" class="font-code-inline text-[10px] text-outline flex flex-col gap-space-xs">
                <span class="flex items-center gap-1"><span class="material-symbols-outlined text-[12px]">schedule</span> Cargando...</span>
            </div>
        </div>
    </aside>

    <!-- ================= CENTER: Chat workspace ================= -->
    <main class="flex-1 min-w-0 flex flex-col bg-surface relative">
        <header class="h-14 px-space-md flex items-center justify-between bg-surface-container-low shrink-0 border-b border-outline-variant/40">
            <div class="flex items-center gap-space-sm min-w-0">
                <span class="material-symbols-outlined text-primary-container text-[20px] shrink-0">neurology</span>
                <span class="font-label-sm text-label-sm uppercase tracking-wider text-outline shrink-0">Contexto:</span>
                <span id="active-subject" class="font-label-md text-label-md text-on-surface-variant truncate">general</span>
            </div>
            <div class="flex items-center gap-space-xs shrink-0">
                <button id="toggle-sources-btn" class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-surface-container hover:bg-surface-container-high text-on-surface font-label-sm transition-all" title="Alternar panel de fuentes">
                    <span class="material-symbols-outlined text-[16px] text-primary-container">source</span>
                    <span>Fuentes</span>
                </button>
                <button id="btn-clear-chat" class="p-1.5 rounded-lg text-outline hover:text-on-surface hover:bg-surface-container transition-colors" title="Limpiar pantalla">
                    <span class="material-symbols-outlined text-[18px]">restart_alt</span>
                </button>
            </div>
        </header>

        <div id="chat-messages" class="flex-1 overflow-y-auto px-space-xl py-space-lg space-y-space-lg" style="scrollbar-width: thin;">
            <div class="text-center py-8">
                <p class="font-body-sm text-body-sm text-on-surface-variant">¡Hola! Soy tu asistente académico. Escribe un mensaje para empezar.</p>
            </div>
        </div>

        <footer class="p-space-md bg-surface-container-low shrink-0 border-t border-outline-variant/40">
            <div class="max-w-4xl mx-auto space-y-space-sm">
                <div class="flex items-center gap-1.5 overflow-x-auto pb-1" style="scrollbar-width: none;">
                    <button id="btn-summarize" class="flex items-center gap-1 px-2.5 py-1.5 rounded-full bg-surface-container text-primary-container hover:bg-surface-container-high font-label-sm text-label-md whitespace-nowrap transition-colors">
                        <span class="material-symbols-outlined text-[14px]">summarize</span><span>Resumir</span>
                    </button>
                    <button id="btn-explain" class="flex items-center gap-1 px-2.5 py-1.5 rounded-full bg-surface-container text-primary-container hover:bg-surface-container-high font-label-sm text-label-md whitespace-nowrap transition-colors">
                        <span class="material-symbols-outlined text-[14px]">search_insights</span><span>Explicar a fondo</span>
                    </button>
                    <button id="btn-quiz" class="flex items-center gap-1 px-2.5 py-1.5 rounded-full bg-surface-container text-secondary hover:bg-surface-container-high font-label-sm text-label-md whitespace-nowrap transition-colors">
                        <span class="material-symbols-outlined text-[14px]">quiz</span><span>Cuestionario</span>
                    </button>
                    <button id="btn-study-sheet" class="flex items-center gap-1 px-2.5 py-1.5 rounded-full bg-surface-container text-secondary hover:bg-surface-container-high font-label-sm text-label-md whitespace-nowrap transition-colors">
                        <span class="material-symbols-outlined text-[14px]">style</span><span>Ficha de estudio</span>
                    </button>
                </div>
                <div class="bg-surface-container rounded-xl p-space-sm flex flex-col gap-space-xs shadow-inner">
                    <textarea id="prompt-input" class="bg-transparent text-on-surface font-body-md text-body-md placeholder:text-outline resize-none outline-none w-full p-space-xs" placeholder="Pregunta algo sobre tus asignaturas... (Shift+Enter para nueva línea)" rows="3"></textarea>
                    <div class="flex items-center justify-between pt-1">
                        <input id="subject-input" type="text" class="w-1/3 px-3 py-1.5 bg-surface-container-high border border-outline-variant rounded font-body-sm text-on-surface placeholder:text-outline focus:outline-none focus:ring-1 focus:ring-primary-container" placeholder="Asignatura" value="general">
                        <button id="send-btn" class="flex items-center justify-center gap-1.5 px-space-md py-2 rounded-lg bg-primary-container text-on-primary font-label-md font-semibold hover:bg-primary transition-all shadow-md">
                            <span>Enviar</span>
                            <span class="material-symbols-outlined text-[18px]">send</span>
                        </button>
                    </div>
                </div>
            </div>
        </footer>
    </main>

    <!-- ================= RIGHT: Sources panel ================= -->
    <aside id="sources-sidebar" class="w-[300px] shrink-0 flex-col bg-surface-container-low overflow-hidden hidden">
        <div class="p-space-md flex items-center justify-between border-b border-outline-variant/40">
            <div class="flex items-center gap-2">
                <span class="material-symbols-outlined text-[18px] text-primary-container">source</span>
                <h2 class="font-headline-sm text-headline-sm text-on-surface">Fuentes & Contexto</h2>
            </div>
            <button id="close-sources-btn" class="p-1 rounded text-outline hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">close</span>
            </button>
        </div>
        <div class="flex-1 overflow-y-auto p-space-md">
            <div class="text-center py-8">
                <span class="material-symbols-outlined text-[32px] text-outline mb-2">info</span>
                <p class="font-body-sm text-body-sm text-on-surface-variant">Las fuentes se mostrarán aquí cuando el backend las proporcione.</p>
            </div>
        </div>
    </aside>
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
        div.className = 'flex items-center gap-2 max-w-4xl mx-auto w-full';
        div.innerHTML = '<span class="material-symbols-outlined text-[16px] ' + (isError ? 'text-error' : 'text-primary') + '">' + (isError ? 'error' : 'hourglass_empty') + '</span>' +
                        '<span class="font-body-sm text-body-sm ' + (isError ? 'text-error' : 'text-on-surface-variant') + '">' + escapeHtml(text) + '</span>';
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
        chatMessages.innerHTML =
            '<div class="flex items-center justify-center gap-2 max-w-4xl mx-auto w-full py-4">' +
            '  <span class="material-symbols-outlined text-primary animate-spin text-[20px]">autorenew</span>' +
            '  <span class="font-body-sm text-body-sm text-on-surface-variant">Escribiendo...</span>' +
            '</div>';
        chatMessages.scrollTop = chatMessages.scrollHeight;
        return id;
    }

    function clearChat() {
        chatMessages.innerHTML =
            '<div class="text-center py-8">' +
            '  <p class="font-body-sm text-body-sm text-on-surface-variant">¡Hola! Soy tu asistente académico. Escribe un mensaje para empezar.</p>' +
            '</div>';
    }

    function renderMessage(role, content) {
        var div = document.createElement('div');
        div.className = 'flex items-start gap-space-md max-w-4xl mx-auto w-full group';
        if (role === 'user') {
            div.innerHTML =
                '<div class="w-9 h-9 rounded-full bg-surface-container-high flex items-center justify-center shrink-0 font-label-md text-on-surface font-bold">TÚ</div>' +
                '<div class="flex-1 min-w-0">' +
                '  <div class="flex items-center gap-space-xs mb-1">' +
                '    <span class="font-label-md text-label-md font-semibold text-on-surface">Estudiante</span>' +
                '  </div>' +
                '  <div class="bg-surface-container-high text-on-surface rounded-xl p-space-md shadow-sm">' +
                '    <p class="font-body-md text-body-md leading-relaxed whitespace-pre-wrap">' + escapeHtml(content) + '</p>' +
                '  </div>' +
                '</div>';
        } else {
            div.innerHTML =
                '<div class="w-9 h-9 rounded-full bg-primary-container/15 flex items-center justify-center shrink-0 text-primary-container">' +
                '  <span class="material-symbols-outlined text-[20px]">smart_toy</span>' +
                '</div>' +
                '<div class="flex-1 min-w-0 space-y-space-sm">' +
                '  <div class="flex items-center gap-space-xs">' +
                '    <span class="font-label-md text-label-md font-semibold text-primary-fixed-dim">Asistente IA</span>' +
                '    <span class="font-code-inline text-[10px] text-outline">Ahora</span>' +
                '  </div>' +
                '  <div class="bg-surface-container-low rounded-xl p-space-lg text-on-surface space-y-space-md shadow-md">' +
                '    <p class="font-body-md text-body-md leading-relaxed text-on-surface-variant whitespace-pre-wrap">' + escapeHtml(content) + '</p>' +
                '  </div>' +
                '  <div class="flex flex-wrap items-center gap-1.5 pt-1">' +
                '    <button onclick="copyAIMessage(this)" class="flex items-center gap-1 px-2.5 py-1 rounded-md bg-surface-container hover:bg-surface-container-high text-on-surface-variant hover:text-on-surface font-label-sm text-label-sm transition-all">' +
                '      <span class="material-symbols-outlined text-[15px]">content_copy</span><span>Copiar</span>' +
                '    </button>' +
                '    <button onclick="createQuizFromMsg(this)" class="flex items-center gap-1 px-2.5 py-1 rounded-md bg-surface-container hover:bg-surface-container-high text-secondary font-label-md text-label-sm transition-all">' +
                '      <span class="material-symbols-outlined text-[15px]">quiz</span><span>Cuestionario</span>' +
                '    </button>' +
                '  </div>' +
                '</div>';
        }
        chatMessages.appendChild(div);
        chatMessages.scrollTop = chatMessages.scrollHeight;
    }

    function getLastAIMessage() {
        var msgs = chatMessages.querySelectorAll('.flex-1.min-w-0.space-y-space-sm');
        if (msgs.length === 0) return '';
        var last = msgs[msgs.length - 1];
        var p = last.querySelector('p');
        return p ? p.textContent : '';
    }

    function showError(message) {
        var div = document.createElement('div');
        div.className = 'flex items-start gap-space-md max-w-4xl mx-auto w-full';
        div.innerHTML =
            '<span class="material-symbols-outlined text-[20px] text-error shrink-0">error</span>' +
            '<div class="flex-1 min-w-0">' +
            '  <div class="bg-error-container/10 border border-error rounded-xl p-space-md">' +
            '    <p class="font-body-md text-body-md text-on-error font-semibold mb-1">Error</p>' +
            '    <p class="font-body-sm text-body-sm text-on-error-container">' + escapeHtml(message) + '</p>' +
            '  </div>' +
            '</div>';
        chatMessages.appendChild(div);
        chatMessages.scrollTop = chatMessages.scrollHeight;
    }

    function showOffline() {
        chatMessages.innerHTML =
            '<div class="flex items-start gap-space-md max-w-4xl mx-auto w-full">' +
            '  <span class="material-symbols-outlined text-[20px] text-outline shrink-0">wifi_off</span>' +
            '  <div class="flex-1 min-w-0">' +
            '    <p class="font-body-md text-body-md text-on-surface-variant">' +
            '      Sin conexión. Revisa tu red y vuelve a intentar.' +
            '    </p>' +
            '  </div>' +
            '</div>';
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
                return '<div class="conversation-item p-space-sm rounded-lg bg-surface-container/50 hover:bg-surface-container cursor-pointer transition-colors group" data-subject="' + escapeHtml(c.subject) + '">' +
                    '<div class="flex items-start justify-between gap-1 mb-1">' +
                    '<span class="font-label-md text-label-md font-medium truncate text-on-surface">' + subj + '</span>' +
                    '<span class="font-code-inline text-[10px] text-outline shrink-0">' + t + '</span>' +
                    '</div>' +
                    '<p class="font-body-sm text-body-sm text-on-surface-variant line-clamp-2 mb-1.5">' + prev + '</p>' +
                    '<div class="flex items-center justify-between">' +
                    '<span class="px-1.5 py-0.5 rounded text-[10px] font-label-sm bg-surface-container-low text-on-surface-variant truncate max-w-[120px] overflow-hidden">' + subj + '</span>' +
                    '<span class="font-code-inline text-[10px] text-outline flex items-center gap-0.5">' +
                    '<span class="material-symbols-outlined text-[12px]">chat_bubble</span> ' + count + '</span>' +
                    '</div>' +
                    '</div>';
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

    async function createQuizFromContent() {
        var content = getLastAIMessage();
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
