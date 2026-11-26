# Study Platform — NOVA

Plataforma web personal de productividad académica con el frontend server-rendered **Stitch v2 / NOVA** (HTML, CSS y HTMX/JavaScript) servido directamente por un backend Zig, SQLite y notas Markdown.

> **Estado de la migración (2026-11-13).** La implementación del backend en este checkout es Zig; las fuentes Rust y C del repositorio fueron retiradas. El frontend existente y las migraciones SQL se conservaron. El binario y la suite HTTP se verificaron en un preview local de Arena. **No se ha desplegado este binario a producción.** La paridad funcional todavía es parcial: las conexiones a Mikareno/GenOffice, las páginas de datos académicos y las operaciones de la suite de oficina siguen en modo de disponibilidad/no implementadas; el endpoint SSE conserva el formato, pero la respuesta actual se entrega después de generar el contenido, no como tokens en vivo. Ver [estado y límites](docs/ARCHITECTURE.md#estado-de-paridad).

## Qué funciona en el port Zig

- CLI de servicio, migraciones SQLite embebidas, `/health`, configuración pública y shell Stitch/NOVA.
- Autenticación con sesiones, Argon2id, registro por invitación opcional, cookies `HttpOnly` y rate limiting.
- CRUD de notas Markdown, contenido en disco, versiones recientes, restauración, exportación, búsqueda FTS5 con fallback `LIKE`.
- Anotaciones de calendario locales, CRUD JSON y exportación iCalendar.
- API de chat/generación con modo simulado si no hay clave de OpenRouter, selección persistida de modelo, historial por materia, contador de costes y respuesta SSE compatible.
- Servidor MCP JSON-RPC con nueve herramientas y autenticación obligatoria por `CUSTOM_MCP_API_KEY`.
- Backups SQLite manuales, integridad antes de restaurar, descarga, restauración en la conexión activa y retención de cinco copias.
- Páginas NOVA de dashboard, asistente, notas, MCP, configuración, administración y placeholders honestos para áreas aún no portadas.

La matriz de rutas y los códigos de respuesta está en [docs/API.md](docs/API.md). Las funciones que dependen de servicios externos no se simulan como datos reales.

## Arquitectura

```text
Navegador
  └─ HTML/CSS/HTMX/JS Stitch v2 · estáticos embebidos en el ejecutable
       └─ servidor HTTP + aplicación Zig (src/*.zig)
            ├─ SQLite del sistema, WAL, FTS5 y migraciones SQL embebidas
            ├─ notas Markdown en NOTES_DIR
            ├─ OpenRouter opcional (modo simulado si no hay clave)
            └─ API MCP propia (X-API-Key)
```

No quedan fuentes de implementación Rust o C en el repositorio. Zig enlaza con libc para sockets y usa la biblioteca compartida SQLite del sistema mediante ABI; SQLite es una dependencia de runtime, no código C del proyecto.

## Requisitos

- Zig **0.14.1**.
- SQLite 3 con FTS5. En Debian/Ubuntu: `sudo apt-get install libsqlite3-dev` para compilar y `libsqlite3-0` para ejecutar.
- Python 3 sólo para la prueba HTTP de integración (usa la biblioteca estándar).

## Build, tests y ejecución local

```bash
zig fmt --check build.zig main.zig test_root.zig src/*.zig desktop/main.zig
zig build
zig build test --summary all
python3 tests/http_smoke.py --binary zig-out/bin/study-platform

# Perfil optimizado para un binario de release:
zig build -Doptimize=ReleaseFast
```

### App de escritorio (opcional)

La misma interfaz server-rendered puede abrirse en una ventana nativa con GTK3 + WebKit2GTK, sin Electron ni un segundo backend:

```bash
zig build desktop
./desktop/install.sh   # copia el binario y crea el lanzador en el menú
```

`desktop/install.sh` instala `~/.local/bin/nova-desktop`, el icono y `~/.local/share/applications/nova.desktop`. Para compilar hacen falta las bibliotecas de desarrollo `gtk+-3.0` y `webkit2gtk-4.1`; el resto del servidor no las necesita.

El lanzador abre `http://127.0.0.1:3100/dashboard` por defecto (el puerto del servicio local). Para otro destino:

```bash
STUDY_URL=http://127.0.0.1:8090/dashboard zig-out/bin/nova-desktop
```

La app no arranca el servidor por su cuenta: si `study-platform` no está activo, WebKit muestra su página de error. Atajos: F5 o Ctrl+R recargar, Ctrl+H inicio, F12 herramientas de desarrollo, Ctrl+Q cerrar.

Crea `.env` a partir de `.env.example`, genera un `SESSION_SECRET` aleatorio de al menos 32 caracteres y arranca:

```bash
cp .env.example .env
# Edita .env; no uses AUTH_DISABLED=true fuera de desarrollo aislado.
zig build run -- --addr 0.0.0.0:8090
```

El servidor usa `0.0.0.0:8090` por defecto. `--addr` controla el bind; `HOST` y `PORT` son valores informativos. El proceso crea `DB_PATH` y aplica las migraciones al arrancar. La base y las notas deben mantenerse fuera de Git.

```bash
zig-out/bin/study-platform --help
zig-out/bin/study-platform --version
zig-out/bin/study-platform --migrate
zig-out/bin/study-platform --create-admin admin
```

`--create-admin` crea o restablece el usuario pero no revela una contraseña temporal. En una base de desarrollo, arranca con `AUTH_DISABLED=true` y establece una contraseña desde `/settings/password`; después vuelve a `AUTH_DISABLED=false`.

## Variables principales

| Variable | Default | Descripción |
|---|---|---|
| `SESSION_SECRET` | obligatorio, ≥32 chars | Secreto para derivar hashes de sesión. |
| `DB_PATH` | `./data/app.db` | Archivo SQLite. |
| `NOTES_DIR` | `./notes` | Bóveda de notas Markdown. |
| `AUTH_DISABLED` | `false` | Desactiva toda la protección; sólo para desarrollo local. |
| `REGISTRATION_TOKEN` | vacío | Invitación requerida por `POST /register`; vacío deshabilita registro. |
| `OPENROUTER_API_KEY` | vacío | Sin clave, chat y generación responden en modo simulado y no llaman al proveedor. |
| `OPENROUTER_MODEL` | `cohere/north-mini-code:free` | Modelo inicial; el selector puede persistir otro modelo en SQLite. |
| `OPENROUTER_MAX_TOKENS` | `8192` | Máximo de tokens por petición. |
| `OPENROUTER_MAX_COST_USD` | `2.0` | Límite mensual aplicado a uso registrado por la aplicación. |
| `MIKARENO_MCP_URL` | `http://127.0.0.1:8080/mcp` | Se muestra en configuración; el cliente Zig de Mikareno aún no está implementado. |
| `GENOFFICE_MCP_URL` | `http://127.0.0.1:8453/mcp` | Se muestra en configuración; el cliente Zig de GenOffice aún no está implementado. |
| `CUSTOM_MCP_API_KEY` | vacío | Sin clave, el servidor MCP propio queda deshabilitado y responde 401. |
| `DB_MAX_CONNECTIONS` | `4` | Reservado; actualmente se utiliza una conexión SQLite `FULLMUTEX`. |
| `MCP_TIMEOUT_MS` | `5000` | Configuración reservada para integración MCP. |

La plantilla completa y comentada está en [.env.example](.env.example).

## Datos persistentes y seguridad

- SQLite mantiene usuarios, sesiones, notas, versiones, anotaciones, chat, costes y configuración. Las ocho migraciones originales se aplican desde Zig.
- Las notas se escriben como `.md` en `NOTES_DIR`; las rutas de archivo se derivan de slugs validados.
- Los backups se guardan junto al directorio de la base, en `backups/`; las restauraciones validan nombre e `integrity_check` antes de usar SQLite Backup API.
- `AUTH_DISABLED=true` permite que cualquier cliente que alcance el puerto use todas las rutas protegidas. No se debe exponer así.
- `/mcp` no usa la cookie web: configura un `CUSTOM_MCP_API_KEY` largo y privado.
- No incluyas claves reales en `.env.example`, el frontend, logs, tests o commits.

## CI

- [Zig validation](.github/workflows/zig-validation.yml): formato, build, tests Zig y smoke HTTP.
- [Preview binary](.github/workflows/preview-backend.yml): build ReleaseFast y prerelease temporal para Arena; no despliega a producción.

Consulta [docs/INDEX.md](docs/INDEX.md) para el resto de la documentación. Las instrucciones de desarrollo están en [docs/DEVELOPER_SETUP.md](docs/DEVELOPER_SETUP.md); los límites actuales están enumerados en [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
