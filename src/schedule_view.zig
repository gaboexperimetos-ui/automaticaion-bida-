//! Horario semanal: rejilla de días × horas del día.
//!
//! Es una vista propia, distinta del calendario: aquí solo está el horario
//! recurrente del colegio (las clases fijas de la semana). No mezcla
//! entregas, eventos ni anotaciones; para eso está `/dashboard/calendar`.

const std = @import("std");
const panel = @import("panel.zig");
const security = @import("security.zig");
const calendar_view = @import("calendar_view.zig");

pub const Dia = struct { key: []const u8, label: []const u8, short: []const u8 };

pub const DIAS = [_]Dia{
    .{ .key = "lunes", .label = "Lunes", .short = "Lun" },
    .{ .key = "martes", .label = "Martes", .short = "Mar" },
    .{ .key = "miercoles", .label = "Miércoles", .short = "Mié" },
    .{ .key = "jueves", .label = "Jueves", .short = "Jue" },
    .{ .key = "viernes", .label = "Viernes", .short = "Vie" },
    .{ .key = "sabado", .label = "Sábado", .short = "Sáb" },
    .{ .key = "domingo", .label = "Domingo", .short = "Dom" },
};

pub const Clase = struct {
    dia: []const u8,
    start: []const u8,
    end: []const u8,
    materia: []const u8,
    codigo: []const u8,
};

pub const Fila = struct {
    start: []const u8,
    end: []const u8,
};

/// Hora con forma H:MM o HH:MM (con o sin minutos tras los dos puntos).
fn looksLikeTime(s: []const u8) bool {
    const colon = std.mem.indexOfScalar(u8, s, ':') orelse return false;
    if (colon == 0 or colon > 2) return false;
    const mins = s[colon + 1 ..];
    if (mins.len == 0) return false;
    for (s[0..colon]) |ch| if (!std.ascii.isDigit(ch)) return false;
    for (mins) |ch| if (!std.ascii.isDigit(ch)) return false;
    return true;
}

/// "07:35 - 08:15 — English [ENGL]" → partes. Devuelve null si la etiqueta
/// no trae una hora reconocible; así una clase rara no rompe la rejilla.
pub fn parseEtiqueta(etiqueta: []const u8) ?Clase {
    const SEP = " — "; // espacio + guion largo (3 bytes) + espacio = 5 bytes
    const sep = std.mem.indexOf(u8, etiqueta, SEP) orelse return null;
    const times = etiqueta[0..sep];
    const name_part = std.mem.trim(u8, etiqueta[sep + SEP.len ..], " \t");
    if (name_part.len == 0) return null;

    // Con hora de inicio y fin ("07:35 - 08:15") o solo inicio ("08:00").
    var start: []const u8 = "";
    var end: []const u8 = "";
    if (std.mem.indexOf(u8, times, " - ")) |dash| {
        start = std.mem.trim(u8, times[0..dash], " \t");
        // Se recorta también el guion: " - " deja "- 08:15".
        end = std.mem.trim(u8, times[dash..], " \t-");
    } else if (std.mem.indexOfScalar(u8, times, '-')) |dash| {
        start = std.mem.trim(u8, times[0..dash], " \t");
        end = std.mem.trim(u8, times[dash..], " \t-");
    } else {
        start = std.mem.trim(u8, times, " \t");
    }
    if (!looksLikeTime(start)) return null;
    if (end.len > 0 and !looksLikeTime(end)) end = "";

    var materia = name_part;
    var codigo: []const u8 = "";
    if (std.mem.lastIndexOfScalar(u8, name_part, '[')) |lb| {
        if (name_part.len > lb + 2 and name_part[name_part.len - 1] == ']') {
            codigo = std.mem.trim(u8, name_part[lb + 1 .. name_part.len - 1], " \t");
            materia = std.mem.trim(u8, name_part[0..lb], " \t");
        }
    }
    if (materia.len == 0) materia = name_part;

    return .{ .dia = "", .start = start, .end = end, .materia = materia, .codigo = codigo };
}

