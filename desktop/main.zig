// NOVA · lanzador de escritorio (HTML5 exacto).
//
// Abre la misma interfaz server-rendered del backend Zig dentro de una ventana
// nativa GTK3 + WebKit2GTK. No hay un segundo backend ni código Rust/C en el
// repo: sólo se enlazan bibliotecas del sistema, igual que se hace con SQLite.
// El motor WebKit va embebido en la app (sin barra de direcciones ni pestañas),
// así que lo que se ve es exactamente la web Nébula, píxel a píxel.
//
// Uso:
//   zig build desktop
//   STUDY_URL=http://127.0.0.1:3100/dashboard zig-out/bin/nova-desktop
//
// Transparencia ligera por defecto: ventana RGBA, superficies translúcidas y
// sin `backdrop-filter` (el desenfoque era lo que pesaba); aurora estática.
// `NOVA_GLASS=full` activa el vidrio esmerilado con blur. Se abre en el
// workspace donde estés. El tamaño se recuerda en ~/.config/nova/window.conf
// (la posición la decide el compositor en Wayland).
//
// Atajos: F5 o Ctrl+R recarga, Ctrl+H va al inicio, Ctrl+Q cierra,
// F12 abre las herramientas de desarrollo.

const std = @import("std");

const c = @cImport({
    @cInclude("gtk/gtk.h");
    @cInclude("gdk/gdkkeysyms.h");
    @cInclude("webkit2/webkit2.h");
});

var g_view: ?*c.GtkWidget = null;
var g_home: ?[*:0]const u8 = null;
var g_title: ?*c.GtkWidget = null;
var g_window: ?*c.GtkWidget = null;
var g_allocator: std.mem.Allocator = undefined;
var g_full: bool = false;

const DEFAULT_W: c.gint = 1200;
const DEFAULT_H: c.gint = 700;

// cimport deja _GdkEventKey incompleto; el layout es ABI estable en GTK3.
const GdkEventKey = extern struct {
    type: c.GdkEventType,
    window: ?*c.GdkWindow,
    send_event: c.gint8,
    time: c.guint32,
    state: c.guint,
    keyval: c.guint,
    length: c.gint,
    string: [*c]u8,
    hardware_keycode: c.guint16,
    group: c.guint8,
    is_modifier: c.guint,
};

// ─────────────────────────── Geometría recordada ───────────────────────────

fn configPath(buf: []u8) ?[]const u8 {
    const home = std.posix.getenv("HOME") orelse return null;
    return std.fmt.bufPrint(buf, "{s}/.config/nova/window.conf", .{home}) catch null;
}

fn configDir(buf: []u8) ?[]const u8 {
    const home = std.posix.getenv("HOME") orelse return null;
    return std.fmt.bufPrint(buf, "{s}/.config/nova", .{home}) catch null;
}

fn loadGeometry() struct { w: c.gint, h: c.gint } {
    var path_buf: [std.fs.max_path_bytes]u8 = undefined;
    const path = configPath(&path_buf) orelse return .{ .w = DEFAULT_W, .h = DEFAULT_H };
    const data = std.fs.cwd().readFileAlloc(g_allocator, path, 64) catch
        return .{ .w = DEFAULT_W, .h = DEFAULT_H };
    defer g_allocator.free(data);
    const trimmed = std.mem.trim(u8, data, " \t\r\n");
    const x = std.mem.indexOfScalar(u8, trimmed, 'x') orelse
        return .{ .w = DEFAULT_W, .h = DEFAULT_H };
    const w = std.fmt.parseInt(c.gint, trimmed[0..x], 10) catch DEFAULT_W;
    const h = std.fmt.parseInt(c.gint, trimmed[x + 1 ..], 10) catch DEFAULT_H;
    if (w < 640 or h < 400) return .{ .w = DEFAULT_W, .h = DEFAULT_H };
    return .{ .w = w, .h = h };
}

