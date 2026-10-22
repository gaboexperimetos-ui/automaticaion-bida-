//! Integridad del shell HTML de `/dashboard` (kit Stitch v2).
//!
//! Harness calcado de `tests/admin_routes.rs`: SQLite temporal con las
//! migraciones reales, usuario sembrado y sesión válida en la tabla
//! `sessions` (hash `SHA256(secret:token)`), `mikareno: None` y MCP apuntando
//! a un puerto muerto (sondeo hermético, sin red). Nada se escribe fuera del
//! directorio temporal.
//!
//! Qué se verifica sobre el HTML servido por `GET /dashboard`:
//! 1. La página responde **200**.
//! 2. **No queda ningún slot sin rellenar**: el shell (`static/stitch-v2/shell/*`)
//!    y el contenido (`static/stitch-v2/_content/clean/dashboard.html`) traen
//!    marcadores `<!--SLOT:...-->` que `stitch_panel::fill_panel` (contenido)
//!    y `stitch_panel::fill_shell` (shell) sustituyen por datos reales. Si el
//!    HTML servido todavía contiene un `<!--SLOT:` es un bug: el marcador
//!    llegó al navegador.
//! 3. `aria-current="page"` aparece **exactamente una vez** (el item activo
//!    del nav-rail). `update_nav_rail` quita el atributo a todos los items y
//!    se lo reinserta sólo al activo; dos marcas = dos items "página actual".
//! 4. Los **21 items `data-path`** del nav-rail apuntan a rutas **reales**:
//!    - lista estática `KNOWN_ROUTES` (grep de `src/handlers/mod.rs`,
//!      `src/academic/mod.rs`, `src/office/mod.rs`, `src/ai/mod.rs`,
//!      `src/handlers/admin.rs`, `src/mcp_server/mod.rs`);
//!    - sondeo real: cada `href` distinto se pide por GET con sesión y un
//!      **404 = enlace MUERTO** (ruta no registrada).
//! 5. El HTML está **balanceado**: `<div>`/`</div>` y `<section>`/`</section>`
//!    abren y cierran en la misma cantidad. Cuando no cuadra, el test dice
//!    **qué etiqueta** quedó sin cerrar.
//! 6. No aparecen literales de JavaScript sin renderizar: `undefined`,
//!    `null` ni `[object Object]`.
//!
//! Rate limiter: el middleware global permite 60 req/min por IP, así que
//! cada petición usa un `X-Forwarded-For` distinto.
//!
//! Coste: cada página que habla con Mikareno reintenta contra el puerto
//! muerto (~4 s por llamada). El estado de la app y el HTML de `/dashboard`
//! se comparten entre los tests (`OnceCell`) para no pagar el sondeo dos
//! veces.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use password_hash::PasswordHasher;
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::middleware::auth::{generate_session_token, hash_session_token};
use study_platform::AppState;

/// Usuario sembrado para el test autenticado.
const TEST_USER: &str = "admin";

