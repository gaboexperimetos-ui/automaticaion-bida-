//! Panel principal (Stitch v2): inyeccion de datos reales.
//!
//! El HTML del kit Stitch es estatico (datos de ejemplo). Este modulo toma ese
//! contenido y reemplaza los marcadores `<!--SLOT:...-->` por datos reales
//! obtenidos del MCP de Mikareno (solo lectura) y de la base local.
//!
//! Si el MCP no responde, cada slot degrada a un estado vacio legible en vez
//! de mostrar datos inventados.
//!
//! Reglas que respeta este modulo:
//!
//! - **Cliente compartido**: todo lo que viene del MCP pasa por
//!   `crate::academic::mikareno_client`, que es la misma instancia en toda la
//!   app. Su caché de respuestas vive 180 s, asi que un render normal no
//!   abre llamadas nuevas al servidor.
//! - **Real o `None`**: los conteos y los datos opcionales son `Option`.
//!   `None` significa "no se pudo saber"; un `Some(0)` significa "se sabe
//!   que cero". Nunca se inventa un cero.
//! - **Sin firma rota**: `fill_panel` y `fill_shell` conservan sus firmas y
//!   los mismos marcadores `<!--SLOT:...-->`.
//! - **Presupuesto de render**: la fase MCP completa en ≤ [`PANEL_BUDGET_SECS`]
//!   segundos. Cada tool del servidor puede tardar 25 s, asi que sin este
//!   tope el panel se comia el timeout del proxy (60 s) en vez de renderizar.
//!   Las llamadas van en paralelo y, si el plazo se agota, los campos quedan
//!   en `None` (estado honesto: "Mikareno no responde ahora").
//! - **Caché en memoria**: los datos del panel y del shell se cachean
//!   ([`PANEL_CACHE_TTL_SECS`]) para que dos cargas seguidas no repitan el
//!   trabajo contra el MCP. Los conteos locales se releen siempre.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{Datelike, Timelike};

use crate::integrations::mikareno::{
    CalendarioEvent, Horario, HorarioDia, MikarenoClient, MikarenoError, Nota, Perfil, Tarea,
};
use crate::AppState;

/// Presupuesto global de la fase MCP del panel, en segundos.
///
/// El servidor Mikareno puede tardar hasta 25 s por tool y el panel necesita
/// varios: en serie la página se iba de 60 s (timeout del proxy). Con este
/// tope la fase MCP completa en ≤ 4 s; si se agota, los campos opcionales
/// quedan en `None` y el HTML dice "Mikareno no responde ahora".
const PANEL_BUDGET_SECS: u64 = 4;

/// TTL de la caché en memoria del panel y del shell, en segundos.
///
/// Cubre el caso "dos cargas seguidas del /dashboard": la segunda se sirve
/// de la caché y no vuelve a preguntar al MCP.
const PANEL_CACHE_TTL_SECS: u64 = 30;

/// TTL de la caché cuando Mikareno no respondió (se reintenta antes).
const PANEL_CACHE_TTL_DEGRADADO_SECS: u64 = 10;

/// Estado del último intento de handshake, por URL de MCP.
///
/// Evita que cada render re-intente el handshake cuando el servidor MCP está
/// caído: se reintenta como mucho una vez cada [`HANDSHAKE_REINTENTO_SECS`].
///
/// Se indexa por URL para que dos configuraciones distintas (p. ej. en los
/// tests) no se pisen el mismo freno.
static ULTIMO_HANDSHAKE: Mutex<Option<(String, u64)>> = Mutex::new(None);

/// ¿Hay un handshake en curso ahora mismo?
///
/// Garantiza que dos requests simultáneos no abran dos sesiones contra el
/// mismo servidor MCP: el segundo espera al primero en vez de duplicarla.
static HANDSHAKE_EN_CURSO: AtomicBool = AtomicBool::new(false);

/// Segundos entre dos intentos de handshake cuando el MCP no responde.
const HANDSHAKE_REINTENTO_SECS: u64 = 30;

/// Caché en memoria de los datos del panel: (URL del MCP, instante, datos).
///
/// Se indexa por URL para que una configuración distinta no reutilice datos
/// de otro servidor.
static PANEL_CACHE: Mutex<Option<(String, Instant, PanelData)>> = Mutex::new(None);

/// Caché en memoria de los datos del shell: (URL del MCP, instante, datos).
static SHELL_CACHE: Mutex<Option<(String, Instant, ShellData)>> = Mutex::new(None);

/// Datos reales que alimentan el panel.
#[derive(Default, Clone)]
pub struct PanelData {
    pub connected: bool,
    pub perfil: Option<Perfil>,
    pub notas: Vec<Nota>,
    pub horario: Option<Horario>,
    pub tareas: Vec<Tarea>,
    pub eventos: Vec<CalendarioEvent>,
    /// Notas locales recientes: (titulo, slug).
    pub recent_notes: Vec<(String, String)>,
    /// Tareas que vencen hoy (conteo que reporta Mikareno). `None` cuando
    /// Mikareno no respondió: no se sabe si hay o no.
    pub tasks_today: Option<u32>,
    /// Tareas de la semana en curso (conteo que reporta Mikareno). `None`
    /// cuando Mikareno no respondió.
    pub tasks_week: Option<u32>,
    /// Modelo de IA configurado (para la etiqueta del Copiloto).
    pub openrouter_model: String,
    // ── Datos de las tarjetas nuevas ─────────────────────────────────
    /// Nº real de notas del vault (tabla `notes` de la BD local). `None` si
    /// la consulta falló: nunca un 0 inventado.
    pub notes_count: Option<u32>,
    /// Nº real de conversaciones (hilos distintos de `chat_history`). `None`
    /// si la consulta falló: nunca un 0 inventado.
    pub conversations_count: Option<u32>,
    /// Próximo evento del calendario real (hoy o posterior). `None` si el
    /// calendario no trajo eventos o Mikareno no respondió.
    ///
    /// `evento` y `tipo` vienen del servidor: `tipo` es `SCHOOL_DAY` o
    /// `NON_SCHOOL_DAY` según el día, así que la tarjeta puede distinguir un
    /// día de clases de un día de vacación.
    pub next_event: Option<CalendarioEvent>,
    /// Nº de herramientas que expone el MCP (`tools/list`). `None` si no se
    /// pudo leer la lista: nunca un 0 inventado.
    pub mcp_tools: Option<usize>,
    /// URL configurada del MCP de Mikareno.
    pub mcp_url: String,
    /// Mikareno no respondió dentro del presupuesto de render: los datos que
    /// faltan no es que valgan cero, es que no se pudieron traer.
    pub mcp_timeout: bool,
}

/// Llama a un tool del MCP con el cliente compartido y reintenta **una** vez
/// si el servidor rechazó la sesión (el MCP se reinició y el
/// `mcp-session-id` guardado ya no existe: responde 404 "Session not found").
/// En ese caso `initialize()` emite un id nuevo y la llamada se repite.
///
/// Para cualquier otro error (red caída, tool que falla) NO se reintenta:
/// reintentar sólo alargaría la carga de la página.
///
/// La llamada está atada a `$deadline` (el presupuesto global del render):
/// si el plazo se agota, el tool se cancela y el campo queda en `None`.
macro_rules! mcp_try {
    ($deadline:expr, $client:expr, $call:expr) => {{
        match tokio::time::timeout_at($deadline, $call).await {
            Ok(Ok(v)) => Some(v),
            Ok(Err(e)) if sesion_vencida(&e) => {
                let _ = tokio::time::timeout_at($deadline, $client.initialize()).await;
                match tokio::time::timeout_at($deadline, $call).await {
                    Ok(Ok(v)) => Some(v),
                    _ => None,
                }
            }
            _ => None,
        }
    }};
}

