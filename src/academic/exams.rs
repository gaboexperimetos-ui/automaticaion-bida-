//! Página de exámenes — evaluaciones reales de Mikareno.
//!
//! GET /dashboard/examenes
//!
//! Fuentes de datos, en este orden:
//!
//! 1. `get_actividades_materia` — la estructura de calificaciones de Mikareno
//!    trae los submenús `EVALUACIONES` y `EXAMEN TRIMESTRAL` con la fecha real
//!    de cada actividad. Es la única fuente con evaluaciones **confirmadas**,
//!    porque las categoriza el propio Mikareno.
//! 2. `get_calendario` — el calendario de Mikareno **no** tiene tipo "examen"
//!    (solo `SCHOOL_DAY` / `NON_SCHOOL_DAY` / eventos genéricos). Como respaldo
//!    honesto se toman los eventos cuyo título contiene una palabra clave de
//!    evaluación (examen, prueba, evaluación, test, parcial), marcados
//!    explícitamente como **posible evaluación**, nunca como examen confirmado.
//!
//! La línea de tiempo agrupa por cercanía temporal real ("Esta semana",
//! "Este mes", "Más adelante", "Ya pasaron", "Otras fechas") y cada evaluación
//! lleva un chip de cuenta atrás calculado con `chrono::Local` sobre su fecha
//! real: "Hoy", "Mañana", "En N días" o "Pasó hace N días". Las fechas que no
//! parsean van a "Otras fechas" sin chip: nunca se inventa una fecha.
//!
//! La carga tiene un **presupuesto de 5 s**: las dos fuentes se piden en
//! paralelo y cada una con su propio tope, así que el tiempo de la página es
//! el de la fuente más lenta, no la suma. Cuando Mikareno responde con un
//! error de negocio ("No se capturó la estructura de calificaciones") la
//! página **no reintenta**: entrega de inmediato el respaldo por título y un
//! aviso que explica qué falta, sin inventar evaluaciones. Reintentar con
//! `force_refresh` obligaba al servidor a raspar la plataforma entera (y con
//! él a bloquear el resto de páginas) a cambio de una espera de minutos.

use std::cmp::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration as StdDuration, Instant as StdInstant};

use axum::{extract::State, response::Html};
use chrono::{Datelike, NaiveDate};
use serde_json::{json, Value};
use tokio::time::{self, Duration, Instant};

use crate::academic::subjects::{
    warn_una_vez, CLAVE_ESTRUCTURA, PRESUPUESTO_HANDSHAKE, PRESUPUESTO_MCP,
};
use crate::academic::{mikareno_client, templates};
use crate::integrations::mikareno::{CalendarioEvent, MikarenoClient};
use crate::AppState;

/// Presupuesto total de la carga de la página: aunque Mikareno no responda,
/// la página se entrega dentro de este plazo.
const PRESUPUESTO_PAGINA: Duration = PRESUPUESTO_MCP;
/// Vigencia del payload memorizado de la estructura de calificaciones.
const TTL_MEMO: StdDuration = StdDuration::from_secs(300);

/// Palabras clave que delatan una evaluación en el título de un evento.
/// Se comparan sobre el título normalizado (sin acentos, en minúsculas) y
/// por prefijo de palabra, para que "examenes" y "evaluaciones" entren pero
/// "contestacion" no se confunda con "test".
const PALABRAS_EVALUACION: [&str; 5] = ["examen", "prueba", "evaluacion", "test", "parcial"];

/// Origen de una evaluación, para poder etiquetarla sin mentir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origen {
    /// Actividad de la estructura de calificaciones de Mikareno: confirmada.
    Mikareno,
    /// Evento de calendario que *coincide* por título: solo posible.
    Posible,
}

/// Una evaluación mostrada en la línea de tiempo.
#[derive(Debug, Clone)]
struct Evaluacion {
    /// Nombre de la actividad o título del evento.
    titulo: String,
    /// Materia asociada (vacía en los eventos de calendario).
    materia: String,
    /// Fecha cruda tal como la entrega Mikareno (puede no parsear).
    fecha: String,
    /// Categoría de Mikareno (`EVALUACIONES`, `EXAMEN TRIMESTRAL`, …).
    clase: String,
    /// De dónde salió el dato.
    origen: Origen,
}

