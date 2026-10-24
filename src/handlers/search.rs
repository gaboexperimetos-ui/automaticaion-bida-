//! Búsqueda de notas — handler `GET /api/search` (+ página opcional `/dashboard/search`).
//!
//! ## 1. Estrategia de búsqueda: FTS5 primero, `LIKE` como red de seguridad
//!
//! Tabla `notes` columnas: `id, title, slug, filename, tags, word_count,
//! summary, created_at, updated_at`.
//!
//! La migración `20240101000006_notes_search_fts.sql` crea la tabla virtual
//! `notes_search_fts(title, content, tokenize='unicode61')` —un índice
//! **nuevo y exclusivo de este endpoint**— con triggers `AFTER INSERT/UPDATE/
//! DELETE` sobre `notes` (y uno sobre `note_versions`) que lo mantienen
//! sincronizado, más un *backfill* de las notas preexistentes. El contenido
//! indexado es: título (columna propia) + tags + cuerpo de la última versión
//! (`note_versions.content`), con fallback a `notes.summary`.
//!
//! No se reutiliza `notes_fts` (migración `20240101000002_knowledge.sql`)
//! porque esa tabla la mantiene el servicio `knowledge` a mano con el cuerpo
//! real de cada nota: un trigger que escribiera en ella haría fallar el
//! `INSERT` del servicio con *constraint failed* (rowid duplicado en FTS5) y
//! dejaría el cuerpo real fuera del índice. `notes_fts` queda intacta.
//!
//! ### 1.2 Saneado de la query (el `MATCH` nunca debe fallar)
//!
//! El texto del usuario se convierte en una expresión FTS5 con cada término
//! entre comillas dobles (las comillas internas se duplican) y el comodín de
//! prefijo `*`: `"algebra"*`. Así los operadores de FTS5 (`AND`, `OR`, `NOT`,
//! `-`, `(`, `)`, `:`, `^`, `"`) que el usuario escriba se tratan como texto
//! literal y **nunca producen un parse-error de `MATCH`**.
//!
//! ### 1.3 Red de seguridad
//!
//! `run_search` intenta FTS5 y, si la tabla no existe en esta base, si la
//! consulta FTS falla o si FTS no devuelve nada, cae al `LIKE` sobre `notes`
//! (`title`, `slug`, `tags`) que siempre ha funcionado. Un fallo de FTS5
//! **nunca** produce un 500 por su cuenta.
//!
//! ## 2. Enganche del top-bar (orquestador — Agente C1)
//!
//! El input de búsqueda global del *top-bar* pertenece al orquestador (Agente C1), que
//! adjunta el JS cliente. El *markup* del input live en:
//! - `static/stitch-v2/shell/topbar.html` (incluido como `STITCH_V2_TOPBAR` en
//!   `src/handlers/stitch_v2.rs`). Es un `<input type="text" placeholder="Buscar notas,
//!   tareas, temas... (Ctrl+K)">` sin `id` ni JS asociado: el orquestador C1 es quien lo
//!   engancha.
//! - (UI legacy) `static/studyos/index.html` línea 53: `<input id="globalSearch"
//!   type="search" placeholder="Buscar notas y tareas…">`.
//!
//! **Contrato que este handler cumple para C1:**
//! - URL: `GET /api/search?q=<query>` (query param `q`, opcional).
//! - Auth: protegido por la misma capa `require_auth_state` que `/dashboard/*` y
//!   `/app/*` (registrado en el router `protected` de `handlers/mod.rs`), por lo que el
//!   top-bar sólo dispara la petición cuando el usuario está autenticado.
//! - Respuesta 200: cuerpo JSON `{ "query": "<str>", "count": <usize>,
//!   "results": [ { "slug": <str>, "title": <str>, "tags": <str> } ] }`.
//! - Campos: `slug` (clave de ruta → `/app/notes/{slug}`), `title` (texto visible),
//!   `tags` (string comma-separado como viene de la tabla).
//! - Límite: 50 resultados (`LIMIT`) para acotar el payload.
//! - Query vacío/whitespace: respuesta 200 con `count: 0` y `results: []` (estado vacío
//!   honesto, sin DB round-trip).
//! - Query demasiado larga (>200): 400 `{ "error": "Query too long" }`.
//! - Error de BD inesperado: 500 `{ "error": "Search unavailable" }`.
//!
//! **JS esperado de C1 (responsabilidad del orquestador, no de este handler):**
//! debounce en el input → `fetch('/api/search?q=' + encodeURIComponent(q))` → leer
//! JSON → mapear `results` a filas `<a href="/app/notes/{slug}">{title}</a>`; si
//! `count === 0` mostrar "No se encontraron notas para «q»." (estado vacío honesto).
//! La página servidor `/dashboard/search` (handler `search_page`) es un *fallback* que
//! ya renderiza esto sin JS, compartiendo el *shell* STITCH.
//!
//! **Sin JS (esta página):** el input del top-bar de `/dashboard/search` se
//! convierte en un `<form method="get" action="/dashboard/search">` cuyo input se
//! llama `q` (ver [`inject_topbar_search_form`]): al pulsar Enter se llega a esta
//! misma página con `?q=…`, sin depender del JS de C1.
//!
//! ## 3. La página `/dashboard/search` (HTML, sin JS)
//!
//! `search_page` renderiza el mismo resultado que el API pero en cards, dentro del
//! **shell Stitch v2 compartido** (`crate::handlers::stitch_v2::stitch_v2_page` +
//! `crate::handlers::stitch_panel::fill_shell`), como el resto de páginas internas
//! (rail lateral con "Búsqueda global" activo, top-bar, contenido en `<main>`):
//! - **Título** enlazado a `/app/notes/{slug}`.
//! - **Snippet** del cuerpo: se lee el `.md` de la bóveda (`notes.filename` en
//!   `config.notes_dir`), se quita el frontmatter (`knowledge::frontmatter::parse_note`)
//!   y se resume con `knowledge::frontmatter::summarize` (Markdown limpio, primera
//!   frase, truncado UTF-8 seguro a [`SNIPPET_CHARS`] caracteres). Si el archivo no
//!   está en disco se recurre al `summary` de la tabla `notes`.
//! - **Chips** de etiquetas, enlazados al filtro `/app/notes?tag=<slug>`.
//!
//! La cabecera de la página lleva el formulario de búsqueda (GET a esta misma ruta)
//! y el input del top-bar apunta a la misma ruta con `?q=` (sin JS).
//!
//! **Sin resaltado de coincidencias**: el texto de la query y el de los resultados
//! se escapan con `html_escape` y se emiten tal cual. No se inyecta HTML del
//! usuario en la página (evita XSS por el `q` de la URL).
//!
//! Estados honestos (nunca un 500):
//! - Query vacía → hint "Escribe algo para buscar" (no se toca la BD).
//! - Sin resultados → `academic::templates::empty_state_html_icon` con el icono
//!   `search_off` (cero resultados es un estado normal, no un error).
//! - Error de BD → `empty_state_html_icon` con icono y mensaje distintos (se dice
//!   que la búsqueda falló; no se disfraza de "sin resultados").
//!
//! Ver también `docs/ARCHITECTURE.md` → F-03: Búsqueda local:
//! `Browser → (HTTP GET /api/search?q=...) → Axum → (SQLite query en notes) → JSON → Browser`.

