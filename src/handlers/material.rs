//! Generador de material académico (Stitch v2).
//!
//! GET /dashboard/material — formulario con selector de materia (notas reales
//! de Mikareno), tipo de material y tema, con botón de generación.
//! POST /dashboard/material — genera material de estudio vía OpenRouter.
//! Responde honestamente con error de modo demo cuando OPENROUTER_API_KEY no
//! está configurada; no genera contenido simulado.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{Json, Request, State},
    http::StatusCode,
    response::Html,
};
use serde::Deserialize;
use tokio::time;
use tracing::warn;

use crate::academic::mikareno_client;
use crate::academic::templates::{subjects_from_notas, SubjectSummary};
use crate::middleware::auth::extract_session_cookie;
use crate::AppState;

/// Presupuesto total de la fase MCP al cargar la página.
///
/// El cliente MCP tiene deadlines propios (20 s de `initialize` + 25 s de
/// `tools/call`), que en el peor caso dejaban la página ~50 s esperando.
/// Este presupuesto recorta la espera: si Mikareno no responde dentro de él,
/// la página se renderiza **sin asignaturas** y con un estado honesto, en
/// vez de quedarse colgada.
const MCP_PRESUPUESTO_SECS: u64 = 5;

/// TTL de la caché de `MaterialData`.
///
/// El contenido académico cambia poco y la fase MCP es cara: 30 s evitan
/// repetir el scrape en recargas seguidas.
const CACHE_TTL_SECS: u64 = 30;

/// Presupuesto para los datos del shell (topbar/aside).
///
/// `stitch_panel::fetch_shell_data` también habla con el MCP y hereda sus
/// deadlines largos (10 s de init + 25 s de call = hasta ~35 s). Acotarlo
/// aquí mantiene la página rápida aunque Mikareno no responda.
const SHELL_PRESUPUESTO_SECS: u64 = 8;

// ── Shell + nav-rail ─────────────────────────────────────────────────────────
//
// El shell (aside, topbar, rutas del menú) es el de `stitch_v2`: esta página
// usa el mismo markup que el resto, con el item "generador-de-material"
// resaltado. No hay una copia local del rail.


/// Contenido de la página (solo el <main> content, sin wrappers).
const CONTENT_MATERIAL: &str = include_str!("../../static/stitch-v2/_content/clean/material.html");

// ── helpers de render ────────────────────────────────────────────────────────

fn esc(s: &str) -> String {
    crate::academic::templates::html_escape(s)
}

/// Datos reales del generador de material.
#[derive(Clone)]
pub struct MaterialData {
    /// Asignaturas reales del estudiante, agrupadas desde sus notas.
    pub subjects: Vec<SubjectSummary>,
    /// True si el MCP de Mikareno está conectado.
    pub connected: bool,
    /// True si OPENROUTER_API_KEY está configurada.
    pub api_configured: bool,
    /// Modelo de IA configurado.
    pub model: String,
    /// True si Mikareno no respondió dentro de `MCP_PRESUPUESTO_SECS`.
    ///
    /// Es un estado distinto de "no conectado": el servidor existe pero
    /// tardó demasiado, así que la página lo dice explícitamente.
    pub timeout: bool,
    /// URL del MCP que originó los datos.
    ///
    /// Si cambia la URL configurada, la caché deja de ser válida.
    pub mcp_url: String,
}

/// Caché corta de los datos del generador.
///
/// Solo se cachean respuestas completas (nunca un estado degradado por
/// timeout), para que un fallo puntual del MCP no se congele 30 s.
static CACHE: Mutex<Option<(Instant, MaterialData)>> = Mutex::new(None);

