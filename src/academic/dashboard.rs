//! Main dashboard page — daily priorities, quick stats, upcoming events.
//!
//! Supports HTMX partial reloads via the `?partial=` query parameter:
//!   - `stats`     → stats grid HTML
//!   - `profile`   → profile card HTML
//!   - `subjects`  → subject quick-links HTML
//!   - `events`    → event timeline HTML
//!   - `tasks`     → pending tasks list HTML
//!   - `greeting`  → saludo con el nombre real del perfil
//!   - `nextclass` → tarjeta "Próxima clase" (horario real de Mikareno)
//!   - `agenda`    → calendario pequeño con las próximas 3 tareas
//!
//! Todo lo que se muestra viene del cliente Mikareno compartido (caché
//! read-through de 180 s) o de la BD local. Si Mikareno no responde, cada
//! bloque cae a un estado vacío honesto: no se inventan clases ni tareas.
//!
//! Tiempos: toda la fase Mikareno (handshake + datos) cabe en un presupuesto
//! total de 4 s. El handshake debe HABER TENIDO ÉXITO (`Ok(Ok(_))`) para
//! declarar "Conectado": un `Ok(Err(..))` es un handshake fallido. Si el
//! plazo se agota, la página se entrega igual con estados vacíos honestos y
//! el aviso "Mikareno no responde ahora".

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::{extract::State, response::Html};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, Timelike, Weekday};
use serde::Deserialize;
use tokio::time::{self, Duration, Instant};

use crate::academic::subjects::handshake_ok;
use crate::academic::{mikareno_client, templates};
use crate::db;
use crate::integrations::mikareno::{Horario, Nota, Perfil, Tarea};
use crate::AppState;

/// Max length for the `partial` query param (longer values yield empty output).
const MAX_PARTIAL_LEN: usize = 32;

/// Presupuesto total de la fase Mikareno del dashboard (4 s).
///
/// Todas las tarjetas de la página (saludo, próxima clase, agenda, notas y
/// calendario) comparten este plazo: si Mikareno no contesta, la página se
/// entrega igual con estados vacíos honestos en vez de colgarse un minuto.
const PRESUPUESTO_MCP: Duration = Duration::from_secs(4);

/// Tope del handshake dentro del presupuesto total.
///
/// El cliente MCP tiene su propio deadline de 20 s (`INIT_DEADLINE_SECS`):
/// sin este tope el handshake se comía la página entera.
const TIMEOUT_HANDSHAKE: Duration = Duration::from_secs(2);

/// Ejecuta una llamada al MCP dentro del presupuesto compartido.
///
/// `None` = la llamada falló (error del MCP) o no llegó a tiempo. Lo que ya
/// está en la caché read-through del cliente (TTL 180 s) se resuelve sin
/// I/O, así que con la caché vacía un `None` significa "Mikareno no
/// respondió".
pub(crate) async fn con_deadline<T, E>(
    deadline: Instant,
    fut: impl std::future::Future<Output = Result<T, E>>,
) -> Option<T> {
    let restante = deadline.saturating_duration_since(Instant::now());
    if restante.is_zero() {
        return None;
    }
    match time::timeout(restante, fut).await {
        Ok(Ok(v)) => Some(v),
        _ => None,
    }
}

