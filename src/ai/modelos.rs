//! Selector de modelo de IA (OpenRouter).
//!
//! ## Qué resuelve
//!
//! El modelo de IA estaba fijo en `OPENROUTER_MODEL` (`.env`), inmutable en
//! runtime: para cambiar de modelo había que editar el `.env` y reiniciar el
//! servicio. El usuario pidió poder **elegir cualquier modelo free de
//! OpenRouter** desde la interfaz.
//!
//! ## Cómo funciona
//!
//! * `GET /api/ai/models` → lista de modelos **free** que ofrece OpenRouter
//!   ahora mismo (`GET https://openrouter.ai/api/v1/models`, filtrando los
//!   que acaban en `:free`). La lista se cachea 30 min en memoria: no se
//!   pega al proveedor en cada visita.
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
//! * Sólo se aceptan identificadores que estén en la lista real de modelos
//!   free de OpenRouter: no se puede pedir un modelo de pago por esta vía.
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

/// Cuánto se cachea el catálogo de modelos (30 min).
///
/// OpenRouter añade y retira modelos con frecuencia, pero 30 min es
/// suficiente para no castigar al proveedor ni congelar la lista.
const TTL_CATALOGO: Duration = Duration::from_secs(1800);

/// Catálogo de respaldo: modelos free conocidos.
///
/// Se usa cuando no hay clave de API o el proveedor no responde, para que la
/// lista nunca esté vacía y el usuario pueda elegir.
const MODELOS_RESPALDO: &[(&str, &str)] = &[
    ("minimax/minimax-m3.1-flash:free", "MiniMax M3.1 Flash"),
    ("poolside/laguna-s-2.1:free", "Laguna S 2.1"),
    ("poolside/laguna-xs-2.1:free", "Laguna XS 2.1"),
    ("cohere/north-mini-code:free", "North Mini Code"),
    ("thinkingmachines/inkling:free", "Inkling"),
    ("thinkingmachines/inkling-small:free", "Inkling Small"),
    ("liquid/lfm-2.5-2.6b:free", "LFM 2.5 2.6B"),
    ("nvidia/nemotron-3.5-lightning:free", "Nemotron 3.5 Lightning"),
];

/// Un modelo del catálogo.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModeloInfo {
    pub id: String,
    pub name: String,
    /// Precio por millón de tokens (entrada/salida). Los free son 0.
    pub free: bool,
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
}

/// Descarga el catálogo de modelos free de OpenRouter.
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
        if !id.ends_with(":free") {
            continue;
        }
        let name = m
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(id)
            .to_string();
        let free = m
            .get("pricing")
            .and_then(|p| p.get("prompt"))
            .and_then(|v| v.as_str())
            .map(|s| s == "0" || s.is_empty())
            .unwrap_or(true);
        out.push(ModeloInfo {
            id: id.to_string(),
            name,
            free,
        });
    }
    if out.is_empty() {
        None
    } else {
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Some(out)
    }
}

/// Catálogo de modelos free (con caché).
pub async fn catalogo(state: &Arc<AppState>) -> Vec<ModeloInfo> {
    if let Some(c) = state.cache_catalogo.get() {
        return c;
    }
    let key = state.config.openrouter_api_key.as_deref();
    let descargado = descargar_catalogo(key).await;
    let modelos = descargado.unwrap_or_else(|| {
        MODELOS_RESPALDO
            .iter()
            .map(|(id, name)| ModeloInfo {
                id: id.to_string(),
                name: name.to_string(),
                free: true,
            })
            .collect()
    });
    state.cache_catalogo.set(modelos.clone());
    modelos
}

/// Modelo activo: el de `app_config` si el usuario eligió uno; si no, el del
/// `.env` (`OPENROUTER_MODEL`).
pub async fn modelo_activo(state: &Arc<AppState>) -> String {
    if let Ok(Some(v)) = sqlx::query_scalar::<_, String>("SELECT value FROM app_config WHERE key = ?")
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
    Json(json!({
        "count": modelos.len(),
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
/// Sólo se aceptan modelos que estén en el catálogo real de free: es la
/// garantía de que esta vía no abre la puerta a un modelo de pago.
pub async fn set_model(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SetModel>,
) -> Response {
    let elegido = payload.model.trim().to_string();
    if elegido.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Indica un modelo" })),
        )
            .into_response();
    }

    let modelos = catalogo(&state).await;
    if !modelos.iter().any(|m| m.id == elegido) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Ese modelo no está en la lista de modelos free de OpenRouter",
                "model": elegido,
            })),
        )
            .into_response();
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
