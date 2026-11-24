//! Vistas del calendario (mes, semana y día) a partir de datos reales:
//! horario, tareas y eventos de Mikareno, anotaciones locales y eventos de
//! Google Calendar cuando el usuario ha autorizado la lectura.
//!
//! Reglas:
//!
//! * **Real o nada**: si una fuente no está disponible, no se inventa nada;
//!   el hueco queda vacío y el estado se explica aparte.
//! * **Una sola vista por ruta**: `/dashboard/calendar`,
//!   `/dashboard/calendar/weekly` y `/dashboard/calendar/daily` comparten el
//!   mismo motor, así que las tres muestran lo mismo con distinto detalle.
//! * **Solo lectura de Mikareno y Google**: las anotaciones son lo único
//!   local y editable (API `/api/calendar/annotations`).

const std = @import("std");
const sqlite = @import("sqlite.zig");
const panel = @import("panel.zig");
const google_calendar = @import("google_calendar.zig");
const security = @import("security.zig");

pub const View = enum { month, week, day };

pub const Cursor = struct { year: i32, month: u32, day: u32 };

pub const Range = struct { from: []const u8, to: []const u8 };

pub const Anotacion = struct {
    id: i64,
    date: []const u8,
    title: []const u8,
    body: []const u8,
    category: []const u8,
    kind: []const u8,
    end_date: []const u8,
    all_day: bool,
    start_time: []const u8,
    end_time: []const u8,
    location: []const u8,
    recurrence: []const u8,
    weekly_day: i64,
};

const MESES = [_][]const u8{
    "enero", "febrero", "marzo",      "abril",   "mayo",      "junio",
    "julio", "agosto",  "septiembre", "octubre", "noviembre", "diciembre",
};

const DIAS_LARGOS = [_][]const u8{
    "domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado",
};

const DIAS_CORTOS = [_][]const u8{ "Dom", "Lun", "Mar", "Mié", "Jue", "Vie", "Sáb" };

// ── Fechas ────────────────────────────────────────────────────────────────

fn daysFromCivil(year: i32, month: u32, day: u32) i64 {
    const y: i64 = if (month <= 2) year - 1 else year;
    const era = @divFloor(if (y >= 0) y else y - 399, 400);
    const yoe: i64 = y - era * 400;
    const m: i64 = @intCast(month);
    const d: i64 = @intCast(day);
    const doy = @divFloor(153 * (if (m > 2) m - 3 else m + 9) + 2, 5) + d - 1;
    const doe = yoe * 365 + @divFloor(yoe, 4) - @divFloor(yoe, 100) + doy;
    return era * 146097 + doe - 719468;
}

const CivilDate = struct { year: i32, month: u32, day: u32 };

fn civilFromDays(days: i64) CivilDate {
    const z = days + 719468;
    const era = @divFloor(if (z >= 0) z else z - 146096, 146097);
    const doe: i64 = z - era * 146097;
    const yoe = @divFloor(doe - @divFloor(doe, 1460) + @divFloor(doe, 36524) - @divFloor(doe, 146096), 365);
    const y = yoe + era * 400;
    const doy = doe - (365 * yoe + @divFloor(yoe, 4) - @divFloor(yoe, 100));
    const mp = @divFloor(5 * doy + 2, 153);
    const d = doy - @divFloor(153 * mp + 2, 5) + 1;
    const m = if (mp < 10) mp + 3 else mp - 9;
    const year: i32 = @intCast(if (m <= 2) y + 1 else y);
    return .{ .year = year, .month = @intCast(m), .day = @intCast(d) };
}

fn isoOf(allocator: std.mem.Allocator, year: i32, month: u32, day: u32) ![]const u8 {
    return std.fmt.allocPrint(allocator, "{d:0>4}-{d:0>2}-{d:0>2}", .{ @as(u32, @intCast(year)), month, day });
}

fn parseIso(iso: []const u8) ?CivilDate {
    if (iso.len < 10) return null;
    const year = std.fmt.parseInt(i32, iso[0..4], 10) catch return null;
    const month = std.fmt.parseInt(u32, iso[5..7], 10) catch return null;
    const day = std.fmt.parseInt(u32, iso[8..10], 10) catch return null;
    if (month < 1 or month > 12 or day < 1 or day > 31) return null;
    return .{ .year = year, .month = month, .day = day };
}

pub fn shiftDays(allocator: std.mem.Allocator, iso: []const u8, delta: i64) ![]const u8 {
    const civil = parseIso(iso) orelse return allocator.dupe(u8, iso);
    const days = daysFromCivil(civil.year, civil.month, civil.day) + delta;
    const next = civilFromDays(days);
    return isoOf(allocator, next.year, next.month, next.day);
}

/// "2026-11-24" → "martes, 24 de noviembre de 2026".
pub fn longDateEs(allocator: std.mem.Allocator, iso: []const u8) ![]const u8 {
    const c = parseIso(iso) orelse return allocator.dupe(u8, iso);
    const wd = panel.weekdayOf(iso) orelse "";
    const dow = DIAS_LARGOS[(weekdayIdx(wd) + 1) % 7];
    return std.fmt.allocPrint(allocator, "{s}, {d} de {s} de {d}", .{ dow, c.day, MESES[c.month - 1], c.year });
}

fn daysInMonth(year: i32, month: u32) u32 {
    const bisiesto = @mod(year, 4) == 0 and (@mod(year, 100) != 0 or @mod(year, 400) == 0);
    return switch (month) {
        1, 3, 5, 7, 8, 10, 12 => 31,
        4, 6, 9, 11 => 30,
        2 => if (bisiesto) 29 else 28,
        else => 30,
    };
}

