//! Búsqueda global: notas locales, datos de Mikareno y datos de Google
//! (Calendar, Drive y Contacts) en una sola página.
//!
//! Reglas del módulo, iguales que el resto de NOVA:
//!
//! * **Real o nada**: si una fuente no está conectada o no autorizada se
//!   dice por qué; nunca se rellena con resultados inventados.
//! * **Escape**: todo texto externo pasa por `security.htmlEscape`.
//! * **Sin estado global**: `page` recibe la consulta y devuelve el HTML.

const std = @import("std");
const json = @import("jsonutil.zig");
const security = @import("security.zig");
const sqlite = @import("sqlite.zig");
const config_mod = @import("config.zig");
const mcp_client = @import("mcp_client.zig");
const oauth = @import("oauth_google.zig");
const gcal = @import("google_calendar.zig");
const gws = @import("google_workspace.zig");

pub const Item = struct {
    kind: []const u8,
    title: []const u8,
    subtitle: []const u8,
    url: []const u8,
};

pub const Results = struct {
    notes: std.ArrayList(Item),
    mikareno: std.ArrayList(Item),
    google: std.ArrayList(Item),
    mikareno_connected: bool = false,
    mikareno_error: []const u8 = "",
    google_authorized: bool = false,
    google_reason: []const u8 = "",
};

fn lowerAscii(c: u8) u8 {
    return if (c >= 'A' and c <= 'Z') c + 32 else c;
}

/// `true` si `haystack` contiene `needle` sin importar mayúsculas ASCII.
/// `needle` debe venir ya en minúsculas y recortado.
pub fn containsIgnoreCase(haystack: []const u8, needle: []const u8) bool {
    if (needle.len == 0) return true;
    if (haystack.len < needle.len) return false;
    var i: usize = 0;
    while (i + needle.len <= haystack.len) : (i += 1) {
        var j: usize = 0;
        while (j < needle.len) : (j += 1) {
            if (lowerAscii(haystack[i + j]) != needle[j]) break;
        }
        if (j == needle.len) return true;
    }
    return false;
}

fn pushLower(allocator: std.mem.Allocator, s: []const u8) ![]const u8 {
    const out = try allocator.alloc(u8, s.len);
    for (s, 0..) |c, i| out[i] = lowerAscii(c);
    return out;
}

fn maybeAdd(
    allocator: std.mem.Allocator,
    list: *std.ArrayList(Item),
    needle: []const u8,
    kind: []const u8,
    title: []const u8,
    subtitle: []const u8,
    url: []const u8,
) !void {
    if (title.len == 0) return;
    if (!containsIgnoreCase(title, needle) and !containsIgnoreCase(subtitle, needle)) return;
    try list.append(.{
        .kind = kind,
        .title = try allocator.dupe(u8, title),
        .subtitle = try allocator.dupe(u8, subtitle),
        .url = try allocator.dupe(u8, url),
    });
}

fn searchNotes(allocator: std.mem.Allocator, db: *sqlite.Db, needle: []const u8, out: *std.ArrayList(Item)) !void {
    const pattern = try std.fmt.allocPrint(allocator, "%{s}%", .{needle});
    var stmt = db.prepare(
        "SELECT n.slug,n.title,COALESCE(n.summary,''),COALESCE(n.tags,'') FROM notes n " ++
            "WHERE n.title LIKE ?1 OR n.tags LIKE ?1 OR n.summary LIKE ?1 " ++
            "OR EXISTS(SELECT 1 FROM note_versions v WHERE v.note_id=n.id AND v.content LIKE ?1) " ++
            "ORDER BY n.updated_at DESC LIMIT 30",
    ) catch return;
    defer stmt.deinit();
    try stmt.bindText(1, pattern);
    while (stmt.next() catch false) {
        const slug = stmt.text(0);
        const title = stmt.text(1);
        const summary = stmt.text(2);
        const tags = stmt.text(3);
        try out.append(.{
            .kind = "Nota",
            .title = try allocator.dupe(u8, title),
            .subtitle = try std.fmt.allocPrint(allocator, "{s}{s}{s}", .{
                summary,
                if (summary.len > 0 and tags.len > 0) " · " else "",
                tags,
            }),
            .url = try std.fmt.allocPrint(allocator, "/app/notes/{s}", .{slug}),
        });
    }
}

