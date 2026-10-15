//! Frontend StudyOS — interfaz principal de la plataforma.
//!
//! Sirve la interfaz StudyOS (HTML/CSS/JS puro) en la raíz de la aplicación:
//!   - GET /       -> index.html (con estado de autenticación inyectado)
//!   - GET /dashboard -> index.html (misma página SPA)
//!   - GET /styles.css -> styles.css
//!   - GET /app.js   -> app.js
//!
//! El login/registro están en /login y /register. Las rutas /app/* y /api/*
//! requieren autenticación (middleware require_auth_state).

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{State, Request},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Response},
};

use crate::middleware::auth::extract_session_cookie;
use crate::AppState;
use std::io::Read;

/// Serve the StudyOS index page on the root and dashboard routes.
pub async fn index(State(_state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let html = include_str!("../../static/studyos/index.html");
    let authenticated = extract_session_cookie(&req).is_some();

    let auth_script = format!(
        r###"<script>
        (function() {{
            window.__STUDYOS_AUTH__ = {{
                authenticated: {authenticated},
                loginUrl: '/login',
                logoutUrl: '/logout'
            }};
        }})();
        </script>"###,
        authenticated = authenticated
    );

    let html = html.replace("<head>", &format!("<head>{}", auth_script));

    (StatusCode::OK, html).into_response()
}

/// Serve styles.css.
pub async fn styles(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut content = String::new();
    std::fs::File::open("static/studyos/styles.css")
        .expect("static/styles.css")
        .read_to_string(&mut content)
        .expect("read styles.css");
    (
        [(http::header::CONTENT_TYPE, "text/css; charset=utf-8")],
        content,
    )
        .into_response()
}

/// Serve app.js.
pub async fn app(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut content = String::new();
    std::fs::File::open("static/studyos/app.js")
        .expect("static/app.js")
        .read_to_string(&mut content)
        .expect("read app.js");
    (
        [(http::header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        content,
    )
        .into_response()
}