/// Semana completa en orden (lunes→domingo) para recorrer los próximos días.
const DIAS_SEMANA: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

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
    let mut init_ok = false;
    let mut calendario: Vec<crate::integrations::mikareno::CalendarioEvent> = Vec::new();
    let mut tareas: Vec<Tarea> = Vec::new();
    let mut notas: Vec<Nota> = Vec::new();
    // Horario semanal real (cache-first del cliente compartido).
    let mut horario: Horario = Horario::default();
    // Empty Perfil until real data arrives — honest placeholders, no mocks.
    let mut perfil = Perfil::default();
    // Aviso honesto: Mikareno no respondió (o no respondió del todo).
    let mut aviso_mcp: Option<String> = None;

    // Fase Mikareno: UN presupuesto total para toda la página.
    //
    // Antes cada llamada esperaba su propio timeout (10 s el handshake, 10 s
    // el horario, y las llamadas de datos sin tope) y la página tardaba un
    // minuto en pintar. Ahora el handshake y los cinco datos comparten un
    // plazo de 4 s: si Mikareno no contesta, todas las tarjetas caen al
    // estado vacío honesto JUNTAS y la página se entrega igual.
    if let Some(client) = client_opt.as_ref() {
        let deadline = Instant::now() + PRESUPUESTO_MCP;

        // Handshake con deadline corto. Sólo `Ok(Ok(()))` es conexión real:
        // `Ok(Err(..))` es un handshake FALLIDO (MCP caído) y el `is_ok()`
        // exterior lo declaraba "Conectado" con el servidor muerto.
        let restante = deadline.saturating_duration_since(Instant::now());
        init_ok =
            handshake_ok(time::timeout(restante.min(TIMEOUT_HANDSHAKE), client.initialize()).await);
        connected = init_ok;

        if init_ok {
            // Los datos van EN PARALELO y cada llamada se corta en el
            // deadline común: la carga dura lo que la más lenta, no la suma.
            // `horario()` es cache-first (TTL 180 s): un horario ya
            // descargado se sirve sin I/O.
            let (cal, per, tar, nts, hor) = tokio::join!(
                con_deadline(deadline, client.calendario()),
                con_deadline(deadline, client.perfil()),
                con_deadline(deadline, client.tareas()),
                con_deadline(deadline, client.notas_full()),
                con_deadline(deadline, client.horario()),
            );
            let completo =
                cal.is_some() && per.is_some() && tar.is_some() && nts.is_some() && hor.is_some();
            calendario = cal.unwrap_or_default();
            // On a fetch error we keep the empty Perfil — no fabricated data.
            perfil = per.unwrap_or_default();
            tareas = tar.unwrap_or_default();
            // Promedios reales del parcial en curso (get_student_full).
            notas.extend(nts.unwrap_or_default());
            horario = hor.unwrap_or_default();

            // Alguna llamada se quedó sin tiempo: se dice, no se disimula.
            if !completo {
                aviso_mcp = Some(
                    "Mikareno no devolvió todos los datos dentro del presupuesto: se muestran \
                     sólo los disponibles."
                        .to_string(),
                );
            }
        } else {
            aviso_mcp = Some(
                "Mikareno no responde ahora: se muestran estados vacíos, no datos.".to_string(),
            );
        }
    }

    let note_count = db::get_note_count(&state.db).await.unwrap_or(0);
    // Sin notas (MCP desconectado o sin notas) la grilla muestra un estado
    // vacío honesto en lugar de asignaturas inventadas.
    let subjects = templates::subjects_from_notas(&notas);

    let pending_tareas: Vec<&Tarea> = tareas
        .iter()
        .filter(|t| !t.estado.to_lowercase().contains("entregada"))
        .collect();

    // --- Estado del día: saludo, próxima clase y agenda ---
    let ahora = Local::now().naive_local();
    let hoy_iso = ahora.format("%Y-%m-%d").to_string();
    // Mikareno marca en el calendario los días no escolares: hoy no hay
    // clases aunque el horario semanal liste ese día.
    let no_lectivo_hoy = calendario
        .iter()
        .any(|e| e.fecha == hoy_iso && e.tipo == "NON_SCHOOL_DAY");
    let proxima = if no_lectivo_hoy {
        ProximaClase::NoLectivo
    } else {
        resolver_proxima_clase(&horario, ahora)
    };
    let greeting = greeting_html(&perfil, ahora);
    let next_class = proxima_clase_html(&proxima, ahora);
    let agenda = agenda_html(&pending_tareas);

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
                let today = hoy_iso.clone();
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
            "greeting" => greeting,
            "nextclass" => next_class,
            "agenda" => agenda,
            _ => String::new(),
        };
        return Html(html);
    }

    // --- Full page render (Stitch design) ---
    let status = templates::mcp_status_banner(connected);
    // Aviso honesto bajo el banner: si Mikareno no respondió, se dice en
    // vez de dejar estados vacíos sin explicación.
    let aviso = aviso_mcp.as_deref().map(aviso_mcp_html).unwrap_or_default();
    let profile = templates::profile_card_html(&perfil);
    let stats = templates::stats_grid_html(
        note_count,
        subjects.len(),
        pending_tareas.len(),
        calendario.len().min(5),
    );

    // Tarjetas del día: próxima clase real + calendario de tareas.
    let top_cards = format!(
        r###"<div class="grid grid-cols-1 lg:grid-cols-2 gap-space-md mb-space-md">
            {next_class}
            {agenda}
        </div>"###
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

    let today = hoy_iso.clone();
    let upcoming: Vec<_> = calendario.iter().take(5).cloned().collect();
    // Sin eventos (o MCP desconectado) la timeline muestra su estado vacío
    // honesto: "No hay eventos programados." con botón Reintentar.
    let events_html = templates::event_timeline_html(&upcoming, Some(&today));
    let events_list = format!(
        r###"<div class="mb-space-md" id="events-section">
            <div class="flex items-center justify-between mb-space-sm">
                <h2 class="font-headline-sm text-headline-sm text-primary-container">Proximos eventos</h2>
                <button hx-get="/dashboard?partial=events" hx-target="#events-section" hx-swap="outerHTML"
                    class="font-label-sm text-label-sm text-primary-container hover:text-primary-fixed-dim" title="Actualizar">Actualizar</button>
            </div>
            {}
        </div>"###,
        events_html
    );

    let tasks_items: Vec<String> = pending_tareas
        .iter()
        .map(|t| format!("{} — {} ({})", t.titulo, t.vencimiento, t.estado))
        .collect();
    // Sin tareas pendientes card_list muestra su estado vacío honesto:
    // "No hay elementos."
    let tasks_list = format!(
        r###"<div class="mb-space-md" id="tasks-section">
            <div class="flex items-center justify-between mb-space-sm">
                <h2 class="font-headline-sm text-headline-sm text-primary-container">Tareas pendientes</h2>
                <button hx-get="/dashboard?partial=tasks" hx-target="#tasks-section" hx-swap="outerHTML"
                    class="font-label-sm text-label-sm text-primary-container hover:text-primary-fixed-dim" title="Actualizar">Actualizar</button>
            </div>
            {}
        </div>"###,
        templates::card_list("Tareas pendientes", &tasks_items)
    );

    // Daily priorities — nearest pending tasks from Mikareno; when there are
    // none, an honest empty note instead of invented examples.
    let prio_li = if pending_tareas.is_empty() {
        r###"<li class="flex items-center p-space-sm rounded-lg bg-surface-container border border-outline">
                    <span class="font-body-md text-body-md text-outline">Sin tareas pendientes en Mikareno.</span>
                </li>"###
            .to_string()
    } else {
        pending_tareas
            .iter()
            .take(3)
            .enumerate()
            .map(|(i, t)| {
                format!(
                    r###"<li class="flex items-center gap-space-sm p-space-sm rounded-lg bg-surface-container border border-outline hover:border-primary-container transition-colors">
                    <input type="checkbox" class="sr-only peer" id="prio{i}">
                    <span class="flex h-5 w-5 items-center justify-center rounded bg-surface-container border border-outline text-xs text-transparent transition-colors peer-checked:bg-secondary peer-checked:border-secondary peer-checked:text-on-secondary">&#10003;</span>
                    <label class="flex-1 font-body-md text-body-md text-on-surface" for="prio{i}">{} (vence: {})</label>
                </li>"###,
                    templates::html_escape(&t.titulo),
                    templates::html_escape(&t.vencimiento),
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };
    let priorities = format!(
        r###"<div class="mb-space-md">
            <h2 class="font-headline-sm text-headline-sm text-primary-container mb-space-sm">Prioridades del dia</h2>
            <ul class="list-none space-y-space-xs">{items}</ul>
        </div>"###,
        items = prio_li
    );

    let content = format!(
        "{}{}{}{}{}{}{}{}{}{}",
        greeting,
        status,
        aviso,
        profile,
        top_cards,
        stats,
        subjects_html,
        events_list,
        tasks_list,
        priorities
    );

    Html(templates::page(
        "Dashboard Academico — Study Platform",
        &content,
    ))
}

// ── Aviso de Mikareno ─────────────────────────────────────────────────────

/// Aviso honesto cuando Mikareno no respondió dentro del presupuesto.
///
/// No se disimula el estado vacío: se dice que Mikareno no responde ahora
/// (o que no devolvió todo), para que el estado vacío se lea como lo que
/// es y no como "no hay datos en Mikareno".
fn aviso_mcp_html(aviso: &str) -> String {
    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-3 mb-space-md">
            <p class="font-body-sm text-body-sm text-on-surface-variant">{}</p>
        </div>"###,
        templates::html_escape(aviso)
    )
}

// ── Saludo ────────────────────────────────────────────────────────────────

/// Saludo según la hora local: mañana (<12), tarde (<19) o noche.
fn saludo_por_hora(hora: u32) -> &'static str {
    if hora < 12 {
        "Buenos días"
    } else if hora < 19 {
        "Buenas tardes"
    } else {
        "Buenas noches"
    }
}

/// Primer nombre del perfil ("Lennin Gabriel Salas" → "Lennin").
fn primer_nombre(nombre: &str) -> String {
    nombre.split_whitespace().next().unwrap_or("").to_string()
}

/// "Lunes 19 de octubre de 2026".
fn fecha_es(fecha: NaiveDate) -> String {
    let iso = format!(
        "{:04}-{:02}-{:02}",
        fecha.year(),
        fecha.month(),
        fecha.day()
    );
    match crate::handlers::schedule::mes_es(fecha.month()) {
        Some(mes) => format!(
            "{}, {} de {} de {}",
            templates::weekday_es(&iso),
            fecha.day(),
            mes,
            fecha.year()
        ),
        None => iso,
    }
}

/// Encabezado de saludo: hora del día + nombre real del perfil.
///
/// Sin nombre real (Mikareno caído) se muestra sólo el saludo: nunca un
/// nombre inventado.
fn greeting_html(perfil: &Perfil, ahora: NaiveDateTime) -> String {
    let saludo = saludo_por_hora(ahora.hour());
    let nombre = primer_nombre(&perfil.nombre);
    let titulo = if nombre.is_empty() {
        saludo.to_string()
    } else {
        format!("{}, {}", saludo, nombre)
    };
    format!(
        r###"<div class="mb-space-md" id="greeting-section">
            <h1 class="font-headline-lg text-headline-lg text-primary-container">{}</h1>
            <p class="font-body-sm text-body-sm text-on-surface-variant">{}</p>
        </div>"###,
        templates::html_escape(&titulo),
        templates::html_escape(&fecha_es(ahora.date())),
    )
}

// ── Próxima clase ─────────────────────────────────────────────────────────

/// Una clase del horario con su hora de inicio (minutos desde medianoche).
#[derive(Clone, Debug)]
struct Clase {
    /// Etiqueta original del horario ("07:35 - 08:15 — Matemáticas [MATE01]").
    etiqueta: String,
    /// Inicio en minutos desde medianoche.
    inicio: u32,
    /// Fin en minutos desde medianoche, si la etiqueta lo trae.
    fin: Option<u32>,
}

/// Estado de la tarjeta "Próxima clase" — siempre derivado del horario real.
#[derive(Clone, Debug)]
enum ProximaClase {
    /// El MCP no trajo horario: no se afirma nada.
    SinHorario,
    /// Hoy el calendario de Mikareno lo marca como día no escolar.
    NoLectivo,
    /// Hoy hay clases, pero el horario no trae hora de inicio interpretable.
    HoySinHora {
        total: usize,
        siguiente: Option<(NaiveDate, Clase)>,
    },
    /// Clase de hoy: en curso o por empezar.
    Hoy { clase: Clase, en_curso: bool },
    /// Hoy no queda ninguna clase por delante.
    SinMasHoy {
        /// `true` si el horario no lista ninguna clase para hoy.
        libre_hoy: bool,
        /// La siguiente clase real de los próximos 7 días, si la hay.
        siguiente: Option<(NaiveDate, Clase)>,
    },
    /// Hoy ya terminó; la siguiente clase real es de otro día.
    OtroDia { fecha: NaiveDate, clase: Clase },
}

/// Minutos desde medianoche de la hora de inicio y el resto de la etiqueta.
///
/// Sólo acepta etiquetas que *empiecen* con "HH:MM" (así "SAB 08:00" no se
/// confunde con una hora de clase).
fn inicio_minutos(etiqueta: &str) -> Option<(u32, &str)> {
    let b = etiqueta.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i > 2 || b.get(i) != Some(&b':') {
        return None;
    }
    let h: u32 = etiqueta[..i].parse().ok()?;
    let j = i + 1;
    let mut k = j;
    while k < b.len() && b[k].is_ascii_digit() {
        k += 1;
    }
    if k - j != 2 {
        return None;
    }
    let m: u32 = etiqueta[j..k].parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some((h * 60 + m, &etiqueta[k..]))
}

/// Hora de fin de una clase ("07:35 - 08:15 — Materia" → 08:15), si la trae.
fn fin_minutos(resto: &str) -> Option<u32> {
    let b = resto.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    if b.get(i) != Some(&b'-') {
        return None;
    }
    i += 1;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    let (fin, _) = inicio_minutos(&resto[i..])?;
    Some(fin)
}

/// Descompone una etiqueta de clase del horario.
fn parse_clase(etiqueta: &str) -> Option<Clase> {
    let (inicio, resto) = inicio_minutos(etiqueta)?;
    Some(Clase {
        etiqueta: etiqueta.to_string(),
        inicio,
        fin: fin_minutos(resto),
    })
}

/// Asignatura y código de una etiqueta ("07:35 - 08:15 — Mate [MATE01]" →
/// ("Mate", "MATE01")).
fn parte_asignatura(etiqueta: &str) -> (String, String) {
    let resto = match etiqueta.rsplit_once(" — ") {
        Some((_, r)) => r.trim(),
        None => etiqueta.trim(),
    };
    if let Some(i) = resto.find(" [") {
        if resto.ends_with(']') && i + 2 < resto.len() {
            return (
                resto[..i].trim().to_string(),
                resto[i + 2..resto.len() - 1].trim().to_string(),
            );
        }
    }
    (resto.to_string(), String::new())
}

/// "HH:MM" a partir de minutos desde medianoche.
fn hhmm(minutos: u32) -> String {
    format!("{:02}:{:02}", minutos / 60, minutos % 60)
}

/// Etiqueta corta de fecha ("Martes 20/10").
fn etiqueta_fecha(fecha: NaiveDate) -> String {
    let iso = format!(
        "{:04}-{:02}-{:02}",
        fecha.year(),
        fecha.month(),
        fecha.day()
    );
    format!(
        "{} {:02}/{:02}",
        templates::weekday_es(&iso),
        fecha.day(),
        fecha.month()
    )
}

/// Resuelve la próxima clase real a partir del horario semanal de Mikareno.
fn resolver_proxima_clase(horario: &Horario, ahora: NaiveDateTime) -> ProximaClase {
    if horario.dias.is_empty() {
        return ProximaClase::SinHorario;
    }

    let hoy = ahora.weekday();
    let hoy_fecha = ahora.date();
    let minutos_ahora = ahora.hour() * 60 + ahora.minute();

    // Clases por día de la semana (sólo las que traen hora interpretable).
    let mut por_dia: HashMap<Weekday, Vec<Clase>> = HashMap::new();
    let mut clases_hoy = 0usize;
    for dia in &horario.dias {
        let Some(wd) = crate::handlers::schedule::dia_a_weekday(&dia.dia) else {
            continue;
        };
        if wd == hoy {
            clases_hoy = dia.clases.len();
        }
        for clase in &dia.clases {
            if let Some(c) = parse_clase(clase) {
                por_dia.entry(wd).or_default().push(c);
            }
        }
    }
    if por_dia.is_empty() {
        return ProximaClase::SinHorario;
    }
    // Orden cronológico dentro de cada día (el MCP no lo garantiza).
    for clases in por_dia.values_mut() {
        clases.sort_by_key(|c| c.inicio);
    }

    if let Some(hoy_clases) = por_dia.get(&hoy) {
        // 1) La clase que está en curso ahora mismo.
        if let Some(c) = hoy_clases.iter().find(|c| {
            c.inicio <= minutos_ahora && c.fin.map(|f| minutos_ahora < f).unwrap_or(false)
        }) {
            return ProximaClase::Hoy {
                clase: c.clone(),
                en_curso: true,
            };
        }
        // 2) La siguiente que empieza hoy.
        if let Some(c) = hoy_clases.iter().find(|c| c.inicio >= minutos_ahora) {
            return ProximaClase::Hoy {
                clase: c.clone(),
                en_curso: false,
            };
        }
        // 3) Hoy ya terminó: la siguiente clase real es de otro día.
        return match siguiente_clase(&por_dia, hoy, hoy_fecha) {
            Some((fecha, clase)) => ProximaClase::OtroDia { fecha, clase },
            None => ProximaClase::SinMasHoy {
                libre_hoy: false,
                siguiente: None,
            },
        };
    }

    // 4) Hoy el horario no lista ninguna clase con hora de inicio.
    let siguiente = siguiente_clase(&por_dia, hoy, hoy_fecha);
    if clases_hoy > 0 {
        return ProximaClase::HoySinHora {
            total: clases_hoy,
            siguiente,
        };
    }
    ProximaClase::SinMasHoy {
        libre_hoy: true,
        siguiente,
    }
}

/// La siguiente clase real de los próximos 7 días (menor hora de inicio).
fn siguiente_clase(
    por_dia: &HashMap<Weekday, Vec<Clase>>,
    hoy: Weekday,
    hoy_fecha: NaiveDate,
) -> Option<(NaiveDate, Clase)> {
    for offset in 1..=7u32 {
        let idx = (hoy.num_days_from_monday() + offset) % 7;
        let wd = DIAS_SEMANA[idx as usize];
        if let Some(clases) = por_dia.get(&wd) {
            if let Some(c) = clases.iter().min_by_key(|c| c.inicio) {
                return Some((hoy_fecha + chrono::Duration::days(offset as i64), c.clone()));
            }
        }
    }
    None
}

/// Línea con la hora de inicio y la asignatura de una clase.
fn linea_clase(clase: &Clase) -> String {
    let (nombre, codigo) = parte_asignatura(&clase.etiqueta);
    let texto = if codigo.is_empty() {
        nombre
    } else {
        format!("{} [{}]", nombre, codigo)
    };
    format!(
        r###"<p class="font-headline-md text-headline-md text-on-surface">{} · {}</p>"###,
        templates::html_escape(&hhmm(clase.inicio)),
        templates::html_escape(&texto),
    )
}

/// Línea de la siguiente clase real ("Siguiente: Martes 20/10 · 09:00 — Mate").
fn linea_siguiente(siguiente: &Option<(NaiveDate, Clase)>, ahora: NaiveDateTime) -> String {
    let Some((fecha, clase)) = siguiente else {
        return String::new();
    };
    let (nombre, codigo) = parte_asignatura(&clase.etiqueta);
    let texto = if codigo.is_empty() {
        nombre
    } else {
        format!("{} [{}]", nombre, codigo)
    };
    format!(
        r###"<p class="font-label-sm text-label-sm text-on-surface-variant mt-space-xs">Siguiente: {} · {} — {} ({})</p>"###,
        templates::html_escape(&etiqueta_fecha(*fecha)),
        templates::html_escape(&hhmm(clase.inicio)),
        templates::html_escape(&texto),
        templates::html_escape(&relativo_dia(*fecha, ahora)),
    )
}

/// "mañana" / "en 3 días" / "la próxima semana" según la fecha destino.
fn relativo_dia(fecha: NaiveDate, ahora: NaiveDateTime) -> String {
    let dias = (fecha - ahora.date()).num_days();
    if dias <= 1 {
        "mañana".to_string()
    } else if dias <= 6 {
        format!("en {} días", dias)
    } else {
        "la próxima semana".to_string()
    }
}

/// Tarjeta "Próxima clase" — sólo afirma lo que el horario real soporta.
fn proxima_clase_html(info: &ProximaClase, ahora: NaiveDateTime) -> String {
    let cuerpo = match info {
        ProximaClase::SinHorario => {
            r###"<p class="font-body-sm text-body-sm text-on-surface-variant">Sin horario disponible: Mikareno no devolvió el horario de clases.</p>"###
                .to_string()
        }
        ProximaClase::NoLectivo => {
            r###"<p class="font-body-sm text-body-sm text-on-surface-variant">Hoy no es día lectivo: Mikareno lo marca como día no escolar.</p>"###
                .to_string()
        }
        ProximaClase::HoySinHora { total, siguiente } => format!(
            r###"<p class="font-body-sm text-body-sm text-on-surface-variant">El horario lista {} clases hoy, pero no trae hora de inicio.</p>{}"###,
            total,
            linea_siguiente(siguiente, ahora),
        ),
        ProximaClase::Hoy { clase, en_curso } => {
            let minutos_ahora = ahora.hour() * 60 + ahora.minute();
            let estado = if *en_curso {
                "En curso ahora".to_string()
            } else {
                let faltan = clase.inicio.saturating_sub(minutos_ahora);
                if faltan == 0 {
                    "Empieza ahora".to_string()
                } else if faltan < 60 {
                    format!("Empieza en {} min", faltan)
                } else {
                    format!("Empieza a las {}", hhmm(clase.inicio))
                }
            };
            format!(
                r###"<p class="font-body-sm text-body-sm text-on-surface-variant">Hoy</p>{}<p class="font-label-sm text-label-sm text-on-surface-variant">{}</p>"###,
                linea_clase(clase),
                templates::html_escape(&estado),
            )
        }
        ProximaClase::SinMasHoy { libre_hoy, siguiente } => {
            let texto = if *libre_hoy {
                "Hoy no tienes clases: libre hoy."
            } else {
                "Las clases de hoy ya terminaron."
            };
            format!(
                r###"<p class="font-body-sm text-body-sm text-on-surface-variant">{}</p>{}"###,
                templates::html_escape(texto),
                linea_siguiente(siguiente, ahora),
            )
        }
        ProximaClase::OtroDia { fecha, clase } => format!(
            r###"<p class="font-body-sm text-body-sm text-on-surface-variant">{}</p>{}<p class="font-label-sm text-label-sm text-on-surface-variant">{}</p>"###,
            templates::html_escape(&etiqueta_fecha(*fecha)),
            linea_clase(clase),
            templates::html_escape(&relativo_dia(*fecha, ahora)),
        ),
    };

    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-space-md" id="next-class-section">
            <div class="flex items-center justify-between mb-space-sm">
                <h2 class="font-headline-sm text-headline-sm text-primary-container">Próxima clase</h2>
                <button hx-get="/dashboard?partial=nextclass" hx-target="#next-class-section" hx-swap="outerHTML"
                    class="inline-flex items-center gap-1 px-2 py-1 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container-high transition-colors" title="Actualizar">
                    <span class="material-symbols-outlined text-[16px]">refresh</span>
                </button>
            </div>
            {cuerpo}
        </div>"###
    )
}

