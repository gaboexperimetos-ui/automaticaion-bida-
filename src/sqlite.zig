const std = @import("std");

const DbHandle = opaque {};
const StmtHandle = opaque {};
const BackupHandle = opaque {};

extern "c" fn sqlite3_open_v2(filename: [*:0]const u8, db: *?*DbHandle, flags: c_int, vfs: ?[*:0]const u8) c_int;
extern "c" fn sqlite3_close_v2(db: *DbHandle) c_int;
extern "c" fn sqlite3_errmsg(db: *DbHandle) [*:0]const u8;
extern "c" fn sqlite3_exec(db: *DbHandle, sql: [*:0]const u8, callback: ?*const fn (?*anyopaque, c_int, [*]?[*:0]u8, [*]?[*:0]u8) callconv(.c) c_int, context: ?*anyopaque, errmsg: *?[*:0]u8) c_int;
extern "c" fn sqlite3_free(ptr: ?*anyopaque) void;
extern "c" fn sqlite3_prepare_v2(db: *DbHandle, sql: [*:0]const u8, nbyte: c_int, stmt: *?*StmtHandle, tail: ?*?[*:0]const u8) c_int;
extern "c" fn sqlite3_finalize(stmt: *StmtHandle) c_int;
extern "c" fn sqlite3_step(stmt: *StmtHandle) c_int;
extern "c" fn sqlite3_reset(stmt: *StmtHandle) c_int;
extern "c" fn sqlite3_clear_bindings(stmt: *StmtHandle) c_int;
extern "c" fn sqlite3_bind_text(stmt: *StmtHandle, index: c_int, value: [*]const u8, length: c_int, destructor: ?*const fn (?*anyopaque) callconv(.c) void) c_int;
extern "c" fn sqlite3_bind_null(stmt: *StmtHandle, index: c_int) c_int;
extern "c" fn sqlite3_bind_int64(stmt: *StmtHandle, index: c_int, value: i64) c_int;
extern "c" fn sqlite3_bind_double(stmt: *StmtHandle, index: c_int, value: f64) c_int;
extern "c" fn sqlite3_column_text(stmt: *StmtHandle, index: c_int) ?[*]const u8;
extern "c" fn sqlite3_column_bytes(stmt: *StmtHandle, index: c_int) c_int;
extern "c" fn sqlite3_column_int64(stmt: *StmtHandle, index: c_int) i64;
extern "c" fn sqlite3_column_double(stmt: *StmtHandle, index: c_int) f64;
extern "c" fn sqlite3_column_type(stmt: *StmtHandle, index: c_int) c_int;
extern "c" fn sqlite3_changes(db: *DbHandle) c_int;
extern "c" fn sqlite3_last_insert_rowid(db: *DbHandle) i64;
extern "c" fn sqlite3_backup_init(destination: *DbHandle, destination_name: [*:0]const u8, source: *DbHandle, source_name: [*:0]const u8) ?*BackupHandle;
extern "c" fn sqlite3_backup_step(backup: *BackupHandle, pages: c_int) c_int;
extern "c" fn sqlite3_backup_finish(backup: *BackupHandle) c_int;

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_NULL: c_int = 5;
const SQLITE_OPEN_READONLY: c_int = 0x00000001;
const SQLITE_OPEN_READWRITE: c_int = 0x00000002;
const SQLITE_OPEN_CREATE: c_int = 0x00000004;
const SQLITE_OPEN_FULLMUTEX: c_int = 0x00010000;
const transient: ?*const fn (?*anyopaque) callconv(.c) void = @ptrFromInt(std.math.maxInt(usize));

pub const Error = error{ SqliteError, OutOfMemory };

