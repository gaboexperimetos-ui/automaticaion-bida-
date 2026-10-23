//! Panel del segundo cerebro (Stitch v2): inyección de notas reales.
//!
//! Reemplaza los marcadores `<!--SLOT:...-->` del HTML del kit Stitch
//! (`static/stitch-v2/_content/clean/notes.html`) por datos reales de la
//! base local (SQLite) y de los archivos `.md` de la bóveda: total real de
//! notas, lista del explorador, etiquetas agregadas y la nota más reciente
//! con su contenido, wikilinks salientes y backlinks. Las zonas sin fuente
//! de datos real (grafo, backlinks vacíos) degradan a un estado vacío legible
//! en vez de mostrar los datos de ejemplo del kit.
//!
//! La nota destacada trae además sus acciones reales: editar (`/app/notes/
//! {slug}/edit`) y eliminar (`DELETE /app/notes/{slug}/delete`, con el mismo
//! `<details>` de confirmación inline que usa `handlers::notes`). La descarga
//! del `.md` sólo aparece cuando su ruta existe en el router.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::json;
use sqlx::Row;

use crate::AppState;

/// Máximo de notas que se leen y escanean en disco por carga de página.
const SCAN_LIMIT: usize = 300;
/// Máximo de filas del explorador antes de resumir con "+N más".
const LIST_LIMIT: usize = 40;
/// Máximo de nodos periféricos que se dibujan en el grafo.
const GRAPH_NODES_MAX: usize = 8;
/// Máximo de etiquetas que se muestran.
const TAGS_MAX: usize = 12;
/// Mínimo de notas para que tenga sentido un bloque "Recientes"
/// (con menos, el explorador ya es la lista completa).
const RECENT_MIN: usize = 6;
/// Caracteres del resumen de una línea que se muestra bajo cada nota.
const SNIPPET_CHARS: usize = 140;

/// Clase Tailwind de un chip de etiqueta inactivo: mismo estilo que las
/// etiquetas del panel, con estado hover y activo.
const CHIP_CLASS_OFF: &str = "px-2 py-0.5 rounded bg-surface-container text-secondary font-label-sm text-[11px] cursor-pointer hover:bg-primary-container/15 hover:text-primary-container transition-colors";

/// Clase Tailwind de un chip de etiqueta activo.
const CHIP_CLASS_ON: &str = "px-2 py-0.5 rounded bg-primary-container text-on-primary-container font-label-sm text-[11px] cursor-pointer font-semibold transition-colors";

/// Script de filtrado del explorador por etiqueta.
///
/// El panel es una página estática (no recibe parámetros de consulta), así que
/// el filtro opera sobre las filas ya renderizadas —cada una lleva sus
/// slugs en `data-stitch-tags`— en lugar de pedir datos al servidor. Los
/// chips de `render_tags` marcan la etiqueta activa, el script deja visibles
/// solo las notas que la llevan y los estados vacíos se derivan de los datos
/// reales (nunca se inventan conteos).
///
/// `/*__CFG__*/` se sustituye por un literal JSON con las clases de los chips
/// y el límite de filas visibles.
const TAG_FILTER_SCRIPT: &str = r##"(function () {
  "use strict";

  var CFG = (/*__CFG__*/ || {});

  function all(sel) {
    return Array.prototype.slice.call(document.querySelectorAll(sel));
  }

  function slugsOf(row) {
    return (row.getAttribute("data-stitch-tags") || "").split(" ");
  }

  function hasTag(row, tag) {
    return slugsOf(row).indexOf(tag) !== -1;
  }

  function labelFor(tag) {
    var chip = document.querySelector('[data-stitch-tag="' + tag + '"]');
    return chip ? (chip.getAttribute("data-stitch-label") || tag) : tag;
  }

  function apply(tag) {
    var active = tag || "";
    var shown = 0;
    var matches = 0;

    all('[data-stitch-row="list"]').forEach(function (row) {
      if (active && !hasTag(row, active)) {
        row.classList.add("hidden");
        return;
      }
      matches += 1;
      if (shown < CFG.limit) {
        row.classList.remove("hidden");
        shown += 1;
      } else {
        row.classList.add("hidden");
      }
    });

    var recentVisible = 0;
    all('[data-stitch-row="recent"]').forEach(function (row) {
      var ok = !active || hasTag(row, active);
      row.classList.toggle("hidden", !ok);
      if (ok) { recentVisible += 1; }
    });
    var recentBox = document.getElementById("stitch-recent");
    if (recentBox) {
      recentBox.classList.toggle("hidden", !!active && recentVisible === 0);
    }

    var more = document.querySelector("[data-stitch-more]");
    if (more) {
      var rest = matches - shown;
      more.textContent = rest > 0
        ? "… y " + rest + " más" + (active ? " con esta etiqueta" : " en la bóveda")
        : "";
      more.classList.toggle("hidden", rest <= 0);
    }

    var empty = document.getElementById("stitch-tag-empty");
    if (empty) {
      if (active && shown === 0) {
        empty.textContent = "Ninguna nota tiene la etiqueta #" + labelFor(active) + ".";
        empty.classList.remove("hidden");
      } else {
        empty.textContent = "";
        empty.classList.add("hidden");
      }
    }

    all("[data-stitch-tag]").forEach(function (chip) {
      var on = (chip.getAttribute("data-stitch-tag") || "") === active;
      if (chip.getAttribute("data-stitch-chip") === "filter") {
        chip.className = on ? CFG.on : CFG.off;
      } else {
        chip.classList.toggle("bg-primary-container", on);
        chip.classList.toggle("text-on-primary-container", on);
        chip.classList.toggle("bg-surface-container", !on);
        chip.classList.toggle("text-secondary", !on);
      }
      chip.setAttribute("aria-pressed", on ? "true" : "false");
    });

    var status = document.getElementById("stitch-tag-status");
    if (status) {
      if (active) {
        status.textContent = shown === 1
          ? "1 nota visible con #" + labelFor(active)
          : shown + " notas visibles con #" + labelFor(active);
      } else {
        status.textContent = "";
      }
      status.classList.toggle("hidden", !active);
    }
  }

  document.addEventListener("click", function (e) {
    var chip = e.target && e.target.closest ? e.target.closest("[data-stitch-tag]") : null;
    if (!chip) { return; }
    e.preventDefault();
    apply(chip.getAttribute("data-stitch-tag") || "");
  });

  function boot() {
    if (!document.querySelector("[data-stitch-tag]")) { return; }
    apply("");
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();"##;

/// Plantilla de la ruta de descarga del `.md` de la nota.
///
/// La ruta **existe** en el router: `src/handlers/mod.rs` la registra como
/// `.route("/app/notes/{slug}/export", get(notes::export))` (verificado en
/// vivo: responde 200 con `Content-Disposition: attachment`). El botón sólo
/// se renderiza si la ruta está registrada, porque enlazar a una ruta
/// inexistente es un 404 al pulsarlo: si algún día se quita del router,
/// aquí va `None` y el enlace desaparece con ella.
const EXPORT_ROUTE_TEMPLATE: Option<&str> = Some("/app/notes/{slug}/export");

/// Script de eliminación de la nota destacada.
///
/// El shell de Stitch v2 no carga htmx (`shell/head.html` no lo incluye),
/// así que el `<details>` de confirmación replica el diálogo de
/// `handlers::notes` pero el envío se hace con `fetch` en lugar de
/// `hx-delete`. La redirección final replica el `HX-Redirect: /app/notes` que
/// devuelve `notes::delete` tras borrar.
///
/// El disparador es un `<button type="button">`, no un `<form>`: la ruta
/// `/app/notes/{slug}/delete` sólo acepta `DELETE`, así que un `POST` de
/// formulario (fallback sin JavaScript) respondería 405. Sin JS el botón no
/// hace nada, que es preferible a un envío a una ruta que rechaza el método.
const DELETE_NOTE_SCRIPT: &str = r##"(function () {
  "use strict";

  function each(list, fn) {
    Array.prototype.forEach.call(list, fn);
  }

  function showMsg(box, text) {
    if (!box) { return; }
    box.textContent = text;
    box.classList.remove("hidden");
  }

  each(document.querySelectorAll("[data-stitch-delete-btn]"), function (btn) {
    btn.addEventListener("click", function () {
      var url = btn.getAttribute("data-delete-url");
      var details = btn.closest("details");
      var msg = details ? details.querySelector("[data-stitch-delete-msg]") : null;
      btn.disabled = true;
      showMsg(msg, "Eliminando…");
      window.fetch(url, { method: "DELETE", credentials: "include" })
        .then(function (res) {
          if (!res.ok) { throw new Error("HTTP " + res.status); }
          window.location.href = "/app/notes";
        })
        .catch(function (err) {
          btn.disabled = false;
          showMsg(msg, "No se pudo eliminar la nota (" + err.message + ").");
        });
    });
  });
})();"##;

