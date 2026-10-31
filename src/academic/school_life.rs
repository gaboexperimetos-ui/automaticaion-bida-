//! Vida escolar — página integradora read-only de datos reales de Mikareno.
//!
//! Agent C implementation (revisada: datos reales o estado vacío honesto,
//! errores técnicos plegados, resumen calculado y un único banner MCP).
//!
//! Lee exclusivamente (read-only) estos tools del MCP de Mikareno
//! (`http://127.0.0.1:8080/mcp`), todos con prefijo `get_` — nunca se invoca
//! ningún tool de escritura. `MikarenoClient::call_tool` es el único entry
//! point usado y sólo se le pasan los nombres de abajo:
//!   - get_asistencia → asistencia semanal por día (`attendaceMap`) y detalle.
//!   - get_semaforo   → indicadores de conducta, registros y tipos de registro.
//!   - get_tareas     → conteos de tareas y lista de deberes/actividades.
//!   - get_biblioteca → recursos institucionales (adapter `parse_biblioteca`).
//!
//! Las 6 secciones de la página y su fuente real (sin datos inventados):
//!   | sección        | tool(s)                          | qué muestra                |
//!   |----------------|----------------------------------|----------------------------|
//!   | Asistencia     | get_asistencia                   | registros por día           |
//!   | Puntualidad    | get_semaforo + get_asistencia    | registros de tardanza       |
//!   | Uniforme       | get_semaforo                     | registros de uniforme       |
//!   | Comportamiento | get_semaforo                     | estadísticas + indicadores  |
//!   | Tareas         | get_tareas                       | conteos + lista de tareas   |
//!   | Biblioteca     | get_biblioteca                   | recursos (`parse_biblioteca`)|
//!
//! Reglas de render (honestidad, aplicadas por todas las secciones):
//!   * Cada sección muestra datos reales del MCP o un estado vacío que nombra
//!     la sección y el motivo; nunca un "0" sin contexto ni datos inventados.
//!   * Un fallo de tool se resume en una frase y el error técnico queda en un
//!     `<details>` plegable; el error crudo nunca se muestra en grande.
//!   * El resumen de arriba ("N secciones con datos · M sin datos") se calcula
//!     sobre el estado real de cada sección (y nombra las que no tienen datos).
//!   * `mcp_status_banner` se emite UNA sola vez (arriba): la sección
//!     Biblioteca ya no lo duplica, así que hay un único botón "Reintentar".
//!
//! Read-only verificado: el cliente sólo hace POST JSON-RPC de *lectura*; no
//! hay ruta ni cookie de mutación en este handler. Se pasa `sections=null`
//! (default) y no se escribe estado alguno.
//!
//! Tiempos: un único presupuesto de 5 s para toda la fase MCP (handshake +
//! consultas en paralelo) y una caché de 30 s del resultado completo. Las
//! secciones que no alcancen a responder se muestran con estado vacío
//! honesto ("Mikareno no responde ahora"), nunca con una espera encadenada.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::{
    extract::{Query, State},
    response::Html,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::{self, Duration};

use crate::academic::{mikareno_client, templates};
use crate::integrations::mikareno::{parse_biblioteca, Biblioteca, MikarenoClient, Tarea};
use crate::AppState;

/// Máxima longitud del query param `partial` (más allá se ignora).
const MAX_PARTIAL_LEN: usize = 32;
/// Presupuesto global de la fase MCP (5 s).
///
/// Es UN solo plazo para el handshake y para todas las consultas: por eso
/// las tools se piden en paralelo (ninguna espera a otra) y todas comparten
/// el mismo deadline. Al agotarse, las secciones sin respuesta muestran un
/// estado vacío honesto en vez de dejar la carga colgada.
const MCP_BUDGET_SECS: u64 = 5;
/// TTL de la caché del resultado completo (30 s).
///
/// Evita repetir las 4 consultas al MCP en cada carga de la página (el
/// cliente MCP ya cachea 180 s, pero la página hace un handshake por carga).
const CACHE_TTL_SECS: u64 = 30;
/// Cuántos elementos mostramos como muestra en listas largas.
const LIST_LIMIT: usize = 12;

/// Motivo honesto mostrado cuando el MCP no responde dentro del presupuesto.
const MOTIVO_AGOTADO: &str = "Mikareno no responde ahora";

/// Marca los errores causados por el agotamiento del presupuesto global.
///
/// Distingue "el MCP no respondió a tiempo" (estado vacío honesto) de un
/// fallo técnico del tool (mensaje + detalle plegado).
const MARCA_AGOTADO: &str = "MCP-AGOTADO";

/// Mensaje honesto cuando el MCP no está disponible (no hay banner aquí: el
/// banner del shell ya lo dice y trae el único botón "Reintentar").
const MSG_SIN_CONEXION: &str =
    "Mikareno no está conectado: no se pudo consultar ningún dato de vida escolar. \
Verifica que el MCP esté corriendo en localhost:8080 y vuelve a reintentar.";

/// Secciones de la página: `(id estable para HTMX, título visible, tools MCP)`.
///
/// El `id` es la única clave aceptada en `?partial=` (nada más se interpola
/// en el HTML), así que un parámetro manipulado no puede inyectar markup.
const SECTIONS: &[(&str, &str, &[&str])] = &[
    ("asistencia", "Asistencia", &["get_asistencia"]),
    (
        "puntualidad",
        "Puntualidad",
        &["get_semaforo", "get_asistencia"],
    ),
    ("uniforme", "Uniforme", &["get_semaforo"]),
    ("comportamiento", "Comportamiento", &["get_semaforo"]),
    ("tareas", "Tareas", &["get_tareas"]),
    ("biblioteca", "Biblioteca", &["get_biblioteca"]),
];

/// Palabras clave de clasificación (se buscan en los VALORES que envió el
/// servidor; no se inventa ninguna categoría que Mikareno no reporte).
const KW_PUNTUALIDAD: &[&str] = &["tardanz", "puntual", "retraso", "demora", "late"];
const KW_UNIFORME: &[&str] = &["uniforme", "uniform"];
const KW_CONDUCTA: &[&str] = &["conducta", "comportamiento", "behavior", "disciplina"];

// ── extractores defensivos de serde_json::Value ─────────────────────────
// Intentan varias claves (snake_case / camelCase / alternativas en español)
// y nunca hacen panic: si nada coincide devuelven el valor por defecto.

fn v_str(v: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(f) = v.get(*k) {
            match f {
                Value::String(s) => return s.to_string(),
                Value::Number(n) => return n.to_string(),
                Value::Object(o) => {
                    // Un campo anidado con nombre propio: se usa su etiqueta.
                    for sub in [
                        "name",
                        "nombre",
                        "title",
                        "label",
                        "descripcion",
                        "description",
                        "value",
                        "valor",
                    ] {
                        if let Some(Value::String(s)) = o.get(sub) {
                            return s.clone();
                        }
                    }
                }
                _ => {}
            }
        }
    }
    String::new()
}

fn v_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0)
}

/// Extrae un arreglo como `Vec<Value>` clonado (evita referencias a temporales).
fn v_arr(v: &Value, key: &str) -> Vec<Value> {
    v.get(key)
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Extrae un sub-objeto como `Value` propietario (null si no existe).
fn v_obj(v: &Value, key: &str) -> Value {
    v.get(key).cloned().unwrap_or(Value::Null)
}

/// Id canónico de una sección (`None` si no existe) — evita interpolar
/// parámetros del usuario en ids de HTML.
fn canonical_section(section: &str) -> Option<&'static str> {
    SECTIONS
        .iter()
        .find(|(id, _, _)| *id == section)
        .map(|(id, _, _)| *id)
}

/// Texto plano con todos los VALORES escalares de un JSON (sin claves), en
/// minúsculas. Sirve para clasificar registros por lo que el servidor dijo
/// ("…Uniforme…", "…tardanza…"), nunca para inventar contenido.
fn texto_plano(v: &Value) -> String {
    fn rec(v: &Value, buf: &mut String, depth: usize) {
        if depth > 4 {
            return;
        }
        match v {
            Value::String(s) => {
                buf.push_str(s);
                buf.push(' ');
            }
            Value::Number(n) => {
                buf.push_str(&n.to_string());
                buf.push(' ');
            }
            Value::Bool(b) => {
                buf.push_str(if *b { "true " } else { "false " });
            }
            Value::Array(a) => {
                for x in a {
                    rec(x, buf, depth + 1);
                }
            }
            Value::Object(o) => {
                for x in o.values() {
                    rec(x, buf, depth + 1);
                }
            }
            Value::Null => {}
        }
    }
    let mut buf = String::new();
    rec(v, &mut buf, 0);
    buf.to_lowercase()
}

