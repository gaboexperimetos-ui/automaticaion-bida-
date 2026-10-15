//! HTML template helpers for academic dashboard.
//! Shared styling: Synthetic Cognition (Stitch) design system over HTMX + Tailwind.

use crate::integrations::mikareno::{CalendarioEvent, HorarioDia, Nota, Perfil};
use std::sync::Arc;

use crate::integrations::mikareno::Tarea;
use crate::AppState;

/// Stitch `tailwind.config` extending Tailwind with the full Synthetic Cognition
/// color token set, typography scale (Manrope headlines / Hanken Grotesk body),
/// shape tokens, spacing scale and blur preset.
const STITCH_TAILWIND_CONFIG: &str = r###"tailwind.config = {
    darkMode: "class",
    theme: {
        extend: {
            colors: {
                "background": "#101419",
                "on-background": "#e0e2ea",
                "surface": "#101419",
                "surface-dim": "#101419",
                "surface-bright": "#36393f",
                "surface-container-lowest": "#0a0e13",
                "surface-container-low": "#181c21",
                "surface-container": "#1c2025",
                "surface-container-high": "#262a30",
                "surface-container-highest": "#31353b",
                "surface-variant": "#31353b",
                "on-surface": "#e0e2ea",
                "on-surface-variant": "#bbcac5",
                "inverse-surface": "#e0e2ea",
                "inverse-on-surface": "#2d3136",
                "outline": "#859490",
                "outline-variant": "#3c4946",
                "surface-tint": "#4ddcc6",
                "primary": "#b5fff0",
                "on-primary": "#003730",
                "primary-container": "#5eead4",
                "on-primary-container": "#00675b",
                "inverse-primary": "#006b5e",
                "primary-fixed": "#6ef9e2",
                "primary-fixed-dim": "#4ddcc6",
                "on-primary-fixed": "#00201b",
                "on-primary-fixed-variant": "#005047",
                "secondary": "#9ed753",
                "on-secondary": "#203700",
                "secondary-container": "#6b9f1f",
                "on-secondary-container": "#1b2f00",
                "secondary-fixed": "#b9f46c",
                "secondary-fixed-dim": "#9ed753",
                "on-secondary-fixed": "#102000",
                "on-secondary-fixed-variant": "#304f00",
                "tertiary": "#f5eeff",
                "on-tertiary": "#381385",
                "tertiary-container": "#dbcdff",
                "on-tertiary-container": "#6448b2",
                "tertiary-fixed": "#e8ddff",
                "tertiary-fixed-dim": "#cebdff",
                "on-tertiary-fixed": "#21005e",
                "on-tertiary-fixed-variant": "#4f319c",
                "error": "#ffb4ab",
                "on-error": "#690005",
                "error-container": "#93000a",
                "on-error-container": "#ffdad6"
            },
            fontFamily: {
                "headline-sm": ["Manrope", "sans-serif"],
                "headline-md": ["Manrope", "sans-serif"],
                "headline-lg": ["Manrope", "sans-serif"],
                "body-sm": ["Hanken Grotesk", "sans-serif"],
                "body-md": ["Hanken Grotesk", "sans-serif"],
                "body-lg": ["Hanken Grotesk", "sans-serif"],
                "label-sm": ["Hanken Grotesk", "sans-serif"],
                "label-md": ["Hanken Grotesk", "sans-serif"],
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
        background: #101419;
        color: #e0e2ea;
        font-family: "Hanken Grotesk", sans-serif;
        -webkit-font-smoothing: antialiased;
        -moz-osx-font-smoothing: grayscale;
        overscroll-behavior: none;
    }
    main > :first-child { margin-top: 0 !important; }
    main > :last-child { margin-bottom: 0 !important; }
}
::-webkit-scrollbar { display: none; }
.card {
    background: #1c2025;
    border: 1px solid #3c4946;
    border-radius: 1rem;
}
.pulse-cyan { animation: pulse 2s infinite; }
@keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.5; } }"###;

