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
const security = @import("security.zig");
const importer = @import("importer.zig");

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

pub const DocText = struct {
    authorized: bool = false,
    found: bool = false,
    name: []const u8 = "",
    mime: []const u8 = "",
    text: []const u8 = "",
    reason: []const u8 = "",
};

/// `true` para Docs, Sheets y Slides (no para archivos subidos).
pub fn isGoogleNative(mime: []const u8) bool {
    return std.mem.startsWith(u8, mime, "application/vnd.google-apps.");
}

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
        "{s}?pageSize={d}&orderBy=modifiedTime%20desc&fields=files(id,name,mimeType,modifiedTime,webViewLink)",
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

fn parseDrivePayload(allocator: std.mem.Allocator, payload: std.json.Value) !DriveResult {
    const files = json.get(payload, "files") orelse return .{ .authorized = true };
    if (files != .array) return .{ .authorized = true };
    var out = std.ArrayList(DriveFile).init(allocator);
    for (files.array.items) |f| {
        if (f != .object) continue;
        out.append(.{
            .id = try allocator.dupe(u8, json.getString(f, "id") orelse ""),
            .name = try allocator.dupe(u8, json.getString(f, "name") orelse "(sin nombre)"),
            .mime = try allocator.dupe(u8, json.getString(f, "mimeType") orelse ""),
            .modified = try allocator.dupe(u8, json.getString(f, "modifiedTime") orelse ""),
            .link = try allocator.dupe(u8, json.getString(f, "webViewLink") orelse ""),
        }) catch {};
    }
    return .{ .authorized = true, .files = out.items };
}

/// Busca archivos en Drive por nombre con la API (`name contains '...'`).
pub fn driveSearch(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
    query: []const u8,
    max_results: usize,
) !DriveResult {
    const trimmed = std.mem.trim(u8, query, " \t\r\n");
    if (trimmed.len == 0) return driveFiles(allocator, db, cfg, user_id, max_results);
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) {
        return .{ .authorized = false, .reason = "Google OAuth no está configurado en el servidor." };
    }
    const token = (try accessTokenFor(allocator, db, cfg, user_id)) orelse {
        return .{ .authorized = false, .reason = "No has autorizado el acceso a Google Drive. Vuelve a entrar con Google y acepta el permiso de Drive." };
    };
    const backslashes = try std.mem.replaceOwned(u8, allocator, trimmed, "\\", "\\\\");
    const quotes = try std.mem.replaceOwned(u8, allocator, backslashes, "'", "\\'");
    const clause = try std.fmt.allocPrint(allocator, "name contains '{s}'", .{quotes});
    const url = try std.fmt.allocPrint(
        allocator,
        "{s}?pageSize={d}&orderBy=modifiedTime%20desc&q={s}&fields=files(id,name,mimeType,modifiedTime,webViewLink)",
        .{ DRIVE_URL, max_results, try oauth.urlEncode(allocator, clause) },
    );
    const payload = oauth.getJsonPublic(allocator, url, token, cfg.timeout_ms) catch {
        return .{ .authorized = false, .reason = "Google Drive no respondió o rechazó la búsqueda." };
    };
    return parseDrivePayload(allocator, payload);
}

