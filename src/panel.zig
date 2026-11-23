//! Datos reales del panel de inicio y del calendario (port del antiguo
//! `stitch_panel.rs` al backend Zig).
//!
//! Reglas que respeta este módulo, igual que el original:
//!
//! * **Real o nada**: si Mikareno no responde, cada hueco lo dice. Nunca se
//!   inventa un cero ni una nota.
//! * **Un solo aviso**: el estado de conexión se muestra una vez
//!   (`syncBanner`), no repetido en cada tarjeta.
//! * **Datos compartidos**: una sola llamada `get_student_full` trae
//!   promedios, horario, calendario y tareas.

const std = @import("std");
const sqlite = @import("sqlite.zig");
const json = @import("jsonutil.zig");
const security = @import("security.zig");
const mcp_client = @import("mcp_client.zig");

pub const Evento = struct { fecha: []const u8, titulo: []const u8, tipo: []const u8 };
pub const Tarea = struct { titulo: []const u8, vencimiento: []const u8 };
pub const Nota = struct { materia: []const u8, nota: f64, trimestre: u8 };

/// Una asignatura del parcial en curso con todo lo que la vista de notas
/// necesita: código real, máximo, si es cuantitativa, si está bloqueada y
/// si Mikareno ya publicó un promedio (`tiene_nota`).
pub const SubjectGrade = struct {
    materia: []const u8,
    codigo: []const u8,
    parcial: []const u8,
    nota: f64,
    tiene_nota: bool,
    maximo: f64,
    cuantitativa: bool,
    bloqueada: bool,
};

pub const Clase = struct { dia: []const u8, etiqueta: []const u8 };

/// Todo lo que el inicio y el calendario necesitan, ya leído.
pub const Datos = struct {
    connected: bool = false,
    timeout: bool = false,
    eventos: []Evento = &.{},
    tareas: []Tarea = &.{},
    notas: []Nota = &.{},
    /// Todas las asignaturas del parcial (incluidas las que aún no tienen
    /// promedio), para la vista de notas.
    grades: []SubjectGrade = &.{},
    /// Etiqueta legible del parcial en curso (`2026 · Trimestre 3 · Parcial 1`).
    partial_label: []const u8 = "",
    horario: []Clase = &.{},
    notes_count: ?i64 = null,
    conversations_count: ?i64 = null,
    notes_activity: [8]u32 = .{0} ** 8,
    /// Contadores de `tarea_stats`; respaldo cuando la lista no llega.
    tasks_today: u32 = 0,
    tasks_week: u32 = 0,
    /// GenOffice responde al handshake MCP (lo sondea la capa web).
    genoffice_connected: bool = false,
    /// Herramientas que anuncia Mikareno ahora mismo (`tools/list`).
    mcp_tools: []const []const u8 = &.{},
    /// Configuración efectiva que la página de Ajustes muestra tal cual.
    ai_configured: bool = false,
    ai_model: []const u8 = "",
    ai_max_tokens: u32 = 0,
    ai_max_cost: f64 = 0,
    mikareno_mcp_url: []const u8 = "",
    auth_disabled: bool = false,
    /// Ruta del vault de Obsidian mostrada en el panel de notas.
    vault_path: []const u8 = "",
};

/// Modelo de IA efectivo: el elegido en la app (`app_config`) o el de `.env`.
pub fn aiModel(allocator: std.mem.Allocator, db: anytype, fallback: []const u8) []const u8 {
    const stmt = db.prepare("SELECT value FROM app_config WHERE key='ai.model'") catch return allocator.dupe(u8, fallback) catch "";
    var query = stmt;
    defer query.deinit();
    const has_row = query.next() catch false;
    if (has_row) return allocator.dupe(u8, query.text(0)) catch fallback;
    return allocator.dupe(u8, fallback) catch "";
}

/// Lee Mikareno una vez (`get_student_full`) y los contadores locales.
///
/// Si la llamada falla o se agota el tiempo, devuelve `Datos` con
/// `connected = false`: el llamador lo refleja en el aviso único.
pub fn fetch(
    allocator: std.mem.Allocator,
    db: anytype,
    mikareno_url: []const u8,
    timeout_ms: u64,
) !Datos {
    var d = Datos{};

    // Contadores locales: nunca se inventan, pero un fallo aquí no debe
    // tumbar la página.
    d.notes_count = db.scalarInt("SELECT COUNT(*) FROM notes") catch null;
    d.conversations_count = db.scalarInt("SELECT COUNT(DISTINCT subject) FROM chat_history") catch null;
    loadNotesActivity(allocator, db, &d.notes_activity);

    if (mikareno_url.len == 0) return d;

    const full = mcp_client.callTool(
        allocator,
        mikareno_url,
        timeout_ms,
        "get_student_full",
        try json.object(allocator, &.{.{ .key = "bg", .value = .{ .bool = true } }}),
        true,
    ) catch |err| {
        d.timeout = err == error.RemoteRpcError or err == error.ToolFailed;
        return d;
    };

    d.connected = true;
    const tarea_stats = parseTareaStats(full);
    d.tasks_today = tarea_stats.hoy;
    d.tasks_week = tarea_stats.semana;
    d.notas = parseNotas(allocator, full);
    d.grades = parseGrades(allocator, full);
    for (d.grades) |g| {
        if (g.parcial.len > 0) {
            d.partial_label = partialLabel(allocator, g.parcial) catch "";
            break;
        }
    }
    d.eventos = parseEventos(allocator, full);
    d.tareas = parseTareas(allocator, full);
    d.horario = parseHorario(allocator, full);
    return d;
}

/// Apuntes creados en cada una de las últimas 8 semanas (la última es la
/// actual). Un fallo deja el arreglo en ceros y el gráfico lo dice.
fn loadNotesActivity(allocator: std.mem.Allocator, db: anytype, out: *[8]u32) void {
    var stmt = db.prepare(
        "SELECT CAST((julianday('now') - julianday(created_at)) / 7 AS INTEGER) AS w, COUNT(*) AS n FROM notes GROUP BY w",
    ) catch return;
    defer stmt.deinit();
    while (stmt.next() catch false) {
        const weeks = stmt.int64(0);
        const count = stmt.int64(1);
        if (weeks < 0 or weeks > 7) continue;
        out[7 - @as(usize, @intCast(weeks))] = @intCast(@max(0, count));
    }
    _ = allocator;
}

fn dayKey(allocator: std.mem.Allocator, fecha: []const u8) ![]const u8 {
    return if (fecha.len >= 10) try allocator.dupe(u8, fecha[0..10]) else allocator.dupe(u8, fecha);
}

/// `calendario[]` → eventos con fecha ISO corta y título.
fn parseEventos(allocator: std.mem.Allocator, full: json.Value) []Evento {
    const items = json.get(full, "calendario") orelse return &.{};
    if (items != .array) return &.{};
    var out = std.ArrayList(Evento).init(allocator);
    for (items.array.items) |item| {
        const start = json.getString(item, "start") orelse continue;
        const titulo = json.getString(item, "title") orelse continue;
        if (start.len == 0 or titulo.len == 0) continue;
        const fecha = dayKey(allocator, start) catch continue;
        const tipo = json.getString(item, "dayType") orelse "";
        out.append(.{ .fecha = fecha, .titulo = titulo, .tipo = tipo }) catch {};
    }
    return out.items;
}

/// `tareas_paginadas.pageContent[]` → tareas con vencimiento.
fn parseTareas(allocator: std.mem.Allocator, full: json.Value) []Tarea {
    const paginadas = json.get(full, "tareas_paginadas") orelse return &.{};
    const page = json.get(paginadas, "pageContent") orelse return &.{};
    if (page != .array) return &.{};
    var out = std.ArrayList(Tarea).init(allocator);
    for (page.array.items) |item| {
        const titulo = json.getString(item, "titulo") orelse continue;
        if (titulo.len == 0) continue;
        const vencimiento = json.getString(item, "vencimiento") orelse "";
        out.append(.{ .titulo = titulo, .vencimiento = vencimiento }) catch {};
    }
    return out.items;
}

/// `promedios.stats` → notas por materia del parcial en curso.
fn parseNotas(allocator: std.mem.Allocator, full: json.Value) []Nota {
    const promedios = json.get(full, "promedios") orelse return &.{};
    const stats = json.get(promedios, "stats") orelse return &.{};
    if (stats != .array) return &.{};
    var out = std.ArrayList(Nota).init(allocator);
    for (stats.array.items) |item| {
        const materia = json.getString(item, "materia") orelse json.getString(item, "courseName") orelse json.getString(item, "name") orelse continue;
        const nota_v = json.get(item, "nota") orelse json.get(item, "average") orelse json.get(item, "promedio") orelse continue;
        const nota: f64 = switch (nota_v) {
            .float => |f| f,
            .integer => |i| @floatFromInt(i),
            else => continue,
        };
        const trimestre: u8 = if (json.getInt(item, "trimestre")) |t|
            @intCast(@max(0, @min(t, 255)))
        else if (json.getString(item, "partialId")) |pid|
            partialTrimester(pid)
        else
            0;
        out.append(.{ .materia = materia, .nota = nota, .trimestre = trimestre }) catch {};
    }
    return out.items;
}

/// `partialId` con forma "2026:T3:1" → 3. Devuelve 0 si no hay marcador.
fn partialTrimester(partial_id: []const u8) u8 {
    const marker = std.mem.indexOf(u8, partial_id, ":T") orelse return 0;
    const rest = partial_id[marker + 2 ..];
    const end = std.mem.indexOfScalar(u8, rest, ':') orelse rest.len;
    return std.fmt.parseInt(u8, rest[0..end], 10) catch 0;
}

/// Contadores de `tarea_stats`: llegan aunque `tareas_paginadas.pageContent`
/// venga vacío (el endpoint paginado necesita rango de fechas).
fn parseTareaStats(full: json.Value) struct { hoy: u32, semana: u32 } {
    const stats = json.get(full, "tarea_stats") orelse return .{ .hoy = 0, .semana = 0 };
    const hoy = json.getInt(stats, "todaysTasks") orelse 0;
    const semana = json.getInt(stats, "tasksCurrentWeek") orelse 0;
    return .{
        .hoy = @intCast(@max(0, @min(hoy, std.math.maxInt(u32)))),
        .semana = @intCast(@max(0, @min(semana, std.math.maxInt(u32)))),
    };
}

/// `horario.periodMap` → clases planas por día de la semana.
fn parseHorario(allocator: std.mem.Allocator, full: json.Value) []Clase {
    const horario = json.get(full, "horario") orelse return &.{};
    const map = json.get(horario, "periodMap") orelse return &.{};
    if (map != .object) return &.{};
    var out = std.ArrayList(Clase).init(allocator);
    const dias = [_]struct { key: []const u8, es: []const u8 }{
        .{ .key = "MONDAY", .es = "lunes" },
        .{ .key = "TUESDAY", .es = "martes" },
        .{ .key = "WEDNESDAY", .es = "miercoles" },
        .{ .key = "THURSDAY", .es = "jueves" },
        .{ .key = "FRIDAY", .es = "viernes" },
        .{ .key = "SATURDAY", .es = "sabado" },
        .{ .key = "SUNDAY", .es = "domingo" },
    };
    for (dias) |dia| {
        const slots = map.object.get(dia.key) orelse continue;
        if (slots != .array) continue;
        for (slots.array.items) |slot| {
            const hora = blk: {
                const s = json.get(slot, "slot") orelse break :blk "";
                break :blk json.getString(s, "prettyTime") orelse "";
            };
            const subject = json.get(slot, "subject") orelse continue;
            const name = json.getString(subject, "name") orelse "";
            const code = json.getString(subject, "code") orelse "";
            const sufijo = if (code.len > 0)
                std.fmt.allocPrint(allocator, " [{s}]", .{code}) catch ""
            else
                "";
            const etiqueta = std.fmt.allocPrint(
                allocator,
                "{s} — {s}{s}",
                .{ hora, name, sufijo },
            ) catch continue;
            out.append(.{ .dia = dia.es, .etiqueta = etiqueta }) catch {};
        }
    }
    return out.items;
}
// ── Renderers de los slots del inicio y del calendario ────────────────────

