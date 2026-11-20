//! Pruebas de integración de la vista mensual y del planner por día.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::sqlite::SqlitePoolOptions;
use study_platform::{AppState, Config};
use tower::ServiceExt;

async fn setup() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("calendar.db");
    std::fs::write(&db_path, b"").unwrap();
    let notes_dir = dir.path().join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    let db_url = format!("sqlite://{}", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();

    let state = Arc::new(AppState {
        cache_mikareno: Arc::new(study_platform::academic::calendar_data::CacheMikareno::default()),
        cache_catalogo: Arc::new(study_platform::ai::modelos::CacheCatalogo::default()),
        config: Config {
            db_max_connections: 1,
            mcp_timeout_ms: 5_000,
            host: "127.0.0.1".to_string(),
            port: 18090,
            session_secret: "test_secret_key_at_least_32_chars_long!!".to_string(),
            db_path,
            notes_dir,
            openrouter_api_key: None,
            openrouter_model: "cohere/north-mini-code:free".to_string(),
            openrouter_max_tokens: 128,
            openrouter_max_cost_usd: 1.0,
            mikareno_mcp_url: "http://127.0.0.1:8080/mcp".to_string(),
            genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
            custom_mcp_api_key: None,
            registration_token: None,
            rust_log: "test".to_string(),
            auth_disabled: true,
        },
        db: pool,
        mikareno: None,
        start_time: std::time::Instant::now(),
    });
    (study_platform::handlers::router(state), dir)
}

