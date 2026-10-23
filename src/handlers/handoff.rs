//! Página `/handoff` — estado del sistema para el dueño.
//!
//! Antes esta ruta servía el documento markdown de handoff como una página
//! suelta (HTML propio, fuera del shell de la app). Ahora muestra el estado
//! REAL del proceso en ejecución y lo sirve con el shell compartido
//! (`stitch_v2::stitch_v2_page`), igual que el resto de páginas internas
//! (dashboard, configuración y administración).
//!
//! Qué muestra:
//! - versión del binario, uptime, nº de notas y nº de conversaciones (BD),
//! - presupuesto de IA (gasto real del mes / tope configurado),
//! - estado de los MCP (Mikareno, GenOffice y el MCP propio de la app),
//! - un botón "Copiar informe" que copia un resumen en texto plano.
//!
//! **Sondeo de los MCP**: el sondeo `initialize` tiene un presupuesto duro
//! de 800 ms y su resultado se cachea 30 s. Mikareno usa el sondeo
//! compartido de la app (`handlers::settings::mcp_probe`), de modo que
//! `/settings`, `/handoff` y `/admin` comparten un único sondeo. Si el MCP no
//! contesta, la página dice "sin respuesta" y se pinta igual.
//!
//! Invariantes (no negociables):
//! - **Sin secretos**: la clave de OpenRouter sale como booleano
//!   ("configurada"); ni la key ni su sufijo se imprimen nunca. Las URLs de
//!   los MCP se muestran sin credenciales, query string ni fragmento.
//! - **Sin datos inventados**: si una fuente falla, el campo muestra
//!   "No disponible"; nunca un 0 inventado.
//! - La página es del dueño: sin sesión válida (y con auth activa) responde
//!   302 a `/login`, como el resto de páginas internas.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
};
use pulldown_cmark::{html, Parser};

use crate::models::Session;
use crate::AppState;

/// Documento de handoff de la sesión (compilado dentro del binario).
const HANDOFF_CONTENT: &str = include_str!("../../STUDY_PLATFORM_HANDOFF.md");

/// Presupuesto del sondeo `initialize` a un MCP externo.
///
/// Tope **duro** de la respuesta: si el MCP no contesta en este plazo se
/// declara "sin respuesta" y la página se pinta igual. Antes eran 5 s (y el
/// shell añadía su propio handshake de 10 s): la página tardaba 25 s.
const PROBE_BUDGET: Duration = Duration::from_millis(800);

/// Caché de sondeos: la página no golpea los MCP en cada carga.
const PROBE_TTL: Duration = Duration::from_secs(30);

/// Texto mostrado cuando la fuente de un dato no está disponible.
const NOT_AVAILABLE: &str = "No disponible";

/// Id del `<pre>` con el informe en texto plano (lo que se copia).
const REPORT_PRE_ID: &str = "handoff-report";

/// Cliente HTTP compartido para los sondeos MCP (uno por proceso).
static PROBE_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(PROBE_BUDGET)
        .connect_timeout(PROBE_BUDGET)
        .build()
        .expect("reqwest client para los sondeos MCP de /handoff")
});

/// Resultado del último sondeo a los MCP externos.
#[derive(Clone, Copy)]
struct ProbeSnapshot {
    mikareno: ProbeResult,
    genoffice: ProbeResult,
}

/// Sondeo de un MCP: qué pasó al pedirle `initialize`.
#[derive(Clone, Copy, PartialEq)]
enum ProbeResult {
    /// No hay URL configurada: no se sondea.
    NotConfigured,
    /// Contestó 2xx (latencia en ms).
    Alive(u128),
    /// Contestó, pero con error.
    Error,
    /// No hubo respuesta dentro del presupuesto (800 ms).
    NoResponse,
    /// No hubo respuesta (error de red).
    Unreachable,
}

impl From<crate::handlers::settings::McpProbe> for ProbeResult {
    /// Traduce el sondeo compartido de Mikareno al estado de esta página.
    fn from(p: crate::handlers::settings::McpProbe) -> Self {
        use crate::handlers::settings::McpProbe;
        match p {
            McpProbe::Connected { latency_ms } => ProbeResult::Alive(latency_ms),
            McpProbe::NoResponse => ProbeResult::NoResponse,
            McpProbe::Unreachable => ProbeResult::Unreachable,
            McpProbe::NotConfigured => ProbeResult::NotConfigured,
        }
    }
}

