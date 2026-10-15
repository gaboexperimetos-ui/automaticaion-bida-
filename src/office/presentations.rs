//! Presentation editor — slide list with add/edit/delete/reorder.
//!
//! Each presentation is a `data/office/presentations/{slug}.json` file:
//! `{ "title": ..., "slides": [{"title": ..., "content": <Markdown>}] }`.
//! Slide content is Markdown rendered to HTML with `pulldown-cmark` in present mode.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Form,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::office::pres_dir;
use crate::office::templates;
use crate::office::templates::{hx_redirect, layout, md_to_html, slugify};
use crate::AppState;

/// Input bounds (400 on violation).
const MAX_TITLE_LEN: usize = 200;
const MAX_CONTENT_LEN: usize = 100_000;
const MAX_ORDER_LEN: usize = 10_000;

/// Generic public message for 500-class failures (never leaks internals).
const INTERNAL_ERROR_HTML: &str =
    "<p class=\"text-red-400\">No se pudo completar la operación.</p>";

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Slide {
    title: String,
    content: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Presentation {
    title: String,
    slides: Vec<Slide>,
}

impl Presentation {
    fn new(title: String) -> Self {
        Presentation {
            title,
            slides: Vec::new(),
        }
    }

    fn load_from(path: &std::path::Path) -> Option<Self> {
        let txt = templates::read_text(path);
        serde_json::from_str::<Presentation>(&txt).ok()
    }

    fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        let txt = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string());
        templates::write_text(path, &txt)
    }
}

#[derive(Deserialize)]
pub struct SlideForm {
    pub title: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Deserialize)]
pub struct ReorderForm {
    pub order: String,
}

