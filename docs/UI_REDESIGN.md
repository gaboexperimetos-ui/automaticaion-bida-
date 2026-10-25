# NOVA 3.0 Nébula — reconstrucción visual integral

> Documento vivo. Actualizado: **2026-10-24**.

## Objetivo y alcance

NOVA 3.0 Nébula reconstruye desde una base nueva el shell, el panel, el chat y todas las subsecciones: pantallas académicas, conocimiento, Oficina, integraciones, administración, configuración y autenticación. No es una variante de la portada anterior ni una capa de retoques sobre el shell antiguo.

El requisito rígido es conservar las funcionalidades existentes. Se reorganizan presentación y navegación, pero se mantienen destinos, controles activos, endpoints y contratos de datos. El backend Rust/Axum, SQLite, HTMX y el HTML renderizado en servidor continúan; no se introduce React, Vite ni un segundo frontend de producción.

## Dirección Nébula

- **Sistema visual:** fondo azul noche `#080b14`, superficies `#111827`–`#2a3852`, texto `#f3f6ff`, acento violeta `#a78bfa`, cian `#67e8f9` y ámbar `#ffbd7a` para estados semánticos. Tipografías Manrope, DM Sans y Space Grotesk, con Material Symbols. La dirección Nébula sustituye por completo la anterior paleta Workspace verde.
- **Shell:** rail flotante de 262 px en escritorio, colapsable, con 22 destinos agrupados y versión `3.0`; cajón con backdrop en móvil. La topbar flotante de 64 px concentra contexto, búsqueda, creación, notificaciones y cuenta. La navegación revela el destino activo cuando pertenece a una sección fuera del viewport.
- **Panel:** composición editorial de jornada, prioridades, agenda, copiloto y conocimiento. Conserva captura rápida, menú de creación y datos renderizados por el servidor.
- **Chat principal:** historial, conversación y contexto forman un workspace propio. Los mensajes usan filas legibles en lugar de tarjetas anidadas; fuentes, acciones de estudio, modelos/cuota, adjuntos, dictado y composer mantienen sus contratos. En móvil el historial es un cajón y las acciones tienen scroll interno.
- **Conversaciones:** el workspace de `/ai` conserva su adaptación y contratos, pero se integra visualmente con el mismo sistema `nx-*`.
- **Conocimiento:** Material y Notas tienen composiciones independientes. Notas combina biblioteca, título, etiquetas, Markdown, edición/dividido/lectura, backlinks y exportación. En móvil inicia en edición, pero Lectura y Dividido siguen disponibles.
- **Académico:** asignaturas, detalle y actividades, planificador, calendario mensual/diario/semanal, evaluaciones, horario, vida escolar, búsqueda y perfil usan secciones, métricas y estados adaptados a sus datos. Horario abre por defecto el horario personal 2026–2027 de Unidad Educativa Álamos y conserva Mikareno como fuente opcional.
- **Oficina:** se conserva el workspace inspirado en GenOffice: pestañas, ribbon, lienzo y panel de herramientas; inicio unificado, Biblioteca, documentos Markdown, hojas con barra `fx` y CSV, presentaciones con miniaturas, edición, orden y modo Presentar.
- **Sistema:** MCP separa servicios, enrutador, Playground y telemetría de solicitudes/gasto/presupuesto. Administración y Configuración reorganizan métricas, backups, conexiones y parámetros sin exponer secretos.
- **Autenticación:** acceso y cambio de contraseña comparten identidad visual y conservan formularios, errores y controles de visibilidad.
- **Responsive y accesibilidad:** skip-link, foco visible, nombres accesibles, `aria-expanded`/`aria-current`, movimiento reducido e impresión. Las tablas y rejillas anchas usan scroll interno solo cuando hace falta.

## Contratos preservados

