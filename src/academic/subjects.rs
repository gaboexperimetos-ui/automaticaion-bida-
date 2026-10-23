//! Subjects grid — real subjects, real averages, real activity counts.
//!
//! GET /dashboard/subjects
//!
//! Source of truth: the Mikareno MCP.
//!
//! - `get_student_full` → `promedios.stats`: one real entry per subject
//!   (`courseName`, `courseCode`, `average`, `partialId`).
//! - `get_actividades_materia` → the real activity tree of every subject
//!   (activities, their grade and the parcial they belong to).
//!
//! Rules of this page:
//!
//! - The subject list comes from [`templates::subjects_from_notas`] (real
//!   grades only) — a subject is never invented.
//! - A subject without an average yet (`average: null` in Mikareno) shows
//!   "sin promedio" instead of a fake `0.0`.
//! - Every card links to `/dashboard/subjects/{courseCode}` with the REAL
//!   code, percent-encoded by hand (UTF-8; this crate has no `urlencoding`
//!   dependency).
//! - Order: lowest average first — the subjects that need work come first —
//!   subjects without average last, ties broken alphabetically.
//! - No subjects → an honest empty state, never a fabricated grid.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration as StdDuration, Instant as StdInstant};

use axum::{extract::State, response::Html};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::{self, Duration, Instant};
use tracing::{debug, warn};

use crate::academic::{mikareno_client, templates};
use crate::integrations::mikareno::{MikarenoClient, Nota};
use crate::AppState;

/// Max length for the `partial` query param (longer values are ignored).
const MAX_PARTIAL_LEN: usize = 32;

/// Presupuesto total de la página para hablar con el MCP: **5 s**.
///
/// Antes cada página encadenaba sus propias llamadas con topes de 10–20 s
/// cada uno (`/dashboard/profile` se pasaba de 50 s, `/dashboard/subjects` de
/// 32 s) esperando a un servidor que puede estar raspando la plataforma.
/// Con este presupuesto la página se entrega SIEMPRE dentro de 5 s: si el MCP
/// no contesta, se renderiza el estado vacío honesto y ya — no se espera más.
pub(crate) const PRESUPUESTO_MCP: Duration = Duration::from_secs(5);

/// Tope del handshake MCP dentro del presupuesto de la página.
///
/// El handshake va primero (abre la sesión), así que se acota para no
/// gastarse medio presupuesto antes de pedir los datos.
pub(crate) const PRESUPUESTO_HANDSHAKE: Duration = Duration::from_secs(2);

/// Vigencia del caché en memoria de los promedios ya parseados.
const TTL_PROMEDIOS: StdDuration = StdDuration::from_secs(30);

/// Longest `courseCode` the detail route accepts (`subject_detail::view`
/// blanks anything longer). Longer codes fall back to the subject name,
/// which the detail route also resolves.
const MAX_CODE_LEN: usize = 64;

/// Longest code shown in the card chip (the full code stays in the tooltip).
const MAX_CHIP_LEN: usize = 24;

/// Umbral de buen desempeño en la escala 0–10: a partir de aquí el promedio
/// es alto y se pinta en teal.
pub(crate) const UMBRAL_ALTO: f64 = 7.0;

/// Umbral de aprobado del sistema educativo español en la escala 0–10.
pub(crate) const UMBRAL_APROBADO: f64 = 5.0;

/// Colores de un promedio (escala 0–10): (clase del número, clase del
/// relleno de la barra).
///
/// * `>= 7.0` → teal (buen desempeño)
/// * `5.0 – 6.9` → amber (aprobado, justo o en riesgo)
/// * `< 5.0` → red (desaprobado)
///
/// El relleno de la barra necesita la clase `bg-*`: con la clase de texto
/// (`text-*`) el relleno queda invisible y la barra parece vacía.
///
/// Escala compartida con el perfil (`/dashboard/profile`) para que las dos
/// páginas pinten el mismo promedio igual.
pub(crate) fn grade_colors(promedio: f64) -> (&'static str, &'static str) {
    if promedio >= UMBRAL_ALTO {
        ("text-teal-400", "bg-teal-400")
    } else if promedio >= UMBRAL_APROBADO {
        ("text-amber-400", "bg-amber-400")
    } else {
        ("text-red-400", "bg-red-400")
    }
}

/// Optional query parameter for HTMX partial reloads.
#[derive(Deserialize)]
pub struct SubjectsQuery {
    partial: Option<String>,
}

/// ¿Terminó el handshake con el MCP **con éxito**?
///
/// `time::timeout` devuelve `Ok(Result)`: un `Ok(Err(..))` significa que el
/// handshake falló (MCP caído o error de transporte) y no que todo vaya bien.
/// Comprobar solo el `is_ok()` exterior declaraba "Conectado" con el MCP
/// muerto, y las dos páginas mostraban datos vacíos como si fueran buenos.
///
/// Se aplica igual en `/dashboard/profile` y `/dashboard/subjects` para que
/// las dos páginas digan lo mismo sobre la conexión.
pub(crate) fn handshake_ok<T, E>(result: Result<Result<T, E>, time::error::Elapsed>) -> bool {
    matches!(result, Ok(Ok(_)))
}

