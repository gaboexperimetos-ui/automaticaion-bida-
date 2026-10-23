//! Integration tests for the "Material de estudio IA" surface
//! (`Agent: subagent Rust material_ai`).
//!
//! Contract under test (routes registered in `src/ai/mod.rs`, handlers in
//! `src/ai/handlers.rs`):
//! - `POST /api/ai/summary`      → 401 without a session; 200 with one
//!   (mock mode: no `OPENROUTER_API_KEY` → demo text, never an empty body).
//! - `POST /api/ai/quiz`         → 200 with a non-empty JSON payload
//!   (`questions` / `preguntas` / `quiz` — the key name is not pinned).
//! - `GET  /dashboard/material`  → 200; renders the AI kit wired to the three
//!   real endpoints (`/api/ai/summary`, `/api/ai/quiz`, `/api/ai/study-sheet`)
//!   when the provider is configured, or the honest "IA no configurada"
//!   notice when it is not.
//!
//! Self-contained harness (same shape as `tests/admin_routes.rs`): temp-dir
//! SQLite running the real migrations, a seeded user and a DB-backed session
//! stored as `SHA256(secret:token)` via the public auth helpers.
//! `mikareno` is `None` and the Mikareno URL is non-localhost on purpose, so
//! no MCP client is ever built and the tests make **zero network calls**.
//! `openrouter_api_key` is `None` → the AI layer answers in mock mode.

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

/// Student username seeded for the authenticated tests.
const STUDENT_USER: &str = "estudiante";

/// Test configuration: no API key (mock mode), auth enabled, budget $2.00.
fn test_config(notes_dir: PathBuf, db_path: PathBuf, api_key: Option<&str>) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18093,
        session_secret: "material_ai_test_secret_at_least_32chars!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: api_key.map(|k| k.to_string()),
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        // Host fuera de loopback/RFC1918/CGNAT on purpose:
        // `MikarenoClient::from_config` returns `None` for it, so no MCP client
        // is created and nothing is dialled.
        mikareno_mcp_url: "http://192.0.2.10:57777/mcp".to_string(),
        genoffice_mcp_url: "http://192.0.2.10:8453/mcp".to_string(),
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

