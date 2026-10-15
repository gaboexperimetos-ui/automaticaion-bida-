# Changelog

All notable changes to Study Platform are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/), SemVer.

## [0.1.0] — 2026-10-10 — "Primer Build Funcional"

### Added
- **Backend**: Rust + Axum web server (single binary, edition 2021)
- **Database**: SQLite single-file with WAL mode, `lib-sqlite3-sys` (bundled from source)
- **Migrations**: Versioned SQL migrations via `sqlx::migrate!()` — `users`, `sessions`, `notes`, `app_config` tables
- **API endpoints**:
  - `GET /health` — service health, component status, no secrets
  - `GET /api/config` — public configuration (no secrets)
  - `GET /api/public-config` — alias for `/api/config`
  - `GET /login` — login form (server-rendered HTML)
  - `POST /login` — session creation with Argon2 password verification
  - `POST /logout` — session termination with cookie clearing
- **Authentication**: Argon2 password hashing, UUID session tokens, HttpOnly + SameSite=Strict cookies
- **Rate limiting**: In-memory 60 req/min per client IP, returns 429 on excess
- **Frontend**: HTMX + Tailwind via CDN, server-rendered, responsive (mobile + desktop)
- **Login page**: Dark/cyberpunk aesthetic, Spanish UI labels
- **Protected routes**: Web routes redirect to `/login` (302), API routes return 401
- **Config validation**: `SESSION_SECRET` must be 32+ chars; OpenRouter key is optional

### Fixed
- Removed `sqlx::Error::FromSqlConversionBox` (non-existent variant in sqlx 0.8)
- Fixed `sqlx::query_scalar` generic args (2 needed in 0.8, not 1)
- Fixed `socket::SocketAddr` import (non-existent in axum 0.8)
- Fixed `Response` type scope in middleware (added `axum::response::Response`)
- Fixed `form::Form` import (use `axum::Form` in 0.8)
- Fixed SQLite connection URL parsing (`./path` interpreted as URL host `.`)
- Fixed duplicate closing brace in `dashboard.rs`
- Fixed duplicate import lines in `db.rs`

### Known Issues / Deferred
- **FTS5**: Disabled in F1 — `content_rowid` with triggers caused "SQL logic error". Replan
- **musl static build**: `musl-gcc` not available on asus-tuf. Binary uses glibc dynamically; compatibility verified via `readelf -V` (max GLIBC_2.34, remote has 2.43)
- **Set password CLI**: `set_password` handler defined but not yet registered as a route
- **New note page**: `new_note` handler defined but not registered as a route
- **Debug info endpoint**: `debug_info` handler defined but not registered as a route
