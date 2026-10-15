-- Mission 11 (sessions): hashed opaque session tokens.
--
-- No schema change is needed for the hash itself: new session rows store
-- `SHA256(SESSION_SECRET || ":" || raw_token)` (64 lowercase hex chars) in
-- the existing `sessions.id TEXT PRIMARY KEY` column, which already fits any
-- TEXT value. Legacy rows (raw UUIDs) keep working: the validator looks up
-- the hashed id first, then the raw cookie value; logout deletes both.
--
-- This migration only adds an index to keep the per-request session lookup
-- (and expiry cleanup) fast. Fully idempotent (`IF NOT EXISTS`).

CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);
