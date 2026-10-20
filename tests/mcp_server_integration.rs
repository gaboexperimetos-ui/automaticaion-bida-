//! Integration tests for the custom MCP server.
//! Tests JSON-RPC protocol, authentication, tool execution, and parameter validation.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;

use study_platform::config::Config;
use study_platform::AppState;

fn test_config(
    db_path: PathBuf,
    notes_dir: PathBuf,
    api_key: Option<String>,
    mikareno_url: &str,
) -> Config {
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
        mikareno_mcp_url: mikareno_url.to_string(),
        genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
        custom_mcp_api_key: api_key,
        registration_token: None,
        rust_log: "test".to_string(),
        auth_disabled: false,
    }
}

/// Default test setup: SQLite in-memory (temp file), notes dir, dev-mode auth.
/// Uses a localhost Mikareno URL (so `mikareno_client` returns `Some`).
async fn setup_test_state(api_key: Option<String>) -> Arc<AppState> {
    setup_test_state_with_mikareno(api_key, "http://127.0.0.1:8080/mcp").await
}

/// Test setup with a custom Mikareno URL.
/// Use a non-localhost URL (e.g. "http://example.invalid/mcp") to make
/// `mikareno_client` return `None` — useful for testing `get_academic_context`
/// without waiting on connection retries.
async fn setup_test_state_with_mikareno(
    api_key: Option<String>,
    mikareno_url: &str,
) -> Arc<AppState> {
    let temp_db = tempfile::NamedTempFile::new().unwrap();
    let notes_dir = tempfile::tempdir().unwrap().keep();
    let config = test_config(
        temp_db.path().to_path_buf(),
        notes_dir,
        api_key,
        mikareno_url,
    );

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

    // Full schema matching migrations/20240101000000_init.sql + 20240101000002_knowledge.sql
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

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS note_versions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
            content TEXT NOT NULL,
            word_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .execute(&pool)
    .await
    .unwrap();

    // FTS5 virtual table for full-text search (bundled SQLite has FTS5 enabled)
    let _ = sqlx::query(
        "CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
            title,
            content,
            tokenize = 'unicode61'
        );",
    )
    .execute(&pool)
    .await;

    let _ = sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_note_versions_note_id ON note_versions(note_id);",
    )
    .execute(&pool)
    .await;

    Arc::new(AppState { config, db: pool,
        mikareno: None,
        start_time: std::time::Instant::now(),
    })
}

/// Send a raw JSON-RPC request to the MCP endpoint and return (status, parsed body).
async fn send_jsonrpc(
    state: Arc<AppState>,
    api_key: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let app = study_platform::handlers::router(state);

    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json");

    if let Some(key) = api_key {
        builder = builder.header("x-api-key", key);
    }

    let response = app
        .oneshot(
            builder
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let parsed: Value = serde_json::from_slice(&bytes).unwrap();
    (status, parsed)
}

/// Build a `tools/call` JSON-RPC request body.
fn tools_call(name: &str, args: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "params": {
            "name": name,
            "arguments": args
        },
        "id": 1
    })
}

// ---------------------------------------------------------------------------
// Existing tests (enhanced with JSON-RPC error-code assertions)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_initialize() {
    let state = setup_test_state(None).await;

    let (status, result) = send_jsonrpc(
        state,
        None,
        json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "id": 1
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["jsonrpc"], "2.0");
    assert!(result["result"]["serverInfo"].is_object());
    assert_eq!(result["result"]["serverInfo"]["name"], "study-platform-mcp");
    assert_eq!(result["result"]["protocolVersion"], "2025-03-26");
}

