//! HTML template helpers for the web office suite.
//!
//! Server-side rendering with HTMX + Tailwind, using the same dark / cyberpunk
//! theme as the Study Platform login page. No external services — just pulldown-cmark
//! for Markdown→HTML and the `csv` crate where relevant.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use pulldown_cmark::{html, Options, Parser};

/// HTTP client compartido para sondeos MCP (read-only `initialize`).
/// Comparte el patrón de `api.rs` y `stitch_mcp.rs`: timeout corto (2.5 s)
/// para que una instancia caída no corte la respuesta.
static PROBE_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(2500))
        .build()
        .expect("reqwest client for office MCP probes")
});

/// TTL del caché de sondeo MCP: 30 s (coincide con `/health`).
const MCP_PROBE_TTL: Duration = Duration::from_secs(30);

/// Caché del último sondeo a GenOffice: `(instante, latency_ms_u_none)`.
/// `None` significa "sin respuesta" (MCP caído o no configurado).
static GENOFFICE_PROBE_CACHE: std::sync::Mutex<Option<(Instant, Option<u128>)>> =
    std::sync::Mutex::new(None);

/// Probea un servidor MCP vía HTTP POST de `initialize` (JSON-RPC 2.0).
///
/// **Read-only**: no llama a `tools/call`, solo verifica que el servidor
/// responda con 2xx. Devuelve `Some(latencia_ms)` si responde, `None` si la
/// URL está vacía o el servidor no responde.
pub async fn probe_mcp(url: &str) -> Option<u128> {
    if url.is_empty() {
        return None;
    }
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {
                "name": "study-platform-office",
                "version": env!("CARGO_PKG_VERSION"),
            },
        },
    });
    let t0 = Instant::now();
    let ok = PROBE_CLIENT
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    ok.then_some(t0.elapsed().as_millis())
}

/// Sondeo con caché (TTL 30 s) del MCP de GenOffice.
/// Devuelve `Some(latencia_ms)` si está activo, `None` si está caído.
/// Nunca lanza: ante error devuelve `None` (degradación honesta).
pub async fn genoffice_probe_cached(url: &str) -> Option<u128> {
    // Patrón idéntico a api.rs: el guard es temporal y se cae antes del .await,
    // manteniendo el future Send para cumplir el trait Handler de Axum.
    if let Some((at, ms)) = *GENOFFICE_PROBE_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
    {
        if at.elapsed() < MCP_PROBE_TTL {
            return ms;
        }
    }

    let ms = probe_mcp(url).await;
    *GENOFFICE_PROBE_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), ms));
    ms
}

/// Comprueba si la tabla `office_files` existe en la base SQLite.
///
/// La suite ofimática puede operar en modo **filesystem-puro** (por defecto,
/// sin tabla) o modo **indexado en BD** (si una migración creó la tabla).
/// No inventa datos: si la tabla no existe, el caller usa el filesystem.
pub async fn office_files_table_exists(pool: &sqlx::SqlitePool) -> bool {
    sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='office_files'",
    )
    .fetch_optional(pool)
    .await
    .is_ok_and(|opt| opt.is_some())
}

/// Lista documentos ofimáticos desde la tabla `office_files` (si existe).
///
/// Esquema esperado (si la tabla existe):
/// ```sql
/// CREATE TABLE office_files (
///     slug        TEXT NOT NULL,
///     kind        TEXT NOT NULL,  -- 'docs', 'sheets', 'presentations'
///     filename    TEXT NOT NULL,
///     size        INTEGER NOT NULL DEFAULT 0,
///     modified_at TEXT
/// );
/// ```
/// Devuelve un `Vec<FileInfo>` vacío si la tabla no existe o hay error
/// (el caller hará fallback al filesystem).
pub async fn list_files_from_db(pool: &sqlx::SqlitePool, kind: &str) -> Vec<FileInfo> {
    #[derive(sqlx::FromRow)]
    struct OfficeRow {
        slug: String,
        size: i64,
        modified_at: Option<String>,
    }

    let rows: Vec<OfficeRow> = sqlx::query_as::<_, OfficeRow>(
        "SELECT slug, size, modified_at FROM office_files WHERE kind = ? ORDER BY modified_at DESC NULLS LAST",
    )
    .bind(kind)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .map(|r| FileInfo {
            name: r.slug.clone(),
            display: r.slug.replace('-', " "),
            size: r.size as u64,
            modified: r.modified_at.unwrap_or_default(),
        })
        .collect()
}