/// Stitch shell: fixed sidebar nav-rail (250px) + fixed top bar (h-16).
/// Preserved across all pages; the content slot renders inside `<main>`.
const STITCH_SHELL: &str = r###"
<!-- Synthetic Cognition shell: sidebar nav-rail -->
<nav class="fixed top-0 left-0 h-screen w-[250px] bg-surface-container-low border-r border-outline-variant/40 z-50 overflow-y-auto flex flex-col">
    <div class="flex items-center justify-between h-14 px-space-md border-b border-outline-variant/40">
        <div class="flex items-center gap-2">
            <span class="material-symbols-outlined text-primary-container text-[20px]">psychology</span>
            <div>
                <span class="font-headline-sm text-headline-sm text-on-surface font-semibold">STUDY PLATFORM</span>
                <div class="flex items-center gap-1.5 mt-0.5">
                    <span class="font-code-inline text-[10px] text-primary-fixed-dim">v2.4</span>
                    <span class="w-1.5 h-1.5 rounded-full bg-secondary animate-pulse"></span>
                </div>
            </div>
        </div>
        <button class="p-1 rounded text-outline hover:text-on-surface hover:bg-surface-container transition-colors" title="Colapsar barra">
            <span class="material-symbols-outlined text-[18px]">chevron_left</span>
        </button>
    </div>
    <div class="flex-1 overflow-y-auto py-space-sm">
        <div class="px-space-sm mb-space-sm">
            <span class="font-label-sm text-[10px] uppercase text-outline tracking-wider block pb-space-xs">General</span>
            <a href="/dashboard" class="flex items-center gap-2 p-space-sm rounded-lg text-on-primary-container bg-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors">
                <span class="material-symbols-outlined text-[18px]">space_dashboard</span> Panel principal
            </a>
            <a href="/dashboard/calendar" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">calendar_today</span> Mi agenda
            </a>
            <a href="/dashboard/planner" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">schedule</span> Planificación
            </a>
        </div>
        <div class="px-space-sm mb-space-sm">
            <span class="font-label-sm text-[10px] uppercase text-outline tracking-wider block pb-space-xs">Estudios</span>
            <a href="/dashboard/subjects" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">menu_book</span> Asignaturas
            </a>
            <a href="/dashboard/tareas" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">check_box</span> Tareas
            </a>
            <a href="/dashboard/examenes" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">school</span> Exámenes
            </a>
            <a href="/dashboard/horario" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">calendar_view_week</span> Horarios
            </a>
        </div>
        <div class="px-space-sm mb-space-sm">
            <span class="font-label-sm text-[10px] uppercase text-outline tracking-wider block pb-space-xs">Inteligencia Artificial</span>
            <a href="/dashboard/ai/assistant" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">auto_awesome</span> Asistente
            </a>
            <a href="/dashboard/ai/chats" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">forum</span> Conversaciones
            </a>
            <a href="/dashboard/ai/generator" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">draw</span> Generador de material
            </a>
        </div>
        <div class="px-space-sm mb-space-sm">
            <span class="font-label-sm text-[10px] uppercase text-outline tracking-wider block pb-space-xs">Conocimiento</span>
            <a href="/dashboard/brain" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">neurology</span> Segundo cerebro
            </a>
            <a href="/dashboard/notes" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">description</span> Mis notas
            </a>
            <a href="/dashboard/search" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">search</span> Búsqueda global
            </a>
        </div>
        <div class="px-space-sm mb-space-sm">
            <span class="font-label-sm text-[10px] uppercase text-outline tracking-wider block pb-space-xs">Oficina</span>
            <a href="/office/documents" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">edit_document</span> Documentos
            </a>
            <a href="/office/sheets" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">table_chart</span> Hojas
            </a>
            <a href="/office/slides" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">slideshow</span> Presentaciones
            </a>
            <a href="/office/library" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">folder_copy</span> Biblioteca
            </a>
        </div>
        <div class="px-space-sm mb-space-sm">
            <span class="font-label-sm text-[10px] uppercase text-outline tracking-wider block pb-space-xs">Integraciones</span>
            <a href="/dashboard/integrations/mcp" class="flex items-center justify-between p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <div class="flex items-center gap-2">
                    <span class="material-symbols-outlined text-[18px]">memory</span> Centro MCP
                </div>
                <span class="px-1.5 py-0.25 rounded-full bg-secondary-container text-[10px] font-code-inline text-on-secondary-container">Conectado</span>
            </a>
            <a href="/dashboard/integrations/status" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">hub</span> Estado de conexiones
            </a>
        </div>
        <div class="px-space-sm mb-space-sm">
            <span class="font-label-sm text-[10px] uppercase text-outline tracking-wider block pb-space-xs">Sistema</span>
            <a href="/dashboard/system/admin" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">query_stats</span> Administración
            </a>
            <a href="/dashboard/system/settings" class="flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors">
                <span class="material-symbols-outlined text-[18px]">settings</span> Configuración
            </a>
        </div>
    </div>
    <div class="h-12 px-space-md border-t border-outline-variant/40 flex items-center gap-2">
        <span class="material-symbols-outlined text-[14px] text-primary-fixed-dim">shield_lock</span>
        <span class="font-code-inline text-[10px] text-outline">Localhost only</span>
    </div>