pub const Db = struct {
    handle: *DbHandle,
    allocator: std.mem.Allocator,
    path: []const u8,

    pub fn open(allocator: std.mem.Allocator, path: []const u8) !Db {
        if (!std.mem.eql(u8, path, ":memory:")) {
            if (std.fs.path.dirname(path)) |parent| {
                if (parent.len > 0) try std.fs.cwd().makePath(parent);
            }
        }
        const z_path = try allocator.dupeZ(u8, path);
        defer allocator.free(z_path);
        var raw: ?*DbHandle = null;
        const code = sqlite3_open_v2(z_path, &raw, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX, null);
        if (code != SQLITE_OK or raw == null) return error.SqliteError;
        var db = Db{ .handle = raw.?, .allocator = allocator, .path = path };
        errdefer _ = sqlite3_close_v2(db.handle);
        try db.exec("PRAGMA busy_timeout=5000;");
        try db.exec("PRAGMA foreign_keys=ON;");
        try db.exec("PRAGMA cache_size=-2000;");
        try db.exec("PRAGMA mmap_size=0;");
        try db.exec("PRAGMA temp_store=FILE;");
        try db.exec("PRAGMA wal_autocheckpoint=1000;");
        // WAL is persisted by SQLite. In-memory databases use memory mode.
        if (!std.mem.eql(u8, path, ":memory:")) try db.exec("PRAGMA journal_mode=WAL;");
        try db.exec("CREATE TABLE IF NOT EXISTS schema_migrations(version TEXT PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT (datetime('now')));");
        try db.migrate();
        return db;
    }

    pub fn close(self: *Db) void {
        _ = sqlite3_close_v2(self.handle);
    }

    pub fn exec(self: *Db, sql: []const u8) !void {
        const z_sql = try self.allocator.dupeZ(u8, sql);
        defer self.allocator.free(z_sql);
        var message: ?[*:0]u8 = null;
        const code = sqlite3_exec(self.handle, z_sql, null, null, &message);
        if (message) |text| sqlite3_free(@ptrCast(text));
        if (code != SQLITE_OK) return error.SqliteError;
    }

    pub fn prepare(self: *Db, sql: []const u8) !Statement {
        const z_sql = try self.allocator.dupeZ(u8, sql);
        defer self.allocator.free(z_sql);
        var raw: ?*StmtHandle = null;
        const code = sqlite3_prepare_v2(self.handle, z_sql, -1, &raw, null);
        if (code != SQLITE_OK or raw == null) return error.SqliteError;
        return .{ .db = self, .handle = raw.? };
    }

    pub fn changes(self: *Db) i64 {
        return sqlite3_changes(self.handle);
    }

    pub fn lastInsertRowId(self: *Db) i64 {
        return sqlite3_last_insert_rowid(self.handle);
    }

    pub fn errorMessage(self: *Db) []const u8 {
        return std.mem.span(sqlite3_errmsg(self.handle));
    }

    pub fn migrate(self: *Db) !void {
        // Bases creadas por la version Rust registran lo aplicado en
        // `_sqlx_migrations`, no en `schema_migrations`. Sin este puente el
        // runner Zig reintenta el init sobre tablas que ya existen y muere.
        try self.importLegacyMigrations();
        const migrations = [_]struct { version: []const u8, sql: []const u8 }{
            .{ .version = "20240101000000", .sql = @embedFile("../migrations/20240101000000_init.sql") },
            .{ .version = "20240101000001", .sql = @embedFile("../migrations/20240101000001_seed.sql") },
            .{ .version = "20240101000002", .sql = @embedFile("../migrations/20240101000002_knowledge.sql") },
            .{ .version = "20240101000003", .sql = @embedFile("../migrations/20240101000003_ai.sql") },
            .{ .version = "20240101000004", .sql = @embedFile("../migrations/20240101000004_sessions.sql") },
            .{ .version = "20240101000005", .sql = @embedFile("../migrations/20240101000005_usage_metrics.sql") },
            .{ .version = "20240101000006", .sql = @embedFile("../migrations/20240101000006_notes_search_fts.sql") },
            .{ .version = "20240101000007", .sql = @embedFile("../migrations/20240101000007_calendar_annotations.sql") },
            .{ .version = "20240101000008", .sql = @embedFile("../migrations/20240101000008_calendar_planner_category.sql") },
            .{ .version = "20240101000009", .sql = @embedFile("../migrations/20240101000009_oauth_google.sql") },
            .{ .version = "20240101000010", .sql = @embedFile("../migrations/20240101000010_google_oauth_tokens.sql") },
            .{ .version = "20240101000011", .sql = @embedFile("../migrations/20240101000011_flashcards.sql") },
            .{ .version = "20240101000012", .sql = @embedFile("../migrations/20240101000012_mikareno_cache.sql") },
        };
        for (migrations) |migration| {
            var applied = try self.prepare("SELECT 1 FROM schema_migrations WHERE version=?1");
            defer applied.deinit();
            try applied.bindText(1, migration.version);
            if (try applied.next()) continue;
            try self.exec("BEGIN IMMEDIATE;");
            self.exec(migration.sql) catch |err| {
                self.exec("ROLLBACK;") catch {};
                return err;
            };
            var record = self.prepare("INSERT INTO schema_migrations(version) VALUES(?1)") catch |err| {
                self.exec("ROLLBACK;") catch {};
                return err;
            };
            defer record.deinit();
            record.bindText(1, migration.version) catch |err| {
                self.exec("ROLLBACK;") catch {};
                return err;
            };
            _ = record.next() catch |err| {
                self.exec("ROLLBACK;") catch {};
                return err;
            };
            self.exec("COMMIT;") catch |err| {
                self.exec("ROLLBACK;") catch {};
                return err;
            };
        }
    }

    /// Copia a `schema_migrations` las versiones que la version Rust ya
    /// aplico (`_sqlx_migrations.success = 1`). Solo actua cuando
    /// `schema_migrations` esta vacia, asi que es idempotente y no pisa un
    /// historial propio.
    fn importLegacyMigrations(self: *Db) !void {
        const own = self.scalarInt("SELECT COUNT(*) FROM schema_migrations") catch return;
        if (own > 0) return;
        const legacy = self.scalarInt("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1") catch return;
        if (legacy == 0) return;
        try self.exec("INSERT OR IGNORE INTO schema_migrations(version) SELECT CAST(version AS TEXT) FROM _sqlx_migrations WHERE success = 1;");
    }

    pub fn scalarInt(self: *Db, sql: []const u8) !i64 {
        var stmt = try self.prepare(sql);
        defer stmt.deinit();
        if (!try stmt.next()) return error.SqliteError;
        return stmt.int64(0);
    }

    pub fn check(self: *Db) bool {
        return (self.scalarInt("SELECT 1") catch 0) == 1;
    }

    /// Create a consistent SQLite snapshot without copying a live WAL file.
    pub fn backupTo(self: *Db, destination_path: []const u8) !void {
        if (std.mem.eql(u8, self.path, destination_path)) return error.InvalidBackupPath;
        if (std.fs.path.dirname(destination_path)) |parent| {
            if (parent.len > 0) try std.fs.cwd().makePath(parent);
        }
        const path_z = try self.allocator.dupeZ(u8, destination_path);
        defer self.allocator.free(path_z);
        var destination: ?*DbHandle = null;
        const open_code = sqlite3_open_v2(path_z, &destination, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX, null);
        if (open_code != SQLITE_OK or destination == null) {
            if (destination) |handle| _ = sqlite3_close_v2(handle);
            return error.SqliteError;
        }
        defer _ = sqlite3_close_v2(destination.?);
        const backup = sqlite3_backup_init(destination.?, "main", self.handle, "main") orelse return error.SqliteError;
        const step_code = sqlite3_backup_step(backup, -1);
        const finish_code = sqlite3_backup_finish(backup);
        if (step_code != SQLITE_DONE or finish_code != SQLITE_OK) return error.SqliteError;
    }

    /// Restore a validated snapshot into the open database, preserving the
    /// live connection and its WAL rather than replacing the database file.
    pub fn restoreFrom(self: *Db, source_path: []const u8) !void {
        if (std.mem.eql(u8, self.path, source_path)) return error.InvalidBackupPath;
        const path_z = try self.allocator.dupeZ(u8, source_path);
        defer self.allocator.free(path_z);
        var source: ?*DbHandle = null;
        const open_code = sqlite3_open_v2(path_z, &source, SQLITE_OPEN_READONLY | SQLITE_OPEN_FULLMUTEX, null);
        if (open_code != SQLITE_OK or source == null) {
            if (source) |handle| _ = sqlite3_close_v2(handle);
            return error.SqliteError;
        }
        defer _ = sqlite3_close_v2(source.?);
        const backup = sqlite3_backup_init(self.handle, "main", source.?, "main") orelse return error.SqliteError;
        const step_code = sqlite3_backup_step(backup, -1);
        const finish_code = sqlite3_backup_finish(backup);
        if (step_code != SQLITE_DONE or finish_code != SQLITE_OK) return error.SqliteError;
    }

    /// Read-only integrity check used before restoring a backup.
    pub fn integrityCheckFile(allocator: std.mem.Allocator, path: []const u8) !bool {
        const path_z = try allocator.dupeZ(u8, path);
        defer allocator.free(path_z);
        var handle: ?*DbHandle = null;
        const open_code = sqlite3_open_v2(path_z, &handle, SQLITE_OPEN_READONLY | SQLITE_OPEN_FULLMUTEX, null);
        if (open_code != SQLITE_OK or handle == null) {
            if (handle) |db| _ = sqlite3_close_v2(db);
            return error.SqliteError;
        }
        defer _ = sqlite3_close_v2(handle.?);
        const sql_z = try allocator.dupeZ(u8, "PRAGMA integrity_check;");
        defer allocator.free(sql_z);
        var raw: ?*StmtHandle = null;
        if (sqlite3_prepare_v2(handle.?, sql_z, -1, &raw, null) != SQLITE_OK or raw == null) return error.SqliteError;
        defer _ = sqlite3_finalize(raw.?);
        if (sqlite3_step(raw.?) != SQLITE_ROW) return false;
        const text_ptr = sqlite3_column_text(raw.?, 0) orelse return false;
        const text_len: usize = @intCast(@max(sqlite3_column_bytes(raw.?, 0), 0));
        return std.mem.eql(u8, text_ptr[0..text_len], "ok");
    }
};

