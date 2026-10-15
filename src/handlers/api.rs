//! API handlers — health check, configuration, public endpoints.
//!
//! No secrets are ever returned. All responses use serde_json::Value for flexibility.

use axum::{extract::State, http::StatusCode, response::Json};
use std::sync::Arc;

use crate::{db, AppState};

/// Health check endpoint.
/// Returns service status and component availability.
/// No sensitive data is exposed.
pub async fn health(State(state): State<Arc<AppState>>) -> (StatusCode, Json<serde_json::Value>) {
    let db_ok = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.db)
        .await
        .is_ok();

    let response = serde_json::json!({
        "status": if db_ok { "healthy" } else { "degraded" },
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": env!("CARGO_PKG_VERSION"),
        "components": {
            "database": db_ok,
            "openrouter": state.config.openrouter_api_key.is_some(),
            "mikareno_mcp": false,
            "genoffice_mcp": false,
        }
    });

    let status = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(response))
}

/// Public configuration (no secrets) — safe for frontend consumption.
pub async fn config(State(state): State<Arc<AppState>>) -> (StatusCode, Json<serde_json::Value>) {
    let response = serde_json::json!({
        "openrouter_configured": state.config.openrouter_api_key.is_some(),
        "openrouter_model": state.config.openrouter_model,
        "mikareno_mcp_url": state.config.mikareno_mcp_url,
        "genoffice_mcp_url": state.config.genoffice_mcp_url,
        "version": env!("CARGO_PKG_VERSION"),
        "is_localhost": state.config.host == "127.0.0.1",
    });

    (StatusCode::OK, Json(response))
}

/// Alias for /api/config
pub async fn public_config(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    config(State(state)).await
}

/// Debug/info endpoint — minimal info, no secrets.
pub async fn debug_info(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let note_count = db::get_note_count(&state.db).await.unwrap_or(0);

    let info = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "database_path": state.config.db_path.display().to_string(),
        "notes_directory": state.config.notes_dir.display().to_string(),
        "note_count": note_count,
        "openrouter_configured": state.config.openrouter_api_key.is_some(),
        "openrouter_model": state.config.openrouter_model,
        "mikareno_mcp_url": state.config.mikareno_mcp_url,
        "genoffice_mcp_url": state.config.genoffice_mcp_url,
        "is_localhost": state.config.host == "127.0.0.1",
    });

    (StatusCode::OK, Json(info))
}
