//! MCP client over SSE/HTTP for the Mikareno student server.
//!
//! Implements the Subset MCP protocol needed by the Study Platform:
//!   1. `initialize`          → establishes a session, captures `mcp-session-id`.
//!   2. `tools/list`          → enumerates available tools.
//!   3. `tools/call`          → invokes a tool and decodes its `text` payload.
//!
//! Transport details (matches the FastMCP `transport="http"` server in
//! `mikareno-mcp/server.py` and the reference client `test_all.py`):
//!   * POST JSON-RPC 2.0 to the SSE endpoint (`base_url`).
//!   * The *first* `initialize` response carries an `Mcp-Session-Id` header
//!     (a UUID). Every subsequent request echoes it back via the same header.
//!   * The body is SSE: one or more `data: <jsonrpc>` lines, but it may also be
//!     a bare JSON document (stateless servers). Both are handled.
//!   * `tools/call` results are `{"content":[{"type":"text","text":"<json>"}]}`
//!     — the `text` is *double-encoded*, so we parse it back into a `Value`.
//!
//! The client is read-only by construction: there is simply no way to issue a
//! write-style method — `call_tool` only forwards the tool names the adapters
//! use, all of which are `get_*` reads. It is also `Clone`.
//!
//! ## Latencia y memoria
//!
//! El servidor MCP re-scrapea el portal del colegio y tarda ~60 s; por eso la
//! caché es *stale-while-revalidate*:
//!
//! * entrada fresca → se devuelve al instante;
//! * entrada expirada → se devuelve **al instante** y el refresco corre en
//!   background (`tokio::spawn`) para la siguiente petición;
//! * sin entrada → fetch bloqueante con deadline corto
//!   ([`FIRST_FETCH_DEADLINE_SECS`]) y error honesto si no llega.
//!
//! Además hay un *circuit breaker*: tras [`BREAKER_FAILURE_THRESHOLD`] fallos
//! consecutivos las llamadas fallan rápido (sin tocar la red) durante
//! [`BREAKER_OPEN_SECS`] segundos, y la caché está acotada (entradas y bytes
//! por entrada) para que el RSS no crezca sin límite.

use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, ACCEPT};
use serde_json::{json, Value};
use thiserror::Error;
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::integrations::mikareno::adapters;
use crate::integrations::mikareno::cache::{
    SseCache, DEFAULT_MAX_ENTRIES, DEFAULT_MAX_ENTRY_BYTES,
};
use crate::integrations::mikareno::models::{
    CalendarioEvent, Horario, Nota, Perfil, Tarea, ToolInfo,
};

/// Default cache TTL (seconds) — mirrors the server's own cache window.
pub const DEFAULT_CACHE_TTL_SECS: u64 = 180;
/// Per-request timeout (cap of a single HTTP exchange).
///
/// It is deliberately larger than the call deadlines below: the per-call
/// deadline is what bounds the user-visible latency, this one only stops a
/// single hung socket. The background refresh (which may legitimately wait
/// for the server's ~1 min scrape) needs the headroom.
pub const REQUEST_TIMEOUT_SECS: u64 = 90;
/// Maximum number of retries after the initial attempt.
pub const MAX_RETRIES: u32 = 3;
/// Backoff base in milliseconds; doubles with each retry (500, 1000, 2000).
pub const BACKOFF_BASE_MS: u64 = 500;
/// Deadline total por llamada MCP (tools/list, tools/call). Con el servidor
/// colgado (acepta HTTP pero no responde), la página degrada a estado vacío
/// en este plazo en vez de quedarse clavada en retries (3 × 30 s).
pub const CALL_DEADLINE_SECS: u64 = 6;
/// Deadline del handshake `initialize` (POST + reintentos). Evita que un
/// servidor MCP colgado bloquee un handler durante 3 × 30 s de reintentos.
pub const INIT_DEADLINE_SECS: u64 = 8;
/// Deadline del *primer* fetch (cache fría): incluye `initialize` + la
/// llamada. Es corto a propósito: si no hay nada cacheado se devuelve un
/// error honesto en vez de dejar la página colgada un minuto esperando el
/// re-scrape del servidor.
pub const FIRST_FETCH_DEADLINE_SECS: u64 = 8;
/// Deadline del refresco en background (stale-while-revalidate). Generoso:
/// el servidor puede tardar ~60 s en re-scrapear.
pub const REFRESH_DEADLINE_SECS: u64 = 120;
/// Deadline del aviso `notifications/initialized` (best-effort).
pub const NOTIFY_DEADLINE_SECS: u64 = 5;
/// Máximo de entradas vivas en la caché (evita el crecimiento sin límite).
pub const CACHE_MAX_ENTRIES: usize = DEFAULT_MAX_ENTRIES;
/// Tamaño máximo estimado (bytes) de un payload cacheado (2 MiB). Un payload
/// mayor se sirve pero no se cachea.
pub const CACHE_MAX_ENTRY_BYTES: usize = DEFAULT_MAX_ENTRY_BYTES;
/// Fallos consecutivos que abren el circuit breaker.
pub const BREAKER_FAILURE_THRESHOLD: u32 = 3;
/// Tiempo que el circuit breaker permanece abierto (fallo rápido, sin red).
pub const BREAKER_OPEN_SECS: u64 = 30;

/// Errors returned by the Mikareno MCP client.
#[derive(Debug, Error)]
pub enum MikarenoError {
    #[error("network or HTTP error: {0}")]
    Http(String),
    #[error("not connected to the MCP session")]
    NotConnected,
    #[error("failed to parse SSE/JSON-RPC response: {0}")]
    SseParse(String),
    #[error("JSON-RPC error (code {code}): {message}")]
    JsonRpc { code: i64, message: String },
    #[error("unexpected response shape: {0}")]
    Unexpected(String),
}

impl MikarenoError {
    pub fn is_connection_error(&self) -> bool {
        matches!(self, MikarenoError::Http(_))
    }
}

/// Extrae el host (sin puerto) de una URL `http(s)://host[:port]/path`.
fn host_of(base_url: &str) -> Option<String> {
    let rest = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))?;
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Credenciales `user:pass@host` → se queda con el host.
    let hostport = hostport.rsplit('@').next().unwrap_or(hostport);
    // Quita el puerto, pero respeta los corchetes de IPv6 literal.
    let host = if let Some(rest) = hostport.strip_prefix('[') {
        rest.split(']').next().unwrap_or("").to_string()
    } else {
        hostport.split(':').next().unwrap_or("").to_string()
    };
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// ¿El host es alcanzable desde esta máquina sin salir a Internet?
///
/// Acepta loopback, RFC 1918 y CGNAT `100.64/10` (el rango de la tailnet de
/// Tailscale). Rechaza hosts públicos: el MCP de Mikareno es de sólo lectura
/// pero no debe apuntar a un MCP de terceros por accidente.
fn is_reachable_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let Ok(ip) = host.parse::<std::net::IpAddr>() else {
        // Hostname no-IP: sólo se acepta `localhost` (tratado arriba).
        return false;
    };
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                // CGNAT 100.64.0.0/10 — la tailnet de Tailscale.
                || (o[0] == 100 && (o[1] & 0b1100_0000) == 0b0100_0000)
        }
        std::net::IpAddr::V6(v6) => {
            // `is_unique_local` cubre fc00::/7, que incluye el fd7a:115c:a1e0::/48
            // que Tailscale asigna a los nodos de la tailnet.
            v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local()
        }
    }
}

