//! AI assistant via OpenRouter.
//! Agent D implementation.
//!
//! Provides chat, summarization, quiz generation, and study sheet creation.
//! Uses a mock provider when OPENROUTER_API_KEY is not configured.
//! Enforces a configurable cost budget ($2 USD default).

pub mod chat;
pub mod handlers;
pub mod modelos;
pub mod openrouter;
pub mod quiz;
pub mod streaming;

pub use chat::AiAssistant;
pub use openrouter::{AiError, CostSummary, CostTracker};
pub use quiz::{Quiz, QuizQuestion};
pub use streaming::{chat_stream_handler, stream_chat, ChatStreamId};

use crate::AppState;
use axum::Router;
use std::sync::Arc;

/// Create a new AI assistant backed by the app state.
pub fn create(state: Arc<AppState>) -> AiAssistant {
    AiAssistant::new(state)
}

/// Register AI routes onto a stateful router.
///
/// ALL routes (`/api/ai/*` and the UI route `GET /ai`) require
/// DB-backed session authentication via `require_auth_state`.
pub fn register_routes(
    router: Router<Arc<AppState>>,
    state: Arc<AppState>,
) -> Router<Arc<AppState>> {
    // Auth-protected API routes
    let api = Router::new()
        .route("/api/ai/chat", axum::routing::post(handlers::chat_handler))
        .route(
            "/api/ai/chat/stream",
            axum::routing::post(streaming::chat_stream_handler),
        )
        .route(
            "/api/ai/summary",
            axum::routing::post(handlers::summarize_handler),
        )
        .route("/api/ai/quiz", axum::routing::post(handlers::quiz_handler))
        .route(
            "/api/ai/study-sheet",
            axum::routing::post(handlers::study_sheet_handler),
        )
        .route("/api/ai/cost", axum::routing::get(handlers::cost_handler))
        .route("/api/ai/models", axum::routing::get(modelos::list_models))
        .route(
            "/api/ai/model",
            axum::routing::get(modelos::get_model).put(modelos::set_model),
        )
        .route(
            "/api/ai/conversations",
            axum::routing::get(handlers::conversations_handler),
        )
        .route(
            "/api/ai/conversations/{subject}/messages",
            axum::routing::get(handlers::conversation_messages_handler),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::auth::require_auth_state,
        ));

    // Auth-protected UI route
    let ui = Router::new()
        .route("/ai", axum::routing::get(handlers::ai_page))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::auth::require_auth_state,
        ));

    router.merge(api).merge(ui)
}