</nav>
<!-- Synthetic Cognition shell: top bar -->
<header class="fixed top-0 left-[250px] right-0 h-16 bg-surface-container-low/90 backdrop-blur-xl border-b border-outline-variant/40 z-40 flex items-center justify-between px-space-md">
    <nav aria-label="breadcrumb" class="flex items-center gap-1.5">
        <span class="material-symbols-outlined text-[14px] text-outline">home</span>
        <span class="material-symbols-outlined text-[14px] text-outline">chevron_right</span>
        <span class="font-body-sm text-body-sm text-on-surface">Panel principal</span>
    </nav>
    <div class="flex items-center gap-2">
        <div class="relative">
            <input type="text" placeholder="Buscar… (Cmd+K)" class="pl-8 pr-24 py-2 w-64 rounded-lg bg-surface-container text-on-surface font-body-md outline-none focus:ring-1 focus:ring-primary-container placeholder:text-outline/60"/>
            <span class="material-symbols-outlined text-[16px] text-outline absolute left-2 top-1/2 -translate-y-1/2">search</span>
            <kbd class="font-code-inline text-[9px] text-outline absolute right-2 top-1/2 -translate-y-1/2">⌘K</kbd>
        </div>
        <button class="p-1.5 rounded-lg text-outline hover:text-on-surface hover:bg-surface-container transition-colors" title="Estado de conexiones">
            <span class="material-symbols-outlined text-[18px]">wifi</span>
        </button>
        <button class="p-1.5 rounded-lg text-outline hover:text-on-surface hover:bg-surface-container transition-colors relative" title="Notificaciones">
            <span class="material-symbols-outlined text-[18px]">notifications</span>
            <span class="absolute top-1 right-1 w-1.5 h-1.5 rounded-full bg-secondary"></span>
        </button>
        <a href="/dashboard/profile" class="p-1.5 rounded-lg text-outline hover:text-on-surface hover:bg-surface-container transition-colors" title="Perfil">
            <span class="material-symbols-outlined text-[18px]">account_circle</span>
        </a>
    </div>
</header>
"###;

/// A subject summary computed from Nota data for display.
#[derive(Debug, Clone)]
pub struct SubjectSummary {
    pub name: String,
    pub average: f64,
    pub count: usize,
}