/// Aplana un payload de registros a una lista de objetos.
///
/// Acepta un array directo o un dict que contenga el array (bajo cualquier
/// clave) y, en su defecto, dicts anidados un nivel.
fn collect_registros(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.iter().filter(|x| x.is_object()).cloned().collect(),
        Value::Object(o) => {
            let mut out: Vec<Value> = Vec::new();
            for val in o.values() {
                if let Value::Array(a) = val {
                    out.extend(a.iter().filter(|x| x.is_object()).cloned());
                }
            }
            if !out.is_empty() {
                return out;
            }
            for val in o.values() {
                if let Value::Object(inner) = val {
                    for x in inner.values() {
                        if let Value::Array(a) = x {
                            out.extend(a.iter().filter(|y| y.is_object()).cloned());
                        }
                    }
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

/// ¿El registro pertenece a la categoría indicada? Búsqueda por palabra clave
/// sobre los valores reales del registro.
fn registro_es(reg: &Value, claves: &[&str]) -> bool {
    let t = texto_plano(reg);
    claves.iter().any(|k| t.contains(k))
}

// ── modelos parseados (datos reales) ──────────────────────────────────────

/// Vista de un registro real (registro de conducta, asistencia, tipo…).
#[derive(Debug, Default, Clone)]
struct Registro {
    fecha: String,
    tipo: String,
    materia: String,
    detalle: String,
}

fn registro_desde(v: &Value) -> Registro {
    Registro {
        fecha: v_str(
            v,
            &[
                "date",
                "fecha",
                "createdDate",
                "recordDate",
                "day",
                "fechaRegistro",
                "registrationDate",
            ],
        ),
        tipo: v_str(
            v,
            &[
                "recordTypeName",
                "typeName",
                "recordType",
                "type",
                "tipo",
                "category",
                "categoria",
                "name",
                "nombre",
                "title",
            ],
        ),
        materia: v_str(
            v,
            &[
                "subject",
                "course",
                "materia",
                "courseName",
                "subjectName",
                "asignatura",
                "area",
            ],
        ),
        detalle: v_str(
            v,
            &[
                "description",
                "descripcion",
                "detail",
                "detalle",
                "observation",
                "observaciones",
                "comment",
                "comments",
                "message",
                "motivo",
                "reason",
                "justification",
                "justificacion",
            ],
        ),
    }
}

/// Un registro de asistencia dentro de un día (`attendaceMap`).
#[derive(Debug, Default, Clone)]
struct AsistenciaReg {
    fecha: String,
    codigo: String,
    detalle: String,
    hora: String,
    observaciones: String,
}

/// Estado de una sección de la página.
#[derive(Debug, Default, Clone)]
struct Seccion {
    /// Cuerpo de la sección: datos reales o estado vacío honesto.
    html: String,
    /// Detalle técnico de un fallo (se muestra plegado, nunca en grande).
    error: Option<String>,
    /// `true` si la sección mostró datos reales del estudiante.
    con_datos: bool,
    /// Conteo de elementos (badge del acordeón), si aplica.
    conteo: Option<u64>,
}

impl Seccion {
    /// Estado vacío honesto: nombra la sección y el motivo de la falta de datos.
    fn vacia(titulo: &str, motivo: &str) -> Seccion {
        Seccion {
            html: format!(
                r#"<p class="font-body-sm text-body-sm text-on-surface-variant">Sin datos de {titulo}: {motivo}</p>"#,
                titulo = templates::html_escape(titulo),
                motivo = templates::html_escape(motivo),
            ),
            error: None,
            con_datos: false,
            conteo: None,
        }
    }

    /// Fallo de tool: frase corta + error técnico plegado.
    fn fallo(titulo: &str, err: &str) -> Seccion {
        Seccion {
            html: format!(
                r#"<p class="font-body-sm text-body-sm text-on-surface-variant">No se pudieron obtener los datos de {titulo}.</p>"#,
                titulo = templates::html_escape(titulo),
            ),
            error: Some(err.to_string()),
            con_datos: false,
            conteo: None,
        }
    }

    /// Estado vacío honesto con detalle técnico plegado (p. ej. cuando se
    /// agotó el presupuesto global: el cuerpo dice que Mikareno no
    /// responde y el motivo técnico queda en un `<details>`).
    fn vacia_con_error(titulo: &str, motivo: &str, err: &str) -> Seccion {
        Seccion {
            html: format!(
                r#"<p class="font-body-sm text-body-sm text-on-surface-variant">Sin datos de {titulo}: {motivo}</p>"#,
                titulo = templates::html_escape(titulo),
                motivo = templates::html_escape(motivo),
            ),
            error: Some(err.to_string()),
            con_datos: false,
            conteo: None,
        }
    }
}

/// Estado de una sección a partir del fallo de su tool.
///
/// Un error marcado como agotado del presupuesto se muestra como estado
/// vacío honesto ("Mikareno no responde ahora"); cualquier otro fallo es
/// técnico y se resume en una frase con el detalle plegado.
fn seccion_error(titulo: &str, err: &str) -> Seccion {
    if err.starts_with(MARCA_AGOTADO) {
        Seccion::vacia_con_error(titulo, MOTIVO_AGOTADO, err)
    } else {
        Seccion::fallo(titulo, err)
    }
}

/// Error de un tool que no respondió dentro del presupuesto global.
fn error_agotado(tool: &str) -> String {
    format!("{MARCA_AGOTADO}: {tool} no respondió dentro de {MCP_BUDGET_SECS} s")
}

/// Payloads crudos de los tools leídos (con su error técnico si lo hubo).
#[derive(Debug, Default)]
struct Crudo {
    asistencia: Option<Result<Value, String>>,
    semaforo: Option<Result<Value, String>>,
    tareas: Option<Result<Value, String>>,
    biblioteca: Option<Result<Value, String>>,
    /// Lista tipada de tareas (adapter compartido del cliente) y su error.
    tareas_tipadas: Option<Result<Vec<Tarea>, String>>,
}

/// Datos de toda la página.
#[derive(Debug, Default, Clone)]
struct LifeData {
    connected: bool,
    /// Mensaje informativo cuando el MCP no pudo inicializarse.
    disconnected_msg: String,
    /// Una entrada por sección, en el mismo orden que `SECTIONS`.
    secciones: Vec<Seccion>,
}

impl LifeData {
    /// Página sin MCP: todas las secciones con estado vacío honesto.
    fn desconectado(msg: &str) -> LifeData {
        LifeData {
            connected: false,
            disconnected_msg: msg.to_string(),
            secciones: SECTIONS
                .iter()
                .map(|(_id, titulo, _)| {
                    Seccion::vacia(titulo, "el MCP de Mikareno no está conectado")
                })
                .collect(),
        }
    }

    /// El MCP no respondió dentro del presupuesto global.
    ///
    /// No es "no está configurado": el aviso dice que Mikareno *no responde
    /// ahora*, y cada sección queda con su estado vacío honesto.
    fn agotado() -> LifeData {
        LifeData {
            connected: false,
            disconnected_msg: format!(
                "{motivo}: no se pudo consultar el MCP dentro de {secs} s.",
                motivo = MOTIVO_AGOTADO,
                secs = MCP_BUDGET_SECS
            ),
            secciones: SECTIONS
                .iter()
                .map(|(_id, titulo, _)| Seccion::vacia(titulo, MOTIVO_AGOTADO))
                .collect(),
        }
    }

    /// Construye las 6 secciones a partir de los payloads crudos.
    fn desde_crudo(crudo: Crudo) -> LifeData {
        LifeData {
            connected: true,
            disconnected_msg: String::new(),
            secciones: vec![
                seccion_asistencia(crudo.asistencia.as_ref()),
                seccion_puntualidad(crudo.semaforo.as_ref(), crudo.asistencia.as_ref()),
                seccion_uniforme(crudo.semaforo.as_ref()),
                seccion_comportamiento(crudo.semaforo.as_ref()),
                seccion_tareas(crudo.tareas.as_ref(), crudo.tareas_tipadas.as_ref()),
                seccion_biblioteca(crudo.biblioteca.as_ref()),
            ],
        }
    }

    /// Sección por id canónico (`None` si el id no existe).
    fn seccion(&self, id: &str) -> Option<&Seccion> {
        SECTIONS
            .iter()
            .position(|(s, _, _)| *s == id)
            .and_then(|i| self.secciones.get(i))
    }
}

/// Error del propio server (p. ej. `{"error": "...", "hint": "..."}`).
fn cap_error(v: &Value) -> Option<String> {
    v.get("error")
        .and_then(|e| e.as_str())
        .or_else(|| v.get("hint").and_then(|h| h.as_str()))
        .map(|s| s.to_string())
}

// ── fetch (con límites de tiempo) ────────────────────────────────────────

/// Deadline único de la fase MCP.
///
/// Todas las consultas de una carga comparten este mismo instante: es un
/// plazo global, no un plazo por tool. Por eso las tools se piden en
/// paralelo y ninguna espera a otra.
fn mcp_deadline() -> time::Instant {
    time::Instant::now() + Duration::from_secs(MCP_BUDGET_SECS)
}

/// Llama a un tool read-only y devuelve su payload crudo o el error técnico.
async fn call_tool_raw(client: &MikarenoClient, tool: &str) -> Result<Value, String> {
    let r = match tool {
        // Lector tipado del adapter de biblioteca.
        "get_biblioteca" => client.biblioteca().await,
        _ => client.call_tool(tool, json!({})).await,
    };
    r.map_err(|e| e.to_string())
}

/// Consulta un tool dentro del deadline global.
///
/// Si el plazo se agota, la consulta se cancela y se devuelve el error
/// marcado como agotado: su sección se mostrará con estado vacío honesto.
async fn call_bounded(
    client: &MikarenoClient,
    deadline: time::Instant,
    tool: &'static str,
) -> Result<Value, String> {
    match time::timeout_at(deadline, call_tool_raw(client, tool)).await {
        Ok(r) => r,
        Err(_) => Err(error_agotado(tool)),
    }
}

/// Consulta un tool sólo si la sección lo necesita (`None` si no).
async fn maybe_call(
    client: &MikarenoClient,
    deadline: time::Instant,
    tools: &[&str],
    tool: &'static str,
) -> Option<Result<Value, String>> {
    if !tools.contains(&tool) {
        return None;
    }
    Some(call_bounded(client, deadline, tool).await)
}

/// Obtiene los 4 tools en PARALELO, con un único deadline global.
///
/// El handshake se hace primero (dentro del mismo presupuesto) porque sin
/// sesión no hay consultas; después las 4 tools salen a la vez. Ninguna
/// espera a otra: el peor caso es [`MCP_BUDGET_SECS`] en total, no la suma
/// de los timeouts de cada una.
async fn fetch_all(client: &MikarenoClient) -> LifeData {
    let deadline = mcp_deadline();

    match time::timeout_at(deadline, client.initialize()).await {
        Ok(Ok(())) => {}
        // El MCP respondió con error: no está colgado, no hay sesión.
        Ok(Err(_)) => return LifeData::desconectado(MSG_SIN_CONEXION),
        // No respondió a tiempo: estado vacío honesto.
        Err(_) => return LifeData::agotado(),
    }

    let c = client.clone();
    let (a, s, t, b) = tokio::join!(
        call_bounded(&c, deadline, "get_asistencia"),
        call_bounded(&c, deadline, "get_semaforo"),
        call_bounded(&c, deadline, "get_tareas"),
        call_bounded(&c, deadline, "get_biblioteca"),
    );

    // Lista tipada de tareas: el mismo tool ya está en la caché del cliente,
    // así que no hay I/O extra (mismo adapter que usa el planificador).
    let tareas_tipadas = if t.is_ok() {
        match time::timeout_at(deadline, client.tareas()).await {
            Ok(r) => Some(r.map_err(|e| e.to_string())),
            Err(_) => None,
        }
    } else {
        None
    };

    LifeData::desde_crudo(Crudo {
        asistencia: Some(a),
        semaforo: Some(s),
        tareas: Some(t),
        biblioteca: Some(b),
        tareas_tipadas,
    })
}

/// Obtiene en PARALELO sólo los tools que necesita una sección (para
/// `?partial=`).
///
/// Nada se pide en cascada: los tools de la sección salen a la vez y todos
/// comparten el deadline global, así que una sección nunca encadena los
/// timeouts de varias consultas.
async fn fetch_one(client: &MikarenoClient, section: &str) -> LifeData {
    let Some((_id, _titulo, tools)) = SECTIONS.iter().find(|(s, _, _)| *s == section) else {
        return LifeData::desconectado(MSG_SIN_CONEXION);
    };
    let deadline = mcp_deadline();

    match time::timeout_at(deadline, client.initialize()).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => return LifeData::desconectado(MSG_SIN_CONEXION),
        Err(_) => return LifeData::agotado(),
    }

    let c = client.clone();
    let (a, s, t, b) = tokio::join!(
        maybe_call(&c, deadline, tools, "get_asistencia"),
        maybe_call(&c, deadline, tools, "get_semaforo"),
        maybe_call(&c, deadline, tools, "get_tareas"),
        maybe_call(&c, deadline, tools, "get_biblioteca"),
    );

    let tareas_tipadas = match &t {
        Some(Ok(_)) => match time::timeout_at(deadline, client.tareas()).await {
            Ok(r) => Some(r.map_err(|e| e.to_string())),
            Err(_) => None,
        },
        _ => None,
    };

    LifeData::desde_crudo(Crudo {
        asistencia: a,
        semaforo: s,
        tareas: t,
        biblioteca: b,
        tareas_tipadas,
    })
}

// ── caché del resultado completo (30 s) ───────────────────────────────────

/// Caché del resultado completo de la página.
///
/// `Mutex` estático: la entrada es `(vencimiento, LifeData)`. Sin ella cada
/// carga repite las consultas al MCP (y su handshake) aunque la respuesta
/// no haya cambiado.
static CACHE: Mutex<Option<(Instant, LifeData)>> = Mutex::new(None);

/// Resultado cacheado si sigue vigente (TTL de [`CACHE_TTL_SECS`]).
fn cache_leer() -> Option<LifeData> {
    let guard = CACHE.lock().ok()?;
    match guard.as_ref() {
        Some((vence, life)) if Instant::now() < *vence => Some(life.clone()),
        _ => None,
    }
}

/// Guarda el resultado con su vencimiento.
fn cache_guardar(life: &LifeData) {
    let Ok(mut guard) = CACHE.lock() else {
        return;
    };
    *guard = Some((
        Instant::now() + Duration::from_secs(CACHE_TTL_SECS),
        life.clone(),
    ));
}

// ── render: piezas compartidas ───────────────────────────────────────────

/// Estado vacío honesto (sin datos) que nombra la sección.
fn vacio_html(seccion: &str, motivo: &str) -> String {
    format!(
        r#"<p class="font-body-sm text-body-sm text-on-surface-variant">Sin datos de {seccion}: {motivo}</p>"#,
        seccion = templates::html_escape(seccion),
        motivo = templates::html_escape(motivo),
    )
}

/// Detalle técnico plegable: el error crudo nunca se muestra en grande.
fn detalle_tecnico_html(err: &str) -> String {
    format!(
        r#"<details class="mt-space-sm">
            <summary class="font-label-sm text-[10px] uppercase tracking-wide text-on-surface-variant cursor-pointer select-none">Detalle técnico</summary>
            <pre class="mt-space-xs p-space-sm rounded-lg bg-surface-container-highest border border-outline text-[11px] text-on-surface-variant font-code-inline whitespace-pre-wrap break-words">{err}</pre>
        </details>"#,
        err = templates::html_escape(err)
    )
}

/// Cuerpo de la sección: contenido + detalle técnico plegado (si hay).
fn seccion_html(s: &Seccion) -> String {
    match &s.error {
        Some(e) => format!("{}{}", s.html, detalle_tecnico_html(e)),
        None => s.html.clone(),
    }
}

/// Fila clave → valor de un objeto JSON (sólo lo que envió el servidor).
fn kv_row(k: &str, valor: &str) -> String {
    format!(
        r#"<li class="flex items-start gap-space-sm py-space-xs">
            <span class="font-label-sm text-[10px] uppercase text-on-surface-variant shrink-0">{k}</span>
            <span class="font-body-sm text-body-sm text-on-surface break-words">{v}</span>
        </li>"#,
        k = templates::html_escape(k),
        v = templates::html_escape(valor),
    )
}

/// Renderiza un objeto JSON como lista de campos (texto, número, booleano,
/// array de textos o conteo de elementos). Nunca inventa claves.
fn render_objeto_kv(v: &Value, depth: usize) -> String {
    let Some(obj) = v.as_object() else {
        return String::new();
    };
    if obj.is_empty() {
        return String::new();
    }
    let mut rows: Vec<String> = Vec::new();
    for (k, val) in obj {
        match val {
            Value::String(s) => {
                if !s.trim().is_empty() {
                    rows.push(kv_row(k, s));
                }
            }
            Value::Number(n) => rows.push(kv_row(k, &n.to_string())),
            Value::Bool(b) => rows.push(kv_row(k, if *b { "sí" } else { "no" })),
            Value::Array(a) => {
                let textos: Vec<&str> = a.iter().filter_map(|x| x.as_str()).collect();
                if !textos.is_empty() {
                    rows.push(kv_row(k, &textos.join(", ")));
                } else if a.is_empty() {
                    rows.push(kv_row(k, "sin elementos"));
                } else {
                    rows.push(kv_row(k, &format!("{} elementos", a.len())));
                }
            }
            Value::Object(o) => {
                if o.is_empty() {
                    rows.push(kv_row(k, "sin datos"));
                } else if depth < 2 {
                    let sub = render_objeto_kv(val, depth + 1);
                    if !sub.is_empty() {
                        rows.push(format!(
                            r#"<li class="py-space-xs"><span class="font-label-sm text-[10px] uppercase text-on-surface-variant">{k}</span>{sub}</li>"#,
                            k = templates::html_escape(k),
                            sub = sub
                        ));
                    }
                } else {
                    rows.push(kv_row(k, &format!("{} campos", o.len())));
                }
            }
            Value::Null => {}
        }
    }
    if rows.is_empty() {
        String::new()
    } else {
        format!(
            r#"<ul class="list-none divide-y divide-outline/30">{}</ul>"#,
            rows.join("")
        )
    }
}

/// Lista de registros reales (fecha · tipo · materia · detalle).
fn render_registros(items: &[Registro]) -> String {
    let rows = items
        .iter()
        .take(LIST_LIMIT)
        .map(|r| {
            let fecha = if r.fecha.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<span class="font-code-inline text-[10px] text-on-surface-variant">{}</span>"#,
                    templates::html_escape(&r.fecha)
                )
            };
            let tipo = if r.tipo.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<span class="font-code-inline text-[10px] text-primary-container">{}</span>"#,
                    templates::html_escape(&r.tipo)
                )
            };
            let materia = if r.materia.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<span class="font-label-sm text-[10px] text-outline">{}</span>"#,
                    templates::html_escape(&r.materia)
                )
            };
            let detalle = if r.detalle.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<div class="font-body-sm text-body-sm text-on-surface-variant break-words">{}</div>"#,
                    templates::html_escape(&r.detalle)
                )
            };
            format!(
                r#"<li class="py-space-xs"><div class="flex items-center gap-space-sm flex-wrap">{fecha}{tipo}{materia}</div>{detalle}</li>"#,
                fecha = fecha,
                tipo = tipo,
                materia = materia,
                detalle = detalle,
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"<ul class="list-none divide-y divide-outline/30">{}</ul>"#,
        rows
    )
}

