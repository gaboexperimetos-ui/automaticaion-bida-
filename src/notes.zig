const std = @import("std");
const http = @import("http.zig");
const sqlite = @import("sqlite.zig");
const config_mod = @import("config.zig");
const json = @import("jsonutil.zig");
const security = @import("security.zig");
const templates = @import("templates.zig");

const Note = struct {
    id: i64,
    title: []const u8,
    slug: []const u8,
    filename: []const u8,
    tags: []const u8,
    word_count: i64,
    summary: []const u8,
    created_at: []const u8,
    updated_at: []const u8,
};

pub fn handles(path: []const u8) bool {
    return std.mem.eql(u8, path, "/api/notes") or std.mem.startsWith(u8, path, "/api/notes/") or std.mem.eql(u8, path, "/api/search") or std.mem.eql(u8, path, "/dashboard/search") or std.mem.startsWith(u8, path, "/api/calendar/annotations") or std.mem.eql(u8, path, "/app/notes") or std.mem.startsWith(u8, path, "/app/notes/");
}

pub fn handle(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request, username: []const u8) !http.Response {
    const path = request.path;
    if (std.mem.eql(u8, path, "/api/notes")) return apiNotes(allocator, db, config, request);
    if (std.mem.eql(u8, path, "/api/search")) return searchApi(allocator, db, request);
    if (std.mem.eql(u8, path, "/dashboard/search")) return searchPage(allocator, db, config, request, username);
    if (std.mem.eql(u8, path, "/api/calendar/annotations")) return annotations(allocator, db, request, null);
    if (std.mem.startsWith(u8, path, "/api/calendar/annotations/")) {
        const id = path["/api/calendar/annotations/".len..];
        return annotations(allocator, db, request, id);
    }
    if (std.mem.eql(u8, path, "/app/notes")) return notesPage(allocator, db, config, request, username);
    if (std.mem.startsWith(u8, path, "/api/notes/")) return apiNoteDispatch(allocator, db, config, request);
    if (std.mem.eql(u8, path, "/app/notes/new")) {
        if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
        const body = try templates.simplePage(allocator, "Crear una nota", "<form method=\"post\" action=\"/app/notes\" class=\"grid gap-4 max-w-3xl\"><label>Título<input name=\"title\" maxlength=\"200\" required></label><label>Etiquetas<input name=\"tags\" maxlength=\"500\"></label><label>Contenido Markdown<textarea name=\"content\" rows=\"16\"></textarea></label><button class=\"px-4 py-2 rounded bg-primary-container text-on-primary-container\">Guardar nota</button></form>");
        return html(allocator, config, "/app/notes", username, body, 200);
    }
    if (std.mem.startsWith(u8, path, "/app/notes/")) return noteHtmlRoute(allocator, db, config, request, username);
    return jsonError(allocator, 404, "not_found");
}

fn apiNotes(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request) !http.Response {
    if (std.mem.eql(u8, request.method, "GET")) {
        var stmt = try db.prepare("SELECT id,title,slug,filename,tags,word_count,COALESCE(summary,''),created_at,updated_at FROM notes ORDER BY updated_at DESC LIMIT 100");
        defer stmt.deinit();
        var rows = std.json.Array.init(allocator);
        while (try stmt.next()) {
            const note = try readNote(allocator, &stmt);
            try rows.append(try noteValue(allocator, note));
        }
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("notes", .{ .array = rows });
        try obj.put("count", .{ .integer = @intCast(rows.items.len) });
        return jsonResponse(allocator, 200, .{ .object = obj });
    }
    if (std.mem.eql(u8, request.method, "POST")) {
        const parsed = parseBody(allocator, request) catch return jsonError(allocator, 400, "Invalid JSON");
        const title = json.getString(parsed, "title") orelse return jsonError(allocator, 400, "Title is required");
        const content = json.getString(parsed, "content") orelse "";
        const tags = json.getString(parsed, "tags") orelse "";
        const new_note = createNote(allocator, db, config, title, content, tags, false) catch |err| switch (err) {
            error.InvalidTitle => return jsonError(allocator, 400, "Invalid title"),
            error.TitleTooLong => return jsonError(allocator, 400, "Title too long"),
            error.ContentTooLarge => return jsonError(allocator, 400, "Content too large"),
            error.TagsTooLong => return jsonError(allocator, 400, "Tags too long"),
            error.NoteExists => return jsonError(allocator, 500, "Internal error"),
            else => return jsonError(allocator, 500, "Internal error"),
        };
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("slug", .{ .string = new_note.slug });
        return jsonResponse(allocator, 201, .{ .object = obj });
    }
    return methodNotAllowed(allocator);
}

fn notesPage(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request, username: []const u8) !http.Response {
    if (std.mem.eql(u8, request.method, "POST")) {
        var form = try http.parseForm(allocator, request.body);
        const title = form.get("title") orelse "";
        const content = form.get("content") orelse "";
        const tags = form.get("tags") orelse "";
        const note = createNote(allocator, db, config, title, content, tags, true) catch |err| switch (err) {
            error.InvalidTitle, error.TitleTooLong, error.ContentTooLarge, error.TagsTooLong => return jsonError(allocator, 400, "Invalid note fields"),
            error.NoteExists => return html(allocator, config, "/app/notes", username, "<p>Ya existe una nota con ese título.</p>", 409),
            else => return jsonError(allocator, 500, "Internal error"),
        };
        return redirect(allocator, try std.fmt.allocPrint(allocator, "/app/notes/{s}", .{note.slug}), 303);
    }
    if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
    var stmt = try db.prepare("SELECT id,title,slug,filename,tags,word_count,COALESCE(summary,''),created_at,updated_at FROM notes ORDER BY updated_at DESC LIMIT 100");
    defer stmt.deinit();
    var cards: std.ArrayListUnmanaged(u8) = .empty;
    while (try stmt.next()) {
        const note = try readNote(allocator, &stmt);
        const title = try security.htmlEscape(allocator, note.title);
        const slug = try security.htmlEscape(allocator, note.slug);
        const tags = try security.htmlEscape(allocator, note.tags);
        const card = try std.fmt.allocPrint(allocator, "<a class=\"block rounded-xl border border-outline-variant/40 p-4 hover:bg-surface-container-high\" href=\"/app/notes/{s}\"><strong>{s}</strong><p class=\"text-sm text-on-surface-variant\">{s}</p><small>{d} palabras</small></a>", .{ slug, title, tags, note.word_count });
        try cards.appendSlice(allocator, card);
    }
    const body = try templates.simplePage(allocator, "Mis notas", try std.fmt.allocPrint(allocator, "<p class=\"mb-4\">Segundo cerebro local · Markdown</p><a class=\"inline-block px-4 py-2 rounded bg-primary-container text-on-primary-container mb-4\" href=\"/app/notes/new\">Nueva nota</a><div class=\"grid gap-3\">{s}</div>", .{if (cards.items.len == 0) "<p>Todavía no hay notas.</p>" else cards.items}));
    return html(allocator, config, "/app/notes", username, body, 200);
}

