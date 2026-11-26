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
const charts = @import("charts.zig");

pub const Evento = struct { fecha: []const u8, titulo: []const u8, tipo: []const u8 };
pub const Tarea = struct { titulo: []const u8, vencimiento: []const u8, materia: []const u8 = "" };
pub const Nota = struct { materia: []const u8, nota: f64, trimestre: u8 };

/// Una asignatura del parcial en curso con todo lo que la vista de notas
/// necesita: código real, máximo, si es cuantitativa, si está bloqueada y
/// si Mikareno ya publicó un promedio (`tiene_nota`).
/// Un submenú del parcial, ya normalizado para las cuentas.
pub const Axis = struct {
    nombre: []const u8,
    peso: f64,
    total: usize,
    calificadas: usize,
    suma: f64,
};

pub const SubjectGrade = struct {
    materia: []const u8,
    codigo: []const u8,
    parcial: []const u8,
    nota: f64,
    tiene_nota: bool,
    maximo: f64,
    cuantitativa: bool,
    bloqueada: bool,
    /// Desglose real por submenú: base para el mínimo por eje y para la
    /// vista "solo lo calificado".
    axes: []const Axis = &.{},
    /// Promedio renormalizado a lo ya calificado; null si no hay nada con nota.
    real: ?f64 = null,
};

pub const Clase = struct { dia: []const u8, etiqueta: []const u8 };

/// Todo lo que el inicio y el calendario necesitan, ya leído.
pub const Datos = struct {
    connected: bool = false,
    timeout: bool = false,
    /// La respuesta académica viene de la copia guardada, no de una llamada
    /// en vivo.
    from_cache: bool = false,
    /// La llamada en vivo falló: además de usar la copia, no hay conexión.
    offline: bool = false,
    cache_age_seconds: ?i64 = null,
    cache_fetched_at: ?i64 = null,
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
    /// Estado del servicio local de OpenCode (una sola instancia).
    opencode: OpenCode = .{},
};

/// Servicio de OpenCode que usa la terminal pequeña del panel.
pub const OpenCode = struct {
    running: bool = false,
    url: []const u8 = "",
    pid: u32 = 0,
    version: []const u8 = "",
    port: []const u8 = "",
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

/// Cada cuánto se vuelve a consultar Mikareno. Entre medias se sirve la
/// copia guardada, así que sin wifi las notas siguen apareciendo.
pub const CACHE_TTL_SECONDS: i64 = 5 * 60 * 60;

const CachedFull = struct { payload: []const u8, fetched_at: i64 };

/// Lee Mikareno (`get_student_full`) y los contadores locales.
///
/// * Si hay copia guardada reciente (< 5 h), se usa sin llamar a Mikareno.
/// * Si la llamada falla (sin wifi), se usa la última copia disponible y se
///   marca `from_cache`: el llamador avisa, pero las notas no desaparecen.
/// * Si Mikareno responde con datos, se guarda la copia.
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

    const now = std.time.timestamp();
    const cached = loadCache(allocator, db);

    if (cached) |c| {
        const age = now - c.fetched_at;
        if (age >= 0 and age < CACHE_TTL_SECONDS) {
            if (applyCached(allocator, &d, c, now)) return d;
        }
    }

    const full = mcp_client.callTool(
        allocator,
        mikareno_url,
        timeout_ms,
        "get_student_full",
        try json.object(allocator, &.{.{ .key = "bg", .value = .{ .bool = true } }}),
        true,
    ) catch |err| {
        if (cached) |c| {
            if (applyCached(allocator, &d, c, now)) {
                d.offline = true;
                d.timeout = err == error.RemoteRpcError or err == error.ToolFailed;
                return d;
            }
        }
        d.offline = true;
        d.timeout = err == error.RemoteRpcError or err == error.ToolFailed;
        return d;
    };

    applyFull(allocator, &d, full) catch {};
    const useful = d.grades.len > 0 or d.horario.len > 0 or d.eventos.len > 0 or d.tareas.len > 0;
    if (useful) {
        saveCache(allocator, db, full, now);
        return d;
    }

    // Mikareno respondió pero aún está refrescando (lista vacía): mejor la
    // última copia que una pantalla vacía.
    if (cached) |c| {
        if (applyCached(allocator, &d, c, now)) return d;
    }
    return d;
}

/// Convierte el JSON de `get_student_full` en `Datos` académicos.
fn applyFull(allocator: std.mem.Allocator, d: *Datos, full: json.Value) !void {
    d.connected = true;
    const tarea_stats = parseTareaStats(full);
    d.tasks_today = tarea_stats.hoy;
    d.tasks_week = tarea_stats.semana;
    d.notas = parseNotas(allocator, full);
    d.grades = parseGrades(allocator, full);
    const subject_axes = parseSubjectAxes(allocator, full);
    for (d.grades) |*g| {
        for (subject_axes) |sa| {
            if (!gradeCodeMatches(sa.codigo, g.codigo)) continue;
            if (g.parcial.len > 0 and sa.parcial.len > 0 and !std.mem.eql(u8, sa.parcial, g.parcial)) continue;
            g.axes = sa.axes;
            g.real = realGradeAxes(sa.axes);
            break;
        }
    }
    for (d.grades) |g| {
        if (g.parcial.len > 0) {
            d.partial_label = partialLabel(allocator, g.parcial) catch "";
            break;
        }
    }
    d.eventos = parseEventos(allocator, full);
    d.tareas = parseTareas(allocator, full);
    d.horario = parseHorario(allocator, full);
}

fn loadCache(allocator: std.mem.Allocator, db: anytype) ?CachedFull {
    var stmt = db.prepare("SELECT payload,fetched_at FROM mikareno_cache WHERE id=1") catch return null;
    defer stmt.deinit();
    if (!(stmt.next() catch false)) return null;
    const payload = allocator.dupe(u8, stmt.text(0)) catch return null;
    return .{ .payload = payload, .fetched_at = stmt.int64(1) };
}

fn saveCache(allocator: std.mem.Allocator, db: anytype, full: json.Value, now: i64) void {
    const payload = json.stringify(allocator, full) catch return;
    if (payload.len == 0) return;
    var stmt = db.prepare(
        "INSERT INTO mikareno_cache(id,payload,fetched_at) VALUES(1,?1,?2) " ++
            "ON CONFLICT(id) DO UPDATE SET payload=excluded.payload,fetched_at=excluded.fetched_at",
    ) catch return;
    defer stmt.deinit();
    stmt.bindText(1, payload) catch return;
    stmt.bindInt(2, now) catch return;
    _ = stmt.next() catch return;
}

fn applyCached(allocator: std.mem.Allocator, d: *Datos, c: CachedFull, now: i64) bool {
    const full = json.parse(allocator, c.payload) catch return false;
    applyFull(allocator, d, full) catch return false;
    d.from_cache = true;
    d.cache_age_seconds = @max(0, now - c.fetched_at);
    d.cache_fetched_at = c.fetched_at;
    return true;
}

/// "hace 2 h", "hace 15 min" o "hace un momento".
pub fn cacheAgeText(allocator: std.mem.Allocator, seconds: i64) ![]const u8 {
    if (seconds < 60) return allocator.dupe(u8, "hace un momento");
    if (seconds < 3600) return std.fmt.allocPrint(allocator, "hace {d} min", .{@divFloor(seconds, 60)});
    if (seconds < 86400) return std.fmt.allocPrint(allocator, "hace {d} h", .{@divFloor(seconds, 3600)});
    return std.fmt.allocPrint(allocator, "hace {d} días", .{@divFloor(seconds, 86400)});
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
        const materia = json.getString(item, "materia") orelse json.getString(item, "courseName") orelse "";
        out.append(.{ .titulo = titulo, .vencimiento = vencimiento, .materia = materia }) catch {};
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
fn diaKey(allocator: std.mem.Allocator, raw: []const u8) ?[]const u8 {
    var lower = allocator.alloc(u8, raw.len) catch return null;
    for (raw, 0..) |ch, i| lower[i] = std.ascii.toLower(ch);
    const pares = [_]struct { en: []const u8, es: []const u8 }{
        .{ .en = "monday", .es = "lunes" },
        .{ .en = "tuesday", .es = "martes" },
        .{ .en = "wednesday", .es = "miercoles" },
        .{ .en = "thursday", .es = "jueves" },
        .{ .en = "friday", .es = "viernes" },
        .{ .en = "saturday", .es = "sabado" },
        .{ .en = "sunday", .es = "domingo" },
    };
    for (pares) |p| {
        if (std.mem.eql(u8, lower, p.en)) return p.es;
    }
    return lower;
}

fn parseHorarioArray(allocator: std.mem.Allocator, horario: json.Value) []Clase {
    if (horario != .array) return &.{};
    var out = std.ArrayList(Clase).init(allocator);
    for (horario.array.items) |slot| {
        const clase = json.getString(slot, "clase") orelse continue;
        if (clase.len == 0) continue;
        const hora = json.getString(slot, "hora") orelse "";
        const codigo = json.getString(slot, "codigo") orelse "";
        const dia_raw = json.getString(slot, "dia") orelse continue;
        const dia = diaKey(allocator, dia_raw) orelse continue;
        const sufijo = if (codigo.len > 0)
            std.fmt.allocPrint(allocator, " [{s}]", .{codigo}) catch ""
        else
            "";
        // Sin hora se conserva la clase (se lista aparte); no se inventa una.
        const etiqueta = if (hora.len > 0)
            std.fmt.allocPrint(allocator, "{s} — {s}{s}", .{ hora, clase, sufijo }) catch continue
        else
            std.fmt.allocPrint(allocator, "{s}{s}", .{ clase, sufijo }) catch continue;
        out.append(.{ .dia = dia, .etiqueta = etiqueta }) catch {};
    }
    return out.items;
}

fn parseHorario(allocator: std.mem.Allocator, full: json.Value) []Clase {
    const horario = json.get(full, "horario") orelse return &.{};
    // Formato real de Mikareno: { periodMap: { MONDAY: [...] } }.
    // Formato legado/tests: [{ dia, hora, clase, codigo }].
    if (json.get(horario, "periodMap") == null) return parseHorarioArray(allocator, horario);
    const map = json.get(horario, "periodMap") orelse return &.{};
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

/// Aviso único de conexión: solo aparece si Mikareno no responde o si se
/// están mostrando datos guardados.
pub fn syncBanner(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.offline) {
        const age = try cacheAgeText(allocator, d.cache_age_seconds orelse 0);
        return std.fmt.allocPrint(
            allocator,
            "<div class=\"nx-sync-banner is-cached\" role=\"status\"><span class=\"material-symbols-outlined\">cloud_off</span><p>Sin conexión con Mikareno. Mostrando los últimos datos guardados {s}.</p><a href=\"/dashboard/mcp\">Revisar conexión<span class=\"material-symbols-outlined\">arrow_forward</span></a></div>",
            .{age},
        );
    }
    if (d.from_cache) {
        const age = try cacheAgeText(allocator, d.cache_age_seconds orelse 0);
        return std.fmt.allocPrint(
            allocator,
            "<div class=\"nx-sync-banner is-cached\" role=\"status\"><span class=\"material-symbols-outlined\">cloud_download</span><p>Datos guardados {s} (caché).</p><a href=\"/dashboard/subjects?refresh=1\">Actualizar datos<span class=\"material-symbols-outlined\">arrow_forward</span></a></div>",
            .{age},
        );
    }
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
    if (d.from_cache) {
        const age = try cacheAgeText(allocator, d.cache_age_seconds orelse 0);
        return std.fmt.allocPrint(allocator, "Mikareno: datos guardados {s}", .{age});
    }
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

/// Medidor circular con la media general del trimestre.
pub fn chartAvg(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.notas.len == 0) return chartEmpty(
        allocator,
        "Sin promedio todavía",
        "Cuando Mikareno publique notas, aquí verás tu media de un vistazo.",
        "/dashboard/subjects",
        "Ver asignaturas",
    );
    var total: f64 = 0;
    var best = d.notas[0].nota;
    var worst = d.notas[0].nota;
    var passing: usize = 0;
    for (d.notas) |n| {
        total += n.nota;
        best = @max(best, n.nota);
        worst = @min(worst, n.nota);
        if (n.nota >= 7.0) passing += 1;
    }
    const count = d.notas.len;
    const avg = total / @as(f64, @floatFromInt(count));
    const frac = @max(0.0, @min(1.0, avg / 10.0));
    const radius: f64 = 54.0;
    const circ = 2.0 * std.math.pi * radius;
    const dash = circ * frac;
    const color = if (avg < 4.0) "var(--nx-danger)" else if (avg < 7.0) "var(--nx-orange)" else "url(#nx-avg-grad)";
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-gauge\"><svg class=\"nx-chart nx-chart-gauge\" viewBox=\"0 0 140 140\" role=\"img\" aria-label=\"Media general {d:.2} sobre 10\">" ++
            "<defs><linearGradient id=\"nx-avg-grad\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" stop-color=\"#a78bfa\"/><stop offset=\"1\" stop-color=\"#67e8f9\"/></linearGradient></defs>" ++
            "<circle class=\"nx-chart-donut-track\" cx=\"70\" cy=\"70\" r=\"54\"/>" ++
            "<circle cx=\"70\" cy=\"70\" r=\"54\" fill=\"none\" stroke=\"{s}\" stroke-width=\"11\" stroke-linecap=\"round\" stroke-dasharray=\"{d:.1} {d:.1}\" transform=\"rotate(-90 70 70)\"/>" ++
            "<text class=\"nx-gauge-value\" x=\"70\" y=\"69\" text-anchor=\"middle\">{d:.2}</text>" ++
            "<text class=\"nx-gauge-label\" x=\"70\" y=\"86\" text-anchor=\"middle\">MEDIA / 10</text></svg>" ++
            "<div class=\"nx-gauge-facts\"><span><b>{d:.1}</b>mejor</span><span><b>{d:.1}</b>menor</span><span><b>{d}</b>sobre 7</span></div></div>",
        .{ avg, color, dash, circ - dash, avg, best, worst, passing },
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
        const fill = if (n.nota < 4.0) "url(#nx-g-bad)" else if (n.nota < 7.0) "url(#nx-g-mid)" else "url(#nx-g-good)";
        const name = try esc(allocator, n.materia);
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<g><title>{s}: {d:.2}</title><text class=\"nx-chart-t\" x=\"0\" y=\"{d}\" dominant-baseline=\"middle\">{s}</text><rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"8\" rx=\"4\" fill=\"var(--nx-surface-3)\"/><rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"8\" rx=\"4\" fill=\"{s}\"/><text class=\"nx-chart-v\" x=\"{d}\" y=\"{d}\" dominant-baseline=\"middle\">{d:.2}</text></g>",
            .{ name, n.nota, y + row / 2, name, bar_x, y + (row - 8) / 2, bar_w, bar_x, y + (row - 8) / 2, bw, fill, bar_x + bar_w + 8, y + row / 2, n.nota },
        )) catch {};
    }
    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 {d} {d}\" role=\"img\" aria-label=\"Media por asignatura\"><defs><linearGradient id=\"nx-g-good\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"0\"><stop offset=\"0\" stop-color=\"#8b5cf6\"/><stop offset=\"1\" stop-color=\"#67e8f9\"/></linearGradient><linearGradient id=\"nx-g-mid\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"0\"><stop offset=\"0\" stop-color=\"#f59e0b\"/><stop offset=\"1\" stop-color=\"#ffbd7a\"/></linearGradient><linearGradient id=\"nx-g-bad\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"0\"><stop offset=\"0\" stop-color=\"#dc2626\"/><stop offset=\"1\" stop-color=\"#ffb4ab\"/></linearGradient></defs>{s}</svg>",
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

    const width: i32 = 360;
    const height: i32 = 170;
    const left: i32 = 30;
    const bottom: i32 = 26;
    const top: i32 = 18;
    const plot_h = height - top - bottom;
    const plot_w = width - left - 10;
    const slot = @divTrunc(plot_w, 7);
    const bar_w: i32 = 10;
    const gap: i32 = 4;
    const group_w = bar_w * 2 + gap;
    const hoy = todayWeekday();
    const fmax = @as(f64, @floatFromInt(max));

    var out = std.ArrayList(u8).init(allocator);
    out.appendSlice(
        "<defs><linearGradient id=\"nx-g-class\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"#d8ccfe\"/><stop offset=\"1\" stop-color=\"#7c3aed\"/></linearGradient><linearGradient id=\"nx-g-task\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"#ffe0b8\"/><stop offset=\"1\" stop-color=\"#f59e0b\"/></linearGradient></defs>",
    ) catch {};
    for ([_]f64{ 0.0, 0.5, 1.0 }) |frac| {
        if (frac == 0.5 and max < 2) continue;
        const y = top + @as(i32, @intFromFloat(@floor(@as(f64, @floatFromInt(plot_h)) * frac)));
        const val: i32 = @intFromFloat(@round(fmax * (1.0 - frac)));
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<line x1=\"{d}\" y1=\"{d}\" x2=\"{d}\" y2=\"{d}\" stroke=\"var(--nx-line)\" stroke-width=\"1\" stroke-dasharray=\"2 4\"/><text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\" text-anchor=\"end\">{d}</text>",
            .{ left, y, width - 10, y, left - 7, y + 3, val },
        )) catch {};
    }
    for (dias, 0..) |dia, i| {
        const cx = left + slot * @as(i32, @intCast(i)) + @divTrunc(slot, 2);
        const gx = cx - @divTrunc(group_w, 2);
        if (std.mem.eql(u8, dia.es, hoy)) {
            out.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"{d}\" rx=\"5\" fill=\"var(--nx-accent-soft)\"/>",
                .{ cx - @divTrunc(slot, 2) + 2, top - 6, slot - 4, plot_h + 22 },
            )) catch {};
        }
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\" text-anchor=\"middle\">{s}</text>",
            .{ cx, top + plot_h + 15, dia.label },
        )) catch {};
        const values = [_]struct { v: i32, fill: []const u8 }{
            .{ .v = clases[i], .fill = "url(#nx-g-class)" },
            .{ .v = tareas[i], .fill = "url(#nx-g-task)" },
        };
        for (values, 0..) |bar, j| {
            if (bar.v <= 0) continue;
            const h: i32 = @max(3, @as(i32, @intFromFloat(@round(@as(f64, @floatFromInt(plot_h)) * (@as(f64, @floatFromInt(bar.v)) / fmax)))));
            const x = gx + @as(i32, @intCast(j)) * (bar_w + gap);
            const by = top + plot_h - h;
            out.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"{d}\" rx=\"3\" fill=\"{s}\"><title>{s}: {d}</title></rect><text class=\"nx-chart-v\" x=\"{d}\" y=\"{d}\" text-anchor=\"middle\">{d}</text>",
                .{ x, by, bar_w, h, bar.fill, if (j == 0) "Clases" else "Entregas", bar.v, x + @divTrunc(bar_w, 2), by - 4, bar.v },
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

    const width: i32 = 360;
    const height: i32 = 170;
    const left: i32 = 18;
    const bottom: i32 = 26;
    const top: i32 = 18;
    const plot_h = height - top - bottom;
    const plot_w = width - left - 16;
    const slot = @divTrunc(plot_w, 7);
    const base = top + plot_h;
    const fany = @as(f64, @floatFromInt(any));

    var xs: [8]i32 = undefined;
    var ys: [8]i32 = undefined;
    var pts = std.ArrayList(u8).init(allocator);
    var area = std.ArrayList(u8).init(allocator);
    area.appendSlice(try std.fmt.allocPrint(allocator, "M {d} {d}", .{ left, base })) catch {};
    for (d.notes_activity, 0..) |value, i| {
        const cx = left + slot * @as(i32, @intCast(i));
        const h: i32 = @intFromFloat(@round(@as(f64, @floatFromInt(plot_h)) * (@as(f64, @floatFromInt(value)) / fany)));
        const y = base - h;
        xs[i] = cx;
        ys[i] = y;
        if (i > 0) pts.appendSlice(" ") catch {};
        pts.appendSlice(try std.fmt.allocPrint(allocator, "{d},{d}", .{ cx, y })) catch {};
        area.appendSlice(try std.fmt.allocPrint(allocator, " L {d} {d}", .{ cx, y })) catch {};
    }
    area.appendSlice(try std.fmt.allocPrint(allocator, " L {d} {d} Z", .{ xs[7], base })) catch {};

    var dots = std.ArrayList(u8).init(allocator);
    for (d.notes_activity, 0..) |value, i| {
        const last = i == 7;
        const r: i32 = if (last) 4 else 3;
        dots.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<circle cx=\"{d}\" cy=\"{d}\" r=\"{d}\" fill=\"var(--nx-bg)\" stroke=\"var(--nx-blue)\" stroke-width=\"2\"><title>Semana {d}: {d} apuntes</title></circle>",
            .{ xs[i], ys[i], r, i + 1, value },
        )) catch {};
    }

    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 {d} {d}\" role=\"img\" aria-label=\"Apuntes por semana\"><defs><linearGradient id=\"nx-g-notes\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"#67e8f9\" stop-opacity=\".42\"/><stop offset=\"1\" stop-color=\"#67e8f9\" stop-opacity=\"0\"/></linearGradient></defs><line x1=\"{d}\" y1=\"{d}\" x2=\"{d}\" y2=\"{d}\" stroke=\"var(--nx-line)\" stroke-width=\"1\" stroke-dasharray=\"2 4\"/><path d=\"{s}\" fill=\"url(#nx-g-notes)\"/><polyline points=\"{s}\" fill=\"none\" stroke=\"var(--nx-blue)\" stroke-width=\"2.4\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>{s}<text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\">hace 8 sem</text><text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\" text-anchor=\"end\">hoy</text></svg>",
        .{ width, height, left, top, width - 16, top, area.items, pts.items, dots.items, left, base + 16, xs[7], base + 16 },
    );
}

