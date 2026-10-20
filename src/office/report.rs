//! Informe del estudiante exportado a `.docx` mediante el MCP de GenOffice.
//!
//! El informe se construye **solo con datos reales**: los que la app ya
//! conoce (notas y promedios del MCP de Mikareno, apuntes de la base SQLite).
//! Cada sección es opcional y distingue dos situaciones:
//!
//! * `None`  → la fuente no está disponible (MCP caído, tabla inexistente…),
//!             y el documento lo dice explícitamente.
//! * `Some(vec![])` → la fuente respondió y no tiene registros.
//!
//! Nunca se rellenan datos inventados. Si el MCP de GenOffice no está
//! disponible, `GET /office/report` responde `503` con
//! `{"error":"GenOffice no disponible","motivo":"…"}` — sin `.docx` de
//! reemplazo y sin HTML de error.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use tokio::time::{timeout, Duration};

use crate::office::genoffice::{GenOfficeClient, OfficeError};
use crate::office::templates;
use crate::AppState;

/// MIME oficial de un `.docx` (OOXML WordprocessingML).
const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// Nombre del archivo descargado (ASCII: `Content-Disposition` simple).
const REPORT_FILENAME: &str = "informe-estudiante.docx";

/// Timeout de cada llamada al MCP de Mikareno al reunir los datos.
const MIKARENO_TIMEOUT_SECS: u64 = 10;

/// Margen para entregar el `.docx` cuando el sondeo previo falló (el MCP
/// puede estar arrancando): si no responde en este plazo, 503 honesto.
const EXPORT_DEADLINE_SECS: u64 = 20;

/// Máximo de apuntes (tabla `notes`) incluidos en el informe.
const MAX_NOTES_ROWS: i64 = 50;

/// Promedio por materia (origen: MCP de Mikareno).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Promedio {
    pub materia: String,
    pub promedio: f64,
    pub trimestre: u8,
    #[serde(default)]
    pub codigo: String,
}

/// Nota individual (origen: MCP de Mikareno).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotaRow {
    pub materia: String,
    pub nota: f64,
    pub trimestre: u8,
}

/// Tarea escolar (origen: MCP de Mikareno).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TareaRow {
    pub titulo: String,
    pub vencimiento: String,
    pub estado: String,
}

/// Apunte del segundo cerebro (origen: tabla `notes` de SQLite).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApuntesRow {
    pub title: String,
    pub tags: String,
    pub word_count: i64,
    pub updated_at: String,
}

/// Datos estructurados del informe.
///
/// Todos los bloques son opcionales: `None` significa "fuente no
/// disponible" y `Some(vec![])` significa "la fuente respondió sin datos".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReportData {
    /// Marca de tiempo (RFC 3339) de generación del informe.
    pub generado_en: String,
    pub estudiante: Option<String>,
    pub institucion: Option<String>,
    pub curso: Option<String>,
    pub promedios: Option<Vec<Promedio>>,
    pub promedios_fuente: Option<String>,
    pub notas: Option<Vec<NotaRow>>,
    pub notas_fuente: Option<String>,
    pub tareas: Option<Vec<TareaRow>>,
    pub tareas_fuente: Option<String>,
    pub apuntes: Option<Vec<ApuntesRow>>,
    pub apuntes_fuente: Option<String>,
}

/// Construye el `.docx` del informe usando el MCP de GenOffice.
///
/// `data` lleva los datos reales ya recogidos; `work_dir` es el directorio
/// donde el MCP escribe el `.docx` (debe ser visible para la app).
/// Devuelve los bytes del documento generado.
///
/// Errores: [`OfficeError::NotConfigured`] si la URL está vacía,
/// [`OfficeError::Unavailable`] si el MCP no responde, y el resto si la
/// herramienta falla o el archivo no aparece.
pub async fn export_docx_report(
    mcp_url: &str,
    data: &ReportData,
    work_dir: &Path,
) -> Result<Vec<u8>, OfficeError> {
    if mcp_url.trim().is_empty() {
        return Err(OfficeError::NotConfigured);
    }

    let markdown = render_markdown(data);

    // El MCP escribe el archivo: necesita un directorio existente y una ruta
    // absoluta (su directorio de trabajo puede ser otro).
    std::fs::create_dir_all(work_dir).map_err(|e| OfficeError::Io(e.to_string()))?;
    let abs_dir = std::fs::canonicalize(work_dir).unwrap_or_else(|_| work_dir.to_path_buf());
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let out = abs_dir.join(format!("informe-estudiante-{stamp}.docx"));

    let client = GenOfficeClient::new(mcp_url.to_string());
    client.initialize().await?;
    client.create_docx(&markdown, &out).await?;

    let bytes = std::fs::read(&out).map_err(|e| {
        OfficeError::Io(format!(
            "el MCP no dejó el documento en {}: {e}",
            out.display()
        ))
    })?;
    let _ = std::fs::remove_file(&out);

    if bytes.is_empty() {
        return Err(OfficeError::Io("el documento generado está vacío".into()));
    }
    Ok(bytes)
}

