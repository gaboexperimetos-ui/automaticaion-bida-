# Decisiones de Arquitectura (ADRs)

## ADR-001: Backend en Rust

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09

### Contexto
Necesitamos un backend web para una plataforma de productividad académica que
consume APIs (MCP, OpenRouter), gestiona SQLite, y sirve una interfaz web.
Debe compilarse en `asus-tuf` y desplegarse a `gabo-Inspiron-5720` (ambos x86_64).

### Decisión
**Rust + Axum** para el backend.

### Justificación
- **Memory safety** sin GC (crucial para recursos limitados del remoto).
- **Single binary** con static linking → deployment trivial (`scp` + `systemd`).
- **Async first** (Tokio) → eficiente para SSE/MCP client.
- **Ecosystem maduro** para SQLite (sqlx, libsqlite3-sys), HTTP (axum, reqwest), SSE (eventsource-stream).
- Compila en asus-tuf (14 GB RAM, 244 GB disk) → recursos suficientes para builds.
- Binario resultante es portable a Ubuntu 26.04 (x86_64, glibc 2.40+).

### Alternativas descartadas
| Opción | Razón para descartar |
|--------|---------------------|
| Python/FastAPI | No hay Python en el remote (solo 3.14.4 sin venv); no hay Rust → doble runtime |
| Go | Compilación rápida pero no memory-safe; ecosistema MCP menos maduro |
| Node.js | Ya está disponible (v26) pero menor control de memoria; runtime pesado |
| Docker | El remote ya corre Docker (29.1.3) con alta carga; añadir contenedor aumenta overhead |

---

## ADR-002: SQLite como única base de datos

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09

### Contexto
Necesitamos almacenar: notas Markdown metadata, conversaciones de chat, cache de
MCP, métricas de uso, configuración. El remote tiene PostgreSQL (port 5432) pero
está sobre cargado (load avg 9.46, 710 MB RAM libre).

### Decisión
**SQLite** (single file, WAL mode, FTS5).

### Justificación
- **Zero configuración** — single file en `data/app.db`.
- **FTS5 nativo** — búsqueda full-text sin índice externo.
- **Backup trivial** — `sqlite3 app.db ".backup backup.db"`.
- **Low overhead** — ~50-100 MB RSS para el proceso SQLite vs ~80 MB para PostgreSQL.
- **Single-writer** tolerable para aplicación personal (1 usuario).
- PostgreSQL ya está corriendo en el remote bajo alta carga — no duplicar.

### Alternativas descartadas
| Opción | Razón |
|--------|-------|
| PostgreSQL (existente) | Ya sobre cargado (load 9.46). No justificar otro cliente conectándose. |
| Redis (cache) | Agrega servicio. Mejor usar SQLite cache table. |
| Indexador externo (Typesense) | Complejidad innecesaria para <10K notas. |

---

## ADR-003: Límites de recursos del MCP — CPUQuota semántica corregida

**Estado:** ✅ Corregida (defecto de MISIÓN 3)  
**Fecha:** 2026-10-09

### Contexto
La MISIÓN 3 recomendó `CPUQuota=40%` para el MCP Mikareno en el remoto. El
remote tiene 4 cores con load avg 9.46 (236% utilizado).

### Defecto identificado
**`CPUQuota=40%` significa 40% de UN CPU, no 40% de todos los CPUs.** En systemd:
- `CPUQuota=40%` = 40% de un core = 10% de la capacidad total de 4 cores.
- Para 40% de 4 cores usar: `CPUQuota=160%` (160% = 4 × 40%).
- `CPUQuota=200%` = 50% de 4 cores = 2 cores completos.

### Decisión corregida
| Límite | Valor | Justificación |
|--------|-------|---------------|
| `CPUQuota` | `200%` (removido temporalmente) | 50% de 4 cores. Durante scraping con Chromium pueden necesitarse picos de CPU. Mejor: `Nice=10` + `IOSchedulingPriority=7` sin límite estricto. |
| `MemoryMax` | `1G` | MCP idle: 96 MB RSS. Con Chromium: ~300-600 MB pico. 1G es margen seguro. |
| `Nice` | `10` | Baja prioridad — cede CPU a Jellyfin, Docker, OpenCode serve. |
| `Restart` | `always` + `RestartSec=10` | Auto-recuperación, pero con límite de restarts: `StartLimitBurst=5`, `StartLimitIntervalSec=60`. |