/// Rutas HTML registradas en el router, sacadas por grep del código fuente.
///
/// `src/handlers/mod.rs` → router `public` y router `protected`.
/// `src/academic/mod.rs` → anidadas bajo `/dashboard` (`.nest("/dashboard")`).
/// `src/office/mod.rs`, `src/ai/mod.rs`, `src/handlers/admin.rs`,
/// `src/mcp_server/mod.rs` → resto del router.
const KNOWN_ROUTES: &[&str] = &[
    // src/handlers/mod.rs — router `public`
    "/",
    "/dashboard",
    "/dashboard/ai",
    "/dashboard/notes",
    "/dashboard/mcp",
    "/login",
    "/register",
    "/logout",
    "/health",
    "/api/config",
    "/api/public-config",
    "/handoff",
    "/legacy",
    "/styles.css",
    "/app.js",
    // src/handlers/mod.rs — router `protected`
    "/dashboard/calendar.ics",
    "/app/notes",
    "/app/notes/new",
    "/app/notes/{slug}",
    "/app/notes/{slug}/edit",
    "/app/notes/{slug}/update",
    "/app/notes/{slug}/delete",
    "/dashboard/material",
    "/api/notes",
    "/api/notes/{slug}",
    "/api/search",
    "/dashboard/search",
    "/api/admin/metrics",
    "/api/admin/backup",
    "/api/admin/backup/restore",
    "/api/admin/backup/download/{name}",
    "/api/admin/backup-status",
    "/settings",
    "/api/settings/ai",
    // src/academic/mod.rs — anidadas bajo `/dashboard`
    "/dashboard/calendar",
    "/dashboard/calendar/daily",
    "/dashboard/horario",
    "/dashboard/planner",
    "/dashboard/profile",
    "/dashboard/subjects",
    "/dashboard/subjects/{code}",
    "/dashboard/examenes",
    "/dashboard/vida-escolar",
    // src/office/mod.rs
    "/office",
    "/office/docs",
    "/office/docs/new",
    "/office/docs/{slug}",
    "/office/docs/{slug}/preview",
    "/office/docs/{slug}/export",
    "/office/sheets",
    "/office/sheets/new",
    "/office/sheets/{slug}",
    "/office/sheets/{slug}/import",
    "/office/sheets/{slug}/export",
    "/office/sheets/{slug}/delete",
    "/office/presentations",
    "/office/presentations/new",
    "/office/presentations/{slug}",
    "/office/presentations/{slug}/present",
    "/office/presentations/{slug}/slides",
    "/office/presentations/{slug}/slides/{index}",
    "/office/presentations/{slug}/slides/reorder",
    "/office/library",
    "/office/library/search",
    "/office/library/import/{type}",
    "/office/library/{type}/{name}/download",
    "/office/library/{type}/{name}/delete",
    "/office/report",
    // src/ai/mod.rs
    "/api/ai/chat",
    "/api/ai/chat/stream",
    "/api/ai/summary",
    "/api/ai/quiz",
    "/api/ai/study-sheet",
    "/api/ai/cost",
    "/api/ai/conversations",
    "/api/ai/conversations/{subject}/messages",
    "/ai",
    // src/handlers/admin.rs
    "/admin",
    "/admin/metrics",
    "/admin/openrouter/budget",
    "/admin/backup/list",
    // src/mcp_server/mod.rs
    "/mcp",
    "/mcp/sse",
];

/// Los 21 items `data-path` del nav-rail (kit Stitch v2, `shell/aside.html`).
const EXPECTED_NAV_PATHS: &[&str] = &[
    "administracion",
    "asignaturas",
    "asistente-academico",
    "biblioteca-de-archivos",
    "busqueda-global",
    "centro-mcp",
    "configuracion",
    "conversaciones",
    "documentos",
    "estado-de-conexiones",
    "examenes",
    "generador-de-material",
    "hojas-de-calculo",
    "horarios",
    "mi-agenda",
    "mis-notas",
    "panel-principal",
    "planificacion",
    "presentaciones",
    "segundo-cerebro",
    "tareas-y-actividades",
    "vida-escolar",
];

/// Test configuration: sin API key (modo simulado), auth activo, MCP en un
/// puerto muerto para que el sondeo sea hermético.
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18095,
        session_secret: "shell_integrity_test_secret_at_least_32chars!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: None,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://127.0.0.1:57777/mcp".to_string(), // puerto muerto: probe hermético
        genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        registration_token: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
    }
}

/// Handle que mantiene vivos los temporales durante el test.
struct TestApp {
    state: Arc<AppState>,
    cookie: String,
    // Keeps the temp DB file (and its dir) alive until the test ends.
    _keep: (tempfile::TempDir, tempfile::NamedTempFile),
}

