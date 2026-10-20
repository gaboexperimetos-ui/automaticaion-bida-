# Guía de arranque — Study Platform (styp)

Documento de entrada para **quien llega hoy al proyecto y tiene que ponerlo en
marcha sin contexto previo**. Todo lo que sigue está verificado contra la
máquina real el **2026-10-19**; los comandos están copiados de una sesión que
funcionó.

- Fuente canónica: `/home/gabo/study-platform` en **asus-tuf** (esta PC). Es un repo git
  con remoto `dell` (bare) en la homelab.
- Producción: **gabo-Inspiron-5720** (alias SSH `homelab`, tailnet `100.92.172.48`).
- Documentos hermanos: `AGENTS.md` (reglas e invariantes), `docs/DEPLOY.md` (deploy),
  `docs/API.md`, `docs/USER_GUIDE.md`, `docs/DECISIONS.md`.

---

## 1. Mapa de la infraestructura

| Componente | Dónde vive | Cómo se llega | Verificación |
|---|---|---|---|
| App (dev) | asus-tuf, `0.0.0.0:8090` | `http://127.0.0.1:8090` | `curl -s localhost:8090/health` |
| App (prod) | Dell, unidad systemd de **sistema** | `http://100.92.172.48:8091` (tailnet) | `curl -s http://100.92.172.48:8091/health` |
| Mikareno MCP | asus-tuf, FastMCP en Python | dev `http://127.0.0.1:8080/mcp` · prod `http://100.115.62.97:8080/mcp` | `journalctl --user -u mikareno-mcp -n 20` |
| GenOffice MCP | **sólo en la Dell** | `http://127.0.0.1:8453/mcp` (local en la Dell) | `curl` POST `initialize` |
| Base de datos real | `~/study-platform/data/app.db` **en la Dell** (~1.5 MB) | filesystem | `ls -l` |
| Base de datos local | `data/app.db` en asus-tuf (~100 KB, **stale**) | filesystem | no confundir |

> La BD que cuenta es la de la Dell. La de asus-tuf es de desarrollo y está desfasada.

---

## 2. Requisitos

| Herramienta | Versión verificada | Para qué |
|---|---|---|
| Rust / cargo | 1.99.0 | backend (axum + sqlx) |
| Python | 3.14.8 (venv) | Mikareno MCP (FastMCP + Playwright) |
| Playwright + Chromium | `chromium-1243` en `~/.cache/ms-playwright` | el MCP raspa la plataforma Mikareno |
| Tailscale | activo | acceso remoto; sin él no hay salida a `100.x` |
| Node + pnpm | presentes | sólo si se toca el frontend/GenOffice |

El frontend es **server-rendered** (HTML + Tailwind + HTMX dentro de `static/stitch-v2/`).
No hay paso de build de assets: se edita el HTML y listo.

---

## 3. Puesta en marcha local (paso a paso)

```bash
# 1) Código
cd /home/gabo/study-platform
git pull                       # o: git clone homelab:/home/gabo/study-platform.git

# 2) Configuración (si no existe .env)
cp .env.example .env
sed -i "s|^SESSION_SECRET=.*|SESSION_SECRET=$(openssl rand -hex 32)|" .env
sed -i "s|^AUTH_DISABLED=.*|AUTH_DISABLED=true|" .env      # dev: sin login
# MIKARENO_MCP_URL y GENOFFICE_MCP_URL: deja los defaults de .env.example

# 3) Levantar el MCP de Mikareno (si no está corriendo)
systemctl --user restart mikareno-mcp
systemctl --user status mikareno-mcp --no-pager

# 4) Comprobar que el MCP responde (debe ser HTTP 200 en ms)
curl -s -o /dev/null -w "MCP %{http_code} %{time_total}s\n" \
  -X POST http://127.0.0.1:8080/mcp \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"probe","version":"1"}}}'

# 5) Compilar y arrancar
cargo build
cargo run -- --addr 127.0.0.1:8090

# 6) Verificar
curl -s http://127.0.0.1:8090/health
```

`/health` esperado (app+MCP locales):

