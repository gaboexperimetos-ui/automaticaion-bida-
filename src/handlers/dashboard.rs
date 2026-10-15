//! Dashboard handler — ical export for the calendar feed.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse};

use crate::AppState;

/// Export the user's calendar as an iCal feed.
pub async fn ical_export(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let client = crate::academic::mikareno_client(&state);
    let mut calendario = Vec::new();

    if let Some(client) = client {
        if client.initialize().await.is_ok() {
            calendario = client.calendario().await.unwrap_or_default();
        }
    }

    let mut out = String::new();
    out.push_str("BEGIN:VCALENDAR\r\n");
    out.push_str("VERSION:2.0\r\n");
    out.push_str("PRODID:-//Study Platform//Academic Calendar//EN\r\n");
    out.push_str("CALSCALE:GREGORIAN\r\n");
    out.push_str("METHOD:PUBLISH\r\n");

    for event in calendario {
        if event.tipo == "SCHOOL_DAY" || event.tipo == "EVENTO" {
            out.push_str("BEGIN:VEVENT\r\n");
            out.push_str("DTSTAMP:20261012T000000Z\r\n");
            out.push_str(&format!("DTSTART;VALUE=DATE:{}\r\n", event.fecha));
            out.push_str(&format!("SUMMARY:{}\r\n", event.evento));
            out.push_str("END:VEVENT\r\n");
        }
    }

    out.push_str("END:VCALENDAR\r\n");

    (
        StatusCode::OK,
        [
            ("Content-Type", "text/calendar; charset=utf-8"),
            (
                "Content-Disposition",
                "attachment; filename=\"calendario.ics\"",
            ),
        ],
        out,
    )
        .into_response()
}
