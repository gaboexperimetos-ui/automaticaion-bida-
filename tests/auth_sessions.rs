//! Session authentication tests (Mission 11).
//!
//! Covers DB-backed sessions with hashed opaque tokens:
//! login mints a token whose cookie passes `require_auth_state`, wrong
//! passwords mint nothing, logout invalidates, expired/unknown tokens are
//! rejected, sessions are isolated per login, the DB stores only the hash,
//! legacy raw-UUID rows still validate during the transition, and a
//! mismatched Origin on POST /login is rejected with 403.
//!
//! All data is fictitious. No secrets are used or asserted. Each test uses a
//! dedicated TEST-NET-1 (`192.0.2.x`) IP so the process-global rate limiter
//! (60/min general, 10/min `login:<ip>` on POST /login) can never trip:
//! no test performs more than 3 login attempts on its own IP.

use std::path::PathBuf;
use std::sync::Arc;

use argon2::PasswordHasher;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::middleware::auth::{
    generate_session_token, hash_session_token, require_auth_state,
};
use study_platform::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Fictitious correct password for the seeded admin user (test-only).
const CORRECT_PASSWORD: &str = "fictitious-correct-password-011";
/// Fictitious wrong password (test-only).
const WRONG_PASSWORD: &str = "fictitious-wrong-password-011";

fn test_config(db_path: PathBuf, notes_dir: PathBuf) -> Config {
    Config {
        host: "127.0.0.1".to_string(),
        port: 18092,
        session_secret: "auth_sessions_test_secret_32_chars!!".to_string(),
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

/// Full test state via embedded migrations (includes the new
/// `20240101000004_sessions.sql`) plus a seeded fictitious admin user.
/// Returns the state plus the temp-dir guard (must be kept alive).
async fn setup() -> (Arc<AppState>, tempfile::TempDir) {
    let work_dir = tempfile::tempdir().unwrap();
    let db_path = work_dir.path().join("test.db");
    std::fs::write(&db_path, b"").expect("Failed to create db file");
    let db_url = format!("sqlite://{}", db_path.display());
    let notes_dir = work_dir.path().join("notes");

    let config = test_config(db_path, notes_dir);

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

    let salt = argon2::password_hash::SaltString::generate(rand::thread_rng());
    let argon2 = argon2::Argon2::default();
    let password_hash = argon2
        .hash_password(CORRECT_PASSWORD.as_bytes(), &salt)
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

    let state = Arc::new(AppState { config, db: pool });
    (state, work_dir)
}

/// Minimal router guarded by the NEW stateful middleware: one page route
/// (302 split) and one API route (401 split).
fn guarded_router(state: Arc<AppState>) -> Router {
    use study_platform::handlers::notes;
    use study_platform::handlers::stitch_v2;

    Router::new()
        .route("/dashboard", get(stitch_v2::stitch_dashboard))
        .route("/app/notes", get(notes::list))
        // API route exercising the 401 rejection split for `/api/*`.
        .route(
            "/api/thing",
            get(|| async { StatusCode::OK }),
        )
        .route_layer(from_fn_with_state(state.clone(), require_auth_state))
        .with_state(state)
}

/// POST /login through the FULL production router. Returns
/// (status, body, set-cookie values, location).
async fn do_login(
    state: Arc<AppState>,
    ip: &str,
    username: &str,
    password: &str,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, String, Vec<String>, Option<String>) {
    let app = study_platform::handlers::router(state);
    let form = format!("username={username}&password={password}");
    let mut builder = Request::builder()
        .method("POST")
        .uri("/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-forwarded-for", ip);
    for (k, v) in extra_headers {
        builder = builder.header(*k, *v);
    }
    let response = app
        .oneshot(builder.body(Body::from(form)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let location = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let cookies: Vec<String> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_string))
        .collect();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    (status, body, cookies, location)
}

/// Extract the raw `session_id` cookie value from Set-Cookie headers.
fn raw_token_from(cookies: &[String]) -> Option<String> {
    cookies.iter().find_map(|c| {
        c.split(';').next().and_then(|first| {
            let first = first.trim();
            first
                .strip_prefix("session_id=")
                .map(str::to_string)
                .filter(|v| !v.is_empty())
        })
    })
}

async fn get_guarded(
    state: Arc<AppState>,
    uri: &str,
    cookie: Option<&str>,
    ip: &str,
) -> (StatusCode, String) {
    let app = guarded_router(state);
    let mut builder = Request::builder().uri(uri).header("x-forwarded-for", ip);
    eprintln!("DEBUG get_guarded uri={uri} cookie={:?}", cookie.map(|s| s.to_string()));
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn session_count(state: &Arc<AppState>) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&state.db)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_login_success_mints_cookie_usable_on_guarded_route() {
    let (state, _guard) = setup().await;

    let (status, _body, cookies, location) =
        do_login(state.clone(), "192.0.2.101", "admin", CORRECT_PASSWORD, &[]).await;

    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(location.as_deref(), Some("/dashboard"));
    let raw = raw_token_from(&cookies).expect("login must set a session cookie");
    assert_eq!(raw.len(), 64, "new tokens are 32 bytes hex-encoded");

    // Cookie flags on the Set-Cookie header.
    let set_cookie = cookies.join("; ");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Strict"), "{set_cookie}");
    assert!(set_cookie.contains("Path=/"), "{set_cookie}");
    assert!(set_cookie.contains("Max-Age="), "{set_cookie}");

    // Raw token passes the stateful gate on both page and API routes.
    let cookie = format!("session_id={raw}");
    let (s, body) = get_guarded(state.clone(), "/dashboard", Some(&cookie), "192.0.2.101").await;
    assert_eq!(s, StatusCode::OK, "dashboard body: {body}");
    let (s, body) = get_guarded(state.clone(), "/app/notes", Some(&cookie), "192.0.2.101").await;
    assert_eq!(s, StatusCode::OK, "notes body: {body}");
}

#[tokio::test]
async fn test_wrong_password_mints_no_session() {
    let (state, _guard) = setup().await;

    let (status, body, cookies, _) =
        do_login(state.clone(), "192.0.2.102", "admin", WRONG_PASSWORD, &[]).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("Usuario o contraseña incorrectos"), "{body}");
    assert!(
        raw_token_from(&cookies).is_none(),
        "wrong password must not set a session cookie: {cookies:?}"
    );
    assert_eq!(session_count(&state).await, 0, "no session row may exist");
}

#[tokio::test]
async fn test_logout_deletes_row_and_token_fails_gate() {
    let (state, _guard) = setup().await;

    let (_, _, cookies, _) =
        do_login(state.clone(), "192.0.2.103", "admin", CORRECT_PASSWORD, &[]).await;
    let raw = raw_token_from(&cookies).expect("login must set a cookie");
    let cookie = format!("session_id={raw}");
    assert_eq!(session_count(&state).await, 1);

    // Logout through the production router.
    let app = study_platform::handlers::router(state.clone());
    let logout = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header("cookie", cookie.clone())
                .header("x-forwarded-for", "192.0.2.103")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::OK);
    let cleared = logout
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(cleared.starts_with("session_id=;"), "{cleared}");

    // Row is gone…
    assert_eq!(session_count(&state).await, 0);

    // …and the reused token now fails the stateful gate on both splits.
    let (s, body) = get_guarded(state.clone(), "/dashboard", Some(&cookie), "192.0.2.103").await;
    assert_eq!(s, StatusCode::FOUND, "dashboard must redirect to /login");
    let (s, body) = get_guarded(state.clone(), "/app/notes", Some(&cookie), "192.0.2.103").await;
    assert_eq!(s, StatusCode::FOUND, "notes page must redirect to /login");
}