/// Índice de día de la semana con lunes = 0 (domingo = 6).
fn weekdayIdx(name: []const u8) usize {
    const orden = [_][]const u8{ "domingo", "lunes", "martes", "miercoles", "jueves", "viernes", "sabado" };
    for (orden, 0..) |d, i| if (std.mem.eql(u8, d, name)) return (i + 6) % 7;
    return 0;
}

fn startOfWeek(allocator: std.mem.Allocator, iso: []const u8) ![]const u8 {
    const wd = panel.weekdayOf(iso) orelse return allocator.dupe(u8, iso);
    return shiftDays(allocator, iso, -@as(i64, @intCast(weekdayIdx(wd))));
}

fn monthShift(year: i32, month: u32, delta: i32) struct { year: i32, month: u32 } {
    var m: i32 = @as(i32, @intCast(month)) + delta;
    var y = year;
    while (m < 1) : (m += 12) y -= 1;
    while (m > 12) : (m -= 12) y += 1;
    return .{ .year = y, .month = @intCast(m) };
}

pub fn cursorFromIso(allocator: std.mem.Allocator, iso: []const u8) Cursor {
    _ = allocator;
    const civil = parseIso(iso) orelse CivilDate{ .year = 2026, .month = 1, .day = 1 };
    return .{ .year = civil.year, .month = civil.month, .day = civil.day };
}

/// Ajusta un cursor con mes o día fuera de rango (por ejemplo `?m=2&d=31`).
pub fn clampCursor(cur: Cursor) Cursor {
    var out = cur;
    if (out.month < 1) out.month = 1;
    if (out.month > 12) out.month = 12;
    if (out.year < 1970) out.year = 1970;
    if (out.year > 3000) out.year = 3000;
    const max_day = daysInMonth(out.year, out.month);
    if (out.day < 1) out.day = 1;
    if (out.day > max_day) out.day = max_day;
    return out;
}

pub fn rangeFor(allocator: std.mem.Allocator, view: View, cur: Cursor) !Range {
    switch (view) {
        .month => {
            const from = try isoOf(allocator, cur.year, cur.month, 1);
            const to = try isoOf(allocator, cur.year, cur.month, daysInMonth(cur.year, cur.month));
            return .{ .from = from, .to = to };
        },
        .week => {
            const cursor = try isoOf(allocator, cur.year, cur.month, cur.day);
            const from = try startOfWeek(allocator, cursor);
            const to = try shiftDays(allocator, from, 6);
            return .{ .from = from, .to = to };
        },
        .day => {
            const one = try isoOf(allocator, cur.year, cur.month, cur.day);
            return .{ .from = one, .to = one };
        },
    }
}

// ── Datos locales ─────────────────────────────────────────────────────────

pub fn loadAnnotations(allocator: std.mem.Allocator, db: *sqlite.Db, from: []const u8, to: []const u8) ![]Anotacion {
    var stmt = db.prepare(
        "SELECT id,date,title,body,COALESCE(category,''),COALESCE(kind,''),COALESCE(end_date,''),all_day,COALESCE(start_time,''),COALESCE(end_time,''),COALESCE(location,''),COALESCE(recurrence,''),COALESCE(weekly_day,-1) " ++
            "FROM calendar_annotations WHERE (date>=?1 AND date<=?2) " ++
            "OR (kind='event' AND date<?1 AND ((all_day=1 AND COALESCE(end_date,date)>?1) OR (all_day=0 AND COALESCE(end_date,date)>=?1))) " ++
            "OR (kind='event' AND recurrence='weekly') ORDER BY date,id",
    ) catch return &.{};
    defer stmt.deinit();
    try stmt.bindText(1, from);
    try stmt.bindText(2, to);
    var out = std.ArrayList(Anotacion).init(allocator);
    while (try stmt.next()) {
        try out.append(.{
            .id = stmt.int64(0),
            .date = try allocator.dupe(u8, stmt.text(1)),
            .title = try allocator.dupe(u8, stmt.text(2)),
            .body = try allocator.dupe(u8, stmt.text(3)),
            .category = try allocator.dupe(u8, stmt.text(4)),
            .kind = try allocator.dupe(u8, stmt.text(5)),
            .end_date = try allocator.dupe(u8, stmt.text(6)),
            .all_day = stmt.int64(7) != 0,
            .start_time = try allocator.dupe(u8, stmt.text(8)),
            .end_time = try allocator.dupe(u8, stmt.text(9)),
            .location = try allocator.dupe(u8, stmt.text(10)),
            .recurrence = try allocator.dupe(u8, stmt.text(11)),
            .weekly_day = stmt.int64(12),
        });
    }
    return out.items;
}

fn datePart(value: []const u8) []const u8 {
    return if (value.len >= 10) value[0..10] else value;
}

fn annotationOn(a: Anotacion, iso: []const u8) bool {
    if (a.date.len < 10) return false;
    const start = a.date[0..10];
    if (std.mem.eql(u8, a.recurrence, "weekly")) {
        if (std.mem.order(u8, iso, start) == .lt) return false;
        if (a.end_date.len >= 10 and std.mem.order(u8, iso, a.end_date[0..10]) == .gt) return false;
        const idx = weekdayIdx(panel.weekdayOf(iso) orelse return false);
        if (a.weekly_day >= 0) return a.weekly_day == @as(i64, @intCast(idx));
        return weekdayIdx(panel.weekdayOf(start) orelse "") == idx;
    }
    if (std.mem.eql(u8, start, iso)) return true;
    if (std.mem.eql(u8, a.kind, "event") and a.end_date.len >= 10) {
        return std.mem.order(u8, start, iso) != .gt and std.mem.order(u8, iso, a.end_date[0..10]) == .lt;
    }
    return false;
}

fn annotationTime(a: Anotacion) []const u8 {
    if (a.all_day or a.start_time.len == 0) return "";
    return a.start_time;
}

// ── Piezas comunes de vista ───────────────────────────────────────────────