### Medición base
- MCP local: RSS 96 MB (idle), VmPeak 117 MB, 1 thread
- Playwright + Chromium adicional durante scraping: ~200-500 MB
- Total estimado pico: 300-600 MB (dentro de MemoryMax=1G)

### Alternativas descartadas
| Opción | Razón |
|--------|-------|
| `CPUQuota=150%` | Podría throttlear Chromium durante scraping crítico |
| `CPUQuota=40%` | Demasiado restrictivo (10% de 4 cores) → timeouts |
| No límites | Riesgo de OOM si Chromium leak |

---

## ADR-004: venv no portable — recrear en remoto

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09

### Contexto
La MISIÓN 3 sugirió sincronizar el venv (`rsync -a`) de CachyOS a Ubuntu.
Esto NO funciona debido a diferencias de sistema.

### Defecto identificado
- CachyOS es Arch-based; Ubuntu 26.04 es Debian-based.
- El venv de CachyOS enlaza contra glibc y librerías de CachyOS.
- Python 3.14.7 (venv) vs 3.14.4 (sistema remoto) — versión diferente.
- Playwright browsers están en `~/.cache/ms-playwright/` (local), no en venv.

### Decisión
**Recrear el venv en el remoto** con `requirements.txt` con versiones fijadas:
1. Copiar `requirements.txt` (text only, no binarios) via `scp`.
2. SSH al remoto: `python3 -m venv venv && venv/bin/pip install -r requirements.txt`.
3. `venv/bin/playwright install --with-deps chromium` (instala browser + system deps).

### requirements.txt (versiones fijadas)
```
fastmcp==4.0.11
playwright==1.63.0
httpx==0.28.1
mcp==2.3.0
pydantic==2.13.5
uvicorn==0.54.0
```

### Alternativas descartadas
| Opción | Razón |
|--------|-------|
| rsync venv | glibc incompatibilidad → import errors |
| pip install --platform | Complex, error-prone |
| Docker | Overhead no justificado (solo 1 proceso) |

---

## ADR-005: Systemd system-level service (no user-level) en remoto

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09

### Contexto
El MCP Mikareno corre como `systemd --user` service en asus-tuf. El remoto
no tiene `systemd --user` para el usuario `gabo` (verificado via SSH:
`pgrep -u gabo systemd --user` → not found).

### Decisión
**Crear servicio system-level** (`/etc/systemd/system/mikareno-mcp.service`)
en lugar de user-level.

### Justificación
- El remote no tiene sesión `systemd --user` activa para `gabo`.
- Los servicios existentes (genoffice-mcp, openclaw-node) también necesitan
  verificación de cómo se inician (screen/tmux/nohup o system-level).
- Un servicio system-level persiste sin necesidad de login de usuario.
- Requiere root para instalar, pero es más robusto para un servidor.