// ── render: secciones ────────────────────────────────────────────────────

/// Asistencia: registro semanal por día (`get_asistencia → resumen_semanal`).
fn seccion_asistencia(r: Option<&Result<Value, String>>) -> Seccion {
    let v = match r {
        Some(Ok(v)) => v,
        Some(Err(e)) => return seccion_error("Asistencia", e),
        None => return Seccion::vacia("Asistencia", "no se consultó el tool get_asistencia"),
    };
    if let Some(err) = cap_error(v) {
        return Seccion::fallo("Asistencia", &err);
    }

    let mut dias: Vec<(String, Vec<AsistenciaReg>)> = Vec::new();
    if let Some(obj) = v.get("resumen_semanal").and_then(|x| x.as_object()) {
        for (dia, regs) in obj {
            let items: Vec<AsistenciaReg> = regs
                .as_array()
                .map(|a| a.as_slice())
                .unwrap_or(&[])
                .iter()
                .filter_map(|r| {
                    if !r.is_object() {
                        return None;
                    }
                    Some(AsistenciaReg {
                        fecha: v_str(r, &["fecha", "date", "createdDate"]),
                        codigo: v_str(r, &["codigo", "code", "tipo", "type"]),
                        detalle: v_str(r, &["detalle", "detail", "description", "descripcion"]),
                        hora: v_str(
                            r,
                            &[
                                "hora_slot",
                                "slotOrdinal",
                                "hora",
                                "hour",
                                "periodo",
                                "period",
                            ],
                        ),
                        observaciones: v_str(
                            r,
                            &["observaciones", "observations", "obs", "nota", "notes"],
                        ),
                    })
                })
                .collect();
            dias.push((dia.clone(), items));
        }
    }
    dias.sort_by(|a, b| a.0.cmp(&b.0));

    let total_server = v.get("total_registros_semana").and_then(|x| x.as_u64());
    let total_real: usize = dias.iter().map(|(_, r)| r.len()).sum();
    if dias.is_empty() && total_real == 0 && total_server.unwrap_or(0) == 0 {
        return Seccion::vacia(
            "Asistencia",
            "Mikareno no devolvió registros de asistencia en esta carga",
        );
    }

    let mut parts: Vec<String> = Vec::new();
    let total = total_server.unwrap_or(total_real as u64);
    parts.push(format!(
        r#"<div class="font-code-inline text-[11px] text-on-surface-variant">Registros de asistencia de la semana: {total}</div>"#,
        total = total
    ));
    if let Some(d) = v
        .get("detalle")
        .and_then(|d| d.get("max_attendance_days"))
        .and_then(|x| x.as_u64())
    {
        parts.push(format!(
            r#"<div class="font-code-inline text-[11px] text-on-surface-variant">Días de asistencia configurados por Mikareno: {d}</div>"#,
            d = d
        ));
    }

    for (dia, regs) in &dias {
        if regs.is_empty() {
            parts.push(format!(
                r#"<div><div class="font-headline-sm text-headline-sm text-on-surface mb-space-xs">{dia}</div>
                <p class="font-body-sm text-body-sm text-on-surface-variant">Sin registros de asistencia para este día.</p></div>"#,
                dia = templates::html_escape(dia)
            ));
            continue;
        }
        let items = regs
            .iter()
            .map(|r| {
                let codigo = if r.codigo.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<span class="font-code-inline text-[10px] text-primary-container">{}</span>"#,
                        templates::html_escape(&r.codigo)
                    )
                };
                let hora = if r.hora.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<span class="font-code-inline text-[10px] text-outline" title="bloque u horario del registro">{}</span>"#,
                        templates::html_escape(&r.hora)
                    )
                };
                let fecha = if r.fecha.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<span class="font-code-inline text-[10px] text-on-surface-variant">{}</span>"#,
                        templates::html_escape(&r.fecha)
                    )
                };
                let detalle = if r.detalle.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<span class="font-body-sm text-body-sm text-on-surface">{}</span>"#,
                        templates::html_escape(&r.detalle)
                    )
                };
                let obs = if r.observaciones.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<div class="font-body-sm text-body-sm text-on-surface-variant break-words">{}</div>"#,
                        templates::html_escape(&r.observaciones)
                    )
                };
                format!(
                    r#"<li class="py-space-xs"><div class="flex items-center gap-space-sm flex-wrap">{fecha}{hora}{codigo}</div><div>{detalle}</div>{obs}</li>"#,
                    fecha = fecha,
                    hora = hora,
                    codigo = codigo,
                    detalle = detalle,
                    obs = obs,
                )
            })
            .collect::<Vec<_>>()
            .join("");
        parts.push(format!(
            r#"<div><div class="font-headline-sm text-headline-sm text-primary-container mb-space-xs">{dia}</div>
            <ul class="list-none divide-y divide-outline/30 pl-space-md border-l border-outline/30 ml-px">{items}</ul></div>"#,
            dia = templates::html_escape(dia),
            items = items
        ));
    }

    Seccion {
        html: format!(r#"<div class="space-y-space-md">{}</div>"#, parts.join("")),
        error: None,
        con_datos: true,
        conteo: Some(total),
    }
}

