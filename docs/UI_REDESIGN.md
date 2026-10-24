# NOVA Studio 3.0 — reconstrucción visual integral

> Documento vivo. Actualizado: **2026-10-24**.

## Objetivo y alcance

NOVA Studio 3.0 sustituye la composición anterior por un espacio académico personal, oscuro y orientado al trabajo. La etapa Studio reconstruye el shell con navegación persistente, una cabecera integrada y un sistema cromático/tipográfico nuevo; no es una capa limitada a la portada. El panel y las pantallas académicas, de IA, conocimiento, Oficina, integraciones, administración, configuración y acceso comparten la misma identidad sin perder sus composiciones propias.

El requisito rígido es conservar las funcionalidades existentes. Se reorganiza la presentación, pero no se eliminan destinos, controles activos, endpoints ni contratos de datos. El backend Rust/Axum, SQLite, HTMX y el renderizado de HTML en servidor se mantienen; no se introduce React, Vite ni un segundo frontend de producción.

## Dirección visual y de interacción

- **Cabecera integrada:** identidad `NOVA`, subtítulo `academic system`, versión visible `3.0`, buscador, creación, estado de conexiones, notificaciones y cuenta. En móvil conserva las acciones esenciales sin ocupar una segunda barra.
- **Navegación de trabajo:** sidebar grafito de 244 px visible desde 1200 px, con offset de 276 px y opción de plegado. Por debajo de ese ancho se convierte en cajón inicialmente cerrado. Los 22 destinos se conservan; Oficina e Integraciones son grupos plegables y el enlace activo queda abierto y visible.
- **Panel editorial:** saludo de gran formato, estado de Mikareno, semana, jornada, copiloto, prioridades y tarjetas de apoyo para conocimiento, próximo examen y promedios.
- **Académico:** tarjetas de asignaturas, detalle con promedio y hechos académicos, actividades en tabla, planificador semanal, calendario mensual/diario/semanal, evaluaciones, horario, vida escolar, perfil y búsqueda. El estado de asignatura no encontrada también tiene una composición Nova propia.
- **IA y conocimiento:** generador de material, asistente con fuentes, historial de conversaciones, notas con editor/vista dividida/preview, grafo, backlinks y centro MCP.
- **Oficina:** portada de aplicaciones, biblioteca, documentos Markdown, hojas editables con CSV y presentaciones con orden de diapositivas y modo de presentación.
- **Sistema:** administración, ajustes, integraciones, estados vacíos y formularios comparten el lenguaje Nova sin convertir todas las pantallas en una única cuadrícula genérica.
- **Sistema visual:** grafito cálido/verdoso, marfil, lima, menta y arena; superficies mate con bordes finos, tipografía Inter/Manrope de mayor escala e iconos Material Symbols. Los tokens se actualizaron en `head.html`, `polish.css`, `nova.css`, `shell.js`, las configuraciones Tailwind y las pantallas de acceso; no quedan estados dinámicos con la antigua paleta violeta/cian.
- **Responsive:** una sola columna en móvil y menú con scroll propio. Las rejillas temporales conservan desplazamiento interno cuando lo necesitan; las actividades de una asignatura se transforman en tarjetas etiquetadas para no perder Nota, Estado ni Descripción. Vault queda después de todo el contenido desplazable.
- **Accesibilidad:** skip-link, foco visible, `aria-expanded`, cierre con Escape y controles con nombres explícitos. El bloqueo de scroll y el fondo modal se aplican solo al cajón estrecho; el sidebar desktop nunca bloquea el documento. También hay estilos de movimiento reducido e impresión.

## Contratos preservados

- `NAV_LINKS` sigue siendo la fuente canónica de **22 destinos**; el shell conserva el mismo conjunto de `data-path` y un único `aria-current="page"`.
- El panel conserva sus **17 slots** de contenido y el shell sus slots `SHELL_AVATAR`, `SHELL_NAME`, `SHELL_ROLE`, `SHELL_STATUS`, `SHELL_NAV_MCP` y `SHELL_VAULT_PCT`.
- `dashboard.html` e `index.html` tienen el mismo contenido activo.
- Se mantienen los identificadores usados por `shell.js`: `toggle-create-menu`, `quick-create-btn`, `create-modal-popover`, `quick-task-modal`, sus botones de cierre y los campos de la tarea rápida.
- Permanecen los IDs, formularios, atributos HTMX y endpoints de los editores, fuentes del asistente, historial, notas, MCP, calendario, hojas, presentaciones, biblioteca, administración y ajustes.
- La adaptación heredada de `/ai` no intercepta el workspace nuevo de conversaciones; las pantallas antiguas sin ese workspace conservan la redirección al asistente actual.
- Las acciones de creación, búsqueda, notificaciones, cuenta y guardado como nota siguen usando los endpoints y la lógica existentes.
- Los enlaces destacados apuntan a rutas reales (`/dashboard/planner`, `/dashboard/calendar`, `/dashboard/notes` y `/dashboard/mcp`).
- La versión de la API pública del shell permanece en `3.0.0`.

