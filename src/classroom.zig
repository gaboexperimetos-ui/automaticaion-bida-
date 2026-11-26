//! Google Classroom: cursos, trabajos y anuncios del usuario, en solo lectura.
//!
//! Reutiliza el refresh_token guardado en `google_oauth_tokens` y los scopes
//! `classroom.courses`, `classroom.coursework.me`, `classroom.announcements`
//! y `classroom.rosters`. Nunca escribe en Classroom ni inventa datos: si no
//! hay token o Google rechaza el acceso, devuelve un estado vacío honesto.

const std = @import("std");
const sqlite = @import("sqlite.zig");
const json = @import("jsonutil.zig");
const oauth = @import("oauth_google.zig");
const security = @import("security.zig");

const API = "https://classroom.googleapis.com/v1";

pub const Course = struct {
    id: []const u8,
    name: []const u8,
    section: []const u8,
    room: []const u8,
    state: []const u8,
    link: []const u8,
    description: []const u8,
    updated: []const u8,
};

pub const Work = struct {
    id: []const u8,
    title: []const u8,
    description: []const u8,
    state: []const u8,
    due: []const u8,
    max_points: []const u8,
    link: []const u8,
    work_type: []const u8,
};

pub const Announcement = struct {
    id: []const u8,
    text: []const u8,
    created: []const u8,
    link: []const u8,
};

pub const CoursesResult = struct {
    authorized: bool = false,
    configured: bool = false,
    courses: []Course = &.{},
    reason: []const u8 = "",
};

pub const Detail = struct {
    authorized: bool = false,
    configured: bool = false,
    course: ?Course = null,
    work: []const Work = &.{},
    announcements: []const Announcement = &.{},
    reason: []const u8 = "",
};

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

fn validCourseId(id: []const u8) bool {
    if (id.len == 0 or id.len > 32) return false;
    for (id) |c| {
        if (!std.ascii.isDigit(c)) return false;
    }
    return true;
}

fn text(value: std.json.Value, key: []const u8) []const u8 {
    return json.getString(value, key) orelse "";
}

/// Cursos activos del usuario (profesor o alumno).
pub fn fetchCourses(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
    max_results: usize,
) !CoursesResult {
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) {
        return .{ .authorized = false, .reason = "Google OAuth no está configurado en el servidor." };
    }
    const token = (try accessTokenFor(allocator, db, cfg, user_id)) orelse {
        return .{ .configured = true, .reason = "No has autorizado el acceso a Google Classroom. Conéctalo para ver tus cursos aquí." };
    };
    const url = try std.fmt.allocPrint(
        allocator,
        "{s}/courses?courseStates=ACTIVE&pageSize={d}&fields=courses(id,name,section,room,courseState,alternateLink,descriptionHeading,description,updateTime)",
        .{ API, max_results },
    );
    const payload = oauth.getJsonPublic(allocator, url, token, cfg.timeout_ms) catch {
        return .{ .configured = true, .reason = "Google Classroom no respondió o rechazó el acceso." };
    };
    const items = json.get(payload, "courses") orelse return .{ .authorized = true, .configured = true };
    if (items != .array) return .{ .authorized = true, .configured = true };
    var out = std.ArrayList(Course).init(allocator);
    for (items.array.items) |c| {
        if (c != .object) continue;
        const heading = text(c, "descriptionHeading");
        out.append(.{
            .id = text(c, "id"),
            .name = text(c, "name"),
            .section = text(c, "section"),
            .room = text(c, "room"),
            .state = text(c, "courseState"),
            .link = text(c, "alternateLink"),
            .description = if (heading.len > 0) heading else text(c, "description"),
            .updated = text(c, "updateTime"),
        }) catch {};
    }
    return .{ .authorized = true, .configured = true, .courses = out.items };
}

fn unsigned(value: i64) u64 {
    return @intCast(@max(value, 0));
}

fn dueLabel(allocator: std.mem.Allocator, work: std.json.Value) ![]const u8 {
    const date = json.get(work, "dueDate") orelse return "";
    const year = unsigned(json.getInt(date, "year") orelse return "");
    const month = unsigned(json.getInt(date, "month") orelse return "");
    const day = unsigned(json.getInt(date, "day") orelse return "");
    if (json.get(work, "dueTime")) |time| {
        const hours = unsigned(json.getInt(time, "hours") orelse 0);
        const minutes = unsigned(json.getInt(time, "minutes") orelse 0);
        return std.fmt.allocPrint(allocator, "{d:0>4}-{d:0>2}-{d:0>2} {d:0>2}:{d:0>2}", .{ year, month, day, hours, minutes });
    }
    return std.fmt.allocPrint(allocator, "{d:0>4}-{d:0>2}-{d:0>2}", .{ year, month, day });
}

