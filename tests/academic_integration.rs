//! Integration tests for the academic dashboard.
//! Tests route authentication, HTML rendering, and MCP fallback behavior.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::AppState;

fn test_config(db_path: PathBuf, notes_dir: PathBuf) -> Config {
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
        openrouter_max_tokens: 100,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://127.0.0.1:8080/mcp".to_string(),
        genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        registration_token: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
    }
}

async fn setup_test_state() -> Arc<AppState> {
    let temp_db = tempfile::NamedTempFile::new().unwrap();
    let notes_dir = tempfile::tempdir().unwrap().keep();
    let config = test_config(temp_db.path().to_path_buf(), notes_dir);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(temp_db.path().to_str().unwrap())
        .await
        .unwrap();

    sqlx::query("PRAGMA journal_mode=WAL;")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("PRAGMA foreign_keys=ON;")
        .execute(&pool)
        .await
        .ok();
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS notes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            slug TEXT NOT NULL UNIQUE,
            filename TEXT NOT NULL,
            tags TEXT NOT NULL DEFAULT '',
            word_count INTEGER NOT NULL DEFAULT 0,
            summary TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .execute(&pool)
    .await
    .unwrap();

    Arc::new(AppState { config, db: pool,
        mikareno: None,
        start_time: std::time::Instant::now(),
    })
}

#[tokio::test]
async fn test_dashboard_requires_auth() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

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
async fn test_calendar_requires_auth() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/calendar")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FOUND);
}

#[tokio::test]
async fn test_horario_requires_auth() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/horario")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FOUND);
}

#[tokio::test]
async fn test_planner_requires_auth() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/planner")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FOUND);
}

#[tokio::test]
async fn test_html_escape() {
    let escaped = study_platform::academic::templates::html_escape("<script>alert('xss')</script>");
    assert!(!escaped.contains("<script>"));
    assert!(escaped.contains("&lt;script&gt;"));
}

#[tokio::test]
async fn test_head_includes_viewport() {
    let head = study_platform::academic::templates::head("Test Page");
    assert!(head.contains("viewport"));
    assert!(head.contains("htmx"));
    assert!(head.contains("tailwindcss"));
}

#[tokio::test]
async fn test_page_has_nav() {
    let page = study_platform::academic::templates::page("Dashboard", "<p>content</p>");
    assert!(page.contains("<nav"));
    assert!(page.contains("/dashboard"));
    assert!(page.contains("/dashboard/calendar"));
    assert!(page.contains("/dashboard/horario"));
    assert!(page.contains("/dashboard/planner"));
}

#[tokio::test]
async fn test_card_list_renders_items() {
    let items = vec!["Item 1".to_string(), "Item 2".to_string()];
    let html = study_platform::academic::templates::card_list("Test List", &items);
    assert!(html.contains("Item 1"));
    assert!(html.contains("Item 2"));
}

#[tokio::test]
async fn test_card_list_empty() {
    let html = study_platform::academic::templates::card_list("Empty List", &vec![]);
    assert!(html.contains("No hay elementos"));
}

#[tokio::test]
async fn test_horario_html_with_data() {
    let dias = vec![study_platform::integrations::mikareno::HorarioDia {
        dia: "Lunes".to_string(),
        clases: vec!["Matemáticas".to_string(), "Física".to_string()],
    }];
    let html = study_platform::academic::templates::horario_html(&dias);
    assert!(html.contains("Lunes"));
    assert!(html.contains("Matemáticas"));
}

#[tokio::test]
async fn test_horario_html_empty() {
    let html = study_platform::academic::templates::horario_html(&[]);
    assert!(html.contains("No hay horario"));
}

#[tokio::test]
async fn test_mock_dashboard_data() {
    let html = study_platform::academic::templates::mock_dashboard_data();
    assert!(html.contains("card"));
    assert!(html.contains("text-primary-container"));
}

#[tokio::test]
async fn test_mikareno_client_localhost() {
    let state = setup_test_state().await;
    let client = study_platform::academic::mikareno_client(&state);
    assert!(client.is_some());
    assert_eq!(client.unwrap().is_connected(), false);
}

