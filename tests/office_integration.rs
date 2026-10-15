//! Integration tests for the Study Platform web office suite.
//!
//! Tests use an isolated SQLite database and temp directory (isolated per test).
//! Auth is simulated with a valid UUID session cookie — the auth middleware only
//! validates UUID format, no DB lookup is performed.

use std::sync::Arc;

use argon2::PasswordHasher;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Duration as ChronoDuration;
use sqlx::sqlite::SqlitePoolOptions;
use study_platform::{AppState, Config};
use tower::ServiceExt;

const TEST_SESSION: &str = "session_id=00000000-0000-4000-8000-000000000000";

/// URL-encode a string for `application/x-www-form-urlencoded` form data.
fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for &byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// Build URL-encoded form data from key-value pairs.
fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Extract the response body as a string.
async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

/// Extract the response body as bytes.
async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

/// Set up a test app with an isolated temp directory and seeded admin user.
///
/// The temp directory is kept alive (returned) so that the office directories
/// created under `<temp>/office/{docs,sheets,presentations}` persist for the
/// duration of the test.
async fn setup() -> (Arc<AppState>, tempfile::TempDir) {
    let work_dir = tempfile::tempdir().unwrap();
    let db_path = work_dir.path().join("test.db");
    // Create the file first — SQLite via sqlx has trouble creating new files
    // directly in temp directories; pre-creating it works reliably.
    std::fs::write(&db_path, b"").expect("Failed to create db file");
    let db_url = format!("sqlite://{}", db_path.display());
    let notes_dir = work_dir.path().join("notes");

    let config = Config {
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
        rust_log: "test".to_string(),
    };

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

    // Seed admin user with known password
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

    // Seed a session row so that DB-backed require_auth_state accepts TEST_SESSION.
    // Drop-and-recreate sessions table without FK constraint to avoid per-connection
    // PRAGMA foreign_keys issues with the pooled connection (sqlite applies it
    // per-connection, not globally). The table still exists for read lookups.
    sqlx::query("DROP TABLE IF EXISTS sessions")
        .execute(&pool)
        .await
        .ok();
    sqlx::query(
        r#"CREATE TABLE sessions (
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

    let uid: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&pool)
        .await
        .unwrap();
    let expires = (chrono::Utc::now() + ChronoDuration::hours(24)).to_rfc3339();
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind("00000000-0000-4000-8000-000000000000")
        .bind(uid)
        .bind(&expires)
        .execute(&pool)
        .await
        .expect("seed session for auth tests");

    let state = Arc::new(AppState { config, db: pool });

    (state, work_dir)
}

/// Build an authenticated GET request to `uri`.
fn authed_get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("cookie", TEST_SESSION)
        .header("x-forwarded-for", "10.0.0.1")
        .body(Body::empty())
        .unwrap()
}

