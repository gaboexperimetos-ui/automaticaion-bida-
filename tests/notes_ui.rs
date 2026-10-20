//! Integration tests for the notes UI (`/app/notes`).
//!
//! Harness copiado de `tests/admin_routes.rs`: SQLite en directorio temporal
//! con las migraciones reales, usuario `admin` sembrado con Argon2 y una
//! sesión en DB (`SHA256(secret:token)`) reutilizada tal cual en la cookie
//! `session_id`, igual que hace el navegador. Cada test usa una IP de
//! TEST-NET-1 propia para que el limitador global (60/min) nunca toque.
//!
//! Contrato verificado (rutas reales de `src/handlers/notes.rs`):
//! - `POST /app/notes` (form urlencoded) → 303 + `Location: /app/notes/<slug>`.
//! - `GET /app/notes/<slug>` → 200 con el Markdown renderizado por el parser
//!   vivo (`src/knowledge/markdown.rs`): `<h2 …>` + contenedor `note-render`.
//! - `GET /app/notes/<slug-inexistente>` → 404 honesto (nunca 500).
//! - `GET /app/notes` → 200 listando la nota por su título.
//!
//! Hermético: sin red, sin datos reales, nada se escribe fuera del temp dir.

use std::path::PathBuf;
use std::sync::Arc;

use argon2::Argon2;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use password_hash::{PasswordHasher, SaltString};
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::middleware::auth::{generate_session_token, hash_session_token};
use study_platform::AppState;

/// Usuario sembrado para las peticiones autenticadas.
const ADMIN_USER: &str = "admin";

/// Formulario de creación de la nota de la prueba.
/// `subject=general` es un campo que el modelo ignora (serde no lo deniega):
/// el alta debe funcionar igual, con o sin él.
const FORM_CREAR: &str = "title=Sprint+Markdown&subject=general&content=%23+Hola+HTML+desde+la+UI";

/// Slug esperado para el título `Sprint Markdown`.
const SLUG: &str = "sprint-markdown";

/// Configuración de test: sin API key (mock), auth activada, MCP inalcanzable.
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18094,
        session_secret: "notes_ui_test_secret_at_least_32_chars!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: None,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://192.0.2.20:8080/mcp".to_string(), // no-localhost ⇒ sin cliente
        genoffice_mcp_url: "http://192.0.2.20:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        registration_token: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
    }
}

/// Estado de la app de test + temporales que lo sostienen.
struct TestApp {
    state: Arc<AppState>,
    cookie: String,
    // Mantiene vivos el directorio de notas y el fichero de la DB.
    _keep: (tempfile::TempDir, tempfile::NamedTempFile),
}

/// Levanta la app: migraciones reales, usuario `admin` sembrado y sesión
/// válida en DB (token opaco hasheado, formato Mission 11).
async fn setup_app() -> TestApp {
    let notes_tmp = tempfile::tempdir().expect("temp dir de notas");
    let db_tmp = tempfile::NamedTempFile::new().expect("fichero de BD");
    let config = test_config(notes_tmp.path().to_path_buf(), db_tmp.path().to_path_buf());
    let secret = config.session_secret.clone();

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", db_tmp.path().display()))
        .await
        .expect("conectar a la BD de test");
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
        .expect("ejecutar migraciones");

    // Semilla de `admin` con contraseña conocida.
    let salt = SaltString::generate(rand::thread_rng());
    let password_hash = Argon2::default()
        .hash_password(b"testpassword123", &salt)
        .expect("hashear contraseña")
        .to_string();
    sqlx::query(
        "INSERT INTO users (username, password_hash) VALUES (?, ?) \
         ON CONFLICT(username) DO UPDATE SET password_hash = EXCLUDED.password_hash",
    )
    .bind(ADMIN_USER)
    .bind(&password_hash)
    .execute(&pool)
    .await
    .expect("sembrar usuario admin");
    let uid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(ADMIN_USER)
        .fetch_one(&pool)
        .await
        .expect("id de admin");

    // Sesión en DB: la cookie lleva el token en claro, la fila guarda el hash.
    let token = generate_session_token();
    let hashed = hash_session_token(&token, &secret);
    let expires = chrono::Utc::now() + chrono::Duration::hours(24);
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(&hashed)
        .bind(uid)
        .bind(expires.to_rfc3339())
        .execute(&pool)
        .await
        .expect("sembrar sesión");

    let state = Arc::new(AppState {
        config,
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

/// GET contra el router real con la cookie de sesión.
async fn get(app: axum::Router, uri: &str, cookie: &str, ip: &str) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .uri(uri)
            .header("cookie", cookie)
            .header("x-forwarded-for", ip)
            .body(Body::empty())
            .expect("construir request"),
    )
    .await
    .expect("respuesta del router")
}

/// POST de formulario urlencoded contra el router real.
async fn post_form(
    app: axum::Router,
    uri: &str,
    cookie: &str,
    ip: &str,
    body: &'static str,
) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", cookie)
            .header("x-forwarded-for", ip)
            .body(Body::from(body))
            .expect("construir request"),
    )
    .await
    .expect("respuesta del router")
}

