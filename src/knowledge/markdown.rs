//! Parser Markdown/Obsidian y renderizador HTML para el segundo cerebro.
//!
//! Parser minimalista escrito a mano (sin dependencias nuevas, sin
//! `pulldown-cmark`): frontmatter `key: value`, wikilinks `[[target|alias]]`,
//! headings ATX (`#`/`##`/`###`) y conteo de palabras. [`render_html`] escapa
//! la salida y aplica los tokens del sistema Stitch definidos en
//! `src/academic/templates/mod.rs`; los wikilinks se resuelven contra el
//! conjunto de notas y apuntan a la **URL real** de la nota
//! (`/app/notes/{slug}`), no a un ancla inexistente; los rotos se muestran
//! como texto plano con la clase `text-outline` — el render nunca falla
//! (nunca 500). [`toc_html`] genera el índice de la nota: enlaces a los
//! anchors que emite `render_html`.

use std::collections::{BTreeMap, HashSet};

/// Delimitador de frontmatter YAML.
const FM_DELIM: &str = "---";

/// Caracteres que separan palabras al generar slugs (además de los espacios).
const SEPARATORS: &str = "-–—/\\:;,()[]'\".+_?!&%#@*=<>|~^${}«»";

/// Documento Markdown ya parseado, listo para analizar enlaces y renderizar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedDoc {
    /// Frontmatter `key: value`, ordenado por clave.
    pub front: BTreeMap<String, String>,
    /// Cuerpo del documento, sin el bloque de frontmatter.
    pub body: String,
    /// Wikilinks únicos en orden de aparición: `(target, alias)`. Sin `|`, el
    /// alias es el propio target (los `[[Nota#Sección]]` se reducen a `Nota`).
    pub wikilinks: Vec<(String, String)>,
    /// Headings `(nivel, texto)` en orden de aparición (solo niveles 1–3).
    pub headings: Vec<(u8, String)>,
    /// Palabras del cuerpo, contadas partiendo por espacios.
    pub words: usize,
}

/// Parsea un documento Markdown/Obsidian: frontmatter, wikilinks, headings y
/// conteo de palabras. Tolerante a entradas raras: sin frontmatter, sin
/// cierre `---` o con wikilinks sin cerrar, todo queda en el cuerpo y el
/// render degenera a texto plano.
pub fn parse(src: &str) -> ParsedDoc {
    let (front, raw_body) = split_frontmatter(src);
    let body = raw_body.trim().to_string();
    ParsedDoc {
        front,
        wikilinks: extract_wikilinks(&body),
        headings: extract_headings(&body),
        words: body.split_whitespace().count(),
        body,
    }
}

/// Separa un bloque `---\nkey: value\n---` del cuerpo. Devuelve
/// `(frontmatter, cuerpo)`. Sin apertura/cierre válido el frontmatter queda
/// vacío y el cuerpo es la entrada completa.
fn split_frontmatter(src: &str) -> (BTreeMap<String, String>, String) {
    let mut front = BTreeMap::new();
    let trimmed = src.trim_start_matches(['\n', ' ', '\t']);
    let lines: Vec<&str> = trimmed.lines().collect();
    if lines.first().map_or(true, |l| l.trim() != FM_DELIM) {
        return (front, src.to_string());
    }
    // Índice de la línea de cierre (`---` o `...`).
    let mut close = None;
    for (i, line) in lines.iter().enumerate().skip(1) {
        let t = line.trim();
        if t == FM_DELIM || t == "..." {
            close = Some(i);
            break;
        }
    }
    let Some(close) = close else {
        return (front, src.to_string());
    };
    for line in &lines[1..close] {
        if let Some((key, value)) = parse_kv(line) {
            front.insert(key, value);
        }
    }
    (front, lines[close + 1..].join("\n"))
}

/// Parsea una línea `key: value` (limpiando comillas del valor). Las listas
/// inline (`tags: [a, b]`) se guardan tal cual y los comentarios se descartan.
fn parse_kv(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, value) = trimmed.split_once(':')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), unquote(value.trim())))
}