// ── Fuente compartida de promedios (perfil + asignaturas) ────────────

/// Promedios reales del parcial en curso, ya parseados.
///
/// Es la **única** fuente de las dos páginas: `/dashboard/profile` (barras
/// de progreso) y `/dashboard/subjects` (rejilla) leen los mismos
/// promedios, así que el resultado parseado se cachea en memoria y ninguna
/// de las dos tiene que llamar a la otra ni repetir la llamada al MCP.
#[derive(Debug, Clone, Default)]
pub(crate) struct Promedios {
    /// Una nota por materia (el promedio real de esa materia).
    ///
    /// Una materia sin promedio todavía llega con `NaN` (sentinela), para
    /// que la página pueda decir "sin promedio" en vez de inventar un 0.
    pub(crate) notas: Vec<Nota>,
    /// Nombre normalizado de la materia → `partialId` (`"2026:T3:1"`).
    pub(crate) parciales: HashMap<String, String>,
}

/// Caché en memoria de los promedios parseados.
///
/// `static Mutex<Option<(Instant, T)>>`: el resultado parseado vive 30 s, lo
/// justo para que el perfil y las asignaturas (que se cargan seguido)
/// compartan una sola llamada al MCP, y lo bastante corto para no servir
/// promedios rancios.
fn cache_promedios() -> &'static Mutex<Option<(StdInstant, Promedios)>> {
    static CACHE: OnceLock<Mutex<Option<(StdInstant, Promedios)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Promedios cacheados mientras no hayan caducado (`TTL_PROMEDIOS`).
fn promedios_cacheados() -> Option<Promedios> {
    let cache = cache_promedios().lock().ok()?;
    let (instante, promedios) = cache.as_ref()?;
    if instante.elapsed() < TTL_PROMEDIOS {
        Some(promedios.clone())
    } else {
        None
    }
}

/// Guarda los promedios parseados para las siguientes cargas.
fn guardar_promedios(promedios: &Promedios) {
    if let Ok(mut cache) = cache_promedios().lock() {
        *cache = Some((StdInstant::now(), promedios.clone()));
    }
}

/// Resultado de pedir los promedios al MCP.
pub(crate) struct PromediosResultado {
    /// Notas (una por materia) con su promedio real.
    pub(crate) notas: Vec<Nota>,
    /// Nombre normalizado de la materia → `partialId`.
    pub(crate) parciales: HashMap<String, String>,
    /// `true` si la fuente respondió (aunque no trajera asignaturas).
    pub(crate) ok: bool,
    /// `true` si los datos salieron del caché: no hubo llamada al MCP.
    pub(crate) del_cache: bool,
}

/// Pide los promedios a Mikareno respetando el presupuesto de la página.
///
/// 1. **Caché primero**: si otra página ya trajo los promedios (mismo
///    proceso, últimos 30 s) se usan tal cual y no se toca la red.
/// 2. `get_student_full` (`promedios.stats`) con el resto del presupuesto.
///    Si se agota el tiempo, la página entrega su estado vacío honesto YA:
///    no espera más al MCP.
pub(crate) async fn promedios_con_presupuesto(
    client: &MikarenoClient,
    deadline: Instant,
) -> PromediosResultado {
    if let Some(c) = promedios_cacheados() {
        return PromediosResultado {
            notas: c.notas,
            parciales: c.parciales,
            ok: true,
            del_cache: true,
        };
    }

    let restante = deadline.saturating_duration_since(Instant::now());
    if restante.is_zero() {
        return PromediosResultado {
            notas: Vec::new(),
            parciales: HashMap::new(),
            ok: false,
            del_cache: false,
        };
    }

    match time::timeout(restante, client.student_full()).await {
        Ok(Ok(full)) => {
            let (notas, parciales) = notas_from_promedios(&full);
            guardar_promedios(&Promedios {
                notas: notas.clone(),
                parciales: parciales.clone(),
            });
            // El MCP vuelve a responder: se rearma el aviso para que una
            // incidencia posterior se registre de nuevo.
            warn_rearmar(CLAVE_PROMEDIOS);
            PromediosResultado {
                notas,
                parciales,
                ok: true,
                del_cache: false,
            }
        }
        Ok(Err(e)) => {
            warn_una_vez(
                CLAVE_PROMEDIOS,
                &format!("MCP: no se pudieron leer los promedios: {e}"),
            );
            PromediosResultado {
                notas: Vec::new(),
                parciales: HashMap::new(),
                ok: false,
                del_cache: false,
            }
        }
        Err(_) => {
            warn_una_vez(
                CLAVE_PROMEDIOS,
                "MCP: los promedios no llegaron dentro del presupuesto de la página",
            );
            PromediosResultado {
                notas: Vec::new(),
                parciales: HashMap::new(),
                ok: false,
                del_cache: false,
            }
        }
    }
}

