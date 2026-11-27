# Changelog

All notable changes to this project are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/).

## [1.1.3] - 2026-11-27 — Entrega comprimida

### Changed
- El servidor comprime con gzip los estáticos de texto (CSS, JS y SVG) y las
  respuestas HTML/JSON de más de 1 KB cuando el cliente lo acepta. En una
  carga del panel, CSS+JS bajan de ~602 KB a ~112 KB y el HTML de 71,8 KB a
  13,6 KB. Los estáticos se comprimen una sola vez al arrancar y se cachean en
  memoria; las fuentes WOFF2 no se recomprimen.

## [1.1.2] - 2026-11-27 — Escritorio HTML5 exacto

### Changed
- La app de escritorio vuelve a GTK3 + WebKit2GTK (`zig build desktop`): abre
  la misma interfaz Nébula server-rendered, idéntica a la web, en una ventana
  propia sin navegador ni pestañas. Se retira el prototipo de widgets nativos,
  que no reproducía el diseño real.
- Transparencia ligera por defecto: ventana RGBA, superficies translúcidas,
  aurora estática y cero `backdrop-filter` (el desenfoque era lo que saturaba
  la GPU). `NOVA_GLASS=full` activa el vidrio esmerilado con blur como modo
  pesado opcional.
- Rendimiento: se elimina Tailwind Play CDN (compilador JS en runtime) por un
  `tailwind.css` estático generado en el build, y las 5 familias de Google
  Fonts + Material Symbols pasan a WOFF2 locales embebidos en el binario. La
  página ya no hace peticiones externas al cargar.
- El tamaño de la ventana se recuerda en `~/.config/nova/window.conf`; la
  posición la decide el compositor en Wayland y abre en el workspace activo.

### Fixed
- La versión del build, `/health` y `serverInfo` del MCP coinciden en 1.1.2.

## [1.1.0] - 2026-11-26 — NOVA 1.1

### Added
- Bandeja unificada `/dashboard/tasks`: Mikareno, Google Classroom, notas
  propias y avisos de WhatsApp en una sola lista, con deduplicación por
  título y fecha, filtro por origen y caché de Classroom de 10 minutos.
- Los avisos de WhatsApp se muestran con remitente y fecha; se descartan los
  mensajes sin contenido académico y las notas índice del vault.
- MCP propio: herramientas `task_create`, `task_complete` y `task_list` para
  crear, cerrar y listar tareas desde Jarvis; `serverInfo` ahora 1.1.0.
- Audio `/dashboard/audio`: grabar o subir audio, convertir con ffmpeg y
  transcribir con la API de Groq (Whisper); el texto se guarda como nota.
  El tope de cuerpo sube a 25 MB sólo para la ruta de audio.

## [1.0.0] - 2026-11-26 — NOVA 1.0

Primera versión estable de la aplicación académica autoalojada.

### Added
- Importadores en Zig para DOCX, ODT, PDF, XLSX, ODS, PPTX, ODP, HTML, MD,
  TXT y CSV hacia la biblioteca Office.
- Importación local de `~/Documentos` y bajo demanda desde Google Drive, con
  deduplicación por contenido.
- Google Classroom: cursos, trabajos y anuncios en solo lectura, y entregas
  próximas en el planificador mediante HTMX con caché de 10 minutos.
- Gráficos de la ficha de materia (anillo del promedio, barras por bloque
  ponderado y reparto calificadas/pendientes) en SVG/CSS puro.
- App de escritorio GTK3 + WebKit2GTK con ventana RGBA y vidrio esmerilado
  (`zig build desktop`).
- Google Drive, Google Calendar y Classroom enlazados desde la navegación y
  los ajustes.

### Changed
- Interfaz NOVA con sidebar de vidrio, temas y navegación revisada.
- El planificador ya no bloquea la carga esperando a Google.

### Notas
- Se incorpora al historial la rama antigua `ui/polish-study-platform`. Sus
  cambios de interfaz y traducción preceden al rediseño NOVA 3.0 actual, por lo
  que la fusión conserva los archivos de `main` para evitar recuperar una UI
  obsoleta o alterar controles y lógica actuales. Los ocho commits originales
  siguen disponibles en el historial para consultar sus diferencias.

## [0.1.0] - 2026-10-09 — Planning Phase

### Added
- Project structure initialized with documentation:
  - `AGENTS.md` — permanent instructions for programming agents
  - `README.md` — project purpose, installation, configuration
  - `docs/REQUIREMENTS.md` — functional and non-functional requirements
  - `docs/ARCHITECTURE.md` — architecture, components, data flows
  - `docs/DECISIONS.md` — architectural decision records (ADRs)
  - `docs/SECURITY.md` — security model, threats, credential handling
  - `docs/TESTING.md` — testing strategy and acceptance criteria
  - `docs/DEPLOYMENT.md` — build, deploy, recovery procedures
  - `docs/ROADMAP.md` — development phases and dependencies
  - `CHANGELOG.md` — this file

### Corrected
- **Critical:** Mission 3 report incorrectly stated MCP files exist on remote
  server. Verified via SSH that `/home/gabo/mikareno-mcp/` does NOT exist on
  `gabo-Inspiron-5720`. The MCP only runs on `asus-tuf`.
- **Systemd CPUQuota semantics:** `CPUQuota=40%` means 40% of ONE CPU core
  (10% of 4 cores), not 40% of all CPUs. Corrected recommendation to
  `CPUQuota=200%` (50% of 4 cores) or removing the limit in favor of `Nice=10`.
- **venv portability:** Python venvs are not portable across distros (glibc
  differences). Must recreate venv on remote with `requirements.txt`, not rsync.
- **systemd --user on remote:** No `systemd --user` session running for `gabo`
  on the remote. Requires `loginctl enable-linger gabo` or system-level service.

### Key findings from missions 0-3
- Local (asus-tuf): MCP active (PID 126524), RSS ~96 MB, port 8080 on 0.0.0.0
- Remote (Ubuntu): No MCP installed. GenOffice MCP active on port 8453. 710 MB free RAM, load avg 9.46/4 cores.
- Tailscale mesh: `100.115.62.97` (local) ↔ `100.92.172.48` (remote)
- Model: `poolside/laguna-s-2.1:free` for development; `cohere/north-mini-code:free` for application
