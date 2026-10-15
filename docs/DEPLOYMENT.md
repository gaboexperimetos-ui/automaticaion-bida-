# Despliegue

## Prerrequisitos

### Remoto (gabo-Inspiron-5720, Ubuntu 26.04)
- ✅ SSH acceso (`gabo@100.92.172.48`, key `~/.ssh/id_ed25519_opencode`)
- ✅ Tailscale activo (IP: 100.92.172.48)
- ✅ Python 3.14.4 (system)
- ✅ GenOffice MCP en port 8453
- ✅ 35 GB disk libre
- ⚠️ 710 MB RAM libre (2 GB available)
- ⚠️ Load avg 9.46 en 4 cores
- ❌ Python venv (needs recreation — see ADR-004)
- ❌ Playwright/Chromium (needs install)
- ❌ Rust toolchain (NOT on remote)
- ❌ OpenRouter API key (pending architect approval)

### Local (asus-tuf, CachyOS)
- ✅ Rust toolchain (pending install — rustup)
- ✅ 244 GB disk libre
- ✅ Node v26, Python 3.14.7

## Pipeline de build y deploy

```
┌──────────────┐         1. cargo build --release           ┌──────────────┐
│   asus-tuf    │ ─────────────────────────────────────────→ │   asus-tuf    │
│  (build env)  │            study-platform (binary)         │  (artifact)  │
└──────────────┘                                            └──────┬───────┘
                                                                   │
           2. scp binary                 ┌──────────────┐         │
           study-platform ─────────────→ │ Ubuntu 22.04 │         │
           gabo@100.92.172.48             │  (remoto)    │ ◄───────┘
                                          └──────────────┘
                                           3. binary en /home/gabo/study-platform/
                                           4. systemd restart
```

# Port: 8090 was OCCUPIED locally by a python process at doc time
# (verified 2026-10-10: ss → 127.0.0.1:8090 users:(("python",pid=126454))).
# NEVER assume 8090 on the remote — verify there first (runbook §2),
# then set the chosen port in BOTH .env PORT AND unit ExecStart --addr.
# 8091 is the default alternative.

### Paso 1: Build (asus-tuf)

```bash
cd /home/gabo/study-platform
cargo build --release

# Verificar glibc compatibility (ADR-010)
# Max GLIBC requirement must be <= remote glibc version
readelf -V target/release/study-platform | grep GLIBC_2 | awk -F'GLIBC_' '{print $2}' | tr -d ',' | sort -V | tail -1
# Output: 34 -> binary requires GLIBC_2.34 (remote has glibc 2.43) ✅

# NOTE: musl static build BLOCKED — musl-gcc not available on asus-tuf (no sudo)
# Binary is dynamically linked against glibc; verified compatible via readelf
# See ADR-010 for rationale
```

### Paso 2: Transferir (NOT EXECUTED — REQUIRES AUTHORIZATION)

```bash
# SCP binario (encriptado)
scp -i ~/.ssh/id_ed25519_opencode \
    target/release/study-platform \
    gabo@100.92.172.48:/home/gabo/study-platform/

# SCP configuración (no incluye .env con secrets)
scp -i ~/.ssh/id_ed25519_opencode \
    .env.example \
    gabo@100.92.172.48:/home/gabo/study-platform/
```

### Paso 3: Configurar en remoto (NOT EXECUTED — REQUIRES AUTHORIZATION)

```bash
# SSH al remoto
ssh -i ~/.ssh/id_ed25519_opencode gabo@100.92.172.48

# Crear directorios
mkdir -p /home/gabo/study-platform/{data,notes}

# Crear .env (manualmente, con secrets locales)
# CLAVE EXACTA: OPENROUTER_MAX_COST_USD (src/config.rs:82; la variante sin
# _USD se ignora silenciosamente). PORT/HOST son solo informativos para Config:
# el socket lo ata el CLI --addr (src/main.rs:95) — ver Paso 4.
cat > /home/gabo/study-platform/.env << 'EOF'
OPENROUTER_API_KEY=***
SESSION_SECRET=***  # openssl rand -hex 32 ; REQUIRED, >=32 chars
HOST=127.0.0.1
PORT=8091
DB_PATH=/home/gabo/study-platform/data/app.db
NOTES_DIR=/home/gabo/study-platform/notes
MIKARENO_MCP_URL=http://127.0.0.1:8080/mcp
GENOFFICE_MCP_URL=http://127.0.0.1:8453/mcp
OPENROUTER_MODEL=cohere/north-mini-code:free
OPENROUTER_MAX_TOKENS=8192
OPENROUTER_MAX_COST_USD=2.0
CUSTOM_MCP_API_KEY=***  # openssl rand -hex 32
RUST_LOG=study_platform=info,tower_http=info
EOF

chmod 600 /home/gabo/study-platform/.env

# Inicializar DB — FLAG --migrate (NO es subcomando `migrate`):
/home/gabo/study-platform/study-platform --migrate
# expected: "Database migrations complete" / "Migrations applied. Exiting."
# (embedded sqlx: 20240101000000_init, ..._seed, ..._knowledge, ..._ai)
# Primer arranque — crear admin:
/home/gabo/study-platform/study-platform --create-admin admin
# NOTA: db.rs imprime `study-platform set-password <user>` como follow-up.
# Ese comando NO EXISTE (solo --addr/--migrate/--create-admin) — ignorar esa línea.
```