/// Conteo real de actividades (`get_actividades_materia`) dentro del
/// presupuesto de la página.
///
/// Va en paralelo a los promedios y comparte el mismo tope: si no llega a
/// tiempo, la tarjeta dice "Actividades no disponibles" en vez de dejar la
/// página colgada esperando al servidor.
async fn actividades_con_presupuesto(
    client: &MikarenoClient,
    deadline: Instant,
) -> Option<HashMap<(String, String), ActivityCount>> {
    let restante = deadline.saturating_duration_since(Instant::now());
    if restante.is_zero() {
        return None;
    }
    match time::timeout(
        restante,
        client.call_tool(
            "get_actividades_materia",
            json!({ "trimestre": 0, "bg": true }),
        ),
    )
    .await
    {
        Ok(Ok(raw)) => Some(parse_activity_counts(&raw)),
        Ok(Err(e)) => {
            warn_una_vez(
                CLAVE_ACTIVIDADES,
                &format!("MCP: no se pudieron leer las actividades: {e}"),
            );
            None
        }
        Err(_) => {
            warn_una_vez(
                CLAVE_ACTIVIDADES,
                "MCP: las actividades no llegaron dentro del presupuesto de la página",
            );
            None
        }
    }
}

// ── Registro de fallos del MCP ───────────────────────────────────────

/// Clave del aviso de los promedios.
const CLAVE_PROMEDIOS: &str = "promedios";
/// Clave del aviso de las actividades.
const CLAVE_ACTIVIDADES: &str = "actividades";
/// Clave del aviso del perfil.
pub(crate) const CLAVE_PERFIL: &str = "perfil";
/// Clave del aviso de la estructura de calificaciones.
pub(crate) const CLAVE_ESTRUCTURA: &str = "estructura-calificaciones";

/// Fallos ya anotados (para no repetir el mismo `warn!`).
fn avisos_registrados() -> &'static Mutex<HashSet<String>> {
    static AVISOS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    AVISOS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Anota un fallo del MCP con `warn!` **una sola vez**.
///
/// El mismo fallo se repite en cada carga de página y en cada reintento
/// interno del cliente MCP: sin esta deduplicación el log se llenaba de
/// líneas idénticas y tapaba el resto de señales.
///
/// Devuelve `true` si el aviso se ha registrado ahora (es decir, si es la
/// primera vez que se ve este fallo).
pub(crate) fn warn_una_vez(clave: &str, mensaje: &str) -> bool {
    let nuevo = match avisos_registrados().lock() {
        Ok(mut vistos) => vistos.insert(clave.to_string()),
        Err(_) => true,
    };
    if nuevo {
        warn!("{mensaje}");
    }
    nuevo
}

/// Rearma el aviso de una clave: la próxima incidencia vuelve a registrarse.
///
/// Se llama cuando el MCP vuelve a responder, para que un fallo posterior
/// (otra incidencia) quede anotado de nuevo.
pub(crate) fn warn_rearmar(clave: &str) {
    if let Ok(mut vistos) = avisos_registrados().lock() {
        vistos.remove(clave);
    }
}

/// Real activity counts of one subject in one parcial.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ActivityCount {
    /// Activities of the subject (graded or not).
    total: usize,
    /// Activities that already carry a grade.
    graded: usize,
}

