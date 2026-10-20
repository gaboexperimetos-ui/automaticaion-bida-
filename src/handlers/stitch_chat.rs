//! Panel del asistente académico (Stitch v2): inyección de datos reales.
//!
//! Reemplaza los marcadores `<!--SLOT:...-->` del HTML del kit Stitch
//! (`static/stitch-v2/_content/clean/asistente.html`) por el estado real del
//! asistente: modelo configurado, modo (mock/API), las **materias reales**
//! que existen en `chat_history`, el **historial real** de la materia activa
//! (los últimos [`HISTORY_TURNS_LIMIT`] turnos, en orden) y el gasto del mes
//! desde `usage_metrics` (la misma fuente que sirve `GET /api/ai/cost`).
//! Mientras no haya historial persistido, cada zona degrada a su estado vacío
//! legible en vez de mostrar datos inventados.
//!
//! Además inyecta el script de **streaming** del compositor: el kit trae un
//! `<script>` propio que hace POST bufferizado a `/api/ai/chat`; este módulo
//! lo intercepta (fase de captura) y lo sustituye por la versión en streaming
//! contra `POST /api/ai/chat/stream` (SSE leído con `fetch` +
//! `ReadableStream`, sin `EventSource` porque el endpoint es POST).
//!
//! La materia activa se resuelve **en el servidor**: la página lee
//! `?subject=` y, si viene, precarga esa materia (opción activa del
//! selector, `var SUBJECT` del script e historial pintado). Si no viene,
//! manda la materia más reciente de `chat_history`.
//!
//! El mismo script hace tres cosas más, siempre **sin recargar la página**:
//!   1. replica en el DOM el turno recién enviado (burbuja del estudiante a
//!      la derecha, burbuja del asistente a la izquierda) con las mismas
//!      clases que el historial pintado por el servidor;
//!   2. cablea el selector de materia: al elegir una materia real carga su
//!      historial con `GET /api/ai/conversations/{subject}/messages` y lo
//!      pinta en el panel central; al escribir una materia nueva la crea y
//!      deja el panel en su estado vacío honesto;
//!   3. sustituye el botón "Todas" del kit (que no hacía nada) por el
//!      selector real de materias.

use std::sync::Arc;

use axum::{
    extract::{Query, Request, State},
    http::StatusCode,
    response::{Html, IntoResponse},
};

use chrono::Datelike;

use crate::handlers::stitch_panel::{fetch_shell_data, fill_shell};
use crate::handlers::stitch_v2::stitch_v2_page;
use crate::middleware::auth::extract_session_cookie;
use crate::AppState;

const MCP_TIMEOUT_SECS: u64 = 10;

/// HTML del kit Stitch para el asistente (el que rellena `fill_chat`).
const CONTENT_ASISTENTE: &str = include_str!("../../static/stitch-v2/_content/clean/asistente.html");

/// Longitud máxima de una materia (el input de "nueva materia" usa 60).
const MAX_SUBJECT_LEN: usize = 60;

/// Parámetros de query del panel del asistente: `?subject=<materia>`.
///
/// El parseo es tolerante a propósito: una query rara (`?subject[]=x`,
/// `?subject` sin valor, `?subject=a&subject=b`) no debe tumbar la página con
/// un 400; en el peor caso la materia se ignora y manda la más reciente.
#[derive(Debug, Default, Clone)]
pub struct ChatQuery {
    /// Materia a precargar (opcional).
    pub subject: Option<String>,
}

impl<'de> serde::Deserialize<'de> for ChatQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Params;

        impl<'de> serde::de::Visitor<'de> for Params {
            type Value = ChatQuery;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("los parámetros de la página del asistente")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut subject: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "subject" {
                        // Con `?subject=a&subject=b` gana el primero.
                        let value: Materia = map.next_value()?;
                        if subject.is_none() {
                            subject = value.0;
                        }
                    } else {
                        let _ = map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                Ok(ChatQuery { subject })
            }
        }

        deserializer.deserialize_map(Params)
    }
}

/// Valor de `?subject=`: acepta texto, número o lista y nunca rechaza la
/// request.
struct Materia(Option<String>);

impl<'de> serde::Deserialize<'de> for Materia {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        de_subject(deserializer).map(Materia)
    }
}

/// Deserializa el valor de `?subject=` sin fallar nunca: cualquier forma
/// inesperada (lista, mapa, número) se degrada a "sin materia" en vez de
/// rechazar la request.
fn de_subject<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Valor;

    impl<'de> serde::de::Visitor<'de> for Valor {
        type Value = Option<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("el nombre de una materia")
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
            Ok(Some(String::from_utf8_lossy(v).to_string()))
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_any(Valor)
        }

        /// `?subject[]=a`: se toma el primer elemento y no se rechaza.
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            match seq.next_element::<String>() {
                Ok(Some(v)) => Ok(Some(v)),
                Ok(None) => Ok(None),
                Err(_) => Ok(None),
            }
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            _map: A,
        ) -> Result<Self::Value, A::Error> {
            Ok(None)
        }
    }

    deserializer.deserialize_any(Valor)
}

/// Normaliza la materia venida de `?subject=`:
///
/// - recorta los espacios sobrantes;
/// - descarta los caracteres de control (no deben poder romper el HTML, el
///   JSON del script ni la consulta);
/// - acota la longitud;
/// - una materia vacía se trata como "no vino nada" (manda la más reciente).
fn normalize_subject(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let clean: String = raw.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    if clean.is_empty() {
        return None;
    }
    Some(clean.chars().take(MAX_SUBJECT_LEN).collect())
}

/// Máximo de sesiones listadas en el panel izquierdo.
const SESSIONS_LIMIT: i64 = 12;

/// Turnos (mensajes `user`/`assistant`) del historial que se muestran en el
/// panel central: los últimos de la materia activa, en orden cronológico.
const HISTORY_TURNS_LIMIT: usize = 20;

/// Materias reales (asuntos distintos con mensajes) que ofrece el selector.
const SUBJECTS_LIMIT: i64 = 50;

