//! Notes handlers — CRUD for Markdown second-brain notes.
//!
//! Notes are stored as:
//! - Metadata (title, slug, tags, word_count, timestamps) in SQLite
//! - Content as Markdown file on disk (Obsidian-compatible)
//! - FTS5 index for full-text search

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Form, Json,
};
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

fn bad_slug_html() -> (StatusCode, axum::response::Html<String>) {
    (
        StatusCode::BAD_REQUEST,
        axum::response::Html("Invalid slug".to_string()),
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

/// List all notes (HTML page).
pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let notes = sqlx::query_as::<_, Note>("SELECT * FROM notes ORDER BY updated_at DESC")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let notes_html: String = notes
        .iter()
        .map(|n| {
            format!(
                r#"<li class="mb-2">
                    <a href="/app/notes/{}" class="text-primary-container hover:text-primary-fixed-dim">{}</a>
                    <span class="text-xs text-outline ml-2">{}</span>
                </li>"#,
                n.slug,
                html_escape(&n.title),
                format_tags(&n.tags)
            )
        })
        .collect::<Vec<_>>()
        .join("");

    let notes_html: String = notes
        .iter()
        .map(|n| {
            format!(
                r#"<li class="mb-2">
                    <a href="/app/notes/{}" class="text-primary-container hover:text-primary-fixed-dim">{}</a>
                    <span class="text-xs text-outline ml-2">{}</span>
                </li>"#,
                n.slug,
                html_escape(&n.title),
                format_tags(&n.tags)
            )
        })
        .collect::<Vec<_>>()
        .join("");

    let empty_msg = if notes.is_empty() {
        r#"<p class="font-body-sm text-body-sm text-on-surface-variant">No tienes notas aún. ¡Crea una!</p>"#
    } else {
        ""
    };

    let content = format!(
        r#"
        <div class="flex justify-between items-center mb-space-md">
            <h2 class="font-headline-sm text-headline-sm text-on-surface">{count} notas</h2>
            <a href="/app/notes/new" class="flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-secondary text-on-secondary font-label-md font-semibold hover:bg-secondary-fixed transition-all">
                <span class="material-symbols-outlined text-[16px]">note_add</span>
                <span>+ Nueva nota</span>
            </a>
        </div>
        <ul class="space-y-space-xs">
            {notes_html}
        </ul>
        {empty_msg}
        "#,
        count = notes.len(),
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
    if !is_safe_slug(&slug) {
        return bad_slug_html();
    }
    let note = sqlx::query_as::<_, Note>("SELECT * FROM notes WHERE slug = ?")
        .bind(&slug)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

    match note {
        Some(n) => {
            let content_path = state.config.notes_dir.join(format!("{}.md", n.slug));
            let content = std::fs::read_to_string(&content_path).unwrap_or_default();

            let html = render_note(&n, &content);
            (StatusCode::OK, axum::response::Html(html))
        }
        None => (
            StatusCode::NOT_FOUND,
            axum::response::Html("Note not found".to_string()),
        ),
    }
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
            let content_path = state.config.notes_dir.join(format!("{}.md", n.slug));
            let content = std::fs::read_to_string(&content_path).unwrap_or_default();

            (
                StatusCode::OK,
                Json(json!({ "note": n, "content": content })),
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
        return bad_slug_html();
    }
    let note = sqlx::query_as::<_, Note>("SELECT * FROM notes WHERE slug = ?")
        .bind(&slug)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

    match note {
        Some(n) => {
            let content_path = state.config.notes_dir.join(format!("{}.md", n.slug));
            let content = std::fs::read_to_string(&content_path).unwrap_or_default();
            (
                StatusCode::OK,
                axum::response::Html(render_note_editor(&n, &content)),
            )
        }
        None => (
            StatusCode::NOT_FOUND,
            axum::response::Html("Note not found".to_string()),
        ),
    }
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
pub async fn create(
    State(state): State<Arc<AppState>>,
    form: Form<CreateNoteRequest>,
) -> impl IntoResponse {
    // Ensure notes directory exists
    let _ = std::fs::create_dir_all(&state.config.notes_dir);

    if form.title.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Title is required" })),
        );
    }
    if let Some(err) = check_note_fields(
        Some(&form.title),
        form.content.as_deref(),
        form.tags.as_deref(),
    ) {
        return err;
    }

    let slug = Note::slugify(&form.title);
    if slug.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid title" })),
        );
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
        );
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
            );
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
        Ok(_) => {
            // FTS index is updated by notes_ai trigger
            (
                StatusCode::CREATED,
                Json(json!({ "slug": slug, "message": "Note created" })),
            )
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
    }

    (StatusCode::OK, Json(json!({ "message": "Note updated" })))
}

/// Delete a note.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !is_safe_slug(&slug) {
        return bad_slug_json();
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
            (StatusCode::OK, Json(json!({ "message": "Note deleted" })))
        }
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Note not found" })),
        ),
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

fn format_tags(tags: &str) -> String {
    if tags.is_empty() {
        String::new()
    } else {
        format!("[{}]", tags)
    }
}

fn render_note(note: &Note, content: &str) -> String {
    // Simple Markdown-to-HTML (basic: headers only)
    let html = markdown_to_html(content);

    let content = format!(
        r#"
        <div class="flex items-center justify-between mb-space-md">
            <h1 class="font-headline-md text-headline-md text-primary-container">{title}</h1>
            <div class="flex items-center gap-2">
                <a href="/app/notes/{slug}/edit" class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                    <span class="material-symbols-outlined text-[16px]">edit</span>Editar
                </a>
                <form hx-delete="/app/notes/{slug}/delete" class="inline" hx-confirm="¿Estás seguro?">
                    <button type="submit" class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg bg-error-container/10 text-error hover:bg-error-container/20 transition-colors">
                        <span class="material-symbols-outlined text-[16px]">delete</span>Borrar
                    </button>
                </form>
            </div>
        </div>
        <div class="bg-surface-container rounded-xl border border-outline-variant p-space-lg shadow-sm">
            {content_html}
        </div>
        "#,
        title = html_escape(&note.title),
        slug = note.slug,
        content_html = html,
    );

    crate::academic::templates::page_ext(
        &format!("{} — Study Platform", note.title),
        "Mis notas",
        "/dashboard/notes",
        &content,
    )
}

fn render_note_editor(note: &Note, content: &str) -> String {
    let content = format!(
        r#"
        <div class="flex items-center gap-2 mb-space-md">
            <a href="/app/notes" class="flex items-center gap-1.5 text-outline hover:text-on-surface transition-colors">
                <span class="material-symbols-outlined text-[18px]">arrow_back</span>
                Volver a Mis notas
            </a>
            <h1 class="font-headline-md text-headline-md text-primary-container">Editar nota</h1>
        </div>
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
        title = html_escape(&note.title),
        slug = note.slug,
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

/// Very basic Markdown to HTML converter (for display only).
/// This is NOT a full Markdown parser — just enough for rendering notes.
fn markdown_to_html(input: &str) -> String {
    // Simple replacement without regex crate
    let mut result = String::new();
    for line in input.lines() {
        if line.starts_with("### ") {
            result.push_str(&format!("<h3>{}</h3>\n", html_escape(&line[4..])));
        } else if line.starts_with("## ") {
            result.push_str(&format!("<h2>{}</h2>\n", html_escape(&line[3..])));
        } else if line.starts_with("# ") {
            result.push_str(&format!("<h1>{}</h1>\n", html_escape(&line[2..])));
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    result
}
