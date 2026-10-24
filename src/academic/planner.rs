//! Planificador semanal de tareas (Mikareno).
//!
//! GET /dashboard/planner?week=YYYY-MM-DD
//!
//! Render semanal honesto: las 7 columnas (lunes→domingo) de la semana
//! pedida con las tareas que vencen ese día, la navegación de semana
//! anterior / siguiente (`?week=YYYY-MM-DD`, acotada a ±8 semanas respecto de
//! la actual) y el estado vacío cuando el MCP no responde.
//!
//! Datos: `get_tareas` del MCP de Mikareno. El servidor real llega con la
//! lista detallada vacía (`lista_tareas: []` — la vista Deberes/Actividades no
//! trae `pageContent`) pero sí con los conteos (`resumen.esta_semana`,
//! `resumen.hoy`, `resumen.manana`). Cuando la lista no trae elementos se
//! muestran las columnas vacías y el conteo real: nunca tareas de ejemplo ni
//! esqueletos de carga.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::Html;
use chrono::{Datelike, Local, NaiveDate};
use serde::Deserialize;
use serde_json::Value;
use tokio::time;

use crate::academic::{mikareno_client, templates};
use crate::handlers::schedule::{etiqueta_dia, rango_semana, semana_actual};
use crate::integrations::mikareno::Tarea;
use crate::AppState;

/// Margen de navegación: como mucho ±8 semanas respecto de la semana actual.
const MAX_WEEK_OFFSET: i64 = 8;

/// Presupuesto para traer los datos del MCP. Cubre *todas* las llamadas del
/// handler (la cruda y la tipada), no una sola: así el tiempo de carga se
/// queda por debajo de los 30 s aunque el MCP tarde. Al agotarse, la vista
/// degrada al estado vacío honesto ("Mikareno no respondió").
const TIMEOUT_DATOS: Duration = Duration::from_secs(25);

/// Ruta de la vista (misma que usa el nav lateral del shell).
const RUTA: &str = "/dashboard/planner";

/// Parámetros de la vista del planificador.
#[derive(Deserialize)]
pub struct PlannerQuery {
    /// Fecha ancla de la semana (`YYYY-MM-DD`); se normaliza a su lunes.
    week: Option<String>,
}

/// Conteos reales que reporta Mikareno (`get_tareas → resumen`).
#[derive(Debug, Default, Clone, Copy)]
struct Resumen {
    esta_semana: u32,
    hoy: u32,
    manana: u32,
}