/// Nota real de la base local: (id, título, slug, tags, actualizada).
pub struct NoteRow {
    pub id: i64,
    pub title: String,
    pub slug: String,
    /// Nombre real del archivo `.md` en la bóveda (`notes.filename`).
    pub filename: String,
    pub tags: String,
    pub updated_at: String,
    /// Resumen real de una línea (primera frase del cuerpo leído del disco).
    /// Vacío si el archivo no existe o está vacío: nunca se inventa.
    pub snippet: String,
}

/// Etiqueta agregada: cuántas notas la usan.
pub struct TagCount {
    /// Etiqueta tal como se escribió en la nota (primera variante vista).
    pub tag: String,
    /// Slug comparable de la etiqueta (minúsculas, sin acentos).
    pub slug: String,
    /// Notas de la bóveda que la usan.
    pub count: usize,
}

/// Enlace saliente `[[...]]` de la nota destacada.
pub struct LinkRow {
    /// Slug resuelto del destino.
    pub slug: String,
    /// Texto visible escrito por la autora (`alias` o el propio target).
    pub display: String,
    /// Título real de la nota destino (vacío si aún no existe).
    pub title: String,
    /// `true` si el destino existe en la bóveda.
    pub exists: bool,
}

/// Backlink real: la nota fuente que menciona a la destacada.
pub struct BacklinkRow {
    pub from_slug: String,
    pub from_title: String,
    pub updated_at: String,
    /// Línea del archivo fuente con la mención, escapada y resaltada.
    pub snippet: String,
}

/// Nota destacada (la más recientemente actualizada) con su contenido real.
pub struct FeaturedNote {
    pub title: String,
    pub slug: String,
    pub tags: String,
    pub updated_at: String,
    pub word_count: i64,
    /// Resumen real de una línea de la nota destacada (leído del disco).
    pub snippet: String,
    /// Markdown real (sin frontmatter) leído del disco.
    pub content: String,
    pub outlinks: Vec<LinkRow>,
    pub backlinks: Vec<BacklinkRow>,
}

#[derive(Default)]
pub struct NotesPanelData {
    pub notes: Vec<NoteRow>,
    /// Total real de notas en la base (no el tamaño de la muestra).
    pub total: usize,
    /// Suma de `word_count` de toda la bóveda.
    pub total_words: i64,
    /// Destinos distintos enlazados, sumado sobre todas las notas escaneadas.
    pub total_links: usize,
    pub tags: Vec<TagCount>,
    pub featured: Option<FeaturedNote>,
    /// Ruta real de la bóveda (`notes_dir` de la configuración).
    pub vault_path: String,
    /// slug -> título, para resolver wikilinks en la vista previa.
    pub titles: HashMap<String, String>,
}

