//! Dashboard handler — ical export for the calendar feed.

use std::sync::Arc;
use std::time::Duration;

use axum::{extract::State, http::StatusCode, response::IntoResponse};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tracing::warn;

use crate::integrations::mikareno::CalendarioEvent;
use crate::AppState;

/// Identificador de producto propio (RFC 5545 §3.6).
const PRODID: &str = "-//Study Platform//ES";

/// RFC 5545 §3.1: una línea de contenido no debería exceder 75 octetos; las
/// más largas se pliegan con CRLF seguido de un espacio.
const MAX_OCTETOS_LINEA: usize = 75;

/// Presupuesto propio del export `.ics`.
///
/// El cliente MCP se apoya en sus deadlines (20 s de `initialize` + 25 s de
/// `tools/call` = 45 s en el peor caso). Este límite corta antes: si
/// Mikareno no responde, se devuelve un VCALENDAR **vacío pero válido** —
/// nunca un error 500, nunca media respuesta.
const ICAL_DEADLINE_SECS: u64 = 8;

/// Entrada local del planner/evento que se agrega a la descarga ICS.
#[derive(Debug)]
struct LocalCalendarEntry {
    id: i64,
    date: String,
    title: String,
    body: String,
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

/// Export the user's calendar as an iCal feed.
pub async fn ical_export(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cliente = crate::academic::mikareno_client(&state);
    let mut calendario = Vec::new();
    let mut degradado = false;

    if let Some(client) = cliente {
        let carga = async {
            if client.initialize().await.is_err() {
                return None;
            }
            client.calendario().await.ok()
        };

        match tokio::time::timeout(Duration::from_secs(ICAL_DEADLINE_SECS), carga).await {
            Ok(Some(eventos)) => calendario = eventos,
            // El MCP falló: se responde igual con un calendario vacío.
            Ok(None) => {}
            Err(_) => {
                degradado = true;
                warn!(
                    "ical: Mikareno no respondió a tiempo (presupuesto {}s); se devuelve un calendario vacío",
                    ICAL_DEADLINE_SECS
                );
            }
        }
    }

    let entradas_locales = sqlx::query(
        "SELECT id, date, title, body, kind, end_date, all_day,
                start_time, end_time, location, uid, recurrence, weekly_day
           FROM calendar_annotations
          ORDER BY date ASC, id ASC",
    )
    .fetch_all(&state.db)
    .await;
    let entradas_locales = match entradas_locales {
        Ok(rows) => rows
            .iter()
            .map(|row| LocalCalendarEntry {
                id: row.get::<i64, _>("id"),
                date: row.get::<String, _>("date"),
                title: row.get::<String, _>("title"),
                body: row.get::<String, _>("body"),
                kind: row.get::<String, _>("kind"),
                end_date: row.get::<Option<String>, _>("end_date"),
                all_day: row.get::<i64, _>("all_day") != 0,
                start_time: row.get::<Option<String>, _>("start_time"),
                end_time: row.get::<Option<String>, _>("end_time"),
                location: row.get::<String, _>("location"),
                uid: row.get::<Option<String>, _>("uid"),
                recurrence: row.get::<Option<String>, _>("recurrence"),
                weekly_day: row.get::<Option<i64>, _>("weekly_day"),
            })
            .collect::<Vec<_>>(),
        Err(error) => {
            warn!("ical: no se pudieron leer entradas locales: {error}");
            Vec::new()
        }
    };
    let out = construir_ical(&calendario, &entradas_locales);

    (
        StatusCode::OK,
        [
            ("Content-Type", "text/calendar; charset=utf-8"),
            (
                "Content-Disposition",
                "attachment; filename=\"calendario.ics\"",
            ),
            (
                "X-Mikareno-Estado",
                if degradado { "timeout" } else { "ok" },
            ),
        ],
        out,
    )
        .into_response()
}

/// Construye un solo `VCALENDAR` con eventos de Mikareno y datos locales.
///
/// Solo se emite información real. Un elemento con fecha u hora ilegible se
/// omite en vez de inventar un `DTSTART`; los planner locales se exportan como
/// eventos de día completo.
fn construir_ical(eventos: &[CalendarioEvent], locales: &[LocalCalendarEntry]) -> String {
    let dtstamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

    let mut out = String::new();
    out.push_str("BEGIN:VCALENDAR\r\n");
    out.push_str("VERSION:2.0\r\n");
    out.push_str("PRODID:");
    out.push_str(PRODID);
    out.push_str("\r\n");
    out.push_str("CALSCALE:GREGORIAN\r\n");
    out.push_str("METHOD:PUBLISH\r\n");

    for ev in eventos {
        if ev.tipo != "SCHOOL_DAY" && ev.tipo != "EVENTO" {
            continue;
        }
        let Some(inicio) = inicio_evento(&ev.fecha) else {
            warn!(fecha = %ev.fecha, "ical: fecha ilegible, evento omitido");
            continue;
        };

        out.push_str("BEGIN:VEVENT\r\n");
        out.push_str("DTSTAMP:");
        out.push_str(&dtstamp);
        out.push_str("\r\n");
        out.push_str("UID:");
        out.push_str(&uid_evento(&ev.fecha, &ev.tipo, &ev.evento));
        out.push_str("\r\n");

        match inicio {
            Inicio::Dia(dia) => {
                // Todo el día: `DTSTART` con valor DATE y `DTEND` exclusivo.
                out.push_str("DTSTART;VALUE=DATE:");
                out.push_str(&dia.format("%Y%m%d").to_string());
                out.push_str("\r\n");
                if let Some(siguiente) = dia.succ_opt() {
                    out.push_str("DTEND;VALUE=DATE:");
                    out.push_str(&siguiente.format("%Y%m%d").to_string());
                    out.push_str("\r\n");
                }
            }
            Inicio::FechaHora(fecha_hora) => {
                // Hora local flotante: Mikareno no expone zona horaria, así
                // que no se declara VTIMEZONE ni se inventa un sufijo `Z`.
                out.push_str("DTSTART:");
                out.push_str(&fecha_hora.format("%Y%m%dT%H%M%S").to_string());
                out.push_str("\r\n");
            }
        }

        out.push_str(&escapar_texto(&ev.evento, "SUMMARY:"));
        out.push_str("\r\n");
        out.push_str("END:VEVENT\r\n");
    }

    for entry in locales {
        agregar_evento_local(&mut out, &dtstamp, entry);
    }

    out.push_str("END:VCALENDAR\r\n");
    out
}

fn weekday_code(day: i64) -> Option<&'static str> {
    match day {
        0 => Some("SU"),
        1 => Some("MO"),
        2 => Some("TU"),
        3 => Some("WE"),
        4 => Some("TH"),
        5 => Some("FR"),
        6 => Some("SA"),
        _ => None,
    }
}