/// Registros candidatos de `get_semaforo` (registros del estudiante).
fn registros_semaforo(v: &Value) -> Vec<Value> {
    collect_registros(&v_obj(v, "registros_estudiante"))
}

/// Registros de asistencia por materia/período (`get_asistencia → detalle`).
fn registros_asistencia_detalle(v: &Value) -> Vec<Value> {
    let detalle = v_obj(v, "detalle");
    collect_registros(&v_obj(&detalle, "registros"))
}

/// Puntualidad: registros que Mikareno etiqueta como tardanza/puntualidad.
fn seccion_puntualidad(
    semaforo: Option<&Result<Value, String>>,
    asistencia: Option<&Result<Value, String>>,
) -> Seccion {
    let sv = match semaforo {
        Some(Ok(v)) => v,
        Some(Err(e)) => return seccion_error("Puntualidad", e),
        None => return Seccion::vacia("Puntualidad", "no se consultó el tool get_semaforo"),
    };
    if let Some(err) = cap_error(sv) {
        return Seccion::fallo("Puntualidad", &err);
    }

    let candidatos_semaforo = registros_semaforo(sv);
    let candidatos_asistencia = match asistencia {
        Some(Ok(av)) => registros_asistencia_detalle(av),
        _ => Vec::new(),
    };
    let revisados = candidatos_semaforo.len() + candidatos_asistencia.len();

    let mut matches: Vec<Registro> = Vec::new();
    for reg in candidatos_semaforo
        .iter()
        .chain(candidatos_asistencia.iter())
    {
        if registro_es(reg, KW_PUNTUALIDAD) {
            matches.push(registro_desde(reg));
        }
    }

    if matches.is_empty() {
        let motivo = if revisados == 0 {
            "Mikareno no devolvió registros de asistencia ni de conducta en esta carga (no hay nada que revisar)".to_string()
        } else {
            format!(
                "Mikareno no devolvió registros de puntualidad (0 de {revisados} registros revisados)"
            )
        };
        return Seccion {
            html: vacio_html("Puntualidad", &motivo),
            error: None,
            con_datos: false,
            conteo: None,
        };
    }

    let html = format!(
        r#"<div class="space-y-space-sm">
            <div class="font-code-inline text-[11px] text-on-surface-variant">Registros de puntualidad: {n}</div>
            {lista}
        </div>"#,
        n = matches.len(),
        lista = render_registros(&matches),
    );
    Seccion {
        html,
        error: None,
        con_datos: true,
        conteo: Some(matches.len() as u64),
    }
}

