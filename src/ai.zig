const std = @import("std");
const http = @import("http.zig");
const sqlite = @import("sqlite.zig");
const config_mod = @import("config.zig");
const json = @import("jsonutil.zig");
const security = @import("security.zig");

const MAX_MESSAGE: usize = 8000;
const MAX_CONTENT: usize = 20000;
const MAX_SUBJECT: usize = 200;
const OPENROUTER_URL = "https://openrouter.ai/api/v1/chat/completions";

const ProviderResult = struct { content: []const u8, input_tokens: i64 = 0, output_tokens: i64 = 0, cost: f64 = 0 };
const Source = struct { title: []const u8, slug: []const u8, content: []const u8 };
const ChatMessage = struct { role: []const u8, content: []const u8 };

pub fn handle(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request, active_streams: *std.atomic.Value(u32)) !http.Response {
    const path = request.path;
    if (std.mem.eql(u8, path, "/api/ai/cost") and std.mem.eql(u8, request.method, "GET")) return cost(allocator, db, config);
    if (std.mem.eql(u8, path, "/api/ai/models") and std.mem.eql(u8, request.method, "GET")) return models(allocator, db, config);
    if (std.mem.eql(u8, path, "/api/ai/model")) return model(allocator, db, config, request);
    if (std.mem.eql(u8, path, "/api/ai/conversations") and std.mem.eql(u8, request.method, "GET")) return conversations(allocator, db);
    if (std.mem.startsWith(u8, path, "/api/ai/conversations/")) return conversationMessages(allocator, db, request);
    if (std.mem.eql(u8, path, "/api/ai/chat") and std.mem.eql(u8, request.method, "POST")) return chat(allocator, db, config, request, false, active_streams);
    if (std.mem.eql(u8, path, "/api/ai/chat/stream") and std.mem.eql(u8, request.method, "POST")) return chat(allocator, db, config, request, true, active_streams);
    if (std.mem.eql(u8, path, "/api/ai/summary") and std.mem.eql(u8, request.method, "POST")) return generate(allocator, db, config, request, "summary");
    if (std.mem.eql(u8, path, "/api/ai/quiz") and std.mem.eql(u8, request.method, "POST")) return generate(allocator, db, config, request, "quiz");
    if (std.mem.eql(u8, path, "/api/ai/study-sheet") and std.mem.eql(u8, request.method, "POST")) return generate(allocator, db, config, request, "study-sheet");
    if (std.mem.eql(u8, path, "/dashboard/material") and std.mem.eql(u8, request.method, "POST")) return material(allocator, db, config, request);
    return jsonError(allocator, 404, "not_found");
}

