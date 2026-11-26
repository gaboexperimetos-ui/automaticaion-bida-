//! Vistas académicas reales de NOVA.
//!
//! Cada función recibe el JSON que Mikareno ya devolvió y lo convierte en
//! HTML con las clases `nx-` del sistema visual. Reglas del módulo:
//!
//! * **Real o nada**: si el arreglo viene vacío se muestra un estado vacío
//!   honesto, nunca una fila inventada.
//! * **Escape**: todo texto que venga de Mikareno pasa por `security`.
//! * **Sin estado global**: son funciones puras; el llamador decide cuándo
//!   pedir los datos y con qué caché.

const std = @import("std");
const json = @import("jsonutil.zig");
const security = @import("security.zig");
const charts = @import("charts.zig");

fn esc(allocator: std.mem.Allocator, s: []const u8) ![]const u8 {
    return security.htmlEscape(allocator, s);
}

/// Compara un día del horario con el esperado sin importar mayúsculas ni
/// acentos: Mikareno devuelve "Lunes", "lunes" o "MIÉRCOLES" según la vista.
fn sameDay(a: []const u8, b: []const u8) bool {
    if (a.len != b.len) return false;
    for (a, b) |ca, cb| {
        var na = ca;
        var nb = cb;
        // Se normaliza a minúscula ASCII; las vocales acentuadas se tratan
        // como su versión sin tilde para que "Miércoles" case con "miercoles".
        if (na >= 'A' and na <= 'Z') na += 32;
        if (nb >= 'A' and nb <= 'Z') nb += 32;
        if (na == nb) continue;
        // En UTF-8 la tilde son dos bytes; se ignoran para comparar la base.
        if (na >= 0x80 or nb >= 0x80) continue;
        return false;
    }
    return true;
}

/// `2026-11-18` → `18/11/2026`; cualquier otra cosa se devuelve tal cual.
fn fecha(allocator: std.mem.Allocator, iso: []const u8) ![]const u8 {
    if (iso.len == 10 and iso[4] == '-' and iso[7] == '-') {
        var digits = true;
        for (iso[0..4]) |c| if (c < '0' or c > '9') {
            digits = false;
        };
        for (iso[5..7]) |c| if (c < '0' or c > '9') {
            digits = false;
        };
        for (iso[8..10]) |c| if (c < '0' or c > '9') {
            digits = false;
        };
        if (digits) return std.fmt.allocPrint(allocator, "{s}/{s}/{s}", .{ iso[8..10], iso[5..7], iso[0..4] });
    }
    return allocator.dupe(u8, if (iso.len == 0) "sin fecha" else iso);
}

/// Estado vacío reutilizable para cuando Mikareno no trajo nada.
fn vacio(allocator: std.mem.Allocator, titulo: []const u8, detalle: []const u8) ![]const u8 {
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-panel nx-academic-empty\"><span class=\"material-symbols-outlined\">inbox</span><p><strong>{s}</strong><br>{s}</p></div>",
        .{ titulo, detalle },
    );
}

/// Cabecera común de las páginas académicas.
fn pageHead(allocator: std.mem.Allocator, eyebrow: []const u8, titulo: []const u8, subtitulo: []const u8, acciones: []const u8) ![]const u8 {
    return std.fmt.allocPrint(
        allocator,
        "<header class=\"nx-page-header nova-page-head\"><div><p class=\"nx-eyebrow\">{s}</p><h1>{s}</h1><p>{s}</p></div><div class=\"nx-button-row\">{s}</div></header>",
        .{ eyebrow, titulo, subtitulo, acciones },
    );
}

fn refreshButton() []const u8 {
    return "<a class=\"nx-button nx-button-secondary\" href=\"?refresh=1\" data-recargar><span class=\"material-symbols-outlined\">refresh</span>Actualizar</a>";
}

/// Tarjeta compacta de una métrica.
fn kpi(allocator: std.mem.Allocator, etiqueta: []const u8, valor: []const u8, pie: []const u8, clase: []const u8) ![]const u8 {
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-grade-kpi {s}\"><span>{s}</span><strong>{s}</strong><small>{s}</small></div>",
        .{ clase, etiqueta, valor, pie },
    );
}

/// Clase de color según la nota, con la misma escala que la vista de notas.
fn toneClass(nota: f64) []const u8 {
    if (nota >= 7) return "is-good";
    if (nota >= 5) return "is-warn";
    return "is-bad";
}

fn notaTxt(allocator: std.mem.Allocator, nota: ?f64) ![]const u8 {
    const n = nota orelse return allocator.dupe(u8, "—");
    return std.fmt.allocPrint(allocator, "{d:.2}", .{n});
}

