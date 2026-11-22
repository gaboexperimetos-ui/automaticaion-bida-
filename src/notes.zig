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
    if (std.mem.eql(u8, path, "/api/calendar/annotations/import")) return importAnnotations(allocator, db, request);
    if (std.mem.eql(u8, path, "/api/notes/sync-vault") and std.mem.eql(u8, request.method, "POST")) {
        return syncVault(allocator, db, config);
    }
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

const CalendarAnnotation = struct {
    date: []const u8,
    title: []const u8,
    body: []const u8,
    category: []const u8,
    kind: []const u8,
    end_date: ?[]const u8,
    all_day: bool,
    start_time: ?[]const u8,
    end_time: ?[]const u8,
    location: []const u8,
    uid: ?[]const u8,
    recurrence: ?[]const u8,
    weekly_day: ?i64,
};

const CALENDAR_ANNOTATION_COLUMNS = "id,date,title,body,category,kind,end_date,all_day,start_time,end_time,location,uid,recurrence,weekly_day,created_at,updated_at";

fn annotations(allocator: std.mem.Allocator, db: *sqlite.Db, request: *const http.Request, id_path: ?[]const u8) !http.Response {
    if (id_path == null and std.mem.eql(u8, request.method, "GET")) {
        const from_param = try http.queryParam(allocator, request.query, "from");
        const to_param = try http.queryParam(allocator, request.query, "to");
        const from = from_param orelse "0000-01-01";
        const to = to_param orelse "9999-12-31";
        if ((from_param != null and !validDate(from)) or (to_param != null and !validDate(to)) or std.mem.order(u8, from, to) == .gt) return jsonError(allocator, 400, "Invalid date range");
        const sql = try std.fmt.allocPrint(allocator, "SELECT {s} FROM calendar_annotations WHERE (date>=?1 AND date<=?2) OR (kind='event' AND date<?1 AND ((all_day=1 AND end_date>?1) OR (all_day=0 AND COALESCE(end_date,date)>=?1))) OR (kind='event' AND recurrence='weekly') ORDER BY date,id", .{CALENDAR_ANNOTATION_COLUMNS});
        var stmt = try db.prepare(sql);
        defer stmt.deinit();
        try stmt.bindText(1, from);
        try stmt.bindText(2, to);
        var items = std.json.Array.init(allocator);
        while (try stmt.next()) try items.append(try calendarAnnotationValue(allocator, &stmt));
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("from", .{ .string = from });
        try obj.put("to", .{ .string = to });
        try obj.put("count", .{ .integer = @intCast(items.items.len) });
        try obj.put("annotations", .{ .array = items });
        return jsonResponse(allocator, 200, .{ .object = obj });
    }
    if (id_path == null and std.mem.eql(u8, request.method, "POST")) {
        const payload = parseBody(allocator, request) catch return jsonError(allocator, 400, "Invalid JSON");
        var problem: []const u8 = "Entrada no válida";
        const entry = (try normalizeCalendarAnnotation(allocator, payload, &problem)) orelse return jsonError(allocator, 400, problem);
        const id = try insertCalendarAnnotation(db, entry);
        return annotationEnvelope(allocator, 201, try calendarAnnotationById(allocator, db, id));
    }

    const raw_id = id_path orelse return methodNotAllowed(allocator);
    const id = std.fmt.parseInt(i64, raw_id, 10) catch return jsonError(allocator, 400, "Invalid id");
    if (std.mem.eql(u8, request.method, "PUT")) {
        const payload = parseBody(allocator, request) catch return jsonError(allocator, 400, "Invalid JSON");
        const date = json.getString(payload, "date");
        if (date) |value| if (!validDate(std.mem.trim(u8, value, " \t\r\n"))) return jsonError(allocator, 400, "La fecha debe tener formato YYYY-MM-DD");
        const title_raw = json.getString(payload, "title");
        const body_raw = json.getString(payload, "body");
        const title = if (title_raw) |value| try cleanCalendarText(allocator, value, 120) else null;
        const body = if (body_raw) |value| try cleanCalendarText(allocator, value, 4000) else null;
        const category_raw = json.getString(payload, "category");
        const category = if (category_raw) |value| normalizeCalendarCategory(value) orelse return jsonError(allocator, 400, "La categoría debe ser academic, work, health o personal") else null;
        var stmt = try db.prepare("UPDATE calendar_annotations SET date=COALESCE(?1,date),title=COALESCE(?2,title),body=COALESCE(?3,body),category=COALESCE(?4,category),updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?5");
        defer stmt.deinit();
        try bindOptionalText(&stmt, 1, if (date) |value| std.mem.trim(u8, value, " \t\r\n") else null);
        try bindOptionalText(&stmt, 2, title);
        try bindOptionalText(&stmt, 3, body);
        try bindOptionalText(&stmt, 4, category);
        try stmt.bindInt(5, id);
        _ = try stmt.next();
        if (db.changes() == 0) return jsonError(allocator, 404, "La anotación no existe");
        return annotationEnvelope(allocator, 200, try calendarAnnotationById(allocator, db, id));
    }
    if (std.mem.eql(u8, request.method, "DELETE")) {
        var stmt = try db.prepare("DELETE FROM calendar_annotations WHERE id=?1");
        defer stmt.deinit();
        try stmt.bindInt(1, id);
        _ = try stmt.next();
        if (db.changes() == 0) return jsonError(allocator, 404, "La anotación no existe");
        return jsonResponse(allocator, 200, try json.object(allocator, &.{
            .{ .key = "deleted", .value = .{ .bool = true } },
            .{ .key = "id", .value = .{ .integer = id } },
        }));
    }
    return methodNotAllowed(allocator);
}