/// Lee el contenido de un Docs/Sheets/Slides exportándolo a texto.
pub fn readDocument(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
    file_id: []const u8,
) !DocText {
    // El id de Drive es seguro por construcción: letras, dígitos, - y _.
    if (file_id.len == 0 or file_id.len > 200) return .{ .reason = "Identificador de archivo no válido." };
    for (file_id) |c| {
        const ok = (c >= 'a' and c <= 'z') or (c >= 'A' and c <= 'Z') or (c >= '0' and c <= '9') or c == '-' or c == '_';
        if (!ok) return .{ .reason = "Identificador de archivo no válido." };
    }
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) {
        return .{ .authorized = false, .reason = "Google OAuth no está configurado en el servidor." };
    }
    const token = (try accessTokenFor(allocator, db, cfg, user_id)) orelse {
        return .{ .authorized = false, .reason = "No has autorizado el acceso a Google Drive. Vuelve a entrar con Google y acepta el permiso de Drive." };
    };
    const meta_url = try std.fmt.allocPrint(allocator, "{s}/{s}?fields=name,mimeType", .{ DRIVE_URL, file_id });
    const meta = oauth.getJsonPublic(allocator, meta_url, token, cfg.timeout_ms) catch {
        return .{ .authorized = true, .reason = "No se encontró el archivo o Google rechazó el acceso." };
    };
    const name = json.getString(meta, "name") orelse "(sin nombre)";
    const mime = json.getString(meta, "mimeType") orelse "";
    if (!isGoogleNative(mime)) {
        return .{ .authorized = true, .found = true, .name = try allocator.dupe(u8, name), .mime = try allocator.dupe(u8, mime), .reason = "Este archivo no es un documento de Google (Docs, Sheets o Slides)." };
    }
    const export_mime = if (std.mem.eql(u8, mime, "application/vnd.google-apps.spreadsheet")) "text/csv" else "text/plain";
    const export_url = try std.fmt.allocPrint(
        allocator,
        "https://www.googleapis.com/drive/v3/files/{s}/export?mimeType={s}",
        .{ file_id, try oauth.urlEncode(allocator, export_mime) },
    );
    const body = oauth.getTextPublic(allocator, export_url, token, cfg.timeout_ms, 4 * 1024 * 1024) catch {
        return .{ .authorized = true, .found = true, .name = try allocator.dupe(u8, name), .mime = try allocator.dupe(u8, mime), .reason = "Google no permitió exportar este documento a texto." };
    };
    return .{
        .authorized = true,
        .found = true,
        .name = try allocator.dupe(u8, name),
        .mime = try allocator.dupe(u8, mime),
        .text = try allocator.dupe(u8, body),
    };
}

fn validFileId(file_id: []const u8) bool {
    if (file_id.len == 0 or file_id.len > 200) return false;
    for (file_id) |c| {
        const ok = (c >= 'a' and c <= 'z') or (c >= 'A' and c <= 'Z') or (c >= '0' and c <= '9') or c == '-' or c == '_';
        if (!ok) return false;
    }
    return true;
}

/// Exporta un archivo nativo de Google (Docs/Sheets/Slides) al MIME pedido.
pub fn exportFile(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
    file_id: []const u8,
    export_mime: []const u8,
    max_bytes: usize,
) !?[]const u8 {
    if (!validFileId(file_id)) return null;
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) return null;
    const token = (try accessTokenFor(allocator, db, cfg, user_id)) orelse return null;
    const url = try std.fmt.allocPrint(
        allocator,
        "https://www.googleapis.com/drive/v3/files/{s}/export?mimeType={s}",
        .{ file_id, try oauth.urlEncode(allocator, export_mime) },
    );
    return oauth.getTextPublic(allocator, url, token, cfg.timeout_ms, max_bytes) catch null;
}

