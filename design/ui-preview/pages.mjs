import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// Datos ficticios exclusivos de la preview. El HTML se extrae de las mismas
// plantillas estáticas y raw strings que utiliza el servidor Rust.
const previewDir = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(previewDir, '../..');
const read = (file) => fs.readFileSync(path.join(repo, file), 'utf8');

function rawContaining(file, needle, hashes = 1) {
  const source = read(file);
  const at = source.indexOf(needle);
  if (at < 0) throw new Error(`No se encontró ${needle} en ${file}`);
  const open = `r${'#'.repeat(hashes)}"`;
  const close = `"${'#'.repeat(hashes)}`;
  const start = source.lastIndexOf(open, at);
  const end = source.indexOf(close, at + needle.length);
  if (start < 0 || end < 0) throw new Error(`Raw string no acotada: ${file} · ${needle}`);
  return source.slice(start + open.length, end);
}

function rawAfter(file, marker, hashes = 1) {
  const source = read(file);
  const at = source.indexOf(marker);
  if (at < 0) throw new Error(`No se encontró ${marker} en ${file}`);
  const open = `r${'#'.repeat(hashes)}"`;
  const close = `"${'#'.repeat(hashes)}`;
  const start = source.indexOf(open, at + marker.length);
  const end = source.indexOf(close, start + open.length);
  if (start < 0 || end < 0) throw new Error(`Raw string no acotada: ${file} · ${marker}`);
  return source.slice(start + open.length, end);
}

// Interpreta únicamente los placeholders de format! y las llaves Rust
// escapadas. Falla de forma explícita si falta un valor de la preview.
function fmt(template, positional = [], named = {}) {
  let out = '';
  let index = 0;
  let sequence = 0;
  while (index < template.length) {
    if (template.startsWith('{{', index)) { out += '{'; index += 2; continue; }
    if (template.startsWith('}}', index)) { out += '}'; index += 2; continue; }
    if (template[index] !== '{') { out += template[index]; index += 1; continue; }
    const end = template.indexOf('}', index + 1);
    if (end < 0) throw new Error('Placeholder de format! sin cerrar');
    const token = template.slice(index + 1, end);
    if (!token) {
      if (sequence >= positional.length) throw new Error(`Falta valor posicional ${sequence}`);
      out += positional[sequence++];
    } else {
      const key = token.split(':')[0];
      if (!Object.hasOwn(named, key)) throw new Error(`Falta valor de format!: ${key}`);
      out += named[key];
    }
    index = end + 1;
  }
  return out;
}

function slots(template, values) {
  let output = template;
  for (const [key, value] of Object.entries(values)) {
    output = output.replaceAll(`<!--SLOT:${key}-->`, String(value));
  }
  const pending = output.match(/<!--SLOT:([A-Z0-9_]+)-->/);
  if (pending) throw new Error(`Slot sin resolver en preview: ${pending[1]}`);
  return output;
}

const badge = (text, tone = 'violet') => `<span class="nova-version-badge" style="color:var(--nova-${tone})">${text}</span>`;
const status = `<div class="nova-page-status" style="width:fit-content;margin-bottom:14px"><span style="color:var(--nova-lime)">●</span> Mikareno conectado · datos de demostración</div>`;
const caption = (number, title, icon = 'folder_open') => `<div class="nova-panel-caption"><span>${number}</span><h2>${title}</h2><span class="material-symbols-outlined">${icon}</span></div>`;

const subjectCards = [
  ['Algoritmos y estructuras de datos', 'ALG-201', '8.70', '87%', '6 actividades · 5 con nota'],
  ['Redes y sistemas distribuidos', 'RED-304', '8.20', '82%', '4 actividades · 4 con nota'],
  ['Sistemas operativos', 'SO-220', '6.70', '67%', '7 actividades · 5 con nota'],
  ['Matemática discreta', 'MAT-110', '—', '0%', 'Sin promedio todavía'],
].map(([name, code, grade, width, activities]) => `<a href="/dashboard/subjects/demo" class="nova-subject-card"><div class="nova-subject-top"><span class="nova-subject-mark"><span class="material-symbols-outlined">menu_book</span></span><span class="nova-version-badge">${code}</span><span class="material-symbols-outlined nova-subject-arrow">arrow_outward</span></div><h3>${name}</h3><div class="nova-subject-grade"><strong class="text-primary-container">${grade}</strong><span>promedio del parcial</span></div><div class="nova-subject-progress"><div class="bg-primary-container" style="width:${width}"></div></div><footer><span>${activities}</span><span>Ver detalle</span></footer></a>`).join('');
const subjectGrid = `<div class="nova-subject-grid">${subjectCards}</div>`;

