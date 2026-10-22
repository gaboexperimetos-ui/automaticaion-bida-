//! Integration tests for wikilinks and the note index (TOC) on `/app/notes/{slug}`.
//!
//! Harness copiado de `tests/admin_routes.rs` (mismo patrón que
//! `tests/notes_ui.rs`): SQLite en directorio temporal con las migraciones
//! reales (`sqlx::migrate!()`), usuario `admin` sembrado con Argon2 y una
//! sesión en DB (`SHA256(secret:token)`) que viaja en la cookie `session_id`.
//! Cada test usa una IP de TEST-NET-1 propia para que el limitador global
//! (60/min) nunca toque.
//!
//! Las dos notas del contrato se crean **por SQL** (INSERT directo en la
//! tabla `notes`, columnas reales: title, slug, filename, tags, word_count,
//! created_at, updated_at) y su `.md` se escribe en el `notes_dir` del
//! harness, que es de donde `notes::view` lee el cuerpo.
//!
//! Contrato verificado (`src/handlers/notes.rs` + `src/knowledge/markdown.rs`):
//! - `GET /app/notes/{slug}` → 200 con el índice de la nota
//!   (`<nav aria-label="Índice de la nota">`) cuando la nota tiene ≥2
//!   headings, y con los `<h2 id=…>` del render.
//! - `[[slug-existente]]` → enlace real `href="/app/notes/slug-existente"`
//!   (nunca un ancla `#slug`).
//! - `[[no-existe-xyz]]` → texto plano: el HTML **no** contiene
//!   `/app/notes/no-existe-xyz` (enlace roto nunca rompe el render).
//! - `GET /app/notes/slug-inexistente` → 404 (nunca 500).
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

/// Slug de la nota origen (la que se abre y contiene los wikilinks).
const SLUG_ORIGEN: &str = "nota-origen";

/// Slug de la nota destino (existe ⇒ el wikilink se vuelve enlace real).
const SLUG_DESTINO: &str = "nota-destino";

/// Slug de un wikilink roto: no hay ninguna nota con ese slug.
const SLUG_ROTO: &str = "no-existe-xyz";

/// Cuerpo de la nota origen: dos headings (⇒ índice) y los dos wikilinks.
const CONTENIDO_ORIGEN: &str = "## Sección Uno

Texto con un enlace a [[nota-destino]] y otro roto: [[no-existe-xyz]].

## Sección Dos

Segunda sección del cuerpo de prueba.
";

/// Cuerpo de la nota destino (un solo heading: no necesita índice).
const CONTENIDO_DESTINO: &str = "## Nota Destino

Cuerpo mínimo de la nota destino.
";

/// Configuración de test: sin API key (mock), auth activada, MCP inalcanzable.
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18096,
        session_secret: "wikilinks_toc_test_secret_at_least_32chars!!".to_string(),
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

