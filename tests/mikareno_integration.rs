//! Integration tests for the Mikareno MCP client.
//!
//! These spin up an in-process mock SSE server (`tokio::net::TcpListener`)
//! that speaks the same MCP-over-HTTP dialect as the real FastMCP server:
//!  - `initialize` → `Mcp-Session-Id` response header
//!  - `tools/list`  → `{"tools":[...]}`
//!  - `tools/call`  → `{"content":[{"type":"text","text":"<json>"}]}`
//!
//! No real Mikareno server is contacted.

use std::sync::Arc;
use std::sync::Mutex;

use serde_json::json;
use study_platform::integrations::mikareno::{
    CalendarioEvent, Horario, MikarenoClient, Nota, Perfil, Tarea, ToolInfo,
};

/// A canned response the mock server emits (wrapped in a JSON-RPC envelope and
/// sent as an SSE `data:` line).
struct MockResp {
    status: u16,
    headers: Vec<(String, String)>,
    result: serde_json::Value,
}

type Handler = Arc<dyn Fn(&str, &str) -> MockResp + Send + Sync>;

/// Canned academic payloads shaped exactly like the real Mikareno `get_*` tools.
fn calendario_payload() -> serde_json::Value {
    json!({
        "resumen": {
            "total_dias": 3, "dias_de_clases": 2, "dias_no_lectivos": 1,
            "otros_eventos": 0, "por_titulo": {"Clase": 2, "Vacaciones": 1}
        },
        "proximos_dias_clases": [
            {"fecha": "2024-03-01T00:00:00", "titulo": "Clase de Matemáticas", "tipo_dia": "SCHOOL_DAY"},
            {"fecha": "2024-03-02", "titulo": "Clase de Lenguaje", "tipo_dia": "SCHOOL_DAY"},
        ],
        "vacaciones": [
            {"fecha": "2024-03-15", "titulo": "Vacaciones de Semana", "tipo_dia": "NON_SCHOOL_DAY"},
        ],
        "eventos": [
            {"fecha": "2024-04-01", "titulo": "Feria del libro", "tipo_dia": ""},
        ],
    })
}

fn horario_payload() -> serde_json::Value {
    json!({
        "horario": [
            {"dia": "lunes", "hora": "08:00-08:40", "clase": "Matemáticas", "codigo": "M1", "duracion_min": 40},
            {"dia": "lunes", "hora": "09:00-09:40", "clase": "Lenguaje", "codigo": "L1", "duracion_min": 40},
            {"dia": "martes", "hora": "08:00-08:40", "clase": "Ciencias", "codigo": "C1", "duracion_min": 40},
        ],
        "total_clases": 3,
        "dias": ["lunes", "martes"],
    })
}

fn perfil_payload() -> serde_json::Value {
    json!({
        "estudiante": {"nombre": "Gabriel", "email": "g@x", "rol": "student"},
        "institucion": {"name": "Colegio Álamo", "officialName": "Álamo"},
        "curso_actual": {"codigo": "7A", "nombre": "7ºB", "paralelo": "A", "seccion": "B", "tutor": "Prof. Rivera"},
        "periodo_lectivo": {"nombre": "T3"},
    })
}

fn tareas_payload() -> serde_json::Value {
    json!({
        "resumen": {"esta_semana": 2, "hoy": 1, "manana": 1},
        "lista_tareas": [
            {"title": "Entregar proyecto", "dueDate": "2024-03-05", "status": "PENDING"},
            {"title": "Ejercicios", "fechaVencimiento": "2024-03-06", "estado": "ENTREGADA"},
        ],
        "total_lista": 2,
    })
}

fn notas_payload() -> serde_json::Value {
    json!({
        "trimestre": 1,
        "materias_y_notas": [
            {"materia": "Matemáticas", "nota": 8.5, "max_score": 10, "codigo": "M1", "is_locked": false},
            {"materia": "Lenguaje", "nota": 9.0, "max_score": 10, "codigo": "L1", "is_locked": false},
        ],
        "total_materias": 2,
        "promedio_general": 8.75,
    })
}

fn tools_list_payload() -> serde_json::Value {
    json!({
        "tools": [
            {"name": "get_calendario", "description": "Calendario escolar"},
            {"name": "get_horario", "description": "Horario semanal"},
            {"name": "get_perfil", "description": "Perfil del estudiante"},
            {"name": "get_tareas", "description": "Deberes/actividades"},
            {"name": "get_notas_trimestre", "description": "Notas por materia"},
        ]
    })
}

