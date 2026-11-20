//! Calendar views (monthly, weekly, daily).

use axum::{extract::State, response::Html};
use chrono::{Datelike, NaiveDate, Timelike};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::time::{self, Duration};

use crate::academic::{calendar_data, mikareno_client, templates};
use crate::integrations::mikareno::{CalendarioEvent, Horario, Tarea};
use crate::AppState;

/// Rango exacto de fechas de la rejilla mensual: empieza el lunes de la
/// primera semana y termina el domingo de la última, igual que StudyDesk.
fn rango_mes(focus: NaiveDate) -> (NaiveDate, NaiveDate) {
    use chrono::Datelike;
    let primero = focus.with_day(1).unwrap_or(focus);
    let (year, month) = (primero.year(), primero.month());
    let siguiente = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    let fin_mes = siguiente.and_then(|s| s.pred_opt()).unwrap_or(primero);
    let inicio = primero - chrono::Duration::days(primero.weekday().num_days_from_monday() as i64);
    let fin_semana =
        fin_mes + chrono::Duration::days(6 - fin_mes.weekday().num_days_from_monday() as i64);
    let fin_seis_semanas = inicio + chrono::Duration::days(41);
    (inicio, fin_semana.max(fin_seis_semanas))
}

/// Banner honesto del estado de Mikareno en el calendario.
fn banner_mcp_datos(datos: &calendar_data::DatosMikareno) -> String {
    if !datos.error {
        return String::new();
    }
    let texto = if datos.leido.is_some() {
        "Mikareno no responde ahora: se muestra la última lectura conocida."
    } else {
        "Mikareno no responde ahora: el calendario va sin datos de la plataforma."
    };
    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-3 mb-space-md">
            <p class="font-body-sm text-body-sm text-on-surface-variant">{texto}</p>
        </div>"###
    )
}

/// Max length for the `partial` query param (overlong values are ignored).
const MAX_PARTIAL_LEN: usize = 32;

/// Presupuesto total de la fase MCP de un render (5 s).
///
/// Es un presupuesto *único*: el handshake y la lectura del calendario no
/// pueden sumarlo. Si el MCP está colgado (acepta la conexión pero no
/// responde), la vista degrada a un estado vacío honesto ("Mikareno no
/// responde ahora") dentro de este plazo, en vez de quedarse esperando.
/// El handshake consume como mucho [`TIMEOUT_HANDSHAKE`] de ese total.
const TIMEOUT_DATOS: Duration = Duration::from_secs(5);

/// Tope del handshake `initialize` dentro de [`TIMEOUT_DATOS`].
///
/// Deja margen para la lectura del calendario: si el handshake se cuelga, la
/// vista degrada en 2 s en vez de agotar los 5 s del presupuesto.
const TIMEOUT_HANDSHAKE: Duration = Duration::from_secs(2);

/// Optional query parameter for HTMX partial reloads.
#[derive(Deserialize)]
pub struct CalendarQuery {
    partial: Option<String>,
    /// Target month (`YYYY-MM`) for the calendar grid, used by prev/next nav.
    month: Option<String>,
    /// Selected day (`YYYY-MM-DD`) in the monthly planner.
    date: Option<String>,
}

/// Parse a `YYYY-MM` focus month into the first-of-month `NaiveDate`; falls back
/// to today's local date when missing or unparseable.
fn parse_month(s: Option<&str>) -> NaiveDate {
    let today = chrono::Local::now().date_naive();
    if let Some(m) = s {
        if let Ok(d) = NaiveDate::parse_from_str(&format!("{}-01", m), "%Y-%m-%d") {
            return d;
        }
    }
    today
}

