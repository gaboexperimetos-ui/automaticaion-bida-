//! Inicio de sesión con Google (OAuth 2.0, flujo de código con PKCE).
//!
//! Por qué así:
//!
//! * **PKCE**: el cliente no guarda un secreto que pueda filtrarse en una app
//!   local; se manda `code_challenge` y Google exige el `code_verifier`.
//! * **state**: un valor aleatorio guardado en una cookie de corta vida. Si
//!   al volver no coincide, se rechaza: corta el CSRF del callback.
//! * **Solo se pide lo justo**: `openid email profile`. No se toca Drive ni
//!   Gmail del usuario, así que no hace falta revisión de Google.
//! * **Sin contraseña**: la cuenta se crea con una contraseña aleatoria que
//!   nadie conoce; solo se entra por Google.

const std = @import("std");
const std_http = std.http;
const json = @import("jsonutil.zig");
const security = @import("security.zig");

const c = @cImport({
    @cInclude("sys/socket.h");
    @cInclude("sys/time.h");
});

pub const Error = error{
    NotConfigured,
    BadResponse,
    HttpError,
    StateMismatch,
    EmailMissing,
    OutOfMemory,
};

pub const Config = struct {
    client_id: []const u8,
    client_secret: []const u8,
    redirect_uri: []const u8,
    timeout_ms: u64,
};

pub const Identity = struct {
    subject: []const u8,
    email: []const u8,
    name: []const u8,
    /// Token para renovar el acceso; vacío si Google no lo dio.
    refresh_token: []const u8 = "",
    /// Scopes efectivamente concedidos por el usuario.
    scope: []const u8 = "",
    /// access_token recién obtenido (válido ~1 h), para el primer uso.
    access_token: []const u8 = "",
};

const AUTH_ENDPOINT = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT = "https://oauth2.googleapis.com/token";
const USERINFO_ENDPOINT = "https://www.googleapis.com/oauth2/v3/userinfo";

pub fn urlEncode(allocator: std.mem.Allocator, raw: []const u8) ![]const u8 {
    const hex = "0123456789ABCDEF";
    var out = std.ArrayList(u8).init(allocator);
    for (raw) |ch| {
        const keep = (ch >= 'a' and ch <= 'z') or
            (ch >= 'A' and ch <= 'Z') or
            (ch >= '0' and ch <= '9') or
            ch == '-' or ch == '_' or ch == '.' or ch == '~';
        if (keep) {
            try out.append(ch);
        } else {
            try out.append('%');
            try out.append(hex[ch >> 4]);
            try out.append(hex[ch & 0x0f]);
        }
    }
    return out.items;
}

/// Token aleatorio para `state` y para el `code_verifier` de PKCE.
pub fn randomToken(allocator: std.mem.Allocator) ![]const u8 {
    var bytes: [32]u8 = undefined;
    std.crypto.random.bytes(&bytes);
    return std.fmt.allocPrint(allocator, "{x}", .{std.fmt.fmtSliceHexLower(&bytes)});
}

/// `code_challenge` = SHA256(verifier) en base64url sin relleno.
pub fn codeChallenge(allocator: std.mem.Allocator, verifier: []const u8) ![]const u8 {
    var digest: [32]u8 = undefined;
    std.crypto.hash.sha2.Sha256.hash(verifier, &digest, .{});
    const enc = std.base64.url_safe_no_pad.Encoder;
    const out = try allocator.alloc(u8, enc.calcSize(digest.len));
    return enc.encode(out, &digest);
}

/// URL a la que se manda el navegador para empezar el login.
pub fn authorizationUrl(allocator: std.mem.Allocator, cfg: Config, state: []const u8, challenge: []const u8) ![]const u8 {
    if (cfg.client_id.len == 0) return error.NotConfigured;
    return std.fmt.allocPrint(
        allocator,
        "{s}?client_id={s}&redirect_uri={s}&response_type=code&scope={s}&state={s}&code_challenge={s}&code_challenge_method=S256&access_type=offline&prompt=consent",
        .{
            AUTH_ENDPOINT,
            try urlEncode(allocator, cfg.client_id),
            try urlEncode(allocator, cfg.redirect_uri),
            try urlEncode(allocator, SCOPES),
            try urlEncode(allocator, state),
            try urlEncode(allocator, challenge),
        },
    );
}