/// Recolecta los datos de notas: base local + escaneo de archivos `.md`.
///
/// Nunca falla: ante un error deja el campo correspondiente vacío.
pub async fn fetch_notes_data(state: &AppState) -> NotesPanelData {
    let mut d = NotesPanelData::default();
    d.vault_path = state.config.notes_dir.display().to_string();

    // 1) Metadatos de todas las notas (una consulta). Los tags viven
    //    agregados por comas en una sola columna, así que el DISTINCT de
    //    etiquetas se hace en memoria.
    // Fila con `slug` vacío = nota inútil: ninguna ruta (`/app/notes/{slug}`,
    // `/app/notes/{slug}/edit`, `/app/notes/{slug}/export`,
    // `/app/notes/{slug}/delete`) la puede direccionar, así que sus enlaces
    // quedarían rotos (p. ej. `/app/notes//export`). Ocurría al crear una
    // nota sin título (su `slugify` da "") y, como la nota se ordenaba por
    // `updated_at DESC`, esa fila garbage se colaba como nota destacada con
    // las tres acciones apuntando a la nada. Se excluye en SQL para que el
    // listado y el total del panel cuenten exactamente las mismas notas.
    let rows = sqlx::query(
        "SELECT id, title, slug, filename, tags, word_count, updated_at \
         FROM notes WHERE TRIM(slug) != '' ORDER BY updated_at DESC LIMIT ?",
    )
    .bind(SCAN_LIMIT as i64)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut word_by_slug: HashMap<String, i64> = HashMap::new();
    let mut tag_counts: HashMap<String, (String, usize)> = HashMap::new();
    for r in rows {
        let id = r.try_get::<i64, _>("id").unwrap_or(0);
        let title: String = r.try_get("title").unwrap_or_default();
        let slug: String = r.try_get("slug").unwrap_or_default();
        let filename: String = r.try_get("filename").unwrap_or_default();
        let tags: String = r.try_get("tags").unwrap_or_default();
        let updated_at: String = r.try_get("updated_at").unwrap_or_default();
        let word_count = r.try_get::<i64, _>("word_count").unwrap_or(0);

        word_by_slug.insert(slug.clone(), word_count);
        d.titles.insert(slug.clone(), title.clone());
        // Las etiquetas se agregan por slug (mismo criterio que la página
        // /app/notes): "Cálculo" y "calculo" son la misma etiqueta.
        for t in tags.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let tag_key = tag_slug(t);
            if tag_key.is_empty() {
                continue;
            }
            let entry = tag_counts
                .entry(tag_key)
                .or_insert_with(|| (t.to_string(), 0usize));
            entry.1 += 1;
        }
        d.notes.push(NoteRow {
            id,
            title,
            slug,
            filename,
            tags,
            updated_at,
            snippet: String::new(),
        });
    }
    // Total real e inversión total en palabras: `COUNT(*)`/`SUM` de toda la
    // tabla (el listado puede truncarse a SCAN_LIMIT). Cuenta las mismas
    // notas que el listado —las de `slug` no vacío, que son las únicas
    // direccionables— para que el número del panel cuadre con las filas
    // que se ven.
    if let Ok(row) = sqlx::query(
        "SELECT COUNT(*) AS total, COALESCE(SUM(word_count), 0) AS words \
         FROM notes WHERE TRIM(slug) != ''",
    )
    .fetch_one(&state.db)
    .await
    {
        d.total = row
            .try_get::<i64, _>("total")
            .unwrap_or(d.notes.len() as i64) as usize;
        d.total_words = row.try_get::<i64, _>("words").unwrap_or(0);
    } else {
        d.total = d.notes.len();
    }
    d.tags = tag_counts
        .into_iter()
        .map(|(slug, (tag, count))| TagCount { tag, slug, count })
        .collect();
    d.tags
        .sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));

    if d.notes.is_empty() {
        return d;
    }

    // 2) Escaneo de archivos .md con el parser de wikilinks de `knowledge`
    //    (mismo criterio que `KnowledgeService::backlinks`): total de
    //    enlaces internos, backlinks de la destacada y su contenido.
    let featured_slug = d.notes[0].slug.clone();
    let mut outlinks: Vec<LinkRow> = Vec::new();
    let mut out_seen: HashSet<String> = HashSet::new();
    let mut backlinks: Vec<BacklinkRow> = Vec::new();
    let mut back_seen: HashSet<String> = HashSet::new();
    let mut featured_content = String::new();
    // Resumen real de cada nota (primera frase del cuerpo, sin frontmatter).
    // Se rellena sólo con lo que hay en el disco: una nota sin archivo legible
    // se queda sin snippet.
    let mut snippets: HashMap<String, String> = HashMap::new();

    for note in &d.notes {
        let raw = std::fs::read_to_string(note_path(
            &state.config.notes_dir,
            &note.filename,
            &note.slug,
        ))
        .unwrap_or_default();
        if raw.trim().is_empty() {
            continue;
        }
        // Cuerpo real sin frontmatter (igual que `get_note`).
        let body = crate::knowledge::frontmatter::parse_note(&raw).content;
        let snippet = crate::knowledge::frontmatter::summarize(&body, SNIPPET_CHARS);
        if !snippet.trim().is_empty() {
            snippets.insert(note.slug.clone(), snippet);
        }
        // Destinos por nota: para "enlaces activos" cuenta destinos únicos.
        let mut seen_this_note: HashSet<String> = HashSet::new();
        for link in crate::knowledge::wikilinks::parse_wikilinks(&raw) {
            let slug = link.resolve();
            if seen_this_note.insert(slug.clone()) {
                d.total_links += 1;
            }
            if note.slug == featured_slug {
                if out_seen.insert(slug.clone()) {
                    outlinks.push(LinkRow {
                        display: link.display.clone().unwrap_or_else(|| link.target.clone()),
                        title: d.titles.get(&slug).cloned().unwrap_or_default(),
                        exists: d.titles.contains_key(&slug),
                        slug,
                    });
                }
            } else if slug == featured_slug && back_seen.insert(note.slug.clone()) {
                backlinks.push(BacklinkRow {
                    from_slug: note.slug.clone(),
                    from_title: note.title.clone(),
                    updated_at: note.updated_at.clone(),
                    snippet: mention_snippet(&raw, link.offset),
                });
            }
        }
        if note.slug == featured_slug {
            featured_content = body;
        }
    }

    // El resumen se asigna a cada nota ya escaneada.
    for n in d.notes.iter_mut() {
        if let Some(s) = snippets.get(&n.slug) {
            n.snippet = s.clone();
        }
    }

    let featured = &d.notes[0];
    d.featured = Some(FeaturedNote {
        title: featured.title.clone(),
        slug: featured.slug.clone(),
        tags: featured.tags.clone(),
        updated_at: featured.updated_at.clone(),
        word_count: word_by_slug.get(&featured.slug).copied().unwrap_or(0),
        snippet: featured.snippet.clone(),
        content: featured_content,
        outlinks,
        backlinks,
    });

    d
}

// ── render de slots ────────────────────────────────────────────────────────

/// Reemplaza los marcadores `<!--SLOT:...-->` por HTML con datos reales.
pub fn fill_notes(content: &str, d: &NotesPanelData) -> String {
    content
        .replace("<!--SLOT:VAULT_PATH-->", &render_vault_path(d))
        .replace("<!--SLOT:NOTES_COUNT-->", &render_count(d))
        .replace("<!--SLOT:NOTES_LIST-->", &render_list(d))
        .replace("<!--SLOT:NOTES_RECENT-->", &render_recent_block(d))
        .replace("<!--SLOT:TAGS-->", &render_tags(d))
        .replace(
            "<!--SLOT:NOTE_TITLE-->",
            &render_featured(|f| esc(&f.title), d),
        )
        .replace("<!--SLOT:NOTE_STATUS-->", &render_status(d))
        .replace("<!--SLOT:NOTE_TAGS-->", &render_note_tags(d))
        .replace("<!--SLOT:NOTE_STATS-->", &render_note_stats(d))
        .replace("<!--SLOT:NOTE_RAW-->", &render_raw(d))
        .replace("<!--SLOT:NOTE_PREVIEW-->", &render_preview(d))
        .replace("<!--SLOT:BACKLINKS_COUNT-->", &render_backlinks_count(d))
        .replace("<!--SLOT:BACKLINKS-->", &render_backlinks(d))
        .replace("<!--SLOT:GRAPH_META-->", &render_graph_meta(d))
        .replace("<!--SLOT:GRAPH-->", &render_graph(d))
        .replace("<!--SLOT:OUTLINKS_COUNT-->", &render_outlinks_count(d))
        .replace("<!--SLOT:OUTLINKS-->", &render_outlinks(d))
        .replace("<!--SLOT:GRAPH_STATS-->", &render_graph_stats(d))
}

/// Texto del slot solo si hay nota destacada (ruta genérica de vacío).
fn render_featured(f: impl Fn(&FeaturedNote) -> String, d: &NotesPanelData) -> String {
    match &d.featured {
        Some(n) => f(n),
        None => String::new(),
    }
}

fn render_vault_path(d: &NotesPanelData) -> String {
    if d.vault_path.is_empty() {
        "vault: local".to_string()
    } else {
        format!("vault: {}", esc(&d.vault_path))
    }
}

