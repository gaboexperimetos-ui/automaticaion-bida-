const std = @import("std");
const http = @import("http.zig");
const sqlite = @import("sqlite.zig");
const config_mod = @import("config.zig");
const json = @import("jsonutil.zig");
const security = @import("security.zig");
const notes = @import("notes.zig");
const mcp_client = @import("mcp_client.zig");
const cyberhex_mod = @import("cyberhex.zig");
const office_mod = @import("office.zig");

pub fn handle(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request) !http.Response {
    if (!std.mem.eql(u8, request.path, "/mcp") and !std.mem.eql(u8, request.path, "/mcp/sse")) return jsonError(allocator, 404, "not_found");
    if (!std.mem.eql(u8, request.method, "POST")) return jsonError(allocator, 405, "Method not allowed");
    const expected_key = config.custom_mcp_api_key orelse return jsonRpcResponse(allocator, 401, null, null, .{ .code = -32001, .message = "MCP server disabled: configure CUSTOM_MCP_API_KEY" });
    const provided_key = request.header("x-api-key") orelse "";
    if (!security.constantTimeEq(provided_key, expected_key)) return jsonRpcResponse(allocator, 401, null, null, .{ .code = -32001, .message = "Unauthorized" });
    const req = json.parse(allocator, request.body) catch return jsonError(allocator, 400, "Invalid JSON-RPC request");
    const rpc_version = json.getString(req, "jsonrpc") orelse "";
    const method = json.getString(req, "method") orelse "";
    const id = json.get(req, "id");
    if (!std.mem.eql(u8, rpc_version, "2.0")) return jsonRpcResponse(allocator, 400, id, null, .{ .code = -32600, .message = "Invalid Request: jsonrpc must be 2.0" });
    const params = json.get(req, "params") orelse try json.object(allocator, &.{});
    const result = dispatch(allocator, db, config, method, params, id) catch |err| {
        const rpc_error = switch (err) {
            error.InvalidParams => RpcError{ .code = -32602, .message = "Invalid params" },
            error.MethodNotFound => RpcError{ .code = -32601, .message = "Method not found" },
            error.NotFound => RpcError{ .code = -32000, .message = "Not found" },
            else => RpcError{ .code = -32603, .message = "Internal error" },
        };
        return jsonRpcResponse(allocator, 200, id, null, rpc_error);
    };
    var response = try jsonRpcResponse(allocator, 200, id, result, null);
    if (std.mem.eql(u8, request.path, "/mcp/sse")) {
        const framed = try std.fmt.allocPrint(allocator, "event: message\ndata: {s}\n\n", .{response.body});
        response.body = framed;
        response.content_type = "text/event-stream; charset=utf-8";
        const headers = try allocator.alloc(http.Header, 1);
        headers[0] = .{ .name = "Cache-Control", .value = "no-cache" };
        response.headers = headers;
    }
    return response;
}

const RpcError = struct { code: i64, message: []const u8 };

fn dispatch(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, method: []const u8, params: std.json.Value, id: ?std.json.Value) !std.json.Value {
    _ = id;
    if (std.mem.eql(u8, method, "initialize")) {
        return json.object(allocator, &.{
            .{ .key = "protocolVersion", .value = .{ .string = "2025-03-26" } },
            .{ .key = "capabilities", .value = try json.object(allocator, &.{ .{ .key = "tools", .value = try json.object(allocator, &.{}) }, .{ .key = "prompts", .value = try json.object(allocator, &.{}) } }) },
            .{ .key = "serverInfo", .value = try json.object(allocator, &.{ .{ .key = "name", .value = .{ .string = "study-platform-mcp" } }, .{ .key = "version", .value = .{ .string = "0.2.0" } } }) },
        });
    }
    if (std.mem.eql(u8, method, "notifications/initialized")) return json.object(allocator, &.{});
    if (std.mem.eql(u8, method, "tools/list")) return json.object(allocator, &.{.{ .key = "tools", .value = try toolDefinitions(allocator) }});
    if (!std.mem.eql(u8, method, "tools/call")) return error.MethodNotFound;
    const name = json.getString(params, "name") orelse return error.InvalidParams;
    const args = json.get(params, "arguments") orelse try json.object(allocator, &.{});
    const payload = try callTool(allocator, db, config, name, args);
    const text_value = try json.stringify(allocator, payload);
    var content = std.json.Array.init(allocator);
    try content.append(try json.object(allocator, &.{ .{ .key = "type", .value = .{ .string = "text" } }, .{ .key = "text", .value = .{ .string = text_value } } }));
    var result = std.json.ObjectMap.init(allocator);
    try result.put("content", .{ .array = content });
    if (payload == .object) {
        var it = payload.object.iterator();
        while (it.next()) |entry| {
            if (std.mem.eql(u8, entry.key_ptr.*, "content")) continue;
            try result.put(entry.key_ptr.*, entry.value_ptr.*);
        }
    }
    return .{ .object = result };
}