fn esc(allocator: std.mem.Allocator, s: []const u8) ![]const u8 {
    return security.htmlEscape(allocator, s);
}

fn kpi(allocator: std.mem.Allocator, value: []const u8, meta: []const u8) ![]const u8 {
    return std.fmt.allocPrint(allocator, "<strong>{s}</strong><small>{s}</small>", .{ value, meta });
}

pub fn kpiClasses(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.horario.len == 0) return kpi(allocator, "—", "sin horario");
    const nombre = todayWeekday();
    var hoy: usize = 0;
    for (d.horario) |c| {
        if (std.mem.eql(u8, c.dia, nombre)) hoy += 1;
    }
    if (hoy == 0) return kpi(allocator, "0", "hoy, sin clases");
    return std.fmt.allocPrint(allocator, "<strong>{d}</strong><small>clases hoy</small>", .{hoy});
}

pub const TaskScope = enum { today, week };

/// Entregas: la lista con fecha manda; si viene vacia se usan los contadores
/// de Mikareno (`tarea_stats`), que llegan aunque el endpoint paginado no.
pub fn taskCounts(allocator: std.mem.Allocator, d: Datos) struct { hoy: u32, semana: u32 } {
    var hoy: u32 = 0;
    if (todayIso(allocator)) |iso| {
        if (iso.len >= 10) {
            for (d.tareas) |t| {
                if (t.vencimiento.len >= 10 and std.mem.eql(u8, t.vencimiento[0..10], iso[0..10])) hoy += 1;
            }
        }
    } else |_| {}
    const semana: u32 = @intCast(@min(d.tareas.len, std.math.maxInt(u32)));
    return .{
        .hoy = if (hoy > 0) hoy else d.tasks_today,
        .semana = if (semana > 0) semana else d.tasks_week,
    };
}

pub fn kpiTasks(allocator: std.mem.Allocator, d: Datos, scope: TaskScope, label: []const u8) ![]const u8 {
    if (!d.connected) return kpi(allocator, "—", if (d.timeout) "sin respuesta" else "sin conexion");
    const counts = taskCounts(allocator, d);
    const count = switch (scope) {
        .today => counts.hoy,
        .week => counts.semana,
    };
    return std.fmt.allocPrint(allocator, "<strong>{d}</strong><small>{s}</small>", .{ count, label });
}

pub fn kpiAvg(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.notas.len == 0) return kpi(allocator, "—", "sin notas");
    var total: f64 = 0;
    for (d.notas) |n| total += n.nota;
    const avg = total / @as(f64, @floatFromInt(d.notas.len));
    return std.fmt.allocPrint(allocator, "<strong>{d:.2}</strong><small>sobre 10</small>", .{avg});
}

pub fn kpiNotes(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.notes_count) |n| return std.fmt.allocPrint(allocator, "<strong>{d}</strong><small>en el vault</small>", .{n});
    return kpi(allocator, "—", "sin dato");
}

pub fn kpiConv(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.conversations_count) |n| return std.fmt.allocPrint(allocator, "<strong>{d}</strong><small>hilos</small>", .{n});
    return kpi(allocator, "—", "sin dato");
}

pub fn trimester(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    for (d.notas) |n| if (n.trimestre > 0) return std.fmt.allocPrint(allocator, "{d}", .{n.trimestre});
    return allocator.dupe(u8, "—");
}

/// Aviso único de conexión: solo aparece si Mikareno no responde.
pub fn syncBanner(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.connected and !d.timeout) return "";
    const msg = if (d.timeout)
        "Mikareno esta tardando en responder. Las tareas y notas pueden aparecer incompletas."
    else
        "Sin conexion con Mikareno. Las tareas y notas pueden aparecer incompletas.";
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-sync-banner\" role=\"status\"><span class=\"material-symbols-outlined\">cloud_off</span><p>{s}</p><a href=\"/dashboard/mcp\">Revisar conexion<span class=\"material-symbols-outlined\">arrow_forward</span></a></div>",
        .{msg},
    );
}

pub fn syncLabel(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.connected) return allocator.dupe(u8, "Mikareno: sincronizado ahora");
    if (d.timeout) return allocator.dupe(u8, "Mikareno: no responde ahora");
    return allocator.dupe(u8, "Mikareno: sin conexion");
}

pub fn syncClasscount(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (!d.connected) return allocator.dupe(u8, "Sin conexion");
    return std.fmt.allocPrint(allocator, "{d} bloques", .{d.horario.len});
}

pub fn syncTaskcount(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (!d.connected) return allocator.dupe(u8, "Sin detalle");
    if (d.tareas.len == 0) return allocator.dupe(u8, "Sin entregas hoy");
    return std.fmt.allocPrint(allocator, "{d} pendientes", .{d.tareas.len});
}

pub fn taskFooter(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (!d.connected) return allocator.dupe(u8, "Detalle no disponible");
    if (d.tareas.len == 0) return allocator.dupe(u8, "Sin tareas activas");
    return std.fmt.allocPrint(allocator, "{d} tareas activas", .{d.tareas.len});
}

pub fn weekTitle() []const u8 {
    return "Semana actual";
}

pub fn todayLine(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const wd = todayWeekday();
    var out = std.ArrayList(u8).init(allocator);
    out.appendSlice(capitalize(allocator, wd) catch wd) catch {};
    out.appendSlice(" · Tu espacio academico") catch {};
    const tri = try trimester(allocator, d);
    if (!std.mem.eql(u8, tri, "—")) {
        out.appendSlice(" · Trimestre ") catch {};
        out.appendSlice(tri) catch {};
    }
    return out.items;
}

fn capitalize(allocator: std.mem.Allocator, s: []const u8) ![]const u8 {
    if (s.len == 0) return allocator.dupe(u8, s);
    const out = try allocator.alloc(u8, s.len);
    out[0] = std.ascii.toUpper(s[0]);
    @memcpy(out[1..], s[1..]);
    return out;
}

/// Barras de la media por asignatura, de menor a mayor: lo que necesita
/// atencion queda arriba.
pub fn subjectBars(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.notas.len == 0) {
        return allocator.dupe(u8, "<p class=\"nx-empty\">Mikareno no trajo notas de asignaturas todavia.</p>");
    }
    const idx = try allocator.alloc(usize, d.notas.len);
    for (idx, 0..) |*slot, i| slot.* = i;
    std.mem.sort(usize, idx, d.notas, struct {
        fn lt(notas: []Nota, a: usize, b: usize) bool {
            return notas[a].nota < notas[b].nota;
        }
    }.lt);

    var out = std.ArrayList(u8).init(allocator);
    for (idx) |i| {
        const n = d.notas[i];
        const pct: u32 = @intFromFloat(@floor(@max(0.0, @min(1.0, n.nota / 10.0)) * 100.0));
        const tone = if (n.nota < 4.0) "is-low" else if (n.nota < 7.0) "is-mid" else "is-high";
        const materia = try esc(allocator, n.materia);
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<div class=\"nx-subject-row\"><span class=\"nx-subject-name\">{s}</span><span class=\"nx-subject-bar {s}\"><i style=\"width:{d}%\"></i></span><span class=\"nx-subject-note\">{d:.2}</span></div>",
            .{ materia, tone, pct, n.nota },
        )) catch {};
    }
    return out.items;
}

pub fn chartEmpty(allocator: std.mem.Allocator, title: []const u8, body: []const u8, href: []const u8, cta: []const u8) ![]const u8 {
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-chart-empty\"><p>{s}</p><span>{s}</span><a href=\"{s}\">{s}<span class=\"material-symbols-outlined\">arrow_forward</span></a></div>",
        .{ title, body, href, cta },
    );
}

/// SVG de barras horizontales con la media por asignatura (0–10).
pub fn chartSubjects(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.notas.len == 0) return chartEmpty(
        allocator,
        "Sin notas todavia",
        "Conecta Mikareno para llenar este grafico con tus asignaturas.",
        "/dashboard/subjects",
        "Ver asignaturas",
    );
    const idx = try allocator.alloc(usize, d.notas.len);
    for (idx, 0..) |*slot, i| slot.* = i;
    std.mem.sort(usize, idx, d.notas, struct {
        fn lt(notas: []Nota, a: usize, b: usize) bool {
            return notas[a].nota > notas[b].nota;
        }
    }.lt);
    const shown = @min(idx.len, 7);

    const row: i32 = 24;
    const top: i32 = 6;
    const bar_x: i32 = 114;
    const bar_w: i32 = 168;
    const width = bar_x + bar_w + 30;
    const height = top * 2 + row * @as(i32, @intCast(shown));

    var out = std.ArrayList(u8).init(allocator);
    for (idx[0..shown], 0..) |i, k| {
        const n = d.notas[i];
        const y = top + @as(i32, @intCast(k)) * row;
        const frac = @max(0.0, @min(1.0, n.nota / 10.0));
        const bw: i32 = @intFromFloat(@round(@as(f64, @floatFromInt(bar_w)) * frac));
        const fill = if (n.nota < 4.0) "var(--nx-danger)" else if (n.nota < 7.0) "var(--nx-orange)" else "var(--nx-accent)";
        const name = try esc(allocator, n.materia);
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<g><title>{s}: {d:.2}</title><text class=\"nx-chart-t\" x=\"0\" y=\"{d}\" dominant-baseline=\"middle\">{s}</text><rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"7\" rx=\"3.5\" fill=\"var(--nx-surface-3)\"/><rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"7\" rx=\"3.5\" fill=\"{s}\"/><text class=\"nx-chart-v\" x=\"{d}\" y=\"{d}\" dominant-baseline=\"middle\">{d:.2}</text></g>",
            .{ name, n.nota, y + row / 2, name, bar_x, y + (row - 8) / 2, bar_w, bar_x, y + (row - 8) / 2, bw, fill, bar_x + bar_w + 8, y + row / 2, n.nota },
        )) catch {};
    }
    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 {d} {d}\" role=\"img\" aria-label=\"Media por asignatura\">{s}</svg>",
        .{ width, height, out.items },
    );
}

/// Clases y entregas por dia de la semana actual (SVG de columnas).
pub fn chartWeek(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const dias = [_]struct { label: []const u8, es: []const u8 }{
        .{ .label = "L", .es = "lunes" },
        .{ .label = "M", .es = "martes" },
        .{ .label = "X", .es = "miercoles" },
        .{ .label = "J", .es = "jueves" },
        .{ .label = "V", .es = "viernes" },
        .{ .label = "S", .es = "sabado" },
        .{ .label = "D", .es = "domingo" },
    };
    var clases = [_]i32{0} ** 7;
    var tareas = [_]i32{0} ** 7;
    for (d.horario) |c| {
        for (dias, 0..) |dia, i| {
            if (std.mem.eql(u8, c.dia, dia.es)) {
                clases[i] += 1;
                break;
            }
        }
    }
    for (d.tareas) |t| {
        if (t.vencimiento.len < 10) continue;
        const wd = weekdayOf(t.vencimiento[0..10]) orelse continue;
        for (dias, 0..) |dia, i| {
            if (std.mem.eql(u8, wd, dia.es)) {
                tareas[i] += 1;
                break;
            }
        }
    }

    var max: i32 = 0;
    for (clases) |v| max = @max(max, v);
    for (tareas) |v| max = @max(max, v);
    if (max == 0) return chartEmpty(
        allocator,
        "Sin carga esta semana",
        "Cuando haya clases o entregas, aqui veras como se reparten.",
        "/dashboard/planner",
        "Abrir planificador",
    );

    const width: i32 = 340;
    const height: i32 = 148;
    const left: i32 = 26;
    const bottom: i32 = 20;
    const top: i32 = 12;
    const plot_h = height - top - bottom;
    const plot_w = width - left - 8;
    const slot = @divTrunc(plot_w, 7);
    const bar_w: i32 = 9;
    const group_w = bar_w * 2 + 3;

    var out = std.ArrayList(u8).init(allocator);
    for ([_]f64{ 0.0, 0.5, 1.0 }) |frac| {
        const y = top + @as(i32, @intFromFloat(@floor(@as(f64, @floatFromInt(plot_h)) * frac)));
        const val: i32 = @intFromFloat(@round(@as(f64, @floatFromInt(max)) * frac));
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<line x1=\"{d}\" y1=\"{d}\" x2=\"{d}\" y2=\"{d}\" stroke=\"var(--nx-line)\" stroke-width=\"1\"/><text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\">{d}</text>",
            .{ left, y, width - 8, y, left - 6, y + 3, val },
        )) catch {};
    }
    for (dias, 0..) |dia, i| {
        const cx = left + slot * @as(i32, @intCast(i)) + @divTrunc(slot, 2);
        const gx = cx - @divTrunc(group_w, 2);
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\" text-anchor=\"middle\">{s}</text>",
            .{ cx, top + plot_h + 13, dia.label },
        )) catch {};
        const values = [_]struct { v: i32, color: []const u8 }{
            .{ .v = clases[i], .color = "var(--nx-accent)" },
            .{ .v = tareas[i], .color = "var(--nx-orange)" },
        };
        for (values, 0..) |bar, j| {
            if (bar.v <= 0) continue;
            const h: i32 = @intFromFloat(@round(@as(f64, @floatFromInt(plot_h)) * (@as(f64, @floatFromInt(bar.v)) / @as(f64, @floatFromInt(max)))));
            const x = gx + @as(i32, @intCast(j)) * (bar_w + 3);
            out.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"{d}\" rx=\"2\" fill=\"{s}\"><title>{d}</title></rect>",
                .{ x, top + plot_h - h, bar_w, h, bar.color, bar.v },
            )) catch {};
        }
    }
    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 {d} {d}\" role=\"img\" aria-label=\"Clases y entregas por dia\">{s}</svg><p class=\"nx-chart-legend\"><span class=\"is-class\"></span>Clases<span class=\"is-task\"></span>Entregas</p>",
        .{ width, height, out.items },
    );
}