## Archivos principales

| Archivo | Responsabilidad |
|---|---|
| `static/stitch-v2/shell/head.html` | Tokens, fuentes, estilos base y carga ordenada de `/polish.css` y `/nova.css` |
| `static/stitch-v2/shell/aside.html` | Sidebar persistente/cajón, grupos, accesos de sistema y Vault |
| `static/stitch-v2/shell/topbar.html` | Cabecera flotante, buscador, acciones y estructura de cuenta |
| `static/stitch-v2/_content/clean/` | Panel, Material, Asistente, Notas, MCP y Ajustes |
| `static/app/nova.css` | Sistema visual integral, composiciones por área, responsive, impresión y movimiento reducido |
| `static/app/polish.css` | Estilos compartidos y capas anteriores, cargados antes de Nova |
| `static/app/shell.js` | Interactividad y llamadas API existentes |
| `src/handlers/stitch_v2.rs` | Composición del shell, navegación activa, cuenta y comportamiento accesible del menú |
| `src/handlers/stitch_panel.rs` | Datos reales/degradados y relleno de slots del panel |
| `src/academic/` | Pantallas académicas, detalle, calendario, planificador, evaluaciones y perfil |
| `src/ai/handlers.rs` | Workspace de conversaciones y sus contratos JavaScript |
| `src/office/` | Documentos, hojas, presentaciones, biblioteca y modo de presentación |
| `src/handlers/static_ui.rs` | Servicio embebido de `/shell.js`, `/polish.css` y `/nova.css` |
| `src/handlers/mod.rs` | Registro de rutas públicas de los assets del shell |
| `design/ui-preview/` | Preview aislada y auditoría reproducible en Chromium |

## Preview local

La preview compone las plantillas y los fragmentos Rust reales. `pages.mjs` resuelve slots y argumentos de `format!` con datos ficticios coherentes; `server.mjs` añade el shell, el estado activo derivado de `NAV_LINKS`, HTMX local, assets y respuestas API ficticias. No modifica la base de datos ni sustituye las pruebas contra Rust/SQLite.

```bash
cd design/ui-preview
npm ci
node server.mjs
```

El servidor escucha en `0.0.0.0:3000`. En otra terminal, desde la raíz del repositorio:

```bash
node design/ui-preview/capture.mjs
```

La auditoría recorre **34 rutas** en escritorio y móvil, más **18 interacciones**. Genera capturas seleccionadas y comprueba los 22 accesos, un único `aria-current` correcto por ruta, visibilidad del menú según viewport, overflow del documento, slots, IDs duplicados, recursos externos, respuestas fallidas, versión y errores de consola. Debe reiniciarse la preview tras cambiar plantillas o Rust; `nova.css` se lee de nuevo en cada petición.

Para revisar una sección concreta sin repetir toda la matriz:

```bash
NOVA_ROUTES=/dashboard/notes,/office/library \
NOVA_VIEWPORTS=mobile \
NOVA_FULL_PAGE=0 \
NOVA_SCREENSHOTS=all \
NOVA_INTERACTIONS=0 \
node design/ui-preview/capture.mjs
```

`NOVA_ROUTES` filtra rutas, `NOVA_VIEWPORTS` acepta `desktop`/`mobile`, `NOVA_FULL_PAGE=0` limita la captura al viewport y `NOVA_SCREENSHOTS=all` guarda todas las rutas seleccionadas.

## Acabado sección por sección

La segunda pasada de diseño corrigió detalles que no detecta una simple comprobación de overflow:

- Navegación del Planificador en tres columnas, sin textos pegados entre anterior, rango y siguiente.
- Historiales de Asistente y Conversaciones con altura natural en móvil; la cuota ya no corta las conversaciones.
- Controles, filtros y metadatos de IA con una escala tipográfica más legible.
- Título de Notas con el estado en una segunda línea móvil para aprovechar el ancho sin truncar el nombre.
- Biblioteca convertida en tarjetas adaptativas en pantallas pequeñas, conservando tabla, descargas, borrado y etiquetas de tipo/tamaño/fecha.
- Eventos semanales con `<time>` semántico, hora visible `HH:MM` o “Sin hora”, y abreviaturas `Lun`, `Mar`, `Mié`, etc., sin ambigüedad ni cortes UTF-8.
- Capturas de viewport para revisar paneles altos a tamaño real y pruebas específicas de regresión para estos cuatro casos móviles.

## Reconstrucción del chat

Asistente (`/dashboard/ai`) y Conversaciones (`/ai`) comparten una superficie de trabajo centrada en la conversación:

- Sin cabecera hero ni tres tarjetas independientes: historial, mensajes y composer forman una sola superficie que aprovecha el viewport.
- Fuentes como panel superpuesto; en móvil, el historial se abre como cajón y no desplaza la conversación hacia abajo.
- Mensajes semánticos con avatar, autor, materia, hora y burbujas diferenciadas; el historial Rust y los turnos del streaming usan las mismas clases.
- Texto de lectura a 15 px, composer con foco visible y acciones de estudio secundarias; al abrir una conversación se muestra el mensaje más reciente.
- Estado vacío con sugerencias que rellenan el compositor sin enviar automáticamente.
- Búsqueda funcional de conversaciones, sesión activa, cierre por Escape o al tocar fuera y composer completo dentro del viewport móvil.
- El indicador de escritura se añade a la conversación en lugar de borrar el historial visible.
- Copiar y crear un cuestionario actúan sobre la respuesta seleccionada; copiar dispone de fallback cuando el navegador deniega el portapapeles.
- La preview incluye mensajes reales de ejemplo y respuestas ficticias para chat, resumen, cuestionario y ficha de estudio.

## Validación realizada

Resultado de la pasada integral final del **2026-10-24**, ejecutada contra la preview con los fixtures y el código de esta etapa:

- **86 comprobaciones y 0 fallos** en Chromium, tras verificar también el contrato real de reordenación de diapositivas (`POST` con `order=1,0,2,3,4`, codificado en el formulario).
- **34 rutas en 1440 × 1000 y 34 en 390 × 844**, incluidos editores, presentación, búsqueda, detalle y estado de asignatura no encontrada.
- Una pasada visual focalizada adicional de **14 comprobaciones y 0 fallos** sobre Búsqueda, detalle de Asignatura, Markdown, Presentaciones, Biblioteca y chat en ambos viewports; las capturas finales se revisaron después de completar los fixtures y la rejilla móvil de acciones del chat.
- Ancho del documento igual al viewport, sin errores de JavaScript, respuestas 404, slots pendientes, IDs duplicados ni recursos externos en la preview.
- 22 destinos y menú inicialmente cerrado en todas las cargas; versión `3.0` presente.
- Apertura/cierre del menú en escritorio y móvil, Escape, `aria-expanded` y bloqueo/restauración del scroll.
- Fuentes del Asistente, historial de Conversaciones, envío sin perder mensajes, copiar y generar cuestionario desde una respuesta.
- Cajón móvil del chat, búsqueda de sesiones, sugerencias y composer visible; hidratación y modos de Notas; secreto/ping/playground de MCP.
- Cuadrícula editable y CSV, toolbar Markdown, orden de diapositivas, tipo de importación en Biblioteca, partial HTMX de Calendario y acordeones de Vida escolar.
- Navegación semanal separada, título de Notas y archivos móviles de Biblioteca con metadatos y acciones visibles dentro del viewport.
- Revisión visual sección por sección de Panel, Académico, IA, Notas, MCP, Administración, Ajustes, Oficina y subpáginas de creación, preview y presentación.
- `node --check` de la preview, la auditoría y `shell.js`, además de `git diff --check`, sin errores.

## Verificación pendiente por entorno

El entorno de edición no incluye `cargo` ni `rustc`; por tanto, no se afirma que el proyecto haya compilado ni que la suite Rust se haya ejecutado. Con el toolchain disponible, ejecutar:

```bash
cargo check
cargo test
cargo build
cargo fmt --check
cargo clippy --all-targets
```

Las plantillas y los assets se incrustan con `include_str!`/`include_bytes!`: para servir estos cambios en producción hay que recompilar el backend. Este rediseño no incluye un despliegue.