/// Scopes pedidos: identidad + lectura de Google Calendar.
/// `access_type=offline` y `prompt=consent` fuerzan el refresh_token.
pub const SCOPES = "openid email profile " ++
    "https://www.googleapis.com/auth/calendar.readonly " ++
    "https://www.googleapis.com/auth/drive.readonly " ++
    "https://www.googleapis.com/auth/contacts.readonly";

fn postForm(allocator: std.mem.Allocator, url: []const u8, body: []const u8, timeout_ms: u64) Error![]const u8 {
    var client = std_http.Client{ .allocator = allocator };
    defer client.deinit();
    var header_buffer: [16 * 1024]u8 = undefined;
    const uri = std.Uri.parse(url) catch return error.BadResponse;
    var request = client.open(.POST, uri, .{
        .server_header_buffer = &header_buffer,
        .keep_alive = false,
        .headers = .{
            .content_type = .{ .override = "application/x-www-form-urlencoded" },
            .accept_encoding = .{ .override = "identity" },
            .connection = .{ .override = "close" },
            .user_agent = .{ .override = "study-platform-zig/0.2.0" },
        },
    }) catch return error.HttpError;
    defer request.deinit();
    if (request.connection) |connection| setSocketTimeout(connection.stream.handle, timeout_ms);
    request.transfer_encoding = .{ .content_length = body.len };
    request.send() catch return error.HttpError;
    request.writeAll(body) catch return error.HttpError;
    request.finish() catch return error.HttpError;
    request.wait() catch return error.HttpError;
    const status: u16 = @intFromEnum(request.response.status);
    var out = std.ArrayList(u8).init(allocator);
    request.reader().readAllArrayList(&out, 1024 * 1024) catch return error.BadResponse;
    if (status < 200 or status >= 300) return error.HttpError;
    return out.items;
}

/// GET autenticado con Bearer contra una API de Google. Pública para que
/// otros módulos (Drive, Contacts) la reutilicen.
pub fn getJsonPublic(allocator: std.mem.Allocator, url: []const u8, token: []const u8, timeout_ms: u64) Error!std.json.Value {
    return getJson(allocator, url, token, timeout_ms);
}

fn getJson(allocator: std.mem.Allocator, url: []const u8, token: []const u8, timeout_ms: u64) Error!std.json.Value {
    var client = std_http.Client{ .allocator = allocator };
    defer client.deinit();
    var header_buffer: [16 * 1024]u8 = undefined;
    const uri = std.Uri.parse(url) catch return error.BadResponse;
    var headers = std.ArrayList(std_http.Header).init(allocator);
    try headers.append(.{ .name = "Authorization", .value = try std.fmt.allocPrint(allocator, "Bearer {s}", .{token}) });
    var request = client.open(.GET, uri, .{
        .server_header_buffer = &header_buffer,
        .keep_alive = false,
        .headers = .{
            .accept_encoding = .{ .override = "identity" },
            .connection = .{ .override = "close" },
            .user_agent = .{ .override = "study-platform-zig/0.2.0" },
        },
        .extra_headers = headers.items,
    }) catch return error.HttpError;
    defer request.deinit();
    if (request.connection) |connection| setSocketTimeout(connection.stream.handle, timeout_ms);
    request.send() catch return error.HttpError;
    request.finish() catch return error.HttpError;
    request.wait() catch return error.HttpError;
    const status: u16 = @intFromEnum(request.response.status);
    var out = std.ArrayList(u8).init(allocator);
    request.reader().readAllArrayList(&out, 1024 * 1024) catch return error.BadResponse;
    if (status < 200 or status >= 300) return error.HttpError;
    const trimmed = std.mem.trim(u8, out.items, " \t\r\n");
    return json.parse(allocator, trimmed) catch return error.BadResponse;
}