fn chat(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request, streaming: bool, active_streams: *std.atomic.Value(u32)) !http.Response {
    if (streaming) {
        const active = active_streams.fetchAdd(1, .monotonic);
        if (active >= 4) {
            _ = active_streams.fetchSub(1, .monotonic);
            return jsonError(allocator, 429, "Demasiados streams activos");
        }
        defer _ = active_streams.fetchSub(1, .monotonic);
    }
    const input = parseBody(allocator, request) catch return jsonError(allocator, 400, "Invalid JSON");
    const message = json.getString(input, "message") orelse return jsonError(allocator, 400, if (streaming) "Invalid request: message is required." else "Invalid request: message is required.");
    if (message.len == 0 or message.len > MAX_MESSAGE) return jsonError(allocator, 400, "Invalid request: message must be 1..=8000 characters.");
    const subject_raw = json.getString(input, "subject") orelse "general";
    if (subject_raw.len > MAX_SUBJECT) return jsonError(allocator, 400, "Invalid request: subject must be at most 200 characters.");
    const subject = if (std.mem.trim(u8, subject_raw, " \t\r\n").len == 0) "general" else std.mem.trim(u8, subject_raw, " \t\r\n");
    const mode = json.getString(input, "mode") orelse "general";
    if (!validMode(mode)) return jsonError(allocator, 422, "Invalid retrieval mode");
    const context_input = json.getString(input, "content") orelse "";
    if (context_input.len > MAX_CONTENT) return jsonError(allocator, 400, "Invalid request: content must be at most 20000 characters.");
    if (try budgetExhausted(allocator, db, config)) |resp| return resp;

    var sources: std.ArrayListUnmanaged(Source) = .empty;
    var context: std.ArrayListUnmanaged(u8) = .empty;
    if (std.mem.eql(u8, mode, "notes") or std.mem.eql(u8, mode, "hybrid")) {
        sources = try retrieveNotes(allocator, db, message);
        for (sources.items) |source| {
            try context.appendSlice(allocator, try std.fmt.allocPrint(allocator, "\n\n[Nota: {s}]\n{s}", .{ source.title, source.content }));
        }
    }
    const system = if (std.mem.eql(u8, mode, "notes")) "Responde en español usando solamente el contexto de notas locales proporcionado. Si no alcanza, dilo con claridad; no inventes fuentes." else "Eres NOVA, un asistente académico. Responde de forma clara y rigurosa; no inventes información ni citas.";
    const prompt = try std.fmt.allocPrint(allocator, "{s}{s}{s}{s}", .{ system, if (context.items.len > 0) "\n\nContexto recuperado:" else "", context.items, if (context_input.len > 0) try std.fmt.allocPrint(allocator, "\n\nMaterial del usuario:\n{s}", .{context_input}) else "" });
    const selected_model = currentModel(allocator, db, config) catch config.openrouter_model;
    const history = loadChatHistory(allocator, db, subject) catch &.{};
    var result: ProviderResult = undefined;
    if (config.openrouter_api_key) |key| {
        result = callProvider(allocator, config, key, selected_model, prompt, message, history) catch return jsonError(allocator, 502, "Error al consultar el proveedor de IA");
    } else {
        result = .{ .content = "[Modo simulado] Configura OPENROUTER_API_KEY para obtener una respuesta generada." };
    }
    persistChat(db, allocator, subject, message, result.content) catch {};
    recordUsage(db, config, selected_model, "chat", result) catch {};
    const mode_present = if (input == .object) input.object.contains("mode") else false;
    if (streaming) return sseResponse(allocator, result, mode, sources.items, mode_present);
    const accept = request.header("accept") orelse "";
    if (std.mem.indexOf(u8, accept, "application/json") != null) {
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("content", .{ .string = result.content });
        try obj.put("mode", .{ .string = mode });
        try obj.put("sources", sourceValues(allocator, sources.items));
        return jsonResponse(allocator, 200, .{ .object = obj });
    }
    return textResponse(200, "text/plain; charset=utf-8", result.content);
}

fn generate(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request, kind: []const u8) !http.Response {
    const input = parseBody(allocator, request) catch return jsonError(allocator, 400, "Invalid JSON");
    const content = json.getString(input, "content") orelse return jsonError(allocator, 400, "Invalid request: content must be 1..=20000 characters.");
    if (content.len == 0 or content.len > MAX_CONTENT) return jsonError(allocator, 400, "Invalid request: content must be 1..=20000 characters.");
    if (try budgetExhausted(allocator, db, config)) |resp| return resp;
    if (config.openrouter_api_key == null) {
        if (std.mem.eql(u8, kind, "summary")) return jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "summary", .value = .{ .string = "[Modo simulado] No se generó un resumen. Configura OPENROUTER_API_KEY para activar NOVA AI." } }}));
        if (std.mem.eql(u8, kind, "study-sheet")) return jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "study_sheet", .value = .{ .string = "[Modo simulado] No se generó material. Configura OPENROUTER_API_KEY para activar NOVA AI." } }}));
        var empty = std.json.ObjectMap.init(allocator);
        try empty.put("title", .{ .string = "Modo simulado" });
        try empty.put("questions", .{ .array = std.json.Array.init(allocator) });
        return jsonResponse(allocator, 200, .{ .object = empty });
    }
    const system = if (std.mem.eql(u8, kind, "summary")) "Resume el material en español en ideas clave." else if (std.mem.eql(u8, kind, "study-sheet")) "Crea una ficha de estudio en Markdown, en español." else "Crea un cuestionario de opción múltiple en JSON.";
    const selected_model = currentModel(allocator, db, config) catch config.openrouter_model;
    const result = callProvider(allocator, config, config.openrouter_api_key.?, selected_model, system, content, &.{}) catch return jsonError(allocator, 502, "Error al consultar el proveedor de IA");
    recordUsage(db, config, selected_model, kind, result) catch {};
    if (std.mem.eql(u8, kind, "summary")) return jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "summary", .value = .{ .string = result.content } }}));
    if (std.mem.eql(u8, kind, "study-sheet")) return jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "study_sheet", .value = .{ .string = result.content } }}));
    var obj = json.parse(allocator, result.content) catch {
        return jsonResponse(allocator, 200, try json.object(allocator, &.{ .{ .key = "title", .value = .{ .string = "Cuestionario" } }, .{ .key = "questions", .value = .{ .array = std.json.Array.init(allocator) } } }));
    };
    const requested = json.getInt(input, "num_questions") orelse 5;
    const limit = @min(10, @max(1, requested));
    if (obj == .object) {
        if (obj.object.getPtr("questions")) |questions| {
            if (questions.* == .array and questions.array.items.len > @as(usize, @intCast(limit))) questions.array.items.len = @intCast(limit);
        }
    }
    return jsonResponse(allocator, 200, obj);
}