/// La selección del mes no se limita a la fecha enfocada: StudyDesk conserva
/// el día elegido al navegar, aunque quede temporalmente fuera del grid.
fn parse_selected_date(s: Option<&str>, fallback: NaiveDate) -> NaiveDate {
    s.and_then(|raw| NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").ok())
        .unwrap_or(fallback)
}

/// Maximum distance (in days) the daily view may navigate away from today.
const MAX_DAY_OFFSET: i64 = 60;

/// Optional query parameters of the daily view.
#[derive(Deserialize)]
pub struct DailyQuery {
    /// Target day (`YYYY-MM-DD`); missing or invalid values fall back to today.
    date: Option<String>,
    /// `list` asks for just the events fragment (HTMX refresh).
    partial: Option<String>,
}

/// Parse the `?date=` parameter of the daily view into a day.
///
/// An unparseable date falls back to today; a valid one is clamped to
/// ±60 days around today, so the view can never wander into the remote past
/// or the far future.
fn parse_target_date(raw: Option<&str>, today: NaiveDate) -> NaiveDate {
    let parsed = raw
        .and_then(|s| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
        .unwrap_or(today);
    let offset = (parsed - today).num_days();
    if offset > MAX_DAY_OFFSET {
        today + chrono::Duration::days(MAX_DAY_OFFSET)
    } else if offset < -MAX_DAY_OFFSET {
        today - chrono::Duration::days(MAX_DAY_OFFSET)
    } else {
        parsed
    }
}

/// Estado de la fase MCP tras el presupuesto de [`TIMEOUT_DATOS`].
struct Carga {
    /// `true` si el MCP respondió con datos: el banner va "conectado".
    conectado: bool,
    /// Eventos del calendario (vacío si el MCP no respondió).
    eventos: Vec<CalendarioEvent>,
    /// `true` si el presupuesto se agotó (el MCP no respondió a tiempo).
    ///
    /// No es lo mismo que "no conectado": aquí el aviso debe decir que
    /// Mikareno *no responde ahora*, no que falta la configuración.
    agotado: bool,
}

/// Resultado interno de la fase MCP, antes de convertirlo en [`Carga`].
enum ResultadoCarga {
    /// El MCP respondió con el calendario.
    Datos(Vec<CalendarioEvent>),
    /// El MCP respondió con un error (tool ausente, fallo del servidor…).
    Fallo,
    /// El presupuesto se agotó: el MCP no responde.
    Agotado,
}

/// Carga el calendario del MCP con un presupuesto acotado.
///
/// Devuelve el estado de la fase MCP:
/// * `conectado = true` → el MCP respondió; `eventos` puede estar vacío si
///   el calendario no trae entradas.
/// * `agotado = true` → el MCP no respondió dentro de [`TIMEOUT_DATOS`]; la
///   vista lo dice con honestidad ("Mikareno no responde ahora") en vez de
///   quedarse colgada.
/// * resto → no hay cliente configurado o el MCP respondió con error.
async fn cargar_calendario(state: &AppState) -> Carga {
    let Some(client) = mikareno_client(state) else {
        return Carga {
            conectado: false,
            eventos: Vec::new(),
            agotado: false,
        };
    };

    // Un único presupuesto para las dos llamadas: el handshake y la lectura
    // del calendario no pueden sumar más de `TIMEOUT_DATOS`.
    let carga = async {
        match time::timeout(TIMEOUT_HANDSHAKE, client.initialize()).await {
            Ok(Ok(())) => {}
            // El handshake falló con error: el MCP respondió (no está colgado).
            Ok(Err(_)) => return ResultadoCarga::Fallo,
            // El handshake se colgó: no hay respuesta dentro del presupuesto.
            Err(_) => return ResultadoCarga::Agotado,
        }
        match client.calendario().await {
            Ok(eventos) => ResultadoCarga::Datos(eventos),
            Err(_) => ResultadoCarga::Fallo,
        }
    };

    match time::timeout(TIMEOUT_DATOS, carga).await {
        Ok(ResultadoCarga::Datos(eventos)) => Carga {
            conectado: true,
            eventos,
            agotado: false,
        },
        Ok(ResultadoCarga::Fallo) => Carga {
            conectado: false,
            eventos: Vec::new(),
            agotado: false,
        },
        // El presupuesto se agotó (en el handshake o leyendo el calendario).
        Ok(ResultadoCarga::Agotado) | Err(_) => Carga {
            conectado: false,
            eventos: Vec::new(),
            agotado: true,
        },
    }
}

/// Aviso honesto cuando el MCP no respondió dentro del presupuesto.
///
/// El banner genérico dice "no conectado", lo cual no es cierto cuando el
/// MCP está lento o saturado: aquí se dice lo que pasó ("no responde ahora")
/// y se ofrece el mismo reintento.
fn aviso_mcp_agotado() -> String {
    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-3 mb-space-md flex flex-col gap-2">
            <div class="flex items-center gap-2">
                <span class="w-2 h-2 rounded-full bg-tertiary"></span>
                <span class="material-symbols-outlined text-[16px] text-outline">wifi_off</span>
                <span class="font-body-sm text-body-sm text-on-surface-variant">● Mikareno no responde ahora — el calendario se muestra vacío: la consulta no terminó en {secs} s</span>
            </div>
            <div class="flex items-center gap-2 pl-6">
                <span class="text-xs text-on-surface-variant">Reintenta en unos segundos.</span>
                <button type="button" onclick="location.reload()" class="inline-flex items-center gap-1 rounded-md border border-outline px-2 py-1 text-on-surface-variant hover:bg-surface-container hover:text-on-surface transition-colors">
                    <span class="material-symbols-outlined text-[14px]">refresh</span>
                    <span class="text-xs">Reintentar</span>
                </button>
            </div>
        </div>"###,
        secs = TIMEOUT_DATOS.as_secs()
    )
}

/// Banner del estado del MCP: el genérico, o el aviso de "no responde ahora"
/// cuando lo que falló fue el presupuesto de tiempo.
fn banner_mcp(carga: &Carga) -> String {
    if carga.agotado {
        aviso_mcp_agotado()
    } else {
        templates::mcp_status_banner(carga.conectado)
    }
}

/// Time of day of an event, when the source delivers one.
///
/// Mikareno truncates its dates to `YYYY-MM-DD`, so today this returns `None`
/// for every event and the view must not invent hours. When a timestamped
/// date shows up (`2026-10-19T08:30:00`) the hour and the exact `HH:MM` are
/// returned and the daily view groups by hour of the day.
fn event_time(fecha: &str) -> Option<(u32, String)> {
    let rest = fecha.get(10..)?.trim_start_matches(['T', ' ']);
    let hhmm: String = rest.chars().take(5).collect();
    let (hh, mm) = hhmm.split_once(':')?;
    let hour: u32 = hh.trim().parse().ok()?;
    let minute: u32 = mm.trim().parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, format!("{:02}:{:02}", hour, minute)))
}

/// Human label of a calendar `tipo` code.
///
/// Unknown codes are shown as they come (underscores as spaces) so nothing
/// is misrepresented; the raw code stays available in the chip's tooltip.
fn tipo_label(tipo: &str) -> String {
    match tipo.trim() {
        "SCHOOL_DAY" => "Día lectivo".to_string(),
        "NON_SCHOOL_DAY" => "Día no lectivo".to_string(),
        "EVENTO" | "" => "Evento".to_string(),
        other => other.replace('_', " "),
    }
}

/// Date of an event in the Spanish format `dd/mm/yyyy`.
fn format_fecha_es(fecha: &str) -> String {
    let iso = fecha.get(..10).unwrap_or(fecha);
    match NaiveDate::parse_from_str(iso, "%Y-%m-%d") {
        Ok(d) => format!("{:02}/{:02}/{:04}", d.day(), d.month(), d.year()),
        Err(_) => fecha.to_string(),
    }
}

/// Number of days of a month.
fn days_in_month(year: i32, month: u32) -> u32 {
    let mut days = 28;
    while NaiveDate::from_ymd_opt(year, month, days + 1).is_some() {
        days += 1;
    }
    days
}

/// Date part (`YYYY-MM-DD`) of an event, also when the source includes a time.
fn day_key(fecha: &str) -> String {
    let f = fecha.trim();
    f.get(..10).unwrap_or(f).to_string()
}

