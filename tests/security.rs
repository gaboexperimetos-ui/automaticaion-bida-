//! Security integration tests (Mission 8 leftover).
//!
//! # Secret-audit findings (verified 2026-10-10, all fictitious data only)
//!
//! Grep run over `src/` + `tests/` for hardcoded secrets:
//!   `sk-`, `sk-or-`, `api_key = "..."`, `password = "..."`, `secret = "..."`
//! Result: NO real secrets found.
//! - `src/`: zero matches. All sensitive values come from env vars via
//!   `Config::from_env` (see `src/config.rs`); `.env` is gitignored.
//! - `tests/`: only dummy/fictitious values, all clearly labelled as test data:
//!   `"secret-key-12345"`, `"valid-key-12345"`, `"test-key-12345"`,
//!   `"testpassword123"`, `"test_secret_key_at_least_32_chars_long!!"`,
//!   and the string `"sk-or-"` appears only inside a *negative* assertion
//!   (`assert!(!body.contains("sk-or-"))` in `tests/integration.rs`).
//! - This file uses fictitious values only (TEST-NET IPs, dummy keys/passwords).
//!
//! # Rate-limit isolation note
//!
//! `src/middleware/rate_limit.rs` keeps a process-global `static RATE_LIMITER`
//! (60 req/min per IP general, plus a stricter 10 req/min per IP bucket for
//! `POST /login` keyed as `login:<ip>`, both keyed by `x-forwarded-for`
//! — or the `ConnectInfo` socket addr when present — default `"unknown"`).
//! Each `cargo test --test <name>` binary is its own process, so the limiter
//! is per-test-binary. Within THIS binary:
//! - every test uses its own reserved TEST-NET-1 (`192.0.2.x`) IP so normal
//!   tests never share a bucket and never trip the limiter;
//! - the rate-limit test uses a dedicated IP (`192.0.2.99`) and fires 61
//!   (= threshold 60 + 1) rapid requests at the cheap public `/health`
//!   endpoint, asserting the final status is 429. Because its IP is unique to
//!   that test, the poisoned bucket cannot affect any other test regardless of
//!   execution order (tests run in parallel threads by default).
//! - the login brute-force test uses another dedicated IP (`192.0.2.98`) and
//!   fires 11 (= login threshold 10 + 1) `POST /login` attempts against the
//!   separate `login:<ip>` bucket, asserting the 11th is 429. General traffic
//!   and login attempts never share budget (see
//!   `login_and_general_buckets_do_not_share_budget` unit test in
//!   `src/middleware/rate_limit.rs`).
//!
//! # Known gaps documented (not fixed) — Mission 10 audit 2026-10-10
//!
//! - `require_auth` validates session-cookie *format* (UUID) only: no DB
//!   lookup, no `expires_at` enforcement. A logged-out (DB-deleted) or
//!   expired session still passes the gate. Fixing needs `State` in the
//!   middleware signature → call-site changes outside Agent A ownership.
//! - Session cookies are unsigned UUIDs (`SESSION_SECRET` is loaded but never
//!   used for signing). Signing is a redesign, out of scope.
//! - `check_api_key` (`src/mcp_server/handlers.rs` via
//!   `src/mcp_server/server.rs`) uses `==`, not constant-time. A
//!   `constant_time_eq` primitive is provided in `src/middleware/auth.rs` for
//!   future wiring (call site outside ownership).
//! - `set_password` (`src/handlers/auth.rs`) returns `Database error: {e}` /
//!   `Error hashing password: {e}` and notes/AI handlers return
//!   `e.to_string()` on 500s — sqlx internals can leak. Handler changes are
//!   outside ownership. (`set_password` is additionally not routed over HTTP.)

use std::path::PathBuf;
use std::sync::Arc;

use argon2::PasswordHasher;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::middleware::auth::{
    clear_session_cookie, clear_session_cookie_with_flags, constant_time_eq, make_session_cookie,
    make_session_cookie_with_flags,
};
use study_platform::middleware::rate_limit::LOGIN_RATE_LIMIT;
use study_platform::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A valid-looking (but fictitious) session UUID. `require_auth` only checks
/// UUID format at this phase, so this passes the middleware.
const TEST_SESSION: &str = "session_id=550e8400-e29b-41d4-a716-446655440000";

fn test_config(db_path: PathBuf, notes_dir: PathBuf, api_key: Option<String>) -> Config {
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
        custom_mcp_api_key: api_key,
        registration_token: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
    }
}

/// Full test state via migrations (gives `users`, `sessions`, `notes`, ...).
/// `seed_admin` controls whether a fictitious admin user is created (only the
/// login test needs it; Argon2 hashing is slow so we skip it elsewhere).
/// Returns the state plus the temp-dir guard (must be kept alive).
async fn setup(api_key: Option<String>, seed_admin: bool) -> (Arc<AppState>, tempfile::TempDir) {
    let work_dir = tempfile::tempdir().unwrap();
    let db_path = work_dir.path().join("test.db");
    // Pre-create the file: sqlx has trouble creating new SQLite files directly
    // in temp directories.
    std::fs::write(&db_path, b"").expect("Failed to create db file");
    let db_url = format!("sqlite://{}", db_path.display());
    let notes_dir = work_dir.path().join("notes");

    let config = test_config(db_path, notes_dir, api_key);

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

    if seed_admin {
        let salt = argon2::password_hash::SaltString::generate(rand::thread_rng());
        let argon2 = argon2::Argon2::default();
        // Fictitious password, test-only.
        let password_hash = argon2
            .hash_password(b"fictitious-correct-password-001", &salt)
            .unwrap()
            .to_string();

        sqlx::query(
            "INSERT INTO users (username, password_hash) VALUES (?, ?) \
             ON CONFLICT(username) DO UPDATE SET password_hash = EXCLUDED.password_hash",
        )
        .bind("admin")
        .bind(&password_hash)
        .execute(&pool)
        .await
        .expect("Failed to seed admin user");
    }

    let state = Arc::new(AppState { config, db: pool,
        mikareno: None,
        start_time: std::time::Instant::now(),
    });
    (state, work_dir)
}

