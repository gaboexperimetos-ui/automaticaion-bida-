const std = @import("std");

pub const Config = struct {
    allocator: std.mem.Allocator,
    host: []const u8,
    port: u16,
    session_secret: []const u8,
    db_path: []const u8,
    notes_dir: []const u8,
    vault_dir: []const u8,
    openrouter_api_key: ?[]const u8,
    openrouter_model: []const u8,
    openrouter_max_tokens: u32,
    openrouter_max_cost_usd: f64,
    mikareno_mcp_url: []const u8,
    mikareno_portal_url: []const u8,
    cyberhex_api_url: []const u8,
    cyberhex_api_key: []const u8,
    cyberhex_timeout_ms: u64,
    genoffice_mcp_url: []const u8,
    genoffice_work_dir: []const u8,
    genoffice_timeout_ms: u64,
    custom_mcp_api_key: ?[]const u8,
    auth_disabled: bool,
    registration_token: ?[]const u8,
    db_max_connections: u32,
    mcp_timeout_ms: u64,

    pub fn load(allocator: std.mem.Allocator) !Config {
        var vars = std.StringHashMap([]const u8).init(allocator);
        try loadDotEnv(allocator, &vars, ".env");

        const host = try value(allocator, &vars, "HOST", "127.0.0.1");
        const port = try parseInt(u16, allocator, &vars, "PORT", "8090");
        const secret = try value(allocator, &vars, "SESSION_SECRET", "");
        if (secret.len < 32) return error.SessionSecretRequired;
        const db_path = try value(allocator, &vars, "DB_PATH", "./data/app.db");
        const notes_dir = try value(allocator, &vars, "NOTES_DIR", "./notes");
        var vault_dir = try value(allocator, &vars, "VAULT_DIR", defaultVaultDir(allocator) catch "./vault");
        if (std.mem.trim(u8, vault_dir, " \t\r\n").len == 0) vault_dir = defaultVaultDir(allocator) catch "./vault";
        const openrouter_key = try optionalValue(allocator, &vars, "OPENROUTER_API_KEY");
        const openrouter_model = try value(allocator, &vars, "OPENROUTER_MODEL", "cohere/north-mini-code:free");
        const max_tokens = try parseInt(u32, allocator, &vars, "OPENROUTER_MAX_TOKENS", "8192");
        const max_cost = try parseFloat(allocator, &vars, "OPENROUTER_MAX_COST_USD", "2.0");
        const mikareno_url = try value(allocator, &vars, "MIKARENO_MCP_URL", "http://127.0.0.1:8080/mcp");
        const mikareno_portal = try value(allocator, &vars, "MIKARENO_PORTAL_URL", "https://alamos.mikareno.com/app/#/scores/student-score-table");
        // CyberHex solo se activa si hay URL y clave; si no, las herramientas
        // de música dicen que no está configurado en vez de fallar raro.
        const cyberhex_url = try value(allocator, &vars, "CYBERHEX_API_URL", "");
        const cyberhex_key = try value(allocator, &vars, "CYBERHEX_API_KEY", "");
        const cyberhex_timeout = try parseInt(u64, allocator, &vars, "CYBERHEX_TIMEOUT_MS", "4000");
        const genoffice_url = try value(allocator, &vars, "GENOFFICE_MCP_URL", "http://127.0.0.1:8453/mcp");
        const genoffice_work_dir = try value(allocator, &vars, "GENOFFICE_WORK_DIR", "");
        const genoffice_timeout_ms = try parseInt(u64, allocator, &vars, "GENOFFICE_TIMEOUT_MS", "90000");
        const mcp_key = try optionalValue(allocator, &vars, "CUSTOM_MCP_API_KEY");
        const auth = try value(allocator, &vars, "AUTH_DISABLED", "false");
        const register_token = try optionalValue(allocator, &vars, "REGISTRATION_TOKEN");
        const connections = @max(1, try parseInt(u32, allocator, &vars, "DB_MAX_CONNECTIONS", "4"));
        const mcp_timeout = try parseInt(u64, allocator, &vars, "MCP_TIMEOUT_MS", "5000");

        return .{
            .allocator = allocator,
            .host = host,
            .port = port,
            .session_secret = secret,
            .db_path = db_path,
            .notes_dir = notes_dir,
            .vault_dir = vault_dir,
            .openrouter_api_key = openrouter_key,
            .openrouter_model = openrouter_model,
            .openrouter_max_tokens = max_tokens,
            .openrouter_max_cost_usd = max_cost,
            .mikareno_mcp_url = mikareno_url,
            .mikareno_portal_url = mikareno_portal,
            .cyberhex_api_url = cyberhex_url,
            .cyberhex_api_key = cyberhex_key,
            .cyberhex_timeout_ms = cyberhex_timeout,
            .genoffice_mcp_url = genoffice_url,
            .genoffice_work_dir = genoffice_work_dir,
            .genoffice_timeout_ms = genoffice_timeout_ms,
            .custom_mcp_api_key = mcp_key,
            .auth_disabled = truthy(auth),
            .registration_token = register_token,
            .db_max_connections = connections,
            .mcp_timeout_ms = mcp_timeout,
        };
    }
};