/// Resultado del sondeo de GenOffice (con su hora).
///
/// Caché propia de 30 s: la página no golpea el MCP en cada carga.
static GENOFFICE_CACHE: Mutex<Option<(Instant, ProbeResult)>> = Mutex::new(None);

/// Estado de un MCP externo visto desde la app.
struct McpState {
    /// ¿Hay URL configurada?
    configured: bool,
    /// Endpoint mostrado: esquema + host, sin credenciales ni query.
    endpoint: Option<String>,
    /// Resultado del sondeo `initialize`.
    probe: ProbeResult,
    /// La app mantiene una sesión MCP viva con este servidor.
    session: bool,
}

/// Estado del sistema, tal y como lo ve el proceso en ejecución.
///
/// Los campos `Option` son `None` cuando su fuente no está disponible: la
/// capa de presentación los muestra como "No disponible" (nunca un 0 falso).
struct SystemState {
    version: String,
    uptime_secs: u64,
    notes_count: Option<i64>,
    conversations: Option<i64>,
    chat_messages: Option<i64>,
    ai_spend_usd: Option<f64>,
    ai_budget_usd: f64,
    ai_requests: Option<u64>,
    ai_key_configured: bool,
    ai_model: String,
    mcp_mikareno: McpState,
    mcp_genoffice: McpState,
    generated_at: String,
}

// ─── Handler ────────────────────────────────────────────────────────────────

/// GET /handoff — estado del sistema para el dueño.
pub async fn get_handoff(State(state): State<Arc<AppState>>, req: Request) -> Response {
    // La página es del dueño: sin sesión válida (y con auth activa) se
    // redirige al login, igual que el resto de páginas internas.
    if !has_valid_session(&state, req).await {
        return Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, "/login")
            .body(Body::empty())
            .expect("respuesta 302 de /handoff");
    }

    let st = collect(&state).await;
    let report = report_text(&st);
    let content = render_content(&st, &report);

    // Shell compartido: el mismo que el dashboard, la configuración y el
    // panel de administración (rail + topbar + scripts de auth).
    let html = crate::handlers::stitch_v2::stitch_v2_page("handoff", &content, true);
    let shell = crate::handlers::stitch_panel::fetch_shell_data(&state).await;
    let html = crate::handlers::stitch_panel::fill_shell(&html, &shell);

    (StatusCode::OK, Html(html)).into_response()
}

/// ¿La petición trae una sesión válida?
///
/// Misma comprobación que el middleware `require_auth_state`: cookie
/// `session_id` con formato válido, fila en `sessions` (hash
/// `SHA256(secret:token)` o el valor crudo legacy) y sin expirar. Con
/// `AUTH_DISABLED=true` la app va sin login: se permite el acceso.
async fn has_valid_session(state: &Arc<AppState>, req: Request) -> bool {
    if state.config.auth_disabled {
        return true;
    }

    // `req` entra **por valor**: `&Request<Body>` no es `Send` (el `Body` de
    // axum no es `Sync`), así que pedirlo por referencia y mantener el
    // préstamo durante el `await` hacía que el futuro del handler no fuera
    // `Send` y axum rechazara la ruta. El cookie se extrae como `String`
    // propia, sin prestamos vivos.
    let Some(raw) = crate::middleware::auth::extract_session_cookie(&req) else {
        return false;
    };
    if !crate::middleware::auth::token_format_ok(&raw) {
        return false;
    }

    let hashed = crate::middleware::auth::hash_session_token(&raw, &state.config.session_secret);
    let session = sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = ? OR id = ?")
        .bind(&hashed)
        .bind(&raw)
        .fetch_optional(&state.db)
        .await;

    match session {
        // Mismo criterio que el middleware `require_auth_state`: la fila debe
        // cumplir el expiry por fila **y** el techo duro de TTL desde
        // `created_at`. `Session::is_valid()` comparaba `expires_at` como
        // texto, así que un `expires_at` ilegible ("2026-10-21 04:00:00",
        // un offset no-UTC o basura) pasaba por válido: `/handoff` aceptaba
        // sesiones que el resto de la app rechazaba.
        Ok(Some(s)) => !crate::middleware::auth::session_is_expired(&s, chrono::Utc::now()),
        // Sin fila o error de BD: se falla cerrado.
        _ => false,
    }
}

