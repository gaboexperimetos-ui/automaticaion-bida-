//! Presentation editor — slide list with add/edit/delete/reorder.
//!
//! Each presentation is a `data/office/presentations/{slug}.json` file:
//! `{ "title": ..., "slides": [{"title": ..., "content": <Markdown>}] }`.
//! Slide content is Markdown rendered to HTML with `pulldown-cmark` in present mode.

use axum::{
    extract::{Path, State},
    response::IntoResponse,
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
        let txt = match templates::read_text(path) {
            Ok(txt) => txt,
            Err(e) => {
                tracing::error!("office presentation read failed: {}", e);
                return None;
            }
        };
        match serde_json::from_str::<Presentation>(&txt) {
            Ok(presentation) => Some(presentation),
            Err(e) => {
                tracing::warn!("office presentation JSON invalid: {}", e);
                None
            }
        }
    }

    fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        let txt = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
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
        r##"<div class="gen-office-empty"><span class="material-symbols-outlined">present_to_all</span><h3>Tu próxima idea merece un escenario.</h3><p>Crea una presentación y construye sus diapositivas en Markdown.</p><a href="/office/presentations/new" class="gen-office-primary-button">Crear presentación</a></div>"##.to_string()
    } else {
        files.iter().map(|f| {
            let p = Presentation::load_from(&dir.join(format!("{}.json", f.name)));
            let slide_count = p.as_ref().map(|p| p.slides.len()).unwrap_or(0);
            format!(
                r##"<article class="gen-office-file-card presentations"><a class="gen-office-file-preview" href="/office/presentations/{slug}"><span class="material-symbols-outlined">present_to_all</span><small>{n} DIAPOSITIVA(S)</small></a><div class="gen-office-file-copy"><a href="/office/presentations/{slug}">{display}</a><span>{modified} · {size}</span></div><footer><a href="/office/presentations/{slug}/present" target="_blank">Presentar</a><form hx-delete="/office/presentations/{slug}"><button type="submit" onclick="return confirm('¿Borrar?')" class="gen-office-icon-button danger" aria-label="Borrar {display}"><span class="material-symbols-outlined">delete</span></button></form></footer></article>"##,
                slug = templates::html_escape(&f.name),
                display = templates::html_escape(&f.display),
                n = slide_count,
                size = templates::human_size(f.size),
                modified = templates::html_escape(&f.modified),
            )
        }).collect::<Vec<_>>().join("\n")
    };

    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-manager-page">{tabs}<header class="gen-office-manager-head"><div><span>PRESENTACIONES · MARKDOWN</span><h1>Ideas listas para presentar.</h1><p>{count} presentación(es) editables en tu espacio local.</p></div><a href="/office/presentations/new" class="gen-office-primary-button"><span class="material-symbols-outlined">add</span>Nueva presentación</a></header><nav class="gen-office-appnav" aria-label="Aplicaciones de oficina"><a href="/office/docs"><span class="material-symbols-outlined">description</span>Documentos</a><a href="/office/sheets"><span class="material-symbols-outlined">table_chart</span>Hojas</a><a class="is-active presentations" href="/office/presentations"><span class="material-symbols-outlined">present_to_all</span>Presentaciones</a><a href="/office/library"><span class="material-symbols-outlined">folder_open</span>Biblioteca</a></nav><section class="gen-office-file-grid">{cards}</section><footer class="gen-office-home-footer"><span><span class="material-symbols-outlined">hard_drive</span> Presentaciones locales</span><span>Edición y modo presentación</span></footer></div>"##,
        tabs = templates::office_tab_strip(
            "presentations",
            "Presentaciones",
            "/office/presentations/new"
        ),
        count = files.len(),
        cards = cards,
    );
    layout_as("Presentaciones", "presentaciones", &body)
}

