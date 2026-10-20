# DEPLOY — Study Platform en la Dell (homelab)

Estado real verificado el **2026-10-19** (deployed 18:18). Este documento describe el
deploy vigente; `docs/DEPLOYMENT.md` es el runbook histórico del 2026-10-10 (obsoleto en
la parte de comandos de servicio).

- **Host destino**: `gabo-Inspiron-5720` (alias SSH `homelab`, tailnet `100.92.172.48`).
- **Origen del build**: `asus-tuf` (`/home/gabo/study-platform`, fuente canónica).
- **Ruta en la Dell**: `/home/gabo/study-platform/`.
- **Binario**: Rust, release **glibc** (no musl: en esta máquina no hay `musl-gcc`; el
  binario de producción siempre fue glibc, verificado por BuildID).

---

## 1. Servicio (systemd)

Unidad de **sistema** (NO de usuario): `systemctl --user` da *unit not found*.

| Campo | Valor |
|---|---|
| Archivo | `/etc/systemd/system/study-platform.service` |
| `User` / `Group` | `gabo` / `gabo` |
| `WorkingDirectory` | `/home/gabo/study-platform` |
| `EnvironmentFile` | `/home/gabo/study-platform/.env` |
| `ExecStart` | `/home/gabo/study-platform/study-platform --addr 0.0.0.0:8091` |
| `Restart` / `RestartSec` | `always` / `10` |
| `MemoryMax` / `CPUQuota` | `1G` / `200%` |
| `StandardOutput` / `StandardError` | `journal` / `journal` |
| `WantedBy` | `multi-user.target` |

- `systemctl is-enabled study-platform` → `enabled`; `is-active` → `active`.
- **Acceso**: `http://100.92.172.48:8091` (tailnet) y `http://127.0.0.1:8091` en la propia
  Dell. El puerto 8090 de la Dell está ocupado por otro servicio: **usar siempre 8091**.
- **No hace falta `sudo` para reiniciar el proceso**: la unidad corre como root y
  `Restart=always`; al hacer `pkill -x study-platform`, systemd levanta el binario solo
  (~11 s después: `RestartSec=10` + arranque). `sudo` solo hace falta para tocar la
  unidad (`daemon-reload`, `enable`, `restart`).

---

## 2. Deploy reproducible (desde `asus-tuf`)

```bash
cd /home/gabo/study-platform

# 1) Build release (glibc)
cargo build --release                      # -> target/release/study-platform

# 2) Backup del binario en servicio (sirve de rollback)
ssh homelab 'cp ~/study-platform/study-platform ~/study-platform/study-platform.prev'

# 3) Copiar el binario nuevo
scp target/release/study-platform homelab:~/study-platform/study-platform

# 4) Reiniciar el proceso (systemd lo levanta solo, sin sudo)
ssh homelab 'pkill -x study-platform'

# 5) Esperar el auto-restart (RestartSec=10 -> ~11 s; 6 s puede caer en la ventana muerta)
sleep 12
```

Si el `scp` del paso 3 falla con *Text file busy* (ETXTBSY,WRITE sobre el binario en
ejecución), usar la variante atómica:

```bash
scp target/release/study-platform homelab:~/study-platform/study-platform.new
ssh homelab 'cd ~/study-platform && mv study-platform.new study-platform && chmod +x study-platform'
```

### Verificación

```bash
ssh homelab 'systemctl is-active study-platform'                 # -> active
ssh homelab 'curl -fsS http://127.0.0.1:8091/health'            # -> 200 + status healthy
ssh homelab 'curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:8091/api/admin/metrics'  # -> 401 (esperado sin sesión)
ssh homelab 'journalctl -u study-platform -n 30 --no-pager'     # sin panic ni loop de reinicio
sha256sum target/release/study-platform                          # local
ssh homelab 'sha256sum ~/study-platform/study-platform'          # remoto: mismo hash
```

Referencia del deploy real del 2026-10-19: `sha256 = 491f1b9a53fdf5caa054180f92cb72c00505b3c9b89286632ae8f02f4c4eeb89`
(10.4 MB), `study-platform.prev` = `29982a17…` (binario anterior, para rollback).