/// ¿La respuesta HTTP indica que la sesión MCP ya no existe?
///
/// El servidor MCP responde `404` con un cuerpo JSON-RPC de error cuando el
/// `mcp-session-id` enviado no corresponde a ninguna sesión viva:
///
/// ```text
/// {"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Session not found"}}
/// ```
///
/// Es la señal de que hay que tirar la sesión cacheada, hacer un
/// `initialize` nuevo y reintentar la llamada una vez.
fn is_session_gone(status: u16, body: &str) -> bool {
    if status != 404 {
        return false;
    }
    let lower = body.to_ascii_lowercase();
    lower.contains("session not found") // gestor de sesiones
        || lower.contains("invalid or expired session") // transporte
        || lower.contains("session has been terminated") // DELETE previo
}

/// Extrae el `mcp-session-id` de las cabeceras de una respuesta.
///
/// `HeaderMap::get` de `reqwest`/`http` ya es *case-insensitive* para los
/// nombres de cabecera, así que `mcp-session-id` encuentra también
/// `Mcp-Session-Id` (que es como lo emite el servidor). Se mantiene el
/// `or_else` explícito por claridad y por si el servidor usara otra
/// capitalización.
fn extract_session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("mcp-session-id")
        .or_else(|| headers.get("Mcp-Session-Id"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Shared, interior-mutable state behind the client.
///
/// Held behind an `Arc` so the client can be freely cloned and the methods
/// stay `&self` (required for ergonomic use from request handlers). The mutexes
/// guard only tiny, non-async critical sections.
struct ClientState {
    /// Current `mcp-session-id`, present only after a successful `initialize`.
    ///
    /// Shared by every clone of the client (vive en el `Arc<ClientState>`):
    /// el id que un clon devuelve en `session_id()` es el mismo que usan
    /// los demás clones al reenviar la cabecera `mcp-session-id`.
    session_id: Mutex<Option<String>>,
    /// Response cache (keyed by `tool(args)`), TTL-bounded.
    cache: Mutex<SseCache>,
    /// Notification `notifications/initialized` already sent, if any.
    ///
    /// El protocolo MCP exige enviarla una sola vez por sesión; se guarda
    /// a qué sesión (`mcp-session-id`) se asocia para no repetirla.
    notified: Mutex<Option<NotificationState>>,
    /// Whether the client has a valid, initialized session.
    connected: AtomicBool,
    /// Whether `initialize` has already run (separate from `connected`, since a
    /// stateless server may not issue a session id but still answer requests).
    initialized: AtomicBool,
    /// Monotonic request id counter for JSON-RPC `id` fields.
    next_id: AtomicU64,
    /// Claves con un refresco en background en curso.
    ///
    /// Evita lanzar N refrescos paralelos para la misma clave cuando varias
    /// peticiones llegan a la vez con la entrada expirada.
    refreshing: Mutex<HashSet<String>>,
    /// Fallos consecutivos del transporte (circuit breaker).
    consecutive_failures: AtomicU32,
    /// Instante hasta el que el circuit breaker queda abierto.
    breaker_open_until: Mutex<Option<Instant>>,
}

/// Qué sesión tiene ya hecha la notificación `notifications/initialized`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NotificationState {
    /// Sesión (`mcp-session-id`) a la que se asocia la notificación.
    /// `None` = servidor sin sesiones (stateless).
    session_id: Option<String>,
}

/// Error interno del transporte: distingue "la sesión ya no existe" del resto
/// de fallos, para poder invalidarla, re-inicializar y reintentar una vez.
#[derive(Debug)]
enum TransportError {
    /// El servidor respondió 404 (`Session not found`): la sesión cacheada
    /// murió (expiró, el servidor se reinició o cambió de credencial).
    SessionGone(String),
    /// Cualquier otro fallo: red, HTTP 5xx agotado, parseo, etc.
    Other(MikarenoError),
}

impl TransportError {
    /// Convierte a la variante pública de error.
    ///
    /// El mensaje conserva "404" y "Session not found" para que los
    /// detectores de sesión vencida del resto de la app (que miran el texto
    /// del error) sigan funcionando.
    fn into_mikareno(self) -> MikarenoError {
        match self {
            TransportError::SessionGone(reason) => MikarenoError::Http(format!(
                "MCP session not found (HTTP 404): {reason}"
            )),
            TransportError::Other(e) => e,
        }
    }
}

/// A read-only MCP client for the Mikareno student server.
#[derive(Clone)]
pub struct MikarenoClient {
    base_url: String,
    http: reqwest::Client,
    state: std::sync::Arc<ClientState>,
}

impl MikarenoClient {
    /// Create a new client pointing at `base_url` (e.g.
    /// `http://127.0.0.1:8080/mcp`) with the default 180s cache TTL.
    pub fn new(base_url: String) -> Self {
        Self::with_cache_ttl(base_url, DEFAULT_CACHE_TTL_SECS)
    }

    /// Crea el cliente sólo si `base_url` apunta a un MCP de Mikareno
    /// alcanzable desde esta máquina: loopback o una dirección privada.
    ///
    /// Se aceptan tres clases de host:
    ///   * loopback (`127.0.0.1`, `::1`, `localhost`);
    ///   * RFC 1918 (`10/8`, `172.16/12`, `192.168/16`);
    ///   * CGNAT `100.64/10`, que es el rango que Tailscale asigna a los nodos
    ///     de la tailnet (p. ej. `100.115.62.97`).
    ///
    /// Cualquier host público se rechaza: el cliente es de sólo lectura pero
    /// no debe apuntar a un MCP de terceros por accidente. En producción la
    /// URL configurada es la del MCP servido en la tailnet, que cae en CGNAT.
    pub fn from_config(base_url: &str) -> Option<Self> {
        if base_url.is_empty() {
            return None;
        }
        let host = host_of(base_url)?;
        if is_reachable_host(&host) {
            Some(Self::new(base_url.to_string()))
        } else {
            None
        }
    }

    /// Igual que [`MikarenoClient::from_config`], pero además exige que el
    /// host sea loopback. Se mantiene para los tests que quieren garantizar
    /// que no se intenta ninguna conexión saliente.
    pub fn maybe_local(base_url: &str) -> Option<Self> {
        let host = host_of(base_url)?;
        if host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
        {
            Some(Self::new(base_url.to_string()))
        } else {
            None
        }
    }

    /// Create a new client with a custom cache TTL.
    pub fn with_cache_ttl(base_url: String, ttl_secs: u64) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .user_agent("study-platform/0.1.0 (MikarenoMCPClient)")
            .build()
            .expect("failed to build reqwest client — this is a programming error");
        Self {
            base_url,
            http,
            state: std::sync::Arc::new(ClientState {
                session_id: Mutex::new(None),
                cache: Mutex::new(SseCache::with_limits(
                    ttl_secs,
                    CACHE_MAX_ENTRIES,
                    CACHE_MAX_ENTRY_BYTES,
                )),
                notified: Mutex::new(None),
                connected: AtomicBool::new(false),
                initialized: AtomicBool::new(false),
                next_id: AtomicU64::new(1),
                refreshing: Mutex::new(HashSet::new()),
                consecutive_failures: AtomicU32::new(0),
                breaker_open_until: Mutex::new(None),
            }),
        }
    }

    fn next_id(&self) -> u64 {
        self.state.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Returns `true` once `initialize` has completed successfully.
    ///
    /// Cheap on purpose: it only reads an atomic flag, it never talks to the
    /// MCP server (the handlers call it on every render to show the status).
    pub fn is_connected(&self) -> bool {
        self.state.connected.load(Ordering::Acquire)
    }

    // ── circuit breaker ───────────────────────────────────────────────

    /// ¿Está abierto el circuit breaker? (true ⇒ fallar rápido, sin red).
    ///
    /// Al expirar la ventana de 30 s el breaker se rearma solo: el contador de
    /// fallos vuelve a cero y la siguiente petición vuelve a tocar la red.
    fn breaker_is_open(&self) -> bool {
        let mut guard = self
            .state
            .breaker_open_until
            .lock()
            .expect("breaker lock poisoned");
        match *guard {
            Some(until) if Instant::now() < until => true,
            Some(_) => {
                *guard = None;
                self.state.consecutive_failures.store(0, Ordering::Release);
                false
            }
            None => false,
        }
    }

    /// Registra un fallo del transporte y abre el breaker si se acumulan
    /// [`BREAKER_FAILURE_THRESHOLD`] fallos consecutivos.
    fn breaker_on_failure(&self) {
        // Si ya está abierto no se extiende la ventana (evita un ciclo de
        // fallos autoalimentado que mantenga el breaker cerrado para siempre).
        if self.breaker_is_open() {
            return;
        }
        let n = self
            .state
            .consecutive_failures
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        if n >= BREAKER_FAILURE_THRESHOLD {
            let until = Instant::now() + Duration::from_secs(BREAKER_OPEN_SECS);
            *self
                .state
                .breaker_open_until
                .lock()
                .expect("breaker lock poisoned") = Some(until);
            warn!(
                "MCP circuit breaker abierto tras {} fallos consecutivos: las llamadas fallarán rápido durante {}s",
                n, BREAKER_OPEN_SECS
            );
        }
    }

    /// Registra un éxito: rearma el breaker y el contador de fallos.
    fn breaker_on_success(&self) {
        self.state.consecutive_failures.store(0, Ordering::Release);
        *self
            .state
            .breaker_open_until
            .lock()
            .expect("breaker lock poisoned") = None;
    }

    /// Error que se devuelve cuando el breaker está abierto.
    fn breaker_error(op: &str) -> MikarenoError {
        MikarenoError::Unexpected(format!(
            "MCP circuit breaker abierto ({op}): {BREAKER_FAILURE_THRESHOLD}+ fallos consecutivos, \
             no se intenta la red durante {BREAKER_OPEN_SECS}s"
        ))
    }

    // ── cache helpers ─────────────────────────────────────────────────

    /// Lectura fresca de la caché (dentro del TTL).
    fn cache_get_fresh(&self, key: &str) -> Option<Value> {
        let c = self.state.cache.lock().expect("cache lock poisoned");
        c.get(key).cloned()
    }

    /// Lectura *rancia* de la caché (expirada pero conservada).
    ///
    /// Es la base del stale-while-revalidate: se sirve el dato viejo al
    /// instante y se refresca en background.
    fn cache_get_stale(&self, key: &str) -> Option<Value> {
        let c = self.state.cache.lock().expect("cache lock poisoned");
        c.get_stale(key).cloned()
    }

    /// Escritura en la caché; si el payload excede el límite de tamaño no se
    /// cachea (quedaría fuera del presupuesto de RAM).
    fn cache_set(&self, key: &str, value: Value) {
        let mut c = self.state.cache.lock().expect("cache lock poisoned");
        if !c.set(key.to_string(), value) {
            warn!(
                "MCP caché: payload de '{}' supera el límite de {} bytes; no se cachea",
                key, CACHE_MAX_ENTRY_BYTES
            );
        } else {
            debug!(
                "MCP caché: '{}' guardado ({} entradas, ~{} bytes)",
                key,
                c.len(),
                c.total_bytes()
            );
        }
    }

    /// Marca una clave como "refrescando". Devuelve `false` si ya había un
    /// refresco en curso para esa clave.
    fn begin_refresh(&self, key: &str) -> bool {
        let mut r = self
            .state
            .refreshing
            .lock()
            .expect("refreshing lock poisoned");
        r.insert(key.to_string())
    }

    /// Libera la marca de "refrescando" de una clave.
    fn end_refresh(&self, key: &str) {
        let mut r = self
            .state
            .refreshing
            .lock()
            .expect("refreshing lock poisoned");
        r.remove(key);
    }

    /// Mark the session as alive and store the session id (if any).
    fn on_handshake_ok(&self, session_id: Option<String>) {
        {
            let mut s = self.state.session_id.lock().expect("session lock poisoned");
            *s = session_id;
        }
        self.state.initialized.store(true, Ordering::Release);
        self.state.connected.store(true, Ordering::Release);
    }

    /// Mark the session as lost: clears the id and forgets initialization state.
    ///
    /// Also forgets the `notifications/initialized` bookkeeping: the next
    /// handshake will open a *new* session, which needs its own notification.
    fn mark_disconnected(&self) {
        {
            let mut s = self.state.session_id.lock().expect("session lock poisoned");
            *s = None;
        }
        {
            let mut n = self.state.notified.lock().expect("notified lock poisoned");
            *n = None;
        }
        self.state.initialized.store(false, Ordering::Release);
        self.state.connected.store(false, Ordering::Release);
    }

    /// Lazily run `initialize` if we don't yet have a session.
    async fn ensure_initialized(&self) -> Result<(), MikarenoError> {
        if self.state.initialized.load(Ordering::Acquire) {
            return Ok(());
        }
        self.initialize().await
    }

    /// ¿Ya enviamos `notifications/initialized` para esta sesión?
    ///
    /// El protocolo la exige **una sola vez por sesión**:
    ///   * con `mcp-session-id` (servidor stateful): una vez por id;
    ///   * sin id (servidor stateless): una vez, y el servidor —que nace
    ///     listo— la ignora de todas formas.
    fn already_notified(&self, session_id: Option<&str>) -> bool {
        let n = self.state.notified.lock().expect("notified lock poisoned");
        match n.as_ref() {
            Some(st) => st.session_id.as_deref() == session_id,
            None => false,
        }
    }

    /// Marca la sesión actual como notificada (`notifications/initialized`).
    fn mark_notified(&self, session_id: Option<String>) {
        let mut n = self.state.notified.lock().expect("notified lock poisoned");
        *n = Some(NotificationState { session_id });
    }

    // ── low-level transport ───────────────────────────────────────────

    /// POST a JSON-RPC envelope to the SSE endpoint with retries and backoff.
    ///
    /// Returns the response headers (for `mcp-session-id` harvesting) and the
    /// raw body text. Transport-level failures (connection refused, timeout)
    /// mark the session disconnected so the next call re-initializes.
    ///
    /// A `404` whose body mentions a missing session is reported as
    /// [`TransportError::SessionGone`] instead of a plain HTTP error: the
    /// caller can then drop the cached session id and re-handshake.
    async fn post_sse(
        &self,
        body: &Value,
        session: Option<&str>,
    ) -> Result<(HeaderMap, String), TransportError> {
        let mut last_err: Option<MikarenoError> = None;

        for attempt in 0..=MAX_RETRIES {
            let mut builder = self
                .http
                .post(&self.base_url)
                .header(ACCEPT, "application/json, text/event-stream")
                .header("content-type", "application/json")
                .json(body);
            if let Some(sid) = session {
                builder = builder.header("mcp-session-id", sid);
            }

            match builder.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let headers = resp.headers().clone();
                    if status.is_success() {
                        match resp.text().await {
                            Ok(text) => return Ok((headers, text)),
                            Err(e) => {
                                debug!("reqwest body read failed on attempt {}: {}", attempt, e);
                                last_err = Some(MikarenoError::Http(e.to_string()));
                            }
                        }
                    } else if status.as_u16() >= 500 {
                        // Server-side fault — retryable; treat as disconnected.
                        warn!("MCP server error HTTP {} on attempt {}", status, attempt);
                        self.mark_disconnected();
                        let snippet = self.snippet(&resp.text().await.unwrap_or_default());
                        last_err =
                            Some(MikarenoError::Http(format!("HTTP {}: {}", status, snippet)));
                    } else {
                        // 4xx — not a retryable transport fault, surface immediately.
                        let body_text = resp.text().await.unwrap_or_default();
                        let snippet = self.snippet(&body_text);
                        if is_session_gone(status.as_u16(), &body_text) {
                            // The session we cached is dead on the server side.
                            // Drop it so the next handshake starts fresh.
                            warn!(
                                "MCP session not found (HTTP {}): invalidating cached session",
                                status
                            );
                            self.mark_disconnected();
                            return Err(TransportError::SessionGone(snippet));
                        }
                        return Err(TransportError::Other(MikarenoError::Http(format!(
                            "HTTP {}: {}",
                            status, snippet
                        ))));
                    }
                }
                Err(e) => {
                    if e.is_connect() || e.is_timeout() || e.is_request() {
                        self.mark_disconnected();
                    }
                    debug!("reqwest transport error on attempt {}: {}", attempt, e);
                    last_err = Some(MikarenoError::Http(e.to_string()));
                }
            }

            if attempt < MAX_RETRIES {
                let backoff = Duration::from_millis(BACKOFF_BASE_MS * (1u64 << attempt));
                sleep(backoff).await;
            }
        }

        Err(TransportError::Other(
            last_err.unwrap_or(MikarenoError::Http("exhausted retries".into())),
        ))
    }

    fn snippet(&self, text: &str) -> String {
        let t = text.trim();
        if t.len() > 200 {
            format!("{}...", &t[..200])
        } else {
            t.to_string()
        }
    }

    // ── protocol primitives ───────────────────────────────────────────

    /// One handshake round-trip: POST `initialize` and extract the session id.
    ///
    /// `session` is the cached session id to echo back, if any. `None` makes
    /// the server open a brand-new session.
    async fn handshake_once(
        &self,
        body: &Value,
        session: Option<&str>,
    ) -> Result<Option<String>, TransportError> {
        let (headers, text) = self.post_sse(body, session).await?;
        let session_id = extract_session_id(&headers);
        let messages = parse_sse(&text);
        // Valida que el handshake fue real (no un error JSON-RPC).
        extract_jsonrpc(&messages).map_err(TransportError::Other)?;
        Ok(session_id)
    }

    /// Send the `initialize` request and harvest the session id.
    ///
    /// The server is a "streamable HTTP" MCP server: the response to
    /// `initialize` carries the `mcp-session-id` header and **every**
    /// subsequent request must echo it back, otherwise the server answers
    /// `404 Session not found`.
    ///
    /// If we already hold a live session, it is reused (the same id is
    /// echoed back) instead of asking for a new one: every page handler
    /// calls `initialize()`, and each fresh session would pile up on the
    /// server until its idle timeout. Should the cached session be stale,
    /// the server answers `404` and we fall back to a brand-new handshake.
    ///
    /// After the handshake this sends `notifications/initialized` — the MCP
    /// SDK (2.3) refuses requests from a connection that never announced
    /// initialization ("Session not found" / hang). It is sent exactly once
    /// per session: a re-`initialize` on a still-valid session is a no-op
    /// for the notification.
    pub async fn initialize(&self) -> Result<(), MikarenoError> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": self.next_id(),
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {
                    "name": "study-platform",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            },
        });

        // La sesión cacheada, si existe, se reenvía en el handshake.
        let cached = self
            .state
            .session_id
            .lock()
            .expect("session lock poisoned")
            .clone();

        let init = async {
            match self.handshake_once(&body, cached.as_deref()).await {
                Ok(session_id) => Ok::<Option<String>, MikarenoError>(session_id),
                Err(TransportError::SessionGone(reason)) if cached.is_some() => {
                    // La sesión cacheada murió en el servidor: `post_sse` ya
                    // la invalidó. Abrimos una sesión nueva desde cero.
                    warn!(
                        "MCP cached session rejected on initialize ({}): opening a new session",
                        reason
                    );
                    self.handshake_once(&body, None)
                        .await
                        .map_err(TransportError::into_mikareno)
                }
                Err(e) => Err(e.into_mikareno()),
            }
        };

        // The handshake gets its own deadline: `post_sse` retries 3× with
        // backoff, so without this a hung server would block the caller for
        // minutes instead of degrading gracefully.
        let session_id = match tokio::time::timeout(
            Duration::from_secs(INIT_DEADLINE_SECS),
            init,
        )
        .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                warn!(
                    "MCP initialize timed out after {}s; marking session disconnected",
                    INIT_DEADLINE_SECS
                );
                self.mark_disconnected();
                return Err(MikarenoError::Unexpected(format!(
                    "MCP sin respuesta a tiempo (initialize, {}s)",
                    INIT_DEADLINE_SECS
                )));
            }
        };

        debug!(
            "Mikareno initialize ok; session_id={}",
            session_id.as_deref().unwrap_or("(none)")
        );
        self.on_handshake_ok(session_id.clone());

        // `notifications/initialized` — required by the MCP SDK 2.3 server.
        // Sent once per session; a session that was already notified (same
        // `mcp-session-id`) must not be notified twice.
        if !self.already_notified(session_id.as_deref()) {
            if let Err(e) = self.send_initialized_notification(session_id.as_deref()).await {
                // A failed notification is not fatal for the handshake itself,
                // but the server may refuse subsequent calls — log it loudly.
                warn!("MCP notifications/initialized failed: {}", e);
            } else {
                self.mark_notified(session_id.clone());
            }
        }

        Ok(())
    }

    /// Send the `notifications/initialized` notification for `session`.
    ///
    /// This is a JSON-RPC *notification* (no `id`), so the server answers
    /// `202 Accepted` with an empty body. We therefore do not try to parse a
    /// JSON-RPC result out of it.
    async fn send_initialized_notification(
        &self,
        session: Option<&str>,
    ) -> Result<(), MikarenoError> {
        let body = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        });

        let send = async {
            let mut builder = self
                .http
                .post(&self.base_url)
                .header(ACCEPT, "application/json, text/event-stream")
                .header("content-type", "application/json")
                .json(&body);
            if let Some(sid) = session {
                builder = builder.header("mcp-session-id", sid);
            }

            let resp = builder.send().await.map_err(|e| {
                MikarenoError::Http(format!("notifications/initialized: {}", e))
            })?;
            let status = resp.status();
            if status.is_success() {
                Ok::<(), MikarenoError>(())
            } else {
                let body = resp.text().await.unwrap_or_default();
                if is_session_gone(status.as_u16(), &body) {
                    // La sesión murió entre el handshake y la notificación:
                    // invalidarla para que la siguiente llamada re-inicialice.
                    warn!("MCP session not found while notifying initialized");
                    self.mark_disconnected();
                }
                let snippet = self.snippet(&body);
                Err(MikarenoError::Http(format!(
                    "notifications/initialized HTTP {}: {}",
                    status, snippet
                )))
            }
        };

        // Deadline propio y corto: la notificación es best-effort y no debe
        // alargar el handshake (que sí tiene presupuesto).
        match tokio::time::timeout(Duration::from_secs(NOTIFY_DEADLINE_SECS), send).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                warn!("MCP notifications/initialized timed out");
                Err(MikarenoError::Unexpected(
                    "MCP notifications/initialized sin respuesta a tiempo".to_string(),
                ))
            }
        }
    }

    /// Ejecuta una llamada JSON-RPC (`tools/list`, `tools/call`) contra el
    /// servidor usando la sesión cacheada, con el deadline estándar
    /// ([`CALL_DEADLINE_SECS`]).
    async fn rpc_call(
        &self,
        method: &str,
        params: Option<Value>,
        op: &str,
    ) -> Result<Value, MikarenoError> {
        self.rpc_call_with_deadline(method, params, op, Duration::from_secs(CALL_DEADLINE_SECS))
            .await
    }

    /// Igual que [`MikarenoClient::rpc_call`] pero con un deadline propio.
    ///
    /// El refresco en background usa un deadline largo (el servidor puede
    /// tardar ~60 s en re-scrapear) mientras que la ruta en primer plano usa
    /// el corto ([`CALL_DEADLINE_SECS`]), para que la página nunca espere.
    ///
    /// Recuperación real: si el servidor responde `404 Session not found`
    /// (la sesión murió: expiró, el servidor se reinició o cambió de
    /// credencial), se invalida la sesión, se hace un `initialize` nuevo y
    /// se reintenta **una** vez. El segundo intento usa la sesión fresca.
    ///
    /// Todo el proceso (intento + recuperación + reintento) cabe en el
    /// deadline: si no, la página se queda colgada.
    async fn rpc_call_with_deadline(
        &self,
        method: &str,
        params: Option<Value>,
        op: &str,
        budget: Duration,
    ) -> Result<Value, MikarenoError> {
        // Circuit breaker abierto: se falla rápido, sin tocar la red.
        if self.breaker_is_open() {
            return Err(Self::breaker_error(op));
        }

        self.ensure_initialized().await?;

        let deadline = tokio::time::Instant::now() + budget;
        let mut retried = false;

        loop {
            // El id de sesión se relee en cada intento: tras una recuperación
            // es el nuevo, no el que el servidor rechazó.
            let session = self
                .state
                .session_id
                .lock()
                .expect("session lock poisoned")
                .clone();

            let mut body = json!({
                "jsonrpc": "2.0",
                "id": self.next_id(),
                "method": method,
            });
            // `params` se omite cuando no aplica (`tools/list`): el servidor
            // valida el cuerpo contra el esquema del método y un `params: null`
            // explícito no es lo mismo que su ausencia.
            if let Some(p) = params.clone() {
                body["params"] = p;
            }

            let round = async {
                let (_headers, text) = self.post_sse(&body, session.as_deref()).await?;
                let result = extract_jsonrpc(&parse_sse(&text)).map_err(TransportError::Other)?;
                Ok::<Value, TransportError>(result)
            };

            match tokio::time::timeout_at(deadline, round).await {
                Ok(Ok(result)) => return Ok(result),
                Ok(Err(TransportError::SessionGone(reason))) if !retried => {
                    retried = true;
                    warn!(
                        "MCP session not found on {} ({}): re-initializing and retrying once",
                        op, reason
                    );
                    // `post_sse` ya invalidó la sesión cacheada; el
                    // `initialize` siguiente abre una nueva y reenvía
                    // `notifications/initialized` para ella.
                    match tokio::time::timeout_at(deadline, self.initialize()).await {
                        Ok(Ok(())) => continue,
                        Ok(Err(e)) => return Err(e),
                        Err(_) => {
                            warn!("MCP re-initialize after session loss timed out ({})", op);
                            return Err(MikarenoError::Unexpected(format!(
                                "MCP sin tiempo para re-inicializar la sesión ({op})"
                            )));
                        }
                    }
                }
                Ok(Err(TransportError::SessionGone(reason))) => {
                    warn!(
                        "MCP session still not found on {} after re-initialize: {}",
                        op, reason
                    );
                    return Err(MikarenoError::Http(format!(
                        "MCP session not found (HTTP 404, {op}): {reason}"
                    )));
                }
                Ok(Err(TransportError::Other(e))) => return Err(e),
                Err(_) => {
                    warn!(
                        "MCP {} timed out after {}s",
                        op,
                        budget.as_secs()
                    );
                    self.mark_disconnected();
                    return Err(MikarenoError::Unexpected(format!(
                        "MCP sin respuesta a tiempo ({op})"
                    )));
                }
            }
        }
    }

    /// Send `tools/list` and decode the advertised tools.
    pub async fn list_tools(&self) -> Result<Vec<ToolInfo>, MikarenoError> {
        // Cache the tool list for the configured TTL.
        let cache_key = "tools:list".to_string();
        if let Some(cached) = self.cache_get_fresh(&cache_key) {
            return tools_from_value(&cached);
        }

        // Entrada expirada: se sirve la lista vieja al instante y se refresca
        // en background (stale-while-revalidate).
        if let Some(cached) = self.cache_get_stale(&cache_key) {
            let client = self.clone();
            let label = "tools/list (refresco)".to_string();
            let op_label = label.clone();
            let fut = async move { client.rpc_call("tools/list", None, &op_label).await };
            self.spawn_refresh(cache_key.clone(), label, fut);
            return tools_from_value(&cached);
        }

        let result = self.rpc_call("tools/list", None, "tools/list").await?;

        // Cache the raw result envelope so list_tools stays cheap on repeats.
        self.cache_set(&cache_key, result.clone());
        tools_from_value(&result)
    }

    /// Invoke a tool by name and return its decoded `text` payload as a `Value`.
    ///
    /// Cache *stale-while-revalidate*:
    ///   * hay entrada fresca → se devuelve al instante;
    ///   * hay entrada expirada → se devuelve **al instante** (dato viejo pero
    ///     honesto) y el refresco se lanza en background para la siguiente
    ///     petición; la página nunca espera al re-scrape (~60 s);
    ///   * no hay entrada alguna → fetch bloqueante con deadline corto
    ///     ([`FIRST_FETCH_DEADLINE_SECS`]); si falla, error honesto.
    pub async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, MikarenoError> {
        // Read-through cache keyed by tool name + args.
        let cache_key = format!(
            "call:{}:{}",
            name,
            serde_json::to_string(&args).unwrap_or_default()
        );

        // 1) Entrada fresca → respuesta inmediata.
        if let Some(cached) = self.cache_get_fresh(&cache_key) {
            return Ok(cached);
        }

        // 2) Entrada EXPIRADA → stale-while-revalidate: se devuelve ya y el
        //    refresco corre en background (no bloquea la página).
        if let Some(cached) = self.cache_get_stale(&cache_key) {
            self.spawn_call_refresh(&cache_key, name, args);
            return Ok(cached);
        }

        // 3) Cache fría → fetch bloqueante con deadline corto.
        self.fetch_call_blocking(&cache_key, name, args).await
    }

    /// Fetch en primer plano (cache fría): deadline corto y error honesto.
    async fn fetch_call_blocking(
        &self,
        cache_key: &str,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, MikarenoError> {
        // Breaker abierto → ni se intenta la red.
        if self.breaker_is_open() {
            return Err(Self::breaker_error(name));
        }

        let params = json!({ "name": name, "arguments": args.clone() });
        let op = format!("tools/call {name}");
        let call = async {
            let result = self
                .rpc_call_with_deadline(
                    "tools/call",
                    Some(params),
                    &op,
                    Duration::from_secs(CALL_DEADLINE_SECS),
                )
                .await?;
            extract_tool_result(&result)
        };

        match tokio::time::timeout(Duration::from_secs(FIRST_FETCH_DEADLINE_SECS), call).await {
            Ok(Ok(value)) => {
                self.breaker_on_success();
                self.cache_set(cache_key, value.clone());
                Ok(value)
            }
            Ok(Err(e)) => {
                self.breaker_on_failure();
                // El error es honesto, pero no se pierde la oportunidad de
                // calentar la caché: el refresco en background deja la
                // siguiente carga servida al instante.
                self.spawn_call_refresh(cache_key, name, args);
                Err(e)
            }
            Err(_) => {
                self.breaker_on_failure();
                self.spawn_call_refresh(cache_key, name, args);
                Err(MikarenoError::Unexpected(format!(
                    "MCP sin respuesta a tiempo (primer fetch de {op}, {}s)",
                    FIRST_FETCH_DEADLINE_SECS
                )))
            }
        }
    }

    /// Lanza el refresco en background de una entrada expirada.
    ///
    /// El refresco usa un deadline largo ([`REFRESH_DEADLINE_SECS`]) porque
    /// el servidor puede tardar ~60 s en re-scrapear.
    fn spawn_call_refresh(&self, cache_key: &str, name: &str, args: serde_json::Value) {
        let client = self.clone();
        let tool = name.to_string();
        let label = format!("tools/call {tool} (refresco)");
        let fut = async move {
            let op = format!("tools/call {tool} (refresco)");
            let params = json!({ "name": tool, "arguments": args });
            let result = client
                .rpc_call_with_deadline(
                    "tools/call",
                    Some(params),
                    &op,
                    Duration::from_secs(REFRESH_DEADLINE_SECS),
                )
                .await?;
            extract_tool_result(&result)
        };
        self.spawn_refresh(cache_key.to_string(), label, fut);
    }

    /// Lanza una tarea en background que actualiza la caché.
    ///
    /// Si ya hay un refresco en curso para la misma clave no se lanza otro
    /// (evita N refrescos paralelos por el mismo payload).
    fn spawn_refresh<Fut>(&self, cache_key: String, label: String, fut: Fut)
    where
        Fut: Future<Output = Result<Value, MikarenoError>> + Send + 'static,
    {
        if !self.begin_refresh(&cache_key) {
            debug!("MCP: ya hay un refresco en curso para {}", cache_key);
            return;
        }

        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => {
                self.end_refresh(&cache_key);
                warn!("MCP: sin runtime Tokio, no se lanza el refresco de {}", label);
                return;
            }
        };

        let client = self.clone();
        handle.spawn(async move {
            // El guard libera la marca de "refrescando" pase lo que pase.
            let _guard = RefreshGuard {
                client: client.clone(),
                key: cache_key.clone(),
            };
            match fut.await {
                Ok(value) => {
                    client.breaker_on_success();
                    client.cache_set(&cache_key, value);
                    debug!("MCP: refresco en background de {} completado", label);
                }
                Err(e) => {
                    client.breaker_on_failure();
                    warn!("MCP: refresco en background de {} falló: {}", label, e);
                }
            }
        });
    }

    // ── typed adapters ───────────────────────────────────────────────

    /// `get_calendario` → flat list of calendar events.
    pub async fn calendario(&self) -> Result<Vec<CalendarioEvent>, MikarenoError> {
        let raw = self.call_tool("get_calendario", json!({})).await?;
        adapters::parse_calendario(&raw).map_err(MikarenoError::SseParse)
    }

    /// `get_horario` → grouped weekly schedule.
    pub async fn horario(&self) -> Result<Horario, MikarenoError> {
        let raw = self.call_tool("get_horario", json!({})).await?;
        adapters::parse_horario(&raw).map_err(MikarenoError::SseParse)
    }

    /// `get_perfil` → { nombre, institucion, curso }.
    pub async fn perfil(&self) -> Result<Perfil, MikarenoError> {
        let raw = self.call_tool("get_perfil", json!({})).await?;
        adapters::parse_perfil(&raw).map_err(MikarenoError::SseParse)
    }

    /// `get_tareas` → flat list of homework tasks.
    ///
    /// Live-verified: `get_tareas` IS advertised by the real server
    /// (`tools/list`, Mikareno Student MCP v4.0.11, 2026-10-10) with input
    /// schema `{force_refresh?: boolean}`. `docs/ARCHITECTURE.md` §3 omits it
    /// from its 17-tool list — that doc list is stale (it also names 7
    /// `get_detalles_*/get_mis_grupos/get_detalle_calendario` tools the server
    /// does not advertise), not the client. Callers already degrade
    /// gracefully via `unwrap_or_default()` when the tool errors/absent.
    pub async fn tareas(&self) -> Result<Vec<Tarea>, MikarenoError> {
        let raw = self.call_tool("get_tareas", json!({})).await?;
        adapters::parse_tareas(&raw).map_err(MikarenoError::SseParse)
    }

    /// `get_notas_trimestre` → graded subjects for a term (1/2/3).
    pub async fn notas(&self, trimestre: u8) -> Result<Vec<Nota>, MikarenoError> {
        let raw = self
            .call_tool("get_notas_trimestre", json!({ "trimestre": trimestre }))
            .await?;
        adapters::parse_notas(&raw).map_err(MikarenoError::SseParse)
    }

    /// `get_student_full` → objeto completo (promedios, horario, calendario,
    /// tareas, mensajes…). Es un solo scrape que cubre todo el panel.
    pub async fn student_full(&self) -> Result<serde_json::Value, MikarenoError> {
        self.call_tool("get_student_full", json!({ "bg": true }))
            .await
    }

    /// Notas por materia del parcial en curso, tomadas de `get_student_full`
    /// (`promedios.stats`). Preferir sobre `notas()`, que en el servidor real
    /// devuelve `materias_y_notas: []`.
    pub async fn notas_full(&self) -> Result<Vec<Nota>, MikarenoError> {
        let full = self.student_full().await?;
        Ok(adapters::parse_promedios_full(&full))
    }

    /// `get_ano_escolar` → fechas, términos, parciales y configuración del
    /// período. Devuelve el JSON crudo (igual que `student_full`); para
    /// tiparlo, pasar por `adapters::parse_ano_escolar` (reexportado en
    /// `mod.rs`). El caché TTL de 180 s lo aplica `call_tool`.
    pub async fn ano_escolar(&self) -> Result<serde_json::Value, MikarenoError> {
        self.call_tool("get_ano_escolar", json!({})).await
    }

    /// `get_biblioteca` → recursos institucionales de la biblioteca.
    ///
    /// También devuelve el JSON crudo; para tiparlo,
    /// `adapters::parse_biblioteca` (reexportado en `mod.rs`).
    pub async fn biblioteca(&self) -> Result<serde_json::Value, MikarenoError> {
        self.call_tool("get_biblioteca", json!({})).await
    }

    /// `get_cumpleanos` → celebración de cumpleaños del estudiante.
    ///
    /// Devuelve el JSON crudo (igual que `ano_escolar`/`biblioteca`); para
    /// tiparlo, pasar por `adapters::parse_cumpleanos` (reexportado en
    /// `mod.rs`). El caché TTL de 180 s lo aplica `call_tool`.
    ///
    /// Nota: el servidor real (`tools/list` verificado contra
    /// `127.0.0.1:8080`) publica este tool con el esquema
    /// `{force_refresh?: bool}` y `additionalProperties: false` — mandar
    /// `bg` (como sí acepta `get_student_full`) devuelve error de
    /// validación *unexpected keyword argument*. Por eso la llamada va sin
    /// argumentos extra.
    pub async fn cumpleanos(&self) -> Result<serde_json::Value, MikarenoError> {
        self.call_tool("get_cumpleanos", json!({})).await
    }

    // ── introspection (used by tests) ─────────────────────────────────

    /// Current session id, if any. Exposed for diagnostics/tests only.
    pub fn session_id(&self) -> Option<String> {
        self.state
            .session_id
            .lock()
            .expect("session lock poisoned")
            .clone()
    }

    /// Siembra una entrada de caché directamente (sólo tests).
    ///
    /// Reproduce exactamente la clave que usa [`MikarenoClient::call_tool`]
    /// para poder probar la ruta "stale" sin tocar la red.
    #[cfg(test)]
    fn seed_cache_for_test(
        &self,
        name: &str,
        args: serde_json::Value,
        value: serde_json::Value,
    ) {
        let key = format!(
            "call:{}:{}",
            name,
            serde_json::to_string(&args).unwrap_or_default()
        );
        self.cache_set(&key, value);
    }
}