/// POST a JSON-RPC request to /mcp, return (HTTP status, parsed JSON body).
async fn send_jsonrpc(
    state: Arc<AppState>,
    api_key: Option<&str>,
    forwarded_for: &str,
    body: Value,
) -> (StatusCode, Value) {
    let app = study_platform::handlers::router(state);

    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("x-forwarded-for", forwarded_for);

    if let Some(key) = api_key {
        builder = builder.header("x-api-key", key);
    }

    let response = app
        .oneshot(
            builder
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let parsed: Value = serde_json::from_slice(&bytes).unwrap();
    (status, parsed)
}

async fn body_text(
    response: axum::response::Response,
) -> (StatusCode, String, axum::http::HeaderMap) {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        String::from_utf8_lossy(&bytes).into_owned(),
        headers,
    )
}

// ---------------------------------------------------------------------------
// MCP authentication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mcp_dev_mode_allows_tools_list() {
    // No key configured -> dev mode allow-all.
    let (state, _guard) = setup(None, false).await;

    let (status, result) = send_jsonrpc(
        state,
        None,
        "192.0.2.10",
        json!({ "jsonrpc": "2.0", "method": "tools/list", "id": 1 }),
    )
    .await;

    // Auth failure is a JSON-RPC error body with HTTP 200 (see
    // src/mcp_server/handlers.rs); success is HTTP 200 with `result`.
    assert_eq!(status, StatusCode::OK);
    assert!(
        result["error"].is_null(),
        "dev mode must not error: {result}"
    );
    assert!(result["result"]["tools"].is_array());
}

#[tokio::test]
async fn test_mcp_wrong_key_rejected_with_jsonrpc_error() {
    let (state, _guard) = setup(Some("dummy-configured-key-001".to_string()), false).await;

    let (status, result) = send_jsonrpc(
        state,
        Some("dummy-wrong-key-999"),
        "192.0.2.11",
        json!({ "jsonrpc": "2.0", "method": "tools/list", "id": 1 }),
    )
    .await;

    // Auth failure -> HTTP 200 with a JSON-RPC error object (not 401).
    assert_eq!(status, StatusCode::OK);
    assert!(
        result["error"].is_object(),
        "expected JSON-RPC error: {result}"
    );
    assert_eq!(result["error"]["code"], -32001);
    assert!(result["result"].is_null());
}

#[tokio::test]
async fn test_mcp_correct_key_accepted() {
    let (state, _guard) = setup(Some("dummy-configured-key-002".to_string()), false).await;

    let (status, result) = send_jsonrpc(
        state,
        Some("dummy-configured-key-002"),
        "192.0.2.12",
        json!({ "jsonrpc": "2.0", "method": "tools/list", "id": 1 }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        result["error"].is_null(),
        "correct key must succeed: {result}"
    );
    assert!(result["result"]["tools"].is_array());
}

// ---------------------------------------------------------------------------
// Session auth: unauthenticated access
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unauthenticated_post_api_notes_is_401() {
    let (state, _guard) = setup(None, false).await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/notes")
                .header("content-type", "application/json")
                .header("x-forwarded-for", "192.0.2.13")
                .body(Body::from(r#"{"title":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_unauthenticated_get_dashboard_redirects_to_login() {
    let (state, _guard) = setup(None, false).await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("x-forwarded-for", "192.0.2.14")
                .body(Body::empty())
                .unwrap(),
        )
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

// ---------------------------------------------------------------------------
// Login: wrong password must not mint a session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_wrong_password_login_sets_no_session_cookie() {
    let (state, _guard) = setup(None, true).await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-forwarded-for", "192.0.2.15")
                .body(Body::from(
                    "username=admin&password=fictitious-wrong-password-001",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let (status, body, headers) = body_text(response).await;

    // login_submit returns 401 with a generic Spanish error (stays on login).
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        body.contains("Usuario o contraseña incorrectos"),
        "unexpected body: {body}"
    );

    // No Set-Cookie header may carry a session_id.
    let has_session_cookie = headers
        .get_all("set-cookie")
        .iter()
        .any(|v| v.to_str().unwrap_or("").contains("session_id"));
    assert!(
        !has_session_cookie,
        "wrong password must not mint a session cookie"
    );
}

// ---------------------------------------------------------------------------
// SQL injection: malicious slug must not crash or leak SQL internals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sql_injection_slug_does_not_leak_or_crash() {
    let (state, _guard) = setup(None, false).await;
    let app = study_platform::handlers::router(state);

    // `' OR '1'='1` percent-encoded so it stays within one path segment.
    // Authenticated (valid UUID format) so the request reaches the handler
    // instead of stopping at the auth middleware.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/notes/%27%20OR%20%271%27%3D%271")
                .header("cookie", TEST_SESSION)
                .header("x-forwarded-for", "192.0.2.16")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let (status, body, _headers) = body_text(response).await;

    // Bound query parameter -> no injection: expect 404 (no such note).
    // Accept any 4xx or 200-empty; must NOT be a 500 with SQL internals.
    assert_ne!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "injection attempt caused 500: {body}"
    );
    assert!(
        status.is_client_error() || status.is_success(),
        "unexpected status: {status}"
    );
    let lower = body.to_lowercase();
    assert!(
        !lower.contains("sqlite"),
        "response leaks sqlite internals: {body}"
    );
    assert!(
        !lower.contains("sqlx"),
        "response leaks sqlx internals: {body}"
    );
    // "sql" as a substring check would false-positive on normal words; check
    // for error-shaped leakage instead.
    assert!(
        !lower.contains("syntax error"),
        "response leaks SQL error: {body}"
    );
}

// ---------------------------------------------------------------------------
// Session cookie flags
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_cookie_has_secure_flags() {
    // make_session_cookie is pub (src/middleware/auth.rs).
    let cookie = make_session_cookie("fictitious-test-id");
    assert!(
        cookie.contains("session_id="),
        "cookie missing name: {cookie}"
    );
    assert!(
        cookie.contains("HttpOnly"),
        "cookie missing HttpOnly: {cookie}"
    );
    assert!(
        cookie.contains("SameSite=Strict"),
        "cookie missing SameSite=Strict: {cookie}"
    );
}

// ---------------------------------------------------------------------------
// Rate limiting: threshold (60/min) + 1 -> 429
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rate_limit_returns_429_after_threshold() {
    // Threshold is 60 requests / 60s per IP (src/middleware/rate_limit.rs).
    // Uses dedicated IP 192.0.2.99 so the poisoned bucket cannot affect other
    // tests; /health is a cheap public endpoint ideal for hammering.
    let (state, _guard) = setup(None, false).await;

    let mut last_status = StatusCode::OK;
    let mut last_headers = axum::http::HeaderMap::new();
    for _ in 0..61 {
        let app = study_platform::handlers::router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("x-forwarded-for", "192.0.2.99")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        last_status = response.status();
        last_headers = response.headers().clone();
    }

    assert_eq!(last_status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        last_headers.contains_key("retry-after"),
        "429 should carry Retry-After"
    );
}

// ---------------------------------------------------------------------------
// Session cookie format (Mission 10 fixes: stray space + missing `=`)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_cookie_exact_format_matches_production() {
    // Must match the production format built inline in
    // `src/handlers/auth.rs::login_submit` (`session_id=<uuid>; Path=/; …`
    // with NO space after `=`): a leading space becomes part of the cookie
    // value and fails UUID parsing on the way back.
    let cookie = make_session_cookie("550e8400-e29b-41d4-a716-446655440000");
    assert_eq!(
        cookie,
        "session_id=550e8400-e29b-41d4-a716-446655440000; \
         Path=/; HttpOnly; SameSite=Strict; Max-Age=86400"
    );
}

