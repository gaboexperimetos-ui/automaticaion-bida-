//! OpenRouter streaming tests (`Agent: subagent Rust streaming`).
//!
//! NO real requests are ever made to OpenRouter. The local OpenRouter mock
//! server (below, an axum app on an ephemeral `127.0.0.1` port) doubles as
//! the provider: it serves both the JSON chat-completions shape and the
//! OpenAI/OpenRouter-compatible SSE encoding (`data: {delta...}` chunks and
//! a final `data: [DONE]` sentinel).
//!
//! Why the mock exists: the provider client hardcodes
//! `OPENROUTER_URL = "https://openrouter.ai/api/v1/chat/completions"`
//! (`src/ai/openrouter.rs`) with no injection point, so pointing the app at
//! this server is not possible without editing `src/**` (out of scope). The
//! mock's own HTTP contract is therefore asserted directly at the wire level,
//! and the app-level streaming route is covered by an `#[ignore]`d test that
//! only uses mock mode (no API key → the client can never reach the real
//! provider even if it is ever unignored early).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::AppState;

/// Text the mock streams, split across SSE delta chunks.
const STREAM_FULL_TEXT: &str = "La fotosintesis convierte la luz en energia quimica.";

/// Authorization headers seen by the mock (prove the bearer key was sent).
type SeenAuth = Arc<Mutex<Vec<String>>>;

/// Handcrafted OpenRouter `chat/completions` response for the non-stream route.
fn openrouter_completion_body(model: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-mock-001",
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": STREAM_FULL_TEXT },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 12,
            "completion_tokens": 11,
            "total_tokens": 23
        }
    })
}

/// OpenAI/OpenRouter-compatible SSE stream: delta chunks, one usage chunk,
/// `[DONE]` sentinel. Returned with `content-type: text/event-stream`.
fn openrouter_sse_body(model: &str) -> String {
    let words: Vec<&str> = STREAM_FULL_TEXT.split(' ').collect();
    let mut out = String::new();
    for (i, word) in words.iter().enumerate() {
        let piece = if i == 0 {
            word.to_string()
        } else {
            format!(" {word}")
        };
        let chunk = serde_json::json!({
            "id": "chatcmpl-mock-stream-001",
            "model": model,
            "choices": [{
                "index": 0,
                "delta": { "role": "assistant", "content": piece },
                "finish_reason": null
            }]
        });
        out.push_str(&format!("data: {chunk}\n\n"));
    }
    // OpenRouter closes streams with a usage chunk and a [DONE] sentinel.
    let usage = serde_json::json!({
        "id": "chatcmpl-mock-stream-001",
        "model": model,
        "choices": [],
        "usage": { "prompt_tokens": 5, "completion_tokens": 9, "total_tokens": 14 }
    });
    out.push_str(&format!("data: {usage}\n\n"));
    out.push_str("data: [DONE]\n\n");
    out
}

/// Mock OpenRouter server state.
#[derive(Clone)]
struct MockState {
    seen_auth: SeenAuth,
    model: String,
}

/// Start the mock provider on an ephemeral port and return `http://.../v1`.
async fn spawn_mock_openrouter() -> (String, SeenAuth) {
    let seen_auth: SeenAuth = Arc::new(Mutex::new(Vec::new()));
    let state = MockState {
        seen_auth: seen_auth.clone(),
        model: "cohere/north-mini-code:free".to_string(),
    };

    let app = axum::Router::new()
        .route(
            "/v1/chat/completions",
            axum::routing::post(chat_completions),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("failed to bind mock OpenRouter");
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    (base, seen_auth)
}

/// `POST /v1/chat/completions` — JSON when `stream` is absent/`false`, SSE
/// when `stream: true`. Records the Authorization header it received.
async fn chat_completions(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> axum::response::Response {
    state.seen_auth.lock().unwrap().push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(String::from)
            .unwrap_or_default(),
    );

    if payload
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        let body = openrouter_sse_body(&state.model);
        axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .unwrap()
    } else {
        axum::response::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                openrouter_completion_body(&state.model).to_string(),
            ))
            .unwrap()
    }
}

/// App harness identical in spirit to `tests/integration.rs`: real migrations,
/// mock AI mode (`openrouter_api_key: None` → no provider calls possible).
async fn setup_app_state() -> (Arc<AppState>, tempfile::TempDir, tempfile::NamedTempFile) {
    let notes_tmp = tempfile::tempdir().unwrap();
    let db_tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = db_tmp.path().to_path_buf();
    let db_url = format!("sqlite://{}", db_path.display());

    let config = Config {
        db_max_connections: 4,
        mcp_timeout_ms: 5_000,
        host: "127.0.0.1".to_string(),
        port: 18093,
        session_secret: "stream_test_secret_key_at_least_32chars!!".to_string(),
        db_path,
        notes_dir: notes_tmp.path().to_path_buf(),
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

    (
        Arc::new(AppState {
            cache_mikareno: std::sync::Arc::new(
                study_platform::academic::calendar_data::CacheMikareno::default(),
            ),
            cache_catalogo: std::sync::Arc::new(
                study_platform::ai::modelos::CacheCatalogo::default(),
            ),
            config,
            db: pool,
            mikareno: None,
            start_time: std::time::Instant::now(),
        }),
        notes_tmp,
        db_tmp,
    )
}

/// JSON body as the SSE-capable frontend would send it.
fn chat_json(message: &str, stream: bool) -> serde_json::Value {
    serde_json::json!({
        "message": message,
        "subject": "biologia",
        "stream": stream,
    })
}

// --- Mock server contract (runs today, zero external network) ---

#[tokio::test]
async fn mock_non_streaming_completion_matches_openrouter_shape() {
    let (base, seen_auth) = spawn_mock_openrouter().await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/chat/completions"))
        .header("authorization", "Bearer sk-or-v1-mocktestkey")
        .json(&chat_json("cual es la unidad de la vida?", false))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["choices"][0]["message"]["content"], STREAM_FULL_TEXT);
    assert_eq!(json["choices"][0]["finish_reason"], "stop");
    assert!(json["usage"]["total_tokens"].as_i64().unwrap() > 0);

    let auth = seen_auth.lock().unwrap().clone();
    assert_eq!(auth[0], "Bearer sk-or-v1-mocktestkey");
}

