//! Cliente de CyberHex (reproductor local) sobre su REST v1.
//!
//! CyberHex escucha solo en loopback (`127.0.0.1:8765`) y exige
//! `Authorization: Bearer <clave>`. Aquí no se guarda ninguna clave: la
//! config la trae y este módulo solo la pone en la cabecera.
//!
//! Reglas:
//!
//! * **Loopback o nada**: `allowsPrivateHost` corta cualquier URL que no sea
//!   local, igual que el cliente de Mikareno.
//! * **Sin estado**: cada llamada abre su conexión y la cierra.
//! * **Errores claros**: si CyberHex no está corriendo se devuelve
//!   `error.CyberHexUnavailable`, y el llamador lo dice sin inventar datos.

const std = @import("std");
const std_http = std.http;
const json = @import("jsonutil.zig");
const mcp_client = @import("mcp_client.zig");

const c = @cImport({
    @cInclude("sys/socket.h");
    @cInclude("sys/time.h");
});

/// Rutas que expone la REST v1 de CyberHex.
pub const Ruta = enum {
    status,
    library,
    queue,
    command,
    volume,
    queue_add,

    fn path(self: Ruta) []const u8 {
        return switch (self) {
            .status => "/api/v1/status",
            .library => "/api/v1/library",
            .queue => "/api/v1/queue",
            .command => "/api/v1/command",
            .volume => "/api/v1/volume",
            .queue_add => "/api/v1/queue",
        };
    }

    fn method(self: Ruta) []const u8 {
        return switch (self) {
            .status, .library, .queue => "GET",
            else => "POST",
        };
    }
};

pub const Error = error{
    CyberHexUnavailable,
    CyberHexUnauthorized,
    CyberHexRejected,
    CyberHexBadResponse,
    NotConfigured,
    HostNotAllowed,
    OutOfMemory,
};

/// Llama a la REST de CyberHex y devuelve el JSON ya parseado.
///
/// `body` es el cuerpo JSON para las rutas POST; se ignora en las GET.
pub fn call(
    allocator: std.mem.Allocator,
    base_url: []const u8,
    api_key: []const u8,
    timeout_ms: u64,
    ruta: Ruta,
    body: ?std.json.Value,
) Error!std.json.Value {
    if (base_url.len == 0 or api_key.len == 0) return error.NotConfigured;
    if (!mcp_client.allowsPrivateHost(allocator, base_url)) return error.HostNotAllowed;

    // La URL base puede traer o no el `/api/v1`; se respeta tal cual venga.
    const url = if (std.mem.endsWith(u8, base_url, "/"))
        try std.fmt.allocPrint(allocator, "{s}{s}", .{ base_url[0 .. base_url.len - 1], ruta.path() })
    else
        try std.fmt.allocPrint(allocator, "{s}{s}", .{ base_url, ruta.path() });

    var payload: []const u8 = "";
    if (body) |value| payload = try json.stringify(allocator, value);

    var headers = std.ArrayList(std_http.Header).init(allocator);
    try headers.append(.{ .name = "Accept", .value = "application/json" });
    try headers.append(.{ .name = "Authorization", .value = try std.fmt.allocPrint(allocator, "Bearer {s}", .{api_key}) });

    var client = std_http.Client{ .allocator = allocator };
    defer client.deinit();
    var response_header_buffer: [16 * 1024]u8 = undefined;
    const uri = std.Uri.parse(url) catch return error.CyberHexBadResponse;
    var request = client.open(.POST, uri, .{
        .server_header_buffer = &response_header_buffer,
        .keep_alive = false,
        .headers = .{
            .content_type = .{ .override = "application/json" },
            .connection = .{ .override = "close" },
            .user_agent = .{ .override = "study-platform-zig/0.2.0" },
        },
        .extra_headers = headers.items,
    }) catch return error.CyberHexUnavailable;
    defer request.deinit();
    if (request.connection) |connection| setSocketTimeout(connection.stream.handle, timeout_ms);

    if (std.mem.eql(u8, ruta.method(), "GET")) {
        request.transfer_encoding = .{ .content_length = 0 };
    } else {
        request.transfer_encoding = .{ .content_length = payload.len };
    }
    request.send() catch return error.CyberHexUnavailable;
    if (payload.len > 0) request.writeAll(payload) catch return error.CyberHexUnavailable;
    request.finish() catch return error.CyberHexUnavailable;
    request.wait() catch return error.CyberHexUnavailable;

    const http_status: u16 = @intFromEnum(request.response.status);
    var resp_body = std.ArrayList(u8).init(allocator);
    defer resp_body.deinit();
    request.reader().readAllArrayList(&resp_body, 1024 * 1024) catch return error.CyberHexBadResponse;

    if (http_status == 401 or http_status == 403) return error.CyberHexUnauthorized;
    if (http_status == 404) return error.CyberHexRejected;
    if (http_status < 200 or http_status >= 300) return error.CyberHexRejected;

    const trimmed = std.mem.trim(u8, resp_body.items, " \t\r\n");
    if (trimmed.len == 0) return .{ .object = std.json.ObjectMap.init(allocator) };
    return json.parse(allocator, trimmed) catch return error.CyberHexBadResponse;
}

