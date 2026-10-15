//! Calendar views (monthly, weekly, daily).

use axum::{extract::State, response::Html};
use serde::Deserialize;
use std::sync::Arc;
use tokio::time::{self, Duration};

use crate::academic::{mikareno_client, templates};
use crate::AppState;

/// Max length for the `partial` query param (overlong values are ignored).
const MAX_PARTIAL_LEN: usize = 32;

/// Optional query parameter for HTMX partial reloads.
#[derive(Deserialize)]
pub struct CalendarQuery {
    partial: Option<String>,
}

/// GET /dashboard/calendar — monthly calendar view.
pub async fn monthly(
    State(state): State<Arc<AppState>>,
    query: axum::extract::Query<CalendarQuery>,
) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut events: Vec<crate::integrations::mikareno::CalendarioEvent> = Vec::new();
    let mut connected = false;

    if let Some(client) = client_opt {
        // Only treat the MCP as connected if the handshake completes within
        // 10s; otherwise fall back to mock data so the page never hangs.
        let init_ok = time::timeout(Duration::from_secs(10), client.initialize())
            .await
            .is_ok();
        if init_ok {
            connected = true;
            events = client.calendario().await.unwrap_or_default();
        }
    }

    // Partial reload — return just the timeline (overlong values ignored).
    let partial = query
        .partial
        .as_deref()
        .filter(|s| s.len() <= MAX_PARTIAL_LEN);
    if partial == Some("timeline") {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        return Html(templates::event_timeline_html(&events, Some(&today)));
    }

    let status = templates::mcp_status_banner(connected);

    let content = if events.is_empty() {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let mock = templates::event_timeline_html(
            &[crate::integrations::mikareno::CalendarioEvent {
                fecha: today.clone(),
                evento: "Clase de Matematicas - Unidad 4".to_string(),
                tipo: "SCHOOL_DAY".to_string(),
            }],
            Some(&today),
        );
        format!(
            r###"<div class="mb-space-md">
                <h2 class="font-headline-sm text-headline-sm text-primary-container">Linea de tiempo</h2>
                {}
            </div>"###,
            mock
        )
    } else {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let list = templates::card_list(
            "Calendario academico",
            &events
                .iter()
                .take(20)
                .map(|e| format!("{} — {} ({})", e.fecha, e.evento, e.tipo))
                .collect::<Vec<_>>(),
        );
        let timeline = format!(
            r###"<div class="mb-space-md">
                <h2 class="font-headline-sm text-headline-sm text-primary-container">Linea de tiempo</h2>
                {}
            </div>"###,
            templates::event_timeline_html(&events, Some(&today))
        );
        format!("{}{}", status, timeline)
    };

    Html(templates::page("Calendario — Study Platform", &content))
}

/// GET /dashboard/calendar/daily — today's events only.
pub async fn daily(State(state): State<Arc<AppState>>) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut events: Vec<crate::integrations::mikareno::CalendarioEvent> = Vec::new();
    let mut connected = false;

    if let Some(client) = client_opt {
        let init_ok = time::timeout(Duration::from_secs(10), client.initialize())
            .await
            .is_ok();
        if init_ok {
            connected = true;
            events = client.calendario().await.unwrap_or_default();
        }
    }

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // Filter for today's events (fecha starts with YYYY-MM-DD).
    let today_events: Vec<_> = if connected {
        events
            .into_iter()
            .filter(|e| e.fecha.starts_with(&today))
            .collect()
    } else {
        // In mock mode, show mock "today" events so the UI is meaningful.
        templates::mock_today_events()
    };

    let status = templates::mcp_status_banner(connected);

    let header = format!(
        r###"<div class="flex items-center justify-between mb-space-sm">
        <h2 class="font-headline-sm text-headline-sm text-primary-container">Eventos de hoy — {}</h2>
        <button hx-get="/dashboard/calendar/daily" hx-target="#daily-events" hx-swap="innerHTML"
            class="font-label-sm text-label-sm text-primary-container hover:text-primary-fixed-dim" title="Actualizar">
            Actualizar
        </button>
    </div>"###,
        templates::html_escape(&today)
    );

    let timeline = format!(
        r###"<div id="daily-events">{}</div>"###,
        templates::event_timeline_html(&today_events, Some(&today))
    );

    let content = format!(r###"{}{}{}"###, status, header, timeline);
    Html(templates::page(
        "Calendario Diario — Study Platform",
        &content,
    ))
}

/// GET /dashboard/calendar/weekly — weekly view.
pub async fn weekly(State(state): State<Arc<AppState>>) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut events: Vec<crate::integrations::mikareno::CalendarioEvent> = Vec::new();

    if let Some(client) = client_opt {
        let init_ok = time::timeout(Duration::from_secs(10), client.initialize())
            .await
            .is_ok();
        if init_ok {
            events = client.calendario().await.unwrap_or_default();
        }
    }

    let days = [
        "Lunes",
        "Martes",
        "Miercoles",
        "Jueves",
        "Viernes",
        "Sabado",
        "Domingo",
    ];
    let mut day_items: Vec<String> = Vec::new();

    for day in &days {
        let day_events: Vec<&crate::integrations::mikareno::CalendarioEvent> =
            events.iter().filter(|e| e.fecha.contains(day)).collect();
        if !day_events.is_empty() {
            let items: Vec<String> = day_events.iter().map(|e| e.evento.clone()).collect();
            day_items.extend(items);
        } else {
            day_items.push(format!("{} — Sin eventos", day));
        }
    }

    let content = templates::card_list("Vista semanal", &day_items);
    Html(templates::page("Semana — Study Platform", &content))
}
