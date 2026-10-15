//! Typed models for Mikareno MCP responses.
//!
//! These are the *clean* domain types that the rest of the Study Platform
//! (Agent B's dashboard, the sync layer, etc.) consumes. They are intentionally
//! stable and flat so they can be serialized directly into API responses.
//!
//! The raw JSON returned by the Mikareno server is richer (nested dicts,
//! snake_case backend fields). Parsing from the raw `serde_json::Value` into
//! these models lives in [`crate::integrations::mikareno::adapters`].

use serde::{Deserialize, Serialize};

/// A single MCP tool exposed by the server, as advertised by `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

/// One event from the school calendar (`get_calendario`).
///
/// Maps to an entry in `proximos_dias_clases`, `vacaciones` or `eventos`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarioEvent {
    /// ISO date, e.g. `"2024-03-01"` (already truncated to 10 chars by the server).
    pub fecha: String,
    /// Human-readable event title.
    pub evento: String,
    /// Day type as emitted by the server (`SCHOOL_DAY`, `NON_SCHOOL_DAY`, …).
    pub tipo: String,
}

/// A single scheduled class slot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HorarioSlot {
    /// Day name, as emitted by the server (`lunes`, `martes`, …).
    pub dia: String,
    /// Pretty time range, e.g. `"08:00-08:40"`.
    pub hora: String,
    /// Subject name.
    pub clase: String,
    /// Subject code.
    pub codigo: String,
    /// Slot duration in minutes.
    pub duracion_min: u32,
}

/// One day of the weekly schedule, grouping its class slots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HorarioDia {
    pub dia: String,
    pub clases: Vec<String>,
}

/// The weekly schedule. `dias` preserves the order in which days were captured.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Horario {
    pub dias: Vec<HorarioDia>,
}

/// The student profile (`get_perfil`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Perfil {
    pub nombre: String,
    pub institucion: String,
    pub curso: String,
}

/// A single homework/task (`get_tareas`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tarea {
    pub titulo: String,
    pub vencimiento: String,
    pub estado: String,
}

/// A single graded subject for a term (`get_notas_trimestre`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Nota {
    pub materia: String,
    pub nota: f64,
    pub trimestre: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendario_event_roundtrips() {
        let ev = CalendarioEvent {
            fecha: "2024-03-01".to_string(),
            evento: "Matemáticas".to_string(),
            tipo: "SCHOOL_DAY".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: CalendarioEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn horario_default_is_empty() {
        assert_eq!(Horario::default().dias.len(), 0);
    }

    #[test]
    fn perfil_default_is_empty() {
        let p = Perfil::default();
        assert!(p.nombre.is_empty() && p.institucion.is_empty() && p.curso.is_empty());
    }
}
