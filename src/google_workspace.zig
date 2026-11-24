//! Google Workspace: Drive (archivos) y Contacts (contactos).
//!
//! Solo lectura. Reutiliza el refresh_token guardado en `google_oauth_tokens`
//! (mismo flujo que Calendar) y, si no hay token o Google rechaza el acceso,
//! devuelve un estado vacío honesto con el motivo. Nunca inventa contactos ni
//! archivos.

const std = @import("std");
const sqlite = @import("sqlite.zig");
const json = @import("jsonutil.zig");
const oauth = @import("oauth_google.zig");

pub const DriveFile = struct {
    id: []const u8,
    name: []const u8,
    mime: []const u8,
    modified: []const u8,
    link: []const u8,
};

pub const Contact = struct {
    name: []const u8,
    email: []const u8,
    phone: []const u8,
};

pub const DriveResult = struct {
    authorized: bool = false,
    files: []DriveFile = &.{},
    reason: []const u8 = "",
};

pub const ContactsResult = struct {
    authorized: bool = false,
    contacts: []Contact = &.{},
    reason: []const u8 = "",
};

const DRIVE_URL = "https://www.googleapis.com/drive/v3/files";
const PEOPLE_URL = "https://people.googleapis.com/v1/people/me/connections";

/// access_token del usuario: reutiliza o renueva el refresh guardado.
fn accessTokenFor(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
) !?[]const u8 {
    var stmt = db.prepare("SELECT refresh_token FROM google_oauth_tokens WHERE user_id=?1") catch return null;
    defer stmt.deinit();
    var uid_buf: [32]u8 = undefined;
    const uid = try std.fmt.bufPrint(&uid_buf, "{d}", .{user_id});
    try stmt.bindText(1, uid);
    if (!try stmt.next()) return null;
    const refresh = try allocator.dupe(u8, stmt.text(0));
    if (refresh.len == 0) return null;
    return oauth.refreshAccessToken(allocator, cfg, refresh) catch null;
}

/// Lista archivos de Google Drive (los más recientes primero).
pub fn driveFiles(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
    max_results: usize,
) !DriveResult {
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) {
        return .{ .authorized = false, .reason = "Google OAuth no está configurado en el servidor." };
    }
    const token = (try accessTokenFor(allocator, db, cfg, user_id)) orelse {
        return .{ .authorized = false, .reason = "No has autorizado el acceso a Google Drive. Vuelve a entrar con Google y acepta el permiso de Drive." };
    };
    const url = try std.fmt.allocPrint(
        allocator,
        "{s}?pageSize={d}&orderBy=modifiedTime desc&fields=files(id,name,mimeType,modifiedTime,webViewLink)",
        .{ DRIVE_URL, max_results },
    );
    const payload = oauth.getJsonPublic(allocator, url, token, cfg.timeout_ms) catch {
        return .{ .authorized = false, .reason = "Google Drive no respondió o rechazó el acceso." };
    };
    const files = json.get(payload, "files") orelse return .{ .authorized = true };
    if (files != .array) return .{ .authorized = true };
    var out = std.ArrayList(DriveFile).init(allocator);
    for (files.array.items) |f| {
        if (f != .object) continue;
        out.append(.{
            .id = json.getString(f, "id") orelse "",
            .name = json.getString(f, "name") orelse "(sin nombre)",
            .mime = json.getString(f, "mimeType") orelse "",
            .modified = json.getString(f, "modifiedTime") orelse "",
            .link = json.getString(f, "webViewLink") orelse "",
        }) catch {};
    }
    return .{ .authorized = true, .files = out.items };
}