fn importAnnotations(allocator: std.mem.Allocator, db: *sqlite.Db, request: *const http.Request) !http.Response {
    if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
    const payload = parseBody(allocator, request) catch return jsonError(allocator, 400, "Invalid JSON");
    const raw_entries = json.get(payload, "annotations") orelse return jsonError(allocator, 400, "Faltan las anotaciones del lote");
    if (raw_entries != .array or raw_entries.array.items.len == 0 or raw_entries.array.items.len > 1000) return jsonError(allocator, 400, "El lote debe contener entre 1 y 1000 eventos");
    var entries = std.ArrayList(CalendarAnnotation).init(allocator);
    for (raw_entries.array.items, 0..) |raw, index| {
        var problem: []const u8 = "Entrada no válida";
        const entry = (try normalizeCalendarAnnotation(allocator, raw, &problem)) orelse return jsonError(allocator, 400, try std.fmt.allocPrint(allocator, "Evento {d}: {s}", .{ index + 1, problem }));
        try entries.append(entry);
    }
    db.exec("BEGIN IMMEDIATE;") catch return jsonError(allocator, 500, "No se pudo iniciar la importación");
    for (entries.items) |entry| {
        _ = insertCalendarAnnotation(db, entry) catch {
            db.exec("ROLLBACK;") catch {};
            return jsonError(allocator, 500, "No se pudo completar la importación; no se guardaron cambios");
        };
    }
    db.exec("COMMIT;") catch {
        db.exec("ROLLBACK;") catch {};
        return jsonError(allocator, 500, "No se pudo completar la importación");
    };
    return jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "imported", .value = .{ .integer = @intCast(entries.items.len) } }}));
}

