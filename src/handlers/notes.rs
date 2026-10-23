//! Notes handlers — CRUD for Markdown second-brain notes.
//!
//! Notes are stored as:
//! - Metadata (title, slug, tags, word_count, timestamps) in SQLite
//! - Content as Markdown file on disk (Obsidian-compatible)
//! - FTS5 index for full-text search

use axum::{
    extract::{Path, Query, State},
    http::{
        header::{CONTENT_DISPOSITION, CONTENT_TYPE},
        HeaderValue, StatusCode,
    },
    response::{IntoResponse, Redirect, Response},
    Form, Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::models::{CreateNoteRequest, Note, UpdateNoteRequest};
use crate::ui::stitch::html_escape;
use crate::AppState;

/// Generic public message for 500-class failures (never leaks internals).
const INTERNAL_ERROR: &str = "Internal error";

/// Input bounds (400 on violation).
const MAX_TITLE_LEN: usize = 200;
const MAX_CONTENT_LEN: usize = 500_000;
const MAX_TAGS_LEN: usize = 500;
const MAX_SLUG_LEN: usize = 200;

/// Query params accepted by the notes list page: `?tag=<slug>` filters by tag.
#[derive(Debug, Default, Deserialize)]
pub struct NotesQuery {
    /// Slug of the tag to filter by (absent or empty = no filter).
    pub tag: Option<String>,
}

/// Slug guard: rejects empty strings, `.` / `..`, separators, `..` sequences.
fn is_safe_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_SLUG_LEN
        && !s.contains("..")
        && !s.contains('/')
        && !s.contains('\\')
        && s != "."
        && s != ".."
}

fn bad_slug_json() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "Invalid slug" })),
    )
}

/// Motivo por el que una nota no se puede mostrar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotaAusente {
    /// No hay ninguna fila con ese slug en la base de datos.
    SinRegistro,
    /// El identificador no es válido (`..`, `/`, `\`, vacío o demasiado largo):
    /// no se busca ni en la base de datos ni en el disco.
    IdentificadorInvalido,
    /// La nota está en la base de datos pero su archivo Markdown no se puede
    /// leer: hay registro, pero no hay cuerpo que servir.
    ArchivoAusente,
    /// La versión del historial no existe o no pertenece a esta nota.
    VersionAusente,
}

/// Recorta un texto largo para mostrarlo en la página sin romper el diseño.
fn clip(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// Página 404 con el mismo aspecto visual que el resto de la aplicación.
///
/// No es un texto plano: reutiliza el shell de `page_ext` (nav-rail, barra
/// superior y tokens del tema) y ofrece una salida clara ("Volver a las
/// notas"). El identificador se muestra escapado y recortado, nunca crudo.
fn nota_ausente_page(slug: &str, motivo: NotaAusente) -> String {
    let (icono, titulo, detalle) = match motivo {
        NotaAusente::SinRegistro => (
            "search_off",
            "Nota no encontrada",
            "No hay ninguna nota con este identificador en tu bóveda.",
        ),
        NotaAusente::IdentificadorInvalido => (
            "block",
            "Nota no encontrada",
            "El identificador de la nota no es válido, así que no se ha buscado ni en la base de datos ni en el disco.",
        ),
        NotaAusente::ArchivoAusente => (
            "draft",
            "Nota no encontrada",
            "La nota está registrada en la bóveda, pero su archivo Markdown no se ha podido leer.",
        ),
        NotaAusente::VersionAusente => (
            "history",
            "Versión no encontrada",
            "Esta versión del historial no existe o no pertenece a esta nota.",
        ),
    };

    let content = format!(
        r#"<div class="flex items-center gap-space-sm mb-space-md">
            <a href="/app/notes" class="font-label-sm text-label-sm text-on-surface-variant hover:text-on-surface transition-colors">
                <span class="material-symbols-outlined text-[16px]">arrow_back</span> Mis notas
            </a>
        </div>
        <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
            <span class="material-symbols-outlined text-[32px] text-outline mb-2">{icono}</span>
            <h1 class="font-headline-md text-headline-md text-on-surface mb-space-sm">{titulo}</h1>
            <p class="font-body-sm text-body-sm text-on-surface-variant mb-space-sm">{detalle}</p>
            <p class="font-body-sm text-body-sm text-outline mb-space-md">
                Identificador: <span class="font-code-inline text-on-surface-variant">{slug}</span>
            </p>
            <p class="font-code-inline text-[11px] text-outline mb-space-md">HTTP 404 · Not Found</p>
            <a href="/app/notes" class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-primary-container text-on-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors">
                <span class="material-symbols-outlined text-[16px]">arrow_back</span> Volver a las notas
            </a>
        </div>"#,
        icono = icono,
        titulo = titulo,
        detalle = detalle,
        slug = html_escape(&clip(slug, 64)),
    );

    crate::academic::templates::page_ext(
        &format!("{} — Study Platform", titulo),
        "Mis notas",
        "/dashboard/notes",
        &content,
    )
}

/// Lee el `.md` de una nota desde el disco.
///
/// Nunca falla: si el archivo no existe o no se puede leer devuelve `None` y
/// deja un `warn` en el log. Así la vista muestra un estado vacío honesto en
/// lugar de romperse con un 500.
fn leer_cuerpo_nota(path: &std::path::Path, slug: &str) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(cuerpo) => Some(cuerpo),
        Err(e) => {
            tracing::warn!(
                slug_len = slug.len(),
                error = %e,
                "notes: no se pudo leer el archivo de la nota; se muestra el estado vacío"
            );
            None
        }
    }
}

/// Estado honesto para una nota cuyo archivo no está disponible en disco.
///
/// La fila de la base de datos existe (título, etiquetas), así que la vista se
/// muestra igual, pero el cuerpo dice claramente que el contenido no se pudo
/// leer en vez de fingir una nota vacía.
fn cuerpo_ausente(slug: &str) -> String {
    format!(
        r#"<div class="flex flex-col items-center gap-2 py-space-lg text-center">
            <span class="material-symbols-outlined text-[28px] text-outline">cloud_off</span>
            <p class="font-body-sm text-body-sm text-on-surface">Contenido no disponible en disco</p>
            <p class="font-body-sm text-body-sm text-on-surface-variant max-w-prose">
                La nota está registrada en la base de datos, pero su archivo Markdown
                (<span class="font-code-inline">{archivo}</span>) no se pudo leer: puede haberse borrado
                o haber salido de la bóveda de notas.
            </p>
        </div>"#,
        archivo = html_escape(&format!("{}.md", clip(slug, 64))),
    )
}

/// Validate note field lengths. Returns `Some(400)` on violation.
fn check_note_fields(
    title: Option<&str>,
    content: Option<&str>,
    tags: Option<&str>,
) -> Option<(StatusCode, Json<serde_json::Value>)> {
    if let Some(t) = title {
        if t.len() > MAX_TITLE_LEN {
            return Some((
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Title too long" })),
            ));
        }
    }
    if let Some(c) = content {
        if c.len() > MAX_CONTENT_LEN {
            return Some((
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Content too large" })),
            ));
        }
    }
    if let Some(t) = tags {
        if t.len() > MAX_TAGS_LEN {
            return Some((
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Tags too long" })),
            ));
        }
    }
    None
}

/// List all notes (HTML page). Supports `?tag=<slug>` to filter by tag.
pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<NotesQuery>,
) -> impl IntoResponse {
    let notes = sqlx::query_as::<_, Note>("SELECT * FROM notes ORDER BY updated_at DESC")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    // Tag chips are built from the tags actually present in the DB (honest counts).
    let tag_index = build_tag_index(&notes);
    let active = params
        .tag
        .as_deref()
        .map(tag_slug)
        .filter(|s| !s.is_empty());

    let visible: Vec<&Note> = match active.as_deref() {
        Some(slug) => notes.iter().filter(|n| note_has_tag(n, slug)).collect(),
        None => notes.iter().collect(),
    };

    let notes_html: String = visible
        .iter()
        .map(|n| {
            format!(
                r#"<li class="mb-2">
                    <a href="/app/notes/{}" class="text-primary-container hover:text-primary-fixed-dim">{}</a>
                    <span class="text-xs text-outline ml-2">{}</span>
                </li>"#,
                n.slug,
                html_escape(&n.title),
                render_tag_links(&n.tags)
            )
        })
        .collect::<Vec<_>>()
        .join("");

    // Empty states: honest, never invent notes.
    let empty_msg = if visible.is_empty() {
        if notes.is_empty() {
            r#"<p class="font-body-sm text-body-sm text-on-surface-variant">No tienes notas aún. ¡Crea una!</p>"#.to_string()
        } else {
            let label = tag_index
                .iter()
                .find(|(slug, _, _)| Some(slug.as_str()) == active.as_deref())
                .map(|(_, label, _)| label.clone())
                .unwrap_or_else(|| active.clone().unwrap_or_default());
            format!(
                r#"<p class="font-body-sm text-body-sm text-on-surface-variant">Ninguna nota tiene la etiqueta <span class="text-on-surface font-semibold">#{label}</span>. <a href="/app/notes" class="text-primary-container hover:underline">Ver todas las notas</a></p>"#,
                label = html_escape(&label),
            )
        }
    } else {
        String::new()
    };

    let count_line = match active.as_deref() {
        Some(_) => format!("{} de {} notas", visible.len(), notes.len()),
        None => format!("{} notas", notes.len()),
    };

    let filter_line = match active.as_deref() {
        Some(slug) => {
            let label = tag_index
                .iter()
                .find(|(s, _, _)| s == slug)
                .map(|(_, label, _)| label.clone())
                .unwrap_or_else(|| slug.to_string());
            format!(
                r#"<a href="/app/notes" class="inline-flex items-center gap-1 mt-1 px-space-sm py-0.5 rounded-full bg-primary-container/15 text-primary-container font-label-sm hover:bg-primary-container/25 transition-colors">#{label} <span class="material-symbols-outlined text-[14px]">close</span></a>"#,
                label = html_escape(&label),
            )
        }
        None => String::new(),
    };

    let chips_html = render_tag_chips(&tag_index, notes.len(), active.as_deref());

    let content = format!(
        r#"
        <div class="flex justify-between items-center mb-space-md">
            <div>
                <h2 class="font-headline-sm text-headline-sm text-on-surface">{count_line}</h2>
                {filter_line}
            </div>
            <a href="/app/notes/new" class="flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-secondary text-on-secondary font-label-md font-semibold hover:bg-secondary-fixed transition-all">
                <span class="material-symbols-outlined text-[16px]">note_add</span>
                <span>+ Nueva nota</span>
            </a>
        </div>
        {chips_html}
        <ul class="space-y-space-xs">
            {notes_html}
        </ul>
        {empty_msg}
        "#,
        count_line = count_line,
        filter_line = filter_line,
        chips_html = chips_html,
        notes_html = notes_html,
        empty_msg = empty_msg,
    );

    let html = crate::academic::templates::page_ext(
        "Notas — Study Platform",
        "Mis notas",
        "/dashboard/notes",
        &content,
    );

    axum::response::Html(html)
}