/// Copia de la caché vigente, si la hay.
///
/// Vigente = rellenada hace menos de `CACHE_TTL_SECS` y obtenida con el
/// mismo MCP configurado ahora mismo. Un `Mutex` envenenado se trata como
/// caché vacía (se recalcula), nunca como un panic.
fn cache_vigente(mcp_url: &str) -> Option<MaterialData> {
    let guard = CACHE.lock().ok()?;
    let entrada: &Option<(Instant, MaterialData)> = &*guard;
    let (instante, datos) = entrada.as_ref()?;
    if instante.elapsed() >= Duration::from_secs(CACHE_TTL_SECS) {
        return None;
    }
    if datos.mcp_url.as_str() != mcp_url {
        return None;
    }
    Some(datos.clone())
}

/// Guarda una copia en la caché (best effort: si el lock está envenenado, se
/// sigue sirviendo la respuesta recién calculada).
fn cache_guardar(datos: &MaterialData) {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((Instant::now(), datos.clone()));
    }
}

/// Resultado de la fase MCP dentro del presupuesto de tiempo.
enum FaseMcp {
    /// El MCP respondió: asignaturas con notas (la lista puede ir vacía).
    Conectado(Vec<SubjectSummary>),
    /// No hay cliente MCP configurado o no se pudo abrir la sesión.
    SinConexion,
    /// El MCP no respondió dentro de `MCP_PRESUPUESTO_SECS`.
    Timeout,
}

/// Recolecta los datos para el generador de material.
///
/// Nunca falla: si Mikareno no responde, `subjects` queda vacío y se muestra
/// un estado honesto. La lista de asignaturas proviene de las notas reales del
/// estudiante (via `get_student_full → promedios.stats.courseName`), agrupadas
/// con `subjects_from_notas` (mismo criterio que `/dashboard/subjects`).
///
/// La fase MCP está acotada a `MCP_PRESUPUESTO_SECS` y el resultado se
/// cachea `CACHE_TTL_SECS`: la carga de la página no depende de que Mikareno
/// esté vivo.
pub async fn fetch_material_data(state: &AppState) -> MaterialData {
    let api_configured = state.config.openrouter_api_key.is_some();
    let model = state.config.openrouter_model.clone();
    let mcp_url = state.config.mikareno_mcp_url.clone();

    // Caché corta: si el MCP ya respondió hace menos de `CACHE_TTL_SECS` y
    // la configuración no ha cambiado, se reutiliza.
    if let Some(cacheada) = cache_vigente(&mcp_url) {
        return MaterialData {
            api_configured,
            model,
            ..cacheada
        };
    }

    let mut d = MaterialData {
        subjects: Vec::new(),
        connected: false,
        api_configured,
        model,
        timeout: false,
        mcp_url,
    };

    match fase_mcp(state).await {
        FaseMcp::Conectado(subjects) => {
            d.connected = true;
            d.subjects = subjects;
        }
        FaseMcp::SinConexion => {}
        FaseMcp::Timeout => d.timeout = true,
    }

    // Un timeout no se cachea: la siguiente carga vuelve a intentarlo.
    if !d.timeout {
        cache_guardar(&d);
    }

    d
}

/// Ejecuta la fase MCP con un presupuesto de tiempo cerrado.
///
/// `initialize` + `notas_full` comparten el mismo presupuesto: si no caben,
/// se corta y se devuelve un estado degradado honesto.
async fn fase_mcp(state: &AppState) -> FaseMcp {
    let Some(client) = mikareno_client(state) else {
        return FaseMcp::SinConexion;
    };

    let fase = async {
        if client.initialize().await.is_err() {
            return None;
        }
        match client.notas_full().await {
            Ok(notas) => Some(subjects_from_notas(&notas)),
            Err(_) => None,
        }
    };

    match time::timeout(Duration::from_secs(MCP_PRESUPUESTO_SECS), fase).await {
        Ok(Some(subjects)) => FaseMcp::Conectado(subjects),
        Ok(None) => FaseMcp::SinConexion,
        Err(_) => {
            warn!(
                "material: Mikareno no respondió a tiempo (presupuesto {}s); la página se renderiza sin asignaturas",
                MCP_PRESUPUESTO_SECS
            );
            FaseMcp::Timeout
        }
    }
}