/// Resultado de la fuente principal: la estructura de calificaciones.
struct FuentePrincipal {
    /// Evaluaciones confirmadas por Mikareno.
    evaluaciones: Vec<Evaluacion>,
    /// ¿Se pudo hablar con el MCP?
    conectado: bool,
    /// Aviso honesto cuando la fuente no entregó evaluaciones confirmadas.
    aviso: Option<String>,
}

/// GET /dashboard/examenes — línea de tiempo de evaluaciones de Mikareno.
///
/// Inicializa el MCP con un *deadline* propio y pide las dos fuentes en
/// paralelo dentro de un presupuesto total de 5 s. Si no responde, la página
/// muestra el banner de desconectado y el estado vacío. Cuando conecta,
/// combina las actividades de evaluación de la estructura de calificaciones
/// con los eventos de calendario cuyo título parece una evaluación, y los
/// ordena cronológicamente para la línea de tiempo.
pub async fn index(State(state): State<Arc<AppState>>) -> Html<String> {
    let client_opt = mikareno_client(&state);
    let mut connected = false;
    let mut evaluaciones: Vec<Evaluacion> = Vec::new();
    let mut aviso: Option<String> = None;

    if let Some(client) = client_opt {
        // Todo lo que sigue cabe en este presupuesto.
        let deadline = Instant::now() + PRESUPUESTO_PAGINA;

        // Handshake MCP con deadline propio, dentro del presupuesto.
        let restante = deadline.saturating_duration_since(Instant::now());
        if matches!(
            time::timeout(restante.min(PRESUPUESTO_HANDSHAKE), client.initialize()).await,
            Ok(Ok(()))
        ) {
            connected = true;
        }

        // Las dos fuentes van EN PARALELO: el tiempo de carga es el de la más
        // lenta, no la suma. Así el respaldo por título no espera a la fuente
        // principal (ni al revés).
        let (principal, eventos) = tokio::join!(
            actividades_con_presupuesto(&client, deadline),
            calendario_con_presupuesto(&client, deadline),
        );

        connected = connected || principal.conectado;
        evaluaciones.extend(principal.evaluaciones);
        aviso = principal.aviso;

        // Fuente 2: respaldo honesto — el calendario no tipa exámenes.
        if let Some(eventos) = eventos {
            connected = true;
            evaluaciones.extend(posibles_de_calendario(&eventos));
        }
    }

    ordenar_evaluaciones(&mut evaluaciones);

    let status = templates::mcp_status_banner(connected);
    let aviso_html = aviso.as_deref().map(aviso_html).unwrap_or_default();

    let body = if evaluaciones.is_empty() {
        r###"<div class="nova-exams-empty"><span class="material-symbols-outlined">event_busy</span><h2>Sin evaluaciones registradas</h2><p>Mikareno no devolvió actividades de evaluación ni eventos de calendario con título de evaluación.</p><a href="/dashboard/calendar" class="nova-button nova-button-quiet">Revisar calendario</a></div>"###.to_string()
    } else {
        let today = chrono::Local::now().date_naive();
        let timeline = timeline_html(&evaluaciones, today);
        format!(
            r###"<div class="nx-exam-summary nova-exam-summary"><span class="material-symbols-outlined">monitoring</span><p>{}</p></div>{}{}"###,
            resumen(&evaluaciones, today),
            nota_posibles(&evaluaciones),
            timeline,
        )
    };
    let content = format!(
        r###"<div class="nx-page nx-exams nova-page nova-exams-page"><header class="nx-page-header nova-page-head"><div><p class="nx-eyebrow">Aprendizaje · Evaluaciones</p><h1>Prepara lo importante.<br>Sin sorpresas.</h1><p>Una lectura cronológica de tus evaluaciones y del tiempo que tienes por delante.</p></div><a href="/dashboard/planner" class="nx-button nx-button-primary"><span class="material-symbols-outlined">event_available</span>Planificar estudio</a></header><section class="nx-exams-board nova-exams-board"><div class="nx-section-head"><div><p class="nx-eyebrow">Cronología</p><h2>Próximas evaluaciones</h2></div><span class="material-symbols-outlined">timeline</span></div>{status}{aviso_html}{body}</section></div>"###
    );

    // Stitch v2 shell con breadcrumb + nav activo resueltos vía page_ext (fill_shell).
    Html(templates::page_ext(
        "Exámenes — Study Platform",
        "Exámenes",
        "/dashboard/examenes",
        &content,
    ))
}