fn render_count(d: &NotesPanelData) -> String {
    match d.total {
        0 => "0 notas".to_string(),
        1 => "1 nota".to_string(),
        n => format!("{n} notas"),
    }
}

/// Explorador: filas del árbol con las notas reales (la más reciente, que es
/// la que muestra el editor, marcada como activa).
///
/// Todas las notas cargadas se renderizan (las que no caben en la lista
/// quedan ocultas) para que el filtro por etiqueta pueda mostrarlas.
fn render_list(d: &NotesPanelData) -> String {
    if d.notes.is_empty() {
        return r#"<div class="p-space-sm rounded-lg bg-surface-container text-body-sm text-outline">Aún no hay notas. <a href="/app/notes" class="text-primary-container hover:underline">Ir a Mis notas</a> para crear la primera.</div>"#
            .to_string();
    }
    let mut rows = String::new();
    for (i, n) in d.notes.iter().enumerate() {
        let (row_classes, icon_classes) = if i == 0 {
            (
                "bg-primary-container/15 text-primary-fixed font-medium",
                "text-primary",
            )
        } else {
            (
                "text-outline hover:text-on-surface hover:bg-surface-container transition-colors",
                "text-outline",
            )
        };
        let hidden = if i < LIST_LIMIT { "" } else { " hidden" };
        rows.push_str(&format!(
            r#"<a class="flex items-start gap-1.5 px-space-xs py-1 rounded {row_classes}" href="/app/notes/{slug}" title="{title}" data-stitch-row="list" data-stitch-tags="{tags}"{hidden}>
<span class="material-symbols-outlined text-[14px] {icon_classes} shrink-0 mt-0.5">description</span>
<span class="min-w-0 flex-1">
<span class="block truncate">{title}</span>
{snippet}
</span>
</a>"#,
            row_classes = row_classes,
            icon_classes = icon_classes,
            slug = esc(&n.slug),
            title = esc(&n.title),
            tags = esc(&note_tag_slugs(&n.tags).join(" ")),
            snippet = snippet_line(&n.snippet, "span", "block truncate font-body-sm text-[11px] text-outline"),
            hidden = hidden,
        ));
    }
    if d.notes.len() > LIST_LIMIT {
        rows.push_str(&format!(
            r#"<div class="px-space-xs py-1 font-body-sm text-[11px] text-outline" data-stitch-more>… y {} más en la bóveda</div>"#,
            d.notes.len() - LIST_LIMIT
        ));
    }
    // Estado vacío del filtro: lo rellena el script si el filtro no deja filas.
    rows.push_str(
        r#"<div id="stitch-tag-empty" class="hidden p-space-sm rounded-lg bg-surface-container text-body-sm text-outline"></div>"#,
    );
    rows
}