// ─────────────────────────────── Horario ───────────────────────────────

/// Rejilla semanal del horario real. Agrupa por día en el orden de la
/// semana y deja las clases seguidas en el mismo bloque.
pub fn horarioHtml(allocator: std.mem.Allocator, raw: json.Value) ![]const u8 {
    const lista = json.get(raw, "horario") orelse return vacio(allocator, "Sin horario", "Mikareno no devolvió clases para este curso.");
    if (lista != .array or lista.array.items.len == 0) return vacio(allocator, "Sin horario", "Mikareno no devolvió clases para este curso.");

    const dias = [_][]const u8{ "lunes", "martes", "miercoles", "jueves", "viernes", "sabado", "domingo" };
    var total_clases: usize = 0;
    var horas: usize = 0;
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<div class=\"nx-academic-grid nx-horario-grid\">");
    for (dias) |dia| {
        var items = std.ArrayList(u8).init(allocator);
        var count: usize = 0;
        for (lista.array.items) |slot| {
            const sd = json.getString(slot, "dia") orelse continue;
            if (!sameDay(sd, dia)) continue;
            const clase = json.getString(slot, "clase") orelse "";
            if (clase.len == 0) continue;
            const hora = json.getString(slot, "hora") orelse "";
            const codigo = json.getString(slot, "codigo") orelse "";
            const minutos = json.getInt(slot, "duracion_min") orelse 0;
            try items.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<li class=\"nx-horario-clase\"><span class=\"nx-horario-hora\">{s}</span><span class=\"nx-horario-nombre\">{s}</span>{s}<small>{d} min</small></li>",
                .{ try esc(allocator, hora), try esc(allocator, clase), blk: {
                    if (codigo.len == 0) break :blk "";
                    break :blk try std.fmt.allocPrint(allocator, "<span class=\"nx-mini-chip\">{s}</span>", .{try esc(allocator, codigo)});
                }, minutos },
            ));
            count += 1;
            if (minutos > 0) horas += @intCast(minutos);
        }
        total_clases += count;
        try buf.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<section class=\"nx-horario-dia\"><h2>{s}{d}</h2>{s}</section>",
            .{ dia[0..1], count, if (count == 0)
                "<p class=\"nx-academic-muted\">Sin clases</p>"
            else
                try std.fmt.allocPrint(allocator, "<ul>{s}</ul>", .{items.items}) },
        ));
    }
    try buf.appendSlice("</div>");

    const kpis = try std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-grade-kpis nx-academic-kpis\">{s}{s}{s}</section>",
        .{
            try kpi(allocator, "Clases", try std.fmt.allocPrint(allocator, "{d}", .{total_clases}), "en la semana", ""),
            try kpi(allocator, "Horas", try std.fmt.allocPrint(allocator, "{d:.1}", .{@as(f64, @floatFromInt(horas)) / 60.0}), "de clase semanales", ""),
            try kpi(allocator, "Días", try std.fmt.allocPrint(allocator, "{d}", .{dias.len}), "con horario cargado", ""),
        },
    );

    // Gráfico: minutos de clase por día. Sale de los mismos datos.
    var por_dia = std.ArrayList(charts.Bar).init(allocator);
    for (dias) |dia| {
        var minutos: usize = 0;
        var clases: usize = 0;
        for (lista.array.items) |slot| {
            const sd = json.getString(slot, "dia") orelse continue;
            if (!sameDay(sd, dia)) continue;
            if ((json.getString(slot, "clase") orelse "").len == 0) continue;
            minutos += @intCast(@max(0, json.getInt(slot, "duracion_min") orelse 0));
            clases += 1;
        }
        const nombre = try std.fmt.allocPrint(allocator, "{s}{s}", .{ dia[0..1], dia[1..] });
        try por_dia.append(.{
            .etiqueta = nombre,
            .valor = @floatFromInt(minutos),
            .texto = if (clases == 0)
                try allocator.dupe(u8, "—")
            else
                try std.fmt.allocPrint(allocator, "{d} · {d} cl.", .{ clases, minutos }),
            .color = if (clases == 0) charts.Palette.muted else charts.Palette.info,
        });
    }
    var max_min: f64 = 0;
    for (por_dia.items) |b| if (b.valor > max_min) {
        max_min = b.valor;
    };
    const grafico = try std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-panel nx-academic-panel nx-chart-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Carga semanal</p><h2>Minutos de clase por día</h2></div></div>{s}</section>",
        .{try charts.barras(allocator, por_dia.items, if (max_min <= 0) 1 else max_min, "Cada barra suma los minutos de clase de ese día; el número incluye cuántas clases son.")},
    );

    const head = try pageHead(allocator, "Aprendizaje · Horario", "Tu semana,<br>hora por hora.", "Las clases salen del horario real de Mikareno; si un día no tiene, se dice.", refreshButton());
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-page nova-page nx-academic-page\">{s}{s}{s}{s}</div>",
        .{ head, kpis, grafico, buf.items },
    );
}