### Paso 4: Service systemd (remoto)

> NOT EXECUTED — remote steps below REQUIRE ARCHITECT AUTHORIZATION.
> The unit below MUST match the shipped `systemd/study-platform.service`
> (Type=simple, User/Group=gabo, WorkingDirectory+EnvironmentFile,
> Restart=always/RestartSec=10, MemoryMax=1G, CPUQuota=200%,
> `After=network.target` ONLY — no mikareno dependency, M10 out of scope).
> Serving is the DEFAULT (no `serve` subcommand): ExecStart MUST pass
> `--addr 127.0.0.1:<verified-free-port>` — env PORT alone does nothing.
> 8090 was occupied locally at doc time → 8091 is the default alternative,
> but ALWAYS verify on the remote first (runbook §2).

```bash
# Como root (requiere sudo) — NOT EXECUTED
sudo tee /etc/systemd/system/study-platform.service > /dev/null << 'EOF'
[Unit]
Description=Study Platform - Academic Productivity
After=network.target

[Service]
Type=simple
User=gabo
Group=gabo
WorkingDirectory=/home/gabo/study-platform
EnvironmentFile=/home/gabo/study-platform/.env
# Localhost-only bind. Port MUST match the verified-free port from
# docs/DEPLOY_RUNBOOK_M10.md step 2 and the PORT value in .env.
# 8090 was occupied at runbook draft time, so 8091 is the default alternative.
ExecStart=/home/gabo/study-platform/study-platform --addr 127.0.0.1:8091
Restart=always
RestartSec=10
MemoryMax=1G
CPUQuota=200%
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

# Iniciar
sudo systemctl daemon-reload
sudo systemctl enable study-platform
sudo systemctl start study-platform
sudo systemctl status study-platform
```

## Verificación local ejecutada (M11, 2026-10-10, asus-tuf)

```text
$ ./target/release/study-platform --version
study-platform 0.1.0
$ ./target/release/study-platform --help
Usage: study-platform [OPTIONS]
  -a, --addr <ADDR>              Bind address [default: 127.0.0.1:8090]
      --migrate                  Run database migrations and exit
      --create-admin <USERNAME>  Create initial admin user
$ sqlite3 --version
3.53.4 2026-07-24 ... (64-bit)
$ ss -tlnp | grep -E '8090|8091'
127.0.0.1:8090 ... users:(("python",pid=126454,fd=47))   # 8090 OCCUPIED → 8091 default
$ sha256sum target/release/study-platform
ca9a959060a42ba63b3bc4aef55eab1f49b05e9614406947096e5ae70b90d619  (6983528 bytes)
$ cargo --version / rustc --version
NOT AVAILABLE (no Rust toolchain on this host) → cargo check/test/build NOT VERIFIED HERE.
```
Remote-only steps (ssh/scp/systemctl/systemctl-on-remote, crontab install):
NOT VERIFIED HERE — marked NOT EXECUTED throughout this doc.

## Recuperación

### Escenario 1: App crashea / OOM

```bash
# 1. Verificar estado
systemctl status study-platform
free -m
journalctl -u study-platform -n 30

# 2. Detener (liberar RAM)
sudo systemctl stop study-platform

# 3. Verificar liberación
free -m  # memory debería subir

# 4. Reiniciar
sudo systemctl start study-platform
sudo systemctl status study-platform

# 5. Si persiste el problema:
# Revisar logs de MCP, OpenRouter timeouts, etc.
journalctl -u study-platform -f
```

### Escenario 2: MCP Mikareno caído

```bash
# Si MCP se migra al remoto:
sudo systemctl status mikareno-mcp
sudo journalctl -u mikareno-mcp -n 30

# Restart
sudo systemctl restart mikareno-mcp
sudo systemctl status mikareno-mcp

# Verificar port
ss -tlnp | grep 8080
```

### Escenario 3: Backup y restore

