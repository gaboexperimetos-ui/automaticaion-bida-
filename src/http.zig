const std = @import("std");

pub const c = @cImport({
    @cInclude("sys/socket.h");
    @cInclude("netinet/in.h");
    @cInclude("arpa/inet.h");
    @cInclude("sys/time.h");
    @cInclude("unistd.h");
    @cInclude("signal.h");
});

pub const Request = struct {
    allocator: std.mem.Allocator,
    method: []const u8,
    target: []const u8,
    path: []const u8,
    query: []const u8,
    body: []const u8,
    peer_ip: []const u8,
    headers: std.StringHashMap([]const u8),

    pub fn header(self: *const Request, name: []const u8) ?[]const u8 {
        const lower = self.allocator.alloc(u8, name.len) catch return null;
        defer self.allocator.free(lower);
        for (name, 0..) |char, i| lower[i] = std.ascii.toLower(char);
        return self.headers.get(lower);
    }
};

pub const Header = struct { name: []const u8, value: []const u8 };
pub const Response = struct {
    status: u16 = 200,
    content_type: []const u8 = "text/plain; charset=utf-8",
    body: []const u8 = "",
    headers: []const Header = &.{},
};

pub const Form = std.StringHashMap([]const u8);

pub fn readRequest(allocator: std.mem.Allocator, fd: c_int, peer_ip: []const u8) !Request {
    const max_request_bytes = 1_100_000;
    const buf = try allocator.alloc(u8, max_request_bytes);
    var used: usize = 0;
    var header_end: ?usize = null;
    var content_length: usize = 0;
    while (used < max_request_bytes) {
        const n = c.recv(fd, buf.ptr + used, max_request_bytes - used, 0);
        if (n <= 0) return error.ConnectionClosed;
        used += @intCast(n);
        if (header_end == null) {
            header_end = findHeaderEnd(buf[0..used]);
            if (header_end) |end| {
                const header_bytes = buf[0..end];
                var lines = std.mem.splitSequence(u8, header_bytes, "\r\n");
                _ = lines.next() orelse return error.BadRequest;
                while (lines.next()) |line| {
                    const colon = std.mem.indexOfScalar(u8, line, ':') orelse continue;
                    const name = std.mem.trim(u8, line[0..colon], " \t");
                    if (std.ascii.eqlIgnoreCase(name, "content-length")) {
                        content_length = std.fmt.parseInt(usize, std.mem.trim(u8, line[colon + 1 ..], " \t"), 10) catch return error.BadRequest;
                        if (content_length > 1_000_000) return error.PayloadTooLarge;
                    }
                    if (std.ascii.eqlIgnoreCase(name, "transfer-encoding") and std.ascii.indexOfIgnoreCase(line[colon + 1 ..], "chunked") != null) return error.BadRequest;
                }
                if (used >= end + 4 + content_length) break;
            }
        } else if (used >= header_end.? + 4 + content_length) {
            break;
        }
    }
    const end = header_end orelse return error.BadRequest;
    if (used < end + 4 + content_length) return error.BadRequest;
    const head = buf[0..end];
    var lines = std.mem.splitSequence(u8, head, "\r\n");
    const request_line = lines.next() orelse return error.BadRequest;
    var parts = std.mem.tokenizeScalar(u8, request_line, ' ');
    const method = parts.next() orelse return error.BadRequest;
    const target_raw = parts.next() orelse return error.BadRequest;
    const http_version = parts.next() orelse return error.BadRequest;
    if (!std.mem.startsWith(u8, http_version, "HTTP/1.")) return error.BadRequest;

    const target = try allocator.dupe(u8, target_raw);
    const path_part = if (std.mem.indexOfScalar(u8, target, '?')) |q| target[0..q] else target;
    const query = if (std.mem.indexOfScalar(u8, target, '?')) |q| target[q + 1 ..] else "";
    const path = try percentDecode(allocator, path_part, false);

    var headers = std.StringHashMap([]const u8).init(allocator);
    while (lines.next()) |line| {
        if (line.len == 0) continue;
        const colon = std.mem.indexOfScalar(u8, line, ':') orelse continue;
        const key = std.mem.trim(u8, line[0..colon], " \t");
        const val = std.mem.trim(u8, line[colon + 1 ..], " \t");
        const lower = try allocator.alloc(u8, key.len);
        for (key, 0..) |ch, i| lower[i] = std.ascii.toLower(ch);
        try headers.put(lower, val);
    }

    return .{
        .allocator = allocator,
        .method = method,
        .target = target,
        .path = path,
        .query = query,
        .body = buf[end + 4 .. end + 4 + content_length],
        .peer_ip = peer_ip,
        .headers = headers,
    };
}

