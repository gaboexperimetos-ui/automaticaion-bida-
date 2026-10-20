//! Página de Configuración (Stitch v2).
//!
//! GET /settings — muestra los valores reales de configuración del proceso
//! (modelo de IA, URL del MCP, modo auth, versión) y el estado de conexión al
//! MCP de Mikareno. El presupuesto mensual se muestra tal cual
//! (`openrouter_max_cost_usd`); el gasto real lo expone
//! `GET /api/admin/metrics`.
//!
//! POST /api/settings/ai (`save_ai`) — valida el formulario de parámetros de
//! IA (modelo, máx. tokens, presupuesto) y devuelve **la página de
//! configuración** con el bloque `.env` listo para aplicar, dentro de un
//! `<details>` pre-formateado (`<pre><code>`) con botón "Copiar .env" (JS
//! inline: `navigator.clipboard.writeText` con respaldo `execCommand`).
//! **No persiste nada**: el entorno se lee sólo al arrancar y
//! `AppState.config` es un `Config` plano detrás de `Arc` (sin `Mutex`,
//! `RwLock` ni `ArcSwap`), así que no se puede mutar en runtime. Por eso el
//! botón del formulario se llama **"Generar .env"** (no "Guardar"): lo único
//! que hace es *generar el bloque `.env`* con los valores introducidos. El
//! tooltip y la nota visible lo dicen sin rodeos: la app lee `.env` al
//! arrancar, así que hay que copiar el bloque, aplicarlo y reiniciar.
//!
//! La clave de OpenRouter **nunca** se imprime: el bloque `.env` sólo
//! contiene las variables no secretas que gobiernan la IA
//! (`OPENROUTER_MODEL`, `OPENROUTER_MAX_TOKENS`, `OPENROUTER_MAX_COST_USD`).
//! El formulario tampoco tiene ningún campo para la clave.
//!
//! El estado 501 se mantiene en ambos casos (htmx o no): lo que no se puede
//! aplicar en caliente sigue siendo 501. Sólo cambia el cuerpo — con
//! `HX-Request` es el fragmento del aviso; sin él, la página completa de
//! configuración con el aviso arriba. Nunca JSON crudo.
//!
//! TODO(config mutable): con `ArcSwap<Config>`/`RwLock<Config>` en
//! `AppState` (cambio fuera de este archivo) `save_ai` puede escribir el
//! valor nuevo y dejar de responder 501. En ese mismo cambio el botón pasa
//! de "Generar .env" a "Guardar" y se retira la nota de reinicio.
//!
//! Reutiliza el shell Stitch v2 (`stitch_v2::stitch_v2_page`) y la barra de
//! estado superior real (`stitch_panel::{fetch_shell_data, fill_shell}`),
//! por lo que el top-bar refleja el nombre del estudiante, estado MCP y
//! porcentaje de vault — idéntico al resto de pantallas.
//!
//! **Sondeo del MCP (corto y en caché)**: el estado de Mikareno NO se
//! comprueba en cada carga con un handshake bloqueante (que tardaba 10-25 s).
//! Aquí vive el sondeo compartido de la app (`mcp_probe`): un `initialize`
//! por HTTP con un presupuesto **duro de 800 ms**, cacheado **30 s** en un
//! `static Mutex<Option<(Instant, McpProbe)>>`. Si el MCP no contesta dentro
//! del presupuesto, la página dice "Mikareno: sin respuesta" (estado honesto,
//! distinto de "sin conexión") y se pinta igual. `/handoff` y `/admin` leen
//! la misma caché: el MCP recibe un sondeo cada 30 s, no uno por página.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{Form, Request, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;

use crate::academic::templates;
use crate::middleware::auth::extract_session_cookie;
use crate::AppState;

// ─── Sondeo del MCP de Mikareno: corto, en caché, nunca bloquea ───────────

/// Presupuesto del sondeo MCP, en milisegundos.
///
/// Es el **techo del camino crítico**: si el MCP no contesta en este plazo
/// se declara "sin respuesta" y la página se pinta igual. Antes el sondeo
/// bloqueaba la respuesta entre 10 y 25 s (el handshake del cliente MCP
/// tiene su propio deadline de 20 s con reintentos por debajo).
pub const MCP_PROBE_BUDGET_MS: u64 = 800;

/// TTL de la caché del sondeo MCP, en segundos.
///
/// Con este TTL, abrir `/settings`, `/handoff` o `/admin` **no** sondea el
/// MCP en cada carga: como mucho un sondeo cada 30 s.
pub const MCP_PROBE_TTL_SECS: u64 = 30;

/// Cliente HTTP del sondeo: tiempo total y de conexión = presupuesto.
///
/// El `connect_timeout` importa tanto como el total: un host inalcanzable se
/// queda esperando el SYN mucho más allá del presupuesto.
static PROBE_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(MCP_PROBE_BUDGET_MS))
        .connect_timeout(Duration::from_millis(MCP_PROBE_BUDGET_MS))
        .build()
        .expect("reqwest client para el sondeo MCP")
});

