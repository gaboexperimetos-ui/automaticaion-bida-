/*! Study Platform shell.js — capa de interactividad real del shell Stitch v2.
 *
 * Servida en /shell.js; `stitch_v2_page` la inyecta al final del <body>.
 * Sin dependencias externas.
 *
 * Qué arregla:
 *   - toasts, diálogos y estados de carga para las interacciones reales.
 *   - buscador del topbar en vivo contra /api/search (Ctrl/Cmd+K).
 *   - paleta de comandos (⌘P / botón "Comandos") en la página de notas.
 *   - campana del topbar: accesos reales (agenda/sistema).
 *   - botón "Crear" del topbar: crea notas/documentos/hojas/presentaciones
 *     via los endpoints POST reales del backend.
 *   - dashboard: cada item del menu "+ Crear" del kit lleva a donde el
 *     feature existe de verdad (o lo crea via API); el modal "Nueva tarea"
 *     guarda una nota (captura rapida) o abre el planificador: las tareas
 *     del curso viven en Mikareno, que la app lee pero no escribe.
 *   - notas: editor REAL (textarea con el contenido verbatim de
 *     /api/notes/{slug}, preview markdown en vivo, modos funcionales,
 *     toolbar activa, Ctrl+S con PUT a la API, "+ tag", zoom, exportar).
 *   - asistente IA: chips que despachan prompts reales al stream script del
 *     kit, adjuntar nota como contexto (sessionStorage) y dictado por voz.
 *
 * No toca el HTML servido: se aumenta el DOM ya montado. Si un elemento no
 * existe (paginas legacy), todo es no-op.
 */