/// List all presentations.
pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let dir = pres_dir(&state);
    let files = templates::list_files(&dir, "json");

    let rows = if files.is_empty() {
        r#"<p class="text-gray-500">No hay presentaciones aún.</p>"#.to_string()
    } else {
        files
            .iter()
            .map(|f| {
                let p = Presentation::load_from(&dir.join(format!("{}.json", f.name)));
                let slide_count = p.as_ref().map(|p| p.slides.len()).unwrap_or(0);
                format!(
                    r#"<tr class="border-t border-gray-800">
                        <td class="py-2"><a href="/office/presentations/{slug}" class="text-accent hover:text-cyan-300">{display}</a></td>
                        <td class="py-2 text-sm text-gray-500">{n} diapositiva(s)</td>
                        <td class="py-2 text-sm text-gray-500">{size}</td>
                        <td class="py-2 text-sm text-gray-500">{modified}</td>
                        <td class="py-2 space-x-2">
                            <a href="/office/presentations/{slug}/present" target="_blank"
                                class="text-xs text-gray-400 hover:text-white">presentar</a>
                            <form hx-delete="/office/presentations/{slug}" class="inline">
                                <button type="submit" onclick="return confirm('¿Borrar?')"
                                    class="text-xs text-red-400 hover:text-red-300">borrar</button>
                            </form>
                        </td>
                    </tr>"#,
                    slug = templates::html_escape(&f.name),
                    display = templates::html_escape(&f.display),
                    n = slide_count,
                    size = templates::human_size(f.size),
                    modified = &f.modified,
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let body = format!(
        r#"<div class="flex justify-between items-center mb-4">
            <h2 class="text-xl font-bold text-accent">Presentaciones</h2>
            <a href="/office/presentations/new" class="px-4 py-2 bg-accent text-black font-bold rounded hover:bg-cyan-300">+ Nueva</a>
        </div>
        <div class="overflow-x-auto">
            <table class="w-full text-left text-sm">
                <thead class="border-b border-gray-800"><tr>
                    <th class="py-2 text-gray-400">Nombre</th>
                    <th class="py-2 text-gray-400">Diapositivas</th>
                    <th class="py-2 text-gray-400">Tamaño</th>
                    <th class="py-2 text-gray-400">Modificado</th>
                    <th class="py-2 text-gray-400">Acciones</th>
                </tr></thead>
                <tbody>{rows}</tbody>
            </table>
        </div>"#,
        rows = rows,
    );
    layout("Presentaciones", &body)
}

/// New presentation form.
pub async fn new_pres() -> impl IntoResponse {
    let body = r#"<h2 class="text-xl font-bold text-accent mb-4">Nueva presentación</h2>
<form hx-post="/office/presentations" hx-swap="outerHTML" class="space-y-4">
    <div>
        <label class="block text-sm text-gray-400 mb-1">Título</label>
        <input type="text" name="title" required
            class="w-full px-4 py-2 bg-card border border-gray-700 rounded text-white focus:border-accent focus:outline-none">
    </div>
    <button type="submit" class="px-6 py-2 bg-accent text-black font-bold rounded hover:bg-cyan-300">Crear</button>
</form>"#;
    layout("Nueva presentación", &body)
}

/// Create a presentation (empty slide deck).
pub async fn create(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SlideForm>,
) -> impl IntoResponse {
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
    let slug = slugify(&form.title);
    let file_path = pres_dir(&state).join(format!("{}.json", slug));
    if file_path.exists() {
        return (
            axum::http::StatusCode::CONFLICT,
            layout(
                "Conflicto",
                "<p class=\"text-red-400\">Ya existe una presentación con ese título.</p>",
            ),
        )
            .into_response();
    }
    let pres = Presentation::new(form.title);
    if let Err(e) = pres.save_to(&file_path) {
        tracing::error!(
            slug_len = slug.len(),
            "office presentations create: save failed: {}",
            e
        );
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            layout("Error", INTERNAL_ERROR_HTML),
        )
            .into_response();
    }
    hx_redirect(&format!("/office/presentations/{}", slug))
}

/// Edit slides for a presentation.
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
    let file_path = pres_dir(&state).join(format!("{}.json", slug));
    let pres = match Presentation::load_from(&file_path) {
        Some(p) => p,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                layout(
                    "No encontrado",
                    "<p class=\"text-gray-500\">Presentación no encontrada.</p>",
                ),
            )
                .into_response();
        }
    };

    let mut cards = String::new();
    for (i, slide) in pres.slides.iter().enumerate() {
        let title_esc = templates::html_escape(&slide.title);
        let content_esc = templates::html_escape(&slide.content);
        cards.push_str(&format!(
            r#"<div class="bg-card border border-gray-800 rounded-lg p-3 mb-3 slide" id="slide-{i}" data-idx="{i}">
                <form hx-put="/office/presentations/{slug}/slides/{i}" class="space-y-2">
                    <div class="flex justify-between items-center mb-2">
                        <input name="title" value="{title}"
                            class="text-sm bg-gray-900 border border-gray-700 rounded px-2 py-1 text-white flex-1 mr-2 focus:border-accent focus:outline-none"
                            placeholder="Título de la diapositiva">
                        <div class="flex gap-1">
                            <button type="submit"
                                class="px-2 py-1 bg-accent text-black font-bold rounded text-xs hover:bg-cyan-300">Guardar</button>
                        </div>
                    </div>
                    <textarea name="content" rows="4"
                        class="w-full px-2 py-1 bg-gray-900 border border-gray-700 rounded text-white font-mono text-xs focus:border-accent focus:outline-none">{content}</textarea>
                </form>
                <div class="flex gap-1 mt-2">
                    <button type="button" onclick="moveSlide({i}, -1)"
                        class="px-2 py-1 bg-gray-700 border border-gray-700 rounded text-xs hover:bg-gray-800">↑</button>
                    <button type="button" onclick="moveSlide({i}, 1)"
                        class="px-2 py-1 bg-gray-700 border border-gray-700 rounded text-xs hover:bg-gray-800">↓</button>
                    <form hx-delete="/office/presentations/{slug}/slides/{i}" class="inline">
                        <button type="submit" onclick="return confirm('¿Eliminar diapositiva?')"
                            class="px-2 py-1 bg-red-700 border border-red-800 rounded text-xs hover:bg-red-600">✕</button>
                    </form>
                </div>
            </div>"#,
            i = i,
            slug = templates::html_escape(&slug),
            title = title_esc,
            content = content_esc,
        ));
    }

    let body = format!(
        r#"<div class="flex justify-between items-center mb-3">
            <h2 class="text-xl font-bold text-accent">Presentación: <span class="text-white">{title}</span></h2>
            <div class="space-x-2">
                <a href="/office/presentations/{slug}/present" target="_blank"
                    class="px-3 py-1 bg-accent text-black font-bold rounded text-sm hover:bg-cyan-300">Presentar</a>
                <form hx-delete="/office/presentations/{slug}" class="inline">
                    <button type="submit" onclick="return confirm('¿Borrar presentación?')"
                        class="px-3 py-1 bg-red-700 border border-red-800 rounded text-sm hover:bg-red-600">Borrar</button>
                </form>
            </div>
        </div>
        <div id="slides">{cards}</div>
        <h3 class="text-sm font-bold text-accent mb-1 mt-4">Añadir diapositiva</h3>
        <form hx-post="/office/presentations/{slug}/slides" class="space-y-2 mb-4">
            <input name="title" placeholder="Título"
                class="w-full px-3 py-1 bg-card border border-gray-700 rounded text-white focus:border-accent focus:outline-none">
            <textarea name="content" rows="3" placeholder="Contenido (Markdown)"
                class="w-full px-3 py-1 bg-card border border-gray-700 rounded text-white font-mono text-xs focus:border-accent focus:outline-none"></textarea>
            <button type="submit" class="px-4 py-1 bg-accent text-black font-bold rounded text-sm hover:bg-cyan-300">Añadir</button>
        </form>
        <form hx-post="/office/presentations/{slug}/slides/reorder" hx-on::before-request="buildOrder()" class="mt-4">
            <input type="hidden" name="order" id="order" value="">
            <button type="submit" class="px-3 py-1 bg-gray-700 border border-gray-700 rounded text-xs hover:bg-gray-800">Aplicar reorden</button>
        </form>
        <script>
            function buildOrder() {{
                const order = Array.from(document.querySelectorAll('#slides [data-idx]'))
                    .map(el => el.getAttribute('data-idx'));
                document.getElementById('order').value = order.join(',');
            }}
            function moveSlide(idx, delta) {{
                buildOrder();
                // Delegate to a simple reorder by rebuilding order with the moved item.
                const order = Array.from(document.querySelectorAll('#slides [data-idx]'))
                    .map(el => parseInt(el.getAttribute('data-idx'), 10));
                const i = order.indexOf(idx);
                if (i === -1) return;
                const j = i + delta;
                if (j < 0 || j >= order.length) return;
                order.splice(i, 1);
                order.splice(j, 0, idx);
                document.getElementById('order').value = order.join(',');
                fetch('/office/presentations/{slug}/slides/reorder', {{
                    method: 'POST',
                    headers: {{'Content-Type': 'application/x-www-form-urlencoded'}},
                    body: 'order=' + encodeURIComponent(document.getElementById('order').value)
                }}).then(() => location.reload());
            }}
        </script>"#,
        title = templates::html_escape(&pres.title),
        slug = templates::html_escape(&slug),
        cards = cards,
    );
    layout(&format!("Editar: {}", slug), &body).into_response()
}

