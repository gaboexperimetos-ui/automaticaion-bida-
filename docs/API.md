# API HTTP — Study Platform

Referencia de la superficie HTTP real del backend Rust (Axum). Todo lo que sigue
está leído del código (`src/handlers/mod.rs` y los `register_routes` de
`src/ai/`, `src/mcp_server/`, `src/academic/`, `src/office/`, `src/backup/`),
no de suposiciones. **Verificado contra el código el 2026-10-19.**

- **Base por defecto:** `http://<HOST>:<PORT>` — `PORT=8090`, `HOST=0.0.0.0` en `.env`
  (en `src/config.rs` los valores por defecto son `127.0.0.1:8090`).
- **Rutas HTML** devuelven `text/html`; las rutas `/api/*` devuelven JSON salvo
  indicación expresa.
- **Límite de peticiones:** 60 req/min por IP (ventana deslizante). `POST /login`
  tiene su propio cubo: 10 req/min por IP. Al exceder: `429` con `Retry-After: 60`.

## Autenticación

| Tipo | Mecanismo | Comportamiento |
|---|---|---|
| Sesión web | Cookie `session_id` (token opaco CSPRNG, 64 hex; en BD solo su hash `SHA256(SESSION_SECRET:token)`), `HttpOnly; SameSite=Strict; Path=/; Max-Age=86400` | Middleware `require_auth_state`. Sin sesión válida: `401` en rutas bajo `/api/`, `302 → /login` en el resto. `AUTH_DISABLED=true` desactiva la comprobación. |
| MCP | Cabecera `X-API-Key` | `check_api_key`: si no hay `CUSTOM_MCP_API_KEY` configurado acepta todo (modo dev); si lo hay, compara en tiempo constante. |

Rutas que viven en el router "público" pero llevan su propia capa `require_auth_state`
(por tanto **sí** requieren sesión): `GET /`, `GET /dashboard`, `GET /dashboard/ai`,
`GET /dashboard/notes`, `GET /dashboard/mcp`.

## Salud y configuración (públicas)

| Método | Ruta | Auth | Body / params | Respuesta típica |
|---|---|---|---|---|
| GET | `/health` | No | — | `200` JSON `{status:"healthy"\|"degraded", timestamp, version, components:{database:bool, openrouter:bool, mikareno_mcp:bool, genoffice_mcp:bool}}`; `503` si la BD no responde. El sondeo de los MCP externos va cacheado 30 s. |
| GET | `/api/config` | No | — | `200` JSON `{openrouter_configured, openrouter_model, mikareno_mcp_url, genoffice_mcp_url, version, is_localhost}`. Sin secretos. |
| GET | `/api/public-config` | No | — | Alias exacto de `/api/config` (misma respuesta). |
| GET | `/handoff` | No | — | `200` HTML (documento de traspaso). |
| GET | `/legacy` | No | — | `200` HTML (UI antigua `study-platform-ui`). |
| GET | `/styles.css` | No | — | `200` CSS. |
| GET | `/app.js` | No | — | `200` JavaScript. |

## Autenticación (login / logout / registro)

| Método | Ruta | Auth | Body | Respuesta típica |
|---|---|---|---|---|
| GET | `/login` | No | — | `200` HTML con el formulario de inicio de sesión. |
| POST | `/login` | No | `application/x-www-form-urlencoded`: `username`, `password` | `302 → /dashboard` + `Set-Cookie: session_id=…`. Errores: `401` "Usuario o contraseña incorrectos" (texto plano), `403` "Origen no autorizado" si el `Origin`/`Referer` no coincide con el `Host`, `429` por el cubo de brute-force, `500` "Error en el servidor". |
| POST | `/register` | **Token de invitación** | Form: `username`, `password`, `confirm`, `invite?` (o cabecera `Authorization: Bearer <token>`) | Requiere `REGISTRATION_TOKEN` en el entorno. Sin esa variable → `403` "El registro está deshabilitado: pide una cuenta al administrador"; token ausente o incorrecto → `403` "Invitación inválida: pide una cuenta al administrador" (la comparación es en tiempo constante). Con token válido: `200` "Cuenta creada. Ahora inicia sesión."; `400` si las contraseñas no coinciden o tienen < 8 caracteres; `409` "Ese usuario ya existe"; `500` error de BD. |
| POST | `/logout` | No (pero usa la cookie) | — | `200` HTML (formulario de login) + cookie de sesión borrada (`Max-Age=0`). Sin cookie: `302 → /login`. Borra la fila de `sessions`. |