/// Próximos eventos del mes y de las semanas siguientes. Se ordenan por
/// fecha, se omiten los que ya pasaron y cada evento navegable abre su día.
fn upcoming_html(events: &[CalendarioEvent], today: &str) -> String {
    let mut upcoming: Vec<&CalendarioEvent> = events
        .iter()
        .filter(|event| day_key(&event.fecha).as_str() >= today)
        .collect();
    upcoming.sort_by(|a, b| a.fecha.cmp(&b.fecha).then_with(|| a.evento.cmp(&b.evento)));

    if upcoming.is_empty() {
        return r#"<div class="nx-calendar-empty" id="event-timeline"><span class="material-symbols-outlined">event_available</span><p>No hay próximos eventos registrados.</p></div>"#.to_string();
    }

    let today_date = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok();
    let items = upcoming
        .iter()
        .take(5)
        .map(|event| {
            let iso = day_key(&event.fecha);
            let parsed = NaiveDate::parse_from_str(&iso, "%Y-%m-%d").ok();
            let (day, month) = parsed
                .map(|date| {
                    (
                        date.day(),
                        crate::handlers::schedule::mes_es(date.month()).unwrap_or(""),
                    )
                })
                .unwrap_or((0, ""));
            let time = event_time(&event.fecha)
                .map(|(_, hhmm)| hhmm)
                .unwrap_or_else(|| "Sin hora".to_string());
            let body = format!(
                r#"<time class="nx-calendar-event-date" datetime="{}"><strong>{:02}</strong><span>{}</span></time><div class="nx-calendar-event-copy"><span>{}</span><h3>{}</h3><p>{} · {}</p></div><span class="material-symbols-outlined nx-calendar-event-arrow">arrow_outward</span>"#,
                templates::html_escape(&event.fecha),
                day,
                templates::html_escape(month),
                templates::html_escape(&tipo_label(&event.tipo)),
                templates::html_escape(&event.evento),
                templates::html_escape(&templates::weekday_es(&iso)),
                templates::html_escape(&time),
            );
            let navigable = match (parsed, today_date) {
                (Some(date), Some(now)) => {
                    let offset = (date - now).num_days();
                    (0..=MAX_DAY_OFFSET).contains(&offset)
                }
                _ => false,
            };
            if navigable {
                format!(
                    r#"<a class="nx-calendar-event" href="/dashboard/calendar/daily?date={}">{}</a>"#,
                    templates::html_escape(&iso),
                    body
                )
            } else {
                format!(r#"<article class="nx-calendar-event">{}</article>"#, body)
            }
        })
        .collect::<String>();

    format!(r#"<div class="nx-calendar-events" id="event-timeline">{items}</div>"#)
}

/// Etiqueta de las cuatro categorías del planner local.
fn categoria_planner_label(categoria: &str) -> &'static str {
    match categoria {
        "work" => "Trabajo",
        "health" => "Salud",
        "personal" => "Personal",
        _ => "Académico",
    }
}

/// Una entrada local editable del planner seleccionado.
fn planner_entry_html(entry: &calendar_data::Anotacion) -> String {
    let title = if entry.title.trim().is_empty() {
        "Entrada sin título"
    } else {
        entry.title.trim()
    };
    let description = if entry.body.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="agenda-day-entry-description">{}</p>"#,
            templates::html_escape(entry.body.trim())
        )
    };
    format!(
        r#"<li class="agenda-day-entry"><span class="agenda-entry-category agenda-entry-category--{}">{}</span><div class="agenda-day-entry-copy"><h3>{}</h3>{}</div><div class="agenda-day-entry-actions"><button type="button" class="agenda-entry-action" data-calendar-edit="{}" aria-label="Editar {}"><span class="material-symbols-outlined" aria-hidden="true">edit</span></button><button type="button" class="agenda-entry-action is-danger" data-calendar-delete="{}" aria-label="Eliminar {}"><span class="material-symbols-outlined" aria-hidden="true">delete</span></button></div></li>"#,
        templates::html_escape(&entry.category),
        templates::html_escape(categoria_planner_label(&entry.category)),
        templates::html_escape(title),
        description,
        entry.id,
        templates::html_escape(title),
        entry.id,
        templates::html_escape(title),
    )
}

/// Evento local importado, con metadatos iCalendar y opción de eliminar.
fn imported_event_html(entry: &calendar_data::Anotacion, target: NaiveDate) -> String {
    let title = if entry.title.trim().is_empty() {
        "Evento importado"
    } else {
        entry.title.trim()
    };
    let start = NaiveDate::parse_from_str(&entry.date, "%Y-%m-%d").ok();
    let end = entry
        .end_date
        .as_deref()
        .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
    let when = if entry.recurrence.as_deref() == Some("weekly") {
        let day = templates::weekday_es(&target.format("%Y-%m-%d").to_string());
        let time = match (&entry.start_time, &entry.end_time) {
            (Some(start), Some(end)) => format!("{}–{}", start, end),
            (Some(start), None) => start.clone(),
            _ => "Con horario".to_string(),
        };
        format!("Cada semana · {} · {}", day, time)
    } else if entry.all_day {
        let last = end.and_then(|value| value.pred_opt()).or(start);
        match (start, last) {
            (Some(start), Some(last)) if last > start => format!(
                "Día completo · {}–{}",
                format_fecha_es(&start.format("%Y-%m-%d").to_string()),
                format_fecha_es(&last.format("%Y-%m-%d").to_string())
            ),
            _ => "Todo el día".to_string(),
        }
    } else {
        let time = match (&entry.start_time, &entry.end_time) {
            (Some(start), Some(end)) => format!("{}–{}", start, end),
            (Some(start), None) => start.clone(),
            _ => "Con horario".to_string(),
        };
        if start.is_some_and(|date| date < target) {
            format!("Continúa · {}", time)
        } else {
            time
        }
    };
    let description = if entry.body.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="agenda-day-entry-description">{}</p>"#,
            templates::html_escape(entry.body.trim())
        )
    };
    let location = if entry.location.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<small class="agenda-imported-location">Lugar · {}</small>"#,
            templates::html_escape(entry.location.trim())
        )
    };
    format!(
        r#"<li class="agenda-day-feed-item agenda-imported-event"><span class="agenda-day-feed-icon is-imported-event material-symbols-outlined" aria-hidden="true">event_note</span><div><strong>{}</strong><small>{}</small>{}{}</div><button type="button" class="agenda-entry-action is-danger" data-calendar-delete="{}" aria-label="Eliminar evento {}"><span class="material-symbols-outlined" aria-hidden="true">delete</span></button></li>"#,
        templates::html_escape(title),
        templates::html_escape(&when),
        location,
        description,
        entry.id,
        templates::html_escape(title),
    )
}

fn panel_section(label: &str, class_name: &str, items: String) -> String {
    if items.is_empty() {
        return String::new();
    }
    format!(
        r#"<section class="agenda-day-group {}"><h3>{}</h3><ul>{}</ul></section>"#,
        class_name,
        templates::html_escape(label),
        items
    )
}