/// Cuerpo de la respuesta como texto.
async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("leer cuerpo");
    String::from_utf8_lossy(&bytes).to_string()
}

// --- 1. Crear la nota por formulario: 303 + Location al slug ---

#[tokio::test]
async fn crear_nota_por_form_responde_303_al_slug() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = post_form(app, "/app/notes", &t.cookie, "192.0.2.31", FORM_CREAR).await;

    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "el alta por formulario debe redirigir (303), llegó {}",
        resp.status()
    );
    let esperado = format!("/app/notes/{SLUG}");
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some(esperado.as_str()),
        "el Location debe apuntar a la nota recién creada"
    );
}

// --- 2. Ver la nota: 200 con el Markdown renderizado (parser vivo) ---

#[tokio::test]
async fn ver_nota_renderiza_markdown_con_h2_y_note_render() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    // Alta previa (misma IP ⇒ mismo bucket del limitador).
    let resp = post_form(
        app.clone(),
        "/app/notes",
        &t.cookie,
        "192.0.2.32",
        FORM_CREAR,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = get(app, &format!("/app/notes/{SLUG}"), &t.cookie, "192.0.2.32").await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "la nota existente debe servirse con 200, llegó {}",
        resp.status()
    );

    let html = body_text(resp).await;
    // Contenedor del render Markdown (`render_note`).
    assert!(
        html.contains("note-render"),
        "la vista debe incluir el contenedor 'note-render'"
    );
    // Heading renderizado por `knowledge::markdown::render_html` (nivel 1-3 → <h2>).
    assert!(
        html.contains("<h2"),
        "el Markdown debe renderizarse como heading <h2>, no como texto plano"
    );
    // El texto del heading escapado, tal como lo emite el parser.
    assert!(
        html.contains("Hola HTML desde la UI"),
        "el contenido de la nota debe aparecer renderizado"
    );
}

// --- 3. Nota inexistente: 404 honesto (nunca 500) ---

#[tokio::test]
async fn ver_nota_inexistente_devuelve_404_no_500() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(app, "/app/notes/no-existe", &t.cookie, "192.0.2.33").await;

    let status = resp.status();
    // Nunca un 5xx: la app no debe romperse por un slug que no existe.
    assert_ne!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "una nota inexistente nunca debe producir un 500"
    );
    // Código honesto: 404 (lo que devuelve `notes::view`) o un redirect.
    assert!(
        status == StatusCode::NOT_FOUND || status.is_redirection(),
        "una nota inexistente debe devolver 404 o redirigir, llegó {status}"
    );

    let html = body_text(resp).await;
    assert!(
        html.to_lowercase().contains("not found"),
        "la respuesta debe explicar honestamente que la nota no existe"
    );
}

// --- 4. Listado: 200 con el título de la nota creada ---

#[tokio::test]
async fn listar_notas_muestra_el_titulo_creado() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = post_form(
        app.clone(),
        "/app/notes",
        &t.cookie,
        "192.0.2.34",
        FORM_CREAR,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = get(app, "/app/notes", &t.cookie, "192.0.2.34").await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "el listado de notas debe servirse con 200, llegó {}",
        resp.status()
    );

    let html = body_text(resp).await;
    assert!(
        html.contains("Sprint Markdown"),
        "el listado debe mostrar el título de la nota creada"
    );
    assert!(
        html.contains(&format!("/app/notes/{SLUG}")),
        "el listado debe enlazar a la nota por su slug"
    );
}
