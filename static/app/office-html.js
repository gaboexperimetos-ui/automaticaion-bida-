/* Editor de HTML: la vista previa no hereda el origen de NOVA ni ejecuta scripts. */
(function (root) {
  "use strict";

  var PREVIEW_POLICY = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:; connect-src 'none'; media-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; navigate-to 'none'\">";

  function previewDocument(source) {
    // Place CSP before any authored markup. Injecting only into a matched
    // <head> can be bypassed by malformed input or a fake tag inside a comment,
    // allowing a resource to load before the policy is parsed. The HTML parser
    // creates the implicit html/head elements for this early meta tag.
    return "<!doctype html>" + PREVIEW_POLICY + String(source || "");
  }

  var api = { previewDocument: previewDocument };
  root.NovaOfficeHtml = api;
  if (typeof module !== "undefined" && module.exports) module.exports = api;

  function initializeEditor() {
    var editor = root.document.getElementById("editor");
    var frame = root.document.getElementById("office-html-preview-frame");
    if (!editor || !frame || editor.dataset.previewReady) return;
    editor.dataset.previewReady = "true";
    var status = root.document.getElementById("office-html-status");
    var byteCount = root.document.getElementById("office-html-byte-count");
    var refresh = root.document.getElementById("office-html-refresh");
    var timer = 0;

    function renderPreview() {
      frame.srcdoc = previewDocument(editor.value);
      if (byteCount) byteCount.textContent = new Blob([editor.value]).size.toLocaleString() + " bytes";
    }

    editor.addEventListener("input", function () {
      if (status) status.textContent = "Cambios sin guardar · la vista previa no ejecuta scripts ni carga recursos de red.";
      root.clearTimeout(timer);
      timer = root.setTimeout(renderPreview, 180);
    });
    if (refresh) refresh.addEventListener("click", renderPreview);

    var form = root.document.getElementById("office-html-save-form");
    if (form) {
      form.addEventListener("submit", function (event) {
        if (new Blob([editor.value]).size > 1024 * 1024) {
          event.preventDefault();
          if (status) status.textContent = "El HTML supera el límite de 1 MiB y no se guardó.";
          if (root.__SP__ && typeof root.__SP__.toast === "function") {
            root.__SP__.toast("El HTML supera el límite de 1 MiB.", "err");
          }
        }
      });
    }
    renderPreview();
  }

  if (root.document) {
    if (root.document.readyState === "loading") root.document.addEventListener("DOMContentLoaded", initializeEditor);
    else initializeEditor();
  }
})(typeof window !== "undefined" ? window : globalThis);