/// Fecha ISO (`YYYY-MM-DD` o con hora) a dias desde el epoch.
fn isoDayNumber(fecha: []const u8) ?i64 {
    if (fecha.len < 10) return null;
    const year = std.fmt.parseInt(i32, fecha[0..4], 10) catch return null;
    const month = std.fmt.parseInt(u32, fecha[5..7], 10) catch return null;
    const day = std.fmt.parseInt(u32, fecha[8..10], 10) catch return null;
    if (month < 1 or month > 12 or day < 1 or day > 31) return null;
    return daysFromCivil(year, month, day);
}

/// Histograma de notas por rango: cuantas materias caen en cada tramo.
pub fn chartGradeDist(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.notas.len == 0) return chartEmpty(
        allocator,
        "Sin notas que repartir",
        "Cuando lleguen calificaciones, aqui veras donde se concentran.",
        "/dashboard/subjects",
        "Ver asignaturas",
    );
    var counts = [_]u32{0} ** 5;
    for (d.notas) |n| {
        const slot: usize = if (n.nota < 4.0) 0 else if (n.nota < 6.0) 1 else if (n.nota < 7.0) 2 else if (n.nota < 9.0) 3 else 4;
        counts[slot] += 1;
    }
    var max: u32 = 0;
    for (counts) |v| max = @max(max, v);
    if (max == 0) return chartEmpty(allocator, "Sin notas que repartir", "Aun no hay datos para este grafico.", "/dashboard/subjects", "Ver asignaturas");

    const width: i32 = 360;
    const height: i32 = 170;
    const left: i32 = 28;
    const top: i32 = 18;
    const bottom: i32 = 26;
    const plot_h = height - top - bottom;
    const plot_w = width - left - 8;
    const slot = @divTrunc(plot_w, 5);
    const bar_w: i32 = 28;
    const labels = [_][]const u8{ "<4", "4-6", "6-7", "7-9", "9-10" };
    const fills = [_][]const u8{ "url(#nx-gd-bad)", "url(#nx-gd-mid)", "url(#nx-gd-mid)", "url(#nx-gd-ok)", "url(#nx-gd-good)" };
    const fmax = @as(f64, @floatFromInt(max));

    var out = std.ArrayList(u8).init(allocator);
    out.appendSlice("<defs><linearGradient id=\"nx-gd-good\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"#d8ccfe\"/><stop offset=\"1\" stop-color=\"#8b5cf6\"/></linearGradient><linearGradient id=\"nx-gd-ok\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"#a5f3fc\"/><stop offset=\"1\" stop-color=\"#0891b2\"/></linearGradient><linearGradient id=\"nx-gd-mid\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"#ffe0b8\"/><stop offset=\"1\" stop-color=\"#f59e0b\"/></linearGradient><linearGradient id=\"nx-gd-bad\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"#ffb4ab\"/><stop offset=\"1\" stop-color=\"#dc2626\"/></linearGradient></defs>") catch {};
    for ([_]f64{ 0.0, 0.5, 1.0 }) |frac| {
        if (frac == 0.5 and max < 2) continue;
        const y = top + @as(i32, @intFromFloat(@floor(@as(f64, @floatFromInt(plot_h)) * frac)));
        const val: i32 = @intFromFloat(@round(fmax * (1.0 - frac)));
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<line x1=\"{d}\" y1=\"{d}\" x2=\"{d}\" y2=\"{d}\" stroke=\"var(--nx-line)\" stroke-width=\"1\" stroke-dasharray=\"2 4\"/><text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\" text-anchor=\"end\">{d}</text>",
            .{ left, y, width - 8, y, left - 6, y + 3, val },
        )) catch {};
    }
    for (counts, 0..) |value, i| {
        const cx = left + slot * @as(i32, @intCast(i)) + @divTrunc(slot, 2);
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\" text-anchor=\"middle\">{s}</text>",
            .{ cx, top + plot_h + 15, labels[i] },
        )) catch {};
        if (value == 0) continue;
        const h: i32 = @max(3, @as(i32, @intFromFloat(@round(@as(f64, @floatFromInt(plot_h)) * (@as(f64, @floatFromInt(value)) / fmax)))));
        const x = cx - @divTrunc(bar_w, 2);
        const y = top + plot_h - h;
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"{d}\" rx=\"5\" fill=\"{s}\"><title>{s}: {d} materia(s)</title></rect><text class=\"nx-chart-v\" x=\"{d}\" y=\"{d}\" text-anchor=\"middle\">{d}</text>",
            .{ x, y, bar_w, h, fills[i], labels[i], value, cx, y - 4, value },
        )) catch {};
    }
    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 {d} {d}\" role=\"img\" aria-label=\"Distribucion de notas por rango\">{s}</svg>",
        .{ width, height, out.items },
    );
}

/// Donut de salud academica: aprobadas, en riesgo y reprobadas.
pub fn chartHealth(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.notas.len == 0) return chartEmpty(
        allocator,
        "Sin diagnostico todavia",
        "En cuanto haya notas veras cuantas materias van bien.",
        "/dashboard/subjects",
        "Ver asignaturas",
    );
    var high: u32 = 0;
    var mid: u32 = 0;
    var low: u32 = 0;
    for (d.notas) |n| {
        if (n.nota >= 7.0) {
            high += 1;
        } else if (n.nota >= 4.0) {
            mid += 1;
        } else {
            low += 1;
        }
    }
    const total = high + mid + low;
    if (total == 0) return chartEmpty(allocator, "Sin diagnostico todavia", "Aun no hay datos para este grafico.", "/dashboard/subjects", "Ver asignaturas");
    const pct = @as(f64, @floatFromInt(high)) / @as(f64, @floatFromInt(total)) * 100.0;
    const radius: f64 = 50.0;
    const circ = 2.0 * std.math.pi * radius;
    const items = [_]struct { count: u32, color: []const u8, label: []const u8 }{
        .{ .count = high, .color = "#a78bfa", .label = "Aprobadas" },
        .{ .count = mid, .color = "#f59e0b", .label = "En riesgo" },
        .{ .count = low, .color = "#f87171", .label = "Reprobadas" },
    };
    var out = std.ArrayList(u8).init(allocator);
    out.appendSlice("<circle cx=\"76\" cy=\"85\" r=\"50\" fill=\"none\" stroke=\"var(--nx-surface-3)\" stroke-width=\"18\"/>") catch {};
    var offset: f64 = 0;
    for (items) |item| {
        if (item.count == 0) continue;
        const seg = circ * (@as(f64, @floatFromInt(item.count)) / @as(f64, @floatFromInt(total)));
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<circle cx=\"76\" cy=\"85\" r=\"50\" fill=\"none\" stroke=\"{s}\" stroke-width=\"18\" stroke-dasharray=\"{d:.2} {d:.2}\" stroke-dashoffset=\"{d:.2}\" transform=\"rotate(-90 76 85)\"/>",
            .{ item.color, seg, circ - seg, -offset },
        )) catch {};
        offset += seg;
    }
    for (items, 0..) |item, i| {
        const y: i32 = 50 + @as(i32, @intCast(i)) * 34;
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<rect x=\"150\" y=\"{d}\" width=\"10\" height=\"10\" rx=\"3\" fill=\"{s}\"/><text class=\"nx-chart-t\" x=\"168\" y=\"{d}\" dominant-baseline=\"middle\">{s}</text><text class=\"nx-chart-v\" x=\"326\" y=\"{d}\" text-anchor=\"end\" dominant-baseline=\"middle\">{d}</text>",
            .{ y - 5, item.color, y, item.label, y, item.count },
        )) catch {};
    }
    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 340 170\" role=\"img\" aria-label=\"Salud academica: {d} aprobadas, {d} en riesgo, {d} reprobadas\"><text class=\"nx-chart-big\" x=\"76\" y=\"82\" text-anchor=\"middle\">{d:.0}%</text><text class=\"nx-chart-axis\" x=\"76\" y=\"99\" text-anchor=\"middle\">APROBADAS</text>{s}</svg>",
        .{ high, mid, low, pct, out.items },
    );
}