/// Banner de estado honesto del MCP de GenOffice.
///
/// - URL vacía → "sin configurar"
/// - Responde → "activo (latencia: Nms)"
/// - No responde → **"Sin conexión al MCP de GenOffice"** (degradación honesta)
pub fn genoffice_status_banner(url: &str, probe_ms: Option<u128>) -> String {
    if url.is_empty() {
        r#"<div class="mb-3 text-xs text-gray-500">GenOffice MCP: sin configurar</div>"#.to_string()
    } else if let Some(ms) = probe_ms {
        format!(
            r#"<div class="mb-3 flex items-center gap-1.5 text-xs text-secondary">
                <span class="w-1.5 h-1.5 rounded-full bg-secondary animate-pulse"></span>
                GenOffice MCP: activo (latencia: {}ms)
            </div>"#,
            ms
        )
    } else {
        r#"<div class="mb-3 flex items-center gap-1.5 text-xs text-red-400">
            <span class="w-1.5 h-1.5 rounded-full bg-error animate-pulse"></span>
            Sin conexión al MCP de GenOffice
        </div>"#
            .to_string()
    }
}

/// Convert Markdown → HTML (CommonMark + tables + footnotes).
pub fn md_to_html(input: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    let parser = Parser::new_ext(input, opts);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

/// Escape `& < > " '` for safe insertion into HTML text or attributes.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Slugify a title into a URL- and filename-safe identifier.
pub fn slugify(title: &str) -> String {
    let mut out: String = title
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else if c.is_whitespace() {
                '-'
            } else {
                '\0'
            }
        })
        .filter(|c| *c != '\0')
        .collect();
    out = out.trim_matches(|c| c == '-' || c == '_').to_string();
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

/// A file discovered in an office directory.
pub struct FileInfo {
    /// The slug (filename stem).
    pub name: String,
    /// Human-readable display name.
    pub display: String,
    /// File size in bytes.
    pub size: u64,
    /// Last-modified timestamp, RFC 3339.
    pub modified: String,
}

/// List files with the given extension in `dir`, newest first.
pub fn list_files(dir: &Path, ext: &str) -> Vec<FileInfo> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return files,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_ext = path.extension().and_then(|e| e.to_str()).unwrap_or("") == ext;
        if !is_ext {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let meta = fs::metadata(&path).ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = meta
            .and_then(|m| m.modified().ok())
            .map(format_rfc)
            .unwrap_or_default();
        let display = name.replace('-', " ");
        files.push(FileInfo {
            name,
            display,
            size,
            modified,
        });
    }
    files.sort_by(|a, b| a.modified.cmp(&b.modified).reverse());
    files
}

/// Count files of a given extension in `dir`.
pub fn count_files(dir: &Path, ext: &str) -> usize {
    list_files(dir, ext).len()
}

/// Resolve a `SysTime` to an RFC 3339 string.
fn format_rfc(t: SystemTime) -> String {
    use chrono::{TimeZone, Utc};
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => Utc
            .timestamp_opt(d.as_secs() as i64, 0)
            .unwrap()
            .to_rfc3339(),
        Err(_) => "unknown".to_string(),
    }
}

/// Pretty human-readable size (KB / MB).
pub fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

/// Envoltorio de Oficina unificado con Nova. Los editores conservan sus
/// formularios, scripts y endpoints; desaparece la cabecera independiente.
pub fn layout(title: &str, body: &str) -> String {
    layout_as(title, "", body)
}

/// Variante que marca la sección activa dentro del mapa Nova.
pub fn layout_as(title: &str, active: &str, body: &str) -> String {
    let html = crate::handlers::stitch_v2::stitch_v2_page(active, body, true);
    let Some(open) = html.find("<title>") else {
        return html;
    };
    let Some(relative_close) = html[open..].find("</title>") else {
        return html;
    };
    let close = open + relative_close + "</title>".len();
    let mut titled = String::with_capacity(html.len() + title.len() + 16);
    titled.push_str(&html[..open]);
    titled.push_str(&format!("<title>{} — NOVA Office</title>", html_escape(title)));
    titled.push_str(&html[close..]);
    titled
}

