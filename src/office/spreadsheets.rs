//! Spreadsheet editor — an HTML table with `contenteditable` cells.
//!
//! Sheets are stored as `data/office/sheets/{slug}.csv`. The grid is serialized
//! client-side into a hidden `csv_data` field and POSTed to the import endpoint
//! (which also accepts pasted CSV). Export parses the CSV, evaluates `SUM`/`AVG`
//! formulas server-side, and returns the computed values as CSV.

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Form,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::office::sheets_dir;
use crate::office::templates;
use crate::office::templates::{hx_redirect, layout, layout_as, slugify};
use crate::AppState;

/// Input bounds (400 on violation).
const MAX_TITLE_LEN: usize = 200;
const MAX_CSV_LEN: usize = 1_000_000;

/// Generic public message for 500-class failures (never leaks internals).
const INTERNAL_ERROR_HTML: &str =
    "<p class=\"text-red-400\">No se pudo completar la operación.</p>";

/// Create form.
#[derive(Deserialize)]
pub struct SheetForm {
    pub title: String,
}

/// Import / save-grid form. Accepts pasted CSV via `csv_data`.
#[derive(Deserialize)]
pub struct ImportForm {
    pub csv_data: String,
}

/// List all spreadsheets.
pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let files = templates::list_files(&sheets_dir(&state), "csv");
    let cards = if files.is_empty() {
        r##"<div class="gen-office-empty"><span class="material-symbols-outlined">table_chart</span><h3>Empieza con una cuadrícula.</h3><p>Crea una hoja de cinco por cinco o importa tus propios datos CSV.</p><a href="/office/sheets/new" class="gen-office-primary-button">Crear hoja</a></div>"##.to_string()
    } else {
        files.iter().map(|f| {
            format!(
                r##"<article class="gen-office-file-card sheets"><a class="gen-office-file-preview" href="/office/sheets/{slug}"><span class="material-symbols-outlined">table_chart</span><small>CSV</small></a><div class="gen-office-file-copy"><a href="/office/sheets/{slug}">{display}</a><span>{modified} · {size}</span></div><footer><a href="/office/sheets/{slug}/export">Exportar CSV</a><form hx-post="/office/sheets/{slug}/delete"><button type="submit" onclick="return confirm('¿Borrar?')" class="gen-office-icon-button danger" aria-label="Borrar {display}"><span class="material-symbols-outlined">delete</span></button></form></footer></article>"##,
                slug = templates::html_escape(&f.name),
                display = templates::html_escape(&f.display),
                size = templates::human_size(f.size),
                modified = templates::html_escape(&f.modified),
            )
        }).collect::<Vec<_>>().join("\n")
    };

    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-manager-page">{tabs}<header class="gen-office-manager-head"><div><span>HOJAS DE CÁLCULO · CSV</span><h1>Datos que puedes trabajar.</h1><p>{count} hoja(s) con cuadrícula editable y fórmulas SUM / AVG.</p></div><a href="/office/sheets/new" class="gen-office-primary-button"><span class="material-symbols-outlined">add</span>Nueva hoja</a></header><nav class="gen-office-appnav" aria-label="Aplicaciones de oficina"><a href="/office/docs"><span class="material-symbols-outlined">description</span>Documentos</a><a class="is-active sheets" href="/office/sheets"><span class="material-symbols-outlined">table_chart</span>Hojas</a><a href="/office/presentations"><span class="material-symbols-outlined">present_to_all</span>Presentaciones</a><a href="/office/library"><span class="material-symbols-outlined">folder_open</span>Biblioteca</a></nav><section class="gen-office-file-grid">{cards}</section><footer class="gen-office-home-footer"><span><span class="material-symbols-outlined">hard_drive</span> CSV local</span><span>Fórmulas calculadas al exportar</span></footer></div>"##,
        tabs = templates::office_tab_strip("sheets", "Hojas", "/office/sheets/new"),
        count = files.len(),
        cards = cards,
    );
    layout_as("Hojas de cálculo", "hojas", &body)
}

