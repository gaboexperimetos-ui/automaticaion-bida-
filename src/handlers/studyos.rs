//! Frontend StudyOS — interfaz HTML/CSS/JS pura (desconectado del router).
//!
//! Este módulo sirvió originalmente la interfaz `static/studyos/` (index.html,
//! styles.css, app.js) en las rutas `GET /`, `GET /dashboard`, `GET /styles.css`
//! y `GET /app.js`. Hoy sus handlers **no están cableados**: ninguna función
//! de este archivo es referenciada desde el resto del crate y `mod studyos;`
//! (en `handlers/mod.rs`) no registra ninguna ruta.
//!
//! Rutas equivalentes que hoy sí sirven la interfaz:
//!   - `GET /`, `GET /dashboard`      -> `stitch_v2::stitch_index`
//!   - `GET /legacy`, `/styles.css`, `/app.js` -> `static_ui::*`
//!
//! Los assets de `static/studyos/` siguen existiendo en disco pero no los
//! sirve ningún handler. Si hay que resucitar esta interfaz, hay que añadir
//! las rutas en `handlers/mod.rs` (o migrar los assets a `static/stitch-v2/`).
//!
//! Sin handlers: no hay nada que exportar desde aquí.