/// Quita comillas envolventes de un valor escalar YAML.
fn unquote(s: &str) -> String {
    if s.len() >= 2 {
        let first = s.chars().next().unwrap();
        let last = s.chars().last().unwrap();
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// Extrae wikilinks `[[target|alias]]` / `[[target]]` sin duplicados, en
/// orden de aparición. Los `[[Nota#Sección]]` se reducen a `Nota`.
fn extract_wikilinks(body: &str) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != b'[' || bytes[i + 1] != b'[' {
            i += 1;
            continue;
        }
        match body.get(i + 2..).and_then(|rest| rest.find("]]")) {
            Some(close) if close > 0 => {
                if let Some(link) = split_target_alias(body[i + 2..i + 2 + close].trim()) {
                    if seen.insert(link.clone()) {
                        links.push(link);
                    }
                }
                i += close + 4; // salta `[[inner]]`
            }
            _ => {
                i += 2; // `[[` sin `]]`: se salta y sigue
            }
        }
    }
    links
}

/// Wikilink parseado desde el interior de `[[…]]`.
///
/// Formas admitidas (las cuatro de Obsidian): `[[Nota]]`, `[[Nota|alias]]`,
/// `[[Nota#Sección]]` y `[[Nota#Sección|alias]]`.
struct WikiLink {
    /// Target: la nota referenciada (antes de `#` y de `|`).
    target: String,
    /// Sección tras `#` (opcional).
    heading: Option<String>,
    /// Texto visible explícito tras `|` (opcional).
    alias: Option<String>,
}

impl WikiLink {
    /// Texto visible del enlace: alias explícito > sección > target.
    fn display(&self) -> &str {
        self.alias
            .as_deref()
            .or(self.heading.as_deref())
            .unwrap_or(&self.target)
    }
}

/// Parsea el interior de `[[…]]`: target, sección y alias. `None` si el
/// target queda vacío (`[[]]`, `[[#sección]]`): eso no es un enlace.
fn parse_wikilink(inner: &str) -> Option<WikiLink> {
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return None;
    }
    // El primer `|` separa el alias (el texto visible); el resto es el target.
    let (target_part, alias) = match trimmed.split_once('|') {
        Some((t, a)) => (t, Some(a.trim().to_string())),
        None => (trimmed, None),
    };
    // El primer `#` separa la sección dentro del target.
    let (target, heading) = match target_part.split_once('#') {
        Some((t, h)) => (t.trim().to_string(), Some(h.trim().to_string())),
        None => (target_part.trim().to_string(), None),
    };
    if target.is_empty() {
        return None;
    }
    Some(WikiLink {
        target,
        heading,
        alias,
    })
}

/// Parte el interior de `[[…]]` en `(target, alias)` para el índice de
/// wikilinks: el target es la nota (sin `#sección`) y el alias el texto
/// explícito tras `|`; sin `|`, el alias es el propio target (los
/// `[[Nota#Sección]]` se reducen a `Nota`).
fn split_target_alias(inner: &str) -> Option<(String, String)> {
    let link = parse_wikilink(inner)?;
    let target = link.target;
    let alias = link.alias.unwrap_or_else(|| target.clone());
    Some((target, alias))
}

/// Recolecta los headings ATX de niveles 1–3, ignorando lo que esté dentro
/// de cercos de código (` ``` `/`~~~`).
fn extract_headings(body: &str) -> Vec<(u8, String)> {
    let mut headings = Vec::new();
    let mut in_fence = false;
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            if let Some((level, text)) = heading_of(t) {
                headings.push((level, text.to_string()));
            }
        }
    }
    headings
}

/// Clasifica una línea ATX: `Some((1..=3, texto))` si es `#`/`##`/`###`
/// seguida de espacio y texto; `None` en otro caso (incluye `####` o más).
fn heading_of(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if !(1..=3).contains(&hashes) {
        return None;
    }
    let rest = line.get(hashes..)?;
    let text = rest.trim();
    if rest.starts_with(' ') && !text.is_empty() {
        Some((hashes as u8, text))
    } else {
        None
    }
}

/// Clases Stitch compartidas por todos los headings renderizados (mismo
/// look de los encabezados del dashboard académico).
const HEADING_CLASSES: &str =
    "font-headline-sm text-headline-sm text-primary-container font-semibold";

/// Clases Stitch de los párrafos del cuerpo.
const PARAGRAPH_CLASSES: &str = "font-body-md text-body-md text-on-surface";

/// Clases Stitch de los spans de código inline.
const CODE_CLASSES: &str =
    "font-code-inline text-code-inline bg-surface-container-high text-primary-fixed-dim px-1 rounded";

