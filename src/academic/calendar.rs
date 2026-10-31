//! Calendar views (monthly, weekly, daily).

use axum::{extract::State, response::Html};
use chrono::{Datelike, NaiveDate, Timelike};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::time::{self, Duration};

use crate::academic::{calendar_data, mikareno_client, templates};
use crate::integrations::mikareno::{CalendarioEvent, Horario, Tarea};
use crate::AppState;

/// Rango de fechas que cubre la rejilla del mes: el mes visible más los días
/// de los meses vecinos que se muestran atenuados.
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
    // La rejilla empieza en lunes y termina en domingo: se toma una semana de
    // margen a cada lado para que ningún día atenuado quede fuera.
    (
        primero - chrono::Duration::days(7),
        fin_mes + chrono::Duration::days(7),
    )
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

/// Class the shared month-grid template puts on every numbered day cell.
const CELL_MARKER: &str = "calendar-cell";

/// Optional query parameter for HTMX partial reloads.
#[derive(Deserialize)]
pub struct CalendarQuery {
    partial: Option<String>,
    /// Target month (`YYYY-MM`) for the calendar grid, used by prev/next nav.
    month: Option<String>,
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

/// Maximum distance (in days) the daily view may navigate away from today.
const MAX_DAY_OFFSET: i64 = 60;

/// Blank cell the shared month grid renders for the days that do not belong
/// to the focused month (it carries no day number).
const BLANK_CELL: &str = r#"<div class="border border-outline/30 rounded-lg"></div>"#;

/// Class of the cells this module renders for the days of the neighbouring
/// months. Deliberately different from [`CELL_MARKER`] so the badge
/// injection never reads a neighbouring day as a day of the focused month.
const OUTSIDE_CELL_CLASS: &str = "calendar-outside-cell";

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

/// Events of a single day, aggregated for the month-grid counter badge.
#[derive(Default)]
struct DayBucket {
    /// How many events the day has.
    count: usize,
    /// Titles of the day, in the order the API returned them.
    titles: Vec<String>,
}

/// Date part (`YYYY-MM-DD`) of an event's date, used as the bucket key.
///
/// The source normally truncates the date, but it may also deliver a
/// timestamp (`2026-10-19T08:30:00`). The grid looks the days up by date,
/// so the key is always the date part: otherwise a day with a timestamped
/// event would lose its badge.
fn day_key(fecha: &str) -> String {
    let f = fecha.trim();
    f.get(..10).unwrap_or(f).to_string()
}

/// Group calendar events by their ISO date (`YYYY-MM-DD`).
fn bucket_by_day(events: &[CalendarioEvent]) -> HashMap<String, DayBucket> {
    let mut buckets: HashMap<String, DayBucket> = HashMap::new();
    for ev in events {
        let bucket = buckets.entry(day_key(&ev.fecha)).or_default();
        bucket.count += 1;
        bucket.titles.push(ev.evento.clone());
    }
    buckets
}

/// Discreet counter badge of a day cell: absolutely positioned, so it never
/// pushes the event chips down, with the day's titles in the tooltip.
fn badge_html(bucket: &DayBucket) -> String {
    let titles = bucket.titles.join(" · ");
    format!(
        r###"<span class="calendar-event-count absolute top-1 right-1 inline-flex items-center justify-center min-w-[16px] h-[16px] px-1 rounded-full bg-primary-container/20 text-primary-container text-[9px] font-semibold leading-none" title="{}">{}</span>"###,
        templates::html_escape(&titles),
        bucket.count
    )
}

/// Read the day number of a grid cell from the first `<span>` it holds (the
/// one with the day of the month). `None` when the markup is not the expected
/// one — the caller then leaves the cell untouched.
fn cell_day_number(cell: &str) -> Option<u32> {
    let span_at = cell.find("<span")?;
    let after_span = &cell[span_at..];
    // End of the opening tag of that span.
    let tag_end = after_span.find('>')? + 1;
    let body = &after_span[tag_end..];
    let body_end = body.find("</span>")?;
    body[..body_end].trim().parse::<u32>().ok()
}

/// Inject the counter badge into every month cell that has events.
///
/// The rendered grid is walked cell by cell (cells carry the `calendar-cell`
/// class); the day number of each cell decides which bucket of events it
/// gets. If the markup ever changes and no day number can be read, the grid
/// is returned unchanged.
fn inject_event_badges(
    grid: &str,
    year: i32,
    month: u32,
    buckets: &HashMap<String, DayBucket>,
) -> String {
    let mut out = String::with_capacity(grid.len() + 256);
    let mut rest = grid;

    loop {
        let Some(at) = rest.find(CELL_MARKER) else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..at]);
        let cell = &rest[at..];