/// Apuntes creados por semana en las ultimas 8 semanas.
pub fn chartNotes(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    var any: u32 = 0;
    for (d.notes_activity) |v| any = @max(any, v);
    if (any == 0) return chartEmpty(
        allocator,
        "Aun sin actividad",
        "Crea apuntes y veras aqui tu ritmo de las ultimas 8 semanas.",
        "/dashboard/notes",
        "Escribir una nota",
    );

    const width: i32 = 340;
    const height: i32 = 148;
    const left: i32 = 26;
    const bottom: i32 = 20;
    const top: i32 = 12;
    const plot_h = height - top - bottom;
    const plot_w = width - left - 8;
    const slot = @divTrunc(plot_w, 8);
    const bar_w = @max(6, slot - 6);

    var out = std.ArrayList(u8).init(allocator);
    for ([_]f64{ 0.0, 1.0 }) |frac| {
        const y = top + @as(i32, @intFromFloat(@floor(@as(f64, @floatFromInt(plot_h)) * frac)));
        const val: u32 = @intFromFloat(@round(@as(f64, @floatFromInt(any)) * frac));
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<line x1=\"{d}\" y1=\"{d}\" x2=\"{d}\" y2=\"{d}\" stroke=\"var(--nx-line)\" stroke-width=\"1\"/><text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\">{d}</text>",
            .{ left, y, width - 8, y, left - 6, y + 3, val },
        )) catch {};
    }
    for (d.notes_activity, 0..) |value, i| {
        if (value == 0) continue;
        const h: i32 = @intFromFloat(@round(@as(f64, @floatFromInt(plot_h)) * (@as(f64, @floatFromInt(value)) / @as(f64, @floatFromInt(any)))));
        const x = left + slot * @as(i32, @intCast(i)) + @divTrunc(slot - bar_w, 2);
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"{d}\" rx=\"2\" fill=\"var(--nx-accent)\"><title>{d} apuntes</title></rect>",
            .{ x, top + plot_h - h, bar_w, h, value },
        )) catch {};
    }
    out.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\">hace 8 sem</text><text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\" text-anchor=\"end\">hoy</text>",
        .{ left, top + plot_h + 13, width - 8, top + plot_h + 13 },
    )) catch {};
    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 {d} {d}\" role=\"img\" aria-label=\"Apuntes por semana\">{s}</svg>",
        .{ width, height, out.items },
    );
}

/// Dia de la semana en espanol a partir de una fecha ISO (`YYYY-MM-DD`).
pub fn weekdayOf(fecha: []const u8) ?[]const u8 {
    if (fecha.len < 10) return null;
    const year = std.fmt.parseInt(i32, fecha[0..4], 10) catch return null;
    const month = std.fmt.parseInt(u32, fecha[5..7], 10) catch return null;
    const day = std.fmt.parseInt(u32, fecha[8..10], 10) catch return null;
    if (month < 1 or month > 12 or day < 1 or day > 31) return null;

    // Algoritmo de Zeller: 0 = sabado, 1 = domingo, 2 = lunes...
    const q: i32 = @intCast(day);
    const m_adj: i32 = if (month < 3) @intCast(month + 12) else @intCast(month);
    const y_adj: i32 = if (month < 3) year - 1 else year;
    const k: i32 = @divTrunc(y_adj, 100);
    const j: i32 = y_adj - k * 100;
    const h: i32 = @mod(q + @divTrunc(13 * (m_adj + 1), 5) + j + @divTrunc(j, 4) + @divTrunc(k, 4) + 5 * k, 7);

    const names = [_][]const u8{ "sabado", "domingo", "lunes", "martes", "miercoles", "jueves", "viernes" };
    return names[@intCast(h)];
}

/// Dia de la semana de hoy, en espanol.
pub fn todayWeekday() []const u8 {
    // Numero de dias desde el epoch (1970-01-01 fue jueves).
    const days = @divFloor(std.time.timestamp(), 86400);
    const idx: usize = @intCast(@mod(days + 4, 7)); // 0 = domingo
    const names = [_][]const u8{ "domingo", "lunes", "martes", "miercoles", "jueves", "viernes", "sabado" };
    return names[idx];
}

// ── Tarjetas del inicio ───────────────────────────────────────────────────

/// Clases de hoy como lineas de tiempo (no tarjetas repetidas).
pub fn classesList(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const hoy = todayWeekday();
    var out = std.ArrayList(u8).init(allocator);
    var any = false;
    for (d.horario) |c| {
        if (!std.mem.eql(u8, c.dia, hoy)) continue;
        any = true;
        // "07:35 - 08:15 — Religion [RELI]" → hora arriba, materia abajo.
        const dash = std.mem.indexOf(u8, c.etiqueta, " — ") orelse c.etiqueta.len;
        const hora = c.etiqueta[0..@min(dash, c.etiqueta.len)];
        const materia = if (dash < c.etiqueta.len) c.etiqueta[dash + 3 ..] else c.etiqueta;
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<div><span>{s}</span><div><h3>{s}</h3></div></div>",
            .{ try esc(allocator, hora), try esc(allocator, materia) },
        )) catch {};
    }
    if (!any) return allocator.dupe(u8, "<div class=\"nx-empty\">Sin clases programadas para hoy.</div>");
    return out.items;
}

/// Tareas con vencimiento hoy.
pub fn tasksToday(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const hoy = todayIso(allocator) catch return allocator.dupe(u8, "<div class=\"nx-empty\">Sin fecha disponible.</div>");
    var out = std.ArrayList(u8).init(allocator);
    for (d.tareas) |t| {
        if (t.vencimiento.len < 10) continue;
        if (!std.mem.eql(u8, t.vencimiento[0..10], hoy)) continue;
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<div><input class=\"nx-task-check\" type=\"checkbox\"/><p>{s}</p><span>Mikareno</span></div>",
            .{try esc(allocator, t.titulo)},
        )) catch {};
    }
    if (out.items.len == 0) {
        return if (d.connected)
            allocator.dupe(u8, "<div class=\"nx-empty\">Sin entregas para hoy.</div>")
        else
            allocator.dupe(u8, "<div class=\"nx-empty\">Sin detalle de entregas ahora mismo.</div>");
    }
    return out.items;
}

/// Tabla de prioridades: hasta 6 tareas ordenadas por vencimiento.
pub fn taskTable(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.tareas.len == 0) {
        const msg = if (!d.connected)
            "Sin detalle de tareas ahora mismo."
        else
            "Sin tareas pendientes.";
        return std.fmt.allocPrint(
            allocator,
            "<tr><td colspan=\"6\" class=\"py-4 text-center nx-empty\">{s}</td></tr>",
            .{msg},
        );
    }
    const idx = try allocator.alloc(usize, d.tareas.len);
    for (idx, 0..) |*slot, i| slot.* = i;
    std.mem.sort(usize, idx, d.tareas, struct {
        fn lt(tareas: []Tarea, a: usize, b: usize) bool {
            return std.mem.lessThan(u8, tareas[a].vencimiento, tareas[b].vencimiento);
        }
    }.lt);

    var out = std.ArrayList(u8).init(allocator);
    for (idx[0..@min(idx.len, 6)]) |i| {
        const t = d.tareas[i];
        const limite = if (t.vencimiento.len >= 10) t.vencimiento[0..10] else "—";
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<tr><td><span class=\"nx-badge\">Pendiente</span></td><td class=\"truncate\">{s}</td><td>Mikareno</td><td>{s}</td><td>—</td><td></td></tr>",
            .{ try esc(allocator, t.titulo), try esc(allocator, limite) },
        )) catch {};
    }
    return out.items;
}

/// Notas recientes del vault local.
pub fn notesList(allocator: std.mem.Allocator, db: anytype) ![]const u8 {
    var stmt = db.prepare("SELECT title, slug FROM notes ORDER BY updated_at DESC LIMIT 3") catch {
        return "<div class=\"nx-empty\">No se pudieron leer las notas.</div>";
    };
    defer stmt.deinit();
    var out = std.ArrayList(u8).init(allocator);
    while (stmt.next() catch false) {
        const title_raw = stmt.text(0);
        const slug = stmt.text(1);
        const title = if (title_raw.len == 0) "Sin titulo" else title_raw;
        if (slug.len == 0) {
            out.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<div><span class=\"material-symbols-outlined\">draft</span><span class=\"truncate\">{s}</span></div>",
                .{try esc(allocator, title)},
            )) catch {};
        } else {
            out.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<div><span class=\"material-symbols-outlined\">draft</span><a href=\"/app/notes/{s}\">{s}</a><span class=\"nx-badge\">nota</span></div>",
                .{ slug, try esc(allocator, title) },
            )) catch {};
        }
    }
    if (out.items.len == 0) {
        return allocator.dupe(u8, "<div class=\"nx-empty\">Todavia no hay notas. <a class=\"nx-text-link\" href=\"/app/notes/new\">Crear la primera</a>.</div>");
    }
    return out.items;
}

/// Tira de la semana actual: los 7 dias con su numero real, el de hoy
/// marcado y los que tienen clase senalados.
pub fn weekCal(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const hoy = try todayIso(allocator);
    const idx = weekdayIndex(todayWeekday());

    // Lunes de esta semana, retrocediendo desde hoy.
    const year = std.fmt.parseInt(i32, hoy[0..4], 10) catch 2026;
    const month = std.fmt.parseInt(u32, hoy[5..7], 10) catch 1;
    const day = std.fmt.parseInt(u32, hoy[8..10], 10) catch 1;
    const base = daysFromCivil(year, month, day) - @as(i64, @intCast(idx));

    const dias = [_][]const u8{ "lunes", "martes", "miercoles", "jueves", "viernes", "sabado", "domingo" };
    const labels = [_][]const u8{ "L", "M", "X", "J", "V", "S", "D" };

    var out = std.ArrayList(u8).init(allocator);
    for (0..7) |i| {
        const fecha_dia = civilFromDays(base + @as(i64, @intCast(i)));
        const y = fecha_dia.year;
        const m = fecha_dia.month;
        const dd = fecha_dia.day;
        var has_class = false;
        for (d.horario) |c| {
            if (std.mem.eql(u8, c.dia, dias[i])) {
                has_class = true;
                break;
            }
        }
        var cls: []const u8 = "nx-cal-day";
        if (i == idx) cls = "nx-cal-day nx-cal-today";
        if (has_class) cls = if (i == idx) "nx-cal-day nx-cal-today has-class" else "nx-cal-day has-class";
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<div class=\"{s}\" title=\"{d:0>4}-{d:0>2}-{d:0>2}\"><span>{s}</span><span>{d}</span></div>",
            .{ cls, @as(u32, @intCast(y)), m, dd, labels[i], dd },
        )) catch {};
    }
    return out.items;
}