const Chip = struct { tone: []const u8, text: []const u8, title: []const u8 };

fn mesaChip(tone: []const u8, text: []const u8) Chip {
    return .{ .tone = tone, .text = text, .title = text };
}

/// Evento genérico que Mikareno repite cada día lectivo ("Clases"). No aporta
/// nada frente a la lista real de clases y llenaba "Próximos eventos".
fn isGenericClassEvent(titulo: []const u8) bool {
    return std.ascii.eqlIgnoreCase(titulo, "clases") or std.ascii.eqlIgnoreCase(titulo, "clase");
}

/// Chip compacto de una clase: en el mes solo interesa la materia, con la
/// hora completa en el tooltip.
fn classChip(etiqueta: []const u8) Chip {
    if (std.mem.indexOf(u8, etiqueta, " — ")) |pos| {
        // " — " son 5 bytes (espacio + guion largo UTF-8 + espacio).
        return .{ .tone = "class", .text = etiqueta[pos + " — ".len ..], .title = etiqueta };
    }
    return .{ .tone = "class", .text = etiqueta, .title = etiqueta };
}

fn chipsForDate(
    allocator: std.mem.Allocator,
    d: panel.Datos,
    anots: []const Anotacion,
    google: google_calendar.Resultado,
    iso: []const u8,
) ![]Chip {
    var list = std.ArrayList(Chip).init(allocator);
    // Primero lo que no es rutina (entregas, eventos, notas) para que no lo
    // tape el "+N más" cuando la celda recorta.
    for (d.tareas) |t| {
        if (!std.mem.eql(u8, datePart(t.vencimiento), iso)) continue;
        try list.append(mesaChip("task", t.titulo));
    }
    for (d.eventos) |e| {
        if (!std.mem.eql(u8, datePart(e.fecha), iso)) continue;
        if (isGenericClassEvent(e.titulo)) continue;
        const tone = if (std.mem.eql(u8, e.tipo, "NON_SCHOOL_DAY")) "event non-school" else "event";
        try list.append(mesaChip(tone, e.titulo));
    }
    for (anots) |a| {
        if (!annotationOn(a, iso)) continue;
        try list.append(mesaChip("note", if (a.title.len == 0) "Anotación" else a.title));
    }
    if (google.authorized) {
        for (google.events) |ev| {
            if (!std.mem.eql(u8, datePart(ev.start), iso)) continue;
            try list.append(mesaChip("gcal", ev.title));
        }
    }
    if (panel.weekdayOf(iso)) |wd| {
        for (d.horario) |c| {
            if (!std.mem.eql(u8, c.dia, wd)) continue;
            try list.append(classChip(c.etiqueta));
        }
    }
    return list.items;
}

fn appendChips(allocator: std.mem.Allocator, body: *std.ArrayList(u8), chips: []const Chip, max_shown: usize) !void {
    if (chips.len == 0) return;
    try body.appendSlice("<div class=\"nx-cal-chips\">");
    const shown = @min(chips.len, max_shown);
    for (chips[0..shown]) |c| {
        try body.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<span class=\"nx-cal-chip {s}\" title=\"{s}\">{s}</span>",
            .{ c.tone, try security.htmlEscape(allocator, c.title), try security.htmlEscape(allocator, c.text) },
        ));
    }
    if (chips.len > shown) {
        try body.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<span class=\"nx-cal-chip is-more\">+{d} más</span>",
            .{chips.len - shown},
        ));
    }
    try body.appendSlice("</div>");
}

fn viewHref(allocator: std.mem.Allocator, view: View, cur: Cursor) ![]const u8 {
    const base = switch (view) {
        .month => "/dashboard/calendar",
        .week => "/dashboard/calendar/weekly",
        .day => "/dashboard/calendar/daily",
    };
    return std.fmt.allocPrint(allocator, "{s}?y={d}&m={d}&d={d}", .{ base, cur.year, cur.month, cur.day });
}

fn appendHeader(allocator: std.mem.Allocator, body: *std.ArrayList(u8), view: View, cur: Cursor) !void {
    try body.appendSlice("<header class=\"nx-cal-head\"><div class=\"nx-cal-head-main\"><p class=\"nx-cal-eyebrow\">Agenda académica</p><h1>Calendario</h1><p class=\"nx-cal-sub\">Clases, entregas, exámenes y anotaciones en un solo lugar</p></div><div class=\"nx-cal-head-actions\"><nav class=\"nx-cal-tabs\" aria-label=\"Vista del calendario\">");
    const labels = [_]struct { view: View, label: []const u8 }{
        .{ .view = .month, .label = "Mes" },
        .{ .view = .week, .label = "Semana" },
        .{ .view = .day, .label = "Día" },
    };
    for (labels) |item| {
        const active = item.view == view;
        try body.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<a href=\"{s}\"{s}>{s}</a>",
            .{ try viewHref(allocator, item.view, cur), if (active) " aria-current=\"page\"" else "", item.label },
        ));
    }
    try body.appendSlice("</nav><a class=\"nx-cal-export\" href=\"/dashboard/calendar.ics\" download title=\"Descargar calendario ICS\"><span class=\"material-symbols-outlined\">download</span><span>Exportar</span></a></div></header>");
}

fn appendLegend(body: *std.ArrayList(u8)) !void {
    try body.appendSlice("<div class=\"nx-cal-legend\" aria-label=\"Tipos de elemento\"><span class=\"is-class\"><i></i>Horario</span><span class=\"is-task\"><i></i>Entregas</span><span class=\"is-event\"><i></i>Eventos</span><span class=\"is-note\"><i></i>Anotaciones</span><span class=\"nx-cal-legend-gcal\"><i></i>Google</span></div>");
}

const UpItem = struct { date: []const u8, title: []const u8, tone: []const u8, badge: []const u8 };