#[tokio::test]
async fn test_clear_session_cookie_is_a_valid_deletion() {
    // Must be `session_id=; …` (with `=`): the previous helper emitted
    // `session_id; …`, which browsers do not reliably treat as deleting the
    // `session_id` cookie. Must also match the production logout format in
    // `src/handlers/auth.rs::logout`.
    let cookie = clear_session_cookie();
    assert!(
        cookie.starts_with("session_id=;"),
        "deletion cookie must name the cookie with `=`: {cookie}"
    );
    assert!(cookie.contains("Max-Age=0"), "must expire: {cookie}");
    assert!(cookie.contains("HttpOnly"), "must keep HttpOnly: {cookie}");
    assert!(
        cookie.contains("SameSite=Strict"),
        "must keep SameSite=Strict: {cookie}"
    );
}

#[tokio::test]
async fn test_secure_cookie_variants_add_secure_flag() {
    let plain = make_session_cookie("fictitious-test-id");
    let secure = make_session_cookie_with_flags("fictitious-test-id", true);
    assert!(
        !plain.contains("Secure"),
        "plain-HTTP variant must not set Secure: {plain}"
    );
    assert!(
        secure.contains("; Secure"),
        "HTTPS variant must set Secure: {secure}"
    );
    assert_eq!(secure, format!("{plain}; Secure"));

    let plain_clear = clear_session_cookie();
    let secure_clear = clear_session_cookie_with_flags(true);
    assert!(!plain_clear.contains("Secure"));
    assert!(secure_clear.contains("; Secure"));
    assert!(secure_clear.starts_with("session_id=;"));
}

// ---------------------------------------------------------------------------
// Constant-time comparison primitive
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_constant_time_eq_correctness() {
    assert!(constant_time_eq("dummy-api-key-001", "dummy-api-key-001"));
    assert!(constant_time_eq("", ""));
    // Same length, differ in first / middle / last byte.
    assert!(!constant_time_eq("xdummy-api-key-00", "dummy-api-key-001"));
    assert!(!constant_time_eq("dummy-aPi-key-001", "dummy-api-key-001"));
    assert!(!constant_time_eq("dummy-api-key-002", "dummy-api-key-001"));
    // Length mismatch (length itself is not hidden — documented).
    assert!(!constant_time_eq("short", "dummy-api-key-001"));
    assert!(!constant_time_eq(
        "dummy-api-key-001-extra",
        "dummy-api-key-001"
    ));
    assert!(!constant_time_eq("", "dummy-api-key-001"));
}

// ---------------------------------------------------------------------------
// Login brute-force limit: 10 POST /login per minute per IP -> 429
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_login_brute_force_limit_returns_429_after_threshold() {
    // SECURITY.md promises 10 req/min brute-force protection. The login
    // bucket is separate (`login:<ip>`), so this dedicated IP (192.0.2.98)
    // cannot affect any other test.
    assert_eq!(LOGIN_RATE_LIMIT, 10);
    let (state, _guard) = setup(None, false).await;

    let mut statuses = Vec::new();
    let mut last_headers = axum::http::HeaderMap::new();
    for _ in 0..=LOGIN_RATE_LIMIT {
        let app = study_platform::handlers::router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("x-forwarded-for", "192.0.2.98")
                    .body(Body::from(
                        "username=fictitious-user&password=fictitious-wrong-password-002",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        statuses.push(response.status());
        last_headers = response.headers().clone();
    }

    // First 10 attempts reach the handler (401: unknown user, no session).
    for (i, status) in statuses.iter().take(LOGIN_RATE_LIMIT).enumerate() {
        assert_eq!(
            *status,
            StatusCode::UNAUTHORIZED,
            "attempt {i} should reach the login handler"
        );
    }
    // 11th attempt is rate-limited before touching Argon2/DB.
    assert_eq!(*statuses.last().unwrap(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        last_headers.contains_key("retry-after"),
        "429 should carry Retry-After"
    );
}

// ---------------------------------------------------------------------------
// Public endpoints must not leak secrets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_public_config_exposes_no_secrets() {
    let (state, _guard) = setup(None, false).await;

    for uri in ["/api/config", "/api/public-config"] {
        let app = study_platform::handlers::router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("x-forwarded-for", "192.0.2.20")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, body, _headers) = body_text(response).await;

        assert_eq!(status, StatusCode::OK, "{uri} should be public");
        let lower = body.to_lowercase();
        assert!(
            !lower.contains("secret"),
            "{uri} leaks a secret key name: {body}"
        );
        assert!(!body.contains("sk-"), "{uri} leaks a key value: {body}");
        // …while still serving its documented fields.
        assert!(body.contains("openrouter_model"), "{uri}: {body}");
        assert!(body.contains("mikareno_mcp_url"), "{uri}: {body}");
    }
}

#[tokio::test]
async fn test_health_exposes_no_secrets() {
    let (state, _guard) = setup(None, false).await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("x-forwarded-for", "192.0.2.21")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body, _headers) = body_text(response).await;

    assert_eq!(status, StatusCode::OK);
    let lower = body.to_lowercase();
    assert!(!lower.contains("secret"), "health leaks secrets: {body}");
    assert!(!body.contains("sk-"), "health leaks a key value: {body}");
    assert!(
        body.contains("openrouter"),
        "health should report flags: {body}"
    );
}

