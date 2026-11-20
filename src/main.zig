const std = @import("std");
const http = @import("http.zig");
const config_mod = @import("config.zig");
const sqlite = @import("sqlite.zig");
const app_mod = @import("app.zig");
const security = @import("security.zig");

var stopping = std.atomic.Value(bool).init(false);
var listener_fd: c_int = -1;

pub fn main() !void {
    const allocator = std.heap.page_allocator;
    const args = try std.process.argsAlloc(allocator);
    defer std.process.argsFree(allocator, args);

    var command: enum { serve, migrate, create_admin, help, version } = .serve;
    var address: []const u8 = "0.0.0.0:8090";
    var admin_name: []const u8 = "admin";
    var index: usize = 1;
    while (index < args.len) : (index += 1) {
        const arg = args[index];
        if (std.mem.eql(u8, arg, "--help") or std.mem.eql(u8, arg, "-h")) command = .help else if (std.mem.eql(u8, arg, "--version") or std.mem.eql(u8, arg, "-V")) command = .version else if (std.mem.eql(u8, arg, "--migrate")) command = .migrate else if (std.mem.eql(u8, arg, "--create-admin")) {
            command = .create_admin;
            if (index + 1 < args.len) {
                index += 1;
                admin_name = args[index];
            }
        } else if (std.mem.eql(u8, arg, "--addr") or std.mem.eql(u8, arg, "-a")) {
            if (index + 1 >= args.len) return error.MissingAddress;
            index += 1;
            address = args[index];
        } else if (std.mem.startsWith(u8, arg, "--addr=")) {
            address = arg[7..];
        } else {
            std.log.err("Argumento desconocido: {s}", .{arg});
            return error.InvalidArgument;
        }
    }

    if (command == .help) {
        printHelp();
        return;
    }
    if (command == .version) {
        std.debug.print("study-platform 0.2.0 (Zig {s})\n", .{builtinVersion()});
        return;
    }

    var arena = std.heap.ArenaAllocator.init(allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var config = config_mod.Config.load(a) catch |err| {
        std.log.err("Configuración inválida: {s}. Define SESSION_SECRET con al menos 32 caracteres.", .{@errorName(err)});
        return err;
    };
    var db = try sqlite.Db.open(a, config.db_path);
    defer db.close();

    if (command == .migrate) {
        std.log.info("Migraciones SQLite aplicadas correctamente: {s}", .{config.db_path});
        return;
    }
    if (command == .create_admin) {
        const temporary = try security.generateSessionToken(a);
        const hash = try security.hashPassword(a, temporary);
        var stmt = try db.prepare("INSERT INTO users(username,password_hash) VALUES(?1,?2) ON CONFLICT(username) DO UPDATE SET password_hash=excluded.password_hash");
        defer stmt.deinit();
        try stmt.bindText(1, admin_name);
        try stmt.bindText(2, hash);
        _ = try stmt.next();
        std.log.info("Cuenta administrativa creada. No se imprimió la contraseña temporal; configura AUTH_DISABLED=true para establecer una nueva desde /settings/password.", .{});
        return;
    }

    const bind = try parseAddress(a, address);
    var app = app_mod.App.init(allocator, &config, &db);
    try serve(bind.host, bind.port, &app);
}

const BindAddress = struct { host: []const u8, port: u16 };

fn parseAddress(allocator: std.mem.Allocator, raw: []const u8) !BindAddress {
    const split = std.mem.lastIndexOfScalar(u8, raw, ':') orelse return error.InvalidAddress;
    const host = raw[0..split];
    const port = std.fmt.parseInt(u16, raw[split + 1 ..], 10) catch return error.InvalidAddress;
    if (host.len == 0 or port == 0) return error.InvalidAddress;
    return .{ .host = try allocator.dupe(u8, host), .port = port };
}

fn serve(host: []const u8, port: u16, app: *app_mod.App) !void {
    const c = http.c;
    const fd = c.socket(c.AF_INET, c.SOCK_STREAM, 0);
    if (fd < 0) return error.SocketCreateFailed;
    listener_fd = fd;
    defer {
        _ = c.close(fd);
        listener_fd = -1;
    }
    var reuse: c_int = 1;
    _ = c.setsockopt(fd, c.SOL_SOCKET, c.SO_REUSEADDR, @ptrCast(&reuse), @sizeOf(c_int));

    var address = std.mem.zeroes(c.struct_sockaddr_in);
    address.sin_family = c.AF_INET;
    address.sin_port = c.htons(port);
    if (std.mem.eql(u8, host, "0.0.0.0")) {
        address.sin_addr.s_addr = c.htonl(0);
    } else if (std.mem.eql(u8, host, "localhost")) {
        address.sin_addr.s_addr = c.htonl(0x7f000001);
    } else {
        const host_z = try std.heap.page_allocator.dupeZ(u8, host);
        defer std.heap.page_allocator.free(host_z);
        if (c.inet_pton(c.AF_INET, host_z, &address.sin_addr) != 1) return error.InvalidAddress;
    }
    if (c.bind(fd, @ptrCast(&address), @sizeOf(c.struct_sockaddr_in)) != 0) return error.BindFailed;
    if (c.listen(fd, 128) != 0) return error.ListenFailed;

    stopping.store(false, .monotonic);
    _ = c.signal(c.SIGTERM, onSignal);
    _ = c.signal(c.SIGINT, onSignal);
    std.log.info("Study Platform Zig escucha en {s}:{d}", .{ host, port });
    const backup_thread = try std.Thread.spawn(.{}, dailyBackupWorker, .{app});
    defer backup_thread.join();

    while (!stopping.load(.monotonic)) {
        var peer = std.mem.zeroes(c.struct_sockaddr_in);
        var peer_len: c.socklen_t = @sizeOf(c.struct_sockaddr_in);
        const client_fd = c.accept(fd, @ptrCast(&peer), &peer_len);
        if (client_fd < 0) {
            if (stopping.load(.monotonic)) break;
            continue;
        }
        var timeout = c.struct_timeval{ .tv_sec = 15, .tv_usec = 0 };
        _ = c.setsockopt(client_fd, c.SOL_SOCKET, c.SO_RCVTIMEO, @ptrCast(&timeout), @sizeOf(c.struct_timeval));
        const peer_ip = peerAddress(peer) catch std.heap.page_allocator.dupe(u8, "0.0.0.0") catch {
            _ = c.close(client_fd);
            continue;
        };
        const thread = std.Thread.spawn(.{}, clientThread, .{ client_fd, peer_ip, app }) catch {
            _ = c.close(client_fd);
            std.heap.page_allocator.free(peer_ip);
            continue;
        };
        thread.detach();
    }
    std.log.info("Servidor Zig detenido limpiamente", .{});
}

fn dailyBackupWorker(app: *app_mod.App) void {
    // Match the original cadence: first backup one minute after startup, then
    // every 24 hours. Short sleeps make shutdown interruptible.
    var seconds_waited: u64 = 0;
    while (!stopping.load(.monotonic)) {
        if (seconds_waited == 60) {
            app.createScheduledBackup() catch |err| std.log.err("Backup diario falló: {s}", .{@errorName(err)});
            seconds_waited = 0;
            var daily_wait: u64 = 0;
            while (daily_wait < 24 * 60 * 60 and !stopping.load(.monotonic)) : (daily_wait += 1) std.time.sleep(std.time.ns_per_s);
            continue;
        }
        std.time.sleep(std.time.ns_per_s);
        seconds_waited += 1;
    }
}

fn onSignal(_: c_int) callconv(.c) void {
    stopping.store(true, .monotonic);
    if (listener_fd >= 0) _ = http.c.shutdown(listener_fd, http.c.SHUT_RDWR);
}

fn peerAddress(peer: http.c.struct_sockaddr_in) ![]const u8 {
    const ip = http.c.ntohl(peer.sin_addr.s_addr);
    return std.fmt.allocPrint(std.heap.page_allocator, "{d}.{d}.{d}.{d}", .{ (ip >> 24) & 0xff, (ip >> 16) & 0xff, (ip >> 8) & 0xff, ip & 0xff });
}

fn clientThread(fd: c_int, peer_ip: []const u8, app: *app_mod.App) void {
    defer _ = http.c.close(fd);
    defer std.heap.page_allocator.free(peer_ip);
    var arena = std.heap.ArenaAllocator.init(std.heap.page_allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    const request = http.readRequest(allocator, fd, peer_ip) catch |err| {
        const response = if (err == error.PayloadTooLarge)
            http.Response{ .status = 413, .content_type = "text/plain; charset=utf-8", .body = "Payload Too Large" }
        else
            http.Response{ .status = 400, .content_type = "text/plain; charset=utf-8", .body = "Bad Request" };
        http.writeResponse(fd, allocator, response);
        return;
    };
    const response = app.dispatch(allocator, &request) catch |err| blk: {
        std.log.warn("Fallo de petición {s} {s}: {s}", .{ request.method, request.path, @errorName(err) });
        break :blk http.Response{ .status = 500, .content_type = "application/json; charset=utf-8", .body = "{\"error\":\"Internal error\"}" };
    };
    http.writeResponse(fd, allocator, response);
}

fn printHelp() void {
    std.debug.print(
        \\Study Platform — NOVA 3.0 (backend 100 % Zig)
        \\Uso: study-platform [--addr HOST:PORT] [--migrate] [--create-admin USER]
        \\
        \\  --addr, -a      Dirección IPv4 de escucha (default 0.0.0.0:8090)
        \\  --migrate       Aplica las migraciones SQLite embebidas y sale
        \\  --create-admin  Crea o restablece una cuenta administrativa
        \\  --version       Muestra versión y versión de Zig
        \\  --help          Muestra esta ayuda
        \\
        \\La configuración se lee de variables de entorno y .env; SESSION_SECRET
        \\es obligatoria (mínimo 32 caracteres).
        \\
    , .{});
}

fn builtinVersion() []const u8 {
    return "0.14.x";
}

test "address parsing rejects malformed values" {
    try std.testing.expectError(error.InvalidAddress, parseAddress(std.testing.allocator, "localhost"));
    const address = try parseAddress(std.testing.allocator, "0.0.0.0:8090");
    defer std.testing.allocator.free(address.host);
    try std.testing.expectEqual(@as(u16, 8090), address.port);
}
