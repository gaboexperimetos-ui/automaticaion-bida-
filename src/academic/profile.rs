//! Profile view — student info from Mikareno.
//!
//! GET /dashboard/profile
//!
//! Displays the perfil (nombre, institucion, curso) fetched from the
//! Mikareno MCP, with an HTMX reload button. When the MCP is disconnected
//! or the fetch fails, the fields stay empty — never fabricated data.
//!
//! Also renders the progress bars per subject (promedio 0–10) built from the
//! same real source the rest of the app uses (`get_student_full →
//! promedios.stats`): one bar per materia with the current average. The bar
//! width is the real percentage of that average over 10 — no animation and
//! no invented subjects; without real data the section shows an honest
//! empty state instead of bars.
//!
//! The averages are shown with the same precision and the same color scale
//! as `/dashboard/subjects` (shared `format_grade` / `grade_colors`), and
//! in the same order (worst average first, shared `order_subjects`), so
//! both pages tell the same story about the same numbers.
//!
//! The MCP connection state is reported honestly: the handshake must
//! SUCCEED, not merely fit in the time budget — with the MCP down the page
//! says "Sin conexión" and shows the empty state instead of pretending
//! to be connected with empty fields.
//!
//! The page has a **5 s budget** for everything it asks the MCP (handshake,
//! `get_perfil` and the promedios, in parallel). If the server does not
//! answer in time the page is delivered right away with the honest empty
//! state — it never waits longer.
//!
//! The promedios come from the same source as `/dashboard/subjects`
//! (`get_student_full → promedios.stats`) and are served from a shared
//! in-memory cache (30 s), so the two pages do not call each other nor
//! repeat the same MCP call.

use std::sync::Arc;
use tokio::time::{self, Instant};

use axum::{extract::State, response::Html};
use serde::Deserialize;

use crate::academic::subjects::{
    format_grade, grade_colors, handshake_ok, order_subjects, promedios_con_presupuesto,
    warn_rearmar, warn_una_vez, CLAVE_PERFIL, PRESUPUESTO_HANDSHAKE, PRESUPUESTO_MCP,
};
use crate::academic::{mikareno_client, templates};
use crate::integrations::mikareno::{Nota, Perfil};
use crate::AppState;

/// Max length for the `partial` query param (longer values are ignored).
const MAX_PARTIAL_LEN: usize = 32;

/// Optional query parameter for HTMX partial reloads.
#[derive(Deserialize)]
pub struct ProfileQuery {
    partial: Option<String>,
}

/// Render a profile field, or an honest placeholder when the value is empty.
fn field_html(value: &str) -> String {
    if value.is_empty() {
        r###"<span class="text-outline">Sin datos</span>"###.to_string()
    } else {
        templates::html_escape(value)
    }
}

/// Partial reload key for the subject progress bars.
const PARTIAL_PROGRESS: &str = "progress";

/// Porcentaje real del promedio sobre 10, acotado a 0–100.
fn promedio_pct(promedio: f64) -> i64 {
    ((promedio / 10.0) * 100.0).clamp(0.0, 100.0).round() as i64
}

/// Notas con promedio real, descartando las materias que aún no lo tienen.
///
/// La fuente compartida (`promedios.stats`) marca con `NaN` las materias sin
/// promedio todavía. Una barra de progreso con `NaN` sería una mentira
/// ("NaN%" en pantalla), así que el perfil solo dibuja las materias que sí
/// tienen promedio; las demás las cuenta la página de asignaturas con su
/// honesto "sin promedio todavía".
fn notas_con_promedio(notas: &[Nota]) -> Vec<Nota> {
    notas
        .iter()
        .filter(|n| n.nota.is_finite())
        .cloned()
        .collect()
}

