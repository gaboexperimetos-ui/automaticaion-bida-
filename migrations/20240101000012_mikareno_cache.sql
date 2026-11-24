-- Última respuesta completa de Mikareno, para seguir viendo notas y horario
-- cuando no hay conexión. Se refresca como máximo cada 5 horas.
CREATE TABLE IF NOT EXISTS mikareno_cache (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    payload    TEXT NOT NULL,
    fetched_at INTEGER NOT NULL
);