fn upLess(_: void, a: UpItem, b: UpItem) bool {
    const o = std.mem.order(u8, a.date, b.date);
    if (o != .eq) return o == .lt;
    return std.mem.lessThan(u8, a.title, b.title);
}

fn appendUpcoming(
    allocator: std.mem.Allocator,
    body: *std.ArrayList(u8),
    d: panel.Datos,
    anots: []const Anotacion,
    google: google_calendar.Resultado,
    from_iso: []const u8,
) !void {
    var items = std.ArrayList(UpItem).init(allocator);
    for (d.eventos) |e| {
        if (std.mem.order(u8, datePart(e.fecha), from_iso) == .lt) continue;
        if (isGenericClassEvent(e.titulo)) continue;
        const non_school = std.mem.eql(u8, e.tipo, "NON_SCHOOL_DAY");
        try items.append(.{
            .date = datePart(e.fecha),
            .title = e.titulo,
            .tone = if (non_school) "non-school" else "event",
            .badge = if (non_school) "No lectivo" else "Evento",
        });
    }
    for (d.tareas) |t| {
        if (std.mem.order(u8, datePart(t.vencimiento), from_iso) == .lt) continue;
        try items.append(.{ .date = datePart(t.vencimiento), .title = t.titulo, .tone = "task", .badge = "Entrega" });
    }
    for (anots) |a| {
        if (a.date.len < 10) continue;
        if (std.mem.order(u8, a.date[0..10], from_iso) == .lt) continue;
        try items.append(.{ .date = a.date[0..10], .title = if (a.title.len == 0) "Anotación" else a.title, .tone = "note", .badge = "Nota" });
    }
    if (google.authorized) {
        for (google.events) |ev| {
            const day = datePart(ev.start);
            if (std.mem.order(u8, day, from_iso) == .lt) continue;
            try items.append(.{ .date = day, .title = ev.title, .tone = "gcal", .badge = "Google" });
        }
    }
    std.mem.sort(UpItem, items.items, {}, upLess);

    try body.appendSlice("<aside class=\"nx-cal-upcoming\" aria-label=\"Próximos eventos\"><header class=\"nx-cal-up-head\"><div><p>A continuación</p><h2>Próximos eventos</h2></div><span class=\"material-symbols-outlined\">upcoming</span></header>");
    if (items.items.len == 0) {
        try body.appendSlice("<p class=\"nx-cal-up-empty\">Sin eventos próximos.</p>");
    } else {
        var shown: usize = 0;
        for (items.items) |item| {
            if (shown >= 6) break;
            shown += 1;
            try body.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<div class=\"nx-cal-up-item\"><span class=\"nx-cal-up-date\">{s}</span><span class=\"nx-cal-up-title\">{s}</span><span class=\"nx-cal-up-badge {s}\">{s}</span></div>",
                .{ if (item.date.len >= 10) item.date[8..10] else item.date, try security.htmlEscape(allocator, item.title), item.tone, item.badge },
            ));
        }
    }
    try body.appendSlice("<a class=\"nx-cal-up-link\" href=\"/dashboard/calendar/weekly\">Ver la semana completa<span class=\"material-symbols-outlined\">arrow_forward</span></a></aside>");
}

// ── Vista de mes ──────────────────────────────────────────────────────────

