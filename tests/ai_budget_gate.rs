//! Integration test for the persistent AI budget gate on `POST /api/ai/chat`.
//!
//! Contract under test (RESPONSE LEVEL ONLY — no internal function is
//! referenced, so the file compiles against any implementation of the gate,
//! whether it lives in `src/ai/openrouter.rs` or in `src/ai/handlers.rs`):
//! - Fresh DB (month spend = 0.00, budget = $2.00) → **200** + non-empty
//!   body (mock/demo mode: no API key configured).
//! - A manual row in `usage_metrics` with `cost_usd = 10.0` (spend ≥ budget)
//!   → **429** with a body that mentions the exhausted budget. The message
//!   wording is intentionally NOT asserted verbatim (the handler may answer
//!   with plain text or with a JSON `{"error": ...}` payload), only that the
//!   body states the budget is exhausted and leaks no internals.
//!
//! Harness: same shape as `tests/admin_routes.rs` — temp-dir SQLite running
//! the real migrations, a seeded `admin` user and a DB-backed session stored
//! as `SHA256(secret:token)` via the public auth helpers, so the auth
//! middleware accepts the request. No network calls, no student data,
//! nothing written outside the temp dir.

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

/// Administrator username seeded for the authenticated requests.
const ADMIN_USER: &str = "admin";

/// Monthly AI budget of the harness (USD). Fixed so the gate threshold is
/// deterministic.
const BUDGET_USD: f64 = 2.0;

/// Seeded spend (USD) that is over the budget: `10.0 >= 2.0`.
const OVER_BUDGET_USD: f64 = 10.0;

/// Test configuration: fixed $2.00 budget, no API key (mock mode), auth on.
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18093,
        session_secret: "ai_budget_gate_test_secret_at_least_32chars!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: None,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: BUDGET_USD,
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

/// Set up the app state: real migrations, seeded admin user and a valid
/// DB-backed session (hashed opaque token), exactly like `admin_routes.rs`.
async fn setup_app() -> TestApp {
    let notes_tmp = tempfile::tempdir().unwrap();
    let notes_dir = notes_tmp.path().to_path_buf();
    let db_tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = db_tmp.path().to_path_buf();
    let db_url = format!("sqlite://{}", db_path.display());

    let config = test_config(notes_dir, db_path);
    let secret = config.session_secret.clone();

    let pool = SqlitePoolOptions::new()
        .max_connections(4)
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

    // DB-backed session the middleware accepts: the row stores the token's
    // hash, the cookie carries the raw token.
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

/// `POST /api/ai/chat` with a valid session cookie and a JSON body.
async fn post_chat(app: axum::Router, cookie: &str) -> axum::response::Response {
    let payload = serde_json::json!({
        "message": "Hola, ¿cómo estás?",
        "subject": "general"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/ai/chat")
        .header("content-type", "application/json")
        .header("cookie", cookie)
        .body(Body::from(payload.to_string()))
        .unwrap();
    app.oneshot(req).await.unwrap()
}

/// Response body as (lossy) text — works for `text/plain` and JSON alike.
async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

// ---------------------------------------------------------------------------
// 1. Fresh database (spend = 0.00) → the call goes through.
// ---------------------------------------------------------------------------

/// With no recorded spend the request must succeed: 200 + a non-empty body
/// from the mock/demo provider (the harness has no API key).
#[tokio::test]
async fn chat_without_spend_is_allowed() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = post_chat(app, &t.cookie).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a chat with zero recorded spend must be accepted (budget = ${BUDGET_USD})"
    );

    let text = body_text(resp).await;
    assert!(
        !text.trim().is_empty(),
        "a successful chat must return a non-empty body"
    );
    let lower = text.to_lowercase();
    assert!(
        lower.contains("modo") || lower.contains("simul"),
        "mock mode answer expected in the body, got: {text}"
    );
}

// ---------------------------------------------------------------------------
// 2. Spend over the budget → the gate answers 429.
// ---------------------------------------------------------------------------

/// Seeding `usage_metrics` with `cost_usd = 10.0` (budget = $2.00) must
/// make the next chat fail with **429** and no provider call.
#[tokio::test]
async fn chat_over_budget_is_rejected_with_429() {
    let t = setup_app().await;

    // Manual spend row, current month (default `created_at` = datetime('now')).
    sqlx::query(
        "INSERT INTO usage_metrics (model, input_tokens, output_tokens, cost_usd, estimated, endpoint) \
         VALUES ('seed', 0, 0, ?, 0, 'seed')",
    )
    .bind(OVER_BUDGET_USD)
    .execute(&t.state.db)
    .await
    .expect("Failed to seed usage_metrics");

    let app = study_platform::handlers::router(t.state.clone());
    let resp = post_chat(app, &t.cookie).await;

    assert_eq!(
        resp.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "a chat above the monthly budget must be rejected with 429 \
         (spend = ${OVER_BUDGET_USD}, budget = ${BUDGET_USD})"
    );

    let text = body_text(resp).await;
    let lower = text.to_lowercase();
    assert!(
        lower.contains("presupuesto")
            || lower.contains("budget")
            || lower.contains("agotad")
            || lower.contains("exhaust"),
        "the 429 body must state that the AI budget is exhausted, got: {text}"
    );
    // No internals leaked to the client.
    assert!(
        !lower.contains("sqlite") && !lower.contains("sqlx") && !lower.contains("usage_metrics"),
        "the 429 body must not leak internals, got: {text}"
    );
}
