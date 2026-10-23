//! Datos del calendario: fuente Mikareno + anotaciones locales.
//!
//! ## Por qué este módulo existe
//!
//! El calendario tenía tres problemas que el usuario reportaba:
//!
//! 1. **No tenía base Mikareno**: la vista mensual usaba el tool
//!    `get_calendario`, que devuelve **43 días desde hoy** (días de clase y
//!    vacaciones). Al navegar a otro mes no había nada, y nunca aparecían las
//!    clases ni las tareas del estudiante. Ahora la fuente es
//!    `get_student_full` (la misma que ya usa el panel del dashboard), que
//!    trae `calendario[]`, `horario.periodMap` y `tareas_paginadas`.
//!
//! 2. **No se podían poner anotaciones**: Mikareno es read-only, así que las
//!    anotaciones son datos **locales** (tabla `calendar_annotations`).
//!
//! 3. **La navegación por meses era lenta**: cada render pagaba el timeout
//!    del MCP. Ahora el calendario lee de una caché en memoria compartida
//!    (TTL 180 s) y, cuando la entrada está expirada, sirve lo **rancio**
//!    al instante y refresca en segundo plano (stale-while-revalidate).
//!
//! ## Estructura de un día
//!
//! Un día del calendario puede tener varias fuentes, y todas se muestran:
//!
//! * **Clases** (`horario`): el horario semanal de Mikareno proyectado sobre
//!   las fechas del mes. Es lo que "pone" la agenda real del curso.
//! * **Eventos** (`calendario[]`): días de clase, vacaciones y eventos
//!   escolares.
//! * **Tareas** (`tareas_paginadas`): con fecha de vencimiento, se muestran
//!   el día que vencen.
//! * **Anotaciones** (`calendar_annotations`): notas del usuario.

use chrono::{Datelike, NaiveDate};
use serde_json::Value;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time;

use crate::academic::mikareno_client;
use crate::integrations::mikareno::{CalendarioEvent, Horario, Tarea};
use crate::AppState;

/// Presupuesto de la fase MCP cuando no hay nada en caché (5 s).
///
/// Es un presupuesto *único* para el handshake y la lectura. Si el MCP está
/// colgado, la vista degrada a un estado vacío honesto dentro de este plazo
/// en vez de quedarse esperando.
const TIMEOUT_DATOS: Duration = Duration::from_secs(5);

/// Tope del handshake `initialize` dentro de [`TIMEOUT_DATOS`].
const TIMEOUT_HANDSHAKE: Duration = Duration::from_secs(2);

/// Antigüedad máxima de la caché en memoria (2 min).
///
/// Más corta que la del cliente MCP (180 s) porque aquí se cachea el
/// *resultado combinado* (calendario + horario + tareas): es lo que consume
/// el calendario, así que la ventana debe ser corta para que un cambio en
/// Mikareno se vea pronto.
const TTL_CACHE: Duration = Duration::from_secs(120);

/// Elementos del calendario que se pintan en una celda (el resto va a "+N").
const MAX_EVENTOS_POR_DIA: usize = 3;

/// Una anotación local del usuario sobre un día.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Anotacion {
    pub id: i64,
    pub date: String,
    pub title: String,
    pub body: String,
}

/// Todo lo que se muestra en un día del calendario.
#[derive(Debug, Default, Clone)]
pub struct DiaData {
    /// Clases del horario de Mikareno para ese día.
    pub clases: Vec<String>,
    /// Eventos escolares (día de clase, vacaciones, evento puntual).
    pub eventos: Vec<CalendarioEvent>,
    /// Tareas que vencen ese día.
    pub tareas: Vec<Tarea>,
    /// Anotaciones escritas por el usuario.
    pub anotaciones: Vec<Anotacion>,
}

impl DiaData {
    /// ¿Hay algo que mostrar en el día?
    pub fn vacio(&self) -> bool {
        self.clases.is_empty()
            && self.eventos.is_empty()
            && self.tareas.is_empty()
            && self.anotaciones.is_empty()
    }
}

/// Datos combinados de Mikareno para el calendario.
#[derive(Debug, Default, Clone)]
pub struct DatosMikareno {
    /// Eventos del calendario escolar (día de clase, vacaciones...).
    pub eventos: Vec<CalendarioEvent>,
    /// Horario semanal (para proyectar clases sobre el mes).
    pub horario: Horario,
    /// Tareas con fecha de vencimiento.
    pub tareas: Vec<Tarea>,
    /// Momento en que se leyeron los datos (para la advertencia de frescura).
    pub leido: Option<Instant>,
    /// ¿Hubo error al leer? (para el aviso honesto)
    pub error: bool,
}

