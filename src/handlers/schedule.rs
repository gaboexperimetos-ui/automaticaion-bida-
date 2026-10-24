//! Horario semanal — SIEMPRE 7 columnas (lunes→domingo) con el número del día
//! del mes en el encabezado (`Lunes 19`).
//!
//! Alternativa a `academic/schedule.rs::weekly` para la ruta GET
//! `/dashboard/horario` (puede delegarse acá al hacer el wiring en
//! `handlers/mod.rs` sin tocar `academic/templates/**`). Los días que el MCP no
//! trae se renderizan con estado honesto "Sin clases" — nunca inventados.
//! El helper `horario_html_detalles` es puro (sin I/O) y queda disponible para
//! delegados y tests.
//!
//! Datos: se reusa el mismo client-call y el mismo adapter del módulo
//! académico (`MikarenoClient::horario` → `adapters::parse_horario`, día en
//! español minúscula `lunes`/`martes`/`miércoles`… con la etiqueta
//! `"07:35 - 08:15 — Materia [CÓD]"`). La página pide el horario con un
//! presupuesto acotado y **cache-first**: el cliente Mikareno tiene caché
//! read-through (TTL 180 s), así que un horario ya descargado se sirve sin
//! I/O. Antes esta página exigía un handshake `initialize` con 10 s de
//! presupuesto *antes* de pedir los datos; cuando el servidor Mikareno está
//! ocupado (su captura con Playwright bloquea el event loop del server ~1
//! min) ese handshake expiraba, la llamada a `get_horario` nunca se hacía y
//! las 7 columnas quedaban en "Sin clases" aunque el MCP sí tuviera el
//! horario.

use std::sync::Arc;

use axum::{extract::State, response::Html};
use chrono::{Datelike, Local, NaiveDate, Weekday};
use tokio::time::{self, Duration, Instant};

use crate::academic::{mikareno_client, templates};
use crate::integrations::mikareno::{Horario, HorarioDia, MikarenoClient};
use crate::AppState;

/// Texto honesto para las columnas sin clases.
const SIN_CLASES: &str = "Sin clases";

/// Presupuesto total para traer el horario del MCP (cache-first + reintento).
///
/// El servidor Mikareno puede tardar ~20 s en responder cuando está capturando
/// (su `_ready` refresca en línea y bloquea el event loop), así que 10 s no
/// alcanzan: el presupuesto da margen para el handshake *y* para la llamada a
/// `get_horario` sin dejar la página colgada indefinidamente.
const TIMEOUT_DATOS: Duration = Duration::from_secs(5);

/// Presupuesto para renegociar la sesión MCP (`initialize`) antes del reintento.
const TIMEOUT_HANDSHAKE: Duration = Duration::from_secs(3);

/// Nombres de mes en español (chrono no trae locale).
const MESES_ES: [&str; 12] = [
    "enero",
    "febrero",
    "marzo",
    "abril",
    "mayo",
    "junio",
    "julio",
    "agosto",
    "septiembre",
    "octubre",
    "noviembre",
    "diciembre",
];

/// Nombre de mes en español.
pub fn mes_es(mes: u32) -> Option<&'static str> {
    if (1..=12).contains(&mes) {
        Some(MESES_ES[(mes - 1) as usize])
    } else {
        None
    }
}

/// Normaliza un nombre de día: minúsculas, sin acentos y sin prefijos de
/// etiqueta (`día lunes` → `lunes`).
fn normalizar_dia(dia: &str) -> String {
    let plano: String = dia
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ë' => 'e',
            'í' | 'ì' | 'ï' => 'i',
            'ó' | 'ò' | 'ö' => 'o',
            'ú' | 'ù' | 'ü' => 'u',
            'ñ' => 'n',
            other => other,
        })
        .collect();
    let s = plano.as_str();
    let s = s
        .strip_prefix("dia ")
        .or_else(|| s.strip_prefix("dia:"))
        .unwrap_or(s);
    s.trim_start_matches([' ', ':', '.', '-']).to_string()
}