fn agregar_evento_local(out: &mut String, dtstamp: &str, entry: &LocalCalendarEntry) {
    let Ok(original_start_date) = NaiveDate::parse_from_str(&entry.date, "%Y-%m-%d") else {
        warn!(id = entry.id, date = %entry.date, "ical: entrada local con fecha ilegible, omitida");
        return;
    };
    let weekly_day = match entry.recurrence.as_deref() {
        Some("weekly") => {
            if entry.kind != "event" || entry.all_day {
                warn!(id = entry.id, "ical: recurrencia semanal inválida, omitida");
                return;
            }
            let Some(day) = entry.weekly_day else {
                warn!(id = entry.id, "ical: recurrencia semanal sin día, omitida");
                return;
            };
            let Some(code) = weekday_code(day) else {
                warn!(id = entry.id, "ical: día semanal inválido, omitido");
                return;
            };
            Some((day, code))
        }
        Some(_) => {
            warn!(
                id = entry.id,
                "ical: tipo de recurrencia no compatible, omitido"
            );
            return;
        }
        None if entry.weekly_day.is_some() => {
            warn!(id = entry.id, "ical: día semanal sin recurrencia, omitido");
            return;
        }
        None => None,
    };
    let start_date = if let Some((day, _)) = weekly_day {
        let current_day = i64::from(original_start_date.weekday().num_days_from_sunday());
        let offset = (day - current_day).rem_euclid(7);
        let Some(date) = original_start_date.checked_add_signed(chrono::Duration::days(offset))
        else {
            warn!(
                id = entry.id,
                "ical: primera fecha semanal fuera de rango, omitida"
            );
            return;
        };
        date
    } else {
        original_start_date
    };
    let todo_dia = entry.all_day || entry.kind == "planner";
    let hora_inicio = if todo_dia {
        None
    } else {
        match entry
            .start_time
            .as_deref()
            .and_then(|value| chrono::NaiveTime::parse_from_str(value, "%H:%M").ok())
        {
            Some(time) => Some(time),
            None => {
                warn!(
                    id = entry.id,
                    "ical: evento local sin hora inicial, omitido"
                );
                return;
            }
        }
    };
    let hora_fin = entry
        .end_time
        .as_deref()
        .and_then(|value| chrono::NaiveTime::parse_from_str(value, "%H:%M").ok());
    if let Some((_, _)) = weekly_day {
        let end_date = entry
            .end_date
            .as_deref()
            .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
        if end_date != Some(original_start_date)
            || hora_inicio.is_none()
            || hora_fin.is_none_or(|end| end <= hora_inicio.unwrap())
        {
            warn!(
                id = entry.id,
                "ical: evento semanal sin rango horario válido, omitido"
            );
            return;
        }
    }
    let uid = if weekly_day.is_some() {
        format!("local-{}@study-platform", entry.id)
    } else {
        entry
            .uid
            .as_deref()
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
            .map(str::to_string)
            .unwrap_or_else(|| format!("local-{}@study-platform", entry.id))
    };

    out.push_str("BEGIN:VEVENT\r\n");
    out.push_str("DTSTAMP:");
    out.push_str(dtstamp);
    out.push_str("\r\nUID:");
    out.push_str(&uid);
    out.push_str("\r\n");

    if todo_dia {
        let end_date = entry
            .end_date
            .as_deref()
            .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
            .filter(|value| *value > start_date)
            .or_else(|| start_date.succ_opt());
        out.push_str("DTSTART;VALUE=DATE:");
        out.push_str(&start_date.format("%Y%m%d").to_string());
        out.push_str("\r\n");
        if let Some(end_date) = end_date {
            out.push_str("DTEND;VALUE=DATE:");
            out.push_str(&end_date.format("%Y%m%d").to_string());
            out.push_str("\r\n");
        }
    } else {
        let Some(start_time) = hora_inicio else {
            return;
        };
        let start = NaiveDateTime::new(start_date, start_time);
        out.push_str("DTSTART:");
        out.push_str(&start.format("%Y%m%dT%H%M%S").to_string());
        out.push_str("\r\n");
        if let Some(end_time) = hora_fin {
            let end_date = if weekly_day.is_some() {
                start_date
            } else {
                entry
                    .end_date
                    .as_deref()
                    .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
                    .unwrap_or(start_date)
            };
            let end = NaiveDateTime::new(end_date, end_time);
            if end > start {
                out.push_str("DTEND:");
                out.push_str(&end.format("%Y%m%dT%H%M%S").to_string());
                out.push_str("\r\n");
            }
        }
        if let Some((_, code)) = weekly_day {
            out.push_str("RRULE:FREQ=WEEKLY;BYDAY=");
            out.push_str(code);
            out.push_str("\r\n");
        }
    }

    out.push_str(&escapar_texto(&entry.title, "SUMMARY:"));
    out.push_str("\r\n");
    if !entry.body.is_empty() {
        out.push_str(&escapar_texto(&entry.body, "DESCRIPTION:"));
        out.push_str("\r\n");
    }
    if !entry.location.is_empty() {
        out.push_str(&escapar_texto(&entry.location, "LOCATION:"));
        out.push_str("\r\n");
    }
    out.push_str("END:VEVENT\r\n");
}