/// GET /dashboard/planner — planificador semanal (`?week=YYYY-MM-DD`).
///
/// `week` es la fecha ancla de la semana a mostrar: se normaliza a su lunes,
/// una fecha ilegible cae en la semana actual y el rango se acota a ±8
/// semanas respecto de la actual.
pub async fn weekly(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PlannerQuery>,
) -> Html<String> {
    let hoy = Local::now().date_naive();
    let lunes_actual = lunes_de(hoy);
    let lunes = resolver_semana(query.week.as_deref(), hoy);
    let dias = semana_actual(lunes);
    let domingo = dias[6];
    let es_semana_actual = lunes == lunes_actual;

    let (conectado, tareas, resumen) = cargar_tareas(&state).await;
    let banner = templates::mcp_status_banner(conectado);

    // MCP sin respuesta: banner de desconectado + estado vacío honesto.
    if !conectado {
        let vacio = templates::empty_state_html(
            "Mikareno no respondió: no hay datos de tareas que mostrar. Verifica que el MCP esté corriendo.",
            &format!("{}?week={}", RUTA, iso(lunes)),
        );
        let content = format!(
            r###"<div class="nova-page nova-planner-page"><header class="nova-page-head compact"><div><p class="nova-eyebrow"><span>02</span> Planificación</p><h1>Una semana,<br><em>paso a paso.</em></h1><p class="nova-page-lead">Organiza el trabajo real de tus materias sin perder de vista lo que vence después.</p></div><a class="nova-button nova-button-quiet" href="/dashboard/calendar">Abrir calendario</a></header><section class="nova-planner-week">{banner}{vacio}</section></div>"###
        );
        return Html(templates::page_ext(
            "Planificador — Study Platform",
            "Planificador",
            RUTA,
            &content,
        ));
    }

    // Reparto de las tareas por día de la semana seleccionada. Lo que vence
    // fuera de la semana y lo que no tiene fecha legible no se descarta: se
    // reporta aparte.
    let mut por_dia: [Vec<&Tarea>; 7] = Default::default();
    let mut otras_semanas: Vec<&Tarea> = Vec::new();
    let mut sin_fecha: Vec<&Tarea> = Vec::new();
    for t in &tareas {
        match fecha_vencimiento(&t.vencimiento) {
            Some(f) if f >= lunes && f <= domingo => {
                let idx = (f.weekday().num_days_from_monday() as usize).min(6);
                por_dia[idx].push(t);
            }
            Some(_) => otras_semanas.push(t),
            None => sin_fecha.push(t),
        }
    }

    let nav = nav_semana(
        lunes,
        lunes_actual,
        lunes_actual - chrono::Duration::weeks(MAX_WEEK_OFFSET),
        lunes_actual + chrono::Duration::weeks(MAX_WEEK_OFFSET),
    );

    let mut celdas = String::new();
    for (i, fecha) in dias.iter().enumerate() {
        let es_hoy = *fecha == hoy;
        let resaltado = if es_hoy {
            "border-2 border-primary-container/60 "
        } else {
            ""
        };
        let chip = if es_hoy {
            r#"<span class="font-label-sm text-primary-fixed-dim">hoy</span>"#
        } else {
            ""
        };
        let items = if por_dia[i].is_empty() {
            r#"<p class="font-label-md text-on-surface-variant mt-space-xs">Sin tareas</p>"#
                .to_string()
        } else {
            let lis = por_dia[i]
                .iter()
                .map(|t| item_tarea(t))
                .collect::<Vec<_>>()
                .join("");
            format!(r#"<ul class="mt-space-xs space-y-space-xs">{lis}</ul>"#)
        };
        celdas.push_str(&format!(
            r##"<div class="nova-planner-day {resaltado}bg-surface-container rounded-lg p-space-sm min-h-[140px]">
                <h3 class="font-headline-sm text-headline-sm text-primary-container font-semibold">{etiqueta}</h3>
                {chip}
                {items}
            </div>"##,
            etiqueta = templates::html_escape(&etiqueta_dia(*fecha)),
        ));
    }
    let grid = format!(
        r##"<div class="overflow-x-auto">
            <div class="nova-planner-grid min-w-[840px] grid grid-cols-7 gap-px bg-outline/40 border border-outline rounded-lg overflow-hidden">
                {celdas}
            </div>
        </div>"##,
        celdas = celdas
    );

    // Notas honestas: qué trajo el MCP y qué no.
    let total_semana: usize = por_dia.iter().map(|v| v.len()).sum();
    let mut notas: Vec<String> = Vec::new();
    if tareas.is_empty() {
        if resumen.esta_semana > 0 || resumen.hoy > 0 {
            notas.push(format!(
                "Mikareno reporta {} tarea(s) esta semana ({} hoy, {} mañana), pero no expone la lista detallada: la vista Deberes/Actividades llega vacía.",
                resumen.esta_semana, resumen.hoy, resumen.manana
            ));
        } else {
            notas.push(
                "Mikareno no reporta tareas pendientes ni expone lista detallada (`get_tareas` → `lista_tareas: []`)."
                    .to_string(),
            );
        }
    } else if total_semana == 0 {
        notas.push("Sin tareas con vencimiento dentro de la semana seleccionada.".to_string());
    } else {
        notas.push(format!(
            "{} tarea(s) con vencimiento en la semana seleccionada.",
            total_semana
        ));
    }
    if !es_semana_actual {
        notas.push(format!(
            "Los conteos de Mikareno ({} esta semana, {} hoy) corresponden a la semana actual, no a la semana seleccionada.",
            resumen.esta_semana, resumen.hoy
        ));
    }

    let resumen_html = templates::card_list("Resumen de tareas", &notas);
    let otras_html = card_tareas("Tareas que vencen en otras semanas", &otras_semanas);
    let sin_fecha_html = card_tareas("Tareas sin fecha de vencimiento legible", &sin_fecha);

    let content = format!(
        r###"<div class="nova-page nova-planner-page">
          <header class="nova-page-head compact"><div><p class="nova-eyebrow"><span>02</span> Planificación</p><h1>Una semana,<br><em>paso a paso.</em></h1><p class="nova-page-lead">Organiza el trabajo real de tus materias sin perder de vista lo que vence después.</p></div><div class="nova-page-actions"><a class="nova-button nova-button-quiet" href="/dashboard/horario">Ver horario</a><a class="nova-button nova-button-primary" href="/dashboard/calendar">Abrir calendario</a></div></header>
          <section class="nova-planner-week"><div class="nova-panel-caption"><span>01</span><h2>Ritmo de la semana</h2><span class="material-symbols-outlined">view_week</span></div>{banner}{nav}{grid}</section>
          <div class="nova-planner-bottom">{resumen_html}{otras_html}{sin_fecha_html}</div>
        </div>"###
    );

    Html(templates::page_ext(
        "Planificador — Study Platform",
        "Planificador",
        RUTA,
        &content,
    ))
}

/// Trae las tareas del MCP con presupuesto acotado.
///
/// Devuelve `(conectado, tareas, resumen)`:
/// * `conectado = false` → el MCP no respondió (banner de desconectado).
/// * `conectado = true` con lista vacía → el MCP responde pero no expone la
///   lista detallada; los conteos de `resumen` siguen siendo reales.
async fn cargar_tareas(state: &AppState) -> (bool, Vec<Tarea>, Resumen) {
    let Some(client) = mikareno_client(state) else {
        return (false, Vec::new(), Resumen::default());
    };

    // Un único presupuesto para las dos llamadas: la cruda (que decide si el
    // MCP responde y trae los conteos) y la tipada (que usa la caché
    // read-through del cliente, sin I/O extra). Al agotarse, el MCP se
    // trata como no conectado: la vista lo dice con honestidad en vez de
    // quedarse colgada.
    let carga = async {
        let raw = client
            .call_tool("get_tareas", serde_json::json!({}))
            .await
            .ok()?;
        // Mismo payload (la caché responde) y adapter tolerante a campos
        // faltantes.
        let tareas = client.tareas().await.unwrap_or_default();
        Some((raw, tareas))
    };

    match time::timeout(TIMEOUT_DATOS, carga).await {
        Ok(Some((raw, tareas))) => (true, tareas, parse_resumen(&raw)),
        _ => (false, Vec::new(), Resumen::default()),
    }
}

/// Lee `resumen.{esta_semana,hoy,manana}` del payload crudo de `get_tareas`.
fn parse_resumen(raw: &Value) -> Resumen {
    let resumen = raw.get("resumen");
    let campo = |k: &str| {
        resumen
            .and_then(|r| r.get(k))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32
    };
    Resumen {
        esta_semana: campo("esta_semana"),
        hoy: campo("hoy"),
        manana: campo("manana"),
    }
}

/// Lunes de la semana que contiene `fecha`.
fn lunes_de(fecha: NaiveDate) -> NaiveDate {
    fecha - chrono::Duration::days(fecha.weekday().num_days_from_monday() as i64)
}

/// Parsea `YYYY-MM-DD` (tolerando un sufijo de hora ISO).
fn parse_fecha(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    let head = s.get(0..10)?;
    NaiveDate::parse_from_str(head, "%Y-%m-%d").ok()
}

/// Resuelve el lunes de la semana a mostrar.
/// * `week` ausente o con fecha ilegible → semana actual.
/// * Se normaliza al lunes de la semana indicada.
/// * Se acota a ±8 semanas respecto de la semana actual.
fn resolver_semana(param: Option<&str>, hoy: NaiveDate) -> NaiveDate {
    let lunes_actual = lunes_de(hoy);
    let minimo = lunes_actual - chrono::Duration::weeks(MAX_WEEK_OFFSET);
    let maximo = lunes_actual + chrono::Duration::weeks(MAX_WEEK_OFFSET);
    let ancla = match param.and_then(parse_fecha) {
        Some(f) => lunes_de(f),
        None => lunes_actual,
    };
    if ancla < minimo {
        minimo
    } else if ancla > maximo {
        maximo
    } else {
        ancla
    }
}

/// Fecha en formato `YYYY-MM-DD` (el formato del query param `week`).
fn iso(fecha: NaiveDate) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        fecha.year(),
        fecha.month(),
        fecha.day()
    )
}

