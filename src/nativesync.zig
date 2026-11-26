//! Sync JSON para el cliente de escritorio nativo (GTK, sin web).
//!
//! El binario nativo no pinta HTML: pide este JSON y dibuja widgets GTK. Aquí
//! se serializan los mismos datos reales que ya usa la web (Mikareno, notas,
//! bandeja de tareas), sin inventar nada: si una fuente está vacía, viaja
//! vacía y el cliente muestra su estado honesto.

const std = @import("std");
const json = @import("jsonutil.zig");
const panel = @import("panel.zig");
const tasks = @import("tasks.zig");
const classroom = @import("classroom.zig");

fn strArray(allocator: std.mem.Allocator, items: []const []const u8) !json.Value {
    var arr = std.json.Array.init(allocator);
    for (items) |s| try arr.append(json.string(s));
    return .{ .array = arr };
}

fn eventosJson(allocator: std.mem.Allocator, eventos: []const panel.Evento) !json.Value {
    var arr = std.json.Array.init(allocator);
    for (eventos) |e| {
        try arr.append(try json.object(allocator, &.{
            .{ .key = "fecha", .value = json.string(e.fecha) },
            .{ .key = "titulo", .value = json.string(e.titulo) },
            .{ .key = "tipo", .value = json.string(e.tipo) },
        }));
    }
    return .{ .array = arr };
}

fn tareasJson(allocator: std.mem.Allocator, tareas: []const panel.Tarea) !json.Value {
    var arr = std.json.Array.init(allocator);
    for (tareas) |t| {
        try arr.append(try json.object(allocator, &.{
            .{ .key = "titulo", .value = json.string(t.titulo) },
            .{ .key = "vencimiento", .value = json.string(t.vencimiento) },
            .{ .key = "materia", .value = json.string(t.materia) },
        }));
    }
    return .{ .array = arr };
}

fn notasJson(allocator: std.mem.Allocator, notas: []const panel.Nota) !json.Value {
    var arr = std.json.Array.init(allocator);
    for (notas) |nota| {
        try arr.append(try json.object(allocator, &.{
            .{ .key = "materia", .value = json.string(nota.materia) },
            .{ .key = "nota", .value = json.float(nota.nota) },
            .{ .key = "trimestre", .value = json.integer(nota.trimestre) },
        }));
    }
    return .{ .array = arr };
}

fn gradesJson(allocator: std.mem.Allocator, grades: []const panel.SubjectGrade) !json.Value {
    var arr = std.json.Array.init(allocator);
    for (grades) |s| {
        var axes = std.json.Array.init(allocator);
        for (s.axes) |ax| {
            try axes.append(try json.object(allocator, &.{
                .{ .key = "nombre", .value = json.string(ax.nombre) },
                .{ .key = "peso", .value = json.float(ax.peso) },
                .{ .key = "total", .value = json.integer(ax.total) },
                .{ .key = "calificadas", .value = json.integer(ax.calificadas) },
                .{ .key = "suma", .value = json.float(ax.suma) },
            }));
        }
        const real: json.Value = if (s.real) |r| json.float(r) else .{ .bool = false };
        try arr.append(try json.object(allocator, &.{
            .{ .key = "materia", .value = json.string(s.materia) },
            .{ .key = "codigo", .value = json.string(s.codigo) },
            .{ .key = "parcial", .value = json.string(s.parcial) },
            .{ .key = "nota", .value = json.float(s.nota) },
            .{ .key = "tiene_nota", .value = .{ .bool = s.tiene_nota } },
            .{ .key = "maximo", .value = json.float(s.maximo) },
            .{ .key = "cuantitativa", .value = .{ .bool = s.cuantitativa } },
            .{ .key = "bloqueada", .value = .{ .bool = s.bloqueada } },
            .{ .key = "real", .value = real },
            .{ .key = "axes", .value = .{ .array = axes } },
        }));
    }
    return .{ .array = arr };
}

fn horarioJson(allocator: std.mem.Allocator, horario: []const panel.Clase) !json.Value {
    var arr = std.json.Array.init(allocator);
    for (horario) |clase| {
        try arr.append(try json.object(allocator, &.{
            .{ .key = "dia", .value = json.string(clase.dia) },
            .{ .key = "etiqueta", .value = json.string(clase.etiqueta) },
        }));
    }
    return .{ .array = arr };
}

fn inboxJson(allocator: std.mem.Allocator, items: []const tasks.Item) !json.Value {
    var arr = std.json.Array.init(allocator);
    for (items) |it| {
        try arr.append(try json.object(allocator, &.{
            .{ .key = "source", .value = json.string(tasks.sourceLabel(it.source)) },
            .{ .key = "title", .value = json.string(it.title) },
            .{ .key = "detail", .value = json.string(it.detail) },
            .{ .key = "due", .value = json.string(it.due) },
            .{ .key = "course", .value = json.string(it.course) },
            .{ .key = "link", .value = json.string(it.link) },
        }));
    }
    return .{ .array = arr };
}

