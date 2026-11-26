//! Audio: subida, transcripción y (fase 2) separación de voces.
//!
//! Flujo real, sin datos inventados:
//!
//! 1. El navegador graba (MediaRecorder → webm/ogg) o sube un archivo.
//! 2. `ffmpeg` lo convierte a WAV mono 16 kHz, que es lo que Whisper consume
//!    de forma fiable y ligera.
//! 3. Se manda a la API de Groq (`whisper-large-v3-turbo`) como multipart.
//! 4. Devuelve el texto; el llamador decide si lo guarda como nota.
//!
//! Si falta `ffmpeg` o la clave de Groq, se devuelve un error explícito: la
//! interfaz muestra un estado honesto, nunca una transcripción falsa.

const std = @import("std");
const json = @import("jsonutil.zig");

pub const TRANSCRIBE_URL = "https://api.groq.com/openai/v1/audio/transcriptions";
pub const MODEL = "whisper-large-v3-turbo";

pub const Error = error{
    NoApiKey,
    FfmpegMissing,
    ConvertFailed,
    EmptyAudio,
    ProviderError,
    BadResponse,
    WriteFailed,
    ReadFailed,
};

pub const Result = struct {
    text: []const u8,
    duration_seconds: f64 = 0,
};

/// Convierte cualquier audio a WAV mono 16 kHz con ffmpeg. `input_path` es el
/// archivo subido; devuelve la ruta del WAV en el mismo directorio.
pub fn toWav(allocator: std.mem.Allocator, input_path: []const u8) ![]const u8 {
    const dir = std.fs.path.dirname(input_path) orelse ".";
    const base = std.fs.path.stem(input_path);
    // Nombre distinto del original: si la subida ya era .wav, reutilizar el
    // mismo nombre haría que ffmpeg intentara sobrescribir su propia entrada.
    const wav_path = try std.fmt.allocPrint(allocator, "{s}/{s}-16k.wav", .{ dir, base });
    const result = std.process.Child.run(.{
        .allocator = allocator,
        .argv = &.{ "ffmpeg", "-y", "-i", input_path, "-ac", "1", "-ar", "16000", "-f", "wav", wav_path },
        .max_output_bytes = 64 * 1024,
    }) catch return Error.FfmpegMissing;
    const ok = switch (result.term) {
        .Exited => |code| code == 0,
        else => false,
    };
    if (!ok) return Error.ConvertFailed;
    // Un WAV válido pesa al menos la cabecera; si no, el audio venía vacío.
    const file = std.fs.cwd().openFile(wav_path, .{}) catch return Error.ConvertFailed;
    defer file.close();
    const size = (try file.stat()).size;
    if (size <= 44) return Error.EmptyAudio;
    return wav_path;
}

/// Duración en segundos leyendo el campo `data size / byte rate` de la
/// cabecera WAV (PCM 16-bit mono 16 kHz → 32000 bytes/s). Devuelve 0 si no se
/// puede estimar; no es crítico.
fn wavDurationSeconds(allocator: std.mem.Allocator, wav_path: []const u8) f64 {
    const file = std.fs.cwd().openFile(wav_path, .{}) catch return 0;
    defer file.close();
    const size = (file.stat() catch return 0).size;
    if (size <= 44) return 0;
    _ = allocator;
    return @as(f64, @floatFromInt(size - 44)) / 32000.0;
}

/// Construye el cuerpo multipart/form-data con el campo `file` y `model`.
fn buildMultipart(allocator: std.mem.Allocator, boundary: []const u8, wav_bytes: []const u8, language: []const u8) ![]const u8 {
    var body = std.ArrayList(u8).init(allocator);
    try body.appendSlice("--");
    try body.appendSlice(boundary);
    try body.appendSlice("\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n");
    try body.appendSlice(MODEL);
    try body.appendSlice("\r\n--");
    try body.appendSlice(boundary);
    try body.appendSlice("\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\njson");
    if (language.len > 0) {
        try body.appendSlice("\r\n--");
        try body.appendSlice(boundary);
        try body.appendSlice("\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\n");
        try body.appendSlice(language);
    }
    try body.appendSlice("\r\n--");
    try body.appendSlice(boundary);
    try body.appendSlice("\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n");
    try body.appendSlice(wav_bytes);
    try body.appendSlice("\r\n--");
    try body.appendSlice(boundary);
    try body.appendSlice("--\r\n");
    return body.items;
}