/// Mapea el nombre de día que emite el MCP a `Weekday`.
///
/// El MCP (`mikareno-mcp/server.py::get_horario`) emite los días en español y
/// minúscula (`lunes`, `martes`, `miércoles`…), pero el mismo dato puede
/// llegar como clave cruda del `periodMap` (`MONDAY`, `WEDNESDAY`) o detrás de
/// un prefijo de horario (`SAB 08:00`); se comparan por prefijo tras
/// normalizar. Lo que no corresponde a lunes–domingo queda como `None` y el
/// renderer lo reporta, no lo oculta.
pub fn dia_a_weekday(dia: &str) -> Option<Weekday> {
    let norm = normalizar_dia(dia);
    // Claves inglesas del `periodMap` (fallback `dm.get(dk, dk)` del server).
    match norm.as_str() {
        "monday" | "mon" => return Some(Weekday::Mon),
        "tuesday" | "tue" | "tues" => return Some(Weekday::Tue),
        "wednesday" | "wed" => return Some(Weekday::Wed),
        "thursday" | "thu" | "thur" | "thurs" => return Some(Weekday::Thu),
        "friday" | "fri" => return Some(Weekday::Fri),
        "saturday" | "sat" => return Some(Weekday::Sat),
        "sunday" | "sun" => return Some(Weekday::Sun),
        _ => {}
    }
    // Español: `lunes`, `martes`, `miércoles`, `jueves`, `viernes`,
    // `sábado`, `domingo` (y sus abreviaturas `lun`, `mar`, `mie`, …).
    match norm.get(..2) {
        Some("lu") => Some(Weekday::Mon),
        Some("ma") => Some(Weekday::Tue),
        Some("mi") => Some(Weekday::Wed),
        Some("ju") => Some(Weekday::Thu),
        Some("vi") => Some(Weekday::Fri),
        Some("sa") => Some(Weekday::Sat),
        Some("do") => Some(Weekday::Sun),
        _ => None,
    }
}

/// Etiqueta `Lunes 19` para una fecha: día en español (`weekday_es`) + día del mes.
pub fn etiqueta_dia(fecha: NaiveDate) -> String {
    let iso = format!(
        "{:04}-{:02}-{:02}",
        fecha.year(),
        fecha.month(),
        fecha.day()
    );
    format!("{} {}", templates::weekday_es(&iso), fecha.day())
}

/// Subtítulo tipo `Semana del 19 al 25 de octubre` (o cruzando dos meses).
pub fn rango_semana(lunes: NaiveDate, domingo: NaiveDate) -> String {
    let mes_fin = mes_es(domingo.month()).unwrap_or("");
    if (lunes.month(), lunes.year()) == (domingo.month(), domingo.year()) {
        format!(
            "Semana del {} al {} de {} de {}",
            lunes.day(),
            domingo.day(),
            mes_fin,
            domingo.year()
        )
    } else {
        let mes_ini = mes_es(lunes.month()).unwrap_or("");
        format!(
            "Semana del {} de {} al {} de {} de {}",
            lunes.day(),
            mes_ini,
            domingo.day(),
            mes_fin,
            domingo.year()
        )
    }
}

/// Las 7 fechas de la semana (lunes→domingo) que contiene `hoy`.
pub fn semana_actual(hoy: NaiveDate) -> [NaiveDate; 7] {
    let lunes = hoy - chrono::Duration::days(hoy.weekday().num_days_from_monday() as i64);
    std::array::from_fn(|i| lunes + chrono::Duration::days(i as i64))
}

/// Nota honesta para días que el MCP devolvió y no se pudieron mapear a
/// lunes–domingo (nunca se descartan en silencio).
fn nota_sin_mapear(sin_mapear: &[&str]) -> String {
    if sin_mapear.is_empty() {
        return String::new();
    }
    let lista = sin_mapear
        .iter()
        .map(|d| templates::html_escape(d))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"<p class="font-body-sm text-body-sm text-on-surface-variant mt-space-xs">Días fuera de lunes–domingo sin mapear: {}.</p>"#,
        lista
    )
}

