const std = @import("std");
const http = @import("http.zig");
const sqlite = @import("sqlite.zig");
const config_mod = @import("config.zig");
const json = @import("jsonutil.zig");
const security = @import("security.zig");
const templates = @import("templates.zig");
const mcp_client = @import("mcp_client.zig");

const MAX_TITLE = 200;
const MAX_CONTENT = 500_000;
const MAX_CSV = 1_000_000;
const MAX_HTML_CONTENT = 1_048_576;
const MAX_SHEET_ROWS = 5000;
const MAX_SHEET_COLUMNS = 500;
const DOCX_MIME = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const MAX_LIBRARY_FILE = 32 * 1024 * 1024;
const Slide = struct { title: []const u8, content: []const u8 };

pub fn handle(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config, request: *const http.Request, username: []const u8) !http.Response {
    const root = try officeRoot(allocator, config);
    for ([_][]const u8{ "docs", "sheets", "presentations", "html", "library", "reports" }) |subdir| {
        const dir = try std.fs.path.join(allocator, &.{ root, subdir });
        try std.fs.cwd().makePath(dir);
    }

    const path = request.path;
    if (std.mem.eql(u8, path, "/office/report")) {
        if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
        return report(allocator, db, config);
    }
    if (std.mem.eql(u8, path, "/office")) return home(allocator, config, request, username, root);
    if (std.mem.eql(u8, path, "/office/docs")) return documents(allocator, config, request, username, root);
    if (std.mem.startsWith(u8, path, "/office/docs/")) return documentRoute(allocator, config, request, username, root);
    if (std.mem.eql(u8, path, "/office/sheets")) return sheets(allocator, config, request, username, root);
    if (std.mem.startsWith(u8, path, "/office/sheets/")) return sheetRoute(allocator, config, request, username, root);
    if (std.mem.eql(u8, path, "/office/presentations")) return presentations(allocator, config, request, username, root);
    if (std.mem.startsWith(u8, path, "/office/presentations/")) return presentationRoute(allocator, config, request, username, root);
    if (std.mem.eql(u8, path, "/office/html")) return htmlPages(allocator, config, request, username, root);
    if (std.mem.startsWith(u8, path, "/office/html/")) return htmlPageRoute(allocator, config, request, username, root);
    if (std.mem.eql(u8, path, "/office/library")) return library(allocator, config, request, username, root);
    if (std.mem.startsWith(u8, path, "/office/library/")) return libraryRoute(allocator, config, request, username, root);
    return page(allocator, config, request.path, username, "No encontrado", "<p>Esta función de NOVA Office no existe.</p>", 404);
}

fn home(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8) !http.Response {
    const docs = try countFiles(allocator, root, "docs", ".md");
    const sheets_count = try countFiles(allocator, root, "sheets", ".csv");
    const decks = try countFiles(allocator, root, "presentations", ".json");
    const html_count = try countFiles(allocator, root, "html", ".html");
    var recent = std.ArrayList(u8).init(allocator);
    var recent_count: usize = 0;
    for ([_]struct { kind: []const u8, ext: []const u8, icon: []const u8 }{
        .{ .kind = "docs", .ext = ".md", .icon = "description" },
        .{ .kind = "sheets", .ext = ".csv", .icon = "table_chart" },
        .{ .kind = "presentations", .ext = ".json", .icon = "present_to_all" },
        .{ .kind = "html", .ext = ".html", .icon = "code" },
    }) |item| {
        if (recent_count >= 6) break;
        const dir = try officeDir(allocator, root, item.kind);
        var opened = std.fs.cwd().openDir(dir, .{ .iterate = true }) catch continue;
        defer opened.close();
        var iter = opened.iterate();
        while (try iter.next()) |entry| {
            if (recent_count >= 6) break;
            if (entry.kind != .file or !std.mem.endsWith(u8, entry.name, item.ext)) continue;
            const slug = entry.name[0 .. entry.name.len - item.ext.len];
            const escaped = try security.htmlEscape(allocator, slug);
            try recent.writer().print("<article class=\"gen-office-file-card {s}\"><a class=\"gen-office-file-preview\" href=\"/office/{s}/{s}\"><span class=\"material-symbols-outlined\">{s}</span><small>{s}</small></a><div class=\"gen-office-file-copy\"><a href=\"/office/{s}/{s}\">{s}</a><span>Archivo local</span></div><a class=\"gen-office-file-arrow\" href=\"/office/{s}/{s}\" aria-label=\"Abrir {s}\">↗</a></article>", .{ item.kind, item.kind, escaped, item.icon, item.ext[1..], item.kind, escaped, escaped, item.kind, escaped, escaped });
            recent_count += 1;
        }
    }
    const recent_html = if (recent_count == 0) "<div class=\"gen-office-empty\"><span class=\"material-symbols-outlined\">folder_open</span><h3>Tu espacio está listo.</h3><p>Crea tu primer documento, hoja, presentación o página HTML.</p></div>" else recent.items;
    const tabs = try officeTabs(allocator, "home");
    const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-home\">{s}<header class=\"gen-office-home-hero\"><div class=\"gen-office-home-kicker\"><span></span> LOCAL · PRIVADO · TUYO</div><h1>¿Qué vas a crear hoy?</h1><p>Escribe, calcula y presenta desde un mismo espacio. Tus archivos permanecen en NOVA.</p><form class=\"gen-office-home-search\" method=\"get\" action=\"/office/library/search\"><span class=\"material-symbols-outlined\">search</span><label class=\"sr-only\" for=\"office-home-search\">Buscar en tus archivos</label><input id=\"office-home-search\" type=\"search\" name=\"q\" placeholder=\"Buscar en tus archivos…\"><button type=\"submit\">Buscar</button></form></header><section class=\"gen-office-launch-section\"><div class=\"gen-office-section-head\"><h2>Crear nuevo</h2><span>Elige una aplicación</span></div><div class=\"gen-office-launch-grid\"><a class=\"gen-office-launcher docs\" href=\"/office/docs/new\"><span class=\"gen-office-launcher-icon\"><span class=\"material-symbols-outlined\">description</span></span><div><strong>Documento</strong><small>Markdown · .md</small></div><span class=\"material-symbols-outlined\">add</span></a><a class=\"gen-office-launcher sheets\" href=\"/office/sheets/new\"><span class=\"gen-office-launcher-icon\"><span class=\"material-symbols-outlined\">table_chart</span></span><div><strong>Hoja de cálculo</strong><small>Cuadrícula · .csv</small></div><span class=\"material-symbols-outlined\">add</span></a><a class=\"gen-office-launcher presentations\" href=\"/office/presentations/new\"><span class=\"gen-office-launcher-icon\"><span class=\"material-symbols-outlined\">present_to_all</span></span><div><strong>Presentación</strong><small>Diapositivas · .json</small></div><span class=\"material-symbols-outlined\">add</span></a><a class=\"gen-office-launcher html\" href=\"/office/html/new\"><span class=\"gen-office-launcher-icon html\"><span class=\"material-symbols-outlined\">code</span></span><div><strong>Página web</strong><small>HTML · preview aislado</small></div><span class=\"material-symbols-outlined\">add</span></a><a class=\"gen-office-launcher library\" href=\"/office/library\"><span class=\"gen-office-launcher-icon\"><span class=\"material-symbols-outlined\">folder_open</span></span><div><strong>Biblioteca</strong><small>Todos tus archivos</small></div><span class=\"material-symbols-outlined\">arrow_outward</span></a></div></section><section class=\"gen-office-recent\"><div class=\"gen-office-section-head\"><h2>Archivos recientes</h2><a href=\"/office/library\">Ver todos →</a></div><div class=\"gen-office-file-grid\">{s}</div></section><footer class=\"gen-office-home-footer\"><span>Guardado local · {d} documentos · {d} hojas · {d} presentaciones · {d} páginas HTML</span><span>NOVA Office</span></footer></div>", .{ tabs, recent_html, docs, sheets_count, decks, html_count });
    return page(allocator, config, request.path, username, "NOVA Office", body, 200);
}

fn officeTabs(allocator: std.mem.Allocator, active: []const u8) ![]const u8 {
    return std.fmt.allocPrint(allocator, "<nav class=\"gen-office-tabs\" aria-label=\"Pestañas de Oficina\"><a class=\"gen-office-brand-tab{s}\" href=\"/office\"><span class=\"gen-office-brand-mark\"><span class=\"material-symbols-outlined\">grid_view</span></span><strong>NOVA <span>Office</span></strong></a><a class=\"gen-office-file-tab{s}\" href=\"/office/docs\"><span class=\"gen-office-file-icon docs\">▤</span><span>Documentos</span></a><a class=\"gen-office-file-tab{s}\" href=\"/office/sheets\"><span class=\"gen-office-file-icon sheets\">▦</span><span>Hojas</span></a><a class=\"gen-office-file-tab{s}\" href=\"/office/presentations\"><span class=\"gen-office-file-icon presentations\">▣</span><span>Presentaciones</span></a><a class=\"gen-office-file-tab{s}\" href=\"/office/html\"><span class=\"gen-office-file-icon html\">&lt;/&gt;</span><span>HTML</span></a><span class=\"gen-office-tabs-spacer\"></span><a class=\"gen-office-library-tab{s}\" href=\"/office/library\"><span class=\"material-symbols-outlined\">folder_open</span><span>Biblioteca</span></a></nav>", .{
        if (std.mem.eql(u8, active, "home")) " is-active" else "",
        if (std.mem.eql(u8, active, "docs")) " is-active" else "",
        if (std.mem.eql(u8, active, "sheets")) " is-active" else "",
        if (std.mem.eql(u8, active, "presentations")) " is-active" else "",
        if (std.mem.eql(u8, active, "html")) " is-active" else "",
        if (std.mem.eql(u8, active, "library")) " is-active" else "",
    });
}

fn officeNav(allocator: std.mem.Allocator, active: []const u8) ![]const u8 {
    return std.fmt.allocPrint(allocator, "<nav class=\"gen-office-appnav\" aria-label=\"Aplicaciones de oficina\"><a{s} href=\"/office/docs\"><span class=\"material-symbols-outlined\">description</span>Documentos</a><a{s} href=\"/office/sheets\"><span class=\"material-symbols-outlined\">table_chart</span>Hojas</a><a{s} href=\"/office/presentations\"><span class=\"material-symbols-outlined\">present_to_all</span>Presentaciones</a><a{s} href=\"/office/html\"><span class=\"material-symbols-outlined\">code</span>HTML</a><a{s} href=\"/office/library\"><span class=\"material-symbols-outlined\">folder_open</span>Biblioteca</a></nav>", .{
        if (std.mem.eql(u8, active, "docs")) " class=\"is-active\"" else "",
        if (std.mem.eql(u8, active, "sheets")) " class=\"is-active sheets\"" else "",
        if (std.mem.eql(u8, active, "presentations")) " class=\"is-active presentations\"" else "",
        if (std.mem.eql(u8, active, "html")) " class=\"is-active html\"" else "",
        if (std.mem.eql(u8, active, "library")) " class=\"is-active library\"" else "",
    });
}

fn officeAiToggle() []const u8 {
    return "<button type=\"button\" class=\"gen-office-ribbon-button\" data-office-ai-toggle onclick=\"toggleOfficeAi()\" aria-controls=\"gen-office-ai-panel\" aria-expanded=\"false\"><span class=\"material-symbols-outlined\">auto_awesome</span>Herramientas</button>";
}

fn officeAiPanel(allocator: std.mem.Allocator, title: []const u8, description: []const u8, content: []const u8) ![]const u8 {
    return std.fmt.allocPrint(allocator, "<button type=\"button\" class=\"gen-office-ai-backdrop\" onclick=\"closeOfficeAi()\" aria-label=\"Cerrar herramientas\"></button><aside class=\"gen-office-ai\" id=\"gen-office-ai-panel\" aria-label=\"Herramientas de Oficina\"><header><div><span class=\"gen-office-ai-mark\">✦</span><strong>NOVA AI</strong></div><button type=\"button\" class=\"gen-office-icon-button\" onclick=\"closeOfficeAi()\" aria-label=\"Cerrar herramientas\">×</button></header><div class=\"gen-office-ai-intro\"><span>ESPACIO DE TRABAJO</span><h2>{s}</h2><p>{s}</p></div><div class=\"gen-office-ai-content\">{s}</div><footer><a href=\"/dashboard/ai\"><span class=\"material-symbols-outlined\">forum</span><span><strong>Abrir NOVA AI</strong><small>Conversa con el asistente académico</small></span><span class=\"material-symbols-outlined\">arrow_outward</span></a></footer></aside>", .{ try security.htmlEscape(allocator, title), try security.htmlEscape(allocator, description), content });
}

const PRESENTATION_EDITOR_JS = "var genOfficeDeckSlug=__SLUG__;function selectOfficeSlide(index){document.querySelectorAll('[data-slide-panel]').forEach(panel=>panel.hidden=panel.dataset.slidePanel!==String(index));document.querySelectorAll('[data-slide-target]').forEach(button=>button.setAttribute('aria-pressed',String(button.dataset.slideTarget===String(index))));const total=document.querySelectorAll('[data-slide-panel]').length,position=document.getElementById('slide-position');if(position)position.textContent=total?(index+1)+' / '+total:'0 / 0';}function buildOfficeOrder(order){const input=document.getElementById('slide-order');if(input)input.value=order.join(',');return order.join(',');}function moveSlide(index,delta){const nodes=[...document.querySelectorAll('#slides [data-idx]')],order=nodes.map(node=>Number(node.dataset.idx)),current=order.indexOf(index),target=current+delta;if(current<0||target<0||target>=order.length)return;[order[current],order[target]]=[order[target],order[current]];const value=buildOfficeOrder(order),body='order='+encodeURIComponent(value);fetch('/office/presentations/'+encodeURIComponent(genOfficeDeckSlug)+'/slides/reorder',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body}).then(response=>{if(!response.ok)throw Error('HTTP '+response.status);const location=response.headers.get('HX-Redirect');window.location.href=location||window.location.href;}).catch(()=>window.alert('No se pudo guardar el nuevo orden.'));}";
const PRESENTATION_JS = "var genPresentationTotal = __TOTAL__;var genPresentationCurrent=0;function updatePresentation(){const slides=[...document.querySelectorAll('[data-present-slide]')];slides.forEach((slide,index)=>slide.hidden=index!==genPresentationCurrent);const position=document.getElementById('presentation-position');if(position)position.textContent=slides.length?(genPresentationCurrent+1)+' / '+slides.length:'0 / 0';const previous=document.getElementById('previous-slide'),next=document.getElementById('next-slide');if(previous)previous.disabled=genPresentationCurrent<=0;if(next)next.disabled=genPresentationCurrent>=slides.length-1;}function changePresentationSlide(delta){genPresentationCurrent=Math.max(0,Math.min(genPresentationTotal-1,genPresentationCurrent+delta));updatePresentation();}function togglePresentationFullscreen(){const stage=document.getElementById('gen-office-present-stage');if(!stage)return;if(!document.fullscreenElement)stage.requestFullscreen?.();else document.exitFullscreen?.();}document.addEventListener('keydown',event=>{if(event.key==='ArrowRight'||event.key===' '){event.preventDefault();changePresentationSlide(1);}else if(event.key==='ArrowLeft'){event.preventDefault();changePresentationSlide(-1);}else if(event.key==='Escape'&&document.fullscreenElement){document.exitFullscreen?.();}});updatePresentation();}";
const MARKDOWN_TOOLBAR_JS = "function insertMd(prefix,suffix){const ta=document.getElementById('editor');if(!ta)return;const start=ta.selectionStart,end=ta.selectionEnd;const selected=ta.value.slice(start,end);ta.setRangeText(prefix+selected+suffix,start,end,'select');ta.focus();} function appendMd(text){const ta=document.getElementById('editor');if(!ta)return;ta.value+=text;ta.focus();}";

const OFFICE_WORKSPACE_JS = "function officeAiCompact(){return window.matchMedia('(max-width: 1100px)').matches;} function syncOfficeAiToggle(){const w=document.querySelector('.gen-office-workspace'),b=document.querySelector('[data-office-ai-toggle]');if(!w||!b)return;b.setAttribute('aria-expanded',String(officeAiCompact()?w.classList.contains('ai-open'):!w.classList.contains('ai-hidden')));} function openOfficeAi(){const w=document.querySelector('.gen-office-workspace');if(!w)return;w.classList.remove('ai-hidden');if(officeAiCompact())w.classList.add('ai-open');syncOfficeAiToggle();} function closeOfficeAi(){const w=document.querySelector('.gen-office-workspace');if(!w)return;if(officeAiCompact())w.classList.remove('ai-open');else w.classList.add('ai-hidden');syncOfficeAiToggle();} function toggleOfficeAi(){const w=document.querySelector('.gen-office-workspace');if(!w)return;(officeAiCompact()?w.classList.contains('ai-open'):!w.classList.contains('ai-hidden'))?closeOfficeAi():openOfficeAi();} document.addEventListener('keydown',e=>{if(e.key==='Escape')closeOfficeAi();});window.addEventListener('resize',syncOfficeAiToggle);syncOfficeAiToggle();";

