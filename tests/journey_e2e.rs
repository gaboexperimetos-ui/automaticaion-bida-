//! Test E2E del **flujo completo de un usuario** (`Agent: subagent Rust tests`).
//!
//! Un único test recorre, con la **misma sesión** de principio a fin, el
//! camino que recorre una persona real en la aplicación:
//!
//! 1. `POST /login` (usuario `admin` sembrado) → 302 + cookie de sesión.
//! 2. `POST /app/notes` (formulario) → 303 + `Location` a la nota creada.
//! 3. `GET /app/notes/{slug}` → 200 con el contenido renderizado.
//! 4. `PUT /app/notes/{slug}/update` (formulario) → 200 + cambio aplicado.
//! 5. `GET /app/notes` → 200 con el título de la nota en el listado.
//! 6. `GET /api/search?q=…` → 200 con la nota entre los resultados.
//! 7. `DELETE /app/notes/{slug}/delete` → 200 con la nota eliminada.
//! 8. `GET /app/notes/{slug}` → 404: la nota ya no existe.
//!
//! Cada paso afirma el estado HTTP **exacto** que devuelve la ruta real y
//! explica en el mensaje qué se esperaba: si una ruta cambia de método, de
//! estado o de forma de la respuesta, el test falla con el detalle en vez de
//! dejar pasar la regresión.
//!
//! Nota sobre el método del paso 4: la ruta `/app/notes/{slug}/update` está
//! registrada como `put(notes::update)` (`src/handlers/mod.rs`) y el editor
//! real la usa con `hx-put` (`src/handlers/notes.rs`), por lo que el flujo
//! auténtico es **PUT**. Un `POST` a esa misma ruta responde 405.
//!
//! Harness autocontenido (mismo patrón que `tests/admin_routes.rs`): SQLite
//! temporal con las migraciones reales, usuario `admin` sembrado y bóveda de
//! notas en un directorio temporal. Sin red, sin datos reales, sin escritura
//! fuera del directorio temporal.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use password_hash::PasswordHasher;
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::models::Note;
use study_platform::AppState;

/// Usuario sembrado para el journey.
const ADMIN_USER: &str = "admin";
/// Contraseña del usuario sembrado (Argon2, hash creado en el test).
const ADMIN_PASSWORD: &str = "journey-e2e-password";

/// Título de la nota creada en el paso 2.
const TITULO: &str = "Nota Journey E2E";
/// Título tras la edición del paso 4.
const TITULO_EDITADO: &str = "Nova Journey E2E";
/// Contenido inicial (paso 2).
const CONTENIDO: &str = "Cuerpo original journey con marcador-original dentro.";
/// Contenido tras la edición (paso 4).
const CONTENIDO_EDITADO: &str = "Cuerpo editado journey con marcador-editado dentro.";
/// Etiquetas de la nota.
const ETIQUETAS: &str = "journey,e2e";
/// Palabra del título usada en la búsqueda del paso 6.
const PALABRA_BUSQUEDA: &str = "journey";

/// Configuración de test: sin API key (modo mock), auth activada y rutas MCP
/// muertas (probe hermético, sin red).
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18093,
        session_secret: "journey_e2e_test_secret_at_least_32chars!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: None,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://127.0.0.1:57777/mcp".to_string(),
        genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        registration_token: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
    }
}

/// Harness: estado de la app + temporales que deben vivir hasta el final.
struct TestApp {
    state: Arc<AppState>,
    /// Bóveda de notas temporal (se comprueba en el paso 2).
    notes_dir: PathBuf,
    // Mantiene vivos el directorio de notas y el fichero de base de datos.
    _keep: (tempfile::TempDir, tempfile::NamedTempFile),
}

/// Levanta la app completa: migraciones reales + usuario `admin` sembrado.
/// No se siembra ninguna sesión: la cookie se obtiene con un login real
/// (paso 1 del journey).
async fn setup_app() -> TestApp {
    let notes_tmp = tempfile::tempdir().unwrap();
    let notes_dir = notes_tmp.path().to_path_buf();
    let db_tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = db_tmp.path().to_path_buf();
    let db_url = format!("sqlite://{}", db_path.display());

    let config = test_config(notes_dir.clone(), db_path);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .expect("no se pudo crear la base de datos de test");

    sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await.ok();
    sqlx::query("PRAGMA foreign_keys=ON;").execute(&pool).await.ok();
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("no se pudieron aplicar las migraciones");

    // Usuario `admin` con contraseña conocida (Argon2, como en producción).
    let salt = argon2::password_hash::SaltString::generate(rand::thread_rng());
    let password_hash = argon2::Argon2::default()
        .hash_password(ADMIN_PASSWORD.as_bytes(), &salt)
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
    .expect("no se pudo sembrar el usuario admin");

    let state = Arc::new(AppState {
        config: config.clone(),
        db: pool,
        mikareno: None,
        start_time: std::time::Instant::now(),
    });

    TestApp {
        state,
        notes_dir,
        _keep: (notes_tmp, db_tmp),
    }
}

