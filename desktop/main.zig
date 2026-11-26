// NOVA · app de escritorio nativa (GTK3, sin motor web).
//
// No embebe WebKit ni depende del navegador del sistema: dibuja la interfaz
// con widgets GTK reales y pide los datos al servidor local por su API JSON
// (`/api/native/sync`). Es híbrida: si el servidor no responde, arranca el
// binario `study-platform` y reintenta. La lógica vive en Zig; aquí sólo hay
// presentación y una petición HTTP.
//
// Uso:
//   zig build desktop
//   ./zig-out/bin/nova-desktop
// Variables: NOVA_SERVER_URL (def. http://127.0.0.1:3100),
//            NOVA_SERVER_BIN (def. zig-out/bin/study-platform).

const std = @import("std");
const json = @import("jsonutil");

const c = @cImport({
    @cInclude("gtk/gtk.h");
});

const allocator = std.heap.c_allocator;

var g_window: ?*c.GtkWidget = null;
var g_stack: ?*c.GtkWidget = null;
var g_status: ?*c.GtkWidget = null;
var g_server_child: ?std.process.Child = null;

// ─────────────────────────── HTTP ───────────────────────────

/// GET de una URL local; devuelve el cuerpo. Falla si el servidor no responde.
fn httpGet(url: []const u8, timeout_ms: u64) ![]u8 {
    _ = timeout_ms;
    var client = std.http.Client{ .allocator = allocator };
    defer client.deinit();
    var body = std.ArrayList(u8).init(allocator);
    const resp = client.fetch(.{
        .location = .{ .url = url },
        .method = .GET,
        .response_storage = .{ .dynamic = &body },
        .max_append_size = 8 * 1024 * 1024,
    }) catch return error.Unreachable;
    if (@intFromEnum(resp.status) < 200 or @intFromEnum(resp.status) >= 300) return error.BadStatus;
    return body.items;
}

/// Devuelve el JSON de sync ya parseado, arrancando el servidor si hace falta.
fn fetchSync() ?json.Value {
    const base = std.posix.getenv("NOVA_SERVER_URL") orelse "http://127.0.0.1:3100";
    const sync_url = std.fmt.allocPrint(allocator, "{s}/api/native/sync", .{base}) catch return null;

    var attempt: u8 = 0;
    while (attempt < 40) : (attempt += 1) {
        if (httpGet(sync_url, 8000)) |body| {
            return json.parse(allocator, body) catch null;
        } else |_| {
            // Si aún no está el servidor, se arranca una vez y se reintenta.
            if (attempt == 0) maybeSpawnServer(base);
            std.Thread.sleep(150 * std.time.ns_per_ms);
        }
    }
    return null;
}

/// Arranca el binario del servidor en segundo plano si no hay uno ya vivo.
fn maybeSpawnServer(base: []const u8) void {
    if (g_server_child != null) return;
    const bin = std.posix.getenv("NOVA_SERVER_BIN") orelse "zig-out/bin/study-platform";
    // Puerto del servidor a partir de la URL base (host:puerto).
    const addr = serverAddr(base);
    var child = std.process.Child.init(&.{ bin, "--addr", addr }, allocator);
    child.cwd = std.posix.getenv("NOVA_SERVER_CWD");
    child.spawn() catch return;
    // No se espera: el servidor queda corriendo tras cerrar la app.
    g_server_child = child;
}

/// Extrae `host:port` de `http://host:port`.
fn serverAddr(base: []const u8) []const u8 {
    var s = base;
    if (std.mem.indexOf(u8, s, "://")) |i| s = s[i + 3 ..];
    if (std.mem.indexOfScalar(u8, s, '/')) |i| s = s[0..i];
    return s;
}

// ─────────────────────────── GTK helpers ───────────────────────────

