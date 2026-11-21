const std = @import("std");
const assets = @import("assets.zig");
const security = @import("security.zig");
const panel = @import("panel.zig");
const mcp_client = @import("mcp_client.zig");

pub const TemplatePage = enum { dashboard, calendar, assistant, notes, mcp, material, settings };

pub fn contentFor(page: TemplatePage) []const u8 {
    return switch (page) {
        .dashboard => assets.dashboard,
        .calendar => assets.calendar,
        .assistant => assets.assistant,
        .notes => assets.notes,
        .mcp => assets.mcp,
        .material => assets.material,
        .settings => assets.settings,
    };
}

pub fn fillPage(allocator: std.mem.Allocator, page: TemplatePage, source: []const u8, model: []const u8, username: []const u8, datos: panel.Datos, db: anytype) ![]const u8 {
    const safe_model = try security.htmlEscape(allocator, model);
    const safe_name = try security.htmlEscape(allocator, username);
    // Inicio denso: cada hueco lleva datos reales del panel. Si Mikareno no
    // responde, el hueco lo dice en vez de inventar un numero.
    const classes_html = if (datos.horario.len == 0)
        "<div class=\"nx-empty\">Sin clases programadas para hoy.</div>"
    else
        try panel.classesList(allocator, datos);
    const tasks_today_html = try panel.tasksToday(allocator, datos);
    const tasks_table_html = try panel.taskTable(allocator, datos);
    const notes_html = try panel.notesList(allocator, db);
    const weekcal_html = try panel.weekCal(allocator, datos);
    const next_event_html = try panel.nextEvent(allocator, datos);
    const exam_html = try panel.examCard(allocator, datos);
    const subject_options_html = try panel.subjectOptions(allocator, datos);

    // Estado real de los dos MCP para las paginas de diagnostico. `datos`
    // viene de una llamada real: si Mikareno fallo, no se finge conexion.
    const status_on = "<span class=\"nx-status-dot is-on\" aria-hidden=\"true\"></span>";
    const status_off = "<span class=\"nx-status-dot is-off\" aria-hidden=\"true\"></span>";
    const mikareno_status_html = if (datos.connected)
        try std.fmt.allocPrint(allocator, "{s}Conectado", .{status_on})
    else
        try std.fmt.allocPrint(allocator, "{s}Sin conexión", .{status_off});
    const genoffice_status_html = if (datos.genoffice_connected)
        try std.fmt.allocPrint(allocator, "{s}Conectado", .{status_on})
    else
        try std.fmt.allocPrint(allocator, "{s}Sin conexión", .{status_off});
    const servers_line = try std.fmt.allocPrint(allocator, "Mikareno: {s} · GenOffice: {s}", .{
        if (datos.connected) "conectado" else "sin conexión",
        if (datos.genoffice_connected) "conectado" else "sin conexión",
    });
    var tools_buf = std.ArrayList(u8).init(allocator);
    for (datos.mcp_tools, 0..) |tool_name, index| {
        if (index > 0) try tools_buf.appendSlice(" · ");
        try tools_buf.appendSlice(try security.htmlEscape(allocator, tool_name));
    }
    const mikareno_tools_line = if (tools_buf.items.len == 0) "sin catálogo" else tools_buf.items;

    const dashboard_values = [_]Replacement{
        .{ .slot = "AI_MODEL", .value = safe_model },
        .{ .slot = "AI_RESUME", .value = "<p class=\"text-sm text-on-surface-variant\">Pregunta a NOVA AI para empezar una sesión de estudio.</p>" },
        .{ .slot = "CHART_NOTES", .value = try panel.chartNotes(allocator, datos) },
        .{ .slot = "CHART_SUBJECTS", .value = try panel.chartSubjects(allocator, datos) },
        .{ .slot = "CHART_WEEK", .value = try panel.chartWeek(allocator, datos) },
        .{ .slot = "CLASSCOUNT", .value = try panel.syncClasscount(allocator, datos) },
        .{ .slot = "CLASSES", .value = classes_html },
        .{ .slot = "EXAM", .value = exam_html },
        .{ .slot = "FOCUS", .value = "<div class=\"nx-card\"><p class=\"nx-eyebrow\">Enfoque</p><p class=\"text-sm text-on-surface-variant\">Tu espacio de estudio está listo.</p></div>" },
        .{ .slot = "GREETING", .value = safe_name },
        .{ .slot = "KPI_AVG", .value = try panel.kpiAvg(allocator, datos) },
        .{ .slot = "KPI_CLASSES", .value = try panel.kpiClasses(allocator, datos) },
        .{ .slot = "KPI_CONV", .value = try panel.kpiConv(allocator, datos) },
        .{ .slot = "KPI_NOTES", .value = try panel.kpiNotes(allocator, datos) },
        .{ .slot = "KPI_TASKS_TODAY", .value = try panel.kpiTasks(allocator, datos, .today, "pendientes") },
        .{ .slot = "KPI_TASKS_WEEK", .value = try panel.kpiTasks(allocator, datos, .week, "activas") },
        .{ .slot = "NEXT_EVENT", .value = next_event_html },
        .{ .slot = "NOTES", .value = notes_html },
        .{ .slot = "SUBJECT_BARS", .value = try panel.subjectBars(allocator, datos) },
        .{ .slot = "SUBJECT_OPTIONS", .value = subject_options_html },
        .{ .slot = "SYNC", .value = try panel.syncLabel(allocator, datos) },
        .{ .slot = "SYNC_BANNER", .value = try panel.syncBanner(allocator, datos) },
        .{ .slot = "TASKCOUNT", .value = try panel.syncTaskcount(allocator, datos) },
        .{ .slot = "TASKFOOTER", .value = try panel.taskFooter(allocator, datos) },
        .{ .slot = "TASKTABLE", .value = tasks_table_html },
        .{ .slot = "TASKTODAY", .value = tasks_today_html },
        .{ .slot = "TODAYLINE", .value = try panel.todayLine(allocator, datos) },
        .{ .slot = "TRIMESTER", .value = try panel.trimester(allocator, datos) },
        .{ .slot = "WEEKCAL", .value = weekcal_html },
        .{ .slot = "WEEKTITLE", .value = panel.weekTitle() },
    };
    const assistant_values = [_]Replacement{
        .{ .slot = "CHAT_CONTEXT", .value = "Contexto general" },
        .{ .slot = "CHAT_COSTS", .value = "$0.00" },
        .{ .slot = "CHAT_FCOUNT", .value = "0" },
        .{ .slot = "CHAT_FILTERS", .value = "" },
        .{ .slot = "CHAT_MESSAGES", .value = "<div class=\"rounded-xl border border-outline-variant/40 p-4 text-sm text-on-surface-variant\">Hola, <span class=\"font-semibold\">" ++ "<!--USER_NAME-->" ++ "</span>. Escribe una pregunta para empezar.</div>" },
        .{ .slot = "CHAT_MODE", .value = "general" },
        .{ .slot = "CHAT_MODEL", .value = safe_model },
        .{ .slot = "CHAT_MODE_DOT", .value = "<i class=\"w-2 h-2 rounded-full bg-outline\"></i>" },
        .{ .slot = "CHAT_QUOTA_BAR", .value = "" },
        .{ .slot = "CHAT_QUOTA_PCT", .value = "0" },
        .{ .slot = "CHAT_SESSIONS", .value = "" },
        .{ .slot = "CHAT_SOURCES", .value = "" },
    };
    // Calendario mensual: KPIs del mes, rejilla 7x6 y proximos eventos.
    const calendar_values = [_]Replacement{
        .{ .slot = "CAL_DAYS_WITH_CLASS", .value = try panel.calDaysWithClass(allocator, datos) },
        .{ .slot = "CAL_EVENTS_MONTH", .value = try panel.calEventsMonth(allocator, datos) },
        .{ .slot = "CAL_GRID", .value = try panel.calGrid(allocator, datos) },
        .{ .slot = "CAL_SYNC_BANNER", .value = try panel.syncBanner(allocator, datos) },
        .{ .slot = "CAL_TASKS_MONTH", .value = try panel.calTasksMonth(allocator, datos) },
        .{ .slot = "CAL_UPCOMING", .value = try panel.calUpcoming(allocator, datos) },
    };
    const notes_values = [_]Replacement{
        .{ .slot = "BACKLINKS", .value = "" },
        .{ .slot = "BACKLINKS_COUNT", .value = "0" },
        .{ .slot = "GRAPH", .value = "<p class=\"text-sm text-on-surface-variant\">El grafo aparecerá al crear notas.</p>" },
        .{ .slot = "GRAPH_META", .value = "" },
        .{ .slot = "GRAPH_STATS", .value = "0 notas" },
        .{ .slot = "NOTES_COUNT", .value = "0" },
        .{ .slot = "NOTES_LIST", .value = "<p class=\"text-sm text-on-surface-variant\">Todavía no hay notas. Crea una para comenzar.</p>" },
        .{ .slot = "NOTES_RECENT", .value = "" },
        .{ .slot = "NOTE_PREVIEW", .value = "<p class=\"text-sm text-on-surface-variant\">Selecciona o crea una nota.</p>" },
        .{ .slot = "NOTE_RAW", .value = "" },
        .{ .slot = "NOTE_STATS", .value = "0 palabras" },
        .{ .slot = "NOTE_STATUS", .value = "Vault local" },
        .{ .slot = "NOTE_TAGS", .value = "" },
        .{ .slot = "NOTE_TITLE", .value = "Sin nota seleccionada" },
        .{ .slot = "OUTLINKS", .value = "" },
        .{ .slot = "OUTLINKS_COUNT", .value = "0" },
        .{ .slot = "TAGS", .value = "" },
        .{ .slot = "VAULT_PATH", .value = "notes/" },
    };
    const mcp_values = [_]Replacement{
        .{ .slot = "MCP_BUDGET", .value = "2.00" },
        .{ .slot = "MCP_BUDGET_INT", .value = "2" },
        .{ .slot = "MCP_KEY_STATE", .value = "No configurada" },
        .{ .slot = "MCP_LATENCY", .value = "—" },
        .{ .slot = "MCP_MODEL", .value = safe_model },
        .{ .slot = "MCP_MODEL_OPTIONS", .value = "" },
        .{ .slot = "MCP_NATIVE_LOCATION", .value = "127.0.0.1" },
        .{ .slot = "MCP_NATIVE_TOOLS", .value = "9 herramientas" },
        .{ .slot = "MCP_ORCHESTRATOR", .value = "Study Platform · Zig" },
        .{ .slot = "MCP_SERVERS_LINE", .value = servers_line },
        .{ .slot = "MCP_SPEND", .value = "0.00" },
        .{ .slot = "MCP_STATUS_AI", .value = "Modo demo" },
        .{ .slot = "MCP_STATUS_GENOFFICE", .value = genoffice_status_html },
        .{ .slot = "MCP_STATUS_MIKARENO", .value = mikareno_status_html },
        .{ .slot = "MCP_TELEMETRY", .value = "" },
        .{ .slot = "MCP_TOOLS", .value = mikareno_tools_line },
        .{ .slot = "MCP_TRANSCRIPT", .value = "" },
        .{ .slot = "MCP_URL_GENOFFICE", .value = "127.0.0.1:8453" },
        .{ .slot = "MCP_URL_MIKARENO", .value = "127.0.0.1:8080" },
    };
    const material_values = [_]Replacement{
        .{ .slot = "AI_DOT", .value = "<span class=\"w-2 h-2 rounded-full bg-outline\"></span>" },
        .{ .slot = "AI_MODEL", .value = safe_model },
        .{ .slot = "INITIAL_HINT", .value = "<p class=\"text-sm text-on-surface-variant\">Elige asignatura, tema y formato para preparar material.</p>" },
        .{ .slot = "MATERIA_OPTIONS", .value = "<option value=\"\">Selecciona una asignatura</option>" },
        .{ .slot = "MCP_STATUS", .value = "Mikareno no está conectado" },
    };
    const settings_values = [_]Replacement{
        .{ .slot = "AI_BADGE", .value = if (model.len > 0) "Configurado" else "Modo demo" },
        .{ .slot = "AI_BUDGET", .value = "2.00" },
        .{ .slot = "AI_DOT", .value = "<span class=\"w-2 h-2 rounded-full bg-outline\"></span>" },
        .{ .slot = "AI_DOT2", .value = "<span class=\"w-2 h-2 rounded-full bg-outline\"></span>" },
        .{ .slot = "AI_ENV", .value = "OPENROUTER_API_KEY=(guardada solo en .env)" },
        .{ .slot = "AI_FORM", .value = "" },
        .{ .slot = "AI_MAX_TOKENS", .value = "8192" },
        .{ .slot = "AI_MODEL", .value = safe_model },
        .{ .slot = "AI_STATUS", .value = "Sin clave API" },
        .{ .slot = "AUTH_MODE", .value = "Protegido por sesión" },
        .{ .slot = "MCP_DOT", .value = if (datos.connected) status_on else status_off },
        .{ .slot = "MCP_STATUS", .value = if (datos.connected) "Conectado" else "Mikareno no responde ahora" },
        .{ .slot = "MCP_URL", .value = "127.0.0.1:8080" },
        .{ .slot = "VERSION", .value = "0.2.0" },
    };

    const replacements: []const Replacement = switch (page) {
        .dashboard => &dashboard_values,
        .calendar => &calendar_values,
        .assistant => &assistant_values,
        .notes => &notes_values,
        .mcp => &mcp_values,
        .material => &material_values,
        .settings => &settings_values,
    };
    return renderSlots(
        allocator,
        source,
        replacements,
        if (page == .assistant) safe_name else null,
    );
}

