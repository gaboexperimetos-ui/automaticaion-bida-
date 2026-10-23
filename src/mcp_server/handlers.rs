//! MCP JSON-RPC handler dispatch logic.
//!
//! All handlers extract `State<Arc<AppState>>` so they work seamlessly when
//! merged into the main application router.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::mcp_server::server::{JsonRpcRequest, JsonRpcResponse, McpError};
use crate::mcp_server::tools::all_tools;
use crate::AppState;

/// Extract a required string parameter from the arguments Value.
/// Returns -32602 (Invalid params) with a descriptive message when the key is
/// missing or the value is not a JSON string.
fn require_str_param(args: &Value, key: &str) -> Result<String, McpError> {
    match args.get(key) {
        Some(v) if v.is_string() => Ok(v.as_str().unwrap().to_string()),
        Some(v) => Err(McpError::InvalidParams(format!(
            "Parameter '{}' must be a string, got: {}",
            key, v
        ))),
        None => Err(McpError::InvalidParams(format!(
            "Missing '{}' parameter",
            key
        ))),
    }
}

/// Extract an optional string parameter from the arguments Value.
/// Returns -32602 when the key is present but not a JSON string.
fn opt_str_param(args: &Value, key: &str) -> Result<Option<String>, McpError> {
    match args.get(key) {
        Some(v) if v.is_string() => Ok(Some(v.as_str().unwrap().to_string())),
        Some(v) => Err(McpError::InvalidParams(format!(
            "Parameter '{}' must be a string, got: {}",
            key, v
        ))),
        None => Ok(None),
    }
}

/// Handle JSON-RPC POST requests to /mcp
pub async fn mcp_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> (StatusCode, HeaderMap, Json<JsonRpcResponse>) {
    // Authentication
    if !crate::mcp_server::server::check_api_key(&state.config, &headers) {
        let resp = JsonRpcResponse::error(request.id, -32001, "Unauthorized".to_string());
        let h = HeaderMap::new();
        return (StatusCode::OK, h, Json(resp));
    }

    // Validate JSON-RPC version
    if request.jsonrpc != "2.0" {
        let resp = JsonRpcResponse::error(
            request.id,
            -32600,
            "Invalid Request: jsonrpc must be 2.0".to_string(),
        );
        let h = HeaderMap::new();
        return (StatusCode::BAD_REQUEST, h, Json(resp));
    }

    let result = dispatch(
        &state,
        &request.method,
        request.params.unwrap_or(json!({})),
        &request.id,
    )
    .await;

    match result {
        Ok(resp) => {
            let mut h = HeaderMap::new();
            h.insert("content-type", "application/json".parse().unwrap());
            (StatusCode::OK, h, Json(resp))
        }
        Err(e) => {
            let resp = JsonRpcResponse::error(request.id, e.code(), e.to_string());
            let h = HeaderMap::new();
            (StatusCode::OK, h, Json(resp))
        }
    }
}

/// Dispatch a JSON-RPC method to the appropriate handler.
async fn dispatch(
    state: &AppState,
    method: &str,
    params: Value,
    id: &Option<Value>,
) -> Result<JsonRpcResponse, McpError> {
    match method {
        "initialize" => Ok(JsonRpcResponse::success(
            id.clone(),
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {
                    "tools": {},
                    "prompts": {},
                },
                "serverInfo": {
                    "name": "study-platform-mcp",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        )),
        "tools/list" => {
            let tools = all_tools();
            Ok(JsonRpcResponse::success(
                id.clone(),
                json!({ "tools": tools }),
            ))
        }
        "tools/call" => {
            let tool_name = require_str_param(&params, "name")?;

            let args = params.get("arguments").cloned().unwrap_or(json!({}));

            handle_tool_call(state, &tool_name, args, id).await
        }
        _ => Err(McpError::MethodNotFound(method.to_string())),
    }
}

/// Envuelve un payload JSON en el sobre de resultado MCP (`content[].text`).
///
/// El texto es JSON puro, así que un cliente puede hacer `serde_json::from_str`
/// sobre él. Además se copian las claves del payload al nivel superior del
/// resultado (`result["name"]`, `result["success"]`, …) para no obligar a
/// parsear el texto cuando sólo se necesita leer un campo.
///
/// La clave `content` queda reservada al sobre MCP: si el payload trae una
/// clave `content` (p. ej. el cuerpo Markdown de una nota), no se copia al
/// nivel superior porque pisaría el array `content[0].text` y dejaría el
/// resultado sin envoltura MCP válida.
fn mcp_json_result(payload: Value) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "content".to_string(),
        json!([{ "type": "text", "text": payload.to_string() }]),
    );
    if let Some(obj) = payload.as_object() {
        for (k, v) in obj {
            if k == "content" {
                continue;
            }
            map.insert(k.clone(), v.clone());
        }
    }
    Value::Object(map)
}

