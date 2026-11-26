const std = @import("std");

pub const Value = std.json.Value;
pub const Pair = struct { key: []const u8, value: Value };

pub fn object(allocator: std.mem.Allocator, pairs: []const Pair) !Value {
    var map = std.json.ObjectMap.init(allocator);
    for (pairs) |pair| try map.put(pair.key, pair.value);
    return .{ .object = map };
}

pub fn array(allocator: std.mem.Allocator, items: []const Value) !Value {
    var list = std.json.Array.init(allocator);
    try list.appendSlice(items);
    return .{ .array = list };
}

pub fn string(v: []const u8) Value {
    return .{ .string = v };
}

pub fn integer(v: anytype) Value {
    return .{ .integer = @intCast(v) };
}

pub fn float(v: f64) Value {
    return .{ .float = v };
}

pub fn boolean(v: bool) Value {
    return .{ .bool = v };
}

pub fn stringify(allocator: std.mem.Allocator, value: anytype) ![]const u8 {
    return std.json.stringifyAlloc(allocator, value, .{});
}

pub fn parse(allocator: std.mem.Allocator, input: []const u8) !Value {
    return std.json.parseFromSliceLeaky(Value, allocator, input, .{});
}

pub fn get(value: Value, key: []const u8) ?Value {
    if (value != .object) return null;
    return value.object.get(key);
}

pub fn getString(value: Value, key: []const u8) ?[]const u8 {
    const v = get(value, key) orelse return null;
    return if (v == .string) v.string else null;
}

pub fn getInt(value: Value, key: []const u8) ?i64 {
    const v = get(value, key) orelse return null;
    return if (v == .integer) v.integer else null;
}

pub fn getFloat(value: Value, key: []const u8) ?f64 {
    const v = get(value, key) orelse return null;
    return switch (v) {
        .float => v.float,
        .integer => @floatFromInt(v.integer),
        else => null,
    };
}

pub fn getBool(value: Value, key: []const u8) ?bool {
    const v = get(value, key) orelse return null;
    return if (v == .bool) v.bool else null;
}
