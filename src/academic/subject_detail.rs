//! Subject detail view — per-materia grade breakdown and activities.
//!
//! GET /dashboard/subjects/{code}
//!
//! Resolves the materia name from the subject code using `student_full`
//! (promedios.stats), then calls `get_detalle_materia` and
//! `get_actividades_materia` for a detailed breakdown of grades and
//! activities grouped by partial/submenu.
//!
//! All MCP calls degrade to honest empty states when the server is
//! disconnected or a tool fails — no fabricated data is ever shown.
//!
//! Notas de la app: cada nota/tarea/evento ya cargado se muestra clicable
//! **solo** si existe una nota real en la tabla `notes` (SQL directo). Si no
//! existe, se muestra como texto plano con un badge honesto ("Sin nota").
//! Nunca se generan enlaces muertos a `/app/notes/{slug}`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::Html,
};
use serde_json::{json, Value};
use sqlx::Row;
use tokio::time::{self, Duration};

use crate::academic::{mikareno_client, templates};
use crate::models::Note;
use crate::AppState;

/// Max length for the `code` path parameter (defends against path traversal).
const MAX_CODE_LEN: usize = 64;

/// GET /dashboard/subjects/{code} — per-materia detail page.
///
/// `code` is the subject `courseCode` from the Mikareno promedios data
/// (e.g. `"INFO01"`). If the code is not found, the page shows an honest
/// "asignatura no encontrada" message.
pub async fn view(State(state): State<Arc<AppState>>, Path(code): Path<String>) -> Html<String> {
    let client_opt = mikareno_client(&state);

    // Guard against path-traversal / absurdly long codes.
    let code = if code.len() > MAX_CODE_LEN {
        String::new()
    } else {
        code
    };

    let mut connected = false;
    let mut materia = String::new();
    let mut promedio_fallback = 0.0f64;
    let mut maximo_fallback = 10.0f64;
    let mut trimestre_display = 0u8;
    let mut bloqueada_fallback = false;
    let mut detalle: Vec<templates::MateriaDetalle> = Vec::new();
    let mut submenus: Vec<templates::SubmenuDetalle> = Vec::new();
    let mut found = false;

    if let Some(client) = client_opt {
        // Handshake — same 10s budget used by the other academic handlers.
        let init_ok = time::timeout(Duration::from_secs(10), client.initialize())
            .await
            .is_ok();
        if init_ok {
            connected = true;

            // 1. Resolve materia name + summary from student_full promedios.
            if let Ok(full) = client.student_full().await {
                found = resolve_from_promedios(
                    &full,
                    &code,
                    &mut materia,
                    &mut promedio_fallback,
                    &mut maximo_fallback,
                    &mut trimestre_display,
                    &mut bloqueada_fallback,
                );

                // 2. Degrado honesto: only fetch detail/activities if the
                //    code resolved to a real materia.
                if found {
                    // 3. get_detalle_materia(materia=<courseName>)
                    //
                    // Verified shape from mikareno-mcp/server.py — the
                    // argument is `materia` (the subject NAME, not the code).
                    // Returns {"materias_detalladas":[...], "total": N}.
                    //
                    // NOTE: the server does SUBSTRING matching on `materia`,
                    // so "Física" also matches "Educación Física". We filter
                    // to the exact courseName to avoid mixing subjects.
                    if let Ok(raw) = client
                        .call_tool(
                            "get_detalle_materia",
                            json!({ "materia": materia.clone(), "bg": true }),
                        )
                        .await
                    {
                        detalle = parse_detalle_materia(&raw, &materia);
                    }

                    // 4. get_actividades_materia(materia=<courseName>, trimestre=<parcial>)
                    //
                    // Verified shape: arguments are `materia` (string name)
                    // and `trimestre` (int, partial number within the term).
                    // Returns {"trimestre": N, "materias":[...], "total_materias": N}.
                    let parcial = extract_parcial_num(&full, &materia, trimestre_display);

                    if let Ok(raw) = client
                        .call_tool(
                            "get_actividades_materia",
                            json!({
                                "materia": materia.clone(),
                                "trimestre": parcial,
                                "bg": true,
                            }),
                        )
                        .await
                    {
                        submenus = parse_actividades_materia(&raw, &materia);
                    }
                }
            }
        }
    }

    // If the code resolved to a materia, render the full detail page.
    // If not (MCP disconnected, timeout, or code not found), render an
    // honest empty state.
    if found {
        // Índice de notas REALES de la BD (`notes`): solo se enlazan slugs
        // que existen de verdad. `None` = la BD no pudo leerse → la página
        // lo dice ("Notas no disponibles") en vez de inventar enlaces.
        let notes = load_note_index(&state.db).await;

        Html(subject_page_html(
            connected,
            &materia,
            &code,
            promedio_fallback,
            maximo_fallback,
            trimestre_display,
            bloqueada_fallback,
            &detalle,
            &submenus,
            notes.as_ref(),
        ))
    } else {
        Html(not_found_page(&code, connected))
    }
}