fn toolDefinitions(allocator: std.mem.Allocator) !std.json.Value {
    const definitions = [_]struct { name: []const u8, description: []const u8, props: []const u8, required: []const []const u8 }{
        .{ .name = "search_notes", .description = "Search local notes", .props = "query", .required = &.{"query"} },
        .{ .name = "get_note", .description = "Get a note by slug", .props = "slug", .required = &.{"slug"} },
        .{ .name = "create_note", .description = "Create a local Markdown note", .props = "title,content,tags", .required = &.{ "title", "content" } },
        .{ .name = "get_academic_context", .description = "Get available academic context", .props = "", .required = &.{} },
        .{ .name = "get_subject_notes", .description = "Get notes tagged with a subject", .props = "subject", .required = &.{"subject"} },
        .{ .name = "get_study_sheets", .description = "List local study sheets", .props = "subject", .required = &.{} },
        .{ .name = "app_list_backups", .description = "List database backups", .props = "", .required = &.{} },
        .{ .name = "app_backup_now", .description = "Create a database backup", .props = "", .required = &.{} },
        .{ .name = "app_ai_cost", .description = "Get monthly AI cost summary", .props = "", .required = &.{} },
        .{ .name = "cyberhex_status", .description = "Current CyberHex track and playback state", .props = "", .required = &.{} },
        .{ .name = "cyberhex_library_search", .description = "Search the CyberHex scanned library", .props = "query", .required = &.{"query"} },
        .{ .name = "cyberhex_queue", .description = "Read the CyberHex play queue", .props = "", .required = &.{} },
        .{ .name = "cyberhex_playback", .description = "Control CyberHex: play, pause, toggle, next, previous or seek", .props = "command,value", .required = &.{"command"} },
        .{ .name = "cyberhex_set_volume", .description = "Set CyberHex volume between 0 and 1", .props = "volume", .required = &.{"volume"} },
        .{ .name = "cyberhex_queue_add", .description = "Add a library track to the CyberHex queue", .props = "path", .required = &.{"path"} },
        .{ .name = "note_update", .description = "Update an existing note (title, content or tags)", .props = "slug,title,content,tags", .required = &.{"slug"} },
        .{ .name = "note_delete", .description = "Delete a note and its Markdown files", .props = "slug", .required = &.{"slug"} },
        .{ .name = "note_versions", .description = "List the version history of a note", .props = "slug", .required = &.{"slug"} },
        .{ .name = "notes_sync_vault", .description = "Import the Obsidian vault into the database", .props = "", .required = &.{} },
        .{ .name = "office_list", .description = "List Office files: docs, sheets, presentations or html", .props = "kind", .required = &.{} },
        .{ .name = "office_read", .description = "Read an Office file by kind and slug", .props = "kind,slug", .required = &.{ "kind", "slug" } },
        .{ .name = "office_write", .description = "Create or overwrite an Office file (document, sheet, deck, page)", .props = "kind,title,content", .required = &.{ "kind", "title" } },
        .{ .name = "office_delete", .description = "Delete an Office file by kind and slug", .props = "kind,slug", .required = &.{ "kind", "slug" } },
        .{ .name = "calendar_list", .description = "List calendar annotations", .props = "", .required = &.{} },
        .{ .name = "calendar_add", .description = "Create a calendar annotation", .props = "title,date,content", .required = &.{"title"} },
        .{ .name = "calendar_delete", .description = "Delete a calendar annotation by id", .props = "id", .required = &.{"id"} },
        .{ .name = "app_status", .description = "Platform status and configuration summary", .props = "", .required = &.{} },
        .{ .name = "student_report", .description = "Build the student report (Markdown) from real Mikareno + local notes data", .props = "format", .required = &.{} },
    };
    var list = std.json.Array.init(allocator);
    for (definitions) |definition| {
        var properties = std.json.ObjectMap.init(allocator);
        var required = std.json.Array.init(allocator);
        var iter = std.mem.splitScalar(u8, definition.props, ',');
        while (iter.next()) |key| {
            if (key.len == 0) continue;
            const type_name = if (std.mem.eql(u8, key, "limit")) "integer" else if (std.mem.eql(u8, key, "tags")) "array" else "string";
            try properties.put(key, try json.object(allocator, &.{.{ .key = "type", .value = .{ .string = type_name } }}));
        }
        for (definition.required) |field| try required.append(.{ .string = field });
        const schema = try json.object(allocator, &.{ .{ .key = "type", .value = .{ .string = "object" } }, .{ .key = "properties", .value = .{ .object = properties } }, .{ .key = "required", .value = .{ .array = required } } });
        try list.append(try json.object(allocator, &.{ .{ .key = "name", .value = .{ .string = definition.name } }, .{ .key = "description", .value = .{ .string = definition.description } }, .{ .key = "inputSchema", .value = schema } }));
    }
    return .{ .array = list };
}