const weekDays = ['Lunes 19', 'Martes 20', 'Miércoles 21', 'Jueves 22', 'Viernes 23', 'Sábado 24', 'Domingo 25'];
const plannerGrid = `<div class="overflow-x-auto"><div class="nova-planner-grid min-w-[840px] grid grid-cols-7">${weekDays.map((day, index) => `<div class="nova-planner-day ${index === 4 ? 'border-2 border-primary-container' : ''}"><h3>${day}</h3>${index < 5 ? `<ul><li class="nova-task-card"><strong>${['Repaso de grafos', 'Laboratorio de redes', 'Lectura de kernel', 'Ejercicios de lógica', 'Cuestionario semanal'][index]}</strong><p>${index + 1} actividad(es)</p></li></ul>` : '<p class="nova-weekly-empty">Sin tareas</p>'}</div>`).join('')}</div></div>`;
const scheduleGrid = `<div class="overflow-x-auto"><div class="nova-schedule-grid min-w-[840px] grid grid-cols-7">${weekDays.map((day, index) => `<div class="nova-schedule-day ${index === 4 ? 'border-2 border-primary-container' : ''}"><h3>${day}</h3>${index < 5 ? `<article class="nova-task-card"><strong>${index % 2 ? 'Redes' : 'Algoritmos'}</strong><p>08:00 — 09:30</p></article><article class="nova-task-card"><strong>Sistemas operativos</strong><p>10:00 — 11:30</p></article>` : '<p class="nova-weekly-empty">Sin clases</p>'}</div>`).join('')}</div></div>`;

const monthCells = ['Lun', 'Mar', 'Mié', 'Jue', 'Vie', 'Sáb', 'Dom'].map((day) => `<div style="padding:8px;color:var(--nova-muted);font-size:9px;text-align:center">${day}</div>`).join('') + Array.from({ length: 35 }, (_, index) => {
  const day = index - 1;
  const valid = day > 0 && day <= 31;
  return `<div class="calendar-cell relative h-28 border border-outline/20 p-space-xs" style="min-height:92px;padding:9px;background:${day === 23 ? 'rgba(182,162,255,.07)' : 'transparent'}"><span style="font-size:9px;color:${valid ? 'var(--nova-secondary)' : 'var(--nova-muted)'}">${valid ? day : ''}</span>${[3, 8, 15, 23, 28].includes(day) ? `<div class="nova-task-card" style="margin-top:12px">${day === 28 ? 'Examen de sistemas' : day === 23 ? 'Entrega de laboratorio' : 'Clase · 08:00'}</div>` : ''}</div>`;
}).join('');
const monthGrid = `<div id="calendar-nav" class="flex items-center justify-between mb-space-sm"><button hx-get="/dashboard/calendar?partial=grid&amp;month=2026-09" hx-target="#calendar-grid" hx-swap="outerHTML" class="inline-flex items-center justify-center w-8 h-8 rounded-lg" title="Mes anterior">‹</button><span class="font-headline-sm text-headline-sm text-primary-container">Octubre 2026</span><span class="font-label-sm text-[10px] px-2 py-0.5 rounded-full bg-primary-container/20 text-primary-container font-semibold whitespace-nowrap ml-2">Mes actual</span><button hx-get="/dashboard/calendar?partial=grid&amp;month=2026-11" hx-target="#calendar-grid" hx-swap="outerHTML" class="inline-flex items-center justify-center w-8 h-8 rounded-lg" title="Próximo mes">›</button></div><div class="grid grid-cols-7">${monthCells}</div>`;
const upcoming = `<aside class="nova-calendar-upcoming">${caption('02', 'A continuación', 'upcoming')}<p class="nova-card-intro">Eventos registrados en el calendario escolar.</p><div class="nova-exam-group-list" style="margin-top:16px"><article class="nova-exam-card" style="grid-template-columns:1fr"><div class="nova-exam-date"><span class="material-symbols-outlined">event</span>23 oct. · 23:59</div><div class="nova-exam-copy"><h3>Entrega de laboratorio</h3><p>Algoritmos</p></div></article><article class="nova-exam-card" style="grid-template-columns:1fr"><div class="nova-exam-date"><span class="material-symbols-outlined">event</span>28 oct. · 08:30</div><div class="nova-exam-copy"><h3>Examen de sistemas</h3><p>Auditorio central</p></div></article></div></aside>`;
const weeklyColumns = weekDays.map((day, index) => `<section class="nova-weekly-day"><header><span>${day.slice(0, 3)}</span><strong>${day}</strong></header><div>${index < 5 ? `<article><span>Clase</span><h3>${index % 2 ? 'Redes y sistemas' : 'Algoritmos'}</h3><time datetime="2026-10-${19 + index}T08:00" title="${19 + index}/10/2026">08:00</time></article>` : '<p class="nova-weekly-empty">Sin eventos</p>'}</div></section>`).join('');