## Dashboard y parciales HTMX

| Método | Ruta | Auth | Body / params | Respuesta típica |
|---|---|---|---|---|
| GET | `/` | Sí | — | `200` HTML (panel principal, shell Stitch v2). |
| GET | `/dashboard` | Sí | — | `200` HTML del panel principal. |
| GET | `/dashboard?partial=…` | Sí | `partial` = `stats` \| `profile` \| `subjects` \| `events` \| `tasks` | `200` **fragmento HTML** del parcial pedido (HTMX). Un `partial` desconocido o de más de 32 caracteres devuelve cuerpo vacío `200`. Sin el query param devuelve la página completa. |
| GET | `/dashboard/ai` | Sí | — | `200` HTML (asistente académico). |
| GET | `/dashboard/notes` | Sí | — | `200` HTML (segundo cerebro / notas). |
| GET | `/dashboard/mcp` | Sí | — | `200` HTML (centro MCP + estado de OpenRouter). |
| GET | `/dashboard/calendar.ics` | Sí | — | `200` `text/calendar; charset=utf-8` con `Content-Disposition: attachment; filename="calendario.ics"`. Genera `VCALENDAR` 2.0 (`PRODID:-//Study Platform//ES`, `CALSCALE:GREGORIAN`, `METHOD:PUBLISH`) con un `VEVENT` por evento de tipo `SCHOOL_DAY` o `EVENTO`: `DTSTAMP` (instante real, UTC), `UID` derivado de fecha+tipo+título, `DTSTART` y `SUMMARY`. Evento de todo el día → `DTSTART;VALUE=DATE:<AAAAMMDD>` + `DTEND;VALUE=DATE:` (exclusivo, día siguiente); evento con hora → `DTSTART:<AAAAMMDDTHHMMSS>` (hora local flotante, **sin** `VTIMEZONE` ni sufijo `Z`, porque Mikareno no expone zona). Un evento con fecha ilegible se omite (no se inventa `DTSTART`). Las líneas de más de 75 octetos se pliegan (RFC 5545 §3.1). Sin MCP disponible el `.ics` sale vacío pero válido. |

## Notas — interfaz HTML (`/app/notes`)

| Método | Ruta | Auth | Body | Respuesta típica |
|---|---|---|---|---|
| GET | `/app/notes` | Sí | — | `200` HTML con el listado de notas (`updated_at DESC`). |
| GET | `/app/notes/new` | Sí | — | `200` HTML con el formulario de creación. |
| POST | `/app/notes` | Sí | Form: `title` (obligatorio), `content`, `tags` | `303 → /app/notes/{slug}`. Errores JSON: `400 {"error":"Title is required"}` / `"Invalid title"` / `"Title too long"` (200) / `"Content too large"` (500 000) / `"Tags too long"` (500), `409 {"error":"Note with this title already exists"}`, `500 {"error":"Internal error"}`. El `slug` se deriva del título y el Markdown se escribe en disco. |
| GET | `/app/notes/{slug}` | Sí | — | `200` HTML (nota renderizada desde su `.md`; si el `.md` no está en disco, la nota se muestra con el aviso "Contenido no disponible en disco", nunca un 500); `404` HTML con la página de estilo de la app ("Nota no encontrada" + enlace "Volver a las notas") para slug inexistente o identificador peligroso (`..`, `/`, …), sin tocar el disco. |
| GET | `/app/notes/{slug}/edit` | Sí | — | `200` HTML (editor con el contenido Markdown; aviso "Contenido no disponible en disco" si el `.md` no se pudo leer); `404` HTML con la misma página de estilo que la vista. |
| PUT | `/app/notes/{slug}/update` | Sí | Form: `title?`, `content?`, `tags?` | `200 {"message":"Note updated"}`; `400` slug inválido o campos demasiado largos; `404`/`500` en JSON. |
| DELETE | `/app/notes/{slug}/delete` | Sí | — | `200 {"message":"Note deleted"}`; `404 {"error":"Note not found"}`; borra el `.md` y la fila en `notes`. |