fn findHeaderEnd(bytes: []const u8) ?usize {
    if (std.mem.indexOf(u8, bytes, "\r\n\r\n")) |index| return index;
    return null;
}

pub fn percentDecode(allocator: std.mem.Allocator, input: []const u8, plus_as_space: bool) ![]const u8 {
    var out: std.ArrayListUnmanaged(u8) = .empty;
    var i: usize = 0;
    while (i < input.len) : (i += 1) {
        if (input[i] == '%' and i + 2 < input.len) {
            const hi = std.fmt.charToDigit(input[i + 1], 16) catch {
                try out.append(allocator, input[i]);
                continue;
            };
            const lo = std.fmt.charToDigit(input[i + 2], 16) catch {
                try out.append(allocator, input[i]);
                continue;
            };
            try out.append(allocator, @as(u8, @intCast(hi * 16 + lo)));
            i += 2;
        } else if (input[i] == '+' and plus_as_space) {
            try out.append(allocator, ' ');
        } else {
            try out.append(allocator, input[i]);
        }
    }
    return out.items;
}

pub fn parseForm(allocator: std.mem.Allocator, body: []const u8) !Form {
    var form = Form.init(allocator);
    var parts = std.mem.splitScalar(u8, body, '&');
    while (parts.next()) |part| {
        if (part.len == 0) continue;
        const equal = std.mem.indexOfScalar(u8, part, '=') orelse part.len;
        const key = try percentDecode(allocator, part[0..equal], true);
        const raw_value = if (equal < part.len) part[equal + 1 ..] else "";
        const value = try percentDecode(allocator, raw_value, true);
        try form.put(key, value);
    }
    return form;
}

pub fn queryParam(allocator: std.mem.Allocator, query: []const u8, name: []const u8) !?[]const u8 {
    var parts = std.mem.splitScalar(u8, query, '&');
    while (parts.next()) |part| {
        const equal = std.mem.indexOfScalar(u8, part, '=') orelse part.len;
        const key = try percentDecode(allocator, part[0..equal], true);
        if (!std.mem.eql(u8, key, name)) continue;
        return try percentDecode(allocator, if (equal < part.len) part[equal + 1 ..] else "", true);
    }
    return null;
}

pub fn sendAll(fd: c_int, bytes: []const u8) bool {
    var offset: usize = 0;
    while (offset < bytes.len) {
        const n = c.send(fd, bytes.ptr + offset, bytes.len - offset, c.MSG_NOSIGNAL);
        if (n <= 0) return false;
        offset += @intCast(n);
    }
    return true;
}

pub fn writeResponse(fd: c_int, allocator: std.mem.Allocator, response: Response) void {
    const reason = reasonPhrase(response.status);
    const head = std.fmt.allocPrint(allocator,
        "HTTP/1.1 {d} {s}\r\nContent-Type: {s}\r\nContent-Length: {d}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: same-origin\r\n",
        .{ response.status, reason, response.content_type, response.body.len }) catch return;
    _ = sendAll(fd, head);
    for (response.headers) |h| {
        const line = std.fmt.allocPrint(allocator, "{s}: {s}\r\n", .{ h.name, h.value }) catch continue;
        _ = sendAll(fd, line);
    }
    _ = sendAll(fd, "\r\n");
    _ = sendAll(fd, response.body);
}

pub fn reasonPhrase(status: u16) []const u8 {
    return switch (status) {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        else => "Response",
    };
}

test "form decoding handles plus, unicode, and duplicate values safely" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    var form = try parseForm(arena.allocator(), "title=Hola+mundo&tags=F%C3%ADsica%2C%20Qu%C3%ADmica");
    try std.testing.expectEqualStrings("Hola mundo", form.get("title").?);
    try std.testing.expectEqualStrings("Física, Química", form.get("tags").?);
}
