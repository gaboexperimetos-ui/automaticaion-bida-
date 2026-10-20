//! Barrido de humo (smoke) de las páginas HTML registradas.
//!
//! Harness calcado de `tests/admin_routes.rs`: SQLite temporal con las
//! migraciones reales, usuario sembrado y sesión válida en la tabla
//! `sessions` (hash `SHA256(secret:token)`), `mikareno: None` y MCP apuntando
//! a un puerto muerto (sondeo hermético, sin red). Cada GET se hace **con
//! sesión** y debe devolver **200**; si alguna ruta devuelve otra cosa, el
//! fallo nombra la ruta exacta y su `content-type`.
//!
//! Lista de rutas verificada por grep (no inventada) en:
//! - `src/handlers/mod.rs`  → `/`, `/dashboard`, `/dashboard/ai`,
//!   `/dashboard/notes`, `/dashboard/mcp`, `/legacy`, `/app/notes`,
//!   `/app/notes/new`, `/dashboard/material`, `/dashboard/search`, `/settings`
//! - `src/academic/mod.rs`  → anidadas bajo `/dashboard`: `/calendar`,
//!   `/calendar/daily`, `/horario`, `/planner`, `/profile`, `/subjects`,
//!   `/subjects/{code}`, `/examenes`, `/vida-escolar`
//! - `src/handlers/admin.rs` → `/admin` (panel F9)
//! - `src/ai/mod.rs`, `src/office/mod.rs` → `/ai`, `/office`
//!
//! Notas sobre la lista:
//! - `/dashboard/settings` **no está registrado** (grep en `src/handlers/mod.rs`
//!   y `src/academic/mod.rs`: sólo existe `/settings`), así que no se barre:
//!   pedirlo daría 404, no 200.
//! - Rutas con parámetro: se usa un valor **real de la BD** (código de
//!   asignatura en `chat_history.subject`, slug en `notes.slug`). Si la BD no
//!   tiene ninguno, se hace `eprintln!` de salto y la ruta **no** se asserta.
//! - Rutas GET no-HTML (`/dashboard/calendar.ics`, `/api/*`) y las descargas
//!   (`/office/report`) quedan fuera: no son páginas.
//!
//! Rate limiter: el middleware global permite 60 req/min por IP. Cada
//! petición usa una `X-Forwarded-For` distinta para que el barrido no dependa
//! del presupuesto compartido de la IP "unknown".

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
const TEST_USER: &str = "smoke";

/// Slug/código sembrados por SQL para las rutas parametrizadas.
const NOTE_SLUG: &str = "nota-smoke";
const NOTE_TITLE: &str = "Nota de humo";
const SUBJECT_CODE: &str = "INFO01";