/// ¿La nota lleva el tag `subject`?
///
/// `Note.tags` es una lista separada por comas. La comparación ignora
/// mayúsculas y espacios: "Matemáticas", "matemáticas" y " MATEMÁTICAS " son
/// la misma asignatura.
///
/// El plegado de mayúsculas es el de Unicode (`to_lowercase`), no el ASCII:
/// las asignaturas llevan tildes ("Matemáticas", "Física", "Química") y con
/// `eq_ignore_ascii_case` la mayúscula acentuada no casaría con la minúscula.
fn note_has_tag(note: &crate::models::Note, subject: &str) -> bool {
    let wanted = subject.trim().to_lowercase();
    if wanted.is_empty() {
        return false;
    }
    note.tags_vec()
        .iter()
        .any(|tag| tag.to_lowercase() == wanted)
}

/// Carga el cuerpo Markdown de cada nota.
///
/// Best-effort: si el `.md` no está en disco la tool no debe caer entera, así
/// que esa nota vuelve con el cuerpo vacío en lugar de propagar el error.
async fn load_note_bodies(
    knowledge: &crate::knowledge::KnowledgeService,
    notes: &[crate::models::Note],
) -> Vec<(crate::models::Note, String)> {
    let mut out = Vec::with_capacity(notes.len());
    for note in notes {
        let body = match knowledge.get_note(&note.slug).await {
            Ok((_, content)) => content,
            Err(_) => String::new(),
        };
        out.push((note.clone(), body));
    }
    out
}

/// Notas de una asignatura.
///
/// El índice FTS5 (`notes_fts`) sólo cubre `title` y `content` — las `tags`
/// NO se indexan — así que una búsqueda full-text por asignatura no encuentra
/// las notas etiquetadas con ella. Por eso el filtro principal es sobre
/// `notes.tags`; la búsqueda full-text queda como respaldo cuando la
/// asignatura aparece en el cuerpo pero en ninguna etiqueta.
async fn subject_notes(
    knowledge: &crate::knowledge::KnowledgeService,
    subject: &str,
    limit: usize,
) -> Result<Vec<crate::models::Note>, McpError> {
    let notes = knowledge
        .list_notes(500)
        .await
        .map_err(McpError::Internal)?;

    let tagged: Vec<crate::models::Note> = notes
        .into_iter()
        .filter(|n| note_has_tag(n, subject))
        .take(limit)
        .collect();

    if !tagged.is_empty() {
        return Ok(tagged);
    }

    // Respaldo: la asignatura puede estar sólo en el texto de la nota.
    Ok(knowledge
        .search(subject, limit)
        .await
        .map_err(McpError::Internal)?
        .into_iter()
        .map(|r| r.note)
        .take(limit)
        .collect())
}

/// Payload de `app_backup_now`: mismo shape que `GET /api/admin/backup`
/// (`success`, `name`, `bytes` y las claves legacy `path`/`size_bytes`).
fn backup_payload(info: &crate::backup::BackupInfo) -> Value {
    json!({
        "success": true,
        "name": info.name.clone(),
        "bytes": info.bytes,
        "path": info.path.clone(),
        "size_bytes": info.bytes,
    })
}

