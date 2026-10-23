//! API handlers — health check, configuration, public endpoints.
//!
//! No secrets are ever returned. All responses use serde_json::Value for flexibility.

use axum::{extract::State, http::StatusCode, response::Json};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use crate::{db, AppState};

/// Shared HTTP client for health probes (one per process).
static PROBE_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(2500))
        .build()
        .expect("reqwest client for health probes")
});

/// Last MCP probe result: (checked_at, mikareno_ok, genoffice_ok).
/// TTL-cached so `/health` stays cheap — the rate-limit tests hammer it.
static MCP_PROBE_CACHE: std::sync::Mutex<Option<(Instant, bool, bool)>> =
    std::sync::Mutex::new(None);

const MCP_PROBE_TTL: Duration = Duration::from_secs(30);

/// POST an MCP `initialize` request; true when the server answers 2xx.
async fn probe_mcp(url: &str) -> bool {
    if url.is_empty() {
        return false;
    }
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "study-platform-health", "version": env!("CARGO_PKG_VERSION") },
        },
    });
    PROBE_CLIENT
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Cached `(mikareno, genoffice)` availability, refreshed at most each TTL.
async fn mcp_components(state: &AppState) -> (bool, bool) {
    if let Some((at, m, g)) = *MCP_PROBE_CACHE.lock().unwrap_or_else(|e| e.into_inner()) {
        if at.elapsed() < MCP_PROBE_TTL {
            return (m, g);
        }
    }
    let (m, g) = tokio::join!(
        probe_mcp(&state.config.mikareno_mcp_url),
        probe_mcp(&state.config.genoffice_mcp_url),
    );
    *MCP_PROBE_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), m, g));
    (m, g)
}

/// Health check endpoint.
/// Returns service status and component availability.
/// No sensitive data is exposed.
pub async fn health(State(state): State<Arc<AppState>>) -> (StatusCode, Json<serde_json::Value>) {
    let db_ok = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.db)
        .await
        .is_ok();

    let (mikareno_ok, genoffice_ok) = mcp_components(&state).await;

    let response = serde_json::json!({
        "status": if db_ok { "healthy" } else { "degraded" },
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": env!("CARGO_PKG_VERSION"),
        "components": {
            "database": db_ok,
            "openrouter": state.config.openrouter_api_key.is_some(),
            "mikareno_mcp": mikareno_ok,
            "genoffice_mcp": genoffice_ok,
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