fn callTool(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, name: []const u8, args: std.json.Value) !std.json.Value {
    if (std.mem.eql(u8, name, "search_notes")) {
        const query = json.getString(args, "query") orelse return error.InvalidParams;
        const results = try searchNotes(allocator, db, query, 10);
        return json.object(allocator, &.{.{ .key = "results", .value = results }});
    }
    if (std.mem.eql(u8, name, "get_note")) {
        const slug = json.getString(args, "slug") orelse return error.InvalidParams;
        if (!safeSlug(slug)) return error.InvalidParams;
        var stmt = try db.prepare("SELECT n.id,n.title,n.slug,n.filename,n.tags,n.word_count,COALESCE(n.summary,''),n.created_at,n.updated_at,COALESCE((SELECT content FROM note_versions v WHERE v.note_id=n.id ORDER BY v.id DESC LIMIT 1),'') FROM notes n WHERE n.slug=?1");
        defer stmt.deinit();
        try stmt.bindText(1, slug);
        if (!try stmt.next()) return error.NotFound;
        const content = try allocator.dupe(u8, stmt.text(9));
        const note_value = try json.object(allocator, &.{ .{ .key = "id", .value = .{ .integer = stmt.int64(0) } }, .{ .key = "title", .value = .{ .string = try allocator.dupe(u8, stmt.text(1)) } }, .{ .key = "slug", .value = .{ .string = try allocator.dupe(u8, stmt.text(2)) } }, .{ .key = "filename", .value = .{ .string = try allocator.dupe(u8, stmt.text(3)) } }, .{ .key = "tags", .value = .{ .string = try allocator.dupe(u8, stmt.text(4)) } }, .{ .key = "word_count", .value = .{ .integer = stmt.int64(5) } }, .{ .key = "summary", .value = .{ .string = try allocator.dupe(u8, stmt.text(6)) } }, .{ .key = "created_at", .value = .{ .string = try allocator.dupe(u8, stmt.text(7)) } }, .{ .key = "updated_at", .value = .{ .string = try allocator.dupe(u8, stmt.text(8)) } } });
        return json.object(allocator, &.{ .{ .key = "note", .value = note_value }, .{ .key = "content", .value = .{ .string = content } } });
    }
    if (std.mem.eql(u8, name, "create_note")) {
        const title = json.getString(args, "title") orelse return error.InvalidParams;
        const content = json.getString(args, "content") orelse return error.InvalidParams;
        const tags = tagsAsString(allocator, json.get(args, "tags")) catch return error.InvalidParams;
        const slug = try createNoteMcp(allocator, db, config, title, content, tags);
        return json.object(allocator, &.{ .{ .key = "success", .value = .{ .bool = true } }, .{ .key = "slug", .value = .{ .string = slug } } });
    }
    if (std.mem.eql(u8, name, "get_academic_context")) {
        const data = mcp_client.callTool(allocator, config.mikareno_mcp_url, config.mcp_timeout_ms, "get_student_full", try json.object(allocator, &.{.{ .key = "bg", .value = .{ .bool = true } }}), true) catch {
            return json.object(allocator, &.{
                .{ .key = "connected", .value = .{ .bool = false } },
                .{ .key = "available", .value = .{ .bool = false } },
                .{ .key = "error", .value = .{ .string = "Mikareno no está disponible; no se inventaron datos académicos." } },
            });
        };
        return json.object(allocator, &.{
            .{ .key = "connected", .value = .{ .bool = true } },
            .{ .key = "available", .value = .{ .bool = true } },
            .{ .key = "data", .value = data },
            .{ .key = "calendario", .value = json.get(data, "calendario") orelse .null },
            .{ .key = "horario", .value = json.get(data, "horario") orelse .null },
            .{ .key = "perfil", .value = json.get(data, "perfil") orelse json.get(data, "estudiante") orelse .null },
        });
    }
    if (std.mem.eql(u8, name, "get_subject_notes")) {
        const subject = json.getString(args, "subject") orelse return error.InvalidParams;
        var stmt = try db.prepare("SELECT slug,title,tags,COALESCE(summary,'') FROM notes WHERE tags LIKE ?1 ORDER BY updated_at DESC LIMIT 50");
        defer stmt.deinit();
        const pattern = try std.fmt.allocPrint(allocator, "%{s}%", .{subject});
        try stmt.bindText(1, pattern);
        var list = std.json.Array.init(allocator);
        while (try stmt.next()) try list.append(try json.object(allocator, &.{ .{ .key = "slug", .value = .{ .string = try allocator.dupe(u8, stmt.text(0)) } }, .{ .key = "title", .value = .{ .string = try allocator.dupe(u8, stmt.text(1)) } }, .{ .key = "tags", .value = .{ .string = try allocator.dupe(u8, stmt.text(2)) } }, .{ .key = "summary", .value = .{ .string = try allocator.dupe(u8, stmt.text(3)) } } }));
        return json.object(allocator, &.{.{ .key = "notes", .value = .{ .array = list } }});
    }
    if (std.mem.eql(u8, name, "get_study_sheets")) return studySheets(allocator, db, args);
    if (std.mem.eql(u8, name, "app_list_backups")) return backups(allocator, config);
    if (std.mem.eql(u8, name, "app_backup_now")) return backupNow(allocator, db, config);
    if (std.mem.eql(u8, name, "app_ai_cost")) return aiCost(allocator, db, config);
    if (std.mem.eql(u8, name, "cyberhex_status")) return cyberhexStatus(allocator, config);
    if (std.mem.eql(u8, name, "cyberhex_library_search")) {
        const query = json.getString(args, "query") orelse return error.InvalidParams;
        return cyberhexLibrary(allocator, config, query);
    }
    if (std.mem.eql(u8, name, "cyberhex_queue")) return cyberhexQueue(allocator, config);
    if (std.mem.eql(u8, name, "cyberhex_playback")) {
        const command_v = json.getString(args, "command") orelse return error.InvalidParams;
        if (!validCyberHexCommand(command_v)) return error.InvalidParams;
        return cyberhexCommand(allocator, config, command_v, json.get(args, "value"));
    }
    if (std.mem.eql(u8, name, "cyberhex_set_volume")) {
        const raw = json.get(args, "volume") orelse return error.InvalidParams;
        const volume = switch (raw) {
            .float => |f| f,
            .integer => |i| @as(f64, @floatFromInt(i)),
            else => return error.InvalidParams,
        };
        if (std.math.isNan(volume) or volume < 0 or volume > 1) return error.InvalidParams;
        return cyberhexVolume(allocator, config, volume);
    }
    if (std.mem.eql(u8, name, "cyberhex_queue_add")) {
        const path = json.getString(args, "path") orelse return error.InvalidParams;
        if (path.len == 0) return error.InvalidParams;
        return cyberhexQueueAdd(allocator, config, path);
    }
    // ── Notas: CRUD completo, igual que desde la interfaz ──
    if (std.mem.eql(u8, name, "note_update")) {
        const slug = json.getString(args, "slug") orelse return error.InvalidParams;
        return noteUpdate(allocator, db, config, slug, json.getString(args, "title"), json.getString(args, "content"), json.getString(args, "tags"));
    }
    if (std.mem.eql(u8, name, "note_delete")) {
        const slug = json.getString(args, "slug") orelse return error.InvalidParams;
        return noteDelete(allocator, db, config, slug);
    }
    if (std.mem.eql(u8, name, "note_versions")) {
        const slug = json.getString(args, "slug") orelse return error.InvalidParams;
        return noteVersions(allocator, db, slug);
    }
    if (std.mem.eql(u8, name, "notes_sync_vault")) return notesSyncVault(allocator, db, config);

    // ── Office: documentos, hojas, presentaciones y páginas ──
    if (std.mem.eql(u8, name, "office_list")) {
        const kind = officeKind(json.getString(args, "kind") orelse "docs") orelse return error.InvalidParams;
        return office_mod.listFilesForMcp(allocator, config, kind);
    }
    if (std.mem.eql(u8, name, "office_read")) {
        const kind = officeKind(json.getString(args, "kind") orelse "") orelse return error.InvalidParams;
        const slug = json.getString(args, "slug") orelse return error.InvalidParams;
        return officeRead(allocator, config, kind, slug);
    }
    if (std.mem.eql(u8, name, "office_write")) {
        const kind = officeKind(json.getString(args, "kind") orelse "") orelse return error.InvalidParams;
        const title = json.getString(args, "title") orelse return error.InvalidParams;
        const content = json.getString(args, "content") orelse "";
        return officeWrite(allocator, config, kind, title, content);
    }
    if (std.mem.eql(u8, name, "office_delete")) {
        const kind = officeKind(json.getString(args, "kind") orelse "") orelse return error.InvalidParams;
        const slug = json.getString(args, "slug") orelse return error.InvalidParams;
        return officeDelete(allocator, config, kind, slug);
    }

    // ── Calendario: anotaciones reales ──
    if (std.mem.eql(u8, name, "calendar_list")) return calendarList(allocator, db);
    if (std.mem.eql(u8, name, "calendar_add")) {
        const title = json.getString(args, "title") orelse return error.InvalidParams;
        const date = json.getString(args, "date") orelse "";
        const body = json.getString(args, "content") orelse "";
        return calendarAdd(allocator, db, title, date, body);
    }
    if (std.mem.eql(u8, name, "calendar_delete")) {
        const raw = json.get(args, "id") orelse return error.InvalidParams;
        const id: i64 = switch (raw) {
            .integer => |i| i,
            else => return error.InvalidParams,
        };
        return calendarDelete(allocator, db, id);
    }
    if (std.mem.eql(u8, name, "app_status")) return appStatus(allocator, db, config);
    if (std.mem.eql(u8, name, "student_report")) {
        const fmt = json.getString(args, "format") orelse "markdown";
        if (!std.mem.eql(u8, fmt, "markdown") and !std.mem.eql(u8, fmt, "md")) return error.InvalidParams;
        return studentReport(allocator, db, config);
    }
    return error.MethodNotFound;
}