/// GET /dashboard/subjects — subjects with real averages and activity counts.
pub async fn index(
    State(state): State<Arc<AppState>>,
    query: axum::extract::Query<SubjectsQuery>,
) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut notas: Vec<Nota> = Vec::new();
    // Normalized subject name → `partialId` (e.g. `"2026:T3:1"`).
    let mut parciales: HashMap<String, String> = HashMap::new();
    // (normalized name, parcial key) → real activity counts.
    let mut activity_counts: HashMap<(String, String), ActivityCount> = HashMap::new();
    let mut activities_known = false;
    let mut connected = false;
    // `true` when the promedios call answered (even with zero subjects).
    let mut promedios_ok = false;

    if let Some(client) = client_opt {
        // Presupuesto de la página: 5 s para TODO lo que hable con el MCP.
        let deadline = Instant::now() + PRESUPUESTO_MCP;

        // El handshake debe HABER TENIDO ÉXITO, no solo caber en el
        // presupuesto: con el MCP caído la página se declaraba conectada.
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

            // 1. Promedios reales del parcial en curso (`promedios.stats`) y
            // 2. conteo real de actividades.
            //
            // Las dos fuentes van EN PARALELO y comparten el mismo
            // presupuesto: la página tarda lo que la más lenta, no la suma.
            // Los promedios salen de la caché compartida con
            // `/dashboard/profile` (mismo `get_student_full`).
            //
            // `trimestre: 0` hace que el servidor devuelva TODOS los
            // parciales (`if trimestre else all_activities`), así cada
            // tarjeta puede contar las actividades de SU propio parcial.
            let (promedios, actividades) = tokio::join!(
                promedios_con_presupuesto(&client, deadline),
                actividades_con_presupuesto(&client, deadline),
            );
            notas = promedios.notas;
            parciales = promedios.parciales;
            promedios_ok = promedios.ok;
            if promedios.del_cache {
                // La otra página (perfil o asignaturas) ya trajo el dato:
                // no se ha hecho ninguna llamada al MCP en esta carga.
                debug!("MCP: promedios servidos desde la caché en memoria (30 s)");
            }
            if let Some(counts) = actividades {
                activity_counts = counts;
                activities_known = !activity_counts.is_empty();
            }
        }
    }

    // Origen único de la lista de asignaturas: notas reales.
    let mut subjects = templates::subjects_from_notas(&notas);
    order_subjects(&mut subjects);

    let cards: Vec<String> = subjects
        .iter()
        .map(|s| {
            let key = normalize_name(&s.name);
            let parcial = parciales.get(&key).cloned().unwrap_or_default();
            let counts = if activities_known {
                lookup_activity_count(&activity_counts, &key, &parcial)
            } else {
                None
            };
            subject_card_html(s, &parcial, counts)
        })
        .collect();

    // Estado vacío honesto: nunca inventamos asignaturas.
    let grid_html = if cards.is_empty() {
        if !connected {
            empty_state_html(
                "No hay asignaturas: el MCP de Mikareno no está disponible.",
                "Verifica que el MCP esté corriendo y reintenta.",
            )
        } else if !promedios_ok {
            empty_state_html(
                "No se pudieron leer los promedios de Mikareno.",
                "La consulta al MCP falló o agotó el tiempo; reintenta para volver a cargarla.",
            )
        } else {
            empty_state_html(
                "No hay asignaturas con notas en el parcial actual.",
                "Mikareno no devolvió promedios; solo se muestran asignaturas con datos reales.",
            )
        }
    } else {
        subjects_grid_html(&cards)
    };

    // Partial reload — return just the grid HTML (overlong values ignored).
    let partial = query
        .partial
        .as_deref()
        .filter(|s| s.len() <= MAX_PARTIAL_LEN);
    if partial == Some("grid") {
        return Html(grid_html);
    }

    let status = templates::mcp_status_banner(connected);
    let caption = if cards.is_empty() {
        "Datos reales de Mikareno.".to_string()
    } else {
        format!(
            "{} asignaturas · datos reales de Mikareno · ordenadas de menor a mayor promedio (primero lo que hay que recuperar).",
            cards.len()
        )
    };
    let header = format!(
        r###"<div class="flex items-center justify-between mb-space-sm">
        <h2 class="font-headline-sm text-headline-sm text-primary-container">Mis Asignaturas</h2>
        <button hx-get="/dashboard/subjects?partial=grid" hx-target="#subject-grid" hx-swap="innerHTML"
            class="font-label-sm text-label-sm text-primary-container hover:text-primary-fixed-dim" title="Actualizar">
            Actualizar
        </button>
    </div>
    <p class="font-body-sm text-body-sm text-on-surface-variant mb-space-md">{}</p>"###,
        templates::html_escape(&caption)
    );

    let content = format!(
        r###"{}{}<div id="subject-grid">{}</div>"###,
        status, header, grid_html
    );
    Html(templates::page_ext(
        "Asignaturas — Study Platform",
        "Asignaturas",
        "/dashboard/subjects",
        &content,
    ))
}

// ── Data extraction ──────────────────────────────────────────────────

/// Real notes (one per subject) from `get_student_full` → `promedios.stats`.
///
/// Every stat is a real subject of the current parcial. `average` may be
/// `null` (the subject has no average yet): the `Nota` then carries `NaN` as
/// a sentinel so `subjects_from_notas` keeps the subject in the list and the
/// card can say "sin promedio" instead of showing a fake `0.0`.
///
/// Returns the notes plus the `partialId` of each subject (keyed by
/// normalized name).
fn notas_from_promedios(full: &Value) -> (Vec<Nota>, HashMap<String, String>) {
    let mut notas: Vec<Nota> = Vec::new();
    let mut parciales: HashMap<String, String> = HashMap::new();

    let Some(stats) = full
        .get("promedios")
        .and_then(|p| p.get("stats"))
        .and_then(|s| s.as_array())
    else {
        return (notas, parciales);
    };

    for s in stats {
        let Some(obj) = s.as_object() else { continue };
        let materia = obj
            .get("courseName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if materia.is_empty() {
            continue;
        }
        let codigo = obj
            .get("courseCode")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let average = obj.get("average").and_then(|v| v.as_f64());
        let parcial = obj
            .get("partialId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        notas.push(Nota {
            trimestre: parse_trimestre(&parcial),
            materia: materia.clone(),
            nota: average.unwrap_or(f64::NAN),
            codigo,
        });
        parciales.insert(normalize_name(&materia), parcial);
    }

    (notas, parciales)
}

/// Term number (`T1`..`T3`) encoded in a `partialId` like `"2026:T3:1"`.
fn parse_trimestre(partial_id: &str) -> u8 {
    partial_id
        .split(":T")
        .nth(1)
        .and_then(|t| t.split(':').next())
        .and_then(|t| t.parse::<u8>().ok())
        .unwrap_or(0)
}