/// Entregas y eventos en los proximos 14 dias, dia a dia.
pub fn chartLoad14(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    var counts = [_]u32{0} ** 14;
    const today = @divFloor(std.time.timestamp(), 86400);
    for (d.tareas) |t| {
        const day = isoDayNumber(t.vencimiento) orelse continue;
        const delta = day - today;
        if (delta < 0 or delta >= 14) continue;
        counts[@intCast(delta)] += 1;
    }
    for (d.eventos) |e| {
        const day = isoDayNumber(e.fecha) orelse continue;
        const delta = day - today;
        if (delta < 0 or delta >= 14) continue;
        counts[@intCast(delta)] += 1;
    }
    var total: u32 = 0;
    var max: u32 = 0;
    for (counts) |v| {
        total += v;
        max = @max(max, v);
    }
    if (total == 0) return chartEmpty(
        allocator,
        "Quince dias despejados",
        "No hay entregas ni eventos proximos; cuando aparezcan, se veran aqui.",
        "/dashboard/planner",
        "Abrir planificador",
    );

    const width: i32 = 360;
    const height: i32 = 170;
    const left: i32 = 26;
    const top: i32 = 18;
    const bottom: i32 = 26;
    const plot_h = height - top - bottom;
    const plot_w = width - left - 10;
    const slot = @divTrunc(plot_w, 14);
    const bar_w: i32 = 12;
    const fmax = @as(f64, @floatFromInt(max));

    var out = std.ArrayList(u8).init(allocator);
    out.appendSlice("<defs><linearGradient id=\"nx-g-load\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"#a5f3fc\"/><stop offset=\"1\" stop-color=\"#7c3aed\"/></linearGradient></defs>") catch {};
    for ([_]f64{ 0.0, 0.5, 1.0 }) |frac| {
        if (frac == 0.5 and max < 2) continue;
        const y = top + @as(i32, @intFromFloat(@floor(@as(f64, @floatFromInt(plot_h)) * frac)));
        const val: i32 = @intFromFloat(@round(fmax * (1.0 - frac)));
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<line x1=\"{d}\" y1=\"{d}\" x2=\"{d}\" y2=\"{d}\" stroke=\"var(--nx-line)\" stroke-width=\"1\" stroke-dasharray=\"2 4\"/><text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\" text-anchor=\"end\">{d}</text>",
            .{ left, y, width - 10, y, left - 6, y + 3, val },
        )) catch {};
    }
    for (counts, 0..) |value, i| {
        const cx = left + slot * @as(i32, @intCast(i)) + @divTrunc(slot, 2);
        if (i % 2 == 0) {
            const date = civilFromDays(today + @as(i64, @intCast(i)));
            out.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\" text-anchor=\"middle\">{d}</text>",
                .{ cx, top + plot_h + 15, date.day },
            )) catch {};
        }
        if (value == 0) continue;
        const h: i32 = @max(4, @as(i32, @intFromFloat(@round(@as(f64, @floatFromInt(plot_h)) * (@as(f64, @floatFromInt(value)) / fmax)))));
        const x = cx - @divTrunc(bar_w, 2);
        const y = top + plot_h - h;
        const rx: i32 = if (i == 0) 100 else 4;
        const fill = if (i == 0) "var(--nx-accent)" else "url(#nx-g-load)";
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"{d}\" rx=\"{d}\" fill=\"{s}\"><title>Dia {d}: {d} entrega(s) y evento(s)</title></rect><text class=\"nx-chart-v\" x=\"{d}\" y=\"{d}\" text-anchor=\"middle\">{d}</text>",
            .{ x, y, bar_w, h, rx, fill, i + 1, value, cx, y - 4, value },
        )) catch {};
    }
    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 {d} {d}\" role=\"img\" aria-label=\"Entregas y eventos en los proximos 14 dias\">{s}<text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\">hoy</text></svg>",
        .{ width, height, out.items, left, top + plot_h + 24 },
    );
}

/// Pesos reales del parcial, siempre disponibles aunque no haya notas.
pub fn chartWeights(allocator: std.mem.Allocator) ![]const u8 {
    const items = [_]struct { label: []const u8, value: f64, color: []const u8 }{
        .{ .label = "Trabajos", .value = weight_trabajos * 100.0, .color = "#8b5cf6" },
        .{ .label = "Examen", .value = weight_examen * 100.0, .color = "#f472b6" },
        .{ .label = "Tareas", .value = weight_tareas * 100.0, .color = "#f59e0b" },
        .{ .label = "Evaluaciones", .value = weight_evaluaciones * 100.0, .color = "#67e8f9" },
    };
    const radius: f64 = 50.0;
    const circ = 2.0 * std.math.pi * radius;
    var out = std.ArrayList(u8).init(allocator);
    out.appendSlice("<circle cx=\"76\" cy=\"85\" r=\"50\" fill=\"none\" stroke=\"var(--nx-surface-3)\" stroke-width=\"18\"/>") catch {};
    var offset: f64 = 0;
    for (items) |item| {
        const seg = circ * (item.value / 100.0);
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<circle cx=\"76\" cy=\"85\" r=\"50\" fill=\"none\" stroke=\"{s}\" stroke-width=\"18\" stroke-dasharray=\"{d:.2} {d:.2}\" stroke-dashoffset=\"{d:.2}\" transform=\"rotate(-90 76 85)\"/>",
            .{ item.color, seg, circ - seg, -offset },
        )) catch {};
        offset += seg;
    }
    for (items, 0..) |item, i| {
        const y: i32 = 46 + @as(i32, @intCast(i)) * 31;
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<rect x=\"150\" y=\"{d}\" width=\"10\" height=\"10\" rx=\"3\" fill=\"{s}\"/><text class=\"nx-chart-t\" x=\"168\" y=\"{d}\" dominant-baseline=\"middle\">{s}</text><text class=\"nx-chart-v\" x=\"326\" y=\"{d}\" text-anchor=\"end\" dominant-baseline=\"middle\">{d:.0}%</text>",
            .{ y - 5, item.color, y, item.label, y, item.value },
        )) catch {};
    }
    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 340 170\" role=\"img\" aria-label=\"Pesos de evaluacion del trimestre\"><text class=\"nx-chart-big\" x=\"76\" y=\"82\" text-anchor=\"middle\">100%</text><text class=\"nx-chart-axis\" x=\"76\" y=\"99\" text-anchor=\"middle\">DE LA NOTA</text>{s}</svg>",
        .{out.items},
    );
}

/// Categorías canónicas de evaluación. El orden manda en los gráficos y en
/// la leyenda; los colores coinciden con `chartWeights`.
const axis_cats = [_]struct { key: []const u8, label: []const u8, color: []const u8 }{
    .{ .key = "TAREA", .label = "Tareas", .color = "#f59e0b" },
    .{ .key = "TRABAJO", .label = "Trabajos", .color = "#8b5cf6" },
    .{ .key = "EVALUAC", .label = "Evaluaciones", .color = "#67e8f9" },
    .{ .key = "EXAMEN", .label = "Examen", .color = "#f472b6" },
};

fn axisCat(name: []const u8) ?usize {
    for (axis_cats, 0..) |cat, i| {
        if (containsIgnoreCase(name, cat.key)) return i;
    }
    return null;
}

/// Recorta un nombre por puntos de código (no por bytes) para que las
/// etiquetas largas no rompan el SVG ni corten un acento por la mitad.
fn clipUtf8(allocator: std.mem.Allocator, s: []const u8, max_chars: usize) ![]const u8 {
    var count: usize = 0;
    var i: usize = 0;
    while (i < s.len) {
        const n: usize = @intCast(std.unicode.utf8ByteSequenceLength(s[i]) catch 1);
        i = @min(s.len, i + n);
        count += 1;
        if (count == max_chars and i < s.len) {
            return std.fmt.allocPrint(allocator, "{s}…", .{s[0..i]});
        }
    }
    return s;
}

/// Radar del rendimiento por tipo de evaluación: promedio de lo ya
/// calificado en cada eje (0–10), agregado sobre todas las materias.
pub fn chartAxesRadar(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    var suma = [_]f64{0} ** 4;
    var cal = [_]usize{0} ** 4;
    for (d.grades) |g| {
        for (g.axes) |a| {
            const ci = axisCat(a.nombre) orelse continue;
            suma[ci] += a.suma;
            cal[ci] += a.calificadas;
        }
    }
    var present: [4]usize = undefined;
    var n: usize = 0;
    for (0..4) |i| {
        if (cal[i] > 0) {
            present[n] = i;
            n += 1;
        }
    }
    if (n < 3) return chartEmpty(
        allocator,
        "Perfil aún incompleto",
        "Hacen falta al menos tres tipos de evaluación con notas para dibujar el radar.",
        "/dashboard/subjects",
        "Ver asignaturas",
    );

    const cx: f64 = 170.0;
    const cy: f64 = 92.0;
    const r: f64 = 56.0;
    const nf = @as(f64, @floatFromInt(n));
    var out = std.ArrayList(u8).init(allocator);

    // Anillos concéntricos y radios.
    for ([_]f64{ 2.5, 5.0, 7.5, 10.0 }) |level| {
        var pts = std.ArrayList(u8).init(allocator);
        for (0..n) |k| {
            const ang = -std.math.pi / 2.0 + 2.0 * std.math.pi * @as(f64, @floatFromInt(k)) / nf;
            const x = cx + r * (level / 10.0) * @cos(ang);
            const y = cy + r * (level / 10.0) * @sin(ang);
            try pts.writer().print("{s}{d:.1},{d:.1}", .{ if (k == 0) "" else " ", x, y });
        }
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<polygon points=\"{s}\" fill=\"none\" stroke=\"var(--nx-line)\" stroke-width=\"1\"/>",
            .{pts.items},
        )) catch {};
    }
    for (0..n) |k| {
        const ang = -std.math.pi / 2.0 + 2.0 * std.math.pi * @as(f64, @floatFromInt(k)) / nf;
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<line x1=\"{d:.1}\" y1=\"{d:.1}\" x2=\"{d:.1}\" y2=\"{d:.1}\" stroke=\"var(--nx-line)\" stroke-width=\"1\"/>",
            .{ cx, cy, cx + r * @cos(ang), cy + r * @sin(ang) },
        )) catch {};
    }

    // Polígono de datos.
    var poly = std.ArrayList(u8).init(allocator);
    for (0..n) |k| {
        const ci = present[k];
        const avg = suma[ci] / @as(f64, @floatFromInt(cal[ci]));
        const v = @max(0.0, @min(10.0, avg));
        const ang = -std.math.pi / 2.0 + 2.0 * std.math.pi * @as(f64, @floatFromInt(k)) / nf;
        const x = cx + r * (v / 10.0) * @cos(ang);
        const y = cy + r * (v / 10.0) * @sin(ang);
        try poly.writer().print("{s}{d:.1},{d:.1}", .{ if (k == 0) "" else " ", x, y });
    }
    out.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<polygon points=\"{s}\" fill=\"url(#nx-radar-fill)\" stroke=\"#a78bfa\" stroke-width=\"1.6\"/>",
        .{poly.items},
    )) catch {};

    // Vértices y etiquetas.
    for (0..n) |k| {
        const ci = present[k];
        const avg = suma[ci] / @as(f64, @floatFromInt(cal[ci]));
        const v = @max(0.0, @min(10.0, avg));
        const ang = -std.math.pi / 2.0 + 2.0 * std.math.pi * @as(f64, @floatFromInt(k)) / nf;
        const co = @cos(ang);
        const si = @sin(ang);
        const x = cx + r * (v / 10.0) * co;
        const y = cy + r * (v / 10.0) * si;
        const lx = cx + (r + 16.0) * co;
        const ly = cy + (r + 16.0) * si;
        const anchor = if (co > 0.35) "start" else if (co < -0.35) "end" else "middle";
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<circle cx=\"{d:.1}\" cy=\"{d:.1}\" r=\"3.2\" fill=\"{s}\"><title>{s}: {d:.2} / 10 ({d} calificadas)</title></circle>" ++
                "<text class=\"nx-chart-t\" x=\"{d:.1}\" y=\"{d:.1}\" text-anchor=\"{s}\">{s}</text>" ++
                "<text class=\"nx-chart-v\" x=\"{d:.1}\" y=\"{d:.1}\" text-anchor=\"{s}\">{d:.1}</text>",
            .{ x, y, axis_cats[ci].color, axis_cats[ci].label, avg, cal[ci], lx, ly, anchor, axis_cats[ci].label, lx, ly + 10.0, anchor, avg },
        )) catch {};
    }
    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 340 190\" role=\"img\" aria-label=\"Perfil por tipo de evaluacion\"><defs><linearGradient id=\"nx-radar-fill\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" stop-color=\"#8b5cf6\" stop-opacity=\".55\"/><stop offset=\"1\" stop-color=\"#67e8f9\" stop-opacity=\".26\"/></linearGradient></defs>{s}</svg>",
        .{out.items},
    );
}

/// Barras apiladas por materia: cuánto aporta cada tipo de evaluación al
/// promedio oficial (0–10). La suma de segmentos es la nota del parcial.
pub fn chartStackedSubjects(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    const Bar = struct { name: []const u8, seg: [4]f64, total: f64 };
    var bars = std.ArrayList(Bar).init(allocator);
    for (d.grades) |g| {
        if (g.axes.len == 0) continue;
        var seg = [_]f64{0} ** 4;
        var total: f64 = 0;
        for (0..4) |ci| {
            var best_cal: usize = 0;
            var best_avg: f64 = 0;
            var best_peso: f64 = 0;
            for (g.axes) |a| {
                const ac = axisCat(a.nombre) orelse continue;
                if (ac != ci or a.calificadas == 0) continue;
                if (a.calificadas < best_cal) continue;
                best_cal = a.calificadas;
                best_avg = a.suma / @as(f64, @floatFromInt(a.calificadas));
                best_peso = a.peso;
            }
            if (best_cal == 0) continue;
            seg[ci] = best_avg * best_peso;
            total += seg[ci];
        }
        if (total > 0.001) bars.append(.{ .name = g.materia, .seg = seg, .total = total }) catch {};
    }
    if (bars.items.len == 0) return chartEmpty(
        allocator,
        "Sin aportes todavía",
        "Cuando haya actividades calificadas verás cómo se forma cada promedio.",
        "/dashboard/subjects",
        "Ver asignaturas",
    );
    std.mem.sort(Bar, bars.items, {}, struct {
        fn lt(_: void, a: Bar, b: Bar) bool {
            return a.total > b.total;
        }
    }.lt);

    const shown = @min(bars.items.len, 6);
    const row: i32 = 27;
    const top: i32 = 8;
    const left: i32 = 104;
    const bar_w: i32 = 188;
    const height = top * 2 + row * @as(i32, @intCast(shown)) + 24;
    var out = std.ArrayList(u8).init(allocator);
    for (bars.items[0..shown], 0..) |b, k| {
        const y = top + @as(i32, @intCast(k)) * row;
        const name = try esc(allocator, try clipUtf8(allocator, b.name, 16));
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<text class=\"nx-chart-t\" x=\"0\" y=\"{d}\" dominant-baseline=\"middle\">{s}</text>" ++
                "<rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"10\" rx=\"5\" fill=\"var(--nx-surface-3)\"/>",
            .{ y + 9, name, left, y + 3, bar_w },
        )) catch {};
        var x = left;
        for (0..4) |ci| {
            const c = b.seg[ci];
            if (c <= 0) continue;
            const w: i32 = @intFromFloat(@round(@as(f64, @floatFromInt(bar_w)) * (c / 10.0)));
            if (w <= 0) continue;
            out.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<rect data-series=\"{d}\" x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"10\" fill=\"{s}\"><title>{s}: {d:.2}</title></rect>",
                .{ ci, x, y + 3, w, axis_cats[ci].color, axis_cats[ci].label, c },
            )) catch {};
            x += w;
        }
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<text class=\"nx-chart-v\" x=\"{d}\" y=\"{d}\" dominant-baseline=\"middle\">{d:.2}</text>",
            .{ left + bar_w + 8, y + 9, b.total },
        )) catch {};
    }
    const legend_y = top + @as(i32, @intCast(shown)) * row + 10;
    const legend_x = [_]i32{ 0, 70, 146, 250 };
    for (0..4) |ci| {
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<rect x=\"{d}\" y=\"{d}\" width=\"8\" height=\"8\" rx=\"2\" fill=\"{s}\"/><text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\" dominant-baseline=\"middle\">{s}</text>",
            .{ legend_x[ci], legend_y - 4, axis_cats[ci].color, legend_x[ci] + 12, legend_y, axis_cats[ci].label },
        )) catch {};
    }
    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 340 {d}\" role=\"img\" aria-label=\"Aporte de cada tipo de evaluacion por materia\">{s}</svg>",
        .{ height, out.items },
    );
}

