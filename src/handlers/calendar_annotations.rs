//! Anotaciones del calendario — API local del usuario.
//!
//! Mikareno es read-only: el calendario trae días de clase, vacaciones y
//! tareas, pero **no** permite escribir nada. Este módulo da esa parte que
//! faltaba: anotar un día concreto ("examen de matemáticas", "entregar el
//! trabajo de la U"). Son datos **locales**, en la tabla
//! `calendar_annotations` (migración `20240101000007_calendar_annotations.sql`).
//!
//! Contrato:
//! - `GET  /api/calendar/annotations?from=YYYY-MM-DD&to=YYYY-MM-DD` →
//!   `{ "annotations": [ { "id", "date", "title", "body", "created_at", "updated_at" } ], "count": n }`.
//!   Sin `from`/`to` devuelve las anotaciones del mes en curso.
//! - `POST /api/calendar/annotations` (JSON `{date, title, body}`) → 201 con la anotación creada.
//! - `PUT  /api/calendar/annotations/{id}` (JSON `{title?, body?}`) → 200 con la anotación actualizada.
//! - `DELETE /api/calendar/annotations/{id}` → 200 `{ "deleted": true }`.
//!
//! Validación: la fecha debe ser `YYYY-MM-DD` real; el título se recorta a 120
//! caracteres y el cuerpo a 4000. Un body o una fecha inválidos devuelven 400
//! con un mensaje en español, nunca un 500.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Datelike;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use crate::AppState;

/// Longitudes máximas (el calendario es una vista, no un gestor de texto).
const MAX_TITLE: usize = 120;
const MAX_BODY: usize = 4000;

/// Filtro opcional por rango de fechas.
#[derive(Debug, Deserialize)]
pub struct RangeQuery {
    from: Option<String>,
    to: Option<String>,
}

/// Cuerpo de creación.
#[derive(Debug, Deserialize)]
pub struct NewAnnotation {
    date: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
}

/// Cuerpo de edición (parcial).
#[derive(Debug, Deserialize)]
pub struct EditAnnotation {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

/// Valida una fecha `YYYY-MM-DD` de calendario (día natural, sin huso).
fn fecha_valida(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").is_ok()
}

/// Normaliza el texto de entrada: recorta y limita la longitud.
fn limpia(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

/// Fila de anotación tal como se devuelve al cliente.
fn fila_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<i64, _>("id"),
        "date": row.get::<String, _>("date"),
        "title": row.get::<String, _>("title"),
        "body": row.get::<String, _>("body"),
        "created_at": row.get::<String, _>("created_at"),
        "updated_at": row.get::<String, _>("updated_at"),
    })
}

/// `GET /api/calendar/annotations` — anotaciones de un rango (mes por defecto).
pub async fn list(
    State(state): State<std::sync::Arc<AppState>>,
    Query(q): Query<RangeQuery>,
) -> Response {
    let hoy = chrono::Local::now().date_naive();
    let (desde, hasta) = match (q.from.as_deref(), q.to.as_deref()) {
        (Some(f), Some(t)) if fecha_valida(f) && fecha_valida(t) => {
            (f.trim().to_string(), t.trim().to_string())
        }
        (Some(f), None) if fecha_valida(f) => {
            let f = chrono::NaiveDate::parse_from_str(f.trim(), "%Y-%m-%d").unwrap_or(hoy);
            let fin = f + chrono::Duration::days(31);
            (
                f.format("%Y-%m-%d").to_string(),
                fin.format("%Y-%m-%d").to_string(),
            )
        }
        _ => {
            // Mes en curso: del 1º al último día.
            let primero = hoy.with_day(1).unwrap_or(hoy);
            let ultimo = primero
                .checked_add_signed(chrono::Duration::days(31))
                .map(|d| d.with_day(1).unwrap_or(d))
                .and_then(|d| d.pred_opt())
                .unwrap_or(hoy);
            (
                primero.format("%Y-%m-%d").to_string(),
                ultimo.format("%Y-%m-%d").to_string(),
            )
        }
    };

    let rows = sqlx::query(
        "SELECT id, date, title, body, created_at, updated_at
           FROM calendar_annotations
          WHERE date >= ? AND date <= ?
          ORDER BY date ASC, id ASC",
    )
    .bind(&desde)
    .bind(&hasta)
    .fetch_all(&state.db)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("anotaciones: no se pudo leer la tabla: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "No se pudieron leer las anotaciones" })),
            )
                .into_response();
        }
    };

    let annotations: Vec<Value> = rows.iter().map(fila_json).collect();
    Json(json!({
        "from": desde,
        "to": hasta,
        "count": annotations.len(),
        "annotations": annotations,
    }))
    .into_response()
}