fn prettyTimestamp(allocator: std.mem.Allocator, raw: []const u8) ![]const u8 {
    if (raw.len == 0) return "";
    var s = raw;
    if (std.mem.indexOfScalar(u8, s, '.')) |dot| s = s[0..dot];
    if (std.mem.indexOfScalar(u8, s, 'Z')) |z| s = s[0..z];
    return std.mem.replaceOwned(u8, allocator, s, "T", " ");
}

fn workStateLabel(state: []const u8) []const u8 {
    if (std.mem.eql(u8, state, "PUBLISHED")) return "Publicado";
    if (std.mem.eql(u8, state, "DRAFT")) return "Borrador";
    if (std.mem.eql(u8, state, "DELETED")) return "Eliminado";
    return state;
}

fn pointsLabel(allocator: std.mem.Allocator, work: std.json.Value) ![]const u8 {
    const value = json.get(work, "maxPoints") orelse return "";
    return switch (value) {
        .integer => |n| try std.fmt.allocPrint(allocator, "{d}", .{n}),
        .float => |f| try std.fmt.allocPrint(allocator, "{d:.1}", .{f}),
        else => "",
    };
}

/// Trabajos (tareas, preguntas, materiales) de un curso.
pub fn fetchWork(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
    course_id: []const u8,
    max_results: usize,
) ![]Work {
    if (!validCourseId(course_id)) return &.{};
    const token = (try accessTokenFor(allocator, db, cfg, user_id)) orelse return &.{};
    return workWithToken(allocator, token, cfg.timeout_ms, course_id, max_results);
}

fn workWithToken(
    allocator: std.mem.Allocator,
    token: []const u8,
    timeout_ms: u64,
    course_id: []const u8,
    max_results: usize,
) ![]Work {
    if (!validCourseId(course_id)) return &.{};
    const url = try std.fmt.allocPrint(
        allocator,
        "{s}/courses/{s}/courseWork?pageSize={d}&orderBy=updateTime%20desc&fields=courseWork(id,title,description,state,dueDate,dueTime,maxPoints,alternateLink,workType)",
        .{ API, course_id, max_results },
    );
    const payload = oauth.getJsonPublic(allocator, url, token, timeout_ms) catch return &.{};
    const items = json.get(payload, "courseWork") orelse return &.{};
    if (items != .array) return &.{};
    var out = std.ArrayList(Work).init(allocator);
    for (items.array.items) |w| {
        if (w != .object) continue;
        out.append(.{
            .id = text(w, "id"),
            .title = text(w, "title"),
            .description = text(w, "description"),
            .state = text(w, "state"),
            .due = try dueLabel(allocator, w),
            .max_points = try pointsLabel(allocator, w),
            .link = text(w, "alternateLink"),
            .work_type = text(w, "workType"),
        }) catch {};
    }
    return out.items;
}

/// Entrega próxima de Classroom, ya lista para el planificador.
pub const Upcoming = struct {
    course_id: []const u8,
    course: []const u8,
    title: []const u8,
    due: []const u8,
    link: []const u8,
};

fn upcomingLessThan(_: void, a: Upcoming, b: Upcoming) bool {
    return std.mem.order(u8, a.due, b.due) == .lt;
}

/// Trabajos con fecha igual o posterior a `today_iso` (YYYY-MM-DD),
/// ordenados por entrega. Reutiliza un solo access token para todos los
/// cursos y nunca falla la página: si Google no responde, devuelve vacío.
pub fn upcomingWork(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
    today_iso: []const u8,
    max_courses: usize,
    per_course: usize,
) ![]Upcoming {
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) return &.{};
    if (today_iso.len != 10) return &.{};
    const courses = fetchCourses(allocator, db, cfg, user_id, max_courses) catch return &.{};
    if (!courses.authorized) return &.{};
    const token = (try accessTokenFor(allocator, db, cfg, user_id)) orelse return &.{};
    var out = std.ArrayList(Upcoming).init(allocator);
    for (courses.courses) |c| {
        if (c.id.len == 0) continue;
        const work = workWithToken(allocator, token, cfg.timeout_ms, c.id, per_course) catch continue;
        for (work) |w| {
            if (w.due.len < 10) continue;
            if (std.mem.order(u8, w.due[0..10], today_iso) == .lt) continue;
            out.append(.{
                .course_id = c.id,
                .course = c.name,
                .title = w.title,
                .due = w.due,
                .link = w.link,
            }) catch {};
        }
    }
    std.mem.sort(Upcoming, out.items, {}, upcomingLessThan);
    if (out.items.len > 30) out.shrinkRetainingCapacity(30);
    return out.items;
}

