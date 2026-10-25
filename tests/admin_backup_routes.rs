//! Integration tests for the unified backup routes (`Agent: subagent tests`).
//!
//! Contract under test (router as wired in `src/handlers/mod.rs`):
//! - `POST /api/admin/backup`            → 200 `{success, name, bytes}` (sistema unificado).
//! - `GET  /api/admin/backup`            → 200 (ruta legacy; delega en `backup_now`).
//! - `POST /api/admin/backup/restore`    → 400 con `{error}` para nombres letales,
//!                                         404 `{error}` si el backup no existe.
//! - `GET  /api/admin/backup/download/{name}` → 200 + `Content-Disposition: attachment`
//!                                         para un backup real; 400/404 para rutas.
//! - Sin sesión → 401 en `/api/*`.
//!
//! Harness idéntico al de `tests/admin_routes.rs`: SQLite temporal con
//! migraciones reales, usuario `admin` sembrado y sesión válida en BD
//! (`SHA256(secret:token)`). Nada sale del temp dir salvo los ficheros de
//! backup que las propias rutas escriben en `data/backups/` (ruta *relativa*,
//! igual que `crate::backup::DEFAULT_DIR`): el test los borra al terminar.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use password_hash::PasswordHasher;
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::middleware::auth::{generate_session_token, hash_session_token};
use study_platform::AppState;

/// Usuario administrador sembrado para las peticiones autenticadas.
const ADMIN_USER: &str = "admin";

/// Directorio *relativo* de backups: el mismo que usa el servidor
/// (`crate::backup::DEFAULT_DIR`), resuelto contra el CWD del proceso — por eso
/// el test usa la misma ruta relativa y no una absoluta.
const BACKUPS_DIR: &str = study_platform::backup::DEFAULT_DIR;

/// Backup simulado para la prueba de descarga.
const DUMMY_NAME: &str = "app-20260101-000000.db";

/// Configuración de test: presupuesto fijo, sin API key (modo mock), auth activa.
fn test_config(notes_dir: PathBuf, db_path: PathBuf) -> Config {
    Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18093,
        session_secret: "admin_backup_routes_test_secret_32chars!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: None,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://127.0.0.1:57777/mcp".to_string(), // puerto muerto: probe hermético
        genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        registration_token: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
    }
}

/// Estado de la app de test + cookie de sesión + temporales vivos.
struct TestApp {
    state: Arc<AppState>,
    cookie: String,
    // Mantiene vivos el temp dir de notas y el fichero de BD temporal.
    _keep: (tempfile::TempDir, tempfile::NamedTempFile),
}

