# Paridad funcional de NOVA Office con GenOffice

**Estado:** trabajo en curso; NOVA Office todavía no es un clon completo.

El objetivo es llevar las capacidades de GenOffice a la aplicación web NOVA,
conservando el tema oscuro y el almacenamiento bajo control del usuario. La
referencia funcional es el proyecto oficial
[`genspark-ai/genoffice`](https://github.com/genspark-ai/genoffice) (Apache-2.0),
no sus capturas ni una maqueta. Antes de reutilizar código o assets de ese
proyecto se deben conservar sus avisos de licencia y atribución. La interfaz
será web: no se incorporará una aplicación Electron como si fuera una función
web.

## Matriz de alcance

| Área | Objetivo de paridad | Estado de NOVA en este PR |
| --- | --- | --- |
| Documentos | Editor real de `.docx` y Markdown; formato, estructura, guardado, exportación y cambios revisables | Editor Markdown básico; propuesta IA opt-in. Sin edición OOXML de Word |
| Hojas | `.xlsx` multípestaña, fórmulas recalculables, formato, gráficos y round-trip | Cuadrícula CSV de una hoja con `SUM`/`AVG`; en este avance se añade importación de una pestaña y descarga `.xlsx`. No preserva estilos, gráficos ni macros |
| Presentaciones | Crear/editar diapositivas y abrir/guardar `.pptx` real | Diapositivas JSON y modo presentación; sin importación/exportación PPTX |
| PDF | Abrir y leer, búsqueda, anotación/edición y conversión | No implementado |
| HTML | Editor de código/diseño, vista aislada en vivo y exportación | No implementado |
| Markdown | Edición estructurada, vista previa, enlaces, imágenes y exportación | Editor de texto con Markdown, vista previa HTML y exportación; sin paridad de bloques |
| IA en editores | Contexto del archivo bajo solicitud, operaciones revisables, diffs, deshacer y cambios rastreados | Panel de propuesta para Markdown; simulación si no hay proveedor. No hay operaciones/diffs de documentos ni paneles completos por app |
| Biblioteca y búsqueda | Archivos indexados por nombre y contenido; favoritos, preguntas con fuentes y conversaciones | Biblioteca local de los tres tipos actuales y búsqueda limitada por nombre |
| Conversiones | Conversiones locales reales entre formatos compatibles | Exportaciones `.md`, HTML y CSV; informes `.docx` dependen del MCP de GenOffice configurado |
| Integraciones de agentes | CLI/MCP capaces de leer/crear/editar los archivos reales | Hay MCP propio y conexión limitada al MCP de GenOffice; no existe paridad de su CLI/skill |

La primera ampliación de formatos usa el bundle local de SheetJS CE 0.20.3
(publicado como `@e965/xlsx`), con licencia Apache-2.0 en
`static/vendor/xlsx-LICENSE.txt`. El importador convierte una pestaña por vez a
la cuadrícula CSV existente; el exportador genera un `.xlsx` nuevo de una sola
pestaña. Esto es interoperabilidad inicial, no round-trip OOXML fiel.

## Etapas y puertas de aceptación

1. **Formatos e integridad.** Añadir importación/exportación verificable de los
   formatos principales, límites de archivo, seguridad de parseo, pruebas de
   round-trip y mensajes explícitos cuando un formato pierda propiedades. El
   primer tramo de Excel convierte una hoja cada vez; no debe presentarse como
   compatibilidad completa con libros complejos.
2. **Seis editores web.** Completar Documentos, Hojas, Presentaciones, PDF, HTML
   y Markdown como aplicaciones de verdad, con guardado/reapertura, navegación
   por pestañas y flujos de importación/exportación propios.
3. **Agente de documentos.** Añadir acciones editables con contexto mínimo,
   vista previa de cambios, historial, aplicar/revertir y flujos de IA para cada
   editor. Sin proveedor configurado, la UI debe decir claramente que está en
   modo simulado; las pruebas no gastarán créditos.
4. **Biblioteca y consulta.** Indexar contenido local de los formatos
   compatibles, filtros/favoritos, preguntas con citas a archivos/páginas/celdas,
   adjuntos y conversaciones persistentes. Todo acceso remoto será opt-in y
   explicado.
5. **Paridad y regresión.** Validar flujos de usuario de punta a punta con un
   navegador contra el backend real y almacenamiento temporal; completar una
   matriz de formatos, teclado, accesibilidad, límites, seguridad y permisos.
   No declarar “clon completo” antes de cerrar las capacidades marcadas arriba.

Las funciones de Mikareno son un sistema externo distinto de GenOffice. Requieren
una conexión autenticada y permisos adecuados; una anotación local del calendario
no equivale a un evento de Mikareno, y los datos privados no se simularán.
