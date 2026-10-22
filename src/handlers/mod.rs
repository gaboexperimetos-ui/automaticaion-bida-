//! HTTP handlers and route registration for the Study Platform.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{from_fn, from_fn_with_state},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};

use crate::academic::templates::html_escape;
use crate::middleware::auth::{extract_session_cookie, require_auth_state};
use crate::middleware::rate_limit::rate_limit_middleware;
use crate::AppState;

mod admin;
mod api;
mod auth;
pub mod calendar_annotations;
mod dashboard;
pub mod handoff;
pub mod material;
pub mod notes;
pub mod schedule;
mod search;
pub mod settings;
mod static_ui;
pub mod stitch_chat;
pub mod stitch_mcp;
pub mod stitch_notes;
pub mod stitch_panel;
pub mod stitch_v2;

/// Build the main application router.
pub fn router(state: Arc<AppState>) -> Router {
    // Public routes — no authentication required
    let public = Router::new()
        .route(
            "/dashboard",
            get(stitch_v2::stitch_dashboard)
                .layer(from_fn_with_state(state.clone(), require_auth_state)),
        )
        .route(
            "/dashboard/ai",
            get(stitch_chat::page).layer(from_fn_with_state(state.clone(), require_auth_state)),
        )
        .route(
            "/dashboard/notes",
            get(stitch_v2::stitch_notes)
                .layer(from_fn_with_state(state.clone(), require_auth_state)),
        )
        .route(
            "/dashboard/mcp",
            get(stitch_v2::stitch_mcp).layer(from_fn_with_state(state.clone(), require_auth_state)),
        )
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
        .route("/app.js", get(static_ui::serve_js))
        // Capa de interactividad del shell Stitch v2 (toasts, menús, editor).
        .route("/shell.js", get(static_ui::serve_shell_js))
        .route("/favicon.ico", get(static_ui::serve_favicon))
        .route("/favicon.svg", get(static_ui::serve_favicon));

    // Protected routes — require authentication
    let protected = Router::new()
        .route("/dashboard/calendar.ics", get(dashboard::ical_export))
        .route("/app/notes", get(notes::list).post(notes::create))
        .route("/app/notes/new", get(notes::new_note))
        .route("/app/notes/{slug}", get(notes::view))
        .route("/app/notes/{slug}/export", get(notes::export))
        .route(
            "/app/notes/{slug}/versions/{version_id}/restore",
            post(notes::restore_version),
        )
        .route("/app/notes/{slug}/edit", get(notes::edit))
        .route("/app/notes/{slug}/update", put(notes::update))
        .route("/app/notes/{slug}/delete", delete(notes::delete))
        .route(
            "/dashboard/material",
            get(material::page).post(material::generate),
        )
        .route("/api/notes", get(notes::api_list).post(notes::api_create))
        .route(
            "/api/notes/{slug}",
            get(notes::api_view)
                .put(notes::api_update)
                .delete(notes::api_delete),
        )
        // Búsqueda de notas (Agente C1 engancha el top-bar a /api/search).
        .route("/api/search", get(search::search_api))
        .route("/dashboard/search", get(search::search_page))
        // Anotaciones del calendario (datos locales del usuario).
        .route(
            "/api/calendar/annotations",
            get(calendar_annotations::list).post(calendar_annotations::create),
        )
        .route(
            "/api/calendar/annotations/{id}",
            put(calendar_annotations::update).delete(calendar_annotations::delete),
        )
        // Admin API legacy (métricas/backup; el dashboard F9 vive en admin::register_routes).
        .route("/api/admin/metrics", get(admin::metrics))
        .route(
            "/api/admin/backup",
            get(admin::backup).post(admin::backup_now_handler),
        )
        .route(
            "/api/admin/backup/restore",
            post(admin::backup_restore_handler),
        )
        .route(
            "/api/admin/backup/download/{name}",
            get(admin::backup_download_handler),
        )
        .route("/api/admin/backup-status", get(admin::backup_status))
        .route("/settings", get(settings::page))
        .route("/api/settings/ai", post(settings::save_ai))
        .route(
            "/settings/password",
            get(auth::password_page).post(auth::password_submit),
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

    // Admin F9 (Agent D): dashboard /admin + metrics + budget + backups
    let admin_panel = admin::register_routes(Router::new(), state.clone());

    Router::new()
        .merge(public)
        .merge(protected)
        .nest("/dashboard", academic)
        .merge(office)
        .merge(mcp_routes)
        .merge(ai)
        .merge(admin_panel)
        // Fallback global: toda ruta no registrada responde 404 (página
        // estilizada o JSON, según el prefijo). Va después de todos los
        // merges para que capture todo lo que no está registrado, y antes
        // de `with_state` para que el handler reciba el estado de la app.
        .fallback(not_found)
        .with_state(state)
        .layer(from_fn(rate_limit_middleware))
}

/// ¿La ruta responde JSON en el 404 (en vez de la página HTML)?
///
/// `/api/*` lo consume el frontend con `fetch` y `/mcp` es JSON-RPC: un
/// cuerpo HTML los rompe, así que ambos responden `{"error":"not_found"}`.
fn es_ruta_json(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/mcp" || path.starts_with("/mcp/")
}

/// Contenido de la página 404 (el shell Stitch v2 lo envuelve después).
///
/// El path solicitado viene de la URL: se **escapa** y se recorta antes de
/// mostrarlo, nunca en crudo.
fn not_found_content(path: &str) -> String {
    let ruta = html_escape(&path.chars().take(120).collect::<String>());
    format!(
        r##"<div class="flex flex-col items-center justify-center py-space-lg">
  <div class="bg-surface-container border border-outline rounded-xl p-space-lg w-full max-w-2xl text-center">
    <span class="material-symbols-outlined text-[40px] text-outline">search_off</span>
    <h1 class="font-headline-md text-headline-md text-on-surface mt-space-sm">Página no encontrada</h1>
    <p class="font-body-sm text-body-sm text-on-surface-variant mt-space-xs">La ruta que has abierto no corresponde a ninguna pantalla de la aplicación.</p>
    <p class="font-code-inline text-[12px] text-on-surface-variant mt-space-sm break-all">Ruta: {ruta}</p>
    <p class="font-code-inline text-[11px] text-outline mt-space-xs">HTTP 404 · Not Found</p>
    <div class="flex flex-wrap gap-space-sm justify-center mt-space-md">
      <a href="/dashboard" class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-primary-container text-on-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors">
        <span class="material-symbols-outlined text-[16px]">dashboard</span> Panel principal
      </a>
      <a href="/dashboard/horario" class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-surface-container-high border border-outline text-on-surface font-label-md font-semibold hover:bg-surface-container-low transition-colors">
        <span class="material-symbols-outlined text-[16px]">schedule</span> Horario
      </a>
      <a href="/app/notes" class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-surface-container-high border border-outline text-on-surface font-label-md font-semibold hover:bg-surface-container-low transition-colors">
        <span class="material-symbols-outlined text-[16px]">description</span> Mis notas
      </a>
      <a href="/admin" class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-surface-container-high border border-outline text-on-surface font-label-md font-semibold hover:bg-surface-container-low transition-colors">
        <span class="material-symbols-outlined text-[16px]">admin_panel_settings</span> Administración
      </a>
    </div>
  </div>
</div>"##,
        ruta = ruta,
    )
}

/// 404 global: toda ruta que no está registrada cae aquí.
///
/// Las rutas de API (`/api/*`) y el MCP (`/mcp`) responden JSON
/// `{"error":"not_found"}`; el resto devuelve la página 404 estilizada con
/// el shell Stitch v2, como las demás páginas internas.
async fn not_found(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();

    if es_ruta_json(&path) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "not_found" })),
        )
            .into_response();
    }

    let content = not_found_content(&path);
    let authenticated = state.config.auth_disabled || extract_session_cookie(&req).is_some();
    let html = stitch_v2::stitch_v2_page("", &content, authenticated);
    let shell = stitch_panel::fetch_shell_data(&state).await;
    let html = stitch_panel::fill_shell(&html, &shell);
    (StatusCode::NOT_FOUND, Html(html)).into_response()
}
