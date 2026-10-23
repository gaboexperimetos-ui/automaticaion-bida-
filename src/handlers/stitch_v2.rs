//! Estudio platform UI v2 — interfaz "Synthetic Cognition" (kit stitch_study_platform_ui_system).
//!
//! Interfaz principal basada en el kit Stitch v2.4. Rutas:
//!   - GET / -> panel principal (dashboard)
//!   - GET /dashboard -> panel principal; `?partial=...` -> academic dashboard
//!   - GET /dashboard/ai -> asistente academico
//!   - GET /dashboard/notes -> segundo cerebro (notas markdown)
//!   - GET /dashboard/mcp -> centro MCP + configuracion OpenRouter

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::{Html, IntoResponse},
};
use chrono::{Datelike, Local, NaiveDate, Timelike, Weekday};

use crate::academic::templates::{html_escape, weekday_es};
use crate::handlers::schedule::mes_es;
use crate::handlers::stitch_panel::PanelData;
use crate::integrations::mikareno::{Horario, Tarea};
use crate::middleware::auth::extract_session_cookie;
use crate::AppState;

/// Mapeo de las paginas del nav-rail a sus rutas en la aplicacion.
///
/// Every target must be an existing route (verified by integration check);
/// menu items without a dedicated page point at the closest real screen.
///
/// Fuente única del mapa de navegación: `material.rs` y el nav de
/// `academic/templates/mod.rs` usan estos mismos destinos.
pub const NAV_LINKS: &[(&str, &str)] = &[
    ("panel-principal", "/dashboard"),
    ("mi-agenda", "/dashboard/calendar"),
    ("planificacion", "/dashboard/planner"),
    ("asignaturas", "/dashboard/subjects"),
    ("tareas-y-actividades", "/dashboard/planner"),
    ("examenes", "/dashboard/examenes"),
    ("vida-escolar", "/dashboard/vida-escolar"),
    ("horarios", "/dashboard/horario"),
    ("asistente-academico", "/dashboard/ai"),
    ("conversaciones", "/ai"),
    ("generador-de-material", "/dashboard/material"),
    ("mis-notas", "/dashboard/notes"),
    ("segundo-cerebro", "/dashboard/notes"),
    ("busqueda-global", "/dashboard/search"),
    ("documentos", "/office/docs"),
    ("hojas-de-calculo", "/office/sheets"),
    ("presentaciones", "/office/presentations"),
    ("biblioteca-de-archivos", "/office/library"),
    ("centro-mcp", "/dashboard/mcp"),
    ("estado-de-conexiones", "/dashboard/mcp"),
    ("administracion", "/admin"),
    ("configuracion", "/settings"),
];

const STITCH_V2_HEAD: &str = include_str!("../../static/stitch-v2/shell/head.html");
const STITCH_V2_ASIDE: &str = include_str!("../../static/stitch-v2/shell/aside.html");
const STITCH_V2_TOPBAR: &str = include_str!("../../static/stitch-v2/shell/topbar.html");
const STITCH_V2_MAIN_CLOSE: &str = "</main>";

// ── Shell responsive y accesibilidad ──────────────────────────────────────
//
// El rail lateral (`<aside>`) del kit es fijo de 250px: por debajo de
// 1024px tapaba el contenido y no había forma de apartarlo. Aquí el rail
// se comporta como cajón "off-canvas" en móvil y además se pliega en
// escritorio, con un botón hamburguesa en el topbar.
//
// El HTML del kit (`static/stitch-v2/shell/aside.html` y `topbar.html`) NO
// se modifica: todo se inyecta desde aquí, para no romper los 21
// `data-path` del nav ni los marcadores `<!--SLOT:SHELL_*-->` que rellena
// `stitch_panel::fill_shell`.

/// `id` del rail lateral (`<aside>`).
const RAIL_ID: &str = "app-rail";
/// `id` del envoltorio topbar + `<main>` (el que lleva el `pl-[250px]`).
const RAIL_CONTENT_ID: &str = "app-rail-content";
/// `id` del topbar (`<header>`).
const TOPBAR_ID: &str = "app-topbar";
/// `id` del botón hamburguesa del topbar.
const RAIL_TOGGLE_ID: &str = "rail-toggle";
/// `id` del botón "Colapsar menú" que ya traía el `<aside>` del kit.
const RAIL_TOGGLE_ASIDE_ID: &str = "rail-toggle-aside";
/// `id` del velo oscuro que cierra el cajón al tocar fuera.
const RAIL_BACKDROP_ID: &str = "rail-backdrop";
/// `id` del `<main>`: destino del skip-link.
const MAIN_CONTENT_ID: &str = "contenido";