**Crash-loop**: el 2026-10-19 18:18:29 el servicio entró en loop (`NRestarts=9`) por un
panic `Overlapping method route. Handler for GET /api/admin/backup already exists`
(`src/handlers/mod.rs:90`). Causa: ruta duplicada en el binario de ese momento. El código
actual registra `/api/admin/backup` una sola vez (`src/handlers/mod.rs:81`). Si reaparece,
mismo síntoma: leer el panic en el journal antes de seguir desplegando.

---

## 3. Rollback

```bash
ssh homelab 'cp ~/study-platform/study-platform.prev ~/study-platform/study-platform && pkill -x study-platform'
sleep 12
ssh homelab 'systemctl is-active study-platform && curl -fsS http://127.0.0.1:8091/health'
```

- El rollback del **binario** no toca la base de datos.
- Rollback de datos: `~/study-platform/data/backups/` guarda copias por fecha
  (`app-YYYYMMDD-HHMMSS.db` + `.sha256`) y `app.db.bak-YYYYMMDD-HHMMSS`; el backup diario
  corre por cron a las 03:15 (`~/study-platform/backup-db.sh`).

---

## 4. Entorno y datos en la Dell

`.env` (`/home/gabo/study-platform/.env`, permisos `600`, **gitignored, no se copia**):

| Variable | Valor (prod) |
|---|---|
| `PORT` | `8091` (el bind real lo decide `--addr`) |
| `HOST` | `127.0.0.1` |
| `DB_PATH` | `/home/gabo/study-platform/data/app.db` |
| `NOTES_DIR` | `/home/gabo/study-platform/notes` |
| `MIKARENO_MCP_URL` | `http://100.115.62.97:8080/mcp` (tailnet a `asus-tuf`) |
| `GENOFFICE_MCP_URL` | `http://127.0.0.1:8453/mcp` (GenOffice local de la Dell) |
| `SESSION_SECRET` | secreto de sesión (≥32 chars) — no mostrar ni versionar |
| `RUST_LOG` | `info` |

Datos y directorios:

- `data/app.db` — **la BD de producción** (~1.5 MB, 21 tablas, usuarios `admin` y `gabo`).
  La `data/app.db` de `asus-tuf` (~100 KB) es de desarrollo: no mezclarlas.
- `data/backups/` — backups automáticos (retención 14 días) + `.bak` manuales.
- `data/office/{docs,sheets,presentations}/` — archivos generados por la suite Office.
- `notes/` — notas markdown (NOTES_DIR).
- `logs/`, `bin/`, y backups de binarios `study-platform.bak-*` de despliegues previos.

---

## 5. Checklist post-deploy

- [ ] `systemctl is-active study-platform` → `active`; `journalctl -u study-platform -n 30` sin panic.
- [ ] `curl -fsS http://127.0.0.1:8091/health` → 200, `status: healthy`, `database: true`.
- [ ] `sha256sum` local == remoto (el binario en servicio es el recién compilado).
- [ ] `GET /api/admin/metrics` → **401** sin sesión; con sesión iniciada en `/login`
      (usuario `admin` o `gabo`) → 200 con el JSON de métricas.
- [ ] Rutas registradas en el binario desplegado (líneas de registro en
      `src/handlers/mod.rs` y `src/academic/mod.rs`): `/` (panel), `/dashboard`,
      `/dashboard/horario`, `/dashboard/notes`, `/dashboard/examenes`,
      `/dashboard/vida-escolar`, `/dashboard/subjects/{code}` → 302 sin sesión, 200 con sesión.
- [ ] `/dashboard/horario` **con datos reales**: depende del MCP de Mikareno por el túnel
      tailnet (`asus-tuf` → `100.115.62.97:8080`). Verificar desde la Dell:
      `curl -s -o /dev/null -w "%{http_code}\n" -X POST http://100.115.62.97:8080/mcp -H 'content-type: application/json' -H 'accept: application/json, text/event-stream' -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{...}}'`
      y que `/health` reporte `mikareno_mcp: true`.
- [ ] Sin fugas: la respuesta de `/health` y `/api/config` no expone `SESSION_SECRET` ni la
      key de OpenRouter.
