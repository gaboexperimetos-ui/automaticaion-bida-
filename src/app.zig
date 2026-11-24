const std = @import("std");
const http = @import("http.zig");
const sqlite = @import("sqlite.zig");
const config_mod = @import("config.zig");
const json = @import("jsonutil.zig");
const security = @import("security.zig");
const assets = @import("assets.zig");
const templates = @import("templates.zig");
const notes = @import("notes.zig");
const ai = @import("ai.zig");
const mcp = @import("mcp.zig");
const system_metrics = @import("system_metrics.zig");
const mcp_client = @import("mcp_client.zig");
const office_mod = @import("office.zig");
const oauth_google = @import("oauth_google.zig");
const google_calendar = @import("google_calendar.zig");
const google_workspace = @import("google_workspace.zig");
const panel = @import("panel.zig");
const academico = @import("academico.zig");
const calendar_view = @import("calendar_view.zig");
const schedule_view = @import("schedule_view.zig");
const flashcards = @import("flashcards.zig");
const busqueda = @import("busqueda.zig");

pub const App = struct {
    allocator: std.mem.Allocator,
    config: *const config_mod.Config,
    db: *sqlite.Db,
    started_at: i64,
    rate_mutex: std.Thread.Mutex = .{},
    rates: std.StringHashMap(RateBucket),
    active_streams: std.atomic.Value(u32) = std.atomic.Value(u32).init(0),

    pub fn init(allocator: std.mem.Allocator, config: *const config_mod.Config, db: *sqlite.Db) App {
        return .{ .allocator = allocator, .config = config, .db = db, .started_at = std.time.timestamp(), .rates = std.StringHashMap(RateBucket).init(allocator) };
    }

    pub fn dispatch(self: *App, allocator: std.mem.Allocator, request: *const http.Request) !http.Response {
        const path = request.path;
        // Los estaticos no consumen el cubo general: una sola pagina carga
        // ~9 assets y con 60/min el navegador se quedaba en 429 al navegar.
        if (!isStaticAsset(path) and !self.allowRate(allocator, request.peer_ip, "general", GENERAL_RATE_LIMIT)) return self.rateLimited(allocator, request);
        if (std.mem.eql(u8, request.method, "POST") and std.mem.eql(u8, path, "/login")) {
            if (!self.allowRate(allocator, request.peer_ip, "login", 10)) return self.rateLimited(allocator, request);
        }
        if (std.mem.eql(u8, path, "/health")) return self.health(allocator);
        if (std.mem.eql(u8, path, "/api/status")) return self.status(allocator);
        if (std.mem.eql(u8, path, "/api/config") or std.mem.eql(u8, path, "/api/public-config")) return self.publicConfig(allocator);
        if (std.mem.eql(u8, path, "/api/debug")) return self.publicConfig(allocator);
        if (assets.find(path) != null or std.mem.startsWith(u8, path, "/static/") or std.mem.startsWith(u8, path, "/app-assets/")) {
            if (std.mem.startsWith(u8, path, "/static/") and std.mem.indexOf(u8, path, "..") != null) return text(allocator, 404, "text/plain; charset=utf-8", "not found");
            if (assets.find(path)) |asset| return text(allocator, 200, asset.mime, asset.bytes);
            return text(allocator, 404, "text/plain; charset=utf-8", "not found");
        }
        if (std.mem.eql(u8, path, "/styles.css")) {
            const css = assets.find("/static/study-platform-ui/styles.css").?;
            return text(allocator, 200, css.mime, css.bytes);
        }
        if (std.mem.eql(u8, path, "/app.js")) {
            const js = assets.find("/static/study-platform-ui/app.js").?;
            return text(allocator, 200, js.mime, js.bytes);
        }
        if (std.mem.eql(u8, path, "/legacy")) {
            const legacy = assets.find("/static/study-platform-ui/index.html").?;
            return text(allocator, 200, legacy.mime, legacy.bytes);
        }
        if (std.mem.eql(u8, path, "/auth/google")) return self.googleLoginStart(allocator, request);
        if (std.mem.eql(u8, path, "/auth/google/callback")) return self.googleLoginCallback(allocator, request);
        if (std.mem.eql(u8, path, "/login")) return self.login(allocator, request);
        if (std.mem.eql(u8, path, "/register")) return self.register(allocator, request);
        if (std.mem.eql(u8, path, "/logout")) return self.logout(allocator, request);
        if (std.mem.eql(u8, path, "/handoff")) return self.handoff(allocator);
        if (std.mem.startsWith(u8, path, "/mcp")) return mcp.handle(allocator, self.db, self.config, request);

        const auth = try self.authFor(request, allocator);
        if (self.requiresAuth(path) and auth == null) {
            if (std.mem.eql(u8, path, "/office/report") or std.mem.startsWith(u8, path, "/api/") or std.mem.eql(u8, path, "/api")) return jsonError(allocator, 401, "Unauthorized");
            return redirect(allocator, "/login", 302);
        }
        const username = if (auth) |a| a.username else "admin";

        if (std.mem.eql(u8, path, "/") or std.mem.eql(u8, path, "/dashboard") or std.mem.eql(u8, path, "/dashboard/")) {
            if (request.query.len > 0) {
                if (try http.queryParam(allocator, request.query, "partial")) |partial| {
                    if (partial.len > 32) return text(allocator, 200, "text/html; charset=utf-8", "");
                    return text(allocator, 200, "text/html; charset=utf-8", try self.partialDashboard(allocator, partial));
                }
            }
            return self.cleanPage(allocator, .dashboard, "/dashboard", username);
        }
        if (std.mem.eql(u8, path, "/dashboard/ai") or std.mem.eql(u8, path, "/ai") or std.mem.eql(u8, path, "/dashboard/chat")) return self.cleanPage(allocator, .assistant, "/dashboard/ai", username);
        if (std.mem.eql(u8, path, "/dashboard/notes")) return self.cleanPage(allocator, .notes, "/dashboard/notes", username);
        if (std.mem.eql(u8, path, "/dashboard/mcp")) return self.cleanPage(allocator, .mcp, "/dashboard/mcp", username);
        if (std.mem.eql(u8, path, "/dashboard/material") and std.mem.eql(u8, request.method, "GET")) return self.cleanPage(allocator, .material, path, username);
        if (std.mem.eql(u8, path, "/settings")) return self.cleanPage(allocator, .settings, path, username);
        if (std.mem.eql(u8, path, "/settings/password")) return self.password(allocator, request, auth.?);
        if (std.mem.eql(u8, path, "/api/settings/ai")) return self.settingsAi(allocator, request, username);

        if (std.mem.eql(u8, path, "/api/calendar.ics") or std.mem.eql(u8, path, "/dashboard/calendar.ics")) return self.calendarIcs(allocator);
        // Búsqueda global: notas + Mikareno + Google. Sustituye a la búsqueda
        // que antes solo miraba las notas, sin tocar `/api/search`.
        if (std.mem.eql(u8, path, "/dashboard/search")) {
            return self.unifiedSearch(allocator, request, username, if (auth) |a| a.user_id else 0);
        }
        if (notes.handles(path)) return notes.handle(allocator, self.db, self.config, request, username);
        if (flashcards.handles(path)) return flashcards.handle(allocator, self.db, request);

        if (std.mem.startsWith(u8, path, "/api/ai/") or std.mem.eql(u8, path, "/dashboard/material")) {
            return ai.handle(allocator, self.db, self.config, request, &self.active_streams);
        }
        if (std.mem.startsWith(u8, path, "/admin") or std.mem.startsWith(u8, path, "/api/admin/")) return self.admin(allocator, request, username);
        if (std.mem.startsWith(u8, path, "/office")) return self.office(allocator, request, username);
        // Calendario completo: mes, semana y día con datos reales, anotaciones
        // locales y Google Calendar en solo lectura.
        if (std.mem.eql(u8, path, "/dashboard/calendar") or std.mem.eql(u8, path, "/dashboard/calendar/")) {
            return self.calendarPage(allocator, request, path, username, .month, if (auth) |a| a.user_id else 0);
        }
        if (std.mem.eql(u8, path, "/dashboard/calendar/weekly") or std.mem.eql(u8, path, "/dashboard/calendar/week")) {
            return self.calendarPage(allocator, request, path, username, .week, if (auth) |a| a.user_id else 0);
        }
        if (std.mem.eql(u8, path, "/dashboard/calendar/daily") or std.mem.eql(u8, path, "/dashboard/calendar/day")) {
            return self.calendarPage(allocator, request, path, username, .day, if (auth) |a| a.user_id else 0);
        }
        // Tarjetas de estudio con repetición espaciada (solo local).
        if (std.mem.eql(u8, path, "/dashboard/flashcards") or std.mem.startsWith(u8, path, "/dashboard/flashcards/")) {
            return self.flashcardsPage(allocator, path, username);
        }
        if (std.mem.startsWith(u8, path, "/dashboard/")) return self.academic(allocator, request, username);

        if (std.mem.eql(u8, path, "/api/notes")) return jsonError(allocator, 404, "not_found");
        if (std.mem.startsWith(u8, path, "/api/") or std.mem.startsWith(u8, path, "/mcp")) return jsonError(allocator, 404, "not_found");
        const escaped = try security.htmlEscape(allocator, path);
        const body = try templates.simplePage(allocator, "Página no encontrada", try std.fmt.allocPrint(allocator, "<p>La ruta <code>{s}</code> no corresponde a una pantalla de la aplicación.</p><a href=\"/dashboard\">Volver al panel principal</a>", .{escaped}));
        return .{ .status = 404, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, path, body, username, auth != null, self.config.mikareno_mcp_url) };
    }

    fn isStaticAsset(path: []const u8) bool {
        const prefixes = [_][]const u8{ "/static/", "/vendor/", "/app-assets/" };
        for (prefixes) |prefix| if (std.mem.startsWith(u8, path, prefix)) return true;
        const exact = [_][]const u8{
            "/favicon.svg", "/favicon.ico",     "/nova.css",       "/polish.css",
            "/themes.css",  "/music.css",       "/shell.js",       "/styles.css",
            "/app.js",      "/calendar-ics.js", "/office-html.js", "/office-sheets.js",
        };
        for (exact) |asset| if (std.mem.eql(u8, path, asset)) return true;
        return false;
    }

    fn allowRate(self: *App, allocator: std.mem.Allocator, ip: []const u8, bucket: []const u8, limit: u32) bool {
        _ = allocator;
        const key = std.fmt.allocPrint(self.allocator, "{s}:{s}", .{ bucket, ip }) catch return true;
        self.rate_mutex.lock();
        defer self.rate_mutex.unlock();
        const now = std.time.timestamp();
        if (self.rates.getPtr(key)) |entry| {
            self.allocator.free(key);
            if (now - entry.window_start >= 60) {
                entry.* = .{ .window_start = now, .count = 1 };
                return true;
            }
            if (entry.count >= limit) return false;
            entry.count += 1;
            return true;
        }
        if (self.rates.count() > 4096) {
            var iter = self.rates.iterator();
            while (iter.next()) |entry| self.allocator.free(entry.key_ptr.*);
            self.rates.clearRetainingCapacity();
        }
        self.rates.put(key, .{ .window_start = now, .count = 1 }) catch {
            self.allocator.free(key);
            return true;
        };
        return true;
    }

    fn rateLimited(self: *App, allocator: std.mem.Allocator, request: *const http.Request) !http.Response {
        _ = self;
        if (std.mem.startsWith(u8, request.path, "/api/") or std.mem.startsWith(u8, request.path, "/mcp")) {
            var response = try jsonError(allocator, 429, "Demasiadas solicitudes");
            const headers = try allocator.alloc(http.Header, 1);
            headers[0] = .{ .name = "Retry-After", .value = "60" };
            response.headers = headers;
            return response;
        }
        const headers = try allocator.alloc(http.Header, 1);
        headers[0] = .{ .name = "Retry-After", .value = "60" };
        return .{ .status = 429, .content_type = "text/plain; charset=utf-8", .body = "Demasiadas solicitudes", .headers = headers };
    }

    fn requiresAuth(_: *App, path: []const u8) bool {
        if (std.mem.eql(u8, path, "/health") or std.mem.eql(u8, path, "/api/status") or std.mem.eql(u8, path, "/api/config") or std.mem.eql(u8, path, "/api/public-config") or std.mem.eql(u8, path, "/handoff") or std.mem.eql(u8, path, "/legacy") or std.mem.eql(u8, path, "/styles.css") or std.mem.eql(u8, path, "/app.js")) return false;
        return std.mem.startsWith(u8, path, "/api/") or std.mem.startsWith(u8, path, "/dashboard") or std.mem.startsWith(u8, path, "/app/") or std.mem.startsWith(u8, path, "/office") or std.mem.startsWith(u8, path, "/admin") or std.mem.startsWith(u8, path, "/settings") or std.mem.eql(u8, path, "/ai") or std.mem.eql(u8, path, "/");
    }

    fn authFor(self: *App, request: *const http.Request, allocator: std.mem.Allocator) !?Auth {
        if (self.config.auth_disabled) return Auth{ .user_id = 1, .username = "admin", .session_id = "" };
        const cookie = request.header("cookie") orelse return null;
        const token = cookieToken(cookie) orelse return null;
        if (token.len > 256 or token.len == 0) return null;
        const hashed = try security.hashSessionToken(allocator, self.config.session_secret, token);
        if (try self.lookupSession(hashed, allocator)) |found| {
            var refresh = self.db.prepare("UPDATE sessions SET expires_at=datetime('now','+1 day') WHERE id=?1") catch return found;
            defer refresh.deinit();
            try refresh.bindText(1, hashed);
            _ = refresh.next() catch {};
            return found;
        }
        // Transitional compatibility for sessions created before opaque-token hashing.
        return self.lookupSession(token, allocator);
    }

    fn lookupSession(self: *App, id: []const u8, allocator: std.mem.Allocator) !?Auth {
        var stmt = self.db.prepare("SELECT u.id,u.username FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.id=?1 AND s.expires_at>datetime('now') AND s.created_at>=datetime('now','-30 days')") catch return null;
        defer stmt.deinit();
        try stmt.bindText(1, id);
        if (!try stmt.next()) return null;
        return Auth{ .user_id = stmt.int64(0), .username = try allocator.dupe(u8, stmt.text(1)), .session_id = try allocator.dupe(u8, id) };
    }

    fn login(self: *App, allocator: std.mem.Allocator, request: *const http.Request) !http.Response {
        return self.loginSubmit(allocator, request);
    }

    /// Config de Google lista para usar, o null si falta algo.
    fn googleConfig(self: *App) ?oauth_google.Config {
        if (self.config.google_client_id.len == 0 or self.config.google_client_secret.len == 0) return null;
        return .{
            .client_id = self.config.google_client_id,
            .client_secret = self.config.google_client_secret,
            .redirect_uri = self.config.google_redirect_uri,
            .timeout_ms = self.config.oauth_timeout_ms,
        };
    }

    /// Paso 1: manda el navegador a Google con state y PKCE guardados en
    /// una cookie de 10 minutos.
    fn googleLoginStart(self: *App, allocator: std.mem.Allocator, request: *const http.Request) !http.Response {
        const cfg = self.googleConfig() orelse return redirect(allocator, "/login?google=off", 302);
        const state = try oauth_google.randomToken(allocator);
        const verifier = try oauth_google.randomToken(allocator);
        const challenge = try oauth_google.codeChallenge(allocator, verifier);
        const url = try oauth_google.authorizationUrl(allocator, cfg, state, challenge);
        const cookie = try oauth_google.buildStateCookie(allocator, state, verifier, requestSecure(request));
        const headers = try allocator.alloc(http.Header, 2);
        headers[0] = .{ .name = "Set-Cookie", .value = cookie };
        headers[1] = .{ .name = "Location", .value = url };
        return .{ .status = 302, .content_type = "text/plain; charset=utf-8", .body = "", .headers = headers };
    }

    /// Paso 2: Google vuelve con `code` y `state`. Se comprueba el state
    /// contra la cookie, se canjea el código y se crea o recupera la cuenta.
    fn googleLoginCallback(self: *App, allocator: std.mem.Allocator, request: *const http.Request) !http.Response {
        const cfg = self.googleConfig() orelse return redirect(allocator, "/login?google=off", 302);
        if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);

        // Si el usuario cancela, Google manda `error=access_denied`.
        if (try http.queryParam(allocator, request.query, "error")) |_| {
            return redirect(allocator, "/login?google=cancelled", 302);
        }
        const code = (try http.queryParam(allocator, request.query, "code")) orelse return redirect(allocator, "/login?google=missing", 302);
        const state = (try http.queryParam(allocator, request.query, "state")) orelse return redirect(allocator, "/login?google=state", 302);

        const cookie = request.header("cookie") orelse "";
        const saved = oauth_google.parseStateCookie(allocator, cookie) catch null;
        const stored = saved orelse return redirect(allocator, "/login?google=state", 302);
        // Comparación en tiempo constante: el state es el que cortaba el CSRF.
        if (!security.constantTimeEq(stored.state, state)) return redirect(allocator, "/login?google=state", 302);

        const identity = oauth_google.exchange(allocator, cfg, code, stored.verifier) catch {
            return redirect(allocator, "/login?google=failed", 302);
        };

        // Con AUTH_DISABLED el espacio es siempre del usuario local 1. Si el
        // token se guardara en otra cuenta, el calendario (que consulta como
        // usuario 1) nunca vería los eventos de Google. Se vincula ahí y se
        // conserva la sesión actual.
        if (self.config.auth_disabled and self.userExists(1)) {
            try self.saveGoogleToken(allocator, 1, identity);
            const clear_admin = try oauth_google.clearStateCookie(allocator, requestSecure(request));
            const headers = try allocator.alloc(http.Header, 2);
            headers[0] = .{ .name = "Set-Cookie", .value = clear_admin };
            headers[1] = .{ .name = "Location", .value = "/dashboard/calendar?google=linked" };
            return .{ .status = 302, .content_type = "text/plain; charset=utf-8", .body = "", .headers = headers };
        }

        const user_id = try self.upsertGoogleUser(allocator, identity);
        const token = try security.generateSessionToken(allocator);
        const hashed = try security.hashSessionToken(allocator, self.config.session_secret, token);
        var save = self.db.prepare("INSERT INTO sessions(id,user_id,expires_at,user_agent) VALUES(?1,?2,datetime('now','+1 day'),?3)") catch {
            return text(allocator, 500, "text/plain; charset=utf-8", "No se pudo abrir la sesión");
        };
        defer save.deinit();
        try save.bindText(1, hashed);
        try save.bindInt(2, user_id);
        try save.bindText(3, request.header("user-agent") orelse "");
        _ = try save.next();

        const cookie_value = try security.cookieValue(allocator, token, requestSecure(request));
        const clear = try oauth_google.clearStateCookie(allocator, requestSecure(request));
        const headers = try allocator.alloc(http.Header, 3);
        headers[0] = .{ .name = "Set-Cookie", .value = cookie_value };
        headers[1] = .{ .name = "Set-Cookie", .value = clear };
        headers[2] = .{ .name = "Location", .value = "/dashboard" };
        return .{ .status = 302, .content_type = "text/plain; charset=utf-8", .body = "", .headers = headers };
    }

    /// Crea la cuenta en la primera entrada o la recupera si ya existía.
    ///
    /// La contraseña es aleatoria y no se guarda en claro: la cuenta solo
    /// entra por Google, y si alguien intenta el login clásico fallará.
    fn upsertGoogleUser(self: *App, allocator: std.mem.Allocator, identity: oauth_google.Identity) !i64 {
        var find = self.db.prepare("SELECT id FROM users WHERE oauth_provider='google' AND oauth_subject=?1") catch return error.DbError;
        defer find.deinit();
        try find.bindText(1, identity.subject);
        if (try find.next()) {
            const existing_id = find.int64(0);
            try self.saveGoogleToken(allocator, existing_id, identity);
            return existing_id;
        }

        // Si ya existe un usuario local con ese correo, se vincula en vez de
        // crear un duplicado.
        var by_email = self.db.prepare("SELECT id FROM users WHERE username=?1 AND oauth_provider=''") catch return error.DbError;
        defer by_email.deinit();
        try by_email.bindText(1, identity.email);
        if (try by_email.next()) {
            const existing_id = by_email.int64(0);
            var link = self.db.prepare("UPDATE users SET oauth_provider='google', oauth_subject=?1, oauth_email=?2 WHERE id=?3") catch return error.DbError;
            defer link.deinit();
            try link.bindText(1, identity.subject);
            try link.bindText(2, identity.email);
            try link.bindInt(3, existing_id);
            _ = try link.next();
            try self.saveGoogleToken(allocator, existing_id, identity);
            return existing_id;
        }

        var pwd_bytes: [32]u8 = undefined;
        std.crypto.random.bytes(&pwd_bytes);
        const plain = try std.fmt.allocPrint(allocator, "{x}", .{std.fmt.fmtSliceHexLower(&pwd_bytes)});
        const hash = try security.hashPassword(allocator, plain);
        var insert = self.db.prepare("INSERT INTO users(username,password_hash,oauth_provider,oauth_subject,oauth_email) VALUES(?1,?2,'google',?3,?4)") catch return error.DbError;
        defer insert.deinit();
        try insert.bindText(1, identity.email);
        try insert.bindText(2, hash);
        try insert.bindText(3, identity.subject);
        try insert.bindText(4, identity.email);
        _ = try insert.next();
        const uid = self.db.lastInsertRowId();
        try self.saveGoogleToken(allocator, uid, identity);
        return uid;
    }

    /// Guarda (o actualiza) el refresh_token de Google para ese usuario.
    /// Si Google no devolvió refresh_token, se conserva el que hubiera.
    fn saveGoogleToken(self: *App, allocator: std.mem.Allocator, user_id: i64, identity: oauth_google.Identity) !void {
        const refresh = std.mem.trim(u8, identity.refresh_token, " \t\r\n");
        if (refresh.len == 0) return;
        var stmt = self.db.prepare(
            "INSERT INTO google_oauth_tokens(user_id,refresh_token,scope,updated_at) VALUES(?1,?2,?3,datetime('now')) " ++
                "ON CONFLICT(user_id) DO UPDATE SET refresh_token=excluded.refresh_token, scope=excluded.scope, updated_at=datetime('now')",
        ) catch return error.DbError;
        defer stmt.deinit();
        try stmt.bindText(1, try std.fmt.allocPrint(allocator, "{d}", .{user_id}));
        try stmt.bindText(2, identity.refresh_token);
        try stmt.bindText(3, identity.scope);
        _ = try stmt.next();
    }

    fn userExists(self: *App, id: i64) bool {
        var stmt = self.db.prepare("SELECT 1 FROM users WHERE id=?1") catch return false;
        defer stmt.deinit();
        var buf: [32]u8 = undefined;
        const text_id = std.fmt.bufPrint(&buf, "{d}", .{id}) catch return false;
        stmt.bindText(1, text_id) catch return false;
        return stmt.next() catch false;
    }

    /// access_token vigente del usuario: reutiliza el guardado o lo renueva.
    fn googleAccessToken(self: *App, allocator: std.mem.Allocator, user_id: i64) !?[]const u8 {
        _ = allocator;
        _ = user_id;
        _ = self;
        return null;
    }

    fn loginSubmit(self: *App, allocator: std.mem.Allocator, request: *const http.Request) !http.Response {
        if (std.mem.eql(u8, request.method, "GET")) {
            if (try self.authFor(request, allocator)) |_| return redirect(allocator, "/dashboard", 302);
            var google_btn: []const u8 = "";
            if (self.googleConfig() != null) {
                google_btn = "<a class=\"nova-google\" href=\"/auth/google\"><span class=\"material-symbols-outlined\">login</span>Entrar con Google</a><hr class=\"nova-or\">";
            }
            // Motivo del fallo, para que el usuario sepa qué hacer.
            if (try http.queryParam(allocator, request.query, "google")) |reason| {
                const msg: []const u8 = if (std.mem.eql(u8, reason, "cancelled"))
                    "Cancelaste el acceso con Google."
                else if (std.mem.eql(u8, reason, "state"))
                    "La petición de acceso venció o no coincidía. Inténtalo otra vez."
                else if (std.mem.eql(u8, reason, "missing"))
                    "Faltan datos en la vuelta de Google. Inténtalo otra vez."
                else if (std.mem.eql(u8, reason, "failed"))
                    "Google no pudo validar el acceso. Revisa la configuración OAuth."
                else if (std.mem.eql(u8, reason, "off"))
                    "El acceso con Google no está configurado en el servidor."
                else
                    "";
                if (msg.len > 0) {
                    google_btn = try std.fmt.allocPrint(
                        allocator,
                        "<p class=\"nova-oauth-error\" role=\"status\">{s}</p>{s}",
                        .{ msg, google_btn },
                    );
                }
            }
            return text(allocator, 200, "text/html; charset=utf-8", templates.loginPage(google_btn));
        }
        return self.loginPost(allocator, request);
    }

    fn loginPost(self: *App, allocator: std.mem.Allocator, request: *const http.Request) !http.Response {
        if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
        if (!originAllowed(request)) return text(allocator, 403, "text/plain; charset=utf-8", "Origen no autorizado");
        var form = try http.parseForm(allocator, request.body);
        const username = form.get("username") orelse "";
        const pwd = form.get("password") orelse "";
        var stmt = try self.db.prepare("SELECT id,password_hash FROM users WHERE username=?1");
        defer stmt.deinit();
        try stmt.bindText(1, username);
        if (!try stmt.next()) return text(allocator, 401, "text/plain; charset=utf-8", "Usuario o contraseña incorrectos");
        const user_id = stmt.int64(0);
        const hash = try allocator.dupe(u8, stmt.text(1));
        if (hash.len == 0 or !security.verifyPassword(allocator, hash, pwd)) return text(allocator, 401, "text/plain; charset=utf-8", "Usuario o contraseña incorrectos");
        const token = try security.generateSessionToken(allocator);
        const stored = try security.hashSessionToken(allocator, self.config.session_secret, token);
        var save = try self.db.prepare("INSERT INTO sessions(id,user_id,expires_at,user_agent) VALUES(?1,?2,datetime('now','+1 day'),?3)");
        defer save.deinit();
        try save.bindText(1, stored);
        try save.bindInt(2, user_id);
        try save.bindText(3, request.header("user-agent") orelse "");
        _ = try save.next();
        const cookie = try security.cookieValue(allocator, token, requestSecure(request));
        const headers = try allocator.alloc(http.Header, 2);
        headers[0] = .{ .name = "Set-Cookie", .value = cookie };
        headers[1] = .{ .name = "Location", .value = "/dashboard" };
        return .{ .status = 302, .content_type = "text/plain; charset=utf-8", .body = "", .headers = headers };
    }

    fn register(self: *App, allocator: std.mem.Allocator, request: *const http.Request) !http.Response {
        if (std.mem.eql(u8, request.method, "GET")) {
            return text(allocator, 200, "text/html; charset=utf-8", "<!doctype html><html lang=\"es\"><meta charset=\"utf-8\"><title>Registro NOVA</title><body><h1>Crear cuenta</h1><form method=\"post\"><input name=\"username\" required><input type=\"password\" name=\"password\" required><input type=\"password\" name=\"confirm\" required><input name=\"invite\"><button>Crear cuenta</button></form></body></html>");
        }
        if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
        const expected = self.config.registration_token orelse return text(allocator, 403, "text/plain; charset=utf-8", "El registro está deshabilitado: pide una cuenta al administrador");
        var form = try http.parseForm(allocator, request.body);
        const received = bearerToken(request.header("authorization")) orelse form.get("invite") orelse "";
        if (!security.constantTimeEq(std.mem.trim(u8, received, " \t"), expected)) return text(allocator, 403, "text/plain; charset=utf-8", "Invitación inválida: pide una cuenta al administrador");
        const username = form.get("username") orelse "";
        const pwd = form.get("password") orelse "";
        const confirm = form.get("confirm") orelse "";
        if (!std.mem.eql(u8, pwd, confirm)) return text(allocator, 400, "text/plain; charset=utf-8", "Las contraseñas no coinciden");
        if (pwd.len < 8 or username.len == 0 or username.len > 80) return text(allocator, 400, "text/plain; charset=utf-8", "Mínimo 8 caracteres");
        const hash = try security.hashPassword(allocator, pwd);
        var stmt = try self.db.prepare("INSERT INTO users(username,password_hash) VALUES(?1,?2)");
        defer stmt.deinit();
        try stmt.bindText(1, username);
        try stmt.bindText(2, hash);
        _ = stmt.next() catch return text(allocator, 409, "text/plain; charset=utf-8", "Ese usuario ya existe");
        return text(allocator, 200, "text/plain; charset=utf-8", "Cuenta creada. Ahora inicia sesión.");
    }

    fn logout(self: *App, allocator: std.mem.Allocator, request: *const http.Request) !http.Response {
        if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
        const cookie = request.header("cookie") orelse return redirect(allocator, "/login", 302);
        if (cookieToken(cookie)) |token| {
            const hashed = try security.hashSessionToken(allocator, self.config.session_secret, token);
            var stmt = self.db.prepare("DELETE FROM sessions WHERE id=?1 OR id=?2") catch return self.clearCookieResponse(allocator, request);
            defer stmt.deinit();
            try stmt.bindText(1, hashed);
            try stmt.bindText(2, token);
            _ = stmt.next() catch {};
        }
        return self.clearCookieResponse(allocator, request);
    }

    fn clearCookieResponse(self: *App, allocator: std.mem.Allocator, request: *const http.Request) !http.Response {
        _ = self;
        const headers = try allocator.alloc(http.Header, 1);
        headers[0] = .{ .name = "Set-Cookie", .value = try security.clearCookie(allocator, requestSecure(request)) };
        return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = templates.loginPage(""), .headers = headers };
    }

    fn password(self: *App, allocator: std.mem.Allocator, request: *const http.Request, auth: Auth) !http.Response {
        if (std.mem.eql(u8, request.method, "GET")) {
            const current_field = if (self.config.auth_disabled) "" else "<label>Contraseña actual<input name=\"current_password\" type=\"password\" required></label>";
            const body = try templates.simplePage(allocator, "Cambiar contraseña", try std.fmt.allocPrint(allocator, "<form method=\"post\" class=\"grid gap-3 max-w-lg\">{s}<label>Nueva contraseña<input name=\"new_password\" type=\"password\" minlength=\"12\" required></label><label>Confirmar nueva contraseña<input name=\"confirm\" type=\"password\" minlength=\"12\" required></label><button>Actualizar contraseña</button></form>", .{current_field}));
            return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, "/settings/password", body, auth.username, true, self.config.mikareno_mcp_url) };
        }
        if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
        var form = try http.parseForm(allocator, request.body);
        const current = form.get("current_password") orelse "";
        const next = form.get("new_password") orelse "";
        const confirm = form.get("confirm") orelse "";
        var find = try self.db.prepare("SELECT password_hash FROM users WHERE id=?1");
        defer find.deinit();
        try find.bindInt(1, auth.user_id);
        if (!try find.next()) return text(allocator, 404, "text/plain; charset=utf-8", "La cuenta no existe");
        if (!self.config.auth_disabled and !security.verifyPassword(allocator, find.text(0), current)) return text(allocator, 400, "text/plain; charset=utf-8", "La contraseña actual no es correcta");
        if (next.len < 12) return text(allocator, 400, "text/plain; charset=utf-8", "La contraseña debe tener al menos 12 caracteres");
        if (!std.mem.eql(u8, next, confirm)) return text(allocator, 400, "text/plain; charset=utf-8", "Las contraseñas no coinciden");
        const new_hash = try security.hashPassword(allocator, next);
        var update = try self.db.prepare("UPDATE users SET password_hash=?1 WHERE id=?2");
        defer update.deinit();
        try update.bindText(1, new_hash);
        try update.bindInt(2, auth.user_id);
        _ = try update.next();
        var revoke = try self.db.prepare("DELETE FROM sessions WHERE user_id=?1 AND id<>?2");
        defer revoke.deinit();
        try revoke.bindInt(1, auth.user_id);
        try revoke.bindText(2, auth.session_id);
        _ = try revoke.next();
        return redirect(allocator, "/settings/password?updated=1", 303);
    }

    fn settingsAi(self: *App, allocator: std.mem.Allocator, request: *const http.Request, username: []const u8) !http.Response {
        if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
        var form = try http.parseForm(allocator, request.body);
        const model = form.get("model") orelse self.config.openrouter_model;
        const tokens = form.get("max_tokens") orelse "8192";
        const cost = form.get("max_cost") orelse "2.0";
        if (model.len > 120) return text(allocator, 400, "text/html; charset=utf-8", "<p>El modelo supera los 120 caracteres.</p>");
        for (model) |ch| if (!(std.ascii.isAlphanumeric(ch) or std.mem.indexOfScalar(u8, "-_./:#@", ch) != null)) return text(allocator, 400, "text/html; charset=utf-8", "<p>Nombre de modelo no válido.</p>");
        const max_tokens = std.fmt.parseInt(u32, tokens, 10) catch return text(allocator, 400, "text/html; charset=utf-8", "<p>max_tokens debe estar entre 1 y 200000.</p>");
        const normalized_cost = try std.mem.replaceOwned(u8, allocator, cost, ",", ".");
        const max_cost = std.fmt.parseFloat(f64, normalized_cost) catch return text(allocator, 400, "text/html; charset=utf-8", "<p>Presupuesto no válido.</p>");
        if (max_tokens < 1 or max_tokens > 200000 or max_cost < 0 or max_cost > 100) return text(allocator, 400, "text/html; charset=utf-8", "<p>Revisa los límites de tokens y presupuesto.</p>");
        const snippet = try std.fmt.allocPrint(allocator, "<div class=\"rounded-xl border border-outline p-4\"><h2>Configuración no persistida</h2><p>Copia este bloque en .env y reinicia el servicio:</p><pre>OPENROUTER_MODEL={s}\nOPENROUTER_MAX_TOKENS={d}\nOPENROUTER_MAX_COST_USD={d:.2}</pre><p>La clave nunca se muestra.</p></div>", .{ model, max_tokens, max_cost });
        if (std.mem.eql(u8, request.header("hx-request") orelse "", "true")) return text(allocator, 501, "text/html; charset=utf-8", snippet);
        const body = try templates.simplePage(allocator, "Configuración", snippet);
        return .{ .status = 501, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, "/settings", body, username, true, self.config.mikareno_mcp_url) };
    }

    fn cleanPage(self: *App, allocator: std.mem.Allocator, page: templates.TemplatePage, route: []const u8, username: []const u8) !http.Response {
        // Triángulo del vault: al abrir el panel de notas se importa lo que
        // OpenCode/Hermes hayan escrito en el vault, sin pulsar el botón.
        if (page == .notes) {
            _ = notes.syncVaultCounts(allocator, self.db, self.config) catch |err| {
                std.log.warn("Auto-sync del vault falló: {s}", .{@errorName(err)});
            };
        }
        // Datos reales una sola vez: Mikareno y los contadores locales. Si el
        // MCP no responde, los huecos lo dicen en vez de inventar numeros.
        var datos = panel.fetch(allocator, self.db, self.config.mikareno_mcp_url, self.config.mcp_timeout_ms) catch panel.Datos{};
        datos.genoffice_connected = self.probeGenOffice(allocator);
        datos.ai_configured = self.config.openrouter_api_key != null;
        datos.ai_model = self.config.openrouter_model;
        datos.ai_max_tokens = self.config.openrouter_max_tokens;
        datos.ai_max_cost = self.config.openrouter_max_cost_usd;
        datos.mikareno_mcp_url = self.config.mikareno_mcp_url;
        datos.auth_disabled = self.config.auth_disabled;
        datos.vault_path = self.config.vault_dir;
        if (page == .mcp) {
            datos.mcp_tools = mcp_client.listTools(allocator, self.config.mikareno_mcp_url, self.config.mcp_timeout_ms, true) catch &.{};
        }
        const content = try templates.fillPage(allocator, page, templates.contentFor(page), self.config.openrouter_model, username, datos, self.db);
        return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, route, content, username, true, self.config.mikareno_mcp_url) };
    }

    fn partialDashboard(self: *App, allocator: std.mem.Allocator, partial: []const u8) ![]const u8 {
        if (std.mem.eql(u8, partial, "stats")) {
            const count = self.db.scalarInt("SELECT COUNT(*) FROM notes") catch 0;
            return std.fmt.allocPrint(allocator, "<div class=\"grid grid-cols-2 gap-3\"><div class=\"nx-panel p-4\"><b>{d}</b><p>Notas locales</p></div><div class=\"nx-panel p-4\"><b>0</b><p>Datos académicos sincronizados</p></div></div>", .{count});
        }
        if (std.mem.eql(u8, partial, "profile")) return "<div class=\"nx-panel p-4\">Perfil académico local</div>";
        if (std.mem.eql(u8, partial, "subjects")) return "<p>Sin asignaturas disponibles: Mikareno no está conectado.</p>";
        if (std.mem.eql(u8, partial, "events")) return "<p>Sin eventos sincronizados.</p>";
        if (std.mem.eql(u8, partial, "tasks")) return "<p>Sin tareas sincronizadas.</p>";
        if (std.mem.eql(u8, partial, "greeting")) return "Hola, estudiante";
        if (std.mem.eql(u8, partial, "nextclass")) return "<p>No se pudieron sincronizar clases.</p>";
        if (std.mem.eql(u8, partial, "agenda")) return "<p>Agenda sin datos externos.</p>";
        return "";
    }

    fn health(self: *App, allocator: std.mem.Allocator) !http.Response {
        const db_ok = self.db.check();
        const uptime = @max(0, std.time.timestamp() - self.started_at);
        const time_text = try timestampIso(allocator);
        const mikareno_ok = self.probeMikareno(allocator);
        const genoffice_ok = self.probeGenOffice(allocator);
        var components = std.json.ObjectMap.init(allocator);
        try components.put("database", .{ .bool = db_ok });
        try components.put("openrouter", .{ .bool = self.config.openrouter_api_key != null });
        try components.put("mikareno_mcp", .{ .bool = mikareno_ok });
        try components.put("genoffice_mcp", .{ .bool = genoffice_ok });
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("status", .{ .string = if (db_ok) "healthy" else "degraded" });
        try obj.put("timestamp", .{ .string = time_text });
        try obj.put("version", .{ .string = "0.2.0" });
        try obj.put("uptime_secs", .{ .integer = uptime });
        try obj.put("database", .{ .bool = db_ok });
        try obj.put("mikareno_mcp", .{ .bool = mikareno_ok });
        try obj.put("components", .{ .object = components });
        return jsonResponse(allocator, if (db_ok) 200 else 503, .{ .object = obj });
    }

    fn status(self: *App, allocator: std.mem.Allocator) !http.Response {
        const note_count = self.db.scalarInt("SELECT COUNT(*) FROM notes") catch 0;
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("status", .{ .string = if (self.db.check()) "ok" else "degraded" });
        try obj.put("database", .{ .bool = self.db.check() });
        try obj.put("notes_count", .{ .integer = note_count });
        try obj.put("version", .{ .string = "0.2.0" });
        return jsonResponse(allocator, 200, .{ .object = obj });
    }

    fn publicConfig(self: *App, allocator: std.mem.Allocator) !http.Response {
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("openrouter_configured", .{ .bool = self.config.openrouter_api_key != null });
        try obj.put("openrouter_model", .{ .string = self.config.openrouter_model });
        try obj.put("mikareno_mcp_url", .{ .string = self.config.mikareno_mcp_url });
        try obj.put("genoffice_mcp_url", .{ .string = self.config.genoffice_mcp_url });
        try obj.put("version", .{ .string = "0.2.0" });
        try obj.put("is_localhost", .{ .bool = std.mem.eql(u8, self.config.host, "127.0.0.1") });
        return jsonResponse(allocator, 200, .{ .object = obj });
    }

    fn handoff(self: *App, allocator: std.mem.Allocator) !http.Response {
        _ = self;
        const escaped = try security.htmlEscape(allocator, @embedFile("../STUDY_PLATFORM_HANDOFF.md"));
        const body = try std.fmt.allocPrint(allocator, "<!doctype html><html lang=\"es\"><meta charset=\"utf-8\"><title>Traspaso · NOVA</title><body><main style=\"max-width:960px;margin:2rem auto;font:15px system-ui\"><h1>Study Platform · Handoff</h1><pre style=\"white-space:pre-wrap\">{s}</pre></main></body></html>", .{escaped});
        return text(allocator, 200, "text/html; charset=utf-8", body);
    }

    fn probeMikareno(self: *App, allocator: std.mem.Allocator) bool {
        return mcp_client.probeCached(allocator, mcp_client.slot_mikareno, self.config.mikareno_mcp_url, @min(self.config.mcp_timeout_ms, 800), true);
    }

    fn probeGenOffice(self: *App, allocator: std.mem.Allocator) bool {
        return mcp_client.probeCached(allocator, mcp_client.slot_genoffice, self.config.genoffice_mcp_url, @min(self.config.mcp_timeout_ms, 800), false);
    }

    fn calendarPage(self: *App, allocator: std.mem.Allocator, request: *const http.Request, path: []const u8, username: []const u8, view: calendar_view.View, user_id: i64) !http.Response {
        const today = try panel.todayIso(allocator);
        var cur = calendar_view.cursorFromIso(allocator, today);
        if (try http.queryParam(allocator, request.query, "y")) |v| {
            if (std.fmt.parseInt(i32, v, 10)) |y| cur.year = y else |_| {}
        }
        if (try http.queryParam(allocator, request.query, "m")) |v| {
            if (std.fmt.parseInt(u32, v, 10)) |m| cur.month = m else |_| {}
        }
        if (try http.queryParam(allocator, request.query, "d")) |v| {
            if (std.fmt.parseInt(u32, v, 10)) |day| cur.day = day else |_| {}
        }
        cur = calendar_view.clampCursor(cur);

        const datos = panel.fetch(allocator, self.db, self.config.mikareno_mcp_url, self.config.mcp_timeout_ms) catch panel.Datos{};
        const range = try calendar_view.rangeFor(allocator, view, cur);
        const anots = calendar_view.loadAnnotations(allocator, self.db, range.from, range.to) catch &.{};
        const gcfg = self.googleConfig() orelse oauth_google.Config{
            .client_id = "",
            .client_secret = "",
            .redirect_uri = "",
            .timeout_ms = self.config.oauth_timeout_ms,
        };
        const gres = google_calendar.fetch(allocator, self.db, gcfg, user_id, 100) catch google_calendar.Resultado{
            .authorized = false,
            .reason = "No se pudo consultar Google Calendar.",
        };

        const content = try calendar_view.render(allocator, datos, anots, gres, view, cur, today);
        return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, path, content, username, true, self.config.mikareno_mcp_url) };
    }

    fn flashcardsPage(self: *App, allocator: std.mem.Allocator, path: []const u8, username: []const u8) !http.Response {
        const today = try panel.todayIso(allocator);
        const content = try flashcards.renderPage(allocator, self.db, today);
        return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, path, content, username, true, self.config.mikareno_mcp_url) };
    }

    fn calendarIcs(self: *App, allocator: std.mem.Allocator) !http.Response {
        const headers = try allocator.alloc(http.Header, 1);
        headers[0] = .{ .name = "Content-Disposition", .value = "attachment; filename=\"calendario.ics\"" };
        const datos = panel.fetch(allocator, self.db, self.config.mikareno_mcp_url, self.config.mcp_timeout_ms) catch panel.Datos{};
        const base = try notes.calendarIcs(allocator, self.db);
        const extra = try calendar_view.academicEventsIcs(allocator, datos);
        const body = try calendar_view.mergeIcs(allocator, base, extra);
        return .{ .status = 200, .content_type = "text/calendar; charset=utf-8", .body = body, .headers = headers };
    }

    /// Vista académica real de una ruta, con su herramienta de Mikareno.
    fn renderAcademicView(self: *App, allocator: std.mem.Allocator, view: AcademicView) ![]const u8 {
        var args = try json.object(allocator, &.{});
        if (view == .evaluaciones) args = try json.object(allocator, &.{.{ .key = "trimestre", .value = .{ .integer = 1 } }});
        const result = mcp_client.callTool(allocator, self.config.mikareno_mcp_url, view.timeoutMs(self.config.mcp_timeout_ms), view.tool(), args, true) catch {
            return templates.simplePage(allocator, view.title(), "<div class=\"nx-panel nx-academic-empty\"><span class=\"material-symbols-outlined\">cloud_off</span><p><strong>Mikareno no responde</strong><br>No se muestran datos académicos inventados. Reintenta en un momento.</p><a class=\"nx-button nx-button-secondary\" href=\"/dashboard/mcp\"><span class=\"material-symbols-outlined\">refresh</span>Revisar conexión</a></div>");
        };
        return switch (view) {
            .horario => academico.horarioHtml(allocator, result),
            .tareas => academico.tareasHtml(allocator, result),
            .evaluaciones => academico.examenesHtml(allocator, result),
            .asistencia => academico.asistenciaHtml(allocator, result),
            .perfil => academico.perfilHtml(allocator, result),
            .mensajes => academico.mensajesHtml(allocator, result),
        };
    }

    /// Búsqueda global (notas, Mikareno y Google) en una sola página.
    fn unifiedSearch(self: *App, allocator: std.mem.Allocator, request: *const http.Request, username: []const u8, user_id: i64) !http.Response {
        const q = (try http.queryParam(allocator, request.query, "q")) orelse "";
        if (q.len > 200) {
            const content = "<div class=\"nx-page nova-page\"><p>La consulta es demasiado larga.</p></div>";
            return .{ .status = 400, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, "/dashboard/search", content, username, true, self.config.mikareno_mcp_url) };
        }
        const content = busqueda.page(allocator, self.db, self.config, user_id, q) catch "<div class=\"nx-page nova-page\"><p>No se pudo ejecutar la búsqueda.</p></div>";
        return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, "/dashboard/search", content, username, true, self.config.mikareno_mcp_url) };
    }

    /// Página de Google Workspace: archivos de Drive y contactos de Contacts.
    fn googleWorkspacePage(self: *App, allocator: std.mem.Allocator, request: *const http.Request, path: []const u8, auth: Auth) !http.Response {
        const q = (try http.queryParam(allocator, request.query, "q")) orelse "";
        const doc_id = (try http.queryParam(allocator, request.query, "doc")) orelse "";
        const cfg = self.googleConfig() orelse oauth_google.Config{
            .client_id = "",
            .client_secret = "",
            .redirect_uri = "",
            .timeout_ms = self.config.oauth_timeout_ms,
        };
        var body = std.ArrayList(u8).init(allocator);
        body.appendSlice("<div class=\"nx-page nova-page\">") catch {};
        if (doc_id.len > 0) {
            const doc = google_workspace.readDocument(allocator, self.db, cfg, auth.user_id, doc_id) catch google_workspace.DocText{
                .reason = "No se pudo leer el documento.",
            };
            body.appendSlice(try google_workspace.docSectionHtml(allocator, doc)) catch {};
            body.appendSlice("</div>") catch {};
            return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, "/dashboard/google", body.items, auth.username, true, self.config.mikareno_mcp_url) };
        }
        body.appendSlice("<header class=\"nx-page-header nova-page-head\"><div><p class=\"nx-eyebrow\">Google</p><h1>Tu Google</h1><p>Busca en Drive y abre Docs, Sheets y Slides; tus contactos, con permiso de solo lectura.</p></div><div class=\"nx-button-row\"><a class=\"nx-button nx-button-secondary\" href=\"/dashboard/google-calendar\"><span class=\"material-symbols-outlined\">calendar_month</span>Calendario</a></div></header>") catch {};
        body.appendSlice("<form method=\"get\" action=\"/dashboard/google\" class=\"nx-search-form\"><span class=\"material-symbols-outlined\" aria-hidden=\"true\">search</span><input type=\"search\" name=\"q\" value=\"") catch {};
        body.appendSlice(try security.htmlEscape(allocator, q)) catch {};
        body.appendSlice("\" placeholder=\"Buscar en Google Drive…\" aria-label=\"Buscar en Google Drive\"><button type=\"submit\" class=\"nx-button\">Buscar</button></form>") catch {};
        const trimmed = std.mem.trim(u8, q, " \t\r\n");
        const drive = (if (trimmed.len > 0)
            google_workspace.driveSearch(allocator, self.db, cfg, auth.user_id, trimmed, 50)
        else
            google_workspace.driveFiles(allocator, self.db, cfg, auth.user_id, 50)) catch google_workspace.DriveResult{
            .authorized = false,
            .reason = "No se pudo consultar Google Drive.",
        };
        body.appendSlice(try google_workspace.driveSectionHtml(allocator, drive)) catch {};
        if (trimmed.len == 0) {
            const contacts = google_workspace.contacts(allocator, self.db, cfg, auth.user_id, 100) catch google_workspace.ContactsResult{
                .authorized = false,
                .reason = "No se pudo consultar Google Contacts.",
            };
            body.appendSlice(try google_workspace.contactsSectionHtml(allocator, contacts)) catch {};
        }
        body.appendSlice("</div>") catch {};
        return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, path, body.items, auth.username, true, self.config.mikareno_mcp_url) };
    }

    /// Página de Google Calendar: eventos reales del usuario, o estado vacío
    /// honesto si no ha autorizado el permiso de calendario.
    fn googleCalendarPage(self: *App, allocator: std.mem.Allocator, path: []const u8, auth: Auth) !http.Response {
        var body = std.ArrayList(u8).init(allocator);
        body.appendSlice("<div class=\"nx-page nova-page\">") catch {};
        body.appendSlice("<header class=\"nx-page-header nova-page-head\"><div><p class=\"nx-eyebrow\">Agenda</p><h1>Google Calendar</h1><p>Tus próximos eventos, leídos de Google con permiso de solo lectura.</p></div></header>") catch {};
        const cfg = self.googleConfig() orelse oauth_google.Config{
            .client_id = "",
            .client_secret = "",
            .redirect_uri = "",
            .timeout_ms = self.config.oauth_timeout_ms,
        };
        const result = google_calendar.fetch(allocator, self.db, cfg, auth.user_id, 50) catch google_calendar.Resultado{
            .authorized = false,
            .reason = "No se pudo consultar Google Calendar.",
        };
        const section = try google_calendar.sectionHtml(allocator, result);
        body.appendSlice(section) catch {};
        body.appendSlice("</div>") catch {};
        return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, path, body.items, auth.username, true, self.config.mikareno_mcp_url) };
    }

    fn academic(self: *App, allocator: std.mem.Allocator, request: *const http.Request, username: []const u8) !http.Response {
        const path = request.path;
        if (std.mem.eql(u8, path, "/dashboard/schedule")) {
            const result = mcp_client.callTool(allocator, self.config.mikareno_mcp_url, self.config.mcp_timeout_ms, "get_horario", try json.object(allocator, &.{}), true) catch {
                return jsonResponse(allocator, 200, try json.object(allocator, &.{
                    .{ .key = "connected", .value = .{ .bool = false } },
                    .{ .key = "schedule", .value = try json.array(allocator, &.{}) },
                    .{ .key = "error", .value = .{ .string = "Mikareno no disponible" } },
                }));
            };
            return jsonResponse(allocator, 200, try json.object(allocator, &.{
                .{ .key = "connected", .value = .{ .bool = true } },
                .{ .key = "schedule", .value = result },
            }));
        }
        if (std.mem.eql(u8, path, "/dashboard/subjects")) {
            // "Actualizar datos" pide un refresco explícito: se marca la copia
            // como vieja (sin borrarla) para forzar la llamada a Mikareno. Si
            // esa llamada falla, la copia sigue sirviendo de respaldo.
            if (try http.queryParam(allocator, request.query, "refresh")) |_| {
                self.db.exec("UPDATE mikareno_cache SET fetched_at=0 WHERE id=1") catch {};
            }
            const datos = panel.fetch(allocator, self.db, self.config.mikareno_mcp_url, self.config.mcp_timeout_ms) catch panel.Datos{};
            const body = try panel.gradesPageHtml(allocator, datos, self.config.mikareno_portal_url);
            return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, path, body, username, true, self.config.mikareno_mcp_url) };
        }
        // Horario: agenda de un día (clases + actividades, hora a hora).
        // La rejilla semanal queda como vista alternativa (?view=week). Es una
        // vista propia, separada del calendario; usa la misma caché que las notas.
        if (std.mem.eql(u8, path, "/dashboard/horario")) {
            const datos = panel.fetch(allocator, self.db, self.config.mikareno_mcp_url, self.config.mcp_timeout_ms) catch panel.Datos{};
            const today = try panel.todayIso(allocator);
            const view = try http.queryParam(allocator, request.query, "view");
            const day_q = try http.queryParam(allocator, request.query, "d");
            const iso = if (day_q) |v| (if (schedule_view.isValidDay(v)) v else today) else today;
            const want_week = view != null and std.mem.eql(u8, view.?, "week");
            const body = if (want_week)
                try schedule_view.render(allocator, datos, today)
            else
                try schedule_view.renderAgenda(allocator, datos, iso, today);
            return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, path, body, username, true, self.config.mikareno_mcp_url) };
        }
        // Google Calendar: eventos reales del usuario que entra con Google.
        if (std.mem.eql(u8, path, "/dashboard/google-calendar")) {
            if (try self.authFor(request, allocator)) |auth| {
                return self.googleCalendarPage(allocator, path, auth);
            }
            return redirect(allocator, "/login", 302);
        }
        // Navegador de Google: Drive + Contacts, solo lectura.
        if (std.mem.eql(u8, path, "/dashboard/google")) {
            if (try self.authFor(request, allocator)) |auth| {
                return self.googleWorkspacePage(allocator, request, path, auth);
            }
            return redirect(allocator, "/login", 302);
        }
        // Vistas académicas reales: cada ruta pide su herramienta y la
        // convierte en HTML. Si Mikareno no responde, la vista lo dice.
        if (academicView(path)) |view| {
            const body = try self.renderAcademicView(allocator, view);
            return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, path, body, username, true, self.config.mikareno_mcp_url) };
        }
        if (std.mem.startsWith(u8, path, "/dashboard/subjects/")) {
            const code = path["/dashboard/subjects/".len..];
            const body = if (code.len == 0 or code.len > 96 or std.mem.indexOfAny(u8, code, "/\\") != null)
                try panel.subjectMissingHtml(allocator, code)
            else blk: {
                const detail = panel.fetchSubjectDetail(allocator, self.config.mikareno_mcp_url, self.config.mcp_timeout_ms, code) catch panel.SubjectDetail{};
                if (detail.found) break :blk try panel.subjectDetailHtml(allocator, detail);
                break :blk try panel.subjectMissingHtml(allocator, code);
            };
            return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, path, body, username, true, self.config.mikareno_mcp_url) };
        }
        const title = if (std.mem.indexOf(u8, path, "calendar") != null) "Mi agenda" else if (std.mem.indexOf(u8, path, "planner") != null) "Planificador semanal" else if (std.mem.indexOf(u8, path, "horario") != null) "Horario" else if (std.mem.indexOf(u8, path, "subjects") != null) "Asignaturas" else if (std.mem.indexOf(u8, path, "examenes") != null) "Evaluaciones" else if (std.mem.indexOf(u8, path, "vida-escolar") != null) "Vida escolar" else "Espacio académico";
        const tool_name = academicTool(path);
        var args = try json.object(allocator, &.{});
        if (std.mem.startsWith(u8, path, "/dashboard/subjects/")) {
            const code = path["/dashboard/subjects/".len..];
            if (code.len > 0 and std.mem.indexOfAny(u8, code, "/\\") == null) args = try json.object(allocator, &.{.{ .key = "materia", .value = .{ .string = code } }});
        }
        const result = mcp_client.callTool(allocator, self.config.mikareno_mcp_url, self.config.mcp_timeout_ms, tool_name, args, true) catch {
            const body = try templates.simplePage(allocator, title, "<div class=\"rounded-xl border border-outline-variant/40 p-5\"><p role=\"status\">Mikareno no responde ahora o no está configurado. No se muestran datos académicos inventados. Intenta sincronizar más tarde.</p></div>");
            return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, path, body, username, true, self.config.mikareno_mcp_url) };
        };
        const pretty = try json.stringify(allocator, result);
        const escaped = try security.htmlEscape(allocator, pretty);
        const content = try std.fmt.allocPrint(allocator, "<div class=\"rounded-xl border border-outline-variant/40 p-5\"><p role=\"status\">Datos reales recibidos de Mikareno ({s}).</p><details><summary>Ver respuesta académica</summary><pre class=\"overflow-auto whitespace-pre-wrap\">{s}</pre></details></div>", .{ tool_name, escaped });
        const body = try templates.simplePage(allocator, title, content);
        return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, path, body, username, true, self.config.mikareno_mcp_url) };
    }

    fn office(self: *App, allocator: std.mem.Allocator, request: *const http.Request, username: []const u8) !http.Response {
        return office_mod.handle(allocator, self.db, self.config, request, username);
    }

    fn admin(self: *App, allocator: std.mem.Allocator, request: *const http.Request, username: []const u8) !http.Response {
        const path = request.path;
        if (std.mem.eql(u8, path, "/api/admin/metrics") or std.mem.eql(u8, path, "/admin/openrouter/budget")) {
            const spent = self.spentThisMonth() catch 0;
            var obj = std.json.ObjectMap.init(allocator);
            try obj.put("version", .{ .string = "0.2.0" });
            try obj.put("uptime_secs", .{ .integer = @max(0, std.time.timestamp() - self.started_at) });
            try obj.put("notes_count", .{ .integer = self.db.scalarInt("SELECT COUNT(*) FROM notes") catch 0 });
            try obj.put("chats_count", .{ .integer = self.db.scalarInt("SELECT COUNT(*) FROM chat_history") catch 0 });
            const mikareno_ok = self.probeMikareno(allocator);
            const genoffice_ok = self.probeGenOffice(allocator);
            try obj.put("mcp_connected", .{ .bool = mikareno_ok });
            try obj.put("mikareno_mcp", .{ .bool = mikareno_ok });
            try obj.put("genoffice_mcp", .{ .bool = genoffice_ok });
            const resources = system_metrics.collect(allocator, std.fs.path.dirname(self.config.db_path) orelse ".");
            try obj.put("ram_mb_used", .{ .integer = resources.ram_mb_used });
            try obj.put("ram_mb_total", .{ .integer = resources.ram_mb_total });
            try obj.put("disk_mb_free", .{ .integer = resources.disk_mb_free });
            try obj.put("db_bytes", .{ .integer = try fileSize(allocator, self.config.db_path) });
            try obj.put("openrouter_configured", .{ .bool = self.config.openrouter_api_key != null });
            try obj.put("spend_usd", .{ .float = spent });
            try obj.put("budget_usd", .{ .float = self.config.openrouter_max_cost_usd });
            try obj.put("available", .{ .bool = true });
            try obj.put("pct", .{ .float = if (self.config.openrouter_max_cost_usd > 0) spent / self.config.openrouter_max_cost_usd * 100 else 0 });
            try obj.put("alert", .{ .bool = self.config.openrouter_max_cost_usd > 0 and spent >= self.config.openrouter_max_cost_usd * 0.9 });
            return jsonResponse(allocator, 200, .{ .object = obj });
        }
        if (std.mem.eql(u8, path, "/admin/metrics")) {
            const resources = system_metrics.collect(allocator, std.fs.path.dirname(self.config.db_path) orelse ".");
            var obj = std.json.ObjectMap.init(allocator);
            try obj.put("uptime_secs", .{ .integer = @max(0, std.time.timestamp() - self.started_at) });
            try obj.put("ram_mb_used", .{ .integer = resources.ram_mb_used });
            try obj.put("ram_mb_total", .{ .integer = resources.ram_mb_total });
            try obj.put("disk_mb_free", .{ .integer = resources.disk_mb_free });
            try obj.put("db_bytes", .{ .integer = try fileSize(allocator, self.config.db_path) });
            return jsonResponse(allocator, 200, .{ .object = obj });
        }
        if (std.mem.eql(u8, path, "/admin/backup/list") or std.mem.eql(u8, path, "/api/admin/backup-status")) return self.backupList(allocator);
        if (std.mem.eql(u8, path, "/api/admin/backup") and std.mem.eql(u8, request.method, "GET")) return self.createBackup(allocator);
        if (std.mem.eql(u8, path, "/api/admin/backup") and std.mem.eql(u8, request.method, "POST")) return self.createBackup(allocator);
        if (std.mem.eql(u8, path, "/admin") or std.mem.eql(u8, path, "/admin/backup/list")) {
            const body = try templates.simplePage(allocator, "Administración", "<div class=\"grid gap-3\"><p>Estado del almacenamiento y uso de IA.</p><a href=\"/admin/backup/list\">Ver backups</a><form method=\"post\" action=\"/api/admin/backup\"><button>Crear backup</button></form></div>");
            return .{ .status = 200, .content_type = "text/html; charset=utf-8", .body = try templates.renderShell(allocator, "/admin", body, username, true, self.config.mikareno_mcp_url) };
        }
        if (std.mem.eql(u8, path, "/api/admin/backup/restore")) {
            if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
            return self.restoreBackup(allocator, request);
        }
        if (std.mem.startsWith(u8, path, "/api/admin/backup/download/")) {
            if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
            return self.downloadBackup(allocator, path["/api/admin/backup/download/".len..]);
        }
        return jsonError(allocator, 404, "not_found");
    }

    /// Run the same integrity-checked snapshot path from the daily worker.
    pub fn createScheduledBackup(self: *App) !void {
        const allocator = self.allocator;
        const dir = try self.backupDir(allocator);
        const stamp = try timestampForFile(allocator);
        const name = try std.fmt.allocPrint(allocator, "app-{s}.db", .{stamp});
        const path = try std.fs.path.join(allocator, &.{ dir, name });
        try self.db.backupTo(path);
        if (!try sqlite.Db.integrityCheckFile(allocator, path)) return error.InvalidBackup;
        try writeBackupSidecar(allocator, path, name);
        self.pruneBackups(allocator, dir);
    }

    fn backupDir(self: *App, allocator: std.mem.Allocator) ![]const u8 {
        const parent = std.fs.path.dirname(self.config.db_path) orelse ".";
        const path = try std.fs.path.join(allocator, &.{ parent, "backups" });
        try std.fs.cwd().makePath(path);
        return path;
    }

    fn createBackup(self: *App, allocator: std.mem.Allocator) !http.Response {
        const dir = try self.backupDir(allocator);
        const stamp = try timestampForFile(allocator);
        const name = try std.fmt.allocPrint(allocator, "app-{s}.db", .{stamp});
        const path = try std.fs.path.join(allocator, &.{ dir, name });
        self.db.backupTo(path) catch return jsonError(allocator, 500, "No se pudo crear el backup");
        const integrity_ok = sqlite.Db.integrityCheckFile(allocator, path) catch false;
        if (!integrity_ok) {
            std.fs.cwd().deleteFile(path) catch {};
            return jsonError(allocator, 500, "El backup no superó el control de integridad");
        }
        try writeBackupSidecar(allocator, path, name);
        self.pruneBackups(allocator, dir);
        const size = try fileSize(allocator, path);
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("success", .{ .bool = true });
        try obj.put("name", .{ .string = name });
        try obj.put("bytes", .{ .integer = size });
        try obj.put("path", .{ .string = path });
        try obj.put("size_bytes", .{ .integer = size });
        return jsonResponse(allocator, 200, .{ .object = obj });
    }

    fn pruneBackups(self: *App, allocator: std.mem.Allocator, dir_path: []const u8) void {
        _ = self;
        var dir = std.fs.cwd().openDir(dir_path, .{ .iterate = true }) catch return;
        defer dir.close();
        var names: std.ArrayListUnmanaged([]const u8) = .empty;
        var it = dir.iterate();
        while (it.next() catch return) |entry| {
            if (entry.kind != .file or !validBackupName(entry.name)) continue;
            names.append(allocator, allocator.dupe(u8, entry.name) catch return) catch return;
        }
        while (names.items.len > 5) {
            var oldest: usize = 0;
            for (1..names.items.len) |index| {
                if (std.mem.order(u8, names.items[index], names.items[oldest]) == .lt) oldest = index;
            }
            const name = names.orderedRemove(oldest);
            dir.deleteFile(name) catch {};
            const sidecar = std.fmt.allocPrint(allocator, "{s}.sha256", .{name}) catch continue;
            dir.deleteFile(sidecar) catch {};
        }
    }

    fn backupList(self: *App, allocator: std.mem.Allocator) !http.Response {
        const dir_path = try self.backupDir(allocator);
        var dir = std.fs.cwd().openDir(dir_path, .{ .iterate = true }) catch {
            return jsonResponse(allocator, 200, try json.object(allocator, &.{ .{ .key = "dir", .value = .{ .string = "data/backups" } }, .{ .key = "count", .value = .{ .integer = 0 } }, .{ .key = "items", .value = .{ .array = std.json.Array.init(allocator) } }, .{ .key = "last_backup", .value = .null } }));
        };
        defer dir.close();
        var names: std.ArrayListUnmanaged([]const u8) = .empty;
        var it = dir.iterate();
        while (try it.next()) |entry| {
            if (entry.kind == .file and validBackupName(entry.name)) try names.append(allocator, try allocator.dupe(u8, entry.name));
        }
        std.mem.sort([]const u8, names.items, {}, backupNameDesc);
        var items = std.json.Array.init(allocator);
        var last_backup: std.json.Value = .null;
        for (names.items, 0..) |name, index| {
            const path = try std.fs.path.join(allocator, &.{ dir_path, name });
            const file = try std.fs.cwd().openFile(path, .{});
            defer file.close();
            const stat = try file.stat();
            const size: i64 = @intCast(@min(stat.size, std.math.maxInt(i64)));
            const created = timestampIsoAt(allocator, @divFloor(stat.mtime, std.time.ns_per_s)) catch null;
            var item = std.json.ObjectMap.init(allocator);
            try item.put("nombre", .{ .string = name });
            try item.put("name", .{ .string = name });
            try item.put("bytes", .{ .integer = size });
            try item.put("created", if (created) |value| .{ .string = value } else .null);
            try item.put("created_at", if (created) |value| .{ .string = value } else .null);
            try item.put("created_es", if (created) |value| .{ .string = value } else .null);
            try items.append(.{ .object = item });
            if (index == 0) {
                last_backup = try json.object(allocator, &.{
                    .{ .key = "name", .value = .{ .string = name } },
                    .{ .key = "path", .value = .{ .string = path } },
                    .{ .key = "size_bytes", .value = .{ .integer = size } },
                    .{ .key = "created_at", .value = if (created) |value| .{ .string = value } else .null },
                });
            }
        }
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("dir", .{ .string = "data/backups" });
        try obj.put("count", .{ .integer = @intCast(items.items.len) });
        try obj.put("items", .{ .array = items });
        try obj.put("last_backup", last_backup);
        return jsonResponse(allocator, 200, .{ .object = obj });
    }

    fn restoreBackup(self: *App, allocator: std.mem.Allocator, request: *const http.Request) !http.Response {
        const payload = json.parse(allocator, request.body) catch return jsonError(allocator, 400, "Invalid JSON");
        const name = json.getString(payload, "name") orelse return jsonError(allocator, 400, "nombre de backup no válido");
        if (!validBackupName(name)) return jsonError(allocator, 400, "nombre de backup no válido");
        const dir = try self.backupDir(allocator);
        const path = try std.fs.path.join(allocator, &.{ dir, name });
        const file = std.fs.cwd().openFile(path, .{}) catch return jsonError(allocator, 404, "backup no encontrado");
        file.close();
        const digest_ok = verifyBackupSidecar(allocator, path) catch false;
        if (!digest_ok) return jsonError(allocator, 422, "El SHA-256 del backup no coincide");
        const integrity_ok = sqlite.Db.integrityCheckFile(allocator, path) catch false;
        if (!integrity_ok) return jsonError(allocator, 422, "el backup no supera el control de integridad");
        self.db.restoreFrom(path) catch return jsonError(allocator, 500, "No se pudo restaurar el backup; reintenta tras detener la app");
        return jsonResponse(allocator, 200, try json.object(allocator, &.{
            .{ .key = "success", .value = .{ .bool = true } },
            .{ .key = "name", .value = .{ .string = name } },
            .{ .key = "bytes", .value = .{ .integer = try fileSize(allocator, path) } },
        }));
    }

    fn downloadBackup(self: *App, allocator: std.mem.Allocator, name: []const u8) !http.Response {
        if (!validBackupName(name)) return jsonError(allocator, 404, "backup no encontrado");
        const dir = try self.backupDir(allocator);
        const path = try std.fs.path.join(allocator, &.{ dir, name });
        const file = std.fs.cwd().openFile(path, .{}) catch return jsonError(allocator, 404, "backup no encontrado");
        defer file.close();
        const body = try file.readToEndAlloc(allocator, 256 * 1024 * 1024);
        const headers = try allocator.alloc(http.Header, 1);
        headers[0] = .{ .name = "Content-Disposition", .value = try std.fmt.allocPrint(allocator, "attachment; filename=\"{s}\"", .{name}) };
        return .{ .status = 200, .content_type = "application/vnd.sqlite3", .body = body, .headers = headers };
    }

    fn spentThisMonth(self: *App) !f64 {
        var stmt = try self.db.prepare("SELECT COALESCE(SUM(cost_usd),0) FROM usage_metrics WHERE created_at>=date('now','start of month')");
        defer stmt.deinit();
        if (!try stmt.next()) return 0;
        return stmt.float64(0);
    }

    fn rateLimitedDummy(_: *App) void {}
};

