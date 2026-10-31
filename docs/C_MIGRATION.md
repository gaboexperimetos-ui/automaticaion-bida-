# Migración del backend a C

**Estado:** en curso · **Vigencia:** 2026-10-31 · **Fase actual:** 1 de 7.

## Decisión y alcance

El usuario eligió migrar el backend a C conservando API, funciones y pruebas. El
frontend seguirá siendo HTML/CSS/JavaScript: “100 % C” se aplica al código del
servidor, no al formato de documentos ni al comportamiento del navegador.

Rust/Axum permanece como implementación de referencia hasta que se cumplan todos
los criterios de salida. No se sustituye producción, no se cambia el systemd y no
se elimina código funcional durante las fases intermedias.

## Punto de partida medido

- Aproximadamente 48.000 líneas Rust en 76 archivos `src/*.rs`.
- 29 archivos de pruebas Rust y más de un centenar de registros `route()`
  literales, incluidos duplicados en pruebas y rutas anidadas.
- Áreas: autenticación/sesiones, notas y archivos Markdown, FTS5, IA/OpenRouter,
  streaming SSE, MCP cliente y servidor, datos académicos, Office, backups,
  administración, assets y shell HTML.

Estas cifras describen magnitud, no porcentaje de migración. El porcentaje se
medirá por contratos HTTP y comportamiento, no por líneas convertidas.

## Fase 1 — Núcleo C y primeros contratos (implementada)

Directorio: [`c_backend/`](../c_backend/).

| Componente | Estado |
|---|---|
| Config `.env` y entorno | Implementada para las 17 variables existentes |
| SQLite por C ABI, health `SELECT 1` | Implementado |
| HTTP, workers, límites, timeouts, IP real | Implementado |
| `/health`, config pública y alias | Implementados |
| Ocho rutas de assets | Implementadas |
| Sondeos MCP HTTP con caché | Implementados |
| ASan, UBSan, `-fanalyzer`, pruebas HTTP | Implementados y verdes |
| Migraciones SQL y `--migrate` | Pendiente; falla cerrado |
| Alta de administrador | Pendiente; falla cerrado |
| Todo lo protegido y demás dominios | Pendiente |

Validación local de esta fase tras la pasada de calidad: 43 comprobaciones
unitarias + 85 comprobaciones HTTP, ASan/UBSan y GCC `-fanalyzer`. Incluye fuzz
smoke determinista, headers/binarios malformados, límites, concurrencia, `.env` y
SQLite en memoria. La CI de C debe confirmar el mismo resultado en cada push
antes de considerar válido un cambio posterior.

## Fases restantes

### Fase 2 — Persistencia compatible y seguridad base

- Runner de migraciones compatible con el historial `_sqlx_migrations`, sin
  repetir ni reinterpretar migraciones en una BD existente.
- Capa prepared statements, transacciones, WAL y backups antes de escrituras.
- Argon2, contraseñas, usuarios, sesiones opacas, cookies, registro y logout.
- CSRF/origen, auth middleware, rate limiting por IP real y límites SSE.
- Pruebas diferenciales de login, autorización, sesiones y ataques actuales.

### Fase 3 — Notas, Markdown y búsqueda

- CRUD HTML/JSON, slugs, etiquetas, archivos, versiones y restauración.
- Frontmatter, wikilinks, backlinks, TOC, exportación e importación Obsidian.
- FTS5 y fallback `LIKE`, preservando consultas y orden de resultados.
- Pruebas de paths peligrosos, límites, Unicode y concurrencia.

### Fase 4 — Académico, MCP y shell completo

- Cliente Mikareno/GenOffice, cachés, deadlines y degradación honesta.
- Calendario, agenda, planner, asignaturas, exámenes, vida escolar y `.ics`.
- Render del shell y slots sin datos ficticios ni placeholders pendientes.
- MCP servidor propio, JSON-RPC y `X-API-Key`.

### Fase 5 — IA y streaming

- Cliente HTTPS/TLS para OpenRouter; no usar texto plano ni desactivar verificación.
- Chat, historial, resúmenes, quizzes, hojas de estudio y catálogo de modelos.
- RAG `hybrid`/`notes`/`general`: recuperación real de notas, límites de
  contexto, web sólo en híbrido, citas verificables y compatibilidad sin `mode`.
- Budget gate transaccional y métricas de uso con la misma semántica.
- SSE `context`/`sources`/`delta`/`reasoning`/`done`/`error`, cancelación y
  límite de streams.
- Pruebas sin consumir modelos de pago ni exponer credenciales.

### Fase 6 — Office, administración y operación

- Documentos, hojas, presentaciones, biblioteca e informes GenOffice.
- Backups consistentes, sidecars, restore, descarga, retención y tarea diaria.
- Métricas, ajustes, catálogo y todos los contratos de administración.
- Empaquetado de assets, CLI completa y documentación de operación C.

### Fase 7 — Paridad, retirada y cutover autorizado

- Suite contractual externa ejecutada contra Rust y C sobre datos equivalentes.
- Fuzzing del parser HTTP/JSON/Markdown y sanitizers en toda la suite.
- Comparativa de memoria, latencia y concurrencia; C no se considera mejor por
  definición: debe medirse.
- Revisión de seguridad y migración de datos en copia, con rollback probado.
- Solo después: retirar Rust y preparar un deploy que requerirá autorización
  explícita independiente.

## Puerta obligatoria para declarar 100 %

No se declarará migrado ni se borrará Rust hasta que se cumpla **todo**:

1. Todas las combinaciones método+ruta, incluidos parámetros y aliases.
2. Paridad de status, body, headers, cookies, redirects, archivos y frames SSE.
3. Paridad del esquema, migraciones, transacciones y datos persistentes.
4. Todas las funciones visibles, estados de error y degradaciones externas.
5. Suite de seguridad, regresión e integración verde contra el binario C.
6. Cero placeholders, respuestas simuladas o endpoints `501` en el inventario.
7. Sanitizers y análisis estático limpios; secretos fuera de logs y respuestas.
8. Benchmark y consumo de RAM documentados, sin regresión injustificada.
9. CI de C verde en el último commit y working tree limpio.
10. Autorización explícita para cualquier despliegue o cambio en producción.

## Reglas durante la migración

- No proxies ni subprocesos que llamen a Rust para aparentar cobertura C.
- No reescrituras destructivas de la BD real ni pruebas contra producción.
- No instalar paquetes del sistema sin autorización.
- No dependencias opacas ni credenciales nuevas en el repositorio.
- Commits pequeños por dominio, push solo a la rama de trabajo y sin deploy.
- `docs/API.md` y las pruebas Rust son la referencia inicial; cualquier
  discrepancia debe resolverse por comportamiento existente, no por conveniencia.
