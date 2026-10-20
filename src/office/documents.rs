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

use crate::office::templates::{layout, md_to_html, slugify};
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
    let genoffice_ms =
        templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    let body = format!(
        r#"<div class="space-y-4">
            <div class="flex justify-between items-center">
                <h2 class="text-xl font-bold text-accent">Suite de oficina</h2>
                {mcp}
            </div>
            <p class="text-gray-400">Documentos, hojas de cálculo y presentaciones.</p>
            <div class="grid grid-cols-1 md:grid-cols-3 gap-4 mt-6">
                <a href="/office/docs" class="block bg-card border border-gray-800 rounded-lg p-5 hover:border-accent">
                    <span class="text-2xl">📄</span>
                    <div class="font-semibold text-white mt-2">Documentos ({docs})</div>
                    <div class="text-sm text-gray-500">Editor Markdown con vista previa.</div>
                </a>
                <a href="/office/sheets" class="block bg-card border border-gray-800 rounded-lg p-5 hover:border-accent">
                    <span class="text-2xl">📊</span>
                    <div class="font-semibold text-white mt-2">Hojas ({sheets})</div>
                    <div class="text-sm text-gray-500">Rejilla editable, CSV y fórmulas.</div>
                </a>
                <a href="/office/presentations" class="block bg-card border border-gray-800 rounded-lg p-5 hover:border-accent">
                    <span class="text-2xl">🎠</span>
                    <div class="font-semibold text-white mt-2">Presentaciones ({pres})</div>
                    <div class="text-sm text-gray-500">Diapositivas en Markdown.</div>
                </a>
            </div>
            <div class="mt-6">
                <a href="/office/library" class="text-accent hover:text-cyan-300">Ir a la biblioteca de archivos</a>
            </div>
        </div>"#,
        docs = docs,
        sheets = sheets,
        pres = pres,
        mcp = mcp_banner,
    );

    layout("Suite de oficina", &body)
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
    let genoffice_ms =
        templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    let rows = if files.is_empty() {
        r#"<p class="text-gray-500">No hay documentos aún.</p>"#.to_string()
    } else {
        files
            .iter()
            .map(|f| {
                format!(
                    r#"<tr class="border-t border-gray-800">
                        <td class="py-2"><a href="/office/docs/{slug}" class="text-accent hover:text-cyan-300">{display}</a></td>
                        <td class="py-2 text-sm text-gray-500">{size}</td>
                        <td class="py-2 text-sm text-gray-500">{modified}</td>
                        <td class="py-2 space-x-2">
                            <a href="/office/docs/{slug}/export?fmt=html" class="text-xs text-gray-400 hover:text-white">html</a>
                            <a href="/office/docs/{slug}/export?fmt=md" class="text-xs text-gray-400 hover:text-white">md</a>
                            <form hx-delete="/office/docs/{slug}" class="inline">
                                <button type="submit" onclick="return confirm('¿Borrar?')"
                                    class="text-xs text-red-400 hover:text-red-300">borrar</button>
                            </form>
                        </td>
                    </tr>"#,
                    slug = templates::html_escape(&f.name),
                    display = templates::html_escape(&f.display),
                    size = templates::human_size(f.size),
                    modified = &f.modified,
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let body = format!(
        r#"<div class="flex justify-between items-center mb-4">
            <div>
                <h2 class="text-xl font-bold text-accent">Documentos</h2>
                <p class="text-sm text-gray-500">{count} documento(s)</p>
            </div>
            <a href="/office/docs/new" class="px-4 py-2 bg-accent text-black font-bold rounded hover:bg-cyan-300">+ Nuevo</a>
        </div>
        <div class="overflow-x-auto">
            <table class="w-full text-left text-sm">
                <thead class="border-b border-gray-800"><tr>
                    <th class="py-2 text-gray-400">Nombre</th>
                    <th class="py-2 text-gray-400">Tamaño</th>
                    <th class="py-2 text-gray-500">Modificado</th>
                    <th class="py-2 text-gray-400">Acciones</th>
                </tr></thead>
                <tbody>{rows}</tbody>
            </table>
        </div>
        {mcp}"#,
        count = files.len(),
        rows = rows,
        mcp = mcp_banner,
    );

    layout("Documentos", &body)
}

/// New document form.
pub async fn new_doc() -> impl IntoResponse {
    let body = r#"<h2 class="text-xl font-bold text-accent mb-4">Nuevo documento</h2>
<form hx-post="/office/docs" hx-swap="outerHTML" class="space-y-4">
    <div>
        <label class="block text-sm text-gray-400 mb-1">Título</label>
        <input type="text" name="title" required
            class="w-full px-4 py-2 bg-card border border-gray-700 rounded text-white focus:border-accent focus:outline-none">
    </div>
    <div>
        <label class="block text-sm text-gray-400 mb-1">Contenido (Markdown)</label>
        <textarea name="content" id="editor" rows="20"
            class="w-full px-4 py-2 bg-card border border-gray-700 rounded text-white focus:border-accent focus:outline-none font-mono text-sm"></textarea>
    </div>
    <button type="submit" class="px-6 py-2 bg-accent text-black font-bold rounded hover:bg-cyan-300">Crear</button>
</form>
<script>"#;
    let mut out = body.to_string();
    out.push_str(templates::MARKDOWN_TOOLBAR_JS);
    out.push_str("</script>");
    layout("Nuevo documento", &out)
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
    let genoffice_ms =
        templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    let editor = doc_editor(&slug, &content);
    let body = format!(
        r#"{saved}
        <div class="flex justify-between items-center mb-3">
            <h2 class="text-xl font-bold text-accent">Editar: <span class="text-white">{title}</span></h2>
            <div class="space-x-2">
                <a href="/office/docs/{slug}/preview" target="_blank" class="px-3 py-1 bg-card border border-gray-700 rounded text-sm hover:bg-gray-800">Vista previa</a>
                <a href="/office/docs/{slug}/export?fmt=md" class="px-3 py-1 bg-card border border-gray-700 rounded text-sm hover:bg-gray-800">Exportar .md</a>
                <a href="/office/docs/{slug}/export?fmt=html" class="px-3 py-1 bg-card border border-gray-700 rounded text-sm hover:bg-gray-800">Exportar .html</a>
            </div>
        </div>
        {mcp}
        {editor}"#,
        saved = saved_banner,
        mcp = mcp_banner,
        title = templates::html_escape(&slug.replace('-', " ")),
        slug = templates::html_escape(&slug),
        editor = editor,
    );

    layout(&format!("Editar: {}", slug), &body).into_response()
}

/// The editor widget (toolbar + textarea + save).
fn doc_editor(slug: &str, content: &str) -> String {
    format!(
        r#"<div class="mb-2 flex flex-wrap gap-1 items-center">
            <button type="button" onclick="insertMd('**','**')" class="px-2 py-1 bg-card border border-gray-700 rounded text-xs hover:border-accent">Bold</button>
            <button type="button" onclick="insertMd('*','*')" class="px-2 py-1 bg-card border border-gray-700 rounded text-xs hover:border-accent">Italic</button>
            <button type="button" onclick="appendMd('## ')" class="px-2 py-1 bg-card border border-gray-700 rounded text-xs hover:border-accent">Heading</button>
            <button type="button" onclick="appendMd('- ')" class="px-2 py-1 bg-card border border-gray-700 rounded text-xs hover:border-accent">List</button>
            <button type="button" onclick="appendMd('1. ')" class="px-2 py-1 bg-card border border-gray-700 rounded text-xs hover:border-accent">Num list</button>
            <button type="button" onclick="insertMd('`','`')" class="px-2 py-1 bg-card border border-gray-700 rounded text-xs hover:border-accent">Code</button>
            <button type="button" onclick="appendMd('```\n\n```')" class="px-2 py-1 bg-card border border-gray-700 rounded text-xs hover:border-accent">Block</button>
            <button type="button" onclick="insertMd('[', '](url)')" class="px-2 py-1 bg-card border border-gray-700 rounded text-xs hover:border-accent">Link</button>
        </div>
        <form hx-put="/office/docs/{slug}" hx-swap="outerHTML" class="space-y-3">
            <textarea name="content" id="editor" rows="28"
                class="w-full px-4 py-2 bg-card border border-gray-700 rounded text-white focus:border-accent focus:outline-none font-mono text-sm">{content}</textarea>
            <div class="flex gap-2">
                <button type="submit" class="px-6 py-2 bg-accent text-black font-bold rounded hover:bg-cyan-300">Guardar</button>
            </div>
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
    let genoffice_ms =
        templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    let body = format!(
        r#"{mcp}
        <div class="flex justify-between items-center mb-4">
            <h2 class="text-xl font-bold text-accent">Vista previa: {title}</h2>
            <a href="/office/docs/{slug}/export?fmt=html" class="px-3 py-1 bg-accent text-black rounded text-sm font-bold hover:bg-cyan-300">Exportar HTML</a>
        </div>
        <div class="prose prose-invert max-w-none bg-card border border-gray-800 rounded-lg p-6">{html}</div>"#,
        mcp = mcp_banner,
        title = templates::html_escape(&slug.replace('-', " ")),
        slug = templates::html_escape(&slug),
        html = html,
    );
    layout(&format!("Vista: {}", slug), &body).into_response()
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