/// Script del compositor en modo streaming.
///
/// El kit Stitch ya incluye su propio script (POST bufferizado a
/// `/api/ai/chat`). Este lo intercepta en fase de captura (los listeners del
/// kit viven en el nodo destino, así que un `stopPropagation` en la fase de
/// captura los evita) y lo sustituye por la versión en streaming contra
/// `POST /api/ai/chat/stream`.
///
/// El stream es SSE pero el endpoint es POST, así que `EventSource` no sirve:
/// se lee el `ReadableStream` de `fetch` con un `TextDecoder` y se trocean
/// los frames a mano por `\n\n` (`event:`/`data:`).
///
/// Contrato de frames (ver `src/ai/streaming.rs`):
///   - `delta`    → `{"content":"..."}`  se añade al texto de la respuesta
///   - `reasoning`→ `{"reasoning":"..."}` texto gris plegable ("Pensando…")
///   - `done`     → `{"usage":{"input":N,"output":N,"cost":0.0}}` y, en modo
///                  demo, `{"content":"[Modo simulado] ..."}`
///   - `error`    → `{"error":"..."}` banner rojo en línea
///
/// Todo el texto se inserta con `textContent` (nunca `innerHTML`).
///
/// Marcadores sustituidos por [`render_stream_script`]:
///   - `__SUBJECT__` → literal JSON del asunto activo (el valor entero, así
///     que se inserta una sola vez, entre comillas y con los escapes
///     correctos).
///   - `__CLASSES__` → literal JSON de las clases de las burbujas (fuente
///     única de verdad compartida con el render en Rust).
///   - `__LIMIT__`   → número de turnos que se pintan como historial.
///   - `__MODEL__`   → literal JSON del nombre del modelo configurado.
const STREAM_SCRIPT: &str = r##"(function () {
  "use strict";

  var SUBJECT = __SUBJECT__;
  var C = __CLASSES__;
  var HISTORY_LIMIT = __LIMIT__;
  var MODEL = __MODEL__;

  var NEW_SUBJECT = "__new__";
  var MONTHS = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];

  var chatContainer = document.getElementById("chat-messages-container");
  var promptInput = document.getElementById("prompt-input");
  var sendBtn = document.getElementById("send-btn");
  var subjectSelect = document.getElementById("chat-subject-select");
  var newSubjectInput = document.getElementById("chat-subject-new");
  if (!chatContainer || !promptInput || !sendBtn) { return; }

  var busy = false;
  var currentCount = 0;

  function el(tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) { n.className = cls; }
    if (text !== undefined && text !== null) { n.textContent = text; }
    return n;
  }

  function scroll() {
    chatContainer.scrollTop = chatContainer.scrollHeight;
  }

  // El nodo sigue en el documento (si el usuario limpió la conversación a
  // media respuesta, el nodo se desconecta y ya no hay dónde escribir).
  function live(b) {
    return !b.root || b.root.isConnected !== false;
  }

  function setBusy(flag) {
    sendBtn.disabled = !!flag;
    sendBtn.classList.toggle("opacity-60", !!flag);
    var lbl = sendBtn.querySelector("span:not(.material-symbols-outlined)");
    if (lbl) { lbl.textContent = flag ? "Generando…" : "Enviar"; }
  }

  function pad2(n) { return (n < 10 ? "0" : "") + n; }

  // Espejo de `time_label` en Rust: hoy → "HH:MM", ayer → "Ayer",
  // antes → "22 de oct". Las marcas de tiempo de sqlite son UTC.
  function timeLabel(raw) {
    if (!raw) { return ""; }
    var m = String(raw).match(/^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})/);
    if (!m) { return String(raw); }
    var now = new Date();
    var y = now.getUTCFullYear();
    var mo = now.getUTCMonth() + 1;
    var d = now.getUTCDate();
    var today = y + "-" + pad2(mo) + "-" + pad2(d);
    var prev = new Date(Date.UTC(y, mo - 1, d - 1));
    var yesterday = prev.getUTCFullYear() + "-" + pad2(prev.getUTCMonth() + 1) + "-" + pad2(prev.getUTCDate());
    var day = m[1] + "-" + m[2] + "-" + m[3];
    if (day === today) { return m[4] + ":" + m[5]; }
    if (day === yesterday) { return "Ayer"; }
    return parseInt(m[3], 10) + " de " + MONTHS[parseInt(m[2], 10) - 1];
  }

  // --- historial ---------------------------------------------------------

  function clearMessages() {
    while (chatContainer.firstChild) { chatContainer.removeChild(chatContainer.firstChild); }
  }

  // Estado vacío honesto: no hay nada guardado para esta materia.
  function renderEmpty() {
    var wrap = el("div", "max-w-4xl mx-auto w-full text-center py-space-lg");
    var icon = el("div", "w-9 h-9 rounded-xl bg-primary-container/15 flex items-center justify-center mx-auto text-primary-container");
    icon.appendChild(el("span", "material-symbols-outlined text-[20px]", "smart_toy"));
    wrap.appendChild(icon);
    wrap.appendChild(el("p", "font-label-md text-label-md font-semibold text-on-surface mt-2", "Sin conversaciones"));
    wrap.appendChild(el("p", "font-body-sm text-body-sm text-outline mt-1", "Envía un mensaje para empezar: el historial de esta materia aparecerá aquí."));
    chatContainer.appendChild(wrap);
    scroll();
  }

  // Burbuja del estudiante: avatar a la derecha, texto a su izquierda.
  function buildUser(text, when) {
    var row = el("div", C.user_row);
    row.setAttribute("data-chat-turn", "user");
    var avatar = el("div", C.user_avatar, "TÚ");
    var col = el("div", C.user_col);
    var head = el("div", C.user_head);
    head.appendChild(el("span", C.user_name, "Estudiante"));
    head.appendChild(el("span", C.time, when || "Ahora"));
    var bubble = el("div", C.user_bubble);
    bubble.appendChild(el("p", C.user_text, text));
    col.appendChild(head);
    col.appendChild(bubble);
    row.appendChild(avatar);
    row.appendChild(col);
    chatContainer.appendChild(row);
    scroll();
    return row;
  }

  // Burbuja del asistente: avatar a la izquierda. `buildAssistant` la usa
  // tanto el historial como el turno en vivo (el vivo añade razonamiento,
  // cursor y uso de tokens).
  function buildAssistant(when) {
    var row = el("div", C.assistant_row);
    row.setAttribute("data-chat-turn", "assistant");
    var avatar = el("div", C.assistant_avatar);
    avatar.appendChild(el("span", "material-symbols-outlined text-[20px]", "smart_toy"));
    var col = el("div", C.assistant_col);
    var head = el("div", C.assistant_head);
    head.appendChild(el("span", C.assistant_name, MODEL || "Asistente IA"));
    head.appendChild(el("span", C.subject_chip, SUBJECT));
    head.appendChild(el("span", C.time, when || "Ahora"));
    var bubble = el("div", C.assistant_bubble);
    var p = el("p", C.assistant_text);
    var textNode = document.createTextNode("");
    p.appendChild(textNode);
    bubble.appendChild(p);
    col.appendChild(head);
    col.appendChild(bubble);
    row.appendChild(avatar);
    row.appendChild(col);
    chatContainer.appendChild(row);
    scroll();
    return { root: row, col: col, bubble: bubble, text: textNode, done: false };
  }

  // Pinta un historial recibido de la API (mismo aspecto que el del servidor).
  function paintHistory(messages) {
    clearMessages();
    var list = messages.slice(-HISTORY_LIMIT);
    if (!list.length) {
      currentCount = 0;
      renderEmpty();
      updateContext(SUBJECT, 0);
      return;
    }
    for (var i = 0; i < list.length; i++) {
      var m = list[i] || {};
      if (m.role === "user") {
        buildUser(String(m.content || ""), timeLabel(m.created_at));
      } else {
        var b = buildAssistant(timeLabel(m.created_at));
        b.text.appendData(String(m.content || ""));
      }
    }
    currentCount = list.length;
    updateContext(SUBJECT, currentCount);
    scroll();
  }

  // --- materia (subject) -------------------------------------------------

  function ensureSubjectOption(value) {
    if (!subjectSelect) { return; }
    for (var i = 0; i < subjectSelect.options.length; i++) {
      if (subjectSelect.options[i].value === value) { return; }
    }
    var opt = document.createElement("option");
    opt.value = value;
    opt.textContent = value;
    var newOpt = subjectSelect.querySelector('option[data-new="1"]');
    if (newOpt && newOpt.parentNode) { subjectSelect.insertBefore(opt, newOpt); }
    else { subjectSelect.appendChild(opt); }
  }

  function setSubject(subject) {
    var s = (subject || "").trim();
    SUBJECT = s || "general";
    ensureSubjectOption(SUBJECT);
    if (subjectSelect) { subjectSelect.value = SUBJECT; }
    syncUrl(SUBJECT);
  }

  // La URL lleva la materia activa: al recargar, el servidor la precarga
  // (y así se puede abrir o compartir un chat concreto).
  function syncUrl(subject) {
    if (!window.history || !window.history.replaceState) { return; }
    try {
      var url = new URL(window.location.href);
      url.searchParams.set("subject", subject);
      window.history.replaceState(null, "", url.pathname + url.search + url.hash);
    } catch (e) { /* sin historial: la materia sigue viva en memoria */ }
  }

  // Chip de contexto de la cabecera (materia + número de mensajes).
  function updateContext(subject, count) {
    var box = document.querySelector("[data-context]");
    if (!box) { return; }
    while (box.firstChild) { box.removeChild(box.firstChild); }
    box.appendChild(el("span", C.context_subject, subject));
    if (typeof count === "number") {
      box.appendChild(document.createTextNode(" "));
      box.appendChild(el("span", "text-outline text-body-sm", "+"));
      box.appendChild(document.createTextNode(" "));
      box.appendChild(el("span", C.context_count, count + " mensajes"));
    }
  }

  // Carga el historial de una materia real desde la API (sin recargar).
  function loadSubject(subject) {
    setSubject(subject);
    var url = "/api/ai/conversations/" + encodeURIComponent(SUBJECT) + "/messages";
    fetch(url, { credentials: "same-origin" })
      .then(function (resp) {
        if (!resp.ok) { throw new Error("HTTP " + resp.status); }
        return resp.json();
      })
      .then(function (data) {
        paintHistory((data && data.messages) ? data.messages : []);
      })
      .catch(function () {
        clearMessages();
        var wrap = el("div", "max-w-4xl mx-auto w-full text-center py-space-lg");
        wrap.appendChild(el("p", "font-label-md text-label-md font-semibold text-on-surface", "No se pudo cargar el historial"));
        wrap.appendChild(el("p", "font-body-sm text-body-sm text-outline mt-1", "Vuelve a intentarlo en un momento."));
        chatContainer.appendChild(wrap);
        scroll();
      });
  }

  function showNewSubjectInput() {
    if (!newSubjectInput) { return; }
    newSubjectInput.style.display = "";
    newSubjectInput.focus();
  }

  function hideNewSubjectInput() {
    if (!newSubjectInput) { return; }
    newSubjectInput.style.display = "none";
  }

  // Materia nueva: se crea (vacía) y a partir de ahí el chat guarda ahí.
  function applyNewSubject() {
    if (!newSubjectInput) { return; }
    var v = (newSubjectInput.value || "").trim();
    hideNewSubjectInput();
    newSubjectInput.value = "";
    if (!v) { return; }
    setSubject(v);
    paintHistory([]);
  }

  // El botón "Todas" del kit no hacía nada: el selector real lo sustituye.
  function hideLegacyFilter() {
    if (!subjectSelect || !subjectSelect.parentNode) { return; }
    var btns = subjectSelect.parentNode.querySelectorAll("button");
    for (var i = 0; i < btns.length; i++) {
      var t = (btns[i].textContent || "").trim().toLowerCase();
      if (t === "todas" || t === "all") { btns[i].style.display = "none"; }
    }
  }

  // --- streaming ---------------------------------------------------------

  // Burbuja del asistente en vivo: razonamiento plegable + texto + uso.
  function renderAssistant() {
    var b = buildAssistant("Ahora");

    var details = el("details", "mb-2");
    details.hidden = true;
    details.appendChild(el("summary", "font-label-sm text-[10px] uppercase tracking-wider text-outline cursor-pointer select-none", "Pensando…"));
    var rbox = el("div", "mt-1 rounded-lg bg-surface-container/60 px-3 py-2");
    var rtext = el("span", "font-body-sm text-body-sm text-outline italic whitespace-pre-wrap");
    rbox.appendChild(rtext);
    details.appendChild(rbox);
    b.col.insertBefore(details, b.bubble);

    var caret = el("span", "font-body-md text-body-md text-primary-container", "▍");
    b.bubble.appendChild(caret);

    var usage = el("div", "mt-1 font-code-inline text-[10px] text-outline");
    b.col.appendChild(usage);

    b.details = details;
    b.reasoning = rtext;
    b.usage = usage;
    b.caret = caret;
    return b;
  }

  function showError(msg) {
    var wrap = el("div", "max-w-4xl mx-auto w-full");
    var banner = el("div", "rounded-xl bg-error-container/20 border border-error/40 px-4 py-3 flex items-start gap-2");
    banner.appendChild(el("span", "material-symbols-outlined text-[18px] text-error", "error"));
    banner.appendChild(el("span", "font-body-sm text-body-sm text-error", msg));
    wrap.appendChild(banner);
    chatContainer.appendChild(wrap);
    scroll();
  }

  function appendDelta(b, chunk) {
    if (!chunk || !live(b)) { return; }
    b.text.appendData(String(chunk));
    scroll();
  }

  function appendReasoning(b, chunk) {
    if (!chunk || !live(b)) { return; }
    b.details.hidden = false;
    b.reasoning.appendChild(document.createTextNode(String(chunk)));
  }

  function finish(b, payload) {
    if (!b || b.done) { return; }
    b.done = true;
    if (b.caret && b.caret.parentNode) { b.caret.parentNode.removeChild(b.caret); }
    if (payload && typeof payload.content === "string" && payload.content) {
      b.text.appendData(payload.content);
    }
    var u = payload ? payload.usage : null;
    if (u && typeof u === "object") {
      var tin = Number(u.input) || 0;
      var tout = Number(u.output) || 0;
      var cost = Number(u.cost) || 0;
      b.usage.textContent = tin + "→" + tout + " tokens · $" + cost.toFixed(2);
    }
    scroll();
  }

  // Un frame SSE: "event: <nombre>\ndata: <json>".
  function handleFrame(b, frame) {
    var ev = "";
    var data = "";
    var lines = frame.split("\n");
    for (var i = 0; i < lines.length; i++) {
      var line = lines[i].replace(/\r$/, "");
      if (line.indexOf("event:") === 0) {
        ev = line.slice(6).trim();
      } else if (line.indexOf("data:") === 0) {
        var v = line.slice(5);
        if (v.charAt(0) === " ") { v = v.slice(1); }
        data = data ? data + "\n" + v : v;
      }
    }
    if (!ev || !data) { return; }
    var payload = null;
    try { payload = JSON.parse(data); } catch (e) { return; }
    if (ev === "delta") {
      appendDelta(b, payload ? payload.content : "");
    } else if (ev === "reasoning") {
      appendReasoning(b, payload ? payload.reasoning : "");
    } else if (ev === "done") {
      finish(b, payload);
    } else if (ev === "error") {
      showError((payload && payload.error) ? String(payload.error) : "Error desconocido");
    }
  }

  function canStream() {
    return typeof window.ReadableStream !== "undefined" &&
           typeof window.TextDecoder !== "undefined" &&
           typeof window.fetch === "function" &&
           typeof window.Promise === "function";
  }

  function streamChat(message, b) {
    return window.fetch("/api/ai/chat/stream", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ message: message, subject: SUBJECT, content: "" })
    }).then(function (resp) {
      if (!resp.ok) {
        return resp.text().then(function (raw) {
          var msg = raw;
          try {
            var j = JSON.parse(raw);
            if (j && j.error) { msg = j.error; }
          } catch (e) { /* la respuesta no es JSON: se muestra tal cual */ }
          throw new Error(msg || ("HTTP " + resp.status));
        });
      }
      if (!resp.body || typeof resp.body.getReader !== "function") {
        throw new Error("El navegador no puede leer el stream de la respuesta.");
      }
      var reader = resp.body.getReader();
      var decoder = new window.TextDecoder("utf-8");
      var buf = "";
      function pump() {
        return reader.read().then(function (chunk) {
          if (chunk.done) { return null; }
          buf += decoder.decode(chunk.value, { stream: true });
          var idx = buf.indexOf("\n\n");
          while (idx >= 0) {
            var frame = buf.slice(0, idx);
            buf = buf.slice(idx + 2);
            handleFrame(b, frame);
            idx = buf.indexOf("\n\n");
          }
          return pump();
        });
      }
      return pump().then(function () {
        buf += decoder.decode();
        if (buf.trim()) { handleFrame(b, buf); }
        if (!b.done) { finish(b, null); }
      });
    });
  }

  // Fallback: sin ReadableStream (o sin TextDecoder) se usa el POST normal.
  function sendBuffered(message, b) {
    return window.fetch("/api/ai/chat", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ message: message, subject: SUBJECT })
    }).then(function (resp) {
      return resp.text().then(function (text) {
        if (!resp.ok) { throw new Error(text || ("HTTP " + resp.status)); }
        finish(b, { content: text });
      });
    });
  }

  // El turno nuevo se replica en el DOM: burbuja del estudiante + burbuja
  // del asistente, sin recargar la página.
  function sendMessage() {
    if (busy) { return; }
    var message = promptInput.value.trim();
    if (!message) { return; }
    promptInput.value = "";
    buildUser(message, "Ahora");
    var b = renderAssistant();
    busy = true;
    setBusy(true);

    var run = canStream() ? streamChat(message, b) : sendBuffered(message, b);
    run.catch(function (err) {
      showError((err && err.message) ? err.message : "Error de red");
    }).then(function () {
      busy = false;
      setBusy(false);
      currentCount += 2;
      updateContext(SUBJECT, currentCount);
    });
  }

  // Intercepta en fase de captura el click del botón y el Enter del
  // compositor: los listeners del kit están en el nodo destino, así que
  // stopPropagation en la captura los evita y gana el streaming.
  document.addEventListener("click", function (e) {
    if (e.target === sendBtn || (sendBtn.contains && sendBtn.contains(e.target))) {
      e.preventDefault();
      e.stopPropagation();
      sendMessage();
    }
  }, true);

  document.addEventListener("keydown", function (e) {
    var isEnter = e.key === "Enter" || e.keyCode === 13;
    if (isEnter && !e.shiftKey && e.target === promptInput) {
      e.preventDefault();
      e.stopPropagation();
      sendMessage();
    }
  }, true);

  // --- cableado del selector de materia ----------------------------------

  if (subjectSelect) {
    subjectSelect.addEventListener("change", function () {
      var v = subjectSelect.value;
      if (v === NEW_SUBJECT) {
        showNewSubjectInput();
        return;
      }
      loadSubject(v);
    });
    hideLegacyFilter();
  }

  if (newSubjectInput) {
    newSubjectInput.addEventListener("keydown", function (e) {
      var isEnter = e.key === "Enter" || e.keyCode === 13;
      if (isEnter) {
        e.preventDefault();
        e.stopPropagation();
        applyNewSubject();
      } else if (e.key === "Escape") {
        hideNewSubjectInput();
        newSubjectInput.value = "";
      }
    });
    newSubjectInput.addEventListener("blur", function () {
      if ((newSubjectInput.value || "").trim()) { applyNewSubject(); }
      else { hideNewSubjectInput(); }
    });
  }

  // Clic en una sesión del panel izquierdo: cambia de materia.
  document.addEventListener("click", function (e) {
    var t = e.target;
    if (!t || typeof t.closest !== "function") { return; }
    var card = t.closest("[data-chat-subject]");
    if (!card) { return; }
    e.preventDefault();
    e.stopPropagation();
    loadSubject(card.getAttribute("data-chat-subject"));
  }, true);

  // "Limpiar pantalla" del kit: la vista se vacía, la BD no. Se dice.
  document.addEventListener("click", function (e) {
    var t = e.target;
    if (!t || typeof t.closest !== "function") { return; }
    var btn = t.closest("#btn-clear-chat, #btn-new-chat");
    if (!btn) { return; }
    e.preventDefault();
    e.stopPropagation();
    clearMessages();
    var wrap = el("div", "max-w-4xl mx-auto w-full text-center py-space-lg");
    wrap.appendChild(el("p", "font-label-md text-label-md font-semibold text-on-surface", "Pantalla limpia"));
    wrap.appendChild(el("p", "font-body-sm text-body-sm text-outline mt-1", "El historial guardado de esta materia sigue en la base de datos."));
    chatContainer.appendChild(wrap);
    scroll();
  }, true);

  // Estado inicial: la materia activa es la que trae el servidor.
  setSubject(SUBJECT);
  currentCount = chatContainer.querySelectorAll("[data-chat-turn]").length;
  updateContext(SUBJECT, currentCount);
})();"##;

