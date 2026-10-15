//! End-to-end integration tests for the Study Platform.
//!
//! Covers the full request lifecycle through the real router:
//! auth redirects, authenticated dashboard pages (mock fallback),
//! notes API, AI mock mode, MCP JSON-RPC, and office auth.
//!
//! All data is fictitious. No secrets are used or asserted.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Duration;
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::AppState;

/// Fixed, valid-format UUID used as a session cookie value.
///
/// The auth middleware validates session cookie *format* (UUID) only,
/// so any well-formed UUID passes authentication in tests.
const TEST_SESSION_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

/// Test configuration: mock AI mode, dev MCP mode, temp paths.
///
/// No secrets: `openrouter_api_key` and `custom_mcp_api_key` are `None`.
fn test_config(db_path: PathBuf, notes_dir: PathBuf) -> Config {
    Config {
        host: "127.0.0.1".to_string(),
        port: 18091,
        session_secret: "e2e_test_secret_key_at_least_32_chars!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: None,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://127.0.0.1:8080/mcp".to_string(),
        genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        rust_log: "test".to_string(),
    }
}

/// Session cookie header value for an authenticated test request.
fn auth_cookie() -> String {
    format!("session_id={TEST_SESSION_UUID}")
}

/// Set up isolated app state: temp-dir SQLite DB with minimal tables
/// (`notes`, `users`, `sessions`, `chat_history`) plus a temp notes dir.
async fn setup_state() -> Arc<AppState> {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("e2e-test.db");
    let notes_dir = tmp.path().join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    let config = test_config(db_path.clone(), notes_dir);

    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .expect("Failed to create e2e test database");

    for ddl in [
        "CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL,
            user_agent TEXT
        )",
        "CREATE TABLE IF NOT EXISTS notes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            slug TEXT NOT NULL UNIQUE,
            filename TEXT NOT NULL,
            tags TEXT NOT NULL DEFAULT '',
            word_count INTEGER NOT NULL DEFAULT 0,
            summary TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
        "CREATE TABLE IF NOT EXISTS chat_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            subject TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    ] {
        sqlx::query(ddl)
            .execute(&pool)
            .await
            .expect("Failed to create e2e test table");
    }

    // Seed a user and a session row so that DB-backed require_auth_state
    // accepts the TEST_SESSION_UUID cookie used by authenticated tests.
    sqlx::query("INSERT INTO users (username, password_hash) VALUES (?, ?)")
        .bind("e2etest")
        .bind("$argon2id$v=19$m=19456,t=2,p=1$placeholder")
        .execute(&pool)
        .await
        .expect("insert user");
    let uid: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&pool)
        .await
        .expect("user id");
    let expires = (chrono::Utc::now() + Duration::hours(24)).to_rfc3339();
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(TEST_SESSION_UUID)
        .bind(uid)
        .bind(&expires)
        .execute(&pool)
        .await
        .expect("insert session");

    // Keep the temp dir alive for the duration of the test process.
    std::mem::forget(tmp);

    Arc::new(AppState { config, db: pool })
}

async fn get(app: axum::Router, uri: &str, cookie: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().uri(uri);
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    app.oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// --- 1. Auth redirects (no cookie) ---

#[tokio::test]
async fn test_e2e_auth_redirects_without_cookie() {
    let state = setup_state().await;
    let app = study_platform::handlers::router(state);

    for uri in [
        "/dashboard",
        "/dashboard/calendar",
        "/dashboard/subjects",
        "/dashboard/profile",
        "/ai",
    ] {
        let resp = get(app.clone(), uri, None).await;
        assert_eq!(
            resp.status(),
            StatusCode::FOUND,
            "GET {uri} should redirect"
        );
        assert_eq!(
            resp.headers().get("location").and_then(|v| v.to_str().ok()),
            Some("/login"),
            "GET {uri} should redirect to /login"
        );
    }
}

#[tokio::test]
async fn test_e2e_api_notes_unauthorized_without_cookie() {
    let state = setup_state().await;
    let app = study_platform::handlers::router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/notes")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"title":"Nota ficticia","content":"contenido"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// --- 2. Authenticated dashboard pages (mock fallback) ---

#[tokio::test]
async fn test_e2e_authenticated_dashboard_pages() {
    let state = setup_state().await;
    let app = study_platform::handlers::router(state);
    let cookie = auth_cookie();

    for uri in [
        "/dashboard",
        "/dashboard/calendar",
        "/dashboard/horario",
        "/dashboard/planner",
        "/dashboard/subjects",
        "/dashboard/profile",
    ] {
        let resp = get(app.clone(), uri, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK, "GET {uri} should succeed");
    }
}

// --- 3. Notes via API ---

#[tokio::test]
async fn test_e2e_notes_api_create_and_view() {
    let state = setup_state().await;
    let app = study_platform::handlers::router(state);
    let cookie = auth_cookie();

    let create_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/notes")
                .header("content-type", "application/json")
                .header("cookie", cookie.clone())
                .body(Body::from(
                    r#"{"title":"Apuntes de repaso e2e","content":"Contenido ficticio de prueba: resumen y formulas de ejemplo."}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        create_resp.status() == StatusCode::CREATED || create_resp.status() == StatusCode::OK,
        "POST /api/notes should succeed, got {}",
        create_resp.status()
    );
    let create_body = body_text(create_resp).await;
    let created: serde_json::Value = serde_json::from_str(&create_body).unwrap();
    let slug = created["slug"].as_str().unwrap().to_string();
    assert!(!slug.is_empty());

    let view_resp = get(app.clone(), &format!("/api/notes/{slug}"), Some(&cookie)).await;
    assert_eq!(view_resp.status(), StatusCode::OK);
    let view_body = body_text(view_resp).await;
    assert!(
        view_body.contains("Contenido ficticio de prueba"),
        "note view should contain the created content"
    );
}

// --- 4. AI mock mode ---

#[tokio::test]
async fn test_e2e_ai_chat_mock_mode() {
    let state = setup_state().await;
    let app = study_platform::handlers::router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/ai/chat")
                .header("content-type", "application/json")
                .header("cookie", auth_cookie())
                .body(Body::from(
                    r#"{"message":"Explicame la fotosintesis de forma breve","subject":"biologia"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    assert!(
        text.contains("[Modo simulado]"),
        "mock AI response should contain the mock marker"
    );
}

#[tokio::test]
async fn test_e2e_ai_cost_endpoint() {
    let state = setup_state().await;
    let app = study_platform::handlers::router(state);

    let resp = get(app, "/api/ai/cost", Some(&auth_cookie())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        json.get("total_cost").is_some(),
        "cost response should include total_cost"
    );
    assert!(
        json.get("budget_limit").is_some(),
        "cost response should include budget_limit"
    );
}

// --- 5. MCP JSON-RPC ---

#[tokio::test]
async fn test_e2e_mcp_initialize() {
    let state = setup_state().await;
    let app = study_platform::handlers::router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        json["result"]["serverInfo"].is_object(),
        "initialize should return result.serverInfo"
    );
}

#[tokio::test]
async fn test_e2e_mcp_tools_list() {
    let state = setup_state().await;
    let app = study_platform::handlers::router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let tools = json["result"]["tools"]
        .as_array()
        .expect("tools/list should return a tools array");
    assert_eq!(tools.len(), 6, "MCP server should expose 6 tools");
}

// --- 6. Office auth ---

#[tokio::test]
async fn test_e2e_office_requires_auth() {
    let state = setup_state().await;
    let app = study_platform::handlers::router(state);

    let resp = get(app, "/office", None).await;
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}