const Replacement = struct { slot: []const u8, value: []const u8 };
const SLOT_PREFIX = "<!--SLOT:";
const SLOT_SUFFIX = "-->";
const USER_NAME_MARKER = "<!--USER_NAME-->";

/// Sustituye todos los slots en un recorrido y un único buffer de salida.
/// Evita copiar la página completa una vez por cada marcador.
fn renderSlots(
    allocator: std.mem.Allocator,
    source: []const u8,
    replacements: []const Replacement,
    username: ?[]const u8,
) ![]const u8 {
    var output: std.ArrayListUnmanaged(u8) = .empty;
    try output.ensureTotalCapacity(allocator, source.len);
    var cursor: usize = 0;

    while (cursor < source.len) {
        const slot_start = findFrom(source, cursor, SLOT_PREFIX);
        const user_start = if (username != null)
            findFrom(source, cursor, USER_NAME_MARKER)
        else
            null;
        var marker_start = slot_start orelse user_start orelse {
            try output.appendSlice(allocator, source[cursor..]);
            break;
        };
        if (user_start) |start| marker_start = @min(marker_start, start);

        try output.appendSlice(allocator, source[cursor..marker_start]);
        if (user_start) |start| {
            if (start == marker_start) {
                try output.appendSlice(allocator, username orelse unreachable);
                cursor = marker_start + USER_NAME_MARKER.len;
                continue;
            }
        }

        const value_start = marker_start + SLOT_PREFIX.len;
        const slot_end = findFrom(source, value_start, SLOT_SUFFIX) orelse break;
        if (replacementFor(replacements, source[value_start..slot_end])) |value| {
            try output.appendSlice(allocator, value);
        }
        cursor = slot_end + SLOT_SUFFIX.len;
    }

    return output.toOwnedSlice(allocator);
}