pub const Auth = struct { user_id: i64, username: []const u8, session_id: []const u8 };
/// Cubo general para rutas dinamicas (paginas, HTMX y API).
/// Vistas académicas con datos reales. Cada una nombra su herramienta.
const AcademicView = enum {
    horario,
    tareas,
    evaluaciones,
    asistencia,
    perfil,
    mensajes,

    fn tool(self: AcademicView) []const u8 {
        return switch (self) {
            .horario => "get_horario",
            .tareas => "get_tareas",
            .evaluaciones => "get_pendientes_calificacion",
            .asistencia => "get_asistencia",
            .perfil => "get_perfil",
            .mensajes => "get_mensajes",
        };
    }

    fn title(self: AcademicView) []const u8 {
        return switch (self) {
            .horario => "Horario",
            .tareas => "Planificador",
            .evaluaciones => "Evaluaciones",
            .asistencia => "Asistencia",
            .perfil => "Perfil",
            .mensajes => "Mensajes",
        };
    }

    /// La bandeja de entrada es la más lenta de Mikareno: se le da margen
    /// para no mostrar un falso "no responde".
    fn timeoutMs(self: AcademicView, base_ms: u64) u64 {
        return switch (self) {
            .mensajes => @max(base_ms, 20000),
            else => base_ms,
        };
    }
};

