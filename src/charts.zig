//! Gráficos de las vistas académicas.
//!
//! Todo se dibuja con SVG en línea y porcentajes: no se añade ninguna
//! librería de charts al repo y no hace falta JavaScript. Cada función es
//! pura y devuelve HTML con clases `nx-chart-*`.
//!
//! Los datos los manda el llamador; aquí no se inventa ni se redondea nada:
//! si un valor no existe, la barra sale vacía y se dice.

const std = @import("std");
const security = @import("security.zig");

fn esc(allocator: std.mem.Allocator, s: []const u8) ![]const u8 {
    return security.htmlEscape(allocator, s);
}

/// Cita una paleta fija para que dos gráficos de la misma página no
/// parezcan de apps distintas.
pub const Palette = struct {
    pub const good = "#2dd4bf";
    pub const warn = "#fbbf24";
    pub const bad = "#f87171";
    pub const accent = "#a78bfa";
    pub const info = "#67e8f9";
    pub const muted = "#7f8ea3";
};

/// Color por nota, misma escala que el resto de la app.
pub fn gradeColor(nota: f64) []const u8 {
    if (nota >= 7) return Palette.good;
    if (nota >= 5) return Palette.warn;
    return Palette.bad;
}

/// Porcentaje acotado a 0–100 para que un dato raro no rompa el dibujo.
fn clampPct(pct: f64) f64 {
    if (std.math.isNan(pct)) return 0;
    if (pct < 0) return 0;
    if (pct > 100) return 100;
    return pct;
}

/// Anillo de progreso. `pct` es 0–100; el texto va en el centro.
pub fn donut(
    allocator: std.mem.Allocator,
    pct: f64,
    color: []const u8,
    valor: []const u8,
    etiqueta: []const u8,
    pie: []const u8,
) ![]const u8 {
    const p = clampPct(pct);
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-chart-donut\">" ++
            "<svg viewBox=\"0 0 120 120\" role=\"img\" aria-label=\"{s}: {d:.0}%\">" ++
            "<circle class=\"nx-chart-donut-track\" cx=\"60\" cy=\"60\" r=\"48\"></circle>" ++
            "<circle class=\"nx-chart-donut-value\" cx=\"60\" cy=\"60\" r=\"48\" pathLength=\"100\" " ++
            "stroke=\"{s}\" stroke-dasharray=\"{d:.2} 100\"></circle></svg>" ++
            "<div class=\"nx-chart-donut-center\"><strong>{s}</strong><span>{s}</span></div>" ++
            "<p class=\"nx-chart-donut-foot\">{s}</p></div>",
        .{ etiqueta, p, color, p, valor, etiqueta, pie },
    );
}

/// Una fila del gráfico de barras: etiqueta, valor y texto a la derecha.
pub const Bar = struct {
    etiqueta: []const u8,
    valor: f64,
    texto: []const u8,
    color: []const u8,
    /// Segundas serie opcional (por ejemplo, la nota oficial junto a la real).
    secundario: ?f64 = null,
    color_secundario: []const u8 = Palette.muted,
};

/// Barras horizontales comparables sobre la misma escala de 0–100.
pub fn barras(allocator: std.mem.Allocator, items: []const Bar, escala_max: f64, nota_al_pie: []const u8) ![]const u8 {
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<div class=\"nx-chart-bars\"><ul>");
    for (items) |item| {
        const max = if (escala_max <= 0) 100.0 else escala_max;
        const pct = clampPct(item.valor / max * 100.0);
        const ancho_sec = if (item.secundario) |s| clampPct(s / max * 100.0) else null;
        try buf.appendSlice("<li class=\"nx-chart-bar-row\">");
        try buf.appendSlice(try std.fmt.allocPrint(allocator, "<span class=\"nx-chart-bar-label\">{s}</span>", .{try esc(allocator, item.etiqueta)}));
        try buf.appendSlice("<span class=\"nx-chart-bar-track\">");
        if (ancho_sec) |w| {
            try buf.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<i class=\"nx-chart-bar-ghost\" style=\"width:{d:.2}%;background:{s}\"></i>",
                .{ w, item.color_secundario },
            ));
        }
        try buf.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<i class=\"nx-chart-bar-fill\" style=\"width:{d:.2}%;background:{s}\"></i></span>",
            .{ pct, item.color },
        ));
        try buf.appendSlice(try std.fmt.allocPrint(allocator, "<span class=\"nx-chart-bar-value\">{s}</span></li>", .{try esc(allocator, item.texto)}));
    }
    try buf.appendSlice("</ul>");
    if (nota_al_pie.len > 0) {
        try buf.appendSlice(try std.fmt.allocPrint(allocator, "<p class=\"nx-chart-note\">{s}</p>", .{try esc(allocator, nota_al_pie)}));
    }
    try buf.appendSlice("</div>");
    return buf.items;
}