const examBody = `<div class="nova-exam-summary"><span class="material-symbols-outlined">monitoring</span><p>3 evaluaciones confirmadas · 2 próximas · 1 posible del calendario</p></div><div class="nova-exam-timeline" id="event-timeline"><section class="nova-exam-group"><div class="nova-exam-group-head"><h2>Próximos 7 días</h2><span>2 evaluaciones</span></div><div class="nova-exam-group-list">${[['28 oct. · 08:30', 'Sistemas operativos: concurrencia', 'Auditorio central'], ['30 oct. · 10:00', 'Redes: routing dinámico', 'Laboratorio 3']].map(([date, title, place]) => `<article class="nova-exam-card"><div class="nova-exam-date"><span class="material-symbols-outlined">event</span>${date}</div><div class="nova-exam-copy"><h3>${title}</h3><p>${place}</p></div><div class="nova-exam-badges">${badge('Mikareno', 'lime')}</div></article>`).join('')}</div></section></div>`;

const lifeCards = ['Asistencia', 'Puntualidad', 'Uniforme', 'Comportamiento', 'Tareas', 'Biblioteca'].map((title, index) => `<details class="nova-life-card" id="sl-${index}" ${index === 0 ? 'open' : ''}><summary class="nova-life-summary-row"><div><span class="nova-eyebrow">0${index + 1}</span><h2>${title}</h2></div><span class="material-symbols-outlined">expand_more</span></summary><div class="nova-life-body"><p>${index === 0 ? '96 % de asistencia registrada en el periodo actual.' : index === 4 ? '2 tareas pendientes con vencimiento esta semana.' : 'Sin incidencias registradas en esta sección.'}</p><button class="nova-button nova-button-quiet" hx-get="/dashboard/vida-escolar?partial=section&id=${index}" hx-target="#sl-${index}">Actualizar</button></div></details>`).join('');

const profileIdentity = `<section class="nova-profile-identity" id="profile-section"><div class="nova-profile-avatar">A</div><div class="nova-profile-identity-copy"><p>PERFIL DEL ESTUDIANTE</p><h2>Alex Loayza</h2><span>Unidad Educativa Nova</span><strong>Ingeniería · 3.º B</strong></div><button class="nova-icon-button" hx-get="/dashboard/profile?partial=profile" hx-target="#profile-section" hx-swap="outerHTML" aria-label="Actualizar perfil"><span class="material-symbols-outlined">refresh</span></button></section>`;
const profileFacts = `<section class="nova-profile-facts" id="profile-details">${caption('01', 'Datos académicos', 'badge')}<div class="nova-profile-fact-grid"><article><span>Nombre completo</span><strong>Alex Loayza</strong></article><article><span>Institución</span><strong>Unidad Educativa Nova</strong></article><article><span>Curso actual</span><strong>Ingeniería · 3.º B</strong></article><article><span>Estado del MCP</span><strong style="color:var(--nova-lime)">Conectado</strong></article></div></section>`;
const profileProgress = `<section class="nova-profile-progress">${caption('02', 'Progreso por materia', 'monitoring')}<p class="nova-profile-progress-intro">Promedios reales del parcial en curso.</p><div id="profile-progress">${[['Algoritmos', '8.70', 87], ['Redes', '8.20', 82], ['Sistemas operativos', '6.70', 67]].map(([name, grade, value]) => `<div class="nova-profile-progress-row"><div style="display:flex;justify-content:space-between"><span>${name}</span><strong>${grade}</strong></div><div class="nova-subject-progress" role="progressbar" aria-valuenow="${value}" aria-valuemin="0" aria-valuemax="100"><div class="bg-primary-container" style="width:${value}%"></div></div><small>${value}% · Mikareno</small></div>`).join('')}</div></section>`;

const fileCard = (kind, title, icon, href, meta) => `<article class="nova-file-card ${kind}"><div class="nova-file-card-top"><span class="nova-file-type"><span class="material-symbols-outlined">${icon}</span></span><span>LOCAL</span></div><h2><a href="${href}">${title}</a></h2><p class="nova-file-meta">${meta}</p><footer><a href="${href}" class="nova-text-link">Abrir<span class="material-symbols-outlined">arrow_forward</span></a><button class="nova-icon-button danger" aria-label="Borrar ${title}"><span class="material-symbols-outlined">delete</span></button></footer></article>`;
const docCards = fileCard('documents', 'apuntes-de-algoritmos', 'description', '/office/docs/demo', '23 oct. 2026 · 4,2 KB') + fileCard('documents', 'ensayo-de-redes', 'description', '/office/docs/demo', '21 oct. 2026 · 2,8 KB');
const sheetCards = fileCard('sheet', 'presupuesto-del-trimestre', 'table_chart', '/office/sheets/demo', '23 oct. 2026 · 1,4 KB');
const presentationCards = fileCard('presentation', 'sistemas-operativos', 'present_to_all', '/office/presentations/demo', '5 diapositivas · 3,1 KB');

