//! File library — unified listing, search, download, delete, and import across the
//! three office directories. All backed by the filesystem (no external services).

use axum::{
    body::Body,
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Form,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::office::templates::{self, hx_redirect, layout, layout_as, slugify};
use crate::office::{type_dir, type_ext};
use crate::AppState;

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

#[derive(Deserialize)]
pub struct ImportForm {
    pub name: String,
    pub content: String,
}

struct LibEntry {
    file: templates::FileInfo,
    kind: &'static str,
}

/// Input bounds (400 on violation).
const MAX_QUERY_LEN: usize = 200;
const MAX_NAME_LEN: usize = 200;
const MAX_CONTENT_LEN: usize = 500_000;

fn gather(state: &AppState) -> Vec<LibEntry> {
    let mut out = Vec::new();
    let kinds: [(&str, &str); 3] = [
        ("docs", "documento"),
        ("sheets", "hoja"),
        ("presentations", "presentación"),
    ];
    for (ty, kind) in kinds {
        let dir = match type_dir(state, ty) {
            Some(d) => d,
            None => continue,
        };
        let ext = type_ext(ty).unwrap_or("");
        for f in templates::list_files(&dir, ext) {
            out.push(LibEntry { file: f, kind });
        }
    }
    out.sort_by(|a, b| a.file.modified.cmp(&b.file.modified).reverse());
    out
}

/// List all office files.
pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    render_list(&state, None).await
}

/// Search files by name.
pub async fn search(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
) -> impl IntoResponse {
    if q.q.as_ref().is_some_and(|s| s.len() > MAX_QUERY_LEN) {
        return (axum::http::StatusCode::BAD_REQUEST, "query too long").into_response();
    }
    render_list(&state, q.q.as_deref()).await
}

async fn render_list(state: &Arc<AppState>, filter: Option<&str>) -> Response {
    rebuild(state, filter, filter.unwrap_or("")).await
}