/// New-sheet form.
pub async fn new_sheet() -> impl IntoResponse {
    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-create-page">{tabs}<main class="gen-office-create-shell"><div class="gen-office-create-heading"><span class="gen-office-launcher-icon sheets"><span class="material-symbols-outlined">table_chart</span></span><div><span>NUEVA HOJA DE CÁLCULO</span><h1>Una cuadrícula lista para tus datos.</h1><p>Crearemos cinco filas y cinco columnas; después puedes ampliarlas desde el editor.</p></div></div><form hx-post="/office/sheets" hx-swap="outerHTML" class="gen-office-create-form compact"><div class="gen-office-create-fields"><label for="new-sheet-title">Nombre de la hoja</label><input id="new-sheet-title" type="text" name="title" placeholder="Ej. Presupuesto del trimestre" required autofocus></div><div class="gen-office-create-facts"><span><strong>5 × 5</strong> cuadrícula inicial</span><span><strong>SUM / AVG</strong> fórmulas</span><span><strong>CSV</strong> exportación</span></div><footer><a href="/office/sheets" class="gen-office-secondary-button">Cancelar</a><span>Se guardará como .csv</span><button type="submit" class="gen-office-primary-button">Crear hoja<span class="material-symbols-outlined">arrow_forward</span></button></footer></form></main></div>"##,
        tabs = templates::office_tab_strip("sheets", "Nueva hoja", "/office/sheets/new"),
    );
    layout_as("Nueva hoja", "hojas", &body)
}

/// Create a spreadsheet (empty grid).
pub async fn create(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SheetForm>,
) -> impl IntoResponse {
    let slug = slugify(&form.title);
    if slug == "untitled" && form.title.trim().is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout(
                "Error",
                "<p class=\"text-red-400\">El título es obligatorio.</p>",
            ),
        )
            .into_response();
    }
    if form.title.len() > MAX_TITLE_LEN {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout(
                "Error",
                "<p class=\"text-red-400\">El título es demasiado largo.</p>",
            ),
        )
            .into_response();
    }
    let file_path = sheets_dir(&state).join(format!("{}.csv", slug));
    if file_path.exists() {
        return (
            axum::http::StatusCode::CONFLICT,
            layout(
                "Conflicto",
                "<p class=\"text-red-400\">Ya existe una hoja con ese título.</p>",
            ),
        )
            .into_response();
    }
    // Seed an empty 5x5 grid so the editor has something to render.
    let seed = ",,,,\n,,,,\n,,,,\n,,,,\n,,,,".to_string();
    if let Err(e) = templates::write_text(&file_path, &seed) {
        tracing::error!(
            slug_len = slug.len(),
            "office sheets create: seed write failed: {}",
            e
        );
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            layout("Error", INTERNAL_ERROR_HTML),
        )
            .into_response();
    }
    hx_redirect(&format!("/office/sheets/{}", slug))
}