/// Renderiza el informe como Markdown (GFM) para `create_docx`.
pub fn render_markdown(d: &ReportData) -> String {
    let mut out = String::new();

    out.push_str("# Informe del estudiante\n\n");
    out.push_str(&format!(
        "Generado el {} por Study Platform. Cada sección indica su fuente; \
         las secciones sin datos disponibles se marcan como tales.\n\n",
        esc(&d.generado_en)
    ));

    out.push_str("## Identificación\n\n");
    out.push_str("| Dato | Valor |\n| --- | --- |\n");
    out.push_str(&format!("| Estudiante | {} |\n", opt_cell(&d.estudiante)));
    out.push_str(&format!("| Institución | {} |\n", opt_cell(&d.institucion)));
    out.push_str(&format!("| Curso | {} |\n", opt_cell(&d.curso)));
    out.push('\n');

    out.push_str("## Promedios por materia\n\n");
    match &d.promedios {
        Some(rows) if !rows.is_empty() => {
            out.push_str(
                "| Materia | Código | Trimestre | Promedio |\n| --- | --- | --- | --- |\n",
            );
            for p in rows {
                out.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    esc(&p.materia),
                    esc(&p.codigo),
                    p.trimestre,
                    p.promedio
                ));
            }
        }
        Some(_) => out.push_str("_La fuente no devolvió promedios._\n"),
        None => out.push_str("_Promedios no disponibles: la fuente académica no respondió._\n"),
    }
    push_source(&mut out, &d.promedios_fuente);

    out.push_str("## Notas por materia\n\n");
    match &d.notas {
        Some(rows) if !rows.is_empty() => {
            out.push_str("| Materia | Nota | Trimestre |\n| --- | --- | --- |\n");
            for n in rows {
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    esc(&n.materia),
                    n.nota,
                    n.trimestre
                ));
            }
        }
        Some(_) => out.push_str("_La fuente no devolvió notas._\n"),
        None => out.push_str("_Notas no disponibles: la fuente académica no respondió._\n"),
    }
    push_source(&mut out, &d.notas_fuente);

    out.push_str("## Tareas\n\n");
    match &d.tareas {
        Some(rows) if !rows.is_empty() => {
            out.push_str("| Tarea | Vencimiento | Estado |\n| --- | --- | --- |\n");
            for t in rows {
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    esc(&t.titulo),
                    esc(&t.vencimiento),
                    esc(&t.estado)
                ));
            }
        }
        Some(_) => out.push_str("_La fuente no devolvió tareas._\n"),
        None => out.push_str("_Tareas no disponibles: la fuente académica no respondió._\n"),
    }
    push_source(&mut out, &d.tareas_fuente);

    out.push_str("## Apuntes\n\n");
    match &d.apuntes {
        Some(rows) if !rows.is_empty() => {
            out.push_str(
                "| Título | Etiquetas | Palabras | Actualizado |\n| --- | --- | --- | --- |\n",
            );
            for a in rows {
                out.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    esc(&a.title),
                    esc(&a.tags),
                    a.word_count,
                    esc(&a.updated_at)
                ));
            }
        }
        Some(_) => out.push_str("_No hay apuntes registrados._\n"),
        None => out.push_str("_Apuntes no disponibles: no se pudo leer la base de datos._\n"),
    }
    push_source(&mut out, &d.apuntes_fuente);

    out
}