/// Estado del MCP de Mikareno tras el sondeo.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum McpProbe {
    /// Contestó `initialize` dentro del presupuesto.
    Connected { latency_ms: u128 },
    /// No hubo respuesta dentro del presupuesto (800 ms).
    NoResponse,
    /// No se pudo conectar (error de red) o la respuesta no fue 2xx.
    Unreachable,
    /// No hay URL de MCP configurada: no se sondea.
    NotConfigured,
}

impl McpProbe {
    /// ¿El MCP respondió al sondeo?
    pub fn connected(&self) -> bool {
        matches!(self, &McpProbe::Connected { .. })
    }
}

/// Caché del sondeo: `(momento del sondeo, resultado)`.
///
/// Es compartida por toda la app: `/settings`, `/handoff` y `/admin` leen y
/// escriben la misma entrada, así que el MCP recibe **un** sondeo cada 30 s
/// como máximo, no uno por carga de página.
static MCP_PROBE_CACHE: Mutex<Option<(Instant, McpProbe)>> = Mutex::new(None);

/// Resultado cacheado si sigue fresco (menos de 30 s).
fn cached_probe() -> Option<McpProbe> {
    let guard = match MCP_PROBE_CACHE.lock() {
        Ok(guard) => guard,
        // Caché envenenado: se sondea de nuevo (no se bloquea la página).
        Err(_) => return None,
    };
    let (at, probe) = (*guard)?;
    (at.elapsed() < Duration::from_secs(MCP_PROBE_TTL_SECS)).then_some(probe)
}

/// Guarda el resultado del sondeo con la hora actual.
fn store_probe(probe: McpProbe) {
    if let Ok(mut guard) = MCP_PROBE_CACHE.lock() {
        *guard = Some((Instant::now(), probe));
    }
}

/// Sondeo en segundo plano con tope de espera.
///
/// Devuelve `Some(resultado)` si el sondeo termina dentro del presupuesto.
/// Si se agota, devuelve `None` **y el task sigue vivo**: al terminar deja el
/// resultado real en la caché, así la siguiente carga ya no espera (ni
/// hereda un "sin respuesta" cacheado durante 30 s).
pub(crate) async fn probe_with_budget(url: String) -> Option<McpProbe> {
    let probe_task = async move {
        let probe = probe_mcp(&url).await;
        store_probe(probe);
        probe
    };

    // Sin runtime de Tokio no hay dónde lanzar el task: sondeo en línea.
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return Some(probe_task.await);
    };

    let task = handle.spawn(probe_task);
    match tokio::time::timeout(Duration::from_millis(MCP_PROBE_BUDGET_MS), task).await {
        Ok(Ok(probe)) => Some(probe),
        // Presupuesto agotado o task caído: "sin respuesta". El task en
        // segundo plano sigue y rellena la caché con el resultado real.
        Ok(Err(_)) | Err(_) => None,
    }
}

