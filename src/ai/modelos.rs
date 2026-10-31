//! Selector de modelo de IA (OpenRouter).
//!
//! ## Qué resuelve
//!
//! El modelo de IA estaba fijo en `OPENROUTER_MODEL` (`.env`), inmutable en
//! runtime: para cambiar de modelo había que editar el `.env` y reiniciar el
//! servicio. El usuario pidió poder **elegir cualquier modelo de
//! OpenRouter** desde la interfaz.
//!
//! ## Cómo funciona
//!
//! * `GET /api/ai/models` → lista de **todos** los modelos que ofrece
//!   OpenRouter ahora mismo (`GET https://openrouter.ai/api/v1/models`).
//!   Los modelos free se muestran primero, seguidos de los de pago.
//!   La lista se cachea 5 min en memoria.
//! * `GET /api/ai/model` → el modelo activo.
//! * `PUT /api/ai/model` (JSON `{"model": "id"}`) → guarda el modelo elegido
//!   en la tabla `app_config` (clave `ai.model`) y lo aplica en runtime.
//!
//! El modelo guardado en `app_config` **tiene prioridad** sobre el
//! `OPENROUTER_MODEL` del `.env`: así el cambio surte efecto sin reiniciar.
//! Si la fila no existe, se usa el del `.env` (comportamiento anterior).
//!
//! ## Seguridad
//!
//! * Se acepta cualquier identificador que esté en el catálogo real de
//!   OpenRouter. El campo `free` indica si el modelo es gratuito o de pago.
//! * Sin clave de API el catálogo se sirve de una lista embebida de respaldo,
//!   para que la lista nunca esté vacía.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::AppState;

/// Clave en `app_config` donde se guarda el modelo elegido.
pub const CLAVE_MODELO: &str = "ai.model";

/// Cuánto se cachea el catálogo de modelos (5 min; `POST
/// /admin/openrouter/refresh-catalog` lo fuerza antes si hace falta).
///
/// OpenRouter añade y retira modelos con frecuencia: 5 min es corto para no
/// congelar la lista del selector y sigue castigando poco al proveedor.
const TTL_CATALOGO: Duration = Duration::from_secs(300);

/// Catálogo de respaldo: modelos free conocidos.
///
/// Se usa cuando no hay clave de API o el proveedor no responde, para que la
/// lista nunca esté vacía y el usuario pueda elegir.
/// Catálogo de respaldo: modelos conocidos de OpenRouter.
///
/// Se usa cuando no hay clave de API o el proveedor no responde, para que la
/// lista nunca esté vacía y el usuario pueda elegir. Incluye modelos free y de
/// pago; el campo `free` se infiere del sufijo `:free` en el ID.
const MODELOS_RESPALDO: &[(&str, &str, bool)] = &[
    // ── Modelos free ──
    (
        "nvidia/nemotron-3-ultra-550b-a55b:free",
        "Nemotron 3 Ultra 550B",
        true,
    ),
    ("poolside/laguna-s-2.1:free", "Laguna S 2.1", true),
    ("poolside/laguna-xs-2.1:free", "Laguna XS 2.1", true),
    ("cohere/north-mini-code:free", "North Mini Code", true),
    ("thinkingmachines/inkling:free", "Inkling", true),
    ("thinkingmachines/inkling-small:free", "Inkling Small", true),
    ("liquid/lfm-2.5-2.6b:free", "LFM 2.5 2.6B", true),
    ("google/gemma-4-31b-it:free", "Gemma 4 31B", true),
    (
        "nvidia/nemotron-3.5-super-vl:free",
        "Nemotron 3.5 Super VL",
        true,
    ),
    ("nvidia/nemotron-nano-8b-v2:free", "Nemotron Nano 8B", true),
    ("nvidia/nemotron-h-24b:free", "Nemotron H 24B", true),
    (
        "meta-llama/llama-3.3-70b-instruct:free",
        "Llama 3.3 70B",
        true,
    ),
    (
        "meta-llama/llama-3.2-3b-instruct:free",
        "Llama 3.2 3B",
        true,
    ),
    (
        "meta-llama/llama-3.2-11b-vision-instruct:free",
        "Llama 3.2 11B Vision",
        true,
    ),
    (
        "deepseek/deepseek-chat-v3-0324:free",
        "DeepSeek Chat V3",
        true,
    ),
    ("deepseek/deepseek-r1-0528:free", "DeepSeek R1", true),
    ("qwen/qwen3-235b-a22b:free", "Qwen3 235B", true),
    ("qwen/qwen3-30b-a3b:free", "Qwen3 30B", true),
    ("qwen/qwen2.5-72b-instruct:free", "Qwen2.5 72B", true),
    ("google/gemini-2.0-flash-001:free", "Gemini 2.0 Flash", true),
    (
        "google/gemini-2.5-flash-preview:free",
        "Gemini 2.5 Flash",
        true,
    ),
    (
        "mistralai/mistral-small-3.1-24b-instruct:free",
        "Mistral Small 3.1",
        true,
    ),
    ("mistralai/mistral-nemo:free", "Mistral Nemo", true),
    (
        "microsoft/phi-4-reasoning-plus:free",
        "Phi-4 Reasoning Plus",
        true,
    ),
    (
        "microsoft/phi-4-mini-reasoning:free",
        "Phi-4 Mini Reasoning",
        true,
    ),
    // ── Modelos de pago (populares) ──
    ("anthropic/claude-opus-4.5", "Claude Opus 4.5", false),
    ("anthropic/claude-sonnet-4.5", "Claude Sonnet 4.5", false),
    ("anthropic/claude-haiku-4.5", "Claude Haiku 4.5", false),
    ("openai/gpt-5.6-sol", "GPT-5.6 Sol", false),
    ("openai/gpt-5.6-terra", "GPT-5.6 Terra", false),
    ("openai/gpt-5.6-luna", "GPT-5.6 Luna", false),
    ("google/gemini-2.5-pro", "Gemini 2.5 Pro", false),
    ("google/gemini-2.5-flash", "Gemini 2.5 Flash", false),
    ("deepseek/deepseek-chat-v3.1", "DeepSeek Chat V3.1", false),
    ("x-ai/grok-4", "Grok 4", false),
    ("meta-llama/llama-4-maverick", "Llama 4 Maverick", false),
    ("meta-llama/llama-4-scout", "Llama 4 Scout", false),
];

