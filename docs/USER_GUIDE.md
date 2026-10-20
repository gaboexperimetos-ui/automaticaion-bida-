# Guía de usuario — Study Platform

Guía de uso de la plataforma de productividad escolar, escrita para Gabo.
Todo lo que aparece aquí está verificado contra el código en `src/`
(**2026-10-19**).

---

## 1. ¿Qué es?

**Study Platform** es una plataforma personal de productividad escolar de un solo
usuario: reúne el horario, las notas, la asistencia, las tareas y los mensajes
de la plataforma escolar Mikareno, las suma a un segundo cerebro en Markdown
compatible con Obsidian, y agrega un asistente académico con IA y una suite de
ofimática web.

- **Backend:** Rust + Axum (`src/main.rs`, `src/handlers/`), servidor-renderizado.
- **Datos:** SQLite en un solo archivo (`data/app.db`) + notas `.md` en disco (`notes/`).
- **Frontend:** HTML generado en el servidor con HTMX + Tailwind, sobre el kit
  visual "Synthetic Cognition" (shell Stitch v2: rail lateral + barra superior).
- **Integraciones:** MCP de Mikareno (datos escolares), MCP de GenOffice (ofimática),
  API de OpenRouter (IA).
- **Acceso:** únicamente desde tu red privada (tailnet de Tailscale). El servicio
  escucha en `127.0.0.1:8091` en el servidor de despliegue y **no** se expone a
  internet. La sesión dura 24 h y hay límite de 60 peticiones/min por IP
  (10/min para `/login`).

### Sesión y modo sin login

- Con `AUTH_DISABLED=false` (por defecto) entras en `/login` con usuario y
  contraseña (Argon2) y la cookie de sesión dura 24 h.
- Con `AUTH_DISABLED=true` la app queda en modo "sin login" (single-user local):
  todas las páginas se sirven directamente. Es el modo pensado para el uso
  personal en la red privada.

---

## 2. El shell (la interfaz común)

Todas las pantallas del shell v2 comparten:

- **Rail lateral izquierdo:** 21 entradas que llevan a rutas reales
  (panel principal, agenda, planificación, asignaturas, exámenes, horarios,
  asistente, generador de material, segundo cerebro, documentos/hojas/
  presentaciones/biblioteca, centro MCP, administración, configuración).
- **Barra superior:** breadcrumb *Estudios → Workspace*, buscador global
  (visual, ver §6), botón **Crear**, pastilla de estado (MCP, IA, porcentaje de
  bóveda), campana de notificaciones (sin enlace) y el avatar/nombre del
  estudiante (clic = cerrar sesión).
- **Estados honestos:** si el MCP de Mikareno no responde, las páginas muestran
  estados vacíos ("Sin clases", "Sin datos", "Sin exámenes programados").
  **Nunca se inventan datos** en ninguna pantalla.

---

## 3. Rutas principales

| Ruta | Qué es | Datos |
|---|---|---|
| `/` y `/dashboard` | Panel principal ("visión") | Mikareno + notas locales |
| `/dashboard/horario` | Grilla de horario | Mikareno (`get_horario`) |
| `/dashboard/calendar` | Calendario mensual | Mikareno (`get_calendario`) |
| `/dashboard/calendar/daily` | Agenda del día | Mikareno |
| `/dashboard/calendar.ics` | Export `.ics` (archivo) | Mikareno |
| `/dashboard/examenes` | Línea de tiempo de exámenes | Mikareno |
| `/dashboard/planner` | Planificador semanal | Mikareno + conteos reales |
| `/dashboard/profile` | Perfil del estudiante | Mikareno (`get_perfil`) |
| `/dashboard/subjects` | Asignaturas con promedios | Mikareno (`get_student_full`) |
| `/dashboard/subjects/{code}` | Detalle de una asignatura | Mikareno (`get_detalle_materia`, `get_actividades_materia`) |
| `/dashboard/vida-escolar` | Vida escolar (6 secciones) | Mikareno (6 tools) |
| `/app/notes` … | Notas Markdown (CRUD) | SQLite + `.md` en disco |
| `/dashboard/notes` | Panel del segundo cerebro | SQLite + `.md` en disco |
| `/dashboard/material` | Generador de material con IA | OpenRouter + Mikareno |
| `/dashboard/search?q=` | Búsqueda de notas (HTML) | SQLite |
| `/api/search?q=` | Búsqueda de notas (JSON) | SQLite |
| `/settings` | Configuración (sólo lectura + generador de `.env`) | Config del proceso |
| `/admin` | Administración y métricas | Proceso + SQLite |
| `/dashboard/mcp` | Centro MCP | Mikareno + GenOffice |
| `/ai` y `/dashboard/ai` | Chat con IA | OpenRouter |
| `/office/*` | Docs, hojas, presentaciones, biblioteca | MCP de GenOffice |