/// Sondea el MCP de Mikareno con `initialize` (mismo criterio que `/health`).
///
/// URL vacía → [`McpProbe::NotConfigured`]; 2xx → [`McpProbe::Connected`] con
/// la latencia; error de red o respuesta no 2xx → [`McpProbe::Unreachable`];
/// sin respuesta dentro del presupuesto → [`McpProbe::NoResponse`].
///
/// El presupuesto es un tope **duro**: el `tokio::time::timeout` de fuera
/// cancela la petición aunque el cliente HTTP no llegue a cortarla (p. ej.
/// durante la resolución DNS).
pub async fn probe_mcp(url: &str) -> McpProbe {
    let url = url.trim();
    if url.is_empty() {
        return McpProbe::NotConfigured;
    }

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {
                "name": "study-platform-probe",
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

    match tokio::time::timeout(Duration::from_millis(MCP_PROBE_BUDGET_MS), request).await {
        // Presupuesto agotado: no hay respuesta. No se distingue de un error
        // de red y no se inventa un estado más preciso.
        Err(_) => McpProbe::NoResponse,
        Ok(Err(e)) if e.is_timeout() => McpProbe::NoResponse,
        Ok(Err(_)) => McpProbe::Unreachable,
        Ok(Ok(r)) if r.status().is_success() => McpProbe::Connected {
            latency_ms: t0.elapsed().as_millis(),
        },
        Ok(Ok(_)) => McpProbe::Unreachable,
    }
}

/// Estado del MCP de Mikareno para las páginas, con caché de 30 s.
///
/// Caché hit → respuesta inmediata (0 ms). Caché miss → un sondeo con el
/// presupuesto corto (800 ms como tope de espera); si no llega a tiempo, la
/// página dice "sin respuesta" y el sondeo sigue en segundo plano.
pub async fn mcp_probe(state: &AppState) -> McpProbe {
    if let Some(probe) = cached_probe() {
        return probe;
    }

    match probe_with_budget(state.config.mikareno_mcp_url.clone()).await {
        Some(probe) => probe,
        None => McpProbe::NoResponse,
    }
}

/// Ruta del handler [`save_ai`]. Registrarla en `handlers::mod::router` como
/// `POST /api/settings/ai` dentro del grupo protegido por sesión.
pub const AI_SETTINGS_ROUTE: &str = "/api/settings/ai";

/// Tope aceptado para `OPENROUTER_MAX_TOKENS` (validación del formulario).
const MAX_TOKENS_LIMIT: u32 = 200_000;

/// Tope aceptado para `OPENROUTER_MAX_COST_USD` (USD por mes).
const MAX_COST_LIMIT_USD: f64 = 100.0;

/// Longitud máxima del identificador de modelo.
const MODEL_MAX_LEN: usize = 120;

/// Tooltip honesto del botón "Generar .env": explica por qué el formulario
/// **no guarda en caliente** (la configuración se lee del entorno al
/// arrancar) y qué hay que hacer con el bloque generado.
const AI_TOOLTIP_TEXT: &str = "No guarda en caliente: la app lee la configuración del entorno (.env) al arrancar, así que AppState.config no se puede mutar en runtime. Este botón sólo genera el bloque .env con tus valores: cópialo al .env del servidor y reinicia el servicio.";

/// Nota visible junto al botón: qué hacer con el bloque generado.
const AI_NOTE_TEXT: &str = "La app lee .env al arrancar: copia el bloque y reinicia el servicio.";

/// Id del `<pre>` con el bloque `.env` del estado actual (tarjeta "IA / OpenRouter").
const ENV_CURRENT_PRE_ID: &str = "env-block-current";

/// Id del `<pre>` con el bloque `.env` de los cambios pendientes (aviso 501).
const ENV_PENDING_PRE_ID: &str = "env-block-pending";

/// JS inline del botón "Copiar .env".
///
/// Delegación de eventos sobre `document`: sirve a todos los bloques `.env`
/// de la página (el del estado actual en la tarjeta "IA / OpenRouter" y el
/// de los cambios pendientes en la respuesta 501) sin duplicar listeners.
/// Usa `navigator.clipboard.writeText` cuando existe (contexto seguro) y
/// `document.execCommand('copy')` como respaldo para HTTP sin TLS. El texto
/// copiado se lee del `<pre>` (`innerText`), así que sale exactamente lo que
/// se ve; la clave de API no está en el DOM, luego no se puede copiar.
const COPY_ENV_SCRIPT: &str = r####"<script>
(function () {
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
  document.addEventListener('click', function (ev) {
    var btn = (ev.target && ev.target.closest) ? ev.target.closest('[data-copy-target]') : null;
    if (!btn) { return; }
    ev.preventDefault();
    var src = document.getElementById(btn.getAttribute('data-copy-target'));
    if (!src) { return; }
    var text = (src.innerText || src.textContent || '').trim();
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(
        function () { setLabel(btn, '¡Copiado!'); },
        function () { setLabel(btn, legacyCopy(text) ? '¡Copiado!' : 'No se pudo copiar'); }
      );
    } else {
      setLabel(btn, legacyCopy(text) ? '¡Copiado!' : 'No se pudo copiar');
    }
  });
})();
</script>"####;

const CONTENT_SETTINGS: &str = include_str!(
    "../../static/stitch-v2/_content/clean/settings.html"
);

/// Bloque antiguo de la sección IA en el template estático (`include_str!` es
/// de sólo lectura): se reemplaza en runtime por la tarjeta "IA / OpenRouter".
const AI_SECTION_OLD: &str = r####"  <!-- Grupo: IA -->
  <section class="bg-surface-container-low rounded-xl p-space-md space-y-space-sm">
    <div class="flex items-center gap-space-xs">
      <!--SLOT:AI_DOT-->
      <h2 class="font-label-md text-label-md text-on-surface">Asistente de IA</h2>
    </div>
    <div class="grid grid-cols-[140px_1fr] gap-space-sm text-[13px]">
      <span class="font-label-sm text-label-sm text-on-surface-variant">Modelo</span>
      <span class="font-code-inline text-on-surface"><!--SLOT:AI_MODEL--></span>

      <span class="font-label-sm text-label-sm text-on-surface-variant">Clave OpenRouter</span>
      <span><!--SLOT:AI_KEY_STATUS--></span>

      <span class="font-label-sm text-label-sm text-on-surface-variant">Presupuesto mensual</span>
      <span class="font-code-inline text-on-surface"><!--SLOT:AI_BUDGET--></span>
    </div>
  </section>"####;

