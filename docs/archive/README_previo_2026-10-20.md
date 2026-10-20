# Study Platform

**A personal academic productivity web platform** — dashboard, AI assistant,
second-brain notes, MCP integration, and web office suite — designed to run on
resource-constrained infrastructure.

## Purpose

A single-user academic productivity suite that aggregates class schedules,
grades, attendance, assignments, and messages from the Mikareno platform (via
MCP), provides an AI-powered academic assistant (OpenRouter), manages personal
knowledge (Obsidian-compatible Markdown), and includes a web office suite
proxied through the existing GenOffice MCP.

## Architecture summary

```
┌─────────────────┐  SSE/HTTP  ┌──────────────────────┐
│  Browser (HTMX)  │ ◄────────► │  Rust/Axum Web App   │
└─────────────────┘            └──────────┬───────────┘
                          │  SQLite (/data/app.db)
                          │  Markdown notes/
                          │  │
                          ▼  ▼
               ┌───────────────┬──────────────────┐
               │  Mikareno MCP  │  GenOffice MCP    │  OpenRouter API
               │  (SSE:8080)    │  (SSE:8453)       │  (HTTPS)
               └───────────────┴──────────────────┘
```

- **Backend:** Rust + Axum (compiled on `asus-tuf`, deployed to Ubuntu remote)
- **Database:** SQLite (single file, FTS5 for search)
- **MCP client:** SSE transport to Mikareno MCP (`127.0.0.1:8080/mcp`)
- **MCP server (custom):** `rmcp` crate, port 8081
- **Frontend:** HTMX + Tailwind (server-rendered, minimal JS)
- **Office suite:** Proxy to GenOffice MCP (`127.0.0.1:8453`)

## Requirements

### Runtime (deployment server — Ubuntu 26.04)
- x86_64 Linux (gabo-Inspiron-5720)
- Python 3.14.x (for Mikareno MCP venv, si se migra)
- Chromium + Playwright deps (para Mikareno MCP)
- GenOffice MCP ya corre en port 8453

### Build (development machine — asus-tuf)
- Rust stable toolchain (via `rustup`)
- Cargo
- 244 GB free disk (para builds)

## Installation

### 1. Rust toolchain
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version
```

### 2. Build the application
```bash
cd /home/gabo/study-platform
cargo build --release
```

### 3. Configuration
```bash
cp .env.example .env
# Edit .env:
# - OPENROUTER_API_KEY=your_key_here
# - SESSION_SECRET=random_32_bytes
# - MIKARENO_MCP_URL=http://127.0.0.1:8080/mcp
# - GENOFFICE_MCP_URL=http://127.0.0.1:8453/mcp
# - PORT=8090
```

### IA gratis (MiniMax M3.1 Flash)

The AI assistant runs at **$0 cost** with the free OpenRouter model. Add the
key to `.env` (never commit this file) in 3 steps:

```bash
echo 'OPENROUTER_API_KEY=sk-or-v1-…' >> .env                    # key from openrouter.ai/keys
echo 'OPENROUTER_MODEL=minimax/minimax-m3.1-flash:free' >> .env # free tier, $0.00
cargo run --release -- --addr 127.0.0.1:8091                   # restart so .env reloads
```

### 4. Database
```bash
# Real CLI: flags, not subcommands (see src/main.rs).
# --migrate runs embedded migrations (migrations/*.sql) and exits.
cargo run -- --migrate
# First run only — create the admin user:
cargo run -- --create-admin <username>
# NOTE src/db.rs prints a `study-platform set-password <user>` follow-up.
# That command DOES NOT EXIST (only --addr/--migrate/--create-admin).
# The created user gets a random temp hash; password rotation is TBD.
```

### 5. Run
```bash
# Serving is the default (there is NO `serve` subcommand).
# The listener binds CLI --addr, NOT the PORT/HOST env vars
# (src/main.rs:95; --addr default 127.0.0.1:8090).
cargo run --release -- --addr 127.0.0.1:8091
# Visit: http://127.0.0.1:8091
# Port: 8090 was occupied locally by a python process at doc time —
# always verify first: ss -tlnp | grep -E '8090|8091'
```

## Configuration

All configuration via `.env` file (never commit to git).
Source of truth: `src/config.rs` (`Config::from_env`).
The ONLY required var is `SESSION_SECRET` (≥32 chars).
`PORT`/`HOST` populate `Config` but do NOT control the listener —
the socket binds CLI `--addr` (`src/main.rs:95`), so the systemd unit
must pass `--addr` explicitly.

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `SESSION_SECRET` | Yes | — | 32-byte random string for session signing (`openssl rand -hex 32`) |
| `OPENROUTER_API_KEY` | No | — (unset) | OpenRouter API key; app runs without it (AI endpoints use mock/degraded) |
| `CUSTOM_MCP_API_KEY` | No | — (unset) | `X-API-Key` for the custom MCP server (dev-open when unset) |
| `HOST` | No | `127.0.0.1` | Recorded in Config only — does NOT bind the listener |
| `PORT` | No | `8090` | Recorded in Config only — does NOT bind the listener (use `--addr`) |
| `MIKARENO_MCP_URL` | No | `http://127.0.0.1:8080/mcp` | Mikareno MCP SSE endpoint |
| `GENOFFICE_MCP_URL` | No | `http://127.0.0.1:8453/mcp` | GenOffice MCP SSE endpoint |
| `OPENROUTER_MODEL` | No | `cohere/north-mini-code:free` | Default LLM model |
| `DB_PATH` | No | `./data/app.db` | SQLite database path |
| `NOTES_DIR` | No | `./notes` | Markdown notes directory |
| `OPENROUTER_MAX_TOKENS` | No | `8192` | Max tokens per response |
| `OPENROUTER_MAX_COST_USD` | No | `2.0` | USD monthly budget (exact key name — `OPENROUTER_MAX_COST` is ignored) |
| `RUST_LOG` | No | `study_platform=info,tower_http=info` | Tracing filter |

