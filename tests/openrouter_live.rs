//! LIVE end-to-end test for Phase 4 — the OpenRouter assistant.
//!
//! Unlike the rest of the suite (mock mode, no network), this test performs a
//! REAL call to `https://openrouter.ai/api/v1/chat/completions` using the free
//! model configured in the project's `.env` (`OPENROUTER_MODEL`, default
//! `minimax/minimax-m3.1-flash:free` — $0 per call).
//!
//! Flow exercised (real router, real SQLite, real provider):
//!   1. temp DB + the project's real migrations (incl. the `admin` seed row)
//!   2. `POST /login` as `admin` → real session cookie → `require_auth_state`
//!   3. `POST /api/ai/chat` → live OpenRouter completion
//!   4. `GET /api/ai/cost` → the budget gate's persisted spend
//!
//! The test is SKIPPED (not failed) when no API key is reachable: the project
//! `.env` is loaded first (without overriding already-exported variables),
//! then `OPENROUTER_API_KEY` is read from the process environment.
//!
//! Secrets: the API key and the session cookie are never printed, logged, or
//! embedded in assertion messages.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHasher};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::AppState;

/// Model used when `OPENROUTER_MODEL` is absent from the environment.
const FALLBACK_MODEL: &str = "minimax/minimax-m3.1-flash:free";

/// Test-only admin credentials (never the real deployment ones).
const ADMIN_USER: &str = "admin";
const ADMIN_PASS: &str = "openrouter-live-test-pass-2026";

/// Test-only session secret (>= 32 chars, as the app expects).
const TEST_SESSION_SECRET: &str = "openrouter_live_test_secret_at_least_32_chars!!";

/// Token cap for the live call. The handler ignores the `max_tokens` field of
/// the JSON body (`ChatRequest` only carries `message` + `subject`), so the
/// effective cap is `Config::openrouter_max_tokens` — set to the same 40.
const MAX_TOKENS: u32 = 40;

/// Chat payload.
///
/// `subject` is REQUIRED by `ChatRequest` (src/ai/handlers.rs) — omitting it
/// yields 422. `max_tokens` is accepted-and-ignored by the handler (serde
/// drops unknown fields); it is kept in the payload to mirror the intended
/// contract, while the real cap comes from the config below.
const CHAT_BODY: &str = r#"{"message":"Di OK","subject":"general","max_tokens":40}"#;

/// Test fixture: app state + pool + the temp dir that must outlive the test.
struct LiveApp {
    state: Arc<AppState>,
    pool: sqlx::SqlitePool,
    _tmp: tempfile::TempDir,
}

/// Build the config for the live run: real API key, real model, real budget.
fn live_config(db_path: PathBuf, notes_dir: PathBuf, api_key: &str, model: &str) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18099,
        session_secret: TEST_SESSION_SECRET.to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: Some(api_key.to_string()),
        openrouter_model: model.to_string(),
        openrouter_max_tokens: MAX_TOKENS,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://127.0.0.1:8080/mcp".to_string(),
        genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        registration_token: None,
        rust_log: "test".to_string(),
        // Force the real auth path even when the project's .env has
        // AUTH_DISABLED=true: the test logs in for real.
        auth_disabled: false,
    }
}

/// Temp DB + real migrations + seeded `admin` with a known password.
async fn setup_state(api_key: &str, model: &str) -> LiveApp {
    let tmp = tempfile::tempdir().expect("temp dir");
    let db_path = tmp.path().join("openrouter-live.db");
    let notes_dir = tmp.path().join("notes");
    std::fs::create_dir_all(&notes_dir).expect("notes dir");

    let config = live_config(db_path.clone(), notes_dir, api_key, model);

    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await
        .expect("connect to temp sqlite db");

    // The project's real migrations (users, sessions, notes, chat_history,
    // usage_metrics, …) plus the `admin` seed row.
    study_platform::db::run_migrations(&pool)
        .await
        .expect("run migrations");

    // The seed creates `admin` with an empty password hash; give it a real
    // Argon2 hash so `POST /login` succeeds.
    let salt = SaltString::generate(&mut rand::thread_rng());
    let hash = Argon2::default()
        .hash_password(ADMIN_PASS.as_bytes(), &salt)
        .expect("hash test password")
        .to_string();
    sqlx::query("UPDATE users SET password_hash = ? WHERE username = ?")
        .bind(&hash)
        .bind(ADMIN_USER)
        .execute(&pool)
        .await
        .expect("set admin password");

    let state = Arc::new(AppState {
        cache_mikareno: std::sync::Arc::new(
            study_platform::academic::calendar_data::CacheMikareno::default(),
        ),
        cache_catalogo: std::sync::Arc::new(study_platform::ai::modelos::CacheCatalogo::default()),
        config,
        db: pool.clone(),
        mikareno: None,
        start_time: Instant::now(),
    });

    LiveApp {
        state,
        pool,
        _tmp: tmp,
    }
}

/// Read a response body as UTF-8 text.
async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    String::from_utf8_lossy(&bytes).to_string()
}