/// Tarjeta "IA / OpenRouter": banner de estado (estética `mcp_status_banner`)
/// + modelo, presupuesto y límite de tokens reales, y debajo el formulario
/// de edición (campos editables + botón "Generar .env"). La clave nunca
/// se muestra.
const AI_SECTION_NEW: &str = r####"  <!-- Grupo: IA / OpenRouter (estado real; nunca muestra la clave) -->
  <section class="bg-surface-container-low rounded-xl p-space-md space-y-space-sm">
    <div class="flex items-center gap-space-xs">
      <!--SLOT:AI_DOT-->
      <h2 class="font-label-md text-label-md text-on-surface">IA / OpenRouter</h2>
    </div>
    <!--SLOT:AI_STATUS-->
    <div class="grid grid-cols-[140px_1fr] gap-space-sm text-[13px]">
      <span class="font-label-sm text-label-sm text-on-surface-variant">Modelo</span>
      <span class="font-code-inline text-on-surface"><!--SLOT:AI_MODEL--></span>

      <span class="font-label-sm text-label-sm text-on-surface-variant">Presupuesto mensual</span>
      <span class="font-code-inline text-on-surface"><!--SLOT:AI_BUDGET--></span>

      <span class="font-label-sm text-label-sm text-on-surface-variant">Máx tokens / respuesta</span>
      <span class="font-code-inline text-on-surface"><!--SLOT:AI_MAX_TOKENS--></span>
    </div>
    <!--SLOT:AI_FORM-->
    <!--SLOT:AI_ENV-->
    <p class="text-[10px] text-outline">La clave de API no se muestra en ninguna página (sólo su estado).</p>
  </section>"####;

/// Datos reales para la página de configuración.
pub struct SettingsData {
    /// Estado del MCP de Mikareno tras el sondeo corto (con caché de 30 s).
    pub mcp_probe: McpProbe,
    /// URL configurada del MCP de Mikareno.
    pub mcp_url: String,
    /// True si OPENROUTER_API_KEY está presente y no vacía.
    pub api_configured: bool,
    pub model: String,
    pub budget_usd: f64,
    /// Máximo de tokens por respuesta (`OPENROUTER_MAX_TOKENS`).
    pub max_tokens: u32,
    pub auth_disabled: bool,
}

/// Recolecta los datos reales de configuración.
///
/// Nunca falla y nunca se queda esperando: el estado del MCP sale del
/// sondeo con caché (`mcp_probe`), cuyo presupuesto es de 800 ms. Si el MCP
/// no contesta, se muestra "sin respuesta" (estado honesto) y la página se
/// pinta igual.
pub async fn fetch_settings_data(state: &AppState) -> SettingsData {
    SettingsData {
        mcp_probe: mcp_probe(state).await,
        mcp_url: state.config.mikareno_mcp_url.clone(),
        api_configured: state.config.openrouter_api_key.is_some(),
        model: state.config.openrouter_model.clone(),
        budget_usd: state.config.openrouter_max_cost_usd,
        max_tokens: state.config.openrouter_max_tokens,
        auth_disabled: state.config.auth_disabled,
    }
}

/// Badge visual del estado del MCP.
fn render_mcp_dot(d: &SettingsData) -> String {
    match d.mcp_probe {
        McpProbe::Connected { .. } => {
            r#"<span class="w-1.5 h-1.5 rounded-full bg-secondary animate-pulse"></span>"#.to_string()
        }
        // "Sin respuesta" no es "sin conexión": el MCP puede estar vivo y
        // sólo ir lento. Se distingue con un punto ámbar.
        McpProbe::NoResponse => {
            r#"<span class="w-1.5 h-1.5 rounded-full bg-yellow-400"></span>"#.to_string()
        }
        _ => r#"<span class="w-1.5 h-1.5 rounded-full bg-outline"></span>"#.to_string(),
    }
}

fn render_mcp_status(d: &SettingsData) -> String {
    match d.mcp_probe {
        McpProbe::Connected { .. } => "Mikareno: conectado".to_string(),
        McpProbe::NoResponse => "Mikareno: sin respuesta".to_string(),
        McpProbe::Unreachable => "Mikareno: sin conexión".to_string(),
        McpProbe::NotConfigured => "Mikareno: no configurado".to_string(),
    }
}

fn render_ai_dot(d: &SettingsData, _alt: bool) -> String {
    if d.api_configured {
        r#"<span class="w-1.5 h-1.5 rounded-full bg-secondary animate-pulse"></span>"#.to_string()
    } else {
        r#"<span class="w-1.5 h-1.5 rounded-full bg-outline"></span>"#.to_string()
    }
}

fn render_ai_model(d: &SettingsData) -> String {
    if d.model.is_empty() {
        "Sin modelo".to_string()
    } else {
        templates::html_escape(&d.model)
    }
}