fn appendMonth(
    allocator: std.mem.Allocator,
    body: *std.ArrayList(u8),
    d: panel.Datos,
    anots: []const Anotacion,
    google: google_calendar.Resultado,
    cur: Cursor,
    today: []const u8,
) !void {
    const total = daysInMonth(cur.year, cur.month);
    const first_iso = try isoOf(allocator, cur.year, cur.month, 1);
    const offset = weekdayIdx(panel.weekdayOf(first_iso) orelse "lunes");
    const last_iso = try isoOf(allocator, cur.year, cur.month, total);

    var days_with_class: usize = 0;
    var tasks_month: usize = 0;
    var events_month: usize = 0;
    var day: u32 = 1;
    while (day <= total) : (day += 1) {
        const iso = try isoOf(allocator, cur.year, cur.month, day);
        const wd = panel.weekdayOf(iso) orelse "";
        var has_class = false;
        for (d.horario) |c| {
            if (std.mem.eql(u8, c.dia, wd)) {
                has_class = true;
                break;
            }
        }
        if (has_class) days_with_class += 1;
    }
    for (d.tareas) |t| {
        const v = datePart(t.vencimiento);
        if (std.mem.order(u8, v, first_iso) != .lt and std.mem.order(u8, v, last_iso) != .gt) tasks_month += 1;
    }
    for (d.eventos) |e| {
        if (isGenericClassEvent(e.titulo)) continue;
        const v = datePart(e.fecha);
        if (std.mem.order(u8, v, first_iso) != .lt and std.mem.order(u8, v, last_iso) != .gt) events_month += 1;
    }
    for (anots) |a| {
        if (!annotationOn(a, first_iso) and !annotationOn(a, last_iso)) continue;
        if (a.date.len >= 10 and std.mem.order(u8, a.date[0..10], first_iso) != .lt and std.mem.order(u8, a.date[0..10], last_iso) != .gt) events_month += 1;
    }
    if (google.authorized) {
        for (google.events) |ev| {
            const v = datePart(ev.start);
            if (std.mem.order(u8, v, first_iso) != .lt and std.mem.order(u8, v, last_iso) != .gt) events_month += 1;
        }
    }

    try body.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-cal-kpis\" aria-label=\"Resumen del mes\"><span class=\"nx-cal-kpi\"><strong>{d}</strong><small>días con horario</small></span><span class=\"nx-cal-kpi\"><strong>{d}</strong><small>entregas</small></span><span class=\"nx-cal-kpi\"><strong>{d}</strong><small>eventos</small></span></div>",
        .{ days_with_class, tasks_month, events_month },
    ));

    const prev = monthShift(cur.year, cur.month, -1);
    const next = monthShift(cur.year, cur.month, 1);
    const prev_iso = try isoOf(allocator, prev.year, prev.month, 1);
    const next_iso = try isoOf(allocator, next.year, next.month, 1);
    const prev_cur = cursorFromIso(allocator, prev_iso);
    const next_cur = cursorFromIso(allocator, next_iso);

    try body.appendSlice("<div class=\"agenda-layout\"><section class=\"nx-card nx-cal-month\" aria-label=\"Calendario mensual\"><header class=\"nx-cal-month-head\">");
    try appendLegend(body);
    try body.appendSlice("<a class=\"nx-cal-today\" href=\"/dashboard/calendar\"><span class=\"material-symbols-outlined\">today</span>Hoy</a></header><div class=\"nx-cal-grid\">");

    try body.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-cal-nav\"><a class=\"nx-cal-nav-btn\" href=\"{s}\" aria-label=\"Mes anterior\"><span class=\"material-symbols-outlined\">chevron_left</span></a><div class=\"nx-cal-title\"><span class=\"nx-cal-month\">{s} {d}</span><span class=\"nx-cal-badge\">{s}</span></div><a class=\"nx-cal-nav-btn\" href=\"{s}\" aria-label=\"Mes siguiente\"><span class=\"material-symbols-outlined\">chevron_right</span></a></div>",
        .{
            try viewHref(allocator, .month, prev_cur),
            MESES[@intCast(cur.month - 1)],
            cur.year,
            if (std.mem.eql(u8, first_iso[0..7], today[0..7])) "Mes actual" else "Otro mes",
            try viewHref(allocator, .month, next_cur),
        },
    ));

    try body.appendSlice("<div class=\"nx-cal-weekdays\">");
    for ([_][]const u8{ "L", "M", "X", "J", "V", "S", "D" }) |label| {
        try body.appendSlice(try std.fmt.allocPrint(allocator, "<div>{s}</div>", .{label}));
    }
    try body.appendSlice("</div><div class=\"nx-cal-days\">");
    for (0..offset) |_| try body.appendSlice("<div class=\"nx-cal-day nx-cal-outside\"></div>");

    day = 1;
    while (day <= total) : (day += 1) {
        const iso = try isoOf(allocator, cur.year, cur.month, day);
        const is_today = std.mem.eql(u8, iso, today);
        try body.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<div class=\"nx-cal-day{s}\"><a class=\"nx-cal-day-num\" href=\"{s}\"{s} title=\"Ver el día\">{d}</a>",
            .{
                if (is_today) " nx-cal-today" else "",
                try viewHref(allocator, .day, cursorFromIso(allocator, iso)),
                if (is_today) " aria-current=\"date\"" else "",
                day,
            },
        ));
        const chips = try chipsForDate(allocator, d, anots, google, iso);
        // En pantallas estrechas la celda no cabe texto: se muestra un
        // contador (naranja si hay entregas) y los chips quedan ocultos.
        if (chips.len > 0) {
            var has_task = false;
            for (chips) |c| {
                if (std.mem.eql(u8, c.tone, "task")) has_task = true;
            }
            try body.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<span class=\"nx-cal-count{s}\" aria-hidden=\"true\">{d}</span>",
                .{ if (has_task) " has-task" else "", chips.len },
            ));
        }
        try appendChips(allocator, body, chips, 3);
        try body.appendSlice("</div>");
    }
    const used = offset + total;
    const trailing = if (used < 42) 42 - used else 0;
    for (0..trailing) |_| try body.appendSlice("<div class=\"nx-cal-day nx-cal-outside\"></div>");
    try body.appendSlice("</div></div>");

    try body.appendSlice("<footer class=\"nx-cal-foot\"><span><span class=\"material-symbols-outlined\">edit_calendar</span>Abre un día para ver el detalle y añadir anotaciones.</span><a href=\"/dashboard/horario\">Abrir mi horario<span class=\"material-symbols-outlined\">arrow_outward</span></a></footer></section>");

    const from_iso = try isoOf(allocator, cur.year, cur.month, 1);
    try appendUpcoming(allocator, body, d, anots, google, if (std.mem.order(u8, today, from_iso) == .gt) today else from_iso);
    try body.appendSlice("</div>");
}

// ── Vista de semana ───────────────────────────────────────────────────────

