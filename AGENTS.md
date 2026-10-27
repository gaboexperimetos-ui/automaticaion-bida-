# AGENTS.md — Study Platform (styp)

App web personal de productividad académica (un solo usuario): backend Rust/axum en
un binario único + SQLite (sqlx, bundled, WAL, FTS5), frontend server-rendered
(HTML + Tailwind + HTMX), shell Stitch v2.4. NO es React/Vite/Docker: un resumen
generado por IA con ese stack quedó archivado en `docs/archive/` — ignorar.

## Repos y máquinas
- Repositorio canónico: `/home/gabo/proyectos/styp` (consolidado el 2026-10-20).
- `~/study-platform` es un **symlink → styp**: es el MISMO árbol, no una segunda
  copia. La unidad systemd y los docs antiguos usan esa ruta y resuelven igual.
- Dev/build y mikareno-mcp viven en esta máquina (asus-tuf). Producción es la
  Dell (ssh `homelab`, `gabo-Inspiron-5720`, tailnet `100.92.172.48`).
- Remotos: `dell` = `homelab:/home/gabo/study-platform.git` (bare) — `git push
  dell main` tras cada commit; también existe `origin` (GitHub), sincronizado a
  2026-10-22.
- `.env` y sus backups (ya en `.gitignore`) contienen secrets: nunca commitearlos.

## Comandos
- Orden de trabajo: `cargo check` → `cargo test` → build. Antes de cerrar tarea:
  `cargo fmt --check && cargo clippy --all-targets`.
- UN archivo de tests: `cargo test --test auth_sessions` (sin `<>`);
  28 suites de integración en `tests/`.
- Dev: `cargo run` (lee `.env`). Sin dependencias externas: SQLite bundled.
- Última suite completa: 562 en verde (2026-10-19); si el árbol se movió desde
  entonces, re-ejecutar antes de dar por buena la cifra.
- CLI real (verificado 2026-10-22 con `--help` del binario): `study-platform
  [--addr ADDR] [--migrate] [--create-admin USER]`. Servir es el modo por
  defecto; NO existe subcomando `serve`.
- El bind lo decide `--addr` (default `0.0.0.0:8090`); `HOST`/`PORT` del `.env`
  son solo informativos (se muestran en la UI).
- `--create-admin` genera contraseña aleatoria y NO la imprime; el mensaje que
  sugiere `study-platform set-password` está desfasado (ese subcomando no
  existe). Camino real: arrancar con `AUTH_DISABLED=true` y cambiar la
  contraseña en `/settings/password`.
- `tests/openrouter_live.rs` llama al proveedor real de OpenRouter dentro de la
  suite (modelo free, respeta el budget gate): requiere key válida en `.env`.
- Hay 3 tests `#[ignore]` con el motivo escrito en el test (p. ej.
  `tests/academic_integration.rs:515` exige el MCP de Mikareno parado;
  `tests/security.rs:1066` cubre auth legacy). No habilitar sin leer el motivo.
- Varias sesiones pueden estar editando a la vez: una compilación a medias puede
  fallar aunque el código esté bien. Reintentar mirando qué ficheros están sucios.

## Config `.env` (fuente de verdad: `src/config.rs` + `.env.example`)
- `SESSION_SECRET` obligatoria (≥32 chars): sin ella la app no arranca.
- El `.env` local de dev tiene `AUTH_DISABLED=true` (sin login) y
  `RUST_LOG=debug`; en prod va `false` (login Argon2).
- IA: sin `OPENROUTER_API_KEY` ⇒ modo demo (todo funciona menos IA). Modelo hoy
  `minimax/minimax-m3.1-flash:free` (default en código:
  `cohere/north-mini-code:free`) — no cambiar sin permiso. Presupuesto mensual
  $2 (`OPENROUTER_MAX_COST_USD`): gate que deja de llamar al proveedor al agotarse.
- `POST /register` exige `REGISTRATION_TOKEN` (sin esa variable responde 403).
- `CUSTOM_MCP_API_KEY` vacía ⇒ el servidor MCP propio (`/mcp`, `/mcp/sse`,
  header `X-API-Key`) queda deshabilitado.
- El cliente MCP solo acepta hosts loopback / RFC 1918 / CGNAT `100.64/10`
  (tailnet). Un host público se IGNORA en silencio ⇒ la app corre sin datos
  académicos sin error evidente (es deliberado).

## Deploy a la Dell (runbook detallado: `docs/DEPLOY.md`)
- Cualquier SSH/SCP/systemctl/crontab hacia `homelab` requiere autorización
  explícita del usuario en esa conversación.
- `cargo build --release` — glibc, NO musl (no hay `musl-gcc` aquí y el binario
  de producción siempre fue glibc; re-verificado 2026-10-22).
- Backup del binario en servicio: `ssh homelab 'cp ~/study-platform/study-platform
  ~/study-platform/study-platform.prev'` (es el rollback).
