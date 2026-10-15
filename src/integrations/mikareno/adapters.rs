//! Typed adapters: parse raw `serde_json::Value` tool output into clean domain models.
//!
//! The Mikareno server (FastMCP over Playwright) returns deeply nested,
//! snake_case dicts whose exact shape can vary between backends/terms. Each
//! adapter below is therefore *defensive*: it tries several well-known field
//! names, falls back to `"desconocido"`/empty strings, and never panics. This
//! keeps the client resilient when the remote schema drifts.
//!
//! None of these functions perform I/O; they are pure transforms of the value
//! already returned by [`MikarenoClient::call_tool`].

use serde_json::Value;

use crate::integrations::mikareno::models::{
    CalendarioEvent, Horario, HorarioDia, HorarioSlot, Nota, Perfil, Tarea,
};

/// Extract a string field from `obj` trying each candidate key in order.
fn first_str(obj: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(v) = obj.get(*k) {
            if let Some(s) = v.as_str() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn opt_str(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// Parse the output of `get_calendario` into a flat list of calendar events.
///
/// Combines `proximos_dias_clases`, `vacaciones` and `eventos`, mapping each
/// entry to `{ fecha, evento=titulo, tipo=tipo_dia }`.
pub fn parse_calendario(raw: &Value) -> Result<Vec<CalendarioEvent>, String> {
    let mut out = Vec::new();
    for list_key in ["proximos_dias_clases", "vacaciones", "eventos"] {
        let list = match raw.get(list_key).and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => continue,
        };
        for entry in list {
            if !entry.is_object() {
                continue;
            }
            let fecha = opt_str(entry, "fecha").unwrap_or_default();
            // Server already truncates to 10 chars, but be defensive.
            let fecha = if fecha.len() >= 10 {
                fecha.chars().take(10).collect()
            } else {
                fecha
            };
            let evento = first_str(entry, &["titulo", "title", "name"]);
            let tipo = first_str(entry, &["tipo_dia", "dayType", "type"]);
            let tipo = if tipo.is_empty() {
                if list_key == "vacaciones" {
                    "NON_SCHOOL_DAY".to_string()
                } else {
                    "EVENTO".to_string()
                }
            } else {
                tipo
            };
            out.push(CalendarioEvent {
                fecha,
                evento: if evento.is_empty() {
                    "Evento sin título".to_string()
                } else {
                    evento
                },
                tipo,
            });
        }
    }
    Ok(out)
}

/// Parse the output of `get_horario` into a grouped weekly schedule.
///
/// Groups flat `{dia, hora, clase, codigo, duracion_min}` entries by day,
/// preserving first-seen day order. Each class becomes
/// `"hora — clase [codigo]"`.
pub fn parse_horario(raw: &Value) -> Result<Horario, String> {
    let slots = raw
        .get("horario")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "get_horario: missing 'horario' array".to_string())?;

    let mut parsed: Vec<HorarioSlot> = Vec::with_capacity(slots.len());
    for s in slots {
        if !s.is_object() {
            continue;
        }
        parsed.push(HorarioSlot {
            dia: first_str(s, &["dia", "day", "dayOfWeek"]),
            hora: opt_str(s, "hora").unwrap_or_default(),
            clase: first_str(s, &["clase", "subject", "name"]),
            codigo: first_str(s, &["codigo", "code"]),
            duracion_min: s.get("duracion_min").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        });
    }
    // first-seen order.
    let canonical: Vec<String> = raw
        .get("dias")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|x| x.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut order: Vec<String> = Vec::new();
    let mut by_day: Vec<(String, Vec<String>)> = Vec::new();
    for slot in parsed {
        if !by_day.iter().any(|(d, _)| d == &slot.dia) {
            order.push(slot.dia.clone());
            by_day.push((slot.dia.clone(), Vec::new()));
        }
        let clases = by_day.iter_mut().find(|(d, _)| d == &slot.dia).unwrap();
        let label = if slot.hora.is_empty() && slot.codigo.is_empty() {
            slot.clase.clone()
        } else if slot.hora.is_empty() {
            format!("{} [{}]", slot.clase, slot.codigo)
        } else if slot.codigo.is_empty() {
            format!("{} — {}", slot.hora, slot.clase)
        } else {
            format!("{} — {} [{}]", slot.hora, slot.clase, slot.codigo)
        };
        clases.1.push(label);
    }

    // Order days according to the canonical list, then any remaining in order.
    let mut ordered: Vec<String> = canonical.clone();
    for d in &order {
        if !ordered.contains(d) {
            ordered.push(d.clone());
        }
    }

    let dias = ordered
        .into_iter()
        .filter_map(|d| {
            let clases = by_day
                .iter()
                .find(|(day, _)| day == &d)
                .map(|(_, c)| c.clone())?;
            Some(HorarioDia { dia: d, clases })
        })
        .collect();

    Ok(Horario { dias })
}

/// Parse the output of `get_perfil` into `{ nombre, institucion, curso }`.
pub fn parse_perfil(raw: &Value) -> Result<Perfil, String> {
    let estudiante = raw.get("estudiante").unwrap_or(&Value::Null);
    let nombre = first_str(estudiante, &["nombre", "name", "fullName"]);

    // `institucion` is returned as a rich dict; pull a readable name, falling
    // back to a JSON stringify of the whole object.
    let inst_val = raw.get("institucion").unwrap_or(&Value::Null);
    let institucion = first_str(
        inst_val,
        &["name", "nombre", "officialName", "displayName", "shortName"],
    );
    let institucion = if institucion.is_empty() {
        serde_json::to_string(inst_val).unwrap_or_else(|_| "desconocida".to_string())
    } else {
        institucion
    };

    // Build a readable course string from `curso_actual`.
    let curso_val = raw.get("curso_actual").unwrap_or(&Value::Null);
    let codigo = first_str(curso_val, &["codigo", "code"]);
    let nombre_curso = first_str(curso_val, &["nombre", "name"]);
    let paralelo = opt_str(curso_val, "paralelo");
    let seccion = opt_str(curso_val, "seccion");
    let grado = opt_str(curso_val, "grado");
    let tutor = opt_str(curso_val, "tutor");

    let curso = if codigo.is_empty() && nombre_curso.is_empty() {
        "Curso desconocido".to_string()
    } else {
        let mut head = String::new();
        if !codigo.is_empty() {
            head.push_str(&codigo);
        }
        if !nombre_curso.is_empty() {
            if !head.is_empty() {
                head.push_str(" — ");
            }
            head.push_str(&nombre_curso);
        }
        if let Some(p) = &paralelo {
            head.push_str(&format!(" ({})", p));
        }
        if let Some(s) = &seccion {
            head.push_str(&format!("/{}", s));
        }
        let mut parts: Vec<String> = vec![head];
        if let Some(t) = &tutor {
            if !t.is_empty() {
                parts.push(format!("tutor: {}", t));
            }
        }
        if let Some(g) = &grado {
            if !g.is_empty() {
                parts.push(format!("grado: {}", g));
            }
        }
        parts.join(", ")
    };

    Ok(Perfil {
        nombre: nombre.clone(),
        institucion,
        curso,
    })
}

/// Parse the output of `get_tareas` into a flat list of tasks.
pub fn parse_tareas(raw: &Value) -> Result<Vec<Tarea>, String> {
    let lista = raw
        .get("lista_tareas")
        .and_then(|v| v.as_array())
        .or_else(|| raw.get("pageContent").and_then(|v| v.as_array()))
        .ok_or_else(|| "get_tareas: missing 'lista_tareas'".to_string())?;

    let mut out = Vec::new();
    for t in lista {
        if !t.is_object() {
            continue;
        }
        let titulo = first_str(t, &["title", "titulo", "name", "asunto", "description"]);
        let vencimiento = first_str(
            t,
            &[
                "dueDate",
                "fechaVencimiento",
                "endDate",
                "fecha_limite",
                "fechaVencimiento",
                "vencimiento",
            ],
        );
        let estado = first_str(t, &["status", "estado", "state", "deliveryStatus"]);
        out.push(Tarea {
            titulo: if titulo.is_empty() {
                "Tarea sin título".to_string()
            } else {
                titulo
            },
            vencimiento,
            estado: if estado.is_empty() {
                "SIN_ESTADO".to_string()
            } else {
                estado
            },
        });
    }
    Ok(out)
}

/// Parse the output of `get_notas_trimestre` into a list of graded subjects.
///
/// `raw` must carry a top-level `trimestre` integer and a
/// `materias_y_notas` array of `{materia, nota, ...}`.
pub fn parse_notas(raw: &Value) -> Result<Vec<Nota>, String> {
    let materias = raw
        .get("materias_y_notas")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "get_notas_trimestre: missing 'materias_y_notas'".to_string())?;

    // Prefer the server-reported term; fall back to the first numeric field.
    let trimestre = raw
        .get("trimestre")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            raw.get("trimestre")
                .and_then(|v| v.as_i64())
                .map(|x| x as u64)
        })
        .unwrap_or(0) as u8;

    let mut out = Vec::new();
    for m in materias {
        if !m.is_object() {
            continue;
        }
        let val = m;
        let materia = first_str(val, &["materia", "courseName", "name", "subject"]);
        let nota = val
            .get("nota")
            .or_else(|| val.get("average"))
            .and_then(|v| {
                if let Some(n) = v.as_f64() {
                    Some(n)
                } else if let Some(s) = v.as_str() {
                    s.parse::<f64>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0.0);
        let trimmed = (nota * 100.0).round() / 100.0;
        out.push(Nota {
            materia,
            nota: trimmed,
            trimestre,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendario_json() -> Value {
        json!({
            "resumen": {"total_dias": 3},
            "proximos_dias_clases": [
                {"fecha": "2024-03-01T00:00:00", "titulo": "Clase de Matemáticas", "tipo_dia": "SCHOOL_DAY"},
                {"fecha": "2024-03-02", "titulo": "Clase de Lenguaje", "tipo_dia": "SCHOOL_DAY"},
            ],
            "vacaciones": [
                {"fecha": "2024-03-15", "titulo": "Vacaciones de Semana", "tipo_dia": "NON_SCHOOL_DAY"},
            ],
            "eventos": [
                {"fecha": "2024-04-01", "titulo": "Feria del libro", "tipo_dia": ""},
            ],
        })
    }

    fn horario_json() -> Value {
        json!({
            "horario": [
                {"dia": "lunes", "hora": "08:00-08:40", "clase": "Matemáticas", "codigo": "M1", "duracion_min": 40},
                {"dia": "lunes", "hora": "09:00-09:40", "clase": "Lenguaje", "codigo": "L1", "duracion_min": 40},
                {"dia": "martes", "hora": "08:00-08:40", "clase": "Ciencias", "codigo": "C1", "duracion_min": 40},
            ],
            "total_clases": 3,
            "dias": ["lunes", "martes"],
        })
    }

    #[test]
    fn parses_calendario_combines_all_lists() {
        let evs = parse_calendario(&calendario_json()).unwrap();
        assert_eq!(evs.len(), 4);
        assert_eq!(evs[0].fecha, "2024-03-01"); // truncated from T00:00:00
        assert_eq!(evs[0].evento, "Clase de Matemáticas");
        assert_eq!(evs[0].tipo, "SCHOOL_DAY");
        assert_eq!(evs[3].tipo, "EVENTO"); // empty tipo from eventos defaults
    }

    #[test]
    fn parses_horario_groups_by_day() {
        let h = parse_horario(&horario_json()).unwrap();
        assert_eq!(h.dias.len(), 2);
        assert_eq!(h.dias[0].dia, "lunes");
        assert_eq!(h.dias[0].clases.len(), 2);
        assert!(h.dias[0].clases[0].contains("Matemáticas"));
        assert_eq!(h.dias[1].dia, "martes");
        assert_eq!(h.dias[1].clases.len(), 1);
    }

    #[test]
    fn parses_perfil() {
        let raw = json!({
            "estudiante": {"nombre": "Gabriel", "email": "g@x"},
            "institucion": {"name": "Colegio Álamo", "officialName": "Álamo"},
            "curso_actual": {"codigo": "7A", "nombre": "7ºB", "paralelo": "A", "tutor": "Prof. Rivera"},
            "periodo_lectivo": {"nombre": "T3"},
        });
        let p = parse_perfil(&raw).unwrap();
        assert_eq!(p.nombre, "Gabriel");
        assert_eq!(p.institucion, "Colegio Álamo");
        assert!(p.curso.contains("7A"));
        assert!(p.curso.contains("Prof. Rivera"));
    }

    #[test]
    fn parses_tareas() {
        let raw = json!({
            "resumen": {"esta_semana": 2},
            "lista_tareas": [
                {"title": "Entregar proyecto", "dueDate": "2024-03-05", "status": "PENDING"},
                {"title": "Ejercicios", "fechaVencimiento": "2024-03-06", "estado": "ENTREGADA"},
            ],
            "total_lista": 2,
        });
        let t = parse_tareas(&raw).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].titulo, "Entregar proyecto");
        assert_eq!(t[0].vencimiento, "2024-03-05");
        assert_eq!(t[0].estado, "PENDING");
        assert_eq!(t[1].estado, "ENTREGADA");
    }

    #[test]
    fn parses_notas() {
        let raw = json!({
            "trimestre": 1,
            "materias_y_notas": [
                {"materia": "Matemáticas", "nota": 8.5, "max_score": 10, "codigo": "M1"},
                {"materia": "Lenguaje", "nota": 9.0, "max_score": 10, "codigo": "L1"},
            ],
            "total_materias": 2,
            "promedio_general": 8.75,
        });
        let n = parse_notas(&raw).unwrap();
        assert_eq!(n.len(), 2);
        assert_eq!(n[0].materia, "Matemáticas");
        assert!((n[0].nota - 8.5).abs() < 1e-9);
        assert_eq!(n[0].trimestre, 1);
    }

    #[test]
    fn parse_tareas_missing_list_is_err() {
        assert!(parse_tareas(&json!({})).is_err());
    }

    #[test]
    fn parse_notas_missing_list_is_err() {
        assert!(parse_notas(&json!({})).is_err());
    }
}
