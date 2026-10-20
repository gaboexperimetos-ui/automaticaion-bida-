//! Study Platform UI — interfaz frontend descargada (`study-platform-ui.zip`).
//!
//! SPA en HTML/CSS/JS puro (sin framework ni llamadas a backend en su versión demo;
//! datos en localStorage del navegador). Se sirve junto con la ruta `/` como interfaz
//! principal de la aplicación.
//!
//! Referencias útiles (dentro del propio paquete): README.md
//! 1. Servir los tres archivos desde el mismo origen que el backend.
//! 2. Ajustar el router para servir index.html, styles.css y app.js, preservando
//!    `/health` y las rutas existentes.
//! 3. Configurar `window.STUDY_PLATFORM_API_BASE` con la URL correcta de la API.

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderValue, StatusCode},
    response::Response,
};
use std::sync::Arc;

use crate::AppState;

/// Serves `index.html` (route `/legacy`) as the legacy Study Platform SPA.
pub async fn serve_index(State(_state): State<Arc<AppState>>) -> Response<Body> {
    let content = include_bytes!("../../static/study-platform-ui/index.html");
    make_response(content, "text/html; charset=utf-8")
}

/// Serves `styles.css`.
pub async fn serve_css(State(_state): State<Arc<AppState>>) -> Response<Body> {
    let content = include_bytes!("../../static/study-platform-ui/styles.css");
    make_response(content, "text/css; charset=utf-8")
}

/// Serves `app.js`.
pub async fn serve_js(State(_state): State<Arc<AppState>>) -> Response<Body> {
    let content = include_bytes!("../../static/study-platform-ui/app.js");
    make_response(content, "application/javascript; charset=utf-8")
}

/// Serves the Stitch v2 interactivity layer (`static/app/shell.js`) at
/// `/shell.js`. La inyecta `stitch_v2_page` al final del `<body>`; el propio
/// shell crea sus componentes (toasts, menús, editor de notas, asistente).
pub async fn serve_shell_js(State(_state): State<Arc<AppState>>) -> Response<Body> {
    let content = include_bytes!("../../static/app/shell.js");
    make_response_with_cache(content, "application/javascript; charset=utf-8")
}

fn make_response(content: &[u8], content_type: &'static str) -> Response<Body> {
    let mut resp = Response::new(Body::from(content.to_vec()));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    resp
}

/// Igual que [`make_response`] pero con caché de sesión (no agresiva): el
/// archivo cambia con cada build, así que basta con revalidar.
fn make_response_with_cache(content: &[u8], content_type: &'static str) -> Response<Body> {
    let mut resp = make_response(content, content_type);
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    resp
}