fn material(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request) !http.Response {
    const input = parseBody(allocator, request) catch return jsonError(allocator, 400, "Invalid JSON");
    const subject = json.getString(input, "materia") orelse return jsonError(allocator, 400, "Falta la materia");
    const topic = json.getString(input, "tema") orelse return jsonError(allocator, 400, "Falta el tema");
    const kind = json.getString(input, "tipo") orelse return jsonError(allocator, 400, "Falta el tipo de material");
    if (subject.len > 200 or topic.len > 400 or kind.len > 40) return jsonError(allocator, 400, "Parámetros demasiado largos");
    if (config.openrouter_api_key == null) {
        var obj = std.json.ObjectMap.init(allocator);
        try obj.put("ok", .{ .bool = false });
        try obj.put("error", .{ .string = "Modo demo: configura OPENROUTER_API_KEY" });
        return jsonResponse(allocator, 200, .{ .object = obj });
    }
    if (try budgetExhausted(allocator, db, config)) |resp| return resp;
    const prompt = try std.fmt.allocPrint(allocator, "Prepara un cuestionario de opción múltiple para {s} sobre {s}.", .{ subject, topic });
    const selected_model = currentModel(allocator, db, config) catch config.openrouter_model;
    const result = callProvider(allocator, config, config.openrouter_api_key.?, selected_model, "Responde en español y genera contenido educativo estructurado.", prompt, &.{}) catch return jsonError(allocator, 502, "Error al generar material");
    recordUsage(db, config, selected_model, "material", result) catch {};
    var obj = std.json.ObjectMap.init(allocator);
    try obj.put("ok", .{ .bool = true });
    try obj.put("tipo", .{ .string = kind });
    try obj.put("materia", .{ .string = subject });
    try obj.put("tema", .{ .string = topic });
    try obj.put("material", .{ .string = result.content });
    return jsonResponse(allocator, 200, .{ .object = obj });
}

fn cost(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config) !http.Response {
    var stmt = try db.prepare("SELECT COALESCE(SUM(cost_usd),0),COUNT(*) FROM usage_metrics WHERE created_at>=date('now','start of month')");
    defer stmt.deinit();
    _ = try stmt.next();
    const spent = stmt.float64(0);
    const count = stmt.int64(1);
    var obj = std.json.ObjectMap.init(allocator);
    try obj.put("total_cost", .{ .float = spent });
    try obj.put("budget_limit", .{ .float = config.openrouter_max_cost_usd });
    try obj.put("requests_count", .{ .integer = count });
    try obj.put("alert", .{ .bool = config.openrouter_max_cost_usd > 0 and spent >= config.openrouter_max_cost_usd * 0.9 });
    return jsonResponse(allocator, 200, .{ .object = obj });
}

pub const model_catalog = [_][]const u8{
    "cohere/north-mini-code:free",
    "minimax/minimax-m3.1-flash:free",
    "google/gemini-2.0-flash-exp:free",
    "meta-llama/llama-3.3-70b-instruct:free",
    "openai/gpt-4o-mini",
};