/// Bloque "Recientes": solo cuando el explorador ya no es la lista completa.
fn render_recent_block(d: &NotesPanelData) -> String {
    if d.notes.len() < RECENT_MIN {
        return String::new();
    }
    let rows = d
        .notes
        .iter()
        .take(5)
        .map(|n| {
            format!(
                r#"<a class="flex items-start gap-1.5 px-space-xs py-1 rounded text-outline hover:text-on-surface hover:bg-surface-container transition-colors" href="/app/notes/{slug}" data-stitch-row="recent" data-stitch-tags="{tags}">
<span class="material-symbols-outlined text-[14px] text-tertiary-fixed-dim shrink-0 mt-0.5">schedule</span>
<span class="min-w-0 flex-1">
<span class="block truncate">{title}</span>
{snippet}
</span>
</a>"#,
                slug = esc(&n.slug),
                title = esc(&n.title),
                tags = esc(&note_tag_slugs(&n.tags).join(" ")),
                snippet = snippet_line(&n.snippet, "span", "block truncate font-body-sm text-[11px] text-outline"),
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"<div class="mb-space-sm" id="stitch-recent">
<span class="font-label-sm text-[10px] uppercase tracking-wider text-outline font-semibold">Recientes</span>
<div class="mt-1 space-y-0.5">{rows}</div>
</div>"#,
        rows = rows
    )
}

/// Etiquetas reales de la bóveda (agregadas desde la columna `tags`).
///
/// Cada etiqueta es un chip clicable que filtra el explorador por esa
/// etiqueta; el conteo es el real de las notas cargadas en el panel.
fn render_tags(d: &NotesPanelData) -> String {
    if d.tags.is_empty() {
        return String::new();
    }
    let mut chips = format!(
        r#"<button type="button" data-stitch-chip="filter" data-stitch-tag="" data-stitch-label="Todas" aria-pressed="true" class="{cls}">Todas <span class="opacity-70">({total})</span></button>"#,
        cls = CHIP_CLASS_ON,
        total = d.notes.len()
    );
    for t in d.tags.iter().take(TAGS_MAX) {
        if t.slug.is_empty() {
            continue;
        }
        chips.push_str(&format!(
            r#"<button type="button" data-stitch-chip="filter" data-stitch-tag="{slug}" data-stitch-label="{label}" aria-pressed="false" class="{cls}">#{label} <span class="opacity-70">({count})</span></button>"#,
            slug = esc(&t.slug),
            label = esc(&t.tag),
            count = t.count,
            cls = CHIP_CLASS_OFF,
        ));
    }
    if d.tags.len() > TAGS_MAX {
        chips.push_str(&format!(
            r#"<span class="px-2 py-0.5 rounded bg-surface-container text-outline font-label-sm text-[11px]">+{}</span>"#,
            d.tags.len() - TAGS_MAX
        ));
    }
    // Aviso honesto si la bóveda tiene más notas de las que carga el panel:
    // los conteos son de las cargadas, no de toda la bóveda.
    let scope = if d.notes.len() >= SCAN_LIMIT {
        format!(
            r#"<p class="mt-1 font-body-sm text-[11px] text-outline">Conteos sobre las {limit} notas más recientes de la bóveda.</p>"#,
            limit = SCAN_LIMIT
        )
    } else {
        String::new()
    };
    format!(
        r#"<div class="mb-space-sm" id="stitch-tag-filter">
<span class="font-label-sm text-[10px] uppercase tracking-wider text-outline font-semibold">Etiquetas</span>
<div class="flex flex-wrap gap-1 mt-1" role="group" aria-label="Filtrar notas por etiqueta">{chips}</div>
<p id="stitch-tag-status" class="mt-1 font-body-sm text-[11px] text-outline hidden"></p>
{scope}
{script}
</div>"#,
        chips = chips,
        scope = scope,
        script = render_tag_filter_script(),
    )
}

/// Envuelve [`TAG_FILTER_SCRIPT`] en su `<script>` e inyecta la configuración
/// (clases de los chips y límite de filas visibles) como literal JSON.
fn render_tag_filter_script() -> String {
    let cfg = json!({
        "off": CHIP_CLASS_OFF,
        "on": CHIP_CLASS_ON,
        "limit": LIST_LIMIT,
    });
    format!(
        "<script>\n{js}\n</script>",
        js = TAG_FILTER_SCRIPT.replace("/*__CFG__*/", &cfg.to_string())
    )
}

/// Barra de estado del editor: fecha real de la última actualización y
/// acciones de la nota destacada (editar, eliminar y —cuando exista la ruta—
/// descargar el `.md`).
fn render_status(d: &NotesPanelData) -> String {
    render_featured(
        |f| {
            format!(
                r#"<div class="flex flex-col items-end gap-0.5 shrink-0">
<div class="flex items-center gap-2">
<span class="flex items-center gap-1 font-body-sm text-body-sm text-secondary">
<span class="w-1.5 h-1.5 rounded-full bg-secondary animate-pulse"></span>
Actualizada {fecha}
</span>
{acciones}
</div>
{snippet}
</div>"#,
                fecha = esc(&fmt_fecha(&f.updated_at)),
                acciones = render_featured_actions(f),
                snippet = snippet_line(
                    &f.snippet,
                    "p",
                    "font-body-sm text-[11px] text-outline truncate max-w-[260px] text-right"
                )
            )
        },
        d,
    )
}

/// Acciones de la nota destacada: editar, eliminar y descargar el `.md`.
///
/// El `<details>` de confirmación replica el de `handlers::notes` (mismo
/// diálogo, mismos botones y mismo destino `DELETE /app/notes/{slug}/delete`);
/// sólo cambia el envío, porque el shell Stitch v2 no carga htmx: el `fetch`
/// con `DELETE` hace de `hx-delete` y redirige a `/app/notes` como hace el
/// `HX-Redirect` del handler.
fn render_featured_actions(f: &FeaturedNote) -> String {
    // Sin slug no hay ninguna acción posible: los tres enlaces quedarían
    // apuntando a `/app/notes//edit`, `/app/notes//export` y
    // `/app/notes//delete`, que no existen (404). Con el slug vacío la nota
    // no tiene acciones, en vez de tenerlas rotas.
    if f.slug.trim().is_empty() {
        return String::new();
    }
    let slug = esc(&f.slug);
    let title = esc(&f.title);
    let export = render_export_action(&f.slug);
    format!(
        r#"<div class="flex items-center gap-1">
<a class="p-1 rounded text-outline hover:text-on-surface hover:bg-surface-container transition-colors" href="/app/notes/{slug}/edit" title="Editar esta nota">
<span class="material-symbols-outlined text-[18px]">edit</span>
</a>
{export}
<details class="relative" data-stitch-note-actions>
<summary class="p-1 rounded text-error hover:bg-error-container/20 transition-colors cursor-pointer select-none" title="Eliminar esta nota">
<span class="material-symbols-outlined text-[18px]">delete</span>
</summary>
<div class="absolute right-0 top-full mt-1 z-20 w-72 p-space-md rounded-xl border border-outline-variant bg-surface-container-high shadow-lg">
<p class="font-body-sm text-body-sm text-on-surface mb-space-sm">¿Eliminar «{title}»? La acción no se puede deshacer.</p>
<p class="hidden font-body-sm text-[11px] text-error mb-space-sm" data-stitch-delete-msg></p>
<div class="flex items-center justify-end gap-2">
<button type="button" onclick="this.closest('details').removeAttribute('open')" class="px-space-sm py-1.5 rounded-lg text-on-surface-variant hover:text-on-surface hover:bg-surface-container transition-colors font-label-md">Cancelar</button>
<button type="button" data-stitch-delete-btn data-delete-url="/app/notes/{slug}/delete" class="flex items-center gap-1 px-space-sm py-1.5 rounded-lg bg-error-container text-on-error-container font-label-md font-semibold hover:opacity-90 transition-opacity">
<span class="material-symbols-outlined text-[16px]">delete</span>Sí, eliminar
</button>
</div>
</div>
</details>
</div>
<style>details[data-stitch-note-actions]>summary{{list-style:none}}details[data-stitch-note-actions]>summary::-webkit-details-marker{{display:none}}</style>
{script}"#,
        slug = slug,
        title = title,
        export = export,
        script = render_delete_script(),
    )
}

/// Envuelve [`DELETE_NOTE_SCRIPT`] en su `<script>`.
///
/// Sin el envoltorio el JavaScript se renderizaría como **texto visible**
/// dentro de la barra de herramientas y el botón «Sí, eliminar» se quedaría
/// sin listener: el JS se ejecutaría como texto, no como código.
fn render_delete_script() -> String {
    format!("<script>\n{js}\n</script>", js = DELETE_NOTE_SCRIPT)
}

/// Enlace de descarga del `.md` de la nota.
///
/// Vacío mientras la ruta de exportación no exista en el router (ver
/// [`EXPORT_ROUTE_TEMPLATE`]): no se enlaza a rutas inexistentes.
fn render_export_action(slug: &str) -> String {
    let Some(template) = EXPORT_ROUTE_TEMPLATE else {
        return String::new();
    };
    // Slug vacío = URL `/app/notes//export` inexistente: no se enlaza.
    if slug.trim().is_empty() {
        return String::new();
    }
    format!(
        r#"<a class="p-1 rounded text-outline hover:text-on-surface hover:bg-surface-container transition-colors" href="{href}" download title="Descargar el .md de esta nota">
<span class="material-symbols-outlined text-[18px]">download</span>
</a>"#,
        href = esc(&template.replace("{slug}", slug)),
    )
}

/// Chips de etiquetas reales de la nota destacada: al pulsarlos se filtra el
/// explorador por esa etiqueta (mismo criterio que los enlaces de etiqueta
/// de la página `/app/notes`).
fn render_note_tags(d: &NotesPanelData) -> String {
    render_featured(
        |f| {
            let tags: Vec<&str> = f
                .tags
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            if tags.is_empty() {
                return r#"<span class="px-2 py-0.5 rounded bg-surface-container text-outline font-label-sm text-label-sm">sin etiquetas</span>"#.to_string();
            }
            tags.iter()
                .map(|t| {
                    let slug = tag_slug(t);
                    if slug.is_empty() {
                        return format!(
                            r#"<span class="px-2 py-0.5 rounded bg-surface-container text-secondary font-label-sm text-label-sm">#{}</span>"#,
                            esc(t)
                        );
                    }
                    format!(
                        r#"<button type="button" data-stitch-tag="{slug}" data-stitch-label="{label}" aria-pressed="false" class="px-2 py-0.5 rounded bg-surface-container text-secondary font-label-sm text-label-sm cursor-pointer transition-colors">#{label}</button>"#,
                        slug = esc(&slug),
                        label = esc(t),
                    )
                })
                .collect::<Vec<_>>()
                .join("")
        },
        d,
    )
}

/// Contador real de palabras y de enlaces `[[...]]` de la nota destacada.
fn render_note_stats(d: &NotesPanelData) -> String {
    render_featured(
        |f| {
            format!(
                r#"<span>{words} palabras</span>
<span>·</span>
<span>{links} enlaces</span>"#,
                words = f.word_count,
                links = f.outlinks.len()
            )
        },
        d,
    )
}

/// Panel izquierdo del editor: el Markdown real (fuente) leído del disco.
fn render_raw(d: &NotesPanelData) -> String {
    let Some(f) = &d.featured else {
        return r#"<p class="text-outline">Sin nota seleccionada todavía.</p>"#.to_string();
    };
    if f.content.trim().is_empty() {
        return format!(
            r#"<p class="text-outline">El archivo .md está vacío o no está disponible. <a class="text-primary-container hover:underline" href="/app/notes/{slug}/edit">Escribir contenido</a>.</p>"#,
            slug = esc(&f.slug)
        );
    }
    let mut out: Vec<String> = Vec::new();
    let mut code_lines: Vec<String> = Vec::new();
    let mut en_codigo = false;
    for line in f.content.lines() {
        if line.trim_start().starts_with("```") {
            if en_codigo {
                en_codigo = false;
                out.push(code_card_raw(&std::mem::take(&mut code_lines)));
            } else {
                en_codigo = true;
            }
            continue;
        }
        if en_codigo {
            code_lines.push(esc(line));
        } else if let Some(h) = line.strip_prefix("# ") {
            out.push(format!(
                r#"<p class="text-primary font-bold text-headline-sm">{}</p>"#,
                esc(h)
            ));
        } else if let Some(h) = line.strip_prefix("## ") {
            out.push(format!(
                r#"<p class="text-primary font-semibold text-body-lg">{}</p>"#,
                esc(h)
            ));
        } else if line.trim().is_empty() {
            continue;
        } else {
            out.push(format!(
                r#"<p class="text-on-surface">{}</p>"#,
                esc_text(line)
            ));
        }
    }
    if en_codigo && !code_lines.is_empty() {
        out.push(code_card_raw(&code_lines));
    }
    out.join("\n")
}

/// Vista previa renderizada del Markdown real: títulos, párrafos, citas,
/// listas, bloques de código y wikilinks resueltos a la nota real (o chips
/// "sin crear" cuando el destino no existe). Es la vista del conversor
/// básico de la app, no un renderer completo.
fn render_preview(d: &NotesPanelData) -> String {
    let Some(f) = &d.featured else {
        return r#"<p class="text-outline">Sin nota seleccionada todavía.</p>"#.to_string();
    };
    if f.content.trim().is_empty() {
        return format!(
            r#"<p class="text-outline">La nota está vacía. <a class="text-primary-container hover:underline" href="/app/notes/{slug}/edit">Escribir contenido</a>.</p>"#,
            slug = esc(&f.slug)
        );
    }
    let mut out: Vec<String> = Vec::new();
    let mut code_lines: Vec<String> = Vec::new();
    let mut en_codigo = false;
    for line in f.content.lines() {
        if line.trim_start().starts_with("```") {
            if en_codigo {
                en_codigo = false;
                let buf = std::mem::take(&mut code_lines);
                out.push(code_card_preview(&buf, &d.titles));
            } else {
                en_codigo = true;
            }
            continue;
        }
        if en_codigo {
            code_lines.push(line.to_string());
        } else {
            out.push(preview_line(line, &d.titles));
        }
    }
    if en_codigo && !code_lines.is_empty() {
        out.push(code_card_preview(&code_lines, &d.titles));
    }
    out.join("\n")
}

fn preview_line(line: &str, titles: &HashMap<String, String>) -> String {
    let t = line.trim();
    if let Some(h) = t.strip_prefix("### ") {
        format!(
            r#"<h3 class="font-label-lg text-label-lg text-on-surface font-semibold mt-2">{}</h3>"#,
            render_inline(h, titles)
        )
    } else if let Some(h) = t.strip_prefix("## ") {
        format!(
            r#"<h2 class="font-headline-md text-headline-md font-bold text-on-surface tracking-tight">{}</h2>"#,
            render_inline(h, titles)
        )
    } else if let Some(h) = t.strip_prefix("# ") {
        format!(
            r#"<h1 class="font-headline-lg text-headline-lg font-bold text-on-surface tracking-tight">{}</h1>"#,
            render_inline(h, titles)
        )
    } else if let Some(q) = t.strip_prefix("> ") {
        format!(
            r#"<blockquote class="border-l-2 border-primary-container/40 pl-3 text-on-surface-variant italic">{}</blockquote>"#,
            render_inline(q, titles)
        )
    } else if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        format!(
            r#"<p class="text-on-surface">• {}</p>"#,
            render_inline(item, titles)
        )
    } else if t.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="text-on-surface">{}</p>"#,
            render_inline(t, titles)
        )
    }
}

