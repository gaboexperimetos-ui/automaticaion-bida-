# Study Platform (styp)

**Plataforma web de productividad académica personal**: panel académico con datos escolares reales, segundo cerebro de notas en Markdown, asistente IA con streaming, generación de material de estudio, suite ofimática y panel de administración con backups automáticos. Un usuario, un binario, una base SQLite.

> **Migración del backend a C en curso (2026-10-31).** `c_backend/` contiene la
> fase 1, opt-in y todavía incompleta. Rust/Axum sigue siendo el backend funcional
> y de producción hasta la paridad total descrita en [`docs/C_MIGRATION.md`](docs/C_MIGRATION.md);
> no se ha desplegado ni sustituido el servicio actual.

> Verificado en vivo el **2026-10-20**: `GET /health` responde
> `{"status":"healthy","components":{"database":true,"genoffice_mcp":true,"mikareno_mcp":true,"openrouter":true},"version":"0.1.0"}`.

## Ubicación del proyecto

| Ruta | Papel |
|---|---|
| `/home/gabo/proyectos/styp` | **Ubicación canónica del repositorio** (consolidada el 2026-10-20) |
| `/home/gabo/study-platform` | Enlace simbólico de compatibilidad → `styp`; los scripts, docs y unidades con la ruta antigua siguen resolviendo |
| El proyecto de Arduino vive aparte: `/home/gabo/arduino-firmware` | Workspace de firmware para Arduino Due (nombre renombrado desde `~/Proyectos` para que no se confunda con `~/proyectos`) |

## Funcionalidades

- **Panel principal** (shell Stitch v2.4 "Synthetic Cognition"): tarjetas del día con saludo, próxima clase y próximas tareas.
- **Dashboard académico**: horario de 7 días (cache-first 30 s), planner semanal, calendario mensual/diario + exportación `.ics`, asignaturas con notas y detalle por código, exámenes con cuenta atrás, vida escolar (6 secciones) y biblioteca.
- **Notas (segundo cerebro)**: Markdown + frontmatter YAML, TOC automático, wikilinks resueltos (los rotos quedan como texto), backlinks, tags, historial de versiones con restauración, exportación por nota e importación segura desde Obsidian.
- **Búsqueda**: FTS5 sobre las notas con fallback a `LIKE` (`GET /api/search`, página en `/dashboard/search`).
- **Asistente IA**: chat por asignatura con **streaming SSE real** (frames `context` / `sources` / `delta` / `reasoning` / `done` / `error`), RAG híbrido sobre notas con web opcional, historial persistido, generación de **resúmenes, cuestionarios y hojas de estudio**, y presupuesto mensual con tope duro.
- **Suite ofimática** (`/office`, respaldada en ficheros): documentos `.md`, hojas `.csv`, presentaciones `.json`, informes y biblioteca de archivos.
- **Administración**: panel `/admin` con métricas del sistema, presupuesto OpenRouter y gestión de backups.
- **Backups automáticos**: tarea diaria (primer volcado 60 s tras arrancar, luego cada 24 h), rotación con retención 5 en `data/backups/`, sidecar `.sha256` con `integrity_check`.
- **Servidor MCP propio**: expone 9 herramientas de la app para clientes externos (OpenCode, etc.), autenticación por `X-API-Key`.

## Arquitectura

```
Navegador (HTML servidor + HTMX + JS mínimo, shell Stitch v2.4)
        │
        ▼
Rust + Axum (study-platform, un solo binario)  ──►  SQLite (WAL, FTS5)
        │                                                    notes/ (Markdown en disco)
        ├── MCP cliente (SSE) ──► Mikareno MCP  (18 tools, cache 180 s, precarga al arrancar)
        ├── MCP cliente (SSE) ──► GenOffice MCP (oficina)
        ├── HTTPS ─────────────► OpenRouter   (IA, presupuesto USD)
        └── MCP servidor propio (rmcp) en /mcp y /mcp/sse (X-API-Key)
```

