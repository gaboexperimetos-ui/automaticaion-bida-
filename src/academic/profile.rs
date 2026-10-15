//! Profile view — student info from Mikareno.
//!
//! GET /dashboard/profile
//!
//! Displays the perfil (nombre, institucion, curso) fetched from the
//! Mikareno MCP, with an HTMX reload button. Falls back to mock data
//! when the MCP is disconnected.

use std::sync::Arc;
use tokio::time::{self, Duration};

use axum::{extract::State, response::Html};
use serde::Deserialize;

use crate::academic::{mikareno_client, templates};
use crate::integrations::mikareno::Perfil;
use crate::AppState;

/// Max length for the `partial` query param (longer values are ignored).
const MAX_PARTIAL_LEN: usize = 32;

/// Optional query parameter for HTMX partial reloads.
#[derive(Deserialize)]
pub struct ProfileQuery {
    partial: Option<String>,
}

/// GET /dashboard/profile — shows perfil info.
pub async fn index(
    State(state): State<Arc<AppState>>,
    query: axum::extract::Query<ProfileQuery>,
) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut perfil = templates::mock_perfil();
    let mut connected = false;

    if let Some(client) = client_opt {
        let init_ok = time::timeout(Duration::from_secs(10), client.initialize())
            .await
            .is_ok();
        if init_ok {
            connected = true;
            perfil = client
                .perfil()
                .await
                .unwrap_or_else(|_| templates::mock_perfil());
        }
    }

    // Partial reload — return just the profile card HTML (overlong values ignored).
    let partial = query
        .partial
        .as_deref()
        .filter(|s| s.len() <= MAX_PARTIAL_LEN);
    if partial == Some("profile") {
        return Html(templates::profile_card_html(&perfil));
    }

    let status = templates::mcp_status_banner(connected);

    let details = format!(
        r###"<div class="grid grid-cols-1 md:grid-cols-2 gap-space-md">
            <div class="bg-surface-container border border-outline rounded-xl p-space-md">
                <h3 class="font-label-sm text-label-sm text-outline mb-space-sm">Nombre completo</h3>
                <p class="font-headline-md text-headline-md text-primary-container">{}</p>
            </div>
            <div class="bg-surface-container border border-outline rounded-xl p-space-md">
                <h3 class="font-label-sm text-label-sm text-outline mb-space-sm">Institución</h3>
                <p class="font-headline-md text-headline-md text-primary-container">{}</p>
            </div>
            <div class="bg-surface-container border border-outline rounded-xl p-space-md">
                <h3 class="font-label-sm text-label-sm text-outline mb-space-sm">Curso actual</h3>
                <p class="font-headline-md text-headline-md text-primary-container">{}</p>
            </div>
            <div class="bg-surface-container border border-outline rounded-xl p-space-md">
                <h3 class="font-label-sm text-label-sm text-outline mb-space-sm">Estado del MCP</h3>
                <p class="font-headline-md text-headline-md text-primary-container">{}</p>
            </div>
        </div>"###,
        templates::html_escape(&perfil.nombre),
        templates::html_escape(&perfil.institucion),
        templates::html_escape(&perfil.curso),
        if connected {
            r###"<span class="text-primary-container">✓ Conectado</span>"###
        } else {
            r###"<span class="text-outline">✗ Sin conexión (datos de ejemplo)</span>"###
        },
    );

    let content = format!(
        r###"{}{}<div id="profile-details">{}</div>"###,
        status,
        templates::profile_card_html(&perfil),
        details
    );

    Html(templates::page("Perfil — Study Platform", &content))
}
