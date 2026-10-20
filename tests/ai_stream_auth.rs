//! Tests de autenticación y streaming de `POST /api/ai/chat/stream`.
//!
//! Contrato verificado (ruta real en `src/ai/mod.rs`, handler en
//! `src/ai/streaming.rs`):
//! - **Sin sesión** → `401` y *nunca* un stream SSE: ni cabecera
//!   `text/event-stream` ni frames `event:` / `data:` en el cuerpo.
//! - **Con sesión y sin API key** (modo simulado) → `200` con
//!   `content-type: text/event-stream` y un frame `done` que declara que el
//!   streaming no está disponible (sin contenido fabricado).
//! - **Cuatro streams simultáneos** (el tope global documentado en
//!   `src/middleware/rate_limit.rs`) → los cuatro se sirven: las primeras
//!   cuatro peticiones nunca reciben `429`.
//!
//! Harness copiado de `tests/admin_routes.rs`: SQLite en directorio temporal
//! con las migraciones reales, usuario `admin` sembrado con Argon2 y una
//! sesión en DB (`SHA256(secret:token)`) reutilizada tal cual en la cookie
//! `session_id`, igual que hace el navegador. `mikareno: None` y
//! `openrouter_api_key: None` (modo simulado) ⇒ ni cliente MCP ni proveedor
//! real: cero llamadas de red.
//!
//! Sobre el limitador: la ruta de streaming está exenta de la ventana por
//! minuto y aquí solo se emiten unas pocas peticiones (muy por debajo de
//! 60/min), de modo que ninguna aserción depende de esa ventana. Cada test
//! usa además una IP propia de TEST-NET-1 (`x-forwarded-for`) para que el
//! cubo general del limitador nunca se comparta entre tests. El único
//! comportamiento del limitador del que se apoya el tercer test es el tope
//! global de concurrencia (`MAX_CONCURRENT_STREAMS = 4`).

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

/// Ruta bajo prueba.
const STREAM_PATH: &str = "/api/ai/chat/stream";

/// Usuario sembrado para las peticiones autenticadas.
const ADMIN_USER: &str = "admin";

/// Cuerpo JSON mínimo y válido (el `message` es obligatorio; con un cuerpo
/// inválido el 401 de auth no se distinguiría de un 400 del extractor).
const STREAM_BODY: &str = r#"{"message":"hola","subject":"general"}"#;

/// IPs de TEST-NET-1, una por test: aíslan el cubo general del limitador.
const IP_TEST_1: &str = "192.0.2.31";
const IP_TEST_2: &str = "192.0.2.32";
const IP_TEST_3: &str = "192.0.2.33";

/// Serializa los tests que tocan la ruta de streaming: el contador de
/// streams vivos es global al proceso, así que dos tests en paralelo
/// podrían hacer que el tercero topase con el límite. Mismo patrón que
/// `src/middleware/rate_limit.rs`.
static STREAM_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Configuración de test: sin API key (modo simulado), auth activada,
/// presupuesto $2.00 y MCP inalcanzable (URL no-localhost ⇒ sin cliente).
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18095,
        session_secret: "ai_stream_auth_test_secret_32chars!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: None,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://192.0.2.40:57777/mcp".to_string(), // no-localhost ⇒ sin cliente
        genoffice_mcp_url: "http://192.0.2.40:8453/mcp".to_string(),
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
    // Mantiene vivos el directorio de notas y el fichero de la BD.
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

/// POST a la ruta de streaming contra el router real, con o sin sesión.
async fn post_stream(
    app: axum::Router,
    cookie: Option<&str>,
    ip: &str,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(STREAM_PATH)
        .header("content-type", "application/json")
        .header("x-forwarded-for", ip);
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    app.oneshot(
        builder
            .body(Body::from(STREAM_BODY))
            .expect("construir request"),
    )
    .await
    .expect("respuesta del router")
}

/// `content-type` de la respuesta (cadena vacía si no viene).
fn content_type(resp: &axum::response::Response) -> String {
    resp.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// Cuerpo completo de la respuesta.
async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("leer cuerpo")
        .to_vec()
}

// --- 1. Sin sesión: 401 y ningún stream ---

