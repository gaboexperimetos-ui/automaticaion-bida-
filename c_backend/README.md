# Backend C — migración incremental

**Vigencia: 2026-10-31. Fase 1 de 7; no es todavía un reemplazo completo.**

Este directorio contiene un backend **C17/POSIX real**: no ejecuta ni embebe el
binario Rust. El backend Rust/Axum sigue siendo la referencia funcional y el
arranque de producción no cambia durante la migración.

## Compilar y probar

Requisitos en Linux: GCC con C17 y `-fanalyzer`, GNU Make, Python 3 y el runtime
`libsqlite3.so.0`. No se instalan paquetes ni se requieren cabeceras `sqlite3-dev`.

```sh
make -C c_backend all       # binario normal
make -C c_backend test      # ASan + UBSan + pruebas unitarias e HTTP
make -C c_backend analyze   # GCC -fanalyzer
make -C c_backend ci        # todas las anteriores validaciones de CI
```

Los artefactos quedan en `c_backend/build/`, ignorados por Git.

## Ejecutar en desarrollo aislado

Usa una base temporal y la raíz del repositorio para los assets. **No apuntes
este binario a la base de producción.**

```sh
SESSION_SECRET='desarrollo-local-c-0123456789abcdef0123456789' \
DB_PATH=/tmp/study-platform-c/app.db \
SP_STATIC_ROOT="$PWD" \
./c_backend/build/study-platform-c --addr 127.0.0.1:18090
```

`SP_STATIC_ROOT` es específico del backend C y selecciona la raíz desde la que
se leen los assets. `C_WORKERS` o `--workers` fija entre 1 y 64 workers.

## Implementado en esta fase

- Configuración de entorno y `.env`, con los valores por defecto del backend
  Rust, validación de `SESSION_SECRET`, enteros y presupuesto.
- CLI de servidor: `--addr`, `--workers`, `--help` y `--version`.
- Apertura SQLite con `FULLMUTEX`, `busy_timeout`, claves foráneas, caché
  acotada, WAL autocheckpoint y `SELECT 1` real para health.
- Servidor HTTP con pool acotado, IP del socket, timeouts, límites de headers y
  body, rechazo de `Transfer-Encoding`, rechazo de `Content-Length`
  contradictorios y cierre sin perder respuestas de error.
- Sondeos MCP `initialize` por HTTP, timeout de 2,5 s y caché de 30 s.
- Once contratos GET públicos:
  - `/health`, `/api/config`, `/api/public-config`;
  - `/legacy`, `/styles.css`, `/app.js`, `/shell.js`, `/polish.css`,
    `/nova.css`, `/favicon.ico`, `/favicon.svg`.

## Límites explícitos

- No hay autenticación, rate limiting, migraciones SQL, rutas protegidas, notas,
  búsqueda, IA, SSE, Office, backups ni MCP servidor todavía.
- Los sondeos MCP por HTTPS devuelven `false`: TLS se aborda con la fase de
  integraciones. Los endpoints HTTP usados por Mikareno/GenOffice sí se sondean.
- `--migrate` y `--create-admin` **fallan de forma explícita** antes de tocar la
  BD; nunca delegan ocultamente en Rust.
- El servidor C es opt-in y no está autorizado para producción ni para exposición
  a red. No conserva aún todos los controles de seguridad del backend Rust.

El inventario, las fases y los criterios para eliminar Rust están en
[`../docs/C_MIGRATION.md`](../docs/C_MIGRATION.md).