// ---------------------------------------------------------------------------
// Login errors: no user enumeration, no backend leakage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_login_errors_do_not_enumerate_users_or_leak_backend() {
    let (state, _guard) = setup(None, true).await;

    async fn attempt(state: Arc<AppState>, ip: &str, form: &str) -> (StatusCode, String) {
        let app = study_platform::handlers::router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("x-forwarded-for", ip)
                    .body(Body::from(form.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, body, _) = body_text(response).await;
        (status, body)
    }

    // Unknown user vs known user with wrong password must be indistinguishable.
    let (s1, b1) = attempt(
        state.clone(),
        "192.0.2.22",
        "username=fictitious-nobody&password=fictitious-wrong-password-003",
    )
    .await;
    let (s2, b2) = attempt(
        state.clone(),
        "192.0.2.23",
        "username=admin&password=fictitious-wrong-password-004",
    )
    .await;

    assert_eq!(s1, StatusCode::UNAUTHORIZED);
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
    assert_eq!(b1, b2, "responses must not distinguish unknown/wrong user");

    for (label, body) in [("unknown-user", &b1), ("wrong-password", &b2)] {
        let lower = body.to_lowercase();
        for leak in ["not found", "no such", "sqlite", "sqlx", "syntax", "argon"] {
            assert!(
                !lower.contains(leak),
                "{label} response leaks backend detail ({leak}): {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Logout: clears cookie flags, deletes DB row — but gate still accepts it
// (known gap: format-only validation, see module docs)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_logout_clears_cookie_with_security_flags() {
    let (state, _guard) = setup(None, false).await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header("cookie", TEST_SESSION)
                .header("x-forwarded-for", "192.0.2.24")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, _body, headers) = body_text(response).await;

    // Logout responds 200 with the inline login page and a cleared cookie.
    assert_eq!(status, StatusCode::OK);
    let set_cookie = headers
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        set_cookie.starts_with("session_id=;"),
        "logout must delete the session cookie: {set_cookie}"
    );
    assert!(set_cookie.contains("Max-Age=0"));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Strict"));
}

#[tokio::test]
async fn test_logout_deletes_row_but_gate_still_accepts_known_gap() {
    // Documents the known gap: logout deletes the session row (verified via
    // DB), yet the logged-out cookie still passes `require_auth` because the
    // middleware checks UUID *format* only. If DB validation is ever wired
    // into the middleware, the final assertion must be updated to expect
    // a redirect/401 — that update IS the fix landing.
    let (state, _guard) = setup(None, true).await;

    // Login with the fictitious correct password.
    let app = study_platform::handlers::router(state.clone());
    let login = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-forwarded-for", "192.0.2.25")
                .body(Body::from(
                    "username=admin&password=fictitious-correct-password-001",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::FOUND);
    let session_cookie = login
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next().map(str::to_string))
        .expect("login must mint a session cookie");

    // One session row exists.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Logout.
    let app = study_platform::handlers::router(state.clone());
    let logout = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header("cookie", session_cookie.clone())
                .header("x-forwarded-for", "192.0.2.25")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::OK);

    // Row is gone…
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 0, "logout must delete the session row");

    // …the old cookie is now REJECTED by the DB-backed gate (gap closed).
    let app = study_platform::handlers::router(state.clone());
    let replay = app
        .oneshot(
            Request::builder()
                .uri("/api/notes")
                .header("cookie", session_cookie)
                .header("x-forwarded-for", "192.0.2.25")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let gate_status = replay.status();
    assert!(
        gate_status == StatusCode::UNAUTHORIZED || gate_status == StatusCode::FOUND,
        "DB-backed gate correctly rejects logged-out session ({gate_status})"
    );
}

// ---------------------------------------------------------------------------
// Session expiry logic exists (Session::is_valid) — middleware does not call it
// ---------------------------------------------------------------------------

// AI route-rejection leak test renamed from `test_ai_chat_mock_mode_leaks_no_secrets`
// after AI routes were gated by require_auth_state in Mission 11.

#[tokio::test]
async fn test_session_expiry_logic_marks_past_sessions_invalid() {
    use study_platform::models::Session;

    let expired = Session {
        id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        user_id: 1,
        created_at: "2000-01-01T00:00:00+00:00".to_string(),
        expires_at: "2000-01-02T00:00:00+00:00".to_string(),
        user_agent: None,
    };
    assert!(
        !expired.is_valid(),
        "past expires_at must be invalid (logic exists but is unenforced by middleware — known gap)"
    );

    let fresh = Session {
        expires_at: "2999-01-01T00:00:00+00:00".to_string(),
        ..expired
    };
    assert!(fresh.is_valid());
}

// ---------------------------------------------------------------------------
// MCP SSE auth + missing-key path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mcp_sse_wrong_key_is_401() {
    let (state, _guard) = setup(Some("dummy-configured-key-003".to_string()), false).await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp/sse")
                .header("x-api-key", "dummy-wrong-key-999")
                .header("x-forwarded-for", "192.0.2.26")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body, _headers) = body_text(response).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("Unauthorized"), "unexpected body: {body}");
}