/// Grilla del horario semanal: SIEMPRE 7 columnas (lunes→domingo), cada una con
/// su fecha de la semana actual en el encabezado (`Lunes 19`), el día actual
/// resaltado (`border-2 border-primary-container/60` + chip `hoy`) y las clases
/// de `get_horario`, o el estado honesto "Sin clases" cuando el MCP no trae
/// datos para ese día. Pura: sin I/O, lista para tests y delegados.
pub fn horario_html_detalles(dias: &[HorarioDia], hoy: NaiveDate) -> String {
    let semana = semana_actual(hoy);

    // Distribuye las clases por día de la semana; lo no mapeable se lista al
    // final en vez de inventarle una columna o descartarlo silenciosamente.
    let mut clases: [Vec<&str>; 7] = Default::default();
    let mut sin_mapear: Vec<&str> = Vec::new();
    for d in dias {
        match dia_a_weekday(&d.dia) {
            Some(wd) => {
                let idx = wd.num_days_from_monday() as usize;
                clases[idx].extend(d.clases.iter().map(String::as_str));
            }
            None => sin_mapear.push(&d.dia),
        }
    }

    let mut celdas = String::new();
    for (i, &fecha) in semana.iter().enumerate() {
        let es_hoy = fecha == hoy;
        let resaltado = if es_hoy {
            "border-2 border-primary-container/60 "
        } else {
            ""
        };
        let chip = if es_hoy {
            r#"<span class="font-label-md text-primary-fixed-dim">hoy</span>"#
        } else {
            ""
        };
        let items = if clases[i].is_empty() {
            format!(
                r#"<p class="font-label-md text-on-surface-variant mt-space-xs">{}</p>"#,
                SIN_CLASES
            )
        } else {
            let lis = clases[i]
                .iter()
                .map(|c| {
                    format!(
                        r#"<li class="font-body-sm text-body-sm text-on-surface-variant py-1 border-b border-outline/30 last:border-b-0">{}</li>"#,
                        templates::html_escape(c)
                    )
                })
                .collect::<Vec<_>>()
                .join("");
            format!(r#"<ul class="mt-space-xs">{}</ul>"#, lis)
        };
        celdas.push_str(&format!(
            r#"<div class="nova-schedule-day {resaltado}bg-surface-container rounded-lg p-space-sm min-h-[120px]">
                <h3 class="font-headline-sm text-headline-sm text-primary-container font-semibold">{}</h3>
                {chip}
                {items}
            </div>"#,
            templates::html_escape(&etiqueta_dia(fecha)),
        ));
    }

    format!(
        r#"<div class="overflow-x-auto">
            <div class="nova-schedule-grid min-w-[840px] grid grid-cols-7 gap-px bg-outline/40 border border-outline rounded-lg overflow-hidden">
                {celdas}
            </div>
        </div>{}"#,
        nota_sin_mapear(&sin_mapear)
    )
}

/// Trae el horario del MCP con presupuesto acotado.
///
/// `client.horario()` es *cache-first* (caché read-through del cliente, TTL
/// 180 s) y se auto-inicializa si la sesión MCP no está viva, así que la
/// llamada normal no necesita handshake previo. Si la llamada falla, se
/// renegociia la sesión (`initialize`) y se reintenta una vez, siempre dentro
/// del mismo presupuesto total.
async fn horario_con_presupuesto(client: &MikarenoClient) -> Option<Horario> {
    let deadline = Instant::now() + TIMEOUT_DATOS;

    // 1) Cache-first: `horario()` sirve la caché read-through del cliente sin
    //    I/O y, si hace falta, se auto-inicializa (no exige handshake previo).
    let primer_intento = time::timeout(
        deadline.saturating_duration_since(Instant::now()),
        client.horario(),
    )
    .await;
    if let Ok(Ok(h)) = primer_intento {
        return Some(h);
    }

    // 2) Falló o agotó el tiempo: re-negocia la sesión MCP y reintenta una
    //    vez, siempre dentro del mismo presupuesto total.
    let restante = deadline.saturating_duration_since(Instant::now());
    if restante < Duration::from_secs(3) {
        return None;
    }
    if time::timeout(restante.min(TIMEOUT_HANDSHAKE), client.initialize())
        .await
        .is_ok()
    {
        let restante = deadline.saturating_duration_since(Instant::now());
        if let Ok(Ok(h)) = time::timeout(restante, client.horario()).await {
            return Some(h);
        }
    }
    None
}

/// GET /dashboard/horario — grilla semanal con SIEMPRE 7 columnas desde el
/// lunes de esta semana. Usa el cache read-through del cliente Mikareno
/// (`horario()`), con banner de estado honesto; sin conexión todas las columns
/// muestran "Sin clases". Mismo contrato que `academic/schedule.rs::weekly`,
/// listo para que la ruta apunte acá.
pub async fn weekly(State(state): State<Arc<AppState>>) -> Html<String> {
    let mut horario = Horario::default();
    let mut conectado = false;

    if let Some(client) = mikareno_client(&state) {
        if let Some(h) = horario_con_presupuesto(&client).await {
            conectado = !h.dias.is_empty();
            horario = h;
        }
    }

    let hoy = Local::now().date_naive();
    let semana = semana_actual(hoy);
    let content = format!(
        r#"<div class="nova-page nova-schedule-page">
          <header class="nova-page-head compact"><div><p class="nova-eyebrow"><span>05</span> Horarios</p><h1>Tu semana,<br><em>bien ubicada.</em></h1><p class="nova-page-lead">{}</p></div><div class="nova-page-actions"><a href="/dashboard/planner" class="nova-button nova-button-quiet">Planificar</a><a href="/dashboard/calendar" class="nova-button nova-button-primary">Abrir agenda</a></div></header>
          <section class="nova-schedule-board"><div class="nova-panel-caption"><span>01</span><h2>Lunes a domingo</h2><span class="material-symbols-outlined">calendar_view_week</span></div>{}{}</section>
        </div>"#,
        rango_semana(semana[0], semana[6]),
        templates::mcp_status_banner(conectado),
        horario_html_detalles(&horario.dias, hoy)
    );

    // Stitch v2 shell con breadcrumb + nav activo resueltos vía page_ext.
    Html(templates::page_ext(
        "Horario — Study Platform",
        "Horario",
        "/dashboard/horario",
        &content,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn siempre_siete_columnas_con_dia_del_mes() {
        let hoy = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(); // lunes
        let html = horario_html_detalles(&[], hoy);
        assert!(html.contains("grid grid-cols-7 gap-px"));
        assert_eq!(html.matches("<h3").count(), 7);
        assert!(html.contains("Lunes 19"));
        assert!(html.contains("Domingo 25"));
    }

    #[test]
    fn hoy_resaltado_y_sin_clases_honesto() {
        let hoy = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
        let html = horario_html_detalles(&[], hoy);
        assert!(html.contains("border-2 border-primary-container/60"));
        assert!(html.contains(">hoy</span>"));
        assert_eq!(html.matches(SIN_CLASES).count(), 7);
    }

    #[test]
    fn dias_se_mapean_y_se_escapan() {
        let hoy = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap(); // miércoles
        let dias = vec![
            HorarioDia {
                dia: "miércoles".to_string(),
                clases: vec!["Química <b>1</b>".to_string()],
            },
            HorarioDia {
                dia: "NadaVeAcá".to_string(),
                clases: vec!["X".to_string()],
            },
        ];
        let html = horario_html_detalles(&dias, hoy);
        assert!(html.contains("Miércoles 21"));
        assert!(html.contains("Química &lt;b&gt;1&lt;/b&gt;"));
        assert!(html.contains("sin mapear: NadaVeAcá"));
        assert_eq!(dia_a_weekday("SÁBADO"), Some(Weekday::Sat));
        assert_eq!(dia_a_weekday("nada"), None);
    }

    #[test]
    fn mapea_claves_inglesas_horas_y_etiquetas() {
        // El `periodMap` del MCP puede entregar la clave cruda (`MONDAY`).
        assert_eq!(dia_a_weekday("MONDAY"), Some(Weekday::Mon));
        assert_eq!(dia_a_weekday("SUNDAY"), Some(Weekday::Sun));
        // …o el día pegado a la hora (`SAB 08:00`).
        assert_eq!(dia_a_weekday("SAB 08:00"), Some(Weekday::Sat));
        assert_eq!(dia_a_weekday("mié 07:35"), Some(Weekday::Wed));
        // …o con prefijo de etiqueta (`día lunes`).
        assert_eq!(dia_a_weekday("día lunes"), Some(Weekday::Mon));
        // Lo que no es un día sigue sin mapear (se reporta, no se oculta).
        assert_eq!(dia_a_weekday("NadaVeAcá"), None);
    }
}