/// Shared HTML head section with Tailwind via CDN, HTMX, and the full
/// Synthetic Cognition token set (fonts, colors, typography, shapes, spacing).
pub fn head(title: &str) -> String {
    format!(
        r###"<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{}</title>
    <script src="https://unpkg.com/htmx.org@2.0.3/dist/htmx.min.js"></script>
    <script src="https://cdn.tailwindcss.com"></script>
    <script id="tailwind-config">
    {}
    </script>
    <link href="https://fonts.googleapis.com/css2?family=Manrope:wght@400..900&family=Hanken+Grotesk:wght@400..900&display=swap" rel="stylesheet">
    <link href="https://fonts.googleapis.com/css2?family=Material+Symbols+Outlined:opsz,wght,FILL,GRAD@20..48,100..700,0..1,-50..200&display=swap" rel="stylesheet">
    <style id="stitch-base">
    {}
    </style>
</head>"###,
        html_escape(title),
        STITCH_TAILWIND_CONFIG,
        STITCH_BASE_STYLE
    )
}

/// Full HTML page wrapper with the Stitch shell (sidebar + top bar).
pub fn page(title: &str, content: &str) -> String {
    format!(
        r###"<!DOCTYPE html>
<html class="dark" lang="es">
{}
<body class="bg-surface text-on-surface font-body-md antialiased min-h-screen">
{}
<main class="pt-16 pl-[250px] p-space-md space-y-space-md">
{}
</main>
</body>
</html>"###,
        head(title),
        STITCH_SHELL,
        content
    )
}

/// Full Stitch document for routes outside the `/dashboard` namespace.
///
/// Reuses the shared `<head>`, the nav-rail + top-bar shell, and the same
/// body/main layout so every screen shares identical tokens. `breadcrumb`
/// (top-bar link label) and `active_href` (highlighted nav-rail entry) are
/// injected at render time so the shell stays consistent across pages.
pub fn page_ext(title: &str, breadcrumb: &str, active_href: &str, content: &str) -> String {
    let shell = STITCH_SHELL
        .replace(
            "<span class=\"font-body-sm text-body-sm text-on-surface\">Panel principal</span>",
            &format!(
                "<span class=\"font-body-sm text-body-sm text-on-surface\">{}</span>",
                html_escape(breadcrumb)
            ),
        )
        .replace(
            &format!(
                "href=\"{}\" class=\"flex items-center gap-2 p-space-sm rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors\">",
                active_href
            ),
            &format!(
                "href=\"{}\" class=\"flex items-center gap-2 p-space-sm rounded-lg text-on-primary-container bg-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors\">",
                active_href
            ),
        );

    format!(
        r###"<!DOCTYPE html>
<html class="dark" lang="es">
{}
<body class="bg-surface text-on-surface font-body-md antialiased min-h-screen">
{}
<main class="pt-16 pl-[250px] p-space-md space-y-space-md">
{}
</main>
</body>
</html>"###,
        head(title),
        shell,
        content
    )
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
pub fn mcp_status_banner(connected: bool) -> String {
    if connected {
        r###"<div class="bg-surface-container border border-outline rounded-xl p-3 mb-space-md flex items-center gap-2">
            <span class="w-2 h-2 rounded-full bg-secondary animate-pulse"></span>
            <span class="material-symbols-outlined text-[16px] text-secondary">lan_wifi</span>
            <span class="font-body-sm text-body-sm text-on-surface">● Conectado al MCP de Mikareno</span>
        </div>"###
            .to_string()
    } else {
        r###"<div class="bg-surface-container border border-outline rounded-xl p-3 mb-space-md flex items-center gap-2">
            <span class="w-2 h-2 rounded-full bg-tertiary animate-pulse"></span>
            <span class="material-symbols-outlined text-[16px] text-outline">wifi_off</span>
            <span class="font-body-sm text-body-sm text-on-surface-variant">● Mikareno no conectado — mostrando datos de ejemplo</span>
        </div>"###.to_string()
    }
}

/// Empty/error state with an HTMX partial reload button.
pub fn empty_state_html(message: &str, hx_get: &str) -> String {
    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-6 text-center" id="empty-state">
        <span class="material-symbols-outlined text-[32px] text-outline mb-2">error_outline</span>
        <p class="font-body-sm text-body-sm text-on-surface-variant mb-space-md">{}</p>
        <button hx-get="{}" hx-target="#empty-state" hx-swap="outerHTML"
            class="inline-flex items-center gap-1.5 px-space-md py-2 rounded-lg bg-primary-container text-on-primary-container font-label-md font-semibold hover:bg-primary-fixed-dim transition-colors">
            <span class="material-symbols-outlined text-[16px]">refresh</span> Reintentar
        </button>
    </div>"###,
        html_escape(message),
        hx_get
    )
}