/// Prepara el estado completo: migraciones reales, admin sembrado y sesión en BD.
async fn setup_admin_app() -> TestApp {
    let notes_tmp = tempfile::tempdir().unwrap();
    let notes_dir = notes_tmp.path().to_path_buf();
    let db_tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = db_tmp.path().to_path_buf();
    let db_url = format!("sqlite://{}", db_path.display());

    let config = test_config(notes_dir, db_path);
    let secret = config.session_secret.clone();

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

    // Admin con contraseña conocida (mismo patrón que integration.rs).
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
    .bind(ADMIN_USER)
    .bind(&password_hash)
    .execute(&pool)
    .await
    .expect("Failed to seed admin user");
    let uid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(ADMIN_USER)
        .fetch_one(&pool)
        .await
        .expect("admin user id");

    // Sesión en BD: la cookie lleva el token en claro, la BD guarda su hash.
    let token = generate_session_token();
    let hashed = hash_session_token(&token, &secret);
    let expires = chrono::Utc::now() + chrono::Duration::hours(24);
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(&hashed)
        .bind(uid)
        .bind(expires.to_rfc3339())
        .execute(&pool)
        .await
        .expect("Failed to seed admin session");

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
    app.oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// POST JSON a través del router real (el extractor `Json` exige el header).
async fn post_json(
    app: axum::Router,
    uri: &str,
    body: serde_json::Value,
    cookie: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json");
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    app.oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

/// POST sin cuerpo (para la ruta que no lee payload).
async fn post_empty(
    app: axum::Router,
    uri: &str,
    cookie: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder().uri(uri).method("POST");
    if let Some(c) = cookie {
        builder = builder.header("cookie", c);
    }
    app.oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

/// Cuerpo como JSON; `Null` si no es JSON válido (p. ej. cuerpo de 401).
async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(resp).await).unwrap_or(serde_json::Value::Null)
}

/// Borra los ficheros de backup creados por el test (copia + sidecar + WAL).
fn limpiar_backups(nombres: &[String]) {
    let dir = Path::new(BACKUPS_DIR);
    for nombre in nombres {
        for sufijo in ["", ".sha256", "-wal", "-shm"] {
            let _ = std::fs::remove_file(dir.join(format!("{nombre}{sufijo}")));
        }
    }
}

// --- 1, 2 y 5. Crear backup (POST/GET) y rechazo sin sesión ---

#[tokio::test]
async fn backup_now_crea_backup_y_exige_sesion() {
    let t = setup_admin_app().await;
    let app = study_platform::handlers::router(t.state.clone());
    let cookie = t.cookie.clone();
    let mut creados: Vec<String> = Vec::new();

    // POST /api/admin/backup con sesión de admin → 200 con el payload del
    // sistema unificado: `{success, name, bytes}`.
    let resp = post_empty(app.clone(), "/api/admin/backup", Some(&cookie)).await;
    let status = resp.status();
    let json = body_json(resp).await;
    assert_eq!(status, StatusCode::OK, "respuesta del backup: {json}");
    assert_eq!(json["success"], serde_json::json!(true));
    let nombre = json["name"]
        .as_str()
        .expect("el backup debe devolver el nombre del fichero")
        .to_string();
    assert!(
        nombre.starts_with("app-") && nombre.ends_with(".db") && !nombre.is_empty(),
        "nombre de backup inesperado: {nombre}"
    );
    assert!(
        json["bytes"].as_u64().unwrap_or(0) > 0,
        "el backup debe tener tamaño: {json}"
    );
    creados.push(nombre);

    // GET /api/admin/backup (ruta legacy) → 200; ya delega en `backup_now`,
    // así que también devuelve `name`.
    let resp = get(app.clone(), "/api/admin/backup", Some(&cookie)).await;
    let status = resp.status();
    let json = body_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "respuesta del backup legacy: {json}"
    );
    assert_eq!(json["success"], serde_json::json!(true));
    let nombre_get = json["name"]
        .as_str()
        .expect("el GET legacy también debe devolver el nombre")
        .to_string();
    assert!(
        nombre_get.starts_with("app-") && nombre_get.ends_with(".db"),
        "nombre de backup inesperado en GET: {nombre_get}"
    );
    creados.push(nombre_get);

    // Sin sesión → 401 (política de `/api/*`).
    let resp = post_empty(app.clone(), "/api/admin/backup", None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    limpiar_backups(&creados);
}

// --- 3. Restauración: nombres letales y backups inexistentes ---

#[tokio::test]
async fn restore_rechaza_nombres_letales_y_inexistentes() {
    let t = setup_admin_app().await;
    let app = study_platform::handlers::router(t.state.clone());
    let cookie = t.cookie.clone();

    // Ruta de traversal: nunca 500, siempre error de cliente con JSON.
    let resp = post_json(
        app.clone(),
        "/api/admin/backup/restore",
        serde_json::json!({ "name": "../etc/passwd" }),
        Some(&cookie),
    )
    .await;
    let status = resp.status();
    assert!(
        matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND
        ),
        "un nombre letal debe dar 400/401/404, nunca 5xx: {status}"
    );
    let json = body_json(resp).await;
    assert!(
        json["error"].as_str().is_some(),
        "la respuesta de error debe ser JSON con clave `error`: {json}"
    );

    // Nombre inexistente tal cual (no es un `app-*.db`): error de cliente
    // (hoy 400 por nombre no válido; 404 si el fichero no está).
    let resp = post_json(
        app.clone(),
        "/api/admin/backup/restore",
        serde_json::json!({ "name": "noexiste-20260101-000000.db" }),
        Some(&cookie),
    )
    .await;
    let status = resp.status();
    assert!(
        matches!(status, StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND),
        "un backup inexistente debe dar 400/404, nunca 5xx: {status}"
    );
    let json = body_json(resp).await;
    assert!(
        json["error"].as_str().is_some(),
        "la respuesta de error debe ser JSON con clave `error`: {json}"
    );

    // Nombre con forma válida pero sin fichero → 404 (no existe el backup).
    let resp = post_json(
        app.clone(),
        "/api/admin/backup/restore",
        serde_json::json!({ "name": "app-noexiste-20260101-000000.db" }),
        Some(&cookie),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let json = body_json(resp).await;
    assert!(
        json["error"].as_str().is_some(),
        "la respuesta 404 debe ser JSON con clave `error`: {json}"
    );
}

// --- 4. Descarga: traversal rechazado y backup real servido ---

#[tokio::test]
async fn download_sirve_backup_real_y_rechaza_traversal() {
    let t = setup_admin_app().await;
    let app = study_platform::handlers::router(t.state.clone());
    let cookie = t.cookie.clone();

    // El servidor resuelve `data/backups` contra su CWD; el test usa la misma
    // ruta relativa, así que el fichero Dummy queda visible para el handler.
    let dir = Path::new(BACKUPS_DIR);
    let dir_ya_existia = dir.exists();
    std::fs::create_dir_all(dir).expect("no se pudo crear data/backups");
    let ruta_dummy = dir.join(DUMMY_NAME);

    // Si el dummy ya existe (otra corrida), se reutiliza su contenido; si no,
    // se crea y se borra al final.
    let (contenido, creado_por_test) = if ruta_dummy.exists() {
        (
            std::fs::read(&ruta_dummy).expect("no se pudo leer el dummy"),
            false,
        )
    } else {
        let bytes = b"dummy-backup-bytes".to_vec();
        std::fs::write(&ruta_dummy, &bytes).expect("no se pudo escribir el dummy");
        (bytes, true)
    };

    // Traversal en la URL (`%2F` decodifica a `/`): 400/404, nunca 5xx.
    let resp = get(
        app.clone(),
        "/api/admin/backup/download/..%2F",
        Some(&cookie),
    )
    .await;
    let status = resp.status();
    assert!(
        matches!(status, StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND),
        "una ruta de traversal debe dar 400/404, nunca 5xx: {status}"
    );
    let json = body_json(resp).await;
    assert!(
        json["error"].as_str().is_some(),
        "la respuesta de error debe ser JSON con clave `error`: {json}"
    );

    // Backup real → 200 con `Content-Disposition: attachment`.
    let resp = get(
        app.clone(),
        &format!("/api/admin/backup/download/{DUMMY_NAME}"),
        Some(&cookie),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let cabeceras = resp.headers().clone();
    let disposicion = cabeceras
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        disposicion.contains("attachment") && disposicion.contains(DUMMY_NAME),
        "cabecera Content-Disposition inesperada: {disposicion}"
    );
    let cuerpo = body_bytes(resp).await;
    assert_eq!(
        cuerpo, contenido,
        "el cuerpo descargado debe ser el fichero de backup"
    );

    // Limpieza: el dummy (si lo creamos) y el directorio (si lo creamos y
    // ha quedado vacío).
    if creado_por_test {
        let _ = std::fs::remove_file(&ruta_dummy);
    }
    if !dir_ya_existia {
        let _ = std::fs::remove_dir(dir);
    }
}