/// Opciones `<option>` para el selector de materias.
fn render_materia_options(d: &MaterialData) -> String {
    if d.subjects.is_empty() {
        if d.timeout {
            r#"<option value="">Mikareno no responde ahora: no hay asignaturas</option>"#.to_string()
        } else if d.connected {
            r#"<option value="">Sin materias con notas</option>"#.to_string()
        } else {
            r#"<option value="">Mikareno no conectado</option>"#.to_string()
        }
    } else {
        let mut opts = vec![r#"<option value="">Selecciona una materia</option>"#.to_string()];
        opts.extend(d.subjects.iter().map(|s| {
            format!("<option value=\"{}\">{}</option>", esc(&s.name), esc(&s.name))
        }));
        opts.join("\n")
    }
}

/// Badge de estado del MCP — conectado, sin conexión o sin respuesta a tiempo.
fn render_mcp_status(d: &MaterialData) -> String {
    if d.timeout {
        r#"<span class="flex items-center gap-1.5 text-[11px] text-outline">
            <span class="w-1.5 h-1.5 rounded-full bg-outline"></span>
            Mikareno: no responde ahora
        </span>"#.to_string()
    } else if d.connected {
        r#"<span class="flex items-center gap-1.5 text-[11px] text-secondary">
            <span class="w-1.5 h-1.5 rounded-full bg-secondary animate-pulse"></span>
            Mikareno: conectado
        </span>"#.to_string()
    } else {
        r#"<span class="flex items-center gap-1.5 text-[11px] text-outline">
            <span class="w-1.5 h-1.5 rounded-full bg-outline"></span>
            Mikareno: sin conexión
        </span>"#.to_string()
    }
}

/// Clase del punto de color para el estado de la IA.
fn render_ai_dot(d: &MaterialData) -> String {
    if d.api_configured {
        "bg-secondary animate-pulse".to_string()
    } else {
        "bg-outline".to_string()
    }
}

/// Nombre del modelo de IA configurado.
fn render_ai_model(d: &MaterialData) -> String {
    if d.model.is_empty() {
        "Sin modelo".to_string()
    } else {
        esc(&d.model)
    }
}

/// Texto inicial en el área de resultados.
fn render_initial_hint(d: &MaterialData) -> String {
    if d.api_configured {
        r#"<p class="font-body-sm text-body-sm text-on-surface-variant flex items-center gap-2">
            <span class="material-symbols-outlined text-[20px] text-outline">auto_awesome</span>
            Selecciona una materia, escribe un tema y elige el tipo de material, luego haz clic en Generar.
        </p>"#.to_string()
    } else {
        r#"<p class="font-body-sm text-body-sm text-outline flex items-center gap-2">
            <span class="material-symbols-outlined text-[20px] text-yellow-400">warning</span>
            <span>Modo demo: configura <code class="font-code-inline text-[10px] bg-surface-container/50 px-1 py-0.5 rounded">OPENROUTER_API_KEY</code> para generar material.</span>
        </p>"#.to_string()
    }
}

/// Reemplaza los marcadores `<!--SLOT:...-->` por HTML con datos reales.
fn fill_material(content: &str, d: &MaterialData) -> String {
    content
        .replace("<!--SLOT:MATERIA_OPTIONS-->", &render_materia_options(d))
        .replace("<!--SLOT:MCP_STATUS-->", &render_mcp_status(d))
        .replace("<!--SLOT:AI_DOT-->", &render_ai_dot(d))
        .replace("<!--SLOT:AI_MODEL-->", &render_ai_model(d))
        .replace("<!--SLOT:INITIAL_HINT-->", &render_initial_hint(d))
}

/// Clases Tailwind del control de asignatura del formulario de IA.
/// Mismo tratamiento que el input de texto que sustituye.
const AI_SUBJECT_CONTROL_CLASSES: &str = "w-full px-space-sm py-2 rounded-lg bg-surface-container-high border border-outline-variant text-on-surface font-body-md text-body-md placeholder:text-outline focus:outline-none focus:ring-1 focus:ring-primary-container";

/// Valor de la opción "Otra (escribir)" del selector de asignatura.
/// Se inyecta en el JS sustituyendo el marcador `__OTHER__`.
const AI_SUBJECT_OTHER: &str = "__otra__";

/// JS mínimo (inline, sin dependencias) para el selector de asignatura.
///
/// Al elegir "Otra (escribir)" revela el campo de texto libre; el valor que
/// se envía al backend vive siempre en el input oculto `subject`, para que el
/// formulario mantenga un único campo `subject` en el POST.
const AI_SUBJECT_JS_TEMPLATE: &str = r###"    <script>
    (function(){
      var sel = document.getElementById('ai-subject-select');
      var hidden = document.getElementById('ai-subject-value');
      var wrap = document.getElementById('ai-subject-other-wrap');
      var other = document.getElementById('ai-subject-other');
      if(!sel || !hidden) return;
      function sync(){
        if(sel.value === '__OTHER__'){
          hidden.value = other ? other.value.trim() : '';
        } else {
          hidden.value = sel.value;
          if(other) other.value = '';
        }
      }
      function toggle(){
        if(wrap) wrap.classList.toggle('hidden', sel.value !== '__OTHER__');
        sync();
      }
      sel.addEventListener('change', toggle);
      if(other) other.addEventListener('input', sync);
      toggle();
    })();
    </script>"###;

/// Campo "Asignatura" del formulario de IA.
///
/// Con el MCP de Mikareno conectado y asignaturas reales disponibles, renderiza
/// un `<select>` con esas asignaturas más la opción "Otra (escribir)", que
/// revela el campo de texto libre. Sin conexión —o sin asignaturas con notas—
/// se mantiene el campo de texto libre con un aviso honesto: nunca se inventan
/// asignaturas ni datos simulados.
fn render_ai_subject_field(d: &MaterialData) -> String {
    if d.connected && !d.subjects.is_empty() {
        let mut opts = vec![r#"<option value="">Selecciona una asignatura</option>"#.to_string()];
        opts.extend(d.subjects.iter().map(|s| {
            format!("<option value=\"{}\">{}</option>", esc(&s.name), esc(&s.name))
        }));
        opts.push(format!(
            "<option value=\"{}\">Otra (escribir)</option>",
            AI_SUBJECT_OTHER
        ));

        return format!(
            r###"            <label class="font-label-sm text-label-sm text-on-surface-variant mb-1 flex items-center gap-1" for="ai-subject-select">
              <span class="material-symbols-outlined text-[18px] text-primary-container">menu_book</span>
              <span>Asignatura (opcional)</span>
            </label>
            <select id="ai-subject-select" class="{classes} appearance-none">
              {options}
            </select>
            <input type="hidden" name="subject" id="ai-subject-value" value="">
            <div id="ai-subject-other-wrap" class="hidden mt-1">
              <input id="ai-subject-other" type="text" placeholder="Escribe la asignatura"
                class="{classes}" autocomplete="off">
            </div>
            <p class="font-code-inline text-[10px] text-outline mt-1">Asignaturas reales de Mikareno</p>"###,
            classes = AI_SUBJECT_CONTROL_CLASSES,
            options = opts.join("\n              "),
        );
    }

    let hint = if d.timeout {
        "Mikareno no responde ahora — escribe la asignatura a mano"
    } else if d.connected {
        "Mikareno no devolvió asignaturas con notas — escribe la asignatura a mano"
    } else {
        "Mikareno no conectado — escribe la asignatura a mano"
    };

    format!(
        r###"            <label class="font-label-sm text-label-sm text-on-surface-variant mb-1 flex items-center gap-1" for="ai-subject-input">
              <span class="material-symbols-outlined text-[18px] text-primary-container">menu_book</span>
              <span>Asignatura (opcional)</span>
            </label>
            <input id="ai-subject-input" name="subject" type="text"
                placeholder="Ej: Matemáticas"
                class="{classes}"
                autocomplete="off">
            <p class="font-code-inline text-[10px] text-outline mt-1">{hint}</p>"###,
        classes = AI_SUBJECT_CONTROL_CLASSES,
        hint = hint,
    )
}

/// Sección "Material de estudio IA": UI HTMX sobre endpoints que YA existen —
/// no implementa backend nuevo.
///
/// Rutas exactas usadas (registradas en `src/ai/mod.rs`, handlers en
/// `src/ai/handlers.rs`):
/// - `POST /api/ai/summary`     → `summary_handler`,     body: `content`
/// - `POST /api/ai/quiz`        → `quiz_handler`,        body: `content, num_questions`
/// - `POST /api/ai/study-sheet` → `study_sheet_handler`, body: `content`
///
/// Cada botón hace `hx-post` a su ruta real y vuelca la respuesta (JSON) tal
/// cual en su propio target estilizado con `font-body-sm`. La sección solo se
/// renderiza cuando `state.config.openrouter_configured` (equivalente a
/// `d.api_configured == config.openrouter_api_key.is_some()`, ver
/// `config::Config::public`); si no, bloque "IA no configurada" con la misma
/// estética que `mcp_status_banner`.
///
/// El campo `subject` es un `<select>` con las asignaturas reales de Mikareno
/// cuando el MCP está conectado; con la opción "Otra (escribir)" como
/// salida manual (ver `render_ai_subject_field`).
fn render_ai_kit(d: &MaterialData) -> String {
    if !d.api_configured {
        return r###"    <div class="w-full max-w-4xl mx-auto flex flex-col gap-space-md mt-space-lg">
      <div class="bg-surface-container border border-outline rounded-xl p-3 flex items-center gap-2">
        <span class="w-2 h-2 rounded-full bg-outline"></span>
        <span class="material-symbols-outlined text-[16px] text-outline">key</span>
        <span class="font-body-sm text-body-sm text-on-surface-variant">● IA no configurada — añade OPENROUTER_API_KEY para habilitar resumen, quiz y ficha de estudio</span>
      </div>
    </div>"###
            .to_string();
    }

    let subject_field = render_ai_subject_field(d);
    // El JS del selector sólo hace falta cuando hay un `<select>` real.
    let subject_js = if d.connected && !d.subjects.is_empty() {
        AI_SUBJECT_JS_TEMPLATE.replace("__OTHER__", AI_SUBJECT_OTHER)
    } else {
        String::new()
    };

    // htmx no está en el shell stitch-v2 (head.html); se carga solo en esta
    // página, igual que hacen auth.rs y las plantillas académicas.
    format!(
        r###"    <div class="w-full max-w-4xl mx-auto flex flex-col gap-space-md mt-space-lg">
      <div class="bg-surface-container border border-outline-variant/40 rounded-xl p-space-md">
        <h2 class="font-headline-sm text-headline-sm text-on-surface flex items-center gap-space-xs mb-space-sm">
          <span class="material-symbols-outlined text-[20px] text-primary-container">auto_awesome</span>
          <span>Material de estudio IA</span>
        </h2>
        <form id="ai-kit-form" class="flex flex-col gap-space-md">
          <div>
            <label class="font-label-sm text-label-sm text-on-surface-variant mb-1 flex items-center gap-1" for="ai-content-input">
              <span class="material-symbols-outlined text-[18px] text-primary-container">article</span>
              <span>Contenido</span>
            </label>
            <textarea id="ai-content-input" name="content" rows="4"
                placeholder="Pega aquí el contenido académico (texto, apuntes, tema...)"
                class="w-full px-space-sm py-2 rounded-lg bg-surface-container-high border border-outline-variant text-on-surface font-body-md text-body-md placeholder:text-outline focus:outline-none focus:ring-1 focus:ring-primary-container resize-y"></textarea>
            <p class="font-code-inline text-[10px] text-outline mt-1">Máx. 20000 caracteres</p>
          </div>
          <div>
{subject_field}
            <input type="hidden" name="num_questions" value="5">
          </div>
          <div class="flex flex-wrap items-center gap-space-md">
            <button type="button" hx-post="/api/ai/summary" hx-include="closest form" hx-target="#ai-summary-out" hx-indicator="#ai-kit-loading"
                class="flex items-center gap-1.5 px-space-sm py-2 rounded-lg bg-primary-container text-on-primary-container font-label-sm hover:bg-primary-fixed-dim transition-all">
              <span class="material-symbols-outlined text-[16px]">notes</span>
              <span>Resumen</span>
            </button>
            <button type="button" hx-post="/api/ai/quiz" hx-include="closest form" hx-target="#ai-quiz-out" hx-indicator="#ai-kit-loading"
                class="flex items-center gap-1.5 px-space-sm py-2 rounded-lg bg-primary-container text-on-primary-container font-label-sm hover:bg-primary-fixed-dim transition-all">
              <span class="material-symbols-outlined text-[16px]">quiz</span>
              <span>Quiz</span>
            </button>
            <button type="button" hx-post="/api/ai/study-sheet" hx-include="closest form" hx-target="#ai-study-sheet-out" hx-indicator="#ai-kit-loading"
                class="flex items-center gap-1.5 px-space-sm py-2 rounded-lg bg-primary-container text-on-primary-container font-label-sm hover:bg-primary-fixed-dim transition-all">
              <span class="material-symbols-outlined text-[16px]">receipt_long</span>
              <span>Ficha de estudio</span>
            </button>
            <span id="ai-kit-loading" class="htmx-indicator font-body-sm text-body-sm text-on-surface-variant flex items-center gap-1.5">
              <span class="material-symbols-outlined text-[18px] animate-spin text-secondary">progress_activity</span>
              <span>Generando con IA...</span>
            </span>
          </div>
        </form>
        <div class="grid grid-cols-1 md:grid-cols-3 gap-space-md mt-space-md">
          <div id="ai-summary-out" aria-live="polite" class="font-body-sm text-body-sm text-on-surface whitespace-pre-wrap break-words rounded-lg border border-outline-variant/40 bg-surface-container-high/60 px-space-sm py-2 min-h-[3rem]"></div>
          <div id="ai-quiz-out" aria-live="polite" class="font-body-sm text-body-sm text-on-surface whitespace-pre-wrap break-words rounded-lg border border-outline-variant/40 bg-surface-container-high/60 px-space-sm py-2 min-h-[3rem]"></div>
          <div id="ai-study-sheet-out" aria-live="polite" class="font-body-sm text-body-sm text-on-surface whitespace-pre-wrap break-words rounded-lg border border-outline-variant/40 bg-surface-container-high/60 px-space-sm py-2 min-h-[3rem]"></div>
        </div>
      </div>
    </div>
{subject_js}
    <script src="https://unpkg.com/htmx.org@2.0.3/dist/htmx.min.js"></script>"###,
        subject_field = subject_field,
        subject_js = subject_js,
    )
}

// ── handlers ────────────────────────────────────────────────────────────────

/// GET /dashboard/material — formulario de generación de material.
pub async fn page(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> (StatusCode, Html<String>) {
    let data = fetch_material_data(&state).await;
    let content = fill_material(CONTENT_MATERIAL, &data);
    let content = format!("{}\n{}", content, render_ai_kit(&data));
    let authenticated = state.config.auth_disabled || extract_session_cookie(&req).is_some();
    let html = crate::handlers::stitch_v2::stitch_v2_page(
        "generador-de-material",
        &content,
        authenticated,
    );
    // El shell (topbar/aside) también habla con el MCP y hereda sus deadlines
    // largos (10 s de init + 25 s de call). Se acota aquí para que esta
    // página no siga esperando aunque la fase de material ya haya caído.
    let shell = match time::timeout(
        Duration::from_secs(SHELL_PRESUPUESTO_SECS),
        crate::handlers::stitch_panel::fetch_shell_data(&state),
    )
    .await
    {
        Ok(shell) => shell,
        Err(_) => {
            warn!(
                "material: el shell no respondió a tiempo (presupuesto {}s)",
                SHELL_PRESUPUESTO_SECS
            );
            shell_neutro(&state)
        }
    };
    let html = crate::handlers::stitch_panel::fill_shell(&html, &shell);
    (StatusCode::OK, Html(html))
}

/// Shell neutro para cuando los datos del shell no llegan a tiempo.
///
/// Mismo criterio de recuento que `stitch_panel::fetch_shell_data` (el MCP
/// nativo de la app + los MCP configurados), pero sin datos de Mikareno: el
/// nombre real no se inventa, se muestra "Estudiante".
fn shell_neutro(state: &AppState) -> crate::handlers::stitch_panel::ShellData {
    let mut mcp_count = 1; // el servidor MCP nativo de la app siempre corre
    if !state.config.mikareno_mcp_url.is_empty() {
        mcp_count += 1;
    }
    if !state.config.genoffice_mcp_url.is_empty() {
        mcp_count += 1;
    }
    crate::handlers::stitch_panel::ShellData {
        name: "Estudiante".to_string(),
        initial: String::new(),
        role: String::new(),
        mcp_ok: false,
        mcp_count,
        vault_pct: "0".to_string(),
    }
}

/// Body for POST /dashboard/material.
#[derive(Debug, Deserialize)]
pub struct GenerateRequest {
    pub materia: String,
    pub tema: String,
    pub tipo: String,
}

/// POST /dashboard/material — genera material de estudio.
///
/// El `tipo` del formulario decide qué se genera: `Quiz` (cuestionario con
/// `questions`), `Resumen`, `Ficha de estudio` o `Explicación` (texto).
/// Un `tipo` desconocido responde 400.
///
/// Si `OPENROUTER_API_KEY` no está configurada, responde honestamente con
/// `{"ok":false,"error":"Modo demo: configura OPENROUTER_API_KEY"}` sin
/// fingir generación.
pub async fn generate(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<GenerateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Honest demo check: no API key → no generation at all.
    if state.config.openrouter_api_key.is_none() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": false,
                "error": "Modo demo: configura OPENROUTER_API_KEY"
            })),
        );
    }

    // Validate inputs.
    let materia = payload.materia.trim();
    let tema = payload.tema.trim();
    let tipo = payload.tipo.trim();

    if materia.is_empty() || tema.is_empty() || tipo.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "ok": false,
                "error": "Materia, tema y tipo son obligatorios"
            })),
        );
    }

    // El selector de tipo decide qué material se genera: "Resumen" ya no
    // devuelve un cuestionario.
    let tipo_sel = match tipo_material(tipo) {
        Some(t) => t,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "error": format!(
                        "Tipo de material desconocido: '{}'. Usa Quiz, Resumen, Ficha de estudio o Explicación.",
                        tipo
                    )
                })),
            );
        }
    };

    match generar(&state, tipo_sel, materia, tema).await {
        Ok(material) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "tipo": tipo,
                "materia": materia,
                "tema": tema,
                "material": material
            })),
        ),
        Err(e) => {
            tracing::warn!(
                endpoint = "material-generate",
                detail = %e.log_detail(),
                "material generation failed"
            );
            (
                e.status_code(),
                Json(serde_json::json!({
                    "ok": false,
                    "error": e.client_message()
                })),
            )
        }
    }
}

