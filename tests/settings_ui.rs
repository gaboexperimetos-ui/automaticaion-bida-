//! Integration tests for the settings page (`GET /settings`).
//!
//! Contract under test (as implemented in `src/handlers/settings.rs`, route
//! registered inside the `protected` router of `src/handlers/mod.rs`):
//! - `GET /settings` without a session → rejected (302 → `/login` for pages,
//!   401 would be the API-family answer; both are accepted here).
//! - `GET /settings` with a session → 200 with the Stitch v2 shell and the
//!   "IA / OpenRouter" card built from the *real* process configuration:
//!   - no API key configured → honest banner "Key no definida",
//!   - budget rendered as "$2.00 USD / mes" (from `openrouter_max_cost_usd`),
//!   - the API key itself is NEVER rendered, not even partially: the page
//!     shows only its presence, so the body must not contain "sk-".
//!
//! Self-contained harness (same shape as `tests/admin_routes.rs`): temp-dir
//! SQLite running the real migrations, a seeded user and a DB-backed session
//! (`SHA256(secret:token)`) minted through the public auth helpers.
//! Hermetic: no network — the MCP URLs are not localhost, so the Mikareno
//! client is never created and no probe is attempted.

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

/// Usuario sembrado para los tests autenticados.
const TEST_USER: &str = "estudiante";

/// Clave falsa usada sólo para comprobar que la página nunca la imprime.
const FAKE_KEY: &str = "sk-or-v1-clave-falsa-000000000000000000";

/// Configuración de test: presupuesto fijo en $2.00, sin clave de API y sin
/// MCP (URL fuera de loopback/RFC1918/CGNAT ⇒ `MikarenoClient::from_config`
/// devuelve `None` y no se intenta ninguna conexión). Autenticación activada.
fn test_config(notes_dir: PathBuf, db_path: PathBuf, api_key: Option<String>) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18093,
        session_secret: "settings_ui_test_secret_at_least_32chars!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: api_key,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://mcp.invalid:8080/mcp".to_string(),
        genoffice_mcp_url: "http://genoffice.invalid:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        registration_token: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
    }
}

/// Handle que mantiene vivos los paths temporales durante el test.
struct TestApp {
    state: Arc<AppState>,
    cookie: String,
    // Mantiene vivos el dir de notas y el fichero SQLite hasta el final.
    _keep: (tempfile::TempDir, tempfile::NamedTempFile),
}

/// Monta el estado real de la app: migraciones de verdad, usuario sembrado y
/// una sesión válida en base de datos (hash del token, formato Mission 11).
async fn setup_app(api_key: Option<String>) -> TestApp {
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

    sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await.ok();
    sqlx::query("PRAGMA foreign_keys=ON;").execute(&pool).await.ok();
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    // Usuario sembrado con contraseña conocida (mismo patrón que admin_routes.rs).
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
    .bind(TEST_USER)
    .bind(&password_hash)
    .execute(&pool)
    .await
    .expect("Failed to seed user");
    let uid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(TEST_USER)
        .fetch_one(&pool)
        .await
        .expect("user id");

    // Sesión en base de datos: se guarda el hash del token; la cookie lleva
    // el token en claro (exactamente como en un login real).
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

/// GET a través del router real, con o sin cookie de sesión.
async fn get(app: axum::Router, uri: &str, cookie: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().uri(uri);
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    app.oneshot(builder.body(Body::empty()).unwrap()).await.unwrap()
}

/// Body de la respuesta como texto (HTML).
async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

// --- 1. Sin sesión: la página de configuración está protegida ---

#[tokio::test]
async fn settings_page_requires_a_session() {
    let t = setup_app(None).await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(app, "/settings", None).await;
    let status = resp.status();
    assert!(
        status == StatusCode::FOUND || status == StatusCode::UNAUTHORIZED,
        "sin sesión /settings debe redirigir (302) o rechazar (401); llegó {status}"
    );
    if status == StatusCode::FOUND {
        assert_eq!(
            resp.headers().get("location").and_then(|v| v.to_str().ok()),
            Some("/login"),
            "/settings sin sesión debe redirigir a /login"
        );
    }
}

// --- 2. Con sesión: tarjeta IA / OpenRouter con estado honesto y presupuesto ---

#[tokio::test]
async fn settings_page_shows_openrouter_card_and_budget() {
    let t = setup_app(None).await;
    let app = study_platform::handlers::router(t.state.clone());
    let cookie = t.cookie.clone();

    let resp = get(app.clone(), "/settings", Some(&cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_text(resp).await;

    // La tarjeta "IA / OpenRouter" existe (no la antigua "Asistente de IA").
    assert!(
        html.contains("IA / OpenRouter"),
        "la página debe mostrar la tarjeta \"IA / OpenRouter\""
    );
    assert!(
        html.contains("Modelo"),
        "la tarjeta debe mostrar la etiqueta \"Modelo\""
    );
    // Sin clave configurada: banner honesto, nunca una key.
    assert!(
        html.contains("Key no definida"),
        "sin OPENROUTER_API_KEY debe mostrarse el banner \"Key no definida\""
    );
    // Presupuesto visible: `format!("${:.2} USD / mes", 2.0)`.
    assert!(
        html.contains("$2.00"),
        "el presupuesto mensual ($2.00) debe ser visible en la página"
    );
    // Ninguna clave (ni siquiera un fragmento) puede aparecer en el HTML.
    assert!(
        !html.contains("sk-"),
        "la página de configuración no debe contener ninguna clave de API"
    );
}

// --- 3. Con clave configurada: se muestra su presencia, nunca su valor ---

#[tokio::test]
async fn settings_page_never_renders_the_api_key() {
    let t = setup_app(Some(FAKE_KEY.to_string())).await;
    let app = study_platform::handlers::router(t.state.clone());
    let cookie = t.cookie.clone();

    let resp = get(app, "/settings", Some(&cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_text(resp).await;

    assert!(
        !html.contains(FAKE_KEY),
        "la clave configurada no debe aparecer en el HTML"
    );
    assert!(
        !html.contains("sk-"),
        "ningún fragmento con prefijo de clave (\"sk-\") debe aparecer en el HTML"
    );
    assert!(
        html.contains("IA / OpenRouter"),
        "la tarjeta \"IA / OpenRouter\" debe seguir presente con clave configurada"
    );
}