/// Barras de progreso por materia con promedios reales (escala 0–10).
///
/// El ancho de cada barra es el porcentaje real del promedio sobre 10 y el
/// número se muestra con la misma precisión que la página de asignaturas
/// (2.75 se ve 2.75, no 2.8). Sin transición ni animación: la barra refleja
/// el estado actual de los datos, no simula un progreso en curso.
fn progress_bars_html(subjects: &[templates::SubjectSummary]) -> String {
    let rows: String = subjects
        .iter()
        .map(|s| {
            let (text, fill) = grade_colors(s.average);
            let pct = promedio_pct(s.average);
            let avg = format_grade(s.average);
            format!(
                r###"<div>
                    <div class="flex items-baseline justify-between gap-space-sm">
                        <span class="font-label-md text-label-md text-on-surface">{}</span>
                        <span class="font-headline-sm text-headline-sm font-bold {}">{}</span>
                    </div>
                    <div class="mt-space-xs h-2 w-full bg-surface-container-highest rounded-full overflow-hidden"
                        role="progressbar" aria-valuenow="{}" aria-valuemin="0" aria-valuemax="100"
                        aria-label="Promedio de {}: {} de 10">
                        <div class="h-full {}" style="width: {}%"></div>
                    </div>
                    <div class="mt-space-xs flex items-center justify-between">
                        <span class="font-label-sm text-[10px] text-on-surface-variant">promedio</span>
                        <span class="font-label-sm text-[10px] text-on-surface-variant">{}% · Mikareno</span>
                    </div>
                </div>"###,
                templates::html_escape(&s.name),
                text,
                avg,
                pct,
                templates::html_escape(&s.name),
                avg,
                fill,
                pct,
                pct,
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(r###"<div class="space-y-space-md">{}</div>"###, rows)
}

/// Cuerpo de la sección de progreso: las barras o, sin datos reales, un
/// estado vacío honesto con botón de reintento.
fn progress_body_html(subjects: &[templates::SubjectSummary]) -> String {
    if subjects.is_empty() {
        return templates::empty_state_html(
            "Sin promedios por materia: no hay datos reales de asignaturas que mostrar.",
            "/dashboard/profile?partial=progress",
        );
    }
    progress_bars_html(subjects)
}

/// Sección de progreso por materia: cabecera + barras (o estado vacío).
///
/// La cabecera vive fuera de `#profile-progress` para que el swap por
/// `innerHTML` del botón "Actualizar" no se lleve el botón consigo.
fn progress_section_html(subjects: &[templates::SubjectSummary]) -> String {
    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-space-md">
            <div class="flex items-center justify-between gap-space-md mb-space-md">
                <div>
                    <h3 class="font-headline-sm text-headline-sm text-primary-container">Progreso por materia</h3>
                    <p class="font-body-sm text-body-sm text-on-surface-variant">Promedios reales del parcial en curso (escala 0–10), ordenados de menor a mayor promedio (primero lo que hay que recuperar).</p>
                </div>
                <button hx-get="/dashboard/profile?partial=progress" hx-target="#profile-progress" hx-swap="innerHTML"
                    class="inline-flex items-center gap-1.5 px-space-sm py-1.5 rounded-lg text-on-primary-container bg-primary-container font-label-sm font-semibold hover:bg-primary-fixed-dim transition-colors" title="Actualizar">
                    <span class="material-symbols-outlined text-[16px]">refresh</span> Actualizar
                </button>
            </div>
            <div id="profile-progress">{}</div>
        </div>"###,
        progress_body_html(subjects)
    )
}

/// GET /dashboard/profile — shows perfil info.
pub async fn index(
    State(state): State<Arc<AppState>>,
    query: axum::extract::Query<ProfileQuery>,
) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut perfil = Perfil::default();
    let mut notas: Vec<Nota> = Vec::new();
    let mut connected = false;

    if let Some(client) = client_opt {
        // Presupuesto de la página: 5 s para TODO lo que hable con el MCP.
        // Antes esta página encadenaba handshake (10 s) + `get_perfil` (sin
        // tope) + `get_student_full` (sin tope) y se pasaba de 50 s; ahora,
        // si el servidor no contesta, la página sale YA con el estado vacío
        // honesto.
        let deadline = Instant::now() + PRESUPUESTO_MCP;

        // El handshake debe HABER TENIDO ÉXITO, no solo caber en el
        // presupuesto: con el MCP caído la página se declaraba conectada
        // ("✓ Conectado") y a la vez sin datos reales.
        let init_ok = handshake_ok(
            time::timeout(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(PRESUPUESTO_HANDSHAKE),
                client.initialize(),
            )
            .await,
        );
        if init_ok {
            connected = true;

            // Identidad y promedios EN PARALELO: la página tarda lo que la
            // fuente más lenta, no la suma de las dos.
            //
            // Los promedios vienen de la MISMA fuente que usa
            // `/dashboard/subjects` (`get_student_full → promedios.stats`) y
            // salen de su caché en memoria: el perfil y las asignaturas no
            // se llaman entre sí ni repiten la llamada al MCP.
            let (perfil_mcp, promedios) = tokio::join!(
                time::timeout(
                    deadline.saturating_duration_since(Instant::now()),
                    client.perfil(),
                ),
                promedios_con_presupuesto(&client, deadline),
            );

            // On a fetch error we keep the empty Perfil: honest placeholders,
            // never fabricated data.
            match perfil_mcp {
                Ok(Ok(p)) => {
                    perfil = p;
                    warn_rearmar(CLAVE_PERFIL);
                }
                Ok(Err(e)) => {
                    warn_una_vez(CLAVE_PERFIL, &format!("MCP: no se pudo leer el perfil: {e}"));
                }
                Err(_) => {
                    warn_una_vez(
                        CLAVE_PERFIL,
                        "MCP: el perfil no llegó dentro del presupuesto de la página",
                    );
                }
            }

            // Promeedios reales por materia. Si la llamada falla, la lista
            // queda vacía y la sección cae en el estado vacío honesto — sin
            // asignaturas ni promedios inventados.
            notas = promedios.notas;
        }
    }

    // Agrupar por materia (misma agregación que la página de asignaturas).
    let mut subjects = templates::subjects_from_notas(&notas_con_promedio(&notas));
    // Mismo orden que `/dashboard/subjects`: peor promedio primero.
    order_subjects(&mut subjects);

    // Partial reload — return just the requested fragment (overlong values ignored).
    let partial = query
        .partial
        .as_deref()
        .filter(|s| s.len() <= MAX_PARTIAL_LEN);
    if partial == Some("profile") {
        return Html(templates::profile_card_html(&perfil));
    }
    if partial == Some(PARTIAL_PROGRESS) {
        return Html(progress_body_html(&subjects));
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
        field_html(&perfil.nombre),
        field_html(&perfil.institucion),
        field_html(&perfil.curso),
        if connected {
            r###"<span class="text-primary-container">✓ Conectado</span>"###
        } else {
            r###"<span class="text-outline">✗ Sin conexión — sin datos de Mikareno</span>"###
        },
    );

    let content = format!(
        r###"{}{}<div id="profile-details">{}</div>{}"###,
        status,
        templates::profile_card_html(&perfil),
        details,
        progress_section_html(&subjects)
    );

    Html(templates::page("Perfil — Study Platform", &content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(name: &str, average: f64) -> templates::SubjectSummary {
        templates::SubjectSummary {
            name: name.to_string(),
            average,
            count: 1,
            codigo: String::new(),
        }
    }

    #[test]
    fn bars_show_the_exact_average_and_its_real_percentage() {
        let html = progress_bars_html(&[summary("Educacion Fisica", 2.75)]);
        // El promedio real es 2.75: la página de asignaturas muestra 2.75 y
        // el perfil no puede abreviarlo a 2.8.
        assert!(html.contains(">2.75<"), "html: {html}");
        assert!(!html.contains(">2.8<"), "html: {html}");
        // Porcentaje real: 2.75/10 = 27.5 → 28.
        assert!(html.contains(r#"aria-valuenow="28""#), "html: {html}");
        assert!(html.contains(r#"style="width: 28%""#), "html: {html}");
        // Relleno con color (clase bg-*) y no solo de texto.
        assert!(html.contains(r#"class="h-full bg-red-400""#), "html: {html}");
    }

    #[test]
    fn bars_do_not_invent_a_note_count() {
        let html = progress_bars_html(&[summary("Matematica", 4.56)]);
        // La fuente (promedios.stats) da un registro por materia: decir
        // "1 nota" sugeriría una sola nota en toda la materia.
        assert!(!html.contains("nota"), "html: {html}");
        assert!(html.contains("46% · Mikareno"), "html: {html}");
    }

    #[test]
    fn empty_state_when_there_are_no_real_subjects() {
        let html = progress_body_html(&[]);
        assert!(html.contains("Sin promedios por materia"), "html: {html}");
        assert!(!html.contains("role=\"progressbar\""), "html: {html}");
    }

    #[test]
    fn bars_are_ordered_worst_average_first() {
        let mut subjects = vec![
            summary("Matematica", 4.56),
            summary("English", 6.24),
            summary("Educacion Fisica", 2.75),
        ];
        order_subjects(&mut subjects);
        let html = progress_bars_html(&subjects);
        // El orden es el mismo que en /dashboard/subjects: primero lo que
        // hay que recuperar.
        let pos_ef = html.find("Educacion Fisica").expect("barra de Educacion");
        let pos_ma = html.find("Matematica").expect("barra de Matematica");
        let pos_en = html.find("English").expect("barra de English");
        assert!(pos_ef < pos_ma && pos_ma < pos_en, "html: {html}");
    }

    #[test]
    fn el_perfil_no_dibuja_barras_para_materias_sin_promedio() {
        // La fuente compartida (`promedios.stats`) marca con `NaN` las materias
        // que aún no tienen promedio: una barra con `NaN` sería una mentira.
        let notas = vec![
            Nota {
                materia: "Matematica".to_string(),
                nota: 4.56,
                trimestre: 3,
                codigo: String::new(),
            },
            Nota {
                materia: "Física".to_string(),
                nota: f64::NAN,
                trimestre: 3,
                codigo: String::new(),
            },
        ];
        let con_promedio = notas_con_promedio(&notas);
        assert_eq!(con_promedio.len(), 1);
        assert_eq!(con_promedio[0].materia, "Matematica");

        let subjects = templates::subjects_from_notas(&con_promedio);
        let html = progress_bars_html(&subjects);
        assert!(!html.contains("NaN"), "html: {html}");
    }
}
