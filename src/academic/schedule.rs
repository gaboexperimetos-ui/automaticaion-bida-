//! Class schedule (horario) view.

use std::sync::Arc;
use tokio::time::{self, Duration};

use axum::{extract::State, response::Html};

use crate::academic::{mikareno_client, templates};
use crate::AppState;

/// GET /dashboard/horario — weekly schedule view.
pub async fn weekly(State(state): State<Arc<AppState>>) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut horario = crate::integrations::mikareno::Horario::default();

    if let Some(client) = client_opt {
        let init_ok = time::timeout(Duration::from_secs(10), client.initialize())
            .await
            .is_ok();
        if init_ok {
            horario = client.horario().await.unwrap_or_default();
        }
    }

    let content = if horario.dias.is_empty() {
        let items = vec![
            "Lunes: Matematicas 08:00-09:30, Ciencias 09:30-11:00".to_string(),
            "Martes: Lenguaje 08:00-09:30, Historia 09:30-11:00".to_string(),
        ];
        format!(
            r###"<div class="mb-space-md">
                <div class="bg-surface-container border border-outline rounded-xl p-space-md">
                    <h2 class="font-headline-sm text-headline-sm text-primary-container mb-space-sm">Horario de clases</h2>
                    <p class="font-body-sm text-body-sm text-outline mb-space-sm">No se pudo cargar el horario. Revisa que el MCP de Mikareno esté activo.</p>
                    <p class="font-body-sm text-body-sm text-outline">Mostrando datos de ejemplo:</p>
                    <ul class="list-none space-y-space-xs">
                        {}
                    </ul>
                </div>
            </div>"###,
            items.iter()
                .map(|it| {
                    format!(
                        r###"<li class="flex items-center gap-space-sm p-space-sm rounded-lg bg-surface-container border border-outline hover:border-primary-container transition-colors">
                            <span class="font-code-inline text-code-inline text-outline">Lun</span>
                            <span class="font-body-md text-body-md text-on-surface">{}</span>
                        </li>"###,
                        templates::html_escape(it)
                    )
                })
                .collect::<Vec<_>>()
                .join("")
        )
    } else {
        templates::horario_html(&horario.dias)
    };

    Html(templates::page("Horario — Study Platform", &content))
}