/// Uniforme: registros que Mikareno etiqueta como uniforme.
fn seccion_uniforme(semaforo: Option<&Result<Value, String>>) -> Seccion {
    let sv = match semaforo {
        Some(Ok(v)) => v,
        Some(Err(e)) => return seccion_error("Uniforme", e),
        None => return Seccion::vacia("Uniforme", "no se consultó el tool get_semaforo"),
    };
    if let Some(err) = cap_error(sv) {
        return Seccion::fallo("Uniforme", &err);
    }

    let candidatos = registros_semaforo(sv);
    let matches: Vec<Registro> = candidatos
        .iter()
        .filter(|reg| registro_es(reg, KW_UNIFORME))
        .map(registro_desde)
        .collect();

    // Catálogo de tipos de registro que Mikareno publica: si menciona
    // "uniforme", lo decimos (dato real) aunque el estudiante no tenga
    // registros de ese tipo.
    let catalogo = v_arr(sv, "tipos_registro");
    let catalogo_texto = texto_plano(&Value::Array(catalogo.clone()));

    if matches.is_empty() {
        let motivo = if catalogo_texto.contains("uniforme") {
            "Mikareno publica el tipo de registro «uniforme» en su catálogo, pero no hay registros de uniforme del estudiante en esta carga".to_string()
        } else if candidatos.is_empty() {
            "Mikareno no devolvió registros de conducta en esta carga (no hay nada que revisar)"
                .to_string()
        } else {
            format!(
                "Mikareno no devolvió registros de uniforme (0 de {n} registros revisados)",
                n = candidatos.len()
            )
        };
        return Seccion {
            html: vacio_html("Uniforme", &motivo),
            error: None,
            con_datos: false,
            conteo: None,
        };
    }

    let html = format!(
        r#"<div class="space-y-space-sm">
            <div class="font-code-inline text-[11px] text-on-surface-variant">Registros de uniforme: {n}</div>
            {lista}
        </div>"#,
        n = matches.len(),
        lista = render_registros(&matches),
    );
    Seccion {
        html,
        error: None,
        con_datos: true,
        conteo: Some(matches.len() as u64),
    }
}

/// Comportamiento (behavior): estadísticas e indicadores de conducta.
fn seccion_comportamiento(semaforo: Option<&Result<Value, String>>) -> Seccion {
    let sv = match semaforo {
        Some(Ok(v)) => v,
        Some(Err(e)) => return seccion_error("Comportamiento", e),
        None => return Seccion::vacia("Comportamiento", "no se consultó el tool get_semaforo"),
    };
    if let Some(err) = cap_error(sv) {
        return Seccion::fallo("Comportamiento", &err);
    }

    let stats = v_obj(sv, "estadisticas_comportamiento");
    let stats_html = render_objeto_kv(&stats, 0);

    let indicadores = v_arr(sv, "indicadores_por_fecha");
    let indicadores_html = if indicadores.is_empty() {
        String::new()
    } else {
        let rows = indicadores
            .iter()
            .take(LIST_LIMIT)
            .map(|ind| {
                let fecha = v_str(ind, &["fecha", "date"]);
                let indicador = v_str(ind, &["indicador", "indicator", "color"]);
                let observacion = v_str(ind, &["observacion", "observation", "observaciones"]);
                let (dot, txt) = semaforo_color(&indicador);
                format!(
                    r#"<li class="flex items-center gap-space-sm py-space-xs">
                        <span class="w-2.5 h-2.5 rounded-full {dot}"></span>
                        <span class="font-code-inline text-[11px] text-on-surface-variant">{fecha}</span>
                        <span class="font-label-sm text-[10px] {txt} font-semibold">{indicador}</span>
                        <span class="ml-auto text-right font-body-sm text-body-sm text-on-surface-variant">{obs}</span>
                    </li>"#,
                    dot = dot,
                    txt = txt,
                    fecha = templates::html_escape(&fecha),
                    indicador = templates::html_escape(&indicador),
                    obs = templates::html_escape(&observacion),
                )
            })
            .collect::<Vec<_>>()
            .join("");
        format!(
            r#"<div><span class="font-label-sm text-[10px] uppercase text-on-surface-variant">Indicadores de conducta por fecha</span>
            <ul class="list-none divide-y divide-outline/30 mt-space-xs">{rows}</ul></div>"#,
            rows = rows
        )
    };

    // Registros de conducta del estudiante (mismo origen que Uniforme).
    let conducta: Vec<Registro> = registros_semaforo(sv)
        .iter()
        .filter(|reg| registro_es(reg, KW_CONDUCTA))
        .map(registro_desde)
        .collect();
    let conducta_html = if conducta.is_empty() {
        String::new()
    } else {
        format!(
            r#"<div><span class="font-label-sm text-[10px] uppercase text-on-surface-variant">Registros de conducta</span>
            {lista}</div>"#,
            lista = render_registros(&conducta)
        )
    };

    if stats_html.is_empty() && indicadores_html.is_empty() && conducta_html.is_empty() {
        return Seccion::vacia(
            "Comportamiento",
            "Mikareno no devolvió estadísticas ni indicadores de conducta en esta carga",
        );
    }

    let con_indicador = sv
        .get("con_indicador")
        .and_then(|x| x.as_u64())
        .unwrap_or(indicadores.len() as u64);
    let html = format!(
        r#"<div class="space-y-space-md">
            <div class="font-code-inline text-[11px] text-on-surface-variant">Registros con indicador de conducta: {n}</div>
            {stats}
            {indicadores}
            {conducta}
        </div>"#,
        n = con_indicador,
        stats = stats_html,
        indicadores = indicadores_html,
        conducta = conducta_html,
    );
    Seccion {
        html,
        error: None,
        con_datos: true,
        conteo: Some(con_indicador),
    }
}