fn findFrom(source: []const u8, start: usize, needle: []const u8) ?usize {
    const relative = std.mem.indexOf(u8, source[start..], needle) orelse return null;
    return start + relative;
}

fn replacementFor(replacements: []const Replacement, slot: []const u8) ?[]const u8 {
    for (replacements) |replacement| {
        if (std.mem.eql(u8, replacement.slot, slot)) return replacement.value;
    }
    return null;
}

pub fn renderShell(allocator: std.mem.Allocator, active_route: []const u8, content: []const u8, username: []const u8, authenticated: bool, mcp_url: []const u8) ![]const u8 {
    const safe_user = try security.htmlEscape(allocator, username);
    const initial = if (username.len > 0) username[0..1] else "A";
    const avatar = try std.fmt.allocPrint(allocator, "<span class=\"w-8 h-8 rounded-full bg-primary-container text-on-primary-container flex items-center justify-center font-semibold\" aria-hidden=\"true\">{s}</span>", .{try security.htmlEscape(allocator, initial)});
    var aside_html: []const u8 = try std.mem.replaceOwned(u8, allocator, assets.aside, "<!--SLOT:SHELL_NAV_MCP-->", "9");
    aside_html = try std.mem.replaceOwned(u8, allocator, aside_html, "<!--SLOT:SHELL_VAULT_PCT-->", "0");
    aside_html = try markActiveNav(allocator, aside_html, active_route);
    aside_html = try std.mem.replaceOwned(u8, allocator, aside_html, "<aside class=\"app-rail nx-sidebar\"", "<aside id=\"app-rail\" class=\"app-rail nx-sidebar\"");

    // El estado del topbar sale de un sondeo real (cacheado 30 s). Antes era
    // texto fijo y decia "sin conexion" incluso con Mikareno respondiendo.
    const mikareno_ok = mcp_client.probeCached(allocator, mcp_client.slot_mikareno, mcp_url, 800, true);
    const status_html = if (mikareno_ok)
        "<span class=\"nx-status-dot is-on\" aria-hidden=\"true\"></span><span>Mikareno conectado</span>"
    else
        "<span class=\"nx-status-dot is-off\" aria-hidden=\"true\"></span><span>Mikareno sin conexión</span>";
    var topbar_html: []const u8 = try std.mem.replaceOwned(u8, allocator, assets.topbar, "<!--SLOT:SHELL_STATUS-->", status_html);
    topbar_html = try std.mem.replaceOwned(u8, allocator, topbar_html, "<!--SLOT:SHELL_AVATAR-->", avatar);
    topbar_html = try std.mem.replaceOwned(u8, allocator, topbar_html, "<!--SLOT:SHELL_NAME-->", safe_user);
    topbar_html = try std.mem.replaceOwned(u8, allocator, topbar_html, "<!--SLOT:SHELL_ROLE-->", "Estudiante");
    topbar_html = try std.mem.replaceOwned(u8, allocator, topbar_html, "<header class=\"app-topbar nx-topbar\"", "<header id=\"app-topbar\" class=\"app-topbar nx-topbar\"");
    // `mcp_url` no se inserta en el shell ni se expone en HTML.
    _ = mcp_url;

    const css = "<style>html,body{margin:0;background:var(--nx-bg,#080b14);color:var(--nx-text,#f3f6ff)}.app-rail{position:fixed;inset:0 auto 0 0;width:250px;overflow:auto;z-index:50}.app-rail-content{margin-left:250px}.app-topbar{position:sticky;top:0;z-index:40}.skip-link{position:absolute;left:-9999px;top:0;background:#a78bfa;color:#140d26;padding:.75rem;z-index:100}.skip-link:focus{left:1rem}.app-main{min-height:100vh;padding-top:4rem}@media(max-width:900px){.app-rail{transform:translateX(-100%)}.app-rail-content{margin-left:0}.app-topbar{left:0!important}}</style>";
    const head = try std.fmt.allocPrint(allocator, "<head>{s}{s}<script>window.__STITCH_V2_AUTH__={{\"authenticated\":{s}}}</script></head>", .{ assets.head, css, if (authenticated) "true" else "false" });
    return std.fmt.allocPrint(
        allocator,
        "<!DOCTYPE html>\n<html class=\"dark\" lang=\"es\">{s}<body class=\"bg-background font-body-md text-body-md text-on-surface antialiased\"><a class=\"skip-link\" href=\"#contenido\">Saltar al contenido</a>{s}<div id=\"app-rail-content\" class=\"app-rail-content\">{s}<main id=\"contenido\" class=\"app-main relative w-full bg-background\">{s}</main></div><script src=\"/calendar-ics.js\" defer></script><script src=\"/shell.js\" defer></script></body></html>",
        .{ head, aside_html, topbar_html, content },
    );
}

