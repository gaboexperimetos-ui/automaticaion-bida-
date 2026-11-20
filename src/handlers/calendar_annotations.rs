//! Anotaciones y eventos locales del calendario.
//!
//! Mikareno es de solo lectura; el planner y los eventos importados se guardan
//! localmente en `calendar_annotations`. La categoría y los campos iCalendar
//! se añaden mediante la migración `20240101000008_calendar_planner_category.sql`.
//!
//! - `GET /api/calendar/annotations?from=YYYY-MM-DD&to=YYYY-MM-DD` lista entradas
//!   que empiezan en el rango y eventos importados que se solapan con él.
//! - `POST /api/calendar/annotations` crea una entrada del planner o un evento.
//! - `POST /api/calendar/annotations/import` importa un lote validado de eventos.
//! - `PUT` y `DELETE /api/calendar/annotations/{id}` editan/eliminan entradas.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Datelike;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};

use crate::AppState;

const MAX_TITLE: usize = 120;
const MAX_BODY: usize = 4000;
const MAX_LOCATION: usize = 256;
const MAX_UID: usize = 512;
const MAX_IMPORT_EVENTS: usize = 1000;
const CATEGORIAS: [&str; 4] = ["academic", "work", "health", "personal"];

/// Filtro opcional por rango de fechas.
#[derive(Debug, Deserialize)]
pub struct RangeQuery {
    from: Option<String>,
    to: Option<String>,
}

/// Entrada nueva: los campos iCalendar solo se usan cuando `kind` es `event`.
#[derive(Debug, Deserialize)]
pub struct NewAnnotation {
    date: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    end_date: Option<String>,
    #[serde(default)]
    all_day: Option<bool>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    end_time: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    uid: Option<String>,
    #[serde(default)]
    recurrence: Option<String>,
    #[serde(default)]
    weekly_day: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ImportAnnotations {
    annotations: Vec<NewAnnotation>,
}

/// Cuerpo de edición parcial del planner.
#[derive(Debug, Deserialize)]
pub struct EditAnnotation {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    category: Option<String>,
}

#[derive(Debug)]
struct NormalizedAnnotation {
    date: String,
    title: String,
    body: String,
    category: String,
    kind: String,
    end_date: Option<String>,
    all_day: bool,
    start_time: Option<String>,
    end_time: Option<String>,
    location: String,
    uid: Option<String>,
    recurrence: Option<String>,
    weekly_day: Option<i64>,
}

fn fecha_valida(value: &str) -> bool {
    chrono::NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").is_ok()
}

fn limpia(value: &str, max: usize) -> String {
    value.trim().chars().take(max).collect()
}

fn categoria_valida(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    CATEGORIAS.contains(&value.as_str()).then_some(value)
}

fn hora_normalizada(value: &str) -> Option<String> {
    let value = value.trim();
    chrono::NaiveTime::parse_from_str(value, "%H:%M")
        .ok()
        .map(|_| value.to_string())
}

fn normaliza(payload: NewAnnotation) -> Result<NormalizedAnnotation, &'static str> {
    if !fecha_valida(&payload.date) {
        return Err("La fecha debe tener formato YYYY-MM-DD");
    }
    let date = payload.date.trim().to_string();
    let title = limpia(&payload.title, MAX_TITLE);
    let body = limpia(&payload.body, MAX_BODY);
    let kind = payload
        .kind
        .as_deref()
        .unwrap_or("planner")
        .trim()
        .to_ascii_lowercase();
    if kind != "planner" && kind != "event" {
        return Err("El tipo debe ser planner o event");
    }
    if (kind == "event" && title.is_empty()) || (title.is_empty() && body.is_empty()) {
        return Err("La entrada necesita un título");
    }
    let category_default = if kind == "event" {
        "personal"
    } else {
        "academic"
    };
    let category = categoria_valida(payload.category.as_deref().unwrap_or(category_default))
        .ok_or("La categoría debe ser academic, work, health o personal")?;

    if kind == "planner" {
        if payload.recurrence.is_some() || payload.weekly_day.is_some() {
            return Err("La recurrencia solo se admite en eventos importados");
        }
        return Ok(NormalizedAnnotation {
            date,
            title,
            body,
            category,
            kind,
            end_date: None,
            all_day: true,
            start_time: None,
            end_time: None,
            location: String::new(),
            uid: None,
            recurrence: None,
            weekly_day: None,
        });
    }