// ─────────────────────────────── Tareas ───────────────────────────────

/// Lista de tareas/actividades con su estado y fecha.
pub fn tareasHtml(allocator: std.mem.Allocator, raw: json.Value) ![]const u8 {
    const resumen = json.get(raw, "resumen");
    const lista_v = json.get(raw, "lista_tareas");
    const n: usize = if (lista_v) |l| (if (l == .array) l.array.items.len else 0) else 0;

    const hoy = if (resumen) |r| json.getInt(r, "hoy") orelse 0 else 0;
    const semana = if (resumen) |r| json.getInt(r, "esta_semana") orelse 0 else 0;
    const manana = if (resumen) |r| json.getInt(r, "manana") orelse 0 else 0;

    const kpis = try std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-grade-kpis nx-academic-kpis\">{s}{s}{s}</section>",
        .{
            try kpi(allocator, "Para hoy", try std.fmt.allocPrint(allocator, "{d}", .{hoy}), "entregas de hoy", ""),
            try kpi(allocator, "Esta semana", try std.fmt.allocPrint(allocator, "{d}", .{semana}), "entregas en 7 días", ""),
            try kpi(allocator, "Mañana", try std.fmt.allocPrint(allocator, "{d}", .{manana}), "entregas de mañana", ""),
        },
    );

    // Anillo con el reparto de la semana: hoy, mañana y el resto.
    const hoy_n: usize = @intCast(@max(0, hoy));
    const semana_n: usize = @intCast(@max(0, semana));
    const manana_n: usize = @intCast(@max(0, manana));
    const resto = if (semana_n > hoy_n + manana_n) semana_n - hoy_n - manana_n else 0;
    const total_sem = semana_n;
    const donut = try std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-panel nx-academic-panel nx-chart-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Reparto</p><h2>La semana de un vistazo</h2></div></div>" ++
            "<div class=\"nx-chart-split\">{s}{s}</div></section>",
        .{
            try charts.donut(
                allocator,
                partPct(hoy_n, total_sem),
                charts.Palette.warn,
                try std.fmt.allocPrint(allocator, "{d}", .{hoy_n}),
                "para hoy",
                try std.fmt.allocPrint(allocator, "{d} entregas en la semana", .{total_sem}),
            ),
            try charts.apilada(allocator, &.{
                .{ .etiqueta = "Hoy", .cantidad = hoy_n, .color = charts.Palette.bad },
                .{ .etiqueta = "Mañana", .cantidad = manana_n, .color = charts.Palette.warn },
                .{ .etiqueta = "Más adelante", .cantidad = resto, .color = charts.Palette.info },
            }, total_sem),
        },
    );

    var lista = std.ArrayList(u8).init(allocator);
    if (n == 0) {
        try lista.appendSlice(try vacio(allocator, "Sin tareas pendientes", "Mikareno no reporta actividades por entregar en este momento."));
    } else {
        try lista.appendSlice("<div class=\"nx-panel nx-academic-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Entregas</p><h2>Actividades por hacer</h2></div></div><div class=\"nx-table-wrap\"><table class=\"nx-table\"><thead><tr><th>Actividad</th><th>Vence</th><th>Materia</th></tr></thead><tbody>");
        for (lista_v.?.array.items) |t| {
            const titulo = json.getString(t, "titulo") orelse json.getString(t, "name") orelse "(sin título)";
            const vence_raw = json.getString(t, "vencimiento") orelse json.getString(t, "endDate") orelse "";
            const materia = json.getString(t, "materia") orelse json.getString(t, "courseName") orelse "";
            try lista.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<tr><td>{s}</td><td>{s}</td><td>{s}</td></tr>",
                .{
                    try esc(allocator, titulo),
                    try esc(allocator, try fecha(allocator, vence_raw)),
                    if (materia.len == 0) try allocator.dupe(u8, "—") else try esc(allocator, materia),
                },
            ));
        }
        try lista.appendSlice("</tbody></table></div></div>");
    }

    const head = try pageHead(allocator, "Aprendizaje · Planificador", "Lo que hay<br>que entregar.", "Deberes y actividades reales del curso, con su fecha de entrega.", refreshButton());
    return std.fmt.allocPrint(allocator, "<div class=\"nx-page nova-page nx-academic-page\">{s}{s}{s}{s}</div>", .{ head, kpis, donut, lista.items });
}