// ── presupuesto y respaldo ───────────────────────────────────────────

/// Pide la estructura de calificaciones dentro del presupuesto de la página.
///
/// Mikareno no siempre captura la vista de calificaciones en su raspado; en
/// ese caso responde con un **error de negocio** (no de transporte):
/// `{"error": "No se capturó la estructura de calificaciones", "hint":
/// "Intenta con force_refresh=true"}`. Esa respuesta llega como `Ok`, así que
/// sin mirarla la página mostraría "0 evaluaciones" sin explicar nada.
///
/// Aquí **no se reintenta**: ni en línea (bloquearía la página) ni con
/// `force_refresh` en segundo plano (obliga al servidor a raspar la
/// plataforma entera y, con ella, bloquea el resto de páginas durante
/// minutos). La página entrega de inmediato el respaldo por título y un
/// aviso que explica qué falta, sin inventar evaluaciones.
async fn actividades_con_presupuesto(
    client: &MikarenoClient,
    deadline: Instant,
) -> FuentePrincipal {
    // 1) Payload memorizado de una carga anterior: no toca la red.
    if let Some(raw) = memo_valido() {
        return FuentePrincipal {
            evaluaciones: evaluaciones_de_actividades(&raw),
            conectado: true,
            aviso: None,
        };
    }

    // 2) Llamada normal (cache-first del cliente MCP).
    //    `trimestre: 0` = todos los términos (el server lo interpreta así).
    let restante = deadline.saturating_duration_since(Instant::now());
    let llamada = time::timeout(
        restante,
        client.call_tool(
            "get_actividades_materia",
            json!({ "trimestre": 0, "bg": true }),
        ),
    )
    .await;

    match llamada {
        Ok(Ok(raw)) if !es_error_estructura(&raw) => {
            guardar_memo(&raw);
            return FuentePrincipal {
                evaluaciones: evaluaciones_de_actividades(&raw),
                conectado: true,
                aviso: None,
            };
        }
        // Falta la estructura de calificaciones: el servidor no la capturó.
        // Se registra UNA vez y la página sigue con el respaldo por título.
        Ok(Ok(_)) => return fuente_sin_estructura(),
        Ok(Err(e)) => {
            warn_una_vez(
                CLAVE_ESTRUCTURA,
                &format!("MCP: no se pudo consultar la estructura de calificaciones: {e}"),
            );
            FuentePrincipal {
                evaluaciones: Vec::new(),
                conectado: false,
                aviso: Some(format!(
                    "No se pudo consultar la estructura de calificaciones de Mikareno: {e}."
                )),
            }
        }
        Err(_) => {
            warn_una_vez(
                CLAVE_ESTRUCTURA,
                "MCP: la estructura de calificaciones no llegó dentro del presupuesto de la página",
            );
            FuentePrincipal {
                evaluaciones: Vec::new(),
                conectado: false,
                aviso: Some(
                    "Mikareno no respondió a tiempo con la estructura de calificaciones."
                        .to_string(),
                ),
            }
        }
    }
}

/// Fuente principal cuando el servidor no capturó la estructura de
/// calificaciones.
///
/// Sin reintentos (ni en línea ni con `force_refresh`): la página entrega de
/// inmediato el respaldo por título del calendario y un aviso que explica
/// qué falta. El fallo se registra con `warn!` **una sola vez**, no en cada
/// carga de página.
fn fuente_sin_estructura() -> FuentePrincipal {
    warn_una_vez(
        CLAVE_ESTRUCTURA,
        "MCP: no se capturó la estructura de calificaciones; \
         se muestran solo las coincidencias por título del calendario",
    );
    FuentePrincipal {
        evaluaciones: Vec::new(),
        conectado: true,
        aviso: Some(
            "Mikareno no devolvió la estructura de calificaciones: no hay \
             evaluaciones confirmadas. Mientras tanto solo se muestran las \
             coincidencias por título del calendario."
                .to_string(),
        ),
    }
}