fn appendWeek(
    allocator: std.mem.Allocator,
    body: *std.ArrayList(u8),
    d: panel.Datos,
    anots: []const Anotacion,
    google: google_calendar.Resultado,
    cur: Cursor,
    today: []const u8,
) !void {
    const cursor = try isoOf(allocator, cur.year, cur.month, cur.day);
    const start = try startOfWeek(allocator, cursor);
    const end = try shiftDays(allocator, start, 6);

    const prev = try shiftDays(allocator, start, -7);
    const next = try shiftDays(allocator, start, 7);
    const prev_cur = cursorFromIso(allocator, prev);
    const next_cur = cursorFromIso(allocator, next);
    const start_civil = parseIso(start) orelse CivilDate{ .year = cur.year, .month = cur.month, .day = cur.day };
    const end_civil = parseIso(end) orelse CivilDate{ .year = cur.year, .month = cur.month, .day = cur.day };

    try body.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-cal-nav\"><a class=\"nx-cal-nav-btn\" href=\"{s}\" aria-label=\"Semana anterior\"><span class=\"material-symbols-outlined\">chevron_left</span></a><div class=\"nx-cal-title\"><span class=\"nx-cal-month\">{d} – {d} {s} {d}</span><span class=\"nx-cal-badge\">Semana</span></div><a class=\"nx-cal-nav-btn\" href=\"{s}\" aria-label=\"Semana siguiente\"><span class=\"material-symbols-outlined\">chevron_right</span></a></div>",
        .{
            try viewHref(allocator, .week, prev_cur),
            start_civil.day,
            end_civil.day,
            MESES[@intCast(end_civil.month - 1)],
            end_civil.year,
            try viewHref(allocator, .week, next_cur),
        },
    ));

    try body.appendSlice("<div class=\"agenda-layout\"><section class=\"nx-card nx-cal-week\" aria-label=\"Calendario semanal\"><div class=\"nx-cal-week-grid\">");
    var i: usize = 0;
    while (i < 7) : (i += 1) {
        const iso = try shiftDays(allocator, start, @intCast(i));
        const civil = parseIso(iso) orelse CivilDate{ .year = cur.year, .month = cur.month, .day = cur.day };
        const is_today = std.mem.eql(u8, iso, today);
        const wd = panel.weekdayOf(iso) orelse "";
        const short = DIAS_CORTOS[(@as(usize, @intCast(weekdayIdx(wd))) + 1) % 7];
        try body.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<div class=\"nx-cal-week-col{s}\"><header class=\"nx-cal-week-head\"><span>{s}</span><strong>{d}</strong></header>",
            .{ if (is_today) " is-today" else "", short, civil.day },
        ));
        const chips = try chipsForDate(allocator, d, anots, google, iso);
        if (chips.len == 0) {
            try body.appendSlice("<p class=\"nx-cal-week-empty\">Sin datos</p>");
        } else {
            try appendChips(allocator, body, chips, 4);
        }
        try body.appendSlice("</div>");
    }
    try body.appendSlice("</div></section>");
    try appendUpcoming(allocator, body, d, anots, google, if (std.mem.order(u8, today, start) == .gt) today else start);
    try body.appendSlice("</div>");
}

// ── Vista de día ──────────────────────────────────────────────────────────

fn appendRow(allocator: std.mem.Allocator, body: *std.ArrayList(u8), time: []const u8, title: []const u8, detail: []const u8, tone: []const u8) !void {
    try body.appendSlice("<div class=\"nx-cal-row\">");
    try body.appendSlice(try std.fmt.allocPrint(allocator, "<span class=\"nx-cal-row-time\">{s}</span>", .{if (time.len == 0) "—" else try security.htmlEscape(allocator, time)}));
    try body.appendSlice("<div class=\"nx-cal-row-main\">");
    try body.appendSlice(try std.fmt.allocPrint(allocator, "<strong>{s}</strong>", .{try security.htmlEscape(allocator, title)}));
    if (detail.len > 0) try body.appendSlice(try std.fmt.allocPrint(allocator, "<p>{s}</p>", .{try security.htmlEscape(allocator, detail)}));
    try body.appendSlice("</div>");
    try body.appendSlice(try std.fmt.allocPrint(allocator, "<span class=\"nx-cal-chip {s}\">{s}</span>", .{ tone, switch (tone[0]) {
        'c' => "Horario",
        't' => "Entrega",
        'e' => "Evento",
        'n' => "Nota",
        else => "Google",
    } }));
    try body.appendSlice("</div>");
}

