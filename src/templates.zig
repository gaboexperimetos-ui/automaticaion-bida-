const std = @import("std");
const assets = @import("assets.zig");
const security = @import("security.zig");

pub const TemplatePage = enum { dashboard, assistant, notes, mcp, material, settings };

pub fn contentFor(page: TemplatePage) []const u8 {
    return switch (page) {
        .dashboard => assets.dashboard,
        .assistant => assets.assistant,
        .notes => assets.notes,
        .mcp => assets.mcp,
        .material => assets.material,
        .settings => assets.settings,
    };
}

pub fn fillPage(allocator: std.mem.Allocator, page: TemplatePage, source: []const u8, model: []const u8, username: []const u8) ![]const u8 {
    const safe_model = try security.htmlEscape(allocator, model);
    const safe_name = try security.htmlEscape(allocator, username);
    const dashboard_values = [_]Replacement{
        .{ .slot = "AI_MODEL", .value = safe_model },
        .{ .slot = "AI_RESUME", .value = "<p class=\"text-sm text-on-surface-variant\">Pregunta a NOVA AI para empezar una sesión de estudio.</p>" },
        .{ .slot = "CLASSCOUNT", .value = "Sincronización pendiente" },
        .{ .slot = "CLASSES", .value = "<p class=\"text-sm text-on-surface-variant\">Mikareno no responde ahora; no se muestran clases inventadas.</p>" },
        .{ .slot = "EXAM", .value = "<div class=\"p-space-md\"><p class=\"nx-eyebrow\">Próxima evaluación</p><p class=\"text-sm text-on-surface-variant\">Sin datos académicos disponibles.</p></div>" },
        .{ .slot = "FOCUS", .value = "<div class=\"p-space-md\"><p class=\"nx-eyebrow\">Enfoque</p><p class=\"text-sm text-on-surface-variant\">Tu espacio de estudio está listo.</p></div>" },
        .{ .slot = "GREETING", .value = safe_name },
        .{ .slot = "NOTES", .value = "<p class=\"text-sm text-on-surface-variant\">Todavía no hay notas locales.</p>" },
        .{ .slot = "SUBJECT_OPTIONS", .value = "<option value=\"\">Selecciona una asignatura</option>" },
        .{ .slot = "SYNC", .value = "Mikareno desconectado" },
        .{ .slot = "TASKCOUNT", .value = "0" },
        .{ .slot = "TASKFOOTER", .value = "Mikareno no responde ahora" },
        .{ .slot = "TASKTABLE", .value = "<tr><td colspan=\"6\" class=\"py-6 text-center text-on-surface-variant\">No hay tareas disponibles; no se inventan datos.</td></tr>" },
        .{ .slot = "TASKTODAY", .value = "<p class=\"text-sm text-on-surface-variant\">No se pudieron sincronizar las entregas.</p>" },
        .{ .slot = "TODAYLINE", .value = "Tu espacio académico personal" },
        .{ .slot = "WEEKCAL", .value = "<p class=\"text-sm text-on-surface-variant\">Calendario sin conexión</p>" },
        .{ .slot = "WEEKTITLE", .value = "Semana actual" },
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
        .{ .slot = "MCP_SERVERS_LINE", .value = "Mikareno: sin conexión" },
        .{ .slot = "MCP_SPEND", .value = "0.00" },
        .{ .slot = "MCP_STATUS_AI", .value = "Modo demo" },
        .{ .slot = "MCP_STATUS_GENOFFICE", .value = "Sin conexión" },
        .{ .slot = "MCP_STATUS_MIKARENO", .value = "Sin conexión" },
        .{ .slot = "MCP_TELEMETRY", .value = "" },
        .{ .slot = "MCP_TOOLS", .value = "search_notes · get_note · create_note · get_academic_context" },
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
        .{ .slot = "MCP_DOT", .value = "<span class=\"w-2 h-2 rounded-full bg-outline\"></span>" },
        .{ .slot = "MCP_STATUS", .value = "Mikareno no responde ahora" },
        .{ .slot = "MCP_URL", .value = "127.0.0.1:8080" },
        .{ .slot = "VERSION", .value = "0.2.0" },
    };

    var out: []const u8 = source;
    const replacements: []const Replacement = switch (page) {
        .dashboard => &dashboard_values,
        .assistant => &assistant_values,
        .notes => &notes_values,
        .mcp => &mcp_values,
        .material => &material_values,
        .settings => &settings_values,
    };
    for (replacements) |r| {
        const needle = try std.fmt.allocPrint(allocator, "<!--SLOT:{s}-->", .{r.slot});
        out = try std.mem.replaceOwned(u8, allocator, out, needle, r.value);
    }
    if (page == .assistant) out = try std.mem.replaceOwned(u8, allocator, out, "<!--USER_NAME-->", safe_name);
    out = try clearUnknownSlots(allocator, out);
    return out;
}