fn noteHtmlRoute(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request, username: []const u8) !http.Response {
    const rest = request.path["/app/notes/".len..];
    const slash = std.mem.indexOfScalar(u8, rest, '/') orelse rest.len;
    const slug = rest[0..slash];
    if (!safeSlug(slug)) return html(allocator, config, "/app/notes", username, "<h2>Nota no encontrada</h2><a href=\"/app/notes\">Volver a las notas</a>", 404);
    const suffix = if (slash < rest.len) rest[slash..] else "";
    if (std.mem.eql(u8, suffix, "/delete") and std.mem.eql(u8, request.method, "DELETE")) return deleteNote(allocator, db, config, slug, false);
    if (std.mem.eql(u8, suffix, "/update") and std.mem.eql(u8, request.method, "PUT")) return updateNote(allocator, db, config, request, slug, false);
    if (std.mem.eql(u8, suffix, "/export") and std.mem.eql(u8, request.method, "GET")) return exportNote(allocator, db, config, slug);
    if (std.mem.eql(u8, suffix, "/versions") and std.mem.eql(u8, request.method, "GET")) return noteVersions(allocator, db, slug);
    if (std.mem.startsWith(u8, suffix, "/versions/") and std.mem.endsWith(u8, suffix, "/restore")) {
        const raw_id = suffix["/versions/".len .. suffix.len - "/restore".len];
        const version_id = std.fmt.parseInt(i64, raw_id, 10) catch return jsonError(allocator, 400, "Invalid version id");
        return restoreVersion(allocator, db, config, request, slug, version_id);
    }
    if (std.mem.eql(u8, suffix, "/edit") and std.mem.eql(u8, request.method, "GET")) {
        const note = getNote(allocator, db, slug) catch return html(allocator, config, "/app/notes", username, "<h2>Nota no encontrada</h2><a href=\"/app/notes\">Volver a las notas</a>", 404);
        const content = loadContent(allocator, config, note) catch "";
        const safe_title = try security.htmlEscape(allocator, note.title);
        const safe_content = try security.htmlEscape(allocator, content);
        const safe_tags = try security.htmlEscape(allocator, note.tags);
        const body = try templates.simplePage(allocator, "Editar nota", try std.fmt.allocPrint(allocator, "<form method=\"post\" action=\"/app/notes\" class=\"grid gap-4\"><label>Título<input name=\"title\" value=\"{s}\"></label><label>Etiquetas<input name=\"tags\" value=\"{s}\"></label><label>Markdown<textarea rows=\"18\">{s}</textarea></label><p>Edición API disponible en PUT /api/notes/{s}.</p></form>", .{ safe_title, safe_tags, safe_content, slug }));
        return html(allocator, config, "/app/notes", username, body, 200);
    }
    if (!std.mem.eql(u8, suffix, "") or !std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
    const note = getNote(allocator, db, slug) catch return html(allocator, config, "/app/notes", username, "<h2>Nota no encontrada</h2><a href=\"/app/notes\">Volver a las notas</a>", 404);
    const content: ?[]const u8 = loadContent(allocator, config, note) catch null;
    const safe_title = try security.htmlEscape(allocator, note.title);
    const slug_html = try security.htmlEscape(allocator, note.slug);
    const body_html = if (content) |value| try renderMarkdown(allocator, value) else "<p class=\"text-on-surface-variant\">Contenido no disponible en disco.</p>";
    const body = try templates.simplePage(allocator, safe_title, try std.fmt.allocPrint(allocator, "<p class=\"text-sm text-outline\">{s} · {s}</p><div class=\"prose prose-invert my-6\">{s}</div><a href=\"/app/notes\">Volver a las notas</a><form method=\"post\" action=\"/api/notes/{s}\"><button type=\"button\" data-delete-slug=\"{s}\">Eliminar</button></form>", .{ slug_html, note.updated_at, body_html, slug_html, slug_html }));
    return html(allocator, config, "/app/notes", username, body, 200);
}

fn searchApi(allocator: std.mem.Allocator, db: *sqlite.Db, request: *const http.Request) !http.Response {
    if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
    const q = (try http.queryParam(allocator, request.query, "q")) orelse "";
    if (q.len > 200) return jsonError(allocator, 400, "Query too long");
    var obj = std.json.ObjectMap.init(allocator);
    try obj.put("query", .{ .string = q });
    var results = std.json.Array.init(allocator);
    const trimmed = std.mem.trim(u8, q, " \t\r\n");
    if (trimmed.len > 0) {
        const expression = try ftsExpression(allocator, trimmed);
        if (expression.len > 0) _ = appendFtsResults(allocator, db, expression, &results);
        if (results.items.len == 0) {
            const pattern = try likePattern(allocator, trimmed);
            var stmt = db.prepare("SELECT slug,title,tags FROM notes WHERE title LIKE ?1 ESCAPE '\\' OR slug LIKE ?1 ESCAPE '\\' OR tags LIKE ?1 ESCAPE '\\' ORDER BY updated_at DESC LIMIT 50") catch return jsonError(allocator, 500, "Search unavailable");
            defer stmt.deinit();
            try stmt.bindText(1, pattern);
            while (try stmt.next()) try appendSearchRow(allocator, &results, &stmt);
        }
    }
    try obj.put("count", .{ .integer = @intCast(results.items.len) });
    try obj.put("results", .{ .array = results });
    return jsonResponse(allocator, 200, .{ .object = obj });
}

fn appendFtsResults(allocator: std.mem.Allocator, db: *sqlite.Db, expression: []const u8, results: *std.json.Array) bool {
    var stmt = db.prepare("SELECT n.slug,n.title,n.tags FROM notes_search_fts JOIN notes n ON n.id=notes_search_fts.rowid WHERE notes_search_fts MATCH ?1 ORDER BY n.updated_at DESC LIMIT 50") catch return false;
    defer stmt.deinit();
    stmt.bindText(1, expression) catch return false;
    while (true) {
        const has_row = stmt.next() catch return false;
        if (!has_row) break;
        appendSearchRow(allocator, results, &stmt) catch return false;
    }
    return results.items.len > 0;
}

fn appendSearchRow(allocator: std.mem.Allocator, results: *std.json.Array, stmt: *sqlite.Statement) !void {
    var row = std.json.ObjectMap.init(allocator);
    try row.put("slug", .{ .string = try allocator.dupe(u8, stmt.text(0)) });
    try row.put("title", .{ .string = try allocator.dupe(u8, stmt.text(1)) });
    try row.put("tags", .{ .string = try allocator.dupe(u8, stmt.text(2)) });
    try results.append(.{ .object = row });
}

fn ftsExpression(allocator: std.mem.Allocator, query: []const u8) ![]const u8 {
    var expression: std.ArrayListUnmanaged(u8) = .empty;
    var terms = std.mem.tokenizeAny(u8, query, " \t\r\n");
    while (terms.next()) |term| {
        var has_token_char = false;
        for (term) |ch| {
            if (std.ascii.isAlphanumeric(ch) or ch >= 0x80) {
                has_token_char = true;
                break;
            }
        }
        if (!has_token_char) continue;
        if (expression.items.len > 0) try expression.append(allocator, ' ');
        try expression.append(allocator, '"');
        for (term) |ch| {
            if (ch == '"') try expression.appendSlice(allocator, "\"\"") else try expression.append(allocator, ch);
        }
        try expression.appendSlice(allocator, "\"*");
    }
    return expression.items;
}

fn searchPage(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request, username: []const u8) !http.Response {
    const q = (try http.queryParam(allocator, request.query, "q")) orelse "";
    if (q.len > 200) return html(allocator, config, "/dashboard/search", username, "<p>La consulta es demasiado larga.</p>", 400);
    const escaped_q = try security.htmlEscape(allocator, q);
    var cards: std.ArrayListUnmanaged(u8) = .empty;
    const trimmed = std.mem.trim(u8, q, " \t\r\n");
    if (trimmed.len > 0) {
        const expression = try ftsExpression(allocator, trimmed);
        if (expression.len > 0) _ = appendFtsCards(allocator, db, expression, &cards);
        if (cards.items.len == 0) {
            const pattern = try likePattern(allocator, trimmed);
            var stmt = try db.prepare("SELECT slug,title,tags,COALESCE(summary,'') FROM notes WHERE title LIKE ?1 ESCAPE '\\' OR slug LIKE ?1 ESCAPE '\\' OR tags LIKE ?1 ESCAPE '\\' ORDER BY updated_at DESC LIMIT 50");
            defer stmt.deinit();
            try stmt.bindText(1, pattern);
            while (try stmt.next()) try appendSearchCard(allocator, &cards, stmt.text(0), stmt.text(1), stmt.text(2), stmt.text(3));
        }
    }
    const help = if (q.len == 0) "Escribe algo para buscar en tus notas locales." else if (cards.items.len == 0) "No se encontraron notas." else cards.items;
    const body = try templates.simplePage(allocator, "Búsqueda global", try std.fmt.allocPrint(allocator, "<form method=\"get\" action=\"/dashboard/search\" class=\"flex gap-2 mb-4\"><input name=\"q\" value=\"{s}\" placeholder=\"Buscar notas…\"><button>Buscar</button></form><div class=\"grid gap-3\">{s}</div>", .{ escaped_q, help }));
    return html(allocator, config, "/dashboard/search", username, body, 200);
}

fn appendFtsCards(allocator: std.mem.Allocator, db: *sqlite.Db, expression: []const u8, cards: *std.ArrayListUnmanaged(u8)) bool {
    var stmt = db.prepare("SELECT n.slug,n.title,n.tags,COALESCE(n.summary,'') FROM notes_search_fts JOIN notes n ON n.id=notes_search_fts.rowid WHERE notes_search_fts MATCH ?1 ORDER BY n.updated_at DESC LIMIT 50") catch return false;
    defer stmt.deinit();
    stmt.bindText(1, expression) catch return false;
    while (true) {
        const has_row = stmt.next() catch return false;
        if (!has_row) break;
        appendSearchCard(allocator, cards, stmt.text(0), stmt.text(1), stmt.text(2), stmt.text(3)) catch return false;
    }
    return cards.items.len > 0;
}

fn appendSearchCard(allocator: std.mem.Allocator, cards: *std.ArrayListUnmanaged(u8), slug_raw: []const u8, title_raw: []const u8, tags_raw: []const u8, summary_raw: []const u8) !void {
    const slug = try security.htmlEscape(allocator, slug_raw);
    const title = try security.htmlEscape(allocator, title_raw);
    const tags = try security.htmlEscape(allocator, tags_raw);
    const summary = try security.htmlEscape(allocator, summary_raw);
    const card = try std.fmt.allocPrint(allocator, "<a class=\"block p-4 rounded-xl border border-outline-variant/40\" href=\"/app/notes/{s}\"><strong>{s}</strong><p>{s}</p><small>{s}</small></a>", .{ slug, title, summary, tags });
    try cards.appendSlice(allocator, card);
}

fn annotations(allocator: std.mem.Allocator, db: *sqlite.Db, request: *const http.Request, id_path: ?[]const u8) !http.Response {
    if (id_path == null and std.mem.eql(u8, request.method, "GET")) {
        const from = (try http.queryParam(allocator, request.query, "from")) orelse "0000-01-01";
        const to = (try http.queryParam(allocator, request.query, "to")) orelse "9999-12-31";
        var stmt = try db.prepare("SELECT id,date,title,body,created_at,updated_at FROM calendar_annotations WHERE date BETWEEN ?1 AND ?2 ORDER BY date,id");
        defer stmt.deinit();
        try stmt.bindText(1, from);
        try stmt.bindText(2, to);
        var items = std.json.Array.init(allocator);
        while (try stmt.next()) try items.append(try annotationValue(allocator, &stmt));
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("annotations", .{ .array = items });
        return jsonResponse(allocator, 200, .{ .object = obj });
    }
    if (id_path == null and std.mem.eql(u8, request.method, "POST")) {
        const parsed = parseBody(allocator, request) catch return jsonError(allocator, 400, "Invalid JSON");
        const date = json.getString(parsed, "date") orelse return jsonError(allocator, 400, "Invalid date");
        const title = json.getString(parsed, "title") orelse "";
        const body = json.getString(parsed, "body") orelse "";
        if (!validDate(date) or title.len > 120 or body.len > 4000) return jsonError(allocator, 400, "Invalid annotation");
        var stmt = try db.prepare("INSERT INTO calendar_annotations(date,title,body) VALUES(?1,?2,?3)");
        defer stmt.deinit();
        try stmt.bindText(1, date);
        try stmt.bindText(2, title);
        try stmt.bindText(3, body);
        _ = try stmt.next();
        return jsonResponse(allocator, 201, try annotationById(allocator, db, db.lastInsertRowId()));
    }
    const raw_id = id_path orelse return jsonError(allocator, 405, "Method not allowed");
    const id = std.fmt.parseInt(i64, raw_id, 10) catch return jsonError(allocator, 400, "Invalid id");
    if (std.mem.eql(u8, request.method, "PUT")) {
        const parsed = parseBody(allocator, request) catch return jsonError(allocator, 400, "Invalid JSON");
        const date = json.getString(parsed, "date") orelse return jsonError(allocator, 400, "Invalid date");
        const title = json.getString(parsed, "title") orelse "";
        const body = json.getString(parsed, "body") orelse "";
        if (!validDate(date) or title.len > 120 or body.len > 4000) return jsonError(allocator, 400, "Invalid annotation");
        var stmt = try db.prepare("UPDATE calendar_annotations SET date=?1,title=?2,body=?3,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?4");
        defer stmt.deinit();
        try stmt.bindText(1, date);
        try stmt.bindText(2, title);
        try stmt.bindText(3, body);
        try stmt.bindInt(4, id);
        _ = try stmt.next();
        if (db.changes() == 0) return jsonError(allocator, 404, "La anotación no existe");
        return jsonResponse(allocator, 200, try annotationById(allocator, db, id));
    }
    if (std.mem.eql(u8, request.method, "DELETE")) {
        var stmt = try db.prepare("DELETE FROM calendar_annotations WHERE id=?1");
        defer stmt.deinit();
        try stmt.bindInt(1, id);
        _ = try stmt.next();
        if (db.changes() == 0) return jsonError(allocator, 404, "La anotación no existe");
        return jsonResponse(allocator, 200, json.object(allocator, &.{.{ .key = "deleted", .value = .{ .bool = true } }}) catch unreachable);
    }
    return methodNotAllowed(allocator);
}

fn annotationValue(allocator: std.mem.Allocator, stmt: *sqlite.Statement) !std.json.Value {
    var obj = std.json.ObjectMap.init(allocator);
    try obj.put("id", .{ .integer = stmt.int64(0) });
    try obj.put("date", .{ .string = try allocator.dupe(u8, stmt.text(1)) });
    try obj.put("title", .{ .string = try allocator.dupe(u8, stmt.text(2)) });
    try obj.put("body", .{ .string = try allocator.dupe(u8, stmt.text(3)) });
    try obj.put("created_at", .{ .string = try allocator.dupe(u8, stmt.text(4)) });
    try obj.put("updated_at", .{ .string = try allocator.dupe(u8, stmt.text(5)) });
    return .{ .object = obj };
}

fn annotationById(allocator: std.mem.Allocator, db: *sqlite.Db, id: i64) !std.json.Value {
    var stmt = try db.prepare("SELECT id,date,title,body,created_at,updated_at FROM calendar_annotations WHERE id=?1");
    defer stmt.deinit();
    try stmt.bindInt(1, id);
    if (!try stmt.next()) return json.object(allocator, &.{}) catch unreachable;
    return annotationValue(allocator, &stmt);
}

fn validDate(date: []const u8) bool {
    if (date.len != 10 or date[4] != '-' or date[7] != '-') return false;
    for (date, 0..) |c, i| if (i != 4 and i != 7 and !std.ascii.isDigit(c)) return false;
    const year = std.fmt.parseInt(u16, date[0..4], 10) catch return false;
    const month = std.fmt.parseInt(u8, date[5..7], 10) catch return false;
    const day = std.fmt.parseInt(u8, date[8..10], 10) catch return false;
    if (year == 0 or month < 1 or month > 12 or day < 1) return false;
    const days_in_month: u8 = switch (month) {
        2 => if (isLeapYear(year)) 29 else 28,
        4, 6, 9, 11 => 30,
        else => 31,
    };
    return day <= days_in_month;
}

fn isLeapYear(year: u16) bool {
    return year % 4 == 0 and (year % 100 != 0 or year % 400 == 0);
}

fn createNote(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, title_raw: []const u8, content: []const u8, tags: []const u8, strict_duplicate: bool) !Note {
    const title = std.mem.trim(u8, title_raw, " \t\r\n");
    if (title.len == 0) return error.InvalidTitle;
    if (try charCount(title) > 200) return error.TitleTooLong;
    if (content.len > 500_000) return error.ContentTooLarge;
    if (tags.len > 500) return error.TagsTooLong;
    const slug = try slugify(allocator, title);
    if (!safeSlug(slug)) return error.InvalidTitle;
    const filename = try std.fmt.allocPrint(allocator, "{s}.md", .{slug});
    const summary = summarize(content);
    var stmt = db.prepare("INSERT INTO notes(title,slug,filename,tags,word_count,summary) VALUES(?1,?2,?3,?4,?5,?6)") catch |err| {
        if (strict_duplicate and isConstraint(err, db)) return error.NoteExists;
        return err;
    };
    defer stmt.deinit();
    try stmt.bindText(1, title);
    try stmt.bindText(2, slug);
    try stmt.bindText(3, filename);
    try stmt.bindText(4, tags);
    try stmt.bindInt(5, wordCount(content));
    try stmt.bindText(6, summary);
    _ = stmt.next() catch |err| {
        if (strict_duplicate and isConstraint(err, db)) return error.NoteExists;
        return err;
    };
    const id = db.lastInsertRowId();
    var version = try db.prepare("INSERT INTO note_versions(note_id,content,word_count) VALUES(?1,?2,?3)");
    defer version.deinit();
    try version.bindInt(1, id);
    try version.bindText(2, content);
    try version.bindInt(3, wordCount(content));
    _ = try version.next();
    try pruneVersions(db, id);
    var fts = db.prepare("INSERT OR REPLACE INTO notes_fts(rowid,title,content) VALUES(?1,?2,?3)") catch return .{ .id = id, .title = title, .slug = slug, .filename = filename, .tags = tags, .word_count = wordCount(content), .summary = summary, .created_at = "", .updated_at = "" };
    defer fts.deinit();
    try fts.bindInt(1, id);
    try fts.bindText(2, title);
    try fts.bindText(3, content);
    _ = fts.next() catch {};
    try writeMarkdown(allocator, config, filename, title, tags, content);
    return .{ .id = id, .title = title, .slug = slug, .filename = filename, .tags = tags, .word_count = wordCount(content), .summary = summary, .created_at = "", .updated_at = "" };
}

fn updateNote(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request, slug: []const u8, api: bool) !http.Response {
    if (!safeSlug(slug)) return jsonError(allocator, 400, "Invalid slug");
    const existing = getNote(allocator, db, slug) catch return jsonError(allocator, 404, "not found");
    const parsed = parseBody(allocator, request) catch return jsonError(allocator, 400, "Invalid JSON");
    const title = json.getString(parsed, "title") orelse existing.title;
    const content = json.getString(parsed, "content") orelse (loadContent(allocator, config, existing) catch "");
    const tags = json.getString(parsed, "tags") orelse existing.tags;
    if (std.mem.trim(u8, title, " \t\r\n").len == 0 or (try charCount(title)) > 200) return jsonError(allocator, 400, "Invalid title");
    if (content.len > 500_000) return jsonError(allocator, 400, "Content too large");
    if (tags.len > 500) return jsonError(allocator, 400, "Tags too long");
    var stmt = try db.prepare("UPDATE notes SET title=?1,tags=?2,word_count=?3,summary=?4,updated_at=datetime('now') WHERE slug=?5");
    defer stmt.deinit();
    try stmt.bindText(1, title);
    try stmt.bindText(2, tags);
    try stmt.bindInt(3, wordCount(content));
    try stmt.bindText(4, summarize(content));
    try stmt.bindText(5, slug);
    _ = try stmt.next();
    var ver = try db.prepare("INSERT INTO note_versions(note_id,content,word_count) VALUES(?1,?2,?3)");
    defer ver.deinit();
    try ver.bindInt(1, existing.id);
    try ver.bindText(2, content);
    try ver.bindInt(3, wordCount(content));
    _ = try ver.next();
    try pruneVersions(db, existing.id);
    var fts = try db.prepare("INSERT OR REPLACE INTO notes_fts(rowid,title,content) VALUES(?1,?2,?3)");
    defer fts.deinit();
    try fts.bindInt(1, existing.id);
    try fts.bindText(2, title);
    try fts.bindText(3, content);
    _ = try fts.next();
    try writeMarkdown(allocator, config, existing.filename, title, tags, content);
    if (api) return jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "updated", .value = .{ .bool = true } }}));
    return jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "message", .value = .{ .string = "Note updated" } }}));
}