fn models(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config) !http.Response {
    const selected = currentModel(allocator, db, config) catch config.openrouter_model;
    const entries = model_catalog;
    var list = std.json.Array.init(allocator);
    for (entries) |id| {
        var item = std.json.ObjectMap.init(allocator);
        try item.put("id", .{ .string = id });
        try item.put("name", .{ .string = id });
        try item.put("free", .{ .bool = std.mem.endsWith(u8, id, ":free") });
        try item.put("selected", .{ .bool = std.mem.eql(u8, id, selected) });
        try list.append(.{ .object = item });
    }
    var obj = std.json.ObjectMap.init(allocator);
    try obj.put("models", .{ .array = list });
    try obj.put("selected", .{ .string = selected });
    try obj.put("active", .{ .string = selected });
    try obj.put("openrouter_configured", .{ .bool = config.openrouter_api_key != null });
    return jsonResponse(allocator, 200, .{ .object = obj });
}

fn model(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request) !http.Response {
    if (std.mem.eql(u8, request.method, "GET")) return jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "model", .value = .{ .string = try currentModel(allocator, db, config) } }}));
    if (!std.mem.eql(u8, request.method, "PUT")) return jsonError(allocator, 405, "Method not allowed");
    const input = parseBody(allocator, request) catch return jsonError(allocator, 400, "Invalid JSON");
    const selected = json.getString(input, "model") orelse return jsonError(allocator, 400, "Invalid model");
    if (!validModel(selected)) return jsonError(allocator, 400, "Invalid model");
    var stmt = try db.prepare("INSERT INTO app_config(key,value,updated_at) VALUES('ai.model',?1,datetime('now')) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=datetime('now')");
    defer stmt.deinit();
    try stmt.bindText(1, selected);
    _ = try stmt.next();
    return jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "model", .value = .{ .string = selected } }}));
}

fn currentModel(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config) ![]const u8 {
    var stmt = try db.prepare("SELECT value FROM app_config WHERE key='ai.model'");
    defer stmt.deinit();
    if (try stmt.next()) return allocator.dupe(u8, stmt.text(0));
    return allocator.dupe(u8, config.openrouter_model);
}

fn conversations(allocator: std.mem.Allocator, db: *sqlite.Db) !http.Response {
    var stmt = try db.prepare("SELECT subject,COALESCE((SELECT content FROM chat_history c2 WHERE c2.subject=c.subject ORDER BY id DESC LIMIT 1),''),COUNT(*),MAX(created_at) FROM chat_history c GROUP BY subject ORDER BY MAX(created_at) DESC LIMIT 50");
    defer stmt.deinit();
    var list = std.json.Array.init(allocator);
    while (try stmt.next()) {
        var item = std.json.ObjectMap.init(allocator);
        try item.put("subject", .{ .string = try allocator.dupe(u8, stmt.text(0)) });
        try item.put("last_message", .{ .string = try allocator.dupe(u8, stmt.text(1)) });
        try item.put("message_count", .{ .integer = stmt.int64(2) });
        try item.put("updated_at", .{ .string = try allocator.dupe(u8, stmt.text(3)) });
        try list.append(.{ .object = item });
    }
    return jsonResponse(allocator, 200, try json.object(allocator, &.{.{ .key = "conversations", .value = .{ .array = list } }}));
}