- `NAV_LINKS` sigue siendo la fuente canónica de **22 destinos**; cada ruta tiene como máximo un `aria-current="page"`.
- El panel conserva sus **17 slots** y el shell los slots de avatar, nombre, rol, estado, navegación MCP y porcentaje del Vault.
- `dashboard.html` e `index.html` son copias exactas y conservan los IDs del menú de creación, captura rápida y formularios.
- Permanecen IDs, formularios, atributos HTMX y endpoints de chat, streaming/fallback, historial, fuentes, modelos/cuotas, adjuntos/dictado, notas, MCP, calendario, Oficina, administración y ajustes.
- El historial permite seleccionar, filtrar y cerrar el panel sin borrar mensajes. Las sugerencias rellenan el composer sin enviar automáticamente.
- El streaming incrementa el contador solo después de una respuesta correcta; una conversación nueva reinicia ese contador.
- `/login` conserva `#login-container`, `#login-error`, campos, `method`/`action`, HTMX, redirección y fragmento 401. El cambio de contraseña conserva sus campos y tres controles de visibilidad.
- La versión pública de `shell.js` permanece en `3.0.0` y la versión visible en `3.0`.

## Formatos y capacidades reales

Oficina guarda y exporta **Markdown, CSV y JSON**. La integración GenOffice MCP puede generar el informe DOCX previsto por el backend. No se declara compatibilidad nativa DOCX/XLSX/PPTX/PDF ni edición automática que el servidor no implementa. El enlace del panel de herramientas a NOVA AI prepara contexto; no simula una edición realizada.

## Archivos principales

| Archivo | Responsabilidad |
|---|---|
| `static/stitch-v2/shell/head.html` | Tokens, fuentes, Tailwind y carga ordenada de `/polish.css` y `/nova.css` |
| `static/stitch-v2/shell/aside.html` | Rail/cajón, 22 accesos, sistema y Vault |
| `static/stitch-v2/shell/topbar.html` | Contexto, búsqueda, creación, notificaciones y cuenta |
| `static/stitch-v2/_content/clean/` | Panel, Material, Asistente, Notas, MCP y Ajustes |
| `static/app/nova.css` | Sistema Nébula, páginas, chat, Agenda, responsive y GenOffice |
| `static/app/polish.css` | Reset, tokens compartidos, diálogos y autenticación |
| `static/app/shell.js` | Interactividad existente, Notas y estilos dinámicos compatibles |
| `src/handlers/stitch_v2.rs` | Shell, navegación activa, rail/cajón y comportamiento accesible |
| `src/handlers/stitch_chat.rs` | Chat principal, historial, acciones y streaming |
| `src/ai/handlers.rs` | Workspace de conversaciones y contratos JavaScript |
| `src/academic/` | Pantallas académicas, calendario, planificador, evaluaciones y perfil |
| `src/office/` | Documentos, hojas, presentaciones, Biblioteca y presentación |
| `design/ui-preview/` | Preview aislada, fixtures y auditoría reproducible en Chromium |

## Preview local

La preview compone las plantillas y fragmentos Rust reales. `pages.mjs` resuelve slots y argumentos de `format!` con datos ficticios; `server.mjs` añade shell, estado activo derivado de `NAV_LINKS`, HTMX local, assets y respuestas API ficticias. Las páginas de autenticación se extraen directamente de `auth.rs`.

Para el chat principal, `pages.mjs` extrae el `STREAM_SCRIPT` real de `stitch_chat.rs` y `server.mjs` simula `/api/ai/chat/stream` con eventos SSE `delta`/`done`. Esto permite probar el JavaScript de producción sin afirmar que se ejercita el backend Rust.

```bash
cd design/ui-preview
npm ci
node server.mjs
```

El servidor escucha en `0.0.0.0:3000`. Desde la raíz del repositorio, en otra terminal:

```bash
NOVA_SCREENSHOTS=all node design/ui-preview/capture.mjs
```

La auditoría cubre **34 rutas del shell más 2 de autenticación**, en escritorio y móvil, y **28 grupos de interacciones**. El alias `/` no se duplica. Comprueba 22 accesos, activo único, overflow global, slots, IDs duplicados, recursos externos, respuestas fallidas, versión, formularios y errores de consola. `NOVA_SCREENSHOTS=all` guarda las 72 capturas de rutas en `/tmp`.

Para una pasada focalizada:

```bash
NOVA_ROUTES=/dashboard/notes,/office/library,/login \
NOVA_VIEWPORTS=mobile \
NOVA_FULL_PAGE=0 \
NOVA_SCREENSHOTS=all \
NOVA_INTERACTIONS=0 \
node design/ui-preview/capture.mjs
```

