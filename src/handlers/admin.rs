//! Panel de administración y API de administración.
//!
//! Rutas:
//!   GET /admin                    → Panel de administración (`dashboard`, shell
//!                                   `page_ext`; `index` es el render alterno
//!                                   con el shell Stitch v2, no cableado)
//!   GET /api/admin/metrics        → JSON con métricas del sistema
//!   GET /api/admin/backup         → Crear backup comprimido de SQLite
//!   GET /api/admin/backup-status  → Información del último backup
//!   POST /api/admin/backup        → Crear backup (botón «Crear backup ahora»)
//!   POST /api/admin/backup/restore    → Restaurar la base desde un backup
//!   GET /api/admin/backup/download/{name} → Descargar el fichero de un backup
//!   GET /admin/backup/list        → Listado JSON (repinta la tabla del panel)
//!
//! El panel permite accionar el backup sin recargar: «Crear backup ahora»
//! hace POST y el JS inline repinta la tabla con la respuesta; «Restaurar»
//! va con confirmación inline (`<details>`, el patrón de las notas) y, si la
//! base está bloqueada, muestra el aviso honesto de «server en uso» en vez de
//! un error genérico.
//!
//! Todas las métricas son reales — se muestran ceros o estados vacíos
//! honestos cuando la fuente no está disponible. Nada se inventa.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Json, Response},
    routing::post,
};

use crate::AppState;

/// Filas de backups que muestra la tabla del panel.
///
/// El cron de producción retiene 14 días; la tarea en proceso (`crate::backup`)
/// retiene 5. Se muestran 14 para que el panel no tape copias que el cron
/// todavía conserva.
const PANEL_BACKUP_ROWS: usize = 14;

/// Cadencia del backup diario (24 h): la base de la cuenta atrás al próximo.
const BACKUP_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;

/// Métricas reales recopiladas para el dashboard de administración.
struct AdminData {
    version: String,
    uptime_secs: u64,
    uptime_human: String,
    notes_count: i64,
    chats_count: i64,
    /// Estado del MCP de Mikareno tras el sondeo corto (caché de 30 s).
    mcp_probe: crate::handlers::settings::McpProbe,
    mcp_latency_ms: Option<u128>,
    openrouter_configured: bool,
    openrouter_model: String,
    spend_usd: f64,
    budget_usd: f64,
    requests_month: u64,
    budget_alert: bool,
    recent_backups: Vec<BackupEntry>,
    /// Número total de backups en el directorio (no sólo los de la tabla).
    backup_count: usize,
    openrouter_catalog_state: String,
}

/// Entrada de backup listada en el panel.
struct BackupEntry {
    /// Nombre del fichero (`app-YYYYmmdd-HHMMSS.db`): es lo que usan las
    /// rutas de descarga (`/api/admin/backup/download/{name}`) y de
    /// restauración (`POST /api/admin/backup/restore`).
    name: String,
    path: String,
    size_bytes: u64,
    /// Fecha real del fichero (RFC-3339, hora local): el `mtime` del backup,
    /// no la hora de render ni la del cron.
    created_at: String,
    /// La misma fecha en milisegundos (epoch), para la cuenta atrás.
    created_ms: Option<i64>,
}

// ─── Handlers públicos ───────────────────────────────────────────────────────

/// GET /admin — Dashboard Stitch v2 con métricas reales.
///
/// Render alterno del panel con el shell Stitch v2 (`stitch_v2_page`), frente
/// a `dashboard` (el handler cableado en `register_routes`, con el shell
/// `page_ext`). Se conserva sin cablear: si el orquestador deja de usar
/// `dashboard`, este sigue siendo un panel completo.
#[allow(dead_code)]
pub async fn index(State(state): State<Arc<AppState>>, req: Request<Body>) -> impl IntoResponse {
    let data = fetch_admin_data(&state).await;
    let content = render_admin_content(&data);
    let authenticated = state.config.auth_disabled
        || crate::middleware::auth::extract_session_cookie(&req).is_some();
    let html =
        crate::handlers::stitch_v2::stitch_v2_page("administracion", &content, authenticated);
    let shell = crate::handlers::stitch_panel::fetch_shell_data(&state).await;
    let html = crate::handlers::stitch_panel::fill_shell(&html, &shell);
    (StatusCode::OK, Html(html))
}

/// GET /api/admin/metrics — Métricas en JSON para monitoreo.
pub async fn metrics(State(state): State<Arc<AppState>>) -> (StatusCode, Json<serde_json::Value>) {
    let data = fetch_admin_data(&state).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "version": data.version,
            "uptime_secs": data.uptime_secs,
            "notes_count": data.notes_count,
            "chats_count": data.chats_count,
            "mcp_connected": data.mcp_probe.connected(),
            "openrouter_configured": data.openrouter_configured,
            "spend_usd": data.spend_usd,
            "budget_usd": data.budget_usd,
        })),
    )
}

/// GET /api/admin/backup — Crear backup de la base de datos local.
///
/// Ruta legacy conservada (firma y respuesta). Ya no genera un `.gz` en
/// `./backups/`: delega en el sistema unificado (`crate::backup`), que
/// escribe la copia en `data/backups/` como `app-YYYYmmdd-HHMMSS.db` con su
/// sidecar de integridad. Así el backup creado desde el panel aparece en
/// `backup-status` y en el listado de la Fase 9 (mismo directorio).
///
/// La respuesta conserva las claves legacy (`path`, `size_bytes`) que usa el
/// JS del panel y añade las del sistema unificado (`name`, `bytes`).
pub async fn backup(State(state): State<Arc<AppState>>) -> (StatusCode, Json<serde_json::Value>) {
    let dir = std::path::Path::new(crate::backup::DEFAULT_DIR);
    match crate::backup::backup_now(&state.db, dir).await {
        Ok(copia) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "success": true,
                "name": copia.name,
                "bytes": copia.bytes,
                // Claves legacy del panel admin.
                "path": copia.path,
                "size_bytes": copia.bytes,
            })),
        ),
        Err(e) => {
            tracing::error!("Backup falló: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "error": e.to_string(),
                })),
            )
        }
    }
}