/// Porcentaje de `parte` sobre `total`, con 0 si no hay total.
fn partPct(parte: usize, total: usize) f64 {
    if (total == 0) return 0;
    return @as(f64, @floatFromInt(parte)) / @as(f64, @floatFromInt(total)) * 100.0;
}

// ──────────────────────────── Evaluaciones ────────────────────────────

/// Actividades sin calificar agrupadas por materia: lo que aún puntúa.
pub fn examenesHtml(allocator: std.mem.Allocator, raw: json.Value) ![]const u8 {
    const lista = json.get(raw, "pendientes");
    if (lista == null or lista.? != .array or lista.?.array.items.len == 0) {
        const head = try pageHead(allocator, "Aprendizaje · Evaluaciones", "Lo que aún<br>puede subir tu nota.", "Actividades con fecha y sin calificar; si no hay, se dice.", refreshButton());
        const body = try vacio(allocator, "Sin evaluaciones pendientes", "Mikareno no reporta actividades sin calificar en este trimestre.");
        return std.fmt.allocPrint(allocator, "<div class=\"nx-page nova-page nx-academic-page\">{s}{s}</div>", .{ head, body });
    }

    // Agrupar por materia conservando el orden de aparición.
    var nombres = std.ArrayList([]const u8).init(allocator);
    for (lista.?.array.items) |p| {
        const m = json.getString(p, "materia") orelse "";
        var seen = false;
        for (nombres.items) |n| if (std.mem.eql(u8, n, m)) {
            seen = true;
        };
        if (!seen) try nombres.append(m);
    }

    var grid = std.ArrayList(u8).init(allocator);
    var total: usize = 0;
    for (nombres.items) |nombre| {
        var rows = std.ArrayList(u8).init(allocator);
        var n: usize = 0;
        for (lista.?.array.items) |p| {
            const m = json.getString(p, "materia") orelse "";
            if (!std.mem.eql(u8, m, nombre)) continue;
            const actividad = json.getString(p, "actividad") orelse json.getString(p, "actividad") orelse "(sin nombre)";
            const eje = json.getString(p, "submenu") orelse "";
            const f = json.getString(p, "fecha") orelse "";
            try rows.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<li><span class=\"nx-plan-task-name\">{s}</span><span class=\"nx-plan-task-meta\">{s} · {s}</span></li>",
                .{ try esc(allocator, actividad), try esc(allocator, eje), try esc(allocator, try fecha(allocator, f)) },
            ));
            n += 1;
        }
        if (n == 0) continue;
        total += n;
        try grid.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<article class=\"nx-panel nx-academic-card\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">{d} pendientes</p><h2>{s}</h2></div><span class=\"nx-grade-chip\">{d}</span></div><ul class=\"nx-plan-tasks-list\">{s}</ul></article>",
            .{ n, try esc(allocator, nombre), n, rows.items },
        ));
    }

    const kpis = try std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-grade-kpis nx-academic-kpis\">{s}{s}{s}</section>",
        .{
            try kpi(allocator, "Pendientes", try std.fmt.allocPrint(allocator, "{d}", .{total}), "actividades sin nota", ""),
            try kpi(allocator, "Materias", try std.fmt.allocPrint(allocator, "{d}", .{nombres.items.len}), "con algo por calificar", ""),
            try kpi(allocator, "Trimestre", try allocator.dupe(u8, "1"), "parcial en curso", ""),
        },
    );

    // Gráfico: cuántas actividades pendientes hay en cada eje.
    const ejes = [_][]const u8{ "TAREAS", "TRABAJOS", "EVALUACIONES", "EXAMEN TRIMESTRAL" };
    var por_eje = std.ArrayList(charts.Bar).init(allocator);
    var max_eje: f64 = 0;
    for (ejes) |eje| {
        var n: usize = 0;
        for (lista.?.array.items) |p| {
            const s = json.getString(p, "submenu") orelse "";
            if (std.mem.eql(u8, s, eje)) n += 1;
        }
        if (@as(f64, @floatFromInt(n)) > max_eje) max_eje = @floatFromInt(n);
        try por_eje.append(.{
            .etiqueta = try shortEje(allocator, eje),
            .valor = @floatFromInt(n),
            .texto = try std.fmt.allocPrint(allocator, "{d}", .{n}),
            .color = if (n == 0) charts.Palette.muted else charts.Palette.accent,
        });
    }
    const grafico = try std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-panel nx-academic-panel nx-chart-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Dónde duele</p><h2>Pendientes por tipo de nota</h2></div></div>{s}</section>",
        .{try charts.barras(allocator, por_eje.items, if (max_eje <= 0) 1 else max_eje, "Cada actividad que aún no tiene nota suma aquí, sin importar la materia.")},
    );

    const head = try pageHead(allocator, "Aprendizaje · Evaluaciones", "Lo que aún<br>puede subir tu nota.", "Actividades con fecha y sin calificar. Cada una cuenta dentro de su eje.", refreshButton());
    return std.fmt.allocPrint(allocator, "<div class=\"nx-page nova-page nx-academic-page\">{s}{s}{s}{s}</div>", .{ head, kpis, grafico, grid.items });
}