/// Franja superior de la suite: inicio, archivo activo, creación y Biblioteca.
/// Mantiene la metáfora de pestañas de GenOffice sin ocultar las rutas reales.
pub fn office_tab_strip(kind: &str, title: &str, new_href: &str) -> String {
    let (icon, label, list_href) = match kind {
        "docs" => ("description", "Documentos", "/office/docs"),
        "sheets" => ("table_chart", "Hojas", "/office/sheets"),
        "presentations" => ("present_to_all", "Presentaciones", "/office/presentations"),
        "library" => ("folder_open", "Biblioteca", "/office/library"),
        _ => ("grid_view", "Oficina", "/office"),
    };
    format!(
        r##"<nav class="gen-office-tabs" aria-label="Pestañas de Oficina">
            <a class="gen-office-brand-tab" href="/office"><span class="gen-office-brand-mark"><span class="material-symbols-outlined">grid_view</span></span><strong>NOVA <span>Office</span></strong></a>
            <a class="gen-office-file-tab is-active" href="{list_href}"><span class="gen-office-file-icon {kind}"><span class="material-symbols-outlined">{icon}</span></span><span>{title}</span><small>{label}</small></a>
            <a class="gen-office-add-tab" href="{new_href}" aria-label="Crear un archivo"><span class="material-symbols-outlined">add</span></a>
            <span class="gen-office-tabs-spacer"></span>
            <a class="gen-office-library-tab" href="/office/library"><span class="material-symbols-outlined">folder_open</span><span>Biblioteca</span></a>
        </nav><script>{feedback_js}</script>"##,
        feedback_js = OFFICE_FEEDBACK_JS,
        list_href = list_href,
        kind = html_escape(kind),
        icon = icon,
        title = html_escape(title),
        label = label,
        new_href = html_escape(new_href),
    )
}

/// Botón común para mostrar/ocultar el panel de herramientas y NOVA AI.
pub const OFFICE_AI_TOGGLE: &str = r##"<button type="button" class="gen-office-ribbon-button" data-office-ai-toggle onclick="toggleOfficeAi()" aria-controls="gen-office-ai-panel" aria-expanded="false"><span class="material-symbols-outlined">auto_awesome</span><span>Herramientas</span></button>"##;

/// Panel lateral de trabajo. Las acciones se pasan como HTML y siempre usan
/// controles/endpoints existentes; el enlace a NOVA AI no finge editar el archivo.
pub fn office_ai_panel(title: &str, description: &str, content: &str) -> String {
    format!(
        r##"<button type="button" class="gen-office-ai-backdrop" onclick="closeOfficeAi()" aria-label="Cerrar herramientas"></button>
        <aside class="gen-office-ai" id="gen-office-ai-panel" aria-label="Herramientas de Oficina">
            <header><div><span class="gen-office-ai-mark"><span class="material-symbols-outlined">auto_awesome</span></span><strong>NOVA AI</strong></div><button type="button" class="gen-office-icon-button" onclick="closeOfficeAi()" aria-label="Cerrar herramientas"><span class="material-symbols-outlined">close</span></button></header>
            <div class="gen-office-ai-intro"><span>ESPACIO DE TRABAJO</span><h2>{title}</h2><p>{description}</p></div>
            <div class="gen-office-ai-content">{content}</div>
            <footer><a href="/dashboard/ai"><span class="material-symbols-outlined">forum</span><span><strong>Abrir NOVA AI</strong><small>Conversa con el asistente académico</small></span><span class="material-symbols-outlined">arrow_outward</span></a></footer>
        </aside>"##,
        title = html_escape(title),
        description = html_escape(description),
        content = content,
    )
}

/// Feedback visible para los formularios HTMX de Oficina. Los errores 4xx/5xx
/// no se intercambian en el DOM por defecto; sin este aviso parecerían clics muertos.
pub const OFFICE_FEEDBACK_JS: &str = r##"
if (!window.genOfficeFeedbackReady) {
    window.genOfficeFeedbackReady = true;
    function genOfficeHtmxFeedback(event, networkError) {
        const target = event.target;
        if (!(target instanceof Element)) return;
        const officePage = target.closest('.gen-office-page') || target.querySelector('.gen-office-page');
        if (!officePage) return;
        const status = networkError ? 0 : (event.detail.xhr ? event.detail.xhr.status : 0);
        let message = 'No se pudo completar la acción. Inténtalo de nuevo.';
        if (networkError) message = 'Sin conexión con el servidor. Tus cambios no se han guardado.';
        else if (status === 400) message = 'Revisa los datos del formulario: faltan campos o no son válidos.';
        else if (status === 404) message = 'El archivo ya no existe o fue movido.';
        else if (status === 409) message = 'Ya existe un archivo con ese nombre. Elige otro título.';
        else if (status === 413) message = 'El contenido supera el tamaño permitido.';
        if (window.__SP__ && typeof window.__SP__.toast === 'function') {
            window.__SP__.toast(message, 'err', 6500);
        } else {
            window.alert(message);
        }
    }
    document.addEventListener('htmx:responseError', function (event) {
        genOfficeHtmxFeedback(event, false);
    });
    document.addEventListener('htmx:sendError', function (event) {
        genOfficeHtmxFeedback(event, true);
    });
}
"##;

