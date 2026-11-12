//! Editor web de páginas HTML con vista en vivo aislada.
//!
//! Los archivos se guardan como HTML normal en `data/office/html`. La vista
//! previa usa un iframe sandboxed, sin scripts ni acceso al origen de NOVA.

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Form,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::office::html_dir;
use crate::office::templates::{self, hx_redirect, layout, layout_as};
use crate::AppState;

const MAX_TITLE_LEN: usize = 200;
const MAX_HTML_LEN: usize = 1024 * 1024;
const INTERNAL_ERROR_HTML: &str =
    "<p class=\"text-red-400\">No se pudo completar la operación.</p>";

#[derive(Deserialize)]
pub struct HtmlForm {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
}

/// Lista páginas HTML guardadas localmente.
pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let files = templates::list_files(&html_dir(&state), "html");
    let cards = if files.is_empty() {
        r##"<div class="gen-office-empty"><span class="material-symbols-outlined">code</span><h3>Un lienzo web en blanco.</h3><p>Crea una página HTML y edita el código con vista previa en vivo.</p><a href="/office/html/new" class="gen-office-primary-button">Crear página</a></div>"##.to_string()
    } else {
        files
            .iter()
            .map(|file| {
                format!(
                    r##"<article class="gen-office-file-card html"><a class="gen-office-file-preview" href="/office/html/{slug}"><span class="material-symbols-outlined">code</span><small>HTML</small></a><div class="gen-office-file-copy"><a href="/office/html/{slug}">{display}</a><span>{modified} · {size}</span></div><footer><a href="/office/html/{slug}/export">Descargar HTML</a><form hx-delete="/office/html/{slug}"><button type="submit" onclick="return confirm('¿Borrar esta página?')" class="gen-office-icon-button danger" aria-label="Borrar {display}"><span class="material-symbols-outlined">delete</span></button></form></footer></article>"##,
                    slug = templates::html_escape(&file.name),
                    display = templates::html_escape(&file.display),
                    size = templates::human_size(file.size),
                    modified = templates::html_escape(&file.modified),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-manager-page">{tabs}<header class="gen-office-manager-head"><div><span>PÁGINAS WEB · HTML</span><h1>Diseña y previsualiza.</h1><p>{count} página(s) HTML guardadas en tu espacio local.</p></div><a href="/office/html/new" class="gen-office-primary-button"><span class="material-symbols-outlined">add</span>Nueva página</a></header><nav class="gen-office-appnav" aria-label="Aplicaciones de oficina"><a href="/office/docs"><span class="material-symbols-outlined">description</span>Documentos</a><a href="/office/sheets"><span class="material-symbols-outlined">table_chart</span>Hojas</a><a href="/office/presentations"><span class="material-symbols-outlined">present_to_all</span>Presentaciones</a><a class="is-active html" href="/office/html"><span class="material-symbols-outlined">code</span>HTML</a><a href="/office/library"><span class="material-symbols-outlined">folder_open</span>Biblioteca</a></nav><section class="gen-office-file-grid">{cards}</section><footer class="gen-office-home-footer"><span><span class="material-symbols-outlined">hard_drive</span> Archivos HTML locales</span><span>Vista previa con sandbox</span></footer></div>"##,
        tabs = templates::office_tab_strip("html", "Páginas HTML", "/office/html/new"),
        count = files.len(),
        cards = cards,
    );
    templates::layout_as_shelled(&state, "Páginas HTML", "html", &body).await
}

/// Formulario de creación de una página HTML.
pub async fn new_page() -> impl IntoResponse {
    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-create-page">{tabs}<main class="gen-office-create-shell"><div class="gen-office-create-heading"><span class="gen-office-launcher-icon html"><span class="material-symbols-outlined">code</span></span><div><span>NUEVA PÁGINA HTML</span><h1>Tu próximo diseño web.</h1><p>Escribe HTML y CSS, y comprueba el resultado antes de guardar.</p></div></div><form hx-post="/office/html" hx-swap="outerHTML" class="gen-office-create-form"><div class="gen-office-create-fields"><label for="new-html-title">Nombre del archivo</label><input id="new-html-title" type="text" name="title" placeholder="Ej. Portafolio personal" required autofocus></div><div class="gen-office-create-editor"><label for="editor">Código inicial</label><textarea id="editor" name="content" rows="18" spellcheck="false" placeholder="<!doctype html>&#10;<html lang=&quot;es&quot;>&#10;  <head><meta charset=&quot;utf-8&quot;><title>Mi página</title></head>&#10;  <body><h1>Hola, mundo</h1></body>&#10;</html>"></textarea></div><footer><a href="/office/html" class="gen-office-secondary-button">Cancelar</a><span>Se guardará como .html</span><button type="submit" class="gen-office-primary-button">Crear página<span class="material-symbols-outlined">arrow_forward</span></button></footer></form></main></div>"##,
        tabs = templates::office_tab_strip("html", "Nueva página", "/office/html/new"),
    );
    layout_as("Nueva página HTML", "html", &body)
}

/// Crea una página HTML local.
pub async fn create(
    State(state): State<Arc<AppState>>,
    Form(form): Form<HtmlForm>,
) -> impl IntoResponse {
    let title = form.title.trim();
    if title.is_empty() || title.len() > MAX_TITLE_LEN || form.content.len() > MAX_HTML_LEN {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout(
                "Error",
                "<p class=\"text-red-400\">El nombre es obligatorio y el contenido no debe superar 1 MiB.</p>",
            ),
        )
            .into_response();
    }
    let slug = templates::slugify(title);
    let file_path = html_dir(&state).join(format!("{slug}.html"));
    if file_path.exists() {
        return (
            axum::http::StatusCode::CONFLICT,
            layout(
                "Conflicto",
                "<p class=\"text-red-400\">Ya existe una página con ese nombre.</p>",
            ),
        )
            .into_response();
    }
    let content = if form.content.trim().is_empty() {
        starter_html(title)
    } else {
        form.content
    };
    if let Err(error) = templates::write_text(&file_path, &content) {
        tracing::error!(
            slug_len = slug.len(),
            "office HTML create: write failed: {}",
            error
        );
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            layout("Error", INTERNAL_ERROR_HTML),
        )
            .into_response();
    }
    hx_redirect(&format!("/office/html/{slug}"))
}