pub fn clasesDe(d: panel.Datos, allocator: std.mem.Allocator) ![]Clase {
    var out = std.ArrayList(Clase).init(allocator);
    for (d.horario) |c| {
        var parsed = parseEtiqueta(c.etiqueta) orelse continue;
        parsed.dia = c.dia;
        try out.append(parsed);
    }
    return out.items;
}

pub const SinHora = struct {
    dia: []const u8,
    materia: []const u8,
    codigo: []const u8,
};

/// Clases sin hora reconocible: no caben en la rejilla, pero tampoco se
/// descartan. Se listan aparte sin inventarles una hora.
pub fn sinHoraDe(d: panel.Datos, allocator: std.mem.Allocator) ![]SinHora {
    var out = std.ArrayList(SinHora).init(allocator);
    for (d.horario) |c| {
        if (parseEtiqueta(c.etiqueta) != null) continue;
        // Un hueco sin nombre sí trae " — ": no es una clase, se ignora.
        if (std.mem.indexOf(u8, c.etiqueta, " — ") != null) continue;
        const raw = std.mem.trim(u8, c.etiqueta, " \t");
        if (raw.len == 0) continue;
        var materia = raw;
        var codigo: []const u8 = "";
        if (std.mem.lastIndexOfScalar(u8, raw, '[')) |lb| {
            if (raw.len > lb + 2 and raw[raw.len - 1] == ']') {
                codigo = std.mem.trim(u8, raw[lb + 1 .. raw.len - 1], " \t");
                materia = std.mem.trim(u8, raw[0..lb], " \t");
            }
        }
        if (materia.len == 0) materia = raw;
        try out.append(.{ .dia = c.dia, .materia = materia, .codigo = codigo });
    }
    return out.items;
}

fn diaLabel(key: []const u8) []const u8 {
    for (DIAS) |dia| {
        if (std.mem.eql(u8, dia.key, key)) return dia.label;
    }
    return key;
}

// ── Agenda del día ────────────────────────────────────────────────────────
//
// El calendario es el mapa del mes (citas y actividades puntuales). El
// horario es la agenda de UN día, hora a hora: clases fijas + eventos y
// entregas de esa fecha, para organizarse.

pub const Item = struct {
    time: []const u8,
    end: []const u8,
    titulo: []const u8,
    detalle: []const u8,
    kind: []const u8, // "class" | "event" | "task"
};

fn datePart(value: []const u8) []const u8 {
    return if (value.len >= 10) value[0..10] else value;
}

/// "2026-11-24 08:00" o "2026-11-24T08:00" → "08:00"; si no hay hora, "".
fn timePart(value: []const u8) []const u8 {
    for (value, 0..) |ch, i| {
        if (ch != 'T' and ch != ' ') continue;
        if (i + 6 <= value.len) {
            const t = value[i + 1 .. i + 6];
            if (looksLikeTime(t)) return t;
        }
        return "";
    }
    return "";
}

fn isGenericClass(titulo: []const u8) bool {
    return std.ascii.eqlIgnoreCase(titulo, "clases") or std.ascii.eqlIgnoreCase(titulo, "clase");
}

fn itemLess(_: void, a: Item, b: Item) bool {
    const a_empty = a.time.len == 0;
    const b_empty = b.time.len == 0;
    if (a_empty != b_empty) return !a_empty; // con hora primero
    const o = std.mem.order(u8, a.time, b.time);
    if (o != .eq) return o == .lt;
    return std.mem.lessThan(u8, a.titulo, b.titulo);
}

pub fn isValidDay(iso: []const u8) bool {
    if (iso.len < 10) return false;
    for (iso[0..10], 0..) |ch, i| {
        if (i == 4 or i == 7) {
            if (ch != '-') return false;
        } else if (!std.ascii.isDigit(ch)) return false;
    }
    return true;
}