/// Dias transcurridos desde el epoch civil (1970-01-01) para una fecha.
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

/// Fecha civil (year, month, day) a partir de dias desde el epoch.
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

/// Proximo evento del calendario, o un estado vacio con enlace.
pub fn nextEvent(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const hoy = todayIso(allocator) catch "";
    var best: ?Evento = null;
    for (d.eventos) |e| {
        if (hoy.len == 10 and std.mem.lessThan(u8, e.fecha, hoy)) continue;
        if (best == null or std.mem.lessThan(u8, e.fecha, best.?.fecha)) best = e;
    }
    if (best) |e| {
        const tipo = if (std.mem.eql(u8, e.tipo, "SCHOOL_DAY"))
            "Dia de clases"
        else if (std.mem.eql(u8, e.tipo, "NON_SCHOOL_DAY"))
            "Dia no lectivo"
        else
            "Evento del calendario";
        return std.fmt.allocPrint(
            allocator,
            "<section class=\"nx-card nx-card-next\"><header class=\"nx-card-head\"><div><p class=\"nx-eyebrow\">Proximo en el calendario</p><h2>{s}</h2></div><span class=\"nx-badge\">{s}</span></header><p class=\"nx-card-next-meta\">{s}</p></section>",
            .{ try esc(allocator, e.titulo), try esc(allocator, e.fecha), tipo },
        );
    }
    return allocator.dupe(u8, "<section class=\"nx-card nx-card-next\"><header class=\"nx-card-head\"><div><p class=\"nx-eyebrow\">Proximo en el calendario</p><h2>Sin eventos proximos</h2></div></header><p class=\"nx-card-next-meta\"><a class=\"nx-text-link\" href=\"/dashboard/calendar\">Abrir calendario<span class=\"material-symbols-outlined\">arrow_forward</span></a></p></section>");
}

/// Tarjeta de examen: proxima evaluacion si el calendario trae una.
pub fn examCard(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const hoy = todayIso(allocator) catch "";
    for (d.eventos) |e| {
        if (std.mem.indexOf(u8, e.titulo, "xamen") == null and std.mem.indexOf(u8, e.titulo, "valuacion") == null) continue;
        if (hoy.len == 10 and std.mem.lessThan(u8, e.fecha, hoy)) continue;
        return std.fmt.allocPrint(
            allocator,
            "<div class=\"nx-card\"><p class=\"nx-eyebrow\">Proxima evaluacion</p><h2 class=\"nx-card-title\">{s}</h2><p class=\"nx-card-next-meta\">{s}</p><a class=\"nx-text-link\" href=\"/dashboard/examenes\">Ver evaluaciones<span class=\"material-symbols-outlined\">arrow_forward</span></a></div>",
            .{ try esc(allocator, e.titulo), try esc(allocator, e.fecha) },
        );
    }
    return allocator.dupe(u8, "<div class=\"nx-card\"><p class=\"nx-eyebrow\">Proxima evaluacion</p><p class=\"nx-empty\">Sin evaluaciones proximas en Mikareno.</p><a class=\"nx-text-link\" href=\"/dashboard/examenes\">Ver evaluaciones<span class=\"material-symbols-outlined\">arrow_forward</span></a></div>");
}

/// Opciones de asignatura para el formulario de captura rapida.
pub fn subjectOptions(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.notas.len == 0) return allocator.dupe(u8, "<option>Sin asignaturas</option>");
    var out = std.ArrayList(u8).init(allocator);
    var seen = std.StringHashMap(void).init(allocator);
    for (d.notas) |n| {
        if (seen.contains(n.materia)) continue;
        seen.put(n.materia, {}) catch {};
        out.appendSlice(try std.fmt.allocPrint(allocator, "<option>{s}</option>", .{try esc(allocator, n.materia)})) catch {};
    }
    return out.items;
}

// ── Calendario mensual ────────────────────────────────────────────────────

const mes_nombres = [_][]const u8{
    "Enero", "Febrero", "Marzo",      "Abril",   "Mayo",      "Junio",
    "Julio", "Agosto",  "Septiembre", "Octubre", "Noviembre", "Diciembre",
};

fn diasDelMes(year: i32, month: u32) u32 {
    const bisiesto = @mod(year, 4) == 0 and (@mod(year, 100) != 0 or @mod(year, 400) == 0);
    return switch (month) {
        1, 3, 5, 7, 8, 10, 12 => 31,
        4, 6, 9, 11 => 30,
        2 => if (bisiesto) 29 else 28,
        else => 30,
    };
}

/// Fecha ISO de hoy en UTC.
pub fn todayIso(allocator: std.mem.Allocator) ![]const u8 {
    const days = @divFloor(std.time.timestamp(), 86400);
    const epoch = std.time.epoch.EpochSeconds{ .secs = @intCast(days * 86400) };
    const year_day = epoch.getEpochDay().calculateYearDay();
    const md = year_day.calculateMonthDay();
    return std.fmt.allocPrint(allocator, "{d:0>4}-{d:0>2}-{d:0>2}", .{
        year_day.year,
        md.month.numeric(),
        md.day_index + 1,
    });
}

fn weekdayIndex(nombre: []const u8) usize {
    const dias = [_][]const u8{ "domingo", "lunes", "martes", "miercoles", "jueves", "viernes", "sabado" };
    for (dias, 0..) |d, i| if (std.mem.eql(u8, d, nombre)) return (i + 6) % 7; // lunes = 0
    return 0;
}

/// Dias del mes con al menos un bloque de horario (los del horario semanal).
pub fn calDaysWithClass(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    var n: usize = 0;
    for (d.horario) |c| {
        var seen = false;
        for (d.horario[0..n]) |prev| {
            if (std.mem.eql(u8, prev.dia, c.dia)) {
                seen = true;
                break;
            }
        }
        if (!seen) n += 1;
    }
    return std.fmt.allocPrint(allocator, "{d}", .{n});
}

fn inMonth(fecha: []const u8, first: []const u8, last: []const u8) bool {
    if (fecha.len < 10) return false;
    const dia = fecha[0..10];
    return std.mem.order(u8, dia, first) != .lt and std.mem.order(u8, dia, last) != .gt;
}

/// Entregas del mes actual.
pub fn calTasksMonth(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const hoy = try todayIso(allocator);
    const first = try std.fmt.allocPrint(allocator, "{s}-01", .{hoy[0..7]});
    const last = try std.fmt.allocPrint(allocator, "{s}-31", .{hoy[0..7]});
    var n: usize = 0;
    for (d.tareas) |t| if (inMonth(t.vencimiento, first, last)) {
        n += 1;
    };
    return std.fmt.allocPrint(allocator, "{d}", .{n});
}

/// Eventos del mes actual.
pub fn calEventsMonth(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const hoy = try todayIso(allocator);
    const first = try std.fmt.allocPrint(allocator, "{s}-01", .{hoy[0..7]});
    const last = try std.fmt.allocPrint(allocator, "{s}-31", .{hoy[0..7]});
    var n: usize = 0;
    for (d.eventos) |e| if (inMonth(e.fecha, first, last)) {
        n += 1;
    };
    return std.fmt.allocPrint(allocator, "{d}", .{n});
}

/// Proximos eventos, ordenados por fecha (maximo 5).
pub fn calUpcoming(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const hoy = try todayIso(allocator);
    const idx = try allocator.alloc(usize, d.eventos.len);
    for (idx, 0..) |*slot, i| slot.* = i;
    std.mem.sort(usize, idx, d.eventos, struct {
        fn lt(eventos: []Evento, a: usize, b: usize) bool {
            return std.mem.lessThan(u8, eventos[a].fecha, eventos[b].fecha);
        }
    }.lt);

    var out = std.ArrayList(u8).init(allocator);
    var shown: usize = 0;
    for (idx) |i| {
        if (shown >= 5) break;
        const e = d.eventos[i];
        if (std.mem.lessThan(u8, e.fecha, hoy)) continue;
        shown += 1;
        const tipo = if (std.mem.eql(u8, e.tipo, "NON_SCHOOL_DAY")) "No lectivo" else "Evento";
        const tone = if (std.mem.eql(u8, e.tipo, "NON_SCHOOL_DAY")) "non-school" else "event";
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<div class=\"nx-cal-up-item\"><span class=\"nx-cal-up-date\">{s}</span><span class=\"nx-cal-up-title\">{s}</span><span class=\"nx-cal-up-badge {s}\">{s}</span></div>",
            .{ if (e.fecha.len >= 10) e.fecha[8..10] else e.fecha, try esc(allocator, e.titulo), tone, tipo },
        )) catch {};
    }
    if (shown == 0) return allocator.dupe(u8, "<p class=\"nx-cal-up-empty\">Sin eventos proximos</p>");
    return out.items;
}

/// Celdas vacias antes del dia 1 del mes (lunes = 0).
fn monthOffset(year: i32, month: u32) usize {
    // Ojo: {d:0>4} sobre un entero con signo imprime "+2026"; hay que pasar a
    // sin signo para que la fecha ISO sea parseable.
    var buf: [16]u8 = undefined;
    const iso = std.fmt.bufPrint(&buf, "{d:0>4}-{d:0>2}-01", .{ @as(u32, @intCast(year)), month }) catch return 0;
    return weekdayIndex(weekdayOf(iso) orelse "lunes");
}

/// Rejilla del mes: 7 columnas x 6 semanas con chips por dia.
pub fn calGrid(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const hoy = try todayIso(allocator);
    const year = std.fmt.parseInt(i32, hoy[0..4], 10) catch 2026;
    const month = std.fmt.parseInt(u32, hoy[5..7], 10) catch 1;
    const total = diasDelMes(year, month);
    const offset = monthOffset(year, month);

    var out = std.ArrayList(u8).init(allocator);
    out.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-cal-nav\"><a class=\"nx-cal-nav-btn\" href=\"/dashboard/calendar\" aria-label=\"Mes anterior\"><span class=\"material-symbols-outlined\">chevron_left</span></a><div class=\"nx-cal-title\"><span class=\"nx-cal-month\">{s} {d}</span><span class=\"nx-cal-badge\">Mes actual</span></div><a class=\"nx-cal-nav-btn\" href=\"/dashboard/calendar\" aria-label=\"Mes siguiente\"><span class=\"material-symbols-outlined\">chevron_right</span></a></div>",
        .{ mes_nombres[@intCast(month - 1)], year },
    )) catch {};

    out.appendSlice("<div class=\"nx-cal-weekdays\">") catch {};
    for ([_][]const u8{ "L", "M", "X", "J", "V", "S", "D" }) |label| {
        out.appendSlice(try std.fmt.allocPrint(allocator, "<div>{s}</div>", .{label})) catch {};
    }
    out.appendSlice("</div><div class=\"nx-cal-days\">") catch {};

    for (0..offset) |_| out.appendSlice("<div class=\"nx-cal-day nx-cal-outside\"></div>") catch {};

    var day: u32 = 1;
    while (day <= total) : (day += 1) {
        const iso = try std.fmt.allocPrint(allocator, "{d:0>4}-{d:0>2}-{d:0>2}", .{ @as(u32, @intCast(year)), month, day });
        const is_today = std.mem.eql(u8, iso, hoy);
        var cls: []const u8 = "nx-cal-day";
        if (is_today) cls = "nx-cal-day nx-cal-today";
        out.appendSlice(try std.fmt.allocPrint(allocator, "<div class=\"{s}\">", .{cls})) catch {};
        out.appendSlice(try std.fmt.allocPrint(allocator, "<span class=\"nx-cal-day-num\"{s}>{d}</span>", .{ if (is_today) " aria-current=\"date\"" else "", day })) catch {};

        var chips = std.ArrayList(u8).init(allocator);
        for (d.horario) |c| {
            const wd = weekdayOf(iso) orelse continue;
            if (!std.mem.eql(u8, wd, c.dia)) continue;
            chips.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<span class=\"nx-cal-chip class\" title=\"{s}\">{s}</span>",
                .{ try esc(allocator, c.etiqueta), try esc(allocator, c.etiqueta) },
            )) catch {};
            break; // un chip de horario por dia basta
        }
        for (d.eventos) |e| {
            if (!std.mem.eql(u8, e.fecha, iso)) continue;
            const tone = if (std.mem.eql(u8, e.tipo, "NON_SCHOOL_DAY")) "event non-school" else "event";
            chips.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<span class=\"nx-cal-chip {s}\" title=\"{s}\">{s}</span>",
                .{ tone, try esc(allocator, e.titulo), try esc(allocator, e.titulo) },
            )) catch {};
        }
        for (d.tareas) |t| {
            if (t.vencimiento.len < 10) continue;
            if (!std.mem.eql(u8, t.vencimiento[0..10], iso)) continue;
            chips.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<span class=\"nx-cal-chip task\" title=\"{s}\">{s}</span>",
                .{ try esc(allocator, t.titulo), try esc(allocator, t.titulo) },
            )) catch {};
        }
        if (chips.items.len > 0) {
            out.appendSlice("<div class=\"nx-cal-chips\">") catch {};
            out.appendSlice(chips.items) catch {};
            out.appendSlice("</div>") catch {};
        }
        out.appendSlice("</div>") catch {};
    }

    const used = offset + total;
    const trailing = if (used < 42) 42 - used else 0;
    for (0..trailing) |_| out.appendSlice("<div class=\"nx-cal-day nx-cal-outside\"></div>") catch {};

    out.appendSlice("</div>") catch {};
    return out.items;
}