/// Clases Stitch de los enlaces del índice (TOC) de la nota.
const TOC_LINK_CLASSES: &str =
    "block text-body-sm text-on-surface-variant hover:text-primary-container hover:underline transition-colors";

/// Clases Stitch de la lista del índice (TOC) de la nota.
const TOC_LIST_CLASSES: &str = "list-none space-y-space-xs";

/// Píxeles de sangría por nivel de heading en el índice: la jerarquía se
/// marca con `padding-left` en el `<li>` (nivel 1 → 0 px, 2 → 16 px, 3 → 32 px).
const TOC_INDENT_PX: usize = 16;

/// Renderiza el cuerpo como HTML Stitch: escapa `& < > " '`, aplica
/// `**bold**`, `*italic*` y `` `code` ``, resuelve wikilinks contra el
/// conjunto de notas (`notes`, con los slugs de la app) y emite los headings
/// con su anchor único.
///
/// Cada wikilink que corresponde a una nota existente se convierte en un
/// anchor a su **URL real** (`/app/notes/{slug}`) —nunca a un ancla
/// `#slug`— con el texto visible del enlace (alias explícito, sección o
/// target). Los wikilinks sin nota que los respalde salen como texto plano
/// con la clase `text-outline`: el render nunca truena (nunca 500).
pub fn render_html(doc: &ParsedDoc, notes: &HashSet<String>) -> String {
    let mut out = String::new();
    let mut used_ids: HashSet<String> = HashSet::new();
    let mut para: Vec<&str> = Vec::new();
    let mut in_fence = false;

    for line in doc.body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            // Los cercos de código no se muestran; sus líneas tampoco ejecutan
            // énfasis ni headings (coherente con `extract_headings`).
            in_fence = !in_fence;
            continue;
        }
        let heading = if in_fence { None } else { heading_of(t) };
        if let Some((_, text)) = heading {
            if !para.is_empty() {
                out.push_str(&paragraph_html(&para, notes));
                para.clear();
            }
            let id = unique_heading_id(text, &mut used_ids);
            out.push_str(&format!(
                r###"<h2 id="{}" class="{}">{}</h2>"###,
                id,
                HEADING_CLASSES,
                html_escape(text)
            ));
            continue;
        }
        if t.is_empty() {
            if !para.is_empty() {
                out.push_str(&paragraph_html(&para, notes));
                para.clear();
            }
        } else {
            para.push(t);
        }
    }
    if !para.is_empty() {
        out.push_str(&paragraph_html(&para, notes));
    }
    out
}

/// Índice de la nota (TOC) en HTML: lista de enlaces a los headings del
/// documento, **sin estado "actual"** (no hay scroll-spy: los `href` son
/// anchors estáticos a los `<h2>` que emite [`render_html`]).
///
/// Los ids salen del **mismo** generador y con la **misma** deduplicación que
/// `render_html` (`slugify` del texto + sufijos `-2`, `-3`… ante choques), de
/// modo que cada `href="#id"` casa con el `id` del `<h2>` correspondiente. La
/// jerarquía se marca con `padding-left` en el `<li>` (0/16/32 px para los
/// niveles 1/2/3). Sin headings devuelve cadena vacía ("" = nota sin índice).
///
/// Uso (wave posterior, en `src/handlers/notes.rs`):
/// ```ignore
/// let doc = crate::knowledge::markdown::parse(&nota_md);
/// let toc = crate::knowledge::markdown::toc_html(&doc);
/// // `toc` se inyecta en el <aside> de la nota; "" si no hay headings.
/// ```
pub fn toc_html(doc: &ParsedDoc) -> String {
    if doc.headings.is_empty() {
        return String::new();
    }
    // `doc.headings` se recogió con el mismo recorrido del cuerpo que usa
    // `render_html` (mismo orden, mismos textos), así que recorrerlo aquí con
    // el mismo generador de ids replica su deduplicación exacta.
    let mut used_ids: HashSet<String> = HashSet::new();
    let mut items = String::new();
    for (level, text) in &doc.headings {
        let id = unique_heading_id(text, &mut used_ids);
        let indent = (*level).saturating_sub(1) as usize * TOC_INDENT_PX;
        items.push_str(&format!(
            r##"<li class="list-none" style="padding-left:{indent}px"><a href="#{id}" class="{link}">{text}</a></li>"##,
            indent = indent,
            id = id,
            link = TOC_LINK_CLASSES,
            text = html_escape(text)
        ));
    }
    format!(
        r##"<nav aria-label="Índice de la nota" class="flex flex-col"><ul class="{list}">{items}</ul></nav>"##,
        list = TOC_LIST_CLASSES,
        items = items
    )
}