/// Una sesión de chat: agrupa los mensajes por `subject`.
#[derive(Default, Clone)]
pub struct SessionRow {
    pub subject: String,
    /// Fecha/hora UTC del último mensaje ("YYYY-MM-DD HH:MM:SS").
    pub last_at: String,
    /// Mensajes de la sesión (user + assistant).
    pub msgs: i64,
    /// Primera pregunta del usuario (título de la sesión).
    pub first_user: String,
    /// Última respuesta del asistente (preview).
    pub preview: String,
}

/// Un mensaje de la conversación enfocada.
#[derive(Clone, Default)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// Contexto real que alimenta la página del asistente.
#[derive(Default, Clone)]
pub struct ChatPanelData {
    pub connected: bool,
    pub model: String,
    pub api_configured: bool,
    /// Sesiones reales (más reciente primero) o vacío.
    pub sessions: Vec<SessionRow>,
    /// Materias reales de `chat_history` (para el selector), más reciente
    /// primero.
    pub subjects: Vec<String>,
    /// (subject, últimos turnos) de la sesión más reciente, para el panel
    /// central.
    pub focus: Option<(String, Vec<ChatMsg>)>,
    /// Materia activa resuelta en el servidor: la que pide `?subject=` o, si
    /// no viene, la más reciente de `chat_history`. `None` cuando no hay
    /// ninguna materia real (todo vacío).
    pub active_subject: Option<String>,
    /// Gasto del mes en USD y presupuesto (misma fuente que GET /api/ai/cost).
    pub spend: f64,
    pub budget: f64,
    pub requests: u64,
}