fn conversationMessages(allocator: std.mem.Allocator, db: *sqlite.Db, request: *const http.Request) !http.Response {
    if (!std.mem.eql(u8, request.method, "GET")) return jsonError(allocator, 405, "Method not allowed");
    const rest = request.path["/api/ai/conversations/".len..];
    const suffix = "/messages";
    if (!std.mem.endsWith(u8, rest, suffix)) return jsonError(allocator, 404, "not_found");
    const subject_enc = rest[0 .. rest.len - suffix.len];
    const subject = try http.percentDecode(allocator, subject_enc, false);
    if (subject.len == 0 or subject.len > MAX_SUBJECT or std.mem.indexOfAny(u8, subject, "/\\") != null or std.mem.indexOf(u8, subject, "..") != null) return jsonError(allocator, 400, "Invalid subject");
    var stmt = try db.prepare("SELECT role,content,created_at FROM chat_history WHERE subject=?1 AND role IN ('user','assistant') ORDER BY id ASC");
    defer stmt.deinit();
    try stmt.bindText(1, subject);
    var messages = std.json.Array.init(allocator);
    while (try stmt.next()) {
        var item = std.json.ObjectMap.init(allocator);
        try item.put("role", .{ .string = try allocator.dupe(u8, stmt.text(0)) });
        try item.put("content", .{ .string = try allocator.dupe(u8, stmt.text(1)) });
        try item.put("created_at", .{ .string = try allocator.dupe(u8, stmt.text(2)) });
        try messages.append(.{ .object = item });
    }
    return jsonResponse(allocator, 200, try json.object(allocator, &.{ .{ .key = "subject", .value = .{ .string = subject } }, .{ .key = "messages", .value = .{ .array = messages } } }));
}

fn budgetExhausted(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config) !?http.Response {
    if (config.openrouter_api_key == null) return null;
    var stmt = try db.prepare("SELECT COALESCE(SUM(cost_usd),0) FROM usage_metrics WHERE created_at>=date('now','start of month')");
    defer stmt.deinit();
    _ = try stmt.next();
    const spent = stmt.float64(0);
    if (spent < config.openrouter_max_cost_usd) return null;
    var obj = std.json.ObjectMap.init(allocator);
    try obj.put("error", .{ .string = "Presupuesto de IA agotado para este mes" });
    try obj.put("budget_usd", .{ .float = config.openrouter_max_cost_usd });
    try obj.put("spent_usd", .{ .float = spent });
    return try jsonResponse(allocator, 429, .{ .object = obj });
}

fn callProvider(allocator: std.mem.Allocator, config: *const config_mod.Config, api_key: []const u8, model_name: []const u8, system: []const u8, user: []const u8, history: []const ChatMessage) !ProviderResult {
    var messages = std.json.Array.init(allocator);
    try messages.append(try json.object(allocator, &.{ .{ .key = "role", .value = .{ .string = "system" } }, .{ .key = "content", .value = .{ .string = system } } }));
    for (history) |item| {
        if (!std.mem.eql(u8, item.role, "user") and !std.mem.eql(u8, item.role, "assistant")) continue;
        try messages.append(try json.object(allocator, &.{ .{ .key = "role", .value = .{ .string = item.role } }, .{ .key = "content", .value = .{ .string = item.content } } }));
    }
    try messages.append(try json.object(allocator, &.{ .{ .key = "role", .value = .{ .string = "user" } }, .{ .key = "content", .value = .{ .string = user } } }));
    const payload_value = try json.object(allocator, &.{
        .{ .key = "model", .value = .{ .string = model_name } },
        .{ .key = "messages", .value = .{ .array = messages } },
        .{ .key = "max_tokens", .value = .{ .integer = config.openrouter_max_tokens } },
    });
    const payload = try json.stringify(allocator, payload_value);
    const auth = try std.fmt.allocPrint(allocator, "Bearer {s}", .{api_key});
    var client = std.http.Client{ .allocator = allocator };
    defer client.deinit();
    var response_body = std.ArrayList(u8).init(allocator);
    defer response_body.deinit();
    const extra = [_]std.http.Header{ .{ .name = "HTTP-Referer", .value = "https://study-platform.local" }, .{ .name = "X-Title", .value = "Study Platform" } };
    const response = try client.fetch(.{
        .location = .{ .url = OPENROUTER_URL },
        .method = .POST,
        .payload = payload,
        .headers = .{ .content_type = .{ .override = "application/json" }, .authorization = .{ .override = auth } },
        .extra_headers = &extra,
        .response_storage = .{ .dynamic = &response_body },
        .max_append_size = 1024 * 1024,
    });
    if (@intFromEnum(response.status) < 200 or @intFromEnum(response.status) >= 300) return error.ProviderError;
    const parsed = try json.parse(allocator, response_body.items);
    const choices_value = json.get(parsed, "choices") orelse return error.ProviderError;
    if (choices_value != .array or choices_value.array.items.len == 0) return error.ProviderError;
    const message_value = json.get(choices_value.array.items[0], "message") orelse return error.ProviderError;
    const content = json.getString(message_value, "content") orelse return error.ProviderError;
    var result = ProviderResult{ .content = content };
    if (json.get(parsed, "usage")) |usage| {
        result.input_tokens = json.getInt(usage, "prompt_tokens") orelse 0;
        result.output_tokens = json.getInt(usage, "completion_tokens") orelse 0;
        if (json.get(usage, "cost")) |cost_value| result.cost = switch (cost_value) {
            .float => |v| v,
            .integer => |v| @floatFromInt(v),
            else => 0,
        };
    }
    return result;
}