/// Interacción del panel lateral. En escritorio se pliega en su columna; en
/// pantallas compactas se abre como cajón superpuesto y Escape lo cierra.
pub const OFFICE_WORKSPACE_JS: &str = r##"
function officeAiCompact() {
    return window.matchMedia('(max-width: 1100px)').matches;
}
function syncOfficeAiToggle() {
    const workspace = document.querySelector('.gen-office-workspace');
    const toggle = document.querySelector('[data-office-ai-toggle]');
    if (!workspace || !toggle) return;
    const open = officeAiCompact() ? workspace.classList.contains('ai-open') : !workspace.classList.contains('ai-hidden');
    toggle.setAttribute('aria-expanded', String(open));
}
function openOfficeAi() {
    const workspace = document.querySelector('.gen-office-workspace');
    if (!workspace) return;
    workspace.classList.remove('ai-hidden');
    if (officeAiCompact()) workspace.classList.add('ai-open');
    syncOfficeAiToggle();
}
function closeOfficeAi() {
    const workspace = document.querySelector('.gen-office-workspace');
    if (!workspace) return;
    if (officeAiCompact()) workspace.classList.remove('ai-open');
    else workspace.classList.add('ai-hidden');
    syncOfficeAiToggle();
}
function toggleOfficeAi() {
    const workspace = document.querySelector('.gen-office-workspace');
    if (!workspace) return;
    const open = officeAiCompact() ? workspace.classList.contains('ai-open') : !workspace.classList.contains('ai-hidden');
    if (open) closeOfficeAi(); else openOfficeAi();
}
document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') closeOfficeAi();
});
window.addEventListener('resize', syncOfficeAiToggle);
syncOfficeAiToggle();
"##;

/// Return an empty page (used when a document/slide editor has no row body).
pub fn empty_layout() -> String {
    layout("NOVA Office", "")
}

/// HTMX-friendly redirect: HTTP 200 + `HX-Redirect` header.
/// HTMX clients perform a client-side navigation to `url`; plain clients receive
/// an empty body. Used for create/update/delete navigation so a single response
/// works for both HTMX forms and direct requests.
pub fn hx_redirect(url: &str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("HX-Redirect", url)
        .body(Body::empty())
        .unwrap()
}

/// File-download response with proper content type and `Content-Disposition: attachment`.
pub fn file_download(filename: &str, mime: &str, bytes: Vec<u8>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mime)
        .header(
            "Content-Disposition",
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(Body::from(bytes))
        .unwrap()
}

/// MIME type for an office file extension.
pub fn mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// JavaScript inserted into pages with a Markdown `<textarea id="editor">`.
/// Provides toolbar buttons that inject bold/italic/code/links/lists/headers.
pub const MARKDOWN_TOOLBAR_JS: &str = r#"
function insertMd(prefix, suffix) {
    const ta = document.getElementById('editor');
    if (!ta) return;
    const start = ta.selectionStart;
    const end = ta.selectionEnd;
    const before = ta.value.substring(0, start);
    const sel = ta.value.substring(start, end);
    const after = ta.value.substring(end);
    ta.value = before + prefix + sel + suffix + after;
    ta.dispatchEvent(new Event('input', { bubbles: true }));
    ta.focus();
}
function appendMd(text) {
    const ta = document.getElementById('editor');
    if (!ta) return;
    ta.value += text + '\n';
    ta.dispatchEvent(new Event('input', { bubbles: true }));
    ta.focus();
}
"#;