/// Prepara el estado real de la app: migraciones, usuario y sesión válida.
async fn setup_app() -> TestApp {
    let notes_tmp = tempfile::tempdir().unwrap();
    let notes_dir = notes_tmp.path().to_path_buf();
    let db_tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = db_tmp.path().to_path_buf();
    let db_url = format!("sqlite://{}", db_path.display());

    let config = test_config(notes_dir, db_path);
    let secret = config.session_secret.clone();

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .expect("Failed to create test database");

    sqlx::query("PRAGMA journal_mode=WAL;")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("PRAGMA foreign_keys=ON;")
        .execute(&pool)
        .await
        .ok();
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    // Usuario con contraseña conocida (misma receta que admin_routes.rs).
    let salt = argon2::password_hash::SaltString::generate(rand::thread_rng());
    let argon2 = argon2::Argon2::default();
    let password_hash = argon2
        .hash_password(b"testpassword123", &salt)
        .unwrap()
        .to_string();
    sqlx::query(
        "INSERT INTO users (username, password_hash) VALUES (?, ?) \
         ON CONFLICT(username) DO UPDATE SET password_hash = EXCLUDED.password_hash",
    )
    .bind(TEST_USER)
    .bind(&password_hash)
    .execute(&pool)
    .await
    .expect("Failed to seed user");
    let uid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(TEST_USER)
        .fetch_one(&pool)
        .await
        .expect("user id");

    // Sesión en BD que el middleware acepta: se guarda el hash del token y la
    // cookie lleva el token en crudo (igual que un login real).
    let token = generate_session_token();
    let hashed = hash_session_token(&token, &secret);
    let expires = chrono::Utc::now() + chrono::Duration::hours(24);
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(&hashed)
        .bind(uid)
        .bind(expires.to_rfc3339())
        .execute(&pool)
        .await
        .expect("Failed to seed session");

    let state = Arc::new(AppState {
        cache_mikareno: std::sync::Arc::new(
            study_platform::academic::calendar_data::CacheMikareno::default(),
        ),
        cache_catalogo: std::sync::Arc::new(study_platform::ai::modelos::CacheCatalogo::default()),
        config: config.clone(),
        db: pool,
        mikareno: None,
        start_time: std::time::Instant::now(),
    });

    TestApp {
        state,
        cookie: format!("session_id={token}"),
        _keep: (notes_tmp, db_tmp),
    }
}

/// Estado compartido: una sola app (y un solo sondeo a Mikareno) para todos
/// los tests del archivo.
static APP: tokio::sync::OnceCell<TestApp> = tokio::sync::OnceCell::const_new();
/// HTML de `/dashboard` ya descargado (se reutiliza entre los tests).
static DASHBOARD_HTML: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

/// App compartida.
async fn app() -> &'static TestApp {
    APP.get_or_init(|| async { setup_app().await }).await
}

/// Router de la app compartida.
fn router(t: &TestApp) -> axum::Router {
    study_platform::handlers::router(t.state.clone())
}

/// GET con sesión y una IP propia (aisla el rate limiter por IP).
async fn get(app: axum::Router, uri: &str, cookie: &str, ip: &str) -> axum::response::Response {
    let req = Request::builder()
        .uri(uri)
        .header("cookie", cookie)
        .header("x-forwarded-for", ip)
        .body(Body::empty())
        .unwrap();
    app.oneshot(req).await.unwrap()
}

/// HTML servido por `GET /dashboard` con una sesión válida (200 obligatorio).
async fn dashboard_html() -> &'static str {
    DASHBOARD_HTML
        .get_or_init(|| async {
            let t = app().await;
            let resp = get(router(t), "/dashboard", &t.cookie, "10.77.0.1").await;
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "GET /dashboard con sesión debe responder 200"
            );
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            String::from_utf8(bytes.to_vec()).expect("el HTML del dashboard debe ser UTF-8")
        })
        .await
}

/// Valor de un atributo (`name="value"`) dentro de una etiqueta.
fn attr_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {}=\"", name);
    let start = tag.find(&needle)? + needle.len();
    let end = start + tag[start..].find('"')?;
    Some(&tag[start..end])
}

