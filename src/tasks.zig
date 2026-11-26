//! Bandeja de tareas unificada de NOVA.
//!
//! Reúne cuatro fuentes reales y las presenta en una sola lista:
//!
//! * **Mikareno** — actividades por entregar (`get_student_full`).
//! * **Google Classroom** — coursework próximo (`upcomingWork`).
//! * **WhatsApp** — notas que el triage de Hermes creó con tag `whatsapp`.
//! * **Notas propias** — notas con tag `tarea`, `pendiente` o `deber`.
//!
//! Reglas: nada inventado, todo escapado, duplicados por título+fecha
//! eliminados y el origen siempre visible. Classroom se cachea en disco para
//! que la página no espere a Google en cada carga.

const std = @import("std");
const json = @import("jsonutil.zig");
const security = @import("security.zig");
const classroom = @import("classroom.zig");
const panel = @import("panel.zig");

pub const Source = enum { mikareno, classroom, whatsapp, note };

pub const Item = struct {
    source: Source,
    title: []const u8,
    detail: []const u8,
    due: []const u8,
    link: []const u8,
    course: []const u8,
};

pub const NoteRow = struct {
    slug: []const u8,
    title: []const u8,
    tags: []const u8,
    summary: []const u8,
};

pub const Counts = struct {
    total: usize = 0,
    mikareno: usize = 0,
    classroom: usize = 0,
    whatsapp: usize = 0,
    note: usize = 0,
};

pub const classroom_cache_ttl: i64 = 600;

fn esc(allocator: std.mem.Allocator, s: []const u8) ![]const u8 {
    return security.htmlEscape(allocator, s);
}

pub fn sourceLabel(source: Source) []const u8 {
    return switch (source) {
        .mikareno => "Mikareno",
        .classroom => "Classroom",
        .whatsapp => "WhatsApp",
        .note => "Nota",
    };
}

fn sourceClass(source: Source) []const u8 {
    return switch (source) {
        .mikareno => "nx-inbox-src-mikareno",
        .classroom => "nx-inbox-src-classroom",
        .whatsapp => "nx-inbox-src-whatsapp",
        .note => "nx-inbox-src-note",
    };
}

pub fn parseSource(raw: []const u8) ?Source {
    if (std.mem.eql(u8, raw, "mikareno")) return .mikareno;
    if (std.mem.eql(u8, raw, "classroom")) return .classroom;
    if (std.mem.eql(u8, raw, "whatsapp")) return .whatsapp;
    if (std.mem.eql(u8, raw, "note") or std.mem.eql(u8, raw, "notas")) return .note;
    return null;
}

/// Clave de deduplicación: título normalizado + día de entrega. Se ignoran
/// mayúsculas, acentos y signos para que "¡Examen!" y "examen" coincidan.
fn normKey(allocator: std.mem.Allocator, title: []const u8, due: []const u8) ![]const u8 {
    var out = std.ArrayList(u8).init(allocator);
    var pending_space = false;
    for (title) |c| {
        if (c >= 0x80) continue;
        if (std.ascii.isAlphanumeric(c)) {
            if (pending_space and out.items.len > 0) try out.append(' ');
            pending_space = false;
            try out.append(std.ascii.toLower(c));
        } else {
            pending_space = true;
        }
    }
    if (due.len >= 10) {
        try out.append('|');
        try out.appendSlice(due[0..10]);
    }
    return out.items;
}

fn lessThan(_: void, a: Item, b: Item) bool {
    const ad = if (a.due.len >= 10) a.due[0..10] else "";
    const bd = if (b.due.len >= 10) b.due[0..10] else "";
    if (ad.len == 0 and bd.len != 0) return false;
    if (ad.len != 0 and bd.len == 0) return true;
    const ord = std.mem.order(u8, ad, bd);
    if (ord != .eq) return ord == .lt;
    return std.mem.order(u8, a.title, b.title) == .lt;
}

fn tagsContain(tags: []const u8, needle: []const u8) bool {
    return std.mem.indexOf(u8, tags, needle) != null;
}

pub fn fromClassroom(allocator: std.mem.Allocator, items: []const classroom.Upcoming) ![]Item {
    var out = std.ArrayList(Item).init(allocator);
    for (items) |it| {
        const external = if (std.mem.startsWith(u8, it.link, "https://") or std.mem.startsWith(u8, it.link, "http://")) it.link else "";
        const link = if (external.len > 0)
            external
        else
            try std.fmt.allocPrint(allocator, "/dashboard/classroom?course={s}", .{it.course_id});
        try out.append(.{
            .source = .classroom,
            .title = if (it.title.len == 0) "Trabajo de Classroom" else it.title,
            .detail = "",
            .due = it.due,
            .link = link,
            .course = it.course,
        });
    }
    return out.items;
}

