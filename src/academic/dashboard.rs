//! Main dashboard page — daily priorities, quick stats, upcoming events.
//!
//! Supports HTMX partial reloads via the `?partial=` query parameter:
//!   - `stats`    → stats grid HTML
//!   - `profile`  → profile card HTML
//!   - `subjects` → subject quick-links HTML
//!   - `events`   → event timeline HTML
//!   - `tasks`    → pending tasks list HTML

use std::sync::Arc;

use axum::{extract::State, response::Html};
use serde::Deserialize;
use tokio::time::{self, Duration};

use crate::academic::{mikareno_client, templates};
use crate::db;
use crate::integrations::mikareno::{Nota, Tarea};
use crate::AppState;

/// Max length for the `partial` query param (longer values yield empty output).
const MAX_PARTIAL_LEN: usize = 32;

/// Optional query parameter for HTMX partial reloads.
#[derive(Deserialize)]
pub struct DashboardQuery {
    partial: Option<String>,
}

/// GET /dashboard — main dashboard page.
pub async fn index(
    State(state): State<Arc<AppState>>,
    query: axum::extract::Query<DashboardQuery>,
) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut connected = false;
    let mut calendario: Vec<crate::integrations::mikareno::CalendarioEvent> = Vec::new();
    let mut tareas: Vec<Tarea> = Vec::new();
    let mut notas: Vec<Nota> = Vec::new();
    let mut perfil = templates::mock_perfil();

    if let Some(client) = client_opt {
        // Only treat the MCP as connected if the handshake completes within
        // 10s; otherwise fall back to mock data so the dashboard never hangs
        // when the MCP server is unreachable.
        let init_ok = time::timeout(Duration::from_secs(10), client.initialize())
            .await
            .is_ok();
        if init_ok {
            connected = true;
            calendario = client.calendario().await.unwrap_or_default();
            perfil = client
                .perfil()
                .await
                .unwrap_or_else(|_| templates::mock_perfil());
            tareas = client.tareas().await.unwrap_or_default();
            for trimestre in [1u8, 2, 3] {
                if let Ok(n) = client.notas(trimestre).await {
                    notas.extend(n);
                }
            }
        }
    }

    let note_count = db::get_note_count(&state.db).await.unwrap_or(0);
    let subjects = if connected {
        templates::subjects_from_notas(&notas)
    } else {
        templates::mock_subjects()
    };

    let pending_tareas: Vec<&Tarea> = tareas
        .iter()
        .filter(|t| !t.estado.to_lowercase().contains("entregada"))
        .collect();

    // --- Partial responses for HTMX ---
    if let Some(partial) = &query.partial {
        if partial.len() > MAX_PARTIAL_LEN {
            return Html(String::new());
        }
        let html = match partial.as_str() {
            "stats" => templates::stats_grid_html(
                note_count,
                subjects.len(),
                pending_tareas.len(),
                calendario.len().min(5),
            ),
            "profile" => templates::profile_card_html(&perfil),
            "subjects" => templates::subject_grid_html(&subjects),
            "events" => {
                let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                let upcoming: Vec<_> = calendario.iter().take(5).cloned().collect();
                templates::event_timeline_html(&upcoming, Some(&today))
            }
            "tasks" => {
                let items: Vec<String> = pending_tareas
                    .iter()
                    .map(|t| format!("{} — {} ({})", t.titulo, t.vencimiento, t.estado))
                    .collect();
                templates::card_list("Tareas pendientes", &items)
            }
            _ => String::new(),
        };
        return Html(html);
    }

    // --- Full page render (Stitch design) ---
    let status = templates::mcp_status_banner(connected);
    let profile = templates::profile_card_html(&perfil);
    let stats = templates::stats_grid_html(
        note_count,
        subjects.len(),
        pending_tareas.len(),
        calendario.len().min(5),
    );

    // Subject quick-links (limit to 4 for dashboard preview)
    let subject_preview: Vec<_> = subjects.iter().take(4).cloned().collect();
    let subjects_html = format!(
        r###"<div class="mb-space-md">
            <div class="flex items-center justify-between mb-space-sm">
                <h2 class="font-headline-sm text-headline-sm text-primary-container">Asignaturas rapidas</h2>
                <a href="/dashboard/subjects" class="font-label-sm text-label-sm text-primary-container hover:text-primary-fixed-dim hover:underline">Ver todas →</a>
            </div>
            <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-space-md">
                {}
            </div>
        </div>"###,
        templates::subject_grid_html(&subject_preview)
    );

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let upcoming: Vec<_> = calendario.iter().take(5).cloned().collect();
    let events_html = templates::event_timeline_html(&upcoming, Some(&today));
    let events_list = if calendario.is_empty() {
        let mock_events = templates::event_timeline_html(
            &[crate::integrations::mikareno::CalendarioEvent {
                fecha: "2024-03-15".to_string(),
                evento: "Evaluacion de Matematicas".to_string(),
                tipo: "SCHOOL_DAY".to_string(),
            }],
            Some(&today),
        );
        format!(
            r###"<div class="mb-space-md" id="events-section">
                <div class="flex items-center justify-between mb-space-sm">
                    <h2 class="font-headline-sm text-headline-sm text-primary-container">Proximos eventos</h2>
                    <button hx-get="/dashboard?partial=events" hx-target="#events-section" hx-swap="outerHTML"
                        class="font-label-sm text-label-sm text-primary-container hover:text-primary-fixed-dim" title="Actualizar">Actualizar</button>
                </div>
                <div class="text-center py-4">
                    <span class="font-body-sm text-body-sm text-outline">Usando datos de ejemplo</span>
                    <p class="font-body-sm text-body-sm text-outline mt-space-xs">Conecta el MCP para ver eventos reales</p>
                </div>
                <div class="mt-space-sm">{}</div>
            </div>"###,
            mock_events
        )
    } else {
        format!(
            r###"<div class="mb-space-md" id="events-section">
                <div class="flex items-center justify-between mb-space-sm">
                    <h2 class="font-headline-sm text-headline-sm text-primary-container">Proximos eventos</h2>
                    <button hx-get="/dashboard?partial=events" hx-target="#events-section" hx-swap="outerHTML"
                        class="font-label-sm text-label-sm text-primary-container hover:text-primary-fixed-dim" title="Actualizar">Actualizar</button>
                </div>
                {}
            </div>"###,
            events_html
        )
    };

    let tasks_items: Vec<String> = pending_tareas
        .iter()
        .map(|t| format!("{} — {} ({})", t.titulo, t.vencimiento, t.estado))
        .collect();
    let tasks_list = if tasks_items.is_empty() {
        let mock_tasks = templates::card_list(
            "Tareas pendientes",
            &vec![
                "Resolver ejercicios de calculo — vence: 2024-03-10".to_string(),
                "Leer capitulo 5 de fisica — vence: 2024-03-12".to_string(),
            ],
        );
        format!(
            r###"<div class="mb-space-md" id="tasks-section">
                <div class="flex items-center justify-between mb-space-sm">
                    <h2 class="font-headline-sm text-headline-sm text-primary-container">Tareas pendientes</h2>
                    <button hx-get="/dashboard?partial=tasks" hx-target="#tasks-section" hx-swap="outerHTML"
                        class="font-label-sm text-label-sm text-primary-container hover:text-primary-fixed-dim" title="Actualizar">Actualizar</button>
                </div>
                <div class="text-center py-4">
                    <span class="font-body-sm text-body-sm text-outline">Usando datos de ejemplo</span>
                </div>
                <div class="mt-space-sm">{}</div>
            </div>"###,
            mock_tasks
        )
    } else {
        format!(
            r###"<div class="mb-space-md" id="tasks-section">
                <div class="flex items-center justify-between mb-space-sm">
                    <h2 class="font-headline-sm text-headline-sm text-primary-container">Tareas pendientes</h2>
                    <button hx-get="/dashboard?partial=tasks" hx-target="#tasks-section" hx-swap="outerHTML"
                        class="font-label-sm text-label-sm text-primary-container hover:text-primary-fixed-dim" title="Actualizar">Actualizar</button>
                </div>
                {}
            </div>"###,
            templates::card_list("Tareas pendientes", &tasks_items)
        )
    };

    // Daily priorities — nearest pending tasks, or examples when none
    let prio_items: Vec<_> = if pending_tareas.is_empty() {
        vec![
            "Estudiar para el examen de fisica".to_string(),
            "Actualizar notas de la semana".to_string(),
            "Responder dudas de la tarea de calculo".to_string(),
        ]
    } else {
        pending_tareas
            .iter()
            .take(3)
            .map(|t| format!("{} (vence: {})", t.titulo, t.vencimiento))
            .collect()
    };
    let prio_li = prio_items
        .iter()
        .enumerate()
        .map(|(i, text)| {
            format!(
                r###"<li class="flex items-center gap-space-sm p-space-sm rounded-lg bg-surface-container border border-outline hover:border-primary-container transition-colors">
                    <input type="checkbox" class="sr-only peer" id="prio{i}">
                    <span class="flex h-5 w-5 items-center justify-center rounded bg-surface-container border border-outline text-xs text-transparent transition-colors peer-checked:bg-secondary peer-checked:border-secondary peer-checked:text-on-secondary">&#10003;</span>
                    <label class="flex-1 font-body-md text-body-md text-on-surface" for="prio{i}">{}</label>
                </li>"###,
                templates::html_escape(text)
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let priorities = format!(
        r###"<div class="mb-space-md">
            <h2 class="font-headline-sm text-headline-sm text-primary-container mb-space-sm">Prioridades del dia</h2>
            <ul class="list-none space-y-space-xs">{items}</ul>
        </div>"###,
        items = prio_li
    );

    let content = format!(
        "{}{}{}{}{}{}{}",
        status, profile, stats, subjects_html, events_list, tasks_list, priorities
    );

    Html(templates::page(
        "Dashboard Academico — Study Platform",
        &content,
    ))
}