```bash
# Backup (diario via cron) — online-safe snapshot (no hace falta parar el servicio)
# NOT EXECUTED on remote — procedure proven locally on temp files (runbook §7)
/usr/bin/sqlite3 /home/gabo/study-platform/data/app.db \
    ".backup /home/gabo/study-platform/data/backups/app_$(date +%Y%m%d).db"

# Verify backup integrity BEFORE trusting it:
/usr/bin/sqlite3 /home/gabo/study-platform/data/backups/app_<STAMP>.db \
    "PRAGMA integrity_check;"
# expected: ok

# Restore — ALWAYS stop the service first; NEVER cp over the live app.db
# while the service is running (use sqlite3 .backup for online snapshots).
sudo systemctl stop study-platform
cp /home/gabo/study-platform/data/backups/app_<STAMP>.db \
   /home/gabo/study-platform/data/app.db
/usr/bin/sqlite3 /home/gabo/study-platform/data/app.db "PRAGMA integrity_check;"
# expected: ok
sudo systemctl start study-platform

# Verify — real route is /health (NOT /api/health), on the VERIFIED port (8091 default)
curl -s http://127.0.0.1:8091/health
```

### Escenario 4: Update del binario

```bash
# Build en asus-tuf
cargo build --release

# Deploy (sin downtime)
scp target/release/study-platform gabo@100.92.172.48:/tmp/study-platform

# Verificar binary
ssh gabo@100.92.172.48 '
    /tmp/study-platform --version &&
    mv /home/gabo/study-platform/study-platform /tmp/study-platform.bak &&
    mv /tmp/study-platform /home/gabo/study-platform/study-platform &&
    chmod +x /home/gabo/study-platform/study-platform
'

# Restart
sudo systemctl restart study-platform
sudo systemctl status study-platform --no-pager
```

## Monitoreo

### Recursos del sistema
```bash
# Via SSH al remoto
watch -n 5 'free -m && echo "---" && cat /proc/loadavg'
```

### Logs de la app
```bash
journalctl -u study-platform -f
journalctl -u study-platform --since "1 hour ago"
```

### Métricas y health (rutas reales — `src/handlers/mod.rs`)
```bash
curl -s http://127.0.0.1:8091/health          # liveness (ruta real: /health)
curl -s http://127.0.0.1:8091/api/config      # config pública (sin secrets)
# NO EXISTEN: /api/health, /api/metrics, /admin/* — no consultarlos.
```

### Cron: backups diarios
```bash
# Crontab del usuario gabo (en remoto)
0 2 * * * /usr/bin/sqlite3 /home/gabo/study-platform/data/app.db ".backup /home/gabo/study-platform/data/backups/app_$(date +\%Y\%m\%d).db"
# Retención: borrar backups > 30 días
0 3 * * * find /home/gabo/study-platform/data/backups/ -name "app_*.db" -mtime +30 -delete
```

## Rollback

### Si la nueva versión falla
```bash
# 1. Detener
sudo systemctl stop study-platform

# 2. Restaurar backup de binary
cp /home/gabo/study-platform/study-platform.bak \
   /home/gabo/study-platform/study-platform
chmod +x /home/gabo/study-platform/study-platform

# 3. Restaurar DB si es necesario (NOT EXECUTED)
cp /home/gabo/study-platform/data/backups/app_<PRE_UPGRADE_STAMP>.db \
   /home/gabo/study-platform/data/app.db
/usr/bin/sqlite3 /home/gabo/study-platform/data/app.db "PRAGMA integrity_check;"
# expected: ok

# 4. Reiniciar
sudo systemctl start study-platform
```

## Actualización del MCP Mikareno (cuando se migre)

> **NOTA:** Esto NO se ejecuta en esta misión. Es el procedimiento futuro.

```bash
# 1. SSH al remoto
ssh gabo@100.92.172.48

# 2. Recrear venv con requirements.txt
python3 -m venv /home/gabo/mikareno-mcp/venv
/home/gabo/mikareno-mcp/venv/bin/pip install -r /home/gabo/mikareno-mcp/requirements.txt

# 3. Instalar Chromium para Playwright
/home/gabo/mikareno-mcp/venv/bin/playwright install --with-deps chromium

# 4. Configurar MCP_HOST=127.0.0.1 en .env
sed -i 's/MCP_HOST=0.0.0.0/MCP_HOST=127.0.0.1/' /home/gabo/mikareno-mcp/.env

# 5. Copiar service file
sudo cp /home/gabo/mikareno-mcp/mikareno-mcp.service \
         /etc/systemd/system/mikareno-mcp.service

# 6. Iniciar y habilitar
sudo systemctl daemon-reload
sudo systemctl enable mikareno-mcp
sudo systemctl start mikareno-mcp
sudo systemctl status mikareno-mcp
```
