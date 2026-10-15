# AGENTS.md — Study Platform

## Qué es esto
App web personal de productividad académica (un solo usuario). Backend Rust/axum
+ SQLite (sqlx), frontend server-rendered (HTML + Tailwind + HTMX), UI Stitch v2.4.
Identifiers y comentarios en inglés; mensajes de usuario en español.

## Comandos
- `cargo check` — verificación estática
- `cargo test` — suite completa
- `cargo test --test <archivo>` — UN solo archivo de tests (sin `<>`, p.ej. `--test auth_sessions`)
- `cargo run` — dev local (lee `.env`)
- Producción: `cargo build --release` (glibc — es lo que corre realmente en la Dell; verificado 2026-10-14 por BuildID idéntico al binario desplegado).
- NOTA: la instrucción musl del handoff era obsoleta: no existe `x86_64-linux-musl-gcc` en esta máquina y el binario en producción nunca fue musl. No reintentar musl sin Docker.
- Orden: cargo check → cargo test → build.

## Git / sincronización entre máquinas
- Canónico: `/home/gabo/study-platform` en asus-tuf (esta PC), rama `main`.
- Remoto `dell` = `homelab:/home/gabo/study-platform.git` (repo BARE en la Dell): `git push dell main` tras cada commit.
- El `.git` dentro de `/home/gabo/study-platform` de la Dell es un snapshot parcial de 3 archivos: NO es el repo canónico.
- `~/proyectos/styp/` está vacío y es obsoleto (plan de reescritura descartado a favor de este repo).

## Deploy (Dell gabo-Inspiron-5720, alias ssh `homelab`)
1. Build: `cargo build --release` en asus-tuf.
2. Backup remoto: `ssh homelab 'cp /home/gabo/study-platform/study-platform /home/gabo/study-platform/study-platform.bak-$(date +%Y%m%d)'`
3. `scp target/release/study-platform homelab:/home/gabo/study-platform/study-platform.new` — **NUNCA al path final**: el binario está en ejecución y `scp` falla (ETXTBSY).
4. `ssh homelab 'cd /home/gabo/study-platform && mv study-platform.new study-platform && chmod +x study-platform'` (rename atómico; el proceso viejo sigue con su inode).
5. Reinicio: `ssh homelab 'echo <password sudo> | sudo -S systemctl restart study-platform'` — la unidad es de **SISTEMA** (`/etc/systemd/system`), NO de usuario: `systemctl --user` da "unit not found".
6. Verificar: `curl http://100.92.172.48:8091/health` → esperado `database/mikareno_mcp/genoffice_mcp: true`.

## Infra (verificada 2026-10-14)
- App en la Dell: `0.0.0.0:8091` (servicio systemd `study-platform.service`), tailnet `http://100.92.172.48:8091`.
- OJO: el puerto **8090 de la Dell ya está ocupado** por otro servicio local; usar siempre 8091.
- mikareno-mcp: Python/FastMCP en asus-tuf (`0.0.0.0:8080`). La Dell lo ve en `http://100.115.62.97:8080/mcp` (tailnet); en local `http://127.0.0.1:8080/mcp`. Nunca `127.0.0.1` desde la Dell: apunta a la Dell misma y cae en mock mode.
- GenOffice MCP en la Dell: `http://127.0.0.1:8453/mcp` (solo local en la Dell; verificado: proceso `genoffice` v0.11.0).
- BD: **la que cuenta es la de la Dell** (`/home/gabo/study-platform/data/app.db`, ~1.5 MB, producción). La `data/app.db` local es dev/stale (~100 KB): no confundir ni mezclar.
- Backup: cron diario 3:15 → `backup-db.sh` (sqlite `.backup` a `data/backups/`, retención 14 días).
- Dell con recursos justos (4 cores, 7.1 GB): el servicio corre con MemoryMax=1G.

## Config `.env` (gitignored — diferencias dev/prod)
- asus-tuf (dev): `PORT=8090`, `HOST=0.0.0.0`, `MIKARENO_MCP_URL=http://127.0.0.1:8080/mcp`, `GENOFFICE_MCP_URL=http://127.0.0.1:8453/mcp`.
- Dell (prod): `PORT=8091`, `HOST=127.0.0.1` (el servicio lo pasa a `0.0.0.0:8091` con `--addr`), `MIKARENO_MCP_URL=http://100.115.62.97:8080/mcp`.
- Nunca secretos en source. OpenRouter: key SEPARADA de OpenCode, presupuesto máximo $2 USD.

## Health
- `/health` hace probe MCP real (POST `initialize`) con cache TTL 30 s: `mikareno_mcp`/`genoffice_mcp` reflejan estado real, no están hardcodeados.

## Seguridad (invariantes — no se negocian)
- NUNCA secretos en código ni en logs; `.env` gitignored.
- No exponer puertos a internet: acceso solo vía Tailscale.
- MCP propio: header `X-API-Key`, solo localhost.
- No modificar firewall, Tailscale, GenOffice MCP ni config de OpenCode.
- No instalar paquetes de sistema sin autorización.
- Confirmar antes de operaciones destructivas.

## Modelo constructor
- `poolside/laguna-s-2.1:free` — no cambiar de modelo ni consumir créditos de pago sin autorización explícita.

## Workflow
1. Diseñar contrato (structs/interface) → 2. código → 3. `cargo check` →
4. `cargo test` → 5. `cargo build` → 6. test de integración real →
7. documentar limitaciones → 8. solo entonces "terminado".

## Prioridad
1. Núcleo funcionando → 2. tests verdes → 3. diseño → 4. operativa.
   IA (OpenRouter): fase posterior, hoy apagada.

## Debugging
- `sudo journalctl -u study-platform -n 50` — logs de la app (unidad de sistema, NO `--user`)
- `journalctl --user -u mikareno-mcp` — logs del MCP
- `ss -tlnp | grep -E '8080|8091|8453'` — puertos
- No asumir: verificar con `curl` o test directo.
