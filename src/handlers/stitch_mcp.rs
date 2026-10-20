//! Panel del centro MCP (Stitch v2): estado real de integraciones.
//!
//! Reemplaza los marcadores `<!--SLOT:...-->` del HTML del kit Stitch
//! (`static/stitch-v2/_content/clean/mcp.html`) por el estado real: qué MCP
//! están conectados (Mikareno, GenOffice), cuántas herramientas expone el
//! servidor Mikareno, qué modelo de IA está configurado y el gasto real del
//! mes (`usage_metrics`). Lo que no tiene fuente real se muestra como estado
//! vacío honesto, nunca con datos de ejemplo.
//!
//! ## Credenciales (reglas de este módulo)
//!
//! - La clave de OpenRouter **nunca** llega al navegador: el campo queda
//!   deshabilitado y sin `name` (no se envía), su valor real no se imprime
//!   **ni sus últimos cuatro caracteres**, y `fill_mcp` borra del HTML
//!   cualquier aparición de la clave por si el template del kit cambiara.
//! - El estado que sí se muestra sale de `config.openrouter_api_key.is_some()`:
//!   "Key configurada" / "Key no definida".
//! - La configuración se lee del entorno al arrancar (`AppState.config` es
//!   inmutable en runtime), así que la tarjeta de IA **no finge guardar**: el
//!   botón es un `submit` real contra `POST /api/settings/ai` —el mismo
//!   handler que usa la página de Configuración— que responde **501** con el
//!   bloque `.env` a aplicar, nunca un "guardado" ficticio.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use crate::academic::mikareno_client;
use crate::AppState;

const MCP_TIMEOUT_SECS: u64 = 10;
/// Timeout corto para los sondeos directos (GenOffice y URLs no locales).
const PROBE_TIMEOUT_MILLIS: u64 = 2500;
/// Ruta del handler que genera el bloque `.env` (responde 501). Es el mismo
/// que usa la página de Configuración: no hay un endpoint que guarde la
/// configuración en caliente.
const AI_SAVE_ROUTE: &str = crate::handlers::settings::AI_SETTINGS_ROUTE;

/// Cliente HTTP compartido para los sondeos (mismo patrón que `/health`).
static PROBE_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(PROBE_TIMEOUT_MILLIS))
        .build()
        .expect("reqwest client para sondeos MCP")
});

/// Estado real de las integraciones.
#[derive(Default)]
pub struct McpPanelData {
    pub mikareno_connected: bool,
    /// `Some(n)`: herramientas vía `tools/list`; `None`: sin datos.
    pub mikareno_tools: Option<usize>,
    /// Latencia (ida y vuelta, ms) del último chequeo a Mikareno.
    pub mikareno_latency_ms: Option<u128>,
    pub mikareno_url: String,
    /// Latencia del sondeo a GenOffice; `None` cuando no responde.
    pub genoffice_probe_ms: Option<u128>,
    pub genoffice_url: String,
    pub model: String,
    pub api_configured: bool,
    /// Gasto de IA del mes en curso (tabla `usage_metrics`).
    pub spend_usd: f64,
    /// Tope configurado (`OPENROUTER_MAX_COST_USD`).
    pub budget_usd: f64,
    /// Solicitudes de IA del mes en curso.
    pub requests_month: u64,
    /// Máximo de tokens por respuesta (`OPENROUTER_MAX_TOKENS`).
    pub max_tokens: u32,
    /// Herramientas del MCP nativo (`mcp_server::tools::all_tools`).
    pub native_tools: usize,
    /// Puerto HTTP de esta instancia, donde vive el MCP nativo (`/mcp`).
    pub native_port: u16,
    /// Secretos que no deben aparecer en el HTML servido. Se usan **sólo**
    /// para borrarlos: nunca se renderizan.
    secrets: Vec<Secret>,
}

/// Valor secreto que no debe aparecer en el HTML.
///
/// No implementa `Debug` a propósito: si alguien depura la estructura, la
/// clave no se imprime.
struct Secret(String);

impl Secret {
    fn new(value: &str) -> Self {
        Secret(value.to_string())
    }

    /// Marcadores que no pueden aparecer en el HTML servido: la clave
    /// completa y, para claves de OpenRouter (`sk-or-v1-…`), su sufijo de
    /// cuatro caracteres por si algún render llegara a enmascararla.
    fn markers(&self) -> Vec<String> {
        let mut out = vec![self.0.clone()];
        if self.0.starts_with("sk-or-v1-") && self.0.len() > 12 {
            let suffix = &self.0[self.0.len() - 4..];
            // Sólo el sufijo pegado a un guion (una clave enmascarada), para no
            // reescribir palabras del HTML que coincidan por casualidad.
            out.push(format!("-{suffix}"));
        }
        out
    }
}