/// API: List notes (JSON, with pagination).
pub async fn api_list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let notes = sqlx::query_as::<_, Note>("SELECT * FROM notes ORDER BY updated_at DESC LIMIT 100")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let response = json!({
        "notes": notes,
        "count": notes.len(),
    });

    (StatusCode::OK, Json(response))
}

/// View a single note.
pub async fn view(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    // Un identificador peligroso (`..`, `/`, `\`) no se busca en la BD ni en el
    // disco: se responde 404 con la página de estilo de la app.
    if !is_safe_slug(&slug) {
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::IdentificadorInvalido)),
        );
    }
    let note = sqlx::query_as::<_, Note>("SELECT * FROM notes WHERE slug = ?")
        .bind(&slug)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

    let Some(n) = note else {
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::SinRegistro)),
        );
    };

    // El slug guardado en la BD también se valida antes de construir la ruta:
    // una fila corrupta nunca puede sacar la lectura fuera de `notes_dir`.
    if !is_safe_slug(&n.slug) {
        tracing::warn!(
            slug_len = n.slug.len(),
            "notes view: slug inválido en la base de datos; se responde 404 sin tocar el disco"
        );
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::IdentificadorInvalido)),
        );
    }

    let content_path = state.config.notes_dir.join(format!("{}.md", n.slug));

    let (md_html, toc) = match leer_cuerpo_nota(&content_path, &n.slug) {
        Some(blog_body) => {
            // Renderer Markdown (src/knowledge/markdown.rs): los wikilinks se
            // resuelven contra el conjunto de notas conocidas (slugs reales).
            let doc = crate::knowledge::markdown::parse(&blog_body);
            let notas_conocidas = wikilink_set(&state.db).await;
            let html = crate::knowledge::markdown::render_html(&doc, &notas_conocidas);
            // Índice de la nota: solo con >=2 headings. Los `id` de los anchors
            // los genera `markdown` (los mismos que emite `render_html`), así
            // que aquí no se replica esa lógica.
            let toc = note_toc(&doc);
            (html, toc)
        }
        // El archivo no está (o no se puede leer): la nota se muestra con un
        // estado vacío honesto, nunca con un 500.
        None => (cuerpo_ausente(&n.slug), String::new()),
    };

    // Metadatos de la cabecera: fechas y palabras de la tabla `notes` y el nº
    // de versiones real de `note_versions` (nunca un número inventado).
    let meta = load_note_meta(&state.db, &n).await;
    let versions = load_note_versions(&state.db, n.id).await;

    let html = render_note(&n, &md_html, &toc, &meta, &versions);
    (StatusCode::OK, axum::response::Html(html))
}

/// Descarga la nota en Markdown: `GET /app/notes/{slug}/export`.
///
/// Devuelve el `.md` tal cual está en disco con `Content-Disposition:
/// attachment`, para que el navegador lo guarde con su nombre en lugar de
/// mostrarlo dentro de la página.
pub async fn export(State(state): State<Arc<AppState>>, Path(slug): Path<String>) -> Response {
    if !is_safe_slug(&slug) {
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::IdentificadorInvalido)),
        )
            .into_response();
    }
    let note = sqlx::query_as::<_, Note>("SELECT * FROM notes WHERE slug = ?")
        .bind(&slug)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

    let Some(note) = note else {
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::SinRegistro)),
        )
            .into_response();
    };

    // El cuerpo vive en el disco: sin fichero no hay nada que exportar.
    let content_path = state.config.notes_dir.join(format!("{}.md", note.slug));
    let Ok(content) = std::fs::read_to_string(&content_path) else {
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::ArchivoAusente)),
        )
            .into_response();
    };

    let mut resp = (StatusCode::OK, content).into_response();
    let headers = resp.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );
    if let Ok(disposition) = HeaderValue::from_str(&export_content_disposition(&note)) {
        headers.insert(CONTENT_DISPOSITION, disposition);
    }
    resp
}

/// Nombre real del archivo de la nota: el `filename` de la base de datos.
///
/// No se re-deriva del slug (el nombre guardado puede no coincidir con él) y
/// nunca se inventa: si la fila viniera vacía se usa `{slug}.md`.
fn export_real_filename(note: &Note) -> String {
    let name = note.filename.trim();
    if name.is_empty() {
        format!("{}.md", note.slug)
    } else {
        name.to_string()
    }
}

/// ¿El nombre se puede poner tal cual en el parámetro `filename=`?
fn is_ascii_safe_filename(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// Versión ASCII y segura del nombre real, para el parámetro `filename=`.
/// Los caracteres no permitidos se sustituyen por `_`; si no queda nada
/// aprovechable se usa el slug saneado. Siempre conserva la extensión
/// `.md`, que es la del archivo exportado.
fn export_ascii_filename(note: &Note) -> String {
    let sanea = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };

    let mut out = sanea(&export_real_filename(note));
    if out.trim_matches(|c| c == '_' || c == '.').is_empty() {
        out = sanea(&note.slug);
    }
    if out.is_empty() {
        out.push_str("nota");
    }
    if !out.ends_with(".md") {
        out.push_str(".md");
    }
    out
}

/// Porcentaje-codificación de RFC 5987 para el parámetro `filename*`.
fn percent_encode_rfc5987(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let c = *byte as char;
        let attr_char = c.is_ascii_alphanumeric() || "!#$&+-.^_`|~".contains(c);
        if attr_char {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

/// Cabecera `Content-Disposition` de la descarga.
///
/// Lleva el nombre real del archivo (el `filename` de la BD). Cuando ese
/// nombre tiene caracteres no ASCII (p. ej. `física-ii.md`) se conserva tal
/// cual en `filename*` (RFC 5987) y `filename` lleva una variante ASCII
/// segura: sin eso la cabecera sería inválida, el navegador no la aceptaría
/// y el Markdown se mostraría en vez de descargarse.
fn export_content_disposition(note: &Note) -> String {
    let real = export_real_filename(note);
    if is_ascii_safe_filename(&real) {
        return format!("attachment; filename=\"{}\"", real);
    }
    format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        export_ascii_filename(note),
        percent_encode_rfc5987(&real)
    )
}

/// Restaura una versión del historial:
/// `POST /app/notes/{slug}/versions/{version_id}/restore`.
///
/// Escribe el contenido de la instantánea en el `.md`, actualiza los
/// metadatos y guarda una instantánea nueva (el historial no se pierde).
/// Responde con un 303 a la vista de la nota.
pub async fn restore_version(
    State(state): State<Arc<AppState>>,
    Path((slug, version_id)): Path<(String, i64)>,
) -> Response {
    if !is_safe_slug(&slug) {
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::IdentificadorInvalido)),
        )
            .into_response();
    }
    let note = sqlx::query_as::<_, Note>("SELECT * FROM notes WHERE slug = ?")
        .bind(&slug)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

    let Some(note) = note else {
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::SinRegistro)),
        )
            .into_response();
    };

    // La versión tiene que existir Y pertenecer a esta nota.
    let version: Option<(String, i64)> = sqlx::query_as(
        "SELECT content, word_count FROM note_versions WHERE id = ? AND note_id = ?",
    )
    .bind(version_id)
    .bind(note.id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    let Some((content, word_count)) = version else {
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::VersionAusente)),
        )
            .into_response();
    };

    let content_path = state.config.notes_dir.join(format!("{}.md", note.slug));
    if let Err(e) = std::fs::write(&content_path, &content) {
        tracing::error!(
            slug_len = slug.len(),
            "notes restore: file write failed: {}",
            e
        );
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::response::Html(INTERNAL_ERROR.to_string()),
        )
            .into_response();
    }

    let now = chrono::Utc::now().to_rfc3339();
    if let Err(e) = sqlx::query("UPDATE notes SET word_count = ?, updated_at = ? WHERE slug = ?")
        .bind(word_count)
        .bind(&now)
        .bind(&note.slug)
        .execute(&state.db)
        .await
    {
        tracing::error!(
            slug_len = slug.len(),
            "notes restore: metadata update failed: {}",
            e
        );
    }

    // Instantánea nueva con el contenido restaurado: el historial se conserva
    // y el índice de búsqueda sigue viendo el cuerpo actual.
    //
    // Solo si el contenido cambia: restaurar dos veces la misma versión no
    // debe apilar copias idénticas del mismo texto en el historial.
    if !ultima_version_tiene_contenido(&state.db, note.id, &content).await {
        snapshot_version(&state.db, note.id, &content, word_count, &now).await;
    }

    Redirect::to(&format!("/app/notes/{}", note.slug)).into_response()
}

