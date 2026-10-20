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
        </div>"#.to_string()
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

/// Full-page HTML wrapper (cyberpunk dark theme + office navigation).
pub fn layout(title: &str, body: &str) -> String {
    LAYOUT_TEMPLATE
        .replace("__TITLE__", &html_escape(title))
        .replace("__BODY__", body)
}

/// Return an empty page (used when a document/slide editor has no row body).
pub fn empty_layout() -> String {
    LAYOUT_TEMPLATE
        .replace("__TITLE__", "Suite de oficina")
        .replace("__BODY__", "")
}

const LAYOUT_TEMPLATE: &str = r##"<!DOCTYPE html>
<html lang="es">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>__TITLE__ — Suite de oficina</title>
    <!-- Google Fonts: Manrope (headings) + Hanken Grotesk (body) -->
    <link rel="preconnect" href="https://fonts.googleapis.com">
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
    <link href="https://fonts.googleapis.com/css2?family=Hanken+Grotesk:wght@400;500;600;700&family=Manrope:wght@400;500;600;700&display=swap" rel="stylesheet">
    <!-- Material Symbols Outlined -->
    <link rel="stylesheet" href="https://fonts.googleapis.com/icon?family=Material+Symbols+Outlined">
    <!-- HTMX + Tailwind CDN -->
    <script src="https://unpkg.com/htmx.org@2.0.3/dist/htmx.min.js"></script>
    <script src="https://cdn.tailwindcss.com"></script>
    <script>
        tailwind.config = {
            theme: {
                extend: {
                    colors: {
                        /* Surface stack */
                        background: '#101419',
                        surface: '#101419',
                        'surface-dim': '#101419',
                        'surface-bright': '#36393f',
                        'surface-container-lowest': '#0a0e13',
                        'surface-container-low': '#181c21',
                        'surface-container': '#1c2025',
                        'surface-container-high': '#262a30',
                        'surface-container-highest': '#31353b',
                        'surface-variant': '#31353b',
                        'surface-tint': '#4ddcc6',
                        'on-surface': '#e0e2ea',
                        'on-surface-variant': '#bbcac5',
                        'on-background': '#e0e2ea',
                        /* Outline */
                        outline: '#859490',
                        'outline-variant': '#3c4946',
                        /* Primary (luminescent cyan - AI telemetry) */
                        primary: '#5eead4',
                        'on-primary': '#003730',
                        'primary-container': '#5eead4',
                        'on-primary-container': '#00675b',
                        'inverse-primary': '#006b5e',
                        'primary-fixed': '#6ef9e2',
                        'primary-fixed-dim': '#4ddcc6',
                        'on-primary-fixed': '#00201b',
                        'on-primary-fixed-variant': '#005047',
                        /* Secondary (soft lime - user action) */
                        secondary: '#9ed753',
                        'on-secondary': '#203700',
                        'secondary-container': '#6b9f1f',
                        'on-secondary-container': '#1b2f00',
                        'secondary-fixed': '#b9f46c',
                        'secondary-fixed-dim': '#9ed753',
                        'on-secondary-fixed': '#102000',
                        'on-secondary-fixed-variant': '#304f00',
                        /* Tertiary (muted violet - taxonomy/metadata) */
                        tertiary: '#a78bfa',
                        'on-tertiary': '#381385',
                        'tertiary-container': '#dbcdff',
                        'on-tertiary-container': '#6448b2',
                        'tertiary-fixed': '#e8ddff',
                        'tertiary-fixed-dim': '#cebdff',
                        'on-tertiary-fixed': '#21005e',
                        'on-tertiary-fixed-variant': '#4f319c',
                        /* Error */
                        error: '#ffb4ab',
                        'on-error': '#690005',
                        'error-container': '#93000a',
                        'on-error-container': '#ffdad6',
                        /* Inverse */
                        'inverse-surface': '#e0e2ea',
                        'inverse-on-surface': '#2d3136',
                    },
                    fontFamily: {
                        manrope: ['Manrope', 'sans-serif'],
                        hanken: ['Hanken Grotesk', 'sans-serif'],
                    },
                }
            },
        }
    </script>
    <style>
        @layer base {
            body {
                @apply bg-surface text-on-surface font-hanken;
            }
            h1, h2, h3, h4, h5, h6 {
                @apply font-manrope;
            }
        }
        td[contenteditable]:focus {
            outline: 2px solid #4ddcc6;
            @apply bg-surface-container;
        }
        th {
            @apply bg-surface-container;
        }
        .slide {
            @apply border border-outline-variant;
        }
    </style>