fn appendDay(
    allocator: std.mem.Allocator,
    body: *std.ArrayList(u8),
    d: panel.Datos,
    anots: []const Anotacion,
    google: google_calendar.Resultado,
    cur: Cursor,
    today: []const u8,
) !void {
    const iso = try isoOf(allocator, cur.year, cur.month, cur.day);
    const wd = panel.weekdayOf(iso) orelse "";
    const prev_cur = cursorFromIso(allocator, try shiftDays(allocator, iso, -1));
    const next_cur = cursorFromIso(allocator, try shiftDays(allocator, iso, 1));
    const long_day = if (weekdayIdx(wd) < 7) DIAS_LARGOS[(@as(usize, @intCast(weekdayIdx(wd))) + 1) % 7] else wd;
    const is_today = std.mem.eql(u8, iso, today);

    try body.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-cal-nav\"><a class=\"nx-cal-nav-btn\" href=\"{s}\" aria-label=\"Día anterior\"><span class=\"material-symbols-outlined\">chevron_left</span></a><div class=\"nx-cal-title\"><span class=\"nx-cal-month\">{s} {d} de {s} {d}</span><span class=\"nx-cal-badge\">{s}</span></div><a class=\"nx-cal-nav-btn\" href=\"{s}\" aria-label=\"Día siguiente\"><span class=\"material-symbols-outlined\">chevron_right</span></a></div>",
        .{
            try viewHref(allocator, .day, prev_cur),
            long_day,
            cur.day,
            MESES[@intCast(cur.month - 1)],
            cur.year,
            if (is_today) "Hoy" else "Día",
            try viewHref(allocator, .day, next_cur),
        },
    ));

    try body.appendSlice("<div class=\"agenda-layout\"><section class=\"nx-card nx-cal-day-view\" aria-label=\"Detalle del día\"><div class=\"nx-cal-day-list\">");
    var rows: usize = 0;

    for (d.horario) |c| {
        if (!std.mem.eql(u8, c.dia, wd)) continue;
        const time = if (std.mem.indexOf(u8, c.etiqueta, " — ")) |pos| c.etiqueta[0..pos] else "";
        const name = if (std.mem.indexOf(u8, c.etiqueta, " — ")) |pos| c.etiqueta[pos + " — ".len ..] else c.etiqueta;
        try appendRow(allocator, body, time, name, "", "class");
        rows += 1;
    }
    for (d.eventos) |e| {
        if (!std.mem.eql(u8, datePart(e.fecha), iso)) continue;
        if (isGenericClassEvent(e.titulo)) continue;
        const tone = if (std.mem.eql(u8, e.tipo, "NON_SCHOOL_DAY")) "event non-school" else "event";
        try appendRow(allocator, body, "", e.titulo, if (std.mem.eql(u8, e.tipo, "NON_SCHOOL_DAY")) "Día no lectivo" else "Evento del colegio", tone);
        rows += 1;
    }
    for (d.tareas) |t| {
        if (!std.mem.eql(u8, datePart(t.vencimiento), iso)) continue;
        const time = if (t.vencimiento.len > 11) t.vencimiento[11..@min(t.vencimiento.len, 16)] else "";
        try appendRow(allocator, body, time, t.titulo, "Entrega de tarea o actividad", "task");
        rows += 1;
    }
    for (anots) |a| {
        if (!annotationOn(a, iso)) continue;
        try appendRow(allocator, body, annotationTime(a), if (a.title.len == 0) "Anotación" else a.title, a.body, "note");
        rows += 1;
    }
    if (google.authorized) {
        for (google.events) |ev| {
            if (!std.mem.eql(u8, datePart(ev.start), iso)) continue;
            const time = if (ev.all_day or ev.start.len < 16) "" else ev.start[11..16];
            try appendRow(allocator, body, time, ev.title, ev.location, "gcal");
            rows += 1;
        }
    }
    if (rows == 0) {
        try body.appendSlice("<p class=\"nx-academic-muted\">Sin clases, entregas ni eventos para este día.</p>");
    }
    try body.appendSlice("</div></section>");

    // Formulario de anotación rápida: escribe en la API local, no en Mikareno.
    try body.appendSlice("<aside class=\"nx-panel nx-cal-add\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Anotaciones</p><h2>Añadir a este día</h2></div></div>");
    try body.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<form id=\"nx-cal-add-form\" data-date=\"{s}\"><input name=\"title\" maxlength=\"120\" required placeholder=\"Título (examen, recordatorio…)\" aria-label=\"Título de la anotación\"><input name=\"body\" maxlength=\"2000\" placeholder=\"Detalle (opcional)\" aria-label=\"Detalle de la anotación\"><button class=\"nx-button\" type=\"submit\">Guardar</button></form><p id=\"nx-cal-add-status\" class=\"nx-academic-muted\" role=\"status\"></p>",
        .{iso},
    ));
    if (google.authorized) {
        try body.appendSlice("<p class=\"nx-academic-muted nx-cal-gcal-note\">Google Calendar también se muestra aquí, en solo lectura.</p>");
    } else if (google.reason.len > 0) {
        try body.appendSlice(try std.fmt.allocPrint(allocator, "<p class=\"nx-academic-muted nx-cal-gcal-note\">{s}</p>", .{try security.htmlEscape(allocator, google.reason)}));
    }
    try body.appendSlice("</aside></div>");

    try body.appendSlice(
        "<script>(function(){var f=document.getElementById('nx-cal-add-form');if(!f)return;f.addEventListener('submit',function(e){e.preventDefault();" ++
            "var t=f.querySelector('input[name=title]'),b=f.querySelector('input[name=body]'),s=document.getElementById('nx-cal-add-status');" ++
            "var title=(t&&t.value||'').trim();if(!title){if(s)s.textContent='Escribe un título.';return;}" ++
            "fetch('/api/calendar/annotations',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({date:f.getAttribute('data-date'),title:title,body:(b&&b.value||'')})})" ++
            ".then(function(r){if(!r.ok)throw new Error('HTTP '+r.status);return r.json();})" ++
            ".then(function(){if(s)s.textContent='Guardado. Recargando…';location.reload();})" ++
            ".catch(function(err){if(s)s.textContent='No se pudo guardar: '+err.message;});});})();</script>",
    );
}

// ── Entrada pública ───────────────────────────────────────────────────────

pub fn render(
    allocator: std.mem.Allocator,
    d: panel.Datos,
    anots: []const Anotacion,
    google: google_calendar.Resultado,
    view: View,
    cur: Cursor,
    today: []const u8,
) ![]const u8 {
    var body = std.ArrayList(u8).init(allocator);
    try body.appendSlice("<div class=\"nx-page nova-page nx-calendar-page\">");
    try appendHeader(allocator, &body, view, cur);
    if (!d.connected or d.offline) {
        try body.appendSlice(try panel.syncBanner(allocator, d));
    }
    // Google está configurado pero nadie ha dado permiso: se ofrece conectar
    // aquí mismo (solo lectura) en vez de dejar una nota pasiva.
    if (google.configured and !google.authorized) {
        try body.appendSlice("<div class=\"nx-sync-banner is-cached\" role=\"status\"><span class=\"material-symbols-outlined\">event</span><p>Conecta tu Google Calendar para ver tus eventos aquí, en solo lectura.</p><a href=\"/auth/google\">Conectar Google<span class=\"material-symbols-outlined\">arrow_forward</span></a></div>");
    }
    switch (view) {
        .month => try appendMonth(allocator, &body, d, anots, google, cur, today),
        .week => try appendWeek(allocator, &body, d, anots, google, cur, today),
        .day => try appendDay(allocator, &body, d, anots, google, cur, today),
    }
    try body.appendSlice("</div>");
    return body.items;
}

// ── Exportación ICS con datos académicos ──────────────────────────────────

fn icsEscape(allocator: std.mem.Allocator, text: []const u8) ![]const u8 {
    var escaped: std.ArrayListUnmanaged(u8) = .empty;
    for (text) |ch| switch (ch) {
        '\\' => try escaped.appendSlice(allocator, "\\\\"),
        ';' => try escaped.appendSlice(allocator, "\\;"),
        ',' => try escaped.appendSlice(allocator, "\\,"),
        '\r' => {},
        '\n' => try escaped.appendSlice(allocator, "\\n"),
        else => try escaped.append(allocator, ch),
    };
    return escaped.items;
}

fn appendIcsLine(allocator: std.mem.Allocator, output: *std.ArrayList(u8), line: []const u8) !void {
    try output.appendSlice(try std.fmt.allocPrint(allocator, "{s}\r\n", .{line}));
}

