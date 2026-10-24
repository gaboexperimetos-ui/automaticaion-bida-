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
use crate::office::templates::{hx_redirect, layout, layout_as, md_to_html, slugify};
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

    let cards = if files.is_empty() {
        r#"<div class="nova-office-empty"><span class="material-symbols-outlined">present_to_all</span><h2>Tu próxima idea merece un escenario.</h2><p>Crea una presentación y construye sus diapositivas en Markdown.</p><a href="/office/presentations/new" class="nova-button nova-button-primary">Crear presentación</a></div>"#.to_string()
    } else {
        files.iter().map(|f| {
            let p = Presentation::load_from(&dir.join(format!("{}.json", f.name)));
            let slide_count = p.as_ref().map(|p| p.slides.len()).unwrap_or(0);
            format!(
                r#"<article class="nova-file-card presentation"><div class="nova-file-card-top"><span class="nova-file-type"><span class="material-symbols-outlined">present_to_all</span></span><span>{n} DIAPOSITIVA(S)</span></div><h2><a href="/office/presentations/{slug}">{display}</a></h2><p class="nova-file-meta">{modified} · {size}</p><footer><a href="/office/presentations/{slug}/present" target="_blank" class="nova-text-link">Presentar<span class="material-symbols-outlined">play_arrow</span></a><form hx-delete="/office/presentations/{slug}"><button type="submit" onclick="return confirm('¿Borrar?')" class="nova-icon-button danger" aria-label="Borrar {display}"><span class="material-symbols-outlined">delete</span></button></form></footer></article>"#,
                slug = templates::html_escape(&f.name),
                display = templates::html_escape(&f.display),
                n = slide_count,
                size = templates::human_size(f.size),
                modified = &f.modified,
            )
        }).collect::<Vec<_>>().join("
")
    };

    let body = format!(
        r#"<div class="nova-page nova-office-page"><header class="nova-page-head compact"><div><p class="nova-eyebrow"><span>07.3</span> Oficina</p><h1>Ideas que entran<br><em>en escena.</em></h1><p class="nova-page-lead">Construye, ordena y presenta tus diapositivas desde un mismo lugar.</p></div><a href="/office/presentations/new" class="nova-button nova-button-primary"><span class="material-symbols-outlined">add</span>Nueva presentación</a></header><nav class="nova-office-switch" aria-label="Aplicaciones de oficina"><a href="/office/docs">Documentos</a><a href="/office/sheets">Hojas</a><a href="/office/presentations" aria-current="page">Presentaciones</a><a href="/office/library">Biblioteca</a></nav><section class="nova-file-grid">{cards}</section></div>"#,
        cards = cards,
    );

    layout_as("Presentaciones", "presentaciones", &body)
}