/// Edit a spreadsheet (contenteditable grid).
pub async fn edit(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout("Error", "<p class=\"text-red-400\">Nombre no válido.</p>"),
        )
            .into_response();
    }
    let file_path = sheets_dir(&state).join(format!("{}.csv", slug));
    if !file_path.exists() {
        return (
            axum::http::StatusCode::NOT_FOUND,
            layout(
                "No encontrado",
                "<p class=\"text-gray-500\">Hoja no encontrada.</p>",
            ),
        )
            .into_response();
    }
    let raw = match templates::read_text(&file_path) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::error!(
                slug_len = slug.len(),
                "office sheets edit: read failed: {}",
                e
            );
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                layout("Error", INTERNAL_ERROR_HTML),
            )
                .into_response();
        }
    };
    let rows = parse_csv(&raw);
    let computed = compute_grid(&rows);
    let grid_html = render_grid(&slug, &rows, &computed);
    let ai_content = format!(
        r##"<div class="gen-office-tool-list"><button type="button" onclick="addOfficeGridRow()"><span class="material-symbols-outlined">add_row_below</span><span><strong>Añadir una fila</strong><small>Amplía la cuadrícula antes de guardar</small></span></button><button type="button" onclick="addOfficeGridColumn()"><span class="material-symbols-outlined">add_column_right</span><span><strong>Añadir una columna</strong><small>Nueva columna editable al final</small></span></button></div><details class="gen-office-import-panel"><summary><span class="material-symbols-outlined">upload_file</span><span><strong>Importar CSV</strong><small>Reemplaza los datos actuales</small></span><span class="material-symbols-outlined">expand_more</span></summary><form hx-post="/office/sheets/{slug}/import"><label for="csv-import">Contenido CSV</label><textarea id="csv-import" name="csv_data" rows="8" placeholder="nombre,nota&#10;Examen,8.5"></textarea><button type="submit" class="gen-office-secondary-button">Importar datos</button></form></details><div class="gen-office-formula-help"><strong>Fórmulas compatibles</strong><code>=SUM(A1:A5)</code><code>=AVG(B2:B8)</code><span>Se calculan en la vista de valores y al exportar.</span></div>"##,
        slug = templates::html_escape(&slug),
    );
    let body = format!(
        r##"<div class="nova-page gen-office-page gen-office-editor-page">{tabs}<header class="gen-office-ribbon"><nav class="gen-office-menubar" aria-label="Acciones de la hoja"><a class="is-active" href="/office/sheets/{slug}">Inicio</a><button type="button" onclick="addOfficeGridRow()">Insertar fila</button><button type="button" onclick="addOfficeGridColumn()">Insertar columna</button><a href="/office/sheets/{slug}/export">Exportar CSV</a></nav><div class="gen-office-ribbon-actions">{ai_toggle}<a class="gen-office-primary-button" href="/office/sheets/{slug}/export"><span class="material-symbols-outlined">download</span>CSV</a></div></header><div class="gen-office-workspace">{ai}<main class="gen-office-editor-main"><div class="gen-office-canvas-head"><div><span>HOJA DE CÁLCULO</span><strong>{title}</strong></div><span class="gen-office-local-badge"><span class="material-symbols-outlined">hard_drive</span>CSV local</span></div><section class="gen-office-sheet-workspace">{grid}</section></main></div><footer class="gen-office-statusbar"><span><span class="material-symbols-outlined">table_chart</span> Hoja1</span><span>Editable · SUM / AVG</span><span>NOVA Office</span></footer></div><script>{workspace_js}</script>"##,
        tabs = templates::office_tab_strip("sheets", &slug.replace('-', " "), "/office/sheets/new"),
        slug = templates::html_escape(&slug),
        ai_toggle = templates::OFFICE_AI_TOGGLE,
        ai = templates::office_ai_panel("Herramientas de la hoja", "Edita celdas directamente o usa la barra fx. Las acciones de esta panel modifican la cuadrícula, no un archivo ficticio.", &ai_content),
        title = templates::html_escape(&slug.replace('-', " ")),
        grid = grid_html,
        workspace_js = templates::OFFICE_WORKSPACE_JS,
    );
    layout_as(&format!("Hoja: {}", slug), "hojas", &body).into_response()
}

/// Import CSV (or save the grid). Accepts `csv_data` (the grid is serialized
/// client-side into this hidden field before submit).
pub async fn import(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Form(form): Form<ImportForm>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout("Error", "<p class=\"text-red-400\">Nombre no válido.</p>"),
        )
            .into_response();
    }
    let file_path = sheets_dir(&state).join(format!("{}.csv", slug));
    if !file_path.exists() {
        return (
            axum::http::StatusCode::NOT_FOUND,
            layout(
                "No encontrado",
                "<p class=\"text-gray-500\">Hoja no encontrada.</p>",
            ),
        )
            .into_response();
    }
    // Conserva el CSV original para que el usuario pueda corregirlo y exportarlo
    // después; las fórmulas se calculan al abrir la vista de valores o exportar.
    if form.csv_data.len() > MAX_CSV_LEN {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout(
                "Error",
                "<p class=\"text-red-400\">El contenido es demasiado grande.</p>",
            ),
        )
            .into_response();
    }
    if let Err(e) = templates::write_text(&file_path, &form.csv_data) {
        tracing::error!(
            slug_len = slug.len(),
            "office sheets import: write failed: {}",
            e
        );
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            layout("Error", INTERNAL_ERROR_HTML),
        )
            .into_response();
    }
    hx_redirect(&format!("/office/sheets/{}", slug))
}