#[tokio::test]
async fn test_tools_list() {
    let state = setup_test_state(None).await;

    let (status, result) = send_jsonrpc(
        state,
        None,
        json!({
            "jsonrpc": "2.0",
            "method": "tools/list",
            "id": 1
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["jsonrpc"], "2.0");
    let tools = result["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 9);
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"search_notes"));
    assert!(names.contains(&"get_note"));
    assert!(names.contains(&"create_note"));
    assert!(names.contains(&"get_academic_context"));
    assert!(names.contains(&"get_subject_notes"));
    assert!(names.contains(&"get_study_sheets"));
}

#[tokio::test]
async fn test_auth_rejects_no_key() {
    let state = setup_test_state(Some("secret-key-12345".to_string())).await;

    let (_, result) = send_jsonrpc(
        state,
        None,
        json!({
            "jsonrpc": "2.0",
            "method": "tools/list",
            "id": 1
        }),
    )
    .await;

    // Without X-API-Key and a key is configured → JSON-RPC error
    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32001);
    assert_eq!(result["error"]["message"], "Unauthorized");
}

#[tokio::test]
async fn test_auth_accepts_valid_key() {
    let state = setup_test_state(Some("valid-key-12345".to_string())).await;

    let (_, result) = send_jsonrpc(
        state,
        Some("valid-key-12345"),
        json!({
            "jsonrpc": "2.0",
            "method": "tools/list",
            "id": 1
        }),
    )
    .await;

    assert!(result["result"].is_object());
    assert!(result["result"]["tools"].is_array());
}

#[tokio::test]
async fn test_auth_no_key_configured() {
    // When no custom_mcp_api_key is set, all requests are allowed (dev mode)
    let state = setup_test_state(None).await;

    let (_, result) = send_jsonrpc(
        state,
        None,
        json!({
            "jsonrpc": "2.0",
            "method": "tools/list",
            "id": 1
        }),
    )
    .await;

    assert!(result["result"].is_object());
}

#[tokio::test]
async fn test_method_not_found() {
    let state = setup_test_state(None).await;

    let (_, result) = send_jsonrpc(
        state,
        None,
        json!({
            "jsonrpc": "2.0",
            "method": "unknown_method",
            "id": 1
        }),
    )
    .await;

    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32601);
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unknown_method"));
}