fn initialize_result_payload() -> serde_json::Value {
    json!({
        "protocolVersion": "2025-03-26",
        "capabilities": {},
        "serverInfo": {"name": "Mikareno Student MCP", "version": "0.1.0"},
    })
}

fn tool_content(payload: serde_json::Value) -> serde_json::Value {
    json!({
        "content": [{"type": "text", "text": serde_json::to_string(&payload).unwrap()}]
    })
}

/// Build the handler used by most tests: a fully-behaved Mikareno mock that
/// also records every request into the shared log.
fn full_handler() -> (Handler, Arc<Mutex<Vec<String>>>) {
    let counts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let counts_for_handler = counts.clone();
    let handler: Handler = Arc::new(move |method: &str, name: &str| {
        {
            let mut v = counts_for_handler.lock().unwrap();
            v.push(format!("{}::{}", method, name));
        }
        match (method, name) {
            ("initialize", _) => MockResp {
                status: 200,
                headers: vec![("mcp-session-id".to_string(), "sess-test-123".to_string())],
                result: initialize_result_payload(),
            },
            ("tools/list", _) => MockResp {
                status: 200,
                headers: vec![],
                result: tools_list_payload(),
            },
            ("tools/call", "get_calendario") => MockResp {
                status: 200,
                headers: vec![],
                result: tool_content(calendario_payload()),
            },
            ("tools/call", "get_horario") => MockResp {
                status: 200,
                headers: vec![],
                result: tool_content(horario_payload()),
            },
            ("tools/call", "get_perfil") => MockResp {
                status: 200,
                headers: vec![],
                result: tool_content(perfil_payload()),
            },
            ("tools/call", "get_tareas") => MockResp {
                status: 200,
                headers: vec![],
                result: tool_content(tareas_payload()),
            },
            ("tools/call", "get_notas_trimestre") => MockResp {
                status: 200,
                headers: vec![],
                result: tool_content(notas_payload()),
            },
            ("tools/call", other) => MockResp {
                status: 200,
                headers: vec![],
                // FastMCP reports tool-level failures via `isError`.
                result: json!({
                    "isError": true,
                    "content": [{"type": "text", "text": format!("unknown tool: {}", other)}]
                }),
            },
            _ => MockResp {
                status: 404,
                headers: vec![],
                result: json!({"error": "unknown method"}),
            },
        }
    });
    (handler, counts)
}

/// Spawn the mock SSE server on an ephemeral port. Returns `(base_url, counts)`.
async fn spawn_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let (handler, counts) = full_handler();
    (spawn_server_with_handler(handler).await, counts)
}

/// Spawn the mock SSE server with an explicit handler. Returns the base URL.
///
/// The accept loop is detached as a tokio task; on the test's runtime it is
/// polled whenever the main task awaits (i.e. while the client is sending
/// requests), so connections are serviced correctly.
async fn spawn_server_with_handler(handler: Handler) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}/mcp", addr.port());

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((socket, _)) => {
                    let h = handler.clone();
                    tokio::spawn(async move {
                        handle_socket(socket, h).await;
                    });
                }
                Err(_) => break,
            }
        }
    });

    base_url
}

/// Read one HTTP request, invoke the handler, and write back the SSE response.
async fn handle_socket(mut socket: tokio::net::TcpStream, handler: Handler) {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let mut reader = BufReader::new(&mut socket);
    let mut req_line = String::new();
    if reader.read_line(&mut req_line).await.unwrap_or(0) == 0 {
        return;
    }

    let mut header_lines: Vec<String> = Vec::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await.unwrap_or(0);
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        header_lines.push(line.clone());
    }

    let mut content_length: usize = 0;
    for h in &header_lines {
        let low = h.to_ascii_lowercase();
        if low.starts_with("content-length:") {
            content_length = h.splitn(2, ':').nth(1).unwrap().trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        if reader.read_exact(&mut body).await.is_err() {
            return;
        }
    }

    let body_val: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let method = body_val
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let id = body_val.get("id").cloned().unwrap_or(json!(1));
    let name = body_val
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let resp = handler(method, name);
    let envelope = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": resp.result,
    });
    let data_line = format!("data: {}\n\n", serde_json::to_string(&envelope).unwrap());

    let mut head = format!("HTTP/1.1 {} OK\r\n", resp.status);
    head.push_str("content-type: text/event-stream\r\n");
    head.push_str(&format!("content-length: {}\r\n", data_line.len()));
    for (k, v) in &resp.headers {
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("connection: close\r\n\r\n");

    let mut out = head.into_bytes();
    out.extend_from_slice(data_line.as_bytes());
    let _ = socket.write_all(&out).await;
    let _ = socket.flush().await;
}