/// Add a slide.
pub async fn add_slide(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Form(form): Form<SlideForm>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid name").into_response();
    }
    if form.title.len() > MAX_TITLE_LEN || form.content.len() > MAX_CONTENT_LEN {
        return (axum::http::StatusCode::BAD_REQUEST, "slide too large").into_response();
    }
    let file_path = pres_dir(&state).join(format!("{}.json", slug));
    if !file_path.exists() {
        return (axum::http::StatusCode::NOT_FOUND, "presentation not found").into_response();
    }
    let mut pres = match Presentation::load_from(&file_path) {
        Some(p) => p,
        None => {
            return (axum::http::StatusCode::NOT_FOUND, "presentation not found").into_response()
        }
    };
    pres.slides.push(Slide {
        title: form.title,
        content: form.content,
    });
    if let Err(e) = pres.save_to(&file_path) {
        tracing::error!(
            slug_len = slug.len(),
            "office presentations add_slide: save failed: {}",
            e
        );
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "No se pudo completar la operación",
        )
            .into_response();
    }
    hx_redirect(&format!("/office/presentations/{}", slug))
}

/// Update a slide.
pub async fn update_slide(
    State(state): State<Arc<AppState>>,
    Path((slug, index)): Path<(String, usize)>,
    Form(form): Form<SlideForm>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid name").into_response();
    }
    if form.title.len() > MAX_TITLE_LEN || form.content.len() > MAX_CONTENT_LEN {
        return (axum::http::StatusCode::BAD_REQUEST, "slide too large").into_response();
    }
    let file_path = pres_dir(&state).join(format!("{}.json", slug));
    if !file_path.exists() {
        return (axum::http::StatusCode::NOT_FOUND, "presentation not found").into_response();
    }
    let mut pres = match Presentation::load_from(&file_path) {
        Some(p) => p,
        None => {
            return (axum::http::StatusCode::NOT_FOUND, "presentation not found").into_response()
        }
    };
    if index >= pres.slides.len() {
        return (axum::http::StatusCode::NOT_FOUND, "slide not found").into_response();
    }
    pres.slides[index].title = form.title;
    pres.slides[index].content = form.content;
    let _ = pres.save_to(&file_path);
    hx_redirect(&format!("/office/presentations/{}", slug))
}

