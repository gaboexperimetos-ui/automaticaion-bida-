//! Rich Markdown document editor — server-rendered with HTMX + pulldown-cmark.
//!
//! Each document is a single `data/office/docs/{slug}.md` file. The toolbar
//! injects Markdown syntax into the `<textarea>`; preview and export render
//! that Markdown to HTML with `pulldown-cmark`.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Form,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::office::templates::{layout, layout_as, md_to_html, slugify};
use crate::office::{docs_dir, templates};
use crate::AppState;

/// Input bounds (400 on violation).
const MAX_TITLE_LEN: usize = 200;
const MAX_CONTENT_LEN: usize = 500_000;

/// Generic public message for 500-class failures (never leaks internals).
const INTERNAL_ERROR_HTML: &str =
    "<p class=\"text-red-400\">No se pudo completar la operación.</p>";

/// Shared form payload for creating and updating documents.
#[derive(Deserialize)]
pub struct DocForm {
    #[serde(default)]
    pub title: String,
    pub content: String,
}

#[derive(Deserialize)]
pub struct ExportQuery {
    pub fmt: Option<String>,
}

#[derive(Deserialize)]
pub struct EditQuery {
    pub saved: Option<String>,
}

/// Office dashboard landing page.
pub async fn office_home(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let dir = docs_dir(&state);
    let docs = templates::count_files(&dir, "md");
    let sheets = templates::count_files(&crate::office::sheets_dir(&state), "csv");
    let pres = templates::count_files(&crate::office::pres_dir(&state), "json");

    // Sondeo honesto del MCP de GenOffice (cached, 30s TTL).
    let genoffice_ms = templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    let body = format!(
        r#"<div class="nova-page nova-office-home"><header class="nova-office-hero"><p class="nova-eyebrow"><span>07</span> Oficina</p><h1>Haz el trabajo.<br><em>Nosotros lo guardamos.</em></h1><p>Un espacio local para escribir, calcular y presentar sin cambiar de aplicación.</p>{mcp}</header><div class="nova-office-app-grid"><a href="/office/docs" class="nova-office-app-card documents"><span class="nova-office-app-icon"><span class="material-symbols-outlined">edit_note</span></span><span class="nova-office-app-number">01</span><h2>Documentos</h2><p>Editor Markdown con vista previa y exportación HTML.</p><footer><strong>{docs}</strong> archivos<span class="material-symbols-outlined">arrow_forward</span></footer></a><a href="/office/sheets" class="nova-office-app-card sheets"><span class="nova-office-app-icon"><span class="material-symbols-outlined">table_chart</span></span><span class="nova-office-app-number">02</span><h2>Hojas de cálculo</h2><p>Cuadrícula editable, CSV y fórmulas SUM y AVG.</p><footer><strong>{sheets}</strong> archivos<span class="material-symbols-outlined">arrow_forward</span></footer></a><a href="/office/presentations" class="nova-office-app-card presentations"><span class="nova-office-app-icon"><span class="material-symbols-outlined">present_to_all</span></span><span class="nova-office-app-number">03</span><h2>Presentaciones</h2><p>Diapositivas Markdown listas para ordenar y presentar.</p><footer><strong>{pres}</strong> archivos<span class="material-symbols-outlined">arrow_forward</span></footer></a></div><a href="/office/library" class="nova-office-library-link"><span class="material-symbols-outlined">folder_open</span><div><h2>Biblioteca de archivos</h2><p>Busca, importa, descarga y organiza todo tu trabajo desde un solo lugar.</p></div><span class="material-symbols-outlined">arrow_outward</span></a></div>"#,
        docs = docs,
        sheets = sheets,
        pres = pres,
        mcp = mcp_banner,
    );

    layout_as("Suite de oficina", "", &body)
}