## Notas — API JSON (`/api/notes`)

| Método | Ruta | Auth | Body | Respuesta típica |
|---|---|---|---|---|
| GET | `/api/notes` | Sí | — | `200 {"notes":[Note…], "count":N}`. `Note` = `{id, title, slug, filename, tags, word_count, summary, created_at, updated_at}`. Máximo 100 notas, `updated_at DESC`. |
| POST | `/api/notes` | Sí | JSON: `{title, content?, tags?}` (`content`/`tags` opcionales) | `201 {"slug":"…"}`. Errores: `400 {"error":"Title is required"}`, `"Invalid title"`, `"Title too long"`, `"Content too large"`, `"Tags too long"`; `500 {"error":"Internal error"}`. No comprueba slug duplicado (a diferencia del POST HTML, que sí devuelve `409`). |
| GET | `/api/notes/{slug}` | Sí | — | `200 {"note":{…}, "content":"<markdown>", "content_available":true}` (`content_available:false` y `content:""` si el `.md` no se pudo leer); `404 {"error":"not found"}`; `400 {"error":"Invalid slug"}`. |
| PUT | `/api/notes/{slug}` | Sí | JSON: `{title?, content?, tags?}` | `200 {"updated":true}`; `404 {"error":"not found"}`; `400` slug inválido o límites de longitud. |
| DELETE | `/api/notes/{slug}` | Sí | — | `200 {"deleted":true}`; `404 {"error":"not found"}`. |

## Búsqueda

| Método | Ruta | Auth | Body / params | Respuesta típica |
|---|---|---|---|---|
| GET | `/api/search` | Sí | Query `q` (opcional, ≤ 200 chars) | `200 {"query":"…", "count":N, "results":[{"slug","title","tags"}, …]}` — `LIKE` sobre `title`, `slug` y `tags` de la tabla `notes`, `updated_at DESC`, máximo 50 resultados. `q` vacío → `200 {"query":"","count":0,"results":[]}` sin tocar la BD. `q` > 200 chars → `400 {"error":"Query too long"}`; fallo de BD → `500 {"error":"Search unavailable"}`. |
| GET | `/dashboard/search` | Sí | Query `q` | `200` HTML con los resultados (fallback sin JS). Sin `q` devuelve la página con el texto de ayuda. |

## IA / OpenRouter (`/api/ai/*`)

Todos requieren sesión. Sin `OPENROUTER_API_KEY` el backend responde en **modo
simulado** (p. ej. el chat empieza por `[Modo simulado]`); no inventa contenido.