/// GET /api/admin/backup-status — Estado del último backup.
///
/// Lee `data/backups/` (el directorio único del sistema unificado) y sólo los
/// ficheros `*.db` (los sidecars `.sha256` quedan fuera). Mismo shape JSON
/// que antes: `{last_backup: {path, size_bytes, created_at}, count}`.
pub async fn backup_status(_state: State<Arc<AppState>>) -> (StatusCode, Json<serde_json::Value>) {
    let dir = std::path::Path::new(crate::backup::DEFAULT_DIR);

    if !dir.exists() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "last_backup": null, "count": 0 })),
        );
    }

    // Ficheros `*.db` del directorio unificado.
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|iter| {
            iter.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("db") {
                        Some(p)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Ordenar por nombre: formato YYYYMMDD-HHMMSS → lexicográfico = cronológico.
    files.sort_by(|a, b| {
        let fa = a.file_name().and_then(|f| f.to_str()).unwrap_or("");
        let fb = b.file_name().and_then(|f| f.to_str()).unwrap_or("");
        fb.cmp(fa)
    });

    let count = files.len();

    let latest = files.first().and_then(|path| {
        std::fs::metadata(path).ok().and_then(|meta| {
            let created = meta.modified().ok()?;
            let dt: chrono::DateTime<chrono::Utc> = chrono::DateTime::from(created);
            Some(BackupEntry {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                path: path.to_string_lossy().to_string(),
                size_bytes: meta.len(),
                created_at: dt.to_rfc3339(),
                created_ms: Some(dt.timestamp_millis()),
            })
        })
    });

    let response = match latest {
        Some(entry) => serde_json::json!({
            "last_backup": {
                "name": entry.name,
                "path": entry.path,
                "size_bytes": entry.size_bytes,
                "created_at": entry.created_at,
            },
            "count": count,
        }),
        None => serde_json::json!({ "last_backup": null, "count": 0 }),
    };

    (StatusCode::OK, Json(response))
}

// ─── Recolección de datos ────────────────────────────────────────────────────

/// Recolecta métricas reales. Nunca falla: cada campo degrada a su valor
/// seguro (0, false, vacío) cuando la fuente no está disponible.
async fn fetch_admin_data(state: &Arc<AppState>) -> AdminData {
    let version = env!("CARGO_PKG_VERSION").to_string();
    let uptime_secs = state.start_time.elapsed().as_secs();
    let uptime_human = format_uptime(uptime_secs);

    // Conteo de notas (BD local)
    let notes_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    // Conteo de conversaciones (tabla chat_history)
    let chats_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_history")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    // Configuración de OpenRouter
    let openrouter_configured = state.config.openrouter_api_key.is_some();
    let openrouter_model = state.config.openrouter_model.clone();
    let budget_usd = state.config.openrouter_max_cost_usd;

    // Gasto real de IA (tabla usage_metrics) — 0.00 si no hay datos
    let (spend_usd, requests_month, budget_alert) = {
        match crate::ai::openrouter::persistent_summary(&state.db, &state.config).await {
            Ok(summary) => (
                summary.total_cost,
                summary.requests_count,
                crate::ai::openrouter::is_alert(summary.total_cost, budget_usd),
            ),
            Err(_) => (0.0, 0, false),
        }
    };

    // Estado del MCP de Mikareno. Sondeo corto y cacheado: nunca bloquea el
    // panel más de 800 ms.
    let mcp_probe = check_mcp_connection(state).await;
    let mcp_latency_ms = match mcp_probe {
        crate::handlers::settings::McpProbe::Connected { latency_ms } => Some(latency_ms),
        _ => None,
    };

    // Backups recientes (y el total del directorio).
    let (recent_backups, backup_count) = list_backups_for_panel().await;

    // Estado del catálogo de modelos de IA
    let openrouter_catalog_state = state
        .cache_catalogo
        .get()
        .map_or_else(|| "expired".to_string(), |_| "fresh".to_string());

    AdminData {
        version,
        uptime_secs,
        uptime_human,
        notes_count,
        chats_count,
        mcp_probe,
        mcp_latency_ms,
        openrouter_configured,
        openrouter_model,
        spend_usd,
        budget_usd,
        requests_month,
        budget_alert,
        recent_backups,
        backup_count,
        openrouter_catalog_state,
    }
}

/// Verifica la conectividad del MCP de Mikareno.
///
/// Usa el **sondeo compartido** de la app (`handlers::settings::mcp_probe`):
/// un `initialize` por HTTP con un presupuesto duro de 800 ms, cacheado 30 s.
/// El handshake del cliente MCP ya **no** se ejecuta en el camino crítico:
/// tardaba hasta 10-20 s (el cliente reintenta con backoff) y bloqueaba el
/// panel entero.
///
/// Si el MCP no responde dentro del presupuesto, devuelve un estado "sin
/// respuesta" (`connected() == false`) **rápido**: el panel se pinta igual.
async fn check_mcp_connection(state: &Arc<AppState>) -> crate::handlers::settings::McpProbe {
    crate::handlers::settings::mcp_probe(state).await
}

/// Lista los backups del directorio único (`data/backups/`).
///
/// Devuelve la lista para la tabla (los [`PANEL_BACKUP_ROWS`] más recientes,
/// el mismo número de días que la retención del cron) y el total real del
/// directorio (que puede ser mayor si hay copias antiguas).
///
/// Usa el listado del sistema unificado (`crate::backup::list_backups`):
/// ficheros `*.db` del directorio único, del más reciente al más antiguo. Los
/// sidecars `.sha256` quedan fuera. La fecha es el `mtime` REAL del fichero.
async fn list_backups_for_panel() -> (Vec<BackupEntry>, usize) {
    let dir = std::path::Path::new(crate::backup::DEFAULT_DIR);
    let todos: Vec<BackupEntry> = crate::backup::list_backups(dir)
        .await
        .into_iter()
        .map(|info| {
            let created_at = created_rfc3339(&info.created);
            let created_ms = chrono::DateTime::parse_from_rfc3339(&created_at)
                .ok()
                .map(|dt| dt.timestamp_millis());
            BackupEntry {
                name: info.name,
                path: info.path,
                size_bytes: info.bytes,
                created_at,
                created_ms,
            }
        })
        .collect();
    let total = todos.len();
    (todos.into_iter().take(PANEL_BACKUP_ROWS).collect(), total)
}

/// Convierte la marca de tiempo de `BackupInfo` (hora local,
/// `%Y-%m-%d %H:%M:%S`) a RFC-3339, que es lo que espera el JS del panel.
/// Si no se puede parsear, se devuelve el texto tal cual (sin inventar fecha).
fn created_rfc3339(created: &str) -> String {
    use chrono::TimeZone;
    chrono::NaiveDateTime::parse_from_str(created, "%Y-%m-%d %H:%M:%S")
        .ok()
        .and_then(|naive| {
            chrono::Local
                .from_local_datetime(&naive)
                .single()
                .map(|dt| dt.to_rfc3339())
        })
        .unwrap_or_else(|| created.to_string())
}

// ─── Renderizado ─────────────────────────────────────────────────────────────

fn esc(s: &str) -> String {
    crate::academic::templates::html_escape(s)
}

/// Formatea segundos de uptime a cadena legible: "2d 5h 30m", "1h 15m", etc.
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
            (secs % 86400) / 3600,
            (secs % 3600) / 60
        )
    }
}