/// Delete a slide.
pub async fn delete_slide(
    State(state): State<Arc<AppState>>,
    Path((slug, index)): Path<(String, usize)>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid name").into_response();
    }
    let file_path = pres_dir(&state).join(format!("{}.json", slug));
    if !file_path.exists() {
        return (axum::http::StatusCode::NOT_FOUND, "presentation not found").into_response();
    }
    let mut pres = match Presentation::load_from(&file_path) {
        Some(p) => p,
        None => {
            return (axum::http::StatusCode::NOT_FOUND, "presentation not found").into_response()
        }
    };
    if index < pres.slides.len() {
        pres.slides.remove(index);
        let _ = pres.save_to(&file_path);
    }
    hx_redirect(&format!("/office/presentations/{}", slug))
}

/// Reorder slides. Expects `order=1,0,2` (a permutation of slide indices).
pub async fn reorder(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Form(form): Form<ReorderForm>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid name").into_response();
    }
    if form.order.len() > MAX_ORDER_LEN {
        return (axum::http::StatusCode::BAD_REQUEST, "order too large").into_response();
    }
    let file_path = pres_dir(&state).join(format!("{}.json", slug));
    if !file_path.exists() {
        return (axum::http::StatusCode::NOT_FOUND, "presentation not found").into_response();
    }
    let mut pres = match Presentation::load_from(&file_path) {
        Some(p) => p,
        None => {
            return (axum::http::StatusCode::NOT_FOUND, "presentation not found").into_response()
        }
    };
    let indices: Vec<usize> = form
        .order
        .split(',')
        .filter_map(|s| s.trim().parse::<usize>().ok())
        .collect();
    // Validate that `order` is a permutation of 0..n; otherwise reject
    // instead of silently ignoring the request.
    let n = pres.slides.len();
    let mut sorted = indices.clone();
    sorted.sort_unstable();
    let expected: Vec<usize> = (0..n).collect();
    if indices.len() != n || sorted != expected {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "order must be a permutation of slide indices",
        )
            .into_response();
    }
    let reordered: Vec<Slide> = indices
        .iter()
        .filter_map(|i| pres.slides.get(*i).cloned())
        .collect();
    pres.slides = reordered;
    let _ = pres.save_to(&file_path);
    hx_redirect(&format!("/office/presentations/{}", slug))
}

/// Delete an entire presentation.
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
    let file_path = pres_dir(&state).join(format!("{}.json", slug));
    let _ = std::fs::remove_file(&file_path);
    hx_redirect("/office/presentations")
}

/// Presentation mode — all slides rendered from Markdown.
pub async fn present(
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
    let file_path = pres_dir(&state).join(format!("{}.json", slug));
    let pres = match Presentation::load_from(&file_path) {
        Some(p) => p,
        None => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                layout(
                    "No encontrado",
                    "<p class=\"text-gray-500\">Presentación no encontrada.</p>",
                ),
            )
                .into_response();
        }
    };

    let mut slides_html = String::new();
    for slide in &pres.slides {
        let title_esc = templates::html_escape(&slide.title);
        let body_html = md_to_html(&slide.content);
        slides_html.push_str(&format!(
            r#"<section class="slide bg-card border border-gray-800 rounded-lg p-6 mb-4 min-h-[300px]">
                <h2 class="text-2xl font-bold text-accent mb-4">{title}</h2>
                <div class="prose prose-invert max-w-none">{body}</div>
            </section>"#,
            title = title_esc,
            body = body_html,
        ));
    }

    let head = format!(
        r#"<div class="flex justify-between items-center mb-4">
            <h2 class="text-2xl font-bold text-accent">{title}</h2>
            <a href="/office/presentations/{slug}/edit" hidden></a>
        </div>"#,
        title = templates::html_escape(&pres.title),
    );
    let _ = head; // title bar kept minimal in presentation mode
    let body = format!(
        r#"<div class="fixed inset-0 bg-dark overflow-y-auto py-8">
            <div class="max-w-3xl mx-auto">
                <div class="text-center mb-6 text-gray-500 text-sm">
                    Presentación: {title} — {n} diapositiva(s)
                </div>
                {slides}
            </div>
        </div>"#,
        title = templates::html_escape(&pres.title),
        n = pres.slides.len(),
        slides = slides_html,
    );
    layout("Presentar", &body).into_response()
}
