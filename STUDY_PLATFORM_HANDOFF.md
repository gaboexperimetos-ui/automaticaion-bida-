# Study Platform / NOVA — handoff de migración a Zig

**Fecha:** 2026-11-13 (UTC)<br>
**Estado:** migración base en Zig, verificada localmente; no desplegada en producción.
**Alcance confirmado:** eliminar las implementaciones Rust y C y servir el frontend existente Stitch v2/NOVA HTML/CSS/HTMX desde el backend Zig.

## Resultado actual

- Backend/CLI Zig 0.14.1 con migraciones SQLite, páginas/estáticos, autenticación Argon2id/sesiones, notas y búsqueda, calendario local, endpoints AI (modo simulado sin clave), nueve herramientas MCP protegidas, backups/restauración SQLite, health/config y respuesta SSE buffered.
- Las fuentes de implementación Rust y C fueron retiradas. Se conservaron los recursos de frontend, migraciones SQL, fixtures y documentación del producto.
- Se corrigió el ownership del hash Argon2: el string PHC se duplica al allocator antes de salir de la función. El test comprueba que el hash vive fuera del stack local y valida el password correcto e incorrecto.
- La API AI ahora envía historial reciente de usuario/asistente a OpenRouter y respeta el modelo persistido en el selector. Sin clave, la respuesta permanece en modo simulado sin red.
- No se han inventado registros académicos para compensar las integraciones que faltan.

## Validación ejecutada

```text
zig build                                        PASS
zig build test                                   PASS
python3 tests/http_smoke.py --binary zig-out/bin/study-platform  PASS
```

El smoke cubre páginas/estáticos, CRUD y búsqueda/historial de notas, exportación de calendario, modo AI simulado, modelo configurable e historial por conversación, MCP autenticado, backups/restauración, registro, login y cookies de sesión, rutas protegidas y MCP keyless deshabilitado. Los datos se crean en una carpeta temporal.

Hay un preview local Arena en `0.0.0.0:8090` arrancado con `AUTH_DISABLED=true`; su propósito es visualización/QA y no producción. No se ejecutaron comandos remotos ni se desplegó a la VPS.

## Bloqueantes y trabajo pendiente antes de producción

1. Implementar y probar clientes de integración HTTP/MCP de **Mikareno** y **GenOffice**, incluyendo timeout, autenticación, parsing de errores y configuración real. A día de hoy las URL se leen/documentan, pero no hay intercambio real.
2. Portar las rutas y comportamientos originales de páginas/datos académicos y de la suite de oficina. Las páginas existentes muestran placeholders/estado no disponible; no deben presentarse como integraciones completas.
3. Cambiar `/api/ai/chat/stream` de respuesta buffered a streaming token-a-token, o mantener documentado el límite.
4. Registrar tamaño/estadísticas reales de SQLite y memoria del proceso; el estado actual no debe etiquetar valores estáticos/desconocidos como uso real.
5. Añadir/regenerar documentación de rutas con paridad verificada; revisar cada ruta de la API original frente a `docs/API.md`.
6. Verificar workflow de CI/preview con un run de GitHub. El workflow construye ReleaseFast y produce un prerelease temporal; no es publicación a producción.
7. Hacer smoke de carga/seguridad en entorno aislado, confirmar permisos de DB/backups y revisar retención/restore con fallo inyectado.
8. Revisar backup list: el campo `created` actualmente se informó como `null`; validar contrato y exponer fecha real derivada de metadata fiable o cambiar el contrato a un campo nullable explícito documentado.
9. Completar métricas/monitorización y runbook operativo antes de planificar un despliegue.

## Operación y seguridad

- Requiere Zig 0.14.1 para build y SQLite 3 con FTS5 como librería de runtime.
- `SESSION_SECRET` aleatorio (≥32 caracteres), `AUTH_DISABLED=false`, `CUSTOM_MCP_API_KEY` fuerte si se expone MCP; `.env` y `data/` nunca se versionan.
- Las llamadas a OpenRouter sólo ocurren al configurar una clave; validar primero límites de coste y modelo con una cuenta de prueba no productiva.
- Usar DB/notes/backup temporales en QA. No restaurar un backup sobre datos reales durante las pruebas.
- La base SQLite y el vault de notas requieren backup regular y permisos restrictivos.

## Primeros comandos

```bash
zig fmt --check build.zig main.zig test_root.zig src/*.zig
zig build
zig build test --summary all
python3 tests/http_smoke.py --binary zig-out/bin/study-platform
zig build -Doptimize=ReleaseFast
```

Consulta [README.md](README.md), [docs/INDEX.md](docs/INDEX.md), [docs/API.md](docs/API.md), [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) y [docs/DEVELOPER_SETUP.md](docs/DEVELOPER_SETUP.md).
