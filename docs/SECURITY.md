# Seguridad

## Principios

1. **No secretos en código fuente.** Todo secreto via variables de entorno o `.env`.
2. **Principio de mínimo privilegio.** El proceso corre como `gabo` (no root).
3. **Defense in depth.** Múltiples capas: localhost binding, API key auth, sessions firmadas.
4. **No exposición a Internet.** La app escucha en `127.0.0.1` (localhost del remoto).
5. **No datos privados en logs.** Nunca loggear credenciales, tokens, o datos académicos.
6. **Verificación de credenciales.** Confirmar que las keys funcionan antes de usar.

## Modelo de amenazas

| Amenaza | Vector | Mitigación |
|---------|--------|-----------|
| Exposición de API key | Commit accidental a git | `.env` en `.gitignore`; `git-secrets` (pre-commit hook) |
| Acceso no autorizado a MCP propio | Sin autenticación | API key header (`X-API-Key`), CORS restrictivo |
| Data exfiltration vía OpenRouter | Envío de datos sensibles | Solo contexto mínimo; nunca enviar credenciales ni PII |
| XSS en frontend | Input del usuario sin sanitizar | HTMX auto-escapa; server-side rendering |
| Session hijacking | Cookie robada | Cookies HTTP-only, Secure, SameSite=Strict, signed |
| OOM del servidor | MCP con Chromium consume memoria | MemoryMax=1G, CPUQuota=200%, Restart=always |
| Ataque de fuerza bruta | Login attempts | Rate limiting (10 req/min por IP) |
| Acceso a datos académicos | MCP sin auth | MCP en localhost; app local-only |
| Acceso SSH | Root login con clave | Port 2222, key-only, root login disabled |

## Credenciales

### Variables de entorno (.env)

| Variable | Propósito | Seguridad |
|----------|-----------|-----------|
| `OPENROUTER_API_KEY` | API key para OpenRouter (aplicación) | Env var, no en source, $2 budget límite |
| `SESSION_SECRET` | Firma de cookies HTTP | 32 bytes random, generado via `openssl rand -hex 32` |
| `MIKARENO_MCP_URL` | URL del MCP de Mikareno | Default `http://127.0.0.1:8080/mcp` |
| `GENOFFICE_MCP_URL` | URL del MCP de GenOffice | Default `http://127.0.0.1:8453/mcp` |
| `CUSTOM_MCP_API_KEY` | API key para MCP propio | Header `X-API-Key`, comparado con `crypto_secure_eq` |

### Credenciales del MCP Mikareno (NO bajo control de la app)

- `MIKARENO_USER`, `MIKARENO_PASS` — manejados por el MCP (Python), no por la app Rust.
- El MCP hace login vía Playwright (headless Chromium).
- **No se almacenan ni loggean** en la app.
- Transferencia solo via SSH (`scp`/`rsync` over SSH).

## Autenticación

### Web app (usuario)
- Login: **Tailscale only** (el usuario accede via `100.92.172.48:8090`).
- No password login. La app no implementa auth de usuario (single-user, localhost).
- Cookies de sesión: `HttpOnly`, `SameSite=Strict`, `Secure` (cuando HTTPS disponible).
- `SESSION_SECRET`: 32 bytes generados via `openssl rand -hex 32`.

### MCP propio (external clients)
- Autenticación: header `X-API-Key`.
- Key comparada via `crypto_secure_eq` (constant-time).
- Key generada: `openssl rand -hex 32`.
- Stored en `.env` como `CUSTOM_MCP_API_KEY`.

## Permisos de archivos

```
/home/gabo/study-platform/
├── .env                    # 600 (owner read/write only)
├── data/
│   └── app.db              # 640 (owner rw, group r)
├── notes/                  # 750 (owner rwx, group rx)
└── study-platform          # 755 (binary)
```

## Privacidad de datos

### Datos académicos sensibles
- Notas, calendario, horarios, asistencia — provienen del MCP (read-only).
- **No enviados a OpenRouter** salvo contexto mínimo para preguntas del usuario.
- Cache en SQLite: TTL 180s, no se persisten datos sensibles sin need-to.
- Mensajes de chat: persistidos en SQLite local, **no enviados a terceros**.

### Datos de segunda mente (notas)
- Notas en Markdown en `notes/` directory (local).
- Metadatos en SQLite.
- **No sincronizados a la nube.**
- Backup via `rsync` a Gitea private repo (cifrado opcional).

## HTTPS / TLS

- **Desarrollo y local:** HTTP en localhost (no TLS necesario).
- **Remoto:** Accesso via Tailscale (100.92.172.48). Tailscale encripta el tráfico.
- **Opcional:** `nginx` reverse proxy con Let's Encrypt si se expone a Internet (NO recomendado).

## Seguridad del MCP Mikareno

- MCP en localhost (`127.0.0.1:8080`) — no accesible via Tailscale.
- MCP es read-only (17 tools, todas lectura).
- El MCP hace login a Mikareno via Playwright — las credenciales están en `.env` del MCP, no en la app.

## Seguridad del MCP GenOffice

- MCP en `0.0.0.0:8453` (ya expuesto via Tailscale).
- La app accede via `127.0.0.1:8453` (localhost del remoto).
- GenOffice MCP tiene su propia autenticación (API key en `ai-settings.json`).

## Incidentes y respuesta

| Incidente | Acción |
|-----------|--------|
| API key comprometida | Rotar inmediatamente, invalidar en OpenRouter dashboard |
| MCP Mikareno caído | Restart via systemd; check `journalctl --user -u mikareno-mcp` |
| OOM en remoto | `systemctl stop study-platform` → libera RAM → investigar |
| OpenRouter rate-limit | Reducir frecuencia de requests, cachear respuestas |
| Data leak | Investigar logs, audit trail en SQLite, notificar usuario |

## Checklist de seguridad (antes de producción)

- [ ] `.env` creado con `openssl rand -hex 32` para SESSION_SECRET
- [ ] `.gitignore` incluye `.env`, `data/`, `notes/`
- [ ] Permisos de archivo configurados (600 para .env)
- [ ] MCP Mikareno en `127.0.0.1` (no 0.0.0.0)
- [ ] App en `127.0.0.1:8090` (no 0.0.0.0)
- [ ] MCP propio con API key auth
- [ ] HTTPS via Tailscale (no HTTP a Internet)
- [ ] Backups funcionando y restaurables
- [ ] Audit trail en logs (sin datos sensibles)