#[tokio::test]
async fn mock_streaming_completion_delivers_sse_chunks_and_done_sentinel() {
    let (base, _) = spawn_mock_openrouter().await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/chat/completions"))
        .json(&chat_json("explique la fotosintesis", true))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .starts_with("text/event-stream"),
        "streaming route must answer with text/event-stream"
    );

    let raw = resp.text().await.unwrap();
    assert!(
        raw.contains("data: [DONE]"),
        "SSE body must end with the [DONE] sentinel"
    );

    // Assemble delta pieces exactly as an SSE consumer would.
    let mut assembled = String::new();
    for line in raw.lines().filter(|l| l.starts_with("data: ")) {
        let payload = line.trim_start_matches("data: ");
        if payload == "[DONE]" {
            break;
        }
        let chunk: serde_json::Value = serde_json::from_str(payload).unwrap();
        if let Some(piece) = chunk["choices"][0]["delta"]["content"].as_str() {
            assembled.push_str(piece);
        }
    }
    assert_eq!(assembled, STREAM_FULL_TEXT);

    // The mock really produced its stream id (no short-circuit).
    assert!(raw.contains("chatcmpl-mock-stream-001"));
}

// --- App-level mock-mode chat (existing route, real router) ---

/// Mock mode end-to-end through the app: no API key configured → the assistant
/// answers without any provider call, and the persistent budgeting ledger
/// records a 0.0-cost, `estimated` row so the spend gate stays correct.
#[tokio::test]
async fn app_chat_in_mock_mode_answers_without_provider_and_records_zero_cost() {
    let (state, _notes_tmp, _db_tmp) = setup_app_state().await;
    let app = study_platform::handlers::router(state.clone());

    // Mock mode is only reachable through the auth middleware — seed session.
    // (format-valid legacy UUID accepted by the transition fallback)
    sqlx::query("INSERT INTO users (username, password_hash) VALUES ('streamtest', 'mock')")
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, (SELECT id FROM users WHERE username='streamtest'), ?)")
        .bind("550e8411-e29b-41d4-a716-446655440001")
        .bind((chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
    let cookie = "session_id=550e8411-e29b-41d4-a716-446655440001";

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/ai/chat")
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .body(Body::from(
                    chat_json("cual es la funcion de la mitocondria?", false).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // chat_handler answers plain text (StatusCode, String).
    let text = String::from_utf8(
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!text.trim().is_empty(), "mock answer must not be empty");

    // Persistent ledger: exactly one usage row, 0.0 USD, estimated mock row.
    use sqlx::Row;
    let row = sqlx::query("SELECT cost_usd, estimated FROM usage_metrics")
        .fetch_one(&state.db)
        .await
        .expect("usage row recorded");
    let cost: f64 = row.get("cost_usd");
    let estimated: i64 = row.get("estimated");
    assert_eq!(cost, 0.0, "mock traffic cost must be 0.0");
    assert_eq!(estimated, 1, "mock traffic must be marked estimated");
}

// --- App-level SSE route (pending implementation, must compile) ---

/// `POST /api/ai/chat/stream` — SSE chat streaming over the app router.
///
/// IGNORED on purpose. Reason: no streaming chat handler/route exists yet in
/// `src/ai/register_routes` (only JSON `POST /api/ai/chat`), so the request
/// would 404 today and, worse, once a streaming handler lands there is
/// currently no way to point it at [`spawn_mock_openrouter`]
/// (`OPENROUTER_URL` is a hardcoded const with no config/env injection). The
/// test stays compiled and mock-mode-only (`openrouter_api_key: None`),
/// meaning removing the `#[ignore]` can never produce a real provider call —
/// it fails with a 404 as long as the route is missing. Enable it when the
/// streaming route is added.
#[ignore = "POST /api/ai/chat/stream does not exist yet in src/ai; mock-mode only so this can never hit the real provider"]
#[tokio::test]
async fn app_stream_endpoint_streams_sse_when_implemented() {
    let (state, _notes_tmp, _db_tmp) = setup_app_state().await;
    let app = study_platform::handlers::router(state.clone());

    sqlx::query("INSERT INTO users (username, password_hash) VALUES ('streamtest', 'mock')")
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions (id, user_id, expires_at) VALUES (?, (SELECT id FROM users WHERE username='streamtest'), ?)")
        .bind("550e8411-e29b-41d4-a716-446655440002")
        .bind((chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();
    let cookie = "session_id=550e8411-e29b-41d4-a716-446655440002";

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/ai/chat/stream")
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .body(Body::from(
                    chat_json("explique la fotosintesis", true).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .starts_with("text/event-stream"),
        "streaming endpoint must answer with text/event-stream"
    );
    let raw = String::from_utf8(
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(raw.contains("data: [DONE]"));
}