/// Export the spreadsheet as CSV with `SUM`/`AVG` formulas evaluated.
pub async fn export(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid name").into_response();
    }
    let file_path = sheets_dir(&state).join(format!("{}.csv", slug));
    if !file_path.exists() {
        return (axum::http::StatusCode::NOT_FOUND, "sheet not found").into_response();
    }
    let raw = match templates::read_text(&file_path) {
        Ok(raw) => raw,
        Err(e) => {
            tracing::error!(
                slug_len = slug.len(),
                "office sheets export: read failed: {}",
                e
            );
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo completar la operación",
            )
                .into_response();
        }
    };
    let rows = parse_csv(&raw);
    let computed = compute_grid(&rows);
    let bytes = match write_csv(&computed) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!(
                slug_len = slug.len(),
                "office sheets export: CSV encoding failed: {}",
                e
            );
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "No se pudo completar la operación",
            )
                .into_response();
        }
    };
    templates::file_download(
        &format!("{}.csv", slug),
        templates::mime_for_ext("csv"),
        bytes,
    )
}

/// Delete a spreadsheet.
pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    if !crate::office::is_safe_slug(&slug) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            layout("Error", "<p class=\"text-red-400\">Nombre no válido.</p>"),
        )
            .into_response();
    }
    let file_path = sheets_dir(&state).join(format!("{}.csv", slug));
    if let Err(e) = std::fs::remove_file(&file_path) {
        let status = if e.kind() == std::io::ErrorKind::NotFound {
            axum::http::StatusCode::NOT_FOUND
        } else {
            tracing::error!(
                slug_len = slug.len(),
                "office sheets delete: failed: {}",
                e
            );
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        };
        return (
            status,
            layout("Error", "<p class=\"text-red-400\">No se pudo borrar la hoja.</p>"),
        )
            .into_response();
    }
    hx_redirect("/office/sheets")
}

// ---------------------------------------------------------------------------
// Grid rendering
// ---------------------------------------------------------------------------

fn render_grid(slug: &str, rows: &[Vec<String>], computed: &[Vec<String>]) -> String {
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0).max(5);
    let display_rows = if rows.is_empty() { 5 } else { rows.len() };

    let mut editable = String::from(
        r##"<table id="grid" class="gen-office-sheet-grid" aria-label="Cuadrícula editable"><thead><tr><th class="gen-office-row-corner"></th>"##,
    );
    for c in 0..cols {
        editable.push_str(&format!("<th scope=\"col\">{}</th>", row_to_header(c)));
    }
    editable.push_str("</tr></thead><tbody>");
    for r in 0..display_rows {
        editable.push_str(&format!("<tr><th scope=\"row\">{}</th>", r + 1));
        for c in 0..cols {
            let val = rows
                .get(r)
                .and_then(|row| row.get(c))
                .map(|s| templates::html_escape(s))
                .unwrap_or_default();
            let cell_ref = format!("{}{}", row_to_header(c), r + 1);
            editable.push_str(&format!(
                "<td contenteditable=\"true\" data-r=\"{r}\" data-c=\"{c}\" data-cell=\"{cell_ref}\" aria-label=\"Celda {cell_ref}\">{val}</td>",
            ));
        }
        editable.push_str("</tr>");
    }
    editable.push_str("</tbody></table>");

    let mut values = String::from(r##"<table class="gen-office-sheet-values"><tbody>"##);
    for r in 0..display_rows {
        values.push_str("<tr>");
        for c in 0..cols {
            let val = computed
                .get(r)
                .and_then(|row| row.get(c))
                .map(|s| templates::html_escape(s))
                .unwrap_or_default();
            values.push_str(&format!("<td>{val}</td>"));
        }
        values.push_str("</tr>");
    }
    values.push_str("</tbody></table>");

    let header = (0..cols).map(row_to_header).collect::<Vec<_>>().join(" ");
    format!(
        r##"<div class="gen-office-sheet-head"><div><span>HOJA1 · CUADRÍCULA EDITABLE</span><p id="sheet-grid-count">{row_count} filas · {col_count} columnas</p></div><form hx-post="/office/sheets/{slug}/import" hx-on::before-request="serializeGrid()"><input type="hidden" name="csv_data" id="csv_data" value=""><button type="submit" id="save-grid-button" class="gen-office-primary-button"><span class="material-symbols-outlined">save</span>Guardar cuadrícula</button></form></div>
        <div class="gen-office-formula-bar"><span id="active-cell">A1</span><span class="gen-office-fx">fx</span><label class="sr-only" for="formula-input">Contenido de la celda seleccionada</label><input id="formula-input" type="text" autocomplete="off" placeholder="Selecciona una celda o escribe una fórmula"></div>
        <div class="gen-office-sheet-grid-wrap">{editable}</div>
        <details class="gen-office-sheet-computed"><summary><span><span class="material-symbols-outlined">calculate</span>Valores calculados</span><span class="material-symbols-outlined">expand_more</span></summary><div>{values}<p>Columnas: {header}</p></div></details><script>{grid_js}</script>"##,
        row_count = display_rows,
        col_count = cols,
        slug = slug,
        editable = editable,
        values = values,
        header = header,
        grid_js = templates::GRID_SERIALIZE_JS,
    )
}