pub fn agendaItems(allocator: std.mem.Allocator, d: panel.Datos, iso: []const u8) ![]Item {
    var out = std.ArrayList(Item).init(allocator);
    const wd = panel.weekdayOf(iso) orelse "";

    for (d.horario) |c| {
        if (!std.mem.eql(u8, c.dia, wd)) continue;
        if (parseEtiqueta(c.etiqueta)) |cl| {
            try out.append(.{ .time = cl.start, .end = cl.end, .titulo = cl.materia, .detalle = cl.codigo, .kind = "class" });
        } else if (std.mem.indexOf(u8, c.etiqueta, " — ") == null) {
            const raw = std.mem.trim(u8, c.etiqueta, " \t");
            if (raw.len == 0) continue;
            try out.append(.{ .time = "", .end = "", .titulo = raw, .detalle = "", .kind = "class" });
        }
    }
    for (d.eventos) |e| {
        if (!std.mem.eql(u8, datePart(e.fecha), iso)) continue;
        if (isGenericClass(e.titulo)) continue;
        const no_lectivo = std.mem.eql(u8, e.tipo, "NON_SCHOOL_DAY");
        try out.append(.{
            .time = timePart(e.fecha),
            .end = "",
            .titulo = e.titulo,
            .detalle = if (no_lectivo) "Día no lectivo" else "Evento del colegio",
            .kind = "event",
        });
    }
    for (d.tareas) |t| {
        if (!std.mem.eql(u8, datePart(t.vencimiento), iso)) continue;
        try out.append(.{
            .time = timePart(t.vencimiento),
            .end = "",
            .titulo = t.titulo,
            .detalle = "Entrega",
            .kind = "task",
        });
    }
    std.mem.sort(Item, out.items, {}, itemLess);
    return out.items;
}

fn kindLabel(kind: []const u8) []const u8 {
    if (std.mem.eql(u8, kind, "class")) return "Clase";
    if (std.mem.eql(u8, kind, "task")) return "Entrega";
    return "Actividad";
}

fn tabsHtml(allocator: std.mem.Allocator, active: []const u8) ![]const u8 {
    const day_cls = if (std.mem.eql(u8, active, "day")) " is-active" else "";
    const week_cls = if (std.mem.eql(u8, active, "week")) " is-active" else "";
    return std.fmt.allocPrint(
        allocator,
        "<nav class=\"nx-sch-tabs\" aria-label=\"Vista del horario\"><a class=\"{s}\" href=\"/dashboard/horario\">Día</a><a class=\"{s}\" href=\"/dashboard/horario?view=week\">Semana</a></nav>",
        .{ day_cls, week_cls },
    );
}

