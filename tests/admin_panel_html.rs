//! Integration tests for the admin panel HTML/JSON surface (Fase 9 dashboard).
//!
//! Contract under test (routes registered by `admin::register_routes` in
//! `src/handlers/admin.rs`, merged in `src/handlers/mod.rs`):
//! - `GET /admin`                → 302 → `/login` without session; 200 HTML with one.
//!   The HTML must show the admin title, the budget banner (`$2.00 USD` with
//!   the deterministic test config) and must NOT leak any secret (`sk-`).
//! - `GET /admin/metrics`        → 200 JSON with numeric `uptime_secs` and
//!   `ram_mb_used`.
//! - `GET /admin/backup/list`    → 200 JSON with a `count` >= 0 and an `items` array.
//!
//! Self-contained harness (same shape as `tests/admin_routes.rs`): temp-dir
//! SQLite running real migrations, seeded `admin` user and a DB-backed
//! session stored as `SHA256(secret:token)` via the public auth helpers.
//! No real student data, no network, nothing is written outside the temp dir.

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

/// Administrator username seeded for the authenticated tests.
const ADMIN_USER: &str = "admin";

/// Test configuration: budget fixed at $2.00 so the budget banner assertions
/// are deterministic. No API key (mock mode), auth enabled.
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18093,
        session_secret: "admin_panel_html_test_secret_at_least_32chars!!".to_string(),
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

/// Handle holding the temp paths alive for the duration of a test.
struct TestApp {
    state: Arc<AppState>,
    cookie: String,
    // Keeps the temp DB file (and its dir) alive until the test ends.
    _keep: (tempfile::TempDir, tempfile::NamedTempFile),
}

/// Set up the full app state: real migrations, seeded admin user and a valid
/// DB-backed session (hashed opaque token, the Mission 11 format).
async fn setup_admin_app() -> TestApp {
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

    sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await.ok();
    sqlx::query("PRAGMA foreign_keys=ON;").execute(&pool).await.ok();
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    // Seed the administrator with a known password (matches admin_routes.rs).
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
    .bind(ADMIN_USER)
    .bind(&password_hash)
    .execute(&pool)
    .await
    .expect("Failed to seed admin user");
    let uid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(ADMIN_USER)
        .fetch_one(&pool)
        .await
        .expect("admin user id");

    // DB-backed session the middleware accepts: store the token's hash, cookie
    // carries the raw token (exactly how a real login mints it).
    let token = generate_session_token();
    let hashed = hash_session_token(&token, &secret);
    let expires = chrono::Utc::now() + chrono::Duration::hours(24);
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(&hashed)
        .bind(uid)
        .bind(expires.to_rfc3339())
        .execute(&pool)
        .await
        .expect("Failed to seed admin session");

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

/// GET through the real router, optionally carrying the session cookie.
async fn get(app: axum::Router, uri: &str, cookie: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().uri(uri);
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    app.oneshot(builder.body(Body::empty()).unwrap()).await.unwrap()
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn body_string(resp: axum::response::Response) -> String {
    String::from_utf8_lossy(&body_bytes(resp).await).into_owned()
}

// --- 1. GET /admin con sesión admin: HTML del panel ---

/// El panel de administración se renderiza (200 HTML), muestra el título de
/// administración, el banner de presupuesto con el tope de $2.00 y no filtra
/// secretos (ningún `sk-`).
#[tokio::test]
async fn admin_panel_html_renders_with_budget_banner_and_no_secrets() {
    let t = setup_admin_app().await;
    let app = study_platform::handlers::router(t.state.clone());
    let cookie = t.cookie.clone();

    let resp = get(app, "/admin", Some(&cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK, "GET /admin must return 200");

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.contains("text/html"),
        "GET /admin must serve HTML, got content-type: {content_type}"
    );

    let html = body_string(resp).await;

    assert!(
        html.contains("Administración") || html.contains("Admin"),
        "el panel debe mostrar el título de administración"
    );

    // Banner de presupuesto: el tope de la config de test es 2.0 USD y el
    // render lo formatea como "$2.00 USD".
    assert!(
        html.contains("$2.00"),
        "el panel debe mostrar el banner de presupuesto con $2.00"
    );

    assert!(
        !html.contains("sk-"),
        "el panel no debe filtrar claves de API (prefijo 'sk-')"
    );
}

// --- 2. GET /admin/metrics: JSON con métricas numéricas ---

/// `GET /admin/metrics` → 200 con JSON: `uptime_secs` y `ram_mb_used` son
/// números (no strings, no null).
#[tokio::test]
async fn admin_metrics_endpoint_returns_numeric_system_metrics() {
    let t = setup_admin_app().await;
    let app = study_platform::handlers::router(t.state.clone());
    let cookie = t.cookie.clone();

    let resp = get(app, "/admin/metrics", Some(&cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK, "GET /admin/metrics must return 200");

    let json: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();

    assert!(
        json["uptime_secs"].is_number(),
        "uptime_secs debe ser un número, got: {}",
        json["uptime_secs"]
    );
    assert!(
        json["ram_mb_used"].is_number(),
        "ram_mb_used debe ser un número, got: {}",
        json["ram_mb_used"]
    );
}

// --- 3. GET /admin/backup/list: JSON con count e items ---

/// `GET /admin/backup/list` → 200 con JSON: `count` (≥ 0) e `items` (array).
#[tokio::test]
async fn admin_backup_list_endpoint_returns_count_and_items() {
    let t = setup_admin_app().await;
    let app = study_platform::handlers::router(t.state.clone());
    let cookie = t.cookie.clone();

    let resp = get(app, "/admin/backup/list", Some(&cookie)).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /admin/backup/list must return 200"
    );

    let json: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();

    let count = json
        .get("count")
        .or_else(|| json.get("contar"))
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| panic!("falta el contador de backups en: {json}"));
    assert!(count >= 0, "el contador de backups no puede ser negativo: {count}");

    assert!(
        json["items"].is_array(),
        "items debe ser un array, got: {}",
        json["items"]
    );
}

// --- 4. GET /admin sin sesión: 302 (a /login) o 401 ---

/// Sin cookie de sesión el panel no se sirve: el middleware de auth
/// redirige a `/login` (302) o rechaza con 401.
#[tokio::test]
async fn admin_panel_requires_session() {
    let t = setup_admin_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(app, "/admin", None).await;
    let status = resp.status();
    assert!(
        status == StatusCode::FOUND || status == StatusCode::UNAUTHORIZED,
        "GET /admin sin sesión debe ser 302 o 401, got: {status}"
    );
}