#[tokio::test]
async fn test_expired_session_is_rejected() {
    let (state, _guard) = setup().await;

    // Insert an expired session row directly (hashed id, past expires_at).
    let raw = generate_session_token();
    let id = hash_session_token(&raw, &state.config.session_secret);
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(&id)
        .bind(1_i64)
        .bind("2000-01-02T00:00:00+00:00")
        .execute(&state.db)
        .await
        .unwrap();

    let cookie = format!("session_id={raw}");
    let (s, _) = get_guarded(state.clone(), "/app/notes", Some(&cookie), "192.0.2.104").await;
    assert_eq!(s, StatusCode::FOUND, "expired session must redirect");
    let (s, _) = get_guarded(state.clone(), "/dashboard", Some(&cookie), "192.0.2.104").await;
    assert_eq!(s, StatusCode::FOUND);
}

#[tokio::test]
async fn test_unknown_and_malformed_tokens_are_rejected() {
    let (state, _guard) = setup().await;

    // Well-formed but never stored.
    let ghost = generate_session_token();
    let cookie = format!("session_id={ghost}");
    let (s, _) = get_guarded(state.clone(), "/app/notes", Some(&cookie), "192.0.2.105").await;
    assert_eq!(s, StatusCode::FOUND, "unknown token must redirect");
    let (s, _) = get_guarded(state.clone(), "/dashboard", Some(&cookie), "192.0.2.105").await;
    assert_eq!(s, StatusCode::FOUND);

    // Garbage token.
    let (s, _) = get_guarded(
        state.clone(),
        "/app/notes",
        Some("session_id=not a valid token!!!"),
        "192.0.2.105",
    )
    .await;
    assert_eq!(s, StatusCode::FOUND, "malformed token must redirect");
    let (s, _) = get_guarded(state.clone(), "/dashboard", Some("session_id=not a valid token!!!"), "192.0.2.105").await;
    assert_eq!(s, StatusCode::FOUND);

    // Missing cookie.
    let (s, _) = get_guarded(state.clone(), "/app/notes", None, "192.0.2.105").await;
    assert_eq!(s, StatusCode::FOUND, "missing cookie must redirect");
    let (s, _) = get_guarded(state.clone(), "/dashboard", None, "192.0.2.105").await;
    assert_eq!(s, StatusCode::FOUND);
}