impl Default for Secret {
    fn default() -> Self {
        Secret(String::new())
    }
}

/// Recolecta los datos del centro MCP. Nunca falla: ante un error cada campo
/// degrada a su valor vacío.
pub async fn fetch_mcp_data(state: &AppState) -> McpPanelData {
    let mut d = McpPanelData {
        mikareno_url: state.config.mikareno_mcp_url.clone(),
        genoffice_url: state.config.genoffice_mcp_url.clone(),
        model: state.config.openrouter_model.clone(),
        api_configured: state.config.openrouter_api_key.is_some(),
        budget_usd: state.config.openrouter_max_cost_usd,
        max_tokens: state.config.openrouter_max_tokens,
        native_tools: crate::mcp_server::tools::all_tools().len(),
        native_port: state.config.port,
        ..Default::default()
    };

    // La clave real no se renderiza nunca: sólo se conserva como marcador
    // para **borrarla** del HTML si el template del kit la inyectara.
    if let Some(key) = state.config.openrouter_api_key.as_deref() {
        if !key.trim().is_empty() {
            d.secrets.push(Secret::new(key.trim()));
        }
    }

    // Tres fuentes independientes, en paralelo: uso real de IA (base local),
    // estado del MCP de Mikareno y sondeo directo al MCP de GenOffice.
    let (uso, mikareno, genoffice) = tokio::join!(
        crate::ai::openrouter::persistent_summary(&state.db, &state.config),
        check_mikareno(state),
        probe_mcp(&state.config.genoffice_mcp_url),
    );
    if let Ok(s) = uso {
        d.spend_usd = s.total_cost;
        d.requests_month = s.requests_count;
    }
    let (mikareno_connected, mikareno_tools, mikareno_latency) = mikareno;
    d.mikareno_connected = mikareno_connected;
    d.mikareno_tools = mikareno_tools;
    d.mikareno_latency_ms = mikareno_latency;
    d.genoffice_probe_ms = genoffice;

    d
}

/// Estado del MCP de Mikareno: `(conectado, nº de herramientas, latencia_ms)`.
///
/// Con URL local usa el cliente completo (`initialize` + `tools/list`); con
/// URL no local el cliente está deshabilitado a propósito, así que un sondeo
/// directo (como el de `/health`) al menos reporta si el servidor está vivo.
async fn check_mikareno(state: &AppState) -> (bool, Option<usize>, Option<u128>) {
    if let Some(client) = mikareno_client(state) {
        let t0 = Instant::now();
        let init = tokio::time::timeout(
            std::time::Duration::from_secs(MCP_TIMEOUT_SECS),
            client.initialize(),
        )
        .await
        .is_ok();
        if init {
            let tools = client.list_tools().await.ok().map(|v| v.len());
            return (true, tools, Some(t0.elapsed().as_millis()));
        }
        return (false, None, None);
    }

    let url = &state.config.mikareno_mcp_url;
    if url.is_empty() {
        return (false, None, None);
    }
    match probe_mcp(url).await {
        Some(ms) => (true, None, Some(ms)),
        None => (false, None, None),
    }
}

/// POST de `initialize` MCP sobre HTTP; devuelve la latencia (ms) cuando el
/// servidor responde 2xx, `None` si la URL está vacía o no responde.
async fn probe_mcp(url: &str) -> Option<u128> {
    if url.is_empty() {
        return None;
    }
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {
                "name": "study-platform-mcp-panel",
                "version": env!("CARGO_PKG_VERSION"),
            },
        },
    });
    let t0 = Instant::now();
    let ok = PROBE_CLIENT
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    ok.then_some(t0.elapsed().as_millis())
}