use axum::{
    body::Body,
    extract::{Query, Request, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use std::sync::Arc;
use tracing::warn;

use crate::academic::templates::{empty_state_html_icon, html_escape};
use crate::AppState;
use std::path::Path;

/// Límite defensivo de longitud de query (400 si se supera). Coincide con
/// `office::library::MAX_QUERY_LEN`.
const MAX_QUERY_LEN: usize = 200;

/// Límite máximo de resultados devueltos (acota el payload).
const MAX_RESULTS: usize = 50;

/// Longitud máxima (en caracteres) del snippet que se muestra en cada card de
/// `/dashboard/search`. El truncado nunca parte un `char` UTF-8 por la mitad.
const SNIPPET_CHARS: usize = 240;

/// Nombre de la tabla virtual FTS5 con el índice de búsqueda web
/// (migración `20240101000006_notes_search_fts.sql`).
const FTS_TABLE: &str = "notes_search_fts";

/// SQL de búsqueda full-text sobre el índice FTS5 `notes_search_fts`.
///
/// El `MATCH` se escribe como `f.notes_search_fts MATCH ?` porque FTS5 sólo
/// expone la columna oculta de la tabla virtual cuando la tabla lleva alias
/// (con `f MATCH ?` el parser da "no such column: f"). El `JOIN` devuelve a
/// `notes` para proyectar `slug`/`title`/`tags` (y `filename`/`summary`, que
/// sólo usa la página HTML) y ordenar por `updated_at`.
const FTS_SEARCH_SQL: &str = r#"
    SELECT n.slug, n.title, n.tags, n.filename, n.summary
    FROM notes_search_fts f
    JOIN notes n ON n.id = f.rowid
    WHERE f.notes_search_fts MATCH ?
    ORDER BY n.updated_at DESC
    LIMIT ?
"#;

/// SQL de búsqueda *fallback* sobre la tabla `notes`. `LIKE ... ESCAPE '\'` con
/// el patrón `%{q}%` permite coincidencia parcial en `title`, `slug` y `tags`.
/// El orden `updated_at DESC` muestra primero las notas más recientes.
const SEARCH_SQL: &str = r#"
    SELECT slug, title, tags, filename, summary FROM notes
    WHERE title LIKE ? ESCAPE '\'
       OR slug  LIKE ? ESCAPE '\'
       OR tags  LIKE ? ESCAPE '\'
    ORDER BY updated_at DESC
    LIMIT ?
"#;

/// Query string deserializada de `GET /api/search?q=...`.
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

/// Un resultado de búsqueda proyectado desde `notes`.
#[derive(Debug, FromRow, Serialize)]
pub struct SearchHit {
    pub slug: String,
    pub title: String,
    pub tags: String,
    /// Nombre del archivo `.md` de la nota en la bóveda (`notes.filename`).
    /// Sólo lo usa la página `/dashboard/search` para leer el cuerpo y armar
    /// el snippet; **no** se serializa: el contrato JSON del API sigue siendo
    /// `{slug, title, tags}`.
    #[serde(skip_serializing)]
    pub filename: String,
    /// Resumen guardado en la BD (`notes.summary`, nullable). Fallback del
    /// snippet cuando el `.md` no está en disco. Tampoco se serializa.
    #[serde(skip_serializing)]
    pub summary: Option<String>,
}

/// Envolvente JSON de la respuesta del API.
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub count: usize,
    pub results: Vec<SearchHit>,
}

/// Comprueba si la tabla virtual FTS5 `notes_search_fts` existe en esta base.
///
/// Si la migración `20240101000006_notes_search_fts.sql` todavía no se ha
/// aplicado (base antigua), devuelve `false` y el handler usa el `LIKE`.
/// Un error al consultar `sqlite_master` también se traduce en `false`: la
/// búsqueda nunca debe romperse por culpa del índice.
async fn fts_table_exists(pool: &sqlx::SqlitePool) -> bool {
    let name: Option<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(FTS_TABLE)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    name.as_deref() == Some(FTS_TABLE)
}

/// Convierte la query del usuario en una expresión FTS5 segura.
///
/// Cada término va entre comillas dobles —las comillas internas se duplican
/// para escaparlas— y seguido del comodín de prefijo `*`: `"algebra"*`. Con
/// esto los operadores de FTS5 que el usuario pueda escribir (`AND`, `OR`,
/// `NOT`, `-`, `(`, `)`, `:`, `^`, `"`) se interpretan como texto literal y la
/// expresión **nunca** provoca un parse-error de `MATCH`.
///
/// Devuelve `None` si la query no tiene términos utilizables.
fn fts_match_expr(q: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    for raw in q.split_whitespace() {
        // Escapado de comillas dobles: en FTS5 se duplican.
        let term = raw.replace('"', "\"\"");
        if term.is_empty() {
            continue;
        }
        terms.push(format!("\"{}\"*", term));
    }

    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

/// Búsqueda full-text sobre `notes_search_fts` (título + tags + cuerpo).
async fn run_fts_search(pool: &sqlx::SqlitePool, q: &str) -> Result<Vec<SearchHit>, sqlx::Error> {
    let expr = match fts_match_expr(q) {
        Some(expr) => expr,
        // Query sin términos indexables (p. ej. sólo signos): sin FTS.
        None => return Ok(Vec::new()),
    };

    sqlx::query_as::<_, SearchHit>(FTS_SEARCH_SQL)
        .bind(expr)
        .bind(MAX_RESULTS as i64)
        .fetch_all(pool)
        .await
}

/// Búsqueda `LIKE` de respaldo sobre `notes` (title + slug + tags).
async fn run_like_search(pool: &sqlx::SqlitePool, q: &str) -> Result<Vec<SearchHit>, sqlx::Error> {
    // Envuelve en %...% y escapa comodines LIKE del input del usuario.
    let pattern = format!("%{}%", escape_like(q));

    sqlx::query_as::<_, SearchHit>(SEARCH_SQL)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(MAX_RESULTS as i64)
        .fetch_all(pool)
        .await
}

/// Ejecuta la búsqueda: FTS5 primero, `LIKE` como red de seguridad.
///
/// - Query vacío/whitespace → `Ok(Vec::new())` sin tocar la BD.
/// - Si la tabla FTS5 no existe en esta base → `LIKE` directamente.
/// - Si la consulta FTS5 falla → se registra y se cae al `LIKE` (nunca 500).
/// - Si FTS5 no devuelve resultados → `LIKE` (cubre subcadenas que el
///   tokenizador no puede indexar, p. ej. `%gebra%`).
/// - El `LIKE` escapa los comodines `%`/`_`/`\` del input del usuario.
async fn run_search(pool: &sqlx::SqlitePool, q: &str) -> Result<Vec<SearchHit>, sqlx::Error> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }

    if fts_table_exists(pool).await {
        match run_fts_search(pool, q).await {
            Ok(hits) if !hits.is_empty() => return Ok(hits),
            Ok(_) => {
                // Sin coincidencias en FTS: se prueba el LIKE (subcadenas).
            }
            Err(e) => {
                warn!("search: FTS5 falló, se usa el fallback LIKE: {}", e);
            }
        }
    }

    run_like_search(pool, q).await
}