#[tokio::test]
async fn test_invalid_jsonrpc_version() {
    let state = setup_test_state(None).await;

    let (status, result) = send_jsonrpc(
        state,
        None,
        json!({
            "jsonrpc": "1.0",
            "method": "tools/list",
            "id": 1
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(result["jsonrpc"], "2.0");
    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32600);
}

// ---------------------------------------------------------------------------
// tools/call end-to-end tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_tools_call_search_notes() {
    let state = setup_test_state(None).await;

    // First create a note so search has something to find
    let (db, notes_dir) = (state.db.clone(), state.config.notes_dir.clone());
    sqlx::query(
        r#"INSERT INTO notes (title, slug, filename, tags, word_count, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, datetime('now'), datetime('now'))"#,
    )
    .bind("Test Note")
    .bind("test")
    .bind("test.md")
    .bind("")
    .bind(5)
    .execute(&db)
    .await
    .unwrap();

    // Insert content into FTS index and note_versions
    let note_id: i64 = sqlx::query_scalar("SELECT id FROM notes WHERE slug = 'test'")
        .fetch_one(&db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO note_versions (note_id, content, word_count) VALUES (?, ?, ?)")
        .bind(note_id)
        .bind("hello world this is a test note")
        .bind(6)
        .execute(&db)
        .await
        .unwrap();
    let _ = sqlx::query("INSERT INTO notes_fts(rowid, title, content) VALUES (?, ?, ?)")
        .bind(note_id)
        .bind("Test Note")
        .bind("hello world this is a test note")
        .execute(&db)
        .await;
    // Write the actual file so get_note can read it
    std::fs::create_dir_all(&notes_dir).ok();
    std::fs::write(
        notes_dir.join("test.md"),
        "---\ntitle: Test Note\ntags: []\ncreated: 2024-01-01T00:00:00Z\nmodified: 2024-01-01T00:00:00Z\n---\n\nhello world",
    )
    .ok();

    let (status, result) = send_jsonrpc(
        state,
        None,
        tools_call(
            "search_notes",
            json!({
                "query": "test"
            }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], 1);
    // The result should have a content array
    let content = result["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    // The text field should be valid JSON (array of search results)
    let text = content[0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert!(parsed.is_array());
    let results = parsed.as_array().unwrap();
    assert!(results.len() >= 1);
    assert_eq!(results[0]["note"]["slug"], "test");
}

#[tokio::test]
async fn test_tools_call_search_notes_empty() {
    let state = setup_test_state(None).await;

    let (status, result) = send_jsonrpc(
        state,
        None,
        tools_call(
            "search_notes",
            json!({
                "query": "nonexistent"
            }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["jsonrpc"], "2.0");
    let content = result["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    let text = content[0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert!(parsed.is_array());
    assert!(parsed.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_tool_create_note() {
    let state = setup_test_state(None).await;
    let db = state.db.clone();
    let notes_dir = state.config.notes_dir.clone();

    let (status, result) = send_jsonrpc(
        state,
        None,
        tools_call(
            "create_note",
            json!({
                "title": "test",
                "content": "hello"
            }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], 1);
    assert!(result["error"].is_null());
    let content = result["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    // The text field should be the JSON-serialized Note
    let text = content[0]["text"].as_str().unwrap();
    let note: Value = serde_json::from_str(text).unwrap();
    assert_eq!(note["title"], "test");
    assert_eq!(note["slug"], "test");
    assert_eq!(note["filename"], "test.md");

    // Verify the note exists in the DB
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes WHERE slug = 'test'")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Verify a version was stored
    let version_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM note_versions WHERE note_id IN (SELECT id FROM notes WHERE slug = 'test')")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(version_count, 1);

    // Verify the file was written to disk
    let file_path = notes_dir.join("test.md");
    assert!(file_path.exists());
    let file_content = std::fs::read_to_string(&file_path).unwrap();
    assert!(file_content.contains("hello"));
}

#[tokio::test]
async fn test_tool_get_note() {
    let state = setup_test_state(None).await;

    // First create a note via MCP
    let (_status, result) = send_jsonrpc(
        state.clone(),
        None,
        tools_call(
            "create_note",
            json!({
                "title": "test",
                "content": "hello world"
            }),
        ),
    )
    .await;
    assert_eq!(result["result"]["content"][0]["type"], "text");

    // Now retrieve it
    let (status, result) = send_jsonrpc(
        state,
        None,
        tools_call(
            "get_note",
            json!({
                "slug": "test"
            }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], 1);
    assert!(result["error"].is_null());
    let content = result["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    let text = content[0]["text"].as_str().unwrap();
    // Text contains JSON-serialized note + content body
    assert!(text.contains("\"slug\":\"test\""));
    assert!(text.contains("hello world"));
}

#[tokio::test]
async fn test_tool_get_note_not_found() {
    let state = setup_test_state(None).await;

    let (status, result) = send_jsonrpc(
        state,
        None,
        tools_call(
            "get_note",
            json!({
                "slug": "nonexistent"
            }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["jsonrpc"], "2.0");
    // Note-not-found is returned as a JSON-RPC error with code -32000
    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32000);
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not found"));
}

#[tokio::test]
async fn test_tool_get_academic_context() {
    // Use a non-localhost URL so mikareno_client returns None instantly
    // (avoids slow connection-retry behaviour against an unreachable localhost port).
    let state = setup_test_state_with_mikareno(None, "http://example.invalid/mcp").await;

    let (status, result) =
        send_jsonrpc(state, None, tools_call("get_academic_context", json!({}))).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["jsonrpc"], "2.0");
    assert!(result["error"].is_null());
    let content = result["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    // The text is a JSON string with "connected": false
    let text = content[0]["text"].as_str().unwrap();
    let ctx: Value = serde_json::from_str(text).unwrap();
    assert_eq!(ctx["connected"], false);
    assert!(ctx["error"].is_string());
}

// ---------------------------------------------------------------------------
// Parameter validation tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_tool_call_missing_params() {
    let state = setup_test_state(None).await;

    // search_notes without query → -32602
    let (_, result) =
        send_jsonrpc(state.clone(), None, tools_call("search_notes", json!({}))).await;
    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32602);
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("query"));

    // create_note without title → -32602
    let (_, result) = send_jsonrpc(
        state.clone(),
        None,
        tools_call("create_note", json!({ "content": "hello" })),
    )
    .await;
    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32602);
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("title"));

    // create_note without content → -32602
    let (_, result) = send_jsonrpc(
        state.clone(),
        None,
        tools_call("create_note", json!({ "title": "test" })),
    )
    .await;
    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32602);
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("content"));

    // get_note without slug → -32602
    let (_, result) = send_jsonrpc(state, None, tools_call("get_note", json!({}))).await;
    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32602);
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("slug"));
}

#[tokio::test]
async fn test_tools_call_with_invalid_params() {
    let state = setup_test_state(None).await;

    // tools/call without name → -32602
    let (_, result) = send_jsonrpc(
        state.clone(),
        None,
        json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "arguments": {} },
            "id": 1
        }),
    )
    .await;
    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32602);
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("name"));

    // tools/call with name not a string → -32602
    let (_, result) = send_jsonrpc(
        state.clone(),
        None,
        json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "name": 123, "arguments": {} },
            "id": 2
        }),
    )
    .await;
    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32602);

    // Unknown tool name → -32601
    let (_, result) = send_jsonrpc(
        state.clone(),
        None,
        tools_call("nonexistent_tool", json!({})),
    )
    .await;
    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32601);

    // search_notes with query as number (wrong type) → -32602
    let (_, result) = send_jsonrpc(
        state,
        None,
        tools_call("search_notes", json!({ "query": 123 })),
    )
    .await;
    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32602);
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("query"));
}