`NOVA_ROUTES` filtra las capturas estructurales; `NOVA_VIEWPORTS` acepta `desktop`/`mobile`, `NOVA_FULL_PAGE=0` limita la captura al viewport y `NOVA_INTERACTIONS=0` omite las interacciones. Para ejecutar solamente estas últimas se usa `NOVA_ROUTES=/__interactions_only__`. Reiniciar la preview tras cambiar plantillas o Rust; el CSS se relee por petición.

## Interacciones automatizadas

1. Acceso: visibilidad de contraseña y 401 con error visible.
2. Cambio de contraseña: tres controles de visibilidad.
3. Shell: rail, menús, modal, Escape y bloqueo/restauración del scroll.
4. Cajón móvil del shell.
5. Fuentes del Asistente.
6. Chat principal: streaming real del script contra SSE ficticio, sin envíos duplicados.
7. Chat principal: acciones especializadas y contexto de la respuesta.
8. Historial de Conversaciones.
9. Envío de chat sin perder el historial.
10. Acciones de una respuesta.
11. Notas: hidratación, modos y selector real de importación `.md/.markdown`.
12. MCP: secreto, ping y Playground.
13. Inicio/listados de Oficina: pestañas, creación, recientes, búsqueda, exportación y borrado.
14. Panel adaptable de herramientas de Oficina.
15. Cuadrícula: selección, `fx`, filas/columnas, renumeración y CSV.
16. Toolbar Markdown, guardado y error 500 visible.
17. Selección y edición de diapositivas.
18. Orden de diapositivas mediante el `POST` y su campo `order` real.
19. Modo Presentar: botones, teclado, límites y una sola escena visible.
20. Biblioteca: apertura, descarga e importación.
21. Cajón móvil de herramientas de Oficina.
22. Partial HTMX de Calendario.
23. Acordeones de Vida escolar.
24. Navegación semanal sin solapamientos.
25. Horario personal: 50 bloques, tutoría/club/recreo, metadatos y fuente Mikareno.
26. Chat móvil: historial, búsqueda, sugerencias y composer visible.
27. Notas móvil: título contenido y modos Lectura/Dividido funcionales.
28. Biblioteca móvil: archivos como tarjetas etiquetadas y contenidas.

El 401 del acceso es deliberado: la prueba permite únicamente ese estado y sus mensajes de consola asociados. Cualquier otra respuesta o error sigue contando como fallo.

## Validación realizada

Resultado de la pasada integral de Nébula del **2026-10-24** contra la preview y sus fixtures:

- **100 comprobaciones y 0 fallos** en Chromium: 72 estructurales y 28 de interacción.
- **36 rutas en 1440 × 1000 y las mismas 36 en 390 × 844**, con capturas de todas ellas.
- Revisión visual de Panel, chat, Notas, Material, asignaturas, agenda, evaluaciones, horario, perfil, MCP, Administración, Configuración, Oficina, autenticación y subpáginas.
- Sin overflow global, errores JavaScript inesperados, respuestas fallidas inesperadas, slots pendientes, IDs duplicados ni recursos externos en la preview.
- Streaming del chat principal con una sola petición por envío, historial conservado y acciones especializadas verificadas.
- `dashboard.html` e `index.html` sincronizados byte a byte.
- `node --check` de `shell.js`, `pages.mjs`, `server.mjs` y `capture.mjs`, además de `git diff --check`, sin errores.
- Parser Lezer sin errores de sintaxis en los 14 archivos Rust revisados; esto no equivale a comprobación de tipos ni compilación.

## Verificación pendiente por entorno

El entorno de edición no incluye `cargo` ni `rustc`; por tanto, no se afirma que el proyecto haya compilado ni que la suite Rust se haya ejecutado. Con el toolchain disponible, ejecutar:

```bash
cargo check
cargo test
cargo build
cargo fmt --check
cargo clippy --all-targets
```

La preview no sustituye pruebas contra Rust/SQLite ni una verificación real de streaming/fallback, persistencia, dictado o servicios MCP externos. `tests/office_integration.rs` incluye ciclos reales de crear, abrir, actualizar, exportar/importar, presentar, reordenar y borrar; esas pruebas quedan pendientes de ejecutarse con el toolchain.

Las plantillas y assets se incrustan con `include_str!`/`include_bytes!`: para servir estos cambios en producción hay que recompilar el backend. Este rediseño no incluye commit, push ni despliegue.
