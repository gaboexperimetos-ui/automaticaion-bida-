-- Mission 11 (Agent B): persistent AI usage metrics for the monthly budget gate.
--
-- DESIGN:
-- - One row per OpenRouter call (live AND mock). Mock calls insert 0.0-cost
--   rows (estimated = 1) so history is complete without ever tripping the gate.
-- - Monthly spend = SELECT COALESCE(SUM(cost_usd), 0) FROM usage_metrics
--   WHERE created_at >= date('now', 'start of month').
--   `created_at` defaults to UTC datetime('now'), so the string comparison
--   against date('now', 'start of month') is a correct chronological filter.
-- - The budget gate (see src/ai/openrouter.rs `check_budget_and_reserve`)
--   runs check-then-insert inside `BEGIN IMMEDIATE` so parallel requests
--   serialize on the SQLite write lock and can never overshoot the cap.
-- - `estimated` = 1 when the cost did not come from the provider (mock mode,
--   or provider response without a `cost` field); 0 when it did.

CREATE TABLE IF NOT EXISTS usage_metrics (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at    TEXT    NOT NULL DEFAULT (datetime('now')),
    model         TEXT    NOT NULL,
    input_tokens  INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd      REAL    NOT NULL DEFAULT 0.0,
    estimated     INTEGER NOT NULL DEFAULT 0,
    endpoint      TEXT    NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS idx_usage_metrics_created_at ON usage_metrics(created_at);