/// New presentation form.
pub async fn new_pres() -> impl IntoResponse {
    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-create-page">{tabs}<main class="gen-office-create-shell"><div class="gen-office-create-heading"><span class="gen-office-launcher-icon presentations"><span class="material-symbols-outlined">present_to_all</span></span><div><span>NUEVA PRESENTACIÓN</span><h1>Tu primera escena empieza aquí.</h1><p>Crea el mazo y añade después títulos, contenido Markdown y el orden de cada escena.</p></div></div><form hx-post="/office/presentations" hx-swap="outerHTML" class="gen-office-create-form compact"><div class="gen-office-create-fields"><label for="new-pres-title">Título de la presentación</label><input id="new-pres-title" type="text" name="title" placeholder="Ej. El sistema circulatorio" required autofocus></div><div class="gen-office-create-facts"><span><strong>Miniaturas</strong> navegación visual</span><span><strong>Markdown</strong> contenido editable</span><span><strong>Presentar</strong> una escena a la vez</span></div><footer><a href="/office/presentations" class="gen-office-secondary-button">Cancelar</a><span>Se guardará como .json</span><button type="submit" class="gen-office-primary-button">Crear presentación<span class="material-symbols-outlined">arrow_forward</span></button></footer></form></main></div>"##,
        tabs = templates::office_tab_strip(
            "presentations",
            "Nueva presentación",
            "/office/presentations/new"
        ),
    );
    layout_as("Nueva presentación", "presentaciones", &body)
}

