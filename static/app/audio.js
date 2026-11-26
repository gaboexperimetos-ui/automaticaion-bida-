// Audio NOVA: graba con MediaRecorder, sube un archivo, transcribe por Groq
// (endpoint del servidor) y ofrece guardar el resultado como nota.
// Todo el audio crudo se manda al servidor; la transcripción la hace Groq.
(function () {
  "use strict";

  const recordBtn = document.getElementById("audio-record");
  const stopBtn = document.getElementById("audio-stop");
  const timerEl = document.getElementById("audio-timer");
  const preview = document.getElementById("audio-preview");
  const statusEl = document.getElementById("audio-status");
  const fileInput = document.getElementById("audio-file");
  const uploadStatus = document.getElementById("audio-upload-status");
  const textEl = document.getElementById("audio-text");
  const saveNote = document.getElementById("audio-save-note");

  if (!recordBtn && !fileInput) return; // página de audio no presente

  let mediaRecorder = null;
  let chunks = [];
  let startedAt = 0;
  let timerHandle = null;
  let lastNoteSlug = "";

  function setStatus(el, msg) {
    if (el) el.textContent = msg;
  }

  function fmt(seconds) {
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    return m + ":" + String(s).padStart(2, "0");
  }

  async function transcribe(blob, ext) {
    setStatus(statusEl, "Transcribiendo…");
    if (textEl) textEl.textContent = "Procesando audio…";
    if (saveNote) saveNote.hidden = true;
    const res = await fetch("/api/audio/transcribe?lang=es", {
      method: "POST",
      headers: { "Content-Type": "audio/" + ext, "X-Audio-Ext": ext },
      body: blob,
    });
    if (!res.ok) {
      let detail = "No se pudo transcribir.";
      try {
        const j = await res.json();
        if (j && j.error) detail = j.error;
      } catch (_) {}
      setStatus(statusEl, detail);
      if (textEl) textEl.textContent = detail;
      return;
    }
    const data = await res.json();
    if (textEl) textEl.textContent = data.text || "(sin texto)";
    lastNoteSlug = data.note || "";
    setStatus(
      statusEl,
      "Listo. " +
        (data.duration_seconds ? data.duration_seconds.toFixed(1) + " s · " : "") +
        (data.note ? "Guardado como nota." : "Transcripción obtenida."),
    );
    if (saveNote && data.note) {
      saveNote.hidden = false;
      saveNote.href = "/dashboard/notes?note=" + encodeURIComponent(data.note);
    }
  }

  function extFromType(type) {
    if (!type) return "webm";
    if (type.indexOf("ogg") >= 0) return "ogg";
    if (type.indexOf("mp4") >= 0 || type.indexOf("m4a") >= 0) return "m4a";
    if (type.indexOf("mpeg") >= 0 || type.indexOf("mp3") >= 0) return "mp3";
    if (type.indexOf("wav") >= 0) return "wav";
    return "webm";
  }

  if (recordBtn) {
    recordBtn.addEventListener("click", async () => {
      if (!navigator.mediaDevices || !window.MediaRecorder) {
        setStatus(statusEl, "Este navegador no permite grabar. Sube un archivo.");
        return;
      }
      try {
        const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
        chunks = [];
        mediaRecorder = new MediaRecorder(stream);
        mediaRecorder.ondataavailable = (e) => {
          if (e.data && e.data.size > 0) chunks.push(e.data);
        };
        mediaRecorder.onstop = () => {
          stream.getTracks().forEach((t) => t.stop());
          const blob = new Blob(chunks, { type: mediaRecorder.mimeType || "audio/webm" });
          if (preview) {
            preview.src = URL.createObjectURL(blob);
            preview.hidden = false;
          }
          transcribe(blob, extFromType(blob.type));
        };
        mediaRecorder.start();
        startedAt = Date.now();
        if (stopBtn) stopBtn.disabled = false;
        recordBtn.disabled = true;
        setStatus(statusEl, "Grabando… pulsa Detener al terminar.");
        timerHandle = setInterval(() => {
          if (timerEl) timerEl.textContent = fmt(Math.floor((Date.now() - startedAt) / 1000));
        }, 500);
      } catch (err) {
        setStatus(statusEl, "No se pudo acceder al micrófono: " + err);
      }
    });
  }

  if (stopBtn) {
    stopBtn.addEventListener("click", () => {
      if (mediaRecorder && mediaRecorder.state === "recording") {
        mediaRecorder.stop();
        clearInterval(timerHandle);
        if (stopBtn) stopBtn.disabled = true;
        if (recordBtn) recordBtn.disabled = false;
      }
    });
  }

  if (fileInput) {
    fileInput.addEventListener("change", () => {
      const file = fileInput.files && fileInput.files[0];
      if (!file) return;
      const ext = (file.name.split(".").pop() || extFromType(file.type)).toLowerCase();
      setStatus(uploadStatus, "Subiendo " + file.name + "…");
      if (preview) {
        preview.src = URL.createObjectURL(file);
        preview.hidden = false;
      }
      transcribe(file, ext);
    });
  }
})();