pub fn renderAgenda(allocator: std.mem.Allocator, d: panel.Datos, iso: []const u8, today: []const u8) ![]const u8 {
    const items = try agendaItems(allocator, d, iso);
    var clases: usize = 0;
    var actividades: usize = 0;
    for (items) |it| {
        if (std.mem.eql(u8, it.kind, "class")) clases += 1 else actividades += 1;
    }
    const prev = try calendar_view.shiftDays(allocator, iso, -1);
    const next = try calendar_view.shiftDays(allocator, iso, 1);
    const long_date = try calendar_view.longDateEs(allocator, iso);
    const is_today = std.mem.eql(u8, iso, today);

    var body = std.ArrayList(u8).init(allocator);
    try body.appendSlice("<div class=\"nx-page nova-page nx-schedule-page\">");
    try body.appendSlice(try tabsHtml(allocator, "day"));
    try body.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<header class=\"nx-sch-head\"><div class=\"nx-sch-head-main\"><p class=\"nx-sch-eyebrow\">Agenda del día</p><h1>{s}</h1><p class=\"nx-sch-sub\">Tus clases y actividades de este día, hora a hora. El calendario es el mapa del mes.</p></div><div class=\"nx-sch-kpis\" aria-label=\"Resumen del día\"><span><strong>{d}</strong><small>clases</small></span><span><strong>{d}</strong><small>actividades</small></span></div></header>",
        .{ try esc(allocator, long_date), clases, actividades },
    ));

    // Navegación de días. La fecha larga ya está en el título: aquí va la
    // forma corta y el atajo "Hoy" solo cuando estás en otro día.
    const mid = if (is_today)
        "Hoy"
    else
        try std.fmt.allocPrint(allocator, "{s}/{s}/{s}", .{ iso[8..10], iso[5..7], iso[0..4] });
    const today_link = if (is_today)
        ""
    else
        "<a class=\"nx-agenda-today\" href=\"/dashboard/horario\">Hoy</a>";
    try body.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-agenda-nav\"><a class=\"nx-agenda-step\" href=\"/dashboard/horario?d={s}\" aria-label=\"Día anterior\"><span class=\"material-symbols-outlined\">chevron_left</span></a><span class=\"nx-agenda-day{s}\">{s}</span><a class=\"nx-agenda-step\" href=\"/dashboard/horario?d={s}\" aria-label=\"Día siguiente\"><span class=\"material-symbols-outlined\">chevron_right</span></a>{s}</div>",
        .{
            try esc(allocator, prev),
            if (is_today) " is-today" else "",
            try esc(allocator, mid),
            try esc(allocator, next),
            today_link,
        },
    ));

    if (!d.connected or d.offline) {
        try body.appendSlice(try panel.syncBanner(allocator, d));
    }

    if (items.len == 0) {
        try body.appendSlice("<section class=\"nx-card nx-agenda-empty\"><span class=\"material-symbols-outlined\">event_available</span><h2>Nada programado para este día</h2><p>No hay clases ni actividades para esta fecha. Puedes ver el mes completo en el calendario.</p><a class=\"nx-button nx-button-secondary\" href=\"/dashboard/calendar\">Abrir calendario</a></section>");
        try body.appendSlice("</div>");
        return body.items;
    }

    try body.appendSlice("<ol class=\"nx-agenda\">");
    for (items) |it| {
        const time_text = if (it.time.len == 0)
            "Todo el día"
        else if (it.end.len == 0)
            it.time
        else
            try std.fmt.allocPrint(allocator, "{s}–{s}", .{ it.time, it.end });
        try body.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<li class=\"nx-agenda-item is-{s}\"><span class=\"nx-agenda-time\">{s}</span><div class=\"nx-agenda-body\"><span class=\"nx-agenda-kind\">{s}</span><span class=\"nx-agenda-title\">{s}</span>{s}</div></li>",
            .{
                it.kind,
                try esc(allocator, time_text),
                kindLabel(it.kind),
                try esc(allocator, it.titulo),
                if (it.detalle.len == 0)
                    ""
                else
                    try std.fmt.allocPrint(allocator, "<span class=\"nx-agenda-meta\">{s}</span>", .{try esc(allocator, it.detalle)}),
            },
        ));
    }
    try body.appendSlice("</ol>");
    try body.appendSlice("<footer class=\"nx-sch-foot\"><span><span class=\"material-symbols-outlined\">info</span>Agenda de un día. El mes completo está en el calendario.</span><a href=\"/dashboard/calendar\">Abrir calendario<span class=\"material-symbols-outlined\">arrow_outward</span></a></footer>");
    try body.appendSlice("</div>");
    return body.items;
}

fn rowLess(_: void, a: Fila, b: Fila) bool {
    const o = std.mem.order(u8, a.start, b.start);
    if (o != .eq) return o == .lt;
    return std.mem.order(u8, a.end, b.end) == .lt;
}