fn json_request(method: &str, uri: &str, value: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn planner_entries_create_update_move_and_delete() {
    let (app, _dir) = setup().await;
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/calendar/annotations",
            serde_json::json!({
                "date": "2026-10-19",
                "title": "Repasar cálculo",
                "body": "Preparar los ejercicios 4 y 5",
                "category": "academic"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = response_json(response).await;
    let id = created["annotation"]["id"].as_i64().unwrap();
    assert_eq!(created["annotation"]["category"], "academic");

    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/calendar/annotations/{id}"),
            serde_json::json!({
                "date": "2026-10-20",
                "title": "Repasar cálculo y álgebra",
                "body": "Revisar apuntes",
                "category": "health"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let updated = response_json(response).await;
    assert_eq!(updated["annotation"]["date"], "2026-10-20");
    assert_eq!(updated["annotation"]["category"], "health");
    assert_eq!(updated["annotation"]["title"], "Repasar cálculo y álgebra");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/calendar/annotations?from=2026-10-20&to=2026-10-20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let listed = response_json(response).await;
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["annotations"][0]["id"], id);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/calendar/annotations/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn planner_rejects_invalid_date_and_category() {
    let (app, _dir) = setup().await;
    for body in [
        serde_json::json!({ "date": "2026-02-30", "title": "Inválido" }),
        serde_json::json!({ "date": "2026-10-19", "title": "Inválido", "category": "unknown" }),
    ] {
        let response = app
            .clone()
            .oneshot(json_request("POST", "/api/calendar/annotations", body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn monthly_calendar_renders_the_selected_day_panel_and_adjacent_dates() {
    let (app, _dir) = setup().await;
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/calendar/annotations",
            serde_json::json!({
                "date": "2026-10-19",
                "title": "Proyecto NOVA",
                "body": "Entregar borrador",
                "category": "work"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard/calendar?month=2026-10&date=2026-10-19")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(html.contains("id=\"selected-day-panel\" data-date=\"2026-10-19\""));
    assert!(html.contains("Proyecto NOVA"));
    assert!(html.contains("data-date=\"2026-09-28\""));
    assert!(html.contains("role=\"gridcell\""));
    assert!(html.contains("calendar-day--selected"));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/calendar?partial=day&date=2026-10-19")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let panel = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(panel.contains("Proyecto NOVA"));
    assert!(panel.contains("agenda-entry-category--work"));
}

#[tokio::test]
async fn ics_import_is_atomic_and_multiday_events_render_on_each_date() {
    let (app, _dir) = setup().await;
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/calendar/annotations/import",
            serde_json::json!({
                "annotations": [
                    {
                        "date": "2026-10-19",
                        "title": "Retiro de estudio",
                        "body": "Material en la biblioteca",
                        "category": "personal",
                        "kind": "event",
                        "end_date": "2026-10-22",
                        "all_day": true,
                        "location": "Quito",
                        "uid": "retiro@example.test"
                    }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let imported = response_json(response).await;
    assert_eq!(imported["imported"], 1);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/calendar/annotations?from=2026-10-20&to=2026-10-20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let listed = response_json(response).await;
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["annotations"][0]["uid"], "retiro@example.test");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard/calendar?partial=day&date=2026-10-20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let panel = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(panel.contains("Eventos importados"));
    assert!(panel.contains("Retiro de estudio"));
    assert!(panel.contains("data-calendar-delete="));

    // La validación del lote precede a la transacción: el registro válido
    // anterior permanece, pero el nuevo lote inválido no añade nada.
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/calendar/annotations/import",
            serde_json::json!({
                "annotations": [
                    {
                        "date": "2026-10-23",
                        "title": "No guardar",
                        "kind": "event",
                        "end_date": "2026-10-23",
                        "all_day": true
                    }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/calendar/annotations?from=2026-10-23&to=2026-10-23")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let listed = response_json(response).await;
    assert_eq!(listed["count"], 0);
}

#[tokio::test]
async fn ics_weekly_recurrence_projects_only_matching_weekdays() {
    let (app, _dir) = setup().await;
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/calendar/annotations/import",
            serde_json::json!({
                "annotations": [
                    {
                        "date": "2026-10-21",
                        "title": "Tutoría semanal",
                        "body": "Repaso de álgebra",
                        "category": "academic",
                        "kind": "event",
                        "end_date": "2026-10-21",
                        "all_day": false,
                        "start_time": "09:00",
                        "end_time": "10:00",
                        "location": "Quito",
                        "uid": "tutoria@example.test",
                        "recurrence": "weekly",
                        "weekly_day": 1
                    },
                    {
                        "date": "2026-10-21",
                        "title": "Tutoría semanal",
                        "body": "Repaso de álgebra",
                        "category": "academic",
                        "kind": "event",
                        "end_date": "2026-10-21",
                        "all_day": false,
                        "start_time": "09:00",
                        "end_time": "10:00",
                        "location": "Quito",
                        "uid": "tutoria@example.test",
                        "recurrence": "weekly",
                        "weekly_day": 3
                    }
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let imported = response_json(response).await;
    assert_eq!(imported["imported"], 2);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard/calendar?partial=day&date=2026-10-26")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let monday = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(monday.contains("Tutoría semanal"));
    assert!(monday.contains("Cada semana"));
    assert!(monday.contains("09:00–10:00"));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/calendar?partial=day&date=2026-10-22")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let thursday = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!thursday.contains("Tutoría semanal"));
}

#[tokio::test]
async fn weekly_import_rejects_invalid_recurrence_metadata() {
    let (app, _dir) = setup().await;
    let base = serde_json::json!({
        "date": "2026-10-21",
        "title": "Tutoría semanal",
        "kind": "event",
        "end_date": "2026-10-21",
        "all_day": false,
        "start_time": "09:00",
        "end_time": "10:00",
        "recurrence": "weekly"
    });
    let mut out_of_range = base.clone();
    out_of_range["weekly_day"] = serde_json::json!(7);
    let mut unsupported = base.clone();
    unsupported["weekly_day"] = serde_json::json!(3);
    unsupported["recurrence"] = serde_json::json!("daily");
    let invalid = [base, out_of_range, unsupported];
    for annotation in invalid {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/calendar/annotations/import",
                serde_json::json!({ "annotations": [annotation] }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