/// List all documents.
pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let dir = docs_dir(&state);

    // Honest source-of-truth: si la tabla `office_files` existe en la BD,
    // usamos los metadatos allí registrados; si no, usamos el filesystem.
    // Nunca inventamos documentos.
    let use_db = templates::office_files_table_exists(&state.db).await;
    let files = if use_db {
        templates::list_files_from_db(&state.db, "docs").await
    } else {
        templates::list_files(&dir, "md")
    };

    // Sondeo honesto del MCP de GenOffice (cached, 30s TTL).
    let genoffice_ms = templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    let cards = if files.is_empty() {
        r#"<div class="nova-office-empty"><span class="material-symbols-outlined">edit_note</span><h2>Tu próximo documento empieza aquí.</h2><p>Crea un archivo Markdown y edítalo desde el navegador.</p><a href="/office/docs/new" class="nova-button nova-button-primary">Crear documento</a></div>"#.to_string()
    } else {
        files.iter().map(|f| {
            format!(
                r#"<article class="nova-file-card"><div class="nova-file-card-top"><span class="nova-file-type"><span class="material-symbols-outlined">description</span></span><span>MARKDOWN</span></div><h2><a href="/office/docs/{slug}">{display}</a></h2><p class="nova-file-meta">{modified} · {size}</p><footer><div><a href="/office/docs/{slug}/export?fmt=html">HTML</a><a href="/office/docs/{slug}/export?fmt=md">MD</a></div><form hx-delete="/office/docs/{slug}"><button type="submit" onclick="return confirm('¿Borrar?')" class="nova-icon-button danger" aria-label="Borrar {display}"><span class="material-symbols-outlined">delete</span></button></form></footer></article>"#,
                slug = templates::html_escape(&f.name),
                display = templates::html_escape(&f.display),
                size = templates::human_size(f.size),
                modified = &f.modified,
            )
        }).collect::<Vec<_>>().join("
")
    };

    let body = format!(
        r#"<div class="nova-page nova-office-page"><header class="nova-page-head compact"><div><p class="nova-eyebrow"><span>07.1</span> Oficina</p><h1>Documentos que<br><em>siguen tu ritmo.</em></h1><p class="nova-page-lead">{count} documento(s) en Markdown, guardados en tu espacio local.</p></div><a href="/office/docs/new" class="nova-button nova-button-primary"><span class="material-symbols-outlined">add</span>Nuevo documento</a></header><nav class="nova-office-switch" aria-label="Aplicaciones de oficina"><a href="/office/docs" aria-current="page">Documentos</a><a href="/office/sheets">Hojas</a><a href="/office/presentations">Presentaciones</a><a href="/office/library">Biblioteca</a></nav>{mcp}<section class="nova-file-grid">{cards}</section></div>"#,
        count = files.len(),
        cards = cards,
        mcp = mcp_banner,
    );

    layout_as("Documentos", "documentos", &body)
}

/// New document form.
pub async fn new_doc() -> impl IntoResponse {
    let body = r#"<div class="nova-page nova-editor-page"><header class="nova-page-head compact"><div><p class="nova-eyebrow"><span>07.1</span> Nuevo documento</p><h1>Una página en blanco.<br><em>Muchas posibilidades.</em></h1></div><a href="/office/docs" class="nova-button nova-button-quiet">Volver</a></header><form hx-post="/office/docs" hx-swap="outerHTML" class="nova-editor-form"><div class="nova-editor-meta"><label for="new-doc-title">Título del documento</label><input id="new-doc-title" type="text" name="title" placeholder="Ej. Apuntes de anatomía" required></div><div class="nova-editor-canvas"><label for="editor">Contenido Markdown</label><textarea name="content" id="editor" rows="24" placeholder="Empieza a escribir en Markdown…"></textarea></div><div class="nova-editor-submit"><span>Se guardará como archivo .md</span><button type="submit" class="nova-button nova-button-primary">Crear documento<span class="material-symbols-outlined">arrow_forward</span></button></div></form></div>
<script>"#;
    let mut out = body.to_string();
    out.push_str(templates::MARKDOWN_TOOLBAR_JS);
    out.push_str("</script>");
    layout_as("Nuevo documento", "documentos", &out)
}

/// Render the editor page (view + edit) for a document.
pub async fn edit(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Query(q): Query<EditQuery>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout("Error", "<p class=\"text-red-400\">Nombre no válido.</p>"),
        )
            .into_response();
    }
    let file_path = docs_dir(&state).join(format!("{}.md", slug));
    if !file_path.exists() {
        return (
            axum::http::StatusCode::NOT_FOUND,
            layout(
                "No encontrado",
                "<p class=\"text-gray-500\">Documento no encontrado.</p>",
            ),
        )
            .into_response();
    }
    let content = templates::read_text(&file_path);
    let saved_banner = if q.saved.is_some() {
        r#"<div class="bg-green-900/40 border border-green-700 text-green-300 px-3 py-2 rounded mb-3 text-sm">✓ Documento guardado</div>"#
    } else {
        ""
    };

    // Sondeo honesto del MCP de GenOffice (cached, 30s TTL).
    let genoffice_ms = templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    let editor = doc_editor(&slug, &content);
    let body = format!(
        r#"<div class="nova-page nova-editor-page"><header class="nova-workspace-head"><div class="nova-workspace-title"><span class="nova-workspace-symbol"><span class="material-symbols-outlined">edit_note</span></span><div><p>Documento Markdown</p><h1>{title}</h1></div></div><div class="nova-workspace-actions"><a href="/office/docs/{slug}/preview" target="_blank" class="nova-button nova-button-quiet">Vista previa</a><a href="/office/docs/{slug}/export?fmt=md" class="nova-button nova-button-quiet">Exportar .md</a><a href="/office/docs/{slug}/export?fmt=html" class="nova-button nova-button-primary">Exportar .html</a></div></header>{saved}{mcp}<section class="nova-document-workspace">{editor}</section></div>"#,
        saved = saved_banner,
        mcp = mcp_banner,
        title = templates::html_escape(&slug.replace('-', " ")),
        slug = templates::html_escape(&slug),
        editor = editor,
    );

    layout_as(&format!("Editar: {}", slug), "documentos", &body).into_response()
}