fn documents(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8) !http.Response {
    const dir = try officeDir(allocator, root, "docs");
    if (std.mem.eql(u8, request.method, "GET")) return listPage(allocator, config, request.path, username, "Documentos", dir, ".md", "Crear documento", "/office/docs/new");
    if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
    const title = (try formField(allocator, request, "title")) orelse "";
    const content = (try formField(allocator, request, "content")) orelse "";
    if (std.mem.trim(u8, title, " \t\r\n").len == 0 or title.len > MAX_TITLE or content.len > MAX_CONTENT) return errorPage(allocator, config, request.path, username, 400, "Campos del documento no válidos.");
    const slug = try slugify(allocator, title);
    if (!safeSlug(slug)) return errorPage(allocator, config, request.path, username, 400, "Nombre no válido.");
    const file_path = try std.fmt.allocPrint(allocator, "{s}/{s}.md", .{ dir, slug });
    if (try fileExists(file_path)) return errorPage(allocator, config, request.path, username, 409, "Ya existe un documento con ese título.");
    writeNew(file_path, content) catch return errorPage(allocator, config, request.path, username, 500, "No se pudo guardar el documento.");
    return hxRedirect(allocator, try std.fmt.allocPrint(allocator, "/office/docs/{s}?saved=1", .{slug}));
}

fn documentRoute(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8) !http.Response {
    const rest = request.path["/office/docs/".len..];
    const dir = try officeDir(allocator, root, "docs");
    if (std.mem.eql(u8, rest, "new")) {
        if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
        const tabs = try officeTabs(allocator, "docs");
        const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-create-page\">{s}<main class=\"gen-office-create-shell\"><div class=\"gen-office-create-heading\"><span class=\"gen-office-launcher-icon\">▤</span><div><span>NUEVO DOCUMENTO</span><h1>Una página en blanco.</h1><p>Crea un archivo Markdown y edítalo desde el espacio de trabajo local.</p></div></div><form hx-post=\"/office/docs\" hx-swap=\"none\" class=\"gen-office-create-form\"><div class=\"gen-office-create-fields\"><label for=\"new-doc-title\">Título</label><input id=\"new-doc-title\" name=\"title\" maxlength=\"200\" required autofocus></div><div class=\"gen-office-create-editor\"><label for=\"new-doc-content\">Contenido Markdown</label><textarea id=\"new-doc-content\" name=\"content\" rows=\"18\"></textarea></div><footer><a class=\"gen-office-secondary-button\" href=\"/office/docs\">Cancelar</a><span>Se guardará como .md</span><button class=\"gen-office-primary-button\" type=\"submit\">Crear documento</button></footer></form></main></div>", .{tabs});
        return page(allocator, config, request.path, username, "Nuevo documento", body, 200);
    }
    const suffix = if (std.mem.endsWith(u8, rest, "/preview")) "/preview" else if (std.mem.endsWith(u8, rest, "/export")) "/export" else "";
    const slug = if (suffix.len > 0) rest[0 .. rest.len - suffix.len] else rest;
    if (!safeSlug(slug)) return errorPage(allocator, config, request.path, username, 400, "Nombre no válido.");
    const file_path = try std.fmt.allocPrint(allocator, "{s}/{s}.md", .{ dir, slug });

    if (suffix.len == 0 and std.mem.eql(u8, request.method, "DELETE")) {
        std.fs.cwd().deleteFile(file_path) catch |err| return errorPage(allocator, config, request.path, username, if (err == error.FileNotFound) 404 else 500, "No se pudo eliminar el documento.");
        return hxRedirect(allocator, "/office/docs");
    }
    if (suffix.len == 0 and std.mem.eql(u8, request.method, "PUT")) {
        if (!try fileExists(file_path)) return errorPage(allocator, config, request.path, username, 404, "Documento no encontrado.");
        const content = (try formField(allocator, request, "content")) orelse "";
        if (content.len > MAX_CONTENT) return errorPage(allocator, config, request.path, username, 400, "El documento supera el tamaño permitido.");
        try writeReplace(file_path, content);
        return hxRedirect(allocator, try std.fmt.allocPrint(allocator, "/office/docs/{s}?saved=1", .{slug}));
    }
    if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
    const content = readFile(allocator, file_path, MAX_CONTENT) catch |err| return errorPage(allocator, config, request.path, username, if (err == error.FileNotFound) 404 else 500, "Documento no encontrado.");

    if (std.mem.eql(u8, suffix, "/export")) {
        const fmt = (try http.queryParam(allocator, request.query, "fmt")) orelse "md";
        const body = if (std.mem.eql(u8, fmt, "html")) try markdownHtml(allocator, content) else content;
        const mime = if (std.mem.eql(u8, fmt, "html")) "text/html; charset=utf-8" else "text/markdown; charset=utf-8";
        const ext = if (std.mem.eql(u8, fmt, "html")) "html" else "md";
        return download(allocator, mime, body, try std.fmt.allocPrint(allocator, "{s}.{s}", .{ slug, ext }));
    }
    const tabs = try officeTabs(allocator, "docs");
    const escaped_slug = try security.htmlEscape(allocator, slug);
    if (std.mem.eql(u8, suffix, "/preview")) {
        const rendered = try markdownHtml(allocator, content);
        const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-preview-page\">{s}<main class=\"gen-office-preview-main\"><div class=\"gen-office-canvas-head\"><div><span>VISTA PREVIA · MARKDOWN</span><strong>{s}</strong></div><a href=\"/office/docs/{s}\">Volver al editor</a></div><article class=\"gen-office-document-preview\"><h1>{s}</h1>{s}</article></main></div>", .{ tabs, escaped_slug, slug, escaped_slug, rendered });
        return page(allocator, config, request.path, username, "Vista previa", body, 200);
    }
    const escaped_content = try security.htmlEscape(allocator, content);
    const tools = try officeAiPanel(allocator, "Escribe con claridad", "Usa formato Markdown. Las herramientas no modifican el archivo por ti.", "<div class=\"gen-office-formula-help\"><strong>Atajos Markdown</strong><code>**negrita**</code><code>*cursiva*</code><code># título</code><span>Guarda los cambios para conservarlos en el archivo local.</span></div>");
    const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-editor-page\">{s}<header class=\"gen-office-ribbon\"><nav class=\"gen-office-menubar\"><a class=\"is-active\" href=\"/office/docs/{s}\">Inicio</a><a href=\"/office/docs/{s}/preview\">Vista previa</a><a href=\"/office/docs/{s}/export?fmt=md\">Exportar Markdown</a><a href=\"/office/docs/{s}/export?fmt=html\">Exportar HTML</a></nav><div class=\"gen-office-ribbon-actions\">{s}<form hx-delete=\"/office/docs/{s}\" hx-confirm=\"¿Eliminar este documento?\" hx-swap=\"none\"><button class=\"gen-office-icon-button danger\" aria-label=\"Eliminar documento\">⌫</button></form></div></header><div class=\"gen-office-workspace\">{s}<main class=\"gen-office-editor-main\"><div class=\"gen-office-canvas-head\"><div><span>DOCUMENTO MARKDOWN</span><strong>{s}</strong></div><span class=\"gen-office-local-badge\">Guardado en este dispositivo</span></div><div class=\"gen-office-doc-workspace\"><nav class=\"gen-office-doc-toolbar\" aria-label=\"Formato Markdown\"><span class=\"gen-office-tool-label\">FORMATO</span><button type=\"button\" onclick=\"insertMd('**','**')\" aria-label=\"Negrita\"><strong>B</strong></button><button type=\"button\" onclick=\"insertMd('*','*')\" aria-label=\"Cursiva\"><em>I</em></button><button type=\"button\" onclick=\"insertMd('`','`')\" aria-label=\"Código\">&lt;/&gt;</button><span class=\"gen-office-tool-separator\"></span><button type=\"button\" onclick=\"appendMd('\\n## Título\\n')\" aria-label=\"Insertar título\">H</button><button type=\"button\" onclick=\"appendMd('\\n- Elemento de lista')\" aria-label=\"Insertar lista\">•</button><span class=\"gen-office-toolbar-format\">Markdown · .md</span></nav><form hx-put=\"/office/docs/{s}\" hx-swap=\"none\" class=\"gen-office-doc-form\"><div class=\"gen-office-paper\"><div class=\"gen-office-paper-meta\"><span>DOCUMENTO LOCAL</span><span>MARKDOWN</span></div><label class=\"sr-only\" for=\"editor\">Contenido Markdown</label><textarea id=\"editor\" name=\"content\" maxlength=\"500000\" spellcheck=\"false\">{s}</textarea></div><div class=\"gen-office-editor-save\"><span>Los cambios se guardan al pulsar Guardar.</span><button class=\"gen-office-primary-button\" type=\"submit\">Guardar</button></div></form></div></main></div><footer class=\"gen-office-statusbar\"><span>Documento local · {s}.md</span><span>Markdown</span><span>NOVA Office</span></footer></div><script>{s}{s}</script>", .{ tabs, slug, slug, slug, slug, officeAiToggle(), slug, tools, escaped_slug, slug, escaped_content, slug, MARKDOWN_TOOLBAR_JS, OFFICE_WORKSPACE_JS });
    return page(allocator, config, request.path, username, "Documentos", body, 200);
}

fn sheets(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8) !http.Response {
    const dir = try officeDir(allocator, root, "sheets");
    if (std.mem.eql(u8, request.method, "GET")) return listPage(allocator, config, request.path, username, "Hojas de cálculo", dir, ".csv", "Nueva hoja", "/office/sheets/new");
    if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
    const title = (try formField(allocator, request, "title")) orelse "";
    if (std.mem.trim(u8, title, " \t\r\n").len == 0 or title.len > MAX_TITLE) return errorPage(allocator, config, request.path, username, 400, "El título es obligatorio y debe tener menos de 200 caracteres.");
    const slug = try slugify(allocator, title);
    const file_path = try std.fmt.allocPrint(allocator, "{s}/{s}.csv", .{ dir, slug });
    if (try fileExists(file_path)) return errorPage(allocator, config, request.path, username, 409, "Ya existe una hoja con ese título.");
    writeNew(file_path, ",,,,\n,,,,\n,,,,\n,,,,\n,,,,") catch return errorPage(allocator, config, request.path, username, 500, "No se pudo crear la hoja.");
    return hxRedirect(allocator, try std.fmt.allocPrint(allocator, "/office/sheets/{s}", .{slug}));
}

fn sheetRoute(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8) !http.Response {
    const rest = request.path["/office/sheets/".len..];
    const dir = try officeDir(allocator, root, "sheets");
    if (std.mem.eql(u8, rest, "new")) {
        if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
        const tabs = try officeTabs(allocator, "sheets");
        const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-create-page\">{s}<main class=\"gen-office-create-shell\"><div class=\"gen-office-create-heading\"><span class=\"gen-office-launcher-icon sheets\">▦</span><div><span>NUEVA HOJA DE CÁLCULO</span><h1>Una cuadrícula lista para tus datos.</h1><p>Crearemos una cuadrícula editable de cinco filas por cinco columnas.</p></div></div><form hx-post=\"/office/sheets\" hx-swap=\"none\" class=\"gen-office-create-form compact\"><div class=\"gen-office-create-fields\"><label for=\"new-sheet-title\">Nombre de la hoja</label><input id=\"new-sheet-title\" name=\"title\" maxlength=\"200\" required autofocus></div><div class=\"gen-office-create-facts\"><span><strong>5 × 5</strong>cuadrícula inicial</span><span><strong>SUM / AVG</strong>fórmulas compatibles</span><span><strong>CSV</strong>importación y exportación</span></div><footer><a class=\"gen-office-secondary-button\" href=\"/office/sheets\">Cancelar</a><span>Se guardará como .csv</span><button class=\"gen-office-primary-button\" type=\"submit\">Crear hoja</button></footer></form></main></div>", .{tabs});
        return page(allocator, config, request.path, username, "Nueva hoja", body, 200);
    }
    const suffix = if (std.mem.endsWith(u8, rest, "/import")) "/import" else if (std.mem.endsWith(u8, rest, "/export")) "/export" else if (std.mem.endsWith(u8, rest, "/delete")) "/delete" else "";
    const slug = if (suffix.len > 0) rest[0 .. rest.len - suffix.len] else rest;
    if (!safeSlug(slug)) return errorPage(allocator, config, request.path, username, 400, "Nombre no válido.");
    const file_path = try std.fmt.allocPrint(allocator, "{s}/{s}.csv", .{ dir, slug });
    if ((suffix.len == 0 and !std.mem.eql(u8, request.method, "GET")) or
        (std.mem.eql(u8, suffix, "/export") and !std.mem.eql(u8, request.method, "GET")) or
        ((std.mem.eql(u8, suffix, "/delete") or std.mem.eql(u8, suffix, "/import")) and !std.mem.eql(u8, request.method, "POST"))) return methodNotAllowed(allocator);
    if (std.mem.eql(u8, suffix, "/delete")) {
        std.fs.cwd().deleteFile(file_path) catch |err| return errorPage(allocator, config, request.path, username, if (err == error.FileNotFound) 404 else 500, "No se pudo eliminar la hoja.");
        return hxRedirect(allocator, "/office/sheets");
    }
    if (std.mem.eql(u8, suffix, "/import")) {
        if (!try fileExists(file_path)) return errorPage(allocator, config, request.path, username, 404, "Hoja no encontrada.");
        const data = (try formField(allocator, request, "csv_data")) orelse "";
        if (data.len > MAX_CSV) return errorPage(allocator, config, request.path, username, 400, "CSV demasiado grande.");
        _ = parseCsv(allocator, data) catch return errorPage(allocator, config, request.path, username, 400, "CSV no válido o con demasiadas filas/columnas.");
        try writeReplace(file_path, data);
        return hxRedirect(allocator, try std.fmt.allocPrint(allocator, "/office/sheets/{s}", .{slug}));
    }
    if (!try fileExists(file_path)) return errorPage(allocator, config, request.path, username, 404, "Hoja no encontrada.");
    const raw = try readFile(allocator, file_path, MAX_CSV);
    const grid = parseCsv(allocator, raw) catch return errorPage(allocator, config, request.path, username, 400, "CSV no válido o con demasiadas filas/columnas.");
    const values = try computeGrid(allocator, grid);
    if (std.mem.eql(u8, suffix, "/export")) return download(allocator, "text/csv; charset=utf-8", try renderCsv(allocator, values), try std.fmt.allocPrint(allocator, "{s}.csv", .{slug}));
    const rendered = try renderGrid(allocator, slug, grid, values);
    const escaped_slug = try security.htmlEscape(allocator, slug);
    const tabs = try officeTabs(allocator, "sheets");
    const tools_content = try std.fmt.allocPrint(allocator, "<div class=\"gen-office-tool-list\"><button type=\"button\" onclick=\"addOfficeGridRow()\"><span class=\"material-symbols-outlined\">add_row_below</span><span><strong>Añadir una fila</strong><small>Amplía la cuadrícula editable</small></span></button><button type=\"button\" onclick=\"addOfficeGridColumn()\"><span class=\"material-symbols-outlined\">add_column_right</span><span><strong>Añadir una columna</strong><small>Agrega una columna al final</small></span></button></div><details class=\"gen-office-import-panel\"><summary><span class=\"material-symbols-outlined\">upload_file</span><span><strong>Importar CSV o Excel</strong><small>Selecciona una hoja; reemplaza los datos actuales</small></span></summary><label for=\"office-xlsx-file\">Archivo CSV o Excel (XLSX, XLS, XLSM, TSV)</label><input id=\"office-xlsx-file\" type=\"file\" accept=\".xlsx,.xls,.xlsm,.csv,.tsv\"><select id=\"office-xlsx-sheet\" hidden></select><button type=\"button\" id=\"office-xlsx-import\" disabled class=\"gen-office-secondary-button\">Importar hoja seleccionada</button><p id=\"office-xlsx-status\" role=\"status\" aria-live=\"polite\">Los archivos se leen localmente; sólo la hoja elegida se guarda como CSV.</p><form id=\"office-csv-import-form\" hx-post=\"/office/sheets/{s}/import\" hx-swap=\"none\"><label for=\"csv-import\">O pega contenido CSV</label><textarea id=\"csv-import\" name=\"csv_data\" rows=\"8\" placeholder=\"nombre,nota&#10;Examen,8.5\"></textarea><button type=\"submit\" class=\"gen-office-secondary-button\">Importar CSV</button></form></details><div class=\"gen-office-formula-help\"><strong>Fórmulas compatibles</strong><code>=SUM(A1:A5)</code><code>=AVG(B2:B8)</code><code>=AVERAGE(B2:B8)</code><span>Se calculan sobre datos originales al guardar y exportar.</span></div>", .{slug});
    const tools = try officeAiPanel(allocator, "Herramientas de la hoja", "Edita las celdas, añade filas o columnas e importa un CSV. Tus cambios solo se guardan al pulsar Guardar cuadrícula.", tools_content);
    const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-editor-page\">{s}<header class=\"gen-office-ribbon\"><nav class=\"gen-office-menubar\"><a class=\"is-active\" href=\"/office/sheets/{s}\">Inicio</a><button type=\"button\" onclick=\"addOfficeGridRow()\">Insertar fila</button><button type=\"button\" onclick=\"addOfficeGridColumn()\">Insertar columna</button><a href=\"/office/sheets/{s}/export\">Exportar CSV</a><button type=\"button\" id=\"office-xlsx-export\" data-filename=\"{s}\" class=\"gen-office-ribbon-button\">Excel XLSX</button></nav><div class=\"gen-office-ribbon-actions\">{s}<a class=\"gen-office-primary-button\" href=\"/office/sheets/{s}/export\">Descargar CSV</a><form hx-post=\"/office/sheets/{s}/delete\" hx-confirm=\"¿Eliminar esta hoja?\" hx-swap=\"none\"><button class=\"gen-office-icon-button danger\" aria-label=\"Eliminar hoja\">⌫</button></form></div></header><div class=\"gen-office-workspace\">{s}<main class=\"gen-office-editor-main\"><div class=\"gen-office-canvas-head\"><div><span>HOJA DE CÁLCULO · CSV</span><strong>{s}</strong></div><span class=\"gen-office-local-badge\">Archivo local</span></div><section class=\"gen-office-sheet-workspace\">{s}</section></main></div><footer class=\"gen-office-statusbar\"><span>CSV local · {s}.csv</span><span>{d} filas</span><span>NOVA Office</span></footer></div><script src=\"/vendor/xlsx-0.20.3.full.min.js\"></script><script src=\"/office-sheets.js\"></script><script>{s}</script>", .{ tabs, slug, slug, slug, officeAiToggle(), slug, slug, tools, escaped_slug, rendered, slug, grid.len, OFFICE_WORKSPACE_JS });
    return page(allocator, config, request.path, username, "Hojas de cálculo", body, 200);
}