    let all_day = payload.all_day.unwrap_or(true);
    let start_time = match payload.start_time.as_deref() {
        Some(value) => {
            Some(hora_normalizada(value).ok_or("La hora inicial debe tener formato HH:MM")?)
        }
        None => None,
    };
    let end_time = match payload.end_time.as_deref() {
        Some(value) => {
            Some(hora_normalizada(value).ok_or("La hora final debe tener formato HH:MM")?)
        }
        None => None,
    };
    if all_day && (start_time.is_some() || end_time.is_some()) {
        return Err("Un evento de día completo no puede incluir horas");
    }
    if !all_day && start_time.is_none() {
        return Err("Un evento con hora necesita una hora inicial");
    }

    let date_start = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d")
        .map_err(|_| "La fecha debe tener formato YYYY-MM-DD")?;
    let end_date = match payload.end_date.as_deref() {
        Some(value) => {
            if !fecha_valida(value) {
                return Err("La fecha final debe tener formato YYYY-MM-DD");
            }
            value.trim().to_string()
        }
        None if all_day => date_start
            .succ_opt()
            .ok_or("La fecha final está fuera del rango permitido")?
            .format("%Y-%m-%d")
            .to_string(),
        None => date.clone(),
    };
    let date_end = chrono::NaiveDate::parse_from_str(&end_date, "%Y-%m-%d")
        .map_err(|_| "La fecha final debe tener formato YYYY-MM-DD")?;
    if (all_day && date_end <= date_start) || (!all_day && date_end < date_start) {
        return Err("La fecha final debe ser posterior a la inicial");
    }
    if !all_day && date_end == date_start {
        if let (Some(start), Some(end)) = (&start_time, &end_time) {
            if end <= start {
                return Err("La hora final debe ser posterior a la inicial");
            }
        }
    }

    let uid = payload
        .uid
        .as_deref()
        .map(|value| limpia(value, MAX_UID))
        .filter(|value| !value.is_empty());
    if uid
        .as_deref()
        .is_some_and(|value| value.chars().any(char::is_control))
    {
        return Err("El UID no puede contener caracteres de control");
    }

    let recurrence = payload
        .recurrence
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let weekly_day = payload.weekly_day;
    match recurrence.as_deref() {
        Some("weekly") => {
            if !all_day && date_end == date_start && start_time.is_some() && end_time.is_some() {
                if !(0..=6).contains(&weekly_day.unwrap_or(-1)) {
                    return Err("Un evento semanal necesita un día de la semana entre 0 y 6");
                }
            } else {
                return Err("La recurrencia semanal requiere un evento horario de un solo día y ambas horas");
            }
        }
        Some(_) => return Err("La única recurrencia admitida es weekly"),
        None if weekly_day.is_some() => {
            return Err("El día semanal solo se admite con recurrencia weekly");
        }
        None => {}
    }

    Ok(NormalizedAnnotation {
        date,
        title,
        body,
        category,
        kind,
        end_date: Some(end_date),
        all_day,
        start_time,
        end_time,
        location: limpia(payload.location.as_deref().unwrap_or(""), MAX_LOCATION),
        uid,
        recurrence,
        weekly_day,
    })
}

fn fila_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<i64, _>("id"),
        "date": row.get::<String, _>("date"),
        "title": row.get::<String, _>("title"),
        "body": row.get::<String, _>("body"),
        "category": row.get::<String, _>("category"),
        "kind": row.get::<String, _>("kind"),
        "end_date": row.get::<Option<String>, _>("end_date"),
        "all_day": row.get::<i64, _>("all_day") != 0,
        "start_time": row.get::<Option<String>, _>("start_time"),
        "end_time": row.get::<Option<String>, _>("end_time"),
        "location": row.get::<String, _>("location"),
        "uid": row.get::<Option<String>, _>("uid"),
        "recurrence": row.get::<Option<String>, _>("recurrence"),
        "weekly_day": row.get::<Option<i64>, _>("weekly_day"),
        "created_at": row.get::<String, _>("created_at"),
        "updated_at": row.get::<String, _>("updated_at"),
    })
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

async fn insertar(pool: &SqlitePool, entry: &NormalizedAnnotation) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO calendar_annotations
            (date, title, body, category, kind, end_date, all_day,
             start_time, end_time, location, uid, recurrence, weekly_day)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&entry.date)
    .bind(&entry.title)
    .bind(&entry.body)
    .bind(&entry.category)
    .bind(&entry.kind)
    .bind(&entry.end_date)
    .bind(i64::from(entry.all_day))
    .bind(&entry.start_time)
    .bind(&entry.end_time)
    .bind(&entry.location)
    .bind(&entry.uid)
    .bind(&entry.recurrence)
    .bind(entry.weekly_day)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

