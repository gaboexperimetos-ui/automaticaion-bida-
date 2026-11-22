# AGENTS.md — Study Platform / NOVA

Aplicación web personal con frontend server-rendered **Stitch v2/NOVA** (HTML, CSS, HTMX/JavaScript) y backend implementado en Zig 0.14.1. El backend enlaza libc para sockets y la biblioteca SQLite del sistema; no hay fuentes de implementación Rust ni C en el repo. No convertir el frontend en React/Vite ni añadir otro backend.

## Invariantes del proyecto

- El alcance acordado es sustituir las implementaciones Rust/C por Zig y mantener los HTML/CSS/HTMX existentes servidos por Zig.
- No reintroducir `.rs`, `.c`, `Cargo.toml`, Makefiles del backend ni wrappers que oculten una implementación no Zig. Las migraciones SQL y el frontend son recursos, no implementaciones de backend.
- No declarar paridad total sin probar rutas y comportamientos. `docs/ARCHITECTURE.md` enumera explícitamente las funciones todavía parciales (cliente Mikareno/GenOffice, áreas académicas/oficina y streaming en vivo).
- No usar datos académicos ficticios: cuando una integración no está implementada o no responde, mostrar un estado vacío honesto.
- No borrar datos de usuario. Usa `DB_PATH` y `NOTES_DIR` temporales para tests; no trabajes contra una base local/prod por accidente.
- No desplegar ni ejecutar SSH/SCP/systemctl/crontab sobre la máquina de producción sin autorización explícita en la conversación actual.

## Toolchain y validación

- Toolchain fijado: **Zig 0.14.1**.
- Antes de cerrar un cambio de backend, ejecutar:

```bash
zig fmt --check build.zig main.zig test_root.zig src/*.zig
zig build
zig build test --summary all
python3 tests/http_smoke.py --binary zig-out/bin/study-platform
```

- Para release: `zig build -Doptimize=ReleaseFast`.
- `tests/http_smoke.py` es sólo harness de integración Python estándar; la aplicación y su lógica permanecen en Zig.
- No configurar una clave real de OpenRouter en tests ni llamar a servicios de pago. El smoke test usa modo simulado y datos temporales.
- CI vive en `.github/workflows/zig-validation.yml`; el prerelease de preview está separado en `preview-backend.yml`.

## Estructura y fuentes de verdad

- `build.zig`, `main.zig`, `test_root.zig`: build, ejecutable y raíz de tests.
- `src/main.zig`: CLI, listener IPv4 y manejo de conexiones.
- `src/app.zig`: rutas, sesiones, rate limit, configuración, admin y backups.
- `src/notes.zig`: CRUD Markdown, búsqueda, notas y anotaciones/calendario.
- `src/ai.zig`: APIs AI/OpenRouter, persistencia de conversaciones y coste.
- `src/mcp.zig`: servidor MCP propio y herramientas.
- `src/sqlite.zig`: ABI SQLite y migraciones embebidas.
- `src/http.zig`, `src/security.zig`, `src/templates.zig`, `src/assets.zig`, `src/jsonutil.zig`, `src/config.zig`: HTTP, seguridad, plantillas/estáticos, JSON y configuración.
- `static/`: frontend Stitch/NOVA y archivos estáticos preservados.
- `migrations/`: ocho migraciones SQL preservadas; no sustituirlas por schema duplicado en Zig.
- Variables actuales: `src/config.zig` y `.env.example`; rutas: `src/app.zig`, `src/notes.zig`, `src/ai.zig`, `src/mcp.zig` y `docs/API.md`.
- Vault de Obsidian: `VAULT_DIR` (por defecto `$HOME/SegundoCerebro`) es la carpeta que `POST /api/notes/sync-vault` importa. La BD SQLite es la fuente de verdad; `NOTES_DIR` es el espejo `.md` que escribe la app. No borres `VAULT_DIR` sin confirmar: puede contener notas personales.

## Build y ejecución segura

- SQLite debe estar disponible en el sistema (`libsqlite3.so.0`; en Debian/Ubuntu, `libsqlite3-dev` para compilar).
- `SESSION_SECRET` es obligatorio y debe tener ≥32 caracteres. Nunca imprimirlo ni copiar un `.env` real al repo.
- `AUTH_DISABLED=true` es sólo para desarrollo privado; el smoke test lo usa en un loopback temporal.
- El MCP propio requiere `CUSTOM_MCP_API_KEY`. Una clave vacía debe dejar el endpoint deshabilitado, no abierto.
- Los artefactos Zig (`.zig-cache/`, `zig-out/`) y los datos runtime están ignorados por Git.

## Documentación y honestidad sobre estado

- Entrada: `docs/INDEX.md`; API: `docs/API.md`; setup: `docs/DEVELOPER_SETUP.md`; arquitectura y paridad: `docs/ARCHITECTURE.md`; pruebas: `docs/TESTING.md`.
- Actualiza documentación si cambian rutas, configuración, seguridad o comportamiento.
- Describe como «buffered SSE» la respuesta `/api/ai/chat/stream` hasta que exista relay token-a-token verificado.
- No describas Mikareno, GenOffice, tareas de backup en segundo plano ni páginas académicas como funcionales mientras sólo haya placeholders.
- No afirmes que el binario Zig está en producción: el preview local/CI no es un despliegue.