test "weekdayOf mapea fechas ISO conocidas" {
    // 2026-11-20 fue viernes; 2026-11-21, sabado.
    try std.testing.expectEqualStrings("viernes", weekdayOf("2026-11-20").?);
    try std.testing.expectEqualStrings("sabado", weekdayOf("2026-11-21").?);
    try std.testing.expectEqualStrings("domingo", weekdayOf("2026-11-22").?);
    try std.testing.expectEqualStrings("lunes", weekdayOf("2026-11-23").?);
    // Fechas basura no inventan dia.
    try std.testing.expect(weekdayOf("") == null);
    try std.testing.expect(weekdayOf("no-es-fecha") == null);
    try std.testing.expect(weekdayOf("2026-13-01") == null);
}

test "daysFromCivil y civilFromDays son inversos" {
    const casos = [_]struct { y: i32, m: u32, d: u32 }{
        .{ .y = 1970, .m = 1, .d = 1 },
        .{ .y = 2026, .m = 11, .d = 20 },
        .{ .y = 2024, .m = 2, .d = 29 },
        .{ .y = 2000, .m = 12, .d = 31 },
    };
    for (casos) |c| {
        const dias = daysFromCivil(c.y, c.m, c.d);
        const vuelta = civilFromDays(dias);
        try std.testing.expectEqual(c.y, vuelta.year);
        try std.testing.expectEqual(c.m, vuelta.month);
        try std.testing.expectEqual(c.d, vuelta.day);
    }
}

test "diasDelMes respeta bisiestos" {
    try std.testing.expectEqual(@as(u32, 29), diasDelMes(2024, 2));
    try std.testing.expectEqual(@as(u32, 28), diasDelMes(2026, 2));
    try std.testing.expectEqual(@as(u32, 31), diasDelMes(2026, 1));
    try std.testing.expectEqual(@as(u32, 30), diasDelMes(2026, 4));
}

test "los datos vacios no inventan numeros" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const d = Datos{}; // sin conexion y sin datos

    // Sin conexion, los KPIs dicen que no hay dato, no un 0 falso.
    const tareas = try kpiTasks(a, d, .today, "pendientes");
    try std.testing.expect(std.mem.indexOf(u8, tareas, "sin conexion") != null);
    const media = try kpiAvg(a, d);
    try std.testing.expect(std.mem.indexOf(u8, media, "sin notas") != null);
    const clases = try kpiClasses(a, d);
    try std.testing.expect(std.mem.indexOf(u8, clases, "sin horario") != null);

    // Sin conexion, hay aviso unico.
    const banner = try syncBanner(a, d);
    try std.testing.expect(std.mem.indexOf(u8, banner, "nx-sync-banner") != null);
    // Con conexion y sin timeout, no hay aviso.
    const ok = Datos{ .connected = true };
    try std.testing.expectEqualStrings("", try syncBanner(a, ok));

    // Los graficos vacios llevan enlace, no un SVG en blanco.
    const chart = try chartNotes(a, d);
    try std.testing.expect(std.mem.indexOf(u8, chart, "nx-chart-empty") != null);
    try std.testing.expect(std.mem.indexOf(u8, chart, "<svg") == null);
}

test "el aviso de timeout se distingue del de sin conexion" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const timeout = try syncBanner(a, Datos{ .timeout = true });
    try std.testing.expect(std.mem.indexOf(u8, timeout, "tardando en responder") != null);
    const caido = try syncBanner(a, Datos{});
    try std.testing.expect(std.mem.indexOf(u8, caido, "Sin conexion") != null);
}

test "las barras de asignaturas ordenan de peor a mejor" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var notas = [_]Nota{
        .{ .materia = "Buena", .nota = 9.0, .trimestre = 1 },
        .{ .materia = "Mala", .nota = 3.0, .trimestre = 1 },
        .{ .materia = "Media", .nota = 6.0, .trimestre = 1 },
    };
    const html = try subjectBars(a, Datos{ .notas = &notas });
    const pos_mala = std.mem.indexOf(u8, html, "Mala").?;
    const pos_media = std.mem.indexOf(u8, html, "Media").?;
    const pos_buena = std.mem.indexOf(u8, html, "Buena").?;
    try std.testing.expect(pos_mala < pos_media and pos_media < pos_buena);
    // El tono sigue el umbral real.
    try std.testing.expect(std.mem.indexOf(u8, html, "is-low") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "is-high") != null);
}

test "el SVG de asignaturas lleva una barra por materia" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var notas = [_]Nota{
        .{ .materia = "A", .nota = 5.0, .trimestre = 1 },
        .{ .materia = "B", .nota = 8.0, .trimestre = 1 },
    };
    const svg = try chartSubjects(a, Datos{ .notas = &notas });
    try std.testing.expect(std.mem.indexOf(u8, svg, "<svg") != null);
    try std.testing.expect(std.mem.indexOf(u8, svg, ">A<") != null);
    try std.testing.expect(std.mem.indexOf(u8, svg, ">B<") != null);
}

test "el grafico semanal cuenta clases y entregas por dia" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var horario = [_]Clase{
        .{ .dia = "lunes", .etiqueta = "07:35 - 08:15 — Matematicas [MAT]" },
        .{ .dia = "lunes", .etiqueta = "08:20 - 09:00 — Historia [HIS]" },
    };
    var tareas = [_]Tarea{
        .{ .titulo = "Informe", .vencimiento = "2026-11-23" }, // lunes
    };
    const svg = try chartWeek(a, Datos{ .horario = &horario, .tareas = &tareas });
    try std.testing.expect(std.mem.indexOf(u8, svg, "<svg") != null);
    // Leyenda de las dos series.
    try std.testing.expect(std.mem.indexOf(u8, svg, "is-class") != null);
    try std.testing.expect(std.mem.indexOf(u8, svg, "is-task") != null);
}

test "sin carga semanal el grafico lo dice en vez de dibujar vacio" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const svg = try chartWeek(a, Datos{});
    try std.testing.expect(std.mem.indexOf(u8, svg, "nx-chart-empty") != null);
    try std.testing.expect(std.mem.indexOf(u8, svg, "<svg") == null);
}

test "las clases de hoy se pintan como lineas con hora y materia" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const hoy = todayWeekday();
    var horario = [_]Clase{
        .{ .dia = hoy, .etiqueta = "07:35 - 08:15 — Religion [RELI]" },
    };
    const html = try classesList(a, Datos{ .horario = &horario });
    try std.testing.expect(std.mem.indexOf(u8, html, "Religion [RELI]") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "07:35") != null);
}

test "el offset del mes usa el dia real (sin el signo + de los enteros con signo)" {
    // 2026-11-01 fue domingo: 6 celdas vacias antes (lunes = 0).
    try std.testing.expectEqual(@as(usize, 6), monthOffset(2026, 11));
    // 2026-06-01 fue lunes.
    try std.testing.expectEqual(@as(usize, 0), monthOffset(2026, 6));
    // 2026-02-01 fue domingo.
    try std.testing.expectEqual(@as(usize, 6), monthOffset(2026, 2));
}

test "la fecha ISO generada no lleva signo y es parseable" {
    var buf: [16]u8 = undefined;
    const iso = try std.fmt.bufPrint(&buf, "{d:0>4}-{d:0>2}-01", .{ @as(u32, @intCast(@as(i32, 2026))), @as(u32, 11) });
    try std.testing.expectEqualStrings("2026-11-01", iso);
    try std.testing.expect(weekdayOf(iso) != null);
}

test "aiModel usa la BD y cae al valor de .env" {
    var db = try sqlite.Db.open(std.testing.allocator, ":memory:");
    defer db.close();
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    try std.testing.expectEqualStrings("fallback/model", aiModel(a, &db, "fallback/model"));
    try db.exec("INSERT INTO app_config(key,value) VALUES('ai.model','elegido/model');");
    try std.testing.expectEqualStrings("elegido/model", aiModel(a, &db, "fallback/model"));
}

// ── Vista de notas: asignaturas y detalle ────────────────────────────
//
// Portada de las páginas Rust `academic/subjects.rs` y
// `academic/subject_detail.rs`. Fuente única: Mikareno. Si no hay nota, la
// tarjeta dice "sin promedio"; nunca se inventa un cero.

fn floatField(value: json.Value, key: []const u8) ?f64 {
    const v = json.get(value, key) orelse return null;
    return switch (v) {
        .float => |f| f,
        .integer => |i| @floatFromInt(i),
        else => null,
    };
}

fn parseGrades(allocator: std.mem.Allocator, full: json.Value) []SubjectGrade {
    const promedios = json.get(full, "promedios") orelse return &.{};
    const stats = json.get(promedios, "stats") orelse return &.{};
    if (stats != .array) return &.{};
    var out = std.ArrayList(SubjectGrade).init(allocator);
    for (stats.array.items) |item| {
        if (item != .object) continue;
        const materia = json.getString(item, "courseName") orelse json.getString(item, "materia") orelse "";
        if (materia.len == 0) continue;
        const nota_v = json.get(item, "average") orelse json.get(item, "nota") orelse json.get(item, "promedio");
        var nota: f64 = 0;
        var tiene_nota = false;
        if (nota_v) |v| switch (v) {
            .float => |f| {
                nota = f;
                tiene_nota = true;
            },
            .integer => |i| {
                nota = @floatFromInt(i);
                tiene_nota = true;
            },
            else => {},
        };
        out.append(.{
            .materia = materia,
            .codigo = json.getString(item, "courseCode") orelse "",
            .parcial = json.getString(item, "partialId") orelse "",
            .nota = nota,
            .tiene_nota = tiene_nota,
            .maximo = floatField(item, "maxScore") orelse 10,
            .cuantitativa = json.getBool(item, "isAverageableQuantitative") orelse true,
            .bloqueada = json.getBool(item, "isLocked") orelse false,
        }) catch {};
    }
    return out.items;
}

/// `2026:T3:1` → `2026 · Trimestre 3 · Parcial 1`.
fn partialLabel(allocator: std.mem.Allocator, parcial: []const u8) ![]const u8 {
    var partes = std.mem.splitScalar(u8, parcial, ':');
    const year = partes.next() orelse "";
    var tri: []const u8 = "";
    var par: []const u8 = "";
    if (partes.next()) |p| tri = if (std.mem.startsWith(u8, p, "T")) p[1..] else p;
    if (partes.next()) |p| par = p;
    if (year.len == 0) return allocator.dupe(u8, "Parcial actual");
    if (tri.len > 0 and par.len > 0) return std.fmt.allocPrint(allocator, "{s} · Trimestre {s} · Parcial {s}", .{ year, tri, par });
    if (tri.len > 0) return std.fmt.allocPrint(allocator, "{s} · Trimestre {s}", .{ year, tri });
    return allocator.dupe(u8, parcial);
}

const Tone = struct { color: []const u8, label: []const u8, pct: u32 };

