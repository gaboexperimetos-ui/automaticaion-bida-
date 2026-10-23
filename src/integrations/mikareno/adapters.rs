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
    AnoEscolar, Biblioteca, CalendarioEvent, CuotaCumple, Horario, HorarioDia, HorarioSlot, Nota,
    Perfil, Tarea,
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
        let codigo = first_str(val, &["codigo", "code", "courseCode"]);
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
            codigo,
        });
    }
    Ok(out)
}

/// Parsea los promedios por materia de `get_student_full` (`promedios.stats`).
///
/// El tool `get_notas_trimestre` devuelve `materias_y_notas: []` en el servidor
/// real, pero `get_student_full` sí trae los promedios del parcial en curso en
/// `promedios.stats[]` con `courseName`, `average` y `partialId` (p.ej.
/// `"2026:T3:1"`, de donde se deriva el trimestre).
pub fn parse_promedios_full(raw: &Value) -> Vec<Nota> {
    let Some(stats) = raw
        .get("promedios")
        .and_then(|p| p.get("stats"))
        .and_then(|s| s.as_array())
    else {
        return Vec::new();
    };
    stats
        .iter()
        .filter_map(|s| {
            let materia = s
                .get("courseName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let codigo = s
                .get("courseCode")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let nota = s.get("average").and_then(|v| v.as_f64())?;
            if materia.is_empty() {
                return None;
            }
            let trimestre = s
                .get("partialId")
                .and_then(|v| v.as_str())
                .and_then(|p| p.split(":T").nth(1))
                .and_then(|t| t.split(':').next())
                .and_then(|t| t.parse::<u8>().ok())
                .unwrap_or(0);
            Some(Nota {
                materia,
                nota: (nota * 100.0).round() / 100.0,
                trimestre,
                codigo,
            })
        })
        .collect()
}

/// Etiquetas de una lista de términos/parciales donde cada entrada puede venir
/// como string (`"T1"`) o como dict (`{"codigo": "T1", "nombre": "…"}`);
/// un dict sin código reconocible se descarta.
fn lista_etiquetas(v: &Value) -> Vec<String> {
    if let Some(arr) = v.as_array() {
        return arr.iter().filter_map(entrada_etiqueta).collect();
    }
    if let Some(map) = v.as_object() {
        // `{"P1": {…}, "P2": {…}}` → las claves son las etiquetas.
        return map.keys().cloned().collect();
    }
    Vec::new()
}

fn entrada_etiqueta(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => {
            let s = s.trim();
            (!s.is_empty()).then(|| s.to_string())
        }
        Value::Number(n) => Some(n.to_string()),
        _ if v.is_object() => {
            let l = first_str(v, &["codigo", "nombre", "name", "id", "label"]);
            (!l.is_empty()).then_some(l)
        }
        _ => None,
    }
}

/// Fecha ISO truncada a `YYYY-MM-DD` si viene con hora (mismo trato que en
/// `parse_calendario`).
fn truncar_fecha(s: &str) -> String {
    if s.len() >= 10 {
        s.chars().take(10).collect()
    } else {
        s.to_string()
    }
}

/// Parsea la salida de `get_ano_escolar` en un resumen del año lectivo.
///
/// Los nombres de campo varían según el backend, así que cada dato admite
/// varios candidatos y los ausentes quedan vacíos (`configuracion` en null).
/// La firma mantiene `Result` por simetría con los demás adapters; hoy
/// siempre devuelve `Ok`.
pub fn parse_ano_escolar(raw: &Value) -> Result<AnoEscolar, String> {
    let null = Value::Null;

    let mut ano = first_str(raw, &["ano", "anio", "year", "anoEscolar", "school_year"]);
    if ano.is_empty() {
        for k in ["ano_escolar", "anio_escolar"] {
            ano = first_str(
                raw.get(k).unwrap_or(&null),
                &["ano", "anio", "year", "codigo", "nombre"],
            );
            if !ano.is_empty() {
                break;
            }
        }
    }

    let terminos = raw
        .get("terminos")
        .or_else(|| raw.get("trimestres"))
        .map(lista_etiquetas)
        .unwrap_or_default();
    let parciales = raw
        .get("parciales")
        .map(lista_etiquetas)
        .unwrap_or_default();

    let mut periodo_actual = first_str(
        raw,
        &["periodo_actual", "trimestre_actual", "termino_actual"],
    );
    if periodo_actual.is_empty() {
        periodo_actual = first_str(
            raw.get("periodo").unwrap_or(&null),
            &["trimestre", "termino", "codigo", "nombre"],
        );
    }

    let mut parcial_actual = first_str(raw, &["parcial_actual"]);
    if parcial_actual.is_empty() {
        parcial_actual = first_str(
            raw.get("parcial").unwrap_or(&null),
            &["nombre", "codigo", "id"],
        );
    }

    Ok(AnoEscolar {
        ano,
        terminos,
        parciales,
        inicio_clases: truncar_fecha(&first_str(
            raw,
            &[
                "inicio_clases",
                "fecha_inicio",
                "inicioClases",
                "start_date",
                "fechaInicio",
            ],
        )),
        fin_clases: truncar_fecha(&first_str(
            raw,
            &[
                "fin_clases",
                "fecha_fin",
                "finClases",
                "end_date",
                "fechaFin",
            ],
        )),
        periodo_actual,
        parcial_actual,
        configuracion: raw
            .get("configuracion")
            .or_else(|| raw.get("config"))
            .cloned()
            .unwrap_or(Value::Null),
    })
}