/// Count how many times a given `method::name` key was recorded.
fn count_invocations(counts: &Arc<Mutex<Vec<String>>>, key: &str) -> usize {
    counts
        .lock()
        .unwrap()
        .iter()
        .filter(|v| v.as_str() == key)
        .count()
}

#[tokio::test]
async fn initialize_stores_session_and_connects() {
    let (base, _counts) = spawn_server().await;
    let client = MikarenoClient::new(base);

    assert!(!client.is_connected());
    client
        .initialize()
        .await
        .expect("initialize should succeed");
    assert!(client.is_connected());
    assert_eq!(client.session_id(), Some("sess-test-123".to_string()));
}

#[tokio::test]
async fn list_tools_returns_advertised_tools() {
    let (base, _counts) = spawn_server().await;
    let client = MikarenoClient::new(base);
    let tools = client.list_tools().await.expect("list_tools");
    assert!(tools.len() >= 3);
    assert_eq!(
        tools[0],
        ToolInfo {
            name: "get_calendario".into(),
            description: "Calendario escolar".into(),
        }
    );
}

#[tokio::test]
async fn calendario_parses_events() {
    let (base, _counts) = spawn_server().await;
    let client = MikarenoClient::new(base);
    let events: Vec<CalendarioEvent> = client.calendario().await.expect("calendario");
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].fecha, "2024-03-01"); // truncated from "...T00:00:00"
    assert_eq!(events[0].evento, "Clase de Matemáticas");
    assert_eq!(events[0].tipo, "SCHOOL_DAY");
    assert!(events.iter().any(|e| e.tipo == "NON_SCHOOL_DAY"));
}

#[tokio::test]
async fn horario_groups_classes_by_day() {
    let (base, _counts) = spawn_server().await;
    let client = MikarenoClient::new(base);
    let horario: Horario = client.horario().await.expect("horario");
    assert_eq!(horario.dias.len(), 2);
    assert_eq!(horario.dias[0].dia, "lunes");
    assert_eq!(horario.dias[0].clases.len(), 2);
    assert_eq!(horario.dias[1].dia, "martes");
    assert_eq!(horario.dias[1].clases.len(), 1);
    assert!(horario.dias[0].clases[0].contains("Matemáticas"));
}

#[tokio::test]
async fn perfil_extracts_fields() {
    let (base, _counts) = spawn_server().await;
    let client = MikarenoClient::new(base);
    let p: Perfil = client.perfil().await.expect("perfil");
    assert_eq!(p.nombre, "Gabriel");
    assert_eq!(p.institucion, "Colegio Álamo");
    assert!(p.curso.contains("7A"));
    assert!(p.curso.contains("Prof. Rivera"));
}

#[tokio::test]
async fn tareas_extracts_list() {
    let (base, _counts) = spawn_server().await;
    let client = MikarenoClient::new(base);
    let t: Vec<Tarea> = client.tareas().await.expect("tareas");
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].titulo, "Entregar proyecto");
    assert_eq!(t[0].vencimiento, "2024-03-05");
    assert_eq!(t[0].estado, "PENDING");
    assert_eq!(t[1].estado, "ENTREGADA");
}

#[tokio::test]
async fn notas_extracts_grades() {
    let (base, _counts) = spawn_server().await;
    let client = MikarenoClient::new(base);
    let n: Vec<Nota> = client.notas(1).await.expect("notas");
    assert_eq!(n.len(), 2);
    assert_eq!(n[0].materia, "Matemáticas");
    assert!((n[0].nota - 8.5).abs() < 1e-9);
    assert_eq!(n[0].trimestre, 1);
}

#[tokio::test]
async fn cache_deduplicates_tool_calls() {
    let (base, counts) = spawn_server().await;
    let client = MikarenoClient::with_cache_ttl(base, 180);

    // Prime the cache.
    let first: Vec<CalendarioEvent> = client.calendario().await.expect("first calendario");
    assert_eq!(first.len(), 4);

    // Second call should be served from cache -> no extra tool invocation.
    let second: Vec<CalendarioEvent> = client.calendario().await.expect("second calendario");
    assert_eq!(second.len(), 4);
    assert_eq!(second, first);

    assert_eq!(
        count_invocations(&counts, "tools/call::get_calendario"),
        1,
        "get_calendario should only hit the server once (cached)"
    );
    assert_eq!(
        count_invocations(&counts, "initialize::"),
        1,
        "initialize should run once"
    );
}