/// Franjas horarias únicas de toda la semana, ordenadas por hora de inicio.
pub fn buildRows(allocator: std.mem.Allocator, clases: []const Clase) ![]Fila {
    var rows = std.ArrayList(Fila).init(allocator);
    for (clases) |c| {
        var seen = false;
        for (rows.items) |r| {
            if (std.mem.eql(u8, r.start, c.start) and std.mem.eql(u8, r.end, c.end)) {
                seen = true;
                break;
            }
        }
        if (!seen) try rows.append(.{ .start = c.start, .end = c.end });
    }
    std.mem.sort(Fila, rows.items, {}, rowLess);
    return rows.items;
}

fn classAt(clases: []const Clase, dia: []const u8, fila: Fila) ?Clase {
    for (clases) |c| {
        if (!std.mem.eql(u8, c.dia, dia)) continue;
        if (!std.mem.eql(u8, c.start, fila.start)) continue;
        return c;
    }
    return null;
}

fn dayHasClasses(clases: []const Clase, dia: []const u8) bool {
    for (clases) |c| {
        if (std.mem.eql(u8, c.dia, dia)) return true;
    }
    return false;
}

fn minutesOf(hhmm: []const u8) ?i32 {
    if (hhmm.len < 5 or hhmm[2] != ':') return null;
    const h = std.fmt.parseInt(i32, hhmm[0..2], 10) catch return null;
    const m = std.fmt.parseInt(i32, hhmm[3..5], 10) catch return null;
    return h * 60 + m;
}

/// Tono estable por materia, para que la misma asignatura tenga siempre el
/// mismo color sin configurar nada.
pub fn toneFor(name: []const u8) u8 {
    var h: u32 = 2166136261;
    for (name) |ch| {
        h ^= ch;
        h *%= 16777619;
    }
    return @intCast(h % 6);
}

fn esc(allocator: std.mem.Allocator, s: []const u8) ![]const u8 {
    return security.htmlEscape(allocator, s);
}