/// Tira apilada de proporciones (aprobadas / riesgo / reprobadas).
pub const Segmento = struct {
    etiqueta: []const u8,
    cantidad: usize,
    color: []const u8,
};

pub fn apilada(allocator: std.mem.Allocator, segmentos: []const Segmento, total: usize) ![]const u8 {
    var buf = std.ArrayList(u8).init(allocator);
    try buf.appendSlice("<div class=\"nx-chart-stack\"><div class=\"nx-chart-stack-bar\" role=\"img\" aria-label=\"Distribución de notas\">");
    var any = false;
    for (segmentos) |s| {
        if (s.cantidad == 0) continue;
        any = true;
        const pct = if (total == 0) 0 else @as(f64, @floatFromInt(s.cantidad)) / @as(f64, @floatFromInt(total)) * 100.0;
        try buf.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<i style=\"width:{d:.2}%;background:{s}\" title=\"{s}: {d}\"></i>",
            .{ clampPct(pct), s.color, try esc(allocator, s.etiqueta), s.cantidad },
        ));
    }
    if (!any) try buf.appendSlice("<i style=\"width:100%;background:#2a3852\"></i>");
    try buf.appendSlice("</div><ul class=\"nx-chart-legend\">");
    for (segmentos) |s| {
        const pct = if (total == 0) 0 else @as(f64, @floatFromInt(s.cantidad)) / @as(f64, @floatFromInt(total)) * 100.0;
        try buf.appendSlice(try std.fmt.allocPrint(
            allocator,
            "<li><i style=\"background:{s}\"></i><span>{s}</span><strong>{d}</strong><small>{d:.0}%</small></li>",
            .{ s.color, try esc(allocator, s.etiqueta), s.cantidad, pct },
        ));
    }
    try buf.appendSlice("</ul></div>");
    return buf.items;
}

/// Barra de progreso simple con etiqueta y porcentaje (tiempo del parcial).
pub fn progreso(allocator: std.mem.Allocator, etiqueta: []const u8, pct: f64, detalle: []const u8, color: []const u8) ![]const u8 {
    const p = clampPct(pct);
    return std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-chart-progress\"><div class=\"nx-chart-progress-head\"><span>{s}</span><strong>{d:.0}%</strong></div>" ++
            "<div class=\"nx-chart-progress-track\"><i style=\"width:{d:.2}%;background:{s}\"></i></div>" ++
            "<small>{s}</small></div>",
        .{ try esc(allocator, etiqueta), p, p, color, try esc(allocator, detalle) },
    );
}

// ─────────────────────────────── Tests ───────────────────────────────

test "el anillo acota el porcentaje y dibuja el arco" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const html = try donut(a, 62.4, Palette.good, "6.24", "Promedio", "sobre 10");
    try std.testing.expect(std.mem.indexOf(u8, html, "stroke-dasharray=\"62.40 100\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "6.24") != null);
    const loco = try donut(a, 480, Palette.bad, "x", "y", "z");
    try std.testing.expect(std.mem.indexOf(u8, loco, "stroke-dasharray=\"100.00 100\"") != null);
    const neg = try donut(a, -3, Palette.bad, "x", "y", "z");
    try std.testing.expect(std.mem.indexOf(u8, neg, "stroke-dasharray=\"0.00 100\"") != null);
}

test "las barras escalan contra el máximo indicado" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const items = [_]Bar{
        .{ .etiqueta = "Matemática", .valor = 6.13, .texto = "6.13", .color = Palette.warn },
        .{ .etiqueta = "Biología", .valor = 10, .texto = "10.00", .color = Palette.good, .secundario = 5.94 },
    };
    const html = try barras(a, &items, 10, "Escala 0–10");
    try std.testing.expect(std.mem.indexOf(u8, html, "width:61.30%") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "nx-chart-bar-ghost") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "Escala 0–10") != null);
}

test "la tira apilada reparte los porcentajes" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const seg = [_]Segmento{
        .{ .etiqueta = "Aprobadas", .cantidad = 3, .color = Palette.good },
        .{ .etiqueta = "En riesgo", .cantidad = 1, .color = Palette.warn },
    };
    const html = try apilada(a, &seg, 4);
    try std.testing.expect(std.mem.indexOf(u8, html, "width:75.00%") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "width:25.00%") != null);
}

test "el progreso no pasa de 100" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const html = try progreso(a, "Parcial", 140, "68 de 68 días", Palette.accent);
    try std.testing.expect(std.mem.indexOf(u8, html, "width:100.00%") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "100%") != null);
}