/// Solo se aceptan las acciones que la REST de CyberHex reconoce.
fn validCyberHexCommand(command_v: []const u8) bool {
    const allowed = [_][]const u8{ "play", "pause", "toggle", "next", "previous", "seek" };
    for (allowed) |name| if (std.mem.eql(u8, command_v, name)) return true;
    return false;
}

/// Config del cliente de CyberHex, o error si no está configurado.
fn cyberhexConfig(config: *const config_mod.Config) !cyberhex_mod.Config {
    if (config.cyberhex_api_url.len == 0 or config.cyberhex_api_key.len == 0) return error.NotConfigured;
    return .{
        .url = config.cyberhex_api_url,
        .key = config.cyberhex_api_key,
        .timeout_ms = config.cyberhex_timeout_ms,
    };
}

/// Mensaje estable para "CyberHex no está conectado", sin inventar nada.
fn cyberhexNoConfigured(allocator: std.mem.Allocator) !std.json.Value {
    return json.object(allocator, &.{
        .{ .key = "connected", .value = .{ .bool = false } },
        .{ .key = "configured", .value = .{ .bool = false } },
        .{ .key = "message", .value = .{ .string = "CyberHex no está conectado: configura CYBERHEX_API_URL y CYBERHEX_API_KEY." } },
    });
}

fn cyberhexStatus(allocator: std.mem.Allocator, config: *const config_mod.Config) !std.json.Value {
    const cfg = cyberhexConfig(config) catch return cyberhexNoConfigured(allocator);
    const payload = cyberhex_mod.status(allocator, cfg) catch |err| switch (err) {
        error.CyberHexUnavailable => return json.object(allocator, &.{
            .{ .key = "connected", .value = .{ .bool = false } },
            .{ .key = "configured", .value = .{ .bool = true } },
            .{ .key = "message", .value = .{ .string = "CyberHex no responde en su puerto local." } },
        }),
        error.CyberHexUnauthorized => return json.object(allocator, &.{
            .{ .key = "connected", .value = .{ .bool = false } },
            .{ .key = "configured", .value = .{ .bool = true } },
            .{ .key = "message", .value = .{ .string = "CyberHex rechazó la clave (Authorization Bearer)." } },
        }),
        else => return error.CyberHexUnavailable,
    };
    var out = payload;
    if (out != .object) return error.CyberHexBadResponse;
    try out.object.put("connected", .{ .bool = true });
    try out.object.put("configured", .{ .bool = true });
    return out;
}

fn cyberhexLibrary(allocator: std.mem.Allocator, config: *const config_mod.Config, query: []const u8) !std.json.Value {
    const cfg = cyberhexConfig(config) catch return cyberhexNoConfigured(allocator);
    return cyberhex_mod.library(allocator, cfg, query) catch return json.object(allocator, &.{
        .{ .key = "connected", .value = .{ .bool = false } },
        .{ .key = "query", .value = .{ .string = query } },
        .{ .key = "results", .value = try json.array(allocator, &.{}) },
    });
}

fn cyberhexQueue(allocator: std.mem.Allocator, config: *const config_mod.Config) !std.json.Value {
    const cfg = cyberhexConfig(config) catch return cyberhexNoConfigured(allocator);
    return cyberhex_mod.queue(allocator, cfg) catch return json.object(allocator, &.{
        .{ .key = "connected", .value = .{ .bool = false } },
        .{ .key = "queue", .value = try json.array(allocator, &.{}) },
    });
}

fn cyberhexCommand(allocator: std.mem.Allocator, config: *const config_mod.Config, accion: []const u8, valor: ?std.json.Value) !std.json.Value {
    const cfg = cyberhexConfig(config) catch return cyberhexNoConfigured(allocator);
    return cyberhex_mod.command(allocator, cfg, accion, valor) catch return json.object(allocator, &.{
        .{ .key = "connected", .value = .{ .bool = false } },
        .{ .key = "command", .value = .{ .string = accion } },
        .{ .key = "ok", .value = .{ .bool = false } },
    });
}

fn cyberhexVolume(allocator: std.mem.Allocator, config: *const config_mod.Config, volumen: f64) !std.json.Value {
    const cfg = cyberhexConfig(config) catch return cyberhexNoConfigured(allocator);
    return cyberhex_mod.setVolume(allocator, cfg, volumen) catch return json.object(allocator, &.{
        .{ .key = "connected", .value = .{ .bool = false } },
        .{ .key = "volume", .value = .{ .float = volumen } },
        .{ .key = "ok", .value = .{ .bool = false } },
    });
}

fn cyberhexQueueAdd(allocator: std.mem.Allocator, config: *const config_mod.Config, path: []const u8) !std.json.Value {
    const cfg = cyberhexConfig(config) catch return cyberhexNoConfigured(allocator);
    return cyberhex_mod.queueAdd(allocator, cfg, path) catch return json.object(allocator, &.{
        .{ .key = "connected", .value = .{ .bool = false } },
        .{ .key = "path", .value = .{ .string = path } },
        .{ .key = "ok", .value = .{ .bool = false } },
    });
}

/// Traduce el nombre corto de un tipo de Office al enum; null si no vale.
fn officeKind(raw: []const u8) ?office_mod.Kind {
    if (std.mem.eql(u8, raw, "docs") or std.mem.eql(u8, raw, "doc") or std.mem.eql(u8, raw, "document")) return .docs;
    if (std.mem.eql(u8, raw, "sheets") or std.mem.eql(u8, raw, "sheet") or std.mem.eql(u8, raw, "xlsx")) return .sheets;
    if (std.mem.eql(u8, raw, "presentations") or std.mem.eql(u8, raw, "deck")) return .presentations;
    if (std.mem.eql(u8, raw, "html") or std.mem.eql(u8, raw, "page")) return .html;
    return null;
}