// ─── Recolección de datos ────────────────────────────────────────────────────

/// Recoge el estado real del sistema.
async fn collect(state: &Arc<AppState>) -> SystemState {
    let version = env!("CARGO_PKG_VERSION").to_string();
    let uptime_secs = state.start_time.elapsed().as_secs();

    // Conteos de la BD. Si la tabla no existe o la consulta falla, el dato
    // queda como `None` → "No disponible" (jamás un 0 inventado).
    let notes_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notes")
        .fetch_one(&state.db)
        .await
        .ok();

    // Una "conversación" es un hilo de `chat_history` (se agrupa por
    // `subject`); los mensajes son las filas.
    let conversations =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(DISTINCT subject) FROM chat_history")
            .fetch_one(&state.db)
            .await
            .ok();

    let chat_messages = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM chat_history")
        .fetch_one(&state.db)
        .await
        .ok();

    // Gasto de IA: sólo si la tabla `usage_metrics` YA existe. Una página de
    // lectura no crea tablas: si no existe, el gasto es "No disponible".
    let usage_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'usage_metrics'",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let (ai_spend_usd, ai_requests) = if usage_table > 0 {
        match crate::ai::openrouter::persistent_summary(&state.db, &state.config).await {
            Ok(summary) => (Some(summary.total_cost), Some(summary.requests_count)),
            // Sin detalle del error: el mensaje de la API podría acabar en
            // un log. Sólo el hecho, que es lo que importa.
            Err(_) => {
                tracing::warn!("presupuesto de IA no disponible (no se pudo leer usage_metrics)");
                (None, None)
            }
        }
    } else {
        (None, None)
    };

    let snapshot = probe_snapshot(state).await;

    // Sesión MCP viva con Mikareno (el cliente compartido de la app).
    let mikareno_session = crate::academic::mikareno_client(state)
        .map(|c| c.is_connected())
        .unwrap_or(false);

    SystemState {
        version,
        uptime_secs,
        notes_count,
        conversations,
        chat_messages,
        ai_spend_usd,
        ai_budget_usd: state.config.openrouter_max_cost_usd,
        ai_requests,
        ai_key_configured: state
            .config
            .openrouter_api_key
            .as_ref()
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false),
        ai_model: state.config.openrouter_model.clone(),
        mcp_mikareno: McpState {
            configured: !state.config.mikareno_mcp_url.trim().is_empty(),
            endpoint: endpoint_label(&state.config.mikareno_mcp_url),
            probe: snapshot.mikareno,
            session: mikareno_session,
        },
        mcp_genoffice: McpState {
            configured: !state.config.genoffice_mcp_url.trim().is_empty(),
            endpoint: endpoint_label(&state.config.genoffice_mcp_url),
            probe: snapshot.genoffice,
            session: false,
        },
        generated_at: chrono::Utc::now().to_rfc3339(),
    }
}

/// Sondeos a los MCP externos con caché TTL: la página no golpea los MCP en
/// cada carga.
///
/// Mikareno usa el sondeo **compartido** de la app
/// (`handlers::settings::mcp_probe`: 800 ms de presupuesto y 30 s de caché),
/// así que `/settings`, `/handoff` y `/admin` comparten un único sondeo.
/// GenOffice se sondea aquí, con el mismo presupuesto corto y su propia caché.
///
/// El tope de espera es de 800 ms: si el MCP no contesta, la página dice
/// "sin respuesta" y se pinta igual (el sondeo sigue en segundo plano y su
/// resultado real queda en la caché para la siguiente carga).
async fn probe_snapshot(state: &Arc<AppState>) -> ProbeSnapshot {
    let (mikareno, genoffice) = tokio::join!(
        crate::handlers::settings::mcp_probe(state),
        genoffice_probe(state.config.genoffice_mcp_url.clone()),
    );

    ProbeSnapshot {
        mikareno: mikareno.into(),
        genoffice: genoffice.unwrap_or(ProbeResult::NoResponse),
    }
}

