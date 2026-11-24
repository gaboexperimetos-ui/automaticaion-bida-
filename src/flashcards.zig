//! Tarjetas de estudio con repetición espaciada (SM-2 simplificado).
//!
//! Todo es local: las tarjetas viven en SQLite y no tocan Mikareno ni Google.
//! El repaso calcula el próximo intervalo con cuatro respuestas
//! (`again`, `hard`, `good`, `easy`), igual que cualquier app de SRS.
//!
//! Reglas:
//!
//!  * Una tarjeta "de hoy" es la que tiene `due_at <= hoy`.
//!  * `again` la deja vencida para hoy y sube la facilidad solo un poco.
//!  * Nunca se inventan tarjetas: si no hay, la página dice que no hay.

const std = @import("std");
const sqlite = @import("sqlite.zig");
const http = @import("http.zig");
const json = @import("jsonutil.zig");
const security = @import("security.zig");
const panel = @import("panel.zig");

pub const Grade = enum { again, hard, good, easy };

pub const Scheduling = struct {
    interval_days: i64,
    ease: f64,
    reps: i64,
    lapses: i64,
};

pub const Card = struct {
    id: i64,
    front: []const u8,
    back: []const u8,
    subject: []const u8,
    due_at: []const u8,
    interval_days: i64,
    ease: f64,
    reps: i64,
    lapses: i64,
    created_at: []const u8,
};

pub const Stats = struct { total: i64, due: i64 };

// ── Fechas ────────────────────────────────────────────────────────────────

fn daysFromCivil(year: i32, month: u32, day: u32) i64 {
    const y: i64 = if (month <= 2) year - 1 else year;
    const era = @divFloor(if (y >= 0) y else y - 399, 400);
    const yoe: i64 = y - era * 400;
    const m: i64 = @intCast(month);
    const d: i64 = @intCast(day);
    const doy = @divFloor(153 * (if (m > 2) m - 3 else m + 9) + 2, 5) + d - 1;
    const doe = yoe * 365 + @divFloor(yoe, 4) - @divFloor(yoe, 100) + doy;
    return era * 146097 + doe - 719468;
}

const Civil = struct { year: i32, month: u32, day: u32 };

fn civilFromDays(days: i64) Civil {
    const z = days + 719468;
    const era = @divFloor(if (z >= 0) z else z - 146096, 146097);
    const doe: i64 = z - era * 146097;
    const yoe = @divFloor(doe - @divFloor(doe, 1460) + @divFloor(doe, 36524) - @divFloor(doe, 146096), 365);
    const y = yoe + era * 400;
    const doy = doe - (365 * yoe + @divFloor(yoe, 4) - @divFloor(yoe, 100));
    const mp = @divFloor(5 * doy + 2, 153);
    const d = doy - @divFloor(153 * mp + 2, 5) + 1;
    const m = if (mp < 10) mp + 3 else mp - 9;
    const year: i32 = @intCast(if (m <= 2) y + 1 else y);
    return .{ .year = year, .month = @intCast(m), .day = @intCast(d) };
}

fn parseIso(iso: []const u8) ?Civil {
    if (iso.len < 10) return null;
    const year = std.fmt.parseInt(i32, iso[0..4], 10) catch return null;
    const month = std.fmt.parseInt(u32, iso[5..7], 10) catch return null;
    const day = std.fmt.parseInt(u32, iso[8..10], 10) catch return null;
    if (month < 1 or month > 12 or day < 1 or day > 31) return null;
    return .{ .year = year, .month = month, .day = day };
}

pub fn shiftIso(allocator: std.mem.Allocator, iso: []const u8, delta: i64) ![]const u8 {
    const civil = parseIso(iso) orelse return allocator.dupe(u8, iso);
    const next = civilFromDays(daysFromCivil(civil.year, civil.month, civil.day) + delta);
    return std.fmt.allocPrint(allocator, "{d:0>4}-{d:0>2}-{d:0>2}", .{ @as(u32, @intCast(next.year)), next.month, next.day });
}