fn dayIndex(dia: []const u8) ?usize {
    const names = [_][]const u8{ "lunes", "martes", "miercoles", "jueves", "viernes", "sabado", "domingo" };
    for (names, 0..) |n, i| {
        if (std.mem.eql(u8, dia, n)) return i;
    }
    return null;
}

fn leadingHour(s: []const u8) ?u32 {
    var i: usize = 0;
    var val: u32 = 0;
    while (i < s.len and s[i] >= '0' and s[i] <= '9') : (i += 1) {
        val = val * 10 + (s[i] - '0');
    }
    if (i == 0 or i >= s.len or s[i] != ':') return null;
    return val;
}

/// Mapa de calor día × bloque horario a partir del horario real.
pub fn chartHeatmap(allocator: std.mem.Allocator, d: Datos) ![]const u8 {
    if (d.horario.len == 0) return chartEmpty(
        allocator,
        "Sin horario todavía",
        "Cuando Mikareno publique tu horario verás aquí la carga de la semana.",
        "/dashboard/schedule",
        "Ver horario",
    );
    const days = [_][]const u8{ "Lun", "Mar", "Mié", "Jue", "Vie", "Sáb", "Dom" };
    const blocks = [_][]const u8{ "6–8", "8–10", "10–12", "12–14", "14–16", "16–18" };
    var counts: [7][6]u32 = std.mem.zeroes([7][6]u32);
    var max: u32 = 0;
    for (d.horario) |c| {
        const di = dayIndex(c.dia) orelse continue;
        const h = leadingHour(c.etiqueta) orelse continue;
        const bi: usize = if (h < 8) 0 else if (h < 10) 1 else if (h < 12) 2 else if (h < 14) 3 else if (h < 16) 4 else 5;
        counts[di][bi] += 1;
        max = @max(max, counts[di][bi]);
    }
    if (max == 0) return chartEmpty(
        allocator,
        "Sin bloques que contar",
        "El horario llegó sin horas legibles; revisa la sincronización con Mikareno.",
        "/dashboard/schedule",
        "Ver horario",
    );
    const left: i32 = 34;
    const top: i32 = 16;
    const cell_w: i32 = 46;
    const cell_h: i32 = 20;
    const width = left + cell_w * 6 + 6;
    const height = top + cell_h * 7 + 6;
    const fmax = @as(f64, @floatFromInt(max));
    var out = std.ArrayList(u8).init(allocator);
    for (blocks, 0..) |label, i| {
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<text class=\"nx-chart-axis\" x=\"{d}\" y=\"10\" text-anchor=\"middle\">{s}</text>",
            .{ left + cell_w * @as(i32, @intCast(i)) + @divTrunc(cell_w, 2), label },
        )) catch {};
    }
    for (days, 0..) |label, i| {
        out.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<text class=\"nx-chart-axis\" x=\"{d}\" y=\"{d}\" text-anchor=\"end\" dominant-baseline=\"middle\">{s}</text>",
            .{ left - 6, top + cell_h * @as(i32, @intCast(i)) + @divTrunc(cell_h, 2), label },
        )) catch {};
    }
    for (0..7) |di| {
        for (0..6) |bi| {
            const v = counts[di][bi];
            const x = left + cell_w * @as(i32, @intCast(bi));
            const y = top + cell_h * @as(i32, @intCast(di));
            if (v == 0) {
                out.appendSlice(try std.fmt.allocPrint(
                    allocator,
                    "<rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"{d}\" rx=\"4\" fill=\"var(--nx-surface-3)\" fill-opacity=\".45\"/>",
                    .{ x + 1, y + 1, cell_w - 3, cell_h - 3 },
                )) catch {};
                continue;
            }
            const alpha = 0.18 + 0.82 * (@as(f64, @floatFromInt(v)) / fmax);
            out.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<rect x=\"{d}\" y=\"{d}\" width=\"{d}\" height=\"{d}\" rx=\"4\" fill=\"#8b5cf6\" fill-opacity=\"{d:.2}\"><title>{s} {s}: {d} clase(s)</title></rect>" ++
                    "<text class=\"nx-chart-v\" x=\"{d}\" y=\"{d}\" text-anchor=\"middle\" dominant-baseline=\"middle\">{d}</text>",
                .{ x + 1, y + 1, cell_w - 3, cell_h - 3, alpha, days[di], blocks[bi], v, x + @divTrunc(cell_w, 2), y + @divTrunc(cell_h, 2), v },
            )) catch {};
        }
    }
    return std.fmt.allocPrint(
        allocator,
        "<svg class=\"nx-chart\" viewBox=\"0 0 {d} {d}\" role=\"img\" aria-label=\"Mapa de calor de clases por dia y bloque horario\">{s}</svg>",
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

// ---------------------------------------------------------------------------
// Conexiones del equipo (página de Ajustes)
//
// Cada tarjeta dice lo que de verdad hay: servicio activo, herramienta
// instalada o nada. No se muestran claves ni rutas de credenciales.

const ConnState = enum { on, soft, off };

fn envValue(allocator: std.mem.Allocator, name: []const u8) []const u8 {
    return std.process.getEnvVarOwned(allocator, name) catch "";
}

fn absExists(path: []const u8) bool {
    if (path.len == 0) return false;
    std.fs.cwd().access(path, .{}) catch return false;
    return true;
}

fn homePathExists(allocator: std.mem.Allocator, home: []const u8, rel: []const u8) bool {
    const path = std.fmt.allocPrint(allocator, "{s}/{s}", .{ home, rel }) catch return false;
    defer allocator.free(path);
    return absExists(path);
}

/// Busca `needle` en el cmdline de los procesos vivos (sin lanzar `ps`).
fn processRunning(allocator: std.mem.Allocator, needle: []const u8) bool {
    var dir = std.fs.openDirAbsolute("/proc", .{ .iterate = true }) catch return false;
    defer dir.close();
    var it = dir.iterate();
    while (it.next() catch null) |entry| {
        if (entry.kind != .directory) continue;
        if (entry.name.len == 0 or entry.name[0] < '0' or entry.name[0] > '9') continue;
        var buf: [64]u8 = undefined;
        const path = std.fmt.bufPrint(&buf, "/proc/{s}/cmdline", .{entry.name}) catch continue;
        const data = std.fs.cwd().readFileAlloc(allocator, path, 32 * 1024) catch continue;
        const hit = std.mem.indexOf(u8, data, needle) != null;
        allocator.free(data);
        if (hit) return true;
    }
    return false;
}

fn pidAlive(allocator: std.mem.Allocator, pid: u32) bool {
    _ = allocator;
    if (pid == 0) return false;
    var buf: [32]u8 = undefined;
    const path = std.fmt.bufPrint(&buf, "/proc/{d}", .{pid}) catch return false;
    std.fs.cwd().access(path, .{}) catch return false;
    return true;
}

fn portFromUrl(url: []const u8) []const u8 {
    const colon = std.mem.lastIndexOfScalar(u8, url, ':') orelse return "";
    var end = colon + 1;
    while (end < url.len and url[end] >= '0' and url[end] <= '9') : (end += 1) {}
    if (end == colon + 1) return "";
    return url[colon + 1 .. end];
}

/// Lee `~/.local/state/opencode/service.json` y comprueba que el PID siga vivo.
/// No lanza procesos ni asume que el servicio esté arriba.
pub fn opencodeInfo(allocator: std.mem.Allocator) OpenCode {
    const home = envValue(allocator, "HOME");
    if (home.len == 0) return .{};
    const path = std.fmt.allocPrint(allocator, "{s}/.local/state/opencode/service.json", .{home}) catch return .{};
    const data = std.fs.cwd().readFileAlloc(allocator, path, 64 * 1024) catch return .{};
    var info = OpenCode{};
    const parsed = std.json.parseFromSlice(std.json.Value, allocator, data, .{}) catch return info;
    defer parsed.deinit();
    if (parsed.value != .object) return info;
    if (parsed.value.object.get("url")) |v| {
        if (v == .string) info.url = allocator.dupe(u8, v.string) catch "";
    }
    if (parsed.value.object.get("version")) |v| {
        if (v == .string) info.version = allocator.dupe(u8, v.string) catch "";
    }
    if (parsed.value.object.get("pid")) |v| {
        if (v == .integer and v.integer > 0) info.pid = @intCast(@min(v.integer, std.math.maxInt(u32)));
    }
    info.running = pidAlive(allocator, info.pid);
    info.port = portFromUrl(info.url);
    return info;
}

fn opencodeBin(allocator: std.mem.Allocator) []const u8 {
    if (std.process.getEnvVarOwned(allocator, "OPENCODE_BIN")) |value| {
        if (value.len > 0) return value;
    } else |_| {}
    const home = envValue(allocator, "HOME");
    if (home.len > 0) {
        const path = std.fmt.allocPrint(allocator, "{s}/.npm-global/bin/opencode", .{home}) catch return "opencode";
        if (absExists(path)) return path;
    }
    return "opencode";
}

fn opencodeRun(allocator: std.mem.Allocator, argv: []const []const u8) ?std.process.Child.RunResult {
    return std.process.Child.run(.{
        .allocator = allocator,
        .argv = argv,
        .max_output_bytes = 64 * 1024,
    }) catch null;
}

/// Pide al servicio de OpenCode un enlace de emparejamiento de un solo uso.
/// Si el servicio está parado, intenta arrancarlo antes. Devuelve "" si falla.
pub fn opencodePairLink(allocator: std.mem.Allocator) ![]const u8 {
    const bin = opencodeBin(allocator);
    if (bin.len == 0) return "";
    if (!opencodeInfo(allocator).running) {
        // `timeout` evita que una petición HTTP se quede esperando al servicio.
        if (opencodeRun(allocator, &.{ "timeout", "12", bin, "service", "start" })) |result| {
            allocator.free(result.stdout);
            allocator.free(result.stderr);
        }
    }
    const result = opencodeRun(allocator, &.{ "timeout", "15", bin, "pair" }) orelse return "";
    defer {
        allocator.free(result.stdout);
        allocator.free(result.stderr);
    }
    const exited_ok = switch (result.term) {
        .Exited => |code| code == 0,
        else => false,
    };
    if (!exited_ok) return "";
    const out = result.stdout;
    const needle = "/auth/connect/";
    const idx = std.mem.indexOf(u8, out, needle) orelse return "";
    var start = idx;
    while (start > 0) : (start -= 1) {
        const ch = out[start - 1];
        if (ch == ' ' or ch == '\n' or ch == '\r' or ch == '\t' or ch == '"' or ch == '\'' or ch == '>' or ch == '<') break;
    }
    var end = idx;
    while (end < out.len) : (end += 1) {
        const ch = out[end];
        if (ch == ' ' or ch == '\n' or ch == '\r' or ch == '\t' or ch == '"' or ch == '\'' or ch == '>' or ch == '<') break;
    }
    const link = out[start..end];
    if (!std.mem.startsWith(u8, link, "http")) return "";
    return allocator.dupe(u8, link);
}

/// Tarjeta compacta tipo terminal. El botón abre OpenCode en esta misma
/// ventana: el backend pide un enlace de emparejamiento y redirige allí.
pub fn opencodeCard(allocator: std.mem.Allocator, info: OpenCode) ![]const u8 {
    const state_class = if (info.running) "is-on" else "is-off";
    const state_text = if (info.running) "Activo" else "Detenido";
    const version = if (info.version.len > 0) info.version else "sin servicio";
    const port = if (info.port.len > 0) info.port else "—";
    const hint = if (info.running)
        try std.fmt.allocPrint(allocator, "Un solo proceso · puerto {s} · PID {d}. Se abre en esta ventana; vuelve con Atrás.", .{ port, info.pid })
    else
        "El servicio está parado. Al abrir, NOVA lo inicia y te conecta a la única instancia.";
    return std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-card nx-oc\" aria-labelledby=\"oc-title\"><header class=\"nx-card-head\"><div><p class=\"nx-eyebrow\">Terminal</p><h2 id=\"oc-title\">OpenCode</h2></div><span class=\"nx-oc-state\"><i class=\"nx-status-dot {s}\" aria-hidden=\"true\"></i>{s}</span></header><div class=\"nx-oc-screen\"><div class=\"nx-oc-bar\"><i></i><i></i><i></i><span>opencode</span></div><pre class=\"nx-oc-log\"><span class=\"nx-oc-prompt\">$</span> opencode --version\n{s}\n<span class=\"nx-oc-prompt\">$</span> opencode serve\n{s}</pre></div><a class=\"nx-oc-open\" href=\"/dashboard/opencode/abrir\">Abrir OpenCode<span class=\"material-symbols-outlined\">arrow_forward</span></a><p class=\"nx-oc-hint\">{s}</p></section>",
        .{ state_class, state_text, version, if (info.running) try std.fmt.allocPrint(allocator, "escuchando en 127.0.0.1:{s}", .{port}) else "servicio detenido", hint },
    );
}

fn connCard(
    allocator: std.mem.Allocator,
    icon: []const u8,
    name: []const u8,
    kind: []const u8,
    desc: []const u8,
    state: ConnState,
    status: []const u8,
    detail: []const u8,
    href: ?[]const u8,
) ![]const u8 {
    const dot = switch (state) {
        .on => "is-on",
        .soft => "is-soft",
        .off => "is-off",
    };
    const safe_name = try esc(allocator, name);
    const safe_kind = try esc(allocator, kind);
    const safe_desc = try esc(allocator, desc);
    const safe_status = try esc(allocator, status);
    const safe_detail = try esc(allocator, detail);
    const action = if (href) |h|
        try std.fmt.allocPrint(allocator, "<a href=\"{s}\">Abrir<span class=\"material-symbols-outlined\">arrow_forward</span></a>", .{h})
    else
        "";
    return std.fmt.allocPrint(
        allocator,
        "<article class=\"nx-conn\"><header><span class=\"nx-conn-icon material-symbols-outlined\">{s}</span><div><h3>{s}</h3><p>{s}</p></div><span class=\"nx-conn-state\"><i class=\"nx-status-dot {s}\" aria-hidden=\"true\"></i>{s}</span></header><p class=\"nx-conn-desc\">{s}</p><footer><code>{s}</code>{s}</footer></article>",
        .{ icon, safe_name, safe_kind, dot, safe_status, safe_desc, safe_detail, action },
    );
}