/// Fecha de vencimiento de una tarea: ISO (`YYYY-MM-DD`, con o sin hora) o el
/// formato español `DD/MM/YYYY`. `None` si no es legible.
fn fecha_vencimiento(vencimiento: &str) -> Option<NaiveDate> {
    let s = vencimiento.trim();
    if let Some(head) = s.get(0..10) {
        if let Ok(f) = NaiveDate::parse_from_str(head, "%Y-%m-%d") {
            return Some(f);
        }
    }
    let partes: Vec<&str> = s.split('/').collect();
    if partes.len() == 3 {
        let dia = partes[0].trim().parse::<u32>().ok()?;
        let mes = partes[1].trim().parse::<u32>().ok()?;
        let anio = partes[2].trim().parse::<i32>().ok()?;
        return NaiveDate::from_ymd_opt(anio, mes, dia);
    }
    None
}

/// Navegación de semana: anterior / actual / siguiente. En el borde del
/// margen (±8 semanas) el enlace se desactiva en vez de llevar a una semana
/// que el servidor recortaría.
fn nav_semana(
    lunes: NaiveDate,
    lunes_actual: NaiveDate,
    minimo: NaiveDate,
    maximo: NaiveDate,
) -> String {
    let anterior = lunes - chrono::Duration::weeks(1);
    let siguiente = lunes + chrono::Duration::weeks(1);

    let prev = if anterior < minimo {
        r##"<span class="font-label-md text-outline px-space-sm py-1 select-none" aria-disabled="true">‹ Anterior</span>"##
            .to_string()
    } else {
        format!(
            r##"<a href="{ruta}?week={fecha}" class="font-label-md text-primary-container hover:bg-surface-container rounded-lg px-space-sm py-1 transition-colors">‹ Anterior</a>"##,
            ruta = RUTA,
            fecha = iso(anterior)
        )
    };

    let next = if siguiente > maximo {
        r##"<span class="font-label-md text-outline px-space-sm py-1 select-none" aria-disabled="true">Siguiente ›</span>"##
            .to_string()
    } else {
        format!(
            r##"<a href="{ruta}?week={fecha}" class="font-label-md text-primary-container hover:bg-surface-container rounded-lg px-space-sm py-1 transition-colors">Siguiente ›</a>"##,
            ruta = RUTA,
            fecha = iso(siguiente)
        )
    };

    let actual = if lunes != lunes_actual {
        format!(
            r##"<a href="{ruta}?week={fecha}" class="font-label-sm text-label-sm text-on-surface-variant hover:text-on-surface transition-colors">Ir a la semana actual</a>"##,
            ruta = RUTA,
            fecha = iso(lunes_actual)
        )
    } else {
        String::new()
    };

    format!(
        r##"<div class="nova-planner-nav flex items-center justify-between gap-space-sm mb-space-md">
            {prev}
            <div class="text-center">
                <h2 class="font-headline-sm text-headline-sm text-primary-container font-semibold">{rango}</h2>
                {actual}
            </div>
            {next}
        </div>"##,
        prev = prev,
        next = next,
        actual = actual,
        rango = templates::html_escape(&rango_semana(lunes, lunes + chrono::Duration::days(6)))
    )
}