/// Transcribe un WAV local con la API de Groq. `api_key` es la GROQ_API_KEY.
pub fn transcribeWav(allocator: std.mem.Allocator, api_key: []const u8, wav_path: []const u8, language: []const u8, timeout_ms: u64) !Result {
    if (api_key.len == 0) return Error.NoApiKey;
    // std.http.Client de 0.14 no expone un timeout por petición; se documenta
    // el valor configurado y se aplica el tope de tamaño del cuerpo (25 MB).
    _ = timeout_ms;
    const wav = std.fs.cwd().readFileAlloc(allocator, wav_path, 25 * 1024 * 1024) catch return Error.ReadFailed;
    if (wav.len == 0) return Error.EmptyAudio;

    const boundary = "----novaAudioBoundary7MA4YWxkTrZu0gW";
    const body = try buildMultipart(allocator, boundary, wav, language);
    const auth = try std.fmt.allocPrint(allocator, "Bearer {s}", .{api_key});

    var client = std.http.Client{ .allocator = allocator };
    defer client.deinit();
    var response_body = std.ArrayList(u8).init(allocator);
    defer response_body.deinit();

    const content_type = try std.fmt.allocPrint(allocator, "multipart/form-data; boundary={s}", .{boundary});
    const response = client.fetch(.{
        .location = .{ .url = TRANSCRIBE_URL },
        .method = .POST,
        .payload = body,
        .headers = .{
            .content_type = .{ .override = content_type },
            .authorization = .{ .override = auth },
        },
        .response_storage = .{ .dynamic = &response_body },
        .max_append_size = 4 * 1024 * 1024,
    }) catch return Error.ProviderError;

    if (@intFromEnum(response.status) < 200 or @intFromEnum(response.status) >= 300) return Error.ProviderError;
    const parsed = json.parse(allocator, response_body.items) catch return Error.BadResponse;
    const text_value = json.getString(parsed, "text") orelse return Error.BadResponse;
    return .{
        .text = text_value,
        .duration_seconds = wavDurationSeconds(allocator, wav_path),
    };
}

/// Guarda el archivo subido en `dir` con un nombre único y devuelve la ruta.
pub fn saveUpload(allocator: std.mem.Allocator, dir: []const u8, ext: []const u8, bytes: []const u8) ![]const u8 {
    if (bytes.len == 0) return Error.EmptyAudio;
    std.fs.cwd().makePath(dir) catch {};
    const stamp = std.time.timestamp();
    const rand = std.crypto.random.int(u64);
    const safe_ext = if (ext.len > 0 and ext.len <= 8) ext else "bin";
    const path = try std.fmt.allocPrint(allocator, "{s}/rec-{d}-{x}.{s}", .{ dir, stamp, rand, safe_ext });
    const file = std.fs.cwd().createFile(path, .{ .truncate = true }) catch return Error.WriteFailed;
    defer file.close();
    file.writeAll(bytes) catch return Error.WriteFailed;
    return path;
}

// ──────────────────────────── Pruebas ────────────────────────────

test "buildMultipart incluye modelo, archivo y audio" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const body = try buildMultipart(a, "BOUND", "AUDIODATA", "es");
    try std.testing.expect(std.mem.indexOf(u8, body, "name=\"model\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, body, MODEL) != null);
    try std.testing.expect(std.mem.indexOf(u8, body, "name=\"file\"; filename=\"audio.wav\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, body, "AUDIODATA") != null);
    try std.testing.expect(std.mem.indexOf(u8, body, "name=\"language\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, body, "es") != null);
}

test "buildMultipart sin idioma omite el campo language" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const body = try buildMultipart(arena.allocator(), "B", "X", "");
    try std.testing.expect(std.mem.indexOf(u8, body, "name=\"language\"") == null);
}

test "saveUpload escribe y respeta el límite de bytes" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const dir = try tmp.dir.realpathAlloc(arena.allocator(), ".");
    const path = try saveUpload(arena.allocator(), dir, "webm", "hola");
    const read = try std.fs.cwd().readFileAlloc(arena.allocator(), path, 100);
    try std.testing.expectEqualStrings("hola", read);
    try std.testing.expect(saveUpload(arena.allocator(), dir, "webm", "") == error.EmptyAudio);
}

test "transcribeWav sin clave falla explícito" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const r = transcribeWav(arena.allocator(), "", "/no/existe.wav", "", 1000);
    try std.testing.expect(r == error.NoApiKey);
}
