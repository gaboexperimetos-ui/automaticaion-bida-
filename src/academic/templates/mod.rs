//! HTML template helpers for academic dashboard.
//! Shared styling: Synthetic Cognition (Stitch) design system over HTMX + Tailwind.

use crate::integrations::mikareno::{CalendarioEvent, HorarioDia, Nota, Perfil};
use std::sync::Arc;

use crate::AppState;

/// Stitch `tailwind.config` extending Tailwind with the full Synthetic Cognition
/// color token set, typography scale (Space Grotesk headlines / DM Sans body),
/// shape tokens, spacing scale and blur preset.
const STITCH_TAILWIND_CONFIG: &str = r###"tailwind.config = {
    darkMode: "class",
    theme: {
        extend: {
            colors: {
                "background": "#090b11",
                "on-background": "#f2f4f8",
                "surface": "#090b11",
                "surface-dim": "#07090e",
                "surface-bright": "#242c3b",
                "surface-container-lowest": "#090c12",
                "surface-container-low": "#0d1017",
                "surface-container": "#11151d",
                "surface-container-high": "#161b25",
                "surface-container-highest": "#1c2230",
                "surface-variant": "#1c2230",
                "on-surface": "#f2f4f8",
                "on-surface-variant": "#a2abbc",
                "inverse-surface": "#f2f4f8",
                "inverse-on-surface": "#121728",
                "outline": "#7d8698",
                "outline-variant": "#2a3242",
                "surface-tint": "#a9b6ff",
                "primary": "#dce1ff",
                "on-primary": "#121728",
                "primary-container": "#a9b6ff",
                "on-primary-container": "#121728",
                "inverse-primary": "#7284ec",
                "primary-fixed": "#dce1ff",
                "primary-fixed-dim": "#a9b6ff",
                "on-primary-fixed": "#121728",
                "on-primary-fixed-variant": "#121728",
                "secondary": "#76d7c0",
                "on-secondary": "#0b2420",
                "secondary-container": "#1f4a40",
                "on-secondary-container": "#d4f8ed",
                "secondary-fixed": "#b8ecdf",
                "secondary-fixed-dim": "#76d7c0",
                "on-secondary-fixed": "#0b2420",
                "on-secondary-fixed-variant": "#1f4a40",
                "tertiary": "#eab96f",
                "on-tertiary": "#2d1e08",
                "tertiary-container": "#eab96f",
                "on-tertiary-container": "#543d1d",
                "tertiary-fixed": "#f7d9a7",
                "tertiary-fixed-dim": "#eab96f",
                "on-tertiary-fixed": "#2d1e08",
                "on-tertiary-fixed-variant": "#543d1d",
                "error": "#ffb4ab",
                "on-error": "#690005",
                "error-container": "#93000a",
                "on-error-container": "#ffdad6"
            },
            fontFamily: {
                "headline-sm": ["Space Grotesk", "sans-serif"],
                "headline-md": ["Space Grotesk", "sans-serif"],
                "headline-lg": ["Space Grotesk", "sans-serif"],
                "body-sm": ["DM Sans", "DM Sans", "sans-serif"],
                "body-md": ["DM Sans", "DM Sans", "sans-serif"],
                "body-lg": ["DM Sans", "DM Sans", "sans-serif"],
                "label-sm": ["DM Sans", "DM Sans", "sans-serif"],
                "label-md": ["DM Sans", "DM Sans", "sans-serif"],
                "code-inline": ["monospace", "monospace"]
            },
            fontSize: {
                "headline-sm": ["1.125rem", { lineHeight: "1.5rem", letterSpacing: "-0.01em" }],
                "headline-md": ["1.25rem", { lineHeight: "2rem", letterSpacing: "-0.015em" }],
                "headline-lg": ["1.5rem", { lineHeight: "2rem", letterSpacing: "-0.02em" }],
                "body-sm": ["0.75rem", { lineHeight: "1.125rem", letterSpacing: "0.01em" }],
                "body-md": ["0.875rem", { lineHeight: "1.375rem", letterSpacing: "0.005em" }],
                "body-lg": ["1rem", { lineHeight: "1.625rem", letterSpacing: "0em" }],
                "label-sm": ["0.75rem", { lineHeight: "1rem", letterSpacing: "0.04em", fontWeight: "600" }],
                "label-md": ["0.875rem", { lineHeight: "1.25rem", letterSpacing: "0.01em", fontWeight: "500" }],
                "code-inline": ["0.8125rem", { lineHeight: "1.25rem", letterSpacing: "0em" }]
            },
            borderRadius: {
                "sm": "0.25rem",
                "DEFAULT": "0.5rem",
                "md": "0.75rem",
                "lg": "1rem",
                "xl": "1.5rem",
                "full": "9999px"
            },
            spacing: {
                "gutter": "1rem",
                "gutter-desktop": "1.5rem",
                "margin": "1rem",
                "margin-desktop": "2rem",
                "space-xs": "0.25rem",
                "space-sm": "0.5rem",
                "space-md": "0.75rem",
                "space-lg": "1.25rem",
                "space-xl": "2rem"
            },
            backdropBlur: { "xl": "12px" }
        }
    }
};"###;

/// Stitch base layer: reset, surface-filled body, scrollbar hide, plus legacy
/// aliases (`.card`, `.pulse-cyan`) re-implemented to the Stitch canvas.
const STITCH_BASE_STYLE: &str = r###"@layer base {
    html, body { margin: 0; padding: 0; }
    body {
        background: #090b11;
        color: #f2f4f8;
        font-family: "DM Sans", sans-serif;
        -webkit-font-smoothing: antialiased;
        -moz-osx-font-smoothing: grayscale;
        overscroll-behavior: none;
    }
    main > :first-child { margin-top: 0 !important; }
    main > :last-child { margin-bottom: 0 !important; }
}
::-webkit-scrollbar { display: none; }
.card {
    background: #11151d;
    border: 1px solid #2a3242;
    border-radius: 1rem;
}
.pulse-cyan { animation: pulse 2s infinite; }
@keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.5; } }"###;

/// A subject summary computed from Nota data for display.
#[derive(Debug, Clone)]
pub struct SubjectSummary {
    pub name: String,
    pub average: f64,
    pub count: usize,
    /// Subject code from `courseCode` (used in URLs). Empty when unavailable.
    pub codigo: String,
}

/// Shared HTML head section with Tailwind via CDN, HTMX, and the full
/// Synthetic Cognition token set (fonts, colors, typography, shapes, spacing).
pub fn head(title: &str) -> String {
    format!(
        r###"<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="theme-color" content="#090b11">
    <title>{}</title>
    <link rel="preconnect" href="https://fonts.googleapis.com">
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
    <script src="https://unpkg.com/htmx.org@2.0.3/dist/htmx.min.js"></script>
    <script src="https://cdn.tailwindcss.com"></script>
    <script src="/shell.js" defer></script>
    <script id="tailwind-config">
    {}
    </script>
    <link href="https://fonts.googleapis.com/css2?family=DM+Sans:wght@400..700&family=Space+Grotesk:wght@400..700&display=swap" rel="stylesheet">
    <link href="https://fonts.googleapis.com/css2?family=Material+Symbols+Outlined:opsz,wght,FILL,GRAD@20..48,100..700,0..1,-50..200&display=swap" rel="stylesheet">
    <style id="stitch-base">
    {}
    </style>
    <link rel="stylesheet" href="/polish.css">
</head>"###,
        html_escape(title),
        STITCH_TAILWIND_CONFIG,
        STITCH_BASE_STYLE
    )
}

