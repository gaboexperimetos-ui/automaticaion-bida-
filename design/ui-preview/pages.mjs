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

const badge = (text, tone = 'violet') => `<span class="nx-badge" style="color:var(--nova-${tone})">${text}</span>`;
const status = `<div class="nx-page-status" style="width:fit-content;margin-bottom:14px"><span style="color:var(--nova-lime)">●</span> Mikareno conectado · datos de demostración</div>`;
const caption = (number, title, icon = 'folder_open') => `<div class="nx-panel-head"><span class="nx-eyebrow">${number}</span><h2>${title}</h2><span class="material-symbols-outlined">${icon}</span></div>`;

const subjectCards = [
  ['Algoritmos y estructuras de datos', 'ALG-201', '8.70', '87%', '6 actividades · 5 con nota'],
  ['Redes y sistemas distribuidos', 'RED-304', '8.20', '82%', '4 actividades · 4 con nota'],
  ['Sistemas operativos', 'SO-220', '6.70', '67%', '7 actividades · 5 con nota'],
  ['Matemática discreta', 'MAT-110', '—', '0%', 'Sin promedio todavía'],
].map(([name, code, grade, width, activities]) => `<a href="/dashboard/subjects/demo" class="nx-subject-card"><div class="nx-subject-top"><span class="nx-subject-mark"><span class="material-symbols-outlined">menu_book</span></span><span class="nx-badge">${code}</span><span class="material-symbols-outlined nx-subject-arrow">arrow_outward</span></div><h3>${name}</h3><div class="nx-subject-grade"><strong class="text-primary-container">${grade}</strong><span>promedio del parcial</span></div><div class="nx-subject-progress"><div class="bg-primary-container" style="width:${width}"></div></div><footer><span>${activities}</span><span>Ver detalle<span class="material-symbols-outlined">arrow_forward</span></span></footer></a>`).join('');
const subjectGrid = `<div class="nx-subject-grid">${subjectCards}</div>`;

const weekDays = ['Lunes 19', 'Martes 20', 'Miércoles 21', 'Jueves 22', 'Viernes 23', 'Sábado 24', 'Domingo 25'];
const plannerGrid = `<div class="overflow-x-auto"><div class="nova-planner-grid min-w-[840px] grid grid-cols-7">${weekDays.map((day, index) => `<div class="nova-planner-day ${index === 4 ? 'border-2 border-primary-container' : ''}"><h3>${day}</h3>${index < 5 ? `<ul><li class="nova-task-card"><strong>${['Repaso de grafos', 'Laboratorio de redes', 'Lectura de kernel', 'Ejercicios de lógica', 'Cuestionario semanal'][index]}</strong><p>${index + 1} actividad(es)</p></li></ul>` : '<p class="nova-weekly-empty">Sin tareas</p>'}</div>`).join('')}</div></div>`;
const personalScheduleHours = ['07:35 - 08:15', '08:15 - 08:35', '08:35 - 09:15', '09:15 - 09:55', '09:55 - 10:35', '10:35 - 11:05', '11:05 - 11:45', '11:45 - 12:25', '12:25 - 13:05', '13:05 - 13:45'];
const personalScheduleDays = [
  ['Religión', 'Tutoría', 'Química', 'Química', 'Inglés', 'Recreo', 'CC.NN', 'Dibujo T', 'Robótica', 'Robótica'],
  ['Inglés', 'Tutoría', 'CC.NN', 'CC.NN', 'EE.FF', 'Recreo', 'Sociales', 'Sociales', 'Science', 'Inglés'],
  ['Matemática', 'Tutoría', 'Física', 'Física', 'Lenguaje', 'Recreo', 'Matemática', 'Matemática', 'Inglés', 'Religión'],
  ['Club', 'Club', 'Science', 'Tutoría', 'Inglés', 'Recreo', 'Sociales', 'Sociales', 'Lenguaje', 'Lenguaje'],
  ['EE.FF', 'Tutoría', 'Inglés', 'Inglés', 'Francés', 'Recreo', 'Lenguaje', 'Matemática', 'Plan lector', 'Matemática'],
];
const scheduleGrid = `<div class="overflow-x-auto"><div class="nova-schedule-grid min-w-[840px] grid grid-cols-7 gap-px bg-outline/40 border border-outline rounded-lg overflow-hidden">${weekDays.map((day, index) => `<div class="nova-schedule-day ${index === 4 ? 'border-2 border-primary-container/60 ' : ''}bg-surface-container rounded-lg p-space-sm min-h-[120px]"><h3 class="font-headline-sm text-headline-sm text-primary-container font-semibold">${day}</h3>${index < 5 ? `<ul class="mt-space-xs">${personalScheduleHours.map((time, slot) => `<li class="font-body-sm text-body-sm text-on-surface-variant py-1 border-b border-outline/30 last:border-b-0">${time} — ${personalScheduleDays[index][slot]}</li>`).join('')}</ul>` : '<p class="font-label-md text-on-surface-variant mt-space-xs">Sin clases</p>'}</div>`).join('')}</div></div>`;
const scheduleSource = '<nav class="nx-schedule-sources" aria-label="Fuente del horario"><a class="nx-schedule-source is-active" href="/dashboard/horario" aria-current="page">Mi horario</a><a class="nx-schedule-source" href="/dashboard/horario?fuente=mikareno">Mikareno</a></nav><div class="nx-schedule-personal"><span class="material-symbols-outlined">verified</span><div><strong>Unidad Educativa Álamos · 10.º</strong><p>Año lectivo 2026–2027 · Prof. Santiago Pasquel · Transcrito de tu imagen.</p></div></div>';