/// Pide el calendario dentro del presupuesto de la página.
async fn calendario_con_presupuesto(
    client: &MikarenoClient,
    deadline: Instant,
) -> Option<Vec<CalendarioEvent>> {
    let restante = deadline.saturating_duration_since(Instant::now());
    match time::timeout(restante, client.calendario()).await {
        Ok(Ok(eventos)) => Some(eventos),
        _ => None,
    }
}

/// `true` si el payload es el error de negocio "No se capturó la estructura
/// de calificaciones" del servidor MCP (llega como respuesta exitosa).
fn es_error_estructura(raw: &Value) -> bool {
    raw.get("error")
        .and_then(Value::as_str)
        .map(|e| e.contains("estructura de calificaciones"))
        .unwrap_or(false)
}

/// Memo del último payload bueno de la estructura de calificaciones.
fn memo() -> &'static Mutex<Option<(StdInstant, Value)>> {
    static MEMO: OnceLock<Mutex<Option<(StdInstant, Value)>>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(None))
}

/// Guarda el payload bueno para las siguientes cargas.
fn guardar_memo(raw: &Value) {
    if let Ok(mut m) = memo().lock() {
        *m = Some((StdInstant::now(), raw.clone()));
    }
}

/// Payload memorizado si sigue vigente (`TTL_MEMO`).
fn memo_valido() -> Option<Value> {
    let m = memo().lock().ok()?;
    let (instante, raw) = m.as_ref()?;
    if instante.elapsed() < TTL_MEMO {
        Some(raw.clone())
    } else {
        None
    }
}

// ── parsing de fuentes ──────────────────────────────────────────────

/// Quita acentos y baja a minúsculas, para comparar texto de forma estable.
fn sin_acentos(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            _ => c,
        })
        .collect()
}

/// Divide el texto normalizado en palabras alfanuméricas.
fn palabras(s: &str) -> Vec<String> {
    sin_acentos(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// `true` si alguna palabra del texto empieza por una palabra clave de
/// evaluación ("examenes" → sí, "contestacion" → no).
fn coincide_evaluacion(texto: &str) -> bool {
    palabras(texto).iter().any(|p| {
        PALABRAS_EVALUACION
            .iter()
            .any(|kw| p.len() >= kw.len() && p.starts_with(kw))
    })
}

/// Extrae las actividades de evaluación de la estructura de calificaciones.
///
/// El servidor devuelve `{ materias: [{ materia, submenus: [{ submenu,
/// actividades: [{ actividad, fecha, … }] }] }] }`. Solo se toman los
/// submenús de evaluación (`EVALUACIONES`, `EXAMEN TRIMESTRAL`, …): las
/// tareas y trabajos quedan fuera.
fn evaluaciones_de_actividades(raw: &Value) -> Vec<Evaluacion> {
    let Some(materias) = raw.get("materias").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<Evaluacion> = Vec::new();

    for mat in materias {
        let materia = mat
            .get("materia")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let Some(submenus) = mat.get("submenus").and_then(Value::as_array) else {
            continue;
        };
        for sub in submenus {
            let clase = sub
                .get("submenu")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !coincide_evaluacion(&clase) {
                continue;
            }
            let Some(acts) = sub.get("actividades").and_then(Value::as_array) else {
                continue;
            };
            for act in acts {
                let titulo = act
                    .get("actividad")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if titulo.is_empty() {
                    continue;
                }
                let fecha = act
                    .get("fecha")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                out.push(Evaluacion {
                    titulo,
                    materia: materia.clone(),
                    fecha,
                    clase: clase.clone(),
                    origen: Origen::Mikareno,
                });
            }
        }
    }
    out
}

/// Respaldo honesto: eventos del calendario cuyo título parece una evaluación.
///
/// El calendario de Mikareno no distingue exámenes, así que estos eventos se
/// marcan como `Origen::Posible` y la UI los etiqueta como "Posible
/// evaluación" — nunca como examen confirmado.
fn posibles_de_calendario(eventos: &[CalendarioEvent]) -> Vec<Evaluacion> {
    eventos
        .iter()
        .filter(|e| coincide_evaluacion(&e.evento))
        .map(|e| Evaluacion {
            titulo: e.evento.clone(),
            materia: String::new(),
            fecha: e.fecha.clone(),
            clase: "Evento de calendario".to_string(),
            origen: Origen::Posible,
        })
        .collect()
}

/// Ordena por fecha real ascendente; lo que no parsea va al final.
fn ordenar_evaluaciones(evaluaciones: &mut [Evaluacion]) {
    evaluaciones.sort_by(|a, b| match (parse_fecha(a), parse_fecha(b)) {
        (Some(fa), Some(fb)) => fa.cmp(&fb),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.titulo.cmp(&b.titulo),
    });
}

// ── ventanas temporales y chips ──────────────────────────────────────

/// Ventana temporal de una evaluación respecto a hoy, usada para agrupar la
/// línea de tiempo. `EstaSemana` cubre el resto de la semana natural (hoy
/// hasta el domingo), `EsteMes` el resto del mes en curso y `MasAdelante` lo
/// que viene después de que termine el mes. `Anteriores` son las ya realizadas
/// y `SinFecha` las que no tienen una fecha parseable — nunca se les inventa
/// una cuenta atrás.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ventana {
    EstaSemana,
    EsteMes,
    MasAdelante,
    Anteriores,
    SinFecha,
}

/// Etiqueta visible de cada ventana.
fn etiqueta_ventana(v: Ventana) -> &'static str {
    match v {
        Ventana::EstaSemana => "Esta semana",
        Ventana::EsteMes => "Este mes",
        Ventana::MasAdelante => "Más adelante",
        Ventana::Anteriores => "Ya pasaron",
        Ventana::SinFecha => "Otras fechas",
    }
}