### 3.1 `/` y `/dashboard` — Panel principal (visión)

Es la pantalla de arranque: saludo con la hora y la fecha, línea de "hoy", estado
de conexión del MCP, modelo de IA configurado, **clases de hoy** (del horario),
contadores de tareas (hoy / semana), **tareas pendientes**, **notas locales
recientes**, calendario de la semana, **próximo examen** y bloque de foco.

Todo se alimenta de un solo scrape (`get_student_full` trae promedios, horario,
calendario y tareas) más `get_perfil`, para no golpear el servidor MCP con cinco
peticiones por carga.

`/dashboard` acepta `?partial=stats|profile|subjects|events|tasks` para recargar
solo un fragmento (HTMX).

### 3.2 `/dashboard/horario` — Grilla de horario

- **Siempre 7 columnas**, de **lunes a domingo**, con el número del día del mes
  en la cabecera (p. ej. "Lunes 19").
- Cada celda trae hora, materia y código (`07:35 – 08:15 — Materia [CÓD]`).
- Los días sin clases muestran **"Sin clases"** (no se rellenan con datos
  inventados).
- Es *cache-first*: si el horario ya está en la caché del cliente MCP se sirve
  sin I/O. Presupuesto total de 30 s para traer datos y 8 s para renegociar la
  sesión MCP, así la página nunca se queda colgada si el servidor Mikareno está
  ocupado capturando.

### 3.3 `/dashboard/calendar` y `/dashboard/calendar.ics`

- Calendario mensual tipo Google Calendar con navegación de mes (`?month=YYYY-MM`),
  eventos reales de Mikareno; `/dashboard/calendar/daily` muestra la agenda del día.
- **`/dashboard/calendar.ics`** descarga un archivo `calendario.ics`
  (`text/calendar`, `Content-Disposition: attachment`) con los eventos de tipo
  `SCHOOL_DAY` y `EVENTO` — se puede importar en Google Calendar, Thunderbird o
  cualquier cliente .ics.

### 3.4 `/dashboard/examenes` — Exámenes

Línea de tiempo de los exámenes extraídos del calendario de Mikareno (eventos
cuyo tipo o título menciona examen/evaluación), ordenados por fecha, con
contador arriba. Si no hay eventos, estado vacío honesto.

### 3.5 `/dashboard/planner` — Planificador semanal

Rejilla de lunes a domingo con la agenda de la semana. Las tareas vienen de
Mikareno; como el servidor real no expone la lista detallada de tareas, la
página usa los **conteos reales** que sí reporta (`tarea_stats`:
`todaysTasks`, `tasksCurrentWeek`) y, si no hay lista, muestra estado vacío.

### 3.6 `/dashboard/profile` — Perfil

Nombre, institución y curso obtenidos de `get_perfil`. Si el MCP no responde,
los campos quedan como "Sin datos".

### 3.7 `/dashboard/subjects` y `/dashboard/subjects/{code}` — Asignaturas

- Listado de asignaturas con **promedios** (calculados sobre las notas reales
  del parcial en curso, agrupadas por materia).
- El detalle (`{code}` = `courseCode` de Mikareno, p. ej. `INFO01`) muestra el
  desglose de notas por submenú/evaluación y las actividades de la materia.
  Si el código no existe, avisa "asignatura no encontrada".