/// Item de tarea: título, estado y vencimiento. Sin casilla de marcado: no
/// hay backend que marque tareas como hechas, así que no se simula.
fn item_tarea(t: &Tarea) -> String {
    let mut detalles: Vec<String> = Vec::new();
    if let Some(f) = fecha_vencimiento(&t.vencimiento) {
        detalles.push(format!("Vence {}", f.format("%d/%m/%Y")));
    }
    let estado = t.estado.trim();
    if !estado.is_empty() && estado != "SIN_ESTADO" {
        detalles.push(estado.to_string());
    }
    let sub = if detalles.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="font-label-sm text-label-sm text-on-surface-variant">{}</p>"#,
            templates::html_escape(&detalles.join(" · "))
        )
    };
    format!(
        r##"<li class="nova-task-card rounded-lg border border-outline bg-surface-container px-space-sm py-space-xs">
            <p class="font-body-sm text-body-sm text-on-surface">{titulo}</p>
            {sub}
        </li>"##,
        titulo = templates::html_escape(&t.titulo),
        sub = sub
    )
}

/// Tarjeta con una lista de tareas (las que no caben en las columnas).
fn card_tareas(titulo: &str, tareas: &[&Tarea]) -> String {
    if tareas.is_empty() {
        return String::new();
    }
    let items: Vec<String> = tareas
        .iter()
        .map(|t| {
            let estado = if t.estado.trim().is_empty() {
                "sin estado".to_string()
            } else {
                t.estado.trim().to_string()
            };
            let fecha = match fecha_vencimiento(&t.vencimiento) {
                Some(f) => format!(" — {}", f.format("%d/%m/%Y")),
                None => String::new(),
            };
            format!("{} — {}{}", t.titulo, estado, fecha)
        })
        .collect();
    templates::card_list(titulo, &items)
}