/// Caché en memoria de los datos de Mikareno del calendario.
///
/// Es un `Mutex` porque el estado de la app es `Clone` y se comparte por
/// `Arc`: la caché vive una sola vez por proceso.
#[derive(Default)]
pub struct CacheMikareno {
    inner: std::sync::Mutex<Option<DatosMikareno>>,
}

impl CacheMikareno {
    /// Copia cacheada, si existe.
    pub fn get(&self) -> Option<DatosMikareno> {
        self.inner.lock().ok().and_then(|g| g.clone())
    }

    /// Guarda una copia.
    pub fn set(&self, datos: DatosMikareno) {
        if let Ok(mut g) = self.inner.lock() {
            *g = Some(datos);
        }
    }
}

/// Proyecta el horario semanal de Mikareno sobre un día concreto.
///
/// El horario viene por nombre de día (`lunes`, `martes`, …). Se compara con
/// el nombre español del día de la fecha, así que el calendario muestra las
/// clases reales de cada día del mes.
fn dia_semana_es(d: NaiveDate) -> &'static str {
    match d.weekday() {
        chrono::Weekday::Mon => "lunes",
        chrono::Weekday::Tue => "martes",
        chrono::Weekday::Wed => "miércoles",
        chrono::Weekday::Thu => "jueves",
        chrono::Weekday::Fri => "viernes",
        chrono::Weekday::Sat => "sábado",
        chrono::Weekday::Sun => "domingo",
    }
}

/// Clases del horario para una fecha concreta.
fn clases_del_dia(horario: &Horario, fecha: NaiveDate) -> Vec<String> {
    if horario.dias.is_empty() {
        return Vec::new();
    }
    let dia = dia_semana_es(fecha);
    horario
        .dias
        .iter()
        .filter(|d| d.dia.eq_ignore_ascii_case(dia))
        .flat_map(|d| d.clases.clone())
        .collect()
}

/// Convierte una fecha de vencimiento de Mikareno a `YYYY-MM-DD`.
///
/// Mikareno usa formatos como `2026-10-20T18:00:00`, `2026-10-20` o
/// `20/10/2026`; se toman los 10 primeros dígitos cuando el formato es ISO.
fn fecha_vencimiento(tarea: &Tarea) -> Option<String> {
    let v = tarea.vencimiento.trim();
    if v.is_empty() {
        return None;
    }
    if v.len() >= 10 && v.as_bytes()[4] == b'-' && v.as_bytes()[7] == b'-' {
        return Some(v.chars().take(10).collect());
    }
    // Formato dd/mm/yyyy (el que usa Mikareno en algunos listados).
    let partes: Vec<&str> = v.split('/').collect();
    if partes.len() == 3 {
        let d = partes[0].trim();
        let m = partes[1].trim();
        let a = partes[2].trim();
        if let (Ok(dd), Ok(mm), Ok(aa)) = (d.parse::<u32>(), m.parse::<u32>(), a.parse::<i32>()) {
            if let Some(f) = NaiveDate::from_ymd_opt(aa, mm, dd) {
                return Some(f.format("%Y-%m-%d").to_string());
            }
        }
    }
    None
}

/// Lee los datos del calendario desde `get_student_full`.
///
/// Se prefiere este tool (uno solo) sobre `get_calendario` + `get_horario` +
/// `get_tareas` por separado: el scrape real ocurre una vez y trae las tres
/// cosas.
fn parsear_student_full(raw: &Value) -> DatosMikareno {
    let eventos = crate::handlers::stitch_panel::parse_calendario_public(raw);
    let horario = crate::handlers::stitch_panel::parse_horario_public(raw);
    let tareas = crate::handlers::stitch_panel::parse_tareas_public(raw);
    DatosMikareno {
        eventos,
        horario,
        tareas,
        leido: Some(Instant::now()),
        error: false,
    }
}

