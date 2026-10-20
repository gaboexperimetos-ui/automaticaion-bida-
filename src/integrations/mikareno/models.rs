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

/// A single graded subject for a term (`get_notas_trimestre` / `get_student_full`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Nota {
    pub materia: String,
    pub nota: f64,
    pub trimestre: u8,
    /// Subject code (e.g. `"MATE01"`) as returned by `courseCode` in the raw
    /// promedios stats. Empty when the server omits it.
    #[serde(default)]
    pub codigo: String,
}

/// Resumen del año escolar (`get_ano_escolar`): fechas, términos, parciales y
/// configuración del período.
///
/// Esquema-tolerante: todos los campos declaran `#[serde(default)]`, así que
/// los faltantes quedan vacíos; `configuracion` guarda el dict crudo del
/// servidor (null si no viene) porque su forma interna no está normalizada.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AnoEscolar {
    /// Año lectivo (p.ej. `"2026"`).
    #[serde(default)]
    pub ano: String,
    /// Términos/trimestres del año, p.ej. `["T1", "T2", "T3"]`.
    #[serde(default)]
    pub terminos: Vec<String>,
    /// Parciales de cada término, p.ej. `["P1", "P2"]`.
    #[serde(default)]
    pub parciales: Vec<String>,
    /// Primer día de clases (ISO, truncado a `YYYY-MM-DD`).
    #[serde(default)]
    pub inicio_clases: String,
    /// Último día de clases (ISO, truncado a `YYYY-MM-DD`).
    #[serde(default)]
    pub fin_clases: String,
    /// Término en curso, si el servidor lo reporta.
    #[serde(default)]
    pub periodo_actual: String,
    /// Parcial en curso, si el servidor lo reporta.
    #[serde(default)]
    pub parcial_actual: String,
    /// Configuración del período tal cual la envía el servidor.
    #[serde(default)]
    pub configuracion: serde_json::Value,
}

/// Un recurso institucional de la biblioteca (`get_biblioteca`).
///
/// Esquema-tolerante: el servidor puede omitir campos según el tipo de
/// recurso; los ausentes quedan vacíos y `disponible` en `None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Biblioteca {
    /// Código institucional del recurso (p.ej. `"BIB-001"`).
    #[serde(default)]
    pub codigo: String,
    /// Título o nombre del recurso.
    #[serde(default)]
    pub titulo: String,
    /// Descripción breve.
    #[serde(default)]
    pub descripcion: String,
    /// URL de acceso (catálogo, PDF, enlace externo).
    #[serde(default)]
    pub url: String,
    /// Tipo de recurso (libro, revista, digital…).
    #[serde(default)]
    pub tipo: String,
    /// Categoría o materia asignada.
    #[serde(default)]
    pub categoria: String,
    /// Disponibilidad; `None` cuando el backend no la envía.
    #[serde(default)]
    pub disponible: Option<bool>,
}

/// Una cuota de cumpleaños del estudiante (`get_cumpleanos`).
///
/// El servidor devuelve `{ "es_cumpleanos_hoy": bool, "celebracion": {…} }`
/// y la forma interna de `celebracion` depende del backend (en días sin
/// cumpleaños viene casi vacía), así que el modelo es *conservador*: los
/// textos opcionales se resuelven en el adapter y el payload crudo se
/// conserva íntegro en `raw` para no perder ningún dato.
///
/// Todos los campos declaran `#[serde(default)]`: un `{}` se deserializa a
/// los valores por defecto sin error.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CuotaCumple {
    /// `true` si hoy es el cumpleaños del estudiante.
    #[serde(default)]
    pub es_cumpleanos_hoy: bool,
    /// Día del cumpleaños, tal como lo redacta el backend (p.ej. `"12 de marzo"`).
    #[serde(default)]
    pub dia: Option<String>,
    /// Mensaje de felicitación.
    #[serde(default)]
    pub mensaje: Option<String>,
    /// Título de la celebración.
    #[serde(default)]
    pub titulo: Option<String>,
    /// Payload crudo tal cual lo devuelve el servidor.
    #[serde(default)]
    pub raw: serde_json::Value,
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