fn loadDotEnv(allocator: std.mem.Allocator, vars: *std.StringHashMap([]const u8), path: []const u8) !void {
    const file = std.fs.cwd().openFile(path, .{}) catch return;
    defer file.close();
    const contents = try file.readToEndAlloc(allocator, 1024 * 1024);
    var lines = std.mem.splitScalar(u8, contents, '\n');
    while (lines.next()) |raw_line| {
        const line = std.mem.trim(u8, raw_line, " \t\r");
        if (line.len == 0 or line[0] == '#') continue;
        const eq = std.mem.indexOfScalar(u8, line, '=') orelse continue;
        const key = std.mem.trim(u8, line[0..eq], " \t");
        if (key.len == 0) continue;
        var val = std.mem.trim(u8, line[eq + 1 ..], " \t\r");
        if (val.len >= 2 and ((val[0] == '"' and val[val.len - 1] == '"') or (val[0] == '\'' and val[val.len - 1] == '\''))) {
            val = val[1 .. val.len - 1];
        }
        try vars.put(try allocator.dupe(u8, key), try allocator.dupe(u8, val));
    }
}

fn value(allocator: std.mem.Allocator, vars: *std.StringHashMap([]const u8), key: []const u8, default: []const u8) ![]const u8 {
    if (std.process.getEnvVarOwned(allocator, key)) |env_value| {
        const duplicated = try allocator.dupe(u8, env_value);
        allocator.free(env_value);
        return duplicated;
    } else |_| {}
    if (vars.get(key)) |v| return v;
    return allocator.dupe(u8, default);
}

fn optionalValue(allocator: std.mem.Allocator, vars: *std.StringHashMap([]const u8), key: []const u8) !?[]const u8 {
    const v = try value(allocator, vars, key, "");
    const trimmed = std.mem.trim(u8, v, " \t\r\n");
    if (trimmed.len == 0) return null;
    if (trimmed.ptr == v.ptr and trimmed.len == v.len) return v;
    return try allocator.dupe(u8, trimmed);
}

fn parseInt(comptime T: type, allocator: std.mem.Allocator, vars: *std.StringHashMap([]const u8), key: []const u8, default: []const u8) !T {
    const raw = try value(allocator, vars, key, default);
    return std.fmt.parseInt(T, raw, 10) catch return error.InvalidConfiguration;
}

fn parseFloat(allocator: std.mem.Allocator, vars: *std.StringHashMap([]const u8), key: []const u8, default: []const u8) !f64 {
    const raw = try value(allocator, vars, key, default);
    const n = std.fmt.parseFloat(f64, raw) catch return error.InvalidConfiguration;
    if (!std.math.isFinite(n) or n < 0) return error.InvalidConfiguration;
    return n;
}

pub fn truthy(s: []const u8) bool {
    return std.ascii.eqlIgnoreCase(s, "true") or std.ascii.eqlIgnoreCase(s, "yes") or std.ascii.eqlIgnoreCase(s, "on") or std.mem.eql(u8, s, "1");
}

/// Ruta por defecto del vault de Obsidian: `$HOME/SegundoCerebro` cuando se
/// puede resolver el directorio personal, `./vault` como respaldo.
pub fn defaultVaultDir(allocator: std.mem.Allocator) ![]const u8 {
    if (std.process.getEnvVarOwned(allocator, "HOME")) |home| {
        if (home.len > 0) {
            return std.fs.path.join(allocator, &.{ home, "SegundoCerebro" });
        }
    } else |_| {}
    return allocator.dupe(u8, "./vault");
}

test "truthy values are case insensitive" {
    try std.testing.expect(truthy("TRUE"));
    try std.testing.expect(truthy("yes"));
    try std.testing.expect(!truthy("no"));
}

test "defaultVaultDir apunta a $HOME/SegundoCerebro" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const dir = try defaultVaultDir(arena.allocator());
    try std.testing.expect(std.mem.endsWith(u8, dir, "/SegundoCerebro"));
}