/// Devuelve (clase bg, clase fg) según el color del indicador.
fn semaforo_color(indicador: &str) -> (&'static str, &'static str) {
    let l = indicador.to_lowercase();
    if l.contains("rojo") || l.contains("red") || l == "r" {
        ("bg-red-400", "text-red-400")
    } else if l.contains("amarillo") || l.contains("yellow") || l.contains("naranja") || l == "a" {
        ("bg-yellow-400", "text-yellow-400")
    } else if l.contains("verde") || l.contains("green") || l == "v" {
        ("bg-green-400", "text-green-400")
    } else {
        ("bg-outline", "text-outline")
    }
}

/// Tareas: conteos reales y lista de deberes/actividades.
fn seccion_tareas(
    tareas: Option<&Result<Value, String>>,
    tipadas: Option<&Result<Vec<Tarea>, String>>,
) -> Seccion {
    let v = match tareas {
        Some(Ok(v)) => v,
        Some(Err(e)) => return seccion_error("Tareas", e),
        None => return Seccion::vacia("Tareas", "no se consultó el tool get_tareas"),
    };
    if let Some(err) = cap_error(v) {
        return Seccion::fallo("Tareas", &err);
    }

    let resumen = v_obj(v, "resumen");
    let esta_semana = resumen
        .get("esta_semana")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let hoy = resumen.get("hoy").and_then(|x| x.as_u64()).unwrap_or(0);
    let manana = resumen.get("manana").and_then(|x| x.as_u64()).unwrap_or(0);

    let lista: Vec<Tarea> = match tipadas {
        Some(Ok(l)) => l.clone(),
        Some(Err(_)) => Vec::new(),
        None => Vec::new(),
    };
    // Si el adapter tipado no pudo leer la lista, el motivo queda plegado
    // (nunca crudo en grande) y el cuerpo sigue mostrando los conteos reales.
    let error_tecnico = match tipadas {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    };
    let total_lista = v
        .get("total_lista")
        .and_then(|x| x.as_u64())
        .unwrap_or(lista.len() as u64);

    if esta_semana == 0 && hoy == 0 && manana == 0 && lista.is_empty() {
        return Seccion::vacia(
            "Tareas",
            "Mikareno no reporta tareas pendientes ni expone lista detallada en esta carga",
        );
    }

    let mut parts: Vec<String> = Vec::new();
    parts.push(format!(
        r#"<div class="flex gap-space-md text-[11px] text-on-surface-variant font-code-inline flex-wrap">
            <span>Esta semana: {semana}</span>
            <span>Hoy: {hoy}</span>
            <span>Mañana: {manana}</span>
        </div>"#,
        semana = esta_semana,
        hoy = hoy,
        manana = manana,
    ));

    if lista.is_empty() {
        parts.push(
            r#"<p class="font-body-sm text-body-sm text-on-surface-variant">Mikareno no expone la lista detallada de tareas en esta carga (la vista Deberes/Actividades llega vacía); los conteos de arriba sí son los que reporta el MCP.</p>"#
                .to_string(),
        );
    } else {
        let extra = if lista.len() > LIST_LIMIT {
            format!(" (mostrando {})", LIST_LIMIT)
        } else {
            String::new()
        };
        let items = lista
            .iter()
            .take(LIST_LIMIT)
            .map(|t| {
                let titulo = if t.titulo.is_empty() {
                    "Tarea sin título"
                } else {
                    t.titulo.as_str()
                };
                let vencimiento = if t.vencimiento.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<span class="font-code-inline text-[10px] text-on-surface-variant">{}</span>"#,
                        templates::html_escape(&t.vencimiento)
                    )
                };
                let estado = if t.estado.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<span class="font-code-inline text-[10px] text-primary-container">{}</span>"#,
                        templates::html_escape(&t.estado)
                    )
                };
                format!(
                    r#"<li class="py-space-xs"><div class="flex items-center gap-space-sm flex-wrap"><span class="font-body-md text-body-md text-on-surface break-words">{titulo}</span>{vencimiento}{estado}</div></li>"#,
                    titulo = templates::html_escape(titulo),
                    vencimiento = vencimiento,
                    estado = estado,
                )
            })
            .collect::<Vec<_>>()
            .join("");
        parts.push(format!(
            r#"<div><span class="font-label-sm text-[10px] uppercase text-on-surface-variant">Deberes y actividades: {total}{extra}</span>
            <ul class="list-none divide-y divide-outline/30 mt-space-xs">{items}</ul></div>"#,
            total = total_lista,
            extra = extra,
            items = items
        ));
    }

    let conteo = if total_lista > 0 {
        total_lista
    } else {
        esta_semana
    };
    Seccion {
        html: format!(r#"<div class="space-y-space-sm">{}</div>"#, parts.join("")),
        error: error_tecnico,
        con_datos: true,
        conteo: Some(conteo),
    }
}

/// Biblioteca: recursos institucionales vía el adapter `parse_biblioteca`.
fn seccion_biblioteca(biblioteca: Option<&Result<Value, String>>) -> Seccion {
    let v = match biblioteca {
        Some(Ok(v)) => v,
        Some(Err(e)) => return seccion_error("Biblioteca", e),
        None => return Seccion::vacia("Biblioteca", "no se consultó el tool get_biblioteca"),
    };
    if let Some(err) = cap_error(v) {
        return Seccion::fallo("Biblioteca", &err);
    }

    // Adapter compartido (se conserva tal cual: es la fuente canónica).
    let recursos = match parse_biblioteca(v) {
        Ok(r) => r,
        Err(e) => {
            return Seccion {
                html: vacio_html(
                    "Biblioteca",
                    "Mikareno no devolvió recursos de biblioteca en esta carga",
                ),
                error: Some(e),
                con_datos: false,
                conteo: None,
            }
        }
    };

    if recursos.is_empty() {
        let mensaje = v_str(v, &["mensaje", "message"]);
        let extra = if mensaje.is_empty() {
            String::new()
        } else {
            format!(
                r#"<p class="font-body-sm text-body-sm text-on-surface-variant mt-space-xs">{}</p>"#,
                templates::html_escape(&mensaje)
            )
        };
        return Seccion {
            html: format!(
                r#"<div class="space-y-space-xs"><p class="font-body-sm text-body-sm text-on-surface-variant">Sin datos de Biblioteca: Mikareno no devolvió recursos de biblioteca en esta carga.</p>{extra}</div>"#,
                extra = extra
            ),
            error: None,
            con_datos: false,
            conteo: None,
        };
    }

    let total = {
        let t = v_u64(v, "total_recursos");
        if t > 0 {
            t
        } else {
            recursos.len() as u64
        }
    };
    let extra = if recursos.len() > LIST_LIMIT {
        format!(" (mostrando {})", LIST_LIMIT)
    } else {
        String::new()
    };
    let cards = recursos
        .iter()
        .take(LIST_LIMIT)
        .map(render_biblioteca_card)
        .collect::<Vec<_>>()
        .join("");
    Seccion {
        html: format!(
            r#"<div class="space-y-space-sm">
                <div class="font-code-inline text-[11px] text-on-surface-variant">Recursos de biblioteca: {total}{extra}</div>
                <ul class="grid grid-cols-1 sm:grid-cols-2 gap-space-md">{cards}</ul>
            </div>"#,
            total = total,
            extra = extra,
            cards = cards
        ),
        error: None,
        con_datos: true,
        conteo: Some(total),
    }
}