/// Escala 0–10: verde ≥ 7, ámbar 5–6.99, rojo < 5. El color va inline
/// para que `.nx-subject-progress > div` no lo pise con el acento global.
fn toneFor(nota: f64) Tone {
    const clamped = @max(0.0, @min(1.0, nota / 10.0));
    const pct: u32 = @intFromFloat(@floor(clamped * 100.0));
    if (nota >= 7) return .{ .color = "#2dd4bf", .label = "Aprobada", .pct = pct };
    if (nota >= 5) return .{ .color = "#fbbf24", .label = "En riesgo", .pct = pct };
    return .{ .color = "#f87171", .label = "Reprobada", .pct = pct };
}

fn lessGrade(grades: []SubjectGrade, a: usize, b: usize) bool {
    const ga = grades[a];
    const gb = grades[b];
    if (ga.tiene_nota != gb.tiene_nota) return ga.tiene_nota;
    if (ga.tiene_nota and gb.tiene_nota and ga.nota != gb.nota) return ga.nota < gb.nota;
    return std.mem.lessThan(u8, ga.materia, gb.materia);
}

fn shortCode(codigo: []const u8) []const u8 {
    if (codigo.len == 0) return "";
    if (std.mem.indexOf(u8, codigo, " - ")) |at| return codigo[0..at];
    return codigo;
}

fn urlEncodePath(allocator: std.mem.Allocator, s: []const u8) ![]const u8 {
    var out = std.ArrayList(u8).init(allocator);
    for (s) |b| {
        const keep = (b >= 'A' and b <= 'Z') or (b >= 'a' and b <= 'z') or
            (b >= '0' and b <= '9') or b == '-' or b == '_' or b == '.' or b == '~';
        if (keep) {
            try out.append(b);
        } else {
            try out.writer().print("%{X:0>2}", .{b});
        }
    }
    return out.items;
}

fn gradeCard(allocator: std.mem.Allocator, g: SubjectGrade) ![]const u8 {
    const name = try esc(allocator, g.materia);
    const href_name = try urlEncodePath(allocator, g.materia);
    const short = try esc(allocator, shortCode(g.codigo));
    const full_code = try esc(allocator, g.codigo);
    const chip = if (short.len == 0)
        ""
    else
        try std.fmt.allocPrint(allocator, "<span class=\"nx-grade-chip\" title=\"{s}\">{s}</span>", .{ full_code, short });
    if (!g.tiene_nota) {
        return std.fmt.allocPrint(
            allocator,
            "<a href=\"/dashboard/subjects/{s}\" class=\"nx-subject-card nova-subject-card\">" ++
                "<div class=\"nx-subject-top\"><span class=\"nx-subject-mark\"><span class=\"material-symbols-outlined\">menu_book</span></span>{s}<span class=\"material-symbols-outlined nx-subject-arrow\">arrow_outward</span></div>" ++
                "<h3>{s}</h3>" ++
                "<div class=\"nx-subject-grade\"><strong style=\"color:#7f8ea3\">—</strong><span>sin promedio todavía</span></div>" ++
                "<div class=\"nx-subject-progress\"><div style=\"height:100%;width:0%;background:#7f8ea3;border-radius:inherit\"></div></div>" ++
                "<footer><span>Sin calificar</span><span>Ver actividades<span class=\"material-symbols-outlined\">arrow_forward</span></span></footer></a>",
            .{ href_name, chip, name },
        );
    }
    const tone = toneFor(g.nota);
    const nota = try std.fmt.allocPrint(allocator, "{d:.2}", .{g.nota});
    const q_exam = (plan_target - g.nota) / weight_examen;
    const goal = if (g.nota >= plan_target)
        try allocator.dupe(u8, "meta de 8 cumplida")
    else if (q_exam <= 10)
        try std.fmt.allocPrint(allocator, "para 8: examen ≥ {d:.1}", .{q_exam})
    else
        try allocator.dupe(u8, "para 8: el examen solo no basta");
    return std.fmt.allocPrint(
        allocator,
        "<a href=\"/dashboard/subjects/{s}\" class=\"nx-subject-card nova-subject-card\">" ++
            "<div class=\"nx-subject-top\"><span class=\"nx-subject-mark\"><span class=\"material-symbols-outlined\">menu_book</span></span>{s}<span class=\"material-symbols-outlined nx-subject-arrow\">arrow_outward</span></div>" ++
            "<h3>{s}</h3>" ++
            "<div class=\"nx-subject-grade\"><strong style=\"color:{s}\">{s}</strong><span>{s}</span></div>" ++
            "<div class=\"nx-subject-progress\"><div style=\"height:100%;width:{d}%;background:{s};border-radius:inherit\"></div></div>" ++
            "<div class=\"nx-subject-goal\"><span class=\"material-symbols-outlined\">flag</span>{s}</div>" ++
            "<footer><span>Sobre {d:.0} puntos</span><span>Ver actividades<span class=\"material-symbols-outlined\">arrow_forward</span></span></footer></a>",
        .{ href_name, chip, name, tone.color, nota, tone.label, tone.pct, tone.color, goal, g.maximo },
    );
}

pub fn gradesGrid(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.grades.len == 0) {
        const msg = if (!d.connected)
            "No hay notas: el MCP de Mikareno no está disponible."
        else
            "Mikareno no devolvió notas para el parcial actual.";
        const hint = if (!d.connected)
            "Verifica el MCP en Integraciones y vuelve a cargar."
        else
            "Solo se muestran asignaturas con datos reales; reintenta más tarde.";
        return std.fmt.allocPrint(
            allocator,
            "<div class=\"nx-grade-empty\" id=\"subject-empty\"><span class=\"material-symbols-outlined\">school</span><p>{s}</p><p>{s}</p><a class=\"nx-button nx-button-secondary\" href=\"/dashboard/subjects\"><span class=\"material-symbols-outlined\">refresh</span>Reintentar</a></div>",
            .{ msg, hint },
        );
    }
    const idx = try allocator.alloc(usize, d.grades.len);
    for (idx, 0..) |*p, i| p.* = i;
    std.mem.sort(usize, idx, d.grades, lessGrade);
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<div class=\"nx-subject-grid nova-subject-grid\">");
    for (idx) |i| try buf.appendSlice(try gradeCard(allocator, d.grades[i]));
    try buf.appendSlice("</div>");
    return buf.items;
}

pub fn gradesKpis(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    var count: u32 = 0;
    var with_note: u32 = 0;
    var total: f64 = 0;
    var approved: u32 = 0;
    var risk: u32 = 0;
    var failed: u32 = 0;
    for (d.grades) |g| {
        count += 1;
        if (!g.tiene_nota) continue;
        with_note += 1;
        total += g.nota;
        if (g.nota >= 7) {
            approved += 1;
        } else if (g.nota >= 5) {
            risk += 1;
        } else {
            failed += 1;
        }
    }
    const avg_txt = if (with_note > 0)
        try std.fmt.allocPrint(allocator, "{d:.2}", .{total / @as(f64, @floatFromInt(with_note))})
    else
        try allocator.dupe(u8, "—");
    return std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-grade-kpis\" aria-label=\"Resumen de notas\">" ++
            "<div class=\"nx-grade-kpi\"><span>Promedio general</span><strong>{s}</strong><small>sobre 10</small></div>" ++
            "<div class=\"nx-grade-kpi\"><span>Materias</span><strong>{d}</strong><small>{d} con nota</small></div>" ++
            "<div class=\"nx-grade-kpi is-good\"><span>Aprobadas</span><strong>{d}</strong><small>7.0 o más</small></div>" ++
            "<div class=\"nx-grade-kpi is-warn\"><span>En riesgo</span><strong>{d}</strong><small>5.0 – 6.99</small></div>" ++
            "<div class=\"nx-grade-kpi is-bad\"><span>Reprobadas</span><strong>{d}</strong><small>menos de 5.0</small></div>" ++
            "</section>",
        .{ avg_txt, count, with_note, approved, risk, failed },
    );
}

pub fn gradesPageHtml(allocator: std.mem.Allocator, d: Datos, portal_url: []const u8) ![]const u8 {
    const banner = try syncBanner(allocator, d);
    const kpis = try gradesKpis(allocator, d);
    const grid = try gradesGrid(allocator, d);
    const label = if (d.partial_label.len > 0) d.partial_label else "Parcial actual";
    const safe_label = try esc(allocator, label);
    const caption = if (d.grades.len == 0)
        "Datos reales de Mikareno; si no hay notas, la página lo dice."
    else
        try std.fmt.allocPrint(allocator, "{d} asignaturas · datos reales de Mikareno · primero lo que hay que recuperar.", .{d.grades.len});
    const portal = if (std.mem.startsWith(u8, portal_url, "https://") or std.mem.startsWith(u8, portal_url, "http://"))
        try std.fmt.allocPrint(
            allocator,
            "<a class=\"nx-button nx-button-primary\" href=\"{s}\" target=\"_blank\" rel=\"noopener\" title=\"Abrir tus notas en Mikareno\"><span class=\"material-symbols-outlined\">open_in_new</span>Libreta oficial</a>",
            .{try esc(allocator, portal_url)},
        )
    else
        "";
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-page nx-subjects nova-page nova-subjects-page\">" ++
            "<header class=\"nx-page-header nova-page-head\"><div><p class=\"nx-eyebrow\">Aprendizaje · Notas</p><h1>Tu progreso,<br>materia por materia.</h1><p>{s}</p></div>" ++
            "<div class=\"nx-button-row\">" ++
            "{s}" ++
            "<a class=\"nx-button nx-button-secondary\" href=\"/dashboard/subjects\"><span class=\"material-symbols-outlined\">refresh</span>Actualizar datos</a>" ++
            "</div></header>" ++
            "{s}{s}" ++
            "<section class=\"nx-collection nova-subjects-collection\"><div class=\"nx-section-head\"><div><p class=\"nx-eyebrow\">{s}</p><h2>Progreso por asignatura</h2></div><p>Las materias con promedio más bajo aparecen primero.</p></div>{s}</section>" ++
            "</div>",
        .{ caption, portal, banner, kpis, safe_label, grid },
    );
}

// ── Detalle de una asignatura (actividades) ──────────────────────────

pub const DetailActivity = struct {
    nombre: []const u8,
    descripcion: []const u8,
    fecha: []const u8,
    nota: ?f64,
    calificada: bool,
};

pub const DetailSubmenu = struct {
    nombre: []const u8,
    total: usize,
    calificadas: usize,
    /// Suma de las notas calificadas (para el plan, sin redondeo).
    suma: f64,
    subtotal: ?f64,
    actividades: []DetailActivity,
};

pub const SubjectDetail = struct {
    connected: bool = false,
    found: bool = false,
    materia: []const u8 = "",
    codigo: []const u8 = "",
    parcial: []const u8 = "",
    nota: f64 = 0,
    tiene_nota: bool = false,
    maximo: f64 = 10,
    bloqueada: bool = false,
    cualitativa: bool = false,
    observacion: []const u8 = "",
    submenus: []DetailSubmenu = &.{},
};

fn lastSegmentInt(s: []const u8, fallback: u8) u8 {
    const at = std.mem.lastIndexOfScalar(u8, s, ':') orelse return fallback;
    return std.fmt.parseInt(u8, s[at + 1 ..], 10) catch fallback;
}

/// `2026-09-30` → `30/09/2026`. Otras formas se dejan tal cual.
fn formatDate(allocator: std.mem.Allocator, iso: []const u8) ![]const u8 {
    if (iso.len == 10 and iso[4] == '-' and iso[7] == '-') {
        var digits = true;
        for (iso[0..4]) |c| {
            if (c < '0' or c > '9') digits = false;
        }
        for (iso[5..7]) |c| {
            if (c < '0' or c > '9') digits = false;
        }
        for (iso[8..10]) |c| {
            if (c < '0' or c > '9') digits = false;
        }
        if (digits) return std.fmt.allocPrint(allocator, "{s}/{s}/{s}", .{ iso[8..10], iso[5..7], iso[0..4] });
    }
    return allocator.dupe(u8, iso);
}