| Método | Ruta | Auth | Body | Respuesta típica |
|---|---|---|---|---|
| POST | `/api/ai/chat` | Sí | JSON: `{message (1..8000 chars), subject (≤200 chars, opcional → "general"), mode? ("hybrid"\|"notes"\|"general")}` | Sin `mode`, comportamiento previo y `200` **text/plain**. Con `Accept: application/json`: `200 {"content":"…","mode":"…","sources":[…]}`. `400` por mensaje/materia inválidos; `422` si `mode` no pertenece al enum; `5xx` en error de recuperación/proveedor. |
| POST | `/api/ai/chat/stream` | Sí | JSON: `{message (obligatorio, 1..8000), content? (≤20000), subject? (≤200, por defecto "general"), mode?}` | `200` `text/event-stream` + `cache-control: no-cache` + cabecera `x-chat-stream-id`. Ver el formato de frames más abajo. `400 {"error":"Invalid request: message is required."}` (o por longitud/modo); `5xx` `{"error":"…"}` si falla la recuperación u OpenRouter antes del stream. |
| POST | `/api/ai/summary` | Sí | JSON: `{content (1..20000)}` | `200 {"summary":"…"}`; `400 {"error":"Invalid request: content must be 1..=20000 characters."}`; `500 {"error":"Internal error"}`. |
| POST | `/api/ai/quiz` | Sí | JSON: `{content (1..20000), num_questions? (por defecto 5, clamp 1..10)}` | `200` JSON `Quiz` = `{title, questions:[{question, options:[…], correct_answer:índice}]}`; mismos `400`/`500` que summary. |
| POST | `/api/ai/study-sheet` | Sí | JSON: `{content (1..20000)}` | `200 {"study_sheet":"<markdown>"}`; mismos `400`/`500`. |
| GET | `/api/ai/cost` | Sí | — | `200 {"total_cost":f64, "budget_limit":f64, "requests_count":u64, "alert":bool}` — gasto leído de la tabla persistente `usage_metrics`; `alert` se activa al ≥ 90 % del presupuesto. |
| GET | `/api/ai/conversations` | Sí | — | `200 {"conversations":[{"subject","last_message","message_count","updated_at"}]}` — sesiones distintas por `subject` de `chat_history`, máx. 50, más reciente primero. |
| GET | `/api/ai/conversations/{subject}/messages` | Sí | — | `200 {"subject":"…", "messages":[{"role","content","created_at"}]}` (solo roles `user`/`assistant`, orden ascendente). `400` "Invalid subject" si el path contiene separadores o `..`; `500` "Internal error". |
| GET | `/ai` | Sí | — | `200` HTML (UI de chat a tres paneles). |

### Modos de recuperación y fuentes

`mode` ausente equivale a `general`, por compatibilidad con clientes anteriores.
El chat de `/dashboard/ai` selecciona `hybrid` por defecto:

| Modo | Notas | Web | Comportamiento |
|---|---|---|---|
| `hybrid` | Sí, primero | Sí, si el modelo decide que hace falta | Añade los fragmentos recuperados al prompt y habilita la herramienta `openrouter:web_search`. |
| `notes` | Sí | No | Exige responder sólo con el contexto recuperado y reconocer cuándo no es suficiente. |
| `general` | No | No | Conserva el par system/user anterior; no lee el vault. |

La recuperación usa el `KnowledgeService` existente (FTS5/BM25 con fallback),
no genera resultados simulados. Como máximo incorpora 5 notas, 2.400 caracteres
por nota y 12.000 en total. Un error de BD se propaga; una nota que no pueda
leerse se registra y se omite. La búsqueda web está limitada a 2 usos y 10
resultados totales por petición, y sólo se activa en `hybrid`.

Cada fuente tiene `{type,title,url,snippet}`, con `type` = `note` o `web`.
Las notas enlazan a `/app/notes/{slug}`; las citas web proceden de anotaciones
`url_citation` del proveedor y sólo se aceptan con URL HTTP(S). No hay fuentes
inventadas en modo demo.

### Formato de frames SSE (`POST /api/ai/chat/stream`)

Cada frame es `event: <nombre>\ndata: <json>\n\n` (un solo campo `data:` por frame,
separador en línea en blanco):

| Evento | `data` | Cuándo |
|---|---|---|
| `context` | `{"mode":"…","web":bool,"sources":[…]}` | Sólo si el body incluye `mode`; frame inicial con las notas efectivamente recuperadas. |
| `sources` | `{"sources":[…]}` | Citas web reales recibidas durante el stream, deduplicadas por URL. |
| `delta` | `{"content":"…"}` | Cada trozo de `choices[0].delta.content` del proveedor. |
| `reasoning` | `{"reasoning":"…"}` | Cada trozo de `choices[0].delta.reasoning`. |
| `done` | `{"usage":{"input":N,"output":N,"cost":0.0}}` | Frame terminal: el stream terminó. |
| `error` | `{"error":"…"}` | Fallo del proveedor a mitad del stream. |

Sin API key el stream envía **un solo** frame `done` con
`data: {"content":"[Modo simulado] streaming no disponible sin API key"}` (no inventa
contenido ni uso). Tiempos del relay: 30 s de inactividad entre trozos, 300 s de
duración total, 10 s de conexión.