```json
{"components":{"database":true,"genoffice_mcp":true,"mikareno_mcp":true,"openrouter":true},
 "status":"healthy","timestamp":"...","version":"0.1.0"}
```

`genoffice_mcp:false` es normal en dev: ese MCP **sólo existe en la Dell**.

### Ojo con el puerto

- En **asus-tuf** el puerto libre es el **8090**.
- En la **Dell** el 8090 está ocupado por otro servicio: usa siempre el **8091**.

---

## 4. Tests

```bash
cd /home/gabo/study-platform
cargo test --all              # suite completa (~1-2 min; un test de red tarda ~2 min)
cargo test --test e2e         # un archivo concreto
cargo test --lib integrations::mikareno   # un módulo de la lib
```

Estado verificado 2026-10-19: **0 fallos** (267 tests de lib + suites de
integración, ~533 en total). Tests que tocan la red real están marcados
`ignored` o tardan; el de OpenRouter en vivo puede fallar por *rate limit* del
modelo gratuito (es flaky, no un defecto).

Mapa rápido de los archivos de test:

| Archivo | Cubre |
|---|---|
| `tests/e2e.rs` | flujo end-to-end del router: /mcp, /office, /api/notes, IA |
| `tests/academic_integration.rs` | rutas académicas (`/dashboard/...`) |
| `tests/auth_sessions.rs` | cookies, sesiones, `require_auth_state` |
| `tests/pages_smoke.rs` | que cada página renderiza 200 y HTML balanceado |
| `tests/notes_ui.rs`, `tests/search_ui.rs`, `tests/settings_ui.rs`, `tests/schedule_ui.rs` | UI de cada módulo |
| `tests/admin_routes.rs`, `tests/admin_backup_routes.rs`, `tests/admin_panel_html.rs` | panel F9 y backups |
| `tests/security.rs` | cabeceras, fugas de secretos |
| `tests/shell_integrity.rs` | integridad del shell: `data-path` ↔ rutas registradas, un solo item activo |

Barridos automáticos útiles (viven en `/tmp/opencode/`, copiarlos al repo si
se quieren conservar):

- `linkcheck.py` — extrae todos los `href="/..."` de las páginas y comprueba
  que responden 200. Resultado actual: **23 enlaces, 0 rotos**.
- `active2.py` — comprueba que cada página resalta **exactamente un** item del
  menú (ni cero ni dos).

---

## 5. Deploy a la Dell (resumen; detalle en `docs/DEPLOY.md`)

```bash
cd /home/gabo/study-platform
cargo build --release                                   # glibc (NO musl: no hay musl-gcc)

ssh homelab 'cp ~/study-platform/study-platform ~/study-platform/study-platform.prev'
scp target/release/study-platform homelab:~/study-platform/study-platform.new
ssh homelab 'cd ~/study-platform && mv study-platform.new study-platform && chmod +x study-platform'
ssh homelab 'pkill -x study-platform'                   # systemd lo levanta solo (~11 s)
sleep 12
curl -s http://100.92.172.48:8091/health
```

Reglas que evitan perder tiempo:

- **Nunca** `scp` al path final: el binario está en ejecución → `ETXTBSY`. Siempre `.new` + `mv`.
- El reinicio **no necesita sudo**: `pkill` + `Restart=always` + `RestartSec=10`.
  `sudo` sólo hace falta para tocar la unidad (`systemctl restart`, `daemon-reload`).
- `sudo` en la Dell **pide password interactiva**: `sudo -n` falla. Si hay que tocar la
  unidad, se hace a mano con el password.
- Verifica el hash: `sha256sum target/release/study-platform` debe coincidir con
  `ssh homelab 'sha256sum ~/study-platform/study-platform'`.

---

## 6. Diagnóstico: síntoma → causa → arreglo