fn noteUpdate(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, slug: []const u8, title: ?[]const u8, content: ?[]const u8, tags: ?[]const u8) !std.json.Value {
    const final_slug = notes.updateNoteForMcp(allocator, db, config, slug, title, content, tags) catch |err| switch (err) {
        error.InvalidSlug, error.InvalidTitle, error.ContentTooLarge, error.TagsTooLong => return error.InvalidParams,
        else => return error.NotFound,
    };
    return json.object(allocator, &.{
        .{ .key = "updated", .value = .{ .bool = true } },
        .{ .key = "slug", .value = .{ .string = final_slug } },
        .{ .key = "url", .value = .{ .string = try std.fmt.allocPrint(allocator, "/dashboard/notes/{s}", .{final_slug}) } },
    });
}

fn noteDelete(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, slug: []const u8) !std.json.Value {
    try notes.deleteNoteForMcp(allocator, db, config, slug);
    return json.object(allocator, &.{
        .{ .key = "deleted", .value = .{ .bool = true } },
        .{ .key = "slug", .value = .{ .string = slug } },
    });
}

fn noteVersions(allocator: std.mem.Allocator, db: *sqlite.Db, slug: []const u8) !std.json.Value {
    const list = try notes.noteVersionsForMcp(allocator, db, slug);
    const n: usize = if (list == .array) list.array.items.len else 0;
    return json.object(allocator, &.{
        .{ .key = "slug", .value = .{ .string = slug } },
        .{ .key = "count", .value = .{ .integer = @intCast(n) } },
        .{ .key = "versions", .value = list },
    });
}

fn notesSyncVault(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config) !std.json.Value {
    const counts = try notes.syncVaultCounts(allocator, db, config);
    return json.object(allocator, &.{
        .{ .key = "imported", .value = .{ .integer = @intCast(counts.imported) } },
        .{ .key = "updated", .value = .{ .integer = @intCast(counts.updated) } },
        .{ .key = "skipped", .value = .{ .integer = @intCast(counts.skipped) } },
        .{ .key = "vault_dir", .value = .{ .string = config.vault_dir } },
    });
}

fn officeRead(allocator: std.mem.Allocator, config: *const config_mod.Config, kind: office_mod.Kind, slug: []const u8) !std.json.Value {
    const file = try office_mod.readFileForMcp(allocator, config, kind, slug);
    return json.object(allocator, &.{
        .{ .key = "kind", .value = .{ .string = kind.dir() } },
        .{ .key = "slug", .value = .{ .string = file.slug } },
        .{ .key = "path", .value = .{ .string = file.path } },
        .{ .key = "content", .value = .{ .string = file.content } },
        .{ .key = "bytes", .value = .{ .integer = @intCast(file.content.len) } },
    });
}

fn officeWrite(allocator: std.mem.Allocator, config: *const config_mod.Config, kind: office_mod.Kind, title: []const u8, content: []const u8) !std.json.Value {
    const slug = office_mod.createFileForMcp(allocator, config, kind, title, content) catch |err| switch (err) {
        error.InvalidTitle, error.ContentTooLarge, error.InvalidSlug => return error.InvalidParams,
        else => return error.InternalError,
    };
    return json.object(allocator, &.{
        .{ .key = "saved", .value = .{ .bool = true } },
        .{ .key = "kind", .value = .{ .string = kind.dir() } },
        .{ .key = "slug", .value = .{ .string = slug } },
        .{ .key = "bytes", .value = .{ .integer = @intCast(content.len) } },
    });
}

fn officeDelete(allocator: std.mem.Allocator, config: *const config_mod.Config, kind: office_mod.Kind, slug: []const u8) !std.json.Value {
    office_mod.deleteFileForMcp(allocator, config, kind, slug) catch |err| switch (err) {
        error.InvalidSlug => return error.InvalidParams,
        error.NotFound => return error.NotFound,
        else => return error.InternalError,
    };
    return json.object(allocator, &.{
        .{ .key = "deleted", .value = .{ .bool = true } },
        .{ .key = "kind", .value = .{ .string = kind.dir() } },
        .{ .key = "slug", .value = .{ .string = slug } },
    });
}

fn calendarList(allocator: std.mem.Allocator, db: *sqlite.Db) !std.json.Value {
    var stmt = try db.prepare("SELECT id,date,title,COALESCE(body,''),created_at FROM calendar_annotations ORDER BY date DESC, id DESC LIMIT 200");
    defer stmt.deinit();
    var list = std.json.Array.init(allocator);
    while (try stmt.next()) {
        try list.append(try json.object(allocator, &.{
            .{ .key = "id", .value = .{ .integer = stmt.int64(0) } },
            .{ .key = "date", .value = .{ .string = try allocator.dupe(u8, stmt.text(1)) } },
            .{ .key = "title", .value = .{ .string = try allocator.dupe(u8, stmt.text(2)) } },
            .{ .key = "body", .value = .{ .string = try allocator.dupe(u8, stmt.text(3)) } },
            .{ .key = "created_at", .value = .{ .string = try allocator.dupe(u8, stmt.text(4)) } },
        }));
    }
    return json.object(allocator, &.{
        .{ .key = "count", .value = .{ .integer = @intCast(list.items.len) } },
        .{ .key = "annotations", .value = .{ .array = list } },
    });
}

fn calendarAdd(allocator: std.mem.Allocator, db: *sqlite.Db, title: []const u8, date: []const u8, body: []const u8) !std.json.Value {
    const trimmed = std.mem.trim(u8, title, " \t\r\n");
    if (trimmed.len == 0 or trimmed.len > 200) return error.InvalidParams;
    if (body.len > 20_000) return error.InvalidParams;
    // Sin fecha se usa hoy: una anotación siempre cae en algún día.
    var final_date: []const u8 = date;
    if (std.mem.trim(u8, date, " \t\r\n").len == 0) {
        final_date = try todayKey(allocator);
    }
    var stmt = try db.prepare("INSERT INTO calendar_annotations(date,title,body) VALUES(?1,?2,?3)");
    defer stmt.deinit();
    try stmt.bindText(1, final_date);
    try stmt.bindText(2, trimmed);
    try stmt.bindText(3, body);
    _ = try stmt.next();
    return json.object(allocator, &.{
        .{ .key = "created", .value = .{ .bool = true } },
        .{ .key = "date", .value = .{ .string = final_date } },
        .{ .key = "title", .value = .{ .string = trimmed } },
    });
}