fn apiNoteRoute(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request, slug: []const u8) !http.Response {
    if (!safeSlug(slug)) return jsonError(allocator, 400, "Invalid slug");
    if (std.mem.eql(u8, request.method, "GET")) {
        const note = getNote(allocator, db, slug) catch return jsonError(allocator, 404, "not found");
        const body = loadContent(allocator, config, note) catch "";
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("note", try noteValue(allocator, note));
        try obj.put("content", .{ .string = body });
        try obj.put("content_available", .{ .bool = body.len > 0 or note.word_count == 0 });
        return jsonResponse(allocator, 200, .{ .object = obj });
    }
    if (std.mem.eql(u8, request.method, "PUT")) return updateNote(allocator, db, config, request, slug, true);
    if (std.mem.eql(u8, request.method, "DELETE")) return deleteNote(allocator, db, config, slug, true);
    return methodNotAllowed(allocator);
}

fn deleteNote(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, slug: []const u8, api: bool) !http.Response {
    if (!safeSlug(slug)) return if (api) jsonError(allocator, 400, "Invalid slug") else jsonError(allocator, 404, "Note not found");
    const note = getNote(allocator, db, slug) catch return if (api) jsonError(allocator, 404, "not found") else jsonError(allocator, 404, "Note not found");
    var stmt = try db.prepare("DELETE FROM notes WHERE slug=?1");
    defer stmt.deinit();
    try stmt.bindText(1, slug);
    _ = try stmt.next();
    const path = try std.fs.path.join(allocator, &.{ config.notes_dir, note.filename });
    std.fs.cwd().deleteFile(path) catch {};
    return if (api) jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "deleted", .value = .{ .bool = true } }})) else jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "message", .value = .{ .string = "Note deleted" } }}));
}