pub fn connections(allocator: std.mem.Allocator, d: Datos, db: anytype) ![]const u8 {
    const home = envValue(allocator, "HOME");
    var out = std.ArrayList(u8).init(allocator);

    // Mikareno: datos académicos (solo lectura).
    const mikareno_state: ConnState = if (d.from_cache) .soft else if (d.connected) .on else .off;
    const mikareno_status = if (d.from_cache) "Caché" else if (d.connected) "Conectado" else "Sin conexión";
    try out.appendSlice(try connCard(
        allocator,
        "school",
        "Mikareno",
        "Académico · MCP",
        "Notas, horario, tareas y calendario del colegio, sólo lectura.",
        mikareno_state,
        mikareno_status,
        if (d.mikareno_mcp_url.len > 0) d.mikareno_mcp_url else "http://127.0.0.1:8080/mcp",
        "/dashboard/mcp",
    ));

    // GenOffice: documentos, hojas y presentaciones.
    const genoffice_url = envValue(allocator, "GENOFFICE_MCP_URL");
    try out.appendSlice(try connCard(
        allocator,
        "description",
        "GenOffice",
        "Oficina · MCP",
        "Crea y edita Word, Excel y PowerPoint con el GenOffice del equipo.",
        if (d.genoffice_connected) .on else .off,
        if (d.genoffice_connected) "Conectado" else "Sin conexión",
        if (genoffice_url.len > 0) genoffice_url else "http://127.0.0.1:8453/mcp",
        "/dashboard/mcp",
    ));

    // Google: OAuth de calendario, correo y classroom (solo lectura).
    const google_client = envValue(allocator, "GOOGLE_CLIENT_ID");
    const google_token = (db.scalarInt("SELECT COUNT(*) FROM google_oauth_tokens") catch 0) > 0;
    const google_state: ConnState = if (google_token) .on else if (google_client.len > 0) .soft else .off;
    const google_status = if (google_token) "Vinculado" else if (google_client.len > 0) "Cliente listo" else "Sin configurar";
    try out.appendSlice(try connCard(
        allocator,
        "calendar_month",
        "Google",
        "Cuenta · OAuth",
        "Calendario, Gmail y Classroom en modo lectura.",
        google_state,
        google_status,
        "Calendar · Gmail · Classroom",
        "/dashboard/calendar",
    ));

    // IA de la app.
    try out.appendSlice(try connCard(
        allocator,
        "auto_awesome",
        "NOVA AI",
        "Inteligencia artificial",
        "Chat con contexto, resúmenes y planes de estudio.",
        if (d.ai_configured) .on else .off,
        if (d.ai_configured) "Configurada" else "Sin clave",
        if (d.ai_model.len > 0) d.ai_model else "modelo sin configurar",
        "/dashboard/ai",
    ));

    // Hermes: agente personal (gateway + CLI).
    const hermes_installed = homePathExists(allocator, home, ".hermes") and homePathExists(allocator, home, ".local/bin/hermes");
    const hermes_running = processRunning(allocator, "/.hermes/hermes-agent") or processRunning(allocator, "hermes-gateway");
    const hermes_state: ConnState = if (hermes_running) .on else if (hermes_installed) .soft else .off;
    const hermes_status = if (hermes_running) "En ejecución" else if (hermes_installed) "Instalado" else "No encontrado";
    try out.appendSlice(try connCard(
        allocator,
        "smart_toy",
        "Hermes Agent",
        "Agente personal",
        "Gateway, CLI y automatizaciones del asistente de tu equipo.",
        hermes_state,
        hermes_status,
        "~/.hermes · gateway + CLI",
        null,
    ));

    // Puente de WhatsApp de Hermes.
    const wa_installed = homePathExists(allocator, home, ".hermes/whatsapp");
    const wa_running = processRunning(allocator, "whatsapp-bridge/bridge.js");
    const wa_state: ConnState = if (wa_running) .on else if (wa_installed) .soft else .off;
    try out.appendSlice(try connCard(
        allocator,
        "chat",
        "Hermes WhatsApp",
        "Mensajería",
        "Puente de mensajes de Hermes para leer y responder por WhatsApp.",
        wa_state,
        if (wa_running) "Conectado" else if (wa_installed) "Instalado" else "No encontrado",
        "127.0.0.1:3000 · sesión local",
        null,
    ));

    // CyberHex: CLI y proyecto local.
    const hex_installed = homePathExists(allocator, home, ".local/bin/hex");
    const hex_project = homePathExists(allocator, home, "proyectos/CyberHex");
    const hex_state: ConnState = if (hex_installed) .on else if (hex_project) .soft else .off;
    try out.appendSlice(try connCard(
        allocator,
        "terminal",
        "CyberHex",
        "Herramienta CLI",
        "Laboratorio y utilidades de seguridad desde la terminal.",
        hex_state,
        if (hex_installed) "CLI instalada" else if (hex_project) "Proyecto local" else "No encontrado",
        if (hex_installed) "~/.local/bin/hex · ~/proyectos/CyberHex" else "~/proyectos/CyberHex",
        null,
    ));

    // MCP propio de NOVA.
    const mcp_key = envValue(allocator, "CUSTOM_MCP_API_KEY");
    try out.appendSlice(try connCard(
        allocator,
        "hub",
        "MCP de NOVA",
        "Integración · MCP",
        "Expone notas, calendario y herramientas a OpenCode y agentes.",
        if (mcp_key.len > 0) .on else .off,
        if (mcp_key.len > 0) "Activo" else "Sin clave",
        "http://127.0.0.1:3100/mcp",
        "/dashboard/mcp",
    ));

    // OpenCode: agente de código en una sola instancia.
    try out.appendSlice(try connCard(
        allocator,
        "terminal",
        "OpenCode",
        "Agente · Terminal",
        "Una sola instancia con su propio puerto, lista para abrir desde el inicio.",
        if (d.opencode.running) .on else .soft,
        if (d.opencode.running) "Activo" else "Detenido",
        if (d.opencode.url.len > 0) d.opencode.url else "~/.local/state/opencode",
        "/dashboard/opencode/abrir",
    ));

    // Vault de Obsidian.
    const vault_ok = absExists(d.vault_path);
    try out.appendSlice(try connCard(
        allocator,
        "folder_open",
        "Segundo cerebro",
        "Vault · Obsidian",
        "Notas Markdown locales que la app lee y sincroniza.",
        if (vault_ok) .on else .soft,
        if (vault_ok) "Sincronizado" else "Sin carpeta",
        if (d.vault_path.len > 0) d.vault_path else "VAULT_DIR sin configurar",
        "/dashboard/notes",
    ));

    return out.items;
}