fn notaTexto(allocator: std.mem.Allocator, item: json.Value) ![]const u8 {
    const v = json.get(item, "nota") orelse json.get(item, "average") orelse json.get(item, "promedio") orelse return "";
    return switch (v) {
        .float => |f| std.fmt.allocPrint(allocator, "{d:.2}", .{f}),
        .integer => |i| std.fmt.allocPrint(allocator, "{d}", .{i}),
        else => "",
    };
}

fn searchMikareno(
    allocator: std.mem.Allocator,
    config: *const config_mod.Config,
    needle: []const u8,
    out: *std.ArrayList(Item),
) struct { connected: bool, error_message: []const u8 } {
    if (config.mikareno_mcp_url.len == 0) return .{ .connected = false, .error_message = "Mikareno no está configurado en el servidor." };
    const full = mcp_client.callTool(
        allocator,
        config.mikareno_mcp_url,
        config.mcp_timeout_ms,
        "get_student_full",
        json.object(allocator, &.{}) catch return .{ .connected = false, .error_message = "No se pudo preparar la consulta a Mikareno." },
        true,
    ) catch return .{ .connected = false, .error_message = "Mikareno no respondió." };

    // Tareas del planificador.
    if (json.get(full, "tareas_paginadas")) |tp| {
        if (json.get(tp, "pageContent")) |page_content| {
            if (page_content == .array) for (page_content.array.items) |t| {
                if (t != .object) continue;
                const titulo = json.getString(t, "titulo") orelse "";
                const venc = json.getString(t, "vencimiento") orelse "";
                const sub = if (venc.len > 0) std.fmt.allocPrint(allocator, "Vence: {s}", .{venc}) catch "" else "Tarea de Mikareno";
                maybeAdd(allocator, out, needle, "Tarea", titulo, sub, "/dashboard/planner") catch {};
            };
        }
    }
    // Eventos del calendario académico.
    if (json.get(full, "calendario")) |cal| {
        if (cal == .array) for (cal.array.items) |e| {
            if (e != .object) continue;
            const titulo = json.getString(e, "titulo") orelse json.getString(e, "title") orelse "";
            const fecha = json.getString(e, "fecha") orelse json.getString(e, "date") orelse "";
            maybeAdd(allocator, out, needle, "Evento", titulo, fecha, "/dashboard/calendar") catch {};
        };
    }
    // Promedios por materia.
    if (json.get(full, "promedios")) |prom| {
        if (json.get(prom, "stats")) |stats| {
            if (stats == .array) for (stats.array.items) |s| {
                if (s != .object) continue;
                const materia = json.getString(s, "materia") orelse json.getString(s, "courseName") orelse json.getString(s, "name") orelse "";
                const nota = notaTexto(allocator, s) catch "";
                const sub = if (nota.len > 0) std.fmt.allocPrint(allocator, "Nota {s}", .{nota}) catch "Materia" else "Materia";
                maybeAdd(allocator, out, needle, "Materia", materia, sub, "/dashboard/subjects") catch {};
            };
        }
    }
    // Horario semanal.
    if (json.get(full, "horario")) |hor| {
        if (hor == .array) for (hor.array.items) |h| {
            if (h != .object) continue;
            const materia = json.getString(h, "materia") orelse json.getString(h, "courseName") orelse json.getString(h, "name") orelse "";
            const dia = json.getString(h, "dia") orelse json.getString(h, "day") orelse "";
            const hora = json.getString(h, "hora") orelse json.getString(h, "startTime") orelse "";
            const sub = if (dia.len > 0 and hora.len > 0)
                std.fmt.allocPrint(allocator, "{s} · {s}", .{ dia, hora }) catch "Horario"
            else if (dia.len > 0) dia else "Horario";
            maybeAdd(allocator, out, needle, "Horario", materia, sub, "/dashboard/horario") catch {};
        };
    }
    return .{ .connected = true, .error_message = "" };
}

