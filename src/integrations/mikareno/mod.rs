//! MCP client for Mikareno (read-only academic data)
//!
//! A read-only MCP client that speaks the MCP-over-SSE protocol spoken by the
//! Mikareno student server (`http://127.0.0.1:8080/mcp`). It handles
//! `initialize` (session-id harvesting), `tools/list`, `tools/call`, TTL-backed
//! response caching, retries with backoff, 30s request timeouts, and exposes
//! typed adapters for `calendario`, `horario`, `perfil`, `tareas` and `notas`,
//! plus raw readers + parsers for `ano_escolar` and `biblioteca`.
//!
//! Agent A implementation. Agent B (dashboard) consumes the public API below.

mod adapters;
mod cache;
mod client;
mod models;

pub use client::{MikarenoClient, MikarenoError, DEFAULT_CACHE_TTL_SECS};
pub use models::{
    AnoEscolar, Biblioteca, CalendarioEvent, CuotaCumple, Horario, HorarioDia, HorarioSlot, Nota,
    Perfil, Tarea, ToolInfo,
};

/// Parsea `get_ano_escolar` al resumen tipado del año lectivo.
pub use adapters::parse_ano_escolar;
/// Parsea `get_biblioteca` a la lista tipada de recursos.
pub use adapters::parse_biblioteca;
// Re-export: parsea `get_cumpleanos` al registro de cumpleaños del estudiante.
pub use adapters::parse_cumpleanos;
/// Parsea `promedios.stats` de `get_student_full` a notas por materia.
pub use adapters::parse_promedios_full;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cumpleanos_roundtrip() {
        let raw = json!({
            "es_cumpleanos_hoy": true,
            "celebracion": {
                "contextKey": "1:128:2026:WEB",
                "isLeapDayAdjustment": false,
                "pending": false,
                "titulo": "¡Feliz cumpleaños!",
                "mensaje": "Celebramos tu día",
                "dia": "12 de marzo",
            }
        });
        let c = parse_cumpleanos(&raw).unwrap();
        assert!(c.es_cumpleanos_hoy);
        assert_eq!(c.titulo.as_deref(), Some("¡Feliz cumpleaños!"));
        assert_eq!(c.mensaje.as_deref(), Some("Celebramos tu día"));
        assert_eq!(c.dia.as_deref(), Some("12 de marzo"));
        // El payload crudo se conserva íntegro.
        assert_eq!(c.raw, raw);
        // Roundtrip serde del modelo.
        let json = serde_json::to_string(&c).unwrap();
        let back: CuotaCumple = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn cumpleanos_tolera_campos_faltantes() {
        // `{}` → valores por defecto, sin error.
        let c = parse_cumpleanos(&json!({})).unwrap();
        assert!(!c.es_cumpleanos_hoy);
        assert!(c.dia.is_none() && c.mensaje.is_none() && c.titulo.is_none());
        assert!(c.raw.is_null());
        // Payload con la forma real del servidor (sin textos).
        let c = parse_cumpleanos(&json!({
            "es_cumpleanos_hoy": false,
            "celebracion": {
                "contextKey": "1:128:2026:WEB",
                "isLeapDayAdjustment": false,
                "pending": false
            }
        }))
        .unwrap();
        assert!(!c.es_cumpleanos_hoy);
        assert!(c.titulo.is_none());
        // Deserializar `{}` como modelo también da defaults.
        let m: CuotaCumple = serde_json::from_value(json!({})).unwrap();
        assert_eq!(m, CuotaCumple::default());
    }
}