/// Instante de inicio de un evento tal y como lo informó Mikareno.
enum Inicio {
    /// Solo hay fecha: el evento es de todo el día.
    Dia(NaiveDate),
    /// Hay fecha y hora local, sin zona horaria conocida.
    FechaHora(NaiveDateTime),
}

/// Interpreta el campo `fecha` de un evento del calendario.
///
/// Mikareno recorta la fecha a `YYYY-MM-DD`, así que en la práctica todos los
/// eventos son de todo el día. También se acepta `YYYY-MM-DDTHH:MM:SS` para
/// los eventos que sí traigan hora.
fn inicio_evento(fecha: &str) -> Option<Inicio> {
    let f = fecha.trim();
    if f.is_empty() {
        return None;
    }
    if f.contains('T') || f.contains(' ') {
        for formato in [
            "%Y-%m-%dT%H:%M:%S",
            "%Y-%m-%dT%H:%M",
            "%Y-%m-%d %H:%M:%S",
            "%Y-%m-%d %H:%M",
        ] {
            if let Ok(fecha_hora) = NaiveDateTime::parse_from_str(f, formato) {
                return Some(Inicio::FechaHora(fecha_hora));
            }
        }
        return None;
    }
    NaiveDate::parse_from_str(f, "%Y-%m-%d")
        .ok()
        .map(Inicio::Dia)
}