## Routes (verified against `src/handlers/mod.rs` + module `register_routes`)

- `GET /health` — liveness (NOT `/api/health`; there is NO `/api/metrics`).
- `GET /api/config`, `GET /api/public-config` — public (non-secret) config.
- `/` → redirects to `/dashboard`; `/login` (GET+POST), `/logout` (POST).
- Dashboard: `/dashboard`, `/dashboard/calendar`, `/dashboard/calendar/daily`,
  `/dashboard/horario`, `/dashboard/planner`, `/dashboard/profile`,
  `/dashboard/subjects`, `/dashboard/calendar.ics` (auth).
- Notes UI+API: `/app/notes…`, `/api/notes…` (auth).
- AI: `/ai` (UI, auth) + `/api/ai/chat|summary|quiz|study-sheet` (POST),
  `/api/ai/cost` (GET).
- Office (filesystem-backed, auth): `/office`, `/office/docs…`,
  `/office/sheets…`, `/office/presentations…`, `/office/library…`.
- Custom MCP server: `POST /mcp`, `POST /mcp/sse` (`X-API-Key` auth).

## Local verification (executed 2026-10-10 on asus-tuf)

```text
$ ./target/release/study-platform --version
study-platform 0.1.0
$ ./target/release/study-platform --help   # Usage: study-platform [OPTIONS]
  -a, --addr <ADDR>              Bind address [default: 127.0.0.1:8090]
      --migrate                  Run database migrations and exit
      --create-admin <USERNAME>  Create initial admin user
$ sqlite3 --version
3.53.4 2026-07-24 ... (64-bit)
$ ss -tlnp | grep -E '8090|8091'
127.0.0.1:8090 ... users:(("python",pid=126454,...))   # 8090 OCCUPIED → use 8091+
$ cargo --version / rustc --version
NOT AVAILABLE here (no Rust toolchain) → `cargo test` NOT VERIFIED HERE.
$ sha256sum target/release/study-platform
ca9a959060a42ba63b3bc4aef55eab1f49b05e9614406947096e5ae70b90d619  (6983528 bytes;
NOTE: differs from the hash recorded in docs/DEPLOY_RUNBOOK_M10.md — binary was
rebuilt since; re-verify hash + GLIBC before any deploy.)
```
Remote-only commands in `docs/DEPLOYMENT.md` / `docs/DEPLOY_RUNBOOK_M10.md`
(ssh/scp/systemctl on `gabo@100.92.172.48`) are marked NOT EXECUTED there.

## Modules

| Module | Status | Description |
|--------|--------|-------------|
| Core API | ✅ Implemented (code+tests in tree) | REST backend with SQLite |
| MCP Client | ✅ Implemented (code+tests in tree) | SSE connection to Mikareno MCP |
| Academic Dashboard | ✅ Implemented (code+tests in tree) | Schedule, grades, tasks UI |
| AI Assistant | ✅ Implemented (code+tests in tree) | OpenRouter chat (per-subject contexts) |
| Second Brain | ✅ Implemented (code+tests in tree) | Markdown notes, Obsidian sync, FTS5 search |
| Study Material | ✅ Implemented (in AI module: summary/quiz/study-sheet) | AI-generated summaries, quizzes, exports |
| Office Suite | ✅ Implemented (filesystem-backed `/office/*`, NOT a GenOffice proxy) | Local docs/sheets/presentations + library |
| Custom MCP Server | ✅ Implemented (code+tests in tree) | rmcp server for external clients |
| Admin & Backup | ⚠️ Partial (no `/admin/*` routes; backup is procedural via sqlite3+cron) | Config, monitoring, backups |

Reality check (M11 docs pass, 2026-10-10, local only): 50 `src/**/*.rs` files,
97 `#[test]`/`#[tokio::test]` annotations present. `cargo test` NOT re-run here
(no Rust toolchain on this host) — "Implemented" means code + tests exist in
tree, not "suite green on this machine". No `/api/metrics`, no `/admin/*`
routes exist (grep-verified). See [docs/ROADMAP.md](docs/ROADMAP.md) for the
development plan.

See [docs/ROADMAP.md](docs/ROADMAP.md) for the development plan.

## License

Personal use. See [docs/SECURITY.md](docs/SECURITY.md) for security considerations.