// ── Programación SM-2 ─────────────────────────────────────────────────────

pub fn schedule(prev_interval: i64, prev_ease: f64, prev_reps: i64, prev_lapses: i64, grade: Grade) Scheduling {
    var ease = if (prev_ease < 1.3) 1.3 else prev_ease;
    var reps = prev_reps;
    var lapses = prev_lapses;
    var interval = prev_interval;
    switch (grade) {
        .again => {
            reps = 0;
            lapses += 1;
            ease = @max(1.3, ease - 0.2);
            interval = 0;
        },
        .hard => {
            reps += 1;
            ease = @max(1.3, ease - 0.15);
            interval = if (prev_interval <= 0) 1 else @max(1, @as(i64, @intFromFloat(@round(@as(f64, @floatFromInt(prev_interval)) * 1.2))));
        },
        .good => {
            reps += 1;
            interval = if (prev_reps == 0) 1 else if (prev_reps == 1) 3 else @as(i64, @intFromFloat(@round(@as(f64, @floatFromInt(prev_interval)) * ease)));
        },
        .easy => {
            reps += 1;
            ease = @min(3.0, ease + 0.15);
            interval = if (prev_interval <= 0) 2 else @as(i64, @intFromFloat(@round(@as(f64, @floatFromInt(prev_interval)) * ease * 1.3)));
        },
    }
    if (interval > 365) interval = 365;
    if (interval < 0) interval = 0;
    return .{ .interval_days = interval, .ease = ease, .reps = reps, .lapses = lapses };
}

// ── Persistencia ──────────────────────────────────────────────────────────

const CARD_COLUMNS = "id,front,back,subject,due_at,interval_days,ease,reps,lapses,created_at";

fn cardFrom(stmt: *sqlite.Statement, allocator: std.mem.Allocator) !Card {
    return .{
        .id = stmt.int64(0),
        .front = try allocator.dupe(u8, stmt.text(1)),
        .back = try allocator.dupe(u8, stmt.text(2)),
        .subject = try allocator.dupe(u8, stmt.text(3)),
        .due_at = try allocator.dupe(u8, stmt.text(4)),
        .interval_days = stmt.int64(5),
        .ease = stmt.float64(6),
        .reps = stmt.int64(7),
        .lapses = stmt.int64(8),
        .created_at = try allocator.dupe(u8, stmt.text(9)),
    };
}

pub fn listCards(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    today: []const u8,
    due_only: bool,
    subject: ?[]const u8,
) ![]Card {
    var sql = std.ArrayList(u8).init(allocator);
    try sql.appendSlice("SELECT " ++ CARD_COLUMNS ++ " FROM flashcards");
    var has_where = false;
    if (due_only) {
        try sql.appendSlice(" WHERE due_at<=?1");
        has_where = true;
    }
    if (subject) |_| {
        try sql.appendSlice(if (has_where) " AND subject=?2" else " WHERE subject=?1");
    }
    try sql.appendSlice(" ORDER BY due_at,id");

    var stmt = db.prepare(sql.items) catch return &.{};
    defer stmt.deinit();
    if (due_only) try stmt.bindText(1, today);
    if (subject) |s| try stmt.bindText(if (due_only) 2 else 1, s);

    var out = std.ArrayList(Card).init(allocator);
    while (try stmt.next()) try out.append(try cardFrom(&stmt, allocator));
    return out.items;
}

pub fn getCard(allocator: std.mem.Allocator, db: *sqlite.Db, id: i64) !?Card {
    var stmt = db.prepare("SELECT " ++ CARD_COLUMNS ++ " FROM flashcards WHERE id=?1") catch return null;
    defer stmt.deinit();
    try stmt.bindInt(1, id);
    if (!try stmt.next()) return null;
    return try cardFrom(&stmt, allocator);
}