/// GET autenticado que devuelve el cuerpo tal cual (para exportaciones de
/// Drive que no son JSON: texto de Docs, CSV de Sheets, etc.).
pub fn getTextPublic(allocator: std.mem.Allocator, url: []const u8, token: []const u8, timeout_ms: u64, max_bytes: usize) Error![]const u8 {
    var client = std_http.Client{ .allocator = allocator };
    defer client.deinit();
    var header_buffer: [16 * 1024]u8 = undefined;
    const uri = std.Uri.parse(url) catch return error.BadResponse;
    var headers = std.ArrayList(std_http.Header).init(allocator);
    try headers.append(.{ .name = "Authorization", .value = try std.fmt.allocPrint(allocator, "Bearer {s}", .{token}) });
    var request = client.open(.GET, uri, .{
        .server_header_buffer = &header_buffer,
        .keep_alive = false,
        .headers = .{
            .accept_encoding = .{ .override = "identity" },
            .connection = .{ .override = "close" },
            .user_agent = .{ .override = "study-platform-zig/0.2.0" },
        },
        .extra_headers = headers.items,
    }) catch return error.HttpError;
    defer request.deinit();
    if (request.connection) |connection| setSocketTimeout(connection.stream.handle, timeout_ms);
    request.send() catch return error.HttpError;
    request.finish() catch return error.HttpError;
    request.wait() catch return error.HttpError;
    const status: u16 = @intFromEnum(request.response.status);
    var out = std.ArrayList(u8).init(allocator);
    request.reader().readAllArrayList(&out, @max(max_bytes, 4096)) catch return error.BadResponse;
    if (status < 200 or status >= 300) return error.HttpError;
    return out.items;
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

/// Cambia el `code` por un `id_token` y devuelve la identidad del usuario.
pub fn exchange(allocator: std.mem.Allocator, cfg: Config, code: []const u8, verifier: []const u8) Error!Identity {
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) return error.NotConfigured;
    const body = try std.fmt.allocPrint(
        allocator,
        "code={s}&client_id={s}&client_secret={s}&redirect_uri={s}&grant_type=authorization_code&code_verifier={s}",
        .{
            try urlEncode(allocator, code),
            try urlEncode(allocator, cfg.client_id),
            try urlEncode(allocator, cfg.client_secret),
            try urlEncode(allocator, cfg.redirect_uri),
            try urlEncode(allocator, verifier),
        },
    );
    const raw = try postForm(allocator, TOKEN_ENDPOINT, body, cfg.timeout_ms);
    const token_json = json.parse(allocator, raw) catch return error.BadResponse;
    const access = json.getString(token_json, "access_token") orelse return error.BadResponse;
    const refresh = json.getString(token_json, "refresh_token") orelse "";
    const scope = json.getString(token_json, "scope") orelse "";

    const info = try getJson(allocator, USERINFO_ENDPOINT, access, cfg.timeout_ms);
    const subject = json.getString(info, "sub") orelse return error.BadResponse;
    const email = json.getString(info, "email") orelse return error.EmailMissing;
    const name = json.getString(info, "name") orelse json.getString(info, "email") orelse "";
    return .{
        .subject = subject,
        .email = email,
        .name = name,
        .refresh_token = refresh,
        .scope = scope,
        .access_token = access,
    };
}

const CALENDAR_ENDPOINT = "https://www.googleapis.com/calendar/v3/calendars/primary/events";

/// Lista los próximos eventos del Google Calendar del usuario.
/// Devuelve el JSON crudo de la API de Google Calendar.
pub fn calendarEvents(
    allocator: std.mem.Allocator,
    cfg: Config,
    access_token: []const u8,
    max_results: usize,
) Error!std.json.Value {
    if (cfg.client_id.len == 0) return error.NotConfigured;
    if (access_token.len == 0) return error.HttpError;
    const url = try std.fmt.allocPrint(
        allocator,
        "{s}?maxResults={d}&singleEvents=true&orderBy=startTime&timeMin={s}",
        .{ CALENDAR_ENDPOINT, max_results, try rfc3339NowUtc(allocator) },
    );
    return getJson(allocator, url, access_token, cfg.timeout_ms);
}

/// Renueva el access_token a partir del refresh_token guardado.
pub fn refreshAccessToken(allocator: std.mem.Allocator, cfg: Config, refresh_token: []const u8) Error![]const u8 {
    if (cfg.client_id.len == 0 or cfg.client_secret.len == 0) return error.NotConfigured;
    if (refresh_token.len == 0) return error.HttpError;
    const body = try std.fmt.allocPrint(
        allocator,
        "client_id={s}&client_secret={s}&refresh_token={s}&grant_type=refresh_token",
        .{
            try urlEncode(allocator, cfg.client_id),
            try urlEncode(allocator, cfg.client_secret),
            try urlEncode(allocator, refresh_token),
        },
    );
    const raw = try postForm(allocator, TOKEN_ENDPOINT, body, cfg.timeout_ms);
    const token_json = json.parse(allocator, raw) catch return error.BadResponse;
    return json.getString(token_json, "access_token") orelse error.BadResponse;
}