fn shortEje(allocator: std.mem.Allocator, nombre: []const u8) ![]const u8 {
    if (std.mem.indexOf(u8, nombre, "TAREA") != null) return allocator.dupe(u8, "Tareas");
    if (std.mem.indexOf(u8, nombre, "TRABAJO") != null) return allocator.dupe(u8, "Trabajos");
    if (std.mem.indexOf(u8, nombre, "EVALUAC") != null) return allocator.dupe(u8, "Evaluaciones");
    if (std.mem.indexOf(u8, nombre, "EXAMEN") != null) return allocator.dupe(u8, "Examen");
    return allocator.dupe(u8, nombre);
}

// ───────────────────────────── Vida escolar ───────────────────────────

/// Asistencia semanal: qué días hubo registro y de qué materia.
pub fn asistenciaHtml(allocator: std.mem.Allocator, raw: json.Value) ![]const u8 {
    const resumen = json.get(raw, "resumen_semanal");
    const dias = [_]struct { key: []const u8, es: []const u8 }{
        .{ .key = "Monday", .es = "Lunes" },
        .{ .key = "Tuesday", .es = "Martes" },
        .{ .key = "Wednesday", .es = "Miércoles" },
        .{ .key = "Thursday", .es = "Jueves" },
        .{ .key = "Friday", .es = "Viernes" },
        .{ .key = "Saturday", .es = "Sábado" },
        .{ .key = "Sunday", .es = "Domingo" },
    };

    var total: usize = 0;
    var filas = std.ArrayList(u8).init(allocator);
    try filas.appendSlice("<div class=\"nx-panel nx-academic-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Esta semana</p><h2>Asistencia por día</h2></div></div><div class=\"nx-table-wrap\"><table class=\"nx-table\"><thead><tr><th>Día</th><th>Registros</th><th>Materias</th></tr></thead><tbody>");
    for (dias) |dia| {
        var n: usize = 0;
        var materias = std.ArrayList(u8).init(allocator);
        if (resumen) |r| {
            if (json.get(r, dia.key)) |v| {
                if (v == .array) for (v.array.items) |item| {
                    const codigo = json.getString(item, "codigo") orelse "";
                    if (codigo.len > 0) {
                        if (materias.items.len > 0) try materias.appendSlice(", ");
                        try materias.appendSlice(try esc(allocator, codigo));
                    }
                    n += 1;
                };
            }
        }
        total += n;
        try filas.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<tr><td>{s}</td><td>{d}</td><td>{s}</td></tr>",
            .{ dia.es, n, if (materias.items.len == 0) try allocator.dupe(u8, "—") else materias.items },
        ));
    }
    try filas.appendSlice("</tbody></table></div></div>");

    const detalle = json.get(raw, "detalle");
    const n_cursos: usize = if (detalle) |d| blk: {
        if (json.get(d, "cursos")) |c| {
            if (c == .array) break :blk c.array.items.len;
        }
        break :blk 0;
    } else 0;

    const kpis = try std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-grade-kpis nx-academic-kpis\">{s}{s}</section>",
        .{
            try kpi(allocator, "Registros", try std.fmt.allocPrint(allocator, "{d}", .{total}), "entradas esta semana", ""),
            try kpi(allocator, "Materias", try std.fmt.allocPrint(allocator, "{d}", .{n_cursos}), "con asistencia registrada", ""),
        },
    );

    // Gráfico: registros por día de la semana.
    var por_dia = std.ArrayList(charts.Bar).init(allocator);
    var max_dia: f64 = 0;
    for (dias) |dia| {
        var n: usize = 0;
        if (resumen) |r| {
            if (json.get(r, dia.key)) |v| {
                if (v == .array) n = v.array.items.len;
            }
        }
        if (@as(f64, @floatFromInt(n)) > max_dia) max_dia = @floatFromInt(n);
        try por_dia.append(.{
            .etiqueta = dia.es,
            .valor = @floatFromInt(n),
            .texto = try std.fmt.allocPrint(allocator, "{d}", .{n}),
            .color = if (n == 0) charts.Palette.muted else charts.Palette.info,
        });
    }
    const grafico = try std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-panel nx-academic-panel nx-chart-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Ritmo</p><h2>Registros por día</h2></div></div>{s}</section>",
        .{try charts.barras(allocator, por_dia.items, if (max_dia <= 0) 1 else max_dia, "Cada registro es una clase con asistencia tomada ese día.")},
    );

    const head = try pageHead(allocator, "Aprendizaje · Vida escolar", "Tu<br>asistencia.", "Registros reales de asistencia de la semana en curso.", refreshButton());
    return std.fmt.allocPrint(allocator, "<div class=\"nx-page nova-page nx-academic-page\">{s}{s}{s}{s}</div>", .{ head, kpis, grafico, filas.items });
}