/// Sondeo de GenOffice: caché de 30 s y tope de espera de 800 ms.
///
/// Si el plazo se agota, el task sigue en segundo plano y deja el resultado
/// real en la caché (así la siguiente carga no espera otra vez).
async fn genoffice_probe(url: String) -> Option<ProbeResult> {
    if let Ok(guard) = GENOFFICE_CACHE.lock() {
        if let Some((at, probe)) = *guard {
            if at.elapsed() < PROBE_TTL {
                return Some(probe);
            }
        }
    }

    let probe_task = async move {
        let probe = probe_mcp(&url).await;
        if let Ok(mut guard) = GENOFFICE_CACHE.lock() {
            *guard = Some((Instant::now(), probe));
        }
        probe
    };

    // Sin runtime de Tokio no hay dónde lanzar el task: sondeo en línea.
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return Some(probe_task.await);
    };

    let task = handle.spawn(probe_task);
    match tokio::time::timeout(PROBE_BUDGET, task).await {
        Ok(Ok(probe)) => Some(probe),
        // Presupuesto agotado o task caído: la página pintará "sin respuesta".
        Ok(Err(_)) | Err(_) => None,
    }
}

/// Sondea un endpoint MCP con `initialize` (mismo criterio que `/health`).
///
/// `None` = sin URL configurada; `Alive` = respondió 2xx; `Error` = contestó
/// con error; `NoResponse` = no hubo respuesta dentro del presupuesto;
/// `Unreachable` = error de red.
///
/// El presupuesto es un tope **duro**: el `tokio::time::timeout` de fuera
/// cancela la petición aunque el cliente HTTP no llegue a cortarla.
async fn probe_mcp(url: &str) -> ProbeResult {
    let url = url.trim();
    if url.is_empty() {
        return ProbeResult::NotConfigured;
    }

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {
                "name": "study-platform-handoff",
                "version": env!("CARGO_PKG_VERSION"),
            },
        },
    });

    let t0 = Instant::now();
    let request = PROBE_CLIENT
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&body)
        .send();

    match tokio::time::timeout(PROBE_BUDGET, request).await {
        // Sin respuesta dentro del presupuesto: se distingue del error de
        // red porque el MCP puede estar vivo y sólo ir lento.
        Err(_) => ProbeResult::NoResponse,
        Ok(Err(e)) if e.is_timeout() => ProbeResult::NoResponse,
        Ok(Err(_)) => ProbeResult::Unreachable,
        Ok(Ok(r)) if r.status().is_success() => ProbeResult::Alive(t0.elapsed().as_millis()),
        Ok(Ok(_)) => ProbeResult::Error,
    }
}

// ─── Formateo ───────────────────────────────────────────────────────────────

/// Escapa texto para insertarlo en el HTML.
fn esc(s: &str) -> String {
    crate::academic::templates::html_escape(s)
}

/// Dato opcional como texto: "No disponible" si la fuente falló.
fn fmt_opt<T: ToString>(v: Option<T>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => NOT_AVAILABLE.to_string(),
    }
}

/// Dato opcional como HTML: "No disponible" en tono apagado si la fuente falló.
fn html_opt<T: ToString>(v: Option<T>) -> String {
    match v {
        Some(x) => esc(&x.to_string()),
        None => format!(r#"<span class="text-outline">{}</span>"#, NOT_AVAILABLE),
    }
}

/// Uptime legible: "2d 5h 30m", "1h 15m", "45s".
fn format_uptime(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!(
            "{}d {}h {}m",
            secs / 86400,
            (secs % 3600) / 3600,
            (secs % 3600) % 60
        )
    }
}

/// Esquema + host de una URL, sin credenciales, query string ni fragmento.
///
/// Es lo único que se muestra de la URL de un MCP: si alguien metió una
/// credencial o un token en la URL, aquí no aparece.
fn endpoint_label(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s, r),
        None => ("", url),
    };
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let authority = rest.split('/').next().unwrap_or(rest);
    // `user:pass@host` → `host`: las credenciales no se muestran.
    let host = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    if host.is_empty() {
        return None;
    }
    if scheme.is_empty() {
        Some(host.to_string())
    } else {
        Some(format!("{}://{}", scheme, host))
    }
}