/// Fecha de hoy en formato `AAAA-MM-DD`.
fn todayKey(allocator: std.mem.Allocator) ![]const u8 {
    const e = std.time.epoch.EpochSeconds{ .secs = @intCast(@max(0, std.time.timestamp())) };
    const year_day = e.getEpochDay().calculateYearDay();
    const month_day = year_day.calculateMonthDay();
    return std.fmt.allocPrint(allocator, "{d:0>4}-{d:0>2}-{d:0>2}", .{ year_day.year, month_day.month.numeric(), month_day.day_index + 1 });
}

fn calendarDelete(allocator: std.mem.Allocator, db: *sqlite.Db, id: i64) !std.json.Value {
    var stmt = try db.prepare("DELETE FROM calendar_annotations WHERE id=?1");
    defer stmt.deinit();
    try stmt.bindInt(1, id);
    _ = try stmt.next();
    return json.object(allocator, &.{
        .{ .key = "deleted", .value = .{ .bool = true } },
        .{ .key = "id", .value = .{ .integer = id } },
    });
}

/// Informe del estudiante en Markdown, construido con datos reales de
/// Mikareno (perfil, calificaciones, calendario) y las notas locales.
fn studentReport(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config) !std.json.Value {
    const markdown = try office_mod.buildStudentReport(allocator, db, config);
    const genoffice_ready = blk: {
        const url = std.mem.trim(u8, config.genoffice_mcp_url, " \t\r\n");
        if (url.len == 0) break :blk false;
        break :blk mcp_client.probe(allocator, url, @min(config.genoffice_timeout_ms, 2500), true) catch false;
    };
    return json.object(allocator, &.{
        .{ .key = "format", .value = .{ .string = "markdown" } },
        .{ .key = "markdown", .value = .{ .string = markdown } },
        .{ .key = "bytes", .value = .{ .integer = @intCast(markdown.len) } },
        .{ .key = "genoffice_ready", .value = .{ .bool = genoffice_ready } },
        .{ .key = "note", .value = .{ .string = "Si GenOffice responde, /office/report devuelve el mismo informe empaquetado en DOCX." } },
    });
}

fn appStatus(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config) !std.json.Value {
    const notes_n = db.scalarInt("SELECT COUNT(*) FROM notes") catch null;
    const ann_n = db.scalarInt("SELECT COUNT(*) FROM calendar_annotations") catch null;
    const chat_n = db.scalarInt("SELECT COUNT(DISTINCT subject) FROM chat_history") catch null;
    var docs: usize = 0;
    var sheets: usize = 0;
    var decks: usize = 0;
    for ([_]office_mod.Kind{ .docs, .sheets, .presentations }) |kind| {
        const listed = office_mod.listFilesForMcp(allocator, config, kind) catch continue;
        const c = json.get(listed, "count") orelse continue;
        if (c != .integer) continue;
        const n: usize = @intCast(@max(0, c.integer));
        switch (kind) {
            .docs => docs = n,
            .sheets => sheets = n,
            .presentations => decks = n,
            else => {},
        }
    }
    return json.object(allocator, &.{
        .{ .key = "app", .value = .{ .string = "NOVA · study-platform" } },
        .{ .key = "port", .value = .{ .integer = config.port } },
        .{ .key = "notes", .value = .{ .integer = notes_n orelse 0 } },
        .{ .key = "calendar_annotations", .value = .{ .integer = ann_n orelse 0 } },
        .{ .key = "conversations", .value = .{ .integer = chat_n orelse 0 } },
        .{ .key = "office_docs", .value = .{ .integer = @intCast(docs) } },
        .{ .key = "office_sheets", .value = .{ .integer = @intCast(sheets) } },
        .{ .key = "office_presentations", .value = .{ .integer = @intCast(decks) } },
        .{ .key = "cyberhex_connected", .value = .{ .bool = config.cyberhex_api_url.len > 0 and config.cyberhex_api_key.len > 0 } },
        .{ .key = "mikareno_configured", .value = .{ .bool = config.mikareno_mcp_url.len > 0 } },
        .{ .key = "ai_model", .value = .{ .string = config.openrouter_model } },
        .{ .key = "vault_dir", .value = .{ .string = config.vault_dir } },
    });
}

fn studySheets(allocator: std.mem.Allocator, db: *sqlite.Db, args: std.json.Value) !std.json.Value {
    const subject = json.getString(args, "subject");
    const sql = if (subject != null)
        "SELECT n.slug,n.title,n.tags,COALESCE((SELECT content FROM note_versions v WHERE v.note_id=n.id ORDER BY v.id DESC LIMIT 1),'') FROM notes n WHERE n.tags LIKE ?1 ORDER BY n.updated_at DESC LIMIT 20"
    else
        "SELECT n.slug,n.title,n.tags,COALESCE((SELECT content FROM note_versions v WHERE v.note_id=n.id ORDER BY v.id DESC LIMIT 1),'') FROM notes n ORDER BY n.updated_at DESC LIMIT 20";
    var stmt = try db.prepare(sql);
    defer stmt.deinit();
    if (subject) |value| {
        const pattern = try std.fmt.allocPrint(allocator, "%{s}%", .{value});
        try stmt.bindText(1, pattern);
    }
    var items = std.json.Array.init(allocator);
    while (try stmt.next()) {
        const metadata = try json.object(allocator, &.{
            .{ .key = "slug", .value = .{ .string = try allocator.dupe(u8, stmt.text(0)) } },
            .{ .key = "title", .value = .{ .string = try allocator.dupe(u8, stmt.text(1)) } },
            .{ .key = "tags", .value = .{ .string = try allocator.dupe(u8, stmt.text(2)) } },
        });
        try items.append(try json.object(allocator, &.{
            .{ .key = "note", .value = metadata },
            .{ .key = "content", .value = .{ .string = try allocator.dupe(u8, stmt.text(3)) } },
        }));
    }
    return json.object(allocator, &.{ .{ .key = "items", .value = .{ .array = items } }, .{ .key = "available", .value = .{ .bool = true } } });
}

