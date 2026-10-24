# Estrategia de Pruebas

## Filosofía

**Construir de forma vertical:** cada módulo pasa por diseño → code → static check →
unit test → integration test → manual verification antes de considerarse terminado.

**No prometer tests que no se implementan.** Cada test listado tiene un comando
real y un expected result. Tests pendientes están marcados como `TODO`.

## Herramientas

| Herramienta | Propósito | Command |
|-------------|-----------|---------|
| `cargo check` | Verificación estática (types, ownership, lifetimes) | `cargo check` |
| `cargo test` | Tests unitarios + integration | `cargo test` |
| `cargo clippy` | Linting y best practices | `cargo clippy -- -D warnings` |
| `curl` / `wrk` | Tests de API HTTP | `curl -s http://127.0.0.1:8090/...` |
| `sqlite3` | Tests de DB | `sqlite3 data/test.db "SELECT ..."` |
| `systemctl` | Tests de servicio systemd | `systemctl status study-platform` |

## Tests unitarios

### Backend Rust (cargo test)

| Módulo | Test | Método |
|--------|------|--------|
| `db` | `test_db_init` | Crear DB en memoria (`:memory:`), verificar schema |
| `db` | `test_note_crud` | Insert/select/update/delete de nota |
| `db` | `test_fts_search` | Insertar 10 notas, buscar, verificar recall |
| `mcp_client` | `test_sse_handshake` | Mock SSE server, verify session ID received |
| `mcp_client` | `test_tool_call` | Mock SSE, execute `get_calendario`, parse JSON |
| `openrouter` | `test_prompt_builder` | Construir prompt con contexto académico, verificar token count < 16K |
| `openrouter` | `test_cost_tracking` | Sumar tokens, verificar cost < $2 USD |
| `notes` | `test_markdown_parse` | Parse Obsidian-style wikilinks, frontmatter |
| `notes` | `test_obsidian_compatible` | Write + read roundtrip, verify format |
| `api` | `test_session_auth` | Request sin cookie → redirect; con cookie → 200 |
| `api` | `test_rate_limit` | 15 requests → 6th blocked (10/min limit) |

### Comando
```bash
# Unit tests (DB en memoria, no necesita remoto ni MCP)
cargo test --lib -- --test-threads=4
```

## Tests de integración

### End-to-end (con MCP y OpenRouter reales)

| Test | Escenario | Expected |
|------|-----------|----------|
| `test_mcp_initialize` | POST `/mcp` con initialize | 200 + `mcp-session-id` header |
| `test_mcp_tools_list` | Ejecutar tools list via SSE | 17 tools devueltos |
| `test_mcp_get_calendario` | Llamar `get_calendario` | JSON con eventos, 200 OK |
| `test_mcp_readonly` | Verificar no hay tools write | Error si tool invierte "write" |
| `test_api_dashboard` | GET `/dashboard` | 200 HTML con calendario renderizado |
| `test_api_notes_crud` | POST/GET/PUT/DELETE `/api/notes` | CRUD lifecycle completo |
| `test_api_chat` | POST `/api/chat/test` con prompt | 200, respuesta en < 10s |
| `test_api_search` | GET `/api/search?q=calculus` | 200 JSON, resultados FTS5 |
| `test_custom_mcp` | SSE POST a `/mcp` del custom server | 200 + tools list |
| `test_backup_restore` | `study-platform backup` + restore | DB restored, data intact |

### Comando
```bash
# Integration tests (requieren MCP corriendo en remote)
cargo test --test integration -- --test-threads=1

# O manualmente:
curl -s http://127.0.0.1:8090/health          # → {"status":"ok"}
curl -s http://127.0.0.1:8090/api/subjects     # → [subjects JSON]
curl -s http://127.0.0.1:8090/api/mcp/status   # → {"connected":true, "tools":17}
```

## Tests de recursos

| Test | Métrica | Umbral |
|------|---------|--------|
| `test_memory_baseline` | RSS del proceso | < 200 MB (idle) |
| `test_memory_pico` | RSS durante scraping | < 1 GB (MemoryMax) |
| `test_cpu_under_load` | CPU% durante scraping | < 100% (1 core, 25% de 4) |
| `test_startup_time` | `systemd start` → ready | < 10 segundos |
| `test_mcp_restart` | Kill process → restart | < 15 segundos (RestartSec=10) |

## Tests de seguridad

| Test | Comando | Expected |
|------|---------|----------|
| `test_no_env_in_logs` | `journalctl -u study-platform` grep `OPENROUTER` | 0 matches |
| `test_dotenv_perms` | `ls -la .env` | `-rw-------` (600) |
| `test_mcp_localhost` | `ss -tlnp \| grep 8080` | `127.0.0.1:8080` only |
| `test_app_localhost` | `ss -tlnp \| grep 8090` | `127.0.0.1:8090` only |
| `test_custom_mcp_auth` | `curl http://127.0.0.1:8081/mcp` sin key | 401 Unauthorized |
| `test_custom_mcp_auth_ok` | `curl -H "X-API-Key: $KEY" http://127.0.0.1:8081/mcp` | 200 OK |
| `test_session_secure` | `curl -v http://127.0.0.1:8090/login` | `Set-Cookie: session=...; HttpOnly; SameSite=Strict` |