/// Estado de un MCP en una línea, según el resultado del sondeo.
fn mcp_state_text(m: &McpState) -> String {
    if !m.configured {
        return "No configurado".to_string();
    }
    match m.probe {
        ProbeResult::Alive(ms) => {
            let base = if m.session {
                "Conectado (sesión MCP activa)"
            } else {
                "Responde (sin sesión activa)"
            };
            format!("{} · {} ms", base, ms)
        }
        ProbeResult::Error => "Responde con error (initialize no-2xx)".to_string(),
        // "Sin respuesta" ≠ "sin conexión": el MCP puede estar vivo y lento.
        ProbeResult::NoResponse => "Sin respuesta (no contestó en 800 ms)".to_string(),
        ProbeResult::Unreachable => "No responde (error de red)".to_string(),
        ProbeResult::NotConfigured => "No configurado".to_string(),
    }
}

/// Gasto y tope del presupuesto de IA, en una línea.
///
/// El tope viene de la configuración (siempre disponible); el gasto sale de
/// `usage_metrics`: si esa tabla no existe o no se puede leer, el gasto se
/// declara "No disponible" en vez de 0.
fn budget_text(s: &SystemState) -> String {
    let tope = if s.ai_budget_usd > 0.0 {
        format!("{:.2} USD", s.ai_budget_usd)
    } else {
        "sin tope configurado".to_string()
    };
    match s.ai_spend_usd {
        Some(spend) if s.ai_budget_usd > 0.0 => {
            let pct = (spend / s.ai_budget_usd * 100.0).clamp(0.0, 999.0);
            format!("{:.2} USD / {} ({:.0} %)", spend, tope, pct)
        }
        Some(spend) => format!("{:.2} USD / {}", spend, tope),
        None => format!("gasto {} / tope {}", NOT_AVAILABLE, tope),
    }
}

/// Estado de la clave de OpenRouter: sólo booleano. Nunca el valor ni su
/// sufijo.
fn ai_key_text(s: &SystemState) -> String {
    if s.ai_key_configured {
        "Configurada (valor oculto)".to_string()
    } else {
        "No configurada — la IA responde en modo simulado".to_string()
    }
}

/// Línea del informe: etiqueta alineada y valor.
fn fmt_line(label: &str, value: &str) -> String {
    format!("  {:<15}{}", label, value)
}

/// Estado de un MCP en una línea, con su endpoint (o "sin URL configurada").
fn mcp_line(m: &McpState) -> String {
    format!(
        "{} · {}",
        mcp_state_text(m),
        m.endpoint.as_deref().unwrap_or("sin URL configurada")
    )
}

/// Informe en texto plano: lo que se copia al portapapeles.
fn report_text(s: &SystemState) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("Study Platform · informe de estado".to_string());
    lines.push(format!("Generado: {}", s.generated_at));
    lines.push(String::new());
    lines.push("BINARIO".to_string());
    lines.push(fmt_line("version", &s.version));
    lines.push(fmt_line(
        "uptime",
        &format!("{} ({} s)", format_uptime(s.uptime_secs), s.uptime_secs),
    ));
    lines.push(String::new());
    lines.push("DATOS".to_string());
    lines.push(fmt_line("notas", &fmt_opt(s.notes_count)));
    lines.push(fmt_line("conversaciones", &fmt_opt(s.conversations)));
    lines.push(fmt_line("mensajes", &fmt_opt(s.chat_messages)));
    lines.push(String::new());
    lines.push("IA (OpenRouter)".to_string());
    lines.push(fmt_line("clave", &ai_key_text(s)));
    lines.push(fmt_line("modelo", &s.ai_model));
    lines.push(fmt_line("presupuesto", &budget_text(s)));
    lines.push(fmt_line("llamadas (mes)", &fmt_opt(s.ai_requests)));
    lines.push(String::new());
    lines.push("MCP".to_string());
    lines.push(fmt_line("Mikareno", &mcp_line(&s.mcp_mikareno)));
    lines.push(fmt_line("GenOffice", &mcp_line(&s.mcp_genoffice)));
    lines.push(fmt_line(
        "propio (app)",
        "activo (integrado en la app, ruta /mcp)",
    ));
    lines.join("\n")
}