pub fn stats(db: *sqlite.Db, today: []const u8) !Stats {
    var total: i64 = 0;
    var due: i64 = 0;
    if (db.prepare("SELECT COUNT(*) FROM flashcards")) |stmt_const| {
        var stmt = stmt_const;
        defer stmt.deinit();
        if (try stmt.next()) total = stmt.int64(0);
    } else |_| {}
    if (db.prepare("SELECT COUNT(*) FROM flashcards WHERE due_at<=?1")) |stmt_const| {
        var stmt = stmt_const;
        defer stmt.deinit();
        try stmt.bindText(1, today);
        if (try stmt.next()) due = stmt.int64(0);
    } else |_| {}
    return .{ .total = total, .due = due };
}

fn insertCard(allocator: std.mem.Allocator, db: *sqlite.Db, front: []const u8, back: []const u8, subject: []const u8, today: []const u8) !i64 {
    _ = allocator;
    var stmt = try db.prepare("INSERT INTO flashcards(front,back,subject,due_at) VALUES(?1,?2,?3,?4)");
    defer stmt.deinit();
    try stmt.bindText(1, front);
    try stmt.bindText(2, back);
    try stmt.bindText(3, subject);
    try stmt.bindText(4, today);
    _ = try stmt.next();
    return db.lastInsertRowId();
}

/// Crea una tarjeta validada (usado por la API HTTP y por MCP).
pub fn createCard(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    front: []const u8,
    back: []const u8,
    subject: []const u8,
    today: []const u8,
) !i64 {
    const f = try trimmed(allocator, front);
    const b = try trimmed(allocator, back);
    const s = try trimmed(allocator, subject);
    if (f.len == 0 or b.len == 0) return error.InvalidParams;
    if (f.len > 1000 or b.len > 4000 or s.len > 120) return error.InvalidParams;
    return insertCard(allocator, db, f, b, s, today);
}

pub fn reviewCard(
    allocator: std.mem.Allocator,
    db: *sqlite.Db,
    id: i64,
    grade: Grade,
    today: []const u8,
) !?Card {
    var sel = db.prepare("SELECT interval_days,ease,reps,lapses FROM flashcards WHERE id=?1") catch return null;
    defer sel.deinit();
    try sel.bindInt(1, id);
    if (!try sel.next()) return null;
    const next = schedule(sel.int64(0), sel.float64(1), sel.int64(2), sel.int64(3), grade);
    const due = try shiftIso(allocator, today, next.interval_days);

    var upd = try db.prepare(
        "UPDATE flashcards SET interval_days=?1,ease=?2,reps=?3,lapses=?4,due_at=?5,last_reviewed=?6,updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?7",
    );
    defer upd.deinit();
    try upd.bindInt(1, next.interval_days);
    try upd.bindFloat(2, next.ease);
    try upd.bindInt(3, next.reps);
    try upd.bindInt(4, next.lapses);
    try upd.bindText(5, due);
    try upd.bindText(6, today);
    try upd.bindInt(7, id);
    _ = try upd.next();
    return try getCard(allocator, db, id);
}

// ── API HTTP ──────────────────────────────────────────────────────────────

pub fn handles(path: []const u8) bool {
    return std.mem.eql(u8, path, "/api/flashcards") or std.mem.startsWith(u8, path, "/api/flashcards/");
}

fn jsonError(allocator: std.mem.Allocator, status: u16, message: []const u8) !http.Response {
    return .{
        .status = status,
        .content_type = "application/json; charset=utf-8",
        .body = try json.stringify(allocator, try json.object(allocator, &.{.{ .key = "error", .value = .{ .string = message } }})),
    };
}

fn jsonOk(allocator: std.mem.Allocator, status: u16, value: std.json.Value) !http.Response {
    return .{ .status = status, .content_type = "application/json; charset=utf-8", .body = try json.stringify(allocator, value) };
}

