//! Estudio platform UI v2 — interfaz "Synthetic Cognition" (kit stitch_study_platform_ui_system).
//!
//! Interfaz principal basada en el kit Stitch v2.4. Rutas:
//!   - GET / -> panel principal (dashboard)
//!   - GET /dashboard -> panel principal; `?partial=...` -> academic dashboard
//!   - GET /dashboard/ai -> asistente academico
//!   - GET /dashboard/notes -> segundo cerebro (notas markdown)
//!   - GET /dashboard/mcp -> centro MCP + configuracion OpenRouter

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{State, Request},
    http::StatusCode,
    response::{Html, IntoResponse},
};

use crate::middleware::auth::extract_session_cookie;
use crate::AppState;

/// Mapeo de las paginas del nav-rail a sus rutas en la aplicacion.
///
/// Every target must be an existing route (verified by integration check);
/// menu items without a dedicated page point at the closest real screen.
const NAV_LINKS: &[(&str, &str)] = &[
    ("panel-principal", "/"),
    ("mi-agenda", "/dashboard/calendar"),
    ("planificacion", "/dashboard/planner"),
    ("asignaturas", "/dashboard/subjects"),
    ("tareas-y-actividades", "/dashboard/planner"),
    ("examenes", "/dashboard/calendar"),
    ("horarios", "/dashboard/horario"),
    ("asistente-academico", "/dashboard/ai"),
    ("conversaciones", "/ai"),
    ("generador-de-material", "/ai"),
    ("segundo-cerebro", "/dashboard/notes"),
    ("mis-notas", "/dashboard/notes"),
    ("busqueda-global", "/dashboard"),
    ("documentos", "/office/docs"),
    ("hojas-de-calculo", "/office/sheets"),
    ("presentaciones", "/office/presentations"),
    ("biblioteca-de-archivos", "/office/library"),
    ("centro-mcp", "/dashboard/mcp"),
    ("estado-de-conexiones", "/dashboard/mcp"),
    ("administracion", "/dashboard/profile"),
    ("configuracion", "/dashboard/profile"),
];

const STITCH_V2_HEAD: &str = include_str!("../../static/stitch-v2/shell/head.html");
const STITCH_V2_ASIDE: &str = include_str!("../../static/stitch-v2/shell/aside.html");
const STITCH_V2_TOPBAR: &str = include_str!("../../static/stitch-v2/shell/topbar.html");
const STITCH_V2_PL_WRAPPER: &str = "<div class=\"pl-[250px]\">";
const STITCH_V2_MAIN_OPEN: &str = "<main class=\"relative pt-16 w-full min-h-screen bg-background\">";
const STITCH_V2_MAIN_CLOSE: &str = "</main>";

/// Contenido de cada pagina (contenido del <main> sin wrappers).
const CONTENT_INDEX: &str = include_str!("../../static/stitch-v2/_content/clean/index.html");
const CONTENT_DASHBOARD: &str = include_str!("../../static/stitch-v2/_content/clean/dashboard.html");
const CONTENT_ASISTENTE: &str = include_str!("../../static/stitch-v2/_content/clean/asistente.html");
const CONTENT_NOTES: &str = include_str!("../../static/stitch-v2/_content/clean/notes.html");
const CONTENT_MCP: &str = include_str!("../../static/stitch-v2/_content/clean/mcp.html");

/// Script que inyecta el estado de autenticacion para el frontend.
const STITCH_V2_AUTH_SCRIPT: &str = "<script>\n(function(){\n  var AUTH = window.__STITCH_V2_AUTH__ || {authenticated:false,loginUrl:'/login',logoutUrl:'/logout'};\n  var loggedIn = AUTH.authenticated;\n  document.addEventListener('click', function(e){\n    var topProfile = document.querySelector('#topProfile');\n    if(topProfile && e.target === topProfile){\n      if(loggedIn){\n        fetch(AUTH.logoutUrl,{method:'POST',credentials:'include'}).then(function(){location.reload();});\n      }else{\n        location.href=AUTH.loginUrl;\n      }\n    }\n  });\n})();\n</script>";