// ── Resolution from student_full promedios ────────────────────────────

/// Search `promedios.stats` for an entry whose `courseCode` matches `code`
/// (case-insensitive). Also falls back to matching the normalized materia
/// name. Returns `true` if a match was found.
///
/// Extracts `courseName`, `average`, `maxScore`, `partialId` → trimestre,
/// and `isLocked` from the matched stat.
fn resolve_from_promedios(
    full: &Value,
    code: &str,
    materia: &mut String,
    promedio: &mut f64,
    maximo: &mut f64,
    trimestre: &mut u8,
    bloqueada: &mut bool,
) -> bool {
    let stats = match full
        .get("promedios")
        .and_then(|p| p.get("stats"))
        .and_then(|s| s.as_array())
    {
        Some(arr) => arr,
        None => return false,
    };

    let code_norm = normalize_name(code);

    for s in stats {
        if let Some(obj) = s.as_object() {
            let c = obj.get("courseCode").and_then(|v| v.as_str()).unwrap_or("");
            let name = obj.get("courseName").and_then(|v| v.as_str()).unwrap_or("");

            let matches_code = !c.is_empty() && c.eq_ignore_ascii_case(code);
            let matches_name = code_norm == normalize_name(name);

            if matches_code || matches_name {
                *materia = name.to_string();
                *promedio = obj.get("average").and_then(|v| v.as_f64()).unwrap_or(0.0);
                *maximo = obj.get("maxScore").and_then(|v| v.as_f64()).unwrap_or(10.0);
                *bloqueada = obj
                    .get("isLocked")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                *trimestre =
                    extract_trimestre_from_partial(obj.get("partialId").and_then(|v| v.as_str()));
                return true;
            }
        }
    }
    false
}

/// Extract the T-number (e.g. 3 for "2026:T3:1") from a `partialId` string.
fn extract_trimestre_from_partial(partial_id: Option<&str>) -> u8 {
    partial_id
        .and_then(|p| p.split(":T").nth(1))
        .and_then(|t| t.split(':').next())
        .and_then(|t| t.parse::<u8>().ok())
        .unwrap_or(0)
}

/// Extract the partial number within the term (e.g. 1 for "2026:T3:1").
/// This is the value passed as `trimestre` to `get_actividades_materia`.
fn extract_parcial_num(full: &Value, materia: &str, _trimestre: u8) -> u8 {
    if let Some(stats) = full
        .get("promedios")
        .and_then(|p| p.get("stats"))
        .and_then(|s| s.as_array())
    {
        let norm = normalize_name(materia);
        for s in stats {
            let name = s.get("courseName").and_then(|v| v.as_str()).unwrap_or("");
            if normalize_name(name) == norm {
                if let Some(pid) = s.get("partialId").and_then(|v| v.as_str()) {
                    let n = pid.rsplit(':').next();
                    if let Some(n) = n {
                        if let Ok(val) = n.parse::<u8>() {
                            return val;
                        }
                    }
                }
            }
        }
    }
    1 // Default to the first parcial.
}

/// Simple accent + case normalization matching the server's `_norm()`.
fn normalize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'á' | 'à' | 'ä' | 'â' | 'ā' => out.push('a'),
            'é' | 'è' | 'ë' | 'ê' | 'ē' => out.push('e'),
            'í' | 'ì' | 'ï' | 'î' | 'ī' => out.push('i'),
            'ó' | 'ò' | 'ö' | 'ô' | 'ō' => out.push('o'),
            'ú' | 'ù' | 'ü' | 'û' | 'ū' => out.push('u'),
            'Á' | 'À' | 'Ä' | 'Â' | 'Ā' => out.push('a'),
            'É' | 'È' | 'Ë' | 'Ê' | 'Ē' => out.push('e'),
            'Í' | 'Ì' | 'Ï' | 'Î' | 'Ī' => out.push('i'),
            'Ó' | 'Ò' | 'Ö' | 'Ô' | 'Ō' => out.push('o'),
            'Ú' | 'Ù' | 'Ü' | 'Û' | 'Ū' => out.push('u'),
            'ñ' | 'Ñ' => out.push('n'),
            _ => out.push(c),
        }
    }
    out.to_lowercase()
}

// ── Parsing: get_detalle_materia ──────────────────────────────────────