/// `g_signal_connect` es un macro que cImport no siempre traduce; se usa la
/// función real `g_signal_connect_data` con un envoltorio tipado.
fn sigConnect(instance: anytype, signal: [*:0]const u8, handler: anytype, data: ?*anyopaque) void {
    _ = c.g_signal_connect_data(@ptrCast(instance), signal, @ptrCast(&handler), data, null, 0);
}

fn label(text: []const u8, big: bool) *c.GtkWidget {
    const z = allocator.dupeZ(u8, text) catch return c.gtk_label_new(null);
    const w = c.gtk_label_new(z.ptr);
    c.gtk_label_set_xalign(@ptrCast(w), 0.0);
    c.gtk_label_set_line_wrap(@ptrCast(w), 1);
    c.gtk_widget_set_margin_start(w, 18);
    c.gtk_widget_set_margin_end(w, 18);
    c.gtk_widget_set_margin_top(w, if (big) 14 else 5);
    c.gtk_widget_set_margin_bottom(w, if (big) 4 else 5);
    if (big) {
        const ctx = c.gtk_widget_get_style_context(w);
        c.gtk_style_context_add_class(ctx, "nova-h1");
    } else {
        // Cada línea es una tarjeta ligera.
        const ctx = c.gtk_widget_get_style_context(w);
        c.gtk_style_context_add_class(ctx, "nova-row");
        c.gtk_widget_set_margin_start(w, 14);
        c.gtk_widget_set_margin_end(w, 14);
        c.gtk_widget_set_margin_top(w, 4);
        c.gtk_widget_set_margin_bottom(w, 4);
    }
    return w;
}

fn heading(text: []const u8) *c.GtkWidget {
    const z = allocator.dupeZ(u8, text) catch return c.gtk_label_new(null);
    const w = c.gtk_label_new(z.ptr);
    c.gtk_label_set_xalign(@ptrCast(w), 0.0);
    c.gtk_widget_set_margin_start(w, 18);
    c.gtk_widget_set_margin_top(w, 16);
    c.gtk_widget_set_margin_bottom(w, 2);
    const ctx = c.gtk_widget_get_style_context(w);
    c.gtk_style_context_add_class(ctx, "nova-h2");
    return w;
}

fn addLine(box: *c.GtkWidget, text: []const u8) void {
    const w = label(text, false);
    c.gtk_box_pack_start(@ptrCast(box), w, 0, 0, 0);
}

fn addHeading(box: *c.GtkWidget, text: []const u8) void {
    const w = heading(text);
    c.gtk_box_pack_start(@ptrCast(box), w, 0, 0, 0);
}

/// Una página nueva: scrolled window con una caja vertical dentro.
fn newPage(name: []const u8) struct { root: *c.GtkWidget, box: *c.GtkWidget } {
    const sw = c.gtk_scrolled_window_new(null, null);
    c.gtk_scrolled_window_set_policy(@ptrCast(sw), c.GTK_POLICY_NEVER, c.GTK_POLICY_AUTOMATIC);
    const vbox = c.gtk_box_new(c.GTK_ORIENTATION_VERTICAL, 2);
    c.gtk_widget_set_margin_start(vbox, 22);
    c.gtk_widget_set_margin_end(vbox, 22);
    c.gtk_widget_set_margin_top(vbox, 10);
    c.gtk_widget_set_margin_bottom(vbox, 16);
    c.gtk_scrolled_window_add_with_viewport(@ptrCast(sw), vbox);
    const zname = allocator.dupeZ(u8, name) catch name;
    c.gtk_stack_add_named(@ptrCast(g_stack), sw, zname.ptr);
    return .{ .root = sw, .box = vbox };
}

fn setStatus(text: []const u8) void {
    if (g_status) |s| {
        const z = allocator.dupeZ(u8, text) catch return;
        c.gtk_label_set_text(@ptrCast(s), z.ptr);
    }
}

// ─────────────────────────── Vistas ───────────────────────────

fn arr(value: json.Value, key: []const u8) []const json.Value {
    const v = json.get(value, key) orelse return &.{};
    return if (v == .array) v.array.items else &.{};
}