/// Items del nav-rail: pares `(data-path, href)` de cada `<a ...>` que lleva
/// `data-path`.
fn nav_items(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<a ") {
        let after = &rest[start..];
        let Some(end) = after.find('>') else { break };
        let tag = &after[..end];
        if let (Some(path), Some(href)) = (attr_value(tag, "data-path"), attr_value(tag, "href")) {
            out.push((path.to_string(), href.to_string()));
        }
        rest = &after[end..];
    }
    out
}

/// Enmascara el contenido de `<script>`/`<style>` (misma longitud) para que
/// sus `<` y `>` de JavaScript no se confundan con etiquetas HTML.
fn mask_code_blocks(html: &str) -> String {
    let mut out = html.to_string();
    for tag in ["script", "style"] {
        let mut search_from = 0usize;
        loop {
            let Some(open) = out[search_from..]
                .find(&format!("<{tag}"))
                .map(|p| search_from + p)
            else {
                break;
            };
            // La apertura debe terminar en `>` (no `<scriptfoo`).
            let Some(open_end) = out[open..].find('>').map(|p| open + p) else {
                break;
            };
            if !out[open + format!("<{tag}").len()..open_end]
                .chars()
                .all(|c| c.is_whitespace() || c == '/' || c.is_ascii_alphanumeric())
            {
                search_from = open + 1;
                continue;
            }
            let close = format!("</{tag}>");
            let Some(close_at) = out[open_end..].find(&close).map(|p| open_end + p) else {
                break;
            };
            let mut chars: Vec<char> = out[open_end + 1..close_at].chars().collect();
            for c in chars.iter_mut() {
                if *c != '\n' {
                    *c = ' ';
                }
            }
            let filler: String = chars.into_iter().collect();
            out.replace_range(open_end + 1..close_at, &filler);
            search_from = close_at + close.len();
        }
    }
    out
}

/// Nombre de la etiqueta de un fragmento `<...>` (`None` si no es una
/// etiqueta HTML válida).
fn tag_name(raw: &str) -> Option<&str> {
    let body = raw.strip_prefix('<')?.strip_suffix('>')?;
    let name = body
        .trim_start_matches('/')
        .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .next()?;
    if name.is_empty() {
        return None;
    }
    let mut chars = name.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some(name)
}

/// Etiquetas `<tag>` que quedan abiertas al final del HTML: pares
/// `(posición, texto de la etiqueta de apertura)`.
fn unclosed_tags(html: &str, tag: &str) -> Vec<(usize, String)> {
    let masked = mask_code_blocks(html);
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut idx = 0usize;
    while let Some(rel) = masked[idx..].find('<') {
        let start = idx + rel;
        let after = &masked[start + 1..];
        // Una etiqueta bien formada no contiene otro `<` antes de su `>`.
        if let Some(inner_lt) = after.find('<') {
            if after.find('>').map(|g| inner_lt < g).unwrap_or(true) {
                idx = start + 1 + inner_lt;
                continue;
            }
        }
        let Some(end_rel) = after.find('>') else {
            break;
        };
        let end = start + 1 + end_rel;
        let raw = &masked[start..=end];
        idx = end + 1;
        if raw.starts_with("<!--") {
            continue;
        }
        let Some(name) = tag_name(raw) else { continue };
        if !name.eq_ignore_ascii_case(tag) {
            continue;
        }
        if raw.starts_with("</") {
            if let Some(pos) = stack.iter().rposition(|(_, t)| {
                tag_name(t)
                    .map(|n| n.eq_ignore_ascii_case(tag))
                    .unwrap_or(false)
            }) {
                stack.remove(pos);
            }
        } else if !raw.ends_with("/>") {
            stack.push((start, raw.to_string()));
        }
    }
    stack
}

