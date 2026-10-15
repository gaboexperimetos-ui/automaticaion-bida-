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

use crate::office::templates::{self, hx_redirect, layout, slugify};
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
                format!(
                    r#"<tr class="border-t border-gray-800">
                        <td class="py-2"><span class="text-accent">{kind}</span> · <a href="/office/library/{ty}/{slug}/download" class="text-gray-200 hover:text-white">{display}</a></td>
                        <td class="py-2 text-sm text-gray-500">{ty}</td>
                        <td class="py-2 text-sm text-gray-500">{size}</td>
                        <td class="py-2 text-sm text-gray-500">{modified}</td>
                        <td class="py-2"><form hx-post="/office/library/{ty}/{slug}/delete" class="inline">
                            <button type="submit" onclick="return confirm('¿Borrar?')"
                                class="text-xs text-red-400 hover:text-red-300">borrar</button>
                        </form></td>
                    </tr>"#,
                    kind = e.kind,
                    ty = e.kind, // note: using kind label for type path segment is WRONG; fix below
                    slug = templates::html_escape(&f.name),
                    display = templates::html_escape(&f.display),
                    size = templates::human_size(f.size),
                    modified = &f.modified,
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let q_val = filter.unwrap_or("");
    let body = format!(
        r#"<div class="flex justify-between items-center mb-4">
            <h2 class="text-xl font-bold text-accent">Biblioteca de archivos</h2>
            <form hx-get="/office/library/search" class="flex gap-2">
                <input type="text" name="q" value="{q}" placeholder="Buscar por nombre..."
                    class="px-3 py-1 bg-card border border-gray-700 rounded text-white focus:border-accent focus:outline-none text-sm">
                <button type="submit" class="px-3 py-1 bg-accent text-black font-bold rounded text-sm hover:bg-cyan-300">Buscar</button>
            </form>
        </div>
        <p class="text-sm text-gray-500 mb-3">{count} archivo(s)</p>
        <div class="overflow-x-auto">
            <table class="w-full text-left text-sm">
                <thead class="border-b border-gray-800"><tr>
                    <th class="py-2 text-gray-400">Archivo</th>
                    <th class="py-2 text-gray-400">Tipo</th>
                    <th class="py-2 text-gray-400">Tamaño</th>
                    <th class="py-2 text-gray-400">Modificado</th>
                    <th class="py-2 text-gray-400">Acciones</th>
                </tr></thead>
                <tbody>{rows}</tbody>
            </table>
        </div>"#,
        q = templates::html_escape(q_val),
        count = filtered.len(),
        rows = rows,
    );

    // Fix: the `ty` placeholder must be the URL type segment, not the display label.
    // We rebuild rows with the correct type by re-iterating.
    let _ = body;
    rebuild(state, filter, q_val)
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
                let url_type = match e.kind {
                    "documento" => "docs",
                    "hoja" => "sheets",
                    "presentación" => "presentations",
                    _ => "docs",
                };
                format!(
                    r#"<tr class="border-t border-gray-800">
                        <td class="py-2"><span class="text-accent">{kind}</span> · <a href="/office/library/{ty}/{slug}/download" class="text-gray-200 hover:text-white">{display}</a></td>
                        <td class="py-2 text-sm text-gray-500">{kind}</td>
                        <td class="py-2 text-sm text-gray-500">{size}</td>
                        <td class="py-2 text-sm text-gray-500">{modified}</td>
                        <td class="py-2"><form hx-post="/office/library/{ty}/{slug}/delete" class="inline">
                            <button type="submit" onclick="return confirm('¿Borrar?')"
                                class="text-xs text-red-400 hover:text-red-300">borrar</button>
                        </form></td>
                    </tr>"#,
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
        r#"<div class="flex justify-between items-center mb-4">
            <h2 class="text-xl font-bold text-accent">Biblioteca de archivos</h2>
            <form hx-get="/office/library/search" class="flex gap-2">
                <input type="text" name="q" value="{q}" placeholder="Buscar por nombre..."
                    class="px-3 py-1 bg-card border border-gray-700 rounded text-white focus:border-accent focus:outline-none text-sm">
                <button type="submit" class="px-3 py-1 bg-accent text-black font-bold rounded text-sm hover:bg-cyan-300">Buscar</button>
            </form>
        </div>
        <p class="text-sm text-gray-500 mb-3">{count} archivo(s)</p>
        <div class="overflow-x-auto">
            <table class="w-full text-left text-sm">
                <thead class="border-b border-gray-800"><tr>
                    <th class="py-2 text-gray-400">Archivo</th>
                    <th class="py-2 text-gray-400">Tipo</th>
                    <th class="py-2 text-gray-400">Tamaño</th>
                    <th class="py-2 text-gray-400">Modificado</th>
                    <th class="py-2 text-gray-400">Acciones</th>
                </tr></thead>
                <tbody>{rows}</tbody>
            </table>
        </div>
        <div class="mt-6">
            <h3 class="text-sm font-bold text-accent mb-2">Importar archivo</h3>
            <form hx-post="/office/library/import/docs" class="grid grid-cols-1 md:grid-cols-4 gap-2 items-end">
                <input type="text" name="name" placeholder="Nombre" required
                    class="px-3 py-1 bg-card border border-gray-700 rounded text-white focus:border-accent focus:outline-none">
                <input type="text" name="content" placeholder="Contenido" required
                    class="px-3 py-1 bg-card border border-gray-700 rounded text-white focus:border-accent focus:outline-none">
                <select name="kind" class="px-3 py-1 bg-card border border-gray-700 rounded text-white focus:border-accent focus:outline-none">
                    <option value="docs">Documento</option>
                    <option value="sheets">Hoja</option>
                    <option value="presentations">Presentación</option>
                </select>
                <button type="submit" class="px-4 py-1 bg-accent text-black font-bold rounded text-sm hover:bg-cyan-300">Importar</button>
            </form>
        </div>"#,
        count = filtered.len(),
        q = templates::html_escape(q_val),
        rows = rows,
    );

    Response::builder()
        .status(200)
        .body(Body::from(layout("Biblioteca", &body)))
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