/// Libera la marca de "refrescando" de una clave cuando la tarea de
/// background termina (o se cae).
struct RefreshGuard {
    client: MikarenoClient,
    key: String,
}

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        self.client.end_refresh(&self.key);
    }
}

/// Decode a `tools/call`/`tools/list` result into the inner payload.
///
/// The server wraps tool output as `{"content":[{"type":"text","text":"..."}]}`
/// where `text` is a JSON-encoded string. We parse it back into a `Value`; if
/// the text is not valid JSON we fall back to returning it as a string.
fn extract_tool_result(result: &Value) -> Result<Value, MikarenoError> {
    if result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let msg = result
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("tool reported an error");
        return Err(MikarenoError::Unexpected(format!(
            "tool returned isError: {}",
            msg
        )));
    }

    if let Some(content) = result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
    {
        if content.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(text) = content.get("text").and_then(|t| t.as_str()) {
                if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                    return Ok(parsed);
                }
                return Ok(Value::String(text.to_string()));
            }
        }
    }
    Err(MikarenoError::Unexpected(
        "no text content in tool result".into(),
    ))
}

fn tools_from_value(result: &Value) -> Result<Vec<ToolInfo>, MikarenoError> {
    let arr = result
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| MikarenoError::Unexpected("tools/list: missing 'tools' array".into()))?;
    let mut out = Vec::with_capacity(arr.len());
    for t in arr {
        let name = t
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = t
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(ToolInfo { name, description });
    }
    Ok(out)
}

