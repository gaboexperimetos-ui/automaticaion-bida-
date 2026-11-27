pub const head = @embedFile("../static/stitch-v2/shell/head.html");
pub const aside = @embedFile("../static/stitch-v2/shell/aside.html");
pub const topbar = @embedFile("../static/stitch-v2/shell/topbar.html");

pub const dashboard = @embedFile("../static/stitch-v2/_content/clean/index.html");
pub const calendar = @embedFile("../static/stitch-v2/_content/clean/calendar.html");
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
    .{ .path = "/static/app/charts.js", .mime = "application/javascript; charset=utf-8", .bytes = @embedFile("../static/app/charts.js") },
    .{ .path = "/static/app/audio.js", .mime = "application/javascript; charset=utf-8", .bytes = @embedFile("../static/app/audio.js") },
    .{ .path = "/static/stitch-v2/shell/head.html", .mime = "text/html; charset=utf-8", .bytes = head },
    .{ .path = "/static/stitch-v2/shell/aside.html", .mime = "text/html; charset=utf-8", .bytes = aside },
    .{ .path = "/static/stitch-v2/shell/topbar.html", .mime = "text/html; charset=utf-8", .bytes = topbar },
    .{ .path = "/static/stitch-v2/_content/clean/index.html", .mime = "text/html; charset=utf-8", .bytes = dashboard },
    .{ .path = "/static/stitch-v2/_content/clean/calendar.html", .mime = "text/html; charset=utf-8", .bytes = calendar },
    .{ .path = "/static/stitch-v2/_content/clean/asistente.html", .mime = "text/html; charset=utf-8", .bytes = assistant },
    .{ .path = "/static/stitch-v2/_content/clean/notes.html", .mime = "text/html; charset=utf-8", .bytes = notes },
    .{ .path = "/static/stitch-v2/_content/clean/mcp.html", .mime = "text/html; charset=utf-8", .bytes = mcp },
    .{ .path = "/static/stitch-v2/_content/clean/material.html", .mime = "text/html; charset=utf-8", .bytes = material },
    .{ .path = "/static/stitch-v2/_content/clean/settings.html", .mime = "text/html; charset=utf-8", .bytes = settings },
    .{ .path = "/static/study-platform-ui/index.html", .mime = "text/html; charset=utf-8", .bytes = @embedFile("../static/study-platform-ui/index.html") },
    .{ .path = "/static/study-platform-ui/styles.css", .mime = "text/css; charset=utf-8", .bytes = @embedFile("../static/study-platform-ui/styles.css") },
    .{ .path = "/static/study-platform-ui/app.js", .mime = "application/javascript; charset=utf-8", .bytes = @embedFile("../static/study-platform-ui/app.js") },
    .{ .path = "/vendor/htmx-2.0.3.min.js", .mime = "application/javascript; charset=utf-8", .bytes = @embedFile("../static/vendor/htmx-2.0.3.min.js") },
    .{ .path = "/vendor/xlsx-0.20.3.full.min.js", .mime = "application/javascript; charset=utf-8", .bytes = @embedFile("../static/vendor/xlsx-0.20.3.full.min.js") },
    .{ .path = "/calendar-ics.js", .mime = "application/javascript; charset=utf-8", .bytes = @embedFile("../static/app/calendar-ics.js") },
    .{ .path = "/office-sheets.js", .mime = "application/javascript; charset=utf-8", .bytes = @embedFile("../static/app/office-sheets.js") },
    .{ .path = "/office-html.js", .mime = "application/javascript; charset=utf-8", .bytes = @embedFile("../static/app/office-html.js") },
    .{ .path = "/static/app/themes.css", .mime = "text/css; charset=utf-8", .bytes = @embedFile("../static/app/themes.css") },
    .{ .path = "/static/app/music.css", .mime = "text/css; charset=utf-8", .bytes = @embedFile("../static/app/music.css") },
    .{ .path = "/static/app/tailwind.css", .mime = "text/css; charset=utf-8", .bytes = @embedFile("../static/app/tailwind.css") },
    .{ .path = "/fonts.css", .mime = "text/css; charset=utf-8", .bytes = @embedFile("../static/fonts.css") },
    .{ .path = "/fonts/inter-latin.woff2", .mime = "font/woff2", .bytes = @embedFile("../static/fonts/inter-latin.woff2") },
    .{ .path = "/fonts/inter-latin-ext.woff2", .mime = "font/woff2", .bytes = @embedFile("../static/fonts/inter-latin-ext.woff2") },
    .{ .path = "/fonts/manrope-latin.woff2", .mime = "font/woff2", .bytes = @embedFile("../static/fonts/manrope-latin.woff2") },
    .{ .path = "/fonts/manrope-latin-ext.woff2", .mime = "font/woff2", .bytes = @embedFile("../static/fonts/manrope-latin-ext.woff2") },
    .{ .path = "/fonts/space-grotesk-latin.woff2", .mime = "font/woff2", .bytes = @embedFile("../static/fonts/space-grotesk-latin.woff2") },
    .{ .path = "/fonts/space-grotesk-latin-ext.woff2", .mime = "font/woff2", .bytes = @embedFile("../static/fonts/space-grotesk-latin-ext.woff2") },
    .{ .path = "/fonts/dm-sans-latin.woff2", .mime = "font/woff2", .bytes = @embedFile("../static/fonts/dm-sans-latin.woff2") },
    .{ .path = "/fonts/dm-sans-latin-ext.woff2", .mime = "font/woff2", .bytes = @embedFile("../static/fonts/dm-sans-latin-ext.woff2") },
    .{ .path = "/fonts/material-symbols-outlined.woff2", .mime = "font/woff2", .bytes = @embedFile("../static/fonts/material-symbols-outlined.woff2") },
};