/// Genera el HTML del contenido principal del panel de administración.
/// Usa marcadores <!--SLOT:...--> que se reemplazan con datos reales.
fn render_admin_content(d: &AdminData) -> String {
    let template = r##"
<div class="flex items-center justify-between mb-space-md">
    <div>
        <h1 class="font-headline-lg text-headline-lg text-on-surface font-bold">Administración</h1>
        <p class="font-body-sm text-body-sm text-on-surface-variant mt-1">
            Monitoreo del sistema y herramientas de administración
        </p>
    </div>
    <span class="px-1.5 py-0.5 rounded text-[10px] font-mono bg-surface-container-high text-primary-fixed-dim border border-outline-variant/60">
        v<!--SLOT:ADMIN_VERSION-->
    </span>
</div>

<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-space-md mb-space-md">
    <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
        <div class="font-code-inline text-[11px] text-on-surface-variant uppercase tracking-wider">Uptime</div>
        <div class="font-headline-md text-headline-md text-primary-container font-bold mt-1"><!--SLOT:ADMIN_UPTIME--></div>
        <div class="font-body-sm text-[10px] text-on-surface-variant mt-1"><!--SLOT:ADMIN_UPTIME_SECS-->s</div>
    </div>
    <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
        <div class="font-code-inline text-[11px] text-on-surface-variant uppercase tracking-wider">Notas</div>
        <div class="font-headline-md text-headline-md text-primary-container font-bold mt-1"><!--SLOT:ADMIN_NOTES_COUNT--></div>
        <div class="font-body-sm text-[10px] text-on-surface-variant mt-1">notas locales (BD)</div>
    </div>
    <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
        <div class="font-code-inline text-[11px] text-on-surface-variant uppercase tracking-wider">Chats</div>
        <div class="font-headline-md text-headline-md text-primary-container font-bold mt-1"><!--SLOT:ADMIN_CHATS_COUNT--></div>
        <div class="font-body-sm text-[10px] text-on-surface-variant mt-1">conversaciones IA</div>
    </div>
    <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
        <div class="font-code-inline text-[11px] text-on-surface-variant uppercase tracking-wider">Gasto IA (mes)</div>
        <div class="font-headline-md text-headline-md text-primary-container font-bold mt-1"><!--SLOT:ADMIN_SPEND--> / <!--SLOT:ADMIN_BUDGET--></div>
        <div class="font-body-sm text-[10px] text-on-surface-variant mt-1"><!--SLOT:ADMIN_REQUESTS--> solicitudes · <!--SLOT:ADMIN_BUDGET_PCT-->% del presupuesto</div>
        <!--SLOT:ADMIN_BUDGET_BAR-->
    </div>
</div>

<!-- Indicadores de integración -->
<div class="grid grid-cols-1 md:grid-cols-2 gap-space-md mb-space-md">
    <div class="bg-surface-container border border-outline rounded-xl p-space-md">
        <div class="flex items-center gap-2">
            <span class="material-symbols-outlined text-[20px]"><!--SLOT:ADMIN_MCP_ICON--></span>
            <h2 class="font-headline-sm text-headline-sm text-on-surface">Mikareno MCP</h2>
        </div>
        <p class="font-body-sm text-body-sm text-on-surface-variant mt-2"><!--SLOT:ADMIN_MCP_STATUS--></p>
        <!--SLOT:ADMIN_MCP_LATENCY-->
    </div>
    <div class="bg-surface-container border border-outline rounded-xl p-space-md">
        <div class="flex items-center gap-2">
            <span class="material-symbols-outlined text-[20px]"><!--SLOT:ADMIN_OR_ICON--></span>
            <h2 class="font-headline-sm text-headline-sm text-on-surface">OpenRouter</h2>
        </div>
        <p class="font-body-sm text-body-sm text-on-surface-variant mt-2">
            Estado: <span class="<!--SLOT:ADMIN_OR_STATUS_COLOR-->"><!--SLOT:ADMIN_OR_STATUS--></span>
        </p>
        <p class="font-body-sm text-[10px] text-on-surface-variant mt-1">Modelo: <!--SLOT:ADMIN_OR_MODEL--></p>
        <p class="font-body-sm text-[10px] text-on-surface-variant mt-1">Catálogo de IA: <!--SLOT:ADMIN_OR_CATALOG_STATE--></p>
        <button id="refresh-catalog-btn" type="button" onclick="refreshCatalog()" class="flex items-center gap-1.5 px-space-sm py-1.5 rounded-lg bg-secondary text-on-secondary font-label-sm font-medium hover:bg-secondary-fixed-dim transition-colors">
            <span class="material-symbols-outlined text-[16px]">refresh</span>
            <span>Refrescar catálogo IA</span>
        </button>
        <div id="refresh-catalog-msg" class="hidden mt-1 p-space-sm rounded-lg text-[10px]"></div>
        <p class="font-body-sm text-[10px] text-outline mt-1">El gasto real del mes (y el % del presupuesto) está en la tarjeta «Gasto IA (mes)».</p>
    </div>
</div>

<!-- Sección de backup -->
<div class="bg-surface-container border border-outline rounded-xl p-space-md mb-space-md" id="admin-backup">
    <div class="flex items-start justify-between gap-2 mb-space-sm flex-wrap">
        <div>
            <h2 class="font-headline-sm text-headline-sm text-on-surface">Copia de seguridad</h2>
            <p class="font-body-sm text-[10px] text-on-surface-variant mt-0.5">Copia automática cada 24 h + copias bajo demanda de la base de datos</p>
        </div>
        <button id="backup-btn" type="button" onclick="crearBackup()" class="flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-primary-container text-on-primary font-label-md font-semibold hover:bg-primary-fixed-dim transition-all shadow-md">
            <span class="material-symbols-outlined text-[18px]">save</span>
            <span>Crear backup ahora</span>
        </button>
    </div>

    <div class="grid grid-cols-1 md:grid-cols-3 gap-space-sm mb-space-sm">
        <div class="bg-surface-container-high/40 border border-outline rounded-lg p-space-sm">
            <div class="font-code-inline text-[10px] text-on-surface-variant uppercase tracking-wider">Último backup</div>
            <div class="font-headline-sm text-headline-sm text-on-surface mt-1" id="admin-last-backup"><!--SLOT:ADMIN_LAST_BACKUP_DATE--></div>
            <div class="font-code-inline text-[10px] text-outline mt-1 truncate" id="admin-last-backup-name" title="<!--SLOT:ADMIN_LAST_BACKUP_PATH-->"><!--SLOT:ADMIN_LAST_BACKUP_NAME--></div>
        </div>
        <div class="bg-surface-container-high/40 border border-outline rounded-lg p-space-sm">
            <div class="font-code-inline text-[10px] text-on-surface-variant uppercase tracking-wider">Próximo backup</div>
            <div class="font-headline-sm text-headline-sm text-on-surface mt-1" id="admin-next-backup" data-last-backup-ms="<!--SLOT:ADMIN_LAST_BACKUP_MS-->"><!--SLOT:ADMIN_NEXT_BACKUP--></div>
            <div class="font-code-inline text-[10px] text-outline mt-1">Estimado: 24 h tras el último</div>
        </div>
        <div class="bg-surface-container-high/40 border border-outline rounded-lg p-space-sm">
            <div class="font-code-inline text-[10px] text-on-surface-variant uppercase tracking-wider">Copias en disco</div>
            <div class="font-headline-sm text-headline-sm text-on-surface mt-1" id="admin-backup-count"><!--SLOT:ADMIN_BACKUP_COUNT--></div>
            <div class="font-code-inline text-[10px] text-outline mt-1">data/backups</div>
        </div>
    </div>

    <div id="admin-backup-msg" class="hidden mb-space-sm"></div>

    <div class="overflow-x-auto">
        <table class="w-full text-left border-collapse">
            <thead>
                <tr class="border-b border-outline">
                    <th class="font-code-inline text-[10px] uppercase tracking-wider text-outline py-1.5 pr-2">Fichero</th>
                    <th class="font-code-inline text-[10px] uppercase tracking-wider text-outline py-1.5 pr-2">Fecha</th>
                    <th class="font-code-inline text-[10px] uppercase tracking-wider text-outline py-1.5 pr-2">Tamaño</th>
                    <th class="font-code-inline text-[10px] uppercase tracking-wider text-outline py-1.5 text-right">Acciones</th>
                </tr>
            </thead>
            <tbody id="admin-backups-body"><!--SLOT:ADMIN_BACKUP_ROWS--></tbody>
        </table>
    </div>
    <p class="font-body-sm text-[10px] text-outline mt-2">
        «Descargar» entrega el fichero SQLite del backup. «Restaurar» copia su contenido sobre la base en vivo: si la base está bloqueada, la app lo dice y no cambia nada.
    </p>
</div>

<script>
(function () {
  'use strict';

  var CADENCIA_MS = 24 * 60 * 60 * 1000;
  // Filas que muestra la tabla: el mismo tope que el render del servidor
  // (PANEL_BACKUP_ROWS), para que el repintado no alargue la tabla.
  var MAX_FILAS = 14;

  function esc(s) {
    if (s === null || s === undefined) { return ''; }
    return String(s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  function formatBytes(n) {
    n = Number(n) || 0;
    if (n < 1024) { return n + ' B'; }
    if (n < 1048576) { return (n / 1024).toFixed(1) + ' KB'; }
    if (n < 1073741824) { return (n / 1048576).toFixed(1) + ' MB'; }
    return (n / 1073741824).toFixed(1) + ' GB';
  }

  function dos(n) { return (n < 10 ? '0' : '') + n; }

  function fechaEs(iso) {
    if (!iso) { return '—'; }
    var d = new Date(iso);
    if (isNaN(d.getTime())) { return String(iso); }
    return dos(d.getDate()) + '/' + dos(d.getMonth() + 1) + '/' + d.getFullYear()
      + ' ' + dos(d.getHours()) + ':' + dos(d.getMinutes());
  }

  function accionesHtml(nombre) {
    var descarga = '/api/admin/backup/download/' + encodeURIComponent(nombre);
    return '<div class="flex items-center justify-end gap-1">'
      + '<a href="' + esc(descarga) + '" download class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors font-label-md"><span class="material-symbols-outlined text-[16px]">download</span>Descargar</a>'
      + '<details class="relative" data-backup-restore>'
      + '<summary class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors cursor-pointer select-none font-label-md"><span class="material-symbols-outlined text-[16px]">restore</span>Restaurar</summary>'
      + '<div class="absolute right-0 top-full mt-1 z-20 w-72 p-space-md rounded-xl border border-outline-variant bg-surface-container-high shadow-lg">'
      + '<p class="font-body-sm text-body-sm text-on-surface mb-space-sm">¿Restaurar la base desde «' + esc(nombre) + '»? Se sobrescribe el contenido actual.</p>'
      + '<p class="hidden font-body-sm text-[11px] text-error mb-space-sm" data-restore-msg></p>'
      + '<div class="flex items-center justify-end gap-2">'
      + '<button type="button" onclick="this.closest(\'details\').removeAttribute(\'open\')" class="px-space-sm py-1.5 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors font-label-md">Cancelar</button>'
      + '<button type="button" data-restore-confirm="' + esc(nombre) + '" class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg bg-primary-container text-on-primary font-label-md font-semibold hover:opacity-90 transition-opacity"><span class="material-symbols-outlined text-[16px]">restore</span>Sí, restaurar</button>'
      + '</div></div></details></div>';
  }

  function filaHtml(item) {
    var nombre = item.nombre || item.name || '';
    var bytes = item.bytes || 0;
    // `created_es` viene ya formateado por el servidor (su zona horaria):
    // así la tabla no salta de hora al repintarse. `fechaEs` es el respaldo.
    var creado = item.created_es || fechaEs(item.created || null);
    return '<tr class="border-b border-outline/20 last:border-0">'
      + '<td class="font-code-inline text-[11px] text-on-surface py-1.5 pr-2 truncate max-w-[220px]" title="' + esc(nombre) + '">' + esc(nombre) + '</td>'
      + '<td class="font-code-inline text-[11px] text-on-surface-variant py-1.5 pr-2 whitespace-nowrap">' + esc(creado) + '</td>'
      + '<td class="font-code-inline text-[11px] text-on-surface-variant py-1.5 pr-2 whitespace-nowrap">' + formatBytes(bytes) + '</td>'
      + '<td class="py-1.5">' + accionesHtml(nombre) + '</td>'
      + '</tr>';
  }

  function aviso(el, texto, tipo) {
    if (!el) { return; }
    el.className = 'flex items-start gap-2 mb-space-sm p-space-sm rounded-lg border text-on-surface '
      + (tipo === 'error' ? 'border-error/40 bg-error-container/10' : 'border-secondary/40 bg-secondary-container/10');
    el.innerHTML = '<span class="material-symbols-outlined text-[18px] '
      + (tipo === 'error' ? 'text-error' : 'text-secondary') + '">'
      + (tipo === 'error' ? 'error' : 'check_circle') + '</span>'
      + '<p class="font-body-sm text-body-sm">' + esc(texto) + '</p>';
    el.classList.remove('hidden');
  }

  function cuentaAtras() {
    var el = document.getElementById('admin-next-backup');
    if (!el) { return; }
    var last = parseInt(el.getAttribute('data-last-backup-ms') || '', 10);
    if (!last) {
      el.textContent = 'Sin backup previo';
      return;
    }
    var restante = last + CADENCIA_MS - Date.now();
    if (restante <= 0) {
      el.textContent = 'Pendiente: ya debería haberse hecho';
      return;
    }
    var horas = Math.floor(restante / 3600000);
    var mins = Math.floor((restante % 3600000) / 60000);
    el.textContent = 'en ' + (horas > 0 ? horas + ' h ' : '') + mins + ' min';
  }

  function refrescar() {
    return fetch('/admin/backup/list', { credentials: 'same-origin' })
      .then(function (res) { return res.json(); })
      .then(function (data) {
        var items = (data && data.items) ? data.items : [];
        var backups = items.filter(function (it) {
          return String(it.nombre || '').endsWith('.db');
        });
        var body = document.getElementById('admin-backups-body');
        if (body) {
          // Solo las MAX_FILAS más recientes: la tarjeta «Copias en disco»
          // sigue mostrando el total real del directorio.
          var filas = backups.slice(0, MAX_FILAS);
          body.innerHTML = filas.length
            ? filas.map(filaHtml).join('')
            : '<tr><td colspan="4" class="font-body-sm text-body-sm text-on-surface-variant py-2">Sin backups en el disco todavía.</td></tr>';
        }
        var primero = backups[0];
        if (primero) {
          var f = document.getElementById('admin-last-backup');
          if (f) { f.textContent = primero.created_es || fechaEs(primero.created); }
          var n = document.getElementById('admin-last-backup-name');
          if (n) {
            n.textContent = primero.nombre + ' · ' + formatBytes(primero.bytes || 0);
            n.setAttribute('title', primero.nombre);
          }
          var c = document.getElementById('admin-backup-count');
          if (c) { c.textContent = String(backups.length); }
          var nb = document.getElementById('admin-next-backup');
          if (nb) {
            var d = new Date(primero.created);
            if (!isNaN(d.getTime())) { nb.setAttribute('data-last-backup-ms', String(d.getTime())); }
          }
        }
        cuentaAtras();
      })
      .catch(function () { /* la tabla inicial sigue mostrando la última foto real */ });
  }

  function crearBackup() {
    var btn = document.getElementById('backup-btn');
    var msg = document.getElementById('admin-backup-msg');
    if (!btn) { return; }
    var original = btn.innerHTML;
    btn.disabled = true;
    btn.innerHTML = '<span class="material-symbols-outlined text-[18px]">hourglass_empty</span><span>Creando…</span>';
    fetch('/api/admin/backup', { method: 'POST', credentials: 'same-origin' })
      .then(function (res) {
        return res.json().catch(function () { return {}; }).then(function (data) {
          return { status: res.status, data: data || {} };
        });
      })
      .then(function (r) {
        if (r.status === 200 && r.data.success) {
          aviso(msg, 'Backup creado: ' + r.data.name + ' (' + formatBytes(r.data.bytes || 0) + ').', 'ok');
          refrescar();
        } else {
          aviso(msg, 'No se pudo crear el backup: ' + ((r.data && r.data.error) || ('HTTP ' + r.status)), 'error');
        }
      })
      .catch(function (e) {
        aviso(msg, 'Error de conexión al crear el backup: ' + e.message, 'error');
      })
      .then(function () {
        btn.disabled = false;
        btn.innerHTML = original;
      });
  }

  function restaurar(nombre, btn) {
    var box = btn.closest('details');
    var msg = box ? box.querySelector('[data-restore-msg]') : null;
    btn.disabled = true;
    if (msg) {
      msg.className = 'font-body-sm text-[11px] text-on-surface-variant mb-space-sm';
      msg.textContent = 'Restaurando…';
    }
    fetch('/api/admin/backup/restore', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ name: nombre })
    })
      .then(function (res) {
        return res.json().catch(function () { return {}; }).then(function (data) {
          return { status: res.status, data: data || {} };
        });
      })
      .then(function (r) {
        if (r.status === 200 && r.data.success) {
          if (msg) {
            msg.className = 'font-body-sm text-[11px] text-secondary mb-space-sm';
            msg.textContent = 'Base restaurada desde ' + nombre + '. Recargando…';
          }
          window.setTimeout(function () { window.location.reload(); }, 1200);
          return;
        }
        var err = (r.data && r.data.error) ? String(r.data.error) : ('HTTP ' + r.status);
        // "server en uso" NO es un error genérico: es un aviso honesto de que
        // la base está bloqueada y de que no se ha cambiado nada.
        var enUso = err.indexOf('server en uso') !== -1;
        if (msg) {
          msg.className = 'font-body-sm text-[11px] ' + (enUso ? 'text-on-surface' : 'text-error') + ' mb-space-sm';
          msg.textContent = enUso
            ? err + ' No se ha cambiado nada.'
            : 'No se pudo restaurar: ' + err;
        }
        btn.disabled = false;
      })
      .catch(function (e) {
        if (msg) {
          msg.className = 'font-body-sm text-[11px] text-error mb-space-sm';
          msg.textContent = 'Error de conexión: ' + e.message;
        }
        btn.disabled = false;
      });
  }

  document.addEventListener('click', function (ev) {
    var btn = ev.target && ev.target.closest ? ev.target.closest('[data-restore-confirm]') : null;
    if (!btn) { return; }
    ev.preventDefault();
    restaurar(btn.getAttribute('data-restore-confirm'), btn);
  });

  // La acción vive en `window` porque el botón la invoca con `onclick`.
  window.crearBackup = crearBackup;

  function refreshCatalog() {
    var btn = document.getElementById('refresh-catalog-btn');
    var msg = document.getElementById('refresh-catalog-msg');
    if (!btn) { return; }
    btn.disabled = true;
    btn.innerHTML = '<span class="material-symbols-outlined text-[16px]">hourglass_empty</span><span>Refrescando…</span>';
    msg.className = 'hidden';
    fetch('/admin/openrouter/refresh-catalog', { method: 'POST', credentials: 'same-origin' })
      .then(function (res) {
        return res.json().catch(function () { return {}; }).then(function (data) {
          return { status: res.status, data: data || {} };
        });
      })
      .then(function (r) {
        if (r.status === 200 && r.data.success) {
          msg.className = 'font-body-sm text-[10px] mb-space-sm p-space-sm rounded-lg border-secondary/40 bg-secondary-container/10';
          msg.innerHTML = '<span class="material-symbols-outlined text-[14px] text-secondary">check_circle</span> <strong>Catálogo refrescado:</strong> ' + (r.data.count || 0) + ' modelos free disponibles.';
          msg.classList.remove('hidden');
          // Recargar la página para mostrar el nuevo estado del catálogo
          window.setTimeout(function () { window.location.reload(); }, 1500);
        } else {
          msg.className = 'font-body-sm text-[10px] mb-space-sm p-space-sm rounded-lg border-error/40 bg-error-container/10';
          msg.innerHTML = '<span class="material-symbols-outlined text-[14px] text-error">error</span> No se pudo refrescar: ' + ((r.data && r.data.error) || ('HTTP ' + r.status));
          msg.classList.remove('hidden');
        }
      })
      .catch(function (e) {
        msg.className = 'font-body-sm text-[10px] mb-space-sm p-space-sm rounded-lg border-error/40 bg-error-container/10';
        msg.innerHTML = '<span class="material-symbols-outlined text-[14px] text-error">error</span> Error de conexión: ' + e.message;
        msg.classList.remove('hidden');
      })
      .then(function () {
        btn.disabled = false;
        btn.innerHTML = '<span class="material-symbols-outlined text-[16px]">refresh</span><span>Refrescar catálogo IA</span>';
      });
  }

  window.refreshCatalog = refreshCatalog;

  cuentaAtras();
  window.setInterval(cuentaAtras, 30000);
})();
</script>
"##;

    let mcp_icon = if d.mcp_probe.connected() {
        "lan_wifi"
    } else {
        "wifi_off"
    };
    let mcp_status = match d.mcp_probe {
        crate::handlers::settings::McpProbe::Connected { .. } => "Mikareno conectado".to_string(),
        // "Sin respuesta" ≠ "sin conexión": el MCP puede estar vivo y lento.
        crate::handlers::settings::McpProbe::NoResponse => {
            "Sin respuesta (no contestó en 800 ms)".to_string()
        }
        crate::handlers::settings::McpProbe::Unreachable => "Sin conexión".to_string(),
        crate::handlers::settings::McpProbe::NotConfigured => "No configurado".to_string(),
    };
    let mcp_latency = match d.mcp_latency_ms {
        Some(ms) => format!(
            r#"<div class="font-body-sm text-[10px] text-on-surface-variant mt-1 flex items-center gap-1">
            <span class="material-symbols-outlined text-[12px] text-secondary">schedule</span>
            Latencia: {}ms
        </div>"#,
            ms
        ),
        None => String::new(),
    };

    let or_icon = if d.openrouter_configured {
        "auto_awesome"
    } else {
        "warning"
    };
    let or_status = if d.openrouter_configured {
        "Activo"
    } else {
        "Modo demo (sin API key)"
    };
    let or_status_color = if d.openrouter_configured {
        "text-secondary"
    } else {
        "text-outline"
    };

    // Estado del catálogo de modelos de IA
    let or_catalog_state = if d.openrouter_catalog_state == "fresh" {
        "Actualizado"
    } else {
        "Desactualizado"
    };

    // Gasto de IA del mes: el % del presupuesto (el filtro mensual ya está
    // en `persistent_summary`) y la barra con ese mismo porcentaje.
    let budget_pct = if d.budget_usd > 0.0 {
        ((d.spend_usd / d.budget_usd) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u32
    } else {
        0
    };
    let budget_bar = if d.budget_usd > 0.0 {
        let bar_color = if d.budget_alert {
            "from-error to-error"
        } else {
            "from-secondary to-primary-container"
        };
        format!(
            r#"<div class="w-full h-1.5 bg-surface-container-highest rounded-full overflow-hidden mt-2">
        <div class="h-full bg-gradient-to-r {bar_color} rounded-full" style="width: {pct}%"></div>
    </div>"#,
            bar_color = bar_color,
            pct = budget_pct,
        )
    } else {
        String::new()
    };

    // Estado del backup diario: último backup (fecha REAL del fichero) y
    // cuenta atrás hasta el siguiente (cadencia de 24 h).
    let (last_date, last_name, last_path, last_ms, next_text) = match d.recent_backups.first() {
        Some(b) => (
            fecha_es(&b.created_at),
            format!("{} · {}", b.name, format_bytes_short(b.size_bytes)),
            b.path.clone(),
            b.created_ms.map(|ms| ms.to_string()).unwrap_or_default(),
            cuenta_atras_texto(b.created_ms),
        ),
        None => (
            "Sin backups todavía".to_string(),
            "Crea el primero con «Crear backup ahora»".to_string(),
            String::new(),
            String::new(),
            "Sin backup previo".to_string(),
        ),
    };

    // Filas de la tabla de backups (el JS las repinta tras crear uno).
    let backup_rows = if d.recent_backups.is_empty() {
        r#"<tr><td colspan="4" class="font-body-sm text-body-sm text-on-surface-variant py-2">Sin backups en el disco todavía.</td></tr>"#.to_string()
    } else {
        d.recent_backups.iter().map(render_backup_row).collect()
    };

    template
        .replace("<!--SLOT:ADMIN_VERSION-->", &esc(&d.version))
        .replace("<!--SLOT:ADMIN_UPTIME-->", &esc(&d.uptime_human))
        .replace("<!--SLOT:ADMIN_UPTIME_SECS-->", &d.uptime_secs.to_string())
        .replace("<!--SLOT:ADMIN_NOTES_COUNT-->", &d.notes_count.to_string())
        .replace("<!--SLOT:ADMIN_CHATS_COUNT-->", &d.chats_count.to_string())
        .replace(
            "<!--SLOT:ADMIN_SPEND-->",
            &format!("${:.2} USD", d.spend_usd),
        )
        .replace(
            "<!--SLOT:ADMIN_BUDGET-->",
            &format!("${:.2} USD", d.budget_usd),
        )
        .replace("<!--SLOT:ADMIN_REQUESTS-->", &d.requests_month.to_string())
        .replace("<!--SLOT:ADMIN_BUDGET_PCT-->", &budget_pct.to_string())
        .replace("<!--SLOT:ADMIN_MCP_ICON-->", mcp_icon)
        .replace("<!--SLOT:ADMIN_MCP_STATUS-->", &mcp_status)
        .replace("<!--SLOT:ADMIN_MCP_LATENCY-->", &mcp_latency)
        .replace("<!--SLOT:ADMIN_OR_ICON-->", or_icon)
        .replace("<!--SLOT:ADMIN_OR_STATUS-->", or_status)
        .replace("<!--SLOT:ADMIN_OR_STATUS_COLOR-->", or_status_color)
        .replace("<!--SLOT:ADMIN_OR_MODEL-->", &esc(&d.openrouter_model))
        .replace("<!--SLOT:ADMIN_OR_CATALOG_STATE-->", or_catalog_state)
        .replace("<!--SLOT:ADMIN_BUDGET_BAR-->", &budget_bar)
        .replace("<!--SLOT:ADMIN_LAST_BACKUP_DATE-->", &esc(&last_date))
        .replace("<!--SLOT:ADMIN_LAST_BACKUP_NAME-->", &esc(&last_name))
        .replace("<!--SLOT:ADMIN_LAST_BACKUP_PATH-->", &esc(&last_path))
        .replace("<!--SLOT:ADMIN_LAST_BACKUP_MS-->", &last_ms)
        .replace("<!--SLOT:ADMIN_NEXT_BACKUP-->", &esc(&next_text))
        .replace(
            "<!--SLOT:ADMIN_BACKUP_COUNT-->",
            &d.backup_count.to_string(),
        )
        .replace("<!--SLOT:ADMIN_BACKUP_ROWS-->", &backup_rows)
}