fn noteVersions(allocator: std.mem.Allocator, db: *sqlite.Db, slug: []const u8) !http.Response {
    if (!safeSlug(slug)) return jsonError(allocator, 400, "Invalid slug");
    const note = getNote(allocator, db, slug) catch return jsonError(allocator, 404, "not found");
    var stmt = try db.prepare("SELECT id,word_count,created_at FROM note_versions WHERE note_id=?1 ORDER BY id DESC LIMIT 5");
    defer stmt.deinit();
    try stmt.bindInt(1, note.id);
    var versions = std.json.Array.init(allocator);
    while (try stmt.next()) {
        try versions.append(try json.object(allocator, &.{
            .{ .key = "id", .value = .{ .integer = stmt.int64(0) } },
            .{ .key = "word_count", .value = .{ .integer = stmt.int64(1) } },
            .{ .key = "created_at", .value = .{ .string = try allocator.dupe(u8, stmt.text(2)) } },
        }));
    }
    return jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "versions", .value = .{ .array = versions } }}));
}

fn restoreVersion(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request, slug: []const u8, version_id: i64) !http.Response {
    if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
    if (!safeSlug(slug) or version_id <= 0) return jsonError(allocator, 400, "Invalid version id");
    const note = getNote(allocator, db, slug) catch return jsonError(allocator, 404, "not found");
    var find = try db.prepare("SELECT content FROM note_versions WHERE id=?1 AND note_id=?2");
    defer find.deinit();
    try find.bindInt(1, version_id);
    try find.bindInt(2, note.id);
    if (!try find.next()) return jsonError(allocator, 404, "Version not found");
    const content = try allocator.dupe(u8, find.text(0));

    try db.exec("BEGIN IMMEDIATE;");
    errdefer db.exec("ROLLBACK;") catch {};
    var update = try db.prepare("UPDATE notes SET word_count=?1,summary=?2,updated_at=datetime('now') WHERE id=?3");
    defer update.deinit();
    try update.bindInt(1, wordCount(content));
    try update.bindText(2, summarize(content));
    try update.bindInt(3, note.id);
    _ = try update.next();
    var snapshot = try db.prepare("INSERT INTO note_versions(note_id,content,word_count) VALUES(?1,?2,?3)");
    defer snapshot.deinit();
    try snapshot.bindInt(1, note.id);
    try snapshot.bindText(2, content);
    try snapshot.bindInt(3, wordCount(content));
    _ = try snapshot.next();
    var fts = db.prepare("INSERT OR REPLACE INTO notes_fts(rowid,title,content) VALUES(?1,?2,?3)") catch null;
    if (fts) |*stmt| {
        defer stmt.deinit();
        try stmt.bindInt(1, note.id);
        try stmt.bindText(2, note.title);
        try stmt.bindText(3, content);
        _ = try stmt.next();
    }
    try pruneVersions(db, note.id);
    try writeMarkdown(allocator, config, note.filename, note.title, note.tags, content);
    try db.exec("COMMIT;");
    return redirect(allocator, try std.fmt.allocPrint(allocator, "/app/notes/{s}", .{slug}), 303);
}