fn normalizeCalendarAnnotation(allocator: std.mem.Allocator, payload: std.json.Value, problem: *[]const u8) !?CalendarAnnotation {
    const date_raw = json.getString(payload, "date") orelse {
        problem.* = "La fecha debe tener formato YYYY-MM-DD";
        return null;
    };
    const date = std.mem.trim(u8, date_raw, " \t\r\n");
    if (!validDate(date)) {
        problem.* = "La fecha debe tener formato YYYY-MM-DD";
        return null;
    }
    const title_raw = json.getString(payload, "title") orelse "";
    const body_raw = json.getString(payload, "body") orelse "";
    const title = try cleanCalendarText(allocator, title_raw, 120);
    const body = try cleanCalendarText(allocator, body_raw, 4000);
    const kind_raw = json.getString(payload, "kind") orelse "planner";
    const kind = if (std.ascii.eqlIgnoreCase(std.mem.trim(u8, kind_raw, " \t\r\n"), "planner")) "planner" else if (std.ascii.eqlIgnoreCase(std.mem.trim(u8, kind_raw, " \t\r\n"), "event")) "event" else {
        problem.* = "El tipo debe ser planner o event";
        return null;
    };
    if ((std.mem.eql(u8, kind, "event") and title.len == 0) or (title.len == 0 and body.len == 0)) {
        problem.* = "La entrada necesita un título";
        return null;
    }
    const category_raw = json.getString(payload, "category") orelse if (std.mem.eql(u8, kind, "event")) "personal" else "academic";
    const category = normalizeCalendarCategory(category_raw) orelse {
        problem.* = "La categoría debe ser academic, work, health o personal";
        return null;
    };
    const raw_recurrence = json.getString(payload, "recurrence");
    const weekly_day = json.getInt(payload, "weekly_day");
    if (std.mem.eql(u8, kind, "planner")) {
        if (raw_recurrence != null or weekly_day != null) {
            problem.* = "La recurrencia solo se admite en eventos importados";
            return null;
        }
        return .{
            .date = date,
            .title = title,
            .body = body,
            .category = category,
            .kind = kind,
            .end_date = null,
            .all_day = true,
            .start_time = null,
            .end_time = null,
            .location = "",
            .uid = null,
            .recurrence = null,
            .weekly_day = null,
        };
    }

    const all_day = json.getBool(payload, "all_day") orelse true;
    const start_raw = json.getString(payload, "start_time");
    const end_raw = json.getString(payload, "end_time");
    const start_time = if (start_raw) |value| normalizeCalendarTime(value) orelse {
        problem.* = "La hora inicial debe tener formato HH:MM";
        return null;
    } else null;
    const end_time = if (end_raw) |value| normalizeCalendarTime(value) orelse {
        problem.* = "La hora final debe tener formato HH:MM";
        return null;
    } else null;
    if (all_day and (start_time != null or end_time != null)) {
        problem.* = "Un evento de día completo no puede incluir horas";
        return null;
    }
    if (!all_day and start_time == null) {
        problem.* = "Un evento con hora necesita una hora inicial";
        return null;
    }
    const end_raw_date = json.getString(payload, "end_date");
    const end_date = if (end_raw_date) |value| blk: {
        const trimmed = std.mem.trim(u8, value, " \t\r\n");
        if (!validDate(trimmed)) {
            problem.* = "La fecha final debe tener formato YYYY-MM-DD";
            return null;
        }
        break :blk trimmed;
    } else if (all_day) try nextCalendarDate(allocator, date) else date;
    if ((all_day and std.mem.order(u8, end_date, date) != .gt) or (!all_day and std.mem.order(u8, end_date, date) == .lt)) {
        problem.* = "La fecha final debe ser posterior a la inicial";
        return null;
    }
    if (!all_day and std.mem.eql(u8, end_date, date)) {
        if (start_time) |start| if (end_time) |end| if (std.mem.order(u8, end, start) != .gt) {
            problem.* = "La hora final debe ser posterior a la inicial";
            return null;
        };
    }
    const uid_raw = json.getString(payload, "uid");
    const uid_value = if (uid_raw) |value| try cleanCalendarText(allocator, value, 512) else "";
    for (uid_value) |byte| if (byte < 0x20 or byte == 0x7f) {
        problem.* = "El UID no puede contener caracteres de control";
        return null;
    };
    const recurrence_raw = raw_recurrence orelse "";
    const recurrence_trimmed = std.mem.trim(u8, recurrence_raw, " \t\r\n");
    const recurrence: ?[]const u8 = if (recurrence_trimmed.len == 0) null else if (std.ascii.eqlIgnoreCase(recurrence_trimmed, "weekly")) "weekly" else {
        problem.* = "La única recurrencia admitida es weekly";
        return null;
    };
    if (recurrence) |value| {
        _ = value;
        if (all_day or !std.mem.eql(u8, end_date, date) or start_time == null or end_time == null or weekly_day == null or weekly_day.? < 0 or weekly_day.? > 6) {
            problem.* = "La recurrencia semanal requiere un evento horario de un día, ambas horas y un día semanal entre 0 y 6";
            return null;
        }
    } else if (weekly_day != null) {
        problem.* = "El día semanal solo se admite con recurrencia weekly";
        return null;
    }
    const location_raw = json.getString(payload, "location") orelse "";
    return .{
        .date = date,
        .title = title,
        .body = body,
        .category = category,
        .kind = kind,
        .end_date = end_date,
        .all_day = all_day,
        .start_time = start_time,
        .end_time = end_time,
        .location = try cleanCalendarText(allocator, location_raw, 256),
        .uid = if (uid_value.len == 0) null else uid_value,
        .recurrence = recurrence,
        .weekly_day = weekly_day,
    };
}

fn cleanCalendarText(allocator: std.mem.Allocator, value: []const u8, max_codepoints: usize) ![]const u8 {
    const trimmed = std.mem.trim(u8, value, " \t\r\n");
    const view = std.unicode.Utf8View.init(trimmed) catch return error.InvalidUtf8;
    var iter = view.iterator();
    var byte_len: usize = 0;
    var count: usize = 0;
    while (iter.nextCodepointSlice()) |part| {
        if (count == max_codepoints) break;
        byte_len += part.len;
        count += 1;
    }
    return allocator.dupe(u8, trimmed[0..byte_len]);
}

fn normalizeCalendarCategory(value: []const u8) ?[]const u8 {
    const trimmed = std.mem.trim(u8, value, " \t\r\n");
    if (std.ascii.eqlIgnoreCase(trimmed, "academic")) return "academic";
    if (std.ascii.eqlIgnoreCase(trimmed, "work")) return "work";
    if (std.ascii.eqlIgnoreCase(trimmed, "health")) return "health";
    if (std.ascii.eqlIgnoreCase(trimmed, "personal")) return "personal";
    return null;
}

fn normalizeCalendarTime(value: []const u8) ?[]const u8 {
    const trimmed = std.mem.trim(u8, value, " \t\r\n");
    if (trimmed.len != 5 or trimmed[2] != ':' or !std.ascii.isDigit(trimmed[0]) or !std.ascii.isDigit(trimmed[1]) or !std.ascii.isDigit(trimmed[3]) or !std.ascii.isDigit(trimmed[4])) return null;
    const hour = std.fmt.parseInt(u8, trimmed[0..2], 10) catch return null;
    const minute = std.fmt.parseInt(u8, trimmed[3..5], 10) catch return null;
    if (hour > 23 or minute > 59) return null;
    return trimmed;
}