        // End of the opening tag of the cell — the badge goes right after it.
        let Some(tag_end) = cell.find('>') else {
            out.push_str(cell);
            return out;
        };
        // Body of this cell, up to the next cell marker.
        let body_end = cell[1..]
            .find(CELL_MARKER)
            .map(|pos| pos + 1)
            .unwrap_or(cell.len());

        out.push_str(&cell[..tag_end + 1]);

        if let Some(bucket) = cell_day_number(&cell[..body_end])
            .and_then(|day| buckets.get(&format!("{:04}-{:02}-{:02}", year, month, day)))
        {
            out.push_str(&badge_html(bucket));
        }

        rest = &cell[tag_end + 1..];
    }
}

/// Cell of a day that does not belong to the focused month: dimmed, so the
/// focused month reads as the highlighted one.
///
/// A day with events also carries the counter badge (dimmed with the cell),
/// so a busy day of a neighbouring month is not mistaken for a free one.
fn outside_cell_html(day: u32, bucket: Option<&DayBucket>) -> String {
    let badge = bucket.map(badge_html).unwrap_or_default();
    format!(
        r###"<div class="{} agenda-outside relative h-32 border border-outline/20 rounded-lg p-space-xs bg-surface-container/30 opacity-50">{badge}<span class="calendar-day-number block font-label-sm text-[10px] text-on-surface-variant">{}</span></div>"###,
        OUTSIDE_CELL_CLASS, day
    )
}

/// Fill the blank cells of the month grid with the days of the neighbouring
/// months, dimmed, so the focused month stands out.
///
/// The shared grid leaves the padding cells empty; here each one is replaced
/// by a cell with the day number of the month it belongs to (the cells
/// before the 1st belong to the previous month, the ones after the last day
/// to the next one). The new cells carry a class of their own, so the badge
/// injection never reads their day number as a day of the focused month.
///
/// A padding day that has events of its own gets the counter badge, looked
/// up in the bucket of *its* date (the month it really belongs to, not the
/// focused one).
fn render_outside_days(
    grid: &str,
    focus: NaiveDate,
    buckets: &HashMap<String, DayBucket>,
) -> String {
    let (year, month) = (focus.year(), focus.month());
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(focus);
    // Weekday of the 1st (Mon=0 … Sun=6): that many cells precede the 1st.
    let leading = first.weekday().num_days_from_monday() as usize;
    let last = NaiveDate::from_ymd_opt(year, month, days_in_month(year, month)).unwrap_or(first);

    let mut out = String::with_capacity(grid.len() + 512);
    let mut rest = grid;
    let mut index = 0usize;

    while let Some(at) = rest.find(BLANK_CELL) {
        out.push_str(&rest[..at]);
        // The leading cells count backwards from the 1st of the focused month
        // (they are the last days of the previous one); the trailing ones
        // count forwards from the day after its last (the next month).
        let fecha = if index < leading {
            first.checked_sub_signed(chrono::Duration::days((leading - index) as i64))
        } else {
            last.checked_add_signed(chrono::Duration::days((index - leading + 1) as i64))
        };
        let bucket = fecha
            .as_ref()
            .and_then(|f| buckets.get(&f.format("%Y-%m-%d").to_string()));
        let dia = fecha.map(|f| f.day()).unwrap_or(0);
        out.push_str(&outside_cell_html(dia, bucket));
        rest = &rest[at + BLANK_CELL.len()..];
        index += 1;
    }
    out.push_str(rest);
    out
}

