# Deploy Runbook — Mission 10 (study-platform, remote `gabo-inspiron-5720`)

> Status: DRAFT. Every remote-execution step below is marked **NOT EXECUTED**.
> No SSH/SCP/systemctl-on-remote was run by Agent B. Service install/start
> steps **REQUIRE ARCHITECT AUTHORIZATION — DO NOT EXECUTE** without approval.

Verified artifact (local, 2026-10-10):

- Binary: `target/release/study-platform`, 6982696 bytes, SHA256
  `d9a84744b07bf3a762ad12ac7d56e0e139370801402089d8e7e8466951aa841f`
- `file`: ELF 64-bit LSB pie executable, x86-64, dynamically linked,
  interpreter `/lib64/ld-linux-x86-64.so.2`, stripped.
- `ldd` needs only: `libgcc_s.so.1`, `libm.so.6`, `libc.so.6` (+ vdso/ld-linux).
- Max GLIBC required: **2.38** (`readelf -V | grep -o GLIBC_[0-9.]* | sort -V | tail -1`).
  Remote glibc is 2.43 → **compatible (2.38 ≤ 2.43)**. Dynamic build, not static musl.
- `--version` → `study-platform 0.1.0`. `--help` works (see §3).
- Remote Tailscale (local `tailscale status` check only): `100.92.172.48`
  `gabo-inspiron-5720` **active**. No SSH attempted.
- STALENESS NOTE (M11, 2026-10-10): the local binary has been rebuilt since —
  now `ca9a959060a42ba63b3bc4aef55eab1f49b05e9614406947096e5ae70b90d619`,
  6983528 bytes. RE-RUN §1.L1–L2 (hash + `readelf` GLIBC check) against the
  exact artifact before any deploy; do NOT trust the M10 hash above.
- TOOLCHAIN NOTE (M11): `cargo`/`rustc` are NOT installed on this host, so
  `cargo test`/`cargo build` re-verification is NOT VERIFIED HERE.

## 0. CLI-vs-doc mismatches (found in M10; DOC FIXES APPLIED in M11)

M10 recorded these read-only (code untouched, docs untouched). In M11
(docs agent) each item below was fixed in `docs/DEPLOYMENT.md` + `README.md`
(code itself unchanged — fixes are doc-side only). Kept here as audit record.

Actual CLI (`src/main.rs`, clap flags — re-confirmed 2026-10-10 via `--help`):

```text
Usage: study-platform [OPTIONS]
  -a, --addr <ADDR>              Bind address [default: 127.0.0.1:8090]
      --migrate                  Run database migrations and exit
      --create-admin <USERNAME>  Create initial admin user
```

Mismatches vs `docs/DEPLOYMENT.md` and `src/db.rs` message:

1. Doc §Paso 3 says `study-platform migrate` (subcommand). **Wrong.**
   Correct: `study-platform --migrate` (flag). Same for §Paso 4
   `ExecStart=... study-platform serve` — there is **no `serve` subcommand**;
   serving is the default when no flag is given. Correct:
   `study-platform --addr 127.0.0.1:<PORT>`.
2. `src/db.rs:110` tells the operator to run `study-platform set-password <user>`.
   **No such CLI flag/subcommand exists** (`--help` shows only
   `--addr/--migrate/--create-admin`). Dead instruction; password rotation
   procedure is therefore incomplete.
3. Doc §Paso 3 `.env` template uses `OPENROUTER_MAX_COST=2.0`. **Wrong key.**
   `src/config.rs:82` reads `OPENROUTER_MAX_COST_USD` (default `2.0`).
   The doc key is silently ignored (falls back to default — same value today,
   but a custom budget set under the doc key would have no effect).
4. Health endpoint: doc claims `curl .../api/health` and `.../api/metrics`.
   Actual route (`src/handlers/mod.rs:33`) is **`GET /health`**.
   No `/api/health` or `/api/metrics` route exists. Use `/health`.
5. Bind precedence: doc implies `PORT`/`HOST` env vars control the listener.
   They populate `Config`, but the listener binds to CLI `--addr`
   (`src/main.rs:95`, default `127.0.0.1:8090`). The systemd unit MUST pass
   `--addr 127.0.0.1:<verified-free-port>` or the env PORT is cosmetic.
