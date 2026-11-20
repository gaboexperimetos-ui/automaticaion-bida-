pub const head = @embedFile("../static/stitch-v2/shell/head.html");
pub const aside = @embedFile("../static/stitch-v2/shell/aside.html");
pub const topbar = @embedFile("../static/stitch-v2/shell/topbar.html");
pub const content_wrapper = @embedFile("../static/stitch-v2/shell/content_wrapper.html");

pub const dashboard = @embedFile("../static/stitch-v2/_content/clean/index.html");
pub const dashboard_alt = @embedFile("../static/stitch-v2/_content/clean/dashboard.html");
pub const assistant = @embedFile("../static/stitch-v2/_content/clean/asistente.html");
pub const notes = @embedFile("../static/stitch-v2/_content/clean/notes.html");
pub const mcp = @embedFile("../static/stitch-v2/_content/clean/mcp.html");
pub const material = @embedFile("../static/stitch-v2/_content/clean/material.html");
pub const settings = @embedFile("../static/stitch-v2/_content/clean/settings.html");

pub const Asset = struct { path: []const u8, mime: []const u8, bytes: []const u8 };
pub const files = [_]Asset{
    .{ .path = "/static/app/favicon.svg", .mime = "image/svg+xml", .bytes = @embedFile("../static/app/favicon.svg") },
    .{ .path = "/static/app/nova.css", .mime = "text/css; charset=utf-8", .bytes = @embedFile("../static/app/nova.css") },
    .{ .path = "/static/app/polish.css", .mime = "text/css; charset=utf-8", .bytes = @embedFile("../static/app/polish.css") },
    .{ .path = "/static/app/shell.js", .mime = "application/javascript; charset=utf-8", .bytes = @embedFile("../static/app/shell.js") },
    .{ .path = "/static/stitch-v2/shell/head.html", .mime = "text/html; charset=utf-8", .bytes = head },
    .{ .path = "/static/stitch-v2/shell/aside.html", .mime = "text/html; charset=utf-8", .bytes = aside },
    .{ .path = "/static/stitch-v2/shell/topbar.html", .mime = "text/html; charset=utf-8", .bytes = topbar },
    .{ .path = "/static/stitch-v2/shell/content_wrapper.html", .mime = "text/html; charset=utf-8", .bytes = content_wrapper },
    .{ .path = "/static/stitch-v2/_content/clean/index.html", .mime = "text/html; charset=utf-8", .bytes = dashboard },
    .{ .path = "/static/stitch-v2/_content/clean/dashboard.html", .mime = "text/html; charset=utf-8", .bytes = dashboard_alt },
    .{ .path = "/static/stitch-v2/_content/clean/asistente.html", .mime = "text/html; charset=utf-8", .bytes = assistant },
    .{ .path = "/static/stitch-v2/_content/clean/notes.html", .mime = "text/html; charset=utf-8", .bytes = notes },
    .{ .path = "/static/stitch-v2/_content/clean/mcp.html", .mime = "text/html; charset=utf-8", .bytes = mcp },
    .{ .path = "/static/stitch-v2/_content/clean/material.html", .mime = "text/html; charset=utf-8", .bytes = material },
    .{ .path = "/static/stitch-v2/_content/clean/settings.html", .mime = "text/html; charset=utf-8", .bytes = settings },
    .{ .path = "/static/study-platform-ui/index.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/study-platform-ui/index.html") },
    .{ .path = "/static/study-platform-ui/styles.css", .mime = "text/css; charset=utf-8", .bytes = @embedFile("../static/study-platform-ui/styles.css") },
    .{ .path = "/static/study-platform-ui/app.js", .mime = "application/javascript; charset=utf-8", .bytes = @embedFile("../static/study-platform-ui/app.js") },
    .{ .path = "/static/studyos/index.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/studyos/index.html") },
    .{ .path = "/static/studyos/styles.css", .mime = "text/css; charset=utf-8", .bytes = @embedFile("../static/studyos/styles.css") },
    .{ .path = "/static/studyos/app.js", .mime = "application/javascript; charset=utf-8", .bytes = @embedFile("../static/studyos/app.js") },
    .{ .path = "/static/stitch-v2/_content/asistente.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/stitch-v2/_content/asistente.html") },
    .{ .path = "/static/stitch-v2/_content/dashboard.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/stitch-v2/_content/dashboard.html") },
    .{ .path = "/static/stitch-v2/_content/index.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/stitch-v2/_content/index.html") },
    .{ .path = "/static/stitch-v2/_content/mcp.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/stitch-v2/_content/mcp.html") },
    .{ .path = "/static/stitch-v2/_content/notes.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/stitch-v2/_content/notes.html") },
    .{ .path = "/static/stitch-v2/asistente.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/stitch-v2/asistente.html") },
    .{ .path = "/static/stitch-v2/dashboard.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/stitch-v2/dashboard.html") },
    .{ .path = "/static/stitch-v2/index.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/stitch-v2/index.html") },
    .{ .path = "/static/stitch-v2/mcp.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/stitch-v2/mcp.html") },
    .{ .path = "/static/stitch-v2/notes.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/stitch-v2/notes.html") },
};

pub fn find(path: []const u8) ?Asset {
    for (files) |file| if (std.mem.eql(u8, path, file.path)) return file;
    if (std.mem.eql(u8, path, "/favicon.svg") or std.mem.eql(u8, path, "/favicon.ico")) return files[0];
    if (std.mem.eql(u8, path, "/nova.css")) return files[1];
    if (std.mem.eql(u8, path, "/polish.css")) return files[2];
    if (std.mem.eql(u8, path, "/shell.js")) return files[3];
    if (std.mem.eql(u8, path, "/styles.css")) return files[16];
    if (std.mem.eql(u8, path, "/app.js")) return files[17];
    if (std.mem.startsWith(u8, path, "/app-assets/")) {
        const suffix = path[12..];
        const canonical = std.fmt.allocPrint(std.heap.page_allocator, "/static/app/{s}", .{suffix}) catch return null;
        defer std.heap.page_allocator.free(canonical);
        for (files) |file| if (std.mem.eql(u8, canonical, file.path)) return file;
    }
    return null;
}

const std = @import("std");