fn markActiveNav(allocator: std.mem.Allocator, html: []const u8, route: []const u8) ![]const u8 {
    const active = primaryPath(route);
    var out: std.ArrayListUnmanaged(u8) = .empty;
    var rest = html;
    while (std.mem.indexOf(u8, rest, "<a ")) |start| {
        try out.appendSlice(allocator, rest[0..start]);
        const after = rest[start..];
        const tag_end = std.mem.indexOfScalar(u8, after, '>') orelse {
            try out.appendSlice(allocator, after);
            return out.items;
        };
        const close = std.mem.indexOf(u8, after[tag_end..], "</a>") orelse {
            try out.appendSlice(allocator, after);
            return out.items;
        };
        const tag = after[0 .. tag_end + 1];
        const segment = after[0 .. tag_end + close + 4];
        const data_path = attrValue(tag, "data-path") orelse "";
        const href = routeForPath(data_path);
        var new_tag = try std.mem.replaceOwned(u8, allocator, tag, "href=\"#\"", try std.fmt.allocPrint(allocator, "href=\"{s}\"", .{href}));
        new_tag = try std.mem.replaceOwned(u8, allocator, new_tag, " aria-current=\"page\"", "");
        if (std.mem.eql(u8, data_path, active)) {
            new_tag = try std.mem.replaceOwned(u8, allocator, new_tag, "class=\"", "aria-current=\"page\" class=\"");
            new_tag = try std.mem.replaceOwned(u8, allocator, new_tag, "class=\"app-nav-link nx-nav-link", "class=\"app-nav-link nx-nav-link bg-primary-container text-on-primary-container font-semibold");
        }
        try out.appendSlice(allocator, new_tag);
        try out.appendSlice(allocator, segment[tag_end + 1 ..]);
        rest = after[tag_end + close + 4 ..];
    }
    try out.appendSlice(allocator, rest);
    return out.items;
}