fn exportNote(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, slug: []const u8) !http.Response {
    const note = getNote(allocator, db, slug) catch return textResponse(allocator, 404, "text/plain; charset=utf-8", "not found");
    const content = loadContent(allocator, config, note) catch "";
    const headers = try allocator.alloc(http.Header, 1);
    headers[0] = .{ .name = "Content-Disposition", .value = try std.fmt.allocPrint(allocator, "attachment; filename=\"{s}\"", .{note.filename}) };
    return .{ .status = 200, .content_type = "text/markdown; charset=utf-8", .body = content, .headers = headers };
}

fn pruneVersions(db: *sqlite.Db, note_id: i64) !void {
    var stmt = try db.prepare("DELETE FROM note_versions WHERE note_id=?1 AND id NOT IN (SELECT id FROM note_versions WHERE note_id=?1 ORDER BY id DESC LIMIT 5)");
    defer stmt.deinit();
    try stmt.bindInt(1, note_id);
    _ = try stmt.next();
}

fn getNote(allocator: std.mem.Allocator, db: *sqlite.Db, slug: []const u8) !Note {
    var stmt = try db.prepare("SELECT id,title,slug,filename,tags,word_count,COALESCE(summary,''),created_at,updated_at FROM notes WHERE slug=?1");
    defer stmt.deinit();
    try stmt.bindText(1, slug);
    if (!try stmt.next()) return error.NotFound;
    return readNote(allocator, &stmt);
}