### Service file (propuesta)
```ini
[Unit]
Description=Mikareno Student MCP Server
After=network.target

[Service]
Type=simple
User=gabo
Group=gabo
WorkingDirectory=/home/gabo/mikareno-mcp
EnvironmentFile=-/home/gabo/mikareno-mcp/.env
ExecStart=/home/gabo/mikareno-mcp/venv/bin/python /home/gabo/mikareno-mcp/server.py
Restart=always
RestartSec=10
StartLimitBurst=5
StartLimitIntervalSec=60
MemoryMax=1G
CPUQuota=200%
Nice=10
IOSchedulingPriority=7
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

### Alternativas descartadas
| Opción | Razón |
|--------|-------|
| `loginctl enable-linger gabo` | Requiere root, activa systemd --user, pero menos robusto |
| `nohup` / `screen` | No auto-recupera, no monitorea |
| Docker | Overhead no justificado |

---

## ADR-006: GenOffice MCP en remoto para suite de oficina

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09

### Contexto
La suite de oficina debe funcionar sin depender de asus-tuf. El remote ya
tiene GenOffice MCP corriendo en port 8453.

### Decisión
**Proxy a GenOffice MCP local del remote** (`127.0.0.1:8453/mcp`).

### Verificación
- GenOffice MCP corre en remoto: PID 1549129, `LISTEN 0.0.0.0:8453`.
- Responde a HTTP (404 en `/` = esperado para MCP; SSE en `/mcp`).
- La app Rust en remoto puede conectar a `127.0.0.1:8453` directamente.

### Limitaciones
- GenOffice MCP necesita API key configurada para search/image/media.
- El remote tiene su propio `~/.config/GenOffice/ai-settings.json`.
- No se necesita mover nada: ya está funcionando en el remote.

---

## ADR-007: HTMX + Tailwind (no JS framework)

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09

### Decisión
**HTMX + Tailwind CSS**, server-rendered. No React, no Vue, no Svelte.

### Justificación
- App personal (1 usuario) → no necesita SPA complejo.
- HTMX: interacciones parciales via AJAX/SSE sin recargar página.
- Tailwind: utility-first CSS, bundle estático.
- Reduce surface area de seguridad (menos JS = menos XSS).
- Compatible con streaming SSE para OpenRouter.
- Bundle size: ~200 KB vs 2-5 MB de un framework JS completo.

### Alternativas descartadas
| Opción | Razón |
|--------|-------|
| React/Vite | Overhead de runtime, bundle grande, complejidad innecesaria |
| Alpine.js | Posible, pero HTMX es suficiente para este uso |
| Vanilla JS + Web Components | Más código manual, menos mantenible |

---

## ADR-008: Separación de modelos — desarrollo vs aplicación

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09

### Decisión
- **Desarrollo:** `poolside/laguna-s-2.1:free` (modelo constructor, no configurable).
- **Aplicación (OpenRouter):** `cohere/north-mini-code:free` (zero cost, 256K context).
- **Respaldo (application):** `gemma-3-4b-it` ($0.05/M, solo si se aprueba).

### Justificación
- El modelo constructor (OpenCode config) NO debe mezclarse con OpenRouter.
- OpenRouter API key está en `service.json` del local, NO configurado en opencode.json del remote.
- La aplicación necesita su propia API key en `.env` (env var `OPENROUTER_API_KEY`).
- No se comparte la misma key entre desarrollo y aplicación (separa billing).

---

## ADR-010: Binary portability — musl static target (no glibc cross-compile)

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09  
**Actualizado:** 2026-10-09 (MISIÓN 5)

### Contexto

La MISIÓN 3 asumió que un binario compilado en CachyOS (glibc 2.44) sería
compatible con Ubuntu 26.04 (glibc 2.43). **Esto es incorrecto.**

### Hallazgo

| Equipo | glibc | gcc | Notas |
|--------|-------|-----|-------|
| asus-tuf (CachyOS) | **2.44** | GCC 14+ | Build environment |
| gabo-Inspiron-5720 (Ubuntu 26.04) | **2.43** | GCC 15.2.0 | Runtime |

glibc es **forward-compatible pero NO backward-compatible**. Un binario compilado
con glibc 2.44 puede usar symbols que no existen en glibc 2.43, causando:
```
/lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.44' not found
```

### Decisión

**Compilar con el target `x86_64-unknown-linux-musl`** para producción.
musl produce binarios **estáticamente enlazados** — no dependen de glibc
del target. El binario funciona en cualquier Linux x86_64.

### Implementación

```bash
# 1. Instalar target musl
rustup target add x86_64-unknown-linux-musl

# 2. Build para producción (Ubuntu)
RUSTFLAGS='-C link-arg=-s' cargo build --release --target x86_64-unknown-linux-musl

# 3. Verificar (sin deploy)
scp target/x86_64-unknown-linux-musl/release/study-platform \
    gabo@100.92.172.48:/tmp/study-platform
