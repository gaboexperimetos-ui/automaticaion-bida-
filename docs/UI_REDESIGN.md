# Nova 3.0 — rediseño de Study Platform

> Documento vivo. Actualizado: **2026-10-24**.

## Objetivo y alcance

Nova 3.0 sustituye la composición anterior por un espacio académico personal, oscuro e inmersivo. No es una variación de la barra lateral clásica: la navegación se convierte en un mapa de pantalla completa, la cabecera flota sobre el contenido y el panel adopta una jerarquía editorial con acentos violeta y cian.

El requisito rígido es conservar las funcionalidades existentes. Se reorganiza la presentación, pero no se eliminan destinos, controles activos, endpoints ni contratos de datos. El backend Rust/Axum, SQLite, HTMX y el renderizado de HTML en servidor se mantienen; no se introduce React, Vite ni un segundo frontend de producción.

## Dirección visual y de interacción

- **Cabecera flotante:** identidad `study / platform`, subtítulo `personal academic OS`, versión visible `v3.0`, buscador, creación, estado de conexiones, notificaciones y cuenta.
- **Navegación inmersiva:** los 22 destinos aparecen en un menú superpuesto a pantalla completa. Estudios, Inteligencia artificial y Conocimiento se muestran como colecciones; Oficina e Integraciones conservan sus grupos plegables. Administración, Configuración y Vault siguen disponibles.
- **Panel editorial:** saludo de gran formato, estado de Mikareno, semana, jornada, copiloto, prioridades y tarjetas de apoyo para conocimiento, próximo examen y promedios.
- **Sistema Nova:** superficies oscuras translúcidas, bordes finos, tipografía Inter/Manrope, iconos Material Symbols, iluminación ambiental y acentos violeta/cian. `/nova.css` se carga después de `/polish.css`.
- **Responsive:** una sola columna en móvil, tabla con desplazamiento interno y menú con scroll propio. Vault queda después de todo el contenido desplazable, sin superponerse a los accesos.
- **Accesibilidad:** skip-link, foco visible, `aria-expanded`, cierre con Escape, bloqueo del scroll del documento mientras el menú está abierto, etiquetas accesibles y controles con nombres explícitos.

## Contratos preservados

- `NAV_LINKS` sigue siendo la fuente canónica de **22 destinos**; el shell conserva el mismo conjunto de `data-path` y un único `aria-current="page"`.
- El panel conserva sus **17 slots** de contenido y el shell sus slots `SHELL_AVATAR`, `SHELL_NAME`, `SHELL_ROLE`, `SHELL_STATUS`, `SHELL_NAV_MCP` y `SHELL_VAULT_PCT`.
- `dashboard.html` e `index.html` tienen el mismo contenido activo.
- Se mantienen los identificadores usados por `shell.js`: `toggle-create-menu`, `quick-create-btn`, `create-modal-popover`, `quick-task-modal`, sus botones de cierre y los campos de la tarea rápida.
- Las acciones de creación, búsqueda, notificaciones, cuenta y guardado como nota siguen usando los endpoints y la lógica existentes.
- Los enlaces destacados apuntan a rutas reales (`/dashboard/planner`, `/dashboard/calendar`, `/dashboard/notes` y `/dashboard/mcp`).
- La versión de la API pública del shell permanece en `3.0.0`.

## Archivos principales

| Archivo | Responsabilidad |
|---|---|
| `static/stitch-v2/shell/head.html` | Tokens, fuentes, estilos base y carga ordenada de `/polish.css` y `/nova.css` |
| `static/stitch-v2/shell/aside.html` | Mapa de navegación inmersivo, grupos, accesos de sistema y Vault |
| `static/stitch-v2/shell/topbar.html` | Cabecera flotante, buscador, acciones y estructura de cuenta |
| `static/stitch-v2/_content/clean/dashboard.html` | Panel principal Nova |
| `static/stitch-v2/_content/clean/index.html` | Copia activa equivalente del panel |
| `static/app/nova.css` | Sistema visual Nova, composición, animaciones y responsive |
| `static/app/polish.css` | Estilos compartidos y capas anteriores, cargados antes de Nova |
| `static/app/shell.js` | Interactividad y llamadas API existentes |
| `src/handlers/stitch_v2.rs` | Composición del shell, navegación activa, cuenta y comportamiento accesible del menú |
| `src/handlers/stitch_panel.rs` | Datos reales/degradados y relleno de slots |
| `src/handlers/static_ui.rs` | Servicio embebido de `/shell.js`, `/polish.css` y `/nova.css` |
| `src/handlers/mod.rs` | Registro de rutas públicas de los assets del shell |
| `design/ui-preview/` | Preview aislada y auditoría reproducible en Chromium |

## Preview local

La preview compone las plantillas y los fragmentos Rust reales, pero utiliza datos ficticios definidos únicamente en `design/ui-preview/server.mjs`. No modifica la base de datos ni sustituye los datos de producción de `stitch_panel.rs`.

```bash
cd design/ui-preview
npm ci
node server.mjs
```

El servidor escucha en `0.0.0.0:3000`. En otra terminal, desde la raíz del repositorio:

```bash
node design/ui-preview/capture.mjs
```

La auditoría genera capturas de escritorio y móvil y comprueba navegación, creación, modal, cuenta, overflow, slots, IDs duplicados y errores de consola. Debe reiniciarse la preview tras cambiar plantillas o Rust; `nova.css` se lee de nuevo en cada petición.

## Validación realizada

Sobre la página compuesta con el shell actual y el menú de cuenta generado desde `stitch_v2.rs`:

- Chromium en **1440 × 1000** y **390 × 844**, con ancho del documento igual al viewport y sin errores de consola o JavaScript.
- 22 destinos, 17 slots sustituidos y ausencia de IDs duplicados.
- Menú inmersivo abierto y cerrado en ambos anchos, Escape funcional, `aria-expanded` correcto y scroll del body bloqueado/restaurado.
- Vault colocado después de los accesos en móvil, sin superposición.
- Menú de creación, modal de tarea y menú de cuenta visibles y operativos.
- Cabecera flotante a 16 px del borde en escritorio y contenido principal sin el padding del layout anterior.
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