const Replacement = struct { slot: []const u8, value: []const u8 };

fn clearUnknownSlots(allocator: std.mem.Allocator, source: []const u8) ![]const u8 {
    var output: std.ArrayListUnmanaged(u8) = .empty;
    var rest = source;
    while (std.mem.indexOf(u8, rest, "<!--SLOT:")) |start| {
        try output.appendSlice(allocator, rest[0..start]);
        const after = rest[start..];
        const end = std.mem.indexOf(u8, after, "-->") orelse {
            rest = "";
            break;
        };
        rest = after[end + 3 ..];
    }
    try output.appendSlice(allocator, rest);
    return output.items;
}

pub fn renderShell(allocator: std.mem.Allocator, active_route: []const u8, content: []const u8, username: []const u8, authenticated: bool, mcp_url: []const u8) ![]const u8 {
    const safe_user = try security.htmlEscape(allocator, username);
    const initial = if (username.len > 0) username[0..1] else "A";
    const avatar = try std.fmt.allocPrint(allocator, "<span class=\"w-8 h-8 rounded-full bg-primary-container text-on-primary-container flex items-center justify-center font-semibold\" aria-hidden=\"true\">{s}</span>", .{try security.htmlEscape(allocator, initial)});
    var aside_html: []const u8 = try std.mem.replaceOwned(u8, allocator, assets.aside, "<!--SLOT:SHELL_NAV_MCP-->", "9");
    aside_html = try std.mem.replaceOwned(u8, allocator, aside_html, "<!--SLOT:SHELL_VAULT_PCT-->", "0");
    aside_html = try markActiveNav(allocator, aside_html, active_route);
    aside_html = try std.mem.replaceOwned(u8, allocator, aside_html, "<aside class=\"app-rail nx-sidebar\"", "<aside id=\"app-rail\" class=\"app-rail nx-sidebar\"");

    var topbar_html: []const u8 = try std.mem.replaceOwned(u8, allocator, assets.topbar, "<!--SLOT:SHELL_STATUS-->", "<span class=\"w-2 h-2 rounded-full bg-outline\"></span><span>Mikareno sin conexión</span>");
    topbar_html = try std.mem.replaceOwned(u8, allocator, topbar_html, "<!--SLOT:SHELL_AVATAR-->", avatar);
    topbar_html = try std.mem.replaceOwned(u8, allocator, topbar_html, "<!--SLOT:SHELL_NAME-->", safe_user);
    topbar_html = try std.mem.replaceOwned(u8, allocator, topbar_html, "<!--SLOT:SHELL_ROLE-->", "Estudiante");
    topbar_html = try std.mem.replaceOwned(u8, allocator, topbar_html, "<header class=\"app-topbar nx-topbar\"", "<header id=\"app-topbar\" class=\"app-topbar nx-topbar\"");
    const safe_mcp_url = try security.htmlEscape(allocator, mcp_url);
    _ = safe_mcp_url; // La URL no se inserta en el shell ni se expone en HTML.

    const css = "<style>html,body{margin:0;background:#080b14;color:#f3f6ff}.app-rail{position:fixed;inset:0 auto 0 0;width:250px;overflow:auto;z-index:50}.app-rail-content{margin-left:250px}.app-topbar{position:sticky;top:0;z-index:40}.skip-link{position:absolute;left:-9999px;top:0;background:#a78bfa;color:#140d26;padding:.75rem;z-index:100}.skip-link:focus{left:1rem}.app-main{min-height:100vh;padding-top:4rem}@media(max-width:900px){.app-rail{transform:translateX(-100%)}.app-rail-content{margin-left:0}.app-topbar{left:0!important}}</style>";
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
    const prefix = std.fmt.allocPrint(std.heap.page_allocator, "{s}=\"", .{name}) catch return null;
    defer std.heap.page_allocator.free(prefix);
    const start = std.mem.indexOf(u8, tag, prefix) orelse return null;
    const value_start = start + prefix.len;
    const end = std.mem.indexOfScalarPos(u8, tag, value_start, '"') orelse return null;
    return tag[value_start..end];
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

test "active route maps to an existing nav anchor" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const marked = try markActiveNav(arena.allocator(), assets.aside, "/dashboard/notes");
    try std.testing.expect(std.mem.indexOf(u8, marked, "href=\"/dashboard/notes\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, marked, "aria-current=\"page\"") != null);
}