/// Texto en línea con backticks -> `<code>` y wikilinks -> enlaces reales.
fn render_inline(raw: &str, titles: &HashMap<String, String>) -> String {
    let links = crate::knowledge::wikilinks::parse_wikilinks(raw);
    let mut out = String::new();
    let mut pos = 0usize;
    for l in &links {
        if l.offset < pos {
            continue;
        }
        let Some(rel_close) = raw[l.offset..].find("]]") else {
            continue;
        };
        let close = l.offset + rel_close + 2;
        out.push_str(&esc_text(&raw[pos..l.offset]));
        let inner = &raw[l.offset + 2..close - 2];
        let slug = l.resolve();
        if titles.contains_key(&slug) {
            out.push_str(&format!(
                r#"<a class="inline-flex items-center gap-0.5 px-1 py-0.2 rounded bg-primary-container/15 text-primary hover:bg-primary-container/30 font-semibold transition-colors" href="/app/notes/{0}">[[{1}]]</a>"#,
                esc(&slug),
                esc_text(inner),
            ));
        } else {
            out.push_str(&format!(
                r#"<span class="inline-flex items-center gap-0.5 px-1 py-0.2 rounded bg-surface-container text-outline font-semibold" title="Aún no existe esa nota">[[{0}]]</span>"#,
                esc_text(inner),
            ));
        }
        pos = close;
    }
    out.push_str(&esc_text(&raw[pos.min(raw.len())..]));
    out
}

/// Bloque de código para la fuente (texto plano escapado).
fn code_card_raw(lines: &[String]) -> String {
    format!(
        r#"<div class="p-space-sm bg-surface-container-low rounded font-code-inline text-code-inline text-outline">{}</div>"#,
        lines.join("<br/>")
    )
}

/// Bloque de código para la vista previa (estilo tarjeta del kit).
fn code_card_preview(lines: &[String], titles: &HashMap<String, String>) -> String {
    let body = lines
        .iter()
        .map(|l| format!("<div>{}</div>", render_inline(l, titles)))
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"<div class="rounded-lg bg-surface-container overflow-hidden">
<div class="p-space-sm font-code-inline text-code-inline text-on-surface space-y-0.5">{body}</div>
</div>"#,
        body = body
    )
}

fn render_backlinks_count(d: &NotesPanelData) -> String {
    render_featured(
        |f| match f.backlinks.len() {
            0 => "Aún sin enlaces".to_string(),
            1 => "1 nota enlaza aquí".to_string(),
            n => format!("{n} notas enlazan aquí"),
        },
        d,
    )
}

