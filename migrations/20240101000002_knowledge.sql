-- Study Platform v0.1.0 — Second brain knowledge tables
--
-- Adds:
--   * note_versions  — content snapshots for version history (last 5)
--   * notes_fts      — standalone FTS5 index (title + content)
--
-- ROOT CAUSE of earlier "SQL logic error" with FTS5
-- ================================================
-- The `notes` table (defined in 20240101000000_init.sql) stores Markdown
-- *content on disk* as .md files — there is no `content` column in the SQL
-- table. The original init migration had a commented-out FTS5 block plus
-- trigger stubs referenced by handler comments ("FTS index is updated by
-- notes_ai trigger", "FTS index cleanup is handled by notes_ad trigger").
--
-- The earlier attempt tried to create an *external-content* FTS5 table whose
-- `content` column mapped to `notes.content` — a column that does not exist.
-- The resulting AFTER INSERT / AFTER DELETE triggers referenced
-- `new.content` / `old.content`, producing:
--
--   SQL logic error 1 (SQLITE_ERROR): no such column: content
--
-- FIX (this migration):
--
-- Use a *standalone* FTS5 virtual table (no external content, no triggers).
-- The `bundled` feature of libsqlite3-sys (verified in build fingerprints)
-- compiles SQLite with -DSQLITE_ENABLE_FTS5, so FTS5 is available. The
-- KnowledgeService syncs the index from application code on every CRUD
-- operation. If FTS5 is unavailable at runtime, the service transparently
-- falls back to LIKE-based search.

-- Content version history: stores snapshots of note content (last 5 per note)
CREATE TABLE IF NOT EXISTS note_versions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    note_id     INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    content     TEXT NOT NULL,
    word_count  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Standalone FTS5 full-text search index.
-- rowid is set explicitly to notes.id by the service so we can JOIN back.
-- No triggers — the service manages INSERT/DELETE inline with CRUD.
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    title,
    content,
    tokenize = 'unicode61'
);

-- Index for fast version-history lookup
CREATE INDEX IF NOT EXISTS idx_note_versions_note_id ON note_versions(note_id);