fn presentations(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8) !http.Response {
    const dir = try officeDir(allocator, root, "presentations");
    if (std.mem.eql(u8, request.method, "GET")) return listPage(allocator, config, request.path, username, "Presentaciones", dir, ".json", "Nueva presentación", "/office/presentations/new");
    if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
    const title = (try formField(allocator, request, "title")) orelse "";
    if (std.mem.trim(u8, title, " \t\r\n").len == 0 or title.len > MAX_TITLE) return errorPage(allocator, config, request.path, username, 400, "El título es obligatorio.");
    const slug = try slugify(allocator, title);
    const file_path = try std.fmt.allocPrint(allocator, "{s}/{s}.json", .{ dir, slug });
    if (try fileExists(file_path)) return errorPage(allocator, config, request.path, username, 409, "Ya existe una presentación con ese título.");
    const initial = try json.object(allocator, &.{
        .{ .key = "title", .value = .{ .string = title } },
        .{ .key = "slides", .value = try json.array(allocator, &.{}) },
    });
    try writeNew(file_path, try json.stringify(allocator, initial));
    return hxRedirect(allocator, try std.fmt.allocPrint(allocator, "/office/presentations/{s}", .{slug}));
}

fn presentationRoute(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8) !http.Response {
    const rest = request.path["/office/presentations/".len..];
    const dir = try officeDir(allocator, root, "presentations");
    if (std.mem.eql(u8, rest, "new")) {
        if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
        const tabs = try officeTabs(allocator, "presentations");
        const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-create-page\">{s}<main class=\"gen-office-create-shell\"><div class=\"gen-office-create-heading\"><span class=\"gen-office-launcher-icon presentations\">▣</span><div><span>NUEVA PRESENTACIÓN</span><h1>Tu primera escena empieza aquí.</h1><p>Crea el mazo y añade después títulos, contenido Markdown y el orden de cada escena.</p></div></div><form hx-post=\"/office/presentations\" hx-swap=\"none\" class=\"gen-office-create-form compact\"><div class=\"gen-office-create-fields\"><label for=\"new-pres-title\">Título de la presentación</label><input id=\"new-pres-title\" name=\"title\" maxlength=\"200\" required autofocus></div><div class=\"gen-office-create-facts\"><span><strong>Escenas</strong>diapositivas editables</span><span><strong>Markdown</strong>contenido ligero</span><span><strong>JSON</strong>archivo local</span></div><footer><a class=\"gen-office-secondary-button\" href=\"/office/presentations\">Cancelar</a><span>Se guardará como .json</span><button class=\"gen-office-primary-button\" type=\"submit\">Crear presentación</button></footer></form></main></div>", .{tabs});
        return page(allocator, config, request.path, username, "Nueva presentación", body, 200);
    }
    const slides_marker = std.mem.indexOf(u8, rest, "/slides");
    const slug = if (slides_marker) |index| rest[0..index] else if (std.mem.endsWith(u8, rest, "/present")) rest[0 .. rest.len - "/present".len] else rest;
    if (!safeSlug(slug)) return errorPage(allocator, config, request.path, username, 400, "Nombre no válido.");
    const file_path = try std.fmt.allocPrint(allocator, "{s}/{s}.json", .{ dir, slug });
    if (!try fileExists(file_path)) return errorPage(allocator, config, request.path, username, 404, "Presentación no encontrada.");
    var presentation = loadPresentation(allocator, file_path) catch return errorPage(allocator, config, request.path, username, 400, "Presentación no válida.");

    if (slides_marker) |index| {
        const suffix = rest[index + "/slides".len ..];
        if (suffix.len == 0) {
            if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
            const title = (try formField(allocator, request, "title")) orelse "";
            const content = (try formField(allocator, request, "content")) orelse "";
            if (std.mem.trim(u8, title, " \t\r\n").len == 0 or title.len > MAX_TITLE or content.len > MAX_CONTENT) return errorPage(allocator, config, request.path, username, 400, "La diapositiva no es válida.");
            var slides = getSlidesPtr(&presentation) orelse return errorPage(allocator, config, request.path, username, 400, "Presentación no válida.");
            if (slides.items.len >= 200) return errorPage(allocator, config, request.path, username, 400, "La presentación alcanzó 200 diapositivas.");
            try slides.append(try slideValue(allocator, title, content));
            try savePresentation(file_path, allocator, presentation);
            return hxRedirect(allocator, try std.fmt.allocPrint(allocator, "/office/presentations/{s}", .{slug}));
        }
        if (std.mem.eql(u8, suffix, "/reorder")) {
            if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
            const order = (try formField(allocator, request, "order")) orelse "";
            if (order.len > 10_000 or !try reorderSlides(allocator, &presentation, order)) return errorPage(allocator, config, request.path, username, 400, "El orden debe ser una permutación válida de las diapositivas.");
            try savePresentation(file_path, allocator, presentation);
            return hxRedirect(allocator, try std.fmt.allocPrint(allocator, "/office/presentations/{s}", .{slug}));
        }
        const raw_index = if (std.mem.startsWith(u8, suffix, "/")) suffix[1..] else "";
        const slide_index = std.fmt.parseInt(usize, raw_index, 10) catch return errorPage(allocator, config, request.path, username, 400, "Índice de diapositiva no válido.");
        const slide_ptr = getSlidePtr(&presentation, slide_index) orelse return errorPage(allocator, config, request.path, username, 404, "Diapositiva no encontrada.");
        if (std.mem.eql(u8, request.method, "DELETE")) {
            const slides = getSlidesPtr(&presentation).?;
            _ = slides.orderedRemove(slide_index);
            try savePresentation(file_path, allocator, presentation);
            return hxRedirect(allocator, try std.fmt.allocPrint(allocator, "/office/presentations/{s}", .{slug}));
        }
        if (std.mem.eql(u8, request.method, "PUT")) {
            const title = (try formField(allocator, request, "title")) orelse json.getString(slide_ptr.*, "title") orelse "";
            const content = (try formField(allocator, request, "content")) orelse json.getString(slide_ptr.*, "content") orelse "";
            if (std.mem.trim(u8, title, " \t\r\n").len == 0 or title.len > MAX_TITLE or content.len > MAX_CONTENT) return errorPage(allocator, config, request.path, username, 400, "La diapositiva no es válida.");
            try slide_ptr.object.put("title", .{ .string = title });
            try slide_ptr.object.put("content", .{ .string = content });
            try savePresentation(file_path, allocator, presentation);
            return hxRedirect(allocator, try std.fmt.allocPrint(allocator, "/office/presentations/{s}", .{slug}));
        }
        return methodNotAllowed(allocator);
    }

    if (std.mem.endsWith(u8, rest, "/present")) {
        if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
        return presentPage(allocator, config, request.path, username, presentation);
    }
    if (std.mem.eql(u8, request.method, "DELETE")) {
        std.fs.cwd().deleteFile(file_path) catch |err| return errorPage(allocator, config, request.path, username, if (err == error.FileNotFound) 404 else 500, "No se pudo eliminar la presentación.");
        return hxRedirect(allocator, "/office/presentations");
    }
    if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
    return editPresentation(allocator, config, request.path, username, slug, presentation);
}

fn htmlPages(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8) !http.Response {
    const dir = try officeDir(allocator, root, "html");
    if (std.mem.eql(u8, request.method, "GET")) return listPage(allocator, config, request.path, username, "Páginas HTML", dir, ".html", "Nueva página", "/office/html/new");
    if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
    const title_raw = (try formField(allocator, request, "title")) orelse "";
    const title = std.mem.trim(u8, title_raw, " \t\r\n");
    const content_raw = (try formField(allocator, request, "content")) orelse "";
    if (title.len == 0 or title.len > MAX_TITLE or content_raw.len > MAX_HTML_CONTENT) return errorPage(allocator, config, request.path, username, 400, "El nombre es obligatorio y el contenido no debe superar 1 MiB.");
    const slug = try slugify(allocator, title);
    const file_path = try std.fmt.allocPrint(allocator, "{s}/{s}.html", .{ dir, slug });
    if (try fileExists(file_path)) return errorPage(allocator, config, request.path, username, 409, "Ya existe una página con ese nombre.");
    const content = if (std.mem.trim(u8, content_raw, " \t\r\n").len == 0) try starterHtml(allocator, title) else content_raw;
    writeNew(file_path, content) catch return errorPage(allocator, config, request.path, username, 500, "No se pudo guardar la página HTML.");
    return hxRedirect(allocator, try std.fmt.allocPrint(allocator, "/office/html/{s}", .{slug}));
}

fn htmlPageRoute(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8) !http.Response {
    const rest = request.path["/office/html/".len..];
    if (std.mem.eql(u8, rest, "new")) {
        if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
        const tabs = try officeTabs(allocator, "html");
        const nav = try officeNav(allocator, "html");
        const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-create-page\">{s}<main class=\"gen-office-create-shell\"><div class=\"gen-office-create-heading\"><span class=\"gen-office-launcher-icon html\">&lt;/&gt;</span><div><span>NUEVA PÁGINA HTML</span><h1>Tu próximo diseño web.</h1><p>Escribe HTML y CSS y comprueba el resultado antes de guardar.</p></div></div>{s}<form hx-post=\"/office/html\" hx-swap=\"none\" class=\"gen-office-create-form\"><div class=\"gen-office-create-fields\"><label for=\"new-html-title\">Nombre del archivo</label><input id=\"new-html-title\" name=\"title\" maxlength=\"200\" required autofocus></div><div class=\"gen-office-create-editor\"><label for=\"new-html-content\">Código inicial</label><textarea id=\"new-html-content\" name=\"content\" rows=\"18\" spellcheck=\"false\" placeholder=\"&lt;!doctype html&gt;&#10;&lt;html lang=&quot;es&quot;&gt;&#10;  &lt;body&gt;&lt;h1&gt;Hola, mundo&lt;/h1&gt;&lt;/body&gt;&#10;&lt;/html&gt;\"></textarea></div><footer><a href=\"/office/html\" class=\"gen-office-secondary-button\">Cancelar</a><span>Se guardará como .html · máximo 1 MiB</span><button type=\"submit\" class=\"gen-office-primary-button\">Crear página</button></footer></form></main></div>", .{ tabs, nav });
        return page(allocator, config, request.path, username, "Nueva página HTML", body, 200);
    }
    const suffix = if (std.mem.endsWith(u8, rest, "/export")) "/export" else "";
    const slug = if (suffix.len > 0) rest[0 .. rest.len - suffix.len] else rest;
    if (!safeSlug(slug)) return errorPage(allocator, config, request.path, username, 400, "Nombre no válido.");
    const dir = try officeDir(allocator, root, "html");
    const file_path = try std.fmt.allocPrint(allocator, "{s}/{s}.html", .{ dir, slug });
    if (std.mem.eql(u8, suffix, "/export")) {
        if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
        const bytes = readFile(allocator, file_path, MAX_HTML_CONTENT) catch |err| return errorPage(allocator, config, request.path, username, if (err == error.FileNotFound) 404 else 500, "Página HTML no encontrada.");
        return download(allocator, "text/html; charset=utf-8", bytes, try std.fmt.allocPrint(allocator, "{s}.html", .{slug}));
    }
    if (std.mem.eql(u8, request.method, "DELETE")) {
        std.fs.cwd().deleteFile(file_path) catch |err| return errorPage(allocator, config, request.path, username, if (err == error.FileNotFound) 404 else 500, "No se pudo eliminar la página.");
        return hxRedirect(allocator, "/office/html");
    }
    if (std.mem.eql(u8, request.method, "PUT")) {
        if (!try fileExists(file_path)) return errorPage(allocator, config, request.path, username, 404, "Página HTML no encontrada.");
        const content = (try formField(allocator, request, "content")) orelse "";
        if (content.len > MAX_HTML_CONTENT) return errorPage(allocator, config, request.path, username, 400, "El contenido supera 1 MiB.");
        try writeReplace(file_path, content);
        return hxRedirect(allocator, try std.fmt.allocPrint(allocator, "/office/html/{s}", .{slug}));
    }
    if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
    const content = readFile(allocator, file_path, MAX_HTML_CONTENT) catch |err| return errorPage(allocator, config, request.path, username, if (err == error.FileNotFound) 404 else 500, "Página HTML no encontrada.");
    const tabs = try officeTabs(allocator, "html");
    const nav = try officeNav(allocator, "html");
    const escaped_slug = try security.htmlEscape(allocator, slug);
    const display_title = try std.mem.replaceOwned(u8, allocator, slug, "-", " ");
    const escaped_title = try security.htmlEscape(allocator, display_title);
    const escaped_content = try security.htmlEscape(allocator, content);
    const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-editor-page gen-office-html-editor\">{s}<header class=\"gen-office-ribbon\"><nav class=\"gen-office-menubar\" aria-label=\"Acciones de la página\"><a class=\"is-active\" href=\"/office/html/{s}\">Código y vista previa</a><a href=\"/office/html\">Mis páginas</a><a href=\"/office/html/{s}/export\">Descargar .html</a></nav><div class=\"gen-office-ribbon-actions\"><button type=\"button\" id=\"office-html-refresh\" class=\"gen-office-ribbon-button\"><span class=\"material-symbols-outlined\">refresh</span><span>Actualizar vista previa</span></button></div></header>{s}<div class=\"gen-office-workspace\"><main class=\"gen-office-editor-main\"><div class=\"gen-office-canvas-head\"><div><span>PÁGINA HTML · CÓDIGO LOCAL</span><strong>{s}</strong></div><span class=\"gen-office-local-badge\"><span class=\"material-symbols-outlined\">security</span>Vista previa aislada</span></div><form id=\"office-html-save-form\" hx-put=\"/office/html/{s}\" hx-swap=\"none\" class=\"gen-office-html-save\"><div class=\"gen-office-html-panes\"><section class=\"gen-office-html-source\"><label for=\"editor\"><span>HTML</span><span>UTF-8 · máx. 1 MiB</span></label><textarea id=\"editor\" name=\"content\" maxlength=\"1048576\" data-office-format=\"HTML\" spellcheck=\"false\" autocapitalize=\"off\" autocomplete=\"off\">{s}</textarea></section><section class=\"gen-office-html-preview\"><header><span><i></i>Vista previa local</span><small>Scripts y red bloqueados</small></header><iframe id=\"office-html-preview-frame\" title=\"Vista previa de la página HTML\" sandbox=\"\" referrerpolicy=\"no-referrer\"></iframe></section></div><footer class=\"gen-office-editor-save\"><span id=\"office-html-status\" role=\"status\" aria-live=\"polite\">Vista previa aislada; no puede acceder a NOVA.</span><button type=\"submit\" class=\"gen-office-primary-button\">Guardar HTML</button></footer></form></main></div><footer class=\"gen-office-statusbar\"><span>.html · UTF-8</span><span id=\"office-html-byte-count\">0 bytes</span><span>NOVA Office</span></footer></div><script src=\"/office-html.js\"></script><script>{s}</script>", .{ tabs, escaped_slug, escaped_slug, nav, escaped_title, escaped_slug, escaped_content, OFFICE_WORKSPACE_JS });
    return page(allocator, config, request.path, username, "Editor HTML", body, 200);
}