pub fn cardValue(allocator: std.mem.Allocator, c: Card) !std.json.Value {
    return json.object(allocator, &.{
        .{ .key = "id", .value = json.integer(c.id) },
        .{ .key = "front", .value = json.string(c.front) },
        .{ .key = "back", .value = json.string(c.back) },
        .{ .key = "subject", .value = json.string(c.subject) },
        .{ .key = "due_at", .value = json.string(c.due_at) },
        .{ .key = "interval_days", .value = json.integer(c.interval_days) },
        .{ .key = "ease", .value = json.float(c.ease) },
        .{ .key = "reps", .value = json.integer(c.reps) },
        .{ .key = "lapses", .value = json.integer(c.lapses) },
        .{ .key = "created_at", .value = json.string(c.created_at) },
    });
}

pub fn parseGrade(payload: std.json.Value) ?Grade {
    if (json.getString(payload, "grade")) |s| {
        if (std.ascii.eqlIgnoreCase(s, "again") or std.mem.eql(u8, s, "0")) return .again;
        if (std.ascii.eqlIgnoreCase(s, "hard") or std.mem.eql(u8, s, "1")) return .hard;
        if (std.ascii.eqlIgnoreCase(s, "good") or std.mem.eql(u8, s, "2")) return .good;
        if (std.ascii.eqlIgnoreCase(s, "easy") or std.mem.eql(u8, s, "3")) return .easy;
    }
    if (json.getInt(payload, "grade")) |n| return switch (n) {
        0 => .again,
        1 => .hard,
        2 => .good,
        3 => .easy,
        else => null,
    };
    return null;
}

fn trimmed(allocator: std.mem.Allocator, raw: []const u8) ![]const u8 {
    return allocator.dupe(u8, std.mem.trim(u8, raw, " \t\r\n"));
}

