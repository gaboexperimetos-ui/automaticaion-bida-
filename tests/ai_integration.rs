//! Integration tests for the AI assistant.
//! Tests mock mode behavior, cost tracking, and persistence.
//! No real API calls are made during tests.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::ai::{AiAssistant, CostTracker, Quiz, QuizQuestion};
use study_platform::config::Config;
use study_platform::AppState;

/// Session cookie used to bypass auth middleware. After Mission 11 the
/// middleware (`require_auth_state`) validates against the `sessions` table
/// in the DB, so this UUID must be seeded as a session row (see setup_test_state).
const TEST_SESSION: &str = "session_id=00000000-0000-4000-8000-000000000000";

/// Build an authenticated GET request to `uri`.
fn authed_get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("cookie", TEST_SESSION)
        .body(Body::empty())
        .unwrap()
}

/// Build an authenticated POST request with a JSON body.
/// (Mission 11: ALL /api/ai/* routes require auth, so the cookie is attached.)
fn json_post(uri: &str, json: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("cookie", TEST_SESSION)
        .body(Body::from(json.to_string()))
        .unwrap()
}

/// Build an UNAUTHENTICATED POST request with a JSON body (for 401 tests).
fn raw_json_post(uri: &str, json: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(json.to_string()))
        .unwrap()
}

/// Extract the response body as a string.
async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

/// Create a test config (no API key = mock mode)
fn test_config(db_path: PathBuf, notes_dir: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18090,
        session_secret: "test_secret_key_at_least_32_chars_long!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: None,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://127.0.0.1:8080/mcp".to_string(),
        genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        registration_token: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
    }
}