/// Split an SSE body into its JSON-RPC `data:` messages.
///
/// Handles multi-line `data:` concatenation per the SSE spec, the
/// `data: ` (with space) and `data:` (no space) prefixes, and falls back to
/// treating the whole body as a bare JSON document when no `data:` lines exist
/// (stateless servers).
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
        // Lines we don't care about (`:` comments, `retry:`, `event:`) are ignored.
    }
    // Trailing flush for bodies that don't end in a blank line.
    flush_data(&data, have_data, &mut messages);

    if messages.is_empty() {
        // Not SSE — try the whole body as a single JSON-RPC document.
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

/// Pick the first JSON-RPC message carrying a `result` (or surface an `error`).
fn extract_jsonrpc(messages: &[Value]) -> Result<Value, MikarenoError> {
    for msg in messages {
        if let Some(err) = msg.get("error") {
            let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let message = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
                .to_string();
            return Err(MikarenoError::JsonRpc { code, message });
        }
        if let Some(result) = msg.get("result") {
            return Ok(result.clone());
        }
    }
    Err(MikarenoError::SseParse(
        "no jsonrpc result or error in response".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sse_single_data_line_parses() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        let msgs = parse_sse(body);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["result"]["ok"], json!(true));
    }

    #[test]
    fn sse_no_space_prefix_parses() {
        let body = "data:{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":42}\n";
        let msgs = parse_sse(body);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["result"], json!(42));
    }

    #[test]
    fn sse_bare_json_fallback_parses() {
        let body = "{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"tools\":[]}}";
        let msgs = parse_sse(body);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0]["result"]["tools"].is_array());
    }

    #[test]
    fn sse_multiline_data_concatenates() {
        let body =
            "data: {\"jsonrpc\":\"2.0\"".to_string() + "\ndata: ,\"id\":1,\"result\":true}\n\n";
        let msgs = parse_sse(&body);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["result"], json!(true));
    }

    #[test]
    fn extract_jsonrpc_picks_result_and_errors() {
        let ok = parse_sse("data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"a\":1}}\n\n");
        assert_eq!(extract_jsonrpc(&ok).unwrap()["a"], json!(1));

        let bad = parse_sse("data: {\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32601,\"message\":\"not found\"}}\n\n");
        let err = extract_jsonrpc(&bad).unwrap_err();
        match err {
            MikarenoError::JsonRpc { code, message } => {
                assert_eq!(code, -32601);
                assert_eq!(message, "not found");
            }
            other => panic!("wrong error variant: {:?}", other),
        }
    }

    #[test]
    fn extract_tool_result_unwraps_text_payload() {
        // tools/call result envelope with a double-encoded text payload
        let result = json!({
            "content": [{"type":"text","text": "{\"trimestre\":1,\"materias_y_notas\":[]}"}]
        });
        let inner = extract_tool_result(&result).unwrap();
        assert_eq!(inner["trimestre"], json!(1));
        assert!(inner["materias_y_notas"].is_array());
    }

    #[test]
    fn extract_tool_result_flags_is_error() {
        let result = json!({"isError": true, "content":[{"type":"text","text":"boom"}]});
        assert!(extract_tool_result(&result).is_err());
    }

    #[test]
    fn extract_tool_result_non_json_text_falls_back_to_string() {
        let result = json!({"content":[{"type":"text","text":"plain text"}]});
        let inner = extract_tool_result(&result).unwrap();
        assert_eq!(inner, json!("plain text"));
    }

    #[test]
    fn extract_session_id_reads_the_header_case_insensitively() {
        // El servidor FastMCP lo emite como `Mcp-Session-Id`; el cliente lo
        // busca como `mcp-session-id` (los nombres de cabecera HTTP son
        // case-insensitive). Función pura: no toca la red.
        let mut headers = HeaderMap::new();
        headers.insert("mcp-session-id", "abc123".parse().unwrap());
        assert_eq!(extract_session_id(&headers), Some("abc123".to_string()));

        // Sin cabecera → sin sesión (servidor stateless).
        assert_eq!(extract_session_id(&HeaderMap::new()), None);

        // Valor vacío o en blanco → se descarta (no es un id usable).
        let mut blank = HeaderMap::new();
        blank.insert("mcp-session-id", "   ".parse().unwrap());
        assert_eq!(extract_session_id(&blank), None);
    }

    #[test]
    fn is_session_gone_detects_only_the_404_session_case() {
        // Cuerpo real del servidor MCP 2.3 (`_error_response("Session not
        // found", 404)`).
        let body = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Session not found"}}"#;
        assert!(is_session_gone(404, body));

        // Variante del transporte: "Not Found: Invalid or expired session ID".
        assert!(is_session_gone(
            404,
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Not Found: Invalid or expired session ID"}}"#
        ));

        // Variante de sesión terminada (DELETE previo).
        assert!(is_session_gone(
            404,
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Not Found: Session has been terminated"}}"#
        ));

        // Un 404 que no habla de sesión NO invalida la sesión cacheada.
        assert!(!is_session_gone(404, r#"{"error":"unknown method"}"#));

        // Un 200 nunca es "session gone", aunque el cuerpo lo mencione.
        assert!(!is_session_gone(200, body));
    }

    #[test]
    fn client_builds_with_custom_ttl() {
        // Smoke test: construction must not panic and the default client must
        // start disconnected (no initialize has run yet).
        let c = MikarenoClient::with_cache_ttl("http://invalid.invalid/mcp".into(), 1);
        assert!(!c.is_connected());
        assert!(c.session_id().is_none());
    }

    #[test]
    fn from_config_accepts_loopback_and_tailnet_addresses() {
        for url in [
            "http://127.0.0.1:8080/mcp",
            "http://localhost:8080/mcp",
            "http://[::1]:8080/mcp",
            // Tailnet de Tailscale (CGNAT 100.64/10) — la URL de producción.
            "http://100.115.62.97:8080/mcp",
            // RFC 1918.
            "http://192.168.1.10:8080/mcp",
            "http://10.0.0.5:8080/mcp",
            "http://172.16.4.4:8080/mcp",
        ] {
            assert!(
                MikarenoClient::from_config(url).is_some(),
                "debería aceptar {url}"
            );
        }
    }

    #[test]
    fn from_config_rejects_public_and_malformed_urls() {
        for url in [
            "",
            "ftp://127.0.0.1:8080/mcp",
            "mcp.invalid:8080",
            // Host público: fuera del alcance acordado.
            "http://mcp.invalid:8080/mcp",
            "http://example.com/mcp",
            // TEST-NET-3 (documentación, no es privado).
            "http://192.0.2.10:8080/mcp",
        ] {
            assert!(
                MikarenoClient::from_config(url).is_none(),
                "debería rechazar {url}"
            );
        }
    }

    /// Stale-while-revalidate: con la entrada expirada, `call_tool` devuelve
    /// el dato viejo **sin esperar a la red**.
    ///
    /// "Reloj falso": `with_cache_ttl(.., 0)` deja la entrada expirada al
    /// instante, sin tener que esperar al TTL real. El servidor de mentira
    /// acepta la conexión pero nunca responde: si la ruta "stale" bloqueara,
    /// el `timeout` de abajo se dispararía y el test fallaría.
    #[tokio::test]
    async fn call_tool_serves_stale_entry_without_waiting_for_the_network() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // Acepta pero nunca responde: cualquier espera es detectable.
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });

        let client = MikarenoClient::with_cache_ttl(format!("http://{addr}/mcp"), 0);
        client.seed_cache_for_test("get_horario", json!({}), json!({ "dia": "lunes" }));

        let res = tokio::time::timeout(
            Duration::from_millis(500),
            client.call_tool("get_horario", json!({})),
        )
        .await;

        let value = res
            .expect("la ruta stale no debe esperar a la red")
            .expect("debe devolver el valor rancio cacheado");
        assert_eq!(value["dia"], json!("lunes"));
    }

    /// Circuit breaker: 3 fallos consecutivos → las llamadas fallan rápido
    /// (sin tocar la red) durante 30 s; un éxito lo rearma.
    #[tokio::test]
    async fn circuit_breaker_opens_after_three_failures_and_fails_fast() {
        // Puerto de descarte: si se intentara la red, el error sería de
        // conexión, no del breaker.
        let client = MikarenoClient::with_cache_ttl("http://127.0.0.1:9/mcp".into(), 60);

        client.breaker_on_failure();
        client.breaker_on_failure();
        assert!(!client.breaker_is_open(), "con 2 fallos aún no abre");
        client.breaker_on_failure();
        assert!(client.breaker_is_open(), "con 3 fallos debe abrir");

        let err = client
            .call_tool("get_horario", json!({}))
            .await
            .expect_err("con el breaker abierto la llamada debe fallar");
        assert!(
            err.to_string().contains("circuit breaker"),
            "mensaje inesperado: {err}"
        );

        // Un éxito rearma el breaker.
        client.breaker_on_success();
        assert!(!client.breaker_is_open());
    }
}