/// Petición cruda a través del router real.
async fn send(
    app: &axum::Router,
    method: Method,
    uri: &str,
    cookie: Option<&str>,
    form: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    let body = match form {
        Some(f) => {
            builder = builder.header(
                header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            );
            Body::from(f.to_string())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .expect("el router no respondió")
}

/// Cuerpo de la respuesta como texto.
async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("no se pudo leer el cuerpo");
    String::from_utf8_lossy(&bytes).to_string()
}

/// Percent-encode mínimo para cuerpos `application/x-www-form-urlencoded`.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Cuerpo de formulario `a=1&b=2`.
fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Cabecera `Location` de la respuesta (o cadena vacía).
fn location(resp: &axum::response::Response) -> String {
    resp.headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Journey completo: login → crear → ver → editar → listar → buscar → borrar
/// → verificar que ya no está.
#[tokio::test]
async fn journey_completo_de_un_usuario() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    // ------------------------------------------------------------------
    // Paso 1 — login: POST /login con el usuario `admin` sembrado.
    // ------------------------------------------------------------------
    let resp = send(
        &app,
        Method::POST,
        "/login",
        None,
        Some(&form(&[
            ("username", ADMIN_USER),
            ("password", ADMIN_PASSWORD),
        ])),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::FOUND,
        "Paso 1 (login): se esperaba 302 FOUND, se recibió {}",
        resp.status()
    );
    assert_eq!(
        location(&resp),
        "/dashboard",
        "Paso 1 (login): el login exitoso debe redirigir a /dashboard"
    );

    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let cookie = set_cookie
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    assert!(
        cookie.starts_with("session_id=") && cookie.len() > "session_id=".len(),
        "Paso 1 (login): la respuesta debe traer la cookie de sesión `session_id`, se recibió: {set_cookie}"
    );
    let cookie = cookie.as_str();

    // ------------------------------------------------------------------
    // Paso 2 — crear nota: POST /app/notes (formulario) → 303 + Location.
    // ------------------------------------------------------------------
    let slug = Note::slugify(TITULO);
    let resp = send(
        &app,
        Method::POST,
        "/app/notes",
        Some(cookie),
        Some(&form(&[
            ("title", TITULO),
            ("content", CONTENIDO),
            ("tags", ETIQUETAS),
        ])),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "Paso 2 (crear nota): se esperaba 303 SEE_OTHER, se recibió {}",
        resp.status()
    );
    assert_eq!(
        location(&resp),
        format!("/app/notes/{}", slug),
        "Paso 2 (crear nota): la redirección debe apuntar a la nota recién creada"
    );
    assert!(
        t.notes_dir.join(format!("{}.md", slug)).exists(),
        "Paso 2 (crear nota): el Markdown {slug}.md debe existir en la bóveda de notas"
    );

    // ------------------------------------------------------------------
    // Paso 3 — ver la nota: GET /app/notes/{slug} → 200 con el contenido.
    // ------------------------------------------------------------------
    let resp = send(&app, Method::GET, &format!("/app/notes/{}", slug), Some(cookie), None).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "Paso 3 (ver nota): se esperaba 200 OK, se recibió {}",
        resp.status()
    );
    let html = body_text(resp).await;
    assert!(
        html.contains(TITULO),
        "Paso 3 (ver nota): el HTML debe mostrar el título `{TITULO}`"
    );
    assert!(
        html.contains("marcador-original"),
        "Paso 3 (ver nota): el HTML debe mostrar el contenido renderizado de la nota"
    );

    // ------------------------------------------------------------------
    // Paso 4 — editar: la ruta real es PUT (el editor usa `hx-put`);
    // responde 200 con el mensaje JSON y el cambio debe verse después.
    // ------------------------------------------------------------------
    let resp = send(
        &app,
        Method::PUT,
        &format!("/app/notes/{}/update", slug),
        Some(cookie),
        Some(&form(&[
            ("title", TITULO_EDITADO),
            ("content", CONTENIDO_EDITADO),
            ("tags", ETIQUETAS),
        ])),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "Paso 4 (editar nota): se esperaba 200 OK, se recibió {}",
        resp.status()
    );
    let json: serde_json::Value =
        serde_json::from_str(&body_text(resp).await).expect("el update debe devolver JSON");
    assert_eq!(
        json["message"], serde_json::json!("Note updated"),
        "Paso 4 (editar nota): la respuesta debe confirmar la actualización, se recibió: {json}"
    );

    let resp = send(&app, Method::GET, &format!("/app/notes/{}", slug), Some(cookie), None).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "Paso 4 (editar nota): la nota debe seguir visible tras editarla, se recibió {}",
        resp.status()
    );
    let html = body_text(resp).await;
    assert!(
        html.contains(TITULO_EDITADO),
        "Paso 4 (editar nota): debe verse el título nuevo `{TITULO_EDITADO}`"
    );
    assert!(
        html.contains("marcador-editado"),
        "Paso 4 (editar nota): debe verse el contenido nuevo"
    );
    assert!(
        !html.contains("marcador-original"),
        "Paso 4 (editar nota): el contenido antiguo no debe seguir en la nota"
    );

    // ------------------------------------------------------------------
    // Paso 5 — listar: GET /app/notes → 200 con el título de la nota.
    // ------------------------------------------------------------------
    let resp = send(&app, Method::GET, "/app/notes", Some(cookie), None).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "Paso 5 (listar notas): se esperaba 200 OK, se recibió {}",
        resp.status()
    );
    let html = body_text(resp).await;
    assert!(
        html.contains(TITULO_EDITADO),
        "Paso 5 (listar notas): el listado debe incluir la nota `{TITULO_EDITADO}`"
    );
    assert!(
        html.contains(&format!("/app/notes/{}", slug)),
        "Paso 5 (listar notas): el listado debe enlazar a la nota con su slug"
    );

    // ------------------------------------------------------------------
    // Paso 6 — buscar: GET /api/search?q=… → 200 con la nota en los
    // resultados.
    // ------------------------------------------------------------------
    let resp = send(
        &app,
        Method::GET,
        &format!("/api/search?q={}", PALABRA_BUSQUEDA),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "Paso 6 (buscar): se esperaba 200 OK, se recibió {}",
        resp.status()
    );
    let json: serde_json::Value =
        serde_json::from_str(&body_text(resp).await).expect("la búsqueda debe devolver JSON");
    let results = json["results"].as_array().unwrap_or_else(|| {
        panic!("Paso 6 (buscar): la respuesta debe traer el array `results`, se recibió: {json}")
    });
    assert!(
        results
            .iter()
            .any(|r| r["slug"] == serde_json::json!(slug) && r["title"] == serde_json::json!(TITULO_EDITADO)),
        "Paso 6 (buscar): la nota `{TITULO_EDITADO}` debe aparecer en los resultados, se recibió: {json}"
    );

    // ------------------------------------------------------------------
    // Paso 7 — borrar: DELETE /app/notes/{slug}/delete → 200 con la nota
    // eliminada.
    // ------------------------------------------------------------------
    let resp = send(
        &app,
        Method::DELETE,
        &format!("/app/notes/{}/delete", slug),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "Paso 7 (borrar nota): se esperaba 200 OK, se recibió {}",
        resp.status()
    );
    let json: serde_json::Value =
        serde_json::from_str(&body_text(resp).await).expect("el borrado debe devolver JSON");
    assert_eq!(
        json["message"], serde_json::json!("Note deleted"),
        "Paso 7 (borrar nota): la respuesta debe confirmar el borrado, se recibió: {json}"
    );
    assert!(
        !t.notes_dir.join(format!("{}.md", slug)).exists(),
        "Paso 7 (borrar nota): el Markdown {slug}.md debe haberse borrado de la bóveda"
    );

    // ------------------------------------------------------------------
    // Paso 8 — verificar que ya no está: GET /app/notes/{slug} → 404.
    // ------------------------------------------------------------------
    let resp = send(&app, Method::GET, &format!("/app/notes/{}", slug), Some(cookie), None).await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "Paso 8 (nota borrada): se esperaba 404 NOT_FOUND, se recibió {}",
        resp.status()
    );
}