- `scp target/release/study-platform homelab:~/study-platform/study-platform.new`
  + `mv` atómico + `chmod +x`. NUNCA scp directo al path final: el binario está
  en ejecución y falla con `ETXTBSY`.
- `ssh homelab 'pkill -x study-platform'` — systemd lo levanta solo en ~11 s
  (Restart=always, RestartSec=10). Sin sudo para reiniciar el proceso; sudo solo
  para tocar la unidad (`daemon-reload`/`enable`/`restart`) y `sudo -n` falla
  (pide contraseña interactiva).
- Verificar: `systemctl is-active study-platform`; `curl
  http://100.92.172.48:8091/health` → 200 con componentes; `sha256sum` local ==
  remoto; `ssh homelab 'journalctl -u study-platform -n 30 --no-pager'` sin
  panic (funciona sin sudo).
- Rollback: restaurar `.prev` y repetir el `pkill`.
- Lección de producción: registrar una ruta DOS veces hace panic de axum
  ("Overlapping method route") y el servicio entra en crash-loop. Ante loop,
  leer el panic del journal antes de reintentar deploy.

## Infra
- Puerto 8091 en la Dell SIEMPRE (el 8090 de esa máquina está ocupado por otro
  servicio). Unidad `study-platform` es **de sistema** (`/etc/systemd/system`):
  `systemctl --user` da "unit not found". Dell pequeña: MemoryMax=1G.
- mikareno-mcp (datos escolares): FastMCP/Python en ESTA máquina (systemd
  `--user`, `mikareno-mcp.service`, `0.0.0.0:8080`, 18 tools; verificado
  escuchando 2026-10-22). La Dell lo ve en `http://100.115.62.97:8080/mcp`;
  local: `http://127.0.0.1:8080/mcp`. Issue conocido: algunas tools se cuelgan
  y el lock compartido bloquea a las demás; la app degrada a error por sus
  deadlines (no es bug de la app). Recovery: `systemctl --user restart
  mikareno-mcp.service`.
- GenOffice MCP: `127.0.0.1:8453` en la Dell; también escucha en esta máquina
  (verificado 2026-10-22, proceso `genoffice`).
- BD: la que cuenta es la de la DELL (`~/study-platform/data/app.db`, ~1.5 MB).
  La `data/app.db` local (~100 KB) es dev/stale: no mezclar. Notas: contenido
  en ficheros `.md` de `NOTES_DIR`; en BD solo metadatos.
- Backups: tarea in-app diaria (60 s tras arrancar, luego cada 24 h; retención
  5 en `data/backups/`, sidecar `.sha256` con `integrity_check`) y cron en la
  Dell a las 03:15 (`backup-db.sh`, retención 14). Restore solo vía API:
  valida nombre + integridad, dentro de transacción, sin sustituir el fichero vivo.
- `/health` hace probes reales (MCP: POST `initialize`) con cache TTL 30 s: los
  componentes del JSON reflejan estado real, no hardcode.
- `.env` de la Dell: `/home/gabo/study-platform/.env`, permisos 600 — NO copiar
  el local de dev encima.

## Seguridad (invariantes — no se negocian)
- NUNCA secrets en código ni logs. La key de OpenRouter vieja fue retirada
  del árbol actual (design/**/centro_mcp_configuraci_n_openrouter/code.html,
  `value=` vaciado); el historial (aa193d0) sigue considerándose expuesto y
  la rotación queda PENDIENTE si esa clave fue real: no usarla, no imprimirla.
- No exponer puertos a internet: acceso solo vía Tailscale.
- No tocar firewall, Tailscale, GenOffice MCP ni config de OpenCode; no
  instalar paquetes de sistema sin autorización; confirmar antes de
  operaciones destructivas.
- No cambiar el modelo del agente ni consumir créditos de pago sin
  autorización explícita.

## Debugging
- App prod (Dell): `ssh homelab 'journalctl -u study-platform -n 50 --no-pager'`.
- mikareno-mcp (esta máquina): `journalctl --user -u mikareno-mcp`.
- Puertos: `ss -tln | grep -E ':(8080|8090|8091|8453)'`.
- No asumir: verificar con `curl` o test directo.

## Docs y convenciones
- Entrada documental: `docs/INDEX.md`. Estado verificado más reciente:
  `docs/SESSION_HANDOFF.md`. API completa: `docs/API.md`. Runbook:
  `docs/DEPLOY.md` (sustituye `DEPLOYMENT.md`). Setup local:
  `docs/DEVELOPER_SETUP.md`.
- Regla del repo: al cerrar una tarea relevante, actualizar el doc
  correspondiente en `docs/` y su fecha de vigencia.
- Fuente de verdad de rutas HTTP: `src/handlers/mod.rs` + `register_routes()`
  de cada módulo; de variables de entorno: `src/config.rs`.
- Mensajes de usuario y commits en español; comentarios del código mezclados
  EN/ES.