/// GET /office/report — descarga el informe del estudiante en `.docx`.
///
/// 200 + `.docx` (con `Content-Disposition: attachment`) cuando el MCP de
/// GenOffice genera el documento; **503 + JSON** si el MCP no está
/// configurado, no responde o falla. Nunca 500 ni HTML: si no hay documento,
/// no hay `.docx` de reemplazo.
pub async fn download(State(state): State<Arc<AppState>>) -> Response {
    let url = state.config.genoffice_mcp_url.trim().to_string();

    // 1) Sin URL no hay servidor: 503 inmediato, sin tocar la red.
    if url.is_empty() {
        tracing::warn!("informe: GENOFFICE_MCP_URL vacía");
        return unavailable("GENOFFICE_MCP_URL no está configurada");
    }

    // 2) Sondeo rápido (2,5 s): si el MCP está caído, no se pierden ~20 s
    //    reuniendo datos ni se espera al timeout del cliente MCP (90 s).
    //    Un sondeo fallido no es concluyente (el servidor puede estar
    //    arrancando): el envío real se intenta igualmente, pero acotado.
    let up = templates::probe_mcp(&url).await.is_some();
    if !up {
        tracing::warn!("informe: el MCP de GenOffice no respondió al sondeo");
    }

    // 3) Datos reales + .docx generado por el MCP.
    let data = collect_report_data(&state).await;
    let work_dir = work_dir(&state);

    let export = export_docx_report(&url, &data, &work_dir);
    let result = if up {
        export.await
    } else {
        match timeout(Duration::from_secs(EXPORT_DEADLINE_SECS), export).await {
            Ok(r) => r,
            Err(_) => Err(OfficeError::Unavailable(format!(
                "el MCP de GenOffice no respondió en {EXPORT_DEADLINE_SECS} s"
            ))),
        }
    };

    match result {
        Ok(bytes) => templates::file_download(REPORT_FILENAME, DOCX_MIME, bytes),
        Err(e) => {
            tracing::warn!(error = %e, "informe: GenOffice no pudo entregar el .docx");
            unavailable(&e.to_string())
        }
    }
}

/// Cuerpo JSON de la respuesta 503 (función pura: sin red, sin estado).
///
/// Forma estable: `{"error":"GenOffice no disponible","motivo":"<detalle>"}`.
/// `error` no cambia nunca; `motivo` explica por qué (URL vacía, MCP caído,
/// fallo de la herramienta, documento no escrito…).
pub fn unavailable_body(motivo: &str) -> serde_json::Value {
    json!({ "error": "GenOffice no disponible", "motivo": motivo })
}

/// Respuesta 503 honesta: el MCP de GenOffice no entregó el documento.
///
/// No se devuelve ningún `.docx` de reemplazo ni HTML de error.
fn unavailable(motivo: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(unavailable_body(motivo)),
    )
        .into_response()
}

/// Reúne los datos reales del informe: apuntes de la BD SQLite y datos
/// académicos del MCP de Mikareno (misma fuente que `/dashboard/subjects`).
async fn collect_report_data(state: &AppState) -> ReportData {
    let mut d = ReportData {
        generado_en: chrono::Utc::now().to_rfc3339(),
        ..Default::default()
    };

    // Base de datos local: tabla `notes`.
    d.apuntes = load_notes(state).await;
    if d.apuntes.is_some() {
        d.apuntes_fuente = Some("Base de datos de Study Platform (SQLite)".to_string());
    }

    // Datos académicos: MCP de Mikareno.
    if let Some(client) = crate::academic::mikareno_client(state) {
        let connected = matches!(
            timeout(
                Duration::from_secs(MIKARENO_TIMEOUT_SECS),
                client.initialize()
            )
            .await,
            Ok(Ok(()))
        );

        if connected {
            if let Ok(Ok(p)) =
                timeout(Duration::from_secs(MIKARENO_TIMEOUT_SECS), client.perfil()).await
            {
                d.estudiante = non_empty(p.nombre);
                d.institucion = non_empty(p.institucion);
                d.curso = non_empty(p.curso);
            }

            match timeout(
                Duration::from_secs(MIKARENO_TIMEOUT_SECS),
                client.notas_full(),
            )
            .await
            {
                Ok(Ok(notas)) => {
                    d.promedios = Some(
                        notas
                            .iter()
                            .map(|n| Promedio {
                                materia: n.materia.clone(),
                                promedio: n.nota,
                                trimestre: n.trimestre,
                                codigo: n.codigo.clone(),
                            })
                            .collect(),
                    );
                    d.promedios_fuente = Some("MCP Mikareno (get_student_full)".to_string());
                    d.notas = Some(
                        notas
                            .iter()
                            .map(|n| NotaRow {
                                materia: n.materia.clone(),
                                nota: n.nota,
                                trimestre: n.trimestre,
                            })
                            .collect(),
                    );
                    d.notas_fuente = Some("MCP Mikareno (get_student_full)".to_string());
                }
                _ => {
                    // Fuente sin respuesta: se deja en `None` (no disponible).
                }
            }

            if let Ok(Ok(tareas)) =
                timeout(Duration::from_secs(MIKARENO_TIMEOUT_SECS), client.tareas()).await
            {
                d.tareas = Some(
                    tareas
                        .iter()
                        .map(|t| TareaRow {
                            titulo: t.titulo.clone(),
                            vencimiento: t.vencimiento.clone(),
                            estado: t.estado.clone(),
                        })
                        .collect(),
                );
                d.tareas_fuente = Some("MCP Mikareno (get_tareas)".to_string());
            }
        }
    }

    d
}