pub fn render(allocator: std.mem.Allocator, d: panel.Datos, today: []const u8) ![]const u8 {
    const clases = try clasesDe(d, allocator);
    const sin_hora = try sinHoraDe(d, allocator);
    const filas = try buildRows(allocator, clases);
    const hoy_dia = panel.weekdayOf(today) orelse "";

    var body = std.ArrayList(u8).init(allocator);
    try body.appendSlice("<div class=\"nx-page nova-page nx-schedule-page\">");

    var total_min: i32 = 0;
    for (clases) |c| {
        if (minutesOf(c.start)) |s| {
            if (minutesOf(c.end)) |e| total_min += @max(0, e - s);
        }
    }
    const horas = @as(f64, @floatFromInt(total_min)) / 60.0;
    var dias_con_clase: usize = 0;
    for (DIAS) |dia| {
        if (dayHasClasses(clases, dia.key)) dias_con_clase += 1;
    }

    try body.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<header class=\"nx-sch-head\"><div class=\"nx-sch-head-main\"><p class=\"nx-sch-eyebrow\">Horario semanal</p><h1>Horario</h1><p class=\"nx-sch-sub\">Tus clases fijas de la semana, con hora de inicio y fin. Es el horario del colegio, no el calendario.</p></div><div class=\"nx-sch-kpis\" aria-label=\"Resumen del horario\"><span><strong>{d}</strong><small>clases</small></span><span><strong>{d:.1}</strong><small>horas</small></span><span><strong>{d}</strong><small>días</small></span></div></header>",
        .{ clases.len, horas, dias_con_clase },
    ));
    try body.appendSlice(try tabsHtml(allocator, "week"));

    if (!d.connected or d.offline) {
        try body.appendSlice(try panel.syncBanner(allocator, d));
    }

    if (clases.len == 0 and sin_hora.len == 0) {
        try body.appendSlice("<section class=\"nx-card nx-sch-empty\"><span class=\"material-symbols-outlined\">calendar_view_week</span><h2>Sin horario cargado</h2><p>Mikareno no devolvió clases para este curso. No se inventan horarios.</p><a class=\"nx-button nx-button-secondary\" href=\"/dashboard/mcp\"><span class=\"material-symbols-outlined\">refresh</span>Revisar conexión</a></section>");
        try body.appendSlice("</div>");
        return body.items;
    }

    // Solo se muestran los días que tienen clase; el fin de semana aparece
    // únicamente si de verdad hay algo programado.
    var activos: [7]bool = undefined;
    for (DIAS, 0..) |dia, i| activos[i] = dayHasClasses(clases, dia.key);

    if (clases.len > 0) {
        try body.appendSlice("<section class=\"nx-card nx-sch-card\" aria-label=\"Horario semanal por horas\"><div class=\"nx-sch-scroll\"><table class=\"nx-sch\"><caption class=\"nx-sch-caption\">Clases de la semana por hora y día</caption><thead><tr><th scope=\"col\" class=\"nx-sch-hour\">Hora</th>");
        for (DIAS, 0..) |dia, i| {
            if (!activos[i]) continue;
            try body.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<th scope=\"col\" class=\"nx-sch-day{s}\"><span>{s}</span></th>",
                .{ if (std.mem.eql(u8, dia.key, hoy_dia)) " is-today" else "", dia.label },
            ));
        }
        try body.appendSlice("</tr></thead><tbody>");

        for (filas) |fila| {
            try body.appendSlice("<tr>");
            const row_end = if (fila.end.len == 0)
                ""
            else
                try std.fmt.allocPrint(allocator, "<small>{s}</small>", .{try esc(allocator, fila.end)});
            try body.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<th scope=\"row\" class=\"nx-sch-hour\"><span>{s}</span>{s}</th>",
                .{ try esc(allocator, fila.start), row_end },
            ));
            for (DIAS, 0..) |dia, i| {
                if (!activos[i]) continue;
                const is_today = std.mem.eql(u8, dia.key, hoy_dia);
                if (classAt(clases, dia.key, fila)) |c| {
                    const tone = toneFor(c.materia);
                    const range = if (c.end.len == 0)
                        try esc(allocator, c.start)
                    else
                        try std.fmt.allocPrint(allocator, "{s}–{s}", .{
                            try esc(allocator, c.start),
                            try esc(allocator, c.end),
                        });
                    try body.appendSlice(try std.fmt.allocPrint(
                        allocator,
                        "<td class=\"nx-sch-cell tone-{d}{s}\"><span class=\"nx-sch-subject\">{s}</span>{s}<span class=\"nx-sch-range\">{s}</span></td>",
                        .{
                            tone,
                            if (is_today) " is-today" else "",
                            try esc(allocator, c.materia),
                            if (c.codigo.len == 0)
                                ""
                            else
                                try std.fmt.allocPrint(allocator, "<span class=\"nx-sch-code\">{s}</span>", .{try esc(allocator, c.codigo)}),
                            range,
                        },
                    ));
                } else {
                    try body.appendSlice(try std.fmt.allocPrint(
                        allocator,
                        "<td class=\"nx-sch-cell is-empty{s}\" aria-label=\"Sin clase\"><span>—</span></td>",
                        .{if (is_today) " is-today" else ""},
                    ));
                }
            }
            try body.appendSlice("</tr>");
        }

        try body.appendSlice("</tbody></table></div><footer class=\"nx-sch-foot\"><span><span class=\"material-symbols-outlined\">info</span>Solo horario de clases. Las entregas y eventos están en el calendario.</span><a href=\"/dashboard/calendar\">Abrir calendario<span class=\"material-symbols-outlined\">arrow_outward</span></a></footer></section>");
    }

    if (sin_hora.len > 0) {
        try body.appendSlice("<section class=\"nx-card nx-sch-sinhora\" aria-label=\"Clases sin hora asignada\"><h2><span class=\"material-symbols-outlined\">help</span>Clases sin hora asignada</h2><p class=\"nx-sch-sinhora-sub\">Mikareno no dio una hora para estas clases. No se inventa: aparecen aquí hasta que el horario las tenga.</p><ul>");
        for (sin_hora) |sh| {
            try body.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<li><span class=\"nx-sch-sinhora-dia\">{s}</span><span class=\"nx-sch-subject\">{s}</span>{s}</li>",
                .{
                    try esc(allocator, diaLabel(sh.dia)),
                    try esc(allocator, sh.materia),
                    if (sh.codigo.len == 0)
                        ""
                    else
                        try std.fmt.allocPrint(allocator, "<span class=\"nx-sch-code\">{s}</span>", .{try esc(allocator, sh.codigo)}),
                },
            ));
        }
        try body.appendSlice("</ul></section>");
    }
    try body.appendSlice("</div>");
    return body.items;
}

