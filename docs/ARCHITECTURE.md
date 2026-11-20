# Arquitectura y estado de paridad

Actualizado el 2026-11-13. El checkout contiene un backend Zig 0.14.1 y el frontend server-rendered Stitch v2/NOVA conservado. Las implementaciones Rust/C originales fueron reemplazadas; los archivos HTML, CSS, HTMX/JavaScript y migraciones SQL son recursos existentes que se mantienen, no otro backend.

## Componentes

```text
src/main.zig       CLI, listener IPv4 y conexiones HTTP
src/http.zig       parsing de request y respuesta
src/app.zig        router, auth/sesiones, admin, backups
src/notes.zig      notas Markdown, FTS5, versiones, calendario/ICS
src/ai.zig         chat, generación, OpenRouter y costes
src/mcp.zig        JSON-RPC MCP propio, nueve herramientas
src/sqlite.zig     bindings SQLite por ABI, migraciones embebidas, backup API
src/security.zig   Argon2id, sesiones, escaping
src/config.zig     configuración de entorno
src/templates.zig  HTML renderizado
src/assets.zig     estáticos embebidos
static/            CSS, HTML, HTMX/JS e iconos del frontend Stitch/NOVA
migrations/        ocho migraciones SQL de la aplicación
```

La aplicación enlaza libc para sockets y la biblioteca compartida SQLite instalada. No mantiene fuentes de backend en C; la ABI SQLite es una dependencia nativa del sistema. El proceso mantiene una conexión `FULLMUTEX` y no un pool de conexiones, aunque se conserva `DB_MAX_CONNECTIONS` como variable legacy reservada.

## Estado comprobado

`zig build`, `zig build test` y `tests/http_smoke.py` han pasado en el entorno local con Zig 0.14.1. El smoke HTTP comprueba rutas representativas públicas, autenticación, notas, calendarización, AI simulado, MCP y backups usando datos temporales.

Comportamientos presentes en el port:

- Migración del esquema SQL original y servicio de frontend/estáticos.
- Usuarios, Argon2id, sesiones persistentes, login/registro de acuerdo con la configuración, cookies HTTP y protección de rutas.
- Notas Markdown, búsqueda FTS5/fallback, historial limitado, exportación y restauración de versiones.
- Anotaciones locales de calendario e ICS validado; no se inventa información escolar.
- Configuración y selección de modelo persistida; el historial reciente entra en la petición OpenRouter; sin clave se usa respuesta simulada sin llamada externa.
- MCP propio con nueve herramientas y `CUSTOM_MCP_API_KEY` obligatorio.
- Backup y restore SQLite mediante online backup API, `integrity_check`, control de nombre y retención de cinco copias.

## Paridad pendiente / límites explícitos

Este backend **no es todavía una sustitución funcional 100% equivalente**. La decisión de producto fue hacer un reemplazo completo de tecnología, pero no justifica afirmar que cada integración esté portada. Estado concreto:

1. **Mikareno y GenOffice:** existen variables `MIKARENO_MCP_URL` y `GENOFFICE_MCP_URL` y contenido de configuración, pero no clientes MCP/HTTP Zig que consulten dichos servicios. Las herramientas académicas MCP locales devuelven vacío/no disponible si no hay datos propios; no reflejan los sistemas externos.
2. **Áreas académicas y suite de oficina:** el shell Stitch/NOVA y enlaces se conservan; determinadas vistas son placeholders/no disponibles, sin operaciones conectadas a datos reales. Revisar rutas y contratos originales antes de producción.
3. **SSE de AI:** `/api/ai/chat/stream` conserva el contrato de evento, pero la generación y envío se hacen en buffer; no hay streaming incremental de tokens. No describirlo como streaming vivo.
4. **Métricas de máquina:** no se recopilan métricas reales de RAM/disco del host. Cualquier valor de configuración o placeholder no debe etiquetarse como medición.
5. **Backups:** el listado se implementó, pero se detectó `created: null` en una ejecución. Confirmar contrato, registrar fecha real de forma fiable (p. ej. metadata SQLite/archivo) o documentar nullable antes de dar la función por cerrada.
6. **CI/preview:** workflows Zig están definidos; se requiere verificar un run en GitHub. El prerelease temporal de preview no es una publicación ni despliegue de producción.
7. **Cobertura total:** el smoke es representativo, no una prueba exhaustiva de todas las rutas, la matriz de permisos, concurrencia, límites/rate limiting o fallos de disco/red.

## Seguridad y límites operativos

- `SESSION_SECRET` debe ser aleatorio y ≥32 caracteres.
- `AUTH_DISABLED=true` salta la autenticación de las rutas protegidas. Sólo se usa en desarrollo local/preview privado.
- MCP se deshabilita con 401 si falta la clave.
- Las claves OpenRouter no deben entrar a HTML o logs; se usa el proveedor sólo si la clave está presente.
- Backup restore valida el identificador de archivo, corre `integrity_check` y usa SQLite Online Backup; aun así debe probarse con un backup corrupto y con interrupción en QA.
- Un listener en `0.0.0.0` requiere firewall, TLS/reverse proxy y una política de secretos antes de exponerse fuera de un entorno controlado.

## Siguiente secuencia recomendada

1. Comparar el inventario de rutas original con `docs/API.md`, implementar las rutas y estado académico/oficina que falten.
2. Implementar clientes Mikareno/GenOffice con timeout, autenticación, manejo de errores y tests simulados sin servicios reales.
3. Implementar streaming real o versionar/documentar claramente el endpoint buffered.
4. Resolver fecha de backups y agregar tests de seguridad/restore fallido; añadir métricas de host reales sólo con un contrato definido.
5. Hacer que CI ejecute formato, Debug/ReleaseFast, unit e HTTP smoke; revisar el resultado remoto y artefacto.
6. Ejecutar pruebas de concurrencia, permisos, rate limit, entrada malformada y carga en entorno aislado.
7. Preparar el runbook y una autorización explícita antes de cualquier despliegue.