#[tokio::test]
async fn disconnect_is_detected_and_returns_error() {
    // Bind then immediately drop so the port has no listener -> connection refused.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let client = MikarenoClient::new(format!("http://127.0.0.1:{}/mcp", port));
    let err = client.initialize().await;
    assert!(err.is_err(), "initialize against a dead port must error");
    assert!(!client.is_connected());
    let msg = err.unwrap_err().to_string();
    assert!(
        msg.to_lowercase().contains("connect")
            || msg.to_lowercase().contains("refused")
            || msg.to_lowercase().contains("network"),
        "unexpected error: {}",
        msg
    );
}

#[tokio::test]
async fn call_tool_unknown_method_yields_error() {
    let (base, _counts) = spawn_server().await;
    let client = MikarenoClient::new(base);
    client.initialize().await.unwrap();
    let err = client.call_tool("does_not_exist", json!({})).await;
    // Unknown tool comes back as `isError`, mapped to an error by the client.
    assert!(err.is_err(), "unknown tool should surface an error");
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("unknown tool") || msg.contains("isError"));
}

#[tokio::test]
async fn retries_on_transient_errors() {
    // Dead port → connection refused on every attempt. The client should retry
    // with exponential backoff before finally failing.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let client = MikarenoClient::with_cache_ttl(format!("http://127.0.0.1:{}/mcp", port), 180);
    let start = std::time::Instant::now();
    let err = client.initialize().await;
    let elapsed = start.elapsed();
    assert!(err.is_err());
    assert!(
        elapsed >= std::time::Duration::from_millis(1000),
        "expected backoff delay, got {:?}",
        elapsed
    );
}

// ── Live `tools/list` discovery (Mission 11) ─────────────────────────────
//
// Recorded 2026-10-10 via a single `initialize` + `tools/list` against the
// live server at `http://127.0.0.1:8080/mcp` (serverInfo: "Mikareno Student
// MCP" v4.0.11). Discovery only — NO data tool was called, NO personal
// academic data was queried, and the server was NOT modified.
//
// REAL live tool inventory (17 tools). Every tool accepts an optional
// `force_refresh: boolean` (default false); extra inputs per tool are noted:
//
// | #  | name                      | description (server)                        | extra inputSchema            |
// |----|---------------------------|---------------------------------------------|------------------------------|
// | 1  | get_student_full          | TODO lo del estudiante (todas secciones)    | sections?: string|null       |
// | 2  | get_notas_trimestre       | Notas por materia del trimestre (1, 2 o 3)  | trimestre:=1                 |
// | 3  | get_horario               | Horario semanal completo                    | —                            |
// | 4  | get_tareas                | Deberes/Actividades + lista por fecha       | —                            |
// | 5  | get_detalle_materia       | Notas por actividad/parcial por materia     | materia?: string|null        |
// | 6  | get_actividades_materia   | Desglose TAREAS/TRABAJOS/EVAL/EXAMEN        | materia?, trimestre:=1       |
// | 7  | get_calendario            | Calendario: clases, vacaciones, eventos     | —                            |
// | 8  | get_mensajes              | Bandeja entrada, no leídos, resumen         | —                            |
// | 9  | get_detalle_mensaje       | Cuerpo + adjuntos de un mensaje             | message_id: integer REQUIRED |
// | 10 | get_asistencia            | Asistencia semanal/detalle/config           | —                            |
// | 11 | get_semaforo              | Semáforo de comportamiento                  | —                            |
// | 12 | get_perfil                | Perfil: identidad, curso, tutor, matrícula  | —                            |
// | 13 | get_ano_escolar           | Año escolar: fechas, términos, parciales    | —                            |
// | 14 | get_reconocimientos       | Reconocimientos y logros                    | —                            |
// | 15 | get_esquemas_calificacion | Esquemas de calificación y colores          | —                            |
// | 16 | get_biblioteca            | Recursos institucionales de biblioteca      | —                            |
// | 17 | get_cumpleanos            | Celebración de cumpleaños en plataforma     | —                            |
//
// Diff vs `docs/ARCHITECTURE.md` §3 (claims 17, stale):
//   * ARCH lists 7 tools the live server does NOT advertise:
//     get_detalles_asignatura, get_detalles_tarea, get_detalles_evento,
//     get_detalles_grupo, get_detalles_profesor, get_mis_grupos,
//     get_detalle_calendario.
//   * ARCH's `get_detalles_mensaje` (plural) ≠ live `get_detalle_mensaje`.
//   * ARCH omits 7 live tools: get_student_full, get_tareas,
//     get_detalle_materia, get_actividades_materia, get_semaforo,
//     get_reconocimientos, get_esquemas_calificacion.
//   * Intersection (10): get_perfil, get_ano_escolar, get_asistencia,
//     get_biblioteca, get_calendario, get_cumpleanos, get_horario,
//     get_mensajes, get_notas_trimestre + get_tareas (client-only/ARCH gap).
//     NOTE: `get_tareas` IS on the live list — the ARCH-vs-client gap is an
//     ARCH staleness issue, now closed; no client change needed.
//
// Diff vs implemented client (5 tools, all present live):
//   get_calendario ✓, get_horario ✓, get_perfil ✓, get_tareas ✓,
//   get_notas_trimestre ✓. The dashboard (`src/academic/`) calls exactly
//   these 5, so NO new adapter was required by Mission 11 rule 3(b).