fn compactDate(iso: []const u8) ![]const u8 {
    if (iso.len < 10) return error.InvalidDate;
    return iso[0..10];
}

fn compactIso(allocator: std.mem.Allocator, iso: []const u8) ![]const u8 {
    const date = try compactDate(iso);
    return std.fmt.allocPrint(allocator, "{s}{s}{s}", .{ date[0..4], date[5..7], date[8..10] });
}

/// VEVENTs de eventos y entregas de Mikareno, para añadirlos al ICS local.
pub fn academicEventsIcs(allocator: std.mem.Allocator, d: panel.Datos) ![]const u8 {
    var out = std.ArrayList(u8).init(allocator);

    var i: usize = 0;
    for (d.eventos) |e| {
        i += 1;
        const date = datePart(e.fecha);
        if (parseIso(date) == null) continue;
        const start = compactIso(allocator, date) catch continue;
        const end = compactIso(allocator, try shiftDays(allocator, date, 1)) catch continue;
        try appendIcsLine(allocator, &out, "BEGIN:VEVENT");
        try appendIcsLine(allocator, &out, try std.fmt.allocPrint(allocator, "UID:mikareno-event-{d}@study-platform.local", .{i}));
        try appendIcsLine(allocator, &out, "DTSTAMP:19700101T000000Z");
        try appendIcsLine(allocator, &out, try std.fmt.allocPrint(allocator, "DTSTART;VALUE=DATE:{s}", .{start}));
        try appendIcsLine(allocator, &out, try std.fmt.allocPrint(allocator, "DTEND;VALUE=DATE:{s}", .{end}));
        try appendIcsLine(allocator, &out, try std.fmt.allocPrint(allocator, "SUMMARY:{s}", .{try icsEscape(allocator, e.titulo)}));
        if (std.mem.eql(u8, e.tipo, "NON_SCHOOL_DAY")) {
            try appendIcsLine(allocator, &out, "CATEGORIES:Día no lectivo");
        }
        try appendIcsLine(allocator, &out, "END:VEVENT");
    }

    i = 0;
    for (d.tareas) |t| {
        i += 1;
        const date = datePart(t.vencimiento);
        if (parseIso(date) == null) continue;
        const start = compactIso(allocator, date) catch continue;
        const end = compactIso(allocator, try shiftDays(allocator, date, 1)) catch continue;
        try appendIcsLine(allocator, &out, "BEGIN:VEVENT");
        try appendIcsLine(allocator, &out, try std.fmt.allocPrint(allocator, "UID:mikareno-task-{d}@study-platform.local", .{i}));
        try appendIcsLine(allocator, &out, "DTSTAMP:19700101T000000Z");
        try appendIcsLine(allocator, &out, try std.fmt.allocPrint(allocator, "DTSTART;VALUE=DATE:{s}", .{start}));
        try appendIcsLine(allocator, &out, try std.fmt.allocPrint(allocator, "DTEND;VALUE=DATE:{s}", .{end}));
        try appendIcsLine(allocator, &out, try std.fmt.allocPrint(allocator, "SUMMARY:{s}", .{try icsEscape(allocator, try std.fmt.allocPrint(allocator, "Entrega: {s}", .{t.titulo}))}));
        try appendIcsLine(allocator, &out, "CATEGORIES:Entrega");
        try appendIcsLine(allocator, &out, "END:VEVENT");
    }
    return out.items;
}

/// Inserta los VEVENT académicos antes del cierre del calendario base.
pub fn mergeIcs(allocator: std.mem.Allocator, base: []const u8, extra: []const u8) ![]const u8 {
    const marker = "END:VCALENDAR";
    const idx = std.mem.lastIndexOf(u8, base, marker) orelse return allocator.dupe(u8, base);
    var out = std.ArrayList(u8).init(allocator);
    try out.appendSlice(base[0..idx]);
    try out.appendSlice(extra);
    try out.appendSlice(base[idx..]);
    return out.items;
}

test "shiftDays cruza fin de mes y de anio" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    try std.testing.expectEqualStrings("2026-12-01", try shiftDays(a, "2026-11-30", 1));
    try std.testing.expectEqualStrings("2027-01-01", try shiftDays(a, "2026-12-31", 1));
    try std.testing.expectEqualStrings("2026-11-30", try shiftDays(a, "2026-12-01", -1));
    try std.testing.expectEqualStrings("2024-02-29", try shiftDays(a, "2024-02-28", 1));
}

test "startOfWeek lleva cualquier dia al lunes" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    try std.testing.expectEqualStrings("2026-11-23", try startOfWeek(a, "2026-11-23"));
    try std.testing.expectEqualStrings("2026-11-23", try startOfWeek(a, "2026-11-29"));
    try std.testing.expectEqualStrings("2026-11-23", try startOfWeek(a, "2026-11-25"));
}

test "annotationOn respeta eventos de varios dias y semanales" {
    const base = Anotacion{
        .id = 1,
        .date = "2026-11-24",
        .title = "Retiro",
        .body = "",
        .category = "academic",
        .kind = "event",
        .end_date = "2026-11-26",
        .all_day = true,
        .start_time = "",
        .end_time = "",
        .location = "",
        .recurrence = "",
        .weekly_day = -1,
    };
    try std.testing.expect(annotationOn(base, "2026-11-24"));
    try std.testing.expect(annotationOn(base, "2026-11-25"));
    try std.testing.expect(!annotationOn(base, "2026-11-26"));
    var weekly = base;
    weekly.recurrence = "weekly";
    weekly.end_date = "";
    weekly.weekly_day = 2; // miércoles (lunes = 0)
    try std.testing.expect(annotationOn(weekly, "2026-11-25"));
    try std.testing.expect(!annotationOn(weekly, "2026-11-24"));
}