fn tagsAsString(allocator: std.mem.Allocator, tags: ?std.json.Value) ![]const u8 {
    const value = tags orelse return "";
    if (value == .string) return value.string;
    if (value != .array) return error.InvalidParams;
    var output: std.ArrayList(u8) = .init(allocator);
    for (value.array.items, 0..) |item, index| {
        if (item != .string or item.string.len > 80) return error.InvalidParams;
        if (index > 0) try output.appendSlice(", ");
        try output.appendSlice(item.string);
    }
    return output.toOwnedSlice();
}

fn searchNotes(allocator: std.mem.Allocator, db: *sqlite.Db, query: []const u8, limit: usize) !std.json.Value {
    var items = std.json.Array.init(allocator);
    const pattern = try std.fmt.allocPrint(allocator, "%{s}%", .{query});
    var stmt = try db.prepare("SELECT n.slug,n.title,n.tags,COALESCE(n.summary,'') FROM notes n WHERE n.title LIKE ?1 OR n.tags LIKE ?1 OR EXISTS(SELECT 1 FROM note_versions v WHERE v.note_id=n.id AND v.content LIKE ?1) ORDER BY n.updated_at DESC LIMIT ?2");
    defer stmt.deinit();
    try stmt.bindText(1, pattern);
    try stmt.bindInt(2, @intCast(@min(limit, 50)));
    while (try stmt.next()) try items.append(try json.object(allocator, &.{ .{ .key = "slug", .value = .{ .string = try allocator.dupe(u8, stmt.text(0)) } }, .{ .key = "title", .value = .{ .string = try allocator.dupe(u8, stmt.text(1)) } }, .{ .key = "tags", .value = .{ .string = try allocator.dupe(u8, stmt.text(2)) } }, .{ .key = "summary", .value = .{ .string = try allocator.dupe(u8, stmt.text(3)) } } }));
    return .{ .array = items };
}

fn createNoteMcp(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, title_raw: []const u8, content: []const u8, tags: []const u8) ![]const u8 {
    const title = std.mem.trim(u8, title_raw, " \t\r\n");
    if (title.len == 0 or title.len > 200 or content.len > 500000 or tags.len > 500) return error.InvalidParams;
    const slug = try slugify(allocator, title);
    const filename = try std.fmt.allocPrint(allocator, "{s}.md", .{slug});
    var stmt = try db.prepare("INSERT INTO notes(title,slug,filename,tags,word_count,summary) VALUES(?1,?2,?3,?4,?5,?6)");
    defer stmt.deinit();
    try stmt.bindText(1, title);
    try stmt.bindText(2, slug);
    try stmt.bindText(3, filename);
    try stmt.bindText(4, tags);
    try stmt.bindInt(5, wordCount(content));
    try stmt.bindText(6, content[0..@min(content.len, 240)]);
    _ = try stmt.next();
    const id = db.lastInsertRowId();
    var version = try db.prepare("INSERT INTO note_versions(note_id,content,word_count) VALUES(?1,?2,?3)");
    defer version.deinit();
    try version.bindInt(1, id);
    try version.bindText(2, content);
    try version.bindInt(3, wordCount(content));
    _ = try version.next();
    try std.fs.cwd().makePath(config.notes_dir);
    const file_path = try std.fs.path.join(allocator, &.{ config.notes_dir, filename });
    const file = try std.fs.cwd().createFile(file_path, .{ .truncate = true });
    defer file.close();
    try file.writeAll(try std.fmt.allocPrint(allocator, "---\ntitle: \"{s}\"\ntags: \"{s}\"\n---\n\n{s}", .{ title, tags, content }));
    return slug;
}

fn backups(allocator: std.mem.Allocator, config: *const config_mod.Config) !std.json.Value {
    const path = try backupDir(allocator, config);
    var list = std.json.Array.init(allocator);
    var dir = std.fs.cwd().openDir(path, .{ .iterate = true }) catch return json.object(allocator, &.{.{ .key = "backups", .value = .{ .array = list } }});
    defer dir.close();
    var iter = dir.iterate();
    while (try iter.next()) |entry| {
        if (entry.kind != .file or !std.mem.endsWith(u8, entry.name, ".db")) continue;
        const full = try std.fs.path.join(allocator, &.{ path, entry.name });
        const file = std.fs.cwd().openFile(full, .{}) catch continue;
        defer file.close();
        const size = (try file.stat()).size;
        try list.append(try json.object(allocator, &.{ .{ .key = "name", .value = .{ .string = try allocator.dupe(u8, entry.name) } }, .{ .key = "bytes", .value = .{ .integer = @intCast(size) } } }));
    }
    return json.object(allocator, &.{.{ .key = "backups", .value = .{ .array = list } }});
}

fn backupNow(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config) !std.json.Value {
    const dir = try backupDir(allocator, config);
    const stamp = try timestamp(allocator);
    const name = try std.fmt.allocPrint(allocator, "app-{s}.db", .{stamp});
    const path = try std.fs.path.join(allocator, &.{ dir, name });
    try db.backupTo(path);
    if (!try sqlite.Db.integrityCheckFile(allocator, path)) return error.SqliteError;
    const file = try std.fs.cwd().openFile(path, .{});
    defer file.close();
    const size = (try file.stat()).size;
    return json.object(allocator, &.{ .{ .key = "success", .value = .{ .bool = true } }, .{ .key = "name", .value = .{ .string = name } }, .{ .key = "bytes", .value = .{ .integer = @intCast(size) } }, .{ .key = "path", .value = .{ .string = path } }, .{ .key = "size_bytes", .value = .{ .integer = @intCast(size) } } });
}

fn aiCost(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config) !std.json.Value {
    var stmt = try db.prepare("SELECT COALESCE(SUM(cost_usd),0),COUNT(*) FROM usage_metrics WHERE created_at>=date('now','start of month')");
    defer stmt.deinit();
    _ = try stmt.next();
    const spent = stmt.float64(0);
    const count = stmt.int64(1);
    return json.object(allocator, &.{ .{ .key = "total_cost", .value = .{ .float = spent } }, .{ .key = "budget_limit", .value = .{ .float = config.openrouter_max_cost_usd } }, .{ .key = "requests_count", .value = .{ .integer = count } }, .{ .key = "alert", .value = .{ .bool = config.openrouter_max_cost_usd > 0 and spent >= config.openrouter_max_cost_usd * 0.9 } } });
}