/// Panel contextual del día: planner editable más los datos de Mikareno que
/// ya alimentan el calendario (entregas, horario y eventos).
fn selected_day_panel(target: NaiveDate, dia: &calendar_data::DiaData) -> String {
    let iso = target.format("%Y-%m-%d").to_string();
    let month = crate::handlers::schedule::mes_es(target.month()).unwrap_or("");
    let label = format!(
        "{} {} de {} de {}",
        templates::weekday_es(&iso),
        target.day(),
        month.to_lowercase(),
        target.year()
    );
    let today = target == chrono::Local::now().date_naive();
    let today_badge = if today {
        r#"<span class="agenda-day-today">Hoy</span>"#
    } else {
        ""
    };

    let planner = dia
        .anotaciones
        .iter()
        .map(planner_entry_html)
        .collect::<String>();
    let imported_events = dia
        .eventos_importados
        .iter()
        .map(|entry| imported_event_html(entry, target))
        .collect::<String>();
    let tasks = dia
        .tareas
        .iter()
        .map(|task| {
            format!(
                r#"<li class="agenda-day-feed-item"><span class="agenda-day-feed-icon is-task material-symbols-outlined" aria-hidden="true">assignment</span><div><strong>{}</strong><small>{}</small></div></li>"#,
                templates::html_escape(if task.titulo.trim().is_empty() { "Tarea" } else { task.titulo.trim() }),
                templates::html_escape(if task.estado.trim().is_empty() { "Entrega registrada en Mikareno" } else { task.estado.trim() }),
            )
        })
        .collect::<String>();
    let classes = dia
        .clases
        .iter()
        .map(|class| {
            format!(
                r#"<li class="agenda-day-feed-item"><span class="agenda-day-feed-icon is-class material-symbols-outlined" aria-hidden="true">school</span><div><strong>{}</strong><small>Horario personal</small></div></li>"#,
                templates::html_escape(class)
            )
        })
        .collect::<String>();
    let events = dia
        .eventos
        .iter()
        .map(|event| {
            let time = event_time(&event.fecha)
                .map(|(_, value)| value)
                .unwrap_or_else(|| "Todo el día".to_string());
            format!(
                r#"<li class="agenda-day-feed-item"><span class="agenda-day-feed-icon is-event material-symbols-outlined" aria-hidden="true">event</span><div><strong>{}</strong><small>{} · {}</small></div></li>"#,
                templates::html_escape(&event.evento),
                templates::html_escape(&tipo_label(&event.tipo)),
                templates::html_escape(&time),
            )
        })
        .collect::<String>();

    let mut sections = String::new();
    sections.push_str(&panel_section("Planificador", "is-planner", planner));
    sections.push_str(&panel_section(
        "Eventos importados",
        "is-imported-event",
        imported_events,
    ));
    sections.push_str(&panel_section("Entregas", "is-task", tasks));
    sections.push_str(&panel_section("Horario", "is-class", classes));
    sections.push_str(&panel_section("Eventos", "is-event", events));
    let empty = if sections.is_empty() {
        r#"<div class="agenda-day-empty"><span class="material-symbols-outlined" aria-hidden="true">event_available</span><p>No hay actividades para este día.</p><span>Añade una entrada al planner cuando quieras.</span></div>"#
    } else {
        ""
    };

    format!(
        r#"<aside id="selected-day-panel" class="agenda-day-panel" data-date="{}" aria-label="Detalle del día seleccionado"><header class="agenda-day-panel-head"><div><p>PLAN DEL DÍA</p><h2>{}</h2>{}</div><button type="button" class="agenda-day-add" data-calendar-add aria-label="Añadir entrada al planner para el {}"><span class="material-symbols-outlined" aria-hidden="true">add</span><span>Añadir</span></button></header><div class="agenda-day-panel-content">{}{}</div></aside>"#,
        iso,
        templates::html_escape(&label),
        today_badge,
        templates::html_escape(&label),
        sections,
        empty,
    )
}

/// GET /dashboard/calendar — vista mensual y planner de StudyDesk con la
/// identidad visual NOVA.
pub async fn monthly(
    State(state): State<Arc<AppState>>,
    query: axum::extract::Query<CalendarQuery>,
) -> Html<String> {
    let datos = calendar_data::datos_mikareno(&state).await;
    let partial = query
        .partial
        .as_deref()
        .filter(|value| value.len() <= MAX_PARTIAL_LEN);
    let focus = parse_month(query.month.as_deref());
    let today_date = chrono::Local::now().date_naive();
    let today = today_date.format("%Y-%m-%d").to_string();
    let selected_date = parse_selected_date(query.date.as_deref(), today_date);
    let selected = selected_date.format("%Y-%m-%d").to_string();
    let (desde, hasta) = rango_mes(focus);
    let anotaciones = calendar_data::anotaciones_en_rango(&state, desde, hasta).await;
    let dias = calendar_data::dias_del_mes(&datos, &anotaciones, focus);

    if partial == Some("day") {
        let anotaciones_dia = if selected_date >= desde && selected_date <= hasta {
            anotaciones.clone()
        } else {
            calendar_data::anotaciones_en_rango(&state, selected_date, selected_date).await
        };
        let dias_dia = calendar_data::dias_del_mes(&datos, &anotaciones_dia, selected_date);
        let dia = dias_dia.get(&selected).cloned().unwrap_or_default();
        return Html(selected_day_panel(selected_date, &dia));
    }

    if partial == Some("timeline") {
        return Html(upcoming_html(&datos.eventos, &today));
    }

    let month_prefix = focus.format("%Y-%m").to_string();
    let mut dias_con_horario = 0usize;
    let mut entregas_mes = 0usize;
    let mut planificadas_mes = 0usize;
    let mut eventos_mes = 0usize;
    let mut eventos_importados = BTreeSet::new();
    for (fecha, dia) in &dias {
        if !fecha.starts_with(&month_prefix) {
            continue;
        }
        dias_con_horario += usize::from(!dia.clases.is_empty());
        entregas_mes += dia.tareas.len();
        planificadas_mes += dia.anotaciones.len();
        eventos_mes += dia.eventos.len();
        eventos_importados.extend(dia.eventos_importados.iter().map(|entry| {
            entry
                .uid
                .as_deref()
                .filter(|uid| !uid.is_empty())
                .map(|uid| format!("uid:{uid}"))
                .unwrap_or_else(|| format!("id:{}", entry.id))
        }));
    }
    eventos_mes += eventos_importados.len();
    let resumen = format!(
        r#"<div id="calendar-summary" class="agenda-summary" aria-label="Resumen del mes" data-month="{month_prefix}"><article><span>Días con horario</span><strong>{dias_con_horario}</strong><small>Clases proyectadas</small></article><article><span>Entregas</span><strong>{entregas_mes}</strong><small>Tareas con fecha</small></article><article><span>Planificadas</span><strong>{planificadas_mes}</strong><small>Entradas personales</small></article><article><span>Eventos</span><strong>{eventos_mes}</strong><small>Escolares e importados</small></article></div>"#
    );
    let grid_html = templates::calendar_grid_html(
        &datos.eventos,
        focus,
        Some(&today),
        Some(&selected),
        Some(&dias),
    );
    let grid_host = format!(
        r#"<div id="calendar-grid" class="agenda-grid-host nx-calendar-grid-host nova-calendar-grid-host" data-month="{}" data-selected-date="{}">{}</div>"#,
        month_prefix, selected, grid_html
    );

    if partial == Some("grid") {
        let summary_oob = resumen.replacen(
            "<div id=\"calendar-summary\"",
            "<div id=\"calendar-summary\" hx-swap-oob=\"outerHTML\"",
            1,
        );
        return Html(format!("{grid_host}{summary_oob}"));
    }

    let anotaciones_dia = if selected_date >= desde && selected_date <= hasta {
        anotaciones.clone()
    } else {
        calendar_data::anotaciones_en_rango(&state, selected_date, selected_date).await
    };
    let dias_dia = calendar_data::dias_del_mes(&datos, &anotaciones_dia, selected_date);
    let dia_seleccionado = dias_dia.get(&selected).cloned().unwrap_or_default();
    let day_panel = selected_day_panel(selected_date, &dia_seleccionado);
    let status = banner_mcp_datos(&datos);
    let timeline = format!(
        r###"<section class="agenda-upcoming nx-upcoming nova-calendar-upcoming" aria-label="Próximos eventos">
            <div class="agenda-upcoming-head"><div><p>A continuación</p><h2>Próximos eventos</h2></div><span class="material-symbols-outlined" aria-hidden="true">upcoming</span></div>
            <p>Lo que viene después, ordenado por fecha.</p>
            {}
            <a class="agenda-upcoming-link" href="/dashboard/calendar/weekly">Ver la semana completa<span class="material-symbols-outlined" aria-hidden="true">arrow_forward</span></a>
        </section>"###,
        upcoming_html(&datos.eventos, &today)
    );
    let today_url = format!(
        "/dashboard/calendar?month={}&date={}",
        today_date.format("%Y-%m"),
        today
    );

    let content = format!(
        r###"<div class="nx-page nx-calendar agenda-page nova-page nova-calendar-page">
          <header class="agenda-header">
            <div class="agenda-heading"><span class="agenda-heading-icon material-symbols-outlined" aria-hidden="true">calendar_month</span><div><p>Agenda académica · 2026—2027</p><h1>Calendario</h1><span>Clases, entregas y eventos en un solo lugar.</span></div></div>
            <div class="agenda-actions"><nav class="agenda-view-tabs" aria-label="Vista del calendario"><a class="is-active" aria-current="page" href="/dashboard/calendar">Mes</a><a href="/dashboard/calendar/weekly">Semana</a><a href="/dashboard/calendar/daily">Día</a></nav><button type="button" class="agenda-plan-button" data-calendar-add><span class="material-symbols-outlined" aria-hidden="true">add</span><span>Planificar</span></button><button type="button" class="agenda-import" data-calendar-ics-import title="Importar archivo ICS"><span class="material-symbols-outlined" aria-hidden="true">upload_file</span><span>Importar ICS</span></button><a class="agenda-export" href="/dashboard/calendar.ics" download title="Descargar calendario ICS"><span class="material-symbols-outlined" aria-hidden="true">download</span><span>Exportar</span></a></div>
          </header>
          <div class="agenda-layout nx-calendar-layout nova-calendar-layout">
            <main class="agenda-main">
              {}
              <section class="agenda-month-card nx-month-panel nova-month-panel" aria-label="Calendario mensual">
                <div class="agenda-month-meta"><div class="agenda-legend" aria-label="Tipos de elemento"><span class="is-task"><i></i>Entregas</span><span class="is-note"><i></i>Planificador</span><span class="is-class"><i></i>Horario</span><span class="is-event"><i></i>Eventos</span></div><a class="agenda-today" href="{}"><span class="material-symbols-outlined" aria-hidden="true">today</span>Hoy</a></div>
                {}
                {}
                <footer class="agenda-month-footer"><span><span class="material-symbols-outlined" aria-hidden="true">touch_app</span>Selecciona un día para ver o planificar actividades.</span><a href="/dashboard/horario">Abrir mi horario<span class="material-symbols-outlined" aria-hidden="true">arrow_outward</span></a></footer>
                </section>
            </main>
            {}
            {}
          </div>
        </div>"###,
        resumen, today_url, status, grid_host, day_panel, timeline,
    );

    Html(
        templates::page_ext_shelled(
            &state,
            "Calendario — Study Platform",
            "Mi agenda",
            "/dashboard/calendar",
            &content,
        )
        .await,
    )
}

