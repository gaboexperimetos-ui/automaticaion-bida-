# Arquitectura

## Visión general

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           REMOTO (Ubuntu 26.04)                          │
│                    gabo-Inspiron-5720 (x86_64, 4 cores)                 │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌─────────────┐    HTTP SSE      ┌────────────────┐                   │
│  │  Mikareno   │◄─────────────────│                │                   │
│  │  MCP        │ 127.0.0.1:8080   │  Rust Backend  │                   │
│  │ (Python)    │                  │  (Axum)        │                   │
│  └─────────────┘                  │  :8090          │                   │
│                                    └───────┬────────┘                   │
│                                            │                            │
│  ┌─────────────┐    HTTP SSE      ┌────────┴────────┐  ┌────────────┐ │
│  │  GenOffice  │◄─────────────────│  Office Suite   │  │  Custom    │ │
│  │  MCP        │ 127.0.0.1:8453   │  Proxy          │  │  MCP      │ │
│  │ (preinstalled)│                 │                │  │  Server   │ │
│  └─────────────┘                  └────────┬────────┘  │  :8081    │ │
│                                            │  REST     └────────────┘ │
│  ┌─────────────┐    HTTPS         ┌───────┴────────┐                 │
│  │  OpenRouter │◄─────────────────│  AI Assistant  │                 │
│  │  API        │ openrouter.ai    │  Proxy          │                 │
│  └─────────────┘                  └────────┬────────┘                 │
│                                            │                           │
│  ┌─────────────┐    File I/O      ┌────────┴────────┐  ┌────────────┐ │
│  │  SQLite DB  │◄─────────────────│  Core App       │  │  Markdown  │ │
│  │  app.db      │                 │  Logic          │◄─│  Notes     │ │
│  └─────────────┘                  └─────────────────┘  │  /notes/   │ │
│                                                       └──┴──────────┘ │
│                                                                          │
│  ┌─────────────┐                                                        │
│  │  systemd    │  Service management + resource limits                  │
│  │  (unit)     │                                                        │
│  └─────────────┘                                                        │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                           LOCAL (asus-tuf)                              │
│                    CachyOS (x86_64, 14 GB RAM, RTX 3060)               │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌─────────────┐                                                        │
│  │  OpenCode    │  Programming agent (poolside/laguna-s-2.1:free)        │
│  │  (local)     │                                                        │
│  └─────────────┘                                                        │
│                                                                          │
│  ┌─────────────┐                                                        │
│  │  Rust toolchain│  Build environment (cargo, rustc)                   │
│  │  (pending)    │                                                        │
│  └─────────────┘                                                        │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘

Tailscale VPN (mesh): 100.115.62.97 ↔ 100.92.172.48
```

## Componentes

### 1. Backend (Rust/Axum)
- **Responsabilidad:** Servir API REST, manejar autenticación, orquestar MCP clients, gestionar SQLite.
- **Framework:** Axum 0.8 (async, basado en Tokio).
- **Puerto:** 8090 (localhost only, nunca expuesto a Internet).
- **Autenticación:** Cookies HTTP-only con firma HMAC (SESSION_SECRET).
- **Dependencias cargo:** ver `Cargo.toml` (en implementación).

### 2. SQLite Database
- **Responsabilidad:** Almacenar notas metadata, conversaciones, cache, configuración, métricas.
- **File:** `data/app.db` (single file, 0-config).
- **Features:** WAL mode, FTS5 para búsqueda, migraciones con `sqlx-cli`.
- **Tables:**
  - `notes` — id, title, path, tags, created, updated, word_count
  - `conversations` — id, subject_id, title, created, updated
  - `messages` — id, conversation_id, role (user/assistant/tool), content, tokens, created
  - `academic_data_cache` — tool_name, key, data (JSON), cached_at, expires_at
  - `config` — key, value (string)
  - `usage_metrics` — date, model, input_tokens, output_tokens, cost_usd
  - `backups` — id, created_at, file_path, status, size_bytes

### 3. MCP Client (Mikareno)
- **Responsabilidad:** Conectar al MCP de Mikareno vía SSE, manejar sessions, ejecutar tools.
- **Transporte:** HTTP con SSE (`Content-Type: application/json, text/event-stream`).
- **Endpoint:** `http://127.0.0.1:8080/mcp` (localhost en remoto).
- **Session management:** POST `/mcp` con `initialize` → recibe `mcp-session-id` header → incluir en requests subsecuentes via Cookie o header.
- **Cache:** 180 segundos (configurable). SQLite table `academic_data_cache`.
- **Tools soportadas:** `get_perfil`, `get_ano_escolar`, `get_asistencia`, `get_biblioteca`, `get_calendario`, `get_cumpleanos`, `get_horario`, `get_mensajes`, `get_notas_trimestre`, `get_detalles_asignatura`, `get_detalles_tarea`, `get_detalles_evento`, `get_detalles_mensaje`, `get_detalles_grupo`, `get_detalles_profesor`, `get_mis_grupos`, `get_detalle_calendario`.
- **Límite:** Read-only. Todas las tools son de lectura. No se puede crear/modificar datos en Mikareno.