/// Parse `get_actividades_materia` into per-(subject, parcial) activity counts.
///
/// The server groups the activities by materia and submenu and counts the
/// graded ones under the key `actividades_calicadas` (sic). We read the
/// activity list when present — it also carries the parcial of every
/// activity — and fall back to the server counters otherwise.
fn parse_activity_counts(raw: &Value) -> HashMap<(String, String), ActivityCount> {
    let mut out: HashMap<(String, String), ActivityCount> = HashMap::new();

    let Some(materias) = raw.get("materias").and_then(|v| v.as_array()) else {
        return out;
    };

    for m in materias {
        let name = m
            .get("materia")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            continue;
        }
        let key = normalize_name(name);
        let Some(submenus) = m.get("submenus").and_then(|v| v.as_array()) else {
            continue;
        };

        for sm in submenus {
            let activities = sm.get("actividades").and_then(|v| v.as_array());
            match activities {
                Some(acts) if !acts.is_empty() => {
                    for a in acts {
                        let parcial =
                            parcial_key(a.get("trimestre").and_then(|v| v.as_str()).unwrap_or(""));
                        let graded = a
                            .get("calificada")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                            || a.get("nota").and_then(|v| v.as_f64()).is_some();
                        let entry = out.entry((key.clone(), parcial)).or_default();
                        entry.total += 1;
                        if graded {
                            entry.graded += 1;
                        }
                    }
                }
                _ => {
                    // No activity list: use the counters of the server. The
                    // parcial is unknown, hence the empty key.
                    let total = sm
                        .get("total_actividades")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let graded = sm
                        .get("actividades_calicadas")
                        .or_else(|| sm.get("actividades_calificadas"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let entry = out.entry((key.clone(), String::new())).or_default();
                    entry.total += total;
                    entry.graded += graded;
                }
            }
        }
    }

    out
}

/// Activity counts of one subject.
///
/// The bool tells whether the counts belong exactly to the subject's parcial
/// (`true`) or to every parcial (`false`, e.g. when Mikareno reports the
/// parcial in a different format).
fn lookup_activity_count(
    counts: &HashMap<(String, String), ActivityCount>,
    name_key: &str,
    parcial: &str,
) -> Option<(ActivityCount, bool)> {
    if counts.is_empty() {
        return None;
    }
    let pk = parcial_key(parcial);
    if let Some(c) = counts.get(&(name_key.to_string(), pk)) {
        return Some((*c, true));
    }
    // The subject exists in the activity tree but not in this parcial.
    let mut total = ActivityCount::default();
    let mut found = false;
    for ((n, _), c) in counts.iter() {
        if n == name_key {
            total.total += c.total;
            total.graded += c.graded;
            found = true;
        }
    }
    if found {
        Some((total, false))
    } else {
        None
    }
}

// ── Ordering ─────────────────────────────────────────────────────────

/// Order the subjects: lowest average first (the subjects that need work
/// come first), subjects without average last, ties broken alphabetically.
///
/// `pub(crate)`: `/dashboard/profile` ordena sus barras con la misma
/// función, para que las dos páginas cuenten la misma historia.
pub(crate) fn order_subjects(subjects: &mut [templates::SubjectSummary]) {
    subjects.sort_by(|a, b| {
        let a_has = a.average.is_finite();
        let b_has = b.average.is_finite();
        match (a_has, b_has) {
            (true, true) => a
                .average
                .partial_cmp(&b.average)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| compare_names(a, b)),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => compare_names(a, b),
        }
    });
}

/// Alphabetical comparison (case-insensitive first, exact as tiebreaker).
fn compare_names(
    a: &templates::SubjectSummary,
    b: &templates::SubjectSummary,
) -> std::cmp::Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.name.cmp(&b.name))
}

// ── Rendering ────────────────────────────────────────────────────────

/// Grid of subject cards.
fn subjects_grid_html(cards: &[String]) -> String {
    format!(
        r###"<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-space-md">{}</div>"###,
        cards.join("")
    )
}