fn starterHtml(allocator: std.mem.Allocator, title: []const u8) ![]const u8 {
    const safe_title = try security.htmlEscape(allocator, title);
    return std.fmt.allocPrint(allocator, "<!doctype html>\n<html lang=\"es\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{s}</title>\n</head>\n<body>\n<main><h1>{s}</h1><p>Empieza a editar esta página en NOVA Office.</p></main>\n</body>\n</html>\n", .{ safe_title, safe_title });
}

fn library(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8) !http.Response {
    if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
    return libraryPage(allocator, config, request, username, root, "");
}

fn libraryRoute(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8) !http.Response {
    const rest = request.path["/office/library/".len..];
    if (std.mem.startsWith(u8, rest, "import/")) {
        if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
        const kind = rest["import/".len..];
        const ext = extensionFor(kind) orelse return errorPage(allocator, config, request.path, username, 404, "Tipo de archivo no admitido.");
        const name = (try formField(allocator, request, "name")) orelse "";
        const content = (try formField(allocator, request, "content")) orelse "";
        if (std.mem.trim(u8, name, " \t\r\n").len == 0 or name.len > MAX_TITLE or content.len > MAX_CONTENT) return errorPage(allocator, config, request.path, username, 400, "Nombre o contenido no válido.");
        const slug = try slugify(allocator, name);
        if (!safeSlug(slug)) return errorPage(allocator, config, request.path, username, 400, "Nombre no válido.");
        const dir = try officeDir(allocator, root, kind);
        const full = try std.fmt.allocPrint(allocator, "{s}/{s}{s}", .{ dir, slug, ext });
        if (try fileExists(full)) return errorPage(allocator, config, request.path, username, 409, "El archivo ya existe.");
        if (std.mem.eql(u8, kind, "presentations")) {
            const presentation = importPresentation(allocator, name, content) catch return errorPage(allocator, config, request.path, username, 400, "La presentación importada no tiene un formato válido.");
            try writeNew(full, try json.stringify(allocator, presentation));
        } else {
            try writeNew(full, content);
        }
        return hxRedirect(allocator, "/office/library");
    }

    const segments = try splitPath(allocator, rest);
    if (segments.len == 1 and std.mem.eql(u8, segments[0], "search")) {
        if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
        const query = (try http.queryParam(allocator, request.query, "q")) orelse "";
        if (query.len > 200) return errorPage(allocator, config, request.path, username, 400, "La búsqueda supera los 200 caracteres.");
        return libraryPage(allocator, config, request, username, root, query);
    }
    if (segments.len != 3) return errorPage(allocator, config, request.path, username, 404, "Archivo no encontrado.");
    const kind = segments[0];
    const slug = segments[1];
    const action = segments[2];
    const ext = extensionFor(kind) orelse return errorPage(allocator, config, request.path, username, 404, "Tipo de archivo no admitido.");
    if (!safeSlug(slug)) return errorPage(allocator, config, request.path, username, 400, "Nombre no válido.");
    if (!std.mem.eql(u8, action, "download") and !std.mem.eql(u8, action, "delete")) return errorPage(allocator, config, request.path, username, 404, "Acción no encontrada.");
    const dir = try officeDir(allocator, root, kind);
    const full = try std.fmt.allocPrint(allocator, "{s}/{s}{s}", .{ dir, slug, ext });
    if (std.mem.eql(u8, action, "delete")) {
        if (!std.mem.eql(u8, request.method, "POST")) return methodNotAllowed(allocator);
        std.fs.cwd().deleteFile(full) catch |err| return errorPage(allocator, config, request.path, username, if (err == error.FileNotFound) 404 else 500, "No se pudo eliminar el archivo.");
        return hxRedirect(allocator, "/office/library");
    }
    if (!std.mem.eql(u8, request.method, "GET")) return methodNotAllowed(allocator);
    const content = readFile(allocator, full, MAX_LIBRARY_FILE) catch |err| return errorPage(allocator, config, request.path, username, if (err == error.FileNotFound) 404 else 500, "Archivo no encontrado.");
    const mime = if (std.mem.eql(u8, kind, "docs")) "text/markdown; charset=utf-8" else if (std.mem.eql(u8, kind, "sheets")) "text/csv; charset=utf-8" else if (std.mem.eql(u8, kind, "html")) "text/html; charset=utf-8" else "application/json; charset=utf-8";
    return download(allocator, mime, content, try std.fmt.allocPrint(allocator, "{s}{s}", .{ slug, ext }));
}

fn libraryPage(allocator: std.mem.Allocator, config: *const config_mod.Config, request: *const http.Request, username: []const u8, root: []const u8, query: []const u8) !http.Response {
    var rows = std.ArrayList(u8).init(allocator);
    var count: usize = 0;
    for ([_]struct { kind: []const u8, ext: []const u8, label: []const u8, icon: []const u8 }{
        .{ .kind = "docs", .ext = ".md", .label = "Documento", .icon = "description" },
        .{ .kind = "sheets", .ext = ".csv", .label = "Hoja", .icon = "table_chart" },
        .{ .kind = "presentations", .ext = ".json", .label = "Presentación", .icon = "present_to_all" },
        .{ .kind = "html", .ext = ".html", .label = "Página HTML", .icon = "code" },
    }) |item| {
        const dir = try officeDir(allocator, root, item.kind);
        var opened = std.fs.cwd().openDir(dir, .{ .iterate = true }) catch continue;
        defer opened.close();
        var iter = opened.iterate();
        while (try iter.next()) |entry| {
            if (entry.kind != .file or !std.mem.endsWith(u8, entry.name, item.ext)) continue;
            const slug = entry.name[0 .. entry.name.len - item.ext.len];
            if (!safeSlug(slug) or (query.len > 0 and std.ascii.indexOfIgnoreCase(slug, query) == null)) continue;
            const escaped = try security.htmlEscape(allocator, slug);
            const open_url = try std.fmt.allocPrint(allocator, "/office/{s}/{s}", .{ item.kind, escaped });
            const download_url = try std.fmt.allocPrint(allocator, "/office/library/{s}/{s}/download", .{ item.kind, escaped });
            const delete_url = try std.fmt.allocPrint(allocator, "/office/library/{s}/{s}/delete", .{ item.kind, escaped });
            const delete_form = try std.fmt.allocPrint(allocator, "<form hx-post=\"{s}\" hx-confirm=\"¿Borrar este archivo?\" hx-swap=\"none\"><button type=\"submit\" class=\"gen-office-icon-button danger\" aria-label=\"Borrar {s}\">⌫</button></form>", .{ delete_url, escaped });
            try rows.writer().print("<tr><td data-label=\"Archivo\"><a class=\"gen-office-library-file {s}\" href=\"{s}\"><span class=\"material-symbols-outlined\">{s}</span><span>{s}</span></a></td><td data-label=\"Tipo\"><span class=\"gen-office-file-kind\">{s}</span></td><td data-label=\"Tamaño\">{s}</td><td><div class=\"gen-office-library-actions\"><a class=\"gen-office-icon-button\" href=\"{s}\" aria-label=\"Abrir {s}\">↗</a><a class=\"gen-office-icon-button\" href=\"{s}\" aria-label=\"Descargar {s}\">↓</a>{s}</div></td></tr>", .{ item.kind, open_url, item.icon, escaped, item.label, humanSizeFromPath(allocator, root, item.kind, entry.name) catch "—", open_url, escaped, download_url, escaped, delete_form });
            count += 1;
        }
    }
    const table_rows = if (count == 0) "<tr><td colspan=\"4\"><div class=\"gen-office-empty compact\"><span class=\"material-symbols-outlined\">search_off</span><p>No se encontraron archivos.</p></div></td></tr>" else rows.items;
    const escaped_query = try security.htmlEscape(allocator, query);
    const tabs = try officeTabs(allocator, "library");
    const nav = try officeNav(allocator, "library");
    const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-manager-page gen-office-library-page\">{s}<header class=\"gen-office-manager-head\"><div><span>BIBLIOTECA · ARCHIVOS LOCALES</span><h1>Todo tu trabajo.</h1><p>{d} archivo(s) visibles. Abre, descarga o importa desde un solo lugar.</p></div><a href=\"#library-import\" class=\"gen-office-primary-button\">Importar archivo</a></header>{s}<form method=\"get\" action=\"/office/library/search\" class=\"gen-office-library-search\"><span class=\"material-symbols-outlined\">search</span><label class=\"sr-only\" for=\"library-query\">Buscar archivos</label><input id=\"library-query\" type=\"search\" name=\"q\" value=\"{s}\" placeholder=\"Buscar por nombre…\"><button type=\"submit\" class=\"gen-office-secondary-button\">Buscar</button></form><div class=\"gen-office-library-layout\"><section class=\"gen-office-library-browser\"><header><div><span>ARCHIVOS</span><strong>{d} resultado(s)</strong></div><span class=\"material-symbols-outlined\">folder_open</span></header><div class=\"gen-office-table-scroll\"><table class=\"gen-office-library-table\"><thead><tr><th>Archivo</th><th>Tipo</th><th>Tamaño</th><th aria-label=\"Acciones\"></th></tr></thead><tbody>{s}</tbody></table></div></section><aside class=\"gen-office-library-import\" id=\"library-import\"><header><span class=\"gen-office-launcher-icon library\">↑</span><div><span>IMPORTAR</span><h2>Nuevo archivo</h2></div></header><p>Crea un archivo directamente en tu biblioteca local.</p><form id=\"library-import-form\" hx-post=\"/office/library/import/docs\" hx-swap=\"none\"><label for=\"library-import-name\">Nombre</label><input id=\"library-import-name\" type=\"text\" name=\"name\" maxlength=\"200\" placeholder=\"mi-archivo\" required><label for=\"library-import-kind\">Tipo de archivo</label><select id=\"library-import-kind\" name=\"kind\" onchange=\"this.form.setAttribute('hx-post','/office/library/import/'+this.value);htmx.process(this.form)\"><option value=\"docs\">Documento Markdown</option><option value=\"sheets\">Hoja CSV</option><option value=\"presentations\">Presentación</option><option value=\"html\">Página HTML</option></select><label for=\"library-import-content\">Contenido</label><textarea id=\"library-import-content\" name=\"content\" rows=\"8\" required></textarea><p class=\"gen-office-import-help\">Para presentaciones, pega texto Markdown para crear una escena o JSON válido.</p><button type=\"submit\" class=\"gen-office-primary-button\">Importar archivo</button></form></aside></div><footer class=\"gen-office-home-footer\"><span>Archivos locales · Markdown · CSV · JSON</span><span>NOVA Office</span></footer></div>", .{ tabs, count, nav, escaped_query, count, table_rows });
    return page(allocator, config, request.path, username, "Biblioteca", body, 200);
}

fn humanSizeFromPath(allocator: std.mem.Allocator, root: []const u8, kind: []const u8, name: []const u8) ![]const u8 {
    const path = try std.fs.path.join(allocator, &.{ root, kind, name });
    const stat = try std.fs.cwd().statFile(path);
    if (stat.size >= 1024 * 1024) return std.fmt.allocPrint(allocator, "{d:.1} MB", .{@as(f64, @floatFromInt(stat.size)) / (1024.0 * 1024.0)});
    if (stat.size >= 1024) return std.fmt.allocPrint(allocator, "{d:.1} KB", .{@as(f64, @floatFromInt(stat.size)) / 1024.0});
    return std.fmt.allocPrint(allocator, "{d} B", .{stat.size});
}

fn report(allocator: std.mem.Allocator, db: *sqlite.Db, config: *const config_mod.Config) !http.Response {
    const root = try officeRoot(allocator, config);
    const genoffice_url = std.mem.trim(u8, config.genoffice_mcp_url, " \t\r\n");
    if (genoffice_url.len == 0) return jsonErrorWithReason(allocator, 503, "GenOffice no disponible", "GENOFFICE_MCP_URL no está configurada.");

    // Probe first so an unavailable document service does not hold up data collection.
    const genoffice_up = mcp_client.probe(allocator, genoffice_url, @min(config.genoffice_timeout_ms, 2500), true) catch false;
    const export_timeout = if (genoffice_up) config.genoffice_timeout_ms else @min(config.genoffice_timeout_ms, 5000);

    var markdown = std.ArrayList(u8).init(allocator);
    const generated = try timestampIsoUtc(allocator);
    try markdown.writer().print("# Informe del estudiante\n\nGenerado el {s} por Study Platform. Cada sección indica su fuente; las secciones sin datos disponibles se marcan como tales.\n\n", .{generated});

    const mikareno_timeout = @min(config.mcp_timeout_ms, 10_000);
    const profile = mcp_client.callTool(allocator, config.mikareno_mcp_url, mikareno_timeout, "get_perfil", try json.object(allocator, &.{}), true) catch null;
    try appendIdentitySection(allocator, &markdown, profile);
    try appendNotesSection(allocator, &markdown, db);

    const academic_args = try json.object(allocator, &.{.{ .key = "bg", .value = .{ .bool = true } }});
    const academic = mcp_client.callTool(allocator, config.mikareno_mcp_url, mikareno_timeout, "get_student_full", academic_args, true) catch null;
    try appendGradeSection(allocator, &markdown, "Promedios por materia", academic, true);
    try appendGradeSection(allocator, &markdown, "Notas por materia", academic, false);
    const tasks = mcp_client.callTool(allocator, config.mikareno_mcp_url, mikareno_timeout, "get_tareas", try json.object(allocator, &.{}), true) catch null;
    try appendTasksSection(allocator, &markdown, tasks);

    const configured_work_dir = std.mem.trim(u8, config.genoffice_work_dir, " \t\r\n");
    const output_dir = if (configured_work_dir.len > 0)
        configured_work_dir
    else
        try std.fs.path.join(allocator, &.{ root, "tmp" });
    try std.fs.cwd().makePath(output_dir);
    const absolute_dir = try std.fs.cwd().realpathAlloc(allocator, output_dir);
    const stamp = try timestampForFile(allocator);
    const output_name = try std.fmt.allocPrint(allocator, "informe-estudiante-{s}.docx", .{stamp});
    const output = try std.fs.path.join(allocator, &.{ absolute_dir, output_name });
    const args = try json.object(allocator, &.{
        .{ .key = "markdown", .value = .{ .string = markdown.items } },
        .{ .key = "out", .value = .{ .string = output } },
        .{ .key = "force", .value = .{ .bool = true } },
    });
    _ = mcp_client.callTool(allocator, genoffice_url, export_timeout, "create_docx", args, true) catch {
        std.fs.cwd().deleteFile(output) catch {};
        return jsonErrorWithReason(allocator, 503, "GenOffice no disponible", if (genoffice_up) "No se pudo invocar create_docx en el servidor MCP configurado." else "El MCP de GenOffice no respondió al sondeo ni pudo generar el documento.");
    };
    const file = std.fs.cwd().openFile(output, .{}) catch return jsonErrorWithReason(allocator, 503, "GenOffice no disponible", "La herramienta respondió, pero el archivo DOCX no apareció en el volumen compartido.");
    defer {
        file.close();
        std.fs.cwd().deleteFile(output) catch {};
    }
    const bytes = file.readToEndAlloc(allocator, 64 * 1024 * 1024) catch return jsonErrorWithReason(allocator, 500, "No se pudo leer el informe", "El archivo DOCX supera el límite o no se puede leer.");
    if (bytes.len < 4 or !std.mem.eql(u8, bytes[0..4], "PK\x03\x04")) return jsonErrorWithReason(allocator, 502, "Respuesta DOCX no válida", "GenOffice no generó un archivo ZIP/DOCX reconocible.");
    const headers = try allocator.alloc(http.Header, 1);
    headers[0] = .{ .name = "Content-Disposition", .value = "attachment; filename=\"informe-estudiante.docx\"" };
    return .{ .status = 200, .content_type = DOCX_MIME, .body = bytes, .headers = headers };
}

fn appendIdentitySection(allocator: std.mem.Allocator, markdown: *std.ArrayList(u8), profile: ?std.json.Value) !void {
    try markdown.appendSlice("## Identificación\n\n| Dato | Valor |\n| --- | --- |\n");
    var name: []const u8 = "";
    var institution: []const u8 = "";
    var course: []const u8 = "";
    if (profile) |data| {
        const student = json.get(data, "estudiante") orelse json.get(data, "perfil") orelse data;
        name = firstReportString(student, &.{ "nombre", "name", "fullName" });
        if (name.len == 0) name = firstReportString(data, &.{ "nombre", "name", "fullName" });

        const institution_value = json.get(data, "institucion") orelse .null;
        institution = firstReportString(institution_value, &.{ "name", "nombre", "officialName", "displayName", "shortName" });
        if (institution.len == 0 and institution_value == .string) institution = institution_value.string;
        if (institution.len == 0 and institution_value != .null) institution = try json.stringify(allocator, institution_value);

        course = try reportCourse(allocator, data);
    }
    try markdown.writer().print("| Estudiante | {s} |\n| Institución | {s} |\n| Curso | {s} |\n\n", .{
        try markdownTableEscape(allocator, if (name.len == 0) "no disponible" else name),
        try markdownTableEscape(allocator, if (institution.len == 0) "no disponible" else institution),
        try markdownTableEscape(allocator, if (course.len == 0) "no disponible" else course),
    });
}