fn readNote(allocator: std.mem.Allocator, stmt: *sqlite.Statement) !Note {
    return .{ .id = stmt.int64(0), .title = try allocator.dupe(u8, stmt.text(1)), .slug = try allocator.dupe(u8, stmt.text(2)), .filename = try allocator.dupe(u8, stmt.text(3)), .tags = try allocator.dupe(u8, stmt.text(4)), .word_count = stmt.int64(5), .summary = try allocator.dupe(u8, stmt.text(6)), .created_at = try allocator.dupe(u8, stmt.text(7)), .updated_at = try allocator.dupe(u8, stmt.text(8)) };
}

fn noteValue(allocator: std.mem.Allocator, note: Note) !std.json.Value {
    var obj = std.json.ObjectMap.init(allocator);
    try obj.put("id", .{ .integer = note.id });
    try obj.put("title", .{ .string = note.title });
    try obj.put("slug", .{ .string = note.slug });
    try obj.put("filename", .{ .string = note.filename });
    try obj.put("tags", .{ .string = note.tags });
    try obj.put("word_count", .{ .integer = note.word_count });
    try obj.put("summary", .{ .string = note.summary });
    try obj.put("created_at", .{ .string = note.created_at });
    try obj.put("updated_at", .{ .string = note.updated_at });
    return .{ .object = obj };
}

fn loadContent(allocator: std.mem.Allocator, config: *const config_mod.Config, note: Note) ![]const u8 {
    if (!safeSlug(note.slug) or std.mem.indexOfAny(u8, note.filename, "/\\") != null) return error.InvalidPath;
    const path = try std.fs.path.join(allocator, &.{ config.notes_dir, note.filename });
    const file = try std.fs.cwd().openFile(path, .{});
    defer file.close();
    const raw = try file.readToEndAlloc(allocator, 500_000);
    if (std.mem.startsWith(u8, raw, "---\n")) {
        if (std.mem.indexOf(u8, raw[4..], "\n---\n")) |end| return raw[4 + end + 5 ..];
    }
    return raw;
}