pub fn handle(allocator: std.mem.Allocator, db: *sqlite.Db, request: *const http.Request) !http.Response {
    const path = request.path;
    const today = try panel.todayIso(allocator);
    const method = request.method;

    if (std.mem.eql(u8, path, "/api/flashcards") or std.mem.eql(u8, path, "/api/flashcards/")) {
        if (std.mem.eql(u8, method, "GET")) {
            var due_only = false;
            if (try http.queryParam(allocator, request.query, "due")) |v| {
                due_only = std.mem.eql(u8, v, "1") or std.ascii.eqlIgnoreCase(v, "true");
            }
            var subject: ?[]const u8 = null;
            if (try http.queryParam(allocator, request.query, "subject")) |v| {
                if (v.len > 0) subject = v;
            }
            const cards = try listCards(allocator, db, today, due_only, subject);
            var arr = std.json.Array.init(allocator);
            for (cards) |c| try arr.append(try cardValue(allocator, c));
            const total = try stats(db, today);
            return jsonOk(allocator, 200, try json.object(allocator, &.{
                .{ .key = "count", .value = json.integer(arr.items.len) },
                .{ .key = "total", .value = json.integer(total.total) },
                .{ .key = "due", .value = json.integer(total.due) },
                .{ .key = "cards", .value = .{ .array = arr } },
            }));
        }
        if (std.mem.eql(u8, method, "POST")) {
            const payload = json.parse(allocator, request.body) catch return jsonError(allocator, 400, "JSON no válido");
            const id = createCard(
                allocator,
                db,
                json.getString(payload, "front") orelse "",
                json.getString(payload, "back") orelse "",
                json.getString(payload, "subject") orelse "",
                today,
            ) catch |err| switch (err) {
                error.InvalidParams => return jsonError(allocator, 400, "Frente y reverso obligatorios (máx. 1000 y 4000 caracteres)"),
                else => return err,
            };
            const saved = (try getCard(allocator, db, id)) orelse return jsonError(allocator, 500, "No se pudo leer la tarjeta");
            return jsonOk(allocator, 201, try cardValue(allocator, saved));
        }
        return jsonError(allocator, 405, "Método no permitido");
    }

    const rest = path["/api/flashcards/".len..];
    if (std.mem.eql(u8, rest, "stats")) {
        if (!std.mem.eql(u8, method, "GET")) return jsonError(allocator, 405, "Método no permitido");
        const s = try stats(db, today);
        return jsonOk(allocator, 200, try json.object(allocator, &.{
            .{ .key = "total", .value = json.integer(s.total) },
            .{ .key = "due", .value = json.integer(s.due) },
        }));
    }

    if (std.mem.endsWith(u8, rest, "/review")) {
        if (!std.mem.eql(u8, method, "POST")) return jsonError(allocator, 405, "Método no permitido");
        const id_txt = rest[0 .. rest.len - "/review".len];
        const id = std.fmt.parseInt(i64, id_txt, 10) catch return jsonError(allocator, 404, "Tarjeta no encontrada");
        const payload = json.parse(allocator, request.body) catch return jsonError(allocator, 400, "JSON no válido");
        const grade = parseGrade(payload) orelse return jsonError(allocator, 400, "Calificación no válida (again|hard|good|easy)");
        const card = (try reviewCard(allocator, db, id, grade, today)) orelse return jsonError(allocator, 404, "Tarjeta no encontrada");
        return jsonOk(allocator, 200, try cardValue(allocator, card));
    }

    const id = std.fmt.parseInt(i64, rest, 10) catch return jsonError(allocator, 404, "Tarjeta no encontrada");

    if (std.mem.eql(u8, method, "GET")) {
        const card = (try getCard(allocator, db, id)) orelse return jsonError(allocator, 404, "Tarjeta no encontrada");
        return jsonOk(allocator, 200, try cardValue(allocator, card));
    }
    if (std.mem.eql(u8, method, "PUT")) {
        const payload = json.parse(allocator, request.body) catch return jsonError(allocator, 400, "JSON no válido");
        const front = json.getString(payload, "front");
        const back = json.getString(payload, "back");
        const subject = json.getString(payload, "subject");
        if (front == null and back == null and subject == null) return jsonError(allocator, 400, "Nada que actualizar");
        var stmt = try db.prepare(
            "UPDATE flashcards SET front=COALESCE(?1,front),back=COALESCE(?2,back),subject=COALESCE(?3,subject),updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?4",
        );
        defer stmt.deinit();
        if (front) |v| try stmt.bindText(1, try trimmed(allocator, v)) else try stmt.bindNull(1);
        if (back) |v| try stmt.bindText(2, try trimmed(allocator, v)) else try stmt.bindNull(2);
        if (subject) |v| try stmt.bindText(3, try trimmed(allocator, v)) else try stmt.bindNull(3);
        try stmt.bindInt(4, id);
        _ = try stmt.next();
        if (db.changes() == 0) return jsonError(allocator, 404, "Tarjeta no encontrada");
        const card = (try getCard(allocator, db, id)) orelse return jsonError(allocator, 404, "Tarjeta no encontrada");
        return jsonOk(allocator, 200, try cardValue(allocator, card));
    }
    if (std.mem.eql(u8, method, "DELETE")) {
        var stmt = try db.prepare("DELETE FROM flashcards WHERE id=?1");
        defer stmt.deinit();
        try stmt.bindInt(1, id);
        _ = try stmt.next();
        if (db.changes() == 0) return jsonError(allocator, 404, "Tarjeta no encontrada");
        return jsonOk(allocator, 200, try json.object(allocator, &.{
            .{ .key = "deleted", .value = json.boolean(true) },
            .{ .key = "id", .value = json.integer(id) },
        }));
    }
    return jsonError(allocator, 405, "Método no permitido");
}

// ── Página ────────────────────────────────────────────────────────────────