/// One event of the selected day, with the timing data the list needs.
struct DayEvent<'a> {
    /// The event as delivered by the source.
    event: &'a CalendarioEvent,
    /// Hour of the day, when the source delivers a time.
    hour: Option<u32>,
    /// Exact time of day (`HH:MM`), when the source delivers a time.
    time: Option<String>,
}

/// Events of one day, ordered by type and title.
fn day_events<'a>(events: &'a [CalendarioEvent], target: &str) -> Vec<DayEvent<'a>> {
    let mut out: Vec<DayEvent<'a>> = events
        .iter()
        .filter(|e| e.fecha.starts_with(target))
        .map(|event| {
            let (hour, time) = match event_time(&event.fecha) {
                Some((h, t)) => (Some(h), Some(t)),
                None => (None, None),
            };
            DayEvent { event, hour, time }
        })
        .collect();
    out.sort_by(|a, b| {
        a.event
            .tipo
            .cmp(&b.event.tipo)
            .then_with(|| a.event.evento.cmp(&b.event.evento))
    });
    out
}

/// One event of the day as a row: title, date and type.
fn day_event_row(item: &DayEvent<'_>) -> String {
    let iso = item.event.fecha.get(..10).unwrap_or(&item.event.fecha);
    let tipo = item.event.tipo.trim();
    let time_html = match &item.time {
        Some(t) => format!(
            r#"<span class="font-label-md text-label-md text-primary-container font-semibold whitespace-nowrap">{}</span>"#,
            templates::html_escape(t)
        ),
        None => String::new(),
    };
    format!(
        r###"<div class="agenda-day-event bg-surface-container border border-outline rounded-xl p-space-sm flex items-start gap-3">
            <span class="agenda-day-event-icon material-symbols-outlined text-[18px] text-primary-container mt-0.5">event</span>
            <div class="agenda-day-event-copy flex-1 min-w-0">
                <div class="font-label-md text-label-md text-on-surface font-semibold">{}</div>
                <div class="font-body-sm text-body-sm text-on-surface-variant">{} · {}</div>
            </div>
            <div class="agenda-day-event-meta flex items-center gap-2 shrink-0">
                {}
                <span class="font-label-sm text-[10px] px-2 py-0.5 rounded-full bg-secondary/10 text-secondary whitespace-nowrap" title="{}">{}</span>
            </div>
        </div>"###,
        templates::html_escape(&item.event.evento),
        templates::weekday_es(iso),
        templates::html_escape(&format_fecha_es(iso)),
        time_html,
        templates::html_escape(tipo),
        templates::html_escape(&tipo_label(tipo)),
    )
}