/// Estado de OpenRouter con la estética de `templates::mcp_status_banner`:
/// punto de color + icono + texto. Es el banner de `api_configured` (bool);
/// nunca imprime la clave, sólo su presencia.
fn render_ai_status(d: &SettingsData) -> String {
    if d.api_configured {
        r#"<div class="bg-surface-container border border-outline rounded-xl p-3 flex items-center gap-2">
            <span class="w-2 h-2 rounded-full bg-secondary animate-pulse"></span>
            <span class="material-symbols-outlined text-[16px] text-secondary">key</span>
            <span class="font-body-sm text-body-sm text-on-surface">● Configurado — clave OpenRouter presente</span>
        </div>"#
            .to_string()
    } else {
        r#"<div class="bg-surface-container border border-outline rounded-xl p-3 flex items-center gap-2">
            <span class="w-2 h-2 rounded-full bg-yellow-400"></span>
            <span class="material-symbols-outlined text-[16px] text-outline">key_off</span>
            <span class="font-body-sm text-body-sm text-on-surface-variant">● Key no definida — modo demo, sin IA. Configura <code class="font-code-inline text-[10px] bg-surface-container/50 px-1 py-0.5 rounded">OPENROUTER_API_KEY</code></span>
        </div>"#
            .to_string()
    }
}

fn render_ai_max_tokens(d: &SettingsData) -> String {
    d.max_tokens.to_string()
}

fn render_ai_budget(d: &SettingsData) -> String {
    format!("${:.2} USD / mes", d.budget_usd)
}

fn render_auth_mode(d: &SettingsData) -> String {
    if d.auth_disabled {
        "Deshabilitada (single-user local)".to_string()
    } else {
        "Requiere sesión (login/password)".to_string()
    }
}

fn render_ai_badge(d: &SettingsData) -> String {
    if d.api_configured {
        "<span class=\"text-secondary font-label-sm\">Activo</span>".to_string()
    } else {
        "<span class=\"text-yellow-400 font-label-sm\">Demo</span>".to_string()
    }
}

/// Bloque `.env` con las variables **no secretas** que gobiernan la IA.
///
/// No incluye `OPENROUTER_API_KEY`: la clave no se imprime nunca (ni en la
/// página ni en la respuesta 501); sólo se muestra su presencia. Los saltos
/// de línea del modelo se aplanan para que el bloque siga siendo un `.env`
/// válido (una línea por variable).
fn env_block_text(model: &str, max_tokens: u32, max_cost_usd: f64) -> String {
    let model = model.replace('\n', " ").replace('\r', " ").trim().to_string();
    format!(
        "OPENROUTER_MODEL={model}\nOPENROUTER_MAX_TOKENS={max_tokens}\nOPENROUTER_MAX_COST_USD={max_cost_usd:.2}"
    )
}

/// `<details>` con el bloque `.env` pre-formateado y su botón "Copiar .env".
///
/// El `<pre>` lleva el `id` al que apunta el `data-copy-target` del botón y
/// el texto se copia desde el DOM (`innerText`): lo que se copia es
/// exactamente lo que se ve. `block` se escapa porque el modelo puede venir
/// del formulario.
fn render_env_details(pre_id: &str, summary: &str, block: &str) -> String {
    format!(
        r####"<details class="rounded-xl border border-outline bg-surface-container/40 p-space-sm space-y-space-sm">
      <summary class="cursor-pointer flex items-center gap-1.5 font-label-sm text-label-sm text-on-surface">
        <span class="material-symbols-outlined text-[16px] text-outline">code</span>
        {summary}
      </summary>
      <div class="space-y-space-sm">
        <pre id="{pre_id}" class="bg-surface-container border border-outline-variant/50 rounded-lg p-3 overflow-x-auto"><code class="font-code-inline text-[12px] text-on-surface">{block}</code></pre>
        <div class="flex flex-wrap items-center gap-space-sm">
          <button type="button" data-copy-target="{pre_id}" class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-primary-container text-on-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors">
            <span class="material-symbols-outlined text-[16px]">content_copy</span>
            <span class="js-copy-label">Copiar .env</span>
          </button>
          <span class="text-[10px] text-outline">Sólo variables no secretas: la clave de OpenRouter no se incluye.</span>
        </div>
      </div>
    </details>"####,
        pre_id = pre_id,
        summary = summary,
        block = templates::html_escape(block),
    )
}

/// Bloque `.env` del estado actual, para la tarjeta "IA / OpenRouter".
///
/// Es la misma ranura que usa la respuesta 501: aquí muestra la configuración
/// vigente en el proceso (no secretas), allá los cambios pendientes.
fn render_ai_env_details(d: &SettingsData) -> String {
    let block = env_block_text(&d.model, d.max_tokens, d.budget_usd);
    render_env_details(
        ENV_CURRENT_PRE_ID,
        "Bloque .env actual (sólo variables no secretas)",
        &block,
    )
}