/// Build an authenticated POST request with form data.
fn authed_post_form(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("cookie", TEST_SESSION)
        .header("x-forwarded-for", "10.0.0.2")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Build an authenticated PUT request with form data.
fn authed_put_form(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header("cookie", TEST_SESSION)
        .header("x-forwarded-for", "10.0.0.3")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Build an authenticated DELETE request.
fn authed_delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("cookie", TEST_SESSION)
        .header("x-forwarded-for", "10.0.0.4")
        .body(Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// Auth tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_office_requires_auth() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // No auth cookie → should redirect to /login (302)
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/office")
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
async fn test_office_home_with_auth() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    let response = app.clone().oneshot(authed_get("/office")).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("Suite de oficina"));
    assert!(html.contains("Documentos"));
    assert!(html.contains("Hojas"));
    assert!(html.contains("Presentaciones"));
    assert!(html.contains("Biblioteca"));
}

// ---------------------------------------------------------------------------
// Documents tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_document_crud_lifecycle() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // Create a document
    let body = form(&[
        ("title", "My First Document"),
        ("content", "# Hello World\n\nThis is a test document."),
    ]);
    let response = app
        .clone()
        .oneshot(authed_post_form("/office/docs", &body))
        .await
        .unwrap();

    // Create returns 200 with HX-Redirect
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/office/docs/my-first-document?saved=1")
    );

    // List documents — should contain the created doc
    let response = app
        .clone()
        .oneshot(authed_get("/office/docs"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("my-first-document"));
    assert!(html.contains("my first document"));

    // Edit (view) the document — should show the content
    let response = app
        .clone()
        .oneshot(authed_get("/office/docs/my-first-document"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("Guardar"));
    assert!(html.contains("# Hello World"));

    // Update the document
    let body = form(&[
        ("title", "My First Document"),
        ("content", "# Updated Content\n\nUpdated text."),
    ]);
    let response = app
        .clone()
        .oneshot(authed_put_form("/office/docs/my-first-document", &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/office/docs/my-first-document?saved=1")
    );

    // Delete the document
    let response = app
        .clone()
        .oneshot(authed_delete("/office/docs/my-first-document"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/office/docs")
    );

    // Verify it's gone from the list
    let response = app
        .clone()
        .oneshot(authed_get("/office/docs"))
        .await
        .unwrap();
    let html = body_string(response).await;
    assert!(!html.contains("my-first-document"));
}

#[tokio::test]
async fn test_document_preview_renders_markdown() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // Create a document
    let body = form(&[
        ("title", "Markdown Preview Test"),
        ("content", "# Hello\n\nThis is **bold** and *italic*."),
    ]);
    app.clone()
        .oneshot(authed_post_form("/office/docs", &body))
        .await
        .unwrap();

    // Preview should render markdown to HTML
    let response = app
        .clone()
        .oneshot(authed_get("/office/docs/markdown-preview-test/preview"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("<h1>Hello</h1>"));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(html.contains("<em>italic</em>"));
}

#[tokio::test]
async fn test_document_export_formats() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // Create a document
    let body = form(&[
        ("title", "Export Test Doc"),
        ("content", "# Export\n\nTest content for export."),
    ]);
    app.clone()
        .oneshot(authed_post_form("/office/docs", &body))
        .await
        .unwrap();

    // Export as Markdown
    let response = app
        .clone()
        .oneshot(authed_get("/office/docs/export-test-doc/export?fmt=md"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/markdown; charset=utf-8")
    );
    let bytes = body_bytes(response).await;
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.contains("# Export"));
    assert!(content.contains("Test content for export."));

    // Export as HTML
    let response = app
        .clone()
        .oneshot(authed_get("/office/docs/export-test-doc/export?fmt=html"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    let bytes = body_bytes(response).await;
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.contains("<h1>Export</h1>"));
    assert!(content.contains("Test content for export."));
}

// ---------------------------------------------------------------------------
// Spreadsheet tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_spreadsheet_create_import_export_formulas() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // Create a spreadsheet
    let body = form(&[("title", "Formula Sheet")]);
    let response = app
        .clone()
        .oneshot(authed_post_form("/office/sheets", &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let redirect = response
        .headers()
        .get("HX-Redirect")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert!(redirect.contains("/office/sheets/formula-sheet"));

    // Import CSV with a SUM formula
    // Row 0: A=1, B=formula for sum of A1:A3
    // Row 1: A=2, B=
    // Row 2: A=3, B=
    let csv_data = "1,=SUM(A1:A3)\n2,\n3,\n";
    let body = form(&[("csv_data", csv_data)]);
    let response = app
        .clone()
        .oneshot(authed_post_form(
            "/office/sheets/formula-sheet/import",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("HX-Redirect")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .contains("/office/sheets/formula-sheet"));

    // Export — should have computed formulas
    let response = app
        .clone()
        .oneshot(authed_get("/office/sheets/formula-sheet/export"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body_bytes(response).await;
    let content = String::from_utf8_lossy(&bytes);
    // The formula cell should be replaced with the computed value (1+2+3 = 6)
    assert!(
        content.contains("6"),
        "Expected SUM result 6 in: {}",
        content
    );
    // The other cells should remain
    assert!(content.contains("1"));
    assert!(content.contains("2"));
    assert!(content.contains("3"));

    // Verify the edit page shows computed values
    let response = app
        .clone()
        .oneshot(authed_get("/office/sheets/formula-sheet"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("Valores calculados"));
}

// ---------------------------------------------------------------------------
// Presentation tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_presentation_create_add_slide_present_delete() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // Create a presentation
    let body = form(&[("title", "Demo Pres"), ("content", "")]);
    let response = app
        .clone()
        .oneshot(authed_post_form("/office/presentations", &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/office/presentations/demo-pres")
    );

    // Add a slide
    let body = form(&[
        ("title", "Slide Title"),
        ("content", "# Slide Content\n\n- Bullet 1\n- Bullet 2"),
    ]);
    let response = app
        .clone()
        .oneshot(authed_post_form(
            "/office/presentations/demo-pres/slides",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/office/presentations/demo-pres")
    );

    // Present mode — should render markdown
    let response = app
        .clone()
        .oneshot(authed_get("/office/presentations/demo-pres/present"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("Slide Title"));
    assert!(html.contains("<h1>Slide Content</h1>"));
    assert!(html.contains("<li>Bullet 1</li>"));
    assert!(html.contains("<li>Bullet 2</li>"));
    assert!(html.contains("1 diapositiva(s)"));

    // Edit page should show slide controls
    let response = app
        .clone()
        .oneshot(authed_get("/office/presentations/demo-pres"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("slide-0"));
    assert!(html.contains("Añadir diapositiva"));

    // Delete the presentation
    let response = app
        .clone()
        .oneshot(authed_delete("/office/presentations/demo-pres"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/office/presentations")
    );

    // Verify it's gone
    let response = app
        .clone()
        .oneshot(authed_get("/office/presentations"))
        .await
        .unwrap();
    let html = body_string(response).await;
    assert!(!html.contains("demo-pres"));
}

// ---------------------------------------------------------------------------
// Library tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_library_search_and_download() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // Create a document and a sheet
    app.clone()
        .oneshot(authed_post_form(
            "/office/docs",
            &form(&[("title", "Searchable Doc"), ("content", "# Content")]),
        ))
        .await
        .unwrap();

    app.clone()
        .oneshot(authed_post_form(
            "/office/sheets",
            &form(&[("title", "Searchable Sheet")]),
        ))
        .await
        .unwrap();

    // Library list — should show both files
    let response = app
        .clone()
        .oneshot(authed_get("/office/library"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("searchable-doc"));
    assert!(html.contains("searchable-sheet"));
    assert!(html.contains("Importar archivo"));

    // Search for "doc" — should find the document
    let response = app
        .clone()
        .oneshot(authed_get("/office/library/search?q=doc"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("searchable-doc"));
    assert!(!html.contains("searchable-sheet"));

    // Download the document
    let response = app
        .clone()
        .oneshot(authed_get("/office/library/docs/searchable-doc/download"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/markdown; charset=utf-8")
    );
    let bytes = body_bytes(response).await;
    let content = String::from_utf8_lossy(&bytes);
    assert!(content.contains("# Content"));

    // Download the sheet
    let response = app
        .clone()
        .oneshot(authed_get(
            "/office/library/sheets/searchable-sheet/download",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/csv; charset=utf-8")
    );
}

#[tokio::test]
async fn test_library_import_via_form() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // Import a document via the library import form
    let body = form(&[
        ("name", "Imported Doc"),
        ("content", "# Imported\n\nThis was imported."),
    ]);
    let response = app
        .clone()
        .oneshot(authed_post_form("/office/library/import/docs", &body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/office/library")
    );

    // Verify it appears in the library
    let response = app
        .clone()
        .oneshot(authed_get("/office/library"))
        .await
        .unwrap();
    let html = body_string(response).await;
    assert!(html.contains("imported-doc"));

    // Verify the file is accessible via documents
    let response = app
        .clone()
        .oneshot(authed_get("/office/docs/imported-doc"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("Imported"));
}

#[tokio::test]
async fn test_library_delete() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // Create a document
    app.clone()
        .oneshot(authed_post_form(
            "/office/docs",
            &form(&[("title", "To Delete"), ("content", "content")]),
        ))
        .await
        .unwrap();

    // Verify it exists
    let response = app
        .clone()
        .oneshot(authed_get("/office/library"))
        .await
        .unwrap();
    let html = body_string(response).await;
    assert!(html.contains("to-delete"));

    // Delete via library
    let response = app
        .clone()
        .oneshot(authed_post_form(
            "/office/library/docs/to-delete/delete",
            "",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/office/library")
    );

    // Verify it's gone
    let response = app
        .clone()
        .oneshot(authed_get("/office/library"))
        .await
        .unwrap();
    let html = body_string(response).await;
    assert!(!html.contains("to-delete"));
}

// ---------------------------------------------------------------------------
// Spreadsheet delete test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_spreadsheet_delete() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // Create a sheet
    app.clone()
        .oneshot(authed_post_form(
            "/office/sheets",
            &form(&[("title", "Sheet To Delete")]),
        ))
        .await
        .unwrap();

    // Delete it
    let body = "";
    let response = app
        .clone()
        .oneshot(authed_post_form(
            "/office/sheets/sheet-to-delete/delete",
            body,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("HX-Redirect")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .contains("/office/sheets"));

    // Verify it's gone from the list
    let response = app
        .clone()
        .oneshot(authed_get("/office/sheets"))
        .await
        .unwrap();
    let html = body_string(response).await;
    assert!(!html.contains("sheet-to-delete"));
}

// ---------------------------------------------------------------------------
// Presentation slide operations test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_presentation_slide_operations() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // Create a presentation
    app.clone()
        .oneshot(authed_post_form(
            "/office/presentations",
            &form(&[("title", "Slide Test Pres"), ("content", "")]),
        ))
        .await
        .unwrap();

    // Add two slides
    app.clone()
        .oneshot(authed_post_form(
            "/office/presentations/slide-test-pres/slides",
            &form(&[("title", "Slide One"), ("content", "Content 1")]),
        ))
        .await
        .unwrap();

    app.clone()
        .oneshot(authed_post_form(
            "/office/presentations/slide-test-pres/slides",
            &form(&[("title", "Slide Two"), ("content", "Content 2")]),
        ))
        .await
        .unwrap();

    // Verify both slides appear in the edit page
    let response = app
        .clone()
        .oneshot(authed_get("/office/presentations/slide-test-pres"))
        .await
        .unwrap();
    let html = body_string(response).await;
    assert!(html.contains("slide-0"));
    assert!(html.contains("slide-1"));
    assert!(!html.contains("slide-2"));

    // Delete slide 0
    let response = app
        .clone()
        .oneshot(authed_delete(
            "/office/presentations/slide-test-pres/slides/0",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("HX-Redirect")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .contains("/office/presentations/slide-test-pres"));

    // Verify only one slide remains
    let response = app
        .clone()
        .oneshot(authed_get("/office/presentations/slide-test-pres"))
        .await
        .unwrap();
    let html = body_string(response).await;
    // After deleting slide 0, only "Slide Two" remains (renumbered to index 0)
    assert!(html.contains("Slide Two"));
    assert!(!html.contains("Slide One"));
    assert!(!html.contains("slide-1"));
}

// ---------------------------------------------------------------------------
// P0: path traversal guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_slug_traversal_blocked() {
    let (state, work_dir) = setup().await;
    let app = study_platform::handlers::router(state.clone());

    // Sentinel file outside the office dirs — traversal must never touch it.
    let sentinel = work_dir.path().join("sentinel-outside.txt");
    std::fs::write(&sentinel, "do-not-touch").unwrap();
    let office_parent = work_dir.path().join("office");
    let before: Vec<String> = walk_files(work_dir.path());

    // Create a real doc so handlers have something valid to compare against.
    app.clone()
        .oneshot(authed_post_form(
            "/office/docs",
            &form(&[("title", "Traversal Base"), ("content", "base")]),
        ))
        .await
        .unwrap();

    // `..` as a single segment must be rejected with 400.
    for uri in [
        "/office/docs/..",
        "/office/docs/%2E%2E",
        "/office/docs/%2e%2e",
        "/office/sheets/..",
        "/office/presentations/..",
        "/office/library/docs/../download",
        "/office/library/docs/%2E%2E/download",
    ] {
        let resp = app.clone().oneshot(authed_get(uri)).await.unwrap();
        assert!(
            resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::NOT_FOUND,
            "expected 400/404 for {}, got {}",
            uri,
            resp.status()
        );
    }

    // Encoded slash traversal smuggled in a single segment must be 400.
    for uri in [
        "/office/docs/%2E%2E%2Fsecret",
        "/office/sheets/%2E%2E%2Fsecret",
        "/office/presentations/%2E%2E%2Fsecret",
    ] {
        let resp = app.clone().oneshot(authed_get(uri)).await.unwrap();
        assert!(
            resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::NOT_FOUND,
            "expected 400/404 for {}, got {}",
            uri,
            resp.status()
        );
    }

    // `a/b` spans two segments → no route matches → 404.
    let resp = app
        .clone()
        .oneshot(authed_get("/office/docs/a/b"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Mutating verbs with traversal slugs must also be rejected, never deleting.
    let resp = app
        .clone()
        .oneshot(authed_put_form(
            "/office/docs/..",
            &form(&[("content", "x")]),
        ))
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::NOT_FOUND,
        "PUT traversal got {}",
        resp.status()
    );
    let resp = app
        .clone()
        .oneshot(authed_delete("/office/docs/.."))
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::NOT_FOUND,
        "DELETE traversal got {}",
        resp.status()
    );
    let resp = app
        .clone()
        .oneshot(authed_post_form("/office/library/docs/../delete", ""))
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::NOT_FOUND,
        "library delete traversal got {}",
        resp.status()
    );

    // Sentinel untouched, and no files created outside the office dir.
    assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "do-not-touch");
    let after: Vec<String> = walk_files(work_dir.path());
    let outside_before: Vec<&String> = before
        .iter()
        .filter(|p| !p.starts_with(office_parent.to_string_lossy().as_ref()))
        .collect();
    let mut outside_after: Vec<String> = after
        .iter()
        .filter(|p| !p.starts_with(office_parent.to_string_lossy().as_ref()))
        .cloned()
        .collect();
    let mut expected: Vec<String> = outside_before.into_iter().cloned().collect();
    // Allow the sqlite db + notes created by setup; just ensure no NEW outside files.
    outside_after.sort();
    expected.sort();
    // New files outside office/ other than db/notes/sentinel would show here; assert sentinel set unchanged
    for p in &outside_after {
        assert!(
            expected.contains(p) || p.ends_with("sentinel-outside.txt"),
            "unexpected outside file created: {}",
            p
        );
    }
    let _ = office_parent;
}

fn walk_files(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => {
                out.push(dir.to_string_lossy().to_string());
                continue;
            }
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.to_string_lossy().to_string());
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// P0: unsupported formulas surface #ERROR
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unsupported_formula_surfaces_error() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    app.clone()
        .oneshot(authed_post_form(
            "/office/sheets",
            &form(&[("title", "Error Sheet")]),
        ))
        .await
        .unwrap();

    // A1=5, B1=unsupported PRODUCT formula.
    let csv_data = "5,=PRODUCT(A1:A2)\n";
    let resp = app
        .clone()
        .oneshot(authed_post_form(
            "/office/sheets/error-sheet/import",
            &form(&[("csv_data", csv_data)]),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Export must contain #ERROR, not the raw formula.
    let resp = app
        .clone()
        .oneshot(authed_get("/office/sheets/error-sheet/export"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = body_bytes(resp).await;
    let content = String::from_utf8_lossy(&bytes).to_string();
    assert!(
        content.contains("#ERROR"),
        "expected #ERROR in export, got: {}",
        content
    );
    assert!(
        content.contains("unsupported formula"),
        "expected error semantics in export, got: {}",
        content
    );
    assert!(
        !content.contains(",=PRODUCT") && !content.contains("\n=PRODUCT"),
        "raw unsupported formula cell leaked into export: {}",
        content
    );

    // Rendered grid must also surface #ERROR, with supported-formula docs.
    let resp = app
        .clone()
        .oneshot(authed_get("/office/sheets/error-sheet"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_string(resp).await;
    assert!(html.contains("#ERROR"), "grid missing #ERROR: {}", html);
    assert!(
        html.contains("=SUM") && html.contains("=AVG"),
        "edit page must document supported formulas: {}",
        html
    );
}

// ---------------------------------------------------------------------------
// P1: document update with content-only form
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_document_update_content_only() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    app.clone()
        .oneshot(authed_post_form(
            "/office/docs",
            &form(&[("title", "Content Only"), ("content", "v1")]),
        ))
        .await
        .unwrap();

    // Editor form sends only `content` (no `title`) — must succeed, not 422.
    let resp = app
        .clone()
        .oneshot(authed_put_form(
            "/office/docs/content-only",
            &form(&[("content", "v2 updated")]),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "content-only update should succeed, not 422"
    );

    let resp = app
        .clone()
        .oneshot(authed_get("/office/docs/content-only"))
        .await
        .unwrap();
    let html = body_string(resp).await;
    assert!(html.contains("v2 updated"));
}

// ---------------------------------------------------------------------------
// P1: presentation create title-only + PUT slide update reachable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_presentation_create_title_only_and_update_slide() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    // Create form sends only `title` — must succeed, not 422.
    let resp = app
        .clone()
        .oneshot(authed_post_form(
            "/office/presentations",
            &form(&[("title", "Title Only Pres")]),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "title-only create should succeed, not 422"
    );
    assert_eq!(
        resp.headers()
            .get("HX-Redirect")
            .and_then(|v| v.to_str().ok()),
        Some("/office/presentations/title-only-pres")
    );

    // Add a slide, then update it via PUT.
    app.clone()
        .oneshot(authed_post_form(
            "/office/presentations/title-only-pres/slides",
            &form(&[("title", "S1"), ("content", "old")]),
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(authed_put_form(
            "/office/presentations/title-only-pres/slides/0",
            &form(&[("title", "S1 updated"), ("content", "new content")]),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify the update landed.
    let resp = app
        .clone()
        .oneshot(authed_get("/office/presentations/title-only-pres"))
        .await
        .unwrap();
    let html = body_string(resp).await;
    assert!(html.contains("S1 updated"));
    assert!(html.contains("new content"));
    // Each slide card must expose an hx-put update form with a Save button.
    assert!(
        html.contains("hx-put=\"/office/presentations/title-only-pres/slides/0\""),
        "slide update form not reachable: {}",
        html
    );
    assert!(html.contains("Guardar"));
}

// ---------------------------------------------------------------------------
// P1: reorder rejects non-permutations with 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_presentation_reorder_rejects_non_permutation() {
    let (state, _work_dir) = setup().await;
    let app = study_platform::handlers::router(state);

    app.clone()
        .oneshot(authed_post_form(
            "/office/presentations",
            &form(&[("title", "Reorder Pres")]),
        ))
        .await
        .unwrap();
    for (t, c) in [("A", "a"), ("B", "b")] {
        app.clone()
            .oneshot(authed_post_form(
                "/office/presentations/reorder-pres/slides",
                &form(&[("title", t), ("content", c)]),
            ))
            .await
            .unwrap();
    }

    // Duplicate index is not a permutation → 400.
    let resp = app
        .clone()
        .oneshot(authed_post_form(
            "/office/presentations/reorder-pres/slides/reorder",
            &form(&[("order", "0,0")]),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Missing index is not a permutation → 400.
    let resp = app
        .clone()
        .oneshot(authed_post_form(
            "/office/presentations/reorder-pres/slides/reorder",
            &form(&[("order", "0")]),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Valid permutation still works.
    let resp = app
        .clone()
        .oneshot(authed_post_form(
            "/office/presentations/reorder-pres/slides/reorder",
            &form(&[("order", "1,0")]),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