/// Fecha real de la evaluación (`YYYY-MM-DD`), o `None` si no se puede parsear.
fn parse_fecha(ev: &Evaluacion) -> Option<NaiveDate> {
    let dia = ev.fecha.get(..10)?;
    NaiveDate::parse_from_str(dia, "%Y-%m-%d").ok()
}

/// Días que quedan en el mes en curso después de hoy (0 si hoy es el último).
fn dias_restantes_del_mes(today: NaiveDate) -> i64 {
    let (year, month) = (today.year(), today.month());
    let primero_siguiente = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    primero_siguiente
        .map(|p| p.signed_duration_since(today).num_days() - 1)
        .unwrap_or(0)
}

/// Días que faltan para el domingo de la semana natural en curso.
fn dias_hasta_el_domingo(today: NaiveDate) -> i64 {
    6 - today.weekday().num_days_from_monday() as i64
}

/// Ventana temporal a la que pertenece una evaluación según su fecha real.
fn ventana_de(ev: &Evaluacion, today: NaiveDate) -> Ventana {
    let Some(fecha) = parse_fecha(ev) else {
        return Ventana::SinFecha;
    };
    let dias = fecha.signed_duration_since(today).num_days();
    if dias < 0 {
        Ventana::Anteriores
    } else if dias <= dias_hasta_el_domingo(today) {
        Ventana::EstaSemana
    } else if dias <= dias_restantes_del_mes(today) {
        Ventana::EsteMes
    } else {
        Ventana::MasAdelante
    }
}

/// Chip de cuenta atrás: texto honesto según los días reales de diferencia.
fn cuenta_atras(dias: i64) -> (String, &'static str) {
    match dias {
        0 => (
            "Hoy".to_string(),
            "text-primary-container bg-primary-container/10",
        ),
        1 => (
            "Mañana".to_string(),
            "text-secondary-container bg-secondary-container/10",
        ),
        n if n > 1 => (
            format!("En {} días", n),
            "text-on-surface-variant bg-surface-container-highest",
        ),
        -1 => ("Pasó hace 1 día".to_string(), "text-outline bg-outline/10"),
        n => (
            format!("Pasó hace {} días", -n),
            "text-outline bg-outline/10",
        ),
    }
}

/// Nombre del día en español a partir de la fecha real; si la fecha no es
/// válida se devuelve la fecha tal cual (nunca un día inventado).
fn dia_semana(ev: &Evaluacion) -> String {
    match ev.fecha.get(..10) {
        Some(dia) => templates::weekday_es(dia),
        None => ev.fecha.clone(),
    }
}