fn academicView(path: []const u8) ?AcademicView {
    if (std.mem.eql(u8, path, "/dashboard/horario")) return .horario;
    if (std.mem.eql(u8, path, "/dashboard/planner")) return .tareas;
    if (std.mem.eql(u8, path, "/dashboard/examenes")) return .evaluaciones;
    if (std.mem.eql(u8, path, "/dashboard/vida-escolar")) return .asistencia;
    if (std.mem.eql(u8, path, "/dashboard/perfil") or std.mem.eql(u8, path, "/dashboard/profile")) return .perfil;
    if (std.mem.eql(u8, path, "/dashboard/mensajes")) return .mensajes;
    return null;
}

const GENERAL_RATE_LIMIT: u32 = 240;

const RateBucket = struct { window_start: i64, count: u32 };

fn academicTool(path: []const u8) []const u8 {
    if (std.mem.indexOf(u8, path, "horario") != null or std.mem.eql(u8, path, "/dashboard/schedule")) return "get_horario";
    if (std.mem.indexOf(u8, path, "planner") != null) return "get_tareas";
    if (std.mem.indexOf(u8, path, "calendar") != null) return "get_calendario";
    if (std.mem.indexOf(u8, path, "profile") != null) return "get_perfil";
    if (std.mem.startsWith(u8, path, "/dashboard/subjects/")) return "get_detalle_materia";
    if (std.mem.indexOf(u8, path, "subjects") != null or std.mem.indexOf(u8, path, "examenes") != null) return "get_student_full";
    if (std.mem.indexOf(u8, path, "vida-escolar") != null) return "get_asistencia";
    return "get_student_full";
}