/// Devuelve los datos de Mikareno para el calendario, con caché.
///
/// Comportamiento (stale-while-revalidate):
/// * **Caché fresca** (< 2 min): se devuelve al instante, sin I/O.
/// * **Caché rancia**: se devuelve al instante y se refresca en background.
/// * **Sin caché**: se lee con un presupuesto de 5 s (degradando honesto).
pub async fn datos_mikareno(state: &Arc<AppState>) -> DatosMikareno {
    let cache = &state.cache_mikareno;
    let ahora = Instant::now();

    if let Some(datos) = cache.get() {
        let fresca = datos
            .leido
            .map(|t| ahora.duration_since(t) < TTL_CACHE)
            .unwrap_or(false);
        if fresca {
            return datos;
        }
        // Rancia: se sirve ya y se refresca en background (una sola vez).
        if let Some(client) = mikareno_client(state) {
            let cache2 = cache_ref(state);
            tokio::spawn(async move {
                let nuevo = leer_mikareno(&client).await;
                if !nuevo.error {
                    cache2.set(nuevo);
                }
            });
        }
        return datos;
    }

    // Sin caché: lectura acotada. El panel ya precalienta al arrancar, así que
    // esto sólo pasa en el primer render tras un reinicio.
    match mikareno_client(state) {
        Some(client) => {
            let datos = leer_mikareno(&client).await;
            if !datos.error {
                cache.set(datos.clone());
            }
            datos
        }
        None => DatosMikareno {
            error: true,
            ..Default::default()
        },
    }
}

/// Acceso al clon de la caché (el `AppState` es `Clone`, la caché no).
fn cache_ref(state: &Arc<AppState>) -> Arc<CacheMikareno> {
    state.cache_mikareno.clone()
}

/// Lectura acotada de Mikareno para el calendario.
async fn leer_mikareno(client: &crate::integrations::mikareno::MikarenoClient) -> DatosMikareno {
    let carga = async {
        match time::timeout(TIMEOUT_HANDSHAKE, client.initialize()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                return DatosMikareno {
                    error: true,
                    ..Default::default()
                }
            }
            Err(_) => {
                return DatosMikareno {
                    error: true,
                    ..Default::default()
                }
            }
        }
        match client.student_full().await {
            Ok(raw) => parsear_student_full(&raw),
            Err(_) => DatosMikareno {
                error: true,
                ..Default::default()
            },
        }
    };

    match time::timeout(TIMEOUT_DATOS, carga).await {
        Ok(datos) => datos,
        Err(_) => DatosMikareno {
            error: true,
            ..Default::default()
        },
    }
}

/// Anotaciones locales del usuario en un rango de fechas.
pub async fn anotaciones_en_rango(
    state: &Arc<AppState>,
    desde: NaiveDate,
    hasta: NaiveDate,
) -> Vec<Anotacion> {
    let desde_s = desde.format("%Y-%m-%d").to_string();
    let hasta_s = hasta.format("%Y-%m-%d").to_string();
    let rows = sqlx::query(
        "SELECT id, date, title, body
           FROM calendar_annotations
          WHERE date >= ? AND date <= ?
          ORDER BY date ASC, id ASC",
    )
    .bind(&desde_s)
    .bind(&hasta_s)
    .fetch_all(&state.db)
    .await;
    match rows {
        Ok(rows) => rows
            .iter()
            .map(|r| Anotacion {
                id: r.get::<i64, _>("id"),
                date: r.get::<String, _>("date"),
                title: r.get::<String, _>("title"),
                body: r.get::<String, _>("body"),
            })
            .collect(),
        Err(e) => {
            tracing::warn!("calendario: no se pudieron leer las anotaciones: {e}");
            Vec::new()
        }
    }
}

/// Construye el mapa de días del mes visible.
///
/// `datos` son los datos de Mikareno (cacheados) y `anotaciones`, las notas
/// locales. El resultado indexa por `YYYY-MM-DD`.
pub fn dias_del_mes(
    datos: &DatosMikareno,
    anotaciones: &[Anotacion],
    foco: NaiveDate,
) -> HashMap<String, DiaData> {
    let mut mapa: HashMap<String, DiaData> = HashMap::new();

    for ev in &datos.eventos {
        if ev.fecha.len() < 10 {
            continue;
        }
        mapa.entry(ev.fecha.chars().take(10).collect())
            .or_default()
            .eventos
            .push(ev.clone());
    }

    for t in &datos.tareas {
        if let Some(fecha) = fecha_vencimiento(t) {
            mapa.entry(fecha).or_default().tareas.push(t.clone());
        }
    }

    for a in anotaciones {
        mapa.entry(a.date.clone())
            .or_default()
            .anotaciones
            .push(a.clone());
    }

    // Las clases se proyectan sobre todos los días del mes (el horario es
    // semanal, así que aplica a cualquier día de la semana).
    let (year, month) = (foco.year(), foco.month());
    let primero = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(foco);
    let dias_mes = days_in_month(year, month);
    for d in 1..=dias_mes {
        let fecha = primero + chrono::Duration::days(d as i64 - 1);
        let clases = clases_del_dia(&datos.horario, fecha);
        if !clases.is_empty() {
            let clave = fecha.format("%Y-%m-%d").to_string();
            mapa.entry(clave).or_default().clases = clases;
        }
    }

    mapa
}