const calendarWeekdays = ['Lun', 'Mar', 'Mié', 'Jue', 'Vie', 'Sáb', 'Dom'];
const monthCells = calendarWeekdays.map((day) => `<div class="calendar-weekday">${day}</div>`).join('') + Array.from({ length: 42 }, (_, index) => {
  const day = index - 2;
  const valid = day >= 1 && day <= 31;
  const shownDay = valid ? day : index < 3 ? 28 + index : day - 31;
  const hasSchedule = valid && index % 7 < 5;
  const special = { 23: ['task', 'Entrega de laboratorio'], 28: ['event', 'Examen de sistemas'], 30: ['event', 'Reunión de tutoría'] }[day];
  const total = valid ? Number(hasSchedule) + Number(Boolean(special)) : 0;
  const chips = valid ? `${special ? `<div class="calendar-chip calendar-chip-${special[0]}" title="${special[1]}">${special[1]}</div>` : ''}${hasSchedule ? '<div class="calendar-chip calendar-chip-class" title="Horario personal">10 bloques · 07:35–13:45</div>' : ''}` : '';
  if (!valid) return `<div class="calendar-outside-cell agenda-outside"><span class="calendar-day-number">${shownDay}</span></div>`;
  return `<div class="calendar-cell agenda-cell relative h-28 ${day === 24 ? 'border-2 border-primary-container/60' : 'border border-outline/40'}" data-date="2026-10-${String(day).padStart(2, '0')}" data-total="${total}" tabindex="0" role="button" aria-label="Añadir o editar anotaciones del 2026-10-${String(day).padStart(2, '0')}"><span class="calendar-day-number">${day}</span><div class="calendar-day-items">${chips}</div></div>`;
}).join('');
const monthGrid = `<div id="calendar-nav" class="agenda-nav"><button hx-get="/dashboard/calendar?partial=grid&amp;month=2026-09" hx-target="#calendar-grid" hx-swap="outerHTML" class="calendar-nav-button" aria-label="Mes anterior"><span class="material-symbols-outlined">chevron_left</span><span class="calendar-nav-label">Anterior</span></button><div class="calendar-month-heading"><span class="calendar-month-label">Octubre 2026</span><span class="calendar-current-month">Mes actual</span></div><button hx-get="/dashboard/calendar?partial=grid&amp;month=2026-11" hx-target="#calendar-grid" hx-swap="outerHTML" class="calendar-nav-button" aria-label="Próximo mes"><span class="calendar-nav-label">Siguiente</span><span class="material-symbols-outlined">chevron_right</span></button></div><div class="calendar-month-grid agenda-month-grid">${monthCells}</div>`;
const upcomingItems = [[28, 'oct', 'Examen de sistemas', 'Miércoles', '08:30'], [30, 'oct', 'Reunión de tutoría', 'Viernes', 'Sin hora']].map(([day, month, title, weekday, time]) => `<a class="nx-calendar-event" href="/dashboard/calendar/daily?date=2026-10-${day}"><time class="nx-calendar-event-date" datetime="2026-10-${day}"><strong>${day}</strong><span>${month}</span></time><div class="nx-calendar-event-copy"><span>Evento</span><h3>${title}</h3><p>${weekday} · ${time}</p></div><span class="material-symbols-outlined nx-calendar-event-arrow">arrow_outward</span></a>`).join('');
const agendaSummary = '<div class="agenda-summary" aria-label="Resumen del mes"><article><span>Días con horario</span><strong>22</strong><small>Bloques lectivos del mes</small></article><article><span>Entregas</span><strong>1</strong><small>Tareas con fecha</small></article><article><span>Eventos</span><strong>3</strong><small>Actividades registradas</small></article></div>';
const upcoming = `<aside class="agenda-upcoming nx-upcoming nova-calendar-upcoming"><div class="agenda-upcoming-head"><div><p>A continuación</p><h2>Próximos eventos</h2></div><span class="material-symbols-outlined">upcoming</span></div><p>Lo que viene después, ordenado por fecha.</p><div class="nx-calendar-events" id="event-timeline">${upcomingItems}</div><a class="agenda-upcoming-link" href="/dashboard/calendar/weekly">Ver la semana completa<span class="material-symbols-outlined">arrow_forward</span></a></aside>`;
const weeklyColumns = weekDays.map((day, index) => `<section class="nova-weekly-day"><header><span>${day.slice(0, 3)}</span><strong>${day}</strong></header><div>${index < 5 ? `<article><span>Clase</span><h3>${index % 2 ? 'Redes y sistemas' : 'Algoritmos'}</h3><time datetime="2026-10-${19 + index}T08:00" title="${19 + index}/10/2026">08:00</time></article>` : '<p class="nova-weekly-empty">Sin eventos</p>'}</div></section>`).join('');

const examBody = `<div class="nx-exam-summary"><span class="material-symbols-outlined">monitoring</span><p>3 evaluaciones confirmadas · 2 próximas · 1 posible del calendario</p></div><div class="nova-exam-timeline" id="event-timeline"><section class="nova-exam-group"><div class="nova-exam-group-head"><h2>Próximos 7 días</h2><span>2 evaluaciones</span></div><div class="nova-exam-group-list">${[['28 oct. · 08:30', 'Sistemas operativos: concurrencia', 'Auditorio central'], ['30 oct. · 10:00', 'Redes: routing dinámico', 'Laboratorio 3']].map(([date, title, place]) => `<article class="nova-exam-card"><div class="nova-exam-date"><span class="material-symbols-outlined">event</span>${date}</div><div class="nova-exam-copy"><h3>${title}</h3><p>${place}</p></div><div class="nova-exam-badges">${badge('Mikareno', 'lime')}</div></article>`).join('')}</div></section></div>`;