/// Format a float grade for display with one decimal place.
fn fmt_grade(g: f64) -> String {
    format!("{:.1}", g)
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

/// Render subjects as a grid of cards with average grades.
pub fn subject_grid_html(subjects: &[SubjectSummary]) -> String {
    if subjects.is_empty() {
        return empty_state_html("No hay asignaturas disponibles.", "/dashboard/subjects");
    }

    let cards: String = subjects
        .iter()
        .map(|s| {
            let avg = fmt_grade(s.average);
            let color = grade_color(s.average);
            let bar_width = ((s.average / 10.0) * 100.0).clamp(0.0, 100.0).round() as i64;
            format!(
                r###"<a href="/dashboard/subjects" class="block">
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
        return empty_state_html("No hay eventos programados.", "/dashboard/calendar");
    }

    let items: String = events
        .iter()
        .map(|e| {
            let is_today = today.map_or(false, |t| e.fecha.starts_with(t));
            let (bg, dot, badge) = if is_today {
                (
                    "bg-primary-container/10 border-primary-container",
                    "font-code-inline text-code-inline text-primary-container pulse-cyan",
                    r###"<span class="font-label-sm text-[10px] text-primary-container font-semibold">HOY</span>"###,
                )
            } else {
                (
                    "bg-surface-container border-outline",
                    "font-code-inline text-code-inline text-outline",
                    "",
                )
            };
            format!(
                r###"<div class="{} border rounded-xl p-space-sm mb-space-xs">
                    <div class="flex items-center justify-between">
                        <div class="flex items-center space-x-3">
                            <span class="{}">lunes</span>
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

    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-space-md mb-space-md" id="profile-section">
        <div class="flex items-center justify-between">
            <div class="flex items-center gap-space-md">
                <div class="w-16 h-16 rounded-full bg-primary-container/10 flex items-center justify-center text-3xl text-primary-container">N</div>
                <div>
                    <h2 class="font-headline-md text-headline-md text-primary-container">{}</h2>
                    <p class="font-body-sm text-body-sm text-on-surface-variant">{}</p>
                    <p class="font-body-sm text-body-sm text-on-surface-variant">{}</p>
                </div>
            </div>
            <button hx-get="/dashboard?partial=profile" hx-target="#profile-section" hx-swap="outerHTML"
                class="inline-flex items-center gap-1.5 px-space-sm py-1.5 rounded-lg text-on-primary-container bg-primary-container font-label-sm font-semibold hover:bg-primary-fixed-dim transition-colors" title="Actualizar">
                <span class="material-symbols-outlined text-[16px]">refresh</span> Actualizar
            </button>
        </div>
    </div>"###,
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

/// Simulate data when MCP is disconnected.
pub fn mock_dashboard_data() -> String {
    r###"<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-space-md mb-space-md">
        <div class="card rounded-xl p-space-md text-center">
            <div class="font-code-inline text-[11px] text-primary-fixed-dim uppercase">Libros</div>
            <div class="font-label-sm text-[10px] text-on-surface-variant">Asignaturas</div>
            <div class="font-headline-md text-headline-md text-primary-container font-bold">4</div>
        </div>
        <div class="card rounded-xl p-space-md text-center">
            <div class="font-code-inline text-[11px] text-primary-fixed-dim uppercase">Evento</div>
            <div class="font-label-sm text-[10px] text-on-surface-variant">Eventos esta semana</div>
            <div class="font-headline-md text-headline-md text-primary-container font-bold">3</div>
        </div>
        <div class="card rounded-xl p-space-md text-center">
            <div class="font-code-inline text-[11px] text-primary-fixed-dim uppercase">Tareas</div>
            <div class="font-label-sm text-[10px] text-on-surface-variant">Tareas pendientes</div>
            <div class="font-headline-md text-headline-md text-primary-container font-bold">2</div>
        </div>
        <div class="card rounded-xl p-space-md text-center">
            <div class="font-code-inline text-[11px] text-primary-fixed-dim uppercase">Notas</div>
            <div class="font-label-sm text-[10px] text-on-surface-variant">Notas</div>
            <div class="font-headline-md text-headline-md text-primary-container font-bold">7</div>
        </div>
    </div>
    <div class="card rounded-xl p-space-md mb-space-md">
        <h2 class="font-headline-sm text-headline-sm text-primary-container font-semibold mb-space-sm">Estado del MCP</h2>
        <p class="font-body-sm text-body-sm text-on-surface-variant">Mikareno no esta conectado. Mostrando datos de ejemplo.</p>
        <p class="font-body-sm text-body-sm text-on-surface-variant mt-space-xs">Conecta el MCP desde el remoto para ver datos reales.</p>
    </div>"###
        .to_string()
}

/// Mock subject summaries for when MCP is disconnected.
pub fn mock_subjects() -> Vec<SubjectSummary> {
    vec![
        SubjectSummary {
            name: "Matematicas".to_string(),
            average: 8.5,
            count: 2,
        },
        SubjectSummary {
            name: "Lenguaje".to_string(),
            average: 9.0,
            count: 1,
        },
        SubjectSummary {
            name: "Ciencias".to_string(),
            average: 7.5,
            count: 3,
        },
        SubjectSummary {
            name: "Historia".to_string(),
            average: 6.0,
            count: 1,
        },
        SubjectSummary {
            name: "Fisica".to_string(),
            average: 5.5,
            count: 2,
        },
    ]
}

/// Mock perfil for when MCP is disconnected.
pub fn mock_perfil() -> Perfil {
    Perfil {
        nombre: "Gabriel".to_string(),
        institucion: "Colegio Alamo".to_string(),
        curso: "2 Medio A".to_string(),
    }
}

/// Mock calendar events for today — used when MCP is disconnected.
pub fn mock_today_events() -> Vec<CalendarioEvent> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    vec![
        CalendarioEvent {
            fecha: today.clone(),
            evento: "Clase de Matematicas - Unidad 4".to_string(),
            tipo: "SCHOOL_DAY".to_string(),
        },
        CalendarioEvent {
            fecha: today.clone(),
            evento: "Entrega de proyecto de Ciencias".to_string(),
            tipo: "EVENTO".to_string(),
        },
    ]
}

/// Build SubjectSummary list from Nota data by grouping per materia.
pub fn subjects_from_notas(notas: &[Nota]) -> Vec<SubjectSummary> {
    use std::collections::HashMap;
    let mut grouped: HashMap<String, Vec<f64>> = HashMap::new();
    for nota in notas {
        grouped
            .entry(nota.materia.clone())
            .or_default()
            .push(nota.nota);
    }
    let mut subjects: Vec<SubjectSummary> = grouped
        .iter()
        .map(|(name, grades)| {
            let avg = grades.iter().sum::<f64>() / grades.len() as f64;
            SubjectSummary {
                name: name.clone(),
                average: (avg * 100.0).round() / 100.0,
                count: grades.len(),
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

/// Collect dashboard data from the DB and the Mikareno MCP, falling back
/// to Stitch-styled mock data when the MCP is disconnected.
pub async fn build_dashboard_data(state: &Arc<AppState>) -> DashboardData {
    let client = crate::academic::mikareno_client(state);
    let mcp_connected = client.as_ref().map_or(false, |c| c.is_connected());

    // Tasks + count
    let tasks = if let Some(client) = &client {
        client.tareas().await.ok().unwrap_or_default()
    } else {
        mock_tasks_data()
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
        mock_today_events()
    };
    let event_count = events.len();

    // Profile
    let perfil = if let Some(client) = &client {
        client.perfil().await.ok().unwrap_or_else(Perfil::default)
    } else {
        mock_perfil()
    };

    // Subjects (from DB + Mikareno) or Stitch mock subjects
    let subjects = if mcp_connected {
        let notas = client
            .as_ref()
            .unwrap()
            .notas(1)
            .await
            .ok()
            .unwrap_or_default();
        subjects_from_notas(&notas)
    } else {
        mock_subjects()
    };
    let subject_count = subjects.len();

    // Priorities: nearest due tasks when MCP is connected, else mock
    let prioridades = if mcp_connected && tarea_count > 0 {
        tareas.iter().take(5).cloned().collect()
    } else {
        mock_priorities()
    };

    // Real note count from the local DB
    let note_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

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

/// Mock tasks when MCP is disconnected (Tarea domain type).
fn mock_tasks_data() -> Vec<Tarea> {
    vec![
        Tarea {
            titulo: "Resolver ejercicios de cálculo".to_string(),
            vencimiento: "2024-03-10".to_string(),
            estado: "Pendiente".to_string(),
        },
        Tarea {
            titulo: "Leer capítulo 5 de física".to_string(),
            vencimiento: "2024-03-12".to_string(),
            estado: "Pendiente".to_string(),
        },
    ]
}

/// Mock tasks when MCP is disconnected (string list for the card).
fn mock_tasks() -> Vec<String> {
    mock_tasks_data()
        .into_iter()
        .map(|t| format!("{} — vence: {}", t.titulo, t.vencimiento))
        .collect()
}

/// Mock daily priorities when MCP is disconnected.
fn mock_priorities() -> Vec<String> {
    vec![
        "Estudiar para el examen de física".to_string(),
        "Actualizar notas de la semana".to_string(),
        "Responder dudas de la tarea de cálculo".to_string(),
    ]
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
            },
            SubjectSummary {
                name: "Lenguaje".to_string(),
                average: 3.0,
                count: 1,
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
            },
            SubjectSummary {
                name: "Media".to_string(),
                average: 5.0,
                count: 1,
            },
            SubjectSummary {
                name: "Baja".to_string(),
                average: 2.0,
                count: 1,
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
    fn test_mock_subjects() {
        let subjects = mock_subjects();
        assert!(!subjects.is_empty());
        assert!(subjects.iter().any(|s| s.name == "Matematicas"));
    }

    #[test]
    fn test_mock_perfil() {
        let p = mock_perfil();
        assert!(!p.nombre.is_empty());
        assert!(!p.institucion.is_empty());
        assert!(!p.curso.is_empty());
    }

    #[test]
    fn test_mock_today_events() {
        let events = mock_today_events();
        assert!(!events.is_empty());
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert!(events.iter().all(|e| e.fecha == today));
    }

    #[test]
    fn test_subjects_from_notas_groups_and_averages() {
        let notas = vec![
            Nota {
                materia: "Math".to_string(),
                nota: 8.0,
                trimestre: 1,
            },
            Nota {
                materia: "Math".to_string(),
                nota: 10.0,
                trimestre: 2,
            },
            Nota {
                materia: "Science".to_string(),
                nota: 7.0,
                trimestre: 1,
            },
        ];
        let subjects = subjects_from_notas(&notas);
        assert_eq!(subjects.len(), 2);

        let math = subjects.iter().find(|s| s.name == "Math").unwrap();
        assert!((math.average - 9.0).abs() < 1e-9);
        assert_eq!(math.count, 2);

        let science = subjects.iter().find(|s| s.name == "Science").unwrap();
        assert!((science.average - 7.0).abs() < 1e-9);
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
    }
}