/// Días que tiene el mes.
fn days_in_month(year: i32, month: u32) -> u32 {
    let primero = NaiveDate::from_ymd_opt(year, month, 1);
    match primero {
        Some(p) => {
            let siguiente = if month == 12 {
                NaiveDate::from_ymd_opt(year + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(year, month + 1, 1)
            };
            match siguiente {
                Some(s) => (s - p).num_days() as u32,
                None => 30,
            }
        }
        None => 30,
    }
}

/// Número de elementos que se pintan en una celda (clases + eventos + tareas
/// + anotaciones), para el badge contador.
pub fn total_elementos(dia: &DiaData) -> usize {
    dia.clases.len() + dia.eventos.len() + dia.tareas.len() + dia.anotaciones.len()
}

/// Cuántos elementos se quedan fuera de la celda (van como "+N").
pub fn ocultos(dia: &DiaData) -> usize {
    total_elementos(dia).saturating_sub(MAX_EVENTOS_POR_DIA)
}

/// ¿Tiene el día algo de Mikareno (no solo anotaciones del usuario)?
pub fn tiene_datos_mikareno(dia: &DiaData) -> bool {
    !dia.clases.is_empty() || !dia.eventos.is_empty() || !dia.tareas.is_empty()
}

/// Chips de una celda del calendario.
///
/// Se muestran, en este orden: las clases del horario, las tareas que vencen
/// ese día, los eventos escolares y las anotaciones del usuario. Cada chip
/// lleva su color para distinguirlos de un vistazo.
pub fn chips_del_dia(dia: &DiaData, max: usize) -> String {
    let mut out = String::new();
    let mut n = 0usize;

    for clase in &dia.clases {
        if n >= max {
            break;
        }
        out.push_str(&format!(
            r#"<div class="mt-space-xs bg-secondary-container/20 border border-secondary-container/30 rounded text-[9px] text-on-surface truncate px-1 py-0.5" title="Clase: {}">{}</div>"#,
            escape(clase),
            escape(clase)
        ));
        n += 1;
    }

    for tarea in &dia.tareas {
        if n >= max {
            break;
        }
        let titulo = if tarea.titulo.trim().is_empty() {
            "Tarea"
        } else {
            tarea.titulo.trim()
        };
        out.push_str(&format!(
            r#"<div class="mt-space-xs bg-error-container/20 border border-error-container/30 rounded text-[9px] text-on-surface truncate px-1 py-0.5" title="Tarea: {}">{}</div>"#,
            escape(titulo),
            escape(titulo)
        ));
        n += 1;
    }

    for evento in &dia.eventos {
        if n >= max {
            break;
        }
        out.push_str(&format!(
            r#"<div class="mt-space-xs bg-primary-container/15 border border-primary-container/20 rounded text-[9px] text-on-surface truncate px-1 py-0.5" title="{}">{}</div>"#,
            escape(&evento.evento),
            escape(&evento.evento)
        ));
        n += 1;
    }

    for anot in &dia.anotaciones {
        if n >= max {
            break;
        }
        let texto = if anot.title.trim().is_empty() {
            anot.body.trim()
        } else {
            anot.title.trim()
        };
        if texto.is_empty() {
            continue;
        }
        out.push_str(&format!(
            r#"<div class="mt-space-xs bg-tertiary-container/25 border border-tertiary-container/40 rounded text-[9px] text-on-surface truncate px-1 py-0.5" title="Anotación: {}">{}</div>"#,
            escape(texto),
            escape(texto)
        ));
        n += 1;
    }

    out
}

/// Escape de HTML para los chips (mínimo pero suficiente para texto).
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