/// Create a presentation (empty slide deck).
pub async fn create(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SlideForm>,
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

    let mut thumbnails = String::new();
    let mut editors = String::new();
    let total = pres.slides.len();
    for (i, slide) in pres.slides.iter().enumerate() {
        let title_esc = templates::html_escape(&slide.title);
        let content_esc = templates::html_escape(&slide.content);
        let preview_html = md_to_html(&slide.content);
        let number = i + 1;
        let pressed = if i == 0 { "true" } else { "false" };
        let hidden = if i == 0 { "" } else { "hidden" };
        thumbnails.push_str(&format!(
            r##"<article class="gen-office-thumb" id="slide-{i}" data-idx="{i}"><button type="button" class="gen-office-thumb-select" data-slide-target="{i}" onclick="selectOfficeSlide({i})" aria-pressed="{pressed}"><span class="gen-office-thumb-number">{number}</span><span class="gen-office-thumb-canvas"><strong>{title}</strong><span>{content}</span></span></button><footer><span>{number} / {total}</span><div><button type="button" onclick="moveSlide({i}, -1)" class="gen-office-icon-button" aria-label="Mover diapositiva hacia arriba"><span class="material-symbols-outlined">arrow_upward</span></button><button type="button" onclick="moveSlide({i}, 1)" class="gen-office-icon-button" aria-label="Mover diapositiva hacia abajo"><span class="material-symbols-outlined">arrow_downward</span></button><form hx-delete="/office/presentations/{slug}/slides/{i}"><button type="submit" onclick="return confirm('¿Eliminar diapositiva?')" class="gen-office-icon-button danger" aria-label="Eliminar diapositiva"><span class="material-symbols-outlined">delete</span></button></form></div></footer></article>"##,
            i = i,
            number = number,
            total = total,
            pressed = pressed,
            slug = templates::html_escape(&slug),
            title = title_esc,
            content = content_esc,
        ));
        editors.push_str(&format!(
            r##"<section class="gen-office-slide-editor" data-slide-panel="{i}" {hidden}><div class="gen-office-slide-canvas" id="slide-preview-{i}"><div class="gen-office-slide-brand">NOVA · ESCENA {number}</div><h2>{title}</h2><div class="gen-office-slide-content" data-preview-content>{preview}</div><footer>{deck_title}</footer></div><form hx-put="/office/presentations/{slug}/slides/{i}" class="gen-office-slide-form"><div><label for="slide-title-{i}">Título</label><input id="slide-title-{i}" name="title" value="{title}" placeholder="Título de la diapositiva" oninput="syncOfficeSlidePreview({i})" required></div><div><label for="slide-content-{i}">Contenido Markdown</label><textarea id="slide-content-{i}" name="content" rows="6" oninput="syncOfficeSlidePreview({i})">{content}</textarea></div><footer><span>La vista se actualiza al escribir</span><button type="submit" class="gen-office-primary-button"><span class="material-symbols-outlined">save</span>Guardar diapositiva</button></footer></form></section>"##,
            i = i,
            number = number,
            hidden = hidden,
            slug = templates::html_escape(&slug),
            title = title_esc,
            content = content_esc,
            preview = preview_html,
            deck_title = templates::html_escape(&pres.title),
        ));
    }
    if pres.slides.is_empty() {
        editors.push_str(
            r##"<div class="gen-office-empty gen-office-slide-empty"><span class="material-symbols-outlined">present_to_all</span><h3>Esta presentación aún no tiene escenas.</h3><p>Añade la primera desde el panel de herramientas.</p><button type="button" class="gen-office-primary-button" onclick="openNewSlide()">Añadir diapositiva</button></div>"##,
        );
    }

    let ai_content = format!(
        r##"<div class="gen-office-tool-list"><a href="/office/presentations/{slug}/present" target="_blank"><span class="material-symbols-outlined">play_arrow</span><span><strong>Iniciar presentación</strong><small>Abrir el modo de una escena a la vez</small></span></a></div><section class="gen-office-new-slide" id="gen-office-new-slide"><div><span>NUEVA ESCENA</span><h3>Añadir diapositiva</h3></div><form hx-post="/office/presentations/{slug}/slides"><label for="new-slide-title">Título</label><input id="new-slide-title" name="title" placeholder="Una idea clara" required><label for="new-slide-content">Contenido Markdown</label><textarea id="new-slide-content" name="content" rows="7" placeholder="Idea principal&#10;&#10;Escribe tu contenido…"></textarea><button type="submit" class="gen-office-primary-button"><span class="material-symbols-outlined">add</span>Añadir diapositiva</button></form></section>"##,
        slug = templates::html_escape(&slug),
    );
    let slug_js = serde_json::to_string(&slug).unwrap_or_else(|_| "\"\"".to_string());
    let deck_js = r##"
var genOfficeDeckSlug = __SLUG__;
function selectOfficeSlide(index) {
    document.querySelectorAll('[data-slide-panel]').forEach((panel) => {
        panel.hidden = panel.getAttribute('data-slide-panel') !== String(index);
    });
    document.querySelectorAll('[data-slide-target]').forEach((button) => {
        button.setAttribute('aria-pressed', String(button.getAttribute('data-slide-target') === String(index)));
    });
    const total = document.querySelectorAll('[data-slide-panel]').length;
    const position = document.getElementById('slide-position');
    if (position && total) position.textContent = (index + 1) + ' / ' + total;
}
function syncOfficeSlidePreview(index) {
    const panel = document.querySelector('[data-slide-panel="' + index + '"]');
    if (!panel) return;
    const title = panel.querySelector('input[name="title"]');
    const content = panel.querySelector('textarea[name="content"]');
    const preview = panel.querySelector('.gen-office-slide-canvas');
    if (preview && title) preview.querySelector('h2').textContent = title.value;
    if (preview && content) preview.querySelector('[data-preview-content]').textContent = content.value;
}
function openNewSlide() {
    openOfficeAi();
    window.setTimeout(() => document.getElementById('new-slide-title')?.focus(), 30);
}
function buildOrder() {
    const order = Array.from(document.querySelectorAll('#slides [data-idx]')).map((el) => el.getAttribute('data-idx'));
    const input = document.getElementById('order');
    if (input) input.value = order.join(',');
    return order.join(',');
}
function moveSlide(idx, delta) {
    const order = Array.from(document.querySelectorAll('#slides [data-idx]')).map((el) => parseInt(el.getAttribute('data-idx'), 10));
    const current = order.indexOf(idx);
    if (current === -1) return;
    const destination = current + delta;
    if (destination < 0 || destination >= order.length) return;
    order.splice(current, 1);
    order.splice(destination, 0, idx);
    const value = order.join(',');
    const input = document.getElementById('order');
    if (input) input.value = value;
    fetch('/office/presentations/' + encodeURIComponent(genOfficeDeckSlug) + '/slides/reorder', {
        method: 'POST',
        headers: {'Content-Type': 'application/x-www-form-urlencoded'},
        body: 'order=' + encodeURIComponent(value)
    }).then((response) => {
        if (!response.ok) throw new Error('HTTP ' + response.status);
        window.location.reload();
    }).catch(() => {
        window.alert('No se pudo guardar el nuevo orden. Inténtalo de nuevo.');
    });
}
"##.replace("__SLUG__", &slug_js);

    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-editor-page">{tabs}<header class="gen-office-ribbon"><nav class="gen-office-menubar" aria-label="Acciones de la presentación"><a class="is-active" href="/office/presentations/{slug}">Inicio</a><button type="button" onclick="openNewSlide()">Nueva diapositiva</button><a href="/office/presentations/{slug}/present" target="_blank">Presentar</a></nav><div class="gen-office-ribbon-actions">{ai_toggle}<a class="gen-office-primary-button" href="/office/presentations/{slug}/present" target="_blank"><span class="material-symbols-outlined">play_arrow</span>Presentar</a><form hx-delete="/office/presentations/{slug}"><button type="submit" onclick="return confirm('¿Borrar presentación?')" class="gen-office-icon-button danger" aria-label="Borrar presentación"><span class="material-symbols-outlined">delete</span></button></form></div></header><div class="gen-office-workspace">{ai}<main class="gen-office-editor-main"><div class="gen-office-canvas-head"><div><span>PRESENTACIÓN</span><strong>{title}</strong></div><span class="gen-office-local-badge"><span class="material-symbols-outlined">hard_drive</span>Archivo local</span></div><section class="gen-office-deck-stage"><aside class="gen-office-thumbnails"><header><span>DIAPOSITIVAS</span><button type="button" class="gen-office-icon-button" onclick="openNewSlide()" aria-label="Añadir diapositiva"><span class="material-symbols-outlined">add</span></button></header><div id="slides" class="gen-office-slide-list">{thumbnails}</div><form hx-post="/office/presentations/{slug}/slides/reorder" hx-on::before-request="buildOrder()" class="gen-office-reorder-form"><input type="hidden" name="order" id="order" value=""><button type="submit" class="gen-office-secondary-button"><span class="material-symbols-outlined">swap_vert</span>Aplicar orden</button></form></aside><div class="gen-office-slide-stage">{editors}</div></section></main></div><footer class="gen-office-statusbar"><span id="slide-position">{position}</span><span>{total} diapositiva(s)</span><span>NOVA Office</span></footer></div><script>{workspace_js}</script><script>{deck_js}</script>"##,
        tabs = templates::office_tab_strip("presentations", &pres.title, "/office/presentations/new"),
        slug = templates::html_escape(&slug),
        ai_toggle = templates::OFFICE_AI_TOGGLE,
        ai = templates::office_ai_panel("Construye la presentación", "Selecciona una escena, edítala en el lienzo y añade nuevas diapositivas desde este panel.", &ai_content),
        title = templates::html_escape(&pres.title),
        thumbnails = thumbnails,
        editors = editors,
        position = if total == 0 { "0 / 0".to_string() } else { format!("1 / {total}") },
        total = total,
        workspace_js = templates::OFFICE_WORKSPACE_JS,
        deck_js = deck_js,
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
    if form.title.trim().is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "slide title is required",
        )
            .into_response();
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
    if form.title.trim().is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "slide title is required",
        )
            .into_response();
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
    if let Err(e) = pres.save_to(&file_path) {
        tracing::error!("office presentations update_slide: save failed: {}", e);
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "No se pudo completar la operación",
        )
            .into_response();
    }
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
    if index >= pres.slides.len() {
        return (axum::http::StatusCode::NOT_FOUND, "slide not found").into_response();
    }
    pres.slides.remove(index);
    if let Err(e) = pres.save_to(&file_path) {
        tracing::error!("office presentations delete_slide: save failed: {}", e);
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "No se pudo completar la operación",
        )
            .into_response();
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
    let indices: Vec<usize> = if form.order.trim().is_empty() {
        Vec::new()
    } else {
        match form
            .order
            .split(',')
            .map(|s| s.trim().parse::<usize>())
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(indices) => indices,
            Err(_) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    "order contains an invalid slide index",
                )
                    .into_response()
            }
        }
    };
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
    if let Err(e) = pres.save_to(&file_path) {
        tracing::error!("office presentations reorder: save failed: {}", e);
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "No se pudo completar la operación",
        )
            .into_response();
    }
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
    if let Err(e) = std::fs::remove_file(&file_path) {
        let status = if e.kind() == std::io::ErrorKind::NotFound {
            axum::http::StatusCode::NOT_FOUND
        } else {
            tracing::error!(
                slug_len = slug.len(),
                "office presentations delete: failed: {}",
                e
            );
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        };
        return (
            status,
            layout(
                "Error",
                "<p class=\"text-red-400\">No se pudo borrar la presentación.</p>",
            ),
        )
            .into_response();
    }
    hx_redirect("/office/presentations")
}