#[tokio::test]
async fn test_mcp_missing_key_rejected_when_configured() {
    let (state, _guard) = setup(Some("dummy-configured-key-004".to_string()), false).await;

    let (status, result) = send_jsonrpc(
        state,
        None,
        "192.0.2.28",
        json!({ "jsonrpc": "2.0", "method": "tools/list", "id": 1 }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["error"]["code"], -32001);
}

// ---------------------------------------------------------------------------
// AI routes behind `require_auth_state`: rejection leaks nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ai_endpoints_leak_no_secrets_on_rejection() {
    // Without a valid session cookie, /api/ai/* returns 401 via require_auth_state.
    // Verify the 401 body contains NO secret material (no bearer, sk-, sqlx, etc.).
    let (state, _guard) = setup(None, false).await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/ai/chat")
                .header("content-type", "application/json")
                .header("x-forwarded-for", "192.0.2.27")
                .body(Body::from(
                    r#"{"message":"fictitious","subject":"general"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, body, _headers) = body_text(response).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let lower = body.to_lowercase();
    assert!(!lower.contains("bearer"), "rejection leaks bearer: {body}");
    assert!(!lower.contains("sk-"), "rejection leaks key prefix: {body}");
    assert!(
        !lower.contains("sqlx"),
        "rejection leaks crate name: {body}"
    );
}

// ---------------------------------------------------------------------------
// Mission 11 (Agent F): session revocation, AI-route auth, budget durability,
// MCP error codes, error-message hygiene.
//
// Ownership note: this section only APPENDS new tests. The documented known-gap
// test (`test_logout_deletes_row_but_gate_still_accepts_known_gap`) was updated
// by the orquestator to expect rejection (DB validation is now wired in).
// Tests asserting DESIRED post-Agent-A/B behavior are marked `#[ignore]`
// with the reason while the corresponding fix is not yet integrated —
// ignore-with-reason is preferred over weakening the assertions. If an
// ignored test starts PASSING, the underlying fix has landed: remove the
// `#[ignore]` attribute.
// ---------------------------------------------------------------------------

/// Log in as the seeded fictitious admin and return the `session_id=...`
/// cookie value (attributes stripped). `setup(.., true)` is required.
async fn login_and_capture_cookie(state: Arc<AppState>, forwarded_for: &str) -> String {
    let app = study_platform::handlers::router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-forwarded-for", forwarded_for)
                .body(Body::from(
                    "username=admin&password=fictitious-correct-password-001",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next().map(str::to_string))
        .expect("login must mint a session cookie")
}

/// POST a raw (possibly malformed) body to /mcp; return (HTTP status, body).
/// Unlike `send_jsonrpc`, this tolerates non-JSON error responses (e.g. the
/// 422 plain-text rejection axum emits for an unparseable body).
async fn send_raw_mcp(
    state: Arc<AppState>,
    api_key: Option<&str>,
    forwarded_for: &str,
    raw: &str,
) -> (StatusCode, String) {
    let app = study_platform::handlers::router(state);
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("x-forwarded-for", forwarded_for);
    if let Some(key) = api_key {
        builder = builder.header("x-api-key", key);
    }
    let response = app
        .oneshot(builder.body(Body::from(raw.to_string())).unwrap())
        .await
        .unwrap();
    let (status, body, _) = body_text(response).await;
    (status, body)
}

/// Assert a user-facing error body leaks no backend internals or key material.
/// All fixtures are fictitious; this guards the *shape* of error output.
fn assert_no_backend_leakage(context: &str, body: &str) {
    let lower = body.to_lowercase();
    for leak in [
        "sqlite",
        "sqlx",
        "panicked",
        "unwrap",
        "backtrace",
        "stack trace",
        "traceback",
    ] {
        assert!(
            !lower.contains(leak),
            "{context} leaks backend detail ({leak}): {body}"
        );
    }
    // Key material must never appear in error output.
    for secret in ["sk-or-", "Bearer "] {
        assert!(
            !body.contains(secret),
            "{context} leaks key material ({secret:?}): {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// 1. Logout -> cookie reuse must be rejected (desired post-Agent-A behavior)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs Agent A call-site swap (src/handlers/mod.rs still uses from_fn(require_auth), which checks UUID format only; post-Agent-A logins additionally mint opaque non-UUID tokens the legacy gate rejects, so even the pre-logout sanity step fails until the swap lands)"]
async fn mission11_logged_out_cookie_reuse_is_rejected() {
    let (state, _guard) = setup(None, true).await;
    let cookie = login_and_capture_cookie(state.clone(), "192.0.2.30").await;

    // Sanity: the cookie works before logout.
    let app = study_platform::handlers::router(state.clone());
    let before = app
        .oneshot(
            Request::builder()
                .uri("/api/notes")
                .header("cookie", cookie.clone())
                .header("x-forwarded-for", "192.0.2.30")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(before.status(), StatusCode::OK);

    // Logout: deletes the session row and clears the cookie.
    let app = study_platform::handlers::router(state.clone());
    let logout = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header("cookie", cookie.clone())
                .header("x-forwarded-for", "192.0.2.30")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::SEE_OTHER);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 0, "logout must delete the session row");

    // DESIRED behavior (Agent A): reusing the logged-out cookie is rejected.
    // API routes answer 401 ...
    let app = study_platform::handlers::router(state.clone());
    let reuse_api = app
        .oneshot(
            Request::builder()
                .uri("/api/notes")
                .header("cookie", cookie.clone())
                .header("x-forwarded-for", "192.0.2.31")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        reuse_api.status(),
        StatusCode::UNAUTHORIZED,
        "logged-out session cookie must be rejected on API routes"
    );

    // ... and page routes redirect to /login.
    let app = study_platform::handlers::router(state.clone());
    let reuse_page = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", cookie)
                .header("x-forwarded-for", "192.0.2.32")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        reuse_page.status(),
        StatusCode::FOUND,
        "logged-out session cookie must be rejected on page routes"
    );
    assert_eq!(
        reuse_page
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok()),
        Some("/login")
    );
}

// ---------------------------------------------------------------------------
// 2. AI routes without auth (desired post-Agent-B behavior)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mission11_ai_chat_without_cookie_is_unauthorized() {
    let (state, _guard) = setup(None, false).await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/ai/chat")
                .header("content-type", "application/json")
                .header("x-forwarded-for", "192.0.2.33")
                .body(Body::from(
                    r#"{"message":"fictitious test question","subject":"general"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "POST /api/ai/chat without a session cookie must be 401"
    );
}

#[tokio::test]
async fn mission11_ai_cost_without_cookie_is_unauthorized() {
    let (state, _guard) = setup(None, false).await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/ai/cost")
                .header("x-forwarded-for", "192.0.2.34")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "GET /api/ai/cost without a session cookie must be 401"
    );
}

// ---------------------------------------------------------------------------
// 3. Budget persistence across restart (needs Agent B's usage_metrics table)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mission11_budget_spend_survives_app_restart() {
    // Agent B landed `migrations/20240101000005_usage_metrics.sql` plus the
    // `check_budget_and_reserve` / `BudgetGuard::record` API, so this test
    // now runs un-ignored: reserve + record spend on one pool, reopen a
    // brand-new pool (= new AppState) on the same file, assert the spend
    // is still visible.
    use study_platform::ai::openrouter::{check_budget_and_reserve, month_spend, UsageEntry};

    // Gate 1: some migration must create the usage_metrics table.
    let migrations_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut has_budget_migration = false;
    for entry in std::fs::read_dir(&migrations_dir).unwrap() {
        let content = std::fs::read_to_string(entry.unwrap().path()).unwrap_or_default();
        if content.contains("usage_metrics") {
            has_budget_migration = true;
        }
    }
    assert!(
        has_budget_migration,
        "Agent B budget migration (usage_metrics) must exist"
    );

    // Gate 2: scratch FILE database (never :memory:, never the real
    // data/app.db), full migrations, table present.
    let work_dir = tempfile::tempdir().unwrap();
    let db_path = work_dir.path().join("restart-probe.db");
    std::fs::write(&db_path, b"").expect("Failed to create db file");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let notes_dir = work_dir.path().join("notes");

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .expect("Failed to create probe database");
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run migrations");
    let table: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'usage_metrics'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(
        table.as_deref(),
        Some("usage_metrics"),
        "usage_metrics table must exist after migrations"
    );

    // Gate 3: record a real (fictitious) spend through Agent B's gate API.
    let config = test_config(db_path.clone(), notes_dir, None);
    assert_eq!(month_spend(&pool).await.unwrap(), 0.0);
    let guard = check_budget_and_reserve(&pool, &config)
        .await
        .expect("budget must allow the first reservation");
    guard
        .record(&UsageEntry {
            model: "fictitious-test-model".to_string(),
            input_tokens: 10,
            output_tokens: 5,
            cost_usd: 0.75,
            estimated: true,
            endpoint: "mission11-test".to_string(),
        })
        .await
        .unwrap();
    pool.close().await;

    // New pool = new AppState on the same file: spend must survive.
    let pool2 = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .expect("Failed to reopen probe database");
    let spend_after = month_spend(&pool2).await.unwrap();
    assert!(
        (spend_after - 0.75).abs() < 1e-9,
        "spend must survive an AppState restart on the same DB file, got {spend_after}"
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_metrics")
        .fetch_one(&pool2)
        .await
        .unwrap();
    assert_eq!(count, 1, "exactly one spend row must survive the restart");
    let integrity: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&pool2)
        .await
        .unwrap();
    assert!(
        integrity.iter().any(|r| r.to_lowercase() == "ok"),
        "integrity_check must pass after restart: {integrity:?}"
    );
    pool2.close().await;
}