#[tokio::test]
async fn test_session_isolation_two_logins_logout_one() {
    let (state, _guard) = setup().await;

    let (_, _, c1, _) =
        do_login(state.clone(), "192.0.2.106", "admin", CORRECT_PASSWORD, &[]).await;
    let (_, _, c2, _) =
        do_login(state.clone(), "192.0.2.107", "admin", CORRECT_PASSWORD, &[]).await;
    let raw1 = raw_token_from(&c1).unwrap();
    let raw2 = raw_token_from(&c2).unwrap();

    assert_ne!(raw1, raw2, "each login must mint a distinct token");
    assert_eq!(session_count(&state).await, 2);

    let cookie1 = format!("session_id={raw1}");
    let cookie2 = format!("session_id={raw2}");
    let (s, _) = get_guarded(state.clone(), "/dashboard", Some(&cookie1), "192.0.2.106").await;
    eprintln!("DEBUG isolation /dashboard cookie1 -> {s}");
    assert_eq!(s, StatusCode::OK);
    let (s, _) = get_guarded(state.clone(), "/dashboard", Some(&cookie2), "192.0.2.107").await;
    eprintln!("DEBUG isolation /dashboard cookie2 -> {s}");
    assert_eq!(s, StatusCode::OK);

    // Logout the first token only.
    let app = study_platform::handlers::router(state.clone());
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/logout")
            .header("cookie", cookie1.clone())
            .header("x-forwarded-for", "192.0.2.106")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    assert_eq!(session_count(&state).await, 1);
    let (s, _) = get_guarded(state.clone(), "/api/thing", Some(&cookie1), "192.0.2.106").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "logged-out token must fail");
    let (s, _) = get_guarded(state.clone(), "/api/thing", Some(&cookie2), "192.0.2.107").await;
    assert_eq!(s, StatusCode::OK, "other session must stay valid");
}

#[tokio::test]
async fn test_db_stores_hash_not_raw_token() {
    let (state, _guard) = setup().await;

    let (_, _, cookies, _) =
        do_login(state.clone(), "192.0.2.108", "admin", CORRECT_PASSWORD, &[]).await;
    let raw = raw_token_from(&cookies).expect("login must set a cookie");

    let row_id: String = sqlx::query_scalar("SELECT id FROM sessions")
        .fetch_one(&state.db)
        .await
        .unwrap();

    assert_ne!(row_id, raw, "DB must never store the raw token");
    assert_eq!(
        row_id,
        hash_session_token(&raw, &state.config.session_secret),
        "stored id must be SHA256(secret:token) hex"
    );
    assert_eq!(row_id.len(), 64);
}

#[tokio::test]
async fn test_legacy_uuid_row_still_validates() {
    let (state, _guard) = setup().await;

    // Pre-Mission-11 row: raw UUID stored directly.
    let legacy = "550e8400-e29b-41d4-a716-446655440011";
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(legacy)
        .bind(1_i64)
        .bind("2999-01-01T00:00:00+00:00")
        .execute(&state.db)
        .await
        .unwrap();

    let cookie = format!("session_id={legacy}");
    let (s, _) = get_guarded(state.clone(), "/api/thing", Some(&cookie), "192.0.2.109").await;
    assert_eq!(
        s,
        StatusCode::OK,
        "legacy UUID rows must validate during transition"
    );
}

#[tokio::test]
async fn test_login_origin_mismatch_is_403() {
    let (state, _guard) = setup().await;

    // Mismatched Origin vs Host → 403, no session minted.
    let (status, _, cookies, _) = do_login(
        state.clone(),
        "192.0.2.110",
        "admin",
        CORRECT_PASSWORD,
        &[
            ("host", "127.0.0.1:8090"),
            ("origin", "https://evil.example.com"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(raw_token_from(&cookies).is_none());
    assert_eq!(session_count(&state).await, 0);

    // Matching Origin → login proceeds.
    let (status, _, cookies, _) = do_login(
        state.clone(),
        "192.0.2.111",
        "admin",
        CORRECT_PASSWORD,
        &[
            ("host", "127.0.0.1:8090"),
            ("origin", "http://127.0.0.1:8090"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(raw_token_from(&cookies).is_some());

    // Absent Origin/Referer → allowed (curl/tests; documented policy).
    let (status, _, cookies, _) =
        do_login(state.clone(), "192.0.2.112", "admin", CORRECT_PASSWORD, &[]).await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(raw_token_from(&cookies).is_some());
}