/// Descarga el contenido binario de un archivo de Drive no nativo.
pub fn downloadFile(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
    file_id: []const u8,
    max_bytes: usize,
) !?[]const u8 {
    if (!validFileId(file_id)) return null;
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) return null;
    const token = (try accessTokenFor(allocator, db, cfg, user_id)) orelse return null;
    const url = try std.fmt.allocPrint(
        allocator,
        "{s}/{s}?alt=media&supportsAllDrives=true",
        .{ DRIVE_URL, file_id },
    );
    return oauth.getTextPublic(allocator, url, token, cfg.timeout_ms, max_bytes) catch null;
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
    try buf.appendSlice("<section class=\"nx-panel nx-gcal-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Google Drive</p><h2>Archivos de Google Drive</h2></div></div>");
    if (!r.authorized) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">");
        try buf.appendSlice(try security.htmlEscape(allocator, r.reason));
        try buf.appendSlice("</p></section>");
        return buf.items;
    }
    if (r.files.len == 0) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">No hay archivos en Google Drive.</p></section>");
        return buf.items;
    }
    try buf.appendSlice("<div class=\"nx-table-wrap\"><table class=\"nx-table\"><thead><tr><th>Nombre</th><th>Tipo</th><th>Modificado</th><th>Acción</th></tr></thead><tbody>");
    for (r.files) |f| {
        const safe_name = try security.htmlEscape(allocator, f.name);
        try buf.appendSlice("<tr><td>");
        if (isGoogleNative(f.mime) and f.id.len > 0) {
            try buf.appendSlice("<a class=\"nx-text-link\" href=\"/dashboard/google?doc=");
            try buf.appendSlice(try security.htmlEscape(allocator, f.id));
            try buf.appendSlice("\">");
            try buf.appendSlice(safe_name);
            try buf.appendSlice("</a>");
        } else if (f.link.len > 0 and (std.mem.startsWith(u8, f.link, "https://") or std.mem.startsWith(u8, f.link, "http://"))) {
            try buf.appendSlice("<a class=\"nx-text-link\" href=\"");
            try buf.appendSlice(try security.htmlEscape(allocator, f.link));
            try buf.appendSlice("\" target=\"_blank\" rel=\"noopener\">");
            try buf.appendSlice(safe_name);
            try buf.appendSlice("</a>");
        } else {
            try buf.appendSlice(safe_name);
        }
        try buf.appendSlice("</td><td>");
        try buf.appendSlice(try security.htmlEscape(allocator, docKindLabel(f.mime)));
        try buf.appendSlice("</td><td>");
        try buf.appendSlice(try security.htmlEscape(allocator, if (f.modified.len == 0) "—" else f.modified));
        try buf.appendSlice("</td><td>");
        const native_convertible = std.mem.eql(u8, f.mime, "application/vnd.google-apps.document") or
            std.mem.eql(u8, f.mime, "application/vnd.google-apps.spreadsheet") or
            std.mem.eql(u8, f.mime, "application/vnd.google-apps.presentation");
        const importable = f.id.len > 0 and if (isGoogleNative(f.mime))
            native_convertible
        else
            importer.kindFor(f.name) != null;
        if (importable) {
            try buf.appendSlice("<form method=\"post\" action=\"/office/library/import-drive\"><input type=\"hidden\" name=\"file\" value=\"");
            try buf.appendSlice(try security.htmlEscape(allocator, f.id));
            try buf.appendSlice("\"><input type=\"hidden\" name=\"name\" value=\"");
            try buf.appendSlice(safe_name);
            try buf.appendSlice("\"><input type=\"hidden\" name=\"mime\" value=\"");
            try buf.appendSlice(try security.htmlEscape(allocator, f.mime));
            try buf.appendSlice("\"><button type=\"submit\" class=\"nx-button nx-button-secondary\">Importar</button></form>");
        } else {
            try buf.appendSlice("—");
        }
        try buf.appendSlice("</td></tr>");
    }
    try buf.appendSlice("</tbody></table></div></section>");
    return buf.items;
}

fn docKindLabel(mime: []const u8) []const u8 {
    if (std.mem.eql(u8, mime, "application/vnd.google-apps.document")) return "Google Docs";
    if (std.mem.eql(u8, mime, "application/vnd.google-apps.spreadsheet")) return "Google Sheets";
    if (std.mem.eql(u8, mime, "application/vnd.google-apps.presentation")) return "Google Slides";
    if (std.mem.eql(u8, mime, "application/vnd.google-apps.folder")) return "Carpeta";
    if (isGoogleNative(mime)) return "Google";
    if (mime.len == 0) return "—";
    return mime;
}

/// Sección HTML con el contenido exportado de un Docs/Sheets/Slides.
pub fn docSectionHtml(allocator: std.mem.Allocator, d: DocText) ![]const u8 {
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<section class=\"nx-panel nx-gcal-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">");
    try buf.appendSlice(try security.htmlEscape(allocator, if (d.mime.len > 0) docKindLabel(d.mime) else "Google Drive"));
    try buf.appendSlice("</p><h2>");
    try buf.appendSlice(try security.htmlEscape(allocator, if (d.name.len > 0) d.name else "Documento"));
    try buf.appendSlice("</h2></div><a class=\"nx-button nx-button-secondary\" href=\"/dashboard/google\">Volver a Drive</a></div>");
    if (!d.authorized or d.reason.len > 0 and d.text.len == 0) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">");
        try buf.appendSlice(try security.htmlEscape(allocator, if (d.reason.len > 0) d.reason else "No se pudo leer el documento."));
        try buf.appendSlice("</p></section>");
        return buf.items;
    }
    if (d.text.len == 0) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">El documento está vacío.</p></section>");
        return buf.items;
    }
    try buf.appendSlice("<pre class=\"nx-doc-preview\">");
    try buf.appendSlice(try security.htmlEscape(allocator, d.text));
    try buf.appendSlice("</pre></section>");
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