fn insertCalendarAnnotation(db: *sqlite.Db, entry: CalendarAnnotation) !i64 {
    var stmt = try db.prepare("INSERT INTO calendar_annotations(date,title,body,category,kind,end_date,all_day,start_time,end_time,location,uid,recurrence,weekly_day) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)");
    defer stmt.deinit();
    try stmt.bindText(1, entry.date);
    try stmt.bindText(2, entry.title);
    try stmt.bindText(3, entry.body);
    try stmt.bindText(4, entry.category);
    try stmt.bindText(5, entry.kind);
    try bindOptionalText(&stmt, 6, entry.end_date);
    try stmt.bindInt(7, if (entry.all_day) 1 else 0);
    try bindOptionalText(&stmt, 8, entry.start_time);
    try bindOptionalText(&stmt, 9, entry.end_time);
    try stmt.bindText(10, entry.location);
    try bindOptionalText(&stmt, 11, entry.uid);
    try bindOptionalText(&stmt, 12, entry.recurrence);
    if (entry.weekly_day) |day| try stmt.bindInt(13, day) else try stmt.bindNull(13);
    _ = try stmt.next();
    return db.lastInsertRowId();
}

fn bindOptionalText(stmt: *sqlite.Statement, index: c_int, value: ?[]const u8) !void {
    if (value) |text| try stmt.bindText(index, text) else try stmt.bindNull(index);
}

fn calendarAnnotationValue(allocator: std.mem.Allocator, stmt: *sqlite.Statement) !std.json.Value {
    var obj = std.json.ObjectMap.init(allocator);
    try obj.put("id", .{ .integer = stmt.int64(0) });
    try obj.put("date", .{ .string = try allocator.dupe(u8, stmt.text(1)) });
    try obj.put("title", .{ .string = try allocator.dupe(u8, stmt.text(2)) });
    try obj.put("body", .{ .string = try allocator.dupe(u8, stmt.text(3)) });
    try obj.put("category", .{ .string = try allocator.dupe(u8, stmt.text(4)) });
    try obj.put("kind", .{ .string = try allocator.dupe(u8, stmt.text(5)) });
    try obj.put("end_date", if (stmt.isNull(6)) .null else .{ .string = try allocator.dupe(u8, stmt.text(6)) });
    try obj.put("all_day", .{ .bool = stmt.int64(7) != 0 });
    try obj.put("start_time", if (stmt.isNull(8)) .null else .{ .string = try allocator.dupe(u8, stmt.text(8)) });
    try obj.put("end_time", if (stmt.isNull(9)) .null else .{ .string = try allocator.dupe(u8, stmt.text(9)) });
    try obj.put("location", .{ .string = try allocator.dupe(u8, stmt.text(10)) });
    try obj.put("uid", if (stmt.isNull(11)) .null else .{ .string = try allocator.dupe(u8, stmt.text(11)) });
    try obj.put("recurrence", if (stmt.isNull(12)) .null else .{ .string = try allocator.dupe(u8, stmt.text(12)) });
    try obj.put("weekly_day", if (stmt.isNull(13)) .null else .{ .integer = stmt.int64(13) });
    try obj.put("created_at", .{ .string = try allocator.dupe(u8, stmt.text(14)) });
    try obj.put("updated_at", .{ .string = try allocator.dupe(u8, stmt.text(15)) });
    return .{ .object = obj };
}

fn calendarAnnotationById(allocator: std.mem.Allocator, db: *sqlite.Db, id: i64) !std.json.Value {
    const sql = try std.fmt.allocPrint(allocator, "SELECT {s} FROM calendar_annotations WHERE id=?1", .{CALENDAR_ANNOTATION_COLUMNS});
    var stmt = try db.prepare(sql);
    defer stmt.deinit();
    try stmt.bindInt(1, id);
    if (!try stmt.next()) return json.object(allocator, &.{});
    return calendarAnnotationValue(allocator, &stmt);
}

fn annotationEnvelope(allocator: std.mem.Allocator, status: u16, annotation: std.json.Value) !http.Response {
    var obj = std.json.ObjectMap.init(allocator);
    if (annotation == .object) {
        var iter = annotation.object.iterator();
        while (iter.next()) |entry| try obj.put(entry.key_ptr.*, entry.value_ptr.*);
        try obj.put("annotation", annotation);
    }
    return jsonResponse(allocator, status, .{ .object = obj });
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
    return createNoteAs(allocator, db, config, title_raw, content, tags, null, strict_duplicate, true);
}

