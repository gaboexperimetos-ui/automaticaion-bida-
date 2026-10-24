//! Comprobaciones estáticas de la interfaz principal.
//!
//! Estos tests no sustituyen la compilación ni la prueba en navegador, pero
//! detectan regresiones comunes en navegación, estructura semántica, diseño
//! adaptable y en el flujo de importación de notas.
const NAVEGACION: &str = include_str!("../static/stitch-v2/shell/aside.html");
const BARRA_SUPERIOR: &str = include_str!("../static/stitch-v2/shell/topbar.html");
const PORTADA: &str = include_str!("../static/stitch-v2/_content/clean/index.html");
const PANEL: &str = include_str!("../static/stitch-v2/_content/clean/dashboard.html");
const NOTAS: &str = include_str!("../static/stitch-v2/_content/clean/notes.html");
const ASISTENTE: &str = include_str!("../static/stitch-v2/_content/clean/asistente.html");
const GENERADOR: &str = include_str!("../static/stitch-v2/_content/clean/material.html");
const INTEGRACIONES: &str = include_str!("../static/stitch-v2/_content/clean/mcp.html");
const CONFIGURACION: &str = include_str!("../static/stitch-v2/_content/clean/settings.html");
const SHELL_JS: &str = include_str!("../static/app/shell.js");
const NOVA_CSS: &str = include_str!("../static/app/nova.css");

fn rutas_nav(html: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("data-path=\"") {
        let after = &rest[start + "data-path=\"".len()..];
        let Some(end) = after.find('"') else { break };
        out.push(&after[..end]);
        rest = &after[end + 1..];
    }
    out
}

#[test]
fn la_navegacion_conserva_los_22_destinos_unicos() {
    let rutas = rutas_nav(NAVEGACION);
    assert_eq!(rutas.len(), 22, "se deben conservar todos los apartados del menú");
    let mut unicas = rutas.clone();
    unicas.sort_unstable();
    unicas.dedup();
    assert_eq!(unicas.len(), rutas.len(), "no debe haber destinos duplicados en el menú");
}

#[test]
fn las_vistas_inyectadas_no_anidan_elementos_main() {
    for (nombre, html) in [
        ("portada", PORTADA),
        ("panel", PANEL),
        ("notas", NOTAS),
        ("asistente", ASISTENTE),
        ("generador", GENERADOR),
        ("integraciones", INTEGRACIONES),
        ("configuración", CONFIGURACION),
    ] {
        assert!(
            !html.contains("<main") && !html.contains("</main>"),
            "la vista {nombre} es un fragmento del main del shell y no debe anidar otra etiqueta main"
        );
    }
}

#[test]
fn la_captura_de_tareas_y_la_importacion_conservan_sus_controles() {
    for id in ["quick-task-modal", "quick-task-title", "quick-task-subject", "quick-task-priority", "quick-task-date", "quick-task-time", "save-task-as-note"] {
        assert!(PORTADA.contains(&format!("id=\"{id}\"")), "falta el control {id} en la portada");
        assert!(PANEL.contains(&format!("id=\"{id}\"")), "falta el control {id} en el panel");
    }
    for id in ["btn-import-vault", "import-vault-modal", "choose-vault-folder-btn", "confirm-import-btn"] {
        assert!(NOTAS.contains(&format!("id=\"{id}\"")), "falta el control de importación {id}");
    }
    assert!(NOTAS.contains("value=\"keep-both\""), "la política segura de conservar ambas versiones debe seguir disponible");
    assert!(NOTAS.contains("value=\"overwrite-newer\""), "la política de actualización controlada debe seguir disponible");
    assert!(SHELL_JS.contains("SP.importObsidianFiles = function"), "el importador real debe estar conectado al shell");
    assert!(!SHELL_JS.contains("setTimeout(siguiente, 120)"), "la importación no debe avanzar antes de que termine cada guardado");
}

#[test]
fn la_interfaz_principal_mantiene_la_identidad_en_espanol() {
    for texto in ["Tu espacio académico", "Panel principal", "Segundo cerebro"] {
        assert!(
            PORTADA.contains(texto) || NAVEGACION.contains(texto) || BARRA_SUPERIOR.contains(texto),
            "falta una etiqueta española esperada: {texto}"
        );
    }
    assert!(NAVEGACION.contains("Espacio de almacenamiento"));
}

#[test]
fn el_css_nova_mantiene_bloques_balanceados() {
    let abiertas = NOVA_CSS.chars().filter(|c| *c == '{').count();
    let cerradas = NOVA_CSS.chars().filter(|c| *c == '}').count();
    assert_eq!(abiertas, cerradas, "el CSS del diseño debe tener todas las llaves cerradas");
    assert!(NOVA_CSS.contains("#chat-messages-container"), "el chat necesita estilos específicos");
    assert!(NOVA_CSS.contains("@media (max-width: 767px)"), "la interfaz debe conservar reglas para móvil");
}