/// Reemplaza los marcadores `<!--SLOT:...-->` con datos reales.
fn fill_settings(content: &str, d: &SettingsData) -> String {
    content
        .replace(AI_SECTION_OLD, AI_SECTION_NEW)
        .replace("<!--SLOT:MCP_DOT-->", &render_mcp_dot(d))
        .replace("<!--SLOT:MCP_STATUS-->", &render_mcp_status(d))
        .replace("<!--SLOT:MCP_URL-->", &templates::html_escape(&d.mcp_url))
        .replace("<!--SLOT:AI_DOT-->", &render_ai_dot(d, false))
        .replace("<!--SLOT:AI_STATUS-->", &render_ai_status(d))
        .replace("<!--SLOT:AI_MODEL-->", &render_ai_model(d))
        .replace("<!--SLOT:AI_BUDGET-->", &render_ai_budget(d))
        .replace("<!--SLOT:AI_MAX_TOKENS-->", &render_ai_max_tokens(d))
        .replace("<!--SLOT:AI_FORM-->", &render_ai_form(d))
        .replace("<!--SLOT:AI_ENV-->", &render_ai_env_details(d))
        .replace("<!--SLOT:AUTH_MODE-->", &render_auth_mode(d))
        .replace("<!--SLOT:AI_DOT2-->", &render_ai_dot(d, true))
        .replace("<!--SLOT:AI_BADGE-->", &render_ai_badge(d))
        .replace("<!--SLOT:VERSION-->", env!("CARGO_PKG_VERSION"))
}

/// Contenido de la página de configuración: slots rellenados + el JS del
/// botón "Copiar .env" (hace falta en cualquier página con un bloque `.env`).
fn settings_content(d: &SettingsData) -> String {
    format!("{}{}", fill_settings(CONTENT_SETTINGS, d), COPY_ENV_SCRIPT)
}

/// Formulario de los parámetros de IA. **Sin secretos**: no existe ningún
/// campo para la clave de OpenRouter.
///
/// Los campos numéricos llegan como `String` a propósito: un `u32`/`f64`
/// directo haría que `serde` respondiese con un 422 genérico, y aquí se
/// quiere un mensaje de validación en español.
#[derive(Debug, Deserialize)]
pub struct AiSettingsForm {
    /// Identificador del modelo en OpenRouter (p. ej. `poolside/laguna-s-2.1:free`).
    pub model: String,
    /// Máximo de tokens por respuesta (`OPENROUTER_MAX_TOKENS`).
    pub max_tokens: String,
    /// Presupuesto mensual en USD (`OPENROUTER_MAX_COST_USD`).
    pub max_cost: String,
}

/// Parámetros de IA ya validados y normalizados.
#[derive(Debug, Clone)]
pub struct AiSettings {
    pub model: String,
    pub max_tokens: u32,
    pub max_cost_usd: f64,
}

/// Valida el formulario de IA.
///
/// Devuelve `Err` con la lista de errores en español (vacía = todo correcto).
/// El modelo se limita a un juego de caracteres conservador para que nunca
/// pueda romper una línea de `.env` (ni inyectar saltos de línea).
pub fn validate_ai_settings(form: &AiSettingsForm) -> Result<AiSettings, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();

    let model = form.model.trim().to_string();
    if model.is_empty() {
        errors.push("El modelo no puede estar vacío.".to_string());
    } else if model.chars().count() > MODEL_MAX_LEN {
        errors.push(format!(
            "El modelo es demasiado largo (máximo {MODEL_MAX_LEN} caracteres)."
        ));
    } else if !model
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '#' | '@'))
    {
        errors.push(
            "El modelo sólo admite letras, dígitos y los caracteres - _ . / : # @.".to_string(),
        );
    }

    let max_tokens: Option<u32> = match form.max_tokens.trim().parse::<u32>() {
        Ok(v) if (1..=MAX_TOKENS_LIMIT).contains(&v) => Some(v),
        Ok(v) => {
            errors.push(format!(
                "El máximo de tokens debe estar entre 1 y {MAX_TOKENS_LIMIT} (valor recibido: {v})."
            ));
            None
        }
        Err(_) => {
            errors.push("El máximo de tokens debe ser un número entero.".to_string());
            None
        }
    };

    // Se acepta coma decimal (escritura española); "2,50" → 2.5.
    let max_cost_usd: Option<f64> = match form.max_cost.trim().replace(',', ".").parse::<f64>() {
        Ok(v) if v.is_finite() && (0.0..=MAX_COST_LIMIT_USD).contains(&v) => Some(v),
        Ok(v) => {
            errors.push(format!(
                "El presupuesto mensual debe ser un número entre 0 y {MAX_COST_LIMIT_USD} USD (valor recibido: {v})."
            ));
            None
        }
        Err(_) => {
            errors.push("El presupuesto mensual debe ser un número (ej. 2.0 USD).".to_string());
            None
        }
    };

    match (max_tokens, max_cost_usd) {
        (Some(max_tokens), Some(max_cost_usd)) if errors.is_empty() => Ok(AiSettings {
            model,
            max_tokens,
            max_cost_usd,
        }),
        _ => Err(errors),
    }
}