/// Reemplaza los marcadores `<!--SLOT:...-->` por HTML real.
///
/// Además parchea la tarjeta de IA del template ([`patch_ai_form`]) y borra
/// cualquier resto de secreto ([`scrub_secrets`]).
pub fn fill_mcp(content: &str, d: &McpPanelData) -> String {
    let html = content
        .replace("<!--SLOT:MCP_ORCHESTRATOR-->", &render_orchestrator(d))
        .replace("<!--SLOT:MCP_LATENCY-->", &render_latency(d))
        .replace("<!--SLOT:MCP_SPEND-->", &render_spend(d))
        .replace("<!--SLOT:MCP_STATUS_AI-->", &render_ai_badge(d))
        .replace("<!--SLOT:MCP_KEY_STATE-->", &render_key_state(d))
        .replace("<!--SLOT:MCP_MODEL_OPTIONS-->", &render_model_options(d))
        .replace("<!--SLOT:MCP_MODEL-->", &render_model(d))
        .replace("<!--SLOT:MCP_BUDGET-->", &render_budget(d))
        .replace("<!--SLOT:MCP_BUDGET_INT-->", &render_budget_int(d))
        .replace("<!--SLOT:MCP_SERVERS_LINE-->", &render_servers_line(d))
        .replace("<!--SLOT:MCP_STATUS_MIKARENO-->", &render_mikareno(d))
        .replace("<!--SLOT:MCP_TOOLS-->", &render_tools(d))
        .replace(
            "<!--SLOT:MCP_URL_MIKARENO-->",
            &render_url(&d.mikareno_url, "Mikareno"),
        )
        .replace("<!--SLOT:MCP_STATUS_GENOFFICE-->", &render_genoffice(d))
        .replace(
            "<!--SLOT:MCP_URL_GENOFFICE-->",
            &render_url(&d.genoffice_url, "GenOffice"),
        )
        .replace("<!--SLOT:MCP_NATIVE_LOCATION-->", &render_native_location(d))
        .replace("<!--SLOT:MCP_NATIVE_TOOLS-->", &render_native_tools(d))
        .replace("<!--SLOT:MCP_TELEMETRY-->", &render_telemetry(d))
        .replace("<!--SLOT:MCP_TRANSCRIPT-->", &render_transcript(d));

    // La tarjeta de IA del template se parchea para que no finja guardar.
    let html = patch_ai_form(&html, d);
    // Red de seguridad: ni la clave real ni su sufijo pueden quedar en el HTML.
    scrub_secrets(&html, &d.secrets)
}

// ── renders ────────────────────────────────────────────────────────────────

/// Estado del orquestador en la cinta superior del encabezado.
fn render_orchestrator(d: &McpPanelData) -> String {
    if d.api_configured {
        r#"<span class="w-2 h-2 rounded-full bg-secondary shadow-[0_0_8px_rgba(158,215,83,0.8)]"></span>
        <span class="font-label-sm text-label-sm text-on-surface">Orquestador: Activo</span>"#.to_string()
    } else {
        r#"<span class="w-2 h-2 rounded-full bg-outline"></span>
        <span class="font-label-sm text-label-sm text-outline">Orquestador: Sin API key</span>"#.to_string()
    }
}

/// Cinta superior: latencia real del último chequeo a Mikareno.
fn render_latency(d: &McpPanelData) -> String {
    match d.mikareno_latency_ms {
        Some(ms) => format!("{ms}ms"),
        None => "—".to_string(),
    }
}

/// Cinta superior: gasto real del mes en curso ( tabla `usage_metrics`).
fn render_spend(d: &McpPanelData) -> String {
    format!("${:.2} USD", d.spend_usd)
}

/// Insignia de salud del bloque de credenciales de OpenRouter.
fn render_ai_badge(d: &McpPanelData) -> String {
    if d.api_configured {
        r#"<div class="flex items-center gap-1.5 px-space-sm py-1 rounded-full bg-secondary/10 text-secondary text-label-sm font-label-sm" id="live-connection-tag">
<span class="w-1.5 h-1.5 rounded-full bg-secondary animate-pulse"></span>
<span>API de OpenRouter activa</span>
</div>"#
            .to_string()
    } else {
        r#"<div class="flex items-center gap-1.5 px-space-sm py-1 rounded-full bg-surface-container-high text-outline text-label-sm font-label-sm" id="live-connection-tag">
<span class="w-1.5 h-1.5 rounded-full bg-outline"></span>
<span>Modo demo (sin API key)</span>
</div>"#
            .to_string()
    }
}

/// Estado real de la credencial (`config.openrouter_api_key.is_some()`).
///
/// No dice nada del valor: ni la clave ni sus últimos cuatro caracteres.
fn render_key_state(d: &McpPanelData) -> String {
    if d.api_configured {
        "Key configurada (en el servidor)".to_string()
    } else {
        "Key no definida (modo demo)".to_string()
    }
}

/// Opciones del selector de modelo: el único modelo real (el de `.env`).
fn render_model_options(d: &McpPanelData) -> String {
    if d.model.is_empty() {
        return r#"<option value="">Sin modelo configurado</option>"#.to_string();
    }
    format!(
        r#"<option value="{m}" selected>{m}</option>"#,
        m = esc(&d.model)
    )
}

/// Modelo real, para la cabecera del copiloto lateral.
fn render_model(d: &McpPanelData) -> String {
    if d.model.is_empty() {
        "Sin modelo".to_string()
    } else {
        esc(&d.model)
    }
}

/// Presupuesto real configurado (`OPENROUTER_MAX_COST_USD`).
fn render_budget(d: &McpPanelData) -> String {
    format!("${:.2} USD", d.budget_usd)
}