/// Igual que `createNote`, pero permite conservar el nombre de archivo original
/// (usado al importar `.md` del vault de Obsidian para no duplicar el archivo
/// al reescribirlo). Con `filename_override = null` usa `{slug}.md`.
/// `export_vault` controla si además se escribe en `VAULT_DIR`: la importación
/// desde el vault lo pone en `false` para no reescribir el archivo del usuario
/// (y no perder frontmatter extra como `created` o `aliases`).
fn createNoteAs(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, title_raw: []const u8, content: []const u8, tags: []const u8, filename_override: ?[]const u8, strict_duplicate: bool, export_vault: bool) !Note {
    const title = std.mem.trim(u8, title_raw, " \t\r\n");
    if (title.len == 0) return error.InvalidTitle;
    if (try charCount(title) > 200) return error.TitleTooLong;
    if (content.len > 500_000) return error.ContentTooLarge;
    if (tags.len > 500) return error.TagsTooLong;
    const slug = try slugify(allocator, title);
    if (!safeSlug(slug)) return error.InvalidTitle;
    const filename = if (filename_override) |f| blk: {
        if (std.mem.indexOfAny(u8, f, "/\\") != null or f.len == 0) return error.InvalidTitle;
        break :blk try allocator.dupe(u8, f);
    } else try std.fmt.allocPrint(allocator, "{s}.md", .{slug});
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
    if (export_vault) {
        try writeMarkdown(allocator, config, filename, title, tags, content);
    } else {
        try writeMarkdownAt(allocator, config.notes_dir, filename, title, tags, content);
    }
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
    const vault = config.vault_dir;
    if (vault.len > 0 and !std.mem.eql(u8, vault, config.notes_dir)) {
        const vault_path = try std.fs.path.join(allocator, &.{ vault, note.filename });
        std.fs.cwd().deleteFile(vault_path) catch {};
    }
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
    try writeMarkdownAt(allocator, config.notes_dir, filename, title, tags, content);
    // Exportación hacia el vault de Obsidian: si está configurado y es otra
    // carpeta, se escribe el mismo `.md` para que Obsidian/OpenCode lo vean.
    // Un fallo del vault no debe impedir guardar la nota en NOTES_DIR.
    const vault = config.vault_dir;
    if (vault.len > 0 and !std.mem.eql(u8, vault, config.notes_dir)) {
        writeVaultMarkdown(allocator, vault, filename, title, tags, content) catch |err| {
            std.log.warn("No se pudo escribir {s} en el vault ({s}): {s}", .{ filename, vault, @errorName(err) });
        };
    }
}

/// Escribe en el vault conservando cualquier campo de frontmatter que ya
/// existiera (`created`, `aliases`, `cssclass`…). Sólo reemplaza `title` y
/// `tags` y el cuerpo; el resto del encabezado se mantiene.
fn writeVaultMarkdown(allocator: std.mem.Allocator, vault: []const u8, filename: []const u8, title: []const u8, tags: []const u8, content: []const u8) !void {
    try std.fs.cwd().makePath(vault);
    const path = try std.fs.path.join(allocator, &.{ vault, filename });

    var extra: std.ArrayList(u8) = .init(allocator);
    if (std.fs.cwd().openFile(path, .{})) |f| {
        defer f.close();
        const raw = f.readToEndAlloc(allocator, 1_000_000) catch "";
        if (std.mem.startsWith(u8, raw, "---\n")) {
            if (std.mem.indexOf(u8, raw[4..], "\n---\n")) |closing| {
                var lines = std.mem.splitScalar(u8, raw[4 .. 4 + closing], '\n');
                while (lines.next()) |line| {
                    const t = std.mem.trim(u8, line, " \t");
                    if (t.len == 0) continue;
                    if (std.mem.startsWith(u8, t, "title:") or std.mem.startsWith(u8, t, "tags:")) continue;
                    try extra.appendSlice(t);
                    try extra.append('\n');
                }
            }
        }
    } else |_| {}

    const file = try std.fs.cwd().createFile(path, .{ .truncate = true });
    defer file.close();
    try file.writeAll("---\ntitle: \"");
    try file.writeAll(title);
    try file.writeAll("\"\ntags: \"");
    try file.writeAll(tags);
    try file.writeAll("\"\n");
    try file.writeAll(extra.items);
    try file.writeAll("---\n\n");
    try file.writeAll(content);
}

