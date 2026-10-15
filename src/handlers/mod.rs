//! HTTP handlers and route registration for the Study Platform.

use std::sync::Arc;

use axum::{
    extract::State,
    middleware::{from_fn, from_fn_with_state},
    routing::{delete, get, post, put},
    Router,
};

use crate::middleware::auth::require_auth_state;
use crate::middleware::rate_limit::rate_limit_middleware;
use crate::AppState;

mod api;
mod auth;
mod dashboard;
pub mod handoff;
pub mod notes;
pub mod stitch_v2;
mod static_ui;
mod studyos;

/// Build the main application router.
pub fn router(state: Arc<AppState>) -> Router {
    // Public routes — no authentication required
    let public = Router::new()
        .route("/dashboard", get(stitch_v2::stitch_dashboard).layer(from_fn_with_state(state.clone(), require_auth_state)))
        .route("/dashboard/ai", get(stitch_v2::stitch_asistente).layer(from_fn_with_state(state.clone(), require_auth_state)))
        .route("/dashboard/notes", get(stitch_v2::stitch_notes).layer(from_fn_with_state(state.clone(), require_auth_state)))
        .route("/dashboard/mcp", get(stitch_v2::stitch_mcp).layer(from_fn_with_state(state.clone(), require_auth_state)))
        .route("/login", get(auth::login_form).post(auth::login_submit))
        .route("/register", post(auth::register))
        .route("/logout", post(auth::logout))
        .route("/health", get(api::health))
        .route("/api/config", get(api::config))
        .route("/api/public-config", get(api::public_config))
        .route("/handoff", get(handoff::get_handoff))
        .route(
            "/",
            get(stitch_v2::stitch_index)
                .layer(from_fn_with_state(state.clone(), require_auth_state)),
        )
        // Previous interface (study-platform-ui), kept reachable for reference.
        .route("/legacy", get(static_ui::serve_index))
        .route("/styles.css", get(static_ui::serve_css))
        .route("/app.js", get(static_ui::serve_js));

    // Protected routes — require authentication
    let protected = Router::new()
        .route("/dashboard/calendar.ics", get(dashboard::ical_export))
        .route("/app/notes", get(notes::list).post(notes::create))
        .route("/app/notes/{slug}", get(notes::view))
        .route("/app/notes/{slug}/edit", get(notes::edit))
        .route("/app/notes/{slug}/update", put(notes::update))
        .route("/app/notes/{slug}/delete", delete(notes::delete))
        .route("/api/notes", get(notes::api_list).post(notes::api_create))
        .route(
            "/api/notes/{slug}",
            get(notes::api_view)
                .put(notes::api_update)
                .delete(notes::api_delete),
        )
        .route_layer(from_fn_with_state(state.clone(), require_auth_state));

    // Academic dashboard routes (Agent B) — nested under /dashboard
    let academic = crate::academic::register_routes(Router::new(), state.clone());

    // Office suite routes (Agent E)
    let office = crate::office::register_routes(Router::new(), state.clone());

    // MCP server routes (Agent F)
    let mcp_routes = crate::mcp_server::register_routes(Router::new(), state.clone());

    // AI assistant routes (Agent A)
    let ai = crate::ai::register_routes(Router::new(), state.clone());

    Router::new()
        .merge(public)
        .merge(protected)
        .nest("/dashboard", academic)
        .merge(office)
        .merge(mcp_routes)
        .merge(ai)
        .with_state(state)
        .layer(from_fn(rate_limit_middleware))
}