const lifeCards = ['Asistencia', 'Puntualidad', 'Uniforme', 'Comportamiento', 'Tareas', 'Biblioteca'].map((title, index) => `<details class="nova-life-card" id="sl-${index}" ${index === 0 ? 'open' : ''}><summary class="nova-life-summary-row"><div><span class="nova-eyebrow">0${index + 1}</span><h2>${title}</h2></div><span class="material-symbols-outlined">expand_more</span></summary><div class="nova-life-body"><p>${index === 0 ? '96 % de asistencia registrada en el periodo actual.' : index === 4 ? '2 tareas pendientes con vencimiento esta semana.' : 'Sin incidencias registradas en esta sección.'}</p><button class="nova-button nova-button-quiet" hx-get="/dashboard/vida-escolar?partial=section&id=${index}" hx-target="#sl-${index}">Actualizar</button></div></details>`).join('');

const profileIdentity = `<section class="nova-profile-identity" id="profile-section"><div class="nova-profile-avatar">A</div><div class="nova-profile-identity-copy"><p>PERFIL DEL ESTUDIANTE</p><h2>Alex Loayza</h2><span>Unidad Educativa Nova</span><strong>Ingeniería · 3.º B</strong></div><button class="nova-icon-button" hx-get="/dashboard/profile?partial=profile" hx-target="#profile-section" hx-swap="outerHTML" aria-label="Actualizar perfil"><span class="material-symbols-outlined">refresh</span></button></section>`;
const profileFacts = `<section class="nova-profile-facts" id="profile-details">${caption('01', 'Datos académicos', 'badge')}<div class="nova-profile-fact-grid"><article><span>Nombre completo</span><strong>Alex Loayza</strong></article><article><span>Institución</span><strong>Unidad Educativa Nova</strong></article><article><span>Curso actual</span><strong>Ingeniería · 3.º B</strong></article><article><span>Estado del MCP</span><strong style="color:var(--nova-lime)">Conectado</strong></article></div></section>`;
const profileProgress = `<section class="nova-profile-progress">${caption('02', 'Progreso por materia', 'monitoring')}<p class="nova-profile-progress-intro">Promedios reales del parcial en curso.</p><div id="profile-progress">${[['Algoritmos', '8.70', 87], ['Redes', '8.20', 82], ['Sistemas operativos', '6.70', 67]].map(([name, grade, value]) => `<div class="nova-profile-progress-row"><div style="display:flex;justify-content:space-between"><span>${name}</span><strong>${grade}</strong></div><div class="nx-subject-progress" role="progressbar" aria-valuenow="${value}" aria-valuemin="0" aria-valuemax="100"><div class="bg-primary-container" style="width:${value}%"></div></div><small>${value}% · Mikareno</small></div>`).join('')}</div></section>`;

const fileCard = (kind, title, icon, href, meta, actions = '') => `<article class="gen-office-file-card ${kind}"><a class="gen-office-file-preview" href="${href}" aria-label="Abrir ${title}"><span class="material-symbols-outlined">${icon}</span><small>${kind === 'docs' ? 'MARKDOWN' : kind === 'sheets' ? 'CSV' : 'PRESENTACIÓN'}</small></a><div class="gen-office-file-copy"><a href="${href}">${title}</a><span>${meta}</span></div>${actions ? '' : `<a class="gen-office-file-arrow" href="${href}" aria-label="Abrir ${title}"><span class="material-symbols-outlined">arrow_outward</span></a>`}${actions ? `<footer>${actions}</footer>` : ''}</article>`;
const docActions = (title) => `<a href="/office/docs/demo/export?fmt=html">HTML</a><a href="/office/docs/demo/export?fmt=md">MD</a><form hx-delete="/office/docs/demo"><button type="submit" class="gen-office-icon-button danger" aria-label="Borrar ${title}"><span class="material-symbols-outlined">delete</span></button></form>`;
const sheetActions = '<a href="/office/sheets/demo/export">Exportar CSV</a><form hx-post="/office/sheets/demo/delete"><button type="submit" class="gen-office-icon-button danger" aria-label="Borrar presupuesto"><span class="material-symbols-outlined">delete</span></button></form>';
const presentationActions = '<a href="/office/presentations/demo/present" target="_blank">Presentar</a><form hx-delete="/office/presentations/demo"><button type="submit" class="gen-office-icon-button danger" aria-label="Borrar presentación"><span class="material-symbols-outlined">delete</span></button></form>';
const docCards = fileCard('docs', 'apuntes-de-algoritmos', 'description', '/office/docs/demo', '23 oct. 2026 · 4,2 KB', docActions('apuntes de algoritmos')) + fileCard('docs', 'ensayo-de-redes', 'description', '/office/docs/demo', '21 oct. 2026 · 2,8 KB', docActions('ensayo de redes'));
const sheetCards = fileCard('sheets', 'presupuesto-del-trimestre', 'table_chart', '/office/sheets/demo', '23 oct. 2026 · 1,4 KB', sheetActions);
const presentationCards = fileCard('presentations', 'sistemas-operativos', 'present_to_all', '/office/presentations/demo', '5 diapositivas · 3,1 KB', presentationActions);
const recentCards = fileCard('docs', 'apuntes-de-algoritmos', 'description', '/office/docs/demo', '23 oct. 2026 · 4,2 KB') + fileCard('docs', 'ensayo-de-redes', 'description', '/office/docs/demo', '21 oct. 2026 · 2,8 KB') + fileCard('sheets', 'presupuesto-del-trimestre', 'table_chart', '/office/sheets/demo', '23 oct. 2026 · 1,4 KB') + fileCard('presentations', 'sistemas-operativos', 'present_to_all', '/office/presentations/demo', '5 diapositivas · 3,1 KB');