/// Envoltorio académico unificado con Nova. Las rutas protegidas conservan
/// sus datos y handlers, pero ya no renderizan el sidebar independiente viejo.
pub fn page(title: &str, content: &str) -> String {
    page_ext(title, "", "", content)
}

/// Documento Nova para cualquier módulo académico o externo. `active_href`
/// puede ser una ruta canónica; el shell resuelve su entrada principal.
pub fn page_ext(title: &str, _breadcrumb: &str, active_href: &str, content: &str) -> String {
    let html = crate::handlers::stitch_v2::stitch_v2_page(active_href, content, true);
    let Some(open) = html.find("<title>") else {
        return html;
    };
    let Some(relative_close) = html[open..].find("</title>") else {
        return html;
    };
    let close = open + relative_close + "</title>".len();
    let mut titled = String::with_capacity(html.len() + title.len() + 16);
    titled.push_str(&html[..open]);
    titled.push_str(&format!("<title>{}</title>", html_escape(title)));
    titled.push_str(&html[close..]);
    titled
}

/// HTML escape a string.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// MCP connection status banner.
///
/// Conectado: chip con punto pulsante (`animate-pulse`).
/// Desconectado: mismo chip sin pulso (no hay actividad que señalar) más un
/// hint corto con la acción sugerida y un botón inline `location.reload()`
/// — recarga de página completa, sin partials HTMX y sin escritura de estado.
pub fn mcp_status_banner(connected: bool) -> String {
    if connected {
        r###"<div class="bg-surface-container border border-outline rounded-xl p-3 mb-space-md flex flex-col gap-2">
            <div class="flex items-center gap-2">
                <span class="w-2 h-2 rounded-full bg-secondary animate-pulse"></span>
                <span class="material-symbols-outlined text-[16px] text-secondary">lan_wifi</span>
                <span class="font-body-sm text-body-sm text-on-surface">● Conectado al MCP de Mikareno</span>
            </div>
        </div>"###
            .to_string()
    } else {
        r###"<div class="bg-surface-container border border-outline rounded-xl p-3 mb-space-md flex flex-col gap-2">
            <div class="flex items-center gap-2">
                <span class="w-2 h-2 rounded-full bg-tertiary"></span>
                <span class="material-symbols-outlined text-[16px] text-outline">wifi_off</span>
                <span class="font-body-sm text-body-sm text-on-surface-variant">● Mikareno no conectado — sin datos de Mikareno: verifica que el MCP esté corriendo</span>
            </div>
            <div class="flex items-center gap-2 pl-6">
                <span class="text-xs text-on-surface-variant">Reinicia el MCP de Mikareno y vuelve a cargar la página.</span>
                <button type="button" onclick="location.reload()" class="inline-flex items-center gap-1 rounded-md border border-outline px-2 py-1 text-on-surface-variant hover:bg-surface-container-high hover:text-on-surface transition-colors">
                    <span class="material-symbols-outlined text-[14px]">refresh</span>
                    <span class="text-xs">Reintentar</span>
                </button>
            </div>
        </div>"###
            .to_string()
    }
}

/// Empty state with an HTMX partial reload button and a configurable
/// Material Symbols icon.
///
/// `icon` is the ligature name of the symbol (`school`, `calendar_month`,
/// `event_busy`…). Only `[a-z0-9_]` survives sanitization, so the value can
/// never inject markup; an empty/invalid icon falls back to `error_outline`.
pub fn empty_state_html_icon(message: &str, icon: &str, hx_get: &str) -> String {
    let icon = sanitize_symbol(icon);
    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-6 text-center" id="empty-state">
        <span class="material-symbols-outlined text-[32px] text-outline mb-2">{}</span>
        <p class="font-body-sm text-body-sm text-on-surface-variant mb-space-md">{}</p>
        <button hx-get="{}" hx-target="#empty-state" hx-swap="outerHTML"
            class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-primary-container text-on-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors">
            <span class="material-symbols-outlined text-[16px]">refresh</span> Reintentar
        </button>
    </div>"###,
        icon,
        html_escape(message),
        hx_get
    )
}

/// Empty/error state with an HTMX partial reload button.
///
/// Default icon: `error_outline`. Use [`empty_state_html_icon`] when the
/// empty state has a semantic symbol (`school`, `calendar_month`,
/// `event_busy`…).
pub fn empty_state_html(message: &str, hx_get: &str) -> String {
    empty_state_html_icon(message, "error_outline", hx_get)
}

/// Keep only the characters valid in a Material Symbols ligature name
/// (`[a-z0-9_]`); falls back to `error_outline` when nothing is left.
fn sanitize_symbol(icon: &str) -> String {
    let cleaned: String = icon
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
        .collect();
    if cleaned.is_empty() {
        "error_outline".to_string()
    } else {
        cleaned
    }
}

/// Reusable status pill (chip) with an honest colour: green when the state
/// is good, amber when it still needs attention.
///
/// Shares the scale of [`grade_color`] (`green-400` / `yellow-400`) so pills
/// and grades read as one system. The text is the source of truth — the
/// colour only reinforces it, never contradicts it.
pub fn status_chip(ok: bool, text: &str) -> String {
    let (fg, bg) = if ok {
        ("text-green-400", "bg-green-400/10")
    } else {
        ("text-yellow-400", "bg-yellow-400/10")
    };
    format!(
        r#"<span class="font-label-sm text-[10px] {} {} px-2 py-0.5 rounded-full">{}</span>"#,
        fg,
        bg,
        html_escape(text),
    )
}

/// Format a float grade for display with one decimal place.
fn fmt_grade(g: f64) -> String {
    format!("{:.1}", g)
}

/// Spanish weekday name derived from an ISO `fecha` (`YYYY-MM-DD`).
/// Falls back to the date itself when it isn't parseable.
pub fn weekday_es(fecha: &str) -> String {
    use chrono::{Datelike, NaiveDate};
    match NaiveDate::parse_from_str(fecha, "%Y-%m-%d") {
        Ok(d) => match d.weekday() {
            chrono::Weekday::Mon => "Lunes",
            chrono::Weekday::Tue => "Martes",
            chrono::Weekday::Wed => "Miércoles",
            chrono::Weekday::Thu => "Jueves",
            chrono::Weekday::Fri => "Viernes",
            chrono::Weekday::Sat => "Sábado",
            chrono::Weekday::Sun => "Domingo",
        }
        .to_string(),
        Err(_) => fecha.to_string(),
    }
}

/// Abreviatura de tres letras de un `chrono::Weekday`.
fn weekday_corto_de(w: chrono::Weekday) -> &'static str {
    match w {
        chrono::Weekday::Mon => "Lun",
        chrono::Weekday::Tue => "Mar",
        chrono::Weekday::Wed => "Mié",
        chrono::Weekday::Thu => "Jue",
        chrono::Weekday::Fri => "Vie",
        chrono::Weekday::Sat => "Sáb",
        chrono::Weekday::Sun => "Dom",
    }
}

/// Fuente de un día de la semana para [`weekday_es_corto`].
pub trait WeekdayCorto {
    /// Abreviatura en español (`Lun`, `Mar`, `Mié`…) o el texto original
    /// cuando la entrada no se reconoce.
    fn corto(&self) -> String;
}