fn noteRowsJson(allocator: std.mem.Allocator, rows: []const tasks.NoteRow) !json.Value {
    var arr = std.json.Array.init(allocator);
    for (rows) |r| {
        try arr.append(try json.object(allocator, &.{
            .{ .key = "slug", .value = json.string(r.slug) },
            .{ .key = "title", .value = json.string(r.title) },
            .{ .key = "tags", .value = json.string(r.tags) },
        }));
    }
    return .{ .array = arr };
}

/// Construye el documento JSON completo que consume el cliente nativo.
/// `classroom_items` puede venir vacío (sin token o caché): viaja como lista
/// vacía y el cliente lo trata como "sin entregas de Classroom".
pub fn build(
    allocator: std.mem.Allocator,
    datos: panel.Datos,
    classroom_items: []const classroom.Upcoming,
    note_rows: []const tasks.NoteRow,
    inbox: []const tasks.Item,
    notes_count: ?i64,
    conversations_count: ?i64,
) !json.Value {
    const counts = tasks.count(inbox);
    const cache_age: json.Value = if (datos.cache_age_seconds) |s| json.integer(s) else .{ .bool = false };
    const nc: json.Value = if (notes_count) |n| json.integer(n) else .{ .bool = false };
    const cc: json.Value = if (conversations_count) |n| json.integer(n) else .{ .bool = false };
    return json.object(allocator, &.{
        .{ .key = "connected", .value = .{ .bool = datos.connected } },
        .{ .key = "offline", .value = .{ .bool = datos.offline } },
        .{ .key = "from_cache", .value = .{ .bool = datos.from_cache } },
        .{ .key = "cache_age_seconds", .value = cache_age },
        .{ .key = "partial_label", .value = json.string(datos.partial_label) },
        .{ .key = "notes_count", .value = nc },
        .{ .key = "conversations_count", .value = cc },
        .{ .key = "tasks_today", .value = json.integer(datos.tasks_today) },
        .{ .key = "tasks_week", .value = json.integer(datos.tasks_week) },
        .{ .key = "eventos", .value = try eventosJson(allocator, datos.eventos) },
        .{ .key = "tareas", .value = try tareasJson(allocator, datos.tareas) },
        .{ .key = "notas", .value = try notasJson(allocator, datos.notas) },
        .{ .key = "grades", .value = try gradesJson(allocator, datos.grades) },
        .{ .key = "horario", .value = try horarioJson(allocator, datos.horario) },
        .{ .key = "inbox", .value = try inboxJson(allocator, inbox) },
        .{ .key = "inbox_counts", .value = try json.object(allocator, &.{
            .{ .key = "total", .value = json.integer(counts.total) },
            .{ .key = "mikareno", .value = json.integer(counts.mikareno) },
            .{ .key = "classroom", .value = json.integer(counts.classroom) },
            .{ .key = "whatsapp", .value = json.integer(counts.whatsapp) },
            .{ .key = "note", .value = json.integer(counts.note) },
        }) },
        .{ .key = "classroom", .value = try classItemsJson(allocator, classroom_items) },
        .{ .key = "note_rows", .value = try noteRowsJson(allocator, note_rows) },
    });
}

fn classItemsJson(allocator: std.mem.Allocator, items: []const classroom.Upcoming) !json.Value {
    var arr = std.json.Array.init(allocator);
    for (items) |u| {
        try arr.append(try json.object(allocator, &.{
            .{ .key = "course", .value = json.string(u.course) },
            .{ .key = "title", .value = json.string(u.title) },
            .{ .key = "due", .value = json.string(u.due) },
        }));
    }
    return .{ .array = arr };
}

test "build serializa datos reales sin inventar" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var tareas_in = [_]panel.Tarea{.{ .titulo = "Taller", .vencimiento = "2026-12-01", .materia = "Física" }};
    var grades_in = [_]panel.SubjectGrade{.{ .materia = "Física", .codigo = "FIS", .parcial = "P1", .nota = 6.5, .tiene_nota = true, .maximo = 10, .cuantitativa = true, .bloqueada = false }};
    const datos = panel.Datos{
        .connected = true,
        .partial_label = "2026 · Trimestre 3",
        .tareas = tareas_in[0..],
        .grades = grades_in[0..],
    };
    const doc = try build(a, datos, &.{}, &.{}, &.{}, 5, 2);
    try std.testing.expectEqualStrings("2026 · Trimestre 3", json.getString(doc, "partial_label").?);
    try std.testing.expectEqual(@as(i64, 5), json.getInt(doc, "notes_count").?);
    const grades = json.get(doc, "grades").?;
    try std.testing.expectEqual(@as(usize, 1), grades.array.items.len);
    const tareas = json.get(doc, "tareas").?;
    try std.testing.expectEqualStrings("Taller", json.getString(tareas.array.items[0], "titulo").?);
}

test "build sin datos deja listas vacías y nulls honestos" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const doc = try build(arena.allocator(), panel.Datos{}, &.{}, &.{}, &.{}, null, null);
    try std.testing.expectEqual(@as(usize, 0), json.get(doc, "eventos").?.array.items.len);
    try std.testing.expect(json.get(doc, "notes_count").? == .bool);
    const counts = json.get(doc, "inbox_counts").?;
    try std.testing.expectEqual(@as(i64, 0), json.getInt(counts, "total").?);
}
