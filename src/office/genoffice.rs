//! Cliente MCP mínimo para el servidor GenOffice.
//!
//! Implementa el subconjunto del protocolo MCP que necesita la suite de
//! oficina:
//!   1. `initialize` → handshake; captura la cabecera `mcp-session-id`.
//!   2. `tools/call` → invoca `create_docx` y devuelve el `result`.
//!
//! Transporte: POST JSON-RPC 2.0 sobre HTTP. El cuerpo puede llegar como SSE
//! (`data: {...}`) o como documento JSON plano; ambos se aceptan (mismo
//! patrón que el cliente de Mikareno en `integrations/mikareno`).
//!
//! **Requisito de despliegue**: el servidor MCP de GenOffice debe compartir el
//! sistema de archivos con la app (mismo host o volumen montado) porque el
//! `.docx` se genera en disco y la app lo lee desde ahí. Si el archivo no
//! aparece, el error es honesto: no se devuelve ningún `.docx` inventado.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{json, Value};
use thiserror::Error;

/// Versión del protocolo MCP negociada en el handshake.
const PROTOCOL_VERSION: &str = "2025-03-26";

/// Timeout por petición al MCP (la conversión a `.docx` puede tardar).
const REQUEST_TIMEOUT_SECS: u64 = 90;

/// Errores de la suite de oficina (incluye el proxy al MCP de GenOffice).
#[derive(Debug, Error)]
pub enum OfficeError {
    /// `GENOFFICE_MCP_URL` vacío: no hay servidor configurado.
    #[error("GenOffice no está configurado")]
    NotConfigured,
    /// El servidor no respondió (conexión, timeout o HTTP no 2xx).
    #[error("GenOffice no disponible: {0}")]
    Unavailable(String),
    /// Error JSON-RPC devuelto por el servidor.
    #[error("error JSON-RPC ({code}): {message}")]
    Rpc { code: i64, message: String },
    /// La herramienta se ejecutó pero devolvió error.
    #[error("la herramienta {tool} falló: {detail}")]
    ToolFailed { tool: String, detail: String },
    /// Forma de respuesta no reconocida.
    #[error("respuesta MCP inesperada: {0}")]
    Unexpected(String),
    /// Error de E/S (leer o escribir el `.docx` generado).
    #[error("error de E/S: {0}")]
    Io(String),
}

/// Cliente MCP de GenOffice (lectura de `result`, escritura de `create_docx`).
pub struct GenOfficeClient {
    base_url: String,
    http: reqwest::Client,
    /// `mcp-session-id` devuelto por el handshake (si el servidor lo emite).
    session_id: Mutex<Option<String>>,
    /// Contador monotónico para los `id` JSON-RPC.
    next_id: AtomicU64,
}

impl GenOfficeClient {
    /// Crea un cliente apuntando a `base_url` (p. ej. `http://127.0.0.1:8453/mcp`).
    pub fn new(base_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .user_agent("study-platform/0.1.0 (GenOfficeMCPClient)")
            .build()
            .expect("no se pudo crear el cliente HTTP de GenOffice");
        Self {
            base_url: base_url.into(),
            http,
            session_id: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }

    /// URL base del servidor MCP.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Handshake MCP: `initialize` + captura del `mcp-session-id`.
    pub async fn initialize(&self) -> Result<(), OfficeError> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": self.next_id(),
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "study-platform-office",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }
        });

        let (session_id, _result) = self.post(&body).await?;
        if let Some(sid) = session_id {
            *self.session_id.lock().unwrap_or_else(|e| e.into_inner()) = Some(sid);
        }
        Ok(())
    }

    /// Invoca una herramienta MCP y devuelve su objeto `result`.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, OfficeError> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": self.next_id(),
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });

        let (_sid, result) = self.post(&body).await?;

        let failed = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || result.get("ok").and_then(|v| v.as_bool()) == Some(false);
        if failed {
            return Err(OfficeError::ToolFailed {
                tool: name.to_string(),
                detail: content_text(&result),
            });
        }
        Ok(result)
    }

    /// Genera un `.docx` a partir de Markdown y lo deja escrito en `out`.
    ///
    /// `out` debe ser una ruta absoluta: el servidor MCP puede tener otro
    /// directorio de trabajo.
    pub async fn create_docx(&self, markdown: &str, out: &Path) -> Result<(), OfficeError> {
        let args = json!({
            "markdown": markdown,
            "out": out.to_string_lossy(),
            "force": true,
        });
        self.call_tool("create_docx", args).await?;
        Ok(())
    }

    /// POST JSON-RPC: devuelve `(mcp-session-id, result)`.
    async fn post(&self, body: &Value) -> Result<(Option<String>, Value), OfficeError> {
        let session = self
            .session_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        let mut builder = self
            .http
            .post(&self.base_url)
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .json(body);
        if let Some(sid) = session.as_deref() {
            builder = builder.header("mcp-session-id", sid);
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| OfficeError::Unavailable(e.to_string()))?;

        let status = resp.status();
        let session_id = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let text = resp
            .text()
            .await
            .map_err(|e| OfficeError::Unavailable(e.to_string()))?;

        if !status.is_success() {
            return Err(OfficeError::Unavailable(format!("HTTP {status}")));
        }

        let result = extract_jsonrpc(&parse_sse(&text))?;
        Ok((session_id, result))
    }
}