/// Une las líneas de un párrafo (cada una con su render inline) separadas por
/// `<br>` dentro de un `<p>` de estilo body.
fn paragraph_html(lines: &[&str], notes: &HashSet<String>) -> String {
    let inlines: Vec<String> = lines.iter().map(|l| inline_html(l, notes)).collect();
    format!(
        r###"<p class="{}">{}</p>"###,
        PARAGRAPH_CLASSES,
        inlines.join("<br>")
    )
}

/// Render inline: primero extrae los spans `` `code` `` (en su `<code>` con
/// escape), luego los wikilinks y por último énfasis sobre el texto restante.
fn inline_html(raw: &str, notes: &HashSet<String>) -> String {
    let mut out = String::new();
    let mut rest = raw;
    loop {
        match rest.find('`') {
            Some(open) => match rest.get(open + 1..).and_then(|r| r.find('`')) {
                Some(close) if close > 0 => {
                    out.push_str(&wikilink_html(&rest[..open], notes));
                    out.push_str(&format!(
                        r###"<code class="{}">"###,
                        CODE_CLASSES
                    ));
                    out.push_str(&html_escape(&rest[open + 1..open + 1 + close]));
                    out.push_str("</code>");
                    rest = &rest[open + close + 2..];
                }
                _ => {
                    // Backtick sin par: literal.
                    out.push_str(&wikilink_html(&rest[..open + 1], notes));
                    rest = &rest[open + 1..];
                }
            },
            None => {
                out.push_str(&wikilink_html(rest, notes));
                break;
            }
        }
    }
    out
}