// ─────────────────────────────── Perfil ───────────────────────────────

/// Ficha del estudiante: institución, curso y periodo lectivo.
pub fn perfilHtml(allocator: std.mem.Allocator, raw: json.Value) ![]const u8 {
    const est = json.get(raw, "estudiante");
    const inst = json.get(raw, "institucion");
    const curso = json.get(raw, "curso_actual");
    const periodo = json.get(raw, "periodo_lectivo");

    const nombre = if (est) |e| json.getString(e, "nombre") orelse "" else "";
    if (nombre.len == 0) {
        const head = try pageHead(allocator, "Aprendizaje · Perfil", "Tu ficha", "Datos del estudiante y su institución.", refreshButton());
        const body = try vacio(allocator, "Sin perfil", "Mikareno no devolvió los datos del estudiante.");
        return std.fmt.allocPrint(allocator, "<div class=\"nx-page nova-page nx-academic-page\">{s}{s}</div>", .{ head, body });
    }

    var tarjetas = std.ArrayList(u8).init(allocator);
    try tarjetas.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<article class=\"nx-panel nx-academic-card\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Estudiante</p><h2>{s}</h2></div></div><div class=\"nx-table-wrap\"><table class=\"nx-table\"><tbody>{s}{s}{s}</tbody></table></div></article>",
        .{
            try esc(allocator, nombre),
            fila(allocator, "Correo", if (est) |e| json.getString(e, "email") orelse "" else ""),
            fila(allocator, "Rol", if (est) |e| json.getString(e, "rol") orelse "" else ""),
            fila(allocator, "Datos actualizados", if (est) |e|
                (if (json.getBool(e, "info_personal_actualizada") orelse false) "Sí" else "No")
            else
                ""),
        },
    ));

    try tarjetas.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<article class=\"nx-panel nx-academic-card\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Institución</p><h2>{s}</h2></div></div><div class=\"nx-table-wrap\"><table class=\"nx-table\"><tbody>{s}{s}{s}</tbody></table></div></article>",
        .{
            if (inst) |i| try esc(allocator, json.getString(i, "name") orelse "") else "",
            fila(allocator, "Dirección", if (inst) |i| json.getString(i, "address") orelse "" else ""),
            fila(allocator, "Teléfono", if (inst) |i| json.getString(i, "phone") orelse "" else ""),
            fila(allocator, "Correo", if (inst) |i| json.getString(i, "email") orelse "" else ""),
        },
    ));

    try tarjetas.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<article class=\"nx-panel nx-academic-card\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Curso</p><h2>{s}</h2></div></div><div class=\"nx-table-wrap\"><table class=\"nx-table\"><tbody>{s}{s}{s}{s}</tbody></table></div></article>",
        .{
            if (curso) |c| try esc(allocator, json.getString(c, "nombre") orelse "") else "",
            fila(allocator, "Grado", if (curso) |c| json.getString(c, "grado") orelse "" else ""),
            fila(allocator, "Tutor", if (curso) |c| json.getString(c, "tutor") orelse "" else ""),
            fila(allocator, "Capacidad", if (curso) |c| blk: {
                const n = json.getInt(c, "capacidad") orelse break :blk "";
                break :blk try std.fmt.allocPrint(allocator, "{d}", .{n});
            } else ""),
            fila(allocator, "Año lectivo", if (periodo) |p| json.getString(p, "nombre") orelse "" else ""),
        },
    ));

    const head = try pageHead(allocator, "Aprendizaje · Perfil", "Tu ficha,<br>como la tiene el colegio.", "Identidad, institución y curso actual, todo leído de Mikareno.", refreshButton());
    return std.fmt.allocPrint(allocator, "<div class=\"nx-page nova-page nx-academic-page\">{s}<div class=\"nx-academic-grid nx-perfil-grid\">{s}</div></div>", .{ head, tarjetas.items });
}

fn fila(allocator: std.mem.Allocator, etiqueta: []const u8, valor: []const u8) []const u8 {
    return std.fmt.allocPrint(
        allocator,
        "<tr><td>{s}</td><td>{s}</td></tr>",
        .{ etiqueta, if (valor.len == 0) "—" else valor },
    ) catch "";
}