/// Mark the current month in the header of the month grid.
///
/// The shared template renders the month label in a `<span>` with the
/// `font-headline-sm` class; a "Mes actual" chip is injected right after it
/// when the focused month is the one we are living in. If the markup ever
/// changes, the grid is returned unchanged.
fn mark_current_month(grid: &str, focus: NaiveDate, today: NaiveDate) -> String {
    if focus.year() != today.year() || focus.month() != today.month() {
        return grid.to_string();
    }
    const LABEL_CLASS: &str = "font-headline-sm";
    let Some(span_at) = grid.find(LABEL_CLASS) else {
        return grid.to_string();
    };
    // End of the opening tag of the label.
    let Some(tag_end) = grid[span_at..].find('>') else {
        return grid.to_string();
    };
    let body_start = span_at + tag_end + 1;
    // End of the label itself: the chip goes right after its closing tag.
    let Some(close_rel) = grid[body_start..].find("</span>") else {
        return grid.to_string();
    };
    let insert_at = body_start + close_rel + "</span>".len();
    let chip = r#"<span class="calendar-current-month font-label-sm text-[10px] px-2 py-0.5 rounded-full bg-primary-container/20 text-primary-container font-semibold whitespace-nowrap ml-2">Mes actual</span>"#;
    let mut out = String::with_capacity(grid.len() + chip.len());
    out.push_str(&grid[..insert_at]);
    out.push_str(chip);
    out.push_str(&grid[insert_at..]);
    out
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

/// GET /dashboard/calendar — Google-Calendar-style monthly view.
///
/// La fuente de datos es Mikareno (misma que el panel del dashboard) a través
/// de [`calendar_data`], que_cachea el resultado: la navegación entre meses
/// no vuelve a pagar el scrape. Cada día puede mostrar clases del horario,
/// eventos escolares, tareas que vencen y las anotaciones del usuario.
///
/// El partial `grid` devuelve **sólo el cuerpo de la rejilla** (sin la
/// cabecera de navegación): así el swap de HTMX nunca puede anidar la
/// rejilla dentro de sí misma (el bug que hacía que el calendario creciera
/// hacia abajo y se encogiera a la mitad con cada clic).
pub async fn monthly(
    State(state): State<Arc<AppState>>,
    query: axum::extract::Query<CalendarQuery>,
) -> Html<String> {
    // Datos de Mikareno (cacheados; el refresh va en background).
    let datos = calendar_data::datos_mikareno(&state).await;

    // Partial reload — return just the requested fragment (overlong values ignored).
    let partial = query
        .partial
        .as_deref()
        .filter(|s| s.len() <= MAX_PARTIAL_LEN);
    let focus = parse_month(query.month.as_deref());
    let today_date = chrono::Local::now().date_naive();
    let today = today_date.format("%Y-%m-%d").to_string();

    // Rango del mes visible (incluye los días de los meses vecinos que la
    // rejilla muestra atenuados).
    let (desde, hasta) = rango_mes(focus);

    // Anotaciones locales del usuario en el rango.
    let anotaciones = calendar_data::anotaciones_en_rango(&state, desde, hasta).await;

    // Mapa día → contenido.
    let dias = calendar_data::dias_del_mes(&datos, &anotaciones, focus);

    // Month grid, with the per-cell event counter badge.
    let buckets = bucket_by_day(&datos.eventos);
    let grid_html = inject_event_badges(
        &templates::calendar_grid_html(&datos.eventos, focus, Some(&today), Some(&dias)),
        focus.year(),
        focus.month(),
        &buckets,
    );
    // Days of the neighbouring months, dimmed, and the current month marked.
    let grid_html = mark_current_month(
        &render_outside_days(&grid_html, focus, &buckets),
        focus,
        today_date,
    );

    // HTMX partial: swap just the grid (used by the month nav buttons).
    //
    // El swap es `outerHTML` sobre `#calendar-grid`, así que la respuesta es
    // el elemento **completo** (con su id). Así el HTMX reemplaza la rejilla
    // en vez de meterla dentro: es imposible que la rejilla se anide en sí
    // misma, que era lo que hacía que el calendario creciera hacia abajo y se
    // encogiera a la mitad con cada clic.
    if partial == Some("grid") {
        return Html(format!(
            r###"<div id="calendar-grid" class="agenda-grid-host nova-calendar-grid-host">{}</div>"###,
            grid_html
        ));
    } // HTMX partial: swap just the timeline.
    if partial == Some("timeline") {
        return Html(upcoming_html(&datos.eventos, &today));
    }

    let month_prefix = focus.format("%Y-%m").to_string();
    let mut dias_con_horario = 0usize;
    let mut entregas_mes = 0usize;
    let mut eventos_mes = 0usize;
    for (fecha, dia) in &dias {
        if !fecha.starts_with(&month_prefix) {
            continue;
        }
        dias_con_horario += usize::from(!dia.clases.is_empty());
        entregas_mes += dia.tareas.len();
        eventos_mes += dia.eventos.len();
    }
    let resumen = format!(
        r#"<div class="agenda-summary" aria-label="Resumen del mes"><article><span>Días con horario</span><strong>{dias_con_horario}</strong><small>Bloques lectivos del mes</small></article><article><span>Entregas</span><strong>{entregas_mes}</strong><small>Tareas con fecha</small></article><article><span>Eventos</span><strong>{eventos_mes}</strong><small>Actividades registradas</small></article></div>"#
    );

    let status = banner_mcp_datos(&datos);
    let timeline = format!(
        r###"<aside class="agenda-upcoming nx-upcoming nova-calendar-upcoming">
            <div class="agenda-upcoming-head"><div><p>A continuación</p><h2>Próximos eventos</h2></div><span class="material-symbols-outlined">upcoming</span></div>
            <p>Lo que viene después, ordenado por fecha.</p>
            {}
            <a class="agenda-upcoming-link" href="/dashboard/calendar/weekly">Ver la semana completa<span class="material-symbols-outlined">arrow_forward</span></a>
        </aside>"###,
        upcoming_html(&datos.eventos, &today)
    );

    let content = format!(
        r###"<div class="nx-page nx-calendar agenda-page nova-page nova-calendar-page">
          <header class="agenda-header">
            <div class="agenda-heading"><span class="agenda-heading-icon material-symbols-outlined">calendar_month</span><div><p>Agenda académica · 2026—2027</p><h1>Calendario</h1><span>Clases, entregas y eventos en un solo lugar.</span></div></div>
            <div class="agenda-actions"><nav class="agenda-view-tabs" aria-label="Vista del calendario"><a class="is-active" aria-current="page" href="/dashboard/calendar">Mes</a><a href="/dashboard/calendar/weekly">Semana</a><a href="/dashboard/calendar/daily">Día</a></nav><a class="agenda-export" href="/dashboard/calendar.ics" download title="Descargar calendario ICS"><span class="material-symbols-outlined">download</span><span>Exportar</span></a></div>
          </header>
          <div class="agenda-layout nx-calendar-layout nova-calendar-layout">
            <main class="agenda-main">
              {}
              <section class="agenda-month-card nx-month-panel nova-month-panel" aria-label="Calendario mensual">
                <div class="agenda-month-meta"><div class="agenda-legend" aria-label="Tipos de elemento"><span class="is-class"><i></i>Horario</span><span class="is-task"><i></i>Entregas</span><span class="is-event"><i></i>Eventos</span><span class="is-note"><i></i>Notas</span></div><a class="agenda-today" href="/dashboard/calendar?month={}"><span class="material-symbols-outlined">today</span>Mes actual</a></div>
                {}
                <div id="calendar-grid" class="agenda-grid-host nx-calendar-grid-host nova-calendar-grid-host">{}</div>
                <footer class="agenda-month-footer"><span><span class="material-symbols-outlined">edit_calendar</span>Selecciona un día para añadir o editar anotaciones.</span><a href="/dashboard/horario">Abrir mi horario<span class="material-symbols-outlined">arrow_outward</span></a></footer>
              </section>
            </main>
            {}
          </div>
        </div>"###,
        resumen,
        today_date.format("%Y-%m"),
        status,
        grid_html,
        timeline,
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
        bucket_by_day, daily_fragment, day_events, event_time, inject_event_badges,
        mark_current_month, nav_dia, parse_target_date, render_outside_days, templates, tipo_label,
        BLANK_CELL, OUTSIDE_CELL_CLASS,
    };
    use crate::integrations::mikareno::CalendarioEvent;
    use chrono::NaiveDate;
    use std::collections::HashMap;

    fn event(fecha: &str, evento: &str) -> CalendarioEvent {
        CalendarioEvent {
            fecha: fecha.to_string(),
            evento: evento.to_string(),
            tipo: "SCHOOL_DAY".to_string(),
        }
    }

    /// Cell markup as rendered by the shared month-grid template.
    fn cell_html(day: u32) -> String {
        format!(
            r#"<div class="calendar-cell relative h-32 border border-outline/20 rounded-lg p-space-xs bg-surface-container/30"><span class="block font-label-sm text-[10px] text-on-surface-variant mb-space-xs">{}</span><div class="mt-space-xs space-y-0.5 overflow-hidden"></div></div>"#,
            day
        )
    }

    #[test]
    fn test_badge_counts_events_of_the_day() {
        let events = vec![
            event("2024-03-01", "Clase de Matemáticas"),
            event("2024-03-01", "Examen de Lengua"),
            event("2024-03-03", "Reunión <b>padres</b>"),
        ];
        let buckets = bucket_by_day(&events);
        let grid = format!("{} {}", cell_html(1), cell_html(3));
        let html = inject_event_badges(&grid, 2024, 3, &buckets);

        assert_eq!(html.matches("min-w-[16px]").count(), 2);
        assert!(html.contains(">2</span>"));
        assert!(html.contains(">1</span>"));
        assert!(html.contains("title=\"Clase de Matemáticas · Examen de Lengua\""));
        assert!(html.contains("title=\"Reunión &lt;b&gt;padres&lt;/b&gt;\""));
    }

    #[test]
    fn test_no_badge_without_events() {
        let grid = cell_html(1);
        let html = inject_event_badges(&grid, 2024, 3, &HashMap::new());
        assert_eq!(html, grid);
    }

    #[test]
    fn test_unexpected_markup_is_kept_untouched() {
        let grid = "<div class=\"calendar-cell\">sin número</div>";
        let buckets = bucket_by_day(&[event("2024-03-01", "Evento")]);
        let html = inject_event_badges(grid, 2024, 3, &buckets);
        assert!(!html.contains("min-w-[16px]"));
        assert!(html.contains("sin número"));
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
    fn test_render_outside_days_fills_the_padding_cells() {
        // January 2026 starts on a Thursday: 3 cells precede the 1st.
        let focus = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let grid = BLANK_CELL.repeat(5);
        let html = render_outside_days(&grid, focus, &HashMap::new());
        // The leading cells are the last days of December 2025...
        assert!(html.contains(">29</span>"));
        assert!(html.contains(">30</span>"));
        assert!(html.contains(">31</span>"));
        // ...and the trailing ones the first days of February 2026.
        assert!(html.contains(">1</span>"));
        assert!(html.contains(">2</span>"));
        // Every padding cell became a dimmed cell of its own.
        assert!(!html.contains(BLANK_CELL));
        assert_eq!(html.matches(OUTSIDE_CELL_CLASS).count(), 5);
        assert_eq!(html.matches("opacity-50").count(), 5);
    }

    #[test]
    fn test_outside_day_with_events_gets_its_badge() {
        // A day of a neighbouring month that has events is not shown as a
        // free day: it carries the counter badge of its own date.
        let focus = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let events = vec![
            event("2025-12-31", "Clase"),
            event("2025-12-30", "Clase"),
            event("2025-12-30", "Examen"),
        ];
        let buckets = bucket_by_day(&events);
        let grid = BLANK_CELL.repeat(3);
        let html = render_outside_days(&grid, focus, &buckets);
        // Two badges: the 31st (1 event) and the 30th (2 events).
        assert_eq!(html.matches("min-w-[16px]").count(), 2);
        assert!(html.contains(">2</span>"));
        assert!(html.contains("title=\"Clase · Examen\""));
    }

    #[test]
    fn test_mark_current_month_only_marks_the_month_we_live_in() {
        let grid = r#"<span class="font-headline-sm text-headline-sm text-primary-container">Enero 2026</span>"#;
        let today = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let marked = mark_current_month(grid, today, today);
        assert!(marked.contains("Mes actual"));
        // Another month is left untouched.
        let other = mark_current_month(grid, NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(), today);
        assert_eq!(other, grid);
    }

    #[test]
    fn test_month_grid_end_to_end() {
        // October 2026 starts on a Thursday: 3 cells precede the 1st.
        let focus = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let today = "2026-10-19";
        let events = vec![
            event("2026-10-19", "Reunión"),
            event("2026-10-19", "Examen"),
        ];
        let grid = templates::calendar_grid_html(&events, focus, Some(today), None);
        let buckets = bucket_by_day(&events);
        let grid = inject_event_badges(&grid, 2026, 10, &buckets);
        let grid = render_outside_days(&grid, focus, &buckets);
        let grid = mark_current_month(&grid, focus, focus);

        // The days of the neighbouring months are shown, dimmed.
        assert!(grid.contains(">28</span>"), "28 de septiembre");
        assert!(grid.contains(">30</span>"), "30 de septiembre");
        assert!(grid.contains(">1</span>"), "1 de noviembre");
        assert!(!grid.contains(BLANK_CELL), "no queda ninguna celda vacía");
        // The badge stays on the 19th, never on a neighbouring day.
        assert_eq!(grid.matches("min-w-[16px]").count(), 1);
        // The current month is marked, right after the label of the grid.
        let chip_at = grid.find("Mes actual").expect("chip del mes actual");
        let chip_tag = grid[..chip_at].rfind("<span").expect("etiqueta del chip");
        assert!(grid[..chip_tag].ends_with("</span>"));
    }

    #[test]
    fn test_tipo_label_translates_known_codes() {
        assert_eq!(tipo_label("SCHOOL_DAY"), "Día lectivo");
        assert_eq!(tipo_label("NON_SCHOOL_DAY"), "Día no lectivo");
        assert_eq!(tipo_label("EVENTO"), "Evento");
        assert_eq!(tipo_label(""), "Evento");
        // Unknown codes are shown as they come.
        assert_eq!(tipo_label("SPECIAL_DAY"), "SPECIAL DAY");
    }

    #[test]
    fn test_bucket_by_day_keys_by_date_not_by_timestamp() {
        // Un evento con hora sigue belonging to its day: the badge of the
        // cell must find it.
        let events = vec![
            event("2026-10-19T08:30:00", "Clase de Matemáticas"),
            event("2026-10-19T10:00:00", "Examen"),
        ];
        let buckets = bucket_by_day(&events);
        let bucket = buckets.get("2026-10-19").expect("clave por fecha");
        assert_eq!(bucket.count, 2);
        // Y la rejilla lo encuentra por la fecha del mes.
        let grid = cell_html(19);
        let html = inject_event_badges(&grid, 2026, 10, &buckets);
        assert_eq!(html.matches("min-w-[16px]").count(), 1);
        assert!(html.contains(">2</span>"));
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