test "portFromUrl extrae el puerto del servicio de OpenCode" {
    try std.testing.expectEqualStrings("49374", portFromUrl("http://127.0.0.1:49374"));
    try std.testing.expectEqualStrings("", portFromUrl("http://127.0.0.1"));
    try std.testing.expectEqualStrings("", portFromUrl(""));
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

/// `...::T3:1::TRABAJOS` → `TRABAJOS` cuando es un submenú conocido.
fn supplyAxisName(supply: []const u8) ?[]const u8 {
    const at = std.mem.lastIndexOf(u8, supply, "::") orelse return null;
    const name = supply[at + 2 ..];
    return if (submenuWeight(name) == null) null else name;
}

fn collectPartialIds(node: json.Value, out: *std.ArrayList([]const u8)) void {
    if (json.get(node, "children")) |kids| {
        if (kids == .array) for (kids.array.items) |c| collectPartialIds(c, out);
    }
    const pid = json.getString(node, "partialId") orelse return;
    if (pid.len == 0) return;
    for (out.items) |seen| if (std.mem.eql(u8, seen, pid)) return;
    out.append(pid) catch {};
}

fn collectAxes(node: json.Value, parcial: []const u8, out: *std.ArrayList(Axis)) void {
    if (json.get(node, "children")) |kids| {
        if (kids == .array and kids.array.items.len > 0) {
            for (kids.array.items) |c| collectAxes(c, parcial, out);
            return;
        }
    }
    const pid = json.getString(node, "partialId") orelse return;
    if (!std.mem.eql(u8, pid, parcial)) return;
    const supply = json.getString(node, "supplyId") orelse return;
    const name = supplyAxisName(supply) orelse return;
    const peso = submenuWeight(name) orelse return;
    var total: usize = 0;
    var calificadas: usize = 0;
    var suma: f64 = 0;
    if (json.get(node, "scoreValues")) |sv| {
        if (sv == .array) for (sv.array.items) |v| {
            total += 1;
            const raw = json.getString(v, "value") orelse continue;
            if (raw.len == 0) continue;
            const n = std.fmt.parseFloat(f64, std.mem.trim(u8, raw, " \t")) catch continue;
            calificadas += 1;
            suma += n;
        };
    }
    if (total == 0 and peso != weight_examen) return;
    out.append(.{
        .nombre = name,
        .peso = peso,
        .total = if (total == 0) 1 else total,
        .calificadas = calificadas,
        .suma = suma,
    }) catch {};
}

fn mergeAxes(axes: *std.ArrayList(Axis)) void {
    var i: usize = 0;
    while (i < axes.items.len) : (i += 1) {
        var j = i + 1;
        while (j < axes.items.len) {
            if (std.mem.eql(u8, axes.items[i].nombre, axes.items[j].nombre)) {
                axes.items[i].total += axes.items[j].total;
                axes.items[i].calificadas += axes.items[j].calificadas;
                axes.items[i].suma += axes.items[j].suma;
                _ = axes.orderedRemove(j);
            } else j += 1;
        }
    }
}

/// `stats.courseCode` llega como `26EGB10A-LLAG - Lengua y Literatura`,
/// mientras que el esquema trae solo `26EGB10A-LLAG`: acepta el prefijo
/// cuando el separador es un espacio o un guion.
fn gradeCodeMatches(scheme: []const u8, grade: []const u8) bool {
    if (scheme.len == 0 or grade.len == 0) return false;
    if (std.mem.eql(u8, scheme, grade)) return true;
    if (grade.len > scheme.len and std.mem.startsWith(u8, grade, scheme)) {
        const next = grade[scheme.len];
        return next == ' ' or next == '-';
    }
    return false;
}

pub const SubjectAxes = struct {
    codigo: []const u8,
    parcial: []const u8,
    axes: []const Axis,
};

/// Desglose por submenú de cada materia a partir del árbol que ya trae
/// `get_student_full`. Evita pedir el detalle de las 15 materias.
fn parseSubjectAxes(allocator: std.mem.Allocator, full: json.Value) []SubjectAxes {
    const ec = json.get(full, "estructura_calificaciones") orelse return &.{};
    const schemes = json.get(ec, "scoreSchemes") orelse return &.{};
    if (schemes != .array) return &.{};
    var out = std.ArrayList(SubjectAxes).init(allocator);
    for (schemes.array.items) |sc| {
        const course = json.get(sc, "course") orelse continue;
        const codigo = json.getString(course, "courseCode") orelse json.getString(course, "code") orelse continue;
        const root = json.get(sc, "root") orelse continue;
        var parciales = std.ArrayList([]const u8).init(allocator);
        collectPartialIds(root, &parciales);
        for (parciales.items) |pid| {
            var axes = std.ArrayList(Axis).init(allocator);
            collectAxes(root, pid, &axes);
            if (axes.items.len == 0) continue;
            mergeAxes(&axes);
            out.append(.{ .codigo = codigo, .parcial = pid, .axes = axes.items }) catch {};
        }
    }
    return out.items;
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
    const real_txt = if (g.real) |r| try std.fmt.allocPrint(allocator, "{d:.2}", .{r}) else try allocator.dupe(u8, "—");
    const real_tone = if (g.real) |r| toneFor(r) else Tone{ .color = "#7f8ea3", .label = "sin datos", .pct = 0 };
    const axes_goal = try axisGoalHtml(allocator, g.axes);
    const q_exam = (plan_target - g.nota) / weight_examen;
    const goal = if (g.nota >= plan_target)
        try allocator.dupe(u8, "meta de 8 cumplida")
    else if (axes_goal.len > 0)
        try std.fmt.allocPrint(allocator, "para 8: {s}", .{axes_goal})
    else if (q_exam <= 10)
        try std.fmt.allocPrint(allocator, "para 8: examen ≥ {d:.1}", .{q_exam})
    else
        try allocator.dupe(u8, "para 8: el examen solo no basta");
    const goal_real = if (g.real) |r|
        if (r >= plan_target)
            try std.fmt.allocPrint(allocator, "solo calificado: {d:.2} · meta cumplida", .{r})
        else
            try std.fmt.allocPrint(allocator, "solo calificado: {d:.2} · faltan {d:.2}", .{ r, plan_target - r })
    else
        try allocator.dupe(u8, "solo calificado: sin datos");
    const real_sort = if (g.real) |r| try std.fmt.allocPrint(allocator, "{d:.2}", .{r}) else try allocator.dupe(u8, "");
    return std.fmt.allocPrint(
        allocator,
        "<a href=\"/dashboard/subjects/{s}\" class=\"nx-subject-card nova-subject-card\" data-oficial=\"{s}\" data-real=\"{s}\">" ++
            "<div class=\"nx-subject-top\"><span class=\"nx-subject-mark\"><span class=\"material-symbols-outlined\">menu_book</span></span>{s}<span class=\"material-symbols-outlined nx-subject-arrow\">arrow_outward</span></div>" ++
            "<h3>{s}</h3>" ++
            "<div class=\"nx-subject-grade\"><strong style=\"color:{s}\" data-modo-oficial=\"{s}\" data-modo-real=\"{s}\" data-color-oficial=\"{s}\" data-color-real=\"{s}\">{s}</strong><span data-modo-oficial=\"{s}\" data-modo-real=\"{s}\">{s}</span></div>" ++
            "<div class=\"nx-subject-progress\"><div style=\"height:100%;width:{d}%;background:{s};border-radius:inherit\" data-bar-oficial=\"{d}\" data-bar-real=\"{d}\" data-bar-color-oficial=\"{s}\" data-bar-color-real=\"{s}\"></div></div>" ++
            "<div class=\"nx-subject-goal\"><span class=\"material-symbols-outlined\">flag</span><span data-modo-oficial=\"{s}\" data-modo-real=\"{s}\">{s}</span></div>" ++
            "<footer><span>Sobre {d:.0} puntos</span><span>Ver actividades<span class=\"material-symbols-outlined\">arrow_forward</span></span></footer></a>",
        .{ href_name, nota, real_sort, chip, name, tone.color, nota, real_txt, tone.color, real_tone.color, nota, tone.label, real_tone.label, tone.label, tone.pct, tone.color, tone.pct, real_tone.pct, tone.color, real_tone.color, goal, goal_real, goal, g.maximo },
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
    var real_count: u32 = 0;
    var real_total: f64 = 0;
    var real_approved: u32 = 0;
    var real_risk: u32 = 0;
    var real_failed: u32 = 0;
    for (d.grades) |g| {
        count += 1;
        if (g.tiene_nota) {
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
        if (g.real) |r| {
            real_count += 1;
            real_total += r;
            if (r >= 7) {
                real_approved += 1;
            } else if (r >= 5) {
                real_risk += 1;
            } else {
                real_failed += 1;
            }
        }
    }
    const avg_txt = if (with_note > 0)
        try std.fmt.allocPrint(allocator, "{d:.2}", .{total / @as(f64, @floatFromInt(with_note))})
    else
        try allocator.dupe(u8, "—");
    const avg_real = if (real_count > 0)
        try std.fmt.allocPrint(allocator, "{d:.2}", .{real_total / @as(f64, @floatFromInt(real_count))})
    else
        try allocator.dupe(u8, "—");
    const small_oficial = try std.fmt.allocPrint(allocator, "{d} con nota", .{with_note});
    const small_real = try std.fmt.allocPrint(allocator, "{d} con lo calificado", .{real_count});
    return std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-grade-kpis\" aria-label=\"Resumen de notas\">" ++
            "<div class=\"nx-grade-kpi\"><span>Promedio general</span><strong data-modo-oficial=\"{s}\" data-modo-real=\"{s}\">{s}</strong><small>sobre 10</small></div>" ++
            "<div class=\"nx-grade-kpi\"><span>Materias</span><strong>{d}</strong><small data-modo-oficial=\"{s}\" data-modo-real=\"{s}\">{s}</small></div>" ++
            "<div class=\"nx-grade-kpi is-good\"><span>Aprobadas</span><strong data-modo-oficial=\"{d}\" data-modo-real=\"{d}\">{d}</strong><small>7.0 o más</small></div>" ++
            "<div class=\"nx-grade-kpi is-warn\"><span>En riesgo</span><strong data-modo-oficial=\"{d}\" data-modo-real=\"{d}\">{d}</strong><small>5.0 – 6.99</small></div>" ++
            "<div class=\"nx-grade-kpi is-bad\"><span>Reprobadas</span><strong data-modo-oficial=\"{d}\" data-modo-real=\"{d}\">{d}</strong><small>menos de 5.0</small></div>" ++
            "</section>",
        .{ avg_txt, avg_real, avg_txt, count, small_oficial, small_real, small_oficial, approved, real_approved, approved, risk, real_risk, risk, failed, real_failed, failed },
    );
}

const modo_script =
    \\<script>(function(){
    \\var KEY='stypModoNotas';
    \\var root=document.body;
    \\var btn=document.querySelector('[data-modo-toggle]');
    \\if(!btn)return;
    \\function each(sel,fn){Array.prototype.forEach.call(document.querySelectorAll(sel),fn);}
    \\function sortCards(){
    \\var grid=document.querySelector('.nx-subject-grid');if(!grid)return;
    \\var real=root.classList.contains('nx-real-mode');
    \\var cards=Array.prototype.slice.call(grid.children);
    \\cards.sort(function(a,b){
    \\var ka=real?a.getAttribute('data-real'):a.getAttribute('data-oficial');
    \\var kb=real?b.getAttribute('data-real'):b.getAttribute('data-oficial');
    \\var na=ka?parseFloat(ka):-1,nb=kb?parseFloat(kb):-1;
    \\if(na<0&&nb<0)return 0;if(na<0)return 1;if(nb<0)return -1;
    \\return na-nb;
    \\});
    \\cards.forEach(function(c){grid.appendChild(c);});
    \\}
    \\function apply(real){
    \\root.classList.toggle('nx-real-mode',real);
    \\each('[data-modo-real]',function(el){
    \\var v=el.getAttribute(real?'data-modo-real':'data-modo-oficial');
    \\if(v!==null)el.textContent=v;
    \\var c=el.getAttribute(real?'data-color-real':'data-color-oficial');
    \\if(c)el.style.color=c;
    \\});
    \\each('[data-bar-real]',function(el){
    \\var w=el.getAttribute(real?'data-bar-real':'data-bar-oficial');
    \\var c=el.getAttribute(real?'data-bar-color-real':'data-bar-color-oficial');
    \\if(w)el.style.width=w+'%';
    \\if(c)el.style.background=c;
    \\});
    \\btn.setAttribute('aria-checked',real?'true':'false');
    \\var l=btn.querySelector('[data-modo-label]');
    \\if(l)l.textContent=real?'Solo lo calificado':'Nota del colegio';
    \\var note=document.querySelector('[data-modo-note]');
    \\if(note)note.textContent=real?'Reordenado por lo calificado. Lo pendiente y el examen sin subir no cuentan.':'Ordenado por la nota del colegio (lo pendiente y el examen sin subir cuentan como 0).';
    \\sortCards();
    \\}
    \\btn.hidden=false;
    \\try{apply(localStorage.getItem(KEY)==='1');}catch(e){}
    \\btn.addEventListener('click',function(){
    \\var real=!root.classList.contains('nx-real-mode');
    \\try{localStorage.setItem(KEY,real?'1':'0');}catch(e){}
    \\apply(real);
    \\});
    \\})();</script>
;

fn modoToggleHtml() []const u8 {
    return "<button type=\"button\" class=\"nx-modo-toggle\" role=\"switch\" aria-checked=\"false\" hidden data-modo-toggle title=\"Alternar entre la nota del colegio y solo lo calificado\"><span class=\"nx-modo-knob\" aria-hidden=\"true\"></span><span data-modo-label>Nota del colegio</span></button>";
}

pub fn gradesPageHtml(allocator: std.mem.Allocator, d: Datos, portal_url: []const u8) ![]const u8 {
    const banner = try syncBanner(allocator, d);
    const kpis = try gradesKpis(allocator, d);
    const grid = try gradesGrid(allocator, d);
    const label = if (d.partial_label.len > 0) d.partial_label else "Parcial actual";
    const safe_label = try esc(allocator, label);
    const portal = if (std.mem.startsWith(u8, portal_url, "https://") or std.mem.startsWith(u8, portal_url, "http://"))
        try std.fmt.allocPrint(
            allocator,
            "<a class=\"nx-button nx-button-primary\" href=\"{s}\" target=\"_blank\" rel=\"noopener\" title=\"Abrir tus notas en Mikareno\"><span class=\"material-symbols-outlined\">open_in_new</span>Libreta oficial</a>",
            .{try esc(allocator, portal_url)},
        )
    else
        "";
    const caption = if (d.grades.len == 0)
        "Datos reales de Mikareno; si no hay notas, la página lo dice."
    else
        try std.fmt.allocPrint(allocator, "{d} asignaturas · datos reales de Mikareno · primero lo que hay que recuperar.", .{d.grades.len});
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-page nx-subjects nova-page nova-subjects-page\">" ++
            "<header class=\"nx-page-header nova-page-head\"><div><p class=\"nx-eyebrow\">Aprendizaje · Notas</p><h1>Tu progreso,<br>materia por materia.</h1><p>{s}</p></div>" ++
            "<div class=\"nx-button-row\">" ++
            "{s}{s}" ++
            "<a class=\"nx-button nx-button-secondary\" href=\"/dashboard/subjects?refresh=1\"><span class=\"material-symbols-outlined\">refresh</span>Actualizar datos</a>" ++
            "</div></header>" ++
            "{s}{s}" ++
            "<section class=\"nx-collection nova-subjects-collection\"><div class=\"nx-section-head\"><div><p class=\"nx-eyebrow\">{s}</p><h2>Progreso por asignatura</h2></div><p class=\"nx-modo-note\" data-modo-note aria-live=\"polite\">Ordenado por la nota del colegio (lo pendiente y el examen sin subir cuentan como 0).</p></div>{s}</section>" ++
            "{s}</div>",
        .{ caption, modoToggleHtml(), portal, banner, kpis, safe_label, grid, modo_script },
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
    /// Promedio renormalizado a lo ya calificado; null si no hay nada con nota.
    real: ?f64 = null,
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
    out.real = realGradeAxes(try buildAxes(allocator, out.submenus));
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

/// Submenús del detalle normalizados para las cuentas. El examen sin
/// actividades queda como un pendiente (25%) para no perder su peso.
fn buildAxes(allocator: std.mem.Allocator, submenus: []const DetailSubmenu) ![]Axis {
    var out = std.ArrayList(Axis).init(allocator);
    var has_exam = false;
    for (submenus) |sm| {
        const w = submenuWeight(sm.nombre) orelse continue;
        var total = sm.total;
        if (total == 0) {
            if (w != weight_examen) continue;
            total = 1;
        }
        if (w == weight_examen) has_exam = true;
        const cal = @min(sm.calificadas, total);
        try out.append(.{
            .nombre = sm.nombre,
            .peso = w,
            .total = total,
            .calificadas = cal,
            .suma = if (cal == 0) 0 else sm.suma,
        });
    }
    // El árbol del detalle no siempre trae el submenú del examen: su 25%
    // sigue pendiente y debe aparecer en el desglose y en las cuentas.
    // Sin ningún submenú no se inventa nada (la carga pudo fallar).
    if (!has_exam and submenus.len > 0) try out.append(.{
        .nombre = "EXAMEN TRIMESTRAL",
        .peso = weight_examen,
        .total = 1,
        .calificadas = 0,
        .suma = 0,
    });
    return out.items;
}

/// Promedio renormalizado a lo que ya tiene nota: los submenús sin
/// calificar no restan y los pesos se reparten entre los que sí cuentan.
pub fn realGradeAxes(axes: []const Axis) ?f64 {
    var num: f64 = 0;
    var den: f64 = 0;
    for (axes) |a| {
        if (a.calificadas == 0) continue;
        num += a.peso * (a.suma / @as(f64, @floatFromInt(a.calificadas)));
        den += a.peso;
    }
    if (den <= 1e-9) return null;
    return num / den;
}

/// Media mínima en lo pendiente de `axes[i]` para llegar a `target`, con 10
/// en los demás ejes. `null` si ese eje no tiene pendientes.
fn axisMinNeeded(axes: []const Axis, i: usize, target: f64) ?f64 {
    if (i >= axes.len) return null;
    const a = axes[i];
    const cal = @min(a.calificadas, a.total);
    if (a.total <= cal or a.total == 0) return null;
    const pend = a.total - cal;
    var others_max: f64 = 0;
    for (axes, 0..) |o, j| {
        if (j == i or o.total == 0) continue;
        const oc = @min(o.calificadas, o.total);
        const on = o.total - oc;
        const den = @as(f64, @floatFromInt(o.total));
        others_max += o.peso * ((o.suma + 10.0 * @as(f64, @floatFromInt(on))) / den);
    }
    const den_i = @as(f64, @floatFromInt(a.total));
    const need_sum = (target - others_max) * den_i / a.peso;
    return (need_sum - a.suma) / @as(f64, @floatFromInt(pend));
}

fn shortAxisName(name: []const u8) []const u8 {
    if (containsIgnoreCase(name, "TAREA")) return "Tareas";
    if (containsIgnoreCase(name, "TRABAJO")) return "Trabajos";
    if (containsIgnoreCase(name, "EVALUAC")) return "Evaluaciones";
    if (containsIgnoreCase(name, "EXAMEN")) return "Examen";
    return name;
}

/// Resumen de todos los ejes del parcial para la tarjeta: el mínimo de
/// cada uno, o ✓ si ya no le quedan pendientes. Es la versión compacta
/// de la tabla del detalle.
fn axisGoalHtml(allocator: std.mem.Allocator, axes: []const Axis) ![]const u8 {
    if (axes.len == 0) return "";
    var buf = std.ArrayList(u8).init(allocator);
    for (axes, 0..) |a, i| {
        if (i > 0) try buf.appendSlice(" · ");
        const short = try esc(allocator, shortAxisName(a.nombre));
        const need = axisMinNeeded(axes, i, plan_target);
        if (need == null or need.? <= 0) {
            try buf.appendSlice(try std.fmt.allocPrint(allocator, "{s} ✓", .{short}));
        } else if (need.? <= 10) {
            try buf.appendSlice(try std.fmt.allocPrint(allocator, "{s} ≥{d:.1}", .{ short, need.? }));
        } else {
            try buf.appendSlice(try std.fmt.allocPrint(allocator, "{s} &gt;10", .{short}));
        }
    }
    return buf.items;
}

fn axisTableHtml(allocator: std.mem.Allocator, axes: []const Axis) ![]const u8 {
    if (axes.len == 0) return "";
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<div class=\"nx-plan-axes\"><p class=\"nx-plan-tasks-title\">Mínimo en cada eje para llegar a 8</p><ul>");
    for (axes, 0..) |a, i| {
        const name = try esc(allocator, a.nombre);
        const cal = @min(a.calificadas, a.total);
        const pend = a.total - cal;
        var meta: []const u8 = undefined;
        var value: []const u8 = undefined;
        var cls: []const u8 = "nx-plan-axis";
        if (pend == 0) {
            meta = try std.fmt.allocPrint(allocator, "{d} calificadas · sin pendientes", .{a.total});
            value = try std.fmt.allocPrint(allocator, "{d:.2}", .{a.suma / @as(f64, @floatFromInt(a.total))});
        } else {
            if (cal == 0) {
                meta = try std.fmt.allocPrint(allocator, "{d} por calificar · sin nota", .{a.total});
            } else {
                meta = try std.fmt.allocPrint(allocator, "{d} de {d} por calificar · llevas {d:.2}", .{ pend, a.total, a.suma / @as(f64, @floatFromInt(cal)) });
            }
            const need = axisMinNeeded(axes, i, plan_target) orelse 0;
            if (need <= 0) {
                cls = "nx-plan-axis is-good";
                value = "ya asegurado";
            } else if (need <= 10) {
                cls = "nx-plan-axis is-good";
                value = try std.fmt.allocPrint(allocator, "≥ {d:.2} / 10", .{need});
            } else {
                cls = "nx-plan-axis is-bad";
                value = try std.fmt.allocPrint(allocator, "imposible solo aquí ({d:.2})", .{need});
            }
        }
        try buf.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<li class=\"{s}\"><span class=\"nx-plan-axis-name\">{s} <em>{d:.0}%</em></span><span class=\"nx-plan-axis-meta\">{s}</span><strong>{s}</strong></li>",
            .{ cls, name, a.peso * 100.0, meta, value },
        ));
    }
    try buf.appendSlice("</ul><p class=\"nx-plan-axis-note\">La cifra es la media mínima en lo que falta de ese eje si en lo que falta de los demás sacas 10. Es la vía más corta para llegar a 8.</p></div>");
    return buf.items;
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

const sim_script =
    \\<script>(function(){
    \\var s=document.querySelector('.nx-subject-sim');if(!s)return;
    \\var base=parseFloat(s.getAttribute('data-base'))||0;
    \\var cp=parseFloat(s.getAttribute('data-pend'))||0;
    \\var ce=parseFloat(s.getAttribute('data-exam'))||0;
    \\var target=parseFloat(s.getAttribute('data-target'))||8;
    \\var pr=s.querySelector('[data-sim="pend"]');
    \\var er=s.querySelector('[data-sim="exam"]');
    \\var out=s.querySelector('[data-sim-result]');
    \\var hint=s.querySelector('[data-sim-hint]');
    \\var po=s.querySelector('[data-out="pend"]');
    \\var eo=s.querySelector('[data-out="exam"]');
    \\function fmt(x){return (Math.round(x*100)/100).toFixed(2);}
    \\function upd(){
    \\var p=pr?parseFloat(pr.value):0;
    \\var e=er?parseFloat(er.value):0;
    \\var f=base+cp*p+ce*e;
    \\if(po)po.textContent=p.toFixed(1);
    \\if(eo)eo.textContent=e.toFixed(1);
    \\out.textContent=fmt(f);
    \\out.style.color=f>=7?'#2dd4bf':(f>=5?'#fbbf24':'#f87171');
    \\if(f>=target){hint.textContent='Llegas a la meta con estas notas.';}
    \\else{hint.textContent='Te faltan '+fmt(target-f)+' puntos para '+target.toFixed(0)+'.';}
    \\}
    \\if(pr)pr.addEventListener('input',upd);
    \\if(er)er.addEventListener('input',upd);
    \\upd();
    \\})();</script>
;

fn simulatorHtml(allocator: std.mem.Allocator, base: f64, pend: f64, exam: f64) ![]const u8 {
    var controls = std.ArrayList(u8).init(allocator);
    if (pend > 1e-9) {
        try controls.appendSlice(
            "<label class=\"nx-sim-control\"><span>Nota en lo pendiente <output data-out=\"pend\">8.0</output></span>" ++
                "<input type=\"range\" min=\"0\" max=\"10\" step=\"0.1\" value=\"8\" data-sim=\"pend\"></label>",
        );
    }
    if (exam > 1e-9) {
        try controls.appendSlice(
            "<label class=\"nx-sim-control\"><span>Examen trimestral <output data-out=\"exam\">8.0</output></span>" ++
                "<input type=\"range\" min=\"0\" max=\"10\" step=\"0.1\" value=\"8\" data-sim=\"exam\"></label>",
        );
    }
    if (controls.items.len == 0) return "";
    const initial = base + pend * 8.0 + exam * 8.0;
    const tone: []const u8 = if (initial >= 7) "#2dd4bf" else if (initial >= 5) "#fbbf24" else "#f87171";
    const hint_txt = if (initial >= plan_target)
        "Llegas a la meta con estas notas."
    else
        try std.fmt.allocPrint(allocator, "Te faltan {d:.2} puntos para {d:.0}.", .{ plan_target - initial, plan_target });
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-subject-sim nova-subject-sim\" data-base=\"{d:.6}\" data-pend=\"{d:.6}\" data-exam=\"{d:.6}\" data-target=\"{d:.2}\">" ++
            "<div class=\"nx-sim-head\"><p class=\"nx-eyebrow\">Simulador</p><h3>¿Y si saco…?</h3></div>" ++
            "<div class=\"nx-sim-controls\">{s}</div>" ++
            "<div class=\"nx-sim-result\" aria-live=\"polite\"><span>Nota final estimada</span><strong data-sim-result style=\"color:{s}\">{d:.2}</strong><small data-sim-hint>{s}</small></div>" ++
            "{s}" ++
            "</div>",
        .{ base, pend, exam, plan_target, controls.items, tone, initial, hint_txt, sim_script },
    );
}

