-- Anotaciones del calendario: notas personales que el usuario escribe sobre
-- un día concreto (examen, evento, recordatorio). Son datos LOCALES del
-- usuario, no vienen de Mikareno (que es read-only).
--
-- Diseño:
--   * `date` en formato `YYYY-MM-DD` (texto, no timestamp: el calendario
--     trabaja con días naturales y no con husos).
--   * Un índice por `date` para que el rango del mes se resuelva rápido.
--   * `created_at`/`updated_at` en ISO-8601 UTC para auditar.
--
-- No hay borrado en cascada con otras tablas: es una tabla independiente.

CREATE TABLE IF NOT EXISTS calendar_annotations (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    date        TEXT    NOT NULL,
    title       TEXT    NOT NULL DEFAULT '',
    body        TEXT    NOT NULL DEFAULT '',
    created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_calendar_annotations_date
    ON calendar_annotations (date);