fn assetByPath(wanted: []const u8) ?Asset {
    for (files) |file| if (std.mem.eql(u8, wanted, file.path)) return file;
    return null;
}

var gzip_mutex = std.Thread.Mutex{};
var gzip_cache = [_]?[]const u8{null} ** files.len;

fn compressible(mime: []const u8) bool {
    return std.mem.startsWith(u8, mime, "text/") or
        std.mem.startsWith(u8, mime, "application/javascript") or
        std.mem.startsWith(u8, mime, "application/json") or
        std.mem.eql(u8, mime, "image/svg+xml");
}

fn compressGzip(data: []const u8) ?[]const u8 {
    var out = std.ArrayList(u8).init(std.heap.page_allocator);
    defer out.deinit();
    var stream = std.io.fixedBufferStream(data);
    std.compress.gzip.compress(stream.reader(), out.writer(), .{}) catch return null;
    return out.toOwnedSlice() catch null;
}

/// Comprime unos bytes arbitrarios (respuestas dinamicas como el tema activo).
pub fn gzip(data: []const u8) ?[]const u8 {
    return compressGzip(data);
}

fn fnv(bytes: []const u8) u64 {
    var hash: u64 = 0xcbf29ce484222325;
    for (bytes) |byte| {
        hash ^= byte;
        hash *%= 0x100000001b3;
    }
    return hash;
}

pub fn etagForBytes(bytes: []const u8) []const u8 {
    return std.fmt.allocPrint(std.heap.page_allocator, "\"{x}\"", .{fnv(bytes)}) catch "\"\"";
}

fn knownTheme(theme: []const u8) bool {
    const names = [_][]const u8{ "gruvbox", "osaka-jade", "tokyo-night", "catppuccin", "nord", "dracula", "rose-pine", "everforest", "solarized", "kanagawa", "nebula" };
    for (names) |name| if (std.mem.eql(u8, theme, name)) return true;
    return false;
}

/// Solo el bloque del tema activo mas el estilo comun. El resto del archivo
/// son las otras 10 paletas, que no se usan y bloquean el pintado.
pub fn themeCss(allocator: std.mem.Allocator, theme: []const u8) []const u8 {
    const css = assetByPath("/static/app/themes.css").?.bytes;
    const first = std.mem.indexOf(u8, css, "html[data-theme=\"") orelse return css;
    if (!knownTheme(theme)) return themeCss(allocator, "nebula");
    var buf: [64]u8 = undefined;
    const marker = std.fmt.bufPrint(&buf, "html[data-theme=\"{s}\"] {{", .{theme}) catch return css[0..first];
    const start = std.mem.indexOf(u8, css, marker) orelse return themeCss(allocator, "nebula");
    const close = std.mem.indexOf(u8, css[start..], "\n}\n") orelse return css[0..first];
    const end = start + close + 3;
    const prefix = css[0..first];
    const block = css[start..end];
    if (std.mem.eql(u8, theme, "nebula")) {
        const out = allocator.alloc(u8, prefix.len + block.len) catch return prefix;
        @memcpy(out[0..prefix.len], prefix);
        @memcpy(out[prefix.len..], block);
        return out;
    }
    const shared = std.mem.indexOf(u8, css[end..], "/* Estilo común") orelse {
        const out = allocator.alloc(u8, prefix.len + block.len) catch return prefix;
        @memcpy(out[0..prefix.len], prefix);
        @memcpy(out[prefix.len..], block);
        return out;
    };
    var tail = css[end + shared ..];
    // Las paletas CyberHex no estan en el selector de temas: se sirven solo
    // si el tema activo es una de ellas (hoy el selector cae a nebula).
    if (std.mem.indexOf(u8, tail, "html[data-theme=\"cyberhex-")) |cut| tail = tail[0..cut];
    const out = allocator.alloc(u8, prefix.len + block.len + tail.len) catch return prefix;
    @memcpy(out[0..prefix.len], prefix);
    @memcpy(out[prefix.len .. prefix.len + block.len], block);
    @memcpy(out[prefix.len + block.len ..], tail);
    return out;
}