/// Levanta la app: migraciones reales, usuario `admin` sembrado, sesión
/// válida en DB (token opaco hasheado, formato Mission 11) y las dos notas
/// del contrato sembradas por SQL.
async fn setup_app() -> TestApp {
    let notes_tmp = tempfile::tempdir().expect("temp dir de notas");
    let notes_dir = notes_tmp.path().to_path_buf();
    let db_tmp = tempfile::NamedTempFile::new().expect("fichero de BD");
    let config = test_config(notes_dir.clone(), db_tmp.path().to_path_buf());
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

    // Las dos notas del contrato: fila en `notes` + fichero `.md` en disco.
    seed_note(
        &pool,
        &notes_dir,
        "Nota Origen",
        SLUG_ORIGEN,
        CONTENIDO_ORIGEN,
    )
    .await;
    seed_note(
        &pool,
        &notes_dir,
        "Nota Destino",
        SLUG_DESTINO,
        CONTENIDO_DESTINO,
    )
    .await;

    let state = Arc::new(AppState {
        cache_mikareno: std::sync::Arc::new(
            study_platform::academic::calendar_data::CacheMikareno::default(),
        ),
        cache_catalogo: std::sync::Arc::new(study_platform::ai::modelos::CacheCatalogo::default()),
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

/// Crea una nota de prueba: escribe su `.md` en el `notes_dir` (de donde
/// `notes::view` lee el cuerpo) e inserta la fila en la tabla `notes`.
async fn seed_note(
    pool: &sqlx::SqlitePool,
    notes_dir: &std::path::Path,
    title: &str,
    slug: &str,
    content: &str,
) {
    let filename = format!("{slug}.md");
    std::fs::write(notes_dir.join(filename.as_str()), content).expect("escribir .md de la nota");

    let now = chrono::Utc::now().to_rfc3339();
    let word_count = content.split_whitespace().count() as i64;
    sqlx::query(
        "INSERT INTO notes (title, slug, filename, tags, word_count, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(title)
    .bind(slug)
    .bind(&filename)
    .bind("prueba")
    .bind(word_count)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .expect("insertar nota en la tabla notes");
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

/// Cuerpo de la respuesta como texto.
async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("leer cuerpo");
    String::from_utf8_lossy(&bytes).to_string()
}

// --- 1. Vista de la nota: 200 con el índice (≥2 headings) y los <h2> ---

#[tokio::test]
async fn ver_nota_muestra_indice_con_dos_headings_y_h2_renderizados() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(
        app,
        &format!("/app/notes/{SLUG_ORIGEN}"),
        &t.cookie,
        "192.0.2.41",
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "la nota existente debe servirse con 200, llegó {}",
        resp.status()
    );

    let html = body_text(resp).await;
    // Índice de la nota: el <nav> que emite `knowledge::markdown::toc_html`
    // (sólo se emite con 2 o más headings).
    assert!(
        html.contains(r#"aria-label="Índice de la nota""#),
        "la nota con 2 headings debe incluir su índice (nav)"
    );
    // Los anchors del índice apuntan a los ids de los <h2> del cuerpo.
    assert!(
        html.contains(r##"href="#seccion-uno""##) && html.contains(r##"href="#seccion-dos""##),
        "el índice debe enlazar cada heading del cuerpo"
    );
    // Y el cuerpo renderiza esos headings como <h2 id=…> (no texto plano).
    assert!(
        html.contains(r#"<h2 id="seccion-uno""#),
        "el heading de la nota debe renderizarse como <h2 id=…>"
    );
    assert!(
        html.contains(r#"<h2 id="seccion-dos""#),
        "el segundo heading de la nota debe renderizarse como <h2 id=…>"
    );
    assert!(
        html.contains("Sección Uno") && html.contains("Sección Dos"),
        "el texto de los headings debe aparecer en el cuerpo renderizado"
    );
}

// --- 2. Wikilink existente: enlace real a la URL de la nota ---

#[tokio::test]
async fn wikilink_existente_apunta_a_la_url_real_de_la_nota() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(
        app,
        &format!("/app/notes/{SLUG_ORIGEN}"),
        &t.cookie,
        "192.0.2.42",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let html = body_text(resp).await;
    assert!(
        html.contains(&format!(r#"href="/app/notes/{SLUG_DESTINO}""#)),
        "un wikilink a una nota existente debe enlazar su URL real"
    );
    // Nunca un ancla `#slug`: el enlace va a la página de la nota.
    assert!(
        !html.contains(&format!("#{SLUG_DESTINO}")),
        "el wikilink no debe resolverse como ancla interna"
    );
}

// --- 3. Wikilink roto: texto plano, sin enlace a una URL inexistente ---

#[tokio::test]
async fn wikilink_roto_queda_como_texto_plano_sin_enlace() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(
        app,
        &format!("/app/notes/{SLUG_ORIGEN}"),
        &t.cookie,
        "192.0.2.43",
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "el render nunca debe romperse"
    );

    let html = body_text(resp).await;
    assert!(
        !html.contains(&format!("/app/notes/{SLUG_ROTO}")),
        "un wikilink roto no debe producir un enlace a {SLUG_ROTO}"
    );
    // El texto sí aparece (visible), pero sin href.
    assert!(
        html.contains(SLUG_ROTO),
        "el wikilink roto debe quedar visible como texto plano"
    );
}

// --- 4. Slug inexistente: 404 honesto (nunca 500) ---

#[tokio::test]
async fn slug_inexistente_devuelve_404_no_500() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = get(
        app,
        "/app/notes/slug-que-no-existe",
        &t.cookie,
        "192.0.2.44",
    )
    .await;

    let status = resp.status();
    assert_ne!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "un slug inexistente nunca debe producir un 500"
    );
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "un slug inexistente debe devolver 404, llegó {status}"
    );
}
