const std = @import("std");
const std_http = std.http;
const json = @import("jsonutil.zig");

const c = @cImport({
    @cInclude("sys/socket.h");
    @cInclude("sys/time.h");
});

const RpcResponse = struct {
    message: ?std.json.Value,
    session_id: ?[]const u8,
};

/// Calls a read-only or write tool on a configured MCP endpoint. Every call
/// performs a standards-compliant initialize/initialized/tools-call sequence;
/// the returned tool text is parsed as JSON when possible and otherwise kept
/// as plain text.
pub fn callTool(
    allocator: std.mem.Allocator,
    url: []const u8,
    timeout_ms: u64,
    tool_name: []const u8,
    arguments: std.json.Value,
    require_private_host: bool,
) !std.json.Value {
    if (url.len == 0) return error.NotConfigured;
    if (require_private_host and !allowsPrivateHost(allocator, url)) return error.HostNotAllowed;

    const init_params = try json.object(allocator, &.{
        .{ .key = "protocolVersion", .value = .{ .string = "2025-03-26" } },
        .{ .key = "capabilities", .value = try json.object(allocator, &.{}) },
        .{ .key = "clientInfo", .value = try json.object(allocator, &.{
            .{ .key = "name", .value = .{ .string = "study-platform-zig" } },
            .{ .key = "version", .value = .{ .string = "0.2.0" } },
        }) },
    });
    const initialized = try rpcRequest(allocator, url, timeout_ms, "initialize", init_params, 1, null, true);
    const init_message = initialized.message orelse return error.InvalidRpcResponse;
    if (json.get(init_message, "error") != null) return error.RemoteRpcError;
    const session_id = initialized.session_id;

    // Notifications are best effort: older stateless MCP servers may not
    // implement the post-initialize notification endpoint behavior.
    _ = rpcRequest(allocator, url, timeout_ms, "notifications/initialized", try json.object(allocator, &.{}), null, session_id, false) catch {};

    const call_params = try json.object(allocator, &.{
        .{ .key = "name", .value = .{ .string = tool_name } },
        .{ .key = "arguments", .value = arguments },
    });
    const response = try rpcRequest(allocator, url, timeout_ms, "tools/call", call_params, 2, session_id, true);
    const message = response.message orelse return error.InvalidRpcResponse;
    if (json.get(message, "error") != null) return error.RemoteRpcError;
    const result = json.get(message, "result") orelse return error.InvalidRpcResponse;
    if (json.getBool(result, "isError") orelse false) return error.ToolFailed;
    return unwrapToolResult(allocator, result);
}

/// Fast liveness check that performs only the MCP initialize handshake.
pub fn probe(allocator: std.mem.Allocator, url: []const u8, timeout_ms: u64, require_private_host: bool) !bool {
    if (url.len == 0) return false;
    if (require_private_host and !allowsPrivateHost(allocator, url)) return false;
    const params = try json.object(allocator, &.{
        .{ .key = "protocolVersion", .value = .{ .string = "2025-03-26" } },
        .{ .key = "capabilities", .value = try json.object(allocator, &.{}) },
        .{ .key = "clientInfo", .value = try json.object(allocator, &.{ .{ .key = "name", .value = .{ .string = "study-platform-zig" } }, .{ .key = "version", .value = .{ .string = "0.2.0" } } }) },
    });
    const response = rpcRequest(allocator, url, timeout_ms, "initialize", params, 1, null, true) catch return false;
    const message = response.message orelse return false;
    return json.get(message, "error") == null and json.get(message, "result") != null;
}

