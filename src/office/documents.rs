//! Rich Markdown document editor — server-rendered with HTMX + pulldown-cmark.
//!
//! Each document is a single `data/office/docs/{slug}.md` file. The toolbar
//! injects Markdown syntax into the `<textarea>`; preview and export render
//! that Markdown to HTML with `pulldown-cmark`.

use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
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
    let docs = templates::count_files(&docs_dir(&state), "md");
    let sheets = templates::count_files(&crate::office::sheets_dir(&state), "csv");
    let pres = templates::count_files(&crate::office::pres_dir(&state), "json");

    let genoffice_ms = templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    // Inicio unificado: los archivos reales de los tres directorios, no tarjetas
    // ficticias ni una dependencia del índice opcional de SQLite.
    let mut recent: Vec<(String, String, String, &str, &str, String)> = Vec::new();
    for (kind, dir, ext, icon) in [
        ("docs", docs_dir(&state), "md", "description"),
        (
            "sheets",
            crate::office::sheets_dir(&state),
            "csv",
            "table_chart",
        ),
        (
            "presentations",
            crate::office::pres_dir(&state),
            "json",
            "present_to_all",
        ),
    ] {
        for file in templates::list_files(&dir, ext) {
            let href = format!("/office/{kind}/{}", file.name);
            let meta = format!("{} · {}", file.modified, templates::human_size(file.size));
            recent.push((file.modified, file.display, href, icon, kind, meta));
        }
    }
    recent.sort_by(|a, b| b.0.cmp(&a.0));
    let recent_html = if recent.is_empty() {
        r##"<div class="gen-office-empty"><span class="material-symbols-outlined">folder_open</span><h3>Tu espacio está listo.</h3><p>Crea tu primer documento, hoja o presentación.</p></div>"##.to_string()
    } else {
        recent
            .iter()
            .take(6)
            .map(|(_, display, href, icon, kind, meta)| {
                format!(
                    r##"<article class="gen-office-file-card {kind}"><a class="gen-office-file-preview" href="{href}" aria-label="Abrir {display}"><span class="material-symbols-outlined">{icon}</span><small>{kind}</small></a><div class="gen-office-file-copy"><a href="{href}">{display}</a><span>{meta}</span></div><a class="gen-office-file-arrow" href="{href}" aria-label="Abrir {display}"><span class="material-symbols-outlined">arrow_outward</span></a></article>"##,
                    kind = kind,
                    href = templates::html_escape(href),
                    display = templates::html_escape(display),
                    icon = icon,
                    meta = templates::html_escape(meta),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-home">
        <nav class="gen-office-tabs" aria-label="Pestañas de Oficina">
            <a class="gen-office-brand-tab is-active" href="/office"><span class="gen-office-brand-mark"><span class="material-symbols-outlined">grid_view</span></span><strong>NOVA <span>Office</span></strong></a>
            <a class="gen-office-file-tab" href="/office/docs"><span class="gen-office-file-icon docs"><span class="material-symbols-outlined">description</span></span><span>Documentos</span><small>{docs}</small></a>
            <a class="gen-office-file-tab" href="/office/sheets"><span class="gen-office-file-icon sheets"><span class="material-symbols-outlined">table_chart</span></span><span>Hojas</span><small>{sheets}</small></a>
            <a class="gen-office-file-tab" href="/office/presentations"><span class="gen-office-file-icon presentations"><span class="material-symbols-outlined">present_to_all</span></span><span>Presentaciones</span><small>{pres}</small></a>
            <span class="gen-office-tabs-spacer"></span><a class="gen-office-library-tab" href="/office/library"><span class="material-symbols-outlined">folder_open</span><span>Biblioteca</span></a>
        </nav>
        <header class="gen-office-home-hero">
            <div class="gen-office-home-kicker"><span></span> LOCAL · PRIVADO · TUYO</div>
            <h1>¿Qué vas a crear hoy?</h1>
            <p>Escribe, calcula y presenta desde un mismo espacio. Tus archivos permanecen en NOVA.</p>
            <form class="gen-office-home-search" method="get" action="/office/library/search"><span class="material-symbols-outlined">search</span><label class="sr-only" for="office-home-search">Buscar en tus archivos</label><input id="office-home-search" type="search" name="q" placeholder="Buscar en tus archivos…" required><button type="submit">Buscar</button></form>
        </header>
        <section class="gen-office-launch-section" aria-labelledby="gen-office-create-title"><div class="gen-office-section-head"><h2 id="gen-office-create-title">Crear nuevo</h2><span>Elige una aplicación</span></div>
            <div class="gen-office-launch-grid">
                <a class="gen-office-launcher docs" href="/office/docs/new"><span class="gen-office-launcher-icon"><span class="material-symbols-outlined">description</span></span><div><strong>Documento</strong><small>Markdown · .md</small></div><span class="material-symbols-outlined">add</span></a>
                <a class="gen-office-launcher sheets" href="/office/sheets/new"><span class="gen-office-launcher-icon"><span class="material-symbols-outlined">table_chart</span></span><div><strong>Hoja de cálculo</strong><small>Cuadrícula · .csv</small></div><span class="material-symbols-outlined">add</span></a>
                <a class="gen-office-launcher presentations" href="/office/presentations/new"><span class="gen-office-launcher-icon"><span class="material-symbols-outlined">present_to_all</span></span><div><strong>Presentación</strong><small>Diapositivas · .json</small></div><span class="material-symbols-outlined">add</span></a>
                <a class="gen-office-launcher library" href="/office/library"><span class="gen-office-launcher-icon"><span class="material-symbols-outlined">folder_open</span></span><div><strong>Biblioteca</strong><small>Todos tus archivos</small></div><span class="material-symbols-outlined">arrow_outward</span></a>
            </div>
        </section>
        <section class="gen-office-recent" aria-labelledby="gen-office-recent-title"><div class="gen-office-section-head"><h2 id="gen-office-recent-title">Archivos recientes</h2><a href="/office/library">Ver todos<span class="material-symbols-outlined">arrow_forward</span></a></div><div class="gen-office-file-grid">{recent}</div></section>
        <footer class="gen-office-home-footer"><span><span class="material-symbols-outlined">hard_drive</span> Guardado local</span>{mcp}</footer>
        </div>"##,
        docs = docs,
        sheets = sheets,
        pres = pres,
        recent = recent_html,
        mcp = mcp_banner,
    );

    layout_as("NOVA Office", "", &body)
}

/// List all documents.
pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // El archivo .md es la fuente de verdad. La tabla office_files, cuando
    // existe, es solo un índice opcional: nunca debe ocultar archivos del disco.
    let files = templates::list_files(&docs_dir(&state), "md");
    let genoffice_ms = templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    let cards = if files.is_empty() {
        r##"<div class="gen-office-empty"><span class="material-symbols-outlined">description</span><h3>Tu próximo documento empieza aquí.</h3><p>Crea un archivo Markdown y edítalo desde el navegador.</p><a href="/office/docs/new" class="gen-office-primary-button">Crear documento</a></div>"##.to_string()
    } else {
        files.iter().map(|f| {
            format!(
                r##"<article class="gen-office-file-card docs"><a class="gen-office-file-preview" href="/office/docs/{slug}"><span class="material-symbols-outlined">description</span><small>MARKDOWN</small></a><div class="gen-office-file-copy"><a href="/office/docs/{slug}">{display}</a><span>{modified} · {size}</span></div><footer><a href="/office/docs/{slug}/export?fmt=html">HTML</a><a href="/office/docs/{slug}/export?fmt=md">MD</a><form hx-delete="/office/docs/{slug}"><button type="submit" onclick="return confirm('¿Borrar?')" class="gen-office-icon-button danger" aria-label="Borrar {display}"><span class="material-symbols-outlined">delete</span></button></form></footer></article>"##,
                slug = templates::html_escape(&f.name),
                display = templates::html_escape(&f.display),
                size = templates::human_size(f.size),
                modified = templates::html_escape(&f.modified),
            )
        }).collect::<Vec<_>>().join("\n")
    };

    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-manager-page">{tabs}
        <header class="gen-office-manager-head"><div><span>DOCUMENTOS · MARKDOWN</span><h1>Tus documentos</h1><p>{count} archivo(s) guardados en tu espacio local.</p></div><a href="/office/docs/new" class="gen-office-primary-button"><span class="material-symbols-outlined">add</span>Nuevo documento</a></header>
        <nav class="gen-office-appnav" aria-label="Aplicaciones de oficina"><a class="is-active docs" href="/office/docs"><span class="material-symbols-outlined">description</span>Documentos</a><a href="/office/sheets"><span class="material-symbols-outlined">table_chart</span>Hojas</a><a href="/office/presentations"><span class="material-symbols-outlined">present_to_all</span>Presentaciones</a><a href="/office/library"><span class="material-symbols-outlined">folder_open</span>Biblioteca</a></nav>
        <section class="gen-office-file-grid">{cards}</section><footer class="gen-office-home-footer"><span><span class="material-symbols-outlined">hard_drive</span> Markdown local</span>{mcp}</footer></div>"##,
        tabs = templates::office_tab_strip("docs", "Documentos", "/office/docs/new"),
        count = files.len(),
        cards = cards,
        mcp = mcp_banner,
    );

    layout_as("Documentos", "documentos", &body)
}