/// Sección de entregas de Classroom para el planificador. Sin datos no
/// imprime nada: la página de Mikareno sigue siendo la principal.
pub fn upcomingHtml(allocator: std.mem.Allocator, items: []const Upcoming) ![]const u8 {
    if (items.len == 0) return "";
    var rows = std.ArrayList(u8).init(allocator);
    for (items) |it| {
        try rows.appendSlice("<tr><td>");
        try rows.appendSlice(try security.htmlEscape(allocator, if (it.due.len == 0) "—" else it.due));
        try rows.appendSlice("</td><td><a class=\"nx-text-link\" href=\"/dashboard/classroom?course=");
        try rows.appendSlice(try security.htmlEscape(allocator, it.course_id));
        try rows.appendSlice("\">");
        try rows.appendSlice(try security.htmlEscape(allocator, if (it.course.len == 0) "Curso" else it.course));
        try rows.appendSlice("</a></td><td>");
        const link = try safeLink(allocator, it.link);
        if (link.len > 0) {
            try rows.appendSlice("<a class=\"nx-text-link\" href=\"");
            try rows.appendSlice(link);
            try rows.appendSlice("\" target=\"_blank\" rel=\"noopener\">");
            try rows.appendSlice(try security.htmlEscape(allocator, if (it.title.len == 0) "Trabajo" else it.title));
            try rows.appendSlice("</a>");
        } else {
            try rows.appendSlice(try security.htmlEscape(allocator, if (it.title.len == 0) "Trabajo" else it.title));
        }
        try rows.appendSlice("</td></tr>");
    }
    return std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-panel nx-classroom-upcoming\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Google Classroom</p><h2>Entregas próximas</h2></div><a class=\"nx-text-link\" href=\"/dashboard/classroom\">Ver cursos<span class=\"material-symbols-outlined\">arrow_forward</span></a></div><div class=\"nx-table-wrap\"><table class=\"nx-table\"><thead><tr><th>Entrega</th><th>Curso</th><th>Trabajo</th></tr></thead><tbody>{s}</tbody></table></div></section>",
        .{rows.items},
    );
}

/// Anuncios recientes de un curso.
pub fn fetchAnnouncements(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
    course_id: []const u8,
    max_results: usize,
) ![]Announcement {
    if (!validCourseId(course_id)) return &.{};
    const token = (try accessTokenFor(allocator, db, cfg, user_id)) orelse return &.{};
    const url = try std.fmt.allocPrint(
        allocator,
        "{s}/courses/{s}/announcements?pageSize={d}&orderBy=updateTime%20desc&fields=announcements(id,text,state,creationTime,updateTime,alternateLink)",
        .{ API, course_id, max_results },
    );
    const payload = oauth.getJsonPublic(allocator, url, token, cfg.timeout_ms) catch return &.{};
    const items = json.get(payload, "announcements") orelse return &.{};
    if (items != .array) return &.{};
    var out = std.ArrayList(Announcement).init(allocator);
    for (items.array.items) |a| {
        if (a != .object) continue;
        const created = if (text(a, "creationTime").len > 0) text(a, "creationTime") else text(a, "updateTime");
        out.append(.{
            .id = text(a, "id"),
            .text = text(a, "text"),
            .created = created,
            .link = text(a, "alternateLink"),
        }) catch {};
    }
    return out.items;
}

/// Detalle de un curso: datos + trabajos + anuncios.
pub fn fetchDetail(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    cfg: oauth.Config,
    user_id: i64,
    course_id: []const u8,
) !Detail {
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) {
        return .{ .reason = "Google OAuth no está configurado en el servidor." };
    }
    if (!validCourseId(course_id)) return .{ .configured = true, .reason = "Curso no válido." };
    const courses = fetchCourses(allocator, db, cfg, user_id, 100) catch CoursesResult{ .configured = true, .reason = "No se pudo consultar Classroom." };
    if (!courses.authorized) return .{ .configured = courses.configured, .reason = courses.reason };
    var found: ?Course = null;
    for (courses.courses) |c| {
        if (std.mem.eql(u8, c.id, course_id)) {
            found = c;
            break;
        }
    }
    if (found == null) return .{ .authorized = true, .configured = true, .reason = "No se encontró ese curso o no tienes acceso." };
    const work = fetchWork(allocator, db, cfg, user_id, course_id, 100) catch &.{};
    const announcements = fetchAnnouncements(allocator, db, cfg, user_id, course_id, 20) catch &.{};
    return .{ .authorized = true, .configured = true, .course = found, .work = work, .announcements = announcements };
}