/// Fecha de un backup en formato español: "19/10/2026 02:05".
///
/// La fecha es la real del fichero (`mtime`); si no se puede parsear se
/// devuelve el texto tal cual, sin inventar ninguna.
fn fecha_es(created_at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(created_at)
        .map(|dt| dt.format("%d/%m/%Y %H:%M").to_string())
        .unwrap_or_else(|_| created_at.to_string())
}

/// Cuenta atrás hasta el próximo backup (24 h tras el último).
///
/// Es una estimación honesta: la tarea periódica corre cada 24 h desde el
/// arranque de la app, así que el reloj real puede adelantarse o atrasarse
/// unos minutos respecto a este cálculo.
fn cuenta_atras_texto(last_ms: Option<i64>) -> String {
    let Some(last) = last_ms else {
        return "Sin backup previo".to_string();
    };
    let restante = last + BACKUP_INTERVAL_MS - chrono::Utc::now().timestamp_millis();
    if restante <= 0 {
        return "Pendiente: ya debería haberse hecho".to_string();
    }
    let horas = restante / 3_600_000;
    let mins = (restante % 3_600_000) / 60_000;
    if horas > 0 {
        format!("en {horas} h {mins} min")
    } else {
        format!("en {mins} min")
    }
}

/// Fila de la tabla de backups: fichero, fecha (es), tamaño y acciones.
///
/// Es la misma fila que pinta el JS tras crear un backup: así la tabla es
/// correcta sin JavaScript y se repinta sin recargar con él.
fn render_backup_row(b: &BackupEntry) -> String {
    format!(
        r#"<tr class="border-b border-outline/20 last:border-0">
<td class="font-code-inline text-[11px] text-on-surface py-1.5 pr-2 truncate max-w-[220px]" title="{path}">{name}</td>
<td class="font-code-inline text-[11px] text-on-surface-variant py-1.5 pr-2 whitespace-nowrap">{fecha}</td>
<td class="font-code-inline text-[11px] text-on-surface-variant py-1.5 pr-2 whitespace-nowrap">{tamano}</td>
<td class="py-1.5">{acciones}</td>
</tr>"#,
        path = esc(&b.name),
        name = esc(&b.name),
        fecha = esc(&fecha_es(&b.created_at)),
        tamano = esc(&format_bytes_short(b.size_bytes)),
        acciones = render_backup_acciones(&b.name),
    )
}