fn writeBackupSidecar(allocator: std.mem.Allocator, path: []const u8, name: []const u8) !void {
    const digest = try sha256File(allocator, path);
    const sidecar_path = try std.fmt.allocPrint(allocator, "{s}.sha256", .{path});
    const sidecar = try std.fs.cwd().createFile(sidecar_path, .{ .truncate = true });
    defer sidecar.close();
    try sidecar.writer().print("{s}  {s}\nPRAGMA integrity_check: ok\n", .{ &digest, name });
}

fn verifyBackupSidecar(allocator: std.mem.Allocator, path: []const u8) !bool {
    const sidecar_path = try std.fmt.allocPrint(allocator, "{s}.sha256", .{path});
    const content = readOptionalFile(allocator, sidecar_path) catch return true; // legacy backups had no digest sidecar.
    const trimmed = std.mem.trim(u8, content, " \t\r\n");
    if (trimmed.len < 64) return true; // legacy integrity-only marker.
    for (trimmed[0..64]) |byte| if (!std.ascii.isHex(byte)) return true;
    const expected = trimmed[0..64];
    const actual = try sha256File(allocator, path);
    return security.constantTimeEq(expected, &actual);
}

fn sha256File(allocator: std.mem.Allocator, path: []const u8) ![64]u8 {
    const file = try std.fs.cwd().openFile(path, .{});
    defer file.close();
    var hash = std.crypto.hash.sha2.Sha256.init(.{});
    var buffer: [64 * 1024]u8 = undefined;
    while (true) {
        const amount = try file.read(&buffer);
        if (amount == 0) break;
        hash.update(buffer[0..amount]);
    }
    var bytes: [32]u8 = undefined;
    hash.final(&bytes);
    const alphabet = "0123456789abcdef";
    var hex: [64]u8 = undefined;
    for (bytes, 0..) |byte, index| {
        hex[index * 2] = alphabet[byte >> 4];
        hex[index * 2 + 1] = alphabet[byte & 0x0f];
    }
    _ = allocator;
    return hex;
}