fn writeMarkdownAt(allocator: std.mem.Allocator, dir: []const u8, filename: []const u8, title: []const u8, tags: []const u8, content: []const u8) !void {
    try std.fs.cwd().makePath(dir);
    const path = try std.fs.path.join(allocator, &.{ dir, filename });
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
    var stmt = try db.prepare("SELECT id,date,title,body,kind,end_date,all_day,start_time,end_time,location,uid,recurrence,weekly_day FROM calendar_annotations ORDER BY date,id");
    defer stmt.deinit();
    while (try stmt.next()) {
        const date = stmt.text(1);
        if (!validDate(date)) continue;
        const date_compact = try compactIcsDate(allocator, date);
        const end_date = if (stmt.isNull(5) or stmt.text(5).len == 0) try nextCalendarDate(allocator, date) else stmt.text(5);
        if (!validDate(end_date)) continue;
        const end_compact = try compactIcsDate(allocator, end_date);
        const title = try icsEscape(allocator, if (stmt.text(2).len == 0) "Anotación" else stmt.text(2));
        const body = try icsEscape(allocator, stmt.text(3));
        const location = try icsEscape(allocator, stmt.text(9));
        const all_day = stmt.int64(6) != 0 or std.mem.eql(u8, stmt.text(4), "planner");
        try appendIcsLine(allocator, &output, "BEGIN:VEVENT");
        const uid = if (!stmt.isNull(10) and stmt.text(10).len > 0) stmt.text(10) else try std.fmt.allocPrint(allocator, "annotation-{d}@study-platform.local", .{stmt.int64(0)});
        try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "UID:{s}", .{try icsEscape(allocator, uid)}));
        try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "DTSTAMP:{s}", .{stamp}));
        if (all_day) {
            try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "DTSTART;VALUE=DATE:{s}", .{date_compact}));
            try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "DTEND;VALUE=DATE:{s}", .{end_compact}));
        } else {
            const start_time = if (!stmt.isNull(7)) stmt.text(7) else "";
            if (normalizeCalendarTime(start_time)) |valid_start| {
                try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "DTSTART:{s}T{s}00", .{ date_compact, try compactIcsTime(allocator, valid_start) }));
            } else {
                try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "DTSTART;VALUE=DATE:{s}", .{date_compact}));
            }
            const end_time = if (!stmt.isNull(8)) normalizeCalendarTime(stmt.text(8)) else null;
            if (end_time) |valid_end| try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "DTEND:{s}T{s}00", .{ end_compact, try compactIcsTime(allocator, valid_end) }));
        }
        try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "SUMMARY:{s}", .{title}));
        if (body.len > 0) try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "DESCRIPTION:{s}", .{body}));
        if (location.len > 0) try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "LOCATION:{s}", .{location}));
        if (std.mem.eql(u8, stmt.text(11), "weekly") and !stmt.isNull(12)) {
            const days = [_][]const u8{ "SU", "MO", "TU", "WE", "TH", "FR", "SA" };
            const day = stmt.int64(12);
            if (day >= 0 and day <= 6) try appendIcsLine(allocator, &output, try std.fmt.allocPrint(allocator, "RRULE:FREQ=WEEKLY;BYDAY={s}", .{days[@intCast(day)]}));
        }
        try appendIcsLine(allocator, &output, "END:VEVENT");
    }
    try appendIcsLine(allocator, &output, "END:VCALENDAR");
    return output.items;
}

fn compactIcsDate(allocator: std.mem.Allocator, date: []const u8) ![]const u8 {
    return std.fmt.allocPrint(allocator, "{s}{s}{s}", .{ date[0..4], date[5..7], date[8..10] });
}

fn compactIcsTime(allocator: std.mem.Allocator, time: []const u8) ![]const u8 {
    return std.fmt.allocPrint(allocator, "{s}{s}", .{ time[0..2], time[3..5] });
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
        return std.fmt.allocPrint(allocator, "{d:0>4}-{d:0>2}-{d:0>2}", .{ @as(u32, year) + 1, month, day });
    }
    return std.fmt.allocPrint(allocator, "{d:0>4}-{d:0>2}-{d:0>2}", .{ year, month, day });
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

/// Parsea el frontmatter YAML mínimo de una nota del vault:
/// `title: "..."` y `tags: ...` (lista entre corchetes o separada por comas).
/// Devuelve título, etiquetas y el cuerpo Markdown sin el frontmatter.
fn parseFrontmatter(allocator: std.mem.Allocator, raw: []const u8) !Frontmatter {
    if (!std.mem.startsWith(u8, raw, "---\n")) {
        return .{ .title = "", .tags = "", .body = std.mem.trim(u8, raw, "\n") };
    }
    const closing = std.mem.indexOf(u8, raw[4..], "\n---\n") orelse return error.InvalidFrontmatter;
    const header = raw[4 .. 4 + closing];
    const body = std.mem.trim(u8, raw[4 + closing + 5 ..], "\n");
    var title: []const u8 = "";
    var tags_raw: []const u8 = "";
    var lines = std.mem.splitScalar(u8, header, '\n');
    while (lines.next()) |line| {
        const trimmed = std.mem.trim(u8, line, " \t");
        if (std.mem.startsWith(u8, trimmed, "title:")) {
            title = try frontmatterValue(allocator, trimmed[6..]);
        } else if (std.mem.startsWith(u8, trimmed, "tags:")) {
            tags_raw = try frontmatterValue(allocator, trimmed[5..]);
        }
    }
    const tags = try tagsValueToTags(allocator, tags_raw);
    return .{ .title = title, .tags = tags, .body = body };
}