/// Acciones de un backup: descargar el fichero y restaurar la base.
///
/// El `<details>` es el patrón de confirmación inline de las notas: al abrirlo
/// se ve la pregunta y los botones. «Sí, restaurar» lo ejecuta el JS con
/// `fetch` (el shell de Stitch v2 no carga htmx).
fn render_backup_acciones(nombre: &str) -> String {
    let n = esc(nombre);
    let url = format!("/api/admin/backup/download/{}", percent_encode(nombre));
    format!(
        r#"<div class="flex items-center justify-end gap-1">
<a href="{url}" download class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors font-label-md"><span class="material-symbols-outlined text-[16px]">download</span>Descargar</a>
<details class="relative" data-backup-restore>
<summary class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors cursor-pointer select-none font-label-md"><span class="material-symbols-outlined text-[16px]">restore</span>Restaurar</summary>
<div class="absolute right-0 top-full mt-1 z-20 w-72 p-space-md rounded-xl border border-outline-variant bg-surface-container-high shadow-lg">
<p class="font-body-sm text-body-sm text-on-surface mb-space-sm">¿Restaurar la base desde «{n}»? Se sobrescribe el contenido actual.</p>
<p class="hidden font-body-sm text-[11px] text-error mb-space-sm" data-restore-msg></p>
<div class="flex items-center justify-end gap-2">
<button type="button" onclick="this.closest('details').removeAttribute('open')" class="px-space-sm py-1.5 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors font-label-md">Cancelar</button>
<button type="button" data-restore-confirm="{n}" class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg bg-primary-container text-on-primary font-label-md font-semibold hover:opacity-90 transition-opacity"><span class="material-symbols-outlined text-[16px]">restore</span>Sí, restaurar</button>
</div>
</div>
</details>
</div>"#,
        url = url,
        n = n,
    )
}