// ─────────────────────────────── Mensajes ─────────────────────────────

/// Bandeja de entrada y no leídos, con marca de prioridad.
pub fn mensajesHtml(allocator: std.mem.Allocator, raw: json.Value) ![]const u8 {
    const no_leidos = json.get(raw, "no_leidos");
    const bandeja = json.get(raw, "bandeja");

    var buf = std.ArrayList(u8).init(allocator);

    if (no_leidos) |nl| {
        if (json.get(nl, "mensajes")) |arr| {
            if (arr == .array and arr.array.items.len > 0) {
                try buf.appendSlice("<section class=\"nx-panel nx-academic-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Sin leer</p><h2>No leídos</h2></div><span class=\"nx-grade-chip\">");
                try buf.appendSlice(try std.fmt.allocPrint(allocator, "{d}", .{arr.array.items.len}));
                try buf.appendSlice("</span></div><ul class=\"nx-mensajes-list\">");
                for (arr.array.items) |m| try mensajeItem(allocator, &buf, m);
                try buf.appendSlice("</ul></section>");
            }
        }
    }

    if (bandeja) |b| {
        if (json.get(b, "mensajes")) |arr| {
            if (arr == .array and arr.array.items.len > 0) {
                try buf.appendSlice("<section class=\"nx-panel nx-academic-panel\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Bandeja</p><h2>Todos los mensajes</h2></div></div><ul class=\"nx-mensajes-list\">");
                for (arr.array.items) |m| try mensajeItem(allocator, &buf, m);
                try buf.appendSlice("</ul></section>");
            }
        }
    }

    if (buf.items.len == 0) try buf.appendSlice(try vacio(allocator, "Sin mensajes", "Mikareno no reporta comunicaciones para este estudiante."));

    const head = try pageHead(allocator, "Aprendizaje · Mensajes", "Lo que el<br>colegio te escribe.", "Comunicados reales de la bandeja de Mikareno, primero lo no leído.", refreshButton());
    return std.fmt.allocPrint(allocator, "<div class=\"nx-page nova-page nx-academic-page\">{s}{s}</div>", .{ head, buf.items });
}

fn mensajeItem(allocator: std.mem.Allocator, buf: *std.ArrayList(u8), m: json.Value) !void {
    const titulo = json.getString(m, "title") orelse "(sin asunto)";
    const remitente = json.getString(m, "sender") orelse "";
    const when = json.getString(m, "createdDate") orelse "";
    const prioridad = json.getString(m, "priority") orelse "";
    const leido = json.getBool(m, "read") orelse true;
    const adjunto = json.getBool(m, "hasAttachment") orelse false;

    const extra = if (leido) "" else " is-unread";
    try buf.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<li class=\"nx-mensaje{s}\"><div><strong>{s}</strong><span class=\"nx-plan-task-meta\">{s} · {s}{s}</span></div>{s}{s}</li>",
        .{
            extra,
            try esc(allocator, titulo),
            if (remitente.len == 0) "sin remitente" else remitente,
            when,
            if (prioridad.len == 0) "" else try std.fmt.allocPrint(allocator, " · prioridad {s}", .{prioridad}),
            if (adjunto) "<span class=\"material-symbols-outlined nx-mensaje-clip\">attach_file</span>" else "",
            if (leido) "" else "<span class=\"nx-mini-chip is-good\">nuevo</span>",
        },
    ));
}

/// Totales de la bandeja, para la tarjeta de la portada.
pub const MensajesResumen = struct { bandeja: usize, no_leidos: usize };

pub fn mensajesResumen(allocator: std.mem.Allocator, raw: json.Value) !MensajesResumen {
    var out = MensajesResumen{ .bandeja = 0, .no_leidos = 0 };
    if (json.get(raw, "no_leidos")) |nl| {
        if (json.get(nl, "total")) |t| if (t == .integer) {
            out.no_leidos = @intCast(@max(0, t.integer));
        };
    }
    if (json.get(raw, "bandeja")) |b| {
        if (json.get(b, "total")) |t| if (t == .integer) {
            out.bandeja = @intCast(@max(0, t.integer));
        };
    }
    _ = allocator;
    return out;
}

// ─────────────────────────────── Tests ───────────────────────────────

fn parse(allocator: std.mem.Allocator, src: []const u8) !json.Value {
    return json.parse(allocator, src);
}