fn reportCourse(allocator: std.mem.Allocator, profile: std.json.Value) ![]const u8 {
    const raw = json.get(profile, "curso_actual") orelse .null;
    if (raw == .string) return raw.string;
    const code = firstReportString(raw, &.{ "codigo", "code" });
    const course_name = firstReportString(raw, &.{ "nombre", "name" });
    if (code.len == 0 and course_name.len == 0) {
        const fallback = firstReportString(profile, &.{ "curso", "course" });
        return if (fallback.len == 0) "Curso desconocido" else fallback;
    }
    var result = std.ArrayList(u8).init(allocator);
    if (code.len > 0) try result.appendSlice(code);
    if (course_name.len > 0) {
        if (result.items.len > 0) try result.appendSlice(" — ");
        try result.appendSlice(course_name);
    }
    const parallel = firstReportString(raw, &.{"paralelo"});
    const section = firstReportString(raw, &.{"seccion"});
    const tutor = firstReportString(raw, &.{"tutor"});
    const grade = firstReportString(raw, &.{"grado"});
    if (parallel.len > 0) try result.writer().print(" ({s})", .{parallel});
    if (section.len > 0) try result.writer().print("/{s}", .{section});
    if (tutor.len > 0) try result.writer().print(", tutor: {s}", .{tutor});
    if (grade.len > 0) try result.writer().print(", grado: {s}", .{grade});
    return result.toOwnedSlice();
}

fn appendNotesSection(allocator: std.mem.Allocator, markdown: *std.ArrayList(u8), db: *sqlite.Db) !void {
    const rows = try collectReportNoteRows(allocator, db);
    try markdown.appendSlice("## Apuntes\n\n");
    if (rows) |table_rows| {
        if (table_rows.len == 0) {
            try markdown.appendSlice("_No hay apuntes registrados._\n\n");
        } else {
            try markdown.appendSlice("| Título | Etiquetas | Palabras | Actualizado |\n| --- | --- | ---: | --- |\n");
            try markdown.appendSlice(table_rows);
            try markdown.appendSlice("\n");
        }
        try markdown.appendSlice("Fuente: Base de datos de Study Platform (SQLite).\n\n");
    } else {
        try markdown.appendSlice("_Apuntes no disponibles: no se pudo leer la base de datos._\n\n");
    }
}

fn collectReportNoteRows(allocator: std.mem.Allocator, db: *sqlite.Db) !?[]const u8 {
    const prepared = db.prepare("SELECT title,tags,word_count,updated_at FROM notes ORDER BY updated_at DESC LIMIT 50") catch return null;
    var stmt = prepared;
    defer stmt.deinit();
    var rows = std.ArrayList(u8).init(allocator);
    while (true) {
        const has_row = stmt.next() catch return null;
        if (!has_row) break;
        try rows.writer().print("| {s} | {s} | {d} | {s} |\n", .{
            try markdownTableEscape(allocator, stmt.text(0)),
            try markdownTableEscape(allocator, stmt.text(1)),
            stmt.int64(2),
            try markdownTableEscape(allocator, stmt.text(3)),
        });
    }
    return try rows.toOwnedSlice();
}

fn appendGradeSection(allocator: std.mem.Allocator, markdown: *std.ArrayList(u8), title: []const u8, academic: ?std.json.Value, include_code: bool) !void {
    try markdown.writer().print("## {s}\n\n", .{title});
    if (academic) |data| {
        const rows = try collectReportGradeRows(allocator, data, include_code);
        if (rows.len == 0) {
            try markdown.appendSlice(if (include_code) "_La fuente no devolvió promedios._\n" else "_La fuente no devolvió notas._\n");
        } else if (include_code) {
            try markdown.appendSlice("| Materia | Código | Trimestre | Promedio |\n| --- | --- | --- | ---: |\n");
            try markdown.appendSlice(rows);
        } else {
            try markdown.appendSlice("| Materia | Nota | Trimestre |\n| --- | ---: | --- |\n");
            try markdown.appendSlice(rows);
        }
        try markdown.appendSlice("\nFuente: MCP Mikareno (get_student_full).\n\n");
    } else {
        try markdown.appendSlice(if (include_code) "_Promedios no disponibles: la fuente académica no respondió._\n\n" else "_Notas no disponibles: la fuente académica no respondió._\n\n");
    }
}

fn collectReportGradeRows(allocator: std.mem.Allocator, academic: std.json.Value, include_code: bool) ![]const u8 {
    const averages = json.get(academic, "promedios") orelse .null;
    const stats = json.get(averages, "stats") orelse .null;
    if (stats != .array) return "";
    var rows = std.ArrayList(u8).init(allocator);
    for (stats.array.items) |entry| {
        const subject = std.mem.trim(u8, json.getString(entry, "courseName") orelse "", " \t\r\n");
        if (subject.len == 0) continue;
        const number_value = json.get(entry, "average") orelse continue;
        const number = reportNumber(number_value) orelse continue;
        const rounded = @round(number * 100.0) / 100.0;
        const average = try std.fmt.allocPrint(allocator, "{d}", .{rounded});
        const code = json.getString(entry, "courseCode") orelse "";
        const term = reportPartial(json.getString(entry, "partialId") orelse "");
        if (include_code) {
            try rows.writer().print("| {s} | {s} | {d} | {s} |\n", .{
                try markdownTableEscape(allocator, subject),
                try markdownTableEscape(allocator, code),
                term,
                average,
            });
        } else {
            try rows.writer().print("| {s} | {s} | {d} |\n", .{ try markdownTableEscape(allocator, subject), average, term });
        }
    }
    return rows.toOwnedSlice();
}

fn appendTasksSection(allocator: std.mem.Allocator, markdown: *std.ArrayList(u8), tasks: ?std.json.Value) !void {
    try markdown.appendSlice("## Tareas\n\n");
    if (tasks) |data| {
        const list = json.get(data, "lista_tareas") orelse json.get(data, "pageContent") orelse .null;
        if (list == .array) {
            if (list.array.items.len == 0) {
                try markdown.appendSlice("_La fuente no devolvió tareas._\n\n");
            } else {
                var rows = std.ArrayList(u8).init(allocator);
                var count: usize = 0;
                for (list.array.items) |task| {
                    if (task != .object) continue;
                    const title = firstReportString(task, &.{ "title", "titulo", "name", "asunto", "description" });
                    const due = firstReportString(task, &.{ "dueDate", "fechaVencimiento", "endDate", "fecha_limite", "vencimiento" });
                    const status = firstReportString(task, &.{ "status", "estado", "state", "deliveryStatus" });
                    try rows.writer().print("| {s} | {s} | {s} |\n", .{
                        try markdownTableEscape(allocator, if (title.len == 0) "Tarea sin título" else title),
                        try markdownTableEscape(allocator, due),
                        try markdownTableEscape(allocator, if (status.len == 0) "SIN_ESTADO" else status),
                    });
                    count += 1;
                }
                if (count == 0) {
                    try markdown.appendSlice("_La fuente no devolvió tareas._\n\n");
                } else {
                    try markdown.appendSlice("| Tarea | Vencimiento | Estado |\n| --- | --- | --- |\n");
                    try markdown.appendSlice(rows.items);
                    try markdown.appendSlice("\n");
                }
            }
            try markdown.appendSlice("Fuente: MCP Mikareno (get_tareas).\n\n");
        } else {
            try markdown.appendSlice("_Tareas no disponibles: la fuente académica no devolvió una lista válida._\n\n");
        }
    } else {
        try markdown.appendSlice("_Tareas no disponibles: la fuente académica no respondió._\n\n");
    }
}

fn reportNumber(value: std.json.Value) ?f64 {
    return switch (value) {
        .integer => |number| @floatFromInt(number),
        .float => |number| number,
        else => null,
    };
}

fn reportPartial(partial_id: []const u8) i64 {
    const marker = std.mem.indexOf(u8, partial_id, ":T") orelse return 0;
    const start = marker + 2;
    const end = start + (std.mem.indexOfScalar(u8, partial_id[start..], ':') orelse partial_id.len - start);
    return std.fmt.parseInt(i64, partial_id[start..end], 10) catch 0;
}

fn firstReportString(value: std.json.Value, fields: []const []const u8) []const u8 {
    for (fields) |field| {
        const text = json.getString(value, field) orelse continue;
        if (std.mem.trim(u8, text, " \t\r\n").len > 0) return text;
    }
    return "";
}

fn markdownTableEscape(allocator: std.mem.Allocator, input: []const u8) ![]const u8 {
    var safe = try std.mem.replaceOwned(u8, allocator, input, "\\", "\\\\");
    safe = try std.mem.replaceOwned(u8, allocator, safe, "|", "\\|");
    safe = try std.mem.replaceOwned(u8, allocator, safe, "\r", " ");
    safe = try std.mem.replaceOwned(u8, allocator, safe, "\n", " ");
    return safe;
}

fn editPresentation(allocator: std.mem.Allocator, config: *const config_mod.Config, route: []const u8, username: []const u8, slug: []const u8, presentation: std.json.Value) !http.Response {
    const title = json.getString(presentation, "title") orelse slug;
    const safe_title = try security.htmlEscape(allocator, title);
    const safe_slug = try security.htmlEscape(allocator, slug);
    const slides = json.get(presentation, "slides") orelse .null;
    if (slides != .array) return errorPage(allocator, config, route, username, 400, "Presentación no válida.");
    var thumbnails = std.ArrayList(u8).init(allocator);
    var editors = std.ArrayList(u8).init(allocator);
    for (slides.array.items, 0..) |slide, index| {
        const slide_title_raw = json.getString(slide, "title") orelse "";
        const slide_title = try security.htmlEscape(allocator, slide_title_raw);
        const slide_content_raw = json.getString(slide, "content") orelse "";
        const slide_content = try security.htmlEscape(allocator, slide_content_raw);
        const slide_html = try markdownHtml(allocator, slide_content_raw);
        try thumbnails.writer().print("<article class=\"gen-office-thumb\" data-idx=\"{d}\"><button type=\"button\" class=\"gen-office-thumb-select\" data-slide-target=\"{d}\" aria-pressed=\"{s}\" onclick=\"selectOfficeSlide({d})\"><span class=\"gen-office-thumb-number\">{d}</span><span class=\"gen-office-thumb-canvas\"><strong>{s}</strong><span>{s}</span></span></button><footer><span>Escena {d}</span><div><button type=\"button\" class=\"gen-office-icon-button\" onclick=\"moveSlide({d},-1)\" aria-label=\"Mover arriba\">↑</button><button type=\"button\" class=\"gen-office-icon-button\" onclick=\"moveSlide({d},1)\" aria-label=\"Mover abajo\">↓</button></div></footer></article>", .{ index, index, if (index == 0) "true" else "false", index, index + 1, slide_title, slide_content, index + 1, index, index });
        try editors.writer().print("<article class=\"gen-office-slide-editor\" id=\"slide-{d}\" data-slide-panel=\"{d}\"{s}><div class=\"gen-office-slide-canvas\"><span class=\"gen-office-slide-brand\">NOVA OFFICE · {d:0>2}</span><h2>{s}</h2><div class=\"gen-office-slide-content\">{s}</div><footer>{s} · Archivo local</footer></div><form class=\"gen-office-slide-form\" hx-put=\"/office/presentations/{s}/slides/{d}\" hx-swap=\"none\"><div><label for=\"slide-title-{d}\">Título</label><input id=\"slide-title-{d}\" name=\"title\" maxlength=\"200\" value=\"{s}\" required></div><div><label for=\"slide-content-{d}\">Contenido Markdown</label><textarea id=\"slide-content-{d}\" name=\"content\" maxlength=\"500000\" rows=\"5\">{s}</textarea></div><footer><span>Diapositiva {d} de {d}</span><button type=\"submit\" class=\"gen-office-primary-button\">Guardar escena</button></footer></form><form hx-delete=\"/office/presentations/{s}/slides/{d}\" hx-confirm=\"¿Eliminar esta diapositiva?\" hx-swap=\"none\"><button class=\"gen-office-secondary-button\">Eliminar diapositiva</button></form></article>", .{ index, index, if (index == 0) "" else " hidden", index + 1, slide_title, slide_html, safe_title, safe_slug, index, index, index, slide_title, index, index, slide_content, index + 1, slides.array.items.len, safe_slug, index });
    }
    const thumbs = if (slides.array.items.len == 0)
        "<div class=\"gen-office-empty compact\"><p>Aún no hay diapositivas. Añade la primera escena para empezar.</p></div>"
    else
        thumbnails.items;
    const editor = if (slides.array.items.len == 0)
        "<div class=\"gen-office-empty gen-office-slide-empty\"><span class=\"material-symbols-outlined\">present_to_all</span><h3>Tu presentación está lista.</h3><p>Añade una diapositiva para comenzar a construirla.</p></div>"
    else
        editors.items;
    const tabs = try officeTabs(allocator, "presentations");
    const add_form = try std.fmt.allocPrint(allocator, "<section class=\"gen-office-new-slide\"><div><span>NUEVA ESCENA</span><h3>Añadir diapositiva</h3></div><form hx-post=\"/office/presentations/{s}/slides\" hx-swap=\"none\"><label for=\"new-slide-title\">Título</label><input id=\"new-slide-title\" name=\"title\" maxlength=\"200\" placeholder=\"Una idea clara\" required><label for=\"new-slide-content\">Contenido Markdown</label><textarea id=\"new-slide-content\" name=\"content\" maxlength=\"500000\" rows=\"6\" placeholder=\"Idea principal&#10;&#10;Escribe tu contenido…\"></textarea><button type=\"submit\" class=\"gen-office-primary-button\">＋ Añadir diapositiva</button></form></section>", .{safe_slug});
    const js = try std.mem.replaceOwned(u8, allocator, PRESENTATION_EDITOR_JS, "__SLUG__", try json.stringify(allocator, .{ .string = safe_slug }));
    const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-editor-page\">{s}<header class=\"gen-office-ribbon\"><nav class=\"gen-office-menubar\"><a class=\"is-active\" href=\"/office/presentations/{s}\">Inicio</a><a href=\"/office/presentations/{s}/present\" target=\"_blank\">Presentar</a><button type=\"button\" onclick=\"document.getElementById('new-slide-title').focus()\">Nueva diapositiva</button></nav><div class=\"gen-office-ribbon-actions\"><a class=\"gen-office-primary-button\" href=\"/office/presentations/{s}/present\" target=\"_blank\">▶ Presentar</a><form hx-delete=\"/office/presentations/{s}\" hx-confirm=\"¿Eliminar esta presentación?\" hx-swap=\"none\"><button class=\"gen-office-icon-button danger\" aria-label=\"Eliminar presentación\">⌫</button></form></div></header><div class=\"gen-office-canvas-head\"><div><span>PRESENTACIÓN</span><strong>{s}</strong></div><span class=\"gen-office-local-badge\">Archivo JSON local</span></div><section class=\"gen-office-deck-stage\"><aside class=\"gen-office-thumbnails\"><header><span>DIAPOSITIVAS</span><button type=\"button\" class=\"gen-office-icon-button\" onclick=\"document.getElementById('new-slide-title').focus()\" aria-label=\"Añadir diapositiva\">＋</button></header><div id=\"slides\" class=\"gen-office-slide-list\">{s}</div><form id=\"reorder-form\" class=\"gen-office-reorder-form\" hx-post=\"/office/presentations/{s}/slides/reorder\" hx-swap=\"none\" onsubmit=\"return buildOfficeOrder([...document.querySelectorAll('#slides [data-idx]')].map(node=>Number(node.dataset.idx)))\"><input type=\"hidden\" name=\"order\" id=\"slide-order\"><button type=\"submit\" class=\"gen-office-secondary-button\">Aplicar orden</button></form></aside><main class=\"gen-office-slide-stage\">{s}{s}</main></section><footer class=\"gen-office-statusbar\"><span id=\"slide-position\">{s}</span><span>{d} diapositiva(s)</span><span>NOVA Office</span></footer></div><script>{s}</script>", .{ tabs, safe_slug, safe_slug, safe_slug, safe_slug, safe_title, thumbs, safe_slug, editor, add_form, if (slides.array.items.len == 0) "0 / 0" else "1 / 1", slides.array.items.len, js });
    return page(allocator, config, route, username, "Presentaciones", body, 200);
}

