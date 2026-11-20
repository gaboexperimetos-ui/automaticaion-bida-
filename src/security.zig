const std = @import("std");

pub fn constantTimeEq(a: []const u8, b: []const u8) bool {
    var diff: usize = a.len ^ b.len;
    const n = @max(a.len, b.len);
    for (0..n) |i| {
        const av: u8 = if (i < a.len) a[i] else 0;
        const bv: u8 = if (i < b.len) b[i] else 0;
        diff |= av ^ bv;
    }
    return diff == 0;
}

pub fn htmlEscape(allocator: std.mem.Allocator, input: []const u8) ![]const u8 {
    var out: std.ArrayListUnmanaged(u8) = .empty;
    for (input) |ch| {
        const replacement: []const u8 = switch (ch) {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&#39;",
            else => &.{ch},
        };
        try out.appendSlice(allocator, replacement);
    }
    return out.toOwnedSlice(allocator);
}

pub fn hexEncode(allocator: std.mem.Allocator, bytes: []const u8) ![]const u8 {
    const out = try allocator.alloc(u8, bytes.len * 2);
    const alphabet = "0123456789abcdef";
    for (bytes, 0..) |byte, i| {
        out[i * 2] = alphabet[byte >> 4];
        out[i * 2 + 1] = alphabet[byte & 0x0f];
    }
    return out;
}

pub fn generateSessionToken(allocator: std.mem.Allocator) ![]const u8 {
    var random: [32]u8 = undefined;
    std.crypto.random.bytes(&random);
    return hexEncode(allocator, &random);
}

pub fn hashSessionToken(allocator: std.mem.Allocator, secret: []const u8, token: []const u8) ![]const u8 {
    var hasher = std.crypto.hash.sha2.Sha256.init(.{});
    hasher.update(secret);
    hasher.update(":");
    hasher.update(token);
    var digest: [32]u8 = undefined;
    hasher.final(&digest);
    return hexEncode(allocator, &digest);
}

pub fn hashPassword(allocator: std.mem.Allocator, password: []const u8) ![]const u8 {
    // Parámetros por defecto del crate Rust original: Argon2id, 19 MiB, t=2.
    // Mantenerlos permite verificar las contraseñas ya guardadas en SQLite.
    var scratch: [256]u8 = undefined;
    const encoded = try std.crypto.pwhash.argon2.strHash(password, .{
        .allocator = allocator,
        .params = .{ .t = 2, .m = 19456, .p = 1 },
        .mode = .argon2id,
    }, &scratch);
    // `strHash` writes into the caller-owned output buffer; never return a
    // slice into this function's stack frame.
    return allocator.dupe(u8, encoded);
}

pub fn verifyPassword(allocator: std.mem.Allocator, encoded: []const u8, password: []const u8) bool {
    _ = allocator;
    std.crypto.pwhash.argon2.strVerify(encoded, password, .{ .allocator = std.heap.page_allocator }) catch return false;
    return true;
}

pub fn cookieValue(allocator: std.mem.Allocator, token: []const u8, secure: bool) ![]const u8 {
    return std.fmt.allocPrint(allocator, "session_id={s}; Path=/; HttpOnly; SameSite=Strict; Max-Age=86400{s}", .{ token, if (secure) "; Secure" else "" });
}

pub fn clearCookie(allocator: std.mem.Allocator, secure: bool) ![]const u8 {
    return std.fmt.allocPrint(allocator, "session_id=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{s}", .{if (secure) "; Secure" else ""});
}

test "constant time comparison and escaping" {
    try std.testing.expect(constantTimeEq("secret", "secret"));
    try std.testing.expect(!constantTimeEq("secret", "secreu"));
    try std.testing.expect(!constantTimeEq("a", "aa"));
    const escaped = try htmlEscape(std.testing.allocator, "<&\"'>");
    defer std.testing.allocator.free(escaped);
    try std.testing.expectEqualStrings("&lt;&amp;&quot;&#39;&gt;", escaped);
}

test "session token hash is stable and secret-dependent" {
    const a = try hashSessionToken(std.testing.allocator, "a-secret", "token");
    defer std.testing.allocator.free(a);
    const b = try hashSessionToken(std.testing.allocator, "a-secret", "token");
    defer std.testing.allocator.free(b);
    const c = try hashSessionToken(std.testing.allocator, "other-secret", "token");
    defer std.testing.allocator.free(c);
    try std.testing.expectEqualStrings(a, b);
    try std.testing.expect(!std.mem.eql(u8, a, c));
}

test "Argon2 password hash round-trips outside the stack frame" {
    const password = "correct horse battery staple";
    const encoded = try hashPassword(std.testing.allocator, password);
    defer std.testing.allocator.free(encoded);
    try std.testing.expect(std.mem.startsWith(u8, encoded, "$argon2id$v=19$m=19456,t=2,p=1$"));
    try std.testing.expect(verifyPassword(std.testing.allocator, encoded, password));
    try std.testing.expect(!verifyPassword(std.testing.allocator, encoded, "not the password"));
}