/// Percent-encoding mínimo para un nombre de fichero dentro de una URL.
///
/// Los backups reales (`app-YYYYmmdd-HHMMSS.db`) no necesitan nada, pero un
/// fichero con espacios o acentos no debe romper el enlace de descarga.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Formatea bytes a string corto: "1.2 KB", "3.4 MB", etc.
fn format_bytes_short(n: u64) -> String {
    if n < 1024 {
        format!("{} B", n)
    } else if n < 1048576 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else if n < 1073741824 {
        format!("{:.1} MB", n as f64 / 1048576.0)
    } else {
        format!("{:.1} GB", n as f64 / 1073741824.0)
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Superficie de administración /admin/...
//
// Rutas (cableadas en handlers/mod.rs con `admin::register_routes`):
//   GET  /admin                            → panel de administración (F9)
//   GET  /admin/metrics                    → métricas del sistema en JSON
//   GET  /admin/openrouter/budget          → presupuesto IA en JSON
//   GET  /admin/backup/list                → listado JSON de `data/backups/`
//   POST /api/admin/backup                 → crear backup (sistema unificado)
//   POST /api/admin/backup/restore         → restaurar la base en vivo
//   GET  /api/admin/backup/download/{name} → descargar el fichero de un backup
//
// El panel usa `GET /admin/backup/list` para repintar la tabla de backups
// después de «Crear backup ahora», sin recargar la página.
//
// Los items van marcados con `#[allow(dead_code)]` como red de seguridad:
// si el orquestador deja de cablear alguna de estas rutas, el resto del
// módulo sigue compilando. No se exponen secrets: solo flags booleanos y
// cifras; ni SESSION_SECRET, ni API keys, ni el .env.

use axum::{middleware::from_fn_with_state, routing::get, Router};

use crate::middleware::auth::require_auth_state;

/// Directorio de backups que lista `GET /admin/backup/list` (spec Fase 9).
///
/// Es el directorio único del sistema unificado (`crate::backup::DEFAULT_DIR`):
/// el botón legacy "Crear backup", la tarea periódica, el listado, la
/// descarga y la restauración usan todos `data/backups/`.
#[allow(dead_code)]
const ADMIN_BACKUPS_DIR: &str = crate::backup::DEFAULT_DIR;

/// Registra las rutas de administración de la Fase 9 sobre un router con estado.
///
/// Patrón idéntico a `crate::academic::register_routes`: sub-router propio con
/// `route_layer(require_auth_state)`, merged al router que llega. Sin sesión →
/// el middleware redirige (302 a /login; 401 solo bajo /api/* según su política)
/// — el handler nunca ve la request sin sesión válida (o AUTH_DISABLED=true).
#[allow(dead_code)]
pub fn register_routes(
    router: Router<Arc<AppState>>,
    state: Arc<AppState>,
) -> Router<Arc<AppState>> {
    let _ = state; // el estado lo aporta el router externo; solo se clona en la capa

    let admin = Router::new()
        .route("/admin", get(dashboard))
        .route("/admin/metrics", get(system_metrics))
        .route("/admin/openrouter/budget", get(openrouter_budget))
        .route(
            "/admin/openrouter/refresh-catalog",
            post(openrouter_refresh_catalog),
        )
        .route("/admin/backup/list", get(backup_list))
        .route_layer(from_fn_with_state(state.clone(), require_auth_state));

    router.merge(admin)
}

/// GET /admin — panel de administración, HTML server-rendered (shell Stitch).
///
/// Reutiliza la recolección (`fetch_admin_data`) y el render legacy
/// (`render_admin_content`: uptime, MCP, OpenRouter, backups) y añade la
/// tarjeta de recursos del sistema de la Fase 9 (RAM / disco / DB). Envuelto
/// con `page_ext` para compartir head + nav-rail + top-bar del resto de las
/// pantallas fuera de /dashboard.
#[allow(dead_code)]
pub async fn dashboard(State(state): State<Arc<AppState>>) -> (StatusCode, Html<String>) {
    let data = fetch_admin_data(&state).await;
    let system_cards = system_section_html(&state).await;
    let content = format!("{}{}", render_admin_content(&data), system_cards);
    let html = crate::academic::templates::page_ext(
        "Admin — Study Platform",
        "Administración",
        "/admin",
        &content,
    );
    (StatusCode::OK, Html(html))
}

/// POST /admin/openrouter/refresh-catalog — fuerza el refresh del catálogo de modelos.
///
/// Utiliza el método privado `CacheCatalogo::refresh` para invalidar el cache
/// actual y descargar un nuevo catálogo desde OpenRouter (o volver al respaldo
/// si el proveedor no responde). El admin puede usar este endpoint para hacer
/// que los nuevos modelos free aparezcan más rápido que el TTL habitual (30 min).
#[allow(dead_code)]
pub async fn openrouter_refresh_catalog(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let key = state.config.openrouter_api_key.as_deref();
    match state.cache_catalogo.refresh(key).await {
        Some(modelos) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "success": true,
                "count": modelos.len(),
                "message": "Catálogo refrescado exitosamente",
            })),
        ),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "error": "No se pudo refrescar el catálogo (proveedor no disponible)",
            })),
        ),
    }
}