fn rpcRequest(
    allocator: std.mem.Allocator,
    url: []const u8,
    timeout_ms: u64,
    method: []const u8,
    params: std.json.Value,
    id: ?i64,
    session_id: ?[]const u8,
    expect_response: bool,
) !RpcResponse {
    const payload_value = if (id) |request_id|
        try json.object(allocator, &.{
            .{ .key = "jsonrpc", .value = .{ .string = "2.0" } },
            .{ .key = "id", .value = .{ .integer = request_id } },
            .{ .key = "method", .value = .{ .string = method } },
            .{ .key = "params", .value = params },
        })
    else
        try json.object(allocator, &.{
            .{ .key = "jsonrpc", .value = .{ .string = "2.0" } },
            .{ .key = "method", .value = .{ .string = method } },
            .{ .key = "params", .value = params },
        });
    const payload = try json.stringify(allocator, payload_value);

    var headers = std.ArrayList(std_http.Header).init(allocator);
    try headers.append(.{ .name = "Accept", .value = "application/json, text/event-stream" });
    if (session_id) |session| try headers.append(.{ .name = "mcp-session-id", .value = session });

    var client = std_http.Client{ .allocator = allocator };
    defer client.deinit();
    var response_header_buffer: [16 * 1024]u8 = undefined;
    const uri = try std.Uri.parse(url);
    var request = try client.open(.POST, uri, .{
        .server_header_buffer = &response_header_buffer,
        .keep_alive = false,
        .headers = .{
            .content_type = .{ .override = "application/json" },
            .connection = .{ .override = "close" },
            .user_agent = .{ .override = "study-platform-zig/0.2.0" },
        },
        .extra_headers = headers.items,
    });
    defer request.deinit();
    if (request.connection) |connection| setSocketTimeout(connection.stream.handle, timeout_ms);

    request.transfer_encoding = .{ .content_length = payload.len };
    try request.send();
    try request.writeAll(payload);
    try request.finish();
    try request.wait();
    const status: u16 = @intFromEnum(request.response.status);
    if (status < 200 or status >= 300) return error.HttpStatus;
    const response_session = headerValue(request.response.parser.get(), "mcp-session-id");
    const owned_session = if (response_session) |value| try allocator.dupe(u8, value) else session_id;
    if (!expect_response) return .{ .message = null, .session_id = owned_session };

    var body = std.ArrayList(u8).init(allocator);
    defer body.deinit();
    try request.reader().readAllArrayList(&body, 1024 * 1024);
    const parsed = try parseRpcMessage(allocator, body.items);
    return .{ .message = parsed, .session_id = owned_session };
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

fn headerValue(headers: []const u8, name: []const u8) ?[]const u8 {
    var lines = std.mem.splitSequence(u8, headers, "\r\n");
    _ = lines.next();
    while (lines.next()) |line| {
        const colon = std.mem.indexOfScalar(u8, line, ':') orelse continue;
        if (std.ascii.eqlIgnoreCase(std.mem.trim(u8, line[0..colon], " \t"), name)) {
            const value = std.mem.trim(u8, line[colon + 1 ..], " \t");
            return if (value.len > 0) value else null;
        }
    }
    return null;
}

fn parseRpcMessage(allocator: std.mem.Allocator, body: []const u8) !std.json.Value {
    const trimmed = std.mem.trim(u8, body, " \t\r\n");
    if (trimmed.len == 0) return error.EmptyRpcResponse;
    if (json.parse(allocator, trimmed)) |parsed| {
        return parsed;
    } else |_| {}

    // MCP servers may return a single JSON-RPC response in an SSE frame.
    var lines = std.mem.splitScalar(u8, trimmed, '\n');
    while (lines.next()) |line_raw| {
        const line = std.mem.trimRight(u8, line_raw, "\r");
        if (!std.mem.startsWith(u8, line, "data:")) continue;
        const data = std.mem.trim(u8, line[5..], " \t");
        if (data.len == 0 or std.mem.eql(u8, data, "[DONE]")) continue;
        if (json.parse(allocator, data)) |parsed| return parsed else |_| {}
    }
    return error.InvalidRpcResponse;
}

fn unwrapToolResult(allocator: std.mem.Allocator, result: std.json.Value) !std.json.Value {
    if (json.get(result, "structuredContent")) |structured| return structured;
    const content = json.get(result, "content") orelse return result;
    if (content != .array) return result;
    for (content.array.items) |entry| {
        if (!std.mem.eql(u8, json.getString(entry, "type") orelse "", "text")) continue;
        const text = json.getString(entry, "text") orelse continue;
        if (json.parse(allocator, text)) |parsed| return parsed else |_| return .{ .string = try allocator.dupe(u8, text) };
    }
    return result;
}

pub fn allowsPrivateHost(allocator: std.mem.Allocator, url: []const u8) bool {
    const uri = std.Uri.parse(url) catch return false;
    const host_component = uri.host orelse return false;
    const host = host_component.toRawMaybeAlloc(allocator) catch return false;
    if (std.ascii.eqlIgnoreCase(host, "localhost")) return true;
    const unbracketed = if (std.mem.startsWith(u8, host, "[") and std.mem.endsWith(u8, host, "]")) host[1 .. host.len - 1] else host;

    if (std.net.Ip4Address.parse(unbracketed, 0)) |ip| {
        const octets = std.mem.asBytes(&ip.sa.addr);
        return octets[0] == 10 or
            (octets[0] == 172 and octets[1] >= 16 and octets[1] <= 31) or
            (octets[0] == 192 and octets[1] == 168) or
            octets[0] == 127 or
            (octets[0] == 169 and octets[1] == 254) or
            (octets[0] == 100 and (octets[1] & 0xc0) == 0x40);
    } else |_| {}

    if (std.net.Ip6Address.parse(unbracketed, 0)) |ip| {
        const bytes = ip.sa.addr;
        const loopback = std.mem.allEqual(u8, bytes[0..15], 0) and bytes[15] == 1;
        return loopback or
            (bytes[0] & 0xfe) == 0xfc or
            (bytes[0] == 0xfe and (bytes[1] & 0xc0) == 0x80);
    } else |_| {}
    return false;
}

test "MCP response parser supports JSON and SSE frames" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    const parsed_json = try parseRpcMessage(allocator, "{\"jsonrpc\":\"2.0\",\"result\":{\"ok\":true}}");
    try std.testing.expect(json.get(parsed_json, "result") != null);
    const parsed_sse = try parseRpcMessage(allocator, "event: message\ndata: {\"jsonrpc\":\"2.0\",\"result\":{\"ok\":true}}\n\n");
    try std.testing.expect(json.get(parsed_sse, "result") != null);
}

test "MCP host allowlist accepts loopback, private and Tailscale addresses only" {
    try std.testing.expect(allowsPrivateHost(std.testing.allocator, "http://127.0.0.1:8080/mcp"));
    try std.testing.expect(allowsPrivateHost(std.testing.allocator, "http://10.0.0.4/mcp"));
    try std.testing.expect(allowsPrivateHost(std.testing.allocator, "http://100.115.62.97:8080/mcp"));
    try std.testing.expect(allowsPrivateHost(std.testing.allocator, "http://[::1]:8080/mcp"));
    try std.testing.expect(!allowsPrivateHost(std.testing.allocator, "https://8.8.8.8/mcp"));
    try std.testing.expect(!allowsPrivateHost(std.testing.allocator, "not-a-url"));
}