const review_script =
    \\<script>(function(){
    \\var root=document.querySelector('.nx-flash-page');if(!root)return;
    \\var startBtn=document.getElementById('nx-flash-start');
    \\var box=document.getElementById('nx-flash-box');
    \\var frontEl=document.getElementById('nx-flash-front');
    \\var backEl=document.getElementById('nx-flash-back');
    \\var revealBtn=document.getElementById('nx-flash-reveal');
    \\var grades=document.getElementById('nx-flash-grades');
    \\var progress=document.getElementById('nx-flash-progress');
    \\var done=document.getElementById('nx-flash-done');
    \\var status=document.getElementById('nx-flash-status');
    \\if(!frontEl||!backEl||!revealBtn||!grades||!progress||!done)return;
    \\var queue=[],pos=0,current=null;
    \\function post(url,body){return fetch(url,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});}
    \\function load(){
    \\if(status)status.textContent='Cargando…';
    \\fetch('/api/flashcards?due=1').then(function(r){return r.json();}).then(function(d){
    \\queue=(d.cards||[]).slice();pos=0;
    \\if(!queue.length){if(status)status.textContent='No hay tarjetas para hoy.';if(startBtn)startBtn.disabled=true;return;}
    \\if(box)box.hidden=false;if(startBtn)startBtn.disabled=true;show();
    \\}).catch(function(e){if(status)status.textContent='No se pudieron cargar: '+e.message;});
    \\}
    \\function show(){
    \\done.hidden=true;grades.hidden=true;backEl.hidden=true;revealBtn.hidden=false;
    \\if(pos>=queue.length){current=null;frontEl.textContent='';backEl.textContent='';revealBtn.hidden=true;done.hidden=false;progress.textContent='';return;}
    \\current=queue[pos];progress.textContent='Tarjeta '+(pos+1)+' de '+queue.length;
    \\frontEl.textContent=current.front;backEl.textContent=current.back;status.textContent='';
    \\}
    \\function reveal(){backEl.hidden=false;revealBtn.hidden=true;grades.hidden=false;}
    \\function grade(g){
    \\if(!current)return;var card=current;
    \\post('/api/flashcards/'+card.id+'/review',{grade:g}).then(function(r){if(!r.ok)throw new Error('HTTP '+r.status);
    \\if(g==='again')queue.push(card);pos++;show();
    \\}).catch(function(e){if(status)status.textContent='No se pudo guardar: '+e.message;});
    \\}
    \\if(startBtn)startBtn.addEventListener('click',load);
    \\revealBtn.addEventListener('click',reveal);
    \\Array.prototype.forEach.call(grades.querySelectorAll('button'),function(b){b.addEventListener('click',function(){grade(b.getAttribute('data-grade'));});});
    \\var form=document.getElementById('nx-flash-form');
    \\if(form){form.addEventListener('submit',function(e){e.preventDefault();
    \\var fd=new FormData(form);var body={front:(fd.get('front')||'').trim(),back:(fd.get('back')||'').trim(),subject:(fd.get('subject')||'').trim()};
    \\var st=document.getElementById('nx-flash-form-status');
    \\if(!body.front||!body.back){if(st)st.textContent='Frente y reverso son obligatorios.';return;}
    \\post('/api/flashcards',body).then(function(r){if(!r.ok)throw new Error('HTTP '+r.status);location.reload();})
    \\.catch(function(err){if(st)st.textContent='No se pudo guardar: '+err.message;});});}
    \\root.addEventListener('click',function(e){var b=e.target.closest('.nx-flash-del');if(!b)return;var id=b.getAttribute('data-id');if(!id)return;
    \\fetch('/api/flashcards/'+id,{method:'DELETE'}).then(function(r){if(!r.ok)throw new Error('HTTP '+r.status);location.reload();})
    \\.catch(function(err){alert('No se pudo eliminar: '+err.message);});});
    \\})();</script>