| Síntoma | Causa real | Arreglo |
|---|---|---|
| `/health` dice `mikareno_mcp:false` pero `curl` al MCP da 200 | caché del probe (30 s) | esperar 30 s y repetir |
| `/health` dice `mikareno_mcp:false` y el MCP no responde | **el MCP está colgado** (ver §7) | `systemctl --user restart mikareno-mcp` |
| La página principal tarda 25 s y sale vacía | el cliente MCP agotó su deadline (25 s) esperando un scrape en frío | repetir: el cache queda listo para las siguientes |
| `ss -ltn` muestra `Recv-Q` grande en el 8080 | **el event loop del MCP está bloqueado** | reiniciar el MCP; la causa raíz ya está corregida en `server.py` |
| Todas las páginas dan `302` a `/login` | `AUTH_DISABLED` no está activo | `AUTH_DISABLED=true` en `.env` (y revisar el bind) |
| `scp` al binario falla con *Text file busy* | ETXTBSY | `scp` a `.new` y `mv` atómico |
| `systemctl --user status study-platform` → *unit not found* | es una unidad de **sistema** | `sudo systemctl status study-platform` |
| `cargo build` se queda esperando | hay otro cargo corriendo (sesión paralela) | `pgrep -a cargo`; esperar o matar el proceso viejo |

---

## 7. La trampa del MCP: nunca bloquear el event loop

**Síntoma**: el MCP de Mikareno acepta la conexión y se queda colgado. Todas las
requests cuelgan, el health check falla y la app "se muere" aunque el proceso
esté vivo. `ss -ltn` muestra el 8080 con `Recv-Q` grande (conexiones apiladas).

**Causa**: `mikareno-mcp/server.py` hacía el trabajo de Playwright **síncrono
dentro de corrutinas `async`**. FastMCP corre sobre un único event loop: mientras
duraba el scrape (30-100 s), **el servidor entero** —incluido el `initialize` del
health check— quedaba sin responder.

**Arreglo (ya aplicado)**: el trabajo bloqueante va al executor de un solo worker
(Playwright sync no es thread-safe, así que las raspas se serializan) pero se
*cede* el control al event loop:

```python
class _Runner:
    executor = ThreadPoolExecutor(max_workers=1)
    @classmethod
    def run(cls, fn):        # uso desde código síncrono
        return cls.executor.submit(fn).result()
    @classmethod
    async def arun(cls, fn):  # uso desde corrutinas: NO bloquea el loop
        loop = asyncio.get_running_loop()
        return await loop.run_in_executor(cls.executor, fn)
```

Además, `_refresh_lock` pasó de `threading.Lock` a `asyncio.Lock`: se mantiene
durante un `await`, y un `threading.Lock` adquirido desde el event loop lo
congelaba entero.

**Prueba de regresión** (`/tmp/opencode/mcp_load_test.py`): con un scrape de 98.8 s
en vuelo, 5 `initialize` seguidos responden en **≤13 ms**. Antes: timeout.

### Tiempos reales (medidos 2026-10-19)

| Operación | Tiempo |
|---|---|
| Scrape en frío del MCP (13 vistas + login) | **~58 s** |
| Deadline del cliente MCP en la app (`CALL_DEADLINE_SECS`) | 25 s |
| Page load con caché caliente | 10-15 ms |
| Page load con caché fría (tras reiniciar el MCP) | 25 s (datos vacíos) y luego 15 ms |

Consecuencia práctica: **tras reiniciar el MCP, la primera carga de cada página
tarda ~25 s y sale sin datos académicos**; a los ~60 s la caché queda lista y
todo vuelve a 15 ms. No es un fallo: es el scrape en frío. Si una página tarda
más de 25 s y sale vacía, mira primero si el MCP acaba de reiniciarse.

El scrape está acotado por `MCP_SCRAPE_DEADLINE_SECS` (240 s por defecto): sin
ese tope, 13 vistas × (goto 20 s + networkidle 10 s) podían ocupar el worker
del executor varios minutos y encolar todas las tool calls detrás.

> `~/mikareno-mcp` **no es un repo git**: este fix existe sólo en el disco de
> asus-tuf. Copia fechada del archivo parcheado:
> `~/mikareno-mcp/server.py.bak-20261019-arun-fix`. Vale la pena meterlo a git.

---

## 8. El menú lateral: una sola fuente de verdad