/// Editor de código HTML con vista previa en un iframe aislado.
pub async fn edit(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout("Error", "<p class=\"text-red-400\">Nombre no válido.</p>"),
        )
            .into_response();
    }
    let file_path = html_dir(&state).join(format!("{slug}.html"));
    if !file_path.exists() {
        return (
            axum::http::StatusCode::NOT_FOUND,
            layout(
                "No encontrado",
                "<p class=\"text-gray-500\">Página no encontrada.</p>",
            ),
        )
            .into_response();
    }
    let content = match templates::read_text(&file_path) {
        Ok(content) => content,
        Err(error) => {
            tracing::error!(
                slug_len = slug.len(),
                "office HTML edit: read failed: {}",
                error
            );
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                layout("Error", INTERNAL_ERROR_HTML),
            )
                .into_response();
        }
    };
    let title = slug.replace('-', " ");
    let ai_content = r##"<form id="office-doc-ai-form" class="gen-office-ai-compose"><label for="office-doc-ai-prompt">Pide ayuda con tu página</label><textarea id="office-doc-ai-prompt" rows="3" maxlength="1000" placeholder="Ej. Propón una sección de presentación" required></textarea><p>La instrucción y hasta 6000 caracteres iniciales se envían sólo cuando lo solicitas. La propuesta no se inserta ni guarda automáticamente.</p><button type="submit" class="gen-office-primary-button"><span class="material-symbols-outlined">auto_awesome</span>Generar propuesta</button><p id="office-doc-ai-status" role="status" aria-live="polite"></p><label for="office-doc-ai-result">Propuesta</label><textarea id="office-doc-ai-result" rows="7" readonly placeholder="La propuesta aparecerá aquí"></textarea><button type="button" id="office-doc-ai-insert" class="gen-office-secondary-button" disabled>Insertar al final del código</button></form>"##;
    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-editor-page gen-office-html-editor">{tabs}<header class="gen-office-ribbon"><nav class="gen-office-menubar" aria-label="Acciones de la página"><a class="is-active" href="/office/html/{slug}">Código y vista previa</a><a href="/office/html">Mis páginas</a><a href="/office/html/{slug}/export">Descargar .html</a></nav><div class="gen-office-ribbon-actions">{ai_toggle}<button type="button" id="office-html-refresh" class="gen-office-ribbon-button"><span class="material-symbols-outlined">refresh</span><span>Actualizar vista previa</span></button></div></header><div class="gen-office-workspace">{ai}<main class="gen-office-editor-main"><div class="gen-office-canvas-head"><div><span>PÁGINA HTML · CÓDIGO LOCAL</span><strong>{title}</strong></div><span class="gen-office-local-badge"><span class="material-symbols-outlined">security</span>Vista previa aislada</span></div><form id="office-html-save-form" hx-put="/office/html/{slug}" class="gen-office-html-save"><div class="gen-office-html-panes"><section class="gen-office-html-source"><label for="editor"><span>HTML</span><span>UTF-8 · máx. 1 MiB</span></label><textarea id="editor" name="content" data-office-format="HTML" spellcheck="false" autocapitalize="off" autocomplete="off">{content}</textarea></section><section class="gen-office-html-preview"><header><span><i></i>Vista previa local</span><small>Scripts y red bloqueados</small></header><iframe id="office-html-preview-frame" title="Vista previa de la página HTML" sandbox="" referrerpolicy="no-referrer"></iframe></section></div><footer class="gen-office-editor-save"><span id="office-html-status" role="status" aria-live="polite"><span class="material-symbols-outlined">info</span>La vista previa está aislada y no puede acceder a NOVA.</span><button type="submit" class="gen-office-primary-button"><span class="material-symbols-outlined">save</span>Guardar HTML</button></footer></form></main></div><footer class="gen-office-statusbar"><span><span class="material-symbols-outlined">code</span> .html · UTF-8</span><span id="office-html-byte-count">0 bytes</span><span>NOVA Office</span></footer></div><script src="/office-html.js"></script><script>{workspace_js}</script>"##,
        tabs = templates::office_tab_strip("html", &title, "/office/html/new"),
        slug = templates::html_escape(&slug),
        ai_toggle = templates::OFFICE_AI_TOGGLE,
        ai = templates::office_ai_panel(
            "Asistente de la página",
            "Solicita una propuesta de código, revísala e insértala sólo si te sirve.",
            ai_content,
        ),
        title = templates::html_escape(&title),
        content = templates::html_escape(&content),
        workspace_js = templates::OFFICE_WORKSPACE_JS,
    );
    layout_as(&format!("HTML: {title}"), "html", &body).into_response()
}

