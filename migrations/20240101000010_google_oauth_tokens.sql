-- Tokens de Google por usuario. Permiten leer Google Calendar en nombre del
-- usuario sin volver a pedir consentimiento en cada visita.
--
-- * `refresh_token` permite obtener access_token nuevos cuando caducan.
-- * Se guarda cifrado con la clave de la app no: se guarda tal cual en la BD
--   local (SQLite del usuario). La BD ya es privada del usuario.
-- * Un usuario puede tener NULL si aún no ha autorizado Calendar.
CREATE TABLE IF NOT EXISTS google_oauth_tokens (
    user_id       INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    refresh_token TEXT NOT NULL DEFAULT '',
    scope         TEXT NOT NULL DEFAULT '',
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
