# AGENTS.md — Instrucciones para agentes de programación

## Modelo constructor
- **Modelo:** `poolside/laguna-s-2.1:free`
- **No cambiar de modelo** ni consumir créditos de pago sin autorización explícita.
- El modelo se usa exclusivamente para desarrollo. OpenRouter (con su propio budget) se usa para la funcionalidad de asistente académico de la aplicación.

## Lenguaje y estilo
- Backend: Rust (edition 2021, toolchain stable).
- Frontend: HTMX + Tailwind CSS. Server-side rendered. JS mínimo (solo para SSE/streaming).
- Código: inglés para identifiers y comentarios. Mensajes de usuario: español.
- Tests: Rust `#[cfg(test)]` + `cargo test`. Integration tests con `axum::test` y `sqlite` en memoria.

## Arquitectura
- Compilar en `asus-tuf` (x86_64, glibc 2.44), deployar binario **musl static** a `gabo-Inspiron-5720` (x86_64, Ubuntu 26.04, glibc 2.43).
- Build: `cargo build --release --target x86_64-unknown-linux-musl RUSTFLAGS='-C link-arg=-s'`
- SQLite single-file (`/home/gabo/study-platform/data/app.db`).
- MCP cliente SSE → `http://127.0.0.1:8080/mcp` (Mikareno, localhost en remoto).
- MCP propio (rmcp) → `http://127.0.0.1:8081/mcp`.
- OpenRouter proxy → API key en `OPENROUTER_API_KEY` (env var, no en source).
- GenOffice MCP → `http://127.0.0.1:8453/mcp` (ya corre en remoto).

## Seguridad
- **NUNCA** incluir secretos en código fuente.
- **NUNCA** loggear credenciales, tokens o datos académicos privados.
- Configuración vía archivo `.env` (gitignored).
- No exponer puertos a Internet. App en `127.0.0.1:8090` (localhost del remoto).
- MCP propio: autenticación por API key (`X-API-Key` header), localhost only.
- Confirmar antes de operaciones destructivas.
- OpenRouter API key: SEPARADA de OpenCode (no copiar desde service.json).

## Restricciones del entorno
- No instalar paquetes del sistema sin autorización.
- No modificar firewall, Tailscale, o MCP de GenOffice existente.
- No modificar OpenCode config ni cambiar el modelo.
- Respetar el límite de `$2 USD` en OpenRouter.
- El remote Ubuntu tiene recursos limitados: 7.1GB RAM, 4 cores (i5-3210M, load avg ~9.5). Usar límites de recursos.

## Workflow de implementación
1. Diseñar contrato del módulo (interface, structs).
2. Escribir código.
3. `cargo check` — verificación estática.
4. `cargo test` — tests unitarios.
5. `cargo build` — build exitoso.
6. Test de integración (endpoint, DB, MCP handshake).
7. Documentar limitaciones y resultados.
8. Marcar como terminado SOLO si pasa todos los criterios de aceptación.

## Prioridad
1. Backend funcionando (API + SQLite + MCP client)
2. MCP cliente de Mikareno (read-only)
3. Frontend básico (dashboard académico)
4. Asistente OpenRouter
5. Second brain (notes + search)
6. Suite de oficina (proxy a GenOffice MCP)
7. MCP server propio
8. Admin / backups / monitoring

## Debugging
- `journalctl --user -u mikareno-mcp` — logs del MCP
- `ss -tlnp | grep 8080` — verificar puerto MCP
- `free -m && cat /proc/loadavg` — recursos del remoto
- No asumir que algo funciona: verificar con `curl` o test directo.