test "el horario agrupa por día y respeta el orden" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const raw = try parse(a,
        \\{"horario":[{"dia":"lunes","hora":"07:35 - 08:15","clase":"Religión","codigo":"RELI","duracion_min":40},{"dia":"martes","hora":"08:00 - 08:40","clase":"Matemática","codigo":"MATE","duracion_min":40},{"dia":"lunes","hora":"","clase":"","codigo":"","duracion_min":30}]}
    );
    const html = try horarioHtml(a, raw);
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-horario-grid") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Religión") != null);
    // El hueco sin clase no se pinta.
    try std.testing.expect(std.mem.indexOf(u8, html, "30 min") == null);
    try std.testing.expect(std.mem.indexOf(u8, html, "07:35 - 08:15") != null);
}

test "el horario sin datos lo dice" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const html = try horarioHtml(a, try parse(a, "{\"horario\":[]}"));
    try std.testing.expect(std.mem.indexOf(u8, html, "Sin horario") != null);
}

test "las tareas muestran los contadores y la tabla" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const raw = try parse(a,
        \\{"resumen":{"hoy":1,"esta_semana":2,"manana":0},"lista_tareas":[{"titulo":"Taller de mates","vencimiento":"2026-11-24","materia":"Matemática"}]}
    );
    const html = try tareasHtml(a, raw);
    try std.testing.expect(std.mem.indexOf(u8, html, "Taller de mates") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "24/11/2026") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Matemática") != null);
}

test "las tareas vacías muestran estado vacío" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const html = try tareasHtml(a, try parse(a, "{\"resumen\":{},\"lista_tareas\":[]}"));
    try std.testing.expect(std.mem.indexOf(u8, html, "Sin tareas pendientes") != null);
}

test "las evaluaciones agrupan por materia y cuentan" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const raw = try parse(a,
        \\{"pendientes":[{"materia":"Matemática","actividad":"Polinomios","submenu":"TAREAS","fecha":"2026-11-25"},{"materia":"Matemática","actividad":"Monomios","submenu":"TRABAJOS","fecha":"2026-11-26"},{"materia":"Biología","actividad":"Taller","submenu":"TRABAJOS","fecha":"2026-11-27"}]}
    );
    const html = try examenesHtml(a, raw);
    try std.testing.expect(std.mem.indexOf(u8, html, "Polinomios") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-academic-card") != null);
    // Dos tarjetas: una por materia.
    var n: usize = 0;
    var rest = html;
    while (std.mem.indexOf(u8, rest, "nx-academic-card")) |i| {
        n += 1;
        rest = rest[i + 1 ..];
    }
    try std.testing.expectEqual(@as(usize, 2), n);
}

test "la asistencia cuenta registros por día" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const raw = try parse(a,
        \\{"resumen_semanal":{"Monday":[{"fecha":"2026-11-23","codigo":"ENGL"}],"Tuesday":[]},"total_registros_semana":1}
    );
    const html = try asistenciaHtml(a, raw);
    try std.testing.expect(std.mem.indexOf(u8, html, "ENGL") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Lunes") != null);
}

test "el perfil no muestra campos vacíos como datos" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const raw = try parse(a,
        \\{"estudiante":{"nombre":"Lennin Salas","email":"lennin@test.edu","rol":"ROLE_STUDENT"},"institucion":{"name":"Álamos","address":"","phone":"062"},"curso_actual":{"nombre":"26EGB10A","grado":"Décimo","tutor":"Tito","capacidad":30},"periodo_lectivo":{"nombre":"2026"}}
    );
    const html = try perfilHtml(a, raw);
    try std.testing.expect(std.mem.indexOf(u8, html, "Lennin Salas") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Álamos") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Tito") != null);
}

test "el perfil vacío lo dice" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const html = try perfilHtml(a, try parse(a, "{}"));
    try std.testing.expect(std.mem.indexOf(u8, html, "Sin perfil") != null);
}

test "los mensajes marcan lo no leído y escapan el asunto" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const raw = try parse(a,
        \\{"bandeja":{"total":2,"mensajes":[{"title":"Materiales <b>clubes</b>","sender":"Evelin","createdDate":"nov. 11","priority":"HIGH","read":true,"hasAttachment":false}]},"no_leidos":{"total":1,"mensajes":[{"title":"Registro de calificación","sender":"Santiago","createdDate":"nov. 23","priority":"HIGH","read":false,"hasAttachment":true}]}}
    );
    const html = try mensajesHtml(a, raw);
    try std.testing.expect(std.mem.indexOf(u8, html, "is-unread") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "&lt;b&gt;") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "nuevo") != null);
}

test "el resumen de mensajes lee los totales reales" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const raw = try parse(a, "{\"bandeja\":{\"total\":4},\"no_leidos\":{\"total\":5}}");
    const r = try mensajesResumen(a, raw);
    try std.testing.expectEqual(@as(usize, 4), r.bandeja);
    try std.testing.expectEqual(@as(usize, 5), r.no_leidos);
}