/// Presentation mode — una diapositiva visible, con teclado y pantalla completa.
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

    let total = pres.slides.len();
    let mut slides_html = String::new();
    for (index, slide) in pres.slides.iter().enumerate() {
        let number = index + 1;
        let hidden = if index == 0 { "" } else { "hidden" };
        slides_html.push_str(&format!(
            r##"<section class="gen-office-present-slide" data-present-slide="{index}" {hidden}><div class="gen-office-present-canvas"><header><span>NOVA</span><span>{number:02} / {total:02}</span></header><div><p>ESCENA {number}</p><h2>{title}</h2><div class="gen-office-present-body">{body}</div></div><footer><span>{deck}</span><span>{number} de {total}</span></footer></div></section>"##,
            index = index,
            hidden = hidden,
            number = number,
            total = total,
            title = templates::html_escape(&slide.title),
            body = md_to_html(&slide.content),
            deck = templates::html_escape(&pres.title),
        ));
    }
    if total == 0 {
        slides_html.push_str(r##"<div class="gen-office-empty"><span class="material-symbols-outlined">present_to_all</span><h3>No hay diapositivas para presentar.</h3><p>Vuelve al editor y añade la primera escena.</p></div>"##);
    }
    let disabled = if total == 0 { "disabled" } else { "" };
    let present_js = r##"
var genPresentationIndex = 0;
var genPresentationTotal = __TOTAL__;
function genPresentationShow(index) {
    if (!genPresentationTotal) return;
    genPresentationIndex = Math.max(0, Math.min(index, genPresentationTotal - 1));
    document.querySelectorAll('[data-present-slide]').forEach((slide) => {
        slide.hidden = slide.getAttribute('data-present-slide') !== String(genPresentationIndex);
    });
    const position = document.getElementById('presentation-position');
    if (position) position.textContent = (genPresentationIndex + 1) + ' / ' + genPresentationTotal;
    const previous = document.getElementById('presentation-prev');
    const next = document.getElementById('presentation-next');
    if (previous) previous.disabled = genPresentationIndex === 0;
    if (next) next.disabled = genPresentationIndex === genPresentationTotal - 1;
}
function genPresentationStep(delta) {
    genPresentationShow(genPresentationIndex + delta);
}
function genOfficeFullscreen() {
    const stage = document.querySelector('.gen-office-present-stage');
    if (!stage) return;
    if (document.fullscreenElement) document.exitFullscreen();
    else if (stage.requestFullscreen) stage.requestFullscreen();
}
document.addEventListener('keydown', (event) => {
    if (event.key === 'ArrowRight' || event.key === 'PageDown' || event.key === ' ') {
        event.preventDefault(); genPresentationStep(1);
    }
    if (event.key === 'ArrowLeft' || event.key === 'PageUp') {
        event.preventDefault(); genPresentationStep(-1);
    }
    if (event.key === 'Home') genPresentationShow(0);
    if (event.key === 'End') genPresentationShow(genPresentationTotal - 1);
});
genPresentationShow(0);
"##
    .replace("__TOTAL__", &total.to_string());

    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-editor-page gen-office-present-mode">{tabs}<header class="gen-office-ribbon"><nav class="gen-office-menubar" aria-label="Acciones de presentación"><a href="/office/presentations/{slug}"><span class="material-symbols-outlined">arrow_back</span>Volver al editor</a><button type="button" class="is-active" onclick="genOfficeFullscreen()">Pantalla completa</button></nav><div class="gen-office-ribbon-actions"><a class="gen-office-primary-button" href="/office/presentations/{slug}"><span class="material-symbols-outlined">edit</span>Editar</a></div></header><main class="gen-office-present-stage">{slides}</main><footer class="gen-office-present-controls"><button type="button" id="presentation-prev" onclick="genPresentationStep(-1)" {disabled}><span class="material-symbols-outlined">arrow_back</span>Anterior</button><span id="presentation-position">{position}</span><button type="button" id="presentation-next" onclick="genPresentationStep(1)" {disabled}>Siguiente<span class="material-symbols-outlined">arrow_forward</span></button></footer></div><script>{present_js}</script>"##,
        tabs =
            templates::office_tab_strip("presentations", &pres.title, "/office/presentations/new"),
        slug = templates::html_escape(&slug),
        slides = slides_html,
        disabled = disabled,
        position = if total == 0 {
            "0 / 0".to_string()
        } else {
            format!("1 / {total}")
        },
        present_js = present_js,
    );
    layout_as("Presentar", "presentaciones", &body).into_response()
}