fn safeLink(allocator: std.mem.Allocator, url: []const u8) ![]const u8 {
    if (!(std.mem.startsWith(u8, url, "https://") or std.mem.startsWith(u8, url, "http://"))) return "";
    return security.htmlEscape(allocator, url);
}

/// Lista de cursos para `/dashboard/classroom`.
pub fn coursesHtml(allocator: std.mem.Allocator, r: CoursesResult) ![]const u8 {
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<section class=\"nx-panel nx-gcal-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Google Classroom</p><h2>Tus cursos</h2></div></div>");
    if (!r.authorized) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">");
        try buf.appendSlice(try security.htmlEscape(allocator, if (r.reason.len > 0) r.reason else "No se pudo consultar Classroom."));
        try buf.appendSlice("</p></section>");
        return buf.items;
    }
    if (r.courses.len == 0) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">No hay cursos activos en Google Classroom.</p></section>");
        return buf.items;
    }
    try buf.appendSlice("<div class=\"nx-classroom-grid\">");
    for (r.courses) |c| {
        const name = try security.htmlEscape(allocator, if (c.name.len > 0) c.name else "Curso sin nombre");
        const meta = try security.htmlEscape(allocator, if (c.section.len > 0) c.section else c.room);
        try buf.appendSlice("<a class=\"nx-classroom-card\" href=\"/dashboard/classroom?course=");
        try buf.appendSlice(try security.htmlEscape(allocator, c.id));
        try buf.appendSlice("\"><span class=\"nx-classroom-card-icon material-symbols-outlined\">school</span><strong>");
        try buf.appendSlice(name);
        try buf.appendSlice("</strong>");
        if (meta.len > 0) {
            try buf.appendSlice("<small>");
            try buf.appendSlice(meta);
            try buf.appendSlice("</small>");
        }
        if (c.description.len > 0) {
            try buf.appendSlice("<p>");
            try buf.appendSlice(try security.htmlEscape(allocator, c.description));
            try buf.appendSlice("</p>");
        }
        try buf.appendSlice("</a>");
    }
    try buf.appendSlice("</div></section>");
    return buf.items;
}