6. Doc §Paso 4 unit has `After=network.target mikareno-mcp.service`.
   Migration is out of scope for M10; the shipped unit uses only
   `After=network.target` (see `systemd/study-platform.service`).

Resolution (M11 docs agent): items 1, 3, 4, 5, 6 fixed in
`docs/DEPLOYMENT.md` (flag-form `--migrate`, no `serve` subcommand, unit
copied from the shipped file, `OPENROUTER_MAX_COST_USD`, `/health` only,
`--addr`-wins note) and in `README.md` (env table + routes section).
Item 2 (dead `set-password` line in `src/db.rs:110`) is code-side and
unchanged — both docs now explicitly warn the command does not exist.

## 1. Pre-flight checklist (local artifact + remote — remote steps NOT EXECUTED)

```bash
# L1. Artifact hash + size (LOCAL — executed, OK)
sha256sum target/release/study-platform
# expected: d9a84744b07bf3a762ad12ac7d56e0e139370801402089d8e7e8466951aa841f
stat -c '%s bytes' target/release/study-platform
# expected: 6982696 bytes

# L2. GLIBC compat (LOCAL — executed, OK)
readelf -V target/release/study-platform | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -1
# expected: 2.38 (must be <= 2.43)
file target/release/study-platform
# expected: ELF 64-bit LSB pie executable, x86-64, dynamically linked, ... stripped
ldd target/release/study-platform
# expected: libgcc_s.so.1, libm.so.6, libc.so.6 only (+ vdso, ld-linux)

# R1. Remote pre-flight (NOT EXECUTED — REQUIRES AUTHORIZATION)
ssh -i ~/.ssh/id_ed25519_opencode gabo@100.92.172.48 ' \
  uname -m && \
  ldd --version | head -1 && \
  df -h /home/gabo | tail -1 && \
  free -m | head -2 && \
  ls -la /home/gabo/study-platform/ 2>/dev/null || echo NO_REMOTE_DIR_YET'
# expected: x86_64; ldd ... GLIBC 2.43; disk OK; mem noted (remote is constrained ~2GB);

# R2. .env keys present + perms (NOT EXECUTED — REQUIRES AUTHORIZATION)
ssh -i ~/.ssh/id_ed25519_opencode gabo@100.92.172.48 ' \
  stat -c "%a %n" /home/gabo/study-platform/.env && \
  for k in SESSION_SECRET PORT HOST DB_PATH NOTES_DIR MIKARENO_MCP_URL GENOFFICE_MCP_URL OPENROUTER_MODEL OPENROUTER_MAX_TOKENS OPENROUTER_MAX_COST_USD; do \
    grep -q "^$k=" /home/gabo/study-platform/.env && echo "OK $k" || echo "MISSING $k"; \
  done'
# expected: 600 /home/gabo/study-platform/.env; OK for every key.
# REQUIRED: SESSION_SECRET (>=32 chars; generate: openssl rand -hex 32).
# OPTIONAL (app runs without): OPENROUTER_API_KEY, CUSTOM_MCP_API_KEY, RUST_LOG.
# WARNING: use OPENROUTER_MAX_COST_USD, NOT OPENROUTER_MAX_COST (see §0.3).
chmod 600 /home/gabo/study-platform/.env   # part of R2, NOT EXECUTED

# R3. data/notes dirs (NOT EXECUTED — REQUIRES AUTHORIZATION)
ssh -i ~/.ssh/id_ed25519_opencode gabo@100.92.172.48 ' \
  mkdir -p /home/gabo/study-platform/data/backups /home/gabo/study-platform/notes && \
  ls -ld /home/gabo/study-platform/data /home/gabo/study-platform/notes'
# expected: drwxr-xr-x ... data, notes (backups/ inside data)
```

## 2. Port selection (never assume 8090 — NOT EXECUTED on remote)

Local fact (executed): `127.0.0.1:8090` is **OCCUPIED locally** by a python
process (`ss -tlnp | grep 8090` → `python pid=126454`). `8091–8093` are free
locally, but that says nothing about the remote — always check there.

```bash
# P1. Check remote listeners (NOT EXECUTED — REQUIRES AUTHORIZATION)
ssh -i ~/.ssh/id_ed25519_opencode gabo@100.92.172.48 'ss -tlnp | grep -E "8090|8091|8092" || echo NO_809X_LISTENERS'
# P2. Pick the port: first free in 8091, 8092, 8093... (example below uses 8091)
# P3. Set it consistently in BOTH places: .env PORT=8091 AND unit ExecStart
#     --addr 127.0.0.1:8091 (CLI --addr wins for the actual bind, see §0.5).
# expected: one free port chosen, e.g. 8091; ss shows nothing on it before deploy.
```