fn presentPage(allocator: std.mem.Allocator, config: *const config_mod.Config, route: []const u8, username: []const u8, presentation: std.json.Value) !http.Response {
    const title = json.getString(presentation, "title") orelse "Presentación";
    const safe_title = try security.htmlEscape(allocator, title);
    const slides = json.get(presentation, "slides") orelse .null;
    if (slides != .array) return errorPage(allocator, config, route, username, 400, "Presentación no válida.");
    var content = std.ArrayList(u8).init(allocator);
    for (slides.array.items, 0..) |slide, index| {
        const slide_title = try security.htmlEscape(allocator, json.getString(slide, "title") orelse "");
        const slide_body = try markdownHtml(allocator, json.getString(slide, "content") orelse "");
        try content.writer().print("<article class=\"gen-office-present-slide\" data-present-slide=\"{d}\"{s}><div class=\"gen-office-present-canvas\"><header><span>NOVA OFFICE</span><span>{s}</span></header><div><p>DIAPOSITIVA {d}</p><h2>{s}</h2><section class=\"gen-office-present-body\">{s}</section></div><footer><span>{s}</span><span>{d} / {d}</span></footer></div></article>", .{ index, if (index == 0) "" else " hidden", safe_title, index + 1, slide_title, slide_body, safe_title, index + 1, slides.array.items.len });
    }
    const count = slides.array.items.len;
    const editor_url = try std.fmt.allocPrint(allocator, "/office/presentations/{s}", .{route["/office/presentations/".len .. route.len - "/present".len]});
    const slide_content = if (count == 0)
        try std.fmt.allocPrint(allocator, "<div class=\"gen-office-empty\"><h2>Esta presentación aún no tiene diapositivas.</h2><a href=\"{s}\">Vuelve al editor para añadir una escena.</a></div>", .{editor_url})
    else
        content.items;
    const js = try std.mem.replaceOwned(u8, allocator, PRESENTATION_JS, "__TOTAL__", try std.fmt.allocPrint(allocator, "{d}", .{count}));
    const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-present-page\"><main class=\"gen-office-present-stage\" id=\"gen-office-present-stage\">{s}</main><nav class=\"gen-office-present-controls\"><a class=\"gen-office-secondary-button\" href=\"{s}\">Volver al editor</a><button id=\"previous-slide\" type=\"button\" onclick=\"changePresentationSlide(-1)\">← Anterior</button><span id=\"presentation-position\">{s}</span><button id=\"next-slide\" type=\"button\" onclick=\"changePresentationSlide(1)\">Siguiente →</button><button type=\"button\" onclick=\"togglePresentationFullscreen()\">Pantalla completa</button></nav><script>{s}</script></div>", .{ slide_content, editor_url, if (count == 0) "0 / 0" else "1 / 1", js });
    return page(allocator, config, route, username, title, body, 200);
}

fn listPage(allocator: std.mem.Allocator, config: *const config_mod.Config, route: []const u8, username: []const u8, title: []const u8, dir: []const u8, ext: []const u8, action_title: []const u8, action_url: []const u8) !http.Response {
    const kind = if (std.mem.eql(u8, ext, ".md")) "docs" else if (std.mem.eql(u8, ext, ".csv")) "sheets" else if (std.mem.eql(u8, ext, ".html")) "html" else "presentations";
    const icon = if (std.mem.eql(u8, kind, "docs")) "description" else if (std.mem.eql(u8, kind, "sheets")) "table_chart" else if (std.mem.eql(u8, kind, "html")) "code" else "present_to_all";
    const tabs = try officeTabs(allocator, kind);
    const nav = try officeNav(allocator, kind);
    var cards = std.ArrayList(u8).init(allocator);
    var count: usize = 0;
    var opened = std.fs.cwd().openDir(dir, .{ .iterate = true }) catch return errorPage(allocator, config, route, username, 500, "No se pudo leer la carpeta.");
    defer opened.close();
    var iter = opened.iterate();
    while (try iter.next()) |entry| {
        if (entry.kind != .file or !std.mem.endsWith(u8, entry.name, ext)) continue;
        const slug = entry.name[0 .. entry.name.len - ext.len];
        if (!safeSlug(slug)) continue;
        const escaped = try security.htmlEscape(allocator, slug);
        const editor_url = try std.fmt.allocPrint(allocator, "/office/{s}/{s}", .{ kind, escaped });
        const download_url = try std.fmt.allocPrint(allocator, "/office/library/{s}/{s}/download", .{ kind, escaped });
        const export_url = if (std.mem.eql(u8, kind, "docs")) try std.fmt.allocPrint(allocator, "{s}/export?fmt=md", .{editor_url}) else if (std.mem.eql(u8, kind, "sheets") or std.mem.eql(u8, kind, "html")) try std.fmt.allocPrint(allocator, "{s}/export", .{editor_url}) else download_url;
        const delete_form = if (std.mem.eql(u8, kind, "sheets"))
            try std.fmt.allocPrint(allocator, "<form hx-post=\"/office/sheets/{s}/delete\" hx-confirm=\"¿Eliminar esta hoja?\" hx-swap=\"none\"><button class=\"gen-office-icon-button danger\" aria-label=\"Eliminar {s}\">⌫</button></form>", .{ escaped, escaped })
        else
            try std.fmt.allocPrint(allocator, "<form hx-delete=\"{s}\" hx-confirm=\"¿Eliminar este archivo?\" hx-swap=\"none\"><button class=\"gen-office-icon-button danger\" aria-label=\"Eliminar {s}\">⌫</button></form>", .{ editor_url, escaped });
        const size = std.fs.cwd().statFile(try std.fmt.allocPrint(allocator, "{s}/{s}", .{ dir, entry.name })) catch null;
        const size_text = if (size) |stat| try formatHumanSize(allocator, stat.size) else "—";
        try cards.writer().print("<article class=\"gen-office-file-card {s}\"><a class=\"gen-office-file-preview\" href=\"{s}\"><span class=\"material-symbols-outlined\">{s}</span><small>{s}</small></a><div class=\"gen-office-file-copy\"><a href=\"{s}\">{s}</a><span>Archivo local · {s}</span></div><footer><a href=\"{s}\">{s}</a><a href=\"{s}\">Descargar</a>{s}</footer></article>", .{ kind, editor_url, icon, ext[1..], editor_url, escaped, size_text, export_url, if (std.mem.eql(u8, kind, "docs")) "Exportar" else if (std.mem.eql(u8, kind, "sheets")) "Exportar CSV" else if (std.mem.eql(u8, kind, "html")) "Exportar HTML" else "Abrir JSON", download_url, delete_form });
        count += 1;
    }
    const content = if (count == 0)
        try std.fmt.allocPrint(allocator, "<div class=\"gen-office-empty\"><span class=\"material-symbols-outlined\">{s}</span><h3>{s}</h3><p>Crea un archivo local para empezar.</p><a href=\"{s}\" class=\"gen-office-primary-button\">{s}</a></div>", .{ icon, if (std.mem.eql(u8, kind, "docs")) "Tu próximo documento empieza aquí." else if (std.mem.eql(u8, kind, "sheets")) "Empieza con una cuadrícula." else if (std.mem.eql(u8, kind, "html")) "Un lienzo web en blanco." else "Tu próxima idea merece un escenario.", action_url, action_title })
    else
        cards.items;
    const body = try std.fmt.allocPrint(allocator, "<div class=\"nova-page gen-office-page gen-office-manager-page\">{s}<header class=\"gen-office-manager-head\"><div><span>{s} · ARCHIVOS LOCALES</span><h1>{s}</h1><p>{d} archivo(s) disponibles en tu espacio local.</p></div><a href=\"{s}\" class=\"gen-office-primary-button\">＋ {s}</a></header>{s}<section class=\"gen-office-file-grid\">{s}</section><footer class=\"gen-office-home-footer\"><span>Guardado local · {s}</span><span>NOVA Office</span></footer></div>", .{ tabs, if (std.mem.eql(u8, kind, "docs")) "DOCUMENTOS MARKDOWN" else if (std.mem.eql(u8, kind, "sheets")) "HOJAS DE CÁLCULO" else if (std.mem.eql(u8, kind, "html")) "PÁGINAS WEB HTML" else "PRESENTACIONES", title, count, action_url, action_title, nav, content, ext[1..] });
    return page(allocator, config, route, username, title, body, 200);
}

fn formatHumanSize(allocator: std.mem.Allocator, bytes: u64) ![]const u8 {
    if (bytes >= 1024 * 1024) return std.fmt.allocPrint(allocator, "{d:.1} MB", .{@as(f64, @floatFromInt(bytes)) / (1024.0 * 1024.0)});
    if (bytes >= 1024) return std.fmt.allocPrint(allocator, "{d:.1} KB", .{@as(f64, @floatFromInt(bytes)) / 1024.0});
    return std.fmt.allocPrint(allocator, "{d} bytes", .{bytes});
}

fn renderGrid(allocator: std.mem.Allocator, slug: []const u8, source: CsvGrid, values: CsvGrid) ![]const u8 {
    const display_rows = @max(source.len, 5);
    const columns = @max(maxColumns(source), 5);
    var editable = std.ArrayList(u8).init(allocator);
    try editable.appendSlice("<table id=\"grid\" class=\"gen-office-sheet-grid\" aria-label=\"Cuadrícula editable\"><thead><tr><th class=\"gen-office-row-corner\"></th>");
    for (0..columns) |col| try editable.writer().print("<th scope=\"col\">{s}</th>", .{try columnName(allocator, col)});
    try editable.appendSlice("</tr></thead><tbody>");
    for (0..display_rows) |row_index| {
        try editable.writer().print("<tr><th scope=\"row\">{d}</th>", .{row_index + 1});
        for (0..columns) |col| {
            const raw = if (row_index < source.len and col < source[row_index].len) source[row_index][col] else "";
            const ref = try std.fmt.allocPrint(allocator, "{s}{d}", .{ try columnName(allocator, col), row_index + 1 });
            try editable.writer().print("<td contenteditable=\"true\" data-r=\"{d}\" data-c=\"{d}\" data-cell=\"{s}\" aria-label=\"Celda {s}\">{s}</td>", .{ row_index, col, ref, ref, try security.htmlEscape(allocator, raw) });
        }
        try editable.appendSlice("</tr>");
    }
    try editable.appendSlice("</tbody></table>");

    var computed = std.ArrayList(u8).init(allocator);
    try computed.appendSlice("<table class=\"gen-office-sheet-values\"><tbody>");
    for (0..display_rows) |row_index| {
        try computed.appendSlice("<tr>");
        for (0..columns) |col| {
            const value = if (row_index < values.len and col < values[row_index].len) values[row_index][col] else "";
            try computed.writer().print("<td>{s}</td>", .{try security.htmlEscape(allocator, value)});
        }
        try computed.appendSlice("</tr>");
    }
    try computed.appendSlice("</tbody></table>");
    const slug_safe = try security.htmlEscape(allocator, slug);
    return std.fmt.allocPrint(allocator, "<div class=\"gen-office-sheet-head\"><div><span>HOJA1 · CUADRÍCULA EDITABLE</span><p id=\"sheet-grid-count\">{d} filas · {d} columnas</p></div><form hx-post=\"/office/sheets/{s}/import\" hx-swap=\"none\" hx-on::before-request=\"serializeGrid()\"><input type=\"hidden\" name=\"csv_data\" id=\"csv_data\" value=\"\"><button type=\"submit\" id=\"save-grid-button\" class=\"gen-office-primary-button\">Guardar cuadrícula</button></form></div><div class=\"gen-office-formula-bar\"><span id=\"active-cell\">A1</span><span class=\"gen-office-fx\">fx</span><label class=\"sr-only\" for=\"formula-input\">Contenido de la celda seleccionada</label><input id=\"formula-input\" type=\"text\" autocomplete=\"off\" placeholder=\"Selecciona una celda o escribe una fórmula\"></div><div class=\"gen-office-sheet-grid-wrap\">{s}</div><details class=\"gen-office-sheet-computed\"><summary><span>Valores calculados</span><span class=\"material-symbols-outlined\">expand_more</span></summary><div>{s}<p>Fórmulas compatibles: <code>=SUM(A1:A5)</code>, <code>=AVG(B2:B8)</code>, <code>=AVERAGE(B2:B8)</code>. Las fórmulas se evalúan sobre los valores originales, sin recursión.</p></div></details><script>{s}</script>", .{ display_rows, columns, slug_safe, editable.items, computed.items, GRID_SERIALIZE_JS });
}

fn columnName(allocator: std.mem.Allocator, col_index: usize) ![]const u8 {
    var value = col_index + 1;
    var reversed = std.ArrayList(u8).init(allocator);
    while (value > 0) {
        const remainder = (value - 1) % 26;
        try reversed.append(@as(u8, @intCast('A' + remainder)));
        value = (value - 1) / 26;
    }
    var name = try allocator.alloc(u8, reversed.items.len);
    for (reversed.items, 0..) |byte, index| name[reversed.items.len - index - 1] = byte;
    return name;
}

const GRID_SERIALIZE_JS = "function officeCsvCell(value){const clean=value.replace(/\\r?\\n/g,' ');return /[\\\",]/.test(clean)?'\\\"'+clean.replace(/\\\"/g,'\\\"\\\"')+'\\\"':clean;} function serializeGrid(){const lines=[];document.querySelectorAll('#grid tbody tr').forEach(tr=>{const cells=tr.querySelectorAll('td[contenteditable]');if(!cells.length)return;lines.push([...cells].map(td=>officeCsvCell(td.textContent)).join(','));});const hidden=document.getElementById('csv_data');if(hidden)hidden.value=lines.join('\\n');return hidden?hidden.value:'';} function officeColumnName(index){let n=index+1,s='';while(n>0){const r=(n-1)%26;s=String.fromCharCode(65+r)+s;n=Math.floor((n-1)/26);}return s;} function renumberOfficeGrid(){const grid=document.getElementById('grid');if(!grid)return;const rows=[...grid.querySelectorAll('tbody tr')],cols=rows.reduce((m,r)=>Math.max(m,r.querySelectorAll('td[contenteditable]').length),0),head=grid.querySelector('thead tr');if(head){while(head.cells.length>cols+1)head.deleteCell(-1);while(head.cells.length<cols+1){const th=document.createElement('th');th.scope='col';head.appendChild(th);} [...head.cells].forEach((cell,i)=>cell.textContent=i?officeColumnName(i-1):'');} rows.forEach((row,r)=>{const number=row.querySelector('th');if(number)number.textContent=String(r+1);row.querySelectorAll('td[contenteditable]').forEach((cell,c)=>{const ref=officeColumnName(c)+(r+1);cell.dataset.r=String(r);cell.dataset.c=String(c);cell.dataset.cell=ref;cell.setAttribute('aria-label','Celda '+ref);});});const count=document.getElementById('sheet-grid-count');if(count)count.textContent=rows.length+' filas · '+cols+' columnas';} function selectOfficeCell(cell){if(!cell)return;document.querySelectorAll('#grid td.is-selected').forEach(item=>item.classList.remove('is-selected'));cell.classList.add('is-selected');const ref=document.getElementById('active-cell'),formula=document.getElementById('formula-input');if(ref)ref.textContent=cell.dataset.cell||'';if(formula)formula.value=cell.textContent;} function addOfficeGridRow(){const body=document.querySelector('#grid tbody');if(!body||body.rows.length>=5000)return;const cols=body.querySelector('tr')?.querySelectorAll('td[contenteditable]').length||5,row=document.createElement('tr'),heading=document.createElement('th');row.appendChild(heading);for(let i=0;i<cols;i++){const td=document.createElement('td');td.contentEditable='true';row.appendChild(td);}body.appendChild(row);renumberOfficeGrid();selectOfficeCell(row.querySelector('td'));} function addOfficeGridColumn(){const grid=document.getElementById('grid');if(!grid||grid.querySelector('thead tr').cells.length>500)return;grid.querySelectorAll('tbody tr').forEach(row=>{const td=document.createElement('td');td.contentEditable='true';row.appendChild(td);});renumberOfficeGrid();} function initializeOfficeGrid(){const grid=document.getElementById('grid'),formula=document.getElementById('formula-input');if(!grid||!formula)return;renumberOfficeGrid();if(!grid.dataset.ready){grid.dataset.ready='true';grid.addEventListener('focusin',event=>{if(event.target.matches('td[contenteditable]'))selectOfficeCell(event.target);});grid.addEventListener('input',event=>{if(event.target.matches('td[contenteditable].is-selected'))formula.value=event.target.textContent;});formula.addEventListener('input',()=>{const selected=grid.querySelector('td.is-selected');if(selected)selected.textContent=formula.value;});formula.addEventListener('keydown',event=>{if(event.key==='Enter'){event.preventDefault();grid.querySelector('td.is-selected')?.focus();}});}if(!grid.querySelector('td.is-selected'))selectOfficeCell(grid.querySelector('td[contenteditable]'));} initializeOfficeGrid();";

const CsvGrid = [][]const []const u8;