#[tokio::test]
async fn test_mikareno_client_not_localhost() {
    let temp_db = tempfile::NamedTempFile::new().unwrap();
    let notes_dir = tempfile::tempdir().unwrap().keep();
    let config = test_config(temp_db.path().to_path_buf(), notes_dir);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(temp_db.path().to_str().unwrap())
        .await
        .unwrap();

    let state = Arc::new(AppState {
        config: Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
            mikareno_mcp_url: "http://remote.example.com/mcp".to_string(),
            ..config
        },
        db: pool,
        mikareno: None,
        start_time: std::time::Instant::now(),
    });

    let client = study_platform::academic::mikareno_client(&state);
    assert!(client.is_none());
}

// --- New route auth tests ---

#[tokio::test]
async fn test_subjects_requires_auth() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/subjects")
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
async fn test_profile_requires_auth() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/profile")
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
async fn test_daily_calendar_requires_auth() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/calendar/daily")
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

// --- Template function tests ---

#[tokio::test]
async fn test_subject_grid_html_with_data() {
    let subjects = vec![
        study_platform::academic::templates::SubjectSummary {
            name: "Matematicas".to_string(),
            average: 8.5,
            count: 2,
            codigo: "M1".to_string(),
        },
        study_platform::academic::templates::SubjectSummary {
            name: "Lenguaje".to_string(),
            average: 6.0,
            count: 1,
            codigo: "L1".to_string(),
        },
    ];
    let html = study_platform::academic::templates::subject_grid_html(&subjects);
    assert!(html.contains("Matematicas"));
    assert!(html.contains("Lenguaje"));
    assert!(html.contains("8.5"));
    assert!(html.contains("6.0"));
    assert!(html.contains("text-green-400"));
    assert!(html.contains("text-yellow-400"));
    assert!(html.contains("/dashboard/subjects/M1"));
    assert!(html.contains("/dashboard/subjects/L1"));
}

#[tokio::test]
async fn test_subject_grid_html_empty() {
    let html = study_platform::academic::templates::subject_grid_html(&[]);
    assert!(html.contains("No hay asignaturas"));
    assert!(html.contains("Reintentar"));
}

#[tokio::test]
async fn test_event_timeline_html_with_data() {
    let events = vec![
        study_platform::integrations::mikareno::CalendarioEvent {
            fecha: "2024-03-15".to_string(),
            evento: "Examen Final".to_string(),
            tipo: "SCHOOL_DAY".to_string(),
        },
        study_platform::integrations::mikareno::CalendarioEvent {
            fecha: "2024-03-16".to_string(),
            evento: "Entrega proyecto".to_string(),
            tipo: "EVENTO".to_string(),
        },
    ];
    let html =
        study_platform::academic::templates::event_timeline_html(&events, Some("2024-03-15"));
    assert!(html.contains("Examen Final"));
    assert!(html.contains("Entrega proyecto"));
    assert!(html.contains("HOY"));
    // Only one event is "today"
    assert_eq!(html.matches("HOY").count(), 1);
}

#[tokio::test]
async fn test_event_timeline_html_empty() {
    let html = study_platform::academic::templates::event_timeline_html(&[], None);
    assert!(html.contains("No hay eventos"));
    assert!(html.contains("Reintentar"));
}

#[tokio::test]
async fn test_subject_grid_html_links_to_subjects() {
    let subjects = vec![study_platform::academic::templates::SubjectSummary {
        name: "Test".to_string(),
        average: 7.0,
        count: 1,
        codigo: String::new(),
    }];
    let html = study_platform::academic::templates::subject_grid_html(&subjects);
    // When codigo is empty, the card falls back to URL-encoded name.
    assert!(html.contains("/dashboard/subjects/"));
}

#[tokio::test]
async fn test_empty_state_html_has_htmx_reload() {
    let html = study_platform::academic::templates::empty_state_html("Missing", "/retry/path");
    assert!(html.contains("Missing"));
    assert!(html.contains("Reintentar"));
    assert!(html.contains("/retry/path"));
    assert!(html.contains("hx-get"));
    assert!(html.contains("hx-target"));
    assert!(html.contains("hx-swap"));
}