/// Recolecta los datos del panel. Nunca falla: ante un error deja el campo vacio.
///
/// La fase MCP va dentro del presupuesto global ([`PANEL_BUDGET_SECS`]) y pide
/// los datos **en paralelo**: `get_student_full` (un único scrape que trae
/// promedios, horario, calendario y tareas), `get_perfil` (el nombre) y
/// `tools/list` (el estado del servidor). Antes iban en serie y cada una podía
/// tardar 25 s, con lo que la página se iba de 60 s.
///
/// El resultado se cachea en memoria ([`PANEL_CACHE_TTL_SECS`]): dos cargas
/// seguidas del /dashboard no repiten el trabajo. Los conteos locales (notas,
/// conversaciones) se releen siempre: son baratos y cambian al escribir.
pub async fn fetch_panel_data(state: &AppState) -> PanelData {
    let url = state.config.mikareno_mcp_url.clone();
    let cached = cache_tomar_panel(&url);
    let mut d = cached.clone().unwrap_or_default();
    d.openrouter_model = state.config.openrouter_model.clone();
    d.mcp_url = url.clone();

    // Base local: notas recientes y conteos reales.
    //
    // El SQL es idéntico al de `crate::db::db_info` (notas = filas de `notes`,
    // conversaciones = hilos distintos de `chat_history`), así que los
    // conteos no divergen del panel de administración ni del handoff. La
    // diferencia es la semántica: aquí son `Option`, porque una consulta
    // fallida debe dejar el dato en `None` ("no se pudo saber") y no en un
    // 0 inventado.
    let (recientes, notas_total, conversaciones) = tokio::join!(
        sqlx::query_as::<_, (String, String)>(
            "SELECT title, slug FROM notes ORDER BY updated_at DESC LIMIT 3"
        )
        .fetch_all(&state.db),
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notes").fetch_one(&state.db),
        sqlx::query_scalar::<_, i64>("SELECT COUNT(DISTINCT subject) FROM chat_history")
            .fetch_one(&state.db),
    );
    if let Ok(rows) = recientes {
        d.recent_notes = rows;
    }
    d.notes_count = notas_total.ok().and_then(|n| u32::try_from(n).ok());
    d.conversations_count = conversaciones.ok().and_then(|n| u32::try_from(n).ok());

    // MCP: sólo si la caché no lo cubre. Con el presupuesto agotado, los
    // campos quedan en `None` y el panel lo dice con claridad.
    if cached.is_none() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(PANEL_BUDGET_SECS);
        let agotado = tokio::time::timeout_at(deadline, fase_mcp_panel(state, &mut d, deadline))
            .await
            .is_err();
        if agotado {
            d.mcp_timeout = true;
        }
        cache_guardar_panel(&url, &d);
    }

    // Próximo evento del calendario: el primero a partir de hoy.
    d.next_event = next_calendar_event(&d);

    d
}

/// Fase MCP del panel: abre sesión (si hace falta) y pide los datos.
///
/// Todo ocurre dentro del mismo plazo (`deadline`): el handshake se hace
/// primero (para no abrir dos sesiones) y las llamadas a los tools van en
/// paralelo. Si el plazo se agota, los campos que no llegaron se quedan en
/// `None` y `mcp_timeout` queda en `true` (estado honesto: "Mikareno no
/// responde ahora").
async fn fase_mcp_panel(state: &AppState, d: &mut PanelData, deadline: tokio::time::Instant) {
    let Some(client) = crate::academic::mikareno_client(state) else {
        return;
    };
    let url = state.config.mikareno_mcp_url.clone();

    // Sesión viva: el cliente compartido la mantiene entre requests. Si
    // aún no la tiene (arranque en frío), se abre una sola vez; si el
    // servidor está caído, se reintenta espaciado para no martillearlo
    // en cada render.
    //
    // Sin sesión no se llama a ningún tool: `call_tool` reintentaría el
    // handshake por su cuenta y cada render pagaría el coste.
    match abrir_sesion_mcp(&client, &url, deadline).await {
        Sesion::Viva => {}
        // El servidor no responde: el panel queda en "sin conexión".
        Sesion::Fallo => return,
        // Se agotó el presupuesto: el panel lo dice ("no responde ahora").
        Sesion::Cortada => {
            d.mcp_timeout = true;
            return;
        }
    }

    // Las tres llamadas en paralelo, bajo el mismo plazo. El orden importa:
    // el servidor MCP es mono-hilo, así que la primera que sale es la que
    // entra primero (`get_student_full` trae casi todo el panel).
    //
    // `join!` espera por dentro, así que va dentro de un `async` para que el
    // presupuesto global lo pueda cortar.
    // `join!` devuelve las salidas en el orden de las ramas:
    // (get_student_full, get_perfil, tools/list).
    let (full, perfil, tools) = match tokio::time::timeout_at(deadline, async {
        tokio::join!(
            async { mcp_try!(deadline, client, client.student_full()) },
            async { mcp_try!(deadline, client, client.perfil()) },
            async { mcp_try!(deadline, client, client.list_tools()) },
        )
    })
    .await
    {
        Ok(v) => v,
        Err(_) => {
            d.mcp_timeout = true;
            (None, None, None)
        }
    };

    // Perfil (nombre, institución, curso).
    if let Some(p) = perfil {
        d.connected = true;
        if !p.nombre.is_empty() {
            d.perfil = Some(p);
        }
    }

    // Una sola llamada con todo (promedios + horario + calendario + tareas).
    if let Some(full) = full {
        d.connected = true;
        d.notas = crate::integrations::mikareno::parse_promedios_full(&full);
        d.horario = parse_period_map(&full);
        d.eventos = parse_calendario(&full);
        parse_tarea_stats(&full, d);
        d.tareas = parse_tareas_paginadas(&full);
    }

    // Estado real del MCP: nº de herramientas (`tools/list`, que el
    // cliente también cachea 180 s). `None` si no se pudo leer.
    d.mcp_tools = tools.map(|t| t.len());

    // Fallback: si `get_student_full` no trajo horario, usa el tool directo
    // (sólo si queda presupuesto: es una llamada más, no una más de las que
    // había antes).
    if d.horario.is_none() {
        if let Some(h) = mcp_try!(deadline, client, client.horario()) {
            d.connected = true;
            d.horario = Some(h);
        }
    }
}

/// Resultado de intentar dejar viva la sesión MCP.
enum Sesion {
    /// La sesión quedó viva.
    Viva,
    /// El servidor no respondió (o no se pudo abrir sesión).
    Fallo,
    /// Se agotó el presupuesto a mitad del handshake.
    Cortada,
}

/// Abre la sesión MCP compartida si hace falta.
///
/// Devuelve [`Sesion::Viva`] si el cliente queda con sesión viva. Si el
/// servidor está caído, el reintento se espacia [`HANDSHAKE_REINTENTO_SECS`]
/// para no martillearlo en cada render.
///
/// Sólo un request a la vez hace el handshake ([`HANDSHAKE_EN_CURSO`]): los
/// demás esperan su resultado dentro del presupuesto en vez de abrir una
/// segunda sesión contra el mismo servidor.
async fn abrir_sesion_mcp(
    client: &MikarenoClient,
    url: &str,
    deadline: tokio::time::Instant,
) -> Sesion {
    if client.is_connected() {
        return Sesion::Viva;
    }
    if HANDSHAKE_EN_CURSO
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        // Otro request está con el handshake: se espera a que la termine.
        return esperar_sesion(client, deadline).await;
    }
    // El flag se libera aunque la tarea se cancele (Drop), para no dejar el
    // servidor "bloqueado" para los requests siguientes.
    let _guard = HandshakeGuard;
    if !handshake_permitido(url) {
        return Sesion::Fallo;
    }
    marcar_handshake(url);
    match tokio::time::timeout_at(deadline, client.initialize()).await {
        Ok(Ok(())) => Sesion::Viva,
        Ok(Err(_)) => Sesion::Fallo,
        Err(_) => {
            // Cortado por el presupuesto: no se penaliza al servidor con el
            // freno de reintento, la próxima carga vuelve a intentarlo.
            desmarcar_handshake(url);
            Sesion::Cortada
        }
    }
}