pub const Statement = struct {
    db: *Db,
    handle: *StmtHandle,

    pub fn deinit(self: *Statement) void {
        _ = sqlite3_finalize(self.handle);
    }

    pub fn bindText(self: *Statement, index: c_int, value: []const u8) !void {
        const result = sqlite3_bind_text(self.handle, index, value.ptr, @intCast(value.len), transient);
        if (result != SQLITE_OK) return error.SqliteError;
    }

    pub fn bindNull(self: *Statement, index: c_int) !void {
        if (sqlite3_bind_null(self.handle, index) != SQLITE_OK) return error.SqliteError;
    }

    pub fn bindInt(self: *Statement, index: c_int, value: i64) !void {
        if (sqlite3_bind_int64(self.handle, index, value) != SQLITE_OK) return error.SqliteError;
    }

    pub fn bindFloat(self: *Statement, index: c_int, value: f64) !void {
        if (sqlite3_bind_double(self.handle, index, value) != SQLITE_OK) return error.SqliteError;
    }

    /// `true` si hay fila disponible; `false` si SQLITE_DONE.
    pub fn next(self: *Statement) !bool {
        const result = sqlite3_step(self.handle);
        if (result == SQLITE_ROW) return true;
        if (result == SQLITE_DONE) return false;
        return error.SqliteError;
    }

    pub fn reset(self: *Statement) void {
        _ = sqlite3_reset(self.handle);
        _ = sqlite3_clear_bindings(self.handle);
    }

    pub fn text(self: *Statement, index: c_int) []const u8 {
        const ptr = sqlite3_column_text(self.handle, index) orelse return "";
        const len: usize = @intCast(@max(sqlite3_column_bytes(self.handle, index), 0));
        return ptr[0..len];
    }

    pub fn isNull(self: *Statement, index: c_int) bool {
        return sqlite3_column_type(self.handle, index) == SQLITE_NULL;
    }

    pub fn int64(self: *Statement, index: c_int) i64 {
        return sqlite3_column_int64(self.handle, index);
    }

    pub fn float64(self: *Statement, index: c_int) f64 {
        return sqlite3_column_double(self.handle, index);
    }
};

