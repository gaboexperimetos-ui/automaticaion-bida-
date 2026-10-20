//! Dashboard handler — ical export for the calendar feed.

use std::sync::Arc;
use std::time::Duration;

use axum::{extract::State, http::StatusCode, response::IntoResponse};
use chrono::{NaiveDate, NaiveDateTime, Utc};
use sha2::{Digest, Sha256};
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

    let out = construir_ical(&calendario);

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

/// Construye el cuerpo `VCALENDAR` a partir de los eventos ya cargados de
/// Mikareno.
///
/// Solo se emite data real: el título y la fecha son los que devolvió
/// Mikareno y `DTSTAMP` es el instante real de generación (RFC 5545 exige un
/// DATE-TIME en UTC). Un evento con fecha ilegible se omite en vez de
/// inventar un `DTSTART`.
fn construir_ical(eventos: &[CalendarioEvent]) -> String {
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

    out.push_str("END:VCALENDAR\r\n");
    out
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
        let ics = construir_ical(&[
            evento("2026-03-02", "Matemáticas", "SCHOOL_DAY"),
            evento("2026-03-03T08:30:00", "Examen de Lengua", "EVENTO"),
        ]);

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
        let otro = construir_ical(&[evento("2026-03-02", "Matemáticas", "SCHOOL_DAY")]);
        assert_eq!(primer_uid(&ics), primer_uid(&otro));
        assert!(primer_uid(&ics).ends_with("@study-platform"));
    }

    #[test]
    fn ical_escapa_y_pliega_el_resumen() {
        // Las comas del título (ISBN) se escapan.
        let ics = construir_ical(&[evento(
            "2026-03-02",
            "Matemáticas, ISBN 978-84-1234-56-7",
            "SCHOOL_DAY",
        )]);
        assert!(ics.contains("SUMMARY:Matemáticas\\, ISBN 978-84-1234-56-7\r\n"));
        assert!(!ics.contains("SUMMARY:Matemáticas, "));

        // Barra invertida y punto y coma también.
        let ics = construir_ical(&[evento("2026-03-02", "Lengua; Novela\\2", "SCHOOL_DAY")]);
        assert!(ics.contains("SUMMARY:Lengua\\; Novela\\\\2\r\n"));

        // Las líneas largas se pliegan sin pasar de 75 octetos y sin perder
        // caracteres.
        let largo = "a".repeat(200);
        let ics = construir_ical(&[evento("2026-03-02", &largo, "SCHOOL_DAY")]);
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