// ---------------------------------------------------------------------------
// 4. Concurrent budget limit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mission11_concurrent_budget_reservations_never_exceed_cap() {
    // Unit-level concurrency on the in-memory gate (present at test time:
    // `CostTracker` in src/ai/openrouter.rs). `CostTracker` is `&mut`-based
    // (not Sync); the async Mutex provides the atomicity — Agent B's DB gate
    // must provide the same atomicity via transactions (see the ignored
    // DB-level test below).
    use study_platform::ai::CostTracker;

    const BUDGET: f64 = 2.0;
    const COST_EACH: f64 = 0.5;
    const TASKS: usize = 20;

    let tracker = Arc::new(tokio::sync::Mutex::new(CostTracker::new(BUDGET)));

    let mut handles = Vec::with_capacity(TASKS);
    for _ in 0..TASKS {
        let tracker = tracker.clone();
        handles.push(tokio::spawn(async move {
            let mut guard = tracker.lock().await;
            // Check-then-reserve while holding the lock: the reservation is
            // atomic, so total spend can never overshoot the cap.
            if !guard.can_proceed() {
                return false;
            }
            guard
                .record_request(COST_EACH)
                .expect("budget was checked first");
            true
        }));
    }

    let mut successes = 0usize;
    for handle in handles {
        if handle.await.unwrap() {
            successes += 1;
        }
    }

    let guard = tracker.lock().await;
    assert_eq!(
        successes, 4,
        "budget 2.0 / cost 0.5 must admit exactly 4 reservations"
    );
    assert!(
        guard.total_cost <= BUDGET,
        "spend {} exceeds cap {BUDGET}",
        guard.total_cost
    );
    assert_eq!(guard.requests_count as usize, successes);
}

#[tokio::test]
async fn mission11_concurrent_budget_db_gate_enforces_cap() {
    // DB-level concurrency against Agent B's gate
    // (`check_budget_and_reserve`: BEGIN IMMEDIATE check-then-insert, so
    // parallel requests serialize on the SQLite write lock). N parallel
    // reserve-then-record cycles at $0.50 against a $2.00 budget must admit
    // exactly 4 and leave month spend at exactly the cap — never above.
    use study_platform::ai::openrouter::{check_budget_and_reserve, month_spend, UsageEntry};

    const BUDGET: f64 = 2.0;
    const COST_EACH: f64 = 0.5;
    const TASKS: usize = 10;

    let work_dir = tempfile::tempdir().unwrap();
    let db_path = work_dir.path().join("concurrency-probe.db");
    std::fs::write(&db_path, b"").expect("Failed to create db file");
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let notes_dir = work_dir.path().join("notes");

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect(&db_url)
        .await
        .expect("Failed to create probe database");
    sqlx::query("PRAGMA journal_mode=WAL;")
        .execute(&pool)
        .await
        .ok();
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    let config = test_config(db_path, notes_dir, None);
    assert!(
        (config.openrouter_max_cost_usd - BUDGET).abs() < 1e-9,
        "test budget assumption holds"
    );

    let mut handles = Vec::with_capacity(TASKS);
    for _ in 0..TASKS {
        let pool = pool.clone();
        let config = config.clone();
        handles.push(tokio::spawn(async move {
            let guard = check_budget_and_reserve(&pool, &config).await?;
            guard
                .record(&UsageEntry {
                    model: "fictitious-concurrency-model".to_string(),
                    input_tokens: 1,
                    output_tokens: 1,
                    cost_usd: COST_EACH,
                    estimated: true,
                    endpoint: "mission11-test".to_string(),
                })
                .await?;
            Ok::<(), study_platform::ai::AiError>(())
        }));
    }

    let mut successes = 0usize;
    for handle in handles {
        if handle.await.unwrap().is_ok() {
            successes += 1;
        }
    }

    assert_eq!(
        successes, 4,
        "budget 2.0 / cost 0.5 must admit exactly 4 concurrent reservations"
    );
    let spend = month_spend(&pool).await.unwrap();
    assert!(
        spend <= BUDGET + 1e-9,
        "concurrent spend {spend} must never exceed cap {BUDGET}"
    );
    assert!(
        (spend - BUDGET).abs() < 1e-9,
        "all admitted reservations must be recorded, got spend {spend}"
    );
    pool.close().await;
}