/// `GET /api/search?q=<query>` → JSON con notas que coinciden en título, tags o slug.
///
/// Ver [`SearchQuery`] y la doc del módulo para el contrato completo con el
/// orquestador C1 (top-bar).
pub async fn search_api(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
) -> Response {
    let query = q.q.unwrap_or_default();

    // 400 si el query supera el límite defensivo.
    if query.len() > MAX_QUERY_LEN {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Query too long" })),
        )
            .into_response();
    }

    // Estado vacío honesto: query vacío → 200 sin tocar la BD.
    if query.trim().is_empty() {
        return (
            StatusCode::OK,
            Json(SearchResponse {
                query: String::new(),
                count: 0,
                results: Vec::new(),
            }),
        )
            .into_response();
    }

    match run_search(&state.db, &query).await {
        Ok(results) => {
            let count = results.len();
            (
                StatusCode::OK,
                Json(SearchResponse {
                    query: query.trim().to_string(),
                    count,
                    results,
                }),
            )
                .into_response()
        }
        Err(e) => {
            warn!("search_api: query failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Search unavailable" })),
            )
                .into_response()
        }
    }
}

/// `GET /dashboard/search?q=<query>` → página HTML de resultados (sin JS).
///
/// Se renderiza dentro del **shell Stitch v2 compartido**
/// (`stitch_v2::stitch_v2_page` + `stitch_panel::fill_shell`), igual que el
/// resto de páginas internas: rail lateral con "Búsqueda global" marcado como
/// item activo, top-bar y contenido dentro de `<main>`.
///
/// El input de búsqueda del top-bar se convierte en un formulario GET a esta
/// misma ruta (`?q=…`), de modo que la página funciona sin JS.
///
/// Cada resultado es una card con el título enlazado a `/app/notes/{slug}`, un
/// snippet del cuerpo y los chips de sus etiquetas.
///
/// Estados honestos, nunca un 500:
/// - query vacía → hint "Escribe algo para buscar";
/// - sin resultados → `empty_state_html_icon` con el icono `search_off`;
/// - error de BD → `empty_state_html_icon` (icono y mensaje distintos).
pub async fn search_page(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
    req: Request<Body>,
) -> Response {
    let raw = q.q.unwrap_or_default();
    let query = raw.trim().to_string();

    // Estado vacío honesto #1: sin query no hay nada que buscar (no se toca la BD).
    let (body, count) = if query.is_empty() {
        (render_empty_hint(), None)
    } else {
        // Un fallo de BD no es un "sin resultados": se distinguen los dos casos.
        let (results, fallo) = match run_search(&state.db, &query).await {
            Ok(r) => (r, false),
            Err(e) => {
                warn!("search_page: query failed: {}", e);
                (Vec::new(), true)
            }
        };

        // URL de reintento (misma query) para el botón del estado vacío.
        let retry_url = format!("/dashboard/search?q={}", url_encode(&query));

        if fallo {
            (
                format!(
                    "{vacio}{script}",
                    vacio = empty_state_html_icon(
                        "La búsqueda no está disponible en este momento. Reintenta en unos segundos.",
                        "cloud_off",
                        &retry_url,
                    ),
                    script = EMPTY_STATE_RETRY_SCRIPT,
                ),
                None,
            )
        } else if results.is_empty() {
            // Icono `search_off` (no `error_outline`): cero resultados es un
            // estado honesto y normal, no un fallo de la búsqueda.
            (
                format!(
                    "{vacio}{script}",
                    vacio = empty_state_html_icon(
                        &format!("No se encontraron notas para «{}».", query),
                        "search_off",
                        &retry_url,
                    ),
                    script = EMPTY_STATE_RETRY_SCRIPT,
                ),
                None,
            )
        } else {
            let cards = results
                .iter()
                .map(|h| render_result_card(h, &state.config.notes_dir))
                .collect::<Vec<_>>()
                .join("");
            (
                format!(
                    r#"<ul class="nova-search-results">{cards}</ul>"#,
                    cards = cards,
                ),
                Some(results.len()),
            )
        }
    };

    let content = render_search_content(&query, count, &body);
    let authenticated = state.config.auth_disabled
        || crate::middleware::auth::extract_session_cookie(&req).is_some();
    let html =
        crate::handlers::stitch_v2::stitch_v2_page("busqueda-global", &content, authenticated);
    let shell = crate::handlers::stitch_panel::fetch_shell_data(&state).await;
    let html = crate::handlers::stitch_panel::fill_shell(&html, &shell);
    let html = inject_page_title(&html, "Buscar — Study Platform");
    let html = inject_topbar_search_form(&html, &query);
    (StatusCode::OK, Html(html)).into_response()
}