fn parseCsv(allocator: std.mem.Allocator, input: []const u8) !CsvGrid {
    if (input.len > MAX_CSV) return error.CsvTooLarge;
    var rows = std.ArrayList([]const []const u8).init(allocator);
    var row = std.ArrayList([]const u8).init(allocator);
    var field = std.ArrayList(u8).init(allocator);
    var quoted = false;
    var index: usize = 0;
    while (index < input.len) : (index += 1) {
        const byte = input[index];
        if (byte == '"') {
            if (quoted and index + 1 < input.len and input[index + 1] == '"') {
                try field.append('"');
                index += 1;
            } else {
                quoted = !quoted;
            }
        } else if (!quoted and byte == ',') {
            if (row.items.len >= MAX_SHEET_COLUMNS) return error.TooManyColumns;
            try row.append(try field.toOwnedSlice());
            field = std.ArrayList(u8).init(allocator);
        } else if (!quoted and (byte == '\n' or byte == '\r')) {
            if (byte == '\r' and index + 1 < input.len and input[index + 1] == '\n') index += 1;
            if (row.items.len >= MAX_SHEET_COLUMNS) return error.TooManyColumns;
            try row.append(try field.toOwnedSlice());
            field = std.ArrayList(u8).init(allocator);
            if (rows.items.len >= MAX_SHEET_ROWS) return error.TooManyRows;
            try rows.append(try row.toOwnedSlice());
            row = std.ArrayList([]const u8).init(allocator);
        } else {
            try field.append(byte);
        }
    }
    if (quoted) return error.InvalidCsv;
    if (field.items.len > 0 or row.items.len > 0 or rows.items.len == 0) {
        if (row.items.len >= MAX_SHEET_COLUMNS) return error.TooManyColumns;
        try row.append(try field.toOwnedSlice());
        if (rows.items.len >= MAX_SHEET_ROWS) return error.TooManyRows;
        try rows.append(try row.toOwnedSlice());
    }
    return rows.toOwnedSlice();
}

fn computeGrid(allocator: std.mem.Allocator, source: CsvGrid) !CsvGrid {
    var rows = std.ArrayList([]const []const u8).init(allocator);
    for (source, 0..) |row, row_index| {
        var values = std.ArrayList([]const u8).init(allocator);
        for (row, 0..) |cell, col_index| try values.append(try computeCell(allocator, source, cell, row_index, col_index));
        try rows.append(try values.toOwnedSlice());
    }
    return rows.toOwnedSlice();
}

fn computeCell(allocator: std.mem.Allocator, grid: CsvGrid, raw: []const u8, row_index: usize, col_index: usize) ![]const u8 {
    _ = row_index;
    _ = col_index;
    const formula = std.mem.trim(u8, raw, " \t\r\n");
    if (!std.mem.startsWith(u8, formula, "=")) return raw;
    const expression = std.mem.trim(u8, formula[1..], " \t\r\n");
    const open = std.mem.indexOfScalar(u8, expression, '(') orelse return formulaError(allocator, raw);
    const close = std.mem.lastIndexOfScalar(u8, expression, ')') orelse return formulaError(allocator, raw);
    if (close != expression.len - 1 or close <= open) return formulaError(allocator, raw);
    const function_name = std.mem.trim(u8, expression[0..open], " \t");
    const is_sum = std.ascii.eqlIgnoreCase(function_name, "SUM");
    const is_avg = std.ascii.eqlIgnoreCase(function_name, "AVG") or std.ascii.eqlIgnoreCase(function_name, "AVERAGE");
    if (!is_sum and !is_avg) return formulaError(allocator, raw);
    const range = std.mem.trim(u8, expression[open + 1 .. close], " \t");
    const colon = std.mem.indexOfScalar(u8, range, ':');
    const first = if (colon) |split| range[0..split] else range;
    const second = if (colon) |split| range[split + 1 ..] else range;
    const start = parseCellRef(std.mem.trim(u8, first, " \t")) orelse return formulaError(allocator, raw);
    const end = parseCellRef(std.mem.trim(u8, second, " \t")) orelse return formulaError(allocator, raw);
    const min_row = @min(start.row, end.row);
    const max_row = @max(start.row, end.row);
    const min_col = @min(start.col, end.col);
    const max_col = @max(start.col, end.col);
    const row_span = max_row - min_row + 1;
    const col_span = max_col - min_col + 1;
    const area = std.math.mul(usize, row_span, col_span) catch return formulaError(allocator, raw);
    if (area > 100_000) return formulaError(allocator, raw);

    var sum: f64 = 0;
    var count: usize = 0;
    for (min_row..max_row + 1) |range_row| {
        if (range_row >= grid.len) continue;
        for (min_col..max_col + 1) |range_col| {
            if (range_col >= grid[range_row].len) continue;
            const value = std.fmt.parseFloat(f64, std.mem.trim(u8, grid[range_row][range_col], " \t")) catch continue;
            sum += value;
            count += 1;
        }
    }
    if (is_avg) sum = if (count == 0) 0 else sum / @as(f64, @floatFromInt(count));
    if (!std.math.isFinite(sum)) return formulaError(allocator, raw);
    if (@floor(sum) == sum and @abs(sum) < 1.0e15) return std.fmt.allocPrint(allocator, "{d}", .{@as(i64, @intFromFloat(sum))});
    return std.fmt.allocPrint(allocator, "{d}", .{sum});
}

fn formulaError(allocator: std.mem.Allocator, formula: []const u8) ![]const u8 {
    const safe_formula = try std.mem.replaceOwned(u8, allocator, formula, "\n", " ");
    return std.fmt.allocPrint(allocator, "#ERROR: unsupported formula '{s}' (supported: =SUM(range), =AVG(range), =AVERAGE(range))", .{safe_formula});
}

const CellRef = struct { row: usize, col: usize };
fn parseCellRef(raw: []const u8) ?CellRef {
    var index: usize = 0;
    var col: usize = 0;
    while (index < raw.len and std.ascii.isAlphabetic(raw[index])) : (index += 1) {
        const byte = std.ascii.toUpper(raw[index]);
        col = std.math.mul(usize, col, 26) catch return null;
        col = std.math.add(usize, col, byte - 'A' + 1) catch return null;
    }
    if (index == 0 or index == raw.len) return null;
    const row_number = std.fmt.parseInt(usize, raw[index..], 10) catch return null;
    if (row_number == 0) return null;
    return .{ .row = row_number - 1, .col = col - 1 };
}

fn maxColumns(grid: CsvGrid) usize {
    var result: usize = 0;
    for (grid) |row| result = @max(result, row.len);
    return result;
}

fn renderCsv(allocator: std.mem.Allocator, grid: CsvGrid) ![]const u8 {
    var output = std.ArrayList(u8).init(allocator);
    for (grid, 0..) |row, row_index| {
        for (row, 0..) |cell, col_index| {
            if (col_index > 0) try output.append(',');
            const needs_quotes = std.mem.indexOfAny(u8, cell, ",\"\r\n") != null;
            if (needs_quotes) try output.append('"');
            for (cell) |byte| {
                if (byte == '"') try output.append('"');
                try output.append(byte);
            }
            if (needs_quotes) try output.append('"');
        }
        if (row_index + 1 < grid.len) try output.appendSlice("\r\n");
    }
    return output.toOwnedSlice();
}

const Presentation = struct { title: []const u8, slides: []const Slide };
fn slideValue(allocator: std.mem.Allocator, title: []const u8, content: []const u8) !std.json.Value {
    return json.object(allocator, &.{ .{ .key = "title", .value = .{ .string = title } }, .{ .key = "content", .value = .{ .string = content } } });
}
fn loadPresentation(allocator: std.mem.Allocator, path: []const u8) !std.json.Value {
    const raw = try readFile(allocator, path, MAX_CONTENT * 4);
    const value = try json.parse(allocator, raw);
    const title = json.getString(value, "title") orelse return error.InvalidPresentation;
    if (title.len == 0 or title.len > MAX_TITLE) return error.InvalidPresentation;
    const slides = json.get(value, "slides") orelse return error.InvalidPresentation;
    if (slides != .array or slides.array.items.len > 200) return error.InvalidPresentation;
    for (slides.array.items) |slide| {
        const slide_title = json.getString(slide, "title") orelse return error.InvalidPresentation;
        const content = json.getString(slide, "content") orelse return error.InvalidPresentation;
        if (std.mem.trim(u8, slide_title, " \t\r\n").len == 0 or slide_title.len > MAX_TITLE or content.len > MAX_CONTENT) return error.InvalidPresentation;
    }
    return value;
}
fn getSlidesPtr(presentation: *std.json.Value) ?*std.json.Array {
    if (presentation.* != .object) return null;
    const slides = presentation.object.getPtr("slides") orelse return null;
    if (slides.* != .array) return null;
    return &slides.array;
}
fn getSlidePtr(presentation: *std.json.Value, index: usize) ?*std.json.Value {
    const slides = getSlidesPtr(presentation) orelse return null;
    if (index >= slides.items.len) return null;
    return &slides.items[index];
}
fn savePresentation(path: []const u8, allocator: std.mem.Allocator, value: std.json.Value) !void {
    try writeReplace(path, try json.stringify(allocator, value));
}
fn reorderSlides(allocator: std.mem.Allocator, presentation: *std.json.Value, order_text: []const u8) !bool {
    const slides = getSlidesPtr(presentation) orelse return false;
    var indices = std.ArrayList(usize).init(allocator);
    var iter = std.mem.splitScalar(u8, order_text, ',');
    while (iter.next()) |part| {
        const index = std.fmt.parseInt(usize, std.mem.trim(u8, part, " \t"), 10) catch return false;
        if (index >= slides.items.len) return false;
        for (indices.items) |existing| if (existing == index) return false;
        try indices.append(index);
    }
    if (indices.items.len != slides.items.len) return false;
    var reordered = std.json.Array.init(allocator);
    for (indices.items) |index| try reordered.append(slides.items[index]);
    slides.* = reordered;
    return true;
}
fn importPresentation(allocator: std.mem.Allocator, name: []const u8, content: []const u8) !std.json.Value {
    if (std.mem.trim(u8, content, " \t\r\n").len > 0 and std.mem.trimLeft(u8, content, " \t\r\n")[0] == '{') {
        const parsed = json.parse(allocator, content) catch return error.InvalidPresentation;
        const title = json.getString(parsed, "title") orelse name;
        const slides = json.get(parsed, "slides") orelse return error.InvalidPresentation;
        if (title.len == 0 or title.len > MAX_TITLE or slides != .array or slides.array.items.len == 0 or slides.array.items.len > 200) return error.InvalidPresentation;
        for (slides.array.items) |slide| {
            const slide_title = json.getString(slide, "title") orelse return error.InvalidPresentation;
            const slide_content = json.getString(slide, "content") orelse return error.InvalidPresentation;
            if (std.mem.trim(u8, slide_title, " \t\r\n").len == 0 or slide_content.len > MAX_CONTENT) return error.InvalidPresentation;
        }
        return parsed;
    }
    return json.object(allocator, &.{ .{ .key = "title", .value = .{ .string = name } }, .{ .key = "slides", .value = try json.array(allocator, &.{try slideValue(allocator, name, content)}) } });
}

fn markdownHtml(allocator: std.mem.Allocator, markdown: []const u8) ![]const u8 {
    var output = std.ArrayList(u8).init(allocator);
    var lines = std.mem.splitScalar(u8, markdown, '\n');
    var in_list = false;
    while (lines.next()) |raw_line| {
        const line = std.mem.trimRight(u8, raw_line, "\r");
        if (line.len == 0) {
            if (in_list) {
                try output.appendSlice("</ul>");
                in_list = false;
            }
            continue;
        }
        var heading: usize = 0;
        while (heading < line.len and heading < 6 and line[heading] == '#') : (heading += 1) {}
        if (heading > 0 and heading < line.len and line[heading] == ' ') {
            if (in_list) {
                try output.appendSlice("</ul>");
                in_list = false;
            }
            const content = try inlineMarkdown(allocator, line[heading + 1 ..]);
            try output.writer().print("<h{d}>{s}</h{d}>", .{ heading, content, heading });
        } else if (std.mem.startsWith(u8, line, "- ") or std.mem.startsWith(u8, line, "* ")) {
            if (!in_list) {
                try output.appendSlice("<ul>");
                in_list = true;
            }
            try output.writer().print("<li>{s}</li>", .{try inlineMarkdown(allocator, line[2..])});
        } else {
            if (in_list) {
                try output.appendSlice("</ul>");
                in_list = false;
            }
            try output.writer().print("<p>{s}</p>", .{try inlineMarkdown(allocator, line)});
        }
    }
    if (in_list) try output.appendSlice("</ul>");
    return output.toOwnedSlice();
}
fn inlineMarkdown(allocator: std.mem.Allocator, input: []const u8) ![]const u8 {
    const escaped = try security.htmlEscape(allocator, input);
    var value: []const u8 = try std.mem.replaceOwned(u8, allocator, escaped, "**", "\x01");
    value = try replaceToken(allocator, value, "\x01", "<strong>", "</strong>");
    value = try replaceToken(allocator, value, "*", "<em>", "</em>");
    value = try replaceToken(allocator, value, "`", "<code>", "</code>");
    return value;
}
fn replaceToken(allocator: std.mem.Allocator, input: []const u8, token: []const u8, open: []const u8, close: []const u8) ![]const u8 {
    var output = std.ArrayList(u8).init(allocator);
    var rest = input;
    var opened = false;
    while (std.mem.indexOf(u8, rest, token)) |index| {
        try output.appendSlice(rest[0..index]);
        try output.appendSlice(if (opened) close else open);
        opened = !opened;
        rest = rest[index + token.len ..];
    }
    try output.appendSlice(rest);
    if (opened) try output.appendSlice(close);
    return output.toOwnedSlice();
}

fn formField(allocator: std.mem.Allocator, request: *const http.Request, key: []const u8) !?[]const u8 {
    const content_type = request.header("content-type") orelse "";
    if (std.mem.startsWith(u8, content_type, "application/json")) {
        const value = json.parse(allocator, request.body) catch return error.InvalidJson;
        return json.getString(value, key);
    }
    var form = try http.parseForm(allocator, request.body);
    return form.get(key);
}
fn splitPath(allocator: std.mem.Allocator, path: []const u8) ![][]const u8 {
    var values = std.ArrayList([]const u8).init(allocator);
    var it = std.mem.splitScalar(u8, path, '/');
    while (it.next()) |part| if (part.len > 0) try values.append(part);
    return values.toOwnedSlice();
}
fn safeSlug(slug: []const u8) bool {
    if (slug.len == 0 or slug.len > MAX_TITLE or std.mem.indexOf(u8, slug, "..") != null) return false;
    const view = std.unicode.Utf8View.init(slug) catch return false;
    var iter = view.iterator();
    while (iter.nextCodepoint()) |codepoint| {
        if (codepoint == '-' or codepoint == '_') continue;
        if (!slugAlphanumeric(codepoint)) return false;
    }
    return true;
}

fn slugify(allocator: std.mem.Allocator, title: []const u8) ![]const u8 {
    var out = std.ArrayList(u8).init(allocator);
    defer out.deinit();
    const view = std.unicode.Utf8View.init(title) catch return allocator.dupe(u8, "untitled");
    var iter = view.iterator();
    while (iter.nextCodepoint()) |codepoint| {
        if (slugAlphanumeric(codepoint)) {
            try appendSlugCodepoint(&out, lowerSlugCodepoint(codepoint));
        } else if (codepoint == '-' or codepoint == '_') {
            try out.append(@intCast(codepoint));
        } else if (slugWhitespace(codepoint)) {
            try out.append('-');
        }
    }
    var start: usize = 0;
    var end = out.items.len;
    while (start < end and (out.items[start] == '-' or out.items[start] == '_')) : (start += 1) {}
    while (end > start and (out.items[end - 1] == '-' or out.items[end - 1] == '_')) : (end -= 1) {}
    if (start == end) return allocator.dupe(u8, "untitled");
    return allocator.dupe(u8, out.items[start..end]);
}

fn appendSlugCodepoint(out: *std.ArrayList(u8), codepoint: u21) !void {
    var buffer: [4]u8 = undefined;
    const length = try std.unicode.utf8Encode(codepoint, &buffer);
    try out.appendSlice(buffer[0..@as(usize, length)]);
}

fn slugWhitespace(codepoint: u21) bool {
    return (codepoint >= 0x09 and codepoint <= 0x0D) or
        codepoint == 0x20 or codepoint == 0x85 or codepoint == 0xA0 or
        codepoint == 0x1680 or (codepoint >= 0x2000 and codepoint <= 0x200A) or
        codepoint == 0x2028 or codepoint == 0x2029 or codepoint == 0x202F or
        codepoint == 0x205F or codepoint == 0x3000;
}

fn slugAlphanumeric(codepoint: u21) bool {
    if (codepoint <= 0x7F) return std.ascii.isAlphanumeric(@intCast(codepoint));
    if (codepoint == 0x00D7 or codepoint == 0x00F7 or codepoint == 0x037E) return false;
    return (codepoint >= 0x00C0 and codepoint <= 0x02FF) or
        (codepoint >= 0x0370 and codepoint <= 0x052F) or
        (codepoint >= 0x0531 and codepoint <= 0x0588) or
        (codepoint >= 0x05D0 and codepoint <= 0x05F4) or
        (codepoint >= 0x0620 and codepoint <= 0x06FF) or
        (codepoint >= 0x0750 and codepoint <= 0x077F) or
        (codepoint >= 0x08A0 and codepoint <= 0x08FF) or
        (codepoint >= 0x0900 and codepoint <= 0x1FFF) or
        (codepoint >= 0x2C00 and codepoint <= 0x2DFF) or
        (codepoint >= 0x3040 and codepoint <= 0xD7FF) or
        (codepoint >= 0xF900 and codepoint <= 0xFAFF) or
        (codepoint >= 0xFF10 and codepoint <= 0xFF19) or
        (codepoint >= 0xFF21 and codepoint <= 0xFF3A) or
        (codepoint >= 0xFF41 and codepoint <= 0xFF5A) or
        (codepoint >= 0x10000 and codepoint <= 0x1EFFF) or
        (codepoint >= 0x20000 and codepoint <= 0x3134F);
}