/// Plan "qué necesito para llegar a 8" con los pesos reales del parcial.
///
/// Tres vías honestas, calculadas con datos reales:
///  * solo el examen trimestral (25%, hoy en 0);
///  * examen a 10 + una media uniforme `p` en las actividades pendientes;
///  * simulador interactivo con deslizadores de pendientes y examen.
pub fn planHtml(allocator: std.mem.Allocator, d: SubjectDetail) ![]const u8 {
    if (!d.tiene_nota) {
        return "<section class=\"nx-subject-plan nova-subject-plan\"><div class=\"nx-plan-head\"><p class=\"nx-eyebrow\">Plan para llegar a 8</p><h2>Sin promedio todavía</h2><p>Cuando Mikareno publique una nota, aquí aparecerá exactamente cuánto necesitas y en qué.</p></div></section>";
    }
    const current = d.nota;
    const axes = try buildAxes(allocator, d.submenus);
    var unknown = false;
    var unknown_names = std.ArrayList(u8).init(allocator);
    for (d.submenus) |sm| {
        if (sm.total == 0 or submenuWeight(sm.nombre) != null) continue;
        unknown = true;
        if (unknown_names.items.len > 0) unknown_names.appendSlice(", ") catch {};
        unknown_names.appendSlice(try esc(allocator, sm.nombre)) catch {};
    }
    var has_pending = false;
    var exam_graded = false;
    var denom: f64 = 0;
    var sum_wnm: f64 = 0;
    var submenus_max: f64 = 0;
    var pending_total: usize = 0;
    var sim_base: f64 = 0;
    var sim_pend: f64 = 0;

    for (axes) |a| {
        const cal = @min(a.calificadas, a.total);
        const pending = a.total - cal;
        const den = @as(f64, @floatFromInt(a.total));
        const n = @as(f64, @floatFromInt(pending));
        const c = @as(f64, @floatFromInt(cal));
        if (a.peso == weight_examen) {
            // Sin nota, su 25% se modela con el deslizador propio del examen.
            // Es también el caso en que el árbol no trae el submenú.
            if (cal == 0) continue;
            exam_graded = true;
        }
        submenus_max += a.peso * ((a.suma + 10.0 * n) / den);
        sim_base += a.peso * a.suma / den;
        sim_pend += a.peso * n / den;
        if (pending > 0) {
            has_pending = true;
            pending_total += pending;
            const m = if (cal > 0) a.suma / c else 0.0;
            denom += a.peso * n / den;
            sum_wnm += a.peso * n * m / den;
        }
    }
    const exam_w: f64 = if (exam_graded) 0.0 else weight_examen;
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
        if (p <= 10) break :blk try std.fmt.allocPrint(allocator, "<div class=\"nx-plan-lever is-good\"><span>Actividades pendientes</span><strong>≥ {d:.2}</strong><small>{s} · {d} entregas por calificar.</small></div>", .{ p, if (exam_w > 0) "Con 10 en el examen" else "En las entregas pendientes", pending_total });
        break :blk "<div class=\"nx-plan-lever is-bad\"><span>Actividades pendientes</span><strong>&gt; 10</strong><small>Ni con todo a 10 se llega a 8 este parcial.</small></div>";
    } else "<div class=\"nx-plan-lever\"><span>Actividades pendientes</span><strong>—</strong><small>Sin actividades computables.</small></div>";

    const max_lever = try std.fmt.allocPrint(allocator, "<div class=\"nx-plan-lever\"><span>Máximo alcanzable</span><strong>{d:.2}</strong><small>Si sacas 10 en todo lo que queda.</small></div>", .{submenus_max});
    const axes_block = try axisTableHtml(allocator, axes);

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
    const note_base = if (unknown)
        try std.fmt.allocPrint(allocator, "Hay un submenú con un esquema no reconocido ({s}); el plan podría variar. Los pesos mostrados se deducen de tus promedios reales y coinciden con las 14 materias calificadas.", .{unknown_names.items})
    else
        "Pesos deducidos de tus promedios reales (error ≤ 0.011 en las 14 materias). Dentro de cada submenú todas las actividades valen lo mismo; el examen trimestral vale 25% y hoy cuenta como 0.";
    const real_note = if (d.real) |r|
        try std.fmt.allocPrint(allocator, " Con solo lo calificado tienes {d:.2}; este plan usa el esquema oficial del colegio.", .{r})
    else
        "";
    const note = try std.fmt.allocPrint(allocator, "<p class=\"nx-plan-note\">{s}{s}</p>", .{ note_base, real_note });

    const sim = try simulatorHtml(allocator, sim_base, sim_pend, exam_w);
    return std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-subject-plan nova-subject-plan\"><div class=\"nx-plan-head\"><p class=\"nx-eyebrow\">Plan para llegar a 8</p>{s}</div>" ++
            "<div class=\"nx-plan-levers\">{s}{s}{s}</div>{s}{s}{s}{s}{s}</section>",
        .{ head, exam_lever, pending_lever, max_lever, axes_block, sim, tasks.items, weights, note },
    );
}

/// Radiografía de la materia con datos reales: anillo del promedio, barras
/// por bloque ponderado y reparto de actividades calificadas/pendientes.
/// Si no hay nada que dibujar devuelve cadena vacía; nunca inventa ceros.
pub fn subjectChartsHtml(allocator: std.mem.Allocator, d: SubjectDetail) ![]const u8 {
    const max = if (d.maximo > 0) d.maximo else 10.0;
    var cards = std.ArrayList(u8).init(allocator);

    const shown: ?f64 = if (d.real) |r| r else if (d.tiene_nota) d.nota else null;
    if (shown) |value| {
        const pct = @max(0.0, @min(100.0, value / max * 100.0));
        const value_txt = try std.fmt.allocPrint(allocator, "{d:.2}", .{value});
        const official = if (d.tiene_nota) try std.fmt.allocPrint(allocator, "{d:.2}", .{d.nota}) else "—";
        const foot = try std.fmt.allocPrint(allocator, "sobre {d:.0} · oficial {s}", .{ max, official });
        const label = if (d.real != null) "Solo calificado" else "Promedio";
        try cards.appendSlice("<div class=\"nx-subject-chart-card\">");
        try cards.appendSlice(try charts.donut(allocator, pct, charts.gradeColor(value), value_txt, label, foot));
        try cards.appendSlice("</div>");
    }

    var bars = std.ArrayList(charts.Bar).init(allocator);
    for (d.submenus) |sm| {
        const sub = sm.subtotal orelse continue;
        if (sm.total == 0) continue;
        const label = if (submenuWeight(sm.nombre)) |w|
            try std.fmt.allocPrint(allocator, "{s} · {d:.0}%", .{ sm.nombre, w * 100.0 })
        else
            try allocator.dupe(u8, sm.nombre);
        const txt = try std.fmt.allocPrint(allocator, "{d:.2}", .{sub});
        try bars.append(.{ .etiqueta = label, .valor = sub, .texto = txt, .color = charts.gradeColor(sub) });
    }
    if (bars.items.len > 0) {
        try cards.appendSlice("<div class=\"nx-subject-chart-card\">");
        try cards.appendSlice(try charts.barras(allocator, bars.items, max, "Subtotal de cada bloque, sobre la nota máxima."));
        try cards.appendSlice("</div>");
    }

    var graded: usize = 0;
    var pending: usize = 0;
    for (d.submenus) |sm| {
        const cal = @min(sm.calificadas, sm.total);
        graded += cal;
        pending += sm.total - cal;
    }
    if (graded + pending > 0) {
        const segs = [_]charts.Segmento{
            .{ .etiqueta = "Calificadas", .cantidad = graded, .color = charts.Palette.good },
            .{ .etiqueta = "Pendientes", .cantidad = pending, .color = charts.Palette.muted },
        };
        try cards.appendSlice("<div class=\"nx-subject-chart-card\">");
        try cards.appendSlice(try charts.apilada(allocator, &segs, graded + pending));
        try cards.appendSlice("</div>");
    }

    if (cards.items.len == 0) return "";
    return std.fmt.allocPrint(
        allocator,
        "<section class=\"nx-subject-charts nova-subject-charts\"><div class=\"nx-section-head\"><div><p class=\"nx-eyebrow\">Radiografía</p><h2>Cómo va la materia</h2></div><span>Datos de Mikareno, sin estimaciones</span></div><div class=\"nx-subject-charts-grid\">{s}</div></section>",
        .{cards.items},
    );
}

pub fn subjectDetailHtml(allocator: std.mem.Allocator, d: SubjectDetail) ![]const u8 {
    const name = try esc(allocator, d.materia);
    const code = try esc(allocator, d.codigo);
    const trim_txt = if (d.parcial.len > 0) try partialLabel(allocator, d.parcial) else try allocator.dupe(u8, "Parcial actual");
    const tone = if (d.tiene_nota) toneFor(d.nota) else Tone{ .color = "#7f8ea3", .label = "Sin calificar", .pct = 0 };
    const nota_txt = if (d.tiene_nota) try std.fmt.allocPrint(allocator, "{d:.2}", .{d.nota}) else try allocator.dupe(u8, "—");
    const real_txt = if (d.real) |r| try std.fmt.allocPrint(allocator, "{d:.2}", .{r}) else try allocator.dupe(u8, "—");
    const real_tone = if (d.real) |r| toneFor(r) else Tone{ .color = "#7f8ea3", .label = "sin datos", .pct = 0 };
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
    const charts_section = try subjectChartsHtml(allocator, d);

    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-page nx-subject-detail nova-page nova-subject-detail-page\">{s}" ++
            "<header class=\"nx-subject-hero nova-subject-detail-hero\"><a href=\"/dashboard/subjects\" class=\"nx-back-link\"><span class=\"material-symbols-outlined\">arrow_back</span>Todas las asignaturas</a>" ++
            "<div><p class=\"nx-eyebrow\">Detalle académico</p><h1>{s}</h1><p>Código {s} · {s}</p></div>" ++
            "<div class=\"nx-subject-note\">{s}<span class=\"nx-grade-chip\" style=\"color:{s}\">{s}</span></div></header>" ++
            "<section class=\"nx-subject-summary nova-subject-summary\"><div class=\"nx-subject-summary-grade\"><span>Promedio actual</span><strong style=\"color:{s}\" data-modo-oficial=\"{s}\" data-modo-real=\"{s}\" data-color-oficial=\"{s}\" data-color-real=\"{s}\">{s}</strong><p>sobre {s}</p><div class=\"nx-subject-progress\"><div style=\"height:100%;width:{d}%;background:{s};border-radius:inherit\" data-bar-oficial=\"{d}\" data-bar-real=\"{d}\" data-bar-color-oficial=\"{s}\" data-bar-color-real=\"{s}\"></div></div></div>" ++
            "<div class=\"nx-subject-summary-facts\"><div><span>Calificación</span><strong>{s}</strong></div><div><span>Solo calificado</span><strong style=\"color:{s}\">{s}</strong></div><div><span>Código</span><strong>{s}</strong></div><div><span>Máximo</span><strong>{s}</strong></div><div><span>Bloqueada</span><strong>{s}</strong></div><div><span>Tipo</span><strong>{s}</strong></div></div></section>" ++
            "{s}{s}{s}{s}{s}</div>",
        .{ banner, name, code, trim_txt, modoToggleHtml(), tone.color, tone.label, tone.color, nota_txt, real_txt, tone.color, real_tone.color, nota_txt, max_txt, tone.pct, tone.color, tone.pct, real_tone.pct, tone.color, real_tone.color, tone.label, real_tone.color, real_txt, code, max_txt, bloqueada, tipo, plan, charts_section, obs, acts.items, modo_script },
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
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-subject-sim") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-sim=\"pend\"></label>") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-sim=\"exam\"></label>") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-base=\"4.126") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-pend=\"0.244444\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-exam=\"0.250000\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "9.07") != null);
}