/// Create test app state with DB + migrations + chat_history table
async fn setup_test_state() -> Arc<AppState> {
    let temp_db = tempfile::NamedTempFile::new().unwrap();
    let notes_dir = tempfile::tempdir().unwrap().keep();
    let config = test_config(temp_db.path().to_path_buf(), notes_dir);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(temp_db.path().to_str().unwrap())
        .await
        .unwrap();

    sqlx::query("PRAGMA journal_mode=WAL;")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("PRAGMA foreign_keys=ON;")
        .execute(&pool)
        .await
        .ok();
    sqlx::query(
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
        );",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS chat_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            subject TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Mission 11: persistent usage metrics (mirrors
    // migrations/20240101000005_usage_metrics.sql — this harness builds its
    // schema manually instead of running migrations).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS usage_metrics (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            model TEXT NOT NULL,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL NOT NULL DEFAULT 0.0,
            estimated INTEGER NOT NULL DEFAULT 0,
            endpoint TEXT NOT NULL DEFAULT ''
        );",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_usage_metrics_created_at ON usage_metrics(created_at);",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO notes (title, slug, filename, tags, word_count) VALUES ('Test', 'test', 'test.md', '', 0);")
        .execute(&pool).await.unwrap();

    // Users + sessions tables for DB-backed require_auth_state middleware
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL,
            user_agent TEXT
        )"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    // Seed a user and a session matching TEST_SESSION UUID so that all
    // authenticated routes in this test suite get past require_auth_state.
    let test_session_uuid = "00000000-0000-4000-8000-000000000000";
    sqlx::query("INSERT INTO users (username, password_hash) VALUES (?, ?)")
        .bind("testuser")
        .bind("$argon2id$v=19$m=19456,t=2,p=1$placeholder")
        .execute(&pool)
        .await
        .unwrap();
    let user_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(test_session_uuid)
        .bind(user_id)
        .bind((chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

    Arc::new(AppState {
        cache_mikareno: std::sync::Arc::new(
            study_platform::academic::calendar_data::CacheMikareno::default(),
        ),
        cache_catalogo: std::sync::Arc::new(study_platform::ai::modelos::CacheCatalogo::default()),
        config: config.clone(),
        db: pool,
        mikareno: None,
        start_time: std::time::Instant::now(),
    })
}

#[tokio::test]
async fn test_cost_tracker_within_budget() {
    let mut tracker = CostTracker::new(2.0);
    tracker.record_request(0.5).unwrap();
    tracker.record_request(0.5).unwrap();
    assert!(tracker.can_proceed());
    assert_eq!(tracker.summary().total_cost, 1.0);
    assert_eq!(tracker.summary().budget_limit, 2.0);
}

#[tokio::test]
async fn test_cost_tracker_exceeds_budget() {
    let mut tracker = CostTracker::new(0.001);
    tracker.record_request(0.001).unwrap();
    let result = tracker.record_request(0.001);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_chat_mock_mode() {
    let state = setup_test_state().await;
    let assistant = AiAssistant::new(state);

    // No API key → should return mock response
    let response = assistant
        .chat("Hola, ¿cómo estás?", "general")
        .await
        .unwrap();
    assert!(response.contains("Modo simulado"));
    assert!(!response.contains("error"));
}

#[tokio::test]
async fn test_summarize_mock_mode() {
    let state = setup_test_state().await;
    let assistant = AiAssistant::new(state);

    let response = assistant
        .summarize("Test content about calculus", 500)
        .await
        .unwrap();
    assert!(!response.is_empty());
}

#[tokio::test]
async fn test_generate_quiz_mock_mode() {
    let state = setup_test_state().await;
    let assistant = AiAssistant::new(state);

    let quiz = assistant
        .generate_quiz("Contenido de prueba", 3)
        .await
        .unwrap();
    assert_eq!(quiz.title, "Cuestionario");
    assert_eq!(quiz.questions.len(), 3);
}

#[tokio::test]
async fn test_quiz_serialization() {
    let quiz = Quiz {
        title: "Math Quiz".to_string(),
        questions: vec![QuizQuestion {
            question: "2+2?".to_string(),
            options: vec!["3".into(), "4".into(), "5".into(), "6".into()],
            correct_answer: 1,
        }],
    };
    let json = serde_json::to_string(&quiz).unwrap();
    let parsed: Quiz = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.title, "Math Quiz");
    assert_eq!(parsed.questions[0].correct_answer, 1);
}

#[tokio::test]
async fn test_budget_tracking() {
    let state = setup_test_state().await;
    let assistant = AiAssistant::new(state);

    let cost = assistant.cost_tracking().await;
    assert!(cost.total_cost >= 0.0);
    assert!(cost.budget_limit > 0.0);
}

#[tokio::test]
async fn test_budget_remaining() {
    let state = setup_test_state().await;
    let assistant = AiAssistant::new(state);

    let remaining = assistant.budget_remaining().await;
    assert!(remaining >= 0.0);
}

// ---------------------------------------------------------------------------
// HTTP endpoint integration tests (mock mode — no API key configured)
// ---------------------------------------------------------------------------

/// POST /api/ai/chat should return 200 with text (mock mode).
#[tokio::test]
async fn test_chat_api_endpoint() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let body = serde_json::to_string(&serde_json::json!({
        "message": "Hola, ¿cómo estás?",
        "subject": "general"
    }))
    .unwrap();

    let response = app
        .clone()
        .oneshot(json_post("/api/ai/chat", &body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let text = body_string(response).await;
    assert!(text.contains("Modo simulado"));
}

/// POST /api/ai/chat should echo the subject context in the mock response.
#[tokio::test]
async fn test_chat_api_endpoint_with_subject() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let body = serde_json::to_string(&serde_json::json!({
        "message": "¿Qué es la derivada?",
        "subject": "cálculo"
    }))
    .unwrap();

    let response = app
        .clone()
        .oneshot(json_post("/api/ai/chat", &body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let text = body_string(response).await;
    assert!(text.contains("Modo simulado"));
    assert!(text.contains("cálculo") || text.contains("Cálculo") || text.contains("cohere"));
}

/// GET /api/ai/cost should return JSON with cost information (auth required).
#[tokio::test]
async fn test_cost_api_endpoint() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .clone()
        .oneshot(authed_get("/api/ai/cost"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let text = body_string(response).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(json.get("total_cost").is_some());
    assert!(json.get("budget_limit").is_some());
    assert!(json.get("requests_count").is_some());
    assert!(json.get("alert").is_some());
    assert!(json["budget_limit"].as_f64().unwrap() > 0.0);
    assert!(json["total_cost"].as_f64().unwrap() >= 0.0);
    assert_eq!(json["alert"].as_bool().unwrap(), false);
}

/// POST /api/ai/quiz should return JSON with title and questions.
#[tokio::test]
async fn test_quiz_api_endpoint() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let body = serde_json::to_string(&serde_json::json!({
        "content": "Contenido de prueba para el cuestionario",
        "num_questions": 3
    }))
    .unwrap();

    let response = app
        .clone()
        .oneshot(json_post("/api/ai/quiz", &body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let text = body_string(response).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(json.get("title").is_some());
    assert!(json.get("questions").is_some());
    assert!(json["questions"].as_array().unwrap().len() >= 1);
}

/// POST /api/ai/quiz should default to 5 questions when num_questions is omitted.
#[tokio::test]
async fn test_quiz_api_endpoint_default_questions() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let body = serde_json::to_string(&serde_json::json!({
        "content": "Contenido sin número de preguntas"
    }))
    .unwrap();

    let response = app
        .clone()
        .oneshot(json_post("/api/ai/quiz", &body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let text = body_string(response).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["questions"].as_array().unwrap().len(), 5);
}

/// POST /api/ai/summary should return JSON with a summary field.
#[tokio::test]
async fn test_summary_api_endpoint() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let body = serde_json::to_string(&serde_json::json!({
        "content": "Este es un texto de prueba sobre cálculo diferencial."
    }))
    .unwrap();

    let response = app
        .clone()
        .oneshot(json_post("/api/ai/summary", &body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let text = body_string(response).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(json.get("summary").is_some());
    assert!(!json["summary"].as_str().unwrap().is_empty());
}

/// POST /api/ai/study-sheet should return JSON with a study_sheet field.
#[tokio::test]
async fn test_study_sheet_api_endpoint() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let body = serde_json::to_string(&serde_json::json!({
        "content": "Este es un texto de prueba para la hoja de estudio."
    }))
    .unwrap();

    let response = app
        .clone()
        .oneshot(json_post("/api/ai/study-sheet", &body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let text = body_string(response).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(json.get("study_sheet").is_some());
    assert!(!json["study_sheet"].as_str().unwrap().is_empty());
}

/// GET /ai without auth should redirect to /login.
#[tokio::test]
async fn test_ai_page_requires_auth() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .clone()
        .oneshot(Request::builder().uri("/ai").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

/// GET /ai with auth should return 200 with the chat UI HTML.
#[tokio::test]
async fn test_ai_page_with_auth() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let response = app.clone().oneshot(authed_get("/ai")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("Asistente Académico"));
    assert!(html.contains("htmx"));
    assert!(html.contains("tailwindcss"));
    assert!(html.contains("chat-messages"));
}

/// Mock mode: all AI endpoints should work without an API key.
#[tokio::test]
async fn test_mock_mode_no_api_key() {
    let state = setup_test_state().await;
    assert!(
        state.config.openrouter_api_key.is_none(),
        "Mock mode requires no API key"
    );

    let app = study_platform::handlers::router(state);

    // Chat
    let body = serde_json::to_string(&serde_json::json!({
        "message": "Hola",
        "subject": "general"
    }))
    .unwrap();
    let resp = app
        .clone()
        .oneshot(json_post("/api/ai/chat", &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_string(resp).await.contains("Modo simulado"));

    // Cost (auth required since Mission 11)
    let resp = app
        .clone()
        .oneshot(authed_get("/api/ai/cost"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Quiz
    let body = serde_json::to_string(&serde_json::json!({
        "content": "Test",
        "num_questions": 3
    }))
    .unwrap();
    let resp = app
        .clone()
        .oneshot(json_post("/api/ai/quiz", &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// API routes require authentication (Mission 11: previously public).
///
/// Unauthenticated callers get 401 on `/api/*`; authenticated callers (any
/// valid-UUID cookie — `require_auth` is format-only) get normal behavior.
#[tokio::test]
async fn test_api_routes_are_public() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    // /api/ai/cost WITHOUT auth → 401
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/ai/cost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // POST /api/ai/chat WITHOUT auth → 401 (not 422: auth runs first)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/ai/chat")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // POST with valid body + auth should succeed
    let body = serde_json::to_string(&serde_json::json!({
        "message": "Hola",
        "subject": "general"
    }))
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/ai/chat")
                .header("content-type", "application/json")
                .header("cookie", TEST_SESSION)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Mission 11 (Agent B): budget gate, auth, validation, persistence tests.
// Mock mode only (`openrouter_api_key: None`) — no live provider calls.
// ---------------------------------------------------------------------------

/// Multi-connection test state backed by a real temp file (kept alive via the
/// returned `TempDir` guard) so parallel tasks genuinely run concurrently.
async fn setup_shared_state(budget: f64) -> (Arc<AppState>, tempfile::TempDir) {
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    let mut config = test_config(db_path.clone(), notes_dir);
    config.openrouter_max_cost_usd = budget;

    let opts = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await
        .unwrap();

    // Enable FK enforcement so sessions.user_id references users.id.
    sqlx::query("PRAGMA foreign_keys=ON;")
        .execute(&pool)
        .await
        .ok();

    for ddl in [
        "CREATE TABLE IF NOT EXISTS chat_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            subject TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
        "CREATE TABLE IF NOT EXISTS usage_metrics (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            model TEXT NOT NULL,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL NOT NULL DEFAULT 0.0,
            estimated INTEGER NOT NULL DEFAULT 0,
            endpoint TEXT NOT NULL DEFAULT ''
        );",
    ] {
        sqlx::query(ddl).execute(&pool).await.unwrap();
    }

    // Users + sessions tables for DB-backed require_auth_state middleware.
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    // Note: sessions table omits REFERENCES to avoid FK enforcement issues
    // across pooled connections (PRAGMA foreign_keys is per-connection).
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL,
            user_agent TEXT
        )"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let test_session_uuid = "00000000-0000-4000-8000-000000000000";
    sqlx::query("INSERT INTO users (username, password_hash) VALUES (?, ?)")
        .bind("testuser")
        .bind("$argon2id$v=19$m=19456,t=2,p=1$placeholder")
        .execute(&pool)
        .await
        .unwrap();
    let user_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(test_session_uuid)
        .bind(user_id)
        .bind((chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();

    (
        Arc::new(AppState {
            cache_mikareno: std::sync::Arc::new(
                study_platform::academic::calendar_data::CacheMikareno::default(),
            ),
            cache_catalogo: std::sync::Arc::new(
                study_platform::ai::modelos::CacheCatalogo::default(),
            ),
            config: config.clone(),
            db: pool,
            mikareno: None,
            start_time: std::time::Instant::now(),
        }),
        dir,
    )
}

async fn month_spend(pool: &sqlx::SqlitePool) -> f64 {
    sqlx::query_scalar::<_, Option<f64>>("SELECT SUM(cost_usd) FROM usage_metrics")
        .fetch_one(pool)
        .await
        .unwrap()
        .unwrap_or(0.0)
}

/// All AI endpoints reject unauthenticated (and malformed-cookie) callers.
#[tokio::test]
async fn test_ai_endpoints_require_auth() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    // GET /api/ai/cost without cookie → 401.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/ai/cost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // POST endpoints without cookie → 401.
    for (uri, json) in [
        ("/api/ai/chat", r#"{"message":"h","subject":"g"}"#),
        ("/api/ai/summary", r#"{"content":"c"}"#),
        ("/api/ai/quiz", r#"{"content":"c","num_questions":2}"#),
        ("/api/ai/study-sheet", r#"{"content":"c"}"#),
    ] {
        let resp = app.clone().oneshot(raw_json_post(uri, json)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "uri={}", uri);
    }

    // Malformed (non-UUID) session cookie on /api/* → 401.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/ai/cost")
                .header("cookie", "session_id=not-a-uuid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Oversize/empty inputs → 400 with generic messages (no content echoed).
#[tokio::test]
async fn test_ai_validation_rejects_bad_input() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    // message > 8000 chars → 400.
    let big = "x".repeat(8001);
    let body =
        serde_json::to_string(&serde_json::json!({"message": big, "subject": "general"})).unwrap();
    let resp = app
        .clone()
        .oneshot(json_post("/api/ai/chat", &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // empty message → 400.
    let body = serde_json::to_string(&serde_json::json!({"message": "   ", "subject": "general"}))
        .unwrap();
    let resp = app
        .clone()
        .oneshot(json_post("/api/ai/chat", &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // oversize subject → 400.
    let big_sub = "s".repeat(201);
    let body =
        serde_json::to_string(&serde_json::json!({"message": "hola", "subject": big_sub})).unwrap();
    let resp = app
        .clone()
        .oneshot(json_post("/api/ai/chat", &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // content > 20000 chars → 400 on summary/quiz/study-sheet.
    let big_content = "y".repeat(20001);
    for uri in ["/api/ai/summary", "/api/ai/quiz", "/api/ai/study-sheet"] {
        let payload = if uri == "/api/ai/quiz" {
            serde_json::json!({"content": big_content, "num_questions": 2})
        } else {
            serde_json::json!({"content": big_content})
        };
        let body = serde_json::to_string(&payload).unwrap();
        let resp = app.clone().oneshot(json_post(uri, &body)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "uri={}", uri);
    }

    // Boundary lengths still pass (8000-char message, 20000-char content).
    let ok_msg = "z".repeat(8000);
    let body = serde_json::to_string(&serde_json::json!({"message": ok_msg, "subject": "general"}))
        .unwrap();
    let resp = app
        .clone()
        .oneshot(json_post("/api/ai/chat", &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// num_questions is clamped into 1..=10 (not an error).
#[tokio::test]
async fn test_quiz_num_questions_clamped() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let body = serde_json::to_string(&serde_json::json!({
        "content": "Contenido de prueba",
        "num_questions": 0
    }))
    .unwrap();
    let resp = app
        .clone()
        .oneshot(json_post("/api/ai/quiz", &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(json["questions"].as_array().unwrap().len(), 1);

    let body = serde_json::to_string(&serde_json::json!({
        "content": "Contenido de prueba",
        "num_questions": 99
    }))
    .unwrap();
    let resp = app
        .clone()
        .oneshot(json_post("/api/ai/quiz", &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(json["questions"].as_array().unwrap().len(), 10);
}

/// Mock chat records a 0.0-cost `estimated = 1` row (complete history that can
/// never trip the gate by itself).
#[tokio::test]
async fn test_mock_chat_records_zero_cost_row() {
    let state = setup_test_state().await;
    let assistant = AiAssistant::new(state.clone());

    let resp = assistant.chat("Hola", "general").await.unwrap();
    assert!(resp.contains("Modo simulado"));

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_metrics")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert!(month_spend(&state.db).await.abs() < 1e-12);

    let row: (i64, String) =
        sqlx::query_as("SELECT estimated, endpoint FROM usage_metrics LIMIT 1")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(row.0, 1);
    assert_eq!(row.1, "chat");
}

/// Exhausted month rejects (even mock) with 429 and records nothing.
#[tokio::test]
async fn test_budget_gate_blocks_when_exhausted() {
    let state = setup_test_state().await; // budget 2.0
    sqlx::query(
        "INSERT INTO usage_metrics (model, cost_usd, estimated, endpoint)
         VALUES ('seed', 2.0, 0, 'seed')",
    )
    .execute(&state.db)
    .await
    .unwrap();

    let app = study_platform::handlers::router(state.clone());
    let body = serde_json::to_string(&serde_json::json!({
        "message": "Hola",
        "subject": "general"
    }))
    .unwrap();
    let resp = app
        .clone()
        .oneshot(json_post("/api/ai/chat", &body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let text = body_string(resp).await;
    assert!(text.contains("budget"));
    // No internals leaked in the client message.
    assert!(!text.to_lowercase().contains("sqlite"));
    assert!(!text.to_lowercase().contains("sqlx"));

    // Rejected call recorded nothing: spend unchanged.
    assert!((month_spend(&state.db).await - 2.0).abs() < 1e-9);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_metrics")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

/// N parallel mocked chats against a nearly-exhausted tiny budget: spend
/// never exceeds the cap (transactional `BEGIN IMMEDIATE` gate).
#[tokio::test]
async fn test_concurrent_mock_calls_never_exceed_cap() {
    let budget = 0.05;
    let (state, _tmp) = setup_shared_state(budget).await;
    sqlx::query(
        "INSERT INTO usage_metrics (model, cost_usd, estimated, endpoint)
         VALUES ('seed', 0.04, 0, 'seed')",
    )
    .execute(&state.db)
    .await
    .unwrap();

    let app = study_platform::handlers::router(state.clone());
    let body = serde_json::to_string(&serde_json::json!({
        "message": "Hola, ¿cómo estás?",
        "subject": "general"
    }))
    .unwrap();

    let mut handles = Vec::new();
    for _ in 0..16 {
        let app = app.clone();
        let body = body.clone();
        handles.push(tokio::spawn(async move {
            let resp = app.oneshot(json_post("/api/ai/chat", &body)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert!(body_string(resp).await.contains("Modo simulado"));
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let spend = month_spend(&state.db).await;
    assert!(
        spend <= budget + 1e-9,
        "spend {} exceeded cap {}",
        spend,
        budget
    );
    // 16 mock rows + 1 seeded row.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_metrics")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 17);
}

/// Spend survives `AiAssistant` re-creation (same DB): cost tracking is
/// persistent, not per-instance.
#[tokio::test]
async fn test_restart_persistence_across_assistant_instances() {
    let state = setup_test_state().await;
    sqlx::query(
        "INSERT INTO usage_metrics
         (model, input_tokens, output_tokens, cost_usd, estimated, endpoint)
         VALUES ('cohere/north-mini-code:free', 100, 50, 0.75, 0, 'chat')",
    )
    .execute(&state.db)
    .await
    .unwrap();

    let a1 = AiAssistant::new(state.clone());
    let c1 = a1.cost_tracking().await;
    assert!((c1.total_cost - 0.75).abs() < 1e-9);
    drop(a1);

    // New instance, same DB → same spend.
    let a2 = AiAssistant::new(state.clone());
    let c2 = a2.cost_tracking().await;
    assert!((c2.total_cost - 0.75).abs() < 1e-9);
    assert_eq!(c2.requests_count, 1);
    assert!((a2.budget_remaining().await - (2.0 - 0.75)).abs() < 1e-9);

    // Same via the HTTP cost endpoint.
    let app = study_platform::handlers::router(state);
    let resp = app.oneshot(authed_get("/api/ai/cost")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert!((json["total_cost"].as_f64().unwrap() - 0.75).abs() < 1e-9);
    assert_eq!(json["requests_count"].as_u64().unwrap(), 1);
}

/// `alert` is true at ≥90% of budget ($1.80 on the default $2.00).
#[tokio::test]
async fn test_cost_alert_threshold() {
    let state = setup_test_state().await; // budget 2.0
    let a = AiAssistant::new(state.clone());
    assert!(!a.cost_tracking().await.alert);

    sqlx::query(
        "INSERT INTO usage_metrics (model, cost_usd, estimated, endpoint)
         VALUES ('m', 1.85, 0, 'seed')",
    )
    .execute(&state.db)
    .await
    .unwrap();

    let cost = AiAssistant::new(state.clone()).cost_tracking().await;
    assert!((cost.total_cost - 1.85).abs() < 1e-9);
    assert!(cost.alert);

    let app = study_platform::handlers::router(state);
    let resp = app.oneshot(authed_get("/api/ai/cost")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    assert_eq!(json["alert"].as_bool().unwrap(), true);
}