pub fn fromMikareno(allocator: std.mem.Allocator, tareas: []const panel.Tarea) ![]Item {
    var out = std.ArrayList(Item).init(allocator);
    for (tareas) |t| {
        if (t.titulo.len == 0) continue;
        try out.append(.{
            .source = .mikareno,
            .title = t.titulo,
            .detail = "",
            .due = t.vencimiento,
            .link = "/dashboard/planner",
            .course = t.materia,
        });
    }
    return out.items;
}

/// Palabras que convierten un aviso de WhatsApp en algo accionable para el
/// colegio. El triage de Hermes etiqueta con `importante` cualquier mensaje
/// que case con su lista; aquí se descarta lo que no menciona nada académico
/// (p. ej. "Nadie importante"). Las notas siguen intactas en el vault.
fn academicText(text: []const u8) bool {
    const keys = [_][]const u8{
        "examen",        "tarea",     "deber",      "entrega",     "trabajo",
        "proyecto",      "leccion",   "lección",   "prueba",      "evaluacion",
        "evaluación",   "nota",      "cuaderno",   "clase",       "profesor",
        "materia",       "trimestre", "exposicion", "exposición", "recuperacion",
        "recuperación", "colegio",   "deberes",    "actividad",
    };
    var lower_buf: [512]u8 = undefined;
    const n = @min(text.len, lower_buf.len);
    for (text[0..n], 0..) |c, i| lower_buf[i] = std.ascii.toLower(c);
    const lower = lower_buf[0..n];
    for (keys) |key| {
        if (std.mem.indexOf(u8, lower, key) != null) return true;
    }
    return false;
}

/// Resume el encabezado que escribe el triage (`De:`, `Chat:`, `Fecha:`) en
/// una línea corta. Si el formato no coincide, devuelve el texto tal cual.
fn whatsappDetail(allocator: std.mem.Allocator, summary: []const u8) ![]const u8 {
    var who: []const u8 = "";
    var when: []const u8 = "";
    var lines = std.mem.splitScalar(u8, summary, '\n');
    while (lines.next()) |line| {
        const trimmed = std.mem.trim(u8, line, " \t\r");
        if (std.mem.startsWith(u8, trimmed, "De: ")) who = trimmed[4..];
        if (std.mem.startsWith(u8, trimmed, "Fecha: ")) when = trimmed[7..];
    }
    if (who.len == 0 and when.len == 0) {
        const flat = std.mem.trim(u8, summary, " \t\r\n");
        return allocator.dupe(u8, if (flat.len > 140) flat[0..140] else flat);
    }
    if (who.len > 0 and when.len > 0) return std.fmt.allocPrint(allocator, "{s} · {s}", .{ who, when });
    return allocator.dupe(u8, if (who.len > 0) who else when);
}

pub fn fromNotes(allocator: std.mem.Allocator, rows: []const NoteRow) ![]Item {
    var out = std.ArrayList(Item).init(allocator);
    for (rows) |row| {
        const whatsapp = tagsContain(row.tags, "whatsapp");
        if (whatsapp and !academicText(row.summary)) continue;
        try out.append(.{
            .source = if (whatsapp) .whatsapp else .note,
            .title = if (row.title.len == 0) "Nota" else row.title,
            .detail = if (whatsapp) try whatsappDetail(allocator, row.summary) else row.summary,
            .due = "",
            .link = try std.fmt.allocPrint(allocator, "/dashboard/notes?note={s}", .{row.slug}),
            .course = "",
        });
    }
    return out.items;
}