fn loadSubmenus(allocator: std.mem.Allocator, url: []const u8, timeout_ms: u64, materia: []const u8, parcial: u8) ![]DetailSubmenu {
    const raw = mcp_client.callTool(allocator, url, timeout_ms, "get_actividades_materia", try json.object(allocator, &.{
        .{ .key = "materia", .value = .{ .string = materia } },
        .{ .key = "trimestre", .value = .{ .integer = @intCast(parcial) } },
        .{ .key = "bg", .value = .{ .bool = true } },
    }), true) catch return &.{};
    const materias = json.get(raw, "materias") orelse return &.{};
    if (materias != .array) return &.{};
    for (materias.array.items) |m| {
        if (!std.mem.eql(u8, json.getString(m, "materia") orelse "", materia)) continue;
        const subs = json.get(m, "submenus") orelse return &.{};
        if (subs != .array) return &.{};
        var list = std.ArrayList(DetailSubmenu).init(allocator);
        for (subs.array.items) |sm| {
            var acts = std.ArrayList(DetailActivity).init(allocator);
            var derivadas: usize = 0;
            var suma: f64 = 0;
            if (json.get(sm, "actividades")) |arr| {
                if (arr == .array) {
                    for (arr.array.items) |a| {
                        const nombre = json.getString(a, "actividad") orelse "";
                        if (nombre.len == 0) continue;
                        const nota = floatField(a, "nota");
                        const calificada = (json.getBool(a, "calificada") orelse false) or nota != null;
                        if (calificada) {
                            derivadas += 1;
                            if (nota) |n| suma += n;
                        }
                        acts.append(.{
                            .nombre = nombre,
                            .descripcion = json.getString(a, "descripcion") orelse "",
                            .fecha = try formatDate(allocator, json.getString(a, "fecha") orelse ""),
                            .nota = nota,
                            .calificada = calificada,
                        }) catch {};
                    }
                }
            }
            const total = json.getInt(sm, "total_actividades") orelse @as(i64, @intCast(acts.items.len));
            // Mikareno a veces omite `actividades_calificadas`; se derivan de
            // las filas reales para no mostrar "0 calificadas" junto a notas.
            const calif = json.getInt(sm, "actividades_calificadas") orelse 0;
            const calificadas: usize = if (calif > 0) @intCast(calif) else derivadas;
            list.append(.{
                .nombre = json.getString(sm, "submenu") orelse "",
                .total = @intCast(@max(0, total)),
                .calificadas = calificadas,
                .suma = suma,
                .subtotal = floatField(sm, "subtotal"),
                .actividades = acts.items,
            }) catch {};
        }
        return list.items;
    }
    return &.{};
}

pub fn fetchSubjectDetail(allocator: std.mem.Allocator, url: []const u8, timeout_ms: u64, query: []const u8) !SubjectDetail {
    var out = SubjectDetail{};
    if (url.len == 0 or query.len == 0 or query.len > 96) return out;
    const raw = mcp_client.callTool(allocator, url, timeout_ms, "get_detalle_materia", try json.object(allocator, &.{
        .{ .key = "materia", .value = .{ .string = query } },
        .{ .key = "bg", .value = .{ .bool = true } },
    }), true) catch return out;
    out.connected = true;
    const det = json.get(raw, "materias_detalladas") orelse return out;
    if (det != .array) return out;
    for (det.array.items) |item| {
        const name = json.getString(item, "materia") orelse "";
        if (!std.mem.eql(u8, name, query)) continue;
        out.found = true;
        out.materia = name;
        out.codigo = json.getString(item, "codigo") orelse "";
        out.parcial = json.getString(item, "partial_id") orelse "";
        if (floatField(item, "nota_promedio")) |n| {
            out.nota = n;
            out.tiene_nota = true;
        }
        out.maximo = floatField(item, "maximo") orelse 10;
        out.bloqueada = json.getBool(item, "bloqueada") orelse false;
        const tipo = json.getString(item, "tipo") orelse "";
        out.cualitativa = std.mem.eql(u8, tipo, "cualitativa");
        if (out.cualitativa) out.observacion = json.getString(item, "observacion") orelse "";
        break;
    }
    if (!out.found) return out;
    const parcial_num = lastSegmentInt(out.parcial, 1);
    out.submenus = try loadSubmenus(allocator, url, timeout_ms, out.materia, parcial_num);
    if (out.submenus.len == 0 and parcial_num != 0) {
        out.submenus = try loadSubmenus(allocator, url, timeout_ms, out.materia, 0);
    }
    return out;
}

/// Peso real de cada submenú en el parcial. Deducido del árbol de notas:
/// coincide con las 14 materias con promedio (error máx. 0.011, redondeo).
/// El EXAMEN vale 25% y, mientras no haya nota, cuenta como 0.
pub const weight_tareas: f64 = 0.20;
pub const weight_trabajos: f64 = 0.40;
pub const weight_evaluaciones: f64 = 0.15;
pub const weight_examen: f64 = 0.25;

fn containsIgnoreCase(haystack: []const u8, needle: []const u8) bool {
    if (needle.len == 0 or haystack.len < needle.len) return false;
    var i: usize = 0;
    while (i + needle.len <= haystack.len) : (i += 1) {
        var j: usize = 0;
        var ok = true;
        while (j < needle.len) : (j += 1) {
            if (std.ascii.toUpper(haystack[i + j]) != std.ascii.toUpper(needle[j])) {
                ok = false;
                break;
            }
        }
        if (ok) return true;
    }
    return false;
}

fn submenuWeight(name: []const u8) ?f64 {
    if (containsIgnoreCase(name, "TAREA")) return weight_tareas;
    if (containsIgnoreCase(name, "TRABAJO")) return weight_trabajos;
    if (containsIgnoreCase(name, "EVALUAC")) return weight_evaluaciones;
    if (containsIgnoreCase(name, "EXAMEN")) return weight_examen;
    return null;
}

/// Porcentaje que aporta cada actividad del submenú a la nota de la materia.
fn activityPercent(sm: DetailSubmenu) ?f64 {
    const w = submenuWeight(sm.nombre) orelse return null;
    if (sm.total == 0) return null;
    return (w / @as(f64, @floatFromInt(sm.total))) * 100.0;
}

fn activityRow(allocator: std.mem.Allocator, a: DetailActivity, pct: ?f64) ![]const u8 {
    const nota_txt = if (a.nota) |n| try std.fmt.allocPrint(allocator, "{d:.2}", .{n}) else try allocator.dupe(u8, "—");
    const nota_color: []const u8 = if (a.nota) |n| blk: {
        break :blk if (n >= 7) "#2dd4bf" else if (n >= 5) "#fbbf24" else "#f87171";
    } else "#7f8ea3";
    const peso_txt = if (pct) |p| try std.fmt.allocPrint(allocator, "{d:.1}%", .{p}) else try allocator.dupe(u8, "—");
    const estado = if (a.calificada)
        "<span class=\"nx-mini-chip is-good\">Calificada</span>"
    else
        "<span class=\"nx-mini-chip\">Pendiente</span>";
    return std.fmt.allocPrint(
        allocator,
        "<tr><td data-label=\"Actividad\" class=\"nx-activity-name\">{s}</td><td data-label=\"Fecha\">{s}</td><td data-label=\"Descripción\">{s}</td><td data-label=\"Peso\" class=\"nx-activity-weight\">{s}</td><td data-label=\"Nota\" style=\"font-weight:700;color:{s}\">{s}</td><td data-label=\"Estado\">{s}</td></tr>",
        .{ try esc(allocator, a.nombre), try esc(allocator, a.fecha), try esc(allocator, a.descripcion), peso_txt, nota_color, nota_txt, estado },
    );
}

fn submenuSection(allocator: std.mem.Allocator, sm: DetailSubmenu) ![]const u8 {
    const pct = activityPercent(sm);
    var rows = std.ArrayList(u8).init(allocator);
    for (sm.actividades) |a| try rows.appendSlice(try activityRow(allocator, a, pct));
    const subtotal = if (sm.subtotal) |s| try std.fmt.allocPrint(allocator, "{d:.2}", .{s}) else try allocator.dupe(u8, "—");
    const weight_txt = if (submenuWeight(sm.nombre)) |w| try std.fmt.allocPrint(allocator, " · {d:.0}% de la nota", .{w * 100.0}) else try allocator.dupe(u8, "");
    return std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-subject-activities nova-subject-activities\">" ++
            "<div class=\"nx-section-head\"><div><p class=\"nx-eyebrow\">Evaluaciones y tareas</p><h2>{s}{s}</h2></div><span>{d} actividades · {d} calificadas · subtotal {s}</span></div>" ++
            "<div class=\"nx-table-wrap\"><table class=\"nx-table nx-activity-table\"><thead><tr><th>Actividad</th><th>Fecha</th><th>Descripción</th><th>Peso</th><th>Nota</th><th>Estado</th></tr></thead><tbody>{s}</tbody></table></div></section>",
        .{ try esc(allocator, sm.nombre), weight_txt, sm.total, sm.calificadas, subtotal, rows.items },
    );
}

pub const plan_target: f64 = 8.0;