/// Presupuesto como número, para el valor inicial del control deslizante.
///
/// Valor exacto (no redondeado): el bloque `.env` que genera el formulario
/// tiene que reflejar el presupuesto real de la configuración.
fn render_budget_int(d: &McpPanelData) -> String {
    let s = format!("{:.2}", d.budget_usd.max(0.0));
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// Conteo de servicios MCP registrados (Mikareno + GenOffice si tienen URL,
/// más el MCP nativo, que siempre está).
fn render_servers_line(d: &McpPanelData) -> String {
    let mut n = 1;
    if !d.mikareno_url.is_empty() {
        n += 1;
    }
    if !d.genoffice_url.is_empty() {
        n += 1;
    }
    format!("{n} servicios registrados")
}

/// Chip de estado (punto de color + texto) para el pie de las tarjetas.
fn status_chip(dot: &str, color: &str, label: &str) -> String {
    format!(
        r#"<span class="flex items-center gap-1 text-[11px] {color}">
<span class="w-1.5 h-1.5 rounded-full {dot}"></span> {label}
</span>"#
    )
}

/// Estado real del MCP de Mikareno: conectado / sin conexión / no configurado.
fn render_mikareno(d: &McpPanelData) -> String {
    if d.mikareno_url.is_empty() {
        status_chip("bg-outline", "text-outline", "No configurado")
    } else if d.mikareno_connected {
        status_chip("bg-secondary animate-pulse", "text-secondary", "Activo")
    } else {
        status_chip("bg-error", "text-error", "Sin conexión")
    }
}

/// Insignia de la tarjeta Mikareno: alcance de lectura + nº de herramientas.
fn render_tools(d: &McpPanelData) -> String {
    match d.mikareno_tools {
        Some(n) => format!("Lectura · {n} herramientas"),
        None => "Lectura".to_string(),
    }
}

/// Endpoint configurado, truncado (el detalle completo va en el `title`).
fn render_url(url: &str, servicio: &str) -> String {
    if url.is_empty() {
        return r#"<span class="text-[11px] text-outline">sin configurar</span>"#.to_string();
    }
    format!(
        r#"<span class="text-[11px] text-outline truncate max-w-[150px]" title="Endpoint del MCP de {servicio}">{url}</span>"#,
        url = esc(url)
    )
}

/// Estado real del MCP de GenOffice; el sondeo no pudo confirmar => "sin
/// verificar" (degradación honesta, nunca un estado inventado).
fn render_genoffice(d: &McpPanelData) -> String {
    if d.genoffice_url.is_empty() {
        status_chip("bg-outline", "text-outline", "No configurado")
    } else if d.genoffice_probe_ms.is_some() {
        status_chip("bg-secondary animate-pulse", "text-secondary", "Activo")
    } else {
        status_chip("bg-outline", "text-outline", "Sin verificar")
    }
}

/// Ubicación real del MCP nativo: misma instancia HTTP, ruta `/mcp`.
fn render_native_location(d: &McpPanelData) -> String {
    format!(
        r#"<span class="flex items-center gap-1 text-[11px] text-primary-fixed-dim" title="MCP nativo de la plataforma (misma instancia HTTP)">
<span class="w-1.5 h-1.5 rounded-full bg-primary-container"></span> Puerto {} · /mcp
</span>"#,
        d.native_port
    )
}

/// Nº real de herramientas expuestas por el MCP nativo.
fn render_native_tools(d: &McpPanelData) -> String {
    format!("{} herramientas", d.native_tools)
}

/// Métricas reales de uso de IA. Sin registro => estado vacío honesto; con
/// registro => gasto, solicitudes y avance del presupuesto.
fn render_telemetry(d: &McpPanelData) -> String {
    if d.requests_month == 0 {
        return r#"<div class="flex flex-col items-center justify-center gap-2 rounded-lg bg-surface-container py-8 text-center">
<span class="material-symbols-outlined text-[28px] text-outline">monitoring</span>
<p class="font-body-sm text-body-sm text-outline">Sin datos de uso todavía: el asistente de IA no ha procesado solicitudes este mes.</p>
</div>"#
            .to_string();
    }
    let pct = if d.budget_usd > 0.0 {
        ((d.spend_usd / d.budget_usd) * 100.0).round().clamp(0.0, 100.0) as u32
    } else {
        0
    };
    format!(
        r#"<p class="font-body-sm text-body-sm text-outline">Uso real del mes en curso, registrado en la base local.</p>
<div class="flex flex-wrap items-center gap-space-md font-code-inline text-[11px]">
<span class="text-on-surface-variant">Solicitudes: {n}</span>
<span class="text-primary-container">Gasto: ${spend:.2} / ${budget:.2} USD</span>
</div>
<div class="w-full h-1.5 bg-surface-container rounded-full overflow-hidden">
<div class="h-full bg-gradient-to-r from-secondary to-primary-container rounded-full" style="width: {pct}%"></div>
</div>"#,
        n = d.requests_month,
        spend = d.spend_usd,
        budget = d.budget_usd,
        pct = pct
    )
}

/// Estado inicial del transcript del playground: sin conversaciones falsas.
fn render_transcript(d: &McpPanelData) -> String {
    let mikareno = if d.mikareno_url.is_empty() {
        "no configurado"
    } else if d.mikareno_connected {
        "activo"
    } else {
        "sin conexión"
    };
    let genoffice = if d.genoffice_url.is_empty() {
        "no configurado"
    } else if d.genoffice_probe_ms.is_some() {
        "activo"
    } else {
        "sin verificar"
    };
    let ia = if d.api_configured {
        format!("API activa (modelo {})", esc(&d.model))
    } else {
        "modo demo (sin API key)".to_string()
    };
    format!(
        r#"<div class="p-2 rounded-lg bg-surface-container/60 text-center">
<span class="font-code-inline text-[11px] text-outline">Canal de pruebas listo. Las respuestas de este panel son simuladas; la inferencia real está en el asistente.</span>
</div>
<div class="flex flex-col gap-1 items-start">
<div class="flex items-center gap-1.5 text-primary-container font-label-sm text-[11px]">
<span class="material-symbols-outlined text-[14px]">auto_awesome</span>
<span>Estado del sistema</span>
</div>
<div class="bg-surface-container p-3 rounded-lg text-on-surface font-body-sm text-body-sm max-w-[90%] leading-relaxed shadow-sm">
MCP Mikareno: {mikareno}. MCP GenOffice: {genoffice}. IA: {ia}.
</div>
</div>"#,
        mikareno = mikareno,
        genoffice = genoffice,
        ia = ia
    )
}

// ── parche del template: la tarjeta de IA no puede fingir que guarda ───────

/// Aplica un parche al template del kit.
///
/// Si el ancla no aparece (el kit Stitch cambió), deja el HTML como estaba y
/// avisa por log: es preferible una página con el botón viejo a una rota.
fn patch(html: &str, from: &str, to: &str) -> String {
    if html.contains(from) {
        html.replace(from, to)
    } else {
        let ancla: String = from.chars().take(60).collect();
        tracing::warn!("stitch_mcp: ancla no encontrada en el template MCP: {ancla}");
        html.to_string()
    }
}

/// Convierte la tarjeta de "API Key" del kit en algo honesto.
///
/// - Envuelve la tarjeta en un `<form>` que hace POST a [`AI_SAVE_ROUTE`]
///   (`/api/settings/ai`): ese handler valida los valores y responde **501**
///   con el bloque `.env` a aplicar, porque la configuración se lee del
///   entorno al arrancar y **no** se puede guardar en caliente.
/// - El campo de la clave queda `disabled` y **sin `name`**: no viaja al
///   servidor y su valor nunca se imprime. La página dice dónde editarla.
/// - "Guardar Parámetros" (que no guardaba nada) pasa a ser "Generar .env".
/// - Los controles que la app no puede cambiar en caliente (la temperatura,
///   que el cliente fija en el código) se muestran deshabilitados con su
///   valor real.
fn patch_ai_form(html: &str, d: &McpPanelData) -> String {
    let mut h = html.to_string();

    // 1. Bloque de la clave: nota de "cómo se aplica" (antes de quitar el
    //    botón de visibilidad, que usa el ancla con `</button>`).
    h = patch(&h, ANCHOR_KEY_NOTE, KEY_NOTE_HTML);

    // 2. Etiqueta: qué variable hay que editar de verdad.
    h = patch(&h, ANCHOR_KEY_LABEL, KEY_LABEL_HTML);

    // 3. El campo de la clave: deshabilitado y sin `name` (no se envía).
    h = patch(
        &h,
        ANCHOR_KEY_INPUT,
        r#"id="openrouter-key" disabled placeholder="sk-or-v1-"#,
    );

    // 4. El botón "Mostrar" no tiene sentido sin una clave que mostrar.
    h = patch(&h, ANCHOR_KEY_TOGGLE, KEY_TOGGLE_HTML);

    // 5. El modelo real es el único que hay: se envía al formulario.
    h = patch(
        &h,
        ANCHOR_MODEL_SELECT,
        r#"id="default-model" name="model">"#,
    );

    // 6. La temperatura está fija en el cliente de OpenRouter (0.7): no es
    //    un parámetro de la configuración, así que no se edita aquí.
    h = patch(
        &h,
        ANCHOR_TEMP_LABEL,
        r#"<span class="font-code-inline text-code-inline text-primary-container" id="temp-value">0.70 (fija en el código)</span>"#,
    );
    h = patch(
        &h,
        ANCHOR_TEMP_SLIDER,
        r#"id="temperature-slider" max="100" min="0" type="range" value="70" disabled"#,
    );

    // 7. El presupuesto real es editable (acota el bloque `.env`): el tope
    //    máximo es el que acepta el handler (100 USD).
    h = patch(
        &h,
        ANCHOR_BUDGET_SLIDER,
        r#"id="budget-slider" name="max_cost" max="100" min="0" step="0.01" type="range""#,
    );
    h = patch(&h, ANCHOR_BUDGET_MIN, "<span>$0 Límite base</span>");
    h = patch(&h, ANCHOR_BUDGET_MAX, "<span>$100 Tope máximo</span>");
    // El label del deslizador con dos decimales (el JS del kit assume enteros).
    h = patch(
        &h,
        ANCHOR_BUDGET_JS,
        "budgetValue.textContent = `$${Number(e.target.value).toFixed(2)} USD`;",
    );

    // 8. Estado real de la configuración, a la vista.
    h = patch(
        &h,
        ANCHOR_ACTION_NOTE,
        &format!(
            "<span>Real: {model} · tope {budget} · {key}. No se guarda en caliente: el bloque .env se copia al servidor y se reinicia el servicio.</span>",
            model = esc(&d.model),
            budget = format!("${:.2}", d.budget_usd),
            key = if d.api_configured {
                "key configurada"
            } else {
                "key no definida"
            },
        ),
    );

    // 9. El botón que mentía: ahora es un `submit` real al handler de la
    //    configuración (que responde 501 con el bloque `.env` a aplicar).
    h = patch(&h, ANCHOR_SAVE_BUTTON, SAVE_BUTTON_HTML);

    // 10. Y su handler en el cliente (el "guardado" simulado) desaparece:
    //     el submit es real, no hay feedback falso.
    h = patch(&h, ANCHOR_FAKE_SAVE_JS, FAKE_SAVE_JS_REPLACEMENT);

    // 11. El formulario envuelve la tarjeta (los campos van dentro).
    h = patch(
        &h,
        ANCHOR_FORM_OPEN,
        &format!(
            "<!-- Secret Key Form -->\n<form method=\"post\" action=\"{route}\" style=\"display:contents\" id=\"mcp-ai-form\">\n<input type=\"hidden\" name=\"max_tokens\" value=\"{max_tokens}\">",
            route = AI_SAVE_ROUTE,
            max_tokens = d.max_tokens,
        ),
    );
    h = patch(
        &h,
        ANCHOR_FORM_CLOSE,
        "</form>\n<!-- Linked MCP Servers Grid -->",
    );

    // 12. Red de seguridad: si el botón viejo sobrevivió al parche (el kit
    //     cambió), se neutraliza: un botón deshabilitado no finge guardar.
    if h.contains("btn-save-key") {
        tracing::warn!("stitch_mcp: el botón de guardado del template sigue presente: se neutraliza");
        h = h.replace(
            r#"id="btn-save-key" type="button""#,
            r#"id="btn-save-key" type="button" disabled"#,
        );
        h = h.replace(
            "<span>Guardar Parámetros</span>",
            "<span>Config fija en .env</span>",
        );
    }

    h
}

/// Borra del HTML cualquier secreto conocido (la clave real y, si aparece,
/// su sufijo enmascarado). Última red antes de servir la página.
fn scrub_secrets(html: &str, secrets: &[Secret]) -> String {
    let mut out = html.to_string();
    for secret in secrets {
        for marker in secret.markers() {
            if marker.is_empty() {
                continue;
            }
            out = out.replace(&marker, "[REDACTADA]");
        }
    }
    out
}

// Anclas del template del kit Stitch (`static/stitch-v2/_content/clean/mcp.html`).
const ANCHOR_KEY_LABEL: &str = r#"<label class="font-label-md text-label-md text-on-surface-variant flex items-center gap-1.5" for="openrouter-key">
<span>API Key Secreta</span>
<span class="material-symbols-outlined text-[15px] text-outline cursor-help" title="Tu clave comienza habitualmente con sk-or-v1-...">info</span>
</label>"#;
const KEY_LABEL_HTML: &str = r#"<div class="font-label-md text-label-md text-on-surface-variant flex items-center gap-1.5">
<span>Clave de OpenRouter (OPENROUTER_API_KEY)</span>
<span class="material-symbols-outlined text-[15px] text-outline cursor-help" title="La app lee OPENROUTER_API_KEY del .env del servidor al arrancar: no se puede cambiar en caliente desde aquí.">info</span>
</div>"#;

/// El campo de la clave: se deshabilita (y no lleva `name`, así que el
/// navegador no lo envía nunca).
const ANCHOR_KEY_INPUT: &str = r#"id="openrouter-key" placeholder="sk-or-v1-"#;

/// Botón "Mostrar": sin clave que mostrar, se sustituye por el aviso.
const ANCHOR_KEY_TOGGLE: &str = r#"<button class="absolute right-3 px-2 py-1 rounded text-outline hover:text-on-surface hover:bg-surface-container-highest transition-colors flex items-center gap-1 font-label-sm text-label-sm" id="toggle-key-visibility" type="button">
<span class="material-symbols-outlined text-[16px]" id="eye-icon">visibility</span>
<span id="eye-text">Mostrar</span>
</button>"#;
const KEY_TOGGLE_HTML: &str =
    r#"<span class="absolute right-3 font-code-inline text-[11px] text-outline">no se muestra aquí</span>"#;

/// Nota bajo el campo de la clave: cómo se aplica de verdad.
const ANCHOR_KEY_NOTE: &str = r#"</button>
</div>
</div>
<!-- Model Selector & Parameters Sub-Grid -->"#;
const KEY_NOTE_HTML: &str = r#"</button>
</div>
<div class="flex flex-col gap-1">
<pre class="w-full font-code-inline text-[11px] bg-surface-container/70 rounded-lg px-2 py-1.5 text-on-surface-variant overflow-x-auto">OPENROUTER_API_KEY=sk-or-v1-&lt;pega-aqui-tu-clave&gt;</pre>
<span class="font-code-inline text-[11px] text-outline">La clave no se envía al navegador ni se guarda desde aquí: edítala en el .env del servidor y reinicia el servicio.</span>
</div>
</div>
<!-- Model Selector & Parameters Sub-Grid -->"#;

/// Selector de modelo: el único modelo real es el que hay en `.env`.
const ANCHOR_MODEL_SELECT: &str = r#"id="default-model">"#;

/// Temperatura: fija en el cliente de OpenRouter, no es configurable.
const ANCHOR_TEMP_LABEL: &str = r#"<span class="font-code-inline text-code-inline text-primary-container" id="temp-value">0.30 (Analítico)</span>"#;
const ANCHOR_TEMP_SLIDER: &str =
    r#"id="temperature-slider" max="100" min="0" type="range" value="30""#;

/// Presupuesto: valor real y tope máximo accepted por el handler.
const ANCHOR_BUDGET_SLIDER: &str = r#"id="budget-slider" max="150" min="1" type="range""#;
const ANCHOR_BUDGET_MIN: &str = "<span>$1 Límite base</span>";
const ANCHOR_BUDGET_MAX: &str = "<span>$150 Carga intensiva</span>";
const ANCHOR_BUDGET_JS: &str = "budgetValue.textContent = `$${e.target.value}.00 USD`;";

/// Estado real de la configuración, junto a los botones.
const ANCHOR_ACTION_NOTE: &str =
    "<span>Gestionado por variables de entorno (.env) en el servidor</span>";

/// El botón que fingía guardar: pasa a ser un `submit` real.
const ANCHOR_SAVE_BUTTON: &str = r#"<button class="px-space-lg py-2 rounded-lg bg-secondary text-on-secondary font-label-md text-label-md font-semibold hover:bg-secondary-fixed transition-all flex items-center gap-1.5 shadow-[0_0_12px_rgba(158,215,83,0.25)]" id="btn-save-key" type="button">
<span class="material-symbols-outlined text-[18px]">check_circle</span>
<span>Guardar Parámetros</span>
</button>"#;
const SAVE_BUTTON_HTML: &str = r#"<button class="px-space-lg py-2 rounded-lg bg-secondary text-on-secondary font-label-md text-label-md font-semibold hover:bg-secondary-fixed transition-all flex items-center gap-1.5 shadow-[0_0_12px_rgba(158,215,83,0.25)]" id="btn-generate-env" type="submit" title="No guarda en caliente: la configuración se lee del .env al arrancar. Genera el bloque .env con estos valores para copiarlo y reiniciar el servicio.">
<span class="material-symbols-outlined text-[18px]">code</span>
<span>Generar .env</span>
</button>"#;

/// Handler en el cliente que simulaba el guardado: se elimina (el submit
/// es real).
const ANCHOR_FAKE_SAVE_JS: &str = r#"    // Save Feedback
    const saveBtn = document.getElementById('btn-save-key');
    if (saveBtn) {
      saveBtn.addEventListener('click', () => {
        const origText = saveBtn.innerHTML;
        saveBtn.innerHTML = `<span class="material-symbols-outlined text-[18px]">info</span><span>Config fija en .env</span>`;
        setTimeout(() => {
          saveBtn.innerHTML = origText;
        }, 2000);
      });
    }
"#;
const FAKE_SAVE_JS_REPLACEMENT: &str = r#"    // El guardado real lo hace el formulario (POST /api/settings/ai): aquí no
    // hay ningún "guardado" simulado.
"#;

/// Apertura y cierre del formulario que envuelve la tarjeta.
const ANCHOR_FORM_OPEN: &str = "<!-- Secret Key Form -->";
const ANCHOR_FORM_CLOSE: &str = "<!-- Linked MCP Servers Grid -->";

// ── helpers ────────────────────────────────────────────────────────────────

fn esc(s: &str) -> String {
    crate::academic::templates::html_escape(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Template real que sirve `/dashboard/mcp` (`include_str!` en
    /// `stitch_v2.rs`).
    const TEMPLATE: &str = include_str!("../../static/stitch-v2/_content/clean/mcp.html");

    /// Clave de ejemplo: termina en `wxyz` para que el sufijo sea
    /// irrepetible dentro del HTML.
    const KEY: &str = "sk-or-v1-redacted-example-0000wxyz";

    fn datos() -> McpPanelData {
        let mut d = McpPanelData {
            model: "cohere/north-mini-code:free".to_string(),
            api_configured: true,
            budget_usd: 2.0,
            max_tokens: 8192,
            mikareno_connected: true,
            mikareno_tools: Some(17),
            ..Default::default()
        };
        d.secrets.push(Secret::new(KEY));
        d
    }

    #[test]
    fn la_pagina_no_muestra_la_clave_ni_su_sufijo() {
        let html = fill_mcp(TEMPLATE, &datos());
        assert!(!html.contains(KEY), "la clave real no puede aparecer en el HTML");
        assert!(
            !html.contains("wxyz"),
            "ni los últimos 4 caracteres de la clave pueden aparecer"
        );
    }

    #[test]
    fn la_clave_se_borra_si_algun_render_la_inyectara() {
        // Red de seguridad: si el template trae la clave, se elimina.
        let template_con_clave = TEMPLATE.replace("<!--SLOT:MCP_KEY_STATE-->", KEY);
        let html = fill_mcp(&template_con_clave, &datos());
        assert!(!html.contains(KEY));
        assert!(html.contains("[REDACTADA]"));
    }

    #[test]
    fn el_formulario_es_honesto_y_no_finge_guardar() {
        let html = fill_mcp(TEMPLATE, &datos());
        // El submit va al handler real de la configuración (responde 501 con
        // el bloque `.env` a aplicar).
        assert!(html.contains(r#"action="/api/settings/ai""#));
        assert!(html.contains(r#"id="btn-generate-env" type="submit""#));
        // El botón que mentía ya no está.
        assert!(!html.contains("btn-save-key"));
        assert!(!html.contains("Guardar Parámetros"));
        // La clave no se envía: el campo está deshabilitado y sin `name`.
        assert!(html.contains(r#"id="openrouter-key" disabled"#));
        assert!(!html.contains(r#"name="openrouter-key""#));
        // Los parámetros que sí se pueden generar van en el formulario.
        assert!(html.contains(r#"name="model""#));
        assert!(html.contains(r#"name="max_cost""#));
        assert!(html.contains(r#"name="max_tokens" value="8192""#));
        // El formulario queda cerrado (si no, el HTML queda roto).
        assert_eq!(html.matches("<form").count(), 1);
        assert_eq!(html.matches("</form>").count(), 1);
        assert!(html.contains("</form>\n<!-- Linked MCP Servers Grid -->"));
    }

    #[test]
    fn el_estado_de_la_clave_es_real() {
        let html = fill_mcp(TEMPLATE, &datos());
        assert!(html.contains("Key configurada"));
        assert!(!html.contains("Key no definida"));

        let mut sin_key = datos();
        sin_key.api_configured = false;
        let html = fill_mcp(TEMPLATE, &sin_key);
        assert!(html.contains("Key no definida"));
        assert!(!html.contains("Key configurada"));
    }

    #[test]
    fn el_estado_real_del_mcp_se_muestra() {
        let html = fill_mcp(TEMPLATE, &datos());
        assert!(html.contains("Lectura · 17 herramientas"));
        assert!(html.contains("cohere/north-mini-code:free"));
        assert!(html.contains("$2.00 USD"));
    }

    #[test]
    fn la_red_de_seguridad_apaga_el_boton_falso() {
        let viejo = r#"<button id="btn-save-key" type="button"><span>Guardar Parámetros</span></button>"#;
        let html = patch_ai_form(viejo, &datos());
        assert!(html.contains("disabled"), "el botón viejo debe quedar inerte");
        assert!(!html.contains("Guardar Parámetros"));
    }
}