fn searchGoogle(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    config: *const config_mod.Config,
    user_id: i64,
    needle: []const u8,
    out: *std.ArrayList(Item),
) struct { authorized: bool, reason: []const u8 } {
    const cfg = oauth.Config{
        .client_id = config.google_client_id,
        .client_secret = config.google_client_secret,
        .redirect_uri = config.google_redirect_uri,
        .timeout_ms = config.oauth_timeout_ms,
    };
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) {
        return .{ .authorized = false, .reason = "Google OAuth no está configurado en el servidor." };
    }
    if (user_id == 0) {
        return .{ .authorized = false, .reason = "Inicia sesión para buscar en tu Google." };
    }

    var authorized = false;
    var reason: []const u8 = "";

    const agenda = gcal.fetch(allocator, db, cfg, user_id, 100) catch gcal.Resultado{
        .authorized = false,
        .reason = "No se pudo consultar Google Calendar.",
    };
    if (agenda.authorized) authorized = true else if (reason.len == 0) reason = agenda.reason;
    for (agenda.events) |e| {
        const sub = if (e.location.len > 0)
            std.fmt.allocPrint(allocator, "{s} · {s}", .{ e.start, e.location }) catch e.start
        else
            e.start;
        maybeAdd(allocator, out, needle, "Google Calendar", e.title, sub, e.link) catch {};
    }

    const drive = gws.driveFiles(allocator, db, cfg, user_id, 100) catch gws.DriveResult{
        .authorized = false,
        .reason = "No se pudo consultar Google Drive.",
    };
    if (drive.authorized) authorized = true else if (reason.len == 0) reason = drive.reason;
    for (drive.files) |f| {
        maybeAdd(allocator, out, needle, "Google Drive", f.name, f.modified, f.link) catch {};
    }

    const people = gws.contacts(allocator, db, cfg, user_id, 200) catch gws.ContactsResult{
        .authorized = false,
        .reason = "No se pudo consultar Google Contacts.",
    };
    if (people.authorized) authorized = true else if (reason.len == 0) reason = people.reason;
    for (people.contacts) |c| {
        const sub = if (c.email.len > 0)
            std.fmt.allocPrint(allocator, "{s} · {s}", .{ c.email, c.phone }) catch c.email
        else
            c.phone;
        maybeAdd(allocator, out, needle, "Contacto", c.name, sub, "") catch {};
    }

    return .{ .authorized = authorized, .reason = reason };
}

/// Ejecuta las tres búsquedas. Las fuentes externas que fallan no rompen la
/// página: se reflejan en `mikareno_error` / `google_reason`.
pub fn search(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    config: *const config_mod.Config,
    user_id: i64,
    query: []const u8,
) !Results {
    const trimmed = std.mem.trim(u8, query, " \t\r\n");
    const needle = try pushLower(allocator, trimmed);
    var r = Results{
        .notes = std.ArrayList(Item).init(allocator),
        .mikareno = std.ArrayList(Item).init(allocator),
        .google = std.ArrayList(Item).init(allocator),
    };
    if (needle.len == 0) return r;
    try searchNotes(allocator, db, needle, &r.notes);
    const mk = searchMikareno(allocator, config, needle, &r.mikareno);
    r.mikareno_connected = mk.connected;
    r.mikareno_error = mk.error_message;
    const gg = searchGoogle(allocator, db, config, user_id, needle, &r.google);
    r.google_authorized = gg.authorized;
    r.google_reason = gg.reason;
    return r;
}

