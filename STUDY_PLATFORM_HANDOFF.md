# Handoff — Study Platform (Session Transfer)

> Creado: 2026-10-11  
> Proyecto: `/home/gabo/study-platform` (remoto: `gabo-Inspiron-5720`, Ubuntu 26.04, Tailscale `100.92.172.48`)  
> Build target: musl static (`x86_binario: `target/release/study-platform`)  
> Servidor: `http://127.0.0.1:8090` (local) / `http://100.92.172.48:8090` (Tailscale)  
> Login credentials: `gabo` / `gabo1234`  
> Modelo constructor: `poolside/laguna-s-2.1:free`

---

## Resumen ejecutivo

Se integró el kit de diseño **Stitch v2.4 "Synthetic Cognition"** como interfaz principal, reemplazando el StudyOS en las 5 rutas del dashboard. Todas las rutas están protegidas con middleware de autenticación DB-backed (session tokens hasheados con SHA256). El servidor corre y es accesible vía Tailscale.

**Estado actual:** Interfaz Stitch v2.4 funcionando en producción, auth funcional, tests fallando en algunos casos relacionados con referencias legacy en tests de sesión.

---

## Estructura del proyecto

```
study-platform/
├── AGENTS.md                     # Instrucciones globales para agentes
├── Cargo.toml                    # Dependencies + profile release (musl target)
├── .env                          # CONFIGURACION (gitignored): secrets, URLs MCP, OpenRouter
├── migrations/                   # SQL migrations (sqlx)
│   ├── 20240101000000_init.sql   # users, notes, app_config tables
│   ├── 20240101000001_seed.sql   # seed data
│   ├── 20240101000002_knowledge.sql  # knowledge tables
│   ├── 20240101000003_ai.sql     # AI tables (conversations, messages)
│   ├── 20240101000004_sessions.sql # idx on sessions.expires_at
│   └── 20240101000005_usage_metrics.sql # cost tracking
├── src/
│   ├── main.rs                   # Entry point: parse CLI → init DB → build router → listen
│   ├── lib.rs                    # Module declarations + AppState struct
│   ├── config.rs                 # Config from env vars (Config struct, PublicConfig)
│   ├── db.rs                     # DB init, migrations, schema verify, create admin user
│   ├── handlers/
│   │   ├── mod.rs                # ROUTE REGISTRATION — merge all routers
│   │   ├── api.rs                # /health, /api/config, /api/public-config
│   │   ├── auth.rs               # POST /register, GET/POST /login, POST /logout
│   │   ├── notes.rs              # Notes CRUD HTML + API (/app/notes, /api/notes)
│   │   ├── dashboard.rs          # /dashboard/calendar.ics export
│   │   ├── studyos.rs            # Legacy StudyOS handler (preserved, not in active routes)
│   │   └── stitch_v2.rs          # ★ ACTIVE: Stitch v2.4 shell builder + 5 handlers
│   ├── middleware/
│   │   ├── mod.rs                # pub mod auth; pub mod rate_limit;
│   │   ├── auth.rs               # require_auth_state, session cookie helpers, hashed token lookup
│   │   └── rate_limit.rs         # 60 req/min general, 10 req/min POST /login
│   ├── models/
│   │   ├── mod.rs                # Session, Pagination, HealthResponse, PublicConfig
│   │   ├── user.rs               # User, RegisterRequest, SetPasswordRequest
│   │   └── note.rs               # Note, CreateNoteRequest, UpdateNoteRequest
│   ├── academic/                 # Academic dashboard module
│   │   ├── mod.rs                # register_routes() → /dashboard/* routes
│   │   ├── templates/mod.rs      # Shared Stitch HTML templates (head, shell, page_ext, etc.)
│   │   ├── calendar.rs           # Monthly/daily calendar views
│   │   ├── dashboard.rs          # Dashboard handler (builds DashboardData from MCP+DB)
│   │   ├── planner.rs            # Weekly planner
│   │   ├── schedule.rs           # Weekly schedule
│   │   ├── subjects.rs           # Subject grades
│   │   └── profile.rs            # Student profile
│   ├── ai/                       # OpenRouter AI assistant
│   │   ├── mod.rs                # register_routes() → /api/ai/chat|summary|quiz|study-sheet|cost
│   │   ├── chat.rs               # AiAssistant service
│   │   ├── openrouter.rs         # OpenRouter HTTP client + cost tracking
│   │   ├── handlers.rs           # Route handlers for AI endpoints
│   │   └── quiz.rs               # Quiz generation types
│   ├── knowledge/                # Second brain: Markdown notes + FTS5 search
│   │   ├── mod.rs                # KnowledgeService
│   │   ├── notes.rs              # CRUD service (separate from handlers::notes)
│   │   ├── search.rs             # FTS5 search with LIKE fallback
│   │   ├── obsidian.rs           # Safe import from Obsidian dirs
│   │   ├── wikilinks.rs          # [[Note Name]] parsing
│   │   └── frontmatter.rs        # YAML frontmatter parse/serialize
│   ├── mcp_server/               # Custom MCP server (our own, port 8081)
│   │   ├── mod.rs                # register_routes() → /mcp, /mcp/sse
│   │   ├── server.rs             # rmcp-based server instance
│   │   ├── tools.rs              # Tool implementations
│   │   └── handlers.rs           # JSON-RPC + SSE handlers
│   ├── office/                   # Web office suite (filesystem-backed docs/spreadsheets/presentations)
│   │   ├── mod.rs                # register_routes() → /office/*
│   │   ├── documents.rs          # Doc CRUD (.md files)
│   │   ├── spreadsheets.rs       # Spreadsheet CRUD (.csv files)
│   │   ├── presentations.rs      # Presentation CRUD (.json files)
│   │   ├── library.rs            # File library
│   │   └── templates/mod.rs      # UI templates
│   ├── integrations/
│   │   ├── mod.rs                # pub mod mikareno;
│   │   └── mikareno/             # Mikareno MCP client (SSE)
│   │       ├── mod.rs            # Client, Models
│   │       ├── client.rs         # SSE transport, initialize, tool calls
│   │       ├── models.rs         # Perfil, Nota, Tarea, CalendarioEvent, HorarioDia, etc.
│   │       ├── cache.rs          # 180s cache layer
│   │       └── adapters.rs       # Data adapters
│   └── ui/
│       └── stitch.rs             # Re-export of head(), html_escape() from academic::templates
├── static/
│   ├── stitch-v2/                # Stitch v2.4 design kit extracted pages
│   │   ├── shell/
│   │   │   ├── aside.html        # Nav-rail (~10KB, 21 items)
│   │   │   ├── topbar.html       # Top-bar/header
│   │   │   └── head.html         # Tailwind CDN + font imports
│   │   └── _content/clean/       # Clean <main> content for each route
│   │       ├── index.html
│   │       ├── dashboard.html
│   │       ├── asistente.html
│   │       ├── notes.html
│   │       └── mcp.html
│   └── studyos/                  # Legacy StudyOS (preserved but NOT in active routes)
├── tests/
│   ├── auth_sessions.rs          # Session auth tests (under active fix)
│   ├── academic_integration.rs   # Academic integration tests
│   └── (other test files...)
├── data/app.db                   # SQLite database (runtime)
├── notes/                        # Markdown note files (runtime)
├── office/                       # Office artifacts (runtime)
├── docs/                         # Architecture, roadmap, security, decisions
│   ├── ARCHITECTURE.md
│   ├── ROADMAP.md
│   ├── SECURITY.md
│   ├── DECISIONS.md
│   ├── TESTING.md
│   ├── DEPLOYMENT.md
│   ├── CHANGELOG.md
│   ├── REQUIREMENTS.md
│   ├── SESSION_HANDOFF.md
│   └── DEPLOY_RUNBOOK_M10.md
└── target/release/study-platform  # Release binary (musl static)
```

---

## Arquitectura de rutas activas

### Rutas públicas (sin auth)
| Ruta | Método | Handler | Descripción |
|------|--------|---------|-------------|
| `/login` | GET | `auth::login_form` | Formulario de login (Stitch-styled) |
| `/login` | POST | `auth::login_submit` | Submit login → creates session, 302 → `/dashboard` |
| `/register` | POST | `auth::register` | Create new user account |
| `/logout` | POST | `auth::logout` | Delete session row, clear cookie, redirect to /login |
| `/health` | GET | `api::health` | Health check endpoint |
| `/api/config` | GET | `api::config` | Public configuration (no secrets) |
| `/api/public-config` | GET | `api::public_config` | Alias for /api/config |

### Rutas Stitch v2.4 protegidas por auth
| Ruta | Método | Handler | Contenido |
|------|--------|---------|-----------|
| `/` | GET | `stitch_v2::stitch_index` | Panel principal |
| `/dashboard` | GET | `stitch_v2::stitch_dashboard` | Dashboard académico |
| `/dashboard/ai` | GET | `stitch_v2::stitch_asistente` | Asistente académico |
| `/dashboard/notes` | GET | `stitch_v2::stitch_notes` | Segundo cerebro (notas) |
| `/dashboard/mcp` | GET | `stitch_v2::stitch_mcp` | Centro MCP |

### Rutas académicas protegidas (nested /dashboard)
| Ruta | Método | Handler | Descripción |
|------|--------|---------|-------------|
| `/dashboard/calendar` | GET | `calendar::monthly` | Calendario mensual |
| `/dashboard/calendar/daily` | GET | `calendar::daily` | Vista diaria calendario |
| `/dashboard/horario` | GET | `schedule::weekly` | Horario semanal |
| `/dashboard/planner` | GET | `planner::weekly` | Planificación semanal |
| `/dashboard/profile` | GET | `profile::index` | Perfil estudiante |
| `/dashboard/subjects` | GET | `subjects::index` | Asignaturas con notas |

### Rutas de notas protegidas
| Ruta | Método | Handler | Descripción |
|------|--------|---------|-------------|
| `/app/notes` | GET | `notes::list` | Lista de notas |
| `/app/notes` | POST | `notes::create` | Crear nota |
| `/app/notes/new` | GET | `notes::new_note` | Formulario nueva nota |
| `/app/notes/{slug}` | GET | `notes::view` | Ver nota individual |
| `/app/notes/{slug}/edit` | GET | `notes::edit` | Editar nota |
| `/app/notes/{slug}/update` | PUT | `notes::update` | Actualizar nota |
| `/app/notes/{slug}/delete` | DELETE | `notes::delete` | Eliminar nota |
| `/api/notes` | GET | `notes::api_list` | API lista notas (JSON) |
| `/api/notes` | POST | `notes::api_create` | API crear nota (JSON) |
| `/api/notes/{slug}` | GET | `notes::api_view` | API ver nota (JSON) |
| `/api/notes/{slug}` | PUT | `notes::api_update` | API actualizar nota (JSON) |
| `/api/notes/{slug}` | DELETE | `notes::api_delete` | API eliminar nota (JSON) |

### Rutas AI protegidas
| Ruta | Método | Handler | Descripción |
|------|--------|---------|-------------|
| `/ai` | GET | `handlers::ai_page` | UI asistente |
| `/api/ai/chat` | POST | `handlers::chat_handler` | Chat stream |
| `/api/ai/summary` | POST | `handlers::summarize_handler` | Generar resumen |
| `/api/ai/quiz` | POST | `handlers::quiz_handler` | Generar cuestionario |
| `/api/ai/study-sheet` | POST | `handlers::study_sheet_handler` | Generar hoja estudio |
| `/api/ai/cost` | GET | `handlers::cost_handler` | Cost tracking |
| `/api/ai/conversations` | GET | `handlers::conversations_handler` | Lista conversaciones |
| `/api/ai/conversations/{subject}/messages` | GET | `handlers::conversation_messages_handler` | Mensajes conversación |

### Rutas Office protegidas
| Ruta | Método | Handler | Descripción |
|------|--------|---------|-------------|
| `/office` | GET | `documents::office_home` | Home oficina |
| `/office/docs` | GET/POST | `documents::list` / `create` | Docs list/create |
| `/office/docs/{slug}` | GET/PUT/DELETE | `documents::{edit,update,delete}` | CRUD doc |
| `/office/sheets` | GET/POST | `spreadsheets::{list,create}` | Sheets list/create |
| `/office/presentations` | GET/POST | `presentations::{list,create}` | Presentations list/create |
| `/office/library` | GET | `library::list` | File library |

### Rutas MCP Server propias
| Ruta | Método | Handler | Descripción |
|------|--------|---------|-------------|
| `/mcp` | POST | `handlers::mcp_handler` | JSON-RPC MCP |
| `/mcp/sse` | POST | `handlers::mcp_sse_handler` | SSE MCP |

---

## Stitch v2.4 — Interfaz Principal (Activa)

### Arquitectura del shell
El kit Stitch usa un enfoque de **"unified shell"**: CSS compartido, no duplicado por handler.

1. `STITCH_V2_HEAD` = `<head>` con Tailwind CDN + fuentes (Manrope + Hanken Grotesk + Material Symbols)
2. `STITCH_V2_ASIDE` = Nav-rail fijo 250px con ~21 items (extraído del kit original)
3. `STITCH_V2_TOPBAR` = Top bar fijo h-16 con breadcrumbs, búsqueda, notificaciones, perfil
4. Content = Clean HTML extraído de `<main>...</main>` de cada página del kit
5. Auth script = Script inline que maneja logout desde topbar profile icon

### Código clave (`src/handlers/stitch_v2.rs`)
```rust
// NAV_LINKS: Mapeo data-path → ruta real
const NAV_LINKS: &[(&str, &str)] = &[
    ("panel-principal", "/"),
    ("mi-agenda", "/dashboard/calendar"),
    // ... 21 items total
    ("centro-mcp", "/dashboard/mcp"),
];

// update_nav_rail(): 
//   1. Reemplaza href="#" con rutas reales por data-path
//   2. Desactiva todos los items (quita clases bg-primary-container)
//   3. Marca item activo con clase + aria-current="page"

// stitch_v2_page():
//   1. Inyecta window.__STITCH_V2_AUTH__={authenticated:true/false} en <head>
//   2. Llama update_nav_rail() con path activo
//   3. Monta: DOCTYPE + head + aside + topbar + content + auth_script
```

### Flujos de Auth en Stitch
- Si NO hay session cookie → `window.__STITCH_V2_AUTH__={"authenticated":false}`, el JS del nav-rail ocultará items privados y mostrará enlace de login
- Si HAY session cookie → `window.__STITCH_V2_AUTH__={"authenticated":true}`, el JS del nav-rail renderiza todos los items y permite logout desde topbar

---

## Autenticación — Detalle Técnico

### Diseño (Mission 11+)
- Tokens: 32 bytes CSPRNG hex-encoded (64 chars), OPACOS
- DB stores `SHA256(SESSION_SECRET || ":" || raw_token)` (hex, 64 chars) in `sessions.id TEXT PRIMARY KEY`
- Legacy support: Raw UUID rows still validate via fallback lookup
- Cookie: `session_id=<token>; Path=/; HttpOnly; SameSite=Strict; Max-Age=86400`
- `Secure` flag is opt-in (disabled for plain HTTP/Tailscale deployment)
- Logout: Deletes BOTH hashed id AND raw value from `sessions` table

### Middleware `require_auth_state`
```rust
pub async fn require_auth_state(State(state), req, next) -> Response {
    1. Extract session_id cookie
    2. Validate token format: length 32..=128, alphanumeric + '-'
    3. Compute SHA256(secret:token) hash
    4. Query: SELECT * FROM sessions WHERE id = ? OR id = ? (hashed first, raw fallback)
    5. Check expiration: session.is_valid() (compares expires_at > now)
    6. If expired: best-effort DELETE + reject
    7. If valid: proceed to next handler
    8. If missing/bad/expired: auth_rejection(path, body)
}
```

### Auth rejection split
- Paths starting with `/api/` → 401 with text body
- All other paths → 302 redirect to `/login`

### CSRF defense
- SameSite=Strict on session cookie (primary)
- Origin/Referer validation on POST /login (secondary, when present)
- No explicit CSRF token system (acceptable for single-user app over Tailscale)

---

## Rate Limiting

- General: 60 requests/minute per IP (sliding window)
- Login brute-force: 10 requests/minute per IP on `POST /login`
- Client IP resolution: `ConnectInfo` socket addr (preferred) → `X-Forwarded-For` (fallback) → `"unknown"`
- Returns 429 with `Retry-After: 60` header

---

## Base de datos (SQLite)

### Tablas principales
| Tabla | Campos | Uso |
|-------|--------|-----|
| `users` | id, username, password_hash(sha256), created_at | Auth |
| `sessions` | id(TEXT PK), user_id, created_at, expires_at, user_agent | Session store |
| `notes` | id, title, slug, filename, tags, word_count, summary, created_at, updated_at | Notes metadata |
| `notes_fts` | FTS5 virtual table | Full-text search |
| `conversations` | id, subject, title, created_at, updated_at | AI chat history |
| `messages` | id, conversation_id, role, content, created_at | AI messages |
| `academic_data_cache` | tool_name, key, data(JSON), cached_at, expires_at | MCP cache |
| `app_config` | key, value | App settings |
| `usage_metrics` | date, model, input_tokens, output_tokens, cost_usd | OpenRouter billing |
| `backups` | id, created_at, file_path, status, size_bytes | Backup tracking |

### Notas sobre contenido de notas
- Metadatos en tabla `notes`
- Contenido en archivos `.md` en `$NOTES_DIR/<slug>.md`
- Frontmatter YAML opcional
- Wikilinks `[[Name]]` soportados
- FTS5 search con LIKE fallback

---

## Integraciones externas

| Servicio | Endpoint | Propósito | Estado |
|----------|----------|-----------|--------|
| Mikareno MCP | `http://127.0.0.1:8080/mcp` | Datos académicos (read-only) | SSE, 180s cache |
| GenOffice MCP | `http://127.0.0.1:8453/mcp` | Suite oficina proxy | SSE |
| OpenRouter API | `openrouter.ai` | IA generativa | Optional, `$2 USD cap |
| Custom MCP Server | `http://127.0.0.1:8081/mcp` | Exponer herramientas internas | localhost only, API key auth |

### Mikareno Tools disponibles
`get_perfil`, `get_ano_escolar`, `get_asistencia`, `get_biblioteca`, `get_calendario`, `get_cumpleanos`, `get_horario`, `get_mensajes`, `get_notas_trimestre`, `get_detalles_asignatura`, `get_detalles_tarea`, `get_detalles_evento`, `get_detalles_mensaje`, `get_detalles_grupo`, `get_detalles_profesor`, `get_mis_grupos`, `get_detalle_calendario`

### Mock mode
Cuando MCP no está conectado (URL no es localhost):
- `mock_perfil()` → Gabriel / Colegio Alamo / 2 Medio A
- `mock_subjects()` → Matematicas, Lenguaje, Ciencias, Historia, Fisica
- `mock_today_events()` → Eventos simulados con badge HOY si coinciden con hoy
- `mock_tasks_data()` → Tareas simuladas
- `mock_priorities()` → Prioridades simuladas

---

## .env (Variables de entorno)

```
PORT=8091
HOST=127.0.0.1
SESSION_SECRET=dev_secret_key_for_local_testing_only_32_chars!
DB_PATH=./data/app.db
NOTES_DIR=./notes
OPENROUTER_API_KEY=
OPENROUTER_MODEL=cohere/north-mini-code:free
OPENROUTER_MAX_TOKENS=8192
OPENROUTER_MAX_COST_USD=2.0
MIKARENO_MCP_URL=http://127.0.0.1:8080/mcp
GENOFFICE_MCP_URL=http://127.0.0.1:8453/mcp
CUSTOM_MCP_API_KEY=dev_secret_key_for_local_testing_only_32_chars!
RUST_LOG=study_platform=debug,tower_http=debug
```

> ⚠️ `SESSION_SECRET` debe tener ≥32 caracteres. El valor actual es solo para testing local.

---

## Estado de Test

### Tests pasantes actuales
- Todos los tests de `rate_limit.rs` (7 unit tests)
- Tests de `academic/templates/mod.rs` (html_escape, head, page structure, mock data rendering)
- Tests de `knowledge/frontmatter.rs`
- Tests de `knowledge/wikilinks.rs`
- Tests de `models` unitarios
- La mayoría de tests de integración de módulos individuales (ai, office, mcp_server, knowledge)

### Tests FALLANDO actualmente (`tests/auth_sessions.rs`)

**Root cause:** Referencias legacy a rutas `/api/thing` y `/page` que ya no existen en el `guarded_router` de los tests.

#### Fallos específicos:

1. **`test_logout_deletes_row_and_token_fails_gate`** (línea 290-293)
   - Línea 292: `get_guarded("/app/notes")` devuelve 404 porque `/app/notes` no tiene handler en guarded_router de test
   - El `guarded_router` solo registra `/dashboard` (stitch) y `/app/notes` (notes handler) pero puede haber problema con cómo se monta

2. **`test_expired_session_is_rejected`** (línea 312-314)
   - Similar: referencia a rutas que pueden no tener handlers en el router de test

3. **`test_unknown_and_malformed_tokens_are_rejected`** (línea 325-346)
   - Similar patrón de referencias

4. **`test_session_isolation_two_logins_logout_one`** (línea 387-390)
   - Claramente usa `/api/thing` en líneas 387 y 390 — esta ruta NO existe en absoluto
   
5. **`test_legacy_uuid_row_still_validates`** (línea 430)
   - Usa `/api/thing` en línea 430 — la ruta no existe

6. **`test_login_success_mints_cookie_usable_on_guarded_route`** — parece OK ahora (usa `/dashboard` y `/app/notes`)

### Fix pendiente
Reemplazar todas las ocurrencias restantes de `/api/thing` y `/page` en `auth_sessions.rs`:
- `/api/thing` → `/dashboard` (page-side) o `/app/notes` (API-side según lo que se quiera probar)
- `/page` → `/dashboard`

Luego `cargo test --test auth_sessions` para verificar.

---

## Build y Deploy

### Build command
```bash
cd /home/gabo/study-platform
cargo build --release --target x86_64-unknown-linux-musl RUSTFLAGS='-C link-arg=-s'
```

### Deploy steps
```bash
# En remoto (desde asus-tuf o directamente):
scp target/release/study-platform gabo@100.92.172.48:/home/gabo/study-platform/

# O directamente compilar en remoto:
ssh gabo@100.92.172.48
cd /home/gabo/study-platform
cargo build --release --target x86_64-unknown-linux-musl RUSTFLAGS='-C link-arg=-s'
```

### Run command
```bash
cd /home/gabo/study-platform
nohup ./target/release/study-platform > /tmp/study-platform.log 2>&1 &
```

### Debug logs
```bash
tail -f /tmp/study-platform.log
journalctl --user -u mikareno-mcp
ss -tlnp | grep 8080
free -m && cat /proc/loadavg
```

---

## Decisiones técnicas recientes (M12)

### Stitch v2.4 Integration
- Reemplazo completo de StudyOS por Stitch interface en las 5 rutas principales
- Shell unificado: CSS compartido, no duplicado por handler
- Nav-rail dinámico: `update_nav_rail()` mapea 21 data-paths a rutas reales, marca activo
- Auth state inyectado en frontend via `window.__STITCH_V2_AUTH__`
- JS inline de logout maneja click en topProfile div (toggle login/logout based on auth state)

### Auth Migration (Mission 11→12)
- Hash opaque tokens: `SHA256(SESSION_SECRET || ":" || raw_token)`
- Legacy UUID fallback preserved (both hashed and raw searched during validation)
- Logout deletes both hashed and raw candidates
- Transition compatibility ensures existing sessions keep working

### Test Infrastructure Updates
- `guarded_router` en tests registara solo `/dashboard` y `/app/notes` (actualizado tras migración stitch)
- `pub mod notes` y `pub mod stitch_v2` declarados en `handlers/mod.rs` para acceso desde tests
- Status codes ajustados: `SEE_OTHER` (303) → `FOUND` (302); message changes match actual error strings

### Design System: Synthetic Cognition
- Palette: canvas `#101419`, cyan `#5eead4`, lime `#9ed753`, violet `#a78bfa`
- Fonts: Manrope (headlines), Hanken Grotesk (body), Material Symbols Outlined (icons)
- Spacing scale: space-xs (4px), space-sm (8px), space-md (12px), space-lg (20px), space-xl (32px)
- Border radius: sm (4px), DEFAULT (8px), md (12px), lg (16px), xl (24px), full (9999px)

---

## Próximos pasos recomendados

### Corto plazo (inmediato)
1. **Fix tests failing** — Reemplazar `/api/thing` y `/page` references en `tests/auth_sessions.rs` por rutas válidas
2. **Run full `cargo test`** — Confirmar todos los tests pasan
3. **Build release binary** — Recompile con musl target
4. **Verify live at Tailscale URL** — Confirmar acceso público funciona

### Fase siguiente (Phase 2 de Stitch)
5. **Inject real data into Stitch v2.4 pages** — Conectar datos reales de Mikareno MCP, OpenRouter AI, y SQLite en las vistas de Stitch (actualmente son páginas estáticas del kit)
6. **Make nav-rail links functional** — Todos los enlaces del nav-rail deberían navegar a rutas funcionales o retornar placeholder pages
7. **Clean up studyos/** — Remover o archivar los archivos legacy de StudyOS

### Roadmap pendiente (según ROADMAP.md)
- Phase 9: Admin & Backup (rutas /admin/*, backups automáticos)
- Monitoring y alertas

---

## Archivos de documentación existentes

Todos los archivos relevantes están en `/home/gabo/study-platform/docs/`:
- `ARCHITECTURE.md` — Diagrama de componentes y flujos de datos
- `ROADMAP.md` — Fases de desarrollo con estados y matrices de dependencia
- `SECURITY.md` — Detalles de seguridad (auth, rate-limit, CSP headers)
- `DECISIONS.md` — ADRs (Architecture Decision Records)
- `TESTING.md` — Estrategia de pruebas
- `DEPLOYMENT.md` — Guía de despliegue
- `CHANGELOG.md` — Historial de cambios
- `REQUIREMENTS.md` — Requisitos funcionales
- `SESSION_HANDOFF.md` — Este documento (versión anterior, reemplazado por este archivo)
- `DEPLOY_RUNBOOK_M10.md` — Runbook de deploy M10

---

## Recursos del servidor remoto

- CPU: i5-3210M, 4 cores
- RAM: 7.1 GB
- OS: Ubuntu 26.04, glibc 2.43
- Load average: ~9.5
- Tailscale IPs: 100.115.62.97 ↔ 100.92.172.48
- Process limits: MemoryMax=1G, CPUQuota=200%, Nice=10
- Microphone access: disabled (privacy)

## Proceso activos (comprobación)

Verificar estos procesos antes de continuar:
```bash
# Verificar si el servidor está corriendo
ps aux | grep study-platform

# Verificar puertos
ss -tlnp | grep -E '8080|8081|8090|8453'

# Verificar servicios systemd
systemctl --user list-units | grep -i study

# Verificar recursos
free -m && cat /proc/loadavg
```

---

*Documento generado automáticamente para transferencia entre sesiones de OpenCode.*