;

pub fn renderPage(allocator: std.mem.Allocator, db: *sqlite.Db, today: []const u8) ![]const u8 {
    const s = try stats(db, today);
    const cards = try listCards(allocator, db, today, false, null);

    var body = std.ArrayList(u8).init(allocator);
    try body.appendSlice("<div class=\"nx-page nova-page nx-flash-page\">");
    try body.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<header class=\"nx-flash-head\"><div><p class=\"nx-eyebrow\">Estudio</p><h1>Tarjetas</h1>" ++
            "<p class=\"nx-cal-sub\">Repetición espaciada con tus propios apuntes. Todo local, sin cuenta externa.</p></div>" ++
            "<div class=\"nx-flash-stats\"><span class=\"nx-flash-stat\"><strong>{d}</strong><small>tarjetas</small></span>" ++
            "<span class=\"nx-flash-stat is-due\"><strong>{d}</strong><small>para hoy</small></span></div></header>",
        .{ s.total, s.due },
    ));

    try body.appendSlice("<div class=\"nx-flash-layout\"><section class=\"nx-panel nx-flash-review\">");
    try body.appendSlice(try std.fmt.allocPrint(
        allocator,
        "<div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Repaso</p><h2>{s}</h2></div></div>" ++
            "<p class=\"nx-academic-muted\">Responde de memoria y elige cómo te fue. El intervalo se ajusta solo.</p>" ++
            "<button id=\"nx-flash-start\" class=\"nx-button\"{s}>{s}</button>" ++
            "<div id=\"nx-flash-box\" class=\"nx-flash-box\" hidden>" ++
            "<p class=\"nx-flash-progress\" id=\"nx-flash-progress\"></p>" ++
            "<div class=\"nx-flash-face nx-flash-front\" id=\"nx-flash-front\"></div>" ++
            "<div class=\"nx-flash-face nx-flash-back\" id=\"nx-flash-back\" hidden></div>" ++
            "<button id=\"nx-flash-reveal\" class=\"nx-button nx-button-secondary\">Mostrar respuesta</button>" ++
            "<div class=\"nx-flash-grades\" id=\"nx-flash-grades\" hidden>" ++
            "<button class=\"nx-flash-grade is-again\" data-grade=\"again\">Otra vez</button>" ++
            "<button class=\"nx-flash-grade is-hard\" data-grade=\"hard\">Difícil</button>" ++
            "<button class=\"nx-flash-grade is-good\" data-grade=\"good\">Bien</button>" ++
            "<button class=\"nx-flash-grade is-easy\" data-grade=\"easy\">Fácil</button></div>" ++
            "<p id=\"nx-flash-done\" class=\"nx-academic-muted nx-flash-done\" hidden>Sesión completa. Vuelve mañana o cuando toquen.</p>" ++
            "<p id=\"nx-flash-status\" class=\"nx-academic-muted\" role=\"status\"></p></div>",
        .{
            if (s.due > 0) try std.fmt.allocPrint(allocator, "{d} para repasar hoy", .{s.due}) else "Nada pendiente hoy",
            if (s.due > 0) "" else " disabled",
            if (s.due > 0) "Empezar repaso" else "Sin tarjetas para hoy",
        },
    ));
    try body.appendSlice("</section>");

    try body.appendSlice(
        "<aside class=\"nx-panel nx-flash-new\"><div class=\"nx-panel-head\"><div><p class=\"nx-eyebrow\">Nueva</p><h2>Crear tarjeta</h2></div></div>" ++
            "<form id=\"nx-flash-form\">" ++
            "<label>Frente<input name=\"front\" maxlength=\"1000\" required placeholder=\"Pregunta o concepto\"></label>" ++
            "<label>Reverso<textarea name=\"back\" maxlength=\"4000\" rows=\"4\" required placeholder=\"Respuesta\"></textarea></label>" ++
            "<label>Materia<input name=\"subject\" maxlength=\"120\" placeholder=\"Opcional\"></label>" ++
            "<button class=\"nx-button\" type=\"submit\">Guardar tarjeta</button></form>" ++
            "<p id=\"nx-flash-form-status\" class=\"nx-academic-muted\" role=\"status\"></p></aside></div>",
    );

    try body.appendSlice("<section class=\"nx-card nx-flash-list\"><header class=\"nx-flash-list-head\"><h2>Todas las tarjetas</h2><span class=\"nx-flash-count\">" ++
        "</span></header>");
    if (cards.len == 0) {
        try body.appendSlice("<p class=\"nx-academic-muted\">Todavía no hay tarjetas. Crea la primera arriba.</p>");
    } else {
        try body.appendSlice("<div class=\"nx-flash-items\">");
        for (cards) |c| {
            try body.appendSlice(try std.fmt.allocPrint(
                allocator,
                "<div class=\"nx-flash-item\"><div class=\"nx-flash-item-main\"><strong>{s}</strong><p>{s}</p><small>{s}{s}vence {s}</small></div>" ++
                    "<button class=\"nx-flash-del\" data-id=\"{d}\" aria-label=\"Eliminar tarjeta\" title=\"Eliminar\">×</button></div>",
                .{
                    try security.htmlEscape(allocator, c.front),
                    try security.htmlEscape(allocator, c.back),
                    if (c.subject.len == 0) "" else try std.fmt.allocPrint(allocator, "{s} · ", .{try security.htmlEscape(allocator, c.subject)}),
                    if (std.mem.order(u8, c.due_at, today) != .gt) "hoy · " else "",
                    try security.htmlEscape(allocator, c.due_at),
                    c.id,
                },
            ));
        }
        try body.appendSlice("</div>");
    }
    try body.appendSlice("</section>");
    try body.appendSlice(review_script);
    try body.appendSlice("</div>");
    return body.items;
}