/// ¿La última versión guardada de la nota tiene exactamente este contenido?
///
/// Se usa para no duplicar instantáneas idénticas: si el historial ya
/// termina con este texto, guardar otra copia sería ruido.
async fn ultima_version_tiene_contenido(
    db: &sqlx::SqlitePool,
    note_id: i64,
    content: &str,
) -> bool {
    let ultima: Option<String> = sqlx::query_scalar(
        "SELECT content FROM note_versions WHERE note_id = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(note_id)
    .fetch_optional(db)
    .await
    .unwrap_or(None);

    ultima.as_deref() == Some(content)
}

/// API: Get a single note with content.
pub async fn api_view(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !is_safe_slug(&slug) {
        return bad_slug_json();
    }
    let note = sqlx::query_as::<_, Note>("SELECT * FROM notes WHERE slug = ?")
        .bind(&slug)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

    match note {
        Some(n) => {
            // Igual que en la vista: el slug de la BD se valida antes de
            // construir la ruta del archivo.
            if !is_safe_slug(&n.slug) {
                tracing::warn!(
                    slug_len = n.slug.len(),
                    "notes api_view: slug inválido en la base de datos; se responde 404 sin tocar el disco"
                );
                return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" })));
            }
            let content_path = state.config.notes_dir.join(format!("{}.md", n.slug));
            let (content, content_available) = match leer_cuerpo_nota(&content_path, &n.slug) {
                Some(cuerpo) => (cuerpo, true),
                None => (String::new(), false),
            };

            (
                StatusCode::OK,
                Json(json!({
                    "note": n,
                    "content": content,
                    "content_available": content_available,
                })),
            )
        }
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
    }
}

/// Edit note form.
pub async fn edit(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !is_safe_slug(&slug) {
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::IdentificadorInvalido)),
        );
    }
    let note = sqlx::query_as::<_, Note>("SELECT * FROM notes WHERE slug = ?")
        .bind(&slug)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

    let Some(n) = note else {
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::SinRegistro)),
        );
    };

    if !is_safe_slug(&n.slug) {
        tracing::warn!(
            slug_len = n.slug.len(),
            "notes edit: slug inválido en la base de datos; se responde 404 sin tocar el disco"
        );
        return (
            StatusCode::NOT_FOUND,
            axum::response::Html(nota_ausente_page(&slug, NotaAusente::IdentificadorInvalido)),
        );
    }

    let content_path = state.config.notes_dir.join(format!("{}.md", n.slug));
    let (content, sin_archivo) = match leer_cuerpo_nota(&content_path, &n.slug) {
        Some(cuerpo) => (cuerpo, false),
        None => (String::new(), true),
    };

    (
        StatusCode::OK,
        axum::response::Html(render_note_editor(&n, &content, sin_archivo)),
    )
}

/// Create note form (new).
pub async fn new_note() -> impl IntoResponse {
    let content = r#"
        <div class="flex items-center gap-2 mb-space-md">
            <a href="/app/notes" class="flex items-center gap-1.5 text-outline hover:text-on-surface transition-colors">
                <span class="material-symbols-outlined text-[18px]">arrow_back</span>
                Volver a Mis notas
            </a>
            <h1 class="font-headline-md text-headline-md text-primary-container">Nueva nota</h1>
        </div>
        <form hx-post="/app/notes" hx-swap="outerHTML" class="space-y-space-md">
            <div>
                <label class="block font-label-sm text-label-sm text-on-surface-variant mb-space-xs">Título</label>
                <input type="text" name="title" id="title" required onkeyup="updateSlug()"
                    class="w-full px-space-sm py-2 bg-surface-container border border-outline-variant rounded font-body-md text-on-surface focus:outline-none focus:ring-1 focus:ring-primary-container">
            </div>
            <div>
                <label class="block font-label-sm text-label-sm text-on-surface-variant mb-space-xs">Slug (auto)</label>
                <span id="slug-preview" class="text-primary-container font-code-inline">sin-titulo</span>
            </div>
            <div>
                <label class="block font-label-sm text-label-sm text-on-surface-variant mb-space-xs">Contenido (Markdown)</label>
                <textarea name="content" rows="20"
                    class="w-full px-space-sm py-2 bg-surface-container border border-outline-variant rounded text-on-surface focus:outline-none focus:ring-1 focus:ring-primary-container font-code-inline text-code-inline"></textarea>
            </div>
            <div>
                <label class="block font-label-sm text-label-sm text-on-surface-variant mb-space-xs">Tags (separados por coma)</label>
                <input type="text" name="tags" placeholder="ej: calculo, examen-final"
                    class="w-full px-space-sm py-2 bg-surface-container border border-outline-variant rounded text-on-surface focus:outline-none focus:ring-1 focus:ring-primary-container">
            </div>
            <button type="submit" class="flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-secondary text-on-secondary font-label-md font-semibold hover:bg-secondary-fixed transition-all shadow-md">
                <span class="material-symbols-outlined text-[16px]">save</span>
                <span>Crear nota</span>
            </button>
        </form>
        <script>
            function updateSlug() {
                const title = document.getElementById('title').value;
                const slug = title.toLowerCase().replace(/[^a-z0-9\s-]/g, '').replace(/\s+/g, '-');
                document.getElementById('slug-preview').textContent = slug || 'sin-titulo';
            }
        </script>
        "#;

    let html = crate::academic::templates::page_ext(
        "Nueva nota — Study Platform",
        "Mis notas",
        "/dashboard/notes",
        content,
    );

    axum::response::Html(html)
}

/// Create a new note.
pub async fn create(State(state): State<Arc<AppState>>, form: Form<CreateNoteRequest>) -> Response {
    // Ensure notes directory exists
    let _ = std::fs::create_dir_all(&state.config.notes_dir);

    if form.title.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Title is required" })),
        )
            .into_response();
    }
    if let Some(err) = check_note_fields(
        Some(&form.title),
        form.content.as_deref(),
        form.tags.as_deref(),
    ) {
        return err.into_response();
    }

    let slug = Note::slugify(&form.title);
    if slug.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid title" })),
        )
            .into_response();
    }
    let filename = format!("{}.md", &slug);
    let file_path = state.config.notes_dir.join(&filename);

    // Check if note with this slug already exists
    let existing: Option<Note> = sqlx::query_as("SELECT * FROM notes WHERE slug = ?")
        .bind(&slug)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

    if existing.is_some() {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "Note with this title already exists" })),
        )
            .into_response();
    }

    // Write content to file
    if let Some(content) = &form.content {
        if let Err(e) = std::fs::write(&file_path, content) {
            tracing::error!(
                slug_len = slug.len(),
                "notes create: file write failed: {}",
                e
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": INTERNAL_ERROR })),
            )
                .into_response();
        }
    } else {
        let _ = std::fs::write(&file_path, "");
    }

    let word_count = form
        .content
        .as_ref()
        .map(|c| c.split_whitespace().count() as i64)
        .unwrap_or(0);

    let now = chrono::Utc::now().to_rfc3339();

    let result = sqlx::query(
        r#"
        INSERT INTO notes (title, slug, filename, tags, word_count, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&form.title)
    .bind(&slug)
    .bind(&filename)
    .bind(&form.tags.clone().unwrap_or_default())
    .bind(word_count)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await;

    match result {
        Ok(res) => {
            // Instantánea inicial del cuerpo: desde el primer momento la nota
            // tiene historial (la BD conserva las 5 últimas).
            let note_id = res.last_insert_rowid();
            snapshot_version(
                &state.db,
                note_id,
                form.content.as_deref().unwrap_or(""),
                word_count,
                &now,
            )
            .await;
            // Redirect to the new note's view page (303 See Other, HTMX-compatible)
            Redirect::to(&format!("/app/notes/{}", slug)).into_response()
        }
        Err(e) => {
            tracing::error!(
                slug_len = slug.len(),
                "notes create: db insert failed: {}",
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": INTERNAL_ERROR })),
            )
                .into_response()
        }
    }
}

/// Update an existing note.
pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    form: Form<UpdateNoteRequest>,
) -> impl IntoResponse {
    if !is_safe_slug(&slug) {
        return bad_slug_json();
    }
    if let Some(err) = check_note_fields(
        form.title.as_deref(),
        form.content.as_deref(),
        form.tags.as_deref(),
    ) {
        return err;
    }
    let file_path = state.config.notes_dir.join(format!("{}.md", slug));
    let now = chrono::Utc::now().to_rfc3339();

    // Update file content
    if let Some(content) = &form.content {
        if let Err(e) = std::fs::write(&file_path, content) {
            tracing::error!(
                slug_len = slug.len(),
                "notes update: file write failed: {}",
                e
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": INTERNAL_ERROR })),
            );
        }
    }

    // Update DB metadata
    if let Some(title) = &form.title {
        if let Err(e) = sqlx::query("UPDATE notes SET title = ?, updated_at = ? WHERE slug = ?")
            .bind(title)
            .bind(&now)
            .bind(&slug)
            .execute(&state.db)
            .await
        {
            tracing::error!(
                slug_len = slug.len(),
                "notes update: title db write failed: {}",
                e
            );
        }
    }
    if let Some(tags) = &form.tags {
        if let Err(e) = sqlx::query("UPDATE notes SET tags = ?, updated_at = ? WHERE slug = ?")
            .bind(tags)
            .bind(&now)
            .bind(&slug)
            .execute(&state.db)
            .await
        {
            tracing::error!(
                slug_len = slug.len(),
                "notes update: tags db write failed: {}",
                e
            );
        }
    }
    if let Some(content) = &form.content {
        let word_count = content.split_whitespace().count() as i64;
        if let Err(e) =
            sqlx::query("UPDATE notes SET word_count = ?, updated_at = ? WHERE slug = ?")
                .bind(word_count)
                .bind(&now)
                .bind(&slug)
                .execute(&state.db)
                .await
        {
            tracing::error!(
                slug_len = slug.len(),
                "notes update: word_count db write failed: {}",
                e
            );
        }

        // Instantánea del historial: el cuerpo nuevo queda guardado como
        // versión (la BD conserva las 5 últimas), como hace el servicio de
        // notas. Sin esto la vista no tendría historial que mostrar.
        if let Some(note_id) = sqlx::query_scalar::<_, i64>("SELECT id FROM notes WHERE slug = ?")
            .bind(&slug)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
        {
            snapshot_version(&state.db, note_id, content, word_count, &now).await;
        }
    }

    (StatusCode::OK, Json(json!({ "message": "Note updated" })))
}