### 3.8 `/dashboard/vida-escolar` — Vida escolar

Página integradora de **6 secciones** en acordeón, todas de solo lectura sobre
tools `get_*` del MCP:

| Sección | Tool |
|---|---|
| Mensajes | `get_mensajes` |
| Asistencia | `get_asistencia` |
| Semáforo | `get_semaforo` |
| Biblioteca | `get_biblioteca` |
| Cumpleaños | `get_cumpleanos` |
| Año escolar | `get_ano_escolar` |

Se cargan en paralelo (6 tools concurrentes, 28 s de deadline; handshake 10 s).
Se puede recargar una sección suelta con `?partial=<sección>`.

### 3.9 Notas Markdown — `/app/notes` y `/dashboard/notes`

**Dónde vive cada cosa**

- El **cuerpo** de la nota es un archivo `{slug}.md` en `notes/`, con frontmatter
  YAML (título, tags, created, modified) — compatible con Obsidian.
- Los **metadatos** (título, slug, tags, `word_count`, fechas) están en la tabla
  `notes` de SQLite.
- El **historial** guarda instantáneas del contenido en `note_versions` (se
  conservan las últimas 5).

**Rutas**

- `/app/notes` — listado de todas las notas.
- `/app/notes/new` — alta de nota.
- `/app/notes/{slug}` — lectura renderizada (Markdown → HTML).
- `/app/notes/{slug}/edit` → `PUT /app/notes/{slug}/update` — edición;
  el `POST /app/notes` / `PUT` **escribe el `.md` en disco** y actualiza SQLite.
- `DELETE /app/notes/{slug}` — borra la nota.
- `/dashboard/notes` — panel del **segundo cerebro**: total real de notas y
  palabras, lista del explorador, etiquetas agregadas con su conteo, nota más
  reciente con su contenido, **enlaces salientes** y **backlinks** reales
  (calculados escaneando los `.md` de la bóveda).

**Wikilinks (sintaxis Obsidian)**

Se admiten las cuatro formas: `[[Nota]]`, `[[Nota|alias]]`, `[[Nota#Sección]]` y
`[[Nota#Sección|alias]]`. Al renderizar, un wikilink **resuelto** se convierte
en un enlace normal a la nota destino (clic normal para abrirla); si la nota
destino **no existe**, queda como texto plano. No existe ninguna variante
"shift-click" ni menú contextual: el único uso de `Shift` en la app es
`Shift+Enter` en el compositor del chat para insertar un salto de línea
(`Enter` envía el mensaje).

**Búsqueda**: por título, slug y etiquetas, coincidencia parcial, ordenadas por
fecha de actualización, con un máximo de 50 resultados.

### 3.10 `/dashboard/material` — Generador de material con IA

Formulario con tres campos: **materia** (select con las notas reales de
Mikareno), **tipo de material** y **tema**. Al enviar (`POST /dashboard/material`)
genera el material vía OpenRouter. Si no hay `OPENROUTER_API_KEY` configurada,
responde con un error honesto de modo demo: **no inventa contenido**.

### 3.11 `/settings` — Configuración (sólo lectura, con generador de `.env`)

Muestra los valores reales del proceso: estado de conexión del MCP de Mikareno y
su URL, modo de autenticación, y la **tarjeta "IA / OpenRouter"** con el modelo,
el **presupuesto mensual**, el máximo de tokens por respuesta y el estado de la
clave. La clave de API **nunca** se muestra, sólo su estado.

El formulario de esa tarjeta (modelo, máx. tokens, presupuesto) tiene un botón
**"Generar .env"**, no "Guardar": al enviarlo a `POST /api/settings/ai` la app
**no guarda nada** —el entorno se lee sólo al arrancar y la configuración es
inmutable en runtime—, responde `501` y devuelve un bloque `.env` con tus
valores (sin la clave) para copiarlo al servidor y reiniciar el servicio. Un
valor no válido devuelve `400` con la lista de errores en español.

### 3.12 `/admin` — Administración

Panel de administración (pestaña del rail lateral). Todo lo que muestra son
cifras reales, leídas de la BD, del proceso y del sistema:

- **Uptime**, notas, chats, RAM usada/total, disco libre y tamaño del archivo
  SQLite (`GET /admin/metrics` en JSON; RAM de `/proc/meminfo`, disco de `df -kP`).
- **Estado del MCP de Mikareno** (conectado / sin conexión + latencia) y del
  modelo de IA con su presupuesto.
- **Presupuesto de IA** (`GET /admin/openrouter/budget`): gasto real del mes
  (`usage_metrics`), tope y alerta al 90 %.

#### Backups (copias de seguridad)

Un backup es una copia del fichero SQLite en `data/backups/`, llamada
`app-YYYYmmdd-HHMMSS.db`, con un sidecar `app-….db.sha256` que guarda el
resultado del `PRAGMA integrity_check` de la base en el momento de la copia
(check de integridad, no una firma criptográfica).

- **Backup diario automático:** la app arranca una tarea interna que hace una
  copia **60 segundos después de arrancar** y luego **una vez cada 24 h**
  (`src/backup::spawn_backup_task`). No hay cron ni scripts externos: es un
  `tokio::task` del propio proceso.
- **Retención:** se conservan las **5 copias más recientes**; las más viejas
  (y sus sidecars) se borran solas tras cada copia.
- **Crear uno a mano:** botón "Crear backup" del panel (`GET /api/admin/backup`)
  o `POST /api/admin/backup` (JSON: `{success, name, bytes}`).
- **Ver el estado:** `GET /api/admin/backup-status` →
  `{last_backup:{path, size_bytes, created_at} | null, count}`; el listado
  completo está en `GET /admin/backup/list`.
- **Descargar una copia:** `GET /api/admin/backup/download/{name}` — devuelve
  el fichero SQLite con `Content-Disposition: attachment`. El `name` sólo se
  acepta si tiene la forma `app-*.db` y el fichero se busca **siempre** dentro
  de `data/backups` (nunca se interpreta como una ruta).
- **Restaurar:** `POST /api/admin/backup/restore` con `{"name":"app-….db"}`.
  Antes de tocar nada valida el nombre y el `integrity_check` del fichero; luego
  copia el **contenido** del backup sobre la base viva (sin sustituir el
  fichero en disco, para no romper el WAL ni el esquema en marcha). Si la base
  está bloqueada, el error es honesto: *"server en uso: reintenta tras detener
  la app"*. Nunca simula un restore que no ha ocurrido.

> Todo el sistema de backups usa **un único directorio** (`data/backups/`): el
> automático, el manual, el listado, la descarga y el restore. El `/backup` en
> la raíz del proyecto es de una versión antigua y ya no se usa.

#### Presupuesto de IA — 2 USD/mes

- Tope por defecto: **2.00 USD/mes** (`OPENROUTER_MAX_COST_USD`).
- El gasto real de cada llamada se guarda en la tabla `usage_metrics`, así que
  sobrevive a los reinicios.
- **Alerta al 90 %** (con el tope de $2,00 salta en **$1,80**); se ve en
  `/admin`, en `/admin/openrouter/budget` y en `GET /api/ai/cost`.
- Si se supera el tope, la app responde **HTTP 429** y **deja de llamar** al
  proveedor hasta el mes siguiente.
- El presupuesto se cambia en la página `/settings` (tarjeta "IA / OpenRouter" →
  botón **"Generar .env"**): genera el bloque `.env` con tus valores para
  copiarlo al servidor y reiniciar el servicio. **No se guarda en caliente.**

### 3.13 Cambiar la contraseña — pendiente de ruta

**Hoy no hay ninguna pantalla ni endpoint HTTP para cambiar la contraseña.**
Esto es lo que hay verificado en el código (2026-10-19):

- El handler `auth::set_password` (que re-hashea con Argon2 y actualiza el
  `password_hash` del usuario `admin`) **existe pero no está registrado** en
  ningún router: no hay ruta que lo alcance.
- No existe ninguna función `password_page` ni página de "cambiar contraseña"
  en el shell.