/// Un modelo del catálogo de OpenRouter.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModeloInfo {
    pub id: String,
    pub name: String,
    /// True si el modelo es gratuito (sufijo `:free` o precio 0).
    pub free: bool,
    /// Precio por millón de tokens (entrada), si lo reporta OpenRouter.
    pub pricing_prompt: Option<String>,
    /// Precio por millón de tokens (salida), si lo reporta OpenRouter.
    pub pricing_completion: Option<String>,
}

/// Caché del catálogo de modelos.
#[derive(Default)]
pub struct CacheCatalogo {
    inner: Mutex<Option<(Instant, Vec<ModeloInfo>)>>,
}

impl CacheCatalogo {
    /// Copia cacheada si no ha expirado.
    pub fn get(&self) -> Option<Vec<ModeloInfo>> {
        let g = self.inner.lock().ok()?;
        let (t, modelos) = g.as_ref()?;
        if t.elapsed() < TTL_CATALOGO {
            Some(modelos.clone())
        } else {
            None
        }
    }

    /// Guarda el catálogo.
    pub fn set(&self, modelos: Vec<ModeloInfo>) {
        if let Ok(mut g) = self.inner.lock() {
            *g = Some((Instant::now(), modelos));
        }
    }

    /// Invalida el catálogo: fuerza a un refresh en la próxima llamada.
    pub fn invalidate(&self) {
        if let Ok(mut g) = self.inner.lock() {
            *g = None;
        }
    }

    /// Fuerza un refresh del catálogo ahora.
    /// Retorna el nuevo catálogo si la descarga tiene éxito, None si falla.
    pub async fn refresh(&self, api_key: Option<&str>) -> Option<Vec<ModeloInfo>> {
        // Invalida el cache primero para forzar un refresh
        self.invalidate();
        // Descarga un nuevo catálogo
        match descargar_catalogo(api_key).await {
            Some(modelos) => {
                self.set(modelos.clone());
                Some(modelos)
            }
            None => None,
        }
    }
}