/// The editor widget (toolbar + textarea + save).
fn doc_editor(slug: &str, content: &str) -> String {
    format!(
        r#"<div class="nova-markdown-toolbar" role="toolbar" aria-label="Formato Markdown">
            <button type="button" onclick="insertMd('**','**')" title="Negrita"><span class="material-symbols-outlined">format_bold</span></button>
            <button type="button" onclick="insertMd('*','*')" title="Cursiva"><span class="material-symbols-outlined">format_italic</span></button>
            <button type="button" onclick="appendMd('## ')" title="Encabezado"><span class="material-symbols-outlined">title</span></button>
            <button type="button" onclick="appendMd('- ')" title="Lista"><span class="material-symbols-outlined">format_list_bulleted</span></button>
            <button type="button" onclick="appendMd('1. ')" title="Lista numerada"><span class="material-symbols-outlined">format_list_numbered</span></button>
            <button type="button" onclick="insertMd('`','`')" title="Código"><span class="material-symbols-outlined">code</span></button>
            <button type="button" onclick="appendMd('```

```')" title="Bloque de código"><span class="material-symbols-outlined">data_object</span></button>
            <button type="button" onclick="insertMd('[', '](url)')" title="Enlace"><span class="material-symbols-outlined">link</span></button>
            <span class="nova-toolbar-hint">Markdown</span>
        </div>
        <form hx-put="/office/docs/{slug}" hx-swap="outerHTML" class="nova-document-edit-form">
            <label class="sr-only" for="editor">Contenido del documento</label>
            <textarea name="content" id="editor" rows="30" spellcheck="true">{content}</textarea>
            <div class="nova-editor-submit"><span>Los cambios se guardan en el archivo local.</span><button type="submit" class="nova-button nova-button-primary"><span class="material-symbols-outlined">save</span>Guardar cambios</button></div>
        </form>
        <script>{js}</script>"#,
        slug = slug,
        content = templates::html_escape(content),
        js = templates::MARKDOWN_TOOLBAR_JS,
    )
}

/// Create a document.
pub async fn create(
    State(state): State<Arc<AppState>>,
    Form(form): Form<DocForm>,
) -> impl IntoResponse {
    if form.title.trim().is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout(
                "Error",
                "<p class=\"text-red-400\">El título es obligatorio.</p>",
            ),
        )
            .into_response();
    }
    if form.title.len() > MAX_TITLE_LEN {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout(
                "Error",
                "<p class=\"text-red-400\">El título es demasiado largo.</p>",
            ),
        )
            .into_response();
    }
    if form.content.len() > MAX_CONTENT_LEN {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout(
                "Error",
                "<p class=\"text-red-400\">El contenido es demasiado grande.</p>",
            ),
        )
            .into_response();
    }
    let slug = slugify(&form.title);
    let file_path = docs_dir(&state).join(format!("{}.md", slug));
    if file_path.exists() {
        return (
            axum::http::StatusCode::CONFLICT,
            layout(
                "Conflicto",
                "<p class=\"text-red-400\">Ya existe un documento con ese título.</p>",
            ),
        )
            .into_response();
    }
    if let Err(e) = templates::write_text(&file_path, &form.content) {
        tracing::error!(
            slug_len = slug.len(),
            "office docs create: write failed: {}",
            e
        );
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            layout("Error", INTERNAL_ERROR_HTML),
        )
            .into_response();
    }

    // Si la tabla `office_files` existe, registramos el índice sin tocar el
    // archivo en disco (el .md real ya está escrito arriba).
    if templates::office_files_table_exists(&state.db).await {
        let _ = sqlx::query(
            r#"
            INSERT INTO office_files (slug, kind, filename, size, modified_at)
            VALUES (?, 'docs', ?, ?, datetime('now'))
            "#,
        )
        .bind(&slug)
        .bind(format!("{}.md", slug))
        .bind(form.content.len() as i64)
        .execute(&state.db)
        .await;
    }

    templates::hx_redirect(&format!("/office/docs/{}?saved=1", slug))
}

/// Update document content.
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Form(form): Form<DocForm>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout("Error", "<p class=\"text-red-400\">Nombre no válido.</p>"),
        )
            .into_response();
    }
    let file_path = docs_dir(&state).join(format!("{}.md", slug));
    if !file_path.exists() {
        return (
            axum::http::StatusCode::NOT_FOUND,
            layout(
                "No encontrado",
                "<p class=\"text-gray-500\">Documento no encontrado.</p>",
            ),
        )
            .into_response();
    }
    if form.content.len() > MAX_CONTENT_LEN {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout(
                "Error",
                "<p class=\"text-red-400\">El contenido es demasiado grande.</p>",
            ),
        )
            .into_response();
    }
    if let Err(e) = templates::write_text(&file_path, &form.content) {
        tracing::error!(
            slug_len = slug.len(),
            "office docs update: write failed: {}",
            e
        );
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            layout("Error", INTERNAL_ERROR_HTML),
        )
            .into_response();
    }
    templates::hx_redirect(&format!("/office/docs/{}?saved=1", slug))
}

/// Delete a document.
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
    let file_path = docs_dir(&state).join(format!("{}.md", slug));
    let _ = std::fs::remove_file(&file_path);

    // Si la tabla `office_files` existe, mantén el índice sincronizado.
    if templates::office_files_table_exists(&state.db).await {
        let _ = sqlx::query("DELETE FROM office_files WHERE kind = 'docs' AND slug = ?")
            .bind(&slug)
            .execute(&state.db)
            .await;
    }

    templates::hx_redirect("/office/docs")
}

/// HTML preview of the Markdown.
pub async fn preview(
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
    let file_path = docs_dir(&state).join(format!("{}.md", slug));
    if !file_path.exists() {
        return (
            axum::http::StatusCode::NOT_FOUND,
            layout(
                "No encontrado",
                "<p class=\"text-gray-500\">Documento no encontrado.</p>",
            ),
        )
            .into_response();
    }
    let content = templates::read_text(&file_path);
    let html = md_to_html(&content);

    // Sondeo honesto del MCP de GenOffice (cached, 30s TTL).
    let genoffice_ms = templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    let body = format!(
        r#"<div class="nova-page nova-preview-page"><header class="nova-workspace-head"><div class="nova-workspace-title"><span class="nova-workspace-symbol"><span class="material-symbols-outlined">visibility</span></span><div><p>Vista previa</p><h1>{title}</h1></div></div><div class="nova-workspace-actions"><a href="/office/docs/{slug}" class="nova-button nova-button-quiet">Volver al editor</a><a href="/office/docs/{slug}/export?fmt=html" class="nova-button nova-button-primary">Exportar HTML</a></div></header>{mcp}<article class="nova-document-preview">{html}</article></div>"#,
        mcp = mcp_banner,
        title = templates::html_escape(&slug.replace('-', " ")),
        slug = templates::html_escape(&slug),
        html = html,
    );
    layout_as(&format!("Vista: {}", slug), "documentos", &body).into_response()
}

/// Export a document as `.md` or `.html`.
pub async fn export(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Query(q): Query<ExportQuery>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid name").into_response();
    }
    let file_path = docs_dir(&state).join(format!("{}.md", slug));
    if !file_path.exists() {
        return (axum::http::StatusCode::NOT_FOUND, "document not found").into_response();
    }
    let content = templates::read_text(&file_path);
    let fmt = q.fmt.as_deref().unwrap_or("md");
    match fmt {
        "html" => {
            let html = md_to_html(&content);
            let full = format!(
                "<!DOCTYPE html>\n<html lang=\"es\"><head><meta charset=\"UTF-8\"><title>{title}</title></head><body>{body}</body></html>\n",
                title = templates::html_escape(&slug),
                body = html,
            );
            templates::file_download(
                &format!("{}.html", slug),
                templates::mime_for_ext("html"),
                full.into_bytes(),
            )
        }
        _ => templates::file_download(
            &format!("{}.md", slug),
            templates::mime_for_ext("md"),
            content.into_bytes(),
        ),
    }
}
