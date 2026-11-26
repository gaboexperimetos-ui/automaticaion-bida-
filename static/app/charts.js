/*! Study Platform charts.js — interactividad de los gráficos SVG del inicio.
 *
 * Servido en /charts.js e inyectado al final del <body> por el shell.
 * Sin dependencias. No toca el HTML servido: aumenta el DOM ya montado.
 *
 * Qué añade a cada `.nx-chart`:
 *   - Tooltip flotante con el dato real (lee el <title> de cada marca).
 *   - Resaltado: al pasar por una marca se atenúa el resto del SVG.
 *   - Barras y celdas: elevación sutil al enfocar.
 *   - Leyenda clicable (gráfico apilado): apaga/enciende un tipo de evaluación.
 */
(function () {
  "use strict";
  if (window.__SP_CHARTS__) return;
  window.__SP_CHARTS__ = true;

  var tip = null;
  function tooltip() {
    if (tip) return tip;
    tip = document.createElement("div");
    tip.className = "nx-chart-tip";
    tip.setAttribute("role", "tooltip");
    tip.style.opacity = "0";
    document.body.appendChild(tip);
    return tip;
  }

  function place(x, y) {
    var t = tooltip();
    var pad = 12;
    var r = t.getBoundingClientRect();
    var left = x + pad;
    var top = y + pad;
    if (left + r.width > window.innerWidth - 8) left = x - r.width - pad;
    if (top + r.height > window.innerHeight - 8) top = y - r.height - pad;
    t.style.left = Math.max(8, left) + "px";
    t.style.top = Math.max(8, top) + "px";
  }

  function show(text, x, y) {
    var t = tooltip();
    t.textContent = text;
    t.style.opacity = "1";
    place(x, y);
  }
  function hide() {
    if (tip) tip.style.opacity = "0";
  }

  // Texto del tooltip: el <title> de la marca si existe; si no, aria-label.
  function markText(el) {
    var title = el.querySelector && el.querySelector("title");
    if (title && title.textContent.trim()) return title.textContent.trim();
    var own = el.getAttribute && el.getAttribute("title");
    if (own) return own;
    return "";
  }

  function isMark(el) {
    if (!el || el.nodeType !== 1) return false;
    var tag = el.tagName.toLowerCase();
    return tag === "rect" || tag === "circle" || tag === "polygon" ||
      tag === "path" || tag === "line";
  }

  function wire(svg) {
    if (svg.__nxWired) return;
    svg.__nxWired = true;

    svg.addEventListener("mousemove", function (ev) {
      var mark = ev.target.closest ? ev.target.closest("rect,circle,polygon,path") : null;
      if (!mark || !svg.contains(mark)) { hide(); return; }
      var text = markText(mark);
      if (!text) { hide(); return; }
      show(text, ev.clientX, ev.clientY);
    });
    svg.addEventListener("mouseleave", hide);

    // Resaltado: atenúa hermanos al enfocar una marca con datos.
    svg.addEventListener("mouseover", function (ev) {
      var mark = ev.target.closest ? ev.target.closest("rect,circle,polygon") : null;
      if (!mark || !markText(mark)) return;
      svg.classList.add("is-hovering");
      mark.classList.add("is-hot");
    });
    svg.addEventListener("mouseout", function (ev) {
      var mark = ev.target.closest ? ev.target.closest("rect,circle,polygon") : null;
      if (mark) mark.classList.remove("is-hot");
      if (!svg.querySelector(".is-hot")) svg.classList.remove("is-hovering");
    });
  }

  // Leyenda clicable del gráfico apilado: alterna la serie por índice.
  function wireStacked(svg) {
    var texts = svg.querySelectorAll("text.nx-chart-axis");
    if (!texts.length) return;
    // La leyenda son los últimos 4 textos de eje (Tareas/Trabajos/Eval/Examen).
    var legend = Array.prototype.slice.call(texts).slice(-4);
    var labels = { Tareas: 0, Trabajos: 1, Evaluaciones: 2, Examen: 3 };
    legend.forEach(function (t) {
      var key = Object.keys(labels).filter(function (k) {
        return t.textContent.indexOf(k) === 0;
      })[0];
      if (!key) return;
      var ci = labels[key];
      t.classList.add("nx-chart-legend-toggle");
      t.setAttribute("tabindex", "0");
      t.setAttribute("role", "button");
      t.setAttribute("aria-pressed", "true");
      function toggle() {
        var on = t.getAttribute("aria-pressed") !== "false";
        t.setAttribute("aria-pressed", on ? "false" : "true");
        t.classList.toggle("is-off", on);
        svg.querySelectorAll("rect[data-series]").forEach(function (r) {
          if (Number(r.getAttribute("data-series")) === ci) {
            r.style.display = on ? "none" : "";
          }
        });
      }
      t.addEventListener("click", toggle);
      t.addEventListener("keydown", function (e) {
        if (e.key === "Enter" || e.key === " ") { e.preventDefault(); toggle(); }
      });
    });
  }

  function init() {
    document.querySelectorAll(".nx-chart").forEach(function (svg) {
      wire(svg);
      if (svg.getAttribute("aria-label").toLowerCase().indexOf("aporte") !== -1) wireStacked(svg);
    });
  }

  var CSS =
    ".nx-chart-tip{position:fixed;z-index:130;max-width:240px;padding:6px 9px;border-radius:8px;" +
    "background:rgba(14,21,38,.92);color:#f3f6ff;border:1px solid rgba(148,163,184,.28);" +
    "box-shadow:0 10px 30px rgba(0,0,0,.4);backdrop-filter:blur(12px);-webkit-backdrop-filter:blur(12px);" +
    "font:600 11px/1.35 'Inter',sans-serif;pointer-events:none;transition:opacity .12s ease;opacity:0}" +
    ".nx-chart.is-hovering rect,.nx-chart.is-hovering circle,.nx-chart.is-hovering polygon{opacity:.34;transition:opacity .12s ease}" +
    ".nx-chart.is-hovering .is-hot{opacity:1!important}" +
    ".nx-chart rect,.nx-chart circle,.nx-chart polygon{transition:opacity .12s ease}" +
    ".nx-chart-legend-toggle{cursor:pointer;user-select:none}" +
    ".nx-chart-legend-toggle:hover{fill:var(--nx-accent,#a78bfa)}" +
    ".nx-chart-legend-toggle.is-off{opacity:.4;text-decoration:line-through}";

  function boot() {
    var s = document.createElement("style");
    s.textContent = CSS;
    document.head.appendChild(s);
    init();
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();