fn readOptionalFile(allocator: std.mem.Allocator, path: []const u8) ![]const u8 {
    const file = try std.fs.cwd().openFile(path, .{});
    defer file.close();
    return file.readToEndAlloc(allocator, 4096);
}

fn validBackupName(name: []const u8) bool {
    if (!std.mem.startsWith(u8, name, "app-") or !std.mem.endsWith(u8, name, ".db") or name.len > 80) return false;
    if (std.mem.indexOfAny(u8, name, "/\\\x00") != null or std.mem.indexOf(u8, name, "..") != null) return false;
    for (name) |ch| if (!(std.ascii.isAlphanumeric(ch) or ch == '-' or ch == '.')) return false;
    return name.len > "app-.db".len;
}

fn backupNameDesc(_: void, left: []const u8, right: []const u8) bool {
    return std.mem.order(u8, left, right) == .gt;
}

fn cookieToken(cookie: []const u8) ?[]const u8 {
    var parts = std.mem.splitScalar(u8, cookie, ';');
    while (parts.next()) |part| {
        const segment = std.mem.trim(u8, part, " \t");
        if (std.mem.startsWith(u8, segment, "session_id=")) return segment[11..];
    }
    return null;
}

fn bearerToken(header: ?[]const u8) ?[]const u8 {
    const raw = header orelse return null;
    if (!std.ascii.startsWithIgnoreCase(raw, "Bearer ")) return null;
    const value = std.mem.trim(u8, raw[7..], " \t");
    return if (value.len == 0) null else value;
}