/// Descarga el catálogo completo de OpenRouter (free y de pago).
///
/// Devuelve `None` si el proveedor no responde (se usará el respaldo).
async fn descargar_catalogo(api_key: Option<&str>) -> Option<Vec<ModeloInfo>> {
    let mut req = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .ok()?;
    let mut rb = req.get("https://openrouter.ai/api/v1/models");
    if let Some(k) = api_key {
        rb = rb.header("Authorization", format!("Bearer {k}"));
    }
    let resp = rb.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let raw: Value = resp.json().await.ok()?;
    let data = raw.get("data")?.as_array()?;
    let mut out: Vec<ModeloInfo> = Vec::new();
    for m in data {
        let Some(id) = m.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let name = m
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(id)
            .to_string();

        // Un modelo es free si su ID termina en ":free" o su precio es 0.
        let is_free = id.ends_with(":free")
            || m.get("pricing")
                .and_then(|p| p.get("prompt"))
                .and_then(|v| v.as_str())
                .map(|s| s == "0" || s.is_empty())
                .unwrap_or(false);

        // Extraer precios si los reporta OpenRouter.
        let pricing_prompt = m
            .get("pricing")
            .and_then(|p| p.get("prompt"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let pricing_completion = m
            .get("pricing")
            .and_then(|p| p.get("completion"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Mostrar TODOS los modelos (free y de pago).
        out.push(ModeloInfo {
            id: id.to_string(),
            name,
            free: is_free,
            pricing_prompt,
            pricing_completion,
        });
    }

    // Ordenar: modelos free primero, luego de pago; dentro de cada grupo por nombre.
    out.sort_by(|a, b| {
        let a_free = a.free;
        let b_free = b.free;
        match b_free.cmp(&a_free) {
            std::cmp::Ordering::Equal => a.name.cmp(&b.name),
            other => other,
        }
    });
    if out.is_empty() {
        None
    } else {
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Some(out)
    }
}

/// Catálogo de modelos de OpenRouter (free y de pago, con caché).
pub async fn catalogo(state: &Arc<AppState>) -> Vec<ModeloInfo> {
    if let Some(c) = state.cache_catalogo.get() {
        return c;
    }
    let key = state.config.openrouter_api_key.as_deref();
    let descargado = descargar_catalogo(key).await;
    let modelos = descargado.unwrap_or_else(|| {
        MODELOS_RESPALDO
            .iter()
            .map(|(id, name, free)| ModeloInfo {
                id: id.to_string(),
                name: name.to_string(),
                free: *free,
                pricing_prompt: None,
                pricing_completion: None,
            })
            .collect()
    });
    state.cache_catalogo.set(modelos.clone());
    modelos
}

/// Modelo activo: el de `app_config` si el usuario eligió uno; si no, el del
/// `.env` (`OPENROUTER_MODEL`).
pub async fn modelo_activo(state: &Arc<AppState>) -> String {
    if let Ok(Some(v)) =
        sqlx::query_scalar::<_, String>("SELECT value FROM app_config WHERE key = ?")
            .bind(CLAVE_MODELO)
            .fetch_optional(&state.db)
            .await
    {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return v;
        }
    }
    state.config.openrouter_model.clone()
}

/// `GET /api/ai/models` — catálogo de modelos free.
pub async fn list_models(State(state): State<Arc<AppState>>) -> Response {
    let modelos = catalogo(&state).await;
    let activo = modelo_activo(&state).await;
    let total = modelos.len();
    let free_count = modelos.iter().filter(|m| m.free).count();
    let paid_count = total - free_count;
    Json(json!({
        "count": total,
        "free_count": free_count,
        "paid_count": paid_count,
        "total_from_openrouter": total,
        "active": activo,
        "models": modelos,
    }))
    .into_response()
}

/// `GET /api/ai/model` — modelo activo.
pub async fn get_model(State(state): State<Arc<AppState>>) -> Response {
    Json(json!({ "model": modelo_activo(&state).await })).into_response()
}

/// Cuerpo de `PUT /api/ai/model`.
#[derive(Debug, Deserialize)]
pub struct SetModel {
    model: String,
}

/// `PUT /api/ai/model` — guarda el modelo elegido.
///
/// Se acepta cualquier identificador que esté en el catálogo de
/// OpenRouter (descargado en tiempo real o lista de respaldo): modelos
/// free y de pago. El campo `free` del catálogo indica si el modelo es
/// gratuito; elegir uno de pago puede generar costos en OpenRouter.
pub async fn set_model(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SetModel>,
) -> Response {
    let elegido = payload.model.trim().to_string();

    // Validar que el modelo esté en el catálogo de OpenRouter.
    let modelos = catalogo(&state).await;
    let modelo_info = modelos.iter().find(|m| m.id == elegido);
    if modelo_info.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Modelo no disponible en el catálogo de OpenRouter",
                "model": elegido,
            })),
        )
            .into_response();
    }

    // Advertir si el modelo es de pago (se guarda igualmente; el usuario decide).
    if let Some(m) = modelo_info {
        if !m.free {
            tracing::warn!("modelo de IA: se seleccionó un modelo de pago: {}", elegido);
        }
    }

    let res = sqlx::query(
        "INSERT INTO app_config (key, value, updated_at)
              VALUES (?, ?, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                        updated_at = excluded.updated_at",
    )
    .bind(CLAVE_MODELO)
    .bind(&elegido)
    .execute(&state.db)
    .await;

    match res {
        Ok(_) => Json(json!({ "ok": true, "model": elegido })).into_response(),
        Err(e) => {
            tracing::warn!("modelo de IA: no se pudo guardar: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "No se pudo guardar el modelo" })),
            )
                .into_response()
        }
    }
}
