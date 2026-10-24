# Puertos y servicios — mapa rápido

> Vigencia: **2026-10-23**. Punto de entrada para cualquiera que trabaje desde
> el repo: qué puerto toca para cada función, en qué máquina y cómo verificarlo.
> El bind HTTP lo decide siempre `--addr` (`HOST`/`PORT` del `.env` son solo
> informativos; no existe subcomando `serve`).

## Resumen por función

| Función | Puerto | Máquina | URL | Cómo se levanta / verifica |
|---|---|---|---|---|
| App en **producción** | **8091** | Dell (`ssh homelab`) | `http://100.92.172.48:8091` (tailnet) · `http://127.0.0.1:8091` en la propia Dell | unidad systemd `study-platform`; `systemctl is-active study-platform`; `curl http://127.0.0.1:8091/health` |
| App en **dev** (única instancia desde 2026-10-23) | **3000** | asus-tuf | `http://127.0.0.1:3000` | manual: `setsid nohup ./target/debug/study-platform --addr 127.0.0.1:3000 >> /tmp/opencode/sp3000.log 2>&1 < /dev/null &`; matar por **pid real** de `ss -tlnp \| grep 3000`, nunca `pkill -x` |
| Puerto **por defecto** del binario | **8090** | — | — | si se arranca sin `--addr` escucha `0.0.0.0:8090`. **En la Dell NO usar: está ocupado por otro servicio.** |
| MCP de **Mikareno** (datos escolares) | **8080** | asus-tuf | local `http://127.0.0.1:8080/mcp` · desde la Dell `http://100.115.62.97:8080/mcp` | systemd `--user` `mikareno-mcp.service`; `journalctl --user -u mikareno-mcp`; recovery `systemctl --user restart mikareno-mcp.service` |
| MCP de **GenOffice** | **8453** | asus-tuf y Dell | `http://127.0.0.1:8453` | proceso `genoffice` (MCP stdio/HTTP gestionado desde el editor) |
| Chromium de **debug CDP** (agentes/QA visual) | **9222** | asus-tuf | — | ad hoc: chromium con `--remote-debugging-port=9222` y perfil desechable; no permanece |

## Hitos del puerto 3000

- **15–22 oct 2026**: la sesión de rediseño sirvió aquí una **maqueta estática** (datos de ejemplo, sin backend).
- **2026-10-23 (temprano)**: el 3000 pasó a ser **app real de dev** (binario + BD local), igual que el 8095.
- **2026-10-23 (cierre)**: se **retira el 8095** (pid 86002); queda **una sola instancia dev: el 3000** (pid real 89479 al momento del cierre). El 8095 deja de estar vivo y en el mapa.
- Si otro agente necesita portabilidad, revisar `ss -tlnp` antes de asumir nada: estos puertos dev son **ad hoc** y cualquier sesión puede variarlos.

## Reglas que no cambian

1. **La app solo se accede por Tailscale.** Nada de exponer 8090/8091/3000/8095 a internet.
2. **8091 SIEMPRE en la Dell** (el 8090 de esa máquina pertenece a otro proceso).
3. **8080 SIEMPRE de mikareno-mcp** en asus-tuf (FastMCP/Python, 18 tools).
4. La BD canónica está en la Dell (`~/study-platform/data/app.db`); la `data/app.db` local de asus-tuf es solo dev.
5. Si en algún momento corren varias instancias dev, comparten la BD local en WAL; **mata instancia solo por su pid real** de `ss -tlnp` — nunca con `pkill -x` local, que puede llevarse la que no es. Postura de este repo (2026-10-23): **una sola instancia dev viva**: el 3000.

## Verificación rápida en una sola pasada

```sh
# en asus-tuf
ss -tlnp | grep -E ':(3000|8080|8090|8453) '
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:3000/health

# en la Dell (ssh homelab)
ss -tlnp | grep -E ':(8090|8091|8453) '
curl -s http://127.0.0.1:8091/health
```

Relacionado: runbook de prod `DEPLOY.md` · setup dev `DEVELOPER_SETUP.md` ·
notas del último sprint `SESSION_HANDOFF.md`.