fn originAllowed(request: *const http.Request) bool {
    const origin = request.header("origin") orelse request.header("referer") orelse return true;
    const host = request.header("host") orelse return true;
    const authority_start = if (std.mem.indexOf(u8, origin, "://")) |i| i + 3 else 0;
    const remainder = origin[authority_start..];
    const authority_end = std.mem.indexOfAny(u8, remainder, "/?#") orelse remainder.len;
    return std.ascii.eqlIgnoreCase(remainder[0..authority_end], host);
}

fn requestSecure(request: *const http.Request) bool {
    const proto = request.header("x-forwarded-proto") orelse return false;
    const first = std.mem.trim(u8, std.mem.sliceTo(proto, ','), " \t");
    return std.ascii.eqlIgnoreCase(first, "https");
}

fn timestampIso(allocator: std.mem.Allocator) ![]const u8 {
    return timestampIsoAt(allocator, std.time.timestamp());
}

fn timestampIsoAt(allocator: std.mem.Allocator, seconds: i128) ![]const u8 {
    const epoch_seconds = std.time.epoch.EpochSeconds{ .secs = @intCast(@max(@as(i128, 0), seconds)) };
    const epoch_day = epoch_seconds.getEpochDay();
    const year_day = epoch_day.calculateYearDay();
    const month_day = year_day.calculateMonthDay();
    const day_seconds = epoch_seconds.getDaySeconds();
    return std.fmt.allocPrint(allocator, "{d:0>4}-{d:0>2}-{d:0>2}T{d:0>2}:{d:0>2}:{d:0>2}Z", .{ year_day.year, month_day.month.numeric(), month_day.day_index + 1, day_seconds.getHoursIntoDay(), day_seconds.getMinutesIntoHour(), day_seconds.getSecondsIntoMinute() });
}