fn attrValue(tag: []const u8, name: []const u8) ?[]const u8 {
    var cursor: usize = 0;
    while (std.mem.indexOf(u8, tag[cursor..], name)) |relative| {
        const start = cursor + relative;
        const value_start = start + name.len;
        if (value_start + 1 < tag.len and
            tag[value_start] == '=' and
            tag[value_start + 1] == '"')
        {
            const content_start = value_start + 2;
            const end = std.mem.indexOfScalarPos(u8, tag, content_start, '"') orelse return null;
            return tag[content_start..end];
        }
        cursor = value_start;
    }
    return null;
}

fn primaryPath(route: []const u8) []const u8 {
    if (std.mem.eql(u8, route, "/") or std.mem.eql(u8, route, "/dashboard")) return "panel-principal";
    if (std.mem.startsWith(u8, route, "/dashboard/calendar")) return "mi-agenda";
    if (std.mem.startsWith(u8, route, "/dashboard/planner")) return "planificacion";
    if (std.mem.startsWith(u8, route, "/dashboard/subjects")) return "asignaturas";
    if (std.mem.startsWith(u8, route, "/dashboard/examenes")) return "examenes";
    if (std.mem.startsWith(u8, route, "/dashboard/vida-escolar")) return "vida-escolar";
    if (std.mem.startsWith(u8, route, "/dashboard/horario")) return "horarios";
    if (std.mem.startsWith(u8, route, "/dashboard/ai") or std.mem.eql(u8, route, "/ai")) return "asistente-academico";
    if (std.mem.startsWith(u8, route, "/dashboard/material")) return "generador-de-material";
    if (std.mem.startsWith(u8, route, "/dashboard/notes") or std.mem.startsWith(u8, route, "/app/notes")) return "segundo-cerebro";
    if (std.mem.startsWith(u8, route, "/dashboard/search")) return "busqueda-global";
    if (std.mem.startsWith(u8, route, "/office/docs")) return "documentos";
    if (std.mem.startsWith(u8, route, "/office/sheets")) return "hojas-de-calculo";
    if (std.mem.startsWith(u8, route, "/office/presentations")) return "presentaciones";
    if (std.mem.startsWith(u8, route, "/office/library")) return "biblioteca-de-archivos";
    if (std.mem.startsWith(u8, route, "/dashboard/mcp")) return "centro-mcp";
    if (std.mem.startsWith(u8, route, "/admin")) return "administracion";
    if (std.mem.startsWith(u8, route, "/settings")) return "configuracion";
    return "";
}