/// Parsea la salida de `get_biblioteca` a una lista de recursos.
///
/// Acepta una lista directa, un dict con la lista bajo
/// `recursos`/`items`/`lista`/`biblioteca`/`resultados`/`coleccion`, o un
/// único recurso suelto. Las entradas que no parecen recursos (dicts sin
/// título ni url ni tipo) se descartan; sin nada reconocible devuelve `Err`
/// para que el llamador degrade con gracia.
pub fn parse_biblioteca(raw: &Value) -> Result<Vec<Biblioteca>, String> {
    if let Some(arr) = raw.as_array() {
        return Ok(arr.iter().filter_map(recurso_biblioteca).collect());
    }
    if raw.is_object() {
        for k in [
            "recursos",
            "items",
            "lista",
            "biblioteca",
            "resultados",
            "coleccion",
        ] {
            if let Some(arr) = raw.get(k).and_then(|v| v.as_array()) {
                return Ok(arr.iter().filter_map(recurso_biblioteca).collect());
            }
        }
        if let Some(recurso) = recurso_biblioteca(raw) {
            return Ok(vec![recurso]);
        }
    }
    Err("get_biblioteca: sin lista de recursos ni recurso único reconocible".to_string())
}

/// Convierte un dict suelto en `Biblioteca`; `None` si no parece un recurso.
fn recurso_biblioteca(entry: &Value) -> Option<Biblioteca> {
    if !entry.is_object() {
        return None;
    }
    let titulo = first_str(entry, &["titulo", "title", "nombre", "name"]);
    let tipo = first_str(entry, &["tipo", "type", "formato", "format"]);
    let url = first_str(entry, &["url", "enlace", "link", "href"]);
    // Exigimos al menos una marca de recurso para no convertir dicts
    // arbitrarios (p.ej. `{"status": "ok"}`) en filas vacías.
    if titulo.is_empty() && tipo.is_empty() && url.is_empty() {
        return None;
    }
    Some(Biblioteca {
        codigo: first_str(entry, &["codigo", "code", "id"]),
        titulo: if titulo.is_empty() {
            "Recurso sin título".to_string()
        } else {
            titulo
        },
        descripcion: first_str(entry, &["descripcion", "description", "resumen", "detalle"]),
        url,
        tipo,
        categoria: first_str(
            entry,
            &["categoria", "category", "materia", "subject", "seccion"],
        ),
        disponible: entry
            .get("disponible")
            .or_else(|| entry.get("available"))
            .and_then(|v| v.as_bool()),
    })
}

