//! Integration tests for the schedule UI (`GET /dashboard/horario`).
//!
//! Harness copiado (mínimo) de `tests/admin_routes.rs`: SQLite en directorio
//! temporal con las migraciones reales, usuario `byron` sembrado con Argon2 y
//! un **login real** (`POST /login`) cuyo `Set-Cookie` se reutiliza tal cual en
//! la petición autenticada — así se ejercita el mismo camino que el navegador
//! (cookie `session_id` opaca, fila en `sessions` hasheada).
//!
//! Contrato verificado:
//! - Sin sesión, el middleware de auth rechaza la página (302 → `/login`).
//! - Con sesión, la página responde 200 y dibuja la grilla semanal de **7
//!   columnas** (Lunes … Domingo), con el estado honesto "Sin clases" en los
//!   días sin datos (una semana puede tener días vacíos; no se inventa nada).
//! - Sin Mikareno (cliente `None`) la página tampoco falla con 5xx: propaga un
//!   estado honesto ("Mikareno no conectado" / "Sin clases").
//!
//! Hermético: la URL del MCP es no-localhost, así que
//! `MikarenoClient::from_config` devuelve `None` y la página no hace I/O.

use std::path::PathBuf;
use std::sync::Arc;

use argon2::password_hash::{PasswordHasher, SaltString};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::AppState;

/// Usuario sembrado y usado en el login real.
const USER: &str = "byron";
/// Contraseña conocida del usuario sembrado.
const PASSWORD: &str = "testpassword123";
/// Ruta bajo prueba.
const HORARIO_PATH: &str = "/dashboard/horario";
/// Cookie con formato válido pero sin fila en `sessions` (fail-closed).
const COOKIE_SIN_SESION: &str =
    "session_id=0000000000000000000000000000000000000000000000000000000000000000";

/// Configuración de test: sin API key, auth activada, MCP inalcanzable
/// (no-localhost ⇒ sin cliente Mikareno ⇒ página sin I/O).
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18093,
        session_secret: "schedule_ui_test_secret_at_least_32_chars!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: None,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://192.0.2.10:8080/mcp".to_string(),
        genoffice_mcp_url: "http://192.0.2.10:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        registration_token: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
    }
}

/// Estado de la app de test + los temporales que lo sostienen.
struct TestApp {
    state: Arc<AppState>,
    // Mantiene vivos el directorio de notas y el fichero de la DB.
    _keep: (tempfile::TempDir, tempfile::NamedTempFile),
}

/// Levanta la app: migraciones reales, usuario `byron` sembrado, sin cliente
/// Mikareno compartido.
async fn setup_app() -> TestApp {
    let notes_tmp = tempfile::tempdir().expect("temp dir de notas");
    let db_tmp = tempfile::NamedTempFile::new().expect("fichero de BD");
    let config = test_config(notes_tmp.path().to_path_buf(), db_tmp.path().to_path_buf());

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

    // Semilla de `byron` con contraseña conocida (mismo formato que el registro).
    let salt = SaltString::generate(rand::thread_rng());
    let password_hash = argon2::Argon2::default()
        .hash_password(PASSWORD.as_bytes(), &salt)
        .expect("hashear contraseña")
        .to_string();
    sqlx::query(
        "INSERT INTO users (username, password_hash) VALUES (?, ?) \
         ON CONFLICT(username) DO UPDATE SET password_hash = EXCLUDED.password_hash",
    )
    .bind(USER)
    .bind(&password_hash)
    .execute(&pool)
    .await
    .expect("sembrar usuario byron");

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
        _keep: (notes_tmp, db_tmp),
    }
}

/// Login real: `POST /login` como `byron` → cookie `session_id=…` (sin
/// atributos) lista para reenviarla.
async fn login_byron(app: axum::Router) -> String {
    let form = format!("username={}&password={}", USER, PASSWORD);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .expect("construir request de login"),
        )
        .await
        .expect("respuesta de login");
    assert_eq!(resp.status(), StatusCode::FOUND, "el login debe redirigir");
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("el login debe emitir Set-Cookie");
    set_cookie.split(';').next().unwrap_or_default().to_string()
}

/// GET contra el router real, opcionalmente con cookie de sesión.
async fn get(app: axum::Router, uri: &str, cookie: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder().uri(uri);
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    app.oneshot(builder.body(Body::empty()).expect("construir request"))
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

// --- 1. Sin autenticación: el middleware rechaza la página ---

#[tokio::test]
async fn horario_sin_sesion_lo_rechaza_el_middleware() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    // Sin cookie: rechazo por middleware (página ⇒ 302 a /login).
    let resp = get(app.clone(), HORARIO_PATH, None).await;
    let status = resp.status();
    assert!(
        status == StatusCode::FOUND || status == StatusCode::UNAUTHORIZED,
        "sin sesión la página debe rechazarse (302/401), llegó {status}"
    );
    if status == StatusCode::FOUND {
        assert_eq!(
            resp.headers().get("location").and_then(|v| v.to_str().ok()),
            Some("/login"),
            "el rechazo por middleware debe mandar a /login"
        );
    }

    // Cookie con formato válido pero sin sesión en la DB: fail-closed.
    let resp = get(app, HORARIO_PATH, Some(COOKIE_SIN_SESION)).await;
    let status = resp.status();
    assert!(
        status == StatusCode::FOUND || status == StatusCode::UNAUTHORIZED,
        "una sesión inexistente no debe abrir la página, llegó {status}"
    );
}

// --- 2. Con sesión: 200 y las 7 columnas de la semana ---

#[tokio::test]
async fn horario_con_sesion_muestra_las_siete_columnas() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());
    let cookie = login_byron(app.clone()).await;

    let resp = get(app, HORARIO_PATH, Some(&cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_text(resp).await;

    // Grilla semanal: 7 columnas (lunes → domingo).
    assert!(
        html.contains("grid-cols-7"),
        "la página debe renderizar la grilla de 7 columnas"
    );
    for dia in [
        "Lunes",
        "Martes",
        "Miércoles",
        "Jueves",
        "Viernes",
        "Sábado",
        "Domingo",
    ] {
        assert!(html.contains(dia), "falta la columna {dia} en la grilla");
    }

    // Sin Mikareno no hay clases: estado honesto "Sin clases" (nunca inventado).
    assert!(
        html.contains("Sin clases"),
        "los días sin datos deben mostrar el estado honesto 'Sin clases'"
    );
}

// --- 3. Mikareno ausente: la página degrada sin 5xx y sin inventar datos ---

#[tokio::test]
async fn horario_sin_mikareno_degrada_con_estado_honesto() {
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());
    let cookie = login_byron(app.clone()).await;

    let resp = get(app, HORARIO_PATH, Some(&cookie)).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "sin Mikareno la página no debe fallar con 5xx"
    );
    let html = body_text(resp).await;

    // Aserto flexible: el handler propaga el banner "Mikareno no conectado"
    // (o, en su defecto, el estado "Sin clases"); lo que no puede ser es un
    // error o un falso "conectado".
    assert!(
        html.contains("Mikareno no conectado") || html.contains("Sin clases"),
        "debe mostrarse un estado honesto (banner de Mikareno o 'Sin clases')"
    );
    assert!(
        !html.contains("Conectado al MCP de Mikareno"),
        "sin Mikareno la página no debe anunciar conexión"
    );
}