/// Junta las fuentes, elimina duplicados por título+fecha y ordena por
/// entrega. El orden de inserción fija la precedencia: Classroom > Mikareno >
/// WhatsApp > nota propia.
pub fn collect(
    allocator: std.mem.Allocator,
    mikareno_tasks: []const panel.Tarea,
    classroom_items: []const classroom.Upcoming,
    notes: []const NoteRow,
) ![]Item {
    var all = std.ArrayList(Item).init(allocator);
    try all.appendSlice(try fromClassroom(allocator, classroom_items));
    try all.appendSlice(try fromMikareno(allocator, mikareno_tasks));
    try all.appendSlice(try fromNotes(allocator, notes));

    var seen = std.StringHashMap(void).init(allocator);
    // Títulos ya vistos con fecha: un aviso sin fecha (WhatsApp/nota) con el
    // mismo título es el mismo pendiente, no una segunda entrega.
    var dated_titles = std.StringHashMap(void).init(allocator);
    var unique = std.ArrayList(Item).init(allocator);
    for (all.items) |item| {
        const key = try normKey(allocator, item.title, item.due);
        if (seen.contains(key)) continue;
        const title_key = try normKey(allocator, item.title, "");
        if (item.due.len >= 10) {
            try dated_titles.put(title_key, {});
        } else if (dated_titles.contains(title_key)) {
            continue;
        }
        try seen.put(key, {});
        try unique.append(item);
    }
    std.mem.sort(Item, unique.items, {}, lessThan);
    return unique.items;
}

pub fn count(items: []const Item) Counts {
    var c = Counts{ .total = items.len };
    for (items) |item| switch (item.source) {
        .mikareno => c.mikareno += 1,
        .classroom => c.classroom += 1,
        .whatsapp => c.whatsapp += 1,
        .note => c.note += 1,
    };
    return c;
}

/// Notas locales que funcionan como tareas: las crea el triage de WhatsApp o
/// las etiqueta la persona con tarea/pendiente/deber. Se excluyen las notas
/// índice del vault (`area`, `estudio`), que no son pendientes.
pub fn fetchNotes(allocator: std.mem.Allocator, db: anytype) ![]NoteRow {
    var stmt = db.prepare(
        "SELECT slug,title,tags,COALESCE(summary,'') FROM notes " ++
            "WHERE tags NOT LIKE '%area%' AND tags NOT LIKE '%estudio%' AND (" ++
            "(tags LIKE '%whatsapp%' AND tags LIKE '%importante%') OR (" ++
            "(tags LIKE '%tarea%' OR tags LIKE '%pendiente%' OR tags LIKE '%deber%') " ++
            "AND tags NOT LIKE '%whatsapp%')) " ++
            "ORDER BY updated_at DESC LIMIT 60",
    ) catch return &.{};
    defer stmt.deinit();
    var out = std.ArrayList(NoteRow).init(allocator);
    while (stmt.next() catch false) {
        out.append(.{
            .slug = try allocator.dupe(u8, stmt.text(0)),
            .title = try allocator.dupe(u8, stmt.text(1)),
            .tags = try allocator.dupe(u8, stmt.text(2)),
            .summary = try allocator.dupe(u8, stmt.text(3)),
        }) catch {};
    }
    return out.items;
}

pub fn cachePath(allocator: std.mem.Allocator, db_path: []const u8) ![]const u8 {
    const dir = std.fs.path.dirname(db_path) orelse ".";
    return std.fs.path.join(allocator, &.{ dir, "classroom-tasks.cache" });
}

/// Devuelve `null` si no hay caché válida (usuario distinto, expirada o
/// corrupta). Una lista vacía es una respuesta válida: no hay entregas.
pub fn loadClassroom(
    allocator: std.mem.Allocator,
    path: []const u8,
    user_id: i64,
    ttl_seconds: i64,
) ?[]classroom.Upcoming {
    const file = std.fs.cwd().openFile(path, .{}) catch return null;
    defer file.close();
    const content = file.readToEndAlloc(allocator, 4 * 1024 * 1024) catch return null;
    const root = json.parse(allocator, content) catch return null;
    const uid = json.getInt(root, "user") orelse return null;
    if (uid != user_id) return null;
    const stamp = json.getInt(root, "ts") orelse return null;
    const now = std.time.timestamp();
    if (stamp > now or now - stamp > ttl_seconds) return null;
    const arr = json.get(root, "items") orelse return null;
    if (arr != .array) return null;
    var out = std.ArrayList(classroom.Upcoming).init(allocator);
    for (arr.array.items) |value| {
        out.append(.{
            .course_id = json.getString(value, "course_id") orelse "",
            .course = json.getString(value, "course") orelse "",
            .title = json.getString(value, "title") orelse "",
            .due = json.getString(value, "due") orelse "",
            .link = json.getString(value, "link") orelse "",
        }) catch {};
    }
    return out.items;
}