/// Contenido de la página: cabecera + formulario de búsqueda + cuerpo.
///
/// `count` es el número de resultados (o `None` si la búsqueda no se ejecutó
/// o no devolvió nada): en ese caso no se inventa un contador.
fn render_search_content(query: &str, count: Option<usize>, body: &str) -> String {
    let badge = match count {
        Some(n) => format!(r#"<span class="nova-version-badge">{n} resultado(s)</span>"#),
        None => String::new(),
    };

    format!(
        r#"<div class="nova-page nova-search-page">
            <header class="nova-search-hero"><p class="nova-eyebrow"><span>05</span> Conocimiento</p><h1>¿Qué quieres<br><em>volver a encontrar?</em></h1><p class="nova-page-lead">Busca por título, etiqueta o contenido dentro de tus notas locales.</p>{badge}</header>
            {form}
            <section class="nova-search-body" aria-label="Resultados de búsqueda">{body}</section>
        </div>"#,
        badge = badge,
        form = render_search_form(query),
        body = body,
    )
}

/// Estado vacío honesto #1: la página sin query (no se toca la BD).
fn render_empty_hint() -> String {
    r#"<div class="nova-search-empty"><span class="material-symbols-outlined">travel_explore</span><h2>Tu biblioteca está lista.</h2><p>Escribe algo para buscar en tus notas.</p></div>"#
        .to_string()
}

/// Inserta el `<title>` de la página en el `<head>` del shell.
///
/// El shell Stitch v2 es común a todas las rutas y no lleva título: sin esto
/// la pestaña del navegador mostraría la URL.
fn inject_page_title(html: &str, title: &str) -> String {
    let Some(pos) = html.find("<head>") else {
        return html.to_string();
    };
    let tag = format!("<title>{}</title>", html_escape(title));
    format!("{}{}{}", &html[..pos], tag, &html[pos..])
}

/// Ancla del input de búsqueda del top-bar del shell Stitch v2.
///
/// Es única en el documento: el resto de inputs de la página (el del formulario
/// principal) tienen otro `placeholder`.
const TOPBAR_INPUT_ANCHOR: &str = "placeholder=\"Buscar notas, tareas, temas";

/// Convierte el input de búsqueda del top-bar en un formulario GET a
/// `/dashboard/search` (sin JS: pulsar Enter ya devuelve resultados).
///
/// El top-bar es compartido por todas las páginas, así que la conversión se
/// aplica **sólo** al HTML de esta página: el input se envuelve en un
/// `<form>` con `display: contents` (no altera la maqueta) y gana los
/// atributos `name="q"` y `value`.
fn inject_topbar_search_form(html: &str, query: &str) -> String {
    let Some(anchor) = html.find(TOPBAR_INPUT_ANCHOR) else {
        return html.to_string();
    };
    let Some(start) = html[..anchor].rfind("<input") else {
        return html.to_string();
    };
    let Some(rel_end) = html[anchor..].find('>') else {
        return html.to_string();
    };
    let end = anchor + rel_end;
    // `type="text"` → `type="search"`: los navegadores añaden el botón de
    // limpiar sin cambiar el estilo del campo.
    let tag = html[start..=end].replace("type=\"text\"", "type=\"search\"");

    // Los atributos nuevos van justo antes del cierre de la etiqueta.
    let insert_at = if tag.ends_with("/>") {
        tag.len() - 2
    } else {
        tag.len() - 1
    };
    let input = format!(
        "{}{}{}",
        &tag[..insert_at],
        format!(" name=\"q\" value=\"{}\"", html_escape(query)),
        &tag[insert_at..]
    );

    let form = format!(
        r#"<form method="get" action="/dashboard/search" class="contents">{input}<button type="submit" class="sr-only">Buscar</button></form>"#,
        input = input
    );

    format!("{}{}{}", &html[..start], form, &html[end + 1..])
}

/// Script que hace funcional el botón "Reintentar" del estado vacío.
///
/// `empty_state_html` genera un botón con atributos `hx-*` de htmx, pero el
/// shell Stitch v2 no carga htmx: sin este script el botón sería inerte. Se
/// quitan los atributos `hx-*` y el botón recarga la página (misma query).
const EMPTY_STATE_RETRY_SCRIPT: &str = r#"<script>
(function(){
  var b = document.querySelector('#empty-state button[hx-get]');
  if(!b) return;
  b.removeAttribute('hx-get');
  b.removeAttribute('hx-target');
  b.removeAttribute('hx-swap');
  b.addEventListener('click', function(){ location.reload(); });
})();
</script>"#;

/// Formulario de búsqueda de la página (GET, sin JS).
///
/// El valor de la query va escapado: un `q` con comillas no puede romper el
/// atributo `value` ni inyectar markup.
fn render_search_form(query: &str) -> String {
    format!(
        r#"<form method="get" action="/dashboard/search" class="nova-search-form">
            <span class="material-symbols-outlined">search</span>
            <label for="search-input" class="sr-only">Buscar notas</label>
            <input id="search-input" type="search" name="q" value="{q}" placeholder="Escribe una palabra, una etiqueta o una idea…" autocomplete="off" />
            <button type="submit" class="nova-button nova-button-primary">Buscar<span class="material-symbols-outlined">arrow_forward</span></button>
        </form>"#,
        q = html_escape(query),
    )
}

/// Card de un resultado: título enlazado, snippet del cuerpo y chips de tags.
///
/// Todo el texto pasa por `html_escape` y **no** hay resaltado de coincidencias
/// (inyectar el `q` del usuario dentro del HTML abriría la puerta a XSS).
fn render_result_card(hit: &SearchHit, notes_dir: &Path) -> String {
    let snippet = note_snippet(notes_dir, &hit.filename, hit.summary.as_deref());
    let snippet_html = if snippet.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="mt-1 font-body-sm text-body-sm text-on-surface-variant">{snippet}</p>"#,
            snippet = html_escape(&snippet),
        )
    };

    let chips = render_tag_chips(&hit.tags);
    let chips_html = if chips.is_empty() {
        String::new()
    } else {
        format!(
            r#"<div class="mt-2 flex flex-wrap gap-1.5">{chips}</div>"#,
            chips = chips
        )
    };

    format!(
        r#"<li class="nova-search-result">
                <span class="nova-search-result-icon"><span class="material-symbols-outlined">description</span></span>
                <div><a href="/app/notes/{slug}">{title}</a>{snippet}{chips}</div>
                <a href="/app/notes/{slug}" class="nova-icon-button" aria-label="Abrir {title}"><span class="material-symbols-outlined">arrow_outward</span></a>
            </li>"#,
        slug = html_escape(&hit.slug),
        title = html_escape(&hit.title),
        snippet = snippet_html,
        chips = chips_html,
    )
}