### 4. MCP Client (GenOffice)
- **Responsabilidad:** Proxy al MCP de GenOffice para la suite de oficina.
- **Endpoint:** `http://127.0.0.1:8453/mcp` (ya corre en remoto).
- **Operaciones:** crear docx desde markdown, generar PDF, crear xlsx desde CSV, buscar web, generar imágenes.
- **Autenticación:** API key (heredado del MCP local en asus-tuf).

### 5. OpenRouter Proxy
- **Responsabilidad:** Enviar prompts del asistente académico a OpenRouter, respetar límites de tokens y coste.
- **API key:** `OPENROUTER_API_KEY` (env var, nunca en source).
- **Modelo principal:** `cohere/north-mini-code:free` (zero cost).
- **Límite de salida:** 8192 tokens.
- **Límite de contexto:** 16K (resumen automático cuando se excede).
- **Límite mensual:** $2 USD (trackado en `usage_metrics`).
- **Fallback:** `gemma-3-4b-it` (solo si se aprueba gasto).
- **Streaming:** SSE streaming de respuestas.

### 6. MCP Server propio (rmcp)
- **Responsabilidad:** Exponer herramientas de la aplicación como MCP para clientes externos (ej: OpenCode remote).
- **Crate:** `rmcp` (Rust MCP SDK).
- **Transporte:** HTTP/SSE.
- **Puerto:** 8081 (localhost only).
- **Autenticación:** API key header (`X-API-Key`).
- **Tools:** `search_notes`, `get_academic_context`, `create_study_sheet`, `create_quiz`.
- **Límite:** Solo lectura + búsqueda. No operaciones destructivas.

### 7. Frontend (HTMX + Tailwind)
- **Responsabilidad:** Dashboard académico, editor de notas, chat, generación de material, office suite UI.
- **HTMX:** Para interacciones sin recargar página (SSE, websockets, AJAX).
- **Tailwind:** Utility-first CSS (incluido via CDN o build local).
- **SSE:** Para streaming de OpenRouter responses.
- **Templates:** `askama` (Rust templating) o inline HTML strings (minimal).

### 8. Markdown Notes
- **Responsabilidad:** Almacenar notas en formato Markdown compatible con Obsidian.
- **Formato:** GFM (GitHub Flavored Markdown) con Wikilinks.
- **Estructura:** `/notes/` directory con subcarpetas por subject/assignatura.
- **Frontmatter:** YAML para metadata (title, tags, created, updated).
- **Metadatos:** En SQLite (`notes` table) para FTS5 search y relaciones.
- **Wikilinks:** `[[note-name]]` para cross-referencing.

### 9. Systemd Service (remoto)
- **Responsabilidad:** Gestionar el proceso de la aplicación y el MCP.
- **MCP service:** system-level service (NOT user-level, because no systemd --user on remote).
- **App service:** system-level service.
- **Resource limits:**
  - MCP: `MemoryMax=1G`, `CPUQuota=200%` (50% de 4 cores), `Nice=10`
  - App: `MemoryMax=1G`, `CPUQuota=200%`, `Nice=10`
- **Autostart:** `WantedBy=multi-user.target`

## Flujos de datos

### F-01: Dashboard académico
```
Browser → (HTTP GET /dashboard) → Axum → (SSE POST /mcp initialize) → Mikareno MCP
  → (tools: get_calendario, get_horario, get_notas) → cache en SQLite
  → render HTML con HTMX → Browser
```

### F-02: Asistente académico
```
Browser → (SSE POST /api/chat/:subject) → Axum
  → construye prompt (contexto + MCP data)
  → (HTTPS POST /v1/chat/completions) → OpenRouter
  → stream SSE → Browser (token por token)
  → guarda en SQLite (conversations, messages)
```

### F-03: Búsqueda local
```
Browser → (HTTP GET /api/search?q=...) → Axum
  → (SQLite FTS5 query en notes + cache)
  → devuelve resultados JSON → Browser
```

### F-04: Suite de oficina
```
Browser → (HTTP POST /api/office/docx) → Axum
  → (SSE POST /mcp tools/call) → GenOffice MCP
  → genera DOCX → PDF → devuelve file → Browser
```

### F-05: Custom MCP Server
```
External client → (SSE POST /mcp) → rmcp server
  → (SQLite query) → devuelve results → client
```

## Deployment pipeline

```
asus-tuf (build):
  1. cargo build --release
  2. scp study-platform root@100.92.172.48:/home/gabo/study-platform/
  3. (future) systemd restart

Remoto (runtime):
  1. Binary en /home/gabo/study-platform/
  2. .env con OPENROUTER_API_KEY, SESSION_SECRET, etc.
  3. SQLite en data/app.db
  4. Notas en notes/
  5. MCP Mikareno en 127.0.0.1:8080 (pendiente migración)
  6. MCP GenOffice en 127.0.0.1:8453 (ya corre)
```

## Boundaries and limits

| Component | Input limit | Output limit | Notes |
|-----------|-------------|--------------|-------|
| OpenRouter | context: 16K tokens | output: 8192 tokens | Resumen auto al 16K |
| MCP cache | — | — | 180s TTL, no datos privados en cache |
| Notes | — | — | FTS5, max query 100 chars |
| SQLite | DB: 10 GB target | — | WAL mode |
| HTTP | 1 MB body | — | límite de payload |
| Sessions | 1 sesión/usuario | — | expira 24h inactividad |