pub fn saveClassroom(
    allocator: std.mem.Allocator,
    path: []const u8,
    user_id: i64,
    items: []const classroom.Upcoming,
) !void {
    var arr = std.ArrayList(json.Value).init(allocator);
    for (items) |it| {
        try arr.append(try json.object(allocator, &.{
            .{ .key = "course_id", .value = json.string(it.course_id) },
            .{ .key = "course", .value = json.string(it.course) },
            .{ .key = "title", .value = json.string(it.title) },
            .{ .key = "due", .value = json.string(it.due) },
            .{ .key = "link", .value = json.string(it.link) },
        }));
    }
    const root = try json.object(allocator, &.{
        .{ .key = "user", .value = json.integer(user_id) },
        .{ .key = "ts", .value = json.integer(std.time.timestamp()) },
        .{ .key = "items", .value = try json.array(allocator, arr.items) },
    });
    const text = try json.stringify(allocator, root);
    if (std.fs.path.dirname(path)) |dir| std.fs.cwd().makePath(dir) catch {};
    const tmp = try std.fmt.allocPrint(allocator, "{s}.tmp", .{path});
    var file = try std.fs.cwd().createFile(tmp, .{ .truncate = true });
    defer file.close();
    try file.writeAll(text);
    try std.fs.cwd().rename(tmp, path);
}

fn kpi(allocator: std.mem.Allocator, label: []const u8, value: usize, foot: []const u8, extra: []const u8) ![]const u8 {
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-grade-kpi {s}\"><span>{s}</span><strong>{d}</strong><small>{s}</small></div>",
        .{ extra, label, value, foot },
    );
}

fn filterLink(allocator: std.mem.Allocator, label: []const u8, href: []const u8, n: usize, active: bool) ![]const u8 {
    return std.fmt.allocPrint(
        allocator,
        "<a class=\"nx-inbox-filter{s}\" href=\"{s}\">{s}<b>{d}</b></a>",
        .{ if (active) " is-active" else "", href, label, n },
    );
}

pub fn pageHtml(allocator: std.mem.Allocator, items: []const Item, filter: ?Source, notice: []const u8) ![]const u8 {
    const c = count(items);
    var out = std.ArrayList(u8).init(allocator);
    try out.appendSlice("<div class=\"nx-page nova-page nx-inbox-page\">");
    try out.appendSlice(
        "<header class=\"nx-page-header nova-page-head\"><div><p class=\"nx-eyebrow\">Bandeja unificada</p><h1>Todo lo que tienes<br>pendiente.</h1><p>Mikareno, Classroom, tus notas y los avisos de WhatsApp en una sola lista, sin duplicados.</p></div><div class=\"nx-button-row\"><a class=\"nx-button nx-button-secondary\" href=\"/dashboard/tasks?refresh=1\"><span class=\"material-symbols-outlined\">refresh</span>Actualizar</a></div></header>",
    );

    if (notice.len > 0) {
        try out.appendSlice("<p class=\"nx-inbox-notice\"><span class=\"material-symbols-outlined\">info</span>");
        try out.appendSlice(try esc(allocator, notice));
        try out.appendSlice("</p>");
    }

    try out.appendSlice("<section class=\"nx-grade-kpis nx-academic-kpis nx-inbox-kpis\">");
    try out.appendSlice(try kpi(allocator, "Total", c.total, "pendientes únicas", ""));
    try out.appendSlice(try kpi(allocator, "Mikareno", c.mikareno, "actividades del colegio", "nx-inbox-kpi-mikareno"));
    try out.appendSlice(try kpi(allocator, "Classroom", c.classroom, "trabajos próximos", "nx-inbox-kpi-classroom"));
    try out.appendSlice(try kpi(allocator, "WhatsApp", c.whatsapp, "avisos importantes", "nx-inbox-kpi-whatsapp"));
    try out.appendSlice(try kpi(allocator, "Notas", c.note, "tareas propias", "nx-inbox-kpi-note"));
    try out.appendSlice("</section>");

    try out.appendSlice("<nav class=\"nx-inbox-filters\" aria-label=\"Filtrar por origen\">");
    try out.appendSlice(try filterLink(allocator, "Todas", "/dashboard/tasks", c.total, filter == null));
    try out.appendSlice(try filterLink(allocator, "Mikareno", "/dashboard/tasks?source=mikareno", c.mikareno, filter == .mikareno));
    try out.appendSlice(try filterLink(allocator, "Classroom", "/dashboard/tasks?source=classroom", c.classroom, filter == .classroom));
    try out.appendSlice(try filterLink(allocator, "WhatsApp", "/dashboard/tasks?source=whatsapp", c.whatsapp, filter == .whatsapp));
    try out.appendSlice(try filterLink(allocator, "Notas", "/dashboard/tasks?source=note", c.note, filter == .note));
    try out.appendSlice("</nav>");

    var shown: usize = 0;
    for (items) |item| {
        if (filter) |f| if (item.source != f) continue;
        if (shown == 0) {
            try out.appendSlice("<section class=\"nx-panel nx-inbox-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Pendientes</p><h2>En orden de entrega</h2></div></div><ul class=\"nx-inbox-list\">");
        }
        shown += 1;
        try out.appendSlice("<li class=\"nx-inbox-row\">");
        try out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<span class=\"nx-inbox-badge {s}\">{s}</span>",
            .{ sourceClass(item.source), sourceLabel(item.source) },
        ));
        try out.appendSlice("<div class=\"nx-inbox-body\"><strong>");
        try out.appendSlice(try esc(allocator, item.title));
        try out.appendSlice("</strong>");
        if (item.course.len > 0 or item.detail.len > 0) {
            try out.appendSlice("<small>");
            if (item.course.len > 0) try out.appendSlice(try esc(allocator, item.course));
            if (item.course.len > 0 and item.detail.len > 0) try out.appendSlice(" · ");
            if (item.detail.len > 0) try out.appendSlice(try esc(allocator, item.detail));
            try out.appendSlice("</small>");
        }
        try out.appendSlice("</div>");
        try out.appendSlice("<span class=\"nx-inbox-due\">");
        try out.appendSlice(try esc(allocator, if (item.due.len == 0) "sin fecha" else item.due));
        try out.appendSlice("</span>");
        if (item.link.len > 0) {
            try out.appendSlice("<a class=\"nx-inbox-open\" href=\"");
            try out.appendSlice(try esc(allocator, item.link));
            try out.appendSlice("\" aria-label=\"Abrir ");
            try out.appendSlice(try esc(allocator, item.title));
            try out.appendSlice("\"><span class=\"material-symbols-outlined\">arrow_forward</span></a>");
        }
        try out.appendSlice("</li>");
    }
    if (shown > 0) {
        try out.appendSlice("</ul></section>");
    } else {
        try out.appendSlice("<div class=\"nx-panel nx-academic-empty\"><span class=\"material-symbols-outlined\">inbox</span><p><strong>Sin tareas en esta vista</strong><br>Cuando haya entregas de Mikareno, Classroom o avisos de WhatsApp aparecerán aquí.</p></div>");
    }

    try out.appendSlice("</div>");
    return out.items;
}

