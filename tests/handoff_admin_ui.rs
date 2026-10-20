//! Integration tests for the owner's handoff page and the admin surface.
//!
//! Contract under test (against the router as implemented in
//! `src/handlers/mod.rs` + `src/handlers/admin.rs`):
//! - `GET /handoff`            → 302 → `/login` without a session; 200 with
//!   one. The page shows the REAL system state (uptime, AI budget) and never
//!   the OpenRouter key — not even its `sk-` prefix.
//! - `GET /admin`              → 200 with the budget banner (`$2.00 USD`).
//! - `POST /api/admin/backup`  → 200 JSON `{success: true, name}`.
//! - `GET /admin/metrics`      → 200 JSON with numeric `uptime_secs` and
//!   `ram_mb_used`.
//! - `GET /admin/backup/list`  → 200 JSON with `count` and `items`.
//!
//! Harness: same shape as `tests/admin_routes.rs` — temp-dir SQLite with the
//! real migrations, a seeded `admin` user and a DB-backed session
//! (`SHA256(secret:token)`), `mikareno: None` and no API key (mock mode).
//! Both MCP URLs point at dead loopback ports, so the probes of `/handoff`
//! fail fast (connection refused) and the test stays hermetic: no network,
//! no real student data, nothing written outside the temp dir — with the
//! single exception of `POST /api/admin/backup`, which by design writes the
//! backup into the app's `data/backups/` directory.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use password_hash::PasswordHasher;
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::middleware::auth::{generate_session_token, hash_session_token};
use study_platform::AppState;

/// Administrator username seeded for the authenticated tests.
const ADMIN_USER: &str = "admin";

/// Test configuration: budget fixed at $2.00 so the budget assertions are
/// deterministic. No API key (mock mode), auth enabled.
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18093,
        session_secret: "handoff_admin_ui_test_secret_32chars!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: None,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        // Puertos muertos: el sondeo `initialize` de /handoff falla de
        // inmediato (connection refused) sin salir a la red.
        mikareno_mcp_url: "http://127.0.0.1:57777/mcp".to_string(),
        genoffice_mcp_url: "http://127.0.0.1:57778/mcp".to_string(),
        custom_mcp_api_key: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
        registration_token: None,
    }
}

/// Handle holding the temp paths alive for the duration of a test.
struct TestApp {
    state: Arc<AppState>,
    cookie: String,
    // Keeps the temp DB file (and its dir) alive until the test ends.
    _keep: (tempfile::TempDir, tempfile::NamedTempFile),
}

/// Set up the app state: real migrations, seeded admin user and a valid
/// DB-backed session (hashed opaque token, the Mission 11 format).
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

    sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await.ok();
    sqlx::query("PRAGMA foreign_keys=ON;").execute(&pool).await.ok();
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    // Seed the administrator with a known password.
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

/// Request through the real router, optionally carrying the session cookie.
async fn request(
    app: axum::Router,
    method: Method,
    uri: &str,
    cookie: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    app.oneshot(builder.body(Body::empty()).unwrap()).await.unwrap()
}

async fn get(
    app: axum::Router,
    uri: &str,
    cookie: Option<&str>,
) -> axum::response::Response {
    request(app, Method::GET, uri, cookie).await
}

async fn post(
    app: axum::Router,
    uri: &str,
    cookie: Option<&str>,
) -> axum::response::Response {
    request(app, Method::POST, uri, cookie).await
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn body_string(resp: axum::response::Response) -> String {
    String::from_utf8_lossy(&body_bytes(resp).await).to_string()
}

// --- 1. /handoff con sesión: estado real, sin secretos ---

#[tokio::test]
async fn handoff_with_session_shows_state_and_hides_secrets() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(app, "/handoff", Some(&t.cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;

    // Uptime (la tarjeta "Uptime" y la línea `uptime` del informe).
    assert!(
        body.to_lowercase().contains("uptime"),
        "/handoff must show the uptime, got: {body}"
    );

    // Presupuesto de IA: tope de $2.00 (configurado en el harness).
    assert!(
        body.contains("2.00"),
        "/handoff must show the $2.00 budget, got: {body}"
    );
    assert!(
        body.contains("USD"),
        "/handoff must show the budget in USD, got: {body}"
    );

    // Sin secretos: ni el prefijo de la clave ni su valor.
    assert!(
        !body.contains("sk-"),
        "/handoff must never print an OpenRouter key prefix"
    );
    if let Some(key) = t.state.config.openrouter_api_key.as_deref() {
        assert!(
            !key.is_empty() && !body.contains(key),
            "/handoff must never print the OpenRouter key"
        );
    }

    // El estado de la clave se muestra como booleano, no como valor.
    assert!(
        body.to_lowercase().contains("configurada"),
        "/handoff must render the key status (configured / not configured)"
    );
}

// --- 2. /handoff sin sesión: redirige al login ---

#[tokio::test]
async fn handoff_without_session_redirects_to_login() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(app, "/handoff", None).await;
    assert_eq!(
        resp.status(),
        StatusCode::FOUND,
        "/handoff without a session must not render the page"
    );
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/login"),
        "unauthenticated /handoff must redirect to /login"
    );
}

// --- 3. /admin con sesión: banner del presupuesto ---

#[tokio::test]
async fn admin_page_with_session_shows_budget_banner() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(app, "/admin", Some(&t.cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;

    assert!(
        body.contains("2.00") && body.contains("USD"),
        "/admin must show the $2.00 USD budget banner, got: {body}"
    );
    assert!(
        body.contains("$2.00") || body.contains("2.00 USD"),
        "/admin budget banner must carry the configured budget, got: {body}"
    );
}

// --- 4. POST /api/admin/backup ---

#[tokio::test]
async fn admin_backup_post_creates_a_named_backup() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = post(app, "/api/admin/backup", Some(&t.cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert_eq!(
        json["success"],
        serde_json::json!(true),
        "backup must report success, got: {json}"
    );
    let name = json["name"]
        .as_str()
        .unwrap_or_else(|| panic!("backup must return a name, got: {json}"));
    assert!(
        name.starts_with("app-") && name.ends_with(".db"),
        "backup name must be app-<timestamp>.db, got: {name}"
    );
}

// --- 5. GET /admin/metrics ---

#[tokio::test]
async fn admin_metrics_reports_numeric_uptime_and_ram() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(app, "/admin/metrics", Some(&t.cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();

    assert!(
        json["uptime_secs"].is_u64() || json["uptime_secs"].is_i64(),
        "uptime_secs must be a number, got: {json}"
    );
    assert!(
        json["ram_mb_used"].is_number(),
        "ram_mb_used must be a number, got: {json}"
    );
}

// --- 6. GET /admin/backup/list ---

#[tokio::test]
async fn admin_backup_list_returns_count_and_items() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(app, "/admin/backup/list", Some(&t.cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();

    assert!(
        json["count"].is_u64() || json["count"].is_i64(),
        "count must be a number, got: {json}"
    );
    assert!(
        json["items"].is_array(),
        "items must be an array, got: {json}"
    );
}