fn section(
    allocator: std.mem.Allocator,
    eyebrow: []const u8,
    title: []const u8,
    items: []const Item,
    empty: []const u8,
) ![]const u8 {
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<section class=\"nx-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">");
    try buf.appendSlice(try security.htmlEscape(allocator, eyebrow));
    try buf.appendSlice("</p><h2>");
    try buf.appendSlice(try security.htmlEscape(allocator, title));
    try buf.appendSlice(" (");
    try buf.writer().print("{d}", .{items.len});
    try buf.appendSlice(")</h2></div></div>");
    if (items.len == 0) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">");
        try buf.appendSlice(try security.htmlEscape(allocator, empty));
        try buf.appendSlice("</p></section>");
        return buf.items;
    }
    try buf.appendSlice("<ul class=\"nx-search-results\">");
    for (items) |item| {
        try buf.appendSlice("<li>");
        const safe_title = try security.htmlEscape(allocator, item.title);
        if (item.url.len > 0 and (std.mem.startsWith(u8, item.url, "/") or std.mem.startsWith(u8, item.url, "https://"))) {
            try buf.appendSlice("<a class=\"nx-text-link\" href=\"");
            try buf.appendSlice(try security.htmlEscape(allocator, item.url));
            try buf.appendSlice("\"");
            if (std.mem.startsWith(u8, item.url, "https://")) try buf.appendSlice(" target=\"_blank\" rel=\"noopener\"");
            try buf.appendSlice(">");
            try buf.appendSlice(safe_title);
            try buf.appendSlice("</a>");
        } else {
            try buf.appendSlice("<strong>");
            try buf.appendSlice(safe_title);
            try buf.appendSlice("</strong>");
        }
        try buf.appendSlice(" <span class=\"nx-search-kind\">");
        try buf.appendSlice(try security.htmlEscape(allocator, item.kind));
        try buf.appendSlice("</span>");
        if (item.subtitle.len > 0) {
            try buf.appendSlice("<p class=\"nx-academic-muted\">");
            try buf.appendSlice(try security.htmlEscape(allocator, item.subtitle));
            try buf.appendSlice("</p>");
        }
        try buf.appendSlice("</li>");
    }
    try buf.appendSlice("</ul></section>");
    return buf.items;
}

/// Cuerpo HTML de `/dashboard/search` (sin el shell).
pub fn page(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    config: *const config_mod.Config,
    user_id: i64,
    query: []const u8,
) ![]const u8 {
    const q = if (query.len > 200) query[0..200] else query;
    const safe_q = try security.htmlEscape(allocator, q);
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<div class=\"nx-page nova-page\">");
    try buf.appendSlice("<header class=\"nx-page-header nova-page-head\"><div><p class=\"nx-eyebrow\">Buscar</p><h1>Búsqueda global</h1><p>Apuntes, tareas, notas, horario, agenda de Google, Drive y contactos en un solo lugar.</p></div></header>");
    try buf.appendSlice("<form method=\"get\" action=\"/dashboard/search\" class=\"nx-search-form\"><span class=\"material-symbols-outlined\" aria-hidden=\"true\">search</span><input type=\"search\" name=\"q\" value=\"");
    try buf.appendSlice(safe_q);
    try buf.appendSlice("\" placeholder=\"Buscar en toda la plataforma…\" aria-label=\"Buscar en toda la plataforma\"><button type=\"submit\" class=\"nx-button\">Buscar</button></form>");

    const trimmed = std.mem.trim(u8, q, " \t\r\n");
    if (trimmed.len == 0) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">Escribe una palabra para buscar en tus apuntes, en Mikareno y en tu cuenta de Google.</p></div>");
        return buf.items;
    }

    const r = try search(allocator, db, config, user_id, trimmed);
    try buf.appendSlice(try section(allocator, "Apuntes", "Notas locales", r.notes.items, "No hay notas que coincidan."));
    const mk_empty = if (!r.mikareno_connected)
        (if (r.mikareno_error.len > 0) r.mikareno_error else "Mikareno no está conectado.")
    else
        "No hay tareas, eventos, materias ni clases que coincidan.";
    try buf.appendSlice(try section(allocator, "Mikareno", "Datos académicos", r.mikareno.items, mk_empty));
    const gg_empty = if (!r.google_authorized)
        (if (r.google_reason.len > 0) r.google_reason else "No has autorizado el acceso a Google.")
    else
        "No hay eventos, archivos ni contactos que coincidan.";
    try buf.appendSlice(try section(allocator, "Google", "Tu cuenta de Google", r.google.items, gg_empty));
    try buf.appendSlice("</div>");
    return buf.items;
}