// ──────────────────────────── Pruebas ────────────────────────────

test "parseSource acepta las cuatro fuentes" {
    try std.testing.expectEqual(Source.mikareno, parseSource("mikareno").?);
    try std.testing.expectEqual(Source.classroom, parseSource("classroom").?);
    try std.testing.expectEqual(Source.whatsapp, parseSource("whatsapp").?);
    try std.testing.expectEqual(Source.note, parseSource("note").?);
    try std.testing.expect(parseSource("otra") == null);
}

test "collect deduplica y prefiere Classroom" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const classroom_items = [_]classroom.Upcoming{
        .{ .course_id = "1", .course = "Biología", .title = "Examen trimestral", .due = "2026-12-02 12:30", .link = "https://classroom.google.com/c/1" },
    };
    const notes = [_]NoteRow{
        .{ .slug = "wa-1", .title = "Examen trimestral", .tags = "whatsapp,importante", .summary = "De: Erick\nFecha: 2026-12-01\n\nEl examen es el miércoles" },
        .{ .slug = "propia-1", .title = "Leer capítulo 4", .tags = "tarea", .summary = "" },
    };
    const mikareno = [_]panel.Tarea{
        .{ .titulo = "Examen trimestral", .vencimiento = "2026-12-02", .materia = "Biología" },
    };
    const items = try collect(a, &mikareno, &classroom_items, &notes);
    try std.testing.expectEqual(@as(usize, 2), items.len);
    try std.testing.expectEqual(Source.classroom, items[0].source);
    try std.testing.expectEqual(Source.note, items[1].source);
}

test "collect conserva el mismo título en fechas distintas" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const mikareno = [_]panel.Tarea{
        .{ .titulo = "Lección", .vencimiento = "2026-12-01" },
        .{ .titulo = "Lección", .vencimiento = "2026-12-08" },
    };
    const items = try collect(a, &mikareno, &.{}, &.{});
    try std.testing.expectEqual(@as(usize, 2), items.len);
}