/// Lista contactos de Google Contacts (People API).
pub fn contacts(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
    max_results: usize,
) !ContactsResult {
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) {
        return .{ .authorized = false, .reason = "Google OAuth no está configurado en el servidor." };
    }
    const token = (try accessTokenFor(allocator, db, cfg, user_id)) orelse {
        return .{ .authorized = false, .reason = "No has autorizado el acceso a tus contactos de Google. Vuelve a entrar con Google y acepta el permiso de contactos." };
    };
    const url = try std.fmt.allocPrint(
        allocator,
        "{s}?personFields=names,emailAddresses,phoneNumbers&pageSize={d}",
        .{ PEOPLE_URL, max_results },
    );
    const payload = oauth.getJsonPublic(allocator, url, token, cfg.timeout_ms) catch {
        return .{ .authorized = false, .reason = "Google Contacts no respondió o rechazó el acceso." };
    };
    const items = json.get(payload, "connections") orelse return .{ .authorized = true };
    if (items != .array) return .{ .authorized = true };
    var out = std.ArrayList(Contact).init(allocator);
    for (items.array.items) |p| {
        if (p != .object) continue;
        const names = json.get(p, "names");
        var name: []const u8 = "";
        if (names) |arr| {
            if (arr == .array and arr.array.items.len > 0) {
                if (arr.array.items[0] == .object) {
                    name = json.getString(arr.array.items[0], "displayName") orelse "";
                }
            }
        }
        const emails = json.get(p, "emailAddresses");
        var email: []const u8 = "";
        if (emails) |arr| {
            if (arr == .array and arr.array.items.len > 0) {
                if (arr.array.items[0] == .object) {
                    email = json.getString(arr.array.items[0], "value") orelse "";
                }
            }
        }
        const phones = json.get(p, "phoneNumbers");
        var phone: []const u8 = "";
        if (phones) |arr| {
            if (arr == .array and arr.array.items.len > 0) {
                if (arr.array.items[0] == .object) {
                    phone = json.getString(arr.array.items[0], "value") orelse "";
                }
            }
        }
        if (name.len == 0 and email.len == 0 and phone.len == 0) continue;
        out.append(.{ .name = name, .email = email, .phone = phone }) catch {};
    }
    return .{ .authorized = true, .contacts = out.items };
}

/// Sección HTML de Drive para la página de Google.
pub fn driveSectionHtml(allocator: std.mem.Allocator, r: DriveResult) ![]const u8 {
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<section class=\"nx-panel nx-gcal-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Google Drive</p><h2>Tus archivos recientes</h2></div></div>");
    if (!r.authorized) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">");
        try buf.appendSlice(try std.mem.replaceOwned(u8, allocator, r.reason, "\n", " "));
        try buf.appendSlice("</p></section>");
        return buf.items;
    }
    if (r.files.len == 0) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">No hay archivos en Google Drive.</p></section>");
        return buf.items;
    }
    try buf.appendSlice("<div class=\"nx-table-wrap\"><table class=\"nx-table\"><thead><tr><th>Nombre</th><th>Tipo</th><th>Modificado</th></tr></thead><tbody>");
    for (r.files) |f| {
        try buf.appendSlice("<tr><td>");
        if (f.link.len > 0 and (std.mem.startsWith(u8, f.link, "https://") or std.mem.startsWith(u8, f.link, "http://"))) {
            try buf.appendSlice("<a class=\"nx-text-link\" href=\"");
            try buf.appendSlice(f.link);
            try buf.appendSlice("\" target=\"_blank\" rel=\"noopener\">");
            try buf.appendSlice(try std.mem.replaceOwned(u8, allocator, f.name, "&", "&amp;"));
            try buf.appendSlice("</a>");
        } else {
            try buf.appendSlice(try std.mem.replaceOwned(u8, allocator, f.name, "&", "&amp;"));
        }
        try buf.appendSlice("</td><td>");
        try buf.appendSlice(if (f.mime.len == 0) "—" else f.mime);
        try buf.appendSlice("</td><td>");
        try buf.appendSlice(if (f.modified.len == 0) "—" else f.modified);
        try buf.appendSlice("</td></tr>");
    }
    try buf.appendSlice("</tbody></table></div></section>");
    return buf.items;
}

/// Sección HTML de Contacts.
pub fn contactsSectionHtml(allocator: std.mem.Allocator, r: ContactsResult) ![]const u8 {
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<section class=\"nx-panel nx-gcal-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Google Contacts</p><h2>Tus contactos</h2></div></div>");
    if (!r.authorized) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">");
        try buf.appendSlice(try std.mem.replaceOwned(u8, allocator, r.reason, "\n", " "));
        try buf.appendSlice("</p></section>");
        return buf.items;
    }
    if (r.contacts.len == 0) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">No hay contactos para mostrar.</p></section>");
        return buf.items;
    }
    try buf.appendSlice("<div class=\"nx-table-wrap\"><table class=\"nx-table\"><thead><tr><th>Nombre</th><th>Correo</th><th>Teléfono</th></tr></thead><tbody>");
    for (r.contacts) |c| {
        try buf.appendSlice("<tr><td>");
        try buf.appendSlice(if (c.name.len == 0) "—" else c.name);
        try buf.appendSlice("</td><td>");
        try buf.appendSlice(if (c.email.len == 0) "—" else c.email);
        try buf.appendSlice("</td><td>");
        try buf.appendSlice(if (c.phone.len == 0) "—" else c.phone);
        try buf.appendSlice("</td></tr>");
    }
    try buf.appendSlice("</tbody></table></div></section>");
    return buf.items;
}
