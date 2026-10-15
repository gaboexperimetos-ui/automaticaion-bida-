# Study Platform — interfaz frontend

Frontend responsive en HTML, CSS y JavaScript sin framework ni dependencias de build. Su propósito es ofrecer una interfaz ligera para el servidor con recursos limitados.

## Abrir la demostración

Abre `index.html` en un navegador moderno. Los datos mostrados son claramente demostrativos y las notas, tareas, asignaturas y documentos se guardan en `localStorage` de ese navegador. No se envían datos a ningún modelo de IA ni a una API en esta vista de demostración.

## Secciones incluidas

- Resumen personal con indicadores, tareas, asignaturas, semana y temporizador de enfoque.
- Agenda con vistas de semana, mes y lista; creación/edición de actividades y exportación iCalendar.
- Asignaturas con búsqueda, progreso editable y notas vinculadas.
- Segundo cerebro con CRUD de notas, etiquetas, búsqueda e import/export Markdown.
- Asistente académico con contextos por materia. En el modo demo no simula una respuesta de LLM ni hace llamadas externas.
- Suite de oficina con edición/guardado local de Markdown, tabla editable CSV y presentación simple con diapositivas.
- Configuración de demostración y prueba básica del endpoint `/health`.

## Integración con Rust/Axum

Este paquete no reemplaza automáticamente los handlers/templates existentes del backend. Antes de integrarlo, comprueba los contratos HTTP reales del proyecto.

1. Sirve los tres archivos desde el mismo origen que el backend o incorpora el contenido a la estructura de templates/estáticos actual.
2. Ajusta el router para servir `index.html`, `styles.css` y `app.js`, preservando `/health`, autenticación y rutas existentes.
3. Configura `window.STUDY_PLATFORM_API_BASE` o la preferencia «URL base de la API» solo tras verificar la URL correcta.
4. `Comprobar servidor` verifica `GET /health`. No demuestra por sí solo que las APIs de notas, IA, oficina o MCP funcionen.
5. Los elementos locales de demo deben reemplazarse por llamadas a los endpoints auténticos, con autenticación y CSRF donde corresponda, antes de producción. Nunca pongas `OPENROUTER_API_KEY` en el JavaScript del navegador.
6. No conectes los datos locales de demo a una bóveda real sin probar primero el flujo de importación y preservación de archivos originales.

## Notas técnicas

- No usa frameworks externos, bundles o grandes dependencias.
- La hoja CSV admite celdas editables y exportación; no pretende reemplazar Excel.
- La presentación permite editar texto y diapositivas en formato JSON local; exportación PPTX no incluida.
- No es una UI autenticada por sí sola; la seguridad efectiva debe implementarse en el backend.