const SELECT_ANNOTATION: &str = "SELECT id, date, title, body, category, kind, end_date, all_day,
            start_time, end_time, location, uid, recurrence, weekly_day, created_at, updated_at
       FROM calendar_annotations";

/// Lista entradas del rango y eventos locales que lo atraviesan.
pub async fn list(
    State(state): State<std::sync::Arc<AppState>>,
    Query(q): Query<RangeQuery>,
) -> Response {
    let hoy = chrono::Local::now().date_naive();
    let (desde, hasta) = match (q.from.as_deref(), q.to.as_deref()) {
        (Some(from), Some(to)) if fecha_valida(from) && fecha_valida(to) => {
            (from.trim().to_string(), to.trim().to_string())
        }
        (Some(from), None) if fecha_valida(from) => {
            let inicio = chrono::NaiveDate::parse_from_str(from.trim(), "%Y-%m-%d").unwrap_or(hoy);
            let fin = inicio + chrono::Duration::days(31);
            (
                inicio.format("%Y-%m-%d").to_string(),
                fin.format("%Y-%m-%d").to_string(),
            )
        }
        _ => {
            let primero = hoy.with_day(1).unwrap_or(hoy);
            let ultimo = primero
                .checked_add_signed(chrono::Duration::days(31))
                .map(|date| date.with_day(1).unwrap_or(date))
                .and_then(|date| date.pred_opt())
                .unwrap_or(hoy);
            (
                primero.format("%Y-%m-%d").to_string(),
                ultimo.format("%Y-%m-%d").to_string(),
            )
        }
    };

    let query = format!(
        "{SELECT_ANNOTATION}
          WHERE (date >= ? AND date <= ?)
             OR (kind = 'event' AND date < ? AND
                 ((all_day = 1 AND end_date > ?)
                   OR (all_day = 0 AND COALESCE(end_date, date) >= ?)))
             OR (kind = 'event' AND recurrence = 'weekly')
          ORDER BY date ASC, id ASC"
    );
    let rows = sqlx::query(&query)
        .bind(&desde)
        .bind(&hasta)
        .bind(&desde)
        .bind(&desde)
        .bind(&desde)
        .fetch_all(&state.db)
        .await;
    match rows {
        Ok(rows) => {
            let annotations: Vec<Value> = rows.iter().map(fila_json).collect();
            Json(json!({
                "from": desde,
                "to": hasta,
                "count": annotations.len(),
                "annotations": annotations,
            }))
            .into_response()
        }
        Err(error) => {
            tracing::warn!("anotaciones: no se pudo leer la tabla: {error}");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudieron leer las anotaciones",
            )
        }
    }
}

/// Crea una entrada del planner o un evento local.
pub async fn create(
    State(state): State<std::sync::Arc<AppState>>,
    Json(payload): Json<NewAnnotation>,
) -> Response {
    let entry = match normaliza(payload) {
        Ok(entry) => entry,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    match insertar(&state.db, &entry).await {
        Ok(id) => {
            let query = format!("{SELECT_ANNOTATION} WHERE id = ?");
            match sqlx::query(&query).bind(id).fetch_one(&state.db).await {
                Ok(row) => (
                    StatusCode::CREATED,
                    Json(json!({ "annotation": fila_json(&row) })),
                )
                    .into_response(),
                Err(error) => {
                    tracing::warn!("anotaciones: se guardó {id}, pero no se pudo releer: {error}");
                    (
                        StatusCode::CREATED,
                        Json(json!({
                            "annotation": {
                                "id": id,
                                "date": entry.date,
                                "title": entry.title,
                                "body": entry.body,
                                "category": entry.category,
                                "kind": entry.kind,
                                "end_date": entry.end_date,
                                "all_day": entry.all_day,
                                "start_time": entry.start_time,
                                "end_time": entry.end_time,
                                "location": entry.location,
                                "uid": entry.uid,
                                "recurrence": entry.recurrence,
                                "weekly_day": entry.weekly_day
                            }
                        })),
                    )
                        .into_response()
                }
            }
        }
        Err(error) => {
            tracing::warn!("anotaciones: no se pudo crear: {error}");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo guardar la anotación",
            )
        }
    }
}