fn writeMarkdown(allocator: std.mem.Allocator, config: *const config_mod.Config, filename: []const u8, title: []const u8, tags: []const u8, content: []const u8) !void {
    if (std.mem.indexOfAny(u8, filename, "/\\") != null) return error.InvalidPath;
    try std.fs.cwd().makePath(config.notes_dir);
    const path = try std.fs.path.join(allocator, &.{ config.notes_dir, filename });
    const file = try std.fs.cwd().createFile(path, .{ .truncate = true });
    defer file.close();
    const frontmatter = try std.fmt.allocPrint(allocator, "---\ntitle: \"{s}\"\ntags: \"{s}\"\n---\n\n", .{ title, tags });
    try file.writeAll(frontmatter);
    try file.writeAll(content);
}

fn renderMarkdown(allocator: std.mem.Allocator, markdown: []const u8) ![]const u8 {
    const escaped = try security.htmlEscape(allocator, markdown);
    var output: std.ArrayListUnmanaged(u8) = .empty;
    var lines = std.mem.splitScalar(u8, escaped, '\n');
    while (lines.next()) |line| {
        const trim = std.mem.trim(u8, line, " \t\r");
        if (trim.len == 0) continue;
        if (std.mem.startsWith(u8, trim, "### ")) {
            try output.appendSlice(allocator, try std.fmt.allocPrint(allocator, "<h3>{s}</h3>", .{trim[4..]}));
        } else if (std.mem.startsWith(u8, trim, "## ")) {
            try output.appendSlice(allocator, try std.fmt.allocPrint(allocator, "<h2>{s}</h2>", .{trim[3..]}));
        } else if (std.mem.startsWith(u8, trim, "# ")) {
            try output.appendSlice(allocator, try std.fmt.allocPrint(allocator, "<h1>{s}</h1>", .{trim[2..]}));
        } else {
            try output.appendSlice(allocator, try std.fmt.allocPrint(allocator, "<p>{s}</p>", .{trim}));
        }
    }
    return output.items;
}

fn slugify(allocator: std.mem.Allocator, title: []const u8) ![]const u8 {
    var out: std.ArrayListUnmanaged(u8) = .empty;
    var last_dash = true;
    for (title) |byte| {
        if (std.ascii.isAlphanumeric(byte)) {
            try out.append(allocator, std.ascii.toLower(byte));
            last_dash = false;
        } else if (byte >= 0x80) {
            // Conserva UTF-8 válido en slugs acentuados (SQLite y rutas HTTP lo admiten).
            try out.append(allocator, byte);
            last_dash = false;
        } else if (!last_dash) {
            try out.append(allocator, '-');
            last_dash = true;
        }
    }
    while (out.items.len > 0 and out.items[out.items.len - 1] == '-') _ = out.pop();
    if (out.items.len == 0) return error.InvalidTitle;
    if (out.items.len > 180) out.items.len = 180;
    return out.items;
}

fn safeSlug(slug: []const u8) bool {
    return slug.len > 0 and !std.mem.eql(u8, slug, ".") and !std.mem.eql(u8, slug, "..") and std.mem.indexOf(u8, slug, "..") == null and std.mem.indexOfAny(u8, slug, "/\\\x00") == null;
}

fn wordCount(text: []const u8) i64 {
    var count: i64 = 0;
    var in_word = false;
    for (text) |byte| {
        if (std.ascii.isWhitespace(byte)) {
            in_word = false;
        } else if (!in_word) {
            in_word = true;
            count += 1;
        }
    }
    return count;
}

fn charCount(text: []const u8) !usize {
    return std.unicode.utf8CountCodepoints(text) catch return text.len;
}

fn summarize(content: []const u8) []const u8 {
    const text = std.mem.trim(u8, content, " \t\r\n");
    if (text.len <= 240) return text;
    return text[0..240];
}

fn likePattern(allocator: std.mem.Allocator, query: []const u8) ![]const u8 {
    var escaped: std.ArrayListUnmanaged(u8) = .empty;
    try escaped.append(allocator, '%');
    for (query) |ch| {
        if (ch == '%' or ch == '_' or ch == '\\') try escaped.append(allocator, '\\');
        try escaped.append(allocator, ch);
    }
    try escaped.append(allocator, '%');
    return escaped.items;
}

fn parseBody(allocator: std.mem.Allocator, request: *const http.Request) !std.json.Value {
    const content_type = request.header("content-type") orelse "";
    if (std.mem.indexOf(u8, content_type, "application/json") == null and std.mem.trim(u8, request.body, " \t\r\n").len == 0) return error.InvalidJson;
    return json.parse(allocator, request.body);
}

fn isConstraint(_: anyerror, db: *sqlite.Db) bool {
    return std.mem.indexOf(u8, db.errorMessage(), "UNIQUE") != null or std.mem.indexOf(u8, db.errorMessage(), "constraint") != null;
}

fn html(allocator: std.mem.Allocator, config: *const config_mod.Config, route: []const u8, username: []const u8, body: []const u8, status: u16) !http.Response {
    return .{ .status = status, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, route, body, username, true, config.mikareno_mcp_url) };
}

fn jsonResponse(allocator: std.mem.Allocator, status: u16, value: std.json.Value) !http.Response {
    return .{ .status = status, .content_type = "application/json; charset=utf-8", .body = try json.stringify(allocator, value) };
}

fn jsonError(allocator: std.mem.Allocator, status: u16, message: []const u8) !http.Response {
    return jsonResponse(allocator, status, try json.object(allocator, &.{.{ .key = "error", .value = .{ .string = message } }}));
}

fn textResponse(_: std.mem.Allocator, status: u16, content_type: []const u8, body: []const u8) http.Response {
    return .{ .status = status, .content_type = content_type, .body = body };
}

fn redirect(allocator: std.mem.Allocator, location: []const u8, status: u16) !http.Response {
    const headers = try allocator.alloc(http.Header, 1);
    headers[0] = .{ .name = "Location", .value = location };
    return .{ .status = status, .content_type = "text/plain; charset=utf-8", .body = "", .headers = headers };
}

fn methodNotAllowed(allocator: std.mem.Allocator) !http.Response {
    return jsonError(allocator, 405, "Method not allowed");
}