// ── Agenda de tareas ──────────────────────────────────────────────────────

/// Clave de orden por vencimiento: las fechas ISO ("2026-10-20") van primero.
fn clave_vencimiento(t: &Tarea) -> (u8, String) {
    let v = t.vencimiento.trim();
    let fecha: String = v.chars().take(10).collect();
    if fecha.len() == 10
        && fecha.as_bytes().get(4) == Some(&b'-')
        && fecha.as_bytes().get(7) == Some(&b'-')
    {
        (0, fecha)
    } else {
        (1, v.to_string())
    }
}

/// Vencimiento en formato corto ("2026-10-20T10:00" → "20/10 10:00").
fn vencimiento_corto(v: &str) -> String {
    let v = v.trim();
    if v.is_empty() {
        return "sin fecha".to_string();
    }
    let fecha: String = v.chars().take(10).collect();
    if let Ok(d) = chrono::NaiveDate::parse_from_str(&fecha, "%Y-%m-%d") {
        let hora = v
            .get(10..)
            .map(|h| {
                h.trim_start_matches(['T', ' '])
                    .chars()
                    .take(5)
                    .collect::<String>()
            })
            .unwrap_or_default();
        if hora.len() == 5 {
            return format!("{:02}/{:02} {}", d.day(), d.month(), hora);
        }
        return format!("{:02}/{:02}/{}", d.day(), d.month(), d.year());
    }
    v.chars().take(30).collect()
}