(function () {
  "use strict";

  if (window.__SP__) { return; }

  var SP = window.__SP__ = {};
  SP.version = "3.0.0";

  var CSS = [
    ":root{--sp-ease:cubic-bezier(.22,1,.36,1);--sp-fast:140ms}",
    "button,input,select,textarea,summary,a{transition:background-color var(--sp-fast) var(--sp-ease),color var(--sp-fast) var(--sp-ease),border-color var(--sp-fast) var(--sp-ease),transform var(--sp-fast) var(--sp-ease),box-shadow var(--sp-fast) var(--sp-ease)}",
    "#app-topbar button:active,#app-rail button:active,#contenido button:active{transform:scale(.98)}",
    "#toast-region{position:fixed;bottom:1rem;right:1rem;z-index:120;display:flex;flex-direction:column;gap:.5rem;max-width:min(26rem,92vw)}",
    ".sp-toast{position:static;display:flex;align-items:flex-start;gap:.55rem;padding:.75rem .85rem;border-radius:.75rem;background:var(--sp-surface-high);color:var(--sp-text);border:1px solid var(--sp-border);box-shadow:var(--sp-shadow);font:500 .8125rem/1.4 'DM Sans',sans-serif}",
    ".sp-toast .material-symbols-outlined{font-size:18px;flex:none;align-self:center}.sp-toast.ok .material-symbols-outlined{color:var(--sp-lime)}.sp-toast.err{border-color:var(--sp-error)}.sp-toast.err .material-symbols-outlined{color:var(--sp-error)}.sp-toast.info .material-symbols-outlined{color:var(--sp-teal)}",
    ".sp-toast button{flex:none;padding:.1rem;border:0;background:transparent;color:var(--sp-text-muted);cursor:pointer;border-radius:.3rem}.sp-toast button:hover{color:var(--sp-text)}",
    ".sp-menu{position:absolute;top:calc(100% + .4rem);right:0;min-width:15rem;width:max-content;max-width:calc(100vw - 24px);background:var(--sp-surface-raised);border:1px solid var(--sp-border);border-radius:.8rem;padding:.4rem;box-shadow:var(--sp-shadow);z-index:75}",
    ".sp-menu a,.sp-menu button{width:100%;display:flex;align-items:center;gap:.55rem;padding:.55rem .65rem;border-radius:.5rem;border:0;background:transparent;color:var(--sp-text);text-align:left;font:500 .8125rem/1.35 'DM Sans',sans-serif;cursor:pointer;text-decoration:none}",
    ".sp-menu a:hover,.sp-menu button:hover{background:var(--sp-teal-soft)}",
    ".sp-menu .sp-menu-head{display:block;padding:.4rem .65rem .55rem;font-size:.66rem;letter-spacing:.09em;text-transform:uppercase;color:var(--sp-text-muted);border-bottom:1px solid var(--sp-border-soft);margin-bottom:.25rem}",
    ".sp-menu .sp-sep{height:1px;background:var(--sp-border-soft);margin:.3rem .2rem}.sp-menu .material-symbols-outlined{font-size:18px;color:var(--sp-teal)}.sp-menu .sp-sub{display:block;font-size:.65rem;color:var(--sp-text-muted);margin-top:.1rem}",
    ".sp-search-panel{position:absolute;top:calc(100% + .4rem);left:0;right:0;width:auto;background:var(--sp-surface-raised);border:1px solid var(--sp-border);border-radius:.8rem;max-height:55vh;overflow:auto;padding:.4rem;box-shadow:var(--sp-shadow);z-index:75}",
    ".sp-search-panel a{display:block;padding:.55rem .6rem;border-radius:.5rem;text-decoration:none}.sp-search-panel a:hover,.sp-search-panel a[aria-selected='true']{background:var(--sp-teal-soft)}",
    ".sp-search-panel .sp-hit-title{font:600 .8125rem/1.35 'DM Sans',sans-serif;color:var(--sp-text)}.sp-search-panel .sp-hit-tags{font:400 .6875rem/1.4 'DM Sans',sans-serif;color:var(--sp-text-muted);margin-top:.15rem}",
    ".sp-veil{position:fixed;inset:0;background:rgba(3,5,10,.76);backdrop-filter:blur(7px);z-index:110;display:flex;align-items:center;justify-content:center;padding:1rem}",
    ".sp-box{width:100%;max-width:26rem;max-height:80vh;overflow:auto;background:var(--sp-surface-raised);border:1px solid var(--sp-border);border-radius:1rem;padding:1.25rem;box-shadow:var(--sp-shadow)}",
    ".sp-box h2{margin:0 0 .3rem;font:600 1.1rem/1.25 'Space Grotesk',sans-serif;color:var(--sp-text)}.sp-box p{margin:.25rem 0 .75rem;font:400 .8125rem/1.55 'DM Sans',sans-serif;color:var(--sp-text-secondary)}",
    ".sp-box input[type=text]{width:100%;padding:.6rem .7rem;border-radius:.6rem;border:1px solid var(--sp-border);background:var(--sp-bg);color:var(--sp-text);font:500 .875rem/1.45 'DM Sans',sans-serif}.sp-box input:focus{outline:2px solid var(--sp-teal);outline-offset:1px}",
    ".sp-box .sp-menu-item{display:flex;align-items:center;gap:.5rem;width:100%;padding:.55rem .6rem;border-radius:.5rem;border:0;background:transparent;color:var(--sp-text);text-align:left;cursor:pointer;font:500 .8125rem/1.3 'DM Sans',sans-serif}.sp-box .sp-menu-item:hover{background:var(--sp-teal-soft)}",
    ".sp-actions{display:flex;justify-content:flex-end;gap:.5rem;margin-top:1rem}.sp-btn{padding:.55rem .85rem;border-radius:.6rem;border:0;cursor:pointer;font:600 .8125rem/1 'DM Sans',sans-serif}",
    ".sp-btn.primary{background:var(--sp-teal);color:#121728}.sp-btn.primary:hover{background:#c2cbff}.sp-btn.ghost{background:var(--sp-surface-high);color:var(--sp-text);border:1px solid var(--sp-border)}.sp-btn.ghost:hover{background:var(--sp-surface-highest)}",
    "[data-sp-loading]{opacity:.65;pointer-events:none}[data-sp-loading] .material-symbols-outlined{animation:sp-rot 900ms linear infinite}@keyframes sp-rot{to{transform:rotate(360deg)}}",
    "#note-editor{width:100%;min-height:60vh;height:100%;background:transparent;border:0;color:var(--sp-text);resize:vertical;padding:1rem;font:400 .875rem/1.75 ui-monospace,SFMono-Regular,Menlo,monospace}#note-editor:focus{outline:2px solid var(--sp-teal);outline-offset:-2px}",
    "#note-preview{color:var(--sp-text-secondary);font-size:.9rem;line-height:1.75}#note-preview h1,#note-preview h2,#note-preview h3{color:var(--sp-text);font-family:'Space Grotesk',sans-serif;letter-spacing:-.035em}",
    "#note-preview h1{font-size:1.8rem;margin:.25em 0 .65em}#note-preview h2{font-size:1.35rem;margin:1.2em 0 .55em}#note-preview h3{font-size:1.1rem;margin:1em 0 .45em}#note-preview p{margin:.65em 0}",
    "#note-preview pre{background:var(--sp-bg);border:1px solid var(--sp-border);border-radius:.65rem;padding:.85rem;overflow:auto;font-size:.8125rem}#note-preview blockquote{border-left:2px solid var(--sp-teal);margin:1em 0;padding:.5em .9em;color:var(--sp-text-secondary);background:var(--sp-teal-soft)}",
    "#note-preview a{color:var(--sp-teal);text-decoration:underline}#note-preview ul,#note-preview ol{margin:.65em 0;padding-left:1.4em}#note-preview li{margin:.3em 0}#note-preview li::marker{color:var(--sp-teal)}",
    "@media print{body.sp-printing>*{display:none!important}.sp-veil{display:none!important}#note-preview{display:block!important;position:absolute;inset:0;z-index:999;padding:0;background:#fff;color:#000}}body.sp-printing #note-preview *{background:#fff!important;color:#000!important}",
    ".sp-coleccion-bar{display:flex;justify-content:flex-end;padding:.35rem .5rem .15rem}#sp-coleccion{max-width:16rem;background:var(--sp-surface-raised);color:var(--sp-text);border:1px solid var(--sp-border);border-radius:.55rem;padding:.35rem .5rem;font:500 .75rem/1.2 'DM Sans',sans-serif;cursor:pointer}#sp-coleccion[data-vista]{border-color:var(--sp-teal);color:var(--sp-teal)}",
    ".sp-banner{display:flex;align-items:center;gap:.5rem;margin:.8rem;padding:.75rem .9rem;border-radius:.65rem;background:var(--sp-surface-raised);border:1px solid var(--sp-border);color:var(--sp-text-secondary);font:500 .8125rem/1.4 'DM Sans',sans-serif}.sp-html{display:grid;gap:.55rem}",
    ".sp-field{display:grid;gap:.3rem}.sp-field-label{font:600 .7rem/1.2 'DM Sans',sans-serif;color:var(--sp-text-secondary);text-transform:uppercase;letter-spacing:.05em}.sp-input{width:100%;background:var(--sp-bg);border:1px solid var(--sp-border);border-radius:.55rem;padding:.55rem .65rem;color:var(--sp-text);font:400 .8125rem/1.4 'DM Sans',sans-serif}.sp-input:focus{outline:2px solid var(--sp-teal);outline-offset:-1px}",
    ".sp-anot-lista{display:grid;gap:.35rem}.sp-link{color:var(--sp-teal);font-weight:600;text-decoration:underline;white-space:nowrap}[data-rec='1']{color:var(--sp-lime)!important;animation:sp-pulse 1.2s ease-in-out infinite}@keyframes sp-pulse{50%{opacity:.55}}"
  ].join("\n");

  var styleInjected = false;
  function injectCss() {
    if (styleInjected) { return; }
    var s = document.createElement("style");
    s.id = "sp-style";
    s.textContent = CSS;
    document.head.appendChild(s);
    styleInjected = true;
  }

  function el(tag, cls, txt) {
    var n = document.createElement(tag);
    if (cls) { n.className = cls; }
    if (txt !== undefined && txt !== null) { n.textContent = txt; }
    return n;
  }
  function icon(name) {
    var s = el("span", "material-symbols-outlined", name);
    s.setAttribute("aria-hidden", "true");
    return s;
  }
  function txt(elx) { return ((elx && elx.textContent) || "").replace(/\s+/g, " ").trim(); }
  function once(elx, key, fn) {
    if (!elx || elx.dataset[key]) { return; }
    elx.dataset[key] = "1";
    fn(elx);
  }
  function track(btn, on) {
    if (!btn) { return; }
    if (on) { btn.setAttribute("data-sp-loading", "1"); btn.setAttribute("aria-busy", "true"); }
    else { btn.removeAttribute("data-sp-loading"); btn.removeAttribute("aria-busy"); }
  }
  function msgErr(err) {
    var message = String((err && err.message) || "Error inesperado");
    var normalized = message.trim().toLowerCase();
    var common = {
      "title is required": "El título es obligatorio.",
      "invalid title": "El título no es válido.",
      "not found": "No se encontró el elemento solicitado.",
      "unauthorized": "La sesión ha caducado. Inicia sesión de nuevo.",
      "forbidden": "No tienes permiso para realizar esta acción.",
      "failed to fetch": "No se pudo conectar con el servidor.",
      "internal server error": "El servidor encontró un error interno.",
      "bad request": "La solicitud no es válida."
    };
    if (common[normalized]) { return common[normalized]; }
    if (/^5\d{2}\b/.test(normalized)) {
      return "El servidor devolvió un error (" + message + ").";
    }
    return message;
  }
  function selText(sel) {
    var o = sel && sel.options && sel.options[sel.selectedIndex];
    return o ? txt(o) : "";
  }
  function sluginator(s) {
    return String(s).toLowerCase().normalize("NFKD").replace(/[\u0300-\u036f]/g, "")
      .replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "") || "notepad";
  }

  SP.api = function (path, opts) {
    opts = opts || {};
    var init = { method: opts.method || "GET", headers: {} };
    if (opts.json !== undefined) {
      init.headers["content-type"] = "application/json";
      init.body = JSON.stringify(opts.json);
    } else if (opts.form) {
      init.method = "POST";
      var fd = new FormData();
      Object.keys(opts.form).forEach(function (k) { fd.append(k, opts.form[k]); });
      init.body = fd;
    }
    return fetch(path, init).then(function (r) {
      return r.json().catch(function () { return {}; }).then(function (data) {
        if (!r.ok) {
          var m = (data && (data.error || data.message)) || r.status + " " + (r.statusText || "");
          throw { status: r.status, data: data, message: m };
        }
        return data;
      });
    });
  };

  /* ------------------------------- toasts ------------------------------ */
  var toastRegion = null;
  SP.toast = function (msg, kind, ms) {
    injectCss();
    if (!toastRegion || !toastRegion.isConnected) {
      toastRegion = el("div");
      toastRegion.id = "toast-region";
      document.body.appendChild(toastRegion);
    }
    kind = kind || "info";
    var t = el("div", "sp-toast " + kind);
    t.setAttribute("role", "status");
    t.appendChild(icon(kind === "ok" ? "check_circle" : kind === "err" ? "error" : "info"));
    t.appendChild(el("span", null, String(msg)));
    var close = el("button");
    close.appendChild(icon("close"));
    close.setAttribute("aria-label", "Cerrar aviso");
    t.appendChild(close);
    var kill = function () {
      t.remove();
      if (toastRegion && !toastRegion.childElementCount) { toastRegion.remove(); toastRegion = null; }
    };
    close.addEventListener("click", kill);
    toastRegion.appendChild(t);
    setTimeout(kill, ms || (kind === "err" ? 6000 : 3200));
  };

  /* ------------------------------ dialogos ------------------------------ */
  /* kind "field": input + Captionar/Cancelar (con Enter = aceptar)
     kind "items": lista de acciones (paleta, adjuntar nota)
     kind "info":  solo mensaje + Aceptar                                */
  SP.dialog = function (opts) {
    opts = opts || {};
    var kind = opts.items && opts.items.length ? "items" : opts.field ? "field" : "info";
    return new Promise(function (resolve) {
      injectCss();
      var veil = el("div", "sp-veil");
      veil.setAttribute("role", "dialog");
      veil.setAttribute("aria-modal", "true");
      veil.setAttribute("aria-label", opts.title || "Diálogo");
      var box = el("div", "sp-box");
      veil.appendChild(box);
      if (opts.title) { box.appendChild(el("h2", null, opts.title)); }
      if (opts.text) { box.appendChild(el("p", null, opts.text)); }

      function done(v) {
        veil.remove();
        document.removeEventListener("keydown", onKey, true);
        resolve(v);
      }
      function onKey(e) {
        if (e.key === "Escape") { e.stopPropagation(); done(null); }
        if (e.key === "Tab") {
          var f = box.querySelectorAll("button, input, a");
          if (!f.length) { return; }
          var first2 = f[0], last2 = f[f.length - 1];
          if (e.shiftKey && document.activeElement === first2) { e.preventDefault(); last2.focus(); }
          else if (!e.shiftKey && document.activeElement === last2) { e.preventDefault(); first2.focus(); }
        }
      }

      var input = null;
      if (kind === "items") {
        opts.items.forEach(function (it) {
          var row = el("button", "sp-menu-item");
          row.type = "button";
          row.appendChild(icon(it.icon || "arrow_right"));
          var col = el("span");
          col.appendChild(el("span", null, it.label));
          if (it.sub) { col.appendChild(el("span", "sp-sub", it.sub)); }
          row.appendChild(col);
          row.addEventListener("click", function () { done(it.value === undefined ? true : it.value); });
          box.appendChild(row);
        });
      } else {
        var form = el("form");
        if (kind === "field") {
          input = document.createElement("input");
          input.type = "text";
          input.placeholder = opts.field.placeholder || "";
          input.setAttribute("aria-label", opts.field.label || opts.title || "Valor");
          input.maxLength = opts.field.max || 120;
          if (opts.field.value) { input.value = opts.field.value; }
          form.appendChild(input);
        } else if (opts.html) {
          // Contenido libre (formulario compuesto): se inyecta en el box.
          var cont = el("div", "sp-html");
          if (typeof opts.html === "string") { cont.innerHTML = opts.html; }
          else if (opts.html.nodeType) { cont.appendChild(opts.html); }
          box.appendChild(cont);
        }
        var acts = el("div", "sp-actions");
        var cancel = el("button", "sp-btn ghost", "Cancelar");
        cancel.type = "button";
        cancel.addEventListener("click", function () { done(null); });
        var ok = el("button", "sp-btn primary", opts.ok || "Aceptar");
        ok.type = "submit";
        acts.appendChild(cancel);
        acts.appendChild(ok);
        form.appendChild(acts);
        form.addEventListener("submit", function (ev) {
          ev.preventDefault();
          if (kind === "field") {
            var v = (input.value || "").trim();
            if (!v) { input.focus(); return; }
            done(v);
          } else if (opts.onOk) {
            // El callback decide qué se devuelve (o cancela con `false`).
            var r = opts.onOk(box);
            if (r === false) { return; }
            done(r === undefined ? true : r);
          } else { done(true); }
        });
        box.appendChild(form);
      }

      document.addEventListener("keydown", onKey, true);
      veil.addEventListener("mousedown", function (e) { if (e.target === veil) { done(null); } });
      document.body.appendChild(veil);
      if (input) { input.focus(); input.select(); }
      else { var first = box.querySelector("button, a"); if (first) { first.focus(); } }
    });
  };

  SP.confirm = function (title, msg, okLabel) {
    return SP.dialog({ title: title, text: msg, ok: okLabel || "Confirmar" });
  };

  /* =================================================================
   *  portales (menus anclados a botones del topbar)
   * ================================================================= */
  var topbar = null;
  function shellTopbar() {
    if (!topbar) { topbar = document.getElementById("app-topbar"); }
    return topbar;
  }
  function findBell() { var t = shellTopbar(); return t && t.querySelector('button[title="Notificaciones"]'); }
  function findSearch() { var t = shellTopbar(); return t && t.querySelector('input[type="text"]'); }
  function findCreate() { var t = shellTopbar(); return t && t.querySelector("button.bg-secondary"); }

  function portal(button, fillFn) {
    var host = button.parentNode;
    var panel = el("div", "sp-menu");
    panel.hidden = true;
    panel.setAttribute("role", "menu");
    host.appendChild(panel);
    button.setAttribute("aria-haspopup", "menu");
    button.setAttribute("aria-expanded", "false");
    function openFn() {
      panel.textContent = "";
      fillFn(panel);
      panel.hidden = false;
      button.setAttribute("aria-expanded", "true");
      document.addEventListener("mousedown", outside, true);
      var a = panel.querySelector("a,button");
      if (a) { a.focus(); }
    }
    function outside(e) {
      if (panel.hidden || host.contains(e.target)) { return; }
      closeFn();
    }
    function closeFn() {
      panel.hidden = true;
      button.setAttribute("aria-expanded", "false");
      document.removeEventListener("mousedown", outside, true);
    }
    button.addEventListener("click", function (e) {
      e.preventDefault();
      if (!panel.hidden) { closeFn(); return; }
      openFn();
    });
    document.addEventListener("keydown", function (e) {
      if (e.key === "Escape" && !panel.hidden) { closeFn(); button.focus(); }
    });
    panel.addEventListener("click", function (e) {
      if (!panel.hidden && e.target.closest("a,button")) { setTimeout(closeFn, 60); }
    });
    return { panel: panel, open: openFn, close: closeFn };
  }

  function fillMenu(menu, items) {
    items.forEach(function (it) {
      if (it === "-") { menu.appendChild(el("div", "sp-sep")); return; }
      if (it.head) { menu.appendChild(el("div", "sp-menu-head", it.head)); return; }
      var a = el("a");
      a.setAttribute("role", "menuitem");
      if (it.href) {
        a.href = it.href;
      } else {
        a.href = "#";
        a.addEventListener("click", function (e) { e.preventDefault(); it.fn(); });
      }
      a.appendChild(icon(it.icon || "chevron_right"));
      var col = el("span");
      col.appendChild(el("span", null, it.label));
      if (it.sub) { col.appendChild(el("span", "sp-sub", it.sub)); }
      a.appendChild(col);
      menu.appendChild(a);
    });
  }

  /* =================================================================
   *  TOPBAR: buscador con /api/search
   * ================================================================= */
  function initSearch() {
    var input = findSearch();
    if (!input) { return; }
    input.id = "topbar-search";
    input.setAttribute("aria-label", "Buscar notas, tareas y temas");
    var box = input.closest(".relative") || input.parentNode;
    var panel = el("div", "sp-search-panel");
    panel.hidden = true;
    panel.setAttribute("role", "listbox");
    panel.setAttribute("aria-label", "Resultados de búsqueda");
    box.appendChild(panel);
    var hits = [];
    var last = "";
    var active = -1;
    var deb = null;

    input.addEventListener("input", function () {
      clearTimeout(deb);
      var q = input.value.trim();
      if (q.length < 2) { hide(); return; }
      deb = setTimeout(function () { run(q); }, 300);
    });
    input.addEventListener("keydown", function (e) {
      if (e.key === "ArrowDown") { e.preventDefault(); move(1); }
      else if (e.key === "ArrowUp") { e.preventDefault(); move(-1); }
      else if (e.key === "Enter") {
        e.preventDefault();
        if (active >= 0 && hits[active]) { window.location.href = "/app/notes/" + hits[active].slug; }
        else if (input.value.trim()) { window.location.href = "/dashboard/search?q=" + encodeURIComponent(input.value.trim()); }
      }
      else if (e.key === "Escape") { hide(); }
    });
    document.addEventListener("click", function (e) {
      if (!box.contains(e.target)) { hide(); }
    });
    document.addEventListener("keydown", function (e) {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        input.focus();
        input.select();
      }
    });

    function move(d) {
      if (!hits.length) { return; }
      active = (active + d + hits.length) % hits.length;
      paint();
    }
    function run(q) {
      last = q;
      SP.api("/api/search?q=" + encodeURIComponent(q)).then(function (data) {
        hits = (data.results || []).slice(0, 8);
        active = hits.length ? 0 : -1;
        paint();
      }).catch(function (err) {
        hits = [];
        active = -1;
        paint(); // el mensaje de error sale como estado vacio
        panel.textContent = "";
        panel.appendChild(el("div", "sp-hit-tags", "La busqueda fallo: " + msgErr(err)));
      });
    }
    function paint() {
      panel.textContent = "";
      if (!hits.length) {
        panel.appendChild(el("div", "sp-hit-tags", last ? "Sin resultados para \u00ab" + last + "\u00bb" : ""));
      } else {
        hits.forEach(function (h, i) {
          var a = el("a");
          a.href = "/app/notes/" + h.slug;
          a.setAttribute("aria-selected", i === active ? "true" : "false");
          a.appendChild(el("div", "sp-hit-title", h.title || h.slug));
          if (h.tags) {
            a.appendChild(el("div", "sp-hit-tags", "#" + String(h.tags).split(",").join(" #")));
          }
          a.addEventListener("mouseenter", function () { active = i; });
          panel.appendChild(a);
        });
      }
      injectCss();
      panel.hidden = false;
    }
    function hide() {
      panel.hidden = true;
      active = -1;
    }
  }

  /* =================================================================
   *  TOPBAR: campana — accesos reales
   * ================================================================= */
  function initBell() {
    var bell = findBell();
    if (!bell) { return; }
    portal(bell, function (menu) {
      fillMenu(menu, [
        { head: "Agenda" },
        { label: "Próxima clase", icon: "schedule", href: "/dashboard/horario" },
        { label: "Tareas y exámenes", icon: "assignment", href: "/dashboard/planner" },
        { label: "Notas recientes", icon: "description", href: "/dashboard/notes" },
        "-",
        { head: "Sistema" },
        { label: "Administración", icon: "admin_panel_settings", href: "/admin" },
        { label: "Copias de seguridad", icon: "backup", href: "/admin" },
        { label: "Configuración", icon: "settings", href: "/settings" }
      ]);
    });
  }

  /* =================================================================
   *  TOPBAR: "+ Crear" real
   * ================================================================= */
  function initTopbarCreate() {
    var btn = findCreate();
    if (!btn) { return; }
    portal(btn, function (menu) {
      fillMenu(menu, [
        { head: "Crear" },
        { label: "Nota", icon: "note_add", sub: "Markdown local", fn: newNotePrompt },
        { label: "Documento", icon: "article", sub: "programas de oficina", fn: function () { newOffice("docs", "Documento"); } },
        { label: "Hoja de cálculo", icon: "table_chart", sub: "csv", fn: function () { newOffice("sheets", "Hoja de cálculo"); } },
        { label: "Presentación", icon: "slideshow", fn: function () { newOffice("presentations", "Presentación"); } },
        { label: "Importar desde Obsidian", icon: "file_open", sub: "carpeta de archivos Markdown", fn: importObsidian },
        "-",
        { head: "Académico" },
        { label: "Conversación con IA", icon: "auto_awesome", href: "/dashboard/ai" },
        { label: "Generar material", icon: "model_training", href: "/dashboard/material" },
        { label: "Tarea del curso", icon: "task_alt", sub: "se gestiona en el planificador", href: "/dashboard/planner" },
        { label: "Ir a exámenes", icon: "assignment_late", href: "/dashboard/examenes" }
      ]);
    });
  }

  function newNotePrompt() {
    SP.dialog({
      title: "Nueva nota",
      text: "Se crea en tu boveda local; aparece como nota del panel.",
      field: { label: "Título de la nota", placeholder: "Ej. Apuntes de redes - clase 5", max: 80 },
      ok: "Crear nota"
    }).then(function (title) {
      if (!title) { return; }
      SP.api("/api/notes", { method: "POST", json: { title: title, content: "# " + title + "\n\n" } })
        .then(function () {
          SP.toast("Nota \u00ab" + title + "\u00bb creada", "ok");
          setTimeout(function () { window.location.href = "/dashboard/notes"; }, 650);
        })
        .catch(function (err) { SP.toast(msgErr(err), "err"); });
    });
  }

  function newOffice(kind, label) {
    SP.dialog({
      title: "Nueva " + label.toLowerCase(),
      text: "Se abre el editor correspondiente.",
      field: { label: "Título", placeholder: "Ej. Proyecto binomios", max: 80 },
      ok: "Crear"
    }).then(function (title) {
      if (!title) { return; }
      var path = "/office/" + kind;
      SP.api(path, { form: { title: title, content: "" } })
        .then(function () {
          SP.toast(label + " creada", "ok");
          var slug = sluginator(title);
          setTimeout(function () {
            if (kind === "docs") { window.location.href = "/office/docs/" + slug; }
            else if (kind === "sheets") { window.location.href = "/office/sheets"; }
            else { window.location.href = "/office/presentations/" + slug; }
          }, 600);
        })
        .catch(function (err) {
          if (err.status === 409) {
            SP.toast("Ya existía un elemento con ese título", "info");
            setTimeout(function () { window.location.href = path; }, 600);
            return;
          }
          SP.toast(msgErr(err), "err");
        });
    });
  }

  /* =================================================================
   *  IMPORTADOR OBSIDIAN (cliente: lee la carpeta, no toca el disco)
   * ================================================================= */
  /* =================================================================
   *  IMPORTADOR OBSIDIAN: lectura local y guardado secuencial
   * ================================================================= */
  function readMarkdownFile(file) {
    return new Promise(function (resolve, reject) {
      var reader = new FileReader();
      reader.onload = function () { resolve(String(reader.result || "")); };
      reader.onerror = function () { reject(new Error("No se pudo leer " + file.name)); };
      reader.readAsText(file);
    });
  }

  function tagsFromFrontmatter(text) {
    var m = /^---\r?\n([\s\S]{0,1200}?)\r?\n---(?:\s|\n|$)/.exec(text);
    if (!m) { return []; }
    var found = [];
    var line = /^tags:\s*(.*)$/m.exec(m[1]);
    if (line) {
      var bracketed = /^\s*\[(.+)\]\s*$/.exec(line[1]);
      if (bracketed) { found = found.concat(bracketed[1].split(/[,;]\s*/)); }
      else if (line[1].trim()) { found.push(line[1].trim().replace(/^['"]|['"]$/g, "")); }
    }
    var inTags = false;
    m[1].split(/\r?\n/).forEach(function (row) {
      if (/^tags:\s*$/.test(row)) { inTags = true; return; }
      if (inTags) {
        var item = /^\s*-\s*(.+)$/.exec(row);
        if (item) { found.push(item[1].trim().replace(/^['"]|['"]$/g, "")); }
        else if (row.trim()) { inTags = false; }
      }
    });
    return found.map(function (tag) {
      return String(tag).replace(/^#/, "").trim();
    }).filter(Boolean);
  }

  function pathTags(file) {
    var parts = String(file.webkitRelativePath || file.name).split("/");
    parts.pop(); // nombre del archivo
    if (parts.length > 1) { parts.shift(); } // omitir la raíz de la bóveda
    return parts.filter(Boolean);
  }

  function getNoteBySlug(slug) {
    return SP.api("/api/notes/" + encodeURIComponent(slug)).then(function (data) {
      return data && data.note ? data.note : null;
    }).catch(function (err) {
      if (err && err.status === 404) { return null; }
      throw err;
    });
  }

  function importedTitle(base, attempt) {
    if (!attempt) { return base; }
    return base + " (importado" + (attempt === 1 ? "" : " " + attempt) + ")";
  }

  function saveImportedNote(file, text, tags, baseTitle, policy, counts) {
    var baseSlug = sluginator(baseTitle);
    return getNoteBySlug(baseSlug).then(function (existing) {
      if (existing && policy === "overwrite-newer") {
        var oldTime = Date.parse(existing.updated_at || existing.updatedAt || "");
        var newTime = Number(file.lastModified || 0);
        if (Number.isFinite(oldTime) && newTime > oldTime) {
          // Conserva etiquetas añadidas en la app; la importación solo suma las del archivo y de la carpeta.
          var mergedTags = String(existing.tags || "").split(/\s*,\s*/).filter(Boolean).concat(tags);
          mergedTags = mergedTags.filter(function (tag, index, all) { return all.indexOf(tag) === index; });
          return SP.api("/api/notes/" + encodeURIComponent(existing.slug || baseSlug), {
            method: "PUT",
            json: { content: text, tags: mergedTags.join(", ") }
          }).then(function () {
            counts.updated++;
            return "updated";
          });
        }
        counts.skipped++;
        return "skipped";
      }

      function createUnique(attempt) {
        var title = importedTitle(baseTitle, attempt);
        var slug = sluginator(title);
        return getNoteBySlug(slug).then(function (collision) {
          if (collision) {
            if (policy === "overwrite-newer" && attempt === 0) {
              counts.skipped++;
              return "skipped";
            }
            if (attempt >= 99) { throw new Error("No se pudo generar un título único para " + baseTitle); }
            return createUnique(attempt + 1);
          }
          return SP.api("/api/notes", {
            method: "POST",
            json: { title: title, content: text, tags: tags.join(", ") }
          }).then(function () {
            counts.created++;
            return "created";
          }).catch(function (err) {
            if (err && err.status === 409 && attempt < 99) {
              return createUnique(attempt + 1);
            }
            throw err;
          });
        });
      }

      return createUnique(existing ? 1 : 0);
    });
  }

  SP.importObsidianFiles = function (files, policy) {
    files = Array.from(files || []).filter(function (file) {
      return file && /\.(md|markdown)$/i.test(file.name || "");
    });
    if (!files.length) {
      SP.toast("No se encontraron archivos Markdown para importar.", "info");
      return Promise.resolve({ created: 0, updated: 0, skipped: 0, failed: [] });
    }

    policy = policy === "overwrite-newer" ? "overwrite-newer" : "keep-both";
    var counts = { created: 0, updated: 0, skipped: 0, failed: [] };
    var chain = Promise.resolve();

    files.forEach(function (file, index) {
      chain = chain.then(function () {
        SP.toast("Importando archivo " + (index + 1) + " de " + files.length + "…", "info", 1600);
        return readMarkdownFile(file).then(function (text) {
          var baseTitle = String(file.name || "Nota importada").replace(/\.(md|markdown)$/i, "").trim() || "Nota importada";
          var tags = pathTags(file).concat(tagsFromFrontmatter(text));
          tags = tags.filter(function (tag, i) { return tags.indexOf(tag) === i; });
          return saveImportedNote(file, text, tags, baseTitle, policy, counts);
        }).catch(function (err) {
          counts.failed.push(String(file.name || "Archivo") + ": " + msgErr(err));
        });
      });
    });

    return chain.then(function () {
      var parts = [counts.created + " notas nuevas", counts.updated + " actualizadas", counts.skipped + " omitidas"];
      if (counts.failed.length) { parts.push(counts.failed.length + " con error"); }
      SP.toast("Importación de Obsidian terminada: " + parts.join(", ") + ".", counts.failed.length ? "err" : "ok", 9000);
      var report = Promise.resolve();
      if (counts.failed.length) {
        report = SP.dialog({
          title: "Importación finalizada con avisos",
          text: counts.failed.slice(0, 8).join("\n") + (counts.failed.length > 8 ? "\nY " + (counts.failed.length - 8) + " más." : ""),
          ok: "Entendido"
        });
      }
      // No recargar la página hasta que la persona haya podido leer los errores.
      return report.catch(function () { return null; }).then(function () {
        if (location.pathname.indexOf("/dashboard/notes") === 0) {
          setTimeout(function () { window.location.reload(); }, 900);
        } else if (counts.created || counts.updated) {
          setTimeout(function () { window.location.href = "/dashboard/notes"; }, 900);
        }
        return counts;
      });
    });
  };

  function importObsidian() {
    var input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.accept = ".md,.markdown";
    input.setAttribute("webkitdirectory", "");
    input.hidden = true;
    input.setAttribute("aria-label", "Seleccionar una carpeta de Obsidian");
    document.body.appendChild(input);
    SP.toast("Selecciona la carpeta de tu bóveda de Obsidian.", "info", 6000);
    input.addEventListener("change", function () {
      var files = Array.from(input.files || []);
      input.remove();
      SP.importObsidianFiles(files, "keep-both");
    }, { once: true });
    input.addEventListener("cancel", function () { input.remove(); }, { once: true });
    input.click();
  }

  /* =================================================================
   *  SETTINGS honesto + /ai legado -> asistente real
   * ================================================================= */
  function initSettings() {
    var form = document.querySelector('form[action="/api/settings/ai"]');
    if (!form) { return; }
    form.addEventListener("submit", function (e) {
      e.preventDefault();
      SP.toast("La configuración de IA se define en .env. Edita el archivo en el servidor y reinicia el servicio; estos cambios no se guardan desde aquí.", "info", 8000);
    });
  }

  function initAiLegacy() {
    if (location.pathname !== "/ai" || document.querySelector(".nova-conversations-page")) { return; }
    var main = document.querySelector("main") || document.body;
    if (!main) { return; }
    var aviso = document.createElement("div");
    aviso.className = "sp-banner";
    aviso.innerHTML = 'Las conversaciones viven en el asistente nuevo: <a class="sp-link" href="/dashboard/ai">abrir asistente</a>';
    if (main.firstChild) { main.insertBefore(aviso, main.firstChild); } else { main.appendChild(aviso); }
    main.querySelectorAll("button").forEach(function (b) {
      once(b, "spAiLegacy", function (btn) {
        btn.addEventListener("click", function (e) {
          e.preventDefault();
          window.location.href = "/dashboard/ai";
        });
      });
    });
    var cargando = Array.prototype.find.call(main.querySelectorAll("*"), function (el) {
      return el.children.length === 0 && txt(el) === "Cargando conversaciones...";
    });
    if (cargando) { cargando.textContent = "Sin conversaciones guardadas: el asistente real es /dashboard/ai"; }
  }

  /* =================================================================
   *  DASHBOARD: menu "+ Crear" del kit + modal de tarea
   * ================================================================= */
  function initDashboards() {
    var toggleCreate = document.getElementById("toggle-create-menu");
    if (!toggleCreate) { return; } // solo existen en dashboard y portada
    var map = [
      { t: "nueva tarea", href: "/dashboard/planner", sub: "tareas del curso (Mikareno)" },
      { t: "nuevo examen", href: "/dashboard/examenes" },
      { t: "nueva clase", href: "/dashboard/horario" },
      { t: "nueva asignatura", href: "/dashboard/subjects" },
      { t: "nota markdown", fn: newNotePrompt },
      { t: "nota md", fn: newNotePrompt },
      { t: "carpeta", fn: function () { SP.toast("La importación de carpetas aún no está disponible; las notas Markdown sí se pueden importar.", "info"); } },
      { t: "grafo", href: "/dashboard/notes" },
      { t: "nuevo chat", href: "/dashboard/ai" },
      { t: "generar resumen", fn: aiCatchPrompt("Resúmeme el material adjunto en puntos clave.") },
      { t: "crear cuestionario", fn: aiCatchPrompt("Crea un cuestionario de 5 preguntas con sus respuestas correctas al final.") },
      { t: "plan de estudio", href: "/dashboard/planner" },
      { t: "documento", href: "/office/docs" },
      { t: "hoja de cálculo", href: "/office/sheets" },
      { t: "hoja c\u00e1lculo", href: "/office/sheets" },
      { t: "hoja de calculo", href: "/office/sheets" },
      { t: "diapositiva", href: "/office/presentations" }
    ];
    document.querySelectorAll("#contenido button").forEach(wireItem);

    function wireItem(btn) {
      if (btn.id === "quick-create-btn") { return; } // el kit ya abre su modal
      once(btn, "spItem", function (b) {
        var t2 = txt(b).toLowerCase();
        if (!t2) { return; }
        for (var i = 0; i < map.length; i++) {
          if (t2.indexOf(map[i].t) !== -1) {
            b.addEventListener("click", function (e) {
              // si el kit ya navega por si mismo (a href), no duplicar
              if (b.tagName === "A") { return; }
              if (map[i].fn) { e.preventDefault(); e.stopPropagation(); map[i].fn(); return; }
              e.stopPropagation();
              window.location.href = map[i].href;
            });
            return;
          }
        }
      });
    }

    function toastVaultPending() {
      SP.toast("La importación de carpetas aún no está disponible; las notas Markdown sí se pueden importar.", "info");
    }

    /* Modal "Nueva tarea academica": captura honesta → nota o planificador */
    var modal = document.getElementById("quick-task-modal");
    if (!modal) { return; }
    var titleInput = modal.querySelector("input[type=text]");
    var selects = modal.querySelectorAll("select");
    var dateInput = modal.querySelector("input[type=date]");
    var timeInput = modal.querySelector("input[type=time]");
    // El ID estable permite traducir el texto visible sin perder el enlace del botón.
    var saveBtn = modal.querySelector("#save-task-as-note");
    if (!saveBtn) {
      var btns = modal.querySelectorAll("button");
      for (var i = 0; i < btns.length; i++) {
        var label = txt(btns[i]).toLowerCase();
        if (label.indexOf("guardar tarea") !== -1 || label.indexOf("guardar como nota") !== -1) {
          saveBtn = btns[i];
          break;
        }
      }
    }
    if (saveBtn) {
      var row = saveBtn.parentNode;
      var hint = el("p", null, "Las tareas del curso viven en el planificador (Mikareno): la app las lee, no las crea. Guarda esta ficha como nota rápida o abre el planificador.");
      hint.style.marginTop = ".7rem";
      row.parentNode.insertBefore(hint, row);
      var planner = el("a", saveBtn.className + " justify-center", "Abrir el planificador");
      planner.href = "/dashboard/planner";
      row.parentNode.insertBefore(planner, row);
      saveBtn.textContent = "Guardar como nota";

      function guardarFicha(e) {
        if (e) { e.preventDefault(); }
        if (saveBtn.disabled) { return; }
        var title = ((titleInput && titleInput.value) || "").trim();
        if (!title) {
          SP.toast("Escribe un título o usa el planificador", "info");
          if (titleInput) { titleInput.focus(); }
          return;
        }
        var lines = ["## " + title, ""];
        if (selects.length > 0 && selText(selects[0])) { lines.push("- Asignatura: " + selText(selects[0])); }
        if (selects.length > 1 && selText(selects[1])) { lines.push("- Prioridad: " + selText(selects[1])); }
        if (dateInput && dateInput.value) {
          lines.push("- Fecha límite: " + dateInput.value + (timeInput && timeInput.value ? " " + timeInput.value : ""));
        }
        lines.push("", "- [ ] Pasa al planificador cuando lo revises", "");
        saveBtn.disabled = true;
        SP.api("/api/notes", { method: "POST", json: { title: title, content: lines.join("\n"), tags: "tarea" } })
          .then(function () {
            saveBtn.disabled = false;
            var cancelButton = modal.querySelector("#cancel-modal-btn");
            if (cancelButton) { cancelButton.click(); }
            else { modal.classList.add("hidden"); }
            SP.toast("Ficha de tarea guardada como nota \u00ab" + title + "\u00bb", "ok", 4200);
          })
          .catch(function (err) {
            saveBtn.disabled = false;
            SP.toast(msgErr(err), "err");
          });
      }

      var taskForm = modal.querySelector("form");
      if (taskForm) {
        taskForm.addEventListener("submit", guardarFicha);
      } else {
        saveBtn.addEventListener("click", guardarFicha);
      }
    }
  }

  function aiCatchPrompt(prompt) {
    return function () {
      sessionStorage.setItem("sp:ai-prompt", prompt);
      window.location.href = "/dashboard/ai";
    };
  }

  /* =================================================================
   *  NOTAS: editor real
   * ================================================================= */
  function initCollections() {
    var primera = document.querySelector('a[data-stitch-row="list"]');
    if (!primera) { return; }
    var lista = primera.parentElement;
    var anclas = lista.querySelectorAll("a[data-stitch-row]");
    var porTag = {};
    anclas.forEach(function (a) {
      (a.getAttribute("data-stitch-tags") || "").split(/[\s,]+/).forEach(function (t) {
        if (t) { porTag[t] = (porTag[t] || 0) + 1; }
      });
    });
    var etiquetas = Object.keys(porTag).sort();
    if (!etiquetas.length) { return; }
    var sel = document.createElement("select");
    sel.id = "sp-coleccion";
    sel.title = "Colección: filtra la lista por etiqueta";
    var opts = '<option value="">Todas las notas (' + anclas.length + ")</option>";
    etiquetas.forEach(function (t) {
      opts += '<option value="' + t.replace(/"/g, "&quot;") + '">' + t + " (" + porTag[t] + ")</option>";
    });
    sel.innerHTML = opts;
    var marco = document.createElement("div");
    marco.className = "sp-coleccion-bar";
    marco.appendChild(sel);
    lista.insertBefore(marco, lista.firstChild);
    sel.addEventListener("change", function () {
      var v = sel.value;
      if (v) {
        sel.setAttribute("data-vista", v);
      } else {
        sel.removeAttribute("data-vista");
      }
      var visibles = 0;
      anclas.forEach(function (a) {
        var tags = a.getAttribute("data-stitch-tags") || "";
        var mostrar = !v || tags.split(/[\s,]+/).indexOf(v) !== -1;
        a.style.display = mostrar ? "" : "none";
        if (mostrar) { visibles++; }
      });
      sel.title = v ? "Mostrando solo \"" + v + "\" (" + visibles + " notas)" : "Todas las notas (" + anclas.length + ")";
    });
  }

  function initCollectionsSafe() {
    try { initCollections(); } catch (e) { /* no rompe la pagina */ }
  }

  function initVaultActions() {
    var importButton = document.getElementById("btn-import-vault");
    // El flujo del botón es exclusivo: si la página trae el modal de
    // importación segura (double-confirm), el propio modal es el propietario
    // del clic y aquí no se registra un selector directo.
    if (importButton && !document.getElementById("import-vault-modal")) {
      once(importButton, "spVaultImport", function (button) {
        button.addEventListener("click", function (event) {
          event.preventDefault();
          importObsidian();
        });
      });
    }
    var folderButton = document.getElementById("btn-new-folder");
    if (folderButton) {
      once(folderButton, "spVaultFolder", function (button) {
        button.addEventListener("click", function () {
          SP.toast("Las carpetas llegan con la importación de bóvedas; por ahora puedes organizar con etiquetas.", "info", 6000);
        });
      });
    }
  }

  function initNotesEditor() {
    var titleInput = document.querySelector('input[placeholder*="Sin notas"]') || document.getElementById("note-title");
    if (!titleInput) { return; }
    if (!titleInput.id) { titleInput.id = "note-title"; }
    var modes = {
      edit: document.getElementById("mode-edit"),
      split: document.getElementById("mode-split"),
      preview: document.getElementById("mode-preview")
    };
    if (!modes.edit || !modes.split || !modes.preview) { return; }

    var panes = document.querySelectorAll("div.flex-1.grid.grid-cols-1 > div");
    if (panes.length < 2) { return; }
    var rawPane = panes[0];
    var prePane = panes[1];
    rawPane.id = "note-editor-pane";
    prePane.id = "note-preview";
    prePane.setAttribute("aria-live", "polite");
    titleInput.removeAttribute("readonly");

    var allNotes = [];
    var current = { slug: "", title: (titleInput.value || "").trim(), content: "", tags: "" };
    var dirty = false;
    var zoom = 1;

    var editor = document.createElement("textarea");
    editor.id = "note-editor";
    editor.placeholder = "Escribe tu nota en Markdown - Ctrl+S guarda";
    editor.spellcheck = false;
    rawPane.innerHTML = '<span class="nx-pane-label">MARKDOWN</span>';
    rawPane.appendChild(editor);
    rawPane.style.padding = "";

    SP.api("/api/notes").then(function (data) {
      allNotes = (data.notes || []).filter(function (n) { return n && n.slug; });
      var found = null;
      if (current.title) {
        for (var i = 0; i < allNotes.length; i++) {
          if (String(allNotes[i].title).trim() === current.title) { found = allNotes[i]; break; }
        }
      }
      if (!found && allNotes.length) { found = allNotes[0]; }
      if (!found) {
        SP.toast("La bóveda está vacía: crea la primera nota con \"Nota\" en + Crear", "info", 5000);
        return;
      }
      hydrate(found.slug);
    }).catch(function (err) { SP.toast(msgErr(err), "err"); });

    function hydrate(slug) {
      SP.api("/api/notes/" + slug).then(function (d) {
        var n = d.note || {};
        current = { slug: slug, title: n.title || slug, content: String(d.content || ""), tags: n.tags || "" };
        titleInput.value = current.title;
        editor.value = current.content;
        buildSaveButton();
        setMode(window.matchMedia("(max-width: 860px)").matches ? "edit" : "split");
        updateStats();
        render();
      }).catch(function (err) { SP.toast(msgErr(err), "err"); });
    }

    editor.addEventListener("input", function () {
      dirty = true;
      updateSaveState();
      updateStats();
      debouncedRender();
    });
    editor.addEventListener("keydown", function (e) {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        save({});
      }
      if (e.key === "Tab") {
        e.preventDefault();
        insert("\u00a0\u00a0");
      }
    });

    var renderTimer = null;
    function debouncedRender() {
      clearTimeout(renderTimer);
      renderTimer = setTimeout(render, 200);
    }
    function render() { renderMarkupInto(prePane, editor.value, allNotes); }

    function setMode(which) {
      rawPane.style.display = which === "preview" ? "none" : "";
      prePane.style.display = which === "edit" ? "none" : "";
      var map = { edit: modes.edit, preview: modes.preview, split: modes.split };
      Object.keys(map).forEach(function (k) {
        var b = map[k];
        if (b) { b.setAttribute("aria-pressed", k === which ? "true" : "false"); }
      });
    }
    modes.edit.addEventListener("click", function (e) { e.preventDefault(); setMode("edit"); });
    modes.split.addEventListener("click", function (e) { e.preventDefault(); setMode("split"); });
    modes.preview.addEventListener("click", function (e) { e.preventDefault(); setMode("preview"); });

    var fmtMap = {
      "Título H1": function () { linePrefix("# "); },
      "Título H2": function () { linePrefix("## "); },
      "Negrita": function () { surround("**", "**"); },
      "Cursiva": function () { surround("*", "*"); },
      "Codigo Inline": function () { surround("`", "`"); },
      "Enlace Interno Bidireccional": wikiLinkPrompt,
      "Lista de verificacion": function () { linePrefix("- [ ] "); },
      "Tabla": function () { insert("\n| Col 1 | Col 2 |\n|---|---|\n|  |  |\n"); },
      "Bloque Matematico": function () { insert("$$\n\n$$"); }
    };
    document.querySelectorAll("button[title]").forEach(function (b) {
      var t = b.getAttribute("title");
      if (fmtMap[t]) {
        once(b, "spFmt", function (btn) {
          btn.setAttribute("aria-label", t);
          btn.addEventListener("click", function (e) { e.preventDefault(); fmtMap[t](); });
        });
      }
    });

    function markDirty() { dirty = true; updateSaveState(); debouncedRender(); }
    function surround(pre, post) {
      var s = editor.selectionStart, e2 = editor.selectionEnd;
      var v = editor.value;
      var sel = v.slice(s, e2) || "texto";
      editor.value = v.slice(0, s) + pre + sel + post + v.slice(e2);
      editor.selectionStart = s + pre.length;
      editor.selectionEnd = s + pre.length + sel.length;
      editor.focus();
      markDirty();
    }
    function linePrefix(prefix) {
      var s = editor.selectionStart;
      var lineStart = editor.value.lastIndexOf("\n", Math.max(0, s - 1)) + 1;
      editor.value = editor.value.slice(0, lineStart) + prefix + editor.value.slice(lineStart);
      editor.selectionStart = editor.selectionEnd = s + prefix.length;
      editor.focus();
      markDirty();
    }
    function insert(str) {
      var s = editor.selectionStart, e2 = editor.selectionEnd;
      editor.value = editor.value.slice(0, s) + str + editor.value.slice(e2);
      editor.selectionStart = editor.selectionEnd = s + str.length;
      editor.focus();
      markDirty();
    }
    function wikiLinkPrompt() {
      var s = editor.selectionStart, e2 = editor.selectionEnd;
      var sel = editor.value.slice(s, e2).trim();
      if (sel) { insert("[[" + sel + "]]"); return; }
      SP.dialog({
        title: "Enlace interno (WikiLink)",
        text: "Nombre de la nota destino (la puedes crear despues).",
        field: { placeholder: "Ej. Apuntes de redes", max: 60 },
        ok: "Insertar"
      }).then(function (name) { if (name) { insert("[[" + name + "]]"); } });
    }

    [["Zoom In", "Ampliar"], ["Zoom Out", "Reducir"], ["Centrar", "Centrar"]].forEach(function (names) {
      var t = names[0];
      var b = document.querySelector('button[title="' + names[0] + '"], button[aria-label="' + names[1] + '"]');
      if (!b) { return; }
      once(b, "spZoom", function (btn) {
        btn.addEventListener("click", function (e) {
          e.preventDefault();
          if (t === "Zoom In") { zoom = Math.min(1.35, zoom + 0.1); }
          else if (t === "Zoom Out") { zoom = Math.max(0.75, zoom - 0.1); }
          else { zoom = 1; }
          editor.style.fontSize = (0.875 * zoom).toFixed(2) + "rem";
          prePane.style.fontSize = (1.125 * zoom).toFixed(2) + "rem";
          if (t === "Centrar") { editor.scrollTop = 0; prePane.scrollTop = 0; }
        });
      });
    });

    var tagBtn = null;
    document.querySelectorAll("main button").forEach(function (b) {
      if (txt(b) === "+ tag") { tagBtn = b; }
    });
    if (tagBtn) {
      once(tagBtn, "spTag", function (btn) {
        btn.addEventListener("click", function (e) {
          e.preventDefault();
          if (!current.slug) { SP.toast("Abre o crea una nota primero", "info"); return; }
          SP.dialog({
            title: "Etiquetas de la nota",
            text: "Separa con comas.",
            field: { placeholder: "matemáticas, redes", max: 200, value: current.tags || "" },
            ok: "Guardar etiquetas"
          }).then(function (v) {
            if (v === null) { return; }
            save({ tags: v });
          });
        });
      });
    }

    function buildSaveButton() {
      if (document.getElementById("note-save")) { return; }
      var b = el("button", "nx-button nx-button-primary nx-note-save", "Guardar");
      b.id = "note-save";
      b.disabled = true;
      b.setAttribute("aria-label", "Guardar nota (Ctrl+S)");
      b.addEventListener("click", function (e) { e.preventDefault(); save({}); });
      var metaRow = modes.split.parentNode &&
        modes.split.parentNode.parentNode;
      if (metaRow) { metaRow.appendChild(b); }
      else { editor.parentNode.appendChild(b); }
    }
    function updateSaveState() {
      var b = document.getElementById("note-save");
      if (b) { b.disabled = !dirty; }
    }
    function save(extra) {
      if (!current.slug) { SP.toast("No hay cambios que guardar", "info"); return; }
      var body = {};
      var ex = extra || {};
      if (ex.tags !== undefined) { body.tags = ex.tags; }
      else {
        body.content = editor.value;
        if (titleInput && titleInput.value.trim() && titleInput.value.trim() !== current.title) {
          body.title = titleInput.value.trim();
        }
      }
      SP.api("/api/notes/" + current.slug, { method: "PUT", json: body })
        .then(function () {
          if (body.title) { current.title = body.title; }
          if (body.tags !== undefined) { current.tags = body.tags; }
          dirty = false;
          updateSaveState();
          updateStats();
          SP.toast("Nota guardada (" + new Date().toLocaleTimeString().slice(0, 5) + ")", "ok");
          SP.api("/api/notes").then(function (d) { allNotes = (d.notes || []).filter(function (n) { return n && n.slug; }); }).catch(function () { });
        })
        .catch(function (err) { SP.toast(msgErr(err), "err"); });
    }
    function updateStats() {
      var words = editor.value.trim() ? editor.value.trim().split(/\s+/).length : 0;
      var stat = document.querySelector(".nova-note-stats") || document.querySelector("div.ml-auto");
      if (stat) { stat.textContent = words + " palabras" + (dirty ? " - sin guardar" : ""); }
    }

    document.querySelectorAll("main button").forEach(function (b) {
      var t = txt(b);
      var noteAction = b.getAttribute("data-note-action");
      once(b, "spAction", function (btn) {
        if (noteAction === "pdf" || t === "Exportar a PDF") {
          btn.addEventListener("click", function (e) {
            e.preventDefault();
            setMode("preview");
            render();
            document.body.classList.add("sp-printing");
            var off = function () { document.body.classList.remove("sp-printing"); };
            window.addEventListener("afterprint", off, { once: true });
            setTimeout(function () { window.print(); }, 80);
            setTimeout(off, 4000);
          });
        } else if (noteAction === "office" || t === "Exportar a Suite Oficina") {
          btn.addEventListener("click", function (e) {
            e.preventDefault();
            var content = editor.value || current.content;
            if (!content || !content.trim()) { SP.toast("La nota está vacía", "info"); return; }
            var name = current.title || (titleInput && titleInput.value) || "nota";
            track(btn, true);
            SP.api("/office/docs", { form: { title: name, content: content } })
              .then(function () {
                SP.toast("Documento creado en Documentos", "ok");
                setTimeout(function () { window.location.href = "/office/docs/" + sluginator(name); }, 700);
              })
              .catch(function (err) {
                if (err.status === 409) {
                  SP.toast("Ya existía; abriendo en Documentos", "info");
                  setTimeout(function () { window.location.href = "/office/docs/" + sluginator(name); }, 500);
                  return;
                }
                SP.toast(msgErr(err), "err");
              })
              .finally(function () { track(btn, false); });
          });
        } else if (noteAction === "ai" || t === "Inyectar a Asistente IA") {
          btn.addEventListener("click", function (e) {
            e.preventDefault();
            var c = editor.value || current.content;
            if (!c || !c.trim()) { SP.toast("La nota está vacía", "info"); return; }
            sessionStorage.setItem("sp:ai-ctx", JSON.stringify({ t: current.title, c: c.slice(0, 6000) }));
            SP.toast("Contexto adjuntado: abriendo el asistente…", "ok");
            setTimeout(function () { window.location.href = "/dashboard/ai"; }, 600);
          });
        } else if (b.id === "btn-new-note" || t === "+ Nota") {
          btn.addEventListener("click", function (e) { e.preventDefault(); newNotePrompt(); });
        } else if (b.id === "btn-new-folder") {
          btn.addEventListener("click", function (e) {
            e.preventDefault();
            SP.toast("Las carpetas llegan con la importacion de bovedas (pendiente en backend)", "info");
          });
        }
      });
    });

    function openPalette() {
      SP.api("/api/notes").then(function (data) {
        var items = [];
        (data.notes || []).slice(0, 12).forEach(function (n) {
          items.push({ label: n.title, icon: "sticky_note_2", sub: "nota", value: "/dashboard/notes?note=" + n.slug });
        });
        [
          ["Panel principal", "dashboard", "/dashboard"],
          ["Mis notas", "description", "/dashboard/notes"],
          ["Horario", "schedule", "/dashboard/horario"],
          ["Planificador", "timeline", "/dashboard/planner"],
          ["Asistente IA", "auto_awesome", "/dashboard/ai"],
          ["Documentos", "article", "/office/docs"],
          ["Configuración", "settings", "/settings"],
          ["Administración", "admin_panel_settings", "/admin"]
        ].forEach(function (x) { items.push({ label: x[0], icon: x[1], value: x[2] }); });
        SP.dialog({ title: "Comandos", text: "Notas y pantallas", items: items }).then(function (v) {
          if (v && typeof v === "string") { window.location.href = v; }
        });
      }).catch(function (err) { SP.toast(msgErr(err), "err"); });
    }
    var paletteBtn = document.getElementById("quick-palette-trigger");
    if (paletteBtn) {
      paletteBtn.addEventListener("click", function (e) { e.preventDefault(); openPalette(); });
    }
    document.addEventListener("keydown", function (e) {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "p") {
        e.preventDefault();
        openPalette();
      }
    });
  }

  /* =================================================================
   *  ASISTENTE IA: chips, attach, microfono
   * ================================================================= */
  /* =================================================================
   *  CALENDARIO: anotaciones locales del usuario
   * ================================================================= */
  function initCalendar() {
    var grid = document.getElementById("calendar-grid");
    if (!grid) { return; }

    function dialogoAnotacion(fecha, anotacion) {
      var cuerpo = document.createElement("div");
      cuerpo.innerHTML =
        '<label class="sp-field"><span class="sp-field-label">Título</span>' +
        '<input class="sp-input" id="sp-anot-titulo" maxlength="120" placeholder="Ej. Examen de matematicas"></label>' +
        '<label class="sp-field"><span class="sp-field-label">Nota</span>' +
        '<textarea class="sp-input" id="sp-anot-cuerpo" rows="4" maxlength="4000" placeholder="Detalles, material que llevar, recordatorios..."></textarea></label>' +
        '<div class="sp-anot-lista" id="sp-anot-lista"></div>';
      SP.dialog({
        title: anotacion ? "Editar anotación" : "Anotar el " + fecha,
        html: cuerpo,
        ok: anotacion ? "Guardar" : "Anotar",
        onOk: function (box) {
          var t = box.querySelector("#sp-anot-titulo");
          var c = box.querySelector("#sp-anot-cuerpo");
          return { title: t ? t.value : "", body: c ? c.value : "" };
        }
      }).then(function (res) {
        if (!res) { return; }
        var json = res;
        var p = anotacion
          ? SP.api("/api/calendar/annotations/" + anotacion.id, { method: "PUT", json: json })
          : SP.api("/api/calendar/annotations", { method: "POST", json: { date: fecha, title: json.title, body: json.body } });
        p.then(function () {
          SP.toast(anotacion ? "Anotación actualizada" : "Anotación guardada", "ok");
          setTimeout(function () { window.location.reload(); }, 500);
        }).catch(function (err) { SP.toast(msgErr(err), "err"); });
      });
    }

    function listar(fecha) {
      SP.api("/api/calendar/annotations?from=" + fecha + "&to=" + fecha).then(function (d) {
        var lista = (d && d.annotations) || [];
        if (!lista.length) { dialogoAnotacion(fecha, null); return; }
        var items = lista.map(function (a) {
          return { label: a.title || a.body.slice(0, 40) || "Anotación", sub: a.body ? a.body.slice(0, 60) : "", icon: "sticky_note_2", value: a };
        });
        items.push({ label: "Añadir otra anotación", icon: "add", value: "new" });
        SP.dialog({ title: "Anotaciones del " + fecha, items: items }).then(function (v) {
          if (!v) { return; }
          if (v === "new") { dialogoAnotacion(fecha, null); return; }
          if (typeof v === "object" && v.id) { dialogoAnotacion(fecha, v); }
        });
      }).catch(function (err) { SP.toast(msgErr(err), "err"); });
    }

    function abrirCelda(cell, e) {
      if (!cell) { return; }
      var fecha = cell.getAttribute("data-date");
      if (!fecha) { return; }
      e.preventDefault();
      e.stopPropagation();
      listar(fecha);
    }

    grid.addEventListener("click", function (e) {
      var cell = e.target.closest ? e.target.closest(".calendar-cell") : null;
      abrirCelda(cell, e);
    });

    grid.addEventListener("keydown", function (e) {
      if (e.key !== "Enter" && e.key !== " ") { return; }
      var cell = e.target.closest ? e.target.closest(".calendar-cell") : null;
      abrirCelda(cell, e);
    });

    // Clic derecho: borrar anotación directamente (atajo para limpiar).
    grid.addEventListener("contextmenu", function (e) {
      var cell = e.target.closest ? e.target.closest(".calendar-cell") : null;
      if (!cell) { return; }
      var fecha = cell.getAttribute("data-date");
      if (!fecha) { return; }
      e.preventDefault();
      SP.api("/api/calendar/annotations?from=" + fecha + "&to=" + fecha).then(function (d) {
        var lista = (d && d.annotations) || [];
        if (!lista.length) { return; }
        SP.api("/api/calendar/annotations/" + lista[0].id, { method: "DELETE" }).then(function () {
          SP.toast("Anotación borrada", "ok");
          setTimeout(function () { window.location.reload(); }, 400);
        }).catch(function (err) { SP.toast(msgErr(err), "err"); });
      }).catch(function () { });
    });
  }

  /* =================================================================
   *  ASISTENTE: selector de modelo de OpenRouter (free y de pago)
   * ================================================================= */
  function initModelPicker() {
    var chips = document.querySelectorAll("#modelo-chip, [data-sp-modelo]");
    if (!chips.length) { return; }

    function pintar(mensaje) {
      chips.forEach(function (c) {
        var span = c.querySelector("[data-sp-modelo-nombre]") || c;
        span.textContent = mensaje;
      });
    }

    // Precio real del catálogo (por 1M tokens). Si OpenRouter no reporta un
    // precio, no se inventa: se omite ese componente.
    function precioEtiqueta(modelo) {
      if (!modelo) { return ""; }
      var fmt = function (v) {
        var n = parseFloat(v);
        if (!isFinite(n)) { return null; }
        var porMillon = n * 1000000;
        if (!porMillon) { return "$0"; }
        return "$" + (porMillon >= 0.01 ? porMillon.toFixed(2) : Number(porMillon.toPrecision(2)));
      };
      var entrada = modelo.pricing_prompt != null ? fmt(modelo.pricing_prompt) : null;
      var salida = modelo.pricing_completion != null ? fmt(modelo.pricing_completion) : null;
      var partes = [];
      if (entrada) { partes.push("entrada " + entrada); }
      if (salida) { partes.push("salida " + salida); }
      if (!partes.length) { return ""; }
      return " · " + partes.join(" / ") + " (por 1M tokens)";
    }

    function subDeModelo(modelo) {
      if (!modelo) { return ""; }
      var estado = modelo.free ? "Gratis" : "De pago";
      return estado + precioEtiqueta(modelo);
    }

    function esAvisoDePago(id, modelo) {
      if (/^[\s\S]*:free$/.test(id)) { return false; }
      if (modelo && modelo.free === true) { return false; }
      return true; // ID desconocido o con free === false: pedir confirmación.
    }

    function aplicarConConfirmacion(id, modelo) {
      var guardar = function () {
        SP.api("/api/ai/model", { method: "PUT", json: { model: id } }).then(function () {
          SP.toast("Modelo cambiado a " + id, "ok", 4000);
          activo = id;
          pintar(id);
        }).catch(function (err) { SP.toast(msgErr(err), "err"); });
      };
      if (!esAvisoDePago(id, modelo)) { guardar(); return; }
      var precio = precioEtiqueta(modelo);
      var aviso = "El modelo " + id + " es de pago: puede generar cargos en tu cuenta de OpenRouter."
        + (precio ? " Referencia del catálogo" + precio + "." : " OpenRouter no reporta precio para este ID: el gasto solo queda registrado en la app como estimado (límite inferior).")
        + " El presupuesto interno OPENROUTER_MAX_COST_USD limita el gasto anotado por la app, pero no sustituye la facturación del proveedor.";
      SP.confirm("Confirmar modelo de pago", aviso, "Aplicar modelo")
        .then(function (confirmado) {
          // El diálogo resuelve `null` al cancelar (Escape / velo / botón):
          // solo con valor explícito se aplica el PUT.
          if (confirmado) { guardar(); }
        })
        .catch(function () { /* cancelado: no se cambia el modelo */ });
    }

    SP.api("/api/ai/models").then(function (d) {
      var modelos = (d && d.models) || [];
      if (!modelos.length) { return; }
      var activo = d.active || "";
      chips.forEach(function (c) {
        var span = c.querySelector("[data-sp-modelo-nombre]") || c;
        var m = modelos.filter(function (x) { return x.id === activo; })[0];
        span.textContent = m ? m.name : activo;
        once(c, "spModelo", function (chip) {
          chip.addEventListener("click", function (e) {
            e.preventDefault();
            var buscar = function (id) {
              for (var i = 0; i < modelos.length; i++) { if (modelos[i].id === id) { return modelos[i]; } }
              return null;
            };
            var items = modelos.map(function (x) {
              return {
                label: x.name,
                sub: subDeModelo(x),
                icon: x.id === activo ? "check_circle" : "memory",
                value: x.id
              };
            });
            // Opción para escribir cualquier modelo de OpenRouter (gratuito o de pago).
            items.push({ label: "Escribir otro modelo de OpenRouter", icon: "edit", value: "custom" });
            SP.dialog({ title: "Modelo de IA (OpenRouter - gratuito y de pago)", items: items }).then(function (id) {
              if (!id || id === activo) { return; }
              if (id === "custom") {
                SP.dialog({
                  title: "Escribe el identificador del modelo",
                  field: {
                    label: "ID de modelo de OpenRouter",
                    placeholder: "Por ejemplo: google/gemma-4-31b-it:free o anthropic/claude-opus-4.5",
                    value: activo,
                    max: 200
                  }
                }).then(function (txt) {
                  if (!txt) { return; }
                  txt = txt.trim();
                  aplicarConConfirmacion(txt, buscar(txt));
                });
                return;
              }
              aplicarConConfirmacion(id, buscar(id));
            });
          });
        });
      });
    }).catch(function () { });
  }

  function initChatWorkspace() {
    var layout = document.querySelector(".nx-chat, .nova-chat-layout");
    if (!layout) { return; }
    var promptInput = document.getElementById("prompt-input");
    var historyToggle = document.getElementById("toggle-history-btn");
    var historyClose = document.getElementById("close-history-btn");
    var sourcesToggle = document.getElementById("toggle-sources-btn");
    var sources = document.getElementById("sources-sidebar");
    var scrim = layout.querySelector(".nx-chat-scrim, .nova-chat-scrim");
    var search = document.getElementById("session-search");
    var sessions = layout.querySelector(".nx-chat-sessions, .nova-chat-sessions");

    function setHistory(open) {
      layout.classList.toggle("is-history-open", !!open);
      if (historyToggle) { historyToggle.setAttribute("aria-expanded", open ? "true" : "false"); }
    }
    function closePanels() {
      setHistory(false);
      if (sources) { sources.classList.add("hidden"); }
    }
    if (historyToggle) { historyToggle.addEventListener("click", function () { setHistory(!layout.classList.contains("is-history-open")); }); }
    if (historyClose) { historyClose.addEventListener("click", function () { setHistory(false); }); }
    if (sourcesToggle) { sourcesToggle.addEventListener("click", function () { setHistory(false); }); }
    if (scrim) { scrim.addEventListener("click", closePanels); }
    document.addEventListener("keydown", function (e) {
      if (e.key === "Escape" && (layout.classList.contains("is-history-open") || (sources && !sources.classList.contains("hidden")))) {
        closePanels();
      }
    });

    if (search && sessions) {
      var filterEmpty = document.createElement("p");
      filterEmpty.className = "nx-chat-history-empty";
      filterEmpty.textContent = "No hay conversaciones que coincidan.";
      filterEmpty.hidden = true;
      sessions.appendChild(filterEmpty);
      search.addEventListener("input", function () {
        if (!filterEmpty.isConnected) { sessions.appendChild(filterEmpty); }
        var query = search.value.trim().toLocaleLowerCase("es");
        var cards = sessions.querySelectorAll(".nx-session, .nova-session-card, .conversation-item, [data-chat-subject]");
        var visible = 0;
        cards.forEach(function (card) {
          var match = !query || (card.textContent || "").toLocaleLowerCase("es").includes(query);
          card.hidden = !match;
          if (match) { visible += 1; }
        });
        sessions.querySelectorAll(".nx-session-group, .nova-session-group").forEach(function (group) { group.hidden = !!query; });
        filterEmpty.hidden = !query || visible > 0;
      });
    }

    document.addEventListener("click", function (e) {
      var starter = e.target.closest && e.target.closest("[data-chat-prompt]");
      if (starter && layout.contains(starter) && promptInput) {
        e.preventDefault();
        promptInput.value = starter.getAttribute("data-chat-prompt") || "";
        promptInput.focus();
        setHistory(false);
        return;
      }
      var card = e.target.closest && e.target.closest(".nx-session, .nova-session-card, .conversation-item, [data-chat-subject]");
      if (card && layout.contains(card)) {
        if (sessions) {
          sessions.querySelectorAll(".is-active").forEach(function (item) { item.classList.remove("is-active"); });
          card.classList.add("is-active");
        }
        setHistory(false);
      }
    }, true);
  }

  function initAssistant() {
    var promptInput = document.getElementById("prompt-input");
    var sendBtn = document.getElementById("send-btn");
    if (!promptInput || !sendBtn) { return; }

    function ctx() {
      try { return JSON.parse(sessionStorage.getItem("sp:ai-ctx") || "null"); }
      catch (e) { return null; }
    }
    function buildPrompt(t) {
      var c = ctx();
      var out = t || "";
      if (c && c.c) { out += "\n\n---\nMaterial (\u00ab" + c.t + "\u00bb):\n" + String(c.c).slice(0, 5000); }
      return out.slice(0, 7600);
    }

    // Las acciones del composer tienen un único controlador en asistente.html.
    // shell.js solo aporta el contexto compartido; no vuelve a enviar el chip.

    var pending = sessionStorage.getItem("sp:ai-prompt");
    if (pending) {
      promptInput.value = buildPrompt(pending);
      sessionStorage.removeItem("sp:ai-prompt");
      promptInput.focus();
      SP.toast("Mensaje listo: pulsa Enviar", "info");
    } else if (ctx()) {
      SP.toast("Material adjuntado: usa una sugerencia o escribe tu pregunta", "info", 5200);
    }

    ["attach_file", "mic"].forEach(function (needle) {
      var b = null;
      var composer = promptInput.closest(".flex.items-center") || promptInput.parentNode;
      document.querySelectorAll("main button, #contenido button").forEach(function (btn) {
        var ic = btn.querySelector("span.material-symbols-outlined");
        if (ic && txt(ic) === needle) { b = btn; }
      });
      if (!b) { return; }
      once(b, needle, function (btn) {
        if (needle === "attach_file") {
          btn.addEventListener("click", function () {
            SP.api("/api/notes").then(function (data) {
              var notes = data.notes || [];
              if (!notes.length) { SP.toast("Todavía no hay notas", "info"); return; }
              var items = notes.slice(0, 20).map(function (n) {
                return { label: n.title, icon: "description", sub: "nota", value: n.slug };
              });
              SP.dialog({ title: "Adjuntar una nota", items: items }).then(function (slug) {
                if (!slug || typeof slug !== "string") { return; }
                SP.api("/api/notes/" + slug).then(function (d) {
                  var c = String(d.content || "");
                  var n = d.note || {};
                  if (!c.trim()) { SP.toast("Esa nota está vacía", "info"); return; }
                  sessionStorage.setItem("sp:ai-ctx", JSON.stringify({ t: n.title || slug, c: c }));
                  promptInput.value = buildPrompt("Basandote en el material adjunto:");
                  promptInput.focus();
                  SP.toast("Nota adjuntada: pulsa Enviar o una sugerencia", "ok");
                }).catch(function (err) { SP.toast(msgErr(err), "err"); });
              });
            }).catch(function (err) { SP.toast(msgErr(err), "err"); });
          });
          return;
        }
        var SR = window.SpeechRecognition || window.webkitSpeechRecognition;
        if (!SR) { SP.toast("Tu navegador no admite el dictado", "err"); return; }
        btn.addEventListener("click", function () {
          if (btn.dataset.rec === "1") { return; }
          var rec;
          try { rec = new SR(); } catch (e2) { SP.toast("No se pudo iniciar el micrófono", "err"); return; }
          rec.lang = document.documentElement.lang || "es";
          rec.interimResults = false;
          rec.maxAlternatives = 1;
          btn.dataset.rec = "1";
          var stop2 = function () { btn.dataset.rec = ""; };
          rec.onresult = function (ev) {
            var t = (ev.results && ev.results[0][0] && ev.results[0][0].transcript) || "";
            promptInput.value = (promptInput.value ? promptInput.value + " " : "") + t;
            promptInput.focus();
          };
          rec.onerror = function (ev) { SP.toast("Dictado: " + ((ev && ev.error) || "error"), "err"); stop2(); };
          rec.onend = stop2;
          try {
            rec.start();
            SP.toast("Escuchando…", "info");
          } catch (e3) { stop2(); }
        });
      });
    });
  }

  /* =================================================================
   *  Renderizador markdown viajero (preview del editor)
   * ================================================================= */
  function renderMarkupInto(container, md, notes) {
    var label = container.querySelector(".nx-pane-label");
    container.textContent = "";
    if (label) { container.appendChild(label); }
    container.appendChild(renderMarkup(md, notes));
  }
  function renderMarkup(md, notes) {
    var frag = document.createDocumentFragment();
    var titlesIndex = {};
    (notes || []).forEach(function (n) { titlesIndex[String(n.title).toLowerCase()] = n.slug; });
    var lines = String(md || "").split("\n");
    var inCode = false;
    var codeBuf = [];
    var listEl = null;
    var listTag = null;

    function closeList() { listEl = null; listTag = null; }
    function ensureList(tag) {
      if (!listEl || listTag !== tag) {
        listEl = document.createElement(tag);
        listTag = tag;
        frag.appendChild(listEl);
      }
      return listEl;
    }
    function inline(s) {
      s = String(s)
        .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
        .replace(/`([^`]+)`/g, "<code>$1</code>")
        .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
        .replace(/\*([^*]+)\*/g, "<em>$1</em>")
        .replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, function (_, txt2, href) {
          var destino = /^(https?:|mailto:|\/|#)/i.test(href) ? href : "#";
          return '<a href="' + destino.replace(/"/g, "&quot;") + '">' + txt2 + "</a>";
        });
      s = s.replace(/\[\[([^\]]+)\]\]/g, function (_, t) {
        var name = t.trim().replace(/"/g, "&quot;");
        var slug = titlesIndex[name.toLowerCase()];
        return "<a href=\"" + (slug ? "/app/notes/" + slug : "/app/notes") + "\">" + name + "</a>";
      });
      return s;
    }
    function node(tag, html) {
      var n = document.createElement(tag);
      n.innerHTML = html;
      return n;
    }
    function preNode() {
      var p = document.createElement("pre");
      p.textContent = codeBuf.join("\n");
      codeBuf = [];
      return p;
    }

    lines.forEach(function (raw) {
      var line = raw;
      if (line.trimStart().slice(0, 3) === "```") {
        if (inCode) { closeList(); frag.appendChild(preNode()); inCode = false; }
        else { closeList(); inCode = true; }
        return;
      }
      if (inCode) { codeBuf.push(line); return; }
      var t = line.trim();
      if (!t) { closeList(); return; }
      var h = /^(#{1,6})\s+(.*)$/.exec(t);
      if (h) {
        closeList();
        frag.appendChild(node("h" + Math.min(6, h[1].length), inline(h[2])));
        return;
      }
      if (/^[-*]\s*\[[ xX]\]\s*/.test(t)) {
        var li = document.createElement("li");
        var done2 = /\[[xX]\]/.test(t);
        li.appendChild(el("span", null, done2 ? "\u2611" : "\u2610"));
        li.appendChild(document.createTextNode(t.replace(/^[-*]\s*\[[ xX]\]\s*/, "")));
        li.style.cssText = "list-style:none;display:flex;gap:.4rem;align-items:baseline";
        closeList();
        var taskList = ensureList("ul");
        frag.appendChild(taskList); // appendChild lo mueve: no-op si ya esta
        taskList.appendChild(li);
        closeList();
        return;
      }
      if (/^[-*]\s+/.test(t)) {
        ensureList("ul").appendChild(node("li", inline(t.slice(2))));
        return;
      }
      if (/^\d+\.\s+/.test(t)) {
        ensureList("ol").appendChild(node("li", inline(t.replace(/^\d+\.\s+/, ""))));
        return;
      }
      if (/^>\s?/.test(t)) {
        closeList();
        frag.appendChild(node("blockquote", inline(t.replace(/^>\s?/, ""))));
        return;
      }
      closeList();
      frag.appendChild(node("p", inline(t)));
    });
    if (inCode && codeBuf.length) { frag.appendChild(preNode()); }
    return frag;
  }

  function boot() {
    injectCss();
    initSearch();
    initBell();
    initTopbarCreate();
    initDashboards();
    initCollectionsSafe();
    initVaultActions();
    initCalendar();
    initModelPicker();
    initNotesEditor();
    initChatWorkspace();
    initAssistant();
    initSettings();
    initAiLegacy();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