/// Etiqueta legible de la categoría de Mikareno ("EXAMEN TRIMESTRAL" →
/// "Examen trimestral", "EVALUACIONES" → "Evaluaciones").
fn etiqueta_clase(clase: &str) -> String {
    let texto = clase
        .split_whitespace()
        .map(|p| {
            let mut chars = p.chars();
            match chars.next() {
                Some(c) => {
                    format!("{}{}", c.to_uppercase(), chars.as_str().to_lowercase())
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if texto.is_empty() {
        "Evaluación".to_string()
    } else {
        texto
    }
}

/// Chip de procedencia: la categoría real de Mikareno o el aviso de "posible".
fn chip_origen(ev: &Evaluacion) -> String {
    let (texto, clases) = match ev.origen {
        Origen::Mikareno => (
            etiqueta_clase(&ev.clase),
            "text-primary-container bg-primary-container/10",
        ),
        Origen::Posible => (
            "Posible evaluación".to_string(),
            "text-yellow-400 bg-yellow-400/10",
        ),
    };
    format!(
        r###"<span class="font-label-sm text-[10px] {} px-space-sm py-0.5 rounded-full">{}</span>"###,
        clases,
        templates::html_escape(&texto)
    )
}

/// Subtítulo de la tarjeta: materia + categoría, o el origen del evento.
fn subtitulo(ev: &Evaluacion) -> String {
    let partes: Vec<String> = match ev.origen {
        Origen::Mikareno => vec![ev.materia.clone(), etiqueta_clase(&ev.clase)]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect(),
        Origen::Posible => vec!["Evento de calendario".to_string()],
    };
    partes.join(" · ")
}

/// Tarjeta de una evaluación con su chip de cuenta atrás y su chip de origen.
fn tarjeta_evaluacion(ev: &Evaluacion, today: NaiveDate) -> String {
    let fecha = parse_fecha(ev);
    let dias = fecha.map(|f| f.signed_duration_since(today).num_days());
    let es_hoy = dias == Some(0);
    let (bg, borde, dia_clase) = if es_hoy {
        (
            "bg-primary-container/10",
            "border-primary-container",
            "font-label-sm text-label-sm text-primary-container pulse-cyan",
        )
    } else {
        (
            "bg-surface-container",
            "border-outline",
            "font-label-sm text-label-sm text-on-surface-variant",
        )
    };
    // Sin fecha parseable no hay chip: mejor un hueco que una cuenta atrás inventada.
    let chip = match dias {
        Some(d) => {
            let (texto, clases) = cuenta_atras(d);
            format!(
                r###"<span class="font-label-sm text-[10px] {} px-space-sm py-0.5 rounded-full">{}</span>"###,
                clases, texto
            )
        }
        None => String::new(),
    };
    format!(
        r###"<article class="nova-exam-card {} {}">
            <div class="nova-exam-date"><span class="material-symbols-outlined">event</span><span class="{}">{}</span></div>
            <div class="nova-exam-copy"><h3>{}</h3><p>{}</p></div>
            <div class="nova-exam-badges">{}{}</div>
        </article>"###,
        bg,
        borde,
        dia_clase,
        templates::html_escape(&dia_semana(ev)),
        templates::html_escape(&ev.titulo),
        templates::html_escape(&subtitulo(ev)),
        chip,
        chip_origen(ev),
    )
}

/// Línea de tiempo agrupada por ventana temporal. Solo se dibujan los grupos
/// que tienen evaluaciones reales.
fn timeline_html(evaluaciones: &[Evaluacion], today: NaiveDate) -> String {
    let ventanas = [
        Ventana::EstaSemana,
        Ventana::EsteMes,
        Ventana::MasAdelante,
        Ventana::Anteriores,
        Ventana::SinFecha,
    ];
    let secciones: String = ventanas
        .iter()
        .filter_map(|v| {
            let items: Vec<&Evaluacion> = evaluaciones
                .iter()
                .filter(|e| ventana_de(e, today) == *v)
                .collect();
            if items.is_empty() {
                return None;
            }
            let tarjetas: String = items
                .iter()
                .map(|e| tarjeta_evaluacion(e, today))
                .collect::<Vec<_>>()
                .join("");
            Some(format!(
                r###"<section class="nova-exam-group">
                    <div class="nova-exam-group-head">
                        <h2 class="font-headline-sm text-headline-sm text-primary-container">{}</h2>
                        <span class="font-label-sm text-[10px] text-outline">{} {}</span>
                    </div>
                    <div class="nova-exam-group-list">{}</div>
                </section>"###,
                etiqueta_ventana(*v),
                items.len(),
                if items.len() == 1 {
                    "evaluación"
                } else {
                    "evaluaciones"
                },
                tarjetas,
            ))
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        r###"<div class="nova-exam-timeline" id="event-timeline">{}</div>"###,
        secciones
    )
}

/// Resumen honesto del encabezado: cuántas evaluaciones trae Mikareno, cuántas
/// son solo posibles y cuántas quedan por delante, según la fecha real.
fn resumen(evaluaciones: &[Evaluacion], today: NaiveDate) -> String {
    let mut confirmadas = 0usize;
    let mut posibles = 0usize;
    let mut futuros = 0usize;
    let mut pasados = 0usize;
    let mut sin_fecha = 0usize;
    for e in evaluaciones {
        match e.origen {
            Origen::Mikareno => confirmadas += 1,
            Origen::Posible => posibles += 1,
        }
        match parse_fecha(e) {
            Some(f) if f >= today => futuros += 1,
            Some(_) => pasados += 1,
            None => sin_fecha += 1,
        }
    }
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let mut partes: Vec<String> = Vec::new();
    if confirmadas > 0 {
        // El plural de "evaluación" es "evaluaciones", no "evaluacións".
        partes.push(format!(
            "{} {} de Mikareno",
            confirmadas,
            if confirmadas == 1 {
                "evaluación"
            } else {
                "evaluaciones"
            }
        ));
    }
    if posibles > 0 {
        partes.push(format!(
            "{} por título",
            if posibles == 1 {
                "1 posible evaluación".to_string()
            } else {
                format!("{} posibles evaluaciones", posibles)
            }
        ));
    }
    if futuros > 0 {
        partes.push(format!("{} próxima{}", futuros, plural(futuros)));
    }
    if pasados > 0 {
        partes.push(format!(
            "{} ya {}",
            pasados,
            if pasados == 1 {
                "realizada"
            } else {
                "realizadas"
            }
        ));
    }
    if sin_fecha > 0 {
        partes.push(format!("{} sin fecha verificable", sin_fecha));
    }
    if partes.is_empty() {
        "Sin evaluaciones".to_string()
    } else {
        partes.join(" · ")
    }
}

/// Aviso visible sobre el estado de la fuente principal de evaluaciones:
/// la página dice qué falta en vez de mostrar una lista vacía sin explicación.
fn aviso_html(aviso: &str) -> String {
    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-space-sm mb-space-sm">
            <div class="flex items-start space-x-3">
                <span class="material-symbols-outlined text-[20px] text-yellow-400">warning</span>
                <p class="font-body-sm text-body-sm text-on-surface-variant">{}</p>
            </div>
        </div>"###,
        templates::html_escape(aviso)
    )
}

/// Aclaración visible sobre el respaldo por título: esos eventos no son
/// exámenes confirmados, solo coincidencias por palabra clave.
fn nota_posibles(evaluaciones: &[Evaluacion]) -> String {
    let hay_posibles = evaluaciones.iter().any(|e| e.origen == Origen::Posible);
    if !hay_posibles {
        return String::new();
    }
    r###"<p class="font-body-sm text-body-sm text-outline mb-space-sm">«Posible evaluación» = evento del calendario cuyo título contiene una palabra clave de evaluación. Mikareno no lo confirma como examen.</p>"###.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(origen: Origen, titulo: &str, fecha: &str) -> Evaluacion {
        Evaluacion {
            titulo: titulo.to_string(),
            materia: "Matemáticas".to_string(),
            fecha: fecha.to_string(),
            clase: "EVALUACIONES".to_string(),
            origen,
        }
    }

    #[test]
    fn detecta_palabras_clave_sin_acentos() {
        assert!(coincide_evaluacion("Examen de Matemática"));
        assert!(coincide_evaluacion("EVALUACIÓN trimestral"));
        assert!(coincide_evaluacion("pruebas parciales"));
        assert!(coincide_evaluacion("Test de lectura"));
        assert!(!coincide_evaluacion("Día de clases"));
        // "test" no debe aparecer dentro de otra palabra.
        assert!(!coincide_evaluacion("Contestación de taller"));
    }

    #[test]
    fn solo_toma_submenus_de_evaluacion() {
        let raw = json!({
            "materias": [{
                "materia": "Matemáticas",
                "submenus": [
                    {"submenu": "EVALUACIONES", "actividades": [
                        {"actividad": "Examen 1", "fecha": "2026-11-04"}
                    ]},
                    {"submenu": "TAREAS", "actividades": [
                        {"actividad": "Tarea 1", "fecha": "2026-11-05"}
                    ]}
                ]
            }]
        });
        let evs = evaluaciones_de_actividades(&raw);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].titulo, "Examen 1");
        assert_eq!(evs[0].materia, "Matemáticas");
        assert_eq!(evs[0].origen, Origen::Mikareno);
    }

    #[test]
    fn fecha_no_parseable_va_a_otras_fechas_sin_chip() {
        let hoy = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let sin_fecha = ev(Origen::Mikareno, "Examen 1", "sin fecha");
        assert_eq!(ventana_de(&sin_fecha, hoy), Ventana::SinFecha);
        assert!(parse_fecha(&sin_fecha).is_none());
        let con_fecha = ev(Origen::Mikareno, "Examen 1", "2026-10-20");
        assert_eq!(parse_fecha(&con_fecha), Some(hoy.succ_opt().unwrap()));
        let chip = tarjeta_evaluacion(&con_fecha, hoy);
        assert!(chip.contains("Mañana"));
    }

    #[test]
    fn detecta_error_de_estructura_de_calificaciones() {
        // Respuesta real del servidor MCP: llega como `Ok`, no como error.
        let raw = json!({
            "error": "No se capturó la estructura de calificaciones",
            "hint": "Intenta con force_refresh=true"
        });
        assert!(es_error_estructura(&raw));

        // Un payload normal NO es error de estructura.
        let ok = json!({"materias": [], "total_materias": 0});
        assert!(!es_error_estructura(&ok));
        assert!(!es_error_estructura(&json!({})));
    }

    #[test]
    fn resumen_pluraliza_en_castellano() {
        let hoy = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        let dos = vec![
            ev(Origen::Mikareno, "Examen 1", "2026-10-20"),
            ev(Origen::Mikareno, "Examen 2", "2026-10-21"),
        ];
        let texto = resumen(&dos, hoy);
        assert!(texto.contains("2 evaluaciones de Mikareno"), "{texto}");
        assert!(!texto.contains("evaluacións"), "{texto}");
        assert!(!texto.contains("realizads"), "{texto}");

        // Una sola evaluación: singular.
        let una = vec![ev(Origen::Mikareno, "Examen 1", "2026-10-20")];
        let texto = resumen(&una, hoy);
        assert!(texto.contains("1 evaluación de Mikareno"), "{texto}");
        assert!(texto.contains("1 próxima"), "{texto}");

        // Posibles: "1 posible evaluación" / "N posibles evaluaciones".
        let mut con_posible = una.clone();
        con_posible.push(Evaluacion {
            titulo: "Examen de prueba".to_string(),
            materia: String::new(),
            fecha: "2026-10-20".to_string(),
            clase: "Evento de calendario".to_string(),
            origen: Origen::Posible,
        });
        let texto = resumen(&con_posible, hoy);
        assert!(texto.contains("1 posible evaluación por título"), "{texto}");
        assert!(!texto.contains("evaluacións"), "{texto}");
    }

    #[test]
    fn el_memo_guarda_y_devuelve_el_payload() {
        let raw = json!({"materias": [{"materia": "Física"}]});
        guardar_memo(&raw);
        let guardado = memo_valido().expect("memo vigente");
        assert_eq!(guardado, raw);
    }

    #[test]
    fn sin_estructura_de_calificaciones_la_pagina_va_al_respaldo_por_titulo() {
        let fuente = fuente_sin_estructura();
        // Sin evaluaciones confirmadas: no se inventa ninguna.
        assert!(fuente.evaluaciones.is_empty());
        // El MCP respondió (no es un problema de conexión).
        assert!(fuente.conectado);
        // Y el aviso dice que la página sigue con las coincidencias por título.
        let aviso = fuente.aviso.expect("aviso honesto");
        assert!(aviso.contains("coincidencias por título"), "{aviso}");
    }
}