Los destinos del menú salen de **`NAV_LINKS`** (`src/handlers/stitch_v2.rs`):

```rust
pub const NAV_LINKS: &[(&str, &str)] = &[
    ("panel-principal", "/dashboard"),
    ("mi-agenda", "/dashboard/calendar"),
    ...
];
```

El `data-path` de cada `<a>` del shell decide su `href` al renderizar
(`stitch_v2::apply_nav` / `apply_nav_with`). Para **añadir una entrada al menú**:

1. Añadir el par a `NAV_LINKS` (en `src/handlers/stitch_v2.rs`).
2. Añadir el `<a data-path="...">` al shell estático
   (`static/stitch-v2/shell/aside.html`) **y** al shell de Rust
   (`STITCH_SHELL` en `src/academic/templates/mod.rs`).
3. Si la página debe quedar resaltada, pasar su ruta como `active_href` a
   `templates::page_ext(...)` (no `templates::page(...)`, que no resalta nada).
4. Actualizar `EXPECTED_NAV_PATHS` en `tests/shell_integrity.rs`.

Si el destino tiene varias etiquetas ("Planificación" y "Tareas" van al
planificador), la **primera** del mapa es la que se resalta.

Antes de este acuerdo, el shell de Rust tenía `href` escritos a mano: 11 de
ellos apuntaban a rutas inexistentes (`/dashboard/tareas`, `/dashboard/brain`,
`/office/documents`…) y cada clic del menú daba 404. Hoy el barrido automático
(`/tmp/opencode/linkcheck.py`) da **0 enlaces rotos** y cada página resalta
exactamente un item.

---

## 8. Política del cliente Mikareno (tailnet sí, internet no)

`MikarenoClient::from_config()` crea cliente sólo si el host es:

- loopback (`127.0.0.1`, `::1`, `localhost`);
- RFC 1918 (`10/8`, `172.16/12`, `192.168/16`);
- CGNAT `100.64/10` → **la tailnet de Tailscale** (p. ej. `100.115.62.97`).

Cualquier host público se ignora. El cliente es de sólo lectura por
construcción (sólo expone tools `get_*`), pero no debe apuntar a un MCP de
terceros por accidente.

Esto es lo que permite que la **app de la Dell** lea datos del MCP que vive en
asus-tuf. Antes la regla era "sólo loopback" y la app de producción se quedaba
**sin datos académicos** (cliente `None`), aunque el MCP contestara.

---

## 9. Decisiones abiertas / lo que falta

| # | Tema | Estado | Quién decide |
|---|---|---|---|
| 1 | `~/proyectos/styp/` (vacío, obsoleto) | pendiente de borrar | usuario |
| 2 | `AUTH_DISABLED` en producción | la prod tiene login activo; el spec dice "sin login" | usuario |
| 3 | Bind de producción | la unidad dice "Localhost-only" pero arranca con `--addr 0.0.0.0:8091`; para exponer sólo tailnet hace falta `sudo` (password interactiva) | usuario |
| 4 | `OPENROUTER_API_KEY` en la Dell | ausente → la IA va en modo demo en prod | usuario (fase posterior) |
| 5 | Mover el MCP de Mikareno a la Dell | opcional; ahora la prod lo alcanza por tailnet | usuario |
| 6 | Commit pendiente | el árbol tiene ~99 archivos modificados sin commitear; el remoto `dell` está en `edd3146` | — |
| 7 | `~/mikareno-mcp` no está bajo control de versiones | el fix del event loop vive sólo en disco (copia fechada: `server.py.bak-20261019-arun-fix`) | usuario |

---

## 10. Invariantes (no negociables)

- Nunca secretos en código, logs ni commits; `.env` está gitignored.
- Acceso sólo por Tailscale; no exponer puertos a internet.
- El MCP propio de la app exige `X-API-Key` y sólo escucha en localhost.
- No tocar firewall, Tailscale, GenOffice MCP ni la config de OpenCode.
- No instalar paquetes de sistema sin autorización.
- Confirmar antes de operaciones destructivas.
- Los mensajes de usuario van en **español**; identificadores y comentarios, en inglés.