/// Resuelve los wikilinks de un texto sin code-spans: los links cuyo target
/// corresponde a una nota existente se vuelven anchors a la **URL real** de
/// esa nota (`/app/notes/{slug}`) con el texto visible; los rotos quedan como
/// texto plano escapado con la clase `text-outline` — nunca 500.
fn wikilink_html(raw: &str, notes: &HashSet<String>) -> String {
    let mut out = String::new();
    let mut rest = raw;
    loop {
        match rest.find("[[") {
            Some(open) => match rest.get(open + 2..).and_then(|r| r.find("]]")) {
                Some(close) if close > 0 => {
                    out.push_str(&emphasis_html(&rest[..open]));
                    let inner = rest[open + 2..open + 2 + close].trim();
                    match parse_wikilink(inner) {
                        Some(link) => {
                            let display = link.display();
                            match resolve_note_slug(&link.target, notes) {
                                // Nota existente: enlace a su URL real.
                                Some(slug) => out.push_str(&format!(
                                    r###"<a class="{link}" href="{href}">{text}</a>"###,
                                    link = NOTE_LINK_CLASSES,
                                    href = html_escape(&format!(
                                        "{prefix}{slug}",
                                        prefix = NOTE_URL_PREFIX
                                    )),
                                    text = html_escape(display)
                                )),
                                // Enlace roto: texto plano (con la clase que lo
                                // delata), el render no truena.
                                None => {
                                    let title = if display == link.target {
                                        String::new()
                                    } else {
                                        format!(r###" title="{}""###, html_escape(&link.target))
                                    };
                                    out.push_str(&format!(
                                        r###"<span class="{cls}"{title}>{text}</span>"###,
                                        cls = BROKEN_LINK_CLASSES,
                                        title = title,
                                        text = html_escape(display)
                                    ));
                                }
                            }
                        }
                        // `[[…]]` sin target (`[[]]`, `[[#sec]]`): literal.
                        None => out.push_str(&html_escape(&rest[open..open + close + 4])),
                    }
                    rest = &rest[open + close + 4..];
                }
                _ => {
                    out.push_str(&emphasis_html(&rest[..open + 2]));
                    rest = &rest[open + 2..];
                }
            },
            None => {
                out.push_str(&emphasis_html(rest));
                break;
            }
        }
    }
    out
}

/// Prefijo de la ruta real de una nota en la app (`/app/notes/{slug}`).
const NOTE_URL_PREFIX: &str = "/app/notes/";

/// Clases Stitch del enlace a una nota existente.
const NOTE_LINK_CLASSES: &str = "text-primary-container underline";

/// Clases Stitch del texto de un enlace roto (no hay nota con ese slug).
const BROKEN_LINK_CLASSES: &str = "text-outline";

/// Longitud máxima de un slug de nota (mismo límite que las rutas de la app).
const MAX_NOTE_SLUG_LEN: usize = 200;

/// Resuelve el target de un wikilink al slug real de una nota existente.
///
/// `notes` es el conjunto de notas conocidas (los slugs de la app; por
/// tolerancia también se aceptan títulos). Devuelve `Some(slug)` con el slug
/// **exacto** que vive en la BD —el que forma la URL real
/// `/app/notes/{slug}`— o `None` si ninguna nota corresponde (enlace roto).
///
/// El slug se deriva con `Note::slugify`, la misma función con la que la app
/// crea las notas (`src/handlers/notes.rs`), de modo que el href coincide con
/// la URL que sirve el router.
fn resolve_note_slug(target: &str, notes: &HashSet<String>) -> Option<String> {
    let t = target.trim();
    if t.is_empty() {
        return None;
    }
    // 1. Slug canónico de la app: es el que guarda la BD.
    let canon = crate::models::Note::slugify(t);
    // 2. Slug plegado sin acentos (este módulo), por si el conjunto usa esa
    //    forma (`[[Física II]]` → `fisica-ii`).
    let folded = slugify(t);
    // 3. El conjunto puede traer títulos en vez de slugs: se acepta el texto
    //    literal y se deriva su slug canónico.
    let matched = if !canon.is_empty() && notes.contains(&canon) {
        canon
    } else if !folded.is_empty() && notes.contains(&folded) {
        folded
    } else if notes.contains(t) {
        canon
    } else {
        return None;
    };
    // El slug tiene que ser apto para un href (nada de `..`, `/`, comillas…).
    if !is_safe_note_slug(&matched) {
        return None;
    }
    Some(matched)
}

/// ¿Es un slug apto para aparecer dentro de un `href`?
///
/// Solo se admiten letras, dígitos, `-` y `_`: nada de `..`, `/`, `\`,
/// comillas ni espacios, así que el href no puede romper el atributo ni
/// apuntar fuera de `/app/notes/`. Las letras Unicode se permiten porque el
/// slug real de la app las conserva (`física-ii`).
fn is_safe_note_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_NOTE_SLUG_LEN
        && !slug.contains("..")
        && slug
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
}

/// Énfasis markdown sobre texto que ya no tiene `` ` `` ni `[[`: se resuelven
/// los pares `**bold**` antes que `*italic*`; los asteriscos sin par quedan
/// literales y escapados.
fn emphasis_html(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    loop {
        let Some(star) = rest.find('*') else {
            out.push_str(&html_escape(rest));
            break;
        };
        let after = &rest[star..];
        if after.starts_with("**") {
            match after.get(2..).and_then(|r| r.find("**")) {
                Some(close) if close > 0 => {
                    out.push_str(&html_escape(&rest[..star]));
                    out.push_str("<strong>");
                    out.push_str(&html_escape(&after[2..2 + close]));
                    out.push_str("</strong>");
                    rest = &after[2 + close + 2..];
                }
                _ => {
                    out.push_str(&html_escape(&after[..2]));
                    rest = &after[2..];
                }
            }
        } else if let Some(close) = after.get(1..).and_then(|r| r.find('*')) {
            out.push_str(&html_escape(&rest[..star]));
            out.push_str("<em>");
            out.push_str(&html_escape(&after[1..1 + close]));
            out.push_str("</em>");
            rest = &after[close + 2..];
        } else {
            // Asterisco sin cierre: literal.
            out.push_str(&html_escape(&after[..1]));
            rest = &after[1..];
        }
    }
    out
}

/// ID único para el anchor de un heading: slug base + sufijo `-2`, `-3`…
/// si hay choque; mínimo `seccion` cuando el slug queda vacío.
fn unique_heading_id(text: &str, used: &mut HashSet<String>) -> String {
    let base = slugify(text);
    let base = if base.is_empty() { "seccion".to_string() } else { base };
    let mut id = base.clone();
    let mut n = 2;
    while used.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    used.insert(id.clone());
    id
}