/// UID estable y determinista para un evento.
///
/// Se deriva del contenido real del evento (fecha, tipo y título) con
/// SHA-256, de modo que la misma entrada produce siempre el mismo UID y
/// reimportar el `.ics` no duplica eventos.
fn uid_evento(fecha: &str, tipo: &str, titulo: &str) -> String {
    let mut h = Sha256::new();
    h.update(fecha.as_bytes());
    h.update([0u8]);
    h.update(tipo.as_bytes());
    h.update([0u8]);
    h.update(titulo.as_bytes());
    let digest = h.finalize();
    let mut hex = String::with_capacity(32);
    for b in digest.iter().take(16) {
        let byte = *b;
        hex.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        hex.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap());
    }
    format!("calendario-{hex}@study-platform")
}

/// Escapa un valor TEXT según RFC 5545 §3.3.11 y pliega la línea de
/// contenido para que ninguna línea física supere los 75 octetos
/// (RFC 5545 §3.1).
///
/// `prefijo` es la propiedad ya escrita en la línea (p. ej. `SUMMARY:`) y
/// cuenta para el presupuesto de 75 octetos. El plegado nunca parte una
/// secuencia de escape.
fn escapar_texto(valor: &str, prefijo: &str) -> String {
    let mut out = String::new();
    let mut usados = prefijo.len();
    out.push_str(prefijo);

    for token in tokens_texto(valor) {
        if usados > 0 && usados + token.len() > MAX_OCTETOS_LINEA {
            out.push_str("\r\n ");
            usados = 1;
        }
        out.push_str(&token);
        usados += token.len();
    }
    out
}