#[tokio::test]
async fn test_profile_card_html() {
    let perfil = study_platform::integrations::mikareno::Perfil {
        nombre: "Test Student".to_string(),
        institucion: "Test School".to_string(),
        curso: "1 A".to_string(),
    };
    let html = study_platform::academic::templates::profile_card_html(&perfil);
    assert!(html.contains("Test Student"));
    assert!(html.contains("Test School"));
    assert!(html.contains("1 A"));
    assert!(html.contains("partial=profile"));
}

#[tokio::test]
async fn test_stats_grid_html_renders_counts() {
    let html = study_platform::academic::templates::stats_grid_html(10, 5, 3, 7);
    assert!(html.contains("10")); // note count
    assert!(html.contains("5")); // subjects
    assert!(html.contains("3")); // tareas
    assert!(html.contains("7")); // events
    assert!(html.contains("partial=stats"));
}

#[tokio::test]
async fn test_mcp_status_banner_disconnected_has_mock_label() {
    let html = study_platform::academic::templates::mcp_status_banner(false);
    assert!(html.contains("no conectado"));
    // Honest policy: el banner ya no promete datos de ejemplo; dice qué falta.
    assert!(html.contains("sin datos de Mikareno"));
}

#[tokio::test]
async fn test_mock_subjects_are_honestly_empty() {
    // Honest policy: sin MCP no se inventan asignaturas; la UI muestra
    // estado vacío en vez de datos de ejemplo.
    let subjects = study_platform::academic::templates::mock_subjects();
    assert!(subjects.is_empty());
}

#[tokio::test]
async fn test_subjects_from_notas_groups_by_materia() {
    let notas = vec![
        study_platform::integrations::mikareno::Nota {
            materia: "Math".to_string(),
            nota: 8.0,
            trimestre: 1,
            codigo: "M1".to_string(),
        },
        study_platform::integrations::mikareno::Nota {
            materia: "Math".to_string(),
            nota: 10.0,
            trimestre: 2,
            codigo: "M1".to_string(),
        },
        study_platform::integrations::mikareno::Nota {
            materia: "Science".to_string(),
            nota: 7.0,
            trimestre: 1,
            codigo: "S1".to_string(),
        },
    ];
    let subjects = study_platform::academic::templates::subjects_from_notas(&notas);
    assert_eq!(subjects.len(), 2);
    let math = subjects.iter().find(|s| s.name == "Math").unwrap();
    assert!((math.average - 9.0).abs() < 1e-9);
    assert_eq!(math.count, 2);
}

#[tokio::test]
async fn test_dashboard_partial_stats() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state);

    // Unauthenticated → 302 redirect (partial doesn't bypass auth)
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard?partial=stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FOUND);
}

/// Ignored when a live Mikareno MCP server is running: the endpoint tries to
/// connect to the real MCP so "mock mode" (and the "HOY" badge) never
/// triggers in this environment.
#[tokio::test]
#[ignore = "requires Mikareno MCP server to be stopped"]
async fn test_daily_calendar_renders_in_mock_mode() {
    let state = setup_test_state().await;
    let app = study_platform::handlers::router(state.clone());

    // Valid-looking UUID session — but require_auth_state looks it up in DB,
    // so we must seed matching rows first (tables don't exist in this fixture).
    let session_id = "550e8400-e29b-41d4-a716-446655440000";

    // Ignored when a live Mikareno MCP server is running (it would prevent
    // mock mode; see comment on the test itself).
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )"#,
    )
    .execute(&state.db)
    .await
    .unwrap();
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            expires_at TEXT NOT NULL,
            user_agent TEXT
        )"#,
    )
    .execute(&state.db)
    .await
    .unwrap();

    sqlx::query("INSERT INTO users (username, password_hash) VALUES (?, ?)")
        .bind("fictitious")
        .bind("$argon2id$v=19$m=19456,t=2,p=1$placeholder")
        .execute(&state.db)
        .await
        .unwrap();
    let next_id: i64 = sqlx::query_scalar("SELECT last_insert_rowid()")
        .fetch_one(&state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)")
        .bind(session_id)
        .bind(next_id)
        .bind((chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();

    let cookie = format!("session_id={session_id}");

    // Daily calendar with mock data (no MCP server running)
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/calendar/daily")
                .header("cookie", cookie)
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
    assert!(html.contains("Calendario Diario"));
    assert!(html.contains("Eventos de hoy"));
    assert!(html.contains("HOY"));
}