/// GET /admin/metrics — métricas del sistema en JSON.
///
/// Fuentes: uptime desde `AppState.start_time`; RAM desde `/proc/meminfo`
/// (MemTotal − MemAvailable); disco libre vía `df -kP` del FS que aloja los
/// datos; bytes del archivo SQLite desde `config.db_path`. Cada métrica
/// degrada a 0 cuando la fuente no está disponible — nada se inventa.
#[allow(dead_code)]
pub async fn system_metrics(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let (ram_used_mb, ram_total_mb, disk_free_mb, db_bytes) = system_files_data(&state).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "uptime_secs": state.start_time.elapsed().as_secs(),
            "ram_mb_used": ram_used_mb,
            "ram_mb_total": ram_total_mb,
            "disk_mb_free": disk_free_mb,
            "db_bytes": db_bytes,
        })),
    )
}

/// GET /admin/openrouter/budget — presupuesto IA en JSON.
///
/// `spend_usd` sale de `SELECT COALESCE(SUM(cost_usd),0) FROM usage_metrics`,
/// previa verificación de existencia de la tabla en `sqlite_master`: si no
/// existe se responde con `"available": false` y gasto 0, sin crear la tabla.
/// `alert` reutiliza `is_alert` de `crate::ai::openrouter` (≥ 90 % del tope).
#[allow(dead_code)]
pub async fn openrouter_budget(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let table_exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'usage_metrics'",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);

    let available = table_exists > 0;
    // El tope `openrouter_max_cost_usd` es MENSUAL: el gasto que se compara
    // debe ser el del mes en curso, no la suma de todo el histórico.
    let spend_usd = if available {
        crate::ai::openrouter::persistent_summary(&state.db, &state.config)
            .await
            .map(|s| s.total_cost)
            .unwrap_or(0.0)
    } else {
        0.0
    };

    let budget_usd = state.config.openrouter_max_cost_usd;
    let alert = crate::ai::openrouter::is_alert(spend_usd, budget_usd);
    let pct = if budget_usd > 0.0 {
        ((spend_usd / budget_usd) * 100.0).round().clamp(0.0, 100.0)
    } else {
        0.0
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "available": available,
            "spend_usd": spend_usd,
            "budget_usd": budget_usd,
            "pct": pct,
            "alert": alert,
        })),
    )
}

/// GET /admin/backup/list — listado de `data/backups/` en JSON.
///
/// IO asíncrono (`tokio::fs`). Sólo archivos regulares `*.db` (los sidecars
/// `.sha256` son el control de integridad interno, no copias); se ordena desc
/// por nombre (los nombres llevan timestamp, el orden lexicográfico es el
/// cronológico). `created` va en RFC-3339 y `created_es` con la fecha ya
/// formateada en la zona horaria del servidor. Dir inexistente o ilegible →
/// lista vacía, sin nada inventado.
#[allow(dead_code)]
pub async fn backup_list() -> (StatusCode, Json<serde_json::Value>) {
    let items = list_backup_files(ADMIN_BACKUPS_DIR).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "dir": ADMIN_BACKUPS_DIR,
            "count": items.len(),
            "items": items,
        })),
    )
}

// ─── Sistema unificado de backups: crear / restaurar / descargar ────────────
// Rutas pendientes de cableo (las añade el orquestador en handlers/mod.rs):
//   POST /admin/backup/now            → admin::backup_now_handler
//   POST /admin/backup/restore         → admin::backup_restore_handler
//   GET  /admin/backup/download/{name} → admin::backup_download_handler

/// Cuerpo de la petición de restauración.
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
pub struct BackupRestoreRequest {
    /// Nombre del fichero de backup: `app-YYYYmmdd-HHMMSS.db`.
    pub name: String,
}

/// Nombre de backup admitible: `app-<algo>.db`, sin separadores ni `..`.
///
/// Réplica local de la comprobación que hace `crate::backup` (que no la
/// exporta): permite rechazar la petición **antes** de tocar el disco. Un
/// nombre con `..`, una barra, una contrabarra o un carácter fuera del
/// conjunto seguro no es un backup de este sistema.
#[allow(dead_code)]
fn nombre_backup_valido(name: &str) -> bool {
    name.len() > "app-".len() + ".db".len()
        && name.starts_with("app-")
        && name.ends_with(".db")
        && !name.contains("..")
        && !name.contains('/')
        && !name.contains('\\')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// POST /admin/backup/now — crea un backup con el sistema unificado.
///
/// Envuelve `crate::backup::backup_now` (checkpoint del WAL + copia + sidecar
/// de integridad) sobre `data/backups/`. Responde `{success, name, bytes}`.
#[allow(dead_code)]
pub async fn backup_now_handler(State(state): State<Arc<AppState>>) -> Response {
    let dir = std::path::Path::new(crate::backup::DEFAULT_DIR);
    match crate::backup::backup_now(&state.db, dir).await {
        Ok(copia) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "success": true,
                "name": copia.name,
                "bytes": copia.bytes,
            })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("backup falló: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "error": e.to_string(),
                })),
            )
                .into_response()
        }
    }
}

/// POST /admin/backup/restore — restaura la base en vivo desde un backup.
///
/// Valida el nombre **antes** de tocar nada: sólo se admiten ficheros
/// `app-*.db`; un nombre letal (ruta, `..`, otro prefijo) responde 400 con
/// `{error}` y no llega al módulo de backup. La restauración en sí comprueba
/// la integridad del fichero antes de sobrescribir la base en vivo.
///
/// Si la base está bloqueada (SQLITE_BUSY) responde **400** con el aviso
/// honesto de "server en uso" (`{error, aviso: true}`): no es un fallo del
/// backup y no se ha cambiado nada. El panel lo muestra tal cual.
#[allow(dead_code)]
pub async fn backup_restore_handler(
    State(state): State<Arc<AppState>>,
    Json(peticion): Json<BackupRestoreRequest>,
) -> Response {
    let dir = std::path::Path::new(crate::backup::DEFAULT_DIR);
    let name = peticion.name.trim();

    // Validación previa (letal): nombre no admitido → 400, sin tocar disco.
    if !nombre_backup_valido(name) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "nombre de backup no válido" })),
        )
            .into_response();
    }

    // El fichero tiene que existir: `restore` lo abriría en exclusiva.
    if !tokio::fs::try_exists(dir.join(name)).await.unwrap_or(false) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "backup no encontrado" })),
        )
            .into_response();
    }

    match crate::backup::restore(&state.db, dir, name).await {
        Ok(copia) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "success": true,
                "name": copia.name,
                "bytes": copia.bytes,
            })),
        )
            .into_response(),
        Err(crate::backup::BackupError::Unsafe) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "nombre de backup no válido" })),
        )
            .into_response(),
        Err(crate::backup::BackupError::Integrity) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "el backup no supera el control de integridad"
            })),
        )
            .into_response(),
        // La base está bloqueada por otra conexión: es un aviso honesto (no un
        // fallo del backup), y el panel lo muestra tal cual en vez de un error
        // genérico. No se ha cambiado nada en la base en vivo.
        Err(e) => match mensaje_en_uso(&e) {
            Some(mensaje) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": mensaje,
                    "aviso": true,
                })),
            )
                .into_response(),
            None => {
                tracing::error!("restore falló: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        },
    }
}