/// Formulario de los parámetros de IA dentro de la tarjeta "IA / OpenRouter".
///
/// **Sin secretos**: no existe ningún campo para la clave de OpenRouter.
///
/// Los campos son **editables** (con `required`): el formulario sirve para
/// componer un bloque `.env` nuevo. El botón se llama "Generar .env" y no
/// "Guardar" porque **no guarda nada**: el `action` apunta al handler
/// [`save_ai`], que responde 501 con el bloque `.env` a aplicar porque
/// `AppState.config` es inmutable en runtime (el entorno se lee al arrancar).
/// El tooltip y la nota visible lo explican sin ambigüedad.
fn render_ai_form(d: &SettingsData) -> String {
    format!(
        r####"    <form method="post" action="{action}" class="space-y-space-sm border-t border-outline-variant/30 pt-space-sm">
      <h3 class="font-label-sm text-label-sm text-on-surface-variant">Editar parámetros de IA</h3>
      <div class="grid grid-cols-[140px_1fr] gap-space-sm items-center text-[13px]">
        <label class="font-label-sm text-label-sm text-on-surface-variant" for="ai-model">Modelo</label>
        <input id="ai-model" name="model" type="text" value="{model}" required
          class="w-full bg-surface-container border border-outline-variant/60 rounded-lg px-2 py-1 font-code-inline text-[12px] text-on-surface" />

        <label class="font-label-sm text-label-sm text-on-surface-variant" for="ai-max-tokens">Máx tokens / respuesta</label>
        <input id="ai-max-tokens" name="max_tokens" type="number" value="{max_tokens}" min="1" max="{max_tokens_limit}" required
          class="w-full bg-surface-container border border-outline-variant/60 rounded-lg px-2 py-1 font-code-inline text-[12px] text-on-surface" />

        <label class="font-label-sm text-label-sm text-on-surface-variant" for="ai-max-cost">Presupuesto mensual (USD)</label>
        <input id="ai-max-cost" name="max_cost" type="number" value="{max_cost}" min="0" max="{max_cost_limit}" step="0.01" required
          class="w-full bg-surface-container border border-outline-variant/60 rounded-lg px-2 py-1 font-code-inline text-[12px] text-on-surface" />
      </div>
      <div class="flex flex-wrap items-center gap-space-sm">
        <button type="submit" title="{tooltip}"
          class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-primary-container text-on-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors">
          <span class="material-symbols-outlined text-[16px]">code</span> Generar .env
        </button>
        <span class="text-[10px] text-outline max-w-[420px]">{note}</span>
      </div>
    </form>"####,
        action = AI_SETTINGS_ROUTE,
        model = templates::html_escape(&d.model),
        max_tokens = d.max_tokens,
        max_cost = format!("{:.2}", d.budget_usd),
        max_tokens_limit = MAX_TOKENS_LIMIT,
        max_cost_limit = MAX_COST_LIMIT_USD,
        tooltip = templates::html_escape(AI_TOOLTIP_TEXT),
        note = templates::html_escape(AI_NOTE_TEXT),
    )
}

/// Envoltorio de contenido con el shell real (top-bar con datos + nav activo).
async fn shell_page(state: &AppState, content: &str) -> String {
    let html = crate::handlers::stitch_v2::stitch_v2_page(
        "configuracion",
        content,
        state.config.auth_disabled,
    );
    let shell = crate::handlers::stitch_panel::fetch_shell_data(state).await;
    crate::handlers::stitch_panel::fill_shell(&html, &shell)
}

/// Aviso de "cambios válidos pero no aplicables en caliente".
///
/// Es un fragmento: la respuesta 501 lo antepone a la página de configuración
/// (o lo devuelve solo si la petición venía de htmx). El bloque `.env` va
/// dentro de un `<details>` pre-formateado con su botón "Copiar .env".
fn render_ai_not_applied_notice(values: &AiSettings) -> String {
    let block = env_block_text(&values.model, values.max_tokens, values.max_cost_usd);
    format!(
        r####"<div class="max-w-3xl mx-auto px-space-md pt-space-lg">
  <section class="bg-surface-container-low rounded-xl border border-outline p-space-md space-y-space-sm">
    <div class="flex items-center gap-space-sm">
      <span class="material-symbols-outlined text-[22px] text-yellow-400">info</span>
      <h2 class="font-headline-sm text-headline-sm text-on-surface">Cambios de IA no aplicados</h2>
    </div>
    <p class="font-body-sm text-body-sm text-on-surface-variant">
      Los valores son válidos, pero no se han guardado: la configuración se lee del entorno al
      arrancar el servicio y <code class="font-code-inline text-[11px] bg-surface-container/60 px-1 py-0.5 rounded">AppState.config</code>
      es inmutable en runtime. Copia el bloque, aplícalo al <code class="font-code-inline text-[11px] bg-surface-container/60 px-1 py-0.5 rounded">.env</code>
      del servidor y reinicia el servicio:
    </p>
    {details}
    <p class="font-code-inline text-[11px] text-outline">sudo systemctl restart study-platform</p>
  </section>
</div>"####,
        details = render_env_details(
            ENV_PENDING_PRE_ID,
            "Bloque .env con los cambios (aún no aplicados)",
            &block,
        ),
    )
}