impl WeekdayCorto for &str {
    fn corto(&self) -> String {
        use chrono::{Datelike, NaiveDate};
        let s: &str = self;
        if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return weekday_corto_de(d.weekday()).to_string();
        }
        let lower = s.to_lowercase();
        match lower.trim() {
            "lunes" | "lun" => "Lun".to_string(),
            "martes" | "mar" => "Mar".to_string(),
            "miércoles" | "miercoles" | "mié" | "mie" => "Mié".to_string(),
            "jueves" | "jue" => "Jue".to_string(),
            "viernes" | "vie" => "Vie".to_string(),
            "sábado" | "sabado" | "sáb" | "sab" => "Sáb".to_string(),
            "domingo" | "dom" => "Dom".to_string(),
            _ => s.to_string(),
        }
    }
}

impl WeekdayCorto for String {
    fn corto(&self) -> String {
        self.as_str().corto()
    }
}

impl WeekdayCorto for &String {
    fn corto(&self) -> String {
        self.as_str().corto()
    }
}

impl WeekdayCorto for chrono::Weekday {
    fn corto(&self) -> String {
        weekday_corto_de(*self).to_string()
    }
}

impl WeekdayCorto for &chrono::Weekday {
    fn corto(&self) -> String {
        weekday_corto_de(**self).to_string()
    }
}

impl WeekdayCorto for chrono::NaiveDate {
    fn corto(&self) -> String {
        use chrono::Datelike;
        weekday_corto_de(self.weekday()).to_string()
    }
}

impl WeekdayCorto for &chrono::NaiveDate {
    fn corto(&self) -> String {
        use chrono::Datelike;
        weekday_corto_de((*self).weekday()).to_string()
    }
}

/// Abreviatura del día de la semana en español: `Lun`, `Mar`, `Mié`…
///
/// Acepta una fecha ISO (`2026-10-19`), un `chrono::Weekday` o un nombre de
/// día ya traducido (`"Lunes"`). Si la entrada no se reconoce se devuelve
/// tal cual, igual que [`weekday_es`].
pub fn weekday_es_corto<T: WeekdayCorto>(u: T) -> String {
    u.corto()
}

/// Grade color class based on value (0–10 scale).
fn grade_color(g: f64) -> &'static str {
    if g >= 7.0 {
        "text-green-400"
    } else if g >= 4.0 {
        "text-yellow-400"
    } else {
        "text-red-400"
    }
}

/// Percent-encode a string for safe use in a URL path segment.
/// Unreserved characters (A–Z, a–z, 0–9, `-`, `_`, `.`, `~`) are kept;
/// everything else is encoded as `%XX`.
fn url_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    out
}