// ── Pruebas ──────────────────────────────────────────────────────────────

test "schedule sube intervalo con good y reinicia con again" {
    const first = schedule(0, 2.5, 0, 0, .good);
    try std.testing.expectEqual(@as(i64, 1), first.interval_days);
    try std.testing.expectEqual(@as(i64, 1), first.reps);
    const second = schedule(first.interval_days, first.ease, first.reps, first.lapses, .good);
    try std.testing.expectEqual(@as(i64, 3), second.interval_days);
    const third = schedule(second.interval_days, second.ease, second.reps, second.lapses, .good);
    try std.testing.expect(third.interval_days > second.interval_days);
    const reset = schedule(third.interval_days, third.ease, third.reps, third.lapses, .again);
    try std.testing.expectEqual(@as(i64, 0), reset.interval_days);
    try std.testing.expectEqual(@as(i64, 0), reset.reps);
    try std.testing.expectEqual(@as(i64, 1), reset.lapses);
    try std.testing.expect(reset.ease < third.ease);
}

test "schedule easy desde cero y hard desde cero" {
    const easy = schedule(0, 2.5, 0, 0, .easy);
    try std.testing.expectEqual(@as(i64, 2), easy.interval_days);
    const hard = schedule(0, 2.5, 0, 0, .hard);
    try std.testing.expectEqual(@as(i64, 1), hard.interval_days);
    try std.testing.expect(hard.ease < 2.5);
}

test "shiftIso cruza mes, anio y bisiesto" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    try std.testing.expectEqualStrings("2026-12-01", try shiftIso(a, "2026-11-30", 1));
    try std.testing.expectEqualStrings("2027-01-01", try shiftIso(a, "2026-12-31", 1));
    try std.testing.expectEqualStrings("2024-02-29", try shiftIso(a, "2024-02-28", 1));
    try std.testing.expectEqualStrings("2026-11-24", try shiftIso(a, "2026-11-24", 0));
}
