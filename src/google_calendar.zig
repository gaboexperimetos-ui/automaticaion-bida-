//! Google Calendar: lee los eventos del usuario que entra con Google y los
//! muestra junto a los de Mikareno en la vista de agenda.
//!
//! Reglas:
//!
//! * **Solo lectura**: se pide `calendar.readonly`. Nunca se escribe en el
//!   calendario del usuario.
//! * **Reutiliza el refresh_token** guardado en `google_oauth_tokens` para
//!   renovar el access_token cuando caduca.
//! * **Si no hay token**: devuelve estado vacío honesto ("no autorizado"),
//!   no inventa eventos ni rompe la página.

const std = @import("std");
const sqlite = @import("sqlite.zig");
const json = @import("jsonutil.zig");
const oauth = @import("oauth_google.zig");

pub const Evento = struct {
    title: []const u8,
    start: []const u8,
    end: []const u8,
    all_day: bool,
    location: []const u8,
    link: []const u8,
};

pub const Resultado = struct {
    authorized: bool = false,
    /// El servidor tiene client_id y client_secret: se puede ofrecer el botón
    /// de conectar aunque todavía no haya autorización.
    configured: bool = false,
    events: []Evento = &.{},
    reason: []const u8 = "",
};

/// access_token del usuario, renovándolo si hace falta.
fn accessTokenFor(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
) !?struct { token: []const u8, refresh: []const u8 } {
    var stmt = db.prepare("SELECT refresh_token FROM google_oauth_tokens WHERE user_id=?1") catch return null;
    defer stmt.deinit();
    var uid_buf: [32]u8 = undefined;
    const uid = try std.fmt.bufPrint(&uid_buf, "{d}", .{user_id});
    try stmt.bindText(1, uid);
    if (!try stmt.next()) return null;
    const refresh = try allocator.dupe(u8, stmt.text(0));
    if (refresh.len == 0) return null;
    const token = oauth.refreshAccessToken(allocator, cfg, refresh) catch return null;
    return .{ .token = token, .refresh = refresh };
}

/// Lee los próximos eventos de Google Calendar del usuario.
pub fn fetch(allocator: std.mem.Allocator, db: *sqlite.Db, cfg: oauth.Config, user_id: i64, max_results: usize) !Resultado {
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) {
        return .{ .authorized = false, .reason = "Google OAuth no está configurado en el servidor." };
    }
    const pair = (try accessTokenFor(allocator, db, cfg, user_id)) orelse {
        return .{
            .authorized = false,
            .configured = true,
            .reason = "No has autorizado el acceso a Google Calendar. Conéctalo para ver tus eventos aquí.",
        };
    };

    const payload = oauth.calendarEvents(allocator, cfg, pair.token, max_results) catch {
        return .{ .authorized = false, .configured = true, .reason = "Google Calendar no respondió o rechazó el acceso." };
    };

    const items = json.get(payload, "items") orelse return .{ .authorized = true, .configured = true };
    if (items != .array) return .{ .authorized = true, .configured = true };

    var out = std.ArrayList(Evento).init(allocator);
    for (items.array.items) |ev| {
        if (ev != .object) continue;
        const summary = json.getString(ev, "summary") orelse "(sin título)";
        const start_obj = json.get(ev, "start") orelse .null;
        const end_obj = json.get(ev, "end") orelse .null;
        // Eventos de todo el día: {"date":"2026-11-24"}; con hora: {"dateTime":...}
        const all_day = json.get(start_obj, "date") != null;
        const start = if (json.get(start_obj, "date")) |d|
            (if (d == .string) d.string else "")
        else if (json.get(start_obj, "dateTime")) |d|
            (if (d == .string) d.string else "")
        else
            "";
        const end = if (json.get(end_obj, "date")) |d|
            (if (d == .string) d.string else "")
        else if (json.get(end_obj, "dateTime")) |d|
            (if (d == .string) d.string else "")
        else
            "";
        const location = json.getString(ev, "location") orelse "";
        const link = json.getString(ev, "htmlLink") orelse "";
        out.append(.{
            .title = summary,
            .start = start,
            .end = end,
            .all_day = all_day,
            .location = location,
            .link = link,
        }) catch {};
    }
    return .{ .authorized = true, .configured = true, .events = out.items };
}

/// HTML de la sección de Google Calendar para la página de agenda.
pub fn sectionHtml(allocator: std.mem.Allocator, r: Resultado) ![]const u8 {
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<section class=\"nx-panel nx-gcal-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Google Calendar</p><h2>Tu calendario de Google</h2></div>");
    if (!r.authorized) {
        try buf.appendSlice("</div><p class=\"nx-academic-muted\">");
        try buf.appendSlice(try std.mem.replaceOwned(u8, allocator, r.reason, "\n", " "));
        try buf.appendSlice("</p></section>");
        return buf.items;
    }
    if (r.events.len == 0) {
        try buf.appendSlice("</div><p class=\"nx-academic-muted\">No tienes eventos próximos en Google Calendar.</p></section>");
        return buf.items;
    }
    try buf.appendSlice("</div><div class=\"nx-table-wrap\"><table class=\"nx-table\"><thead><tr><th>Fecha</th><th>Evento</th><th>Lugar</th></tr></thead><tbody>");
    for (r.events) |ev| {
        try buf.appendSlice("<tr><td>");
        try buf.appendSlice(try std.mem.replaceOwned(u8, allocator, ev.start, "T", " "));
        try buf.appendSlice("</td><td>");
        if (ev.link.len > 0 and (std.mem.startsWith(u8, ev.link, "https://") or std.mem.startsWith(u8, ev.link, "http://"))) {
            try buf.appendSlice("<a class=\"nx-text-link\" href=\"");
            try buf.appendSlice(ev.link);
            try buf.appendSlice("\" target=\"_blank\" rel=\"noopener\">");
            try buf.appendSlice(ev.title);
            try buf.appendSlice("</a>");
        } else {
            try buf.appendSlice(ev.title);
        }
        try buf.appendSlice("</td><td>");
        try buf.appendSlice(if (ev.location.len == 0) "—" else ev.location);
        try buf.appendSlice("</td></tr>");
    }
    try buf.appendSlice("</tbody></table></div></section>");
    return buf.items;
}

test "sin client_id no se ofrece conectar Google" {
    var db = try sqlite.Db.open(std.testing.allocator, ":memory:");
    defer db.close();
    const cfg = oauth.Config{ .client_id = "", .client_secret = "", .redirect_uri = "", .timeout_ms = 100 };
    const r = try fetch(std.testing.allocator, &db, cfg, 1, 10);
    try std.testing.expect(!r.configured);
    try std.testing.expect(!r.authorized);
}

test "configurado sin token pide conectar en vez de fallar" {
    var db = try sqlite.Db.open(std.testing.allocator, ":memory:");
    defer db.close();
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const cfg = oauth.Config{ .client_id = "id", .client_secret = "secret", .redirect_uri = "http://127.0.0.1:3100/auth/google/callback", .timeout_ms = 100 };
    const r = try fetch(arena.allocator(), &db, cfg, 1, 10);
    try std.testing.expect(r.configured);
    try std.testing.expect(!r.authorized);
}