**Persistencia:** el turno se escribe en la base **antes** del frame terminal
(por si el cliente se desconecta justo después del último `delta`): el mensaje
del usuario y la respuesta acumulada del asistente van a `chat_history` (la
misma tabla y columnas que usa el chat con búfer, así que también aparecen en
`/dashboard/ai` y en `GET /api/ai/conversations`) y el consumo real de tokens a
`usage_metrics` con coste `0.0` y marca `estimated` (el `usage` del streaming no
trae coste fiable). La persistencia es *best effort*: un `INSERT` que falla se
registra con `warn` y no rompe el stream ya emitido.

## Administración

### API legacy (`/api/admin/*`)

Todas comparten el sistema unificado de backups (`src/backup`): escriben y leen
siempre del directorio `data/backups` (ficheros `app-YYYYmmdd-HHMMSS.db` + sidecar
`.sha256` con el resultado del `PRAGMA integrity_check`).

| Método | Ruta | Auth | Body | Respuesta típica |
|---|---|---|---|---|
| GET | `/api/admin/metrics` | Sí | — | `200 {version, uptime_secs, notes_count, chats_count, mcp_connected, openrouter_configured, spend_usd, budget_usd}`. |
| GET | `/api/admin/backup` | Sí | — | `200 {"success":true, "name":"app-20261019-185000.db", "bytes":N, "path":"…", "size_bytes":N}` (crea ahora una copia de la BD en `data/backups`; `path`/`size_bytes` son claves legacy del panel); `500 {"success":false,"error":"…"}`. |
| POST | `/api/admin/backup` | Sí | — | `200 {"success":true, "name":"…", "bytes":N}` — crea la copia (mismo sistema unificado; es el gemelo en `POST` del `GET` legacy); `500 {"success":false,"error":"…"}`. |
| POST | `/api/admin/backup/restore` | Sí | JSON: `{"name":"app-20261019-185000.db"}` | `200 {"success":true,"name":"…","bytes":N}` — restaura el **contenido** del backup sobre la base viva (no sustituye el fichero). Antes de tocar nada valida el nombre (`app-*.db`, sin `..` ni separadores) y el `integrity_check` del fichero. `400 {"error":"nombre de backup no válido"}` (nombre rechazado), `404 {"error":"backup no encontrado"}` (fichero inexistente), `422 {"error":"el backup no supera el control de integridad"}`, `500 {"error":"…"}` (p. ej. "server en uso: reintenta tras detener la app"). Nunca simula un restore que no ha ocurrido. |
| GET | `/api/admin/backup/download/{name}` | Sí | — | `200` el fichero SQLite con `content-type: application/vnd.sqlite3` y `Content-Disposition: attachment; filename="{name}"`. El nombre se valida igual que en el restore y se busca **siempre** dentro de `data/backups` (nunca como ruta): nombre no admitido o inexistente → `404 {"error":"backup no encontrado"}`. |
| GET | `/api/admin/backup-status` | Sí | — | `200 {"last_backup":{"path","size_bytes","created_at"} \| null, "count":N}` sobre `data/backups` (ficheros `*.db`, orden cronológico inverso). |

### Panel F9 (`/admin/*`)

| Método | Ruta | Auth | Body | Respuesta típica |
|---|---|---|---|---|
| GET | `/admin` | Sí | — | `200` HTML: panel de administración (uptime, notas, chats, gasto, presupuesto, estado MCP/OpenRouter, backups y recursos del sistema). |
| GET | `/admin/metrics` | Sí | — | `200 {uptime_secs, ram_mb_used, ram_mb_total, disk_mb_free, db_bytes}` (RAM de `/proc/meminfo`, disco de `df -kP`, tamaño del SQLite). Cada métrica degrada a `0` si su fuente falla. |
| GET | `/admin/openrouter/budget` | Sí | — | `200 {available:bool, spend_usd, budget_usd, pct, alert}` — `available:false` si la tabla `usage_metrics` no existe (gasto 0, sin crearla). `alert` a ≥ 90 % del tope. |
| GET | `/admin/backup/list` | Sí | — | `200 {dir:"data/backups", count:N, items:[{nombre, bytes, created}]}` — `created` en RFC-3339 o `null`; directorio inexistente → lista vacía. |