// ── Tests ─────────────────────────────────────────────────────────────────

test "parsea la etiqueta con hora, materia y código" {
    const c = parseEtiqueta("07:35 - 08:15 — English [ENGL]").?;
    try std.testing.expectEqualStrings("07:35", c.start);
    try std.testing.expectEqualStrings("08:15", c.end);
    try std.testing.expectEqualStrings("English", c.materia);
    try std.testing.expectEqualStrings("ENGL", c.codigo);
}

test "acepta etiquetas sin código y sin espacios alrededor del guion" {
    const a = parseEtiqueta("10:35 - 11:05 — Recreo").?;
    try std.testing.expectEqualStrings("Recreo", a.materia);
    try std.testing.expectEqualStrings("", a.codigo);
    const b = parseEtiqueta("10:35-11:05 — Recreo").?;
    try std.testing.expectEqualStrings("10:35", b.start);
    try std.testing.expectEqualStrings("11:05", b.end);
}

test "una etiqueta sin hora no se convierte en clase" {
    try std.testing.expect(parseEtiqueta("Sin hora — Materia") == null);
    try std.testing.expect(parseEtiqueta("") == null);
}

test "las filas se deduplican y se ordenan por hora" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const clases = [_]Clase{
        .{ .dia = "lunes", .start = "08:15", .end = "08:35", .materia = "Tutoria", .codigo = "TUTO" },
        .{ .dia = "martes", .start = "07:35", .end = "08:15", .materia = "English", .codigo = "ENGL" },
        .{ .dia = "miercoles", .start = "08:15", .end = "08:35", .materia = "Tutoria", .codigo = "TUTO" },
    };
    const filas = try buildRows(a, &clases);
    try std.testing.expectEqual(@as(usize, 2), filas.len);
    try std.testing.expectEqualStrings("07:35", filas[0].start);
    try std.testing.expectEqualStrings("08:15", filas[1].start);
}

test "el render muestra los días con clase, las horas y las materias" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var horario = [_]panel.Clase{
        .{ .dia = "lunes", .etiqueta = "07:35 - 08:15 — English [ENGL]" },
        .{ .dia = "martes", .etiqueta = "07:35 - 08:15 — Matemática [MATE]" },
    };
    const d = panel.Datos{ .connected = true, .horario = &horario };
    const html = try render(a, d, "2026-11-24");
    try std.testing.expect(std.mem.indexOf(u8, html, "English") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Matemática") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "07:35") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Lunes") != null);
    // El martes es hoy: su columna se marca, no el resto.
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-sch-day is-today") != null);
}

test "sin clases el horario dice que no hay datos en vez de inventarlos" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const d = panel.Datos{ .connected = false, .offline = true };
    const html = try render(a, d, "2026-11-24");
    try std.testing.expect(std.mem.indexOf(u8, html, "Sin horario cargado") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Mostrando los últimos datos guardados") != null);
}