pub fn indexOf(path: []const u8) ?usize {
    for (files, 0..) |file, i| if (std.mem.eql(u8, path, file.path)) return i;
    return null;
}

/// Version gzip de un asset embebido. Se genera una vez y se reutiliza.
pub fn gzipForPath(path: []const u8) ?[]const u8 {
    const index = indexOf(path) orelse return null;
    gzip_mutex.lock();
    defer gzip_mutex.unlock();
    if (gzip_cache[index]) |cached| return cached;
    if (!compressible(files[index].mime)) return null;
    const compressed = compressGzip(files[index].bytes) orelse return null;
    gzip_cache[index] = compressed;
    return compressed;
}

/// Precomprime al arrancar para que la primera pagina no pague el coste.
pub fn warmCompression() void {
    for (files) |file| _ = gzipForPath(file.path);
}

var etag_cache = [_]?[]const u8{null} ** files.len;

/// ETag estable por asset: cambia si cambian los bytes embebidos.
pub fn etagForPath(path: []const u8) ?[]const u8 {
    const index = indexOf(path) orelse return null;
    gzip_mutex.lock();
    defer gzip_mutex.unlock();
    if (etag_cache[index]) |cached| return cached;
    var hash: u64 = 0xcbf29ce484222325;
    for (files[index].bytes) |byte| {
        hash ^= byte;
        hash *%= 0x100000001b3;
    }
    const tag = std.fmt.allocPrint(std.heap.page_allocator, "\"{x}\"", .{hash}) catch return null;
    etag_cache[index] = tag;
    return tag;
}

pub fn find(path: []const u8) ?Asset {
    for (files) |file| if (std.mem.eql(u8, path, file.path)) return file;
    if (std.mem.eql(u8, path, "/favicon.svg") or std.mem.eql(u8, path, "/favicon.ico")) return files[0];
    if (std.mem.eql(u8, path, "/nova.css")) return files[1];
    if (std.mem.eql(u8, path, "/polish.css")) return files[2];
    if (std.mem.eql(u8, path, "/shell.js")) return files[3];
    if (std.mem.eql(u8, path, "/charts.js")) return assetByPath("/static/app/charts.js");
    if (std.mem.eql(u8, path, "/audio.js")) return assetByPath("/static/app/audio.js");
    if (std.mem.eql(u8, path, "/themes.css")) return assetByPath("/static/app/themes.css");
    if (std.mem.eql(u8, path, "/music.css")) return assetByPath("/static/app/music.css");
    if (std.mem.eql(u8, path, "/tailwind.css")) return assetByPath("/static/app/tailwind.css");
    if (std.mem.eql(u8, path, "/fonts.css")) return assetByPath("/fonts.css");
    if (std.mem.eql(u8, path, "/styles.css")) return assetByPath("/static/study-platform-ui/styles.css");
    if (std.mem.eql(u8, path, "/app.js")) return assetByPath("/static/study-platform-ui/app.js");
    if (std.mem.startsWith(u8, path, "/app-assets/")) {
        const suffix = path[12..];
        const canonical = std.fmt.allocPrint(std.heap.page_allocator, "/static/app/{s}", .{suffix}) catch return null;
        defer std.heap.page_allocator.free(canonical);
        for (files) |file| if (std.mem.eql(u8, canonical, file.path)) return file;
    }
    return null;
}

const std = @import("std");
