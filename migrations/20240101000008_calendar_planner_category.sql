-- Añade categorías y soporte para eventos locales importados desde iCalendar.
-- Las anotaciones antiguas permanecen como entradas del planner académico.
ALTER TABLE calendar_annotations
    ADD COLUMN category TEXT NOT NULL DEFAULT 'academic';

ALTER TABLE calendar_annotations
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'planner'
        CHECK (kind IN ('planner', 'event'));

ALTER TABLE calendar_annotations
    ADD COLUMN end_date TEXT;

ALTER TABLE calendar_annotations
    ADD COLUMN all_day INTEGER NOT NULL DEFAULT 1
        CHECK (all_day IN (0, 1));

ALTER TABLE calendar_annotations
    ADD COLUMN start_time TEXT;

ALTER TABLE calendar_annotations
    ADD COLUMN end_time TEXT;

ALTER TABLE calendar_annotations
    ADD COLUMN location TEXT NOT NULL DEFAULT '';

ALTER TABLE calendar_annotations
    ADD COLUMN uid TEXT;

ALTER TABLE calendar_annotations
    ADD COLUMN recurrence TEXT
        CHECK (recurrence IS NULL OR recurrence = 'weekly');

ALTER TABLE calendar_annotations
    ADD COLUMN weekly_day INTEGER
        CHECK (
            (recurrence IS NULL AND weekly_day IS NULL)
            OR (recurrence IS NOT NULL AND recurrence = 'weekly'
                AND weekly_day IS NOT NULL AND weekly_day BETWEEN 0 AND 6)
        );

CREATE INDEX IF NOT EXISTS idx_calendar_annotations_kind_date
    ON calendar_annotations (kind, date);