/// Devuelve el valor de una clave del frontmatter, quitando comillas.
fn frontmatterValue(allocator: std.mem.Allocator, value: []const u8) ![]const u8 {
    var v = std.mem.trim(u8, value, " \t");
    if (v.len >= 2 and v[0] == '"' and v[v.len - 1] == '"') {
        v = v[1 .. v.len - 1];
    } else if (v.len >= 2 and v[0] == '\'' and v[v.len - 1] == '\'') {
        v = v[1 .. v.len - 1];
    }
    return try allocator.dupe(u8, v);
}

/// Convierte el valor crudo de `tags` en una cadena separada por comas.
/// Acepta `etiqueta1, etiqueta2`, `[etiqueta1, etiqueta2]` o `etiqueta1`.
fn tagsValueToTags(allocator: std.mem.Allocator, raw: []const u8) ![]const u8 {
    var v = std.mem.trim(u8, raw, " \t");
    if (v.len >= 2 and v[0] == '[' and v[v.len - 1] == ']') v = v[1 .. v.len - 1];
    if (v.len == 0) return allocator.dupe(u8, "");
    var out: std.ArrayList([]const u8) = .init(allocator);
    defer out.deinit();
    var parts = std.mem.splitScalar(u8, v, ',');
    while (parts.next()) |part| {
        var tag = std.mem.trim(u8, part, " \t\"'");
        tag = std.mem.trim(u8, tag, "#");
        if (tag.len == 0) continue;
        try out.append(try allocator.dupe(u8, tag));
    }
    if (out.items.len == 0) return allocator.dupe(u8, "");
    var joined: std.ArrayList(u8) = .init(allocator);
    for (out.items, 0..) |tag, i| {
        if (i > 0) try joined.appendSlice(", ");
        try joined.appendSlice(tag);
    }
    return joined.toOwnedSlice();
}

const Frontmatter = struct {
    title: []const u8,
    tags: []const u8,
    body: []const u8,
};

/// Actualiza una nota existente en BD, fichero `.md` (espejo en notes_dir),
/// FTS y versiones. Usado por la sincronización del vault.
fn updateNoteDB(
    db: *sqlite.Db,
    id: i64,
    title: []const u8,
    tags: []const u8,
    content: []const u8,
) !void {
    var stmt = try db.prepare(
        "UPDATE notes SET title=?1,tags=?2,word_count=?3,summary=?4,updated_at=datetime('now') WHERE id=?5",
    );
    defer stmt.deinit();
    try stmt.bindText(1, title);
    try stmt.bindText(2, tags);
    try stmt.bindInt(3, wordCount(content));
    try stmt.bindText(4, summarize(content));
    try stmt.bindInt(5, id);
    _ = try stmt.next();

    var ver = try db.prepare("INSERT INTO note_versions(note_id,content,word_count) VALUES(?1,?2,?3)");
    defer ver.deinit();
    try ver.bindInt(1, id);
    try ver.bindText(2, content);
    try ver.bindInt(3, wordCount(content));
    _ = try ver.next();
    try pruneVersions(db, id);

    var fts = try db.prepare("INSERT OR REPLACE INTO notes_fts(rowid,title,content) VALUES(?1,?2,?3)");
    defer fts.deinit();
    try fts.bindInt(1, id);
    try fts.bindText(2, title);
    try fts.bindText(3, content);
    _ = fts.next() catch {};
}