test "collect ordena por fecha y deja las notas al final" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const notes = [_]NoteRow{
        .{ .slug = "n1", .title = "Nota sin fecha", .tags = "tarea", .summary = "" },
    };
    const mikareno = [_]panel.Tarea{
        .{ .titulo = "Tarde", .vencimiento = "2026-12-10" },
        .{ .titulo = "Pronto", .vencimiento = "2026-12-01", .materia = "Física" },
    };
    const items = try collect(a, &mikareno, &.{}, &notes);
    try std.testing.expectEqualStrings("Pronto", items[0].title);
    try std.testing.expectEqualStrings("Física", items[0].course);
    try std.testing.expectEqualStrings("Tarde", items[1].title);
    try std.testing.expectEqualStrings("Nota sin fecha", items[2].title);
}

test "fromNotes descarta avisos sin contenido académico" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const rows = [_]NoteRow{
        .{ .slug = "wa-1", .title = "WhatsApp biancagodoy: Nadie importante", .tags = "whatsapp,importante", .summary = "De: biancagodoy\n\nNadie importante" },
        .{ .slug = "wa-2", .title = "WhatsApp Erick: trae el cuaderno", .tags = "whatsapp,importante", .summary = "De: Erick\nFecha: 2026-11-26 15:33\n\nTrae el cuaderno para la clase" },
        .{ .slug = "propia", .title = "Estudiar física", .tags = "tarea", .summary = "" },
    };
    const items = try fromNotes(a, &rows);
    try std.testing.expectEqual(@as(usize, 2), items.len);
    try std.testing.expectEqual(Source.whatsapp, items[0].source);
    try std.testing.expectEqualStrings("Erick · 2026-11-26 15:33", items[0].detail);
    try std.testing.expectEqual(Source.note, items[1].source);
}

test "academicText reconoce avisos del colegio y rechaza charla" {
    try std.testing.expect(academicText("El EXAMEN es el miércoles"));
    try std.testing.expect(academicText("trae el cuaderno"));
    try std.testing.expect(!academicText("Nadie importante"));
    try std.testing.expect(!academicText("nos vemos luego"));
}

test "pageHtml escapa el texto y muestra los orígenes" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const items = [_]Item{
        .{ .source = .whatsapp, .title = "<script>alerta</script>", .detail = "", .due = "", .link = "", .course = "" },
        .{ .source = .classroom, .title = "Taller", .detail = "", .due = "2026-12-01", .link = "/dashboard/classroom?course=1", .course = "Biología" },
    };
    const html = try pageHtml(a, &items, null, "");
    try std.testing.expect(std.mem.indexOf(u8, html, "&lt;script&gt;") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-inbox-src-whatsapp") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-inbox-src-classroom") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Bandeja unificada") != null);
}

test "pageHtml con filtro vacío muestra el estado honesto" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const items = [_]Item{
        .{ .source = .mikareno, .title = "Tarea", .detail = "", .due = "", .link = "", .course = "" },
    };
    const html = try pageHtml(arena.allocator(), &items, .whatsapp, "");
    try std.testing.expect(std.mem.indexOf(u8, html, "Sin tareas en esta vista") != null);
}

test "pageHtml muestra el aviso cuando una fuente no responde" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const items = [_]Item{};
    const html = try pageHtml(arena.allocator(), &items, null, "Classroom no está conectado.");
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-inbox-notice") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Classroom no está conectado.") != null);
}

test "la caché de Classroom sobrevive ida y vuelta" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const path = try tmp.dir.realpathAlloc(a, ".");
    const cache = try std.fs.path.join(a, &.{ path, "classroom-tasks.cache" });
    const items = [_]classroom.Upcoming{
        .{ .course_id = "883050322377", .course = "Matemática", .title = "Polinomios", .due = "2026-12-01 23:59", .link = "https://classroom.google.com/c/abc" },
    };
    try saveClassroom(a, cache, 1, &items);
    const loaded = loadClassroom(a, cache, 1, classroom_cache_ttl) orelse return error.CacheMissing;
    try std.testing.expectEqual(@as(usize, 1), loaded.len);
    try std.testing.expectEqualStrings("Polinomios", loaded[0].title);
    try std.testing.expect(loadClassroom(a, cache, 2, classroom_cache_ttl) == null);
    try std.testing.expect(loadClassroom(a, cache, 1, -1) == null);
}