/// Plan "qué necesito para llegar a 8" con los pesos reales del parcial.
///
/// Dos vías honestas, calculadas con datos reales:
///  * solo el examen trimestral (25%, hoy en 0);
///  * examen a 10 + una media uniforme `p` en las actividades pendientes.
pub fn planHtml(allocator: std.mem.Allocator, d: SubjectDetail) ![]const u8 {
    if (!d.tiene_nota) {
        return "<section class=\"nx-subject-plan nova-subject-plan\"><div class=\"nx-plan-head\"><p class=\"nx-eyebrow\">Plan para llegar a 8</p><h2>Sin promedio todavía</h2><p>Cuando Mikareno publique una nota, aquí aparecerá exactamente cuánto necesitas y en qué.</p></div></section>";
    }
    const current = d.nota;
    var has_exam_sub = false;
    var unknown = false;
    var has_pending = false;
    var denom: f64 = 0;
    var sum_wnm: f64 = 0;
    var submenus_max: f64 = 0;
    var pending_total: usize = 0;

    for (d.submenus) |sm| {
        const w = submenuWeight(sm.nombre) orelse {
            if (sm.total > 0) unknown = true;
            continue;
        };
        if (w == weight_examen) has_exam_sub = true;
        const pending = if (sm.total > sm.calificadas) sm.total - sm.calificadas else 0;
        const c = @as(f64, @floatFromInt(sm.calificadas));
        const n = @as(f64, @floatFromInt(pending));
        const den = c + n;
        if (den > 0) submenus_max += w * ((sm.suma + 10.0 * n) / den);
        if (pending > 0 and den > 0) {
            has_pending = true;
            pending_total += pending;
            const m = if (sm.calificadas > 0) sm.suma / c else 0.0;
            denom += w * n / den;
            sum_wnm += w * n * m / den;
        }
    }
    const exam_w: f64 = if (has_exam_sub) 0.0 else weight_examen;
    if (exam_w > 0) submenus_max += exam_w * 10.0;

    const q_need: f64 = if (exam_w > 0) (plan_target - current) / exam_w else 99.0;
    var p_need: ?f64 = null;
    if (denom > 1e-9) {
        const room = plan_target - exam_w * 10.0 - current;
        const p = (room + sum_wnm) / denom;
        p_need = if (p < 0) 0 else p;
    }

    const gap = plan_target - current;
    const head = if (gap <= 0)
        try std.fmt.allocPrint(allocator, "<h2>Ya estás en {d:.2}, sobre la meta</h2><p>Mantén el nivel: con el examen en 0 ya superas 8. Cualquier nota en el examen solo suma.</p>", .{current})
    else
        try std.fmt.allocPrint(allocator, "<h2>Te faltan {d:.2} puntos para 8</h2><p>Promedio actual {d:.2} sobre {d:.0}. Las cuentas usan los pesos reales de tu colegio.</p>", .{ gap, current, d.maximo });

    const exam_lever = if (exam_w <= 0)
        "<div class=\"nx-plan-lever\"><span>Examen trimestral · 25%</span><strong>con nota</strong><small>Ya está dentro del promedio; usa las pendientes.</small></div>"
    else if (q_need <= 10)
        try std.fmt.allocPrint(allocator, "<div class=\"nx-plan-lever\"><span>Examen trimestral · 25%</span><strong>≥ {d:.2}</strong><small>Vía rápida, dejando las pendientes como están.</small></div>", .{q_need})
    else
        try std.fmt.allocPrint(allocator, "<div class=\"nx-plan-lever is-warn\"><span>Examen trimestral · 25%</span><strong>&gt; 10</strong><small>Solo el examen no alcanza: necesitarías {d:.2}.</small></div>", .{q_need});

    const pending_lever = if (!has_pending)
        "<div class=\"nx-plan-lever\"><span>Actividades pendientes</span><strong>sin pendientes</strong><small>No quedan entregas por calificar en este parcial.</small></div>"
    else if (p_need) |p| blk: {
        if (p <= 10) break :blk try std.fmt.allocPrint(allocator, "<div class=\"nx-plan-lever is-good\"><span>Actividades pendientes</span><strong>≥ {d:.2}</strong><small>Con 10 en el examen · {d} entregas por calificar.</small></div>", .{ p, pending_total });
        break :blk "<div class=\"nx-plan-lever is-bad\"><span>Actividades pendientes</span><strong>&gt; 10</strong><small>Ni con todo a 10 se llega a 8 este parcial.</small></div>";
    } else "<div class=\"nx-plan-lever\"><span>Actividades pendientes</span><strong>—</strong><small>Sin actividades computables.</small></div>";

    const max_lever = try std.fmt.allocPrint(allocator, "<div class=\"nx-plan-lever\"><span>Máximo alcanzable</span><strong>{d:.2}</strong><small>Si sacas 10 en todo lo que queda.</small></div>", .{submenus_max});

    var tasks = std.ArrayList(u8).init(allocator);
    if (has_pending and p_need != null and p_need.? <= 10) {
        try tasks.appendSlice("<div class=\"nx-plan-tasks\"><p class=\"nx-plan-tasks-title\">Entregas que faltan y cuánto valen</p><ul>");
        for (d.submenus) |sm| {
            if (submenuWeight(sm.nombre) == null) continue;
            const pct = activityPercent(sm);
            for (sm.actividades) |a| {
                if (a.calificada) continue;
                const pct_txt = if (pct) |pv| try std.fmt.allocPrint(allocator, "{d:.1}%", .{pv}) else try allocator.dupe(u8, "—");
                try tasks.appendSlice(try std.fmt.allocPrint(allocator, "<li><span class=\"nx-plan-task-name\">{s}</span><span class=\"nx-plan-task-meta\">{s} · {s} · meta ≥ {d:.2}</span></li>", .{ try esc(allocator, a.nombre), try esc(allocator, sm.nombre), pct_txt, p_need.? }));
            }
        }
        try tasks.appendSlice("</ul></div>");
    }

    const weights = "<div class=\"nx-plan-weights\"><span class=\"nx-weight-chip\">TAREAS 20%</span><span class=\"nx-weight-chip\">TRABAJOS 40%</span><span class=\"nx-weight-chip\">EVALUACIONES 15%</span><span class=\"nx-weight-chip\">EXAMEN TRIMESTRAL 25%</span></div>";
    const note = if (unknown)
        "<p class=\"nx-plan-note\">Hay un submenú con un esquema no reconocido; el plan podría variar. Los pesos mostrados se deducen de tus promedios reales y coinciden con las 14 materias calificadas.</p>"
    else
        "<p class=\"nx-plan-note\">Pesos deducidos de tus promedios reales (error ≤ 0.011 en las 14 materias). Dentro de cada submenú todas las actividades valen lo mismo; el examen trimestral vale 25% y hoy cuenta como 0.</p>";

    return std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-subject-plan nova-subject-plan\"><div class=\"nx-plan-head\"><p class=\"nx-eyebrow\">Plan para llegar a 8</p>{s}</div>" ++
            "<div class=\"nx-plan-levers\">{s}{s}{s}</div>{s}{s}{s}</section>",
        .{ head, exam_lever, pending_lever, max_lever, tasks.items, weights, note },
    );
}

pub fn subjectDetailHtml(allocator: std.mem.Allocator, d: SubjectDetail) ![]const u8 {
    const name = try esc(allocator, d.materia);
    const code = try esc(allocator, d.codigo);
    const trim_txt = if (d.parcial.len > 0) try partialLabel(allocator, d.parcial) else try allocator.dupe(u8, "Parcial actual");
    const tone = if (d.tiene_nota) toneFor(d.nota) else Tone{ .color = "#7f8ea3", .label = "Sin calificar", .pct = 0 };
    const nota_txt = if (d.tiene_nota) try std.fmt.allocPrint(allocator, "{d:.2}", .{d.nota}) else try allocator.dupe(u8, "—");
    const max_txt = try std.fmt.allocPrint(allocator, "{d:.0}", .{d.maximo});
    const bloqueada = if (d.bloqueada) "Sí" else "No";
    const tipo = if (d.cualitativa) "Cualitativa" else "Cuantitativa";
    const banner = try syncBanner(allocator, Datos{ .connected = d.connected, .timeout = false });
    const obs = if (d.observacion.len > 0)
        try std.fmt.allocPrint(allocator, "<aside class=\"nx-subject-observation nova-subject-observation\"><span class=\"material-symbols-outlined\">sticky_note_2</span><div><h2>Observación</h2><p>{s}</p></div></aside>", .{try esc(allocator, d.observacion)})
    else
        "";

    var acts = std.ArrayList(u8).init(allocator);
    if (d.submenus.len == 0) {
        try acts.appendSlice("<div class=\"nx-grade-empty\"><span class=\"material-symbols-outlined\">event_busy</span><p>No hay actividades registradas para esta asignatura.</p><a class=\"nx-button nx-button-secondary\" href=\"/dashboard/subjects\">Volver a notas</a></div>");
    } else {
        for (d.submenus) |sm| try acts.appendSlice(try submenuSection(allocator, sm));
    }
    const plan = try planHtml(allocator, d);

    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-page nx-subject-detail nova-page nova-subject-detail-page\">{s}" ++
            "<header class=\"nx-subject-hero nova-subject-detail-hero\"><a href=\"/dashboard/subjects\" class=\"nx-back-link\"><span class=\"material-symbols-outlined\">arrow_back</span>Todas las asignaturas</a>" ++
            "<div><p class=\"nx-eyebrow\">Detalle académico</p><h1>{s}</h1><p>Código {s} · {s}</p></div>" ++
            "<div class=\"nx-subject-note\"><span class=\"nx-grade-chip\" style=\"color:{s}\">{s}</span></div></header>" ++
            "<section class=\"nx-subject-summary nova-subject-summary\"><div class=\"nx-subject-summary-grade\"><span>Promedio actual</span><strong style=\"color:{s}\">{s}</strong><p>sobre {s}</p><div class=\"nx-subject-progress\"><div style=\"height:100%;width:{d}%;background:{s};border-radius:inherit\"></div></div></div>" ++
            "<div class=\"nx-subject-summary-facts\"><div><span>Calificación</span><strong>{s}</strong></div><div><span>Código</span><strong>{s}</strong></div><div><span>Máximo</span><strong>{s}</strong></div><div><span>Bloqueada</span><strong>{s}</strong></div><div><span>Tipo</span><strong>{s}</strong></div></div></section>" ++
            "{s}{s}{s}</div>",
        .{ banner, name, code, trim_txt, tone.color, tone.label, tone.color, nota_txt, max_txt, tone.pct, tone.color, tone.label, code, max_txt, bloqueada, tipo, plan, obs, acts.items },
    );
}

pub fn subjectMissingHtml(allocator: std.mem.Allocator, query: []const u8) ![]const u8 {
    const q = try esc(allocator, query);
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-page nx-subject-detail nova-page nova-subject-detail-page\">" ++
            "<header class=\"nx-subject-hero nova-subject-detail-hero\"><a href=\"/dashboard/subjects\" class=\"nx-back-link\"><span class=\"material-symbols-outlined\">arrow_back</span>Todas las asignaturas</a><div><p class=\"nx-eyebrow\">Detalle académico</p><h1>{s}</h1><p>No se encontró esta asignatura en el parcial actual.</p></div><div class=\"nx-subject-note\"></div></header>" ++
            "<div class=\"nx-grade-empty\"><span class=\"material-symbols-outlined\">search_off</span><p>Mikareno no devolvió datos para <strong>{s}</strong>.</p><p>Puede que el nombre haya cambiado o que el MCP no esté respondiendo ahora.</p><a class=\"nx-button nx-button-secondary\" href=\"/dashboard/subjects\"><span class=\"material-symbols-outlined\">arrow_back</span>Volver a notas</a></div></div>",
        .{ q, q },
    );
}

test "toneFor usa la escala acordada" {
    try std.testing.expectEqualStrings("Aprobada", toneFor(7.0).label);
    try std.testing.expectEqualStrings("En riesgo", toneFor(5.0).label);
    try std.testing.expectEqualStrings("Reprobada", toneFor(4.99).label);
    try std.testing.expectEqual(@as(u32, 50), toneFor(5.0).pct);
}

test "partialLabel traduce un partialId real" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    try std.testing.expectEqualStrings("2026 · Trimestre 3 · Parcial 1", try partialLabel(a, "2026:T3:1"));
}

test "los pesos reales de cada submenú" {
    try std.testing.expectApproxEqAbs(weight_tareas, submenuWeight("TAREAS").?, 1e-9);
    try std.testing.expectApproxEqAbs(weight_trabajos, submenuWeight("TRABAJOS").?, 1e-9);
    try std.testing.expectApproxEqAbs(weight_evaluaciones, submenuWeight("EVALUACIONES").?, 1e-9);
    try std.testing.expectApproxEqAbs(weight_examen, submenuWeight("EXAMEN TRIMESTRAL").?, 1e-9);
    try std.testing.expect(submenuWeight("LECCIONES") == null);
}

test "las fechas ISO se muestran en formato local" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    try std.testing.expectEqualStrings("30/09/2026", try formatDate(a, "2026-09-30"));
    try std.testing.expectEqualStrings("sin fecha", try formatDate(a, "sin fecha"));
}

test "el plan para 8 usa los pesos reales (caso Matemática)" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var acts = [_]DetailActivity{
        .{ .nombre = "Carátula", .descripcion = "", .fecha = "08/09/2026", .nota = 7, .calificada = true },
        .{ .nombre = "Trucos", .descripcion = "", .fecha = "09/09/2026", .nota = 9, .calificada = true },
        .{ .nombre = "Monomio", .descripcion = "", .fecha = "16/09/2026", .nota = 10, .calificada = true },
        .{ .nombre = "Polinomio", .descripcion = "", .fecha = "16/09/2026", .nota = 8, .calificada = true },
        .{ .nombre = "Pendiente", .descripcion = "", .fecha = "25/11/2026", .nota = null, .calificada = false },
    };
    var subs = [_]DetailSubmenu{
        .{ .nombre = "TAREAS", .total = 9, .calificadas = 4, .suma = 34, .subtotal = 8.5, .actividades = &acts },
        .{ .nombre = "TRABAJOS", .total = 3, .calificadas = 2, .suma = 16, .subtotal = 8.0, .actividades = acts[0..0] },
        .{ .nombre = "EVALUACIONES", .total = 2, .calificadas = 2, .suma = 16.5, .subtotal = 8.25, .actividades = acts[0..0] },
    };
    const d = SubjectDetail{ .tiene_nota = true, .nota = 6.13, .maximo = 10, .submenus = &subs };
    const html = try planHtml(a, d);
    try std.testing.expect(std.mem.indexOf(u8, html, "7.48") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "5.65") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "EXAMEN TRIMESTRAL 25%") != null);
}

test "la libreta oficial usa la URL configurada y se escapa" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const d = Datos{};
    const html = try gradesPageHtml(a, d, "https://colegio.test/app?x=1&y=2");
    try std.testing.expect(std.mem.indexOf(u8, html, "Libreta oficial") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "https://colegio.test/app?x=1&amp;y=2") != null);
    const sin = try gradesPageHtml(a, d, "");
    try std.testing.expect(std.mem.indexOf(u8, sin, "Libreta oficial") == null);
    const peligro = try gradesPageHtml(a, d, "javascript:alert(1)");
    try std.testing.expect(std.mem.indexOf(u8, peligro, "Libreta oficial") == null);
}