/// Extract the assistant text from a chat response body.
///
/// The real handler answers `text/plain` (the body IS the content). If a
/// future revision wraps it in JSON, accept the common `content`/`response`
/// keys instead of failing on the shape change.
fn extract_content(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(c) = v.get("content").and_then(|c| c.as_str()) {
            return c.to_string();
        }
        if let Some(c) = v.get("response").and_then(|c| c.as_str()) {
            return c.to_string();
        }
    }
    body.to_string()
}

/// Log in as `admin` and return the session cookie header value.
///
/// The cookie value is kept in memory only — it is never printed.
async fn login(app: &axum::Router) -> String {
    let form = format!("username={}&password={}", ADMIN_USER, ADMIN_PASS);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .expect("login request");

    assert_eq!(
        resp.status(),
        StatusCode::FOUND,
        "POST /login should redirect on success (got {})",
        resp.status()
    );

    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("login should set a session cookie");
    let pair = set_cookie.split(';').next().unwrap_or("").trim();
    let token = pair
        .strip_prefix("session_id=")
        .expect("cookie should be named session_id");
    assert!(!token.is_empty(), "session cookie must not be empty");
    format!("session_id={}", token)
}

/// Live E2E: the full Phase-4 cycle against the real OpenRouter provider.
///
/// Skipped when `OPENROUTER_API_KEY` is not available in the test environment.
#[tokio::test]
async fn test_live_openrouter_chat_cycle() {
    // Load the project .env (does NOT override already-exported variables) so
    // the live run works from the repo root without manual exports.
    let _ = dotenvy::dotenv();

    let api_key = match std::env::var("OPENROUTER_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!(
                "SKIP: OPENROUTER_API_KEY is not set in the test environment — \
                 no live OpenRouter call performed."
            );
            return;
        }
    };

    let model = std::env::var("OPENROUTER_MODEL").unwrap_or_else(|_| FALLBACK_MODEL.to_string());
    println!("live test: model={model} (cost: free tier)");

    let app_fixture = setup_state(&api_key, &model).await;
    let app = study_platform::handlers::router(app_fixture.state.clone());

    // --- 1. Real login as admin (session cookie for require_auth_state) ---
    let cookie = login(&app).await;

    // --- 2. Live chat ---
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/ai/chat")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie.clone())
                .body(Body::from(CHAT_BODY))
                .unwrap(),
        )
        .await
        .expect("chat request");

    let status = resp.status();
    let body = body_text(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "POST /api/ai/chat should return 200 from the live provider (got {status}); body: {}",
        &body.chars().take(200).collect::<String>()
    );

    let content = extract_content(&body);
    assert!(
        !content.trim().is_empty(),
        "live chat content must not be empty (provider returned an empty body)"
    );
    assert!(
        !content.contains("[Modo simulado]"),
        "response came from mock mode — the API key was not used"
    );
    println!(
        "live test: model replied with {} chars",
        content.chars().count()
    );

    // --- 3. Usage row persisted by the budget gate ---
    let rows: Vec<(String, i64, i64, f64, i64)> = sqlx::query_as(
        "SELECT model, input_tokens, output_tokens, cost_usd, estimated \
         FROM usage_metrics WHERE endpoint = 'chat'",
    )
    .fetch_all(&app_fixture.pool)
    .await
    .expect("query usage_metrics");

    assert_eq!(
        rows.len(),
        1,
        "the live call must record exactly one usage row, got {}",
        rows.len()
    );
    let (used_model, input_tokens, output_tokens, cost_usd, _estimated) = &rows[0];
    assert_eq!(
        used_model, &model,
        "the recorded model must be the configured one"
    );
    assert!(
        *cost_usd >= 0.0,
        "cost_usd must be >= 0 (free model), got {cost_usd}"
    );
    assert!(
        *input_tokens > 0 && *output_tokens > 0,
        "the provider must report real token usage (in={input_tokens}, out={output_tokens})"
    );
    println!("live test: usage in={input_tokens} out={output_tokens} cost_usd={cost_usd}");

    // --- 4. The budget gate's persisted spend ---
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/ai/cost")
                .header(header::COOKIE, cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("cost request");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /api/ai/cost should return 200"
    );

    let json: serde_json::Value =
        serde_json::from_str(&body_text(resp).await).expect("cost body should be JSON");
    // The endpoint names the field `total_cost`; accept `spend_usd` too in
    // case the contract is renamed later.
    let spend = json
        .get("spend_usd")
        .or_else(|| json.get("total_cost"))
        .and_then(|v| v.as_f64())
        .expect("cost payload should expose the monthly spend as a number");
    assert!(spend >= 0.0, "spend_usd must be >= 0.0, got {spend}");
    let requests = json
        .get("requests_count")
        .and_then(|v| v.as_u64())
        .expect("cost payload should expose requests_count");
    assert!(
        requests >= 1,
        "the live call must be counted in requests_count (got {requests})"
    );
    println!("live test: month spend={spend} requests={requests}");

    let _ = api_key; // never printed
}
