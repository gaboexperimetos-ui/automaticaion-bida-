# INDEX — Índice maestro de documentación

> Punto de entrada documental de Study Platform. Última auditoría: **2026-10-20**.
> Regla de este índice: cada documento indica su vigencia. "Histórico" significa que
> refleja decisiones de su fecha y conviene cotejarlo con `SESSION_HANDOFF.md`
> (el estado verificado más reciente) antes de asumir que sigue vigente.

## Documentos vivos (mantener al día)

| Documento | Idioma | Qué contiene | Última actualización |
|---|---|---|---|
| [`../README.md`](../README.md) | es | Entrada al proyecto: qué es, arquitectura, config, rutas, despliegue | verificado 2026-10-20 |
| [`SESSION_HANDOFF.md`](SESSION_HANDOFF.md) | es | Estado verificado del último sprint (IA, admin/backup, notas, académico, shell, seguridad, despliegue, tests) | 2026-10-19 |
| [`API.md`](API.md) | es | Referencia HTTP completa de endpoints | 2026-10-19 |
| [`USER_GUIDE.md`](USER_GUIDE.md) | es | Guía de uso por pantallas | 2026-10-19 |
| [`DEVELOPER_SETUP.md`](DEVELOPER_SETUP.md) | es | Setup de desarrollo local (toolchain, BD, arranque) | 2026-10-19 |
| [`DEPLOY.md`](DEPLOY.md) | es | Runbook de despliegue a la homelab Dell (procedimiento verificado) | 2026-10-19 |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | es | Diagrama de componentes y flujos | 2026-10-19 |
| [`ROADMAP.md`](ROADMAP.md) | es | Fases de desarrollo y plan pendiente | 2026-10-19 |

## Históricos (leer con su fecha en mente)

| Documento | Qué contiene | Fecha |
|---|---|---|
| [`REQUIREMENTS.md`](REQUIREMENTS.md) | Requisitos funcionales originales | 2026-10-09 |
| [`SECURITY.md`](SECURITY.md) | Seguridad (los detalles al día están en `SESSION_HANDOFF.md` §Seguridad y en `../README.md`) | 2026-10-09 |
| [`TESTING.md`](TESTING.md) | Estrategia de pruebas | 2026-10-09 |
| [`DECISIONS.md`](DECISIONS.md) | ADRs (Architecture Decision Records) | 2026-10-09 |
| [`DEPLOYMENT.md`](DEPLOYMENT.md) | Runbook antiguo — **sustituido por `DEPLOY.md`** | 2026-10-10 |
| [`DEPLOY_RUNBOOK_M10.md`](DEPLOY_RUNBOOK_M10.md) | Runbook del hito M10 | 2026-10-10 |
| [`CHANGELOG.md`](CHANGELOG.md) · [`../CHANGELOG.md`](../CHANGELOG.md) | Historial de cambios | — |
| [`../STUDY_PLATFORM_HANDOFF.md`](../STUDY_PLATFORM_HANDOFF.md) | Handoff antiguo (Misión 12, Stitch v2) — **sustituido por `SESSION_HANDOFF.md`** | 2026-10-11 |

## Otros ficheros de raíz

| Fichero | Qué es |
|---|---|
| [`../AGENTS.md`](../AGENTS.md) | Instrucciones operativas para agentes que trabajan en este repo (deploy real, pruebas, convenciones) |
| `../DEPLOY_MANUAL_COMMANDS.txt` | Comandos de deploy manuales apuntados a mano |
| `../create_user.rs` | Fragmento suelto auxiliar para crear usuario |
| `archive/` | Material desfasado: `README_previo_2026-10-20.md` (README anterior, desfasado respecto al código) y `md_summary_obsoleto_2026-10-15.md` (resumen generado por IA con un stack que **no** es el real — decía React/Vite/Docker Compose; la implementación es Rust/Axum + HTML servidor) |

## Fuente de verdad

| Pregunta | Mirar |
|---|---|
| Variables de entorno | `src/config.rs` y `.env.example` |
| Rutas HTTP | `src/handlers/mod.rs` + `register_routes()` de cada módulo; forma completa en `API.md` |
| Seguridad | `src/middleware/auth.rs`, `rate_limit.rs`, `src/handlers/auth.rs` |
| Backups | `src/backup/` |
| Estado del último sprint | `SESSION_HANDOFF.md` |

Cómo mantener: al cerrar una tarea relevante, actualiza el documento correspondiente,
mueve a esta tabla o a `archive/` lo que quede obsoleto, y ajusta fechas.