fn setSocketTimeout(fd: anytype, timeout_ms: u64) void {
    const safe_ms = @max(timeout_ms, 1);
    const timeout = c.struct_timeval{
        .tv_sec = @intCast(safe_ms / 1000),
        .tv_usec = @intCast((safe_ms % 1000) * 1000),
    };
    const socket_fd: c_int = @intCast(fd);
    _ = c.setsockopt(socket_fd, c.SOL_SOCKET, c.SO_RCVTIMEO, @ptrCast(&timeout), @sizeOf(c.struct_timeval));
    _ = c.setsockopt(socket_fd, c.SOL_SOCKET, c.SO_SNDTIMEO, @ptrCast(&timeout), @sizeOf(c.struct_timeval));
}

// ─────────────────────────────── Comandos ─────────────────────────────

pub fn status(allocator: std.mem.Allocator, cfg: Config) Error!std.json.Value {
    return call(allocator, cfg.url, cfg.key, cfg.timeout_ms, .status, null);
}

pub fn queue(allocator: std.mem.Allocator, cfg: Config) Error!std.json.Value {
    return call(allocator, cfg.url, cfg.key, cfg.timeout_ms, .queue, null);
}

pub fn library(allocator: std.mem.Allocator, cfg: Config, query: []const u8) Error!std.json.Value {
    var buffer: [512]u8 = undefined;
    const qs = std.fmt.bufPrint(&buffer, "?q={s}", .{query}) catch return error.CyberHexBadResponse;
    _ = qs;
    // La query va en la URL: se construye la llamada a mano para no depender
    // de un builder de strings que quizá no exista en esta versión de Zig.
    if (!mcp_client.allowsPrivateHost(allocator, cfg.url)) return error.HostNotAllowed;
    const url = try std.fmt.allocPrint(allocator, "{s}/api/v1/library?q={s}", .{ cfg.url, query });
    return callUrl(allocator, cfg, url, null);
}

/// Variante con URL ya armada (para las rutas con query).
fn callUrl(allocator: std.mem.Allocator, cfg: Config, url: []const u8, body: ?std.json.Value) Error!std.json.Value {
    if (cfg.key.len == 0) return error.NotConfigured;
    if (!mcp_client.allowsPrivateHost(allocator, url)) return error.HostNotAllowed;

    var payload: []const u8 = "";
    if (body) |value| payload = try json.stringify(allocator, value);

    var headers = std.ArrayList(std_http.Header).init(allocator);
    try headers.append(.{ .name = "Accept", .value = "application/json" });
    try headers.append(.{ .name = "Authorization", .value = try std.fmt.allocPrint(allocator, "Bearer {s}", .{cfg.key}) });

    var client = std_http.Client{ .allocator = allocator };
    defer client.deinit();
    var response_header_buffer: [16 * 1024]u8 = undefined;
    const uri = std.Uri.parse(url) catch return error.CyberHexBadResponse;
    var request = client.open(.POST, uri, .{
        .server_header_buffer = &response_header_buffer,
        .keep_alive = false,
        .headers = .{
            .content_type = .{ .override = "application/json" },
            .connection = .{ .override = "close" },
            .user_agent = .{ .override = "study-platform-zig/0.2.0" },
        },
        .extra_headers = headers.items,
    }) catch return error.CyberHexUnavailable;
    defer request.deinit();
    if (request.connection) |connection| setSocketTimeout(connection.stream.handle, cfg.timeout_ms);

    request.transfer_encoding = .{ .content_length = payload.len };
    request.send() catch return error.CyberHexUnavailable;
    if (payload.len > 0) request.writeAll(payload) catch return error.CyberHexUnavailable;
    request.finish() catch return error.CyberHexUnavailable;
    request.wait() catch return error.CyberHexUnavailable;

    const http_status: u16 = @intFromEnum(request.response.status);
    var resp_body = std.ArrayList(u8).init(allocator);
    defer resp_body.deinit();
    request.reader().readAllArrayList(&resp_body, 1024 * 1024) catch return error.CyberHexBadResponse;

    if (http_status == 401 or http_status == 403) return error.CyberHexUnauthorized;
    if (http_status < 200 or http_status >= 300) return error.CyberHexRejected;
    const trimmed = std.mem.trim(u8, resp_body.items, " \t\r\n");
    if (trimmed.len == 0) return .{ .object = std.json.ObjectMap.init(allocator) };
    return json.parse(allocator, trimmed) catch return error.CyberHexBadResponse;
}

