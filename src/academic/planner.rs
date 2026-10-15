//! Weekly planner with task management.

use std::sync::Arc;
use tokio::time::{self, Duration};

use axum::{extract::State, response::Html};

use crate::academic::{mikareno_client, templates};
use crate::AppState;

/// GET /dashboard/planner — weekly planner view.
pub async fn weekly(State(state): State<Arc<AppState>>) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut tareas: Vec<crate::integrations::mikareno::Tarea> = Vec::new();

    if let Some(client) = client_opt {
        let init_ok = time::timeout(Duration::from_secs(10), client.initialize())
            .await
            .is_ok();
        if init_ok {
            tareas = client.tareas().await.unwrap_or_default();
        }
    }

    let days = [
        "Lunes",
        "Martes",
        "Miércoles",
        "Jueves",
        "Viernes",
        "Sábado",
        "Domingo",
    ];

    let mut planner_html =
        String::from(r###"<div class="grid grid-cols-1 lg:grid-cols-7 gap-space-md">"###);
    for day in &days {
        let tasks = tareas
            .iter()
            .map(|t| {
                format!(
                    r###"<li class="flex items-center gap-space-sm p-space-sm rounded-lg bg-surface-container border border-outline hover:border-primary-container transition-colors">
                        <input type="checkbox" class="sr-only peer" id="t_{}">
                        <span class="flex h-5 w-5 items-center justify-center rounded bg-surface-container border border-outline text-xs text-transparent transition-colors peer-checked:bg-secondary peer-checked:border-secondary peer-checked:text-on-secondary">&#10003;</span>
                        <label class="flex-1 font-body-sm text-body-sm text-on-surface" for="t_{}">{}</label>
                    </li>"###,
                    templates::html_escape(&day.replace(' ', "_")) + &t.titulo.len().to_string(),
                    templates::html_escape(&day.replace(' ', "_")) + &t.titulo.len().to_string(),
                    templates::html_escape(&format!("{} — {}", t.titulo, t.vencimiento))
                )
            })
            .collect::<Vec<_>>()
            .join("");

        planner_html.push_str(&format!(
            r###"<div class="bg-surface-container border border-outline rounded-xl p-space-sm min-h-[200px]">
                <h3 class="font-headline-sm text-headline-sm text-primary-container mb-space-sm">{}</h3>
                <ul class="list-none space-y-space-xs">{}</ul>
            </div>"###,
            day, tasks
        ));
    }
    planner_html.push_str("</div>");

    let tasks_html = if tareas.is_empty() {
        templates::card_list(
            "Tareas pendientes",
            &vec![
                "Resolver ejercicios de cálculo — vence: 2024-03-10".to_string(),
                "Leer capítulo 5 de física — vence: 2024-03-12".to_string(),
            ],
        )
    } else {
        let items: Vec<String> = tareas
            .iter()
            .map(|t| format!("{} — {} ({})", t.titulo, t.vencimiento, t.estado))
            .collect();
        templates::card_list("Tareas pendientes", &items)
    };

    let content = format!(r###"{}{}"###, planner_html, tasks_html);

    Html(templates::page("Planificador — Study Platform", &content))
}