/// Sin cookie de sesión la ruta SSE responde 401. El rechazo ocurre en la
/// capa de auth, antes del handler: el cuerpo no es un stream SSE (ni
/// cabecera `text/event-stream`, ni frames `event:`/`data:`).
#[tokio::test(flavor = "current_thread")]
async fn stream_route_without_session_returns_401_and_no_sse_frames() {
    let _guard = STREAM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = post_stream(app, None, IP_TEST_1).await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "la ruta de streaming exige sesión"
    );

    let ct = content_type(&resp);
    let body = body_bytes(resp).await;
    let text = String::from_utf8_lossy(&body).to_string();

    // El rechazo no abre un stream.
    assert!(
        !ct.starts_with("text/event-stream"),
        "un 401 no debe abrir un stream SSE (content-type={ct})"
    );
    assert!(
        !text.contains("event:"),
        "el cuerpo de un 401 no debe traer frames SSE: {text}"
    );
    assert!(
        !text.contains("data:"),
        "el cuerpo de un 401 no debe traer frames SSE: {text}"
    );

    // El rechazo es un error explícito. La capa de auth responde hoy texto
    // plano ("Unauthorized") en `/api/*`; la capa de IA responde
    // `{"error": "..."}`. Si el cuerpo es JSON, debe llevar clave "error".
    match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(json) => assert!(
            json.get("error").is_some(),
            "un rechazo JSON debe traer la clave \"error\": {text}"
        ),
        Err(_) => assert!(
            text.contains("Unauthorized") || text.contains("Invalid session"),
            "el cuerpo del 401 debe ser un error explícito, se obtuvo: {text}"
        ),
    }
}

// --- 2. Con sesión (modo simulado): stream SSE con frame `done` ---

/// Con sesión y sin API key la ruta responde un stream SSE cuyo frame
/// terminal es `done` y declara que el streaming no está disponible. Se
/// parsea el stream completo (no se asume la forma exacta de los frames
/// intermedios) y se comprueba que el aviso aparece en el texto y en el
/// payload JSON del frame `data:`.
#[tokio::test(flavor = "current_thread")]
async fn stream_route_with_session_returns_event_stream_with_mock_done_frame() {
    let _guard = STREAM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());

    let resp = post_stream(app, Some(t.cookie.as_str()), IP_TEST_2).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "con sesión y modo simulado el stream debe servirse"
    );

    let ct = content_type(&resp);
    assert!(
        ct.starts_with("text/event-stream"),
        "la ruta de streaming debe responder con text/event-stream, se obtuvo: {ct}"
    );

    // Stream completo.
    let body = body_bytes(resp).await;
    let raw = String::from_utf8_lossy(&body).to_string();

    assert!(
        raw.contains("event: done"),
        "el stream debe llevar un frame `done`: {raw}"
    );
    assert!(
        raw.contains("Modo simulado"),
        "el stream debe declarar el modo simulado: {raw}"
    );
    assert!(
        raw.contains("streaming no disponible"),
        "el stream debe declarar que el streaming no está disponible: {raw}"
    );

    // El payload del frame es JSON.
    let data = raw
        .lines()
        .find_map(|l| l.strip_prefix("data:"))
        .expect("el stream debe traer una línea data:");
    let payload: serde_json::Value = serde_json::from_str(data.trim())
        .unwrap_or_else(|e| panic!("el payload del frame debe ser JSON ({e}): {data}"));
    let content = payload["content"].as_str().unwrap_or_default();
    assert!(
        content.contains("Modo simulado"),
        "el payload del frame debe llevar el aviso de simulación: {payload}"
    );
}

// --- 3. Cuatro streams simultáneos: ninguno recibe 429 ---

/// Cuatro peticiones simultáneas (el tope global de streams concurrentes es
/// 4) con sesión y en modo simulado: las cuatro deben servirse. No se
/// prueba la quinta a propósito: el contador de streams es global al
/// proceso y otro binario de test en paralelo podría ocupar un hueco.
#[tokio::test(flavor = "current_thread")]
async fn four_simultaneous_streams_are_all_served_without_429() {
    let _guard = STREAM_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let t = setup_app().await;
    let app = study_platform::handlers::router(t.state.clone());
    let cookie = t.cookie.as_str();

    // Las cuatro peticiones están en vuelo a la vez: cada una toma su hueco
    // de stream antes de que la siguiente llegue al límite.
    let (r1, r2, r3, r4) = tokio::join!(
        post_stream(app.clone(), Some(cookie), IP_TEST_3),
        post_stream(app.clone(), Some(cookie), IP_TEST_3),
        post_stream(app.clone(), Some(cookie), IP_TEST_3),
        post_stream(app.clone(), Some(cookie), IP_TEST_3),
    );

    let responses = [r1, r2, r3, r4];
    for resp in &responses {
        let status = resp.status();
        assert_ne!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "las primeras cuatro peticiones simultáneas nunca deben recibir 429"
        );
        assert_eq!(
            status,
            StatusCode::OK,
            "cada una de las cuatro peticiones simultáneas debe servirse"
        );
        let ct = content_type(resp);
        assert!(
            ct.starts_with("text/event-stream"),
            "cada stream simultáneo debe responder con text/event-stream, se obtuvo: {ct}"
        );
    }
    // Al soltar los cuerpos se liberan los huecos de stream.
    drop(responses);
}