| Componente | Tecnología | Detalle |
|---|---|---|
| Backend actual | Rust, Axum 0.8 + Tokio | Implementación funcional y de producción |
| Backend en migración | C17/POSIX + SQLite C ABI | `c_backend/`, fase 1 opt-in; [estado y puertas de paridad](docs/C_MIGRATION.md) |
| BD | SQLite + sqlx (WAL, FTS5) | `data/app.db`, migraciones embebidas en `migrations/` |
| Frontend | HTML servidor + HTMX + Tailwind | Shell Stitch v2.4, interfaz legacy accesible en `/legacy` |
| Auth | Argon2 (contraseñas) + tokens de sesión opacos | ver [Seguridad](#seguridad) |
| IA | cliente OpenRouter (reqwest) | streaming SSE, budget gate |
| Oficina | módulo `office` propio | respaldado en ficheros (no es proxy de GenOffice) |

## Topología real

| Máquina | Rol | Servicios |
|---|---|---|
| **asus-tuf** (CachyOS, esta máquina) | Desarrollo, builds y MCP de datos | `mikareno-mcp` (`systemd --user`, `0.0.0.0:8080`, 18 tools) · instancia local de la app en `0.0.0.0:8090` · toolchain Rust |
| **gabo-Inspiron-5720** (homelab Dell, Ubuntu 26.04, tailnet `100.92.172.48`) | **Producción** | unidad systemd **de sistema** `study-platform` en `:8091` (8090 está ocupado en esa máquina), binario `~/study-platform/study-platform`, BD real, `genoffice-mcp` en `127.0.0.1:8453` |
| Tailscale | Malla VPN | la app de producción alcanza el Mikareno MCP de asus-tuf: `MIKARENO_MCP_URL=http://100.115.62.97:8080/mcp` |

## Estructura del repositorio

```
styp/
├── src/
│   ├── main.rs               # CLI (--addr/--migrate/--create-admin), arranque
│   ├── lib.rs                # AppState y módulos
│   ├── config.rs             # Config::from_env — fuente de verdad de variables
│   ├── db.rs                 # init, migraciones, verify_schema, create_admin, backup delegado
│   ├── backup/               # backups rotativos + tarea diaria (spawn_backup_task)
│   ├── handlers/             # rutas y páginas: api, auth, notes, search, settings,
│   │                         # admin, material, handoff, static_ui (legacy),
│   │                         # stitch_v2, stitch_panel, stitch_chat, stitch_notes, stitch_mcp
│   ├── middleware/           # auth (sesiones) y rate_limit
│   ├── academic/             # dashboard académico + templates Stitch compartidos
│   ├── ai/                   # OpenRouter: chat, streaming SSE, quiz, coste
│   ├── knowledge/            # notas, búsqueda FTS5, wikilinks, frontmatter, Obsidian
│   ├── office/               # documentos, hojas, presentaciones, biblioteca, report
│   ├── mcp_server/           # servidor MCP propio (rmcp)
│   └── integrations/mikareno/# cliente SSE + cache + adaptadores
├── migrations/               # SQLite: init, seed, knowledge, ai, sessions, usage_metrics, FTS5
├── tests/                    # 28 suites de integración (auth, ai, office, admin, e2e…)
├── static/                   # stitch-v2 (shell/UI) y study-platform-ui (legacy, /legacy)
├── data/                     # app.db + data/backups/ (runtime)
├── notes/                    # notas en Markdown (runtime)
├── docs/                     # documentación — empieza por docs/INDEX.md
├── systemd/                  # unidad de sistema para la homelab
└── .env.example              # plantilla de configuración documentada
```

## Requisitos y build

- Rust stable (`rustup`) — en asus-tuf está instalado y en uso.
- No hay dependencias externas para ejecutar: SQLite va embebido (bundled).
- Release optimizado para la máquina pequeña de producción: `opt-level="z"`, `lto="thin"`, `strip=true`.

```bash
cd /home/gabo/proyectos/styp
cargo build --release
# binario: target/release/study-platform
```

## Uso del binario (CLI real)

```bash
study-platform [OPTIONS]

  -a, --addr <ADDR>              Bind address   [default: 0.0.0.0:8090]
      --migrate                  Ejecutar migraciones y salir
      --create-admin <USERNAME>  Crear usuario administrador inicial
```

- **Servir es el modo por defecto** (no existe subcomando `serve`).
- **El bind real lo decide `--addr`, no `HOST`/`PORT`** (esas variables solo se muestran en la UI).
- **`--create-admin` genera una contraseña temporal aleatoria (Argon2) que no se imprime.** El mensaje que sugiere `study-platform set-password` está desfasado: ese subcomando no existe. Camino real para obtener login: arrancar con `AUTH_DISABLED=true` (sin login) y cambiar la contraseña desde la app en `/settings/password`.

## Configuración

Toda la configuración sale del `.env` (nunca subirlo). Fuente de verdad: `src/config.rs` + `.env.example`.

| Variable | ¿Obligatoria? | Por defecto | Descripción |
|---|---|---|---|
| `SESSION_SECRET` | **Sí** (≥32 caracteres) | — | Semilla para hashear tokens de sesión (`openssl rand -hex 32`) |
| `AUTH_DISABLED` | No | `false` | Modo "sin login": cualquier nodo de la red entra con todos los permisos. **Solo desarrollo** |
| `REGISTRATION_TOKEN` | No | — (sin él, `POST /register` queda deshabilitado con 403) | Token para el registro público de cuentas |
| `HOST` / `PORT` | No | `127.0.0.1` / `8090` | Solo informativos (el bind es la CLI `--addr`) |
| `DB_PATH` | No | `./data/app.db` | Fichero SQLite |
| `NOTES_DIR` | No | `./notes` | Carpeta de notas Markdown |
| `OPENROUTER_API_KEY` | No | — | Sin clave o vacía ⇒ **modo demo**: IA deshabilitada, el resto funciona |
| `OPENROUTER_MODEL` | No | `cohere/north-mini-code:free` | Ejemplo gratuito alternativo: `minimax/minimax-m3.1-flash:free` |
| `OPENROUTER_MAX_TOKENS` | No | `8192` | Tope por respuesta |
| `OPENROUTER_MAX_COST_USD` | No | `2.0` | Presupuesto mensual; agotado ⇒ error de presupuesto sin llamar al proveedor |
| `MIKARENO_MCP_URL` | No | `http://127.0.0.1:8080/mcp` | Solo loopback, RFC 1918 y CGNAT `100.64/10` (tailnet); hosts públicos se ignoran a propósito |
| `GENOFFICE_MCP_URL` | No | `http://127.0.0.1:8453/mcp` | Vacío = deshabilitado |
| `CUSTOM_MCP_API_KEY` | No | — | Sin clave ⇒ el servidor MCP propio queda deshabilitado |
| `DB_MAX_CONNECTIONS` | No | `4` | Pool SQLite (más conexiones = más RAM y contención del WAL) |
| `MCP_TIMEOUT_MS` | No | `5000` | Presupuesto de tiempo por llamada MCP |
| `RUST_LOG` | No | `study_platform=info,tower_http=info` | Filtro de tracing |

## Rutas HTTP (resumen verificado; detalle completo en [docs/API.md](docs/API.md))

| Área | Rutas |
|---|---|
| Públicas | `GET /health` · `GET /api/config` · `GET /api/public-config` · `GET/POST /login` · `POST /register` · `POST /logout` · `GET /handoff` |
| Páginas (auth) | `/` panel · `/dashboard` · `/dashboard/ai` · `/dashboard/notes` · `/dashboard/mcp` · interfaz anterior en `/legacy` (serve `/app.js`, `/styles.css`) |
| Académico (auth) | `/dashboard/horario` · `/dashboard/planner` · `/dashboard/calendar` (+`/daily`, `.ics`) · `/dashboard/subjects` (+`/{code}`) · `/dashboard/examenes` · `/dashboard/vida-escolar` · `/dashboard/profile` |
| Notas (auth) | UI: `/app/notes` (GET/POST), `/new`, `/{slug}`, `/{slug}/edit`, `update` PUT, `delete` DELETE, `export`, `versions/{id}/restore` · API: `/api/notes` CRUD |
| Búsqueda y material | `GET /api/search` · `GET /dashboard/search` · `GET/POST /dashboard/material` |
| IA (auth) | `/ai` · `POST /api/ai/chat` · `POST /api/ai/chat/stream` · `POST /api/ai/summary` · `POST /api/ai/quiz` · `POST /api/ai/study-sheet` · `GET /api/ai/cost` · `GET /api/ai/conversations[/{subject}/messages]` |
| Oficina (auth) | `/office` · `/office/docs…` · `/office/sheets…` · `/office/presentations…` · `/office/library` · `/office/report` |
| Administración (auth) | `/admin` · `/admin/metrics` · `/admin/openrouter/budget` · `/admin/backup/list` · API: `GET /api/admin/metrics`, `GET/POST /api/admin/backup`, `POST /api/admin/backup/restore`, `GET /api/admin/backup/download/{name}`, `GET /api/admin/backup-status` |
| MCP propio | `POST /mcp` · `POST /mcp/sse` (JSON-RPC/SSE, `X-API-Key`) |
| Cuenta | `GET/POST /settings/password` · `GET /settings` · `POST /api/settings/ai` |

Cualquier ruta no registrada responde 404 estilizado (JSON `{"error":"not_found"}` si empieza por `/api/` o `/mcp/`).

## Base de datos y datos en runtime

- Tablas principales: `users`, `sessions`, `notes` + `notes_fts` (FTS5), conversaciones e historial IA, `academic_data_cache`, `app_config`, `usage_metrics`, `backups` (ver `migrations/`).
- El contenido de las notas vive en ficheros `.md` en `NOTES_DIR`; en BD van los metadatos.
- Backups en `data/backups/app-YYYYmmdd-HHMMSS.db` (+ sidecar `.sha256` con `integrity_check`), retención 5, volcado `wal_checkpoint(TRUNCATE)` antes de copiar. Restauración vía API con validación de nombre e integridad, dentro de una transacción sin sustituir el fichero vivo.

## Seguridad

- Tokens de sesión de 64 caracteres hex (32 bytes CSPRNG), opacos: en BD se guarda `SHA256(SESSION_SECRET ":" token)`; legacy UUID sigue validando por compatibilidad.
- TTL duro de sesión de **30 días** (`SESSION_TTL_DAYS`, constante en `src/middleware/auth.rs`).
- Cookie `session_id`: `HttpOnly`, `SameSite=Strict`, `Secure` condicional (solo si la petición llegó por HTTPS).
- Rate limiting: 60 req/min general y 10 req/min en `POST /login`.
- `POST /register` exige `REGISTRATION_TOKEN`; sin él respondo 403.
- La clave de OpenRouter se sanea de todo lo servido (`static/**` con `value=""`).
- El cliente MCP solo acepta endpoints loopback / privados / CGNAT de Tailscale.

## Calidad y pruebas

- 28 suites de integración en `tests/` + tests de librería. Última corrida completa verificada en esta rama (arena, 2026-10-25): **620 tests en verde, 0 fallos** (`cargo test --no-fail-fast`; 3 `#[ignore]` con su motivo). La cifra de 562 del 2026-10-19 es histórica y ya no es la medida vigente.
- `cargo fmt --check` PASS · `cargo clippy --all-targets` sin errores.
- Tests marcados con `#[ignore]` tienen el motivo escrito en el propio test.

```bash
cargo test            # suite completa
cargo fmt --check && cargo clippy --all-targets
```

## Despliegue (producción = homelab Dell)

Resumen del procedimiento verificado el 2026-10-19 (detalle en [docs/DEPLOY.md](docs/DEPLOY.md)):

1. Build en asus-tuf: `cargo build --release` (o musl estático si se prefiere).
2. Verificar `sha256sum` del binario en ambos lados.
3. `scp` a `~/study-platform/study-platform.new` y **`mv` atómico** (nunca scp directo: `ETXTBSY`).
4. `pkill -x study-platform` → la unidad systemd de sistema lo levanta sola en ~11 s (`Restart=always`).
5. Salud: `curl http://100.92.172.48:8091/health` → 200.

Notas: la BD real es la de la Dell; `sudo` solo hace falta para tocar la unidad, no para reiniciar.

## Documentación

Empieza por **[docs/INDEX.md](docs/INDEX.md)** — índice maestro con propósito, vigencia y estado de cada documento: arquitectura (`ARCHITECTURE.md`), API (`API.md`), guía de uso (`USER_GUIDE.md`), setup de desarrollo (`DEVELOPER_SETUP.md`), despliegue (`DEPLOY.md`), requisitos, roadmap, seguridad, pruebas, decisiones (ADRs), changelog y el handoff del último sprint (`SESSION_HANDOFF.md`).

El material desfasado quedó archivado en `docs/archive/` (README anterior y un resumen obsolete generado por IA que describía un stack distinto del real).

## Licencia

Uso personal; código bajo MIT (`Cargo.toml`).
