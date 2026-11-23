-- Cuentas que entran con Google. `password_hash` sigue NOT NULL por
-- compatibilidad con el login clásico: a estas cuentas se les pone una
-- contraseña aleatoria imposible de adivinar, de modo que solo puedan
-- entrar por Google.
ALTER TABLE users ADD COLUMN oauth_provider TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN oauth_subject TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN oauth_email TEXT NOT NULL DEFAULT '';

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_oauth
    ON users (oauth_provider, oauth_subject)
    WHERE oauth_provider <> '';

CREATE INDEX IF NOT EXISTS idx_users_oauth_email
    ON users (oauth_email)
    WHERE oauth_email <> '';