## Criterios de aceptación por módulo

### Módulo 1: Core API + SQLite
- [x] `cargo check` pasa (sin errors; 3 warnings por dead code future features)
- [x] `cargo clippy` — 0 errors
- [x] `cargo fmt --check` — pass
- [x] `cargo test` — 13 tests pasan (integration suite)
- [x] DB crea schema correcto (4 tablas: users, sessions, notes, app_config)
- [x] API `/health` responde `{"status":"healthy", "components":{"database":true, ...}}`
- [x] API `/api/config` devuelve configuración pública (sin secrets)
- [x] Session auth: request sin cookie → 302 redirect a `/login`; con cookie → 200
- [x] Rate limiting: 60 req/min, 61ª → 429
- [x] Login: wrong password → 401; correct → 302 redirect to `/dashboard`

### Módulo 2: MCP Client
- [ ] SSE handshake con Mikareno MCP funciona
- [ ] Execute `get_perfil` devuelve datos de usuario
- [ ] Execute `get_calendario` devuelve eventos
- [ ] Cache funciona (180s TTL)
- [ ] Reintentos en connection error (max 3, backoff)

### Módulo 3: Dashboard académico
- [ ] GET `/dashboard` muestra calendario con próximos eventos
- [ ] GET `/dashboard/horario` muestra horario por día
- [ ] GET `/dashboard/notas` muestra notas por asignatura
- [ ] Export a iCal: GET `/dashboard/calendar.ics`

### Módulo 4: Asistente OpenRouter
- [ ] POST `/api/chat/:subject` devuelve respuesta en < 10s
- [ ] Streaming SSE funciona (token por token)
- [ ] Cost tracking: incrementa $0.00 por uso de modelo free
- [ ] Context size limit: 16K tokens (resumen auto)
- [ ] Sin envío de credenciales al LLM

### Módulo 5: Second Brain
- [ ] POST `/api/notes` crea nota Markdown
- [ ] GET `/api/notes/:id` devuelve nota con frontmatter
- [ ] GET `/api/search?q=...` devuelve FTS5 results < 500ms
- [ ] Wikilinks `[[note]]` funcionan
- [ ] Import desde Obsidian (read-only sync)

### Módulo 6: Study Material
- [ ] POST `/api/materials/summary` genera resumen
- [ ] POST `/api/materials/quiz` genera cuestionario
- [ ] POST `/api/materials/export` generate PDF/Markdown
- [ ] Templates de prompts validados

### Módulo 7: Office Suite
- [ ] POST `/api/office/docx-to-pdf` proxy a GenOffice MCP
- [ ] POST `/api/office/create-docx` from markdown
- [ ] POST `/api/office/create-xlsx` from CSV/JSON
- [ ] GET `/api/office/status` → `{"available":true}`

### Módulo 8: Custom MCP Server
- [ ] SSE POST `/mcp` → 200 + session ID
- [ ] `tools/list` → devuelve herramientas (search_notes, get_academic_context)
- [ ] Auth: request sin API key → 401
- [ ] `tools/call search_notes` → funciona con FTS5

### Módulo 9: Admin & Backup
- [ ] GET `/admin/metrics` → uso CPU, memoria, disk
- [ ] GET `/admin/openrouter/budget` → cost tracking
- [ ] POST `/admin/backup` → crea backup SQLite
- [ ] POST `/admin/backup/restore` → restaura desde backup
- [ ] Test de restauración: backup → restore → verify data

## Métricas de calidad

| Métrica | Objetivo | Medición |
|---------|----------|----------|
| Code coverage | > 70% | `cargo tarpaulin --out xml` |
| Clippy warnings | 0 | `cargo clippy -- -D warnings` |
| Unit test pass rate | 100% | `cargo test --lib` |
| Integration test pass rate | > 90% | `cargo test --test integration` |
| API p99 latency | < 500 ms | `wrk -t2 -c10 -d30s http://...` |
| MCP fetch latency | < 5s (cached), < 30s (scrape) | `time curl ...` |
| Build time | < 60s (incremental) | `cargo build` |
| Binary size | < 20 MB | `ls -la target/release/study-platform` |


## Interfaz principal: regresiones de navegación y accesibilidad

La suite `tests/ui_regressions.rs` comprueba que el menú conserva sus 22 destinos,
que las vistas parciales no anidan otro elemento `<main>`, que los controles
principales de captura e importación siguen presentes y que el importador no
avanza al siguiente archivo antes de finalizar el guardado actual.

```bash
cargo test --test ui_regressions
cargo test --test shell_integrity
cargo test --test pages_smoke
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

La validación estática no sustituye la prueba manual: abrir las páginas en el
navegador, probar el menú en escritorio y móvil, importar una bóveda pequeña
de prueba, comprobar los avisos de conflicto, editar y guardar una nota, y
probar los flujos del asistente y del generador. No ejecutar importaciones sobre
la bóveda personal real como prueba destructiva.