test "suma las horas reales de las clases" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var horario = [_]panel.Clase{
        .{ .dia = "lunes", .etiqueta = "07:35 - 08:15 — English [ENGL]" },
        .{ .dia = "lunes", .etiqueta = "08:15 - 08:35 — Tutoria [TUTO]" },
    };
    const d = panel.Datos{ .connected = true, .horario = &horario };
    const html = try render(a, d, "2026-11-24");
    try std.testing.expect(std.mem.indexOf(u8, html, "<strong>1.0</strong><small>horas") != null);
}

test "acepta una etiqueta con solo hora de inicio" {
    const c = parseEtiqueta("08:00 — Matemáticas reales [MAT]").?;
    try std.testing.expectEqualStrings("08:00", c.start);
    try std.testing.expectEqualStrings("", c.end);
    try std.testing.expectEqualStrings("Matemáticas reales", c.materia);
    try std.testing.expectEqualStrings("MAT", c.codigo);
}

test "una clase sin hora se lista aparte y no en la rejilla" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var horario = [_]panel.Clase{
        .{ .dia = "lunes", .etiqueta = "Matemáticas reales [MAT]" },
        .{ .dia = "lunes", .etiqueta = "07:35 - 08:15 — English [ENGL]" },
    };
    const d = panel.Datos{ .connected = true, .horario = &horario };
    const html = try render(a, d, "2026-11-24");
    try std.testing.expect(std.mem.indexOf(u8, html, "Clases sin hora asignada") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Matemáticas reales") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "English") != null);
}

test "isValidDay acepta solo YYYY-MM-DD" {
    try std.testing.expect(isValidDay("2026-11-24"));
    try std.testing.expect(!isValidDay("2026-1-24"));
    try std.testing.expect(!isValidDay("24/11/2026"));
    try std.testing.expect(!isValidDay(""));
}

test "la agenda junta clases, entregas y eventos del día por hora" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var horario = [_]panel.Clase{
        .{ .dia = "martes", .etiqueta = "07:35 - 08:15 — English [ENGL]" },
    };
    var eventos = [_]panel.Evento{
        .{ .fecha = "2026-11-24 10:00", .titulo = "Reunión de curso", .tipo = "EVENT" },
        .{ .fecha = "2026-11-25", .titulo = "Otro día", .tipo = "EVENT" },
    };
    var tareas = [_]panel.Tarea{
        .{ .titulo = "Entrega real", .vencimiento = "2026-11-24 09:00" },
    };
    const d = panel.Datos{ .connected = true, .horario = &horario, .eventos = &eventos, .tareas = &tareas };
    const items = try agendaItems(a, d, "2026-11-24");
    try std.testing.expectEqual(@as(usize, 3), items.len);
    try std.testing.expectEqualStrings("English", items[0].titulo);
    try std.testing.expectEqualStrings("07:35", items[0].time);
    try std.testing.expectEqualStrings("Entrega real", items[1].titulo);
    try std.testing.expectEqualStrings("task", items[1].kind);
    try std.testing.expectEqualStrings("Reunión de curso", items[2].titulo);
    try std.testing.expectEqualStrings("event", items[2].kind);
}

test "la agenda muestra el día y sus actividades" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var horario = [_]panel.Clase{
        .{ .dia = "martes", .etiqueta = "07:35 - 08:15 — English [ENGL]" },
    };
    var tareas = [_]panel.Tarea{
        .{ .titulo = "Entrega real", .vencimiento = "2026-11-24 09:00" },
    };
    const d = panel.Datos{ .connected = true, .horario = &horario, .tareas = &tareas };
    const html = try renderAgenda(a, d, "2026-11-24", "2026-11-24");
    try std.testing.expect(std.mem.indexOf(u8, html, "martes, 24 de noviembre de 2026") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "English") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Entrega real") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Hoy") != null);
}

test "un día sin clases ni actividades lo dice sin inventar" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const d = panel.Datos{ .connected = true };
    const html = try renderAgenda(a, d, "2026-11-24", "2026-11-24");
    try std.testing.expect(std.mem.indexOf(u8, html, "Nada programado para este día") != null);
}