/// Libera [`HANDSHAKE_EN_CURSO`] aunque la tarea se cancele.
struct HandshakeGuard;

impl Drop for HandshakeGuard {
    fn drop(&mut self) {
        HANDSHAKE_EN_CURSO.store(false, Ordering::Release);
    }
}

/// Espera a que otro request termine su handshake.
///
/// Devuelve [`Sesion::Viva`] si la sesión aparece; si el plazo se agota,
/// [`Sesion::Cortada`]: el servidor no respondió a tiempo.
async fn esperar_sesion(client: &MikarenoClient, deadline: tokio::time::Instant) -> Sesion {
    let espera = async {
        loop {
            if client.is_connected() {
                return Sesion::Viva;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    match tokio::time::timeout_at(deadline, espera).await {
        Ok(s) => s,
        Err(_) => Sesion::Cortada,
    }
}

/// ¿El error es "el servidor no conoce esta sesión" (HTTP 404 / `Session not
/// found`)? Es el único caso en que un `initialize()` nuevo lo arregla.
fn sesion_vencida(e: &MikarenoError) -> bool {
    match e {
        // El cliente formatea los errores HTTP como "HTTP {status}: {body}".
        MikarenoError::Http(msg) => msg.contains("HTTP 404") || msg.contains("Session not found"),
        _ => false,
    }
}

/// ¿Toca abrir sesión con este MCP ahora?
///
/// Evita que cada render re-intente el handshake cuando el servidor está
/// caído: se reintenta como mucho una vez cada [`HANDSHAKE_REINTENTO_SECS`].
fn handshake_permitido(url: &str) -> bool {
    let ahora = chrono::Utc::now().timestamp().max(0) as u64;
    let guard = ULTIMO_HANDSHAKE
        .lock()
        .expect("mutex de handshakes envenenado");
    match guard.as_ref() {
        Some((u, t)) if u == url => ahora.saturating_sub(*t) >= HANDSHAKE_REINTENTO_SECS,
        // Nunca se intentó con esta URL (o es otra): se permite.
        _ => true,
    }
}

/// Marca que se intentó abrir sesión con este MCP.
fn marcar_handshake(url: &str) {
    let ahora = chrono::Utc::now().timestamp().max(0) as u64;
    let mut guard = ULTIMO_HANDSHAKE
        .lock()
        .expect("mutex de handshakes envenenado");
    *guard = Some((url.to_string(), ahora));
}

/// Borra el freno de reintento de este MCP.
///
/// Se usa cuando el handshake se cortó por el presupuesto (no por un fallo
/// del servidor): penalizar al servidor con 30 s de freno dejaría el panel
/// degradado aunque el MCP esté bien.
fn desmarcar_handshake(url: &str) {
    let mut guard = ULTIMO_HANDSHAKE
        .lock()
        .expect("mutex de handshakes envenenado");
    if guard.as_ref().is_some_and(|(u, _)| u == url) {
        *guard = None;
    }
}

// ── caché en memoria del panel y del shell ────────────────────────────────

/// Toma de la caché los datos del panel si siguen siendo válidos.
///
/// La clave es la URL del MCP: con otra configuración no se reutiliza nada.
fn cache_tomar_panel(url: &str) -> Option<PanelData> {
    let guard = PANEL_CACHE.lock().expect("mutex del panel envenenado");
    let (u, t, d) = guard.as_ref()?;
    if u != url {
        return None;
    }
    let ttl = if d.mcp_timeout {
        PANEL_CACHE_TTL_DEGRADADO_SECS
    } else {
        PANEL_CACHE_TTL_SECS
    };
    if t.elapsed() < Duration::from_secs(ttl) {
        Some(d.clone())
    } else {
        None
    }
}

/// Guarda los datos del panel en la caché.
fn cache_guardar_panel(url: &str, d: &PanelData) {
    cache_guardar_panel_en(url, d, Instant::now());
}

/// Igual que [`cache_guardar_panel`], con el instante de escritura
/// parametrizado (para poder probar el TTL sin esperar).
fn cache_guardar_panel_en(url: &str, d: &PanelData, t: Instant) {
    let mut guard = PANEL_CACHE.lock().expect("mutex del panel envenenado");
    *guard = Some((url.to_string(), t, d.clone()));
}

/// Toma de la caché los datos del shell si siguen siendo válidos.
fn cache_tomar_shell(url: &str) -> Option<ShellData> {
    let guard = SHELL_CACHE.lock().expect("mutex del shell envenenado");
    let (u, t, d) = guard.as_ref()?;
    if u != url {
        return None;
    }
    if t.elapsed() < Duration::from_secs(PANEL_CACHE_TTL_SECS) {
        Some(d.clone())
    } else {
        None
    }
}

/// Guarda los datos del shell en la caché.
fn cache_guardar_shell(url: &str, d: &ShellData) {
    let mut guard = SHELL_CACHE.lock().expect("mutex del shell envenenado");
    *guard = Some((url.to_string(), Instant::now(), d.clone()));
}

// ── parseo de get_student_full ─────────────────────────────────────────────

/// `horario.periodMap` → `Horario` con las mismas etiquetas planas que usa el
/// adaptador de `get_horario` (`"07:35 - 08:15 — Religión [RELI]"`).
fn parse_period_map(full: &serde_json::Value) -> Option<Horario> {
    let map = full.get("horario")?.get("periodMap")?.as_object()?;
    let days = [
        ("MONDAY", "lunes"),
        ("TUESDAY", "martes"),
        ("WEDNESDAY", "miércoles"),
        ("THURSDAY", "jueves"),
        ("FRIDAY", "viernes"),
        ("SATURDAY", "sábado"),
        ("SUNDAY", "domingo"),
    ];
    let mut dias = Vec::new();
    for (key, es) in days {
        let Some(slots) = map.get(key).and_then(|v| v.as_array()) else {
            continue;
        };
        if slots.is_empty() {
            continue;
        }
        let mut clases = Vec::new();
        for slot in slots {
            let hora = slot
                .get("slot")
                .and_then(|s| s.get("prettyTime"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let name = slot
                .get("subject")
                .and_then(|s| s.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let code = slot
                .get("subject")
                .and_then(|s| s.get("code"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let label = match (hora.is_empty(), name.is_empty(), code.is_empty()) {
                (true, _, true) => name.to_string(),
                (true, _, false) => format!("{} [{}]", name, code),
                (false, _, true) => format!("{} — {}", hora, name),
                (false, _, false) => format!("{} — {} [{}]", hora, name, code),
            };
            clases.push(label);
        }
        dias.push(HorarioDia {
            dia: es.to_string(),
            clases,
        });
    }
    if dias.is_empty() {
        None
    } else {
        Some(Horario { dias })
    }
}

/// `calendario[]` → eventos con fecha ISO corta y título.
fn parse_calendario(full: &serde_json::Value) -> Vec<CalendarioEvent> {
    let Some(items) = full.get("calendario").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|e| {
            let start = e.get("start").and_then(|v| v.as_str()).unwrap_or("");
            let fecha = start.get(0..10).unwrap_or(start).to_string();
            let evento = e
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if fecha.is_empty() || evento.is_empty() {
                return None;
            }
            let tipo = e
                .get("dayType")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(CalendarioEvent {
                fecha,
                evento,
                tipo,
            })
        })
        .collect()
}

/// `tarea_stats` → conteos de tareas.
///
/// Los conteos son `Option`: si Mikareno no trajo el bloque, no se sabe
/// cuántas tareas hay y no se inventa un 0.
fn parse_tarea_stats(full: &serde_json::Value, d: &mut PanelData) {
    let Some(stats) = full.get("tarea_stats") else {
        return;
    };
    d.tasks_today = stats
        .get("todaysTasks")
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok());
    d.tasks_week = stats
        .get("tasksCurrentWeek")
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok());
}

/// `tareas_paginadas.pageContent[]` → lista de tareas (si el MCP la trae).
fn parse_tareas_paginadas(full: &serde_json::Value) -> Vec<Tarea> {
    let Some(items) = full
        .get("tareas_paginadas")
        .and_then(|t| t.get("pageContent"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|t| {
            let titulo = t
                .get("name")
                .or_else(|| t.get("title"))
                .or_else(|| t.get("subject"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if titulo.is_empty() {
                return None;
            }
            let vencimiento = t
                .get("dueDate")
                .or_else(|| t.get("deadline"))
                .or_else(|| t.get("date"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let estado = t
                .get("status")
                .or_else(|| t.get("state"))
                .and_then(|v| v.as_str())
                .unwrap_or("pendiente")
                .to_string();
            Some(Tarea {
                titulo,
                vencimiento,
                estado,
            })
        })
        .collect()
}

// ── helpers ────────────────────────────────────────────────────────────────

/// Próximo evento del calendario real (a partir de hoy).
///
/// Usa el mismo calendario que el resto del panel (`get_student_full`), así
/// que no cuesta una llamada extra al MCP.
///
/// El calendario de Mikareno mezcla dos cosas: eventos con nombre propio
/// (examen, reunión…) y marcadores de día (`SCHOOL_DAY` / `NON_SCHOOL_DAY`).
/// Se prefiere siempre un evento con nombre propio; si el calendario sólo
/// tiene marcadores de día, se devuelve el próximo día del calendario.
///
/// `tipo` y `evento` vienen del servidor, así que la tarjeta puede distinguir
/// "Clases" de "Vacación" sin inventar nada.
fn next_calendar_event(d: &PanelData) -> Option<CalendarioEvent> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let futuros: Vec<&CalendarioEvent> = d.eventos.iter().filter(|e| e.fecha >= today).collect();
    // 1) Evento con título propio (no es un marcador de día).
    let con_nombre = futuros
        .iter()
        .copied()
        .filter(|e| e.tipo != "SCHOOL_DAY" && e.tipo != "NON_SCHOOL_DAY")
        .min_by(|a, b| a.fecha.cmp(&b.fecha))
        .cloned();
    if con_nombre.is_some() {
        return con_nombre;
    }
    // 2) Próximo día del calendario (lectivo o vacación).
    futuros
        .iter()
        .copied()
        .min_by(|a, b| a.fecha.cmp(&b.fecha))
        .cloned()
}

fn esc(s: &str) -> String {
    crate::academic::templates::html_escape(s)
}

fn weekday_es(w: chrono::Weekday) -> &'static str {
    use chrono::Weekday::*;
    match w {
        Mon => "lunes",
        Tue => "martes",
        Wed => "miércoles",
        Thu => "jueves",
        Fri => "viernes",
        Sat => "sábado",
        Sun => "domingo",
    }
}

fn weekday_short_es(w: chrono::Weekday) -> &'static str {
    use chrono::Weekday::*;
    match w {
        Mon => "L",
        Tue => "M",
        Wed => "X",
        Thu => "J",
        Fri => "V",
        Sat => "S",
        Sun => "D",
    }
}

fn month_es(m: u32) -> &'static str {
    match m {
        1 => "enero",
        2 => "febrero",
        3 => "marzo",
        4 => "abril",
        5 => "mayo",
        6 => "junio",
        7 => "julio",
        8 => "agosto",
        9 => "septiembre",
        10 => "octubre",
        11 => "noviembre",
        12 => "diciembre",
        _ => "",
    }
}

/// Primera palabra del nombre completo ("Lennin Gabriel Salas" -> "Lennin").
fn first_name(full: &str) -> String {
    full.split_whitespace().next().unwrap_or("").to_string()
}

/// "07:35 - 08:15" de una etiqueta "07:35 - 08:15 — Asignatura [CODE]".
fn slot_time(label: &str) -> String {
    label
        .split(" — ")
        .next()
        .unwrap_or(label)
        .trim()
        .to_string()
}

/// "Asignatura" de una etiqueta; vacío si es un receso.
fn slot_name(label: &str) -> String {
    let rest = label.split(" — ").nth(1).unwrap_or("");
    let name = rest.split(" [").next().unwrap_or(rest).trim();
    name.to_string()
}

/// Hora/minuto de inicio si la etiqueta empieza por "HH:MM".
fn slot_start_minutes(label: &str) -> Option<u32> {
    let t = label.get(0..5)?;
    let mut it = t.split(':');
    let h: u32 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    Some(h * 60 + m)
}

/// Clases del día indicado según el horario real.
fn classes_for(d: &PanelData, wd: &str) -> Vec<String> {
    d.horario
        .as_ref()
        .and_then(|h| {
            h.dias
                .iter()
                .find(|x| x.dia.eq_ignore_ascii_case(wd))
                .map(|x| x.clases.clone())
        })
        .unwrap_or_default()
}

// ── render de slots ────────────────────────────────────────────────────────

fn render_greeting(d: &PanelData) -> String {
    let hour = chrono::Local::now().hour();
    let saludo = if hour < 12 {
        "Buenos días"
    } else if hour < 19 {
        "Buenas tardes"
    } else {
        "Buenas noches"
    };
    let name = d
        .perfil
        .as_ref()
        .map(|p| first_name(&p.nombre))
        .filter(|n| !n.is_empty());
    match name {
        Some(n) => format!("{}, {}", saludo, esc(&n)),
        None => saludo.to_string(),
    }
}

fn render_todayline(d: &PanelData) -> String {
    let now = chrono::Local::now();
    let base = format!(
        "| {}, {} de {}",
        capitalize(weekday_es(now.weekday())),
        now.day(),
        month_es(now.month())
    );
    let tri = d.notas.first().map(|n| n.trimestre).filter(|t| *t > 0);
    match tri {
        Some(t) => format!("{} • Trimestre {}", base, t),
        None => base,
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn render_sync(d: &PanelData) -> String {
    if d.connected {
        "Mikareno: sincronizado ahora".to_string()
    } else if d.mcp_timeout {
        "Mikareno: no responde ahora".to_string()
    } else {
        "Mikareno: sin conexión".to_string()
    }
}

fn render_classcount(d: &PanelData) -> String {
    let now = chrono::Local::now();
    let wd = weekday_es(now.weekday());
    let now_min = now.hour() * 60 + now.minute();
    let remaining = classes_for(d, wd)
        .iter()
        .filter(|l| slot_start_minutes(l).map(|m| m >= now_min).unwrap_or(false))
        .count();
    let dot = if d.connected {
        r#"<span class="w-1.5 h-1.5 rounded-full bg-secondary"></span> "#
    } else {
        r#"<span class="w-1.5 h-1.5 rounded-full bg-error"></span> "#
    };
    format!("{}{} clases rest.", dot, remaining)
}

fn render_classes(d: &PanelData) -> String {
    let now = chrono::Local::now();
    let wd = weekday_es(now.weekday());
    let clases = classes_for(d, wd);
    if clases.is_empty() {
        return r##"<div class="p-space-sm rounded-lg bg-surface-container text-body-sm text-body-sm text-outline">Sin clases programadas para hoy.</div>"##.to_string();
    }
    clases
        .iter()
        .map(|label| {
            let time = slot_time(label);
            let name = slot_name(label);
            let name = if name.is_empty() {
                "Receso".to_string()
            } else {
                name
            };
            format!(
                r##"<div class="p-space-sm rounded-lg bg-surface-container hover:bg-surface-container-high transition-all flex flex-col gap-1.5">
                <div class="flex items-center justify-between">
                    <span class="font-code-inline text-[11px] text-primary-fixed-dim px-1.5 py-0.5 rounded bg-primary-container/10">{time}</span>
                </div>
                <h3 class="font-label-md text-label-md text-on-surface font-semibold leading-snug">{name}</h3>
            </div>"##,
                time = esc(&time),
                name = esc(&name)
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Tareas cuya fecha de vencimiento es hoy (comparación por prefijo ISO).
fn tasks_today(d: &PanelData) -> Vec<&Tarea> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    d.tareas
        .iter()
        .filter(|t| !t.vencimiento.is_empty() && t.vencimiento.starts_with(&today))
        .collect()
}

fn render_taskcount(d: &PanelData) -> String {
    // Prefiere el conteo oficial de Mikareno; cae a la lista local si falta.
    let n = match d.tasks_today {
        Some(n) if n > 0 => n as usize,
        Some(_) => tasks_today(d).len(),
        // Mikareno no dio el dato: no se sabe si hay tareas o no.
        None => {
            return if d.connected {
                "Mikareno no informó de las tareas".to_string()
            } else if d.mcp_timeout {
                "Mikareno no responde ahora".to_string()
            } else {
                "Mikareno sin conexión".to_string()
            };
        }
    };
    if n == 0 {
        "Sin entregas hoy".to_string()
    } else {
        format!("{} pendientes", n)
    }
}

fn render_tasks_today(d: &PanelData) -> String {
    let tareas = tasks_today(d);
    if tareas.is_empty() {
        return match d.tasks_today {
            Some(0) => r##"<div class="p-2 rounded-lg bg-surface-container text-body-sm text-body-sm text-outline">No hay entregas con vencimiento hoy.</div>"##.to_string(),
            Some(n) => format!(
                r##"<div class="p-2 rounded-lg bg-surface-container text-body-sm text-body-sm text-outline">{n} tarea(s) para hoy. El detalle no está disponible en Mikareno ahora mismo.</div>"##,
                n = n
            ),
            // Sin el dato no se afirma que no haya tareas.
            None if d.connected => r##"<div class="p-2 rounded-lg bg-surface-container text-body-sm text-body-sm text-outline">Mikareno no informó de las tareas de hoy: no se puede saber si hay entregas.</div>"##.to_string(),
            None if d.mcp_timeout => r##"<div class="p-2 rounded-lg bg-surface-container text-body-sm text-body-sm text-outline">Mikareno no responde ahora: no se puede saber si hay entregas hoy.</div>"##.to_string(),
            None => r##"<div class="p-2 rounded-lg bg-surface-container text-body-sm text-body-sm text-outline">Mikareno no respondió: no se puede saber si hay entregas hoy.</div>"##.to_string(),
        };
    }
    tareas
        .iter()
        .map(|t| {
            format!(
                r##"<div class="p-2 rounded-lg bg-surface-container flex items-start gap-2.5">
                <input class="mt-1 w-4 h-4 rounded bg-surface-container-lowest accent-secondary cursor-pointer" type="checkbox"/>
                <div class="flex-1 min-w-0">
                    <p class="font-label-md text-label-md text-on-surface font-medium truncate">{titulo}</p>
                    <span class="font-body-sm text-[10px] text-outline">Mikareno</span>
                </div>
            </div>"##,
                titulo = esc(&t.titulo)
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn render_notes(d: &PanelData) -> String {
    if d.recent_notes.is_empty() {
        return r##"<div class="p-2.5 rounded-lg bg-surface-container text-body-sm text-body-sm text-outline">Todavía no hay notas. <a href="/dashboard/notes" class="text-primary-container hover:underline">Crear la primera</a>.</div>"##.to_string();
    }
    d.recent_notes
        .iter()
        .map(|(title, slug)| {
            format!(
                r##"<div class="p-2.5 rounded-lg bg-surface-container hover:bg-surface-container-high transition-colors flex items-center justify-between">
                <div class="flex items-start gap-2 min-w-0">
                    <span class="material-symbols-outlined text-[18px] text-tertiary-fixed-dim mt-0.5">draft</span>
                    <a href="/app/notes/{slug}" class="font-code-inline text-[12px] text-primary-fixed-dim hover:underline cursor-pointer truncate block">{title}</a>
                </div>
                <span class="font-code-inline text-[10px] px-1.5 py-0.5 rounded bg-surface-container-lowest text-outline">nota</span>
            </div>"##,
                slug = esc(slug),
                title = esc(title)
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn render_ai_resume(d: &PanelData) -> String {
    // No hay historial de chat persistido todavía: invitación neutra.
    let _ = d;
    r##"<div class="p-3 rounded-lg bg-surface-container mb-space-sm">
        <div class="flex items-start gap-2.5">
            <div class="w-7 h-7 rounded-lg bg-primary-container/20 flex items-center justify-center shrink-0 text-primary-container">
                <span class="material-symbols-outlined text-[16px]">auto_awesome</span>
            </div>
            <div class="min-w-0 flex-1">
                <span class="font-label-sm text-[10px] uppercase tracking-wider text-outline">Asistente académico</span>
                <p class="font-label-md text-label-md text-on-surface font-medium mt-0.5">Pregunta lo que necesites sobre tus materias.</p>
                <div class="flex items-center gap-2 mt-2">
                    <a href="/dashboard/ai" class="px-2.5 py-1 rounded bg-primary-container text-on-primary font-label-sm text-label-sm font-semibold hover:bg-primary-fixed-dim transition-all">Abrir asistente</a>
                </div>
            </div>
        </div>
    </div>"##
        .to_string()
}

fn render_tasktable(d: &PanelData) -> String {
    if d.tareas.is_empty() {
        let msg = match d.tasks_week {
            Some(n) if n > 0 => format!(
                "{} tarea(s) esta semana. Mikareno no expone el detalle ahora mismo.",
                n
            ),
            Some(_) => "Sin tareas pendientes.".to_string(),
            // Sin el dato no se afirma que no haya tareas.
            None if d.connected => {
                "Mikareno no informó de las tareas: no se puede saber cuántas hay.".to_string()
            }
            None if d.mcp_timeout => {
                "Mikareno no responde ahora: no se puede saber cuántas tareas hay.".to_string()
            }
            None => "Mikareno no respondió: no se puede saber cuántas tareas hay.".to_string(),
        };
        return format!(
            r##"<tr><td colspan="6" class="py-4 text-center font-body-sm text-body-sm text-outline">{}</td></tr>"##,
            esc(&msg)
        );
    }
    d.tareas
        .iter()
        .take(6)
        .map(|t| {
            let limite = if t.vencimiento.is_empty() {
                "—".to_string()
            } else {
                t.vencimiento.clone()
            };
            format!(
                r##"<tr class="hover:bg-surface-container transition-colors group">
                <td class="py-2.5 pl-1 align-middle">
                    <input class="w-3.5 h-3.5 rounded bg-surface-container-lowest accent-secondary cursor-pointer" type="checkbox"/>
                </td>
                <td class="py-2.5 pr-2 align-middle font-medium text-on-surface min-w-[140px]">
                    <div class="truncate font-label-md text-label-sm">{titulo}</div>
                    <span class="text-[10px] font-code-inline text-primary-fixed-dim">{estado}</span>
                </td>
                <td class="py-2.5 pr-2 align-middle whitespace-nowrap text-outline">—</td>
                <td class="py-2.5 pr-2 align-middle whitespace-nowrap">
                    <span class="font-code-inline text-[11px] text-outline">{limite}</span>
                </td>
                <td class="py-2.5 pr-2 align-middle whitespace-nowrap text-outline">—</td>
                <td class="py-2.5 pr-1 align-middle text-right whitespace-nowrap">
                    <div class="flex items-center justify-end gap-1 opacity-60 group-hover:opacity-100 transition-opacity">
                        <span class="material-symbols-outlined text-[16px] text-on-surface-variant">more_vert</span>
                    </div>
                </td>
            </tr>"##,
                titulo = esc(&t.titulo),
                estado = esc(&t.estado),
                limite = esc(&limite)
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn render_taskfooter(d: &PanelData) -> String {
    let total = match d.tasks_week {
        Some(n) => (n as usize).max(d.tareas.len()),
        None => d.tareas.len(),
    };
    let shown = d.tareas.len().min(6);
    if total == 0 {
        match d.tasks_week {
            Some(_) => "Sin tareas activas".to_string(),
            // Sin el dato no se afirma que no haya tareas.
            None if d.connected => "Mikareno no informó de las tareas".to_string(),
            None if d.mcp_timeout => "Mikareno no responde ahora".to_string(),
            None => "Mikareno sin conexión".to_string(),
        }
    } else {
        format!("Mostrando {} de {} tareas activas", shown, total)
    }
}

fn render_weektitle() -> String {
    let now = chrono::Local::now();
    format!("Semana {}", now.iso_week().week())
}

fn render_weekcal(d: &PanelData) -> String {
    let today = chrono::Local::now();
    let monday =
        today.date_naive() - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let today_iso = today.format("%Y-%m-%d").to_string();

    let mut cells = String::new();
    for i in 0..7i64 {
        let date = monday + chrono::Duration::days(i);
        let iso = date.format("%Y-%m-%d").to_string();
        let wd = date.weekday();
        let is_today = iso == today_iso;

        let has_classes = !classes_for(d, weekday_es(wd)).is_empty();
        let has_task = d.tareas.iter().any(|t| t.vencimiento.starts_with(&iso));

        let mut dots = String::new();
        if has_task {
            dots.push_str(r#"<span class="w-1.5 h-1.5 rounded-full bg-error"></span>"#);
        }
        if has_classes {
            dots.push_str(r#"<span class="w-1 h-1 rounded-full bg-secondary"></span>"#);
        }
        if dots.is_empty() {
            dots.push_str(r#"<span class="h-1"></span>"#);
        }

        if is_today {
            cells.push_str(&format!(
                r##"<div class="flex flex-col items-center p-1.5 rounded-lg bg-primary-container/15 text-primary-container shadow-inner">
                <span class="font-label-sm text-[10px] font-bold text-primary-container">{letter}</span>
                <span class="font-code-inline text-code-inline text-primary-container font-bold mt-0.5">{day}</span>
                <div class="flex gap-0.5 mt-1.5">{dots}</div>
            </div>"##,
                letter = weekday_short_es(wd),
                day = date.day(),
                dots = dots
            ));
        } else {
            cells.push_str(&format!(
                r##"<div class="flex flex-col items-center p-1.5 rounded-lg bg-surface-container/60 hover:bg-surface-container transition-colors">
                <span class="font-label-sm text-[10px] text-outline">{letter}</span>
                <span class="font-code-inline text-code-inline text-on-surface font-semibold mt-0.5">{day}</span>
                <div class="flex gap-0.5 mt-1.5">{dots}</div>
            </div>"##,
                letter = weekday_short_es(wd),
                day = date.day(),
                dots = dots
            ));
        }
    }
    cells
}

fn next_exam(d: &PanelData) -> Option<&CalendarioEvent> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    d.eventos
        .iter()
        .filter(|e| e.evento.to_uppercase().contains("EXAMEN") && e.fecha >= today)
        .min_by(|a, b| a.fecha.cmp(&b.fecha))
}

fn render_exam(d: &PanelData) -> String {
    match next_exam(d) {
        Some(ev) => format!(
            r##"<div class="rounded-xl bg-surface-container-low p-space-md shadow-md relative overflow-hidden">
            <div class="absolute -right-6 -bottom-6 w-28 h-28 bg-error/10 rounded-full blur-xl pointer-events-none"></div>
            <div class="flex items-center justify-between mb-2">
                <span class="px-2 py-0.5 rounded text-[10px] font-code-inline bg-error-container/40 text-error font-semibold flex items-center gap-1">
                    <span class="material-symbols-outlined text-[12px]">timer</span> EXAMEN
                </span>
                <span class="font-code-inline text-[11px] text-outline">{fecha}</span>
            </div>
            <h3 class="font-headline-sm text-headline-sm text-on-surface font-bold leading-snug">{evento}</h3>
        </div>"##,
            fecha = esc(&ev.fecha),
            evento = esc(&ev.evento)
        ),
        None => String::new(),
    }
}

fn render_focus(d: &PanelData) -> String {
    if d.notas.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<&Nota> = d.notas.iter().collect();
    sorted.sort_by(|a, b| {
        a.nota
            .partial_cmp(&b.nota)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let bars: Vec<String> = sorted
        .iter()
        .take(6)
        .map(|n| {
            let pct = (n.nota * 10.0).clamp(0.0, 100.0) as u32;
            let color = if n.nota < 4.0 {
                "bg-error"
            } else if n.nota < 7.0 {
                "bg-secondary"
            } else {
                "bg-primary-container"
            };
            format!(
                r##"<div class="flex items-center justify-between text-body-sm text-[11px]">
                    <span class="text-on-surface-variant truncate">{materia}</span>
                    <span class="font-code-inline text-on-surface ml-2">{nota:.2}</span>
                </div>
                <div class="w-full h-1.5 rounded-full bg-surface-container overflow-hidden mb-1.5">
                    <div class="h-full {color} rounded-full" style="width: {pct}%"></div>
                </div>"##,
                materia = esc(&n.materia),
                nota = n.nota,
                color = color,
                pct = pct
            )
        })
        .collect();

    format!(
        r##"<div class="rounded-xl bg-surface-container-low p-space-md shadow-md flex flex-col">
        <div class="flex items-center justify-between pb-space-xs mb-space-xs">
            <span class="font-label-sm text-[10px] uppercase text-outline tracking-wider font-semibold">Promedio por materia</span>
            <span class="font-code-inline text-[11px] text-outline">Trimestre {tri}</span>
        </div>
        <div class="space-y-1.5 mt-1">{bars}</div>
        <div class="mt-space-sm pt-space-xs flex justify-end">
            <a href="/dashboard/subjects" class="font-label-sm text-[11px] text-outline hover:text-on-surface transition-colors flex items-center gap-1">
                <span class="material-symbols-outlined text-[14px]">query_stats</span> Ver asignaturas
            </a>
        </div>
    </div>"##,
        tri = d.notas.first().map(|n| n.trimestre).unwrap_or(0),
        bars = bars.join("")
    )
}

/// Etiqueta del modelo de IA realmente configurado.
fn render_ai_model(d: &PanelData) -> String {
    if d.openrouter_model.is_empty() {
        "Modelo: sin configurar".to_string()
    } else {
        format!("Modelo: {}", esc(&d.openrouter_model))
    }
}

/// Opciones `<option>` del selector de asignatura del modal de tareas, usando
/// las materias reales del estudiante.
fn render_subject_options(d: &PanelData) -> String {
    if d.notas.is_empty() {
        return r##"<option>Sin asignaturas</option>"##.to_string();
    }
    let mut materias: Vec<&str> = d.notas.iter().map(|n| n.materia.as_str()).collect();
    materias.sort();
    materias.dedup();
    materias
        .iter()
        .map(|m| format!("<option>{}</option>", esc(m)))
        .collect::<Vec<_>>()
        .join("")
}

/// Reemplaza los marcadores `<!--SLOT:...-->` por HTML con datos reales.
pub fn fill_panel(content: &str, d: &PanelData) -> String {
    content
        .replace("<!--SLOT:GREETING-->", &render_greeting(d))
        .replace("<!--SLOT:TODAYLINE-->", &render_todayline(d))
        .replace("<!--SLOT:AI_MODEL-->", &render_ai_model(d))
        .replace("<!--SLOT:SUBJECT_OPTIONS-->", &render_subject_options(d))
        .replace("<!--SLOT:SYNC-->", &render_sync(d))
        .replace("<!--SLOT:CLASSCOUNT-->", &render_classcount(d))
        .replace("<!--SLOT:CLASSES-->", &render_classes(d))
        .replace("<!--SLOT:TASKCOUNT-->", &render_taskcount(d))
        .replace("<!--SLOT:TASKTODAY-->", &render_tasks_today(d))
        .replace("<!--SLOT:NOTES-->", &render_notes(d))
        .replace("<!--SLOT:AI_RESUME-->", &render_ai_resume(d))
        .replace("<!--SLOT:TASKTABLE-->", &render_tasktable(d))
        .replace("<!--SLOT:TASKFOOTER-->", &render_taskfooter(d))
        .replace("<!--SLOT:WEEKTITLE-->", &render_weektitle())
        .replace("<!--SLOT:WEEKCAL-->", &render_weekcal(d))
        .replace("<!--SLOT:EXAM-->", &render_exam(d))
        .replace("<!--SLOT:FOCUS-->", &render_focus(d))
}

// ── shell (topbar + aside): datos reales en todas las páginas ──────────────

/// Datos reales para el shell común (topbar/aside) de todas las páginas.
///
/// La API pública no cambia: `mcp_ok` es `false` cuando el servidor MCP no
/// dejó sesión viva (caída o presupuesto agotado).
#[derive(Clone)]
pub struct ShellData {
    pub name: String,
    /// Iniciales del usuario real (p.ej. "LG" para "Lennin Gabriel Salas").
    /// Vacío si no hay perfil: el shell dibuja el icono de persona.
    pub initial: String,
    pub role: String,
    pub mcp_ok: bool,
    pub mcp_count: usize,
    pub vault_pct: String,
}

/// Iniciales del usuario a partir de su nombre real.
///
/// "Lennin Gabriel Salas Arcos" -> "LA" (primera letra del primer nombre y
/// del último). Con un solo nombre -> esa letra. Sin nombre -> cadena vacía
/// (nunca un texto fijo inventado).
fn iniciales_de(nombre: &str) -> String {
    let palabras: Vec<&str> = nombre.split_whitespace().collect();
    let inicial = |p: &str| {
        p.chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default()
    };
    let primero = match palabras.first() {
        Some(p) => *p,
        None => return String::new(),
    };
    match palabras.last() {
        Some(ultimo) if *ultimo != primero => {
            format!("{}{}", inicial(primero), inicial(ultimo))
        }
        _ => inicial(primero),
    }
}

/// Recolecta el estado del shell: perfil del caché y estado MCP.
///
/// El perfil viene del **cliente compartido**: `get_perfil` sale de su caché
/// de 180 s, así que un render normal no abre llamadas nuevas. Si el MCP no
/// responde, el shell muestra un neutro "Estudiante" en lugar de inventar un
/// nombre.
///
/// Como en el panel, la fase MCP va dentro del presupuesto global y el
/// resultado se cachea en memoria: una segunda página no vuelve a preguntar
/// al servidor.
pub async fn fetch_shell_data(state: &AppState) -> ShellData {
    let url = state.config.mikareno_mcp_url.clone();
    let cached = cache_tomar_shell(&url);
    let mut d = cached.clone().unwrap_or(ShellData {
        name: "Estudiante".to_string(),
        initial: String::new(),
        role: String::new(),
        mcp_ok: false,
        mcp_count: 1, // el servidor MCP nativo de la app siempre corre
        vault_pct: "0".to_string(),
    });
    d.mcp_count = 1; // el servidor MCP nativo de la app siempre corre
    if !url.is_empty() {
        d.mcp_count += 1;
    }
    if !state.config.genoffice_mcp_url.is_empty() {
        d.mcp_count += 1;
    }

    d.vault_pct = vault_usage_pct(state).await;

    if cached.is_none() {
        // Si el panel acaba de traer los datos del MCP, el shell los reutiliza
        // (el nombre y el estado de sesión ya están ahí): el /dashboard no
        // paga dos presupuuestos, uno por función.
        if let Some(panel) = cache_tomar_panel(&url) {
            shell_desde_panel(&mut d, &panel);
            cache_guardar_shell(&url, &d);
            return d;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(PANEL_BUDGET_SECS);
        if let Some(client) = crate::academic::mikareno_client(state) {
            // Sesión viva del cliente compartido (cache 180 s entre requests).
            match abrir_sesion_mcp(&client, &url, deadline).await {
                Sesion::Viva => {
                    d.mcp_ok = true;
                    // `get_perfil` sale de la caché de 180 s del cliente: no es
                    // una llamada nueva por render.
                    if let Some(p) = mcp_try!(deadline, client, client.perfil()) {
                        if !p.nombre.is_empty() {
                            d.name = p.nombre.clone();
                            d.initial = iniciales_de(&p.nombre);
                            d.role = p.curso;
                        }
                    }
                }
                // Sin sesión (servidor caído o presupuesto agotado): el shell
                // se queda con el nombre neutro.
                Sesion::Fallo | Sesion::Cortada => {
                    d.mcp_ok = false;
                }
            }
        }
        cache_guardar_shell(&url, &d);
    }

    d
}

/// Rellena el shell con los datos que el panel ya trajo del MCP.
///
/// El nombre, el curso y el estado de la sesión vienen en el panel: no hace
/// falta volver a preguntar al servidor (ni pagar otro presupuesto).
fn shell_desde_panel(d: &mut ShellData, panel: &PanelData) {
    d.mcp_ok = panel.connected;
    if let Some(p) = panel.perfil.as_ref() {
        if !p.nombre.is_empty() {
            d.name = p.nombre.clone();
            d.initial = iniciales_de(&p.nombre);
            d.role = p.curso.clone();
        }
    }
}

/// Inyecta el shell con datos reales en el HTML final de cada página.
pub fn fill_shell(html: &str, d: &ShellData) -> String {
    let avatar = if d.initial.is_empty() {
        r#"<div class="w-8 h-8 rounded-full bg-surface-container-highest flex items-center justify-center"><span class="material-symbols-outlined text-[18px] text-on-surface-variant">person</span></div>"#.to_string()
    } else {
        // Dos iniciales (p.ej. "LA") necesitan un cuerpo algo menor para
        // caber en el círculo de 32 px sin recortarse.
        let size = if d.initial.chars().count() > 1 {
            "text-[11px]"
        } else {
            "text-label-md"
        };
        format!(
            r#"<div class="w-8 h-8 rounded-full bg-primary-container/20 ring-1 ring-primary/40 flex items-center justify-center font-label-md {size} text-primary-container font-semibold">{inicial}</div>"#,
            size = size,
            inicial = esc(&d.initial)
        )
    };
    let status = if d.mcp_ok {
        r#"<span class="w-2 h-2 rounded-full bg-secondary animate-pulse"></span><span class="font-label-sm text-label-sm text-on-surface-variant">En línea · Mikareno conectado</span>"#.to_string()
    } else {
        r#"<span class="w-2 h-2 rounded-full bg-outline"></span><span class="font-label-sm text-label-sm text-on-surface-variant">En línea · Mikareno sin conexión</span>"#.to_string()
    };
    let role = if d.role.is_empty() {
        "Panel de estudios".to_string()
    } else {
        esc(&d.role)
    };
    let nav_badge = format!("{} integradas", d.mcp_count);
    html.replace("<!--SLOT:SHELL_AVATAR-->", &avatar)
        .replace("<!--SLOT:SHELL_NAME-->", &esc(&d.name))
        .replace("<!--SLOT:SHELL_ROLE-->", &role)
        .replace("<!--SLOT:SHELL_STATUS-->", &status)
        .replace("<!--SLOT:SHELL_NAV_MCP-->", &nav_badge)
        .replace("<!--SLOT:SHELL_VAULT_PCT-->", &d.vault_pct)
}

/// Porcentaje de uso del directorio de notes (~ / 10 GB honesto como techo).
async fn vault_usage_pct(state: &AppState) -> String {
    const CEILING_BYTES: u64 = 10 * 1024 * 1024 * 1024; // 10 GB honesto como techo
    fn dir_size(path: &std::path::Path) -> u64 {
        let mut total: u64 = 0;
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_dir() {
                        total += dir_size(&entry.path());
                    } else {
                        total += meta.len();
                    }
                }
            }
        }
        total
    }
    let used = dir_size(&state.config.notes_dir);
    let pct = if CEILING_BYTES == 0 {
        0.0
    } else {
        (used as f64 / CEILING_BYTES as f64) * 100.0
    };
    if pct.fract() == 0.0 {
        format!("{}", pct as u64)
    } else {
        format!("{:.1}", pct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evento(fecha: &str, titulo: &str, tipo: &str) -> CalendarioEvent {
        CalendarioEvent {
            fecha: fecha.to_string(),
            evento: titulo.to_string(),
            tipo: tipo.to_string(),
        }
    }

    #[test]
    fn iniciales_salen_del_nombre_real() {
        assert_eq!(iniciales_de("Lennin Gabriel Salas Arcos"), "LA");
        assert_eq!(iniciales_de("Lennin"), "L");
        // Un nombre con acentos no debe cortar por byte.
        assert_eq!(iniciales_de("Ángela Pérez"), "ÁP");
        // Sin nombre no hay iniciales inventadas.
        assert_eq!(iniciales_de(""), "");
        assert_eq!(iniciales_de("   "), "");
    }

    #[test]
    fn next_event_toma_el_primer_evento_real_desde_hoy() {
        let mut d = PanelData::default();
        let hoy = chrono::Local::now().format("%Y-%m-%d").to_string();
        let manana = (chrono::Local::now().date_naive() + chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        d.eventos = vec![
            evento("2020-01-01", "Clases", "SCHOOL_DAY"),
            evento(&hoy, "Clases", "SCHOOL_DAY"),
            evento(&manana, "Examen de Física", "EXAM"),
        ];
        let ev = next_calendar_event(&d).expect("debe haber evento");
        assert_eq!(ev.evento, "Examen de Física");
    }

    #[test]
    fn next_event_ignora_los_dias_pasados() {
        let mut d = PanelData::default();
        d.eventos = vec![evento("2020-01-01", "Clases", "SCHOOL_DAY")];
        assert!(next_calendar_event(&d).is_none());
    }

    #[test]
    fn next_event_es_none_sin_calendario() {
        let d = PanelData::default();
        assert!(next_calendar_event(&d).is_none());
    }

    #[test]
    fn task_stats_dejan_none_si_no_vienen() {
        let mut d = PanelData::default();
        parse_tarea_stats(&serde_json::json!({}), &mut d);
        assert!(d.tasks_today.is_none());
        assert!(d.tasks_week.is_none());

        parse_tarea_stats(
            &serde_json::json!({"tarea_stats": {"todaysTasks": 2, "tasksCurrentWeek": 5}}),
            &mut d,
        );
        assert_eq!(d.tasks_today, Some(2));
        assert_eq!(d.tasks_week, Some(5));
    }

    #[test]
    fn sin_datos_de_mikareno_no_se_inventa_ningun_cero() {
        let d = PanelData::default();
        // El conteo de tareas dice "sin conexión", no "0 pendientes".
        assert_eq!(render_taskcount(&d), "Mikareno sin conexión");
        // La tabla de tareas lo dice con claridad y sin afirmar que no haya.
        assert!(render_tasktable(&d).contains("no se puede saber"));
    }

    #[test]
    fn shell_sin_perfil_no_dibuja_iniciales_fijas() {
        let d = ShellData {
            name: "Estudiante".to_string(),
            initial: String::new(),
            role: String::new(),
            mcp_ok: false,
            mcp_count: 1,
            vault_pct: "0".to_string(),
        };
        let html = fill_shell("<div><!--SLOT:SHELL_AVATAR--></div>", &d);
        assert!(html.contains("person"), "debe caer al icono de persona");
        assert!(!html.contains(">L<"), "no debe haber iniciales fijas");
    }

    #[test]
    fn shell_dibuja_las_iniciales_del_perfil() {
        let d = ShellData {
            name: "Lennin Gabriel Salas Arcos".to_string(),
            initial: iniciales_de("Lennin Gabriel Salas Arcos"),
            role: "10A".to_string(),
            mcp_ok: true,
            mcp_count: 3,
            vault_pct: "1.5".to_string(),
        };
        let html = fill_shell(
            "<!--SLOT:SHELL_AVATAR--><!--SLOT:SHELL_NAME--><!--SLOT:SHELL_VAULT_PCT-->",
            &d,
        );
        assert!(html.contains(">LA<"), "debe mostrar las iniciales reales");
        assert!(html.contains("Lennin Gabriel Salas Arcos"));
        // El badge del vault lo sigue llenando fill_shell.
        assert!(html.contains("1.5"));
    }

    // ── presupuesto de render y caché en memoria ───────────────────────

    /// Serializa los tests de caché: la caché es un global compartido y los
    /// tests de un mismo binario corren en paralelo.
    static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_cache() -> std::sync::MutexGuard<'static, ()> {
        CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn el_presupuesto_de_render_cabe_en_cuatro_segundos() {
        assert!(
            PANEL_BUDGET_SECS <= 4,
            "la fase MCP del panel no puede pasar de 4 s"
        );
    }

    #[test]
    fn la_cache_del_panel_sirve_durante_el_ttl_y_se_invalida() {
        let _lock = lock_cache();
        // Un solo test para la caché: es un global compartido y los tests
        // corren en paralelo (si no, se pisan la entrada).
        let url = "http://127.0.0.1:59101/mcp";
        let mut d = PanelData::default();
        d.connected = true;
        cache_guardar_panel(url, &d);

        let hit = cache_tomar_panel(url).expect("la caché debe servir");
        assert!(hit.connected, "debe devolver los datos cacheados");

        // Otra URL no puede reutilizar los datos de este MCP.
        assert!(
            cache_tomar_panel("http://127.0.0.1:59103/mcp").is_none(),
            "otra URL no puede reutilizar los datos"
        );

        // Pasado el TTL, la entrada ya no sirve: se vuelve a preguntar al MCP.
        let viejo = Instant::now()
            .checked_sub(Duration::from_secs(PANEL_CACHE_TTL_SECS + 1))
            .unwrap_or_else(Instant::now);
        cache_guardar_panel_en(url, &d, viejo);
        assert!(
            cache_tomar_panel(url).is_none(),
            "pasado el TTL la caché no debe servir"
        );
    }

    #[test]
    fn la_cache_degradada_se_reintenta_antes() {
        let _lock = lock_cache();
        let url = "http://127.0.0.1:59104/mcp";
        let mut d = PanelData::default();
        d.mcp_timeout = true;
        cache_guardar_panel(url, &d);

        // Dentro del TTL degradado sigue sirviendo...
        let hit = cache_tomar_panel(url).expect("la caché degradada sirve");
        assert!(hit.mcp_timeout);

        // ...pero se reintenta antes que una entrada sana.
        let viejo = Instant::now()
            .checked_sub(Duration::from_secs(PANEL_CACHE_TTL_DEGRADADO_SECS + 1))
            .unwrap_or_else(Instant::now);
        cache_guardar_panel_en(url, &d, viejo);
        assert!(
            cache_tomar_panel(url).is_none(),
            "una entrada degradada debe expirar antes"
        );

        // Una entrada sana con la misma antigüedad sigue sirviendo.
        let mut sana = PanelData::default();
        sana.connected = true;
        cache_guardar_panel_en(url, &sana, viejo);
        assert!(
            cache_tomar_panel(url).is_some(),
            "una entrada sana aguanta más que una degradada"
        );
    }

    #[test]
    fn el_tiempo_agotado_dice_que_mikareno_no_responde() {
        let mut d = PanelData::default();
        d.mcp_timeout = true;
        assert_eq!(render_sync(&d), "Mikareno: no responde ahora");
        assert_eq!(render_taskcount(&d), "Mikareno no responde ahora");
        // Sin inventar ceros: sigue diciendo que no se puede saber.
        assert!(render_tasktable(&d).contains("no se puede saber"));
        assert!(render_tasktable(&d).contains("no responde ahora"));
        assert!(render_tasks_today(&d).contains("no se puede saber"));
    }

    #[test]
    fn sin_timeout_el_panel_sigue_diciendo_sin_conexion() {
        let d = PanelData::default();
        assert_eq!(render_sync(&d), "Mikareno: sin conexión");
        assert_eq!(render_taskcount(&d), "Mikareno sin conexión");
    }
}