fn loadChatHistory(allocator: std.mem.Allocator, db: *sqlite.Db, subject: []const u8) ![]ChatMessage {
    var stmt = try db.prepare("SELECT role,content FROM (SELECT id,role,content FROM chat_history WHERE subject=?1 AND role IN ('user','assistant') ORDER BY id DESC LIMIT 20) ORDER BY id ASC");
    defer stmt.deinit();
    try stmt.bindText(1, subject);
    var messages: std.ArrayListUnmanaged(ChatMessage) = .empty;
    while (try stmt.next()) {
        try messages.append(allocator, .{ .role = try allocator.dupe(u8, stmt.text(0)), .content = try allocator.dupe(u8, stmt.text(1)) });
    }
    return messages.items;
}

fn retrieveNotes(allocator: std.mem.Allocator, db: *sqlite.Db, query: []const u8) !std.ArrayListUnmanaged(Source) {
    var sources: std.ArrayListUnmanaged(Source) = .empty;
    const pattern = try std.fmt.allocPrint(allocator, "%{s}%", .{query});
    var stmt = db.prepare("SELECT n.title,n.slug,COALESCE((SELECT content FROM note_versions v WHERE v.note_id=n.id ORDER BY id DESC LIMIT 1),n.summary,'') FROM notes n WHERE n.title LIKE ?1 OR n.tags LIKE ?1 OR n.summary LIKE ?1 ORDER BY n.updated_at DESC LIMIT 5") catch return sources;
    defer stmt.deinit();
    try stmt.bindText(1, pattern);
    while (try stmt.next()) {
        var content = try allocator.dupe(u8, stmt.text(2));
        if (content.len > 2400) content = content[0..2400];
        try sources.append(allocator, .{ .title = try allocator.dupe(u8, stmt.text(0)), .slug = try allocator.dupe(u8, stmt.text(1)), .content = content });
    }
    return sources;
}

fn sourceValues(allocator: std.mem.Allocator, sources: []const Source) std.json.Value {
    var array = std.json.Array.init(allocator);
    for (sources) |source| {
        var item = std.json.ObjectMap.init(allocator);
        item.put("type", .{ .string = "note" }) catch continue;
        item.put("title", .{ .string = source.title }) catch continue;
        const url = std.fmt.allocPrint(allocator, "/app/notes/{s}", .{source.slug}) catch continue;
        item.put("url", .{ .string = url }) catch continue;
        item.put("snippet", .{ .string = source.content }) catch continue;
        array.append(.{ .object = item }) catch continue;
    }
    return .{ .array = array };
}

fn persistChat(db: *sqlite.Db, allocator: std.mem.Allocator, subject: []const u8, user: []const u8, assistant: []const u8) !void {
    _ = allocator;
    var stmt = try db.prepare("INSERT INTO chat_history(subject,role,content) VALUES(?1,?2,?3)");
    defer stmt.deinit();
    try stmt.bindText(1, subject);
    try stmt.bindText(2, "user");
    try stmt.bindText(3, user);
    _ = try stmt.next();
    stmt.reset();
    try stmt.bindText(1, subject);
    try stmt.bindText(2, "assistant");
    try stmt.bindText(3, assistant);
    _ = try stmt.next();
}