/// Divide el cuerpo de la respuesta en mensajes JSON-RPC (SSE o JSON plano).
fn parse_sse(body: &str) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut data = String::new();
    let mut have_data = false;

    for raw in body.split('\n') {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            flush_data(&data, have_data, &mut messages);
            data.clear();
            have_data = false;
        } else if let Some(rest) = line.strip_prefix("data:") {
            have_data = true;
            let content = rest.strip_prefix(' ').unwrap_or(rest);
            if data.is_empty() {
                data.push_str(content);
            } else {
                data.push('\n');
                data.push_str(content);
            }
        }
        // `event:`, `retry:` y comentarios se ignoran.
    }
    flush_data(&data, have_data, &mut messages);

    if messages.is_empty() {
        // No es SSE — el cuerpo completo es un documento JSON-RPC.
        if let Ok(v) = serde_json::from_str::<Value>(body.trim()) {
            messages.push(v);
        }
    }
    messages
}

fn flush_data(data: &str, have: bool, out: &mut Vec<Value>) {
    if have {
        let trimmed = data.trim();
        if !trimmed.is_empty() {
            if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                out.push(v);
            }
        }
    }
}

/// Extrae el `result` del primer mensaje válido; propaga el error JSON-RPC.
fn extract_jsonrpc(messages: &[Value]) -> Result<Value, OfficeError> {
    for msg in messages {
        if let Some(err) = msg.get("error") {
            let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let message = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
                .to_string();
            return Err(OfficeError::Rpc { code, message });
        }
        if let Some(result) = msg.get("result") {
            return Ok(result.clone());
        }
    }
    Err(OfficeError::Unexpected(
        "la respuesta MCP no trae 'result' ni 'error'".into(),
    ))
}

/// Texto plano del contenido de una herramienta (`content[].text`).
fn content_text(result: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(items) = result.get("content").and_then(|c| c.as_array()) {
        for item in items {
            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                parts.push(text.to_string());
            }
        }
    }
    if parts.is_empty() {
        if let Some(text) = result.get("text").and_then(|t| t.as_str()) {
            parts.push(text.to_string());
        }
    }
    if parts.is_empty() {
        result.to_string()
    } else {
        parts.join(" | ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_data_line_is_parsed() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        let msgs = parse_sse(body);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["result"]["ok"], json!(true));
    }

    #[test]
    fn plain_json_body_is_parsed() {
        let body = "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"a\":1}}";
        let msgs = parse_sse(body);
        assert_eq!(msgs.len(), 1);
        assert_eq!(extract_jsonrpc(&msgs).unwrap()["a"], json!(1));
    }

    #[test]
    fn jsonrpc_error_becomes_office_error() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":3,\"error\":{\"code\":-32601,\"message\":\"no such tool\"}}\n\n";
        let err = extract_jsonrpc(&parse_sse(body)).unwrap_err();
        match err {
            OfficeError::Rpc { code, message } => {
                assert_eq!(code, -32601);
                assert_eq!(message, "no such tool");
            }
            other => panic!("variante inesperada: {other:?}"),
        }
    }

    #[test]
    fn is_error_result_is_reported_as_tool_failure() {
        let result = json!({"isError": true, "content": [{"type": "text", "text": "boom"}]});
        assert_eq!(content_text(&result), "boom");
    }
}