/// New document form.
pub async fn new_doc() -> impl IntoResponse {
    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-create-page">{tabs}<main class="gen-office-create-shell"><div class="gen-office-create-heading"><span class="gen-office-launcher-icon docs"><span class="material-symbols-outlined">description</span></span><div><span>NUEVO DOCUMENTO</span><h1>Una página en blanco.</h1><p>Crea un archivo Markdown editable, con vista previa y exportación HTML.</p></div></div>
        <form hx-post="/office/docs" hx-swap="outerHTML" class="gen-office-create-form"><div class="gen-office-create-fields"><label for="new-doc-title">Título del documento</label><input id="new-doc-title" type="text" name="title" placeholder="Ej. Apuntes de anatomía" required autofocus></div><div class="gen-office-create-editor"><label for="editor">Contenido Markdown</label><textarea name="content" id="editor" rows="22" placeholder="# Título&#10;&#10;Empieza a escribir…"></textarea></div><footer><a href="/office/docs" class="gen-office-secondary-button">Cancelar</a><span>Se guardará como .md</span><button type="submit" class="gen-office-primary-button">Crear documento<span class="material-symbols-outlined">arrow_forward</span></button></footer></form></main></div>"##,
        tabs = templates::office_tab_strip("docs", "Nuevo documento", "/office/docs/new"),
    );
    layout_as("Nuevo documento", "documentos", &body)
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
    let content = match templates::read_text(&file_path) {
        Ok(content) => content,
        Err(e) => {
            tracing::error!(
                slug_len = slug.len(),
                "office docs edit: read failed: {}",
                e
            );
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                layout("Error", INTERNAL_ERROR_HTML),
            )
                .into_response();
        }
    };
    let saved_banner = if q.saved.is_some() {
        r##"<span class="gen-office-saved"><span class="material-symbols-outlined">check_circle</span>Guardado</span>"##
    } else {
        ""
    };

    let genoffice_ms = templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    let editor = doc_editor(&slug, &content);
    let ai_content = format!(
        r##"<div class="gen-office-tool-list"><button type="button" onclick="closeOfficeAi();document.getElementById('editor').focus()"><span class="material-symbols-outlined">edit</span><span><strong>Continuar escribiendo</strong><small>Volver al lienzo del documento</small></span></button><a href="/office/docs/{slug}/preview"><span class="material-symbols-outlined">visibility</span><span><strong>Revisar resultado</strong><small>Abrir la vista previa renderizada</small></span></a><a href="/office/docs/{slug}/export?fmt=html"><span class="material-symbols-outlined">download</span><span><strong>Exportar como HTML</strong><small>Descargar una página independiente</small></span></a></div>"##,
        slug = templates::html_escape(&slug),
    );
    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-editor-page">{tabs}
        <header class="gen-office-ribbon"><nav class="gen-office-menubar" aria-label="Acciones del documento"><a class="is-active" href="/office/docs/{slug}">Inicio</a><a href="/office/docs/{slug}/preview">Vista previa</a><a href="/office/docs/{slug}/export?fmt=md">Exportar .md</a></nav><div class="gen-office-ribbon-actions">{saved}{ai_toggle}<a class="gen-office-primary-button" href="/office/docs/{slug}/export?fmt=html"><span class="material-symbols-outlined">download</span>HTML</a></div></header>
        <div class="gen-office-workspace">{ai}<main class="gen-office-editor-main"><div class="gen-office-canvas-head"><div><span>DOCUMENTO MARKDOWN</span><strong>{title}</strong></div><span class="gen-office-local-badge"><span class="material-symbols-outlined">hard_drive</span>Archivo local</span></div><section class="gen-office-doc-workspace">{editor}</section></main></div>
        <footer class="gen-office-statusbar"><span><span class="material-symbols-outlined">description</span> .md · UTF-8</span>{mcp}<span>NOVA Office</span></footer></div><script>{workspace_js}</script>"##,
        tabs = templates::office_tab_strip("docs", &slug.replace('-', " "), "/office/docs/new"),
        slug = templates::html_escape(&slug),
        saved = saved_banner,
        ai_toggle = templates::OFFICE_AI_TOGGLE,
        ai = templates::office_ai_panel("Herramientas del documento", "Revisa, exporta o continúa editando. NOVA AI se abre aparte para no alterar tu archivo sin confirmación.", &ai_content),
        title = templates::html_escape(&slug.replace('-', " ")),
        editor = editor,
        mcp = mcp_banner,
        workspace_js = templates::OFFICE_WORKSPACE_JS,
    );

    layout_as(&format!("Editar: {}", slug), "documentos", &body).into_response()
}