/// New presentation form.
pub async fn new_pres() -> impl IntoResponse {
    let body = r#"<div class="nova-page nova-editor-page"><header class="nova-page-head compact"><div><p class="nova-eyebrow"><span>07.3</span> Nueva presentación</p><h1>La primera diapositiva<br><em>empieza con un título.</em></h1><p class="nova-page-lead">Después podrás añadir contenido Markdown y ordenar cada escena.</p></div><a href="/office/presentations" class="nova-button nova-button-quiet">Volver</a></header><form hx-post="/office/presentations" hx-swap="outerHTML" class="nova-create-sheet-form"><span class="material-symbols-outlined">present_to_all</span><div><label for="new-pres-title">Título de la presentación</label><input id="new-pres-title" type="text" name="title" placeholder="Ej. El sistema circulatorio" required></div><button type="submit" class="nova-button nova-button-primary">Crear presentación<span class="material-symbols-outlined">arrow_forward</span></button></form></div>"#;
    layout_as("Nueva presentación", "presentaciones", &body)
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
            r#"<article class="nova-slide-card slide" id="slide-{i}" data-idx="{i}"><header><span>DIAPOSITIVA {number}</span><div><button type="button" onclick="moveSlide({i}, -1)" class="nova-icon-button" aria-label="Mover diapositiva hacia arriba"><span class="material-symbols-outlined">arrow_upward</span></button><button type="button" onclick="moveSlide({i}, 1)" class="nova-icon-button" aria-label="Mover diapositiva hacia abajo"><span class="material-symbols-outlined">arrow_downward</span></button><form hx-delete="/office/presentations/{slug}/slides/{i}"><button type="submit" onclick="return confirm('¿Eliminar diapositiva?')" class="nova-icon-button danger" aria-label="Eliminar diapositiva"><span class="material-symbols-outlined">delete</span></button></form></div></header><form hx-put="/office/presentations/{slug}/slides/{i}"><label for="slide-title-{i}">Título</label><input id="slide-title-{i}" name="title" value="{title}" placeholder="Título de la diapositiva"><label for="slide-content-{i}">Contenido Markdown</label><textarea id="slide-content-{i}" name="content" rows="7">{content}</textarea><footer><span>Markdown</span><button type="submit" class="nova-button nova-button-quiet"><span class="material-symbols-outlined">save</span>Guardar</button></footer></form></article>"#,
            i = i,
            number = i + 1,
            slug = templates::html_escape(&slug),
            title = title_esc,
            content = content_esc,
        ));
    }

    let body = format!(
        r#"<div class="nova-page nova-deck-page"><header class="nova-workspace-head"><div class="nova-workspace-title"><span class="nova-workspace-symbol"><span class="material-symbols-outlined">present_to_all</span></span><div><p>Presentación</p><h1>{title}</h1></div></div><div class="nova-workspace-actions"><a href="/office/presentations" class="nova-button nova-button-quiet">Volver</a><a href="/office/presentations/{slug}/present" target="_blank" class="nova-button nova-button-primary"><span class="material-symbols-outlined">play_arrow</span>Presentar</a><form hx-delete="/office/presentations/{slug}"><button type="submit" onclick="return confirm('¿Borrar presentación?')" class="nova-icon-button danger" aria-label="Borrar presentación"><span class="material-symbols-outlined">delete</span></button></form></div></header><div class="nova-deck-layout"><section class="nova-deck-slides"><div class="nova-panel-caption"><span>01</span><h2>Diapositivas</h2><span class="material-symbols-outlined">view_carousel</span></div><div id="slides" class="nova-slide-list">{cards}</div><form hx-post="/office/presentations/{slug}/slides/reorder" hx-on::before-request="buildOrder()" class="nova-reorder-form"><input type="hidden" name="order" id="order" value=""><button type="submit" class="nova-button nova-button-quiet"><span class="material-symbols-outlined">swap_vert</span>Aplicar orden</button></form></section><aside class="nova-new-slide"><div class="nova-panel-caption"><span>02</span><h2>Nueva escena</h2><span class="material-symbols-outlined">add</span></div><p>Añade una diapositiva al final del mazo.</p><form hx-post="/office/presentations/{slug}/slides"><label for="new-slide-title">Título</label><input id="new-slide-title" name="title" placeholder="Una idea clara"><label for="new-slide-content">Contenido Markdown</label><textarea id="new-slide-content" name="content" rows="9" placeholder="Añade un encabezado&#10;&#10;Escribe tu contenido…"></textarea><button type="submit" class="nova-button nova-button-primary">Añadir diapositiva</button></form></aside></div></div>
        <script>
            function buildOrder() {{
                const order = Array.from(document.querySelectorAll('#slides [data-idx]'))
                    .map(el => el.getAttribute('data-idx'));
                document.getElementById('order').value = order.join(',');
            }}
            function moveSlide(idx, delta) {{
                buildOrder();
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
    layout_as(&format!("Editar: {}", slug), "presentaciones", &body).into_response()
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
    for (index, slide) in pres.slides.iter().enumerate() {
        let title_esc = templates::html_escape(&slide.title);
        let body_html = md_to_html(&slide.content);
        slides_html.push_str(&format!(
            r#"<section class="nova-present-slide"><div class="nova-present-slide-number">{number}</div><div><h2>{title}</h2><div class="nova-present-slide-body">{body}</div></div></section>"#,
            number = index + 1,
            title = title_esc,
            body = body_html,
        ));
    }

    let body = format!(
        r#"<div class="nova-page nova-present-mode"><header class="nova-workspace-head"><div class="nova-workspace-title"><span class="nova-workspace-symbol"><span class="material-symbols-outlined">present_to_all</span></span><div><p>Modo presentación · {n} diapositiva(s)</p><h1>{title}</h1></div></div><div class="nova-workspace-actions"><a href="/office/presentations/{slug}" class="nova-button nova-button-quiet"><span class="material-symbols-outlined">arrow_back</span>Volver al editor</a></div></header><main class="nova-present-deck">{slides}</main></div>"#,
        title = templates::html_escape(&pres.title),
        slug = templates::html_escape(&slug),
        n = pres.slides.len(),
        slides = slides_html,
    );
    layout_as("Presentar", "presentaciones", &body).into_response()
}