fn lowerSlugCodepoint(codepoint: u21) u21 {
    if (codepoint >= 'A' and codepoint <= 'Z') return codepoint + 0x20;
    if ((codepoint >= 0x00C0 and codepoint <= 0x00D6) or (codepoint >= 0x00D8 and codepoint <= 0x00DE)) return codepoint + 0x20;
    if (codepoint == 0x0130) return 'i';
    if (codepoint == 0x0178) return 0x00FF;
    if (codepoint == 0x1E9E) return 0x00DF;
    if (codepoint == 0x212A) return 'k';
    if (codepoint == 0x212B) return 0x00E5;
    if ((codepoint >= 0x0391 and codepoint <= 0x03A1) or (codepoint >= 0x03A3 and codepoint <= 0x03AB)) return codepoint + 0x20;
    if (codepoint >= 0x0410 and codepoint <= 0x042F) return codepoint + 0x20;
    if (codepoint >= 0x0400 and codepoint <= 0x040F) return codepoint + 0x50;
    if (codepoint >= 0x0531 and codepoint <= 0x0556) return codepoint + 0x30;
    return codepoint;
}

fn extensionFor(kind: []const u8) ?[]const u8 {
    if (std.mem.eql(u8, kind, "docs")) return ".md";
    if (std.mem.eql(u8, kind, "sheets")) return ".csv";
    if (std.mem.eql(u8, kind, "presentations")) return ".json";
    if (std.mem.eql(u8, kind, "html")) return ".html";
    return null;
}
fn officeRoot(allocator: std.mem.Allocator, config: *const config_mod.Config) ![]const u8 {
    const parent = std.fs.path.dirname(config.db_path) orelse ".";
    return std.fs.path.join(allocator, &.{ parent, "office" });
}

/// Tipos de archivo que reconoce NOVA Office.
pub const Kind = enum {
    docs,
    sheets,
    presentations,
    html,

    pub fn dir(self: Kind) []const u8 {
        return @tagName(self);
    }

    pub fn ext(self: Kind) []const u8 {
        return switch (self) {
            .docs => ".md",
            .sheets => ".csv",
            .presentations => ".json",
            .html => ".html",
        };
    }
};

pub const OfficeError = error{
    InvalidTitle,
    TitleTooLong,
    ContentTooLarge,
    FileExists,
    NotFound,
    InvalidSlug,
    IoError,
};

/// Crea (o reescribe) un archivo de Office con el contenido dado. Lo usa el
/// MCP para que un agente pueda escribir en la plataforma igual que desde la
/// interfaz: documentos, hojas de cálculo, presentaciones y páginas HTML.
pub fn createFileForMcp(
    allocator: std.mem.Allocator,
    config: *const config_mod.Config,
    kind: Kind,
    title: []const u8,
    content: []const u8,
) OfficeError![]const u8 {
    const trimmed = std.mem.trim(u8, title, " \t\r\n");
    if (trimmed.len == 0) return error.InvalidTitle;
    if (trimmed.len > 200) return error.TitleTooLong;
    const max: usize = if (kind == .docs) 500_000 else 2_000_000;
    if (content.len > max) return error.ContentTooLarge;

    const root = officeRoot(allocator, config) catch return error.IoError;
    const dir = officeDir(allocator, root, kind.dir()) catch return error.IoError;
    std.fs.cwd().makePath(dir) catch return error.IoError;

    const slug = slugify(allocator, trimmed) catch return error.IoError;
    if (!safeSlug(slug)) return error.InvalidSlug;
    const path = std.fmt.allocPrint(allocator, "{s}/{s}{s}", .{ dir, slug, kind.ext() }) catch return error.IoError;

    if (fileExists(path) catch false) {
        // Reescribir lo existente es lo esperable al "guardar" desde un agente.
        writeReplace(path, content) catch return error.IoError;
        return slug;
    }
    writeNew(path, content) catch return error.IoError;
    return slug;
}

/// Contenido de un archivo de Office ya guardado.
pub const OfficeFile = struct { slug: []const u8, content: []const u8, path: []const u8 };

pub fn readFileForMcp(allocator: std.mem.Allocator, config: *const config_mod.Config, kind: Kind, slug: []const u8) OfficeError!OfficeFile {
    if (!safeSlug(slug)) return error.InvalidSlug;
    const root = officeRoot(allocator, config) catch return error.IoError;
    const dir = officeDir(allocator, root, kind.dir()) catch return error.IoError;
    const path = std.fmt.allocPrint(allocator, "{s}/{s}{s}", .{ dir, slug, kind.ext() }) catch return error.IoError;
    const limit: usize = if (kind == .sheets) 4_000_000 else 2_000_000;
    const content = readFile(allocator, path, limit) catch return error.NotFound;
    return .{ .slug = slug, .content = content, .path = path };
}

/// Lista los archivos de un tipo de Office.
pub fn listFilesForMcp(allocator: std.mem.Allocator, config: *const config_mod.Config, kind: Kind) !std.json.Value {
    const root = officeRoot(allocator, config) catch return error.IoError;
    const dir_path = try officeDir(allocator, root, kind.dir());
    var list = std.json.Array.init(allocator);
    var dir = std.fs.cwd().openDir(dir_path, .{ .iterate = true }) catch {
        return json.object(allocator, &.{
            .{ .key = "kind", .value = .{ .string = kind.dir() } },
            .{ .key = "count", .value = .{ .integer = 0 } },
            .{ .key = "files", .value = .{ .array = list } },
        });
    };
    defer dir.close();
    var iter = dir.iterate();
    while (try iter.next()) |entry| {
        if (entry.kind != .file or !std.mem.endsWith(u8, entry.name, kind.ext())) continue;
        const full = try std.fs.path.join(allocator, &.{ dir_path, entry.name });
        const stat = std.fs.cwd().statFile(full) catch continue;
        var slug = entry.name;
        if (std.mem.endsWith(u8, slug, kind.ext())) slug = slug[0 .. slug.len - kind.ext().len];
        try list.append(try json.object(allocator, &.{
            .{ .key = "slug", .value = .{ .string = try allocator.dupe(u8, slug) } },
            .{ .key = "name", .value = .{ .string = try allocator.dupe(u8, entry.name) } },
            .{ .key = "bytes", .value = .{ .integer = @intCast(stat.size) } },
            .{ .key = "modified", .value = .{ .integer = @intCast(@max(0, stat.mtime)) } },
        }));
    }
    return json.object(allocator, &.{
        .{ .key = "kind", .value = .{ .string = kind.dir() } },
        .{ .key = "count", .value = .{ .integer = @intCast(list.items.len) } },
        .{ .key = "files", .value = .{ .array = list } },
    });
}

/// Borra un archivo de Office.
pub fn deleteFileForMcp(allocator: std.mem.Allocator, config: *const config_mod.Config, kind: Kind, slug: []const u8) OfficeError!void {
    if (!safeSlug(slug)) return error.InvalidSlug;
    const root = officeRoot(allocator, config) catch return error.IoError;
    const dir = officeDir(allocator, root, kind.dir()) catch return error.IoError;
    const path = std.fmt.allocPrint(allocator, "{s}/{s}{s}", .{ dir, slug, kind.ext() }) catch return error.IoError;
    std.fs.cwd().deleteFile(path) catch |err| switch (err) {
        error.FileNotFound => return error.NotFound,
        else => return error.IoError,
    };
}
fn officeDir(allocator: std.mem.Allocator, root: []const u8, kind: []const u8) ![]const u8 {
    return std.fs.path.join(allocator, &.{ root, kind });
}
fn countFiles(allocator: std.mem.Allocator, root: []const u8, kind: []const u8, ext: []const u8) !usize {
    const dir_path = try officeDir(allocator, root, kind);
    var dir = try std.fs.cwd().openDir(dir_path, .{ .iterate = true });
    defer dir.close();
    var iter = dir.iterate();
    var count: usize = 0;
    while (try iter.next()) |entry| if (entry.kind == .file and std.mem.endsWith(u8, entry.name, ext)) {
        count += 1;
    };
    return count;
}
fn fileExists(path: []const u8) !bool {
    std.fs.cwd().access(path, .{}) catch |err| switch (err) {
        error.FileNotFound => return false,
        else => return err,
    };
    return true;
}
fn writeNew(path: []const u8, content: []const u8) !void {
    const file = try std.fs.cwd().createFile(path, .{ .exclusive = true });
    defer file.close();
    try file.writeAll(content);
}
fn writeReplace(path: []const u8, content: []const u8) !void {
    const file = try std.fs.cwd().createFile(path, .{ .truncate = true });
    defer file.close();
    try file.writeAll(content);
}
fn readFile(allocator: std.mem.Allocator, path: []const u8, max_size: usize) ![]const u8 {
    const file = try std.fs.cwd().openFile(path, .{});
    defer file.close();
    return file.readToEndAlloc(allocator, max_size);
}
fn page(allocator: std.mem.Allocator, config: *const config_mod.Config, route: []const u8, username: []const u8, title: []const u8, content: []const u8, status: u16) !http.Response {
    const body = content;
    const rendered = try templates.renderShell(allocator, route, body, username, true, config.mikareno_mcp_url);
    const page_title = try std.fmt.allocPrint(allocator, "<title>{s} — NOVA Office</title>", .{try security.htmlEscape(allocator, title)});
    const shell = try std.mem.replaceOwned(u8, allocator, rendered, "<title>NOVA Nébula · Tu espacio académico</title>", page_title);
    return .{ .status = status, .content_type = "text/html; charset=utf-8", .body = shell };
}
fn errorPage(allocator: std.mem.Allocator, config: *const config_mod.Config, route: []const u8, username: []const u8, status: u16, message: []const u8) !http.Response {
    const escaped = try security.htmlEscape(allocator, message);
    return page(allocator, config, route, username, "NOVA Office", try std.fmt.allocPrint(allocator, "<p role=\"alert\">{s}</p>", .{escaped}), status);
}
fn hxRedirect(allocator: std.mem.Allocator, location: []const u8) !http.Response {
    const headers = try allocator.alloc(http.Header, 1);
    headers[0] = .{ .name = "HX-Redirect", .value = location };
    return .{ .status = 200, .content_type = "text/html; charset=utf-8", .headers = headers };
}
fn download(allocator: std.mem.Allocator, mime: []const u8, body: []const u8, filename: []const u8) !http.Response {
    const headers = try allocator.alloc(http.Header, 1);
    headers[0] = .{ .name = "Content-Disposition", .value = try std.fmt.allocPrint(allocator, "attachment; filename=\"{s}\"", .{filename}) };
    return .{ .status = 200, .content_type = mime, .body = body, .headers = headers };
}
fn jsonErrorWithReason(allocator: std.mem.Allocator, status: u16, message: []const u8, reason: []const u8) !http.Response {
    return .{ .status = status, .content_type = "application/json; charset=utf-8", .body = try json.stringify(allocator, try json.object(allocator, &.{ .{ .key = "error", .value = .{ .string = message } }, .{ .key = "motivo", .value = .{ .string = reason } } })) };
}
fn methodNotAllowed(allocator: std.mem.Allocator) !http.Response {
    return .{ .status = 405, .content_type = "application/json; charset=utf-8", .body = try json.stringify(allocator, try json.object(allocator, &.{.{ .key = "error", .value = .{ .string = "Method not allowed" } }})) };
}
fn timestampIsoUtc(allocator: std.mem.Allocator) ![]const u8 {
    const now = std.time.timestamp();
    const epoch = std.time.epoch.EpochSeconds{ .secs = @intCast(@max(0, now)) };
    const year_day = epoch.getEpochDay().calculateYearDay();
    const month_day = year_day.calculateMonthDay();
    const day_seconds = epoch.getDaySeconds();
    return std.fmt.allocPrint(allocator, "{d:0>4}-{d:0>2}-{d:0>2}T{d:0>2}:{d:0>2}:{d:0>2}Z", .{ year_day.year, month_day.month.numeric(), month_day.day_index + 1, day_seconds.getHoursIntoDay(), day_seconds.getMinutesIntoHour(), day_seconds.getSecondsIntoMinute() });
}

fn timestampForFile(allocator: std.mem.Allocator) ![]const u8 {
    const now = std.time.timestamp();
    const epoch = std.time.epoch.EpochSeconds{ .secs = @intCast(@max(0, now)) };
    const year_day = epoch.getEpochDay().calculateYearDay();
    const month_day = year_day.calculateMonthDay();
    const day_seconds = epoch.getDaySeconds();
    return std.fmt.allocPrint(allocator, "{d:0>4}{d:0>2}{d:0>2}-{d:0>2}{d:0>2}{d:0>2}", .{ year_day.year, month_day.month.numeric(), month_day.day_index + 1, day_seconds.getHoursIntoDay(), day_seconds.getMinutesIntoHour(), day_seconds.getSecondsIntoMinute() });
}

test "office paths reject traversal and slugify safely" {
    try std.testing.expect(!safeSlug("../outside"));
    try std.testing.expect(!safeSlug("a/b"));
    try std.testing.expect(safeSlug("safe-name_2"));
    const slug = try slugify(std.testing.allocator, "My First Document");
    defer std.testing.allocator.free(slug);
    try std.testing.expectEqualStrings("my-first-document", slug);
    const accented = try slugify(std.testing.allocator, "Matemática");
    defer std.testing.allocator.free(accented);
    try std.testing.expectEqualStrings("matemática", accented);
    const separators = try slugify(std.testing.allocator, "A__ B!!");
    defer std.testing.allocator.free(separators);
    try std.testing.expectEqualStrings("a__-b", separators);
    const long_title = try std.testing.allocator.alloc(u8, MAX_TITLE);
    defer std.testing.allocator.free(long_title);
    @memset(long_title, 'a');
    const long_slug = try slugify(std.testing.allocator, long_title);
    defer std.testing.allocator.free(long_slug);
    try std.testing.expect(safeSlug(long_slug));
    try std.testing.expectEqual(@as(usize, MAX_TITLE), long_slug.len);
}

test "CSV parser computes SUM and AVG ranges" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    const grid = try parseCsv(allocator, "1,=SUM(A1:A3)\n2,=AVG(A1:A3)\n3,\n");
    const computed = try computeGrid(allocator, grid);
    try std.testing.expectEqualStrings("6", computed[0][1]);
    try std.testing.expectEqualStrings("2", computed[1][1]);
}

test "markdown renderer escapes HTML and handles headings, emphasis and lists" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const html = try markdownHtml(arena.allocator(), "# Hello\n\n**bold** and *italic*.\n\n- one\n- two\n<script>");
    try std.testing.expect(std.mem.indexOf(u8, html, "<h1>Hello</h1>") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "<strong>bold</strong>") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "&lt;script&gt;") != null);
    try std.testing.expect(std.mem.indexOf(u8, html, "<li>two</li>") != null);
}

test "report renders structured Mikareno grades with source-compatible rounding" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    const academic = try json.parse(allocator, "{\"promedios\":{\"stats\":[{\"courseName\":\"Matemática | Física\",\"courseCode\":\"MAT-01\",\"average\":8.567,\"partialId\":\"2026:T2:P1\"}]}}");
    const averages = try collectReportGradeRows(allocator, academic, true);
    try std.testing.expectEqualStrings("| Matemática \\| Física | MAT-01 | 2 | 8.57 |\n", averages);
    const grades = try collectReportGradeRows(allocator, academic, false);
    try std.testing.expectEqualStrings("| Matemática \\| Física | 8.57 | 2 |\n", grades);
}

test "report distinguishes missing and empty academic sources and renders task rows" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    const academic = try json.parse(allocator, "{\"promedios\":{\"stats\":[]}}");
    try std.testing.expectEqualStrings("", try collectReportGradeRows(allocator, academic, true));

    const tasks = try json.parse(allocator, "{\"lista_tareas\":[{\"title\":\"Proyecto | ciencia\",\"dueDate\":\"2026-11-20\",\"status\":\"PENDING\"}]}");
    var markdown = std.ArrayList(u8).init(allocator);
    try appendTasksSection(allocator, &markdown, tasks);
    try std.testing.expect(std.mem.indexOf(u8, markdown.items, "Proyecto \\| ciencia") != null);
    try std.testing.expect(std.mem.indexOf(u8, markdown.items, "Fuente: MCP Mikareno (get_tareas)") != null);

    var unavailable = std.ArrayList(u8).init(allocator);
    try appendGradeSection(allocator, &unavailable, "Promedios por materia", null, true);
    try std.testing.expect(std.mem.indexOf(u8, unavailable.items, "no disponibles: la fuente académica no respondió") != null);
}