fn saveGeometry(window: *c.GtkWidget) void {
    var w: c.gint = 0;
    var h: c.gint = 0;
    c.gtk_window_get_size(@ptrCast(window), &w, &h);
    if (w < 640 or h < 400) return;
    var dir_buf: [std.fs.max_path_bytes]u8 = undefined;
    const dir = configDir(&dir_buf) orelse return;
    std.fs.cwd().makePath(dir) catch {};
    var path_buf: [std.fs.max_path_bytes]u8 = undefined;
    const path = configPath(&path_buf) orelse return;
    var file = std.fs.cwd().createFile(path, .{}) catch return;
    defer file.close();
    file.writer().print("{d}x{d}\n", .{ w, h }) catch {};
}

// ─────────────────────────── Señales ───────────────────────────

fn onDestroy(widget: *c.GtkWidget, data: ?*anyopaque) callconv(.C) void {
    _ = data;
    saveGeometry(widget);
    g_window = null;
    g_view = null;
    c.gtk_main_quit();
}

fn applyWindowCss() void {
    const css: [:0]const u8 =
        "window, window.background, .background { background-color: transparent; background-image: none; box-shadow: none; }" ++
        "headerbar { background-color: rgba(10, 12, 20, 0.55); background-image: none; border-bottom: 1px solid rgba(255, 255, 255, 0.07); box-shadow: none; }" ++
        "headerbar:backdrop { background-color: rgba(10, 12, 20, 0.45); }";
    const provider = c.gtk_css_provider_new();
    _ = c.gtk_css_provider_load_from_data(@ptrCast(provider), css.ptr, -1, null);
    c.gtk_style_context_add_provider_for_screen(
        c.gdk_screen_get_default(),
        @ptrCast(provider),
        c.GTK_STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    c.g_object_unref(@ptrCast(provider));
}

// La ventana tiene canal alfa: si no se limpia el fondo, GTK deja basura.
fn onDraw(widget: *c.GtkWidget, cr: *c.cairo_t, data: ?*anyopaque) callconv(.C) c.gboolean {
    _ = widget;
    _ = data;
    c.cairo_set_source_rgba(cr, 0, 0, 0, 0);
    c.cairo_set_operator(cr, c.CAIRO_OPERATOR_SOURCE);
    c.cairo_paint(cr);
    return 0;
}

fn viewAsWebView(widget: *c.GtkWidget) *c.WebKitWebView {
    return @ptrCast(widget);
}

// Transparencia Nébula. Por defecto: ventana RGBA con superficies translúcidas
// y CERO `backdrop-filter` (el desenfoque era lo que saturaba la GPU); la aurora
// queda estática. `NOVA_GLASS=full` añade el vidrio esmerilado con blur.
fn injectGlassCss(view: *c.WebKitWebView, frosted: bool) void {
    const css: [:0]const u8 = if (frosted)
        "html, body { background: transparent !important; background-image: none !important; }" ++
            ":root, html, body, [data-theme] {" ++
            "  --nx-bg: rgba(8, 11, 20, 0.12) !important;" ++
            "  --nx-bg-deep: rgba(5, 7, 13, 0.16) !important;" ++
            "  --nx-sidebar: rgba(10, 13, 22, 0.22) !important;" ++
            "  --nx-surface: rgba(17, 24, 39, 0.30) !important;" ++
            "  --nx-surface-2: rgba(23, 32, 51, 0.34) !important;" ++
            "  --nx-surface-3: rgba(29, 41, 64, 0.38) !important;" ++
            "  --nx-panel: rgba(17, 24, 39, 0.30) !important;" ++
            "  --nx-panel-solid: rgba(17, 24, 39, 0.30) !important;" ++
            "}" ++
            "#contenido, .nx-chat, .nx-conversation, main, .nx-dashboard { background: transparent !important; }" ++
            "* { -webkit-backdrop-filter: none !important; backdrop-filter: none !important; }" ++
            ".nx-sidebar { -webkit-backdrop-filter: blur(28px) saturate(160%) !important; backdrop-filter: blur(28px) saturate(160%) !important; }" ++
            ".nx-topbar { -webkit-backdrop-filter: blur(22px) saturate(160%) !important; backdrop-filter: blur(22px) saturate(160%) !important; }" ++
            ".nx-panel, .nx-card, .nx-kpi, .nx-week-strip, .nx-cal-day, .nx-cal-kpi," ++
            ".nx-academic-card, .nx-plan-card, .nx-chart-card, .nx-message, .nx-chat-composer," ++
            ".gen-office-tabs, .gen-office-launcher, .gen-office-home-search {" ++
            "  -webkit-backdrop-filter: blur(18px) saturate(150%) !important;" ++
            "  backdrop-filter: blur(18px) saturate(150%) !important;" ++
            "}" ++
            "body::after { animation: none !important; }" ++
            "html { scroll-behavior: auto !important; }"
    else
        "html, body { background: transparent !important; background-image: none !important; }" ++
            ":root, html, body, [data-theme] {" ++
            "  --nx-bg: rgba(8, 11, 20, 0.55) !important;" ++
            "  --nx-bg-deep: rgba(5, 7, 13, 0.62) !important;" ++
            "  --nx-sidebar: rgba(10, 13, 22, 0.68) !important;" ++
            "  --nx-surface: rgba(17, 24, 39, 0.72) !important;" ++
            "  --nx-surface-2: rgba(23, 32, 51, 0.74) !important;" ++
            "  --nx-surface-3: rgba(29, 41, 64, 0.76) !important;" ++
            "  --nx-panel: rgba(17, 24, 39, 0.72) !important;" ++
            "  --nx-panel-solid: rgba(17, 24, 39, 0.78) !important;" ++
            "}" ++
            "#contenido, .nx-chat, .nx-conversation, main, .nx-dashboard { background: transparent !important; }" ++
            "* { -webkit-backdrop-filter: none !important; backdrop-filter: none !important; }" ++
            "body::after { animation: none !important; filter: none !important; }" ++
            "html { scroll-behavior: auto !important; }";
    const manager = c.webkit_web_view_get_user_content_manager(view);
    const sheet = c.webkit_user_style_sheet_new(
        css.ptr,
        c.WEBKIT_USER_CONTENT_INJECT_ALL_FRAMES,
        c.WEBKIT_USER_STYLE_LEVEL_USER,
        null,
        null,
    );
    c.webkit_user_content_manager_add_style_sheet(manager, sheet);
    c.webkit_user_style_sheet_unref(sheet);
}

fn onReload(button: ?*c.GtkWidget, data: ?*anyopaque) callconv(.C) void {
    _ = button;
    _ = data;
    if (g_view) |view| c.webkit_web_view_reload(viewAsWebView(view));
}

fn onHome(button: ?*c.GtkWidget, data: ?*anyopaque) callconv(.C) void {
    _ = button;
    _ = data;
    if (g_view) |view| {
        if (g_home) |home| c.webkit_web_view_load_uri(viewAsWebView(view), home);
    }
}

fn onBack(button: ?*c.GtkWidget, data: ?*anyopaque) callconv(.C) void {
    _ = button;
    _ = data;
    if (g_view) |view| c.webkit_web_view_go_back(viewAsWebView(view));
}

fn onDevTools(button: ?*c.GtkWidget, data: ?*anyopaque) callconv(.C) void {
    _ = button;
    _ = data;
    if (g_view) |view| {
        const inspector = c.webkit_web_view_get_inspector(viewAsWebView(view));
        c.webkit_web_inspector_show(inspector);
    }
}

fn onKeyPress(widget: *c.GtkWidget, event: *GdkEventKey, data: ?*anyopaque) callconv(.C) c.gboolean {
    _ = widget;
    _ = data;
    const ctrl = (event.state & c.GDK_CONTROL_MASK) != 0;
    switch (event.keyval) {
        c.GDK_KEY_F5 => {
            onReload(null, null);
            return 1;
        },
        c.GDK_KEY_F12 => {
            onDevTools(null, null);
            return 1;
        },
        c.GDK_KEY_r, c.GDK_KEY_R => if (ctrl) {
            onReload(null, null);
            return 1;
        },
        c.GDK_KEY_h, c.GDK_KEY_H => if (ctrl) {
            onHome(null, null);
            return 1;
        },
        c.GDK_KEY_q, c.GDK_KEY_Q => if (ctrl) {
            if (g_window) |w| c.gtk_widget_destroy(w);
            return 1;
        },
        else => {},
    }
    return 0;
}

fn onLoadFailed(
    view: *c.WebKitWebView,
    event: c.WebKitLoadEvent,
    failing_uri: [*c]const u8,
    err: ?*c.GError,
    data: ?*anyopaque,
) callconv(.C) c.gboolean {
    _ = view;
    _ = event;
    _ = failing_uri;
    _ = data;
    if (err) |e| {
        if (e.message) |msg| {
            std.debug.print("NOVA: no se pudo cargar la app: {s}\n", .{std.mem.span(msg)});
        }
    }
    return 0; // FALSE: deja que WebKit muestre su página de error.
}

fn onPermissionRequest(
    view: *c.WebKitWebView,
    request: *c.WebKitPermissionRequest,
    data: ?*anyopaque,
) callconv(.C) c.gboolean {
    _ = view;
    _ = data;
    // El micrófono es necesario para grabar audio y transcribirlo dentro de la
    // app; el resto de permisos se deja a WebKit.
    const user_media_type = c.webkit_user_media_permission_request_get_type();
    if (c.g_type_check_instance_is_a(@ptrCast(@alignCast(request)), user_media_type) != 0) {
        c.webkit_permission_request_allow(request);
        return 1;
    }
    return 0;
}

fn onTitleChanged(object: *c.GObject, pspec: ?*c.GParamSpec, data: ?*anyopaque) callconv(.C) void {
    _ = pspec;
    _ = data;
    if (g_title) |label| {
        const title = c.webkit_web_view_get_title(@ptrCast(@alignCast(object)));
        if (title != null) {
            const text = std.mem.span(title);
            if (text.len > 0) {
                const full = std.fmt.allocPrintZ(g_allocator, "{s} · NOVA", .{text}) catch return;
                defer g_allocator.free(full);
                c.gtk_header_bar_set_title(@ptrCast(label), full.ptr);
            }
        }
    }
}

// ─────────────────────────── Ventana ───────────────────────────

fn buildWindow() void {
    const window = c.gtk_window_new(c.GTK_WINDOW_TOPLEVEL);
    g_window = window;
    c.gtk_window_set_title(@ptrCast(window), "NOVA · Study Platform");
    const geom = loadGeometry();
    c.gtk_window_set_default_size(@ptrCast(window), geom.w, geom.h);
    c.gtk_window_set_icon_name(@ptrCast(window), "applications-internet");
    c.gtk_widget_set_size_request(window, 880, 560);

    // La ventana siempre es RGBA: la transparencia es el efecto que se quiere.
    if (c.gtk_widget_get_screen(window)) |screen| {
        if (c.gdk_screen_get_rgba_visual(screen)) |visual| {
            c.gtk_widget_set_visual(window, visual);
        }
    }
    c.gtk_widget_set_app_paintable(window, 1);
    _ = c.g_signal_connect_data(window, "draw", @as(c.GCallback, @ptrCast(&onDraw)), null, null, 0);

    const box = c.gtk_box_new(c.GTK_ORIENTATION_VERTICAL, 0);

    const header = c.gtk_header_bar_new();
    c.gtk_header_bar_set_show_close_button(@ptrCast(header), 1);
    c.gtk_header_bar_set_title(@ptrCast(header), "NOVA · Study Platform");
    c.gtk_header_bar_set_subtitle(@ptrCast(header), "Personal OS · Nébula");
    g_title = header;

    const back = c.gtk_button_new_from_icon_name("go-previous-symbolic", c.GTK_ICON_SIZE_BUTTON);
    c.gtk_button_set_relief(@ptrCast(back), c.GTK_RELIEF_NONE);
    c.gtk_widget_set_tooltip_text(back, "Atrás (Alt+Izquierda)");
    _ = c.g_signal_connect_data(back, "clicked", @as(c.GCallback, @ptrCast(&onBack)), null, null, 0);
    c.gtk_header_bar_pack_start(@ptrCast(header), back);

    const home = c.gtk_button_new_from_icon_name("go-home-symbolic", c.GTK_ICON_SIZE_BUTTON);
    c.gtk_button_set_relief(@ptrCast(home), c.GTK_RELIEF_NONE);
    c.gtk_widget_set_tooltip_text(home, "Inicio (Ctrl+H)");
    _ = c.g_signal_connect_data(home, "clicked", @as(c.GCallback, @ptrCast(&onHome)), null, null, 0);
    c.gtk_header_bar_pack_start(@ptrCast(header), home);

    const reload = c.gtk_button_new_from_icon_name("view-refresh-symbolic", c.GTK_ICON_SIZE_BUTTON);
    c.gtk_button_set_relief(@ptrCast(reload), c.GTK_RELIEF_NONE);
    c.gtk_widget_set_tooltip_text(reload, "Recargar (F5)");
    _ = c.g_signal_connect_data(reload, "clicked", @as(c.GCallback, @ptrCast(&onReload)), null, null, 0);
    c.gtk_header_bar_pack_start(@ptrCast(header), reload);

    const devtools = c.gtk_button_new_from_icon_name("utilities-terminal-symbolic", c.GTK_ICON_SIZE_BUTTON);
    c.gtk_button_set_relief(@ptrCast(devtools), c.GTK_RELIEF_NONE);
    c.gtk_widget_set_tooltip_text(devtools, "Herramientas de desarrollo (F12)");
    _ = c.g_signal_connect_data(devtools, "clicked", @as(c.GCallback, @ptrCast(&onDevTools)), null, null, 0);
    c.gtk_header_bar_pack_end(@ptrCast(header), devtools);

    c.gtk_window_set_titlebar(@ptrCast(window), header);

    const view = c.webkit_web_view_new();
    g_view = view;
    var transparent = c.GdkRGBA{ .red = 0, .green = 0, .blue = 0, .alpha = 0 };
    c.webkit_web_view_set_background_color(@ptrCast(view), &transparent);
    injectGlassCss(viewAsWebView(view), g_full);
    const settings = c.webkit_web_view_get_settings(@ptrCast(view));
    c.webkit_settings_set_enable_developer_extras(settings, 1);
    c.webkit_settings_set_hardware_acceleration_policy(settings, c.WEBKIT_HARDWARE_ACCELERATION_POLICY_ALWAYS);
    c.webkit_settings_set_enable_smooth_scrolling(settings, 0);
    c.webkit_settings_set_enable_webgl(settings, 0);
    c.webkit_settings_set_enable_webaudio(settings, 1);
    c.webkit_settings_set_enable_media_stream(settings, 1);
    c.webkit_settings_set_enable_webrtc(settings, 1);
    c.webkit_settings_set_enable_write_console_messages_to_stdout(settings, 0);

    _ = c.g_signal_connect_data(view, "load-failed", @as(c.GCallback, @ptrCast(&onLoadFailed)), null, null, 0);
    _ = c.g_signal_connect_data(view, "permission-request", @as(c.GCallback, @ptrCast(&onPermissionRequest)), null, null, 0);
    _ = c.g_signal_connect_data(view, "notify::title", @as(c.GCallback, @ptrCast(&onTitleChanged)), null, null, 0);
    _ = c.g_signal_connect_data(window, "destroy", @as(c.GCallback, @ptrCast(&onDestroy)), null, null, 0);
    _ = c.g_signal_connect_data(window, "key-press-event", @as(c.GCallback, @ptrCast(&onKeyPress)), null, null, 0);

    c.gtk_box_pack_start(@ptrCast(box), view, 1, 1, 0);
    c.gtk_container_add(@ptrCast(window), box);
    if (g_home) |home_uri| c.webkit_web_view_load_uri(@ptrCast(view), home_uri);
}

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    g_allocator = gpa.allocator();

    const url = blk: {
        if (std.process.getEnvVarOwned(g_allocator, "STUDY_URL")) |value| break :blk value else |_| {}
        break :blk try g_allocator.dupe(u8, "http://127.0.0.1:3100/dashboard");
    };
    defer g_allocator.free(url);
    const url_z = try g_allocator.dupeZ(u8, url);
    defer g_allocator.free(url_z);
    g_home = url_z.ptr;

    _ = c.gtk_init(null, null);
    c.g_set_prgname("nova");
    c.g_set_application_name("NOVA · Study Platform");
    c.webkit_web_context_set_cache_model(c.webkit_web_context_get_default(), c.WEBKIT_CACHE_MODEL_WEB_BROWSER);

    g_full = if (std.posix.getenv("NOVA_GLASS")) |value| std.mem.eql(u8, value, "full") else false;

    applyWindowCss();
    buildWindow();
    c.gtk_widget_show_all(g_window.?);
    c.gtk_main();
}