/// Minuto actual en RFC 3339 UTC para acotar la consulta de Calendar.
fn rfc3339NowUtc(allocator: std.mem.Allocator) ![]const u8 {
    const e = std.time.epoch.EpochSeconds{ .secs = @intCast(@max(0, std.time.timestamp())) };
    const year_day = e.getEpochDay().calculateYearDay();
    const month_day = year_day.calculateMonthDay();
    const day_seconds = e.getDaySeconds();
    return std.fmt.allocPrint(
        allocator,
        "{d:0>4}-{d:0>2}-{d:0>2}T{d:0>2}:{d:0>2}:{d:0>2}Z",
        .{ year_day.year, month_day.month.numeric(), month_day.day_index + 1, day_seconds.getHoursIntoDay(), day_seconds.getMinutesIntoHour(), day_seconds.getSecondsIntoMinute() },
    );
}

/// Cookie corta que guarda el `state` y el `code_verifier` de este intento.
pub fn stateCookieName() []const u8 {
    return "nova_oauth";
}

pub fn buildStateCookie(allocator: std.mem.Allocator, state: []const u8, verifier: []const u8, secure: bool) ![]const u8 {
    // Formato `state.verifier`; los dos son hex y no llevan punto.
    const payload = try std.fmt.allocPrint(allocator, "{s}.{s}", .{ state, verifier });
    const encoded = try urlEncode(allocator, payload);
    return std.fmt.allocPrint(
        allocator,
        "{s}={s}; Path=/auth/google; HttpOnly; SameSite=Lax; Max-Age=600{s}",
        .{ stateCookieName(), encoded, if (secure) "; Secure" else "" },
    );
}

/// Saca `state` y `verifier` de la cookie guardada. Null si no está o está
/// mal formada.
pub fn parseStateCookie(allocator: std.mem.Allocator, cookie_header: []const u8) !?struct { state: []const u8, verifier: []const u8 } {
    const prefix = try std.fmt.allocPrint(allocator, "{s}=", .{stateCookieName()});
    var parts = std.mem.splitScalar(u8, cookie_header, ';');
    while (parts.next()) |raw| {
        const item = std.mem.trim(u8, raw, " \t");
        if (!std.mem.startsWith(u8, item, prefix)) continue;
        const value = item[prefix.len..];
        const dot = std.mem.indexOfScalar(u8, value, '.') orelse return null;
        const state = try percentDecode(allocator, value[0..dot]);
        const verifier = try percentDecode(allocator, value[dot + 1 ..]);
        if (state.len == 0 or verifier.len < 43) return null;
        return .{ .state = state, .verifier = verifier };
    }
    return null;
}

fn percentDecode(allocator: std.mem.Allocator, raw: []const u8) ![]const u8 {
    var out = std.ArrayList(u8).init(allocator);
    var i: usize = 0;
    while (i < raw.len) {
        if (raw[i] == '%' and i + 2 < raw.len) {
            const hi = std.fmt.charToDigit(raw[i + 1], 16) catch {
                try out.append(raw[i]);
                i += 1;
                continue;
            };
            const lo = std.fmt.charToDigit(raw[i + 2], 16) catch {
                try out.append(raw[i]);
                i += 1;
                continue;
            };
            try out.append(hi * 16 + lo);
            i += 3;
        } else {
            try out.append(raw[i]);
            i += 1;
        }
    }
    return out.items;
}

pub fn clearStateCookie(allocator: std.mem.Allocator, secure: bool) ![]const u8 {
    return std.fmt.allocPrint(
        allocator,
        "{s}=; Path=/auth/google; HttpOnly; SameSite=Lax; Max-Age=0{s}",
        .{ stateCookieName(), if (secure) "; Secure" else "" },
    );
}

// ─────────────────────────────── Tests ───────────────────────────────

test "el code_challenge es SHA256 en base64url" {
    // Vector de la RFC 7636: verifier "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const challenge = try codeChallenge(a, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    try std.testing.expectEqualStrings("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM", challenge);
}