## 3. DB migrate (NOT EXECUTED — REQUIRES AUTHORIZATION)

```bash
# M1. Copy binary (example; needs approval — NOT EXECUTED)
scp -i ~/.ssh/id_ed25519_opencode target/release/study-platform gabo@100.92.172.48:/home/gabo/study-platform/study-platform
# M2. Run migrations with the FLAG form (NOT EXECUTED)
/home/gabo/study-platform/study-platform --migrate
# expected log: "Database migrations complete" / "Migrations applied. Exiting."
# Migrations applied (embedded, sqlx): 20240101000000_init, ..._seed, ..._knowledge, ..._ai
# M3. Create admin on first run (NOT EXECUTED)
/home/gabo/study-platform/study-platform --create-admin admin
# expected: "Admin user created" log. NOTE mismatch §0.2: the printed
# 'set-password' follow-up command does not exist — do not trust that line.
```

## 4. Service install / enable / start (NOT EXECUTED — REQUIRES ARCHITECT AUTHORIZATION)

Unit file ships in-repo at `systemd/study-platform.service` (Type=simple,
User=gabo, WorkingDirectory + EnvironmentFile, Restart=always/RestartSec=10,
MemoryMax=1G, CPUQuota=200%, `--addr 127.0.0.1:8091` localhost bind,
`After=network.target` only).

```bash
# S1. Install unit (NOT EXECUTED — REQUIRES AUTHORIZATION)
sudo cp /home/gabo/study-platform/systemd/study-platform.service /etc/systemd/system/study-platform.service
# If editing port: sudoedit /etc/systemd/system/study-platform.service
#   -> ExecStart must equal the §2 chosen port.
sudo systemctl daemon-reload
sudo systemctl enable study-platform
sudo systemctl start study-platform
sudo systemctl status study-platform --no-pager
# expected: Active: active (running); Memory <= 1G; no crash-loop.
# S2. Logs on failure (NOT EXECUTED)
journalctl -u study-platform -n 50 --no-pager
# expected: "listening on http://127.0.0.1:<PORT>" or the real error.
```

## 5. Health verification via Tailscale (no public ports — NOT EXECUTED)

```bash
# H1. From any Tailscale-joined host, over the tailnet IP (NOT EXECUTED).
#     Nothing is exposed publicly; the app binds 127.0.0.1 on the remote,
#     so this curl must run ON the remote (via ssh) or via an ssh tunnel.
ssh -i ~/.ssh/id_ed25519_opencode gabo@100.92.172.48 'curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:8091/health'
# expected: 200  (use the §2 chosen port, NOT a hardcoded 8090)
# H2. Body check (NOT EXECUTED)
ssh -i ~/.ssh/id_ed25519_opencode gabo@100.92.172.48 'curl -s http://127.0.0.1:8091/health'
# expected: JSON/text health payload (route is /health, NOT /api/health).
```

## 6. Backup / restore (procedure PROVEN LOCALLY on temp files — §7; remote steps NOT EXECUTED)

```bash
# B1. Nightly backup + 30-day retention — install as user gabo crontab on remote (NOT EXECUTED)
#   0 2 * * * /usr/bin/sqlite3 /home/gabo/study-platform/data/app.db ".backup /home/gabo/study-platform/data/backups/app_$(date +\%Y\%m\%d).db"
#   0 3 * * * find /home/gabo/study-platform/data/backups/ -name 'app_*.db' -mtime +30 -delete
# B2. Manual backup before any upgrade (NOT EXECUTED)
#   /usr/bin/sqlite3 /home/gabo/study-platform/data/app.db ".backup /home/gabo/study-platform/data/backups/app_$(date +%Y%m%d_%H%M).db"
#   sqlite3 /home/gabo/study-platform/data/backups/app_<STAMP>.db "PRAGMA integrity_check;"
# expected: ok
# B3. Restore (NOT EXECUTED — REQUIRES AUTHORIZATION)
#   sudo systemctl stop study-platform
#   cp /home/gabo/study-platform/data/backups/app_<STAMP>.db /home/gabo/study-platform/data/app.db
#   sqlite3 /home/gabo/study-platform/data/app.db "PRAGMA integrity_check;"
# expected: ok
#   sudo systemctl start study-platform
#   curl -s http://127.0.0.1:<PORT>/health   # expected 200
# NEVER copy the live app.db while the service is running — always stop first
# or use sqlite3 .backup (online-safe snapshot).
```