/// Guarda el código HTML de una página.
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Form(form): Form<HtmlForm>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout("Error", "<p class=\"text-red-400\">Nombre no válido.</p>"),
        )
            .into_response();
    }
    if form.content.len() > MAX_HTML_LEN {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout(
                "Error",
                "<p class=\"text-red-400\">El contenido supera 1 MiB.</p>",
            ),
        )
            .into_response();
    }
    let file_path = html_dir(&state).join(format!("{slug}.html"));
    if !file_path.exists() {
        return (axum::http::StatusCode::NOT_FOUND, "Página no encontrada.").into_response();
    }
    if let Err(error) = templates::write_text(&file_path, &form.content) {
        tracing::error!(
            slug_len = slug.len(),
            "office HTML update: write failed: {}",
            error
        );
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            layout("Error", INTERNAL_ERROR_HTML),
        )
            .into_response();
    }
    hx_redirect(&format!("/office/html/{slug}"))
}

/// Descarga el HTML fuente, sin transformarlo.
pub async fn export(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid name").into_response();
    }
    let file_path = html_dir(&state).join(format!("{slug}.html"));
    match templates::read_bytes(&file_path) {
        Ok(bytes) => {
            templates::file_download(&format!("{slug}.html"), "text/html; charset=utf-8", bytes)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (axum::http::StatusCode::NOT_FOUND, "Página no encontrada.").into_response()
        }
        Err(error) => {
            tracing::error!(
                slug_len = slug.len(),
                "office HTML export: read failed: {}",
                error
            );
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo completar la operación",
            )
                .into_response()
        }
    }
}

/// Elimina una página HTML local.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout("Error", "<p class=\"text-red-400\">Nombre no válido.</p>"),
        )
            .into_response();
    }
    match std::fs::remove_file(html_dir(&state).join(format!("{slug}.html"))) {
        Ok(()) => hx_redirect("/office/html"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (axum::http::StatusCode::NOT_FOUND, "Página no encontrada.").into_response()
        }
        Err(error) => {
            tracing::error!(
                slug_len = slug.len(),
                "office HTML delete: remove failed: {}",
                error
            );
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo completar la operación",
            )
                .into_response()
        }
    }
}

fn starter_html(title: &str) -> String {
    let title = templates::html_escape(title);
    format!(
        "<!doctype html>\n<html lang=\"es\">\n<head>\n  <meta charset=\"utf-8\">\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n  <title>{title}</title>\n  <style>body{{max-width:760px;margin:10vh auto;padding:2rem;font:16px/1.6 system-ui,sans-serif;color:#182033}}h1{{font-size:clamp(2rem,6vw,4rem);letter-spacing:-.04em}}p{{color:#526078}}</style>\n</head>\n<body>\n  <main><h1>{title}</h1><p>Empieza a editar esta página en NOVA Office.</p></main>\n</body>\n</html>\n"
    )
}