/// Slug ASCII de un texto: minúsculas, sin acentos, palabras unidas por `-`;
/// cualquier símbolo actúa de separador y los guiones consecutivos colapsan.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    // `true` al inicio: corta guiones de borde izquierdo.
    let mut last_dash = true;
    for ch in s.chars() {
        for low in ch.to_lowercase() {
            match classify(low) {
                Tok::Base(c) => {
                    out.push(c);
                    last_dash = false;
                }
                Tok::Sep => {
                    if !last_dash {
                        out.push('-');
                        last_dash = true;
                    }
                }
                Tok::Drop => {} // descartado (p. ej. diacríticos combinantes)
            }
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

/// Clasificación de un char ya en minúsculas para [`slugify`].
enum Tok {
    /// Pasa al slug tal cual.
    Base(char),
    /// Separa palabras (se colapsa a un solo `-`).
    Sep,
    /// Se descarta silenciosamente.
    Drop,
}

/// Clasifica un carácter minúsculo: ASCII alfanumérico, acentos latinos
/// comunes (que se pliegan a su letra base), separadores o descarte.
fn classify(low: char) -> Tok {
    if low.is_ascii_alphanumeric() {
        return Tok::Base(low);
    }
    if let Some(base) = accent_base(low) {
        return Tok::Base(base);
    }
    if low.is_whitespace() || SEPARATORS.contains(low) {
        return Tok::Sep;
    }
    Tok::Drop
}

/// Pliega acentos latinos frecuentes a su letra base ASCII.
fn accent_base(c: char) -> Option<char> {
    match c {
        'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' => Some('a'),
        'é' | 'è' | 'ë' | 'ê' => Some('e'),
        'í' | 'ì' | 'ï' | 'î' => Some('i'),
        'ó' | 'ò' | 'ö' | 'ô' | 'õ' => Some('o'),
        'ú' | 'ù' | 'ü' | 'û' => Some('u'),
        'ñ' => Some('n'),
        'ç' => Some('c'),
        'ý' | 'ÿ' => Some('y'),
        'š' => Some('s'),
        'ž' => Some('z'),
        'œ' => Some('o'),
        'æ' => Some('a'),
        'ß' => Some('s'),
        'ø' => Some('o'),
        'ł' => Some('l'),
        'ð' => Some('d'),
        _ => None,
    }
}

/// Escapa HTML: `& < > " '` (misma política que `templates::html_escape`,
/// copia local para mantener el módulo autocontenido).
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_frontmatter_wikilinks_headings() {
        let src = "---\ntitle: Física Cuántica\ntags: [ondas, fotones]\n---\n\n# Teoría\n\nHola **mundo** con *énfasis* y `código`.\n\nEnlaza [[Física II|la física]], otra vez [[Física II|la física]] y [[Fantasma]].\n";
        let doc = parse(src);

        // Frontmatter.
        assert_eq!(
            doc.front.get("title").map(String::as_str),
            Some("Física Cuántica")
        );
        assert_eq!(
            doc.front.get("tags").map(String::as_str),
            Some("[ondas, fotones]")
        );

        // Wikilinks sin duplicados: el segundo `[[Física II|la física]]` cae.
        assert_eq!(
            doc.wikilinks,
            vec![
                ("Física II".to_string(), "la física".to_string()),
                ("Fantasma".to_string(), "Fantasma".to_string()),
            ]
        );

        // Un heading de nivel 1 y conteo de palabras por espacios.
        assert_eq!(doc.headings, vec![(1, "Teoría".to_string())]);
        assert_eq!(doc.words, 19);

        // Render con "Física II" existente (slug `física-ii`, el que guarda la
        // app) y "Fantasma" inexistente.
        let mut notes = HashSet::new();
        notes.insert("física-ii".to_string());
        let html = render_html(&doc, &notes);

        assert!(html.contains("<strong>mundo</strong>"));
        assert!(html.contains("<em>énfasis</em>"));
        assert!(html.contains("<code class=\"font-code-inline"));
        // El enlace apunta a la URL REAL de la nota, no a un ancla `#slug`.
        assert!(html
            .contains(r#"<a class="text-primary-container underline" href="/app/notes/física-ii">la física</a>"#));
        assert!(!html.contains("href=\"#"));
        // El enlace roto sale como texto plano con la clase `text-outline`.
        assert!(html.contains(r#"<span class="text-outline">Fantasma</span>"#));
        assert!(!html.contains("Fantasma</a>"));
        // Heading Stitch con anchor único y párrafos con clase body.
        assert!(html
            .contains(r###"<h2 id="teoria" class="font-headline-sm text-headline-sm text-primary-container font-semibold">Teoría</h2>"###));
        assert_eq!(html.matches("<p class=\"font-body-md").count(), 2);
    }

    #[test]
    fn test_wikilink_formas_y_url_real() {
        // Las cuatro formas de Obsidian apuntan al mismo slug real; lo que
        // cambia es el texto visible.
        let src = "[[Física II]]\n\n[[Física II#Ondas]]\n\n[[Física II|la física]]\n\n[[Física II#Ondas|alias]]\n";
        let doc = parse(src);
        let mut notes = HashSet::new();
        notes.insert("física-ii".to_string());
        let html = render_html(&doc, &notes);

        // Target: la URL real de la nota.
        assert!(html
            .contains(r#"<a class="text-primary-container underline" href="/app/notes/física-ii">Física II</a>"#));
        // `[[slug#sección]]`: target = slug, display = sección.
        assert!(html.contains(
            r#"<a class="text-primary-container underline" href="/app/notes/física-ii">Ondas</a>"#
        ));
        // `[[target|alias]]`: el alias manda como texto visible.
        assert!(html
            .contains(r#"<a class="text-primary-container underline" href="/app/notes/física-ii">la física</a>"#));
        // `[[target#sección|alias]]`: manda el alias explícito.
        assert!(html.contains(
            r#"<a class="text-primary-container underline" href="/app/notes/física-ii">alias</a>"#
        ));
        // Nunca un ancla `#slug` (esa página no existe).
        assert!(!html.contains("href=\"#"));
    }

    #[test]
    fn test_wikilink_roto_es_texto_plano_nunca_500() {
        let doc = parse("ver [[Fantasma]] y [[Otra Más|alias roto]]");
        let notes = HashSet::<String>::new();
        let html = render_html(&doc, &notes);

        // Sin nota que lo respalde: texto plano con la clase `text-outline`.
        assert!(html.contains(r#"<span class="text-outline">Fantasma</span>"#));
        assert!(html.contains(r#"<span class="text-outline" title="Otra Más">alias roto</span>"#));
        assert!(!html.contains("<a "));
        assert!(!html.contains("href="));
    }

    #[test]
    fn test_wikilink_slug_inseguro_o_traversal_no_se_enlaza() {
        // Extrae los valores de `href="…"` del HTML renderizado.
        fn hrefs(html: &str) -> Vec<String> {
            let mut out = Vec::new();
            let mut rest = html;
            while let Some(start) = rest.find("href=\"") {
                rest = &rest[start + 6..];
                let Some(end) = rest.find('"') else { break };
                out.push(rest[..end].to_string());
                rest = &rest[end..];
            }
            out
        }

        // Un target con `..` o con barras nunca produce un href con `..`:
        // el slug sale sanitizado o el enlace queda roto.
        let doc = parse("[[../../etc/passwd]]\n\n[[con/barra]]");
        let mut notes = HashSet::new();
        notes.insert("../../etc/passwd".to_string());
        notes.insert("con/barra".to_string());
        let html = render_html(&doc, &notes);

        assert_eq!(
            hrefs(&html),
            vec!["/app/notes/etcpasswd", "/app/notes/conbarra"]
        );
        for href in hrefs(&html) {
            let slug = href.strip_prefix("/app/notes/").unwrap();
            assert!(!slug.contains(".."));
            assert!(!slug.contains('/'));
            assert!(!slug.contains('\\'));
        }

        // Sin nota que lo respalde: texto plano honesto, sin href.
        let notes = HashSet::<String>::new();
        let html = render_html(&doc, &notes);
        assert!(hrefs(&html).is_empty());
        assert!(html.contains(r#"<span class="text-outline">"#));

        // Un slug con comillas no puede romper el atributo `href`.
        let doc = parse("[[evil\"onload=\"x]]");
        let mut notes = HashSet::new();
        notes.insert("evil\"onload=\"x".to_string());
        let html = render_html(&doc, &notes);
        assert_eq!(hrefs(&html), vec!["/app/notes/evilonloadx"]);
        assert!(!html.contains("onload=\""));
    }

    #[test]
    fn test_wikilink_resuelve_titulos_tambien() {
        // El conjunto puede traer títulos (no solo slugs): el href usa el slug
        // canónico de la app.
        let doc = parse("[[Física II]]");
        let mut notes = HashSet::new();
        notes.insert("Física II".to_string());
        let html = render_html(&doc, &notes);

        assert!(html
            .contains(r#"<a class="text-primary-container underline" href="/app/notes/física-ii">Física II</a>"#));
    }

    #[test]
    fn test_slugify_sin_acentos() {
        assert_eq!(slugify("Teoría Atómica"), "teoria-atomica");
        assert_eq!(slugify("¡Ñandú & Pingüino!"), "nandu-pinguino");
        assert_eq!(slugify("!!!Hola Mundo!!!"), "hola-mundo");
        assert_eq!(slugify("   "), "");
    }

    #[test]
    fn test_entrada_rota_nunca_panics() {
        // Sin cierre `---`: el frontmatter queda vacío y todo es cuerpo.
        let doc = parse("---\ntitulo sin cerrar\nel resto es cuerpo");
        assert!(doc.front.is_empty());
        assert_eq!(doc.words, 8);

        // Wikilink sin cerrar y heading dentro de cerco de código.
        let doc = parse("ver [[hueco sin cerrar\n\n```\n# no es heading\n```");
        assert!(doc.wikilinks.is_empty());
        assert!(doc.headings.is_empty());

        let notes = HashSet::<String>::new();
        let html = render_html(&doc, &notes);
        assert!(html.contains("ver [[")); // literal, sin caerse
        assert!(!html.contains("<h2"));
    }

    #[test]
    fn test_toc_refleja_headings() {
        let src = "---\ntitle: Nota\n---\n\n# Teoría\n\ntexto\n\n## Ondas\n\ntexto\n\n### Fase\n\ntexto\n\n## Interferencia\n";
        let doc = parse(src);
        let toc = toc_html(&doc);

        // Un `<li>` por heading, en el orden del documento.
        assert_eq!(toc.matches("<li ").count(), 4);
        // La jerarquía se marca con `padding-left` en el `<li>`.
        assert!(toc.contains(r##"style="padding-left:0px"><a href="#teoria""##));
        assert!(toc.contains(r##"style="padding-left:16px"><a href="#ondas""##));
        assert!(toc.contains(r##"style="padding-left:32px"><a href="#fase""##));
        assert!(toc.contains(r##"style="padding-left:16px"><a href="#interferencia""##));
        // El texto del heading viaja escapado dentro del enlace.
        assert!(toc.contains(">Teoría</a>"));

        // Sin headings no hay índice (cadena vacía, no un `<ul>` vacío).
        let sin = parse("solo texto, ningún heading");
        assert_eq!(toc_html(&sin), "");
    }

    #[test]
    fn test_toc_hrefs_casan_con_render_html() {
        // Headings repetidos y con acentos: la deduplicación (`-2`, `-3`) debe
        // ser idéntica en el índice y en el render.
        let src = "# Teoría\n\n## Sección\n\n## Sección\n\n### Teoría\n\n## Sección\n";
        let doc = parse(src);
        let notes = HashSet::<String>::new();
        let html = render_html(&doc, &notes);
        let toc = toc_html(&doc);

        // Extrae los valores de `id="…"` y de `href="#…"` en orden.
        fn extract(html: &str, marker: &str) -> Vec<String> {
            let mut out = Vec::new();
            let mut rest = html;
            while let Some(start) = rest.find(marker) {
                rest = &rest[start + marker.len()..];
                let Some(end) = rest.find('"') else { break };
                out.push(rest[..end].to_string());
                rest = &rest[end..];
            }
            out
        }

        let ids = extract(&html, "id=\"");
        let hrefs = extract(&toc, "href=\"#");
        assert_eq!(ids, vec!["teoria", "seccion", "seccion-2", "teoria-2", "seccion-3"]);
        // Cada `href` del índice apunta a un `id` real del render.
        assert_eq!(hrefs, ids);
    }
}