const chatSessionsDemo = `<div class="nx-session-group">Hoy</div><button type="button" class="nx-session is-active" data-chat-subject="Sistemas operativos"><span class="nx-session-top"><strong>Semáforos y concurrencia</strong><time>16:30</time></span><span class="nx-session-preview">Un semáforo coordina el acceso a recursos compartidos.</span><span class="nx-session-footer"><span>Sistemas operativos</span><span><span class="material-symbols-outlined">chat_bubble</span>6</span></span></button><button type="button" class="nx-session" data-chat-subject="Redes"><span class="nx-session-top"><strong>Routing dinámico</strong><time>14:10</time></span><span class="nx-session-preview">Comparación entre RIP y OSPF con ejemplos.</span><span class="nx-session-footer"><span>Redes</span><span><span class="material-symbols-outlined">chat_bubble</span>4</span></span></button>`;
const chatMessagesDemo = `<div class="nx-message nx-message-user" data-chat-turn="user"><div class="nx-message-avatar nx-message-user-avatar">TÚ</div><div class="nx-message-content"><div class="nx-message-meta"><span class="nx-message-author">Tú</span><span class="nx-message-time">16:28</span></div><div class="nx-message-bubble"><p class="nx-message-text">¿Puedes explicarme qué es un semáforo con un ejemplo sencillo?</p></div></div></div><div class="nx-message nx-message-assistant" data-chat-turn="assistant"><div class="nx-message-avatar nx-message-assistant-avatar"><span class="material-symbols-outlined">auto_awesome</span></div><div class="nx-message-content"><div class="nx-message-meta"><span class="nx-message-author">MiniMax M3</span><span class="nx-message-subject">Sistemas operativos</span><span class="nx-message-time">16:30</span></div><div class="nx-message-bubble"><p class="nx-message-text">Un semáforo es una herramienta para coordinar procesos que necesitan compartir un recurso.

Imagina una sala de estudio con tres ordenadores. El semáforo empieza en 3: cada estudiante que entra resta uno y, cuando llega a cero, los demás esperan. Al salir, suma uno y avisa al siguiente.

Así se evita que demasiados procesos utilicen algo a la vez.</p></div></div></div>`;