## 7. Local backup/restore proof (EXECUTED 2026-10-10, temp files only — real `data/app.db` untouched)

```bash
rm -rf /tmp/m10_test && mkdir -p /tmp/m10_test/backups
sqlite3 /tmp/m10_test/scratch.db "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t(v) VALUES ('alpha'),('beta'),('gamma');"
sqlite3 /tmp/m10_test/scratch.db ".backup /tmp/m10_test/backups/scratch_20261010.db"   # exit 0
sqlite3 /tmp/m10_test/scratch.db "INSERT INTO t(v) VALUES ('delta-after-backup');"
cp /tmp/m10_test/backups/scratch_20261010.db /tmp/m10_test/restored.db                  # exit 0
sqlite3 /tmp/m10_test/restored.db "SELECT count(*) FROM t; PRAGMA integrity_check;"
```

Results (executed, PASS):

- `scratch.db` after extra insert: 4 rows (proves backup is a point-in-time snapshot).
- `restored.db` (copy of backup): **3 rows**, `integrity_check` → **ok**.
- Conclusion: `.backup` → `cp` → `integrity_check` procedure restores the
  pre-backup state correctly. Same commands apply to `data/app.db` (§6).

## 8. Rollback (NOT EXECUTED — REQUIRES AUTHORIZATION)

```bash
# ROLLBACK binary (NOT EXECUTED)
#   sudo systemctl stop study-platform
#   cp /home/gabo/study-platform/study-platform.bak /home/gabo/study-platform/study-platform
#   chmod +x /home/gabo/study-platform/study-platform
#   ./study-platform --version   # expected: previous version string
# ROLLBACK DB if the new version migrated forward incompatibly (NOT EXECUTED)
#   cp /home/gabo/study-platform/data/backups/app_<PRE_UPGRADE_STAMP>.db /home/gabo/study-platform/data/app.db
#   sqlite3 /home/gabo/study-platform/data/app.db "PRAGMA integrity_check;"  # expected: ok
#   sudo systemctl start study-platform && curl -s http://127.0.0.1:<PORT>/health  # expected 200
# Keep .bak convention: before every upgrade,
#   mv study-platform study-platform.bak (NOT EXECUTED)
```

## 9. Recovery scenarios (all NOT EXECUTED)

```bash
# R1. Crash / OOM
#   systemctl status study-platform; free -m; journalctl -u study-platform -n 30
#   sudo systemctl stop study-platform; free -m   # memory should recover
#   sudo systemctl start study-platform; systemctl status study-platform
#   Unit self-heals via Restart=always (RestartSec=10); MemoryMax=1G caps the
#   blast radius on the constrained remote. If crash-looping, inspect journal
#   for MCP/OpenRouter timeouts before widening limits.
# R2. Mikareno MCP down (out of scope for M10 — app degrades, stays up)
#   ss -tlnp | grep 8080; curl -s http://127.0.0.1:8080/mcp -o /dev/null -w '%{http_code}\n'
#   Do NOT add a systemd dependency on mikareno (unit has After=network.target only).
# R3. OpenRouter down / budget hit (app runs without AI)
#   Unset or rotate OPENROUTER_API_KEY; AI endpoints 503/return degraded,
#   core notes/dashboard/health unaffected. Monthly cap OPENROUTER_MAX_COST_USD=2.0.
```

## 10. Steps requiring architect authorization (nothing below was executed)

1. All SSH/SCP to `gabo@100.92.172.48` (§1.R1–R3, §2.P1, §3, §4, §5, §6.B1–B3, §8, §9).
2. `sudo systemctl daemon-reload / enable / start / stop / restart` for
   `study-platform` on the remote.
3. Writing `/etc/systemd/system/study-platform.service`, `chmod 600 .env`,
   creating remote `data/`/`notes/` dirs, crontab install.
4. Any `cargo build --release` re-run that would overwrite the verified
   artifact (hash above) — re-verify §1.L1–L2 afterwards.