/// Importación en lote atómica para evitar peticiones por cada VEVENT.
pub async fn import(
    State(state): State<std::sync::Arc<AppState>>,
    Json(payload): Json<ImportAnnotations>,
) -> Response {
    if payload.annotations.is_empty() || payload.annotations.len() > MAX_IMPORT_EVENTS {
        return error_response(
            StatusCode::BAD_REQUEST,
            "El lote debe contener entre 1 y 1000 eventos",
        );
    }
    let entries: Result<Vec<_>, _> = payload
        .annotations
        .into_iter()
        .enumerate()
        .map(|(index, payload)| {
            normaliza(payload).map_err(|error| format!("Evento {}: {error}", index + 1))
        })
        .collect();
    let entries = match entries {
        Ok(entries) => entries,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error),
    };

    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::warn!("ical import: no se pudo iniciar transacción: {error}");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo iniciar la importación",
            );
        }
    };
    for entry in &entries {
        let result = sqlx::query(
            "INSERT INTO calendar_annotations
                (date, title, body, category, kind, end_date, all_day,
                 start_time, end_time, location, uid, recurrence, weekly_day)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&entry.date)
        .bind(&entry.title)
        .bind(&entry.body)
        .bind(&entry.category)
        .bind(&entry.kind)
        .bind(&entry.end_date)
        .bind(i64::from(entry.all_day))
        .bind(&entry.start_time)
        .bind(&entry.end_time)
        .bind(&entry.location)
        .bind(&entry.uid)
        .bind(&entry.recurrence)
        .bind(entry.weekly_day)
        .execute(&mut *tx)
        .await;
        if let Err(error) = result {
            tracing::warn!("ical import: fallo al guardar el lote: {error}");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo completar la importación; no se guardaron cambios",
            );
        }
    }
    if let Err(error) = tx.commit().await {
        tracing::warn!("ical import: no se pudo confirmar el lote: {error}");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "No se pudo completar la importación",
        );
    }
    Json(json!({ "imported": entries.len() })).into_response()
}

/// Actualiza título, fecha, categoría y descripción de una entrada local.
pub async fn update(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
    Json(payload): Json<EditAnnotation>,
) -> Response {
    let existing = sqlx::query("SELECT id FROM calendar_annotations WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await;
    match existing {
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "La anotación no existe"),
        Err(error) => {
            tracing::warn!("anotaciones: no se pudo leer {id}: {error}");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo leer la anotación",
            );
        }
        Ok(Some(_)) => {}
    }
    if payload
        .date
        .as_deref()
        .is_some_and(|date| !fecha_valida(date))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "La fecha debe tener formato YYYY-MM-DD",
        );
    }
    let category = match payload.category.as_deref() {
        Some(value) => match categoria_valida(value) {
            Some(category) => Some(category),
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "La categoría debe ser academic, work, health o personal",
                )
            }
        },
        None => None,
    };
    let query = format!(
        "UPDATE calendar_annotations
            SET date = COALESCE(?, date),
                title = COALESCE(?, title),
                body = COALESCE(?, body),
                category = COALESCE(?, category),
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE id = ?"
    );
    let result = sqlx::query(&query)
        .bind(payload.date.as_deref().map(str::trim))
        .bind(
            payload
                .title
                .as_deref()
                .map(|title| limpia(title, MAX_TITLE)),
        )
        .bind(payload.body.as_deref().map(|body| limpia(body, MAX_BODY)))
        .bind(category)
        .bind(id)
        .execute(&state.db)
        .await;
    match result {
        Ok(_) => {
            let query = format!("{SELECT_ANNOTATION} WHERE id = ?");
            match sqlx::query(&query).bind(id).fetch_one(&state.db).await {
                Ok(row) => Json(json!({ "annotation": fila_json(&row) })).into_response(),
                Err(_) => Json(json!({ "ok": true })).into_response(),
            }
        }
        Err(error) => {
            tracing::warn!("anotaciones: no se pudo actualizar {id}: {error}");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo actualizar la anotación",
            )
        }
    }
}

/// Elimina una entrada local.
pub async fn delete(
    State(state): State<std::sync::Arc<AppState>>,
    Path(id): Path<i64>,
) -> Response {
    match sqlx::query("DELETE FROM calendar_annotations WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await
    {
        Ok(result) if result.rows_affected() == 0 => {
            error_response(StatusCode::NOT_FOUND, "La anotación no existe")
        }
        Ok(_) => Json(json!({ "deleted": true, "id": id })).into_response(),
        Err(error) => {
            tracing::warn!("anotaciones: no se pudo borrar {id}: {error}");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo borrar la anotación",
            )
        }
    }
}