/// The "now" marker of the daily view.
///
/// With times available it is a divider aligned to the hour rows; without
/// them it is only a clock reading, so it never suggests that the events
/// are placed along a timeline.
fn now_band(now_hm: &str, with_hours: bool) -> String {
    if with_hours {
        format!(
            r###"<div class="agenda-now-band flex items-center gap-2 my-space-sm" title="Hora actual (hora local del servidor)">
                <span class="h-2 w-2 rounded-full bg-primary-container shrink-0"></span>
                <span class="font-label-sm text-label-sm text-primary-container whitespace-nowrap">Ahora · {}</span>
                <span class="flex-1 h-px bg-primary-container/40"></span>
            </div>"###,
            templates::html_escape(now_hm)
        )
    } else {
        format!(
            r###"<div class="agenda-now-band flex items-center gap-2 mb-space-sm" title="Hora actual del servidor (los eventos no tienen horario)">
                <span class="h-2 w-2 rounded-full bg-primary-container shrink-0"></span>
                <span class="font-label-sm text-label-sm text-primary-container whitespace-nowrap">Ahora · {}</span>
                <span class="font-label-sm text-[10px] text-on-surface-variant whitespace-nowrap">solo el reloj del servidor</span>
            </div>"###,
            templates::html_escape(now_hm)
        )
    }
}

/// Honest notice shown when the source does not deliver times of day.
fn no_hours_notice() -> String {
    r###"<div class="agenda-daily-notice bg-surface-container border border-outline rounded-xl p-3 mb-space-md flex items-start gap-2">
        <span class="material-symbols-outlined text-[18px] text-outline mt-0.5">info</span>
        <p class="font-body-sm text-body-sm text-on-surface-variant">Mikareno no entrega horas: se muestran los eventos del día sin horario</p>
    </div>"###
        .to_string()
}