/// Actualiza el nav-rail: asigna rutas a los enlaces y marca el item activo.
fn update_nav_rail(aside: &str, active: &str) -> String {
    let mut result = aside.to_string();

    // 1. Asignar rutas a los enlaces (data-path -> href).
    for &(path, route) in NAV_LINKS {
        result = result.replace(
            &format!("data-path=\"{}\" href=\"#\"", path),
            &format!("data-path=\"{}\" href=\"{}\"", path, route),
        );
    }

    // 2. Desactivar todos los items: las clases de "activo" no deben aplicarse.
    result = result.replace("bg-primary-container text-on-primary-container font-semibold", "");
    result = result.replace(
        "px-space-sm py-1.5 rounded transition-all",
        "px-space-sm py-1.5 rounded text-on-surface-variant hover:bg-surface-container-high hover:text-on-surface transition-all",
    );
    // eliminar dobles espacios resultantes
    result = result.replace("  ", " ");

    // 3. Marcar el item activo con sus clases y aria-current.
    let active_class = "flex items-center gap-space-xs px-space-sm py-1.5 rounded transition-all bg-primary-container text-on-primary-container font-semibold";
    let route = NAV_LINKS.iter().find(|(p, _)| *p == active).map(|(_, r)| *r).unwrap_or("/");
    result = result.replace(
        &format!("data-path=\"{}\" href=\"{}\"", active, route),
        &format!("data-path=\"{}\" href=\"{}\" class=\"{}\" aria-current=\"page\"", active, route, active_class),
    );

    result
}

/// Construye la pagina completa: head + shell + contenido + script de auth.
fn stitch_v2_page(active: &str, content: &str, authenticated: bool) -> String {
    // Real <head> (fonts, Tailwind CDN, design tokens) plus the early auth
    // flag; omitting STITCH_V2_HEAD here rendered every page unstyled.
    let head_with_auth = format!(
        "<head>{head}<script>window.__STITCH_V2_AUTH__={{\"authenticated\":{authenticated}}}</script></head>",
        head = STITCH_V2_HEAD,
        authenticated = authenticated,
    );

    let aside = update_nav_rail(STITCH_V2_ASIDE, active);
    let body_classes = "bg-background font-body-md text-body-md text-on-surface antialiased";

    format!(
        r###"<!DOCTYPE html>
<html class="dark" lang="es">{head}
<body class="{body_classes}">{aside}{pl_wrapper}{topbar}{main_open}{content}{main_close}
{auth_script}
</body>
</html>"###,
        head = head_with_auth,
        aside = aside,
        pl_wrapper = STITCH_V2_PL_WRAPPER,
        topbar = STITCH_V2_TOPBAR,
        main_open = STITCH_V2_MAIN_OPEN,
        content = content,
        main_close = STITCH_V2_MAIN_CLOSE,
        auth_script = STITCH_V2_AUTH_SCRIPT,
    )
}

/// GET / -> panel principal (dashboard).
pub async fn stitch_index(State(_state): State<Arc<AppState>>, req: Request<Body>) -> impl IntoResponse {
    let authenticated = extract_session_cookie(&req).is_some();
    let html = stitch_v2_page("panel-principal", CONTENT_INDEX, authenticated);
    (StatusCode::OK, Html(html))
}

/// GET /dashboard -> panel principal.
///
/// HTMX partial reloads (`?partial=stats|profile|events|tasks`) issued by
/// the legacy academic dashboard templates are delegated to
/// [`crate::academic::dashboard::index`], which renders those fragments.
pub async fn stitch_dashboard(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> axum::response::Response {
    use axum::extract::FromRequest;

    if req.uri().query().is_some_and(|q| q.contains("partial=")) {
        let parsed = axum::extract::Query::<
            crate::academic::dashboard::DashboardQuery,
        >::from_request(req, &state);
        return match parsed.await {
            Ok(query) => {
                crate::academic::dashboard::index(State(state), query).await.into_response()
            }
            Err(rejection) => rejection.into_response(),
        };
    }

    let authenticated = extract_session_cookie(&req).is_some();
    let html = stitch_v2_page("panel-principal", CONTENT_DASHBOARD, authenticated);
    (StatusCode::OK, Html(html)).into_response()
}

/// GET /dashboard/ai -> asistente academico.
pub async fn stitch_asistente(State(_state): State<Arc<AppState>>, req: Request<Body>) -> impl IntoResponse {
    let authenticated = extract_session_cookie(&req).is_some();
    let html = stitch_v2_page("asistente-academico", CONTENT_ASISTENTE, authenticated);
    (StatusCode::OK, Html(html))
}

/// GET /dashboard/notes -> segundo cerebro (notas markdown).
pub async fn stitch_notes(State(_state): State<Arc<AppState>>, req: Request<Body>) -> impl IntoResponse {
    let authenticated = extract_session_cookie(&req).is_some();
    let html = stitch_v2_page("segundo-cerebro", CONTENT_NOTES, authenticated);
    (StatusCode::OK, Html(html))
}

/// GET /dashboard/mcp -> centro MCP + configuracion OpenRouter.
pub async fn stitch_mcp(State(_state): State<Arc<AppState>>, req: Request<Body>) -> impl IntoResponse {
    let authenticated = extract_session_cookie(&req).is_some();
    let html = stitch_v2_page("centro-mcp", CONTENT_MCP, authenticated);
    (StatusCode::OK, Html(html))
}