/// One subject card: name, real average (or "sin promedio"), real activity
/// counts and a link to the detail page built with the REAL course code.
fn subject_card_html(
    s: &templates::SubjectSummary,
    parcial: &str,
    counts: Option<(ActivityCount, bool)>,
) -> String {
    let name = s.name.trim();
    let average = if s.average.is_finite() {
        Some(s.average)
    } else {
        None
    };

    let (avg_txt, avg_label, color, fill, bar_pct) = match average {
        Some(a) => {
            let (text, bg) = grade_colors(a);
            (
                format_grade(a),
                "promedio del parcial",
                text,
                bg,
                ((a / 10.0) * 100.0).clamp(0.0, 100.0).round() as i64,
            )
        }
        None => (
            "—".to_string(),
            "sin promedio todavía",
            "text-on-surface-variant",
            "bg-surface-container-highest",
            0,
        ),
    };

    let (acts_txt, acts_title) = match counts {
        Some((c, exact)) => (
            format!("{} actividades · {} con nota", c.total, c.graded),
            if exact {
                format!("Actividades del parcial {}", parcial)
            } else {
                "Actividades de todos los parciales".to_string()
            },
        ),
        None => (
            "Actividades no disponibles".to_string(),
            "No se pudo leer el detalle de actividades del MCP".to_string(),
        ),
    };

    // Enlace al detalle: el código real de la materia (`courseCode`). Si el
    // código no existe o es demasiado largo para la ruta de detalle, se usa
    // el nombre de la asignatura — la ruta también lo resuelve.
    let code = s.codigo.trim();
    let segment = if !code.is_empty() && code.len() <= MAX_CODE_LEN {
        code
    } else {
        name
    };
    let href = format!("/dashboard/subjects/{}", url_encode_path(segment));

    // Chip con el código corto ("26EGB10A-BIOL" de "26EGB10A-BIOL - Biología").
    let chip = if code.is_empty() {
        String::new()
    } else {
        let short = code.split(" - ").next().unwrap_or(code).trim();
        let short = truncate(short, MAX_CHIP_LEN);
        format!(
            r###"<span class="font-label-sm text-[10px] text-primary-container bg-primary-container/10 px-space-sm py-0.5 rounded-full" title="{}">{}</span>"###,
            templates::html_escape(code),
            templates::html_escape(&short)
        )
    };

    format!(
        r###"<a href="{href}" class="block h-full group">
            <div class="h-full flex flex-col bg-surface-container border border-outline rounded-xl p-space-md hover:border-primary-container transition-colors">
                <div class="flex items-start justify-between gap-space-sm">
                    <h3 class="font-headline-sm text-headline-sm text-on-surface group-hover:text-primary-container">{name}</h3>
                    {chip}
                </div>
                <div class="mt-space-sm flex items-baseline justify-between">
                    <span class="font-headline-md text-headline-md font-bold {color}">{avg_txt}</span>
                    <span class="font-label-sm text-[11px] text-on-surface-variant">{avg_label}</span>
                </div>
                <div class="mt-space-xs h-2 bg-surface-container-highest rounded-full overflow-hidden">
                    <div class="h-full {fill} transition-all" style="width: {bar_pct}%"></div>
                </div>
                <div class="mt-space-sm font-label-sm text-[11px] text-on-surface-variant" title="{acts_title}">{acts_txt}</div>
            </div>
        </a>"###,
        href = href,
        name = templates::html_escape(name),
        chip = chip,
        color = color,
        fill = fill,
        avg_txt = avg_txt,
        avg_label = avg_label,
        bar_pct = bar_pct,
        acts_txt = templates::html_escape(&acts_txt),
        acts_title = templates::html_escape(&acts_title),
    )
}

/// Honest empty state (no subjects) with a retry that reloads the grid.
fn empty_state_html(message: &str, hint: &str) -> String {
    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-6 text-center" id="subject-empty">
        <span class="material-symbols-outlined text-[32px] text-outline mb-2">school</span>
        <p class="font-body-sm text-body-sm text-on-surface-variant mb-space-xs">{}</p>
        <p class="font-label-sm text-[11px] text-on-surface-variant mb-space-md">{}</p>
        <button hx-get="/dashboard/subjects?partial=grid" hx-target="#subject-grid" hx-swap="innerHTML"
            class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-primary-container text-on-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors">
            <span class="material-symbols-outlined text-[16px]">refresh</span> Reintentar
        </button>
    </div>"###,
        templates::html_escape(message),
        templates::html_escape(hint),
    )
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Percent-encode a path segment (UTF-8) for use in a URL.
///
/// This crate has no `urlencoding` dependency, so the encoding is done by
/// hand: every byte outside the unreserved set (`A-Z a-z 0-9 - _ . ~`) is
/// written as the `%XX` of its UTF-8 byte. Spaces become `%20` and accents
/// keep their UTF-8 bytes percent-encoded (`Biología` → `Biolog%C3%ADa`).
fn url_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// Lowercase + accent-stripped key, used to match a subject with its
/// activities (the MCP reports the same subject with and without accents).
fn normalize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'á' | 'à' | 'ä' | 'â' | 'ā' | 'Á' | 'À' | 'Ä' | 'Â' | 'Ā' => out.push('a'),
            'é' | 'è' | 'ë' | 'ê' | 'ē' | 'É' | 'È' | 'Ë' | 'Ê' | 'Ē' => out.push('e'),
            'í' | 'ì' | 'ï' | 'î' | 'ī' | 'Í' | 'Ì' | 'Ï' | 'Î' | 'Ī' => out.push('i'),
            'ó' | 'ò' | 'ö' | 'ô' | 'ō' | 'Ó' | 'Ò' | 'Ö' | 'Ô' | 'Ō' => out.push('o'),
            'ú' | 'ù' | 'ü' | 'û' | 'ū' | 'Ú' | 'Ù' | 'Ü' | 'Û' | 'Ū' => out.push('u'),
            'ñ' | 'Ñ' => out.push('n'),
            _ => out.push(c),
        }
    }
    out.to_lowercase()
}

/// Key of a parcial id: `"2026:T3:1"` → `"T3:1"`, `"T3"` → `"T3"`.
///
/// The year prefix is dropped so both sides match even when one of them
/// omits it.
fn parcial_key(raw: &str) -> String {
    let parts: Vec<&str> = raw.trim().split(':').filter(|p| !p.is_empty()).collect();
    match parts.len() {
        0 => String::new(),
        1 => parts[0].to_string(),
        n => format!("{}:{}", parts[n - 2], parts[n - 1]),
    }
}