/// `POST /api/calendar/annotations` — crea una anotación.
pub async fn create(
    State(state): State<std::sync::Arc<AppState>>,
    Json(payload): Json<NewAnnotation>,
) -> Response {
    if !fecha_valida(&payload.date) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "La fecha debe tener formato YYYY-MM-DD" })),
        )
            .into_response();
    }
    let date = payload.date.trim().to_string();
    let title = limpia(&payload.title, MAX_TITLE);
    let body = limpia(&payload.body, MAX_BODY);
    if title.is_empty() && body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "La anotación está vacía" })),
        )
            .into_response();
    }

    let res = sqlx::query("INSERT INTO calendar_annotations (date, title, body) VALUES (?, ?, ?)")
        .bind(&date)
        .bind(&title)
        .bind(&body)
        .execute(&state.db)
        .await;

    match res {
        Ok(r) => {
            let id = r.last_insert_rowid();
            let row = sqlx::query(
                "SELECT id, date, title, body, created_at, updated_at
                   FROM calendar_annotations WHERE id = ?",
            )
            .bind(id)
            .fetch_one(&state.db)
            .await;
            match row {
                Ok(row) => (
                    StatusCode::CREATED,
                    Json(json!({ "annotation": fila_json(&row) })),
                )
                    .into_response(),
                Err(_) => (
                    StatusCode::CREATED,
                    Json(json!({
                        "annotation": { "id": id, "date": date, "title": title, "body": body }
                    })),
                )
                    .into_response(),
            }
        }
        Err(e) => {
            tracing::warn!("anotaciones: no se pudo crear: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "No se pudo guardar la anotación" })),
            )
                .into_response()
        }
    }
}

/// `PUT /api/calendar/annotations/{id}` — actualiza título y/o cuerpo.
pub async fn update(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
    Json(payload): Json<EditAnnotation>,
) -> Response {
    let existente = sqlx::query("SELECT id FROM calendar_annotations WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await;
    match existente {
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "La anotación no existe" })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::warn!("anotaciones: no se pudo leer {id}: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "No se pudo leer la anotación" })),
            )
                .into_response();
        }
        Ok(Some(_)) => {}
    }

    let title = payload.title.as_deref().map(|t| limpia(t, MAX_TITLE));
    let body = payload.body.as_deref().map(|b| limpia(b, MAX_BODY));

    let res = sqlx::query(
        "UPDATE calendar_annotations
            SET title = COALESCE(?, title),
                body  = COALESCE(?, body),
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE id = ?",
    )
    .bind(title)
    .bind(body)
    .bind(id)
    .execute(&state.db)
    .await;

    match res {
        Ok(_) => {
            let row = sqlx::query(
                "SELECT id, date, title, body, created_at, updated_at
                   FROM calendar_annotations WHERE id = ?",
            )
            .bind(id)
            .fetch_one(&state.db)
            .await;
            match row {
                Ok(row) => Json(json!({ "annotation": fila_json(&row) })).into_response(),
                Err(_) => Json(json!({ "ok": true })).into_response(),
            }
        }
        Err(e) => {
            tracing::warn!("anotaciones: no se pudo actualizar {id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "No se pudo actualizar la anotación" })),
            )
                .into_response()
        }
    }
}

/// `DELETE /api/calendar/annotations/{id}` — borra la anotación.
pub async fn delete(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
) -> Response {
    match sqlx::query("DELETE FROM calendar_annotations WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(r) => {
            if r.rows_affected() == 0 {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "La anotación no existe" })),
                )
                    .into_response();
            }
            Json(json!({ "deleted": true, "id": id })).into_response()
        }
        Err(e) => {
            tracing::warn!("anotaciones: no se pudo borrar {id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "No se pudo borrar la anotación" })),
            )
                .into_response()
        }
    }
}