/// Sólo se enlazan URLs http(s): evita esquemas ejecutables en el `href`.
fn url_segura(url: &str) -> bool {
    let l = url.trim().to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// Tarjeta de un recurso de la biblioteca: título, tipo, disponibilidad,
/// descripción y enlace. Todo el texto viene escapado.
fn render_biblioteca_card(r: &Biblioteca) -> String {
    let titulo = templates::html_escape(&r.titulo);
    let tipo = if r.tipo.is_empty() {
        String::new()
    } else {
        format!(
            r#"<span class="font-code-inline text-[10px] text-on-surface-variant bg-surface-container-highest px-space-sm rounded">{}</span>"#,
            templates::html_escape(&r.tipo)
        )
    };
    // Badge sólo si el backend informa disponibilidad: si no, no se inventa.
    let badge = match r.disponible {
        Some(true) => badge_str(
            "Disponible",
            "bg-green-400",
            "text-surface-container-lowest",
        ),
        Some(false) => badge_str(
            "No disponible",
            "bg-red-400",
            "text-surface-container-lowest",
        ),
        None => String::new(),
    };
    let codigo = if r.codigo.is_empty() {
        String::new()
    } else {
        format!(
            r#"<span class="font-code-inline text-[10px] text-outline">{}</span>"#,
            templates::html_escape(&r.codigo)
        )
    };
    let descripcion = if r.descripcion.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="font-body-sm text-body-sm text-on-surface-variant break-words">{}</p>"#,
            templates::html_escape(&r.descripcion)
        )
    };
    let enlace = if r.url.is_empty() {
        String::new()
    } else if url_segura(&r.url) {
        format!(
            r#"<a href="{href}" target="_blank" rel="noopener noreferrer" class="font-code-inline text-[11px] text-primary-container hover:underline break-words">{label}</a>"#,
            href = templates::html_escape(&r.url),
            label = templates::html_escape(&r.url)
        )
    } else {
        format!(
            r#"<span class="font-code-inline text-[11px] text-on-surface-variant break-words">{}</span>"#,
            templates::html_escape(&r.url)
        )
    };
    format!(
        r#"<li class="p-space-sm rounded-lg bg-surface-container-highest border border-outline space-y-space-xs">
            <div class="font-headline-sm text-headline-sm text-on-surface break-words">{titulo}</div>
            <div class="flex items-center gap-space-sm flex-wrap">{tipo}{badge}{codigo}</div>
            {descripcion}
            {enlace}
        </li>"#,
        titulo = titulo,
        tipo = tipo,
        badge = badge,
        codigo = codigo,
        descripcion = descripcion,
        enlace = enlace,
    )
}

// ── render: acordeón, resumen y página ───────────────────────────────────

/// Contenido interior de una sección (datos reales o estado vacío honesto).
fn section_inner(section: &str, life: &LifeData) -> String {
    match life.seccion(section) {
        Some(s) => seccion_html(s),
        None => vacio_html("Sección", "la sección no existe"),
    }
}

/// Cuerpo de una sección: contenedor con id estable para HTMX `outerHTML`.
fn section_body(section: &str, life: &LifeData) -> String {
    format!(
        r#"<div id="body-{}" class="nova-life-body">{}</div>"#,
        section,
        section_inner(section, life)
    )
}

fn badge_str(label: &str, bg: &str, fg: &str) -> String {
    format!(
        r#"<span class="font-label-sm text-[10px] {} {} px-space-sm rounded-full">{}</span>"#,
        bg,
        fg,
        templates::html_escape(label)
    )
}

fn section_badge(section: &str, life: &LifeData) -> String {
    match life.seccion(section) {
        Some(s) => match s.conteo {
            Some(c) if c > 0 && s.con_datos => badge_str(
                &c.to_string(),
                "bg-primary-container/10",
                "text-primary-container",
            ),
            _ => String::new(),
        },
        None => String::new(),
    }
}

fn section_icon(section: &str) -> &'static str {
    match section {
        "asistencia" => "event_available",
        "puntualidad" => "timer",
        "uniforme" => "checkroom",
        "comportamiento" => "traffic",
        "tareas" => "assignment",
        "biblioteca" => "menu_book",
        _ => "help_outline",
    }
}

fn render_section(section: &str, life: &LifeData) -> String {
    let Some((_id, title, _tools)) = SECTIONS.iter().find(|(id, _, _)| *id == section) else {
        return String::new();
    };
    let icon = section_icon(section);
    let badge = section_badge(section, life);
    let body = section_body(section, life);
    let open = if section == SECTIONS[0].0 {
        " open"
    } else {
        ""
    };
    format!(
        r##"<details class="nova-life-card nova-life-{sid}" id="sl-{sid}"{open}>
    <summary class="nova-life-summary-row">
      <div class="flex items-center gap-space-sm">
        <span class="material-symbols-outlined text-[18px] text-primary-container">{icon}</span>
        <span class="font-headline-sm text-headline-sm text-primary-container">{title}</span>
        {badge}
      </div>
      <div class="flex items-center gap-1.5">
        <button
          type="button"
          hx-get="/dashboard/vida-escolar?partial={sid}"
          hx-target="#body-{sid}"
          hx-swap="outerHTML"
          class="p-1 rounded-lg text-outline hover:text-on-surface hover:bg-surface-container transition-colors"
          title="Actualizar sección">
          <span class="material-symbols-outlined text-[16px]">refresh</span>
        </button>
        <span class="material-symbols-outlined text-[16px] text-outline">chevron_down</span>
      </div>
    </summary>
    {body}
  </details>"##,
        sid = section,
        open = open,
        icon = icon,
        title = templates::html_escape(title),
        badge = badge,
        body = body,
    )
}

/// Resumen honesto del estado de las secciones: "N secciones con datos ·
/// M sin datos" (calculado sobre el estado real, no sobre lo esperado).
fn resumen_html(life: &LifeData) -> String {
    let total = SECTIONS.len();
    let mut con_datos = 0usize;
    let mut sin_datos: Vec<&str> = Vec::new();
    let mut con_error: Vec<&str> = Vec::new();
    for (id, titulo, _) in SECTIONS {
        match life.seccion(id) {
            Some(s) => {
                if s.con_datos {
                    con_datos += 1;
                } else {
                    sin_datos.push(titulo);
                }
                if s.error.is_some() {
                    con_error.push(titulo);
                }
            }
            None => sin_datos.push(titulo),
        }
    }
    let sin = total - con_datos;
    let extra_error = if con_error.is_empty() {
        String::new()
    } else {
        format!(" · {} con error técnico", con_error.len())
    };
    let detalle_sin = if sin_datos.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="font-body-sm text-body-sm text-on-surface-variant mt-space-xs">Sin datos: {lista}</p>"#,
            lista = templates::html_escape(&sin_datos.join(", "))
        )
    };
    let detalle_error = if con_error.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="font-body-sm text-body-sm text-on-surface-variant mt-space-xs">Con error técnico (detalle plegado en cada sección): {lista}</p>"#,
            lista = templates::html_escape(&con_error.join(", "))
        )
    };
    format!(
        r#"<div class="nova-life-overview">
            <p>{con} {con_label} con datos · {sin} sin datos{extra_error}</p>
            {detalle_sin}{detalle_error}
        </div>"#,
        con = con_datos,
        con_label = if con_datos == 1 {
            "sección"
        } else {
            "secciones"
        },
        sin = sin,
        extra_error = extra_error,
        detalle_sin = detalle_sin,
        detalle_error = detalle_error,
    )
}

/// Página completa (Stitch v2 + fill_shell).
fn page_html(life: &LifeData) -> String {
    // Banner MCP: se emite UNA sola vez. El banner desconectado ya trae el
    // botón "Reintentar", así que aquí no se añade otro.
    let banner = templates::mcp_status_banner(life.connected);
    let aviso = if !life.connected && !life.disconnected_msg.is_empty() {
        format!(
            r#"<p class="font-body-sm text-body-sm text-on-surface-variant mb-space-md">{msg}</p>"#,
            msg = templates::html_escape(&life.disconnected_msg)
        )
    } else {
        String::new()
    };
    let resumen = resumen_html(life);
    let accordion: String = SECTIONS
        .iter()
        .map(|(s, _, _)| render_section(s, life))
        .collect::<Vec<_>>()
        .join("");
    let content = format!(
        r###"<div class="nx-page nx-life nova-page nova-life-page">
          <header class="nx-page-header nova-page-head"><div><p class="nx-eyebrow">Aprendizaje · Vida escolar</p><h1>Tu día a día,<br>también cuenta.</h1><p>Asistencia, convivencia, deberes y recursos institucionales con el detalle de cada fuente.</p></div><span class="nx-version-badge">Mikareno · solo lectura</span></header>
          {resumen}{banner}{aviso}
          <div class="nx-life-grid nova-life-grid">{accordion}</div>
        </div>"###,
        resumen = resumen,
        banner = banner,
        aviso = aviso,
        accordion = accordion,
    );
    templates::page_ext(
        "Vida escolar — Study Platform",
        "Vida escolar",
        "/dashboard/vida-escolar",
        &content,
    )
}

// ── handler ───────────────────────────────────────────────────────────────