</head>
<body class="min-h-screen">
    <header class="fixed top-0 left-0 right-0 bg-surface-container-low/90 backdrop-blur-sm border-b border-outline z-50">
        <div class="flex items-center justify-between px-6 py-3">
            <div class="flex items-center space-x-6">
                <h1 class="text-xl font-semibold text-primary">Suite de oficina</h1>
                <nav class="flex items-center space-x-1">
                    <a href="/office/docs" class="flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-on-surface-variant hover:text-on-surface hover:bg-surface-container-high/50 transition-colors">
                        <span class="material-symbols-outlined text-base">edit_document</span>
                        <span>Documentos</span>
                    </a>
                    <a href="/office/sheets" class="flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-on-surface-variant hover:text-on-surface hover:bg-surface-container-high/50 transition-colors">
                        <span class="material-symbols-outlined text-base">table_chart</span>
                        <span>Hojas</span>
                    </a>
                    <a href="/office/presentations" class="flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-on-surface-variant hover:text-on-surface hover:bg-surface-container-high/50 transition-colors">
                        <span class="material-symbols-outlined text-base">slideshow</span>
                        <span>Presentaciones</span>
                    </a>
                    <a href="/office/library" class="flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-on-surface-variant hover:text-on-surface hover:bg-surface-container-high/50 transition-colors">
                        <span class="material-symbols-outlined text-base">folder_copy</span>
                        <span>Biblioteca</span>
                    </a>
                </nav>
            </div>
            <form hx-post="/logout" hx-swap="outerHTML">
                <button type="submit" class="flex items-center space-x-1.5 px-3 py-1.5 rounded-md text-on-surface-variant hover:text-primary transition-colors">
                    <span class="material-symbols-outlined text-base">logout</span>
                    <span>Salir</span>
                </button>
            </form>
        </div>
    </header>
    <main class="pt-16 pb-6 px-6">
        <div class="max-w-5xl mx-auto">
            <div class="bg-surface-container rounded-xl border border-outline p-6">
__BODY__
            </div>
        </div>
    </main>
</body>
</html>"##;

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
    ta.focus();
}
function appendMd(text) {
    const ta = document.getElementById('editor');
    if (!ta) return;
    ta.value += text + '\n';
    ta.focus();
}
"#;

/// JavaScript for the spreadsheet grid: serialize a contenteditable `<table id="grid">`
/// into a hidden `csv_data` input so it can be submitted with the form.
pub const GRID_SERIALIZE_JS: &str = r#"
function serializeGrid() {
    const rows = document.querySelectorAll('#grid tr');
    const lines = [];
    rows.forEach(tr => {
        const cells = tr.querySelectorAll('td[contenteditable]');
        const vals = [];
        cells.forEach(td => vals.push(td.textContent.replace(/\r?\n/g, ' ')));
        lines.push(vals.join(','));
    });
    const hidden = document.getElementById('csv_data');
    if (hidden) hidden.value = lines.join('\n');
}
"#;

/// Read a file's text, returning an empty string on error.
pub fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Write text to a file, creating parent dirs.
pub fn write_text(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(path, content)
}

/// Read file bytes, returning empty on error.
pub fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_default()
}

/// Filename stem helper.
pub fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string()
}