ssh gabo@100.92.172.48 '/tmp/study-platform --version'
```

### Limitaciones

- Algunas crates que usan `dlopen` o dynamic loading pueden no funcionar con musl.
- `sqlx` con `lib-sqlite3-sys` compila SQLite con `cc` — necesita C compiler
  para el target musl (`musl-gcc`). Si no está disponible, usar `cargo-zigbuild`.
- Alternativa: usar `sqlx` con `sqlite` + `bundled` features, o `rusqlite` con `bundled`.

### Alternativas descartadas
| Opción | Razón |
|--------|-------|
| Build nativo (glibc 2.44) | Riesgo de incompatibilidad con glibc 2.43 en remoto |
| Build en remoto | Recursos insuficientes (2 GB free RAM, load 9.46) |
| Docker cross-build | Overhead no justificado |

---

## ADR-011: No copiar OpenRouter API key desde service.json

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09

### Contexto

`~/.config/opencode/service.json` contiene `OPENROUTER_API_KEY` y `GROQ_API_KEY`
como variables de entorno para OpenCode. La app Rust necesita su propia API key.

### Decisión

**No copiar la API key de OpenCode a la app.** La app necesita su propia key
configurada en `.env`:

```bash
# .env (en remoto, manual)
OPENROUTER_API_KEY=sk-or-xxx  # Key DEDICADA a la app, no la de OpenCode
```

### Justificación

- Separación de billing: OpenCode (development) y app (production) usan keys distintas.
- Si la key de OpenCode se revoca/compromete, no afecta a la app.
- Límites de gasto independientes.

---

## ADR-012: GenOffice MCP como dependencia opcional, no crítica

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09

### Contexto

La MISIÓN 3 trató GenOffice MCP como una dependencia activa para la suite de
oficina. Sin embargo, el MCP de GenOffice corre en `asus-tuf` (local), no en el
remoto, lo que significa que:

1. Si `asus-tuf` está apagado, la suite de oficina no funciona.
2. La app no puede depender de GenOffice MCP para funciones esenciales.

### Decisión

- **Suite de oficina básica:** Implementada nativamente en Rust (Markdown →
  HTML → PDF, generación de tablas simples, etc.).
- **Suite de oficina avanzada:** Proxy opcional al GenOffice MCP en `127.0.0.1:8453`
  (remoto — ya está corriendo). Si no disponible, funciona con limitaciones.
- **No hard dependency** sobre GenOffice MCP en el remoto.

### Verificación
- GenOffice MCP corre en remoto: port 8453, PID 1549129. ✅
- App puede usar `127.0.0.1:8453/mcp` como proxy opcional. ✅
- Si MCP cae, app degrades gracefully (funciones básicas siguen funcionando). ✅

---

## ADR-013: ADR-009 restringido — MCP propio no se expone públicamente

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09

### Contexto

ADR-009 recomendó cambiar `MCP_HOST=127.0.0.1` para el MCP de Mikareno. El MCP
propio de la app (rmcp, port 8081) fue descrito sin restricciones de exposición.

### Decisión

El MCP propio **NO se expone públicamente**:
- Puerto 8081: `127.0.0.1:8081` (localhost del remoto solamente).
- Autenticación: `X-API-Key` header (key generada via `openssl rand -hex 32`).
- No accesible via Tailscale ni LAN.
- Clientes externos (ej: OpenCode remote) se conectan via SSH tunnel si es necesario.

### Verificación de compatibilidad cliente
- Probar con OpenCode V2 MCP client antes de documentar.
- Verificar que `sse` transport works con `mcp-session-id` header.
- No asumir compatibilidad — testear con un cliente real.

---

## ADR-009: MCP host=127.0.0.1 (no 0.0.0.0) en remoto

**Estado:** ✅ Aprobada  
**Fecha:** 2026-10-09

### Decisión
Al migrar el MCP al remote, cambiar `MCP_HOST` de `0.0.0.0` a `127.0.0.1`.

### Justificación
- La aplicación Rust corre en el mismo servidor → localhost es suficiente.
- Evita exponer el MCP a Tailscale (`100.92.172.48:8080`) o LAN.
- El MCP contiene credenciales de Mikareno (`MIKARENO_USER/PASS`).
- Defense in depth: aunque Tailscale es privado, localhost es más seguro.

### Implementación
Editar `/home/gabo/mikareno-mcp/.env` (en remoto, después de migration):
```ini
MCP_HOST=127.0.0.1    # cambiado de 0.0.0.0
MCP_PORT=8080
MCP_PATH=/mcp
```

### Alternativas descartadas
| Opción | Razón |
|--------|-------|
| Mantener 0.0.0.0 | Riesgo de exposición innecesaria |
| Tailscale ACL | Más complejo, localhost es suficiente |