## Configuración y material

| Método | Ruta | Auth | Body | Respuesta típica |
|---|---|---|---|---|
| GET | `/settings` | Sí | — | `200` HTML: valores reales de configuración (modelo de IA, URL del MCP, modo de auth, versión, presupuesto, estado del MCP). **Read-only**; la clave de OpenRouter nunca se muestra, solo su estado. La tarjeta "IA / OpenRouter" trae además un formulario de edición (modelo, máx. tokens, presupuesto) cuyo botón es **"Generar .env"** (no "Guardar"). |
| POST | `/api/settings/ai` | Sí | Form: `model`, `max_tokens`, `max_cost` (acepta coma decimal) | **No persiste nada** (la configuración se lee del entorno al arrancar y `AppState.config` es inmutable en runtime). Valores válidos → `501 Not Implemented` con la **página de configuración** (o, con `HX-Request`, sólo el fragmento del aviso) y dentro un bloque `.env` listo para copiar (`OPENROUTER_MODEL`, `OPENROUTER_MAX_TOKENS`, `OPENROUTER_MAX_COST_USD`; **la clave nunca se imprime**). Valores inválidos → `400` con la lista de errores en español. El cuerpo es **siempre HTML**, nunca JSON. Límites: modelo ≤ 120 caracteres y sólo `[A-Za-z0-9-_./:#@]`; `max_tokens` 1..=200 000; `max_cost` 0..=100 USD. |
| GET | `/dashboard/material` | Sí | — | `200` HTML (generador de material). |
| POST | `/dashboard/material` | Sí | JSON: `{materia, tema, tipo}` (los tres obligatorios) | `200 {"ok":true, "tipo":"…", "materia":"…", "tema":"…", "material":<Quiz>}` — genera un cuestionario de opción múltiple. Sin `OPENROUTER_API_KEY`: `200 {"ok":false,"error":"Modo demo: configura OPENROUTER_API_KEY"}` (no genera nada). Campos vacíos → `400 {"ok":false,"error":"Materia, tema y tipo son obligatorios"}`; error del proveedor → su status con `{"ok":false,"error":"…"}`. |

## MCP (Model Context Protocol)

Rutas del servidor MCP propio. **No** usan la sesión web: se autentican con la
cabecera `X-API-Key` (si no hay clave configurada, se acepta cualquier petición).

El transporte es: **JSON-RPC sobre JSON** en `POST /mcp`, y **SSE** en
`POST /mcp/sse`. Es decir, `POST /mcp` **no** devuelve `text/event-stream`: la
respuesta es un único objeto JSON-RPC.

| Método | Ruta | Auth | Body | Respuesta típica |
|---|---|---|---|---|
| POST | `/mcp` | `X-API-Key` | JSON-RPC 2.0: `{jsonrpc:"2.0", method, params?, id?}` | `200` **JSON** (no SSE) con la respuesta JSON-RPC 2.0. Métodos: `initialize` → `{protocolVersion:"2025-03-26", capabilities:{tools:{},prompts:{}}, serverInfo:{name:"study-platform-mcp", version}}`; `tools/list` → `{tools:[…]}`; `tools/call` → `{result:{content:[{type:"text", text:"…"}]}}`. Errores: `401` → cuerpo JSON-RPC con `error.code:-32001 "Unauthorized"` (se devuelve con status HTTP 200); `jsonrpc != "2.0"` → `400` + `-32600`; método desconocido → `-32601`; parámetros inválidos → `-32602`; error interno → `-32603`; parseo inválido → `-32700`. |
| POST | `/mcp/sse` | `X-API-Key` | JSON-RPC 2.0 opcional en el cuerpo | `200` `text/event-stream` con un frame inicial `event: message\ndata: {"jsonrpc":"2.0","method":"notifications/initialized","params":{}}\n\n` y, si el cuerpo trae una petición JSON-RPC válida, un segundo frame `event: message` con la respuesta. Sin clave válida → `401` con cuerpo `Unauthorized` (texto plano). |

