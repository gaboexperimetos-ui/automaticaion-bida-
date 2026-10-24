# Nova 3.0 — rediseño integral de Study Platform

> Documento vivo. Actualizado: **2026-10-24**.

## Objetivo y alcance

Nova 3.0 sustituye la composición anterior por un espacio académico personal, oscuro e inmersivo. No es una variación de la barra lateral clásica ni una capa visual limitada a la portada: el shell, el panel y las pantallas académicas, de IA, conocimiento, Oficina, integraciones, administración y configuración tienen composiciones propias.

El requisito rígido es conservar las funcionalidades existentes. Se reorganiza la presentación, pero no se eliminan destinos, controles activos, endpoints ni contratos de datos. El backend Rust/Axum, SQLite, HTMX y el renderizado de HTML en servidor se mantienen; no se introduce React, Vite ni un segundo frontend de producción.

## Dirección visual y de interacción

- **Cabecera flotante:** identidad `study / platform`, subtítulo `personal academic OS`, versión visible `v3.0`, buscador, creación, estado de conexiones, notificaciones y cuenta.
- **Navegación inmersiva:** los 22 destinos aparecen en un menú superpuesto a pantalla completa. Estudios, Inteligencia artificial y Conocimiento se muestran como colecciones; Oficina e Integraciones conservan sus grupos plegables. Administración, Configuración y Vault siguen disponibles.
- **Panel editorial:** saludo de gran formato, estado de Mikareno, semana, jornada, copiloto, prioridades y tarjetas de apoyo para conocimiento, próximo examen y promedios.
- **Académico:** tarjetas de asignaturas, detalle con promedio y hechos académicos, actividades en tabla, planificador semanal, calendario mensual/diario/semanal, evaluaciones, horario, vida escolar, perfil y búsqueda. El estado de asignatura no encontrada también tiene una composición Nova propia.
- **IA y conocimiento:** generador de material, asistente con fuentes, historial de conversaciones, notas con editor/vista dividida/preview, grafo, backlinks y centro MCP.
- **Oficina:** portada de aplicaciones, biblioteca, documentos Markdown, hojas editables con CSV y presentaciones con orden de diapositivas y modo de presentación.
- **Sistema:** administración, ajustes, integraciones, estados vacíos y formularios comparten el lenguaje Nova sin convertir todas las pantallas en una única cuadrícula genérica.
- **Sistema visual:** superficies oscuras translúcidas, bordes finos, tipografía Inter/Manrope, iconos Material Symbols, iluminación ambiental y acentos violeta/cian. `/nova.css` se carga después de `/polish.css`.
- **Responsive:** una sola columna en móvil, tablas y rejillas con desplazamiento interno cuando lo necesitan, y menú con scroll propio. Vault queda después de todo el contenido desplazable, sin superponerse a los accesos.
- **Accesibilidad:** skip-link, foco visible, `aria-expanded`, cierre con Escape, bloqueo del scroll del documento mientras el menú está abierto, etiquetas accesibles y controles con nombres explícitos. También hay estilos de movimiento reducido e impresión.

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
| `static/stitch-v2/shell/aside.html` | Mapa de navegación inmersivo, grupos, accesos de sistema y Vault |
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

La preview compone las plantillas y los fragmentos Rust reales. `pages.mjs` resuelve slots y argumentos de `format!` con datos ficticios; `server.mjs` añade el shell, HTMX local, assets y respuestas API ficticias. No modifica la base de datos ni sustituye las pruebas contra Rust/SQLite.

```bash
cd design/ui-preview
npm ci
node server.mjs
```

El servidor escucha en `0.0.0.0:3000`. En otra terminal, desde la raíz del repositorio:

```bash
node design/ui-preview/capture.mjs
```

La auditoría recorre **34 rutas** en escritorio y móvil, más **12 interacciones**. Genera capturas seleccionadas y comprueba los 22 accesos, overflow del documento, slots, IDs duplicados, recursos externos, respuestas fallidas, versión y errores de consola. Debe reiniciarse la preview tras cambiar plantillas o Rust; `nova.css` se lee de nuevo en cada petición.

## Validación realizada

Resultado de la pasada integral del **2026-10-24**:

- **80 comprobaciones y 0 fallos** en Chromium.
- **34 rutas en 1440 × 1000 y 34 en 390 × 844**, incluidos editores, presentación, búsqueda, detalle y estado de asignatura no encontrada.
- Ancho del documento igual al viewport, sin errores de JavaScript, respuestas 404, slots pendientes, IDs duplicados ni recursos externos en la preview.
- 22 destinos y menú inicialmente cerrado en todas las cargas; versión `3.0` presente.
- Apertura/cierre del menú en escritorio y móvil, Escape, `aria-expanded` y bloqueo/restauración del scroll.
- Fuentes del Asistente, historial de Conversaciones, hidratación y modos de Notas, secreto/ping/playground de MCP.
- Cuadrícula editable y CSV, toolbar Markdown, orden de diapositivas, tipo de importación en Biblioteca, partial HTMX de Calendario y acordeones de Vida escolar.
- Revisión visual de las capturas de panel, Calendario, Notas, Presentaciones, Detalle y estado no encontrado.
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