/// Tipos de material que el generador sabe producir.
///
/// El `value` del `<select id="tipo-select">` (plantilla
/// `static/stitch-v2/_content/clean/material.html`) decide **qué** se genera:
/// antes todos los valores caían siempre en el cuestionario, así que elegir
/// "Resumen" o "Ficha de estudio" devolvía preguntas.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TipoMaterial {
    /// Cuestionario de opción múltiple (JSON con `questions`).
    Quiz,
    /// Resumen en texto plano.
    Resumen,
    /// Ficha de estudio en Markdown.
    FichaEstudio,
    /// Explicación del tema en texto plano.
    Explicacion,
}

/// Normaliza el valor del selector a un tipo conocido.
///
/// Acepta los `value` exactos de la plantilla y algunas variantes en español
/// (minúsculas, con o sin tilde) para que un POST directo no falle por
/// mayúsculas.
pub fn tipo_material(tipo: &str) -> Option<TipoMaterial> {
    match tipo.trim().to_ascii_lowercase().as_str() {
        "quiz" | "cuestionario" => Some(TipoMaterial::Quiz),
        "resumen" | "summary" => Some(TipoMaterial::Resumen),
        "ficha de estudio" | "ficha" | "study sheet" => Some(TipoMaterial::FichaEstudio),
        "explicación" | "explicacion" | "explicación" | "explanation" => {
            Some(TipoMaterial::Explicacion)
        }
        _ => None,
    }
}