/// Snippet de una nota para la card de resultados.
///
/// Fuente preferida: el cuerpo real del `.md` en disco, resumido con
/// `knowledge::frontmatter::summarize` (limpia el Markdown inline, se queda con
/// la primera frase y trunca a [`SNIPPET_CHARS`] caracteres sin partir un
/// `char` UTF-8). Si el archivo no está en disco se recurre al `summary` de la
/// tabla `notes`, con el mismo truncado.
fn note_snippet(notes_dir: &Path, filename: &str, summary: Option<&str>) -> String {
    if let Some(body) = read_note_body(notes_dir, filename) {
        if !body.trim().is_empty() {
            let snippet = crate::knowledge::frontmatter::summarize(&body, SNIPPET_CHARS);
            if !snippet.trim().is_empty() {
                return snippet;
            }
        }
    }
    summary
        .map(|s| truncate_utf8(s.trim(), SNIPPET_CHARS))
        .unwrap_or_default()
}

/// Cuerpo de una nota leído de su `.md` en la bóveda (sin frontmatter).
///
/// Devuelve `None` si el archivo no existe o no se puede leer (en ese caso la
/// página cae al `summary` de la BD).
fn read_note_body(notes_dir: &Path, filename: &str) -> Option<String> {
    let raw = std::fs::read_to_string(notes_dir.join(filename)).ok()?;
    Some(crate::knowledge::frontmatter::parse_note(&raw).content)
}