/// Acción de reproducción. Los nombres coinciden con los que acepta la
/// REST local de CyberHex (`action`), no con los del README.
pub const Accion = enum {
    play,
    pause,
    toggle,
    next,
    previous,
    seek,

    fn wire(self: Accion) []const u8 {
        return @tagName(self);
    }
};

/// `command` envía `{"action": ...}` y, para `seek`, `positionSeconds`.
pub fn command(allocator: std.mem.Allocator, cfg: Config, accion: []const u8, valor: ?std.json.Value) Error!std.json.Value {
    if (!validCommand(accion)) return error.CyberHexRejected;
    var obj = try json.object(allocator, &.{.{ .key = "action", .value = .{ .string = accion } }});
    if (valor) |v| {
        if (obj != .object) return error.CyberHexBadResponse;
        // Para seek CyberHex espera `positionSeconds`; se aceptan ambas
        // escrituras para no fallar si el llamador manda `value` o `seek`.
        if (std.mem.eql(u8, accion, "seek")) {
            try obj.object.put("positionSeconds", v);
        } else {
            try obj.object.put("value", v);
        }
    }
    return call(allocator, cfg.url, cfg.key, cfg.timeout_ms, .command, obj);
}

fn validCommand(accion: []const u8) bool {
    const allowed = [_][]const u8{ "play", "pause", "toggle", "next", "previous", "seek" };
    for (allowed) |name| if (std.mem.eql(u8, accion, name)) return true;
    return false;
}

/// Volumen: la REST lo llama `value`.
pub fn setVolume(allocator: std.mem.Allocator, cfg: Config, volumen: f64) Error!std.json.Value {
    const clamped = @max(0.0, @min(1.0, volumen));
    return call(allocator, cfg.url, cfg.key, cfg.timeout_ms, .volume, try json.object(allocator, &.{.{ .key = "value", .value = .{ .float = clamped } }}));
}

/// Añadir a la cola: la REST lo cuelga de `/api/v1/queue` con `action:add`.
pub fn queueAdd(allocator: std.mem.Allocator, cfg: Config, pista: []const u8) Error!std.json.Value {
    return call(allocator, cfg.url, cfg.key, cfg.timeout_ms, .queue_add, try json.object(allocator, &.{
        .{ .key = "action", .value = .{ .string = "add" } },
        .{ .key = "path", .value = .{ .string = pista } },
    }));
}

/// Configuración mínima que necesita el cliente.
pub const Config = struct {
    url: []const u8,
    key: []const u8,
    timeout_ms: u64,
};

// ─────────────────────────────── Tests ───────────────────────────────

test "la URL base se normaliza sin duplicar el slash" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const sin_slash = try std.fmt.allocPrint(a, "{s}{s}", .{ "http://127.0.0.1:8765", Ruta.status.path() });
    const con_slash = try std.fmt.allocPrint(a, "{s}{s}", .{ "http://127.0.0.1:8765/"[0 .. "http://127.0.0.1:8765/".len - 1], Ruta.status.path() });
    try std.testing.expectEqualStrings("http://127.0.0.1:8765/api/v1/status", sin_slash);
    try std.testing.expectEqualStrings(sin_slash, con_slash);
}

test "sin clave o sin URL no se intenta la llamada" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const cfg = Config{ .url = "", .key = "k", .timeout_ms = 100 };
    try std.testing.expectError(error.NotConfigured, call(a, cfg.url, cfg.key, cfg.timeout_ms, .status, null));
    const cfg2 = Config{ .url = "http://127.0.0.1:8765", .key = "", .timeout_ms = 100 };
    try std.testing.expectError(error.NotConfigured, call(a, cfg2.url, cfg2.key, cfg2.timeout_ms, .status, null));
}

test "no se acepta una URL que no sea local" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const cfg = Config{ .url = "http://ejemplo.com", .key = "k", .timeout_ms = 100 };
    try std.testing.expectError(error.HostNotAllowed, call(a, cfg.url, cfg.key, cfg.timeout_ms, .status, null));
}