/// The editor widget (ribbon de formato + lienzo + guardado real).
fn doc_editor(slug: &str, content: &str) -> String {
    format!(
        r##"<div class="gen-office-doc-toolbar" role="toolbar" aria-label="Formato Markdown">
            <span class="gen-office-tool-label">FORMATO</span>
            <button type="button" onclick="insertMd('**','**')" title="Negrita" aria-label="Negrita"><span class="material-symbols-outlined">format_bold</span></button>
            <button type="button" onclick="insertMd('*','*')" title="Cursiva" aria-label="Cursiva"><span class="material-symbols-outlined">format_italic</span></button>
            <button type="button" onclick="appendMd('## ')" title="Encabezado" aria-label="Encabezado"><span class="material-symbols-outlined">title</span></button>
            <span class="gen-office-tool-separator"></span>
            <button type="button" onclick="appendMd('- ')" title="Lista" aria-label="Lista con viñetas"><span class="material-symbols-outlined">format_list_bulleted</span></button>
            <button type="button" onclick="appendMd('1. ')" title="Lista numerada" aria-label="Lista numerada"><span class="material-symbols-outlined">format_list_numbered</span></button>
            <button type="button" onclick="appendMd('&gt; ')" title="Cita" aria-label="Cita"><span class="material-symbols-outlined">format_quote</span></button>
            <span class="gen-office-tool-separator"></span>
            <button type="button" onclick="insertMd('`','`')" title="Código" aria-label="Código en línea"><span class="material-symbols-outlined">code</span></button>
            <button type="button" onclick="appendMd('```\n\n```')" title="Bloque de código" aria-label="Bloque de código"><span class="material-symbols-outlined">data_object</span></button>
            <button type="button" onclick="insertMd('[', '](url)')" title="Enlace" aria-label="Insertar enlace"><span class="material-symbols-outlined">link</span></button>
            <span class="gen-office-toolbar-format">Markdown</span>
        </div>
        <form hx-put="/office/docs/{slug}" hx-swap="outerHTML" class="gen-office-doc-form">
            <label class="sr-only" for="editor">Contenido del documento</label>
            <div class="gen-office-paper"><div class="gen-office-paper-meta"><span>DOCUMENTO NOVA</span><span>MARKDOWN · UTF-8</span></div><textarea name="content" id="editor" rows="30" spellcheck="true">{content}</textarea></div>
            <footer class="gen-office-editor-save"><span><span class="material-symbols-outlined">cloud_done</span> Los cambios se guardan en el archivo local.</span><button type="submit" id="save-document-button" class="gen-office-primary-button"><span class="material-symbols-outlined">save</span>Guardar cambios</button></footer>
        </form>
        <script>{js}</script>"##,
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
        if let Err(e) = sqlx::query(
            r#"
            INSERT INTO office_files (slug, kind, filename, size, modified_at)
            VALUES (?, 'docs', ?, ?, datetime('now'))
            "#,
        )
        .bind(&slug)
        .bind(format!("{}.md", slug))
        .bind(form.content.len() as i64)
        .execute(&state.db)
        .await
        {
            tracing::warn!("office docs create: optional index write failed: {}", e);
        }
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
    if let Err(e) = std::fs::remove_file(&file_path) {
        let status = if e.kind() == std::io::ErrorKind::NotFound {
            axum::http::StatusCode::NOT_FOUND
        } else {
            tracing::error!(slug_len = slug.len(), "office docs delete: failed: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        };
        return (
            status,
            layout(
                "Error",
                "<p class=\"text-red-400\">No se pudo borrar el documento.</p>",
            ),
        )
            .into_response();
    }

    // Si la tabla `office_files` existe, mantén el índice sincronizado.
    if templates::office_files_table_exists(&state.db).await {
        if let Err(e) = sqlx::query("DELETE FROM office_files WHERE kind = 'docs' AND slug = ?")
            .bind(&slug)
            .execute(&state.db)
            .await
        {
            tracing::warn!("office docs delete: optional index cleanup failed: {}", e);
        }
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
    let content = match templates::read_text(&file_path) {
        Ok(content) => content,
        Err(e) => {
            tracing::error!(
                slug_len = slug.len(),
                "office docs preview: read failed: {}",
                e
            );
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                layout("Error", INTERNAL_ERROR_HTML),
            )
                .into_response();
        }
    };
    let html = md_to_html(&content);

    // Sondeo honesto del MCP de GenOffice (cached, 30s TTL).
    let genoffice_ms = templates::genoffice_probe_cached(&state.config.genoffice_mcp_url).await;
    let mcp_banner =
        templates::genoffice_status_banner(&state.config.genoffice_mcp_url, genoffice_ms);

    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-editor-page">{tabs}<header class="gen-office-ribbon"><nav class="gen-office-menubar" aria-label="Acciones de vista previa"><a href="/office/docs/{slug}">Editar documento</a><a class="is-active" href="/office/docs/{slug}/preview">Vista previa</a></nav><div class="gen-office-ribbon-actions"><a class="gen-office-primary-button" href="/office/docs/{slug}/export?fmt=html"><span class="material-symbols-outlined">download</span>Exportar HTML</a></div></header><main class="gen-office-preview-main"><div class="gen-office-canvas-head"><div><span>VISTA RENDERIZADA</span><strong>{title}</strong></div><a href="/office/docs/{slug}" class="gen-office-secondary-button"><span class="material-symbols-outlined">edit</span>Volver al editor</a></div><article class="gen-office-document-preview">{html}</article></main><footer class="gen-office-statusbar"><span>Vista previa local</span>{mcp}<span>NOVA Office</span></footer></div>"##,
        tabs = templates::office_tab_strip("docs", &slug.replace('-', " "), "/office/docs/new"),
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
    let content = match templates::read_text(&file_path) {
        Ok(content) => content,
        Err(e) => {
            tracing::error!(
                slug_len = slug.len(),
                "office docs export: read failed: {}",
                e
            );
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo completar la operación",
            )
                .into_response();
        }
    };
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