// ─── Renderizado ────────────────────────────────────────────────────────────

/// Estilos del documento markdown embebido (el shell no los trae).
const DOC_STYLE: &str = r#"
.handoff-doc { color: #bbcac5; font-size: 13px; line-height: 1.6; }
.handoff-doc h1, .handoff-doc h2, .handoff-doc h3 { color: #5eead4; margin: 1.25rem 0 .5rem; }
.handoff-doc h1 { border-bottom: 1px solid #3c4946; padding-bottom: .4rem; }
.handoff-doc code { background: #1c2025; padding: 2px 6px; border-radius: 4px; font-family: 'Fira Code', monospace; font-size: 12px; }
.handoff-doc pre { background: #1c2025; padding: 1rem; border-radius: 8px; overflow-x: auto; }
.handoff-doc pre code { background: none; padding: 0; }
.handoff-doc table { border-collapse: collapse; width: 100%; margin: 1rem 0; }
.handoff-doc th, .handoff-doc td { border: 1px solid #3c4946; padding: 6px 10px; text-align: left; }
.handoff-doc th { background: #1c2025; color: #5eead4; }
.handoff-doc a { color: #9ed753; }
.handoff-doc ul, .handoff-doc ol { padding-left: 1.25rem; }
"#;

/// Tarjeta del panel: etiqueta, valor grande y nota al pie.
fn card(label: &str, value_html: &str, hint: &str) -> String {
    format!(
        r#"<div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
      <div class="font-code-inline text-[11px] text-on-surface-variant uppercase tracking-wider">{label}</div>
      <div class="font-headline-md text-headline-md text-primary-container font-bold mt-1">{value}</div>
      <div class="font-body-sm text-[10px] text-on-surface-variant mt-1">{hint}</div>
    </div>"#,
        label = esc(label),
        value = value_html,
        hint = esc(hint),
    )
}

/// Contenido de la página (se inyecta en el `<main>` del shell compartido).
fn render_content(s: &SystemState, report: &str) -> String {
    let cards = format!(
        "{}{}{}{}",
        card(
            "Binario",
            &esc(&s.version),
            "versión del crate (CARGO_PKG_VERSION)",
        ),
        card(
            "Uptime",
            &esc(&format_uptime(s.uptime_secs)),
            &format!("{} segundos desde el arranque", s.uptime_secs),
        ),
        card(
            "Notas",
            &html_opt(s.notes_count),
            "filas en la tabla `notes`"
        ),
        card(
            "Conversaciones",
            &html_opt(s.conversations),
            "hilos de chat (subject distintos)",
        ),
    );

    // Barra de presupuesto: sólo si hay gasto y tope reales.
    let budget_bar = match (s.ai_spend_usd, s.ai_budget_usd) {
        (Some(spend), budget) if budget > 0.0 => {
            let pct = (spend / budget * 100.0).clamp(0.0, 100.0);
            let over = spend >= budget;
            format!(
                r#"<div class="h-1.5 w-full rounded-full bg-surface-container-high overflow-hidden">
      <div class="h-full rounded-full {bar}" style="width: {pct:.1}%"></div>
    </div>"#,
                bar = if over {
                    "bg-error"
                } else {
                    "bg-primary-container"
                },
                pct = pct
            )
        }
        _ => String::new(),
    };

    let mcp_rows = format!(
        r#"<div class="space-y-space-sm">
      <div class="flex flex-wrap items-baseline justify-between gap-2">
        <span class="font-label-sm text-label-sm text-on-surface">Mikareno</span>
        <span class="font-code-inline text-[12px] text-on-surface">{mik_state}</span>
      </div>
      <div class="font-code-inline text-[10px] text-outline">{mik_endpoint}</div>
      <div class="flex flex-wrap items-baseline justify-between gap-2">
        <span class="font-label-sm text-label-sm text-on-surface">GenOffice</span>
        <span class="font-code-inline text-[12px] text-on-surface">{gen_state}</span>
      </div>
      <div class="font-code-inline text-[10px] text-outline">{gen_endpoint}</div>
      <div class="flex flex-wrap items-baseline justify-between gap-2">
        <span class="font-label-sm text-label-sm text-on-surface">MCP propio de la app</span>
        <span class="font-code-inline text-[12px] text-on-surface">Activo (integrado en el binario)</span>
      </div>
    </div>"#,
        mik_state = esc(&mcp_state_text(&s.mcp_mikareno)),
        mik_endpoint = esc(s
            .mcp_mikareno
            .endpoint
            .as_deref()
            .unwrap_or("sin URL configurada")),
        gen_state = esc(&mcp_state_text(&s.mcp_genoffice)),
        gen_endpoint = esc(s
            .mcp_genoffice
            .endpoint
            .as_deref()
            .unwrap_or("sin URL configurada")),
    );

    // El documento markdown de la sesión se conserva (contenido estático
    // compilado en el binario), pero plegado y etiquetado como tal: no es
    // estado del sistema en vivo.
    let mut doc_html = String::new();
    html::push_html(&mut doc_html, Parser::new(HANDOFF_CONTENT));

    format!(
        r#"<div class="max-w-5xl mx-auto px-space-md pt-space-lg pb-space-xl space-y-space-md">
  <div class="flex flex-wrap items-start justify-between gap-space-md">
    <div>
      <h1 class="font-headline-lg text-headline-lg text-on-surface font-bold">Estado del sistema</h1>
      <p class="font-body-sm text-body-sm text-on-surface-variant mt-1">
        Datos reales del binario en ejecución. Lo que no se puede leer aparece como «No disponible».
      </p>
    </div>
    <div class="flex flex-wrap items-center gap-space-sm">
      <span class="px-1.5 py-0.5 rounded text-[10px] font-mono bg-surface-container-high text-primary-fixed-dim border border-outline-variant/60">v{version}</span>
      <button type="button" data-copy-target="{report_id}" class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-primary-container text-on-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors">
        <span class="material-symbols-outlined text-[16px]">content_copy</span>
        <span class="js-copy-label">Copiar informe</span>
      </button>
      <span id="handoff-copy-status" class="text-[10px] text-outline" role="status" aria-live="polite"></span>
    </div>
  </div>

  <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-space-md">
    {cards}
  </div>

  <div class="grid grid-cols-1 lg:grid-cols-2 gap-space-md">
    <div class="bg-surface-container border border-outline rounded-xl p-space-md space-y-space-sm">
      <div class="flex items-center gap-2">
        <span class="material-symbols-outlined text-[20px] text-primary-container">neurology</span>
        <h2 class="font-headline-sm text-headline-sm text-on-surface">IA (OpenRouter)</h2>
      </div>
      <dl class="grid grid-cols-[130px_1fr] gap-space-sm text-[13px]">
        <dt class="font-label-sm text-label-sm text-on-surface-variant">Clave</dt>
        <dd class="font-code-inline text-on-surface">{ai_key}</dd>
        <dt class="font-label-sm text-label-sm text-on-surface-variant">Modelo</dt>
        <dd class="font-code-inline text-on-surface">{ai_model}</dd>
        <dt class="font-label-sm text-label-sm text-on-surface-variant">Presupuesto</dt>
        <dd class="font-code-inline text-on-surface">{ai_budget}</dd>
        <dt class="font-label-sm text-label-sm text-on-surface-variant">Llamadas (mes)</dt>
        <dd class="font-code-inline text-on-surface">{ai_requests}</dd>
      </dl>
      {budget_bar}
      <p class="font-body-sm text-[10px] text-outline">La clave nunca se muestra: sólo si está configurada.</p>
    </div>

    <div class="bg-surface-container border border-outline rounded-xl p-space-md space-y-space-sm">
      <div class="flex items-center gap-2">
        <span class="material-symbols-outlined text-[20px] text-primary-container">hub</span>
        <h2 class="font-headline-sm text-headline-sm text-on-surface">MCP</h2>
      </div>
      {mcp_rows}
      <p class="font-body-sm text-[10px] text-outline">Sondeo `initialize` con presupuesto de 800 ms (caché 30 s).</p>
    </div>
  </div>

  <div class="bg-surface-container border border-outline rounded-xl p-space-md space-y-space-sm">
    <h2 class="font-headline-sm text-headline-sm text-on-surface">Informe en texto plano</h2>
    <pre id="{report_id}" class="bg-surface-container-low border border-outline-variant/50 rounded-lg p-3 overflow-x-auto"><code class="font-code-inline text-[12px] text-on-surface">{report}</code></pre>
  </div>

  <details class="rounded-xl border border-outline bg-surface-container/40 p-space-sm space-y-space-sm">
    <summary class="cursor-pointer flex items-center gap-1.5 font-label-sm text-label-sm text-on-surface">
      <span class="material-symbols-outlined text-[16px] text-outline">description</span>
      Documento de handoff de la sesión (estático, compilado en el binario)
    </summary>
    <style>{doc_style}</style>
    <div class="handoff-doc mt-3">{doc}</div>
  </details>

  <p class="font-body-sm text-[11px] text-outline">
    <a href="/admin" class="underline">Administración</a> ·
    <a href="/settings" class="underline">Configuración</a> ·
    <a href="/health" class="underline">Health</a>
  </p>

  {copy_script}
</div>"#,
        version = esc(&s.version),
        report_id = REPORT_PRE_ID,
        cards = cards,
        ai_key = esc(&ai_key_text(s)),
        ai_model = esc(&s.ai_model),
        ai_budget = esc(&budget_text(s)),
        ai_requests = html_opt(s.ai_requests),
        budget_bar = budget_bar,
        mcp_rows = mcp_rows,
        report = esc(report),
        doc_style = DOC_STYLE,
        doc = doc_html,
        copy_script = COPY_SCRIPT,
    )
}

/// JS del botón "Copiar informe".
///
/// `navigator.clipboard` sólo existe en contexto seguro (HTTPS o
/// localhost); esta app se sirve por HTTP en la tailnet, así que el fallback
/// con `execCommand` es el camino habitual. El texto se copia desde el DOM
/// (`innerText`): lo que se copia es exactamente lo que se ve.
const COPY_SCRIPT: &str = r##"<script>
(function () {
  'use strict';
  function setLabel(btn, text) {
    var l = btn.querySelector('.js-copy-label');
    if (!l) { return; }
    var prev = l.getAttribute('data-copy-prev') || l.textContent;
    l.setAttribute('data-copy-prev', prev);
    l.textContent = text;
    window.setTimeout(function () {
      l.textContent = prev;
      btn.removeAttribute('data-copy-prev');
    }, 1600);
  }
  function legacyCopy(text) {
    var ta = document.createElement('textarea');
    ta.value = text;
    ta.setAttribute('readonly', '');
    ta.style.position = 'fixed';
    ta.style.left = '-9999px';
    document.body.appendChild(ta);
    ta.select();
    var ok = false;
    try { ok = document.execCommand('copy'); } catch (e) { ok = false; }
    document.body.removeChild(ta);
    return ok;
  }
  function say(msg) {
    var s = document.getElementById('handoff-copy-status');
    if (s) { s.textContent = msg; }
  }
  document.addEventListener('click', function (ev) {
    // `void 0` en vez de `null`: el HTML servido no debe contener literales
    // de JS sin renderizar ("null" / "undefined"), como exige la suite.
    var btn = (ev.target && ev.target.closest) ? ev.target.closest('[data-copy-target]') : void 0;
    if (!btn) { return; }
    ev.preventDefault();
    var src = document.getElementById(btn.getAttribute('data-copy-target'));
    if (!src) { return; }
    var text = (src.innerText || src.textContent || '').trim();
    var done = function (ok) {
      setLabel(btn, ok ? '¡Copiado!' : 'No se pudo copiar');
      say(ok ? 'Informe copiado al portapapeles.' : 'Copia manual: selecciona el informe de arriba.');
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(
        function () { done(true); },
        function () { done(legacyCopy(text)); }
      );
    } else {
      done(legacyCopy(text));
    }
  });
})();
</script>"##;