/// Cuenta aperturas de una etiqueta (`<div` seguido de delimitador) sin
/// contar cierres ni coincidencias como `<division>`.
fn count_open(html: &str, tag: &str) -> usize {
    let needle = format!("<{}", tag);
    let mut count = 0usize;
    let mut idx = 0usize;
    while let Some(pos) = html[idx..].find(&needle) {
        let at = idx + pos + needle.len();
        let next = html[at..].chars().next();
        if matches!(
            next,
            Some(' ') | Some('\t') | Some('\n') | Some('\r') | Some('>') | Some('/')
        ) {
            count += 1;
        }
        idx = at;
    }
    count
}

/// Cuenta cierres de una etiqueta (`</div>`).
fn count_close(html: &str, tag: &str) -> usize {
    html.matches(&format!("</{}>", tag)).count()
}

// --- 1. La página responde 200 y no deja slots sin rellenar ---

#[tokio::test]
async fn dashboard_200_y_sin_slots_sin_rellenar() {
    let html = dashboard_html().await;

    // Slots que sobrevivieron al render: bug real, se listan uno a uno.
    let mut slots: Vec<&str> = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("<!--SLOT:") {
        let tail = &rest[pos + "<!--SLOT:".len()..];
        let end = tail.find("-->").map(|e| e + 3).unwrap_or(0);
        slots.push(&tail[..end]);
        rest = &tail[end..];
    }

    if !slots.is_empty() {
        eprintln!(
            "BUG: el HTML servido por GET /dashboard contiene {} marcador(es) `<!--SLOT:...-->` sin rellenar:",
            slots.len()
        );
        for s in &slots {
            eprintln!("  - {s}");
        }
    }

    assert!(
        slots.is_empty(),
        "el shell de /dashboard se sirvió con {} slot(s) sin rellenar: {:?}",
        slots.len(),
        slots
    );
}

// --- 2. `aria-current="page"` exactamente una vez ---

#[tokio::test]
async fn dashboard_marca_un_solo_item_activo_con_aria_current() {
    let html = dashboard_html().await;

    let marca = "aria-current=\"page\"";
    let veces = html.matches(marca).count();

    if veces != 1 {
        // Localiza los items marcados para que el fallo sea accionable.
        for (path, href) in nav_items(html) {
            let tag_start = html.find(&format!("data-path=\"{}\"", path)).unwrap_or(0);
            let ini = tag_start.saturating_sub(400);
            let fin = (tag_start + 400).min(html.len());
            if html[ini..fin].contains(marca) {
                eprintln!("  item marcado: data-path=\"{path}\" href=\"{href}\"");
            }
        }
    }

    assert_eq!(
        veces,
        1,
        "aria-current=\"page\" debe aparecer exactamente 1 vez (el item activo del nav-rail), apareció {veces} veces"
    );
}

// --- 3. Los `data-path` apuntan a rutas reales ---