/// Divide un valor TEXT en tokens con escape: cada secuencia de escape es un
/// token, de forma que el plegado de línea no pueda partirla.
fn tokens_texto(valor: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for c in valor.chars() {
        match c {
            '\\' => tokens.push("\\\\".to_string()),
            ';' => tokens.push("\\;".to_string()),
            ',' => tokens.push("\\,".to_string()),
            '\n' | '\r' => tokens.push("\\n".to_string()),
            otro => tokens.push(otro.to_string()),
        }
    }
    tokens
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

    fn primer_uid(ics: &str) -> String {
        ics.lines()
            .find_map(|l| l.strip_prefix("UID:"))
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn ical_usa_crlf_y_fechas_honestas() {
        let ics = construir_ical(
            &[
                evento("2026-03-02", "Matemáticas", "SCHOOL_DAY"),
                evento("2026-03-03T08:30:00", "Examen de Lengua", "EVENTO"),
            ],
            &[],
        );

        // Todas las líneas físicas terminan en CRLF: no queda ni un LF suelto.
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(ics.ends_with("END:VCALENDAR\r\n"));
        let sin_crlf = ics.replace("\r\n", "");
        assert!(!sin_crlf.contains('\n'), "queda un LF sin CR");
        assert!(!sin_crlf.contains('\r'), "queda un CR sin LF");

        // Cabeceras obligatorias.
        assert!(ics.contains("VERSION:2.0\r\n"));
        assert!(ics.contains("PRODID:-//Study Platform//ES\r\n"));
        assert!(ics.contains("CALSCALE:GREGORIAN\r\n"));
        assert!(ics.contains("DTSTAMP:"));
        assert!(ics.contains("UID:"));

        // Sin hora → `VALUE=DATE` con formato %Y%m%d; con hora → DATE-TIME.
        assert!(ics.contains("DTSTART;VALUE=DATE:20260302\r\n"));
        assert!(ics.contains("DTSTART:20260303T083000\r\n"));

        // El UID es estable: el mismo evento da el mismo UID en otra
        // exportación.
        let otro = construir_ical(&[evento("2026-03-02", "Matemáticas", "SCHOOL_DAY")], &[]);
        assert_eq!(primer_uid(&ics), primer_uid(&otro));
        assert!(primer_uid(&ics).ends_with("@study-platform"));
    }

    #[test]
    fn ical_exporta_eventos_importados_y_entradas_del_planner() {
        let local = [
            LocalCalendarEntry {
                id: 7,
                date: "2026-03-03".to_string(),
                title: "Cita importada".to_string(),
                body: "Llevar documentos".to_string(),
                kind: "event".to_string(),
                end_date: Some("2026-03-04".to_string()),
                all_day: false,
                start_time: Some("08:30".to_string()),
                end_time: Some("09:45".to_string()),
                location: "Quito".to_string(),
                uid: Some("cita-7@example.test".to_string()),
                recurrence: None,
                weekly_day: None,
            },
            LocalCalendarEntry {
                id: 8,
                date: "2026-03-05".to_string(),
                title: "Repasar álgebra".to_string(),
                body: "Ejercicios 1, 2 y 3".to_string(),
                kind: "planner".to_string(),
                end_date: None,
                all_day: true,
                start_time: None,
                end_time: None,
                location: String::new(),
                uid: None,
                recurrence: None,
                weekly_day: None,
            },
        ];
        let ics = construir_ical(&[], &local);
        assert!(ics.contains("UID:cita-7@example.test\r\n"));
        assert!(ics.contains("DTSTART:20260303T083000\r\n"));
        assert!(ics.contains("DTEND:20260303T094500\r\n"));
        assert!(ics.contains("LOCATION:Quito\r\n"));
        assert!(ics.contains("DTSTART;VALUE=DATE:20260305\r\n"));
        assert!(ics.contains("DTEND;VALUE=DATE:20260306\r\n"));
        assert!(ics.contains("DESCRIPTION:Ejercicios 1\\, 2 y 3\r\n"));
        assert!(ics.matches("BEGIN:VEVENT").count() == 2);
    }

    #[test]
    fn ical_exporta_recurrencia_semanal_por_dia_y_horario() {
        let entry = LocalCalendarEntry {
            id: 9,
            date: "2026-03-03".to_string(),
            title: "Tutoría semanal".to_string(),
            body: "Aula 2".to_string(),
            kind: "event".to_string(),
            end_date: Some("2026-03-03".to_string()),
            all_day: false,
            start_time: Some("08:00".to_string()),
            end_time: Some("09:00".to_string()),
            location: "Quito".to_string(),
            uid: Some("original@example.test".to_string()),
            recurrence: Some("weekly".to_string()),
            weekly_day: Some(3),
        };
        let ics = construir_ical(&[], &[entry]);
        assert!(ics.contains("UID:local-9@study-platform\r\n"));
        assert!(ics.contains("DTSTART:20260304T080000\r\n"));
        assert!(ics.contains("DTEND:20260304T090000\r\n"));
        assert!(ics.contains("RRULE:FREQ=WEEKLY;BYDAY=WE\r\n"));
        assert!(!ics.contains("original@example.test"));
    }

    #[test]
    fn ical_escapa_y_pliega_el_resumen() {
        // Las comas del título (ISBN) se escapan.
        let ics = construir_ical(
            &[evento(
                "2026-03-02",
                "Matemáticas, ISBN 978-84-1234-56-7",
                "SCHOOL_DAY",
            )],
            &[],
        );
        assert!(ics.contains("SUMMARY:Matemáticas\\, ISBN 978-84-1234-56-7\r\n"));
        assert!(!ics.contains("SUMMARY:Matemáticas, "));

        // Barra invertida y punto y coma también.
        let ics = construir_ical(
            &[evento("2026-03-02", "Lengua; Novela\\2", "SCHOOL_DAY")],
            &[],
        );
        assert!(ics.contains("SUMMARY:Lengua\\; Novela\\\\2\r\n"));

        // Las líneas largas se pliegan sin pasar de 75 octetos y sin perder
        // caracteres.
        let largo = "a".repeat(200);
        let ics = construir_ical(&[evento("2026-03-02", &largo, "SCHOOL_DAY")], &[]);
        for linea in ics.split("\r\n") {
            assert!(linea.len() <= 75, "línea de {} octetos", linea.len());
        }
        let mut lineas = ics.split("\r\n").skip_while(|l| !l.starts_with("SUMMARY:"));
        let mut plegado = String::new();
        if let Some(primera) = lineas.next() {
            plegado.push_str(primera.strip_prefix("SUMMARY:").unwrap_or(primera));
        }
        for linea in lineas.take_while(|l| l.starts_with(' ')) {
            plegado.push_str(linea.strip_prefix(' ').unwrap_or(linea));
        }
        assert_eq!(plegado, largo);
    }
}