/// Events of the day grouped by hour, with the "now" band on the current
/// hour. Events without a time of day are kept in a final group, so nothing
/// is lost when only part of the source delivers timestamps.
fn hourly_timeline(items: &[DayEvent<'_>], now_hm: &str, now_hour: u32) -> String {
    let mut groups: BTreeMap<u32, Vec<&DayEvent<'_>>> = BTreeMap::new();
    let mut untimed: Vec<&DayEvent<'_>> = Vec::new();
    for item in items {
        match item.hour {
            Some(hour) => groups.entry(hour).or_default().push(item),
            None => untimed.push(item),
        }
    }

    let mut out = String::new();
    let mut band_placed = false;
    for (hour, group) in &groups {
        if !band_placed && *hour > now_hour {
            out.push_str(&now_band(now_hm, true));
            band_placed = true;
        }
        let count = if group.len() == 1 {
            "1 evento".to_string()
        } else {
            format!("{} eventos", group.len())
        };
        out.push_str(&format!(
            r###"<div class="mb-space-md">
                <div class="flex items-center gap-2 mb-space-xs">
                    <span class="font-label-md text-label-md text-primary-container font-semibold">{:02}:00</span>
                    <span class="flex-1 h-px bg-outline/40"></span>
                    <span class="font-label-sm text-[10px] text-on-surface-variant">{}</span>
                </div>
                <div class="space-y-1">{}</div>
            </div>"###,
            hour,
            count,
            group
                .iter()
                .map(|i| day_event_row(i))
                .collect::<String>()
        ));
    }
    if !untimed.is_empty() {
        out.push_str(&format!(
            r###"<div class="mb-space-md">
                <div class="flex items-center gap-2 mb-space-xs">
                    <span class="font-label-md text-label-md text-on-surface-variant font-semibold">Sin hora</span>
                    <span class="flex-1 h-px bg-outline/40"></span>
                </div>
                <div class="space-y-1">{}</div>
            </div>"###,
            untimed
                .iter()
                .map(|i| day_event_row(i))
                .collect::<String>()
        ));
    }
    if !band_placed {
        out.push_str(&now_band(now_hm, true));
    }
    out
}

/// Body of the daily view: the events of the selected day.
///
/// Without times of day the events are listed in order (type, then title)
/// with their date and type, plus the honest notice that says why there is
/// no time column.
fn daily_fragment(items: &[DayEvent<'_>], now_hm: &str, now_hour: u32, with_hours: bool) -> String {
    if items.is_empty() {
        return r###"<div class="agenda-daily-empty bg-surface-container border border-outline rounded-xl p-6 text-center">
            <span class="material-symbols-outlined text-[32px] text-outline mb-2">event_busy</span>
            <p class="font-body-sm text-body-sm text-on-surface-variant">No hay eventos para este día.</p>
        </div>"###
            .to_string();
    }

    let mut out = String::new();
    if with_hours {
        out.push_str(&hourly_timeline(items, now_hm, now_hour));
    } else {
        out.push_str(&no_hours_notice());
        out.push_str(&items.iter().map(|i| day_event_row(i)).collect::<String>());
    }
    out
}

/// Class of the "day navigation" links of the daily view.
const NAV_CLASS: &str = "inline-flex items-center gap-1 px-space-sm py-1.5 rounded-lg border border-outline text-on-surface font-label-sm font-semibold hover:bg-surface-container transition-colors";

/// Class of a navigation link that is disabled at the edge of the window.
///
/// It keeps the box of the enabled links, so the row does not jump when the
/// link turns into a disabled one.
const NAV_CLASS_OFF: &str = "inline-flex items-center gap-1 px-space-sm py-1.5 rounded-lg border border-outline/40 text-outline font-label-sm font-semibold select-none";

/// Day navigation: previous / today / next.
///
/// At the edge of the window (±60 days) the link is disabled instead of
/// pointing to a day the server would clip back to the same one: a link
/// that does not move tells the user nothing.
fn nav_dia(target: NaiveDate, min_date: NaiveDate, max_date: NaiveDate) -> (String, String) {
    let anterior = target - chrono::Duration::days(1);
    let siguiente = target + chrono::Duration::days(1);

    let prev = if anterior < min_date {
        format!(
            r#"<span class="{clase}" aria-disabled="true" title="Límite de navegación: 60 días atrás">‹ Anterior</span>"#,
            clase = NAV_CLASS_OFF
        )
    } else {
        format!(
            r#"<a href="/dashboard/calendar/daily?date={fecha}" class="{clase}" title="Día anterior">‹ Anterior</a>"#,
            fecha = anterior.format("%Y-%m-%d"),
            clase = NAV_CLASS
        )
    };

    let next = if siguiente > max_date {
        format!(
            r#"<span class="{clase}" aria-disabled="true" title="Límite de navegación: 60 días adelante">Siguiente ›</span>"#,
            clase = NAV_CLASS_OFF
        )
    } else {
        format!(
            r#"<a href="/dashboard/calendar/daily?date={fecha}" class="{clase}" title="Día siguiente">Siguiente ›</a>"#,
            fecha = siguiente.format("%Y-%m-%d"),
            clase = NAV_CLASS
        )
    };

    (prev, next)
}

/// GET /dashboard/calendar/daily — the events of one day.
///
/// The day is chosen with `?date=YYYY-MM-DD` (invalid values fall back to
/// today, valid ones are clamped to ±60 days). When the source delivers
/// times of day the events are grouped by hour; when it does not (Mikareno
/// truncates its dates to `YYYY-MM-DD`) the events are listed in order and
/// the view says so, instead of inventing an hour.
pub async fn daily(
    State(state): State<Arc<AppState>>,
    query: axum::extract::Query<DailyQuery>,
) -> Html<String> {
    // MCP con presupuesto acotado: si no responde, la vista degrada a un
    // estado vacío honesto en menos de 5 s en vez de quedarse colgada.
    let carga = cargar_calendario(&state).await;
    let events = &carga.eventos;

    let now = chrono::Local::now();
    let now_hm = now.format("%H:%M").to_string();
    let now_hour = now.hour();
    let today = now.date_naive();

    // Validated target day: invalid → today, clamped to ±60 days.
    let target = parse_target_date(query.date.as_deref(), today);
    let target_iso = target.format("%Y-%m-%d").to_string();
    let is_today = target == today;

    let items = day_events(events, &target_iso);
    let with_hours = items.iter().any(|i| i.hour.is_some());
    let fragment = daily_fragment(&items, &now_hm, now_hour, with_hours);

    // HTMX partial: just the list, for the refresh button.
    if query.partial.as_deref() == Some("list") {
        return Html(fragment);
    }

    let status = banner_mcp(&carga);

    // Day navigation, clamped to the same window the target day is.
    let min_date = today - chrono::Duration::days(MAX_DAY_OFFSET);
    let max_date = today + chrono::Duration::days(MAX_DAY_OFFSET);
    let (prev, next) = nav_dia(target, min_date, max_date);

    let title = if is_today {
        format!("Eventos de hoy — {}", target_iso)
    } else {
        format!("Eventos del {}", target_iso)
    };
    let hoy_chip = if is_today {
        r#"<span class="font-label-sm text-[10px] px-2 py-0.5 rounded-full bg-primary-container/20 text-primary-container font-semibold">HOY</span>"#
    } else {
        ""
    };

    let navigation = format!(
        r###"<div class="nova-daily-nav"><div>{}<a href="/dashboard/calendar/daily" class="{}" title="Volver a hoy">Hoy</a>{}</div><button hx-get="/dashboard/calendar/daily?date={}&partial=list" hx-target="#daily-events" hx-swap="innerHTML" class="{}" title="Actualizar"><span class="material-symbols-outlined">refresh</span>Actualizar</button></div>"###,
        prev, NAV_CLASS, next, target_iso, NAV_CLASS,
    );

    // With times of day the "now" band belongs to the timeline (inside the
    // list, so a refresh keeps it in sync); without them it is only a clock.
    let band = if with_hours {
        String::new()
    } else {
        now_band(&now_hm, false)
    };
    let list = format!(
        r###"<div id="daily-events" class="space-y-1">{}</div>"###,
        fragment
    );

    let content = format!(
        r###"<div class="nx-page nx-daily agenda-page agenda-daily-page nova-page nova-daily-page"><header class="agenda-header"><div class="agenda-heading"><span class="agenda-heading-icon material-symbols-outlined">calendar_view_day</span><div><p>Agenda académica · Vista diaria</p><h1>Día</h1><span>{} · {}</span></div></div><div class="agenda-actions"><nav class="agenda-view-tabs" aria-label="Vista del calendario"><a href="/dashboard/calendar">Mes</a><a href="/dashboard/calendar/weekly">Semana</a><a class="is-active" aria-current="page" href="/dashboard/calendar/daily">Día</a></nav><a class="agenda-export" href="/dashboard/calendar.ics" download title="Descargar calendario ICS"><span class="material-symbols-outlined">download</span><span>Exportar</span></a></div></header><section class="nx-daily-panel nova-daily-panel agenda-daily-board"><div class="agenda-subview-head"><div><p>Tu jornada</p><h2>{}</h2></div>{}</div>{}{}{}{}</section></div>"###,
        templates::weekday_es(&target_iso),
        templates::html_escape(&format_fecha_es(&target_iso)),
        templates::html_escape(&title),
        hoy_chip,
        status,
        navigation,
        band,
        list,
    );
    Html(
        templates::page_ext_shelled(
            &state,
            "Calendario Diario — Study Platform",
            "Mi agenda",
            "/dashboard/calendar",
            &content,
        )
        .await,
    )
}

/// GET /dashboard/calendar/weekly — weekly view.
pub async fn weekly(State(state): State<Arc<AppState>>) -> Html<String> {
    let carga = cargar_calendario(&state).await;
    let events = &carga.eventos;
    let status = banner_mcp(&carga);

    let days = [
        "Lunes",
        "Martes",
        "Miércoles",
        "Jueves",
        "Viernes",
        "Sábado",
        "Domingo",
    ];
    let mut columns: Vec<String> = Vec::new();

    for day in &days {
        let day_events: Vec<&CalendarioEvent> = events
            .iter()
            .filter(|e| templates::weekday_es(&e.fecha) == *day)
            .collect();
        let items = if day_events.is_empty() {
            r#"<p class="nova-weekly-empty">Sin eventos</p>"#.to_string()
        } else {
            day_events
                .iter()
                .map(|e| {
                    let time_label = event_time(&e.fecha)
                        .map(|(_, hhmm)| hhmm)
                        .unwrap_or_else(|| "Sin hora".to_string());
                    format!(
                        r#"<article><span>{}</span><h3>{}</h3><time datetime="{}" title="{}">{}</time></article>"#,
                        templates::html_escape(&tipo_label(&e.tipo)),
                        templates::html_escape(&e.evento),
                        templates::html_escape(&e.fecha),
                        templates::html_escape(&format_fecha_es(&e.fecha)),
                        templates::html_escape(&time_label),
                    )
                })
                .collect::<Vec<_>>()
                .join("")
        };
        columns.push(format!(
            r#"<section class="nova-weekly-day"><header><span>{}</span><strong>{}</strong></header><div>{}</div></section>"#,
            day.chars().take(3).collect::<String>(),
            day,
            items,
        ));
    }

    let content = format!(
        r###"<div class="nx-page nx-weekly agenda-page agenda-weekly-page nova-page nova-weekly-page"><header class="agenda-header"><div class="agenda-heading"><span class="agenda-heading-icon material-symbols-outlined">calendar_view_week</span><div><p>Agenda académica · Vista semanal</p><h1>Semana</h1><span>Los eventos se agrupan por día usando los datos reales del calendario.</span></div></div><div class="agenda-actions"><nav class="agenda-view-tabs" aria-label="Vista del calendario"><a href="/dashboard/calendar">Mes</a><a class="is-active" aria-current="page" href="/dashboard/calendar/weekly">Semana</a><a href="/dashboard/calendar/daily">Día</a></nav><a class="agenda-export" href="/dashboard/calendar.ics" download title="Descargar calendario ICS"><span class="material-symbols-outlined">download</span><span>Exportar</span></a></div></header><section class="nx-schedule-board nova-schedule-board agenda-weekly-board"><div class="agenda-subview-head"><div><p>Tu semana</p><h2>Eventos por día</h2></div><span class="material-symbols-outlined">calendar_view_week</span></div>{}<div class="nx-weekly-agenda nova-weekly-agenda">{}</div></section></div>"###,
        status,
        columns.join(""),
    );
    Html(
        templates::page_ext_shelled(
            &state,
            "Semana — Study Platform",
            "Mi agenda",
            "/dashboard/calendar",
            &content,
        )
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        daily_fragment, day_events, event_time, nav_dia, parse_selected_date, parse_target_date,
        rango_mes, templates, tipo_label,
    };
    use crate::integrations::mikareno::CalendarioEvent;
    use chrono::NaiveDate;

    fn event(fecha: &str, evento: &str) -> CalendarioEvent {
        CalendarioEvent {
            fecha: fecha.to_string(),
            evento: evento.to_string(),
            tipo: "SCHOOL_DAY".to_string(),
        }
    }

    #[test]
    fn calendar_range_uses_six_complete_weeks_for_stable_month_layout() {
        let focus = NaiveDate::from_ymd_opt(2027, 2, 1).unwrap();
        let (start, end) = rango_mes(focus);
        assert_eq!(start, NaiveDate::from_ymd_opt(2027, 2, 1).unwrap());
        assert_eq!(end, NaiveDate::from_ymd_opt(2027, 3, 14).unwrap());
        assert_eq!((end - start).num_days() + 1, 42);
    }

    #[test]
    fn test_parse_target_date_validates_and_clamps() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        // Missing or invalid dates fall back to today.
        assert_eq!(parse_target_date(None, today), today);
        assert_eq!(parse_target_date(Some("no-es-una-fecha"), today), today);
        assert_eq!(parse_target_date(Some("2026-13-40"), today), today);
        // A valid date is kept.
        assert_eq!(
            parse_target_date(Some("2026-10-01"), today),
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
        );
        // Beyond ±60 days the day is clamped to the window.
        assert_eq!(
            parse_target_date(Some("2026-12-31"), today),
            NaiveDate::from_ymd_opt(2026, 12, 18).unwrap()
        );
        assert_eq!(
            parse_target_date(Some("2026-01-01"), today),
            NaiveDate::from_ymd_opt(2026, 8, 20).unwrap()
        );
    }

    #[test]
    fn test_event_time_only_when_the_source_has_a_time() {
        // Mikareno truncates its dates: no time means no hour.
        assert!(event_time("2026-10-19").is_none());
        assert!(event_time("2026-10-19T").is_none());
        // With a timestamp the hour and the exact HH:MM come out.
        assert_eq!(
            event_time("2026-10-19T08:30:00"),
            Some((8, "08:30".to_string()))
        );
        assert_eq!(
            event_time("2026-10-19 08:30"),
            Some((8, "08:30".to_string()))
        );
    }

    #[test]
    fn test_day_events_filters_the_day_and_orders_the_list() {
        let events = vec![
            event("2026-10-19", "B"),
            event("2026-10-19", "A"),
            event("2026-10-20", "C"),
        ];
        let items = day_events(&events, "2026-10-19");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].event.evento, "A");
        assert_eq!(items[1].event.evento, "B");
        assert!(items.iter().all(|i| i.hour.is_none()));
    }

    #[test]
    fn test_daily_fragment_without_hours_says_so_and_invents_nothing() {
        let events = vec![
            event("2026-10-19", "Reunión de Tutoría"),
            event("2026-10-19", "Excursión"),
        ];
        let items = day_events(&events, "2026-10-19");
        let html = daily_fragment(&items, "19:30", 19, false);
        assert!(html.contains("Mikareno no entrega horas"));
        assert!(html.contains("Reunión de Tutoría"));
        assert!(html.contains("Excursión"));
        assert!(html.contains("19/10/2026"));
        assert!(html.contains("Día lectivo"));
        // No hour is invented and no timeline is suggested.
        assert!(!html.contains("19:00"));
        assert!(!html.contains("Ahora ·"));
    }

    #[test]
    fn test_daily_fragment_with_hours_groups_by_hour() {
        let events = vec![
            CalendarioEvent {
                fecha: "2026-10-19T08:30:00".to_string(),
                evento: "Matemáticas".to_string(),
                tipo: "SCHOOL_DAY".to_string(),
            },
            CalendarioEvent {
                fecha: "2026-10-19T10:00:00".to_string(),
                evento: "Lengua".to_string(),
                tipo: "EVENTO".to_string(),
            },
        ];
        let items = day_events(&events, "2026-10-19");
        let html = daily_fragment(&items, "09:15", 9, true);
        // The "now" band sits between the 08:00 and the 10:00 groups.
        let at_08 = html.find("08:00").expect("grupo de las 08:00");
        let at_now = html.find("Ahora · 09:15").expect("banda de ahora");
        let at_10 = html.find("10:00").expect("grupo de las 10:00");
        assert!(at_08 < at_now && at_now < at_10);
        assert!(html.contains("08:30"));
        // No notice: the source did deliver times.
        assert!(!html.contains("Mikareno no entrega horas"));
    }

    #[test]
    fn test_nav_dia_is_disabled_at_the_edge_of_the_window() {
        let min_date = NaiveDate::from_ymd_opt(2026, 8, 20).unwrap();
        let max_date = NaiveDate::from_ymd_opt(2026, 12, 18).unwrap();

        // In the middle both links move.
        let (prev, next) = nav_dia(
            NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(),
            min_date,
            max_date,
        );
        assert!(prev.contains("date=2026-10-18"));
        assert!(next.contains("date=2026-10-20"));

        // At the edges the link is disabled, not a link to the same day.
        let (prev, next) = nav_dia(max_date, min_date, max_date);
        assert!(prev.contains("date=2026-12-17"));
        assert!(next.contains("aria-disabled=\"true\""));
        assert!(!next.contains("<a href"));

        let (prev, next) = nav_dia(min_date, min_date, max_date);
        assert!(prev.contains("aria-disabled=\"true\""));
        assert!(!prev.contains("<a href"));
        assert!(next.contains("date=2026-08-21"));
    }
}