test "el simulador omite el examen cuando ya tiene nota" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var subs = [_]DetailSubmenu{
        .{ .nombre = "TAREAS", .total = 4, .calificadas = 2, .suma = 12, .subtotal = 6, .actividades = &.{} },
        .{ .nombre = "EXAMEN TRIMESTRAL", .total = 1, .calificadas = 1, .suma = 9, .subtotal = 9, .actividades = &.{} },
    };
    const d = SubjectDetail{ .tiene_nota = true, .nota = 3.5, .maximo = 10, .submenus = &subs };
    const html = try planHtml(a, d);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-sim=\"pend\"></label>") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-sim=\"exam\"></label>") == null);
    try std.testing.expect(std.mem.indexOf(u8, html, "con nota") != null);
}

test "el examen sin nota usa su propio deslizador" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var subs = [_]DetailSubmenu{
        .{ .nombre = "TAREAS", .total = 4, .calificadas = 2, .suma = 12, .subtotal = 6, .actividades = &.{} },
        .{ .nombre = "EXAMEN TRIMESTRAL", .total = 1, .calificadas = 0, .suma = 0, .subtotal = null, .actividades = &.{} },
    };
    const d = SubjectDetail{ .tiene_nota = true, .nota = 4, .maximo = 10, .submenus = &subs };
    const html = try planHtml(a, d);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-sim=\"exam\"></label>") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-exam=\"0.250000\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-base=\"0.600000\"") != null);
}

test "sin pendientes ni examen se omite el simulador" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var subs = [_]DetailSubmenu{
        .{ .nombre = "TAREAS", .total = 4, .calificadas = 4, .suma = 32, .subtotal = 8, .actividades = &.{} },
        .{ .nombre = "EXAMEN TRIMESTRAL", .total = 1, .calificadas = 1, .suma = 9, .subtotal = 9, .actividades = &.{} },
    };
    const d = SubjectDetail{ .tiene_nota = true, .nota = 8.1, .maximo = 10, .submenus = &subs };
    const html = try planHtml(a, d);
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-subject-sim") == null);
}

test "la ficha dibuja los tres gráficos con datos reales" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var subs = [_]DetailSubmenu{
        .{ .nombre = "TAREAS", .total = 9, .calificadas = 4, .suma = 34, .subtotal = 8.5, .actividades = &.{} },
        .{ .nombre = "TRABAJOS", .total = 3, .calificadas = 2, .suma = 16, .subtotal = 8.0, .actividades = &.{} },
        .{ .nombre = "EXAMEN TRIMESTRAL", .total = 1, .calificadas = 0, .suma = 0, .subtotal = null, .actividades = &.{} },
    };
    const d = SubjectDetail{ .tiene_nota = true, .nota = 6.13, .real = 6.5, .maximo = 10, .submenus = &subs };
    const html = try subjectChartsHtml(a, d);
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-chart-donut") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-chart-bars") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-chart-stack") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Solo calificado") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "TAREAS · 20%") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Pendientes") != null);
}

test "sin datos no se inventan gráficos" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const d = SubjectDetail{ .tiene_nota = false, .maximo = 10 };
    try std.testing.expectEqualStrings("", try subjectChartsHtml(a, d));
}

test "sin promedio no se inventa plan ni simulador" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const d = SubjectDetail{ .tiene_nota = false, .maximo = 10 };
    const html = try planHtml(a, d);
    try std.testing.expect(std.mem.indexOf(u8, html, "Sin promedio") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-subject-sim") == null);
}

test "un submenú desconocido avisa en el plan" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var subs = [_]DetailSubmenu{
        .{ .nombre = "TAREAS", .total = 4, .calificadas = 4, .suma = 32, .subtotal = 8, .actividades = &.{} },
        .{ .nombre = "LECCIONES", .total = 2, .calificadas = 0, .suma = 0, .subtotal = null, .actividades = &.{} },
    };
    const d = SubjectDetail{ .tiene_nota = true, .nota = 6, .maximo = 10, .submenus = &subs };
    const html = try planHtml(a, d);
    try std.testing.expect(std.mem.indexOf(u8, html, "esquema no reconocido") != null);
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

test "el mínimo por eje asume 10 en los demás" {
    const axes = [_]Axis{
        .{ .nombre = "TAREAS", .peso = weight_tareas, .total = 9, .calificadas = 4, .suma = 34 },
        .{ .nombre = "TRABAJOS", .peso = weight_trabajos, .total = 3, .calificadas = 2, .suma = 16 },
        .{ .nombre = "EVALUACIONES", .peso = weight_evaluaciones, .total = 2, .calificadas = 2, .suma = 16.5 },
        .{ .nombre = "EXAMEN TRIMESTRAL", .peso = weight_examen, .total = 1, .calificadas = 0, .suma = 0 },
    };
    try std.testing.expectApproxEqAbs(@as(f64, 0.3625), axisMinNeeded(&axes, 0, 8.0).?, 0.001);
    try std.testing.expectApproxEqAbs(@as(f64, 1.9688), axisMinNeeded(&axes, 1, 8.0).?, 0.001);
    try std.testing.expectApproxEqAbs(@as(f64, 5.7167), axisMinNeeded(&axes, 3, 8.0).?, 0.001);
    try std.testing.expect(axisMinNeeded(&axes, 2, 8.0) == null);
}

test "solo lo calificado renormaliza los pesos" {
    const axes = [_]Axis{
        .{ .nombre = "TAREAS", .peso = 0.20, .total = 4, .calificadas = 2, .suma = 12 },
        .{ .nombre = "TRABAJOS", .peso = 0.40, .total = 1, .calificadas = 1, .suma = 7 },
        .{ .nombre = "EVALUACIONES", .peso = 0.15, .total = 2, .calificadas = 2, .suma = 13.5 },
        .{ .nombre = "EXAMEN TRIMESTRAL", .peso = 0.25, .total = 1, .calificadas = 0, .suma = 0 },
    };
    try std.testing.expectApproxEqAbs(@as(f64, 6.6833), realGradeAxes(&axes).?, 0.001);
    const only_exam = [_]Axis{.{ .nombre = "EXAMEN TRIMESTRAL", .peso = 0.25, .total = 1, .calificadas = 0, .suma = 0 }};
    try std.testing.expect(realGradeAxes(&only_exam) == null);
}

test "el examen sin actividades cuenta como pendiente" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var subs = [_]DetailSubmenu{
        .{ .nombre = "TAREAS", .total = 3, .calificadas = 2, .suma = 18, .subtotal = 9, .actividades = &.{} },
        .{ .nombre = "EXAMEN TRIMESTRAL", .total = 0, .calificadas = 0, .suma = 0, .subtotal = null, .actividades = &.{} },
    };
    const axes = try buildAxes(a, &subs);
    try std.testing.expectEqual(@as(usize, 2), axes.len);
    try std.testing.expectEqual(@as(usize, 1), axes[1].total);
    try std.testing.expectApproxEqAbs(@as(f64, 9.0), realGradeAxes(axes).?, 1e-9);
}

test "el código del esquema casa con el de las estadísticas" {
    try std.testing.expect(gradeCodeMatches("26EGB10A-MATE", "26EGB10A-MATE - Matematica"));
    try std.testing.expect(gradeCodeMatches("26EGB10A-MATE", "26EGB10A-MATE"));
    try std.testing.expect(!gradeCodeMatches("26EGB10A-MATE", "26EGB10A-MATEMATICAS"));
    try std.testing.expect(!gradeCodeMatches("26EGB10A-MATE", "26EGB10A-BIOL - Biología"));
}

test "lee el desglose por submenú del árbol de Mikareno" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const raw =
        \\{"estructura_calificaciones":{"scoreSchemes":[{"course":{"courseCode":"26EGB10A-MATE","code":"MATE"},"root":{"children":[{"partialId":"2026:T3","supplyId":"L::COURSE::T3::Promedios","children":[{"partialId":"2026:T3:1","supplyId":"L::COURSE::T3:1::1 Trimestre","children":[{"partialId":"2026:T3:1","supplyId":"L::COURSE::T3:1::TAREAS","children":[{"partialId":"2026:T3:1","supplyId":"L::COURSE::T3:1::TAREAS","scoreValues":[{"value":"10.00"},{"value":""},{"value":"8.00"}]}]},{"partialId":"2026:T3:1","supplyId":"L::COURSE::T3:1::EXAMEN TRIMESTRAL","children":[]}]},{"partialId":"2026:T3:2","supplyId":"L::COURSE::T3:2::2 Trimestre","children":[]}]}]}}]}}
    ;
    const full = try json.parse(a, raw);
    const got = parseSubjectAxes(a, full);
    try std.testing.expectEqual(@as(usize, 1), got.len);
    try std.testing.expectEqualStrings("26EGB10A-MATE", got[0].codigo);
    try std.testing.expectEqualStrings("2026:T3:1", got[0].parcial);
    try std.testing.expectEqual(@as(usize, 2), got[0].axes.len);
    try std.testing.expectEqualStrings("TAREAS", got[0].axes[0].nombre);
    try std.testing.expectEqual(@as(usize, 3), got[0].axes[0].total);
    try std.testing.expectEqual(@as(usize, 2), got[0].axes[0].calificadas);
    try std.testing.expectApproxEqAbs(@as(f64, 18.0), got[0].axes[0].suma, 1e-9);
    try std.testing.expectEqualStrings("EXAMEN TRIMESTRAL", got[0].axes[1].nombre);
    try std.testing.expectApproxEqAbs(@as(f64, 9.0), realGradeAxes(got[0].axes).?, 1e-9);
}

test "la tarjeta muestra el eje más barato y los dos modos" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const axes = [_]Axis{
        .{ .nombre = "TAREAS", .peso = weight_tareas, .total = 9, .calificadas = 4, .suma = 34 },
        .{ .nombre = "TRABAJOS", .peso = weight_trabajos, .total = 3, .calificadas = 2, .suma = 16 },
        .{ .nombre = "EVALUACIONES", .peso = weight_evaluaciones, .total = 2, .calificadas = 2, .suma = 16.5 },
        .{ .nombre = "EXAMEN TRIMESTRAL", .peso = weight_examen, .total = 1, .calificadas = 0, .suma = 0 },
    };
    const g = SubjectGrade{
        .materia = "Matematica",
        .codigo = "26EGB10A-MATE",
        .parcial = "2026:T3:1",
        .nota = 6.13,
        .tiene_nota = true,
        .maximo = 10,
        .cuantitativa = true,
        .bloqueada = false,
        .axes = &axes,
        .real = realGradeAxes(&axes),
    };
    const html = try gradeCard(a, g);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-oficial=\"6.13\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-modo-real=\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Tareas ≥0.4 · Trabajos ≥2.0 · Evaluaciones ✓ · Examen ≥5.7") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "solo calificado:") != null);
}

test "el plan lista los cuatro ejes con su mínimo" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var subs = [_]DetailSubmenu{
        .{ .nombre = "TAREAS", .total = 9, .calificadas = 4, .suma = 34, .subtotal = 8.5, .actividades = &.{} },
        .{ .nombre = "TRABAJOS", .total = 3, .calificadas = 2, .suma = 16, .subtotal = 8.0, .actividades = &.{} },
        .{ .nombre = "EVALUACIONES", .total = 2, .calificadas = 2, .suma = 16.5, .subtotal = 8.25, .actividades = &.{} },
        .{ .nombre = "EXAMEN TRIMESTRAL", .total = 0, .calificadas = 0, .suma = 0, .subtotal = null, .actividades = &.{} },
    };
    const d = SubjectDetail{ .tiene_nota = true, .nota = 6.13, .maximo = 10, .submenus = &subs, .real = 7.4 };
    const html = try planHtml(a, d);
    try std.testing.expect(std.mem.indexOf(u8, html, "Mínimo en cada eje para llegar a 8") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "TAREAS <em>20%</em>") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "5 de 9 por calificar") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "1 por calificar · sin nota") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Con solo lo calificado tienes 7.40") != null);
}

test "la vista de notas trae el interruptor de perspectiva" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const axes = [_]Axis{
        .{ .nombre = "TAREAS", .peso = weight_tareas, .total = 4, .calificadas = 2, .suma = 12 },
    };
    var grades = [_]SubjectGrade{.{
        .materia = "Matematica",
        .codigo = "26EGB10A-MATE",
        .parcial = "2026:T3:1",
        .nota = 4,
        .tiene_nota = true,
        .maximo = 10,
        .cuantitativa = true,
        .bloqueada = false,
        .axes = &axes,
        .real = realGradeAxes(&axes),
    }};
    const d = Datos{ .connected = true, .grades = &grades };
    const html = try gradesPageHtml(a, d, "");
    try std.testing.expect(std.mem.indexOf(u8, html, "data-modo-toggle") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Solo lo calificado") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "data-modo-note") != null);
}

test "la copia de Mikareno se guarda, se lee y marca la antigüedad" {
    var db = try sqlite.Db.open(std.testing.allocator, ":memory:");
    defer db.close();
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    const payload = try json.parse(a, "{\"promedios\":{\"stats\":[{\"materia\":\"Matemática\",\"nota\":9.5,\"trimestre\":3}]}}");
    saveCache(a, &db, payload, 1000);

    const cached = loadCache(a, &db) orelse return error.TestUnexpectedResult;
    try std.testing.expectEqual(@as(i64, 1000), cached.fetched_at);

    var d = Datos{};
    try std.testing.expect(applyCached(a, &d, cached, 1000 + 3600));
    try std.testing.expect(d.from_cache);
    try std.testing.expectEqual(@as(?i64, 3600), d.cache_age_seconds);
    try std.testing.expectEqual(@as(usize, 1), d.notas.len);
}

test "la copia caduca a las 5 horas" {
    try std.testing.expectEqual(@as(i64, 18000), CACHE_TTL_SECONDS);
}

test "cacheAgeText formatea minutos, horas y dias" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    try std.testing.expectEqualStrings("hace un momento", try cacheAgeText(a, 30));
    try std.testing.expectEqualStrings("hace 15 min", try cacheAgeText(a, 900));
    try std.testing.expectEqualStrings("hace 2 h", try cacheAgeText(a, 7200));
    try std.testing.expectEqualStrings("hace 3 días", try cacheAgeText(a, 3 * 86400));
}

test "sin conexión devuelve la última copia guardada" {
    var db = try sqlite.Db.open(std.testing.allocator, ":memory:");
    defer db.close();
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();

    const payload = try json.parse(a, "{\"promedios\":{\"stats\":[{\"materia\":\"Historia\",\"nota\":8.0,\"trimestre\":3}]}}");
    const now = std.time.timestamp();
    saveCache(a, &db, payload, now - CACHE_TTL_SECONDS - 10);

    const d = try fetch(a, &db, "http://127.0.0.1:1", 300);
    try std.testing.expect(d.from_cache);
    try std.testing.expect(d.offline);
    try std.testing.expect(d.connected);
    try std.testing.expectEqual(@as(usize, 1), d.notas.len);
    try std.testing.expect((d.cache_age_seconds orelse 0) >= CACHE_TTL_SECONDS);
}