- El CLI **no** expone `set-password`: sólo acepta `--migrate` y
  `--create-admin <usuario>`. Y `--create-admin` sobre un usuario que ya existe
  **no** cambia la contraseña (su `ON CONFLICT` sólo re-declara el nombre), así
  que tampoco sirve como atajo.
- `POST /register` crea cuentas nuevas, pero exige el token de invitación
  `REGISTRATION_TOKEN`; sin esa variable en el entorno el registro está
  deshabilitado (`403`).

Mientras tanto, para cambiar la contraseña hay que hacerlo **fuera de la app**:
generar un hash Argon2 de la nueva contraseña y actualizar la fila en SQLite
(`UPDATE users SET password_hash = '…' WHERE username = '…'`), o registrar la
ruta que falta. La ruta prevista sería algo como `POST /api/account/password`
sobre el handler ya existente.

### 3.14 `/dashboard/mcp` — Centro MCP

Estado de los servidores MCP (Mikareno y GenOffice), latencia, herramientas
disponibles, telemetría, estado de la clave de OpenRouter y el presupuesto
configurado.

### 3.15 `/office/*` — Suite de ofimática

Proxy al MCP de GenOffice: `/office` (inicio), `/office/docs`, `/office/sheets`,
`/office/presentations`, `/office/library` (biblioteca de archivos). Requiere que
el MCP de GenOffice esté levantado (**sólo existe en la Dell**).

**`GET /office/report`** descarga el **informe del estudiante en `.docx`**
(apuntes de la BD + perfil y notas del MCP de Mikareno). Si el MCP de GenOffice
no está disponible la respuesta es `503` con un JSON honesto
(`{"error":"GenOffice no disponible","motivo":"…"}`): **no** hay `.docx` de
reemplazo. Sin sesión responde `401` en JSON (no te manda a la página de login).

---

## 4. Chat con IA (`/ai` y `/dashboard/ai`)

Hay dos pantallas de chat: `/ai` (asistente clásico) y `/dashboard/ai` (el mismo
asistente dentro del shell v2, con sesiones, filtros por materia, cuota y
costs). Ambos usan los mismos endpoints:

| Endpoint | Qué hace |
|---|---|
| `POST /api/ai/chat` | Chat con una materia como contexto |
| `POST /api/ai/chat/stream` | **El mismo chat en streaming SSE** (`text/event-stream`, un frame por evento) |
| `POST /api/ai/summary` | Resume un texto (máx. 500 tokens) |
| `POST /api/ai/quiz` | Genera preguntas de quiz (1 a 10) |
| `POST /api/ai/study-sheet` | Genera una hoja de estudio |
| `GET /api/ai/cost` | Gasto del mes, tope y nº de peticiones |
| `GET /api/ai/conversations` | Historial de conversaciones por materia |

**Streaming:** la API expone el streaming por SSE en `POST /api/ai/chat/stream`
(`text/event-stream`, con eventos `delta`, `reasoning`, `done` y `error`, más la
cabecera `x-chat-stream-id`). El turno **se guarda en la base antes del frame
final**: el mensaje y la respuesta del asistente van a `chat_history` (los mismos
sitios que el chat normal) y el consumo a `usage_metrics`. Ojo: **la interfaz
que tienes delante hoy consume `POST /api/ai/chat`** (respuesta completa de una
vez), no el endpoint de streaming; el streaming queda disponible para clientes
que lo usen (por ejemplo desde otro agente o script).

**Presupuesto: 2 USD/mes por defecto**

- Tope configurable con `OPENROUTER_MAX_COST_USD` (por defecto `2.0`).
- El gasto real de cada llamada se guarda en la tabla `usage_metrics` de
  SQLite (modelo, tokens de entrada/salida, coste, si fue estimado, endpoint),
  así que **sobrevive reinicios** del servidor.
- El control se hace con una transacción `BEGIN IMMEDIATE` (bloqueo de
  escritura): se comprueba el gasto y se reserva antes de llamar al proveedor.
- **Alerta al 90 %** del tope (con el tope de $2,00 salta en $1,80).
- Si se supera el tope, la app responde **HTTP 429** con el mensaje
  "Monthly AI budget exhausted… Try again next month." y **deja de llamar** al
  proveedor.