fn apiNoteDispatch(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request) !http.Response {
    const rest = request.path["/api/notes/".len..];
    if (std.mem.endsWith(u8, rest, "/export")) {
        if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
        return exportNote(allocator, db, config, rest[0 .. rest.len - "/export".len]);
    }
    if (std.mem.endsWith(u8, rest, "/versions") and std.mem.eql(u8, request.method, "GET")) {
        return noteVersions(allocator, db, rest[0 .. rest.len - "/versions".len]);
    }
    return apiNoteRoute(allocator, db, config, request, rest);
}

// Allow other Zig modules to share the `/api/notes/{slug}` controller.
pub fn handleApiNote(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request) !http.Response {
    return apiNoteDispatch(allocator, db, config, request);
}

// Produce a standards-compliant calendar from local annotations. School data
// from Mikareno is read-only and is not fabricated when no client is configured.
pub fn calendarIcs(allocator: std.mem.Allocator, db: *sqlite.Db) ![]const u8 {
    var output: std.ArrayListUnmanaged(u8) = .empty;
    try appendIcsLine(allocator, &output, "BEGIN:VCALENDAR");
    try appendIcsLine(allocator, &output, "VERSION:2.0");
    try appendIcsLine(allocator, &output, "PRODID:-//Study Platform//ES");
    try appendIcsLine(allocator, &output, "CALSCALE:GREGORIAN");
    try appendIcsLine(allocator, &output, "METHOD:PUBLISH");
    const stamp = try icsTimestamp(allocator);
    var stmt = try db.prepare("SELECT id,date,title,body FROM calendar_annotations ORDER BY date,id");
    defer stmt.deinit();
    while (try stmt.next()) {
        const date = stmt.text(1);
        if (!validDate(date)) continue;
        const compact = try std.fmt.allocPrint(allocator, "{s}{s}{s}", .{ date[0..4], date[5..7], date[8..10] });
        const next_date = try nextCalendarDate(allocator, date);
        const escaped_title = try icsEscape(allocator, if (stmt.text(2).len == 0) "Anotación" else stmt.text(2));
        const escaped_body = try icsEscape(allocator, stmt.text(3));
        try appendIcsLine(allocator, &output, "BEGIN:VEVENT");
        try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "UID:annotation-{d}@study-platform.local", .{stmt.int64(0)}));
        try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "DTSTAMP:{s}", .{stamp}));
        try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "DTSTART;VALUE=DATE:{s}", .{compact}));
        try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "DTEND;VALUE=DATE:{s}", .{next_date}));
        try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "SUMMARY:{s}", .{escaped_title}));
        if (escaped_body.len > 0) try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "DESCRIPTION:{s}", .{escaped_body}));
        try appendIcsLine(allocator, &output, "END:VEVENT");
    }
    try appendIcsLine(allocator, &output, "END:VCALENDAR");
    return output.items;
}

fn icsTimestamp(allocator: std.mem.Allocator) ![]const u8 {
    const seconds = std.time.timestamp();
    const epoch = std.time.epoch.EpochSeconds{ .secs = @intCast(@max(0, seconds)) };
    const year_day = epoch.getEpochDay().calculateYearDay();
    const month_day = year_day.calculateMonthDay();
    const day = epoch.getDaySeconds();
    return std.fmt.allocPrint(allocator, "{d:0>4}{d:0>2}{d:0>2}T{d:0>2}{d:0>2}{d:0>2}Z", .{ year_day.year, month_day.month.numeric(), month_day.day_index + 1, day.getHoursIntoDay(), day.getMinutesIntoHour(), day.getSecondsIntoMinute() });
}

fn nextCalendarDate(allocator: std.mem.Allocator, date: []const u8) ![]const u8 {
    const year = try std.fmt.parseInt(u16, date[0..4], 10);
    var month = try std.fmt.parseInt(u8, date[5..7], 10);
    var day = try std.fmt.parseInt(u8, date[8..10], 10);
    const days_in_month: u8 = switch (month) {
        2 => if (isLeapYear(year)) 29 else 28,
        4, 6, 9, 11 => 30,
        else => 31,
    };
    if (day < days_in_month) {
        day += 1;
    } else if (month < 12) {
        month += 1;
        day = 1;
    } else {
        month = 1;
        day = 1;
        return std.fmt.allocPrint(allocator, "{d:0>4}{d:0>2}{d:0>2}", .{ @as(u32, year) + 1, month, day });
    }
    return std.fmt.allocPrint(allocator, "{d:0>4}{d:0>2}{d:0>2}", .{ year, month, day });
}

fn icsEscape(allocator: std.mem.Allocator, text: []const u8) ![]const u8 {
    var escaped: std.ArrayListUnmanaged(u8) = .empty;
    var i: usize = 0;
    while (i < text.len) : (i += 1) {
        switch (text[i]) {
            '\\' => try escaped.appendSlice(allocator, "\\\\"),
            ';' => try escaped.appendSlice(allocator, "\\;"),
            ',' => try escaped.appendSlice(allocator, "\\,"),
            '\r' => {
                if (i + 1 < text.len and text[i + 1] == '\n') i += 1;
                try escaped.appendSlice(allocator, "\\n");
            },
            '\n' => try escaped.appendSlice(allocator, "\\n"),
            else => try escaped.append(allocator, text[i]),
        }
    }
    return escaped.items;
}

fn appendIcsLine(allocator: std.mem.Allocator, output: *std.ArrayListUnmanaged(u8), line: []const u8) !void {
    var offset: usize = 0;
    var column: usize = 0;
    while (offset < line.len) {
        const first = line[offset];
        const width: usize = if (first < 0x80) 1 else std.unicode.utf8ByteSequenceLength(first) catch 1;
        const safe_width = @min(width, line.len - offset);
        if (column + safe_width > 75) {
            try output.appendSlice(allocator, "\r\n ");
            column = 1;
        }
        try output.appendSlice(allocator, line[offset .. offset + safe_width]);
        column += safe_width;
        offset += safe_width;
    }
    try output.appendSlice(allocator, "\r\n");
}

test "slug validation rejects traversal" {
    try std.testing.expect(safeSlug("apuntes-algebra"));
    try std.testing.expect(!safeSlug("../secret"));
    try std.testing.expect(!safeSlug("a\\b"));
}