test "la URL de Google lleva todo lo obligatorio" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const cfg = Config{
        .client_id = "abc.apps.googleusercontent.com",
        .client_secret = "s",
        .redirect_uri = "http://127.0.0.1:8090/auth/google/callback",
        .timeout_ms = 1000,
    };
    const url = try authorizationUrl(a, cfg, "el-state", "el-challenge");
    try std.testing.expect(std.mem.startsWith(u8, url, "https://accounts.google.com/o/oauth2/v2/auth?"));
    try std.testing.expect(std.mem.indexOf(u8, url, "code_challenge_method=S256") != null);
    try std.testing.expect(std.mem.indexOf(u8, url, "scope=openid%20email%20profile") != null);
    // Sin esto Google no entrega refresh_token y el calendario se quedaría
    // sin actualizar en cuanto caduca el access_token.
    try std.testing.expect(std.mem.indexOf(u8, url, "access_type=offline") != null);
    try std.testing.expect(std.mem.indexOf(u8, url, "prompt=consent") != null);
    // El redirect_uri va escapado: los dos puntos y las barras no se dejan crudos.
    try std.testing.expect(std.mem.indexOf(u8, url, "http%3A%2F%2F127.0.0.1%3A8090") != null);
}

test "sin client_id no se arma la URL" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const cfg = Config{ .client_id = "", .client_secret = "", .redirect_uri = "", .timeout_ms = 1 };
    try std.testing.expectError(error.NotConfigured, authorizationUrl(a, cfg, "s", "c"));
}

test "la cookie de estado viaja y se lee" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const verifier = try randomToken(a);
    const state = try randomToken(a);
    const cookie = try buildStateCookie(a, state, verifier, false);
    // La cookie lleva los dos valores separados por un punto.
    const marker = try std.fmt.allocPrint(a, "{s}={s}.{s};", .{ stateCookieName(), state, verifier });
    try std.testing.expect(std.mem.indexOf(u8, cookie, marker) != null);

    // Y se recuperan intactos desde una cabecera Cookie real.
    const header = try std.fmt.allocPrint(a, "otra=1; {s}", .{cookie});
    const parsed = (try parseStateCookie(a, header)).?;
    try std.testing.expectEqualStrings(state, parsed.state);
    try std.testing.expectEqualStrings(verifier, parsed.verifier);
}

test "una cookie sin el separador se rechaza" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const header = try std.fmt.allocPrint(a, "{s}=sin-separador", .{stateCookieName()});
    try std.testing.expect((try parseStateCookie(a, header)) == null);
}

test "sin secreto no se puede canjear el codigo" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const cfg = Config{ .client_id = "id", .client_secret = "", .redirect_uri = "http://x/cb", .timeout_ms = 1 };
    try std.testing.expectError(error.NotConfigured, exchange(a, cfg, "code", "verifier"));
}

test "la URL codifica caracteres raros del redirect" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const cfg = Config{
        .client_id = "id con espacio",
        .client_secret = "s",
        .redirect_uri = "http://127.0.0.1:8090/auth/google/callback?a=1&b=2",
        .timeout_ms = 1,
    };
    const url = try authorizationUrl(a, cfg, "s", "c");
    // Ni el espacio ni el & del redirect pueden quedar crudos.
    try std.testing.expect(std.mem.indexOf(u8, url, " ") == null);
    try std.testing.expect(std.mem.indexOf(u8, url, "client_id=id%20con%20espacio") != null);
    try std.testing.expect(std.mem.indexOf(u8, url, "a%3D1%26b%3D2") != null);
}

test "el state se compara como texto exacto" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const verifier = try randomToken(a);
    const cookie = try buildStateCookie(a, "abc", verifier, true);
    // La cookie va marcada como Secure cuando la peticion lo es.
    try std.testing.expect(std.mem.indexOf(u8, cookie, "; Secure") != null);
    try std.testing.expect(std.mem.indexOf(u8, cookie, "HttpOnly") != null);
    try std.testing.expect(std.mem.indexOf(u8, cookie, "SameSite=Lax") != null);
}

test "la cookie caduca se borra con Max-Age=0" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const cleared = try clearStateCookie(a, false);
    try std.testing.expect(std.mem.indexOf(u8, cleared, "Max-Age=0") != null);
    try std.testing.expect(std.mem.indexOf(u8, cleared, "; Secure") == null);
}