/// Parse the output of `get_detalle_materia`, filtering to the exact
/// `materia` name (the server does substring matching, which can return
/// multiple subjects like "Física" matching "Educación Física").
fn parse_detalle_materia(raw: &Value, materia_filter: &str) -> Vec<templates::MateriaDetalle> {
    let arr = match raw.get("materias_detalladas").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };

    arr.iter()
        .filter_map(|e| {
            if !e.is_object() {
                return None;
            }
            let materia = e
                .get("materia")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Exact match only — avoid substring false-positives.
            if materia != materia_filter {
                return None;
            }
            if materia.is_empty() {
                return None;
            }
            let tipo = e.get("tipo").and_then(|v| v.as_str()).unwrap_or("");
            let es_cualitativa = tipo == "cualitativa";
            Some(templates::MateriaDetalle {
                materia,
                codigo: e
                    .get("codigo")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                nota_promedio: e.get("nota_promedio").and_then(|v| v.as_f64()),
                maximo: e.get("maximo").and_then(|v| v.as_f64()).unwrap_or(10.0),
                bloqueada: e
                    .get("bloqueada")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                es_cualitativa,
                observacion: if es_cualitativa {
                    e.get("observacion")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                } else {
                    String::new()
                },
            })
        })
        .collect()
}

// ── Parsing: get_actividades_materia ──────────────────────────────────

/// Parse the output of `get_actividades_materia`, filtering to the exact
/// `materia` name (the server does substring matching, which can return
/// multiple subjects like "Física" matching "Educación Física").
fn parse_actividades_materia(raw: &Value, materia_filter: &str) -> Vec<templates::SubmenuDetalle> {
    // The response has `materias` (we filtered by name on the server, so
    // typically one entry). Extract its submenus.
    let materias = match raw.get("materias").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Vec::new(),
    };

    // Find the materia that exactly matches the requested name.
    for m in materias {
        if !m.is_object() {
            continue;
        }
        let name = m.get("materia").and_then(|v| v.as_str()).unwrap_or("");
        if name != materia_filter {
            continue;
        }

        if let Some(submenus_arr) = m.get("submenus").and_then(|v| v.as_array()) {
            return submenus_arr
                .iter()
                .filter_map(|s| {
                    if !s.is_object() {
                        return None;
                    }
                    let nombre = s.get("submenu").and_then(|v| v.as_str()).unwrap_or("");
                    Some(templates::SubmenuDetalle {
                        nombre: nombre.to_string(),
                        total_actividades: s
                            .get("total_actividades")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize,
                        actividades_calificadas: s
                            .get("actividades_calificadas")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize,
                        subtotal: s.get("subtotal").and_then(|v| v.as_f64()),
                        actividades: s
                            .get("actividades")
                            .and_then(|v| v.as_array())
                            .map(|arr| parse_actividades_list(arr))
                            .unwrap_or_default(),
                    })
                })
                .collect();
        }
    }
    Vec::new()
}