fn row_to_header(col: usize) -> String {
    let mut n = col + 1;
    let mut s = String::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        s.insert(0, char::from(b'A' + rem as u8));
        n = (n - 1) / 26;
    }
    s
}

// ---------------------------------------------------------------------------
// CSV + formula engine
// ---------------------------------------------------------------------------

fn parse_csv(text: &str) -> Vec<Vec<String>> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut parsed_any = false;
    for rec in rdr.records() {
        match rec {
            Ok(r) => {
                rows.push(r.iter().map(|f| f.to_string()).collect::<Vec<_>>());
                parsed_any = true;
            }
            Err(_) => {}
        }
    }
    if !parsed_any {
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            rows.push(line.split(',').map(|s| s.trim().to_string()).collect());
        }
    }
    rows
}

fn write_csv(rows: &[Vec<String>]) -> std::io::Result<Vec<u8>> {
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    for row in rows {
        let rec: Vec<&str> = row.iter().map(|s| s.as_str()).collect();
        wtr.write_record(&rec)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    }
    wtr.into_inner().map_err(|e| e.into_error())
}

/// Column letters (`A`..) → zero-based index.
fn col_letter_to_idx(col: &str) -> Option<usize> {
    let upper = col.trim().to_uppercase();
    if upper.is_empty() || !upper.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut idx: usize = 0;
    for c in upper.chars() {
        idx = idx * 26 + ((c as u8 - b'A') as usize + 1);
    }
    Some(idx - 1)
}

/// `A1` → `(col, row)` zero-based.
fn parse_cell_ref(s: &str) -> Option<(usize, usize)> {
    let s = s.trim();
    let mut split = 0;
    while split < s.len() && s.as_bytes()[split].is_ascii_alphabetic() {
        split += 1;
    }
    if split == 0 || split == s.len() {
        return None;
    }
    let col = col_letter_to_idx(&s[..split])?;
    let row: usize = s[split..].parse().ok()?;
    if row == 0 {
        return None;
    }
    Some((col, row - 1))
}

/// `A1:B2` → `(c1, r1, c2, r2)`.
fn parse_range(s: &str) -> Option<(usize, usize, usize, usize)> {
    let s = s.trim();
    let (start, end) = match s.find(':') {
        Some(pos) => (&s[..pos], &s[pos + 1..]),
        None => (s, s),
    };
    let (c1, r1) = parse_cell_ref(start)?;
    let (c2, r2) = parse_cell_ref(end)?;
    Some((c1.min(c2), r1.min(r2), c1.max(c2), r2.max(r2)))
}

fn parse_func(body: &str) -> Option<(String, String)> {
    let paren = body.find('(')?;
    let close = body.rfind(')')?;
    if close < paren {
        return None;
    }
    let name = body[..paren].trim().to_lowercase();
    let inner = body[paren + 1..close].trim().to_string();
    if name != "sum" && name != "avg" && name != "average" {
        return None;
    }
    Some((name, inner))
}

fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() || s.starts_with('=') {
        return None;
    }
    s.parse::<f64>().ok()
}

/// Evaluate a single `=SUM(...)` / `=AVG(...)` formula against the raw grid.
fn eval_formula(formula: &str, grid: &[Vec<String>]) -> Option<f64> {
    let f = formula.trim();
    if !f.starts_with('=') {
        return None;
    }
    let body = f[1..].trim();
    let (func, range) = parse_func(body)?;
    let (c1, r1, c2, r2) = parse_range(&range)?;
    let mut vals: Vec<f64> = Vec::new();
    for r in r1..=r2 {
        if r >= grid.len() {
            continue;
        }
        for c in c1..=c2 {
            if c >= grid[r].len() {
                continue;
            }
            if let Some(v) = parse_number(&grid[r][c]) {
                vals.push(v);
            }
        }
    }
    match func.as_str() {
        "sum" => Some(vals.iter().sum()),
        "avg" | "average" => {
            if vals.is_empty() {
                Some(0.0)
            } else {
                Some(vals.iter().sum::<f64>() / vals.len() as f64)
            }
        }
        _ => None,
    }
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

/// Replace every `=...` cell with its computed numeric value. The raw grid is
/// read-only so formula cells referencing other formula cells do not recurse.
/// Cells with unsupported formulas become a `#ERROR` marker (surfaced both in
/// the rendered grid and in CSV export) instead of keeping raw `=...` text.
fn compute_grid(grid: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut out = grid.to_vec();
    for row in out.iter_mut() {
        for cell in row.iter_mut() {
            if cell.trim_start().starts_with('=') {
                let raw = cell.trim().to_string();
                if let Some(v) = eval_formula(cell, grid) {
                    *cell = fmt_num(v);
                } else {
                    *cell = format!(
                        "#ERROR: unsupported formula '{}' (supported: =SUM(range), =AVG(range))",
                        raw
                    );
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn col_letter_roundtrip() {
        assert_eq!(col_letter_to_idx("A"), Some(0));
        assert_eq!(col_letter_to_idx("Z"), Some(25));
        assert_eq!(col_letter_to_idx("AA"), Some(26));
    }

    #[test]
    fn parse_cell_ref_basic() {
        assert_eq!(parse_cell_ref("A1"), Some((0, 0)));
        assert_eq!(parse_cell_ref("B2"), Some((1, 1)));
        assert_eq!(parse_cell_ref("A10"), Some((0, 9)));
    }

    #[test]
    fn parse_range_basic() {
        assert_eq!(parse_range("A1:A3"), Some((0, 0, 0, 2)));
        assert_eq!(parse_range("A1:B2"), Some((0, 0, 1, 1)));
    }

    #[test]
    fn sum_formula_evaluates() {
        let grid = vec![
            vec!["1".to_string(), "=SUM(A1:A3)".to_string()],
            vec!["2".to_string(), "".to_string()],
            vec!["3".to_string(), "".to_string()],
        ];
        let computed = compute_grid(&grid);
        assert_eq!(computed[0][1], "6");
    }

    #[test]
    fn avg_formula_evaluates() {
        let grid = vec![
            vec!["2".to_string(), "".to_string()],
            vec!["4".to_string(), "=AVG(A1:A2)".to_string()],
        ];
        let computed = compute_grid(&grid);
        assert_eq!(computed[1][1], "3");
    }

    #[test]
    fn parse_csv_grid() {
        let rows = parse_csv("1,2,3\n4,5,6");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["1", "2", "3"]);
        assert_eq!(rows[1], vec!["4", "5", "6"]);
    }

    #[test]
    fn write_csv_roundtrip() {
        let rows = parse_csv("1,2\n3,4");
        let bytes = write_csv(&rows).expect("CSV de prueba válido");
        let s = String::from_utf8_lossy(&bytes).to_string();
        assert!(s.contains("1,2"));
        assert!(s.contains("3,4"));
    }
}