/// Payload de `app_ai_cost`: mismo shape que `GET /api/ai/cost`
/// (`total_cost`, `budget_limit`, `requests_count`, `alert`).
fn ai_cost_payload(summary: &crate::ai::openrouter::CostSummary) -> Value {
    json!({
        "total_cost": summary.total_cost,
        "budget_limit": summary.budget_limit,
        "requests_count": summary.requests_count,
        "alert": summary.alert,
    })
}

/// Frame SSE del protocolo MCP: `event: message\ndata: {json}\n\n`.
///
/// El cliente parsea la línea `data:` como JSON-RPC (spec MCP: el transporte
/// SSE entrega un frame por mensaje, con el JSON en la línea `data:`).
fn sse_frame(value: &Value) -> String {
    match serde_json::to_string(value) {
        Ok(text) => format!("event: message\ndata: {text}\n\n"),
        Err(_) => "event: message\ndata: {}\n\n".to_string(),
    }
}

/// Frame de notificación inicial del transporte SSE.
const SSE_INITIALIZED_FRAME: &str =
    "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n\n";

/// Handle individual tool calls.
async fn handle_tool_call(
    state: &AppState,
    tool_name: &str,
    args: Value,
    id: &Option<Value>,
) -> Result<JsonRpcResponse, McpError> {
    let knowledge = crate::knowledge::KnowledgeService::new(Arc::new(state.clone()));

    let result = match tool_name {
        "search_notes" => {
            let query = require_str_param(&args, "query")?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

            let results = knowledge
                .search(&query, limit)
                .await
                .map_err(McpError::Internal)?;

            mcp_json_result(json!(results))
        }
        "get_note" => {
            let slug = require_str_param(&args, "slug")?;

            match knowledge.get_note(&slug).await {
                // El cuerpo va en `content` dentro del payload JSON, no
                // concatenado al JSON de la nota: así `content[0].text` sigue
                // siendo JSON parseable, como en el resto de tools.
                Ok((note, content)) => mcp_json_result(json!({
                    "note": note,
                    "content": content,
                })),
                Err(e) => return Ok(JsonRpcResponse::error(id.clone(), -32000, e)),
            }
        }
        "create_note" => {
            let title = require_str_param(&args, "title")?;
            let content = require_str_param(&args, "content")?;
            let tags = args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            match knowledge.create_note(&title, content, tags).await {
                Ok(note) => mcp_json_result(json!(note)),
                Err(e) => return Ok(JsonRpcResponse::error(id.clone(), -32000, e)),
            }
        }
        "get_academic_context" => {
            let ctx = if let Some(client) = crate::academic::mikareno_client(state) {
                if client.initialize().await.is_ok() {
                    let calendario = client.calendario().await.unwrap_or_default();
                    let horario = client.horario().await.unwrap_or_default();
                    let perfil = client.perfil().await.unwrap_or_default();
                    json!({
                        "connected": true,
                        "calendario": calendario,
                        "horario": horario,
                        "perfil": perfil,
                    })
                } else {
                    json!({ "connected": false, "error": "MCP not reachable" })
                }
            } else {
                json!({ "connected": false, "error": "MCP URL not configured for localhost" })
            };

            mcp_json_result(ctx)
        }
        "get_subject_notes" => {
            let subject = require_str_param(&args, "subject")?;
            let notes = subject_notes(&knowledge, &subject, 20).await?;
            mcp_json_result(json!(notes))
        }
        "get_study_sheets" => {
            let subject = opt_str_param(&args, "subject")?;

            // Las hojas de estudio no se persisten: la app las genera bajo
            // demanda desde el cuerpo de una nota (`POST /api/ai/study-sheet`).
            // Esta tool devuelve el material del que se derivan —las notas de
            // la asignatura con su contenido— en vez de buscar la palabra
            // "estudio", que no encuentra nada.
            let notes = match subject.as_deref() {
                Some(s) => subject_notes(&knowledge, s, 20).await?,
                None => knowledge.list_notes(20).await.map_err(McpError::Internal)?,
            };

            let sheets = load_note_bodies(&knowledge, &notes).await;
            let payload: Vec<Value> = sheets
                .into_iter()
                .map(|(note, content)| json!({ "note": note, "content": content }))
                .collect();

            mcp_json_result(json!(payload))
        }
        "app_list_backups" => {
            let dir = std::path::Path::new(crate::backup::DEFAULT_DIR);
            let backups = crate::backup::list_backups(dir).await;
            mcp_json_result(json!(backups))
        }
        "app_backup_now" => {
            // Escribe siempre en el directorio único del sistema (`data/backups`).
            let dir = std::path::Path::new(crate::backup::DEFAULT_DIR);
            match crate::backup::backup_now(&state.db, dir).await {
                Ok(info) => mcp_json_result(backup_payload(&info)),
                Err(e) => return Ok(JsonRpcResponse::error(id.clone(), -32000, e.to_string())),
            }
        }
        "app_ai_cost" => {
            let summary = crate::ai::openrouter::persistent_summary(&state.db, &state.config)
                .await
                .map_err(|e| McpError::Internal(e.to_string()))?;
            mcp_json_result(ai_cost_payload(&summary))
        }
        _ => return Err(McpError::MethodNotFound(tool_name.to_string())),
    };

    Ok(JsonRpcResponse::success(id.clone(), result))
}