fn routeForPath(data_path: []const u8) []const u8 {
    const pairs = [_]struct { key: []const u8, route: []const u8 }{
        .{ .key = "panel-principal", .route = "/dashboard" },
        .{ .key = "mi-agenda", .route = "/dashboard/calendar" },
        .{ .key = "planificacion", .route = "/dashboard/planner" },
        .{ .key = "asignaturas", .route = "/dashboard/subjects" },
        .{ .key = "tareas-y-actividades", .route = "/dashboard/planner" },
        .{ .key = "examenes", .route = "/dashboard/examenes" },
        .{ .key = "vida-escolar", .route = "/dashboard/vida-escolar" },
        .{ .key = "horarios", .route = "/dashboard/horario" },
        .{ .key = "asistente-academico", .route = "/dashboard/ai" },
        .{ .key = "conversaciones", .route = "/ai" },
        .{ .key = "generador-de-material", .route = "/dashboard/material" },
        .{ .key = "mis-notas", .route = "/dashboard/notes" },
        .{ .key = "segundo-cerebro", .route = "/dashboard/notes" },
        .{ .key = "busqueda-global", .route = "/dashboard/search" },
        .{ .key = "documentos", .route = "/office/docs" },
        .{ .key = "hojas-de-calculo", .route = "/office/sheets" },
        .{ .key = "presentaciones", .route = "/office/presentations" },
        .{ .key = "biblioteca-de-archivos", .route = "/office/library" },
        .{ .key = "centro-mcp", .route = "/dashboard/mcp" },
        .{ .key = "estado-de-conexiones", .route = "/dashboard/mcp" },
        .{ .key = "administracion", .route = "/admin" },
        .{ .key = "configuracion", .route = "/settings" },
    };
    for (pairs) |p| if (std.mem.eql(u8, p.key, data_path)) return p.route;
    return "/dashboard";
}