fn buildInbox(doc: json.Value) void {
    const p = newPage("tareas");
    addHeading(p.box, "Bandeja unificada");
    const counts = json.get(doc, "inbox_counts");
    if (counts) |cnt| {
        const line = std.fmt.allocPrint(allocator, "Mikareno {d} · Classroom {d} · WhatsApp {d} · Notas {d}", .{ json.getInt(cnt, "mikareno") orelse 0, json.getInt(cnt, "classroom") orelse 0, json.getInt(cnt, "whatsapp") orelse 0, json.getInt(cnt, "note") orelse 0 }) catch "";
        addLine(p.box, line);
    }
    const items = arr(doc, "inbox");
    if (items.len == 0) {
        addLine(p.box, "Sin pendientes. Todo al día.");
        return;
    }
    for (items) |it| {
        const src = json.getString(it, "source") orelse "";
        const title = json.getString(it, "title") orelse "(sin título)";
        const due = json.getString(it, "due") orelse "";
        const line = std.fmt.allocPrint(allocator, "[{s}] {s}{s}{s}", .{ src, title, if (due.len > 0) "  — " else "", due }) catch title;
        addLine(p.box, line);
    }
}

fn buildGrades(doc: json.Value) void {
    const p = newPage("asignaturas");
    addHeading(p.box, "Asignaturas");
    const label_parcial = json.getString(doc, "partial_label") orelse "";
    if (label_parcial.len > 0) addLine(p.box, label_parcial);
    const grades = arr(doc, "grades");
    if (grades.len == 0) {
        addLine(p.box, "Sin asignaturas sincronizadas.");
        return;
    }
    for (grades) |g| {
        const materia = json.getString(g, "materia") orelse "(materia)";
        const tiene = json.getBool(g, "tiene_nota") orelse false;
        var text: []const u8 = undefined;
        if (!tiene) {
            text = std.fmt.allocPrint(allocator, "{s}: sin calificar", .{materia}) catch materia;
        } else if (json.get(g, "real")) |rv| {
            if (rv == .float) {
                text = std.fmt.allocPrint(allocator, "{s}: {d:.2} (calificado) · nota {d:.2}/{d:.0}", .{ materia, rv.float, json.getFloat(g, "nota") orelse 0, json.getFloat(g, "maximo") orelse 0 }) catch materia;
            } else {
                text = std.fmt.allocPrint(allocator, "{s}: nota {d:.2}/{d:.0}", .{ materia, json.getFloat(g, "nota") orelse 0, json.getFloat(g, "maximo") orelse 0 }) catch materia;
            }
        } else {
            text = std.fmt.allocPrint(allocator, "{s}: nota {d:.2}/{d:.0}", .{ materia, json.getFloat(g, "nota") orelse 0, json.getFloat(g, "maximo") orelse 0 }) catch materia;
        }
        addLine(p.box, text);
    }
}

fn buildHorario(doc: json.Value) void {
    const p = newPage("horario");
    addHeading(p.box, "Horario");
    const horario = arr(doc, "horario");
    if (horario.len == 0) {
        addLine(p.box, "Sin horario cargado.");
        return;
    }
    for (horario) |clase| {
        const dia = json.getString(clase, "dia") orelse "";
        const et = json.getString(clase, "etiqueta") orelse "";
        addLine(p.box, std.fmt.allocPrint(allocator, "{s}  {s}", .{ dia, et }) catch et);
    }
}

fn buildEventos(doc: json.Value) void {
    const p = newPage("calendario");
    addHeading(p.box, "Próximos eventos");
    const eventos = arr(doc, "eventos");
    if (eventos.len == 0) {
        addLine(p.box, "Sin eventos próximos.");
        return;
    }
    for (eventos) |e| {
        const fecha = json.getString(e, "fecha") orelse "";
        const titulo = json.getString(e, "titulo") orelse "";
        const tipo = json.getString(e, "tipo") orelse "";
        addLine(p.box, std.fmt.allocPrint(allocator, "{s}  {s}  ({s})", .{ fecha, titulo, tipo }) catch titulo);
    }
}