/// Materia activa real (la que el servidor resolvió), o `None` si no hay
/// ninguna.
///
/// Prioridad: la fijada por el servidor (`?subject=`), y si no, la del
/// historial enfocado.
fn active_subject_opt(d: &ChatPanelData) -> Option<&str> {
    d.active_subject
        .as_deref()
        .or_else(|| d.focus.as_ref().map(|(s, _)| s.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Materia activa que se inyecta en el script y en el selector. Sin ninguna
/// materia real, el neutro "general".
fn active_subject(d: &ChatPanelData) -> &str {
    active_subject_opt(d).unwrap_or("general")
}

/// Clases de las burbujas de la conversación.
///
/// **Fuente única de verdad**: las usa el render del historial en Rust
/// ([`render_messages`]) y el script que replica un turno nuevo en el DOM
/// (inyectado como literal JSON), de modo que un turno recién enviado se ve
/// exactamente igual que uno cargado desde la base de datos.
#[derive(serde::Serialize)]
struct UiClasses {
    /// Fila del mensaje del estudiante (avatar a la derecha).
    user_row: &'static str,
    user_avatar: &'static str,
    user_col: &'static str,
    user_head: &'static str,
    user_name: &'static str,
    user_bubble: &'static str,
    user_text: &'static str,
    /// Fila del mensaje del asistente (avatar a la izquierda).
    assistant_row: &'static str,
    assistant_avatar: &'static str,
    assistant_col: &'static str,
    assistant_head: &'static str,
    assistant_name: &'static str,
    assistant_bubble: &'static str,
    assistant_text: &'static str,
    /// Chip con la materia dentro de la cabecera del asistente.
    subject_chip: &'static str,
    /// Marca de tiempo.
    time: &'static str,
    /// Chip de la cabecera de la página (materia activa).
    context_subject: &'static str,
    /// Chip de la cabecera de la página (nº de mensajes).
    context_count: &'static str,
}

/// Clases de la interfaz del panel de chat.
fn ui_classes() -> UiClasses {
    UiClasses {
        user_row: "flex items-start gap-space-md max-w-4xl mx-auto w-full flex-row-reverse",
        user_avatar: "w-9 h-9 rounded-full bg-surface-container-high flex items-center justify-center shrink-0 shadow-sm font-label-md font-bold text-[11px] text-on-surface",
        user_col: "flex-1 min-w-0 flex flex-col items-end",
        user_head: "flex items-center gap-space-xs mb-1 flex-row-reverse",
        user_name: "font-label-md text-label-md font-semibold text-on-surface",
        user_bubble: "bg-surface-container-high text-on-surface rounded-xl p-space-md shadow-sm max-w-[85%]",
        user_text: "font-body-md text-body-md leading-relaxed whitespace-pre-wrap",
        assistant_row: "flex items-start gap-space-md max-w-4xl mx-auto w-full",
        assistant_avatar: "w-9 h-9 rounded-full bg-primary-container/15 flex items-center justify-center shrink-0 shadow-sm text-primary-container",
        assistant_col: "flex-1 min-w-0",
        assistant_head: "flex flex-wrap items-center gap-space-xs mb-1",
        assistant_name: "font-label-md text-label-md font-semibold text-primary-fixed",
        assistant_bubble: "bg-surface-container-low rounded-xl p-space-lg text-on-surface shadow-md",
        assistant_text: "font-body-md text-body-md leading-relaxed text-on-surface-variant whitespace-pre-wrap",
        subject_chip: "px-1.5 py-0.5 rounded-full font-label-sm text-[10px] bg-primary-container/10 text-primary-container font-medium",
        time: "font-code-inline text-[10px] text-outline",
        context_subject: "px-2 py-0.5 rounded font-label-sm text-label-sm bg-surface-container-high text-primary-fixed font-semibold",
        context_count: "px-1.5 py-0.5 rounded-full font-code-inline text-[10px] bg-secondary-container/20 text-secondary",
    }
}

/// Recolecta los datos del asistente. Nunca falla: ante un error deja los
/// campos vacíos y la página degrada a sus estados vacíos.
///
/// La materia activa es la más reciente de `chat_history`.
pub async fn fetch_chat_data(state: &AppState) -> ChatPanelData {
    fetch_chat_data_for(state, None).await
}

/// Igual que [`fetch_chat_data`], pero con la materia activa fijada de
/// antemano (la que pide `?subject=`). Si no se indica ninguna, o la
/// indicada no existe, manda la materia más reciente de `chat_history`.
pub async fn fetch_chat_data_for(state: &AppState, subject: Option<&str>) -> ChatPanelData {
    let mut d = ChatPanelData {
        model: state.config.openrouter_model.clone(),
        api_configured: state.config.openrouter_api_key.is_some(),
        ..Default::default()
    };

    // Sesiones reales de `chat_history` agrupadas por asignatura (la misma
    // agrupación que usa GET /api/ai/conversations).
    let rows = sqlx::query_as::<_, (String, String, i64, Option<String>, Option<String>)>(
        "SELECT subject, MAX(created_at) AS last_at, COUNT(*) AS msgs, \
         (SELECT c2.content FROM chat_history c2 \
            WHERE c2.subject = c.subject AND c2.role = 'user' \
            ORDER BY c2.created_at ASC LIMIT 1) AS first_user, \
         (SELECT c2.content FROM chat_history c2 \
            WHERE c2.subject = c.subject AND c2.role = 'assistant' \
            ORDER BY c2.created_at DESC LIMIT 1) AS preview \
         FROM chat_history c \
         WHERE c.role IN ('user', 'assistant') \
         GROUP BY c.subject \
         ORDER BY last_at DESC LIMIT ?",
    )
    .bind(SESSIONS_LIMIT)
    .fetch_all(&state.db)
    .await;
    if let Ok(rows) = rows {
        d.sessions = rows
            .into_iter()
            .map(|(subject, last_at, msgs, first_user, preview)| SessionRow {
                subject,
                last_at,
                msgs,
                first_user: first_user.unwrap_or_default(),
                preview: preview.unwrap_or_default(),
            })
            .collect();
    }

    // Materias reales para el selector: todos los asuntos con mensajes
    // (no solo los 12 de la lista de sesiones).
    if let Ok(rows) = sqlx::query_as::<_, (String,)>(
        "SELECT subject FROM chat_history \
         WHERE role IN ('user', 'assistant') AND TRIM(subject) <> '' \
         GROUP BY subject ORDER BY MAX(created_at) DESC LIMIT ?",
    )
    .bind(SUBJECTS_LIMIT)
    .fetch_all(&state.db)
    .await
    {
        d.subjects = rows.into_iter().map(|(subject,)| subject).collect();
    }

    // Materia activa: la que pide la query o, si no, la más reciente de
    // `chat_history`.
    let active: Option<String> = subject
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| d.sessions.first().map(|s| s.subject.clone()));

    // Historial real de la materia activa: los últimos turnos, en orden
    // cronológico.
    if let Some(subject) = active {
        d.active_subject = Some(subject.clone());
        let msgs = sqlx::query_as::<_, (String, String, String)>(
            "SELECT role, content, created_at FROM ( \
               SELECT id, role, content, created_at FROM chat_history \
                WHERE subject = ? AND role IN ('user', 'assistant') \
                ORDER BY created_at DESC, id DESC LIMIT ? \
             ) ORDER BY created_at ASC, id ASC",
        )
        .bind(&subject)
        .bind(HISTORY_TURNS_LIMIT as i64)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
        if !msgs.is_empty() {
            d.focus = Some((
                subject,
                msgs.into_iter()
                    .map(|(role, content, created_at)| ChatMsg {
                        role,
                        content,
                        created_at,
                    })
                    .collect(),
            ));
        }
    }

    // Gasto real del mes: consulta idéntica a la del endpoint de costos.
    if let Ok(sum) = crate::ai::openrouter::persistent_summary(&state.db, &state.config).await {
        d.spend = sum.total_cost;
        d.budget = sum.budget_limit;
        d.requests = sum.requests_count;
    }

    if let Some(client) = crate::academic::mikareno_client(state) {
        if tokio::time::timeout(
            std::time::Duration::from_secs(MCP_TIMEOUT_SECS),
            client.initialize(),
        )
        .await
        .is_ok()
        {
            d.connected = true;
        }
    }

    d
}

/// Reemplaza los marcadores `<!--SLOT:...-->` por HTML con datos reales.
pub fn fill_chat(content: &str, d: &ChatPanelData) -> String {
    let filled = content
        .replace("<!--SLOT:CHAT_FILTERS-->", &render_filters(d))
        .replace("<!--SLOT:CHAT_SESSIONS-->", &render_sessions(d))
        .replace("<!--SLOT:CHAT_MODE_DOT-->", &render_mode_dot(d))
        .replace("<!--SLOT:CHAT_MODEL-->", &render_model(d))
        .replace("<!--SLOT:CHAT_QUOTA_PCT-->", &render_quota_pct(d))
        .replace("<!--SLOT:CHAT_QUOTA_BAR-->", &render_quota_bar(d))
        .replace("<!--SLOT:CHAT_COSTS-->", &render_costs(d))
        .replace("<!--SLOT:CHAT_MODE-->", &render_mode(d))
        .replace("<!--SLOT:CHAT_CONTEXT-->", &render_context(d))
        .replace("<!--SLOT:CHAT_FCOUNT-->", &render_fcount())
        .replace("<!--SLOT:CHAT_MESSAGES-->", &render_messages(d))
        .replace("<!--SLOT:CHAT_SOURCES-->", &render_sources(d))
        .replace("<!--SLOT:CHAT_COMPOSER_MODE-->", &render_composer_mode(d));

    // El script de streaming va al final del contenido: así se ejecuta
    // después del `<script>` del kit (que ya registró sus listeners) y
    // puede interceptarlos en fase de captura.
    //
    // Concatenación (no `format!`): el script lleva llaves `{`/`}` de JS y
    // el HTML del kit puede llevarlas dentro de atributos o estilos.
    let script = render_stream_script(d);
    let mut out = String::with_capacity(filled.len() + script.len() + 1);
    out.push_str(&filled);
    out.push('\n');
    out.push_str(&script);
    out
}

/// Como [`fill_chat`], pero fijando la materia activa (la que llega en
/// `?subject=`). Si el asunto no es válido, se ignora y manda el que ya
/// traía el panel.
pub fn fill_chat_with_subject(content: &str, d: &ChatPanelData, subject: Option<&str>) -> String {
    let mut d = d.clone();
    if let Some(s) = normalize_subject(subject) {
        d.active_subject = Some(s);
    }
    fill_chat(content, &d)
}

/// GET /dashboard/ai -> asistente académico.
///
/// La materia activa se resuelve **en el servidor**: `?subject=<materia>`
/// precarga esa materia (opción activa del selector, `var SUBJECT` del script
/// e historial pintado). Sin el parámetro, la materia más reciente de
/// `chat_history`.
pub async fn page(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ChatQuery>,
    req: Request,
) -> impl IntoResponse {
    let subject = normalize_subject(query.subject.as_deref());
    let data = fetch_chat_data_for(&state, subject.as_deref()).await;
    let content = fill_chat_with_subject(CONTENT_ASISTENTE, &data, subject.as_deref());
    let authenticated = state.config.auth_disabled || extract_session_cookie(&req).is_some();
    let html = stitch_v2_page("asistente-academico", &content, authenticated);
    let shell = fetch_shell_data(&state).await;
    let html = fill_shell(&html, &shell);
    (StatusCode::OK, Html(html))
}

// ── render de slots ────────────────────────────────────────────────────────

/// Nombre del modelo configurado, tal cual llega a `state.config` (el kit lo
/// coloca tras el punto de modo en la telemetría).
fn render_model(d: &ChatPanelData) -> String {
    model_display(&d.model)
}

/// Punto + etiqueta de modo: esquina inferior del bloque de telemetría
/// (donde el kit mostraba "Conectado").
fn render_mode(d: &ChatPanelData) -> String {
    let (dot, text, label) = if d.api_configured {
        ("bg-secondary", "text-secondary", "API activa")
    } else {
        ("bg-outline", "text-outline", "Modo demo (sin API key)")
    };
    format!(
        r#"<span class="w-2 h-2 rounded-full {dot} animate-pulse"></span> <span class="{text}">{label}</span>"#
    )
}

/// Solo el punto de modo (va junto al nombre del modelo).
fn render_mode_dot(d: &ChatPanelData) -> String {
    let dot = if d.api_configured { "bg-secondary" } else { "bg-outline" };
    format!(r#"<span class="w-1.5 h-1.5 rounded-full {dot} animate-pulse"></span>"#)
}

/// Modo abreviado para el composer ("Modo: <esto>").
fn render_composer_mode(d: &ChatPanelData) -> String {
    if d.api_configured {
        "API activa".to_string()
    } else {
        "demo (sin API key)".to_string()
    }
}

/// Porcentaje del presupuesto mensual ya consumido (0-100).
fn quota_pct(d: &ChatPanelData) -> u32 {
    if d.budget > 0.0 {
        (d.spend / d.budget * 100.0).clamp(0.0, 100.0).round() as u32
    } else {
        0
    }
}

/// Etiqueta del porcentaje; sin presupuesto muestra un guion.
fn render_quota_pct(d: &ChatPanelData) -> String {
    if d.budget > 0.0 {
        format!("{}%", quota_pct(d))
    } else {
        "—".to_string()
    }
}

/// Barra de cuota con el ancho real del consumo mensual.
fn render_quota_bar(d: &ChatPanelData) -> String {
    format!(
        r#"<div class="bg-gradient-to-r from-secondary to-primary-container h-full rounded-full" style="width: {pct}%"></div>"#,
        pct = quota_pct(d)
    )
}

/// Gasto USD del mes (y tope si hay presupuesto); neutral cuando no consume.
fn render_costs(d: &ChatPanelData) -> String {
    if d.spend == 0.0 {
        return "Sin consumo este mes".to_string();
    }
    if d.budget > 0.0 {
        format!("${:.2} / ${:.2}", d.spend, d.budget)
    } else {
        format!("${:.2}", d.spend)
    }
}

/// Chips de contexto de la cabecera: materia activa y tamaño de la sesión
/// (los turnos que se están mostrando). Sin historial, solo la materia.
fn render_context(d: &ChatPanelData) -> String {
    let c = ui_classes();
    let Some(subject) = active_subject_opt(d) else {
        return r#"<span class="px-2 py-0.5 rounded font-label-sm text-label-sm bg-surface-container text-outline">Sin conversación activa</span>"#.to_string();
    };
    let chip = format!(
        r#"<span class="{subject_chip}">{subject}</span>"#,
        subject_chip = c.context_subject,
        subject = esc(subject),
    );
    match &d.focus {
        Some((_, msgs)) => format!(
            r#"{chip} <span class="text-outline text-body-sm">+</span> <span class="{count}">{n} mensajes</span>"#,
            chip = chip,
            count = c.context_count,
            n = msgs.len(),
        ),
        None => chip,
    }
}

/// Contador del botón "Fuentes": la vista todavía no sirve fuentes, 0 es la
/// cifra honesta (el panel derecho lo aclara).
fn render_fcount() -> String {
    "0".to_string()
}

/// Selector de materia: los asuntos reales de `chat_history` y la opción de
/// escribir uno nuevo (la que usa el script para crear una materia vacía).
///
/// La materia activa lleva `selected`, así que el selector queda marcado
/// aunque el script no llegue a ejecutarse.
fn render_filters(d: &ChatPanelData) -> String {
    let active = active_subject_opt(d).unwrap_or("");
    let mut opts = String::new();
    let mut seen: Vec<&str> = Vec::new();

    for s in d.subjects.iter() {
        if seen.contains(&s.as_str()) {
            continue;
        }
        seen.push(s.as_str());
        opts.push_str(&subject_option(s, s == active));
    }

    // La materia activa siempre está en la lista, aunque no tenga mensajes.
    if !active.is_empty() && !seen.contains(&active) {
        opts.push_str(&subject_option(active, true));
    }

    // Sin materias reales: solo el neutro "general" (no se inventa ninguna).
    if opts.is_empty() {
        opts.push_str(r#"<option value="general">general</option>"#);
    }

    format!(
        r#"<select id="chat-subject-select" title="Materia de la conversación" class="px-2 py-1 rounded-lg bg-surface-container text-on-surface font-label-sm text-label-sm outline-none focus:ring-1 focus:ring-primary-container max-w-[150px]">
{opts}
<option value="__new__" data-new="1">+ Nueva materia…</option>
</select>
<input id="chat-subject-new" type="text" maxlength="60" placeholder="Nombre de la materia" style="display: none;" class="px-2 py-1 rounded-lg bg-surface-container text-on-surface font-label-sm text-label-sm outline-none focus:ring-1 focus:ring-primary-container w-[130px]"/>"#,
        opts = opts,
    )
}

/// Una `<option>` del selector de materia; la activa lleva `selected`.
fn subject_option(value: &str, selected: bool) -> String {
    format!(
        r#"<option value="{value}"{selected}>{label}</option>"#,
        value = esc(value),
        selected = if selected { " selected" } else { "" },
        label = esc(value),
    )
}

/// Lista de conversaciones reales agrupadas por antigüedad (Hoy / Ayer /
/// Anteriores). La materia activa se marca como tal porque es la que muestra
/// el panel central. Cada tarjeta lleva su materia en `data-chat-subject`
/// (el script la usa para cambiar de conversación sin recargar).
fn render_sessions(d: &ChatPanelData) -> String {
    if d.sessions.is_empty() {
        return r#"<div class="p-space-sm rounded-lg bg-surface-container font-body-sm text-body-sm text-outline">Sin conversaciones. Envía un mensaje para empezar.</div>"#.to_string();
    }

    let today_iso = chrono::Utc::now().date_naive().to_string();
    let active = active_subject_opt(d);
    let mut out = String::new();
    let mut shown = String::new();
    for s in d.sessions.iter() {
        let (key, label) = session_bucket(&s.last_at, &today_iso);
        if key != shown.as_str() {
            let pad = if out.is_empty() { "pt-space-xs" } else { "pt-space-sm" };
            out.push_str(&format!(
                r#"<div class="px-space-sm {pad} pb-1 font-label-sm text-[10px] tracking-wider uppercase text-outline">{label}</div>"#,
                pad = pad,
                label = label,
            ));
            shown = key.to_string();
        }
        out.push_str(&session_card(active == Some(s.subject.as_str()), s));
    }
    out
}

/// Grupo temporal de una sesión según la fecha UTC: (clave, etiqueta).
fn session_bucket(last_at: &str, today_iso: &str) -> (&'static str, &'static str) {
    let date = last_at.get(0..10).unwrap_or("");
    if date == today_iso {
        ("hoy", "Hoy")
    } else if date == yesterday_iso(today_iso) {
        ("ayer", "Ayer")
    } else {
        ("anteriores", "Anteriores")
    }
}

fn yesterday_iso(today_iso: &str) -> String {
    chrono::NaiveDate::parse_from_str(today_iso, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.pred_opt())
        .map(|d| d.to_string())
        .unwrap_or_default()
}

/// Tarjeta de sesión con las clases exactas del kit. `active` aplica el estilo
/// destacado de la conversación abierta.
fn session_card(active: bool, s: &SessionRow) -> String {
    // Título = primera pregunta del usuario; si la sesión empieza sin user
    // (no debería), cae al subject. Preview = última respuesta del asistente.
    let title = if s.first_user.is_empty() {
        s.subject.clone()
    } else {
        truncate(&s.first_user, 60)
    };
    let preview = if s.preview.is_empty() {
        s.subject.clone()
    } else {
        truncate(&s.preview, 110)
    };
    let args = (
        esc(&title),
        esc(&time_label(&s.last_at)),
        esc(&preview),
        esc(&s.subject),
        s.msgs.max(0),
    );

    if active {
        format!(
            r##"<div class="relative p-space-sm rounded-lg bg-surface-container-high text-on-surface cursor-pointer group shadow-sm" data-chat-subject="{subject_attr}">
<div class="absolute left-0 top-2 bottom-2 w-1 bg-primary-container rounded-r-full"></div>
<div class="flex items-start justify-between gap-1 mb-1">
<span class="font-label-md text-label-md font-semibold truncate text-primary-fixed">{title}</span>
<span class="font-code-inline text-[10px] text-outline shrink-0">{time}</span>
</div>
<p class="font-body-sm text-body-sm text-on-surface-variant line-clamp-1 mb-1.5">{preview}</p>
<div class="flex items-center justify-between">
<span class="px-1.5 py-0.5 rounded text-[10px] font-label-sm bg-surface-container text-secondary truncate max-w-[120px]">{subject}</span>
<span class="font-code-inline text-[10px] text-outline flex items-center gap-0.5">
<span class="material-symbols-outlined text-[12px]">chat_bubble</span> {n}
</span>
</div>
</div>"##,
            subject_attr = args.3,
            title = args.0,
            time = args.1,
            preview = args.2,
            subject = args.3,
            n = args.4,
        )
    } else {
        format!(
            r##"<div class="p-space-sm rounded-lg bg-surface-container/50 hover:bg-surface-container text-on-surface-variant hover:text-on-surface cursor-pointer transition-colors group" data-chat-subject="{subject_attr}">
<div class="flex items-start justify-between gap-1 mb-1">
<span class="font-label-md text-label-md font-medium truncate">{title}</span>
<span class="font-code-inline text-[10px] text-outline shrink-0">{time}</span>
</div>
<p class="font-body-sm text-body-sm text-outline line-clamp-1 mb-1.5">{preview}</p>
<div class="flex items-center justify-between">
<span class="px-1.5 py-0.5 rounded text-[10px] font-label-sm bg-surface-container-low text-on-surface-variant truncate max-w-[120px]">{subject}</span>
<span class="font-code-inline text-[10px] text-outline flex items-center gap-0.5">
<span class="material-symbols-outlined text-[12px]">chat_bubble</span> {n}
</span>
</div>
</div>"##,
            subject_attr = args.3,
            title = args.0,
            time = args.1,
            preview = args.2,
            subject = args.3,
            n = args.4,
        )
    }
}

/// Panel central: el historial real de la materia activa (últimos turnos) o
/// el estado vacío honesto. El estudiante va a la derecha y el asistente a
/// la izquierda; las clases son las mismas que usa el script al replicar un
/// turno nuevo, así que un turno en vivo se ve idéntico a uno guardado.
fn render_messages(d: &ChatPanelData) -> String {
    let c = ui_classes();
    let Some((subject, msgs)) = &d.focus else {
        return empty_state();
    };

    let model = model_label(d);
    msgs.iter()
        .map(|m| {
            if m.role == "user" {
                format!(
                    r#"<div class="{row}" data-chat-turn="user">
<div class="{avatar}">TÚ</div>
<div class="{col}">
<div class="{head}">
<span class="{name}">Estudiante</span>
<span class="{time_cls}">{time}</span>
</div>
<div class="{bubble}">
<p class="{text}">{content}</p>
</div>
</div>
</div>"#,
                    row = c.user_row,
                    avatar = c.user_avatar,
                    col = c.user_col,
                    head = c.user_head,
                    name = c.user_name,
                    time_cls = c.time,
                    bubble = c.user_bubble,
                    text = c.user_text,
                    time = esc(&time_label(&m.created_at)),
                    content = esc(&m.content),
                )
            } else {
                format!(
                    r#"<div class="{row}" data-chat-turn="assistant">
<div class="{avatar}">
<span class="material-symbols-outlined text-[20px]">smart_toy</span>
</div>
<div class="{col}">
<div class="{head}">
<span class="{name}">{model}</span>
<span class="{chip}">{subject}</span>
<span class="{time_cls}">{time}</span>
</div>
<div class="{bubble}">
<p class="{text}">{content}</p>
</div>
</div>
</div>"#,
                    row = c.assistant_row,
                    avatar = c.assistant_avatar,
                    col = c.assistant_col,
                    head = c.assistant_head,
                    name = c.assistant_name,
                    chip = c.subject_chip,
                    time_cls = c.time,
                    bubble = c.assistant_bubble,
                    text = c.assistant_text,
                    model = esc(&model),
                    subject = esc(subject),
                    time = esc(&time_label(&m.created_at)),
                    content = esc(&m.content),
                )
            }
        })
        .collect()
}

/// Estado vacío honesto del panel central: no hay nada guardado todavía.
fn empty_state() -> String {
    r#"<div class="max-w-4xl mx-auto w-full text-center py-space-lg" data-chat-empty="1">
<div class="w-9 h-9 rounded-xl bg-primary-container/15 flex items-center justify-center mx-auto text-primary-container">
<span class="material-symbols-outlined text-[20px]">smart_toy</span>
</div>
<p class="font-label-md text-label-md font-semibold text-on-surface mt-2">Sin conversaciones</p>
<p class="font-body-sm text-body-sm text-outline mt-1">Envía un mensaje para empezar: el historial de esta materia aparecerá aquí.</p>
</div>"#
        .to_string()
}

/// Panel derecho: estado honesto (la app no conecta fuentes a esta vista aún)
/// con los valores reales de configuración.
fn render_sources(d: &ChatPanelData) -> String {
    let (mode_text, mode_class) = if d.api_configured {
        ("API activa", "text-secondary")
    } else {
        ("Modo demo (sin API key)", "text-outline")
    };
    let (mika_text, mika_class) = if d.connected {
        ("Conectado", "text-secondary")
    } else {
        ("Sin conexión", "text-outline")
    };
    format!(
        r#"<div class="p-space-sm rounded-lg bg-surface-container space-y-1">
<div class="flex items-center gap-2">
<span class="material-symbols-outlined text-[18px] text-outline">source</span>
<span class="font-label-md text-label-md font-semibold text-on-surface">Sin fuentes conectadas</span>
</div>
<p class="font-body-sm text-body-sm text-outline">El contexto de notas y material de Mikareno todavía no está conectado a esta vista.</p>
</div>
<div class="p-space-sm rounded-lg bg-surface-container space-y-space-xs">
<span class="font-label-sm text-[10px] uppercase tracking-wider text-outline block">Ajuste de Asistente</span>
<div class="flex items-center justify-between text-body-sm">
<span class="text-on-surface">Modelo</span>
<span class="font-code-inline text-primary-container font-medium">{model}</span>
</div>
<div class="flex items-center justify-between text-body-sm">
<span class="text-on-surface">Estado</span>
<span class="font-code-inline {mode_class} font-medium">{mode_text}</span>
</div>
<div class="flex items-center justify-between text-body-sm">
<span class="text-on-surface">Mikareno</span>
<span class="font-code-inline {mika_class} font-medium">{mika_text}</span>
</div>
</div>"#,
        model = model_display(&d.model),
        mode_class = mode_class,
        mode_text = mode_text,
        mika_class = mika_class,
        mika_text = mika_text,
    )
}

// ── script de streaming ────────────────────────────────────────────────────

/// Marcadores dentro de [`STREAM_SCRIPT`]: cada uno ocupa el valor entero,
/// de modo que la sustitución no puede duplicarlo ni dejar un literal
/// pegado al que ya traía la plantilla.
const SUBJECT_MARKER: &str = "__SUBJECT__";
const CLASSES_MARKER: &str = "__CLASSES__";
const LIMIT_MARKER: &str = "__LIMIT__";
const MODEL_MARKER: &str = "__MODEL__";

/// Envuelve [`STREAM_SCRIPT`] en su etiqueta `<script>` y fija el asunto
/// activo (el de la conversación enfocada; "general" si no hay ninguna).
///
/// El asunto y el modelo viajan como literal JSON (`serde_json::to_string`):
/// una cadena entre comillas con los escapes correctos, insertada **una sola
/// vez** en el marcador. El script se concatena (no con `format!`) para que las
/// llaves `{`/`}` del JS no se interpreten como placeholders de Rust.
fn render_stream_script(d: &ChatPanelData) -> String {
    let subject = active_subject(d);
    let model = model_label(d);
    let classes = serde_json::to_string(&ui_classes()).unwrap_or_else(|_| "{}".to_string());

    let js = STREAM_SCRIPT
        .replace(SUBJECT_MARKER, &js_literal(subject))
        .replace(CLASSES_MARKER, &classes)
        .replace(LIMIT_MARKER, &HISTORY_TURNS_LIMIT.to_string())
        .replace(MODEL_MARKER, &js_literal(&model));

    let mut out = String::with_capacity(js.len() + 24);
    out.push_str("<script>\n");
    out.push_str(&js);
    out.push_str("\n</script>");
    out
}

/// Literal JS de una cadena: JSON válido (entre comillas) y sin `<` literal,
/// para que un valor con `</script>` no pueda cerrar la etiqueta `<script>`
/// en la que vive el script.
fn js_literal(value: &str) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "\"general\"".to_string())
        .replace('<', "\\u003C")
}

// ── helpers ────────────────────────────────────────────────────────────────

/// Nombre visible del modelo: el configurado, o un neutro si no hay.
fn model_label(d: &ChatPanelData) -> String {
    if d.model.is_empty() {
        "Asistente".to_string()
    } else {
        d.model.clone()
    }
}

/// Mismo texto escapado para cabeceras/etiquetas.
fn model_display(model: &str) -> String {
    if model.is_empty() {
        "sin modelo".to_string()
    } else {
        esc(model)
    }
}

/// Etiqueta amigable del UTC de sqlite: hoy → "HH:MM"; ayer → "Ayer";
/// antes → "DD de mes". Si no parsea, devuelve el texto tal cual.
fn time_label(created_at: &str) -> String {
    let Ok(dt) = chrono::NaiveDateTime::parse_from_str(created_at.trim(), "%Y-%m-%d %H:%M:%S")
    else {
        return created_at.trim().to_string();
    };
    let today = chrono::Utc::now().date_naive();
    let date = dt.date();
    if date == today {
        dt.format("%H:%M").to_string()
    } else if date == today - chrono::Duration::days(1) {
        "Ayer".to_string()
    } else {
        format!("{} de {}", date.day(), month_abbr_es(date.month()))
    }
}

/// Abreviaturas de mes en español (para "22 de oct").
fn month_abbr_es(m: u32) -> &'static str {
    match m {
        1 => "ene",
        2 => "feb",
        3 => "mar",
        4 => "abr",
        5 => "may",
        6 => "jun",
        7 => "jul",
        8 => "ago",
        9 => "sep",
        10 => "oct",
        11 => "nov",
        _ => "dic",
    }
}

/// Recorta por caracteres (no bytes) y añade elipsis si cortó. Escapar arriba.
fn truncate(s: &str, max: usize) -> String {
    let trimmed = s.trim();
    let mut out: String = trimmed.chars().take(max).collect();
    if trimmed.chars().count() > max {
        out.push('…');
    }
    out
}

fn esc(s: &str) -> String {
    crate::academic::templates::html_escape(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datos(subject: &str) -> ChatPanelData {
        ChatPanelData {
            focus: Some((subject.to_string(), Vec::new())),
            ..Default::default()
        }
    }

    /// Línea donde el script asigna el asunto.
    fn linea_subject(script: &str) -> String {
        script
            .lines()
            .find(|l| l.contains("var SUBJECT"))
            .expect("el script inyecta el asunto")
            .trim()
            .to_string()
    }

    #[test]
    fn el_asunto_se_inyecta_una_sola_vez_como_literal_valido() {
        let script = render_stream_script(&datos("Cálculo"));

        // Ningún marcador se queda sin sustituir.
        assert!(
            !script.contains(SUBJECT_MARKER),
            "queda el marcador del asunto"
        );
        assert!(!script.contains(CLASSES_MARKER), "queda el marcador de clases");
        assert!(!script.contains(LIMIT_MARKER), "queda el marcador del límite");
        assert!(!script.contains(MODEL_MARKER), "queda el marcador del modelo");
        assert!(!script.contains("__CFG__"), "queda un marcador ajeno");
        assert!(!script.contains("/*__"), "queda un marcador en comentario");

        // Un único literal, no dos pegados (`"general""general"` rompía el JS).
        let linea = linea_subject(&script);
        assert_eq!(linea, r#"var SUBJECT = "Cálculo";"#);
        assert!(!linea.contains("\"\""));
        assert_eq!(linea.matches('"').count(), 2);

        // El valor se inserta una sola vez en todo el script.
        assert_eq!(script.matches("Cálculo").count(), 1);

        // Balance de llaves y paréntesis del script inyectado.
        assert_eq!(script.matches('{').count(), script.matches('}').count());
        assert_eq!(script.matches('(').count(), script.matches(')').count());
    }

    #[test]
    fn el_asunto_se_escapa_y_no_rompe_el_script() {
        let subject = "Matemáticas \"Cálculo\"\\x</script>";
        let script = render_stream_script(&datos(subject));
        let linea = linea_subject(&script);

        // El literal inyectado es JSON válido y round-trippea al asunto real.
        let lit = linea
            .trim_start_matches("var SUBJECT = ")
            .trim_end_matches(';');
        let v: String = serde_json::from_str(lit).expect("literal JSON válido");
        assert_eq!(v, subject);

        // Sin `<` literal: el asunto no puede cerrar el <script>.
        assert!(!lit.contains('<'), "el literal puede cerrar el <script>");
        assert_eq!(script.matches("</script>").count(), 1);
    }

    #[test]
    fn sin_conversacion_el_asunto_por_defecto_es_general() {
        let script = render_stream_script(&ChatPanelData::default());
        assert_eq!(linea_subject(&script), r#"var SUBJECT = "general";"#);
    }

    #[test]
    fn el_script_lleva_el_limite_de_turnos_y_las_clases() {
        let script = render_stream_script(&datos("Cálculo"));
        assert!(
            script.contains(&format!("var HISTORY_LIMIT = {HISTORY_TURNS_LIMIT};")),
            "el script no fija el límite de turnos"
        );
        // Las clases llegan como literal JSON con las claves que usa el JS.
        assert!(script.contains("\"user_row\""));
        assert!(script.contains("\"assistant_row\""));
    }

    #[test]
    fn el_historial_distingue_el_rol_del_estudiante_y_del_asistente() {
        let d = ChatPanelData {
            focus: Some((
                "Cálculo".to_string(),
                vec![
                    ChatMsg {
                        role: "user".to_string(),
                        content: "¿Qué es un límite?".to_string(),
                        created_at: "2026-10-19 10:00:00".to_string(),
                    },
                    ChatMsg {
                        role: "assistant".to_string(),
                        content: "Es un valor de aproximación.".to_string(),
                        created_at: "2026-10-19 10:00:05".to_string(),
                    },
                ],
            )),
            ..Default::default()
        };
        let html = render_messages(&d);

        // El estudiante a la derecha, el asistente a la izquierda.
        assert!(html.contains("data-chat-turn=\"user\""));
        assert!(html.contains("data-chat-turn=\"assistant\""));
        assert!(html.contains("flex-row-reverse"));
        assert!(html.contains("¿Qué es un límite?"));
        assert!(html.contains("Es un valor de aproximación."));
        // El texto llega escapado.
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn sin_historial_el_estado_vacio_es_honesto() {
        let html = render_messages(&ChatPanelData::default());
        assert!(html.contains("Sin conversaciones"));
        assert!(html.contains("Envía un mensaje para empezar"));
    }

    #[test]
    fn el_selector_ofrece_las_materias_reales_y_la_opcion_de_nueva() {
        let d = ChatPanelData {
            subjects: vec!["Cálculo".to_string(), "Biología".to_string()],
            focus: Some(("Cálculo".to_string(), Vec::new())),
            ..Default::default()
        };
        let html = render_filters(&d);
        assert!(html.contains("id=\"chat-subject-select\""));
        // La materia activa va marcada; las demás, no.
        assert!(html.contains("<option value=\"Cálculo\" selected>Cálculo</option>"));
        assert!(html.contains("<option value=\"Biología\">Biología</option>"));
        // La opción de escribir una materia nueva.
        assert!(html.contains("data-new=\"1\""));
        assert!(html.contains("id=\"chat-subject-new\""));
    }

    // ── materia resuelta en el servidor (`?subject=`) ──────────────────────

    /// Datos con la materia activa fijada por el servidor.
    fn datos_con_materia(subject: &str) -> ChatPanelData {
        ChatPanelData {
            active_subject: Some(subject.to_string()),
            focus: Some((subject.to_string(), Vec::new())),
            ..Default::default()
        }
    }

    /// La query tal como la ve axum.
    fn query_de(uri: &str) -> Result<Option<String>, axum::extract::rejection::QueryRejection> {
        let uri: axum::http::Uri = uri.parse().expect("uri válida");
        axum::extract::Query::<ChatQuery>::try_from_uri(&uri).map(|q| q.0.subject)
    }

    #[test]
    fn la_materia_de_la_query_marca_la_opcion_activa_del_selector() {
        let d = ChatPanelData {
            subjects: vec!["Cálculo".to_string(), "Biología".to_string()],
            active_subject: Some("Biología".to_string()),
            ..Default::default()
        };
        let html = render_filters(&d);
        assert!(html.contains("<option value=\"Biología\" selected>Biología</option>"));
        assert!(html.contains("<option value=\"Cálculo\">Cálculo</option>"));
    }

    #[test]
    fn la_materia_pedida_entra_en_el_selector_aunque_no_tenga_historial() {
        let d = ChatPanelData {
            subjects: vec!["Cálculo".to_string()],
            active_subject: Some("Física".to_string()),
            ..Default::default()
        };
        let html = render_filters(&d);
        assert!(html.contains("<option value=\"Física\" selected>Física</option>"));
    }

    #[test]
    fn sin_materias_en_la_bd_el_selector_solo_ofrece_general() {
        let html = render_filters(&ChatPanelData::default());
        assert!(html.contains("<option value=\"general\">general</option>"));
        // "general" + "+ Nueva materia…": no se inventa ninguna materia.
        assert_eq!(html.matches("<option").count(), 2);
    }

    #[test]
    fn el_subject_del_script_es_el_de_la_query_y_sigue_siendo_json_valido() {
        let script = render_stream_script(&datos_con_materia("Física"));
        let linea = linea_subject(&script);
        assert_eq!(linea, r#"var SUBJECT = "Física";"#);

        // El literal sigue siendo JSON válido (el fix del marcador se mantiene).
        let lit = linea
            .trim_start_matches("var SUBJECT = ")
            .trim_end_matches(';');
        let v: String = serde_json::from_str(lit).expect("literal JSON válido");
        assert_eq!(v, "Física");
        assert!(!lit.contains('<'));
    }

    #[test]
    fn la_materia_se_normaliza() {
        assert_eq!(normalize_subject(None), None);
        assert_eq!(normalize_subject(Some("   ")), None);
        assert_eq!(normalize_subject(Some("  Cálculo  ")).as_deref(), Some("Cálculo"));
        // Los caracteres de control no pueden colarse.
        assert_eq!(normalize_subject(Some("Cálculo\n")).as_deref(), Some("Cálculo"));
        // La longitud está acotada.
        let largo = "x".repeat(500);
        assert_eq!(normalize_subject(Some(&largo)).unwrap().len(), MAX_SUBJECT_LEN);
    }

    #[test]
    fn la_query_se_lee_sin_tumbar_la_pagina() {
        // La materia llega percent-encoded.
        assert_eq!(
            query_de("/dashboard/ai?subject=C%C3%A1lculo")
                .unwrap()
                .as_deref(),
            Some("Cálculo")
        );
        // Sin query no hay materia.
        assert_eq!(query_de("/dashboard/ai").unwrap(), None);
        // Una query vacía se normaliza a "no hay materia".
        assert!(normalize_subject(query_de("/dashboard/ai?subject=").unwrap().as_deref()).is_none());
        // Una query rara no se rechaza (no hay 400).
        assert!(query_de("/dashboard/ai?subject[]=x").is_ok());
        assert!(query_de("/dashboard/ai?otro=1").is_ok());
        assert!(query_de("/dashboard/ai?subject=a&subject=b").is_ok());
    }

    /// Estado de prueba: BD en memoria con `chat_history` y dos materias
    /// ("Cálculo" más antigua que "Biología").
    async fn estado_de_prueba() -> AppState {
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("BD en memoria");
        sqlx::query(
            "CREATE TABLE chat_history ( \
               id INTEGER PRIMARY KEY AUTOINCREMENT, \
               subject TEXT NOT NULL, \
               role TEXT NOT NULL, \
               content TEXT NOT NULL, \
               created_at TEXT NOT NULL DEFAULT (datetime('now')))",
        )
        .execute(&db)
        .await
        .expect("tabla chat_history");
        for (subject, role, content, created_at) in [
            (
                "Cálculo",
                "user",
                "¿Qué es un límite?",
                "2026-10-18 10:00:00",
            ),
            (
                "Cálculo",
                "assistant",
                "Un valor de aproximación.",
                "2026-10-18 10:00:05",
            ),
            (
                "Biología",
                "user",
                "¿Qué es la mitosis?",
                "2026-10-19 10:00:00",
            ),
            (
                "Biología",
                "assistant",
                "La división celular.",
                "2026-10-19 10:00:05",
            ),
        ] {
            sqlx::query(
                "INSERT INTO chat_history (subject, role, content, created_at) \
                 VALUES (?, ?, ?, ?)",
            )
            .bind(subject)
            .bind(role)
            .bind(content)
            .bind(created_at)
            .execute(&db)
            .await
            .expect("mensaje insertado");
        }

        AppState {
            config: crate::config::Config {
                host: "127.0.0.1".to_string(),
                port: 1,
                session_secret: "test_secret_key_at_least_32_chars_long!!".to_string(),
                db_path: std::path::PathBuf::from(":memory:"),
                notes_dir: std::path::PathBuf::from("/tmp"),
                openrouter_api_key: None,
                openrouter_model: "test/model".to_string(),
                openrouter_max_tokens: 100,
                openrouter_max_cost_usd: 2.0,
                mikareno_mcp_url: String::new(),
                genoffice_mcp_url: String::new(),
                custom_mcp_api_key: None,
                rust_log: "test".to_string(),
                auth_disabled: true,
                registration_token: None,
                db_max_connections: 4,
                mcp_timeout_ms: 5_000,
            },
            db,
            mikareno: None,
            start_time: std::time::Instant::now(),
        }
    }

    #[tokio::test]
    async fn el_historial_pintado_es_el_de_la_materia_pedida() {
        let state = estado_de_prueba().await;
        let d = fetch_chat_data_for(&state, Some("Cálculo")).await;

        assert_eq!(d.active_subject.as_deref(), Some("Cálculo"));
        // La sesión más reciente es otra: la materia pedida manda.
        assert_eq!(d.sessions.first().map(|s| s.subject.as_str()), Some("Biología"));
        let (subject, msgs) = d.focus.expect("historial de la materia pedida");
        assert_eq!(subject, "Cálculo");
        assert!(msgs.iter().any(|m| m.content == "¿Qué es un límite?"));
        assert!(!msgs.iter().any(|m| m.content == "¿Qué es la mitosis?"));
    }

    #[tokio::test]
    async fn sin_query_manda_la_materia_mas_reciente() {
        let state = estado_de_prueba().await;
        let d = fetch_chat_data_for(&state, None).await;

        assert_eq!(d.active_subject.as_deref(), Some("Biología"));
        let (subject, _) = d.focus.expect("historial de la materia más reciente");
        assert_eq!(subject, "Biología");
    }

    #[tokio::test]
    async fn la_materia_pedida_sin_historial_deja_el_panel_vacio_pero_seleccionada() {
        let state = estado_de_prueba().await;
        let d = fetch_chat_data_for(&state, Some("Física")).await;

        assert_eq!(d.active_subject.as_deref(), Some("Física"));
        assert!(d.focus.is_none(), "no hay historial: el panel queda vacío");
        // Pero la materia activa es la pedida y el selector la marca.
        assert_eq!(active_subject(&d), "Física");
        let html = render_filters(&d);
        assert!(html.contains("<option value=\"Física\" selected>Física</option>"));
        assert_eq!(linea_subject(&render_stream_script(&d)), r#"var SUBJECT = "Física";"#);
    }
}