fn recordUsage(db: *sqlite.Db, config: *const config_mod.Config, model_name: []const u8, endpoint: []const u8, result: ProviderResult) !void {
    var stmt = try db.prepare("INSERT INTO usage_metrics(model,input_tokens,output_tokens,cost_usd,estimated,endpoint) VALUES(?1,?2,?3,?4,?5,?6)");
    defer stmt.deinit();
    try stmt.bindText(1, model_name);
    try stmt.bindInt(2, result.input_tokens);
    try stmt.bindInt(3, result.output_tokens);
    try stmt.bindFloat(4, result.cost);
    try stmt.bindInt(5, if (config.openrouter_api_key == null or result.cost == 0) 1 else 0);
    try stmt.bindText(6, endpoint);
    _ = try stmt.next();
}

fn sseResponse(allocator: std.mem.Allocator, result: ProviderResult, mode: []const u8, sources: []const Source, include_context: bool) !http.Response {
    var body: std.ArrayListUnmanaged(u8) = .empty;
    if (include_context) {
        const context = try json.object(allocator, &.{ .{ .key = "mode", .value = .{ .string = mode } }, .{ .key = "web", .value = .{ .bool = false } }, .{ .key = "sources", .value = sourceValues(allocator, sources) } });
        try appendFrame(allocator, &body, "context", context);
    }
    try appendFrame(allocator, &body, "delta", try json.object(allocator, &.{.{ .key = "content", .value = .{ .string = result.content } }}));
    try appendFrame(allocator, &body, "done", try json.object(allocator, &.{.{ .key = "usage", .value = try json.object(allocator, &.{ .{ .key = "input", .value = .{ .integer = result.input_tokens } }, .{ .key = "output", .value = .{ .integer = result.output_tokens } }, .{ .key = "cost", .value = .{ .float = result.cost } } }) }}));
    var id_bytes: [16]u8 = undefined;
    std.crypto.random.bytes(&id_bytes);
    const stream_id = try security.hexEncode(allocator, &id_bytes);
    const headers = try allocator.alloc(http.Header, 2);
    headers[0] = .{ .name = "Cache-Control", .value = "no-cache" };
    headers[1] = .{ .name = "x-chat-stream-id", .value = stream_id };
    return .{ .status = 200, .content_type = "text/event-stream; charset=utf-8", .body = body.items, .headers = headers };
}

fn appendFrame(allocator: std.mem.Allocator, body: *std.ArrayListUnmanaged(u8), event: []const u8, payload: std.json.Value) !void {
    const data = try json.stringify(allocator, payload);
    try body.appendSlice(allocator, try std.fmt.allocPrint(allocator, "event: {s}\ndata: {s}\n\n", .{ event, data }));
}

fn validMode(mode: []const u8) bool {
    return std.mem.eql(u8, mode, "general") or std.mem.eql(u8, mode, "notes") or std.mem.eql(u8, mode, "hybrid");
}

fn validModel(model_name: []const u8) bool {
    if (model_name.len == 0 or model_name.len > 120) return false;
    for (model_name) |ch| if (!(std.ascii.isAlphanumeric(ch) or std.mem.indexOfScalar(u8, "-_./:#@", ch) != null)) return false;
    return true;
}

fn parseBody(allocator: std.mem.Allocator, request: *const http.Request) !std.json.Value {
    return json.parse(allocator, request.body);
}

fn jsonResponse(allocator: std.mem.Allocator, status: u16, value: std.json.Value) !http.Response {
    return .{ .status = status, .content_type = "application/json; charset=utf-8", .body = try json.stringify(allocator, value) };
}

fn jsonError(allocator: std.mem.Allocator, status: u16, message: []const u8) !http.Response {
    return jsonResponse(allocator, status, try json.object(allocator, &.{.{ .key = "error", .value = .{ .string = message } }}));
}

fn textResponse(status: u16, content_type: []const u8, body: []const u8) http.Response {
    return .{ .status = status, .content_type = content_type, .body = body };
}

test "AI retrieval modes and model validation are constrained" {
    try std.testing.expect(validMode("general"));
    try std.testing.expect(validMode("notes"));
    try std.testing.expect(!validMode("invented"));
    try std.testing.expect(validModel("provider/model:free"));
    try std.testing.expect(!validModel("model\r\nX-Injected: yes"));
}
