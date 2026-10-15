//! Subjects view — grades grouped by subject with averages.
//!
//! GET /dashboard/subjects
//!
//! Pulls note data from the Mikareno MCP (all trimestres) when connected,
//! groups by materia, computes the average grade per subject, and renders
//! them as cards. Falls back to mock data when the MCP is disconnected.

use std::sync::Arc;
use tokio::time::{self, Duration};

use axum::{extract::State, response::Html};
use serde::Deserialize;

use crate::academic::{mikareno_client, templates};
use crate::integrations::mikareno::Nota;
use crate::AppState;

/// Max length for the `partial` query param (longer values are ignored).
const MAX_PARTIAL_LEN: usize = 32;

/// Optional query parameter for HTMX partial reloads.
#[derive(Deserialize)]
pub struct SubjectsQuery {
    partial: Option<String>,
}

/// GET /dashboard/subjects — subjects with grades and note counts.
pub async fn index(
    State(state): State<Arc<AppState>>,
    query: axum::extract::Query<SubjectsQuery>,
) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut per_subject_notas: Vec<Nota> = Vec::new();
    let mut connected = false;

    if let Some(client) = client_opt {
        let init_ok = time::timeout(Duration::from_secs(10), client.initialize())
            .await
            .is_ok();
        if init_ok {
            connected = true;
            // Fetch notes from all trimestres so we can average across terms.
            for trimestre in [1u8, 2, 3] {
                if let Ok(notas) = client.notas(trimestre).await {
                    per_subject_notas.extend(notas);
                }
            }
        }
    }

    let subjects = if connected {
        templates::subjects_from_notas(&per_subject_notas)
    } else {
        templates::mock_subjects()
    };

    // Partial reload — return just the grid HTML (overlong values ignored).
    let partial = query
        .partial
        .as_deref()
        .filter(|s| s.len() <= MAX_PARTIAL_LEN);
    if partial == Some("grid") {
        return Html(templates::subject_grid_html(&subjects));
    }

    let status = templates::mcp_status_banner(connected);
    let header = format!(
        r###"<div class="flex items-center justify-between mb-space-sm">
        <h2 class="font-headline-sm text-headline-sm text-primary-container">Mis Asignaturas</h2>
        <button hx-get="/dashboard/subjects?partial=grid" hx-target="#subject-grid" hx-swap="innerHTML"
            class="font-label-sm text-label-sm text-primary-container hover:text-primary-fixed-dim" title="Actualizar">
            Actualizar
        </button>
    </div>"###
    );

    let grid_html = format!(
        r###"<div id="subject-grid">{}</div>"###,
        templates::subject_grid_html(&subjects)
    );

    let content = format!(r###"{}{}{}"###, status, header, grid_html);
    Html(templates::page("Asignaturas — Study Platform", &content))
}