/// Trunca a `max_chars` caracteres sin partir un `char` UTF-8 por la mitad
/// (mismo criterio que `frontmatter::truncate_chars`, que no es público).
fn truncate_utf8(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars - 1).collect();
    format!("{}…", head)
}

/// Chips de etiquetas de una nota: cada uno enlaza al filtro de notas por
/// etiqueta (`/app/notes?tag=<slug>`).
fn render_tag_chips(tags: &str) -> String {
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
            r#"<a href="/app/notes?tag={slug}" class="px-2 py-0.5 rounded-full bg-primary-container/15 text-primary-container font-label-sm hover:bg-primary-container/25 transition-colors">#{label}</a>"#,
            slug = html_escape(&slug),
            label = html_escape(label),
        ));
    }
    out
}

/// Slug de una etiqueta para el filtro `/app/notes?tag=…`.
///
/// Misma normalización que `handlers::notes::tag_slug` (que es privada y no
/// se puede importar desde aquí): minúsculas, acentos plegados a ASCII y
/// guiones entre palabras.
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

/// Percent-encoda un valor para usarlo en un query string (`?q=…`).
///
/// Sólo deja fuera los caracteres unreserved de RFC 3986; el resto se
/// escapa como `%XX`, así que el resultado es seguro dentro de un atributo
/// HTML.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Escape de caracteres especiales de SQLite `LIKE` (`%`, `_`, `\`) para que el
/// input del usuario se case literalmente. Se usa con `ESCAPE '\'` en el SQL.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => {
                out.push('\\');
                out.push('\\');
            }
            '%' => {
                out.push('\\');
                out.push('%');
            }
            '_' => {
                out.push('\\');
                out.push('_');
            }
            other => out.push(other),
        }
    }
    out
}