/// Render subjects as a grid of cards with average grades.
pub fn subject_grid_html(subjects: &[SubjectSummary]) -> String {
    if subjects.is_empty() {
        return empty_state_html_icon(
            "No hay asignaturas disponibles.",
            "school",
            "/dashboard/subjects",
        );
    }

    let cards: String = subjects
        .iter()
        .map(|s| {
            let avg = fmt_grade(s.average);
            let color = grade_color(s.average);
            let bar_width = ((s.average / 10.0) * 100.0).clamp(0.0, 100.0).round() as i64;
            // Link to the subject detail page using the course code. If the
            // code is empty (server omitted courseCode), fall back to the
            // URL-encoded materia name so the link is never a dead `/subjects/`.
            let link_code = if s.codigo.is_empty() {
                url_encode_path(&s.name)
            } else {
                url_encode_path(&s.codigo)
            };
            format!(
                r###"<a href="/dashboard/subjects/{}" class="block">
                    <div class="bg-surface-container border border-outline rounded-xl p-space-md hover:border-primary-container transition-colors group">
                        <div class="flex items-center justify-between">
                            <h3 class="font-headline-sm text-headline-sm text-on-surface group-hover:text-primary-container">{}</h3>
                            <span class="font-label-sm text-[10px] text-primary-container bg-primary-container/10 px-space-sm py-0.5 rounded-full">{}</span>
                        </div>
                        <div class="mt-space-sm flex items-baseline justify-between">
                            <span class="font-headline-md text-headline-md font-bold {}">{}</span>
                            <span class="font-label-sm text-[11px] text-on-surface-variant">promedio</span>
                        </div>
                        <div class="mt-space-xs h-2 bg-surface-container-highest rounded-full overflow-hidden">
                            <div class="h-full {} transition-all" style="width: {}%"></div>
                        </div>
                    </div>
                </a>"###,
                link_code,
                html_escape(&s.name),
                s.count,
                color,
                avg,
                color,
                bar_width,
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        r###"<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-space-md">{}</div>"###,
        cards
    )
}

/// Render calendar events as a vertical timeline.
///
/// Events whose `fecha` matches `today` are highlighted with a cyan accent.
pub fn event_timeline_html(events: &[CalendarioEvent], today: Option<&str>) -> String {
    if events.is_empty() {
        return empty_state_html_icon(
            "No hay eventos programados.",
            "calendar_month",
            "/dashboard/calendar",
        );
    }

    let items: String = events
        .iter()
        .map(|e| {
            let is_today = today.map_or(false, |t| e.fecha.starts_with(t));
            let (bg, dot, badge) = if is_today {
                (
                    "bg-primary-container/10 border-primary-container",
                    "font-label-sm text-label-sm text-primary-container pulse-cyan",
                    r###"<span class="font-label-sm text-[10px] text-primary-container font-semibold">HOY</span>"###,
                )
            } else {
                (
                    "bg-surface-container border-outline",
                    "font-label-sm text-label-sm text-on-surface-variant",
                    "",
                )
            };
            let day_abbr = weekday_es(&e.fecha);
            format!(
                r###"<div class="{} border rounded-xl p-space-sm mb-space-xs">
                    <div class="flex items-center justify-between">
                        <div class="flex items-center space-x-3">
                            <span class="{}">{}</span>
                            <div>
                                <div class="font-label-md text-label-md text-on-surface font-semibold">{}</div>
                                <div class="font-body-sm text-body-sm text-on-surface-variant">{}</div>
                            </div>
                        </div>
                        {}
                    </div>
                </div>"###,
                bg,
                dot,
                day_abbr,
                html_escape(&e.evento),
                html_escape(&e.fecha),
                badge,
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        r###"<div class="space-y-0" id="event-timeline">{}</div>"###,
        items
    )
}

/// Spanish month name (1-indexed).
fn month_name_es(month: u32) -> &'static str {
    match month {
        1 => "Enero",
        2 => "Febrero",
        3 => "Marzo",
        4 => "Abril",
        5 => "Mayo",
        6 => "Junio",
        7 => "Julio",
        8 => "Agosto",
        9 => "Septiembre",
        10 => "Octubre",
        11 => "Noviembre",
        12 => "Diciembre",
        _ => "—",
    }
}

/// Google-Calendar-style month grid. `focus` is any day in the target month.
/// Columns run Monday→Sunday, each cell numbered; events whose ISO `fecha`
/// matches a cell are listed below the day number. `today` (`YYYY-MM-DD`) is
/// highlighted. Leading/trailing blanks always pad the grid so weeks run
/// Monday→Sunday with a minimum of six rows.
///
/// `dias` (opcional) es el mapa `YYYY-MM-DD` → contenido del día que
/// devuelve `academic::calendar_data`: al pasarlo, cada celda muestra clases
/// del horario, tareas que vencen, eventos y anotaciones del usuario. Sin él
/// la celda sólo muestra los eventos de `events` (comportamiento anterior).
pub fn calendar_grid_html(
    events: &[CalendarioEvent],
    focus: chrono::NaiveDate,
    today: Option<&str>,
    dias: Option<&std::collections::HashMap<String, crate::academic::calendar_data::DiaData>>,
) -> String {
    use chrono::Datelike;

    let year = focus.year();
    let month = focus.month();

    // First weekday of the month and how many days it has.
    let first = chrono::NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(focus);
    let mut days_in_month = 1u32;
    while chrono::NaiveDate::from_ymd_opt(year, month, days_in_month + 1).is_some() {
        days_in_month += 1;
    }
    // Monday-based leading blanks: Mon=0 … Sun=6.
    let start = first.weekday().num_days_from_monday();

    // Prev/next month anchors (first of month); safe unwrap on valid inputs.
    let prev = if month == 1 {
        chrono::NaiveDate::from_ymd_opt(year - 1, 12, 1).unwrap_or(focus)
    } else {
        chrono::NaiveDate::from_ymd_opt(year, month - 1, 1).unwrap_or(focus)
    };
    let next = if month == 12 {
        chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap_or(focus)
    } else {
        chrono::NaiveDate::from_ymd_opt(year, month + 1, 1).unwrap_or(focus)
    };

    let label = format!("{} {}", month_name_es(month), year);
    let prev_href = format!(
        "/dashboard/calendar?partial=grid&month={:04}-{:02}",
        prev.year(),
        prev.month()
    );
    let next_href = format!(
        "/dashboard/calendar?partial=grid&month={:04}-{:02}",
        next.year(),
        next.month()
    );

    // La navegación de meses se renderiza FUERA de la rejilla (en su propio
    // bloque `#calendar-nav`): el swap de la rejilla es `outerHTML`, así que
    // si los botones vivieran dentro, desaparecerían con el primer clic.
    let nav = format!(
        r###"<div id="calendar-nav" class="agenda-nav flex items-center justify-between mb-space-sm">
            <button hx-get="{}" hx-target="#calendar-grid" hx-swap="outerHTML"
                class="calendar-nav-button inline-flex items-center justify-center w-8 h-8 rounded-lg text-outline hover:text-on-surface hover:bg-surface-container transition-colors" title="Mes anterior" aria-label="Mes anterior"><span class="material-symbols-outlined">chevron_left</span><span class="calendar-nav-label">Anterior</span></button>
            <div class="calendar-month-heading"><span class="calendar-month-label font-headline-sm text-headline-sm text-primary-container">{}</span></div>
            <button hx-get="{}" hx-target="#calendar-grid" hx-swap="outerHTML"
                class="calendar-nav-button inline-flex items-center justify-center w-8 h-8 rounded-lg text-outline hover:text-on-surface hover:bg-surface-container transition-colors" title="Próximo mes" aria-label="Próximo mes"><span class="calendar-nav-label">Siguiente</span><span class="material-symbols-outlined">chevron_right</span></button>
        </div>"###,
        prev_href, label, next_href
    );

    let weekdays = ["Lun", "Mar", "Mié", "Jue", "Vie", "Sáb", "Dom"];

    let mut cells: Vec<String> = Vec::new();
    // Leading blanks for days before the 1st.
    for _ in 0..start {
        cells.push(r#"<div class="border border-outline/30 rounded-lg"></div>"#.to_string());
    }
    // Numbered day cells with their events.
    for d in 1..=days_in_month {
        let fecha = format!("{:04}-{:02}-{:02}", year, month, d);
        let is_today = today.map_or(false, |t| t == fecha);

        let mut day_evs: Vec<&CalendarioEvent> =
            events.iter().filter(|e| e.fecha == fecha).collect();
        day_evs.sort_by(|a, b| a.evento.cmp(&b.evento));
        let ev_html: String = day_evs.iter().take(3).map(|e| {
            format!(
                r#"<div class="calendar-chip calendar-chip-event mt-space-xs bg-primary-container/15 border border-primary-container/20 rounded text-[9px] text-on-surface truncate px-1 py-0.5" title="{}">{}</div>"#,
                html_escape(&e.evento),
                html_escape(&e.evento)
            )
        }).collect();

        // Con el mapa de días (clases + tareas + anotaciones) el contenido de
        // la celda se construye ahí; si no, se usan los eventos sueltos.
        let (contenido, atributos_celda) = match dias.and_then(|m| m.get(&fecha)) {
            Some(dia) => {
                let chips = crate::academic::calendar_data::chips_del_dia(dia, 3);
                let extra = crate::academic::calendar_data::total_elementos(dia);
                let oculto = crate::academic::calendar_data::ocultos(dia);
                let mas = if oculto > 0 {
                    format!(
                        r#"<div class="calendar-more text-[9px] text-outline mt-space-xs">+{oculto} más</div>"#
                    )
                } else {
                    String::new()
                };
                let attrs = format!(
                    r#" data-date="{fecha}" data-total="{extra}" data-dia-vacio="{}""#,
                    if dia.vacio() { "1" } else { "0" }
                );
                (format!("{chips}{mas}"), attrs)
            }
            None => {
                let attrs = format!(
                    r#" data-date="{fecha}" data-total="{}" data-dia-vacio="{}""#,
                    day_evs.len(),
                    if day_evs.is_empty() { "1" } else { "0" }
                );
                (ev_html, attrs)
            }
        };

        let (cell_cls, num_cls) = if is_today {
            (
                "border-2 border-primary-container/60 bg-surface-container/40 rounded-lg p-space-xs",
                "block font-label-md text-primary-container",
            )
        } else {
            (
                "border border-outline/20 rounded-lg p-space-xs bg-surface-container/30",
                "block font-label-sm text-[10px] text-on-surface-variant",
            )
        };

        cells.push(format!(
            r#"<div class="calendar-cell agenda-cell relative h-32 {}{}" tabindex="0" role="button" aria-label="Añadir o editar anotaciones del {fecha}" title="Añadir o editar anotaciones · {fecha}"><span class="calendar-day-number {} mb-space-xs">{}</span><div class="calendar-day-items mt-space-xs space-y-0.5 overflow-hidden">{}</div></div>"#,
            cell_cls, atributos_celda, num_cls, d, contenido
        ));
    }
    // Trailing blanks: pad to a multiple of 7, minimum six rows.
    let total = start as u32 + days_in_month;
    let rows = total.div_ceil(7).max(6);
    for _ in 0..(rows * 7 - total) {
        cells.push(r#"<div class="border border-outline/30 rounded-lg"></div>"#.to_string());
    }

    let header: String = weekdays
        .iter()
        .map(|d| format!(
            r#"<div class="calendar-weekday font-label-sm text-[10px] uppercase text-outline text-center py-1.5">{}</div>"#,
            d
        ))
        .collect();

    format!(
        r###"{nav}
        <div class="calendar-month-grid agenda-month-grid grid grid-cols-7 gap-px bg-surface-container-highest rounded-lg overflow-hidden border border-outline">
            {}
            {}
        </div>"###,
        header,
        cells.join("")
    )
}

/// Render a profile card from a Perfil.
pub fn profile_card_html(perfil: &Perfil) -> String {
    let nombre = if perfil.nombre.is_empty() {
        "Sin nombre".to_string()
    } else {
        perfil.nombre.clone()
    };
    let institucion = if perfil.institucion.is_empty() {
        "Institucion no disponible".to_string()
    } else {
        perfil.institucion.clone()
    };
    let curso = if perfil.curso.is_empty() {
        "Curso no disponible".to_string()
    } else {
        perfil.curso.clone()
    };
    let inicial = perfil
        .nombre
        .chars()
        .find(|c| c.is_alphabetic())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "N".to_string());

    format!(
        r###"<section class="nova-profile-identity" id="profile-section"><div class="nova-profile-avatar">{}</div><div class="nova-profile-identity-copy"><p>PERFIL DEL ESTUDIANTE</p><h2>{}</h2><span>{}</span><strong>{}</strong></div><button hx-get="/dashboard?partial=profile" hx-target="#profile-section" hx-swap="outerHTML" class="nova-icon-button" title="Actualizar" aria-label="Actualizar perfil"><span class="material-symbols-outlined">refresh</span></button></section>"###,
        html_escape(&inicial),
        html_escape(&nombre),
        html_escape(&institucion),
        html_escape(&curso),
    )
}

/// Render the stats grid with HTMX refresh button.
pub fn stats_grid_html(
    note_count: i64,
    subject_count: usize,
    tarea_count: usize,
    event_count: usize,
) -> String {
    format!(
        r###"<div class="flex items-center justify-between mb-space-sm">
        <h2 class="font-headline-sm text-headline-sm text-primary-container">Estadísticas</h2>
        <button hx-get="/dashboard?partial=stats" hx-target="#stats-grid" hx-swap="innerHTML"
            class="inline-flex items-center gap-1 px-2 py-1 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors" title="Actualizar">
            <span class="material-symbols-outlined text-[16px]">refresh</span>
        </button>
    </div>
    <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-space-md" id="stats-grid">
        <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
            <div class="font-code-inline text-[11px] text-on-surface-variant uppercase">Libros</div>
            <div class="font-label-sm text-[10px] text-on-surface-variant">Asignaturas</div>
            <div class="font-headline-md text-headline-md text-primary-container font-bold">{}</div>
        </div>
        <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
            <div class="font-code-inline text-[11px] text-on-surface-variant uppercase">Notas</div>
            <div class="font-label-sm text-[10px] text-on-surface-variant">Notas propias (BD)</div>
            <div class="font-headline-md text-headline-md text-primary-container font-bold">{}</div>
        </div>
        <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
            <div class="font-code-inline text-[11px] text-on-surface-variant uppercase">Tareas</div>
            <div class="font-label-sm text-[10px] text-on-surface-variant">Tareas pendientes</div>
            <div class="font-headline-md text-headline-md text-primary-container font-bold">{}</div>
        </div>
        <div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
            <div class="font-code-inline text-[11px] text-on-surface-variant uppercase">Evento</div>
            <div class="font-label-sm text-[10px] text-on-surface-variant">Eventos proximos</div>
            <div class="font-headline-md text-headline-md text-primary-container font-bold">{}</div>
        </div>
    </div>"###,
        subject_count, note_count, tarea_count, event_count
    )
}

/// Format schedule days as HTML.
pub fn horario_html(dias: &[HorarioDia]) -> String {
    if dias.is_empty() {
        return r###"<p class="font-body-sm text-body-sm text-on-surface-variant">No hay horario disponible.</p>"###.to_string();
    }

    let mut html =
        String::from("<div class=\"grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-space-md\">");
    for dia in dias {
        let clases = dia
            .clases
            .iter()
            .map(|c| {
                format!(
                    "<li class=\"font-body-sm text-body-sm text-on-surface-variant py-1\">{}</li>",
                    html_escape(c)
                )
            })
            .collect::<Vec<_>>()
            .join("");
        html.push_str(&format!(
            r###"<div class="bg-surface-container border border-outline rounded-xl p-space-md">
                <h3 class="font-headline-sm text-headline-sm text-primary-container font-semibold mb-space-sm">{}</h3>
                <ul class="space-y-space-xs">{}</ul>
            </div>"###,
            html_escape(&dia.dia),
            clases
        ));
    }
    html.push_str("</div>");
    html
}

/// Format a list of items as a card.
pub fn card_list(title: &str, items: &[String]) -> String {
    if items.is_empty() {
        return format!(
            r###"<div class="bg-surface-container border border-outline rounded-xl p-space-md text-center">
                <h2 class="font-headline-sm text-headline-sm text-on-surface font-semibold mb-space-sm">{}</h2>
                <p class="font-body-sm text-body-sm text-on-surface-variant">No hay elementos.</p>
            </div>"###,
            html_escape(title)
        );
    }

    let items_html = items
        .iter()
        .map(|i| {
            format!(
                "<li class=\"font-body-sm text-body-sm text-on-surface-variant py-2 border-b border-outline/40\">{}</li>",
                html_escape(i)
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-space-md">
            <h2 class="font-headline-sm text-headline-sm text-primary-container font-semibold mb-space-md">{}</h2>
            <ul class="list-none space-y-space-xs">{}</ul>
        </div>"###,
        html_escape(title),
        items_html
    )
}

/// Honest empty state for the dashboard when the MCP is disconnected:
/// zeroed stats and an explanatory message — never fabricated counts.
pub fn mock_dashboard_data() -> String {
    format!(
        r###"{}<div class="card rounded-xl p-space-md mb-space-md">
        <h2 class="font-headline-sm text-headline-sm text-primary-container font-semibold mb-space-sm">Estado del MCP</h2>
        <p class="font-body-sm text-body-sm text-on-surface-variant">Mikareno no está conectado — sin datos de Mikareno.</p>
        <p class="font-body-sm text-body-sm text-on-surface-variant mt-space-xs">Verifica que el MCP esté corriendo para ver datos reales.</p>
    </div>"###,
        mcp_status_banner(false)
    )
}

/// Honest empty subject list for when MCP is disconnected — no fabricated
/// subjects; subject_grid_html renders an empty state instead.
pub fn mock_subjects() -> Vec<SubjectSummary> {
    Vec::new()
}

/// Honest empty perfil for when MCP is disconnected — fields stay empty;
/// profile_card_html renders "Sin nombre"/"Curso no disponible" placeholders.
pub fn mock_perfil() -> Perfil {
    Perfil::default()
}

/// Honest empty event list for when MCP is disconnected — no fabricated
/// events; event_timeline_html renders an empty state instead.
pub fn mock_today_events() -> Vec<CalendarioEvent> {
    Vec::new()
}

/// Build SubjectSummary list from Nota data by grouping per materia.
pub fn subjects_from_notas(notas: &[Nota]) -> Vec<SubjectSummary> {
    use std::collections::HashMap;
    let mut grouped: HashMap<String, (Vec<f64>, String)> = HashMap::new();
    for nota in notas {
        let entry = grouped.entry(nota.materia.clone()).or_default();
        entry.0.push(nota.nota);
        if entry.1.is_empty() && !nota.codigo.is_empty() {
            entry.1 = nota.codigo.clone();
        }
    }
    let mut subjects: Vec<SubjectSummary> = grouped
        .iter()
        .map(|(name, (grades, codigo))| {
            let avg = grades.iter().sum::<f64>() / grades.len() as f64;
            SubjectSummary {
                name: name.clone(),
                average: (avg * 100.0).round() / 100.0,
                count: grades.len(),
                codigo: codigo.clone(),
            }
        })
        .collect();
    subjects.sort_by(|a, b| a.name.cmp(&b.name));
    subjects
}

/// Aggregated data needed to render the Stitch dashboard.
#[derive(Debug)]
pub struct DashboardData {
    pub subject_count: usize,
    pub note_count: i64,
    pub tarea_count: usize,
    pub event_count: usize,
    pub subjects: Vec<SubjectSummary>,
    pub events: Vec<CalendarioEvent>,
    pub tareas: Vec<String>,
    pub prioridades: Vec<String>,
    pub perfil: Perfil,
    pub mcp_connected: bool,
}

/// Collect dashboard data from the DB and the Mikareno MCP. When the MCP is
/// unavailable the fields stay empty and the renderers show honest empty
/// states — never fabricated data.
pub async fn build_dashboard_data(state: &Arc<AppState>) -> DashboardData {
    let client = crate::academic::mikareno_client(state);

    // Tasks + count (empty when the MCP is unavailable)
    let tasks = if let Some(client) = &client {
        client.tareas().await.ok().unwrap_or_default()
    } else {
        Vec::new()
    };
    let tarea_count = tasks.len();
    let tareas: Vec<String> = tasks
        .into_iter()
        .map(|t| format!("{} — vence: {}", t.titulo, t.vencimiento))
        .collect();

    // Events + count
    let events = if let Some(client) = &client {
        client.calendario().await.ok().unwrap_or_default()
    } else {
        Vec::new()
    };
    let event_count = events.len();

    // Profile (empty fields when the MCP is unavailable)
    let perfil = if let Some(client) = &client {
        client.perfil().await.ok().unwrap_or_default()
    } else {
        Perfil::default()
    };

    // Subjects grouped from the current parcial's grades
    let subjects = if let Some(client) = &client {
        let notas = client.notas(1).await.ok().unwrap_or_default();
        subjects_from_notas(&notas)
    } else {
        Vec::new()
    };
    let subject_count = subjects.len();

    // Priorities: nearest due tasks, empty when there are none
    let prioridades = tareas.iter().take(5).cloned().collect();

    // Real note count from the local DB
    let note_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    // Connection status after the calls above (the handshake sets the flag).
    let mcp_connected = client.as_ref().map_or(false, |c| c.is_connected());

    DashboardData {
        subject_count,
        note_count,
        tarea_count,
        event_count,
        subjects,
        events,
        tareas,
        prioridades,
        perfil,
        mcp_connected,
    }
}

/// Render the complete dashboard page as Stitch HTML.
pub fn dashboard_html(data: &DashboardData) -> String {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    let content = format!(
        r#"
        {mcp_banner}
        <div class="grid grid-cols-1 lg:grid-cols-3 gap-space-md mb-space-md">
            <div class="lg:col-span-2">{profile}</div>
            <div>{stats}</div>
        </div>
        <div class="mb-space-md">
            <div class="flex items-center justify-between mb-space-sm">
                <h2 class="font-headline-sm text-headline-sm text-primary-container">Asignaturas rápidas</h2>
                <a href="/dashboard/subjects" class="font-label-sm text-label-sm text-primary-container hover:text-primary-fixed-dim hover:underline">Ver todas →</a>
            </div>
            {subjects_html}
        </div>
        <div class="grid grid-cols-1 lg:grid-cols-2 gap-space-md mb-space-md">
            <div>{events_html}</div>
            <div>{tasks_html}</div>
        </div>
        <div>{priorities_html}</div>
        "#,
        mcp_banner = mcp_status_banner(data.mcp_connected),
        profile = profile_card_html(&data.perfil),
        stats = stats_grid_html(
            data.note_count,
            data.subject_count,
            data.tarea_count,
            data.event_count
        ),
        subjects_html = subject_grid_html(&data.subjects),
        events_html = event_timeline_html(&data.events, Some(&today)),
        tasks_html = card_list("Tareas pendientes", &data.tareas),
        priorities_html = card_list("Prioridades del día", &data.prioridades),
    );

    page_ext(
        "Dashboard — Study Platform",
        "Panel principal",
        "/dashboard",
        &content,
    )
}

// ── Subject detail display types (consumed by `subject_detail.rs`) ──────

/// Parsed output of `get_detalle_materia` for a single subject entry.
#[derive(Debug, Clone, Default)]
pub struct MateriaDetalle {
    pub materia: String,
    pub codigo: String,
    pub nota_promedio: Option<f64>,
    pub maximo: f64,
    pub bloqueada: bool,
    /// `true` when the subject is qualitative (observación de texto) rather
    /// than numeric.
    pub es_cualitativa: bool,
    pub observacion: String,
}

/// A single activity / evaluation entry parsed from `get_actividades_materia`.
#[derive(Debug, Clone, Default)]
pub struct ActividadDetalle {
    pub nombre: String,
    pub descripcion: String,
    pub fecha: String,
    pub nota: Option<f64>,
    pub calificada: bool,
    pub submenu: String,
}

/// A submenu group (e.g. "TAREAS", "EXAMEN TRIMESTRAL") with its activities.
#[derive(Debug, Clone, Default)]
pub struct SubmenuDetalle {
    pub nombre: String,
    pub total_actividades: usize,
    pub actividades_calificadas: usize,
    pub subtotal: Option<f64>,
    pub actividades: Vec<ActividadDetalle>,
}

/// Render the subject detail page: summary card + activities-by-submenu table.
///
/// * `connected` — MCP status banner (degrades to disconnected empty state).
/// * `detalle` — parsed from `get_detalle_materia` (may be empty on failure).
/// * `submenus` — parsed from `get_actividades_materia` (may be empty on
///   failure).
/// * `promedio_fallback` / `maximo_fallback` — values resolved from
///   `student_full` promedios so the header renders even when the detail tool
///   fails (degrado honesto).
pub fn subject_detail_html(
    connected: bool,
    materia: &str,
    codigo: &str,
    promedio_fallback: f64,
    maximo_fallback: f64,
    trimestre: u8,
    bloqueada_fallback: bool,
    detalle: &[MateriaDetalle],
    submenus: &[SubmenuDetalle],
) -> String {
    let status = mcp_status_banner(connected);

    // Resolve display values: prefer detalle, fall back to promedios values.
    let (avg, maximo, bloqueada, es_cualitativa, observacion) = detalle.first().map_or_else(
        || {
            (
                promedio_fallback,
                maximo_fallback,
                bloqueada_fallback,
                false,
                String::new(),
            )
        },
        |d| {
            (
                d.nota_promedio.unwrap_or(promedio_fallback),
                d.maximo,
                d.bloqueada,
                d.es_cualitativa,
                d.observacion.clone(),
            )
        },
    );

    let avg_fmt = fmt_grade(avg);
    let color = grade_color(avg);
    let bar_width = ((avg / 10.0) * 100.0).clamp(0.0, 100.0).round() as i64;
    let maximo_fmt = fmt_grade(maximo);
    let bloqueada_txt = if bloqueada { "Sí" } else { "No" };

    let header = format!(
        r###"<div class="flex items-center justify-between mb-space-sm">
        <div class="flex items-center gap-space-sm">
            <a href="/dashboard/subjects" class="font-label-sm text-label-sm text-on-surface-variant hover:text-on-surface">
                <span class="material-symbols-outlined text-[16px]">arrow_back</span> Asignaturas
            </a>
        </div>
    </div>
    <h1 class="font-headline-lg text-headline-lg text-on-surface">{}</h1>
    <p class="font-body-sm text-body-sm text-on-surface-variant">Código: {} · Trimestre: {}</p>"###,
        html_escape(materia),
        html_escape(codigo),
        trimestre,
    );

    let summary_card = format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-space-md mb-space-md">
        <div class="flex items-center justify-between">
            <h2 class="font-headline-md text-headline-md text-primary-container">{}</h2>
            <span class="font-label-sm text-[10px] text-primary-container bg-primary-container/10 px-space-sm py-0.5 rounded-full">{}</span>
        </div>
        <div class="mt-space-sm flex items-baseline justify-between">
            <span class="font-headline-xl text-headline-lg font-bold {}">{}</span>
            <span class="font-label-sm text-[11px] text-on-surface-variant">promedio / {}</span>
        </div>
        <div class="mt-space-xs h-2 bg-surface-container-highest rounded-full overflow-hidden">
            <div class="h-full {} transition-all" style="width: {}%"></div>
        </div>
        <div class="mt-space-sm grid grid-cols-2 md:grid-cols-4 gap-space-sm font-body-sm">
            <div><span class="text-on-surface-variant">Código:</span> {}</div>
            <div><span class="text-on-surface-variant">Máximo:</span> {}</div>
            <div><span class="text-on-surface-variant">Bloqueada:</span> {}</div>
            <div><span class="text-on-surface-variant">Tipo:</span> {}</div>
        </div>
    </div>"###,
        html_escape(materia),
        html_escape(&avg_fmt),
        color,
        avg_fmt,
        maximo_fmt,
        color,
        bar_width,
        html_escape(codigo),
        maximo_fmt,
        bloqueada_txt,
        if es_cualitativa {
            "Cualitativa"
        } else {
            "Cuantitativa"
        },
    );

    // Activities grouped by submenu.
    let activities_html = if submenus.is_empty() {
        empty_state_html(
            "No hay actividades registradas para esta asignatura.",
            "/dashboard/subjects",
        )
    } else {
        let mut parts: Vec<String> = Vec::new();
        for sm in submenus {
            let rows: String = sm.actividades.iter().map(|a| {
                let nota_str = match a.nota {
                    Some(n) => fmt_grade(n),
                    None => "—".to_string(),
                };
                let nota_cls = match a.nota {
                    Some(n) if n >= 7.0 => "text-green-400",
                    Some(n) if n >= 4.0 => "text-yellow-400",
                    Some(_) => "text-red-400",
                    None => "text-on-surface-variant",
                };
                let estado = status_chip(
                    a.calificada,
                    if a.calificada { "Calificada" } else { "Pendiente" },
                );
                format!(
                    r###"<tr class="border-b border-outline/40">
                        <td class="py-space-xs font-body-sm text-body-sm text-on-surface">{}</td>
                        <td class="py-space-xs font-body-sm text-body-sm text-on-surface-variant">{}</td>
                        <td class="py-space-xs font-body-sm text-body-sm text-on-surface-variant">{}</td>
                        <td class="py-space-xs font-headline-sm text-headline-sm font-bold {}">{}</td>
                        <td class="py-space-xs">{}</td>
                    </tr>"###,
                    html_escape(&a.nombre),
                    html_escape(&a.fecha),
                    html_escape(&a.descripcion),
                    nota_cls,
                    nota_str,
                    estado,
                )
            }).collect::<Vec<_>>().join("");

            let subtotal_str = sm.subtotal.map_or("—".to_string(), |s| fmt_grade(s));

            let section = format!(
                r###"<div class="bg-surface-container border border-outline rounded-xl p-space-md mb-space-md">
                    <div class="flex items-center justify-between mb-space-sm">
                        <h3 class="font-headline-sm text-headline-sm text-primary-container">{}</h3>
                        <span class="font-label-sm text-[10px] text-on-surface-variant">{} actividades · {} calificadas · subtotal: {}</span>
                    </div>
                    <div class="overflow-x-auto">
                        <table class="w-full font-label-sm">
                            <thead>
                                <tr class="border-b border-outline/40">
                                    <th class="text-left py-2 font-label-sm text-label-sm text-on-surface-variant">Actividad</th>
                                    <th class="text-left py-2 font-label-sm text-label-sm text-on-surface-variant">Fecha</th>
                                    <th class="text-left py-2 font-label-sm text-label-sm text-on-surface-variant">Descripción</th>
                                    <th class="text-left py-2 font-label-sm text-label-sm text-on-surface-variant">Nota</th>
                                    <th class="text-left py-2 font-label-sm text-label-sm text-on-surface-variant">Estado</th>
                                </tr>
                            </thead>
                            <tbody>{}</tbody>
                        </table>
                    </div>
                </div>"###,
                html_escape(&sm.nombre),
                sm.total_actividades,
                sm.actividades_calificadas,
                subtotal_str,
                rows,
            );
            parts.push(section);
        }
        parts.join("")
    };

    let content = format!(
        r###"{}{}{}{}{}"###,
        status,
        header,
        summary_card,
        observacion_html(&observacion),
        activities_html
    );

    page_ext(
        &format!("{} — Study Platform", materia),
        "Asignaturas",
        "/dashboard/subjects",
        &content,
    )
}

/// Render the qualitative observation (if any) as a small card.
fn observacion_html(obs: &str) -> String {
    if obs.is_empty() {
        return String::new();
    }
    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-space-md mb-space-md">
            <h3 class="font-headline-sm text-headline-sm text-primary-container mb-space-sm">Observación</h3>
            <p class="font-body-sm text-body-sm text-on-surface-variant">{}</p>
        </div>"###,
        html_escape(obs),
    )
}

/// URL-encode for use in `href` query parameters (kept here for completeness;
/// the path-segment encoder `url_encode_path` above is preferred for path use).
#[allow(dead_code)]
fn url_encode_query(s: &str) -> String {
    url_encode_path(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
    }

    #[test]
    fn test_head_includes_meta() {
        let h = head("Test");
        assert!(h.contains("<meta name=\"viewport\""));
        assert!(h.contains("htmx.org"));
        assert!(h.contains("tailwindcss.com"));
    }

    #[test]
    fn test_page_structure() {
        let p = page("Test", "content");
        assert!(p.contains("<!DOCTYPE html>"));
        assert!(p.contains("<nav"));
        assert!(p.contains("content"));
    }

    #[test]
    fn test_page_nav_includes_new_routes() {
        let p = page("Test", "");
        assert!(p.contains("/dashboard/subjects"));
        assert!(p.contains("/dashboard/profile"));
    }

    #[test]
    fn test_horario_empty() {
        assert!(horario_html(&[]).contains("No hay horario"));
    }

    #[test]
    fn test_card_list_empty() {
        assert!(card_list("Test", &[]).contains("No hay elementos"));
    }

    #[test]
    fn test_card_list_with_items() {
        assert!(card_list("Tareas", &vec!["Tarea 1".to_string()]).contains("Tarea 1"));
    }

    #[test]
    fn test_subject_grid_empty() {
        let html = subject_grid_html(&[]);
        assert!(html.contains("No hay asignaturas"));
        assert!(html.contains("Reintentar"));
    }

    #[test]
    fn test_subject_grid_with_data() {
        let subjects = vec![
            SubjectSummary {
                name: "Matematicas".to_string(),
                average: 8.5,
                count: 2,
                codigo: "M1".to_string(),
            },
            SubjectSummary {
                name: "Lenguaje".to_string(),
                average: 3.0,
                count: 1,
                codigo: "L1".to_string(),
            },
        ];
        let html = subject_grid_html(&subjects);
        assert!(html.contains("Matematicas"));
        assert!(html.contains("Lenguaje"));
        assert!(html.contains("8.5"));
        assert!(html.contains("3.0"));
    }

    #[test]
    fn test_subject_grid_grade_colors() {
        let subjects = vec![
            SubjectSummary {
                name: "Alta".to_string(),
                average: 9.0,
                count: 1,
                codigo: "A1".to_string(),
            },
            SubjectSummary {
                name: "Media".to_string(),
                average: 5.0,
                count: 1,
                codigo: "M1".to_string(),
            },
            SubjectSummary {
                name: "Baja".to_string(),
                average: 2.0,
                count: 1,
                codigo: "B1".to_string(),
            },
        ];
        let html = subject_grid_html(&subjects);
        assert!(html.contains("text-green-400"));
        assert!(html.contains("text-yellow-400"));
        assert!(html.contains("text-red-400"));
    }

    #[test]
    fn test_event_timeline_empty() {
        let html = event_timeline_html(&[], None);
        assert!(html.contains("No hay eventos"));
        assert!(html.contains("Reintentar"));
    }

    #[test]
    fn test_event_timeline_with_data() {
        let events = vec![CalendarioEvent {
            fecha: "2024-03-15".to_string(),
            evento: "Examen".to_string(),
            tipo: "SCHOOL_DAY".to_string(),
        }];
        let html = event_timeline_html(&events, Some("2024-03-15"));
        assert!(html.contains("Examen"));
        assert!(html.contains("HOY"));
    }

    #[test]
    fn test_event_timeline_highlight_today() {
        let today = "2024-03-15";
        let events = vec![
            CalendarioEvent {
                fecha: today.to_string(),
                evento: "Hoy".to_string(),
                tipo: "EVENT".to_string(),
            },
            CalendarioEvent {
                fecha: "2024-03-16".to_string(),
                evento: "Otro dia".to_string(),
                tipo: "EVENT".to_string(),
            },
        ];
        let html = event_timeline_html(&events, Some(today));
        assert!(html.contains("HOY"));
        assert!(html.matches("pulse-cyan").count() == 1);
    }

    #[test]
    fn test_event_timeline_no_highlight_without_today() {
        let events = vec![CalendarioEvent {
            fecha: "2024-03-15".to_string(),
            evento: "Evento".to_string(),
            tipo: "EVENT".to_string(),
        }];
        let html = event_timeline_html(&events, None);
        assert!(!html.contains("HOY"));
    }

    #[test]
    fn test_profile_card() {
        let p = Perfil {
            nombre: "Ana".to_string(),
            institucion: "Colegio X".to_string(),
            curso: "1 B".to_string(),
        };
        let html = profile_card_html(&p);
        assert!(html.contains("Ana"));
        assert!(html.contains("Colegio X"));
        assert!(html.contains("1 B"));
        assert!(html.contains("partial=profile"));
    }

    #[test]
    fn test_profile_card_empty() {
        let p = Perfil::default();
        let html = profile_card_html(&p);
        assert!(html.contains("Sin nombre"));
        assert!(html.contains("Institucion no disponible"));
    }

    #[test]
    fn test_stats_grid_html() {
        let html = stats_grid_html(7, 4, 2, 3);
        assert!(html.contains("Asignaturas"));
        assert!(html.contains("Notas propias (BD)"));
        assert!(html.contains("Tareas pendientes"));
        assert!(html.contains("Eventos proximos"));
        assert!(html.contains("7"));
        assert!(html.contains("4"));
        assert!(html.contains("2"));
        assert!(html.contains("3"));
        assert!(html.contains("partial=stats"));
    }

    #[test]
    fn test_empty_state_html() {
        let html = empty_state_html("No data", "/test/path");
        assert!(html.contains("No data"));
        assert!(html.contains("Reintentar"));
        assert!(html.contains("/test/path"));
    }

    #[test]
    fn test_empty_state_html_keeps_default_icon() {
        let html = empty_state_html("No data", "/test/path");
        assert!(html.contains(">error_outline</span>"));
    }

    #[test]
    fn test_empty_state_html_icon_uses_given_symbol() {
        let html = empty_state_html_icon("Sin exámenes", "school", "/dashboard/examenes");
        assert!(html.contains(">school</span>"));
        assert!(html.contains("Sin exámenes"));
        assert!(html.contains("/dashboard/examenes"));
        assert!(html.contains("Reintentar"));
        // El icono configurable sustituye al genérico.
        assert!(!html.contains(">error_outline</span>"));
    }

    #[test]
    fn test_empty_state_html_icon_sanitizes_symbol() {
        let html = empty_state_html_icon("x", "sch<ool>", "/p");
        assert!(html.contains(">school</span>"));
        assert!(!html.contains("<ool>"));
    }

    #[test]
    fn test_status_chip_colors_are_honest() {
        let ok = status_chip(true, "Calificada");
        assert!(ok.contains("Calificada"));
        assert!(ok.contains("text-green-400"));
        assert!(!ok.contains("text-yellow-400"));

        let pending = status_chip(false, "Pendiente");
        assert!(pending.contains("Pendiente"));
        assert!(pending.contains("text-yellow-400"));
        assert!(!pending.contains("text-green-400"));
    }

    #[test]
    fn test_mcp_status_banner_connected() {
        let html = mcp_status_banner(true);
        assert!(html.contains("Conectado al MCP"));
    }

    #[test]
    fn test_mcp_status_banner_disconnected() {
        let html = mcp_status_banner(false);
        assert!(html.contains("no conectado"));
    }

    #[test]
    fn test_mcp_status_banner_disconnected_has_retry() {
        let html = mcp_status_banner(false);
        assert!(html.contains("Reintentar"));
        assert!(html.contains("location.reload()"));
        // Sin partials HTMX: la recarga es de página completa.
        assert!(!html.contains("hx-get"));
    }

    #[test]
    fn test_mcp_status_banner_connected_pulses() {
        let html = mcp_status_banner(true);
        assert!(html.contains("animate-pulse"));
    }

    #[test]
    fn test_weekday_es_corto() {
        assert_eq!(weekday_es_corto("2026-10-19"), "Lun");
        assert_eq!(weekday_es_corto("2026-10-21"), "Mié");
        assert_eq!(weekday_es_corto(chrono::Weekday::Sat), "Sáb");
        assert_eq!(weekday_es_corto("lunes"), "Lun");
        assert_eq!(weekday_es_corto("no-es-fecha"), "no-es-fecha");
    }

    #[test]
    fn test_mock_subjects() {
        // Honest mocks: empty until real data arrives.
        assert!(mock_subjects().is_empty());
    }

    #[test]
    fn test_mock_perfil() {
        let p = mock_perfil();
        assert!(p.nombre.is_empty());
        assert!(p.institucion.is_empty());
        assert!(p.curso.is_empty());
    }

    #[test]
    fn test_mock_today_events() {
        assert!(mock_today_events().is_empty());
    }

    #[test]
    fn test_subjects_from_notas_groups_and_averages() {
        let notas = vec![
            Nota {
                materia: "Math".to_string(),
                nota: 8.0,
                trimestre: 1,
                codigo: "M1".to_string(),
            },
            Nota {
                materia: "Math".to_string(),
                nota: 10.0,
                trimestre: 2,
                codigo: "M1".to_string(),
            },
            Nota {
                materia: "Science".to_string(),
                nota: 7.0,
                trimestre: 1,
                codigo: "S1".to_string(),
            },
        ];
        let subjects = subjects_from_notas(&notas);
        assert_eq!(subjects.len(), 2);

        let math = subjects.iter().find(|s| s.name == "Math").unwrap();
        assert!((math.average - 9.0).abs() < 1e-9);
        assert_eq!(math.count, 2);
        assert_eq!(math.codigo, "M1");

        let science = subjects.iter().find(|s| s.name == "Science").unwrap();
        assert!((science.average - 7.0).abs() < 1e-9);
        assert_eq!(science.codigo, "S1");
    }

    #[test]
    fn test_subjects_from_notas_empty() {
        let subjects = subjects_from_notas(&[]);
        assert!(subjects.is_empty());
    }

    #[test]
    fn test_mock_dashboard_data() {
        let html = mock_dashboard_data();
        assert!(html.contains("card"));
        assert!(html.contains("no está conectado"));
    }
}