/// Parsea la salida de `get_cumpleanos` al registro de cumpleaños.
///
/// El servidor real (`mikareno-mcp/server.py::get_cumpleanos`, verificado
/// contra `127.0.0.1:8080`) responde:
/// `{"es_cumpleanos_hoy": bool, "celebracion": {…}}`, donde `celebracion`
/// trae datos del backend whose keys drift between terms (`contextKey`,
/// `pending`, `isLeapDayAdjustment`…), por eso cada texto admite varios
/// candidatos y el payload crudo se conserva en `raw`.
///
/// Nunca falla: un `{}` devuelve los valores por defecto (mismo trato que
/// `parse_ano_escolar`).
pub fn parse_cumpleanos(raw: &Value) -> Result<CuotaCumple, String> {
    let null = Value::Null;
    // La celebración vive bajo `celebracion` (o `celebration` según backend).
    let celebracion = raw
        .get("celebracion")
        .or_else(|| raw.get("celebration"))
        .unwrap_or(&null);

    let es_cumpleanos_hoy = [
        "es_cumpleanos_hoy",
        "esCumpleanosHoy",
        "isUserBirthday",
        "is_birthday",
    ]
    .iter()
    .find_map(|k| raw.get(*k).and_then(|v| v.as_bool()))
    .or_else(|| {
        ["es_cumpleanos_hoy", "isUserBirthday"]
            .iter()
            .find_map(|k| celebracion.get(*k).and_then(|v| v.as_bool()))
    })
    .unwrap_or(false);

    // Título/mensaje/día: primero en la celebración, luego en la raíz.
    let titulo = {
        let t = first_str(
            celebracion,
            &[
                "titulo",
                "title",
                "nombre",
                "name",
                "encabezado",
                "headline",
            ],
        );
        if t.is_empty() {
            first_str(raw, &["titulo", "title"])
        } else {
            t
        }
    };
    let mensaje = {
        let m = first_str(
            celebracion,
            &[
                "mensaje",
                "message",
                "texto",
                "descripcion",
                "description",
                "felicitacion",
            ],
        );
        if m.is_empty() {
            first_str(raw, &["mensaje", "message"])
        } else {
            m
        }
    };
    let dia = {
        let d = first_str(
            celebracion,
            &["dia", "day", "fecha", "date", "diaNacimiento"],
        );
        if d.is_empty() {
            first_str(raw, &["dia", "day", "fecha", "date"])
        } else {
            d
        }
    };

    // `raw` guarda el payload tal cual; un objeto vacío no aporta nada y se
    // deja en null para que `{}` equivalga al valor por defecto del modelo.
    let crudo = if raw.as_object().map(|m| m.is_empty()).unwrap_or(false) {
        Value::Null
    } else {
        raw.clone()
    };

    Ok(CuotaCumple {
        es_cumpleanos_hoy,
        dia: (!dia.is_empty()).then_some(dia),
        mensaje: (!mensaje.is_empty()).then_some(mensaje),
        titulo: (!titulo.is_empty()).then_some(titulo),
        raw: crudo,
    })
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

    #[test]
    fn parse_ano_escolar_y_biblioteca_roundtrip() {
        let raw_ano = json!({
            "ano": "2026",
            "terminos": ["T1", {"codigo": "T2", "nombre": "2º trimestre"}, "T3"],
            "parciales": ["P1", "P2", "P3"],
            "inicio_clases": "2026-02-16T00:00:00",
            "fin_clases": "2026-12-18",
            "periodo_actual": "T3",
            "parcial_actual": "P2",
            "configuracion": {"dias_habiles": 5},
        });
        let a = parse_ano_escolar(&raw_ano).unwrap();
        assert_eq!(a.ano, "2026");
        assert_eq!(a.terminos, vec!["T1", "T2", "T3"]); // string y dict → etiqueta
        assert_eq!(a.parciales, vec!["P1", "P2", "P3"]);
        assert_eq!(a.inicio_clases, "2026-02-16"); // truncado desde ISO-datetime
        assert_eq!(a.fin_clases, "2026-12-18");
        assert_eq!(a.periodo_actual, "T3");
        assert_eq!(a.parcial_actual, "P2");
        assert_eq!(a.configuracion["dias_habiles"], json!(5));
        let back: AnoEscolar = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(a, back);

        let raw_bib = json!({
            "recursos": [
                {
                    "codigo": "BIB-007",
                    "titulo": "Diccionario de la RAE",
                    "descripcion": "Edición vigente 2026",
                    "url": "https://biblioteca.local/7",
                    "tipo": "libro",
                    "categoria": "Lenguaje",
                    "disponible": true,
                },
            ],
        });
        let b = parse_biblioteca(&raw_bib).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].titulo, "Diccionario de la RAE");
        assert_eq!(b[0].codigo, "BIB-007");
        assert_eq!(b[0].categoria, "Lenguaje");
        assert_eq!(b[0].disponible, Some(true));
        let back: Vec<Biblioteca> =
            serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn parse_ano_escolar_y_biblioteca_toleran_campos_faltantes() {
        let a = parse_ano_escolar(&json!({})).unwrap();
        assert!(a.ano.is_empty());
        assert!(a.terminos.is_empty() && a.parciales.is_empty());
        assert!(a.inicio_clases.is_empty() && a.fin_clases.is_empty());
        assert!(a.periodo_actual.is_empty() && a.parcial_actual.is_empty());
        assert!(a.configuracion.is_null());

        // Lista vacía → Ok vacío; recurso mínimo usa rellenos; basura → Err.
        assert!(parse_biblioteca(&json!([])).unwrap().is_empty());
        let b = parse_biblioteca(&json!([{"titulo": "Revista UNAM"}])).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].titulo, "Revista UNAM");
        assert!(b[0].descripcion.is_empty() && b[0].url.is_empty());
        assert!(b[0].disponible.is_none());
        assert!(parse_biblioteca(&json!(null)).is_err());
        assert!(parse_biblioteca(&json!({"status": "ok"})).is_err());
    }
}