/// Detalle de un curso para `/dashboard/classroom?course=<id>`.
pub fn detailHtml(allocator: std.mem.Allocator, d: Detail) ![]const u8 {
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<section class=\"nx-panel nx-gcal-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Google Classroom</p><h2>");
    if (d.course) |c| {
        try buf.appendSlice(try security.htmlEscape(allocator, if (c.name.len > 0) c.name else "Curso"));
    } else {
        try buf.appendSlice("Curso");
    }
    try buf.appendSlice("</h2></div><a class=\"nx-button nx-button-secondary\" href=\"/dashboard/classroom\">Volver a cursos</a></div>");
    if (!d.authorized) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">");
        try buf.appendSlice(try security.htmlEscape(allocator, if (d.reason.len > 0) d.reason else "No se pudo consultar el curso."));
        try buf.appendSlice("</p></section>");
        return buf.items;
    }
    try buf.appendSlice("<h3 class=\"nx-subhead\">Trabajos</h3>");
    if (d.work.len == 0) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">Este curso no tiene trabajos publicados.</p>");
    } else {
        try buf.appendSlice("<div class=\"nx-table-wrap\"><table class=\"nx-table\"><thead><tr><th>Entrega</th><th>Trabajo</th><th>Estado</th><th>Puntos</th></tr></thead><tbody>");
        for (d.work) |w| {
            try buf.appendSlice("<tr><td>");
            try buf.appendSlice(try security.htmlEscape(allocator, if (w.due.len == 0) "—" else w.due));
            try buf.appendSlice("</td><td>");
            const link = try safeLink(allocator, w.link);
            if (link.len > 0) {
                try buf.appendSlice("<a class=\"nx-text-link\" href=\"");
                try buf.appendSlice(link);
                try buf.appendSlice("\" target=\"_blank\" rel=\"noopener\">");
                try buf.appendSlice(try security.htmlEscape(allocator, if (w.title.len > 0) w.title else "Trabajo"));
                try buf.appendSlice("</a>");
            } else {
                try buf.appendSlice(try security.htmlEscape(allocator, if (w.title.len > 0) w.title else "Trabajo"));
            }
            if (w.description.len > 0) {
                try buf.appendSlice("<small class=\"nx-classroom-desc\">");
                try buf.appendSlice(try security.htmlEscape(allocator, w.description));
                try buf.appendSlice("</small>");
            }
            try buf.appendSlice("</td><td>");
            try buf.appendSlice(try security.htmlEscape(allocator, workStateLabel(if (w.state.len == 0) "—" else w.state)));
            try buf.appendSlice("</td><td>");
            try buf.appendSlice(try security.htmlEscape(allocator, if (w.max_points.len == 0) "—" else w.max_points));
            try buf.appendSlice("</td></tr>");
        }
        try buf.appendSlice("</tbody></table></div>");
    }
    try buf.appendSlice("<h3 class=\"nx-subhead\">Anuncios</h3>");
    if (d.announcements.len == 0) {
        try buf.appendSlice("<p class=\"nx-academic-muted\">Sin anuncios recientes.</p>");
    } else {
        try buf.appendSlice("<div class=\"nx-classroom-announcements\">");
        for (d.announcements) |a| {
            try buf.appendSlice("<article class=\"nx-classroom-announcement\"><span>");
            try buf.appendSlice(try security.htmlEscape(allocator, try prettyTimestamp(allocator, a.created)));
            try buf.appendSlice("</span><p>");
            try buf.appendSlice(try security.htmlEscape(allocator, a.text));
            try buf.appendSlice("</p>");
            const link = try safeLink(allocator, a.link);
            if (link.len > 0) {
                try buf.appendSlice("<a class=\"nx-text-link\" href=\"");
                try buf.appendSlice(link);
                try buf.appendSlice("\" target=\"_blank\" rel=\"noopener\">Abrir en Classroom</a>");
            }
            try buf.appendSlice("</article>");
        }
        try buf.appendSlice("</div>");
    }
    try buf.appendSlice("</section>");
    return buf.items;
}

test "sin client_id Classroom no se ofrece conectar" {
    var db = try sqlite.Db.open(std.testing.allocator, ":memory:");
    defer db.close();
    const cfg = oauth.Config{ .client_id = "", .client_secret = "", .redirect_uri = "", .timeout_ms = 100 };
    const r = try fetchCourses(std.testing.allocator, &db, cfg, 1, 10);
    try std.testing.expect(!r.configured);
    try std.testing.expect(!r.authorized);
}

test "configurado sin token pide conectar en vez de fallar" {
    var db = try sqlite.Db.open(std.testing.allocator, ":memory:");
    defer db.close();
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const cfg = oauth.Config{ .client_id = "id", .client_secret = "secret", .redirect_uri = "http://127.0.0.1:3100/auth/google/callback", .timeout_ms = 100 };
    const r = try fetchCourses(arena.allocator(), &db, cfg, 1, 10);
    try std.testing.expect(r.configured);
    try std.testing.expect(!r.authorized);
}

test "id de curso no numérico se rechaza" {
    try std.testing.expect(!validCourseId("../etc"));
    try std.testing.expect(!validCourseId(""));
    try std.testing.expect(validCourseId("123456789"));
}

test "sin OAuth no hay entregas próximas" {
    var db = try sqlite.Db.open(std.testing.allocator, ":memory:");
    defer db.close();
    const cfg = oauth.Config{ .client_id = "", .client_secret = "", .redirect_uri = "", .timeout_ms = 100 };
    const items = try upcomingWork(std.testing.allocator, &db, cfg, 1, "2026-11-26", 5, 10);
    try std.testing.expectEqual(@as(usize, 0), items.len);
}

test "las entregas próximas se dibujan con enlace al curso" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const items = [_]Upcoming{
        .{ .course_id = "883050322377", .course = "Matemática", .title = "Polinomios", .due = "2026-12-01 23:59", .link = "https://classroom.google.com/c/abc" },
        .{ .course_id = "869701131147", .course = "Biología", .title = "Cáncer", .due = "2026-12-08 04:59", .link = "" },
    };
    const html = try upcomingHtml(a, &items);
    try std.testing.expect(std.mem.indexOf(u8, html, "Entregas próximas") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "/dashboard/classroom?course=883050322377") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "2026-12-08 04:59") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Polinomios") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "classroom.google.com") != null);
}

test "sin entregas no se imprime la sección" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    try std.testing.expectEqualStrings("", try upcomingHtml(arena.allocator(), &.{}));
}