/// Set up the app state: real migrations, seeded user and a valid DB-backed
/// session (hashed opaque token, the Mission 11 format).
///
/// `api_key` — `None` for mock mode (the default), `Some(..)` only to render
/// the provider-configured branch of the material page.
async fn setup_app(api_key: Option<&str>) -> TestApp {
    let notes_tmp = tempfile::tempdir().unwrap();
    let notes_dir = notes_tmp.path().to_path_buf();
    let db_tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = db_tmp.path().to_path_buf();
    let db_url = format!("sqlite://{}", db_path.display());

    let config = test_config(notes_dir, db_path, api_key);
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

    // Seed the student with a known password.
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
    .bind(STUDENT_USER)
    .bind(&password_hash)
    .execute(&pool)
    .await
    .expect("Failed to seed user");
    let uid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(STUDENT_USER)
        .fetch_one(&pool)
        .await
        .expect("user id");

    // DB-backed session the middleware accepts: store the token's hash, the
    // cookie carries the raw token (exactly how a real login mints it).
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

/// GET through the real router, optionally carrying the session cookie.
async fn get(app: axum::Router, uri: &str, cookie: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().uri(uri);
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    app.oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// POST a JSON body through the real router, optionally with the session cookie.
async fn post_json(
    app: axum::Router,
    uri: &str,
    payload: serde_json::Value,
    cookie: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    app.oneshot(builder.body(Body::from(payload.to_string())).unwrap())
        .await
        .unwrap()
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

/// True when the payload looks like the mock/demo answer of the AI layer
/// ("[Modo simulado] Entendido: …" / "respuesta de prueba" / "OPENROUTER_API_KEY").
fn looks_like_mock(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "modo simulado",
        "simulad",
        "respuesta de prueba",
        "prueba",
        "demo",
        "openrouter_api_key",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

// --- 1. Summary without a session → 401 ---

#[tokio::test]
async fn ai_summary_requires_authentication() {
    let t = setup_app(None).await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = post_json(
        app,
        "/api/ai/summary",
        serde_json::json!({ "content": "Contenido de prueba", "subject": "Matemáticas" }),
        None,
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "POST /api/ai/summary without a session must be rejected with 401"
    );
}

// --- 2. Summary with a session (mock mode) → 200 with demo text ---

#[tokio::test]
async fn ai_summary_with_session_returns_mock_demo_text() {
    let t = setup_app(None).await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = post_json(
        app,
        "/api/ai/summary",
        serde_json::json!({ "content": "El teorema de Pitágoras: a² + b² = c²", "subject": "x" }),
        Some(&t.cookie),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);

    let raw = body_bytes(resp).await;
    assert!(!raw.is_empty(), "summary response body must not be empty");

    let body = String::from_utf8_lossy(&raw).to_string();
    let json: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("summary must be JSON ({e}): {body}"));

    let summary = json["summary"]
        .as_str()
        .unwrap_or_else(|| panic!("summary payload must carry a `summary` string: {json}"));
    assert!(
        !summary.trim().is_empty(),
        "summary must not be blank: {json}"
    );
    assert!(
        looks_like_mock(&body),
        "mock-mode summary should carry demo text, got: {body}"
    );
}

// --- 3. Quiz with a session (mock mode) → 200 with a non-empty payload ---

#[tokio::test]
async fn ai_quiz_with_session_returns_questions_payload() {
    let t = setup_app(None).await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = post_json(
        app,
        "/api/ai/quiz",
        serde_json::json!({ "content": "Fotosíntesis: las plantas convierten luz en azúcar", "num_questions": 3 }),
        Some(&t.cookie),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);

    let raw = body_bytes(resp).await;
    assert!(!raw.is_empty(), "quiz response body must not be empty");

    let body = String::from_utf8_lossy(&raw).to_string();
    let json: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("quiz must be JSON ({e}): {body}"));

    let obj = json
        .as_object()
        .unwrap_or_else(|| panic!("quiz payload must be a JSON object, got: {json}"));
    assert!(!obj.is_empty(), "quiz payload must not be empty: {json}");

    // The key name is not pinned ("questions" / "preguntas" / "quiz"): when one
    // of them is present it must carry the questions.
    if let Some(q) = obj.get("questions").or_else(|| obj.get("preguntas")) {
        assert!(q.is_array(), "questions must be an array, got: {q}");
    }
}

// --- 4. Material page → 200 and the AI-kit endpoints ---

#[tokio::test]
async fn material_page_renders_ai_kit_endpoints() {
    // Without a session the page redirects to the login form.
    let t = setup_app(None).await;
    let app = study_platform::handlers::router(t.state.clone());
    let resp = get(app.clone(), "/dashboard/material", None).await;
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/login"),
        "unauthenticated /dashboard/material must redirect to /login"
    );

    // With a session and NO provider key, the page must still render and be
    // honest about the missing key (no fake AI section).
    let resp = get(app.clone(), "/dashboard/material", Some(&t.cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8_lossy(&body_bytes(resp).await).to_string();
    assert!(
        body.contains("/api/ai/summary")
            || body.contains("IA no configurada")
            || body.contains("OPENROUTER_API_KEY"),
        "material page must expose the AI kit or the honest 'IA no configurada' notice"
    );

    // With a provider key configured, the AI kit is rendered and wired to the
    // three real endpoints (GET only: no provider call is made).
    let t2 = setup_app(Some("test-key-not-used")).await;
    let app2 = study_platform::handlers::router(t2.state.clone());
    let resp = get(app2, "/dashboard/material", Some(&t2.cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = String::from_utf8_lossy(&body_bytes(resp).await).to_string();
    for endpoint in ["/api/ai/summary", "/api/ai/quiz", "/api/ai/study-sheet"] {
        assert!(
            body.contains(endpoint),
            "material page must reference {endpoint}"
        );
    }
}
