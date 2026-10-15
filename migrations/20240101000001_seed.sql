INSERT INTO users (username, password_hash)
VALUES ('admin', '')
ON CONFLICT(username) DO UPDATE SET username = username;

-- Note: password must be set via CLI before first use