/// Lee los apuntes de la tabla `notes` (metadatos reales, sin inventar).
async fn load_notes(state: &AppState) -> Option<Vec<ApuntesRow>> {
    let rows = sqlx::query(
        "SELECT title, tags, word_count, updated_at FROM notes \
         ORDER BY updated_at DESC LIMIT ?",
    )
    .bind(MAX_NOTES_ROWS)
    .fetch_all(&state.db)
    .await
    .ok()?;

    Some(
        rows.into_iter()
            .map(|r| ApuntesRow {
                title: r.get::<String, _>("title"),
                tags: r.get::<String, _>("tags"),
                word_count: r.get::<i64, _>("word_count"),
                updated_at: r.get::<String, _>("updated_at"),
            })
            .collect(),
    )
}

/// Directorio donde el MCP de GenOffice escribe el `.docx`.
///
/// Por defecto `<db parent>/office/tmp`; se puede fijar con la variable de
/// entorno `GENOFFICE_WORK_DIR` (útil si el MCP corre en otro contenedor).
fn work_dir(state: &AppState) -> PathBuf {
    if let Ok(dir) = std::env::var("GENOFFICE_WORK_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    crate::office::office_root(state).join("tmp")
}

/// Texto de una celda: "no disponible" cuando el dato falta.
fn opt_cell(value: &Option<String>) -> String {
    match value {
        Some(v) if !v.trim().is_empty() => esc(v),
        _ => "no disponible".to_string(),
    }
}

/// Añade la línea de fuente de una sección (si la hay).
fn push_source(out: &mut String, source: &Option<String>) {
    if let Some(src) = source {
        out.push_str(&format!("\nFuente: {}\n", esc(src)));
    }
    out.push('\n');
}

/// `Some` solo si el texto no está vacío.
fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Escapa el texto para una celda de tabla Markdown.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\r', " ")
        .replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_body_shape_is_stable() {
        let body = unavailable_body("el MCP de GenOffice no responde");
        let obj = body.as_object().expect("el cuerpo 503 es un objeto JSON");
        assert_eq!(obj.len(), 2, "forma del cuerpo 503: {body}");
        assert_eq!(obj["error"], "GenOffice no disponible");
        assert_eq!(obj["motivo"], "el MCP de GenOffice no responde");
        // Nunca HTML ni un .docx de reemplazo: el cuerpo es JSON puro.
        let raw = serde_json::to_string(&body).unwrap();
        assert!(!raw.contains("<html"), "el 503 no debe ser HTML: {raw}");
    }

    #[test]
    fn markdown_marks_missing_sources() {
        let d = ReportData {
            generado_en: "2026-10-19T12:00:00Z".into(),
            ..Default::default()
        };
        let md = render_markdown(&d);
        assert!(md.contains("no disponibles"));
        assert!(md.contains("no disponible"));
        assert!(!md.contains("| Materia | Código | Trimestre | Promedio |"));
    }

    #[test]
    fn markdown_renders_real_promedios() {
        let d = ReportData {
            generado_en: "2026-10-19T12:00:00Z".into(),
            promedios: Some(vec![Promedio {
                materia: "Matemática".into(),
                promedio: 8.5,
                trimestre: 1,
                codigo: "MATE01".into(),
            }]),
            promedios_fuente: Some("MCP Mikareno".into()),
            ..Default::default()
        };
        let md = render_markdown(&d);
        assert!(md.contains("| Matemática | MATE01 | 1 | 8.5 |"));
        assert!(md.contains("Fuente: MCP Mikareno"));
    }

    #[test]
    fn empty_source_is_distinct_from_missing_source() {
        let d = ReportData {
            generado_en: "2026-10-19T12:00:00Z".into(),
            promedios: Some(Vec::new()),
            ..Default::default()
        };
        let md = render_markdown(&d);
        assert!(md.contains("_La fuente no devolvió promedios._"));
    }

    #[test]
    fn pipes_are_escaped_in_tables() {
        let d = ReportData {
            generado_en: "2026-10-19T12:00:00Z".into(),
            promedios: Some(vec![Promedio {
                materia: "Ciencias | Física".into(),
                promedio: 7.0,
                trimestre: 2,
                codigo: String::new(),
            }]),
            ..Default::default()
        };
        let md = render_markdown(&d);
        assert!(md.contains("Ciencias \\| Física"));
    }
}
