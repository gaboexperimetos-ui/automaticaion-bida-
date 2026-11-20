const std = @import("std");

const c = @cImport({
    @cInclude("sys/statvfs.h");
});

pub const Metrics = struct {
    ram_mb_used: i64 = 0,
    ram_mb_total: i64 = 0,
    disk_mb_free: i64 = 0,
};

/// Collect real host metrics. A metric remains zero only when its OS source
/// cannot be read; callers should expose that limitation rather than inventing
/// a value.
pub fn collect(allocator: std.mem.Allocator, data_path: []const u8) Metrics {
    var result = Metrics{};
    if (readMemory(allocator)) |memory| {
        result.ram_mb_total = @intCast(@min(memory.total_kib / 1024, std.math.maxInt(i64)));
        result.ram_mb_used = @intCast(@min(memory.used_kib / 1024, std.math.maxInt(i64)));
    } else |_| {}
    if (diskFreeBytes(allocator, data_path)) |bytes| {
        result.disk_mb_free = @intCast(@min(bytes / (1024 * 1024), std.math.maxInt(i64)));
    } else |_| {}
    return result;
}

const Memory = struct { total_kib: u64, used_kib: u64 };

fn readMemory(allocator: std.mem.Allocator) !Memory {
    const file = try std.fs.cwd().openFile("/proc/meminfo", .{});
    defer file.close();
    const contents = try file.readToEndAlloc(allocator, 64 * 1024);
    const total = meminfoKiB(contents, "MemTotal:") orelse return error.MissingMemTotal;
    const available = meminfoKiB(contents, "MemAvailable:") orelse return error.MissingMemAvailable;
    return .{ .total_kib = total, .used_kib = total -| available };
}

fn meminfoKiB(contents: []const u8, key: []const u8) ?u64 {
    var lines = std.mem.splitScalar(u8, contents, '\n');
    while (lines.next()) |line| {
        if (!std.mem.startsWith(u8, line, key)) continue;
        var fields = std.mem.tokenizeAny(u8, line[key.len..], " \t");
        const value = fields.next() orelse return null;
        const unit = fields.next() orelse return null;
        if (!std.mem.eql(u8, unit, "kB")) return null;
        return std.fmt.parseInt(u64, value, 10) catch null;
    }
    return null;
}

fn diskFreeBytes(allocator: std.mem.Allocator, path: []const u8) !u64 {
    const path_z = try allocator.dupeZ(u8, path);
    defer allocator.free(path_z);
    var stats: c.struct_statvfs = undefined;
    if (c.statvfs(path_z, &stats) != 0) return error.StatVfsFailed;
    const available: u64 = @intCast(stats.f_bavail);
    const fragment_size: u64 = @intCast(stats.f_frsize);
    return std.math.mul(u64, available, fragment_size) catch std.math.maxInt(u64);
}

test "meminfo parser reads numeric kB values without fabricating missing fields" {
    const sample = "MemTotal:       16384000 kB\nMemFree:         1000000 kB\nMemAvailable:    8192000 kB\n";
    try std.testing.expectEqual(@as(?u64, 16384000), meminfoKiB(sample, "MemTotal:"));
    try std.testing.expectEqual(@as(?u64, 8192000), meminfoKiB(sample, "MemAvailable:"));
    try std.testing.expectEqual(@as(?u64, null), meminfoKiB(sample, "SwapTotal:"));
}