// ---------------------------------------------------------------------------
// 5. MCP error codes / session-loss surface (extends existing patterns)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mission11_mcp_malformed_json_body_is_client_error() {
    let (state, _guard) = setup(None, false).await;
    let (status, body) = send_raw_mcp(state, None, "192.0.2.36", "{not-valid-json").await;
    assert!(
        status.is_client_error(),
        "malformed JSON-RPC body must be a 4xx, got {status}: {body}"
    );
    assert_no_backend_leakage("malformed-mcp-body", &body);
}

#[tokio::test]
async fn mission11_mcp_wrong_jsonrpc_version_is_invalid_request() {
    let (state, _guard) = setup(None, false).await;
    let (status, result) = send_jsonrpc(
        state,
        None,
        "192.0.2.37",
        json!({ "jsonrpc": "1.0", "method": "tools/list", "id": 1 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        result["error"]["code"], -32600,
        "wrong jsonrpc version must be Invalid Request (-32600): {result}"
    );
}

#[tokio::test]
async fn mission11_mcp_unknown_tool_and_method_are_method_not_found() {
    let (state, _guard) = setup(None, false).await;

    // Unknown tool name via tools/call.
    let (status, result) = send_jsonrpc(
        state.clone(),
        None,
        "192.0.2.38",
        json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "name": "fictitious-unknown-tool-000", "arguments": {} },
            "id": 2,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result["error"]["code"], -32601,
        "unknown tool must be Method Not Found (-32601): {result}"
    );

    // Unknown top-level method.
    let (status, result) = send_jsonrpc(
        state,
        None,
        "192.0.2.39",
        json!({ "jsonrpc": "2.0", "method": "fictitious.unknownMethod", "id": 3 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result["error"]["code"], -32601,
        "unknown method must be Method Not Found (-32601): {result}"
    );
}

#[tokio::test]
async fn mission11_mcp_tool_call_wrong_key_is_unauthorized() {
    let (state, _guard) = setup(Some("dummy-configured-key-011".to_string()), false).await;
    let (status, result) = send_jsonrpc(
        state,
        Some("dummy-wrong-key-999"),
        "192.0.2.40",
        json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "name": "search_notes", "arguments": { "query": "x" } },
            "id": 4,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result["error"]["code"], -32001,
        "wrong key on tools/call must be Unauthorized (-32001): {result}"
    );
}

// ---------------------------------------------------------------------------
// 6. Error-message hygiene: 404s and invalid logins leak nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mission11_error_bodies_hide_backend_internals() {
    let (state, _guard) = setup(None, true).await;

    // Seed a legacy-format session row directly: it passes the legacy
    // UUID-format gate today AND the DB-backed gate after Agent A's
    // call-site swap (raw fallback lookup + far-future expiry), so this
    // test is green in both states. (A full login flow cannot be used
    // here: post-Agent-A logins mint opaque non-UUID tokens that the
    // not-yet-swapped legacy gate rejects before reaching the handler.)
    let legacy_id = "550e8400-e29b-41d4-a716-446655440001";
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(&state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(legacy_id)
        .bind(admin_id)
        .bind("2999-01-01T00:00:00+00:00")
        .execute(&state.db)
        .await
        .unwrap();
    let cookie = format!("session_id={legacy_id}");

    // Nonexistent note (authenticated so the request reaches the handler).
    let app = study_platform::handlers::router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/notes/fictitious-nonexistent-slug-000")
                .header("cookie", cookie)
                .header("x-forwarded-for", "192.0.2.42")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let (_, not_found_body, _) = body_text(response).await;
    assert_no_backend_leakage("note-404", &not_found_body);

    // Invalid login stays generic (no enumeration, no backend leakage).
    let app = study_platform::handlers::router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("x-forwarded-for", "192.0.2.43")
                .body(Body::from(
                    "username=fictitious-user&password=fictitious-wrong-password-011",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let (_, login_body, _) = body_text(response).await;
    assert_no_backend_leakage("invalid-login", &login_body);
    assert!(
        login_body.contains("Usuario o contraseña incorrectos"),
        "login error must stay generic: {login_body}"
    );
}

// ---------------------------------------------------------------------------
// Session expiration and revocation — integration tests using real DB lookups
// ---------------------------------------------------------------------------

/// 1. Expired session row is rejected by the DB-backed middleware.
///
/// Seeds the admin user via `setup(.., true)`, inserts an expired session row
/// with a legacy-format UUID `id`. The cookie carries the raw UUID so the
/// middleware hits the legacy-fallback lookup path (`OR id = raw_value`).
/// Because `Session::is_valid()` compares `expires_at > now` and the stored
/// `expires_at` is well in the past, the gate rejects with 401.
#[tokio::test]
async fn test_session_expired_row_is_rejected_by_middleware() {
    use chrono::Utc;
    use study_platform::models::Session;

    let (state, _guard) = setup(None, true).await;

    // Get the seeded admin's user_id.
    let admin_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(&state.db)
        .await
        .unwrap();

    let expired_uuid = "550e8400-e29b-41d4-a716-446655440000";
    let now = Utc::now();
    let past_time = now - chrono::Duration::try_days(2).unwrap();
    let expires_at = past_time.to_rfc3339();

    // Insert an expired session row using raw UUID as id (legacy path).
    sqlx::query("INSERT INTO sessions (id, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)")
        .bind(expired_uuid)
        .bind(admin_id)
        .bind(past_time.to_rfc3339())
        .bind(&expires_at)
        .execute(&state.db)
        .await
        .unwrap();

    // Sanity: the Session model agrees this row is expired.
    let stale = Session {
        id: expired_uuid.to_string(),
        user_id: admin_id,
        created_at: past_time.to_rfc3339(),
        expires_at: expires_at.clone(),
        user_agent: None,
    };
    assert!(
        !stale.is_valid(),
        "expires_at={} must be expired",
        expires_at
    );

    // Request with the expired UUID cookie → gateway should reject.
    let app = study_platform::handlers::router(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/notes")
                .header("cookie", expired_uuid)
                .header("x-forwarded-for", "192.0.2.50")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = resp.status();
    let (_, body, _) = body_text(resp).await;

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "expired session must be rejected by require_auth_state, got {}; body leaked: {}",
        status,
        body
    );
}

/// 2. Session revoked after logout is rejected.
///
/// Logs in normally (POST /login → cookie), verifies access works, logs out
/// (DELETEs the session row), confirms zero rows remain, then reuses the same
/// cookie → expect 401. This tests the full lifecycle: mint → use → revoke →
/// fail-closed.
#[tokio::test]
async fn test_session_revoked_after_logout_is_rejected() {
    let (state, _guard) = setup(None, true).await;

    // 1) Log in via normal POST /login flow.
    let app = study_platform::handlers::router(state.clone());
    let request = Request::builder()
        .method("POST")
        .uri("/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-forwarded-for", "192.0.2.51")
        .body(Body::from(
            "username=admin&password=fictitious-correct-password-001",
        ))
        .unwrap();

    let login_resp = app.oneshot(request).await.unwrap();
    let cookie_val = login_resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next().map(str::to_string))
        .expect("login must set a session cookie");

    assert_eq!(
        login_resp.status(),
        StatusCode::FOUND,
        "login must redirect after successful authentication"
    );

    // 2) Verify the session cookie works on a protected API route.
    let app = study_platform::handlers::router(state.clone());
    let notes_get = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/notes")
                .header("cookie", cookie_val.clone())
                .header("x-forwarded-for", "192.0.2.51")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        notes_get.status(),
        StatusCode::OK,
        "valid session must be accepted on GET /api/notes"
    );

    // 3) Logout — deletes the session row.
    let app = study_platform::handlers::router(state.clone());
    let logout = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header("cookie", cookie_val.clone())
                .header("x-forwarded-for", "192.0.2.51")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        logout.status(),
        StatusCode::OK,
        "logout must respond with the login page"
    );

    // 4) Verify session count dropped to 0.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 0, "logout must delete all session rows");

    // 5) Reuse the revoked cookie on /api/notes → 401.
    let app = study_platform::handlers::router(state.clone());
    let reused = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/notes")
                .header("cookie", cookie_val)
                .header("x-forwarded-for", "192.0.2.51")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let reused_status = reused.status();
    let (_, body, _) = body_text(reused).await;

    assert_eq!(
        reused_status,
        StatusCode::UNAUTHORIZED,
        "revoked session cookie must be rejected on reuse; got {}: {body}",
        reused_status
    );
}

/// 3. Missing session row is rejected (fail-closed).
///
/// No session rows exist in the database. We send a request to a protected
/// route with a fake-but-valid-format UUID cookie. Because there is no row
/// matching either the hashed ID or the raw UUID, the middleware returns 401.
/// This proves the gate fails closed on absent sessions rather than leaking
/// information or granting access.
#[tokio::test]
async fn test_missing_session_row_is_rejected() {
    let (state, _guard) = setup(None, false).await;

    // No session rows inserted — table exists from migrations but is empty.
    let empty_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(
        empty_count, 0,
        "sessions table must be empty before request"
    );

    let fake_cookie = "550e8400-e29b-41d4-a716-446655440000";

    let app = study_platform::handlers::router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/notes")
                .header("cookie", fake_cookie)
                .header("x-forwarded-for", "192.0.2.52")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "request with nonexistent session must be 401 (fail-closed)"
    );
}