#[tokio::test]
async fn test_tools_call_with_auth() {
    // Verify that tools/call works with auth when a key is configured
    let state = setup_test_state(Some("test-key-12345".to_string())).await;
    let db = state.db.clone();

    let (status, result) = send_jsonrpc(
        state,
        Some("test-key-12345"),
        tools_call("search_notes", json!({ "query": "test" })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["jsonrpc"], "2.0");
    assert!(result["result"].is_object());
    assert!(result["result"]["content"].is_array());

    // Verify it works without error even though DB is empty
    let _ = db;
}

#[tokio::test]
async fn test_tools_call_auth_reject() {
    // tools/call should also be protected by auth
    let state = setup_test_state(Some("secret-key".to_string())).await;

    let (_, result) = send_jsonrpc(
        state,
        None,
        tools_call("search_notes", json!({ "query": "test" })),
    )
    .await;

    assert!(result["error"].is_object());
    assert_eq!(result["error"]["code"], -32001);
    assert_eq!(result["error"]["message"], "Unauthorized");
}

#[tokio::test]
async fn test_auth_constant_time_compare_lengths() {
    // Configured key accepts the exact key, rejects same-length and
    // different-length wrong keys (constant-time comparison path).
    let configured = "fictitious-key-AbC12345";
    let state = setup_test_state(Some(configured.to_string())).await;
    let body = json!({
        "jsonrpc": "2.0",
        "method": "tools/list",
        "id": 1
    });

    let (_, ok) = send_jsonrpc(state.clone(), Some(configured), body.clone()).await;
    assert!(ok["result"].is_object());

    let (_, same_len) =
        send_jsonrpc(state.clone(), Some("fictitious-key-XyZ12345"), body.clone()).await;
    assert!(same_len["error"].is_object());
    assert_eq!(same_len["error"]["code"], -32001);

    let (_, diff_len) = send_jsonrpc(state, Some("short"), body).await;
    assert!(diff_len["error"].is_object());
    assert_eq!(diff_len["error"]["code"], -32001);
}