/// Genera el material pedido con la IA y lo devuelve como `serde_json::Value`.
///
/// El valor es un objeto con `questions` para el cuestionario (lo consume
/// `renderQuiz` en la plantilla) y un string de texto para el resto (lo
/// consume `renderText`).
async fn generar(
    state: &Arc<AppState>,
    tipo: TipoMaterial,
    materia: &str,
    tema: &str,
) -> Result<serde_json::Value, crate::ai::AiError> {
    let assistant = crate::ai::AiAssistant::new(Arc::clone(state));
    let sistema = format!(
        "Eres un docente de {} que explica con claridad y en español.",
        materia
    );

    match tipo {
        TipoMaterial::Quiz => {
            let prompt = format!(
                "Genera un cuestionario de opción múltiple sobre el tema '{}' para la asignatura '{}'.",
                tema, materia
            );
            let quiz = assistant.generate_quiz(&prompt, 5).await?;
            Ok(serde_json::to_value(&quiz)
                .unwrap_or_else(|_| serde_json::json!({ "error": "Error al serializar el cuestionario" })))
        }
        TipoMaterial::Resumen => {
            let prompt = format!(
                "Genera un resumen académico del tema '{}' de la asignatura '{}': \
                 conceptos clave, definiciones, fórmulas y un ejemplo. \
                 Responde en español con Markdown, máximo 500 tokens.",
                tema, materia
            );
            let texto = texto_ia(state, "resumen", sistema, prompt, 900).await?;
            Ok(serde_json::Value::String(texto))
        }
        TipoMaterial::FichaEstudio => {
            let prompt = format!(
                "Genera una hoja de estudio en Markdown sobre el tema '{}' de la asignatura '{}': \
                 título, resumen, conceptos clave, fórmulas y ejemplos. Máximo 900 tokens.",
                tema, materia
            );
            let texto = texto_ia(state, "ficha-estudio", sistema, prompt, 1400).await?;
            Ok(serde_json::Value::String(texto))
        }
        TipoMaterial::Explicacion => {
            let prompt = format!(
                "Explica el tema '{}' de la asignatura '{}': qué es, cómo se aplica y un ejemplo.",
                tema, materia
            );
            let texto = texto_ia(state, "explicacion", sistema, prompt, 1200).await?;
            Ok(serde_json::Value::String(texto))
        }
    }
}

/// Pide a la IA un texto libre con el rol de sistema indicado.
///
/// Se usa `chat_completion` (y no `AiAssistant::chat`) para no escribir en el
/// historial de conversaciones, que es el del chat del asistente.
async fn texto_ia(
    state: &Arc<AppState>,
    endpoint: &'static str,
    sistema: String,
    prompt: String,
    max_tokens: u32,
) -> Result<String, crate::ai::AiError> {
    let mut tracker = crate::ai::openrouter::CostTracker::new(state.config.openrouter_max_cost_usd);
    let messages = vec![
        crate::ai::openrouter::ChatMessage {
            role: "system".to_string(),
            content: sistema,
        },
        crate::ai::openrouter::ChatMessage {
            role: "user".to_string(),
            content: prompt,
        },
    ];
    crate::ai::openrouter::chat_completion(
        &state.db,
        &state.config,
        &mut tracker,
        endpoint,
        messages,
        max_tokens,
    )
    .await
}