const previewChatClasses = {
  user_row: 'nx-message nx-message-user nova-message nova-message-user flex-row-reverse',
  user_avatar: 'nx-message-avatar nx-message-user-avatar nova-message-avatar nova-message-avatar-user',
  user_col: 'nx-message-content nova-message-content', user_head: 'nx-message-meta nova-message-meta',
  user_name: 'nx-message-author nova-message-author', user_bubble: 'nx-message-bubble nova-message-bubble', user_text: 'nx-message-text nova-message-text',
  assistant_row: 'nx-message nx-message-assistant nova-message nova-message-assistant',
  assistant_avatar: 'nx-message-avatar nx-message-assistant-avatar nova-message-avatar nova-message-avatar-assistant',
  assistant_col: 'nx-message-content nova-message-content', assistant_head: 'nx-message-meta nova-message-meta',
  assistant_name: 'nx-message-author nova-message-author', assistant_bubble: 'nx-message-bubble nova-message-bubble', assistant_text: 'nx-message-text nova-message-text',
  subject_chip: 'nx-message-subject nova-message-subject', time: 'nx-message-time nova-message-time',
  context_subject: 'nx-context-subject nova-context-subject', context_count: 'nx-context-count nova-context-count',
};
const previewStreamScript = rawAfter('src/handlers/stitch_chat.rs', 'const STREAM_SCRIPT', 2)
  .replaceAll('__SUBJECT__', JSON.stringify('Sistemas operativos'))
  .replaceAll('__CLASSES__', JSON.stringify(previewChatClasses))
  .replaceAll('__LIMIT__', '20')
  .replaceAll('__MODEL__', JSON.stringify('MiniMax M3'));


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
    CHAT_FCOUNT: '0', CHAT_FILTERS: '<select id="chat-subject-select" class="nx-chat-subject-select" title="Materia de la conversación"><option>Sistemas operativos</option><option>Redes</option><option value="__new__">+ Nueva materia…</option></select><input id="chat-subject-new" class="nx-chat-subject-new" style="display:none" placeholder="Nombre de la materia">',
    CHAT_SESSIONS: chatSessionsDemo,
    CHAT_MODE_DOT: '<span style="color:var(--nova-lime)">●</span>', CHAT_MODEL: 'MiniMax M3', CHAT_QUOTA_PCT: '12 %',
    CHAT_QUOTA_BAR: '<div style="width:12%;height:100%;background:var(--nova-violet)"></div>', CHAT_COSTS: '$0.42 / $25.00', CHAT_MODE: 'Conectado',
    CHAT_CONTEXT: '<span class="nx-context-subject">Sistemas operativos</span><span class="nx-context-count">2 mensajes</span>',
    CHAT_MESSAGES: chatMessagesDemo,
    CHAT_COMPOSER_MODE: 'Respuesta detallada', CHAT_SOURCES: '<div><strong>Aún no hay fuentes</strong><p>Envía una pregunta para recuperar las notas y citas del modo elegido.</p></div>',
  }) + `<script>${previewStreamScript}</script>`);

  add('/ai', rawContaining('src/ai/handlers.rs', 'nova-conversations-page', 2)
    .replaceAll('__MODEL__', 'minimax-m2').replaceAll('__MODE_COLOR__', 'text-green-400')
    .replaceAll('__MODE_LABEL__', 'Conectado').replaceAll('__MODE_DOT__', 'bg-green-400'));

  const graph = '<svg viewBox="0 0 220 190" aria-label="Grafo de notas"><g stroke="rgba(169,182,255,.25)"><line x1="110" y1="95" x2="45" y2="45"/><line x1="110" y1="95" x2="180" y2="50"/><line x1="110" y1="95" x2="55" y2="155"/><line x1="110" y1="95" x2="175" y2="150"/></g><g fill="#a9b6ff"><circle cx="110" cy="95" r="13"/><circle cx="45" cy="45" r="7"/><circle cx="180" cy="50" r="7"/><circle cx="55" cy="155" r="7"/><circle cx="175" cy="150" r="7"/></g></svg>';
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
    MCP_ORCHESTRATOR: '<strong><span class="text-secondary">●</span> Activo</strong>', MCP_LATENCY: '42 ms', MCP_SPEND: '$0.42',
    MCP_SERVERS_LINE: '3 servicios registrados · 2 externos · 1 nativo', MCP_STATUS_MIKARENO: badge('Conectado', 'lime'), MCP_URL_MIKARENO: 'mikareno.local/mcp', MCP_TOOLS: '18 tools',
    MCP_STATUS_GENOFFICE: badge('Conectado', 'lime'), MCP_URL_GENOFFICE: 'genoffice.local/mcp', MCP_NATIVE_LOCATION: badge('Integrado', 'lime'), MCP_NATIVE_TOOLS: '8 tools',
    MCP_STATUS_AI: '<span id="live-connection-tag" class="nova-version-badge">● API activa</span>', MCP_KEY_STATE: badge('Configurada', 'lime'),
    MCP_MODEL_OPTIONS: '<option>minimax/minimax-m2</option><option>openrouter/auto</option>', MCP_BUDGET: '$25.00 USD', MCP_BUDGET_INT: '25', MCP_MODEL: 'minimax-m2',
    MCP_TRANSCRIPT: '<div class="nova-chat-welcome"><span class="material-symbols-outlined">terminal</span><p>Registro de pruebas local.</p></div>',
    MCP_TELEMETRY: '<div class="nx-admin-metrics nx-telemetry-metrics"><article><span class="material-symbols-outlined">forum</span><div><small>Solicitudes</small><strong>128</strong><p>este mes</p></div></article><article><span class="material-symbols-outlined">payments</span><div><small>Gasto</small><strong>$0.42</strong><p>USD acumulados</p></div></article><article><span class="material-symbols-outlined">account_balance_wallet</span><div><small>Presupuesto</small><strong>$25.00</strong><p>2% utilizado</p></div></article></div><div class="nx-quota-track"><div style="width:2%;height:100%;background:var(--nx-accent)"></div></div>',
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
  add('/dashboard/calendar', fmt(rawContaining('src/academic/calendar.rs', 'nova-calendar-page', 3), [agendaSummary, '2026-10', '', monthGrid, upcoming]));
  add('/dashboard/calendar/daily', fmt(rawContaining('src/academic/calendar.rs', 'nova-daily-page', 3), ['Viernes', '23 de octubre de 2026', 'Agenda diaria', badge('Hoy', 'lime'), status, '<nav class="nova-daily-nav"><a class="nova-button nova-button-quiet" href="?date=2026-10-22">‹ Jueves</a><strong>Viernes 23</strong><a class="nova-button nova-button-quiet" href="?date=2026-10-24">Sábado ›</a></nav>', '', '<div id="daily-events" class="nova-exam-group-list"><article class="nova-exam-card"><div class="nova-exam-date">08:00</div><div class="nova-exam-copy"><h3>Algoritmos</h3><p>Clase · Aula 204</p></div></article><article class="nova-exam-card"><div class="nova-exam-date">23:59</div><div class="nova-exam-copy"><h3>Entrega de laboratorio</h3><p>Tarea pendiente</p></div></article></div>']));
  add('/dashboard/calendar/weekly', fmt(rawContaining('src/academic/calendar.rs', 'nova-weekly-page', 3), [status, weeklyColumns]));
  add('/dashboard/examenes', fmt(rawContaining('src/academic/exams.rs', 'nova-exams-page', 3), [], { status, aviso_html: '', body: examBody }));
  add('/dashboard/horario', fmt(rawContaining('src/handlers/schedule.rs', 'nova-schedule-page', 1), ['19 — 25 de octubre de 2026', scheduleSource, scheduleGrid]));
  add('/dashboard/vida-escolar', fmt(rawContaining('src/academic/school_life.rs', 'nova-life-page', 3), [], { resumen: '<div class="nova-life-overview"><p>6 secciones con datos · actualización manual disponible</p></div>', banner: status, aviso: '', accordion: lifeCards }));

  add('/dashboard/search', fmt(rawContaining('src/handlers/search.rs', 'nova-search-page', 1), [], {
    badge: badge('3 resultados'), form: '<form method="get" action="/dashboard/search" class="nx-search-form"><span class="material-symbols-outlined">search</span><label for="search-input" class="sr-only">Buscar notas</label><input id="search-input" type="search" name="q" value="concurrencia"><button type="submit" class="nx-button nx-button-primary">Buscar<span class="material-symbols-outlined">arrow_forward</span></button></form>',
    body: '<ul class="nova-search-results"><li class="nova-search-result"><span class="nova-search-result-icon"><span class="material-symbols-outlined">description</span></span><div><a href="/dashboard/notes">Concurrencia y semáforos</a><p>Un semáforo coordina el acceso a recursos compartidos…</p></div><a href="/dashboard/notes" class="nova-icon-button" aria-label="Abrir"><span class="material-symbols-outlined">arrow_outward</span></a></li><li class="nova-search-result"><span class="nova-search-result-icon"><span class="material-symbols-outlined">description</span></span><div><a href="/dashboard/notes">Productor y consumidor</a><p>Problema clásico de sincronización entre procesos…</p></div></li><li class="nova-search-result"><span class="nova-search-result-icon"><span class="material-symbols-outlined">description</span></span><div><a href="/dashboard/notes">Exclusión mutua</a><p>Cómo evitar que dos procesos usen un recurso a la vez…</p></div></li></ul>',
  }));
  add('/dashboard/profile', fmt(rawContaining('src/academic/profile.rs', 'nova-profile-page', 3), [profileIdentity, status, profileFacts, profileProgress]));

  const detailHeader = fmt(rawContaining('src/academic/subject_detail.rs', 'nova-subject-detail-hero', 3), ['Sistemas operativos', 'SO-220', '2', '<a href="/dashboard/notes" class="inline-flex items-center gap-1 mt-space-xs font-label-sm text-[11px] text-primary-container bg-primary-container/10 px-space-sm py-0.5 rounded-full hover:bg-primary-container/20 transition-colors"><span class="material-symbols-outlined text-[14px]">sticky_note_2</span> Nota de la asignatura</a>']);
  const detailSummary = fmt(rawContaining('src/academic/subject_detail.rs', 'nova-subject-summary', 3), ['text-primary-container', '6.70', '10.0', 'bg-primary-container', '67', '6.70', 'SO-220', '10.0', 'No', 'Cuantitativa']);
  const detailRows = `<tr><td>Taller de planificación</td><td>04/10/2026</td><td>Práctica 3</td><td>9.00</td><td>${badge('Calificada', 'lime')}</td><td>Sin nota</td></tr><tr><td>Quiz de memoria virtual</td><td>11/10/2026</td><td>Quiz</td><td>7.50</td><td>${badge('Calificada', 'lime')}</td><td>${badge('Con nota', 'lime')}</td></tr><tr><td>Proyecto de procesos</td><td>16/10/2026</td><td>Proyecto</td><td>8.00</td><td>${badge('Calificada', 'lime')}</td><td>Sin nota</td></tr><tr><td>Laboratorio de procesos</td><td>18/10/2026</td><td>Práctica 4</td><td>8.50</td><td>${badge('Calificada', 'lime')}</td><td>${badge('Con nota', 'lime')}</td></tr><tr><td>Práctica de sincronización</td><td>21/10/2026</td><td>Laboratorio</td><td>8.00</td><td>${badge('Calificada', 'lime')}</td><td>Sin nota</td></tr><tr><td>Examen de concurrencia</td><td>28/10/2026</td><td>Parcial 2</td><td>—</td><td>${badge('Pendiente')}</td><td>Sin nota</td></tr><tr><td>Entrega: scheduler</td><td>30/10/2026</td><td>Proyecto</td><td>—</td><td>${badge('Pendiente')}</td><td>Sin nota</td></tr>`;
  const activityLabels = ['Actividad', 'Fecha', 'Descripción', 'Nota', 'Estado', 'Nota de la app'];
  let activityCell = 0;
  const labeledDetailRows = detailRows.replace(/<td>/g, () => `<td data-label="${activityLabels[activityCell++ % activityLabels.length]}">`);
  const detailActivities = fmt(rawContaining('src/academic/subject_detail.rs', 'nova-subject-activities', 3), ['Evaluaciones y tareas', '7', '5', '8.20', '2 de 7 enlazadas a notas', labeledDetailRows]);
  add('/dashboard/subjects/demo', `<div class="nx-page nx-subject-detail">${status}${detailHeader}${detailSummary}<aside class="nx-subject-observation"><span class="material-symbols-outlined">sticky_note_2</span><div><h2>Observación</h2><p>Repasar sincronización antes del próximo parcial.</p></div></aside>${detailActivities}</div>`);
  add('/dashboard/subjects/missing', fmt(rawContaining('src/academic/subject_detail.rs', 'nova-subject-missing', 3), [], { status, code: 'NO-404' }));

  const officeTabs = (kind, title, newHref) => {
    const [icon, label, listHref] = {
      docs: ['description', 'Documentos', '/office/docs'],
      sheets: ['table_chart', 'Hojas', '/office/sheets'],
      presentations: ['present_to_all', 'Presentaciones', '/office/presentations'],
      library: ['folder_open', 'Biblioteca', '/office/library'],
    }[kind] || ['grid_view', 'Oficina', '/office'];
    return fmt(rawContaining('src/office/templates/mod.rs', 'gen-office-file-tab is-active', 2), [], {
      list_href: listHref, kind, icon, title, label, new_href: newHref, feedback_js: officeFeedbackJs,
    });
  };
  const officeAiToggle = rawAfter('src/office/templates/mod.rs', 'pub const OFFICE_AI_TOGGLE', 2);
  const officeFeedbackJs = rawAfter('src/office/templates/mod.rs', 'pub const OFFICE_FEEDBACK_JS', 2);
  const officeWorkspaceJs = rawAfter('src/office/templates/mod.rs', 'pub const OFFICE_WORKSPACE_JS', 2);
  const officeAiPanel = (title, description, content) => fmt(
    rawContaining('src/office/templates/mod.rs', 'gen-office-ai-backdrop', 2), [],
    { title, description, content },
  );

  add('/office', fmt(rawContaining('src/office/documents.rs', 'gen-office-home-hero', 2), [], {
    docs: '2', sheets: '1', pres: '1', recent: recentCards, mcp: '',
  }));
  add('/office/docs', fmt(rawContaining('src/office/documents.rs', 'gen-office-manager-head', 2), [], {
    tabs: officeTabs('docs', 'Documentos', '/office/docs/new'), count: '2', cards: docCards, mcp: '',
  }));
  add('/office/sheets', fmt(rawContaining('src/office/spreadsheets.rs', 'gen-office-manager-head', 2), [], {
    tabs: officeTabs('sheets', 'Hojas', '/office/sheets/new'), count: '1', cards: sheetCards,
  }));
  add('/office/presentations', fmt(rawContaining('src/office/presentations.rs', 'gen-office-manager-head', 2), [], {
    tabs: officeTabs('presentations', 'Presentaciones', '/office/presentations/new'), count: '1', cards: presentationCards,
  }));

  const toolbarJs = rawAfter('src/office/templates/mod.rs', 'pub const MARKDOWN_TOOLBAR_JS', 1);
  const gridJs = rawAfter('src/office/templates/mod.rs', 'pub const GRID_SERIALIZE_JS', 1);
  const officeIntegration = (source, marker, extras = {}) => fmt(rawContaining(source, marker, 2), [], {
    deck_js: '', present_js: '', ...extras,
    tabs: officeTabs(extras.active || 'docs', extras.file_label || extras.title || 'Archivo', extras.new_href || '/office/docs/new'),
    feedback_js: officeFeedbackJs, workspace_js: officeWorkspaceJs, grid_js: gridJs,
  });
  add('/office/docs/new', officeIntegration('src/office/documents.rs', 'gen-office-create-page', {
    file_label: 'Nuevo documento',
  }));
  const docEditor = fmt(rawContaining('src/office/documents.rs', 'gen-office-doc-toolbar', 2), [], {
    slug: 'demo', content: '# Apuntes de algoritmos\n\n## HeapSort\n\nLa ordenación por montículos tiene complejidad **O(n log n)**.', js: toolbarJs,
  });
  const docTools = '<div class="gen-office-tool-list"><a href="/office/docs/demo/preview"><span class="material-symbols-outlined">visibility</span><span><strong>Revisar resultado</strong><small>Abrir la vista previa renderizada</small></span></a><a href="/office/docs/demo/export?fmt=html"><span class="material-symbols-outlined">download</span><span><strong>Exportar como HTML</strong><small>Descargar una página independiente</small></span></a></div>';
  add('/office/docs/demo', officeIntegration('src/office/documents.rs', 'gen-office-doc-workspace', {
    file_label: 'apuntes de algoritmos', saved: '', ai_toggle: officeAiToggle,
    ai: officeAiPanel('Herramientas del documento', 'Revisa, exporta o continúa editando. NOVA AI se abre aparte para no alterar tu archivo sin confirmación.', docTools),
    title: 'apuntes de algoritmos', slug: 'demo', editor: docEditor, mcp: '',
  }));
  add('/office/docs/demo/preview', officeIntegration('src/office/documents.rs', 'gen-office-preview-main', {
    file_label: 'apuntes de algoritmos', mcp: '', title: 'apuntes de algoritmos', slug: 'demo',
    html: '<h1>Apuntes de algoritmos</h1><h2>HeapSort</h2><p>La ordenación por montículos tiene complejidad <strong>O(n log n)</strong>.</p><pre><code>heapify(values)</code></pre>',
  }));
  add('/office/sheets/new', officeIntegration('src/office/spreadsheets.rs', 'gen-office-create-page', {
    active: 'sheets', file_label: 'Nueva hoja', new_href: '/office/sheets/new',
  }));
  add('/office/presentations/new', officeIntegration('src/office/presentations.rs', 'gen-office-create-page', {
    active: 'presentations', file_label: 'Nueva presentación', new_href: '/office/presentations/new',
  }));

  const editable = `<table id="grid" class="gen-office-sheet-grid"><thead><tr><th class="gen-office-row-corner"></th>${['A', 'B', 'C', 'D', 'E'].map((name) => `<th scope="col">${name}</th>`).join('')}</tr></thead><tbody>${Array.from({ length: 5 }, (_, row) => `<tr><th scope="row">${row + 1}</th>${Array.from({ length: 5 }, (_, col) => {
    const value = row === 0 ? ['Concepto', 'Horas', 'Nota', 'Total', 'Media'][col] : row === 1 ? ['Examen', '2', '8.5', '=SUM(C2:C3)', '=AVG(C2:C3)'][col] : row === 2 ? ['Laboratorio', '3', '9.0', '', ''][col] : '';
    const ref = `${String.fromCharCode(65 + col)}${row + 1}`;
    return `<td contenteditable="true" data-r="${row}" data-c="${col}" data-cell="${ref}" aria-label="Celda ${ref}">${value}</td>`;
  }).join('')}</tr>`).join('')}</tbody></table>`;
  const values = '<table class="gen-office-sheet-values"><tbody><tr><td>Examen</td><td>2</td><td>8.5</td><td>17.5</td><td>8.75</td></tr><tr><td>Laboratorio</td><td>3</td><td>9.0</td><td></td><td></td></tr></tbody></table>';
  const sheetGrid = fmt(rawContaining('src/office/spreadsheets.rs', 'gen-office-formula-bar', 2), [], {
    slug: 'demo', row_count: '5', col_count: '5', editable, values, header: 'A B C D E', grid_js: gridJs,
  });
  const sheetTools = '<div class="gen-office-tool-list"><button type="button" onclick="addOfficeGridRow()"><span class="material-symbols-outlined">add_row_below</span><span><strong>Insertar fila</strong><small>Amplía la cuadrícula al final</small></span></button><button type="button" onclick="addOfficeGridColumn()"><span class="material-symbols-outlined">add_column_right</span><span><strong>Insertar columna</strong><small>Añade una columna editable</small></span></button></div><details class="gen-office-import-panel"><summary><span class="material-symbols-outlined">upload_file</span><span><strong>Importar CSV</strong><small>Pega datos para sustituir la cuadrícula</small></span></summary><form hx-post="/office/sheets/demo/import"><label for="sheet-import-data">Contenido CSV</label><textarea id="sheet-import-data" name="csv_data" rows="8"></textarea><button class="gen-office-secondary-button" type="submit">Importar datos</button></form></details>';
  add('/office/sheets/demo', officeIntegration('src/office/spreadsheets.rs', 'gen-office-sheet-workspace', {
    active: 'sheets', file_label: 'presupuesto del trimestre', new_href: '/office/sheets/new', ai_toggle: officeAiToggle,
    ai: officeAiPanel('Herramientas de la hoja', 'Añade filas o columnas, importa CSV y conserva fórmulas compatibles con SUM y AVG.', sheetTools),
    title: 'presupuesto del trimestre', slug: 'demo', grid: sheetGrid,
  }));

  const slideTitles = ['Concurrencia', 'Semáforos', 'Productor y consumidor', 'Interbloqueos', 'Memoria virtual'];
  const thumbTemplate = rawContaining('src/office/presentations.rs', 'gen-office-thumb-select', 2);
  const editorTemplate = rawContaining('src/office/presentations.rs', 'data-slide-panel', 2);
  let thumbnails = '';
  let editors = '';
  slideTitles.forEach((title, index) => {
    const content = `## ${title}\n\nIdea principal de la diapositiva.`;
    const preview = `<h2>${title}</h2><p>Idea principal de la diapositiva.</p>`;
    thumbnails += fmt(thumbTemplate, [], {
      i: String(index), number: String(index + 1), total: '5', pressed: index === 0 ? 'true' : 'false', slug: 'demo', title, content,
    });
    editors += fmt(editorTemplate, [], {
      i: String(index), number: String(index + 1), hidden: index === 0 ? '' : 'hidden', slug: 'demo', title, content, preview, deck_title: 'Sistemas operativos',
    });
  });
  const newSlideTools = fmt(rawContaining('src/office/presentations.rs', 'gen-office-new-slide', 2), [], { slug: 'demo' });
  const deckJs = rawContaining('src/office/presentations.rs', 'var genOfficeDeckSlug', 2).replace('__SLUG__', '"demo"');
  add('/office/presentations/demo', officeIntegration('src/office/presentations.rs', 'gen-office-deck-stage', {
    active: 'presentations', file_label: 'Sistemas operativos', new_href: '/office/presentations/new', slug: 'demo', ai_toggle: officeAiToggle,
    ai: officeAiPanel('Construye la presentación', 'Selecciona una escena, edítala en el lienzo y añade nuevas diapositivas desde este panel.', newSlideTools),
    title: 'Sistemas operativos', thumbnails, editors, position: '1 / 5', total: '5', deck_js: deckJs,
  }));
  const presentTemplate = rawContaining('src/office/presentations.rs', 'data-present-slide', 2);
  const presentSlides = slideTitles.map((title, index) => fmt(presentTemplate, [], {
    index: String(index), hidden: index === 0 ? '' : 'hidden', number: String(index + 1).padStart(2, '0'), total: '05', title,
    body: '<p>Coordinación de procesos y acceso seguro a los recursos compartidos.</p><ul><li>Exclusión mutua</li><li>Sincronización</li><li>Prevención de interbloqueos</li></ul>', deck: 'Sistemas operativos',
  })).join('');
  const presentJs = rawContaining('src/office/presentations.rs', 'var genPresentationIndex', 2).replace('__TOTAL__', '5');
  add('/office/presentations/demo/present', officeIntegration('src/office/presentations.rs', 'gen-office-present-controls', {
    active: 'presentations', file_label: 'Sistemas operativos', new_href: '/office/presentations/new', slug: 'demo', slides: presentSlides,
    disabled: '', position: '1 / 5', present_js: presentJs,
  }));

  const libraryEntries = [
    ['description', 'apuntes-de-algoritmos', 'documento', '4,2 KB', '23 oct.', 'docs'],
    ['table_chart', 'presupuesto-del-trimestre', 'hoja', '1,4 KB', '22 oct.', 'sheets'],
    ['description', 'ensayo-de-redes', 'documento', '2,8 KB', '21 oct.', 'docs'],
    ['present_to_all', 'sistemas-operativos', 'presentación', '3,1 KB', '21 oct.', 'presentations'],
  ];
  const renderLibraryRows = (entries) => entries.map(([icon, name, kind, size, modified, type]) => `<tr><td data-label="Archivo"><a class="gen-office-library-file ${type}" href="/office/${type}/demo"><span class="material-symbols-outlined">${icon}</span><span>${name}</span></a></td><td data-label="Tipo"><span class="gen-office-file-kind">${kind}</span></td><td data-label="Tamaño">${size}</td><td data-label="Modificado">${modified}</td><td><div class="gen-office-library-actions"><a class="gen-office-icon-button" href="/office/${type}/demo" aria-label="Abrir ${name}"><span class="material-symbols-outlined">arrow_outward</span></a><a class="gen-office-icon-button" href="/office/library/${type}/demo/download" aria-label="Descargar ${name}"><span class="material-symbols-outlined">download</span></a><form hx-post="/office/library/${type}/demo/delete"><button type="submit" class="gen-office-icon-button danger" aria-label="Borrar ${name}"><span class="material-symbols-outlined">delete</span></button></form></div></td></tr>`).join('');
  add('/office/library', officeIntegration('src/office/library.rs', 'gen-office-library-layout', {
    active: 'library', file_label: 'Biblioteca', new_href: '#library-import', count: '4', q: '', rows: renderLibraryRows(libraryEntries),
  }));
  add('/office/library/search', officeIntegration('src/office/library.rs', 'gen-office-library-layout', {
    active: 'library', file_label: 'Biblioteca', new_href: '#library-import', count: '1', q: 'algoritmos', rows: renderLibraryRows(libraryEntries.filter((entry) => entry[1].includes('algoritmos'))),
  }));

  return pages;
}