/// Delete a note.
pub async fn delete(State(state): State<Arc<AppState>>, Path(slug): Path<String>) -> Response {
    if !is_safe_slug(&slug) {
        return bad_slug_json().into_response();
    }
    // Delete file
    let file_path = state.config.notes_dir.join(format!("{}.md", slug));
    if file_path.exists() {
        let _ = std::fs::remove_file(&file_path);
    }

    // Delete from DB
    let result = sqlx::query("DELETE FROM notes WHERE slug = ?")
        .bind(&slug)
        .execute(&state.db)
        .await;

    match result {
        Ok(res) if res.rows_affected() > 0 => {
            // FTS index cleanup is handled by the notes_ad trigger
            let mut resp =
                (StatusCode::OK, Json(json!({ "message": "Note deleted" }))).into_response();
            // HTMX: vuelve al listado tras borrar (la vista ya no existe).
            resp.headers_mut()
                .insert("HX-Redirect", HeaderValue::from_static("/app/notes"));
            resp
        }
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Note not found" })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(
                slug_len = slug.len(),
                "notes delete: db delete failed: {}",
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": INTERNAL_ERROR })),
            )
                .into_response()
        }
    }
}

/// API: Create note (from JSON).
pub async fn api_create(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateNoteRequest>,
) -> impl IntoResponse {
    if payload.title.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Title is required" })),
        );
    }
    if let Some(err) = check_note_fields(
        Some(&payload.title),
        payload.content.as_deref(),
        payload.tags.as_deref(),
    ) {
        return err;
    }
    let slug = Note::slugify(&payload.title);
    if slug.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid title" })),
        );
    }
    let filename = format!("{}.md", &slug);
    let file_path = state.config.notes_dir.join(&filename);

    let _ = std::fs::create_dir_all(&state.config.notes_dir);

    if let Some(content) = &payload.content {
        if let Err(e) = std::fs::write(&file_path, content) {
            tracing::error!(
                slug_len = slug.len(),
                "notes api_create: file write failed: {}",
                e
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": INTERNAL_ERROR })),
            );
        }
    } else {
        let _ = std::fs::write(&file_path, "");
    }

    let word_count = payload
        .content
        .as_ref()
        .map(|c| c.split_whitespace().count() as i64)
        .unwrap_or(0);

    let now = chrono::Utc::now().to_rfc3339();

    let result = sqlx::query(
        r#"INSERT INTO notes (title, slug, filename, tags, word_count, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&payload.title)
    .bind(&slug)
    .bind(&filename)
    .bind(payload.tags.as_deref().unwrap_or(""))
    .bind(word_count)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await;

    match result {
        Ok(_) => (StatusCode::CREATED, Json(json!({ "slug": slug }))),
        Err(e) => {
            tracing::error!(
                slug_len = slug.len(),
                "notes api_create: db insert failed: {}",
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": INTERNAL_ERROR })),
            )
        }
    }
}

/// API: Update note (from JSON).
pub async fn api_update(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Json(payload): Json<UpdateNoteRequest>,
) -> impl IntoResponse {
    if !is_safe_slug(&slug) {
        return bad_slug_json();
    }
    if let Some(err) = check_note_fields(
        payload.title.as_deref(),
        payload.content.as_deref(),
        payload.tags.as_deref(),
    ) {
        return err;
    }
    let now = chrono::Utc::now().to_rfc3339();
    let file_path = state.config.notes_dir.join(format!("{}.md", slug));

    if let Some(content) = &payload.content {
        let _ = std::fs::write(&file_path, content);
    }

    let result = sqlx::query(
        r#"UPDATE notes SET title = COALESCE(?, title),
           tags = COALESCE(?, tags),
           updated_at = ?
           WHERE slug = ? returning id"#,
    )
    .bind(&payload.title)
    .bind(payload.tags.as_deref())
    .bind(&now)
    .bind(&slug)
    .execute(&state.db)
    .await;

    match result {
        Ok(res) if res.rows_affected() > 0 => (StatusCode::OK, Json(json!({ "updated": true }))),
        Ok(_) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
        Err(e) => {
            tracing::error!(
                slug_len = slug.len(),
                "notes api_update: db update failed: {}",
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": INTERNAL_ERROR })),
            )
        }
    }
}

/// API: Delete note.
pub async fn api_delete(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !is_safe_slug(&slug) {
        return bad_slug_json();
    }
    let file_path = state.config.notes_dir.join(format!("{}.md", slug));
    if file_path.exists() {
        let _ = std::fs::remove_file(&file_path);
    }

    let result = sqlx::query("DELETE FROM notes WHERE slug = ?")
        .bind(&slug)
        .execute(&state.db)
        .await;

    match result {
        Ok(res) if res.rows_affected() > 0 => (StatusCode::OK, Json(json!({ "deleted": true }))),
        Ok(_) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
        Err(e) => {
            tracing::error!(
                slug_len = slug.len(),
                "notes api_delete: db delete failed: {}",
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": INTERNAL_ERROR })),
            )
        }
    }
}

// Helpers