/// Rebuild the table with correct type segments in the action links.
async fn rebuild(state: &Arc<AppState>, filter: Option<&str>, q_val: &str) -> Response {
    let all = gather(state);
    let filtered: Vec<&LibEntry> = match filter {
        Some(q) if !q.trim().is_empty() => {
            let q = q.to_lowercase();
            all.iter()
                .filter(|e| {
                    e.file.name.to_lowercase().contains(&q)
                        || e.file.display.to_lowercase().contains(&q)
                })
                .collect()
        }
        _ => all.iter().collect(),
    };
    let rows = if filtered.is_empty() {
        r##"<tr><td colspan="5"><div class="gen-office-empty compact"><span class="material-symbols-outlined">search_off</span><p>No se encontraron archivos.</p></div></td></tr>"##.to_string()
    } else {
        filtered
            .iter()
            .map(|e| {
                let f = &e.file;
                let (url_type, icon) = match e.kind {
                    "documento" => ("docs", "description"),
                    "hoja" => ("sheets", "table_chart"),
                    "presentación" => ("presentations", "present_to_all"),
                    _ => ("docs", "draft"),
                };
                format!(
                    r##"<tr><td data-label="Archivo"><a href="/office/{ty}/{slug}" class="gen-office-library-file {ty}"><span class="material-symbols-outlined">{icon}</span><span>{display}</span></a></td><td data-label="Tipo"><span class="gen-office-file-kind">{kind}</span></td><td data-label="Tamaño">{size}</td><td data-label="Modificado">{modified}</td><td><div class="gen-office-library-actions"><a href="/office/{ty}/{slug}" class="gen-office-icon-button" aria-label="Abrir {display}"><span class="material-symbols-outlined">arrow_outward</span></a><a href="/office/library/{ty}/{slug}/download" class="gen-office-icon-button" aria-label="Descargar {display}"><span class="material-symbols-outlined">download</span></a><form hx-post="/office/library/{ty}/{slug}/delete"><button type="submit" onclick="return confirm('¿Borrar?')" class="gen-office-icon-button danger" aria-label="Borrar {display}"><span class="material-symbols-outlined">delete</span></button></form></div></td></tr>"##,
                    icon = icon,
                    kind = e.kind,
                    ty = url_type,
                    slug = templates::html_escape(&f.name),
                    display = templates::html_escape(&f.display),
                    size = templates::human_size(f.size),
                    modified = templates::html_escape(&f.modified),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-manager-page gen-office-library-page">{tabs}<header class="gen-office-manager-head"><div><span>BIBLIOTECA · ARCHIVOS LOCALES</span><h1>Todo tu trabajo.</h1><p>{count} archivo(s) visibles. Abre, descarga o importa desde un solo lugar.</p></div><a href="#library-import" class="gen-office-primary-button"><span class="material-symbols-outlined">upload_file</span>Importar archivo</a></header><nav class="gen-office-appnav" aria-label="Aplicaciones de oficina"><a href="/office/docs"><span class="material-symbols-outlined">description</span>Documentos</a><a href="/office/sheets"><span class="material-symbols-outlined">table_chart</span>Hojas</a><a href="/office/presentations"><span class="material-symbols-outlined">present_to_all</span>Presentaciones</a><a class="is-active library" href="/office/library"><span class="material-symbols-outlined">folder_open</span>Biblioteca</a></nav>
        <form hx-get="/office/library/search" hx-target="closest .nova-page" hx-select=".nova-page" hx-swap="outerHTML" hx-push-url="true" class="gen-office-library-search"><span class="material-symbols-outlined">search</span><label class="sr-only" for="library-query">Buscar archivos</label><input id="library-query" type="search" name="q" value="{q}" placeholder="Buscar por nombre…"><button type="submit" class="gen-office-secondary-button">Buscar</button></form>
        <div class="gen-office-library-layout"><section class="gen-office-library-browser"><header><div><span>ARCHIVOS</span><strong>{count} resultado(s)</strong></div><span class="material-symbols-outlined">folder_open</span></header><div class="gen-office-table-scroll"><table class="gen-office-library-table"><thead><tr><th>Archivo</th><th>Tipo</th><th>Tamaño</th><th>Modificado</th><th aria-label="Acciones"></th></tr></thead><tbody>{rows}</tbody></table></div></section>
        <aside class="gen-office-library-import" id="library-import"><header><span class="gen-office-launcher-icon library"><span class="material-symbols-outlined">upload_file</span></span><div><span>IMPORTAR</span><h2>Nuevo archivo</h2></div></header><p>Crea un archivo directamente en tu biblioteca local.</p><form id="library-import-form" hx-post="/office/library/import/docs"><label for="library-import-name">Nombre</label><input id="library-import-name" type="text" name="name" placeholder="mi-archivo" required><label for="library-import-kind">Tipo de archivo</label><select id="library-import-kind" name="kind" onchange="this.form.setAttribute('hx-post','/office/library/import/'+this.value)"><option value="docs">Documento Markdown</option><option value="sheets">Hoja CSV</option><option value="presentations">Presentación</option></select><label for="library-import-content">Contenido</label><textarea id="library-import-content" name="content" rows="8" placeholder="Escribe o pega el contenido…" required></textarea><p class="gen-office-import-help" id="library-import-help">Markdown para documentos, CSV para hojas. Una presentación acepta texto para una primera escena o JSON válido.</p><button type="submit" class="gen-office-primary-button">Importar archivo<span class="material-symbols-outlined">arrow_forward</span></button></form></aside></div><footer class="gen-office-home-footer"><span><span class="material-symbols-outlined">hard_drive</span> Archivos locales</span><span>Los nombres abren el editor</span></footer></div>"##,
        tabs = templates::office_tab_strip("library", "Biblioteca", "#library-import"),
        count = filtered.len(),
        q = templates::html_escape(q_val),
        rows = rows,
    );

    Response::builder()
        .status(200)
        .body(Body::from(
            templates::layout_as_shelled(&state, "Biblioteca", "biblioteca", &body).await,
        ))
        .unwrap()
}

/// Download a file by type + name (slug).
pub async fn download(
    State(state): State<Arc<AppState>>,
    Path((kind, name)): Path<(String, String)>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&name) {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid name").into_response();
    }
    let ext = match type_ext(&kind) {
        Some(e) => e,
        None => return (axum::http::StatusCode::NOT_FOUND, "unknown type").into_response(),
    };
    let dir = match type_dir(&state, &kind) {
        Some(d) => d,
        None => return (axum::http::StatusCode::NOT_FOUND, "unknown type").into_response(),
    };
    let file_path = dir.join(format!("{}.{}", name, ext));
    if !file_path.exists() {
        return (axum::http::StatusCode::NOT_FOUND, "file not found").into_response();
    }
    let bytes = match templates::read_bytes(&file_path) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!(
                kind = kind.as_str(),
                name_len = name.len(),
                "office library download: read failed: {}",
                e
            );
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo completar la operación",
            )
                .into_response();
        }
    };
    let mime = crate::office::templates::mime_for_ext(ext);
    templates::file_download(&format!("{}.{}", name, ext), mime, bytes)
}