- Sin `OPENROUTER_API_KEY` la app entra en **modo simulado** (las respuestas
  empiezan por `[Modo simulado]`); con el modelo gratuito configurado el coste
  real es $0.

---

## 5. Cómo funciona el MCP de Mikareno

- **Origen de los datos:** `mikareno-mcp` (servidor Python/FastMCP) hace el
  *scraping* de la plataforma escolar y expone tools de solo lectura. La app
  **no** habla con la plataforma escolar directamente: habla con el MCP por
  JSON-RPC 2.0 sobre HTTP/SSE (`POST /mcp`, cabecera `mcp-session-id`).
- **Herramientas usadas** (todas prefijo `get_`): `get_perfil`, `get_horario`,
  `get_calendario`, `get_tareas`, `get_notas_trimestre`, `get_student_full`,
  `get_ano_escolar`, `get_biblioteca`, `get_asistencia`, `get_mensajes`,
  `get_semaforo`, `get_cumpleanos`, `get_detalle_materia`,
  `get_actividades_materia`. El cliente es de lectura por construcción: sólo
  existe `call_tool` con nombres `get_*`.
- **Caché de 180 segundos** (TTL por defecto, read-through) sobre las respuestas
  crudas de cada tool, con clave `name(args)`. Por eso abrir dos páginas seguidas
  no vuelve a lanzar el scraping. La caché es **compartida por toda la app** (un
  único cliente MCP en `AppState`), y al arrancar se **precalienta en segundo
  plano** para que la primera visita real no pague el coste en frío.
- **Tolerancia a fallos:** 30 s de timeout por petición, 3 reintentos con backoff
  (500 ms → 1 s → 2 s), deadline de 25 s por llamada de tool y handshake de 10 s en
  las páginas. Si algo falla, la pantalla muestra un estado vacío honesto; nunca
  se rellenan datos de ejemplo.
- **La app también es servidor MCP:** expone `POST /mcp` (JSON-RPC 2.0 **en
  JSON**, no SSE) y `POST /mcp/sse` (transporte SSE) con **9 tools** propios —
  `search_notes`, `get_note`, `create_note`, `get_academic_context`,
  `get_subject_notes`, `get_study_sheets`, `app_list_backups`, `app_backup_now`,
  `app_ai_cost` — para que otros agentes puedan trabajar contra tus notas y tu
  contexto académico. Las tres últimas son de operación: `app_backup_now`
  devuelve `{success, name, bytes, path, size_bytes}` y `app_ai_cost` devuelve
  `{total_cost, budget_limit, requests_count, alert}`.

---

## 6. Buscar (y lo que aún no está conectado)

- **Buscador global del top-bar:** la caja de búsqueda de la barra superior es
  hoy **visual**; no está enganchada a la API. No esperes resultados de ella.
- **Búsqueda que sí funciona:**
  - `/dashboard/search?q=término` — página HTML con los resultados (título,
    etiquetas) y enlaces a `/app/notes/{slug}`.
  - `GET /api/search?q=término` — JSON `{query, count, results[{slug,title,tags}]}`,
    máximo 50 resultados; con `q` vacío devuelve `count: 0` sin tocar la base, y
    con `q` de más de 200 caracteres responde 400.
- Busca en **título, slug y etiquetas** por coincidencia parcial, ordenadas por
  última modificación.

---

## 7. Atajos y detalles de uso

- `Enter` envía el mensaje en el chat; `Shift+Enter` hace salto de línea.
- En las páginas con datos de Mikareno se puede recargar un bloque suelto con
  `?partial=...` (dashboard, calendario, perfil, asignaturas, vida escolar).
- Los estados vacíos son información real: "Sin clases", "Sin datos",
  "Sin exámenes programados" significan que el MCP no devolvió ese dato, no que
  falte configurarlo.
- La salud del sistema está en `/admin` (uptime, RAM, disco, tamaño de la BD,
  presupuesto de IA, backups) y en `/dashboard/mcp` (estado de los servidores).