/// JavaScript for the spreadsheet grid: serialize a contenteditable `<table id="grid">`
/// into a hidden `csv_data` input so it can be submitted with the form.
pub const GRID_SERIALIZE_JS: &str = r#"
function officeCsvCell(value) {
    const clean = value.replace(/\r?\n/g, ' ');
    return /[",]/.test(clean) ? '"' + clean.replace(/"/g, '""') + '"' : clean;
}
function serializeGrid() {
    const lines = [];
    document.querySelectorAll('#grid tbody tr').forEach((tr) => {
        const cells = tr.querySelectorAll('td[contenteditable]');
        if (!cells.length) return;
        const values = [];
        cells.forEach((td) => values.push(officeCsvCell(td.textContent)));
        lines.push(values.join(','));
    });
    const hidden = document.getElementById('csv_data');
    if (hidden) hidden.value = lines.join('\n');
    return hidden ? hidden.value : '';
}
function officeColumnName(index) {
    let value = index + 1;
    let name = '';
    while (value > 0) {
        const remainder = (value - 1) % 26;
        name = String.fromCharCode(65 + remainder) + name;
        value = Math.floor((value - 1) / 26);
    }
    return name;
}
function renumberOfficeGrid() {
    const grid = document.getElementById('grid');
    if (!grid) return;
    const rows = [...grid.querySelectorAll('tbody tr')];
    const columns = rows.reduce((max, row) => Math.max(max, row.querySelectorAll('td[contenteditable]').length), 0);
    const head = grid.querySelector('thead tr');
    if (head) {
        while (head.cells.length > columns + 1) head.deleteCell(-1);
        while (head.cells.length < columns + 1) head.insertCell(-1).outerHTML = '<th scope="col"></th>';
        [...head.cells].forEach((cell, index) => { cell.textContent = index === 0 ? '' : officeColumnName(index - 1); });
    }
    rows.forEach((row, rowIndex) => {
        const number = row.querySelector('th');
        if (number) { number.textContent = String(rowIndex + 1); number.scope = 'row'; }
        row.querySelectorAll('td[contenteditable]').forEach((cell, columnIndex) => {
            const ref = officeColumnName(columnIndex) + (rowIndex + 1);
            cell.dataset.r = String(rowIndex);
            cell.dataset.c = String(columnIndex);
            cell.dataset.cell = ref;
            cell.setAttribute('aria-label', 'Celda ' + ref);
        });
    });
    const count = document.getElementById('sheet-grid-count');
    if (count) count.textContent = rows.length + ' filas · ' + columns + ' columnas';
}
function selectOfficeCell(cell) {
    if (!cell) return;
    document.querySelectorAll('#grid td.is-selected').forEach((item) => item.classList.remove('is-selected'));
    cell.classList.add('is-selected');
    const reference = document.getElementById('active-cell');
    const formula = document.getElementById('formula-input');
    if (reference) reference.textContent = cell.dataset.cell || '';
    if (formula) formula.value = cell.textContent;
}
function addOfficeGridRow() {
    const body = document.querySelector('#grid tbody');
    if (!body) return;
    const columns = body.querySelector('tr')?.querySelectorAll('td[contenteditable]').length || 5;
    const row = document.createElement('tr');
    row.appendChild(document.createElement('th'));
    for (let index = 0; index < columns; index += 1) {
        const cell = document.createElement('td');
        cell.contentEditable = 'true';
        row.appendChild(cell);
    }
    body.appendChild(row);
    renumberOfficeGrid();
    selectOfficeCell(row.querySelector('td'));
}
function addOfficeGridColumn() {
    const grid = document.getElementById('grid');
    if (!grid) return;
    grid.querySelectorAll('tbody tr').forEach((row) => {
        const cell = document.createElement('td');
        cell.contentEditable = 'true';
        row.appendChild(cell);
    });
    renumberOfficeGrid();
}
function initializeOfficeGrid() {
    const grid = document.getElementById('grid');
    const formula = document.getElementById('formula-input');
    if (!grid || !formula) return;
    renumberOfficeGrid();
    if (!grid.dataset.ready) {
        grid.dataset.ready = 'true';
        grid.addEventListener('focusin', (event) => {
            if (event.target.matches('td[contenteditable]')) selectOfficeCell(event.target);
        });
        grid.addEventListener('input', (event) => {
            if (event.target.matches('td[contenteditable].is-selected')) formula.value = event.target.textContent;
        });
        formula.addEventListener('input', () => {
            const selected = grid.querySelector('td.is-selected');
            if (selected) selected.textContent = formula.value;
        });
        formula.addEventListener('keydown', (event) => {
            if (event.key === 'Enter') {
                event.preventDefault();
                const selected = grid.querySelector('td.is-selected');
                if (selected) selected.focus();
            }
        });
    }
    if (!grid.querySelector('td.is-selected')) selectOfficeCell(grid.querySelector('td[contenteditable]'));
}
initializeOfficeGrid();
"#;

/// Read a file's text without hiding permission or I/O failures.
pub fn read_text(path: &Path) -> std::io::Result<String> {
    fs::read_to_string(path)
}

/// Write text to a file, creating parent dirs.
pub fn write_text(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)
}

/// Read file bytes without converting an I/O failure into an empty download.
pub fn read_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    fs::read(path)
}

/// Filename stem helper.
pub fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}