/// Handle SSE endpoint (POST /mcp/sse for streaming).
///
/// Siempre emite primero el frame `notifications/initialized`. Si el cliente
/// postea una petición JSON-RPC, la respuesta viaja en un frame SSE
/// (`event: message` + `data: {json}`), que es lo que la spec MCP define
/// para el transporte SSE: el cliente parsea la línea `data:` como JSON.
pub async fn mcp_sse_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    if !crate::mcp_server::server::check_api_key(&state.config, &headers) {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::from("Unauthorized"))
            .unwrap();
    }

    let mut body = String::from(SSE_INITIALIZED_FRAME);

    // Cuerpo opcional: si es una petición JSON-RPC válida, se responde.
    if let Ok(bytes) = axum::body::to_bytes(request.into_body(), 1024 * 1024).await {
        if let Ok(req) = serde_json::from_slice::<JsonRpcRequest>(&bytes) {
            if req.jsonrpc == "2.0" {
                let JsonRpcRequest {
                    jsonrpc: _,
                    method,
                    params,
                    id,
                } = req;
                let response =
                    match dispatch(&state, &method, params.unwrap_or(json!({})), &id).await {
                        Ok(r) => r,
                        Err(e) => JsonRpcResponse::error(id.clone(), e.code(), e.to_string()),
                    };
                if let Ok(text) = serde_json::to_value(&response) {
                    body.push_str(&sse_frame(&text));
                }
            }
        }
    }

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(Body::from(body))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_error_codes() {
        let e = McpError::Unauthorized;
        assert_eq!(e.code(), -32001);

        let e = McpError::Parse("test".to_string());
        assert_eq!(e.code(), -32700);

        let e = McpError::MethodNotFound("foo".to_string());
        assert_eq!(e.code(), -32601);
    }

    #[test]
    fn test_jsonrpc_success_response() {
        let resp = JsonRpcResponse::success(Some(json!(1)), json!({"result": "ok"}));
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_jsonrpc_error_response() {
        let resp = JsonRpcResponse::error(Some(json!(1)), -32601, "Method not found".to_string());
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().unwrap().code, -32601);
    }

    #[test]
    fn test_mcp_json_result_wraps_payload_and_keeps_top_level_keys() {
        let result = mcp_json_result(json!({"success": true, "name": "app-x.db", "bytes": 42}));

        // Sobre MCP estándar: content[0].text con el payload en JSON.
        assert_eq!(result["content"][0]["type"], json!("text"));
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["success"], json!(true));
        assert_eq!(parsed["name"], json!("app-x.db"));
        assert_eq!(parsed["bytes"], json!(42));

        // Las claves también están en el nivel superior del resultado.
        assert_eq!(result["success"], json!(true));
        assert_eq!(result["name"], json!("app-x.db"));
        assert_eq!(result["bytes"], json!(42));
    }

    /// El sobre MCP reserva la clave `content`: un payload que traiga una
    /// clave `content` (el cuerpo de una nota) no puede pisar el array
    /// `content[0].text`, o el resultado dejaría de ser MCP válido.
    #[test]
    fn test_mcp_json_result_never_clobbers_the_content_envelope() {
        let result = mcp_json_result(json!({"note": {"slug": "x"}, "content": "# markdown"}));

        // `content` sigue siendo el array del sobre MCP.
        assert!(result["content"].is_array());
        assert_eq!(result["content"][0]["type"], json!("text"));

        // Y el payload completo sigue disponible dentro del texto.
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["note"]["slug"], json!("x"));
        assert_eq!(parsed["content"], json!("# markdown"));
    }

    /// El filtro por tag no depende de mayúsculas ni de espacios.
    #[test]
    fn test_note_has_tag_is_case_and_space_insensitive() {
        let note = crate::models::Note {
            id: 1,
            title: "Derivadas".to_string(),
            slug: "derivadas".to_string(),
            filename: "derivadas.md".to_string(),
            tags: "Matemáticas, Cálculo".to_string(),
            word_count: 10,
            summary: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        };

        assert!(note_has_tag(&note, "matemáticas"));
        assert!(note_has_tag(&note, "MATEMÁTICAS"));
        assert!(note_has_tag(&note, "  Cálculo  "));
        assert!(!note_has_tag(&note, "física"));
        assert!(!note_has_tag(&note, ""));
    }

    #[test]
    fn test_backup_payload_shape_matches_admin_backup_route() {
        let info = crate::backup::BackupInfo {
            name: "app-20260101-000000.db".to_string(),
            path: "data/backups/app-20260101-000000.db".to_string(),
            bytes: 2048,
            created: "2026-01-01 00:00:00".to_string(),
        };
        let payload = backup_payload(&info);
        assert_eq!(payload["success"], json!(true));
        assert_eq!(payload["name"], json!("app-20260101-000000.db"));
        assert_eq!(payload["bytes"], json!(2048));
        assert_eq!(payload["size_bytes"], json!(2048));
        assert_eq!(
            payload["path"],
            json!("data/backups/app-20260101-000000.db")
        );
    }

    #[test]
    fn test_ai_cost_payload_shape() {
        let summary = crate::ai::openrouter::CostSummary {
            total_cost: 1.5,
            budget_limit: 2.0,
            requests_count: 7,
            alert: false,
        };
        let payload = ai_cost_payload(&summary);
        assert_eq!(payload["total_cost"], json!(1.5));
        assert_eq!(payload["budget_limit"], json!(2.0));
        assert_eq!(payload["requests_count"], json!(7));
        assert_eq!(payload["alert"], json!(false));
    }

    #[test]
    fn test_sse_frame_is_parseable_by_the_client() {
        let resp = JsonRpcResponse::success(Some(json!(1)), json!({"tools": []}));
        let value = serde_json::to_value(&resp).unwrap();
        let frame = sse_frame(&value);

        assert!(frame.starts_with("event: message\ndata: "));
        assert!(frame.ends_with("\n\n"));

        // El cliente toma la línea `data:` y la parsea como JSON-RPC.
        let data_line = frame
            .lines()
            .find(|l| l.starts_with("data: "))
            .expect("debe haber una línea data:");
        let parsed: Value = serde_json::from_str(&data_line["data: ".len()..]).unwrap();
        assert_eq!(parsed["jsonrpc"], json!("2.0"));
        assert_eq!(parsed["id"], json!(1));
        assert!(parsed["result"]["tools"].is_array());
    }
}