/// Format a real average: up to two decimals, without trailing zeros.
///
/// `pub(crate)`: the profile renders the same averages and must show them
/// with the same precision (2.75 stays 2.75, not 2.8).
pub(crate) fn format_grade(g: f64) -> String {
    let s = format!("{:.2}", g);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// Shorten a string to `max` characters (adding an ellipsis when cut).
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn summary(name: &str, average: f64, codigo: &str) -> templates::SubjectSummary {
        templates::SubjectSummary {
            name: name.to_string(),
            average,
            count: 1,
            codigo: codigo.to_string(),
        }
    }

    #[test]
    fn url_encode_path_encodes_spaces_and_accents() {
        assert_eq!(url_encode_path("INFO01"), "INFO01");
        assert_eq!(
            url_encode_path("26EGB10A-BIOL - Biología"),
            "26EGB10A-BIOL%20-%20Biolog%C3%ADa"
        );
        assert_eq!(url_encode_path("Matemática 1"), "Matem%C3%A1tica%201");
    }

    #[test]
    fn card_links_to_the_real_code() {
        let html = subject_card_html(
            &summary("Biología", 3.2, "26EGB10A-BIOL - Biología"),
            "2026:T3:1",
            Some((
                ActivityCount {
                    total: 6,
                    graded: 1,
                },
                true,
            )),
        );
        assert!(html.contains("/dashboard/subjects/26EGB10A-BIOL%20-%20Biolog%C3%ADa"));
        assert!(html.contains("Biolog"));
        assert!(html.contains("3.2"));
        assert!(html.contains("6 actividades"));
        assert!(html.contains("1 con nota"));
    }

    #[test]
    fn card_falls_back_to_name_when_code_is_too_long() {
        let long = "X".repeat(MAX_CODE_LEN + 1);
        let html = subject_card_html(&summary("Matematica", 4.56, &long), "2026:T3:1", None);
        // The link uses the name (the route resolves it too), not the code
        // that the detail route would reject.
        assert!(html.contains(r#"href="/dashboard/subjects/Matematica""#));
        assert!(!html.contains(&format!("/dashboard/subjects/{long}")));
    }

    #[test]
    fn card_shows_the_exact_average_not_a_rounded_one() {
        let html = subject_card_html(
            &summary("Educacion Fisica", 2.75, "26EGB10A-EEFF - Educacion Fisica"),
            "2026:T3:1",
            None,
        );
        // The real average is 2.75: showing "2.8" would contradict the
        // /dashboard/profile bars and the percentage (28%).
        assert!(html.contains(">2.75<"), "html: {html}");
        assert!(!html.contains(">2.8<"), "html: {html}");
    }

    #[test]
    fn card_bar_fill_is_visible() {
        let html = subject_card_html(&summary("Matematica", 4.56, "M"), "2026:T3:1", None);
        // The fill needs a `bg-*` class: with the `text-*` class the bar was
        // rendered without color (invisible fill).
        assert!(
            html.contains(r#"<div class="h-full bg-red-400 transition-all" style="width: 46%">"#),
            "html: {html}"
        );
        assert!(
            !html.contains("text-red-400 transition-all"),
            "html: {html}"
        );
    }

    #[test]
    fn colors_follow_the_spanish_pass_thresholds() {
        assert_eq!(grade_colors(7.0), ("text-teal-400", "bg-teal-400"));
        assert_eq!(grade_colors(5.0), ("text-amber-400", "bg-amber-400"));
        assert_eq!(grade_colors(4.99), ("text-red-400", "bg-red-400"));
    }

    #[test]
    fn card_without_average_does_not_invent_a_grade() {
        let html = subject_card_html(
            &summary("Física", f64::NAN, "26EGB10A-FISI - Física"),
            "2026:T3:1",
            Some((
                ActivityCount {
                    total: 3,
                    graded: 0,
                },
                true,
            )),
        );
        assert!(html.contains("sin promedio todavía"));
        assert!(!html.contains("NaN"));
        assert!(html.contains("3 actividades"));
    }

    #[test]
    fn notas_from_promedios_keeps_subjects_without_average() {
        let full = json!({
            "promedios": { "stats": [
                { "courseName": "Matematica", "courseCode": "26EGB10A-MATE - Matematica",
                  "average": 4.56, "partialId": "2026:T3:1" },
                { "courseName": "Física", "courseCode": "26EGB10A-FISI - Física",
                  "average": null, "partialId": "2026:T3:1" }
            ]}
        });
        let (notas, parciales) = notas_from_promedios(&full);
        assert_eq!(notas.len(), 2);
        assert_eq!(notas[0].materia, "Matematica");
        assert_eq!(notas[0].nota, 4.56);
        assert_eq!(notas[0].codigo, "26EGB10A-MATE - Matematica");
        assert_eq!(notas[0].trimestre, 3);
        assert!(notas[1].nota.is_nan());
        assert_eq!(parciales.get("matematica"), Some(&"2026:T3:1".to_string()));
    }

    #[test]
    fn subjects_are_ordered_by_worst_average_first() {
        let mut subjects = vec![
            summary("Matematica", 4.56, "M"),
            summary("English", 6.24, "E"),
            summary("Educacion Fisica", 2.75, "EF"),
            summary("Física", f64::NAN, "F"),
        ];
        order_subjects(&mut subjects);
        let names: Vec<&str> = subjects.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Educacion Fisica", "Matematica", "English", "Física"]
        );
    }

    #[test]
    fn activity_counts_are_parsed_per_materia_and_parcial() {
        let raw = json!({
            "materias": [{
                "materia": "Lengua y Literatura",
                "submenus": [{
                    "submenu": "TAREAS",
                    "total_actividades": 2,
                    "actividades_calicadas": 1,
                    "actividades": [
                        { "actividad": "Tarea 1", "trimestre": "2026:T3:1", "nota": 10.0, "calificada": true },
                        { "actividad": "Tarea 2", "trimestre": "2026:T3:1", "nota": null, "calificada": false }
                    ]
                }]
            }]
        });
        let counts = parse_activity_counts(&raw);
        let c = counts
            .get(&("lengua y literatura".to_string(), "T3:1".to_string()))
            .expect("counts for the subject");
        assert_eq!(c.total, 2);
        assert_eq!(c.graded, 1);
    }

    #[test]
    fn activity_counts_fall_back_to_server_counters() {
        let raw = json!({
            "materias": [{
                "materia": "Quimica",
                "submenus": [{ "submenu": "TAREAS", "total_actividades": 3, "actividades_calicadas": 0 }]
            }]
        });
        let counts = parse_activity_counts(&raw);
        let c = counts
            .get(&("quimica".to_string(), String::new()))
            .expect("counts for the subject");
        assert_eq!(c.total, 3);
        assert_eq!(c.graded, 0);
    }

    #[test]
    fn parcial_key_drops_the_year() {
        assert_eq!(parcial_key("2026:T3:1"), "T3:1");
        assert_eq!(parcial_key("T3:1"), "T3:1");
        assert_eq!(parcial_key("T3"), "T3");
        assert_eq!(parcial_key(""), "");
    }

    #[test]
    fn normalize_name_strips_accents() {
        assert_eq!(normalize_name("Física"), "fisica");
        assert_eq!(normalize_name("Educacion Fisica"), "educacion fisica");
    }

    #[tokio::test]
    async fn a_failed_handshake_is_not_a_connected_mcp() {
        // `Ok(Err(..))`: el handshake falló (MCP caído) — no está conectado.
        assert!(!handshake_ok::<(), &str>(Ok(Err("transport error"))));
        // `Ok(Ok(..))`: handshake correcto — sí está conectado.
        assert!(handshake_ok::<(), &str>(Ok(Ok(()))));
        // `Err(Elapsed)`: se agotó el tiempo — tampoco está conectado.
        let elapsed: Result<Result<(), tokio::time::error::Elapsed>, tokio::time::error::Elapsed> =
            Err(
                time::timeout(Duration::from_millis(1), std::future::pending::<()>())
                    .await
                    .unwrap_err(),
            );
        assert!(!handshake_ok(elapsed));
    }

    #[test]
    fn la_pagina_tiene_un_presupuesto_de_cinco_segundos() {
        // El presupuesto es la garantía de la página: si el MCP no contesta,
        // la respuesta sale como mucho en 5 s con el estado vacío honesto.
        assert_eq!(PRESUPUESTO_MCP, Duration::from_secs(5));
        // El handshake va dentro de ese presupuesto, no aparte.
        assert!(PRESUPUESTO_HANDSHAKE <= PRESUPUESTO_MCP);
    }

    #[test]
    fn el_cache_de_promedios_comparte_el_dato_entre_perfil_y_asignaturas() {
        // Se guarda un resultado parseado…
        let promedios = Promedios {
            notas: vec![Nota {
                materia: "Matematica".to_string(),
                nota: 4.56,
                trimestre: 3,
                codigo: "MATE01".to_string(),
            }],
            parciales: HashMap::from([("matematica".to_string(), "2026:T3:1".to_string())]),
        };
        guardar_promedios(&promedios);

        // …y las dos páginas lo leen del mismo sitio, sin llamar al MCP.
        let cacheado = promedios_cacheados().expect("promedios en caché");
        assert_eq!(cacheado.notas.len(), 1);
        assert_eq!(cacheado.notas[0].nota, 4.56);
        assert_eq!(
            cacheado.parciales.get("matematica").map(String::as_str),
            Some("2026:T3:1")
        );

        // Se limpia para no dejar estado global para otros tests.
        if let Ok(mut c) = cache_promedios().lock() {
            *c = None;
        }
    }

    #[test]
    fn el_aviso_del_mcp_se_registra_una_sola_vez() {
        let clave = "test-aviso-mcp";
        // La primera vez se anota; la segunda ya no (el mismo fallo se repite
        // en cada carga de página).
        assert!(warn_una_vez(clave, "MCP: fallo de prueba"));
        assert!(!warn_una_vez(clave, "MCP: fallo de prueba"));
        // Rearmarlo deja registrar una incidencia posterior.
        warn_rearmar(clave);
        assert!(warn_una_vez(clave, "MCP: fallo de prueba"));
        warn_rearmar(clave);
    }
}