### Tools expuestas (`tools/list`)

| Tool | Parámetros | Resultado |
|---|---|---|
| `search_notes` | `query` (obligatorio), `limit` (opcional, por defecto 10) | Texto JSON con los resultados de búsqueda. |
| `get_note` | `slug` (obligatorio) | Texto: metadatos de la nota + su contenido Markdown. |
| `create_note` | `title` y `content` (obligatorios), `tags` (array opcional) | Texto JSON con la nota creada. |
| `get_academic_context` | — | Texto JSON `{connected, calendario, horario, perfil}`; si el MCP no responde, `{connected:false, error:"MCP not reachable"}`. |
| `get_subject_notes` | `subject` (obligatorio) | Texto JSON con las notas filtradas (límite 20). |
| `get_study_sheets` | `subject` (opcional) | Texto JSON con las hojas de estudio encontradas (límite 20). |
| `app_list_backups` | — | Texto JSON con los backups de `data/backups` (`BackupInfo` = `{name, path, bytes, created}`), del más reciente al más antiguo. |
| `app_backup_now` | — | Crea una copia ahora. `result` = `{content:[{type:"text",text:"<json>"}], success:true, name:"app-YYYYmmdd-HHMMSS.db", bytes:N, path:"…", size_bytes:N}` — el `text` es el mismo JSON y **las claves se copian también al nivel superior** de `result`, así que un cliente puede leer `result.name` sin parsear el texto. Si falla: error JSON-RPC `-32000` con el mensaje. |
| `app_ai_cost` | — | `result` = `{content:[{type:"text",text:"<json>"}], total_cost:f64, budget_limit:f64, requests_count:u64, alert:bool}` — mismo shape que `GET /api/ai/cost`, leído de la tabla `usage_metrics`. |

Las tres tools de operación (`app_*`) no aceptan argumentos: se llaman sin
`arguments`.

## Académico (anidado bajo `/dashboard`)

Todas requieren sesión (`route_layer` de `require_auth_state`).

| Método | Ruta | Auth | Body / params | Respuesta típica |
|---|---|---|---|---|
| GET | `/dashboard/calendar` | Sí | — | `200` HTML (calendario mensual). |
| GET | `/dashboard/calendar/daily` | Sí | — | `200` HTML (calendario diario). |
| GET | `/dashboard/horario` | Sí | — | `200` HTML (horario semanal). |
| GET | `/dashboard/planner` | Sí | — | `200` HTML (planificador semanal). |
| GET | `/dashboard/profile` | Sí | — | `200` HTML (perfil del estudiante). |
| GET | `/dashboard/subjects` | Sí | — | `200` HTML (asignaturas). |
| GET | `/dashboard/subjects/{code}` | Sí | — | `200` HTML (detalle de asignatura). |
| GET | `/dashboard/examenes` | Sí | — | `200` HTML (exámenes). |
| GET | `/dashboard/vida-escolar` | Sí | — | `200` HTML (vida escolar). |

## Office suite

Todas requieren sesión. `/office/report` usa `require_auth_json`: sin sesión
válida responde **`401` + JSON**, nunca un 302 a la página de login (es una
descarga, no una página).

