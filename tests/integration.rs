//! Integration tests for the Study Platform API.
//!
//! Tests use SQLite file databases in temp directories (isolated per test).
//! No real student data is used.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use password_hash::PasswordHasher;
use sqlx::sqlite::SqlitePoolOptions;

use tower::ServiceExt;

use study_platform::{AppState, Config};

/// Creates a test configuration without reading env vars.
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
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
        openrouter_max_tokens: 8192,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://127.0.0.1:8080/mcp".to_string(),
        genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        registration_token: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
    }
}

/// Set up test app with isolated database and seeded admin user.
async fn setup_test_app() -> (Arc<AppState>, Config) {
    let temp = tempfile::NamedTempFile::new().unwrap();
    let db_path = temp.path().to_path_buf();
    let db_url = format!("sqlite://{}", db_path.display());

    let notes_dir = tempfile::tempdir().unwrap().keep();

    let config = test_config(notes_dir, db_path);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .expect("Failed to create test database");

    // Run migrations
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

    // Seed an admin user with known password
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
    .bind("admin")
    .bind(&password_hash)
    .execute(&pool)
    .await
    .expect("Failed to seed admin user");

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

    (state, config)
}

// --- Health & Config Tests ---

#[tokio::test]
async fn test_health_endpoint() {
    let (state, _) = setup_test_app().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["status"], "healthy");
    assert!(json["components"]["database"].as_bool().unwrap());
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn test_public_config_no_secrets() {
    let (state, _) = setup_test_app().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    // Verify no secrets are in the response
    assert!(!body_str.contains("test_secret"));
    assert!(!body_str.contains("replace_me"));
    assert!(!body_str.contains("sk-or-"));

    // Verify expected fields
    let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();
    assert!(json["openrouter_model"].as_str().is_some());
    assert!(json["mikareno_mcp_url"].as_str().is_some());
    assert!(json["genoffice_mcp_url"].as_str().is_some());
    assert!(json["is_localhost"].as_bool().is_some());
}

// --- Auth Tests ---

#[tokio::test]
async fn test_login_page_served() {
    let (state, _) = setup_test_app().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();

    assert!(html.contains("Study Platform"));
    assert!(html.contains("password"));
    assert!(html.contains("htmx"));
    assert!(html.contains("tailwind"));
}

#[tokio::test]
async fn test_login_redirect_when_unauthenticated() {
    let (state, _) = setup_test_app().await;
    let app = study_platform::handlers::router(state);

    // Protected route without session should redirect to /login
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
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

#[tokio::test]
async fn test_api_notes_requires_auth() {
    let (state, _) = setup_test_app().await;
    let app = study_platform::handlers::router(state);

    // API route without session should return 401
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/notes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_successful_login_returns_session() {
    let (state, _) = setup_test_app().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=admin&password=testpassword123"))
                .unwrap(),
        )
        .await
        .unwrap();

    // Login redirects with 302 Found to the dashboard.
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok()),
        Some("/dashboard")
    );

    let cookie = response.headers().get("set-cookie");
    assert!(cookie.is_some(), "Session cookie should be set");
    let cookie_str = cookie.unwrap().to_str().unwrap();
    assert!(cookie_str.contains("session_id="));
    assert!(cookie_str.contains("HttpOnly"));
}

#[tokio::test]
async fn test_failed_login_returns_401() {
    let (state, _) = setup_test_app().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=admin&password=wrongpassword"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_logout_clears_session() {
    let (state, _) = setup_test_app().await;
    let app = study_platform::handlers::router(state);

    // Login first
    let login_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=admin&password=testpassword123"))
                .unwrap(),
        )
        .await
        .unwrap();

    let cookie = login_response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let session_cookie = cookie.split(';').next().unwrap();

    // Logout
    let logout_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header("cookie", session_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Logout responds 200 OK with the inline login page (cookie cleared
    // below), instead of redirecting.
    assert_eq!(logout_response.status(), StatusCode::OK);

    let new_cookie = logout_response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(new_cookie.contains("Max-Age=0"));
}

// --- Notes CRUD Tests ---

#[tokio::test]
async fn test_notes_crud_lifecycle() {
    let (state, _) = setup_test_app().await;
    let app = study_platform::handlers::router(state);

    // Login
    let login_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("username=admin&password=testpassword123"))
                .unwrap(),
        )
        .await
        .unwrap();

    let cookie = login_response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let session_cookie = cookie.split(';').next().unwrap();

    // Create a note
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/notes")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("cookie", session_cookie)
                .body(Body::from(
                    "title=Mi Primera Nota&content=# Hola Mundo&tags=prueba,estudio",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        create_response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok()),
        Some("/app/notes/mi-primera-nota")
    );

    // List notes
    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/notes")
                .header("cookie", session_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(list_response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(list_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Mi Primera Nota"));

    // View the note
    let view_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/notes/mi-primera-nota")
                .header("cookie", session_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(view_response.status(), StatusCode::OK);

    // Delete the note
    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/app/notes/mi-primera-nota/delete")
                .header("cookie", session_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(delete_response.status(), StatusCode::OK);
}

// --- Model Unit Tests ---

#[tokio::test]
async fn test_slugify_function() {
    use study_platform::models::Note;
    assert_eq!(Note::slugify("Hola Mundo"), "hola-mundo");
    assert_eq!(Note::slugify("¡Hola! ¿Cómo estás?"), "hola-cómo-estás");
    assert_eq!(Note::slugify("  Test  "), "test");
}

#[tokio::test]
async fn test_password_validation() {
    use study_platform::models::SetPasswordRequest;

    let req = SetPasswordRequest {
        password: "".to_string(),
        confirm: "".to_string(),
    };
    assert!(req.validate().is_err());

    let req = SetPasswordRequest {
        password: "short".to_string(),
        confirm: "short".to_string(),
    };
    assert!(req.validate().is_err());

    let req = SetPasswordRequest {
        password: "password123".to_string(),
        confirm: "password456".to_string(),
    };
    assert!(req.validate().is_err());

    let req = SetPasswordRequest {
        password: "securepassword123".to_string(),
        confirm: "securepassword123".to_string(),
    };
    assert!(req.validate().is_ok());
}

// --- Database Tests ---

#[tokio::test]
async fn test_database_schema() {
    let (state, _) = setup_test_app().await;

    let tables: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
    )
    .fetch_all(&state.db)
    .await
    .expect("Failed to query tables");

    assert!(tables.contains(&"users".to_string()));
    assert!(tables.contains(&"sessions".to_string()));
    assert!(tables.contains(&"notes".to_string()));
    assert!(tables.contains(&"app_config".to_string()));
}

#[tokio::test]
async fn test_notes_empty_initially() {
    let (state, _) = setup_test_app().await;

    let count: i64 = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notes")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