/// Normaliza una etiqueta a un slug comparable y seguro para la URL
/// (minúsculas, acentos plegados, solo `[a-z0-9_-]`).
fn tag_slug(raw: &str) -> String {
    raw.trim()
        .to_lowercase()
        .split_whitespace()
        .map(|word| {
            word.chars()
                .map(|c| match c {
                    'á' => 'a',
                    'é' => 'e',
                    'í' => 'i',
                    'ó' => 'o',
                    'ú' => 'u',
                    'ü' => 'u',
                    'ñ' => 'n',
                    'ç' => 'c',
                    _ => c,
                })
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Índice de etiquetas presentes en la BD: `(slug, etiqueta original, conteo)`.
/// Solo contiene etiquetas que existen de verdad; los conteos son reales.
fn build_tag_index(notes: &[Note]) -> Vec<(String, String, usize)> {
    let mut index: std::collections::BTreeMap<String, (String, usize)> =
        std::collections::BTreeMap::new();
    for note in notes {
        for raw in note.tags.split(',') {
            let label = raw.trim();
            if label.is_empty() {
                continue;
            }
            let slug = tag_slug(label);
            if slug.is_empty() {
                continue;
            }
            let entry = index
                .entry(slug)
                .or_insert_with(|| (label.to_string(), 0usize));
            entry.1 += 1;
        }
    }
    let mut out: Vec<(String, String, usize)> = index
        .into_iter()
        .map(|(slug, (label, count))| (slug, label, count))
        .collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    out
}

/// ¿La nota tiene alguna etiqueta cuyo slug coincide con el filtro?
fn note_has_tag(note: &Note, slug: &str) -> bool {
    note.tags.split(',').any(|raw| tag_slug(raw) == slug)
}

/// Clases Tailwind de un chip de etiqueta (activo / inactivo).
fn chip_class(active: bool) -> &'static str {
    if active {
        "px-space-sm py-1 rounded-full bg-primary-container text-on-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors"
    } else {
        "px-space-sm py-1 rounded-full bg-surface-container text-on-surface-variant font-label-md hover:bg-primary-container/15 hover:text-primary-container transition-colors"
    }
}

/// Barra de chips: «Todas» + una chip por etiqueta existente, con su conteo real.
fn render_tag_chips(
    tag_index: &[(String, String, usize)],
    total: usize,
    active: Option<&str>,
) -> String {
    if tag_index.is_empty() {
        return String::new();
    }
    let mut chips = format!(
        r#"<a href="/app/notes" class="{}">Todas <span class="opacity-70">({total})</span></a>"#,
        chip_class(active.is_none()),
        total = total
    );
    for (slug, label, count) in tag_index {
        chips.push_str(&format!(
            r#"<a href="/app/notes?tag={slug}" class="{cls}">#{label} <span class="opacity-70">({count})</span></a>"#,
            slug = slug,
            label = html_escape(label),
            count = count,
            cls = chip_class(active == Some(slug.as_str())),
        ));
    }
    format!(
        r#"<div class="flex flex-wrap items-center gap-2 mb-space-md" role="group" aria-label="Filtrar notas por etiqueta">{chips}</div>"#,
        chips = chips
    )
}

/// Etiquetas de una nota como enlaces al filtro (vacío si la nota no tiene).
fn render_tag_links(tags: &str) -> String {
    let mut out = String::new();
    for raw in tags.split(',') {
        let label = raw.trim();
        if label.is_empty() {
            continue;
        }
        let slug = tag_slug(label);
        if slug.is_empty() {
            continue;
        }
        out.push_str(&format!(
            r#"<a href="/app/notes?tag={slug}" class="ml-1 px-1.5 py-0.5 rounded bg-surface-container text-[11px] text-on-surface-variant hover:text-primary-container hover:bg-primary-container/15 transition-colors">#{label}</a>"#,
            slug = slug,
            label = html_escape(label),
        ));
    }
    out
}

/// Conjunto de notas conocidas por el resolver de wikilinks.
///
/// Contiene el **slug real** de cada nota (el que forma la URL
/// `/app/notes/{slug}`) y además `Note::slugify(title)`, para que un
/// wikilink escrito con el título (`[[Física II]]`) resuelva al slug que
/// guarda la BD (`física-ii`).
///
/// La lógica de slugs no se reimplementa aquí: vive en `Note::slugify` y en
/// `knowledge::markdown`, que hace el matching exacto contra este conjunto.
async fn wikilink_set(db: &sqlx::SqlitePool) -> std::collections::HashSet<String> {
    let notas = sqlx::query_as::<_, (String, String)>("SELECT slug, title FROM notes")
        .fetch_all(db)
        .await
        .unwrap_or_default();

    let mut set = std::collections::HashSet::with_capacity(notas.len() * 2);
    for (slug, title) in notas {
        set.insert(slug);
        let canon = Note::slugify(&title);
        if !canon.is_empty() {
            set.insert(canon);
        }
    }
    set
}

/// Índice de la nota (`<nav>` con enlaces a los anchors del cuerpo).
///
/// Devuelve "" cuando la nota tiene menos de dos headings: con 0 o 1
/// secciones el índice no aporta nada y solo añade ruido visual. La
/// generación del HTML (y de los `id`) vive en
/// `crate::knowledge::markdown::toc_html`, que comparte generador y
/// deduplicación con `render_html`: cada `href="#id"` casa con el `id` del
/// `<h2>` correspondiente.
fn note_toc(doc: &crate::knowledge::markdown::ParsedDoc) -> String {
    if doc.headings.len() < 2 {
        return String::new();
    }
    crate::knowledge::markdown::toc_html(doc)
}

/// Bloque visual del índice: un `<aside>` con un `<details open>` plegable
/// (en móvil se puede colapsar para dejar sitio al cuerpo; en escritorio
/// permanece arriba del contenido). Vacío si la nota no tiene índice.
fn render_toc_block(toc: &str) -> String {
    if toc.is_empty() {
        return String::new();
    }
    format!(
        r#"<aside class="note-toc mb-space-md">
            <details open class="group rounded-xl border border-outline-variant bg-surface-container p-space-md">
                <summary class="flex items-center gap-2 cursor-pointer select-none font-label-md text-label-md text-on-surface">
                    <span class="material-symbols-outlined text-[18px] text-primary-container">list</span>
                    <span>Índice de la nota</span>
                    <span class="material-symbols-outlined text-[18px] ml-auto text-on-surface-variant transition-transform group-open:rotate-180">expand_more</span>
                </summary>
                <div class="mt-space-sm">
                    {toc}
                </div>
            </details>
        </aside>"#,
        toc = toc
    )
}

/// Metadatos de la cabecera de una nota. Todo sale de la BD: si no hay dato,
/// se muestra 0 o el valor crudo (nunca un dato inventado).
#[derive(Debug, Clone, Default)]
struct NoteMeta {
    /// `notes.created_at` en crudo.
    created_at: String,
    /// `notes.updated_at` en crudo.
    updated_at: String,
    /// `notes.word_count`.
    word_count: i64,
    /// Nº real de filas en `note_versions` de esta nota.
    version_count: i64,
}

/// Una instantánea del historial de la nota (`note_versions`).
#[derive(Debug, Clone)]
struct NoteVersion {
    id: i64,
    word_count: i64,
    /// `note_versions.created_at` en crudo.
    created_at: String,
}

/// Metadatos de cabecera + nº real de versiones (`COUNT(*)` de `note_versions`).
async fn load_note_meta(db: &sqlx::SqlitePool, note: &Note) -> NoteMeta {
    let version_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM note_versions WHERE note_id = ?")
            .bind(note.id)
            .fetch_one(db)
            .await
            .unwrap_or(0);

    NoteMeta {
        created_at: note.created_at.clone(),
        updated_at: note.updated_at.clone(),
        word_count: note.word_count,
        version_count,
    }
}

/// Últimas instantáneas del historial (la BD conserva 5 por nota, la más
/// reciente primero).
async fn load_note_versions(db: &sqlx::SqlitePool, note_id: i64) -> Vec<NoteVersion> {
    sqlx::query_as::<_, (i64, i64, String)>(
        "SELECT id, word_count, created_at FROM note_versions
         WHERE note_id = ? ORDER BY id DESC LIMIT 5",
    )
    .bind(note_id)
    .fetch_all(db)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|(id, word_count, created_at)| NoteVersion {
                id,
                word_count,
                created_at,
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Guarda una instantánea del cuerpo en `note_versions` y poda el historial a
/// las 5 últimas (igual que hace el servicio de conocimiento). Un fallo aquí
/// no rompe la edición: solo se pierde el historial.
async fn snapshot_version(
    db: &sqlx::SqlitePool,
    note_id: i64,
    content: &str,
    word_count: i64,
    now: &str,
) {
    if let Err(e) = sqlx::query(
        "INSERT INTO note_versions (note_id, content, word_count, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(note_id)
    .bind(content)
    .bind(word_count)
    .bind(now)
    .execute(db)
    .await
    {
        tracing::warn!("notes: no se pudo guardar la versión: {}", e);
        return;
    }
    let _ = sqlx::query(
        r#"DELETE FROM note_versions
           WHERE note_id = ? AND id NOT IN (
               SELECT id FROM note_versions WHERE note_id = ? ORDER BY id DESC LIMIT 5
           )"#,
    )
    .bind(note_id)
    .bind(note_id)
    .execute(db)
    .await;
}

/// Interpreta una marca de tiempo de la BD: RFC3339 (`2026-10-19T18:50:15Z`)
/// o el formato de SQLite (`2026-10-19 18:50:15`). `None` si no encaja.
fn parse_ts(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, fmt) {
            return Some(naive.and_utc());
        }
    }
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc())
}

/// Fecha legible en hora local. Si la marca no se puede interpretar se
/// devuelve el valor crudo: nunca una fecha inventada.
fn format_ts(raw: &str) -> String {
    match parse_ts(raw) {
        Some(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%d/%m/%Y %H:%M")
            .to_string(),
        None => raw.trim().to_string(),
    }
}

/// Atributo `datetime` de `<time>`: ISO-8601 si la marca se puede parsear.
fn ts_attr(raw: &str) -> String {
    match parse_ts(raw) {
        Some(dt) => dt.to_rfc3339(),
        None => raw.trim().to_string(),
    }
}

/// Cabecera de metadatos de la nota: fechas, palabras y nº de versiones.
/// Todos los valores son los de la BD (0 si no hay historial).
fn render_meta_block(meta: &NoteMeta) -> String {
    let palabras = if meta.word_count == 1 {
        "palabra"
    } else {
        "palabras"
    };
    let versiones = if meta.version_count == 1 {
        "versión"
    } else {
        "versiones"
    };

    let items = [
        meta_time_item("calendar_today", "Creada", &meta.created_at),
        meta_time_item("update", "Actualizada", &meta.updated_at),
        meta_item(
            "notes",
            "Palabras",
            &format!("{} {}", meta.word_count, palabras),
        ),
        meta_item(
            "history",
            "Versiones",
            &format!("{} {}", meta.version_count, versiones),
        ),
    ];

    format!(
        r#"<div class="mb-space-md flex flex-wrap items-center gap-x-space-lg gap-y-space-xs rounded-xl border border-outline-variant bg-surface-container px-space-md py-space-sm font-body-sm text-body-sm">
            {items}
        </div>"#,
        items = items.join(""),
    )
}

/// Ítem de metadatos con fecha: el valor original queda en `datetime`.
fn meta_time_item(icon: &str, label: &str, raw: &str) -> String {
    format!(
        r#"<span class="flex items-center gap-1.5">
            <span class="material-symbols-outlined text-[16px] text-outline">{icon}</span>
            <span class="text-outline">{label}</span>
            <time datetime="{dt}" class="text-on-surface font-label-md">{value}</time>
        </span>"#,
        icon = icon,
        label = label,
        dt = html_escape(&ts_attr(raw)),
        value = html_escape(&format_ts(raw)),
    )
}

/// Ítem de metadatos con un valor numérico.
fn meta_item(icon: &str, label: &str, value: &str) -> String {
    format!(
        r#"<span class="flex items-center gap-1.5">
            <span class="material-symbols-outlined text-[16px] text-outline">{icon}</span>
            <span class="text-outline">{label}</span>
            <span class="text-on-surface font-label-md">{value}</span>
        </span>"#,
        icon = icon,
        label = label,
        value = html_escape(value),
    )
}

/// Bloque «Versiones»: las últimas instantáneas guardadas en `note_versions`
/// (la BD conserva 5 por nota). Cada una muestra su fecha real y un botón
/// «Restaurar» que apunta a la ruta existente
/// (`POST /app/notes/{slug}/versions/{id}/restore`).
fn render_versions_block(slug: &str, versions: &[NoteVersion]) -> String {
    if versions.is_empty() {
        return String::new();
    }

    let items: Vec<String> = versions
        .iter()
        .map(|v| {
            format!(
                r#"<li class="flex items-center justify-between gap-2 rounded-lg border border-outline-variant bg-surface-container px-space-sm py-space-xs">
                    <span class="flex items-center gap-1.5 font-body-sm text-body-sm text-on-surface-variant">
                        <span class="material-symbols-outlined text-[16px] text-outline">history</span>
                        <time datetime="{dt}" class="text-on-surface">{when}</time>
                        <span class="text-outline">· {words} palabras</span>
                    </span>
                    <form method="post" action="/app/notes/{slug}/versions/{id}/restore" class="inline">
                        <button type="submit" onclick="return confirm('¿Restaurar esta versión de la nota?')"
                            class="flex items-center gap-1 px-space-sm py-1 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container-high transition-colors font-label-sm">
                            <span class="material-symbols-outlined text-[16px]">restore</span>Restaurar
                        </button>
                    </form>
                </li>"#,
                dt = html_escape(&ts_attr(&v.created_at)),
                when = html_escape(&format_ts(&v.created_at)),
                words = v.word_count,
                slug = html_escape(slug),
                id = v.id,
            )
        })
        .collect();

    format!(
        r#"<section class="mt-space-lg">
            <h2 class="font-headline-sm text-headline-sm text-on-surface mb-space-sm">Versiones</h2>
            <ul class="space-y-space-xs">{items}</ul>
        </section>"#,
        items = items.join("")
    )
}

fn render_note(
    note: &Note,
    md_html: &str,
    toc: &str,
    meta: &NoteMeta,
    versions: &[NoteVersion],
) -> String {
    let toc_block = render_toc_block(toc);
    let meta_block = render_meta_block(meta);
    let versions_block = render_versions_block(&note.slug, versions);
    let content = format!(
        r#"
        <style>
            summary::-webkit-details-marker {{ display: none; }}
            summary {{ list-style: none; }}
        </style>
        <div class="flex items-start justify-between gap-2 mb-space-sm">
            <h1 class="font-headline-md text-headline-md text-primary-container">{title}</h1>
            <div class="flex items-center gap-2">
                <a href="/app/notes/{slug}/edit" class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                    <span class="material-symbols-outlined text-[16px]">edit</span>Editar
                </a>
                <a href="/app/notes/{slug}/export" download="{filename}" class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                    <span class="material-symbols-outlined text-[16px]">download</span>Exportar Markdown
                </a>
                <details class="relative">
                    <summary class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg bg-error-container/10 text-error hover:bg-error-container/20 transition-colors cursor-pointer select-none">
                        <span class="material-symbols-outlined text-[16px]">delete</span>Borrar
                    </summary>
                    <div class="absolute right-0 top-full mt-1 z-20 w-72 p-space-md rounded-xl border border-outline-variant bg-surface-container-high shadow-lg">
                        <p class="font-body-sm text-body-sm text-on-surface mb-space-sm">¿Eliminar «{title}»? La acción no se puede deshacer.</p>
                        <div class="flex items-center justify-end gap-2">
                            <button type="button" onclick="this.closest('details').removeAttribute('open')"
                                class="px-space-sm py-1.5 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors font-label-md">Cancelar</button>
                            <form hx-delete="/app/notes/{slug}/delete" hx-swap="none" class="inline">
                                <button type="submit" class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg bg-error-container text-on-error-container font-label-md font-semibold hover:opacity-90 transition-opacity">
                                    <span class="material-symbols-outlined text-[16px]">delete</span>Sí, eliminar
                                </button>
                            </form>
                        </div>
                    </div>
                </details>
            </div>
        </div>
        {meta_block}
        {toc_block}
        <div class="note-render bg-surface-container rounded-xl border border-outline-variant p-space-lg shadow-sm font-body-md text-body-md text-on-surface">
            {md_html}
        </div>
        {versions_block}
        "#,
        title = html_escape(&note.title),
        slug = html_escape(&note.slug),
        filename = html_escape(&format!("{}.md", note.slug)),
        meta_block = meta_block,
        toc_block = toc_block,
        md_html = md_html,
        versions_block = versions_block,
    );

    crate::academic::templates::page_ext(
        &format!("{} — Study Platform", note.title),
        "Mis notas",
        "/dashboard/notes",
        &content,
    )
}

fn render_note_editor(note: &Note, content: &str, sin_archivo: bool) -> String {
    // Aviso honesto: la nota existe en la BD pero su `.md` no se pudo leer, así
    // que el editor arranca vacío (no se inventa contenido).
    let aviso = if sin_archivo {
        r#"<div class="flex items-start gap-2 mb-space-md p-space-sm rounded-lg border border-error/40 bg-error-container/10 text-on-surface">
            <span class="material-symbols-outlined text-[18px] text-error">warning</span>
            <p class="font-body-sm text-body-sm">
                Contenido no disponible en disco: no se pudo leer el archivo Markdown de esta nota.
                El editor está vacío; si guardas, se creará el archivo.
            </p>
        </div>"#
    } else {
        ""
    };

    let content = format!(
        r#"
        <div class="flex items-center gap-2 mb-space-md">
            <a href="/app/notes" class="flex items-center gap-1.5 text-outline hover:text-on-surface transition-colors">
                <span class="material-symbols-outlined text-[18px]">arrow_back</span>
                Volver a Mis notas
            </a>
            <h1 class="font-headline-md text-headline-md text-primary-container">Editar nota</h1>
        </div>
        {aviso}
        <form hx-put="/app/notes/{slug}/update" hx-swap="outerHTML" class="space-y-space-md">
            <div>
                <label class="block font-label-sm text-label-sm text-on-surface-variant mb-space-xs">Título</label>
                <input type="text" name="title" value="{title}"
                    class="w-full px-space-sm py-2 bg-surface-container border border-outline-variant rounded text-on-surface focus:outline-none focus:ring-1 focus:ring-primary-container">
            </div>
            <div>
                <label class="block font-label-sm text-label-sm text-on-surface-variant mb-space-xs">Contenido (Markdown)</label>
                <textarea name="content" rows="20"
                    class="w-full px-space-sm py-2 bg-surface-container border border-outline-variant rounded text-on-surface focus:outline-none focus:ring-1 focus:ring-primary-container font-code-inline text-code-inline">{content}</textarea>
            </div>
            <div>
                <label class="block font-label-sm text-label-sm text-on-surface-variant mb-space-xs">Tags</label>
                <input type="text" name="tags" value="{tags}" placeholder="ej: calculo, examen"
                    class="w-full px-space-sm py-2 bg-surface-container border border-outline-variant rounded text-on-surface focus:outline-none focus:ring-1 focus:ring-primary-container">
            </div>
            <button type="submit" class="flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-secondary text-on-secondary font-label-md font-semibold hover:bg-secondary-fixed transition-all shadow-md">
                <span class="material-symbols-outlined text-[16px]">save</span>
                <span>Guardar</span>
            </button>
        </form>
        "#,
        aviso = aviso,
        title = html_escape(&note.title),
        slug = html_escape(&note.slug),
        content = html_escape(content),
        tags = html_escape(&note.tags),
    );

    crate::academic::templates::page_ext(
        &format!("Editar: {} — Study Platform", note.title),
        "Mis notas",
        "/dashboard/notes",
        &content,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nota_de_prueba() -> Note {
        Note {
            id: 1,
            title: "Nota de prueba".to_string(),
            slug: "nota-de-prueba".to_string(),
            filename: "nota-de-prueba.md".to_string(),
            tags: String::new(),
            word_count: 6,
            summary: None,
            created_at: "2026-10-19 00:00:00".to_string(),
            updated_at: "2026-10-19 00:00:00".to_string(),
        }
    }

    /// Extrae, en orden, los valores de `id="…"` de un HTML.
    fn ids(html: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = html;
        while let Some(start) = rest.find("id=\"") {
            rest = &rest[start + 4..];
            let Some(end) = rest.find('"') else { break };
            out.push(rest[..end].to_string());
            rest = &rest[end..];
        }
        out
    }

    /// El conjunto que alimenta los wikilinks sale de la BD: un enlace a una
    /// nota existente apunta a su URL real (`/app/notes/{slug}`) y el que no
    /// tiene nota detrás sale como texto plano (nunca un enlace roto).
    #[tokio::test]
    async fn wikilinks_se_resuelven_contra_las_notas_de_la_bd() {
        // BD con las migraciones reales, en un fichero temporal.
        let db_file = tempfile::NamedTempFile::new().expect("fichero de BD temporal");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite://{}", db_file.path().display()))
            .await
            .expect("conectar a la BD de test");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("ejecutar migraciones");

        // Una nota real: su slug es el que forma la URL `/app/notes/{slug}`.
        sqlx::query(
            "INSERT INTO notes (title, slug, filename, tags, word_count, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("Física II")
        .bind("física-ii")
        .bind("física-ii.md")
        .bind("")
        .bind(0i64)
        .bind("2026-10-19 00:00:00")
        .bind("2026-10-19 00:00:00")
        .execute(&pool)
        .await
        .expect("sembrar nota");

        let notas = wikilink_set(&pool).await;
        let doc = crate::knowledge::markdown::parse("ver [[Física II]] y [[Fantasma]]");
        let html = crate::knowledge::markdown::render_html(&doc, &notas);

        // Nota existente: enlace a su URL real (nunca a un ancla `#slug`).
        assert!(
            html.contains(r#"href="/app/notes/física-ii""#),
            "el wikilink debe apuntar a la nota real: {html}"
        );
        assert!(
            html.contains(">Física II</a>"),
            "el texto visible se conserva"
        );

        // Nota inexistente: texto plano con la clase que lo delata, sin href.
        assert!(
            html.contains(r#"<span class="text-outline">Fantasma</span>"#),
            "un wikilink sin nota debe quedar como texto plano: {html}"
        );
        assert!(!html.contains("Fantasma</a>"), "no debe inventar un enlace");
    }

    #[test]
    fn indice_sobre_el_cuerpo_y_cada_href_apunta_a_un_id_real() {
        let src = "# Teoría\n\ntexto\n\n## Ondas\n\ntexto\n";
        let doc = crate::knowledge::markdown::parse(src);
        let md_html =
            crate::knowledge::markdown::render_html(&doc, &std::collections::HashSet::new());
        let html = render_note(
            &nota_de_prueba(),
            &md_html,
            &note_toc(&doc),
            &NoteMeta::default(),
            &[],
        );

        // El cuerpo renderizado sigue presente.
        assert!(html.contains("note-render"));

        // El índice se renderiza por encima del cuerpo, plegable (`<details open>`).
        let pos_toc = html.find("Índice de la nota").expect("falta el índice");
        let pos_cuerpo = html.find("note-render").expect("falta el cuerpo");
        assert!(pos_toc < pos_cuerpo, "el índice debe ir antes del cuerpo");
        assert!(html.contains(r#"<nav aria-label="Índice de la nota""#));
        assert!(html.contains("<details open"));

        // Cada `href="#id"` del índice casa con un `id` emitido por el render.
        for id in ids(&md_html) {
            assert!(
                html.contains(&format!(r##"href="#{id}""##)),
                "falta el enlace a #{id}"
            );
        }

        // Con un único heading no hay índice.
        let una = crate::knowledge::markdown::parse("# Solo\n\ntexto\n");
        assert_eq!(note_toc(&una), "");
        let html_una = render_note(
            &nota_de_prueba(),
            "",
            &note_toc(&una),
            &NoteMeta::default(),
            &[],
        );
        assert!(html_una.contains("note-render"));
        assert!(!html_una.contains("Índice de la nota"));

        // El `<details>` de borrar sigue intacto.
        assert!(html.contains(r#"hx-delete="/app/notes/nota-de-prueba/delete""#));
    }

    /// Un slug con separadores o `..` nunca es válido: la ruta que se
    /// construye con él no puede salirse de `notes_dir`.
    #[test]
    fn slug_peligroso_se_rechaza() {
        for malo in ["", ".", "..", "a/b", "a\\b", "../etc/passwd", "a..b"] {
            assert!(!is_safe_slug(malo), "no debe ser válido: {malo:?}");
        }
        assert!(is_safe_slug("nota-de-prueba"));
        assert!(is_safe_slug("física-ii"));
    }

    /// La página 404 usa el mismo shell que el resto de la app (no un texto
    /// plano), escapa el identificador y ofrece una salida al listado.
    #[test]
    fn pagina_404_usa_el_shell_y_escapa_el_identificador() {
        let html = nota_ausente_page("no-existe", NotaAusente::SinRegistro);
        assert!(
            html.contains("<!DOCTYPE html>"),
            "debe usar el shell completo"
        );
        assert!(html.contains("Volver a las notas"));
        assert!(
            html.to_lowercase().contains("not found"),
            "debe explicar honestamente que la nota no existe"
        );
        assert!(html.contains("no-existe"));

        let xss = nota_ausente_page(
            "<script>alert(1)</script>",
            NotaAusente::IdentificadorInvalido,
        );
        assert!(
            !xss.contains("<script>alert"),
            "el identificador debe ir escapado: {xss}"
        );
        assert!(xss.contains("&lt;script&gt;"));
    }

    /// Sin archivo en disco la nota se muestra con un estado vacío honesto.
    #[test]
    fn estado_vacio_honesto_cuando_falta_el_archivo() {
        let html = cuerpo_ausente("mi-nota");
        assert!(html.contains("Contenido no disponible en disco"));
        assert!(html.contains("mi-nota.md"));
    }

    /// La cabecera muestra los metadatos que hay en la BD: ni una fecha ni un
    /// recuento inventados.
    #[test]
    fn cabecera_muestra_fechas_palabras_y_versiones_reales() {
        let meta = NoteMeta {
            created_at: "2026-10-19 08:30:00".to_string(),
            updated_at: "2026-10-19 18:45:00".to_string(),
            word_count: 123,
            version_count: 3,
        };
        let html = render_note(&nota_de_prueba(), "", "", &meta, &[]);

        assert!(
            html.contains("123 palabras"),
            "el recuento de palabras es real"
        );
        assert!(html.contains("3 versiones"), "el nº de versiones es real");
        // La fecha original queda en `datetime` (ISO-8601, sin zona inventada).
        assert!(html.contains(r#"datetime="2026-10-19T08:30:00+00:00""#));
        assert!(html.contains(r#"datetime="2026-10-19T18:45:00+00:00""#));
        // Una nota sin historial lo dice: 0 versiones, sin inventar entradas.
        let vacia = render_note(&nota_de_prueba(), "", "", &NoteMeta::default(), &[]);
        assert!(vacia.contains("0 versiones"));
    }

    /// El bloque de versiones lista las instantáneas reales y ofrece
    /// restaurar solo contra una ruta que existe.
    #[test]
    fn bloque_de_versiones_con_fecha_y_boton_de_restaurar() {
        let versions = vec![
            NoteVersion {
                id: 7,
                word_count: 10,
                created_at: "2026-10-19 10:00:00".to_string(),
            },
            NoteVersion {
                id: 6,
                word_count: 8,
                created_at: "2026-10-18 10:00:00".to_string(),
            },
        ];
        let html = render_note(&nota_de_prueba(), "", "", &NoteMeta::default(), &versions);

        assert!(html.contains("Versiones"));
        assert!(html.contains(r#"datetime="2026-10-19T10:00:00+00:00""#));
        assert!(html.contains(r#"datetime="2026-10-18T10:00:00+00:00""#));
        assert!(html.contains(r#"action="/app/notes/nota-de-prueba/versions/7/restore""#));
        assert!(html.contains(r#"action="/app/notes/nota-de-prueba/versions/6/restore""#));
        assert!(html.contains("10 palabras"));

        // Sin historial no hay bloque ni botones (nunca enlaces muertos).
        let sin = render_note(&nota_de_prueba(), "", "", &NoteMeta::default(), &[]);
        assert!(!sin.contains("Restaurar"));
        assert!(!sin.contains("Versiones</h2>"));
    }

    /// Config de pruebas: rutas fijas, sin secretos.
    fn config_de_prueba(
        notes_dir: std::path::PathBuf,
        db_path: std::path::PathBuf,
    ) -> crate::Config {
        crate::Config {
            host: "127.0.0.1".to_string(),
            port: 18090,
            session_secret: "test_secret_key_at_least_32_chars_long!!".to_string(),
            db_path,
            notes_dir,
            openrouter_api_key: None,
            openrouter_model: "test/model".to_string(),
            openrouter_max_tokens: 1024,
            openrouter_max_cost_usd: 1.0,
            mikareno_mcp_url: "http://127.0.0.1:8080/mcp".to_string(),
            genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
            custom_mcp_api_key: None,
            registration_token: None,
            rust_log: "test".to_string(),
            auth_disabled: true,
            db_max_connections: 4,
            mcp_timeout_ms: 5_000,
        }
    }

    /// Estado de pruebas: BD temporal con las migraciones reales y un
    /// directorio de notas temporal.
    async fn estado_de_prueba() -> (
        std::sync::Arc<crate::AppState>,
        tempfile::TempDir,
        tempfile::TempDir,
    ) {
        let db_dir = tempfile::tempdir().expect("directorio de la BD");
        let db_path = db_dir.path().join("test.db");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&db_path)
                    .create_if_missing(true),
            )
            .await
            .expect("conectar a la BD de test");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("ejecutar migraciones");

        let notes = tempfile::tempdir().expect("directorio de notas");
        let config = config_de_prueba(notes.path().to_path_buf(), db_path);
        let state = std::sync::Arc::new(crate::AppState {
            cache_mikareno: std::sync::Arc::new(
                crate::academic::calendar_data::CacheMikareno::default(),
            ),
            cache_catalogo: std::sync::Arc::new(crate::ai::modelos::CacheCatalogo::default()),
            config,
            db: pool,
            mikareno: None,
            start_time: std::time::Instant::now(),
        });
        (state, db_dir, notes)
    }

    /// Sembrar una nota de prueba (fila en `notes` + fichero en disco).
    async fn sembrar_nota(state: &crate::AppState, notes: &std::path::Path) -> i64 {
        std::fs::write(notes.join("nota-de-prueba.md"), "# Hola\n\nmundo\n")
            .expect("escribir el .md");
        sqlx::query(
            "INSERT INTO notes (title, slug, filename, tags, word_count, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("Nota de prueba")
        .bind("nota-de-prueba")
        .bind("nota-de-prueba.md")
        .bind("")
        .bind(2i64)
        .bind("2026-10-19 00:00:00")
        .bind("2026-10-19 00:00:00")
        .execute(&state.db)
        .await
        .expect("sembrar la nota");
        sqlx::query_scalar::<_, i64>("SELECT id FROM notes WHERE slug = ?")
            .bind("nota-de-prueba")
            .fetch_one(&state.db)
            .await
            .expect("id de la nota")
    }

    /// La exportación devuelve el `.md` con `Content-Disposition: attachment`
    /// y el nombre correcto: el navegador lo descarga, no lo muestra.
    #[tokio::test]
    async fn export_devuelve_el_markdown_para_descargar() {
        use tower::ServiceExt;

        let (state, _db, notes) = estado_de_prueba().await;
        sembrar_nota(&state, notes.path()).await;

        let app = axum::Router::new()
            .route("/app/notes/{slug}/export", axum::routing::get(export))
            .with_state(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/app/notes/nota-de-prueba/export")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("respuesta del export");

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/markdown; charset=utf-8"
        );
        assert_eq!(
            response
                .headers()
                .get(CONTENT_DISPOSITION)
                .unwrap()
                .to_str()
                .unwrap(),
            "attachment; filename=\"nota-de-prueba.md\""
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(body.to_vec()).unwrap(),
            "# Hola\n\nmundo\n",
            "el cuerpo descargado es el .md de disco"
        );
    }

    /// Una nota que no existe no se exporta: 404 con la página estilizada de
    /// la app, no un texto plano ("Note not found").
    #[tokio::test]
    async fn export_de_una_nota_inexistente_devuelve_404() {
        use tower::ServiceExt;

        let (state, _db, _notes) = estado_de_prueba().await;
        let app = axum::Router::new()
            .route("/app/notes/{slug}/export", axum::routing::get(export))
            .with_state(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/app/notes/no-existe/export")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("respuesta del export");

        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("<!DOCTYPE html>"),
            "el 404 debe usar el shell de la app: {html}"
        );
        assert!(
            html.contains("Volver a las notas"),
            "el 404 debe ofrecer una salida al listado"
        );
        assert!(
            !html.contains("Note not found"),
            "el 404 no puede ser un texto plano: {html}"
        );
    }

    /// El `Content-Disposition` lleva el nombre real del archivo (el
    /// `filename` de la BD), no un nombre re-derivado del slug.
    #[tokio::test]
    async fn export_lleva_el_nombre_de_archivo_de_la_bd() {
        use tower::ServiceExt;

        let (state, _db, notes) = estado_de_prueba().await;
        // La fila dice `mis-notas.md` aunque el slug sea `nota-real`: la
        // descarga se llama como el archivo, no como el slug.
        std::fs::write(notes.path().join("nota-real.md"), "# Hola\n").expect("escribir el .md");
        sqlx::query(
            "INSERT INTO notes (title, slug, filename, tags, word_count, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("Nota real")
        .bind("nota-real")
        .bind("mis-notas.md")
        .bind("")
        .bind(2i64)
        .bind("2026-10-19 00:00:00")
        .bind("2026-10-19 00:00:00")
        .execute(&state.db)
        .await
        .expect("sembrar la nota");

        let app = axum::Router::new()
            .route("/app/notes/{slug}/export", axum::routing::get(export))
            .with_state(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/app/notes/nota-real/export")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("respuesta del export");

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/markdown; charset=utf-8"
        );
        let disposition = response
            .headers()
            .get(CONTENT_DISPOSITION)
            .expect("cabecera Content-Disposition")
            .to_str()
            .expect("cabecera válida");
        assert_eq!(disposition, r#"attachment; filename="mis-notas.md""#);
        assert!(
            !disposition.contains("nota-real"),
            "el nombre no se re-deriva del slug: {disposition}"
        );
    }

    /// Un nombre de archivo con acentos sigue dando una cabecera válida: el
    /// nombre real viaja en `filename*` (RFC 5987) y `filename` lleva una
    /// variante ASCII. Sin esto la cabecera no se aceptaba y el navegador
    /// mostraba el Markdown en vez de descargarlo.
    #[tokio::test]
    async fn export_con_nombre_con_acentos_manda_una_cabecera_valida() {
        use tower::ServiceExt;

        let (state, _db, notes) = estado_de_prueba().await;
        std::fs::write(notes.path().join("física-ii.md"), "# Hola\n").expect("escribir el .md");
        sqlx::query(
            "INSERT INTO notes (title, slug, filename, tags, word_count, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("Física II")
        .bind("física-ii")
        .bind("física-ii.md")
        .bind("")
        .bind(2i64)
        .bind("2026-10-19 00:00:00")
        .bind("2026-10-19 00:00:00")
        .execute(&state.db)
        .await
        .expect("sembrar la nota");

        let app = axum::Router::new()
            .route("/app/notes/{slug}/export", axum::routing::get(export))
            .with_state(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/app/notes/f%C3%ADsica-ii/export")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("respuesta del export");

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let disposition = response
            .headers()
            .get(CONTENT_DISPOSITION)
            .expect("cabecera Content-Disposition")
            .to_str()
            .expect("la cabecera debe ser ASCII válido");
        assert!(
            disposition.contains("filename*=UTF-8''f%C3%ADsica-ii.md"),
            "el nombre real debe ir codificado en filename*: {disposition}"
        );
        assert!(
            disposition.contains(r#"filename="f_sica-ii.md""#),
            "filename debe llevar una variante ASCII: {disposition}"
        );
    }

    /// El botón «Restaurar» es un formulario POST a la ruta de restore.
    #[test]
    fn el_boton_restaurar_hace_post_a_la_ruta_de_restore() {
        let versiones = vec![NoteVersion {
            id: 7,
            word_count: 3,
            created_at: "2026-10-19 00:00:00".to_string(),
        }];
        let html = render_versions_block("mi-nota", &versiones);

        assert!(
            html.contains(r#"method="post""#),
            "el botón debe ser un POST: {html}"
        );
        assert!(
            html.contains(r#"action="/app/notes/mi-nota/versions/7/restore""#),
            "el formulario debe apuntar a la ruta de restore: {html}"
        );
    }

    /// Restaurar la misma versión tres veces no apila copias idénticas del
    /// mismo texto: el historial crece como mucho con una fila.
    #[tokio::test]
    async fn restaurar_la_misma_version_no_duplica_el_historial() {
        use tower::ServiceExt;

        let (state, _db, notes) = estado_de_prueba().await;
        let note_id = sembrar_nota(&state, notes.path()).await;
        for (contenido, fecha) in [
            ("cuerpo viejo", "2026-10-18 00:00:00"),
            ("cuerpo nuevo", "2026-10-19 00:00:00"),
        ] {
            sqlx::query(
                "INSERT INTO note_versions (note_id, content, word_count, created_at)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(note_id)
            .bind(contenido)
            .bind(2i64)
            .bind(fecha)
            .execute(&state.db)
            .await
            .expect("sembrar la versión");
        }
        // Se restaura la versión antigua, que no es la última del historial.
        let version_id: i64 = sqlx::query_scalar(
            "SELECT id FROM note_versions WHERE note_id = ? ORDER BY id ASC LIMIT 1",
        )
        .bind(note_id)
        .fetch_one(&state.db)
        .await
        .expect("id de la versión");

        for _ in 0..3 {
            let app = axum::Router::new()
                .route(
                    "/app/notes/{slug}/versions/{version_id}/restore",
                    axum::routing::post(restore_version),
                )
                .with_state(state.clone());

            let response = app
                .oneshot(
                    axum::http::Request::builder()
                        .method(axum::http::Method::POST)
                        .uri(format!(
                            "/app/notes/nota-de-prueba/versions/{version_id}/restore"
                        ))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .expect("respuesta del restore");

            // Tras restaurar se vuelve a la nota (303).
            assert_eq!(response.status(), axum::http::StatusCode::SEE_OTHER);
            assert_eq!(
                response
                    .headers()
                    .get(axum::http::header::LOCATION)
                    .unwrap()
                    .to_str()
                    .unwrap(),
                "/app/notes/nota-de-prueba"
            );
        }

        let versiones: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM note_versions WHERE note_id = ?")
                .bind(note_id)
                .fetch_one(&state.db)
                .await
                .expect("contar versiones");
        assert_eq!(
            versiones, 3,
            "3 restauraciones de la misma versión = 2 versiones + 1 instantánea, no 5"
        );
    }

    /// Restaurar una versión devuelve el cuerpo de esa versión al disco y
    /// vuelve a la vista de la nota (303).
    #[tokio::test]
    async fn restore_devuelve_el_cuerpo_de_la_version() {
        use tower::ServiceExt;

        let (state, _db, notes) = estado_de_prueba().await;
        let note_id = sembrar_nota(&state, notes.path()).await;
        let version_id: i64 = sqlx::query_scalar(
            "INSERT INTO note_versions (note_id, content, word_count, created_at)
             VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(note_id)
        .bind("cuerpo viejo")
        .bind(2i64)
        .bind("2026-10-18 00:00:00")
        .fetch_one(&state.db)
        .await
        .expect("sembrar la versión");

        let app = axum::Router::new()
            .route(
                "/app/notes/{slug}/versions/{version_id}/restore",
                axum::routing::post(restore_version),
            )
            .with_state(state.clone());

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method(axum::http::Method::POST)
                    .uri(format!(
                        "/app/notes/nota-de-prueba/versions/{version_id}/restore"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("respuesta del restore");

        assert_eq!(response.status(), axum::http::StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            "/app/notes/nota-de-prueba"
        );

        // El cuerpo en disco es el de la versión restaurada…
        let contenido =
            std::fs::read_to_string(notes.path().join("nota-de-prueba.md")).expect("leer el .md");
        assert_eq!(contenido, "cuerpo viejo");
        // …y la nota queda con el recuento de palabras de esa versión.
        let word_count: i64 = sqlx::query_scalar("SELECT word_count FROM notes WHERE slug = ?")
            .bind("nota-de-prueba")
            .fetch_one(&state.db)
            .await
            .expect("word_count de la nota");
        assert_eq!(word_count, 2);
    }

    /// Una versión que no existe (o que es de otra nota) no se restaura.
    #[tokio::test]
    async fn restore_de_una_version_inexistente_devuelve_404() {
        use tower::ServiceExt;

        let (state, _db, notes) = estado_de_prueba().await;
        sembrar_nota(&state, notes.path()).await;

        let app = axum::Router::new()
            .route(
                "/app/notes/{slug}/versions/{version_id}/restore",
                axum::routing::post(restore_version),
            )
            .with_state(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method(axum::http::Method::POST)
                    .uri("/app/notes/nota-de-prueba/versions/9999/restore")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("respuesta del restore");

        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    /// La edición guarda una instantánea: el historial existe de verdad (lo
    /// que la vista «Versiones» muestra).
    #[tokio::test]
    async fn editar_una_nota_guarda_una_version_en_el_historial() {
        let (state, _db, notes) = estado_de_prueba().await;
        let note_id = sembrar_nota(&state, notes.path()).await;

        let form = UpdateNoteRequest {
            title: None,
            content: Some("cuerpo nuevo".to_string()),
            tags: None,
        };
        update(
            axum::extract::State(state.clone()),
            Path("nota-de-prueba".to_string()),
            Form(form),
        )
        .await;

        let versiones: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM note_versions WHERE note_id = ?")
                .bind(note_id)
                .fetch_one(&state.db)
                .await
                .expect("contar versiones");
        assert!(versiones >= 1, "la edición deja historial: {versiones}");
    }
}