| Método | Ruta | Auth | Body / params | Respuesta típica |
|---|---|---|---|---|
| GET | `/office` | Sí | — | `200` HTML (portada de documentos). |
| GET | `/office/report` | Sí (401 + JSON si no hay sesión) | — | `200` el informe del estudiante en `.docx` (`content-type: application/vnd.openxmlformats-officedocument.wordprocessingml.document`, `Content-Disposition: attachment; filename="informe-estudiante.docx"`), generado por el MCP de GenOffice a partir de datos reales (apuntes de la BD + perfil y notas del MCP de Mikareno). Si GenOffice no está disponible → `503` con JSON `{"error":"GenOffice no disponible","motivo":"<detalle>"}` (URL vacía, MCP caído, fallo de la herramienta o timeout). Timeout: 20 s si el sondeo previo al MCP falló; si el sondeo va bien, manda el timeout del cliente MCP (90 s). **Nunca** devuelve un `.docx` de reemplazo ni HTML de error. |
| GET, POST | `/office/docs` | Sí | POST: cuerpo del documento | `200` HTML (listado) / creación del documento. |
| GET | `/office/docs/new` | Sí | — | `200` HTML (editor nuevo). |
| GET, PUT, DELETE | `/office/docs/{slug}` | Sí | PUT: contenido del documento | `200` HTML (editor) / actualización / borrado. |
| GET | `/office/docs/{slug}/preview` | Sí | — | `200` HTML (vista previa). |
| GET | `/office/docs/{slug}/export` | Sí | — | `200` archivo exportado. |
| GET, POST | `/office/sheets` | Sí | POST: datos de la hoja | `200` HTML (listado) / creación. |
| GET | `/office/sheets/new` | Sí | — | `200` HTML (hoja nueva). |
| GET | `/office/sheets/{slug}` | Sí | — | `200` HTML (editor de hoja). |
| POST | `/office/sheets/{slug}/import` | Sí | archivo a importar | Importación de la hoja. |
| GET | `/office/sheets/{slug}/export` | Sí | — | `200` archivo exportado. |
| POST | `/office/sheets/{slug}/delete` | Sí | — | Borrado de la hoja. |
| GET, POST | `/office/presentations` | Sí | POST: datos de la presentación | `200` HTML (listado) / creación. |
| GET | `/office/presentations/new` | Sí | — | `200` HTML (presentación nueva). |
| GET, DELETE | `/office/presentations/{slug}` | Sí | — | `200` HTML (editor) / borrado. |
| GET | `/office/presentations/{slug}/present` | Sí | — | `200` HTML (modo presentación). |
| POST | `/office/presentations/{slug}/slides` | Sí | datos de la diapositiva | Añade una diapositiva. |
| PUT, DELETE | `/office/presentations/{slug}/slides/{index}` | Sí | PUT: contenido de la diapositiva | Actualiza / borra la diapositiva. |
| POST | `/office/presentations/{slug}/slides/reorder` | Sí | nuevo orden | Reordena las diapositivas. |
| GET | `/office/library` | Sí | — | `200` HTML (biblioteca de archivos). |
| GET | `/office/library/search` | Sí | Query `q` (≤ 200 chars) | `200` HTML con los resultados; `400` "query too long". |
| POST | `/office/library/import/{type}` | Sí | archivo a importar | Importa un archivo a la biblioteca. |
| GET | `/office/library/{type}/{name}/download` | Sí | — | `200` archivo descargado. |
| POST | `/office/library/{type}/{name}/delete` | Sí | — | Borra el archivo de la biblioteca. |

## Notas de implementación

- Las respuestas de error JSON del backend usan la forma `{"error":"<mensaje>"}`
  (en `/api/admin/*` el backup añade `success:false`). Los endpoints de descarga
  (`/office/report`) usan `{"error":"…","motivo":"…"}`.
- `src/handlers/api.rs` define además `debug_info()`, pero **no** está registrado en
  ningún router: no hay ruta `GET /api/debug`.
- `POST /api/settings/ai` **sí** está registrada (`handlers::mod::router`, grupo
  protegido por sesión) y **no persiste nada**: responde `501` con la página de
  configuración y el bloque `.env` para copiar. Aplicar los cambios es manual
  (editar `.env` y reiniciar el servicio).
- `src/handlers/auth.rs` define `set_password()` (actualiza el `password_hash` del
  usuario `admin` con Argon2), pero **no está registrado en ningún router** y no
  existe ninguna página de cambio de contraseña: hoy **no hay ruta HTTP para
  cambiar la contraseña**. El CLI tampoco expone `set-password` (sólo `--migrate`
  y `--create-admin`).
- `api::config` y `api::public_config` devuelven exactamente el mismo cuerpo.
- En `/api/ai/chat` el cuerpo de éxito es `text/plain`, no JSON.
- Las rutas `/dashboard*` con `partial=` que no coincidence con `stats|profile|subjects|events|tasks`
  devuelven `200` con cuerpo vacío.