/// Parse the `actividades` array inside a submenu entry.
fn parse_actividades_list(arr: &[Value]) -> Vec<templates::ActividadDetalle> {
    arr.iter()
        .filter_map(|a| {
            if !a.is_object() {
                return None;
            }
            let nombre = a
                .get("actividad")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if nombre.is_empty() {
                return None;
            }
            Some(templates::ActividadDetalle {
                nombre,
                descripcion: a
                    .get("descripcion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                fecha: a
                    .get("fecha")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                nota: a.get("nota").and_then(|v| v.as_f64()),
                calificada: a
                    .get("calificada")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                submenu: a
                    .get("submenu")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}

// ── Índice de notas de la app (BD real) ───────────────────────────────

/// Máximo de notas leídas de la tabla `notes` para resolver enlaces.
/// Se toman las más reciente; si el usuario tiene más, las más antiguas
/// simplemente no se enlazan (nunca un enlace muerto).
const MAX_NOTES_SCAN: i64 = 500;

/// Índice de las notas que existen de verdad en la app.
///
/// - `by_slug` → clave = forma slugificada (`Note::slugify`) del slug o del
///   título de la nota. Valor = slug real de la fila de `notes`.
/// - `by_key` → clave = clave normalizada (sin acentos, minúsculas, espacios
///   colapsados) del título o del slug. Valor = slug real de la fila.
#[derive(Debug, Default)]
struct NoteIndex {
    by_slug: HashMap<String, String>,
    by_key: HashMap<String, String>,
}

impl NoteIndex {
    /// Indexa una nota de la BD (slug + título).
    fn insert(&mut self, slug: &str, title: &str) {
        if slug.is_empty() {
            return;
        }
        let slug = slug.to_string();

        let slug_forms = [slug.clone(), Note::slugify(&slug), Note::slugify(title)];
        for form in slug_forms.iter() {
            if !form.is_empty() {
                self.by_slug
                    .entry(form.clone())
                    .or_insert_with(|| slug.clone());
            }
        }

        let key_forms = [normalize_key(title), normalize_key(&slug)];
        for form in key_forms.iter() {
            if !form.is_empty() {
                self.by_key
                    .entry(form.clone())
                    .or_insert_with(|| slug.clone());
            }
        }
    }

    /// Devuelve el slug REAL de la nota que corresponde a `name`, o `None`
    /// si la BD no tiene ninguna nota con ese nombre (→ texto plano).
    fn find(&self, name: &str) -> Option<&str> {
        let slug = Note::slugify(name);
        if !slug.is_empty() {
            if let Some(found) = self.by_slug.get(&slug) {
                return Some(found.as_str());
            }
        }
        let key = normalize_key(name);
        if !key.is_empty() {
            if let Some(found) = self.by_key.get(&key) {
                return Some(found.as_str());
            }
        }
        None
    }
}

/// Lee las notas existentes de la BD (`notes`) para poder enlazar solo
/// slugs reales. `None` = la BD no pudo leerse (degradación honesta: la
/// página muestra "Notas no disponibles" en vez de enlaces rotos).
async fn load_note_index(pool: &sqlx::SqlitePool) -> Option<NoteIndex> {
    let rows = sqlx::query("SELECT slug, title FROM notes ORDER BY updated_at DESC LIMIT ?")
        .bind(MAX_NOTES_SCAN)
        .fetch_all(pool)
        .await
        .ok()?;

    let mut index = NoteIndex::default();
    for row in rows {
        let slug: String = match row.try_get("slug") {
            Ok(s) => s,
            Err(_) => continue,
        };
        let title: String = row.try_get("title").unwrap_or_default();
        index.insert(&slug, &title);
    }
    Some(index)
}

/// Clave de comparación: sin acentos, minúsculas y espacios colapsados.
fn normalize_key(s: &str) -> String {
    normalize_name(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Render de la página ───────────────────────────────────────────────

/// Formatea una nota con un decimal.
fn fmt_grade(g: f64) -> String {
    format!("{:.1}", g)
}

/// Color de la nota según su valor (escala 0–10).
fn grade_color(g: f64) -> &'static str {
    if g >= 7.0 {
        "text-green-400"
    } else if g >= 4.0 {
        "text-yellow-400"
    } else {
        "text-red-400"
    }
}

/// Percent-encode para usar un slug en un segmento de ruta.
fn url_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// Renderiza la página de detalle de la asignatura.
///
/// Cada nota/tarea/evento ya cargado aparece clicable **solo** si hay una
/// nota real en la BD (`notes`); si no hay, se muestra como texto plano con
/// un badge honesto ("Sin nota") y el submenú informa de cuántos están
/// enlazados de verdad.
#[allow(clippy::too_many_arguments)]
fn subject_page_html(
    connected: bool,
    materia: &str,
    codigo: &str,
    promedio_fallback: f64,
    maximo_fallback: f64,
    trimestre: u8,
    bloqueada_fallback: bool,
    detalle: &[templates::MateriaDetalle],
    submenus: &[templates::SubmenuDetalle],
    notes: Option<&NoteIndex>,
) -> String {
    let status = templates::mcp_status_banner(connected);

    // Valores visibles: preferimos el detalle, caemos a los promedios.
    let (avg, maximo, bloqueada, es_cualitativa, observacion) = detalle.first().map_or_else(
        || {
            (
                promedio_fallback,
                maximo_fallback,
                bloqueada_fallback,
                false,
                String::new(),
            )
        },
        |d| {
            (
                d.nota_promedio.unwrap_or(promedio_fallback),
                d.maximo,
                d.bloqueada,
                d.es_cualitativa,
                d.observacion.clone(),
            )
        },
    );

    let avg_fmt = fmt_grade(avg);
    let color = grade_color(avg);
    let bar_width = ((avg / 10.0) * 100.0).clamp(0.0, 100.0).round() as i64;
    let maximo_fmt = fmt_grade(maximo);
    let bloqueada_txt = if bloqueada { "Sí" } else { "No" };

    // Nota de la asignatura: solo si existe una nota con ese nombre.
    let subject_note_html = match notes.and_then(|ix| ix.find(materia)) {
        Some(slug) => format!(
            r###"<a href="/app/notes/{}" class="inline-flex items-center gap-1 mt-space-xs font-label-sm text-[11px] text-primary-container bg-primary-container/10 px-space-sm py-0.5 rounded-full hover:bg-primary-container/20 transition-colors"><span class="material-symbols-outlined text-[14px]">sticky_note_2</span> Nota de la asignatura</a>"###,
            url_encode_path(slug)
        ),
        None => String::new(),
    };

    let header = format!(
        r###"<header class="nova-subject-detail-hero"><a href="/dashboard/subjects" class="nova-back-link"><span class="material-symbols-outlined">arrow_back</span>Todas las asignaturas</a><p class="nova-eyebrow"><span>03.1</span> Detalle académico</p><h1>{}</h1><p>Código {} · Trimestre {}</p>{}</header>"###,
        templates::html_escape(materia),
        templates::html_escape(codigo),
        trimestre,
        subject_note_html,
    );

    let summary_card = format!(
        r###"<section class="nova-subject-summary"><div class="nova-subject-summary-grade"><span>PROMEDIO ACTUAL</span><strong class="{}">{}</strong><p>sobre {}</p><div class="nova-subject-progress"><div class="{}" style="width: {}%"></div></div></div><div class="nova-subject-summary-facts"><div><span>Calificación</span><strong>{}</strong></div><div><span>Código</span><strong>{}</strong></div><div><span>Máximo</span><strong>{}</strong></div><div><span>Bloqueada</span><strong>{}</strong></div><div><span>Tipo</span><strong>{}</strong></div></div></section>"###,
        color,
        avg_fmt,
        maximo_fmt,
        color,
        bar_width,
        templates::html_escape(&avg_fmt),
        templates::html_escape(codigo),
        maximo_fmt,
        bloqueada_txt,
        if es_cualitativa {
            "Cualitativa"
        } else {
            "Cuantitativa"
        },
    );

    let observacion_card = if observacion.is_empty() {
        String::new()
    } else {
        format!(
            r###"<aside class="nova-subject-observation"><span class="material-symbols-outlined">sticky_note_2</span><div><h2>Observación</h2><p>{}</p></div></aside>"###,
            templates::html_escape(&observacion),
        )
    };

    // Actividades (notas/tareas/eventos) agrupadas por submenú.
    let activities_html = if submenus.is_empty() {
        templates::empty_state_html(
            "No hay actividades registradas para esta asignatura.",
            "/dashboard/subjects",
        )
    } else {
        let mut parts: Vec<String> = Vec::new();
        for sm in submenus {
            let mut con_nota = 0usize;
            let rows: String = sm
                .actividades
                .iter()
                .map(|a| {
                    let nota_str = match a.nota {
                        Some(n) => fmt_grade(n),
                        None => "—".to_string(),
                    };
                    let nota_cls = match a.nota {
                        Some(n) if n >= 7.0 => "text-green-400",
                        Some(n) if n >= 4.0 => "text-yellow-400",
                        Some(_) => "text-red-400",
                        None => "text-on-surface-variant",
                    };
                    // Un solo origen de verdad para el estilo de los chips.
                    let estado = if a.calificada {
                        templates::status_chip(true, "Calificada")
                    } else {
                        templates::status_chip(false, "Pendiente")
                    };

                    // Enlace a la nota de la app: solo si el slug existe en `notes`.
                    let (nombre_cell, nota_badge) =
                        match notes.and_then(|ix| ix.find(&a.nombre)) {
                            Some(slug) => {
                                con_nota += 1;
                                let href = url_encode_path(slug);
                                (
                                    format!(
                                        r###"<a href="/app/notes/{}" class="inline-flex items-center gap-1 font-medium text-primary-container hover:text-primary-fixed-dim hover:underline">{} <span class="material-symbols-outlined text-[14px]">open_in_new</span></a>"###,
                                        href,
                                        templates::html_escape(&a.nombre)
                                    ),
                                    format!(
                                        r###"<a href="/app/notes/{}" title="Abrir la nota de la app" class="inline-flex items-center hover:opacity-80 transition-opacity">{}</a>"###,
                                        href,
                                        templates::status_chip(true, "Con nota")
                                    ),
                                )
                            }
                            None => {
                                let badge = if notes.is_some() {
                                    templates::status_chip(false, "Sin nota")
                                } else {
                                    // Estado no comparable: texto plano, sin chip verde/ámbar.
                                    r###"<span class="font-label-sm text-[10px] text-on-surface-variant" title="No se pudo leer la tabla de notas">Notas no disponibles</span>"###
                                        .to_string()
                                };
                                (
                                    format!(
                                        r###"<span class="text-on-surface">{}</span>"###,
                                        templates::html_escape(&a.nombre)
                                    ),
                                    badge.to_string(),
                                )
                            }
                        };

                    format!(
                        r###"<tr class="border-b border-outline/40">
                        <td class="py-space-xs font-body-sm text-body-sm text-on-surface">{}</td>
                        <td class="py-space-xs font-body-sm text-body-sm text-on-surface-variant">{}</td>
                        <td class="py-space-xs font-body-sm text-body-sm text-on-surface-variant">{}</td>
                        <td class="py-space-xs font-headline-sm text-headline-sm font-bold {}">{}</td>
                        <td class="py-space-xs">{}</td>
                        <td class="py-space-xs">{}</td>
                    </tr>"###,
                        nombre_cell,
                        templates::html_escape(&a.fecha),
                        templates::html_escape(&a.descripcion),
                        nota_cls,
                        nota_str,
                        estado,
                        nota_badge,
                    )
                })
                .collect::<Vec<_>>()
                .join("");

            let subtotal_str = sm.subtotal.map_or("—".to_string(), |s| fmt_grade(s));
            let mostradas = sm.actividades.len();
            let enlace_txt = match notes {
                Some(_) => format!("{} de {} enlazadas a notas", con_nota, mostradas),
                None => "Notas de la app no disponibles".to_string(),
            };

            let section = format!(
                r###"<section class="nova-subject-activities">
                    <div class="nova-activity-head">
                        <h3 class="font-headline-sm text-headline-sm text-primary-container">{}</h3>
                        <span class="font-label-sm text-[10px] text-on-surface-variant">{} actividades · {} calificadas · subtotal: {} · {}</span>
                    </div>
                    <div class="nova-table-scroll">
                        <table class="nova-activity-table">
                            <thead>
                                <tr class="border-b border-outline/40">
                                    <th class="text-left py-2 font-label-sm text-label-sm text-on-surface-variant">Actividad</th>
                                    <th class="text-left py-2 font-label-sm text-label-sm text-on-surface-variant">Fecha</th>
                                    <th class="text-left py-2 font-label-sm text-label-sm text-on-surface-variant">Descripción</th>
                                    <th class="text-left py-2 font-label-sm text-label-sm text-on-surface-variant">Nota</th>
                                    <th class="text-left py-2 font-label-sm text-label-sm text-on-surface-variant">Estado</th>
                                    <th class="text-left py-2 font-label-sm text-label-sm text-on-surface-variant">Nota de la app</th>
                                </tr>
                            </thead>
                            <tbody>{}</tbody>
                        </table>
                    </div>
                </section>"###,
                templates::html_escape(&sm.nombre),
                sm.total_actividades,
                sm.actividades_calificadas,
                subtotal_str,
                enlace_txt,
                rows,
            );
            parts.push(section);
        }
        parts.join("")
    };

    let content = format!(
        r###"<div class="nova-page nova-subject-detail-page">{}{}{}{}{}</div>"###,
        status, header, summary_card, observacion_card, activities_html
    );

    templates::page_ext(
        &format!("{} — Study Platform", materia),
        "Asignaturas",
        "/dashboard/subjects",
        &content,
    )
}

// ── Not-found page ────────────────────────────────────────────────────

fn not_found_page(code: &str, connected: bool) -> String {
    let status = templates::mcp_status_banner(connected);
    let content = format!(
        r###"<div class="nova-page nova-subject-detail-page nova-subject-missing">
            {status}
            <header class="nova-subject-detail-hero">
                <a href="/dashboard/subjects" class="nova-back-link"><span class="material-symbols-outlined">arrow_back</span>Todas las asignaturas</a>
                <p class="nova-eyebrow"><span>03.1</span> Detalle académico</p>
                <h1>Asignatura no encontrada.<br><em>Revisa el código.</em></h1>
                <p>No hay ninguna asignatura con código <strong>{code}</strong> en los datos de Mikareno.</p>
            </header>
            <section class="nova-subject-missing-card">
                <span class="nova-subject-missing-icon material-symbols-outlined">search_off</span>
                <div>
                    <p class="nova-eyebrow"><span>Sin coincidencias</span> Mikareno</p>
                    <h2>El curso sigue donde lo dejaste.</h2>
                    <p>Comprueba el identificador o regresa al listado para abrir una de las asignaturas disponibles.</p>
                    <a href="/dashboard/subjects" class="nova-button nova-button-primary"><span class="material-symbols-outlined">arrow_back</span>Explorar asignaturas</a>
                </div>
            </section>
        </div>"###,
        status = status,
        code = templates::html_escape(code),
    );

    templates::page_ext(
        "Asignatura no encontrada — Study Platform",
        "Asignaturas",
        "/dashboard/subjects",
        &content,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::academic::templates;
    use serde_json::json;

    #[test]
    fn normalize_name_strips_accents() {
        assert_eq!(normalize_name("Matemáticas"), "matematicas");
        assert_eq!(normalize_name("INFORMATICA"), "informatica");
        assert_eq!(normalize_name("Ñoño"), "nono");
        assert_eq!(normalize_name(""), "");
    }

    #[test]
    fn test_extract_trimestre_from_partial() {
        assert_eq!(extract_trimestre_from_partial(Some("2026:T3:1")), 3);
        assert_eq!(extract_trimestre_from_partial(Some("2026:T1:2")), 1);
        assert_eq!(extract_trimestre_from_partial(None), 0);
        assert_eq!(extract_trimestre_from_partial(Some("invalid")), 0);
    }

    #[test]
    fn parse_detalle_materia_extracts_fields() {
        let raw = json!({
            "materias_detalladas": [{
                "materia": "INFORMATICA",
                "codigo": "INFO01",
                "maximo": 10,
                "nota_promedio": 8.5,
                "parcial_id": "2026:T3:1",
                "bloqueada": false,
                "apto_cuantitativa": true
            }],
            "total": 1
        });
        let d = parse_detalle_materia(&raw, "INFORMATICA");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].materia, "INFORMATICA");
        assert_eq!(d[0].codigo, "INFO01");
        assert_eq!(d[0].nota_promedio, Some(8.5));
        assert!(!d[0].es_cualitativa);
    }

    #[test]
    fn parse_detalle_materia_qualitative() {
        let raw = json!({
            "materias_detalladas": [{
                "materia": "Educacion Fisica",
                "codigo": "EF01",
                "tipo": "cualitativa",
                "observacion": "Apto",
                "bloqueada": true
            }],
            "total": 1
        });
        let d = parse_detalle_materia(&raw, "Educacion Fisica");
        assert_eq!(d.len(), 1);
        assert!(d[0].es_cualitativa);
        assert_eq!(d[0].observacion, "Apto");
        assert!(d[0].bloqueada);
    }

    #[test]
    fn parse_detalle_materia_empty() {
        let raw = json!({"materias_detalladas": [], "total": 0});
        assert!(parse_detalle_materia(&raw, "TEST").is_empty());
    }

    #[test]
    fn parse_detalle_materia_filters_substring() {
        // Server does substring matching: "Física" matches both
        // "Física" and "Educación Física". The parser must filter to
        // the exact match only.
        let raw = json!({
            "materias_detalladas": [
                {"materia": "Educacion Fisica", "codigo": "EF", "nota_promedio": 2.0, "maximo": 10, "bloqueada": false},
                {"materia": "Física", "codigo": "FIS", "nota_promedio": 3.0, "maximo": 10, "bloqueada": false}
            ],
            "total": 2
        });
        let d = parse_detalle_materia(&raw, "Física");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].materia, "Física");
        assert_eq!(d[0].codigo, "FIS");
    }

    #[test]
    fn parse_actividades_materia_groups_by_submenu() {
        let raw = json!({
            "trimestre": 1,
            "materias": [{
                "materia": "INFORMATICA",
                "profesor": "Prof. X",
                "submenus": [{
                    "submenu": "TAREAS",
                    "total_actividades": 2,
                    "actividades_calificadas": 1,
                    "subtotal": 9.0,
                    "actividades": [
                        {"actividad": "Proyecto", "descripcion": "Web", "fecha": "2024-03-01", "nota": 9.0, "calificada": true, "profesor": "Prof. X"},
                        {"actividad": "Ejercicio", "descripcion": "", "fecha": "2024-03-05", "nota": null, "calificada": false, "profesor": "Prof. X"}
                    ]
                }]
            }],
            "total_materias": 1
        });
        let subs = parse_actividades_materia(&raw, "INFORMATICA");
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].nombre, "TAREAS");
        assert_eq!(subs[0].total_actividades, 2);
        assert_eq!(subs[0].actividades_calificadas, 1);
        assert_eq!(subs[0].subtotal, Some(9.0));
        assert_eq!(subs[0].actividades.len(), 2);
        assert_eq!(subs[0].actividades[0].nombre, "Proyecto");
        assert_eq!(subs[0].actividades[0].nota, Some(9.0));
        assert!(subs[0].actividades[0].calificada);
        assert_eq!(subs[0].actividades[1].nota, None);
        assert!(!subs[0].actividades[1].calificada);
    }

    #[test]
    fn parse_actividades_materia_empty() {
        let raw = json!({"trimestre": 1, "materias": [], "total_materias": 0});
        assert!(parse_actividades_materia(&raw, "TEST").is_empty());
    }

    #[test]
    fn resolve_from_promedios_finds_by_code() {
        let raw = json!({
            "promedios": {
                "stats": [{
                    "courseName": "INFORMATICA",
                    "courseCode": "INFO01",
                    "average": 8.5,
                    "maxScore": 10,
                    "partialId": "2026:T3:1",
                    "isLocked": false
                }]
            }
        });
        let mut materia = String::new();
        let mut promedio = 0.0f64;
        let mut maximo = 0.0f64;
        let mut trimestre = 0u8;
        let mut bloqueada = false;
        assert!(resolve_from_promedios(
            &raw,
            "INFO01",
            &mut materia,
            &mut promedio,
            &mut maximo,
            &mut trimestre,
            &mut bloqueada
        ));
        assert_eq!(materia, "INFORMATICA");
        assert_eq!(promedio, 8.5);
        assert_eq!(maximo, 10.0);
        assert_eq!(trimestre, 3);
        assert!(!bloqueada);
    }

    #[test]
    fn resolve_from_promedios_finds_by_name_fallback() {
        let raw = json!({
            "promedios": {
                "stats": [{
                    "courseName": "Matematicas",
                    "courseCode": "MATE01",
                    "average": 7.0,
                    "maxScore": 10,
                    "partialId": "2026:T2:1",
                    "isLocked": true
                }]
            }
        });
        let mut materia = String::new();
        let mut promedio = 0.0f64;
        let mut maximo = 0.0f64;
        let mut trimestre = 0u8;
        let mut bloqueada = false;
        // Match by normalized name (accents stripped).
        assert!(resolve_from_promedios(
            &raw,
            "Matemáticas",
            &mut materia,
            &mut promedio,
            &mut maximo,
            &mut trimestre,
            &mut bloqueada
        ));
        assert_eq!(materia, "Matematicas");
        assert_eq!(trimestre, 2);
        assert!(bloqueada);
    }

    #[test]
    fn resolve_from_promedios_not_found() {
        let raw = json!({"promedios": {"stats": []}});
        let mut materia = String::new();
        let mut promedio = 0.0f64;
        let mut maximo = 0.0f64;
        let mut trimestre = 0u8;
        let mut bloqueada = false;
        assert!(!resolve_from_promedios(
            &raw,
            "NOEXISTE",
            &mut materia,
            &mut promedio,
            &mut maximo,
            &mut trimestre,
            &mut bloqueada
        ));
    }

    #[test]
    fn resolve_from_promedios_no_promedios() {
        let raw = json!({});
        let mut materia = String::new();
        let mut promedio = 0.0f64;
        let mut maximo = 0.0f64;
        let mut trimestre = 0u8;
        let mut bloqueada = false;
        assert!(!resolve_from_promedios(
            &raw,
            "CODE",
            &mut materia,
            &mut promedio,
            &mut maximo,
            &mut trimestre,
            &mut bloqueada
        ));
    }

    #[test]
    fn extract_parcial_num_from_partial_id() {
        let full = json!({
            "promedios": {
                "stats": [{
                    "courseName": "INFORMATICA",
                    "partialId": "2026:T3:1"
                }]
            }
        });
        assert_eq!(extract_parcial_num(&full, "INFORMATICA", 3), 1);
    }

    #[test]
    fn extract_parcial_num_default_on_missing() {
        let full = json!({"promedios": {"stats": []}});
        assert_eq!(extract_parcial_num(&full, "NOEXISTE", 0), 1);
    }

    // ── Notas de la app ───────────────────────────────────────────────

    fn submenu_demo() -> Vec<templates::SubmenuDetalle> {
        vec![templates::SubmenuDetalle {
            nombre: "TAREAS".to_string(),
            total_actividades: 1,
            actividades_calificadas: 1,
            subtotal: Some(7.5),
            actividades: vec![templates::ActividadDetalle {
                nombre: "Ensayo".to_string(),
                descripcion: "trabajo en clase".to_string(),
                fecha: "2026-03-01".to_string(),
                nota: Some(7.5),
                calificada: true,
                submenu: "TAREAS".to_string(),
            }],
        }]
    }

    #[test]
    fn note_index_only_resolves_existing_notes() {
        let mut ix = NoteIndex::default();
        ix.insert("parcial-1", "Parcial 1");
        assert_eq!(ix.find("Parcial 1"), Some("parcial-1"));
        assert_eq!(ix.find("parcial 1"), Some("parcial-1"));
        // No existe en la BD → sin enlace.
        assert_eq!(ix.find("Examen Final"), None);
    }

    #[test]
    fn note_index_matches_normalized_title() {
        let mut ix = NoteIndex::default();
        ix.insert("matematicas", "Matematicas");
        // La actividad trae acentes y la nota no: la clave normalizada enlaza.
        assert_eq!(ix.find("Matemáticas"), Some("matematicas"));
    }

    #[test]
    fn normalize_key_collapses_whitespace() {
        assert_eq!(normalize_key("  Parcial   1 "), "parcial 1");
        assert_eq!(normalize_key("Matemáticas"), "matematicas");
        assert_eq!(normalize_key(""), "");
    }

    #[test]
    fn page_links_only_real_note_slugs() {
        let mut ix = NoteIndex::default();
        ix.insert("ensayo", "Ensayo");
        let html = subject_page_html(
            true,
            "INFORMATICA",
            "INFO01",
            8.0,
            10.0,
            3,
            false,
            &[],
            &submenu_demo(),
            Some(&ix),
        );
        assert!(html.contains("/app/notes/ensayo"));
        assert!(html.contains("Con nota"));
    }

    #[test]
    fn page_shows_plain_text_when_note_missing() {
        let ix = NoteIndex::default();
        let html = subject_page_html(
            true,
            "INFORMATICA",
            "INFO01",
            8.0,
            10.0,
            3,
            false,
            &[],
            &submenu_demo(),
            Some(&ix),
        );
        // Sin enlaces muertos.
        assert!(!html.contains("/app/notes/"));
        assert!(html.contains("Sin nota"));
    }

    #[test]
    fn page_reports_when_notes_db_is_unavailable() {
        let html = subject_page_html(
            true,
            "INFORMATICA",
            "INFO01",
            8.0,
            10.0,
            3,
            false,
            &[],
            &submenu_demo(),
            None,
        );
        assert!(!html.contains("/app/notes/"));
        assert!(html.contains("Notas no disponibles"));
    }
}