/// Mensaje honesto de "la base está en uso" (SQLITE_BUSY) del restore.
///
/// `crate::backup` lo devuelve como `BackupError::Sqlite` con el texto
/// "server en uso: reintenta tras detener la app (...)". Aquí se recupera ese
/// texto tal cual (sin el prefijo técnico de la variante del error) para que
/// el panel lo muestre como aviso, no como un fallo genérico.
fn mensaje_en_uso(e: &crate::backup::BackupError) -> Option<String> {
    if let crate::backup::BackupError::Sqlite(msg) = e {
        if msg.contains("server en uso") {
            return Some(msg.clone());
        }
    }
    None
}

/// GET /admin/backup/download/{name} — descarga el fichero de un backup.
///
/// El nombre viene en la URL: se valida con el mismo criterio que la
/// restauración (`app-*.db`, sin `..` ni separadores) y **se ignora** como
/// ruta — el fichero se busca siempre dentro de `data/backups/`. Un nombre no
/// admitido o inexistente responde 404 (no se revela qué hay en el disco).
#[allow(dead_code)]
pub async fn backup_download_handler(Path(name): Path<String>) -> Response {
    let dir = std::path::Path::new(crate::backup::DEFAULT_DIR);
    let name = name.trim();

    if !nombre_backup_valido(name) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "backup no encontrado" })),
        )
            .into_response();
    }

    let bytes = match tokio::fs::read(dir.join(name)).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("descarga de backup falló ({name}): {e}");
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "backup no encontrado" })),
            )
                .into_response();
        }
    };

    let mut respuesta = Response::new(Body::from(bytes));
    let cabeceras = respuesta.headers_mut();
    cabeceras.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.sqlite3"),
    );
    cabeceras.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    respuesta
}

// ─── Helpers Fase 9 ──────────────────────────────────────────────────────────

/// Lee `data/backups/` y arma el array `{nombre, bytes, created}`.
/// `created` va a RFC-3339; null cuando el FS no expone la marca de tiempo.
///
/// Sólo entran los ficheros de backup (`*.db`): los sidecars `.sha256` son
/// el control de integridad interno de cada copia, no copias, y no deben
/// aparecer como "backups" en el panel ni poder descargarse como tales.
#[allow(dead_code)]
async fn list_backup_files(dir: &str) -> Vec<serde_json::Value> {
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };

    let mut files: Vec<(String, u64, Option<String>, Option<String>)> = Vec::new();
    while let Ok(Some(entry)) = rd.next_entry().await {
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let nombre = entry.file_name().to_string_lossy().into_owned();
        if !nombre.ends_with(".db") {
            continue;
        }
        let bytes = meta.len();
        // `modified()` primero: es la misma marca que usa el render del
        // panel, así la tabla no cambia de fecha al repintarse con el JS.
        let created = meta
            .modified()
            .or_else(|_| meta.created())
            .ok()
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
        // La fecha ya formateada en español y en la zona horaria del
        // servidor: el JS la usa tal cual para que la tabla no cambie de
        // hora al repintarse (la del navegador puede ser otra).
        let created_es = meta.modified().or_else(|_| meta.created()).ok().map(|t| {
            chrono::DateTime::<chrono::Local>::from(t)
                .format("%d/%m/%Y %H:%M")
                .to_string()
        });
        files.push((nombre, bytes, created, created_es));
    }

    files.sort_by(|a, b| b.0.cmp(&a.0));

    files
        .into_iter()
        .map(|(nombre, bytes, created, created_es)| {
            serde_json::json!({
                "nombre": nombre,
                "bytes": bytes,
                "created": created,
                "created_es": created_es,
            })
        })
        .collect()
}

/// Recolecta los recursos de la Fase 9: RAM (MB, base 1024) usada/total, MB
/// libres del FS que aloja los datos y bytes del archivo SQLite. Degrada a 0
/// lo que no pueda leerse.
#[allow(dead_code)]
async fn system_files_data(state: &Arc<AppState>) -> (u64, u64, u64, u64) {
    // RAM: MemTotal y MemAvailable de /proc/meminfo (kB). Sin MemTotal
    // (no es Linux) → 0/0 honesto; sin MemAvailable → usado = total.
    let (ram_used_mb, ram_total_mb) = match tokio::fs::read_to_string("/proc/meminfo").await {
        Ok(text) => {
            let total_kb = meminfo_kb(&text, "MemTotal:");
            let avail_kb = meminfo_kb(&text, "MemAvailable:");
            (total_kb.saturating_sub(avail_kb) / 1024, total_kb / 1024)
        }
        Err(_) => (0, 0),
    };

    // Disco libre del FS que aloja ./data (fallback al cwd si no existe).
    let data_dir = state
        .config
        .db_path
        .parent()
        .filter(|p| p.exists())
        .unwrap_or_else(|| std::path::Path::new("."));
    let disk_free_mb = df_free_mb(data_dir).await;

    // Tamaño del archivo de la base de datos (0 si no existe aún).
    let db_bytes = tokio::fs::metadata(&state.config.db_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    (ram_used_mb, ram_total_mb, disk_free_mb, db_bytes)
}

/// Extrae el valor kB de una clave de `/proc/meminfo` (p.ej. "MemTotal:").
#[allow(dead_code)]
fn meminfo_kb(text: &str, key: &str) -> u64 {
    text.lines()
        .filter(|line| line.starts_with(key))
        .filter_map(|line| {
            line.split_whitespace()
                .nth(1)
                .and_then(|v| v.parse::<u64>().ok())
        })
        .next()
        .unwrap_or(0)
}

/// MB disponibles en el FS de `path`, vía `df -kP` (POSIX; sin crates nuevos).
///
/// Llamada asíncrona para no bloquear el executor. La cabecera y las filas
/// partidas por nombres de dispositivo largos se descartan al exigir campos
/// numéricos en la columna `Available`; 0 si no hay `df` en el sistema.
#[allow(dead_code)]
async fn df_free_mb(path: &std::path::Path) -> u64 {
    let output = match tokio::process::Command::new("df")
        .arg("-kP")
        .arg(path)
        .output()
        .await
    {
        Ok(out) if out.status.success() && !out.stdout.is_empty() => out,
        _ => return 0,
    };

    let available_kb: Vec<u64> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() >= 5 {
                cols[3].parse::<u64>().ok()
            } else {
                None
            }
        })
        .collect();
    available_kb.first().map_or(0, |kb| kb / 1024) // KiB → MB
}

/// Tarjetas "Recursos del sistema" del dashboard (server-rendered, sin JS),
/// con los mismos tokens Stitch que el resto del panel.
#[allow(dead_code)]
async fn system_section_html(state: &Arc<AppState>) -> String {
    let (ram_used_mb, ram_total_mb, disk_free_mb, db_bytes) = system_files_data(state).await;

    let ram_value = if ram_total_mb > 0 {
        format!("{}/{} MB", ram_used_mb, ram_total_mb)
    } else {
        "—".to_string()
    };
    let disk_value = if disk_free_mb > 0 {
        format!("{} MB", disk_free_mb)
    } else {
        "—".to_string()
    };
    let db_value = if db_bytes > 0 {
        format_bytes_short(db_bytes)
    } else {
        "—".to_string()
    };
    let db_path = state.config.db_path.to_string_lossy().to_string();

    format!(
        r#"<div class="grid grid-cols-1 md:grid-cols-3 gap-space-md mb-space-md">
    <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
        <div class="font-code-inline text-[11px] text-on-surface-variant uppercase tracking-wider">RAM usada</div>
        <div class="font-headline-md text-headline-md text-primary-container font-bold mt-1">{}</div>
        <div class="font-body-sm text-[10px] text-on-surface-variant mt-1">sistema (meminfo)</div>
    </div>
    <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
        <div class="font-code-inline text-[11px] text-on-surface-variant uppercase tracking-wider">Disco libre</div>
        <div class="font-headline-md text-headline-md text-primary-container font-bold mt-1">{}</div>
        <div class="font-body-sm text-[10px] text-on-surface-variant mt-1">FS de datos (df)</div>
    </div>
    <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
        <div class="font-code-inline text-[11px] text-on-surface-variant uppercase tracking-wider">Base de datos</div>
        <div class="font-headline-md text-headline-md text-primary-container font-bold mt-1">{}</div>
        <div class="font-code-inline text-[10px] text-on-surface-variant mt-1" title="{}">{}</div>
    </div>
</div>"#,
        ram_value,
        disk_value,
        db_value,
        esc(&db_path),
        esc(&db_path),
    )
}