fn timestampForFile(allocator: std.mem.Allocator) ![]const u8 {
    const iso = try timestampIso(allocator);
    var buf: std.ArrayListUnmanaged(u8) = .empty;
    for (iso) |ch| if (std.ascii.isDigit(ch)) try buf.append(allocator, ch);
    if (buf.items.len < 14) return allocator.dupe(u8, "19700101-000000");
    return std.fmt.allocPrint(allocator, "{s}-{s}", .{ buf.items[0..8], buf.items[8..14] });
}

fn fileSize(allocator: std.mem.Allocator, path: []const u8) !i64 {
    _ = allocator;
    const file = try std.fs.cwd().openFile(path, .{});
    defer file.close();
    const stat = try file.stat();
    return @intCast(stat.size);
}

fn text(allocator: std.mem.Allocator, status: u16, content_type: []const u8, body: []const u8) http.Response {
    _ = allocator;
    return .{ .status = status, .content_type = content_type, .body = body };
}

fn jsonResponse(allocator: std.mem.Allocator, status: u16, value: std.json.Value) !http.Response {
    return .{ .status = status, .content_type = "application/json; charset=utf-8", .body = try json.stringify(allocator, value) };
}

fn jsonError(allocator: std.mem.Allocator, status: u16, message: []const u8) !http.Response {
    return jsonResponse(allocator, status, try json.object(allocator, &.{.{ .key = "error", .value = .{ .string = message } }}));
}

fn redirect(allocator: std.mem.Allocator, location: []const u8, status: u16) !http.Response {
    const headers = try allocator.alloc(http.Header, 1);
    headers[0] = .{ .name = "Location", .value = location };
    return .{ .status = status, .content_type = "text/plain; charset=utf-8", .body = "", .headers = headers };
}

fn methodNotAllowed(allocator: std.mem.Allocator) !http.Response {
    return jsonError(allocator, 405, "Method not allowed");
}