pub fn loginPage() []const u8 {
    return "<!DOCTYPE html><html lang=\"es\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Iniciar sesión · NOVA</title><link rel=\"stylesheet\" href=\"/nova.css\"><style>body{margin:0;background:#080b14;color:#f3f6ff;font:16px system-ui;display:grid;place-items:center;min-height:100vh}.card{width:min(92vw,420px);padding:2rem;border:1px solid #2c3a56;border-radius:1rem;background:#141d30}label{display:block;margin:.8rem 0 .3rem}input{width:100%;box-sizing:border-box;padding:.8rem;background:#090e19;color:white;border:1px solid #71809e;border-radius:.5rem}button{margin-top:1.2rem;width:100%;padding:.8rem;background:#a78bfa;color:#140d26;border:0;border-radius:.5rem;font-weight:700}</style></head><body><main class=\"card\"><p>NOVA · PERSONAL OS</p><h1>Inicia sesión</h1><form method=\"post\" action=\"/login\"><label for=\"username\">Usuario</label><input id=\"username\" name=\"username\" autocomplete=\"username\" required><label for=\"password\">Contraseña</label><input id=\"password\" name=\"password\" type=\"password\" autocomplete=\"current-password\" required><button type=\"submit\">Entrar a mi espacio</button></form><p>Tu sesión se protege con una cookie HttpOnly.</p></main></body></html>";
}

pub fn simplePage(allocator: std.mem.Allocator, title: []const u8, body: []const u8) ![]const u8 {
    return std.fmt.allocPrint(allocator, "<section class=\"max-w-5xl mx-auto p-6\"><p class=\"uppercase tracking-widest text-xs text-outline\">NOVA · espacio académico</p><h1 class=\"text-3xl font-semibold my-4\">{s}</h1>{s}</section>", .{ title, body });
}

test "page slots render in one pass and clear unknown markers" {
    const replacements = [_]Replacement{
        .{ .slot = "TITLE", .value = "<strong>NOVA</strong>" },
    };
    const rendered = try renderSlots(
        std.testing.allocator,
        "<main><!--SLOT:TITLE--><!--SLOT:MISSING--></main><!--USER_NAME--><!--SLOT:TITLE-->",
        &replacements,
        "Ada",
    );
    defer std.testing.allocator.free(rendered);
    try std.testing.expectEqualStrings(
        "<main><strong>NOVA</strong></main>Ada<strong>NOVA</strong>",
        rendered,
    );
}

test "non assistant pages retain the user marker" {
    const rendered = try renderSlots(
        std.testing.allocator,
        "<!--USER_NAME--><!--SLOT:UNKNOWN-->",
        &.{},
        null,
    );
    defer std.testing.allocator.free(rendered);
    try std.testing.expectEqualStrings("<!--USER_NAME-->", rendered);
}

test "active route maps to an existing nav anchor" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const marked = try markActiveNav(arena.allocator(), assets.aside, "/dashboard/notes");
    try std.testing.expect(std.mem.indexOf(u8, marked, "href=\"/dashboard/notes\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, marked, "aria-current=\"page\"") != null);
}
