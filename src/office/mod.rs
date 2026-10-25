//! Web office suite: documents, spreadsheets, presentations, and file library.
//!
//! All persistence is filesystem-based so **no external services are required**.
//! Office artifacts live under `data/office/{docs,sheets,presentations}` (the
//! `office` subdirectory next to the SQLite database, derived from
//! `AppState.config.db_path`).
//!
//! Routes are all auth-protected via `crate::middleware::auth::require_auth_state`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json, Router,
};
use serde_json::json;

use crate::middleware::auth::require_auth_state;
use crate::AppState;

pub mod documents;
pub mod genoffice;
pub mod library;
pub mod presentations;
pub mod report;
pub mod spreadsheets;
pub mod templates;

pub use genoffice::OfficeError;
pub use report::export_docx_report;

/// Filesystem root for all office artifacts: `<db parent>/office`.
pub(crate) fn office_root(state: &AppState) -> PathBuf {
    let parent = state
        .config
        .db_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    parent.join("office")
}

/// Directory holding Markdown documents (`*.md`).
pub(crate) fn docs_dir(state: &AppState) -> PathBuf {
    office_root(state).join("docs")
}

/// Directory holding spreadsheets (`*.csv`).
pub(crate) fn sheets_dir(state: &AppState) -> PathBuf {
    office_root(state).join("sheets")
}

/// Directory holding presentations (`*.json`).
pub(crate) fn pres_dir(state: &AppState) -> PathBuf {
    office_root(state).join("presentations")
}

/// Ensure the three office directories exist.
pub(crate) fn ensure_dirs(state: &AppState) {
    for dir in [docs_dir(state), sheets_dir(state), pres_dir(state)] {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            // Los handlers devuelven 500 al escribir; deja además una pista
            // operativa clara en lugar de ocultar el fallo de inicialización.
            tracing::error!("office directory initialization failed: {}", e);
        }
    }
}

/// Extension used on disk for each office type.
pub(crate) fn type_ext(ty: &str) -> Option<&'static str> {
    match ty {
        "docs" => Some("md"),
        "sheets" => Some("csv"),
        "presentations" => Some("json"),
        _ => None,
    }
}

/// Directory for a given office type.
pub(crate) fn type_dir(state: &AppState, ty: &str) -> Option<PathBuf> {
    match ty {
        "docs" => Some(docs_dir(state)),
        "sheets" => Some(sheets_dir(state)),
        "presentations" => Some(pres_dir(state)),
        _ => None,
    }
}

/// Return `true` when `s` is safe to join into a filesystem path as a slug or
/// file stem. Rejects empty strings, `.` / `..`, embedded path separators,
/// and any `..` sequence (covers `../`, `a/../b`, and percent-decoded `..`).
pub(crate) fn is_safe_slug(s: &str) -> bool {
    !s.is_empty()
        && !s.contains("..")
        && !s.contains('/')
        && !s.contains('\\')
        && s != "."
        && s != ".."
}

/// Register the office routes onto a stateful router.
///
/// Routes are all auth-protected via `require_auth_state` (applied as a `route_layer`).
/// The `state` argument is used to eagerly create the office directories
/// before the first request. The caller merges the returned router into the
/// main application router and applies `with_state` if not already applied.
pub fn register_routes(
    router: Router<Arc<AppState>>,
    state: Arc<AppState>,
) -> Router<Arc<AppState>> {
    ensure_dirs(&state);
    tracing::debug!(
        "office routes registered; root={}",
        office_root(&state).display()
    );

    let office: Router<Arc<AppState>> = Router::new()
        .route("/office", axum::routing::get(documents::office_home))
        // Documents
        .route(
            "/office/docs",
            axum::routing::get(documents::list).post(documents::create),
        )
        .route("/office/docs/new", axum::routing::get(documents::new_doc))
        .route(
            "/office/docs/{slug}",
            axum::routing::get(documents::edit)
                .put(documents::update)
                .delete(documents::delete),
        )
        .route(
            "/office/docs/{slug}/preview",
            axum::routing::get(documents::preview),
        )
        .route(
            "/office/docs/{slug}/export",
            axum::routing::get(documents::export),
        )
        // Spreadsheets
        .route(
            "/office/sheets",
            axum::routing::get(spreadsheets::list).post(spreadsheets::create),
        )
        .route(
            "/office/sheets/new",
            axum::routing::get(spreadsheets::new_sheet),
        )
        .route(
            "/office/sheets/{slug}",
            axum::routing::get(spreadsheets::edit),
        )
        .route(
            "/office/sheets/{slug}/import",
            axum::routing::post(spreadsheets::import),
        )
        .route(
            "/office/sheets/{slug}/export",
            axum::routing::get(spreadsheets::export),
        )
        .route(
            "/office/sheets/{slug}/delete",
            axum::routing::post(spreadsheets::delete),
        )
        // Presentations
        .route(
            "/office/presentations",
            axum::routing::get(presentations::list).post(presentations::create),
        )
        .route(
            "/office/presentations/new",
            axum::routing::get(presentations::new_pres),
        )
        .route(
            "/office/presentations/{slug}",
            axum::routing::get(presentations::edit).delete(presentations::delete),
        )
        .route(
            "/office/presentations/{slug}/present",
            axum::routing::get(presentations::present),
        )
        .route(
            "/office/presentations/{slug}/slides",
            axum::routing::post(presentations::add_slide),
        )
        .route(
            "/office/presentations/{slug}/slides/{index}",
            axum::routing::put(presentations::update_slide).delete(presentations::delete_slide),
        )
        .route(
            "/office/presentations/{slug}/slides/reorder",
            axum::routing::post(presentations::reorder),
        )
        // Library
        .route("/office/library", axum::routing::get(library::list))
        .route(
            "/office/library/search",
            axum::routing::get(library::search),
        )
        .route(
            "/office/library/import/{type}",
            axum::routing::post(library::import),
        )
        .route(
            "/office/library/{type}/{name}/download",
            axum::routing::get(library::download),
        )
        .route(
            "/office/library/{type}/{name}/delete",
            axum::routing::post(library::delete),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_auth_state,
        ));

    // Endpoints JSON de la suite de oficina (descargas). Viven aparte porque
    // `require_auth_state` redirige a `/login` (HTML) en las rutas que no
    // empiezan por `/api/`: una descarga sin sesión debe responder 401 + JSON,
    // nunca una página de login. Ver `require_auth_json`.
    let json_api: Router<Arc<AppState>> = Router::new()
        // Informe del estudiante (.docx vía MCP de GenOffice)
        .route("/office/report", axum::routing::get(report::download))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_auth_json,
        ));

    router.merge(office).merge(json_api)
}

/// Autenticación para los endpoints JSON de la suite de oficina.
///
/// Reutiliza la validación de sesión de [`require_auth_state`] (misma tabla
/// `sessions`, mismo comprovado de expiración) y traduce su 302→`/login` en
/// un **401 + JSON**: un endpoint de descarga nunca devuelve HTML.
///
/// Fallo cerrado: sin cookie, con cookie inválida o con la base de datos
/// caída, la respuesta es 401 (no se sirve el documento).
async fn require_auth_json(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let resp = require_auth_state(State(state), req, next).await;
    if resp.status() == StatusCode::FOUND {
        // 302 → /login: el middleware no distingue el motivo, así que el
        // 401 los agrupa ("ausente, inválida o expirada").
        return unauthorized("Sesión ausente, inválida o expirada");
    }
    resp
}

/// Respuesta 401 en JSON (nunca HTML).
fn unauthorized(motivo: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "No autorizado", "motivo": motivo })),
    )
        .into_response()
}