/// Tarjetas de backlinks reales (la nota fuente que menciona a la activa).
fn render_backlinks(d: &NotesPanelData) -> String {
    let Some(f) = &d.featured else {
        return String::new();
    };
    if f.backlinks.is_empty() {
        return r#"<div class="p-space-sm rounded-lg bg-surface-container text-body-sm text-outline md:col-span-2">Ninguna nota menciona a esta todavía. Usa [[Título de esta nota]] en otra nota para crear el primer backlink.</div>"#.to_string();
    }
    f.backlinks
        .iter()
        .take(6)
        .map(|b| {
            format!(
                r#"<div class="p-space-sm rounded-lg bg-surface-container hover:bg-surface-container-high transition-colors group">
<div class="flex items-center justify-between mb-1">
<div class="flex items-center gap-1.5 min-w-0">
<span class="material-symbols-outlined text-primary-fixed-dim text-[16px]">file_open</span>
<a class="font-label-md text-label-md text-on-surface font-semibold group-hover:text-primary transition-colors truncate" href="/app/notes/{slug}">[[{title}]]</a>
</div>
<span class="font-body-sm text-[11px] text-outline shrink-0">{fecha}</span>
</div>
<p class="font-body-sm text-body-sm text-on-surface-variant line-clamp-2">{snippet}</p>
</div>"#,
                slug = esc(&b.from_slug),
                title = esc(&b.from_title),
                fecha = esc(&fmt_fecha(&b.updated_at)),
                snippet = b.snippet
            )
        })
        .collect()
}

fn render_graph_meta(d: &NotesPanelData) -> String {
    format!(
        r#"<div class="flex items-center justify-between text-body-sm text-outline font-label-sm">
<span>Notas: <strong>{total}</strong></span>
<div class="flex items-center gap-1">
<span class="w-2 h-2 rounded-full {dot}"></span>
<span class="text-on-surface-variant">{links} enlaces internos</span>
</div>
</div>"#,
        total = d.total,
        links = d.total_links,
        dot = if d.total_links > 0 {
            "bg-secondary"
        } else {
            "bg-outline"
        }
    )
}

/// Grafo honesto: nodo central (nota activa) + notas realmente enlazadas
/// (destinos de sus `[[...]]` y notas que la mencionan). Si no hay enlaces,
/// estado vacío con el mismo contenedor.
fn render_graph(d: &NotesPanelData) -> String {
    let Some(f) = &d.featured else {
        return graph_empty(
            "Sin notas todavía: el grafo se dibujará cuando haya contenido enlazado.",
        );
    };
    let mut nodes: Vec<(String, bool)> = Vec::new();
    let mut keys: HashSet<String> = HashSet::new();
    for l in &f.outlinks {
        let (label, key, exists) = if l.exists {
            (l.title.clone(), format!("n:{}", l.slug), true)
        } else {
            (
                format!("{} (sin crear)", l.display),
                format!("x:{}", l.slug),
                false,
            )
        };
        if keys.insert(key) {
            nodes.push((label, exists));
        }
    }
    for b in &f.backlinks {
        let key = format!("n:{}", b.from_slug);
        if keys.insert(key) {
            nodes.push((b.from_title.clone(), true));
        }
    }
    if nodes.is_empty() {
        return graph_empty(
            "Esta nota no tiene enlaces [[...]] todavía: el grafo se dibujará cuando conectes notas.",
        );
    }
    let mostrados = nodes.len().min(GRAPH_NODES_MAX);
    let mut edges = String::new();
    let mut dots = String::new();
    for (i, (label, _)) in nodes.iter().take(mostrados).enumerate() {
        let ang = -std::f64::consts::FRAC_PI_2
            + (i as f64) * (2.0 * std::f64::consts::PI) / (mostrados as f64);
        let x = (160.0 + 100.0 * ang.cos()).round() as i64;
        let y = (140.0 + 92.0 * ang.sin()).round() as i64;
        edges.push_str(&format!(
            r#"<line class="text-primary-container/40" stroke="currentColor" stroke-width="1.5" x1="160" x2="{x}" y1="140" y2="{y}"></line>"#
        ));
        dots.push_str(&format!(
            r#"<g class="cursor-pointer transition-transform hover:scale-110">
<circle class="fill-surface-container-high stroke-secondary" cx="{x}" cy="{y}" r="10" stroke-width="2"></circle>
<text class="font-label-sm text-[10px] fill-on-surface" text-anchor="middle" x="{x}" y="{yl}">{label}</text>
</g>"#,
            yl = y + 22,
            label = esc(&trunc(label, 14))
        ));
    }
    let extra = nodes.len() - mostrados;
    let overlay = if extra > 0 {
        format!(" conexiones (+{extra} más)")
    } else {
        " conexiones".to_string()
    };
    format!(
        r#"<svg class="w-full h-full" viewbox="0 0 320 280">
<defs>
<filter height="140%" id="glow-cyan" width="140%" x="-20%" y="-20%">
<fegaussianblur result="blur" stddeviation="3"></fegaussianblur>
<fecomposite in="SourceGraphic" in2="blur" operator="over"></fecomposite>
</filter>
</defs>
{edges}
<g class="cursor-pointer transition-transform hover:scale-105">
<circle class="fill-primary-container/20 stroke-primary-container" cx="160" cy="140" r="18" stroke-width="2.5"></circle>
<circle class="fill-primary-container" cx="160" cy="140" filter="url(#glow-cyan)" r="10"></circle>
<text class="font-label-md text-[11px] font-bold fill-primary-fixed" text-anchor="middle" x="160" y="176">{centro}</text>
</g>
{dots}
</svg>
<div class="absolute bottom-2 left-2 px-2 py-0.5 rounded bg-surface-container/80 backdrop-blur-sm text-[10px] font-code-inline text-outline">{mostrados}{overlay}</div>"#,
        edges = edges,
        centro = esc(&trunc(&f.title, 18)),
        dots = dots,
        mostrados = mostrados,
        overlay = overlay
    )
}

/// Contenido interno del contenedor fijo del grafo (el `<div>` de h-72 del
/// kit queda en el HTML): estado vacío honesto.
fn graph_empty(msg: &str) -> String {
    format!(
        r#"<div class="flex flex-col items-center gap-2 p-space-md text-center">
<span class="material-symbols-outlined text-[32px] text-outline">hub</span>
<p class="font-body-sm text-body-sm text-outline max-w-[230px]">{msg}</p>
</div>"#,
        msg = esc(msg)
    )
}

fn render_outlinks_count(d: &NotesPanelData) -> String {
    render_featured(
        |f| match f.outlinks.len() {
            0 => "0 refs".to_string(),
            n => format!("{n} refs"),
        },
        d,
    )
}

