-- Tarjetas de estudio con repetición espaciada (SM-2 simplificado).
-- Solo local: no modifica Mikareno ni Google.
CREATE TABLE IF NOT EXISTS flashcards (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    front         TEXT NOT NULL,
    back          TEXT NOT NULL,
    subject       TEXT NOT NULL DEFAULT '',
    due_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%d', 'now')),
    interval_days INTEGER NOT NULL DEFAULT 0,
    ease          REAL NOT NULL DEFAULT 2.5,
    reps          INTEGER NOT NULL DEFAULT 0,
    lapses        INTEGER NOT NULL DEFAULT 0,
    last_reviewed TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_flashcards_due ON flashcards(due_at);
CREATE INDEX IF NOT EXISTS idx_flashcards_subject ON flashcards(subject);