fn buildClassroom(doc: json.Value) void {
    const p = newPage("classroom");
    addHeading(p.box, "Classroom · entregas");
    const items = arr(doc, "classroom");
    if (items.len == 0) {
        addLine(p.box, "Sin entregas de Classroom (sin token o caché vacía).");
        return;
    }
    for (items) |u| {
        const course = json.getString(u, "course") orelse "";
        const title = json.getString(u, "title") orelse "";
        const due = json.getString(u, "due") orelse "";
        addLine(p.box, std.fmt.allocPrint(allocator, "{s}  {s}  — {s}", .{ course, title, due }) catch title);
    }
}

fn buildNotas(doc: json.Value) void {
    const p = newPage("notas");
    addHeading(p.box, "Notas del vault");
    const rows = arr(doc, "note_rows");
    if (rows.len == 0) {
        addLine(p.box, "Sin notas con etiqueta de tarea.");
        return;
    }
    for (rows) |row| {
        const title = json.getString(row, "title") orelse "";
        const tags = json.getString(row, "tags") orelse "";
        addLine(p.box, std.fmt.allocPrint(allocator, "{s}   #{s}", .{ title, tags }) catch title);
    }
}

fn buildInicio(doc: json.Value) void {
    const p = newPage("inicio");
    addHeading(p.box, "Inicio");
    const connected = json.getBool(doc, "connected") orelse false;
    const offline = json.getBool(doc, "offline") orelse false;
    const partial = json.getString(doc, "partial_label") orelse "";
    const state: []const u8 = if (offline) "Sin conexión · datos guardados" else if (connected) "Conectado a Mikareno" else "Sin datos académicos";
    addLine(p.box, state);
    if (partial.len > 0) addLine(p.box, partial);
    const nc = json.getInt(doc, "notes_count");
    const cc = json.getInt(doc, "conversations_count");
    const tt = json.getInt(doc, "tasks_today") orelse 0;
    const tw = json.getInt(doc, "tasks_week") orelse 0;
    const nc_s = intOrQ(nc);
    const cc_s = intOrQ(cc);
    const line = std.fmt.allocPrint(allocator, "Tareas hoy {d} · semana {d} · notas {s} · conversaciones {s}", .{ tt, tw, nc_s, cc_s }) catch "";
    addLine(p.box, line);
}

fn intOrQ(v: ?i64) []const u8 {
    return if (v) |n| std.fmt.allocPrint(allocator, "{d}", .{n}) catch "?" else "?";
}

// ─────────────────────────── Señales GTK ───────────────────────────

fn onDestroy(_: *c.GtkWidget, _: ?*anyopaque) callconv(.C) void {
    c.gtk_main_quit();
}

fn onRefresh(_: *c.GtkWidget, _: ?*anyopaque) callconv(.C) void {
    setStatus("Actualizando…");
    if (fetchSync()) |doc| {
        buildInicio(doc);
        buildInbox(doc);
        buildGrades(doc);
        buildHorario(doc);
        buildEventos(doc);
        buildClassroom(doc);
        buildNotas(doc);
        c.gtk_widget_show_all(g_window);
        setStatus("Actualizado");
    } else {
        setStatus("No se pudo contactar con el servidor");
    }
}

fn onNavClicked(_: *c.GtkWidget, data: ?*anyopaque) callconv(.C) void {
    if (data == null) return;
    const name = @as([*:0]const u8, @ptrCast(data));
    c.gtk_stack_set_visible_child_name(@ptrCast(g_stack), name);
}