/// Static snapshot of the live `tools/list` names (2026-10-10, v4.0.11).
/// Kept as a test fixture — this test never contacts the network.
const LIVE_TOOL_NAMES: &[&str] = &[
    "get_student_full",
    "get_notas_trimestre",
    "get_horario",
    "get_tareas",
    "get_detalle_materia",
    "get_actividades_materia",
    "get_calendario",
    "get_mensajes",
    "get_detalle_mensaje",
    "get_asistencia",
    "get_semaforo",
    "get_perfil",
    "get_ano_escolar",
    "get_reconocimientos",
    "get_esquemas_calificacion",
    "get_biblioteca",
    "get_cumpleanos",
];

/// Tools the typed client exercises (one per public adapter method).
const CLIENT_TOOL_NAMES: [&str; 5] = [
    "get_calendario",
    "get_horario",
    "get_perfil",
    "get_tareas",
    "get_notas_trimestre",
];

#[test]
fn live_inventory_has_17_tools_and_covers_client() {
    assert_eq!(
        LIVE_TOOL_NAMES.len(),
        17,
        "live list snapshot must hold 17 tools"
    );
    for tool in CLIENT_TOOL_NAMES.iter() {
        assert!(
            LIVE_TOOL_NAMES.contains(tool),
            "client tool {} must exist on the live list",
            tool
        );
    }
}

#[tokio::test]
async fn mock_tools_list_is_subset_of_live_inventory() {
    let (base, _counts) = spawn_server().await;
    let client = MikarenoClient::new(base);
    let tools = client.list_tools().await.expect("list_tools");
    assert!(!tools.is_empty());
    for t in &tools {
        assert!(
            LIVE_TOOL_NAMES.contains(&t.name.as_str()),
            "mock tool {} is not on the live list — fixture drift?",
            t.name
        );
    }
}

#[tokio::test]
async fn tareas_absence_degrades_gracefully() {
    // If a server ever stops advertising `get_tareas` (the old ARCH gap),
    // the typed call must surface an error and the dashboard's
    // `unwrap_or_default()` pattern must yield an empty list — never a panic.
    let handler: Handler = Arc::new(move |method: &str, _name: &str| match method {
        "initialize" => MockResp {
            status: 200,
            headers: vec![("mcp-session-id".to_string(), "sess-no-tareas".to_string())],
            result: initialize_result_payload(),
        },
        "tools/list" => MockResp {
            status: 200,
            headers: vec![],
            result: json!({"tools": []}),
        },
        _ => MockResp {
            status: 200,
            headers: vec![],
            result: json!({
                "isError": true,
                "content": [{"type": "text", "text": "unknown tool: get_tareas"}]
            }),
        },
    });
    // Note: the catch-all above also covers tools/call; match on method only.
    let base = spawn_server_with_handler(handler).await;
    let client = MikarenoClient::new(base);
    let err = client.tareas().await;
    assert!(err.is_err(), "absent get_tareas must surface an error");
    // Dashboard pattern: fall back to an empty list.
    let tareas: Vec<Tarea> = err.unwrap_or_default();
    assert!(tareas.is_empty());
}