/// Contenido de la respuesta de error de validación.
fn render_ai_errors_content(errors: &[String]) -> String {
    let items: String = errors
        .iter()
        .map(|e| {
            format!(
                "<li class=\"font-body-sm text-body-sm text-on-surface-variant\">{}</li>",
                templates::html_escape(e)
            )
        })
        .collect::<Vec<_>>()
        .join("\n        ");

    format!(
        r####"<div class="max-w-3xl mx-auto px-space-md py-space-lg space-y-space-md">
  <header class="flex items-center gap-space-sm">
    <span class="material-symbols-outlined text-[22px] text-red-400">error</span>
    <h1 class="font-headline-sm text-headline-sm text-on-surface">Configuración de IA no válida</h1>
  </header>
  <section class="bg-surface-container-low rounded-xl p-space-md space-y-space-sm">
    <ul class="list-disc pl-5 space-y-1">
        {items}
    </ul>
    <a href="/settings" class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-primary-container text-on-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors">
      <span class="material-symbols-outlined text-[16px]">arrow_back</span> Volver a Configuración
    </a>
  </section>
</div>"####,
        items = items,
    )
}

/// POST /api/settings/ai — valida los parámetros de IA enviados por el
/// formulario de la tarjeta "IA / OpenRouter".
///
/// Respuestas:
/// - 400 con la lista de errores en español si el formulario no es válido.
/// - 501 con la **página de configuración** y el bloque `.env` listo para
///   copiar (dentro de un `<details>`) si los valores son válidos pero no se
///   pueden guardar en caliente.
///
/// `headers` distingue una petición htmx de una nativa: htmx no hace swap con
/// respuestas 4xx/5xx, así que en ese caso se devuelve sólo el fragmento del
/// aviso; una petición nativa recibe la página de configuración completa con
/// el aviso arriba. El estado sigue siendo **501 en ambos casos** (los
/// cambios no se aplicaron) y el cuerpo es **siempre HTML**, nunca JSON
/// crudo.
///
/// TODO(config mutable): hoy `AppState.config` es un `Config` plano detrás
/// de `Arc` (sin `Mutex`/`RwLock`/`ArcSwap`) y el entorno sólo se lee al
/// arrancar, así que **no** se escribe nada: ni `.env` ni la base de datos.
/// En cuanto exista un store mutable (cambio fuera de este archivo) hay que
/// (a) persistir el valor nuevo, (b) devolver 200 y (c) cambiar la etiqueta
/// del botón de "Generar .env" a "Guardar" (`render_ai_form`).
///
/// No acepta ni muestra secretos: la clave de OpenRouter no forma parte del
/// formulario ni del bloque `.env` generado.
pub async fn save_ai(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<AiSettingsForm>,
) -> Response {
    match validate_ai_settings(&form) {
        Ok(values) => {
            let notice = render_ai_not_applied_notice(&values);
            let body = if headers.get("HX-Request").is_some() {
                // htmx: sólo el fragmento del aviso.
                notice
            } else {
                // Nativa: la página de configuración con el aviso arriba.
                let data = fetch_settings_data(&state).await;
                let content = settings_content(&data);
                shell_page(&state, &format!("{notice}{content}")).await
            };
            (StatusCode::NOT_IMPLEMENTED, Html(body)).into_response()
        }
        Err(errors) => {
            let content = render_ai_errors_content(&errors);
            let html = shell_page(&state, &content).await;
            (StatusCode::BAD_REQUEST, Html(html)).into_response()
        }
    }
}

/// GET /settings — página de configuración con el shell Stitch v2 real.
pub async fn page(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> (StatusCode, Html<String>) {
    let data = fetch_settings_data(&state).await;
    let content = settings_content(&data);
    let authenticated = state.config.auth_disabled || extract_session_cookie(&req).is_some();
    let html = crate::handlers::stitch_v2::stitch_v2_page(
        "configuracion",
        &content,
        authenticated,
    );
    let shell = crate::handlers::stitch_panel::fetch_shell_data(&state).await;
    let html = crate::handlers::stitch_panel::fill_shell(&html, &shell);
    (StatusCode::OK, Html(html))
}