/// Skip-link: primer elemento del `<body>`, visible sólo al recibir el
/// foco (teclado). Lleva al contenido principal, no al nav.
fn skip_link() -> String {
    format!(r##"<a class="skip-link" href="#{MAIN_CONTENT_ID}">Saltar al contenido</a>"##)
}

/// Botón hamburguesa del topbar. `aria-controls` apunta al rail y
/// `aria-expanded` lo sincroniza el script con el estado real.
fn rail_toggle_button() -> String {
    format!(
        r##"<button id="{RAIL_TOGGLE_ID}" type="button" aria-controls="{RAIL_ID}" aria-expanded="true" aria-label="Mostrar u ocultar el menú lateral" title="Mostrar u ocultar el menú lateral" class="p-2 -ml-1 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors"><span class="material-symbols-outlined text-[20px]">menu</span></button>"##
    )
}

/// Velo oscuro tras el cajón (sólo móvil). Es un `<button>` para que sea
/// alcanzable con el teclado y para que el lector de pantalla lo anuncie.
fn rail_backdrop() -> String {
    format!(
        r##"<button id="{RAIL_BACKDROP_ID}" type="button" aria-label="Cerrar el menú de navegación" title="Cerrar el menú de navegación" class="fixed inset-0 bg-black/60 z-30"></button>"##
    )
}

/// Envoltorio del topbar + `<main>`. El `id` permite al CSS del shell
/// liberar los 250px del rail cuando éste se pliega.
fn rail_content_wrapper() -> String {
    format!(r##"<div id="{RAIL_CONTENT_ID}" class="pl-[250px]">"##)
}

/// `id` del botón que abre el menú de usuario del topbar.
const USER_MENU_BUTTON_ID: &str = "topProfile";

/// `id` del panel desplegable del menú de usuario.
const USER_MENU_ID: &str = "user-menu";

/// Botón que abre el menú de usuario.
///
/// Envuelve el avatar, el nombre y el chevron: el objetivo es toda la
/// identidad, no sólo un icono de 16 px. `aria-haspopup` + `aria-expanded`
/// + `aria-controls` dejan el estado del desplegable anunciado.
fn user_menu_button() -> String {
    format!(
        r##"<button id="{USER_MENU_BUTTON_ID}" type="button" aria-haspopup="menu" aria-expanded="false" aria-controls="{USER_MENU_ID}" aria-label="Abrir el menú de usuario" title="Menú de usuario" class="flex items-center gap-space-xs pl-space-xs border-l border-outline-variant/40 rounded-lg py-1 pr-1.5 hover:bg-surface-container transition-colors"><span class="relative shrink-0"><!--SLOT:SHELL_AVATAR--><span class="absolute bottom-0 right-0 w-2.5 h-2.5 rounded-full bg-secondary ring-2 ring-surface-container-low"></span></span><span class="flex flex-col text-left"><span class="font-label-sm text-label-sm text-on-surface font-semibold leading-tight"><!--SLOT:SHELL_NAME--></span><span class="font-body-sm text-[10px] text-outline leading-tight"><!--SLOT:SHELL_ROLE--></span></span><span class="material-symbols-outlined text-[16px] text-outline" aria-hidden="true">expand_more</span></button>"##
    )
}

/// Panel desplegable del menú de usuario.
///
/// La cabecera repite el avatar y los datos del perfil (los mismos
/// `SLOT` que rellena `stitch_panel::fill_shell`, que sustituye todas
/// sus apariciones), de modo que las iniciales son las reales del perfil
/// y no un texto fijo.
///
/// La última acción depende de la sesión: con sesión abierta, "Cerrar
/// sesión" (formulario `POST /logout`, funciona sin JavaScript); sin
/// sesión, "Iniciar sesión" hacia `/login`.
fn user_menu_panel(authenticated: bool) -> String {
    const ITEM: &str = "flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm font-label-md text-on-surface-variant transition-colors hover:bg-surface-container-high hover:text-on-surface";
    let action = if authenticated {
        format!(
            r##"<form method="post" action="/logout" role="none"><button role="menuitem" type="submit" class="{ITEM}"><span class="material-symbols-outlined text-[18px]" aria-hidden="true">logout</span>Cerrar sesión</button></form>"##,
            ITEM = ITEM
        )
    } else {
        format!(
            r##"<a role="menuitem" href="/login" class="{ITEM}"><span class="material-symbols-outlined text-[18px]" aria-hidden="true">login</span>Iniciar sesión</a>"##,
            ITEM = ITEM
        )
    };
    format!(
        r##"<div id="{USER_MENU_ID}" hidden class="absolute right-0 top-full mt-2 w-72 z-50 rounded-xl border border-outline-variant/40 bg-surface-container shadow-2xl p-1.5"><div class="flex items-center gap-3 px-2.5 py-2 border-b border-outline-variant/30"><span class="relative shrink-0"><!--SLOT:SHELL_AVATAR--><span class="absolute bottom-0 right-0 w-2.5 h-2.5 rounded-full bg-secondary ring-2 ring-surface-container"></span></span><span class="flex min-w-0 flex-col text-left"><span class="truncate font-label-sm text-label-sm text-on-surface font-semibold leading-tight"><!--SLOT:SHELL_NAME--></span><span class="truncate font-body-sm text-[10px] text-outline leading-tight"><!--SLOT:SHELL_ROLE--></span></span></div><div role="menu" aria-label="Menú de usuario" class="pt-1"><a role="menuitem" href="/settings" class="{ITEM}"><span class="material-symbols-outlined text-[18px]" aria-hidden="true">settings</span>Configuración</a><a role="menuitem" href="/settings/password" class="{ITEM}"><span class="material-symbols-outlined text-[18px]" aria-hidden="true">key</span>Cambiar contraseña</a>{action}</div></div>"##,
        ITEM = ITEM,
        action = action
    )
}

/// Sustituye el bloque de identidad del topbar por el botón + panel del
/// menú de usuario.
///
/// El kit trae el avatar, el nombre y un chevron **decorativo** (sin
/// botón ni panel): no había forma de abrir un menú de usuario. Aquí ese
/// bloque se convierte en un `<button>` (avatar + nombre + chevron) y se
/// le añade el panel desplegable.
///
/// Si la estructura del kit cambia, no se inyecta nada: es preferible un
/// topbar sin menú a un menú colgado del sitio equivocado.
fn inject_user_menu(topbar: &str, authenticated: bool) -> String {
    // Ancla del bloque de identidad: el borde izquierdo de la tarjeta.
    const ANCHOR: &str = "border-l border-outline-variant/40";
    // El chevron es el último hijo del bloque: marca dónde acaba.
    const CHEVRON: &str = "expand_more";

    let Some(anchor_pos) = topbar.find(ANCHOR) else {
        return topbar.to_string();
    };
    let Some(open) = topbar[..anchor_pos].rfind("<div") else {
        return topbar.to_string();
    };
    let Some(chevron) = topbar[open..].find(CHEVRON) else {
        return topbar.to_string();
    };
    let chevron = open + chevron;
    // Cierre del `<span>` del chevron.
    let Some(close_span) = topbar[chevron..].find("</span>") else {
        return topbar.to_string();
    };
    let after_span = chevron + close_span + "</span>".len();
    // Cierre del `<div>` del bloque: el primero tras el chevron.
    let Some(close_div) = topbar[after_span..].find("</div>") else {
        return topbar.to_string();
    };
    let block_end = after_span + close_div + "</div>".len();

    let mut out = String::with_capacity(topbar.len() + 512);
    out.push_str(&topbar[..open]);
    // Contenedor posicionado: el panel se ancla a la esquina inferior
    // derecha de la identidad.
    out.push_str(r##"<div class="relative">"##);
    out.push_str(&user_menu_button());
    out.push_str(&user_menu_panel(authenticated));
    out.push_str("</div>");
    out.push_str(&topbar[block_end..]);
    out
}

/// Cierre del envoltorio `pl-[250px]` (si no, el HTML queda desbalanceado:
/// un `<div>` abierto y nunca cerrado).
fn pl_close() -> String {
    "</div>".to_string()
}

/// `<main>`: destino del skip-link ("Saltar al contenido"). El
/// `tabindex="-1"` hace que el foco viaje a él al activarlo.
fn main_open() -> String {
    format!(
        r##"<main id="{MAIN_CONTENT_ID}" tabindex="-1" class="relative pt-16 w-full min-h-screen bg-background">"##
    )
}

/// CSS del shell: rail plegable, skip-link y foco visible.
///
/// Los `id` de este CSS deben coincidir con `RAIL_ID`, `RAIL_CONTENT_ID`,
/// `TOPBAR_ID` y `RAIL_BACKDROP_ID`.
///
/// El kit (`static/stitch-v2/shell/head.html`) no elimina los
/// `:focus-visible`; esta capa los **refuerza** con un anillo visible
/// (WCAG 2.4.7) en vez de dejarlos depender de los tokens del kit.
const SHELL_CSS: &str = r##"
/* --- Shell v2: rail lateral plegable + skip-link + foco visible --- */
.skip-link{position:absolute;left:0;top:0;z-index:100;transform:translateY(-160%);
  padding:.5rem .875rem;border-radius:0 0 .5rem 0;background:#00675b;color:#e0e2ea;
  font-family:'Hanken Grotesk',sans-serif;font-size:.75rem;font-weight:600;line-height:1rem;
  text-decoration:none;transition:transform .15s ease}
.skip-link:focus{transform:translateY(0)}
#app-rail{transition:transform .22s ease,visibility .22s ease}
#rail-backdrop{display:none;position:fixed;inset:0;background:rgba(0,0,0,.55);z-index:30}
@media (max-width:1023.98px){
  #app-rail{transform:translateX(-100%);visibility:hidden}
  body.rail-open #app-rail{transform:translateX(0);visibility:visible}
  body.rail-open #rail-backdrop{display:block}
  #app-rail-content{padding-left:0!important}
  #app-topbar{left:0!important}
}
body.rail-collapsed #app-rail{transform:translateX(-100%);visibility:hidden}
body.rail-collapsed #app-rail-content{padding-left:0!important}
body.rail-collapsed #app-topbar{left:0!important}
a:focus-visible,button:focus-visible,input:focus-visible,select:focus-visible,textarea:focus-visible,summary:focus-visible,[tabindex]:focus-visible{
  outline:2px solid #5eead4;outline-offset:2px;border-radius:4px}
#app-topbar input:focus-visible{outline:2px solid #5eead4;outline-offset:2px}
#contenido:focus-visible{outline:2px solid #5eead4;outline-offset:-6px}
/* Menú de usuario: el atributo `hidden` manda sobre cualquier utilidad
   de display que el CDN de Tailwind pueda aplicar. */
#user-menu[hidden]{display:none!important}
@media (prefers-reduced-motion:reduce){#app-rail,.skip-link{transition:none}}
"##;

/// Script mínimo del shell: alterna la clase del rail y mantiene sincronizado
/// el `aria-expanded` de los dos botones que lo controlan.
///
/// Un único estado por ancho de pantalla: en escritorio se usa
/// `rail-collapsed` (el rail se pliega dejando el hueco libre) y en móvil
/// `rail-open` (el rail sale como cajón sobre el contenido). Ambas clases
/// son excluyentes, así que un cambio de ancho no deja el shell en un
/// estado imposible.
const SHELL_JS: &str = r##"
(function(){
  var body=document.body;
  var rail=document.getElementById('app-rail');
  var toggle=document.getElementById('rail-toggle');
  var asideToggle=document.getElementById('rail-toggle-aside');
  var backdrop=document.getElementById('rail-backdrop');
  if(!rail||!toggle){return;}
  var wide=function(){return window.matchMedia('(min-width:1024px)').matches;};
  var isOpen=function(){
    return wide()?!body.classList.contains('rail-collapsed'):body.classList.contains('rail-open');
  };
  var sync=function(){
    if(wide()){body.classList.remove('rail-open');}
    else{body.classList.remove('rail-collapsed');}
    var open=isOpen();
    var label=open?'Ocultar el menú lateral':'Mostrar el menú lateral';
    var buttons=[toggle,asideToggle];
    for(var i=0;i<buttons.length;i++){
      var b=buttons[i];
      if(!b){continue;}
      b.setAttribute('aria-expanded',open?'true':'false');
      b.setAttribute('aria-label',label);
    }
  };
  var flip=function(){
    if(wide()){
      body.classList.toggle('rail-collapsed');
      sync();
      return;
    }
    var opening=!body.classList.contains('rail-open');
    body.classList.toggle('rail-open');
    sync();
    if(opening){
      var first=rail.querySelector('nav a');
      if(first){first.focus();}
    }else{
      toggle.focus();
    }
  };
  toggle.addEventListener('click',flip);
  if(asideToggle){asideToggle.addEventListener('click',flip);}
  if(backdrop){
    backdrop.addEventListener('click',function(){
      body.classList.remove('rail-open');
      sync();
      toggle.focus();
    });
  }
  document.addEventListener('keydown',function(e){
    if(e.key==='Escape'&&body.classList.contains('rail-open')){
      body.classList.remove('rail-open');
      sync();
      toggle.focus();
    }
  });
  window.addEventListener('resize',sync);
  sync();
})();
"##;

/// Contenido de cada pagina (contenido del <main> sin wrappers).
const CONTENT_INDEX: &str = include_str!("../../static/stitch-v2/_content/clean/index.html");
const CONTENT_DASHBOARD: &str =
    include_str!("../../static/stitch-v2/_content/clean/dashboard.html");
const CONTENT_ASISTENTE: &str =
    include_str!("../../static/stitch-v2/_content/clean/asistente.html");
const CONTENT_NOTES: &str = include_str!("../../static/stitch-v2/_content/clean/notes.html");
const CONTENT_MCP: &str = include_str!("../../static/stitch-v2/_content/clean/mcp.html");

/// Script del menú de usuario: abre y cierra el desplegable, lo cierra con
/// Escape o al pulsar fuera, y mueve el foco entre los items con las
/// flechas (patrón WAI-ARIA de menú).
///
/// No intercepta la navegación: los items son enlaces y el cierre de
/// sesión es un formulario `POST /logout`, así que todo funciona también
/// sin JavaScript.
const USER_MENU_JS: &str = r##"
(function(){
  var btn=document.getElementById('topProfile');
  var panel=document.getElementById('user-menu');
  if(!btn||!panel){return;}
  var open=false;
  var items=function(){
    return Array.prototype.slice.call(panel.querySelectorAll('[role="menuitem"]'));
  };
  var focusedIndex=function(){
    var list=items();
    for(var i=0;i<list.length;i++){
      if(list[i]===document.activeElement){return i;}
    }
    return -1;
  };
  var focusAt=function(i){
    var list=items();
    if(!list.length){return;}
    list[((i%list.length)+list.length)%list.length].focus();
  };
  var setOpen=function(next){
    if(open&&!next&&panel.contains(document.activeElement)){btn.focus();}
    open=next;
    panel.hidden=!open;
    btn.setAttribute('aria-expanded',open?'true':'false');
    btn.setAttribute('aria-label',open?'Cerrar el menú de usuario':'Abrir el menú de usuario');
  };
  btn.addEventListener('click',function(e){
    e.stopPropagation();
    setOpen(!open);
    if(open){focusAt(0);}
  });
  document.addEventListener('click',function(e){
    if(open&&!panel.contains(e.target)&&!btn.contains(e.target)){setOpen(false);}
  });
  document.addEventListener('keydown',function(e){
    if(!open){return;}
    if(e.key==='Escape'){setOpen(false);}
    else if(e.key==='ArrowDown'){e.preventDefault();focusAt(focusedIndex()+1);}
    else if(e.key==='ArrowUp'){e.preventDefault();focusAt(focusedIndex()-1);}
  });
})();
"##;

/// Script que inyecta el estado de autenticacion para el frontend.
///
/// Antes este script cerraba la sesión al pulsar `#topProfile`; ahora ese
/// `id` es el botón del menú de usuario, así que el cierre de sesión vive
/// en el propio menú (formulario `POST /logout`) y aquí sólo se mantiene
/// el flag para el resto del frontend.
const STITCH_V2_AUTH_SCRIPT: &str = "<script>\n(function(){\n  var AUTH = window.__STITCH_V2_AUTH__ || {authenticated:false,loginUrl:'/login',logoutUrl:'/logout'};\n  window.__STITCH_V2_AUTH__ = AUTH;\n})();\n</script>";

/// Clases de estado del item activo del nav-rail.
const ACTIVE_ITEM_CLASSES: &str = "bg-primary-container text-on-primary-container font-semibold";

/// Clases de estado de un item inactivo del nav-rail.
const IDLE_ITEM_CLASSES: &str =
    "text-on-surface-variant hover:bg-surface-container-high hover:text-on-surface";

/// Tokens de "estado" que se sustituyen al activar un item. Se comparan
/// como tokens exactos para no tocar las clases estructurales del item
/// (`flex`, `justify-between`, `gap-space-xs`...).
const STATE_CLASS_TOKENS: &[&str] = &[
    "bg-primary-container",
    "text-on-primary-container",
    "font-semibold",
    "text-on-surface-variant",
    "hover:bg-surface-container-high",
    "hover:bg-surface-container",
    "hover:text-on-surface",
    "hover:bg-primary-fixed-dim",
];

/// Compone el `class` de un item del nav-rail conservando su estructura
/// (posiciones, separaciones) y aplicando solo el estado activo/inactivo.
fn item_class(existing: &str, active: bool) -> String {
    let mut tokens: Vec<&str> = existing
        .split_whitespace()
        .filter(|t| !STATE_CLASS_TOKENS.contains(t))
        .collect();
    let state = if active {
        ACTIVE_ITEM_CLASSES
    } else {
        IDLE_ITEM_CLASSES
    };
    tokens.extend(state.split_whitespace());
    tokens.join(" ")
}

/// Valor de un atributo (`name="value"`) dentro de una etiqueta.
fn attr_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {}=\"", name);
    let start = tag.find(&needle)? + needle.len();
    let end = start + tag[start..].find('"')?;
    Some(&tag[start..end])
}

/// Reemplaza el valor de un atributo presente en la etiqueta.
fn replace_attr(tag: &str, name: &str, value: &str) -> String {
    let needle = format!(" {}=\"", name);
    let Some(pos) = tag.find(&needle) else {
        return tag.to_string();
    };
    let start = pos + needle.len();
    let Some(len) = tag[start..].find('"') else {
        return tag.to_string();
    };
    let end = start + len;
    let mut out = String::with_capacity(tag.len() + value.len());
    out.push_str(&tag[..start]);
    out.push_str(value);
    out.push_str(&tag[end..]);
    out
}

/// Elimina un atributo (con su espacio previo) de la etiqueta.
fn remove_attr(tag: &str, name: &str) -> String {
    let needle = format!(" {}=\"", name);
    let Some(pos) = tag.find(&needle) else {
        return tag.to_string();
    };
    let start = pos + needle.len(); // tras la comilla de apertura
    let Some(len) = tag[start..].find('"') else {
        return tag.to_string();
    };
    let end = start + len + 1; // incluye la comilla de cierre
    let mut out = String::with_capacity(tag.len());
    out.push_str(&tag[..pos]);
    out.push_str(&tag[end..]);
    out
}

/// Inserta un atributo antes del cierre de la etiqueta.
fn insert_attr(tag: &str, name: &str, value: &str) -> String {
    let close = tag.rfind('>').unwrap_or(tag.len());
    let mut out = String::with_capacity(tag.len() + name.len() + value.len() + 4);
    out.push_str(&tag[..close]);
    out.push_str(&format!(" {}=\"{}\"", name, value));
    out.push_str(&tag[close..]);
    out
}

/// Añade un `id` a la primera etiqueta `<tag ...>` del fragmento.
///
/// Si la etiqueta ya trae `id`, no se toca: nunca se duplica un
/// identificador en el mismo documento.
fn add_id_to_first_tag(html: &str, tag: &str, id: &str) -> String {
    let needle = format!("<{}", tag);
    let Some(pos) = html.find(&needle) else {
        return html.to_string();
    };
    let after = &html[pos..];
    // `<aside` no es `<asideXYZ>`: el siguiente carácter debe separar.
    let Some(next) = after[needle.len()..].chars().next() else {
        return html.to_string();
    };
    if !matches!(next, ' ' | '\t' | '\n' | '>') {
        return html.to_string();
    }
    let Some(tag_end) = after.find('>') else {
        return html.to_string();
    };
    if attr_value(&after[..tag_end], "id").is_some() {
        return html.to_string();
    }
    let insert_at = pos + needle.len();
    let mut out = String::with_capacity(html.len() + id.len() + 6);
    out.push_str(&html[..insert_at]);
    out.push_str(&format!(" id=\"{}\"", id));
    out.push_str(&html[insert_at..]);
    out
}

/// Añade atributos a la etiqueta `<tag ...>` que contiene el texto `anchor`.
///
/// Se usa para los botones del kit (que no se editan) a los que hay que
/// añadir `aria-label`, `aria-expanded`, `type` o `id`.
fn add_attrs_to_tag_containing(html: &str, tag: &str, anchor: &str, attrs: &str) -> String {
    let Some(anchor_pos) = html.find(anchor) else {
        return html.to_string();
    };
    let Some(open) = html[..anchor_pos].rfind(&format!("<{}", tag)) else {
        return html.to_string();
    };
    let after = &html[open..];
    let Some(tag_end) = after.find('>') else {
        return html.to_string();
    };
    // Las etiquetas autocerradas (`<input ... />`) admiten atributos antes
    // de la barra: insertar después produciría HTML inválido.
    let mut insert_at = open + tag_end;
    if html[..insert_at].ends_with('/') {
        insert_at -= 1;
    }
    let mut out = String::with_capacity(html.len() + attrs.len());
    out.push_str(&html[..insert_at]);
    out.push_str(attrs);
    out.push_str(&html[insert_at..]);
    out
}

/// Marca los iconos de Material Symbols como decorativos (`aria-hidden`).
///
/// Sin esto, un lector de pantalla anuncia el nombre de la ligadura
/// ("dashboard", "notifications") en lugar del texto real del enlace o del
/// botón. Los iconos de los items del nav siempre van acompañados de su
/// texto, así que ocultarlos no pierde información.
fn hide_material_icons(html: &str) -> String {
    const ICON: &str = "<span class=\"material-symbols-outlined";
    let mut out = String::with_capacity(html.len() + 256);
    let mut rest = html;
    while let Some(start) = rest.find(ICON) {
        let after = &rest[start..];
        let Some(tag_end) = after.find('>') else {
            out.push_str(after);
            return out;
        };
        out.push_str(&rest[..start]);
        let tag = &after[..tag_end];
        if attr_value(tag, "aria-hidden").is_some() {
            out.push_str(tag);
        } else {
            out.push_str(&insert_attr(tag, "aria-hidden", "true"));
        }
        out.push('>');
        rest = &after[tag_end + 1..];
    }
    out.push_str(rest);
    out
}

/// Texto visible de un item del nav, sin el icono Material Symbols.
///
/// Se usa como nombre accesible del enlace: aunque el rail se muestre
/// plegado (o el icono se oculte por CSS), el enlace sigue teniendo
/// nombre para el lector de pantalla.
fn visible_label(inner: &str) -> String {
    const ICON: &str = "<span class=\"material-symbols-outlined";

    // 1) Quita los iconos (con su contenido: el nombre de la ligadura).
    let mut limpio = String::with_capacity(inner.len());
    let mut rest = inner;
    while let Some(start) = rest.find(ICON) {
        limpio.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(tag_end) = after.find('>') else {
            limpio.push_str(after);
            rest = "";
            break;
        };
        match after[tag_end..].find("</span>") {
            Some(close) => rest = &after[tag_end + close + "</span>".len()..],
            None => {
                limpio.push_str(&after[tag_end + 1..]);
                rest = "";
                break;
            }
        }
    }
    limpio.push_str(rest);

    // 2) Quita el resto de etiquetas y decodifica las entidades básicas.
    let mut texto = String::with_capacity(limpio.len());
    let mut en_etiqueta = false;
    for ch in limpio.chars() {
        match ch {
            '<' => en_etiqueta = true,
            '>' => en_etiqueta = false,
            _ if !en_etiqueta => texto.push(ch),
            _ => {}
        }
    }
    let texto = texto
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    texto.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Inserta el botón hamburguesa como primer hijo del grupo izquierdo del
/// topbar (el que hoy lleva la migas de pan y el buscador).
///
/// Si la estructura del topbar cambia, no se inyecta nada: es preferible
/// un shell sin botón a un botón colgado del sitio equivocado.
fn inject_rail_toggle(topbar: &str) -> String {
    let Some(header_end) = topbar.find('>') else {
        return topbar.to_string();
    };
    let after = &topbar[header_end + 1..];
    let Some(div_end) = after.find('>') else {
        return topbar.to_string();
    };
    if !after[..div_end].trim_start().starts_with("<div") {
        return topbar.to_string();
    }
    let insert_at = header_end + 1 + div_end + 1;
    let toggle = rail_toggle_button();
    let mut out = String::with_capacity(topbar.len() + toggle.len());
    out.push_str(&topbar[..insert_at]);
    out.push_str(&toggle);
    out.push_str(&topbar[insert_at..]);
    out
}

/// Reescribe un item del nav-rail (`<a ...>...</a>`).
///
/// Aplica la ruta real (`href`), el estado visual (`class`) y `aria-current`
/// **en un solo atributo cada uno**: parchear la etiqueta con `replace`
/// duplicaba `class` y `aria-current`, y el navegador descarta el
/// duplicado, dejando el item activo sin resaltar y con dos items
/// marcados como página actual.
fn rewrite_nav_item(item: &str, active: &str) -> String {
    let Some(tag_end) = item.find('>') else {
        return item.to_string();
    };
    let (tag, inner) = item.split_at(tag_end + 1);

    let Some(path) = attr_value(tag, "data-path").map(|p| p.to_string()) else {
        return item.to_string();
    };
    let is_active = path == active;

    let mut out = String::with_capacity(item.len());
    let new_tag = rewrite_nav_tag(tag, &path, is_active);

    // Nombre accesible del enlace: su texto visible. Garantiza que el item
    // tenga nombre aunque el rail se muestre plegado y el texto no se vea.
    let new_tag = match visible_label(inner) {
        label if label.is_empty() => new_tag,
        label => {
            let tag = remove_attr(&new_tag, "aria-label");
            insert_attr(&tag, "aria-label", &html_escape(&label))
        }
    };

    let tag_len = new_tag.len();
    out.push_str(&new_tag);
    out.push_str(inner);

    // Contraste del item activo: fondo `bg-primary-container` (teal claro).
    // El icono `text-primary-container` y el punto decorativo quedan
    // invisibles sobre ese fondo, asi que se pasan al color de texto del
    // estado activo. Solo se toca el contenido del item, nunca su `class`.
    if is_active {
        let inner = out[tag_len..]
            .replacen("text-primary-container", "text-on-primary-container", 1)
            .replacen(
                "w-2 h-2 rounded-full bg-primary-container animate-pulse",
                "w-2 h-2 rounded-full bg-on-primary-container animate-pulse",
                1,
            );
        out.truncate(tag_len);
        out.push_str(&inner);
    }

    out
}

/// Reescribe la etiqueta de apertura `<a ...>` de un item del nav-rail.
fn rewrite_nav_tag(tag: &str, path: &str, is_active: bool) -> String {
    let mut out = tag.to_string();

    // Ruta real del item (si el item tiene destino propio).
    if let Some((_, route)) = NAV_LINKS.iter().find(|(p, _)| *p == path) {
        out = replace_attr(&out, "href", route);
    }

    // Estado visual: se reescribe el `class` existente, no se añade otro.
    // Se conservan las clases estructurales (`flex`, `justify-between`...)
    // para no romper la alineacion de los items con badge.
    if let Some(cls) = attr_value(&out, "class").map(|c| c.to_string()) {
        out = replace_attr(&out, "class", &item_class(&cls, is_active));
    }

    // `aria-current` solo en el item de la ruta actual.
    out = remove_attr(&out, "aria-current");
    if is_active {
        out = insert_attr(&out, "aria-current", "page");
    }

    out
}

/// Actualiza el nav-rail: asigna rutas a los enlaces y marca el item activo.
///
/// El item activo se resuelve por `data-path` exacto de la ruta actual
/// (`active`), de modo que solo un item queda resaltado y con
/// `aria-current="page"`. Ningún badge ni contador se inventa: los
/// contadores del shell (MCP integrado, % de vault) los rellena
/// `fill_shell` con datos reales.
fn update_nav_rail(aside: &str, active: &str) -> String {
    let mut result = String::with_capacity(aside.len());
    let mut rest = aside;

    while let Some(start) = rest.find("<a ") {
        result.push_str(&rest[..start]);
        let after = &rest[start..];
        let (Some(tag_end), Some(item_end)) = (after.find('>'), after.find("</a>")) else {
            // Etiqueta sin cierre: se copia tal cual y se sale.
            result.push_str(after);
            return result;
        };
        if item_end < tag_end {
            result.push_str(after);
            return result;
        }
        result.push_str(&rewrite_nav_item(&after[..item_end + 4], active));
        rest = &after[item_end + 4..];
    }
    result.push_str(rest);

    result
}

/// Item principal de una ruta: el `data-path` que debe quedar resaltado.
///
/// Varias entradas del menú comparten destino ("Planificación" y "Tareas" van al
/// planificador; "Mis notas" y "Segundo cerebro" van a las notas). El primero
/// del mapa que apunta a la ruta es el principal; los demás son alias.
fn primary_path_for(route: &str) -> Option<&'static str> {
    NAV_LINKS.iter().find(|(_, r)| *r == route).map(|(p, _)| *p)
}

/// Reescribe rutas **y** marca el item activo del nav.
///
/// `active_route` es la ruta de la página actual (p. ej. `/dashboard/notes`).
/// El item se localiza por `data-path`, no por `href`: con destinos
/// compartidos, buscar el `href` resaltaría varios items a la vez.
///
/// Los tokens y las clases de estado son parametrizables porque el shell
/// estático y el shell académico no usan el mismo diseño.
pub fn apply_nav(html: &str, active_route: &str) -> String {
    apply_nav_with(
        html,
        active_route,
        STATE_CLASS_TOKENS,
        ACTIVE_ITEM_CLASSES,
        IDLE_ITEM_CLASSES,
    )
}

/// Tokens de estado del shell académico (los del shell estático no encajan).
pub const ACADEMIC_STATE_TOKENS: &[&str] = &[
    "bg-primary-container",
    "text-on-primary-container",
    "font-semibold",
    "text-on-surface-variant",
    "hover:text-on-surface",
    "hover:bg-surface-container",
    "hover:bg-primary-fixed-dim",
];

/// Clase del item activo en el shell académico.
pub const ACADEMIC_ACTIVE_CLASSES: &str = "text-on-primary-container bg-primary-container \
     font-label-md font-semibold hover:bg-primary-fixed-dim";

/// Clase del item inactivo en el shell académico.
pub const ACADEMIC_IDLE_CLASSES: &str =
    "text-on-surface-variant hover:text-on-surface hover:bg-surface-container";

/// Variante parametrizable de [`apply_nav`].
pub fn apply_nav_with(
    html: &str,
    active_route: &str,
    tokens: &[&str],
    active_classes: &str,
    idle_classes: &str,
) -> String {
    let active_path = primary_path_for(active_route);
    let mut result = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(start) = rest.find("<a ") {
        result.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(tag_end) = after.find('>') else {
            result.push_str(after);
            return result;
        };
        let tag = &after[..tag_end + 1];
        let path = attr_value(tag, "data-path").map(|p| p.to_string());

        let mut out = match path.as_deref().and_then(|p| {
            NAV_LINKS
                .iter()
                .find(|(k, _)| *k == p)
                .map(|(_, route)| *route)
        }) {
            Some(route) => replace_attr(tag, "href", route),
            None => tag.to_string(),
        };

        if let Some(p) = path.as_deref() {
            let is_active = active_path == Some(p);
            if let Some(cls) = attr_value(&out, "class").map(|c| c.to_string()) {
                let state = if is_active {
                    active_classes
                } else {
                    idle_classes
                };
                out = replace_attr(&out, "class", &swap_state_tokens(&cls, tokens, state));
            }
            out = remove_attr(&out, "aria-current");
            if is_active {
                out = insert_attr(&out, "aria-current", "page");
            }
        }

        result.push_str(&out);
        rest = &after[tag_end + 1..];
    }
    result.push_str(rest);
    result
}

/// Sustituye los tokens de estado de una clase por los del estado pedido.
///
/// Conserva las clases estructurales (`flex`, `justify-between`, `gap-2`…):
/// sólo se quitan los tokens de estado y se anexa el estado nuevo.
fn swap_state_tokens(existing: &str, tokens: &[&str], state: &str) -> String {
    let mut kept: Vec<&str> = existing
        .split_whitespace()
        .filter(|t| !tokens.contains(t))
        .collect();
    kept.extend(state.split_whitespace());
    kept.join(" ")
}

/// Construye la pagina completa: head + shell + contenido + script de auth.
pub fn stitch_v2_page(active: &str, content: &str, authenticated: bool) -> String {
    // Real <head> (fonts, Tailwind CDN, design tokens) plus the early auth
    // flag; omitting STITCH_V2_HEAD here rendered every page unstyled.
    // El CSS del shell (rail plegable, skip-link, foco visible) va aquí:
    // sin él, `:focus-visible` se queda sin anillo visible.
    let head_with_auth = format!(
        "<head>{head}<style>{css}</style><script>window.__STITCH_V2_AUTH__={{\"authenticated\":{authenticated}}}</script></head>",
        head = STITCH_V2_HEAD,
        css = SHELL_CSS,
        authenticated = authenticated,
    );

    // Rail: rutas reales, item activo, nombre accesible e iconos
    // decorativos. El `id` y los atributos del botón de "Colapsar menú"
    // (que el kit trae sin `aria-label` y sin función) se añaden aquí.
    let mut aside = update_nav_rail(STITCH_V2_ASIDE, active);
    aside = hide_material_icons(&aside);
    aside = add_id_to_first_tag(&aside, "aside", RAIL_ID);
    aside = add_attrs_to_tag_containing(
        &aside,
        "button",
        "title=\"Colapsar menú\"",
        &format!(
            " id=\"{}\" type=\"button\" aria-controls=\"{}\" aria-expanded=\"true\" \
             aria-label=\"Colapsar o expandir el menú lateral\"",
            RAIL_TOGGLE_ASIDE_ID, RAIL_ID
        ),
    );

    // Topbar: `id` para el CSS, botón hamburguesa y accesibilidad de los
    // controles (el de notificaciones es sólo un icono: necesita nombre).
    let mut topbar = hide_material_icons(STITCH_V2_TOPBAR);
    topbar = add_id_to_first_tag(&topbar, "header", TOPBAR_ID);
    topbar = add_attrs_to_tag_containing(
        &topbar,
        "button",
        "title=\"Notificaciones\"",
        " aria-label=\"Notificaciones\"",
    );
    topbar = add_attrs_to_tag_containing(
        &topbar,
        "input",
        "placeholder=",
        " aria-label=\"Buscar notas, tareas y temas\"",
    );
    topbar = inject_rail_toggle(&topbar);
    // Menú de usuario: el bloque de identidad del kit pasa a ser un
    // botón que abre un desplegable con las acciones de la cuenta.
    topbar = inject_user_menu(&topbar, authenticated);

    let body_classes = "bg-background font-body-md text-body-md text-on-surface antialiased";

    format!(
        r###"<!DOCTYPE html>
<html class="dark" lang="es">{head}
<body class="{body_classes}">{skip_link}{aside}{backdrop}{pl_wrapper}{topbar}{main_open}{content}{main_close}{pl_close}
{auth_script}
<script>
{shell_js}
{user_menu_js}
</script>
<script src="/shell.js" defer></script>
</body>
</html>"###,
        head = head_with_auth,
        body_classes = body_classes,
        skip_link = skip_link(),
        aside = aside,
        backdrop = rail_backdrop(),
        pl_wrapper = rail_content_wrapper(),
        topbar = topbar,
        main_open = main_open(),
        content = content,
        main_close = STITCH_V2_MAIN_CLOSE,
        pl_close = pl_close(),
        auth_script = STITCH_V2_AUTH_SCRIPT,
        shell_js = SHELL_JS,
        user_menu_js = USER_MENU_JS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Aperturas de una etiqueta (`<div` seguido de delimitador).
    fn count_open(html: &str, tag: &str) -> usize {
        let needle = format!("<{}", tag);
        let mut count = 0usize;
        let mut idx = 0usize;
        while let Some(pos) = html[idx..].find(&needle) {
            let at = idx + pos + needle.len();
            if matches!(
                html[at..].chars().next(),
                Some(' ') | Some('\t') | Some('\n') | Some('\r') | Some('>') | Some('/')
            ) {
                count += 1;
            }
            idx = at;
        }
        count
    }

    /// Cierres de una etiqueta (`</div>`).
    fn count_close(html: &str, tag: &str) -> usize {
        html.matches(&format!("</{}>", tag)).count()
    }

    /// Items del nav-rail: `data-path` de cada `<a ...>`.
    fn data_paths(html: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = html;
        while let Some(start) = rest.find("<a ") {
            let after = &rest[start..];
            let Some(end) = after.find('>') else { break };
            if let Some(path) = attr_value(&after[..end], "data-path") {
                out.push(path.to_string());
            }
            rest = &after[end..];
        }
        out
    }

    #[test]
    fn el_nav_rail_conserva_los_22_data_path() {
        let html = stitch_v2_page("panel-principal", "<p>x</p>", false);
        let paths = data_paths(&html);
        assert_eq!(paths.len(), 22, "el nav-rail debe tener 22 items");
        for p in [
            "panel-principal",
            "mi-agenda",
            "planificacion",
            "asignaturas",
            "tareas-y-actividades",
            "examenes",
            "vida-escolar",
            "horarios",
            "asistente-academico",
            "conversaciones",
            "generador-de-material",
            "segundo-cerebro",
            "mis-notas",
            "busqueda-global",
            "documentos",
            "hojas-de-calculo",
            "presentaciones",
            "biblioteca-de-archivos",
            "centro-mcp",
            "estado-de-conexiones",
            "administracion",
            "configuracion",
        ] {
            assert!(paths.iter().any(|x| x == p), "falta el item {p}");
        }
    }

    #[test]
    fn solo_un_item_del_nav_queda_marcado_como_pagina_actual() {
        for active in ["panel-principal", "segundo-cerebro", "configuracion"] {
            let html = stitch_v2_page(active, "<p>x</p>", false);
            assert_eq!(
                html.matches("aria-current=\"page\"").count(),
                1,
                "aria-current=\"page\" debe aparecer 1 vez (activo: {active})"
            );
        }
    }

    #[test]
    fn los_enlaces_del_nav_tienen_nombre_accesible() {
        let html = stitch_v2_page("panel-principal", "<p>x</p>", false);
        // El icono no es el nombre: cada enlace nombra su destino.
        assert!(html.contains("aria-label=\"Panel principal\""));
        assert!(html.contains("aria-label=\"Hojas de cálculo\""));
        assert!(html.contains("aria-label=\"Asistente académico\""));
        // El icono de Material Symbols queda como decorativo.
        assert!(html.contains("aria-hidden=\"true\""));
    }

    #[test]
    fn el_skip_link_va_al_inicio_del_body_y_apunta_al_contenido() {
        let html = stitch_v2_page("panel-principal", "<p>x</p>", false);
        let body = html.find("<body").expect("body");
        let skip = html.find("class=\"skip-link\"").expect("skip-link");
        let aside = html.find("<aside").expect("aside");
        assert!(
            skip > body && skip < aside,
            "el skip-link va al inicio del body"
        );
        assert!(html.contains("href=\"#contenido\""));
        assert!(html.contains("<main id=\"contenido\" tabindex=\"-1\""));
    }

    #[test]
    fn el_css_del_shell_usa_los_mismos_ids_que_el_html() {
        for id in [
            RAIL_ID,
            RAIL_CONTENT_ID,
            TOPBAR_ID,
            RAIL_BACKDROP_ID,
            MAIN_CONTENT_ID,
        ] {
            assert!(
                SHELL_CSS.contains(id),
                "el CSS del shell debe referenciar el id `{id}`"
            );
        }
        // El rail se pliega en móvil y en escritorio.
        assert!(SHELL_CSS.contains("@media (max-width:1023.98px)"));
        assert!(SHELL_CSS.contains("body.rail-open #app-rail"));
        assert!(SHELL_CSS.contains("body.rail-collapsed #app-rail"));
        // El foco visible no se elimina: se refuerza.
        assert!(SHELL_CSS.contains(":focus-visible"));
        assert!(SHELL_CSS.contains("outline:2px solid"));
    }

    #[test]
    fn el_shell_equilibra_las_etiquetas_igual_que_antes() {
        // El shell cierra el `<div class="pl-[250px]">`; los cambios de a11y
        // no pueden alterar ese balance.
        let original = format!(
            "{aside}<div class=\"pl-[250px]\">{topbar}<main>{content}</main></div>",
            aside = STITCH_V2_ASIDE,
            topbar = STITCH_V2_TOPBAR,
            content = "<p>x</p>",
        );
        let nuevo = stitch_v2_page("panel-principal", "<p>x</p>", false);
        for tag in ["div", "section", "button", "span", "a"] {
            let d_orig = count_open(&original, tag) as i64 - count_close(&original, tag) as i64;
            let d_nuevo = count_open(&nuevo, tag) as i64 - count_close(&nuevo, tag) as i64;
            assert_eq!(d_orig, d_nuevo, "el balance de <{tag}> cambió");
        }
    }
}

// ── Tarjetas del día (datos reales del cliente Mikareno compartido) ───────
//
// Las tres tarjetas (saludo, próxima clase y próximas tareas) se construyen
// con los datos que `stitch_panel::fetch_panel_data` ya trajo del cliente
// Mikareno compartido (perfil, horario, tareas y calendario): no hay I/O
// extra ni datos inventados. Si Mikareno no responde, cada tarjeta cae a un
// estado vacío honesto.

/// Normaliza un nombre de día (minúsculas, sin acentos) para comparar el
/// horario real con el día de la semana.
fn norm_dia(dia: &str) -> String {
    dia.trim()
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            _ => c,
        })
        .collect()
}

/// Nombre del día en español (minúsculas) para un `Weekday`.
fn dia_es(dia: Weekday) -> &'static str {
    match dia {
        Weekday::Mon => "lunes",
        Weekday::Tue => "martes",
        Weekday::Wed => "miércoles",
        Weekday::Thu => "jueves",
        Weekday::Fri => "viernes",
        Weekday::Sat => "sábado",
        Weekday::Sun => "domingo",
    }
}

/// Parte "HH:MM" inicial de una etiqueta de clase: (minutos, resto).
///
/// Sólo acepta etiquetas que *empiecen* por una hora ("07:35 - 08:15 —
/// Matemáticas [MATE01]"); el resto se descarta.
fn split_hora(etiqueta: &str) -> Option<(u32, &str)> {
    let b = etiqueta.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i > 2 || b.get(i) != Some(&b':') {
        return None;
    }
    let h: u32 = etiqueta[..i].parse().ok()?;
    let j = i + 1;
    let mut k = j;
    while k < b.len() && b[k].is_ascii_digit() {
        k += 1;
    }
    if k - j != 2 {
        return None;
    }
    let m: u32 = etiqueta[j..k].parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some((h * 60 + m, &etiqueta[k..]))
}

/// Hora de fin de la clase ("07:35 - 08:15 — Materia" → 08:15), si la trae.
fn fin_minutos(etiqueta: &str) -> Option<u32> {
    let (_, resto) = split_hora(etiqueta)?;
    let resto = resto.trim_start().strip_prefix('-')?.trim_start();
    split_hora(resto).map(|(m, _)| m)
}

/// Asignatura de una etiqueta ("07:35 - 08:15 — Mate [MATE01]" → "Mate").
fn asignatura_de(etiqueta: &str) -> String {
    let resto = match etiqueta.split_once(" — ") {
        Some((_, r)) => r.trim(),
        None => etiqueta.trim(),
    };
    match resto.split_once(" [") {
        Some((nombre, cola)) if cola.ends_with(']') => nombre.trim().to_string(),
        _ => resto.to_string(),
    }
}

/// "HH:MM" a partir de minutos desde medianoche.
fn hhmm(minutos: u32) -> String {
    format!("{:02}:{:02}", minutos / 60, minutos % 60)
}

/// Clases de un día del horario real: (inicio, fin, etiqueta), ordenadas por
/// hora de inicio. Las etiquetas sin hora interpretable se descartan.
fn clases_de(horario: &Horario, dia: Weekday) -> Vec<(u32, Option<u32>, String)> {
    let objetivo = norm_dia(dia_es(dia));
    let Some(dia_horario) = horario.dias.iter().find(|d| norm_dia(&d.dia) == objetivo) else {
        return Vec::new();
    };
    let mut clases: Vec<(u32, Option<u32>, String)> = dia_horario
        .clases
        .iter()
        .filter_map(|etiqueta| {
            let (inicio, _) = split_hora(etiqueta)?;
            Some((inicio, fin_minutos(etiqueta), etiqueta.clone()))
        })
        .collect();
    clases.sort_by_key(|(inicio, _, _)| *inicio);
    clases
}

/// Etiqueta corta de fecha ("Martes 21/10").
fn etiqueta_fecha(fecha: NaiveDate) -> String {
    format!(
        "{} {}",
        weekday_es(&fecha.format("%Y-%m-%d").to_string()),
        fecha.format("%d/%m")
    )
}

/// "mañana" / "en 3 días" / "la próxima semana" según la fecha destino.
fn relativo_dia(fecha: NaiveDate, hoy: NaiveDate) -> String {
    let dias = (fecha - hoy).num_days();
    if dias <= 1 {
        "mañana".to_string()
    } else if dias <= 6 {
        format!("en {} días", dias)
    } else {
        "la próxima semana".to_string()
    }
}

/// Fecha en español ("lunes, 19 de octubre de 2026").
fn fecha_es(fecha: NaiveDate) -> String {
    let iso = fecha.format("%Y-%m-%d").to_string();
    match mes_es(fecha.month()) {
        Some(mes) => format!(
            "{}, {} de {} de {}",
            weekday_es(&iso),
            fecha.day(),
            mes,
            fecha.year()
        ),
        None => iso,
    }
}

/// Tarjeta del shell v2: superficie con borde, icono y título.
fn card_shell(titulo: &str, icono: &str, cuerpo: &str) -> String {
    format!(
        r###"<div class="bg-surface-container border border-outline rounded-xl p-space-md flex flex-col">
  <div class="flex items-center gap-2 mb-space-sm">
    <span class="material-symbols-outlined text-[18px] text-primary-container">{icono}</span>
    <h2 class="font-headline-sm text-headline-sm text-on-surface">{titulo}</h2>
  </div>
  {cuerpo}
</div>"###,
        icono = icono,
        titulo = html_escape(titulo),
        cuerpo = cuerpo,
    )
}

/// Saludo con el nombre real del perfil y la fecha de hoy.
///
/// Sin perfil (Mikareno caído) sólo se muestra el saludo: nunca un nombre
/// inventado.
fn card_saludo(d: &PanelData) -> String {
    let ahora = Local::now().naive_local();
    let saludo = match ahora.hour() {
        0..=11 => "Buenos días",
        12..=18 => "Buenas tardes",
        _ => "Buenas noches",
    };
    let nombre = d
        .perfil
        .as_ref()
        .map(|p| p.nombre.split_whitespace().next().unwrap_or("").to_string())
        .unwrap_or_default();
    let titulo = if nombre.is_empty() {
        saludo.to_string()
    } else {
        format!("{}, {}", saludo, nombre)
    };
    let institucion = d
        .perfil
        .as_ref()
        .map(|p| p.institucion.trim().to_string())
        .unwrap_or_default();
    let curso = d
        .perfil
        .as_ref()
        .map(|p| p.curso.trim().to_string())
        .unwrap_or_default();
    let sub = match (institucion.is_empty(), curso.is_empty()) {
        (false, false) => format!("{} · {}", institucion, curso),
        (false, true) => institucion,
        (true, false) => curso,
        (true, true) => String::new(),
    };

    let cuerpo = if nombre.is_empty() {
        format!(
            r###"<p class="font-headline-sm text-headline-sm text-on-surface">{titulo}</p>
<p class="font-body-sm text-body-sm text-on-surface-variant mt-space-xs">{fecha}</p>
<p class="font-label-sm text-label-sm text-outline mt-space-sm">Mikareno no conectado: sin datos de perfil.</p>"###,
            titulo = html_escape(&titulo),
            fecha = html_escape(&fecha_es(ahora.date())),
        )
    } else {
        format!(
            r###"<p class="font-headline-sm text-headline-sm text-on-surface">{titulo}</p>
<p class="font-body-sm text-body-sm text-on-surface-variant mt-space-xs">{fecha}</p>
{sub}"###,
            titulo = html_escape(&titulo),
            fecha = html_escape(&fecha_es(ahora.date())),
            sub = if sub.is_empty() {
                String::new()
            } else {
                format!(
                    "<p class=\"font-label-sm text-label-sm text-on-surface-variant mt-space-xs\">{}</p>",
                    html_escape(&sub)
                )
            },
        )
    };
    card_shell("Saludo", "waving_hand", &cuerpo)
}

/// Tarjeta "Próxima clase" — sólo afirma lo que el horario real sostiene.
fn card_proxima_clase(d: &PanelData) -> String {
    let ahora = Local::now().naive_local();
    let hoy = ahora.date();
    let minutos_ahora = ahora.hour() * 60 + ahora.minute();
    let hoy_iso = hoy.format("%Y-%m-%d").to_string();
    // Mikareno marca en el calendario los días no escolares: hoy no hay
    // clases aunque el horario semanal liste ese día.
    let no_lectivo = d
        .eventos
        .iter()
        .any(|e| e.fecha == hoy_iso && e.tipo == "NON_SCHOOL_DAY");

    let Some(horario) = d.horario.as_ref() else {
        return card_shell(
            "Próxima clase",
            "schedule",
            r###"<p class="font-body-sm text-body-sm text-on-surface-variant">Mikareno no conectado: no hay horario para calcular la próxima clase.</p>"###,
        );
    };

    let linea = |inicio: u32, etiqueta: &str| {
        format!(
            r###"<p class="font-body-md text-body-md text-on-surface">{} · {}</p>"###,
            html_escape(&hhmm(inicio)),
            html_escape(&asignatura_de(etiqueta)),
        )
    };

    let cuerpo = if no_lectivo {
        match siguiente_clase(horario, hoy, minutos_ahora, false) {
            Some((fecha, inicio, _, etiqueta)) => format!(
                r###"<p class="font-body-sm text-body-sm text-on-surface-variant">Hoy no es día lectivo: Mikareno lo marca como día no escolar.</p>
<p class="font-label-sm text-label-sm text-on-surface-variant mt-space-xs">{fecha}</p>
{linea}"###,
                fecha = html_escape(&etiqueta_fecha(fecha)),
                linea = linea(inicio, &etiqueta),
            ),
            None => r###"<p class="font-body-sm text-body-sm text-on-surface-variant">Hoy no es día lectivo y no hay clases programadas en los próximos 7 días.</p>"###.to_string(),
        }
    } else {
        let clases_hoy = clases_de(horario, hoy.weekday());
        // Clase en curso: la hora de inicio ya pasó y la de fin aún no.
        let en_curso = clases_hoy.iter().find(|(inicio, fin, _)| {
            *inicio <= minutos_ahora && fin.map(|f| minutos_ahora < f).unwrap_or(false)
        });
        if let Some((inicio, _, etiqueta)) = en_curso {
            format!(
                r###"<p class="font-label-sm text-label-sm text-on-surface-variant">En curso ahora</p>
{linea}"###,
                linea = linea(*inicio, etiqueta),
            )
        } else if let Some((inicio, _, etiqueta)) = clases_hoy
            .iter()
            .find(|(inicio, _, _)| *inicio >= minutos_ahora)
        {
            let faltan = *inicio - minutos_ahora;
            let estado = if faltan == 0 {
                "Empieza ahora".to_string()
            } else if faltan < 60 {
                format!("Empieza en {} min", faltan)
            } else {
                format!("Empieza a las {}", hhmm(*inicio))
            };
            format!(
                r###"<p class="font-label-sm text-label-sm text-on-surface-variant">Hoy</p>
{linea}
<p class="font-label-sm text-label-sm text-on-surface-variant mt-space-xs">{estado}</p>"###,
                linea = linea(*inicio, etiqueta),
                estado = html_escape(&estado),
            )
        } else {
            // Hoy no queda ninguna clase por delante: la siguiente es de otro día.
            let msg = if clases_hoy.is_empty() {
                "Hoy no tienes clases en el horario."
            } else {
                "Las clases de hoy ya terminaron."
            };
            match siguiente_clase(horario, hoy, minutos_ahora, false) {
                Some((fecha, inicio, _, etiqueta)) => format!(
                    r###"<p class="font-body-sm text-body-sm text-on-surface-variant">{msg}</p>
<p class="font-label-sm text-label-sm text-on-surface-variant mt-space-xs">{fecha} · {rel}</p>
{linea}"###,
                    msg = html_escape(msg),
                    fecha = html_escape(&etiqueta_fecha(fecha)),
                    rel = html_escape(&relativo_dia(fecha, hoy)),
                    linea = linea(inicio, &etiqueta),
                ),
                None => format!(
                    r###"<p class="font-body-sm text-body-sm text-on-surface-variant">{msg} No hay clases programadas en los próximos 7 días.</p>"###,
                    msg = html_escape(msg),
                ),
            }
        }
    };

    card_shell("Próxima clase", "schedule", &cuerpo)
}

/// Próxima clase real del horario: hoy (si todavía queda) o el siguiente día
/// con clases dentro de los próximos 7 días.
fn siguiente_clase(
    horario: &Horario,
    hoy: NaiveDate,
    minutos_ahora: u32,
    incluir_hoy: bool,
) -> Option<(NaiveDate, u32, Option<u32>, String)> {
    if incluir_hoy {
        if let Some((inicio, fin, etiqueta)) = clases_de(horario, hoy.weekday())
            .into_iter()
            .find(|(inicio, _, _)| *inicio >= minutos_ahora)
        {
            return Some((hoy, inicio, fin, etiqueta));
        }
    }
    for offset in 1..=7i64 {
        let fecha = hoy + chrono::Duration::days(offset);
        if let Some((inicio, fin, etiqueta)) =
            clases_de(horario, fecha.weekday()).into_iter().next()
        {
            return Some((fecha, inicio, fin, etiqueta));
        }
    }
    None
}

/// Clave de orden por vencimiento: las fechas ISO ("2026-10-20") van primero.
fn clave_vencimiento(t: &Tarea) -> (u8, String) {
    let v = t.vencimiento.trim();
    let fecha: String = v.chars().take(10).collect();
    if fecha.len() == 10
        && fecha.as_bytes().get(4) == Some(&b'-')
        && fecha.as_bytes().get(7) == Some(&b'-')
    {
        (0, fecha)
    } else {
        (1, v.to_string())
    }
}

/// Vencimiento en formato corto ("2026-10-20T10:00" → "20/10 10:00").
fn vencimiento_corto(v: &str) -> String {
    let v = v.trim();
    if v.is_empty() {
        return "sin fecha".to_string();
    }
    let fecha: String = v.chars().take(10).collect();
    if let Ok(d) = NaiveDate::parse_from_str(&fecha, "%Y-%m-%d") {
        let hora = v
            .get(10..)
            .map(|h| {
                h.trim_start_matches(['T', ' '])
                    .chars()
                    .take(5)
                    .collect::<String>()
            })
            .unwrap_or_default();
        if hora.len() == 5 {
            return format!("{:02}/{:02} {}", d.day(), d.month(), hora);
        }
        return format!("{:02}/{:02}/{}", d.day(), d.month(), d.year());
    }
    v.chars().take(30).collect()
}

/// Tarjeta "Próximas tareas" con las 3 tareas pendientes más próximas.
///
/// Sin tareas en Mikareno muestra el estado honesto "Sin tareas pendientes
/// en Mikareno." (nunca ejemplos inventados).
fn card_agenda(d: &PanelData) -> String {
    let mut tareas: Vec<&Tarea> = d
        .tareas
        .iter()
        .filter(|t| !t.estado.to_lowercase().contains("entregada"))
        .collect();
    tareas.sort_by(|a, b| clave_vencimiento(a).cmp(&clave_vencimiento(b)));

    let cuerpo = if tareas.is_empty() {
        r###"<p class="font-body-sm text-body-sm text-on-surface-variant">Sin tareas pendientes en Mikareno.</p>"###.to_string()
    } else {
        let items = tareas
            .iter()
            .take(3)
            .map(|t| {
                format!(
                    r###"<li class="flex items-start gap-space-sm py-2 border-b border-outline/40 last:border-b-0">
  <span class="material-symbols-outlined text-[16px] text-primary-container">task_alt</span>
  <div class="flex-1">
    <p class="font-body-sm text-body-sm text-on-surface">{titulo}</p>
    <p class="font-label-sm text-label-sm text-on-surface-variant">vence {vence}</p>
  </div>
</li>"###,
                    titulo = html_escape(&t.titulo),
                    vence = html_escape(&vencimiento_corto(&t.vencimiento)),
                )
            })
            .collect::<Vec<_>>()
            .join("");
        let resto = if tareas.len() > 3 {
            format!(
                r###"<p class="font-label-sm text-label-sm text-on-surface-variant mt-space-sm">y {} más</p>"###,
                tareas.len() - 3
            )
        } else {
            String::new()
        };
        format!(
            r###"<ul class="list-none space-y-space-xs">{}</ul>{}"###,
            items, resto
        )
    };

    card_shell("Próximas tareas", "task_alt", &cuerpo)
}

/// Grid con las tres tarjetas del día.
fn render_day_cards(d: &PanelData) -> String {
    format!(
        r###"<div class="grid grid-cols-1 lg:grid-cols-3 gap-space-md mb-space-md">
  {saludo}
  {proxima}
  {agenda}
</div>"###,
        saludo = card_saludo(d),
        proxima = card_proxima_clase(d),
        agenda = card_agenda(d),
    )
}

/// Inserta las tarjetas del día en el contenido del dashboard.
///
/// Ancla: el comentario `<!-- MAIN DENSE GRID -->` del kit Stitch. Si el kit
/// cambiara y el ancla no existiera, las tarjetas se anteponen al contenido
/// (nunca se pierden ni rompen el HTML).
fn insert_day_cards(content: &str, cards: &str) -> String {
    const ANCHOR: &str = "<!-- MAIN DENSE GRID -->";
    if content.contains(ANCHOR) {
        content.replace(ANCHOR, &format!("{}\n{}", cards, ANCHOR))
    } else {
        format!("{}\n{}", cards, content)
    }
}

/// GET / -> panel principal (dashboard).
pub async fn stitch_index(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> impl IntoResponse {
    let data = crate::handlers::stitch_panel::fetch_panel_data(&state).await;
    let content = crate::handlers::stitch_panel::fill_panel(CONTENT_INDEX, &data);
    let authenticated = state.config.auth_disabled || extract_session_cookie(&req).is_some();
    let html = stitch_v2_page("panel-principal", &content, authenticated);
    let shell = crate::handlers::stitch_panel::fetch_shell_data(&state).await;
    let html = crate::handlers::stitch_panel::fill_shell(&html, &shell);
    (StatusCode::OK, Html(html))
}

/// GET /dashboard -> panel principal.
///
/// HTMX partial reloads (`?partial=stats|profile|events|tasks`) issued by
/// the legacy academic dashboard templates are delegated to
/// [`crate::academic::dashboard::index`], which renders those fragments.
pub async fn stitch_dashboard(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> axum::response::Response {
    use axum::extract::FromRequest;

    if req.uri().query().is_some_and(|q| q.contains("partial=")) {
        let parsed =
            axum::extract::Query::<crate::academic::dashboard::DashboardQuery>::from_request(
                req, &state,
            );
        return match parsed.await {
            Ok(query) => crate::academic::dashboard::index(State(state), query)
                .await
                .into_response(),
            Err(rejection) => rejection.into_response(),
        };
    }

    let data = crate::handlers::stitch_panel::fetch_panel_data(&state).await;
    let content = crate::handlers::stitch_panel::fill_panel(CONTENT_DASHBOARD, &data);
    // Tarjetas del día (saludo, próxima clase, próximas tareas) con los datos
    // reales que ya trajo el panel del cliente Mikareno compartido.
    let content = insert_day_cards(&content, &render_day_cards(&data));
    let authenticated = state.config.auth_disabled || extract_session_cookie(&req).is_some();
    let html = stitch_v2_page("panel-principal", &content, authenticated);
    let shell = crate::handlers::stitch_panel::fetch_shell_data(&state).await;
    let html = crate::handlers::stitch_panel::fill_shell(&html, &shell);
    (StatusCode::OK, Html(html)).into_response()
}

/// GET /dashboard/ai -> asistente academico.
pub async fn stitch_asistente(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> impl IntoResponse {
    let data = crate::handlers::stitch_chat::fetch_chat_data(&state).await;
    let content = crate::handlers::stitch_chat::fill_chat(CONTENT_ASISTENTE, &data);
    let authenticated = state.config.auth_disabled || extract_session_cookie(&req).is_some();
    let html = stitch_v2_page("asistente-academico", &content, authenticated);
    let shell = crate::handlers::stitch_panel::fetch_shell_data(&state).await;
    let html = crate::handlers::stitch_panel::fill_shell(&html, &shell);
    (StatusCode::OK, Html(html))
}

/// GET /dashboard/notes -> segundo cerebro (notas markdown).
pub async fn stitch_notes(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> impl IntoResponse {
    let data = crate::handlers::stitch_notes::fetch_notes_data(&state).await;
    let content = crate::handlers::stitch_notes::fill_notes(CONTENT_NOTES, &data);
    let authenticated = state.config.auth_disabled || extract_session_cookie(&req).is_some();
    let html = stitch_v2_page("segundo-cerebro", &content, authenticated);
    let shell = crate::handlers::stitch_panel::fetch_shell_data(&state).await;
    let html = crate::handlers::stitch_panel::fill_shell(&html, &shell);
    (StatusCode::OK, Html(html))
}

/// GET /dashboard/mcp -> centro MCP + configuracion OpenRouter.
pub async fn stitch_mcp(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> impl IntoResponse {
    let data = crate::handlers::stitch_mcp::fetch_mcp_data(&state).await;
    let content = crate::handlers::stitch_mcp::fill_mcp(CONTENT_MCP, &data);
    let authenticated = state.config.auth_disabled || extract_session_cookie(&req).is_some();
    let html = stitch_v2_page("centro-mcp", &content, authenticated);
    let shell = crate::handlers::stitch_panel::fetch_shell_data(&state).await;
    let html = crate::handlers::stitch_panel::fill_shell(&html, &shell);
    (StatusCode::OK, Html(html))
}
