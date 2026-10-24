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
    render_list(&state, None)
}

/// Search files by name.
pub async fn search(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
) -> impl IntoResponse {
    if q.q.as_ref().is_some_and(|s| s.len() > MAX_QUERY_LEN) {
        return (axum::http::StatusCode::BAD_REQUEST, "query too long").into_response();
    }
    render_list(&state, q.q.as_deref())
}

fn render_list(state: &AppState, filter: Option<&str>) -> Response {
    rebuild(state, filter, filter.unwrap_or(""))
}

/// Rebuild the table with correct type segments in the action links.
fn rebuild(state: &AppState, filter: Option<&str>, q_val: &str) -> Response {
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
        r#"<tr><td colspan="5" class="py-4 text-center text-gray-500">No se encontraron archivos.</td></tr>"#.to_string()
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
                    r#"<tr>
                        <td><a href="/office/library/{ty}/{slug}/download" class="nova-library-file"><span class="material-symbols-outlined">{icon}</span><span>{display}</span></a></td>
                        <td><span class="nova-file-kind">{kind}</span></td>
                        <td>{size}</td>
                        <td>{modified}</td>
                        <td><div class="nova-library-actions"><a href="/office/library/{ty}/{slug}/download" class="nova-icon-button" aria-label="Descargar {display}"><span class="material-symbols-outlined">download</span></a><form hx-post="/office/library/{ty}/{slug}/delete"><button type="submit" onclick="return confirm('¿Borrar?')" class="nova-icon-button danger" aria-label="Borrar {display}"><span class="material-symbols-outlined">delete</span></button></form></div></td>
                    </tr>"#,
                    icon = icon,
                    kind = e.kind,
                    ty = url_type,
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
        r#"<div class="nova-page nova-library-page"><header class="nova-page-head compact"><div><p class="nova-eyebrow"><span>08</span> Biblioteca</p><h1>Todo tu trabajo.<br><em>Un solo archivo.</em></h1><p class="nova-page-lead">Documentos, hojas y presentaciones ordenados por su última modificación.</p></div><span class="nova-version-badge">{count} archivo(s)</span></header><nav class="nova-office-switch" aria-label="Aplicaciones de oficina"><a href="/office/docs">Documentos</a><a href="/office/sheets">Hojas</a><a href="/office/presentations">Presentaciones</a><a href="/office/library" aria-current="page">Biblioteca</a></nav><form hx-get="/office/library/search" hx-target="closest .nova-page" hx-select=".nova-page" hx-swap="outerHTML" hx-push-url="true" class="nova-library-search"><span class="material-symbols-outlined">search</span><label class="sr-only" for="library-query">Buscar archivos</label><input id="library-query" type="search" name="q" value="{q}" placeholder="Buscar por nombre…"><button type="submit" class="nova-button nova-button-quiet">Buscar</button></form><div class="nova-library-layout"><section class="nova-library-browser"><div class="nova-panel-caption"><span>01</span><h2>Archivos recientes</h2><span class="material-symbols-outlined">folder_open</span></div><div class="nova-table-scroll"><table class="nova-library-table"><thead><tr><th>Archivo</th><th>Tipo</th><th>Tamaño</th><th>Modificado</th><th aria-label="Acciones"></th></tr></thead><tbody>{rows}</tbody></table></div></section><aside class="nova-library-import"><div class="nova-panel-caption"><span>02</span><h2>Importar</h2><span class="material-symbols-outlined">upload_file</span></div><p>Crea un archivo directamente en la biblioteca local.</p><form id="library-import-form" hx-post="/office/library/import/docs"><label for="library-import-name">Nombre</label><input id="library-import-name" type="text" name="name" placeholder="mi-archivo" required><label for="library-import-content">Contenido</label><textarea id="library-import-content" name="content" rows="7" placeholder="Escribe o pega el contenido…" required></textarea><label for="library-import-kind">Tipo de archivo</label><select id="library-import-kind" name="kind" onchange="this.form.setAttribute('hx-post','/office/library/import/'+this.value)"><option value="docs">Documento</option><option value="sheets">Hoja</option><option value="presentations">Presentación</option></select><button type="submit" class="nova-button nova-button-primary">Importar archivo<span class="material-symbols-outlined">arrow_forward</span></button></form></aside></div></div>"#,
        count = filtered.len(),
        q = templates::html_escape(q_val),
        rows = rows,
    );

    Response::builder()
        .status(200)
        .body(Body::from(layout_as("Biblioteca", "biblioteca", &body)))
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
    let bytes = templates::read_bytes(&file_path);
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
        None => return hx_redirect("/office/library"),
    };
    let dir = match type_dir(&state, &kind) {
        Some(d) => d,
        None => return hx_redirect("/office/library"),
    };
    let file_path = dir.join(format!("{}.{}", name, ext));
    let _ = std::fs::remove_file(&file_path);
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
    if form.name.len() > MAX_NAME_LEN || form.content.len() > MAX_CONTENT_LEN {
        return (axum::http::StatusCode::BAD_REQUEST, "import too large").into_response();
    }
    if let Some(parent) = dir.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::create_dir_all(&dir);
    let slug = slugify(&form.name);
    let file_path = dir.join(format!("{}.{}", slug, ext));
    if let Err(e) = templates::write_text(&file_path, &form.content) {
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