#[tokio::test]
async fn nav_data_paths_apuntan_a_rutas_registradas() {
    let t = app().await;
    let html = dashboard_html().await;

    let items = nav_items(html);
    assert_eq!(
        items.len(),
        EXPECTED_NAV_PATHS.len(),
        "el nav-rail debe tener {} items con data-path, tiene {}",
        EXPECTED_NAV_PATHS.len(),
        items.len()
    );

    // Conjunto de `data-path`: debe coincidir con la lista conocida.
    let vistos: BTreeSet<&str> = items.iter().map(|(p, _)| p.as_str()).collect();
    let esperados: BTreeSet<&str> = EXPECTED_NAV_PATHS.iter().copied().collect();
    let faltantes: Vec<&&str> = esperados.difference(&vistos).collect();
    let sobrantes: Vec<&&str> = vistos.difference(&esperados).collect();
    for f in &faltantes {
        eprintln!("  data-path ausente en el nav: {f}");
    }
    for s in &sobrantes {
        eprintln!("  data-path no esperado en el nav: {s}");
    }
    assert!(
        faltantes.is_empty() && sobrantes.is_empty(),
        "el conjunto de data-path del nav no coincide con el esperado (faltan {:?}, sobran {:?})",
        faltantes,
        sobrantes
    );

    // Rutas distintas a las que apuntan los items.
    let mut rutas: Vec<String> = items.iter().map(|(_, h)| h.clone()).collect();
    rutas.sort();
    rutas.dedup();

    // (a) Lista estática de rutas registradas.
    let desconocidas: Vec<&String> = rutas
        .iter()
        .filter(|r| !KNOWN_ROUTES.contains(&r.as_str()))
        .collect();
    for r in &desconocidas {
        eprintln!("ENLACE MUERTO (ruta no registrada en el router): {r}");
    }
    assert!(
        desconocidas.is_empty(),
        "hay hrefs del nav que apuntan a rutas inexistentes: {:?}",
        desconocidas
    );

    // (b) Sondeo real: un 404 significa que la ruta NO está registrada.
    let mut muertos: Vec<String> = Vec::new();
    let mut otros: Vec<String> = Vec::new();
    for (i, ruta) in rutas.iter().enumerate() {
        let ip = format!("10.78.{}.{}", i / 250, (i % 250) + 1);
        let resp = get(router(t), ruta, &t.cookie, &ip).await;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            eprintln!("ENLACE MUERTO (404 al pedirlo): {ruta}");
            muertos.push(ruta.clone());
        } else if status != StatusCode::OK {
            eprintln!("  ruta existente pero con estado no-200: {ruta} -> {status}");
            otros.push(format!("{ruta} -> {status}"));
        }
    }
    assert!(
        muertos.is_empty(),
        "enlaces MUERTOS en el nav-rail (404): {:?}",
        muertos
    );
    if !otros.is_empty() {
        eprintln!("  (rutas registradas que no devolvieron 200: {:?})", otros);
    }
}

// --- 4. HTML balanceado ---

#[tokio::test]
async fn dashboard_html_tiene_etiquetas_balanceadas() {
    let html = dashboard_html().await;

    for tag in ["div", "section"] {
        let abiertos = count_open(html, tag);
        let cerrados = count_close(html, tag);
        if abiertos != cerrados {
            eprintln!(
                "  desbalance: <{tag}> x{abiertos} vs </{tag}> x{cerrados} (diferencia {})",
                abiertos as i64 - cerrados as i64
            );
            // Qué etiqueta quedó sin cerrar (con su posición en el HTML).
            for (pos, raw) in unclosed_tags(html, tag) {
                eprintln!("  etiqueta <{tag}> sin cerrar en el byte {pos}: {raw}");
            }
        }
        assert_eq!(
            abiertos, cerrados,
            "etiquetas <{tag}> desbalanceadas: {abiertos} aperturas vs {cerrados} cierres"
        );
    }
}

// --- 5. Sin literales de JavaScript sin renderizar ---

#[tokio::test]
async fn dashboard_html_no_tiene_literals_js_sin_renderizar() {
    let html = dashboard_html().await;

    for literal in ["undefined", "null", "[object Object]"] {
        if html.contains(literal) {
            // Muestra el contexto para que el fallo apunte al origen.
            let mut pos = 0usize;
            let mut shown = 0;
            while let Some(p) = html[pos..].find(literal) {
                let at = pos + p;
                let ini = at.saturating_sub(60);
                let fin = (at + literal.len() + 60).min(html.len());
                eprintln!(
                    "  '{}' en {}: ...{}...",
                    literal,
                    at,
                    html[ini..fin].replace('\n', " ")
                );
                pos = at + literal.len();
                shown += 1;
                if shown >= 5 {
                    eprintln!("  (más ocurrencias omitidas)");
                    break;
                }
            }
        }
        assert!(
            !html.contains(literal),
            "el HTML de /dashboard contiene el literal `{}` (valor de JS sin renderizar)",
            literal
        );
    }
}