/// Delete a file by type + name.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path((kind, name)): Path<(String, String)>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&name) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout("Error", "<p class=\"text-red-400\">Nombre no válido.</p>"),
        )
            .into_response();
    }
    let ext = match type_ext(&kind) {
        Some(e) => e,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                layout(
                    "Error",
                    "<p class=\"text-red-400\">Tipo de archivo no válido.</p>",
                ),
            )
                .into_response()
        }
    };
    let dir = match type_dir(&state, &kind) {
        Some(d) => d,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                layout(
                    "Error",
                    "<p class=\"text-red-400\">Tipo de archivo no válido.</p>",
                ),
            )
                .into_response()
        }
    };
    let file_path = dir.join(format!("{}.{}", name, ext));
    if let Err(e) = std::fs::remove_file(&file_path) {
        let status = if e.kind() == std::io::ErrorKind::NotFound {
            axum::http::StatusCode::NOT_FOUND
        } else {
            tracing::error!(
                kind = kind.as_str(),
                name_len = name.len(),
                "office library delete: failed: {}",
                e
            );
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        };
        return (
            status,
            layout(
                "Error",
                "<p class=\"text-red-400\">No se pudo borrar el archivo.</p>",
            ),
        )
            .into_response();
    }
    hx_redirect("/office/library")
}

/// Import a file from a simple form (name + content).
pub async fn import(
    State(state): State<Arc<AppState>>,
    Path(kind): Path<String>,
    Form(form): Form<ImportForm>,
) -> impl IntoResponse {
    let ext = match type_ext(&kind) {
        Some(e) => e,
        None => return (axum::http::StatusCode::NOT_FOUND, "unknown type").into_response(),
    };
    let dir = match type_dir(&state, &kind) {
        Some(d) => d,
        None => return (axum::http::StatusCode::NOT_FOUND, "unknown type").into_response(),
    };
    if form.name.trim().is_empty() {
        return (axum::http::StatusCode::BAD_REQUEST, "name is required").into_response();
    }
    if form.name.len() > MAX_NAME_LEN || form.content.len() > MAX_CONTENT_LEN {
        return (axum::http::StatusCode::BAD_REQUEST, "import too large").into_response();
    }

    // El formulario de Biblioteca permite crear la primera escena con texto
    // sencillo. Antes se guardaba ese texto con extensión .json y el editor ya
    // no podía abrirlo. JSON existente se acepta solo si tiene la estructura real.
    let content = if kind == "presentations" {
        match serde_json::from_str::<serde_json::Value>(&form.content) {
            Ok(value) => {
                let valid = value
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|title| !title.trim().is_empty())
                    && value
                        .get("slides")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|slides| {
                            slides.iter().all(|slide| {
                                slide
                                    .get("title")
                                    .and_then(serde_json::Value::as_str)
                                    .is_some_and(|title| !title.trim().is_empty())
                                    && slide
                                        .get("content")
                                        .and_then(serde_json::Value::as_str)
                                        .is_some()
                            })
                        });
                if !valid {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "JSON de presentación no válido",
                    )
                        .into_response();
                }
                form.content.clone()
            }
            Err(_) => serde_json::to_string_pretty(&serde_json::json!({
                "title": form.name.trim(),
                "slides": [{
                    "title": form.name.trim(),
                    "content": form.content,
                }],
            }))
            .unwrap_or_else(|_| "{}".to_string()),
        }
    } else {
        form.content.clone()
    };

    let slug = slugify(&form.name);
    let file_path = dir.join(format!("{}.{}", slug, ext));
    if let Err(e) = templates::write_text(&file_path, &content) {
        tracing::error!(
            kind = kind.as_str(),
            slug_len = slug.len(),
            "office library import: write failed: {}",
            e
        );
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "No se pudo completar la operación",
        )
            .into_response();
    }
    hx_redirect("/office/library")
}