/// Enlaces salientes reales de la nota activa. Los destinos que aún no
/// existen se marcan "sin crear" en vez de inventar categorías.
fn render_outlinks(d: &NotesPanelData) -> String {
    let Some(f) = &d.featured else {
        return String::new();
    };
    if f.outlinks.is_empty() {
        return r#"<div class="p-2 rounded bg-surface-container-lowest text-body-sm text-outline">La nota no tiene enlaces [[...]] todavía. Escribe [[Título de otra nota]] para conectarla.</div>"#.to_string();
    }
    f.outlinks
        .iter()
        .take(8)
        .map(|l| {
            let (dot, label) = if l.exists {
                ("bg-primary-container", "nota")
            } else {
                ("bg-error", "sin crear")
            };
            let text = if l.display.is_empty() {
                format!("[[{}]]", esc(&l.slug))
            } else {
                format!("[[{}]]", esc(&l.display))
            };
            if l.exists {
                format!(
                    r#"<a class="flex items-center justify-between p-1.5 rounded hover:bg-surface-container-high text-on-surface transition-colors" href="/app/notes/{slug}">
<div class="flex items-center gap-1.5 truncate">
<span class="w-1.5 h-1.5 rounded-full {dot}"></span>
<span class="truncate">{text}</span>
</div>
<span class="text-outline text-[11px]">{label}</span>
</a>"#,
                    slug = esc(&l.slug),
                    dot = dot,
                    text = text,
                    label = label
                )
            } else {
                format!(
                    r#"<div class="flex items-center justify-between p-1.5 rounded bg-surface-container-lowest/60" title="La nota destino no existe todavía">
<div class="flex items-center gap-1.5 truncate">
<span class="w-1.5 h-1.5 rounded-full {dot}"></span>
<span class="truncate">{text}</span>
</div>
<span class="text-outline text-[11px]">{label}</span>
</div>"#,
                    dot = dot,
                    text = text,
                    label = label
                )
            }
        })
        .collect()
}

/// Reemplazo del "Hash / cohesión" ficticio: cifras reales de la bóveda.
fn render_graph_stats(d: &NotesPanelData) -> String {
    format!(
        r#"<span>Bóveda: <span class="font-code-inline text-on-surface-variant">{total} notas</span></span>
<span>Palabras: <strong class="text-secondary font-semibold">{words}</strong></span>"#,
        total = d.total,
        words = d.total_words
    )
}

// ── helpers ────────────────────────────────────────────────────────────────

/// Línea de resumen real de una nota: una sola línea con la primera frase
/// del cuerpo (ver `knowledge::frontmatter::summarize`).
///
/// No inventa nada: si la nota no tiene archivo legible en disco o su cuerpo
/// está vacío, devuelve una cadena vacía y la vista conserva solo el título.
fn snippet_line(snippet: &str, tag: &str, class: &str) -> String {
    let s = snippet.trim();
    if s.is_empty() {
        return String::new();
    }
    format!(
        r#"<{tag} class="{class}">{snippet}</{tag}>"#,
        tag = tag,
        class = class,
        snippet = esc(s)
    )
}

/// Ruta real del archivo `.md` de una nota.
///
/// Usa `notes.filename` tal cual (la BD lo guarda con su extensión, p. ej.
/// "slug.md"); sólo si la columna viniera vacía se reconstruye desde el slug.
fn note_path(notes_dir: &Path, filename: &str, slug: &str) -> std::path::PathBuf {
    if filename.trim().is_empty() {
        notes_dir.join(format!("{slug}.md"))
    } else {
        notes_dir.join(filename)
    }
}

/// Normaliza una etiqueta a un slug comparable y seguro para la URL
/// (minúsculas, acentos plegados, solo `[a-z0-9_-]`).
///
/// Mismo criterio que `handlers::notes::tag_slug`, que es privado y no se
/// puede reutilizar desde aquí.
fn tag_slug(raw: &str) -> String {
    raw.trim()
        .to_lowercase()
        .split_whitespace()
        .map(|word| {
            word.chars()
                .map(|c| match c {
                    'á' => 'a',
                    'é' => 'e',
                    'í' => 'i',
                    'ó' => 'o',
                    'ú' => 'u',
                    'ü' => 'u',
                    'ñ' => 'n',
                    'ç' => 'c',
                    _ => c,
                })
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Slugs de las etiquetas de una nota (columna `tags`, separada por comas).
fn note_tag_slugs(tags: &str) -> Vec<String> {
    tags.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(tag_slug)
        .filter(|s| !s.is_empty())
        .collect()
}

fn esc(s: &str) -> String {
    crate::academic::templates::html_escape(s)
}

/// Escapa texto visible y además convierte pares de backticks en `<code>`
/// inline (impar de backticks: se escapa sin transformar).
fn esc_text(s: &str) -> String {
    if !s.contains('`') || s.matches('`').count() % 2 != 0 {
        return esc(s);
    }
    let mut out = String::new();
    let mut en_codigo = false;
    for parte in s.split('`') {
        if en_codigo {
            out.push_str(&format!(
                r#"<code class="px-1.5 py-0.5 rounded bg-surface-container font-code-inline text-code-inline text-secondary">{}</code>"#,
                esc(parte)
            ));
        } else if !parte.is_empty() {
            out.push_str(&esc(parte));
        }
        en_codigo = !en_codigo;
    }
    out
}

/// Línea del archivo fuente que contiene la mención en `offset`, con la
/// mención resaltada y el texto escapado.
fn mention_snippet(content: &str, offset: usize) -> String {
    let offset = offset.min(content.len());
    let start = content[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = content[offset..]
        .find('\n')
        .map_or(content.len(), |i| offset + i);
    let line = &content[start..end];
    let Some(rel) = line.find("[[") else {
        return esc(line);
    };
    let Some(rel_close) = line[rel..].find("]]") else {
        return esc(line);
    };
    let mention_end = rel + rel_close + 2;
    let (prefix, suffix) = (line[..rel].trim(), line[mention_end..].trim());
    format!(
        "{p1}{prefix} <span class=\"text-primary font-medium\">{mention}</span>{suffix}{p2}",
        p1 = if prefix.is_empty() { "" } else { "… " },
        prefix = esc(prefix),
        mention = esc(&line[rel..mention_end]),
        suffix = esc(suffix),
        p2 = if suffix.is_empty() { "" } else { " …" }
    )
}

const MESES: [&str; 12] = [
    "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
];

/// "2026-10-16T21:45:00+00:00" (RFC3339) o "2026-10-16 21:45:00" -> "16 oct 2026".
fn fmt_fecha(s: &str) -> String {
    let Some(dia) = s.get(0..10) else {
        return s.to_string();
    };
    let mut it = dia.split('-');
    match (
        it.next(),
        it.next().and_then(|m| m.parse::<usize>().ok()),
        it.next(),
    ) {
        (Some(y), Some(m), Some(d)) if (1..=12).contains(&m) => {
            format!("{} {} {}", d, MESES[m - 1], y)
        }
        _ => s.get(0..10).unwrap_or(s).to_string(),
    }
}

/// Trunca por caracteres (no bytes) para etiquetas del grafo.
fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}