/// GET /dashboard/vida-escolar
///
/// Lee los tools read-only de Mikareno (en paralelo, con un único
/// presupuesto de 5 s) y renderiza el acordeón Stitch v2 con el resumen de
/// secciones. `?partial=` recarga sólo esa sección vía HTMX (`outerHTML`).
///
/// La carga completa se cachea 30 s: dos cargas seguidas no repiten las
/// consultas al MCP.
pub async fn index(
    State(state): State<Arc<AppState>>,
    query: Query<SchoolLifeQuery>,
) -> Html<String> {
    let client_opt = mikareno_client(&state);

    // --- Partial reload (HTMX): una sola sección ---
    if let Some(p) = &query.partial {
        if p.len() > MAX_PARTIAL_LEN {
            return Html(String::new());
        }
        // Sólo ids canónicos: nada del input del usuario llega al HTML.
        let Some(canonical) = canonical_section(p) else {
            return Html(String::new());
        };
        let life = match client_opt.as_ref() {
            Some(client) => fetch_one(client, canonical).await,
            None => LifeData::desconectado(MSG_SIN_CONEXION),
        };
        return Html(section_body(canonical, &life));
    }

    // --- Full page ---
    // Caché 30 s: la segunda carga seguida no vuelve a preguntar al MCP.
    if let Some(life) = cache_leer() {
        return Html(page_html(&life));
    }
    let life = match client_opt.as_ref() {
        Some(client) => fetch_all(client).await,
        None => LifeData::desconectado(MSG_SIN_CONEXION),
    };
    // Sólo se cachea lo que el MCP respondió: si no hay sesión, la carga
    // siguiente reintenta la conexión en vez de servir un fallo cacheado.
    if life.connected {
        cache_guardar(&life);
    }
    // La página completa usa `page_ext` (sin estado); aquí relleno el shell.
    let html = page_html(&life);
    let shell = crate::handlers::stitch_panel::fetch_shell_data(&state).await;
    Html(crate::handlers::stitch_panel::fill_shell(&html, &shell))
}

/// Query params: `?partial=<sección>` recarga sólo esa sección vía HTMX.
#[derive(Debug, Deserialize)]
pub struct SchoolLifeQuery {
    partial: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok(v: Value) -> Result<Value, String> {
        Ok(v)
    }

    #[test]
    fn ids_de_seccion_canonicos() {
        assert_eq!(canonical_section("biblioteca"), Some("biblioteca"));
        assert_eq!(canonical_section("cualquiera"), None);
        assert_eq!(canonical_section("\"><script>"), None);
    }

    #[test]
    fn error_tecnico_va_plegado() {
        let s = Seccion::fallo("Asistencia", "timeout de red");
        let html = seccion_html(&s);
        assert!(html.contains("<details"));
        assert!(html.contains("Detalle técnico"));
        // El error crudo no aparece en grande: sólo dentro del <details>.
        assert!(!html.contains(
            "<p class=\"font-body-sm text-body-sm text-on-surface-variant\">timeout de red"
        ));
    }

    #[test]
    fn estado_vacio_nombra_la_seccion() {
        let s = Seccion::vacia("Uniforme", "Mikareno no devolvió registros");
        assert!(s.html.contains("Uniforme"));
        assert!(!s.con_datos);
        assert!(s.error.is_none());
    }

    #[test]
    fn asistencia_muestra_datos_reales() {
        let raw = ok(json!({
            "resumen_semanal": {
                "Lunes": [{"fecha": "2026-10-13", "codigo": "P", "detalle": "Presente", "hora_slot": "1"}]
            },
            "total_registros_semana": 1
        }));
        let s = seccion_asistencia(Some(&raw));
        assert!(s.con_datos);
        assert_eq!(s.conteo, Some(1));
        assert!(s.html.contains("Lunes"));
        assert!(s.html.contains("Presente"));
    }

    #[test]
    fn asistencia_vacia_no_inventa_ceros() {
        let raw = ok(json!({"resumen_semanal": {}, "total_registros_semana": 0}));
        let s = seccion_asistencia(Some(&raw));
        assert!(!s.con_datos);
        assert!(s.html.contains("Asistencia"));
    }

    #[test]
    fn uniforme_filtra_por_tipo_real() {
        let raw = ok(json!({
            "registros_estudiante": [
                {"date": "2026-10-14", "recordTypeName": "Uniforme", "description": "Camisa fuera de norma"},
                {"date": "2026-10-15", "recordTypeName": "Tardanza", "description": "Llegó tarde"}
            ],
            "tipos_registro": [{"name": "Uniforme"}, {"name": "Tardanza"}]
        }));
        let uni = seccion_uniforme(Some(&raw));
        assert!(uni.con_datos);
        assert!(uni.html.contains("Camisa fuera de norma"));
        assert!(!uni.html.contains("Llegó tarde"));

        let punt = seccion_puntualidad(Some(&raw), None);
        assert!(punt.con_datos);
        assert!(punt.html.contains("Llegó tarde"));
    }

    #[test]
    fn biblioteca_usa_el_adapter_compartido() {
        let raw = ok(json!({
            "recursos": [{"titulo": "Revista UNAM", "tipo": "Revista", "url": "https://ejemplo.org/x"}],
            "total_recursos": 1
        }));
        let s = seccion_biblioteca(Some(&raw));
        assert!(s.con_datos);
        assert!(s.html.contains("Revista UNAM"));
    }

    #[test]
    fn biblioteca_invalida_reporta_error_tecnico() {
        let raw = ok(json!({"status": "ok"}));
        let s = seccion_biblioteca(Some(&raw));
        assert!(!s.con_datos);
        assert!(s.error.is_some());
        assert!(seccion_html(&s).contains("<details"));
    }

    #[test]
    fn la_pagina_emite_un_solo_banner_y_un_solo_reintento() {
        let vida = LifeData::desconectado(MSG_SIN_CONEXION);
        let html = page_html(&vida);
        // Un único banner MCP ⇒ un único botón "Reintentar" (la sección
        // Biblioteca ya no lo duplica).
        assert_eq!(html.matches("Mikareno no conectado").count(), 1);
        assert_eq!(html.matches("Reintentar").count(), 1);
        // Resumen honesto, calculado sobre el estado real.
        assert!(html.contains("0 secciones con datos · 6 sin datos"));
        // Las 6 secciones aparecen nombradas.
        for (_id, titulo, _) in SECTIONS {
            assert!(html.contains(titulo), "falta la sección {titulo}");
        }
    }

    #[test]
    fn resumen_cuenta_secciones_reales() {
        let vida = LifeData {
            connected: true,
            disconnected_msg: String::new(),
            secciones: vec![
                Seccion {
                    html: "x".into(),
                    error: None,
                    con_datos: true,
                    conteo: Some(3),
                },
                Seccion::vacia("Puntualidad", "sin datos"),
                Seccion::fallo("Uniforme", "boom"),
                Seccion::vacia("Comportamiento", "sin datos"),
                Seccion::vacia("Tareas", "sin datos"),
                Seccion::vacia("Biblioteca", "sin datos"),
            ],
        };
        let html = resumen_html(&vida);
        assert!(html.contains("1 sección con datos · 5 sin datos"));
        assert!(html.contains("· 1 con error técnico"));
        assert!(html.contains("Sin datos: Puntualidad"));
    }

    #[test]
    fn presupuesto_agotado_muestra_estado_vacio_honesto() {
        // El MCP no respondió dentro del presupuesto: la sección queda
        // vacía y lo dice ("Mikareno no responde ahora"), con el motivo
        // técnico plegado.
        let err = error_agotado("get_biblioteca");
        let s = seccion_error("Biblioteca", &err);
        assert!(!s.con_datos);
        assert!(s.html.contains("Mikareno no responde ahora"));
        assert!(s.html.contains("Biblioteca"));
        assert!(s.error.is_some());
        assert!(seccion_html(&s).contains("<details"));
    }

    #[test]
    fn fallo_tecnico_no_se_confunde_con_el_presupuesto_agotado() {
        // Un fallo técnico del tool no es "no responde": se resume con la
        // frase de fallo y el detalle plegado.
        let s = seccion_error("Uniforme", "timeout de red");
        assert!(!s.con_datos);
        assert!(!s.html.contains("Mikareno no responde ahora"));
        assert!(s
            .html
            .contains("No se pudieron obtener los datos de Uniforme."));
        assert!(s.error.is_some());
    }

    #[test]
    fn la_cache_devuelve_el_resultado_guardado() {
        let life = LifeData::desconectado(MSG_SIN_CONEXION);
        cache_guardar(&life);
        let leido = cache_leer().expect("la caché devuelve lo guardado");
        assert_eq!(leido.secciones.len(), SECTIONS.len());
        assert!(!leido.connected);
    }
}