test "sqlite can migrate an in-memory database and run FTS5" {
    var db = try Db.open(std.testing.allocator, ":memory:");
    defer db.close();
    try std.testing.expect(db.check());
    try db.exec("INSERT INTO notes(title,slug,filename,tags) VALUES('Álgebra','algebra','algebra.md','matemáticas');");
    try std.testing.expectEqual(@as(i64, 1), try db.scalarInt("SELECT COUNT(*) FROM notes"));
}

test "el historial de migraciones Rust se importa sin re-ejecutar" {
    var db = try Db.open(std.testing.allocator, ":memory:");
    defer db.close();
    try db.exec("DELETE FROM schema_migrations;");
    try db.exec("CREATE TABLE _sqlx_migrations(version INTEGER PRIMARY KEY, description TEXT NOT NULL, success BOOLEAN NOT NULL);");
    try db.exec("INSERT INTO _sqlx_migrations(version, description, success) VALUES (20240101000000,'init',1),(20240101000001,'seed',1);");

    try db.importLegacyMigrations();
    try std.testing.expectEqual(@as(i64, 2), try db.scalarInt("SELECT COUNT(*) FROM schema_migrations"));

    // Idempotente: una segunda pasada no duplica ni pisa el historial.
    try db.importLegacyMigrations();
    try std.testing.expectEqual(@as(i64, 2), try db.scalarInt("SELECT COUNT(*) FROM schema_migrations"));
}