/// Tarjeta de calendario con las próximas 3 tareas pendientes.
///
/// Sin tareas en la caché de Mikareno muestra el estado honesto
/// "Sin tareas pendientes en Mikareno." (nunca ejemplos inventados).
fn agenda_html(tareas: &[&Tarea]) -> String {
    let mut items: Vec<&Tarea> = tareas.to_vec();
    items.sort_by(|a, b| clave_vencimiento(*a).cmp(&clave_vencimiento(*b)));

    let cuerpo = if items.is_empty() {
        r###"<p class="font-body-sm text-body-sm text-on-surface-variant">Sin tareas pendientes en Mikareno.</p>"###
            .to_string()
    } else {
        let lis = items
            .iter()
            .take(3)
            .map(|t| {
                format!(
                    r###"<li class="flex items-start gap-space-sm py-2 border-b border-outline/40 last:border-b-0">
                        <span class="material-symbols-outlined text-[16px] text-primary-container">task_alt</span>
                        <div class="flex-1">
                            <p class="font-body-sm text-body-sm text-on-surface">{}</p>
                            <p class="font-label-sm text-label-sm text-on-surface-variant">vence {}</p>
                        </div>
                    </li>"###,
                    templates::html_escape(&t.titulo),
                    templates::html_escape(&vencimiento_corto(&t.vencimiento)),
                )
            })
            .collect::<Vec<_>>()
            .join("");
        let resto = if items.len() > 3 {
            format!(
                r###"<p class="font-label-sm text-label-sm text-on-surface-variant mt-space-sm">y {} más</p>"###,
                items.len() - 3
            )
        } else {
            String::new()
        };
        format!(
            r###"<ul class="list-none space-y-space-xs">{}</ul>{}"###,
            lis, resto
        )
    };

    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-space-md" id="agenda-section">
            <div class="flex items-center justify-between mb-space-sm">
                <h2 class="font-headline-sm text-headline-sm text-primary-container">Próximas tareas</h2>
                <button hx-get="/dashboard?partial=agenda" hx-target="#agenda-section" hx-swap="outerHTML"
                    class="inline-flex items-center gap-1 px-2 py-1 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container-high transition-colors" title="Actualizar">
                    <span class="material-symbols-outlined text-[16px]">refresh</span>
                </button>
            </div>
            {cuerpo}
        </div>"###
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::mikareno::HorarioDia;

    fn horario_semanal() -> Horario {
        Horario {
            dias: vec![
                HorarioDia {
                    dia: "lunes".to_string(),
                    clases: vec![
                        "07:35 - 08:15 — Religión [RELI]".to_string(),
                        "08:15 - 09:00 — Matemáticas [MATE01]".to_string(),
                    ],
                },
                HorarioDia {
                    dia: "martes".to_string(),
                    clases: vec!["09:00 - 09:45 — Historia [HIST]".to_string()],
                },
            ],
        }
    }

    /// Lunes 19/10/2026 a las hh:mm.
    fn lunes(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 19)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    #[test]
    fn saludo_depende_de_la_hora() {
        assert_eq!(saludo_por_hora(7), "Buenos días");
        assert_eq!(saludo_por_hora(13), "Buenas tardes");
        assert_eq!(saludo_por_hora(20), "Buenas noches");
    }

    #[test]
    fn saludo_usa_el_nombre_real_del_perfil() {
        let perfil = Perfil {
            nombre: "Lennin Gabriel Salas".to_string(),
            institucion: String::new(),
            curso: String::new(),
        };
        let html = greeting_html(&perfil, lunes(9, 0));
        assert!(html.contains("Buenos días, Lennin"));
        // Sin nombre real no se inventa ninguno.
        let html = greeting_html(&Perfil::default(), lunes(9, 0));
        assert!(html.contains("Buenos días"));
        assert!(!html.contains("Buenos días,"));
    }

    #[test]
    fn horas_solo_al_inicio_de_la_etiqueta() {
        assert_eq!(
            inicio_minutos("07:35 - 08:15 — Mate").map(|(m, _)| m),
            Some(455)
        );
        assert!(inicio_minutos("SAB 08:00").is_none());
        assert!(inicio_minutos("Mate [X]").is_none());
    }

    #[test]
    fn proxima_clase_es_la_de_hoy() {
        let h = horario_semanal();
        // Antes de la primera clase → la primera del día.
        match resolver_proxima_clase(&h, lunes(6, 0)) {
            ProximaClase::Hoy { clase, en_curso } => {
                assert_eq!(clase.inicio, 7 * 60 + 35);
                assert!(!en_curso);
            }
            otra => panic!("esperaba clase de hoy, obtuve {:?}", otra),
        }
        // Durante la primera clase → la que está en curso.
        match resolver_proxima_clase(&h, lunes(7, 50)) {
            ProximaClase::Hoy { clase, en_curso } => {
                assert_eq!(clase.inicio, 7 * 60 + 35);
                assert!(en_curso);
            }
            otra => panic!("esperaba la clase en curso, obtuve {:?}", otra),
        }
        // Terminada la primera → la segunda del día (08:15).
        match resolver_proxima_clase(&h, lunes(8, 20)) {
            ProximaClase::Hoy { clase, .. } => assert_eq!(clase.inicio, 8 * 60 + 15),
            otra => panic!("esperaba la segunda clase, obtuve {:?}", otra),
        }
    }

    #[test]
    fn sin_horario_no_se_inventa_nada() {
        assert!(matches!(
            resolver_proxima_clase(&Horario::default(), lunes(8, 0)),
            ProximaClase::SinHorario
        ));
        let html = proxima_clase_html(&ProximaClase::SinHorario, lunes(8, 0));
        assert!(html.contains("Sin horario disponible"));
    }

    #[test]
    fn libre_hoy_cuando_el_horario_no_lista_el_dia() {
        let h = Horario {
            dias: vec![HorarioDia {
                dia: "martes".to_string(),
                clases: vec!["09:00 - 09:45 — Historia [HIST]".to_string()],
            }],
        };
        match resolver_proxima_clase(&h, lunes(8, 0)) {
            ProximaClase::SinMasHoy {
                libre_hoy,
                siguiente,
            } => {
                assert!(libre_hoy);
                assert!(siguiente.is_some());
            }
            otra => panic!("esperaba libre hoy, obtuve {:?}", otra),
        }
        let html = proxima_clase_html(
            &ProximaClase::SinMasHoy {
                libre_hoy: true,
                siguiente: None,
            },
            lunes(8, 0),
        );
        assert!(html.contains("libre hoy"));
    }

    #[test]
    fn agenda_ordena_y_limita_a_tres() {
        let tareas = vec![
            Tarea {
                titulo: "Tarea 25".to_string(),
                vencimiento: "2026-10-25".to_string(),
                estado: "PENDIENTE".to_string(),
            },
            Tarea {
                titulo: "Tarea 20".to_string(),
                vencimiento: "2026-10-20T10:00".to_string(),
                estado: "PENDIENTE".to_string(),
            },
            Tarea {
                titulo: "Tarea 21".to_string(),
                vencimiento: "2026-10-21".to_string(),
                estado: "PENDIENTE".to_string(),
            },
            Tarea {
                titulo: "Tarea 22".to_string(),
                vencimiento: "2026-10-22".to_string(),
                estado: "PENDIENTE".to_string(),
            },
        ];
        let refs: Vec<&Tarea> = tareas.iter().collect();
        let html = agenda_html(&refs);
        // Ordenadas por vencimiento: 20, 21, 22 (la del 25 queda fuera).
        assert!(html.contains("Tarea 20"));
        assert!(html.contains("Tarea 21"));
        assert!(html.contains("Tarea 22"));
        assert!(!html.contains("Tarea 25"), "sólo las 3 más próximas");
        assert!(html.contains("y 1 más"));
        assert!(html.contains("20/10 10:00"));
    }

    #[test]
    fn agenda_sin_tareas_es_honesta() {
        let html = agenda_html(&[]);
        assert!(html.contains("Sin tareas pendientes"));
    }

    #[tokio::test]
    async fn con_deadline_devuelve_el_valor_o_corta_la_espera() {
        // Responde a tiempo → su valor.
        let deadline = Instant::now() + Duration::from_secs(1);
        let v = con_deadline(deadline, async { Ok::<u8, &str>(7) }).await;
        assert_eq!(v, Some(7));

        // No responde → se corta en el deadline (no espera de más).
        let deadline = Instant::now() + Duration::from_millis(50);
        let v = con_deadline(deadline, async {
            std::future::pending::<Result<u8, &str>>().await
        })
        .await;
        assert_eq!(v, None);

        // Presupuesto ya agotado → ni se intenta la llamada.
        let vencido = Instant::now() - Duration::from_secs(1);
        let v = con_deadline(vencido, async { Ok::<u8, &str>(7) }).await;
        assert_eq!(v, None);

        // Error del MCP (no un timeout) → `None`: estado vacío honesto.
        let deadline = Instant::now() + Duration::from_secs(1);
        let v = con_deadline(deadline, async { Err::<u8, &str>("mcp caido") }).await;
        assert_eq!(v, None);
    }

    #[test]
    fn aviso_de_mikareno_es_honesto() {
        let html =
            aviso_mcp_html("Mikareno no responde ahora: se muestran estados vacíos, no datos.");
        assert!(html.contains("Mikareno no responde ahora"));
    }
}