/// Sincroniza el vault de Obsidian (`config.vault_dir`) con la base de datos.
/// Importa cada `.md` del vault: crea notas nuevas y actualiza las que
/// cambiaron. Devuelve conteos de importadas, actualizadas y omitidas.
pub fn syncVault(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    config: *const config_mod.Config,
) !http.Response {
    var dir = try std.fs.cwd().openDir(config.vault_dir, .{ .iterate = true });
    defer dir.close();

    var imported: usize = 0;
    var updated: usize = 0;
    var skipped: usize = 0;
    var errors: std.ArrayList([]const u8) = .init(allocator);
    defer errors.deinit();

    var iter = dir.iterate();
    while (try iter.next()) |entry| {
        if (entry.kind != .file) continue;
        if (!std.mem.endsWith(u8, entry.name, ".md")) continue;

        const path = try std.fs.path.join(allocator, &.{ config.vault_dir, entry.name });

        var file: std.fs.File = undefined;
        {
            const open_result = std.fs.cwd().openFile(path, .{});
            if (open_result) |f| {
                file = f;
            } else |_| {
                const name_copy = allocator.dupe(u8, entry.name) catch continue;
                _ = errors.append(name_copy) catch {};
                continue;
            }
        }
        defer file.close();

        var raw: []const u8 = "";
        {
            const read_result = file.readToEndAlloc(allocator, 500_000);
            if (read_result) |r| {
                raw = r;
            } else |_| {
                const name_copy = allocator.dupe(u8, entry.name) catch continue;
                _ = errors.append(name_copy) catch {};
                continue;
            }
        }

        var fm: Frontmatter = .{ .title = "", .tags = "", .body = "" };
        {
            const parse_result = parseFrontmatter(allocator, raw);
            if (parse_result) |p| {
                fm = p;
            } else |_| {
                const name_copy = allocator.dupe(u8, entry.name) catch continue;
                _ = errors.append(name_copy) catch {};
                continue;
            }
        }

        const title = if (fm.title.len > 0) fm.title else entry.name[0 .. entry.name.len - 3];
        const slug = try slugify(allocator, title);

        if (getNote(allocator, db, slug)) |note| {
            const current = std.mem.trim(u8, loadContent(allocator, config, note) catch "", "\n");
            if (std.mem.eql(u8, current, fm.body)) {
                skipped += 1;
                continue;
            }
            try updateNoteDB(db, note.id, title, fm.tags, fm.body);
            try writeMarkdownAt(allocator, config.notes_dir, note.filename, title, fm.tags, fm.body);
            updated += 1;
        } else |err| {
            if (err == error.NotFound) {
                _ = try createNoteAs(allocator, db, config, title, fm.body, fm.tags, entry.name, false, false);
                imported += 1;
            } else {
                const name_copy = allocator.dupe(u8, entry.name) catch continue;
                _ = errors.append(name_copy) catch {};
                continue;
            }
        }
    }

    var obj = std.json.ObjectMap.init(allocator);
    try obj.put("imported", .{ .integer = @intCast(imported) });
    try obj.put("updated", .{ .integer = @intCast(updated) });
    try obj.put("skipped", .{ .integer = @intCast(skipped) });
    try obj.put("vault_dir", .{ .string = config.vault_dir });
    var err_list = std.json.Array.init(allocator);
    for (errors.items) |err_name| {
        try err_list.append(.{ .string = err_name });
    }
    try obj.put("errors", .{ .array = err_list });
    return jsonResponse(allocator, 200, .{ .object = obj });
}

test "slug validation rejects traversal" {
    try std.testing.expect(safeSlug("apuntes-algebra"));
    try std.testing.expect(!safeSlug("../secret"));
    try std.testing.expect(!safeSlug("a\\b"));
}

test "parseFrontmatter extrae title y tags y limpia el cuerpo" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const raw = "---\ntitle: \"Nota de prueba\"\ntags: [es, test]\n---\n\nCuerpo real.\n";
    const fm = try parseFrontmatter(arena.allocator(), raw);
    try std.testing.expectEqualStrings("Nota de prueba", fm.title);
    try std.testing.expectEqualStrings("es, test", fm.tags);
    try std.testing.expectEqualStrings("Cuerpo real.", fm.body);
}

test "parseFrontmatter sin frontmatter usa todo el texto como cuerpo" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const fm = try parseFrontmatter(arena.allocator(), "Solo texto plano.\n");
    try std.testing.expectEqualStrings("", fm.title);
    try std.testing.expectEqualStrings("", fm.tags);
    try std.testing.expectEqualStrings("Solo texto plano.", fm.body);
}

test "writeMarkdownAt escribe frontmatter y cuerpo" {
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const dir = try tmp.dir.realpathAlloc(arena.allocator(), ".");
    try writeMarkdownAt(arena.allocator(), dir, "nota.md", "Titulo", "a, b", "Cuerpo");
    const data = try tmp.dir.readFileAlloc(arena.allocator(), "nota.md", 10000);
    try std.testing.expect(std.mem.indexOf(u8, data, "title: \"Titulo\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, data, "tags: \"a, b\"") != null);
    try std.testing.expect(std.mem.endsWith(u8, data, "Cuerpo"));
}

test "writeVaultMarkdown conserva el frontmatter extra" {
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const dir = try tmp.dir.realpathAlloc(arena.allocator(), ".");
    {
        const f = try tmp.dir.createFile("nota.md", .{});
        defer f.close();
        try f.writeAll("---\ntitle: \"Viejo\"\ntags: [x]\ncreated: 2026-11-20\naliases: [a]\n---\n\nViejo cuerpo.\n");
    }
    try writeVaultMarkdown(arena.allocator(), dir, "nota.md", "Nuevo", "y", "Cuerpo nuevo");
    const data = try tmp.dir.readFileAlloc(arena.allocator(), "nota.md", 10000);
    try std.testing.expect(std.mem.indexOf(u8, data, "title: \"Nuevo\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, data, "tags: \"y\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, data, "created: 2026-11-20") != null);
    try std.testing.expect(std.mem.indexOf(u8, data, "aliases: [a]") != null);
    try std.testing.expect(std.mem.endsWith(u8, data, "Cuerpo nuevo"));
}