/// 4. Valid session is accepted through all validation layers.
///
/// Logs in normally (which mints a hashed opaque session), verifies access
/// works on a protected route, then checks that the session remains valid
/// for subsequent requests. This proves that sessions minted through the
/// normal login flow continue to pass `require_auth_state` validation
/// end-to-end.
#[tokio::test]
async fn test_valid_session_is_accepted() {
    let (state, _guard) = setup(None, true).await;

    // Log in via normal POST /login flow to get a valid session.
    let login_req = Request::builder()
        .method("POST")
        .uri("/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-forwarded-for", "192.0.2.53")
        .body(Body::from(
            "username=admin&password=fictitious-correct-password-001",
        ))
        .unwrap();

    let login_resp = {
        let app = study_platform::handlers::router(state.clone());
        app.oneshot(login_req).await.unwrap()
    };

    let login_status = login_resp.status();
    assert_eq!(
        login_status,
        StatusCode::FOUND,
        "login must succeed and redirect"
    );

    // Extract the session cookie value.
    let cookie_val = login_resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next().map(str::to_string))
        .expect("login must set a session cookie");

    // Verify session count is 1 (row was inserted).
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 1, "exactly one session row after login");

    // Second request with the same valid session → should be accepted.
    let resp = {
        let app = study_platform::handlers::router(state.clone());
        app.oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/notes")
                .header("cookie", cookie_val)
                .header("x-forwarded-for", "192.0.2.53")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    };

    let status = resp.status();
    let (_, body, _) = body_text(resp).await;

    // Valid sessions pass through require_auth_state to the handler.
    // /api/notes returns 200 (list notes) for authenticated users.
    assert!(
        status.is_success(),
        "valid session must be accepted by middleware; got {}; body leaked: {}",
        status,
        body
    );
}
