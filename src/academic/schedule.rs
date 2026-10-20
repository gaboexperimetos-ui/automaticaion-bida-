//! Class schedule (horario) view.
//!
//! Presupuesto corto: la página se entrega en ≤5 s aunque Mikareno no
//! conteste. `client.horario()` es *cache-first* (caché read-through del
//! cliente compartido, TTL 180 s): un horario ya descargado se sirve sin
//! I/O y **sin re-raspar** la plataforma. Si la llamada no llega a tiempo,
//! la página se pinta YA con el estado vacío honesto.
//!
//! Grilla de SIEMPRE 7 columnas (lunes→domingo) con la fecha de cada día y
//! el día actual resaltado; los días que Mikareno no trae muestran "Sin
//! clases" (`horario_html_detalles`, puro y sin I/O).

use std::sync::Arc;
use tokio::time::{self, Duration, Instant};

use axum::{extract::State, response::Html};
use chrono::Local;

use crate::academic::dashboard::con_deadline;
use crate::academic::subjects::handshake_ok;
use crate::academic::{mikareno_client, templates};
use crate::handlers::schedule::{horario_html_detalles, rango_semana, semana_actual};
use crate::integrations::mikareno::{Horario, MikarenoClient};
use crate::AppState;

/// Presupuesto total de la carga de la página (handshake + horario).
///
/// Antes eran 30 s (más el timeout de 30 s del cliente MCP): la página
/// tardaba medio minuto en pintar con el servidor ocupado.
const PRESUPUESTO_PAGINA: Duration = Duration::from_secs(5);

/// Tope del handshake MCP dentro del presupuesto total.
const PRESUPUESTO_HANDSHAKE: Duration = Duration::from_secs(2);

/// Trae el horario del MCP con presupuesto acotado.
///
/// `client.horario()` es *cache-first* (caché read-through del cliente, TTL
/// 180 s) y se auto-inicializa si la sesión MCP no está viva, así que la
/// llamada normal no necesita handshake previo ni re-rasca la plataforma.
/// Si la llamada falla o expira, se devuelve `None` y la página se pinta
/// igual con el estado vacío honesto.
async fn horario_con_presupuesto(client: &MikarenoClient, deadline: Instant) -> Option<Horario> {
    con_deadline(deadline, client.horario()).await
}

/// GET /dashboard/horario — grilla semanal con SIEMPRE 7 columnas.
pub async fn weekly(State(state): State<Arc<AppState>>) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut horario = Horario::default();
    let mut conectado = false;
    // Aviso honesto cuando Mikareno no responde ahora.
    let mut aviso: Option<&'static str> = None;

    if let Some(client) = client_opt {
        // Un único plazo para el handshake y la llamada: la página no espera
        // más de 5 s en ningún caso.
        let deadline = Instant::now() + PRESUPUESTO_PAGINA;

        // Handshake con deadline corto. Sólo `Ok(Ok(()))` es conexión real:
        // `Ok(Err(..))` es un handshake FALLIDO (MCP caído) y el `is_ok()`
        // exterior lo declaraba "Conectado" con el servidor muerto.
        let restante = deadline.saturating_duration_since(Instant::now());
        let init_ok = handshake_ok(
            time::timeout(restante.min(PRESUPUESTO_HANDSHAKE), client.initialize()).await,
        );

        // El horario es cache-first: si ya está en la caché del cliente se
        // sirve sin I/O; si hay que raspar, el deadline corta la espera.
        match horario_con_presupuesto(&client, deadline).await {
            Some(h) => {
                horario = h;
                // Con datos reales la página muestra horario: el banner no
                // puede decir "sin datos" mientras hay horario en pantalla.
                conectado = init_ok || !horario.dias.is_empty();
                if !init_ok {
                    aviso = Some(
                        "Mikareno no responde ahora: se muestra el horario de la última lectura.",
                    );
                }
            }
            None => {
                aviso = Some("Mikareno no responde ahora: el horario no se pudo cargar.");
            }
        }
    }

    let hoy = Local::now().date_naive();
    let semana = semana_actual(hoy);
    let aviso_html = aviso
        .map(|a| {
            format!(
                r#"<div class="bg-surface-container border border-outline rounded-xl p-3 mb-space-md">
            <p class="font-body-sm text-body-sm text-on-surface-variant">{a}</p>
        </div>"#
            )
        })
        .unwrap_or_default();

    let content = format!(
        r#"<div class="mb-space-md">
            <h2 class="font-headline-sm text-headline-sm text-primary-container font-semibold">Horario semanal</h2>
            <p class="font-body-sm text-body-sm text-on-surface-variant">{}</p>
        </div>
        {}{}{}"#,
        rango_semana(semana[0], semana[6]),
        templates::mcp_status_banner(conectado),
        aviso_html,
        horario_html_detalles(&horario.dias, hoy),
    );

    // Stitch v2 shell con breadcrumb + nav activo resueltos vía page_ext.
    Html(templates::page_ext(
        "Horario — Study Platform",
        "Horario",
        "/dashboard/horario",
        &content,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sin_mikareno_la_pagina_no_espera_y_devuelve_vacio() {
        // MCP caído (puerto cerrado): la llamada falla o expira y la página
        // sigue teniendo un horario vacío que pintar de inmediato.
        let client = MikarenoClient::new("http://127.0.0.1:1/mcp".to_string());
        let deadline = Instant::now() + Duration::from_millis(300);
        let h = horario_con_presupuesto(&client, deadline).await;
        assert!(h.is_none() || h.unwrap().dias.is_empty());
    }
}
