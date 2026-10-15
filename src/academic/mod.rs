//! Academic dashboard and planning interface.
//! Agent B implementation.
//!
//! Provides routes for the academic dashboard: daily priorities, calendar,
//! schedule, subjects, and weekly planner. Uses Agent A's MikarenoClient
//! for academic data, with simulated data fallback when MCP is disconnected.

pub mod calendar;
pub mod dashboard;
pub mod planner;
pub mod profile;
pub mod schedule;
pub mod subjects;
pub mod templates;

use std::sync::Arc;

use axum::{middleware::from_fn_with_state, routing::get, Router};

use crate::middleware::auth::require_auth_state;
use crate::AppState;

/// Register academic dashboard routes onto a stateful router.
///
/// Routes are auth-protected via `require_auth_state` (applied as a `route_layer`).
/// The returned router is merged into the main application router.
pub fn register_routes(
    router: Router<Arc<AppState>>,
    state: Arc<AppState>,
) -> Router<Arc<AppState>> {
    let _ = state; // State is provided by the outer router

    let academic = Router::new()
        .route("/calendar", get(calendar::monthly))
        .route("/calendar/daily", get(calendar::daily))
        .route("/horario", get(schedule::weekly))
        .route("/planner", get(planner::weekly))
        .route("/profile", get(profile::index))
        .route("/subjects", get(subjects::index))
        .route_layer(from_fn_with_state(state.clone(), require_auth_state));

    router.merge(academic)
}

/// Create a MikarenoClient from app config.
/// Returns None if the URL is not localhost.
pub fn mikareno_client(state: &AppState) -> Option<crate::integrations::mikareno::MikarenoClient> {
    let url = &state.config.mikareno_mcp_url;
    if url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost") {
        Some(crate::integrations::mikareno::MikarenoClient::new(
            url.clone(),
        ))
    } else {
        None
    }
}