/// Al entrar el ratón en la barra lateral, se asoma; al salir, se esconde.
fn onSidebarEnter(_: *c.GtkWidget, _: ?*anyopaque) callconv(.C) void {
    if (g_sidebar_revealer) |r| c.gtk_revealer_set_reveal_child(@ptrCast(r), 1);
}
fn onSidebarLeave(_: *c.GtkWidget, _: ?*anyopaque) callconv(.C) void {
    if (g_sidebar_revealer) |r| c.gtk_revealer_set_reveal_child(@ptrCast(r), 0);
}

var g_sidebar_revealer: ?*c.GtkWidget = null;

fn navButton(text: []const u8, view: []const u8) *c.GtkWidget {
    const z = allocator.dupeZ(u8, text) catch return c.gtk_button_new();
    const b = c.gtk_button_new_with_label(z.ptr);
    c.gtk_button_set_relief(@ptrCast(b), c.GTK_RELIEF_NONE);
    const zv = allocator.dupeZ(u8, view) catch view;
    sigConnect(b, "clicked", onNavClicked, @ptrCast(@constCast(zv.ptr)));
    return b;
}

// ─────────────────────────── main ───────────────────────────

pub fn main() !void {
    c.gtk_init(null, null);

    g_window = c.gtk_window_new(c.GTK_WINDOW_TOPLEVEL);
    c.gtk_window_set_title(@ptrCast(g_window), "NOVA");
    c.gtk_window_set_default_size(@ptrCast(g_window), 1180, 760);
    c.gtk_window_set_position(@ptrCast(g_window), c.GTK_WIN_POS_CENTER);
    sigConnect(g_window, "destroy", onDestroy, null);

    // Aparición de GTK: tema oscuro simple embebido (sin CSS externo).
    const provider = c.gtk_css_provider_new();
    const css =
        // Fondo aurora degradado y texto base.
        "window { background: linear-gradient(135deg, #241a3a 0%, #15131f 55%, #1c1633 100%); color: #ece9f6; }" ++
        // Tarjeta por línea.
        ".nova-row { background: rgba(42,36,66,0.55); border: 1px solid rgba(185,167,255,0.10);" ++
        "  border-radius: 12px; padding: 8px 14px; font-size: 13.5px; }" ++
        // Títulos.
        ".nova-h1 { font-size: 26px; font-weight: 800; color: #c9b8ff; padding-top: 6px; }" ++
        ".nova-h2 { font-size: 16px; font-weight: 700; color: #8fe3c2; margin-top: 10px; }" ++
        // Barra lateral tipo cristal oscuro.
        "list, #sidebar { background: rgba(18,15,30,0.6); }" ++
        // Botones.
        "button { background: rgba(42,36,66,0.5); color: #ece9f6; border: 1px solid rgba(185,167,255,0.08);" ++
        "  border-radius: 11px; padding: 9px 16px; margin: 3px 8px; transition: all 120ms ease; }" ++
        "button:hover { background: #2c2745; border-color: rgba(185,167,255,0.35); }" ++
        // Barra de estado.
        ".nova-status { color: #9b93b8; font-size: 12px; }";
    const zcss = allocator.dupeZ(u8, css) catch css;
    _ = c.gtk_css_provider_load_from_data(provider, zcss.ptr, -1, null);
    c.gtk_style_context_add_provider_for_screen(c.gtk_widget_get_screen(g_window), @ptrCast(provider), c.GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);

    // Layout: revealer (barra lateral) + separador + contenido.
    const root = c.gtk_box_new(c.GTK_ORIENTATION_HORIZONTAL, 0);
    c.gtk_container_add(@ptrCast(g_window), root);

    // Zona que dispara el asomo de la barra (franja estrecha a la izquierda).
    const edge = c.gtk_event_box_new();
    c.gtk_widget_set_size_request(edge, 6, -1);
    c.gtk_box_pack_start(@ptrCast(root), edge, 0, 0, 0);
    c.gtk_widget_add_events(edge, c.GDK_ENTER_NOTIFY_MASK | c.GDK_LEAVE_NOTIFY_MASK);
    sigConnect(edge, "enter-notify-event", onSidebarEnter, null);

    const revealer = c.gtk_revealer_new();
    g_sidebar_revealer = revealer;
    c.gtk_revealer_set_transition_type(@ptrCast(revealer), c.GTK_REVEALER_TRANSITION_TYPE_SLIDE_RIGHT);
    c.gtk_revealer_set_transition_duration(@ptrCast(revealer), 180);
    c.gtk_box_pack_start(@ptrCast(root), revealer, 0, 0, 0);

    const sidebar = c.gtk_box_new(c.GTK_ORIENTATION_VERTICAL, 2);
    c.gtk_widget_set_name(sidebar, "sidebar");
    c.gtk_widget_set_size_request(sidebar, 200, -1);
    c.gtk_container_add(@ptrCast(revealer), sidebar);
    c.gtk_widget_add_events(sidebar, c.GDK_LEAVE_NOTIFY_MASK);
    sigConnect(sidebar, "leave-notify-event", onSidebarLeave, null);

    const brand = label("NOVA", true);
    c.gtk_box_pack_start(@ptrCast(sidebar), brand, 0, 0, 0);
    c.gtk_box_pack_start(@ptrCast(sidebar), navButton("Inicio", "inicio"), 0, 0, 0);
    c.gtk_box_pack_start(@ptrCast(sidebar), navButton("Tareas", "tareas"), 0, 0, 0);
    c.gtk_box_pack_start(@ptrCast(sidebar), navButton("Asignaturas", "asignaturas"), 0, 0, 0);
    c.gtk_box_pack_start(@ptrCast(sidebar), navButton("Horario", "horario"), 0, 0, 0);
    c.gtk_box_pack_start(@ptrCast(sidebar), navButton("Calendario", "calendario"), 0, 0, 0);
    c.gtk_box_pack_start(@ptrCast(sidebar), navButton("Classroom", "classroom"), 0, 0, 0);
    c.gtk_box_pack_start(@ptrCast(sidebar), navButton("Notas", "notas"), 0, 0, 0);
    const refresh = c.gtk_button_new_with_label("Actualizar");
    sigConnect(refresh, "clicked", onRefresh, null);
    c.gtk_box_pack_end(@ptrCast(sidebar), refresh, 0, 0, 8);

    // Contenido: stack + barra de estado.
    const content = c.gtk_box_new(c.GTK_ORIENTATION_VERTICAL, 0);
    c.gtk_box_pack_start(@ptrCast(root), content, 1, 1, 0);

    g_stack = c.gtk_stack_new();
    c.gtk_box_pack_start(@ptrCast(content), g_stack, 1, 1, 0);

    g_status = c.gtk_label_new(null);
    c.gtk_widget_set_name(g_status, "nova-status");
    const sctx = c.gtk_widget_get_style_context(g_status);
    c.gtk_style_context_add_class(sctx, "nova-status");
    c.gtk_widget_set_margin_start(g_status, 12);
    c.gtk_widget_set_margin_top(g_status, 4);
    c.gtk_widget_set_margin_bottom(g_status, 4);
    c.gtk_widget_set_opacity(g_status, 0.7);
    c.gtk_box_pack_end(@ptrCast(content), g_status, 0, 0, 0);

    // Primera carga de datos (arranca el servidor si hace falta).
    setStatus("Cargando datos…");
    if (fetchSync()) |doc| {
        buildInicio(doc);
        buildInbox(doc);
        buildGrades(doc);
        buildHorario(doc);
        buildEventos(doc);
        buildClassroom(doc);
        buildNotas(doc);
        setStatus("Listo");
    } else {
        const p = newPage("inicio");
        addHeading(p.box, "Sin conexión");
        addLine(p.box, "No se pudo contactar con el servidor local ni arrancarlo. Revisa NOVA_SERVER_BIN.");
        setStatus("Sin datos");
    }

    c.gtk_widget_show_all(g_window);
    c.gtk_revealer_set_reveal_child(@ptrCast(revealer), 0);
    c.gtk_main();
}