/// Test configuration: sin API key (modo simulado), auth activo, MCP en un
/// puerto muerto para que el sondeo sea hermético.
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18094,
        session_secret: "pages_smoke_test_secret_at_least_32chars!!".to_string(),
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

    // Datos reales para las rutas parametrizadas:
    // - una nota (slug en `notes.slug`) para `/app/notes/{slug}`
    // - un código de asignatura (`chat_history.subject`) para
    //   `/dashboard/subjects/{code}`
    sqlx::query(
        "INSERT INTO notes (title, slug, filename, tags, word_count) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(NOTE_TITLE)
    .bind(NOTE_SLUG)
    .bind(format!("{}.md", NOTE_SLUG))
    .bind("smoke")
    .bind(0_i64)
    .execute(&pool)
    .await
    .expect("Failed to seed note");
    sqlx::query("INSERT INTO chat_history (subject, role, content) VALUES (?, ?, ?)")
        .bind(SUBJECT_CODE)
        .bind("user")
        .bind("pregunta de humo")
        .execute(&pool)
        .await
        .expect("Failed to seed chat history");

    let state = Arc::new(AppState {
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

/// Código de asignatura real en la BD (`chat_history.subject`).
async fn subject_code(pool: &sqlx::SqlitePool) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT subject FROM chat_history WHERE TRIM(subject) <> '' ORDER BY id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

/// Slug de nota real en la BD (`notes.slug`).
async fn note_slug(pool: &sqlx::SqlitePool) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT slug FROM notes ORDER BY id LIMIT 1")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}

/// Barrido: todas las páginas HTML registradas deben responder 200.
#[tokio::test]
async fn smoke_paginas_registradas_responden_200_con_sesion() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    // Rutas sin parámetro (ver cabecera del archivo para la fuente de cada una).
    let mut rutas: Vec<String> = vec![
        // src/handlers/mod.rs (router `public`)
        "/".to_string(),
        "/dashboard".to_string(),
        "/dashboard/ai".to_string(),
        "/dashboard/notes".to_string(),
        "/dashboard/mcp".to_string(),
        "/legacy".to_string(),
        // src/handlers/mod.rs (router `protected`)
        "/app/notes".to_string(),
        "/app/notes/new".to_string(),
        "/dashboard/material".to_string(),
        "/dashboard/search?q=x".to_string(),
        "/settings".to_string(),
        // src/academic/mod.rs (anidadas bajo /dashboard)
        "/dashboard/calendar".to_string(),
        "/dashboard/calendar/daily".to_string(),
        "/dashboard/horario".to_string(),
        "/dashboard/planner".to_string(),
        "/dashboard/profile".to_string(),
        "/dashboard/subjects".to_string(),
        "/dashboard/examenes".to_string(),
        "/dashboard/vida-escolar".to_string(),
        // src/handlers/admin.rs
        "/admin".to_string(),
        // src/ai/mod.rs, src/office/mod.rs
        "/ai".to_string(),
        "/office".to_string(),
    ];

    // Rutas parametrizadas: valor real de la BD o salto con mensaje claro.
    match subject_code(&t.state.db).await {
        Some(code) => rutas.push(format!("/dashboard/subjects/{code}")),
        None => eprintln!(
            "SKIP /dashboard/subjects/{{code}}: la BD no tiene ninguna asignatura (chat_history.subject vacío)"
        ),
    }
    match note_slug(&t.state.db).await {
        Some(slug) => rutas.push(format!("/app/notes/{slug}")),
        None => eprintln!("SKIP /app/notes/{{slug}}: la BD no tiene ninguna nota"),
    }

    let mut fallos: Vec<String> = Vec::new();
    for (i, ruta) in rutas.iter().enumerate() {
        // IP única por ruta: el bucket global es de 60 req/min por IP.
        let ip = format!("10.42.0.{}", (i % 250) as u8 + 1);
        let resp = get(app.clone(), ruta, &t.cookie, &ip).await;
        let status = resp.status();
        if status != StatusCode::OK {
            let ct = resp
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("<sin content-type>");
            fallos.push(format!("GET {ruta} -> {status} [{ct}] (se esperaba 200)"));
        }
    }

    assert!(
        fallos.is_empty(),
        "páginas que no devolvieron 200 ({} de {} rutas barridas):\n  - {}",
        fallos.len(),
        rutas.len(),
        fallos.join("\n  - ")
    );
}

/// Los destinos del menú lateral (`NAV_LINKS`) deben existir todos.
///
/// El nav se renderiza con `href` reales: si uno apunta a una ruta que no está
/// registrada, el 404 lo ve el usuario al hacer clic. Este test barre el mapa
/// canónico y falla nombrando el `data-path` roto.
#[tokio::test]
async fn destinos_del_menu_lateral_existen() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let mut fallos: Vec<String> = Vec::new();
    for (i, (path, route)) in study_platform::handlers::stitch_v2::NAV_LINKS
        .iter()
        .enumerate()
    {
        let ip = format!("10.43.0.{}", (i % 250) as u8 + 1);
        let resp = get(app.clone(), route, &t.cookie, &ip).await;
        let status = resp.status();
        if status != StatusCode::OK {
            fallos.push(format!("{path} -> {route} devolvió {status}"));
        }
    }

    assert!(
        fallos.is_empty(),
        "destinos del menú que no existen ({} de {}):\n  - {}",
        fallos.len(),
        study_platform::handlers::stitch_v2::NAV_LINKS.len(),
        fallos.join("\n  - ")
    );
}