fn backupDir(allocator: std.mem.Allocator, config: *const config_mod.Config) ![]const u8 {
    const parent = std.fs.path.dirname(config.db_path) orelse ".";
    const path = try std.fs.path.join(allocator, &.{ parent, "backups" });
    try std.fs.cwd().makePath(path);
    return path;
}

fn timestamp(allocator: std.mem.Allocator) ![]const u8 {
    const now = std.time.timestamp();
    const e = std.time.epoch.EpochSeconds{ .secs = @intCast(@max(0, now)) };
    const y = e.getEpochDay().calculateYearDay();
    const m = y.calculateMonthDay();
    const t = e.getDaySeconds();
    return std.fmt.allocPrint(allocator, "{d:0>4}{d:0>2}{d:0>2}-{d:0>2}{d:0>2}{d:0>2}", .{ y.year, m.month.numeric(), m.day_index + 1, t.getHoursIntoDay(), t.getMinutesIntoHour(), t.getSecondsIntoMinute() });
}

fn slugify(allocator: std.mem.Allocator, title: []const u8) ![]const u8 {
    var out: std.ArrayListUnmanaged(u8) = .empty;
    var dash = true;
    for (title) |byte| {
        if (std.ascii.isAlphanumeric(byte)) {
            try out.append(allocator, std.ascii.toLower(byte));
            dash = false;
        } else if (byte >= 0x80) {
            try out.append(allocator, byte);
            dash = false;
        } else if (!dash) {
            try out.append(allocator, '-');
            dash = true;
        }
    }
    while (out.items.len > 0 and out.items[out.items.len - 1] == '-') _ = out.pop();
    if (out.items.len == 0) return error.InvalidParams;
    return out.items;
}

fn safeSlug(slug: []const u8) bool {
    return slug.len > 0 and std.mem.indexOf(u8, slug, "..") == null and std.mem.indexOfAny(u8, slug, "/\\\x00") == null;
}

fn wordCount(text: []const u8) i64 {
    var n: i64 = 0;
    var inside = false;
    for (text) |c| {
        if (std.ascii.isWhitespace(c)) inside = false else if (!inside) {
            n += 1;
            inside = true;
        }
    }
    return n;
}

fn jsonRpcResponse(allocator: std.mem.Allocator, status: u16, id: ?std.json.Value, result: ?std.json.Value, rpc_error: ?RpcError) !http.Response {
    var obj = std.json.ObjectMap.init(allocator);
    try obj.put("jsonrpc", .{ .string = "2.0" });
    if (id) |v| try obj.put("id", v) else try obj.put("id", .null);
    if (result) |v| try obj.put("result", v);
    if (rpc_error) |e| try obj.put("error", try json.object(allocator, &.{ .{ .key = "code", .value = .{ .integer = e.code } }, .{ .key = "message", .value = .{ .string = e.message } } }));
    return jsonResponse(allocator, status, .{ .object = obj });
}

fn jsonResponse(allocator: std.mem.Allocator, status: u16, value: std.json.Value) !http.Response {
    return .{ .status = status, .content_type = "application/json; charset=utf-8", .body = try json.stringify(allocator, value) };
}

fn jsonError(allocator: std.mem.Allocator, status: u16, message: []const u8) !http.Response {
    return jsonResponse(allocator, status, try json.object(allocator, &.{.{ .key = "error", .value = .{ .string = message } }}));
}

test "MCP exposes the protocol version and the full tool catalog" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    const tools = try toolDefinitions(allocator);
    // 9 herramientas base + 6 de CyberHex + 13 de la plataforma (notas,
    // Office, calendario, informe y estado). Se comprueba que estén las
    // familias para que un cambio no deje el catálogo a medias.
    try std.testing.expectEqual(@as(usize, 28), tools.array.items.len);
    var has_notes = false;
    var has_cyberhex = false;
    var has_office = false;
    var has_calendar = false;
    var has_report = false;
    for (tools.array.items) |tool| {
        const name = json.getString(tool, "name") orelse "";
        if (std.mem.eql(u8, name, "search_notes")) has_notes = true;
        if (std.mem.eql(u8, name, "cyberhex_status")) has_cyberhex = true;
        if (std.mem.eql(u8, name, "office_write")) has_office = true;
        if (std.mem.eql(u8, name, "calendar_add")) has_calendar = true;
        if (std.mem.eql(u8, name, "student_report")) has_report = true;
    }
    try std.testing.expect(has_notes);
    try std.testing.expect(has_cyberhex);
    try std.testing.expect(has_office);
    try std.testing.expect(has_calendar);
    try std.testing.expect(has_report);
    const initialized = try dispatch(allocator, undefined, undefined, "initialize", try json.object(allocator, &.{}), null);
    try std.testing.expectEqualStrings("2025-03-26", json.getString(initialized, "protocolVersion").?);
}

test "las herramientas de CyberHex avisan sin configurar" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const config = testConfig(a);
    const payload = try callTool(a, undefined, &config, "cyberhex_status", try json.object(a, &.{}));
    try std.testing.expectEqual(@as(bool, false), json.getBool(payload, "connected").?);
    try std.testing.expectEqual(@as(bool, false), json.getBool(payload, "configured").?);
}

fn testConfig(a: std.mem.Allocator) config_mod.Config {
    return .{
        .allocator = a,
        .host = "127.0.0.1",
        .port = 8090,
        .session_secret = "0123456789abcdef0123456789abcdef",
        .db_path = "./data/app.db",
        .notes_dir = "./notes",
        .vault_dir = "./vault",
        .openrouter_api_key = null,
        .openrouter_model = "cohere/north-mini-code:free",
        .openrouter_max_tokens = 8192,
        .openrouter_max_cost_usd = 2.0,
        .mikareno_mcp_url = "http://127.0.0.1:8080/mcp",
        .mikareno_portal_url = "",
        .cyberhex_api_url = "",
        .cyberhex_api_key = "",
        .cyberhex_timeout_ms = 500,
        .google_client_id = "",
        .google_client_secret = "",
        .google_redirect_uri = "",
        .oauth_timeout_ms = 5000,
        .genoffice_mcp_url = "http://127.0.0.1:8453/mcp",
        .genoffice_work_dir = "",
        .genoffice_timeout_ms = 90000,
        .custom_mcp_api_key = null,
        .auth_disabled = true,
        .registration_token = null,
        .db_max_connections = 4,
        .mcp_timeout_ms = 5000,
    };
}