export function createPages(dashboardContent) {
  const pages = new Map();
  const add = (route, content) => pages.set(route, content);
  add('/', dashboardContent);
  add('/dashboard', dashboardContent);

  add('/dashboard/material', slots(read('static/stitch-v2/_content/clean/material.html'), {
    AI_DOT: '<span style="color:var(--nova-lime)">●</span>', AI_MODEL: 'MiniMax M3 · conectado',
    MATERIA_OPTIONS: '<option>Algoritmos</option><option>Redes</option><option>Sistemas operativos</option>',
    MCP_STATUS: badge('Mikareno disponible', 'lime'),
    INITIAL_HINT: '<div class="nova-chat-welcome"><span class="material-symbols-outlined">auto_awesome</span><h2>Tu material aparecerá aquí.</h2><p>Completa el brief y genera una primera versión.</p></div>',
  }));

  add('/settings', slots(read('static/stitch-v2/_content/clean/settings.html'), {
    VERSION: '3.0', AUTH_MODE: 'sesión local', MCP_DOT: '<span style="color:var(--nova-lime)">●</span>',
    MCP_STATUS: 'Conectado y respondiendo', MCP_URL: 'http://mikareno.local/mcp', AI_DOT: '<span style="color:var(--nova-lime)">●</span>',
    AI_STATUS: badge('OpenRouter configurado', 'lime'), AI_MODEL: 'minimax/minimax-m2', AI_BUDGET: '$25.00', AI_MAX_TOKENS: '4096',
    AI_FORM: '<form><label>Modelo<input value="minimax/minimax-m2" readonly></label><label>Tokens máximos<input value="4096" readonly></label><label>Presupuesto<input value="25" readonly></label><button type="button">Parámetros administrados en .env</button></form>',
    AI_ENV: '<p class="nova-privacy-note">OPENROUTER_API_KEY · configurada en el servidor</p>', AI_DOT2: '<span style="color:var(--nova-lime)">●</span>', AI_BADGE: 'conectada',
  }));

  add('/dashboard/ai', slots(read('static/stitch-v2/_content/clean/asistente.html'), {
    CHAT_FCOUNT: '2', CHAT_FILTERS: '<button type="button">Algoritmos</button><button type="button">Redes</button>',
    CHAT_SESSIONS: '<div class="conversation-item"><strong>Repaso de concurrencia</strong><p>¿Me explicas los semáforos?</p></div><div class="conversation-item"><strong>Routing dinámico</strong><p>Comparación entre RIP y OSPF.</p></div>',
    CHAT_MODE_DOT: '<span style="color:var(--nova-lime)">●</span>', CHAT_MODEL: 'MiniMax M3', CHAT_QUOTA_PCT: '12 %',
    CHAT_QUOTA_BAR: '<div style="width:12%;height:100%;background:var(--nova-violet)"></div>', CHAT_COSTS: '$0.42 / $25.00', CHAT_MODE: 'Conectado',
    CHAT_CONTEXT: 'Sistemas operativos',
    CHAT_MESSAGES: '<div class="nova-chat-welcome"><span class="material-symbols-outlined">auto_awesome</span><h2>Piensa en voz alta.</h2><p>Pregunta por tus materias o retoma una conversación.</p></div>',
    CHAT_COMPOSER_MODE: 'Respuesta detallada', CHAT_SOURCES: '<article class="nova-task-card">Apuntes de concurrencia</article><article class="nova-task-card">Guía de semáforos</article>',
  }));

  add('/ai', rawContaining('src/ai/handlers.rs', 'nova-conversations-page', 2)
    .replaceAll('__MODEL__', 'minimax-m2').replaceAll('__MODE_COLOR__', 'text-green-400')
    .replaceAll('__MODE_LABEL__', 'Conectado').replaceAll('__MODE_DOT__', 'bg-green-400'));

  const graph = '<svg viewBox="0 0 220 190" aria-label="Grafo de notas"><g stroke="rgba(182,162,255,.25)"><line x1="110" y1="95" x2="45" y2="45"/><line x1="110" y1="95" x2="180" y2="50"/><line x1="110" y1="95" x2="55" y2="155"/><line x1="110" y1="95" x2="175" y2="150"/></g><g fill="#b6a2ff"><circle cx="110" cy="95" r="13"/><circle cx="45" cy="45" r="7"/><circle cx="180" cy="50" r="7"/><circle cx="55" cy="155" r="7"/><circle cx="175" cy="150" r="7"/></g></svg>';
  add('/dashboard/notes', slots(read('static/stitch-v2/_content/clean/notes.html'), {
    VAULT_PATH: '~/vault', NOTES_COUNT: '12', NOTES_LIST: '<a class="nova-task-card" style="display:block">Concurrencia y semáforos</a><a class="nova-task-card" style="display:block">Routing dinámico</a><a class="nova-task-card" style="display:block">HeapSort en Rust</a>',
    NOTES_RECENT: '<p>Concurrencia · hace 12 min</p><p>Routing · ayer</p>', TAGS: '<span class="nova-version-badge">#sistemas</span><span class="nova-version-badge">#redes</span>',
    NOTE_TITLE: 'Concurrencia y semáforos', NOTE_STATUS: badge('Guardado', 'lime'), NOTE_TAGS: '<span class="nova-version-badge">#sistemas</span>',
    NOTE_STATS: '184 palabras · 1.240 caracteres', NOTE_RAW: '<pre># Concurrencia y semáforos\n\nUn **semáforo** coordina el acceso a recursos compartidos.</pre>',
    NOTE_PREVIEW: '<article class="prose"><h1>Concurrencia y semáforos</h1><p>Un <strong>semáforo</strong> coordina el acceso a recursos compartidos.</p><h2>Ideas clave</h2><ul><li>Exclusión mutua</li><li>Sincronización</li><li>Productor y consumidor</li></ul></article>',
    BACKLINKS_COUNT: '2', BACKLINKS: '<a class="nova-task-card">Sistemas operativos</a><a class="nova-task-card">Productor y consumidor</a>',
    GRAPH_META: '5 nodos · 4 conexiones', GRAPH: graph, OUTLINKS_COUNT: '3', OUTLINKS: '<a class="nova-task-card" style="display:block">Exclusión mutua</a><a class="nova-task-card" style="display:block">Procesos</a>', GRAPH_STATS: 'Densidad 0.40 · 2 notas huérfanas',
  }));

  add('/dashboard/mcp', slots(read('static/stitch-v2/_content/clean/mcp.html'), {
    MCP_ORCHESTRATOR: '<span>Orquestador</span><strong>3 servicios</strong>', MCP_LATENCY: '42 ms', MCP_SPEND: '$0.42',
    MCP_SERVERS_LINE: '3 servicios registrados · 2 externos · 1 nativo', MCP_STATUS_MIKARENO: badge('Conectado', 'lime'), MCP_URL_MIKARENO: 'mikareno.local/mcp', MCP_TOOLS: '18 tools',
    MCP_STATUS_GENOFFICE: badge('Conectado', 'lime'), MCP_URL_GENOFFICE: 'genoffice.local/mcp', MCP_NATIVE_LOCATION: badge('Integrado', 'lime'), MCP_NATIVE_TOOLS: '8 tools',
    MCP_STATUS_AI: '<span id="live-connection-tag" class="nova-version-badge">● API activa</span>', MCP_KEY_STATE: badge('Configurada', 'lime'),
    MCP_MODEL_OPTIONS: '<option>minimax/minimax-m2</option><option>openrouter/auto</option>', MCP_BUDGET: '$25.00 USD', MCP_BUDGET_INT: '25', MCP_MODEL: 'minimax-m2',
    MCP_TRANSCRIPT: '<div class="nova-chat-welcome"><span class="material-symbols-outlined">terminal</span><p>Registro de pruebas local.</p></div>',
    MCP_TELEMETRY: '<div class="nova-admin-metrics"><article><div><small>Solicitudes</small><strong>128</strong><p>este mes</p></div></article><article><div><small>Latencia media</small><strong>42 ms</strong><p>últimas 24 h</p></div></article><article><div><small>Disponibilidad</small><strong>99,8 %</strong><p>local</p></div></article></div>',
  }));

  add('/admin', slots(rawContaining('src/handlers/admin.rs', 'nova-admin-page', 2), {
    ADMIN_VERSION: '3.0.0', ADMIN_UPTIME: '4 d 06 h', ADMIN_UPTIME_SECS: '367200', ADMIN_NOTES_COUNT: '12', ADMIN_CHATS_COUNT: '38',
    ADMIN_SPEND: '$0.42', ADMIN_BUDGET: '$25.00', ADMIN_REQUESTS: '128', ADMIN_BUDGET_BAR: '<div class="nova-quota-track"><div style="width:2%;height:100%;background:var(--nova-violet)"></div></div>', ADMIN_BUDGET_PCT: '2',
    ADMIN_LAST_BACKUP_DATE: '23/10/2026 03:00', ADMIN_LAST_BACKUP_PATH: 'data/backups/app-demo.db', ADMIN_LAST_BACKUP_NAME: 'app-demo.db · 2,4 MB', ADMIN_LAST_BACKUP_MS: '1792713600000', ADMIN_NEXT_BACKUP: 'en 6 h 12 min', ADMIN_BACKUP_COUNT: '4',
    ADMIN_BACKUP_ROWS: '<tr><td>app-demo.db</td><td>23/10/2026 03:00</td><td>2,4 MB</td><td><a class="nova-text-link" href="#">Descargar</a></td></tr>',
    ADMIN_MCP_ICON: 'cloud_done', ADMIN_MCP_STATUS: 'Conectado', ADMIN_MCP_LATENCY: '<code>42 ms</code>', ADMIN_OR_ICON: 'auto_awesome', ADMIN_OR_STATUS_COLOR: 'text-green-400', ADMIN_OR_STATUS: 'Conectado', ADMIN_OR_MODEL: 'minimax-m2',
  }));

  add('/dashboard/subjects', fmt(rawContaining('src/academic/subjects.rs', 'nova-subjects-page', 3), ['Parcial 2 · 4 asignaturas · datos reales de Mikareno', status, subjectGrid]));
  add('/dashboard/planner', fmt(rawContaining('src/academic/planner.rs', 'nova-planner-bottom', 3), [], {
    banner: status, nav: '<div class="nova-planner-nav"><a href="?week=2026-10-12">‹ Anterior</a><strong>19 — 25 de octubre</strong><a href="?week=2026-10-26">Siguiente ›</a></div>', grid: plannerGrid,
    resumen_html: `<section>${caption('02', 'Resumen de tareas')}<p class="nova-card-intro">5 tareas con vencimiento esta semana.</p></section>`,
    otras_html: `<section>${caption('03', 'Más adelante')}<p class="nova-card-intro">Proyecto final de redes · 2 nov.</p></section>`, sin_fecha_html: `<section>${caption('04', 'Sin fecha')}<p class="nova-card-intro">No hay tareas sin fecha legible.</p></section>`,
  }));
  add('/dashboard/calendar', fmt(rawContaining('src/academic/calendar.rs', 'nova-calendar-page', 3), ['2026-10', status, monthGrid, upcoming]));
  add('/dashboard/calendar/daily', fmt(rawContaining('src/academic/calendar.rs', 'nova-daily-page', 3), ['Agenda diaria', 'Viernes', '23 de octubre de 2026', badge('Hoy', 'lime'), status, '<nav class="nova-daily-nav"><a class="nova-button nova-button-quiet" href="?date=2026-10-22">‹ Jueves</a><strong>Viernes 23</strong><a class="nova-button nova-button-quiet" href="?date=2026-10-24">Sábado ›</a></nav>', '', '<div id="daily-events" class="nova-exam-group-list"><article class="nova-exam-card"><div class="nova-exam-date">08:00</div><div class="nova-exam-copy"><h3>Algoritmos</h3><p>Clase · Aula 204</p></div></article><article class="nova-exam-card"><div class="nova-exam-date">23:59</div><div class="nova-exam-copy"><h3>Entrega de laboratorio</h3><p>Tarea pendiente</p></div></article></div>']));
  add('/dashboard/calendar/weekly', fmt(rawContaining('src/academic/calendar.rs', 'nova-weekly-page', 3), [status, weeklyColumns]));
  add('/dashboard/examenes', fmt(rawContaining('src/academic/exams.rs', 'nova-exams-page', 3), [], { status, aviso_html: '', body: examBody }));
  add('/dashboard/horario', fmt(rawContaining('src/handlers/schedule.rs', 'nova-schedule-page', 1), ['19 — 25 de octubre de 2026', status, scheduleGrid]));
  add('/dashboard/vida-escolar', fmt(rawContaining('src/academic/school_life.rs', 'nova-life-page', 3), [], { resumen: '<div class="nova-life-overview"><p>6 secciones con datos · actualización manual disponible</p></div>', banner: status, aviso: '', accordion: lifeCards }));

  add('/dashboard/search', fmt(rawContaining('src/handlers/search.rs', 'nova-search-page', 1), [], {
    badge: badge('3 resultados'), form: '<form method="get" action="/dashboard/search" class="nova-search-form"><span class="material-symbols-outlined">search</span><label for="search-input" class="sr-only">Buscar notas</label><input id="search-input" type="search" name="q" value="concurrencia"><button type="submit" class="nova-button nova-button-primary">Buscar<span class="material-symbols-outlined">arrow_forward</span></button></form>',
    body: '<ul class="nova-search-results"><li class="nova-search-result"><span class="nova-search-result-icon"><span class="material-symbols-outlined">description</span></span><div><a href="/dashboard/notes">Concurrencia y semáforos</a><p>Un semáforo coordina el acceso a recursos compartidos…</p></div><a href="/dashboard/notes" class="nova-icon-button" aria-label="Abrir"><span class="material-symbols-outlined">arrow_outward</span></a></li><li class="nova-search-result"><span class="nova-search-result-icon"><span class="material-symbols-outlined">description</span></span><div><a href="/dashboard/notes">Productor y consumidor</a><p>Problema clásico de sincronización entre procesos…</p></div></li></ul>',
  }));
  add('/dashboard/profile', fmt(rawContaining('src/academic/profile.rs', 'nova-profile-page', 3), [profileIdentity, status, profileFacts, profileProgress]));

  const detailHeader = fmt(rawContaining('src/academic/subject_detail.rs', 'nova-subject-detail-hero', 3), ['Sistemas operativos', 'SO-220', '2', '<a class="nova-link" href="/dashboard/notes">Nota de la asignatura</a>']);
  const detailSummary = fmt(rawContaining('src/academic/subject_detail.rs', 'nova-subject-summary', 3), ['text-primary-container', '6.70', '10.0', 'bg-primary-container', '67', '6.70', 'SO-220', '10.0', 'No', 'Cuantitativa']);
  const detailRows = `<tr><td>Laboratorio de procesos</td><td>18/10/2026</td><td>Práctica 4</td><td>8.50</td><td>${badge('Calificada', 'lime')}</td><td>${badge('Con nota', 'lime')}</td></tr><tr><td>Examen de concurrencia</td><td>28/10/2026</td><td>Parcial 2</td><td>—</td><td>${badge('Pendiente')}</td><td>Sin nota</td></tr>`;
  const detailActivities = fmt(rawContaining('src/academic/subject_detail.rs', 'nova-subject-activities', 3), ['Evaluaciones y tareas', '7', '5', '8.20', '2 de 7 enlazadas a notas', detailRows]);
  add('/dashboard/subjects/demo', `<div class="nova-page nova-subject-detail-page">${status}${detailHeader}${detailSummary}<aside class="nova-subject-observation"><span class="material-symbols-outlined">sticky_note_2</span><div><h2>Observación</h2><p>Repasar sincronización antes del próximo parcial.</p></div></aside>${detailActivities}</div>`);
  add('/dashboard/subjects/missing', fmt(rawContaining('src/academic/subject_detail.rs', 'nova-subject-missing', 3), [], { status, code: 'NO-404' }));

  add('/office', fmt(rawContaining('src/office/documents.rs', 'nova-office-home', 1), [], { docs: '2', sheets: '1', pres: '1', mcp: '' }));
  add('/office/docs', fmt(rawContaining('src/office/documents.rs', 'nova-office-page', 1), [], { count: '2', cards: docCards, mcp: '' }));
  add('/office/sheets', fmt(rawContaining('src/office/spreadsheets.rs', 'nova-office-page', 1), [], { cards: sheetCards }));
  add('/office/presentations', fmt(rawContaining('src/office/presentations.rs', 'nova-office-page', 1), [], { cards: presentationCards }));

  const toolbarJs = rawAfter('src/office/templates/mod.rs', 'pub const MARKDOWN_TOOLBAR_JS', 1);
  add('/office/docs/new', `${rawContaining('src/office/documents.rs', 'nova-editor-form', 1)}${toolbarJs}</script>`);
  const docEditor = fmt(rawContaining('src/office/documents.rs', 'nova-markdown-toolbar', 1), [], { slug: 'demo', content: '# Apuntes de algoritmos\n\n## HeapSort\n\nLa ordenación por montículos tiene complejidad **O(n log n)**.', js: toolbarJs });
  add('/office/docs/demo', fmt(rawContaining('src/office/documents.rs', 'nova-document-workspace', 1), [], { saved: '', mcp: '', title: 'apuntes de algoritmos', slug: 'demo', editor: docEditor }));
  add('/office/docs/demo/preview', fmt(rawContaining('src/office/documents.rs', 'nova-preview-page', 1), [], { mcp: '', title: 'apuntes de algoritmos', slug: 'demo', html: '<h1>Apuntes de algoritmos</h1><h2>HeapSort</h2><p>La ordenación por montículos tiene complejidad <strong>O(n log n)</strong>.</p><pre><code>heapify(values)</code></pre>' }));
  add('/office/sheets/new', rawContaining('src/office/spreadsheets.rs', 'nova-create-sheet-form', 1));
  add('/office/presentations/new', rawContaining('src/office/presentations.rs', 'nova-create-sheet-form', 1));

  const editable = `<table id="grid" class="nova-sheet-grid"><tbody>${Array.from({ length: 5 }, (_, row) => `<tr>${Array.from({ length: 5 }, (_, col) => `<td contenteditable="true" data-r="${row}" data-c="${col}">${row === 0 ? ['Concepto', 'Horas', 'Nota', 'Total', 'Media'][col] : row === 1 ? ['Examen', '2', '8.5', '=SUM(C2:C3)', '=AVG(C2:C3)'][col] : row === 2 ? ['Laboratorio', '3', '9.0', '', ''][col] : ''}</td>`).join('')}</tr>`).join('')}</tbody></table>`;
  const values = '<table class="nova-sheet-values"><tbody><tr><td>Examen</td><td>8.5</td><td>17.5</td><td>8.75</td></tr><tr><td>Laboratorio</td><td>9.0</td><td></td><td></td></tr></tbody></table>';
  const gridJs = rawAfter('src/office/templates/mod.rs', 'pub const GRID_SERIALIZE_JS', 1);
  const sheetGrid = fmt(rawContaining('src/office/spreadsheets.rs', 'nova-sheet-grid-head', 1), [], { slug: 'demo', editable, values, header: 'A B C D E', grid_js: gridJs });
  add('/office/sheets/demo', fmt(rawContaining('src/office/spreadsheets.rs', 'nova-sheet-page', 1), [], { title: 'presupuesto del trimestre', slug: 'demo', grid: sheetGrid }));

  const slideCards = ['Concurrencia', 'Semáforos', 'Productor y consumidor'].map((title, index) => `<article class="nova-slide-card slide" id="slide-${index}" data-idx="${index}"><header><span>DIAPOSITIVA ${index + 1}</span><div><button class="nova-icon-button" type="button" onclick="moveSlide(${index},-1)" aria-label="Subir"><span class="material-symbols-outlined">arrow_upward</span></button><button class="nova-icon-button" type="button" onclick="moveSlide(${index},1)" aria-label="Bajar"><span class="material-symbols-outlined">arrow_downward</span></button></div></header><form hx-put="/office/presentations/demo/slides/${index}"><label>Título</label><input name="title" value="${title}"><label>Contenido Markdown</label><textarea name="content" rows="6">## ${title}\n\nIdea principal de la diapositiva.</textarea><footer><span>Markdown</span><button class="nova-button nova-button-quiet" type="submit">Guardar</button></footer></form></article>`).join('');
  add('/office/presentations/demo', fmt(rawContaining('src/office/presentations.rs', 'nova-deck-page', 1), [], { title: 'Sistemas operativos', slug: 'demo', cards: slideCards }));
  const presentSlides = ['Concurrencia', 'Semáforos', 'Productor y consumidor'].map((title, index) => `<section class="nova-present-slide"><div class="nova-present-slide-number">${index + 1}</div><div><h2>${title}</h2><div class="nova-present-slide-body"><p>Coordinación de procesos y acceso seguro a los recursos compartidos.</p><ul><li>Exclusión mutua</li><li>Sincronización</li><li>Prevención de interbloqueos</li></ul></div></div></section>`).join('');
  add('/office/presentations/demo/present', fmt(rawContaining('src/office/presentations.rs', 'nova-present-mode', 1), [], { title: 'Sistemas operativos', slug: 'demo', n: '3', slides: presentSlides }));

  const libraryRows = [
    ['description', 'apuntes-de-algoritmos', 'documento', '4,2 KB', '23 oct.', 'docs'],
    ['table_chart', 'presupuesto-del-trimestre', 'hoja', '1,4 KB', '22 oct.', 'sheets'],
    ['present_to_all', 'sistemas-operativos', 'presentación', '3,1 KB', '21 oct.', 'presentations'],
  ].map(([icon, name, kind, size, modified, type]) => `<tr><td data-label="Archivo"><a class="nova-library-file" href="/office/library/${type}/demo/download"><span class="material-symbols-outlined">${icon}</span><span>${name}</span></a></td><td data-label="Tipo"><span class="nova-file-kind">${kind}</span></td><td data-label="Tamaño">${size}</td><td data-label="Modificado">${modified}</td><td><div class="nova-library-actions"><a class="nova-icon-button" href="/office/library/${type}/demo/download" aria-label="Descargar ${name}"><span class="material-symbols-outlined">download</span></a><form hx-post="/office/library/${type}/demo/delete"><button type="submit" class="nova-icon-button danger" aria-label="Borrar ${name}"><span class="material-symbols-outlined">delete</span></button></form></div></td></tr>`).join('');
  add('/office/library', fmt(rawContaining('src/office/library.rs', 'nova-library-page', 1), [], { count: '4', q: '', rows: libraryRows }));
  add('/office/library/search', pages.get('/office/library'));

  return pages;
}
