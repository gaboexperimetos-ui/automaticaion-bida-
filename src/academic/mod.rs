//! Academic dashboard and planning interface.
//! Agent B implementation.
//!
//! Provides routes for the academic dashboard: daily priorities, calendar,
//! schedule, subjects, and weekly planner. Uses Agent A's MikarenoClient
//! for academic data, with simulated data fallback when MCP is disconnected.

pub mod calendar;
pub mod calendar_data;
pub mod dashboard;
pub mod exams;
pub mod planner;
pub mod profile;
pub mod schedule;
pub mod school_life;
pub mod subject_detail;
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
        .route("/horario", get(crate::handlers::schedule::weekly))
        .route("/planner", get(planner::weekly))
        .route("/profile", get(profile::index))
        .route("/subjects", get(subjects::index))
        .route("/subjects/{code}", get(subject_detail::view))
        .route("/examenes", get(exams::index))
        .route("/vida-escolar", get(school_life::index))
        .route_layer(from_fn_with_state(state.clone(), require_auth_state));

    router.merge(academic)
}

/// Devuelve el cliente Mikareno compartido de la app (si está configurado).
///
/// Reutiliza la misma instancia en todos los handlers para que la caché de
/// respuestas (TTL 180s) y la sesión MCP persistan entre requests. Si no hay
/// cliente compartido, cae a crear uno local (tests/arranques antiguos).
pub fn mikareno_client(state: &AppState) -> Option<crate::integrations::mikareno::MikarenoClient> {
    if let Some(client) = state.mikareno.clone() {
        return Some(client);
    }
    crate::integrations::mikareno::MikarenoClient::from_config(&state.config.mikareno_mcp_url)
}
