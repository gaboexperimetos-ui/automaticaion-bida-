# Requisitos del Sistema

## Alcance

Una plataforma de productividad académica personal que integra datos de Mikareno
(vía MCP), asistente de IA (OpenRouter), segundo cerebro (notas Markdown),
generación de material de estudio, y suite de oficina web.

## Requisitos funcionales

### RF-01: Integración con MCP de Mikareno
- **Como** usuario autenticado, **quiero** que la plataforma consulte mis datos de Mikareno (calendario, horario, notas, asistencia, tareas), **para** tener toda la información académica en un solo lugar.
- El cliente MCP usa transporte SSE sobre HTTP (`POST /mcp`).
- El handshake inicial retorna `mcp-session-id` header; requests subsecuentes deben incluirlo.
- El MCP es **read-only**: no existen herramientas de escritura en las 17 tools disponibles.
- Cache local de 180s (configurable) para reducir requests al MCP.
- **Criterio de aceptación:** Al iniciar sesión, el dashboard muestra calendario, horario y notas actualizados. Verificado con `curl -X POST http://127.0.0.1:8080/mcp ... initialize`.

### RF-02: Panel académico
- **Como** estudiante, **quiero** ver mi horario, calendario académico, exámenes, tareas y notas en un panel web, **para** organizar mi tiempo.
- Datos provenientes del MCP de Mikareno (read-only).
- **Criterio de aceptación:** El panel muestra los próximos eventos del calendario con fechas correctas. Exportable a iCal.

### RF-03: Asistente académico (OpenRouter)
- **Como** usuario, **quiero** preguntar dudas académicas a un asistente de IA que tenga contexto de mis notas y tareas, **para** recibir explicaciones personalizadas.
- El asistente envía a OpenRouter únicamente el contexto necesario (máx. 8192 tokens de salida, contexto limitado a 16K).
- Modelo por defecto: `cohere/north-mini-code:free` (zero cost). Respaldo: `gemma-3-4b-it`.
- **Criterio de aceptación:** El asistente responde preguntas sobre el calendario y notas con contexto académico correcto. Coste ≤ $2 USD/mes.

### RF-04: Segundo cerebro (notas + Obsidian)
- **Como** usuario, **quiero** tomar notas en Markdown y organizarlas con etiquetas y enlaces, **para** construir un repositorio de conocimiento.
- Formato Markdown compatible con Obsidian ( Wikilinks, frontmatter, carpetas).
- Metadatos (tags, links, timestamps) almacenados en SQLite.
- Sincronización con directorio Obsidian existente (importación unidireccional).
- **Criterio de aceptación:** Una nota creada en la web app es visible y enlazable desde Obsidian. Búsqueda FTS5 devuelve resultados en <500 ms.

### RF-05: Generación de material de estudio
- **Como** usuario, **quiero** generar resúmenes, cuestionarios de práctica y hojas de estudio a partir de mis apuntes, **para** preparar exámenes.
- Usa OpenRouter para generación (con límites de tokens).
- Templates de prompts predefinidos y validados.
- Exportación a PDF/Markdown.
- **Criterio de aceptación:** Al subir apuntes y solicitar un resumen, se genera Markdown con puntos clave. Exportable a PDF.

### RF-06: Suite de oficina web
- **Como** usuario, **quiero** crear documentos, hojas de cálculo y presentaciones desde la web, **para** no depender de software instalado.
- Proxy al MCP de GenOffice (`127.0.0.1:8453`).
- Operaciones soportadas: crear docx desde markdown, generar PDF, crear xlsx desde CSV/JSON.
- **Criterio de aceptación:** Se puede crear un documento Word desde texto Markdown y exportarlo a PDF. PDF generado es legible.

### RF-07: Búsqueda local
- **Como** usuario, **quiero** buscar en mis notas, apuntes y datos académicos en un solo lugar, **para** encontrar información rápidamente.
- SQLite FTS5 como motor de bindex.
- No envía contenido a servicios externos.
- **Criterio de aceptación:** Búsqueda de 100 notas indexadas devuelve resultados relevantes en <500 ms.

### RF-08: MCP server propio
- **Como** desarrollador, **quiero** exponer funcionalidades de la aplicación como herramientas MCP, **para** que otros clientes (ej: OpenCode) las usen.
- Usa el crate `rmcp` (Rust MCP SDK).
- Transporte: HTTP/SSE (no stdio).
- Autenticación: API key header (`X-API-Key`).
- **Criterio de aceptación:** Un cliente MCP externo puede listar y ejecutar herramientas (`search_notes`, `get_academic_context`).

### RF-09: Gestión de conversaciones por asignatura
- **Como** usuario, **quiero** mantener conversaciones separadas por asignatura, **para** no mezclar contextos.
- Cada conversación asociada a una materia con historial limitado a 16K tokens.
- **Criterio de aceptación:** Cambiar de asignatura preserva historial independiente. Resumen automático cuando excede 16K tokens.

### RF-10: Administración y backups
- **Como** administrador, **quiero** configurar la aplicación, monitorear recursos y hacer backups, **para** mantener la disponibilidad y seguridad.
- Backup diario de SQLite via `sqlite3 .backup` + rsync a Gitea repo.
- Métricas de consumo OpenRouter (tokens, coste).
- **Criterio de aceptación:** Un backup puede restaurarse exitosamente. Métricas visibles en dashboard admin.

## Requisitos no funcionales

### RNF-01: Seguridad
- No secretos en código fuente.
- API keys en variables de entorno.
- No loggear credenciales ni datos académicos privados.
- MCP propio: autenticación por API key.
- HTTP solo en localhost (no exposición a Internet).

### RNF-02: Rendimiento
- App en remoto: startup < 5 segundos.
- API endpoints: < 200 ms para queries cached, < 2 segundos para MCP fetch.
- Búsqueda: < 500 ms para 1000 notas.
- Streaming OpenRouter: < 100 ms latencia de primer token.

### RNF-03: Recursos
- MCP en remoto: límite de memoria 1 GB (MemoryMax).
- CPUQuota ajustado a ~50% de cores (ver ADR-003).
- App completa: target < 1.5 GB RSS en remote (7.1 GB total, 710 MB libre).

### RNF-04: Portabilidad
- Backend: binary único (Rust static linking).
- No depender de asus-tuf para funciones esenciales (MCP debe migrarse al remoto).
- Build en asus-tuf, deploy en Ubuntu (ambos x86_64).

### RNF-05: Disponibilidad
- MCP Mikareno: debe estar en el remoto (no depender de asus-tuf encendido).
- App: reinicio automático (systemd Restart=always).
- Backups: diarios, retención 30 días.

### RNF-06: Presupuesto IA
- Modelo de desarrollo: `poolside/laguna-s-2.1:free` (no configurable, no consumir créditos).
- OpenRouter budget: $2 USD/mes máximo.
- No cambiar a modelos de pago sin aprobación explícita.
